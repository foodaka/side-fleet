use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::agent::Agent;

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
    /// Which coding agent runs the task. A follow-up keeps the thread's latest agent unless it switches.
    #[serde(default = "default_agent")]
    pub agent: Agent,
    /// Model id for that agent; empty means the machine's default (Codex, opencode).
    pub model: String,
    /// Reasoning effort (see [`Agent::check_effort`]); None means the agent's default.
    #[serde(default)]
    pub effort: Option<String>,
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
    /// Session id reported by the agent for this run.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Session to resume (`claude --resume`, `codex exec resume`, `opencode run --session`), set on follow-ups.
    #[serde(default)]
    pub resume_session_id: Option<String>,
    /// Only this worker may claim the run (follow-ups must run where the session lives).
    #[serde(default)]
    pub pinned_worker_id: Option<String>,
    /// Permission mode (Claude Code's names; mapped for other agents); None means the worker's default.
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// `worktree` (own checkout and branch, default) or `checkout` (in place, one at a time per repo).
    #[serde(default = "default_isolation")]
    pub isolation: String,
    /// Move to another machine if this one drops: the worker pushes checkpoints of
    /// the worktree branch to `origin`, and a lost run is queued again elsewhere.
    #[serde(default)]
    pub failover: bool,
    /// Set on a run queued to replace one whose machine dropped: that lost run.
    #[serde(default)]
    pub moved_from: Option<Uuid>,
    /// Create the worktree from this branch on `origin` (fetched first) instead of HEAD.
    #[serde(default)]
    pub resume_from_branch: Option<String>,
    /// Images the user attached to this message (`GET /v1/images/:id`, `/v1/worker/images/:id`).
    #[serde(default)]
    pub images: Vec<ImageRef>,
    /// `agent` (the coding agent works on the task), `restore` (the worker puts
    /// the thread's worktree back to the snapshot in `restore_to`) or `pr` (the
    /// worker commits and pushes the thread's branch and opens a pull request).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// For a restore run: the snapshot commit to restore.
    #[serde(default)]
    pub restore_to: Option<String>,
    /// For a fork (a new thread continuing from another thread's message): that message's run.
    #[serde(default)]
    pub forked_from: Option<Uuid>,
    /// For a fork: the snapshot its worktree starts from (the files after that message).
    #[serde(default)]
    pub fork_commit: Option<String>,
    /// For a fork: the earlier conversation, given to the agent ahead of the task.
    /// For a `pr` run: the pull request's title, then a blank line and its body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// On a root run: when its thread was archived (hidden from the list).
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
    /// On a root run: the pull request opened from its branch, as last checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PullRequest>,
}

/// A GitHub pull request for a thread's branch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    pub url: String,
    pub number: i64,
    /// `open`, `merged` or `closed`.
    pub state: String,
}

/// A thread whose open pull request a worker should look at again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrCheck {
    pub thread: Uuid,
    pub url: String,
}

/// `POST /v1/worker/prs`: what a worker found out about threads' pull requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrReports {
    pub worker_id: String,
    pub prs: Vec<PrReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrReport {
    pub thread: Uuid,
    pub pr: PullRequest,
}

pub fn default_kind() -> String {
    "agent".into()
}

/// A stored image, as runs and events refer to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub id: Uuid,
    pub media_type: String,
    pub bytes: usize,
}

/// An image sent with a task or reply: base64 `data` of a PNG, JPEG, GIF or WebP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUpload {
    pub media_type: String,
    pub data: String,
}

/// Images one message may carry.
pub const MAX_ATTACHED_IMAGES: usize = 8;

pub fn default_agent() -> Agent {
    Agent::Claude
}

pub fn default_isolation() -> String {
    "worktree".into()
}

