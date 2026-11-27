// The control plane: agents, their activity, policies, approvals and the audit log.
// Types follow docs/API.md ("Control plane"); labels and tones are how the pages show them.

import { api, authHeaders, ApiError, Unauthorized } from './api';

export type AgentKind = 'claude' | 'codex' | 'opencode' | 'custom';
export type AgentStatus = 'running' | 'waiting' | 'idle' | 'suspended';
export type Risk = 'low' | 'medium' | 'high' | 'unknown';
export type Decision = 'allow' | 'require_approval' | 'deny';
export type Tone = 'ok' | 'warn' | 'err' | 'run' | 'off';

export interface UserRef {
	id: string;
	login: string;
}

export interface Agent {
	id: string;
	name: string;
	description: string;
	kind: AgentKind;
	provider: string;
	model: string;
	runtime: string;
	host: string;
	worker_id: string | null;
	environment: string;
	team: string;
	owner: UserRef | null;
	risk: Risk;
	source: 'discovered' | 'registered';
	status: AgentStatus;
	pending_approvals: number;
	credentials: string[];
	can_cancel: boolean;
	suspension: { at: string; by: string; reason: string } | null;
	created_at: string;
	last_active_at: string | null;
}

export interface Permission {
	action: string;
	tool: string;
	decision: Decision;
	policy: { id: string; name: string } | null;
}

export interface PolicyRef {
	id: string;
	name: string;
	priority: number;
	scope: PolicyScope;
	/** The team or agent a team or agent policy is for (approvals carry it). */
	applies_to?: string | null;
}

export interface Incident {
	id: string;
	suspended_at: string;
	suspended_by: string;
	reason: string;
	revoked: string[];
	cancelled_runs: number;
	cancelled_approvals: number;
	not_stopped: string | null;
	reactivated_at: string | null;
	reactivated_by: string | null;
}

export interface AgentDetail {
	agent: Agent;
	permissions: Permission[];
	policies: PolicyRef[];
	incidents: Incident[];
}

export interface ActivityEvent {
	id: string;
	at: string;
	agent: { id: string; name: string } | null;
	owner: UserRef | null;
	team: string;
	run_id: string | null;
	type: string;
	tool: string;
	action: string;
	context: { task?: string; repo?: string; branch?: string } | null;
	outcome: string;
	policy: { id: string; name: string } | null;
	decision: Decision | null;
	metadata: Record<string, unknown> | null;
}

export type PolicyScope = 'org' | 'team' | 'agent';

export interface Policy {
	id: string;
	name: string;
	description: string;
	scope: PolicyScope;
	applies_to: string;
	tools: string[];
	category: string;
	decision: Decision;
	priority: number;
	active: boolean;
	builtin: boolean;
	created_at?: string;
	updated_at: string;
	updated_by: string | null;
}

export interface Evaluation {
	decision: Decision;
	policy: PolicyRef | null;
	overridden: { id: string; name: string; priority: number; decision: Decision }[];
	actions: { tool: string; target: string }[];
	reason: string;
	elapsed_ms: number;
}

export interface ControlApproval {
	id: string;
	kind: 'policy' | 'ask';
	status: 'pending' | 'approved' | 'denied' | 'expired' | 'cancelled';
	agent: { id: string; name: string } | null;
	owner: { id?: string; login: string } | null;
	runtime: string;
	run_id: string | null;
	title: string;
	tool: string;
	tool_name: string;
	command: string;
	digest: string | null;
	context: { task?: string; repo?: string; branch?: string } | null;
	policy: PolicyRef | null;
	message: string | null;
	decided_by: string | null;
	created_at: string;
	expires_at: string;
	decided_at: string | null;
	can_decide: boolean;
}

export interface AuditRecord {
	id: string;
	seq: number;
	at: string;
	actor: string;
	actor_kind: 'admin' | 'member' | 'agent' | 'api_key' | 'system';
	event: string;
	agent_id: string | null;
	target: string;
	detail: string;
	data: {
		what?: string;
		agent?: string;
		owner?: string;
		permissions?: string;
		policy?: string;
		approval?: string;
		decided_by?: string;
		outcome?: string;
	} | null;
	prev_hash: string;
	hash: string;
}

export interface AuditVerify {
	intact: boolean;
	records: number;
	broken_at: number | null;
	head: string;
	verified_at: string;
}

export interface Summary {
	agents: { total: number; teams: number; unowned: number; running: number; waiting: number; suspended: number };
	approvals: { pending: number; soonest_expiry: string | null };
	decisions_24h: { total: number; allowed: number; approval: number; denied: number; hours: { hour: string; allowed: number; approval: number; denied: number }[] };
	governed_7d: { total: number; governed: number; no_policy: number; unattributed: number };
	engine: { p95_ms: number | null };
}

// ---- labels and tones ----

export const STATUS: Record<AgentStatus, { label: string; tone: Tone }> = {
	running: { label: 'Running', tone: 'run' },
	waiting: { label: 'Waiting on approval', tone: 'warn' },
	idle: { label: 'Idle', tone: 'off' },
	suspended: { label: 'Suspended', tone: 'err' }
};

