//! The worker loop: register, heartbeat every 15s, long-poll for a claimable run,
//! execute it, and report back. Backs off on control plane errors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sidefleet_core::{Agent, Capabilities, NewEvent, PrCheck, PrReport, Run, RunStatus, Slots, ASK_MODE};
use std::sync::atomic::{AtomicUsize, Ordering};
use anyhow::{Context, Result};
use serde_json::json;
use tokio::sync::{mpsc, watch, Mutex, Semaphore};
use uuid::Uuid;

use crate::agent::{self, AgentSpec, ApprovalHook};
use crate::approval::{self, Bridge};
use crate::checkpoint::{self, ChangeTracker};
use crate::client::{Backoff, ClientError, ControlClient};
use crate::diff;
use crate::snapshot;
use crate::detect::{self, DetectOptions};

/// How long the control plane may hold a claim open waiting for a run.
const CLAIM_WAIT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub control_url: String,
    pub token: String,
    pub worker_id: String,
    pub display_name: String,
    pub repos_root: PathBuf,
    pub claude_bin: String,
    pub codex_bin: String,
    pub opencode_bin: String,
    pub permission_mode: String,
    pub max_turns: Option<u32>,
    pub extra_args: Vec<String>,
    pub poll_interval: Duration,
    pub heartbeat_interval: Duration,
    pub check_sign_in: bool,
    pub scan_depth: usize,
    /// Only used by tests: exit after this many runs.
    pub max_runs: Option<usize>,
    /// How many runs may execute at the same time on this machine.
    pub parallel: usize,
    /// Where per-task git worktrees are created (`<dir>/<repo-slug>/<root run id>`).
    pub worktrees_dir: PathBuf,
    /// Build of this binary, reported in capabilities.
    pub version: String,
    /// Act on "Update" from the page: pull/reinstall and restart when idle.
    pub allow_remote_update: bool,
    /// The `sidefleet` executable, which serves "Ask me" permission prompts to Claude
    /// Code (`sidefleet approval-mcp`). None: "Ask me" refuses like "Read & edit files".
    pub approval_exe: Option<PathBuf>,
    /// How long a tool call waits for someone to approve it before it is denied.
    pub approval_timeout: Duration,
}

