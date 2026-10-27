//! The "waiting on you" inbox: tasks that need a tool call approved, finished,
//! failed, paused on a usage limit or lost their machine, until the developer
//! opens or dismisses them (or, for an approval, it is answered).
//!
//! Attention is derived from runs and their events (see [`crate::attention`]);
//! only what each viewer has seen is stored. A viewer is the signed-in user, or
//! the nil uuid for the developer key. Changes reach the page over
//! `GET /v1/inbox/events` and, for browsers that opted in, as a web push.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sidefleet_core::{summarize_tool_input, RunStatus};
use sqlx::{PgPool, Row};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::attention::{self, Attention, Reason, ThreadFacts};
use crate::auth::Caller;
use crate::push;
use crate::AppState;

/// Run event kinds that can move a task in or out of the inbox. The inbox
/// recomputes the task's state when one is published. State the inbox reads that
/// changes without one of these needs a call to [`touch`].
pub const TRIGGER_KINDS: &[&str] = &["status", "usage_limit", "approval_requested", "approval_decided"];

/// Tasks whose latest run is older than this are not looked at.
const WINDOW_DAYS: i32 = 30;
/// At most this many tasks are listed.
const MAX_ITEMS: i64 = 200;
/// A task must have started waiting this recently to be pushed (not on old news after a restart).
const PUSH_FRESH_SECS: i64 = 300;
/// Wait this long before pushing, so a task that is picked up again or opened right away stays quiet.
const PUSH_DELAY: Duration = Duration::from_secs(10);

/// The viewer whose "seen" marks a request reads and writes.
pub fn viewer(caller: &Caller) -> Uuid {
    caller.user_id().unwrap_or(Uuid::nil())
}

/// A task in the inbox, as the page lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InboxItem {
    /// The task (root run of the thread).
    pub id: Uuid,
    /// Its latest run.
    pub run_id: Uuid,
    pub title: String,
    pub repo: String,
    pub agent: String,
    pub reason: Reason,
    pub label: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// When it started waiting.
    pub since: DateTime<Utc>,
    /// Who started it: a task waits on them, not on the whole team (None: anyone in the org).
    #[serde(skip)]
    pub owner: Option<Uuid>,
}

/// Live state: per-org change signals and what was pushed.
#[derive(Clone)]
pub struct Inbox {
    inner: Arc<Inner>,
}

struct Inner {
    orgs: Mutex<HashMap<Uuid, broadcast::Sender<()>>>,
    /// Last attention pushed per task, so one wait is pushed once.
    pushed: Mutex<HashMap<Uuid, (Reason, DateTime<Utc>)>>,
    push_delay: Mutex<Duration>,
    any_push_host: AtomicBool,
    http: reqwest::Client,
}

impl Default for Inbox {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                orgs: Mutex::default(),
                pushed: Mutex::default(),
                push_delay: Mutex::new(PUSH_DELAY),
                any_push_host: AtomicBool::new(false),
                http: reqwest::Client::new(),
            }),
        }
    }
}

impl Inbox {
    fn subscribe(&self, org: Uuid) -> broadcast::Receiver<()> {
        self.inner.orgs.lock().unwrap().entry(org).or_insert_with(|| broadcast::channel(16).0).subscribe()
    }

    /// Tell the org's open pages to look again.
    pub fn signal(&self, org: Uuid) {
        if let Some(tx) = self.inner.orgs.lock().unwrap().get(&org) {
            let _ = tx.send(());
        }
    }

    fn signal_all(&self) {
        for tx in self.inner.orgs.lock().unwrap().values() {
            let _ = tx.send(());
        }
    }

    /// Tests: push sooner, and to any host (a stand-in push service on localhost).
    pub fn set_push_delay(&self, d: Duration) {
        *self.inner.push_delay.lock().unwrap() = d;
    }
    pub fn allow_any_push_host(&self, yes: bool) {
        self.inner.any_push_host.store(yes, Ordering::Relaxed);
    }

    /// Record that this wait is being pushed; false if it already was.
    fn mark_pushed(&self, root: Uuid, key: (Reason, DateTime<Utc>)) -> bool {
        let mut m = self.inner.pushed.lock().unwrap();
        if m.get(&root) == Some(&key) {
            return false;
        }
        let cutoff = Utc::now() - chrono::Duration::hours(1);
        m.retain(|_, (_, since)| *since > cutoff);
        m.insert(root, key);
        true
    }
}

// ---- deriving attention ----

