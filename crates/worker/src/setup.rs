//! Getting a task's worktree ready to work in. A worktree starts without what git
//! ignores (`node_modules`, `.env`), so a repo can list files to bring along and a
//! command to run in `.sidefleet/worktree.json`:
//!
//! ```json
//! { "copy": [".env.local"], "setup": "pnpm install --frozen-lockfile" }
//! ```
//!
//! The file is read from the checkout, never from the task's worktree, so what a
//! task's branch says can't change what this machine runs. Setup happens once per
//! worktree and a failure is reported without stopping the task: the agent can often
//! finish the job itself.

use std::path::{Component, Path};
use std::process::Stdio;
use std::time::Duration;

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde::Deserialize;
use tokio::process::Command;

use crate::checkpoint::git;

pub const CONFIG: &str = ".sidefleet/worktree.json";
/// Kept in the worktree's own git folder, so it goes when the worktree does.
const MARKER: &str = "sidefleet-setup-done";
const SETUP_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const FAILURE_LINES: usize = 15;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    copy: Vec<String>,
    #[serde(default)]
    setup: Option<String>,
}

/// What a worktree still needs.
#[derive(Debug)]
pub struct Plan {
    config: Config,
    marker: std::path::PathBuf,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub copied: Vec<String>,
    pub ran: Option<String>,
    pub problems: Vec<String>,
}

/// The setup this worktree is due, if any. An error is a config that can't be read;
/// nothing is recorded for it, so fixing the file takes effect on the next message.
pub async fn plan(repo: &Path, worktree: &Path) -> Result<Option<Plan>, String> {
    let raw = match std::fs::read_to_string(repo.join(CONFIG)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not read {CONFIG}: {e}")),
    };
    let config: Config = serde_json::from_str(&raw).map_err(|e| format!("could not read {CONFIG}: {e}"))?;
    if config.copy.is_empty() && config.setup.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return Ok(None);
    }
    let git_dir = git(worktree, &["rev-parse", "--absolute-git-dir"], None).await.map_err(|e| format!("{e:#}"))?;
    let marker = Path::new(&git_dir).join(MARKER);
    Ok((!marker.exists()).then_some(Plan { config, marker }))
}

/// Copy what the plan lists from the checkout, then run its command in the worktree.
/// Dropping this future (the task was stopped) ends the command with it.
pub async fn run(repo: &Path, worktree: &Path, plan: Plan) -> Outcome {
    let mut outcome = Outcome::default();
    for entry in &plan.config.copy {
        match copy_ignored(repo, worktree, entry).await {
            Ok(true) => outcome.copied.push(entry.clone()),
            Ok(false) => {}
            Err(e) => outcome.problems.push(format!("did not copy {entry}: {e}")),
        }
    }
    if let Some(script) = plan.config.setup.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        match run_script(worktree, script).await {
            Ok(()) => outcome.ran = Some(script.to_string()),
            Err(e) => outcome.problems.push(format!("`{script}` failed: {e}")),
        }
    }
    if let Err(e) = std::fs::write(&plan.marker, "") {
        tracing::warn!(error = %e, "could not record that the worktree is set up");
    }
    outcome
}

/// Bring one entry across unless it is missing or already there (`Ok(false)`). Only
/// what git ignores in the worktree, so a checkpoint or pull request never picks it up.
async fn copy_ignored(repo: &Path, worktree: &Path, entry: &str) -> Result<bool, String> {
    let rel = entry.trim_end_matches('/');
    if rel.is_empty() || !Path::new(rel).components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("it must be a path inside the repo".into());
    }
    let (src, dst) = (repo.join(rel), worktree.join(rel));
    let Ok(meta) = std::fs::symlink_metadata(&src) else {
        return Ok(false);
    };
    if std::fs::symlink_metadata(&dst).is_ok() {
        return Ok(false);
    }
    // A pattern like `node_modules/` only matches when git is told it is a folder.
    let probe = if meta.is_dir() { format!("{rel}/") } else { rel.to_string() };
    if git(worktree, &["check-ignore", "--quiet", "--", &probe], None).await.is_err() {
        return Err("git does not ignore it, so a checkpoint could commit it".into());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Share the data on a filesystem that can (APFS, btrfs, XFS); some `cp`s lack the flag.
    let clone: &str = if cfg!(target_os = "macos") { "-c" } else { "--reflink=auto" };
    let mut last = String::new();
    for flags in [&["-a", clone][..], &["-a"][..]] {
        let out = Command::new("cp").args(flags).arg("--").arg(&src).arg(&dst).stdin(Stdio::null()).kill_on_drop(true).output().await.map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(true);
        }
        last = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if dst.is_dir() {
            let _ = std::fs::remove_dir_all(&dst);
        } else {
            let _ = std::fs::remove_file(&dst);
        }
    }
    Err(last)
}

