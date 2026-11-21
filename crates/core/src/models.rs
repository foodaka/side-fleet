use std::collections::BTreeMap;

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
    /// Other files the user attached: markdown, text, code, PDFs (`GET /v1/files/:id`, `/v1/worker/files/:id`).
    #[serde(default)]
    pub files: Vec<FileRef>,
    /// `agent` (the coding agent works on the task), `restore` (the worker puts
    /// the thread's worktree back to the snapshot in `restore_to`), `pr` (the
    /// worker commits and pushes the thread's branch and opens a pull request) or
    /// `integrate` (the worker merges the thread's branch into its room's branch,
    /// named in `context`, and pushes that).
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
    /// For an `integrate` run: the room branch to merge into.
    /// For a room's task: the room brief or update, ahead of the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// On a root run: when its thread was archived (hidden from the list).
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
    /// On a root run: the thread is snoozed (hidden from the list and the inbox) until then.
    /// Something new in the thread (a reply, or it starting to wait on you) ends it early.
    #[serde(default)]
    pub snoozed_until: Option<DateTime<Utc>>,
    /// On a root run: when its thread was pinned (kept at the top of the list).
    #[serde(default)]
    pub pinned_at: Option<DateTime<Utc>>,
    /// On a root run: the pull request opened from its branch, as last checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PullRequest>,
    /// The user who started this run (a task, a reply, a fork…): only machines they own run it.
    /// None for the developer key, whose runs go to machines connected with a static token.
    #[serde(default)]
    pub created_by: Option<Uuid>,
    /// The room this thread works on (set on every run of the thread).
    #[serde(default)]
    pub room_id: Option<Uuid>,
    /// A task scheduled to start later: no machine claims it before this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_at: Option<DateTime<Utc>>,
    /// Most this run's agent may pay for APIs, in millionths of a dollar. None: no payments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_micros: Option<i64>,
    /// Payments above this wait for someone to approve them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_above_micros: Option<i64>,
    /// What the agent has paid so far.
    #[serde(default)]
    pub spent_micros: i64,
    /// The access key the worker made for this run's payments (public: a [`PayKeyOffer`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pay_key: Option<Value>,
    /// The user's signed authorization of that key, once given (public data).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pay_authorization: Option<Value>,
    /// The services its agent may pay without asking, as they were when the run was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pay_policy: Option<PayPolicy>,
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

/// A stored file the user attached, as runs refer to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRef {
    pub id: Uuid,
    /// The file's own name (no folders), as the agent sees it.
    pub name: String,
    pub media_type: String,
    pub bytes: usize,
}

/// A file sent with a task or reply: its `name` and base64 `data`. Any type;
/// the agent is told where it is and reads it (or `--file` for opencode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileUpload {
    pub name: String,
    #[serde(default)]
    pub media_type: String,
    pub data: String,
}

/// Files (other than images) one message may carry.
pub const MAX_ATTACHED_FILES: usize = 8;

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
    /// The agent of the thread's running message, else its latest; a follow-up can switch it.
    #[serde(default = "default_agent")]
    pub latest_agent: Agent,
    /// That message's model; empty means the machine's default.
    #[serde(default)]
    pub latest_model: String,
    /// That message's reasoning effort; None means the agent's default.
    #[serde(default)]
    pub latest_effort: Option<String>,
    /// The last machine to take a message of the thread; a moved thread has changed machines.
    #[serde(default)]
    pub latest_worker_id: Option<String>,
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
    /// This worker's build answers "Clean up" on the Machines page.
    #[serde(default)]
    pub can_clean_worktrees: bool,
}

/// What the Machines page asks of a machine's worktrees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WorktreeCleanupRequest {
    /// List what a cleanup would do, deleting nothing.
    Preview,
    /// Remove these threads' worktrees (root run ids), if they are still safe to.
    Remove { ids: Vec<Uuid> },
}

