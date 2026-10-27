//! HTTP surface. Developer routes take the developer key or a session cookie and
//! act in the caller's current org; worker routes take a static or per-org
//! worker token and act in that token's org. `/` serves the single-page UI.

use std::convert::Infallible;
use std::time::Duration;

use sidefleet_core::{
    Agent, Approval, ApprovalRequest, ArchiveRequest, ClaimRequest, DecideApprovalRequest, CreateRunRequest, CreateRunResponse, FinishRunRequest, FollowUpRequest, ForkRequest,
    HeartbeatRequest, HeartbeatResponse, ImageUpload, LiveTextRequest, PostEventsRequest, RegisterWorkerRequest, Run, RunControl, RunEvent,
    RunStatus, RunSummary, ISOLATION_MODES, LIVE_TEXT, LIVE_TEXT_MAX_BYTES, MAX_ATTACHED_IMAGES, PERMISSION_MODES,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use futures::Stream;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tower_http::{
    catch_panic::CatchPanicLayer,
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use uuid::Uuid;

use crate::approvals::{DecideOutcome, RequestOutcome};
use crate::auth::{self, Caller, Identity, WorkerAuth};
use crate::events::Update;
use crate::db::{self, AppendOutcome, FinishOutcome, FollowUpOutcome, ForkOutcome, PrOutcome, RestoreOutcome};
use crate::oauth::{self, public_base};
use crate::tenancy::{self, Decide, NewEnrollment, Poll, DEFAULT_ORG};
use crate::AppState;

/// The web app, built from `web/` with `pnpm build` and committed so a source
/// install needs only Rust. Embedded in release builds; read from disk in debug.
#[derive(rust_embed::Embed)]
#[folder = "web-dist/"]
struct WebApp;
const CONNECT_MD: &str = include_str!("../static/connect.md");
/// The longest a worker's claim is held waiting for a run (under common proxy idle timeouts).
const MAX_CLAIM_WAIT_SECS: u64 = 25;
/// How often a waiting claim looks again without being woken.
const CLAIM_RECHECK: Duration = Duration::from_secs(5);
/// Tasks and replies up to this many bytes; well under what an agent CLI accepts as an argument.
pub const MAX_TASK_BYTES: usize = 100_000;
/// One batch of worker events, images included.
pub const MAX_EVENTS_BODY_BYTES: usize = 64 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    let dev = Router::new()
        // Tasks and replies can carry images (base64).
        .route("/v1/runs", post(create_run).get(list_runs).layer(DefaultBodyLimit::max(MAX_EVENTS_BODY_BYTES)))
        .route("/v1/runs/{id}", get(get_run))
        .route("/v1/runs/{id}/thread", get(get_thread))
        .route("/v1/runs/{id}/summary", get(run_summary))
        .route("/v1/runs/{id}/messages", post(follow_up).layer(DefaultBodyLimit::max(MAX_EVENTS_BODY_BYTES)))
        .route("/v1/runs/{id}/events", get(run_events_sse))
        .route("/v1/runs/{id}/stop", post(stop_run))
        .route("/v1/runs/{id}/restore", post(restore_run))
        .route("/v1/runs/{id}/fork", post(fork_run))
        .route("/v1/runs/{id}/pr", post(open_pr))
        .route("/v1/runs/{id}/archive", post(archive_run))
        .route("/v1/runs/{id}/approvals", get(list_approvals))
        .route("/v1/runs/{id}/approvals/{approval}/approve", post(approve_tool))
        .route("/v1/runs/{id}/approvals/{approval}/deny", post(deny_tool))
        .route("/v1/images/{id}", get(get_image))
        .route("/v1/workers", get(list_workers))
        .route("/v1/workers/{id}", axum::routing::delete(delete_worker))
        .route("/v1/workers/{id}/update", post(request_update))
        .route("/v1/repos", get(get_added_repos).put(put_added_repos))
        .route("/v1/setup", get(setup))
        // Chat agents (Hermes, OpenClaw, Claude) drive the fleet through MCP with an API token.
        .route("/mcp", get(crate::mcp::get).post(crate::mcp::post))
        .route("/v1/api-tokens", get(list_api_tokens).post(create_api_token))
        .route("/v1/api-tokens/{id}", axum::routing::delete(revoke_api_token))
        .route("/v1/api-tokens/{id}/updates", axum::routing::put(set_chat_updates).delete(clear_chat_updates))
        .route("/v1/api-tokens/{id}/updates/test", post(test_chat_updates))
        .route("/v1/orgs", post(create_org))
        .route("/v1/orgs/{org}", axum::routing::patch(rename_org))
        .route("/v1/orgs/{org}/tokens", get(list_tokens).post(create_token))
        .route("/v1/orgs/{org}/tokens/{id}", axum::routing::delete(revoke_token))
        .route("/v1/orgs/{org}/members", get(list_members).post(add_member))
        .route("/v1/enrollments", get(list_enrollments))
        .route("/v1/enrollments/lookup", post(lookup_enrollment))
        .route("/v1/enrollments/{id}/approve", post(approve_enrollment))
        .route("/v1/enrollments/{id}/deny", post(deny_enrollment))
        .route("/v1/runs/{id}/integrate", post(integrate_run))
        .route("/v1/rooms/{id}/tasks", post(create_room_task).layer(DefaultBodyLimit::max(MAX_EVENTS_BODY_BYTES)))
        .merge(crate::inbox::router())
        .merge(crate::team::router())
        .merge(crate::rooms::router())
        .merge(crate::machines::router())
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_dev));

    let worker = Router::new()
        .route("/v1/worker/register", post(register_worker))
        .route("/v1/worker/heartbeat", post(heartbeat))
        .route("/v1/worker/update-ack", post(update_ack))
        .route("/v1/worker/claim", post(claim_run))
        // Events can carry screenshots (base64), well past axum's 2 MB default.
        .route("/v1/worker/runs/{id}/events", post(post_events).layer(DefaultBodyLimit::max(MAX_EVENTS_BODY_BYTES)))
        .route("/v1/worker/runs/{id}/live", post(post_live_text))
        .route("/v1/worker/runs/{id}/control", get(run_control))
        .route("/v1/worker/runs/{id}/approvals", post(request_approval))
        .route("/v1/worker/runs/{id}/approvals/{approval}", get(poll_approval))
        .route("/v1/worker/images/{id}", get(worker_get_image))
        .route("/v1/worker/runs/{id}/finish", post(finish_run))
        .route("/v1/worker/prs", post(report_prs))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_worker));

    Router::new()
        .route("/", get(index))
        .route("/_app/{*path}", get(web_asset))
        .route("/robots.txt", get(|| web_file("robots.txt")))
        .route("/favicon.ico", get(|| web_file("favicon.ico")))
        .route("/apple-touch-icon.png", get(|| web_file("apple-touch-icon.png")))
        .route("/icon-192.png", get(|| web_file("icon-192.png")))
        .route("/icon-512.png", get(|| web_file("icon-512.png")))
        .route("/maskable-512.png", get(|| web_file("maskable-512.png")))
        .route("/manifest.webmanifest", get(|| web_file("manifest.webmanifest")))
        .route("/service-worker.js", get(|| web_file("service-worker.js")))
        .route("/healthz", get(healthz))
        .route("/connect.md", get(connect_md))
        .route("/auth/github/login", get(oauth::login))
        .route("/auth/github/callback", get(oauth::callback))
        .route("/auth/logout", post(oauth::logout))
        .route("/v1/auth", get(auth_methods))
        .route("/v1/me", get(me))
        .route("/v1/enroll", post(enroll))
        .route("/v1/enroll/{id}", get(enroll_poll))
        .merge(crate::team::public_router())
        .merge(dev)
        .merge(worker)
        .layer(CorsLayer::permissive())
        // gzip/brotli for the page, assets and JSON; the default predicate skips SSE.
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(CatchPanicLayer::custom(crate::request_log::panic_response))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(crate::request_log::RequestSpan)
                .on_request(())
                .on_response(crate::request_log::LogResponse)
                .on_failure(crate::request_log::LogFailure),
        )
        // Outermost: give every request an id (kept if the client sent one) and echo it back.
        .layer(PropagateRequestIdLayer::new(crate::request_log::REQUEST_ID))
        .layer(SetRequestIdLayer::new(crate::request_log::REQUEST_ID, MakeRequestUuid))
        .with_state(state)
}

