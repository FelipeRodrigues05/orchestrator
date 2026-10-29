//! Spoken (and typed) command parser. Pure: tested with a phrase table.
//!
//! The phrases are Portuguese because the operator speaks Portuguese (ADR 0013);
//! the English words are accepted for the typed command line.

use std::sync::LazyLock;

use regex::Regex;

use crate::{broker::Answer, model::normalize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spoken {
    /// Answers the open permission request.
    Answer(Answer),
    /// "resumo": the pet reads the room state out loud.
    Summary,
    /// Text for an agent. Without names it goes to the agent selected in the UI.
    Message { group: Option<String>, agent: Option<String>, text: String },
}

static ROUTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:grupo|group)\s+(?P<group>[^,:;]+?)\s*[,;]?\s+(?:agente|agent)\s+(?P<agent>[^,:;.]+?)\s*[:,;.]\s*(?P<text>.+?)\s*$",
    )
    .expect("valid regex")
});

static AGENT_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:agente|agent)\s+(?P<agent>[^,:;.]+?)\s*[:,;.]\s*(?P<text>.+?)\s*$").expect("valid regex")
});

pub fn parse(text: &str) -> Spoken {
    match normalize(text).as_str() {
        "permitir" | "permite" | "permita" | "pode" | "allow" => {
            return Spoken::Answer(Answer::Allow);
        }
        "sempre" | "sempre permitir" | "permitir sempre" | "always" | "always allow" => {
            return Spoken::Answer(Answer::AlwaysAllow);
        }
        "negar" | "nega" | "negue" | "nao" | "deny" => return Spoken::Answer(Answer::Deny),
        "resumo" | "status" | "estado" | "summary" => return Spoken::Summary,
        _ => {}
    }
    if let Some(captures) = ROUTED.captures(text) {
        return Spoken::Message {
            group: Some(captures["group"].trim().to_owned()),
            agent: Some(captures["agent"].trim().to_owned()),
            text: captures["text"].to_owned(),
        };
    }
    if let Some(captures) = AGENT_ONLY.captures(text) {
        return Spoken::Message {
            group: None,
            agent: Some(captures["agent"].trim().to_owned()),
            text: captures["text"].to_owned(),
        };
    }
    Spoken::Message { group: None, agent: None, text: text.trim().to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(group: Option<&str>, agent: Option<&str>, text: &str) -> Spoken {
        Spoken::Message { group: group.map(Into::into), agent: agent.map(Into::into), text: text.into() }
    }

    #[test]
    fn phrase_table() {
        let table = [
            ("permitir", Spoken::Answer(Answer::Allow)),
            ("Permitir.", Spoken::Answer(Answer::Allow)),
            ("  PERMITE!", Spoken::Answer(Answer::Allow)),
            ("sempre", Spoken::Answer(Answer::AlwaysAllow)),
            ("Sempre permitir.", Spoken::Answer(Answer::AlwaysAllow)),
            ("negar", Spoken::Answer(Answer::Deny)),
            ("Não.", Spoken::Answer(Answer::Deny)),
            ("deny", Spoken::Answer(Answer::Deny)),
            ("Resumo.", Spoken::Summary),
            (
                "Grupo API, agente Testes: escreva o teste do refund.",
                message(Some("API"), Some("Testes"), "escreva o teste do refund."),
            ),
            (
                "grupo Front agente Checkout, use o tema escuro",
                message(Some("Front"), Some("Checkout"), "use o tema escuro"),
            ),
            ("Agente Backend: rode o clippy", message(None, Some("Backend"), "rode o clippy")),
            ("group API, agent Tests: run the suite", message(Some("API"), Some("Tests"), "run the suite")),
            ("adicione logs no handler", message(None, None, "adicione logs no handler")),
            ("permitir o acesso ao banco", message(None, None, "permitir o acesso ao banco")),
        ];
        for (phrase, expected) in table {
            assert_eq!(parse(phrase), expected, "{phrase:?}");
        }
    }
}
