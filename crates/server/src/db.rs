//! Postgres access. Plain `sqlx::query` (no compile-time macros) so the
//! workspace builds without a database.

use sidefleet_core::{Agent, Capabilities, ImageRef, ImageUpload, PrCheck, PrReport, Run, RunEvent, RunStatus, RunSummary, Slots, UsageLimit, Worker};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

fn row_to_run(r: &PgRow) -> Run {
    let status: String = r.get("status");
    Run {
        id: r.get("id"),
        task: r.get("task"),
        agent: Agent::parse(r.get::<&str, _>("agent")).unwrap_or(Agent::Claude),
        model: r.get("model"),
        effort: r.get("effort"),
        repo: r.get("repo"),
        status: RunStatus::parse(&status).unwrap_or(RunStatus::Failed),
        worker_id: r.get("worker_id"),
        stop_requested: r.get("stop_requested"),
        created_at: r.get("created_at"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
        result: r.get("result"),
        parent_id: r.get("parent_id"),
        session_id: r.get("session_id"),
        resume_session_id: r.get("resume_session_id"),
        pinned_worker_id: r.get("pinned_worker_id"),
        permission_mode: r.get("permission_mode"),
        isolation: r.get("isolation"),
        failover: r.get("failover"),
        moved_from: r.get("moved_from"),
        resume_from_branch: r.get("resume_from_branch"),
        images: serde_json::from_value(r.get("images")).unwrap_or_default(),
        kind: r.get("kind"),
        restore_to: r.get("restore_to"),
        forked_from: r.get("forked_from"),
        fork_commit: r.get("fork_commit"),
        context: r.get("context"),
        archived_at: r.get("archived_at"),
        pr: r.get::<Option<Value>, _>("pr").and_then(|v| serde_json::from_value(v).ok()),
        created_by: r.get("created_by"),
        room_id: r.get("room_id"),
    }
}

fn row_to_event(r: &PgRow) -> RunEvent {
    RunEvent {
        run_id: r.get("run_id"),
        seq: r.get("seq"),
        kind: r.get("kind"),
        payload: r.get("payload"),
        at: r.get("at"),
    }
}

pub async fn set_update_requested(pool: &PgPool, org: Uuid, id: &str, value: bool) -> sqlx::Result<bool> {
    let res = sqlx::query("UPDATE workers SET update_requested = $2 WHERE id = $1 AND org_id = $3")
        .bind(id)
        .bind(value)
        .bind(org)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn update_requested(pool: &PgPool, org: Uuid, id: &str) -> sqlx::Result<bool> {
    let row = sqlx::query("SELECT update_requested FROM workers WHERE id = $1 AND org_id = $2").bind(id).bind(org).fetch_optional(pool).await?;
    Ok(row.map(|r| r.get::<bool, _>("update_requested")).unwrap_or(false))
}

fn row_to_worker(r: &PgRow) -> Worker {
    let caps: Value = r.get("capabilities");
    Worker {
        id: r.get("id"),
        display_name: r.get("display_name"),
        capabilities: serde_json::from_value(caps).unwrap_or_default(),
        last_heartbeat_at: r.get("last_heartbeat_at"),
    }
}

const RUN_COLS: &str =
    "id, task, agent, model, effort, repo, status, worker_id, stop_requested, created_at, started_at, finished_at, result, parent_id, session_id, resume_session_id, pinned_worker_id, permission_mode, isolation, failover, moved_from, resume_from_branch, images, kind, restore_to, forked_from, fork_commit, context, archived_at, pr, created_by, room_id";

/// Append an event inside `tx`. The caller must already hold the run row lock
/// (`SELECT ... FOR UPDATE`) so seq assignment is serialised per run.
pub(crate) async fn append_event_locked(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    kind: &str,
    payload: &Value,
) -> sqlx::Result<RunEvent> {
    let row = sqlx::query(
        "INSERT INTO run_events (run_id, seq, kind, payload)
         VALUES ($1, (SELECT COALESCE(MAX(seq) + 1, 0) FROM run_events WHERE run_id = $1), $2, $3)
         RETURNING run_id, seq, kind, payload, at",
    )
    .bind(run_id)
    .bind(kind)
    .bind(payload)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row_to_event(&row))
}

/// Lock a run row. With `org`, a run of another org is treated as missing.
pub(crate) async fn lock_run(tx: &mut Transaction<'_, Postgres>, org: Option<Uuid>, run_id: Uuid) -> sqlx::Result<Option<Run>> {
    let row = sqlx::query(&format!("SELECT {RUN_COLS} FROM runs WHERE id = $1 AND ($2::uuid IS NULL OR org_id = $2) FOR UPDATE"))
        .bind(run_id)
        .bind(org)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.as_ref().map(row_to_run))
}

fn status_payload(status: RunStatus) -> Value {
    json!({ "status": status.as_str() })
}

/// A task to queue (validated by the route).
pub struct NewRun<'a> {
    pub task: &'a str,
    pub agent: Agent,
    pub model: &'a str,
    pub effort: Option<&'a str>,
    pub repo: &'a str,
    pub permission_mode: Option<&'a str>,
    pub isolation: &'a str,
    pub pinned_worker_id: Option<&'a str>,
    pub failover: bool,
    pub images: &'a [ImageUpload],
    /// Who starts it: only their machines run it.
    pub created_by: Option<Uuid>,
    /// For a room's task: the room, its brief for the agent, and the room branch to start from.
    pub room_id: Option<Uuid>,
    pub context: Option<&'a str>,
    pub resume_from_branch: Option<&'a str>,
}

pub async fn create_run(pool: &PgPool, org: Uuid, new: NewRun<'_>) -> sqlx::Result<(Run, RunEvent)> {
    let NewRun { task, agent, model, effort, repo, permission_mode, isolation, pinned_worker_id, failover, images, created_by, room_id, context, resume_from_branch } = new;
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, permission_mode, isolation, pinned_worker_id, agent, failover, images, org_id, effort,
                           created_by, room_id, context, resume_from_branch)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) RETURNING {RUN_COLS}"
    ))
    .bind(id)
    .bind(task)
    .bind(model)
    .bind(repo)
    .bind(permission_mode)
    .bind(isolation)
    .bind(pinned_worker_id)
    .bind(agent.as_str())
    .bind(failover)
    .bind(json!([]))
    .bind(org)
    .bind(effort)
    .bind(created_by)
    .bind(room_id)
    .bind(context)
    .bind(resume_from_branch)
    .fetch_one(&mut *tx)
    .await?;
    let mut run = row_to_run(&row);
    run.images = attach_uploads(&mut tx, id, images).await?;
    let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "queued", "task": task})).await?;
    tx.commit().await?;
    Ok((run, ev))
}

/// Archive or unarchive the thread `run_id` belongs to. Returns its root, or None if there is no such run.
pub async fn set_archived(pool: &PgPool, org: Uuid, run_id: Uuid, archived: bool) -> sqlx::Result<Option<Run>> {
    let row = sqlx::query(&format!(
        "UPDATE runs SET archived_at = CASE WHEN $2 THEN COALESCE(archived_at, now()) END
         WHERE id = (SELECT COALESCE(parent_id, id) FROM runs WHERE id = $1 AND org_id = $3) RETURNING {RUN_COLS}"
    ))
    .bind(run_id)
    .bind(archived)
    .bind(org)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_run))
}

pub enum FollowUpOutcome {
    Created(Run, RunEvent),
    NotFound,
    /// Already `MAX_QUEUED_MESSAGES` messages waiting in the thread.
    QueueFull,
    /// The previous run never reported a session id (it failed before starting, or was stopped early).
    NoSession(Run),
    /// The model (or effort) isn't one the agent takes.
    BadModel(String),
    /// The thread's machine doesn't have the agent asked to switch to.
    AgentMissing(String),
}

/// Messages that may wait behind a running one in a thread.
pub const MAX_QUEUED_MESSAGES: usize = 10;

/// The session `agent` can continue in a thread: the latest one since the
/// thread last switched to that agent (optionally only one held by `worker`).
/// None when another agent spoke more recently than any session of this one:
/// that session is missing the turns since, so the agent gets a transcript instead.
pub fn session_for(thread: &[Run], agent: Agent, worker: Option<&str>) -> Option<String> {
    thread
        .iter()
        .rev()
        .take_while(|r| r.agent == agent)
        .filter(|r| worker.is_none() || r.worker_id.as_deref() == worker)
        .find_map(|r| r.session_id.clone())
}