async fn index() -> Response {
    web_file("index.html").await
}

async fn web_asset(Path(path): Path<String>) -> Response {
    web_file(&format!("_app/{path}")).await
}

async fn web_file(path: &str) -> Response {
    let Some(file) = WebApp::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    // Hashed build output never changes; the page itself must always be fresh.
    let cache = if path.starts_with("_app/immutable/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    (
        [
            (axum::http::header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
            (axum::http::header::CACHE_CONTROL, cache.to_string()),
        ],
        file.data,
    )
        .into_response()
}

/// Instructions an agent on another machine can read to set itself up as a
/// worker. Contains no secrets, so it is public; the token travels in the
/// prompt the user pastes, never in this document.
async fn connect_md(State(state): State<AppState>, headers: HeaderMap) -> ([(axum::http::HeaderName, &'static str); 1], String) {
    (
        [(axum::http::header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        CONNECT_MD.replace("{{ORIGIN}}", &public_base(&state, &headers)),
    )
}

/// Liveness plus a database round trip, for the platform's health check.
async fn healthz(State(state): State<AppState>) -> Response {
    match tokio::time::timeout(Duration::from_secs(3), sqlx::query("SELECT 1").execute(&state.pool)).await {
        Ok(Ok(_)) => "ok".into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "database unavailable").into_response(),
    }
}

// ---- errors ----

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

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "run not found".into())
}

fn forbidden(msg: &str) -> ApiError {
    ApiError(StatusCode::FORBIDDEN, msg.into())
}

/// 402: the org's plan does not allow this.
fn over_limit(caller_plan: &str, what: String) -> ApiError {
    ApiError(StatusCode::PAYMENT_REQUIRED, format!("{what} on the {caller_plan} plan. Upgrade the org's plan or free one up."))
}

/// Refuse a new run when the org already has as many in flight as its plan allows.
async fn check_run_limit(state: &AppState, caller: &Caller) -> Result<(), ApiError> {
    if let Some(l) = caller.org.limits {
        if db::count_active_runs(&state.pool, caller.org_id()).await? >= l.parallel_runs {
            return Err(over_limit(&caller.org.plan, format!("{} runs can be queued or running at once", l.parallel_runs)));
        }
    }
    Ok(())
}

// ---- developer API ----

async fn create_run(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(req): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    start_run(&state, &caller, req, None).await
}

/// Start a task in a room: the room's repo, a room brief for the agent, and the
/// room branch to start from once something has been merged into it.
async fn create_room_task(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
    Json(mut req): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    let room = crate::rooms::get(&state.pool, caller.org_id(), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "room not found".into()))?;
    if room.archived_at.is_some() {
        return Err(ApiError(StatusCode::CONFLICT, "this room is archived; bring it back to start tasks in it".into()));
    }
    let Some(obj) = req.as_object_mut() else {
        return Err(ApiError(StatusCode::BAD_REQUEST, "expected a JSON object".into()));
    };
    obj.insert("repo".into(), json!(room.repo));
    let req: CreateRunRequest = serde_json::from_value(req).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("invalid body: {e}")))?;
    start_run(&state, &caller, req, Some(room)).await
}

async fn start_run(
    state: &AppState,
    caller: &Caller,
    req: CreateRunRequest,
    room: Option<crate::rooms::Room>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    let task = req.task.trim();
    if task.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "task must not be empty".into()));
    }
    if task.len() > MAX_TASK_BYTES {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("task is too long ({} characters; the limit is {})", task.chars().count(), MAX_TASK_BYTES)));
    }
    let agent = parse_agent(req.agent.as_deref())?;
    let model = req.model.trim();
    agent.check_model(model).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    let effort = check_effort(agent, req.effort.as_deref())?;
    let repo = req.repo.trim();
    if repo.is_empty() || repo.contains("..") || repo.starts_with('/') {
        return Err(ApiError(StatusCode::BAD_REQUEST, "repo must be a relative repo name".into()));
    }
    check_images(&req.images)?;
    let mode = check_permission_mode(req.permission_mode.as_deref())?;
    check_full_access(caller, mode)?;
    check_ask_agent(mode, agent)?;
    let isolation = match req.isolation.as_deref() {
        None | Some("") => "worktree",
        Some(i) if ISOLATION_MODES.contains(&i) => i,
        Some(_) => return Err(ApiError(StatusCode::BAD_REQUEST, format!("isolation must be one of {}", ISOLATION_MODES.join(", ")))),
    };
    let worker = own_machine(state, caller, req.worker_id.as_deref()).await?;
    if room.is_some() && isolation != "worktree" {
        return Err(ApiError(StatusCode::BAD_REQUEST, "a room's tasks work in their own worktree, so they can be merged into the room branch".into()));
    }
    let (brief, from_branch) = match &room {
        Some(r) => (Some(crate::rooms::brief_for(&state.pool, r, None, true).await?), r.integrated_at.map(|_| r.branch.clone())),
        None => (None, None),
    };
    let new = db::NewRun {
        task,
        agent,
        model,
        effort,
        repo,
        permission_mode: mode,
        isolation,
        pinned_worker_id: worker,
        failover: req.failover,
        images: &req.images,
        created_by: caller.user_id(),
        room_id: room.as_ref().map(|r| r.id),
        context: brief.as_deref(),
        resume_from_branch: from_branch.as_deref(),
    };
    check_run_limit(state, caller).await?;
    let (run, ev) = db::create_run(&state.pool, caller.org_id(), new).await?;
    tag_run(state, caller, run.id).await?;
    state.events.publish(ev);
    tracing::info!(run_id = %run.id, agent = %run.agent, model = %run.model, repo = %run.repo, "run queued");
    Ok((StatusCode::ACCEPTED, Json(CreateRunResponse { id: run.id, status: run.status })))
}

fn parse_agent(agent: Option<&str>) -> Result<Agent, ApiError> {
    match agent.map(str::trim) {
        None | Some("") => Ok(Agent::Claude),
        Some(a) => Agent::parse(a).ok_or_else(|| {
            let all: Vec<&str> = sidefleet_core::AGENTS.iter().map(Agent::as_str).collect();
            ApiError(StatusCode::BAD_REQUEST, format!("agent must be one of {}", all.join(", ")))
        }),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    /// List archived threads instead of the others.
    #[serde(default)]
    archived: bool,
}
fn default_limit() -> i64 {
    50
}

async fn list_runs(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<RunSummary>>, ApiError> {
    Ok(Json(db::list_runs(&state.pool, caller.org_id(), q.limit.clamp(1, 500), q.archived).await?))
}

/// Attached images must be raster images the agents can read, and not too many or too large.
fn check_images(images: &[ImageUpload]) -> Result<(), ApiError> {
    use base64::Engine;
    let bad = |m: String| Err(ApiError(StatusCode::BAD_REQUEST, m));
    if images.len() > MAX_ATTACHED_IMAGES {
        return bad(format!("at most {MAX_ATTACHED_IMAGES} images per message"));
    }
    for img in images {
        if !db::IMAGE_TYPES.contains(&img.media_type.as_str()) {
            return bad(format!("images must be PNG, JPEG, GIF or WebP, not {}", img.media_type));
        }
        match base64::engine::general_purpose::STANDARD.decode(&img.data) {
            Ok(b) if b.len() > db::MAX_IMAGE_BYTES => return bad(format!("an image is over {} MB", db::MAX_IMAGE_BYTES / 1024 / 1024)),
            Ok(_) => {}
            Err(_) => return bad("an image is not valid base64".into()),
        }
    }
    Ok(())
}

/// An effort the agent takes, or None (absent or empty) for its default.
fn check_effort(agent: Agent, effort: Option<&str>) -> Result<Option<&str>, ApiError> {
    match effort.map(str::trim) {
        None | Some("") => Ok(None),
        Some(e) => agent.check_effort(e).map(|_| Some(e)).map_err(|m| ApiError(StatusCode::BAD_REQUEST, m)),
    }
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

/// A message sent through an API token: its chat app hears how the task goes.
async fn tag_run(state: &AppState, caller: &Caller, run: Uuid) -> Result<(), ApiError> {
    if let Some(t) = caller.token {
        tenancy::tag_run(&state.pool, run, t).await?;
    }
    Ok(())
}

/// A machine chosen for a run: it must exist and be the caller's own (machines run only their owner's tasks).
async fn own_machine<'a>(state: &AppState, caller: &Caller, worker: Option<&'a str>) -> Result<Option<&'a str>, ApiError> {
    let Some(w) = worker.map(str::trim).filter(|w| !w.is_empty()) else { return Ok(None) };
    if !db::worker_exists(&state.pool, caller.org_id(), w).await? {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("no machine is registered as {w}")));
    }
    if db::worker_owner(&state.pool, caller.org_id(), w).await? != caller.user_id() {
        return Err(forbidden(&format!("{w} is a teammate's machine; tasks only run on their owner's machines")));
    }
    Ok(Some(w))
}

