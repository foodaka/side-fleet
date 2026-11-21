//! The audit log: who did what in an org, appended as it happens (docs/multiplayer/08).
//!
//! State changes the control plane makes on its own (a worker claiming, finishing or
//! losing a run; an approval being asked for, expiring or cancelled) are recorded in the
//! same transaction as the change. Actions someone takes on the page or the API are
//! recorded by the route once the change is made, with [`record_after`]: a failure to write
//! the record is logged and never undoes or fails what the person did.
//!
//! Nothing secret is stored: `detail`, `target` and `actor` go through the same redaction
//! as run events, and strings in `detail` are clipped. Task text, agent output and tool
//! input are never put in `detail`; an approval carries only the one-line redacted summary
//! its card already shows.
//!
//! Who reads it: owners of the org (and the developer key) see all of it. Everyone else
//! sees the entries about their own machines and the things they did themselves.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidefleet_core::redact::{redact, redact_json};
use sqlx::{postgres::PgRow, PgExecutor, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::auth::Caller;
use crate::tenancy::OrgMembership;
use crate::AppState;

/// Strings in `detail` are clipped to this many bytes.
pub const MAX_DETAIL_STRING_BYTES: usize = 500;
/// A page of the log holds at most this many events.
pub const MAX_PAGE: i64 = 200;
const DEFAULT_PAGE: i64 = 50;
/// An export holds at most this many events (newest first); narrow the filter for more.
pub const MAX_EXPORT: i64 = 10_000;

/// Every action written today. Stable names: the API filter and exports use them.
pub const ACTIONS: &[&str] = &[
    // Runs
    "run.created",
    "run.claimed",
    "run.started",
    "run.finished",
    "run.stopped",
    "run.stopped_by_other",
    "run.requeued",
    "run.moved",
    "run.permission_mode_changed",
    // Tool approvals ("Ask me")
    "approval.requested",
    "approval.approved",
    "approval.denied",
    "approval.timed_out",
    "approval.cancelled",
    // Machines
    "machine.enrolled",
    "machine.enrollment_approved",
    "machine.enrollment_denied",
    "machine.removed",
    // Tokens
    "worker_token.created",
    "worker_token.revoked",
    "api_token.created",
    "api_token.revoked",
    // People and the org
    "org.created",
    "org.renamed",
    "invite.created",
    "invite.revoked",
    "member.added",
    "member.joined",
    "member.role_changed",
    "member.removed",
    "member.left",
];

/// Reserved: not written yet, but accepted by the filter so a client can ask for them now and
/// the names don't change when they are. Spend and payment events carry what they need in
/// `detail` (for example `amount`, `currency`, `payee`, `decision`, `reason`); no schema change.
pub const RESERVED_ACTIONS: &[&str] = &[
    "budget.authorized",
    "budget.exceeded",
    "payment.requested",
    "payment.made",
    "payment.refused",
    "payment.refunded",
    "handoff.created",
    "handoff.accepted",
    "handoff.declined",
    "machine.claimed",
    "machine.reassigned",
    "machine.moved",
    "thread.made_private",
    "thread.made_team",
];

/// Whether `action` names a known action (written or reserved), or a group of them
/// (`run.` or `run.*`: every action that starts with `run.`).
pub fn valid_action_filter(action: &str) -> bool {
    let all = || ACTIONS.iter().chain(RESERVED_ACTIONS);
    match action.strip_suffix('*').or_else(|| action.ends_with('.').then_some(action)) {
        Some(prefix) => prefix.ends_with('.') && all().any(|a| a.starts_with(prefix)),
        None => all().any(|a| *a == action),
    }
}

// ---- writing ----

/// Who did it. `name` is what the log shows: a login, `developer key`, `worker:<id>` or `system`.
#[derive(Debug, Clone)]
pub struct Actor {
    pub id: Option<Uuid>,
    pub name: String,
    /// The API token it came through, if any (recorded in `detail.api_token`).
    pub token: Option<Uuid>,
}

impl Actor {
    /// A person on the page, an API token (as its user), or the developer key.
    pub fn caller(c: &Caller) -> Actor {
        Actor {
            id: c.user_id(),
            name: c.user.as_ref().map_or_else(|| DEVELOPER_KEY.to_string(), |u| u.login.clone()),
            token: c.token,
        }
    }
    pub fn worker(id: &str) -> Actor {
        Actor { id: None, name: format!("worker:{id}"), token: None }
    }
    /// The control plane itself: a deadline passing, a machine going quiet.
    pub fn system() -> Actor {
        Actor { id: None, name: SYSTEM.to_string(), token: None }
    }
}

pub const DEVELOPER_KEY: &str = "developer key";
pub const SYSTEM: &str = "system";

/// One entry to append.
#[derive(Debug, Clone)]
pub struct Entry {
    pub action: &'static str,
    pub target: Option<String>,
    pub run_id: Option<Uuid>,
    pub worker_id: Option<String>,
    pub detail: Value,
}

impl Entry {
    pub fn new(action: &'static str) -> Entry {
        debug_assert!(ACTIONS.contains(&action), "unknown audit action {action}");
        Entry { action, target: None, run_id: None, worker_id: None, detail: json!({}) }
    }
    /// About a run; it is also the target unless one is set.
    pub fn run(mut self, id: Uuid) -> Entry {
        self.run_id = Some(id);
        self.target.get_or_insert_with(|| id.to_string());
        self
    }
    /// On a machine; it is the target unless one is set.
    pub fn worker(mut self, id: Option<&str>) -> Entry {
        if let Some(id) = id {
            self.worker_id = Some(id.to_string());
            self.target.get_or_insert_with(|| id.to_string());
        }
        self
    }
    pub fn target(mut self, t: impl ToString) -> Entry {
        self.target = Some(t.to_string());
        self
    }
    /// Merge these keys into the detail (later keys win). Nulls are left out.
    pub fn detail(mut self, v: Value) -> Entry {
        if let (Some(d), Value::Object(new)) = (self.detail.as_object_mut(), v) {
            d.extend(new.into_iter().filter(|(_, v)| !v.is_null()));
        }
        self
    }
}

/// What actually gets stored as `detail`: redacted, strings clipped, the API token named.
pub fn stored_detail(actor: &Actor, detail: &Value) -> Value {
    let mut d = sidefleet_core::redact_tool_input(detail, MAX_DETAIL_STRING_BYTES);
    if !d.is_object() {
        d = json!({});
    }
    if let Some(t) = actor.token {
        d["api_token"] = json!(t);
    }
    d
}

fn clip(s: &str, max: usize) -> String {
    sidefleet_core::stream::truncate_bytes(&redact(s), max)
}

/// Append an entry, in a transaction or not.
pub async fn record<'e, E: PgExecutor<'e>>(ex: E, org: Uuid, actor: &Actor, e: Entry) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO audit_events (org_id, actor_id, actor, action, target, run_id, worker_id, detail)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(org)
    .bind(actor.id)
    .bind(clip(&actor.name, 200))
    .bind(e.action)
    .bind(e.target.as_deref().map(|t| clip(t, 200)))
    .bind(e.run_id)
    .bind(e.worker_id.as_deref().map(|w| clip(w, 200)))
    .bind(stored_detail(actor, &e.detail))
    .execute(ex)
    .await?;
    Ok(())
}

