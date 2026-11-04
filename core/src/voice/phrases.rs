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
                let what = speakable(what, "e mais");
                let bash = tool.eq_ignore_ascii_case("bash");
                let action = if bash { format!("rodar {what}") } else { format!("usar {tool} em {what}") };
                format!("Agente {agent}, do grupo {group}, pede permissão para {action}")
            }
            (Phrase::Permission { agent, group, tool, what }, En) => {
                let what = speakable(what, "and more");
                let bash = tool.eq_ignore_ascii_case("bash");
                let action = if bash { format!("run {what}") } else { format!("use {tool} on {what}") };
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
        assert_eq!(ask.render(Language::Pt), "Agente Tests, do grupo API, pede permissão para rodar cargo test");
        assert_eq!(ask.render(Language::English), "Agent Tests, from group API, asks permission to run cargo test");
        assert_eq!(Language::parse("English"), Some(Language::English));
        assert_eq!(Language::parse("pt-BR"), Some(Language::Pt));

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
        assert_eq!(
            long.render(Language::English),
            "Agent Kio, from group Backend, asks permission to run php artisan test"
        );
        let many = Phrase::Permission {
            agent: "Kio".into(),
            group: "Backend".into(),
            tool: "Bash".into(),
            what: "git add src/a.rs src/b.rs src/c.rs src/d.rs src/e.rs src/f.rs src/g.rs".into(),
        };
        assert_eq!(
            many.render(Language::Pt),
            "Agente Kio, do grupo Backend, pede permissão para rodar git add a.rs b.rs c.rs d.rs e.rs f.rs e mais"
        );
    }
}
