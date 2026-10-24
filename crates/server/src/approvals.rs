//! Tool approvals ("Ask me"): a running agent's worker asks whether a tool call
//! may go ahead, someone on the page approves or denies it, and the worker reads
//! the decision. Each change is also appended to the run's events
//! (`approval_requested`, `approval_decided`) so open pages see it live.
//!
//! Every change happens under the run's row lock, so a decision and the run
//! ending cannot cross: an approval of a run that is no longer running is
//! cancelled, never approved.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sidefleet_core::{
    redact_tool_input, summarize_tool_input, Approval, ApprovalAction, ApprovalRequest, ApprovalStatus, RunEvent, RunStatus,
    DEFAULT_APPROVAL_TIMEOUT_SECS, MAX_APPROVAL_TIMEOUT_SECS,
};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::{append_event_locked, lock_run};

/// Strings in a stored tool input are clipped to this many bytes.
pub const MAX_INPUT_STRING_BYTES: usize = 4096;
/// What a denial without a message tells the agent.
pub const DENIED_MESSAGE: &str = "The user denied this tool call on the Sidefleet page.";

const COLS: &str = "id, run_id, tool_name, input, tool_use_id, status, message, decided_by, created_at, expires_at, decided_at";

fn row_to_approval(r: &PgRow) -> Approval {
    Approval {
        id: r.get("id"),
        run_id: r.get("run_id"),
        tool_name: r.get("tool_name"),
        input: r.get("input"),
        tool_use_id: r.get("tool_use_id"),
        status: ApprovalStatus::parse(r.get::<&str, _>("status")).unwrap_or(ApprovalStatus::Cancelled),
        message: r.get("message"),
        decided_by: r.get("decided_by"),
        created_at: r.get("created_at"),
        expires_at: r.get("expires_at"),
        decided_at: r.get("decided_at"),
    }
}

/// The `approval_requested` event: the approval plus a one-line summary of the input.
fn requested_payload(a: &Approval) -> Value {
    let mut v = serde_json::to_value(a).unwrap_or(json!({}));
    v["summary"] = json!(summarize_tool_input(&a.tool_name, &a.input));
    v
}

fn decided_payload(a: &Approval) -> Value {
    json!({
        "id": a.id,
        "tool_name": a.tool_name,
        "tool_use_id": a.tool_use_id,
        "status": a.status,
        "message": a.message,
        "decided_by": a.decided_by,
        "decided_at": a.decided_at,
    })
}

pub enum RequestOutcome {
    Created(Box<Approval>, RunEvent),
    NotFound,
    NotOwner,
    /// The run is not running (stopped, finished): nothing to ask about.
    NotRunning,
}

