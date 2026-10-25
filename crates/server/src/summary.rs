//! A thread in a few lines, for scripts and chat agents that can't follow the
//! event stream: where it stands, what the agent last said, which files it
//! changed, what waits for approval and the pull request. `GET /v1/runs/:id/summary`.

use serde_json::{json, Value};
use sidefleet_core::{ApprovalStatus, RunStatus};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{approvals, db};

/// Longest reply text kept; chat agents get the end of a long one.
const MAX_REPLY_CHARS: usize = 4_000;
/// Changed files listed; the count is always the full one.
const MAX_FILES: usize = 30;

/// The thread `id` belongs to, summarised. None when the run is not in `org`.
pub async fn thread(pool: &PgPool, org: Uuid, id: Uuid, base: &str) -> sqlx::Result<Option<Value>> {
    let Some(run) = db::get_run(pool, org, id).await? else { return Ok(None) };
    let root_id = run.parent_id.unwrap_or(run.id);
    let runs = db::thread_runs(pool, org, root_id).await?;
    let (Some(root), Some(latest)) = (runs.first(), runs.last()) else { return Ok(None) };

    // What the agent last said, from the newest message that got a reply.
    let mut reply = None;
    let mut error = None;
    for r in runs.iter().rev().filter(|r| r.kind == "agent") {
        let events = db::events_since(pool, r.id, 0).await?;
        let text = |kind: &str, key: &str| {
            events.iter().rev().find(|e| e.kind == kind).and_then(|e| e.payload.get(key)?.as_str()).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
        };
        if r.id == latest.id && r.status == RunStatus::Failed {
            error = r.result.as_ref().and_then(|v| v.get("error")?.as_str().map(str::to_string)).or_else(|| text("error", "message"));
        }
        if let Some(t) = text("result", "result").or_else(|| text("assistant_text", "text")) {
            reply = Some(clip_start(&t, MAX_REPLY_CHARS));
            break;
        }
    }

    // Files changed in the thread so far: the newest diff's running total.
    let mut changed = json!({ "count": 0, "additions": 0, "deletions": 0, "files": [] });
    for r in runs.iter().rev() {
        let events = db::events_since(pool, r.id, 0).await?;
        let Some(d) = events.iter().rev().find(|e| e.kind == "diff") else { continue };
        let d = d.payload.get("total").filter(|t| !t.is_null()).or_else(|| d.payload.get("turn")).cloned().unwrap_or(Value::Null);
        let files: Vec<Value> = d["files"]
            .as_array()
            .map(|fs| {
                fs.iter()
                    .take(MAX_FILES)
                    .map(|f| json!({ "path": f["path"], "status": f["status"], "additions": f["additions"], "deletions": f["deletions"] }))
                    .collect()
            })
            .unwrap_or_default();
        changed = json!({ "count": d["file_count"], "additions": d["additions"], "deletions": d["deletions"], "files": files });
        break;
    }

    let pending: Vec<Value> = approvals::list(pool, org, latest.id)
        .await?
        .into_iter()
        .filter(|a| a.status == ApprovalStatus::Pending)
        .map(|a| json!({ "id": a.id, "tool": a.tool_name, "input": a.input }))
        .collect();

    Ok(Some(json!({
        "id": root.id,
        "task": root.task,
        "repo": root.repo,
        "agent": latest.agent,
        "model": latest.model,
        "machine": latest.worker_id.as_deref().or(latest.pinned_worker_id.as_deref()),
        "status": latest.status,
        "messages": runs.len(),
        "latest_message": { "id": latest.id, "task": latest.task, "kind": latest.kind, "started_at": latest.started_at, "finished_at": latest.finished_at },
        "reply": reply,
        "error": error,
        "changed_files": changed,
        "pending_approvals": pending,
        "pr": root.pr,
        "url": format!("{}/#/runs/{}", base.trim_end_matches('/'), root.id),
    })))
}

/// The last `max` characters of `s`, marked when cut.
fn clip_start(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - max).collect();
    format!("…{tail}")
}