/// Append an entry for something that has already happened: a failure is logged, not returned,
/// so it never fails (and invites a retry of) what the person did.
pub async fn record_after(pool: &PgPool, org: Uuid, actor: &Actor, e: Entry) {
    let action = e.action;
    if let Err(err) = record(pool, org, actor, e).await {
        tracing::error!(error = %err, %org, action, "could not write the audit log");
    }
}

// ---- reading ----

#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub actor: String,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub target: Option<String>,
    pub run_id: Option<Uuid>,
    pub worker_id: Option<String>,
    pub detail: Value,
}

fn row_to_event(r: &PgRow) -> AuditEvent {
    AuditEvent {
        id: r.get("id"),
        at: r.get("at"),
        actor: r.get("actor"),
        actor_id: r.get("actor_id"),
        action: r.get("action"),
        target: r.get("target"),
        run_id: r.get("run_id"),
        worker_id: r.get("worker_id"),
        // Stored redacted; redacted again on the way out, as run events are.
        detail: redact_json(&r.get::<Value, _>("detail")),
    }
}

/// Which entries a caller may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// The whole org's (owners, the developer key).
    Org,
    /// Those about the user's own machines in the org, and those they did.
    Own(Uuid),
    /// None (a member acting without a person, which doesn't happen today).
    Nothing,
}

pub fn visibility(caller: &Caller, org: &OrgMembership) -> Visibility {
    if org.role == "owner" {
        Visibility::Org
    } else {
        caller.user_id().map_or(Visibility::Nothing, Visibility::Own)
    }
}

