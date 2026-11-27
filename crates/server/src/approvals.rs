//! Tool approvals ("Ask me"): a running agent's worker asks whether a tool call
//! may go ahead, someone on the page approves or denies it, and the worker reads
//! the decision. Each change is also appended to the run's events
//! (`approval_requested`, `approval_decided`) so open pages see it live.
//!
//! Every change happens under the run's row lock, so a decision and the run
//! ending cannot cross: an approval of a run that is no longer running is
//! cancelled, never approved.
//!
//! The control plane's policies put approvals here too (`kind = 'policy'`), bound to
//! the exact action by `digest`. A custom agent's approval has no run: those change
//! under the approval's own row lock instead. Every change is recorded in the agent's
//! activity and the audit log.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sidefleet_core::redact::redact_json;
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

const COLS: &str = "id, run_id, tool_name, input, tool_use_id, status, message, decided_by, created_at, expires_at, decided_at, digest";
/// What the audit and activity records of a change need on top of [`COLS`].
const EXTRA: &str = "org_id, kind, agent_id, policy_id, policy_name, action, context";

/// What an approval is about, beyond the tool call: the agent asking, and for a policy's
/// approval, the policy and the digest of the exact action.
#[derive(Debug, Clone, Default)]
pub struct Hold {
    /// `ask` ("Ask me") or `policy`.
    pub kind: &'static str,
    pub agent_id: Option<Uuid>,
    pub policy_id: Option<Uuid>,
    pub policy_name: Option<String>,
    /// `organisation, priority 10`.
    pub policy_scope: Option<String>,
    pub digest: Option<String>,
    /// The action tool that needed it, e.g. `shell.exec`.
    pub action: Option<String>,
    /// `{task, repo, branch}`.
    pub context: Value,
}

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
        digest: r.get("digest"),
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
pub async fn request(pool: &PgPool, org: Uuid, run_id: Uuid, worker_id: &str, req: &ApprovalRequest, hold: &Hold) -> sqlx::Result<RequestOutcome> {
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
    let row = insert(&mut tx, org, Some(run_id), &tool_name, &redact_tool_input(&req.input, MAX_INPUT_STRING_BYTES), tool_use_id, timeout, hold).await?;
    let approval = row_to_approval(&row);
    let ev = append_event_locked(&mut tx, run_id, "approval_requested", &requested_payload(&approval)).await?;
    record_requested(&mut tx, org, &row, hold).await?;
    tx.commit().await?;
    Ok(RequestOutcome::Created(Box::new(approval), ev))
}

#[allow(clippy::too_many_arguments)]
async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    run_id: Option<Uuid>,
    tool_name: &str,
    input: &Value,
    tool_use_id: Option<String>,
    timeout: u64,
    hold: &Hold,
) -> sqlx::Result<PgRow> {
    sqlx::query(&format!(
        "INSERT INTO approvals (id, org_id, run_id, tool_name, input, tool_use_id, expires_at, kind, agent_id, policy_id, policy_name, digest, action, context)
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7), $8, $9, $10, $11, $12, $13, $14) RETURNING {COLS}, {EXTRA}"
    ))
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(run_id)
    .bind(tool_name)
    .bind(input)
    .bind(tool_use_id)
    .bind(timeout as f64)
    .bind(if hold.kind.is_empty() { "ask" } else { hold.kind })
    .bind(hold.agent_id)
    .bind(hold.policy_id)
    .bind(&hold.policy_name)
    .bind(&hold.digest)
    .bind(&hold.action)
    .bind(if hold.context.is_null() { json!({}) } else { redact_json(&hold.context) })
    .fetch_one(&mut **tx)
    .await
}

/// What is being asked, in one line: the command for a shell call, else a summary.
pub fn command_text(tool_name: &str, input: &Value) -> String {
    match input.get("command").and_then(Value::as_str) {
        Some(c) => c.to_string(),
        None => match input {
            Value::String(s) => format!("{tool_name} {s}"),
            _ => summarize_tool_input(tool_name, input),
        },
    }
}

