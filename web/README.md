# Sidefleet web UI

SvelteKit (static, hash routing) + Tailwind 4 + shadcn-svelte, with chat components
from [Svelte AI Elements](https://svelte-ai-elements.vercel.app). The compiled app is
written to `crates/server/web-dist/` and embedded in the `sidefleet` binary, so
installing Sidefleet from source needs only Rust. Node is needed only to change the UI.

```bash
pnpm install
pnpm dev        # http://localhost:5173, proxies /v1 to `sidefleet serve` on 127.0.0.1:7373
pnpm check      # type-check
pnpm build      # writes ../crates/server/web-dist (commit it)
```

Point the dev server at another control plane with `SIDEFLEET_DEV_API=http://host:port pnpm dev`.

- `src/lib/sf/` is Sidefleet's own code: API client, live state, event stream, thread model, components.
- `src/lib/components/ui/` and `src/lib/components/ai-elements/` were copied in by the shadcn-svelte CLI;
  edit them freely.
- `vite.config.ts` stubs syntax-highlighting grammars we don't need, plus Mermaid and KaTeX, to keep the
  embedded app small. Add a language to `LANGS` there if agents print it often.
