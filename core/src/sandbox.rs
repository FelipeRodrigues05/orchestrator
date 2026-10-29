//! Bubblewrap sandbox for agent processes (ADR 0006, step 9): the system is
//! read-only, the agent's worktree is writable, and so are the state folders the
//! agent CLIs need. `/tmp` is private.

use std::path::{Path, PathBuf};

use crate::discovery::which;

pub fn available() -> bool {
    which("bwrap").is_some()
}

/// Folders and files under `$HOME` agent CLIs write to (sessions, caches, auth).
const HOME_STATE: [&str; 21] = [
    ".cache",
    ".npm",
    ".bun",
    ".cargo/registry",
    ".cargo/git",
    ".codex",
    ".claude",
    ".claude.json",
    ".gemini",
    ".qwen",
    ".opencode",
    ".config/opencode",
    ".config/goose",
    ".config/github-copilot",
    ".local/share/opencode",
    ".local/state/opencode",
    ".local/share/claude",
    ".local/state/claude",
    ".local/share/goose",
    ".local/state/goose",
    ".local/share/pnpm",
];

/// The main repository's `.git` for a worktree (`.git` there is a file pointing to
/// `<root>/.git/worktrees/<name>`); commits made in the worktree write there.
pub fn git_common_dir(worktree: &Path) -> Option<PathBuf> {
    let pointer = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let gitdir = pointer.trim().strip_prefix("gitdir:")?.trim();
    let (common, _) = gitdir.split_once("/worktrees/")?;
    Some(PathBuf::from(common))
}

/// `program args…` run inside bubblewrap with `worktree` as the only project
/// folder it can write.
pub fn wrap(program: &str, args: &[String], worktree: &Path) -> (String, Vec<String>) {
    let mut out: Vec<String> =
        ["--die-with-parent", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
    let mut bind = |path: &Path| {
        let path = path.to_string_lossy().into_owned();
        out.extend(["--bind-try".to_owned(), path.clone(), path]);
    };
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for entry in HOME_STATE {
            bind(&home.join(entry));
        }
    }
    // Extra writable paths, colon separated, for CLIs this list does not know.
    if let Some(extra) = std::env::var_os("ORCH_SANDBOX_BIND") {
        for path in std::env::split_paths(&extra) {
            bind(&path);
        }
    }
    if let Some(git) = git_common_dir(worktree) {
        bind(&git);
    }
    // Last, so it wins over the private /tmp when the worktree lives there.
    bind(worktree);
    out.extend(["--chdir".to_owned(), worktree.to_string_lossy().into_owned(), "--".to_owned(), program.to_owned()]);
    out.extend(args.iter().cloned());
    ("bwrap".into(), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_main_repository_of_a_worktree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".git"), "gitdir: /home/me/shop/.git/worktrees/tests-1\n").unwrap();
        assert_eq!(git_common_dir(dir.path()), Some(PathBuf::from("/home/me/shop/.git")));
    }

    #[test]
    fn writes_land_only_in_the_worktree() {
        if !available() {
            return;
        }
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        let script = format!("touch inside.txt; touch {}/escaped.txt 2>/dev/null; true", outside.path().display());
        let (program, args) = wrap("sh", &["-c".into(), script], worktree.path());
        let status = std::process::Command::new(program).args(args).status().unwrap();
        assert!(status.success());
        assert!(worktree.path().join("inside.txt").exists());
        assert!(!outside.path().join("escaped.txt").exists(), "the sandbox let a write out");
    }
}
