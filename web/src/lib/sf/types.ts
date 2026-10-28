// Shapes returned by the Sidefleet control plane (crates/core/src/models.rs).

export type RunStatus = 'queued' | 'running' | 'succeeded' | 'failed' | 'stopped';

export type AgentId = 'claude' | 'codex' | 'opencode';

export interface Run {
	id: string;
	task: string;
	agent: AgentId;
	/** Empty means the machine's default model (Codex, opencode). */
	model: string;
	/** Reasoning effort; null means the agent's default. */
	effort?: string | null;
	repo: string;
	status: RunStatus;
	worker_id: string | null;
	stop_requested: boolean;
	created_at: string;
	started_at: string | null;
	finished_at: string | null;
	result: Record<string, any> | null;
	parent_id: string | null;
	session_id: string | null;
	resume_session_id: string | null;
	pinned_worker_id: string | null;
	permission_mode: string | null;
	isolation: 'worktree' | 'checkout';
	/** Move to another machine if this one drops. */
	failover: boolean;
	/** On a run queued to replace one whose machine dropped: that run. */
	moved_from: string | null;
	/** The pushed branch a moved run continues from. */
	resume_from_branch: string | null;
	/** Images the user attached to this message. */
	images?: { id: string; media_type: string; bytes: number }[];
	/** `restore` runs put the thread's worktree back to a snapshot instead of running the agent;
	 * `pr` runs push the thread's branch and open a pull request. */
	kind: 'agent' | 'restore' | 'pr' | 'integrate';
	restore_to: string | null;
	/** For a fork: the message (run) of the thread it continues from. */
	forked_from: string | null;
	fork_commit: string | null;
	/** On a thread's first run: when the thread was archived. */
	archived_at?: string | null;
	/** On a thread's first run: the pull request opened from its branch, as last checked. */
	pr?: PullRequest | null;
	/** Who sent this message (a user id); null for the developer key and older runs. */
	created_by?: string | null;
	/** The room this thread belongs to, if any. */
	room_id?: string | null;
	/** For an `integrate` run: the room branch it merges into. */
	context?: string | null;
}

export interface PullRequest {
	url: string;
	number: number;
	state: 'open' | 'merged' | 'closed';
}

export interface RunSummary extends Run {
	follow_ups: number;
	thread_status: RunStatus;
	last_activity_at: string;
	/** Tool calls in the thread waiting for you to approve or deny them ("Ask me"). */
	pending_approvals?: number;
}

export type ApprovalStatus = 'pending' | 'approved' | 'denied' | 'expired' | 'cancelled';

export interface RunEvent {
	run_id: string;
	seq: number;
	kind: string;
	payload: Record<string, any>;
	at: string;
}

export interface SystemStats {
	cpu_cores: number;
	cpu_percent: number;
	load_avg_1m: number;
	mem_total_mb: number;
	mem_used_mb: number;
	disk_total_gb: number;
	disk_free_gb: number;
	uptime_secs: number;
}

export interface Capabilities {
	agents: {
		claude?: { version: string; signed_in: boolean; sign_in_error?: string };
		codex?: { version: string; signed_in?: boolean; models?: string[] };
		/** `models` is capped; `models_total` is how many opencode reported. */
		opencode?: { version: string; models: string[]; models_total: number };
		ollama?: { version: string; models: string[] };
	};
	/** `id` is the project's identity across machines (normalised git remote, or the folder name). */
	repos: { name: string; path: string; id?: string }[];
	hostname: string;
	os: string;
	version: string;
	system?: SystemStats;
	slots?: { parallel: number; active: number };
}

export interface Worker {
	id: string;
	display_name: string;
	capabilities: Capabilities;
	last_heartbeat_at: string;
	online: boolean;
	update_requested: boolean;
	outdated: boolean;
	/** Agents whose usage limit this machine's account hit, with when it resets; it takes none of their tasks until then. */
	limited_until?: Partial<Record<'claude' | 'codex' | 'opencode', string>>;
	/** Whose machine it is; null for machines connected with a static token. */
	owner?: { id: string; login: string } | null;
	/** It runs your tasks: machines only run their owner's tasks. Absent on older control planes (treat as yours). */
	mine?: boolean;
	/** Connected to another of its owner's orgs and serving them here too; doesn't count against this org's plan. */
	guest?: boolean;
}

/** One of your machines and the org it connected to (`GET /v1/me/machines`); it serves you in all your orgs. */
export interface MyMachine {
	id: string;
	display_name: string;
	org: { id: string; name: string };
	online: boolean;
	last_heartbeat_at: string;
	/** Repo keys, as tasks match them. */
	repos: string[];
	agents: AgentId[];
}

/** Someone in an org (`GET /v1/orgs/:org/members`). */
export interface Member {
	id: string;
	login: string;
	role: 'owner' | 'member';
}

