//! Pull requests. A `pr` run commits what is left in the thread's worktree,
//! pushes its `sidefleet/...` branch to `origin` and opens a pull request with
//! the GitHub CLI (`gh`), signed in as whoever runs this worker. Open pull
//! requests are looked at again now and then, so the page shows when one is
//! merged or closed.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use sidefleet_core::{FailedCheck, PrChecks, PullRequest};

use crate::checkpoint::{commit_and_push, git, is_task_branch};

const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// The fields `view` asks `gh pr view` for.
const VIEW_FIELDS: &str = "number,url,state,title,baseRefName,isDraft,reviewDecision,mergeable,statusCheckRollup";

/// What `gh pr view --json VIEW_FIELDS` prints.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: i64,
    url: String,
    state: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    base_ref_name: String,
    #[serde(default)]
    is_draft: bool,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED` or empty.
    #[serde(default)]
    review_decision: Option<String>,
    /// `MERGEABLE`, `CONFLICTING` or `UNKNOWN` (still working it out).
    #[serde(default)]
    mergeable: Option<String>,
    #[serde(default)]
    status_check_rollup: Option<Vec<GhCheck>>,
}

/// A check run (GitHub Actions and apps) or a commit status (older integrations).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhCheck {
    /// Check runs: `QUEUED`, `IN_PROGRESS`, `COMPLETED`…
    #[serde(default)]
    status: Option<String>,
    /// Check runs, once completed: `SUCCESS`, `FAILURE`, `NEUTRAL`, `SKIPPED`, `CANCELLED`…
    #[serde(default)]
    conclusion: Option<String>,
    /// Commit statuses: `SUCCESS`, `PENDING`, `FAILURE`, `ERROR`, `EXPECTED`.
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
}

enum Outcome {
    Passed,
    Failed,
    Pending,
    /// Skipped or neutral: neither here nor there, so not counted.
    Ignored,
}

impl GhCheck {
    fn outcome(&self) -> Outcome {
        let up = |s: &Option<String>| s.as_deref().unwrap_or("").to_ascii_uppercase();
        if self.state.is_some() {
            return match up(&self.state).as_str() {
                "SUCCESS" => Outcome::Passed,
                "FAILURE" | "ERROR" => Outcome::Failed,
                _ => Outcome::Pending,
            };
        }
        if up(&self.status) != "COMPLETED" {
            return Outcome::Pending;
        }
        match up(&self.conclusion).as_str() {
            "SUCCESS" => Outcome::Passed,
            "NEUTRAL" | "SKIPPED" | "STALE" => Outcome::Ignored,
            _ => Outcome::Failed,
        }
    }
}

fn checks(rollup: Vec<GhCheck>) -> Option<PrChecks> {
    let mut c = PrChecks::default();
    for check in rollup {
        match check.outcome() {
            Outcome::Passed => c.passed += 1,
            Outcome::Pending => c.pending += 1,
            Outcome::Ignored => {}
            Outcome::Failed => {
                c.failed += 1;
                if c.failing.len() < 5 {
                    let name = check.name.or(check.context).unwrap_or_else(|| "A check".into());
                    c.failing.push(FailedCheck { name, url: check.details_url.or(check.target_url).filter(|u| !u.is_empty()) });
                }
            }
        }
    }
    (c.passed + c.failed + c.pending > 0).then_some(c)
}

impl From<GhPr> for PullRequest {
    fn from(p: GhPr) -> Self {
        let review = match p.review_decision.as_deref() {
            Some("APPROVED") => Some("approved"),
            Some("CHANGES_REQUESTED") => Some("changes_requested"),
            Some("REVIEW_REQUIRED") => Some("review_required"),
            _ => None,
        };
        PullRequest {
            url: p.url,
            number: p.number,
            state: p.state.to_ascii_lowercase(),
            title: Some(p.title).filter(|t| !t.is_empty()),
            base: Some(p.base_ref_name).filter(|b| !b.is_empty()),
            draft: p.is_draft,
            review: review.map(str::to_string),
            conflicts: p.mergeable.as_deref() == Some("CONFLICTING"),
            checks: p.status_check_rollup.and_then(checks),
        }
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
    match gh(dir, &["pr", "view", branch_or_url, "--json", VIEW_FIELDS]).await {
        Ok(out) => Ok(Some(serde_json::from_str::<GhPr>(&out).context("reading gh's answer")?.into())),
        Err(e) if format!("{e:#}").contains("no pull requests found") => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether `branch`'s pull request was merged with `head` as its last commit. Anything
/// but a clear yes (no `gh`, offline, commits added since) is a no.
pub async fn merged_at(dir: &Path, branch: &str, head: &str) -> bool {
    #[derive(Deserialize)]
    struct Merged {
        state: String,
        #[serde(rename = "headRefOid")]
        head: String,
    }
    let Ok(out) = gh(dir, &["pr", "view", branch, "--json", "state,headRefOid"]).await else {
        return false;
    };
    serde_json::from_str::<Merged>(&out).is_ok_and(|p| p.state.eq_ignore_ascii_case("merged") && p.head == head)
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
        assert_eq!(pr, PullRequest { url: "https://github.com/o/r/pull/5".into(), number: 5, state: "open".into(), ..Default::default() });
        assert!(opened);
        assert_eq!(g(wt.clone(), s(&["log", "-1", "--format=%s"])).await, "Fix the thing");
        assert_eq!(g(origin.clone(), s(&["rev-parse", "refs/heads/sidefleet/pr123"])).await, g(wt.clone(), s(&["rev-parse", "HEAD"])).await);
        let log = std::fs::read_to_string(&gh_log).unwrap();
        assert!(log.contains("pr create --head sidefleet/pr123 --title Fix the thing --body Because. --base main"), "{log}");

        // Again: the open one is reused, not a second one opened.
        let (_, opened) = open(&wt, "sidefleet/pr123", "Fix the thing\n\nBecause.").await.unwrap();
        assert!(!opened);
    }

    /// What `gh pr view` says, boiled down to what the page shows.
    #[test]
    fn reads_checks_review_and_conflicts() {
        let out = r#"{"number":9,"url":"https://github.com/o/r/pull/9","state":"OPEN","title":"Fix it","baseRefName":"main","isDraft":true,
            "reviewDecision":"CHANGES_REQUESTED","mergeable":"CONFLICTING","statusCheckRollup":[
            {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"https://github.com/o/r/actions/runs/1"},
            {"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":""},
            {"__typename":"CheckRun","name":"docs","status":"COMPLETED","conclusion":"SKIPPED"},
            {"__typename":"CheckRun","name":"e2e","status":"IN_PROGRESS","conclusion":""},
            {"__typename":"StatusContext","context":"deploy","state":"ERROR","targetUrl":""},
            {"__typename":"StatusContext","context":"preview","state":"PENDING"}]}"#;
        let pr: PullRequest = serde_json::from_str::<GhPr>(out).unwrap().into();
        assert_eq!((pr.state.as_str(), pr.title.as_deref(), pr.base.as_deref(), pr.draft, pr.review.as_deref(), pr.conflicts), ("open", Some("Fix it"), Some("main"), true, Some("changes_requested"), true));
        let c = pr.checks.unwrap();
        assert_eq!((c.passed, c.failed, c.pending), (1, 2, 2));
        assert_eq!(c.failing, vec![
            FailedCheck { name: "test".into(), url: Some("https://github.com/o/r/actions/runs/1".into()) },
            FailedCheck { name: "deploy".into(), url: None },
        ]);

        // No checks, no review asked for, mergeable: nothing extra.
        let pr: PullRequest = serde_json::from_str::<GhPr>(r#"{"number":1,"url":"u","state":"MERGED","reviewDecision":"","mergeable":"UNKNOWN","statusCheckRollup":[]}"#).unwrap().into();
        assert_eq!((pr.review, pr.conflicts, pr.checks), (None, false, None));
    }
}
