// Shapes returned by the Sidefleet control plane (crates/core/src/models.rs).

export type RunStatus = 'queued' | 'running' | 'succeeded' | 'failed' | 'stopped';

export interface Run {
	id: string;
	task: string;
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
		codex?: { version: string };
		ollama?: { version: string; models: string[] };
	};
	repos: { name: string; path: string }[];
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
}

export interface Setup {
	control_url: string;
	worker_token: string;
	worker_token_count: number;
	version: string;
	build?: string;
}
