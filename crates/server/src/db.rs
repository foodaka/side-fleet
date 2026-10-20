//! Postgres access. Plain `sqlx::query` (no compile-time macros) so the
//! workspace builds without a database.

use sidefleet_core::{Capabilities, Run, RunEvent, RunStatus, RunSummary, Worker};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

fn row_to_run(r: &PgRow) -> Run {
    let status: String = r.get("status");
    Run {
        id: r.get("id"),
        task: r.get("task"),
        model: r.get("model"),
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
    "id, task, model, repo, status, worker_id, stop_requested, created_at, started_at, finished_at, result, parent_id, session_id, resume_session_id, pinned_worker_id, permission_mode, isolation";

/// Append an event inside `tx`. The caller must already hold the run row lock
/// (`SELECT ... FOR UPDATE`) so seq assignment is serialised per run.
async fn append_event_locked(
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

async fn lock_run(tx: &mut Transaction<'_, Postgres>, run_id: Uuid) -> sqlx::Result<Option<Run>> {
    let row = sqlx::query(&format!("SELECT {RUN_COLS} FROM runs WHERE id = $1 FOR UPDATE"))
        .bind(run_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.as_ref().map(row_to_run))
}

fn status_payload(status: RunStatus) -> Value {
    json!({ "status": status.as_str() })
}

pub async fn create_run(
    pool: &PgPool,
    task: &str,
    model: &str,
    repo: &str,
    permission_mode: Option<&str>,
    isolation: &str,
) -> sqlx::Result<(Run, RunEvent)> {
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, permission_mode, isolation) VALUES ($1, $2, $3, $4, 'queued', $5, $6)
         RETURNING {RUN_COLS}"
    ))
    .bind(id)
    .bind(task)
    .bind(model)
    .bind(repo)
    .bind(permission_mode)
    .bind(isolation)
    .fetch_one(&mut *tx)
    .await?;
    let run = row_to_run(&row);
    let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "queued", "task": task})).await?;
    tx.commit().await?;
    Ok((run, ev))
}

pub enum FollowUpOutcome {
    Created(Run, RunEvent),
    NotFound,
    /// The thread's latest run is still queued or running.
    Busy(Run),
    /// The previous run never reported a session id (it failed before starting, or was stopped early).
    NoSession(Run),
}

/// Continue a thread with a new message. The new run resumes the latest
/// session in the thread and is pinned to the worker that holds it.
pub async fn create_follow_up(
    pool: &PgPool,
    run_id: Uuid,
    task: &str,
    model: Option<&str>,
    permission_mode: Option<&str>,
) -> sqlx::Result<FollowUpOutcome> {
    let Some(any) = get_run(pool, run_id).await? else {
        return Ok(FollowUpOutcome::NotFound);
    };
    let root_id = any.parent_id.unwrap_or(any.id);
    let thread = thread_runs(pool, root_id).await?;
    let Some(latest) = thread.last().cloned() else {
        return Ok(FollowUpOutcome::NotFound);
    };
    if !latest.status.is_terminal() {
        return Ok(FollowUpOutcome::Busy(latest));
    }
    // The most recent session id anywhere in the thread (a stopped follow-up may lack one).
    let session = thread.iter().rev().find_map(|r| r.session_id.clone());
    let Some(session) = session else {
        return Ok(FollowUpOutcome::NoSession(latest));
    };
    let worker = thread.iter().rev().find_map(|r| r.worker_id.clone());
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, parent_id, resume_session_id, pinned_worker_id, permission_mode, isolation)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, $9) RETURNING {RUN_COLS}"
    ))
    .bind(id)
    .bind(task)
    .bind(model.unwrap_or(&latest.model))
    .bind(&latest.repo)
    .bind(root_id)
    .bind(&session)
    .bind(&worker)
    .bind(permission_mode.or(latest.permission_mode.as_deref()))
    .bind(&latest.isolation)
    .fetch_one(&mut *tx)
    .await?;
    let run = row_to_run(&row);
    let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "queued", "task": task, "resumes": session})).await?;
    tx.commit().await?;
    Ok(FollowUpOutcome::Created(run, ev))
}

/// All runs of a thread, root first, in order.
pub async fn thread_runs(pool: &PgPool, root_id: Uuid) -> sqlx::Result<Vec<Run>> {
    let rows = sqlx::query(&format!(
        "SELECT {RUN_COLS} FROM runs WHERE id = $1 OR parent_id = $1 ORDER BY created_at"
    ))
    .bind(root_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_run).collect())
}

