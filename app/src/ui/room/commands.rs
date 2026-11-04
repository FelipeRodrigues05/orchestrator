//! The `:commands` typed on the command line or in the palette.

use std::{path::PathBuf, time::Instant};

use orch_core::{
    model::{Agent, Effect, PermissionRule},
    orchestrator::Command,
};
use uuid::Uuid;

use super::format::{AGENT_COLORS, parse_color};
use super::{DIM, Prompt, Room, View, WARN};
use crate::ui::{palette::Palette, theme::Theme};

impl Room {
    pub(super) fn run(&mut self, line: &str) {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        let word = word.to_lowercase();
        match word.as_str() {
            "" => {}
            "help" | "h" => self.help = true,
            "ping" => {
                self.link.ping_requested = true;
                self.link.ping_sent = Some(Instant::now());
                self.outbox.push(Command::Ping);
            }
            "ack" => self.ack(),
            "open" if rest.is_empty() => self.palette = Some(Palette::open_project()),
            "open" => {
                let root = self.project_root(rest);
                self.outbox.push(Command::OpenProject { root });
            }
            "project" | "close" => self.project_command(&word, rest),
            // `:group API: payments backend`, or just `:group API` (the goal is optional).
            "group" => {
                let (name, goal) = rest.split_once(':').unwrap_or((rest, ""));
                if name.trim().is_empty() {
                    self.say("usage: :group <name>[: <goal>]", WARN);
                } else {
                    self.outbox.push(Command::CreateGroup {
                        name: name.trim().into(),
                        goal: goal.trim().into(),
                    });
                }
            }
            "persona" => self.persona_command(rest),
            "color" | "colour" | "cor" => self.color_command(rest),
            "intro" => match rest {
                "on" => self.outbox.push(Command::SetIntro(true)),
                "off" => self.outbox.push(Command::SetIntro(false)),
                _ => self.say("usage: :intro on|off", WARN),
            },
            "theme" | "tema" => {
                if let Some(theme) = Theme::parse(rest) {
                    self.outbox.push(Command::SetTheme(theme.name().to_owned()));
                } else {
                    let names: Vec<&str> = Theme::ALL.iter().map(|t| t.name()).collect();
                    self.say(&format!("themes: {}", names.join(", ")), WARN);
                }
            }
            "new" => self.new_command(rest),
            "start" | "stop" | "remove" | "merge" => self.agent_command(&word, rest),
            "edit" | "code" => {
                let (agent, editor) = match rest.strip_prefix("project") {
                    Some(editor) => (None, editor.trim()),
                    None => (self.selected, rest),
                };
                let editor = (!editor.is_empty()).then(|| editor.to_owned());
                self.outbox.push(Command::OpenIn { agent, editor });
            }
            "provider" if !rest.is_empty() => match self.selected {
                Some(agent) => self.outbox.push(Command::SetProvider {
                    agent,
                    provider: rest.to_owned(),
                }),
                None => self.say("select an agent first", WARN),
            },
            "model" => match (self.selected, rest) {
                (None, _) => self.say("select an agent first", WARN),
                (Some(agent), "") => {
                    self.palette = Some(Palette::model(agent));
                    self.outbox.push(Command::ProbeModels(agent));
                }
                (Some(agent), model) => self.outbox.push(Command::SetModel {
                    agent,
                    model: model.to_owned(),
                }),
            },
            "update" => self.outbox.push(Command::UpdateClis),
            // `:fallback claude/opus, codex/gpt-6-astra` · `:fallback auto` · `:fallback reset`
            "fallback" if rest.is_empty() => self.say(
                "usage: :fallback <provider[/model]>, … | auto | reset",
                WARN,
            ),
            "fallback" => self.outbox.push(Command::SetFallback(rest.to_owned())),
            // `:compact 80` · `:compact off`: when agents go on with a summary.
            "compact" => match rest {
                "off" | "none" => self.outbox.push(Command::SetCompactAt(None)),
                percent => match percent.trim_end_matches('%').parse::<u64>() {
                    Ok(p) if (30..=99).contains(&p) => {
                        self.outbox.push(Command::SetCompactAt(Some(p)));
                    }
                    _ => self.say("usage: :compact <30-99>% | off", WARN),
                },
            },
            "limit" => self.limit_command(rest),
            "timeout" => match rest.parse::<u64>() {
                Ok(minutes) if minutes > 0 => {
                    self.outbox.push(Command::SetPermissionTimeout(minutes));
                }
                _ => self.say("usage: :timeout <minutes>", WARN),
            },
            "pet" if !rest.is_empty() => self.pet_command(rest),
            "lead" => self.lead_command(rest),
            "wake" => match rest {
                "on" => self.set_wake(true),
                "off" => self.set_wake(false),
                _ => self.say("usage: :wake on|off", WARN),
            },
            "sandbox" => match rest {
                "on" => self.outbox.push(Command::SetSandbox(true)),
                "off" => self.outbox.push(Command::SetSandbox(false)),
                _ => self.say("usage: :sandbox on|off", WARN),
            },
            "voice" => self.voice_command(rest),
            "lang" | "language" => match rest {
                "" => self.say("usage: :lang pt|en", WARN),
                code => self.outbox.push(Command::SetLanguage(code.to_owned())),
            },
            "email" | "mail" => match rest {
                "" => self.outbox.push(Command::OpenEmail),
                url => self.outbox.push(Command::SetEmailUrl(url.to_owned())),
            },
            "term" | "terminal" => self.outbox.push(Command::OpenTerminal(self.selected)),
            "key" if rest.ends_with(" delete") || rest.ends_with(" remove") => {
                let service = rest
                    .rsplit_once(' ')
                    .map_or(rest, |(service, _)| service)
                    .trim();
                self.outbox
                    .push(Command::DeleteApiKey(service.to_lowercase()));
            }
            "key" if !rest.is_empty() => {
                self.prompt = Some(Prompt::Secret {
                    service: rest.to_lowercase(),
                    key: String::new(),
                });
            }
            "allow" | "deny" if !rest.is_empty() => self.rule_command(&word, rest),
            "view" => match View::from_name(rest) {
                Some(view) => self.set_view(view),
                None => self.say("views: overview, agents, perms, log, setup", WARN),
            },
            "agent" | "select" => match self.find_agent(rest) {
                Some(id) => self.select(Some(id)),
                None => self.say(&format!("no agent named {rest:?}"), WARN),
            },
            other => match View::from_name(other) {
                Some(view) => self.set_view(view),
                None => self.say(&format!("unknown command :{other} · :help"), WARN),
            },
        }
    }