/// Continue a thread with a new message. The new run resumes the latest
/// session in the thread and is pinned to the worker that holds it. While an
/// earlier message is still queued or running, the new one waits behind it:
/// its session and machine are settled when it is claimed (`claim_run`).
///
/// With another `agent` than the thread's latest message, the thread switches
/// agents: the new one starts a session of its own in the same worktree, with
/// a transcript of the thread so far (written at claim time, once the messages
/// before it have finished).
#[allow(clippy::too_many_arguments)]
pub async fn create_follow_up(
    pool: &PgPool,
    org: Uuid,
    run_id: Uuid,
    task: &str,
    agent: Option<Agent>,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
    images: &[ImageUpload],
    created_by: Option<Uuid>,
    context: Option<&str>,
) -> sqlx::Result<FollowUpOutcome> {
    let Some(any) = get_run(pool, org, run_id).await? else {
        return Ok(FollowUpOutcome::NotFound);
    };
    let root_id = any.parent_id.unwrap_or(any.id);
    let thread = thread_runs(pool, org, root_id).await?;
    let Some(latest) = thread.last().cloned() else {
        return Ok(FollowUpOutcome::NotFound);
    };
    let agent = agent.unwrap_or(latest.agent);
    // Without a model: the one this agent last used in the thread, else its default.
    let model = match model {
        Some(m) => m.to_string(),
        None => thread.iter().rev().find(|r| r.agent == agent).map(|r| r.model.clone()).unwrap_or_else(|| default_model(agent).into()),
    };
    if let Err(e) = agent.check_model(&model) {
        return Ok(FollowUpOutcome::BadModel(e));
    }
    // Without an effort: the one this agent last used in the thread; empty means its default.
    let effort = match effort {
        Some("") => None,
        Some(e) => Some(e.to_string()),
        None => thread.iter().rev().find(|r| r.agent == agent).and_then(|r| r.effort.clone()),
    };
    if let Some(Err(e)) = effort.as_deref().map(|e| agent.check_effort(e)) {
        return Ok(FollowUpOutcome::BadModel(e));
    }
    let busy = !latest.status.is_terminal();
    if busy && thread.iter().filter(|r| r.status == RunStatus::Queued).count() >= MAX_QUEUED_MESSAGES {
        return Ok(FollowUpOutcome::QueueFull);
    }
    let session = session_for(&thread, agent, None);
    // Another agent has spoken since this one's last session (or it never had one): hand over a transcript.
    let handoff = session.is_none() && thread.iter().any(|r| r.agent != agent);
    if !busy && session.is_none() && !handoff {
        return Ok(FollowUpOutcome::NoSession(latest));
    }
    let machine = thread.iter().rev().find_map(|r| r.worker_id.clone());
    if agent != latest.agent {
        if let Some(w) = &machine {
            if !worker_has_agent(pool, org, w, agent).await? {
                return Ok(FollowUpOutcome::AgentMissing(w.clone()));
            }
        }
    }
    // Behind a busy run the session and machine may still change (or not exist yet): settle them at claim time.
    let session = if busy { None } else { session };
    let worker = if busy { None } else { machine };
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, parent_id, resume_session_id, pinned_worker_id, permission_mode, isolation, agent, failover, images, org_id, effort,
                           created_by, room_id, context)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) RETURNING {RUN_COLS}"
    ))
    .bind(id)
    .bind(task)
    .bind(&model)
    .bind(&latest.repo)
    .bind(root_id)
    .bind(&session)
    .bind(&worker)
    .bind(permission_mode.or(latest.permission_mode.as_deref()))
    .bind(&latest.isolation)
    .bind(agent.as_str())
    .bind(latest.failover)
    .bind(json!([]))
    .bind(org)
    .bind(&effort)
    .bind(created_by)
    .bind(latest.room_id)
    .bind(context)
    .fetch_one(&mut *tx)
    .await?;
    let mut run = row_to_run(&row);
    run.images = attach_uploads(&mut tx, id, images).await?;
    let mut payload = json!({"status": "queued", "task": task, "resumes": session});
    if busy {
        payload["after"] = json!(latest.id);
    }
    if agent != latest.agent {
        payload["switched_from"] = json!(latest.agent);
    }
    let ev = append_event_locked(&mut tx, id, "status", &payload).await?;
    // Writing in an archived thread brings it back.
    sqlx::query("UPDATE runs SET archived_at = NULL WHERE id = $1").bind(root_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(FollowUpOutcome::Created(run, ev))
}

/// The model a switch to `agent` uses when none is given (the web's default too).
fn default_model(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "sonnet",
        Agent::Codex | Agent::Opencode => "",
    }
}

/// Whether a machine can run `agent`. Workers from before multi-agent support report no agents and run Claude Code.
async fn worker_has_agent(pool: &PgPool, org: Uuid, worker: &str, agent: Agent) -> sqlx::Result<bool> {
    let caps = sqlx::query("SELECT capabilities FROM workers WHERE id = $1 AND org_id = $2").bind(worker).bind(org).fetch_optional(pool).await?;
    let caps: Option<Capabilities> = caps.and_then(|r| serde_json::from_value(r.get::<Value, _>("capabilities")).ok());
    Ok(caps.is_some_and(|c| {
        let a = c.agents.runnable();
        a.contains(&agent) || (a.is_empty() && agent == Agent::Claude)
    }))
}

pub enum RestoreOutcome {
    Created(Run, RunEvent),
    NotFound,
    /// The thread's latest run is still queued or running.
    Busy(Run),
    /// The thread works in the checkout itself; restoring would overwrite the developer's files.
    InPlace,
    /// The run has no snapshot (it never started, or its worker predates snapshots).
    NoCheckpoint,
}

/// Queue a restore run that puts the thread's worktree back to how it was
/// before `run_id` started. It runs on the machine holding the snapshot and
/// keeps the thread's session, so the next message continues the conversation.
pub async fn create_restore(pool: &PgPool, org: Uuid, run_id: Uuid, created_by: Option<Uuid>) -> sqlx::Result<RestoreOutcome> {
    let Some(target) = get_run(pool, org, run_id).await? else {
        return Ok(RestoreOutcome::NotFound);
    };
    let root_id = target.parent_id.unwrap_or(target.id);
    let thread = thread_runs(pool, org, root_id).await?;
    let Some(latest) = thread.last().cloned() else {
        return Ok(RestoreOutcome::NotFound);
    };
    if !latest.status.is_terminal() {
        return Ok(RestoreOutcome::Busy(latest));
    }
    if target.isolation != "worktree" {
        return Ok(RestoreOutcome::InPlace);
    }
    let before = snapshot_commit(pool, run_id, "before").await?;
    let (Some(before), Some(worker)) = (before, target.worker_id.clone()) else {
        return Ok(RestoreOutcome::NoCheckpoint);
    };
    let session = session_for(&thread, latest.agent, None);
    let quoted: String = target.task.chars().take(80).collect();
    let task = format!("Restore the files to before “{quoted}{}”", if target.task.chars().count() > 80 { "…" } else { "" });
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, parent_id, session_id, pinned_worker_id, permission_mode, isolation, agent, kind, restore_to, org_id,
                           created_by, room_id)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, 'worktree', $9, 'restore', $10, $11, $12, $13) RETURNING {RUN_COLS}"
    ))
    .bind(id)
    .bind(&task)
    .bind(&latest.model)
    .bind(&latest.repo)
    .bind(root_id)
    .bind(&session)
    .bind(&worker)
    .bind(&latest.permission_mode)
    .bind(latest.agent.as_str())
    .bind(&before)
    .bind(org)
    .bind(created_by)
    .bind(latest.room_id)
    .fetch_one(&mut *tx)
    .await?;
    let run = row_to_run(&row);
    let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "queued", "task": task, "restores": run_id})).await?;
    tx.commit().await?;
    Ok(RestoreOutcome::Created(run, ev))
}

pub enum PrOutcome {
    Created(Run, RunEvent),
    NotFound,
    /// The thread is still queued or running.
    Busy(Run),
    /// The thread ran in the checkout itself, so it has no branch of its own.
    InPlace,
    /// No machine has run it yet.
    NoMachine,
}

