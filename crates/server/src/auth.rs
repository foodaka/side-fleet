//! Auth for the two audiences: developers (the web page) and workers.
//!
//! Developers present either the single-tenant developer key as a bearer token
//! (acting in the default org), a session cookie from GitHub sign-in (acting
//! in one of their orgs, picked with the `X-Sidefleet-Org` header), or a
//! personal API token (`sfa_…`, acting as its user in its org, limited to what
//! its [`Scope`] allows). Workers
//! present a static token from `SIDEFLEET_WORKER_TOKENS` (default org) or a
//! per-org token from `worker_tokens`. Tokens are compared as SHA-256 digests in
//! constant time; no token or cookie value is ever logged.

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::tenancy::{self, ApiTokenUse, OrgMembership, Scope, User, DEFAULT_ORG};
use crate::AppState;

pub const SESSION_COOKIE: &str = "sf_session";
pub const ORG_HEADER: &str = "x-sidefleet-org";
/// Personal API tokens start with this; anything else after `Bearer` is the developer key.
pub const API_TOKEN_PREFIX: &str = "sfa_";

pub fn sha256_hex(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn digests_equal(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim)
}

pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

fn matches(presented: &str, expected: &str) -> bool {
    let a = Sha256::digest(presented.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    a.ct_eq(&b).into()
}

fn unauthorized(audience: &str) -> Response {
    let msg = format!("missing or invalid {audience} bearer token");
    let mut res = (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")], msg.clone()).into_response();
    res.extensions_mut().insert(crate::request_log::ErrorMessage(msg));
    res
}

fn forbidden(msg: &str) -> Response {
    crate::request_log::error_response(StatusCode::FORBIDDEN, msg)
}

fn server_error(e: sqlx::Error) -> Response {
    tracing::error!(error = %e, "database error during auth");
    crate::request_log::error_response(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

/// Who is calling, before an org is chosen.
#[derive(Clone, Debug)]
pub enum Identity {
    /// The single-tenant developer key.
    DevKey,
    /// A signed-in user (session cookie).
    User(User),
    /// A personal API token, bound to one org.
    ApiToken(ApiTokenUse),
}

/// Resolve the caller from the bearer developer key or the session cookie.
/// `Ok(None)` means nobody is signed in.
#[allow(clippy::result_large_err)]
pub async fn identify(state: &AppState, headers: &HeaderMap) -> Result<Option<Identity>, Response> {
    if let Some(t) = bearer(headers) {
        if t.starts_with(API_TOKEN_PREFIX) {
            return match tenancy::api_token_use(&state.pool, &sha256_hex(t)).await {
                Ok(Some(u)) => Ok(Some(Identity::ApiToken(u))),
                Ok(None) => Err(unauthorized("API")),
                Err(e) => Err(server_error(e)),
            };
        }
        let key = &state.config.dev_key;
        return if !key.is_empty() && matches(t, key) { Ok(Some(Identity::DevKey)) } else { Err(unauthorized("developer")) };
    }
    if let Some(c) = cookie(headers, SESSION_COOKIE) {
        return match tenancy::session_user(&state.pool, c).await {
            Ok(Some(u)) => Ok(Some(Identity::User(u))),
            Ok(None) => Ok(None),
            Err(e) => Err(server_error(e)),
        };
    }
    Ok(None)
}

/// A developer request, resolved to the org it acts in.
#[derive(Clone, Debug)]
pub struct Caller {
    pub org: OrgMembership,
    pub user: Option<User>,
    /// Set for an API token: what it may do. None for the page and the developer key.
    pub scope: Option<Scope>,
    /// Set for an API token: its id, so the runs it starts can be told apart.
    pub token: Option<Uuid>,
}

impl Caller {
    pub fn org_id(&self) -> Uuid {
        self.org.id
    }
    pub fn is_owner(&self) -> bool {
        self.org.role == "owner"
    }
    pub fn user_id(&self) -> Option<Uuid> {
        self.user.as_ref().map(|u| u.id)
    }
    /// Whether runs may use full access (`bypassPermissions`) and tool calls may be approved.
    pub fn is_full(&self) -> bool {
        self.scope.is_none_or(|s| s == Scope::Full)
    }
}

/// Whether an API token with `scope` may call `method path`. Tokens never
/// manage orgs, members, tokens or machines, and never see `/v1/setup` (it
/// can carry a worker token): those stay with the page and the developer key.
pub fn scope_allows(scope: Scope, method: &Method, path: &str) -> bool {
    let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
    let need = match (method, seg.as_slice()) {
        (&Method::GET, ["v1", "runs"] | ["v1", "runs", _] | ["v1", "runs", _, "thread" | "events" | "approvals" | "summary"]) => Scope::Read,
        (&Method::GET, ["v1", "images", _] | ["v1", "workers"] | ["v1", "repos"]) => Scope::Read,
        (&Method::POST, ["v1", "runs"] | ["v1", "runs", _, "messages" | "stop" | "restore" | "fork" | "pr" | "archive"]) => Scope::Tasks,
        (&Method::POST, ["v1", "runs", _, "approvals", _, "approve" | "deny"]) => Scope::Full,
        // Each MCP tool calls the routes above as the token, so the token's scope still decides.
        (&Method::GET | &Method::POST, ["mcp"]) => Scope::Read,
        _ => return false,
    };
    scope >= need
}

/// Developer API: developer key or session cookie; puts a [`Caller`] in the
/// request extensions. Users pick the org with `X-Sidefleet-Org` (id or slug),
/// defaulting to their first (personal) org.
pub async fn require_dev(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let identity = match identify(&state, req.headers()).await {
        Ok(Some(i)) => i,
        Ok(None) => return unauthorized("developer"),
        Err(r) => return r,
    };
    let caller = match identity {
        Identity::DevKey => match tenancy::default_org(&state.pool).await {
            Ok(org) => Caller { org, user: None, scope: None, token: None },
            Err(e) => return server_error(e),
        },
        Identity::ApiToken(t) => {
            let wanted = req.headers().get(ORG_HEADER).and_then(|v| v.to_str().ok()).map(str::trim).filter(|s| !s.is_empty());
            if wanted.is_some_and(|w| w != t.org.id.to_string() && w != t.org.slug) {
                return forbidden("this API token belongs to another org");
            }
            if !scope_allows(t.scope, req.method(), req.uri().path()) {
                return forbidden(&format!("this API token's scope ({}) does not allow that", t.scope.as_str()));
            }
            Caller { org: t.org, user: t.user, scope: Some(t.scope), token: Some(t.id) }
        }
        Identity::User(user) => {
            let orgs = match tenancy::memberships(&state.pool, user.id).await {
                Ok(o) => o,
                Err(e) => return server_error(e),
            };
            let wanted = req.headers().get(ORG_HEADER).and_then(|v| v.to_str().ok()).map(str::trim).filter(|s| !s.is_empty());
            let org = match wanted {
                Some(w) => match orgs.into_iter().find(|o| o.id.to_string() == w || o.slug == w) {
                    Some(o) => o,
                    None => return forbidden("you are not a member of that org"),
                },
                None => match orgs.into_iter().next() {
                    Some(o) => o,
                    None => return forbidden("you are not a member of any org"),
                },
            };
            Caller { org, user: Some(user), scope: None, token: None }
        }
    };
    crate::request_log::record_caller(caller.org_id(), caller.user.as_ref().map(|u| u.login.as_str()));
    req.extensions_mut().insert(caller);
    next.run(req).await
}

/// What a worker request is allowed to act on.
#[derive(Clone, Debug)]
pub struct WorkerAuth {
    /// Digest of the presented token; a worker stays bound to the token it registered with.
    pub token_hash: String,
    pub org: Uuid,
}

/// Worker API: a static `SIDEFLEET_WORKER_TOKENS` entry (default org) or a live
/// per-org token. A revoked token gets 401 on its next call.
pub async fn require_worker(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(t) = bearer(req.headers()).map(str::to_owned) else {
        return unauthorized("worker");
    };
    let hash = sha256_hex(&t);
    let org = if state.config.worker_tokens.iter().any(|w| matches(&t, w)) {
        DEFAULT_ORG
    } else {
        match tenancy::token_org(&state.pool, &hash).await {
            Ok(Some(org)) => org,
            Ok(None) => return unauthorized("worker"),
            Err(e) => return server_error(e),
        }
    };
    crate::request_log::record_caller(org, None);
    req.extensions_mut().insert(WorkerAuth { token_hash: hash, org });
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cookies() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "a=1; sf_session=abc ; b=2".parse().unwrap());
        assert_eq!(cookie(&h, SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie(&h, "c"), None);
    }

    #[test]
    fn scopes_allow_what_they_say() {
        let ok = |s, m: Method, p: &str| scope_allows(s, &m, p);
        let run = "/v1/runs/0b0b8a5e-0000-0000-0000-000000000000";
        assert!(ok(Scope::Read, Method::GET, "/v1/runs"));
        assert!(ok(Scope::Read, Method::GET, &format!("{run}/events")));
        assert!(ok(Scope::Read, Method::GET, "/v1/workers"));
        assert!(!ok(Scope::Read, Method::POST, "/v1/runs"));
        assert!(ok(Scope::Tasks, Method::POST, "/v1/runs"));
        assert!(ok(Scope::Tasks, Method::POST, &format!("{run}/messages")));
        assert!(!ok(Scope::Tasks, Method::POST, &format!("{run}/approvals/x/approve")));
        assert!(ok(Scope::Full, Method::POST, &format!("{run}/approvals/x/deny")));
        for (m, p) in [
            (Method::GET, "/v1/setup"),
            (Method::GET, "/v1/api-tokens"),
            (Method::POST, "/v1/api-tokens"),
            (Method::POST, "/v1/orgs/x/tokens"),
            (Method::GET, "/v1/enrollments"),
            (Method::DELETE, "/v1/workers/mac"),
            (Method::POST, "/v1/workers/mac/update"),
            (Method::PUT, "/v1/repos"),
        ] {
            assert!(!ok(Scope::Full, m.clone(), p), "{m} {p}");
        }
    }
}
