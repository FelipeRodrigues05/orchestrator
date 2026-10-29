//! What the operator says or asks for: free text and speech, the pet's actions, the
//! spoken summary, and opening an editor, a terminal or the email.

use std::path::PathBuf;

use uuid::Uuid;

use super::{AgentCmd, Command, Internal, Orchestrator};
use crate::{
    broker::DecisionSource,
    discovery,
    host::{self, HostAction},
    model::{AgentState, Language, LogLevel, Provider, normalize},
    voice::{self, Priority, commands::Spoken, phrases::Phrase},
};

impl Orchestrator {
    /// Gives the pet its model: the operator's choice, or a fast default.
    pub(super) fn apply_host_provider(&mut self) {
        let provider = match &self.host_choice {
            Some(label) => self.find_provider(Some(label)),
            None => host::default_provider(&self.state),
        };
        let provider = provider.filter(|p| matches!(p, Provider::Api { .. } | Provider::Local { .. }));
        let label = provider.as_ref().map(|p| match p {
            Provider::Api { service, model, .. } => format!("{service}/{model}"),
            other => other.short_name().to_owned(),
        });
        if label != self.state.host.provider {
            self.host.set_provider(provider);
            self.state.host.provider = label;
            self.dirty = true;
        }
    }

    pub(super) fn host_action(&mut self, action: HostAction) {
        let missing = |this: &mut Self, name: &str| this.notice(LogLevel::Warn, format!("pet: no agent named {name}"));
        match action {
            HostAction::Message { group, agent, text } => match self.find_agent(group.as_deref(), &agent) {
                Some(id) => self.message(id, &text),
                None => missing(self, &agent),
            },
            HostAction::CreateGroup { name, goal } => self.create_group(&name, &goal),
            HostAction::CreateAgent { group, name, provider, task, persona } => {
                self.create_agent(&group, &name, provider.as_deref(), &task, &persona);
            }
            HostAction::Start { agent } => match self.find_agent(None, &agent) {
                Some(id) => self.start(id, None),
                None => missing(self, &agent),
            },
            HostAction::Stop { agent } => match self.find_agent(None, &agent) {
                Some(id) => {
                    if let Some(runner) = self.running.remove(&id) {
                        let _ = runner.send(AgentCmd::Stop);
                    }
                }
                None => missing(self, &agent),
            },
            HostAction::Answer { agent, answer } => {
                let request = agent
                    .and_then(|name| self.find_agent(None, &name))
                    .and_then(|id| self.state.permissions.iter().find(|p| p.agent_id == id))
                    .map(|p| p.id);
                self.broker.answer(request, answer, DecisionSource::Voice);
            }
            HostAction::SetProvider { agent, provider } => match self.find_agent(None, &agent) {
                Some(id) => self.set_provider(id, &provider),
                None => missing(self, &agent),
            },
            HostAction::OpenEditor { agent, editor } => {
                let agent = agent.and_then(|name| self.find_agent(None, &name));
                let _ = self.inbox.send(Internal::Run(Command::OpenIn { agent, editor }));
            }
            HostAction::OpenTerminal { agent } => {
                let agent = agent.and_then(|name| self.find_agent(None, &name));
                self.open_terminal(agent);
            }
            HostAction::OpenEmail => {
                let _ = self.inbox.send(Internal::Run(Command::OpenEmail));
            }
            HostAction::OpenUrl(url) => match discovery::open_url(&url) {
                Ok(()) => self.notice(LogLevel::Info, format!("opened {url}")),
                Err(error) => self.notice(LogLevel::Error, error.to_string()),
            },
        }
    }

    /// Typed or spoken free text (SPEC "Voz").
    pub(super) fn say(&mut self, text: &str, source: DecisionSource) {
        match voice::commands::parse(text) {
            Spoken::Answer(answer) => {
                if self.state.permissions.is_empty() {
                    self.notice(LogLevel::Warn, "no pending permission request");
                } else {
                    self.broker.answer(None, answer, source);
                }
            }
            Spoken::Summary => {
                let summary = self.summary();
                self.notice(LogLevel::Info, summary.clone());
                self.speak(Priority::Other, summary);
            }
            Spoken::Message { group, agent, text } => {
                // `@pet …`, speech without a name, or nobody selected: the pet decides.
                let (to_pet, text) = match text.trim_start().strip_prefix("@pet") {
                    Some(rest) => (true, rest.trim_start_matches([',', ':', ' ']).to_owned()),
                    None => (false, text),
                };
                let unnamed = agent.is_none();
                let nobody = self.selected.is_none_or(|id| self.state.agent(id).is_none());
                if unnamed
                    && (to_pet || source == DecisionSource::Voice || nobody)
                    && self.state.host.provider.is_some()
                {
                    // Speech is already in the log as "heard: …".
                    if source != DecisionSource::Voice {
                        self.log(None, LogLevel::Operator, &format!("→ pet: {text}"));
                    }
                    self.host.ask(text, self.state.clone(), self.selected);
                    return;
                }
                let target = match agent.as_deref() {
                    Some(name) => self.find_agent(group.as_deref(), name),
                    None => self.selected.filter(|id| self.state.agent(*id).is_some()),
                };
                match target {
                    Some(id) => self.message(id, &text),
                    None if agent.is_some() => {
                        let name = agent.unwrap_or_default();
                        self.notice(LogLevel::Warn, format!("no agent named {name}"));
                        self.announce(Priority::Error, Phrase::NoAgent { name });
                    }
                    None => self.notice(LogLevel::Warn, "select an agent first (j/k), or say \"agente <nome>: …\""),
                }
            }
        }
    }

