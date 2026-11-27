<script lang="ts">
	// What agents did: tasks, tool calls, permission requests and decisions, newest first.
	import { onMount, untrack } from 'svelte';
	import * as Select from '#lib/components/ui/select';
	import { Button } from '#lib/components/ui/button';
	import { listActivity, listAgents, EVENT_TYPES, type ActivityEvent, type ActivityFilter, type Agent } from '#lib/sf/control';
	import Chip from '#lib/sf/ui/control/Chip.svelte';
	import PageHead from '#lib/sf/ui/control/PageHead.svelte';
	import Pills from '#lib/sf/ui/control/Pills.svelte';
	import ActivityRows from '#lib/sf/ui/control/ActivityRows.svelte';

	let agents = $state<Agent[]>([]);
	let events = $state<ActivityEvent[] | null>(null);
	let counts = $state<Record<string, number>>({});
	let error = $state('');
	let more = $state(false);

	let agent = $state('all');
	let owner = $state('all');
	let team = $state('all');
	let type = $state('all');
	let range = $state('today');
	let outcome = $state('all');

	const PAGE = 100;
	const since = (r: string) => {
		const d = new Date();
		if (r === 'today') d.setHours(0, 0, 0, 0);
		else d.setDate(d.getDate() - (r === '7d' ? 7 : 30));
		return d.toISOString();
	};
	const filter = (): ActivityFilter => ({
		agent: agent === 'all' ? undefined : agent,
		owner: owner === 'all' ? undefined : owner,
		team: team === 'all' ? undefined : team,
		type: type === 'all' ? undefined : type,
		outcome: outcome === 'all' ? undefined : outcome,
		since: since(range),
		limit: PAGE
	});

	let seq = 0;
	async function load() {
		const mine = ++seq;
		try {
			const r = await listActivity(filter());
			if (mine !== seq) return;
			if (events && events.length > PAGE) {
				// Older pages are loaded: put what's new on top and keep them.
				const ids = new Set(events.map((e) => e.id));
				events = [...r.events.filter((e) => !ids.has(e.id)), ...events];
			} else {
				events = r.events;
				more = r.events.length === PAGE;
			}
			counts = r.counts ?? {};
			error = '';
		} catch (e) {
			if (mine === seq) error = (e as Error).message;
		}
	}
	async function loadMore() {
		const last = events?.at(-1);
		if (!last) return;
		try {
			const r = await listActivity({ ...filter(), before: last.id });
			events = [...(events ?? []), ...r.events];
			more = r.events.length === PAGE;
		} catch (e) {
			error = (e as Error).message;
		}
	}
	// A new filter starts from the top.
	$effect(() => {
		void [agent, owner, team, type, range, outcome];
		untrack(() => {
			events = null;
			void load();
		});
	});
	onMount(() => {
		void listAgents().then((a) => (agents = a)).catch(() => {});
		const t = setInterval(load, 10_000);
		return () => clearInterval(t);
	});

	const owners = $derived([...new Map(agents.filter((a) => a.owner).map((a) => [a.owner!.id, a.owner!.login])).entries()].sort((a, b) => a[1].localeCompare(b[1])));
	const teams = $derived([...new Set(agents.map((a) => a.team).filter(Boolean))].sort());
	const total = $derived(Object.values(counts).reduce((s, n) => s + n, 0));
	const pills = $derived([
		{ id: 'all', label: 'All', n: total },
		{ id: 'allowed', label: 'Allowed', n: counts.allowed ?? 0 },
		{ id: 'approval_required', label: 'Needs approval', n: counts.approval_required ?? 0 },
		{ id: 'denied', label: 'Denied', n: counts.denied ?? 0 },
		{ id: 'failed', label: 'Failed', n: counts.failed ?? 0 }
	]);
	const RANGES: Record<string, string> = { today: 'Today', '7d': 'Last 7 days', '30d': 'Last 30 days' };
	const agentLabel = $derived(agent === 'all' ? 'All agents' : (agents.find((a) => a.id === agent)?.name ?? 'Agent'));
	const ownerLabel = $derived(owner === 'all' ? 'Anyone' : (owners.find(([id]) => id === owner)?.[1] ?? 'Owner'));
</script>

<svelte:head><title>Activity · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-4.5 px-4 pt-8 pb-[calc(3rem+env(safe-area-inset-bottom))] md:px-8">
	<PageHead title="Activity" sub="Tasks, tool calls, permission requests and decisions from every instrumented agent. Secrets never appear here.">
		{#snippet actions()}<Chip tone="run">Live</Chip>{/snippet}
	</PageHead>

	<div class="grid grid-cols-[repeat(auto-fit,minmax(150px,1fr))] gap-3 rounded-xl border bg-card px-4 py-3.5">
		<div class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Agent
			<Select.Root type="single" bind:value={agent}>
				<Select.Trigger class="h-8.5 w-full text-[13px] text-foreground" aria-label="Agent">{agentLabel}</Select.Trigger>
				<Select.Content>
					<Select.Item value="all" label="All agents">All agents</Select.Item>
					{#each agents as a (a.id)}<Select.Item value={a.id} label={a.name}>{a.name}</Select.Item>{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Owner
			<Select.Root type="single" bind:value={owner}>
				<Select.Trigger class="h-8.5 w-full text-[13px] text-foreground" aria-label="Owner">{ownerLabel}</Select.Trigger>
				<Select.Content>
					<Select.Item value="all" label="Anyone">Anyone</Select.Item>
					{#each owners as [id, login] (id)}<Select.Item value={id} label={login}>{login}</Select.Item>{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Team
			<Select.Root type="single" bind:value={team}>
				<Select.Trigger class="h-8.5 w-full text-[13px] text-foreground" aria-label="Team">{team === 'all' ? 'All teams' : team}</Select.Trigger>
				<Select.Content>
					<Select.Item value="all" label="All teams">All teams</Select.Item>
					{#each teams as t (t)}<Select.Item value={t} label={t}>{t}</Select.Item>{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Event type
			<Select.Root type="single" bind:value={type}>
				<Select.Trigger class="h-8.5 w-full text-[13px] text-foreground" aria-label="Event type">{type === 'all' ? 'All events' : EVENT_TYPES[type]}</Select.Trigger>
				<Select.Content>
					<Select.Item value="all" label="All events">All events</Select.Item>
					{#each Object.entries(EVENT_TYPES) as [id, label] (id)}<Select.Item value={id} {label}>{label}</Select.Item>{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Date range
			<Select.Root type="single" bind:value={range}>
				<Select.Trigger class="h-8.5 w-full text-[13px] text-foreground" aria-label="Date range">{RANGES[range]}</Select.Trigger>
				<Select.Content>
					{#each Object.entries(RANGES) as [id, label] (id)}<Select.Item value={id} {label}>{label}</Select.Item>{/each}
				</Select.Content>
			</Select.Root>
		</div>
	</div>

	<div class="flex flex-wrap items-center gap-1.5">
		<span class="mr-1 text-[12px] text-muted-foreground">Policy outcome</span>
		<Pills options={pills} bind:value={outcome} label="Policy outcome" />
	</div>

	{#if error && !events}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">Could not load activity: {error}</p>
	{:else if !events}
		<p class="text-[13px] text-muted-foreground">Loading…</p>
	{:else}
		<div class="rounded-xl border bg-card">
			<ActivityRows {events} empty="Nothing matches these filters." />
		</div>
		{#if more}<Button variant="outline" size="sm" class="self-center" onclick={loadMore}>Show older</Button>{/if}
	{/if}
</section>
