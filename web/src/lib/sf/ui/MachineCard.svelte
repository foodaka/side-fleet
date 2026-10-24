<script lang="ts">
	import { Button } from '#lib/components/ui/button';
	import * as Tooltip from '#lib/components/ui/tooltip';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import { toast } from 'svelte-sonner';
	import { api } from '../api';
	import { fleet } from '../state.svelte';
	import { parseBuild, pct, rel, repoKey, uptime } from '../format';
	import Gauge from './Gauge.svelte';
	import { telemetry } from '../telemetry.svelte';
	import { duration } from '../format';
	import StatusDot from './StatusDot.svelte';
	import type { Worker } from '../types';

	let { w, hot = false, onhover }: { w: Worker; hot?: boolean; onhover?: (id: string | null) => void } = $props();
	const c = $derived(w.capabilities);
	const sys = $derived(c.system);
	const slots = $derived(c.slots);
	const build = $derived(parseBuild(c.version));
	// Count projects, not folders: several clones of one project count once.
	const keys = $derived(new Set((c.repos ?? []).map(repoKey)));
	const repos = $derived(keys.size);
	const chosen = $derived(fleet.added ? [...keys].filter((k) => fleet.added!.has(k)).length : 0);
	let requesting = $state(false);
	const AGENT_NAMES = { claude: 'Claude Code', codex: 'Codex', opencode: 'opencode' } as const;
	// Agents this machine takes no tasks for until their usage limit resets.
	const limits = $derived(
		Object.entries(w.limited_until ?? {})
			.filter(([, until]) => until && Date.parse(until) > fleet.now)
			.map(([agent, until]) => ({ name: AGENT_NAMES[agent as keyof typeof AGENT_NAMES] ?? agent, until: clock(until!) }))
	);
	// Name the agent unless it is the only one here.
	const several = $derived([c.agents.claude, c.agents.codex, c.agents.opencode].filter(Boolean).length > 1);
	function clock(iso: string) {
		const d = new Date(iso);
		const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
		return d.toDateString() === new Date(fleet.now).toDateString() ? time : `${d.toLocaleDateString([], { weekday: 'short' })} ${time}`;
	}
	const tasks = $derived(fleet.runs.filter((r) => r.thread_status === 'running' && r.worker_id === w.id));
	const history = $derived(telemetry.cpu[w.id] ?? []);
	// Sparkline over the readings collected while the page is open (one per heartbeat).
	const spark = $derived.by(() => {
		const h = history.length === 1 ? [history[0], history[0]] : history;
		if (!h.length) return null;
		const pts = h.map((v, i) => [(i / (h.length - 1)) * 120, 50 - (Math.min(v, 100) / 100) * 48]);
		const line = pts.map((p, i) => (i ? 'L' : 'M') + p[0].toFixed(1) + ' ' + p[1].toFixed(1)).join('');
		return { line, area: line + 'L120 52L0 52Z', end: pts[pts.length - 1] };
	});

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

	/** Forget an offline machine (frees a slot on the plan). It comes back if it connects again with a valid token. */
	async function remove() {
		if (!confirm(`Remove ${w.display_name}? It reappears if it connects again; revoke its token to keep it out.`)) return;
		try {
			await api(`/v1/workers/${encodeURIComponent(w.id)}`, { method: 'DELETE' });
			toast(`Removed ${w.display_name}`);
			await fleet.refresh();
		} catch (e) {
			toast.error('Could not remove the machine', { description: (e as Error).message });
		}
	}
</script>

<article
	id="machine-{w.id}"
	class="flex flex-col gap-5 rounded-xl border bg-card p-5 transition-[border-color,box-shadow] duration-200 {w.online ? '' : 'opacity-70'} {hot ? 'border-run/50 shadow-[0_0_0_4px_color-mix(in_oklab,var(--run)_10%,transparent)]' : ''}"
	onmouseenter={() => onhover?.(w.id)}
	onmouseleave={() => onhover?.(null)}
