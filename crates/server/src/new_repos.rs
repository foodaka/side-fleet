//! New projects: the page asks one of your machines to start a git repo or clone one
//! into its repos folder. The machine takes the request with its next claim (at once,
//! when it is waiting for work) or heartbeat, makes it, and reports the repo, which goes
//! straight into the machine's list so a task can start in it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sidefleet_core::{clone_folder, valid_repo_folder, NewRepoDone, NewRepoJob, NewRepoSource};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::auth::{Caller, WorkerAuth};
use crate::AppState;

/// A request nobody took in this long is given up on (the machine went away).
const PENDING_FOR_SECS: f64 = 600.0;
/// ...and one taken but never reported, in this long (a clone has 10 minutes).
const RUNNING_FOR_SECS: f64 = 900.0;
const MAX_URL_BYTES: usize = 2000;

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

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/workers/{id}/new-repos", post(create)).route("/v1/new-repos/{id}", get(status))
}

pub fn worker_router() -> Router<AppState> {
    Router::new().route("/v1/worker/new-repos/{id}", post(done))
}

#[derive(Debug, Deserialize)]
struct CreateRequest {
    /// The folder's name. For a clone, defaults to the URL's last part.
    #[serde(default)]
    name: String,
    /// Clone this; none starts an empty repo.
    #[serde(default)]
    url: Option<String>,
}

/// Whether `url` looks like something `git clone` fetches over the network.
fn clonable(url: &str) -> bool {
    let u = url.trim();
    !u.is_empty()
        && u.len() <= MAX_URL_BYTES
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
        && (u.starts_with("https://") || u.starts_with("http://") || u.starts_with("ssh://") || u.starts_with("git://") || (u.contains('@') && u.contains(':') && !u.contains("://")))
}

/// Ask machine `id` to make a new project. Only the machine's owner may: it writes to their disk.
async fn create(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<String>, Json(req): Json<CreateRequest>) -> Result<Response, ApiError> {
    let org = caller.org_id();
    let row = sqlx::query("SELECT owner_id, capabilities->>'new_repos_root' AS root, (last_heartbeat_at > now() - interval '45 seconds') AS online FROM workers WHERE id = $1 AND org_id = $2")
        .bind(&id)
        .bind(org)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "unknown machine".into()))?;
    let owner: Option<Uuid> = row.get("owner_id");
    let allowed = match owner {
        Some(o) => Some(o) == caller.user_id(),
        None => caller.is_owner(),
    };
    if !allowed {
        return Err(ApiError(StatusCode::FORBIDDEN, "only the machine's owner can add projects to it".into()));
    }
    if row.get::<Option<String>, _>("root").is_none() {
        return Err(ApiError(StatusCode::CONFLICT, "this machine's Sidefleet is too old to make projects; update it first".into()));
    }
    if !row.get::<Option<bool>, _>("online").unwrap_or(false) {
        return Err(ApiError(StatusCode::CONFLICT, "this machine is offline".into()));
    }
    let url = req.url.as_deref().map(str::trim).filter(|u| !u.is_empty());
    if let Some(u) = url {
        if !clonable(u) {
            return Err(bad("that doesn't look like a git URL (https://… or git@host:owner/repo)"));
        }
    }
    let name = match (req.name.trim(), url) {
        ("", Some(u)) => clone_folder(u),
        (n, _) => n.to_string(),
    };
    if !valid_repo_folder(&name) {
        return Err(bad("a project name is letters, digits, - _ and ., and doesn't start with a dot"));
    }
    // A stuck request for the same name shouldn't block a new one forever.
    expire(&state.pool).await?;
    let busy = sqlx::query("SELECT 1 AS x FROM new_repos WHERE worker_id = $1 AND org_id = $2 AND lower(name) = lower($3) AND status IN ('pending', 'running')")
        .bind(&id)
        .bind(org)
        .bind(&name)
        .fetch_optional(&state.pool)
        .await?;
    if busy.is_some() {
        return Err(ApiError(StatusCode::CONFLICT, format!("{name} is already being made on this machine")));
    }
    let new_id = Uuid::new_v4();
    sqlx::query("INSERT INTO new_repos (id, org_id, worker_id, created_by, kind, name, url) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(new_id)
        .bind(org)
        .bind(&id)
        .bind(caller.user_id())
        .bind(if url.is_some() { "clone" } else { "init" })
        .bind(&name)
        .bind(url)
        .execute(&state.pool)
        .await?;
    tracing::info!(worker = %id, %name, clone = url.is_some(), "new project requested");
    // Wake the machine's waiting claim, which takes it.
    state.claims.signal(org);
    Ok((StatusCode::ACCEPTED, Json(json!({"id": new_id, "name": name}))).into_response())
}