/// The activity and audit records of a new approval. Call last in the transaction.
async fn record_requested(tx: &mut Transaction<'_, Postgres>, org: Uuid, row: &PgRow, hold: &Hold) -> sqlx::Result<()> {
    let tool_name: String = row.get("tool_name");
    let input: Value = row.get("input");
    let run_id: Option<Uuid> = row.get("run_id");
    let command = command_text(&tool_name, &input);
    let (agent_label, owner_label, agent_name) = crate::control::agents::labels(tx, org, hold.agent_id).await?;
    let policy = hold.policy_name.clone().map(|n| (hold.policy_id.unwrap_or_default(), n));
    crate::control::activity::record(
        &mut **tx,
        org,
        crate::control::activity::NewEvent {
            agent_id: hold.agent_id,
            run_id,
            kind: "permission_request",
            tool: hold.action.clone().unwrap_or_else(|| tool_name.clone()),
            action: match &input {
                Value::String(target) => target.clone(),
                _ => command.clone(),
            },
            context: hold.context.clone(),
            outcome: "approval_required",
            policy: policy.clone(),
            decision: Some("require_approval"),
            metadata: json!({"approval_id": row.get::<Uuid, _>("id"), "kind": hold.kind, "digest": hold.digest}),
        },
    )
    .await?;
    let (perms, policy_label) = match (&hold.policy_name, hold.kind) {
        (Some(n), _) => (
            format!("{n}: approval required"),
            format!("{n} ({})", hold.policy_scope.clone().unwrap_or_default()),
        ),
        _ => ("\"Ask me\" permission mode: the person decides each call".to_string(), "None (\"Ask me\")".to_string()),
    };
    crate::control::audit::append(
        tx,
        org,
        crate::control::audit::NewRecord {
            actor: crate::control::audit::Actor::agent(&agent_name),
            event: "approval.requested",
            agent_id: hold.agent_id,
            target: agent_name.clone(),
            detail: command.chars().take(200).collect(),
            data: crate::control::audit::Investigate {
                what: format!("The agent asked to run {tool_name}. The gateway held the call."),
                agent: agent_label,
                owner: owner_label,
                permissions: perms,
                policy: policy_label,
                approval: match &hold.digest {
                    Some(d) => format!("Required, pending, bound to {d}"),
                    None => "Required, pending".into(),
                },
                decided_by: "Not decided yet".into(),
                outcome: "Held at the execution gateway".into(),
            },
        },
    )
    .await?;
    Ok(())
}

/// The activity and audit records of a decided, expired or cancelled approval.
async fn record_decided(tx: &mut Transaction<'_, Postgres>, row: &PgRow) -> sqlx::Result<()> {
    let org: Uuid = row.get("org_id");
    let agent_id: Option<Uuid> = row.get("agent_id");
    let status = ApprovalStatus::parse(row.get::<&str, _>("status")).unwrap_or(ApprovalStatus::Cancelled);
    let tool_name: String = row.get("tool_name");
    let input: Value = row.get("input");
    let command = command_text(&tool_name, &input);
    let by: Option<String> = row.get("decided_by");
    let digest: Option<String> = row.get("digest");
    let policy_name: Option<String> = row.get("policy_name");
    let policy_id: Option<Uuid> = row.get("policy_id");
    let action: Option<String> = row.get("action");
    let (kind, outcome, event, verb) = match status {
        ApprovalStatus::Approved => ("approval_approved", "approved", "approval.approved", "approved"),
        ApprovalStatus::Denied => ("approval_rejected", "rejected", "approval.rejected", "rejected"),
        ApprovalStatus::Expired => ("approval_expired", "expired", "approval.expired", "expired"),
        _ => ("approval_cancelled", "cancelled", "approval.cancelled", "cancelled"),
    };
    let who = by.clone().unwrap_or_else(|| "Sidefleet".into());
    crate::control::activity::record(
        &mut **tx,
        org,
        crate::control::activity::NewEvent {
            agent_id,
            run_id: row.get("run_id"),
            kind,
            tool: action.unwrap_or_else(|| tool_name.clone()),
            action: format!("{command} ({verb} by {who})"),
            context: row.get("context"),
            outcome,
            policy: policy_name.clone().map(|n| (policy_id.unwrap_or_default(), n)),
            decision: None,
            metadata: json!({"approval_id": row.get::<Uuid, _>("id"), "message": row.get::<Option<String>, _>("message")}),
        },
    )
    .await?;
    let (agent_label, owner_label, agent_name) = crate::control::agents::labels(tx, org, agent_id).await?;
    let actor = match &by {
        Some(b) => crate::control::audit::Actor { name: b.clone(), kind: "member" },
        None => crate::control::audit::Actor::system(),
    };
    crate::control::audit::append(
        tx,
        org,
        crate::control::audit::NewRecord {
            actor,
            event,
            agent_id,
            target: agent_name,
            detail: command.chars().take(200).collect(),
            data: crate::control::audit::Investigate {
                what: format!("A held {tool_name} call was {verb}."),
                agent: agent_label,
                owner: owner_label,
                permissions: match &policy_name {
                    Some(n) => format!("{n}: approval required"),
                    None => "\"Ask me\" permission mode".into(),
                },
                policy: policy_name.unwrap_or_else(|| "None (\"Ask me\")".into()),
                approval: match (&digest, status) {
                    (Some(d), ApprovalStatus::Approved) => format!("Required, approved once for {d}"),
                    _ => format!("Required, {verb}"),
                },
                decided_by: who,
                outcome: match status {
                    ApprovalStatus::Approved => "Released to the agent; it runs exactly this action".into(),
                    _ => row.get::<Option<String>, _>("message").unwrap_or_else(|| "Not executed".into()),
                },
            },
        },
    )
    .await?;
    Ok(())
}