>
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
		<div class="grid grid-cols-[auto_1fr] items-end gap-4">
			<div>
				<div class="tnum text-[30px] leading-none font-medium tracking-tight">{Math.round(sys.cpu_percent)}%</div>
				<div class="mt-1 text-xs text-muted-foreground">CPU, {sys.cpu_cores} cores</div>
			</div>
			<svg class="h-[52px] w-full" viewBox="0 0 120 52" preserveAspectRatio="none" role="img" aria-label="CPU while this page has been open">
				<defs><linearGradient id="spark-{w.id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="var(--run)" stop-opacity="0.25" /><stop offset="1" stop-color="var(--run)" stop-opacity="0" /></linearGradient></defs>
				<line x1="0" y1="26" x2="120" y2="26" stroke="var(--border)" stroke-dasharray="2 3" vector-effect="non-scaling-stroke" />
				{#if spark}
					<path d={spark.area} fill="url(#spark-{w.id})" />
					<path d={spark.line} fill="none" stroke="var(--run)" stroke-width="1.6" stroke-linejoin="round" vector-effect="non-scaling-stroke" />
				{/if}
			</svg>
		</div>
		<div class="grid grid-cols-1 gap-x-5 gap-y-3.5 sm:grid-cols-2">
			<Gauge label="Memory" value="{(sys.mem_used_mb / 1024).toFixed(1)} / {Math.round(sys.mem_total_mb / 1024)} GB" percent={pct(sys.mem_used_mb, sys.mem_total_mb)} />
			<Gauge label="Disk" value="{Math.round(sys.disk_free_gb).toLocaleString()} GB free" percent={pct(sys.disk_total_gb - sys.disk_free_gb, sys.disk_total_gb)} />
		</div>
		{#if slots}
			<div class="flex flex-col gap-2">
				<div class="flex justify-between text-xs"><span class="text-muted-foreground">Tasks</span><span class="tnum text-foreground/80">{tasks.length} of {slots.parallel} running</span></div>
				<div class="grid gap-1.5" style="grid-template-columns: repeat({slots.parallel}, minmax(0, 1fr))">
					{#each Array(slots.parallel) as _, i}
						<div class="h-2 overflow-hidden rounded {i < tasks.length ? 'sf-slot-busy bg-run-soft' : 'bg-muted'}"></div>
					{/each}
				</div>
				{#each tasks as t (t.id)}
					<a href="#/runs/{t.id}" class="flex min-w-0 items-center gap-2 text-[12.5px] hover:underline">
						<span class="size-1.5 shrink-0 rounded-full bg-run"></span>
						<span class="min-w-0 flex-1 truncate">{t.task}</span>
						<span class="tnum shrink-0 text-muted-foreground">{duration(t.started_at, null, fleet.now)}</span>
					</a>
				{/each}
			</div>
		{/if}
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
		{#if c.agents.codex}
			{@const out = c.agents.codex.signed_in === false}
			<Tooltip.Root>
				<Tooltip.Trigger class="inline-flex h-6 items-center gap-1.5 rounded-md border px-2 text-xs {out ? 'border-err/40 text-err-ink' : ''}">
					{#if out}<TriangleAlert class="size-3.5" />{:else if c.agents.codex.signed_in}<CircleCheck class="size-3.5 text-ok" />{/if}
					Codex <span class="tnum text-muted-foreground">{c.agents.codex.version}</span>
				</Tooltip.Trigger>
				<Tooltip.Content>{out ? 'Not signed in: run `codex login` on this machine once' : c.agents.codex.signed_in ? `Signed in; ${c.agents.codex.models?.length ?? 0} models` : 'Sign-in not checked (older Sidefleet)'}</Tooltip.Content>
			</Tooltip.Root>
		{/if}
		{#if c.agents.opencode}
			{@const none = !c.agents.opencode.models_total}
			<Tooltip.Root>
				<Tooltip.Trigger class="inline-flex h-6 items-center gap-1.5 rounded-md border px-2 text-xs {none ? 'border-err/40 text-err-ink' : ''}">
					{#if none}<TriangleAlert class="size-3.5" />{:else}<CircleCheck class="size-3.5 text-ok" />{/if}
					opencode <span class="tnum text-muted-foreground">{none ? c.agents.opencode.version : `${c.agents.opencode.models_total} models`}</span>
				</Tooltip.Trigger>
				<Tooltip.Content>{none ? 'No provider set up: run `opencode auth login` on this machine (e.g. OpenRouter)' : `Version ${c.agents.opencode.version}`}</Tooltip.Content>
			</Tooltip.Root>
		{/if}
		{#if c.agents.ollama}<span class="inline-flex h-6 items-center gap-1.5 rounded-md border px-2 text-xs">Ollama <span class="tnum text-muted-foreground">{c.agents.ollama.models.length} models</span></span>{/if}
		{#if !c.agents.claude && !c.agents.codex && !c.agents.opencode}<span class="inline-flex h-6 items-center rounded-md border border-dashed px-2 text-xs text-muted-foreground">No coding agent found</span>{/if}
		{#each limits as l (l.name)}
			<Tooltip.Root>
				<Tooltip.Trigger class="inline-flex h-6 items-center gap-1.5 rounded-md border border-queue/50 px-2 text-xs text-queue-ink">
					<TriangleAlert class="size-3.5" />{several ? `${l.name}: ` : ''}usage limit until <span class="tnum">{l.until}</span>
				</Tooltip.Trigger>
				<Tooltip.Content>This machine's {l.name} account hit its usage limit. Its {l.name} tasks go to other machines until then.</Tooltip.Content>
			</Tooltip.Root>
		{/each}
		<button type="button" onclick={() => (fleet.pickerOpen = true)} class="inline-flex h-6 items-center rounded-md px-2 text-xs text-muted-foreground hover:bg-accent hover:text-foreground">
			<span class="tnum">{repos} repos{fleet.added?.size ? `, ${chosen} chosen` : ''}</span>
		</button>
	</div>

	<footer class="mt-auto flex items-center justify-between gap-3 border-t pt-3.5 text-xs">
		<span class="min-w-0 truncate text-muted-foreground" title={c.version}>
			{#if build.sha}Sidefleet {build.version}, build <span class="font-mono">{build.sha}</span>{:else}Version unknown{/if}
		</span>
		{#if !w.online}
			<button type="button" class="shrink-0 text-muted-foreground hover:text-err-ink" onclick={remove}>Remove</button>
		{:else if w.update_requested}
			<span class="flex shrink-0 items-center gap-1.5 text-run-ink"><RefreshCw class="size-3.5 animate-spin [animation-duration:2.5s]" />Updating when idle</span>
		{:else if w.outdated && w.online}
			<Button size="sm" variant="outline" class="h-7 shrink-0 rounded-full border-queue/50 text-queue-ink" onclick={update} disabled={requesting}>Update to latest</Button>
		{:else if w.online && c.version}
			<button type="button" class="shrink-0 text-muted-foreground hover:text-foreground" onclick={update} disabled={requesting}>Up to date</button>
		{/if}
	</footer>
</article>