pub async fn run(cfg: WorkerConfig) -> Result<()> {
    let client = ControlClient::new(&cfg.control_url, &cfg.token, &cfg.worker_id)?;
    let detect_opts = DetectOptions {
        claude_bin: cfg.claude_bin.clone(),
        codex_bin: cfg.codex_bin.clone(),
        opencode_bin: cfg.opencode_bin.clone(),
        repos_root: cfg.repos_root.clone(),
        check_sign_in: cfg.check_sign_in,
        scan_depth: cfg.scan_depth,
    };

    tracing::info!(worker = %cfg.worker_id, repos_root = %cfg.repos_root.display(), "detecting capabilities");
    let mut caps = detect::detect_all(&detect_opts).await;
    caps.slots = Some(Slots { parallel: cfg.parallel.max(1), active: 0 });
    caps.version = cfg.version.clone();
    log_caps(&caps);
    let caps = Arc::new(Mutex::new(caps));
    let active_count = Arc::new(AtomicUsize::new(0));
    // Active runs: run id -> (repo, isolation). Reported on every heartbeat so the
    // control plane can fail runs we no longer execute; also keeps in-place runs one per repo.
    let active: Arc<Mutex<HashMap<Uuid, (String, String)>>> = Arc::new(Mutex::new(HashMap::new()));

    register_with_backoff(&client, &cfg, &caps, &active).await;

    // Heartbeat task: re-scans repos each time so new checkouts show up.
    {
        let client = client.clone();
        let caps = caps.clone();
        let interval = cfg.heartbeat_interval;
        let root = cfg.repos_root.clone();
        let depth = cfg.scan_depth;
        let cfg2 = cfg.clone();
        let parallel = cfg.parallel.max(1);
        let active_count = active_count.clone();
        let active = active.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                {
                    let mut c = caps.lock().await;
                    c.repos = detect::scan_repos(&root, depth);
                    c.system = Some(detect::system_stats(&root));
                    c.slots = Some(Slots { parallel, active: active_count.load(Ordering::Relaxed) });
                }
                let snapshot = caps.lock().await.clone();
                let ids: Vec<Uuid> = active.lock().await.keys().copied().collect();
                match client.heartbeat(&snapshot, &ids).await {
                    Ok(resp) => {
                        if !resp.check_prs.is_empty() {
                            tokio::spawn(check_prs(client.clone(), cfg2.repos_root.clone(), resp.check_prs));
                        }
                        if resp.update_requested && cfg2.allow_remote_update {
                            if !ids.is_empty() {
                                tracing::info!(active = ids.len(), "update requested; waiting until this machine is idle");
                            } else {
                                tracing::info!("update requested from the page; updating now");
                                let _ = client.update_ack().await;
                                match tokio::task::spawn_blocking(|| crate::updater::update(true)).await {
                                    Ok(Ok(r)) => tracing::info!(restarted = ?r.restarted, manual = ?r.needs_manual_restart, "update finished"),
                                    Ok(Err(e)) => tracing::error!(error = %e, "update failed"),
                                    Err(e) => tracing::error!(error = %e, "update task failed"),
                                }
                            }
                        }
                    }
                    Err(ClientError::NotRegistered) => {
                        tracing::warn!("control plane forgot us; re-registering");
                        register_with_backoff(&client, &cfg2, &caps, &active).await;
                    }
                    Err(e) => tracing::warn!(error = %e, "heartbeat failed"),
                }
            }
        });
    }

    let mut backoff = Backoff::new(Duration::from_secs(30));
    let permits = Arc::new(Semaphore::new(cfg.parallel.max(1)));
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Uuid>();
    let mut completed = 0usize;
    let cfg = Arc::new(cfg);
    loop {
        // Reap finished runs (non-blocking) so `completed` and `active` stay current.
        while let Ok(id) = done_rx.try_recv() {
            active.lock().await.remove(&id);
            completed += 1;
        }
        if let Some(max) = cfg.max_runs {
            if completed >= max {
                return Ok(());
            }
            // Claim no more than the runs left; wait for those in flight to end.
            if completed + active.lock().await.len() >= max {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        }
        // Wait for a free slot before asking for work, so we never claim more than we can run.
        let permit = match permits.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        // Offer both identities (shared across machines) and path names (what tasks
        // created before identities existed refer to).
        let repos: Vec<String> = {
            let c = caps.lock().await;
            let mut v: Vec<String> = c.repos.iter().flat_map(|r| [r.key().to_string(), r.name.clone()]).collect();
            v.sort();
            v.dedup();
            v
        };
        // Only ask for tasks of agents installed here.
        let agents: Vec<String> = caps.lock().await.agents.runnable().iter().map(|a| a.as_str().to_string()).collect();
        if agents.is_empty() {
            drop(permit);
            tokio::time::sleep(cfg.poll_interval.max(Duration::from_secs(10))).await;
            continue;
        }
        let busy: Vec<String> = active
            .lock()
            .await
            .values()
            .filter(|(_, iso)| iso == "checkout")
            .map(|(repo, _)| repo.clone())
            .collect();
        // Our load, so the control plane can prefer a machine with more free slots.
        let slots = Slots { parallel: cfg.parallel.max(1), active: active_count.load(Ordering::Relaxed) };
        // Let the control plane hold the claim until a run is queued, so a new task starts at once.
        // While a repo is busy here, keep the wait short: when that run ends, the next claim must
        // say so, or a task for that repo would wait out the whole poll.
        let wait = if busy.is_empty() { CLAIM_WAIT } else { cfg.poll_interval };
        let asked = tokio::time::Instant::now();
        match client.claim(&repos, &busy, &agents, &slots, wait).await {
            Ok(Some(run)) => {
                backoff.reset();
                let repo_path = resolve_repo(&caps.lock().await.repos, &run.repo);
                active.lock().await.insert(run.id, (run.repo.clone(), run.isolation.clone()));
                active_count.fetch_add(1, Ordering::Relaxed);
                let client = client.clone();
                let cfg = cfg.clone();
                let done_tx = done_tx.clone();
                let active_count = active_count.clone();
                tokio::spawn(async move {
                    let id = run.id;
                    execute_run(&client, &cfg, run, repo_path).await;
                    active_count.fetch_sub(1, Ordering::Relaxed);
                    drop(permit);
                    let _ = done_tx.send(id);
                });
            }
            Ok(None) => {
                drop(permit);
                backoff.reset();
                // A control plane that held the claim has waited already; an older one answers
                // at once, so poll it at the usual pace.
                tokio::time::sleep(cfg.poll_interval.saturating_sub(asked.elapsed())).await;
            }
            Err(ClientError::NotRegistered) => {
                drop(permit);
                register_with_backoff(&client, &cfg, &caps, &active).await;
            }
            Err(e) => {
                drop(permit);
                let wait = backoff.next_delay();
                tracing::warn!(error = %e, retry_in = ?wait, "claim failed");
                tokio::time::sleep(wait).await;
            }
        }
    }
}

/// The checkout a task should run in. An exact path name wins; otherwise the
/// checkouts with that identity, preferring the one whose folder is named after
/// the project, then the shortest path (a machine can hold several clones).
pub fn resolve_repo(repos: &[sidefleet_core::RepoInfo], wanted: &str) -> Option<PathBuf> {
    if let Some(r) = repos.iter().find(|r| r.name == wanted) {
        return Some(PathBuf::from(&r.path));
    }
    let project = wanted.rsplit('/').next().unwrap_or(wanted);
    repos
        .iter()
        .filter(|r| r.key() == wanted)
        .min_by_key(|r| {
            let folder = r.name.rsplit('/').next().unwrap_or(&r.name).to_ascii_lowercase();
            (folder != project, r.path.len(), r.path.clone())
        })
        .map(|r| PathBuf::from(&r.path))
}

