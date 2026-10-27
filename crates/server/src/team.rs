//! Bringing people into an org: invite links, and removing or leaving. A link
//! carries a random code; only its SHA-256 is stored, and it is shown once.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::auth::{self, sha256_hex, Caller, Identity};
use crate::tenancy::{self, OrgMembership, DEFAULT_ORG};
use crate::AppState;

/// Letters and digits that can't be mistaken for each other, as in enrollment codes.
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const MAX_EXPIRY_DAYS: i64 = 30;

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

fn forbidden(m: &str) -> ApiError {
    ApiError(StatusCode::FORBIDDEN, m.into())
}

/// `K7QD-M2XP-9RTA`: 12 characters in three groups.
fn new_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let chars: Vec<char> = (0..12).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect();
    chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

/// The code as typed: upper case, dashes optional.
fn normalise(code: &str) -> String {
    let raw: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    raw.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

#[derive(Debug, Clone, Serialize)]
pub struct Invite {
    pub id: Uuid,
    pub role: String,
    pub created_by_login: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub max_uses: Option<i32>,
    pub uses: i32,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const INVITE_COLS: &str = "i.id, i.role, u.login AS created_by_login, i.expires_at, i.max_uses, i.uses, i.revoked_at, i.created_at";

fn row_to_invite(r: &sqlx::postgres::PgRow) -> Invite {
    Invite {
        id: r.get("id"),
        role: r.get("role"),
        created_by_login: r.get("created_by_login"),
        expires_at: r.get("expires_at"),
        max_uses: r.get("max_uses"),
        uses: r.get("uses"),
        revoked_at: r.get("revoked_at"),
        created_at: r.get("created_at"),
    }
}

/// Why an invite can't be used, if it can't.
fn unusable(i: &Invite) -> Option<&'static str> {
    if i.revoked_at.is_some() {
        Some("revoked")
    } else if i.expires_at <= Utc::now() {
        Some("expired")
    } else if i.max_uses.is_some_and(|m| i.uses >= m) {
        Some("used_up")
    } else {
        None
    }
}

pub async fn create_invite(pool: &PgPool, org: Uuid, by: Option<Uuid>, role: &str, days: i64, max_uses: Option<i32>) -> sqlx::Result<(Invite, String)> {
    let code = new_code();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO org_invites (id, org_id, code_hash, role, created_by, max_uses, expires_at) VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(days => $7))")
        .bind(id)
        .bind(org)
        .bind(sha256_hex(&code))
        .bind(role)
        .bind(by)
        .bind(max_uses)
        .bind(days as i32)
        .execute(pool)
        .await?;
    let row = sqlx::query(&format!("SELECT {INVITE_COLS} FROM org_invites i LEFT JOIN users u ON u.id = i.created_by WHERE i.id = $1"))
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok((row_to_invite(&row), code))
}

/// The org's invites: usable ones first, then the rest of the last 30 days.
pub async fn list_invites(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<Invite>> {
    let rows = sqlx::query(&format!(
        "SELECT {INVITE_COLS} FROM org_invites i LEFT JOIN users u ON u.id = i.created_by
         WHERE i.org_id = $1 AND i.created_at > now() - interval '30 days' ORDER BY i.created_at DESC"
    ))
    .bind(org)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<Invite> = rows.iter().map(row_to_invite).collect();
    out.sort_by_key(|i| unusable(i).is_some());
    Ok(out)
}

pub async fn revoke_invite(pool: &PgPool, org: Uuid, id: Uuid) -> sqlx::Result<bool> {
    let r = sqlx::query("UPDATE org_invites SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1 AND org_id = $2").bind(id).bind(org).execute(pool).await?;
    Ok(r.rows_affected() == 1)
}

pub enum Accept {
    Joined(OrgMembership),
    Unusable(&'static str),
    NotFound,
}

/// Join the org an invite is for. Joining an org one already belongs to is fine and uses nothing up.
pub async fn accept(pool: &PgPool, code: &str, user: Uuid) -> sqlx::Result<Accept> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!(
        "SELECT {INVITE_COLS}, i.org_id FROM org_invites i LEFT JOIN users u ON u.id = i.created_by WHERE i.code_hash = $1 FOR UPDATE OF i"
    ))
    .bind(sha256_hex(&normalise(code)))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else { return Ok(Accept::NotFound) };
    let invite = row_to_invite(&row);
    let org: Uuid = row.get("org_id");
    let member = sqlx::query("SELECT 1 AS x FROM org_members WHERE org_id = $1 AND user_id = $2").bind(org).bind(user).fetch_optional(&mut *tx).await?.is_some();
    if !member {
        if let Some(why) = unusable(&invite) {
            return Ok(Accept::Unusable(why));
        }
        sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1, $2, $3)").bind(org).bind(user).bind(&invite.role).execute(&mut *tx).await?;
        sqlx::query("UPDATE org_invites SET uses = uses + 1 WHERE id = $1").bind(invite.id).execute(&mut *tx).await?;
        // Start with an empty inbox rather than everything the team ever finished.
        sqlx::query("INSERT INTO inbox_cleared (viewer_id, org_id, cleared_at) VALUES ($1, $2, now()) ON CONFLICT DO NOTHING")
            .bind(user)
            .bind(org)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    match tenancy::memberships(pool, user).await?.into_iter().find(|o| o.id == org) {
        Some(m) => Ok(Accept::Joined(m)),
        None => Ok(Accept::NotFound),
    }
}

