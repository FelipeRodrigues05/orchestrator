//! Dispatch of the UI's `Command`s, and the agent and group commands that have no
//! better home (the settings live in `settings`).

use uuid::Uuid;

use super::{AgentCmd, Command, Internal, Orchestrator};
use crate::{
    broker::{DecisionSource, RuleScope},
    model::{AgentState, LogLevel, PermissionRule},
    worktree,
};

impl Orchestrator {
    pub(super) async fn command(&mut self, command: Command) {
        match command {
            Command::Ping | Command::Shutdown => {}
            Command::OpenProject { root } => self.open_project(root).await,
            Command::FocusProject(id) => self.focus_project(id),
            Command::CloseProject(id) => self.close_project(id),
            Command::CreateGroup { name, goal } => {
                self.ensure_project().await;
                self.create_group(&name, &goal);
            }
            Command::CreateAgent { group, name, provider, task, persona } => {
                self.ensure_project().await;
                self.ensure_workspace_group(&group);
                self.create_agent(&group, &name, provider.as_deref(), &task, &persona);
            }
            Command::SetPersona { agent, persona } => self.set_persona(agent, &persona),
            Command::SetColor { agent, color } => {
                let Some(target) = self.agent_mut(agent) else { return };
                target.color = color;
                let snapshot = target.clone();
                if let Some(store) = &self.store {
                    store.save_agent(&snapshot);
                }
                self.dirty = true;
            }
            Command::SetModel { agent, model } => self.set_model(agent, model.trim()),
            Command::ProbeModels(agent) => self.probe_models(agent),
            Command::SetFallback(text) => self.set_fallback(&text),
            Command::SetCompactAt(percent) => {
                self.compact_at = percent.map(|p| p.clamp(30, 99));
                if let Some(store) = &self.store {
                    store.set_setting("compact_at", &self.compact_at.map_or_else(|| "off".into(), |p| p.to_string()));
                }
                let text = match self.compact_at {
                    Some(p) => format!("agents go on with a summary when their context is {p}% full"),
                    None => "agents keep their whole context (the CLI may still compact it)".into(),
                };
                self.notice(LogLevel::Info, text);
            }
            Command::StartAgent(id) => self.start(id, None),
            Command::StopAgent(id) => self.stop_agent(id),
            Command::RemoveAgent(id) => self.remove_agent(id),
            Command::Message { agent, text } => self.message(agent, &text),
            Command::Say { text, selected } => {
                self.selected = selected.or(self.selected);
                self.say(&text, DecisionSource::Click);
            }
            Command::Answer { request, answer } => self.broker.answer(Some(request), answer, DecisionSource::Click),
            Command::Select(agent) => self.select(agent),
            Command::OpenIn { agent, editor } => self.open_in(agent, editor).await,
            Command::SetApiKey { service, key } => self.set_api_key(service, key),
            Command::SetProvider { agent, provider } => self.set_provider(agent, &provider),
            Command::SetGroupLimit { group, limit } => self.set_group_limit(&group, limit),
            Command::OpenEmail => self.open_email().await,
            Command::OpenTerminal(agent) => self.open_terminal(agent),
            Command::SetEmailUrl(url) => self.set_email_url(&url),
            Command::PetName(name) => self.pet_name(name).await,
            Command::SetLanguage(text) => self.set_language(&text),
            Command::SetVoice(id) => self.set_voice(&id),
            Command::SetWake(name) => self.set_wake(name),
            Command::Merge(id) => self.merge(id),
            Command::Lead { group, agents } => self.lead(&group, agents),
            Command::SetSandbox(on) => self.set_sandbox(on),
            Command::InstallVoice => self.install_voice(),
            Command::AddEditors(editors) => {
                for editor in editors {
                    if !self.state.editors.iter().any(|e| e.name == editor.name) {
                        self.state.editors.push(editor);
                    }
                }
                self.dirty = true;
            }
            Command::SetPermissionTimeout(minutes) => self.set_permission_timeout(minutes),
            Command::SetHostProvider(label) => self.set_host_provider(label),
            Command::UpdateClis => self.update_clis(),
            Command::AddRule { agent, rule } => self.add_rule(agent, rule),
            Command::VoicePress => {
                if let Some(voice) = &self.voice {
                    voice.press();
                }
            }
            Command::VoiceRelease => {
                if let Some(voice) = &self.voice {
                    voice.release();
                }
            }
            Command::SaveLayout(layout) => {
                if let (Some(store), Some(project)) = (&self.store, self.state.focus) {
                    store.save_layout(project, layout.clone());
                    self.layouts.insert(project, Some(layout.clone()));
                }
                self.state.layout = Some(layout);
            }
        }
    }