async fn register_with_backoff(
    client: &ControlClient,
    cfg: &WorkerConfig,
    caps: &Arc<Mutex<Capabilities>>,
    active: &Arc<Mutex<HashMap<Uuid, (String, String)>>>,
) {
    let mut backoff = Backoff::new(Duration::from_secs(30));
    loop {
        let snapshot = caps.lock().await.clone();
        let ids: Vec<Uuid> = active.lock().await.keys().copied().collect();
        match client.register(&cfg.display_name, &snapshot, &ids).await {
            Ok(w) => {
                tracing::info!(worker = %w.id, control = %cfg.control_url, "registered with control plane");
                return;
            }
            Err(e) => {
                let wait = backoff.next_delay();
                tracing::warn!(error = %e, retry_in = ?wait, "register failed");
                tokio::time::sleep(wait).await;
            }
        }
    }
}

fn log_caps(caps: &Capabilities) {
    if let Some(c) = &caps.agents.claude {
        tracing::info!(version = %c.version, signed_in = c.signed_in, error = c.sign_in_error.as_deref().unwrap_or(""), "claude code");
    }
    if let Some(c) = &caps.agents.codex {
        tracing::info!(version = %c.version, signed_in = ?c.signed_in, models = c.models.len(), "codex");
    }
    if let Some(o) = &caps.agents.opencode {
        tracing::info!(version = %o.version, models = o.models_total, "opencode");
    }
    if caps.agents.runnable().is_empty() {
        tracing::warn!("no coding agent found (claude, codex or opencode); this machine will not take tasks");
    }
    if let Some(o) = &caps.agents.ollama {
        tracing::info!(version = %o.version, models = o.models.len(), "ollama found");
    }
    tracing::info!(count = caps.repos.len(), repos = ?caps.repos.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), "git checkouts");
}

/// A per-task git worktree: `<worktrees_dir>/<repo-slug>/<root run id>` on branch
/// `sidefleet/<first 8 of root id>`. Follow-ups reuse the root's worktree so the
/// agent's session (Claude Code keys sessions by cwd) resumes in the same place.
/// A run moved here from a machine that dropped starts from `resume_from`, the
/// task branch that machine pushed to `origin`. A fork starts from `start`, a
/// snapshot: the branch begins at the commit that was HEAD when it was taken,
/// and the files are then set to the snapshot.
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// Where a fresh task branch began, when it began from the remote's default branch.
    pub from: Option<StartPoint>,
}

/// `origin/<default branch>` at a commit.
#[derive(Debug, Clone)]
pub struct StartPoint {
    pub reference: String,
    pub commit: String,
}

const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Fetch the default branch of `origin`; if HEAD is just behind it (an ancestor), return it
/// as the place to start from. Offline, no remote, a failed fetch, or local commits that are
/// not pushed all give `None`, so the task starts from HEAD as before.
async fn latest_default_branch(repo: &Path) -> Option<StartPoint> {
    use crate::checkpoint::git as timed_git;
    // Never wait on an SSH passphrase or host-key prompt; fail and start from HEAD instead.
    // A command the user set themselves is left alone.
    let custom_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some() || timed_git(repo, &["config", "core.sshCommand"], None).await.is_ok();
    let batch: &[&str] = if custom_ssh { &[] } else { &["-c", "core.sshCommand=ssh -o BatchMode=yes"] };
    let head = timed_git(repo, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], None).await.ok();
    let branch = match head.as_deref().and_then(|h| h.strip_prefix("origin/")) {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => {
            // origin/HEAD is not set locally (git remote add, no clone): ask the remote.
            let out = timed_git(repo, &[batch, &["ls-remote", "--symref", "origin", "HEAD"]].concat(), Some(FETCH_TIMEOUT)).await.ok()?;
            out.lines().find_map(|l| l.strip_prefix("ref: refs/heads/")?.split_whitespace().next().map(str::to_string))?
        }
    };
    let remote_ref = format!("refs/remotes/origin/{branch}");
    timed_git(repo, &[batch, &["fetch", "--quiet", "--no-tags", "origin", &format!("+refs/heads/{branch}:{remote_ref}")]].concat(), Some(FETCH_TIMEOUT)).await.ok()?;
    let commit = timed_git(repo, &["rev-parse", "--verify", "--quiet", &format!("{remote_ref}^{{commit}}")], None).await.ok()?;
    timed_git(repo, &["merge-base", "--is-ancestor", "HEAD", &commit], None).await.ok()?;
    Some(StartPoint { reference: format!("origin/{branch}"), commit })
}

