//! GitHub sign-in (OAuth web flow). The callback creates or refreshes the user,
//! gives a new user a personal org, and sets an httpOnly, SameSite=Lax session
//! cookie. The GitHub access token is used once to read the profile and is
//! never stored or logged.

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use serde_json::Value;

use crate::auth::{cookie, digests_equal, SESSION_COOKIE};
use crate::{tenancy, AppState};

const STATE_COOKIE: &str = "sf_oauth_state";

/// `https://app.sidefleet.xyz`, or `http://<Host header>` when no public URL is configured.
pub fn public_base(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(u) = &state.config.public_url {
        return u.clone();
    }
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("localhost:7373");
    format!("http://{host}")
}

fn set_cookie(state: &AppState, name: &str, value: &str, path: &str, max_age: i64) -> String {
    let secure = if state.config.secure_cookies() { "; Secure" } else { "" };
    format!("{name}={value}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}")
}

fn fail(status: StatusCode, msg: &str) -> Response {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Sign-in failed</title>\
         <body style=\"font:15px system-ui;max-width:32rem;margin:15vh auto;padding:0 1rem\">\
         <h1 style=\"font-size:20px\">GitHub sign-in did not work</h1><p>{}</p><p><a href=\"/\">Back to Sidefleet</a></p>",
        msg.replace('<', "&lt;")
    );
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

pub async fn login(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(gh) = &state.config.github else {
        return fail(StatusCode::NOT_FOUND, "GitHub sign-in is not configured on this control plane.");
    };
    let nonce = tenancy::random_hex(16);
    let redirect = format!("{}/auth/github/callback", public_base(&state, &headers));
    let url = match reqwest::Url::parse_with_params(
        &gh.authorize_url,
        &[("client_id", gh.client_id.as_str()), ("redirect_uri", &redirect), ("scope", "read:user user:email"), ("state", &nonce)],
    ) {
        Ok(u) => u,
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR, "The GitHub authorize URL is invalid."),
    };
    (
        [(header::SET_COOKIE, set_cookie(&state, STATE_COOKIE, &nonce, "/auth", 600))],
        Redirect::to(url.as_str()),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    id: i64,
    login: String,
    email: Option<String>,
}

pub async fn callback(State(state): State<AppState>, headers: HeaderMap, Query(q): Query<Callback>) -> Response {
    let Some(gh) = state.config.github.clone() else {
        return fail(StatusCode::NOT_FOUND, "GitHub sign-in is not configured on this control plane.");
    };
    if let Some(e) = q.error_description {
        return fail(StatusCode::BAD_REQUEST, &format!("GitHub said: {e}"));
    }
    let (Some(code), Some(got)) = (q.code, q.state) else {
        return fail(StatusCode::BAD_REQUEST, "The callback is missing its code or state.");
    };
    // The state must match the cookie set when this browser started the flow (CSRF).
    match cookie(&headers, STATE_COOKIE) {
        Some(expected) if digests_equal(expected, &got) => {}
        _ => return fail(StatusCode::BAD_REQUEST, "The sign-in link expired or was opened in another browser. Start again."),
    }
    let redirect = format!("{}/auth/github/callback", public_base(&state, &headers));
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).user_agent("sidefleet").build();
    let Ok(http) = http else { return fail(StatusCode::INTERNAL_SERVER_ERROR, "HTTP client error.") };

    let token: Value = match http
        .post(&gh.token_url)
        .header(header::ACCEPT, "application/json")
        .json(&serde_json::json!({ "client_id": gh.client_id, "client_secret": gh.client_secret, "code": code, "redirect_uri": redirect }))
        .send()
        .await
    {
        Ok(r) => r.json().await.unwrap_or(Value::Null),
        Err(e) => {
            tracing::warn!(error = %e, "github token exchange failed");
            return fail(StatusCode::BAD_GATEWAY, "Could not reach GitHub.");
        }
    };
    let Some(access) = token.get("access_token").and_then(Value::as_str) else {
        return fail(StatusCode::BAD_GATEWAY, "GitHub did not accept the sign-in code. Start again.");
    };
    let api = gh.api_url.trim_end_matches('/');
    let user: GithubUser = match http.get(format!("{api}/user")).bearer_auth(access).header(header::ACCEPT, "application/vnd.github+json").send().await {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(u) => u,
            Err(_) => return fail(StatusCode::BAD_GATEWAY, "GitHub returned an unexpected profile."),
        },
        _ => return fail(StatusCode::BAD_GATEWAY, "Could not read your GitHub profile."),
    };
    let email = match user.email.clone() {
        Some(e) => Some(e),
        None => primary_email(&http, api, access).await,
    };
    let (u, created) = match tenancy::upsert_github_user(&state.pool, user.id, &user.login, email.as_deref()).await {
        Ok(x) => x,
        Err(e) => {
            tracing::error!(error = %e, "saving user failed");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not save your account.");
        }
    };
    let session = match tenancy::create_session(&state.pool, u.id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "creating session failed");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not start a session.");
        }
    };
    tracing::info!(user = %u.login, new = created, "signed in with GitHub");
    let mut res = Redirect::to("/").into_response();
    let h = res.headers_mut();
    if let Ok(v) = set_cookie(&state, SESSION_COOKIE, &session, "/", tenancy::SESSION_DAYS * 86_400).parse() {
        h.append(header::SET_COOKIE, v);
    }
    if let Ok(v) = set_cookie(&state, STATE_COOKIE, "", "/auth", 0).parse() {
        h.append(header::SET_COOKIE, v);
    }
    res
}

async fn primary_email(http: &reqwest::Client, api: &str, access: &str) -> Option<String> {
    let list: Vec<Value> = http.get(format!("{api}/user/emails")).bearer_auth(access).send().await.ok()?.json().await.ok()?;
    list.iter()
        .find(|e| e.get("primary").and_then(Value::as_bool) == Some(true) && e.get("verified").and_then(Value::as_bool) == Some(true))
        .and_then(|e| e.get("email")?.as_str().map(str::to_owned))
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(c) = cookie(&headers, SESSION_COOKIE) {
        if let Err(e) = tenancy::delete_session(&state.pool, c).await {
            tracing::error!(error = %e, "deleting session failed");
        }
    }
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, set_cookie(&state, SESSION_COOKIE, "", "/", 0))]).into_response()
}
