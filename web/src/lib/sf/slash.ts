import { api } from './api';
import type { SlashCommand } from './types';

/** Fetched lists, per machine and repo; asked again after this long. */
const FRESH_MS = 60_000;
const cache = new Map<string, { at: number; list: Promise<SlashCommand[]> }>();

/** The slash commands and skills Claude Code offers on `worker` in `repo`, the repo's own first. */
export function slashCommands(worker: string, repo: string): Promise<SlashCommand[]> {
	const key = `${worker}\n${repo}`;
	const hit = cache.get(key);
	if (hit && Date.now() - hit.at < FRESH_MS) return hit.list;
	const list = api<SlashCommand[]>(`/v1/workers/${encodeURIComponent(worker)}/commands?repo=${encodeURIComponent(repo)}`);
	cache.set(key, { at: Date.now(), list });
	list.catch(() => cache.delete(key));
	return list;
}

/** What is typed after the slash while the message is only `/word`, else null. */
export function slashQuery(text: string): string | null {
	const m = /^\/(\S*)$/.exec(text);
	return m ? m[1] : null;
}

/** Commands for `query`: names starting with it, then names with a part starting with it, then descriptions mentioning it. */
export function matchCommands(list: SlashCommand[], query: string): SlashCommand[] {
	const q = query.toLowerCase();
	if (!q) return list;
	const rank = (c: SlashCommand) => {
		const n = c.name.toLowerCase();
		if (n.startsWith(q)) return 0;
		if (n.split(/[:\-_]/).some((p) => p.startsWith(q))) return 1;
		if (n.includes(q)) return 2;
		if (c.description?.toLowerCase().includes(q)) return 3;
		return -1;
	};
	return list
		.map((c, i) => ({ c, r: rank(c), i }))
		.filter((x) => x.r >= 0)
		.sort((a, b) => a.r - b.r || a.i - b.i)
		.map((x) => x.c);
}