/// Queue a `pr` run on the machine with the thread's worktree: it commits what is
/// left, pushes the branch and opens (or updates) the thread's pull request.
pub async fn create_pr_run(pool: &PgPool, org: Uuid, run_id: Uuid, created_by: Option<Uuid>) -> sqlx::Result<PrOutcome> {
    thread_branch_run(pool, org, run_id, "pr", created_by, |root| {
        let task = if root.pr.as_ref().is_some_and(|p| p.state == "open") { "Update the pull request" } else { "Open a pull request" };
        let first_line = root.task.lines().find(|l| !l.trim().is_empty()).unwrap_or("Changes from Sidefleet").trim();
        let title: String = first_line.chars().take(72).collect();
        let title = if first_line.chars().count() > 72 { format!("{}…", title.trim_end()) } else { title };
        let body: String = root.task.chars().take(4000).collect();
        (task.to_string(), format!("{title}\n\n{body}\n\nOpened from Sidefleet."))
    })
    .await
}

/// Queue an `integrate` run on the machine with the thread's worktree: it merges
/// the thread's branch into `room_branch` and pushes that.
pub async fn create_integrate_run(pool: &PgPool, org: Uuid, run_id: Uuid, created_by: Option<Uuid>, room_branch: &str) -> sqlx::Result<PrOutcome> {
    // The task text names the merge commit on the room branch ("Room: <first line>").
    thread_branch_run(pool, org, run_id, "integrate", created_by, |root| {
        let first: String = root.task.lines().find(|l| !l.trim().is_empty()).unwrap_or("a task").trim().chars().take(72).collect();
        (format!("Add to the room: {first}"), room_branch.to_string())
    })
    .await
}

/// A run that works on a finished thread's branch on the machine that has it
/// (`pr`, `integrate`). `make` gives its task text and context from the root.
async fn thread_branch_run(
    pool: &PgPool,
    org: Uuid,
    run_id: Uuid,
    kind: &str,
    created_by: Option<Uuid>,
    make: impl FnOnce(&Run) -> (String, String),
) -> sqlx::Result<PrOutcome> {
    let Some(target) = get_run(pool, org, run_id).await? else {
        return Ok(PrOutcome::NotFound);
    };
    let root_id = target.parent_id.unwrap_or(target.id);
    let thread = thread_runs(pool, org, root_id).await?;
    let (Some(root), Some(latest)) = (thread.first().cloned(), thread.last().cloned()) else {
        return Ok(PrOutcome::NotFound);
    };
    if !latest.status.is_terminal() {
        return Ok(PrOutcome::Busy(latest));
    }
    if root.isolation != "worktree" {
        return Ok(PrOutcome::InPlace);
    }
    let Some(worker) = thread.iter().rev().find_map(|r| r.worker_id.clone()) else {
        return Ok(PrOutcome::NoMachine);
    };
    let (task, context) = make(&root);
    let session = session_for(&thread, latest.agent, None);
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, parent_id, session_id, pinned_worker_id, permission_mode, isolation, agent, kind, context, org_id,
                           created_by, room_id)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, 'worktree', $9, $10, $11, $12, $13, $14) RETURNING {RUN_COLS}"
    ))
    .bind(id)
    .bind(&task)
    .bind(&latest.model)
    .bind(&latest.repo)
    .bind(root_id)
    .bind(&session)
    .bind(&worker)
    .bind(&latest.permission_mode)
    .bind(latest.agent.as_str())
    .bind(kind)
    .bind(&context)
    .bind(org)
    .bind(created_by)
    .bind(root.room_id)
    .fetch_one(&mut *tx)
    .await?;
    let run = row_to_run(&row);
    let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "queued", "task": task})).await?;
    tx.commit().await?;
    Ok(PrOutcome::Created(run, ev))
}

/// Store what a worker found out about threads' pull requests. Only threads in
/// its org; the worker that reports a pull request is the one that checks it later.
pub async fn record_prs(pool: &PgPool, org: Uuid, worker_id: &str, prs: &[PrReport]) -> sqlx::Result<()> {
    for r in prs {
        // Only threads this machine ran: another member's machine can't set the PR link on someone else's task.
        sqlx::query(
            "UPDATE runs SET pr = $3, pr_worker = $4, pr_checked_at = now() WHERE id = $1 AND org_id = $2 AND parent_id IS NULL
               AND EXISTS (SELECT 1 FROM runs t WHERE (t.id = $1 OR t.parent_id = $1) AND t.org_id = $2 AND t.worker_id = $4)",
        )
            .bind(r.thread)
            .bind(org)
            .bind(json!(r.pr))
            .bind(worker_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Open pull requests this worker opened and hasn't checked in a while; marks
/// them checked now so a failing check isn't handed out on every heartbeat.
pub async fn prs_to_check(pool: &PgPool, org: Uuid, worker_id: &str) -> sqlx::Result<Vec<PrCheck>> {
    let rows = sqlx::query(
        "UPDATE runs SET pr_checked_at = now() WHERE id IN (
             SELECT id FROM runs WHERE org_id = $1 AND pr_worker = $2 AND pr->>'state' = 'open'
               AND (pr_checked_at IS NULL OR pr_checked_at < now() - interval '3 minutes')
             ORDER BY pr_checked_at NULLS FIRST LIMIT 20)
         RETURNING id, pr->>'url' AS url",
    )
    .bind(org)
    .bind(worker_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|r| PrCheck { thread: r.get("id"), url: r.get("url") }).collect())
}

/// The commit (`before` or `after`) in a run's latest `snapshot` event, if it looks like one.
async fn snapshot_commit(pool: &PgPool, run_id: Uuid, which: &str) -> sqlx::Result<Option<String>> {
    let row = sqlx::query(
        "SELECT payload FROM run_events WHERE run_id = $1 AND kind = 'snapshot' ORDER BY seq DESC LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .map(|r| r.get::<Value, _>("payload"))
        .and_then(|p| p.get(which).and_then(Value::as_str).map(str::to_string))
        .filter(|c| (40..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit())))
}

pub struct NewFork<'a> {
    pub task: &'a str,
    pub agent: Agent,
    pub model: &'a str,
    pub effort: Option<&'a str>,
    pub permission_mode: Option<&'a str>,
    /// The machine chosen to run it (validated by the route), if any.
    pub worker_id: Option<&'a str>,
    /// Bytes of transcript the task leaves room for.
    pub context_budget: usize,
    /// Who forks it: only their machines run the fork.
    pub created_by: Option<Uuid>,
}

pub enum ForkOutcome {
    Created(Run, RunEvent),
    NotFound,
    /// The message is still queued or running.
    NotFinished(Run),
    /// The fork would run on this machine (the chosen one, or the one with the files), which doesn't have the agent.
    AgentMissing(String),
}

/// Start a new thread from `run_id`: its worktree begins with the files as they
/// were after that message (on the machine that has them), and the agent gets
/// the conversation up to it as a transcript.
pub async fn create_fork(pool: &PgPool, org: Uuid, run_id: Uuid, new: NewFork<'_>) -> sqlx::Result<ForkOutcome> {
    let Some(target) = get_run(pool, org, run_id).await? else {
        return Ok(ForkOutcome::NotFound);
    };
    if !target.status.is_terminal() {
        return Ok(ForkOutcome::NotFinished(target));
    }
    let thread = thread_runs(pool, org, target.parent_id.unwrap_or(target.id)).await?;
    let upto = thread.iter().position(|r| r.id == run_id).map(|i| i + 1).unwrap_or(thread.len());
    let mut history = Vec::with_capacity(upto);
    for r in thread.into_iter().take(upto) {
        let events = events_since(pool, r.id, 0).await?;
        history.push((r, events));
    }
    let context = crate::fork::transcript(&history, new.context_budget);

    let fork_commit = snapshot_commit(pool, run_id, "after").await?;
    // The files come along only on the machine that has them; another chosen machine starts from its HEAD.
    // Someone else's machine never runs this fork (machines run only their owner's runs).
    let files_on = match target.worker_id.as_deref().filter(|_| fork_commit.is_some()) {
        Some(w) if worker_owner(pool, org, w).await? == new.created_by => Some(w),
        _ => None,
    };
    let pinned = match new.worker_id.or(files_on) {
        Some(w) => {
            if !worker_has_agent(pool, org, w, new.agent).await? {
                return Ok(ForkOutcome::AgentMissing(w.to_string()));
            }
            Some(w.to_string())
        }
        None => None,
    };
    let fork_commit = fork_commit.filter(|_| pinned.is_some() && pinned.as_deref() == files_on);

    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, permission_mode, isolation, pinned_worker_id, agent, forked_from, fork_commit, context, org_id, effort,
                           created_by, room_id)
         VALUES ($1, $2, $3, $4, 'queued', $5, 'worktree', $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING {RUN_COLS}"
    ))
    .bind(id)
    .bind(new.task)
    .bind(new.model)
    .bind(&target.repo)
    .bind(new.permission_mode.or(target.permission_mode.as_deref()))
    .bind(&pinned)
    .bind(new.agent.as_str())
    .bind(run_id)
    .bind(&fork_commit)
    .bind(&context)
    .bind(org)
    .bind(new.effort)
    .bind(new.created_by)
    .bind(target.room_id)
    .fetch_one(&mut *tx)
    .await?;
    let run = row_to_run(&row);
    let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "queued", "task": new.task, "forked_from": run_id})).await?;
    tx.commit().await?;
    Ok(ForkOutcome::Created(run, ev))
}