pub async fn prepare_worktree(repo: &Path, repo_name: &str, root_id: Uuid, worktrees_dir: &Path, resume_from: Option<&str>, start: Option<&str>) -> Result<Worktree> {
    let slug: String = repo_name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let path = worktrees_dir.join(slug).join(root_id.to_string());
    let branch = format!("sidefleet/{}", &root_id.simple().to_string()[..8]);
    if path.join(".git").exists() {
        return Ok(Worktree { path, branch, from: None });
    }
    std::fs::create_dir_all(path.parent().unwrap()).context("creating worktrees dir")?;
    let p = path.to_string_lossy().to_string();
    if let Some(from) = resume_from {
        let start = crate::checkpoint::fetch_branch(repo, from).await?;
        git(repo, &["worktree", "add", "-B", &branch, &p, &start]).await?;
        return Ok(Worktree { path, branch, from: None });
    }
    // Create the branch at HEAD, or reuse it when it already exists (worker restarted).
    let branch_exists = git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).await.is_ok();
    if branch_exists {
        git(repo, &["worktree", "add", &p, &branch]).await?;
    } else if let Some(snap) = start {
        git(repo, &["cat-file", "-e", &format!("{snap}^{{commit}}")]).await.context("the snapshot to fork from is not in this repo")?;
        let base = git(repo, &["rev-parse", "--verify", "--quiet", &format!("{snap}^1")]).await.unwrap_or_else(|_| "HEAD".into());
        git(repo, &["worktree", "add", "-b", &branch, &p, &base]).await?;
        snapshot::restore(&path, snap).await.context("setting up the forked files")?;
    } else {
        let from = latest_default_branch(repo).await;
        let base = from.as_ref().map_or("HEAD", |f| f.commit.as_str());
        git(repo, &["worktree", "add", "-b", &branch, &p, base]).await?;
        return Ok(Worktree { path, branch, from });
    }
    Ok(Worktree { path, branch, from: None })
}