/// A filter on the log; every part is optional.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Filter {
    /// Entries about this run.
    #[serde(default)]
    pub run: Option<Uuid>,
    /// Entries about any run of the thread rooted at this run.
    #[serde(default)]
    pub thread: Option<Uuid>,
    /// Entries on this machine (its id).
    #[serde(default)]
    pub machine: Option<String>,
    /// Who did it: a login, `developer key`, `worker:<id>`, `system`, or a user id.
    #[serde(default)]
    pub actor: Option<String>,
    /// An action, or a group of them (`run.` / `run.*`).
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    /// From this time (inclusive).
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    /// Before this time (exclusive).
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    /// The page after the entry with this id (the `next` of the page before).
    #[serde(default)]
    pub before: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    /// Exports only: `csv` (default) or `json`.
    #[serde(default)]
    pub format: Option<String>,
}

impl Filter {
    /// Trim, drop empty parts, and check what can be checked.
    pub fn clean(mut self) -> Result<Filter, String> {
        let tidy = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self.machine = tidy(self.machine);
        self.actor = tidy(self.actor);
        self.action = tidy(self.action);
        self.target = tidy(self.target);
        if let Some(a) = &self.action {
            if !valid_action_filter(a) {
                return Err(format!("unknown action {a:?}; see docs/API.md for the list"));
            }
        }
        if let (Some(s), Some(u)) = (self.since, self.until) {
            if s >= u {
                return Err("since must be before until".into());
            }
        }
        if [&self.machine, &self.actor, &self.target].into_iter().flatten().any(|s| s.len() > 200) {
            return Err("filter values are at most 200 characters".into());
        }
        Ok(self)
    }
}

/// Entries of `org` matching `f` that `who` may read, newest first, at most `limit`
/// (plus one more, to tell whether there is a next page).
pub async fn list(pool: &PgPool, org: Uuid, who: Visibility, f: &Filter, limit: i64) -> sqlx::Result<Vec<AuditEvent>> {
    let mut q: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT id, at, actor, actor_id, action, target, run_id, worker_id, detail FROM audit_events e WHERE e.org_id = ");
    q.push_bind(org);
    match who {
        Visibility::Org => {}
        Visibility::Own(user) => {
            q.push(" AND (e.actor_id = ");
            q.push_bind(user);
            q.push(" OR e.worker_id IN (SELECT w.id FROM workers w WHERE w.org_id = e.org_id AND w.owner_id = ");
            q.push_bind(user);
            q.push("))");
        }
        Visibility::Nothing => {
            q.push(" AND false");
        }
    }
    if let Some(r) = f.run {
        q.push(" AND e.run_id = ");
        q.push_bind(r);
    }
    if let Some(t) = f.thread {
        q.push(" AND e.run_id IN (SELECT r.id FROM runs r WHERE r.org_id = e.org_id AND (r.id = ");
        q.push_bind(t);
        q.push(" OR r.parent_id = ");
        q.push_bind(t);
        q.push("))");
    }
    if let Some(m) = &f.machine {
        q.push(" AND e.worker_id = ");
        q.push_bind(m.clone());
    }
    if let Some(a) = &f.actor {
        match a.parse::<Uuid>() {
            Ok(id) => {
                q.push(" AND e.actor_id = ");
                q.push_bind(id);
            }
            Err(_) => {
                q.push(" AND lower(e.actor) = lower(");
                q.push_bind(a.trim_start_matches('@').to_string());
                q.push(")");
            }
        }
    }
    if let Some(a) = &f.action {
        match a.strip_suffix('*').or_else(|| a.ends_with('.').then_some(a.as_str())) {
            Some(prefix) => {
                // Prefixes are checked against the known names, so they hold no LIKE wildcards.
                q.push(" AND e.action LIKE ");
                q.push_bind(format!("{prefix}%"));
            }
            None => {
                q.push(" AND e.action = ");
                q.push_bind(a.clone());
            }
        }
    }
    if let Some(t) = &f.target {
        q.push(" AND e.target = ");
        q.push_bind(t.clone());
    }
    if let Some(s) = f.since {
        q.push(" AND e.at >= ");
        q.push_bind(s);
    }
    if let Some(u) = f.until {
        q.push(" AND e.at < ");
        q.push_bind(u);
    }
    if let Some(b) = f.before {
        // Keyset: strictly older than that entry, in the same order as the page.
        q.push(" AND (e.at, e.id) < (SELECT c.at, c.id FROM audit_events c WHERE c.id = ");
        q.push_bind(b);
        q.push(" AND c.org_id = e.org_id)");
    }
    q.push(" ORDER BY e.at DESC, e.id DESC LIMIT ");
    q.push_bind(limit + 1);
    let rows = q.build().fetch_all(pool).await?;
    Ok(rows.iter().map(row_to_event).collect())
}