/// The user a machine belongs to (None: no such machine, or one connected with a static token).
pub async fn worker_owner(pool: &PgPool, org: Uuid, worker: &str) -> sqlx::Result<Option<Uuid>> {
    let row = sqlx::query("SELECT owner_id FROM workers WHERE id = $1 AND org_id = $2").bind(worker).bind(org).fetch_optional(pool).await?;
    Ok(row.and_then(|r| r.get("owner_id")))
}

/// Each machine's owner (id and login), by machine id.
pub async fn worker_owners(pool: &PgPool, org: Uuid) -> sqlx::Result<std::collections::HashMap<String, Option<(Uuid, String)>>> {
    let rows = sqlx::query("SELECT w.id, u.id AS owner_id, u.login FROM workers w LEFT JOIN users u ON u.id = w.owner_id WHERE w.org_id = $1")
        .bind(org)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let owner: Option<Uuid> = r.get("owner_id");
            (r.get("id"), owner.map(|o| (o, r.get("login"))))
        })
        .collect())
}

/// All runs of a thread, root first, in order.
pub async fn thread_runs(pool: &PgPool, org: Uuid, root_id: Uuid) -> sqlx::Result<Vec<Run>> {
    let rows = sqlx::query(&format!(
        "SELECT {RUN_COLS} FROM runs WHERE (id = $1 OR parent_id = $1) AND org_id = $2 ORDER BY created_at"
    ))
    .bind(root_id)
    .bind(org)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_run).collect())
}

/// A run of `org`; runs of other orgs read as missing.
pub async fn get_run(pool: &PgPool, org: Uuid, id: Uuid) -> sqlx::Result<Option<Run>> {
    let row = sqlx::query(&format!("SELECT {RUN_COLS} FROM runs WHERE id = $1 AND org_id = $2"))
        .bind(id)
        .bind(org)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_run))
}

/// Root runs (threads), newest activity first, with thread status (the running
/// message if any, else the latest) and follow-up count.
pub async fn list_runs(pool: &PgPool, org: Uuid, limit: i64, archived: bool) -> sqlx::Result<Vec<RunSummary>> {
    let cols = RUN_COLS.split(", ").map(|c| format!("r.{c}")).collect::<Vec<_>>().join(", ");
    let rows = sqlx::query(&format!(
        "SELECT {cols},
                (SELECT count(*) FROM runs f WHERE f.parent_id = r.id) AS follow_ups,
                (SELECT l.status FROM runs l WHERE l.id = r.id OR l.parent_id = r.id ORDER BY (l.status = 'running') DESC, l.created_at DESC LIMIT 1) AS thread_status,
                (SELECT max(greatest(l.created_at, coalesce(l.finished_at, l.created_at))) FROM runs l WHERE l.id = r.id OR l.parent_id = r.id) AS last_activity_at,
                (SELECT count(*) FROM approvals a JOIN runs l ON l.id = a.run_id
                 WHERE (l.id = r.id OR l.parent_id = r.id) AND l.status = 'running' AND a.status = 'pending') AS pending_approvals
         FROM runs r WHERE r.parent_id IS NULL AND r.org_id = $3 AND (r.archived_at IS NOT NULL) = $2 ORDER BY last_activity_at DESC LIMIT $1"
    ))
    .bind(limit)
    .bind(archived)
    .bind(org)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let ts: String = r.get("thread_status");
            let mut run = row_to_run(r);
            // The fork transcript is for the agent; the list doesn't need it.
            run.context = None;
            RunSummary {
                run,
                follow_ups: r.get("follow_ups"),
                thread_status: RunStatus::parse(&ts).unwrap_or(RunStatus::Failed),
                last_activity_at: r.get("last_activity_at"),
                pending_approvals: r.get("pending_approvals"),
            }
        })
        .collect())
}

pub async fn events_since(pool: &PgPool, run_id: Uuid, from_seq: i64) -> sqlx::Result<Vec<RunEvent>> {
    let rows = sqlx::query(
        "SELECT run_id, seq, kind, payload, at FROM run_events
         WHERE run_id = $1 AND seq >= $2 ORDER BY seq",
    )
    .bind(run_id)
    .bind(from_seq)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_event).collect())
}

/// Append worker-reported events. Returns the stored events and the current
/// control state. Events are only accepted while the run is `running` and
/// owned by `worker_id`.
pub enum AppendOutcome {
    Appended(Vec<RunEvent>, Run),
    NotFound,
    NotOwner,
    NotRunning(Run),
}

pub async fn append_events(
    pool: &PgPool,
    org: Uuid,
    run_id: Uuid,
    worker_id: &str,
    events: &[sidefleet_core::NewEvent],
) -> sqlx::Result<AppendOutcome> {
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else {
        return Ok(AppendOutcome::NotFound);
    };
    if run.worker_id.as_deref() != Some(worker_id) {
        return Ok(AppendOutcome::NotOwner);
    }
    if run.status != RunStatus::Running {
        return Ok(AppendOutcome::NotRunning(run));
    }
    let mut stored = Vec::with_capacity(events.len());
    let mut session: Option<String> = None;
    for e in events {
        // Images skip redaction (it would only mangle base64) and go to their own table.
        let mut payload = e.payload.clone();
        let images = payload.as_object_mut().and_then(|o| o.remove("images"));
        let mut payload = sidefleet_core::redact::redact_json(&payload);
        if let Some(Value::Array(images)) = images {
            let mut refs = Vec::new();
            for img in images.iter().take(MAX_IMAGES_PER_EVENT) {
                if let Some(r) = store_image(&mut tx, run_id, img).await? {
                    refs.push(r);
                }
            }
            if !refs.is_empty() {
                payload["images"] = Value::Array(refs);
            }
        }
        if matches!(e.kind.as_str(), "agent_init" | "result") {
            if let Some(sid) = payload.get("session_id").and_then(Value::as_str) {
                session = Some(sid.to_string());
            }
        }
        stored.push(append_event_locked(&mut tx, run_id, &e.kind, &payload).await?);
    }
    if let Some(sid) = session {
        sqlx::query("UPDATE runs SET session_id = $2 WHERE id = $1")
            .bind(run_id)
            .bind(sid)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(AppendOutcome::Appended(stored, run))
}

pub const MAX_IMAGES_PER_EVENT: usize = 8;
/// Decoded size limit for one image.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// Raster formats only: an SVG served from this origin could run script.
pub const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Store one `{media_type, data}` (base64) image and return the reference that
/// replaces it in the event: `{id, media_type, bytes}`. Unusable images are dropped.
async fn store_image(tx: &mut Transaction<'_, Postgres>, run_id: Uuid, img: &Value) -> sqlx::Result<Option<Value>> {
    use base64::Engine;
    let media_type = img.get("media_type").and_then(Value::as_str).unwrap_or("");
    let data = img.get("data").and_then(Value::as_str).unwrap_or("");
    if !IMAGE_TYPES.contains(&media_type) || data.len() > MAX_IMAGE_BYTES / 3 * 4 + 4 {
        tracing::warn!(%run_id, media_type, len = data.len(), "image dropped: unsupported type or too large");
        return Ok(None);
    }
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
        tracing::warn!(%run_id, "image dropped: not valid base64");
        return Ok(None);
    };
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO run_images (id, run_id, media_type, data) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(run_id)
        .bind(media_type)
        .bind(&bytes)
        .execute(&mut **tx)
        .await?;
    Ok(Some(json!({"id": id, "media_type": media_type, "bytes": bytes.len()})))
}