pub const ISOLATION_MODES: &[&str] = &["worktree", "checkout"];

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
    /// Tool calls in the thread waiting for someone to approve or deny them ("Ask me").
    #[serde(default)]
    pub pending_approvals: i64,
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
    /// Build of the worker binary, e.g. `0.1.0 (2026-10-20 8ec33ec)`.
    #[serde(default)]
    pub version: String,
    /// Live machine numbers, refreshed on every heartbeat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemStats>,
    /// Task slots: how many may run at once and how many are busy right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<Slots>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemStats {
    pub cpu_cores: usize,
    /// Average CPU busy percentage across cores since the previous sample.
    pub cpu_percent: f32,
    pub load_avg_1m: f64,
    pub mem_total_mb: u64,
    pub mem_used_mb: u64,
    /// Disk holding the repos folder.
    pub disk_total_gb: f64,
    pub disk_free_gb: f64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Slots {
    pub parallel: usize,
    pub active: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Agents {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ClaudeAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<CodexAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode: Option<OpencodeAgent>,
    /// A local model server, not an agent: opencode can use its models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaAgent>,
}

impl Agents {
    /// The agents this machine can run tasks with.
    pub fn runnable(&self) -> Vec<Agent> {
        let mut v = Vec::new();
        if self.claude.is_some() {
            v.push(Agent::Claude);
        }
        if self.codex.is_some() {
            v.push(Agent::Codex);
        }
        if self.opencode.is_some() {
            v.push(Agent::Opencode);
        }
        v
    }
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
    /// From `codex login status`; None when not checked (older workers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in: Option<bool>,
    /// Models this machine's Codex offers (`codex debug models`, the listed ones).
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpencodeAgent {
    pub version: String,
    /// `provider/model` ids opencode can use here (`opencode models`), e.g.
    /// `openrouter/moonshotai/kimi-k2`; capped, see the worker's detection.
    #[serde(default)]
    pub models: Vec<String>,
    /// Number of models before the cap.
    #[serde(default)]
    pub models_total: usize,
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
    /// The project's identity across machines: the normalised `origin` remote
    /// (`github.com/foodaka/side-fleet`), or `name` when there is no remote.
    /// Empty from older workers; treat as `name`.
    #[serde(default)]
    pub id: String,
}

impl RepoInfo {
    /// Identity used to match tasks to machines (falls back to the path name).
    pub fn key(&self) -> &str {
        if self.id.is_empty() { &self.name } else { &self.id }
    }
}

/// Normalise a git remote URL into a host/path identity, so the same project
/// matches on every machine whatever its folder is called. Credentials, ports,
/// `.git` and trailing slashes are dropped; the result is lower-case.
///
/// `git@github.com:Foo/Bar.git`, `https://user:tok@github.com/foo/bar`,
/// `ssh://git@github.com:22/foo/bar.git` all become `github.com/foo/bar`.
pub fn repo_identity(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        // scheme://[user[:pass]@]host[:port]/path
        let rest = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
        let (hostport, path) = rest.split_once('/')?;
        (hostport.split(':').next().unwrap_or(hostport), path)
    } else if let Some((left, path)) = url.split_once(':') {
        // scp-like: [user@]host:path (but not a Windows drive or a local path)
        if left.contains('/') || left.len() <= 1 {
            return None;
        }
        (left.rsplit_once('@').map(|(_, h)| h).unwrap_or(left), path)
    } else {
        return None; // a local path remote: no cross-machine identity
    };
    let path = path.trim_matches('/').trim_end_matches(".git").trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_ascii_lowercase(), path.to_ascii_lowercase()))
}

#[cfg(test)]
mod identity_tests {
    use super::repo_identity;

    #[test]
    fn normalises_common_remote_forms() {
        let want = Some("github.com/foodaka/side-fleet".to_string());
        assert_eq!(repo_identity("git@github.com:foodaka/side-fleet.git"), want);
        assert_eq!(repo_identity("git@github.com:Foodaka/Side-Fleet"), want);
        assert_eq!(repo_identity("https://github.com/foodaka/side-fleet.git"), want);
        assert_eq!(repo_identity("https://github.com/foodaka/side-fleet/"), want);
        assert_eq!(repo_identity("https://x-access-token:ghp_secret@github.com/foodaka/side-fleet.git"), want);
        assert_eq!(repo_identity("ssh://git@github.com:22/foodaka/side-fleet.git"), want);
        assert_eq!(repo_identity("https://gitlab.example.com:8443/group/sub/proj.git"), Some("gitlab.example.com/group/sub/proj".into()));
    }

    #[test]
    fn local_paths_have_no_identity() {
        assert_eq!(repo_identity("/srv/git/project.git"), None);
        assert_eq!(repo_identity("../other"), None);
        assert_eq!(repo_identity("C:/repos/x"), None);
        assert_eq!(repo_identity(""), None);
    }
}

