//! Rooms: a goal a team works on together. People talk in the room; the work
//! happens in tasks, ordinary threads that each run on their owner's machine on
//! a branch of their own. A finished task is merged into the room's branch (an
//! `integrate` run), and new tasks start from it. Every task's agent gets a room
//! brief: the goal, the team's notes and what the other tasks are doing.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{self, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::auth::Caller;
use crate::AppState;

pub const MAX_NAME_CHARS: usize = 120;
pub const MAX_GOAL_BYTES: usize = 20_000;
pub const MAX_NOTE_BYTES: usize = 10_000;
/// The brief an agent gets ahead of its task, at most.
const MAX_BRIEF_BYTES: usize = 12 * 1024;

/// The branch finished tasks are merged into.
pub fn branch(room: Uuid) -> String {
    format!("sidefleet/room-{}", &room.simple().to_string()[..8])
}

#[derive(Debug, Clone, Serialize)]
pub struct Room {
    pub id: Uuid,
    pub name: String,
    pub goal: String,
    pub repo: String,
    pub branch: String,
    pub created_by: Option<Uuid>,
    pub created_by_login: Option<String>,
    pub created_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    /// When a task was first merged into the room branch; tasks started after it start from that branch.
    pub integrated_at: Option<DateTime<Utc>>,
    pub last_activity_at: DateTime<Utc>,
    pub task_count: i64,
    pub active_count: i64,
    pub integrated_count: i64,
}

const ROOM_SELECT: &str = "SELECT rm.id, rm.name, rm.goal, rm.repo, rm.created_by, u.login AS created_by_login, rm.created_at, rm.archived_at,
        GREATEST(rm.last_activity_at, (SELECT max(r.created_at) FROM runs r WHERE r.room_id = rm.id)) AS last_activity_at,
        (SELECT min(r.finished_at) FROM runs r WHERE r.room_id = rm.id AND r.kind = 'integrate' AND r.status = 'succeeded') AS integrated_at,
        (SELECT count(*) FROM runs r WHERE r.room_id = rm.id AND r.parent_id IS NULL) AS task_count,
        (SELECT count(*) FROM runs r WHERE r.room_id = rm.id AND r.parent_id IS NULL AND EXISTS (
            SELECT 1 FROM runs t WHERE (t.id = r.id OR t.parent_id = r.id) AND t.status IN ('queued', 'running'))) AS active_count,
        (SELECT count(DISTINCT r.parent_id) FROM runs r WHERE r.room_id = rm.id AND r.kind = 'integrate' AND r.status = 'succeeded') AS integrated_count
     FROM rooms rm LEFT JOIN users u ON u.id = rm.created_by";

fn row_to_room(r: &PgRow) -> Room {
    let id: Uuid = r.get("id");
    Room {
        id,
        name: r.get("name"),
        goal: r.get("goal"),
        repo: r.get("repo"),
        branch: branch(id),
        created_by: r.get("created_by"),
        created_by_login: r.get("created_by_login"),
        created_at: r.get("created_at"),
        archived_at: r.get("archived_at"),
        integrated_at: r.get("integrated_at"),
        last_activity_at: r.get("last_activity_at"),
        task_count: r.get("task_count"),
        active_count: r.get("active_count"),
        integrated_count: r.get("integrated_count"),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: Uuid,
    pub author_id: Option<Uuid>,
    pub author_login: Option<String>,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Integration {
    pub status: String,
    pub at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskCard {
    pub id: Uuid,
    pub title: String,
    pub created_by: Option<Uuid>,
    pub created_by_login: Option<String>,
    pub agent: String,
    pub model: String,
    pub worker_id: Option<String>,
    pub status: String,
    pub last_activity_at: DateTime<Utc>,
    pub files: Vec<String>,
    pub additions: i64,
    pub deletions: i64,
    pub integration: Option<Integration>,
    pub pr: Option<Value>,
}

pub async fn get(pool: &PgPool, org: Uuid, id: Uuid) -> sqlx::Result<Option<Room>> {
    let row = sqlx::query(&format!("{ROOM_SELECT} WHERE rm.id = $1 AND rm.org_id = $2")).bind(id).bind(org).fetch_optional(pool).await?;
    Ok(row.as_ref().map(row_to_room))
}

pub async fn list(pool: &PgPool, org: Uuid, archived: bool) -> sqlx::Result<Vec<Room>> {
    let rows = sqlx::query(&format!("{ROOM_SELECT} WHERE rm.org_id = $1 AND (rm.archived_at IS NOT NULL) = $2 ORDER BY 9 DESC LIMIT 200"))
        .bind(org)
        .bind(archived)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_room).collect())
}

pub async fn create(pool: &PgPool, org: Uuid, by: Option<Uuid>, name: &str, goal: &str, repo: &str) -> sqlx::Result<Room> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO rooms (id, org_id, name, goal, repo, created_by) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(id)
        .bind(org)
        .bind(name)
        .bind(goal)
        .bind(repo)
        .bind(by)
        .execute(pool)
        .await?;
    Ok(get(pool, org, id).await?.expect("the room just created"))
}

