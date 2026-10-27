import type { AgentId, Worker } from './types';

export const AGENTS: { id: AgentId; name: string; hint: string }[] = [
	{ id: 'claude', name: 'Claude Code', hint: "Anthropic's agent, with the machine's Claude sign-in" },
	{ id: 'codex', name: 'Codex', hint: "OpenAI's agent, with the machine's ChatGPT sign-in" },
	{ id: 'opencode', name: 'opencode', hint: 'Open source; any model it is set up for (OpenRouter, local…)' }
];

export const agentName = (id: string | null | undefined) => AGENTS.find((a) => a.id === id)?.name ?? 'Claude Code';

/** Claude Code's model aliases, each the latest version the machine's Claude Code knows; Codex and opencode offer whatever each machine reports. */
export const MODELS = [
	{ id: 'sonnet', name: 'Sonnet', hint: 'Balanced' },
	{ id: 'opus', name: 'Opus', hint: 'Most capable' },
	{ id: 'fable', name: 'Fable', hint: 'Newest family' },
	{ id: 'haiku', name: 'Haiku', hint: 'Fastest' }
] as const;

/** A full Claude model id (`claude-sonnet-5-5`): what Claude Code reports running, and what tasks from before the aliases stored. */
const claudeId = /^claude-[a-z0-9-]+$/;

/** How a model reads: `Sonnet`, `Sonnet 5.5`, `gpt-6-luna`, `moonshotai/kimi-k2` (opencode's provider prefix dropped). */
export function modelName(id: string, agent?: string | null) {
	if (!id) return 'default model';
	const claude = MODELS.find((m) => m.id === id);
	if (claude) return claude.name;
	const full = /^claude-([a-z]+)-(\d+)-(\d+)$/.exec(id);
	if (full) return `${full[1][0].toUpperCase()}${full[1].slice(1)} ${full[2]}.${full[3]}`;
	return agent === 'opencode' && id.includes('/') ? id.slice(id.indexOf('/') + 1) : id;
}

/** Claude Code takes one of its aliases or a full id; Codex and opencode any id that can't pass as a flag (empty: the machine's default). */
export const modelValid = (agent: AgentId, model: string) =>
	agent === 'claude'
		? MODELS.some((m) => m.id === model) || claudeId.test(model)
		: /^[A-Za-z0-9._:/@+-]*$/.test(model) && !model.startsWith('-');

/** Default model for an agent: Sonnet for Claude Code, the machine's default otherwise. */
export const defaultModel = (agent: AgentId) => (agent === 'claude' ? 'sonnet' : '');

/** `Claude Code · Sonnet 5.5`, `opencode · moonshotai/kimi-k2`. */
export const agentModel = (agent: string | null | undefined, model: string) => `${agentName(agent)} · ${modelName(model, agent)}`;

/** Reasoning efforts each agent takes ('' is the agent's own default); opencode has none. */
export const EFFORTS: Record<AgentId, { id: string; name: string; hint?: string }[]> = {
	claude: [
		{ id: 'low', name: 'Low' },
		{ id: 'medium', name: 'Medium' },
		{ id: 'high', name: 'High' },
		{ id: 'xhigh', name: 'Extra High' },
		{ id: 'max', name: 'Max' },
		{ id: 'ultrathink', name: 'Ultrathink', hint: 'Deep thinking on this message' },
		{ id: 'ultracode', name: 'Ultracode', hint: 'xhigh effort plus multi-agent workflow orchestration' }
	],
	codex: [
		{ id: 'minimal', name: 'Minimal' },
		{ id: 'low', name: 'Low' },
		{ id: 'medium', name: 'Medium' },
		{ id: 'high', name: 'High' },
		{ id: 'xhigh', name: 'Extra High' }
	],
	opencode: []
};

export const effortValid = (agent: AgentId, effort: string) => !effort || EFFORTS[agent].some((e) => e.id === effort);

export const effortName = (agent: string | null | undefined, effort: string | null | undefined) =>
	EFFORTS[(agent ?? 'claude') as AgentId]?.find((e) => e.id === effort)?.name ?? effort ?? '';

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
	{ id: 'plan', name: 'Read only', hint: 'Looks and answers, changes nothing' },
	{ id: 'ask', name: 'Ask me', hint: 'Edits files; asks you here before commands and anything else' }
] as const;

/** The modes an agent offers: only Claude Code can ask you yet. */
export const permsFor = (agent: AgentId) => PERMISSIONS.filter((p) => p.id !== 'ask' || agent === 'claude');

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

/** A time ahead: `in 7 days`, `in 5 hours`, `in 20 minutes`, `soon`. */
export function until(iso: string | null | undefined, now = Date.now()): string {
	if (!iso) return '';
	const s = (new Date(iso).getTime() - now) / 1000;
	const n = (v: number, unit: string) => `in ${v} ${unit}${v === 1 ? '' : 's'}`;
	if (s < 60) return 'soon';
	if (s < 3600) return n(Math.round(s / 60), 'minute');
	if (s < 86400) return n(Math.round(s / 3600), 'hour');
	return n(Math.round(s / 86400), 'day');
}

/** The compact form for tight spaces: `now`, `5m`, `2h`, `3d`, then the date. */
export function ago(iso: string | null | undefined, now = Date.now()): string {
	if (!iso) return '';
	const t = new Date(iso).getTime(), s = Math.max(0, (now - t) / 1000);
	if (s < 60) return 'now';
	if (s < 3600) return Math.floor(s / 60) + 'm';
	if (s < 86400) return Math.floor(s / 3600) + 'h';
	if (s < 7 * 86400) return Math.floor(s / 86400) + 'd';
	return new Date(t).toLocaleDateString([], { month: 'short', day: 'numeric' });
}

/** Which calendar bucket a time falls in, by the viewer's local days. */
export function dayGroup(iso: string, now = Date.now()): 'Today' | 'Yesterday' | 'Previous 7 days' | 'Older' {
	const midnight = new Date(now).setHours(0, 0, 0, 0), t = new Date(iso).getTime();
	if (t >= midnight) return 'Today';
	if (t >= midnight - 86400000) return 'Yesterday';
	if (t >= midnight - 7 * 86400000) return 'Previous 7 days';
	return 'Older';
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
