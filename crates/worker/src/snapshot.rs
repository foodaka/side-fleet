//! Snapshots to go back to. The trees `diff` takes before and after each turn
//! are kept as commits (parent: HEAD at the time) under
//! `refs/sidefleet/snapshots/<root>/<run>/{before,after}`, so git never prunes
//! them and nothing shows on the user's branches. A restore run puts a worktree
//! back to one; a fork starts its worktree from one.

use std::path::Path;

use anyhow::{Context, Result};
use uuid::Uuid;

pub fn ref_name(root_id: Uuid, run_id: Uuid, which: &str) -> String {
    format!("refs/sidefleet/snapshots/{root_id}/{run_id}/{which}")
}

/// Whether `s` looks like a commit id (a value from the control plane is checked before use).
pub fn is_commit_id(s: &str) -> bool {
    (40..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Keep `tree` (from `diff::snapshot`) as a commit on top of HEAD, under `refname`.
pub async fn keep(dir: &Path, tree: &str, refname: &str) -> Result<String> {
    let head = git(dir, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).await.ok();
    let mut args = vec!["commit-tree", tree, "-m", "sidefleet snapshot"];
    if let Some(h) = &head {
        args.extend(["-p", h.as_str()]);
    }
    let commit = git(dir, &args).await?;
    git(dir, &["update-ref", refname, &commit]).await?;
    Ok(commit)
}

/// Put the worktree at `dir` back to `snapshot`: HEAD returns to the commit it
/// was on, files the snapshot didn't have are removed (ignored files are left
/// alone) and uncommitted changes in the snapshot come back unstaged.
pub async fn restore(dir: &Path, snapshot: &str) -> Result<()> {
    let base = git(dir, &["rev-parse", "--verify", "--quiet", &format!("{snapshot}^1")]).await.ok();
    match &base {
        Some(b) => {
            git(dir, &["reset", "--hard", "--quiet", b]).await?;
        }
        None => {
            // A snapshot of a repo with no commits yet: just drop the index.
            let _ = git(dir, &["read-tree", "--empty"]).await;
        }
    }
    git(dir, &["clean", "-fd", "--quiet"]).await?;
    // Index and files to the snapshot (this also removes files it doesn't have),
    // then unstage so its uncommitted changes are plain working-tree edits again.
    git(dir, &["read-tree", "-u", "--reset", snapshot]).await?;
    if base.is_some() {
        git(dir, &["reset", "--quiet"]).await?;
    } else {
        git(dir, &["read-tree", "--empty"]).await?;
    }
    Ok(())
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args).stdin(std::process::Stdio::null());
    // commit-tree needs an identity even on machines without one configured.
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "Sidefleet"),
        ("GIT_AUTHOR_EMAIL", "sidefleet@localhost"),
        ("GIT_COMMITTER_NAME", "Sidefleet"),
        ("GIT_COMMITTER_EMAIL", "sidefleet@localhost"),
    ] {
        cmd.env(k, v);
    }
    let out = cmd.output().await.context("running git")?;
    if !out.status.success() {
        anyhow::bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff;
    use std::process::Command;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(dir).args(args)
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    async fn snap(dir: &Path, root: Uuid, run: Uuid, which: &str) -> String {
        let tree = diff::snapshot(dir).await.unwrap();
        keep(dir, &tree, &ref_name(root, run, which)).await.unwrap()
    }

    #[tokio::test]
    async fn kept_snapshots_restore_files_head_and_uncommitted_edits() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        sh(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        sh(dir, &["add", "."]);
        sh(dir, &["commit", "-q", "-m", "init"]);
        let (root, run) = (Uuid::new_v4(), Uuid::new_v4());
        // An uncommitted edit the user staged before the run.
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        sh(dir, &["add", "a.txt"]);
        let head = sh(dir, &["rev-parse", "HEAD"]);
        let before = snap(dir, root, run, "before").await;
        assert_eq!(sh(dir, &["rev-parse", &format!("{before}^1")]), head, "a snapshot's parent is HEAD at the time");
        assert_eq!(sh(dir, &["rev-parse", &ref_name(root, run, "before")]), before);
        assert_eq!(sh(dir, &["status", "--porcelain"]), "M  a.txt", "the user's index is untouched");

        // The agent commits a new file, deletes one and writes an ignored file.
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/out"), "x").unwrap();
        sh(dir, &["add", "b.txt"]);
        sh(dir, &["commit", "-q", "-m", "agent"]);
        std::fs::remove_file(dir.join("a.txt")).unwrap();
        let after = snap(dir, root, run, "after").await;

        // Back to before: HEAD back, b.txt gone, the staged edit back as a plain edit, ignored file kept.
        restore(dir, &before).await.unwrap();
        assert_eq!(sh(dir, &["rev-parse", "HEAD"]), head);
        assert!(!dir.join("b.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\ntwo\n");
        assert_eq!(sh(dir, &["status", "--porcelain"]), " M a.txt");
        assert!(dir.join("target/out").exists(), "ignored files are left alone");

        // And forward again to after.
        restore(dir, &after).await.unwrap();
        assert!(dir.join("b.txt").exists());
        assert!(!dir.join("a.txt").exists());
    }

    #[tokio::test]
    async fn works_in_a_repo_without_commits() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        sh(dir, &["init", "-q"]);
        let (root, run) = (Uuid::new_v4(), Uuid::new_v4());
        let before = snap(dir, root, run, "before").await;
        std::fs::write(dir.join("new.txt"), "hi\n").unwrap();
        snap(dir, root, run, "after").await;
        restore(dir, &before).await.unwrap();
        assert!(!dir.join("new.txt").exists());
    }

    #[test]
    fn commit_ids() {
        assert!(is_commit_id(&"a".repeat(40)) && is_commit_id(&"0".repeat(64)));
        assert!(!is_commit_id("HEAD") && !is_commit_id(&"g".repeat(40)) && !is_commit_id("--all"));
    }
}
