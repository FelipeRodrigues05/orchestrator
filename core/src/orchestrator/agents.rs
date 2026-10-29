//! Agent lifecycle: start, group limits, provider switches, messages, runner events,
//! and the per-agent actor that owns a runner.

use std::fmt::Write as _;
use std::path::PathBuf;

use tokio::sync::mpsc;
use uuid::Uuid;

use super::{AgentCmd, Internal, Orchestrator};
use crate::{
    model::{Agent, AgentState, LogLevel},
    runner::{AgentEvent, AgentRunner, RunnerContext, ToolStatus, update},
    voice::{Priority, phrases::Phrase},
    worktree,
};

impl Orchestrator {
    /// Starts the agent with its task (and `extra` from the operator, if any).
    pub(super) fn start(&mut self, id: Uuid, extra: Option<String>) {
        if self.running.contains_key(&id) {
            if let Some(text) = extra {
                self.message(id, &text);
            }
            return;
        }
        let Some(agent) = self.state.agent(id).cloned() else {
            return;
        };
        let Some(group) = self.state.group(agent.group_id).cloned() else {
            return;
        };
        let mut prompt = self.briefing(&agent, &group);
        if let Some(extra) = extra {
            self.log(Some(id), LogLevel::Operator, &extra);
            let _ = write!(prompt, "\n\nOperator: {extra}");
        }
        self.launch(id, prompt);
    }

    /// Agents of the group working right now.
    pub(super) fn active_in(&self, group: Uuid) -> usize {
        self.state
            .agents
            .iter()
            .filter(|a| a.group_id == group && matches!(a.state, AgentState::Running | AgentState::AwaitingPermission))
            .count()
    }

    /// Starts queued agents while their groups have free slots.
    pub(super) fn pump(&mut self) {
        let mut index = 0;
        while index < self.waiting.len() {
            let id = self.waiting[index].0;
            let group = self.state.agent(id).and_then(|a| self.state.group(a.group_id)).cloned();
            let free = group.as_ref().is_none_or(|g| g.max_active.is_none_or(|max| self.active_in(g.id) < max));
            if free {
                let (id, prompt) = self.waiting.remove(index);
                self.launch(id, prompt);
            } else {
                index += 1;
            }
        }
    }

    /// Runs the agent on its current provider and sends `prompt` first.
    pub(super) fn launch(&mut self, id: Uuid, prompt: String) {
        let (Some(agent), Some(project)) = (self.state.agent(id).cloned(), self.state.project_of_agent(id).cloned())
        else {
            return;
        };
        let Some(group) = self.state.group(agent.group_id).cloned() else {
            return;
        };
        if group.max_active.is_some_and(|max| self.active_in(group.id) >= max) {
            self.waiting.retain(|(waiting, _)| *waiting != id);
            self.waiting.push((id, prompt));
            if let Some(agent) = self.agent_mut(id) {
                agent.state = AgentState::Queued;
            }
            self.log(Some(id), LogLevel::Info, &format!("waiting for a slot in {}", group.name));
            return;
        }
        // Counts as working right away, so a burst of starts respects the limit.
        if let Some(agent) = self.agent_mut(id) {
            agent.state = AgentState::Running;
        }
        let context = RunnerContext {
            broker: self.broker.clone(),
            group_id: group.id,
            group_name: group.name.clone(),
            sandbox: self.state.sandbox,
            mcp: self.mcp.clone(),
            team: Some(self.team.clone()),
        };
        let runner = match (self.runners)(&agent.provider, context) {
            Ok(runner) => runner,
            Err(error) => {
                self.notice(LogLevel::Error, format!("{}: {error}", agent.name));
                return;
            }
        };
        self.last_prompt.insert(id, prompt.clone());
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(AgentCmd::Send(prompt));
        self.turns.entry(id).or_default().sent += 1;
        self.running.insert(id, tx);
        self.log(Some(id), LogLevel::Info, &format!("starting with {}", agent.provider.short_name()));
        tokio::spawn(agent_actor(runner, agent, project.root, rx, self.inbox.clone()));
    }