    /// `:open shop` finds a recent project or a repository found on disk by its
    /// folder name; anything that looks like a path is taken as one.
    fn project_root(&self, wanted: &str) -> PathBuf {
        let looks_like_path = wanted.starts_with(['/', '~', '.']) || wanted.contains('/');
        let known = self
            .state
            .as_ref()
            .filter(|_| !looks_like_path)
            .and_then(|state| {
                let name = wanted.to_lowercase();
                let named = |root: &&PathBuf, exact: bool| {
                    root.file_name().is_some_and(|n| {
                        let n = n.to_string_lossy().to_lowercase();
                        if exact {
                            n == name
                        } else {
                            n.starts_with(&name)
                        }
                    })
                };
                let all = || state.recent_projects.iter().chain(&state.repositories);
                all()
                    .find(|root| named(root, true))
                    .or_else(|| all().find(|root| named(root, false)))
                    .cloned()
            });
        known.unwrap_or_else(|| expand_home(wanted))
    }

    /// The agent a command is about: the one named, or the selected one.
    fn target(&self, name: &str) -> Option<Uuid> {
        if name.is_empty() {
            self.selected
        } else {
            self.find_agent(name)
        }
    }

    /// `:project web` focuses an open project by name; `:close` closes the focused one.
    fn project_command(&mut self, word: &str, rest: &str) {
        let Some(state) = &self.state else { return };
        let wanted = rest.to_lowercase();
        let found = if wanted.is_empty() {
            state.project()
        } else {
            state
                .projects
                .iter()
                .find(|p| p.name.to_lowercase() == wanted)
                .or_else(|| {
                    state
                        .projects
                        .iter()
                        .find(|p| p.name.to_lowercase().starts_with(&wanted))
                })
        };
        match (found.map(|p| p.id), word) {
            (Some(id), "close") => self.outbox.push(Command::CloseProject(id)),
            (Some(id), _) => self.outbox.push(Command::FocusProject(id)),
            (None, _) => self.say(&format!("no open project named {rest}"), WARN),
        }
    }

    /// `:persona Back: backend only` (the selected agent without a name).
    fn persona_command(&mut self, rest: &str) {
        let (name, persona) = rest.split_once(':').unwrap_or(("", rest));
        match self.target(name.trim()) {
            Some(agent) => self.outbox.push(Command::SetPersona {
                agent,
                persona: persona.trim().into(),
            }),
            None => self.say("usage: :persona <agent>: <who it is>", WARN),
        }
    }

    /// `:color green` for the selected agent, `:color Kio verde`, `:color auto|none`.
    fn color_command(&mut self, rest: &str) {
        let (name, color) = rest.rsplit_once(' ').unwrap_or(("", rest));
        let colors: Vec<&str> = AGENT_COLORS.iter().map(|(en, _)| *en).collect();
        let usage = format!("usage: :color [agent] <{}|auto|none>", colors.join("|"));
        let Some(color) = parse_color(color) else {
            self.say(&usage, WARN);
            return;
        };
        match self.target(name.trim()) {
            Some(agent) => self.outbox.push(Command::SetColor { agent, color }),
            None => self.say(&usage, WARN),
        }
    }

    /// `:new API Back opencode: the task | backend only` (the persona is optional).
    fn new_command(&mut self, rest: &str) {
        let Some((group, name, provider, task)) = parse_new(rest) else {
            self.say("usage: :new <group> <name> [provider]: <task>", WARN);
            return;
        };
        let (task, persona) = match task.split_once(" | ") {
            Some((task, persona)) => (task.trim().to_owned(), persona.trim().to_owned()),
            None => (task, String::new()),
        };
        self.outbox.push(Command::CreateAgent {
            group,
            name,
            provider,
            task,
            persona,
        });
    }

