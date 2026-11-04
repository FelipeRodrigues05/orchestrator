//! The app's minimal tool set for API providers: read, edit, search and run a
//! command, all confined to the agent's worktree.

use std::fmt::Write as _;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use regex::Regex;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::model::resolve;

const MAX_OUTPUT: usize = 16 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_file",
            description: "Read a text file from the working directory.",
            schema: json!({ "type": "object", "properties": {
                "path": { "type": "string", "description": "Path relative to the working directory" }
            }, "required": ["path"] }),
        },
        ToolDef {
            name: "edit_file",
            description: "Replace old_text with new_text in a file (old_text must appear exactly once). \
                          With an empty old_text the file is created or overwritten with new_text.",
            schema: json!({ "type": "object", "properties": {
                "path": { "type": "string" },
                "old_text": { "type": "string" },
                "new_text": { "type": "string" }
            }, "required": ["path", "old_text", "new_text"] }),
        },
        ToolDef {
            name: "search",
            description: "Search files for a regular expression. Returns path:line: text matches.",
            schema: json!({ "type": "object", "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string", "description": "Folder to search, default ." }
            }, "required": ["pattern"] }),
        },
        ToolDef {
            name: "run_command",
            description: "Run a shell command in the working directory and return its output.",
            schema: json!({ "type": "object", "properties": {
                "command": { "type": "string" }
            }, "required": ["command"] }),
        },
    ]
}

fn text<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The broker's view of a call: tool name, input and the paths it touches.
pub fn permission_of(call: &ToolCall) -> (String, Value, Vec<PathBuf>) {
    let path = text(&call.input, "path");
    match call.name.as_str() {
        "read_file" => ("Read".into(), json!({ "file_path": path }), vec![PathBuf::from(path)]),
        "edit_file" => ("Edit".into(), json!({ "file_path": path }), vec![PathBuf::from(path)]),
        "search" => {
            let folder = if path.is_empty() { "." } else { path };
            ("Search".into(), json!({ "path": folder }), vec![PathBuf::from(folder)])
        }
        "run_command" => ("Bash".into(), json!({ "command": text(&call.input, "command") }), Vec::new()),
        other => (other.to_owned(), call.input.clone(), Vec::new()),
    }
}

/// `path` resolved against the worktree; it must stay in it or in an extra folder.
fn confined(worktree: &Path, extra: &[PathBuf], path: &str) -> Result<PathBuf, String> {
    let full = resolve(worktree, Path::new(path));
    if full.starts_with(worktree) || extra.iter().any(|folder| full.starts_with(folder)) {
        Ok(full)
    } else {
        Err(format!("{path} is outside the working directories"))
    }
}

fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… (truncated)");
    }
    text
}

/// Runs a tool in `worktree`; `extra` are the agent's folders in the group's other
/// projects (ADR 0024), reachable by absolute path.
pub async fn run(call: &ToolCall, worktree: &Path, extra: &[PathBuf], sandbox: bool) -> Result<String, String> {
    let input = &call.input;
    match call.name.as_str() {
        "read_file" => {
            let path = confined(worktree, extra, text(input, "path"))?;
            tokio::fs::read_to_string(&path).await.map(truncate).map_err(|e| e.to_string())
        }
        "edit_file" => {
            let path = confined(worktree, extra, text(input, "path"))?;
            let (old, new) = (text(input, "old_text"), text(input, "new_text"));
            if old.is_empty() {
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
                }
                tokio::fs::write(&path, new).await.map_err(|e| e.to_string())?;
                return Ok(format!("wrote {}", text(input, "path")));
            }
            let current = tokio::fs::read_to_string(&path).await.map_err(|e| e.to_string())?;
            match current.matches(old).count() {
                1 => {
                    tokio::fs::write(&path, current.replacen(old, new, 1)).await.map_err(|e| e.to_string())?;
                    Ok(format!("edited {}", text(input, "path")))
                }
                0 => Err("old_text not found".into()),
                n => Err(format!("old_text appears {n} times; add more context")),
            }
        }
        "search" => {
            let folder = text(input, "path");
            let folder = confined(worktree, extra, if folder.is_empty() { "." } else { folder })?;
            let pattern = Regex::new(text(input, "pattern")).map_err(|e| e.to_string())?;
            let worktree = worktree.to_path_buf();
            tokio::task::spawn_blocking(move || search(&folder, &worktree, &pattern)).await.map_err(|e| e.to_string())
        }
        "run_command" => {
            let command = text(input, "command");
            let shell = ["-c".to_owned(), command.to_owned()];
            let (program, args) = if sandbox && crate::sandbox::available() {
                crate::sandbox::wrap("sh", &shell, worktree, extra)
            } else {
                ("sh".to_owned(), shell.to_vec())
            };
            let child = Command::new(program)
                .args(args)
                .current_dir(worktree)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| e.to_string())?;
            let output = tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output())
                .await
                .map_err(|_| format!("timed out after {}s", COMMAND_TIMEOUT.as_secs()))?
                .map_err(|e| e.to_string())?;
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            let _ = write!(text, "\n[exit {}]", output.status.code().unwrap_or(-1));
            Ok(truncate(text))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

fn search(folder: &Path, worktree: &Path, pattern: &Regex) -> String {
    let mut found = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !name.starts_with('.') && !matches!(name.as_str(), "target" | "node_modules") {
                    stack.push(path);
                }
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (number, line) in content.lines().enumerate() {
                if pattern.is_match(line) {
                    let shown = path.strip_prefix(worktree).unwrap_or(&path).display();
                    found.push(format!("{shown}:{}: {}", number + 1, line.trim()));
                    if found.len() >= 200 {
                        return found.join("\n") + "\n… (more matches)";
                    }
                }
            }
        }
    }
    if found.is_empty() { "no matches".into() } else { found.join("\n") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, input: Value) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), input }
    }

    #[tokio::test]
    async fn edit_read_search_and_run_inside_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run(
            &call("edit_file", json!({ "path": "src/a.rs", "old_text": "", "new_text": "fn a() {}\n" })),
            root,
            &[],
            false,
        )
        .await
        .unwrap();
        run(&call("edit_file", json!({ "path": "src/a.rs", "old_text": "a()", "new_text": "b()" })), root, &[], false)
            .await
            .unwrap();
        assert_eq!(
            run(&call("read_file", json!({ "path": "src/a.rs" })), root, &[], false).await.unwrap(),
            "fn b() {}\n"
        );
        let found = run(&call("search", json!({ "pattern": "fn b" })), root, &[], false).await.unwrap();
        assert_eq!(found, "src/a.rs:1: fn b() {}");
        let output = run(&call("run_command", json!({ "command": "ls src" })), root, &[], false).await.unwrap();
        assert!(output.starts_with("a.rs") && output.ends_with("[exit 0]"));
    }

    #[tokio::test]
    async fn paths_outside_the_worktree_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let escape = call("read_file", json!({ "path": "../../etc/passwd" }));
        assert!(run(&escape, dir.path(), &[], false).await.is_err());
        let (tool, _, paths) = permission_of(&escape);
        assert_eq!((tool.as_str(), paths.len()), ("Read", 1));
    }
}
