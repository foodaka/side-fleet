// The audit log: who did what in the org. The server decides what each person may read
// (owners see everything; others what happened on their machines and what they did) and
// redacts what it stores; this only asks for it and words it.

import { ApiError, authHeaders, Unauthorized } from './api';
import { agentName } from './format';
import type { AuditEvent } from './types';

export interface AuditFilter {
	/** An action, or a group of them (`run.*`). */
	action?: string;
	/** A login, `worker:<id>`, `system`, `developer key` or a user id. */
	actor?: string;
	machine?: string;
	run?: string;
	thread?: string;
	since?: string;
	until?: string;
}

/** The groups the Activity log filters by. */
export const ACTION_GROUPS: { id: string; label: string }[] = [
	{ id: 'run.*', label: 'Tasks' },
	{ id: 'approval.*', label: 'Approvals' },
	{ id: 'machine.*', label: 'Machines' },
	{ id: 'member.*', label: 'Members' },
	{ id: 'invite.*', label: 'Invites' },
	{ id: 'worker_token.*', label: 'Machine tokens' },
	{ id: 'api_token.*', label: 'API tokens' },
	{ id: 'org.*', label: 'Org' }
];

/** `?action=…&before=…`: empty parts left out (the server rejects an empty id). */
export function auditQuery(f: AuditFilter, extra: Record<string, string | number | null | undefined> = {}): string {
	const q = new URLSearchParams();
	for (const [k, v] of Object.entries({ ...f, ...extra })) if (v !== undefined && v !== null && v !== '') q.set(k, String(v));
	const s = q.toString();
	return s ? `?${s}` : '';
}

/** Local midnight at the start of a `yyyy-mm-dd` day, or of the day after it. */
export function dayBound(day: string, after = false): string | undefined {
	if (!/^\d{4}-\d{2}-\d{2}$/.test(day)) return undefined;
	const [y, m, d] = day.split('-').map(Number);
	return new Date(y, m - 1, d + (after ? 1 : 0)).toISOString();
}

/** Download the filtered log as CSV or JSON (the server caps it at 10,000 entries). */
export async function downloadAudit(orgId: string, f: AuditFilter, format: 'csv' | 'json'): Promise<{ truncated: boolean }> {
	const r = await fetch(`/v1/orgs/${orgId}/audit/export${auditQuery(f, { format })}`, { headers: authHeaders(), credentials: 'same-origin' });
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
	const name = /filename="([^"]+)"/.exec(r.headers.get('content-disposition') ?? '')?.[1] ?? `sidefleet-audit.${format}`;
	const url = URL.createObjectURL(await r.blob());
	const a = document.createElement('a');
	a.href = url;
	a.download = name;
	document.body.append(a);
	a.click();
	a.remove();
	setTimeout(() => URL.revokeObjectURL(url), 1000);
	return { truncated: r.headers.get('x-sidefleet-truncated') === 'true' };
}

export type ActorKind = 'person' | 'machine' | 'system' | 'key';

/** Who did it, as the log shows them. */
export function actorOf(e: AuditEvent): { kind: ActorKind; label: string; login: string | null } {
	if (e.actor.startsWith('worker:')) return { kind: 'machine', label: e.actor.slice('worker:'.length), login: null };
	if (e.actor === 'system') return { kind: 'system', label: 'Sidefleet', login: null };
	if (e.actor === 'developer key') return { kind: 'key', label: 'Developer key', login: null };
	return { kind: 'person', label: `@${e.actor}`, login: e.actor };
}

const str = (v: unknown) => (typeof v === 'string' && v ? v : null);
const tool = (e: AuditEvent) => str(e.detail.tool_name) ?? 'a tool';
const status: Record<string, string> = { succeeded: 'succeeded', failed: 'failed', stopped: 'was stopped' };
const mode: Record<string, string> = {
	acceptEdits: 'Accept edits',
	bypassPermissions: 'Full access',
	plan: 'Plan',
	ask: 'Ask me',
	default: 'Default',
	dontAsk: "Don't ask"
};
const modeName = (v: unknown) => (str(v) ? (mode[v as string] ?? (v as string)) : 'the default');

/** What happened, as the rest of a sentence after who did it: `queued a task`. */
export function describe(e: AuditEvent): string {
	const d = e.detail;
	switch (e.action) {
		case 'run.created':
			switch (d.via) {
				case 'reply': return 'replied in a thread';
				case 'fork': return 'forked a thread';
				case 'restore': return 'asked to restore the files';
				case 'pr': return 'asked for a pull request';
				case 'integrate': return 'asked to merge into the room';
				default: return `queued a task${d.agent ? ` for ${agentName(d.agent as string)}` : ''}`;
			}
		case 'run.claimed': return 'picked up a task';
		case 'run.started': return `started ${agentName(str(d.agent))}`;
		case 'run.finished': return `finished: the task ${status[str(d.status) ?? ''] ?? str(d.status) ?? 'ended'}${str(d.reason) ? ` (${d.reason})` : ''}`;
		case 'run.stopped': return 'stopped a task';
		case 'run.stopped_by_other': return "stopped a teammate's task";
		case 'run.requeued': return 'handed a task back: usage limit reached';
		case 'run.moved': return 'moved a task to another machine: its machine stopped responding';
		case 'run.permission_mode_changed': return `changed permissions from ${modeName(d.from)} to ${modeName(d.to)}`;
		case 'approval.requested': return `asked to use ${tool(e)}`;
		case 'approval.approved': return `approved ${tool(e)}`;
		case 'approval.denied': return `denied ${tool(e)}`;
		case 'approval.timed_out': return `denied ${tool(e)}: nobody answered in time`;
		case 'approval.cancelled': return `cancelled a question about ${tool(e)}: the task ended`;
		case 'machine.enrolled': return `connected ${str(d.display_name) ?? 'a machine'}${str(d.owner) ? ` for @${d.owner}` : ''}`;
		case 'machine.enrollment_approved': return `approved ${str(d.machine_name) ?? 'a machine'} joining`;
		case 'machine.enrollment_denied': return `turned down ${str(d.machine_name) ?? 'a machine'}`;
		case 'machine.removed': return `removed ${e.worker_id ?? 'a machine'}`;
		case 'worker_token.created': return `created machine token “${str(d.name) ?? ''}”`;
		case 'worker_token.revoked': return `revoked machine token “${str(d.name) ?? ''}”`;
		case 'api_token.created': return `created API token “${str(d.name) ?? ''}” (${str(d.scope) ?? ''})`;
		case 'api_token.revoked': return `revoked API token “${str(d.name) ?? ''}”`;
		case 'org.created': return 'created the org';
		case 'org.renamed': return `renamed the org to ${str(d.to) ?? ''}`;
		case 'invite.created': return `created an invite link for a${d.role === 'owner' ? 'n owner' : ' member'}`;
		case 'invite.revoked': return 'revoked an invite link';
		case 'member.added': return `added @${str(d.login) ?? 'someone'}`;
		case 'member.joined': return 'joined with an invite link';
		case 'member.role_changed': return `made @${str(d.login) ?? 'someone'} ${d.to === 'owner' ? 'an owner' : 'a member'}`;
		case 'member.removed': return `removed @${str(d.login) ?? 'someone'}`;
		case 'member.left': return 'left the org';
		default: return e.action;
	}
}

/** A readable, unambiguous time: `21 Nov 2026, 14:03:07`. */
export function stamp(iso: string): string {
	return new Date(iso).toLocaleString([], { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' });
}