pub async fn update(pool: &PgPool, org: Uuid, id: Uuid, name: Option<&str>, goal: Option<&str>, archived: Option<bool>) -> sqlx::Result<Option<Room>> {
    let r = sqlx::query(
        "UPDATE rooms SET name = COALESCE($3, name), goal = COALESCE($4, goal),
                archived_at = CASE WHEN $5 IS NULL THEN archived_at WHEN $5 THEN COALESCE(archived_at, now()) END,
                last_activity_at = now()
         WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org)
    .bind(name)
    .bind(goal)
    .bind(archived)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Ok(None);
    }
    get(pool, org, id).await
}

pub async fn add_note(pool: &PgPool, room: Uuid, author: Option<Uuid>, body: &str) -> sqlx::Result<Note> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO room_notes (id, room_id, author_id, body) VALUES ($1, $2, $3, $4)").bind(id).bind(room).bind(author).bind(body).execute(pool).await?;
    sqlx::query("UPDATE rooms SET last_activity_at = now() WHERE id = $1").bind(room).execute(pool).await?;
    let row = sqlx::query("SELECT n.id, n.author_id, u.login AS author_login, n.body, n.created_at FROM room_notes n LEFT JOIN users u ON u.id = n.author_id WHERE n.id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(row_to_note(&row))
}

fn row_to_note(r: &PgRow) -> Note {
    Note { id: r.get("id"), author_id: r.get("author_id"), author_login: r.get("author_login"), body: r.get("body"), created_at: r.get("created_at") }
}

/// The room's notes, oldest first (the latest `limit`).
pub async fn notes(pool: &PgPool, room: Uuid, limit: i64) -> sqlx::Result<Vec<Note>> {
    let rows = sqlx::query(
        "SELECT * FROM (SELECT n.id, n.author_id, u.login AS author_login, n.body, n.created_at FROM room_notes n
                        LEFT JOIN users u ON u.id = n.author_id WHERE n.room_id = $1 ORDER BY n.created_at DESC LIMIT $2) x
         ORDER BY created_at",
    )
    .bind(room)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_note).collect())
}

