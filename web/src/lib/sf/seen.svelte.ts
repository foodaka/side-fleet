import type { RunSummary } from './types';

// Which finished runs you have looked at since they last changed, so the sidebar
// can flag the ones that finished while you were elsewhere. Lives in this browser
// only. The first visit marks everything as seen, so nothing starts out unread.

const KEY = 'sidefleet_seen';
const KEEP = 300;

type Stored = { since: number | null; runs: Record<string, number> };

const time = (iso: string) => new Date(iso).getTime();

class Seen {
	#since = $state<number | null>(null);
	#runs = $state<Record<string, number>>({});

	constructor() {
		try {
			const s = JSON.parse(localStorage.getItem(KEY) ?? 'null') as Stored | null;
			if (s && typeof s.since === 'number') {
				this.#since = s.since;
				this.#runs = s.runs ?? {};
			}
		} catch {
			/* storage blocked or bad JSON: start fresh */
		}
	}

	/** Call once runs have loaded: on a first visit, everything so far counts as seen. */
	init(runs: RunSummary[]) {
		if (this.#since != null) return;
		this.#since = Math.max(Date.now(), ...runs.map((r) => time(r.last_activity_at)));
		this.#save();
	}

	/** Finished, and changed since you last opened it. */
	unread(r: RunSummary): boolean {
		if (this.#since == null || r.thread_status === 'running' || r.thread_status === 'queued') return false;
		return time(r.last_activity_at) > (this.#runs[r.id] ?? this.#since);
	}

	mark(r: RunSummary) {
		const t = time(r.last_activity_at);
		if ((this.#runs[r.id] ?? 0) >= t) return;
		this.#runs[r.id] = t;
		const ids = Object.keys(this.#runs);
		if (ids.length > KEEP) {
			ids.sort((a, b) => this.#runs[a] - this.#runs[b]).slice(0, ids.length - KEEP).forEach((id) => delete this.#runs[id]);
		}
		this.#save();
	}

	#save() {
		try {
			localStorage.setItem(KEY, JSON.stringify({ since: this.#since, runs: this.#runs } satisfies Stored));
		} catch {
			/* private mode: unread marks last for this tab only */
		}
	}
}

export const seen = new Seen();
