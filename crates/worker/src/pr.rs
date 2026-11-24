//! Pull requests. A `pr` run commits what is left in the thread's worktree (but
//! files the page left out), pushes its `sidefleet/...` branch to `origin` and
//! opens a pull request with the GitHub CLI (`gh`), signed in as whoever runs this
//! worker, titled and described by the thread's agent when asked. A `merge` run
//! merges it. Open pull
//! requests are looked at again now and then, so the page shows when one is
//! merged or closed.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use sidefleet_core::{FailedCheck, MergeRequest, PrChecks, PrRequest, PullRequest};

use crate::checkpoint::{commit_staged, git, is_task_branch, push_branch};
use crate::pr_text::Writer;

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

/// What `open` did.
#[derive(Debug)]
pub struct Opened {
    pub pr: PullRequest,
    /// This call opened it (rather than pushing to the open one).
    pub opened: bool,
    /// The agent wrote its title and description.
    pub written: bool,
    /// Files asked to be left out that are still only in the worktree.
    pub kept_out: Vec<String>,
    /// Files asked to be left out that the branch already had, so they're in it anyway.
    pub already_pushed: Vec<String>,
}

/// Commit the thread's changes (but the files `req` leaves out), push `branch` from
/// `worktree` and open a pull request for it, or push to the one already open.
/// A new one gets the title and description `req` gives, else ones `writer` writes,
/// else the task's.
pub async fn open(worktree: &Path, branch: &str, req: &PrRequest, writer: Option<&Writer<'_>>) -> Result<Opened> {
    if !is_task_branch(branch) {
        anyhow::bail!("{branch} is not a sidefleet task branch; not pushing it");
    }
    let head = git(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"], None).await.unwrap_or_default();
    if head != branch {
        anyhow::bail!("the worktree is on {} instead of {branch}; not pushing", if head.is_empty() { "a detached HEAD" } else { &head });
    }

    // Stage everything, then take back what's left out.
    git(worktree, &["add", "-A"], None).await?;
    let specs: Vec<String> = req.exclude.iter().filter(|p| !p.is_empty()).map(|p| format!(":(literal){p}")).collect();
    let mut kept_out = Vec::new();
    if !specs.is_empty() {
        for chunk in specs.chunks(100) {
            let mut args = vec!["reset", "--quiet", "--"];
            args.extend(chunk.iter().map(String::as_str));
            // An unborn HEAD can't be reset to; there's nothing committed to keep out then either.
            let _ = git(worktree, &args, None).await;
        }
        for chunk in specs.chunks(100) {
            let mut args = vec!["status", "--porcelain", "--untracked-files=all", "--"];
            args.extend(chunk.iter().map(String::as_str));
            let out = git(worktree, &args, None).await.unwrap_or_default();
            kept_out.extend(out.lines().filter_map(|l| l.get(3..)).map(|p| p.trim_matches('"').to_string()));
        }
    }

    let base = git(worktree, &["rev-parse", "--abbrev-ref", "origin/HEAD"], None).await.ok().and_then(|b| b.strip_prefix("origin/").map(str::to_string));
    let merge_base = match &base {
        Some(b) => git(worktree, &["merge-base", &format!("origin/{b}"), "HEAD"], None).await.ok(),
        None => None,
    };
    let mut already_pushed = Vec::new();
    if let (Some(mb), false) = (&merge_base, specs.is_empty()) {
        for chunk in specs.chunks(100) {
            let range = format!("{mb}..HEAD");
            let mut args = vec!["diff", "--name-only", range.as_str(), "--"];
            args.extend(chunk.iter().map(String::as_str));
            already_pushed.extend(git(worktree, &args, None).await.unwrap_or_default().lines().map(str::to_string));
        }
    }

    let existing = view(worktree, branch).await?.filter(|p| p.state != "closed");
    let fallback_title = if req.fallback_title.trim().is_empty() { "Changes from Sidefleet" } else { req.fallback_title.trim() };
    let given_title = req.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let mut written = false;
    let (title, body) = if existing.is_some() {
        (String::new(), String::new())
    } else if let Some(t) = given_title {
        (t.to_string(), req.body.clone().unwrap_or_else(|| req.fallback_body.clone()))
    } else if let (true, Some(w), Some(mb)) = (req.write, writer, &merge_base) {
        let wrote = match crate::pr_text::material(worktree, mb, &req.messages).await {
            Ok(m) => crate::pr_text::write(w, worktree, &m).await,
            Err(e) => Err(e),
        };
        match wrote {
            Ok((t, b)) => {
                written = true;
                (t, req.body.clone().unwrap_or(format!("{b}\n\nOpened from Sidefleet.")))
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "the agent couldn't write the pull request; using the task");
                (fallback_title.to_string(), req.body.clone().unwrap_or_else(|| req.fallback_body.clone()))
            }
        }
    } else {
        (fallback_title.to_string(), req.body.clone().unwrap_or_else(|| req.fallback_body.clone()))
    };

    // A new pull request's commit is named after it; one pushed to an open one, after the latest message.
    let message = match &existing {
        None => title.clone(),
        Some(_) => {
            let latest = req.messages.last().and_then(|m| m.lines().map(str::trim).find(|l| !l.is_empty())).unwrap_or(fallback_title);
            let latest: String = latest.chars().take(72).collect();
            format!("Update: {latest}")
        }
    };
    commit_staged(worktree, &message).await.context("committing the changes")?;
    push_branch(worktree, branch).await.context("pushing the branch")?;

    if let Some(pr) = existing {
        return Ok(Opened { pr, opened: false, written, kept_out, already_pushed });
    }
    let ahead = match &base {
        Some(b) => git(worktree, &["rev-list", "--count", &format!("origin/{b}..HEAD")], None).await.unwrap_or_default(),
        None => String::new(),
    };
    if ahead == "0" {
        anyhow::bail!("the branch has no commits that {} doesn't already have, so there is nothing to open a pull request for", base.as_deref().unwrap_or("the base branch"));
    }
    let mut args = vec!["pr", "create", "--head", branch, "--title", title.as_str(), "--body", body.as_str()];
    if let Some(b) = &base {
        args.extend(["--base", b.as_str()]);
    }
    if req.draft {
        args.push("--draft");
    }
    gh(worktree, &args).await?;
    let pr = view(worktree, branch).await?.context("gh opened the pull request but can't find it")?;
    Ok(Opened { pr, opened: true, written, kept_out, already_pushed })
}

