import { api, getKey, keyWorks, setKey, Unauthorized } from './api';
import type { RunSummary, Setup, Worker } from './types';
import { telemetry } from './telemetry.svelte';
import { repoKey } from './format';

type Conn = 'connecting' | 'ok' | 'unreachable' | 'unauthorized';

/** Everything the pages share, refreshed from the control plane every 5 seconds. */
class Fleet {
	workers = $state<Worker[]>([]);
	runs = $state<RunSummary[]>([]);
	/** Repos chosen for the composer menu; null until loaded. */
	added = $state<Set<string> | null>(null);
	setup = $state<Setup | null>(null);
	conn = $state<Conn>('connecting');
	loaded = $state(false);
	/** Ticks every second so relative times and durations stay fresh. */
	now = $state(Date.now());
	pickerOpen = $state(false);
	addMachineOpen = $state(false);

	online = $derived(this.workers.filter((w) => w.online));
	needsKey = $derived(this.conn === 'unauthorized');

	#timer: ReturnType<typeof setInterval> | undefined;

	start() {
		if (this.#timer) return;
		if (!getKey()) this.conn = 'unauthorized';
		else void this.refresh();
		this.#timer = setInterval(() => void this.refresh(), 5000);
		setInterval(() => (this.now = Date.now()), 1000);
		// Background tabs throttle timers; catch up the moment the tab is visible again.
		document.addEventListener('visibilitychange', () => {
			if (document.visibilityState === 'visible') {
				this.now = Date.now();
				void this.refresh();
			}
		});
	}

	async refresh() {
		if (this.conn === 'unauthorized' && !getKey()) return;
		try {
			const [workers, runs, repos] = await Promise.all([
				api<Worker[]>('/v1/workers'),
				api<RunSummary[]>('/v1/runs?limit=100'),
				api<{ added: string[] }>('/v1/repos')
			]);
			this.workers = workers;
			this.runs = runs;
			telemetry.ingest(workers, runs);
			this.added = new Set(repos.added);
			void this.#migrateAdded(workers, repos.added);
			if (!this.setup) this.setup = await api<Setup>('/v1/setup');
			this.conn = 'ok';
			this.loaded = true;
		} catch (e) {
			this.conn = e instanceof Unauthorized ? 'unauthorized' : 'unreachable';
		}
	}

	async signIn(key: string): Promise<boolean> {
		if (!(await keyWorks(key))) return false;
		setKey(key);
		this.conn = 'connecting';
		this.setup = null;
		await this.refresh();
		return true;
	}

	signOut() {
		setKey('');
		this.conn = 'unauthorized';
	}

	/** Every repo any machine reports, name -> machines (online ones first). */
	allRepos = $derived.by(() => {
		const m = new Map<string, Worker[]>();
		[...this.workers]
			.sort((a, b) => Number(b.online) - Number(a.online))
			.forEach((w) =>
				(w.capabilities.repos ?? []).forEach((r) => {
					const k = repoKey(r);
					const list = m.get(k) ?? [];
					if (!list.includes(w)) list.push(w); // a machine can hold several clones
					m.set(k, list);
				})
			);
		return m;
	});

	onlineRepos = $derived(new Set(this.online.flatMap((w) => (w.capabilities.repos ?? []).map(repoKey))));

	/** What the composer's repo menu offers: the chosen repos, or all of them when there are only a few. */
	composerRepos = $derived.by(() => {
		const FEW = 8;
		const picked =
			this.added && this.added.size ? [...this.added] : this.onlineRepos.size <= FEW ? [...this.onlineRepos] : [];
		return picked.sort((a, b) => a.localeCompare(b));
	});

	workerFor(repo: string) {
		return this.online.find((w) => (w.capabilities.repos ?? []).some((r) => repoKey(r) === repo || r.name === repo));
	}

	#migrated = false;
	/** Repos chosen before identities existed are stored by folder name; convert them once. */
	async #migrateAdded(workers: Worker[], added: string[]) {
		if (this.#migrated || !added.length) return;
		const byName = new Map<string, string>();
		workers.forEach((w) => (w.capabilities.repos ?? []).forEach((r) => { if (r.id && r.id !== r.name) byName.set(r.name, r.id); }));
		if (!byName.size) return; // workers have not reported identities yet
		this.#migrated = true;
		const next = [...new Set(added.map((n) => byName.get(n) ?? n))];
		if (next.length !== added.length || next.some((n, i) => n !== added[i])) await this.saveAdded(next);
	}

	async saveAdded(names: string[]) {
		const r = await api<{ added: string[] }>('/v1/repos', { method: 'PUT', body: JSON.stringify({ added: names }) });
		this.added = new Set(r.added);
		return r.added;
	}
}

export const fleet = new Fleet();

const INSTALL = 'curl -fsSL https://raw.githubusercontent.com/foodaka/side-fleet/main/install.sh | sh';
export const installCmd = INSTALL;

export function connectCmd(masked: boolean) {
	const t = fleet.setup?.worker_token;
	const tok = t ? (masked ? t.slice(0, 4) + '••••••••' : t) : '<worker token>';
	return `sidefleet connect --control ${location.origin} --token ${tok}`;
}

export function agentPrompt(masked: boolean) {
	const t = fleet.setup?.worker_token;
	const tok = t ? (masked ? t.slice(0, 4) + '••••••••' : t) : '<worker token>';
	return `Read ${location.origin}/connect.md and follow it to connect this machine to Sidefleet. Control plane: ${location.origin}. Worker token: ${tok} (keep it out of logs and your summary).`;
}