    /// Starts the agent with `prompt`, stopping its current run first if needed.
    pub(super) fn restart_with(&mut self, id: Uuid, prompt: String) {
        match self.running.remove(&id) {
            Some(agent) => {
                let _ = agent.send(AgentCmd::Stop);
                self.pending_restart.insert(id, prompt);
            }
            None => self.launch(id, prompt),
        }
    }

    /// The prompt for a new provider taking over: task plus the conversation so far.
    pub(super) fn handoff(&self, id: Uuid, previous: &str) -> Option<String> {
        let agent = self.state.agent(id)?;
        let group = self.state.group(agent.group_id)?;
        let lines: Vec<String> = self
            .state
            .log
            .iter()
            .filter(|e| e.agent_id == Some(id))
            .map(|e| {
                let who = if e.level == LogLevel::Operator { "Operator" } else { "Agent" };
                let text: String = e.text.chars().take(300).collect();
                format!("{who}: {text}")
            })
            .collect();
        let recent = &lines[lines.len().saturating_sub(60)..];
        Some(format!(
            "Group goal: {}\n\nYour task: {}\n\nYou are taking over this task from another model ({previous}). \
             The working directory already holds its changes; check them before going on. \
             The conversation so far:\n{}\n\nContinue from where it stopped.",
            group.goal,
            agent.task,
            recent.join("\n")
        ))
    }

    pub(super) fn set_provider(&mut self, id: Uuid, wanted: &str) {
        let Some(provider) = self.find_provider(Some(wanted)) else {
            let labels: Vec<_> = self.state.providers.iter().map(|p| p.label.as_str()).collect();
            self.notice(LogLevel::Warn, format!("unknown provider {wanted}; available: {}", labels.join(", ")));
            return;
        };
        let Some(agent) = self.agent_mut(id) else {
            return;
        };
        if agent.provider == provider {
            return;
        }
        let previous = agent.provider.short_name().to_owned();
        agent.provider = provider;
        let (name, next) = (agent.name.clone(), agent.provider.short_name().to_owned());
        let snapshot = agent.clone();
        if let Some(store) = &self.store {
            store.save_agent(&snapshot);
        }
        self.auto_updated.remove(&id);
        self.log(Some(id), LogLevel::Info, &format!("provider: {previous} → {next}"));
        if self.running.contains_key(&id) {
            if let Some(prompt) = self.handoff(id, &previous) {
                self.restart_with(id, prompt);
            }
            self.notice(LogLevel::Info, format!("{name} continues on {next}"));
        } else {
            self.notice(LogLevel::Info, format!("{name} will run on {next}"));
        }
    }

    pub(super) fn update(&mut self, agent: Option<Uuid>, plans: Vec<update::Plan>) {
        let inbox = self.inbox.clone();
        tokio::spawn(async move {
            for plan in plans {
                let result = update::run(&plan).await;
                let _ = inbox.send(Internal::Updated { agent, label: plan.label.clone(), result });
            }
        });
    }

