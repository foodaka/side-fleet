//! Your machines. Each connects to one org (its home, which counts it against that org's
//! plan) and serves its owner in every org they belong to, so it is listed once, here, with
//! its home.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sidefleet_core::{Agent, Capabilities};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::auth::Caller;
use crate::db;
use crate::AppState;

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

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/me/machines", get(my_machines))
}

/// One of your machines, and its home org.
#[derive(Debug, Serialize)]
pub struct MyMachine {
    pub id: String,
    pub display_name: String,
    pub org: OrgRef,
    pub online: bool,
    pub last_heartbeat_at: DateTime<Utc>,
    /// The projects it has (repo keys, as tasks match them).
    pub repos: Vec<String>,
    /// The agents it can run tasks with.
    pub agents: Vec<Agent>,
}

#[derive(Debug, Serialize)]
pub struct OrgRef {
    pub id: Uuid,
    pub name: String,
}

/// The caller's machines, each once, with its home org.
pub async fn list_mine(pool: &PgPool, user: Uuid) -> sqlx::Result<Vec<MyMachine>> {
    let rows = sqlx::query(
        "SELECT w.id, w.display_name, w.capabilities, w.last_heartbeat_at, o.id AS org_id, o.name AS org_name
         FROM workers w JOIN orgs o ON o.id = w.org_id
         JOIN worker_tokens t ON t.token_hash = w.token_hash AND t.org_id = w.org_id AND t.revoked_at IS NULL
         WHERE w.owner_id = $1
         ORDER BY o.name, w.display_name",
    )
    .bind(user)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let caps: Capabilities = serde_json::from_value(r.get::<Value, _>("capabilities")).unwrap_or_default();
            let mut repos: Vec<String> = caps.repos.iter().map(|x| x.key().to_string()).collect();
            repos.sort();
            repos.dedup();
            let last: DateTime<Utc> = r.get("last_heartbeat_at");
            MyMachine {
                id: r.get("id"),
                display_name: r.get("display_name"),
                org: OrgRef { id: r.get("org_id"), name: r.get("org_name") },
                online: db::is_online(last),
                last_heartbeat_at: last,
                repos,
                agents: caps.agents.runnable(),
            }
        })
        .collect())
}

async fn my_machines(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<Vec<MyMachine>>, ApiError> {
    let Some(user) = caller.user_id() else { return Ok(Json(vec![])) };
    Ok(Json(list_mine(&state.pool, user).await?))
}
