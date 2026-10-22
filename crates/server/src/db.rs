//! Postgres access. Plain `sqlx::query` (no compile-time macros) so the
//! workspace builds without a database.

use sidefleet_core::{Agent, Capabilities, ImageRef, ImageUpload, Run, RunEvent, RunStatus, RunSummary, Slots, UsageLimit, Worker};
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

pub async fn set_update_requested(pool: &PgPool, id: &str, value: bool) -> sqlx::Result<bool> {
    let res = sqlx::query("UPDATE workers SET update_requested = $2 WHERE id = $1")
        .bind(id)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn update_requested(pool: &PgPool, id: &str) -> sqlx::Result<bool> {
    let row = sqlx::query("SELECT update_requested FROM workers WHERE id = $1").bind(id).fetch_optional(pool).await?;
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
    "id, task, agent, model, repo, status, worker_id, stop_requested, created_at, started_at, finished_at, result, parent_id, session_id, resume_session_id, pinned_worker_id, permission_mode, isolation, failover, moved_from, resume_from_branch, images";

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

/// A task to queue (validated by the route).
pub struct NewRun<'a> {
    pub task: &'a str,
    pub agent: Agent,
    pub model: &'a str,
    pub repo: &'a str,
    pub permission_mode: Option<&'a str>,
    pub isolation: &'a str,
    pub pinned_worker_id: Option<&'a str>,
    pub failover: bool,
    pub images: &'a [ImageUpload],
}

pub async fn create_run(pool: &PgPool, new: NewRun<'_>) -> sqlx::Result<(Run, RunEvent)> {
    let NewRun { task, agent, model, repo, permission_mode, isolation, pinned_worker_id, failover, images } = new;
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, permission_mode, isolation, pinned_worker_id, agent, failover, images)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, $9, $10) RETURNING {RUN_COLS}"
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
    .fetch_one(&mut *tx)
    .await?;
    let mut run = row_to_run(&row);
    run.images = attach_uploads(&mut tx, id, images).await?;
    let ev = append_event_locked(&mut tx, id, "status", &json!({"status": "queued", "task": task})).await?;
    tx.commit().await?;
    Ok((run, ev))
}

pub enum FollowUpOutcome {
    Created(Run, RunEvent),
    NotFound,
    /// Already `MAX_QUEUED_MESSAGES` messages waiting in the thread.
    QueueFull,
    /// The previous run never reported a session id (it failed before starting, or was stopped early).
    NoSession(Run),
}

/// Messages that may wait behind a running one in a thread.
pub const MAX_QUEUED_MESSAGES: usize = 10;

