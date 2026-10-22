import { api } from './api';
import { followRun } from './sse';
import { fleet } from './state.svelte';
import type { Run, RunEvent } from './types';
import { agentName } from './format';
import type { DiffSet } from './diff/parse';

export type Item =
	| { kind: 'text'; key: string; text: string; at: string }
	| {
			kind: 'tool';
			key: string;
			id: string;
			name: string;
			summary: string;
			output: string | null;
			isError: boolean;
			truncated: boolean;
			at: string;
	  }
	| { kind: 'note'; key: string; tone: 'muted' | 'err' | 'ok'; text: string; at: string }
	| { kind: 'stderr'; key: string; text: string; at: string };

export interface Turn {
	run: Run;
	items: Item[];
	/** Shown above the narration: where and how it ran. */
	machine: string | null;
	branch: string | null;
	workspaceNote: string | null;
	command: string | null;
	sessionModel: string | null;
	/** What the agent is doing right now, while it runs. */
	activity: string | null;
	result: {
		ok: boolean;
		text: string;
		subtype: string;
		durationMs: number | null;
		turns: number | null;
		cost: number | null;
		/** Tokens used, for agents that report no cost (Codex). */
		tokens: number | null;
	} | null;
	lastText: string;
	reconnecting: boolean;
	/** What this turn changed, and everything the thread has changed so far (null: same as `turn`). */
	diff: { turn: DiffSet; total: DiffSet | null } | null;
}

function newTurn(run: Run): Turn {
	return {
		run,
		items: [],
		machine: run.worker_id,
		branch: null,
		workspaceNote: null,
		command: null,
		sessionModel: null,
		activity: run.status === 'queued' ? 'queued' : null,
		result: null,
		lastText: '',
		reconnecting: false,
		diff: null
	};
}

/** A conversation: the root run plus its follow-ups, each streamed into a Turn. */
export class Thread {
	runs = $state<Run[]>([]);
	turns = $state<Turn[]>([]);
	error = $state<string | null>(null);
	#abort = new AbortController();

	root = $derived(this.runs[0] ?? null);
	latest = $derived(this.runs[this.runs.length - 1] ?? null);
	live = $derived(!!this.latest && (this.latest.status === 'queued' || this.latest.status === 'running'));
	branch = $derived(this.turns.find((t) => t.branch)?.branch ?? null);
	machine = $derived(this.turns.findLast((t) => t.machine)?.machine ?? null);
	hasSession = $derived(this.runs.some((r) => !!r.session_id));
	/** Everything the thread has changed: the latest turn that changed anything knows the total. */
	changes = $derived.by(() => {
		const d = this.turns.findLast((t) => t.diff)?.diff;
		return d ? (d.total ?? d.turn) : null;
	});

	async load(id: string) {
		try {
			this.runs = await api<Run[]>(`/v1/runs/${id}/thread`);
		} catch (e) {
			this.error = (e as Error).message;
			return;
		}
		this.turns = this.runs.map(newTurn);
		// Replay every turn in order; only the last can still be live.
		for (let i = 0; i < this.runs.length; i++) {
			const p = this.#follow(i);
			if (i < this.runs.length - 1) await p;
		}
	}

	/** Queue a follow-up message on the same session. */
	async reply(text: string, permission?: string) {
		const root = this.root!;
		const body: Record<string, string> = { task: text };
		if (permission) body.permission_mode = permission;
		const r = await api<{ id: string }>(`/v1/runs/${root.id}/messages`, { method: 'POST', body: JSON.stringify(body) });
		const run = await api<Run>(`/v1/runs/${r.id}`);
		this.runs.push(run);
		this.turns.push(newTurn(run));
		void this.#follow(this.runs.length - 1);
	}

	async stop() {
		const l = this.latest;
		if (l) await api(`/v1/runs/${l.id}/stop`, { method: 'POST' });
	}

	dispose() {
		this.#abort.abort();
	}

	#follow(i: number) {
		const turn = this.turns[i];
		const live = this.runs[i].status === 'queued' || this.runs[i].status === 'running';
		return followRun(
			this.runs[i].id,
			(ev) => {
				turn.reconnecting = false;
				apply(turn, this.runs[i], ev);
			},
			this.#abort.signal,
			() => (turn.reconnecting = true)
		).then(() => {
			// A run that just finished changes the lists and machine slots: refresh now.
			if (live && !this.#abort.signal.aborted) void fleet.refresh();
		});
	}
}