/// Approvals the developer has not answered, per task: since when (the oldest
/// pending one) and what for (its tool and a redacted one-line summary).
///
/// One past its deadline counts as answered: it only becomes `expired` when the
/// worker next reads it, and the agent has stopped waiting by then.
pub async fn pending_approvals(pool: &PgPool, org: Uuid) -> sqlx::Result<HashMap<Uuid, (DateTime<Utc>, String)>> {
    let rows = sqlx::query(
        "SELECT DISTINCT ON (root_id) coalesce(r.parent_id, r.id) AS root_id, a.created_at, a.tool_name, a.input
         FROM approvals a JOIN runs r ON r.id = a.run_id
         WHERE a.status = 'pending' AND a.expires_at > now() AND a.org_id = $1 AND r.org_id = $1
         ORDER BY root_id, a.created_at",
    )
    .bind(org)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let tool: String = r.get("tool_name");
            let summary = summarize_tool_input(&tool, &r.get::<serde_json::Value, _>("input"));
            let what = if summary.is_empty() { tool } else { format!("{tool}: {summary}") };
            (r.get("root_id"), (r.get("created_at"), what))
        })
        .collect())
}

/// Every task of `org` (or just `root`) that waits on someone: those waiting for
/// an approval first, then newest first.
async fn waiting(pool: &PgPool, org: Uuid, root: Option<Uuid>) -> sqlx::Result<Vec<InboxItem>> {
    let rows = sqlx::query(
        "SELECT root.id AS root_id, root.task AS title, root.repo, root.created_by, l.id AS run_id, l.agent, l.status, l.kind,
                (l.moved_from IS NOT NULL) AS moved,
                se.payload->>'reason' AS status_reason, coalesce(se.at, l.created_at) AS status_at,
                ul.payload->>'until' AS limit_until
         FROM runs root
         CROSS JOIN LATERAL (
             SELECT id, agent, status, kind, moved_from, created_at, finished_at FROM runs l
             WHERE l.id = root.id OR l.parent_id = root.id ORDER BY l.created_at DESC LIMIT 1) l
         LEFT JOIN LATERAL (
             SELECT payload, at FROM run_events e WHERE e.run_id = l.id AND e.kind = 'status' ORDER BY e.seq DESC LIMIT 1) se ON true
         LEFT JOIN LATERAL (
             SELECT payload FROM run_events e WHERE e.run_id = l.id AND e.kind = 'usage_limit'
               AND e.seq > coalesce((SELECT max(s.seq) FROM run_events s
                                     WHERE s.run_id = l.id AND s.kind = 'status' AND s.payload->>'status' = 'running'), -1)
             ORDER BY e.seq DESC LIMIT 1) ul ON true
         WHERE root.parent_id IS NULL AND root.org_id = $1 AND root.archived_at IS NULL
           AND ($2::uuid IS NULL OR root.id = $2)
           AND coalesce(l.finished_at, l.created_at) > now() - make_interval(days => $3)
         ORDER BY status_at DESC LIMIT $4",
    )
    .bind(org)
    .bind(root)
    .bind(WINDOW_DAYS)
    .bind(MAX_ITEMS)
    .fetch_all(pool)
    .await?;
    let mut approvals = pending_approvals(pool, org).await?;
    let mut out = Vec::new();
    for r in rows {
        let root_id: Uuid = r.get("root_id");
        let status: String = r.get("status");
        let facts = ThreadFacts {
            status: RunStatus::parse(&status).unwrap_or(RunStatus::Failed),
            kind: r.get("kind"),
            moved: r.get("moved"),
            status_reason: r.get("status_reason"),
            status_at: r.get("status_at"),
            usage_limit_until: r
                .get::<Option<String>, _>("limit_until")
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|t| t.with_timezone(&Utc)),
            approval: approvals.remove(&root_id),
        };
        if let Some(Attention { reason, since, detail }) = attention::compute(&facts) {
            out.push(InboxItem {
                id: root_id,
                run_id: r.get("run_id"),
                title: r.get("title"),
                repo: r.get("repo"),
                agent: r.get("agent"),
                reason,
                label: reason.label(),
                detail,
                since,
                owner: r.get("created_by"),
            });
        }
    }
    // An agent blocked on a tool call comes before everything else, then newest first.
    out.sort_by_key(|i| std::cmp::Reverse((i.reason == Reason::ApprovalNeeded, i.since)));
    Ok(out)
}