/// Store the images attached to a just-inserted message (validated by the route) and list them on it.
async fn attach_uploads(tx: &mut Transaction<'_, Postgres>, run_id: Uuid, images: &[ImageUpload]) -> sqlx::Result<Vec<ImageRef>> {
    let mut refs = Vec::with_capacity(images.len());
    for img in images {
        if let Some(r) = store_image(tx, run_id, &json!(img)).await? {
            refs.push(serde_json::from_value(r).expect("store_image returns an image reference"));
        }
    }
    if !refs.is_empty() {
        sqlx::query("UPDATE runs SET images = $2 WHERE id = $1").bind(run_id).bind(json!(refs)).execute(&mut **tx).await?;
    }
    Ok(refs)
}

/// An image's media type and bytes, if its run belongs to `org`.
pub async fn get_image(pool: &PgPool, org: Uuid, id: Uuid) -> sqlx::Result<Option<(String, Vec<u8>)>> {
    let row = sqlx::query("SELECT i.media_type, i.data FROM run_images i JOIN runs r ON r.id = i.run_id WHERE i.id = $1 AND r.org_id = $2")
        .bind(id)
        .bind(org)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| (r.get("media_type"), r.get("data"))))
}

/// Request a stop. A queued run becomes `stopped` immediately; a running run
/// gets `stop_requested` set for the worker to act on. Returns the run and any
/// status event to broadcast.
pub async fn request_stop(pool: &PgPool, org: Uuid, run_id: Uuid) -> sqlx::Result<Option<(Run, Option<RunEvent>)>> {
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else {
        return Ok(None);
    };
    let out = match run.status {
        RunStatus::Queued => {
            let row = sqlx::query(&format!(
                "UPDATE runs SET status = 'stopped', stop_requested = true, finished_at = now(),
                 result = $2 WHERE id = $1 RETURNING {RUN_COLS}"
            ))
            .bind(run_id)
            .bind(json!({"reason": "stopped before a worker claimed it"}))
            .fetch_one(&mut *tx)
            .await?;
            let ev =
                append_event_locked(&mut tx, run_id, "status", &status_payload(RunStatus::Stopped))
                    .await?;
            (row_to_run(&row), Some(ev))
        }
        RunStatus::Running => {
            // If nobody can act on the stop (worker offline or gone), finish it here.
            let worker_online: bool = match &run.worker_id {
                Some(w) => sqlx::query("SELECT (last_heartbeat_at > now() - interval '45 seconds') AS online FROM workers WHERE id = $1 AND org_id = $2")
                    .bind(w)
                    .bind(org)
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(|r| r.get::<bool, _>("online"))
                    .unwrap_or(false),
                None => false,
            };
            if !worker_online {
                let row = sqlx::query(&format!(
                    "UPDATE runs SET status = 'stopped', stop_requested = true, finished_at = now(), result = $2
                     WHERE id = $1 RETURNING {RUN_COLS}"
                ))
                .bind(run_id)
                .bind(json!({"reason": "stopped; the worker was offline"}))
                .fetch_one(&mut *tx)
                .await?;
                crate::approvals::cancel_pending(&mut tx, run_id).await?;
                let ev = append_event_locked(&mut tx, run_id, "status", &json!({"status": "stopped", "reason": "worker offline"})).await?;
                tx.commit().await?;
                return Ok(Some((row_to_run(&row), Some(ev))));
            }
            let row = sqlx::query(&format!(
                "UPDATE runs SET stop_requested = true WHERE id = $1 RETURNING {RUN_COLS}"
            ))
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
            let ev = if !run.stop_requested {
                Some(append_event_locked(&mut tx, run_id, "stop_requested", &json!({})).await?)
            } else {
                None
            };
            (row_to_run(&row), ev)
        }
        _ => (run, None),
    };
    tx.commit().await?;
    Ok(Some(out))
}

/// SQL condition: worker `w` could run queued run `r` now. It is online, has
/// the repo (by identity or path name) and the agent, has not hit that agent's
/// usage limit, and is not the machine that lost the run (failover). Workers from before multi-agent support report no
/// agents and run Claude Code. Only machines of the run's own org count.
const CAN_RUN: &str = "w.org_id = r.org_id
      AND w.owner_id IS NOT DISTINCT FROM r.created_by
      AND w.last_heartbeat_at > now() - interval '45 seconds'
      AND NOT (w.id = ANY(r.excluded_worker_ids))
      AND EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(w.capabilities->'repos', '[]'::jsonb)) AS info(repo)
                  WHERE info.repo->>'name' = r.repo OR info.repo->>'id' = r.repo)
      AND (w.capabilities->'agents' ? r.agent
           OR (r.agent = 'claude' AND COALESCE(w.capabilities->'agents', '{}'::jsonb) - 'ollama' = '{}'::jsonb))
      AND NOT EXISTS (SELECT 1 FROM worker_limits l WHERE l.org_id = w.org_id AND l.worker_id = w.id AND l.agent = r.agent AND l.until > now())";

/// How long a new run waits for the least busy machine before any machine that
/// can run it may take it (that machine may have the repo busy, or be slow to poll).
const LEAST_BUSY_GRACE_SECS: i64 = 10;

/// What a claim produced: the run to execute, plus queued messages that were
/// failed on the way because there was no session left to continue.
pub struct Claimed {
    pub run: Option<(Run, RunEvent)>,
    pub failed: Vec<RunEvent>,
}

