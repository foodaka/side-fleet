// The review desk (`#/review`): your finished tasks that need a decision, in the order to
// take them, and what you've decided so far. Built from the task list and the inbox the page
// already follows; what you did on the desk is kept in this browser, per person.

import { fleet } from '../state.svelte';
import { inbox } from '../inbox.svelte';
import type { RunSummary } from '../types';
import type { Note } from './notes';

/** What you did with a task on the desk. New activity in the task brings it back. */
interface Decision {
	kind: 'merged' | 'later' | 'stopped';
	at: string;
}

/** Notes sent back to the agent, and when: the task returns as the next round. */
export interface Round {
	n: number;
	notes: Note[];
	sentAt: string;
	/** The flags raised when you sent them, to say which the new commits made go away. */
	flags: { id: string; text: string }[];
}

interface Saved {
	decided: Record<string, Decision>;
	/** `<task>:<flag>`: flags you marked as looked at. */
	seen: Record<string, true>;
	/** Notes written but not sent, per task. */
	drafts: Record<string, Note[]>;
	rounds: Record<string, Round>;
}

export type Section = 'approval' | 'merge' | 'finished' | 'stopped';

export interface DeskItem {
	run: RunSummary;
	section: Section;
	round: Round | null;
	/** Back from your notes: the round is ready to look at. */
	back: boolean;
}

const blank = (): Saved => ({ decided: {}, seen: {}, drafts: {}, rounds: {} });
const terminal = (r: RunSummary) => r.thread_status === 'succeeded' || r.thread_status === 'failed' || r.thread_status === 'stopped';
const after = (a: string, b: string) => new Date(a).getTime() > new Date(b).getTime();

class Review {
	state = $state<Saved>(blank());
	#key = '';

	/** Load what this person did on the desk (once their identity is known). */
	sync() {
		const key = `sidefleet_review:${fleet.org?.id ?? '-'}:${fleet.userId ?? 'dev'}`;
		if (key === this.#key) return;
		this.#key = key;
		try {
			this.state = { ...blank(), ...JSON.parse(localStorage.getItem(key) ?? '{}') };
		} catch {
			this.state = blank();
		}
	}

	save() {
		if (!this.#key) return;
		try { localStorage.setItem(this.#key, JSON.stringify(this.state)); } catch { /* storage full or blocked */ }
	}

	/** Your tasks, by what they need from you. */
	items = $derived.by((): DeskItem[] => {
		const out: DeskItem[] = [];
		for (const r of fleet.runs) {
			if (r.archived_at || r.room_id) continue;
			if (fleet.userId && r.created_by && r.created_by !== fleet.userId) continue;
			if (r.snoozed_until && new Date(r.snoozed_until).getTime() > fleet.now) continue;
			const round = this.state.rounds[r.id] ?? null;
			const away = !!round && (!terminal(r) || !after(r.last_activity_at, round.sentAt));
			const back = !!round && !away;
			const decided = this.state.decided[r.id];
			if (decided && !after(r.last_activity_at, decided.at)) continue;
			const reason = inbox.byId.get(r.id)?.reason;
			let section: Section | null = null;
			if (r.pending_approvals || reason === 'approval_needed') section = 'approval';
			else if (away || !terminal(r)) continue;
			else if (r.pr?.state === 'open') section = 'merge';
			else if (r.pr) continue;
			else if (reason === 'failed' || reason === 'usage_limit' || reason === 'machine_lost') section = 'stopped';
			else if (reason === 'finished' || back) section = r.thread_status === 'succeeded' ? 'finished' : 'stopped';
			if (section) out.push({ run: r, section, round, back });
		}
		return out;
	});

	/** Tasks working on notes you sent from the desk. */
	away = $derived(fleet.runs.filter((r) => {
		const round = this.state.rounds[r.id];
		return !!round && !r.archived_at && (!terminal(r) || !after(r.last_activity_at, round.sentAt)) && !this.state.decided[r.id];
	}));

	/**
	 * The order to take them in, which Next follows too: approvals first, since the agent is
	 * stopped on them; then work back from your notes, while it's fresh; then PRs with flags
	 * (the page works those out once open, so here: failing checks or conflicts); then the rest.
	 */
	queue = $derived.by(() => {
		const rank = (i: DeskItem) =>
			i.section === 'approval' ? 0
			: i.back ? 1
			: i.section === 'merge' && (i.run.pr?.checks?.failed || i.run.pr?.conflicts) ? 2
			: i.section === 'merge' ? 3
			: i.section === 'finished' ? 4
			: 5;
		return [...this.items].sort((a, b) => rank(a) - rank(b) || (b.run.last_activity_at > a.run.last_activity_at ? 1 : -1));
	});

	count = $derived(this.items.length);

	/** Tasks you left for later; new activity brings them back by itself. */
	later = $derived(fleet.runs.filter((r) => {
		const d = this.state.decided[r.id];
		return d?.kind === 'later' && !r.archived_at && !after(r.last_activity_at, d.at);
	}));

	bringBack(id: string) {
		delete this.state.decided[id];
		this.save();
	}

	decide(id: string, kind: Decision['kind']) {
		this.state.decided[id] = { kind, at: new Date().toISOString() };
		delete this.state.rounds[id];
		void inbox.dismiss(id).catch(() => {});
		this.save();
	}

	/** Notes are on their way: the task leaves the desk until it's back as the next round. */
	sent(id: string, notes: Note[], flags: { id: string; text: string; file?: string }[]) {
		const prev = this.state.rounds[id];
		this.state.rounds[id] = { n: (prev?.n ?? 1) + 1, notes, sentAt: new Date().toISOString(), flags: flags.map((f) => ({ id: f.id, text: f.text })) };
		// A note on a flagged file means you looked at that flag: it doesn't come back next round.
		for (const f of flags) if (f.file && notes.some((n) => n.file === f.file)) this.state.seen[`${id}:${f.id}`] = true;
		this.state.drafts[id] = [];
		delete this.state.decided[id];
		void inbox.dismiss(id).catch(() => {});
		this.save();
	}

	drafts(id: string): Note[] {
		return this.state.drafts[id] ?? [];
	}

	addNote(id: string, note: Note) {
		// Assign first, then push to the stored array: pushing to a fresh one would be lost.
		if (!this.state.drafts[id]) this.state.drafts[id] = [];
		this.state.drafts[id].push(note);
		this.save();
	}

	dropNote(id: string, index: number) {
		this.state.drafts[id]?.splice(index, 1);
		this.save();
	}

	isSeen(id: string, flag: string) {
		return !!this.state.seen[`${id}:${flag}`];
	}

	toggleSeen(id: string, flag: string) {
		const k = `${id}:${flag}`;
		if (this.state.seen[k]) delete this.state.seen[k];
		else this.state.seen[k] = true;
		this.save();
	}
}

export const review = new Review();
