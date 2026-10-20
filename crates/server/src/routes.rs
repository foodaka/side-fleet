//! HTTP surface. Developer routes are protected by `SIDEFLEET_DEV_KEY`, worker
//! routes by `SIDEFLEET_WORKER_TOKENS`. `/` serves the single-page UI.

use std::convert::Infallible;
use std::time::Duration;

use sidefleet_core::{
    ClaimRequest, CreateRunRequest, CreateRunResponse, FinishRunRequest, FollowUpRequest,
    HeartbeatRequest, PostEventsRequest, RegisterWorkerRequest, Run, RunControl, RunEvent,
    RunStatus, RunSummary, ISOLATION_MODES, PERMISSION_MODES,
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use futures::Stream;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use uuid::Uuid;

use crate::auth::{self, WorkerTokenHash};
use crate::db::{self, AppendOutcome, FinishOutcome, FollowUpOutcome};
use crate::AppState;

const INDEX_HTML: &str = include_str!("../static/index.html");
const CONNECT_MD: &str = include_str!("../static/connect.md");
/// Tasks and replies up to this many bytes; well under what `claude -p` accepts as an argument.
pub const MAX_TASK_BYTES: usize = 100_000;
pub const ALLOWED_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"];

pub fn router(state: AppState) -> Router {
    let dev = Router::new()
        .route("/v1/runs", post(create_run).get(list_runs))
        .route("/v1/runs/{id}", get(get_run))
        .route("/v1/runs/{id}/thread", get(get_thread))
        .route("/v1/runs/{id}/messages", post(follow_up))
        .route("/v1/runs/{id}/events", get(run_events_sse))
        .route("/v1/runs/{id}/stop", post(stop_run))
        .route("/v1/workers", get(list_workers))
        .route("/v1/repos", get(get_added_repos).put(put_added_repos))
        .route("/v1/setup", get(setup))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_dev));

    let worker = Router::new()
        .route("/v1/worker/register", post(register_worker))
        .route("/v1/worker/heartbeat", post(heartbeat))
        .route("/v1/worker/claim", post(claim_run))
        .route("/v1/worker/runs/{id}/events", post(post_events))
        .route("/v1/worker/runs/{id}/control", get(run_control))
        .route("/v1/worker/runs/{id}/finish", post(finish_run))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_worker));

    Router::new()
        .route("/", get(index))
        .route("/healthz", get(|| async { "ok" }))
        .route("/connect.md", get(connect_md))
        .merge(dev)
        .merge(worker)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// Instructions an agent on another machine can read to set itself up as a
/// worker. Contains no secrets, so it is public; the token travels in the
/// prompt the user pastes, never in this document.
async fn connect_md(headers: HeaderMap) -> ([(axum::http::HeaderName, &'static str); 1], String) {
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("localhost:7373");
    (
        [(axum::http::header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        CONNECT_MD.replace("{{ORIGIN}}", &format!("http://{host}")),
    )
}

// ---- errors ----

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database error".into())
    }
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "run not found".into())
}

// ---- developer API ----

async fn create_run(
    State(state): State<AppState>,
    Json(req): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    let task = req.task.trim();
    if task.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "task must not be empty".into()));
    }
    if task.len() > MAX_TASK_BYTES {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("task is too long ({} characters; the limit is {})", task.chars().count(), MAX_TASK_BYTES)));
    }
    if !ALLOWED_MODELS.contains(&req.model.as_str()) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("model must be one of {}", ALLOWED_MODELS.join(", ")),
        ));
    }
    let repo = req.repo.trim();
    if repo.is_empty() || repo.contains("..") || repo.starts_with('/') {
        return Err(ApiError(StatusCode::BAD_REQUEST, "repo must be a relative repo name".into()));
    }
    let mode = check_permission_mode(req.permission_mode.as_deref())?;
    let isolation = match req.isolation.as_deref() {
        None | Some("") => "worktree",
        Some(i) if ISOLATION_MODES.contains(&i) => i,
        Some(_) => return Err(ApiError(StatusCode::BAD_REQUEST, format!("isolation must be one of {}", ISOLATION_MODES.join(", ")))),
    };
    let (run, ev) = db::create_run(&state.pool, task, &req.model, repo, mode, isolation).await?;
    state.events.publish(ev);
    tracing::info!(run_id = %run.id, model = %run.model, repo = %run.repo, "run queued");
    Ok((StatusCode::ACCEPTED, Json(CreateRunResponse { id: run.id, status: run.status })))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}
