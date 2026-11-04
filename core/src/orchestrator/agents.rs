//! Agent lifecycle: start, group limits, provider switches, messages, runner events,
//! and the per-agent actor that owns a runner.

use std::fmt::Write as _;
use std::path::PathBuf;

use tokio::sync::mpsc;
use uuid::Uuid;

use super::{AgentCmd, Internal, Orchestrator};
use crate::{
    model::{Agent, AgentState, ContextUse, LogLevel, Provider},
    router::{self, Candidate},
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
        // Agents made before a project lost (or never had) git work in its folder.
        let place = worktree::place_for(&project.root, &agent.name, id);
        let agent = if !worktree::has_git(&project.root) && agent.worktree != place {
            let Some(target) = self.agent_mut(id) else { return };
            target.worktree = place;
            let snapshot = target.clone();
            if let Some(store) = &self.store {
                store.save_agent(&snapshot);
            }
            self.log(Some(id), LogLevel::Info, "no git here: working in the project folder itself");
            snapshot
        } else {
            agent
        };
        let attached = self.attached_places(&agent);
        let context = RunnerContext {
            broker: self.broker.clone(),
            group_id: group.id,
            group_name: group.name.clone(),
            sandbox: self.state.sandbox,
            mcp: self.mcp.clone(),
            team: Some(self.team.clone()),
            extra_dirs: attached.iter().map(|(_, place)| place.clone()).collect(),
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
        tokio::spawn(agent_actor(runner, agent, project.root, attached, rx, self.inbox.clone()));
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
        let note = format!(
            "You are taking over this task from another model ({previous}). The working directory already holds \
             its changes; check them before going on."
        );
        self.continuation(id, &note)
    }

    /// The briefing, `note`, and the conversation so far in brief: what an agent
    /// restarts with. Tool calls and the app's own lines are left out.
    fn continuation(&self, id: Uuid, note: &str) -> Option<String> {
        let agent = self.state.agent(id)?;
        let group = self.state.group(agent.group_id)?;
        let lines: Vec<String> = self
            .state
            .log
            .iter()
            .filter(|e| e.agent_id == Some(id) && is_conversation(e))
            .map(|e| {
                let who = if e.level == LogLevel::Operator { "Operator" } else { "Agent" };
                let text: String = e.text.chars().take(300).collect();
                format!("{who}: {text}")
            })
            .collect();
        let recent = &lines[lines.len().saturating_sub(60)..];
        Some(format!(
            "{}\n\n{note}\nThe conversation so far:\n{}\n\nContinue from where it stopped.",
            self.briefing(agent, group),
            recent.join("\n")
        ))
    }

    /// Fills `Agent::models` from the provider: Claude Code's aliases, the models of a
    /// local Ollama, or a short ACP session that only lists them (no task, no cost).
    pub(super) fn probe_models(&mut self, id: Uuid) {
        let Some(agent) = self.state.agent(id).cloned() else { return };
        if self.running.contains_key(&id) && !agent.models.is_empty() {
            return;
        }
        let inbox = self.inbox.clone();
        let reply = move |event: AgentEvent| {
            let _ = inbox.send(Internal::Agent { id, event });
        };
        match agent.provider {
            Provider::CliMcp { .. } => {
                let available = crate::runner::mcp_cli::CLAUDE_MODELS.map(str::to_owned).to_vec();
                reply(AgentEvent::Models { current: None, available });
            }
            Provider::CliAcp { command } => {
                // Its worktree when it has one, else the project: a CLI may list models per folder.
                let cwd = Some(agent.worktree.clone())
                    .filter(|w| w.is_dir())
                    .or_else(|| self.state.project_of_agent(id).map(|p| p.root.clone()))
                    .unwrap_or_else(std::env::temp_dir);
                self.log(Some(id), LogLevel::Info, "asking its CLI which models it offers…");
                tokio::spawn(async move {
                    match crate::runner::acp::probe_models(&command, &cwd).await {
                        Ok((_, available)) => reply(AgentEvent::Models { current: None, available }),
                        Err(error) => reply(AgentEvent::Notice(format!("no model list: {error}"))),
                    }
                });
            }
            Provider::Local { url, .. } => {
                tokio::spawn(async move {
                    let available: Vec<String> = crate::discovery::ollama_models(&url)
                        .await
                        .into_iter()
                        .filter_map(|option| match option.provider {
                            Provider::Local { model, .. } => Some(model),
                            _ => None,
                        })
                        .collect();
                    reply(AgentEvent::Models { current: None, available });
                });
            }
            Provider::Api { .. } => {
                let text = "this API's models are not listed yet: type the model id (e.g. claude-opus-5-5)";
                self.log(Some(id), LogLevel::Info, text);
            }
        }
    }

    /// The agent's provider (and model) as the router names them.
    pub(super) fn candidate_of(&self, agent: &Agent) -> Option<Candidate> {
        let option = self.state.providers.iter().find(|o| match (&o.provider, &agent.provider) {
            // An API or local provider is the same whatever model it runs.
            (Provider::Api { service: a, .. }, Provider::Api { service: b, .. })
            | (Provider::Local { url: a, .. }, Provider::Local { url: b, .. }) => a == b,
            (a, b) => a == b,
        })?;
        let model = match agent.provider {
            Provider::CliAcp { .. } | Provider::CliMcp { .. } => agent.model.clone(),
            Provider::Api { .. } | Provider::Local { .. } => None,
        };
        Some(Candidate { label: option.label.clone(), provider: agent.provider.clone(), model })
    }

    /// The agent hit its usage limit, or its CLI cannot run the model: set that
    /// model aside and go on with the closest one available, conversation included
    /// (ADR 0020). A model the CLI cannot run is replaced within the same CLI first.
    pub(super) fn fall_back(&mut self, id: Uuid, reason: Reason) {
        let Some(agent) = self.state.agent(id).cloned() else { return };
        let Some(current) = self.candidate_of(&agent) else {
            self.notice(LogLevel::Warn, format!("{} hit a limit on a provider the app does not list", agent.name));
            return;
        };
        self.router.exhaust(&current);
        let next = match reason {
            Reason::Limit => self.router.next(&current, &self.state.providers),
            Reason::UnsupportedModel => self.router.next_model(&current, &self.state.providers),
        };
        let what = match reason {
            Reason::Limit => "hit its limit",
            Reason::UnsupportedModel => "cannot run on this CLI",
        };
        let Some(next) = next else {
            self.notice(
                LogLevel::Error,
                format!("{}: {} {what} and no other model is available", agent.name, current.name()),
            );
            return;
        };
        let (from, to) = (current.name(), next.name());
        if let Some(target) = self.agent_mut(id) {
            target.provider = next.provider;
            target.model = next.model;
            target.models.clear();
            let snapshot = target.clone();
            if let Some(store) = &self.store {
                store.save_agent(&snapshot);
            }
        }
        self.auto_updated.remove(&id);
        let line = match reason {
            Reason::Limit => format!("limit reached on {from} → going on with {to}"),
            Reason::UnsupportedModel => format!("{from} cannot run here → going on with {to}"),
        };
        self.log(Some(id), LogLevel::Warn, &line);
        self.notice(LogLevel::Warn, format!("{}: {from} {what}; continuing on {to}", agent.name));
        self.announce(Priority::Error, Phrase::FellBack { agent: agent.name.clone(), to: to.clone() });
        if let Some(prompt) = self.handoff(id, &from) {
            self.restart_with(id, prompt);
        }
    }

    /// A full context is costly and degrades answers: past `compact_at`, the agent
    /// restarts with its briefing and a summary of the conversation.
    fn compact_if_full(&mut self, id: Uuid) {
        let (Some(limit), true) = (self.compact_at, self.running.contains_key(&id)) else { return };
        let Some(percent) = self.state.agent(id).and_then(|a| a.context).and_then(|c| c.percent()) else { return };
        if percent < limit {
            return;
        }
        let note = "Your earlier conversation was summarised to save tokens; the working directory holds everything \
                    done so far.";
        let Some(prompt) = self.continuation(id, note) else { return };
        self.log(
            Some(id),
            LogLevel::Info,
            &format!("context {percent}% full → going on with a summary to save tokens"),
        );
        if let Some(agent) = self.agent_mut(id) {
            agent.context = None;
        }
        self.restart_with(id, prompt);
    }

    pub(super) fn set_provider(&mut self, id: Uuid, wanted: &str) {
        let Some(provider) = self.find_provider(Some(wanted)) else {
            let labels: Vec<_> = self.state.providers.iter().map(|p| p.label.as_str()).collect();
            self.notice(LogLevel::Warn, format!("unknown provider {wanted}; available: {}", labels.join(", ")));
            return;
        };
        self.switch_provider(id, provider);
    }

    /// Puts the agent on `provider`; a running agent restarts there with a summary
    /// of its conversation.
    fn switch_provider(&mut self, id: Uuid, provider: Provider) {
        let Some(agent) = self.agent_mut(id) else {
            return;
        };
        if agent.provider == provider {
            return;
        }
        let previous = agent.provider.short_name().to_owned();
        // The model and the list belong to the provider left behind.
        let same_cli = std::mem::discriminant(&agent.provider) == std::mem::discriminant(&provider)
            && matches!(provider, Provider::Api { .. } | Provider::Local { .. });
        agent.provider = provider;
        if !same_cli {
            agent.model = None;
            // Claude Code's aliases are known before it runs; other CLIs report theirs.
            agent.models = match agent.provider {
                Provider::CliMcp { .. } => crate::runner::mcp_cli::CLAUDE_MODELS.map(str::to_owned).to_vec(),
                _ => Vec::new(),
            };
        }
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

    /// Another model of the same provider: in the live session for ACP CLIs, by a
    /// restart that keeps the conversation for Claude Code, or in the provider for
    /// API and local models.
    pub(super) fn set_model(&mut self, id: Uuid, model: &str) {
        let Some(agent) = self.state.agent(id).cloned() else { return };
        if model.is_empty() {
            self.notice(LogLevel::Warn, "usage: :model <id> · the palette lists what the agent offers");
            return;
        }
        match agent.provider {
            Provider::Api { service, key_ref, .. } => {
                self.switch_provider(id, Provider::Api { service, model: model.into(), key_ref });
                return;
            }
            Provider::Local { url, .. } => {
                self.switch_provider(id, Provider::Local { url, model: model.into() });
                return;
            }
            Provider::CliAcp { .. } | Provider::CliMcp { .. } => {}
        }
        if !agent.models.is_empty() && !agent.models.iter().any(|m| m == model) {
            self.notice(
                LogLevel::Warn,
                format!("{} does not offer {model}; it offers {}", agent.name, agent.models.join(", ")),
            );
            return;
        }
        let previous = agent.model.clone().unwrap_or_else(|| "its default".into());
        if let Some(target) = self.agent_mut(id) {
            target.model = Some(model.to_owned());
            let snapshot = target.clone();
            if let Some(store) = &self.store {
                store.save_agent(&snapshot);
            }
        }
        self.log(Some(id), LogLevel::Info, &format!("model: {previous} → {model}"));
        let running = self.running.get(&id).cloned();
        match (running, &agent.provider) {
            (Some(runner), Provider::CliAcp { .. }) => {
                let _ = runner.send(AgentCmd::SetModel(model.to_owned()));
                self.notice(LogLevel::Info, format!("{} uses {model} from its next message", agent.name));
            }
            (Some(_), _) => {
                if let Some(prompt) = self.handoff(id, &previous) {
                    self.restart_with(id, prompt);
                }
                self.notice(LogLevel::Info, format!("{} continues on {model}", agent.name));
            }
            (None, _) => self.notice(LogLevel::Info, format!("{} will use {model}", agent.name)),
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
            AgentEvent::Context { used, size } => {
                if let Some(agent) = self.agent_mut(id) {
                    agent.context = Some(ContextUse { used, size });
                }
            }
            AgentEvent::Cost(run) => {
                let base = self.cost_base.get(&id).copied().unwrap_or(0);
                if let Some(agent) = self.agent_mut(id) {
                    agent.cost_micros = base + run;
                }
            }
            AgentEvent::Models { current, available } => {
                if let Some(label) = self.state.agent(id).and_then(|a| self.candidate_of(a)).map(|c| c.label) {
                    self.router.learn_models(&label, available.clone());
                }
                if let Some(agent) = self.agent_mut(id) {
                    agent.models = available;
                    if let Some(current) = current
                        && agent.model.as_deref() != Some(current.as_str())
                    {
                        let line = format!("model: {current}");
                        agent.model = Some(current);
                        self.log(Some(id), LogLevel::Info, &line);
                    }
                }
            }
            AgentEvent::Notice(text) => self.log(Some(id), LogLevel::Warn, &text),
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
                self.compact_if_full(id);
                self.pump();
                self.announce(Priority::Done, Phrase::Done { agent: name, group });
            }
            AgentEvent::Error(error) => {
                if let Some(agent) = self.agent_mut(id) {
                    agent.state = AgentState::Error;
                }
                self.turns_lost(id, "hit an error");
                self.log(Some(id), LogLevel::Error, &router::readable_error(&error));
                // A model the CLI cannot run is not fixed by updating the CLI: an
                // adapter can lag behind the model the account defaults to.
                if router::is_unsupported_model(&error) {
                    self.fall_back(id, Reason::UnsupportedModel);
                    return;
                }
                if router::is_limit(&error) {
                    self.fall_back(id, Reason::Limit);
                    return;
                }
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
                // The next run's cost counts from zero again.
                if let Some(agent) = self.state.agent(id) {
                    self.cost_base.insert(id, agent.cost_micros);
                }
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

/// One task per started agent: creates its worktrees (in the project and in the
/// group's other projects), owns its runner, forwards its events to the orchestrator.
async fn agent_actor(
    mut runner: Box<dyn AgentRunner>,
    agent: Agent,
    root: PathBuf,
    attached: Vec<(PathBuf, PathBuf)>,
    mut commands: mpsc::UnboundedReceiver<AgentCmd>,
    inbox: mpsc::UnboundedSender<Internal>,
) {
    let id = agent.id;
    let span = tracing::info_span!("agent", agent_id = %id, name = %agent.name);
    let _enter = span.enter();
    let mut ready = worktree::ensure(&root, &agent.worktree).await;
    for (root, place) in &attached {
        if ready.is_ok() {
            ready = worktree::ensure(root, place).await.map_err(|e| format!("{}: {e}", root.display()));
        }
    }
    let started = match ready {
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
                Some(AgentCmd::SetModel(model)) => {
                    if let Err(error) = runner.set_model(&model).await {
                        let _ = inbox.send(Internal::Agent { id, event: AgentEvent::Notice(error) });
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

/// Why an agent leaves its model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason {
    /// Out of tokens, quota or rate.
    Limit,
    /// The CLI cannot run the model (an adapter older than the model, say).
    UnsupportedModel,
}

/// A line of the conversation itself: what the operator and the agent said, not
/// tool calls or the app's own notes.
fn is_conversation(entry: &crate::model::LogEntry) -> bool {
    const APP: [&str; 12] = [
        "▸",
        "■",
        "✖",
        "starting with",
        "model:",
        "provider:",
        "stopped",
        "session started",
        "limit reached",
        "asking its CLI",
        "context ",
        "waiting for a slot",
    ];
    matches!(entry.level, LogLevel::Operator | LogLevel::Info)
        && !APP.iter().any(|prefix| entry.text.trim_start().starts_with(prefix))
}
