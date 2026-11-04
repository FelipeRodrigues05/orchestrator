//! Model router (ADR 0020): when an agent hits a usage limit, its provider/model is
//! set aside for a while and the agent goes on with the closest model that is
//! still available, keeping its conversation.

use std::{
    collections::HashMap,
    sync::LazyLock,
    time::{Duration, Instant},
};

use regex::Regex;

use crate::{
    model::{Provider, ProviderOption},
    runner::mcp_cli::CLAUDE_MODELS,
};

/// How long a provider/model that hit its limit is left alone.
pub const COOLDOWN: Duration = Duration::from_secs(60 * 60);

/// Whether an agent's error means it ran out of tokens, quota or rate.
pub fn is_limit(error: &str) -> bool {
    static LIMIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)usage limit|rate[ _-]?limit|quota|\b429\b|too many requests|credit balance|insufficient[ _]credits|limit (reached|exceeded)|out of (tokens|credits)|resource[ _]exhausted",
        )
        .expect("valid regex")
    });
    LIMIT.is_match(error)
}

/// Whether an error says the CLI cannot run the model it was asked for (an adapter
/// older than the model, a typo, a model of another plan).
pub fn is_unsupported_model(error: &str) -> bool {
    static UNSUPPORTED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)model\b[^.\n]{0,120}?(requires a newer version|is not supported|not supported|does not exist|is not available|unknown model|invalid model)|(unknown|invalid|unsupported) model",
        )
        .expect("valid regex")
    });
    UNSUPPORTED.is_match(error)
}

/// The human part of an error that arrives wrapped in JSON (`{"error":{"message":…}}`,
/// sometimes escaped twice): the innermost `message`, or the text as it came.
pub fn readable_error(error: &str) -> String {
    static MESSAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\\*"message\\*"\s*:\s*\\*"((?:[^"\\]|\\[^"])+)"#).expect("valid regex"));
    MESSAGE
        .captures_iter(error)
        .last()
        .map_or_else(|| error.trim().to_owned(), |found| found[1].replace("\\n", " ").trim().to_owned())
}

/// How capable a model is, from its name: 3 frontier, 2 mid, 1 small or local.
/// Unknown names count as mid.
pub fn tier(model: &str) -> u8 {
    // "gemini" would read as "mini".
    let model = model.to_ascii_lowercase().replace("gemini", "gem");
    let has = |words: &[&str]| words.iter().any(|w| model.contains(w));
    if has(&["haiku", "mini", "nano", "lite", "flash", "small", "llama", "qwen", "gemma", "phi", "mistral"]) {
        1
    } else if has(&["opus", "astra", "-pro", " pro", "ultra", "max", "o3", "o4", "gpt-5", "gpt-6"]) {
        3
    } else {
        2
    }
}

/// Where an agent can go: a provider and, if it matters, a model of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub provider: Provider,
    /// For CLIs: the model to ask for (`None`: the CLI's default). API and local
    /// providers carry their model inside `provider`.
    pub model: Option<String>,
}

impl Candidate {
    /// `codex/gpt-6-astra`, `claude`, `anthropic/claude-sonnet-5-5`.
    pub fn name(&self) -> String {
        match (&self.provider, &self.model) {
            (Provider::Api { model, .. } | Provider::Local { model, .. }, _) | (_, Some(model)) => {
                format!("{}/{model}", self.label)
            }
            _ => self.label.clone(),
        }
    }

