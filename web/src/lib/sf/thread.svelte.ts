import { api } from './api';
import { followRun } from './sse';
import { fleet } from './state.svelte';
import type { AgentId, ApprovalStatus, Run, RunEvent } from './types';
import { agentName } from './format';
import type { DiffSet } from './diff/parse';

/** An image from a tool result, stored on the control plane (`GET /v1/images/:id`). */
export interface ImageRef {
	id: string;
	media_type: string;
	bytes: number;
}

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
			images: ImageRef[];
			at: string;
	  }
	| {
			/** A tool call waiting for (or given) your decision ("Ask me"). */
			kind: 'approval';
			key: string;
			id: string;
			runId: string;
			name: string;
			summary: string;
			input: Record<string, any>;
			status: ApprovalStatus;
			message: string | null;
			decidedBy: string | null;
			expiresAt: string;
			at: string;
	  }
	| { kind: 'note'; key: string; tone: 'muted' | 'err' | 'ok'; text: string; at: string; href?: string }
	| { kind: 'stderr'; key: string; text: string; at: string };

export interface Turn {
	run: Run;
	items: Item[];
	/** Shown above the narration: where and how it ran. */
	machine: string | null;
	branch: string | null;
	workspaceNote: string | null;
	/** Where a new task branch started, e.g. origin/main at a commit. */
	startedFrom: { ref: string; commit: string } | null;
	/** Latest checkpoint pushed to origin (failover runs). */
	checkpoint: string | null;
	/** For a moved run: the machine that dropped it. */
	movedFrom: string | null;
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
	/** The text the agent is writing right now, until its final message replaces it. Never stored. */
	liveText: string;
	reconnecting: boolean;
	/** What this turn changed, and everything the thread has changed so far (null: same as `turn`). */
	diff: { turn: DiffSet; total: DiffSet | null } | null;
	/** Snapshots (commits) of the files before and after this turn, to restore to or fork from. */
	snapshot: { before: string; after: string } | null;
}

function newTurn(run: Run): Turn {
	return {
		run,
		items: [],
		machine: run.worker_id,
		branch: null,
		workspaceNote: null,
		startedFrom: null,
		checkpoint: null,
		movedFrom: null,
		command: null,
		sessionModel: null,
		activity: run.status === 'queued' ? 'queued' : null,
		result: null,
		lastText: '',
		liveText: '',
		reconnecting: false,
		diff: null,
		snapshot: null
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
	/** The message being worked on: the running one, else the latest (messages can wait behind it). */
	current = $derived(this.runs.findLast((r) => r.status === 'running') ?? this.latest);
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
		// Replay finished turns in order; live ones (running, or queued behind it) stream side by side.
		for (let i = 0; i < this.runs.length; i++) {
			const p = this.#follow(i);
			if (isTerminal(this.runs[i])) await p;
		}
	}

	/**
	 * Send a follow-up on the same session. While a message runs, it waits behind it.
	 * With another agent the thread switches to it: it starts its own session with the conversation so far.
	 */
	async reply(text: string, permission?: string, images: { media_type: string; data: string }[] = [], agent?: { agent: AgentId; model: string }, effort?: string) {
		const root = this.root!;
		const body: Record<string, unknown> = { task: text };
		if (permission) body.permission_mode = permission;
		// '' goes back to the agent's default; absent keeps the thread's.
		if (effort !== undefined) body.effort = effort;
		if (agent) Object.assign(body, agent);
		if (images.length) body.images = images;
		const r = await api<{ id: string }>(`/v1/runs/${root.id}/messages`, { method: 'POST', body: JSON.stringify(body) });
		const run = await api<Run>(`/v1/runs/${r.id}`);
		this.runs.push(run);
		this.turns.push(newTurn(run));
		void this.#follow(this.runs.length - 1);
	}

	/** Put the files back to how they were before `runId` started; the session carries on. */
	async restore(runId: string) {
		await this.#action(`/v1/runs/${runId}/restore`);
	}

	/** Push the branch and open a pull request for it (or update the open one). */
	async openPr() {
		await this.#action(`/v1/runs/${this.root!.id}/pr`);
	}

	async #action(path: string) {
		const r = await api<{ id: string }>(path, { method: 'POST' });
		const run = await api<Run>(`/v1/runs/${r.id}`);
		this.runs.push(run);
		this.turns.push(newTurn(run));
		void this.#follow(this.runs.length - 1);
	}

	/** Stop everything: drop the waiting messages first so none starts once the running one ends. */
	async stop() {
		const pending = this.runs.filter((r) => !isTerminal(r)).reverse();
		for (const r of pending) await api(`/v1/runs/${r.id}/stop`, { method: 'POST' });
	}

	/** Send now: stop the running message so the waiting ones go next, in the same session. */
	async interrupt() {
		for (const r of this.runs.filter((r) => r.status === 'running'))
			await api(`/v1/runs/${r.id}/stop`, { method: 'POST' });
	}

	/** Drop one waiting message. */
	async cancel(id: string) {
		await api(`/v1/runs/${id}/stop`, { method: 'POST' });
	}

	/** Approve or deny a tool call the agent is waiting on. The event stream confirms it. */
	async decide(item: Extract<Item, { kind: 'approval' }>, approve: boolean) {
		const a = await api<{ status: ApprovalStatus; decided_by: string | null; message: string | null }>(
			`/v1/runs/${item.runId}/approvals/${item.id}/${approve ? 'approve' : 'deny'}`,
			{ method: 'POST' }
		);
		item.status = a.status;
		item.decidedBy = a.decided_by;
		item.message = a.message;
	}

	/** Whether this run is a message waiting for an earlier one to finish. */
	waiting(id: string) {
		const i = this.runs.findIndex((r) => r.id === id);
		return i > 0 && this.runs[i].status === 'queued' && this.runs.slice(0, i).some((r) => !isTerminal(r));
	}

	dispose() {
		this.#abort.abort();
	}

	async #adopt(id: string) {
		if (this.runs.some((r) => r.id === id)) return;
		const run = await api<Run>(`/v1/runs/${id}`);
		if (this.#abort.signal.aborted || this.runs.some((r) => r.id === id)) return;
		this.runs.push(run);
		this.turns.push(newTurn(run));
		void this.#follow(this.runs.length - 1);
	}

	#follow(i: number) {
		const turn = this.turns[i];
		const live = !isTerminal(this.runs[i]);
		return followRun(
			this.runs[i].id,
			(ev) => {
				turn.reconnecting = false;
				apply(turn, this.runs[i], ev);
				// The machine dropped and the run was queued again elsewhere: follow the new run too.
				if (ev.kind === 'status' && ev.payload?.moved_to) void this.#adopt(ev.payload.moved_to);
				if (ev.kind === 'pull_request' && this.root && ev.payload?.url) {
					const { url, number, state } = ev.payload;
					this.root.pr = { url, number, state };
				}
			},
			this.#abort.signal,
			() => (turn.reconnecting = true)
		).then(() => {
			// A run that just finished changes the lists and machine slots: refresh now.
			if (live && !this.#abort.signal.aborted) void fleet.refresh();
		});
	}
}