fn default_limit() -> i64 {
    50
}

async fn list_runs(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<RunSummary>>, ApiError> {
    Ok(Json(db::list_runs(&state.pool, q.limit.clamp(1, 500)).await?))
}

fn check_permission_mode(mode: Option<&str>) -> Result<Option<&str>, ApiError> {
    match mode {
        None | Some("") => Ok(None),
        Some(m) if PERMISSION_MODES.contains(&m) => Ok(Some(m)),
        Some(_) => Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("permission_mode must be one of {}", PERMISSION_MODES.join(", ")),
        )),
    }
}

/// All runs of the thread this run belongs to, root first.
async fn get_thread(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Run>>, ApiError> {
    let run = db::get_run(&state.pool, id).await?.ok_or_else(not_found)?;
    Ok(Json(db::thread_runs(&state.pool, run.parent_id.unwrap_or(run.id)).await?))
}

/// Continue the conversation: queue a run that resumes the thread's Claude Code session.
async fn follow_up(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<FollowUpRequest>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    let task = req.task.trim();
    if task.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "message must not be empty".into()));
    }
    if task.len() > MAX_TASK_BYTES {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("message is too long ({} characters; the limit is {})", task.chars().count(), MAX_TASK_BYTES)));
    }
    if let Some(m) = &req.model {
        if !ALLOWED_MODELS.contains(&m.as_str()) {
            return Err(ApiError(StatusCode::BAD_REQUEST, format!("model must be one of {}", ALLOWED_MODELS.join(", "))));
        }
    }
    let mode = check_permission_mode(req.permission_mode.as_deref())?;
    match db::create_follow_up(&state.pool, id, task, req.model.as_deref(), mode).await? {
        FollowUpOutcome::Created(run, ev) => {
            state.events.publish(ev);
            tracing::info!(run_id = %run.id, parent = ?run.parent_id, "follow-up queued");
            Ok((StatusCode::ACCEPTED, Json(CreateRunResponse { id: run.id, status: run.status })))
        }
        FollowUpOutcome::NotFound => Err(not_found()),
        FollowUpOutcome::Busy(_) => Err(ApiError(StatusCode::CONFLICT, "the previous message is still running; wait for it to finish or stop it".into())),
        FollowUpOutcome::NoSession(_) => Err(ApiError(StatusCode::CONFLICT, "this thread has no Claude Code session to resume; start a new task instead".into())),
    }
}

async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Run>, ApiError> {
    db::get_run(&state.pool, id).await?.map(Json).ok_or_else(not_found)
}

async fn stop_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Run>, ApiError> {
    let (run, ev) = db::request_stop(&state.pool, id).await?.ok_or_else(not_found)?;
    if let Some(ev) = ev {
        state.events.publish(ev);
    }
    tracing::info!(run_id = %id, status = run.status.as_str(), "stop requested");
    Ok(Json(run))
}

