//! Open projects (ADR 0017), their groups, and creating agents in them.

use std::path::PathBuf;

use uuid::Uuid;

use super::{AgentCmd, Orchestrator};
use crate::{
    broker::RuleScope,
    model::{Agent, AgentState, Group, LogLevel, Project, Provider, normalize},
    worktree,
};

impl Orchestrator {
    pub(super) async fn open_project(&mut self, root: PathBuf) {
        let root = match tokio::fs::canonicalize(&root).await {
            Ok(root) if root.is_dir() => root,
            _ => {
                self.notice(LogLevel::Error, format!("{} is not a folder", root.display()));
                return;
            }
        };
        if let Some(id) = self.state.projects.iter().find(|p| p.root == root).map(|p| p.id) {
            self.focus_project(id);
            return;
        }
        let loaded = match &self.store {
            Some(store) => store.load(&root).await,
            None => None,
        };
        let (project, groups, agents, layout) = if let Some(loaded) = loaded {
            (loaded.project, loaded.groups, loaded.agents, loaded.layout)
        } else {
            let name = root.file_name().map_or_else(|| "project".into(), |n| n.to_string_lossy().into_owned());
            let project = Project { id: Uuid::new_v4(), name, root: root.clone() };
            if let Some(store) = &self.store {
                store.save_project(&project);
            }
            (project, Vec::new(), Vec::new(), None)
        };
        for group in &groups {
            self.broker.set_rules(RuleScope::Group(group.id), group.rules.clone());
        }
        for agent in &agents {
            self.broker.set_rules(RuleScope::Agent(agent.id), agent.rules.clone());
        }
        let (id, name) = (project.id, project.name.clone());
        if let Some(store) = &self.store {
            store.opened(id);
        }
        self.state.recent_projects.retain(|recent| *recent != root);
        self.state.recent_projects.insert(0, root.clone());
        self.state.recent_projects.truncate(super::RECENT_PROJECTS);
        let counts = (groups.len(), agents.len());
        self.layouts.insert(id, layout);
        self.state.projects.push(project);
        self.state.groups.extend(groups);
        self.state.agents.extend(agents);
        self.focus_project(id);
        self.notice(LogLevel::Info, format!("opened {name}: {} groups, {} agents", counts.0, counts.1));
        if !worktree::is_repository(&root).await {
            self.notice(LogLevel::Warn, "not a git repository: agents need one to get their worktrees");
        }
    }

    pub(super) fn focus_project(&mut self, id: Uuid) {
        if !self.state.projects.iter().any(|p| p.id == id) {
            return;
        }
        self.state.focus = Some(id);
        self.state.layout = self.layouts.get(&id).cloned().flatten();
        if self.selected.and_then(|agent| self.state.project_of_agent(agent)).is_none_or(|p| p.id != id) {
            self.selected = None;
        }
        self.save_open_projects();
        self.dirty = true;
    }

    pub(super) fn close_project(&mut self, id: Uuid) {
        let Some(index) = self.state.projects.iter().position(|p| p.id == id) else {
            return;
        };
        let agents: Vec<Uuid> = self.state.agents_of(Some(id)).map(|a| a.id).collect();
        for agent in &agents {
            if let Some(runner) = self.running.remove(agent) {
                let _ = runner.send(AgentCmd::Stop);
            }
        }
        self.waiting.retain(|(agent, _)| !agents.contains(agent));
        self.state.agents.retain(|a| !agents.contains(&a.id));
        self.state.groups.retain(|g| g.project_id != id);
        let project = self.state.projects.remove(index);
        self.layouts.remove(&id);
        if self.selected.is_some_and(|agent| agents.contains(&agent)) {
            self.selected = None;
        }
        if self.state.focus == Some(id) {
            self.state.focus = None;
            self.state.layout = None;
            if let Some(next) = self.state.projects.last().map(|p| p.id) {
                self.focus_project(next);
            }
        }
        self.save_open_projects();
        self.notice(LogLevel::Info, format!("closed {} ({} agents stopped)", project.name, agents.len()));
    }

    pub(super) fn save_open_projects(&self) {
        let Some(store) = &self.store else { return };
        let roots: Vec<String> = self.state.projects.iter().map(|p| p.root.to_string_lossy().into_owned()).collect();
        store.set_setting("open_projects", &roots.join("\n"));
        if let Some(project) = self.state.project() {
            store.set_setting("focus_project", &project.root.to_string_lossy());
        }
    }

    /// Agents need a project; with none open, the personal workspace opens.
    pub(super) async fn ensure_project(&mut self) {
        if !self.state.projects.is_empty() {
            return;
        }
        let root = self.workspace.clone();
        match worktree::init_repository(&root).await {
            Ok(()) => self.open_project(root).await,
            Err(error) => self.notice(LogLevel::Error, format!("could not prepare the personal workspace: {error}")),
        }
    }

