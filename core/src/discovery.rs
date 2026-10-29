//! What this machine offers: agent CLIs on `PATH`, a local Ollama, editors and the
//! terminal used for terminal editors.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use crate::model::{Editor, Provider, ProviderOption};

pub const OLLAMA_URL: &str = "http://localhost:11434";

/// API services the `ApiRunner` speaks, with the model used when none is given.
pub const API_SERVICES: [(&str, &str); 4] = [
    ("anthropic", "claude-sonnet-5-5"),
    ("openai", "gpt-5"),
    ("openrouter", "anthropic/claude-sonnet-5.5"),
    ("gemini", "gemini-2.5-flash"),
];

pub fn which(binary: &str) -> Option<PathBuf> {
    if binary.contains('/') {
        let path = PathBuf::from(binary);
        return path.is_file().then_some(path);
    }
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).map(|dir| dir.join(binary)).find(|path| is_executable(path)))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// CLIs that speak ACP, natively or through an adapter.
pub fn acp_clis() -> Vec<ProviderOption> {
    let native: [(&str, &str, &str); 7] = [
        ("opencode", "opencode", "opencode acp"),
        ("gemini", "gemini", "gemini --experimental-acp"),
        ("claude", "claude-code-acp", "claude-code-acp"),
        ("claude", "claude-agent-acp", "claude-agent-acp"),
        ("codex", "codex-acp", "codex-acp"),
        ("goose", "goose", "goose acp"),
        ("qwen", "qwen", "qwen --experimental-acp"),
    ];
    let mut options: Vec<ProviderOption> = Vec::new();
    for (label, binary, command) in native {
        if which(binary).is_some() && !options.iter().any(|o| o.label == label) {
            options.push(acp(label, command));
        }
    }
    // Claude Code directly, asking permission through the app's MCP tool.
    if which("claude").is_some() {
        options.push(ProviderOption {
            label: "claude-mcp".into(),
            provider: Provider::CliMcp { command: "claude".into() },
            ready: true,
        });
    }
    // Adapters fetched by npx for CLIs that do not speak ACP themselves.
    if which("npx").is_some() {
        for (label, cli, package) in
            [("claude", "claude", "@zed-industries/claude-code-acp"), ("codex", "codex", "@zed-industries/codex-acp")]
        {
            if which(cli).is_some() && !options.iter().any(|o| o.label == label) {
                options.push(acp(label, &format!("npx -y {package}")));
            }
        }
    }
    options
}

fn acp(label: &str, command: &str) -> ProviderOption {
    ProviderOption { label: label.into(), provider: Provider::CliAcp { command: command.into() }, ready: true }
}

/// Models served by a local Ollama, as `ollama/<model>` options.
pub async fn ollama_models(url: &str) -> Vec<ProviderOption> {
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(2)).build() else {
        return Vec::new();
    };
    let Ok(response) = client.get(format!("{url}/api/tags")).send().await else {
        return Vec::new();
    };
    let Ok(json) = response.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    json["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["name"].as_str())
        .map(|name| ProviderOption {
            label: format!("ollama/{name}"),
            provider: Provider::Local { url: url.into(), model: name.into() },
            ready: true,
        })
        .collect()
}

pub fn api_options(has_key: impl Fn(&str) -> bool) -> Vec<ProviderOption> {
    API_SERVICES
        .iter()
        .map(|(service, model)| ProviderOption {
            label: (*service).into(),
            provider: Provider::Api {
                service: (*service).into(),
                model: (*model).into(),
                key_ref: crate::secrets::key_ref(service),
            },
            ready: has_key(service),
        })
        .collect()
}

pub fn editors() -> Vec<Editor> {
    let known: [(&str, &[&str], bool); 9] = [
        ("code", &["code"], false),
        ("zed", &["zeditor", "zed"], false),
        ("cursor", &["cursor"], false),
        ("subl", &["subl"], false),
        ("idea", &["idea", "intellij-idea-ultimate-edition", "idea.sh"], false),
        ("rustrover", &["rustrover", "rustrover.sh"], false),
        ("nvim", &["nvim"], true),
        ("helix", &["hx", "helix"], true),
        ("vim", &["vim"], true),
    ];
    known
        .iter()
        .filter_map(|(name, binaries, terminal)| {
            binaries.iter().find(|b| which(b).is_some()).map(|binary| Editor {
                name: (*name).into(),
                command: (*binary).into(),
                terminal: *terminal,
            })
        })
        .collect()
}

pub fn terminal() -> Option<String> {
    std::env::var("TERMINAL").ok().filter(|t| which(t).is_some()).or_else(|| {
        ["kitty", "ghostty", "alacritty", "foot", "wezterm", "konsole", "gnome-terminal", "xterm"]
            .iter()
            .find(|t| which(t).is_some())
            .map(|t| (*t).to_owned())
    })
}

/// Opens `path` in `editor`, detached; terminal editors run inside the terminal.
pub fn open_in(editor: &Editor, path: &Path) -> std::io::Result<()> {
    let mut command = if editor.terminal {
        let terminal = terminal().ok_or_else(|| std::io::Error::other("no terminal found"))?;
        let mut command = Command::new(&terminal);
        match terminal.as_str() {
            "gnome-terminal" => command.arg("--"),
            "wezterm" => command.arg("start").arg("--"),
            _ => command.arg("-e"),
        };
        command.arg(&editor.command).arg(path);
        command
    } else {
        let mut command = Command::new(&editor.command);
        command.arg(path);
        command
    };
    command.current_dir(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

/// Opens a web or mail link with the desktop's default handler. Only `http`,
/// `https` and `mailto` links: never files or programs.
pub fn open_url(url: &str) -> std::io::Result<()> {
    let allowed = ["http://", "https://", "mailto:"].iter().any(|scheme| url.starts_with(scheme));
    if !allowed {
        return Err(std::io::Error::other("only http, https and mailto links can be opened"));
    }
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Opens the system terminal in `path`.
pub fn open_terminal(path: &Path) -> std::io::Result<()> {
    let terminal = terminal().ok_or_else(|| std::io::Error::other("no terminal found"))?;
    let dir = path.to_string_lossy().into_owned();
    let mut command = Command::new(&terminal);
    match terminal.as_str() {
        "kitty" => command.arg("--directory").arg(&dir),
        "alacritty" => command.arg("--working-directory").arg(&dir),
        "foot" | "ghostty" | "gnome-terminal" => command.arg(format!("--working-directory={dir}")),
        "wezterm" => command.arg("start").arg("--cwd").arg(&dir),
        "konsole" => command.arg("--workdir").arg(&dir),
        _ => &mut command,
    };
    command.current_dir(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("/usr/bin/rm").is_err());
    }
}
