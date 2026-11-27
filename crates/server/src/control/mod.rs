//! The agent control plane: the agent inventory, activity, policies checked at the
//! tool-call boundary, approvals, a tamper-evident audit log and suspension.
//! See docs/API.md ("Control plane").
//!
//! Every query is scoped to the caller's org. Reads are open to members; policies,
//! suspension and reactivation need an org owner; approvals are decided by an org owner
//! or the agent's owner.

pub mod activity;
pub mod agents;
pub mod audit;
pub mod gateway;
pub mod policies;

use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sidefleet_core::redact_tool_input;
use uuid::Uuid;

use self::agents::{AgentInput, AgentRow, ReactivateOutcome, RegisterError, SuspendOutcome, SuspendRequest};
use self::audit::{Actor, Investigate, NewRecord};
use self::policies::PolicyInput;
use crate::approvals::DecideOutcome;
use crate::auth::Caller;
use crate::AppState;

pub struct ApiError(pub StatusCode, pub String);

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

impl From<RegisterError> for ApiError {
    fn from(e: RegisterError) -> Self {
        match e {
            RegisterError::Bad(m) => ApiError(StatusCode::BAD_REQUEST, m),
            RegisterError::Db(e) => e.into(),
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn owners_only(caller: &Caller, what: &str) -> ApiResult<()> {
    if caller.is_owner() {
        Ok(())
    } else {
        Err(ApiError(StatusCode::FORBIDDEN, format!("only org owners can {what}")))
    }
}

async fn agent_or_404(state: &AppState, caller: &Caller, id: Uuid) -> ApiResult<AgentRow> {
    agents::get(&state.pool, caller.org_id(), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "agent not found".into()))
}

/// Developer routes (behind `require_dev`).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/agents", get(list_agents).post(register_agent))
        .route("/v1/agents/{id}", get(get_agent).patch(update_agent))
        .route("/v1/agents/{id}/claim", post(claim_agent))
        .route("/v1/agents/{id}/suspend", post(suspend_agent))
        .route("/v1/agents/{id}/reactivate", post(reactivate_agent))
        .route("/v1/agents/{id}/key", post(reissue_key))
        .route("/v1/activity", get(list_activity))
        .route("/v1/policies", get(list_policies).post(create_policy))
        .route("/v1/policies/{id}", axum::routing::patch(update_policy).delete(delete_policy))
        .route("/v1/policy/evaluate", post(evaluate_policy))
        .route("/v1/approvals", get(list_approvals))
        .route("/v1/approvals/{id}/approve", post(approve))
        .route("/v1/approvals/{id}/reject", post(reject))
        .route("/v1/audit", get(list_audit))
        .route("/v1/audit/verify", get(verify_audit))
        .route("/v1/audit/export", get(export_audit))
        .route("/v1/control/summary", get(summary))
}

/// Custom agents' routes, with an agent key.
pub fn gateway_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/v1/gateway/evaluate", post(gateway_evaluate))
        .route("/v1/gateway/approvals/{id}", get(gateway_poll))
        .route("/v1/gateway/events", post(gateway_event))
        .route_layer(middleware::from_fn_with_state(state, require_agent_key))
}

// ---- agents ----

