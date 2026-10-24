//! Pull requests. A `pr` run commits what is left in the thread's worktree,
//! pushes its `sidefleet/...` branch to `origin` and opens a pull request with
//! the GitHub CLI (`gh`), signed in as whoever runs this worker. Open pull
//! requests are looked at again now and then, so the page shows when one is
//! merged or closed.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use sidefleet_core::PullRequest;

use crate::checkpoint::{commit_and_push, git, is_task_branch};

const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// What `gh pr view --json number,url,state` prints.
#[derive(Deserialize)]
struct GhPr {
    number: i64,
    url: String,
    state: String,
}

impl From<GhPr> for PullRequest {
    fn from(p: GhPr) -> Self {
        PullRequest { url: p.url, number: p.number, state: p.state.to_ascii_lowercase() }
    }
}

/// Push `branch` from `worktree` and open a pull request for it, or reuse the one
/// already open for the branch. `context` is the title, a blank line, then the body.
/// Returns the pull request and whether this call opened it.
pub async fn open(worktree: &Path, branch: &str, context: &str) -> Result<(PullRequest, bool)> {
    if !is_task_branch(branch) {
        anyhow::bail!("{branch} is not a sidefleet task branch; not pushing it");
    }
    let (title, body) = context.split_once("\n\n").unwrap_or((context, ""));
    let title = if title.trim().is_empty() { "Changes from Sidefleet" } else { title.trim() };
    commit_and_push(worktree, branch, title, None).await.context("pushing the branch")?;

    if let Some(pr) = view(worktree, branch).await? {
        if pr.state != "closed" {
            return Ok((pr, false));
        }
    }
    let base = git(worktree, &["rev-parse", "--abbrev-ref", "origin/HEAD"], None).await.ok().and_then(|b| b.strip_prefix("origin/").map(str::to_string));
    let ahead = match &base {
        Some(b) => git(worktree, &["rev-list", "--count", &format!("origin/{b}..HEAD")], None).await.unwrap_or_default(),
        None => String::new(),
    };
    if ahead == "0" {
        anyhow::bail!("the branch has no commits that {} doesn't already have, so there is nothing to open a pull request for", base.as_deref().unwrap_or("the base branch"));
    }
    let mut args = vec!["pr", "create", "--head", branch, "--title", title, "--body", body];
    if let Some(b) = &base {
        args.extend(["--base", b.as_str()]);
    }
    gh(worktree, &args).await?;
    let pr = view(worktree, branch).await?.context("gh opened the pull request but can't find it")?;
    Ok((pr, true))
}

/// The pull request for a branch (or a pull request URL), if there is one.
pub async fn view(dir: &Path, branch_or_url: &str) -> Result<Option<PullRequest>> {
    match gh(dir, &["pr", "view", branch_or_url, "--json", "number,url,state"]).await {
        Ok(out) => Ok(Some(serde_json::from_str::<GhPr>(&out).context("reading gh's answer")?.into())),
        Err(e) if format!("{e:#}").contains("no pull requests found") => Ok(None),
        Err(e) => Err(e),
    }
}

/// `gh args` in `dir`, never prompting.
async fn gh(dir: &Path, args: &[&str]) -> Result<String> {
    let cmd = tokio::process::Command::new("gh")
        .args(args)
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = match tokio::time::timeout(GH_TIMEOUT, cmd).await {
        Err(_) => anyhow::bail!("gh {} timed out", args.first().copied().unwrap_or("")),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("the GitHub CLI (gh) isn't installed on this machine; install it and run `gh auth login`")
        }
        Ok(r) => r.context("running gh")?,
    };
    if !out.status.success() {
        anyhow::bail!("gh {} failed: {}", args.iter().take(2).copied().collect::<Vec<_>>().join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Commits what's left, pushes the branch, then asks `gh` (a stand-in here) to open it.
    #[tokio::test]
    async fn opens_a_pull_request_after_pushing_the_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, repo, wt, bin) = (tmp.path().join("origin.git"), tmp.path().join("repo"), tmp.path().join("wt"), tmp.path().join("bin"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let g = |dir: std::path::PathBuf, args: Vec<String>| async move { git(&dir, &args.iter().map(String::as_str).collect::<Vec<_>>(), None).await.unwrap() };
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        g(tmp.path().into(), s(&["init", "--quiet", "--bare", origin.to_str().unwrap()])).await;
        g(repo.clone(), s(&["init", "--quiet", "-b", "main"])).await;
        g(repo.clone(), s(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--allow-empty", "-m", "init"])).await;
        g(repo.clone(), s(&["remote", "add", "origin", origin.to_str().unwrap()])).await;
        g(repo.clone(), s(&["push", "--quiet", "origin", "main"])).await;
        g(repo.clone(), s(&["remote", "set-head", "origin", "main"])).await;
        g(repo.clone(), s(&["worktree", "add", "--quiet", "-b", "sidefleet/pr123", wt.to_str().unwrap(), "HEAD"])).await;

        // Nothing changed yet: no pull request to open.
        let gh_log = tmp.path().join("gh.log");
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> {log}\ncase \"$2\" in\n  view) if grep -q '^pr create' {log}; then echo '{{\"number\":5,\"url\":\"https://github.com/o/r/pull/5\",\"state\":\"OPEN\"}}'; else echo 'no pull requests found for branch' >&2; exit 1; fi ;;\n  create) echo https://github.com/o/r/pull/5 ;;\nesac\n",
            log = gh_log.display()
        );
        std::fs::write(bin.join("gh"), script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
        assert!(format!("{:#}", open(&wt, "sidefleet/pr123", "T\n\nB").await.unwrap_err()).contains("nothing to open"));

        std::fs::write(wt.join("f.txt"), "work").unwrap();
        let (pr, opened) = open(&wt, "sidefleet/pr123", "Fix the thing\n\nBecause.").await.unwrap();
        assert_eq!(pr, PullRequest { url: "https://github.com/o/r/pull/5".into(), number: 5, state: "open".into() });
        assert!(opened);
        assert_eq!(g(wt.clone(), s(&["log", "-1", "--format=%s"])).await, "Fix the thing");
        assert_eq!(g(origin.clone(), s(&["rev-parse", "refs/heads/sidefleet/pr123"])).await, g(wt.clone(), s(&["rev-parse", "HEAD"])).await);
        let log = std::fs::read_to_string(&gh_log).unwrap();
        assert!(log.contains("pr create --head sidefleet/pr123 --title Fix the thing --body Because. --base main"), "{log}");

        // Again: the open one is reused, not a second one opened.
        let (_, opened) = open(&wt, "sidefleet/pr123", "Fix the thing\n\nBecause.").await.unwrap();
        assert!(!opened);
    }
}
