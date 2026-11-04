//! One git worktree and branch per agent (ADR 0006).

use std::path::{Path, PathBuf};

use tokio::process::Command;
use uuid::Uuid;

/// `<root>/.worktrees/<name>-<short id>`: unique per agent even with equal names.
pub fn path_for(root: &Path, name: &str, id: Uuid) -> PathBuf {
    root.join(".worktrees").join(format!("{}-{}", slug(name), &id.simple().to_string()[..8]))
}

pub fn slug(name: &str) -> String {
    let slug: String =
        crate::model::normalize(name).chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() { "agent".into() } else { slug }
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").args(args).current_dir(dir).output().await.map_err(|e| format!("git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Makes `root` a git repository with a first (empty) commit, unless it is one.
pub async fn init_repository(root: &Path) -> Result<(), String> {
    tokio::fs::create_dir_all(root).await.map_err(|e| format!("{}: {e}", root.display()))?;
    if !is_repository(root).await {
        git(root, &["init", "-q"]).await?;
    }
    if git(root, &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        let identity = ["-c", "user.name=Orchestrator", "-c", "user.email=orchestrator@localhost"];
        let mut args = identity.to_vec();
        args.extend(["commit", "-q", "--allow-empty", "-m", "Personal workspace"]);
        git(root, &args).await?;
    }
    Ok(())
}

pub async fn is_repository(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"]).await.is_ok_and(|out| out == "true")
}

/// Creates the worktree on branch `orch/<dir name>` unless it already exists.
pub async fn ensure(root: &Path, worktree: &Path) -> Result<(), String> {
    if worktree.join(".git").exists() {
        return Ok(());
    }
    if !is_repository(root).await {
        return Err(format!("{} is not a git repository", root.display()));
    }
    if git(root, &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        return Err("the repository has no commits yet; commit once so agents can branch".into());
    }
    exclude_worktrees(root).await;
    let name = worktree.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let branch = format!("orch/{name}");
    let path = worktree.to_string_lossy().into_owned();
    let exists = git(root, &["rev-parse", "--verify", &format!("refs/heads/{branch}")]).await.is_ok();
    let args: Vec<&str> =
        if exists { vec!["worktree", "add", &path, &branch] } else { vec!["worktree", "add", "-b", &branch, &path] };
    git(root, &args).await.map(|_| ())
}

/// Commits what the agent left in its worktree and merges its branch into the
/// project's checked-out branch. On conflict the merge is aborted and the files
/// are listed in the error.
pub async fn merge(root: &Path, worktree: &Path, message: &str) -> Result<String, String> {
    if !worktree.join(".git").exists() {
        return Err("the agent has no worktree yet".into());
    }
    let dirty = !git(worktree, &["status", "--porcelain"]).await?.is_empty();
    if dirty {
        git(worktree, &["add", "-A"]).await?;
        // Fall back to a neutral identity when the repository has none.
        if git(worktree, &["commit", "-q", "-m", message]).await.is_err() {
            let identity = ["-c", "user.name=Orchestrator", "-c", "user.email=orchestrator@localhost"];
            let mut args = identity.to_vec();
            args.extend(["commit", "-q", "-m", message]);
            git(worktree, &args).await?;
        }
    }
    let branch = git(worktree, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    let ahead = git(root, &["rev-list", "--count", &format!("HEAD..{branch}")]).await?;
    if ahead == "0" {
        return Ok(format!("{branch} has nothing new to merge"));
    }
    let merged = if let Ok(output) = git(root, &["merge", "--no-edit", &branch]).await {
        Ok(output)
    } else {
        let identity = ["-c", "user.name=Orchestrator", "-c", "user.email=orchestrator@localhost"];
        let mut args = identity.to_vec();
        args.extend(["merge", "--no-edit", branch.as_str()]);
        git(root, &args).await
    };
    match merged {
        Ok(_) => Ok(format!("merged {branch} ({ahead} commits)")),
        Err(error) => {
            let conflicts = git(root, &["diff", "--name-only", "--diff-filter=U"]).await.unwrap_or_default();
            let _ = git(root, &["merge", "--abort"]).await;
            if conflicts.is_empty() {
                Err(format!("merge failed: {error}"))
            } else {
                Err(format!("conflicts, merge aborted: {}", conflicts.lines().collect::<Vec<_>>().join(", ")))
            }
        }
    }
}

/// Keeps `.worktrees/` out of `git status` without touching the tracked `.gitignore`.
async fn exclude_worktrees(root: &Path) {
    let Ok(dir) = git(root, &["rev-parse", "--git-common-dir"]).await else {
        return;
    };
    let exclude = root.join(dir).join("info").join("exclude");
    let current = tokio::fs::read_to_string(&exclude).await.unwrap_or_default();
    if current.lines().any(|line| line.trim() == "/.worktrees/") {
        return;
    }
    if let Some(parent) = exclude.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&exclude, format!("{current}\n/.worktrees/\n")).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            git(dir.path(), &args).await.unwrap();
        }
        dir
    }

    #[tokio::test]
    async fn two_agents_never_share_a_worktree() {
        let repo = repo().await;
        let a = path_for(repo.path(), "Tests", Uuid::new_v4());
        let b = path_for(repo.path(), "Tests", Uuid::new_v4());
        assert_ne!(a, b);
        ensure(repo.path(), &a).await.unwrap();
        ensure(repo.path(), &b).await.unwrap();
        ensure(repo.path(), &a).await.unwrap();
        assert!(a.join(".git").exists() && b.join(".git").exists());
        let status = git(repo.path(), &["status", "--porcelain"]).await.unwrap();
        assert!(status.is_empty(), "worktrees should be excluded: {status}");
    }

    #[tokio::test]
    async fn merges_the_agent_work_and_aborts_on_conflicts() {
        let repo = repo().await;
        let root = repo.path();
        std::fs::write(root.join("shared.txt"), "base\n").unwrap();
        for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "base"]]
        {
            git(root, &args).await.unwrap();
        }
        let a = path_for(root, "A", Uuid::new_v4());
        ensure(root, &a).await.unwrap();
        std::fs::write(a.join("feature.txt"), "from A\n").unwrap();
        let merged = merge(root, &a, "A: feature").await.unwrap();
        assert!(merged.starts_with("merged"), "{merged}");
        assert!(root.join("feature.txt").exists());
        assert!(merge(root, &a, "again").await.unwrap().contains("nothing new"));

        let b = path_for(root, "B", Uuid::new_v4());
        ensure(root, &b).await.unwrap();
        std::fs::write(b.join("shared.txt"), "from B\n").unwrap();
        std::fs::write(root.join("shared.txt"), "from root\n").unwrap();
        for args in
            [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "root edit"]]
        {
            git(root, &args).await.unwrap();
        }
        let error = merge(root, &b, "B: shared").await.unwrap_err();
        assert!(error.contains("shared.txt"), "{error}");
        assert!(git(root, &["status", "--porcelain"]).await.unwrap().is_empty(), "merge was not aborted");
    }

    #[tokio::test]
    async fn refuses_folders_that_are_not_repositories() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = path_for(dir.path(), "x", Uuid::new_v4());
        assert!(ensure(dir.path(), &worktree).await.is_err());
    }
}
