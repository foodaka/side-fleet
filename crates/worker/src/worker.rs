//! The worker loop: register, heartbeat every 15s, poll for a claimable run,
//! execute it, and report back. Backs off on control plane errors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sidefleet_core::{Capabilities, NewEvent, Run, RunStatus, Slots};
use std::sync::atomic::{AtomicUsize, Ordering};
use anyhow::{Context, Result};
use serde_json::json;
use tokio::sync::{mpsc, watch, Mutex, Semaphore};
use uuid::Uuid;

use crate::agent::{self, AgentSpec};
use crate::client::{Backoff, ClientError, ControlClient};
use crate::detect::{self, DetectOptions};

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub control_url: String,
    pub token: String,
    pub worker_id: String,
    pub display_name: String,
    pub repos_root: PathBuf,
    pub claude_bin: String,
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
}

pub async fn run(cfg: WorkerConfig) -> Result<()> {
    let client = ControlClient::new(&cfg.control_url, &cfg.token, &cfg.worker_id)?;
    let detect_opts = DetectOptions {
        claude_bin: cfg.claude_bin.clone(),
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
    let slots = Arc::new(Semaphore::new(cfg.parallel.max(1)));
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
        }
        // Wait for a free slot before asking for work, so we never claim more than we can run.
        let permit = match slots.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let repos: Vec<String> = caps.lock().await.repos.iter().map(|r| r.name.clone()).collect();
        let busy: Vec<String> = active
            .lock()
            .await
            .values()
            .filter(|(_, iso)| iso == "checkout")
            .map(|(repo, _)| repo.clone())
            .collect();
        match client.claim(&repos, &busy).await {
            Ok(Some(run)) => {
                backoff.reset();
                let repo_path = caps
                    .lock()
                    .await
                    .repos
                    .iter()
                    .find(|r| r.name == run.repo)
                    .map(|r| PathBuf::from(&r.path));
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
                tokio::time::sleep(cfg.poll_interval).await;
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
    match &caps.agents.claude {
        Some(c) => tracing::info!(version = %c.version, signed_in = c.signed_in, error = c.sign_in_error.as_deref().unwrap_or(""), "claude code"),
        None => tracing::warn!("claude code not found"),
    }
    if let Some(c) = &caps.agents.codex {
        tracing::info!(version = %c.version, "codex found");
    }
    if let Some(o) = &caps.agents.ollama {
        tracing::info!(version = %o.version, models = o.models.len(), "ollama found");
    }
    tracing::info!(count = caps.repos.len(), repos = ?caps.repos.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), "git checkouts");
}

/// A per-task git worktree: `<worktrees_dir>/<repo-slug>/<root run id>` on branch
/// `sidefleet/<first 8 of root id>`. Follow-ups reuse the root's worktree so the
/// Claude Code session (keyed by cwd) resumes in the same place.
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

pub async fn prepare_worktree(repo: &Path, repo_name: &str, root_id: Uuid, worktrees_dir: &Path) -> Result<Worktree> {
    let slug: String = repo_name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let path = worktrees_dir.join(slug).join(root_id.to_string());
    let branch = format!("sidefleet/{}", &root_id.simple().to_string()[..8]);
    if path.join(".git").exists() {
        return Ok(Worktree { path, branch });
    }
    std::fs::create_dir_all(path.parent().unwrap()).context("creating worktrees dir")?;
    // Create the branch at HEAD, or reuse it when it already exists (worker restarted).
    let branch_exists = git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).await.is_ok();
    let p = path.to_string_lossy().to_string();
    if branch_exists {
        git(repo, &["worktree", "add", &p, &branch]).await?;
    } else {
        git(repo, &["worktree", "add", "-b", &branch, &p, "HEAD"]).await?;
    }
    Ok(Worktree { path, branch })
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

/// Execute one claimed run: forward events as they arrive, poll for stop, report the outcome.
async fn execute_run(client: &ControlClient, cfg: &WorkerConfig, run: Run, repo_path: Option<PathBuf>) {
    let run_id = run.id;
    tracing::info!(run_id = %run_id, repo = %run.repo, model = %run.model, isolation = %run.isolation, "executing run");

    let Some(repo_dir) = repo_path else {
        let msg = format!("repo {} is no longer present under {}", run.repo, cfg.repos_root.display());
        let _ = client.post_events(run_id, vec![NewEvent { kind: "error".into(), payload: json!({"message": msg}) }]).await;
        let _ = client.finish(run_id, RunStatus::Failed, Some(json!({"error": msg}))).await;
        return;
    };

    // Where the agent works: its own worktree and branch, or the checkout itself.
    let mut pre_events: Vec<NewEvent> = Vec::new();
    let cwd = if run.isolation == "worktree" {
        let root_id = run.parent_id.unwrap_or(run.id);
        match prepare_worktree(&repo_dir, &run.repo, root_id, &cfg.worktrees_dir).await {
            Ok(wt) => {
                pre_events.push(NewEvent {
                    kind: "workspace".into(),
                    payload: json!({"mode": "worktree", "path": wt.path.to_string_lossy(), "branch": wt.branch, "repo": repo_dir.to_string_lossy()}),
                });
                wt.path
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

    let spec = AgentSpec {
        bin: cfg.claude_bin.clone(),
        task: run.task.clone(),
        model: run.model.clone(),
        cwd,
        permission_mode: run.permission_mode.clone().unwrap_or_else(|| cfg.permission_mode.clone()),
        max_turns: cfg.max_turns,
        extra_args: cfg.extra_args.clone(),
        resume_session_id: run.resume_session_id.clone(),
    };

    let (event_tx, mut event_rx) = mpsc::channel::<NewEvent>(256);
    let (stop_tx, stop_rx) = watch::channel(false);

    let exec = tokio::spawn(agent::execute(spec, event_tx, stop_rx));

    // Forwarder: sends each event immediately; also polls control every second
    // while idle so a stop is noticed even if the agent is silent.
    let forwarder = {
        let client = client.clone();
        let stop_tx = stop_tx.clone();
        tokio::spawn(async move {
            let mut poll = tokio::time::interval(Duration::from_secs(1));
            poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut backoff = Backoff::new(Duration::from_secs(10));
            loop {
                tokio::select! {
                    ev = event_rx.recv() => {
                        let Some(ev) = ev else { break };
                        // Batch whatever else is already waiting.
                        let mut batch = vec![ev];
                        while let Ok(more) = event_rx.try_recv() {
                            batch.push(more);
                            if batch.len() >= 100 { break; }
                        }
                        loop {
                            match client.post_events(run_id, batch.clone()).await {
                                Ok(ctl) => {
                                    backoff.reset();
                                    if ctl.stop_requested || ctl.status != RunStatus::Running {
                                        let _ = stop_tx.send(true);
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
        Err(e) => agent::Outcome {
            status: RunStatus::Failed,
            result: serde_json::json!({"error": format!("executor task panicked: {e}")}),
        },
    };
    // Executor dropped its sender; forwarder drains the channel and exits.
    let _ = forwarder.await;

    let mut backoff = Backoff::new(Duration::from_secs(30));
    for attempt in 1..=10 {
        match client.finish(run_id, outcome.status, Some(outcome.result.clone())).await {
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
