//! Your machines in your other orgs, and moving one into the org the page acts in.
//!
//! A machine belongs to the org of the token it connected with. Moving it points
//! that token at the new org and carries the machine's row over, so its next
//! heartbeat lands in the new org with nothing to reconnect. Only the machine's
//! owner can move it, only between orgs they are a member of, and only when
//! nothing is running on it.

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
    Router::new().route("/v1/me/machines", get(elsewhere)).route("/v1/me/machines/move", post(move_here))
}

/// One of your machines in another of your orgs.
#[derive(Debug, Serialize)]
pub struct Elsewhere {
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

/// The caller's machines in every org they belong to except `here`.
pub async fn list_elsewhere(pool: &PgPool, user: Uuid, here: Uuid) -> sqlx::Result<Vec<Elsewhere>> {
    let rows = sqlx::query(
        "SELECT w.id, w.display_name, w.capabilities, w.last_heartbeat_at, o.id AS org_id, o.name AS org_name
         FROM workers w JOIN orgs o ON o.id = w.org_id JOIN org_members m ON m.org_id = w.org_id AND m.user_id = $1
         WHERE w.owner_id = $1 AND w.org_id <> $2
         ORDER BY w.last_heartbeat_at DESC",
    )
    .bind(user)
    .bind(here)
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
            Elsewhere {
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

pub enum Move {
    Moved { display_name: String },
    /// Not there, or not the caller's.
    NotFound,
    /// A task is running on it.
    Busy,
    /// It connected with the control plane's static token (or a revoked one), which can't change orgs.
    FixedToken,
    /// Other machines connected with the same token; moving it would move them too.
    SharedToken(i64),
    /// Someone else's machine already has this id in the target org.
    Taken,
}

/// Move `user`'s machine `id` from org `from` to org `to`. The caller checks that
/// `user` is a member of both.
pub async fn move_machine(pool: &PgPool, user: Uuid, from: Uuid, to: Uuid, id: &str) -> sqlx::Result<Move> {
    let mut tx = pool.begin().await?;
    let Some(w) = sqlx::query("SELECT display_name, token_hash, owner_id FROM workers WHERE org_id = $1 AND id = $2 FOR UPDATE")
        .bind(from)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(Move::NotFound);
    };
    if w.get::<Option<Uuid>, _>("owner_id") != Some(user) {
        return Ok(Move::NotFound);
    }
    let hash: String = w.get("token_hash");
    let Some(token) = sqlx::query("SELECT id FROM worker_tokens WHERE token_hash = $1 AND org_id = $2 AND revoked_at IS NULL FOR UPDATE")
        .bind(&hash)
        .bind(from)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(Move::FixedToken);
    };
    let others: i64 = sqlx::query("SELECT count(*) AS n FROM workers WHERE org_id = $1 AND token_hash = $2 AND id <> $3")
        .bind(from)
        .bind(&hash)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?
        .get("n");
    if others > 0 {
        return Ok(Move::SharedToken(others));
    }
    let there = sqlx::query("SELECT owner_id FROM workers WHERE org_id = $1 AND id = $2").bind(to).bind(id).fetch_optional(&mut *tx).await?;
    if there.is_some_and(|r| r.get::<Option<Uuid>, _>("owner_id") != Some(user)) {
        return Ok(Move::Taken);
    }
    if sqlx::query("SELECT 1 AS x FROM runs WHERE org_id = $1 AND worker_id = $2 AND status = 'running'")
        .bind(from)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .is_some()
    {
        return Ok(Move::Busy);
    }
    // Past runs in the old org keep their history without a machine, as when it is removed.
    sqlx::query("UPDATE runs SET worker_id = NULL WHERE org_id = $1 AND worker_id = $2").bind(from).bind(id).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO workers (org_id, id, display_name, token_hash, capabilities, last_heartbeat_at, slots_parallel, slots_active, owner_id)
         SELECT $2, id, display_name, token_hash, capabilities, last_heartbeat_at, slots_parallel, slots_active, owner_id
         FROM workers WHERE org_id = $1 AND id = $3
         ON CONFLICT (org_id, id) DO UPDATE SET display_name = EXCLUDED.display_name, token_hash = EXCLUDED.token_hash,
            capabilities = EXCLUDED.capabilities, last_heartbeat_at = EXCLUDED.last_heartbeat_at, update_requested = false,
            slots_parallel = EXCLUDED.slots_parallel, slots_active = EXCLUDED.slots_active",
    )
    .bind(from)
    .bind(to)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM workers WHERE org_id = $1 AND id = $2").bind(from).bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE worker_tokens SET org_id = $2 WHERE id = $1").bind(token.get::<Uuid, _>("id")).bind(to).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Move::Moved { display_name: w.get("display_name") })
}

async fn elsewhere(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<Vec<Elsewhere>>, ApiError> {
    let Some(user) = caller.user_id() else { return Ok(Json(vec![])) };
    Ok(Json(list_elsewhere(&state.pool, user, caller.org_id()).await?))
}

#[derive(Deserialize)]
struct MoveReq {
    /// The org the machine is in now.
    from: Uuid,
    id: String,
}

/// Bring one of your machines from another of your orgs into this one.
async fn move_here(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(req): Json<MoveReq>) -> Result<Json<Value>, ApiError> {
    let Some(user) = caller.user_id() else {
        return Err(ApiError(StatusCode::FORBIDDEN, "sign in to move a machine".into()));
    };
    let to = caller.org_id();
    if req.from == to {
        return Err(ApiError(StatusCode::BAD_REQUEST, "that machine is already in this org".into()));
    }
    // Only between orgs you belong to: you can't pull a machine out of an org you left.
    if !crate::tenancy::memberships(&state.pool, user).await?.iter().any(|o| o.id == req.from) {
        return Err(ApiError(StatusCode::NOT_FOUND, "no machine of yours by that name there".into()));
    }
    if let Some(l) = caller.org.limits {
        if db::count_workers(&state.pool, to).await? >= l.machines {
            return Err(ApiError(
                StatusCode::PAYMENT_REQUIRED,
                format!("{} machines are allowed on the {} plan. Upgrade the org's plan or free one up.", l.machines, caller.org.plan),
            ));
        }
    }
    match move_machine(&state.pool, user, req.from, to, &req.id).await? {
        Move::Moved { display_name } => {
            tracing::info!(worker = %req.id, from = %req.from, to = %to, "machine moved");
            Ok(Json(json!({ "id": req.id, "display_name": display_name })))
        }
        Move::NotFound => Err(ApiError(StatusCode::NOT_FOUND, "no machine of yours by that name there".into())),
        Move::Busy => Err(ApiError(StatusCode::CONFLICT, "a task is running on that machine; move it when it finishes".into())),
        Move::FixedToken => Err(ApiError(
            StatusCode::CONFLICT,
            "that machine connected with this control plane's built-in token, which can't change orgs; run `sidefleet connect` on it again from this org".into(),
        )),
        Move::SharedToken(n) => Err(ApiError(
            StatusCode::CONFLICT,
            format!("that machine shares its token with {n} other machine{}; run `sidefleet connect` on it again from this org", if n == 1 { "" } else { "s" }),
        )),
        Move::Taken => Err(ApiError(StatusCode::CONFLICT, "a teammate's machine here already uses that machine id".into())),
    }
}