/// The thread `id` belongs to, if the caller may make its agent do things: only its creator
/// may (their machine runs it). Anyone can still read and fork it.
async fn own_thread(state: &AppState, caller: &Caller, id: Uuid) -> Result<Run, ApiError> {
    let run = db::get_run(&state.pool, caller.org_id(), id).await?.ok_or_else(not_found)?;
    let root = match run.parent_id {
        Some(p) => db::get_run(&state.pool, caller.org_id(), p).await?.ok_or_else(not_found)?,
        None => run,
    };
    match root.created_by {
        Some(owner) if Some(owner) != caller.user_id() => {
            let login: Option<String> = sqlx::query_scalar("SELECT login FROM users WHERE id = $1").bind(owner).fetch_optional(&state.pool).await?;
            let who = login.map_or_else(|| "a teammate".to_string(), |l| format!("@{l}"));
            Err(forbidden(&format!("this is {who}'s task and it runs on their machine; fork it to continue on yours")))
        }
        _ => Ok(root),
    }
}

/// Full access runs any command without asking, so an API token needs the `full` scope to
/// start a run with it, or to continue (reply to, fork) a thread that runs with it.
fn check_full_access(caller: &Caller, mode: Option<&str>) -> Result<(), ApiError> {
    if mode == Some(FULL_ACCESS_MODE) && !caller.is_full() {
        return Err(forbidden("full access (bypassPermissions) needs an API token with the full scope; choose another permission_mode"));
    }
    Ok(())
}
const FULL_ACCESS_MODE: &str = "bypassPermissions";

/// "Ask me" relays Claude Code's permission prompts; the other agents have no such hook yet.
fn check_ask_agent(mode: Option<&str>, agent: Agent) -> Result<(), ApiError> {
    if mode == Some(sidefleet_core::ASK_MODE) && agent != Agent::Claude {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("\"Ask me\" works with Claude Code only for now, not {}", agent.display_name())));
    }
    Ok(())
}

/// The thread in a few lines, for scripts and chat agents (see `summary`).
async fn run_summary(State(state): State<AppState>, Extension(caller): Extension<Caller>, headers: HeaderMap, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, ApiError> {
    let base = public_base(&state, &headers);
    Ok(Json(crate::summary::thread(&state.pool, caller.org_id(), id, &base).await?.ok_or_else(not_found)?))
}

/// All runs of the thread this run belongs to, root first.
async fn get_thread(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Run>>, ApiError> {
    let run = db::get_run(&state.pool, caller.org_id(), id).await?.ok_or_else(not_found)?;
    Ok(Json(db::thread_runs(&state.pool, caller.org_id(), run.parent_id.unwrap_or(run.id)).await?))
}

/// Continue the conversation: queue a run that resumes the thread's session, with the same agent
/// or, when `agent` names another, with that one (it gets a transcript of the thread so far).
/// While an earlier message runs, the new one waits behind it (stop that one to send this now).
async fn follow_up(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
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
    let agent = match req.agent.as_deref().map(str::trim) {
        None | Some("") => None,
        a => Some(parse_agent(a)?),
    };
    check_images(&req.images)?;
    let mode = check_permission_mode(req.permission_mode.as_deref())?;
    if let Some(a) = agent {
        check_ask_agent(mode, a)?;
    }
    let root = own_thread(&state, &caller, id).await?;
    let Some(run) = db::get_run(&state.pool, caller.org_id(), id).await? else {
        return Err(not_found());
    };
    // A room's task hears what its teammates did since.
    let update = match root.room_id {
        Some(r) => match crate::rooms::get(&state.pool, caller.org_id(), r).await? {
            Some(room) => Some(crate::rooms::brief_for(&state.pool, &room, Some(root.id), false).await?),
            None => None,
        },
        None => None,
    };
    // Without a mode the reply keeps the thread's latest one.
    let inherited = match mode {
        Some(_) => None,
        None => db::thread_runs(&state.pool, caller.org_id(), run.parent_id.unwrap_or(run.id)).await?.pop().and_then(|r| r.permission_mode),
    };
    check_full_access(&caller, mode.or(inherited.as_deref()))?;
    check_run_limit(&state, &caller).await?;
    match db::create_follow_up(
        &state.pool,
        caller.org_id(),
        id,
        task,
        agent,
        req.model.as_deref().map(str::trim),
        req.effort.as_deref().map(str::trim),
        mode,
        &req.images,
        caller.user_id(),
        update.as_deref(),
    )
    .await?
    {
        FollowUpOutcome::Created(run, ev) => {
            tag_run(&state, &caller, run.id).await?;
            state.events.publish(ev);
            tracing::info!(run_id = %run.id, parent = ?run.parent_id, "follow-up queued");
            Ok((StatusCode::ACCEPTED, Json(CreateRunResponse { id: run.id, status: run.status })))
        }
        FollowUpOutcome::NotFound => Err(not_found()),
        FollowUpOutcome::QueueFull => Err(ApiError(StatusCode::CONFLICT, format!("{} messages are already waiting; wait for them or stop some", db::MAX_QUEUED_MESSAGES))),
        FollowUpOutcome::NoSession(_) => Err(ApiError(StatusCode::CONFLICT, "this thread has no agent session to resume; start a new task instead".into())),
        FollowUpOutcome::BadModel(e) => Err(ApiError(StatusCode::BAD_REQUEST, e)),
        FollowUpOutcome::AgentMissing(w) => Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("{w} doesn't have {}; the thread's files are there, so pick an agent installed on it", agent.map_or("that agent", |a| a.as_str())),
        )),
    }
}

/// Put the thread's files back to how they were before this run started.
async fn restore_run(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    own_thread(&state, &caller, id).await?;
    check_run_limit(&state, &caller).await?;
    match db::create_restore(&state.pool, caller.org_id(), id, caller.user_id()).await? {
        RestoreOutcome::Created(run, ev) => {
            state.events.publish(ev);
            tracing::info!(run_id = %run.id, restores = %id, "restore queued");
            Ok((StatusCode::ACCEPTED, Json(CreateRunResponse { id: run.id, status: run.status })))
        }
        RestoreOutcome::NotFound => Err(not_found()),
        RestoreOutcome::Busy(_) => Err(ApiError(StatusCode::CONFLICT, "the thread is still running; wait for it to finish or stop it".into())),
        RestoreOutcome::InPlace => Err(ApiError(StatusCode::CONFLICT, "this task ran in the checkout itself; only tasks in their own worktree can be restored".into())),
        RestoreOutcome::NoCheckpoint => Err(ApiError(StatusCode::CONFLICT, "this message has no snapshot to restore".into())),
    }
}