/// Apply `action` to a locked, pending approval. Returns the stored approval and its event.
async fn transition(
    tx: &mut Transaction<'_, Postgres>,
    current: &Approval,
    to: ApprovalStatus,
    message: Option<&str>,
    by: Option<&str>,
) -> sqlx::Result<(Approval, RunEvent)> {
    let row = set_status(tx, current.id, to, message, by).await?;
    let a = row_to_approval(&row);
    let ev = append_event_locked(tx, a.run_id, "approval_decided", &decided_payload(&a)).await?;
    record_decided(tx, &row).await?;
    Ok((a, ev))
}

async fn set_status(tx: &mut Transaction<'_, Postgres>, id: Uuid, to: ApprovalStatus, message: Option<&str>, by: Option<&str>) -> sqlx::Result<PgRow> {
    sqlx::query(&format!(
        "UPDATE approvals SET status = $2, message = $3, decided_by = $4, decided_at = now() WHERE id = $1 RETURNING {COLS}, {EXTRA}"
    ))
    .bind(id)
    .bind(to.as_str())
    .bind(message)
    .bind(by)
    .fetch_one(&mut **tx)
    .await
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
    /// A custom agent's approval (no run), approved or denied now.
    DecidedDetached,
    /// A custom agent's approval that was already decided, expired or cancelled.
    AlreadyDecidedDetached(ApprovalStatus),
    /// A custom agent's approval whose deadline passed; now expired.
    ExpiredDetached,
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

// ---- the control plane's view: every approval in the org ----

/// An approval as the Approvals page shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OrgApproval {
    pub id: Uuid,
    pub kind: String,
    pub status: ApprovalStatus,
    pub agent: Option<Value>,
    pub owner: Option<Value>,
    pub runtime: String,
    pub run_id: Option<Uuid>,
    pub title: String,
    pub tool: String,
    pub tool_name: String,
    pub command: String,
    pub digest: Option<String>,
    pub context: Value,
    pub policy: Option<Value>,
    pub message: Option<String>,
    pub decided_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub can_decide: bool,
    /// Who may decide besides org owners: the agent's owner, and for "Ask me" the thread's.
    #[serde(skip)]
    pub agent_owner: Option<Uuid>,
    #[serde(skip)]
    pub thread_owner: Option<Uuid>,
}

impl OrgApproval {
    pub fn decidable_by(&self, user: Option<Uuid>, org_owner: bool) -> bool {
        if org_owner {
            return true;
        }
        let Some(u) = user else { return false };
        self.agent_owner == Some(u) || (self.kind == "ask" && self.thread_owner == Some(u))
    }
}

