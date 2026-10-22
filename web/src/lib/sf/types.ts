// Shapes returned by the Sidefleet control plane (crates/core/src/models.rs).

export type RunStatus = 'queued' | 'running' | 'succeeded' | 'failed' | 'stopped';

export type AgentId = 'claude' | 'codex' | 'opencode';

export interface Run {
	id: string;
	task: string;
	agent: AgentId;
	/** Empty means the machine's default model (Codex, opencode). */
	model: string;
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
	/** `restore` runs put the thread's worktree back to a snapshot instead of running the agent. */
	kind: 'agent' | 'restore';
	restore_to: string | null;
	/** For a fork: the message (run) of the thread it continues from. */
	forked_from: string | null;
	fork_commit: string | null;
}

export interface RunSummary extends Run {
	follow_ups: number;
	thread_status: RunStatus;
	last_activity_at: string;
}

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
}

export interface Setup {
	control_url: string;
	worker_token: string;
	worker_token_count: number;
	version: string;
	build?: string;
	/** Worker id of the machine hosting the control plane, if it also runs tasks. */
	host_worker?: string | null;
}