const isTerminal = (r: Run) => r.status !== 'queued' && r.status !== 'running';

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
			if (p.moved_to) {
				run.finished_at ??= ev.at;
				t.activity = null;
				t.items.push({ kind: 'note', key, at: ev.at, tone: 'muted', text: `${p.lost_worker ?? t.machine ?? 'The machine'} stopped responding. Moving the task to another machine.` });
				break;
			}
			if (p.status === 'queued') {
				t.activity = 'queued';
				if (p.moved_from) t.movedFrom = p.lost_worker ?? null;
			}
			else if (p.status === 'running') {
				run.started_at ??= ev.at;
				run.worker_id = p.worker_id ?? run.worker_id;
				t.machine = run.worker_id;
				t.activity = run.kind === 'restore' ? 'Restoring the files' : run.kind === 'pr' ? 'Pushing the branch and opening a pull request' : 'Starting ' + agentName(run.agent);
			} else {
				run.finished_at ??= ev.at;
				t.activity = null;
				t.liveText = '';
				// Nobody can answer the agent any more.
				for (const it of t.items) if (it.kind === 'approval' && it.status === 'pending') it.status = 'cancelled';
				if (p.status !== 'succeeded' || p.reason)
					t.items.push({
						kind: 'note', key, at: ev.at,
						tone: p.status === 'succeeded' ? 'ok' : 'err',
						text: p.status === 'stopped' ? 'Stopped' + (p.reason ? `: ${p.reason}` : '') : p.status === 'failed' ? 'Failed' + (p.reason ? `: ${p.reason}` : '') : 'Done'
					});
			}
			break;
		}
		case 'checkpoint':
			if (p.sha) t.checkpoint = p.sha;
			else if (p.error) t.items.push({ kind: 'note', key, tone: 'err', text: `Could not push a checkpoint: ${p.error}`, at: ev.at });
			break;
		case 'workspace':
			if (p.mode === 'worktree') {
				t.branch = p.branch;
				t.startedFrom = p.from ?? null;
			}
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
		case 'assistant_live':
			t.liveText = p.text ?? '';
			break;
		case 'assistant_text':
			t.liveText = '';
			t.lastText = p.text ?? '';
			t.items.push({ kind: 'text', key, text: p.text ?? '', at: ev.at });
			t.activity = 'Working';
			break;
		case 'tool_use':
			t.liveText = '';
			t.items.push({
				kind: 'tool', key, id: p.id, name: p.name, summary: p.summary ?? '',
				output: null, isError: false, truncated: false, images: [], at: ev.at
			});
			t.activity = (verbs[p.name] ?? 'Using ' + p.name).replace(/ for$/, '') + '…';
			break;
		case 'tool_result': {
			const tool = t.items.findLast((x) => x.kind === 'tool' && x.id === p.tool_use_id);
			if (tool && tool.kind === 'tool') {
				tool.output = p.content ?? '';
				tool.isError = !!p.is_error;
				tool.truncated = !!p.truncated;
				tool.images = p.images ?? [];
			}
			t.activity = 'Thinking';
			break;
		}
		case 'approval_requested':
			t.items.push({
				kind: 'approval', key, id: p.id, runId: run.id, name: p.tool_name, summary: p.summary ?? '',
				input: p.input ?? {}, status: p.status ?? 'pending', message: null, decidedBy: null, expiresAt: p.expires_at, at: ev.at
			});
			t.activity = 'Waiting for you to approve';
			break;
		case 'approval_decided': {
			const a = t.items.find((x) => x.kind === 'approval' && x.id === p.id);
			if (a && a.kind === 'approval') {
				a.status = p.status;
				a.message = p.message ?? null;
				a.decidedBy = p.decided_by ?? null;
			}
			if (!t.items.some((x) => x.kind === 'approval' && x.status === 'pending')) t.activity = 'Thinking';
			break;
		}
		case 'result':
			t.liveText = '';
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
		case 'snapshot':
			if (p.before && p.after) t.snapshot = { before: p.before, after: p.after };
			break;
		case 'restored':
			t.items.push({ kind: 'note', key, tone: 'ok', text: 'Restored the files', at: ev.at });
			t.activity = null;
			break;
		case 'pull_request':
			t.items.push({ kind: 'note', key, tone: 'ok', text: `${p.opened ? 'Opened' : 'Pushed to'} pull request #${p.number}`, href: p.url, at: ev.at });
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
