use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
    Router::new().route("/v1/me/machines", get(my_machines)).route("/v1/me/machines/assign", post(assign_here))
}

/// One personal machine and its enabled organization assignments.
#[derive(Debug, Serialize)]
pub struct MyMachine {
    pub id: String,
    pub display_name: String,
    pub machine_id: Uuid,
    pub home_org: Option<OrgRef>,
    pub orgs: Vec<OrgRef>,
    pub online: bool,
    pub last_heartbeat_at: DateTime<Utc>,
    /// The projects it has (repo keys, as tasks match them).
    pub repos: Vec<String>,
    /// The agents it can run tasks with.
    pub agents: Vec<Agent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrgRef {
    pub id: Uuid,
    pub name: String,
}

/// The caller's machines, including those with no assignments.
pub async fn list_mine(pool: &PgPool, user: Uuid) -> sqlx::Result<Vec<MyMachine>> {
    let rows = sqlx::query(
        "SELECT m.*, o.name AS home_name, COALESCE((SELECT jsonb_agg(jsonb_build_object('id', o.id, 'name', o.name) ORDER BY o.name)
         FROM workers w JOIN orgs o ON o.id = w.org_id JOIN org_members om ON om.org_id = o.id AND om.user_id = $1
         WHERE w.machine_id = m.id AND w.enabled), '[]'::jsonb) AS orgs
         FROM personal_machines m LEFT JOIN orgs o ON o.id = m.home_org_id WHERE m.owner_id = $1 ORDER BY m.display_name"
    ).bind(user).fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|r| {
            let caps: Capabilities = serde_json::from_value(r.get::<Value, _>("capabilities")).unwrap_or_default();
            let mut repos: Vec<String> = caps.repos.iter().map(|x| x.key().to_string()).collect();
            repos.sort();
            repos.dedup();
            let last: DateTime<Utc> = r.get("last_heartbeat_at");
            MyMachine {
                id: r.get("worker_id"),
                machine_id: r.get("id"),
                home_org: r.get::<Option<Uuid>, _>("home_org_id").map(|id| OrgRef { id, name: r.get("home_name") }),
                display_name: r.get("display_name"),
                orgs: serde_json::from_value(r.get("orgs")).unwrap_or_default(),
                online: db::is_online(last),
                last_heartbeat_at: last,
                repos,
                agents: caps.agents.runnable(),
            }
        })
        .collect())
}

#[derive(Deserialize)]
struct AssignmentReq {
    machine_id: Uuid,
    enabled: bool,
}

/// An assignment changes routing, never machine ownership or authentication.
async fn assign_here(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(req): Json<AssignmentReq>) -> Result<Json<Value>, ApiError> {
    let user = caller.user_id().ok_or_else(|| ApiError(StatusCode::FORBIDDEN, "sign in to assign a machine".into()))?;
    let org = caller.org_id();
    let mut tx = state.pool.begin().await?;
    if sqlx::query("SELECT user_id FROM org_members WHERE org_id = $1 AND user_id = $2 FOR KEY SHARE")
        .bind(org).bind(user).fetch_optional(&mut *tx).await?.is_none() {
        return Err(ApiError(StatusCode::FORBIDDEN, "you are no longer a member of this organization".into()));
    }
    let m = sqlx::query("SELECT * FROM personal_machines WHERE id = $1 AND owner_id = $2 FOR UPDATE")
        .bind(req.machine_id).bind(user).fetch_optional(&mut *tx).await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "machine not found".into()))?;
    let id: String = m.get("worker_id");
    let existing = sqlx::query("SELECT machine_id, enabled FROM workers WHERE org_id = $1 AND id = $2")
        .bind(org).bind(&id).fetch_optional(&mut *tx).await?;
    if existing.as_ref().is_some_and(|w| w.get::<Option<Uuid>, _>("machine_id") != Some(req.machine_id)) {
        return Err(ApiError(StatusCode::CONFLICT, "another machine here uses that id".into()));
    }
    // Assignments are guests: only the machine's home org pays for it.
    sqlx::query("INSERT INTO workers (org_id, id, display_name, token_hash, capabilities, last_heartbeat_at, owner_id, machine_id, enabled)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (org_id, id) DO UPDATE SET enabled = EXCLUDED.enabled")
        .bind(org).bind(&id).bind(m.get::<String, _>("display_name")).bind(m.get::<String, _>("token_hash"))
        .bind(m.get::<Value, _>("capabilities")).bind(m.get::<DateTime<Utc>, _>("last_heartbeat_at"))
        .bind(user).bind(req.machine_id).bind(req.enabled).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO worker_limits (org_id, worker_id, agent, until)
        SELECT $1, $2, l.agent, max(l.until) FROM worker_limits l JOIN workers w ON w.org_id = l.org_id AND w.id = l.worker_id
        WHERE w.machine_id = $3 GROUP BY l.agent
        ON CONFLICT (org_id, worker_id, agent) DO UPDATE SET until = GREATEST(worker_limits.until, EXCLUDED.until)")
        .bind(org).bind(&id).bind(req.machine_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"ok": true})))
}

/// All projections, including disabled ones needed to finish in-flight work.
pub async fn worker_orgs(pool: &PgPool, hash: &str, id: &str) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar("SELECT org_id FROM workers WHERE token_hash = $1 AND id = $2 ORDER BY org_id")
        .bind(hash).bind(id).fetch_all(pool).await
}

async fn my_machines(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<Vec<MyMachine>>, ApiError> {
    let Some(user) = caller.user_id() else { return Ok(Json(vec![])) };
    Ok(Json(list_mine(&state.pool, user).await?))
}

/// Assignments allowed to receive new work and organization data.
pub async fn active_worker_orgs(pool: &PgPool, hash: &str, id: &str) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar("SELECT w.org_id FROM workers w WHERE w.token_hash = $1 AND w.id = $2 AND w.enabled
        AND (w.owner_id IS NULL OR EXISTS (SELECT 1 FROM org_members m WHERE m.org_id = w.org_id AND m.user_id = w.owner_id)) ORDER BY w.org_id")
        .bind(hash).bind(id).fetch_all(pool).await
}