    pub(super) fn message(&mut self, id: Uuid, text: &str) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if !self.running.contains_key(&id) {
            self.start(id, Some(text));
            return;
        }
        self.log(Some(id), LogLevel::Operator, &text);
        self.deliver(id, text);
    }

    pub(super) fn agent_event(&mut self, id: Uuid, event: AgentEvent) {
        let (name, group) = self.names(id);
        match event {
            AgentEvent::Text(text) => {
                if let Some(book) = self.turns.get_mut(&id) {
                    book.text.push_str(&text);
                }
                self.output(id, &text);
            }
            AgentEvent::Thought(_) => {}
            AgentEvent::Usage(usage) => {
                if let Some(agent) = self.agent_mut(id) {
                    agent.usage += usage;
                }
            }
            AgentEvent::Tool { title, status } => match status {
                ToolStatus::Started => self.log(Some(id), LogLevel::Info, &format!("▸ {title}")),
                ToolStatus::Failed => self.log(Some(id), LogLevel::Warn, &format!("✖ {title}")),
                ToolStatus::Done => {}
            },
            AgentEvent::TurnEnd { stop_reason } => {
                self.open_lines.remove(&id);
                // The broker's "resolved" may arrive after the turn already ended.
                if let Some(agent) = self.agent_mut(id)
                    && matches!(agent.state, AgentState::Running | AgentState::AwaitingPermission)
                {
                    agent.state = AgentState::Done;
                }
                self.log(Some(id), LogLevel::Info, &format!("■ turn finished ({stop_reason})"));
                self.turn_ended(id);
                self.pump();
                self.announce(Priority::Done, Phrase::Done { agent: name, group });
            }
            AgentEvent::Error(error) => {
                if let Some(agent) = self.agent_mut(id) {
                    agent.state = AgentState::Error;
                }
                self.turns_lost(id, "hit an error");
                self.log(Some(id), LogLevel::Error, &error);
                if update::looks_outdated(&error) {
                    let provider = self.state.agent(id).map(|a| a.provider.clone());
                    match provider.as_ref().and_then(update::plan) {
                        Some(plan) if self.auto_updated.insert(id) => {
                            self.log(
                                Some(id),
                                LogLevel::Warn,
                                &format!("{} looks outdated: updating and retrying", plan.label),
                            );
                            self.announce(Priority::Error, Phrase::Updating { cli: plan.label.clone(), agent: name });
                            self.update(Some(id), vec![plan]);
                            return;
                        }
                        Some(plan) => self.notice(
                            LogLevel::Error,
                            format!("{name}: {} is still too old after updating; switch it with :provider", plan.label),
                        ),
                        None => self.notice(
                            LogLevel::Error,
                            format!("{name}: its CLI looks outdated; update it or :provider another"),
                        ),
                    }
                }
                self.announce(Priority::Error, Phrase::Error { agent: name, group });
            }
            AgentEvent::Exited => {
                self.running.remove(&id);
                self.turns_lost(id, "stopped");
                self.open_lines.remove(&id);
                if let Some(agent) = self.agent_mut(id) {
                    if agent.state != AgentState::Error {
                        agent.state = AgentState::Stopped;
                    }
                    agent.started_at = None;
                }
                self.log(Some(id), LogLevel::Info, "stopped");
                if let Some(prompt) = self.pending_restart.remove(&id) {
                    self.launch(id, prompt);
                }
                self.pump();
            }
        }
        self.dirty = true;
    }
}

/// One task per started agent: creates its worktree, owns its runner, forwards
/// its events to the orchestrator.
async fn agent_actor(
    mut runner: Box<dyn AgentRunner>,
    agent: Agent,
    root: PathBuf,
    mut commands: mpsc::UnboundedReceiver<AgentCmd>,
    inbox: mpsc::UnboundedSender<Internal>,
) {
    let id = agent.id;
    let span = tracing::info_span!("agent", agent_id = %id, name = %agent.name);
    let _enter = span.enter();
    let started = match worktree::ensure(&root, &agent.worktree).await {
        Ok(()) => runner.start(&agent, "").await,
        Err(error) => Err(error),
    };
    let failed = started.is_err();
    let _ = inbox.send(Internal::Started { id, result: started });
    if failed {
        return;
    }
    let mut events = runner.events();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(AgentCmd::Send(text)) => {
                    if let Err(error) = runner.send(text).await {
                        let _ = inbox.send(Internal::Agent { id, event: AgentEvent::Error(error) });
                    }
                }
                Some(AgentCmd::Stop) | None => {
                    let _ = runner.stop().await;
                    break;
                }
            },
            event = events.recv() => match event {
                Some(AgentEvent::Exited) | None => break,
                Some(event) => {
                    let _ = inbox.send(Internal::Agent { id, event });
                }
            },
        }
    }
    let _ = inbox.send(Internal::Agent { id, event: AgentEvent::Exited });
}