/// Push the thread's branch and open a pull request for it (or update the open one).
async fn open_pr(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    own_thread(&state, &caller, id).await?;
    check_run_limit(&state, &caller).await?;
    match db::create_pr_run(&state.pool, caller.org_id(), id, caller.user_id()).await? {
        PrOutcome::Created(run, ev) => {
            tag_run(&state, &caller, run.id).await?;
            state.events.publish(ev);
            tracing::info!(run_id = %run.id, thread = %id, "pull request queued");
            Ok((StatusCode::ACCEPTED, Json(CreateRunResponse { id: run.id, status: run.status })))
        }
        PrOutcome::NotFound => Err(not_found()),
        PrOutcome::Busy(_) => Err(ApiError(StatusCode::CONFLICT, "the thread is still running; wait for it to finish or stop it".into())),
        PrOutcome::InPlace => Err(ApiError(StatusCode::CONFLICT, "this task ran in the checkout itself, so it has no branch of its own to open a pull request from".into())),
        PrOutcome::NoMachine => Err(ApiError(StatusCode::CONFLICT, "no machine has run this task yet".into())),
    }
}

/// Merge a room task's branch into the room's branch, on the machine with its worktree.
async fn integrate_run(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    let root = own_thread(&state, &caller, id).await?;
    let room = match root.room_id {
        Some(r) => crate::rooms::get(&state.pool, caller.org_id(), r).await?,
        None => None,
    };
    let Some(room) = room else {
        return Err(ApiError(StatusCode::CONFLICT, "this task isn't in a room".into()));
    };
    check_run_limit(&state, &caller).await?;
    match db::create_integrate_run(&state.pool, caller.org_id(), id, caller.user_id(), &room.branch).await? {
        PrOutcome::Created(run, ev) => {
            state.events.publish(ev);
            tracing::info!(run_id = %run.id, thread = %root.id, room = %room.id, "integration queued");
            Ok((StatusCode::ACCEPTED, Json(CreateRunResponse { id: run.id, status: run.status })))
        }
        PrOutcome::NotFound => Err(not_found()),
        PrOutcome::Busy(_) => Err(ApiError(StatusCode::CONFLICT, "the task is still running; wait for it to finish or stop it".into())),
        PrOutcome::InPlace => Err(ApiError(StatusCode::CONFLICT, "this task ran in the checkout itself, so it has no branch to merge".into())),
        PrOutcome::NoMachine => Err(ApiError(StatusCode::CONFLICT, "no machine has run this task yet".into())),
    }
}

/// Fork the thread at this message into a new thread, with any agent and model.
async fn fork_run(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
    Json(req): Json<ForkRequest>,
) -> Result<(StatusCode, Json<CreateRunResponse>), ApiError> {
    let task = req.task.trim();
    if task.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "message must not be empty".into()));
    }
    if task.len() > MAX_TASK_BYTES {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("message is too long ({} characters; the limit is {})", task.chars().count(), MAX_TASK_BYTES)));
    }
    let agent = parse_agent(req.agent.as_deref())?;
    let model = req.model.trim();
    agent.check_model(model).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    let effort = check_effort(agent, req.effort.as_deref())?;
    let mode = check_permission_mode(req.permission_mode.as_deref())?;
    check_ask_agent(mode, agent)?;
    let context_budget = crate::fork::budget(task);
    let worker = own_machine(&state, &caller, req.worker_id.as_deref()).await?;
    let new = db::NewFork { task, agent, model, effort, permission_mode: mode, worker_id: worker, context_budget, created_by: caller.user_id() };
    let Some(target) = db::get_run(&state.pool, caller.org_id(), id).await? else {
        return Err(not_found());
    };
    // Without a mode the fork keeps the message's.
    check_full_access(&caller, mode.or(target.permission_mode.as_deref()))?;
    check_run_limit(&state, &caller).await?;
    match db::create_fork(&state.pool, caller.org_id(), id, new).await? {
        ForkOutcome::Created(run, ev) => {
            tag_run(&state, &caller, run.id).await?;
            state.events.publish(ev);
            tracing::info!(run_id = %run.id, forked_from = %id, agent = %run.agent, model = %run.model, "fork queued");
            Ok((StatusCode::ACCEPTED, Json(CreateRunResponse { id: run.id, status: run.status })))
        }
        ForkOutcome::NotFound => Err(not_found()),
        ForkOutcome::NotFinished(_) => Err(ApiError(StatusCode::CONFLICT, "this message is still running; fork it once it finishes".into())),
        ForkOutcome::AgentMissing(w) => Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("{w} doesn't have {}; pick an agent installed there, or another machine", agent.as_str()),
        )),
    }
}

async fn get_run(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
) -> Result<Json<Run>, ApiError> {
    db::get_run(&state.pool, caller.org_id(), id).await?.map(Json).ok_or_else(not_found)
}

/// An image from a tool result. Fetched by the page with its key, never embedded by URL.
async fn get_image(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> Result<Response, ApiError> {
    image_response(&state, caller.org_id(), id).await
}

/// An image attached to a message, for the worker that runs it.
async fn worker_get_image(State(state): State<AppState>, Extension(auth): Extension<WorkerAuth>, Path(id): Path<Uuid>) -> Result<Response, ApiError> {
    image_response(&state, auth.org, id).await
}

/// An image of `org`'s runs; other orgs' images read as missing.
async fn image_response(state: &AppState, org: Uuid, id: Uuid) -> Result<Response, ApiError> {
    let (media_type, data) = db::get_image(&state.pool, org, id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "image not found".into()))?;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, media_type),
            (axum::http::header::CACHE_CONTROL, "private, max-age=31536000, immutable".to_string()),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        data,
    )
        .into_response())
}

async fn stop_run(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
) -> Result<Json<Run>, ApiError> {
    // Stopping is always safe, so an org owner may stop anyone's.
    if !caller.is_owner() {
        own_thread(&state, &caller, id).await?;
    }
    let (run, ev) = db::request_stop(&state.pool, caller.org_id(), id).await?.ok_or_else(not_found)?;
    if let Some(ev) = ev {
        state.events.publish(ev);
    }
    tracing::info!(run_id = %id, status = run.status.as_str(), "stop requested");
    Ok(Json(run))
}

/// What the page needs to show a copy-paste "connect a machine" command. In
/// the default org it hands out the first static worker token; other orgs
/// connect machines by approving them on the page or with a token they create.
async fn setup(State(state): State<AppState>, Extension(caller): Extension<Caller>, headers: HeaderMap) -> Json<serde_json::Value> {
    let default = caller.org_id() == DEFAULT_ORG;
    let static_tokens = if default { state.config.worker_tokens.as_slice() } else { &[] };
    Json(json!({
        "control_url": public_base(&state, &headers),
        "worker_token": static_tokens.first().cloned().unwrap_or_default(),
        "worker_token_count": static_tokens.len(),
        "version": env!("CARGO_PKG_VERSION"),
        "build": state.config.build,
        "host_worker": if default { state.config.host_worker.clone() } else { None },
        "hosted": state.config.hosted(),
    }))
}

async fn list_workers(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<serde_json::Value>, ApiError> {
    let org = caller.org_id();
    let workers = db::list_workers(&state.pool, org).await?;
    let mut limits = db::active_limits(&state.pool, org).await?;
    let mut out = Vec::with_capacity(workers.len());
    let owners = db::worker_owners(&state.pool, org).await?;
    for w in workers {
        let online = db::is_online(w.last_heartbeat_at);
        let mut v = serde_json::to_value(&w).unwrap_or(json!({}));
        v["online"] = json!(online);
        let owner = owners.get(&w.id).cloned().flatten();
        v["mine"] = json!(owner.as_ref().map(|(id, _)| *id) == caller.user_id());
        v["owner"] = owner.map_or(json!(null), |(id, login)| json!({ "id": id, "login": login }));
        // Agents whose usage limit this machine hit: agent -> when it resets.
        v["limited_until"] = json!(limits.remove(&w.id).unwrap_or_default());
        v["update_requested"] = json!(db::update_requested(&state.pool, org, &w.id).await?);
        v["outdated"] = json!(!w.capabilities.version.is_empty() && w.capabilities.version != state.config.build);
        out.push(v);
    }
    Ok(Json(json!(out)))
}

/// Ask a machine to update itself and restart when it is idle.
async fn request_update(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    own_or_org_owner(&state, &caller, &id).await?;
    if db::set_update_requested(&state.pool, caller.org_id(), &id, true).await? {
        tracing::info!(worker = %id, "update requested");
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "unknown machine".into()))
    }
}

