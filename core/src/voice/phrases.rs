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
    /// Already in the right language (the pet's own words, summaries).
    Text(String),
}

impl Phrase {
    pub fn render(&self, language: Language) -> String {
        use Language::{English as En, Pt};
        match (self, language) {
            (Phrase::Permission { agent, group, tool, what }, Pt) => {
                let bash = tool.eq_ignore_ascii_case("bash");
                let action = if bash { format!("rodar {what}") } else { format!("usar {tool} em {what}") };
                format!("Agente {agent}, do grupo {group}, pede permissão para {action}")
            }
            (Phrase::Permission { agent, group, tool, what }, En) => {
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
            (Phrase::NoAgent { name }, Pt) => format!("Não achei o agente {name}"),
            (Phrase::NoAgent { name }, En) => format!("I could not find agent {name}"),
            (Phrase::Text(text), _) => text.clone(),
        }
    }
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
    }
}
