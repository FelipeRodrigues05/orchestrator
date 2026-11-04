//! What the app says out loud, per language. The voice actor renders a `Phrase` in
//! the operator's language, so callers never build spoken text themselves.

pub use crate::model::Language;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phrase {
    Permission {
        agent: String,
        group: String,
        tool: String,
        what: String,
    },
    Done {
        agent: String,
        group: String,
    },
    Error {
        agent: String,
        group: String,
    },
    StartFailed {
        agent: String,
        group: String,
    },
    Updating {
        cli: String,
        agent: String,
    },
    NoAgent {
        name: String,
    },
    /// An agent hit its usage limit and goes on with another model.
    FellBack {
        agent: String,
        to: String,
    },
    /// Already in the right language (the pet's own words, summaries).
    Text(String),
}

impl Phrase {
    pub fn render(&self, language: Language) -> String {
        use Language::{English as En, Pt};
        match (self, language) {
            (Phrase::Permission { agent, group, tool, what }, Pt) => {
                let action = if is_shell(tool) {
                    format!("rodar {}", program(what))
                } else {
                    format!("usar {tool} em {}", speakable(what, "e mais"))
                };
                format!("Agente {agent}, do grupo {group}, pede permissão para {action}")
            }
            (Phrase::Permission { agent, group, tool, what }, En) => {
                let action = if is_shell(tool) {
                    format!("run {}", program(what))
                } else {
                    format!("use {tool} on {}", speakable(what, "and more"))
                };
                format!("Agent {agent}, from group {group}, asks permission to {action}")
            }
            (Phrase::Done { agent, group }, Pt) => {
                format!("Agente {agent}, do grupo {group}, terminou")
            }
            (Phrase::Done { agent, group }, En) => {
                format!("Agent {agent}, from group {group}, is done")
            }
            (Phrase::Error { agent, group }, Pt) => {
                format!("Agente {agent}, do grupo {group}, teve um erro")
            }
            (Phrase::Error { agent, group }, En) => {
                format!("Agent {agent}, from group {group}, hit an error")
            }
            (Phrase::StartFailed { agent, group }, Pt) => {
                format!("Agente {agent}, do grupo {group}, não conseguiu iniciar")
            }
            (Phrase::StartFailed { agent, group }, En) => {
                format!("Agent {agent}, from group {group}, could not start")
            }
            (Phrase::Updating { cli, agent }, Pt) => {
                format!("Atualizando o {cli} do agente {agent}")
            }
            (Phrase::Updating { cli, agent }, En) => format!("Updating {cli} for agent {agent}"),
            (Phrase::FellBack { agent, to }, Pt) => {
                format!("Agente {agent} atingiu o limite; continuando com {to}")
            }
            (Phrase::FellBack { agent, to }, En) => format!("Agent {agent} hit its limit; going on with {to}"),
            (Phrase::NoAgent { name }, Pt) => format!("Não achei o agente {name}"),
            (Phrase::NoAgent { name }, En) => format!("I could not find agent {name}"),
            (Phrase::Text(text), _) => text.clone(),
        }
    }
}

/// Shell tools, as the CLIs name them.
fn is_shell(tool: &str) -> bool {
    ["bash", "shell", "execute", "run_command", "terminal"].iter().any(|t| tool.eq_ignore_ascii_case(t))
}

/// The program a command runs, by name: `FOO=1 sudo /usr/bin/php artisan test` →
/// `php`. Commands are read on screen; out loud only what runs.
fn program(command: &str) -> String {
    command
        .split_whitespace()
        .find(|word| !word.contains('=') && !matches!(*word, "sudo" | "env" | "time" | "nice" | "command"))
        .map_or_else(|| "um comando".to_owned(), |word| word.rsplit('/').next().unwrap_or(word).to_owned())
}

/// Words worth saying out loud: paths become their file name, flags (`--parallel`)
/// are left out and a long command stops after a few words; the screen keeps the
/// whole request.
fn speakable(what: &str, and_more: &str) -> String {
    const WORDS: usize = 8;
    let words: Vec<String> = what
        .split_whitespace()
        .filter(|word| !word.starts_with('-'))
        .map(|word| match word.trim_end_matches('/').rsplit_once('/') {
            Some((_, name)) if !name.is_empty() => name.to_owned(),
            _ => word.to_owned(),
        })
        .collect();
    if words.len() > WORDS { format!("{} {and_more}", words[..WORDS].join(" ")) } else { words.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_follow_the_language() {
        let ask = Phrase::Permission {
            agent: "Tests".into(),
            group: "API".into(),
            tool: "Bash".into(),
            what: "cargo test".into(),
        };
        assert_eq!(ask.render(Language::Pt), "Agente Tests, do grupo API, pede permissão para rodar cargo");
        assert_eq!(ask.render(Language::English), "Agent Tests, from group API, asks permission to run cargo");
        assert_eq!(Language::parse("English"), Some(Language::English));
        assert_eq!(Language::parse("pt-BR"), Some(Language::Pt));
        assert_eq!(program("FOO=1 sudo /usr/bin/php artisan test"), "php");

        // Paths are spoken as their file name; long commands are cut.
        let path =
            "/home/me/www/personal/job-applications/.worktrees/kio-bcc2e5c1/tests/Unit/DTOs/ExtractedJobTest.php";
        let edit = Phrase::Permission {
            agent: "Kio".into(),
            group: "Backend".into(),
            tool: "Write".into(),
            what: path.into(),
        };
        assert_eq!(
            edit.render(Language::Pt),
            "Agente Kio, do grupo Backend, pede permissão para usar Write em ExtractedJobTest.php"
        );
        let long = Phrase::Permission {
            agent: "Kio".into(),
            group: "Backend".into(),
            tool: "Bash".into(),
            what: "php /srv/app/artisan test --filter=ExtractedJobTest --stop-on-failure --parallel".into(),
        };
        assert_eq!(long.render(Language::English), "Agent Kio, from group Backend, asks permission to run php");
        let many = Phrase::Permission {
            agent: "Kio".into(),
            group: "Backend".into(),
            tool: "Bash".into(),
            what: "git add src/a.rs src/b.rs src/c.rs src/d.rs src/e.rs src/f.rs src/g.rs".into(),
        };
        assert_eq!(many.render(Language::Pt), "Agente Kio, do grupo Backend, pede permissão para rodar git");
    }
}
