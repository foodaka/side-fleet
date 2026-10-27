<script lang="ts">
	import { fleet } from '#lib/sf/state.svelte';
	import { agentModel, duration, money, rel, repoLabel } from '#lib/sf/format';
	import StatusPill from '#lib/sf/ui/StatusPill.svelte';
	import StatusDot from '#lib/sf/ui/StatusDot.svelte';
	import type { RunSummary } from '#lib/sf/types';

	type F = 'all' | 'running' | 'queued' | 'problems' | 'done';
	let filter = $state<F>('all');
	const match: Record<F, (r: RunSummary) => boolean> = {
		all: () => true,
		running: (r) => r.thread_status === 'running',
		queued: (r) => r.thread_status === 'queued',
		problems: (r) => r.thread_status === 'failed' || r.thread_status === 'stopped',
		done: (r) => r.thread_status === 'succeeded'
	};
	const count = (f: F) => fleet.runs.filter(match[f]).length;
	const list = $derived(fleet.runs.filter(match[filter]));
	const tabs: { id: F; label: string }[] = [
		{ id: 'all', label: 'All' },
		{ id: 'running', label: 'Running' },
		{ id: 'queued', label: 'Queued' },
		{ id: 'problems', label: 'Failed or stopped' },
		{ id: 'done', label: 'Done' }
	];

	const today = new Date().toDateString();
	const doneToday = $derived(fleet.runs.filter((r) => r.thread_status === 'succeeded' && new Date(r.last_activity_at).toDateString() === today).length);
	const spend = $derived(fleet.runs.reduce((a, r) => a + (Number(r.result?.total_cost_usd) || 0), 0));
	const busyMachines = $derived(new Set(fleet.runs.filter((r) => r.thread_status === 'running').map((r) => r.worker_id)).size);
	const workerOnline = (id: string | null) => !!fleet.workers.find((w) => w.id === id)?.online;
</script>

<svelte:head><title>Overview · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-6 px-4 pt-8 pb-[calc(2rem+env(safe-area-inset-bottom))] md:px-8">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-[22px] font-semibold tracking-tight">Overview</h1>
			<p class="mt-1 text-[13px] text-muted-foreground">What the fleet is doing, and every task it has run, newest activity first.</p>
		</div>
	</header>

	<dl class="grid grid-cols-2 overflow-hidden rounded-xl border bg-card sm:grid-cols-4">
		<div class="flex flex-col gap-1 border-b p-4 sm:border-r sm:border-b-0">
			<dt class="text-xs text-muted-foreground">Running now</dt>
			<dd class="tnum text-2xl font-medium tracking-tight">{count('running')}</dd>
			<dd class="text-xs text-muted-foreground">{busyMachines ? `on ${busyMachines} machine${busyMachines === 1 ? '' : 's'}` : 'all machines idle'}</dd>
		</div>
		<div class="flex flex-col gap-1 border-b p-4 sm:border-r sm:border-b-0">
			<dt class="text-xs text-muted-foreground">Waiting</dt>
			<dd class="tnum text-2xl font-medium tracking-tight">{count('queued')}</dd>
			<dd class="text-xs text-muted-foreground">{count('queued') ? 'for a machine with the repo' : 'nothing in the queue'}</dd>
		</div>
		<div class="flex flex-col gap-1 border-r p-4 sm:border-r">
			<dt class="text-xs text-muted-foreground">Done today</dt>
			<dd class="tnum text-2xl font-medium tracking-tight">{doneToday}</dd>
			<dd class="text-xs text-muted-foreground">{count('problems')} failed or stopped in total</dd>
		</div>
		<div class="flex flex-col gap-1 p-4">
			<dt class="text-xs text-muted-foreground">Agent spend</dt>
			<dd class="tnum text-2xl font-medium tracking-tight">{money(spend)}</dd>
			<dd class="text-xs text-muted-foreground">as reported by the agents (Codex reports none)</dd>
		</div>
	</dl>

	<div class="flex flex-wrap gap-1" role="tablist" aria-label="Filter runs">
		{#each tabs as t (t.id)}
			<button
				role="tab"
				aria-selected={filter === t.id}
				onclick={() => (filter = t.id)}
				class="inline-flex h-8 items-center gap-1.5 rounded-full px-3 text-[13px] transition-colors {filter === t.id ? 'bg-primary text-primary-foreground' : 'text-muted-foreground hover:bg-accent hover:text-foreground'}"
			>
				{t.label}<span class="tnum text-xs opacity-60">{count(t.id)}</span>
			</button>
		{/each}
	</div>

	<div class="overflow-hidden rounded-xl border bg-card">
		<div class="hidden grid-cols-[minmax(0,1fr)_9rem_8.5rem_5rem_5rem_6rem] gap-4 border-b px-5 py-2.5 text-xs text-muted-foreground md:grid">
			<span>Task</span><span>Machine</span><span>Status</span><span class="text-right">Took</span><span class="text-right">Cost</span><span class="text-right">Updated</span>
		</div>
		{#each list as r (r.id)}
			{@const cost = Number(r.result?.total_cost_usd)}
			<a href="#/runs/{r.id}" class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 border-b px-5 py-3 last:border-b-0 hover:bg-accent/40 md:grid-cols-[minmax(0,1fr)_9rem_8.5rem_5rem_5rem_6rem]">
				<span class="min-w-0">
					<span class="block truncate text-[13.5px] font-medium">{r.task}</span>
					<span class="mt-0.5 flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
						<span class="truncate font-mono text-[11.5px]">{repoLabel(r.repo)}</span>
						<span class="shrink-0 max-sm:hidden">{agentModel(r.agent, r.model)}</span>
						{#if r.follow_ups}<span class="tnum shrink-0 max-sm:hidden">{r.follow_ups + 1} messages</span>{/if}
						<span class="tnum shrink-0 md:hidden">{rel(r.last_activity_at, fleet.now)}</span>
					</span>
				</span>
				<span class="hidden items-center gap-2 text-[13px] md:flex">
					{#if r.worker_id}<StatusDot status={workerOnline(r.worker_id) ? 'online' : 'offline'} /><span class="truncate">{r.worker_id}</span>
					{:else}<span class="text-muted-foreground">No machine yet</span>{/if}
				</span>
				<span class="justify-self-end md:justify-self-start"><StatusPill status={r.thread_status} /></span>
				<span class="tnum hidden text-right text-[13px] text-muted-foreground md:block">{duration(r.started_at, r.thread_status === 'running' ? null : r.finished_at ?? r.last_activity_at, fleet.now) || '–'}</span>
				<span class="tnum hidden text-right text-[13px] md:block">{cost ? money(cost, 3) : '–'}</span>
				<span class="tnum hidden text-right text-xs text-muted-foreground md:block">{rel(r.last_activity_at, fleet.now)}</span>
			</a>
		{:else}
			<div class="flex flex-col items-center gap-3 px-6 py-14 text-center">
				<p class="text-[13px] text-muted-foreground">{fleet.runs.length ? 'No runs match this filter.' : 'Nothing has run yet.'}</p>
				{#if !fleet.runs.length}<a href="#/" class="text-[13px] font-medium underline underline-offset-2">Start your first task</a>{/if}
			</div>
		{/each}
	</div>
</section>