/// Give a viewer new to this org an empty inbox: what waited before they arrived is not theirs.
async fn ensure_viewer(pool: &PgPool, org: Uuid, viewer: Uuid) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO inbox_cleared (viewer_id, org_id, cleared_at) VALUES ($1, $2, now()) ON CONFLICT DO NOTHING")
        .bind(viewer)
        .bind(org)
        .execute(pool)
        .await?;
    Ok(())
}

/// The viewer's inbox in `org`: waiting tasks they have not seen since they started waiting.
pub async fn list(pool: &PgPool, org: Uuid, viewer: Uuid) -> sqlx::Result<Vec<InboxItem>> {
    let all = waiting(pool, org, None).await?;
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = all.iter().map(|i| i.id).collect();
    let seen: HashMap<Uuid, DateTime<Utc>> = sqlx::query("SELECT root_id, seen_at FROM inbox_seen WHERE viewer_id = $1 AND root_id = ANY($2)")
        .bind(viewer)
        .bind(&ids)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|r| (r.get("root_id"), r.get("seen_at")))
        .collect();
    let cleared: Option<DateTime<Utc>> = sqlx::query("SELECT cleared_at FROM inbox_cleared WHERE viewer_id = $1 AND org_id = $2")
        .bind(viewer)
        .bind(org)
        .fetch_optional(pool)
        .await?
        .map(|r| r.get("cleared_at"));
    Ok(all
        .into_iter()
        .filter(|i| i.owner.is_none_or(|o| o == viewer))
        .filter(|i| {
            let a = Attention { reason: i.reason, since: i.since, detail: None };
            attention::unseen(&a, seen.get(&i.id).copied().max(cleared))
        })
        .collect())
}

/// Mark a task seen (opened or dismissed). Any run of the thread may be named.
/// Returns the task, or None if it is not in `org`.
pub async fn dismiss(pool: &PgPool, org: Uuid, viewer: Uuid, id: Uuid) -> sqlx::Result<Option<Uuid>> {
    let row = sqlx::query(
        "INSERT INTO inbox_seen (viewer_id, root_id, seen_at)
         SELECT $1, coalesce(parent_id, id), now() FROM runs WHERE id = $2 AND org_id = $3
         ON CONFLICT (viewer_id, root_id) DO UPDATE SET seen_at = EXCLUDED.seen_at
         RETURNING root_id",
    )
    .bind(viewer)
    .bind(id)
    .bind(org)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get("root_id")))
}

pub async fn dismiss_all(pool: &PgPool, org: Uuid, viewer: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO inbox_cleared (viewer_id, org_id, cleared_at) VALUES ($1, $2, now())
         ON CONFLICT (viewer_id, org_id) DO UPDATE SET cleared_at = EXCLUDED.cleared_at",
    )
    .bind(viewer)
    .bind(org)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- reacting to run events ----

