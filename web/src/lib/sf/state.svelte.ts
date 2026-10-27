import { api, ApiError, currentKey, currentOrg, keyWorks, setKey, setOrg, Unauthorized } from './api';
import type { AuthMethods, MyMachine, Enrollment, Me, Member, RunSummary, Setup, Worker } from './types';
import { telemetry } from './telemetry.svelte';
import { toast } from 'svelte-sonner';
import { repoKey } from './format';

type Conn = 'connecting' | 'ok' | 'unreachable' | 'unauthorized';

/** Everything the pages share, refreshed from the control plane every 5 seconds. */
class Fleet {
	workers = $state<Worker[]>([]);
	/** Your machines in all your orgs. */
	myMachines = $state<MyMachine[]>([]);
	/** Settings' "Your machines" dialog. */
	machinesOpen = $state(false);
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
	/** Archived threads; null until the sidebar's archived list is first opened. */
	archived = $state<RunSummary[] | null>(null);
	/** A thread whose page should open the fork dialog for its latest message (asked from the sidebar). */
	forkThread = $state<string | null>(null);
	newOrgOpen = $state(false);
	feedbackOpen = $state(false);
	/** How this control plane lets people in; null until loaded. */
	methods = $state<AuthMethods | null>(null);
	/** Who is signed in and their orgs. */
	me = $state<Me | null>(null);
	/** Machines waiting for someone in this org to approve them. */
	pending = $state<Enrollment[]>([]);
	orgId = $state(currentOrg());
	/** People in the org the page acts in (empty with the developer key). */
	members = $state<Member[]>([]);
	#membersFor = '';
	#membersAt = 0;

	online = $derived(this.workers.filter((w) => w.online));
	/** Machines that run your tasks: each machine only runs its owner's. */
	mine = $derived(this.workers.filter((w) => w.mine !== false));
	myOnline = $derived(this.mine.filter((w) => w.online));
	/** The signed-in user's id; null with the developer key. */
	userId = $derived(this.me?.user?.id ?? null);
	/** Several people share this org: show who started what. */
	multi = $derived(this.members.length > 1);
	memberLogin = $derived(new Map(this.members.map((m) => [m.id, m.login])));
	needsSignIn = $derived(this.conn === 'unauthorized');
	/** The org the page acts in. */
	org = $derived(this.me?.orgs.find((o) => o.id === this.orgId) ?? this.me?.orgs[0] ?? null);
	isOwner = $derived(this.org?.role === 'owner');
	/** Your machines in your other orgs, which you can move into this one. */
	elsewhere = $derived(this.myMachines.filter((m) => m.org.id !== this.org?.id));

	#timer: ReturnType<typeof setInterval> | undefined;
	#asked = false;