pub enum Remove {
    Removed,
    NotMember,
    LastOwner,
}

/// Take someone out of an org: their machines' tokens there are revoked (the
/// machines get 401 at their next call) and their queued runs there stop.
pub async fn remove_member(pool: &PgPool, org: Uuid, user: Uuid) -> sqlx::Result<Remove> {
    let mut tx = pool.begin().await?;
    let role: Option<String> = sqlx::query("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2 FOR UPDATE")
        .bind(org)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?
        .map(|r| r.get("role"));
    let Some(role) = role else { return Ok(Remove::NotMember) };
    if role == "owner" {
        let owners: i64 = sqlx::query("SELECT count(*) AS n FROM org_members WHERE org_id = $1 AND role = 'owner'").bind(org).fetch_one(&mut *tx).await?.get("n");
        if owners <= 1 {
            return Ok(Remove::LastOwner);
        }
    }
    sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2").bind(org).bind(user).execute(&mut *tx).await?;
    sqlx::query("UPDATE worker_tokens SET revoked_at = COALESCE(revoked_at, now()) WHERE org_id = $1 AND owner_id = $2").bind(org).bind(user).execute(&mut *tx).await?;
    sqlx::query("UPDATE api_tokens SET revoked_at = COALESCE(revoked_at, now()) WHERE org_id = $1 AND user_id = $2").bind(org).bind(user).execute(&mut *tx).await?;
    // Invite links they made would otherwise let them (or whoever has one) back in.
    sqlx::query("UPDATE org_invites SET revoked_at = COALESCE(revoked_at, now()) WHERE org_id = $1 AND created_by = $2").bind(org).bind(user).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE runs SET status = 'stopped', finished_at = now(), result = '{\"error\": \"its owner left the org\"}'::jsonb
         WHERE org_id = $1 AND created_by = $2 AND status = 'queued'",
    )
    .bind(org)
    .bind(user)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE runs SET stop_requested = true WHERE org_id = $1 AND created_by = $2 AND status = 'running'").bind(org).bind(user).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Remove::Removed)
}

// ---- HTTP ----

/// Developer routes, behind developer auth.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/orgs/{org}/invites", get(get_invites).post(post_invite))
        .route("/v1/orgs/{org}/invites/{id}", delete(delete_invite))
        .route("/v1/orgs/{org}/members/{user}", delete(delete_member))
}

/// Public routes: reading an invite needs no sign-in; accepting needs a GitHub session.
pub fn public_router() -> Router<AppState> {
    Router::new().route("/v1/invites/{code}", get(get_invite)).route("/v1/invites/{code}/accept", post(post_accept))
}

/// The caller's membership in the org named in the path, which must be their current org.
fn current_org(caller: &Caller, org: &str) -> Result<OrgMembership, ApiError> {
    if caller.org.id.to_string() == org || caller.org.slug == org {
        Ok(caller.org.clone())
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "org not found".into()))
    }
}

fn owner_of(caller: &Caller, org: &str) -> Result<OrgMembership, ApiError> {
    let m = current_org(caller, org)?;
    if m.role != "owner" {
        return Err(forbidden("only an owner of this org can do that"));
    }
    if caller.user.is_none() || m.id == DEFAULT_ORG {
        return Err(forbidden("invites are for orgs people sign in to with GitHub"));
    }
    Ok(m)
}

#[derive(Deserialize, Default)]
struct NewInviteReq {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    expires_in_days: Option<i64>,
    #[serde(default)]
    max_uses: Option<i32>,
}

async fn post_invite(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(org): Path<String>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let org = owner_of(&caller, &org)?;
    let req: NewInviteReq = if body.iter().all(u8::is_ascii_whitespace) {
        NewInviteReq::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("invalid body: {e}")))?
    };
    let role = req.role.as_deref().unwrap_or("member");
    if !["owner", "member"].contains(&role) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "role must be owner or member".into()));
    }
    let days = req.expires_in_days.unwrap_or(7);
    if !(1..=MAX_EXPIRY_DAYS).contains(&days) {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("expires_in_days must be 1 to {MAX_EXPIRY_DAYS}")));
    }
    if req.max_uses.is_some_and(|m| m < 1) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "max_uses must be at least 1".into()));
    }
    let (invite, code) = create_invite(&state.pool, org.id, caller.user_id(), role, days, req.max_uses).await?;
    tracing::info!(org = %org.id, invite = %invite.id, "invite created");
    Ok((StatusCode::CREATED, Json(json!({ "invite": invite, "code": code }))))
}