export const RISK: Record<Risk, { label: string; tone: Tone }> = {
	low: { label: 'Low', tone: 'off' },
	medium: { label: 'Medium', tone: 'warn' },
	high: { label: 'High', tone: 'err' },
	unknown: { label: 'Unknown', tone: 'run' }
};

export const DECISION: Record<Decision, { label: string; tone: Tone }> = {
	allow: { label: 'Allowed', tone: 'ok' },
	require_approval: { label: 'Approval required', tone: 'warn' },
	deny: { label: 'Denied', tone: 'err' }
};

export const OUTCOME: Record<string, { label: string; tone: Tone }> = {
	allowed: { label: 'Allowed', tone: 'ok' },
	approval_required: { label: 'Approval required', tone: 'warn' },
	denied: { label: 'Denied', tone: 'err' },
	failed: { label: 'Failed', tone: 'err' },
	succeeded: { label: 'Succeeded', tone: 'ok' },
	approved: { label: 'Approved', tone: 'ok' },
	rejected: { label: 'Rejected', tone: 'err' },
	expired: { label: 'Expired', tone: 'off' },
	cancelled: { label: 'Cancelled', tone: 'off' },
	started: { label: 'Started', tone: 'run' },
	stopped: { label: 'Stopped', tone: 'off' },
	suspended: { label: 'Suspended', tone: 'err' },
	reactivated: { label: 'Reactivated', tone: 'ok' }
};
export const outcome = (o: string) => OUTCOME[o] ?? { label: o.replace(/_/g, ' '), tone: 'off' as Tone };

export const EVENT_TYPES: Record<string, string> = {
	task_started: 'Task started',
	task_completed: 'Task completed',
	execution_failure: 'Execution failure',
	tool_call: 'Tool call',
	permission_request: 'Permission request',
	approval_approved: 'Approval',
	approval_rejected: 'Rejection',
	approval_expired: 'Approval expired',
	approval_cancelled: 'Approval cancelled',
	agent_suspended: 'Agent suspended',
	agent_reactivated: 'Agent reactivated'
};
export const eventType = (t: string) => EVENT_TYPES[t] ?? t.replace(/_/g, ' ');

/** An approval's status as people read it: the API's `denied` is a rejection. */
export const APPROVAL_STATUS: Record<ControlApproval['status'], { label: string; tone: Tone }> = {
	pending: { label: 'Pending', tone: 'warn' },
	approved: { label: 'Approved', tone: 'ok' },
	denied: { label: 'Rejected', tone: 'err' },
	expired: { label: 'Expired', tone: 'off' },
	cancelled: { label: 'Cancelled', tone: 'off' }
};

export const KIND: Record<AgentKind, string> = { claude: 'Claude Code', codex: 'Codex', opencode: 'opencode', custom: 'Custom (API)' };
export const kindName = (k: string) => KIND[k as AgentKind] ?? k;

export const SCOPE: Record<PolicyScope, string> = { org: 'Organisation', team: 'Team', agent: 'Agent' };

export const ACTOR_KIND: Record<AuditRecord['actor_kind'], string> = {
	admin: 'Admin',
	member: 'Member',
	agent: 'Agent',
	api_key: 'API key',
	system: 'Sidefleet'
};

/** `sha256:7f3a…c91e`. */
export function shortDigest(d: string | null | undefined): string {
	if (!d) return '';
	const [algo, hex] = d.includes(':') ? d.split(':', 2) : ['sha256', d];
	return hex.length > 10 ? `${algo}:${hex.slice(0, 4)}…${hex.slice(-4)}` : d;
}
export const shortHash = (h: string | null | undefined) => (h ? h.slice(0, 6) : '');

/** `14:32:08` today, else `27 Nov 14:32`. */
export function clock(iso: string | null | undefined, now = Date.now()): string {
	if (!iso) return '';
	const d = new Date(iso);
	const today = new Date(now).toDateString() === d.toDateString();
	const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: today ? '2-digit' : undefined, hour12: false });
	return today ? time : `${d.toLocaleDateString([], { day: 'numeric', month: 'short' })} ${time}`;
}

/** `26 min`, `52 s`, `2 h`: what is left before `iso`. */
export function remaining(iso: string, now = Date.now()): string {
	const s = Math.max(0, Math.round((new Date(iso).getTime() - now) / 1000));
	if (s < 60) return `${s} s`;
	if (s < 3600) return `${Math.round(s / 60)} min`;
	if (s < 86400) return `${Math.round(s / 3600)} h`;
	return `${Math.round(s / 86400)} d`;
}

export const ownerLabel = (o: UserRef | { login: string } | null | undefined) => (o ? o.login : 'Unassigned');
export const policyScopeLabel = (p: PolicyRef | Policy) =>
	`${SCOPE[p.scope]}${'applies_to' in p && p.applies_to && p.scope === 'team' ? ` ${p.applies_to}` : ''} · priority ${p.priority}`;