/// What `merge` did.
#[derive(Debug)]
pub struct Merged {
    pub pr: PullRequest,
    /// Changed files in the worktree that weren't in the pull request, and stay behind.
    pub left_behind: usize,
}

/// Merge the thread's pull request on GitHub, as long as its last commit is the one
/// on `branch` here: what the page showed is what gets merged.
pub async fn merge(worktree: &Path, branch: &str, req: &MergeRequest) -> Result<Merged> {
    let method = match req.method.as_str() {
        "squash" => "--squash",
        "merge" => "--merge",
        "rebase" => "--rebase",
        m => anyhow::bail!("{m} isn't a way to merge; use squash, merge or rebase"),
    };
    #[derive(Deserialize)]
    struct Head {
        state: String,
        #[serde(rename = "headRefOid")]
        head: String,
    }
    let now: Head = serde_json::from_str(&gh(worktree, &["pr", "view", &req.url, "--json", "state,headRefOid"]).await?).context("reading gh's answer")?;
    if !now.state.eq_ignore_ascii_case("open") {
        anyhow::bail!("the pull request is {} already", now.state.to_ascii_lowercase());
    }
    let head = git(worktree, &["rev-parse", "HEAD"], None).await?;
    if head != now.head {
        anyhow::bail!("the pull request's last commit isn't the one on {branch} here; push the changes first (or pull what was added on GitHub), then merge");
    }
    gh(worktree, &["pr", "merge", &req.url, method, "--match-head-commit", &head]).await?;
    let pr = view(worktree, &req.url).await?.context("gh merged the pull request but can't find it")?;
    let left_behind = git(worktree, &["status", "--porcelain"], None).await.unwrap_or_default().lines().count();
    Ok(Merged { pr, left_behind })
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

    /// Commits what's left (but what's left out), pushes the branch, asks `gh` (a stand-in
    /// here) to open it, pushes to it again, then merges it.
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

        // A stand-in gh: no pull request until one is created, open until it's merged.
        let gh_log = tmp.path().join("gh.log");
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> {log}\ncase \"$2\" in\n  view) if ! grep -q '^pr create' {log}; then echo 'no pull requests found for branch' >&2; exit 1; fi\n    state=OPEN; grep -q '^pr merge' {log} && state=MERGED\n    echo \"{{\\\"number\\\":5,\\\"url\\\":\\\"https://github.com/o/r/pull/5\\\",\\\"state\\\":\\\"$state\\\",\\\"headRefOid\\\":\\\"$(git -C {wt} rev-parse HEAD)\\\"}}\" ;;\n  create) echo https://github.com/o/r/pull/5 ;;\nesac\n",
            log = gh_log.display(),
            wt = wt.display()
        );
        std::fs::write(bin.join("gh"), script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
        let req = |exclude: &[&str]| PrRequest {
            fallback_title: "Fix the thing".into(),
            fallback_body: "Because.".into(),
            draft: true,
            exclude: exclude.iter().map(|x| x.to_string()).collect(),
            messages: vec!["Fix the thing".into(), "Also the docs".into()],
            ..Default::default()
        };

        // Nothing changed yet: no pull request to open.
        assert!(format!("{:#}", open(&wt, "sidefleet/pr123", &req(&[]), None).await.unwrap_err()).contains("nothing to open"));

        // A file left out stays in the worktree, uncommitted.
        std::fs::write(wt.join("f.txt"), "work").unwrap();
        std::fs::write(wt.join("scratch notes.md"), "mine").unwrap();
        // The agent (a stand-in claude that wants to see the diff) writes the title and description.
        let claude = bin.join("claude");
        std::fs::write(&claude, "#!/bin/sh\ngrep -q '+work' || exit 3\necho '{\"is_error\":false,\"structured_output\":{\"title\":\"Do the work\",\"body\":\"It was due.\"}}'\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let writer = Writer { agent: sidefleet_core::Agent::Claude, claude_bin: claude.to_str().unwrap(), codex_bin: "/nonexistent/codex" };
        let o = open(&wt, "sidefleet/pr123", &PrRequest { write: true, ..req(&["scratch notes.md"]) }, Some(&writer)).await.unwrap();
        assert_eq!(o.pr, PullRequest { url: "https://github.com/o/r/pull/5".into(), number: 5, state: "open".into(), ..Default::default() });
        assert!(o.opened && o.written);
        assert_eq!((o.kept_out, o.already_pushed), (vec!["scratch notes.md".to_string()], vec![]));
        assert_eq!(g(wt.clone(), s(&["log", "-1", "--format=%s"])).await, "Do the work");
        assert_eq!(g(wt.clone(), s(&["show", "--name-only", "--format=", "HEAD"])).await, "f.txt");
        assert_eq!(g(wt.clone(), s(&["status", "--porcelain"])).await, "?? \"scratch notes.md\"");
        assert_eq!(g(origin.clone(), s(&["rev-parse", "refs/heads/sidefleet/pr123"])).await, g(wt.clone(), s(&["rev-parse", "HEAD"])).await);
        let log = std::fs::read_to_string(&gh_log).unwrap();
        assert!(log.contains("pr create --head sidefleet/pr123 --title Do the work --body It was due.\n"), "{log}");
        assert!(log.contains("Opened from Sidefleet. --base main --draft"), "{log}");

        // Again: pushed to the open one, named after the latest message; f.txt is already on the branch.
        std::fs::write(wt.join("docs.md"), "docs").unwrap();
        let o = open(&wt, "sidefleet/pr123", &req(&["scratch notes.md", "f.txt"]), None).await.unwrap();
        assert!(!o.opened);
        assert_eq!(o.already_pushed, vec!["f.txt".to_string()]);
        assert_eq!(g(wt.clone(), s(&["log", "-1", "--format=%s"])).await, "Update: Also the docs");

        // Merging: a way that isn't one is refused; then gh merges exactly the pushed commit.
        let m = |method: &str| MergeRequest { method: method.into(), url: "https://github.com/o/r/pull/5".into() };
        assert!(merge(&wt, "sidefleet/pr123", &m("octopus")).await.is_err());
        let merged = merge(&wt, "sidefleet/pr123", &m("squash")).await.unwrap();
        assert_eq!((merged.pr.state.as_str(), merged.left_behind), ("merged", 1));
        let head = g(wt.clone(), s(&["rev-parse", "HEAD"])).await;
        assert!(std::fs::read_to_string(&gh_log).unwrap().contains(&format!("pr merge https://github.com/o/r/pull/5 --squash --match-head-commit {head}")));
        assert!(format!("{:#}", merge(&wt, "sidefleet/pr123", &m("squash")).await.unwrap_err()).contains("merged already"));
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