/// The room's tasks (its threads), most recently active first.
pub async fn tasks(pool: &PgPool, room: Uuid) -> sqlx::Result<Vec<TaskCard>> {
    let rows = sqlx::query(
        "SELECT root.id, root.task, root.created_by, u.login AS created_by_login, root.pr,
                l.agent, l.model, l.status, COALESCE(l.finished_at, l.started_at, l.created_at) AS last_activity_at,
                (SELECT r.worker_id FROM runs r WHERE (r.id = root.id OR r.parent_id = root.id) AND r.worker_id IS NOT NULL
                  ORDER BY r.created_at DESC LIMIT 1) AS worker_id,
                (SELECT e.payload->'total' FROM run_events e JOIN runs r ON r.id = e.run_id
                  WHERE (r.id = root.id OR r.parent_id = root.id) AND e.kind = 'diff' ORDER BY e.at DESC, e.seq DESC LIMIT 1) AS diff,
                i.status AS i_status, COALESCE(i.finished_at, i.created_at) AS i_at, i.result AS i_result
         FROM runs root
         LEFT JOIN users u ON u.id = root.created_by
         CROSS JOIN LATERAL (SELECT agent, model, status, created_at, started_at, finished_at FROM runs l
                             WHERE (l.id = root.id OR l.parent_id = root.id) AND l.kind = 'agent' ORDER BY l.created_at DESC LIMIT 1) l
         LEFT JOIN LATERAL (SELECT status, created_at, finished_at, result FROM runs i
                            WHERE i.parent_id = root.id AND i.kind = 'integrate' ORDER BY i.created_at DESC LIMIT 1) i ON true
         WHERE root.room_id = $1 AND root.parent_id IS NULL
         ORDER BY last_activity_at DESC",
    )
    .bind(room)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let diff: Option<Value> = r.get("diff");
            let diff = diff.unwrap_or(Value::Null);
            let files = diff["files"].as_array().map(|fs| fs.iter().filter_map(|f| f["path"].as_str().map(str::to_string)).collect()).unwrap_or_default();
            let integration = r.get::<Option<String>, _>("i_status").map(|status| {
                let result: Value = r.get::<Option<Value>, _>("i_result").unwrap_or(Value::Null);
                Integration {
                    status,
                    at: r.get("i_at"),
                    commit: result["integrated"]["commit"].as_str().map(str::to_string),
                    error: result["error"].as_str().map(str::to_string),
                    conflicts: result["conflicts"].as_array().map(|c| c.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()),
                }
            });
            TaskCard {
                id: r.get("id"),
                title: r.get("task"),
                created_by: r.get("created_by"),
                created_by_login: r.get("created_by_login"),
                agent: r.get("agent"),
                model: r.get("model"),
                worker_id: r.get("worker_id"),
                status: r.get("status"),
                last_activity_at: r.get("last_activity_at"),
                files,
                additions: diff["additions"].as_i64().unwrap_or(0),
                deletions: diff["deletions"].as_i64().unwrap_or(0),
                integration,
                pr: r.get("pr"),
            }
        })
        .collect())
}

/// What a task's agent is told about the room, ahead of its first message
/// (`first`) or a reply. `this_task` is left out of the other tasks.
pub fn brief(room: &Room, notes: &[Note], tasks: &[TaskCard], this_task: Option<Uuid>, first: bool) -> String {
    let mut s = String::new();
    if first {
        s.push_str(
            "You are working on one task in a shared room on Sidefleet: several people's agents work toward one goal, \
             each on its own branch and machine. Finished tasks are merged into the room branch ",
        );
        s.push_str(&format!("`{}`", room.branch));
        if room.integrated_at.is_some() {
            s.push_str(", and your branch starts from it");
        }
        s.push_str(". Stay within your task; avoid reworking files the other tasks are changing unless your task needs it.\n\n");
    } else {
        s.push_str("Room update from Sidefleet, for context (what your teammates are doing now):\n\n");
    }
    s.push_str(&format!("## Room: {}\n", one_line(&room.name, 120)));
    if first && !room.goal.trim().is_empty() {
        s.push_str(&format!("Goal:\n{}\n", clip(room.goal.trim(), 4000)));
    }
    let others: Vec<&TaskCard> = tasks.iter().filter(|t| Some(t.id) != this_task).take(20).collect();
    if !others.is_empty() {
        s.push_str("\n## Other tasks in this room\n");
        for t in others {
            let who = t.created_by_login.as_deref().map(|l| format!(" by @{l}")).unwrap_or_default();
            let state = match t.integration.as_ref().map(|i| i.status.as_str()) {
                Some("succeeded") => "merged into the room branch".to_string(),
                _ => t.status.clone(),
            };
            s.push_str(&format!("- \"{}\"{who}: {state}", one_line(&t.title, 100)));
            if !t.files.is_empty() {
                let shown: Vec<&str> = t.files.iter().take(8).map(String::as_str).collect();
                let more = t.files.len().saturating_sub(shown.len());
                s.push_str(&format!("; files: {}{}", shown.join(", "), if more > 0 { format!(" (+{more} more)") } else { String::new() }));
            }
            s.push('\n');
        }
    }
    let recent: Vec<&Note> = notes.iter().rev().take(if first { 15 } else { 8 }).collect();
    if !recent.is_empty() {
        s.push_str("\n## Notes from the team (oldest first)\n");
        for n in recent.into_iter().rev() {
            let who = n.author_login.as_deref().unwrap_or("someone");
            s.push_str(&format!("- @{who}: {}\n", clip(&n.body.replace('\n', " "), 500)));
        }
    }
    s.push_str(if first { "\n## Your task\n" } else { "\n## New message\n" });
    clip(&s, MAX_BRIEF_BYTES)
}