/// A page of entries and the cursor for the next one.
#[derive(Debug, Serialize)]
pub struct Page {
    pub events: Vec<AuditEvent>,
    /// Pass as `before` for the next (older) page; null on the last page.
    pub next: Option<i64>,
}

pub async fn page(pool: &PgPool, org: Uuid, who: Visibility, f: &Filter) -> sqlx::Result<Page> {
    let limit = f.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let mut events = list(pool, org, who, f, limit).await?;
    let more = events.len() as i64 > limit;
    events.truncate(limit as usize);
    let next = if more { events.last().map(|e| e.id) } else { None };
    Ok(Page { events, next })
}

// ---- export ----

/// A CSV field: quoted when it needs to be, and a leading `=`, `+`, `-`, `@` or tab is
/// prefixed with `'` so a spreadsheet shows it rather than running it as a formula.
fn csv_field(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

pub const CSV_HEADER: &str = "id,at,actor,actor_id,action,target,run_id,worker_id,detail";

pub fn to_csv(events: &[AuditEvent]) -> String {
    let mut out = String::with_capacity(64 * (events.len() + 1));
    out.push_str(CSV_HEADER);
    out.push_str("\r\n");
    for e in events {
        let opt = |v: Option<String>| v.unwrap_or_default();
        let fields = [
            e.id.to_string(),
            e.at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            e.actor.clone(),
            opt(e.actor_id.map(|u| u.to_string())),
            e.action.clone(),
            opt(e.target.clone()),
            opt(e.run_id.map(|u| u.to_string())),
            opt(e.worker_id.clone()),
            e.detail.to_string(),
        ];
        out.push_str(&fields.iter().map(String::as_str).map(csv_field).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

// ---- HTTP ----

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

/// Developer routes, behind developer auth. API tokens can't reach them (`auth::scope_allows`).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/orgs/{org}/audit", get(get_org_audit))
        .route("/v1/orgs/{org}/audit/export", get(export_org_audit))
        .route("/v1/workers/{id}/audit", get(get_worker_audit))
}

/// The org named in the path: the caller's current org only, so the log is always the one
/// of the org the request acts in.
fn current_org(caller: &Caller, org: &str) -> Result<OrgMembership, ApiError> {
    if caller.org.id.to_string() == org || caller.org.slug == org {
        Ok(caller.org.clone())
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "org not found".into()))
    }
}

fn clean(f: Filter) -> Result<Filter, ApiError> {
    f.clean().map_err(|m| ApiError(StatusCode::BAD_REQUEST, m))
}

async fn get_org_audit(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(org): Path<String>, Query(f): Query<Filter>) -> Result<Json<Page>, ApiError> {
    let org = current_org(&caller, &org)?;
    let f = clean(f)?;
    Ok(Json(page(&state.pool, org.id, visibility(&caller, &org), &f).await?))
}

/// The same filtered entries as a download: `format=csv` (default) or `json`. Newest first,
/// at most [`MAX_EXPORT`]; `X-Sidefleet-Truncated: true` says there were more.
async fn export_org_audit(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(org): Path<String>, Query(f): Query<Filter>) -> Result<Response, ApiError> {
    let org = current_org(&caller, &org)?;
    let csv = match f.format.as_deref().map(str::trim) {
        None | Some("") | Some("csv") => true,
        Some("json") => false,
        Some(_) => return Err(ApiError(StatusCode::BAD_REQUEST, "format must be csv or json".into())),
    };
    let f = clean(f)?;
    let mut events = list(&state.pool, org.id, visibility(&caller, &org), &f, MAX_EXPORT).await?;
    let truncated = events.len() as i64 > MAX_EXPORT;
    events.truncate(MAX_EXPORT as usize);
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let (body, kind, ext) = if csv {
        (to_csv(&events), "text/csv; charset=utf-8", "csv")
    } else {
        (serde_json::to_string(&events).unwrap_or_else(|_| "[]".into()), "application/json", "json")
    };
    tracing::info!(org = %org.id, rows = events.len(), truncated, format = ext, "audit log exported");
    Ok((
        [
            (header::CONTENT_TYPE, kind.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"sidefleet-audit-{}-{stamp}.{ext}\"", org.slug)),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::HeaderName::from_static("x-sidefleet-truncated"), truncated.to_string()),
        ],
        body,
    )
        .into_response())
}

