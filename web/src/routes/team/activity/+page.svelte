<script lang="ts">
	// Team → Activity log: who did what in this org. Owners see everything; everyone else
	// sees what happened on their own machines and what they did themselves (the server
	// decides). Filters, older pages, and the same filtered entries as CSV or JSON.
	import { Button } from '#lib/components/ui/button';
	import * as Select from '#lib/components/ui/select';
	import { untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import Download from '@lucide/svelte/icons/download';
	import History from '@lucide/svelte/icons/history';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { ACTION_GROUPS, auditQuery, dayBound, downloadAudit, type AuditFilter } from '#lib/sf/audit';
	import type { AuditEvent, AuditPage } from '#lib/sf/types';
	import AuditList from '#lib/sf/ui/AuditList.svelte';

	const org = $derived(fleet.org);
	const orgId = $derived(org?.id ?? null);

	let group = $state('all');
	let actor = $state('anyone');
	let machine = $state('any');
	let from = $state('');
	let to = $state('');

	const filter = $derived<AuditFilter>({
		action: group === 'all' ? undefined : group,
		actor: actor === 'anyone' ? undefined : actor,
		machine: machine === 'any' ? undefined : machine,
		since: dayBound(from),
		until: dayBound(to, true)
	});
	const filtered = $derived(group !== 'all' || actor !== 'anyone' || machine !== 'any' || !!from || !!to);

	let events = $state<AuditEvent[] | null>(null);
	let next = $state<number | null>(null);
	let error = $state('');
	let loading = $state(false);
	let exporting = $state(false);
	// Only the newest request may write the list (filters can change while one is in flight).
	let ticket = 0;

	async function load(more: boolean) {
		const id = orgId;
		if (!id) return;
		const mine = ++ticket;
		loading = true;
		try {
			const page = await api<AuditPage>(`/v1/orgs/${id}/audit${auditQuery(filter, { limit: 50, before: more ? next : null })}`);
			if (mine !== ticket) return;
			events = more ? [...(events ?? []), ...page.events] : page.events;
			next = page.next;
			error = '';
		} catch (err) {
			if (mine !== ticket) return;
			error = (err as Error).message;
			if (!more) events = [];
		}
		loading = false;
	}

	// The "Who" menu lists the org's people.
	$effect(() => {
		if (orgId) untrack(() => void fleet.reloadMembers());
	});
	$effect(() => {
		void orgId;
		void filter;
		untrack(() => {
			events = null;
			next = null;
			void load(false);
		});
	});

	async function exportAs(format: 'csv' | 'json') {
		if (!orgId) return;
		exporting = true;
		try {
			const { truncated } = await downloadAudit(orgId, filter, format);
			if (truncated) toast('Exported the newest 10,000 entries', { description: 'Narrow the filter or the dates to export the rest.' });
		} catch (err) {
			toast.error('Could not export', { description: (err as Error).message });
		}
		exporting = false;
	}

	function reset() {
		group = 'all';
		actor = 'anyone';
		machine = 'any';
		from = '';
		to = '';
	}

	const groupLabel = $derived(ACTION_GROUPS.find((g) => g.id === group)?.label ?? 'Everything');
	const actorLabel = $derived(actor === 'anyone' ? 'Anyone' : actor === 'system' ? 'Sidefleet' : actor.startsWith('worker:') ? actor.slice(7) : `@${actor}`);
	const machines = $derived([...new Set(fleet.workers.map((w) => w.id))].sort());
</script>

<svelte:head><title>Activity log · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 py-8 md:px-8">
	<header class="flex flex-col gap-1.5">
		<a href="#/team" class="inline-flex w-fit items-center gap-1 text-xs text-muted-foreground hover:text-foreground"><ArrowLeft class="size-3" />{org?.name ?? 'Team'}</a>
		<h1 class="flex items-center gap-2 text-[22px] font-semibold tracking-tight"><History class="size-5" />Activity log</h1>
		<p class="text-[13px] leading-relaxed text-muted-foreground">
			{#if fleet.isOwner}
				Who did what in {org?.name ?? 'this org'}: tasks, approvals, machines, tokens and people. It never holds task text, agent output or secrets.
			{:else}
				What happened on your machines and what you did in {org?.name ?? 'this org'}. Owners of the org see everyone's.
			{/if}
		</p>
	</header>

	<div class="flex flex-wrap items-end gap-2">
		<Select.Root type="single" bind:value={group}>
			<Select.Trigger size="sm" class="h-8 rounded-full text-[13px]" aria-label="What">{groupLabel}</Select.Trigger>
			<Select.Content>
				<Select.Item value="all" label="Everything">Everything</Select.Item>
				{#each ACTION_GROUPS as g (g.id)}
					<Select.Item value={g.id} label={g.label}>{g.label}</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
		<Select.Root type="single" bind:value={actor}>
			<Select.Trigger size="sm" class="h-8 rounded-full text-[13px]" aria-label="Who">{actorLabel}</Select.Trigger>
			<Select.Content>
				<Select.Item value="anyone" label="Anyone">Anyone</Select.Item>
				{#each fleet.members as m (m.id)}
					<Select.Item value={m.login} label="@{m.login}">@{m.login}</Select.Item>
				{/each}
				{#each machines as id (id)}
					<Select.Item value="worker:{id}" label={id}><span class="font-mono text-[12px]">{id}</span></Select.Item>
				{/each}
				<Select.Item value="system" label="Sidefleet">Sidefleet</Select.Item>
			</Select.Content>
		</Select.Root>
		{#if machines.length}
			<Select.Root type="single" bind:value={machine}>
				<Select.Trigger size="sm" class="h-8 rounded-full text-[13px]" aria-label="Machine">{machine === 'any' ? 'Any machine' : machine}</Select.Trigger>
				<Select.Content>
					<Select.Item value="any" label="Any machine">Any machine</Select.Item>
					{#each machines as id (id)}
						<Select.Item value={id} label={id}><span class="font-mono text-[12px]">{id}</span></Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		{/if}
		<label class="flex items-center gap-1.5 text-xs text-muted-foreground">From
			<input type="date" bind:value={from} max={to || undefined} class="h-8 rounded-full border bg-background px-3 text-[13px] text-foreground" />
		</label>
		<label class="flex items-center gap-1.5 text-xs text-muted-foreground">To
			<input type="date" bind:value={to} min={from || undefined} class="h-8 rounded-full border bg-background px-3 text-[13px] text-foreground" />
		</label>
		{#if filtered}<button type="button" class="h-8 px-2 text-xs text-muted-foreground hover:text-foreground" onclick={reset}>Clear</button>{/if}
		<div class="ml-auto flex items-center gap-1.5">
			<Button variant="outline" size="sm" class="h-8 rounded-full text-[13px]" disabled={exporting || !orgId} onclick={() => exportAs('csv')}><Download class="size-3.5" />CSV</Button>
			<Button variant="outline" size="sm" class="h-8 rounded-full text-[13px]" disabled={exporting || !orgId} onclick={() => exportAs('json')}><Download class="size-3.5" />JSON</Button>
		</div>
	</div>

	{#if error}
		<p class="rounded-xl border border-err/40 bg-err-soft/40 p-4 text-[13px] text-err-ink">Could not load the activity log: {error}</p>
	{/if}
	{#if events === null}
		<p class="text-[13px] text-muted-foreground">Loading…</p>
	{:else if !events.length && !error}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">{filtered ? 'Nothing matches these filters.' : 'Nothing has happened here yet.'}</p>
	{:else if events.length}
		<AuditList {events} now={fleet.now} />
		{#if next !== null}
			<Button variant="outline" size="sm" class="h-8 w-fit self-center rounded-full text-[13px]" disabled={loading} onclick={() => load(true)}>{loading ? 'Loading…' : 'Older'}</Button>
		{/if}
	{/if}
</section>