/// Forget a machine (frees a slot on the plan). It reappears if it heart-beats
/// again with a valid token; revoke its token to keep it out.
async fn delete_worker(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    own_or_org_owner(&state, &caller, &id).await?;
    if db::delete_worker(&state.pool, caller.org_id(), &id).await? {
        tracing::info!(worker = %id, "machine removed");
        Ok(StatusCode::NO_CONTENT)
    } else if db::worker_exists(&state.pool, caller.org_id(), &id).await? {
        Err(ApiError(StatusCode::CONFLICT, "a task is still running on this machine; stop it first".into()))
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "unknown machine".into()))
    }
}

/// Managing a machine (update, remove) is for its owner, or an owner of the org.
async fn own_or_org_owner(state: &AppState, caller: &Caller, worker: &str) -> Result<(), ApiError> {
    if caller.is_owner() {
        return Ok(());
    }
    match db::worker_owner(&state.pool, caller.org_id(), worker).await? {
        o if o == caller.user_id() => Ok(()),
        _ => Err(forbidden("only the machine's owner (or an owner of the org) can do that")),
    }
}

/// Hide a thread from the list (or bring it back). Any run of the thread may be named.
async fn archive_run(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
    Json(req): Json<ArchiveRequest>,
) -> Result<Json<Run>, ApiError> {
    if !caller.is_owner() {
        own_thread(&state, &caller, id).await?;
    }
    let root = db::set_archived(&state.pool, caller.org_id(), id, req.archived).await?.ok_or_else(not_found)?;
    tracing::info!(run_id = %root.id, archived = req.archived, "thread archive set");
    state.inbox.signal(caller.org_id()); // archived tasks leave the inbox
    Ok(Json(root))
}

// ---- tool approvals ("Ask me") ----

/// Every approval the run asked for, oldest first.
async fn list_approvals(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> Result<Json<Vec<Approval>>, ApiError> {
    if db::get_run(&state.pool, caller.org_id(), id).await?.is_none() {
        return Err(not_found());
    }
    Ok(Json(crate::approvals::list(&state.pool, caller.org_id(), id).await?))
}

async fn approve_tool(state: State<AppState>, caller: Extension<Caller>, path: Path<(Uuid, Uuid)>, body: axum::body::Bytes) -> Result<Json<Approval>, ApiError> {
    decide_tool(state, caller, path, body, true).await
}

async fn deny_tool(state: State<AppState>, caller: Extension<Caller>, path: Path<(Uuid, Uuid)>, body: axum::body::Bytes) -> Result<Json<Approval>, ApiError> {
    decide_tool(state, caller, path, body, false).await
}

/// Approve or deny a tool call the agent is waiting on. The body (`{message?}`) is optional.
async fn decide_tool(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, approval)): Path<(Uuid, Uuid)>,
    body: axum::body::Bytes,
    approve: bool,
) -> Result<Json<Approval>, ApiError> {
    // The tool call runs on the creator's machine, so only they decide it.
    own_thread(&state, &caller, id).await?;
    let req: DecideApprovalRequest = if body.iter().all(u8::is_ascii_whitespace) {
        DecideApprovalRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("invalid body: {e}")))?
    };
    let by = caller.user.as_ref().map_or_else(|| "developer key".to_string(), |u| u.login.clone());
    let outcome = crate::approvals::decide(&state.pool, caller.org_id(), id, approval, approve, &by, req.message.as_deref()).await?;
    let (status, msg) = match outcome {
        DecideOutcome::Decided(a, ev) => {
            state.events.publish(ev);
            tracing::info!(run_id = %id, approval = %approval, tool = %a.tool_name, approved = approve, "tool call decided");
            return Ok(Json(a));
        }
        DecideOutcome::NotFound => return Err(ApiError(StatusCode::NOT_FOUND, "approval not found".into())),
        DecideOutcome::AlreadyDecided(a) => (StatusCode::CONFLICT, format!("this tool call was already {}", a.status.as_str())),
        DecideOutcome::Expired(_, ev) => {
            ev.into_iter().for_each(|e| state.events.publish(e));
            (StatusCode::GONE, "too late: the wait for a decision ran out and the tool call was denied".to_string())
        }
        DecideOutcome::RunEnded(_, ev) => {
            ev.into_iter().for_each(|e| state.events.publish(e));
            (StatusCode::CONFLICT, "the run is no longer running".to_string())
        }
    };
    Err(ApiError(status, msg))
}

#[derive(Deserialize)]
struct UpdateAck {
    id: String,
}

/// The worker is about to update: clear the flag so it is not asked again.
async fn update_ack(
    State(state): State<AppState>,
    Extension(w): Extension<WorkerAuth>,
    Json(req): Json<UpdateAck>,
) -> Result<StatusCode, ApiError> {
    if !db::worker_owns_token(&state.pool, w.org, &req.id, &w.token_hash).await? {
        return Err(ApiError(StatusCode::NOT_FOUND, "worker not registered".into()));
    }
    db::set_update_requested(&state.pool, w.org, &req.id, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AddedRepos {
    added: Vec<String>,
}

/// The repos the developer chose to show in the composer (out of everything the machines report).
async fn get_added_repos(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(json!({ "added": db::list_added_repos(&state.pool, caller.org_id()).await? })))
}

async fn put_added_repos(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(req): Json<AddedRepos>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut names: Vec<String> = req.added.into_iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
    names.sort();
    names.dedup();
    if names.len() > 1000 || names.iter().any(|n| n.len() > 512) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "too many or too long repo names".into()));
    }
    db::set_added_repos(&state.pool, caller.org_id(), &names).await?;
    Ok(Json(json!({ "added": names })))
}

#[derive(Deserialize)]
struct EventsQuery {
    /// Start replay from this seq (default 0). `Last-Event-ID` takes precedence.
    from: Option<i64>,
}

/// SSE: replays stored events from `from`/`Last-Event-ID + 1`, then streams
/// live ones. Each event's `id` is its seq, so browsers resume on reconnect.
/// Between them come `assistant_live` frames without an id: the text the agent
/// is writing, which the next stored `assistant_text` replaces.
/// Ends with an `end` event once the run reaches a terminal status.
async fn run_events_sse(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
    Query(q): Query<EventsQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let run = db::get_run(&state.pool, caller.org_id(), id).await?.ok_or_else(not_found)?;
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
                Ok(Update::Live(text)) => yield Ok(live_sse(id, &text)),
                Ok(Update::Event(ev)) => {
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

/// Shaped like a stored event so the page handles both alike, but with no seq.
fn live_sse(run_id: Uuid, text: &str) -> Event {
    let data = json!({"run_id": run_id, "kind": LIVE_TEXT, "payload": {"text": text}, "at": chrono::Utc::now()});
    Event::default().event(LIVE_TEXT).data(data.to_string())
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
    Extension(auth): Extension<WorkerAuth>,
    Json(req): Json<RegisterWorkerRequest>,
) -> Result<Json<sidefleet_core::Worker>, ApiError> {
    if req.id.trim().is_empty() || req.id.len() > 128 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "worker id must be 1-128 chars".into()));
    }
    if let Some(l) = tenancy::org_limits(&state.pool, auth.org).await? {
        if !db::worker_exists(&state.pool, auth.org, &req.id).await? && db::count_workers(&state.pool, auth.org).await? >= l.machines {
            return Err(ApiError(
                StatusCode::PAYMENT_REQUIRED,
                format!("this org's plan allows {} machines; remove one on the Machines page or upgrade", l.machines),
            ));
        }
    }
    let Some(w) = db::upsert_worker(&state.pool, auth.org, &req.id, &req.display_name, &auth.token_hash, &req.capabilities).await? else {
        return Err(ApiError(StatusCode::CONFLICT, format!("a teammate's machine is already registered as {}; pick another machine id (--id)", req.id)));
    };
    tracing::info!(worker = %w.id, org = %auth.org, repos = w.capabilities.repos.len(), "worker registered");
    for ev in db::reconcile_worker_runs(&state.pool, auth.org, &req.id, &req.active_runs).await? {
        tracing::warn!(run_id = %ev.run_id, worker = %req.id, "run lost by worker restart; marked finished");
        state.events.publish(ev);
    }
    Ok(Json(w))
}

