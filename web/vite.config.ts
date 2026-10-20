import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig, type Plugin } from 'vite';

// Syntax highlighting: keep the grammars coding agents actually print and stub the
// rest. Shiki ships ~300 languages as lazy chunks (about 15 MB), which would all end
// up embedded in the `sidefleet` binary. Unknown languages render as plain code.
const LANGS = new Set([
	'bash', 'shellscript', 'shell', 'sh', 'zsh', 'fish', 'powershell', 'bat',
	'javascript', 'typescript', 'jsx', 'tsx', 'json', 'jsonc', 'json5', 'yaml', 'toml', 'ini', 'dotenv',
	'html', 'css', 'scss', 'svelte', 'vue', 'astro', 'markdown', 'mdx', 'xml', 'graphql',
	'python', 'rust', 'go', 'java', 'kotlin', 'swift', 'ruby', 'php', 'c', 'cpp', 'csharp', 'dart', 'elixir', 'lua', 'zig',
	'sql', 'prisma', 'diff', 'docker', 'dockerfile', 'make', 'makefile', 'nginx', 'hcl', 'terraform', 'regexp', 'log'
]);
// Optional renderer extras we don't use for agent narration: the full Shiki bundle
// (every theme plus a WebAssembly engine; rendering uses `shiki/core`), Mermaid
// diagrams and KaTeX maths. Those blocks render as plain code instead.
const STUBS: Record<string, string> = {
	shiki: `export * from 'shiki/core';
export const bundledLanguages = {}, bundledThemes = {};
export function createHighlighter() { throw new Error('The full Shiki bundle is not part of Sidefleet'); }`,
	mermaid: `export default { initialize() {}, async render() { throw new Error('Diagrams are not rendered in Sidefleet'); } };`,
	katex: `const esc = (s) => String(s).replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);
export const renderToString = (s) => '<code>' + esc(s) + '</code>';
export default { renderToString };`,
	'katex/dist/katex.min.css': ''
};

function trimRendererExtras(): Plugin {
	const LANG_STUB = '\0sidefleet-unused-shiki-lang';
	return {
		name: 'sidefleet-trim-renderer-extras',
		enforce: 'pre',
		resolveId(id) {
			const m = /^@shikijs\/langs\/([\w-]+)/.exec(id);
			if (m && !LANGS.has(m[1])) return LANG_STUB;
			if (id in STUBS) return '\0sidefleet-stub:' + id + (id.endsWith('.css') ? '' : '.js');
		},
		load(id) {
			if (id === LANG_STUB) return 'export default []';
			if (id.startsWith('\0sidefleet-stub:')) return STUBS[id.slice('\0sidefleet-stub:'.length).replace(/\.js$/, '')];
		}
	};
}

// The control plane (`sidefleet serve`) embeds the compiled app from
// crates/server/web-dist. Hash routing keeps every page at `/`, so the server
// only serves index.html and the hashed assets, and old `/#/runs/<id>` links work.
export default defineConfig({
	plugins: [
		trimRendererExtras(),
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			adapter: adapter({
				pages: '../crates/server/web-dist',
				assets: '../crates/server/web-dist',
				fallback: 'index.html',
				precompress: false,
				strict: false
			}),
			router: { type: 'hash' },
			paths: { relative: true }
		})
	],
	server: {
		// `pnpm dev` talks to a running `sidefleet serve` (default port 7373).
		proxy: {
			'/v1': { target: process.env.SIDEFLEET_DEV_API ?? 'http://127.0.0.1:7373', changeOrigin: true },
			'/connect.md': { target: process.env.SIDEFLEET_DEV_API ?? 'http://127.0.0.1:7373', changeOrigin: true }
		}
	}
});