// ---- calls ----

const json = (body: unknown) => ({ method: 'POST', body: JSON.stringify(body) });
const qs = (q: Record<string, string | number | null | undefined>) => {
	const p = new URLSearchParams();
	for (const [k, v] of Object.entries(q)) if (v !== null && v !== undefined && v !== '') p.set(k, String(v));
	const s = p.toString();
	return s ? `?${s}` : '';
};

export const listAgents = () => api<Agent[]>('/v1/agents');
export const getAgent = (id: string) => api<AgentDetail>(`/v1/agents/${encodeURIComponent(id)}`);
export const registerAgent = (body: Partial<Agent> & { name: string; owner_id?: string }) =>
	api<{ agent: Agent; key: string }>('/v1/agents', json(body));
export const updateAgent = (id: string, body: Partial<Pick<Agent, 'name' | 'description' | 'team' | 'environment' | 'risk'>> & { owner_id?: string | null }) =>
	api<Agent>(`/v1/agents/${encodeURIComponent(id)}`, { method: 'PATCH', body: JSON.stringify(body) });
export const claimAgent = (id: string) => api<Agent>(`/v1/agents/${encodeURIComponent(id)}/claim`, { method: 'POST' });
export const suspendAgent = (id: string, body: { reason: string; revoke_credentials: boolean; cancel_running: boolean }) =>
	api<{ agent: Agent; incident: Incident }>(`/v1/agents/${encodeURIComponent(id)}/suspend`, json(body));
export const reactivateAgent = (id: string) => api<Agent>(`/v1/agents/${encodeURIComponent(id)}/reactivate`, { method: 'POST' });
export const newAgentKey = (id: string) => api<{ key: string }>(`/v1/agents/${encodeURIComponent(id)}/key`, { method: 'POST' });

export interface ActivityFilter {
	agent?: string;
	owner?: string;
	team?: string;
	type?: string;
	outcome?: string;
	since?: string;
	until?: string;
	limit?: number;
	before?: string;
}
export const listActivity = (f: ActivityFilter = {}) =>
	api<{ events: ActivityEvent[]; counts: Record<string, number> }>(`/v1/activity${qs({ ...f })}`);

export const listPolicies = () => api<Policy[]>('/v1/policies');
export type PolicyInput = Pick<Policy, 'name' | 'description' | 'scope' | 'applies_to' | 'tools' | 'category' | 'decision' | 'priority' | 'active'>;
export const createPolicy = (body: PolicyInput) => api<Policy>('/v1/policies', json(body));
export const updatePolicy = (id: string, body: Partial<PolicyInput>) =>
	api<Policy>(`/v1/policies/${encodeURIComponent(id)}`, { method: 'PATCH', body: JSON.stringify(body) });
export const deletePolicy = (id: string) => api(`/v1/policies/${encodeURIComponent(id)}`, { method: 'DELETE' });
export const evaluate = (body: { agent_id?: string; tool: string; action: string }) => api<Evaluation>('/v1/policy/evaluate', json(body));

export const listApprovals = (status: 'pending' | 'resolved') =>
	api<{ approvals: ControlApproval[]; counts: { pending: number; resolved: number } }>(`/v1/approvals?status=${status}`);
export const decideApproval = (id: string, verdict: 'approve' | 'reject', message?: string) =>
	api<ControlApproval>(`/v1/approvals/${encodeURIComponent(id)}/${verdict}`, json(message ? { message } : {}));

export interface AuditFilter {
	q?: string;
	event?: string;
	agent?: string;
	since?: string;
	until?: string;
	limit?: number;
	before?: number;
}
export const listAudit = (f: AuditFilter = {}) => api<{ records: AuditRecord[]; total: number }>(`/v1/audit${qs({ ...f })}`);
export const verifyAudit = () => api<AuditVerify>('/v1/audit/verify');
export const summary = () => api<Summary>('/v1/control/summary');

/** Download the audit log under `f` as a file. A plain link would not carry the developer key. */
export async function exportAudit(format: 'csv' | 'jsonl', f: AuditFilter = {}) {
	const r = await fetch(`/v1/audit/export${qs({ ...f, format })}`, { headers: authHeaders(), credentials: 'same-origin' });
	if (r.status === 401) throw new Unauthorized();
	if (!r.ok) {
		let msg = r.statusText;
		try {
			msg = (await r.json()).error ?? msg;
		} catch {
			/* not JSON */
		}
		throw new ApiError(msg, r.status);
	}
	const blob = await r.blob();
	const url = URL.createObjectURL(blob);
	const a = document.createElement('a');
	a.href = url;
	a.download = `sidefleet-audit-${new Date().toISOString().slice(0, 10)}.${format}`;
	document.body.append(a);
	a.click();
	a.remove();
	setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** The error to show for a failed write: a 403 names the role it needs. */
export function writeError(err: unknown, needs = 'an org owner'): string {
	if (err instanceof ApiError && err.status === 403) return err.message || `Only ${needs} can do that.`;
	return (err as Error).message;
}