async fn heartbeat(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Json(req): Json<HeartbeatRequest>,
) -> Result<Json<HeartbeatResponse>, ApiError> {
    if db::heartbeat(&state.pool, auth.org, &req.id, &auth.token_hash, &req.capabilities).await? {
        for ev in db::reconcile_worker_runs(&state.pool, auth.org, &req.id, &req.active_runs).await? {
            tracing::warn!(run_id = %ev.run_id, worker = %req.id, "run no longer executing on its worker; marked finished");
            state.events.publish(ev);
        }
        Ok(Json(HeartbeatResponse {
            server_version: state.config.build.clone(),
            update_requested: db::update_requested(&state.pool, auth.org, &req.id).await?,
            check_prs: db::prs_to_check(&state.pool, auth.org, &req.id).await?,
        }))
    } else {
        // Unknown worker (e.g. control plane restarted with a fresh DB): tell it to re-register.
        Err(ApiError(StatusCode::NOT_FOUND, "worker not registered".into()))
    }
}

async fn claim_run(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Json(req): Json<ClaimRequest>,
) -> Result<Response, ApiError> {
    if !db::worker_owns_token(&state.pool, auth.org, &req.worker_id, &auth.token_hash).await? {
        return Err(ApiError(StatusCode::NOT_FOUND, "worker not registered".into()));
    }
    if req.repos.is_empty() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    // Workers from before multi-agent support only run Claude Code.
    let agents = if req.agents.is_empty() { vec![Agent::Claude.as_str().to_string()] } else { req.agents.clone() };
    // Workers from before restores and forks only run plain agent runs.
    let kinds = if req.kinds.is_empty() { vec!["agent".to_string()] } else { req.kinds.clone() };
    // A long poll: subscribe before the first look, so a run queued in between still wakes us.
    let wait = Duration::from_secs(req.wait_secs.unwrap_or(0).min(MAX_CLAIM_WAIT_SECS));
    let deadline = tokio::time::Instant::now() + wait;
    let mut wake = state.claims.subscribe(auth.org);
    loop {
        let max_running = tenancy::org_limits(&state.pool, auth.org).await?.map(|l| l.parallel_runs);
        let claimed =
            db::claim_run(&state.pool, auth.org, &req.worker_id, &req.repos, &req.busy_repos, &agents, req.slots.as_ref(), &kinds, max_running)
                .await?;
        for ev in claimed.failed {
            tracing::warn!(run_id = %ev.run_id, "queued message had no session to continue; failed");
            state.events.publish(ev);
        }
        if let Some((run, ev)) = claimed.run {
            state.events.publish(ev);
            tracing::info!(run_id = %run.id, worker = %req.worker_id, "run claimed");
            return Ok(Json(run).into_response());
        }
        let now = tokio::time::Instant::now();
        if now >= deadline || state.claims.is_closed() {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        // Also look again every few seconds: some runs become claimable by time alone
        // (the least-busy grace, a failover exclusion or a usage limit running out).
        let recheck = tokio::time::sleep_until(deadline.min(now + CLAIM_RECHECK));
        tokio::select! {
            Ok(()) = wake.changed() => {}
            _ = recheck => {}
        }
    }
}

async fn report_prs(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Json(req): Json<sidefleet_core::PrReports>,
) -> Result<StatusCode, ApiError> {
    if !db::worker_owns_token(&state.pool, auth.org, &req.worker_id, &auth.token_hash).await? {
        return Err(ApiError(StatusCode::NOT_FOUND, "worker not registered".into()));
    }
    if req.prs.len() > 100 || req.prs.iter().any(|r| !matches!(r.pr.state.as_str(), "open" | "merged" | "closed") || !r.pr.url.starts_with("https://")) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "bad pull request report".into()));
    }
    db::record_prs(&state.pool, auth.org, &req.worker_id, &req.prs).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct WorkerQuery {
    worker_id: String,
}

/// A worker acting on a run names itself; its token must be the one that machine registered with,
/// or another member's token could write to, finish or ask approvals in someone else's runs.
async fn worker_is(state: &AppState, auth: &WorkerAuth, worker_id: &str) -> Result<(), ApiError> {
    if db::worker_owns_token(&state.pool, auth.org, worker_id, &auth.token_hash).await? {
        Ok(())
    } else {
        Err(ApiError(StatusCode::FORBIDDEN, "this token doesn't belong to that machine".into()))
    }
}

async fn post_events(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Path(id): Path<Uuid>,
    Query(q): Query<WorkerQuery>,
    Json(req): Json<PostEventsRequest>,
) -> Result<Json<RunControl>, ApiError> {
    if req.events.len() > 1000 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "too many events in one batch".into()));
    }
    worker_is(&state, &auth, &q.worker_id).await?;
    match db::append_events(&state.pool, auth.org, id, &q.worker_id, &req.events).await? {
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

/// The text the agent is writing, passed straight to whoever follows the run:
/// never stored, as the `assistant_text` event that ends it is.
async fn post_live_text(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Path(id): Path<Uuid>,
    Query(q): Query<WorkerQuery>,
    Json(req): Json<LiveTextRequest>,
) -> Result<StatusCode, ApiError> {
    // Redaction can lengthen the text a little.
    if req.text.len() > 2 * LIVE_TEXT_MAX_BYTES {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "live text too long".into()));
    }
    // Most of the time nobody is looking: skip the database.
    if !state.events.watched(id) {
        return Ok(StatusCode::NO_CONTENT);
    }
    let run = db::get_run(&state.pool, auth.org, id).await?.ok_or_else(not_found)?;
    worker_is(&state, &auth, &q.worker_id).await?;
    if run.worker_id.as_deref() != Some(q.worker_id.as_str()) {
        return Err(ApiError(StatusCode::FORBIDDEN, "run belongs to another worker".into()));
    }
    if run.status == RunStatus::Running {
        state.events.publish_live(id, sidefleet_core::redact(&req.text));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn run_control(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Path(id): Path<Uuid>,
) -> Result<Json<RunControl>, ApiError> {
    let run = db::get_run(&state.pool, auth.org, id).await?.ok_or_else(not_found)?;
    Ok(Json(RunControl {
        status: run.status,
        stop_requested: run.stop_requested || run.status.is_terminal(),
    }))
}

/// The worker's agent wants to use a tool the permission mode leaves to the person:
/// store the question and show it on the page. The worker then polls for the answer.
async fn request_approval(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Path(id): Path<Uuid>,
    Query(q): Query<WorkerQuery>,
    Json(req): Json<ApprovalRequest>,
) -> Result<(StatusCode, Json<Approval>), ApiError> {
    if req.tool_name.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "tool_name must not be empty".into()));
    }
    worker_is(&state, &auth, &q.worker_id).await?;
    match crate::approvals::request(&state.pool, auth.org, id, &q.worker_id, &req).await? {
        RequestOutcome::Created(a, ev) => {
            state.events.publish(ev);
            tracing::info!(run_id = %id, approval = %a.id, tool = %a.tool_name, "tool call waiting for approval");
            crate::chat_updates::approval_requested(&state, auth.org, &a);
            Ok((StatusCode::CREATED, Json(*a)))
        }
        RequestOutcome::NotFound => Err(not_found()),
        RequestOutcome::NotOwner => Err(ApiError(StatusCode::FORBIDDEN, "run belongs to another worker".into())),
        RequestOutcome::NotRunning => Err(ApiError(StatusCode::CONFLICT, "the run is not running".into())),
    }
}

