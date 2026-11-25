//! New projects from the page: start an empty git repo, or clone one, as a folder in
//! the machine's repos root, where the next scan finds it.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sidefleet_core::{valid_repo_folder, NewRepoJob, NewRepoSource, RepoInfo};
use tokio::process::Command;

/// A clone that takes longer than this is given up on.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// What making the project came to: the repo, or why not, and a warning for a repo
/// made with something short of it.
#[derive(Debug, Default)]
pub struct Outcome {
    pub repo: Option<RepoInfo>,
    pub error: Option<String>,
    pub warning: Option<String>,
}

impl Outcome {
    fn failed(msg: impl Into<String>) -> Self {
        Outcome { error: Some(msg.into()), ..Default::default() }
    }
}

pub async fn create(root: &Path, job: &NewRepoJob) -> Outcome {
    create_with(root, job, &[]).await
}

/// [`create`], with extra environment for git (tests set an identity this way).
async fn create_with(root: &Path, job: &NewRepoJob, env: &[(&str, &str)]) -> Outcome {
    if !valid_repo_folder(&job.name) {
        return Outcome::failed(format!("“{}” can't be a folder name: use letters, digits, - _ and .", job.name));
    }
    let dir = root.join(&job.name);
    if dir.exists() {
        return Outcome::failed(format!("{} already exists on this machine", dir.display()));
    }
    let warning = match &job.source {
        NewRepoSource::Init => match init(&dir, &job.name, env).await {
            Ok(w) => w,
            Err(e) => {
                // Leave nothing half made behind, unless git got as far as a repo.
                if !dir.join(".git").exists() {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                return Outcome::failed(e);
            }
        },
        NewRepoSource::Clone { url } => match clone(url, &dir).await {
            Ok(()) => None,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Outcome::failed(e);
            }
        },
    };
    Outcome { repo: Some(crate::detect::repo_info(root, &dir)), error: None, warning }
}

/// `git init` on the user's default branch (else `main`), a README and a first commit.
/// A commit git refuses (no name or email set) leaves the repo, with a warning.
async fn init(dir: &Path, name: &str, env: &[(&str, &str)]) -> Result<Option<String>, String> {
    std::fs::create_dir(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let branch = git(dir, &["config", "--get", "init.defaultBranch"]).await.ok().filter(|b| !b.is_empty()).unwrap_or_else(|| "main".into());
    git(dir, &["init", "--quiet", "-b", &branch]).await?;
    std::fs::write(dir.join("README.md"), format!("# {name}\n")).map_err(|e| format!("could not write the README: {e}"))?;
    git(dir, &["add", "README.md"]).await?;
    let mut commit = Command::new("git");
    commit.args(["commit", "--quiet", "-m", "Initial commit"]).current_dir(dir).envs(env.iter().copied());
    Ok(output(commit).await.err().map(|e| {
        if e.contains("user.email") || e.contains("user.name") || e.contains("identity") {
            "Made without a first commit: git has no name or email on this machine. Set user.name and user.email there.".into()
        } else {
            format!("Made without a first commit: {e}")
        }
    }))
}

async fn clone(url: &str, dir: &Path) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') {
        return Err("that isn't a git URL".into());
    }
    let mut cmd = Command::new("git");
    cmd.args(["clone", "--quiet", "--", url]).arg(dir);
    // Nobody is at this terminal to type a password or accept a host key.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let out = tokio::time::timeout(CLONE_TIMEOUT, output(cmd)).await.map_err(|_| "the clone took over 10 minutes; gave up".to_string())??;
    Ok(out).map(|_| ())
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    output(cmd).await
}

/// Stdout, or the last lines of stderr as the error.
async fn output(mut cmd: Command) -> Result<String, String> {
    let out = cmd.stdin(Stdio::null()).kill_on_drop(true).output().await.map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = err.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(3)..].join(" ");
    Err(sidefleet_core::redact(if tail.is_empty() { "git failed" } else { &tail }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn job(name: &str, source: NewRepoSource) -> NewRepoJob {
        NewRepoJob { id: Uuid::new_v4(), name: name.into(), source }
    }

    async fn sh(dir: &Path, args: &[&str]) {
        assert!(Command::new("git").args(args).current_dir(dir).status().await.unwrap().success(), "git {args:?}");
    }

    #[tokio::test]
    async fn init_makes_a_repo_with_a_first_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let who = [("GIT_AUTHOR_NAME", "T"), ("GIT_AUTHOR_EMAIL", "t@example.com"), ("GIT_COMMITTER_NAME", "T"), ("GIT_COMMITTER_EMAIL", "t@example.com")];
        let out = create_with(root, &job("my-app", NewRepoSource::Init), &who).await;
        assert_eq!(out.error, None);
        assert_eq!(out.warning, None);
        let repo = out.repo.unwrap();
        assert_eq!((repo.name.as_str(), repo.key()), ("my-app", "my-app"));
        assert_eq!(std::fs::read_to_string(root.join("my-app/README.md")).unwrap(), "# my-app\n");
        assert_eq!(git(&root.join("my-app"), &["log", "--format=%s"]).await.unwrap(), "Initial commit");
        // The same name again is refused, and the repo left alone.
        let again = create(root, &job("my-app", NewRepoSource::Init)).await;
        assert!(again.error.unwrap().contains("already exists"));
    }

    #[tokio::test]
    async fn clone_takes_the_remote_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("acme").join("web-app.git");
        std::fs::create_dir_all(&origin).unwrap();
        sh(&origin, &["init", "--quiet", "--bare", "-b", "main"]).await;
        let root = tmp.path().join("code");
        std::fs::create_dir(&root).unwrap();
        let url = format!("file://{}", origin.display());
        let out = create(&root, &job("web-app", NewRepoSource::Clone { url })).await;
        assert_eq!(out.error, None);
        let repo = out.repo.unwrap();
        assert_eq!(repo.name, "web-app");
        assert!(root.join("web-app/.git").exists());
    }

    #[tokio::test]
    async fn a_failed_clone_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let url = format!("file://{}", tmp.path().join("missing.git").display());
        let out = create(tmp.path(), &job("missing", NewRepoSource::Clone { url })).await;
        assert!(out.error.is_some());
        assert!(!tmp.path().join("missing").exists());
    }

    #[tokio::test]
    async fn bad_names_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["../escape", ".git", "a/b"] {
            assert!(create(tmp.path(), &job(name, NewRepoSource::Init)).await.error.is_some(), "{name}");
        }
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}