/// Why a worktree would go or stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeState {
    Merged,
    OnDefault,
    InUse,
    Changes,
    Unmerged,
    OtherBranch,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeItem {
    pub root_id: Uuid,
    pub repo: String,
    pub branch: String,
    pub state: WorktreeState,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorktreePreview {
    pub items: Vec<WorktreeItem>,
}

/// What a worker reports back for a [`WorktreeCleanupRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeCleanupResult {
    Preview(WorktreePreview),
    Removed(WorktreeCleanup),
}

/// What a worktree cleanup did on a machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorktreeCleanup {
    pub removed: usize,
    pub freed_bytes: u64,
    /// A task is running in it, or it is locked.
    pub kept_in_use: usize,
    /// Uncommitted or untracked files.
    pub kept_changes: usize,
    /// Commits that aren't merged, or another branch checked out.
    pub kept_unmerged: usize,
    pub failed: usize,
    /// The worktrees removed.
    #[serde(default)]
    pub items: Vec<WorktreeItem>,
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
    /// What each alias runs here, `fable` to `claude-fable-5-2`. Empty from
    /// older workers, and when Claude Code wouldn't say.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, String>,
    /// The slash commands and skills Claude Code offers outside any project:
    /// built in, the user's own and the plugins'. Empty from older workers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<SlashCommand>,
}

/// A slash command or skill the composer can offer after `/`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommand {
    /// Without the slash: `review`, `stripe:test-cards`.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// What it takes after its name, e.g. `[pr number]`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub argument_hint: String,
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
    /// The project's own skills and slash commands (`.claude/skills`, `.claude/commands`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<SlashCommand>,
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
    /// Files for the agent to read, up to `MAX_ATTACHED_FILES`.
    #[serde(default)]
    pub files: Vec<FileUpload>,
    /// Start later: no machine picks the task up before this time (at most
    /// `MAX_START_DELAY_DAYS` ahead). Absent: as soon as a machine can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_at: Option<DateTime<Utc>>,
    /// Let the agent pay for APIs, up to this many millionths of a dollar (see [`MAX_BUDGET_MICROS`]).
    #[serde(default)]
    pub budget_micros: Option<i64>,
    /// Payments above this wait for approval ([`DEFAULT_ASK_ABOVE_MICROS`] when absent).
    #[serde(default)]
    pub ask_above_micros: Option<i64>,
}

/// How far ahead a task can be scheduled to start.
pub const MAX_START_DELAY_DAYS: i64 = 30;

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
    /// Files for the agent to read, up to `MAX_ATTACHED_FILES`.
    #[serde(default)]
    pub files: Vec<FileUpload>,
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

/// Pin a thread to the top of the list, or unpin it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinRequest {
    pub pinned: bool,
}

/// Snooze a thread until a time, or wake it now (`until` null).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnoozeRequest {
    pub until: Option<DateTime<Utc>>,
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

// ---- agent budgets (payments over MPP on Tempo) ----

/// The largest budget a task may have, in millionths of a dollar ($100).
pub const MAX_BUDGET_MICROS: i64 = 100_000_000;
/// Payments above this ask for approval unless the task says otherwise ($1).
pub const DEFAULT_ASK_ABOVE_MICROS: i64 = 1_000_000;
/// The approval tool name a payment above the threshold is asked about under.
pub const PAY_TOOL_NAME: &str = "sidefleet_pay";
/// The approval a budget waits under until the user's wallet signs it: it shows in the
/// bell and sends a push like any approval, but only a signature approves it; denying it
/// starts the agent without payments.
pub const BUDGET_TOOL_NAME: &str = "sidefleet_budget";

/// The worker made an access key for a run's payments and asks the user to authorize it.
/// Only public data: the key's address, never the key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PayKeyOffer {
    /// The access key's address (0x...).
    pub address: String,
    pub chain_id: u64,
    /// The TIP-20 token the budget is in (0x...); 6 decimals, so base units are micros.
    pub token: String,
    /// When the key should stop working.
    pub expires_at: DateTime<Utc>,
    /// How long the worker waits for the authorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// The user's root key signed an authorization for the offered key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizePayKeyRequest {
    /// The account the key spends from (0x...).
    pub wallet: String,
    /// The signed key authorization: an object (`sidefleet wallet`), or RLP hex (the page).
    pub authorization: Value,
    /// The key it authorizes, for an RLP authorization the control plane doesn't decode.
    #[serde(default)]
    pub key_id: Option<String>,
}