async fn get_invites(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(org): Path<String>) -> Result<Json<Vec<Invite>>, ApiError> {
    let org = owner_of(&caller, &org)?;
    Ok(Json(list_invites(&state.pool, org.id).await?))
}

async fn delete_invite(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path((org, id)): Path<(String, Uuid)>) -> Result<StatusCode, ApiError> {
    let org = owner_of(&caller, &org)?;
    if revoke_invite(&state.pool, org.id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "invite not found".into()))
    }
}

/// Owners remove anyone; anyone may leave (except the last owner, and nobody leaves their personal org's only seat).
async fn delete_member(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path((org, user)): Path<(String, Uuid)>) -> Result<StatusCode, ApiError> {
    let m = current_org(&caller, &org)?;
    let me = caller.user_id();
    if me != Some(user) && m.role != "owner" {
        return Err(forbidden("only an owner of this org can remove people"));
    }
    match remove_member(&state.pool, m.id, user).await? {
        Remove::Removed => {
            tracing::info!(org = %m.id, user = %user, by = ?me, "member removed");
            state.claims.signal(m.id);
            Ok(StatusCode::NO_CONTENT)
        }
        Remove::NotMember => Err(ApiError(StatusCode::NOT_FOUND, "not a member of this org".into())),
        Remove::LastOwner => Err(ApiError(StatusCode::CONFLICT, "the org needs at least one owner; make someone else an owner first".into())),
    }
}

async fn get_invite(State(state): State<AppState>, Path(code): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let row = sqlx::query(&format!(
        "SELECT {INVITE_COLS}, o.name AS org_name FROM org_invites i JOIN orgs o ON o.id = i.org_id LEFT JOIN users u ON u.id = i.created_by
         WHERE i.code_hash = $1"
    ))
    .bind(sha256_hex(&normalise(&code)))
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Ok(Json(json!({ "valid": false, "reason": "not_found" })));
    };
    let invite = row_to_invite(&row);
    let why = unusable(&invite);
    Ok(Json(json!({
        "org_name": row.get::<String, _>("org_name"),
        "inviter_login": invite.created_by_login,
        "role": invite.role,
        "valid": why.is_none(),
        "reason": why,
    })))
}

async fn post_accept(State(state): State<AppState>, headers: HeaderMap, Path(code): Path<String>) -> Result<Response, ApiError> {
    let user = match auth::identify(&state, &headers).await {
        Ok(Some(Identity::User(u))) => u,
        Ok(_) => return Err(ApiError(StatusCode::UNAUTHORIZED, "sign in with GitHub to join".into())),
        Err(r) => return Ok(r),
    };
    match accept(&state.pool, &code, user.id).await? {
        Accept::Joined(org) => {
            tracing::info!(org = %org.id, user = %user.login, "joined with an invite");
            Ok(Json(json!({ "org": org })).into_response())
        }
        Accept::Unusable(why) => Err(ApiError(StatusCode::GONE, format!("this invite is {}", why.replace('_', " ")))),
        Accept::NotFound => Err(ApiError(StatusCode::NOT_FOUND, "no invite with that code".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn codes_are_three_groups_and_normalise_back() {
        let c = new_code();
        assert_eq!(c.len(), 14);
        assert_eq!(c.split('-').count(), 3);
        assert_eq!(normalise(&c.to_lowercase().replace('-', "")), c);
        assert_eq!(normalise(" k7qd-m2xp 9rta "), "K7QD-M2XP-9RTA");
    }

    #[test]
    fn expired_revoked_and_used_up_invites_are_unusable() {
        let ok = Invite {
            id: Uuid::nil(),
            role: "member".into(),
            created_by_login: None,
            expires_at: Utc::now() + Duration::days(1),
            max_uses: Some(2),
            uses: 1,
            revoked_at: None,
            created_at: Utc::now(),
        };
        assert_eq!(unusable(&ok), None);
        assert_eq!(unusable(&Invite { uses: 2, ..ok.clone() }), Some("used_up"));
        assert_eq!(unusable(&Invite { expires_at: Utc::now() - Duration::seconds(1), ..ok.clone() }), Some("expired"));
        assert_eq!(unusable(&Invite { revoked_at: Some(Utc::now()), ..ok }), Some("revoked"));
    }
}
