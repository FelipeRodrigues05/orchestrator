//! Internal messages from the app's own actors and the broker's events.

use chrono::Utc;

use super::{Internal, Orchestrator};
use crate::{
    broker::{BrokerEvent, DecisionSource, RuleScope},
    host::HostEvent,
    model::{AgentState, Decision, LogLevel},
    persistence::HistoryEntry,
    voice::{Priority, VoiceEvent, phrases::Phrase},
};

impl Orchestrator {
    pub(super) fn internal(&mut self, message: Internal) {
        match message {
            Internal::Started { id, result } => {
                let (name, group) = self.names(id);
                match result {
                    Ok(()) => {
                        if let Some(agent) = self.agent_mut(id) {
                            agent.state = AgentState::Running;
                            agent.started_at = Some(Utc::now());
                        }
                        self.dirty = true;
                    }
                    Err(error) => {
                        self.running.remove(&id);
                        if let Some(agent) = self.agent_mut(id) {
                            agent.state = AgentState::Error;
                        }
                        self.log(Some(id), LogLevel::Error, &error);
                        self.pump();
                        self.notice(LogLevel::Error, format!("{name} could not start: {error}"));
                        self.announce(Priority::Error, Phrase::StartFailed { agent: name, group });
                    }
                }
            }
            Internal::Agent { id, event } => self.agent_event(id, event),
            Internal::Broker(event) => self.broker_event(event),
            Internal::Voice(VoiceEvent::Status(status)) => {
                self.state.voice = status;
                self.dirty = true;
            }
            Internal::Voice(VoiceEvent::Heard(text)) => {
                self.log(None, LogLevel::Operator, &format!("heard: {text}"));
                self.say(&text, DecisionSource::Voice);
            }
            Internal::Providers(providers) => {
                self.state.providers = providers;
                self.apply_host_provider();
                self.dirty = true;
            }
            Internal::Host(event) => match event {
                HostEvent::Thinking(on) => {
                    self.state.host.thinking = on;
                    self.dirty = true;
                }
                HostEvent::Reply(text) => {
                    self.log(None, LogLevel::Pet, &text);
                    self.speak(Priority::Other, text);
                }
                HostEvent::Action(action) => self.host_action(action),
                HostEvent::Usage(usage) => {
                    self.state.host.usage += usage;
                    self.dirty = true;
                }
                HostEvent::Error(error) => self.notice(LogLevel::Warn, format!("pet: {error}")),
            },
            Internal::Updated { agent, label, result } => match (agent, result) {
                (Some(id), Ok(output)) => {
                    self.log(Some(id), LogLevel::Info, &format!("{label} updated {output}"));
                    let prompt = self.last_prompt.get(&id).cloned().unwrap_or_default();
                    if !prompt.is_empty() {
                        self.restart_with(id, prompt);
                    }
                }
                (Some(id), Err(error)) => {
                    self.log(Some(id), LogLevel::Error, &format!("automatic update failed: {error}"));
                    self.notice(
                        LogLevel::Error,
                        format!("could not update {label}; update it by hand or :provider another"),
                    );
                }
                (None, Ok(output)) => {
                    self.notice(LogLevel::Info, format!("{label} updated {output}"));
                    self.refresh_providers();
                }
                (None, Err(error)) => self.notice(LogLevel::Error, error),
            },
            // Handled in the main loop, where commands can await.
            Internal::Run(_) => {}
            Internal::VoiceDownloaded { voice: id, result } => match result {
                Ok(()) => {
                    if let Some(voice) = &self.voice {
                        voice.set_voice(Some(id.clone()));
                    }
                    if let Some(store) = &self.store {
                        store.set_setting("voice", &id);
                    }
                    self.notice(LogLevel::Info, format!("voice: {id}"));
                }
                Err(error) => self.notice(LogLevel::Error, format!("could not download {id}: {error}")),
            },
            Internal::Merged { agent, result } => match result {
                Ok(text) => {
                    self.log(Some(agent), LogLevel::Info, &text);
                    self.notice(LogLevel::Info, text);
                }
                Err(error) => {
                    self.log(Some(agent), LogLevel::Error, &error);
                    self.notice(LogLevel::Error, error);
                }
            },
            Internal::VoiceInstalled(Ok(())) => {
                if let Some(voice) = &self.voice {
                    voice.reload();
                }
                self.notice(
                    LogLevel::Info,
                    "voice models installed; speech-to-text is ready (Piper is a system package)",
                );
            }
            Internal::VoiceInstalled(Err(error)) => {
                self.notice(LogLevel::Error, format!("voice install failed: {error}"));
            }
            Internal::Team(request) => self.team_request(request),
            Internal::Repositories(found) => self.state.repositories = found,
            Internal::Notice(text, level) => {
                if text.is_empty() {
                    self.refresh_providers();
                } else {
                    self.notice(level, text);
                }
            }
        }
    }