const ORG_SELECT: &str = "SELECT p.*, a.name AS agent_name, a.kind AS agent_kind, a.host AS agent_host, a.owner_id AS agent_owner,
        u.login AS owner_login, pol.scope AS policy_scope, pol.priority AS policy_priority, pol.applies_to AS policy_applies_to,
        pol.category AS policy_category,
        (SELECT t.created_by FROM runs t WHERE t.id = COALESCE(r.parent_id, r.id)) AS thread_owner
   FROM approvals p
   LEFT JOIN agents a ON a.id = p.agent_id
   LEFT JOIN users u ON u.id = a.owner_id
   LEFT JOIN runs r ON r.id = p.run_id
   LEFT JOIN policies pol ON pol.id = p.policy_id";

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "opencode",
        _ => "Custom agent",
    }
}

fn row_to_org(r: &PgRow) -> OrgApproval {
    let tool_name: String = r.get("tool_name");
    let input: Value = r.get("input");
    let agent_id: Option<Uuid> = r.get("agent_id");
    let agent_owner: Option<Uuid> = r.get("agent_owner");
    let policy_id: Option<Uuid> = r.get("policy_id");
    let policy_name: Option<String> = r.get("policy_name");
    let category: Option<String> = r.get("policy_category");
    let kind: String = r.get("kind");
    let host: Option<String> = r.get("agent_host");
    let agent_kind: Option<String> = r.get("agent_kind");
    let command = command_text(&tool_name, &input);
    let title = match category.filter(|c| !c.is_empty()) {
        Some(c) => c,
        None if kind == "ask" => format!("Use {tool_name}"),
        None => policy_name.clone().unwrap_or_else(|| format!("Use {tool_name}")),
    };
    OrgApproval {
        id: r.get("id"),
        status: ApprovalStatus::parse(r.get::<&str, _>("status")).unwrap_or(ApprovalStatus::Cancelled),
        agent: agent_id.map(|id| json!({"id": id, "name": r.get::<Option<String>, _>("agent_name")})),
        owner: agent_owner.map(|id| json!({"id": id, "login": r.get::<Option<String>, _>("owner_login")})),
        runtime: match (&agent_kind, &host) {
            (Some(k), Some(h)) if !h.is_empty() => format!("{} on {h}", kind_label(k)),
            (Some(k), _) => kind_label(k).to_string(),
            _ => String::new(),
        },
        run_id: r.get("run_id"),
        title,
        tool: r.get::<Option<String>, _>("action").unwrap_or_else(|| tool_name.clone()),
        tool_name,
        command,
        digest: r.get("digest"),
        context: r.get("context"),
        policy: policy_id.map(|id| {
            json!({"id": id, "name": policy_name, "scope": r.get::<Option<String>, _>("policy_scope"),
                   "applies_to": r.get::<Option<String>, _>("policy_applies_to"), "priority": r.get::<Option<i32>, _>("policy_priority")})
        }),
        message: r.get("message"),
        decided_by: r.get("decided_by"),
        created_at: r.get("created_at"),
        expires_at: r.get("expires_at"),
        decided_at: r.get("decided_at"),
        can_decide: false,
        kind,
        agent_owner,
        thread_owner: r.get("thread_owner"),
    }
}

/// The org's approvals, newest first: `pending`, `resolved` or (None) both.
pub async fn list_org(pool: &PgPool, org: Uuid, status: Option<&str>) -> sqlx::Result<Vec<OrgApproval>> {
    let filter = match status {
        Some("pending") => "AND p.status = 'pending'",
        Some("resolved") => "AND p.status <> 'pending'",
        _ => "",
    };
    let rows = sqlx::query(&format!("{ORG_SELECT} WHERE p.org_id = $1 {filter} ORDER BY p.created_at DESC LIMIT 300"))
        .bind(org)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_org).collect())
}

pub async fn get_org(pool: &PgPool, org: Uuid, id: Uuid) -> sqlx::Result<Option<OrgApproval>> {
    let row = sqlx::query(&format!("{ORG_SELECT} WHERE p.org_id = $1 AND p.id = $2")).bind(org).bind(id).fetch_optional(pool).await?;
    Ok(row.as_ref().map(row_to_org))
}

