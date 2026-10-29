//! What each mode lists: the commands, the wizard's choices, providers and voices.

use orch_core::model::{Agent, AgentState, AppState, Language, Provider};
use uuid::Uuid;

use super::{Item, Mode, Palette, Pick, Step};
use crate::ui::room::{ACCENT, TEXT, View, WARN};

impl Palette {
    pub(super) fn items(&self, state: &AppState, selected: Option<Uuid>) -> Vec<Item> {
        let agent = selected.and_then(|id| state.agent(id));
        match &self.mode {
            Mode::Commands => self.command_items(state, agent),
            Mode::NewAgent(wizard) => match wizard.step {
                Step::Group => self.group_items(state),
                Step::Provider => self.provider_items(state),
                _ => Vec::new(),
            },
            Mode::SwitchProvider(_) => self.provider_items(state),
            Mode::Voice => self.voice_items(state),
            Mode::PetModel => self.pet_model_items(state),
            Mode::ApiKey => state
                .providers
                .iter()
                .filter(|p| matches!(p.provider, Provider::Api { .. }))
                .map(|p| {
                    let status = if p.ready { "key stored" } else { "no key yet" };
                    item(p.label.clone(), status, "", Pick::Service(p.label.clone()))
                })
                .collect(),
            Mode::OpenIn => state
                .editors
                .iter()
                .map(|e| {
                    item(
                        e.name.clone(),
                        e.command.clone(),
                        "",
                        Pick::Editor(e.name.clone()),
                    )
                })
                .collect(),
            Mode::NewGroup { .. } | Mode::OpenProject | Mode::Persona(_) => Vec::new(),
        }
    }

    /// Every action, with the selected agent's first, and `:typed` to run as a command.
    fn command_items(&self, state: &AppState, agent: Option<&Agent>) -> Vec<Item> {
        let mut items = vec![
            item(
                "New agent",
                "group › name › provider › task",
                "n",
                Pick::NewAgent,
            ),
            item("New group", "a shared goal for agents", "", Pick::NewGroup),
            item("Open project", "a git repository", "", Pick::OpenProject),
        ];
        if let Some(agent) = agent {
            items.extend(agent_items(agent));
        }
        items.push(item("Open project in editor", "", "", Pick::OpenProjectIn));
        items.push(item(
            "Open terminal",
            "in the selected agent's worktree or the project",
            "",
            Pick::OpenTerminal,
        ));
        items.push(item(
            "Open email",
            "webmail from :email <url>, else the mail app",
            "",
            Pick::OpenEmail,
        ));
        let other = if state.voice.language == Language::Pt {
            "en"
        } else {
            "pt"
        };
        items.push(item(
            format!("Language: switch to {other}"),
            format!(
                "now {} · speech, pet answers, announcements",
                state.voice.language.code()
            ),
            "",
            Pick::Language(other.into()),
        ));
        items.push(item(
            "Pet voice",
            format!(
                "now {}",
                state.voice.voice.clone().unwrap_or_else(|| "none".into())
            ),
            "",
            Pick::ChooseVoice,
        ));
        items.extend(project_items(state));
        items.push(item("Set API key", "Secret Service", "", Pick::ApiKey));
        items.push(item(
            "Update agent CLIs",
            "opencode, claude, codex, gemini…",
            "",
            Pick::UpdateClis,
        ));
        let pet = state.host.provider.clone().unwrap_or_else(|| "none".into());
        items.push(item("Pet model", format!("now {pet}"), "", Pick::PetModel));
        if !state.voice.stt || !state.voice.tts {
            items.push(item(
                "Install voice models",
                "~550 MB: whisper small + pt_BR voice",
                "",
                Pick::InstallVoice,
            ));
        }
        for (index, view) in View::ALL.into_iter().enumerate() {
            let keys = ["F1", "F2", "F3", "F4", "F5"][index];
            items.push(item(
                format!("View: {}", view.label().to_lowercase()),
                "",
                keys,
                Pick::View(view),
            ));
        }
        for agent in &state.agents {
            let group = state.group(agent.group_id).map_or("?", |g| g.name.as_str());
            let detail = match state.project_of_agent(agent.id) {
                Some(project) if state.projects.len() > 1 => {
                    format!("{} · {group}", project.name)
                }
                _ => group.to_owned(),
            };
            items.push(item(
                format!("Go to {}", agent.name),
                detail,
                "",
                Pick::GoTo(agent.id),
            ));
        }
        items.push(item("Help", "keys and commands", "?", Pick::Help));
        let typed = self.query.trim().trim_start_matches(':').trim();
        if !typed.is_empty() {
            let mut run = item(
                format!(":{typed}"),
                "run as a command · :help lists them",
                "",
                Pick::Run(typed.into()),
            );
            run.style = ACCENT;
            items.push(run);
        }
        items
    }

    /// The new-agent wizard's groups, plus "create" for a name not taken.
    fn group_items(&self, state: &AppState) -> Vec<Item> {
        if state.project().is_none() {
            return vec![item("Open a project first", "", "", Pick::OpenProject)];
        }
        let mut items: Vec<Item> = state
            .focused_groups()
            .map(|g| {
                item(
                    g.name.clone(),
                    g.goal.clone(),
                    "",
                    Pick::Group(g.name.clone()),
                )
            })
            .collect();
        let typed = self.query.trim();
        if !typed.is_empty()
            && !state
                .focused_groups()
                .any(|g| g.name.eq_ignore_ascii_case(typed))
        {
            let mut create = item(
                format!("+ create group “{typed}”"),
                "then its goal",
                "",
                Pick::CreateGroup(typed.into()),
            );
            create.style = ACCENT;
            items.push(create);
        }
        items
    }