    pub(super) fn broker_event(&mut self, event: BrokerEvent) {
        match event {
            BrokerEvent::Queued(request) => {
                let summary = format!("permission: {} {}", request.tool, request.input_summary());
                if let Some(agent) = self.agent_mut(request.agent_id) {
                    agent.state = AgentState::AwaitingPermission;
                }
                self.log(Some(request.agent_id), LogLevel::Warn, &summary);
                self.state.permissions.push(request);
            }
            BrokerEvent::Resolved { request, decision, source } => {
                self.state.permissions.retain(|p| p.id != request.id);
                let agent_id = request.agent_id;
                let still_waiting = self.state.permissions.iter().any(|p| p.agent_id == agent_id);
                if let Some(agent) = self.agent_mut(agent_id)
                    && agent.state == AgentState::AwaitingPermission
                    && !still_waiting
                {
                    agent.state = AgentState::Running;
                }
                let verdict = match &decision {
                    Decision::Allow => "allowed".to_owned(),
                    Decision::AlwaysAllow(rule) => format!("always allowed as {rule}"),
                    Decision::Deny { reason } => format!("denied: {reason}"),
                };
                // Rule and default decisions are frequent; only operator-facing ones go to the log.
                if !matches!(source, DecisionSource::Rule | DecisionSource::Default) {
                    let level = if decision.allows() { LogLevel::Info } else { LogLevel::Warn };
                    let text =
                        format!("{} {} → {verdict} ({})", request.tool, request.input_summary(), source.as_str());
                    self.log(Some(agent_id), level, &text);
                }
                if let Some(store) = &self.store {
                    store.record(HistoryEntry {
                        agent_id,
                        tool: request.tool.clone(),
                        summary: request.input_summary(),
                        decision: verdict,
                        source: source.as_str().into(),
                    });
                }
            }
            BrokerEvent::RuleLearned { scope: RuleScope::Agent(agent_id), rule } => {
                if let Some(agent) = self.agent_mut(agent_id) {
                    agent.rules.push(rule.clone());
                    let snapshot = agent.clone();
                    if let Some(store) = &self.store {
                        store.save_agent(&snapshot);
                    }
                }
                self.log(Some(agent_id), LogLevel::Info, &format!("rule learned: {rule}"));
                self.share_rule(agent_id, &rule);
            }
            BrokerEvent::RuleLearned { scope: RuleScope::Group(group_id), rule } => {
                if let Some(group) = self.state.groups.iter_mut().find(|g| g.id == group_id) {
                    group.rules.push(rule.clone());
                    let (snapshot, name) = (group.clone(), group.name.clone());
                    if let Some(store) = &self.store {
                        store.save_group(&snapshot);
                    }
                    self.log(None, LogLevel::Info, &format!("rule learned for group {name}: {rule}"));
                }
            }
        }
        self.dirty = true;
    }
}