    pub(super) fn find_agent(&self, group: Option<&str>, name: &str) -> Option<Uuid> {
        let group = group.and_then(|g| self.find_group(g)).map(|g| g.id);
        let wanted = normalize(name);
        let candidates = || self.state.agents.iter().filter(|a| group.is_none_or(|g| a.group_id == g));
        candidates()
            .find(|a| normalize(&a.name) == wanted)
            .or_else(|| candidates().find(|a| normalize(&a.name).starts_with(&wanted)))
            .map(|a| a.id)
    }

    /// The pet's spoken summary (Portuguese, ADR 0013).
    /// The pet's spoken summary, in the operator's language.
    pub(super) fn summary(&self) -> String {
        let english = self.state.voice.language == Language::English;
        let Some(project) = self.state.project() else {
            return if english { "No project open." } else { "Nenhum projeto aberto." }.into();
        };
        let count = |state: AgentState| self.state.agents.iter().filter(|a| a.state == state).count();
        let total = self.state.agents.len();
        let others = self.state.projects.len() - 1;
        let mut parts = vec![match (english, others) {
            (true, 0) => format!("Project {}, {total} agents", project.name),
            (false, 0) => format!("Projeto {}, {total} agentes", project.name),
            (true, n) => format!("{} projects open, {} in focus, {total} agents", n + 1, project.name),
            (false, n) => format!("{} projetos abertos, {} em foco, {total} agentes", n + 1, project.name),
        }];
        for (state, pt, en) in [
            (AgentState::Running, "trabalhando", "working"),
            (AgentState::AwaitingPermission, "esperando permissão", "waiting for permission"),
            (AgentState::Done, "concluídos", "done"),
            (AgentState::Error, "com erro", "with errors"),
        ] {
            let n = count(state);
            if n > 0 {
                parts.push(format!("{n} {}", if english { en } else { pt }));
            }
        }
        let pending = self.state.permissions.len();
        if pending > 0 {
            parts.push(if english {
                format!("{pending} permission requests waiting")
            } else {
                format!("{pending} pedidos de permissão na fila")
            });
        }
        parts.join(", ") + "."
    }

    pub(super) async fn open_email(&mut self) {
        let url = match &self.store {
            Some(store) => store.setting("email_url").await,
            None => None,
        }
        .unwrap_or_else(|| "mailto:".into());
        match discovery::open_url(&url) {
            Ok(()) => self.notice(LogLevel::Info, format!("opened {url}")),
            Err(error) => self.notice(LogLevel::Error, format!("could not open the email: {error}")),
        }
    }

    pub(super) fn open_terminal(&mut self, agent: Option<Uuid>) {
        let path = match agent.and_then(|id| self.state.agent(id)) {
            Some(agent) if agent.worktree.exists() => agent.worktree.clone(),
            _ => match self.state.project() {
                Some(project) => project.root.clone(),
                None => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
            },
        };
        match discovery::open_terminal(&path) {
            Ok(()) => self.notice(LogLevel::Info, format!("terminal in {}", path.display())),
            Err(error) => self.notice(LogLevel::Error, format!("could not open a terminal: {error}")),
        }
    }

    pub(super) async fn open_in(&mut self, agent: Option<Uuid>, editor: Option<String>) {
        let path = match agent.and_then(|id| self.state.agent(id)) {
            Some(agent) => agent.worktree.clone(),
            None => {
                if let Some(project) = self.state.project() {
                    project.root.clone()
                } else {
                    self.notice(LogLevel::Warn, "open a project first");
                    return;
                }
            }
        };
        if !path.exists() {
            self.notice(LogLevel::Warn, format!("{} does not exist yet; start the agent first", path.display()));
            return;
        }
        let last = match &self.store {
            Some(store) if editor.is_none() => store.setting("last_editor").await,
            _ => None,
        };
        let wanted = editor.or(last);
        let chosen = wanted
            .as_deref()
            .and_then(|name| self.state.editors.iter().find(|e| e.name.eq_ignore_ascii_case(name) || e.command == name))
            .or_else(|| self.state.editors.first())
            .cloned();
        let Some(chosen) = chosen else {
            self.notice(LogLevel::Warn, "no editor found on PATH");
            return;
        };
        match discovery::open_in(&chosen, &path) {
            Ok(()) => {
                if let Some(store) = &self.store {
                    store.set_setting("last_editor", &chosen.name);
                }
                self.notice(LogLevel::Info, format!("opened {} in {}", path.display(), chosen.name));
            }
            Err(error) => self.notice(LogLevel::Error, format!("{}: {error}", chosen.name)),
        }
    }
}
