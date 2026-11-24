// Projects: the task list grouped by repo, each with an icon. The icons are the org's,
// kept on the control plane so everyone (and the Mac app) sees the same ones; how the
// list is grouped and which groups are folded is yours, kept in this browser.

import { toast } from 'svelte-sonner';
import { api } from './api';
import { repoLabel } from './format';

export type IconKind = 'emoji' | 'letter' | 'image';
export interface ProjectIcon {
	kind: IconKind;
	/** The emoji, the tile's colour (#rrggbb), or a data: URL. */
	value: string;
}
interface StoredIcon extends ProjectIcon {
	repo: string;
}

/** The tile colours a letter icon can take; a repo without an icon gets one of them. */
export const TILE_COLORS = ['#7c5cff', '#e5484d', '#f08a2e', '#d4a20a', '#22a06b', '#12a5b0', '#3e63dd', '#d6409f', '#6b6a65'];

/** A choice of emoji for the picker; any other can be pasted. */
export const EMOJI = [
	'🪐', '🌙', '🚀', '⚡️', '🔥', '🌱', '🍋', '🍓', '🎯', '🧪', '🛠️', '📦', '🧭', '🗺️', '📱', '⌚️',
	'💳', '🔐', '🧾', '📝', '🎨', '🎧', '📷', '🏔️', '🌊', '🐙', '🦊', '🐝', '🐢', '🦉', '🧠', '✨'
];

/** The icon's square size in pixels once an image is shrunk. */
export const IMAGE_SIZE = 64;

/** The name shown for a repo: its last path part (`side-fleet` for github.com/acme/side-fleet). */
export function projectName(repo: string): string {
	return repoLabel(repo).split('/').pop() || repo;
}

function hash(s: string): number {
	let h = 7;
	for (const c of s) h = (Math.imul(h, 31) + c.charCodeAt(0)) >>> 0;
	return h;
}

/** The icon a repo shows when nobody set one: its first letter on a colour picked from its name. */
export function defaultIcon(repo: string): ProjectIcon {
	return { kind: 'letter', value: TILE_COLORS[hash(repo) % TILE_COLORS.length] };
}

/** Crop an image file square and shrink it to `IMAGE_SIZE` as a PNG data: URL. */
export async function shrinkImage(file: File): Promise<string> {
	const url = URL.createObjectURL(file);
	try {
		const img = new Image();
		img.src = url;
		await img.decode();
		const side = Math.min(img.naturalWidth, img.naturalHeight);
		const c = document.createElement('canvas');
		c.width = c.height = IMAGE_SIZE;
		const ctx = c.getContext('2d')!;
		ctx.imageSmoothingQuality = 'high';
		ctx.drawImage(img, (img.naturalWidth - side) / 2, (img.naturalHeight - side) / 2, side, side, 0, 0, IMAGE_SIZE, IMAGE_SIZE);
		return c.toDataURL('image/png');
	} finally {
		URL.revokeObjectURL(url);
	}
}

const VIEW = 'sidefleet_runs_view';
const FOLDED = 'sidefleet_projects_folded';
const SHOW_ALL = 'sidefleet_projects_all';
const HIDDEN = 'sidefleet_projects_hidden';

export type ListView = 'time' | 'project';

function readSet(key: string): Set<string> {
	try {
		const v = JSON.parse(localStorage.getItem(key) ?? '[]');
		return new Set(Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []);
	} catch {
		return new Set();
	}
}
function writeSet(key: string, s: Set<string>) {
	try { localStorage.setItem(key, JSON.stringify([...s])); } catch { /* storage blocked: lasts until reload */ }
}

class Projects {
	/** Icons set in this org, by repo. */
	icons = $state<Map<string, ProjectIcon>>(new Map());
	view = $state<ListView>('time');
	/** Groups folded shut. */
	folded = $state<Set<string>>(new Set());
	/** Groups showing every task rather than the latest few. */
	showAll = $state<Set<string>>(new Set());
	/** Groups left out of the list (they come back from the foot of it). */
	hidden = $state<Set<string>>(new Set());
	/** The repo whose icon is being picked; null when the picker is shut. */
	picking = $state<string | null>(null);
	/** A repo to start a new task in, set from a group's menu and taken by the composer. */
	composeIn = $state<string | null>(null);

	#loadedAt = 0;

	constructor() {
		try { if (localStorage.getItem(VIEW) === 'project') this.view = 'project'; } catch { /* storage blocked */ }
		this.folded = readSet(FOLDED);
		this.showAll = readSet(SHOW_ALL);
		this.hidden = readSet(HIDDEN);
	}

	icon(repo: string): ProjectIcon {
		return this.icons.get(repo) ?? defaultIcon(repo);
	}

	setView(v: ListView) {
		this.view = v;
		try { localStorage.setItem(VIEW, v); } catch { /* storage blocked */ }
	}

	#toggle(key: string, field: 'folded' | 'showAll' | 'hidden', repo: string, on?: boolean) {
		const s = new Set(this[field]);
		if (on ?? !s.has(repo)) s.add(repo);
		else s.delete(repo);
		this[field] = s;
		writeSet(key, s);
	}
	toggleFolded(repo: string) { this.#toggle(FOLDED, 'folded', repo); }
	toggleShowAll(repo: string) { this.#toggle(SHOW_ALL, 'showAll', repo); }
	hide(repo: string) { this.#toggle(HIDDEN, 'hidden', repo, true); }
	unhideAll() {
		this.hidden = new Set();
		writeSet(HIDDEN, this.hidden);
	}

	#take(list: StoredIcon[]) {
		this.icons = new Map(list.filter((i) => ['emoji', 'letter', 'image'].includes(i.kind)).map((i) => [i.repo, { kind: i.kind, value: i.value }]));
	}

	/** Fetch the org's icons, at most once a minute unless forced (switching org, coming back to the tab). */
	async load(force = false) {
		if (!force && Date.now() - this.#loadedAt < 60_000) return;
		this.#loadedAt = Date.now();
		try {
			this.#take(await api<StoredIcon[]>('/v1/project-icons'));
		} catch {
			/* an older control plane, or offline: defaults until the next try */
		}
	}

	/** Forget the last org's icons. */
	reset() {
		this.icons = new Map();
		this.#loadedAt = 0;
	}

	/** Set a repo's icon for the whole org (null: back to the default), at once. */
	async setIcon(repo: string, icon: ProjectIcon | null) {
		const before = this.icons;
		const next = new Map(before);
		if (icon) next.set(repo, icon);
		else next.delete(repo);
		this.icons = next;
		try {
			this.#take(await api<StoredIcon[]>('/v1/project-icons', { method: 'PUT', body: JSON.stringify({ repo, icon }) }));
		} catch (e) {
			this.icons = before;
			toast.error('Could not set the icon', { description: (e as Error).message });
		}
	}
}

export const projects = new Projects();