/// One machine's entries ("History" on its card): for its owner, and the org's owners.
async fn get_worker_audit(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<String>, Query(mut f): Query<Filter>) -> Result<Json<Page>, ApiError> {
    let org = caller.org.clone();
    let exists = crate::db::worker_exists(&state.pool, org.id, &id).await?;
    let owner = crate::db::worker_owner(&state.pool, org.id, &id).await?;
    let who = match visibility(&caller, &org) {
        Visibility::Org => Visibility::Org,
        // Their own machine: all of its entries, whoever caused them.
        Visibility::Own(me) if exists && owner == Some(me) => Visibility::Org,
        _ if !exists => return Err(ApiError(StatusCode::NOT_FOUND, "unknown machine".into())),
        _ => return Err(ApiError(StatusCode::FORBIDDEN, "only the machine's owner (or an owner of the org) can see its history".into())),
    };
    f.machine = Some(id);
    let f = clean(f)?;
    Ok(Json(page(&state.pool, org.id, who, &f).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_filters() {
        assert!(valid_action_filter("run.created"));
        assert!(valid_action_filter("payment.made"), "reserved names are accepted");
        assert!(valid_action_filter("run."));
        assert!(valid_action_filter("run.*"));
        assert!(valid_action_filter("budget.*"));
        assert!(!valid_action_filter("run"));
        assert!(!valid_action_filter("*"));
        assert!(!valid_action_filter("nope.*"));
        assert!(!valid_action_filter("run.%"));
        assert!(!valid_action_filter("run.created'"));
    }

    #[test]
    fn names_are_dotted_and_unique() {
        let all: Vec<&str> = ACTIONS.iter().chain(RESERVED_ACTIONS).copied().collect();
        for a in &all {
            assert!(a.split('.').count() == 2 && a.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'), "{a}");
        }
        let mut sorted = all.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "an action is listed twice");
    }

    #[test]
    fn detail_is_redacted_and_clipped() {
        let actor = Actor { id: None, name: "alice".into(), token: Some(Uuid::nil()) };
        let d = stored_detail(
            &actor,
            &json!({
                "summary": "curl -H 'Authorization: Bearer abcdefghijklmnop' https://x",
                "api_key": "plain-but-named-secret",
                "nested": {"cmd": "export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz1234"},
                "long": "x".repeat(5000),
            }),
        );
        let s = d.to_string();
        assert!(!s.contains("abcdefghijklmnop"), "{s}");
        assert!(!s.contains("plain-but-named-secret"), "{s}");
        assert!(!s.contains("ghp_abc"), "{s}");
        // Clipped, plus the "… [truncated N bytes]" marker.
        assert!(d["long"].as_str().unwrap().len() < MAX_DETAIL_STRING_BYTES + 40);
        assert_eq!(d["api_token"], json!(Uuid::nil()));
        assert_eq!(stored_detail(&Actor::system(), &json!("not an object")), json!({}));
    }

    #[test]
    fn entries_default_their_target() {
        let run = Uuid::new_v4();
        let e = Entry::new("run.claimed").run(run).worker(Some("mac"));
        assert_eq!(e.target.as_deref(), Some(run.to_string().as_str()));
        assert_eq!(e.worker_id.as_deref(), Some("mac"));
        let e = Entry::new("machine.removed").worker(Some("mac")).detail(json!({"a": 1, "b": null}));
        assert_eq!(e.target.as_deref(), Some("mac"));
        assert_eq!(e.detail, json!({"a": 1}));
    }

    #[test]
    fn csv_is_quoted_and_formula_safe() {
        let e = AuditEvent {
            id: 7,
            at: DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z").unwrap().with_timezone(&Utc),
            actor: "=HYPERLINK(\"x\")".into(),
            actor_id: None,
            action: "run.created".into(),
            target: Some("a,b".into()),
            run_id: None,
            worker_id: None,
            detail: json!({"k": "v"}),
        };
        let csv = to_csv(&[e]);
        let line = csv.lines().nth(1).unwrap();
        assert!(line.starts_with("7,2026-01-02T03:04:05.000Z,\"'=HYPERLINK(\"\"x\"\")\",,run.created,\"a,b\",,,"), "{line}");
        assert!(line.ends_with("\"{\"\"k\"\":\"\"v\"\"}\""), "{line}");
        assert!(csv.starts_with(CSV_HEADER));
    }

    #[test]
    fn filters_are_checked() {
        assert!(Filter { action: Some("bogus".into()), ..Default::default() }.clean().is_err());
        let now = Utc::now();
        assert!(Filter { since: Some(now), until: Some(now), ..Default::default() }.clean().is_err());
        let f = Filter { actor: Some("  ".into()), action: Some(" run.* ".into()), ..Default::default() }.clean().unwrap();
        assert_eq!(f.actor, None);
        assert_eq!(f.action.as_deref(), Some("run.*"));
    }
}
