<script lang="ts">
	import { tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { fleet } from '../state.svelte';
	import { seen } from '../seen.svelte';
	import { inbox, reasonTone } from '../inbox.svelte';
	import { agentModel, ago, dayGroup, duration, modelName, repoLabel, scheduled, startTime } from '../format';
	import type { RunSummary } from '../types';
	import StatusDot from './StatusDot.svelte';
	import PrIcon from './PrIcon.svelte';
	import AgentIcon from './AgentIcon.svelte';
	import Avatar from './Avatar.svelte';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import GitFork from '@lucide/svelte/icons/git-fork';
	import Archive from '@lucide/svelte/icons/archive';
	import ArchiveRestore from '@lucide/svelte/icons/archive-restore';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import AlarmClock from '@lucide/svelte/icons/alarm-clock';
	import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { prefs } from '../prefs.svelte';
	import { isSnoozed, localInput, presetTime, wakeLabel } from '../snooze';

	let { query = '', activeId = null }: { query?: string; activeId?: string | null } = $props();

	let scroller = $state<HTMLElement | null>(null);
	let showArchived = $state(false);
	let showSnoozed = $state(false);

	// "Pick a date and time…" for one thread.
	let picking = $state<RunSummary | null>(null);
	let pickAt = $state('');
	function pick(r: RunSummary) {
		picking = r;
		pickAt = localInput(presetTime({ label: '', time: '09:00', day: 'tomorrow' }, Date.now()));
	}
	const pickedTime = $derived(pickAt ? new Date(pickAt) : null);
	const pickOk = $derived(!!pickedTime && !isNaN(pickedTime.getTime()) && pickedTime.getTime() > fleet.now);
	function snoozePicked(e: Event) {
		e.preventDefault();
		if (!picking || !pickOk) return;
		void fleet.setSnoozed(picking, pickedTime);
		picking = null;
	}

	function toggleArchived() {
		showArchived = !showArchived;
		if (showArchived) void fleet.loadArchived();
	}

	const live = (r: RunSummary) => r.thread_status === 'running' || r.thread_status === 'queued';

	// With teammates: your own tasks, or everyone's. The choice sticks.
	const SCOPE = 'sidefleet_runs_scope';
	let scope = $state<'mine' | 'team'>('mine');
	try { if (localStorage.getItem(SCOPE) === 'team') scope = 'team'; } catch { /* storage blocked */ }
	function setScope(s: 'mine' | 'team') {
		scope = s;
		try { localStorage.setItem(SCOPE, s); } catch { /* storage blocked */ }
	}
	const filtering = $derived(fleet.multi && !!fleet.userId);
	const scoped = $derived(filtering && scope === 'mine' ? fleet.runs.filter((r) => r.created_by === fleet.userId) : fleet.runs);
	// Snoozed threads wait in their own section and come back on their own when the time comes.
	const runs = $derived(scoped.filter((r) => !isSnoozed(r, fleet.now)));
	const snoozed = $derived(scoped.filter((r) => isSnoozed(r, fleet.now)).sort((a, b) => a.snoozed_until!.localeCompare(b.snoozed_until!)));
	const theirs = $derived(filtering ? fleet.runs.filter((r) => r.created_by !== fleet.userId && !isSnoozed(r, fleet.now)).length : 0);

	/** The repo most runs are in; rows from any other repo say which one. */
	const homeRepo = $derived.by(() => {
		const n = new Map<string, number>();
		runs.forEach((r) => n.set(r.repo, (n.get(r.repo) ?? 0) + 1));
		return [...n.entries()].sort((a, b) => b[1] - a[1])[0]?.[0] ?? '';
	});

	const q = $derived(query.trim().toLowerCase());
	// Archived tasks are searched too once their list has been opened.
	const matches = $derived(
		q ? [...fleet.runs, ...(fleet.archived ?? [])].filter((r) => [r.task, repoLabel(r.repo), r.worker_id ?? ''].some((s) => s.toLowerCase().includes(q))) : []
	);

	// Threads waiting on you first, then running, then queued, then those scheduled for later (soonest first).
	const later = (r: RunSummary) => scheduled(r, fleet.now);
	const rank = (r: RunSummary) => (r.pending_approvals ? 0 : r.thread_status === 'running' ? 1 : later(r) ? 3 : 2);
	const byStart = (a: RunSummary, b: RunSummary) => (later(a) && later(b) ? new Date(a.start_at!).getTime() - new Date(b.start_at!).getTime() : 0);
	const active = $derived(runs.filter(live).sort((a, b) => rank(a) - rank(b) || byStart(a, b)));
	const groups = $derived.by(() => {
		const out: { label: string; runs: RunSummary[] }[] = [];
		for (const r of runs) {
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

{#snippet row(r: RunSummary, detail: boolean, asleep = false)}
	{@const on = r.id === activeId}
	{@const waiting = on ? undefined : inbox.byId.get(r.id)}
	{@const unread = !on && (!!waiting || seen.unread(r))}
	{@const other = r.repo !== homeRepo}
	{@const asking = !!r.pending_approvals && live(r)}
	<div class="group/row relative">
	<a
		href="#/runs/{r.id}"
		data-run={r.id}
		aria-current={on ? 'page' : undefined}
		title={r.task}
		class="group flex items-start gap-2.5 rounded-md px-2.5 py-[7px] text-[13px] leading-[18px] transition-colors pointer-coarse:pr-9 {on
			? 'bg-sidebar-accent text-foreground'
			: 'text-foreground/80 hover:bg-sidebar-accent/60 hover:text-foreground'}"
	>
		<span class="grid h-[18px] w-2 shrink-0 place-items-center">
			{#if asking}
				<span aria-hidden="true" class="inline-block size-1.5 shrink-0 rounded-full bg-queue pulse-run"></span>
			{:else}
				<StatusDot status={r.thread_status} class={r.thread_status === 'succeeded' && !unread ? 'opacity-45' : ''} />
			{/if}
		</span>
		<span class="flex min-w-0 flex-1 flex-col">
			<span class="flex min-w-0 items-center gap-1.5">
				{#if filtering && fleet.othersRun(r)}<Avatar login={fleet.login(r.created_by)} size={14} class="ring-0" />{/if}
				<span class="min-w-0 flex-1 truncate {unread ? 'font-medium text-foreground' : ''}">{r.task}</span>
				{#if r.pr}
					<span class="tnum flex shrink-0 items-center gap-0.5 text-[11px] text-muted-foreground group-hover/row:invisible" title="Pull request #{r.pr.number}: {r.pr.state}">
						<PrIcon state={r.pr.state} />{r.pr.number}
					</span>
				{/if}
				{#if waiting}
					<span class="size-2 shrink-0 rounded-full {reasonTone(waiting.reason)} ring-2 ring-sidebar group-hover/row:invisible" role="img" aria-label="Waiting on you: {waiting.label}" title={waiting.label}></span>
				{:else if unread}
					<span class="size-1.5 shrink-0 rounded-full bg-foreground group-hover/row:invisible" aria-label="New"></span>
				{:else if !detail && !asleep}
					<span class="tnum shrink-0 text-[11px] text-faint group-hover:text-muted-foreground group-hover/row:invisible">{ago(r.last_activity_at, fleet.now)}</span>
				{/if}
			</span>
			<span class="flex min-w-0 items-center gap-1.5 text-[11.5px] text-muted-foreground">
				{#if r.isolation === 'checkout'}<span class="shrink-0" title="In the repo's own checkout, not a worktree"><FolderGit2 class="size-3 text-faint" /></span>{/if}
				<span class="tnum min-w-0 flex-1 truncate">
					{#if asking}
						<span class="font-medium text-queue-ink">Waiting on you{#if r.pending_approvals! > 1}&nbsp;· {r.pending_approvals} tool calls{/if}</span>
					{:else if detail}
						{#if r.thread_status === 'running'}
							{r.latest_worker_id ?? 'Starting'} · {duration(r.started_at, null, fleet.now) || 'starting'}
						{:else if r.start_at && later(r)}
							Scheduled · {startTime(r.start_at, fleet.now)}{#if other}&nbsp;· {repoLabel(r.repo)}{/if}
						{:else}
							Waiting for a machine{#if other}&nbsp;· {repoLabel(r.repo)}{/if}
						{/if}
					{:else if asleep}
						<AlarmClock class="mr-1 inline size-3 align-[-2px]" />Back {wakeLabel(r.snoozed_until!, fleet.now)}{#if other}&nbsp;· {repoLabel(r.repo)}{/if}
					{:else}
						{[other ? repoLabel(r.repo) : null, fleet.workers.length > 1 ? r.latest_worker_id : null].filter(Boolean).join(' · ')}
					{/if}
				</span>
				<span class="flex max-w-[55%] shrink-0 items-center gap-1 text-faint" title="{agentModel(r.latest_agent, r.latest_model)}{r.latest_effort ? ` · ${r.latest_effort}` : ''}">
					{#if !asking}<span class="truncate text-[11px]">{r.latest_model ? modelName(r.latest_model, r.latest_agent) : 'default'}{#if r.latest_effort}&nbsp;· {r.latest_effort}{/if}</span>{/if}
					<AgentIcon agent={r.latest_agent} class="size-3 {r.latest_agent === 'claude' ? 'opacity-80' : ''}" />
				</span>
			</span>
		</span>
	</a>
	<DropdownMenu.Root>
		<DropdownMenu.Trigger
			class="absolute top-[5px] right-1.5 grid size-[22px] place-items-center rounded text-muted-foreground opacity-0 transition-opacity group-hover/row:opacity-100 hover:bg-sidebar-accent hover:text-foreground focus-visible:opacity-100 data-[state=open]:bg-sidebar-accent data-[state=open]:opacity-100 pointer-coarse:opacity-100"
			aria-label="Actions for {r.task.slice(0, 60)}"
		>
			<Ellipsis class="size-3.5" />
		</DropdownMenu.Trigger>
		<DropdownMenu.Content align="start" side="right" class="w-52">
			<DropdownMenu.Item onclick={() => { fleet.forkThread = r.id; void goto('#/runs/' + r.id); }}>
				<GitFork class="size-3.5" /><span class="flex flex-col"><span>{fleet.othersRun(r) ? 'Continue on my machine…' : 'Fork…'}</span><span class="text-xs text-muted-foreground">New thread from its latest message, any agent, model or machine{fleet.othersRun(r) ? ' of yours' : ''}</span></span>
			</DropdownMenu.Item>
			{#if r.archived_at}
				<DropdownMenu.Item onclick={() => fleet.setArchived(r, false)}><ArchiveRestore class="size-3.5" />Unarchive</DropdownMenu.Item>
			{:else}
				{#if isSnoozed(r, fleet.now)}
					<DropdownMenu.Item onclick={() => fleet.setSnoozed(r, null)}><AlarmClock class="size-3.5" />Unsnooze</DropdownMenu.Item>
				{/if}
				<DropdownMenu.Sub>
					<DropdownMenu.SubTrigger><AlarmClock class="size-3.5" />{isSnoozed(r, fleet.now) ? 'Snooze until…' : 'Snooze'}</DropdownMenu.SubTrigger>
					<DropdownMenu.SubContent class="min-w-56">
						{#each prefs.snoozePresets as p, i (i)}
							<DropdownMenu.Item onclick={() => fleet.setSnoozed(r, presetTime(p, Date.now()))}>
								<span class="flex-1 whitespace-nowrap">{p.label}</span><span class="tnum ml-4 text-xs whitespace-nowrap text-muted-foreground">{wakeLabel(presetTime(p, fleet.now), fleet.now)}</span>
							</DropdownMenu.Item>
						{/each}
						<DropdownMenu.Separator />
						<DropdownMenu.Item onclick={() => pick(r)}>Pick a date and time…</DropdownMenu.Item>
						<DropdownMenu.Item onclick={() => void goto('#/settings')}><span class="text-muted-foreground">Edit these choices…</span></DropdownMenu.Item>
					</DropdownMenu.SubContent>
				</DropdownMenu.Sub>
				<DropdownMenu.Item onclick={() => fleet.setArchived(r, true)}><Archive class="size-3.5" />Archive</DropdownMenu.Item>
			{/if}
		</DropdownMenu.Content>
	</DropdownMenu.Root>
	</div>
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
	{:else}
		{#if filtering}
			<div class="mx-1 mt-3 grid grid-cols-2 rounded-md bg-sidebar-accent/60 p-0.5 text-[12px]" role="tablist" aria-label="Whose tasks">
				{#each [['mine', 'Mine'], ['team', 'Team']] as [id, label] (id)}
					<button
						type="button"
						role="tab"
						aria-selected={scope === id}
						class="rounded px-2 py-1 transition-colors {scope === id ? 'bg-background font-medium text-foreground shadow-xs' : 'text-muted-foreground hover:text-foreground'}"
						onclick={() => setScope(id as 'mine' | 'team')}
					>{label}{#if id === 'team' && theirs}<span class="tnum ml-1 text-faint">{theirs}</span>{/if}</button>
				{/each}
			</div>
		{/if}
	{/if}
	{#if fleet.loaded && !q && !runs.length}
		<div class="mx-1 mt-4 rounded-lg border border-dashed border-sidebar-border px-3 py-4 text-xs leading-relaxed text-muted-foreground">
			{#if filtering && scope === 'mine' && fleet.runs.length}
				You haven't started a task here yet. Switch to Team to see what everyone else is running.
			{:else}
				No tasks yet. Each one you start shows up here, so you can jump between them.
			{/if}
		</div>
	{:else if fleet.loaded && !q}
		{#if active.length}
			{@render heading('Active', active.length)}
			{#each active as r (r.id)}{@render row(r, true)}{/each}
		{/if}
		{#each groups as g (g.label)}
			{@render heading(g.label)}
			{#each g.runs as r (r.id)}{@render row(r, false)}{/each}
		{/each}
		{#if fleet.runs.length >= 100}
			<p class="px-2.5 pt-4 text-[11px] text-faint">Showing the latest {fleet.runs.length} tasks{filtering ? ' in the team' : ''}.</p>
		{/if}
	{/if}
	{#if fleet.loaded && !q && snoozed.length}
		<button
			type="button"
			aria-expanded={showSnoozed}
			onclick={() => (showSnoozed = !showSnoozed)}
			class="mt-3 flex w-full items-center gap-1 rounded-md px-2.5 py-1 text-[11px] font-medium text-faint hover:text-muted-foreground"
		>
			<ChevronRight class="size-3 transition-transform {showSnoozed ? 'rotate-90' : ''}" />Snoozed
			<span class="tnum font-normal">{snoozed.length}</span>
		</button>
		{#if showSnoozed}
			{#each snoozed as r (r.id)}{@render row(r, false, true)}{/each}
			<p class="px-2.5 pt-1 text-[11px] leading-relaxed text-faint">Each comes back at its time, or sooner if it finishes, fails or asks you something.</p>
		{/if}
	{/if}
	{#if fleet.loaded && !q}
		<button
			type="button"
			aria-expanded={showArchived}
			onclick={toggleArchived}
			class="mt-3 flex w-full items-center gap-1 rounded-md px-2.5 py-1 text-[11px] font-medium text-faint hover:text-muted-foreground"
		>
			<ChevronRight class="size-3 transition-transform {showArchived ? 'rotate-90' : ''}" />Archived
			{#if fleet.archived?.length}<span class="tnum font-normal">{fleet.archived.length}</span>{/if}
		</button>
		{#if showArchived}
			{#if !fleet.archived}
				<p class="px-2.5 py-1 text-xs text-muted-foreground">Loading…</p>
			{:else if !fleet.archived.length}
				<p class="px-2.5 py-1 text-xs text-muted-foreground">Nothing archived. Archive a task from its ⋯ menu to tidy this list.</p>
			{:else}
				{#each fleet.archived as r (r.id)}{@render row(r, false)}{/each}
			{/if}
		{/if}
	{/if}
</div>

<Dialog.Root open={!!picking} onOpenChange={(o) => { if (!o) picking = null; }}>
	<Dialog.Content class="sm:max-w-[24rem]">
		<Dialog.Header>
			<Dialog.Title>Snooze until…</Dialog.Title>
			<Dialog.Description class="truncate">{picking?.task}</Dialog.Description>
		</Dialog.Header>
		<form class="flex flex-col gap-3" onsubmit={snoozePicked}>
			<input
				type="datetime-local"
				bind:value={pickAt}
				min={localInput(new Date(fleet.now))}
				aria-label="Snooze until"
				class="h-9 w-full rounded-lg border border-input bg-card px-3 text-[14px] outline-none focus:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]"
			/>
			<p class="text-[12px] text-muted-foreground">
				{#if pickOk}It comes back {wakeLabel(pickedTime!, fleet.now)}, or sooner if it finishes, fails or asks you something.{:else}Pick a time ahead.{/if}
			</p>
			<Button type="submit" class="h-9 rounded-full" disabled={!pickOk}>Snooze</Button>
		</form>
	</Dialog.Content>
</Dialog.Root>