/// How a request is getting on: `pending`, `running`, `done` (with `repo` and `path`) or `failed` (with `error`).
async fn status(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> Result<Json<Value>, ApiError> {
    expire(&state.pool).await?;
    let r = sqlx::query("SELECT id, worker_id, kind, name, status, repo, path, error, warning FROM new_repos WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(caller.org_id())
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such request".into()))?;
    let s = |k: &str| r.get::<Option<String>, _>(k);
    Ok(Json(json!({
        "id": r.get::<Uuid, _>("id"), "worker_id": s("worker_id"), "kind": s("kind"), "name": s("name"), "status": s("status"),
        "repo": s("repo"), "path": s("path"), "error": s("error"), "warning": s("warning"),
    })))
}

/// Fail requests a machine never took or never answered.
async fn expire(pool: &PgPool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE new_repos SET status = 'failed', finished_at = now(),
                error = CASE status WHEN 'pending' THEN 'The machine never picked this up. Is Sidefleet still running there?'
                                    ELSE 'The machine stopped answering before it finished.' END
         WHERE (status = 'pending' AND created_at < now() - make_interval(secs => $1))
            OR (status = 'running' AND taken_at < now() - make_interval(secs => $2))",
    )
    .bind(PENDING_FOR_SECS)
    .bind(RUNNING_FOR_SECS)
    .execute(pool)
    .await?;
    Ok(())
}

/// Hand machine `worker` the requests waiting for it in any org it serves, each once.
pub async fn take(pool: &PgPool, orgs: &[Uuid], worker: &str) -> sqlx::Result<Vec<NewRepoJob>> {
    let rows = sqlx::query(
        "UPDATE new_repos SET status = 'running', taken_at = now()
         WHERE worker_id = $1 AND org_id = ANY($2) AND status = 'pending' AND created_at > now() - make_interval(secs => $3)
         RETURNING id, kind, name, url",
    )
    .bind(worker)
    .bind(orgs)
    .bind(PENDING_FOR_SECS)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| NewRepoJob {
            id: r.get("id"),
            name: r.get("name"),
            source: match r.get::<String, _>("kind").as_str() {
                "clone" => NewRepoSource::Clone { url: r.get::<Option<String>, _>("url").unwrap_or_default() },
                _ => NewRepoSource::Init,
            },
        })
        .collect())
}

/// The machine made the project, or couldn't. A made one joins the machine's repo list now,
/// not at its next heartbeat, and the org's chosen repos when it has chosen some.
async fn done(State(state): State<AppState>, Extension(auth): Extension<WorkerAuth>, Path(id): Path<Uuid>, Json(req): Json<NewRepoDone>) -> Result<StatusCode, ApiError> {
    if !crate::db::worker_owns_token(&state.pool, auth.org, &req.worker_id, &auth.token_hash).await? {
        return Err(ApiError(StatusCode::NOT_FOUND, "worker not registered".into()));
    }
    let mut tx = state.pool.begin().await?;
    let Some(row) = sqlx::query("SELECT org_id FROM new_repos WHERE id = $1 AND worker_id = $2 AND org_id = ANY($3) AND status IN ('pending', 'running') FOR UPDATE")
        .bind(id)
        .bind(&req.worker_id)
        .bind(&auth.orgs)
        .fetch_optional(&mut *tx)
        .await?
    else {
        // Expired, or reported twice: nothing to do.
        return Ok(StatusCode::NO_CONTENT);
    };
    let org: Uuid = row.get("org_id");
    let clip = |s: &Option<String>| s.as_ref().map(|s| s.chars().take(1000).collect::<String>());
    match &req.repo {
        Some(repo) => {
            sqlx::query("UPDATE new_repos SET status = 'done', repo = $2, path = $3, warning = $4, finished_at = now() WHERE id = $1")
                .bind(id)
                .bind(repo.key())
                .bind(&repo.path)
                .bind(clip(&req.warning))
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "UPDATE workers SET capabilities = jsonb_set(capabilities, '{repos}', COALESCE(capabilities->'repos', '[]'::jsonb) || jsonb_build_array($3::jsonb))
                 WHERE id = $1 AND org_id = ANY($2)
                   AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(capabilities->'repos', '[]'::jsonb)) r WHERE r->>'path' = $4)",
            )
            .bind(&req.worker_id)
            .bind(&auth.orgs)
            .bind(json!(repo))
            .bind(&repo.path)
            .execute(&mut *tx)
            .await?;
            // An org with no chosen repos sees all of them; choosing this one would hide the rest.
            sqlx::query("INSERT INTO added_repos (org_id, name) SELECT $1, $2 WHERE EXISTS (SELECT 1 FROM added_repos WHERE org_id = $1) ON CONFLICT DO NOTHING")
                .bind(org)
                .bind(repo.key())
                .execute(&mut *tx)
                .await?;
            tracing::info!(worker = %req.worker_id, repo = repo.key(), "new project made");
        }
        None => {
            let error = clip(&req.error).unwrap_or_else(|| "the machine couldn't make it".into());
            sqlx::query("UPDATE new_repos SET status = 'failed', error = $2, finished_at = now() WHERE id = $1").bind(id).bind(&error).execute(&mut *tx).await?;
            tracing::warn!(worker = %req.worker_id, %error, "new project failed");
        }
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::clonable;

    #[test]
    fn clone_urls() {
        for ok in ["https://github.com/acme/app.git", "git@github.com:acme/app.git", "ssh://git@host:22/a/b", "http://h/a/b"] {
            assert!(clonable(ok), "{ok}");
        }
        for bad in ["", "file:///etc", "/home/me/repo", "--upload-pack=x", "https://a b", "ext::sh -c x"] {
            assert!(!clonable(bad), "{bad}");
        }
    }
}