    /// `:start`, `:stop`, `:remove`, `:merge` on the named or selected agent.
    fn agent_command(&mut self, word: &str, rest: &str) {
        let Some(id) = self.target(rest) else {
            self.say(&format!("which agent? select one or :{word} <name>"), WARN);
            return;
        };
        self.outbox.push(match word {
            "start" => Command::StartAgent(id),
            "stop" => Command::StopAgent(id),
            "merge" => Command::Merge(id),
            _ => Command::RemoveAgent(id),
        });
    }

    /// `:limit API 2`, `:limit API off`.
    fn limit_command(&mut self, rest: &str) {
        let (group, value) = rest.rsplit_once(' ').unwrap_or((rest, ""));
        let limit = match value {
            "off" | "none" | "0" => Some(None),
            number => number.parse::<usize>().ok().map(Some),
        };
        match (group.trim(), limit) {
            ("", _) | (_, None) => self.say("usage: :limit <group> <n|off>", WARN),
            (group, Some(limit)) => self.outbox.push(Command::SetGroupLimit {
                group: group.into(),
                limit,
            }),
        }
    }

    /// `:pet float`, `:pet dock`, or `:pet <model>` to choose what the pet thinks with.
    fn pet_command(&mut self, rest: &str) {
        match rest {
            "float" if !self.can_float => self.say(
                "this desktop has no layer-shell (GNOME): the pet stays in the room",
                WARN,
            ),
            "float" | "dock" => {
                self.float_pet = rest == "float";
                self.save_layout();
            }
            model => self.outbox.push(Command::SetHostProvider(model.to_owned())),
        }
    }

    /// `:lead API 3`: the pet splits the group's goal into that many agents.
    fn lead_command(&mut self, rest: &str) {
        let (group, count) = match rest.rsplit_once(' ') {
            Some((group, n)) => match n.parse::<usize>() {
                Ok(n) => (group.trim(), n),
                Err(_) => (rest, 3),
            },
            None => (rest, 3),
        };
        if group.is_empty() {
            self.say("usage: :lead <group> [agents]", WARN);
            return;
        }
        self.outbox.push(Command::Lead {
            group: group.into(),
            agents: count,
        });
    }

    /// `:voice` lists voices, `:voice <id>` switches (downloading it), `:voice install`.
    fn voice_command(&mut self, rest: &str) {
        match rest {
            "install yes" => self.outbox.push(Command::InstallVoice),
            "install" => self.say(
                "downloads ~630 MB (whisper small + tiny, pt_BR voice): :voice install yes",
                WARN,
            ),
            "" => {
                let installed = self
                    .state
                    .as_ref()
                    .map(|s| s.voice.voices.join(", "))
                    .unwrap_or_default();
                self.say(
                    &format!(
                        "voices: {installed} · :voice <id> (e.g. en_US-lessac-medium) downloads others"
                    ),
                    DIM,
                );
            }
            id => self.outbox.push(Command::SetVoice(id.to_owned())),
        }
    }

    /// `:allow Bash(cargo test:*)` / `:deny …` for the selected agent.
    fn rule_command(&mut self, word: &str, rest: &str) {
        let effect = if word == "allow" {
            Effect::Allow
        } else {
            Effect::Deny
        };
        match (self.selected, PermissionRule::parse(rest, effect)) {
            (Some(agent), Some(rule)) => self.outbox.push(Command::AddRule { agent, rule }),
            _ => self.say(
                "usage: select an agent, then :allow Bash(cargo test:*)",
                WARN,
            ),
        }
    }

    /// By name, in the focused project first.
    pub(super) fn find_agent(&self, name: &str) -> Option<Uuid> {
        let name = name.to_lowercase();
        let state = self.state.as_ref()?;
        let exact = |a: &&Agent| a.name.to_lowercase() == name;
        let prefix = |a: &&Agent| !name.is_empty() && a.name.to_lowercase().starts_with(&name);
        state
            .focused_agents()
            .find(exact)
            .or_else(|| state.focused_agents().find(prefix))
            .or_else(|| state.agents.iter().find(exact))
            .or_else(|| state.agents.iter().find(prefix))
            .map(|a| a.id)
    }
}

/// `API Tests opencode: write tests` → group, name, provider, task.
pub(super) fn parse_new(rest: &str) -> Option<(String, String, Option<String>, String)> {
    let (head, task) = rest.split_once(':')?;
    let mut words = head.split_whitespace();
    let group = words.next()?.to_owned();
    let name = words.next()?.to_owned();
    let provider = words.next().map(str::to_owned);
    let task = task.trim();
    (!task.is_empty()).then(|| (group, name, provider, task.to_owned()))
}

pub(in crate::ui) fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some(rest) => dirs_home().join(rest.trim_start_matches('/')),
        None => PathBuf::from(path),
    }
}

pub(super) fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}
