<script lang="ts">
	import { tick } from 'svelte';
	import { fleet } from '../state.svelte';
	import { seen } from '../seen.svelte';
	import { ago, dayGroup, duration, repoLabel } from '../format';
	import type { RunSummary } from '../types';
	import StatusDot from './StatusDot.svelte';

	let { query = '', activeId = null }: { query?: string; activeId?: string | null } = $props();

	let scroller = $state<HTMLElement | null>(null);

	const live = (r: RunSummary) => r.thread_status === 'running' || r.thread_status === 'queued';

	/** The repo most runs are in; rows from any other repo say which one. */
	const homeRepo = $derived.by(() => {
		const n = new Map<string, number>();
		fleet.runs.forEach((r) => n.set(r.repo, (n.get(r.repo) ?? 0) + 1));
		return [...n.entries()].sort((a, b) => b[1] - a[1])[0]?.[0] ?? '';
	});

	const q = $derived(query.trim().toLowerCase());
	const matches = $derived(
		q ? fleet.runs.filter((r) => [r.task, repoLabel(r.repo), r.worker_id ?? ''].some((s) => s.toLowerCase().includes(q))) : []
	);

	const active = $derived(
		fleet.runs.filter(live).sort((a, b) =>
			a.thread_status === b.thread_status ? 0 : a.thread_status === 'running' ? -1 : 1
		)
	);
	const groups = $derived.by(() => {
		const out: { label: string; runs: RunSummary[] }[] = [];
		for (const r of fleet.runs) {
			if (live(r)) continue;
			const label = dayGroup(r.last_activity_at, fleet.now);
			if (out.at(-1)?.label !== label) out.push({ label, runs: [] });
			out.at(-1)!.runs.push(r);
		}
		return out;
	});

	// Keep the open run in view, e.g. after opening it from somewhere else.
	$effect(() => {
		const id = activeId;
		if (!id || !scroller) return;
		void tick().then(() => scroller?.querySelector(`[data-run="${CSS.escape(id)}"]`)?.scrollIntoView({ block: 'nearest' }));
	});
</script>

{#snippet row(r: RunSummary, detail: boolean)}
	{@const on = r.id === activeId}
	{@const unread = !on && seen.unread(r)}
	{@const other = r.repo !== homeRepo}
	<a
		href="#/runs/{r.id}"
		data-run={r.id}
		aria-current={on ? 'page' : undefined}
		title={r.task}
		class="group flex items-start gap-2.5 rounded-md px-2.5 py-[7px] text-[13px] leading-[18px] transition-colors {on
			? 'bg-sidebar-accent text-foreground'
			: 'text-foreground/80 hover:bg-sidebar-accent/60 hover:text-foreground'}"
	>
		<span class="grid h-[18px] w-2 shrink-0 place-items-center">
			<StatusDot status={r.thread_status} class={r.thread_status === 'succeeded' && !unread ? 'opacity-45' : ''} />
		</span>
		<span class="flex min-w-0 flex-1 flex-col">
			<span class="truncate {unread ? 'font-medium text-foreground' : ''}">{r.task}</span>
			{#if detail}
				<span class="tnum truncate text-[11.5px] text-muted-foreground">
					{#if r.thread_status === 'running'}
						{r.worker_id ?? 'Starting'} · {duration(r.started_at, null, fleet.now) || 'starting'}
					{:else}
						Waiting for a machine{#if other}&nbsp;· {repoLabel(r.repo)}{/if}
					{/if}
				</span>
			{:else if other}
				<span class="truncate font-mono text-[11px] text-faint">{repoLabel(r.repo)}</span>
			{/if}
		</span>
		{#if unread}
			<span class="mt-[6px] size-1.5 shrink-0 rounded-full bg-foreground" aria-label="New"></span>
		{:else if !detail}
			<span class="tnum shrink-0 text-[11px] text-faint group-hover:text-muted-foreground">{ago(r.last_activity_at, fleet.now)}</span>
		{/if}
	</a>
{/snippet}

{#snippet heading(label: string, count?: number)}
	<h3 class="flex items-center gap-1.5 px-2.5 pt-4 pb-1 text-[11px] font-medium text-faint">
		{label}{#if count}<span class="tnum rounded-full bg-run-soft px-1.5 text-[10.5px] leading-4 text-run-ink">{count}</span>{/if}
	</h3>
{/snippet}

<div
	bind:this={scroller}
	class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-2 pb-3 [mask-image:linear-gradient(to_bottom,transparent,black_10px,black_calc(100%-24px),transparent)] [scrollbar-width:thin]"
>
	{#if !fleet.loaded}
		<div class="flex flex-col gap-1 px-2.5 pt-4" aria-hidden="true">
			{#each [72, 88, 60, 80, 66, 84] as w}
				<div class="flex h-8 items-center gap-2.5">
					<span class="size-1.5 rounded-full bg-muted"></span>
					<span class="h-3 animate-pulse rounded bg-muted" style="width: {w}%"></span>
				</div>
			{/each}
		</div>
	{:else if q}
		{@render heading(matches.length ? `${matches.length} match${matches.length === 1 ? '' : 'es'}` : 'No matches')}
		{#each matches as r (r.id)}{@render row(r, false)}{/each}
		{#if !matches.length}
			<p class="px-2.5 py-2 text-xs text-muted-foreground">Nothing in the latest {fleet.runs.length} tasks mentions “{query.trim()}”.</p>
		{/if}
	{:else if !fleet.runs.length}
		<div class="mx-1 mt-4 rounded-lg border border-dashed border-sidebar-border px-3 py-4 text-xs leading-relaxed text-muted-foreground">
			No tasks yet. Each one you start shows up here, so you can jump between them.
		</div>
	{:else}
		{#if active.length}
			{@render heading('Active', active.length)}
			{#each active as r (r.id)}{@render row(r, true)}{/each}
		{/if}
		{#each groups as g (g.label)}
			{@render heading(g.label)}
			{#each g.runs as r (r.id)}{@render row(r, false)}{/each}
		{/each}
		{#if fleet.runs.length >= 100}
			<p class="px-2.5 pt-4 text-[11px] text-faint">Showing the latest {fleet.runs.length} tasks.</p>
		{/if}
	{/if}
</div>