pub async fn get_run(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Run>> {
    let row = sqlx::query(&format!("SELECT {RUN_COLS} FROM runs WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_run))
}

/// Root runs (threads), newest activity first, with thread status and follow-up count.
pub async fn list_runs(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<RunSummary>> {
    let cols = RUN_COLS.split(", ").map(|c| format!("r.{c}")).collect::<Vec<_>>().join(", ");
    let rows = sqlx::query(&format!(
        "SELECT {cols},
                (SELECT count(*) FROM runs f WHERE f.parent_id = r.id) AS follow_ups,
                (SELECT l.status FROM runs l WHERE l.id = r.id OR l.parent_id = r.id ORDER BY l.created_at DESC LIMIT 1) AS thread_status,
                (SELECT max(greatest(l.created_at, coalesce(l.finished_at, l.created_at))) FROM runs l WHERE l.id = r.id OR l.parent_id = r.id) AS last_activity_at
         FROM runs r WHERE r.parent_id IS NULL ORDER BY last_activity_at DESC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let ts: String = r.get("thread_status");
            RunSummary {
                run: row_to_run(r),
                follow_ups: r.get("follow_ups"),
                thread_status: RunStatus::parse(&ts).unwrap_or(RunStatus::Failed),
                last_activity_at: r.get("last_activity_at"),
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
    run_id: Uuid,
    worker_id: &str,
    events: &[sidefleet_core::NewEvent],
) -> sqlx::Result<AppendOutcome> {
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, run_id).await? else {
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
        let payload = sidefleet_core::redact::redact_json(&e.payload);
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

/// Request a stop. A queued run becomes `stopped` immediately; a running run
/// gets `stop_requested` set for the worker to act on. Returns the run and any
/// status event to broadcast.
pub async fn request_stop(pool: &PgPool, run_id: Uuid) -> sqlx::Result<Option<(Run, Option<RunEvent>)>> {
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, run_id).await? else {
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
                Some(w) => sqlx::query("SELECT (last_heartbeat_at > now() - interval '45 seconds') AS online FROM workers WHERE id = $1")
                    .bind(w)
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

/// Atomically claim the oldest queued run for one of `repos`.
pub async fn claim_run(
    pool: &PgPool,
    worker_id: &str,
    repos: &[String],
    busy_repos: &[String],
) -> sqlx::Result<Option<(Run, RunEvent)>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "UPDATE runs SET status = 'running', worker_id = $1, started_at = now()
         WHERE id = (
            SELECT id FROM runs WHERE status = 'queued' AND repo = ANY($2)
              AND NOT (repo = ANY($3))
              AND (pinned_worker_id IS NULL OR pinned_worker_id = $1)
            ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED
         )
         RETURNING {RUN_COLS}"
    ))
    .bind(worker_id)
    .bind(repos)
    .bind(busy_repos)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let run = row_to_run(&row);
    let ev = append_event_locked(
        &mut tx,
        run.id,
        "status",
        &json!({"status": "running", "worker_id": worker_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Some((run, ev)))
}

pub enum FinishOutcome {
    Finished(Run, RunEvent),
    NotFound,
    NotOwner,
    AlreadyFinished(Run),
}

pub async fn finish_run(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: &str,
    status: RunStatus,
    result: Option<Value>,
) -> sqlx::Result<FinishOutcome> {
    debug_assert!(status.is_terminal());
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, run_id).await? else {
        return Ok(FinishOutcome::NotFound);
    };
    if run.worker_id.as_deref() != Some(worker_id) {
        return Ok(FinishOutcome::NotOwner);
    }
    if run.status != RunStatus::Running {
        return Ok(FinishOutcome::AlreadyFinished(run));
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
    let ev = append_event_locked(&mut tx, run_id, "status", &status_payload(status)).await?;
    tx.commit().await?;
    Ok(FinishOutcome::Finished(row_to_run(&row), ev))
}

/// Runs assigned to `worker_id` that are `running` but not in `active`: the
/// worker lost them (restart, crash). Mark them finished so stops and replies
/// are not left waiting forever. A run that had a stop pending becomes
/// `stopped`, otherwise `failed`.
pub async fn reconcile_worker_runs(pool: &PgPool, worker_id: &str, active: &[Uuid]) -> sqlx::Result<Vec<RunEvent>> {
    let rows = sqlx::query("SELECT id FROM runs WHERE status = 'running' AND worker_id = $1 AND NOT (id = ANY($2))")
        .bind(worker_id)
        .bind(active)
        .fetch_all(pool)
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let id: Uuid = row.get("id");
        let mut tx = pool.begin().await?;
        let Some(run) = lock_run(&mut tx, id).await? else { continue };
        if run.status != RunStatus::Running || active.contains(&id) {
            continue;
        }
        let status = if run.stop_requested { RunStatus::Stopped } else { RunStatus::Failed };
        let reason = format!("the worker {worker_id} restarted while this run was in progress");
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

/// Fail running runs whose worker has not heart-beaten for `stale_secs`.
pub async fn reap_orphaned_runs(pool: &PgPool, stale_secs: i64) -> sqlx::Result<Vec<RunEvent>> {
    let rows = sqlx::query(
        "SELECT r.id FROM runs r JOIN workers w ON w.id = r.worker_id
         WHERE r.status = 'running' AND w.last_heartbeat_at < now() - make_interval(secs => $1)",
    )
    .bind(stale_secs as f64)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        let id: Uuid = row.get("id");
        let mut tx = pool.begin().await?;
        let Some(run) = lock_run(&mut tx, id).await? else { continue };
        if run.status != RunStatus::Running {
            continue;
        }
        sqlx::query(
            "UPDATE runs SET status = 'failed', finished_at = now(), result = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(json!({"error": "worker stopped heart-beating while the run was in progress"}))
        .execute(&mut *tx)
        .await?;
        let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "failed", "reason": "worker lost"})).await?;
        tx.commit().await?;
        out.push(ev);
    }
    Ok(out)
}

// ---- workers ----

pub async fn upsert_worker(
    pool: &PgPool,
    id: &str,
    display_name: &str,
    token_hash: &str,
    caps: &Capabilities,
) -> sqlx::Result<Worker> {
    let row = sqlx::query(
        "INSERT INTO workers (id, display_name, token_hash, capabilities, last_heartbeat_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (id) DO UPDATE SET display_name = EXCLUDED.display_name,
            token_hash = EXCLUDED.token_hash, capabilities = EXCLUDED.capabilities,
            last_heartbeat_at = now()
         RETURNING id, display_name, capabilities, last_heartbeat_at",
    )
    .bind(id)
    .bind(display_name)
    .bind(token_hash)
    .bind(serde_json::to_value(caps).unwrap_or(Value::Null))
    .fetch_one(pool)
    .await?;
    Ok(row_to_worker(&row))
}

/// Returns false when the worker is unknown or registered with a different token.
pub async fn heartbeat(
    pool: &PgPool,
    id: &str,
    token_hash: &str,
    caps: &Capabilities,
) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE workers SET capabilities = $3, last_heartbeat_at = now()
         WHERE id = $1 AND token_hash = $2",
    )
    .bind(id)
    .bind(token_hash)
    .bind(serde_json::to_value(caps).unwrap_or(Value::Null))
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn worker_owns_token(pool: &PgPool, id: &str, token_hash: &str) -> sqlx::Result<bool> {
    let row = sqlx::query("SELECT 1 AS ok FROM workers WHERE id = $1 AND token_hash = $2")
        .bind(id)
        .bind(token_hash)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn list_workers(pool: &PgPool) -> sqlx::Result<Vec<Worker>> {
    let rows = sqlx::query(
        "SELECT id, display_name, capabilities, last_heartbeat_at FROM workers ORDER BY display_name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_worker).collect())
}

pub fn is_online(last_heartbeat_at: DateTime<Utc>) -> bool {
    Utc::now().signed_duration_since(last_heartbeat_at).num_seconds() < 45
}

pub async fn list_added_repos(pool: &PgPool) -> sqlx::Result<Vec<String>> {
    let rows = sqlx::query("SELECT name FROM added_repos ORDER BY name").fetch_all(pool).await?;
    Ok(rows.iter().map(|r| r.get("name")).collect())
}

/// Replace the whole added set in one transaction.
pub async fn set_added_repos(pool: &PgPool, names: &[String]) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM added_repos WHERE NOT (name = ANY($1))").bind(names).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO added_repos (name) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING")
        .bind(names)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