/// The approval as it stands. Past its deadline it expires; once its run ends it is cancelled.
async fn poll_approval(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Path((id, approval)): Path<(Uuid, Uuid)>,
) -> Result<Json<Approval>, ApiError> {
    let (a, ev) = crate::approvals::poll(&state.pool, auth.org, id, approval, chrono::Utc::now())
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "approval not found".into()))?;
    if let Some(ev) = ev {
        state.events.publish(ev);
    }
    Ok(Json(a))
}

async fn finish_run(
    State(state): State<AppState>,
    Extension(auth): Extension<WorkerAuth>,
    Path(id): Path<Uuid>,
    Query(q): Query<WorkerQuery>,
    Json(req): Json<FinishRunRequest>,
) -> Result<Json<Run>, ApiError> {
    if !req.status.is_terminal() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "finish status must be terminal".into()));
    }
    worker_is(&state, &auth, &q.worker_id).await?;
    match db::finish_run(&state.pool, auth.org, id, &q.worker_id, req.status, req.result, req.usage_limit.as_ref()).await? {
        FinishOutcome::Finished(run, evs) => {
            for ev in evs {
                state.events.publish(ev);
            }
            if req.usage_limit.is_some() {
                tracing::warn!(run_id = %id, worker = %q.worker_id, agent = %run.agent, "usage limit hit; no other machine can take the run now");
            }
            tracing::info!(run_id = %id, status = run.status.as_str(), "run finished");
            crate::chat_updates::run_finished(&state, auth.org, &run);
            Ok(Json(run))
        }
        FinishOutcome::Requeued(run, evs) => {
            for ev in evs {
                state.events.publish(ev);
            }
            tracing::warn!(run_id = %id, worker = %q.worker_id, agent = %run.agent, "usage limit hit; run re-queued for another machine");
            Ok(Json(run))
        }
        FinishOutcome::NotFound => Err(not_found()),
        FinishOutcome::NotOwner => Err(ApiError(StatusCode::FORBIDDEN, "run belongs to another worker".into())),
        FinishOutcome::AlreadyFinished(run) => Ok(Json(run)),
    }
}

// ---- sign-in, orgs, tokens ----

/// Public: which ways of signing in this control plane offers.
async fn auth_methods(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "github": state.config.github.is_some(),
        "dev_key": !state.config.dev_key.is_empty(),
        "hosted": state.config.hosted(),
    }))
}

/// The signed-in user (null for the developer key) and the orgs they can act in.
async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let identity = match auth::identify(&state, &headers).await {
        Ok(Some(i)) => i,
        Ok(None) => return Ok((StatusCode::UNAUTHORIZED, Json(json!({"error": "not signed in"}))).into_response()),
        Err(r) => return Ok(r),
    };
    let (user, orgs, token) = match identity {
        Identity::DevKey => (None, vec![tenancy::default_org(&state.pool).await?], None),
        Identity::User(u) => {
            let orgs = tenancy::memberships(&state.pool, u.id).await?;
            (Some(u), orgs, None)
        }
        // A token sees only its own org, and says what it may do.
        Identity::ApiToken(t) => (t.user, vec![t.org], Some(json!({ "id": t.id, "name": t.name, "scope": t.scope }))),
    };
    let mut v = json!({ "user": user, "orgs": orgs });
    if let Some(t) = token {
        v["token"] = t;
    }
    Ok(Json(v).into_response())
}

/// The org named in a path (id or slug): the caller's current org, or another org they belong to.
async fn path_org(state: &AppState, caller: &Caller, org: &str) -> Result<tenancy::OrgMembership, ApiError> {
    if caller.org.id.to_string() == org || caller.org.slug == org {
        return Ok(caller.org.clone());
    }
    if let Some(u) = &caller.user {
        if let Some(m) = tenancy::memberships(&state.pool, u.id).await?.into_iter().find(|m| m.id.to_string() == org || m.slug == org) {
            return Ok(m);
        }
    }
    Err(ApiError(StatusCode::NOT_FOUND, "org not found".into()))
}

async fn owned_org(state: &AppState, caller: &Caller, org: &str) -> Result<tenancy::OrgMembership, ApiError> {
    let m = path_org(state, caller, org).await?;
    if m.role != "owner" {
        return Err(forbidden("only an owner of this org can do that"));
    }
    Ok(m)
}

#[derive(Deserialize)]
struct NameReq {
    name: String,
}

fn check_name(name: &str, what: &str) -> Result<String, ApiError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 80 {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("{what} must be 1-80 characters")));
    }
    Ok(n.to_string())
}

/// A team org, owned by the caller.
async fn create_org(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(req): Json<NameReq>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let Some(user) = &caller.user else {
        return Err(forbidden("sign in with GitHub to create orgs"));
    };
    let name = check_name(&req.name, "org name")?;
    let id = tenancy::create_org(&state.pool, &name, user.id).await?;
    let org = tenancy::memberships(&state.pool, user.id).await?.into_iter().find(|m| m.id == id);
    Ok((StatusCode::CREATED, Json(json!(org))))
}

/// Rename an org (owners). Its slug stays, so links and saved org choices keep working.
async fn rename_org(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(org): Path<String>, Json(req): Json<NameReq>) -> Result<Json<serde_json::Value>, ApiError> {
    let org = owned_org(&state, &caller, &org).await?;
    let name = check_name(&req.name, "org name")?;
    tenancy::rename_org(&state.pool, org.id, &name).await?;
    tracing::info!(org = %org.id, "org renamed");
    Ok(Json(json!({ "id": org.id, "name": name })))
}

async fn list_members(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(org): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let org = path_org(&state, &caller, &org).await?;
    Ok(Json(json!(tenancy::list_members(&state.pool, org.id).await?)))
}

#[derive(Deserialize)]
struct AddMemberReq {
    login: String,
    #[serde(default)]
    role: Option<String>,
}

/// Add someone who has signed in at least once, by GitHub login. Owners only.
async fn add_member(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(org): Path<String>, Json(req): Json<AddMemberReq>) -> Result<Json<serde_json::Value>, ApiError> {
    let org = owned_org(&state, &caller, &org).await?;
    if org.id == DEFAULT_ORG {
        return Err(forbidden("the default org belongs to whoever holds the developer key"));
    }
    let role = req.role.as_deref().unwrap_or("member");
    if !["owner", "member"].contains(&role) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "role must be owner or member".into()));
    }
    match tenancy::add_member(&state.pool, org.id, req.login.trim(), role).await? {
        tenancy::AddMember::Added(u) => Ok(Json(json!({ "login": u.login, "role": role }))),
        tenancy::AddMember::NoSuchUser => Err(ApiError(StatusCode::NOT_FOUND, "no Sidefleet user with that GitHub login; they need to sign in once first".into())),
    }
}

// ---- API tokens ----

/// The caller's own API tokens in the current org (the page or the developer key; a token
/// can't reach these routes).
async fn list_api_tokens(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(json!(tenancy::list_api_tokens(&state.pool, caller.org_id(), caller.user_id()).await?)))
}

#[derive(Deserialize)]
struct ApiTokenReq {
    name: String,
    scope: String,
}

/// Mint an API token that acts as the caller in the current org. The value is in this response only.
async fn create_api_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(req): Json<ApiTokenReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = check_name(&req.name, "token name")?;
    let scope = tenancy::Scope::parse(req.scope.trim()).ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "scope must be one of read, tasks, full".into()))?;
    let (token, value) = tenancy::create_api_token(&state.pool, caller.org_id(), caller.user_id(), &name, scope).await?;
    tracing::info!(org = %caller.org_id(), token = %token.id, scope = scope.as_str(), "API token created");
    let mut v = json!(token);
    v["token"] = json!(value);
    Ok((StatusCode::CREATED, Json(v)))
}

async fn revoke_api_token(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, ApiError> {
    let t = tenancy::revoke_api_token(&state.pool, caller.org_id(), caller.user_id(), id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "token not found".into()))?;
    tracing::info!(org = %caller.org_id(), token = %id, "API token revoked");
    Ok(Json(json!(t)))
}