    fn set_persona(&mut self, agent: Uuid, persona: &str) {
        let Some(target) = self.agent_mut(agent) else { return };
        persona.trim().clone_into(&mut target.persona);
        let snapshot = target.clone();
        if let Some(store) = &self.store {
            store.save_agent(&snapshot);
        }
        let when = if self.running.contains_key(&agent) { " (from its next start)" } else { "" };
        let text = match snapshot.persona.as_str() {
            "" => format!("{} has no persona now{when}", snapshot.name),
            persona => format!("{} is now: {persona}{when}", snapshot.name),
        };
        self.notice(LogLevel::Info, text);
    }

    fn stop_agent(&mut self, id: Uuid) {
        if let Some(index) = self.waiting.iter().position(|(w, _)| *w == id) {
            self.waiting.remove(index);
            if let Some(agent) = self.agent_mut(id) {
                agent.state = AgentState::Stopped;
            }
        }
        if let Some(agent) = self.running.remove(&id) {
            let _ = agent.send(AgentCmd::Stop);
            let (name, _) = self.names(id);
            self.notice(LogLevel::Info, format!("stopping {name}"));
        }
    }

    fn remove_agent(&mut self, id: Uuid) {
        if let Some(agent) = self.running.remove(&id) {
            let _ = agent.send(AgentCmd::Stop);
        }
        if let Some(index) = self.state.agents.iter().position(|a| a.id == id) {
            let agent = self.state.agents.remove(index);
            if let Some(store) = &self.store {
                store.delete_agent(id);
            }
            let path = agent.worktree.display().to_string();
            self.notice(LogLevel::Info, format!("removed {} (worktree kept at {path})", agent.name));
        }
    }

    fn select(&mut self, agent: Option<Uuid>) {
        self.selected = agent;
        // Picking an agent of another project (palette, pet) brings its project up.
        let project = agent.and_then(|id| self.state.project_of_agent(id)).map(|p| p.id);
        if let Some(project) = project.filter(|p| Some(*p) != self.state.focus) {
            self.focus_project(project);
        }
    }

    fn set_group_limit(&mut self, group: &str, limit: Option<usize>) {
        let Some(id) = self.find_group(group).map(|g| g.id) else {
            self.notice(LogLevel::Warn, format!("no group named {group}"));
            return;
        };
        if let Some(target) = self.state.groups.iter_mut().find(|g| g.id == id) {
            target.max_active = limit.filter(|n| *n > 0);
            let snapshot = target.clone();
            if let Some(store) = &self.store {
                store.save_group(&snapshot);
            }
            let text = match snapshot.max_active {
                Some(n) => format!("{}: at most {n} agents at once", snapshot.name),
                None => format!("{}: no limit", snapshot.name),
            };
            self.notice(LogLevel::Info, text);
        }
        self.pump();
    }

    fn add_rule(&mut self, agent: Uuid, rule: PermissionRule) {
        let Some(target) = self.agent_mut(agent) else { return };
        let text = format!("{} now has rule {rule}", target.name);
        target.rules.push(rule);
        let snapshot = target.clone();
        self.broker.set_rules(RuleScope::Agent(agent), snapshot.rules.clone());
        if let Some(store) = &self.store {
            store.save_agent(&snapshot);
        }
        self.notice(LogLevel::Info, text);
    }

    /// Commits the agent's worktree and merges its branch into the project.
    fn merge(&mut self, id: Uuid) {
        let (Some(agent), Some(project)) = (self.state.agent(id).cloned(), self.state.project_of_agent(id).cloned())
        else {
            return;
        };
        if self.running.contains_key(&id) && agent.state != AgentState::Done {
            self.notice(LogLevel::Warn, format!("{} is still working; merging what it has so far", agent.name));
        }
        self.log(Some(id), LogLevel::Info, "merging into the project…");
        let inbox = self.inbox.clone();
        let message = format!("{}: {}", agent.name, agent.task);
        tokio::spawn(async move {
            let result = worktree::merge(&project.root, &agent.worktree, &message).await;
            let _ = inbox.send(Internal::Merged { agent: id, result });
        });
    }

    /// The pet splits the group's goal into agents and starts them.
    fn lead(&mut self, group: &str, agents: usize) {
        let Some(target) = self.find_group(group).cloned() else {
            self.notice(LogLevel::Warn, format!("no group named {group}"));
            return;
        };
        if self.state.host.provider.is_none() {
            self.notice(LogLevel::Warn, "the lead needs the pet's model: :pet <model>");
            return;
        }
        let provider = self.state.providers.iter().find(|p| p.ready).map(|p| p.label.clone()).unwrap_or_default();
        let prompt = format!(
            "Act as the lead of group \"{}\". Its goal is: \"{}\". Split the goal into at most {agents} \
             independent tasks that can run in parallel without editing the same files. For each task call \
             create_agent in group \"{}\" with a short name, the task, a persona that keeps it to its part, and \
             provider \"{provider}\" unless the operator asked otherwise; then call start_agent for each. Finally \
             say in one sentence what you created.",
            target.name, target.goal, target.name
        );
        self.log(None, LogLevel::Operator, &format!("→ pet: plan group {}", target.name));
        self.host.ask(prompt, self.state.clone(), self.selected);
    }
}