/// What the page needs to show a copy-paste "connect a machine" command.
/// Only the developer key holder can read it; it hands out the first worker token.
async fn setup(State(state): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("localhost:7373");
    Json(json!({
        "control_url": format!("http://{host}"),
        "worker_token": state.config.worker_tokens.first().cloned().unwrap_or_default(),
        "worker_token_count": state.config.worker_tokens.len(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn list_workers(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let workers = db::list_workers(&state.pool).await?;
    let out: Vec<_> = workers
        .into_iter()
        .map(|w| {
            let online = db::is_online(w.last_heartbeat_at);
            let mut v = serde_json::to_value(&w).unwrap_or(json!({}));
            v["online"] = json!(online);
            v
        })
        .collect();
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
struct AddedRepos {
    added: Vec<String>,
}

/// The repos the developer chose to show in the composer (out of everything the machines report).
async fn get_added_repos(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(json!({ "added": db::list_added_repos(&state.pool).await? })))
}

async fn put_added_repos(
    State(state): State<AppState>,
    Json(req): Json<AddedRepos>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut names: Vec<String> = req.added.into_iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
    names.sort();
    names.dedup();
    if names.len() > 1000 || names.iter().any(|n| n.len() > 512) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "too many or too long repo names".into()));
    }
    db::set_added_repos(&state.pool, &names).await?;
    Ok(Json(json!({ "added": names })))
}

#[derive(Deserialize)]
struct EventsQuery {
    /// Start replay from this seq (default 0). `Last-Event-ID` takes precedence.
    from: Option<i64>,
}

/// SSE: replays stored events from `from`/`Last-Event-ID + 1`, then streams
/// live ones. Each event's `id` is its seq, so browsers resume on reconnect.
/// Ends with an `end` event once the run reaches a terminal status.
async fn run_events_sse(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<EventsQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let run = db::get_run(&state.pool, id).await?.ok_or_else(not_found)?;
    let from = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .map(|last| last + 1)
        .or(q.from)
        .unwrap_or(0)
        .max(0);

    // Subscribe before reading history so nothing is missed in between.
    let mut rx = state.events.subscribe(id);
    let pool = state.pool.clone();
    let initially_terminal = run.status.is_terminal();

    let stream = async_stream::stream! {
        let mut next_seq = from;
        let history = match db::events_since(&pool, id, next_seq).await {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(error = %e, "history fetch failed");
                yield Ok(Event::default().event("error").data("history fetch failed"));
                return;
            }
        };
        let mut terminal = initially_terminal;
        for ev in history {
            next_seq = ev.seq + 1;
            terminal |= is_terminal_event(&ev);
            yield Ok(to_sse(&ev));
        }
        if terminal {
            yield Ok(Event::default().event("end").data("run finished"));
            return;
        }
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if ev.seq < next_seq {
                        continue; // already replayed from history
                    }
                    if ev.seq > next_seq {
                        // A gap: fetch what we missed from the database.
                        if let Ok(missing) = db::events_since(&pool, id, next_seq).await {
                            for m in missing.into_iter().filter(|m| m.seq < ev.seq) {
                                yield Ok(to_sse(&m));
                            }
                        }
                    }
                    next_seq = ev.seq + 1;
                    let t = is_terminal_event(&ev);
                    yield Ok(to_sse(&ev));
                    if t {
                        yield Ok(Event::default().event("end").data("run finished"));
                        return;
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    if let Ok(missing) = db::events_since(&pool, id, next_seq).await {
                        for m in missing {
                            next_seq = m.seq + 1;
                            let t = is_terminal_event(&m);
                            yield Ok(to_sse(&m));
                            if t {
                                yield Ok(Event::default().event("end").data("run finished"));
                                return;
                            }
                        }
                    }
                }
                Err(RecvError::Closed) => {
                    // Sender dropped after a terminal event; drain anything left.
                    if let Ok(missing) = db::events_since(&pool, id, next_seq).await {
                        for m in missing {
                            yield Ok(to_sse(&m));
                        }
                    }
                    yield Ok(Event::default().event("end").data("run finished"));
                    return;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new().interval(Duration::from_secs(10)).text("keep-alive"),
    ))
}

fn is_terminal_event(ev: &RunEvent) -> bool {
    ev.kind == "status"
        && ev
            .payload
            .get("status")
            .and_then(|s| s.as_str())
            .and_then(RunStatus::parse)
            .map(|s| s.is_terminal())
            .unwrap_or(false)
}

fn to_sse(ev: &RunEvent) -> Event {
    Event::default()
        .id(ev.seq.to_string())
        .event(ev.kind.clone())
        .data(serde_json::to_string(ev).unwrap_or_default())
}

// ---- worker API ----

async fn register_worker(
    State(state): State<AppState>,
    Extension(WorkerTokenHash(hash)): Extension<WorkerTokenHash>,
    Json(req): Json<RegisterWorkerRequest>,
) -> Result<Json<sidefleet_core::Worker>, ApiError> {
    if req.id.trim().is_empty() || req.id.len() > 128 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "worker id must be 1-128 chars".into()));
    }
    let w = db::upsert_worker(&state.pool, &req.id, &req.display_name, &hash, &req.capabilities).await?;
    tracing::info!(worker = %w.id, repos = w.capabilities.repos.len(), "worker registered");
    Ok(Json(w))
}