async fn list_agents(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> ApiResult<Json<Value>> {
    let rows = agents::list(&state.pool, caller.org_id()).await?;
    Ok(Json(json!(rows.iter().map(AgentRow::view).collect::<Vec<_>>())))
}

async fn register_agent(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(input): Json<AgentInput>) -> ApiResult<(StatusCode, Json<Value>)> {
    let (agent, key) = agents::register(&state.pool, caller.org_id(), &input, caller.user_id(), &Actor::caller(&caller)).await?;
    tracing::info!(agent = %agent.id, org = %caller.org_id(), "agent registered");
    Ok((StatusCode::CREATED, Json(json!({"agent": agent.view(), "key": key}))))
}

async fn get_agent(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let a = agent_or_404(&state, &caller, id).await?;
    Ok(Json(agents::detail(&state.pool, caller.org_id(), &a).await?))
}

fn may_manage(caller: &Caller, a: &AgentRow) -> bool {
    caller.is_owner() || (a.owner_id.is_some() && a.owner_id == caller.user_id())
}

async fn update_agent(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>, Json(input): Json<AgentInput>) -> ApiResult<Json<Value>> {
    let a = agent_or_404(&state, &caller, id).await?;
    if !may_manage(&caller, &a) {
        return Err(ApiError(StatusCode::FORBIDDEN, "only org owners and the agent's owner can change it".into()));
    }
    if input.owner_id.is_some_and(|o| Some(o) != a.owner_id) && !caller.is_owner() {
        return Err(ApiError(StatusCode::FORBIDDEN, "only org owners can give an agent to someone else".into()));
    }
    let after = agents::update(&state.pool, caller.org_id(), &a, &input, &Actor::caller(&caller)).await?;
    Ok(Json(json!(after.view())))
}

async fn claim_agent(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let a = agent_or_404(&state, &caller, id).await?;
    let Some(me) = caller.user_id() else {
        return Err(bad("the developer key is not a person; set owner_id with PATCH instead"));
    };
    if a.owner_id.is_some() && a.owner_id != Some(me) {
        return Err(ApiError(StatusCode::CONFLICT, "someone already owns this agent".into()));
    }
    let input = AgentInput { owner_id: Some(me), ..Default::default() };
    let after = agents::update(&state.pool, caller.org_id(), &a, &input, &Actor::caller(&caller)).await?;
    Ok(Json(json!(after.view())))
}

async fn suspend_agent(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>, Json(req): Json<SuspendRequest>) -> ApiResult<Json<Value>> {
    owners_only(&caller, "suspend agents")?;
    match agents::suspend(&state.pool, caller.org_id(), id, &req, &Actor::caller(&caller)).await? {
        SuspendOutcome::Suspended(a, incident, events) => {
            for ev in events {
                state.events.publish(ev);
            }
            state.inbox.signal(caller.org_id());
            tracing::warn!(agent = %id, org = %caller.org_id(), runs = incident.cancelled_runs, "agent suspended");
            Ok(Json(json!({"agent": a.view(), "incident": incident})))
        }
        SuspendOutcome::NotFound => Err(ApiError(StatusCode::NOT_FOUND, "agent not found".into())),
        SuspendOutcome::AlreadySuspended => Err(ApiError(StatusCode::CONFLICT, "this agent is already suspended".into())),
    }
}

async fn reactivate_agent(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    owners_only(&caller, "reactivate agents")?;
    match agents::reactivate(&state.pool, caller.org_id(), id, &Actor::caller(&caller)).await? {
        ReactivateOutcome::Reactivated(a) => {
            // Its machine may claim runs for it again.
            state.claims.signal(caller.org_id());
            Ok(Json(json!(a.view())))
        }
        ReactivateOutcome::NotFound => Err(ApiError(StatusCode::NOT_FOUND, "agent not found".into())),
        ReactivateOutcome::NotSuspended => Err(ApiError(StatusCode::CONFLICT, "this agent is not suspended".into())),
    }
}

async fn reissue_key(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let a = agent_or_404(&state, &caller, id).await?;
    if !may_manage(&caller, &a) {
        return Err(ApiError(StatusCode::FORBIDDEN, "only org owners and the agent's owner can issue its key".into()));
    }
    if a.source != "registered" {
        return Err(bad("agents Sidefleet runs don't use agent keys"));
    }
    if a.suspended() {
        return Err(ApiError(StatusCode::CONFLICT, "reactivate the agent before issuing a new key".into()));
    }
    let key = agents::reissue_key(&state.pool, caller.org_id(), &a, &Actor::caller(&caller)).await?;
    Ok(Json(json!({"key": key})))
}

// ---- activity ----

async fn list_activity(State(state): State<AppState>, Extension(caller): Extension<Caller>, Query(f): Query<activity::Filter>) -> ApiResult<Json<Value>> {
    let (events, counts) = activity::list(&state.pool, caller.org_id(), &f).await?;
    Ok(Json(json!({"events": events, "counts": counts})))
}

async fn summary(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> ApiResult<Json<Value>> {
    for ev in crate::approvals::expire_overdue(&state.pool, caller.org_id()).await? {
        state.events.publish(ev);
    }
    Ok(Json(activity::summary(&state.pool, caller.org_id()).await?))
}

// ---- policies ----

async fn list_policies(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(policies::list(&state.pool, caller.org_id()).await?)))
}