#[derive(Deserialize)]
struct ChatUpdatesReq {
    kind: String,
    url: String,
    #[serde(default)]
    all_tasks: bool,
}

/// Turn on updates in chat for one of the caller's connections. The response carries the
/// secret the app checks updates with (Hermes route secret, OpenClaw hook token).
async fn set_chat_updates(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
    Json(req): Json<ChatUpdatesReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kind = req.kind.trim();
    if !crate::chat_updates::KINDS.contains(&kind) {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("kind must be one of {}", crate::chat_updates::KINDS.join(", "))));
    }
    let url = req.url.trim();
    if url.len() > 2000 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "that address is too long".into()));
    }
    crate::chat_updates::check_url(url, state.config.private_webhooks).await.map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    let (token, secret) = tenancy::set_chat_updates(&state.pool, caller.org_id(), caller.user_id(), id, kind, url, req.all_tasks)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "token not found".into()))?;
    tracing::info!(org = %caller.org_id(), token = %id, kind, "chat updates on");
    let mut v = json!(token);
    v["secret"] = json!(secret);
    Ok(Json(v))
}

async fn clear_chat_updates(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, ApiError> {
    let t = tenancy::clear_chat_updates(&state.pool, caller.org_id(), caller.user_id(), id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "token not found".into()))?;
    Ok(Json(json!(t)))
}

/// Send one test update now and say how it went (`502` with the reason when it didn't arrive).
async fn test_chat_updates(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> Result<Json<serde_json::Value>, ApiError> {
    let target = tenancy::chat_update_target(&state.pool, caller.org_id(), caller.user_id(), id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "updates aren't on for that connection".into()))?;
    crate::chat_updates::send_test(&state, &target).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_tokens(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(org): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let org = owned_org(&state, &caller, &org).await?;
    Ok(Json(json!(tenancy::list_tokens(&state.pool, org.id).await?)))
}

/// Mint a worker token. The value is in this response only.
async fn create_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(org): Path<String>,
    Json(req): Json<NameReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let org = owned_org(&state, &caller, &org).await?;
    let name = check_name(&req.name, "token name")?;
    let (token, value) = tenancy::create_token(&state.pool, org.id, &name, caller.user_id()).await?;
    tracing::info!(org = %org.id, token = %token.id, "worker token created");
    let mut v = json!(token);
    v["token"] = json!(value);
    Ok((StatusCode::CREATED, Json(v)))
}

async fn revoke_token(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path((org, id)): Path<(String, Uuid)>) -> Result<Json<serde_json::Value>, ApiError> {
    let org = owned_org(&state, &caller, &org).await?;
    let t = tenancy::revoke_token(&state.pool, org.id, id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "token not found".into()))?;
    tracing::info!(org = %org.id, token = %id, "worker token revoked");
    Ok(Json(json!(t)))
}

// ---- machine enrollment ----

#[derive(Deserialize)]
struct EnrollReq {
    worker_id: String,
    machine_name: String,
    #[serde(default)]
    capabilities: sidefleet_core::Capabilities,
}

/// Public: `sidefleet connect` without a token asks to join. The response holds
/// a code to show the person, and a secret only the waiting connect knows.
async fn enroll(State(state): State<AppState>, headers: HeaderMap, Json(req): Json<EnrollReq>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let worker_id = req.worker_id.trim();
    let name = req.machine_name.trim();
    if worker_id.is_empty() || worker_id.len() > 128 || name.is_empty() || name.len() > 128 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "worker_id and machine_name must be 1-128 characters".into()));
    }
    // Self-hosted: there is one org, so the request waits on its Machines page.
    // Hosted: whoever approves it with the code chooses the org.
    let org = if state.config.hosted() { None } else { Some(DEFAULT_ORG) };
    match tenancy::create_enrollment(&state.pool, org, name, worker_id, &req.capabilities).await? {
        NewEnrollment::Created(e, secret) => {
            tracing::info!(enrollment = %e.id, machine = %e.machine_name, "machine asked to join");
            let base = public_base(&state, &headers);
            Ok((
                StatusCode::CREATED,
                Json(json!({
                    "id": e.id,
                    "code": e.code,
                    "secret": secret,
                    "expires_at": e.expires_at,
                    "approve_url": format!("{base}/#/machines?code={}", e.code),
                    "poll_interval_secs": 2,
                })),
            ))
        }
        NewEnrollment::TooMany => Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "too many machines are waiting for approval; try again in a few minutes".into())),
    }
}

/// Public, authenticated by the enrollment secret: pending (202), approved with
/// the token (200, once), denied (403), expired or already collected (410).
async fn enroll_poll(State(state): State<AppState>, Path(id): Path<Uuid>, headers: HeaderMap) -> Result<Response, ApiError> {
    let secret = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    Ok(match tenancy::poll_enrollment(&state.pool, id, secret).await? {
        Poll::Pending(e) => (StatusCode::ACCEPTED, Json(json!({"status": "pending", "code": e.code, "expires_at": e.expires_at}))).into_response(),
        Poll::Approved { token, org } => {
            tracing::info!(enrollment = %id, org = %org.id, "machine approved; token collected");
            Json(json!({"status": "approved", "token": token, "org": {"id": org.id, "name": org.name, "slug": org.slug}})).into_response()
        }
        Poll::Denied => (StatusCode::FORBIDDEN, Json(json!({"status": "denied", "error": "the request to join was denied"}))).into_response(),
        Poll::Expired => (StatusCode::GONE, Json(json!({"status": "expired", "error": "the request expired before anyone approved it"}))).into_response(),
        Poll::Collected => (StatusCode::GONE, Json(json!({"status": "collected", "error": "the token for this request was already handed out"}))).into_response(),
        Poll::NotFound => return Err(ApiError(StatusCode::NOT_FOUND, "unknown enrollment".into())),
    })
}

async fn list_enrollments(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(json!(tenancy::pending_enrollments(&state.pool, caller.org_id()).await?)))
}

#[derive(Deserialize)]
struct CodeReq {
    #[serde(default)]
    code: Option<String>,
}

/// Find a waiting machine by the code its terminal shows.
async fn lookup_enrollment(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(req): Json<CodeReq>) -> Result<Json<serde_json::Value>, ApiError> {
    let code = req.code.unwrap_or_default();
    let e = tenancy::enrollment_by_code(&state.pool, caller.org_id(), &code).await?;
    e.map(|e| Json(json!(e))).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no machine is waiting with that code (it may have expired)".into()))
}

async fn approve_enrollment(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>, Json(req): Json<CodeReq>) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(l) = caller.org.limits {
        if db::count_workers(&state.pool, caller.org_id()).await? >= l.machines {
            return Err(over_limit(&caller.org.plan, format!("{} machines are allowed", l.machines)));
        }
    }
    decide(&state, &caller, id, req.code.as_deref(), true).await
}

async fn deny_enrollment(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>, Json(req): Json<CodeReq>) -> Result<Json<serde_json::Value>, ApiError> {
    decide(&state, &caller, id, req.code.as_deref(), false).await
}

async fn decide(state: &AppState, caller: &Caller, id: Uuid, code: Option<&str>, approve: bool) -> Result<Json<serde_json::Value>, ApiError> {
    match tenancy::decide_enrollment(&state.pool, caller.org_id(), caller.user_id(), id, code, approve).await? {
        Decide::Done(e) => {
            tracing::info!(enrollment = %id, org = %caller.org_id(), approved = approve, machine = %e.machine_name, "enrollment decided");
            Ok(Json(json!(e)))
        }
        Decide::NotFound => Err(ApiError(StatusCode::NOT_FOUND, "no machine is waiting with that id and code".into())),
        Decide::Expired => Err(ApiError(StatusCode::GONE, "the request expired; run `sidefleet connect` again on that machine".into())),
        Decide::AlreadyDecided(s) => Err(ApiError(StatusCode::CONFLICT, format!("this request was already {s}"))),
    }
}
