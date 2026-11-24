//! Project icons: the emoji, coloured letter or small picture shown next to a repo's group
//! in the task list. Anyone in the org can set one and everyone in it sees it. A repo with
//! no icon set gets a default the page picks (a letter tile, or the GitHub owner's avatar).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use crate::auth::Caller;
use crate::AppState;

/// A 64px PNG is a few KB; this leaves room for a busy JPEG and stops the table being used as storage.
pub const MAX_IMAGE_BYTES: usize = 48 * 1024;
const MAX_REPO_BYTES: usize = 300;
const MAX_EMOJI_BYTES: usize = 64;
const IMAGE_PREFIXES: [&str; 3] = ["data:image/png;base64,", "data:image/jpeg;base64,", "data:image/webp;base64,"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Icon {
    /// `emoji`, `letter` (value: the tile colour, `#rrggbb`) or `image` (value: a data: URL).
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectIcon {
    pub repo: String,
    #[serde(flatten)]
    pub icon: Icon,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct SetIconRequest {
    pub repo: String,
    /// None (or null) goes back to the default.
    #[serde(default)]
    pub icon: Option<Icon>,
}

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

fn bad(msg: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/project-icons", get(list_icons).put(set_icon))
}

/// Why `icon` can't be stored, if it can't.
pub fn check(icon: &Icon) -> Result<(), String> {
    let v = icon.value.as_str();
    match icon.kind.as_str() {
        "emoji" => {
            if v.trim().is_empty() || v.len() > MAX_EMOJI_BYTES || v.chars().any(|c| c.is_ascii_alphanumeric() || c.is_control()) {
                return Err("an emoji icon is one emoji".into());
            }
        }
        "letter" => {
            let hex = v.strip_prefix('#').unwrap_or("");
            if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("a letter icon's value is its colour, like #7c5cff".into());
            }
        }
        "image" => {
            let Some(data) = IMAGE_PREFIXES.iter().find_map(|p| v.strip_prefix(p)) else {
                return Err("an image icon is a PNG, JPEG or WebP data: URL".into());
            };
            if v.len() > MAX_IMAGE_BYTES {
                return Err(format!("an image icon is limited to {} KB; shrink it first", MAX_IMAGE_BYTES / 1024));
            }
            if data.is_empty() || !data.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
                return Err("the image isn't valid base64".into());
            }
        }
        _ => return Err("kind is emoji, letter or image".into()),
    }
    Ok(())
}

/// Every icon set in this org.
async fn list_icons(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Result<Json<Vec<ProjectIcon>>, ApiError> {
    let rows = sqlx::query("SELECT repo, kind, value, updated_at FROM project_icons WHERE org_id = $1 ORDER BY repo")
        .bind(caller.org_id())
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(
        rows.iter()
            .map(|r| ProjectIcon { repo: r.get("repo"), icon: Icon { kind: r.get("kind"), value: r.get("value") }, updated_at: r.get("updated_at") })
            .collect(),
    ))
}

/// Set a repo's icon, or go back to the default with `icon: null`. Answers with every icon, as GET does.
async fn set_icon(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(req): Json<SetIconRequest>) -> Result<Json<Vec<ProjectIcon>>, ApiError> {
    let repo = req.repo.trim();
    if repo.is_empty() || repo.len() > MAX_REPO_BYTES {
        return Err(bad("repo is required"));
    }
    match &req.icon {
        Some(icon) => {
            check(icon).map_err(|m| bad(&m))?;
            sqlx::query(
                "INSERT INTO project_icons (org_id, repo, kind, value, updated_by) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (org_id, repo) DO UPDATE SET kind = $3, value = $4, updated_by = $5, updated_at = now()",
            )
            .bind(caller.org_id())
            .bind(repo)
            .bind(&icon.kind)
            .bind(&icon.value)
            .bind(caller.user_id())
            .execute(&state.pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM project_icons WHERE org_id = $1 AND repo = $2").bind(caller.org_id()).bind(repo).execute(&state.pool).await?;
        }
    }
    tracing::info!(repo, kind = req.icon.as_ref().map(|i| i.kind.as_str()).unwrap_or("default"), "project icon set");
    list_icons(State(state), Extension(caller)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(kind: &str, value: &str) -> Icon {
        Icon { kind: kind.into(), value: value.into() }
    }

    #[test]
    fn only_well_formed_icons_are_kept() {
        assert!(check(&icon("emoji", "🪐")).is_ok());
        assert!(check(&icon("emoji", "⚡️")).is_ok());
        assert!(check(&icon("emoji", "👩🏽‍💻")).is_ok());
        assert!(check(&icon("emoji", "")).is_err());
        assert!(check(&icon("emoji", "hello")).is_err());
        assert!(check(&icon("letter", "#7c5cff")).is_ok());
        assert!(check(&icon("letter", "7c5cff")).is_err());
        assert!(check(&icon("letter", "#7c5cfz")).is_err());
        assert!(check(&icon("image", "data:image/png;base64,iVBORw0KGgo=")).is_ok());
        assert!(check(&icon("image", "data:image/svg+xml;base64,PHN2Zz4=")).is_err());
        assert!(check(&icon("image", "data:image/png;base64,<script>")).is_err());
        assert!(check(&icon("image", &format!("data:image/png;base64,{}", "A".repeat(MAX_IMAGE_BYTES)))).is_err());
        assert!(check(&icon("sticker", "x")).is_err());
    }
}