/// What the worker polls for after offering a key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PayKeyStatus {
    #[serde(default)]
    pub wallet: Option<String>,
    #[serde(default)]
    pub authorization: Option<Value>,
    /// The user turned the budget down (or nobody answered): start without payments.
    #[serde(default)]
    pub declined: bool,
}

/// A run's budget and key, for `sidefleet wallet authorize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayKeyInfo {
    pub run_id: Uuid,
    pub task: String,
    pub budget_micros: Option<i64>,
    pub ask_above_micros: Option<i64>,
    pub spent_micros: i64,
    pub key: Option<PayKeyOffer>,
    pub authorized: bool,
}

/// A payment the agent made, or one that was refused or failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentReport {
    /// `paid`, `refused` (over budget or denied) or `failed`.
    pub outcome: String,
    pub amount_micros: i64,
    pub url: String,
    pub host: String,
    /// What the agent said the payment is for.
    #[serde(default)]
    pub reason: Option<String>,
    /// Why it was refused or failed.
    #[serde(default)]
    pub detail: Option<String>,
    /// The service's receipt (`Payment-Receipt`), for a paid one.
    #[serde(default)]
    pub receipt: Option<Value>,
    /// The address the service asked to be paid (0x...).
    #[serde(default)]
    pub recipient: Option<String>,
}

/// Which services a task's agent may pay without asking first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayMode {
    /// Only the approved services; any other asks, whatever the amount.
    #[default]
    Approved,
    /// Any service (under the threshold).
    Any,
}

/// Your approved services, kept with your account and copied onto each task with a budget.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PayPolicy {
    #[serde(default)]
    pub mode: PayMode,
    #[serde(default)]
    pub approved: Vec<ApprovedService>,
}

/// A service the agent may pay without asking, by the host it is reached at (which TLS proves).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovedService {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The address its first payment went to. A payment to another address asks first, so
    /// a swapped endpoint can't redirect money quietly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
}

pub const MAX_APPROVED_SERVICES: usize = 200;

impl PayPolicy {
    pub fn find(&self, host: &str) -> Option<&ApprovedService> {
        self.approved.iter().find(|s| s.host.eq_ignore_ascii_case(host))
    }

    /// Lowercase hosts, one entry each, at most [`MAX_APPROVED_SERVICES`]; an error names a bad host.
    pub fn normalized(self) -> Result<PayPolicy, String> {
        if self.approved.len() > MAX_APPROVED_SERVICES {
            return Err(format!("at most {MAX_APPROVED_SERVICES} approved services"));
        }
        let mut approved: Vec<ApprovedService> = Vec::with_capacity(self.approved.len());
        for mut s in self.approved {
            s.host = s.host.trim().trim_end_matches('.').to_ascii_lowercase();
            let ok = !s.host.is_empty() && s.host.len() <= 253 && s.host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'));
            if !ok {
                return Err(format!("{:?} is not a host name", s.host));
            }
            s.name = s.name.map(|n| n.chars().take(100).collect());
            s.recipient = s.recipient.filter(|r| r.len() == 42 && r.starts_with("0x") && r[2..].bytes().all(|b| b.is_ascii_hexdigit()));
            match approved.iter_mut().find(|a| a.host == s.host) {
                Some(a) => *a = s,
                None => approved.push(s),
            }
        }
        Ok(PayPolicy { mode: self.mode, approved })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRecorded {
    pub spent_micros: i64,
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
    /// Someone is cleaning up this machine's worktrees from the Machines page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_cleanup: Option<WorktreeCleanupRequest>,
    /// Open pull requests this worker opened, due to be checked for a merge or close.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check_prs: Vec<PrCheck>,
    /// Runs the worker reported active that are already over here (stopped on the
    /// page while the worker was stuck): end them and free their slots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finished_runs: Vec<Uuid>,
    /// Worker routes take gzip-compressed bodies (`Content-Encoding: gzip`).
    #[serde(default)]
    pub accepts_gzip: bool,
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