// ---- developer API wire types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    pub task: String,
    /// claude (default), codex or opencode.
    #[serde(default)]
    pub agent: Option<String>,
    /// A model for that agent; may be empty for codex and opencode (the machine's default).
    #[serde(default)]
    pub model: String,
    /// Reasoning effort for that agent (see `Agent::efforts`); the agent's default when absent.
    #[serde(default)]
    pub effort: Option<String>,
    pub repo: String,
    /// acceptEdits (default), bypassPermissions, plan, ask, default, dontAsk.
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// worktree (default) or checkout.
    #[serde(default)]
    pub isolation: Option<String>,
    /// Run on this machine only; any machine with the repo when absent.
    #[serde(default)]
    pub worker_id: Option<String>,
    /// Move to another machine if this one drops (off by default). Pushes
    /// checkpoints of the task's `sidefleet/...` branch to the repo's `origin`.
    #[serde(default)]
    pub failover: bool,
    /// Images for the agent to look at, up to `MAX_ATTACHED_IMAGES`.
    #[serde(default)]
    pub images: Vec<ImageUpload>,
}

/// Continue a thread: a new run that resumes the previous session with the same agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowUpRequest {
    pub task: String,
    /// Switch the thread to another agent (claude, codex or opencode) from this message on.
    /// It starts its own session in the same worktree with a transcript of the thread so far.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Without it: the effort this agent last used in the thread.
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub images: Vec<ImageUpload>,
}

/// Fork a thread at a message into a new thread, with any agent and model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkRequest {
    /// The first message of the new thread.
    pub task: String,
    /// claude (default), codex or opencode.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Run on this machine. Absent: on the machine with the message's files, or any machine
    /// when there is no snapshot. Another machine starts from its own HEAD, without the files.
    #[serde(default)]
    pub worker_id: Option<String>,
}

/// Archive or unarchive a thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveRequest {
    pub archived: bool,
}

pub const PERMISSION_MODES: &[&str] = &["acceptEdits", "bypassPermissions", "plan", ASK_MODE, "default", "dontAsk"];

/// Sidefleet's own permission mode, "Ask me": Claude Code reads and edits files in the
/// workspace on its own and asks the person on the page before anything else (shell
/// commands, fetches, MCP tools). The worker relays each question as an [`Approval`].
pub const ASK_MODE: &str = "ask";

// ---- tool approvals ("Ask me") ----

/// How long a tool call waits for a decision before it is denied.
pub const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 15 * 60;
/// The longest wait a worker may ask for.
pub const MAX_APPROVAL_TIMEOUT_SECS: u64 = 24 * 60 * 60;

/// Lifecycle of an approval. Only a pending one changes, and only once:
/// pending -> approved | denied | expired (nobody decided in time) | cancelled (the run ended).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

/// What can happen to an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAction {
    Approve,
    Deny,
    /// Its time ran out (the worker asked after the deadline).
    Expire,
    /// Its run ended (finished, stopped, lost).
    Cancel,
}

impl ApprovalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Denied => "denied",
            ApprovalStatus::Expired => "expired",
            ApprovalStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => ApprovalStatus::Pending,
            "approved" => ApprovalStatus::Approved,
            "denied" => ApprovalStatus::Denied,
            "expired" => ApprovalStatus::Expired,
            "cancelled" => ApprovalStatus::Cancelled,
            _ => return None,
        })
    }

    /// The status after `action`, or None when it cannot happen (already decided).
    /// `overdue` means the deadline has passed: a late approve or deny expires it instead.
    pub fn after(self, action: ApprovalAction, overdue: bool) -> Option<ApprovalStatus> {
        if self != ApprovalStatus::Pending {
            return None;
        }
        Some(match action {
            ApprovalAction::Approve | ApprovalAction::Deny if overdue => ApprovalStatus::Expired,
            ApprovalAction::Approve => ApprovalStatus::Approved,
            ApprovalAction::Deny => ApprovalStatus::Denied,
            ApprovalAction::Expire => ApprovalStatus::Expired,
            ApprovalAction::Cancel => ApprovalStatus::Cancelled,
        })
    }
}

