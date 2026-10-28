//! Your preferences from the Settings page, kept with your account so every browser you
//! sign in on starts with the same task defaults. The page decides what they mean; this
//! only stores one JSON object per person. The developer key has no person, so its page
//! keeps them in the browser.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde_json::{json, Value};

use crate::auth::Caller;
use crate::AppState;

/// Plenty for a page of settings; stops the column being used as storage.
const MAX_BYTES: usize = 16 * 1024;

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
    Router::new().route("/v1/me/prefs", get(get_prefs).put(put_prefs))
}

fn no_user() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "preferences are kept per person; sign in to keep them with your account".into())
}

/// Your preferences: `{}` if you never changed one.
async fn get_prefs(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<Value>, ApiError> {
    let user = caller.user_id().ok_or_else(no_user)?;
    let prefs: Option<Value> = sqlx::query_scalar("SELECT prefs FROM users WHERE id = $1").bind(user).fetch_optional(&state.pool).await?.flatten();
    Ok(Json(prefs.unwrap_or_else(|| json!({}))))
}

/// Replace your preferences with this object.
async fn put_prefs(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let user = caller.user_id().ok_or_else(no_user)?;
    if !body.is_object() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "preferences must be a JSON object".into()));
    }
    if body.to_string().len() > MAX_BYTES {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, format!("preferences are limited to {} KB", MAX_BYTES / 1024)));
    }
    sqlx::query("UPDATE users SET prefs = $2 WHERE id = $1").bind(user).bind(&body).execute(&state.pool).await?;
    Ok(Json(body))
}