	start() {
		if (this.#timer) return;
		void fetch('/v1/auth')
			.then((r) => (r.ok ? r.json() : null))
			.then((m) => (this.methods = m ?? { github: false, dev_key: true, hosted: false }))
			.catch(() => (this.methods = { github: false, dev_key: true, hosted: false }));
		void this.refresh();
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

	async refresh(retry = true) {
		// Signed out: stop polling until a key is entered (GitHub sign-in reloads the page).
		if (this.conn === 'unauthorized' && this.#asked && !currentKey()) return;
		this.#asked = true;
		try {
			// Until signed in, ask once instead of firing every call into a 401.
			if (this.conn !== 'ok') await api<Me>('/v1/me');
			const [me, workers, runs, repos, pending, myMachines] = await Promise.all([
				api<Me>('/v1/me'),
				api<Worker[]>('/v1/workers'),
				api<RunSummary[]>('/v1/runs?limit=100'),
				api<{ added: string[] }>('/v1/repos'),
				api<Enrollment[]>('/v1/enrollments'),
				api<MyMachine[]>('/v1/me/machines').catch(() => [] as MyMachine[])
			]);
			this.me = me;
			this.workers = workers;
			this.myMachines = myMachines;
			this.runs = runs;
			this.pending = pending;
			telemetry.ingest(workers, runs);
			this.added = new Set(repos.added);
			void this.#migrateAdded(workers, repos.added);
			if (!this.setup) this.setup = await api<Setup>('/v1/setup');
			void this.#loadMembers();
			this.conn = 'ok';
			this.loaded = true;
		} catch (e) {
			// The remembered org is gone (left it, or another account): fall back to the first one.
			if (e instanceof ApiError && e.status === 403 && this.orgId && retry) {
				this.switchOrg('');
				return;
			}
			this.conn = e instanceof Unauthorized ? 'unauthorized' : 'unreachable';
		}
	}

	/** The org's members, refreshed when the org changes and every minute or so. */
	async #loadMembers(force = false) {
		const org = this.org;
		if (!this.me?.user || !org) {
			this.members = [];
			return;
		}
		if (!force && this.#membersFor === org.id && Date.now() - this.#membersAt < 60_000) return;
		this.#membersFor = org.id;
		this.#membersAt = Date.now();
		try {
			this.members = await api<Member[]>(`/v1/orgs/${org.id}/members`);
		} catch {
			/* keep what we had */
		}
	}

	reloadMembers() {
		return this.#loadMembers(true);
	}

	/** A user's login, if they are in this org. */
	login(id: string | null | undefined): string | null {
		return id ? (this.memberLogin.get(id) ?? null) : null;
	}

	/** Whether a thread (or message) was started by someone else; unknown authors count as yours. */
	othersRun(r: { created_by?: string | null }): boolean {
		return !!r.created_by && !!this.userId && r.created_by !== this.userId;
	}

	async loadArchived() {
		try {
			this.archived = await api<RunSummary[]>('/v1/runs?archived=true&limit=100');
		} catch (e) {
			toast.error('Could not load archived tasks', { description: (e as Error).message });
		}
	}

	/** Archive a thread (or bring it back), moving it between the lists at once; archiving offers an undo. */
	async setArchived(r: RunSummary, archived: boolean) {
		const moved = { ...r, archived_at: archived ? new Date().toISOString() : null };
		const [from, to] = archived ? ['runs', 'archived'] as const : ['archived', 'runs'] as const;
		const before = { runs: this.runs, archived: this.archived };
		if (this[from]) this[from] = this[from]!.filter((x) => x.id !== r.id);
		if (this[to]) this[to] = [moved, ...this[to]!].sort((a, b) => b.last_activity_at.localeCompare(a.last_activity_at));
		try {
			await api(`/v1/runs/${r.id}/archive`, { method: 'POST', body: JSON.stringify({ archived }) });
			if (archived) toast('Archived', { description: r.task.slice(0, 80), action: { label: 'Undo', onClick: () => void this.setArchived(moved, false) } });
		} catch (e) {
			Object.assign(this, before);
			toast.error(archived ? 'Could not archive' : 'Could not unarchive', { description: (e as Error).message });
		}
		void this.refresh();
	}

	/** Move one of your machines to another of your orgs (this one unless named); it carries on without reconnecting. */
	async moveMachine(m: MyMachine, to: string = this.org?.id ?? ''): Promise<boolean> {
		const name = this.me?.orgs.find((o) => o.id === to)?.name ?? 'this org';
		try {
			await api('/v1/me/machines/move', { method: 'POST', body: JSON.stringify({ from: m.org.id, id: m.id }), headers: { 'X-Sidefleet-Org': to } });
			toast.success(`${m.display_name} moved to ${name}`, { description: `It takes your tasks in ${name} now, not in ${m.org.name}.` });
			await this.refresh();
			return true;
		} catch (e) {
			toast.error(`Could not move ${m.display_name}`, { description: (e as Error).message });
			return false;
		}
	}

	/** Act in another of the user's orgs: everything on the page is per org. */
	switchOrg(id: string) {
		setOrg(id);
		this.orgId = id;
		this.workers = [];
		this.runs = [];
		this.archived = null;
		this.pending = [];
		this.members = [];
		this.#membersFor = '';
		this.added = null;
		this.setup = null;
		this.#migrated = false;
		this.loaded = false;
		this.conn = 'connecting';
		if (location.hash.startsWith('#/runs/')) location.hash = '#/runs';
		else if (location.hash.startsWith('#/rooms/')) location.hash = '#/rooms';
		void this.refresh(false);
	}

	async signIn(key: string): Promise<boolean> {
		if (!(await keyWorks(key))) return false;
		setKey(key);
		this.conn = 'connecting';
		this.setup = null;
		await this.refresh();
		return true;
	}

	async signOut() {
		if (this.me?.user) await fetch('/auth/logout', { method: 'POST', credentials: 'same-origin' }).catch(() => {});
		setKey('');
		setOrg('');
		this.orgId = '';
		this.me = null;
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

	#migrated = false;
	/** Repos chosen before identities existed are stored by folder name; convert them once. */
	async #migrateAdded(workers: Worker[], added: string[]) {
		if (this.#migrated || !added.length) return;
		const byName = new Map<string, string>();
		workers.forEach((w) => (w.capabilities.repos ?? []).forEach((r) => { if (r.id && r.id !== r.name) byName.set(r.name, r.id); }));
		if (!byName.size) return; // workers have not reported identities yet
		this.#migrated = true;
		// Also repair ids whose host was an SSH alias (`github.com-work/org/repo`): same path, real host.
		const keys = new Set(workers.flatMap((w) => (w.capabilities.repos ?? []).map(repoKey)));
		const byPath = new Map<string, string[]>();
		keys.forEach((k) => { const p = k.split('/').slice(1).join('/'); if (p) byPath.set(p, [...(byPath.get(p) ?? []), k]); });
		const fix = (n: string) => {
			if (keys.has(n)) return n;
			if (byName.has(n)) return byName.get(n)!;
			const same = byPath.get(n.split('/').slice(1).join('/'));
			return same && same.length === 1 ? same[0] : n;
		};
		const next = [...new Set(added.map(fix))];
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

/** The connect line. Without a static token (hosted orgs) the machine asks to join and is approved on Machines. */
export function connectCmd(masked: boolean) {
	const t = fleet.setup?.worker_token;
	if (!t) return `sidefleet connect --control ${location.origin}`;
	const tok = masked ? t.slice(0, 4) + '••••••••' : t;
	return `sidefleet connect --control ${location.origin} --token ${tok}`;
}

export function agentPrompt(masked: boolean) {
	const t = fleet.setup?.worker_token;
	if (!t)
		return `Read ${location.origin}/connect.md and follow it to connect this machine to Sidefleet. Control plane: ${location.origin}. There is no token: run connect without --token and tell me the code it prints so I can approve it on the Machines page.`;
	const tok = masked ? t.slice(0, 4) + '••••••••' : t;
	return `Read ${location.origin}/connect.md and follow it to connect this machine to Sidefleet. Control plane: ${location.origin}. Worker token: ${tok} (keep it out of logs and your summary).`;
}