async fn heartbeat(
    State(state): State<AppState>,
    Extension(WorkerTokenHash(hash)): Extension<WorkerTokenHash>,
    Json(req): Json<HeartbeatRequest>,
) -> Result<StatusCode, ApiError> {
    if db::heartbeat(&state.pool, &req.id, &hash, &req.capabilities).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        // Unknown worker (e.g. control plane restarted with a fresh DB): tell it to re-register.
        Err(ApiError(StatusCode::NOT_FOUND, "worker not registered".into()))
    }
}

async fn claim_run(
    State(state): State<AppState>,
    Extension(WorkerTokenHash(hash)): Extension<WorkerTokenHash>,
    Json(req): Json<ClaimRequest>,
) -> Result<Response, ApiError> {
    if !db::worker_owns_token(&state.pool, &req.worker_id, &hash).await? {
        return Err(ApiError(StatusCode::NOT_FOUND, "worker not registered".into()));
    }
    if req.repos.is_empty() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    match db::claim_run(&state.pool, &req.worker_id, &req.repos, &req.busy_repos).await? {
        Some((run, ev)) => {
            state.events.publish(ev);
            tracing::info!(run_id = %run.id, worker = %req.worker_id, "run claimed");
            Ok(Json(run).into_response())
        }
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

#[derive(Deserialize)]
struct WorkerQuery {
    worker_id: String,
}

async fn post_events(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<WorkerQuery>,
    Json(req): Json<PostEventsRequest>,
) -> Result<Json<RunControl>, ApiError> {
    if req.events.len() > 1000 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "too many events in one batch".into()));
    }
    match db::append_events(&state.pool, id, &q.worker_id, &req.events).await? {
        AppendOutcome::Appended(stored, run) => {
            for ev in stored {
                state.events.publish(ev);
            }
            Ok(Json(RunControl { status: run.status, stop_requested: run.stop_requested }))
        }
        AppendOutcome::NotFound => Err(not_found()),
        AppendOutcome::NotOwner => Err(ApiError(StatusCode::FORBIDDEN, "run belongs to another worker".into())),
        AppendOutcome::NotRunning(run) => {
            Ok(Json(RunControl { status: run.status, stop_requested: true }))
        }
    }
}

async fn run_control(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RunControl>, ApiError> {
    let run = db::get_run(&state.pool, id).await?.ok_or_else(not_found)?;
    Ok(Json(RunControl {
        status: run.status,
        stop_requested: run.stop_requested || run.status.is_terminal(),
    }))
}

async fn finish_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<WorkerQuery>,
    Json(req): Json<FinishRunRequest>,
) -> Result<Json<Run>, ApiError> {
    if !req.status.is_terminal() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "finish status must be terminal".into()));
    }
    match db::finish_run(&state.pool, id, &q.worker_id, req.status, req.result).await? {
        FinishOutcome::Finished(run, ev) => {
            state.events.publish(ev);
            tracing::info!(run_id = %id, status = run.status.as_str(), "run finished");
            Ok(Json(run))
        }
        FinishOutcome::NotFound => Err(not_found()),
        FinishOutcome::NotOwner => Err(ApiError(StatusCode::FORBIDDEN, "run belongs to another worker".into())),
        FinishOutcome::AlreadyFinished(run) => Ok(Json(run)),
    }
}
