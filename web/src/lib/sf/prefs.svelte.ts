// Your preferences for how the app behaves (the Settings page). Kept in this
// browser and shared by its open tabs.

const KEY = 'sidefleet_prefs';

/**
 * Which key sends from a message box. `smart`: replies go on Enter, a new task (longer,
 * and it takes a machine) on ⌘/Ctrl+Enter, and on touch screens Return always adds a line.
 */
export type SendKey = 'smart' | 'enter' | 'mod-enter';
export type DiffView = 'unified' | 'split';

interface Stored {
	sendKey: SendKey;
	diffView: DiffView;
	diffHideWhitespace: boolean;
	diffCollapsed: boolean;
	confirmStop: boolean;
}

const DEFAULTS: Stored = { sendKey: 'smart', diffView: 'unified', diffHideWhitespace: false, diffCollapsed: false, confirmStop: true };

export const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
const touch = () => typeof matchMedia !== 'undefined' && !matchMedia('(any-pointer: fine)').matches;

function read(): Partial<Stored> {
	try {
		const s = JSON.parse(localStorage.getItem(KEY) ?? 'null') as Partial<Stored> | null;
		if (s) return s;
		// The diff layout used to be stored on its own.
		const v = localStorage.getItem('sf.diffView');
		return v === 'split' ? { diffView: 'split' } : {};
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

	constructor() {
		this.#load();
		if (typeof window !== 'undefined') window.addEventListener('storage', (e) => { if (e.key === KEY) this.#load(); });
	}

	#load() {
		const s = { ...DEFAULTS, ...read() };
		this.sendKey = ['smart', 'enter', 'mod-enter'].includes(s.sendKey) ? s.sendKey : DEFAULTS.sendKey;
		this.diffView = s.diffView === 'split' ? 'split' : 'unified';
		this.diffHideWhitespace = !!s.diffHideWhitespace;
		this.diffCollapsed = !!s.diffCollapsed;
		this.confirmStop = s.confirmStop !== false;
	}

	/** Change preferences and save them. */
	set(p: Partial<Stored>) {
		Object.assign(this, p);
		const s: Stored = { sendKey: this.sendKey, diffView: this.diffView, diffHideWhitespace: this.diffHideWhitespace, diffCollapsed: this.diffCollapsed, confirmStop: this.confirmStop };
		try { localStorage.setItem(KEY, JSON.stringify(s)); } catch { /* storage blocked: lasts until reload */ }
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
