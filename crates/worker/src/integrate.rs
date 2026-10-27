//! Rooms. An `integrate` run merges a task thread's branch into its room's shared
//! branch on `origin` (`sidefleet/room-...`), so later tasks in the room start from
//! everyone's finished work. The merge happens in a throwaway worktree, never in
//! the task's own; a conflict aborts it and leaves the room branch as it was, and
//! nothing is ever force-pushed.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::checkpoint::{fetch_branch, git, is_task_branch};

const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Tries when another task's integration lands on the room branch first.
const ATTEMPTS: usize = 3;

/// The room branch after a successful integration.
#[derive(Debug, Clone, PartialEq)]
pub struct Integrated {
    pub branch: String,
    pub commit: String,
    /// Files the merge changed on the room branch (or against the default branch, for a new room branch).
    pub files: Vec<String>,
}

/// Why an integration failed; `conflicts` lists the files when the merge conflicted.
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub conflicts: Vec<String>,
}

impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Self {
        Failure { message: format!("{e:#}"), conflicts: Vec::new() }
    }
}

/// Merge `task_branch` (checked out in `worktree`) into `room_branch` on `origin`.
/// Anything uncommitted in the worktree is committed to the task branch first.
/// `title` names the merge commit ("Room: <title>").
pub async fn integrate(worktree: &Path, task_branch: &str, room_branch: &str, title: &str) -> Result<Integrated, Failure> {
    if !is_task_branch(task_branch) {
        return Err(anyhow::anyhow!("{task_branch} is not a sidefleet task branch").into());
    }
    if !is_room_branch(room_branch) {
        return Err(anyhow::anyhow!("{room_branch} is not a sidefleet room branch; not pushing to it").into());
    }
    commit_leftovers(worktree, task_branch).await?;
    let first = title.lines().next().unwrap_or("").trim();
    let message = format!("Room: {}", first.strip_prefix("Add to the room: ").unwrap_or(first));
    for attempt in 1..=ATTEMPTS {
        match try_once(worktree, task_branch, room_branch, &message).await {
            Ok(Attempt::Done(i)) => return Ok(i),
            Ok(Attempt::Rejected) if attempt < ATTEMPTS => {
                tracing::info!(room = %room_branch, attempt, "room branch moved while integrating; trying again");
            }
            Ok(Attempt::Rejected) => {
                return Err(anyhow::anyhow!("the room branch kept moving while integrating; try again").into());
            }
            Err(f) => return Err(f),
        }
    }
    unreachable!("the loop returns on its last attempt")
}

enum Attempt {
    Done(Integrated),
    /// The push lost a race with another integration.
    Rejected,
}

async fn try_once(worktree: &Path, task_branch: &str, room_branch: &str, message: &str) -> Result<Attempt, Failure> {
    git(worktree, &["fetch", "--quiet", "--no-tags", "origin"], Some(FETCH_TIMEOUT)).await?;
    let room_exists = !git(worktree, &["ls-remote", "--heads", "origin", &format!("refs/heads/{room_branch}")], Some(FETCH_TIMEOUT)).await?.is_empty();
    let start_ref = if room_exists { fetch_branch(worktree, room_branch).await? } else { default_branch(worktree).await? };
    let start = git(worktree, &["rev-parse", &format!("{start_ref}^{{commit}}")], None).await?;

    let tmp = TempWorktree::add(worktree, worktree, &start).await?;
    let result = merge_and_push(&tmp.path, task_branch, room_branch, message, &start).await;
    tmp.remove().await;
    result
}