fn policy_record(actor: &Actor, event: &'static str, p: &policies::Policy, detail: String, what: String) -> NewRecord {
    NewRecord {
        actor: actor.clone(),
        event,
        agent_id: if p.scope == "agent" { Uuid::parse_str(&p.applies_to).ok() } else { None },
        target: p.name.clone(),
        detail,
        data: Investigate {
            what,
            agent: match p.scope.as_str() {
                "team" => format!("Agents in team {}", p.applies_to),
                "agent" => format!("Agent {}", p.applies_to),
                _ => "Every agent".into(),
            },
            owner: "Not applicable".into(),
            permissions: if p.active { format!("{}: {}", if p.category.is_empty() { &p.name } else { &p.category }, policies::decision_label(&p.decision)) } else { "Falls back to the next matching policy".into() },
            policy: format!("{} ({})", p.name, p.scope_label()),
            approval: "Not applicable".into(),
            decided_by: actor.name.clone(),
            outcome: "Live from the next tool call".into(),
        },
    }
}

async fn create_policy(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(input): Json<PolicyInput>) -> ApiResult<(StatusCode, Json<Value>)> {
    owners_only(&caller, "change policies")?;
    policies::ensure_seeded(&state.pool, caller.org_id()).await?;
    let p = policies::merged(None, &input).map_err(bad)?;
    let actor = Actor::caller(&caller);
    let saved = policies::insert(&state.pool, caller.org_id(), &p, &actor.name).await?;
    let detail = format!("{} · priority {}", policies::decision_label(&saved.decision), saved.priority);
    audit::append_now(&state.pool, caller.org_id(), policy_record(&actor, "policy.created", &saved, detail, format!("{} created a policy.", actor.name))).await?;
    Ok((StatusCode::CREATED, Json(json!(saved))))
}

async fn update_policy(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>, Json(input): Json<PolicyInput>) -> ApiResult<Json<Value>> {
    owners_only(&caller, "change policies")?;
    let current = policies::get(&state.pool, caller.org_id(), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "policy not found".into()))?;
    let next = policies::merged(Some(&current), &input).map_err(bad)?;
    let actor = Actor::caller(&caller);
    let mut tx = state.pool.begin().await?;
    let saved = policies::update(&mut tx, caller.org_id(), &next, &actor.name).await?;
    let toggled = current.active != saved.active;
    let changes = policies::changes(&current, &saved);
    if toggled {
        let on = saved.active;
        let detail = format!("{} · {}", saved.scope_label(), policies::decision_label(&saved.decision));
        let what = format!("{} turned {} a policy.", actor.name, if on { "on" } else { "off" });
        audit::append(&mut tx, caller.org_id(), policy_record(&actor, if on { "policy.activated" } else { "policy.deactivated" }, &saved, detail, what)).await?;
    }
    if changes != "no changes" || !toggled {
        let what = format!("{} saved changes to a policy.", actor.name);
        audit::append(&mut tx, caller.org_id(), policy_record(&actor, "policy.updated", &saved, changes, what)).await?;
    }
    tx.commit().await?;
    Ok(Json(json!(saved)))
}

async fn delete_policy(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    owners_only(&caller, "change policies")?;
    let current = policies::get(&state.pool, caller.org_id(), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "policy not found".into()))?;
    if current.builtin {
        return Err(ApiError(StatusCode::CONFLICT, "the Default deny policy can be turned off but not deleted".into()));
    }
    let actor = Actor::caller(&caller);
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM policies WHERE org_id = $1 AND id = $2").bind(caller.org_id()).bind(id).execute(&mut *tx).await?;
    let mut rec = policy_record(&actor, "policy.deleted", &current, format!("{} · priority {}", policies::decision_label(&current.decision), current.priority), format!("{} deleted a policy.", actor.name));
    rec.data.permissions = "Falls back to the next matching policy".into();
    audit::append(&mut tx, caller.org_id(), rec).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The decision the gateway would make, without acting on it.
async fn evaluate_policy(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(req): Json<gateway::CheckRequest>) -> ApiResult<Json<Value>> {
    let agent = match req.agent_id {
        Some(id) => Some(agent_or_404(&state, &caller, id).await?),
        None => None,
    };
    let Some((tool, input)) = req.call() else { return Err(bad("send {tool, action} or {tool_name, input}")) };
    let input = redact_tool_input(&input, crate::approvals::MAX_INPUT_STRING_BYTES);
    let e = gateway::evaluate_call(&state.pool, caller.org_id(), agent.as_ref(), &tool, &input, req.workdir.as_deref()).await?;
    Ok(Json(gateway::evaluated_json(&e)))
}

