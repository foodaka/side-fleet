<script lang="ts">
	import { Button } from '#lib/components/ui/button';
	import * as Tooltip from '#lib/components/ui/tooltip';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import { toast } from 'svelte-sonner';
	import { api } from '../api';
	import { fleet } from '../state.svelte';
	import { parseBuild, pct, rel, uptime } from '../format';
	import Gauge from './Gauge.svelte';
	import StatusDot from './StatusDot.svelte';
	import type { Worker } from '../types';

	let { w }: { w: Worker } = $props();
	const c = $derived(w.capabilities);
	const sys = $derived(c.system);
	const slots = $derived(c.slots);
	const build = $derived(parseBuild(c.version));
	const repos = $derived(c.repos?.length ?? 0);
	const chosen = $derived(fleet.added ? (c.repos ?? []).filter((r) => fleet.added!.has(r.name)).length : 0);
	let requesting = $state(false);

	async function update() {
		requesting = true;
		try {
			await api(`/v1/workers/${encodeURIComponent(w.id)}/update`, { method: 'POST' });
			toast.success(`${w.display_name} will update`, { description: 'It pulls, rebuilds and restarts as soon as it has no running tasks.' });
			await fleet.refresh();
		} catch (e) {
			toast.error('Could not request the update', { description: (e as Error).message });
		}
		requesting = false;
	}
</script>

<article class="flex flex-col gap-5 rounded-xl border bg-card p-5 {w.online ? '' : 'opacity-70'}">
	<header class="flex items-start justify-between gap-3">
		<div class="min-w-0">
			<h2 class="truncate text-[15px] font-semibold tracking-tight">{w.display_name}</h2>
			<p class="mt-0.5 truncate text-xs text-muted-foreground">
				{c.os}{#if w.display_name !== c.hostname && c.hostname}, {c.hostname}{/if}{#if sys}, up {uptime(sys.uptime_secs)}{/if}
			</p>
		</div>
		<span class="flex shrink-0 items-center gap-1.5 text-xs {w.online ? 'text-ok-ink' : 'text-muted-foreground'}">
			<StatusDot status={w.online ? 'online' : 'offline'} />{w.online ? 'Online' : `Last seen ${rel(w.last_heartbeat_at, fleet.now)}`}
		</span>
	</header>

	{#if sys}
		<div class="grid grid-cols-2 gap-x-5 gap-y-3.5">
			<Gauge label="CPU" value="{Math.round(sys.cpu_percent)}% of {sys.cpu_cores}" percent={Math.round(sys.cpu_percent)} />
			<Gauge label="Memory" value="{(sys.mem_used_mb / 1024).toFixed(1)} / {Math.round(sys.mem_total_mb / 1024)} GB" percent={pct(sys.mem_used_mb, sys.mem_total_mb)} />
			<Gauge label="Disk free" value="{Math.round(sys.disk_free_gb)} of {Math.round(sys.disk_total_gb)} GB" percent={pct(sys.disk_total_gb - sys.disk_free_gb, sys.disk_total_gb)} />
			{#if slots}<Gauge label="Tasks" value="{slots.active} of {slots.parallel} running" percent={pct(slots.active, slots.parallel)} />{/if}
		</div>
	{:else if w.online}
		<p class="rounded-lg bg-muted px-3 py-2.5 text-xs leading-relaxed text-muted-foreground">
			No live numbers yet: this machine runs an older Sidefleet. Update it and CPU, memory and disk appear here.
		</p>
	{/if}

	<div class="flex flex-wrap gap-1.5">
		{#if c.agents.claude}
			<Tooltip.Root>
				<Tooltip.Trigger class="inline-flex h-6 items-center gap-1.5 rounded-md border px-2 text-xs {c.agents.claude.signed_in ? '' : 'border-err/40 text-err-ink'}">
					{#if c.agents.claude.signed_in}<CircleCheck class="size-3.5 text-ok" />{:else}<TriangleAlert class="size-3.5" />{/if}
					Claude Code <span class="tnum text-muted-foreground">{c.agents.claude.version}</span>
				</Tooltip.Trigger>
				<Tooltip.Content>{c.agents.claude.signed_in ? 'Signed in; tasks run with this account' : 'Not signed in: run `claude` on this machine once'}</Tooltip.Content>
			</Tooltip.Root>
		{/if}
		{#if c.agents.codex}<span class="inline-flex h-6 items-center gap-1.5 rounded-md border px-2 text-xs">Codex <span class="tnum text-muted-foreground">{c.agents.codex.version}</span></span>{/if}
		{#if c.agents.ollama}<span class="inline-flex h-6 items-center gap-1.5 rounded-md border px-2 text-xs">Ollama <span class="tnum text-muted-foreground">{c.agents.ollama.models.length} models</span></span>{/if}
		{#if !c.agents.claude && !c.agents.codex && !c.agents.ollama}<span class="inline-flex h-6 items-center rounded-md border border-dashed px-2 text-xs text-muted-foreground">No coding agent found</span>{/if}
		<button type="button" onclick={() => (fleet.pickerOpen = true)} class="inline-flex h-6 items-center rounded-md px-2 text-xs text-muted-foreground hover:bg-accent hover:text-foreground">
			<span class="tnum">{repos} repos{fleet.added?.size ? `, ${chosen} chosen` : ''}</span>
		</button>
	</div>

	<footer class="mt-auto flex items-center justify-between gap-3 border-t pt-3.5 text-xs">
		<span class="min-w-0 truncate text-muted-foreground" title={c.version}>
			{#if build.sha}Sidefleet {build.version}, build <span class="font-mono">{build.sha}</span>{:else}Version unknown{/if}
		</span>
		{#if w.update_requested}
			<span class="flex shrink-0 items-center gap-1.5 text-run-ink"><RefreshCw class="size-3.5 animate-spin [animation-duration:2.5s]" />Updating when idle</span>
		{:else if w.outdated && w.online}
			<Button size="sm" variant="outline" class="h-7 shrink-0 rounded-full border-queue/50 text-queue-ink" onclick={update} disabled={requesting}>Update to latest</Button>
		{:else if w.online && c.version}
			<button type="button" class="shrink-0 text-muted-foreground hover:text-foreground" onclick={update} disabled={requesting}>Up to date</button>
		{/if}
	</footer>
</article>
