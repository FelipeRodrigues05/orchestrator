//! Keeping agent CLIs current. Knowing how each CLI updates itself is CLI-specific,
//! so it lives with the runners (ADR 0004).

use std::{process::Stdio, sync::LazyLock, time::Duration};

use regex::Regex;
use tokio::process::Command;

use crate::{discovery::which, model::Provider};

const UPDATE_TIMEOUT: Duration = Duration::from_secs(300);

static OUTDATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(newer version|please (upgrade|update)|upgrade to the latest|update to the latest|is outdated|out of date|unsupported (protocol|client) version|version .* (is )?no longer supported)",
    )
    .expect("valid regex")
});

/// Whether an agent error says its CLI is too old.
pub fn looks_outdated(error: &str) -> bool {
    OUTDATED.is_match(error)
}

/// `npx -y pkg` → `npx -y pkg@latest`, so adapters fetched by npx never stay on a
/// stale cached version. Other commands are unchanged.
pub fn latest_command(command: &str) -> String {
    let mut words: Vec<String> = shell_words::split(command).unwrap_or_else(|_| vec![command.to_owned()]);
    let is_npx = words.first().is_some_and(|w| w.rsplit('/').next() == Some("npx"));
    if is_npx
        && let Some(package) = words.iter_mut().skip(1).find(|w| !w.starts_with('-'))
        && !has_version(package)
    {
        package.push_str("@latest");
    }
    shell_words::join(words)
}

/// `@scope/name@1.2` and `name@latest` carry a version; `@scope/name` does not.
fn has_version(package: &str) -> bool {
    package.trim_start_matches('@').contains('@')
}

/// How to update the CLI behind a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub label: String,
    /// `None`: nothing to install; restarting picks up the latest (npx `@latest`).
    pub argv: Option<Vec<String>>,
}

fn npm_global(package: &str) -> Option<Vec<String>> {
    which("npm").map(|_| vec!["npm".into(), "install".into(), "-g".into(), format!("{package}@latest")])
}

pub fn plan(provider: &Provider) -> Option<Plan> {
    let (Provider::CliAcp { command } | Provider::CliMcp { command }) = provider else {
        return None;
    };
    let words = shell_words::split(command).ok()?;
    let program = words.first()?.rsplit('/').next()?.to_owned();
    if program == "npx" {
        // `npx -y @zed-industries/codex-acp` is the adapter, not npx.
        let package = words.iter().skip(1).find(|w| !w.starts_with('-'))?;
        let name = package.rsplit('/').next()?.split('@').next()?.to_owned();
        let label = if name.is_empty() { package.clone() } else { name };
        return Some(Plan { label, argv: None });
    }
    let argv = |args: &[&str]| Some(args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
    let argv = match program.as_str() {
        "opencode" => argv(&["opencode", "upgrade"]),
        "claude" => argv(&["claude", "update"]),
        "codex" => argv(&["codex", "update"]),
        "goose" => argv(&["goose", "update"]),
        "gemini" => npm_global("@google/gemini-cli"),
        "qwen" => npm_global("@qwen-code/qwen-code"),
        "claude-code-acp" | "claude-agent-acp" | "codex-acp" => npm_global(&format!("@zed-industries/{program}")),
        _ => return None,
    };
    Some(Plan { label: program, argv })
}

/// Runs the update; returns the last lines of its output.
pub async fn run(plan: &Plan) -> Result<String, String> {
    let Some(argv) = &plan.argv else {
        return Ok("the adapter is fetched at its latest version on start".into());
    };
    let (program, args) = argv.split_first().ok_or("empty update command")?;
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let output = tokio::time::timeout(UPDATE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{} timed out", argv.join(" ")))?
        .map_err(|e| e.to_string())?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join(" · ");
    if output.status.success() { Ok(tail) } else { Err(format!("{} failed: {tail}", argv.join(" "))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_outdated_cli_errors() {
        assert!(looks_outdated(
            r#"{"message":"The 'gpt-5.6-sol' model requires a newer version of Codex. Please upgrade to the latest app"}"#
        ));
        assert!(looks_outdated("Your CLI is outdated, run update"));
        assert!(!looks_outdated("permission denied: no response"));
    }

    #[test]
    fn npx_adapters_always_run_at_latest() {
        assert_eq!(latest_command("npx -y @zed-industries/codex-acp"), "npx -y @zed-industries/codex-acp@latest");
        let plan = plan(&Provider::CliAcp { command: "npx -y @zed-industries/codex-acp@0.16.0".into() }).unwrap();
        assert_eq!((plan.label.as_str(), plan.argv), ("codex-acp", None));
        assert_eq!(
            latest_command("npx -y @zed-industries/codex-acp@0.16.0"),
            "npx -y @zed-industries/codex-acp@0.16.0"
        );
        assert_eq!(latest_command("opencode acp"), "opencode acp");
    }

    #[test]
    fn plans_per_cli() {
        let acp = |command: &str| Provider::CliAcp { command: command.into() };
        assert_eq!(plan(&acp("opencode acp")).unwrap().argv, Some(vec!["opencode".into(), "upgrade".into()]));
        assert_eq!(plan(&acp("npx -y @zed-industries/codex-acp")).unwrap().argv, None);
        assert!(plan(&Provider::Local { url: String::new(), model: String::new() }).is_none());
    }
}
