// Syntax colours for diffs: GitHub's light and dark themes at once, as CSS
// variables, so switching the theme needs no re-render. Languages load on demand.

import type { HighlighterCore, ThemedToken } from 'shiki/core';

export interface Tok {
	content: string;
	/** `--shiki-light:#…;--shiki-dark:#…` */
	style: string;
}

type Loader = () => Promise<{ default: unknown }>;

const LANGS: Record<string, [string, Loader]> = {
	ts: ['typescript', () => import('@shikijs/langs/typescript')],
	mts: ['typescript', () => import('@shikijs/langs/typescript')],
	cts: ['typescript', () => import('@shikijs/langs/typescript')],
	tsx: ['tsx', () => import('@shikijs/langs/tsx')],
	js: ['javascript', () => import('@shikijs/langs/javascript')],
	mjs: ['javascript', () => import('@shikijs/langs/javascript')],
	cjs: ['javascript', () => import('@shikijs/langs/javascript')],
	jsx: ['jsx', () => import('@shikijs/langs/jsx')],
	svelte: ['svelte', () => import('@shikijs/langs/svelte')],
	vue: ['vue', () => import('@shikijs/langs/vue')],
	rs: ['rust', () => import('@shikijs/langs/rust')],
	go: ['go', () => import('@shikijs/langs/go')],
	py: ['python', () => import('@shikijs/langs/python')],
	rb: ['ruby', () => import('@shikijs/langs/ruby')],
	java: ['java', () => import('@shikijs/langs/java')],
	kt: ['kotlin', () => import('@shikijs/langs/kotlin')],
	swift: ['swift', () => import('@shikijs/langs/swift')],
	c: ['c', () => import('@shikijs/langs/c')],
	h: ['c', () => import('@shikijs/langs/c')],
	cc: ['cpp', () => import('@shikijs/langs/cpp')],
	cpp: ['cpp', () => import('@shikijs/langs/cpp')],
	hpp: ['cpp', () => import('@shikijs/langs/cpp')],
	cs: ['csharp', () => import('@shikijs/langs/csharp')],
	php: ['php', () => import('@shikijs/langs/php')],
	sh: ['bash', () => import('@shikijs/langs/bash')],
	bash: ['bash', () => import('@shikijs/langs/bash')],
	zsh: ['bash', () => import('@shikijs/langs/bash')],
	json: ['json', () => import('@shikijs/langs/json')],
	jsonc: ['jsonc', () => import('@shikijs/langs/jsonc')],
	yaml: ['yaml', () => import('@shikijs/langs/yaml')],
	yml: ['yaml', () => import('@shikijs/langs/yaml')],
	toml: ['toml', () => import('@shikijs/langs/toml')],
	md: ['markdown', () => import('@shikijs/langs/markdown')],
	mdx: ['mdx', () => import('@shikijs/langs/mdx')],
	html: ['html', () => import('@shikijs/langs/html')],
	css: ['css', () => import('@shikijs/langs/css')],
	scss: ['scss', () => import('@shikijs/langs/scss')],
	sql: ['sql', () => import('@shikijs/langs/sql')],
	graphql: ['graphql', () => import('@shikijs/langs/graphql')],
	lua: ['lua', () => import('@shikijs/langs/lua')],
	ex: ['elixir', () => import('@shikijs/langs/elixir')],
	exs: ['elixir', () => import('@shikijs/langs/elixir')],
	zig: ['zig', () => import('@shikijs/langs/zig')],
	dart: ['dart', () => import('@shikijs/langs/dart')],
	tf: ['terraform', () => import('@shikijs/langs/terraform')],
	xml: ['xml', () => import('@shikijs/langs/xml')],
	svg: ['xml', () => import('@shikijs/langs/xml')],
	ini: ['ini', () => import('@shikijs/langs/ini')]
};
const NAMES: Record<string, string> = {
	Dockerfile: 'dockerfile', Makefile: 'makefile', '.env': 'dotenv', '.gitignore': 'shellscript'
};
const BY_NAME: Record<string, Loader> = {
	dockerfile: () => import('@shikijs/langs/dockerfile'),
	makefile: () => import('@shikijs/langs/makefile'),
	dotenv: () => import('@shikijs/langs/dotenv'),
	shellscript: () => import('@shikijs/langs/shellscript')
};

function langFor(path: string): [string, Loader] | null {
	const name = path.slice(path.lastIndexOf('/') + 1);
	if (NAMES[name]) return [NAMES[name], BY_NAME[NAMES[name]]];
	if (name.startsWith('.env')) return ['dotenv', BY_NAME.dotenv];
	const dot = name.lastIndexOf('.');
	return dot > 0 ? (LANGS[name.slice(dot + 1).toLowerCase()] ?? null) : null;
}

let core: Promise<HighlighterCore> | null = null;
const loading = new Map<string, Promise<void>>();

function highlighter() {
	core ??= (async () => {
		const [{ createHighlighterCore }, { createJavaScriptRegexEngine }] = await Promise.all([
			import('shiki/core'),
			import('shiki/engine/javascript')
		]);
		return createHighlighterCore({
			themes: [import('@shikijs/themes/github-light-default'), import('@shikijs/themes/github-dark-default')],
			langs: [],
			engine: createJavaScriptRegexEngine({ forgiving: true })
		});
	})();
	return core;
}

/** Tokens per line for each text, or null when the language is unknown. */
export async function highlight(path: string, texts: string[][]): Promise<Tok[][][] | null> {
	const lang = langFor(path);
	if (!lang) return null;
	const h = await highlighter();
	const [id, load] = lang;
	if (!h.getLoadedLanguages().includes(id)) {
		if (!loading.has(id)) loading.set(id, load().then((m) => h.loadLanguage(m.default as never)));
		await loading.get(id);
	}
	return texts.map((lines) => {
		if (!lines.length) return [];
		const r = h.codeToTokens(lines.join('\n'), {
			lang: id,
			themes: { light: 'github-light-default', dark: 'github-dark-default' },
			defaultColor: false,
			tokenizeMaxLineLength: 1000
		});
		return r.tokens.map((line: ThemedToken[]) =>
			line.map((t) => ({
				content: t.content,
				style: Object.entries(t.htmlStyle ?? {}).map(([k, v]) => `${k}:${v}`).join(';')
			}))
		);
	});
}

/** A line's tokens cut at the word-change marks, so each piece knows whether it changed. */
export function segments(text: string, toks: Tok[] | undefined, marks: [number, number][] | undefined) {
	const base: Tok[] = toks?.length ? toks : [{ content: text, style: '' }];
	if (!marks?.length) return base.map((t) => ({ ...t, mark: false }));
	const out: (Tok & { mark: boolean })[] = [];
	let pos = 0;
	for (const t of base) {
		let start = 0;
		const end = t.content.length;
		while (start < end) {
			const abs = pos + start;
			const m = marks.find(([s, e]) => abs >= s && abs < e);
			const next = m ? m[1] : Math.min(...marks.filter(([s]) => s > abs).map(([s]) => s), Infinity);
			const cut = Math.min(end, next - pos);
			out.push({ content: t.content.slice(start, cut), style: t.style, mark: !!m });
			start = cut;
		}
		pos += end;
	}
	return out;
}