/// Ends the script's whole process group, unless it finished by itself.
struct KillGroup(i32);

impl Drop for KillGroup {
    fn drop(&mut self) {
        let _ = killpg(Pid::from_raw(self.0), Signal::SIGKILL);
    }
}

async fn run_script(worktree: &Path, script: &str) -> Result<(), String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script).current_dir(worktree).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).kill_on_drop(true);
    if let Some(path) = crate::detect::path_with_user_bins() {
        cmd.env("PATH", path);
    }
    for k in crate::detect::STRIPPED_ENV {
        cmd.env_remove(k);
    }
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    let group = KillGroup(child.id().unwrap_or(0) as i32);
    let out = match tokio::time::timeout(SETUP_TIMEOUT, child.wait_with_output()).await {
        Err(_) => return Err(format!("still running after {} minutes", SETUP_TIMEOUT.as_secs() / 60)),
        Ok(r) => r.map_err(|e| e.to_string())?,
    };
    std::mem::forget(group);
    if out.status.success() {
        return Ok(());
    }
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    let lines: Vec<&str> = text.trim().lines().collect();
    Err(sidefleet_core::redact::redact(&lines[lines.len().saturating_sub(FAILURE_LINES)..].join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repo ignoring `.env` and `deps/`, and a worktree of it.
    async fn repo(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let (repo, wt) = (root.join("repo"), root.join("wt"));
        std::fs::create_dir_all(repo.join(".sidefleet")).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"], None).await.unwrap();
        std::fs::write(repo.join(".gitignore"), ".env\ndeps/\n").unwrap();
        std::fs::write(repo.join("tracked.txt"), "t").unwrap();
        git(&repo, &["add", ".gitignore", "tracked.txt"], None).await.unwrap();
        git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "init"], None).await.unwrap();
        git(&repo, &["worktree", "add", "--quiet", "-b", "sidefleet/t", wt.to_str().unwrap()], None).await.unwrap();
        (repo, wt)
    }

    fn config(repo: &Path, json: &str) {
        std::fs::write(repo.join(CONFIG), json).unwrap();
    }

    #[tokio::test]
    async fn copies_ignored_files_and_runs_the_command_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo(tmp.path()).await;
        std::fs::write(repo.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(repo.join("deps/pkg")).unwrap();
        std::fs::write(repo.join("deps/pkg/index.js"), "x").unwrap();
        config(&repo, r#"{"copy": [".env", "deps/", "not-there"], "setup": "echo ran >> setup.log"}"#);

        let out = run(&repo, &wt, plan(&repo, &wt).await.unwrap().unwrap()).await;
        assert_eq!(out, Outcome { copied: vec![".env".into(), "deps/".into()], ran: Some("echo ran >> setup.log".into()), problems: vec![] });
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "SECRET=1");
        assert!(wt.join("deps/pkg/index.js").is_file());
        assert!(plan(&repo, &wt).await.unwrap().is_none(), "a worktree is set up once");
    }

    #[tokio::test]
    async fn refuses_what_git_would_commit_or_what_is_outside_the_repo_and_keeps_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo(tmp.path()).await;
        std::fs::write(repo.join("untracked.txt"), "u").unwrap();
        std::fs::write(tmp.path().join("outside"), "o").unwrap();
        std::fs::write(repo.join(".env"), "from checkout").unwrap();
        std::fs::write(wt.join(".env"), "the agent's").unwrap();
        config(&repo, r#"{"copy": ["untracked.txt", "../outside", "/etc/hostname", ".env"]}"#);

        let out = run(&repo, &wt, plan(&repo, &wt).await.unwrap().unwrap()).await;
        assert!(out.copied.is_empty(), "{out:?}");
        assert_eq!(out.problems.len(), 3, "{out:?}");
        assert!(!wt.join("untracked.txt").exists());
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "the agent's");
    }

    #[tokio::test]
    async fn a_failed_command_is_reported_and_not_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo(tmp.path()).await;
        config(&repo, r#"{"setup": "echo no lockfile >&2; exit 3"}"#);
        let out = run(&repo, &wt, plan(&repo, &wt).await.unwrap().unwrap()).await;
        assert_eq!(out.ran, None);
        assert!(out.problems[0].contains("no lockfile"), "{out:?}");
        assert!(plan(&repo, &wt).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_task_branch_cannot_supply_its_own_config_and_a_bad_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo(tmp.path()).await;
        std::fs::create_dir_all(wt.join(".sidefleet")).unwrap();
        config(&wt, r#"{"setup": "touch pwned"}"#);
        assert!(plan(&repo, &wt).await.unwrap().is_none());
        config(&repo, r#"{"setup": "#);
        assert!(plan(&repo, &wt).await.unwrap_err().contains(CONFIG));
    }
}
