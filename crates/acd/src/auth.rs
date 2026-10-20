//! Bearer-token auth for the two audiences: developers (the web page) and workers.
//! Tokens are compared in constant time against SHA-256 digests; no token value
//! is ever logged.

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::AppState;

pub fn sha256_hex(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn bearer(req: &Request) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn matches(presented: &str, expected: &str) -> bool {
    let a = Sha256::digest(presented.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    a.ct_eq(&b).into()
}

fn unauthorized(audience: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        format!("missing or invalid {audience} bearer token"),
    )
        .into_response()
}

/// Developer API: `Authorization: Bearer <ACD_DEV_KEY>`.
pub async fn require_dev(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match bearer(&req) {
        Some(t) if matches(t, &state.config.dev_key) => next.run(req).await,
        _ => unauthorized("developer"),
    }
}

/// Worker API: `Authorization: Bearer <one of ACD_WORKER_TOKENS>`.
/// Stores the token hash in request extensions so `register` can persist it.
pub async fn require_worker(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(t) = bearer(&req).map(str::to_owned) else {
        return unauthorized("worker");
    };
    let ok = state.config.worker_tokens.iter().any(|w| matches(&t, w));
    if !ok {
        return unauthorized("worker");
    }
    req.extensions_mut().insert(WorkerTokenHash(sha256_hex(&t)));
    next.run(req).await
}

#[derive(Clone, Debug)]
pub struct WorkerTokenHash(pub String);
