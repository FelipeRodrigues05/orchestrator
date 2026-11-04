//! What agents remember between tasks (ADR 0023): one Markdown file per agent, or
//! per global agent so the memory travels with it across projects. Agents write it
//! with the `remember` tool; the operator with `:remember`. The latest notes go
//! into the agent's briefing.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use uuid::Uuid;

/// The notes a briefing carries at most (the newest ones).
pub const BRIEFING_LIMIT: usize = 3_000;

/// `<dir>/<key>.md`; `key` is the agent's profile id, or its own id.
pub fn path(dir: &Path, key: Uuid) -> PathBuf {
    dir.join(format!("{key}.md"))
}

/// Adds a note as a dated line.
pub fn append(dir: &Path, key: Uuid, note: &str) -> io::Result<()> {
    let note = note.split_whitespace().collect::<Vec<_>>().join(" ");
    if note.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path(dir, key))?;
    writeln!(file, "- {}: {note}", chrono::Local::now().format("%Y-%m-%d"))
}

/// The newest notes that fit in `limit` characters, whole lines only, oldest
/// first; empty when there are none.
pub fn recent(dir: &Path, key: Uuid, limit: usize) -> String {
    let Ok(text) = fs::read_to_string(path(dir, key)) else { return String::new() };
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0;
    for line in text.lines().rev().filter(|l| !l.trim().is_empty()) {
        size += line.chars().count() + 1;
        if size > limit {
            break;
        }
        kept.push(line);
    }
    kept.reverse();
    kept.join("\n")
}

/// Moves one memory to another key (an agent that becomes a global agent keeps
/// what it learnt); notes already under `to` are kept and the moved ones follow.
pub fn rename(dir: &Path, from: Uuid, to: Uuid) -> io::Result<()> {
    let source = path(dir, from);
    let Ok(text) = fs::read_to_string(&source) else { return Ok(()) };
    fs::create_dir_all(dir)?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path(dir, to))?;
    file.write_all(text.as_bytes())?;
    fs::remove_file(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_kept_newest_last_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let key = Uuid::new_v4();
        assert_eq!(recent(dir.path(), key, 100), "");
        append(dir.path(), key, "prefers Pest\n over   PHPUnit").unwrap();
        append(dir.path(), key, "the API lives in app/Http").unwrap();
        let notes = recent(dir.path(), key, 1_000);
        assert!(notes.ends_with("the API lives in app/Http"), "{notes}");
        assert!(notes.contains("prefers Pest over PHPUnit"), "{notes}");
        // Only what fits, newest first to go in.
        assert!(recent(dir.path(), key, 40).contains("app/Http"));
        assert!(!recent(dir.path(), key, 40).contains("Pest"));

        let other = Uuid::new_v4();
        rename(dir.path(), key, other).unwrap();
        assert_eq!(recent(dir.path(), key, 1_000), "");
        assert!(recent(dir.path(), other, 1_000).contains("Pest"));
    }
}