async fn merge_and_push(dir: &Path, task_branch: &str, room_branch: &str, message: &str, start: &str) -> Result<Attempt, Failure> {
    let mut args = identity(dir).await;
    // Hooks off: the room branch carries teammates' commits, and a repo whose hooks live in tracked
    // files (a relative core.hooksPath) would otherwise run their scripts here with nobody asking.
    args.extend(["-c", NO_HOOKS, "merge", "--no-ff", "--no-edit", "--no-verify", "--quiet", "-m", message, task_branch]);
    if let Err(e) = git(dir, &args, None).await {
        let conflicts: Vec<String> = git(dir, &["diff", "--name-only", "--diff-filter=U"], None)
            .await
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .filter(|l| !l.is_empty())
            .collect();
        let _ = git(dir, &["merge", "--abort"], None).await;
        if conflicts.is_empty() {
            return Err(e.context("merging into the room branch").into());
        }
        let n = conflicts.len();
        return Err(Failure {
            message: format!("conflicts with the room branch in {n} file{}: {}", if n == 1 { "" } else { "s" }, conflicts.join(", ")),
            conflicts,
        });
    }
    let commit = git(dir, &["rev-parse", "HEAD"], None).await?;
    let files: Vec<String> = git(dir, &["diff", "--name-only", start, &commit], None).await?.lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    let refspec = format!("HEAD:refs/heads/{room_branch}");
    if let Err(e) = git(dir, &["push", "--no-verify", "--quiet", "origin", &refspec], Some(FETCH_TIMEOUT)).await {
        let msg = format!("{e:#}");
        if ["rejected", "non-fast-forward", "fetch first", "stale info"].iter().any(|s| msg.contains(s)) {
            return Ok(Attempt::Rejected);
        }
        return Err(e.context("pushing the room branch").into());
    }
    Ok(Attempt::Done(Integrated { branch: room_branch.to_string(), commit, files }))
}

/// Commit what the agent left uncommitted on the task branch (not pushed).
async fn commit_leftovers(worktree: &Path, task_branch: &str) -> Result<()> {
    let head = git(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"], None).await.unwrap_or_default();
    if head != task_branch {
        anyhow::bail!("the worktree is on {} instead of {task_branch}", if head.is_empty() { "a detached HEAD" } else { &head });
    }
    if !git(worktree, &["status", "--porcelain"], None).await?.is_empty() {
        git(worktree, &["add", "-A"], None).await?;
        let mut args = identity(worktree).await;
        args.extend(["-c", NO_HOOKS, "commit", "--no-verify", "--quiet", "-m", "sidefleet: changes for the room"]);
        git(worktree, &args, None).await?;
    }
    Ok(())
}

/// Commits need some identity: the user's when git has one.
async fn identity(dir: &Path) -> Vec<&'static str> {
    let has = git(dir, &["config", "user.email"], None).await.is_ok_and(|e| !e.is_empty());
    if has {
        vec![]
    } else {
        vec!["-c", "user.name=Sidefleet", "-c", "user.email=sidefleet@localhost"]
    }
}

/// `origin`'s default branch as a remote-tracking ref, e.g. `refs/remotes/origin/main`.
async fn default_branch(dir: &Path) -> Result<String> {
    if let Ok(r) = git(dir, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"], None).await {
        if !r.is_empty() {
            return Ok(r);
        }
    }
    for b in ["main", "master"] {
        let r = format!("refs/remotes/origin/{b}");
        if git(dir, &["rev-parse", "--verify", "--quiet", &r], None).await.is_ok() {
            return Ok(r);
        }
    }
    anyhow::bail!("can't tell origin's default branch; run `git remote set-head origin --auto` in the repo")
}