// ---- approvals ----

#[derive(Deserialize)]
struct ApprovalsQuery {
    status: Option<String>,
}

async fn list_approvals(State(state): State<AppState>, Extension(caller): Extension<Caller>, Query(q): Query<ApprovalsQuery>) -> ApiResult<Json<Value>> {
    for ev in crate::approvals::expire_overdue(&state.pool, caller.org_id()).await? {
        state.events.publish(ev);
    }
    let mut list = crate::approvals::list_org(&state.pool, caller.org_id(), q.status.as_deref()).await?;
    for a in &mut list {
        a.can_decide = a.status == sidefleet_core::ApprovalStatus::Pending && a.decidable_by(caller.user_id(), caller.is_owner());
    }
    let (pending, resolved) = crate::approvals::counts(&state.pool, caller.org_id()).await?;
    Ok(Json(json!({"approvals": list, "counts": {"pending": pending, "resolved": resolved}})))
}

#[derive(Deserialize, Default)]
struct DecideBody {
    message: Option<String>,
}

async fn approve(state: State<AppState>, caller: Extension<Caller>, path: Path<Uuid>, body: axum::body::Bytes) -> ApiResult<Json<Value>> {
    decide(state, caller, path, body, true).await
}

async fn reject(state: State<AppState>, caller: Extension<Caller>, path: Path<Uuid>, body: axum::body::Bytes) -> ApiResult<Json<Value>> {
    decide(state, caller, path, body, false).await
}

async fn decide(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>, body: axum::body::Bytes, approve: bool) -> ApiResult<Json<Value>> {
    if !caller.is_full() {
        return Err(ApiError(StatusCode::FORBIDDEN, "deciding approvals needs an API token with the full scope".into()));
    }
    let a = crate::approvals::get_org(&state.pool, caller.org_id(), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "approval not found".into()))?;
    if !a.decidable_by(caller.user_id(), caller.is_owner()) {
        return Err(ApiError(StatusCode::FORBIDDEN, "only org owners and the agent's owner can decide this approval".into()));
    }
    let req: DecideBody = if body.iter().all(u8::is_ascii_whitespace) { DecideBody::default() } else { serde_json::from_slice(&body).map_err(|e| bad(format!("invalid body: {e}")))? };
    let by = Actor::caller(&caller).name;
    let err = match crate::approvals::decide_any(&state.pool, caller.org_id(), id, approve, &by, req.message.as_deref()).await? {
        DecideOutcome::Decided(_, ev) => {
            state.events.publish(ev);
            None
        }
        DecideOutcome::DecidedDetached => None,
        DecideOutcome::NotFound => Some(ApiError(StatusCode::NOT_FOUND, "approval not found".into())),
        DecideOutcome::AlreadyDecided(x) => Some(ApiError(StatusCode::CONFLICT, format!("this approval was already {}", x.status.as_str()))),
        DecideOutcome::AlreadyDecidedDetached(s) => Some(ApiError(StatusCode::CONFLICT, format!("this approval was already {}", s.as_str()))),
        DecideOutcome::Expired(_, ev) => {
            ev.into_iter().for_each(|e| state.events.publish(e));
            Some(ApiError(StatusCode::GONE, "too late: the wait for a decision ran out and the action was denied".into()))
        }
        DecideOutcome::ExpiredDetached => Some(ApiError(StatusCode::GONE, "too late: the wait for a decision ran out and the action was denied".into())),
        DecideOutcome::RunEnded(_, ev) => {
            ev.into_iter().for_each(|e| state.events.publish(e));
            Some(ApiError(StatusCode::CONFLICT, "the run is no longer running".into()))
        }
    };
    if let Some(e) = err {
        return Err(e);
    }
    tracing::info!(approval = %id, approved = approve, "approval decided");
    let mut after = crate::approvals::get_org(&state.pool, caller.org_id(), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "approval not found".into()))?;
    after.can_decide = false;
    Ok(Json(json!(after)))
}

// ---- audit ----

async fn list_audit(State(state): State<AppState>, Extension(caller): Extension<Caller>, Query(f): Query<audit::Filter>) -> ApiResult<Json<Value>> {
    let (records, total) = audit::list(&state.pool, caller.org_id(), &f).await?;
    Ok(Json(json!({"records": records, "total": total})))
}