/// A worker asks about a tool call of `run_id`, which it must be running.
pub async fn request(pool: &PgPool, org: Uuid, run_id: Uuid, worker_id: &str, req: &ApprovalRequest) -> sqlx::Result<RequestOutcome> {
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else {
        return Ok(RequestOutcome::NotFound);
    };
    if run.worker_id.as_deref() != Some(worker_id) {
        return Ok(RequestOutcome::NotOwner);
    }
    if run.status != RunStatus::Running || run.stop_requested {
        return Ok(RequestOutcome::NotRunning);
    }
    let timeout = req.timeout_secs.unwrap_or(DEFAULT_APPROVAL_TIMEOUT_SECS).clamp(1, MAX_APPROVAL_TIMEOUT_SECS);
    let tool_name: String = req.tool_name.chars().take(200).collect();
    let tool_use_id = req.tool_use_id.as_ref().map(|t| t.chars().take(200).collect::<String>());
    let row = sqlx::query(&format!(
        "INSERT INTO approvals (id, org_id, run_id, tool_name, input, tool_use_id, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7)) RETURNING {COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(run_id)
    .bind(&tool_name)
    .bind(redact_tool_input(&req.input, MAX_INPUT_STRING_BYTES))
    .bind(tool_use_id)
    .bind(timeout as f64)
    .fetch_one(&mut *tx)
    .await?;
    let approval = row_to_approval(&row);
    let ev = append_event_locked(&mut tx, run_id, "approval_requested", &requested_payload(&approval)).await?;
    tx.commit().await?;
    Ok(RequestOutcome::Created(Box::new(approval), ev))
}

/// Apply `action` to a locked, pending approval. Returns the stored approval and its event.
async fn transition(
    tx: &mut Transaction<'_, Postgres>,
    current: &Approval,
    to: ApprovalStatus,
    message: Option<&str>,
    by: Option<&str>,
) -> sqlx::Result<(Approval, RunEvent)> {
    let row = sqlx::query(&format!(
        "UPDATE approvals SET status = $2, message = $3, decided_by = $4, decided_at = now() WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(current.id)
    .bind(to.as_str())
    .bind(message)
    .bind(by)
    .fetch_one(&mut **tx)
    .await?;
    let a = row_to_approval(&row);
    let ev = append_event_locked(tx, a.run_id, "approval_decided", &decided_payload(&a)).await?;
    Ok((a, ev))
}

async fn lock_approval(tx: &mut Transaction<'_, Postgres>, org: Uuid, run_id: Uuid, id: Uuid) -> sqlx::Result<Option<Approval>> {
    let row = sqlx::query(&format!("SELECT {COLS} FROM approvals WHERE id = $1 AND run_id = $2 AND org_id = $3 FOR UPDATE"))
        .bind(id)
        .bind(run_id)
        .bind(org)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.as_ref().map(row_to_approval))
}

fn expired_message(a: &Approval) -> String {
    let mins = (a.expires_at - a.created_at).num_seconds().max(1) as f64 / 60.0;
    let wait = if mins >= 1.0 { format!("{} minutes", mins.round()) } else { format!("{} seconds", (mins * 60.0).round()) };
    format!("Nobody approved this tool call on the Sidefleet page within {wait}, so it was denied.")
}

const CANCELLED_MESSAGE: &str = "The run ended before anyone decided.";

pub enum DecideOutcome {
    /// Approved or denied now.
    Decided(Approval, RunEvent),
    NotFound,
    /// It was already decided, expired or cancelled.
    AlreadyDecided(Approval),
    /// Its deadline passed before this decision came in; now expired.
    Expired(Approval, Option<RunEvent>),
    /// Its run is no longer running; now cancelled.
    RunEnded(Approval, Option<RunEvent>),
}

/// Someone on the page approves or denies a pending approval of `run_id`.
pub async fn decide(
    pool: &PgPool,
    org: Uuid,
    run_id: Uuid,
    id: Uuid,
    approve: bool,
    by: &str,
    message: Option<&str>,
) -> sqlx::Result<DecideOutcome> {
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else {
        return Ok(DecideOutcome::NotFound);
    };
    let Some(current) = lock_approval(&mut tx, org, run_id, id).await? else {
        return Ok(DecideOutcome::NotFound);
    };
    if current.status != ApprovalStatus::Pending {
        return Ok(DecideOutcome::AlreadyDecided(current));
    }
    if run.status != RunStatus::Running {
        let (a, ev) = transition(&mut tx, &current, ApprovalStatus::Cancelled, Some(CANCELLED_MESSAGE), None).await?;
        tx.commit().await?;
        return Ok(DecideOutcome::RunEnded(a, Some(ev)));
    }
    let overdue = current.expires_at <= Utc::now();
    let action = if approve { ApprovalAction::Approve } else { ApprovalAction::Deny };
    let Some(to) = current.status.after(action, overdue) else {
        return Ok(DecideOutcome::AlreadyDecided(current));
    };
    let message = match to {
        ApprovalStatus::Denied => Some(message.map(str::trim).filter(|m| !m.is_empty()).map(|m| m.chars().take(1000).collect()).unwrap_or_else(|| DENIED_MESSAGE.to_string())),
        ApprovalStatus::Expired => Some(expired_message(&current)),
        _ => None,
    };
    let by = (to != ApprovalStatus::Expired).then_some(by);
    let (a, ev) = transition(&mut tx, &current, to, message.as_deref(), by).await?;
    tx.commit().await?;
    Ok(if to == ApprovalStatus::Expired { DecideOutcome::Expired(a, Some(ev)) } else { DecideOutcome::Decided(a, ev) })
}

/// The worker reads an approval. A pending one past its deadline expires now, and
/// one whose run stopped or ended is cancelled, so the worker always learns an end.
pub async fn poll(pool: &PgPool, org: Uuid, run_id: Uuid, id: Uuid, now: DateTime<Utc>) -> sqlx::Result<Option<(Approval, Option<RunEvent>)>> {
    // Cheap path: no lock while it is simply still pending.
    let row = sqlx::query(&format!("SELECT {COLS} FROM approvals WHERE id = $1 AND run_id = $2 AND org_id = $3"))
        .bind(id)
        .bind(run_id)
        .bind(org)
        .fetch_optional(pool)
        .await?;
    let Some(a) = row.as_ref().map(row_to_approval) else { return Ok(None) };
    if a.status != ApprovalStatus::Pending {
        return Ok(Some((a, None)));
    }
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else { return Ok(None) };
    let Some(current) = lock_approval(&mut tx, org, run_id, id).await? else { return Ok(None) };
    let action = if run.status != RunStatus::Running || run.stop_requested {
        Some(ApprovalAction::Cancel)
    } else if current.expires_at <= now {
        Some(ApprovalAction::Expire)
    } else {
        None
    };
    let Some(to) = action.and_then(|act| current.status.after(act, current.expires_at <= now)) else {
        return Ok(Some((current, None)));
    };
    let message = if to == ApprovalStatus::Expired { expired_message(&current) } else { CANCELLED_MESSAGE.to_string() };
    let (a, ev) = transition(&mut tx, &current, to, Some(&message), None).await?;
    tx.commit().await?;
    Ok(Some((a, Some(ev))))
}

/// Cancel the pending approvals of a run that is ending. The caller holds the run lock.
pub async fn cancel_pending(tx: &mut Transaction<'_, Postgres>, run_id: Uuid) -> sqlx::Result<Vec<RunEvent>> {
    let rows = sqlx::query(&format!("SELECT {COLS} FROM approvals WHERE run_id = $1 AND status = 'pending' ORDER BY created_at FOR UPDATE"))
        .bind(run_id)
        .fetch_all(&mut **tx)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let (_, ev) = transition(tx, &row_to_approval(r), ApprovalStatus::Cancelled, Some(CANCELLED_MESSAGE), None).await?;
        out.push(ev);
    }
    Ok(out)
}

/// Approvals of one run, oldest first.
pub async fn list(pool: &PgPool, org: Uuid, run_id: Uuid) -> sqlx::Result<Vec<Approval>> {
    let rows = sqlx::query(&format!("SELECT {COLS} FROM approvals WHERE run_id = $1 AND org_id = $2 ORDER BY created_at"))
        .bind(run_id)
        .bind(org)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_approval).collect())
}
