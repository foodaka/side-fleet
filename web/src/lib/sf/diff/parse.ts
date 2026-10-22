// Unified diff patches from the worker (`diff` events), parsed into lines with
// numbers, word-level change ranges, and side-by-side rows.

export interface DiffFile {
	path: string;
	old_path: string | null;
	status: 'added' | 'deleted' | 'modified' | 'renamed';
	additions: number;
	deletions: number;
	/** Hunks only (from the first `@@`); null when the worker left it out. */
	patch: string | null;
	omitted: 'binary' | 'generated' | 'large' | 'budget' | null;
}

export interface DiffSet {
	files: DiffFile[];
	/** Files changed, which can exceed `files.length` on huge changes. */
	file_count: number;
	additions: number;
	deletions: number;
}

/** A `[start, end)` character range of a line that changed within it. */
export type Range = [number, number];

export interface Line {
	kind: 'ctx' | 'add' | 'del';
	old: number | null;
	new: number | null;
	text: string;
	/** Word-level changes against the line it replaced. */
	marks?: Range[];
	/** Position of this line in the old and new side texts, for highlighting. */
	oi?: number;
	ni?: number;
}

export interface Hunk {
	oldStart: number;
	newStart: number;
	/** The function or heading git found above the hunk. */
	section: string;
	/** Unchanged lines skipped since the previous hunk (or the top of the file). */
	gap: number;
	lines: Line[];
}

export interface Parsed {
	hunks: Hunk[];
	/** Old and new side texts, one entry per line, for the syntax highlighter. */
	oldText: string[];
	newText: string[];
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/;

export function parsePatch(patch: string): Parsed {
	const hunks: Hunk[] = [];
	const oldText: string[] = [];
	const newText: string[] = [];
	let h: Hunk | null = null;
	let o = 0, n = 0, prevOldEnd = 1;
	for (const raw of patch.replace(/\n$/, '').split('\n')) {
		const m = HUNK.exec(raw);
		if (m) {
			o = +m[1]; n = +m[3];
			// A zero-length side starts *after* the given line number.
			const oldStart = m[2] === '0' ? o + 1 : o;
			h = { oldStart, newStart: m[4] === '0' ? n + 1 : n, section: m[5].trim(), gap: Math.max(0, oldStart - prevOldEnd), lines: [] };
			if (m[2] === '0') o++;
			if (m[4] === '0') n++;
			hunks.push(h);
			continue;
		}
		if (!h) continue;
		const c = raw[0];
		const text = raw.slice(1);
		// An empty line is context whose leading space was stripped along the way.
		if (c === ' ' || raw === '') {
			h.lines.push({ kind: 'ctx', old: o++, new: n++, text, oi: oldText.push(text) - 1, ni: newText.push(text) - 1 });
		} else if (c === '-') {
			h.lines.push({ kind: 'del', old: o++, new: null, text, oi: oldText.push(text) - 1 });
		} else if (c === '+') {
			h.lines.push({ kind: 'add', old: null, new: n++, text, ni: newText.push(text) - 1 });
		}
		// `\ No newline at end of file` and anything unexpected are skipped.
		prevOldEnd = o;
	}
	for (const hk of hunks) markWords(hk.lines);
	return { hunks, oldText, newText };
}

/** Pair each run of deletions with the additions that follow it and mark the words that differ. */
function markWords(lines: Line[]) {
	let i = 0;
	while (i < lines.length) {
		if (lines[i].kind !== 'del') { i++; continue; }
		const ds = i;
		while (i < lines.length && lines[i].kind === 'del') i++;
		const as = i;
		while (i < lines.length && lines[i].kind === 'add') i++;
		const dels = lines.slice(ds, as), adds = lines.slice(as, i);
		for (let k = 0; k < Math.min(dels.length, adds.length); k++) {
			const r = wordDiff(dels[k].text, adds[k].text);
			if (r) [dels[k].marks, adds[k].marks] = r;
		}
	}
}

const WORD = /\w+|\s+|[^\w\s]/g;

/** Changed ranges in each of two lines, or null when they share too little to be worth marking. */
export function wordDiff(a: string, b: string): [Range[], Range[]] | null {
	if (a === b) return null;
	const ta = a.match(WORD) ?? [], tb = b.match(WORD) ?? [];
	const n = ta.length, m = tb.length;
	if (!n || !m || n * m > 40_000) return null;
	// Longest common subsequence of tokens.
	const dp = Array.from({ length: n + 1 }, () => new Uint16Array(m + 1));
	for (let x = n - 1; x >= 0; x--)
		for (let y = m - 1; y >= 0; y--)
			dp[x][y] = ta[x] === tb[y] ? dp[x + 1][y + 1] + 1 : Math.max(dp[x + 1][y], dp[x][y + 1]);
	const ra: Range[] = [], rb: Range[] = [];
	let x = 0, y = 0, pa = 0, pb = 0, same = 0;
	const push = (r: Range[], s: number, e: number) => {
		const last = r[r.length - 1];
		if (last && last[1] === s) last[1] = e;
		else r.push([s, e]);
	};
	while (x < n || y < m) {
		if (x < n && y < m && ta[x] === tb[y]) {
			same += ta[x].trim() ? ta[x].length : 0;
			pa += ta[x++].length; pb += tb[y++].length;
		} else if (y < m && (x === n || dp[x][y + 1] >= dp[x + 1][y])) {
			push(rb, pb, (pb += tb[y++].length));
		} else {
			push(ra, pa, (pa += ta[x++].length));
		}
	}
	// Mostly rewritten lines read better without confetti.
	const visible = Math.max(a.trim().length, b.trim().length);
	if (!visible || same / visible < 0.4) return null;
	return [ra, rb];
}

export interface SplitRow {
	left: Line | null;
	right: Line | null;
}

/** Side-by-side rows: context on both sides, deletions beside the additions that replaced them. */
export function splitRows(lines: Line[]): SplitRow[] {
	const rows: SplitRow[] = [];
	let i = 0;
	while (i < lines.length) {
		const l = lines[i];
		if (l.kind === 'ctx') { rows.push({ left: l, right: l }); i++; continue; }
		const dels: Line[] = [], adds: Line[] = [];
		while (i < lines.length && lines[i].kind === 'del') dels.push(lines[i++]);
		while (i < lines.length && lines[i].kind === 'add') adds.push(lines[i++]);
		for (let k = 0; k < Math.max(dels.length, adds.length); k++)
			rows.push({ left: dels[k] ?? null, right: adds[k] ?? null });
	}
	return rows;
}

/** `src/lib/x.ts` → `['src/lib/', 'x.ts']`. */
export function splitPath(p: string): [string, string] {
	const i = p.lastIndexOf('/');
	return i < 0 ? ['', p] : [p.slice(0, i + 1), p.slice(i + 1)];
}

/** GitHub's five-block diffstat: how many blocks are green, red and grey. */
export function statBlocks(add: number, del: number): [number, number, number] {
	const total = add + del;
	if (!total) return [0, 0, 5];
	const colored = Math.min(5, total);
	let g = Math.round((colored * add) / total);
	if (add && !g) g = 1;
	if (del && g === colored) g = colored - 1;
	return [g, colored - g, 5 - colored];
}

export const omittedReason: Record<NonNullable<DiffFile['omitted']>, string> = {
	binary: 'Binary file not shown.',
	generated: 'Generated file. Its diff is hidden.',
	large: 'This diff is too large to show here.',
	budget: 'Not shown: this change is too large to send in full.'
};
