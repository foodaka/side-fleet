//! Failover checkpoints. For a run that may move to another machine if this one
//! drops, commit the worktree to the run's `sidefleet/...` branch and push that
//! branch, and only that branch, to `origin` after the agent changed something,
//! at most once a minute. Each push is reported as a `checkpoint` event; the
//! control plane hands the newest one to the run that replaces a lost run.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use sidefleet_core::NewEvent;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use uuid::Uuid;

use crate::client::ControlClient;

/// Tools whose result may leave changed files behind (names as normalised for every agent).
const CHANGING_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit", "Bash"];

pub const MIN_INTERVAL: Duration = Duration::from_secs(60);
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);

/// Only branches the worker made for a task are ever fetched or pushed.
pub fn is_task_branch(name: &str) -> bool {
    name.strip_prefix("sidefleet/").is_some_and(|rest| {
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/')) && !rest.contains("//")
    })
}

/// Spots a `tool_result` that answers a file-changing `tool_use`.
#[derive(Default)]
pub struct ChangeTracker {
    pending: HashSet<String>,
}

impl ChangeTracker {
    pub fn observe(&mut self, ev: &NewEvent) -> bool {
        match ev.kind.as_str() {
            "tool_use" => {
                let name = ev.payload["name"].as_str().unwrap_or("");
                if CHANGING_TOOLS.contains(&name) {
                    self.pending.insert(ev.payload["id"].to_string());
                }
                false
            }
            "tool_result" => self.pending.remove(&ev.payload["tool_use_id"].to_string()),
            _ => false,
        }
    }
}

/// Runs until `changes` closes (the run ended). Each message on `changes` means
/// the worktree may have changed; nothing is pushed once `stopped` is set (the
/// run was stopped, or the control plane moved it elsewhere).
pub async fn run(
    client: ControlClient,
    run_id: Uuid,
    worktree: PathBuf,
    branch: String,
    mut changes: mpsc::UnboundedReceiver<()>,
    stopped: watch::Receiver<bool>,
    min_interval: Duration,
) {
    let mut last_at: Option<Instant> = None;
    let mut last_sha: Option<String> = None;
    while changes.recv().await.is_some() {
        if let Some(at) = last_at {
            let deadline = at + min_interval;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    m = changes.recv() => if m.is_none() { return },
                }
            }
        }
        while changes.try_recv().is_ok() {}
        if *stopped.borrow() {
            return;
        }
        last_at = Some(Instant::now());
        let payload = match checkpoint(&worktree, &branch, last_sha.as_deref()).await {
            Ok(None) => continue,
            Ok(Some(sha)) => {
                tracing::info!(run_id = %run_id, branch = %branch, sha = %sha, "pushed a checkpoint");
                last_sha = Some(sha.clone());
                json!({"branch": branch, "sha": sha})
            }
            Err(e) => {
                tracing::warn!(run_id = %run_id, error = %e, "checkpoint failed");
                json!({"branch": branch, "error": sidefleet_core::redact::redact(&format!("{e:#}"))})
            }
        };
        if let Err(e) = client.post_events(run_id, vec![NewEvent { kind: "checkpoint".into(), payload }]).await {
            tracing::warn!(error = %e, "posting checkpoint event failed");
        }
    }
}

/// Commit everything in the worktree and push its branch to `origin`. Returns
/// the pushed sha, or None when there was nothing new since `last_sha`.
pub async fn checkpoint(worktree: &Path, branch: &str, last_sha: Option<&str>) -> Result<Option<String>> {
    commit_and_push(worktree, branch, "sidefleet checkpoint", last_sha).await
}