async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("running git")?;
    if !out.status.success() {
        anyhow::bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Download the images attached to the run into a private temporary folder
/// (outside the repo, so they never end up in a commit). One that cannot be
/// fetched is reported and left out.
async fn fetch_images(client: &ControlClient, run: &Run) -> (Option<PathBuf>, Vec<PathBuf>) {
    if run.images.is_empty() {
        return (None, vec![]);
    }
    let dir = std::env::temp_dir().join("sidefleet-images").join(run.id.to_string());
    let made = {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)
    };
    if let Err(e) = made {
        let msg = format!("could not save the attached images ({e}); the task runs without them");
        let _ = client.post_events(run.id, vec![NewEvent { kind: "error".into(), payload: json!({"message": msg}) }]).await;
        return (None, vec![]);
    }
    let mut paths = Vec::with_capacity(run.images.len());
    for (n, img) in run.images.iter().enumerate() {
        let ext = match img.media_type.as_str() {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        let path = dir.join(format!("image-{}.{ext}", n + 1));
        let saved = match client.image(img.id).await {
            Ok(bytes) => std::fs::write(&path, bytes).map_err(anyhow::Error::from),
            Err(e) => Err(e),
        };
        match saved {
            Ok(()) => paths.push(path),
            Err(e) => {
                let msg = format!("could not fetch attached image {} ({e:#}); the task runs without it", n + 1);
                let _ = client.post_events(run.id, vec![NewEvent { kind: "error".into(), payload: json!({"message": msg}) }]).await;
            }
        }
    }
    (Some(dir), paths)
}

/// Execute one claimed run: forward events as they arrive, poll for stop, report the outcome.
async fn execute_run(client: &ControlClient, cfg: &WorkerConfig, run: Run, repo_path: Option<PathBuf>) {
    let run_id = run.id;
    tracing::info!(run_id = %run_id, repo = %run.repo, agent = %run.agent, model = %run.model, isolation = %run.isolation, "executing run");
    let claimed_at = std::time::Instant::now();

    let Some(repo_dir) = repo_path else {
        let msg = format!("repo {} is no longer present under {}", run.repo, cfg.repos_root.display());
        let _ = client.post_events(run_id, vec![NewEvent { kind: "error".into(), payload: json!({"message": msg}) }]).await;
        let _ = client.finish(run_id, RunStatus::Failed, Some(json!({"error": msg})), None).await;
        return;
    };

    // Where the agent works: its own worktree and branch, or the checkout itself.
    let mut pre_events: Vec<NewEvent> = Vec::new();
    // The worktree and branch to checkpoint, for a run that may move if this machine drops.
    let mut checkpoints: Option<(PathBuf, String)> = None;
    let root_id = run.parent_id.unwrap_or(run.id);
    let fork_from = run.fork_commit.as_deref().filter(|c| snapshot::is_commit_id(c));
    let mut in_worktree = false;
    let mut branch: Option<String> = None;
    let cwd = if run.isolation == "worktree" {
        let mut resume_from = run.resume_from_branch.as_deref();
        let mut prepared = prepare_worktree(&repo_dir, &run.repo, root_id, &cfg.worktrees_dir, resume_from, fork_from).await;
        if let (Err(e), Some(from)) = (&prepared, resume_from) {
            pre_events.push(NewEvent {
                kind: "error".into(),
                payload: json!({"message": format!("could not continue from {from} on origin ({e:#}); starting from this machine's HEAD")}),
            });
            resume_from = None;
            prepared = prepare_worktree(&repo_dir, &run.repo, root_id, &cfg.worktrees_dir, None, fork_from).await;
        }
        match prepared {
            Ok(wt) => {
                in_worktree = true;
                branch = Some(wt.branch.clone());
                pre_events.push(NewEvent {
                    kind: "workspace".into(),
                    payload: json!({"mode": "worktree", "path": wt.path.to_string_lossy(), "branch": wt.branch, "repo": repo_dir.to_string_lossy(), "resumed_from": resume_from, "from": wt.from.as_ref().map(|f| json!({"ref": f.reference, "commit": f.commit}))}),
                });
                if run.failover {
                    checkpoints = Some((wt.path.clone(), wt.branch.clone()));
                }
                wt.path
            }
            Err(e) if fork_from.is_some() => {
                // Running a fork in the checkout would lose the files it continues from.
                let msg = format!("could not set up the forked worktree: {e:#}");
                pre_events.push(NewEvent { kind: "error".into(), payload: json!({"message": msg}) });
                let _ = client.post_events(run_id, pre_events).await;
                report_finish(client, run_id, RunStatus::Failed, json!({"error": msg}), None).await;
                return;
            }
            Err(e) => {
                pre_events.push(NewEvent {
                    kind: "workspace".into(),
                    payload: json!({"mode": "checkout", "path": repo_dir.to_string_lossy(), "note": format!("could not create a worktree ({e:#}); running in the checkout instead")}),
                });
                repo_dir.clone()
            }
        }
    } else {
        pre_events.push(NewEvent { kind: "workspace".into(), payload: json!({"mode": "checkout", "path": repo_dir.to_string_lossy()}) });
        repo_dir.clone()
    };
    if let Err(e) = client.post_events(run_id, pre_events).await {
        tracing::warn!(error = %e, "posting workspace event failed");
    }

    if run.kind == "restore" {
        let (status, result) = match run_restore(client, &run, root_id, &cwd, in_worktree).await {
            Ok(sha) => (RunStatus::Succeeded, json!({"restored_to": sha})),
            Err(e) => {
                let msg = format!("{e:#}");
                let _ = client.post_events(run_id, vec![NewEvent { kind: "error".into(), payload: json!({"message": msg}) }]).await;
                (RunStatus::Failed, json!({"error": msg}))
            }
        };
        report_finish(client, run_id, status, result, None).await;
        return;
    }

    if run.kind == "pr" {
        let (status, result) = match run_pr(client, &run, root_id, &cwd, branch.as_deref()).await {
            Ok(pr) => (RunStatus::Succeeded, json!({"pr": pr})),
            Err(e) => {
                let msg = sidefleet_core::redact::redact(&format!("{e:#}"));
                let _ = client.post_events(run_id, vec![NewEvent { kind: "error".into(), payload: json!({"message": msg}) }]).await;
                (RunStatus::Failed, json!({"error": msg}))
            }
        };
        report_finish(client, run_id, status, result, None).await;
        return;
    }

    // The tree before the agent starts, and the one the thread started from, to diff against afterwards.
    let setup_at = std::time::Instant::now();
    let before = match diff::point(&cwd).await {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::debug!(error = %e, "no snapshot before the run");
            None
        }
    };
    let point_at = std::time::Instant::now();
    let base = match &before {
        Some(b) => diff::thread_base(&cwd, root_id, b).await,
        None => None,
    };
    let base_at = std::time::Instant::now();
    // Kept as a commit too, to restore to or fork from later.
    let before_commit = match &before {
        Some(p) => snapshot::keep(&cwd, &p.tree, &snapshot::ref_name(root_id, run_id, "before")).await.map_err(|e| tracing::warn!(error = %format!("{e:#}"), "keeping the snapshot failed")).ok(),
        None => None,
    };
    let diff_cwd = cwd.clone();

    let keep_at = std::time::Instant::now();
    let (images_dir, images) = fetch_images(client, &run).await;
    // What runs before the agent starts, so slow git work ahead of each turn shows up.
    let ms = |d: Duration| d.as_millis() as u64;
    tracing::debug!(
        run_id = %run_id,
        workdir_ms = ms(setup_at - claimed_at),
        point_ms = ms(point_at - setup_at),
        thread_base_ms = ms(base_at - point_at),
        snapshot_ms = ms(keep_at - base_at),
        images_ms = ms(keep_at.elapsed()),
        total_ms = ms(claimed_at.elapsed()),
        "ready to start the agent"
    );

    let bin = match run.agent {
        Agent::Claude => &cfg.claude_bin,
        Agent::Codex => &cfg.codex_bin,
        Agent::Opencode => &cfg.opencode_bin,
    };
    let permission_mode = run.permission_mode.clone().unwrap_or_else(|| cfg.permission_mode.clone());
    let (stop_tx, stop_rx) = watch::channel(false);

    // "Ask me": Claude Code's permission prompts come to us over a socket and go to the page.
    let mut bridge = None;
    if run.agent == Agent::Claude && permission_mode == ASK_MODE {
        match start_approvals(client, cfg, run_id, &stop_tx) {
            Ok(b) => bridge = Some(b),
            Err(e) => {
                let msg = format!("could not set up \"Ask me\" ({e:#}); tool calls that need approval are refused, as with Read & edit files");
                tracing::warn!(run_id = %run_id, "{msg}");
                let _ = client.post_events(run_id, vec![NewEvent { kind: "error".into(), payload: json!({"message": msg}) }]).await;
            }
        }
    }
    let approval = match (&bridge, &cfg.approval_exe) {
        (Some(b), Some(exe)) => Some(ApprovalHook { exe: exe.clone(), socket: b.socket.clone(), timeout: cfg.approval_timeout }),
        _ => None,
    };

    let spec = AgentSpec {
        agent: run.agent,
        bin: bin.clone(),
        // A fork's agent gets the earlier conversation ahead of the new message.
        task: match &run.context {
            Some(c) if !c.is_empty() => format!("{c}\n\n{}", run.task),
            _ => run.task.clone(),
        },
        model: run.model.clone(),
        effort: run.effort.clone(),
        cwd,
        permission_mode,
        max_turns: cfg.max_turns,
        extra_args: cfg.extra_args.clone(),
        resume_session_id: run.resume_session_id.clone(),
        images,
        approval,
    };

    let (event_tx, mut event_rx) = mpsc::channel::<NewEvent>(256);

    let exec = tokio::spawn(agent::execute(spec, event_tx, stop_rx));

    // Failover runs push a checkpoint of their branch after file changes. Never
    // for other runs: pushing is outward-facing and opt-in.
    let change_tx = checkpoints.map(|(path, branch)| {
        let (tx, rx) = mpsc::unbounded_channel::<()>();
        tokio::spawn(checkpoint::run(client.clone(), run_id, path, branch, rx, stop_tx.subscribe(), checkpoint::MIN_INTERVAL));
        tx
    });

    // Forwarder: sends each event immediately; also polls control every second
    // while idle so a stop is noticed even if the agent is silent.
    let forwarder = {
        let client = client.clone();
        let stop_tx = stop_tx.clone();
        tokio::spawn(async move {
            let mut changes = ChangeTracker::default();
            let mut poll = tokio::time::interval(Duration::from_secs(1));
            poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut backoff = Backoff::new(Duration::from_secs(10));
            loop {
                tokio::select! {
                    ev = event_rx.recv() => {
                        let Some(ev) = ev else { break };
                        // Batch whatever else is already waiting; an event with images goes on its own.
                        let has_images = |e: &NewEvent| e.payload.get("images").is_some();
                        let mut batch = vec![ev];
                        while !has_images(&batch[0]) {
                            let Ok(more) = event_rx.try_recv() else { break };
                            let last = has_images(&more);
                            batch.push(more);
                            if last || batch.len() >= 100 { break; }
                        }
                        loop {
                            match client.post_events(run_id, batch.clone()).await {
                                Ok(ctl) => {
                                    backoff.reset();
                                    if ctl.stop_requested || ctl.status != RunStatus::Running {
                                        let _ = stop_tx.send(true);
                                    }
                                    // After the post, so a run moved elsewhere is already stopped here.
                                    if let Some(tx) = &change_tx {
                                        // Every event must be observed (tool_use records what a later result closes).
                                        let mut changed = false;
                                        for e in &batch {
                                            changed |= changes.observe(e);
                                        }
                                        if changed {
                                            let _ = tx.send(());
                                        }
                                    }
                                    break;
                                }
                                Err(e) => {
                                    let wait = backoff.next_delay();
                                    tracing::warn!(error = %e, retry_in = ?wait, "posting events failed");
                                    tokio::time::sleep(wait).await;
                                }
                            }
                        }
                    }
                    _ = poll.tick() => {
                        match client.control(run_id).await {
                            Ok(ctl) if ctl.stop_requested || ctl.status != RunStatus::Running => {
                                let _ = stop_tx.send(true);
                            }
                            Ok(_) => {}
                            Err(e) => tracing::debug!(error = %e, "control poll failed"),
                        }
                    }
                }
            }
        })
    };

    let outcome = match exec.await {
        Ok(o) => o,
        Err(e) => agent::Outcome::new(RunStatus::Failed, json!({"error": format!("executor task panicked: {e}")})),
    };
    // Executor dropped its sender; forwarder drains the channel and exits.
    let _ = forwarder.await;
    drop(bridge);
    if let Some(dir) = images_dir {
        let _ = std::fs::remove_dir_all(dir);
    }

    // What this turn changed, sent before the outcome so it lands while the run is still live.
    if let Some(before) = &before {
        match diff::snapshot(&diff_cwd).await {
            Ok(after) => {
                let mut events = Vec::new();
                if let Some(before_commit) = &before_commit {
                    match snapshot::keep(&diff_cwd, &after, &snapshot::ref_name(root_id, run_id, "after")).await {
                        Ok(c) => events.push(NewEvent { kind: "snapshot".into(), payload: json!({"before": before_commit, "after": c}) }),
                        Err(e) => tracing::warn!(error = %format!("{e:#}"), "keeping the snapshot failed"),
                    }
                }
                if let Some(payload) = diff::diff_event(&diff_cwd, base.as_ref(), before, &after).await {
                    events.push(NewEvent { kind: "diff".into(), payload });
                }
                if !events.is_empty() {
                    if let Err(e) = client.post_events(run_id, events).await {
                        tracing::warn!(error = %e, "posting diff failed");
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "no snapshot after the run"),
        }
    }

    report_finish(client, run_id, outcome.status, outcome.result, outcome.usage_limit).await;
}

/// Listen for the run's permission prompts and relay each to the page.
fn start_approvals(client: &ControlClient, cfg: &WorkerConfig, run_id: Uuid, stop: &watch::Sender<bool>) -> Result<Bridge> {
    anyhow::ensure!(cfg.approval_exe.is_some(), "this worker does not know its own executable");
    let (client, timeout, stop) = (client.clone(), cfg.approval_timeout, stop.subscribe());
    let bridge = Bridge::start(approval::socket_dir(run_id), move |ask| {
        let (client, stop) = (client.clone(), stop.clone());
        async move { approval::ask_page(&client, run_id, &ask, timeout, stop).await }
    })
    .context("listening for permission prompts")?;
    Ok(bridge)
}

/// A restore run: put the thread's worktree back to `run.restore_to`. The
/// restore is snapshotted and diffed like any turn, so it can itself be undone.
async fn run_restore(client: &ControlClient, run: &Run, root_id: Uuid, cwd: &Path, in_worktree: bool) -> Result<String> {
    if !in_worktree {
        anyhow::bail!("refusing to restore files outside the task's own worktree");
    }
    let target = run.restore_to.clone().filter(|c| snapshot::is_commit_id(c)).context("restore run without a snapshot to restore")?;
    let before = diff::point(cwd).await?;
    let base = diff::thread_base(cwd, root_id, &before).await;
    let before_commit = snapshot::keep(cwd, &before.tree, &snapshot::ref_name(root_id, run.id, "before")).await?;
    snapshot::restore(cwd, &target).await.context("restoring the snapshot")?;
    let after = diff::snapshot(cwd).await?;
    let after_commit = snapshot::keep(cwd, &after, &snapshot::ref_name(root_id, run.id, "after")).await?;
    let mut events = vec![NewEvent { kind: "snapshot".into(), payload: json!({"before": before_commit, "after": after_commit}) }];
    if let Some(payload) = diff::diff_event(cwd, base.as_ref(), &before, &after).await {
        events.push(NewEvent { kind: "diff".into(), payload });
    }
    events.push(NewEvent { kind: "restored".into(), payload: json!({"to": target}) });
    client.post_events(run.id, events).await?;
    Ok(target)
}

/// A pull request run: commit and push the thread's branch, open a pull request
/// for it (or find the open one), and tell the control plane about it.
async fn run_pr(client: &ControlClient, run: &Run, root_id: Uuid, cwd: &Path, branch: Option<&str>) -> Result<sidefleet_core::PullRequest> {
    let branch = branch.context("this task has no branch of its own to open a pull request from")?;
    let (pr, opened) = crate::pr::open(cwd, branch, run.context.as_deref().unwrap_or("")).await?;
    client.report_prs(vec![PrReport { thread: root_id, pr: pr.clone() }]).await?;
    client.post_events(run.id, vec![NewEvent { kind: "pull_request".into(), payload: json!({"url": pr.url, "number": pr.number, "state": pr.state, "opened": opened, "branch": branch})}]).await?;
    Ok(pr)
}

/// Look at open pull requests again and report the ones that changed state.
async fn check_prs(client: ControlClient, dir: PathBuf, checks: Vec<PrCheck>) {
    let mut reports = Vec::new();
    for c in checks {
        match crate::pr::view(&dir, &c.url).await {
            Ok(Some(pr)) => reports.push(PrReport { thread: c.thread, pr }),
            Ok(None) => {}
            Err(e) => tracing::debug!(url = %c.url, error = %format!("{e:#}"), "checking a pull request failed"),
        }
    }
    if !reports.is_empty() {
        if let Err(e) = client.report_prs(reports).await {
            tracing::warn!(error = %e, "reporting pull requests failed");
        }
    }
}

async fn report_finish(client: &ControlClient, run_id: Uuid, status: RunStatus, result: serde_json::Value, usage_limit: Option<sidefleet_core::UsageLimit>) {
    let mut backoff = Backoff::new(Duration::from_secs(30));
    for attempt in 1..=10 {
        match client.finish(run_id, status, Some(result.clone()), usage_limit.clone()).await {
            Ok(r) => {
                tracing::info!(run_id = %run_id, status = r.status.as_str(), "run finished");
                return;
            }
            Err(e) => {
                let wait = backoff.next_delay();
                tracing::warn!(error = %e, attempt, retry_in = ?wait, "finish failed");
                tokio::time::sleep(wait).await;
            }
        }
    }
    tracing::error!(run_id = %run_id, "giving up reporting the run outcome");
}

pub fn default_worker_id() -> String {
    let host = gethostname::gethostname().to_string_lossy().to_string();
    let host = host.trim_end_matches(".local").to_string();
    if host.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        host
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_repo;
    use sidefleet_core::RepoInfo;
    use std::path::PathBuf;

    fn r(name: &str, id: &str) -> RepoInfo {
        RepoInfo { name: name.into(), path: format!("/home/u/{name}"), id: id.into() }
    }

    #[test]
    fn resolves_by_path_name_then_identity() {
        let id = "github.com/foodaka/side-fleet";
        let repos = vec![r("code/side-fleet-old-copy", id), r("code/side-fleet", id), r("scratch", "scratch")];
        // exact path name (tasks from before identities existed)
        assert_eq!(resolve_repo(&repos, "code/side-fleet-old-copy"), Some(PathBuf::from("/home/u/code/side-fleet-old-copy")));
        // identity: prefer the clone whose folder is named after the project
        assert_eq!(resolve_repo(&repos, id), Some(PathBuf::from("/home/u/code/side-fleet")));
        assert_eq!(resolve_repo(&repos, "scratch"), Some(PathBuf::from("/home/u/scratch")));
        assert_eq!(resolve_repo(&repos, "github.com/someone/else"), None);
    }

    async fn sh(dir: &std::path::Path, args: &[&str]) -> String {
        crate::checkpoint::git(dir, args, None).await.unwrap()
    }

    async fn commit(dir: &std::path::Path, msg: &str) -> String {
        sh(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--allow-empty", "-m", msg]).await;
        sh(dir, &["rev-parse", "HEAD"]).await
    }

    /// A clone of `origin` whose main is one commit behind it.
    async fn stale_clone(tmp: &std::path::Path) -> (PathBuf, String) {
        let origin = tmp.join("origin");
        sh(tmp, &["init", "--quiet", "-b", "main", origin.to_str().unwrap()]).await;
        commit(&origin, "one").await;
        let repo = tmp.join("repo");
        sh(tmp, &["clone", "--quiet", origin.to_str().unwrap(), repo.to_str().unwrap()]).await;
        let latest = commit(&origin, "two").await;
        (repo, latest)
    }

    #[tokio::test]
    async fn new_task_starts_from_the_latest_default_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, latest) = stale_clone(tmp.path()).await;
        let wt = super::prepare_worktree(&repo, "r", uuid::Uuid::new_v4(), &tmp.path().join("wts"), None, None).await.unwrap();
        let from = wt.from.expect("started from origin");
        assert_eq!((from.reference.as_str(), from.commit.as_str()), ("origin/main", latest.as_str()));
        assert_eq!(sh(&wt.path, &["rev-parse", "HEAD"]).await, latest);
    }

    #[tokio::test]
    async fn unpushed_local_commits_keep_the_task_on_head() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _) = stale_clone(tmp.path()).await;
        let local = commit(&repo, "mine").await;
        let wt = super::prepare_worktree(&repo, "r", uuid::Uuid::new_v4(), &tmp.path().join("wts"), None, None).await.unwrap();
        assert!(wt.from.is_none());
        assert_eq!(sh(&wt.path, &["rev-parse", "HEAD"]).await, local);
    }

    #[tokio::test]
    async fn no_remote_keeps_the_task_on_head() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        sh(tmp.path(), &["init", "--quiet", "-b", "main", repo.to_str().unwrap()]).await;
        let head = commit(&repo, "one").await;
        let wt = super::prepare_worktree(&repo, "r", uuid::Uuid::new_v4(), &tmp.path().join("wts"), None, None).await.unwrap();
        assert!(wt.from.is_none());
        assert_eq!(sh(&wt.path, &["rev-parse", "HEAD"]).await, head);
    }
}
