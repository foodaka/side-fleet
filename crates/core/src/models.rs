use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Lifecycle of a run. Transitions: queued -> running -> succeeded|failed|stopped,
/// and queued -> stopped when stopped before any worker claimed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Stopped,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
            RunStatus::Stopped => "stopped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => RunStatus::Queued,
            "running" => RunStatus::Running,
            "succeeded" => RunStatus::Succeeded,
            "failed" => RunStatus::Failed,
            "stopped" => RunStatus::Stopped,
            _ => return None,
        })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunStatus::Succeeded | RunStatus::Failed | RunStatus::Stopped
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: Uuid,
    pub task: String,
    pub model: String,
    pub repo: String,
    pub status: RunStatus,
    pub worker_id: Option<String>,
    pub stop_requested: bool,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result: Option<Value>,
    /// Root run of the thread this run continues (None for a root).
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    /// Claude Code session id reported by the agent for this run.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Session to resume (`claude -p --resume`), set on follow-ups.
    #[serde(default)]
    pub resume_session_id: Option<String>,
    /// Only this worker may claim the run (follow-ups must run where the session lives).
    #[serde(default)]
    pub pinned_worker_id: Option<String>,
    /// Permission mode for Claude Code; None means the worker's default.
    #[serde(default)]
    pub permission_mode: Option<String>,
}

/// A root run plus what the page needs to list a thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    #[serde(flatten)]
    pub run: Run,
    /// Number of follow-up messages in the thread.
    pub follow_ups: i64,
    /// Status of the most recent run in the thread.
    pub thread_status: RunStatus,
    /// Last activity in the thread.
    pub last_activity_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub run_id: Uuid,
    pub seq: i64,
    pub kind: String,
    pub payload: Value,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worker {
    pub id: String,
    pub display_name: String,
    pub capabilities: Capabilities,
    pub last_heartbeat_at: DateTime<Utc>,
}

/// What a worker machine can do. Reported on register and on every heartbeat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub agents: Agents,
    #[serde(default)]
    pub repos: Vec<RepoInfo>,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Agents {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ClaudeAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<CodexAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaAgent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeAgent {
    pub version: String,
    /// Whether a trivial `claude -p` succeeded, i.e. the CLI is signed in.
    pub signed_in: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexAgent {
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OllamaAgent {
    pub version: String,
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    /// Path relative to the worker's repos root, e.g. `golf/pgs-web`.
    pub name: String,
    /// Absolute path on the worker machine.
    pub path: String,
}

// ---- developer API wire types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    pub task: String,
    pub model: String,
    pub repo: String,
    /// acceptEdits (default), bypassPermissions, plan, default, dontAsk.
    #[serde(default)]
    pub permission_mode: Option<String>,
}

/// Continue a thread: a new run that resumes the previous Claude Code session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowUpRequest {
    pub task: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
}

pub const PERMISSION_MODES: &[&str] = &["acceptEdits", "bypassPermissions", "plan", "default", "dontAsk"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunResponse {
    pub id: Uuid,
    pub status: RunStatus,
}

// ---- worker API wire types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterWorkerRequest {
    pub id: String,
    pub display_name: String,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub id: String,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRequest {
    pub worker_id: String,
    /// Repo names (relative paths) this worker has checked out.
    pub repos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub kind: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostEventsRequest {
    pub events: Vec<NewEvent>,
}

/// Returned to the worker after posting events and from the control endpoint,
/// so it learns about a stop request without an extra round trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunControl {
    pub status: RunStatus,
    pub stop_requested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishRunRequest {
    pub status: RunStatus,
    #[serde(default)]
    pub result: Option<Value>,
}