/// Room branches are named by the control plane after the room: `sidefleet/room-<8 hex>`. Anything
/// else (another task's branch, a hand-made `sidefleet/...` one) is never merged into or pushed.
pub fn is_room_branch(name: &str) -> bool {
    name.strip_prefix("sidefleet/room-").is_some_and(|id| id.len() == 8 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Points git's hooks at nothing, so no hook script runs during the merge.
const NO_HOOKS: &str = "core.hooksPath=/dev/null";

/// A detached worktree for the merge, removed afterwards whatever happened.
struct TempWorktree {
    repo: PathBuf,
    path: PathBuf,
}

impl TempWorktree {
    /// Next to the task's worktree, in a directory only this user can read (not the shared temp dir).
    async fn add(repo: &Path, beside: &Path, start: &str) -> Result<TempWorktree> {
        let dir = beside.parent().unwrap_or(beside).join(".sidefleet-integrate");
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).context("creating the merge directory")?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).context("restricting the merge directory")?;
        let path = dir.join(uuid::Uuid::new_v4().simple().to_string());
        let p = path.to_str().context("temporary path is not UTF-8")?;
        git(repo, &["-c", NO_HOOKS, "worktree", "add", "--quiet", "--detach", p, start], None).await.context("creating a worktree to merge in")?;
        Ok(TempWorktree { repo: repo.to_path_buf(), path })
    }

    async fn remove(self) {
        let p = self.path.to_string_lossy().to_string();
        if let Err(e) = git(&self.repo, &["worktree", "remove", "--force", &p], None).await {
            tracing::debug!(error = %format!("{e:#}"), "removing the merge worktree");
        }
        let _ = std::fs::remove_dir_all(&self.path);
        let _ = git(&self.repo, &["worktree", "prune"], None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_room_branches_are_merged_into() {
        assert!(is_room_branch("sidefleet/room-1a2b3c4d"));
        for bad in ["sidefleet/1a2b3c4d", "sidefleet/room-1A2B3C4D", "sidefleet/room-1a2b3c4", "sidefleet/room-1a2b3c4d5", "sidefleet/room-1a2b3c4d/x", "sidefleet/release", "main"] {
            assert!(!is_room_branch(bad), "{bad}");
        }
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        origin: PathBuf,
        repo: PathBuf,
        root: PathBuf,
    }

    async fn g(dir: &Path, args: &[&str]) -> String {
        git(dir, args, None).await.unwrap()
    }

    /// A bare origin with `main` (one file), and a clone whose origin/HEAD points at main.
    async fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, repo) = (tmp.path().join("origin.git"), tmp.path().join("repo"));
        std::fs::create_dir_all(&repo).unwrap();
        g(tmp.path(), &["init", "--quiet", "--bare", "-b", "main", origin.to_str().unwrap()]).await;
        g(&repo, &["init", "--quiet", "-b", "main"]).await;
        std::fs::write(repo.join("shared.txt"), "base\n").unwrap();
        g(&repo, &["add", "-A"]).await;
        g(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "init"]).await;
        g(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]).await;
        g(&repo, &["push", "--quiet", "origin", "main"]).await;
        g(&repo, &["remote", "set-head", "origin", "main"]).await;
        let root = tmp.path().to_path_buf();
        Fixture { _tmp: tmp, origin, repo, root }
    }

    /// A task worktree on `branch` from main, with `file` written (left uncommitted).
    async fn task(f: &Fixture, branch: &str, file: &str, body: &str) -> PathBuf {
        let wt = f.root.join(branch.replace('/', "-"));
        g(&f.repo, &["worktree", "add", "--quiet", "-b", branch, wt.to_str().unwrap(), "main"]).await;
        std::fs::write(wt.join(file), body).unwrap();
        wt
    }

    fn leftover_merge_worktrees(repo: &Path) -> usize {
        let out = std::process::Command::new("git").arg("-C").arg(repo).args(["worktree", "list"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("sidefleet-integrate")).count()
    }

    #[tokio::test]
    async fn first_integration_starts_the_room_branch_from_the_default_branch() {
        let f = fixture().await;
        let wt = task(&f, "sidefleet/aaaa1111", "a.txt", "a\n").await;
        let i = integrate(&wt, "sidefleet/aaaa1111", "sidefleet/room-12345678", "Add a\nmore text").await.unwrap();
        assert_eq!(i.branch, "sidefleet/room-12345678");
        assert_eq!(i.files, vec!["a.txt".to_string()]);
        assert_eq!(g(&f.origin, &["rev-parse", "refs/heads/sidefleet/room-12345678"]).await, i.commit);
        // The merge commit sits on top of main and names the task.
        assert_eq!(g(&f.origin, &["log", "-1", "--format=%s", "refs/heads/sidefleet/room-12345678"]).await, "Room: Add a");
        let main = g(&f.origin, &["rev-parse", "refs/heads/main"]).await;
        g(&f.origin, &["merge-base", "--is-ancestor", &main, &i.commit]).await;
        // The leftover change was committed on the task branch, which wasn't pushed.
        assert!(g(&wt, &["status", "--porcelain"]).await.is_empty());
        assert!(git(&f.origin, &["rev-parse", "--verify", "--quiet", "refs/heads/sidefleet/aaaa1111"], None).await.is_err());
        assert_eq!(leftover_merge_worktrees(&f.repo), 0);
    }

    #[tokio::test]
    async fn tasks_on_different_files_both_land_on_the_room_branch() {
        let f = fixture().await;
        let a = task(&f, "sidefleet/aaaa1111", "a.txt", "a\n").await;
        let b = task(&f, "sidefleet/bbbb2222", "b.txt", "b\n").await;
        let room = "sidefleet/room-12345678";
        integrate(&a, "sidefleet/aaaa1111", room, "Add a").await.unwrap();
        let second = integrate(&b, "sidefleet/bbbb2222", room, "Add b").await.unwrap();
        assert_eq!(second.files, vec!["b.txt".to_string()]);
        let tree = g(&f.origin, &["ls-tree", "--name-only", &format!("refs/heads/{room}")]).await;
        let names: Vec<&str> = tree.lines().collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "shared.txt"]);
        assert_eq!(leftover_merge_worktrees(&f.repo), 0);
    }

    #[tokio::test]
    async fn a_conflict_fails_and_leaves_the_room_branch_alone() {
        let f = fixture().await;
        let a = task(&f, "sidefleet/aaaa1111", "shared.txt", "from a\n").await;
        let b = task(&f, "sidefleet/bbbb2222", "shared.txt", "from b\n").await;
        let room = "sidefleet/room-12345678";
        let first = integrate(&a, "sidefleet/aaaa1111", room, "A").await.unwrap();
        let err = integrate(&b, "sidefleet/bbbb2222", room, "B").await.unwrap_err();
        assert_eq!(err.conflicts, vec!["shared.txt".to_string()]);
        assert!(err.message.contains("conflicts with the room branch in 1 file"), "{}", err.message);
        assert_eq!(g(&f.origin, &["rev-parse", &format!("refs/heads/{room}")]).await, first.commit);
        assert_eq!(leftover_merge_worktrees(&f.repo), 0);
        // The task's own worktree is untouched by the failed merge.
        assert!(g(&b, &["status", "--porcelain"]).await.is_empty());
        assert_eq!(std::fs::read_to_string(b.join("shared.txt")).unwrap(), "from b\n");
    }

    #[tokio::test]
    async fn hook_scripts_committed_to_the_repo_do_not_run_during_the_merge() {
        // A repo whose hooks live in tracked files (husky v4-v8 style): anyone who can push to the
        // room branch controls them, so the worker must not run them while it merges.
        let f = fixture().await;
        let marker = f.root.join("hook-ran");
        std::fs::create_dir_all(f.repo.join(".hooks")).unwrap();
        for hook in ["post-checkout", "post-merge", "pre-merge-commit", "prepare-commit-msg", "commit-msg"] {
            let path = f.repo.join(".hooks").join(hook);
            std::fs::write(&path, format!("#!/bin/sh\necho {hook} >> '{}'\n", marker.display())).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        g(&f.repo, &["add", "-A"]).await;
        g(&f.repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--no-verify", "-m", "hooks"]).await;
        g(&f.repo, &["push", "--quiet", "origin", "main"]).await;
        g(&f.repo, &["config", "core.hooksPath", ".hooks"]).await;
        let wt = task(&f, "sidefleet/aaaa1111", "a.txt", "a\n").await;
        let _ = std::fs::remove_file(&marker); // the task's own checkout above may run them; that isn't the merge
        integrate(&wt, "sidefleet/aaaa1111", "sidefleet/room-12345678", "Add a").await.unwrap();
        assert!(!marker.exists(), "a hook ran: {}", std::fs::read_to_string(&marker).unwrap_or_default());
    }

    #[tokio::test]
    async fn merge_worktrees_are_private_to_the_user() {
        let f = fixture().await;
        let wt = task(&f, "sidefleet/aaaa1111", "a.txt", "a\n").await;
        integrate(&wt, "sidefleet/aaaa1111", "sidefleet/room-12345678", "Add a").await.unwrap();
        let dir = wt.parent().unwrap().join(".sidefleet-integrate");
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[tokio::test]
    async fn refuses_branches_that_are_not_sidefleet_ones() {
        let f = fixture().await;
        let wt = task(&f, "sidefleet/aaaa1111", "a.txt", "a\n").await;
        let err = integrate(&wt, "sidefleet/aaaa1111", "main", "x").await.unwrap_err();
        assert!(err.message.contains("not a sidefleet room branch"), "{}", err.message);
        assert_eq!(g(&f.origin, &["rev-parse", "refs/heads/main"]).await, g(&f.repo, &["rev-parse", "main"]).await);
    }
}
