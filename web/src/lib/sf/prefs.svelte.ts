// Your preferences for how the app behaves (the Settings page). Kept in this
// browser and shared by its open tabs; signed in, the ones about tasks and sending
// are also kept with your account, so every browser you sign in on starts the same.

import { api } from './api';

const KEY = 'sidefleet_prefs';

/**
 * Which key sends from a message box. `smart`: replies go on Enter, a new task (longer,
 * and it takes a machine) on ⌘/Ctrl+Enter, and on touch screens Return always adds a line.
 */
export type SendKey = 'smart' | 'enter' | 'mod-enter';
export type DiffView = 'unified' | 'split';
export type Workspace = 'worktree' | 'checkout';

interface Stored {
	sendKey: SendKey;
	diffView: DiffView;
	diffHideWhitespace: boolean;
	diffCollapsed: boolean;
	confirmStop: boolean;
	/** The permission mode a new task starts with; the composer can change it for one task. */
	permission: string;
	/** The same for a task started in a room, which reads teammates' notes, so it starts narrower. */
	roomPermission: string;
	/** Where a new task works: its own worktree and branch, or the checkout itself. */
	workspace: Workspace;
}

const DEFAULTS: Stored = {
	sendKey: 'smart',
	diffView: 'unified',
	diffHideWhitespace: false,
	diffCollapsed: false,
	confirmStop: true,
	permission: 'bypassPermissions',
	roomPermission: 'acceptEdits',
	workspace: 'worktree'
};
const MODES = ['acceptEdits', 'bypassPermissions', 'plan', 'ask'];

/** What follows you between browsers. How the Changes panel looks (and the theme) suits each screen, so it stays put. */
const SYNCED = ['permission', 'roomPermission', 'workspace', 'sendKey', 'confirmStop'] as const;
const synced = (s: Partial<Stored>) =>
	Object.fromEntries(SYNCED.filter((k) => s[k] !== undefined).map((k) => [k, s[k]])) as Partial<Stored>;
const savedHere = () => { try { return localStorage.getItem(KEY) !== null; } catch { return false; } };

export const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
const touch = () => typeof matchMedia !== 'undefined' && !matchMedia('(any-pointer: fine)').matches;

function read(): Partial<Stored> {
	try {
		const s = JSON.parse(localStorage.getItem(KEY) ?? 'null') as Partial<Stored> | null;
		if (s) return s;
		// The diff layout and the composer's workspace used to be stored on their own.
		const old: Partial<Stored> = {};
		if (localStorage.getItem('sf.diffView') === 'split') old.diffView = 'split';
		if (localStorage.getItem('sidefleet_iso') === 'checkout') old.workspace = 'checkout';
		return old;
	} catch {
		return {};
	}
}

class Prefs {
	sendKey = $state<SendKey>(DEFAULTS.sendKey);
	diffView = $state<DiffView>(DEFAULTS.diffView);
	diffHideWhitespace = $state(DEFAULTS.diffHideWhitespace);
	diffCollapsed = $state(DEFAULTS.diffCollapsed);
	confirmStop = $state(DEFAULTS.confirmStop);
	permission = $state(DEFAULTS.permission);
	roomPermission = $state(DEFAULTS.roomPermission);
	workspace = $state<Workspace>(DEFAULTS.workspace);

	/** Signed in: the synced ones are kept with your account too. */
	account = $state(false);

	constructor() {
		this.#apply(read());
		if (typeof window !== 'undefined') window.addEventListener('storage', (e) => { if (e.key === KEY) this.#apply(read()); });
	}

	#apply(p: Partial<Stored>) {
		const s = { ...DEFAULTS, ...p };
		this.sendKey = ['smart', 'enter', 'mod-enter'].includes(s.sendKey) ? s.sendKey : DEFAULTS.sendKey;
		this.diffView = s.diffView === 'split' ? 'split' : 'unified';
		this.diffHideWhitespace = !!s.diffHideWhitespace;
		this.diffCollapsed = !!s.diffCollapsed;
		this.confirmStop = s.confirmStop !== false;
		this.permission = MODES.includes(s.permission) ? s.permission : DEFAULTS.permission;
		this.roomPermission = MODES.includes(s.roomPermission) ? s.roomPermission : DEFAULTS.roomPermission;
		this.workspace = s.workspace === 'checkout' ? 'checkout' : 'worktree';
	}

	#stored(): Stored {
		return {
			sendKey: this.sendKey,
			diffView: this.diffView,
			diffHideWhitespace: this.diffHideWhitespace,
			diffCollapsed: this.diffCollapsed,
			confirmStop: this.confirmStop,
			permission: this.permission,
			roomPermission: this.roomPermission,
			workspace: this.workspace
		};
	}

	#saveHere() {
		try { localStorage.setItem(KEY, JSON.stringify(this.#stored())); } catch { /* storage blocked: lasts until reload */ }
	}

	#push() {
		void api('/v1/me/prefs', { method: 'PUT', body: JSON.stringify(synced(this.#stored())) }).catch(() => { /* kept here; sent with the next change */ });
	}

	/** Take your account's preferences over this browser's. The first time, this browser's become your account's. */
	async #pull() {
		try {
			const theirs = synced(await api<Partial<Stored>>('/v1/me/prefs'));
			if (!Object.keys(theirs).length) {
				if (savedHere()) this.#push();
				return;
			}
			this.#apply({ ...this.#stored(), ...theirs });
			this.#saveHere();
		} catch {
			/* offline, or the developer key (nobody in particular): this browser's copy stands */
		}
	}

	/** Signed in: keep the synced preferences with your account, and pick up changes made elsewhere when you come back to the tab. */
	connect() {
		if (this.account) return;
		this.account = true;
		void this.#pull();
		document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') void this.#pull(); });
	}

	/** Change preferences and save them. */
	set(p: Partial<Stored>) {
		Object.assign(this, p);
		this.#saveHere();
		if (this.account && Object.keys(synced(p)).length) this.#push();
	}

	/** The mode a new task starts with for this agent: only Claude Code can ask you, so the others edit files instead. */
	permissionFor(agent: string, room = false) {
		const p = room ? this.roomPermission : this.permission;
		return p === 'ask' && agent !== 'claude' ? 'acceptEdits' : p;
	}

	/** Whether plain Enter sends from a box of this kind, here and now. */
	enterSends(box: 'task' | 'reply') {
		if (this.sendKey === 'enter') return true;
		if (this.sendKey === 'mod-enter') return false;
		return box === 'reply' && !touch();
	}

	/** Whether a keypress in a message box sends it. ⌘/Ctrl+Enter always does. */
	sends(e: KeyboardEvent, box: 'task' | 'reply') {
		if (e.key !== 'Enter' || e.isComposing || e.keyCode === 229) return false;
		if (isMac ? e.metaKey : e.ctrlKey) return true;
		return !e.shiftKey && !e.altKey && !e.metaKey && !e.ctrlKey && this.enterSends(box);
	}

	/** The keys that send from a box of this kind, for a hint: `['⌘', '↵']` or `['↵']`. */
	sendKeys(box: 'task' | 'reply') {
		return this.enterSends(box) ? ['↵'] : [isMac ? '⌘' : 'Ctrl', '↵'];
	}
}

export const prefs = new Prefs();