    /// Installed voices, a few to download, and the typed id.
    fn voice_items(&self, state: &AppState) -> Vec<Item> {
        let mut items: Vec<Item> = state
            .voice
            .voices
            .iter()
            .map(|v| item(v.clone(), "installed", "", Pick::Voice(v.clone())))
            .collect();
        for suggested in [
            "pt_BR-faber-medium",
            "pt_BR-cadu-medium",
            "en_US-lessac-medium",
            "en_US-amy-medium",
            "en_GB-alan-medium",
        ] {
            if !state.voice.voices.iter().any(|v| v == suggested) {
                items.push(item(
                    suggested,
                    "downloads ~60 MB",
                    "",
                    Pick::Voice(suggested.into()),
                ));
            }
        }
        let typed = self.query.trim();
        if typed.contains('-') && !items.iter().any(|i| i.label == typed) {
            let mut custom = item(
                format!("use “{typed}”"),
                "a Piper voice id",
                "",
                Pick::Voice(typed.into()),
            );
            custom.style = ACCENT;
            items.push(custom);
        }
        items
    }

    /// Providers the pet can think with: API and local models.
    fn pet_model_items(&self, state: &AppState) -> Vec<Item> {
        self.provider_items(state)
            .into_iter()
            .filter(|item| match &item.pick {
                Pick::Provider(label) => {
                    label.contains('/')
                        || state.providers.iter().any(|p| {
                            &p.label == label
                                && matches!(
                                    p.provider,
                                    Provider::Api { .. } | Provider::Local { .. }
                                )
                        })
                }
                _ => false,
            })
            .collect()
    }

    pub(super) fn provider_items(&self, state: &AppState) -> Vec<Item> {
        let mut items: Vec<Item> = state
            .providers
            .iter()
            .map(|p| {
                let kind = match &p.provider {
                    Provider::CliAcp { .. } => "ACP cli",
                    Provider::CliMcp { .. } => "MCP cli",
                    Provider::Api { .. } => "API",
                    Provider::Local { .. } => "local model",
                };
                let mut entry = item(
                    p.label.clone(),
                    if p.ready {
                        kind.to_owned()
                    } else {
                        format!("{kind} · needs :key {}", p.label)
                    },
                    "",
                    Pick::Provider(p.label.clone()),
                );
                entry.style = if p.ready { TEXT } else { WARN };
                entry
            })
            .collect();
        // `anthropic/claude-opus-5-5`: another model of an API service.
        let typed = self.query.trim();
        if typed.contains('/') && !state.providers.iter().any(|p| p.label == typed) {
            let mut custom = item(
                format!("use “{typed}”"),
                "service/model",
                "",
                Pick::Provider(typed.into()),
            );
            custom.style = ACCENT;
            items.push(custom);
        }
        items
    }
}

pub(super) fn item(
    label: impl Into<String>,
    detail: impl Into<String>,
    hint: &'static str,
    pick: Pick,
) -> Item {
    Item {
        label: label.into(),
        detail: detail.into(),
        hint,
        pick,
        style: TEXT,
    }
}

/// What can be done with the selected agent.
fn agent_items(agent: &Agent) -> Vec<Item> {
    let running = !matches!(agent.state, AgentState::Stopped | AgentState::Error);
    let verb = if running { "Stop" } else { "Start" };
    vec![
        item(
            format!("{verb} {}", agent.name),
            "",
            "s",
            Pick::Toggle(agent.id, running),
        ),
        item(
            format!("Message {}", agent.name),
            "",
            "Enter",
            Pick::Message,
        ),
        item(
            format!("Open {} in editor", agent.name),
            "its worktree",
            "e",
            Pick::OpenIn,
        ),
        item(
            format!("Switch provider of {}", agent.name),
            format!(
                "now {} · keeps the conversation",
                agent.provider.short_name()
            ),
            "",
            Pick::SwitchProvider(agent.id),
        ),
        item(
            format!("Persona of {}", agent.name),
            if agent.persona.is_empty() {
                "none: who it is, what it leaves to teammates".to_owned()
            } else {
                agent.persona.clone()
            },
            "",
            Pick::Persona(agent.id),
        ),
        item(
            format!("Remove {}", agent.name),
            "the worktree stays on disk",
            "",
            Pick::Remove(agent.id),
        ),
        item(
            format!("Merge {} into the project", agent.name),
            "commits its worktree, merges its branch",
            "",
            Pick::Merge(agent.id),
        ),
    ]
}

/// Switching to or closing open projects, and planning the focused project's groups.
fn project_items(state: &AppState) -> Vec<Item> {
    let mut items = Vec::new();
    for project in &state.projects {
        if state.focus == Some(project.id) {
            items.push(item(
                format!("Close project {}", project.name),
                "stops its agents; reopen with :open",
                "",
                Pick::CloseProject(project.id),
            ));
        } else {
            items.push(item(
                format!("Switch to project {}", project.name),
                project.root.display().to_string(),
                "Alt+n",
                Pick::FocusProject(project.id),
            ));
        }
    }
    for group in state.focused_groups() {
        items.push(item(
            format!("Plan {} with the pet", group.name),
            "splits the goal into agents and starts them",
            "",
            Pick::Lead(group.name.clone()),
        ));
    }
    items
}