/** A link that lets someone join an org. */
export interface Invite {
	id: string;
	role: 'owner' | 'member';
	created_by_login: string | null;
	expires_at: string;
	max_uses: number | null;
	uses: number;
	revoked_at: string | null;
}

/** What an invite link says about itself (`GET /v1/invites/:code`, no sign-in needed). */
export interface InviteInfo {
	org_name: string;
	inviter_login: string | null;
	role: string;
	valid: boolean;
	reason?: 'expired' | 'revoked' | 'used_up' | 'not_found' | (string & {});
}

/** A shared goal split into tasks, each on its owner's machine, merged into one room branch. */
export interface Room {
	id: string;
	name: string;
	goal: string;
	repo: string;
	branch: string;
	created_by: string | null;
	created_by_login: string | null;
	created_at: string;
	archived_at: string | null;
	/** First successful integration; new tasks then start from the room branch. */
	integrated_at: string | null;
	last_activity_at: string;
	task_count: number;
	active_count: number;
	integrated_count: number;
}

export interface RoomNote {
	id: string;
	author_id: string | null;
	author_login: string | null;
	body: string;
	created_at: string;
	edited_at: string | null;
}

export interface TaskCard {
	/** The thread's root run id. */
	id: string;
	title: string;
	created_by: string | null;
	created_by_login: string | null;
	agent: AgentId;
	model: string;
	worker_id: string | null;
	status: RunStatus;
	last_activity_at: string;
	files: string[];
	additions: number;
	deletions: number;
	integration: null | { status: RunStatus | (string & {}); at: string; commit?: string; error?: string; conflicts?: string[] };
	pr: PullRequest | null;
}

export interface RoomDetail {
	room: Room;
	notes: RoomNote[];
	tasks: TaskCard[];
}

export interface Setup {
	control_url: string;
	worker_token: string;
	worker_token_count: number;
	version: string;
	build?: string;
	/** Worker id of the machine hosting the control plane, if it also runs tasks. */
	host_worker?: string | null;
	/** Multi-tenant (GitHub sign-in); machines join with org tokens or approval on the page. */
	hosted?: boolean;
}

export interface AuthMethods {
	github: boolean;
	dev_key: boolean;
	hosted: boolean;
}

export interface Limits {
	machines: number;
	parallel_runs: number;
}

export interface Org {
	id: string;
	name: string;
	slug: string;
	plan: 'free' | 'pro';
	role: 'owner' | 'member';
	/** Null for the default org of a self-hosted control plane. */
	limits: Limits | null;
}

export interface Me {
	/** Null when signed in with the developer key. */
	user: { id: string; login: string; email: string | null } | null;
	orgs: Org[];
}

export interface WorkerToken {
	id: string;
	name: string;
	prefix: string;
	created_at: string;
	revoked_at: string | null;
	last_used_at: string | null;
	/** Only in the response that created it. */
	token?: string;
}

export type ApiScope = 'read' | 'tasks' | 'full';

/** A personal token for scripts and chat agents (Hermes, OpenClaw, MCP clients): acts as its user in one org. */
export interface ApiToken {
	id: string;
	name: string;
	scope: ApiScope;
	prefix: string;
	created_at: string;
	revoked_at: string | null;
	last_used_at: string | null;
	/** Updates in chat: Sidefleet tells the app when tasks finish or need an OK. */
	updates: ChatUpdates | null;
	/** Only in the response that created it. */
	token?: string;
}

export interface ChatUpdates {
	kind: 'hermes' | 'openclaw' | 'webhook';
	url: string;
	/** Every task, not only the ones this app started or replied to. */
	all_tasks: boolean;
	last_at: string | null;
	/** Why the last update didn't arrive. */
	last_error: string | null;
}

export interface Enrollment {
	id: string;
	code: string;
	machine_name: string;
	worker_id: string;
	capabilities: { hostname?: string; os?: string; version?: string; repo_count?: number; agents?: Capabilities['agents'] };
	status: 'pending' | 'approved' | 'denied' | 'collected';
	created_at: string;
	expires_at: string;
}

/** Why a task waits on you (crates/server/src/attention.rs). */
export type InboxReason = 'finished' | 'failed' | 'usage_limit' | 'machine_lost' | 'approval_needed';

/** A task in the "waiting on you" inbox (`GET /v1/inbox`). */
export interface InboxItem {
	/** The task (its thread's first run). */
	id: string;
	/** Its latest run. */
	run_id: string;
	title: string;
	repo: string;
	agent: AgentId;
	/** Unknown reasons may appear from newer control planes; show their label. */
	reason: InboxReason | (string & {});
	label: string;
	detail?: string;
	/** When it started waiting. */
	since: string;
}