async fn verify_audit(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(audit::verify(&state.pool, caller.org_id()).await?)))
}

#[derive(Deserialize)]
struct ExportQuery {
    format: Option<String>,
    #[serde(flatten)]
    filter: audit::Filter,
}

async fn export_audit(State(state): State<AppState>, Extension(caller): Extension<Caller>, Query(q): Query<ExportQuery>) -> ApiResult<Response> {
    let records = audit::export(&state.pool, caller.org_id(), &q.filter).await?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let (body, mime, ext) = match q.format.as_deref().unwrap_or("jsonl") {
        "csv" => (audit::to_csv(&records), "text/csv; charset=utf-8", "csv"),
        "jsonl" | "json" => (audit::to_jsonl(&records), "application/x-ndjson", "jsonl"),
        _ => return Err(bad("format must be csv or jsonl")),
    };
    Ok((
        [(header::CONTENT_TYPE, mime.to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"sidefleet-audit-{stamp}.{ext}\""))],
        body,
    )
        .into_response())
}

// ---- the gateway, for custom agents ----

/// Gateway routes: `Bearer sfk_…`, an agent key. Puts the [`AgentRow`] in the request.
async fn require_agent_key(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let key = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim).map(str::to_owned);
    let Some(key) = key.filter(|k| k.starts_with(agents::AGENT_KEY_PREFIX)) else {
        return ApiError(StatusCode::UNAUTHORIZED, "missing or invalid agent key".into()).into_response();
    };
    match agents::by_key(&state.pool, &key).await {
        Ok(Some(a)) => {
            crate::request_log::record_caller(a.org_id, None);
            req.extensions_mut().insert(a);
            next.run(req).await
        }
        Ok(None) => ApiError(StatusCode::UNAUTHORIZED, "missing or invalid agent key".into()).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn gateway_evaluate(State(state): State<AppState>, Extension(agent): Extension<AgentRow>, Json(req): Json<gateway::CheckRequest>) -> ApiResult<Json<Value>> {
    let checked = gateway::check(&state.pool, agent.org_id, &agent, None, &req).await?.ok_or_else(|| bad("send {tool, action} or {tool_name, input}"))?;
    Ok(Json(checked.body))
}

async fn gateway_poll(State(state): State<AppState>, Extension(agent): Extension<AgentRow>, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let a = crate::approvals::poll_detached(&state.pool, agent.org_id, Some(agent.id), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "approval not found".into()))?;
    Ok(Json(json!(a)))
}

#[derive(Deserialize)]
struct GatewayEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    context: Option<Value>,
    #[serde(default)]
    metadata: Option<Value>,
}

async fn gateway_event(State(state): State<AppState>, Extension(agent): Extension<AgentRow>, Json(ev): Json<GatewayEvent>) -> ApiResult<StatusCode> {
    let (kind, default_outcome, status): (&'static str, &'static str, Option<&str>) = match ev.kind.as_str() {
        "task_started" => ("task_started", "started", Some("running")),
        "task_completed" => ("task_completed", "succeeded", Some("idle")),
        "execution_failure" => ("execution_failure", "failed", Some("idle")),
        "tool_call" => ("tool_call", "allowed", None),
        _ => return Err(bad("type must be task_started, task_completed, execution_failure or tool_call")),
    };
    let outcome: &'static str = match ev.outcome.as_deref() {
        None => default_outcome,
        Some("succeeded") => "succeeded",
        Some("failed") => "failed",
        Some("allowed") => "allowed",
        Some("denied") => "denied",
        Some("started") => "started",
        Some("stopped") => "stopped",
        Some(_) => return Err(bad("outcome must be started, succeeded, failed, stopped, allowed or denied")),
    };
    if agent.suspended() && kind == "task_started" {
        return Err(ApiError(StatusCode::FORBIDDEN, "this agent is suspended (agent suspended)".into()));
    }
    activity::record(
        &state.pool,
        agent.org_id,
        activity::NewEvent {
            agent_id: Some(agent.id),
            kind,
            tool: ev.tool.unwrap_or_else(|| "task".into()),
            action: ev.action.unwrap_or_default(),
            context: ev.context.unwrap_or(Value::Null),
            outcome,
            metadata: ev.metadata.unwrap_or(Value::Null),
            ..Default::default()
        },
    )
    .await?;
    agents::report_status(&state.pool, agent.id, status).await?;
    Ok(StatusCode::NO_CONTENT)
}