    /// In the personal workspace an agent's group is made on the spot: there is no
    /// project to plan groups for.
    pub(super) fn ensure_workspace_group(&mut self, group: &str) {
        // Opened projects keep their canonical root.
        let workspace = std::fs::canonicalize(&self.workspace).unwrap_or_else(|_| self.workspace.clone());
        let in_workspace = self.state.project().is_some_and(|p| p.root == workspace);
        if in_workspace && self.find_group(group).is_none() {
            self.create_group(group, "agents that work outside any project");
        }
    }

    pub(super) fn create_group(&mut self, name: &str, goal: &str) {
        let Some(project) = self.state.project() else {
            self.notice(LogLevel::Warn, "open a project first: :open <path>");
            return;
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            self.notice(LogLevel::Warn, "usage: :group <name>[: <goal>]");
            return;
        }
        if self.state.focused_groups().any(|g| normalize(&g.name) == normalize(&name)) {
            self.notice(LogLevel::Warn, format!("group {name} already exists"));
            return;
        }
        let group = Group {
            id: Uuid::new_v4(),
            project_id: project.id,
            name,
            goal: goal.trim().to_owned(),
            rules: Vec::new(),
            max_active: None,
        };
        if let Some(store) = &self.store {
            store.save_group(&group);
        }
        let text = format!("group {} created", group.name);
        self.state.groups.push(group);
        self.notice(LogLevel::Info, text);
    }

    /// By name, in the focused project first, then in the other open projects.
    pub(super) fn find_group(&self, name: &str) -> Option<&Group> {
        let wanted = normalize(name);
        let exact = |g: &&Group| normalize(&g.name) == wanted;
        let prefix = |g: &&Group| !wanted.is_empty() && normalize(&g.name).starts_with(&wanted);
        self.state
            .focused_groups()
            .find(exact)
            .or_else(|| self.state.focused_groups().find(prefix))
            .or_else(|| self.state.groups.iter().find(exact))
            .or_else(|| self.state.groups.iter().find(prefix))
    }

    pub(super) fn find_provider(&self, wanted: Option<&str>) -> Option<Provider> {
        let Some(wanted) = wanted.map(str::trim).filter(|w| !w.is_empty()) else {
            return self.state.providers.iter().find(|p| p.ready).map(|p| p.provider.clone());
        };
        if let Some(option) = self.state.providers.iter().find(|p| p.label.eq_ignore_ascii_case(wanted)) {
            return Some(option.provider.clone());
        }
        // `anthropic/claude-opus-5-5`: an API service with another model.
        let (service, model) = wanted.split_once('/')?;
        let option = self.state.providers.iter().find(|p| p.label.eq_ignore_ascii_case(service))?;
        match &option.provider {
            Provider::Api { service, key_ref, .. } => {
                Some(Provider::Api { service: service.clone(), model: model.into(), key_ref: key_ref.clone() })
            }
            _ => None,
        }
    }

    pub(super) fn create_agent(&mut self, group: &str, name: &str, provider: Option<&str>, task: &str, persona: &str) {
        if self.state.projects.is_empty() {
            self.notice(LogLevel::Warn, "open a project first: :open <path>");
            return;
        }
        let Some(group) = self.find_group(group).cloned() else {
            self.notice(LogLevel::Warn, format!("no group named {group}; create it with :group"));
            return;
        };
        let name = name.trim().to_owned();
        if name.is_empty() || task.trim().is_empty() {
            self.notice(LogLevel::Warn, "usage: :new <group> <name> [provider]: <task>");
            return;
        }
        if self.state.agents.iter().any(|a| a.group_id == group.id && normalize(&a.name) == normalize(&name)) {
            self.notice(LogLevel::Warn, format!("{name} already exists in {}", group.name));
            return;
        }
        let Some(provider) = self.find_provider(provider) else {
            let labels: Vec<_> = self.state.providers.iter().map(|p| p.label.as_str()).collect();
            self.notice(LogLevel::Warn, format!("unknown provider; available: {}", labels.join(", ")));
            return;
        };
        let Some(project) = self.state.project_of_group(group.id).cloned() else {
            return;
        };
        let id = Uuid::new_v4();
        let agent = Agent {
            id,
            group_id: group.id,
            worktree: worktree::path_for(&project.root, &name, id),
            name,
            task: task.trim().to_owned(),
            persona: persona.trim().to_owned(),
            model: None,
            models: Vec::new(),
            color: None,
            context: None,
            cost_micros: 0,
            provider,
            state: AgentState::Stopped,
            rules: Vec::new(),
            started_at: None,
            usage: Default::default(),
        };
        if let Some(store) = &self.store {
            store.save_agent(&agent);
        }
        self.broker.set_rules(RuleScope::Agent(id), Vec::new());
        let text = format!(
            "{} created in {} with {}; :start or just send it a message",
            agent.name,
            group.name,
            agent.provider.short_name()
        );
        self.state.agents.push(agent);
        self.selected = Some(id);
        self.notice(LogLevel::Info, text);
    }
}