/// Commit everything in the worktree with `message` and push its branch to `origin`.
/// Returns the pushed sha, or None when there was nothing new since `last_sha`.
pub async fn commit_and_push(worktree: &Path, branch: &str, message: &str, last_sha: Option<&str>) -> Result<Option<String>> {
    if !is_task_branch(branch) {
        anyhow::bail!("{branch} is not a sidefleet task branch; not pushing it");
    }
    let head = git(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"], None).await.unwrap_or_default();
    if head != branch {
        anyhow::bail!("the worktree is on {} instead of {branch}; not pushing", if head.is_empty() { "a detached HEAD" } else { &head });
    }
    if !git(worktree, &["status", "--porcelain"], None).await?.is_empty() {
        git(worktree, &["add", "-A"], None).await?;
        // Keep the user's identity when there is one; commits need some identity.
        let has_identity = git(worktree, &["config", "user.email"], None).await.is_ok_and(|e| !e.is_empty());
        let mut args = vec![];
        if !has_identity {
            args.extend(["-c", "user.name=Sidefleet", "-c", "user.email=sidefleet@localhost"]);
        }
        args.extend(["commit", "--no-verify", "--quiet", "-m", message]);
        git(worktree, &args, None).await?;
    }
    let sha = git(worktree, &["rev-parse", "HEAD"], None).await?;
    if last_sha == Some(sha.as_str()) {
        return Ok(None);
    }
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    git(worktree, &["push", "--no-verify", "--quiet", "origin", &refspec], Some(PUSH_TIMEOUT)).await?;
    Ok(Some(sha))
}

/// Fetch a task branch from `origin` into `refs/remotes/origin/<branch>` and return that ref.
pub async fn fetch_branch(repo: &Path, branch: &str) -> Result<String> {
    if !is_task_branch(branch) {
        anyhow::bail!("{branch} is not a sidefleet task branch");
    }
    let remote_ref = format!("refs/remotes/origin/{branch}");
    git(repo, &["fetch", "--quiet", "--no-tags", "origin", &format!("+refs/heads/{branch}:{remote_ref}")], Some(PUSH_TIMEOUT)).await?;
    Ok(remote_ref)
}

/// `git -C dir args`, never prompting for credentials.
pub(crate) async fn git(dir: &Path, args: &[&str], timeout: Option<Duration>) -> Result<String> {
    let cmd = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = match timeout {
        Some(t) => tokio::time::timeout(t, cmd).await.map_err(|_| anyhow::anyhow!("git {} timed out", args.join(" ")))?,
        None => cmd.await,
    }
    .context("running git")?;
    if !out.status.success() {
        anyhow::bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn ev(kind: &str, payload: Value) -> NewEvent {
        NewEvent { kind: kind.into(), payload }
    }

    #[test]
    fn only_results_of_changing_tools_count() {
        let mut t = ChangeTracker::default();
        assert!(!t.observe(&ev("tool_use", json!({"id": "a", "name": "Read"}))));
        assert!(!t.observe(&ev("tool_result", json!({"tool_use_id": "a"}))));
        assert!(!t.observe(&ev("tool_use", json!({"id": "b", "name": "Edit"}))));
        assert!(!t.observe(&ev("assistant_text", json!({"text": "hi"}))));
        assert!(t.observe(&ev("tool_result", json!({"tool_use_id": "b"}))));
        assert!(!t.observe(&ev("tool_result", json!({"tool_use_id": "b"}))));
        assert!(!t.observe(&ev("tool_use", json!({"id": "c", "name": "Bash"}))));
        assert!(t.observe(&ev("tool_result", json!({"tool_use_id": "c"}))));
    }

    #[test]
    fn only_task_branches_are_pushed_or_fetched() {
        assert!(is_task_branch("sidefleet/0123abcd"));
        assert!(!is_task_branch("main"));
        assert!(!is_task_branch("sidefleet/"));
        assert!(!is_task_branch("sidefleet/a b"));
        assert!(!is_task_branch("sidefleet/a..b"));
        assert!(!is_task_branch("--upload-pack=sidefleet/x"));
    }

    async fn sh(dir: &Path, args: &[&str]) -> String {
        git(dir, args, None).await.unwrap()
    }

    /// Commits the worktree and pushes only its branch; nothing new means no push.
    #[tokio::test]
    async fn checkpoints_commit_and_push_the_task_branch_only() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh(tmp.path(), &["init", "--quiet", "--bare", origin.to_str().unwrap()]).await;
        sh(&repo, &["init", "--quiet", "-b", "main"]).await;
        sh(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--allow-empty", "-m", "init"]).await;
        sh(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]).await;
        let wt = tmp.path().join("wt");
        sh(&repo, &["worktree", "add", "--quiet", "-b", "sidefleet/abcd1234", wt.to_str().unwrap(), "HEAD"]).await;
        std::fs::write(wt.join("work.txt"), "half done").unwrap();
        // A side branch that must never reach origin.
        sh(&repo, &["branch", "private"]).await;

        let sha = checkpoint(&wt, "sidefleet/abcd1234", None).await.unwrap().expect("pushed");
        assert_eq!(sh(&origin, &["rev-parse", "refs/heads/sidefleet/abcd1234"]).await, sha);
        assert_eq!(sh(&origin, &["for-each-ref", "--format=%(refname)"]).await, "refs/heads/sidefleet/abcd1234");
        assert_eq!(sh(&wt, &["log", "-1", "--format=%s"]).await, "sidefleet checkpoint");
        assert_eq!(checkpoint(&wt, "sidefleet/abcd1234", Some(&sha)).await.unwrap(), None);

        // Another machine picks it up from origin.
        let other = tmp.path().join("other");
        sh(tmp.path(), &["clone", "--quiet", origin.to_str().unwrap(), other.to_str().unwrap()]).await;
        let r = fetch_branch(&other, "sidefleet/abcd1234").await.unwrap();
        assert_eq!(sh(&other, &["rev-parse", &r]).await, sha);

        // A worktree switched to another branch is not pushed.
        sh(&wt, &["checkout", "--quiet", "-b", "sidefleet-elsewhere"]).await;
        assert!(checkpoint(&wt, "sidefleet/abcd1234", Some(&sha)).await.is_err());
    }
}