/// Continue a thread with a new message. The new run resumes the latest
/// session in the thread and is pinned to the worker that holds it. While an
/// earlier message is still queued or running, the new one waits behind it:
/// its session and machine are settled when it is claimed (`claim_run`).
pub async fn create_follow_up(
    pool: &PgPool,
    run_id: Uuid,
    task: &str,
    model: Option<&str>,
    permission_mode: Option<&str>,
    images: &[ImageUpload],
) -> sqlx::Result<FollowUpOutcome> {
    let Some(any) = get_run(pool, run_id).await? else {
        return Ok(FollowUpOutcome::NotFound);
    };
    let root_id = any.parent_id.unwrap_or(any.id);
    let thread = thread_runs(pool, root_id).await?;
    let Some(latest) = thread.last().cloned() else {
        return Ok(FollowUpOutcome::NotFound);
    };
    let busy = !latest.status.is_terminal();
    if busy && thread.iter().filter(|r| r.status == RunStatus::Queued).count() >= MAX_QUEUED_MESSAGES {
        return Ok(FollowUpOutcome::QueueFull);
    }
    // The most recent session id anywhere in the thread (a stopped follow-up may lack one).
    let session = thread.iter().rev().find_map(|r| r.session_id.clone());
    if !busy && session.is_none() {
        return Ok(FollowUpOutcome::NoSession(latest));
    }
    // Behind a busy run the session and machine may still change (or not exist yet): settle them at claim time.
    let session = if busy { None } else { session };
    let worker = if busy { None } else { thread.iter().rev().find_map(|r| r.worker_id.clone()) };
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "INSERT INTO runs (id, task, model, repo, status, parent_id, resume_session_id, pinned_worker_id, permission_mode, isolation, agent, failover, images)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, $9, $10, $11, $12) RETURNING {RUN_COLS}"
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
    .bind(latest.agent.as_str())
    .bind(latest.failover)
    .bind(json!([]))
    .fetch_one(&mut *tx)
    .await?;
    let mut run = row_to_run(&row);
    run.images = attach_uploads(&mut tx, id, images).await?;
    let mut payload = json!({"status": "queued", "task": task, "resumes": session});
    if busy {
        payload["after"] = json!(latest.id);
    }
    let ev = append_event_locked(&mut tx, id, "status", &payload).await?;
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

/// Root runs (threads), newest activity first, with thread status (the running
/// message if any, else the latest) and follow-up count.
pub async fn list_runs(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<RunSummary>> {
    let cols = RUN_COLS.split(", ").map(|c| format!("r.{c}")).collect::<Vec<_>>().join(", ");
    let rows = sqlx::query(&format!(
        "SELECT {cols},
                (SELECT count(*) FROM runs f WHERE f.parent_id = r.id) AS follow_ups,
                (SELECT l.status FROM runs l WHERE l.id = r.id OR l.parent_id = r.id ORDER BY (l.status = 'running') DESC, l.created_at DESC LIMIT 1) AS thread_status,
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

/// An image's media type and bytes.
pub async fn get_image(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<(String, Vec<u8>)>> {
    let row = sqlx::query("SELECT media_type, data FROM run_images WHERE id = $1").bind(id).fetch_optional(pool).await?;
    Ok(row.map(|r| (r.get("media_type"), r.get("data"))))
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

/// SQL condition: worker `w` could run queued run `r` now. It is online, has
/// the repo (by identity or path name) and the agent, has not hit that agent's
/// usage limit, and is not the machine that lost the run (failover). Workers from before multi-agent support report no
/// agents and run Claude Code.
const CAN_RUN: &str = "w.last_heartbeat_at > now() - interval '45 seconds'
      AND NOT (w.id = ANY(r.excluded_worker_ids))
      AND EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(w.capabilities->'repos', '[]'::jsonb)) AS info(repo)
                  WHERE info.repo->>'name' = r.repo OR info.repo->>'id' = r.repo)
      AND (w.capabilities->'agents' ? r.agent
           OR (r.agent = 'claude' AND COALESCE(w.capabilities->'agents', '{}'::jsonb) - 'ollama' = '{}'::jsonb))
      AND NOT EXISTS (SELECT 1 FROM worker_limits l WHERE l.worker_id = w.id AND l.agent = r.agent AND l.until > now())";

/// How long a new run waits for the least busy machine before any machine that
/// can run it may take it (that machine may have the repo busy, or be slow to poll).
const LEAST_BUSY_GRACE_SECS: i64 = 10;

/// What a claim produced: the run to execute, plus queued messages that were
/// failed on the way because there was no session left to continue.
pub struct Claimed {
    pub run: Option<(Run, RunEvent)>,
    pub failed: Vec<RunEvent>,
}

/// Atomically claim the oldest queued run for one of `repos`.
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
pub async fn claim_run(
    pool: &PgPool,
    worker_id: &str,
    repos: &[String],
    busy_repos: &[String],
    agents: &[String],
    slots: Option<&Slots>,
) -> sqlx::Result<Claimed> {
    let free = slots.map(|s| s.parallel.saturating_sub(s.active) as i32);
    if let Some(s) = slots {
        sqlx::query("UPDATE workers SET slots_parallel = $2, slots_active = $3 WHERE id = $1")
            .bind(worker_id)
            .bind(s.parallel as i32)
            .bind(s.active as i32)
            .execute(pool)
            .await?;
    }
    let mut failed = Vec::new();
    // Each pass either claims a run, finds none, or fails one unresumable message and looks again.
    for _ in 0..=MAX_QUEUED_MESSAGES {
        let mut tx = pool.begin().await?;
        let row = sqlx::query(&format!(
            "UPDATE runs SET status = 'running', worker_id = $1, started_at = now()
             WHERE id = (
                SELECT r.id FROM runs r WHERE r.status = 'queued' AND r.repo = ANY($2)
                  AND NOT (r.repo = ANY($3))
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
                  AND NOT EXISTS (SELECT 1 FROM worker_limits l WHERE l.worker_id = $1 AND l.agent = r.agent AND l.until > now())
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
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(Claimed { run: None, failed });
        };
        let mut run = row_to_run(&row);
        if let (Some(root), None, None) = (run.parent_id, &run.resume_session_id, &run.moved_from) {
            // A message that waited behind another: continue the thread's latest session on this machine.
            let session: Option<String> = sqlx::query(
                "SELECT session_id FROM runs WHERE (id = $1 OR parent_id = $1) AND id <> $2 AND worker_id = $3
                   AND session_id IS NOT NULL
                 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(root)
            .bind(run.id)
            .bind(worker_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(|r| r.get("session_id"));
            match session {
                Some(sid) => {
                    sqlx::query("UPDATE runs SET resume_session_id = $2, pinned_worker_id = $3 WHERE id = $1")
                        .bind(run.id)
                        .bind(&sid)
                        .bind(worker_id)
                        .execute(&mut *tx)
                        .await?;
                    run.resume_session_id = Some(sid);
                    run.pinned_worker_id = Some(worker_id.to_string());
                }
                None => {
                    let reason = "the earlier message ended before the agent started a session, so there is nothing to continue";
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
        }
        // One slot fewer free here until the next heartbeat or claim says otherwise.
        sqlx::query("UPDATE workers SET slots_active = slots_active + 1 WHERE id = $1 AND slots_active IS NOT NULL")
            .bind(worker_id)
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

pub async fn finish_run(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: &str,
    status: RunStatus,
    result: Option<Value>,
    usage_limit: Option<&UsageLimit>,
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
    let mut events = Vec::new();
    if let (Some(limit), RunStatus::Failed) = (usage_limit, status) {
        // The reset time the agent reported, if it is ahead of us; otherwise an hour.
        let now = Utc::now();
        let until = limit
            .resets_at
            .filter(|t| *t > now && *t < now + chrono::Duration::days(8))
            .unwrap_or(now + chrono::Duration::seconds(DEFAULT_LIMIT_SECS));
        sqlx::query(
            "INSERT INTO worker_limits (worker_id, agent, until) VALUES ($1, $2, $3)
             ON CONFLICT (worker_id, agent) DO UPDATE SET until = EXCLUDED.until",
        )
        .bind(worker_id)
        .bind(run.agent.as_str())
        .bind(until)
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
                           failover, moved_from, resume_from_branch, excluded_worker_ids, images)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, true, $9, $10, ARRAY[$11], $12) RETURNING {RUN_COLS}"
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
    id: &str,
    display_name: &str,
    token_hash: &str,
    caps: &Capabilities,
) -> sqlx::Result<Worker> {
    let row = sqlx::query(
        "INSERT INTO workers (id, display_name, token_hash, capabilities, last_heartbeat_at, slots_parallel, slots_active)
         VALUES ($1, $2, $3, $4, now(), $5, $6)
         ON CONFLICT (id) DO UPDATE SET display_name = EXCLUDED.display_name,
            token_hash = EXCLUDED.token_hash, capabilities = EXCLUDED.capabilities,
            last_heartbeat_at = now(), update_requested = false,
            slots_parallel = EXCLUDED.slots_parallel, slots_active = EXCLUDED.slots_active
         RETURNING id, display_name, capabilities, last_heartbeat_at",
    )
    .bind(id)
    .bind(display_name)
    .bind(token_hash)
    .bind(serde_json::to_value(caps).unwrap_or(Value::Null))
    .bind(caps.slots.as_ref().map(|s| s.parallel as i32))
    .bind(caps.slots.as_ref().map(|s| s.active as i32))
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
        "UPDATE workers SET capabilities = $3, last_heartbeat_at = now(),
            slots_parallel = $4, slots_active = $5
         WHERE id = $1 AND token_hash = $2",
    )
    .bind(id)
    .bind(token_hash)
    .bind(serde_json::to_value(caps).unwrap_or(Value::Null))
    .bind(caps.slots.as_ref().map(|s| s.parallel as i32))
    .bind(caps.slots.as_ref().map(|s| s.active as i32))
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

pub async fn worker_exists(pool: &PgPool, id: &str) -> sqlx::Result<bool> {
    let row = sqlx::query("SELECT 1 AS ok FROM workers WHERE id = $1").bind(id).fetch_optional(pool).await?;
    Ok(row.is_some())
}

pub async fn list_workers(pool: &PgPool) -> sqlx::Result<Vec<Worker>> {
    let rows = sqlx::query(
        "SELECT id, display_name, capabilities, last_heartbeat_at, update_requested FROM workers ORDER BY display_name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_worker).collect())
}

/// Agents each machine may not use until their usage limit resets: worker id -> agent -> until.
pub async fn active_limits(pool: &PgPool) -> sqlx::Result<std::collections::HashMap<String, serde_json::Map<String, Value>>> {
    let rows = sqlx::query("SELECT worker_id, agent, until FROM worker_limits WHERE until > now()").fetch_all(pool).await?;
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