pub async fn counts(pool: &PgPool, org: Uuid) -> sqlx::Result<(i64, i64)> {
    let r = sqlx::query("SELECT count(*) FILTER (WHERE status = 'pending') AS pending, count(*) FILTER (WHERE status <> 'pending') AS resolved FROM approvals WHERE org_id = $1")
        .bind(org)
        .fetch_one(pool)
        .await?;
    Ok((r.get("pending"), r.get("resolved")))
}

/// Expire the org's pending approvals whose deadline passed, so lists don't show them as
/// waiting. Returns the run events to publish.
pub async fn expire_overdue(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<RunEvent>> {
    let rows = sqlx::query("SELECT id, run_id FROM approvals WHERE org_id = $1 AND status = 'pending' AND expires_at <= now()")
        .bind(org)
        .fetch_all(pool)
        .await?;
    let mut out = Vec::new();
    for r in rows {
        let id: Uuid = r.get("id");
        match r.get::<Option<Uuid>, _>("run_id") {
            Some(run_id) => {
                if let Some((_, Some(ev))) = poll(pool, org, run_id, id, Utc::now()).await? {
                    out.push(ev);
                }
            }
            None => {
                poll_detached(pool, org, None, id).await?;
            }
        }
    }
    Ok(out)
}

// ---- approvals without a run: custom agents through the gateway ----

/// A custom agent's approval, as the gateway answers it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GatewayApproval {
    pub id: Uuid,
    pub status: ApprovalStatus,
    pub message: Option<String>,
    pub digest: Option<String>,
    pub expires_at: DateTime<Utc>,
}

fn row_to_gateway(r: &PgRow) -> GatewayApproval {
    GatewayApproval {
        id: r.get("id"),
        status: ApprovalStatus::parse(r.get::<&str, _>("status")).unwrap_or(ApprovalStatus::Cancelled),
        message: r.get("message"),
        digest: r.get("digest"),
        expires_at: r.get("expires_at"),
    }
}

pub async fn request_detached(pool: &PgPool, org: Uuid, tool_name: &str, input: &Value, timeout_secs: Option<u64>, hold: &Hold) -> sqlx::Result<GatewayApproval> {
    let timeout = timeout_secs.unwrap_or(DEFAULT_APPROVAL_TIMEOUT_SECS).clamp(1, MAX_APPROVAL_TIMEOUT_SECS);
    let mut tx = pool.begin().await?;
    let row = insert(&mut tx, org, None, tool_name, &redact_tool_input(input, MAX_INPUT_STRING_BYTES), None, timeout, hold).await?;
    record_requested(&mut tx, org, &row, hold).await?;
    tx.commit().await?;
    Ok(row_to_gateway(&row))
}

/// Read a custom agent's approval (`agent`: only its own); a pending one past its deadline expires.
pub async fn poll_detached(pool: &PgPool, org: Uuid, agent: Option<Uuid>, id: Uuid) -> sqlx::Result<Option<GatewayApproval>> {
    let mut tx = pool.begin().await?;
    let Some(row) = sqlx::query(&format!(
        "SELECT {COLS}, {EXTRA} FROM approvals WHERE id = $1 AND org_id = $2 AND run_id IS NULL AND ($3::uuid IS NULL OR agent_id = $3) FOR UPDATE"
    ))
    .bind(id)
    .bind(org)
    .bind(agent)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(None);
    };
    let status = ApprovalStatus::parse(row.get::<&str, _>("status"));
    let expires: DateTime<Utc> = row.get("expires_at");
    if status == Some(ApprovalStatus::Pending) && expires <= Utc::now() {
        let a = row_to_approval_detached_msg(&row);
        let row = set_status(&mut tx, id, ApprovalStatus::Expired, Some(&a), None).await?;
        record_decided(&mut tx, &row).await?;
        tx.commit().await?;
        return Ok(Some(row_to_gateway(&row)));
    }
    Ok(Some(row_to_gateway(&row)))
}