/// Atomically claim the oldest queued run of `org` for one of `repos`, unless
/// the org already has `max_running` runs in progress (its plan's parallel limit).
///
/// Least busy first: claims are pulled, so the first machine to ask would win.
/// When `slots` says how loaded the asking machine is, an unpinned run younger
/// than [`LEAST_BUSY_GRACE_SECS`] is skipped while another machine that can run
/// it has more free slots. Runs of an agent whose usage limit this machine hit
/// are skipped until the limit resets.
///
/// A message is only claimable once every earlier message of its thread has
/// finished (and any run moved off a lost machine, which takes the place of the
/// message it replaces), and only by the machine that ran the thread last (where
/// the session lives); one queued behind a busy run resumes the latest session
/// there. A moved run itself is exempt: it starts afresh on another machine.
#[allow(clippy::too_many_arguments)]
pub async fn claim_run(
    pool: &PgPool,
    org: Uuid,
    worker_id: &str,
    repos: &[String],
    busy_repos: &[String],
    agents: &[String],
    slots: Option<&Slots>,
    kinds: &[String],
    max_running: Option<i64>,
) -> sqlx::Result<Claimed> {
    let free = slots.map(|s| s.parallel.saturating_sub(s.active) as i32);
    if let Some(s) = slots {
        sqlx::query("UPDATE workers SET slots_parallel = $2, slots_active = $3 WHERE id = $1 AND org_id = $4")
            .bind(worker_id)
            .bind(s.parallel as i32)
            .bind(s.active as i32)
            .bind(org)
            .execute(pool)
            .await?;
    }
    let mut failed = Vec::new();
    // Each pass either claims a run, finds none, or fails one unresumable message and looks again.
    for _ in 0..=MAX_QUEUED_MESSAGES {
        let mut tx = pool.begin().await?;
        if let Some(max) = max_running {
            // Serialise claims within the org so two workers cannot both take the last slot.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))").bind(org.to_string()).execute(&mut *tx).await?;
            let running: i64 = sqlx::query("SELECT count(*) AS n FROM runs WHERE org_id = $1 AND status = 'running'")
                .bind(org)
                .fetch_one(&mut *tx)
                .await?
                .get("n");
            if running >= max {
                tx.commit().await?;
                return Ok(Claimed { run: None, failed });
            }
        }
        let row = sqlx::query(&format!(
            "UPDATE runs SET status = 'running', worker_id = $1, started_at = now()
             WHERE id = (
                SELECT r.id FROM runs r WHERE r.status = 'queued' AND r.org_id = $9 AND r.repo = ANY($2)
                  AND NOT (r.repo = ANY($3))
                  AND r.created_by IS NOT DISTINCT FROM (SELECT o.owner_id FROM workers o WHERE o.org_id = $9 AND o.id = $1)
                  AND COALESCE(r.pinned_worker_id,
                        (SELECT p.worker_id FROM runs p
                          WHERE r.parent_id IS NOT NULL AND r.moved_from IS NULL
                            AND (p.id = r.parent_id OR p.parent_id = r.parent_id) AND p.worker_id IS NOT NULL
                          ORDER BY p.created_at DESC LIMIT 1),
                        $1) = $1
                  AND NOT EXISTS (
                        SELECT 1 FROM runs p
                         WHERE r.parent_id IS NOT NULL AND r.moved_from IS NULL
                           AND (p.id = r.parent_id OR p.parent_id = r.parent_id)
                           AND p.id <> r.id AND (p.created_at < r.created_at OR p.moved_from IS NOT NULL)
                           AND p.status IN ('queued', 'running'))
                  AND (NOT ($1 = ANY(r.excluded_worker_ids)) OR r.created_at < now() - make_interval(secs => $7))
                  AND r.agent = ANY($4)
                  AND r.kind = ANY($8)
                  AND (r.forked_from IS NULL OR 'fork' = ANY($8))
                  AND NOT EXISTS (SELECT 1 FROM worker_limits l WHERE l.org_id = $9 AND l.worker_id = $1 AND l.agent = r.agent AND l.until > now())
                  AND ($5::int IS NULL OR r.pinned_worker_id IS NOT NULL
                       OR (r.parent_id IS NOT NULL AND r.moved_from IS NULL)
                       OR r.created_at < now() - make_interval(secs => $6)
                       OR NOT EXISTS (SELECT 1 FROM workers w WHERE w.id <> $1 AND {CAN_RUN}
                                      AND w.slots_parallel - w.slots_active > $5))
                ORDER BY r.created_at LIMIT 1 FOR UPDATE OF r SKIP LOCKED
             )
             RETURNING {RUN_COLS}"
        ))
        .bind(worker_id)
        .bind(repos)
        .bind(busy_repos)
        .bind(agents)
        .bind(free)
        .bind(LEAST_BUSY_GRACE_SECS as f64)
        .bind(EXCLUSION_GRACE_SECS as f64)
        .bind(kinds)
        .bind(org)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(Claimed { run: None, failed });
        };
        let mut run = row_to_run(&row);
        // (A restore carries the thread's session itself and starts no agent.)
        if let (Some(root), None, None, "agent") = (run.parent_id, &run.resume_session_id, &run.moved_from, run.kind.as_str()) {
            // A message that waited behind another, or one that switches agents: continue this agent's
            // latest session on this machine, or hand it a transcript when another agent spoke since.
            // (A run moved to another machine is created after the messages waiting behind it.)
            let earlier: Vec<Run> = thread_runs(pool, org, root)
                .await?
                .into_iter()
                .filter(|r| r.id != run.id && (r.created_at < run.created_at || r.moved_from.is_some()))
                .collect();
            let session = session_for(&earlier, run.agent, Some(worker_id));
            let handoff = session.is_none() && earlier.iter().any(|r| r.agent != run.agent);
            let fail = match (session, handoff) {
                (Some(sid), _) => {
                    sqlx::query("UPDATE runs SET resume_session_id = $2, pinned_worker_id = $3 WHERE id = $1")
                        .bind(run.id)
                        .bind(&sid)
                        .bind(worker_id)
                        .execute(&mut *tx)
                        .await?;
                    run.resume_session_id = Some(sid);
                    run.pinned_worker_id = Some(worker_id.to_string());
                    None
                }
                // Workers before forks don't pass `context` to the agent.
                (None, true) if !kinds.iter().any(|k| k == "fork") => {
                    Some("this machine's sidefleet is too old to switch agents in a thread; update it and send the message again")
                }
                (None, true) => {
                    let mut history = Vec::with_capacity(earlier.len());
                    for r in earlier {
                        let events = events_since(pool, r.id, 0).await?;
                        history.push((r, events));
                    }
                    let context = crate::fork::transcript(&history, crate::fork::budget(&run.task));
                    sqlx::query("UPDATE runs SET context = $2, pinned_worker_id = $3 WHERE id = $1")
                        .bind(run.id)
                        .bind(&context)
                        .bind(worker_id)
                        .execute(&mut *tx)
                        .await?;
                    run.context = Some(context);
                    run.pinned_worker_id = Some(worker_id.to_string());
                    None
                }
                (None, false) => Some("the earlier message ended before the agent started a session, so there is nothing to continue"),
            };
            if let Some(reason) = fail {
                sqlx::query("UPDATE runs SET status = 'failed', worker_id = NULL, started_at = NULL, finished_at = now(), result = $2 WHERE id = $1")
                    .bind(run.id)
                    .bind(json!({"error": reason}))
                    .execute(&mut *tx)
                    .await?;
                let ev = append_event_locked(&mut tx, run.id, "status", &json!({"status": "failed", "reason": reason})).await?;
                tx.commit().await?;
                failed.push(ev);
                continue;
            }
        }
        // One slot fewer free here until the next heartbeat or claim says otherwise.
        sqlx::query("UPDATE workers SET slots_active = slots_active + 1 WHERE id = $1 AND org_id = $2 AND slots_active IS NOT NULL")
            .bind(worker_id)
            .bind(org)
            .execute(&mut *tx)
            .await?;
        let ev = append_event_locked(
            &mut tx,
            run.id,
            "status",
            &json!({"status": "running", "worker_id": worker_id}),
        )
        .await?;
        tx.commit().await?;
        return Ok(Claimed { run: Some((run, ev)), failed });
    }
    Ok(Claimed { run: None, failed })
}

pub enum FinishOutcome {
    /// Finished; the events to broadcast (a `usage_limit` event, then the status).
    Finished(Run, Vec<RunEvent>),
    /// The run hit this machine's usage limit and went back in the queue for another machine.
    Requeued(Run, Vec<RunEvent>),
    NotFound,
    NotOwner,
    AlreadyFinished(Run),
}

/// How long a machine is left alone after a usage limit with no reset time.
const DEFAULT_LIMIT_SECS: i64 = 3600;

#[allow(clippy::too_many_arguments)]
pub async fn finish_run(
    pool: &PgPool,
    org: Uuid,
    run_id: Uuid,
    worker_id: &str,
    status: RunStatus,
    result: Option<Value>,
    usage_limit: Option<&UsageLimit>,
) -> sqlx::Result<FinishOutcome> {
    debug_assert!(status.is_terminal());
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else {
        return Ok(FinishOutcome::NotFound);
    };
    if run.worker_id.as_deref() != Some(worker_id) {
        return Ok(FinishOutcome::NotOwner);
    }
    if run.status != RunStatus::Running {
        return Ok(FinishOutcome::AlreadyFinished(run));
    }
    // Whatever the agent was still waiting on can no longer be answered.
    let mut events = crate::approvals::cancel_pending(&mut tx, run_id).await?;
    if let (Some(limit), RunStatus::Failed) = (usage_limit, status) {
        // The reset time the agent reported, if it is ahead of us; otherwise an hour.
        let now = Utc::now();
        let until = limit
            .resets_at
            .filter(|t| *t > now && *t < now + chrono::Duration::days(8))
            .unwrap_or(now + chrono::Duration::seconds(DEFAULT_LIMIT_SECS));
        sqlx::query(
            "INSERT INTO worker_limits (worker_id, agent, until, org_id) VALUES ($1, $2, $3, $4)
             ON CONFLICT (org_id, worker_id, agent) DO UPDATE SET until = EXCLUDED.until",
        )
        .bind(worker_id)
        .bind(run.agent.as_str())
        .bind(until)
        .bind(org)
        .execute(&mut *tx)
        .await?;
        events.push(
            append_event_locked(&mut tx, run_id, "usage_limit", &json!({"worker_id": worker_id, "agent": run.agent.as_str(), "until": until}))
                .await?,
        );
        // Once, and only when it may run anywhere and another machine can take it now.
        let requeue = run.pinned_worker_id.is_none()
            && sqlx::query(&format!(
                "SELECT r.requeues = 0 AND EXISTS (SELECT 1 FROM workers w WHERE w.id <> $2 AND {CAN_RUN}) AS ok
                 FROM runs r WHERE r.id = $1"
            ))
            .bind(run_id)
            .bind(worker_id)
            .fetch_one(&mut *tx)
            .await?
            .get::<bool, _>("ok");
        if requeue {
            let row = sqlx::query(&format!(
                "UPDATE runs SET status = 'queued', worker_id = NULL, started_at = NULL, session_id = NULL,
                 requeues = requeues + 1 WHERE id = $1 RETURNING {RUN_COLS}"
            ))
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
            events.push(
                append_event_locked(
                    &mut tx,
                    run_id,
                    "status",
                    &json!({"status": "queued", "reason": "usage limit", "requeued_from": worker_id}),
                )
                .await?,
            );
            tx.commit().await?;
            return Ok(FinishOutcome::Requeued(row_to_run(&row), events));
        }
    }
    let result = result.map(|r| sidefleet_core::redact::redact_json(&r));
    let row = sqlx::query(&format!(
        "UPDATE runs SET status = $2, finished_at = now(), result = $3 WHERE id = $1 RETURNING {RUN_COLS}"
    ))
    .bind(run_id)
    .bind(status.as_str())
    .bind(result)
    .fetch_one(&mut *tx)
    .await?;
    events.push(append_event_locked(&mut tx, run_id, "status", &status_payload(status)).await?);
    tx.commit().await?;
    Ok(FinishOutcome::Finished(row_to_run(&row), events))
}