/// A tool call waiting for (or given) a decision from the person on the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: Uuid,
    pub run_id: Uuid,
    /// The agent's tool, e.g. `Bash`, `WebFetch`, `mcp__github__create_issue`.
    pub tool_name: String,
    /// The tool's input, redacted and with long strings clipped.
    pub input: Value,
    /// The agent's id for the tool call (`tool_use` events carry the same `id`).
    #[serde(default)]
    pub tool_use_id: Option<String>,
    pub status: ApprovalStatus,
    /// Why it was denied, expired or cancelled, as the agent is told.
    #[serde(default)]
    pub message: Option<String>,
    /// Who decided: a GitHub login, or `developer key`.
    #[serde(default)]
    pub decided_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub decided_at: Option<DateTime<Utc>>,
}

/// The worker asks the page about a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub tool_name: String,
    pub input: Value,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// How long to wait for a decision (default [`DEFAULT_APPROVAL_TIMEOUT_SECS`]).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Body of approve / deny; both fields optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DecideApprovalRequest {
    /// For a denial: what to tell the agent instead of the default.
    #[serde(default)]
    pub message: Option<String>,
}

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
    /// Runs this worker is executing right now. Any run assigned to it that is
    /// not listed here was lost (worker restart, crash) and gets failed.
    #[serde(default)]
    pub active_runs: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub id: String,
    pub capabilities: Capabilities,
    #[serde(default)]
    pub active_runs: Vec<Uuid>,
}

/// Returned on every heartbeat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    /// The control plane's build, for the worker to compare against its own.
    #[serde(default)]
    pub server_version: String,
    /// Someone pressed Update on the Machines page: update when idle, then restart.
    #[serde(default)]
    pub update_requested: bool,
    /// Open pull requests this worker opened, due to be checked for a merge or close.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check_prs: Vec<PrCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRequest {
    pub worker_id: String,
    /// Repo names (relative paths) this worker has checked out.
    pub repos: Vec<String>,
    /// Repos where an in-place (`checkout`) run is active; runs for these are skipped.
    #[serde(default)]
    pub busy_repos: Vec<String>,
    /// Agents installed on this machine (claude, codex, opencode). Empty from
    /// older workers, which only run Claude Code.
    #[serde(default)]
    pub agents: Vec<String>,
    /// This machine's task slots right now, so the control plane can hand a task
    /// to the least busy machine. None from older workers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<Slots>,
    /// Kinds of run this worker can execute (`agent`, `restore`, `fork`). Empty
    /// from older workers, which only run plain `agent` runs.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Hold the request up to this many seconds until a run is claimable,
    /// instead of answering 204 at once. None from older workers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_secs: Option<u64>,
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

/// The reply text an agent is writing (see [`crate::LIVE_TEXT`]), for whoever
/// follows the run right now. Never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveTextRequest {
    pub text: String,
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
    /// The agent stopped because this machine's account hit its usage limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit: Option<UsageLimit>,
}

/// An agent run ended on the account's usage or rate limit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageLimit {
    /// When the limit resets, if the agent said; the control plane assumes an hour otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod approval_tests {
    use super::{ApprovalAction::*, ApprovalStatus::*};

    #[test]
    fn only_a_pending_approval_changes() {
        assert_eq!(Pending.after(Approve, false), Some(Approved));
        assert_eq!(Pending.after(Deny, false), Some(Denied));
        assert_eq!(Pending.after(Expire, false), Some(Expired));
        assert_eq!(Pending.after(Cancel, false), Some(Cancelled));
        for done in [Approved, Denied, Expired, Cancelled] {
            for action in [Approve, Deny, Expire, Cancel] {
                assert_eq!(done.after(action, false), None, "{done:?} + {action:?}");
                assert_eq!(done.after(action, true), None, "{done:?} + {action:?} late");
            }
        }
    }

    #[test]
    fn a_late_decision_expires_instead() {
        assert_eq!(Pending.after(Approve, true), Some(Expired));
        assert_eq!(Pending.after(Deny, true), Some(Expired));
        assert_eq!(Pending.after(Cancel, true), Some(Cancelled));
    }

    #[test]
    fn statuses_round_trip() {
        for s in [Pending, Approved, Denied, Expired, Cancelled] {
            assert_eq!(super::ApprovalStatus::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
        }
    }
}