const verbs: Record<string, string> = {
	Bash: 'Ran', Read: 'Read', Grep: 'Searched', Glob: 'Listed', Edit: 'Edited', Write: 'Wrote',
	MultiEdit: 'Edited', NotebookEdit: 'Edited', WebFetch: 'Fetched', WebSearch: 'Searched the web for',
	Agent: 'Delegated', Task: 'Delegated', TodoWrite: 'Updated the plan'
};
export const toolVerb = (name: string) => verbs[name] ?? 'Used ' + name;

function apply(t: Turn, run: Run, ev: RunEvent) {
	const p = ev.payload ?? {};
	const key = run.id + ':' + ev.seq;
	switch (ev.kind) {
		case 'status': {
			run.status = p.status;
			if (p.status === 'queued') t.activity = 'queued';
			else if (p.status === 'running') {
				run.started_at ??= ev.at;
				run.worker_id = p.worker_id ?? run.worker_id;
				t.machine = run.worker_id;
				t.activity = 'Starting ' + agentName(run.agent);
			} else {
				run.finished_at ??= ev.at;
				t.activity = null;
				if (p.status !== 'succeeded' || p.reason)
					t.items.push({
						kind: 'note', key, at: ev.at,
						tone: p.status === 'succeeded' ? 'ok' : 'err',
						text: p.status === 'stopped' ? 'Stopped' + (p.reason ? `: ${p.reason}` : '') : p.status === 'failed' ? 'Failed' + (p.reason ? `: ${p.reason}` : '') : 'Done'
					});
			}
			break;
		}
		case 'workspace':
			if (p.mode === 'worktree') t.branch = p.branch;
			else t.workspaceNote = p.note ?? 'in the checkout';
			break;
		case 'started':
			t.command = p.command;
			t.activity = 'Waiting for the first message';
			break;
		case 'agent_init':
			t.sessionModel = p.model;
			run.session_id = p.session_id ?? run.session_id;
			t.activity = 'Thinking';
			break;
		case 'assistant_thinking':
			t.activity = 'Thinking';
			break;
		case 'assistant_text':
			t.lastText = p.text ?? '';
			t.items.push({ kind: 'text', key, text: p.text ?? '', at: ev.at });
			t.activity = 'Working';
			break;
		case 'tool_use':
			t.items.push({
				kind: 'tool', key, id: p.id, name: p.name, summary: p.summary ?? '',
				output: null, isError: false, truncated: false, at: ev.at
			});
			t.activity = (verbs[p.name] ?? 'Using ' + p.name).replace(/ for$/, '') + '…';
			break;
		case 'tool_result': {
			const tool = t.items.findLast((x) => x.kind === 'tool' && x.id === p.tool_use_id);
			if (tool && tool.kind === 'tool') {
				tool.output = p.content ?? '';
				tool.isError = !!p.is_error;
				tool.truncated = !!p.truncated;
			}
			t.activity = 'Thinking';
			break;
		}
		case 'result':
			run.session_id = p.session_id ?? run.session_id;
			t.result = {
				ok: !p.is_error,
				text: p.result && p.result !== t.lastText ? p.result : '',
				subtype: p.subtype ?? '',
				durationMs: p.duration_ms ?? null,
				turns: p.num_turns ?? null,
				cost: p.total_cost_usd ?? null,
				tokens: tokenCount(p.usage)
			};
			t.activity = null;
			break;
		case 'diff':
			if (p.turn) t.diff = { turn: p.turn, total: p.total ?? null };
			break;
		case 'stop_requested':
			t.activity = 'Stopping';
			break;
		case 'stopped':
			break;
		case 'stderr':
			t.items.push({ kind: 'stderr', key, text: p.text ?? '', at: ev.at });
			break;
		case 'error':
			t.items.push({ kind: 'note', key, tone: 'err', text: p.message ?? 'Error', at: ev.at });
			t.activity = null;
			break;
	}
}

/** Input plus output tokens from Codex or opencode usage. */
function tokenCount(u: Record<string, any> | null | undefined): number | null {
	if (!u) return null;
	const n = (Number(u.input) || 0) + (Number(u.output) || 0);
	return n || null;
}
