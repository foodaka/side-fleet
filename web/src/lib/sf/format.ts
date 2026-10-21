import type { AgentId, Worker } from './types';

export const AGENTS: { id: AgentId; name: string; hint: string }[] = [
	{ id: 'claude', name: 'Claude Code', hint: "Anthropic's agent, with the machine's Claude sign-in" },
	{ id: 'codex', name: 'Codex', hint: "OpenAI's agent, with the machine's ChatGPT sign-in" },
	{ id: 'opencode', name: 'opencode', hint: 'Open source; any model it is set up for (OpenRouter, local…)' }
];

export const agentName = (id: string | null | undefined) => AGENTS.find((a) => a.id === id)?.name ?? 'Claude Code';

/** Claude Code's models; Codex and opencode offer whatever each machine reports. */
export const MODELS = [
	{ id: 'claude-sonnet-5-5', name: 'Sonnet 5.5', hint: 'Balanced' },
	{ id: 'claude-opus-5-5', name: 'Opus 5.5', hint: 'Most capable' },
	{ id: 'claude-haiku-5-5', name: 'Haiku 5.5', hint: 'Fastest' }
] as const;

/** How a model reads: `Sonnet 5.5`, `gpt-6-luna`, `moonshotai/kimi-k2` (opencode's provider prefix dropped). */
export function modelName(id: string, agent?: string | null) {
	if (!id) return 'default model';
	const claude = MODELS.find((m) => m.id === id);
	if (claude) return claude.name;
	return agent === 'opencode' && id.includes('/') ? id.slice(id.indexOf('/') + 1) : id;
}

/** `Claude Code · Sonnet 5.5`, `opencode · moonshotai/kimi-k2`. */
export const agentModel = (agent: string | null | undefined, model: string) => `${agentName(agent)} · ${modelName(model, agent)}`;

/** Whether a machine can run tasks with this agent (an older control plane run without `agent` is Claude Code). */
export const hasAgent = (w: Worker, agent: AgentId) => !!w.capabilities.agents?.[agent];

/** Models the given machines offer for an agent, most common first. */
export function machineModels(workers: Worker[], agent: AgentId): string[] {
	const count = new Map<string, number>();
	workers.forEach((w) => {
		const a = w.capabilities.agents;
		const list = agent === 'codex' ? a.codex?.models : agent === 'opencode' ? a.opencode?.models : undefined;
		(list ?? []).forEach((m) => count.set(m, (count.get(m) ?? 0) + 1));
	});
	return [...count.keys()].sort((x, y) => count.get(y)! - count.get(x)! || 0);
}

export const PERMISSIONS = [
	{ id: 'acceptEdits', name: 'Read & edit files', hint: 'Can change files; shell commands are refused' },
	{ id: 'bypassPermissions', name: 'Full access', hint: 'Can also run commands on that machine' },
	{ id: 'plan', name: 'Read only', hint: 'Looks and answers, changes nothing' }
] as const;

/** What a permission mode means for an agent: Codex maps them onto its sandbox. */
export function permHint(id: string, agent: AgentId): string {
	if (agent === 'codex') {
		if (id === 'acceptEdits') return 'Edits files and runs commands inside its sandbox, no network';
		if (id === 'bypassPermissions') return 'No sandbox: runs anything on that machine';
	}
	return PERMISSIONS.find((p) => p.id === id)?.hint ?? '';
}

export const permName = (id: string | null | undefined) =>
	PERMISSIONS.find((p) => p.id === id)?.name ?? (id === 'default' ? 'Default permissions' : id ?? '');

export const WORKSPACES = [
	{ id: 'worktree', name: 'Own branch', hint: 'A separate worktree and branch, so tasks run side by side' },
	{ id: 'checkout', name: 'In place', hint: 'The checkout itself; one task per repo at a time' }
] as const;

export const STATUS_LABEL: Record<string, string> = {
	queued: 'Queued',
	running: 'Running',
	succeeded: 'Done',
	failed: 'Failed',
	stopped: 'Stopped'
};

export function rel(iso: string | null | undefined, now = Date.now()): string {
	if (!iso) return '';
	const s = Math.max(0, (now - new Date(iso).getTime()) / 1000);
	if (s < 10) return 'just now';
	if (s < 60) return Math.round(s) + 's ago';
	if (s < 3600) return Math.round(s / 60) + 'm ago';
	if (s < 86400) return Math.round(s / 3600) + 'h ago';
	return Math.round(s / 86400) + 'd ago';
}

export function duration(start: string | null | undefined, end?: string | null, now = Date.now()): string {
	if (!start) return '';
	const s = Math.max(0, ((end ? new Date(end).getTime() : now) - new Date(start).getTime()) / 1000);
	if (s < 60) return Math.round(s) + 's';
	if (s < 3600) return Math.floor(s / 60) + 'm ' + String(Math.round(s % 60)).padStart(2, '0') + 's';
	return Math.floor(s / 3600) + 'h ' + String(Math.round((s % 3600) / 60)).padStart(2, '0') + 'm';
}

export const clock = (iso: string) =>
	new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });

export const money = (n: number | null | undefined, digits = 2) => (n == null ? '' : '$' + Number(n).toFixed(digits));

export const pct = (used: number, total: number) => (total ? Math.min(100, Math.round((used / total) * 100)) : 0);

export function uptime(secs: number): string {
	const d = Math.floor(secs / 86400), h = Math.floor((secs % 86400) / 3600);
	return d ? `${d}d ${h}h` : `${h}h ${Math.floor((secs % 3600) / 60)}m`;
}

/** "0.1.0 (2026-10-20 dec70b750)" -> { version, date, sha } */
export function parseBuild(b: string | undefined) {
	const m = /^(\S+)\s+\((\S+)\s+(\S+)\)$/.exec(b ?? '');
	return m ? { version: m[1], date: m[2], sha: m[3] } : { version: b ?? '', date: '', sha: '' };
}

/** A repo's identity across machines; older workers only report the folder name. */
export const repoKey = (r: { name: string; id?: string }) => r.id || r.name;

/** How a repo identity reads: `github.com/foodaka/side-fleet` -> `foodaka/side-fleet`. */
export function repoLabel(key: string | null | undefined): string {
	if (!key) return '';
	const [first, ...rest] = key.split('/');
	return rest.length >= 2 && first.includes('.') ? rest.join('/') : key;
}
