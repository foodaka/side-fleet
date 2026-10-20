//! The worker loop: register, heartbeat every 15s, poll for a claimable run,
//! execute it, and report back. Backs off on control plane errors.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ac_core::{Capabilities, NewEvent, Run, RunStatus};
use anyhow::Result;
use tokio::sync::{mpsc, watch, Mutex};
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
    let caps = detect::detect_all(&detect_opts).await;
    log_caps(&caps);
    let caps = Arc::new(Mutex::new(caps));

    register_with_backoff(&client, &cfg, &caps).await;

    // Heartbeat task: re-scans repos each time so new checkouts show up.
    {
        let client = client.clone();
        let caps = caps.clone();
        let interval = cfg.heartbeat_interval;
        let root = cfg.repos_root.clone();
        let depth = cfg.scan_depth;
        let cfg2 = cfg.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                {
                    let mut c = caps.lock().await;
                    c.repos = detect::scan_repos(&root, depth);
                }
                let snapshot = caps.lock().await.clone();
                match client.heartbeat(&snapshot).await {
                    Ok(()) => {}
                    Err(ClientError::NotRegistered) => {
                        tracing::warn!("control plane forgot us; re-registering");
                        register_with_backoff(&client, &cfg2, &caps).await;
                    }
                    Err(e) => tracing::warn!(error = %e, "heartbeat failed"),
                }
            }
        });
    }

    let mut backoff = Backoff::new(Duration::from_secs(30));
    let mut completed = 0usize;
    loop {
        if let Some(max) = cfg.max_runs {
            if completed >= max {
                return Ok(());
            }
        }
        let repos: Vec<String> = caps.lock().await.repos.iter().map(|r| r.name.clone()).collect();
        match client.claim(&repos).await {
            Ok(Some(run)) => {
                backoff.reset();
                let repo_path = caps
                    .lock()
                    .await
                    .repos
                    .iter()
                    .find(|r| r.name == run.repo)
                    .map(|r| PathBuf::from(&r.path));
                execute_run(&client, &cfg, run, repo_path).await;
                completed += 1;
            }
            Ok(None) => {
                backoff.reset();
                tokio::time::sleep(cfg.poll_interval).await;
            }
            Err(ClientError::NotRegistered) => {
                register_with_backoff(&client, &cfg, &caps).await;
            }
            Err(e) => {
                let wait = backoff.next();
                tracing::warn!(error = %e, retry_in = ?wait, "claim failed");
                tokio::time::sleep(wait).await;
            }
        }
    }
}

async fn register_with_backoff(client: &ControlClient, cfg: &WorkerConfig, caps: &Arc<Mutex<Capabilities>>) {
    let mut backoff = Backoff::new(Duration::from_secs(30));
    loop {
        let snapshot = caps.lock().await.clone();
        match client.register(&cfg.display_name, &snapshot).await {
            Ok(w) => {
                tracing::info!(worker = %w.id, control = %cfg.control_url, "registered with control plane");
                return;
            }
            Err(e) => {
                let wait = backoff.next();
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

/// Execute one claimed run: forward events as they arrive, poll for stop, report the outcome.
async fn execute_run(client: &ControlClient, cfg: &WorkerConfig, run: Run, repo_path: Option<PathBuf>) {
    let run_id = run.id;
    tracing::info!(run_id = %run_id, repo = %run.repo, model = %run.model, "executing run");

    let Some(cwd) = repo_path else {
        let msg = format!("repo {} is no longer present under {}", run.repo, cfg.repos_root.display());
        let _ = client.post_events(run_id, vec![NewEvent { kind: "error".into(), payload: serde_json::json!({"message": msg}) }]).await;
        let _ = client.finish(run_id, RunStatus::Failed, Some(serde_json::json!({"error": msg}))).await;
        return;
    };

    let spec = AgentSpec {
        bin: cfg.claude_bin.clone(),
        task: run.task.clone(),
        model: run.model.clone(),
        cwd,
        permission_mode: cfg.permission_mode.clone(),
        max_turns: cfg.max_turns,
        extra_args: cfg.extra_args.clone(),
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
                                    let wait = backoff.next();
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
                let wait = backoff.next();
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