/// Runs assigned to `worker_id` that are `running` but not in `active`: the
/// worker lost them (restart, crash). Mark them finished so stops and replies
/// are not left waiting forever. A run that had a stop pending becomes
/// `stopped`; a failover run is moved to another machine; otherwise `failed`.
pub async fn reconcile_worker_runs(pool: &PgPool, org: Uuid, worker_id: &str, active: &[Uuid]) -> sqlx::Result<Vec<RunEvent>> {
    let rows = sqlx::query("SELECT id FROM runs WHERE status = 'running' AND worker_id = $1 AND org_id = $3 AND NOT (id = ANY($2))")
        .bind(worker_id)
        .bind(active)
        .bind(org)
        .fetch_all(pool)
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let id: Uuid = row.get("id");
        let mut tx = pool.begin().await?;
        let Some(run) = lock_run(&mut tx, Some(org), id).await? else { continue };
        if run.status != RunStatus::Running || active.contains(&id) {
            continue;
        }
        out.extend(crate::approvals::cancel_pending(&mut tx, id).await?);
        let reason = format!("the worker {worker_id} restarted while this run was in progress");
        if let Some(evs) = move_lost_run(&mut tx, &run, worker_id, &reason).await? {
            tx.commit().await?;
            out.extend(evs);
            continue;
        }
        let status = if run.stop_requested { RunStatus::Stopped } else { RunStatus::Failed };
        sqlx::query("UPDATE runs SET status = $2, finished_at = now(), result = $3 WHERE id = $1")
            .bind(id)
            .bind(status.as_str())
            .bind(json!({"error": reason}))
            .execute(&mut *tx)
            .await?;
        let ev = append_event_locked(&mut tx, id, "status", &json!({"status": status.as_str(), "reason": "worker restarted"})).await?;
        tx.commit().await?;
        out.push(ev);
    }
    Ok(out)
}

/// Fail running runs whose worker has not heart-beaten for `stale_secs`; a
/// failover run is moved to another machine instead.
pub async fn reap_orphaned_runs(pool: &PgPool, stale_secs: i64) -> sqlx::Result<Vec<RunEvent>> {
    let rows = sqlx::query(
        "SELECT r.id FROM runs r JOIN workers w ON w.id = r.worker_id AND w.org_id = r.org_id
         WHERE r.status = 'running' AND w.last_heartbeat_at < now() - make_interval(secs => $1)",
    )
    .bind(stale_secs as f64)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        let id: Uuid = row.get("id");
        let mut tx = pool.begin().await?;
        let Some(run) = lock_run(&mut tx, None, id).await? else { continue };
        if run.status != RunStatus::Running {
            continue;
        }
        out.extend(crate::approvals::cancel_pending(&mut tx, id).await?);
        let reason = "worker stopped heart-beating while the run was in progress";
        let worker = run.worker_id.clone().unwrap_or_default();
        if let Some(evs) = move_lost_run(&mut tx, &run, &worker, reason).await? {
            tx.commit().await?;
            out.extend(evs);
            continue;
        }
        sqlx::query(
            "UPDATE runs SET status = 'failed', finished_at = now(), result = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(json!({"error": reason}))
        .execute(&mut *tx)
        .await?;
        let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "failed", "reason": "worker lost"})).await?;
        tx.commit().await?;
        out.push(ev);
    }
    Ok(out)
}

/// How long the machine that lost a run is kept from claiming its replacement.
/// After this, with no other machine having taken it, the original may continue
/// it (a single laptop that slept should not leave the task queued forever).
pub const EXCLUSION_GRACE_SECS: i64 = 600;

/// Size cap of the thread digest written into a moved run's task.
const DIGEST_MAX_BYTES: usize = 8 * 1024;

/// Move a lost failover run (row locked by the caller): mark it failed with
/// reason "worker lost, moved" and queue a replacement in the same thread that
/// any other machine may claim, continuing from the last pushed checkpoint.
/// Returns None (nothing done) unless the run opted in, was not itself a move,
/// and had no stop pending. Returns the events to broadcast.
async fn move_lost_run(
    tx: &mut Transaction<'_, Postgres>,
    run: &Run,
    lost_worker: &str,
    why: &str,
) -> sqlx::Result<Option<Vec<RunEvent>>> {
    if !run.failover || run.moved_from.is_some() || run.stop_requested {
        return Ok(None);
    }
    let root_id = run.parent_id.unwrap_or(run.id);
    // The latest pushed checkpoint anywhere in the thread (they share one branch).
    let checkpoint: Option<Value> = sqlx::query(
        "SELECT e.payload FROM run_events e JOIN runs r ON r.id = e.run_id
         WHERE (r.id = $1 OR r.parent_id = $1) AND e.kind = 'checkpoint' AND e.payload ? 'sha' AND e.payload ? 'branch'
         ORDER BY e.at DESC, e.seq DESC LIMIT 1",
    )
    .bind(root_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(|r| r.get("payload"));
    let branch = checkpoint.as_ref().and_then(|c| c["branch"].as_str()).map(str::to_string);
    let sha = checkpoint.as_ref().and_then(|c| c["sha"].as_str()).unwrap_or("").to_string();

    let digest = thread_digest(tx, root_id, run.id).await?;
    let mut task = format!(
        "{}\n\n---\nSidefleet moved this task to this machine: the machine running it ({lost_worker}) stopped responding.",
        run.task
    );
    if !digest.is_empty() {
        task.push_str(&format!(" What happened so far:\n\n{digest}\n"));
    }
    match &branch {
        Some(b) => task.push_str(&format!("\nContinue from branch {b} at {sha}; it has your work so far.")),
        None => task.push_str("\nNo work was pushed before the machine dropped; start from the repository as it is."),
    }

    let new_id = Uuid::new_v4();
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, parent_id, permission_mode, isolation, agent,
                           failover, moved_from, resume_from_branch, excluded_worker_ids, images, org_id, effort, created_by, room_id)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, true, $9, $10, ARRAY[$11], $12,
                 (SELECT org_id FROM runs WHERE id = $9), $13, $14, $15) RETURNING {RUN_COLS}"
    ))
    .bind(new_id)
    .bind(&task)
    .bind(&run.model)
    .bind(&run.repo)
    .bind(root_id)
    .bind(&run.permission_mode)
    .bind(&run.isolation)
    .bind(run.agent.as_str())
    .bind(run.id)
    .bind(branch.as_ref().filter(|_| run.isolation == "worktree"))
    .bind(lost_worker)
    .bind(json!(run.images))
    .bind(&run.effort)
    .bind(run.created_by)
    .bind(run.room_id)
    .fetch_one(&mut **tx)
    .await?;
    let moved = row_to_run(&row);

    sqlx::query("UPDATE runs SET status = 'failed', finished_at = now(), result = $2 WHERE id = $1")
        .bind(run.id)
        .bind(json!({"error": format!("{why}; moved to another machine"), "moved_to": new_id}))
        .execute(&mut **tx)
        .await?;
    let lost_ev = append_event_locked(
        tx,
        run.id,
        "status",
        &json!({"status": "failed", "reason": "worker lost, moved", "moved_to": new_id, "lost_worker": lost_worker}),
    )
    .await?;
    let queued_ev = append_event_locked(
        tx,
        new_id,
        "status",
        &json!({"status": "queued", "moved_from": run.id, "lost_worker": lost_worker, "branch": branch, "sha": sha}),
    )
    .await?;
    tracing::info!(lost = %run.id, moved = %moved.id, worker = lost_worker, branch = ?moved.resume_from_branch, "moved a lost run");
    Ok(Some(vec![lost_ev, queued_ev]))
}