/// Follows every run event; when one can change a task's attention, the org's
/// pages are told and a task that just started waiting is pushed. Spawned by
/// [`AppState::new`].
pub async fn watch(state: AppState) {
    let mut rx = state.events.subscribe_all();
    loop {
        match rx.recv().await {
            Ok(t) if TRIGGER_KINDS.contains(&t.kind.as_str()) => {
                let found = sqlx::query("SELECT org_id, coalesce(parent_id, id) AS root FROM runs WHERE id = $1")
                    .bind(t.run_id)
                    .fetch_optional(&state.pool)
                    .await;
                match found {
                    Ok(Some(r)) => {
                        if let Err(e) = touch(&state, r.get("org_id"), r.get("root")).await {
                            tracing::warn!(error = %e, "inbox update failed");
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!(error = %e, "inbox lookup failed"),
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "inbox fell behind run events; refreshing every page");
                state.inbox.signal_all();
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// A task's attention may have changed: refresh the org's pages, and push it if it
/// just started waiting. Call this after changing state the inbox reads without a
/// run event (pending approvals, for one).
pub async fn touch(state: &AppState, org: Uuid, root: Uuid) -> sqlx::Result<()> {
    state.inbox.signal(org);
    let Some(vapid) = state.config.vapid.clone() else { return Ok(()) };
    let Some(w) = waiting(&state.pool, org, Some(root)).await?.pop() else { return Ok(()) };
    if Utc::now() - w.since > chrono::Duration::seconds(PUSH_FRESH_SECS) {
        return Ok(());
    }
    let key = (w.reason, w.since);
    let delay = *state.inbox.inner.push_delay.lock().unwrap();
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        // Still waiting the same way (not picked up again, not replied to)?
        let now = match waiting(&state.pool, org, Some(root)).await {
            Ok(mut v) => v.pop(),
            Err(e) => return tracing::warn!(error = %e, "push recheck failed"),
        };
        let Some(item) = now.filter(|i| (i.reason, i.since) == key) else { return };
        if !state.inbox.mark_pushed(root, key) {
            return;
        }
        let subs = match push::recipients(&state.pool, org, root, item.since).await {
            Ok(s) => s,
            Err(e) => return tracing::warn!(error = %e, "push recipients failed"),
        };
        let body = push::payload(root, &item.title, item.reason);
        for sub in subs {
            match push::send(&state.inbox.inner.http, &vapid, &sub, &body).await {
                push::Delivery::Sent => tracing::info!(task = %root, reason = ?item.reason, "push sent"),
                push::Delivery::Gone => {
                    tracing::info!(task = %root, "push subscription gone; forgetting it");
                    let _ = push::forget_endpoint(&state.pool, &sub.endpoint).await;
                }
                push::Delivery::Failed(e) => tracing::warn!(task = %root, error = %e, "push failed"),
            }
        }
    });
    Ok(())
}

// ---- HTTP ----

/// Developer routes; merged into the router behind developer auth.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/inbox", get(get_inbox))
        .route("/v1/inbox/events", get(inbox_events))
        .route("/v1/inbox/dismiss-all", post(post_dismiss_all))
        .route("/v1/inbox/{id}/dismiss", post(post_dismiss))
        .route("/v1/push", get(get_push))
        .route("/v1/push/subscriptions", post(subscribe).delete(unsubscribe))
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        crate::request_log::error_response(self.0, self.1)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database error".into())
    }
}

async fn get_inbox(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<serde_json::Value>, ApiError> {
    let v = viewer(&caller);
    ensure_viewer(&state.pool, caller.org_id(), v).await?;
    Ok(Json(json!({ "items": list(&state.pool, caller.org_id(), v).await? })))
}

/// SSE: an `inbox` event with the whole list now and whenever it changes.
async fn inbox_events(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (org, v) = (caller.org_id(), viewer(&caller));
    ensure_viewer(&state.pool, org, v).await?;
    let mut rx = state.inbox.subscribe(org);
    let pool = state.pool.clone();
    let stream = async_stream::stream! {
        let mut last = String::new();
        loop {
            match list(&pool, org, v).await {
                Ok(items) => {
                    let data = json!({ "items": items }).to_string();
                    if data != last {
                        yield Ok(Event::default().event("inbox").data(data.clone()));
                        last = data;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "inbox stream refresh failed"),
            }
            match rx.recv().await {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
            // Let a burst of changes (a finish writes several events) settle into one refresh.
            tokio::time::sleep(Duration::from_millis(150)).await;
            while rx.try_recv().is_ok() {}
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive")))
}

async fn post_dismiss(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    dismiss(&state.pool, caller.org_id(), viewer(&caller), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "run not found".into()))?;
    state.inbox.signal(caller.org_id());
    Ok(StatusCode::NO_CONTENT)
}

async fn post_dismiss_all(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<StatusCode, ApiError> {
    dismiss_all(&state.pool, caller.org_id(), viewer(&caller)).await?;
    state.inbox.signal(caller.org_id());
    Ok(StatusCode::NO_CONTENT)
}

/// Whether this control plane can push, and the key browsers subscribe with.
async fn get_push(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "available": state.config.vapid.is_some(),
        "public_key": state.config.vapid.as_ref().map(|v| v.public_key.clone()),
    }))
}

async fn subscribe(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(sub): Json<push::Subscription>) -> Result<StatusCode, ApiError> {
    if state.config.vapid.is_none() {
        return Err(ApiError(StatusCode::CONFLICT, "push is not set up on this control plane (no VAPID key)".into()));
    }
    push::check(&sub, state.inbox.inner.any_push_host.load(Ordering::Relaxed)).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.into()))?;
    push::save(&state.pool, viewer(&caller), &sub).await?;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct EndpointReq {
    endpoint: String,
}

async fn unsubscribe(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(req): Json<EndpointReq>) -> Result<StatusCode, ApiError> {
    push::delete(&state.pool, viewer(&caller), &req.endpoint).await?;
    Ok(StatusCode::NO_CONTENT)
}