fn one_line(s: &str, max: usize) -> String {
    clip(s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim(), max)
}

/// At most `max` bytes, cut at a character boundary, with an ellipsis when cut.
fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub(3);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// The brief for a task about to start (or a reply in one), from the room as it is now.
pub async fn brief_for(pool: &PgPool, room: &Room, this_task: Option<Uuid>, first: bool) -> sqlx::Result<String> {
    let notes = notes(pool, room.id, 20).await?;
    let tasks = tasks(pool, room.id).await?;
    Ok(brief(room, &notes, &tasks, this_task, first))
}

// ---- HTTP ----

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

/// Developer routes, behind developer auth. Starting a task in a room is in `routes` (`POST /v1/rooms/:id/tasks`).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/rooms", routing::get(get_rooms).post(post_room))
        .route("/v1/rooms/{id}", routing::get(get_room).patch(patch_room))
        .route("/v1/rooms/{id}/notes", post(post_note))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    archived: bool,
}

async fn get_rooms(State(state): State<AppState>, Extension(caller): Extension<Caller>, Query(q): Query<ListQuery>) -> Result<Json<Vec<Room>>, ApiError> {
    Ok(Json(list(&state.pool, caller.org_id(), q.archived).await?))
}

#[derive(Deserialize)]
struct NewRoom {
    name: String,
    #[serde(default)]
    goal: String,
    repo: String,
}

fn check_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("a room needs a name of 1 to {MAX_NAME_CHARS} characters")));
    }
    Ok(name.to_string())
}

fn check_goal(goal: &str) -> Result<&str, ApiError> {
    if goal.len() > MAX_GOAL_BYTES {
        return Err(ApiError(StatusCode::BAD_REQUEST, "the goal is too long".into()));
    }
    Ok(goal.trim())
}

async fn post_room(State(state): State<AppState>, Extension(caller): Extension<Caller>, Json(req): Json<NewRoom>) -> Result<(StatusCode, Json<Room>), ApiError> {
    let name = check_name(&req.name)?;
    let goal = check_goal(&req.goal)?;
    let repo = req.repo.trim();
    if repo.is_empty() || repo.contains("..") || repo.starts_with('/') {
        return Err(ApiError(StatusCode::BAD_REQUEST, "repo must be a relative repo name".into()));
    }
    let room = create(&state.pool, caller.org_id(), caller.user_id(), &name, goal, repo).await?;
    tracing::info!(room = %room.id, org = %caller.org_id(), "room created");
    Ok((StatusCode::CREATED, Json(room)))
}

async fn get_room(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>) -> Result<Json<Value>, ApiError> {
    let room = get(&state.pool, caller.org_id(), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "room not found".into()))?;
    let notes = notes(&state.pool, id, 500).await?;
    let tasks = tasks(&state.pool, id).await?;
    Ok(Json(json!({ "room": room, "notes": notes, "tasks": tasks })))
}

#[derive(Deserialize)]
struct PatchRoom {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
}

async fn patch_room(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>, Json(req): Json<PatchRoom>) -> Result<Json<Room>, ApiError> {
    let room = get(&state.pool, caller.org_id(), id).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "room not found".into()))?;
    if room.created_by.is_some() && room.created_by != caller.user_id() && !caller.is_owner() {
        return Err(ApiError(StatusCode::FORBIDDEN, "only the room's creator or an owner of the org can change it".into()));
    }
    let name = req.name.as_deref().map(check_name).transpose()?;
    let goal = req.goal.as_deref().map(check_goal).transpose()?;
    let room = update(&state.pool, caller.org_id(), id, name.as_deref(), goal, req.archived).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "room not found".into()))?;
    Ok(Json(room))
}