/// A short plain-text account of a thread for a run that continues it without
/// the agent's session: each message, what the agent said and the tools it
/// used. The lost run's own message is left out (it heads the new task). Keeps
/// the most recent part when over the cap.
async fn thread_digest(tx: &mut Transaction<'_, Postgres>, root_id: Uuid, lost_id: Uuid) -> sqlx::Result<String> {
    let runs = sqlx::query("SELECT id, task FROM runs WHERE id = $1 OR parent_id = $1 ORDER BY created_at")
        .bind(root_id)
        .fetch_all(&mut **tx)
        .await?;
    let mut lines: Vec<String> = Vec::new();
    for r in &runs {
        let id: Uuid = r.get("id");
        if id != lost_id {
            lines.push(format!("User: {}", clip(r.get::<&str, _>("task"), 600)));
        }
        let evs = sqlx::query(
            "SELECT kind, payload FROM run_events WHERE run_id = $1 AND kind IN ('assistant_text', 'tool_use') ORDER BY seq",
        )
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
        for e in evs {
            let p: Value = e.get("payload");
            match e.get::<&str, _>("kind") {
                "assistant_text" => lines.push(format!("Assistant: {}", clip(p["text"].as_str().unwrap_or(""), 600))),
                _ => lines.push(format!(
                    "- {} {}",
                    p["name"].as_str().unwrap_or("tool"),
                    clip(p["summary"].as_str().unwrap_or(""), 200)
                )),
            }
        }
    }
    // Newest lines first until the budget runs out.
    let marker = "(earlier steps omitted)";
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0;
    for l in lines.iter().rev() {
        if used + l.len() + 1 > DIGEST_MAX_BYTES - marker.len() - 1 {
            kept.push(marker);
            break;
        }
        used += l.len() + 1;
        kept.push(l);
    }
    kept.reverse();
    Ok(kept.join("\n"))
}

/// At most `max` bytes on a char boundary, on one line, with an ellipsis when cut.
fn clip(s: &str, max: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// ---- workers ----

pub async fn upsert_worker(
    pool: &PgPool,
    org: Uuid,
    id: &str,
    display_name: &str,
    token_hash: &str,
    caps: &Capabilities,
) -> sqlx::Result<Option<Worker>> {
    let row = sqlx::query(
        "INSERT INTO workers (id, display_name, token_hash, capabilities, last_heartbeat_at, slots_parallel, slots_active, org_id, owner_id)
         VALUES ($1, $2, $3, $4, now(), $5, $6, $7, (SELECT t.owner_id FROM worker_tokens t WHERE t.token_hash = $3 AND t.org_id = $7))
         ON CONFLICT (org_id, id) DO UPDATE SET display_name = EXCLUDED.display_name, owner_id = EXCLUDED.owner_id,
            token_hash = EXCLUDED.token_hash, capabilities = EXCLUDED.capabilities,
            last_heartbeat_at = now(), update_requested = false,
            slots_parallel = EXCLUDED.slots_parallel, slots_active = EXCLUDED.slots_active
         -- A machine id registered by one person can't be taken over with another person's token.
         WHERE workers.owner_id IS NOT DISTINCT FROM EXCLUDED.owner_id
         RETURNING id, display_name, capabilities, last_heartbeat_at",
    )
    .bind(id)
    .bind(display_name)
    .bind(token_hash)
    .bind(serde_json::to_value(caps).unwrap_or(Value::Null))
    .bind(caps.slots.as_ref().map(|s| s.parallel as i32))
    .bind(caps.slots.as_ref().map(|s| s.active as i32))
    .bind(org)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_worker))
}

/// Returns false when the worker is unknown or registered with a different token.
pub async fn heartbeat(
    pool: &PgPool,
    org: Uuid,
    id: &str,
    token_hash: &str,
    caps: &Capabilities,
) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE workers SET capabilities = $3, last_heartbeat_at = now(),
            slots_parallel = $4, slots_active = $5
         WHERE id = $1 AND token_hash = $2 AND org_id = $6",
    )
    .bind(id)
    .bind(token_hash)
    .bind(serde_json::to_value(caps).unwrap_or(Value::Null))
    .bind(caps.slots.as_ref().map(|s| s.parallel as i32))
    .bind(caps.slots.as_ref().map(|s| s.active as i32))
    .bind(org)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn worker_owns_token(pool: &PgPool, org: Uuid, id: &str, token_hash: &str) -> sqlx::Result<bool> {
    let row = sqlx::query("SELECT 1 AS ok FROM workers WHERE id = $1 AND token_hash = $2 AND org_id = $3")
        .bind(id)
        .bind(token_hash)
        .bind(org)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn worker_exists(pool: &PgPool, org: Uuid, id: &str) -> sqlx::Result<bool> {
    let row = sqlx::query("SELECT 1 AS ok FROM workers WHERE id = $1 AND org_id = $2").bind(id).bind(org).fetch_optional(pool).await?;
    Ok(row.is_some())
}

pub async fn count_workers(pool: &PgPool, org: Uuid) -> sqlx::Result<i64> {
    Ok(sqlx::query("SELECT count(*) AS n FROM workers WHERE org_id = $1").bind(org).fetch_one(pool).await?.get("n"))
}

/// Forget a machine. Its runs stay (without a machine); a worker still holding a
/// valid token re-registers on its next heartbeat.
pub async fn delete_worker(pool: &PgPool, org: Uuid, id: &str) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE runs SET worker_id = NULL WHERE org_id = $1 AND worker_id = $2 AND status <> 'running'")
        .bind(org)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let busy = sqlx::query("SELECT 1 AS ok FROM runs WHERE org_id = $1 AND worker_id = $2").bind(org).bind(id).fetch_optional(&mut *tx).await?;
    if busy.is_some() {
        // A run is still in progress there; keep the machine until it ends.
        return Ok(false);
    }
    let res = sqlx::query("DELETE FROM workers WHERE org_id = $1 AND id = $2").bind(org).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(res.rows_affected() == 1)
}

/// Queued plus running runs, for the plan's parallel limit.
pub async fn count_active_runs(pool: &PgPool, org: Uuid) -> sqlx::Result<i64> {
    Ok(sqlx::query("SELECT count(*) AS n FROM runs WHERE org_id = $1 AND status IN ('queued', 'running')")
        .bind(org)
        .fetch_one(pool)
        .await?
        .get("n"))
}

pub async fn list_workers(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<Worker>> {
    let rows = sqlx::query(
        "SELECT id, display_name, capabilities, last_heartbeat_at, update_requested FROM workers WHERE org_id = $1 ORDER BY display_name",
    )
    .bind(org)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_worker).collect())
}

/// Agents each machine may not use until their usage limit resets: worker id -> agent -> until.
pub async fn active_limits(pool: &PgPool, org: Uuid) -> sqlx::Result<std::collections::HashMap<String, serde_json::Map<String, Value>>> {
    let rows = sqlx::query("SELECT worker_id, agent, until FROM worker_limits WHERE org_id = $1 AND until > now()").bind(org).fetch_all(pool).await?;
    let mut out: std::collections::HashMap<String, serde_json::Map<String, Value>> = Default::default();
    for r in rows {
        let until: DateTime<Utc> = r.get("until");
        out.entry(r.get("worker_id")).or_default().insert(r.get("agent"), json!(until));
    }
    Ok(out)
}

pub fn is_online(last_heartbeat_at: DateTime<Utc>) -> bool {
    Utc::now().signed_duration_since(last_heartbeat_at).num_seconds() < 45
}

pub async fn list_added_repos(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<String>> {
    let rows = sqlx::query("SELECT name FROM added_repos WHERE org_id = $1 ORDER BY name").bind(org).fetch_all(pool).await?;
    Ok(rows.iter().map(|r| r.get("name")).collect())
}

/// Replace the whole added set in one transaction.
pub async fn set_added_repos(pool: &PgPool, org: Uuid, names: &[String]) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM added_repos WHERE org_id = $2 AND NOT (name = ANY($1))").bind(names).bind(org).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO added_repos (org_id, name) SELECT $2, unnest($1::text[]) ON CONFLICT DO NOTHING")
        .bind(names)
        .bind(org)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