fn row_to_approval_detached_msg(r: &PgRow) -> String {
    let created: DateTime<Utc> = r.get("created_at");
    let expires: DateTime<Utc> = r.get("expires_at");
    let mins = ((expires - created).num_seconds().max(1) as f64 / 60.0).round().max(1.0);
    format!("Nobody approved this action within {mins} minutes, so it was denied.")
}

/// Decide any approval of the org: one with a run goes through [`decide`]; one without
/// changes under its own row lock.
pub async fn decide_any(pool: &PgPool, org: Uuid, id: Uuid, approve: bool, by: &str, message: Option<&str>) -> sqlx::Result<DecideOutcome> {
    let run_id: Option<Option<Uuid>> = sqlx::query_scalar("SELECT run_id FROM approvals WHERE id = $1 AND org_id = $2").bind(id).bind(org).fetch_optional(pool).await?;
    match run_id {
        None => Ok(DecideOutcome::NotFound),
        Some(Some(run_id)) => decide(pool, org, run_id, id, approve, by, message).await,
        Some(None) => {
            let mut tx = pool.begin().await?;
            let row = sqlx::query(&format!("SELECT {COLS}, {EXTRA} FROM approvals WHERE id = $1 AND org_id = $2 FOR UPDATE"))
                .bind(id)
                .bind(org)
                .fetch_one(&mut *tx)
                .await?;
            let status = ApprovalStatus::parse(row.get::<&str, _>("status")).unwrap_or(ApprovalStatus::Cancelled);
            let overdue = row.get::<DateTime<Utc>, _>("expires_at") <= Utc::now();
            let action = if approve { ApprovalAction::Approve } else { ApprovalAction::Deny };
            let Some(to) = status.after(action, overdue) else {
                return Ok(DecideOutcome::AlreadyDecidedDetached(status));
            };
            let msg = match to {
                ApprovalStatus::Denied => Some(message.map(str::trim).filter(|m| !m.is_empty()).map(|m| m.chars().take(1000).collect()).unwrap_or_else(|| DENIED_MESSAGE.to_string())),
                ApprovalStatus::Expired => Some(row_to_approval_detached_msg(&row)),
                _ => None,
            };
            let by = (to != ApprovalStatus::Expired).then_some(by);
            let row = set_status(&mut tx, id, to, msg.as_deref(), by).await?;
            record_decided(&mut tx, &row).await?;
            tx.commit().await?;
            Ok(if to == ApprovalStatus::Expired { DecideOutcome::ExpiredDetached } else { DecideOutcome::DecidedDetached })
        }
    }
}

/// Cancel an agent's pending approvals (it was suspended). Returns how many, and the run
/// events to publish.
pub async fn cancel_for_agent(pool: &PgPool, org: Uuid, agent: Uuid, message: &str) -> sqlx::Result<(i32, Vec<RunEvent>)> {
    let rows = sqlx::query("SELECT id, run_id FROM approvals WHERE org_id = $1 AND agent_id = $2 AND status = 'pending' ORDER BY created_at")
        .bind(org)
        .bind(agent)
        .fetch_all(pool)
        .await?;
    let (mut n, mut events) = (0, Vec::new());
    for r in rows {
        let id: Uuid = r.get("id");
        let mut tx = pool.begin().await?;
        match r.get::<Option<Uuid>, _>("run_id") {
            Some(run_id) => {
                if lock_run(&mut tx, Some(org), run_id).await?.is_none() {
                    continue;
                }
                let Some(current) = lock_approval(&mut tx, org, run_id, id).await? else { continue };
                if current.status != ApprovalStatus::Pending {
                    continue;
                }
                let (_, ev) = transition(&mut tx, &current, ApprovalStatus::Cancelled, Some(message), None).await?;
                events.push(ev);
            }
            None => {
                let pending: bool = sqlx::query_scalar("SELECT status = 'pending' FROM approvals WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
                if !pending {
                    continue;
                }
                let row = set_status(&mut tx, id, ApprovalStatus::Cancelled, Some(message), None).await?;
                record_decided(&mut tx, &row).await?;
            }
        }
        tx.commit().await?;
        n += 1;
    }
    Ok((n, events))
}