    fn model_name(&self) -> String {
        match (&self.provider, &self.model) {
            (Provider::Api { model, .. } | Provider::Local { model, .. }, _) | (_, Some(model)) => model.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Router {
    /// `label/model` (or `label`) → when it hit its limit.
    exhausted: HashMap<String, Instant>,
    /// Models each provider label reported while running.
    models: HashMap<String, Vec<String>>,
    /// The operator's order (`:fallback a, b/c`); empty: by closeness.
    pub chain: Vec<String>,
}

impl Router {
    pub fn learn_models(&mut self, label: &str, models: Vec<String>) {
        if !models.is_empty() {
            self.models.insert(label.to_owned(), models);
        }
    }

    pub fn exhaust(&mut self, candidate: &Candidate) {
        self.exhausted.insert(candidate.name(), Instant::now());
    }

    pub fn reset(&mut self) {
        self.exhausted.clear();
    }

    fn available(&self, candidate: &Candidate) -> bool {
        self.exhausted.get(&candidate.name()).is_none_or(|at| at.elapsed() >= COOLDOWN)
    }

    /// Every provider/model the agent could use, from the ready providers.
    pub fn candidates(&self, options: &[ProviderOption]) -> Vec<Candidate> {
        let mut out = Vec::new();
        for option in options.iter().filter(|o| o.ready) {
            let base = Candidate { label: option.label.clone(), provider: option.provider.clone(), model: None };
            let known: Vec<String> = match &option.provider {
                Provider::CliAcp { .. } | Provider::CliMcp { .. } => {
                    self.models.get(&option.label).cloned().unwrap_or_else(|| {
                        if option.label.contains("claude") {
                            CLAUDE_MODELS.iter().map(|m| (*m).to_owned()).collect()
                        } else {
                            Vec::new()
                        }
                    })
                }
                Provider::Api { .. } | Provider::Local { .. } => Vec::new(),
            };
            if known.is_empty() {
                out.push(base);
            } else {
                out.extend(known.into_iter().map(|model| Candidate { model: Some(model), ..base.clone() }));
            }
        }
        out
    }

    /// The closest other model of the same provider (the CLI cannot run `current`),
    /// or the closest model anywhere when it offers no other.
    pub fn next_model(&self, current: &Candidate, options: &[ProviderOption]) -> Option<Candidate> {
        let level = tier(&current.model_name());
        self.candidates(options)
            .into_iter()
            .filter(|c| {
                c.label == current.label && c.model.is_some() && c.name() != current.name() && self.available(c)
            })
            .min_by_key(|c| {
                let other = tier(&c.model_name());
                (i16::from(level) - i16::from(other)).unsigned_abs() * 2 + u16::from(other > level)
            })
            .or_else(|| self.next(current, options))
    }

    /// The closest available model to `current`: the operator's order when set,
    /// otherwise the same tier first (another provider before a smaller model of
    /// the same one), then the tiers below, then above.
    pub fn next(&self, current: &Candidate, options: &[ProviderOption]) -> Option<Candidate> {
        let candidates: Vec<Candidate> =
            self.candidates(options).into_iter().filter(|c| c.name() != current.name() && self.available(c)).collect();
        if !self.chain.is_empty() {
            return self
                .chain
                .iter()
                .find_map(|wanted| candidates.iter().find(|c| c.name() == *wanted || c.label == *wanted).cloned());
        }
        let level = tier(&current.model_name());
        candidates.into_iter().min_by_key(|c| {
            let other = tier(&c.model_name());
            // Lower tiers before higher ones at the same distance: cheaper, and a
            // limit on a big model is often per model.
            let distance = (i16::from(level) - i16::from(other)).unsigned_abs() * 2 + u16::from(other > level);
            (distance, c.label == current.label)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(label: &str, provider: Provider) -> ProviderOption {
        ProviderOption { label: label.into(), provider, ready: true }
    }

    fn options() -> Vec<ProviderOption> {
        vec![
            option("claude", Provider::CliMcp { command: "claude".into() }),
            option("codex", Provider::CliAcp { command: "codex-acp".into() }),
            option("ollama", Provider::Local { url: "http://localhost:11434".into(), model: "qwen3:8b".into() }),
        ]
    }

    fn claude(model: &str) -> Candidate {
        Candidate {
            label: "claude".into(),
            provider: Provider::CliMcp { command: "claude".into() },
            model: Some(model.into()),
        }
    }

    #[test]
    fn recognises_limits_not_other_errors() {
        for limit in [
            "Claude AI usage limit reached|1793300000",
            "Error 429: Too Many Requests",
            "You exceeded your current quota, please check your plan",
            "Your credit balance is too low to access the Anthropic API",
            "rate_limit_error: Number of request tokens has exceeded your per-minute rate limit",
        ] {
            assert!(is_limit(limit), "{limit}");
        }
        for other in ["could not start opencode: No such file", "permission denied: outside the worktree"] {
            assert!(!is_limit(other), "{other}");
        }
    }

    #[test]
    fn opus_out_goes_to_the_closest_model_elsewhere() {
        let mut router = Router::default();
        router.learn_models("codex", vec!["gpt-6-mini".into(), "gpt-6-astra".into()]);
        let opus = claude("opus");
        router.exhaust(&opus);
        let next = router.next(&opus, &options()).unwrap();
        assert_eq!(next.name(), "codex/gpt-6-astra");

        // That one out too: a mid model next (sonnet), before the small ones.
        router.exhaust(&next);
        assert_eq!(router.next(&next, &options()).unwrap().name(), "claude/sonnet");
    }

    #[test]
    fn the_operator_order_wins_and_exhausted_ones_are_skipped() {
        let mut router = Router { chain: vec!["ollama".into(), "codex".into()], ..Router::default() };
        let opus = claude("opus");
        assert_eq!(router.next(&opus, &options()).unwrap().label, "ollama");
        let ollama = router.next(&opus, &options()).unwrap();
        router.exhaust(&ollama);
        assert_eq!(router.next(&opus, &options()).unwrap().label, "codex");
        router.reset();
        assert_eq!(router.next(&opus, &options()).unwrap().label, "ollama");
    }

    #[test]
    fn unsupported_models_stay_on_the_provider_when_they_can() {
        let codex_error = r#"Internal error: {"message": "{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-5.6-sol' model requires a newer version of Codex. Please upgrade to the latest app or CLI and try again.\"}}", "codex_error_info": "other"}"#;
        assert!(is_unsupported_model(codex_error));
        assert!(!is_limit(codex_error));
        assert_eq!(
            readable_error(codex_error),
            "The 'gpt-5.6-sol' model requires a newer version of Codex. Please upgrade to the latest app or CLI and try again."
        );
        assert_eq!(readable_error("plain words"), "plain words");
        assert!(!is_unsupported_model("Model metadata missing; using fallback"));

        let mut router = Router::default();
        router.learn_models("codex", vec!["gpt-5.6-sol".into(), "gpt-5.5".into(), "gpt-5.5-mini".into()]);
        let sol = Candidate {
            label: "codex".into(),
            provider: Provider::CliAcp { command: "codex-acp".into() },
            model: Some("gpt-5.6-sol".into()),
        };
        router.exhaust(&sol);
        assert_eq!(router.next_model(&sol, &options()).unwrap().name(), "codex/gpt-5.5");
    }

    #[test]
    fn tiers_from_names() {
        assert_eq!(tier("opus"), 3);
        assert_eq!(tier("gpt-6-astra"), 3);
        assert_eq!(tier("claude-sonnet-5-5"), 2);
        assert_eq!(tier("haiku"), 1);
        assert_eq!(tier("gpt-6-mini"), 1);
        assert_eq!(tier("qwen3:8b"), 1);
        assert_eq!(tier("gemini-2.5-pro"), 3);
        assert_eq!(tier("gemini-2.5-flash"), 1);
    }
}