#[derive(Deserialize)]
struct NewNote {
    body: String,
}

async fn post_note(State(state): State<AppState>, Extension(caller): Extension<Caller>, Path(id): Path<Uuid>, Json(req): Json<NewNote>) -> Result<(StatusCode, Json<Note>), ApiError> {
    let body = req.body.trim();
    if body.is_empty() || body.len() > MAX_NOTE_BYTES {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("a note is 1 to {MAX_NOTE_BYTES} bytes")));
    }
    if get(&state.pool, caller.org_id(), id).await?.is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "room not found".into()));
    }
    Ok((StatusCode::CREATED, Json(add_note(&state.pool, id, caller.user_id(), body).await?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> Room {
        Room {
            id: Uuid::parse_str("1a2b3c4d-0000-0000-0000-000000000000").unwrap(),
            name: "Apple Pay".into(),
            goal: "Add Apple Pay to checkout.".into(),
            repo: "shop".into(),
            branch: "sidefleet/room-1a2b3c4d".into(),
            created_by: None,
            created_by_login: None,
            created_at: Utc::now(),
            archived_at: None,
            integrated_at: None,
            last_activity_at: Utc::now(),
            task_count: 2,
            active_count: 1,
            integrated_count: 0,
        }
    }

    fn task(id: u128, title: &str, who: &str, files: &[&str]) -> TaskCard {
        TaskCard {
            id: Uuid::from_u128(id),
            title: title.into(),
            created_by: None,
            created_by_login: Some(who.into()),
            agent: "claude".into(),
            model: "sonnet".into(),
            worker_id: None,
            status: "running".into(),
            last_activity_at: Utc::now(),
            files: files.iter().map(|f| f.to_string()).collect(),
            additions: 0,
            deletions: 0,
            integration: None,
            pr: None,
        }
    }

    #[test]
    fn branch_is_named_after_the_room() {
        assert_eq!(branch(room().id), "sidefleet/room-1a2b3c4d");
    }

    #[test]
    fn first_brief_has_goal_other_tasks_and_notes_but_not_this_task() {
        let tasks = vec![task(1, "Backend endpoint", "sam", &["server/pay.rs"]), task(2, "Frontend button\nmore", "mark", &[])];
        let notes = vec![Note { id: Uuid::nil(), author_id: None, author_login: Some("ana".into()), body: "Use the sandbox keys".into(), created_at: Utc::now() }];
        let b = brief(&room(), &notes, &tasks, Some(Uuid::from_u128(2)), true);
        assert!(b.contains("`sidefleet/room-1a2b3c4d`"));
        assert!(b.contains("Goal:\nAdd Apple Pay to checkout."));
        assert!(b.contains("- \"Backend endpoint\" by @sam: running; files: server/pay.rs"));
        assert!(!b.contains("Frontend button"));
        assert!(b.contains("- @ana: Use the sandbox keys"));
        assert!(b.ends_with("## Your task\n"));
    }

    #[test]
    fn a_reply_gets_an_update_without_the_goal_and_merged_tasks_say_so() {
        let mut t = task(1, "Backend endpoint", "sam", &[]);
        t.integration = Some(Integration { status: "succeeded".into(), at: Utc::now(), commit: None, error: None, conflicts: None });
        let b = brief(&room(), &[], &[t], None, false);
        assert!(b.starts_with("Room update from Sidefleet"));
        assert!(!b.contains("Goal:"));
        assert!(b.contains("merged into the room branch"));
        assert!(b.ends_with("## New message\n"));
    }

    #[test]
    fn clip_cuts_on_a_character_boundary() {
        assert_eq!(clip("héllo", 100), "héllo");
        let c = clip(&"é".repeat(100), 11);
        assert!(c.ends_with('…') && c.len() <= 11 + '…'.len_utf8());
    }
}
