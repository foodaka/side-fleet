<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { fleet } from '../state.svelte';
	import { seen } from '../seen.svelte';
	import { inbox, reasonTone } from '../inbox.svelte';
	import { agentModel, ago, dayGroup, duration, modelName, repoKey, repoLabel, scheduled, startTime } from '../format';
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
	import Pin from '@lucide/svelte/icons/pin';
	import PinOff from '@lucide/svelte/icons/pin-off';
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { prefs } from '../prefs.svelte';
	import { isSnoozed, localInput, presetTime, wakeLabel } from '../snooze';
	import { projects, projectName } from '../projects.svelte';
	import ProjectIcon from './ProjectIcon.svelte';
	import ProjectIconPicker from './ProjectIconPicker.svelte';
	import SquarePen from '@lucide/svelte/icons/square-pen';
	import Smile from '@lucide/svelte/icons/smile';
	import EyeOff from '@lucide/svelte/icons/eye-off';
	import ChevronsDownUp from '@lucide/svelte/icons/chevrons-down-up';
	import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';

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
	const awake = $derived(scoped.filter((r) => !isSnoozed(r, fleet.now)));
	// Pinned threads stay at the top, in the order they were pinned; the rest go by what they're doing and when.
	const pinned = $derived(awake.filter((r) => r.pinned_at).sort((a, b) => a.pinned_at!.localeCompare(b.pinned_at!)));
	const runs = $derived(awake.filter((r) => !r.pinned_at));
	const snoozed = $derived(scoped.filter((r) => isSnoozed(r, fleet.now)).sort((a, b) => a.snoozed_until!.localeCompare(b.snoozed_until!)));
	const theirs = $derived(filtering ? fleet.runs.filter((r) => r.created_by !== fleet.userId && !isSnoozed(r, fleet.now)).length : 0);

	/** The repo most runs are in; rows from any other repo say which one. */
	const homeRepo = $derived.by(() => {
		const n = new Map<string, number>();
		awake.forEach((r) => n.set(r.repo, (n.get(r.repo) ?? 0) + 1));
		return [...n.entries()].sort((a, b) => b[1] - a[1])[0]?.[0] ?? '';
	});

	const q = $derived(query.trim().toLowerCase());
	// Archived tasks are searched too once their list has been opened.
	const matches = $derived(
		q ? [...fleet.runs, ...(fleet.archived ?? [])].filter((r) => [r.task, repoLabel(r.repo), r.worker_id ?? '', fleet.workerName(r.latest_worker_id)].some((s) => s.toLowerCase().includes(q))) : []
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

	// By project: one group per repo, the ones waiting on you or busy first, then the latest.
	const SHOWN = 5;
	const byProject = $derived(projects.view === 'project');
	const projectGroups = $derived.by(() => {
		const by = new Map<string, RunSummary[]>();
		for (const r of runs) {
			if (!by.has(r.repo)) by.set(r.repo, []);
			by.get(r.repo)!.push(r);
		}
		const sorted = (rs: RunSummary[]) => rs.sort((a, b) => rank(a) - rank(b) || byStart(a, b) || b.last_activity_at.localeCompare(a.last_activity_at));
		const urgency = (rs: RunSummary[]) => Math.min(...rs.map((r) => (live(r) ? rank(r) : 4)));
		return [...by.entries()]
			.map(([repo, rs]) => ({ repo, runs: sorted(rs), urgency: urgency(rs), latest: rs.reduce((m, r) => (r.last_activity_at > m ? r.last_activity_at : m), '') }))
			.sort((a, b) => a.urgency - b.urgency || b.latest.localeCompare(a.latest));
	});
	const shownGroups = $derived(projectGroups.filter((g) => !projects.hidden.has(g.repo)));
	const hiddenCount = $derived(projectGroups.length - shownGroups.length);
	// Repos one of your machines has now: a new task can start there.
	const myRepos = $derived(new Set(fleet.myOnline.flatMap((w) => (w.capabilities.repos ?? []).map(repoKey))));

	function newTaskIn(repo: string) {
		projects.composeIn = repo;
		void goto('#/');
	}

	onMount(() => void projects.load());

	// Keep the open run in view, e.g. after opening it from somewhere else.
	$effect(() => {
		const id = activeId;
		if (!id || !scroller) return;
		void tick().then(() => scroller?.querySelector(`[data-run="${CSS.escape(id)}"]`)?.scrollIntoView({ block: 'nearest' }));
	});
</script>

{#snippet row(r: RunSummary, detail: boolean, asleep = false, grouped = false)}
	{@const on = r.id === activeId}
	{@const waiting = on ? undefined : inbox.byId.get(r.id)}
	{@const unread = !on && (!!waiting || seen.unread(r))}
	{@const other = !grouped && (byProject || r.repo !== homeRepo)}
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
							<span class="flex min-w-0 gap-1"><span class="truncate">{r.latest_worker_id ? fleet.workerName(r.latest_worker_id) : 'Starting'}</span><span class="shrink-0">· {duration(r.started_at, null, fleet.now) || 'starting'}</span></span>
						{:else if r.start_at && later(r)}
							Scheduled · {startTime(r.start_at, fleet.now)}{#if other}&nbsp;· {@render repoTag(r.repo)}{/if}
						{:else}
							Waiting for a machine{#if other}&nbsp;· {@render repoTag(r.repo)}{/if}
						{/if}
					{:else if asleep}
						<AlarmClock class="mr-1 inline size-3 align-[-2px]" />Back {wakeLabel(r.snoozed_until!, fleet.now)}{#if other}&nbsp;· {@render repoTag(r.repo)}{/if}
					{:else}
						{#if other}{@render repoTag(r.repo)}{/if}{#if other && fleet.workers.length > 1}&nbsp;·&nbsp;{/if}{#if fleet.workers.length > 1}{fleet.workerName(r.latest_worker_id)}{/if}
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
			{#if r.pinned_at}
				<DropdownMenu.Item onclick={() => fleet.setPinned(r, false)}><PinOff class="size-3.5" />Unpin</DropdownMenu.Item>
			{:else}
				<DropdownMenu.Item onclick={() => fleet.setPinned(r, true)}><Pin class="size-3.5" />Pin to top</DropdownMenu.Item>
			{/if}
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

{#snippet repoTag(repo: string)}<span class="inline-flex items-baseline gap-1"><ProjectIcon {repo} size={11} class="self-center" />{byProject ? projectName(repo) : repoLabel(repo)}</span>{/snippet}

{#snippet project(g: { repo: string; runs: RunSummary[] })}
	{@const open = !projects.folded.has(g.repo)}
	{@const asking = g.runs.filter((r) => r.pending_approvals && live(r)).length}
	{@const running = g.runs.filter((r) => r.thread_status === 'running' && !r.pending_approvals).length}
	{@const fresh = g.runs.some((r) => r.id !== activeId && (inbox.byId.has(r.id) || seen.unread(r)))}
	{@const all = projects.showAll.has(g.repo)}
	{@const shown = all ? g.runs : g.runs.filter((r, i) => i < SHOWN || r.id === activeId)}
	<div class="group/proj relative mt-1.5">
		<button
			type="button"
			aria-expanded={open}
			title={g.repo}
			class="flex w-full items-center gap-2 rounded-md py-[5px] pr-9 pl-1.5 text-left transition-colors hover:bg-sidebar-accent/60"
			onclick={() => projects.toggleFolded(g.repo)}
		>
			<ChevronRight class="size-3 shrink-0 text-faint transition-transform {open ? 'rotate-90' : ''}" />
			<ProjectIcon repo={g.repo} size={18} />
			<span class="min-w-0 flex-1 truncate text-[13px] font-semibold text-foreground">{projectName(g.repo)}</span>
			<span class="flex shrink-0 items-center gap-1 group-hover/proj:invisible pointer-coarse:hidden">
				{#if asking}<span class="tnum grid h-4 min-w-4 place-items-center rounded-full bg-queue-soft px-1 text-[10.5px] font-semibold text-queue-ink" title="{asking} waiting on you">{asking}</span>{/if}
				{#if running}<span class="tnum grid h-4 min-w-4 place-items-center rounded-full bg-run-soft px-1 text-[10.5px] font-semibold text-run-ink" title="{running} running">{running}</span>{/if}
				{#if !open && !asking && !running}
					{#if fresh}<span class="size-1.5 rounded-full bg-foreground" aria-label="New"></span>
					{:else}<span class="tnum text-[11px] text-faint">{ago(g.runs[0].last_activity_at, fleet.now)}</span>{/if}
				{/if}
			</span>
		</button>
		<DropdownMenu.Root>
			<DropdownMenu.Trigger
				class="absolute top-1/2 right-1.5 grid size-[22px] -translate-y-1/2 place-items-center rounded text-muted-foreground opacity-0 transition-opacity group-hover/proj:opacity-100 hover:bg-sidebar-accent hover:text-foreground focus-visible:opacity-100 data-[state=open]:bg-sidebar-accent data-[state=open]:opacity-100 pointer-coarse:opacity-100"
				aria-label="Actions for {projectName(g.repo)}"
			>
				<Ellipsis class="size-3.5" />
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="start" side="right" class="w-56">
				<DropdownMenu.Item onclick={() => (projects.picking = g.repo)}><Smile class="size-3.5" />Set icon…</DropdownMenu.Item>
				<DropdownMenu.Item disabled={!myRepos.has(g.repo)} onclick={() => newTaskIn(g.repo)}>
					<SquarePen class="size-3.5" /><span class="flex flex-col"><span>New task in {projectName(g.repo)}</span>{#if !myRepos.has(g.repo)}<span class="text-xs text-muted-foreground">None of your machines online has it</span>{/if}</span>
				</DropdownMenu.Item>
				<DropdownMenu.Separator />
				<DropdownMenu.Item onclick={() => projects.toggleFolded(g.repo)}>
					{#if open}<ChevronsDownUp class="size-3.5" />Fold{:else}<ChevronsUpDown class="size-3.5" />Unfold{/if}
				</DropdownMenu.Item>
				<DropdownMenu.Item onclick={() => projects.hide(g.repo)}><EyeOff class="size-3.5" />Hide from the list</DropdownMenu.Item>
				<DropdownMenu.Separator />
				<DropdownMenu.Label class="truncate text-xs font-normal text-muted-foreground">{g.repo}</DropdownMenu.Label>
			</DropdownMenu.Content>
		</DropdownMenu.Root>
	</div>
	{#if open}
		<div class="ml-[13px] border-l border-sidebar-border pl-1.5">
			{#each shown as r (r.id)}{@render row(r, live(r), false, true)}{/each}
			{#if g.runs.length > SHOWN}
				<button type="button" class="px-2.5 py-1 text-[11.5px] text-faint hover:text-muted-foreground" onclick={() => projects.toggleShowAll(g.repo)}>
					{all ? 'Show fewer' : `Show ${g.runs.length - shown.length} more`}
				</button>
			{/if}
		</div>
	{/if}
{/snippet}

{#snippet segment(label: string, options: string[][], value: string, set: (v: string) => void, badge = 0)}
	<div class="grid flex-1 grid-cols-2 rounded-md bg-sidebar-accent/60 p-0.5" role="tablist" aria-label={label}>
		{#each options as [id, text] (id)}
			<button
				type="button"
				role="tab"
				aria-selected={value === id}
				class="truncate rounded px-1.5 py-1 transition-colors {value === id ? 'bg-background font-medium text-foreground shadow-xs' : 'text-muted-foreground hover:text-foreground'}"
				onclick={() => set(id)}
			>{text}{#if id === 'team' && badge}<span class="tnum ml-1 text-faint">{badge}</span>{/if}</button>
		{/each}
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
		<div class="mx-1 mt-3 flex flex-col gap-1.5 text-[12px]">
			{#if filtering}
				{@render segment('Whose tasks', [['mine', 'Mine'], ['team', 'Team']], scope, (v) => setScope(v as 'mine' | 'team'), theirs)}
			{/if}
			{@render segment('Group tasks', [['time', 'By time'], ['project', 'By project']], projects.view, (v) => projects.setView(v as 'time' | 'project'))}
		</div>
	{/if}
	{#if fleet.loaded && !q && !awake.length}
		<div class="mx-1 mt-4 rounded-lg border border-dashed border-sidebar-border px-3 py-4 text-xs leading-relaxed text-muted-foreground">
			{#if filtering && scope === 'mine' && fleet.runs.length}
				You haven't started a task here yet. Switch to Team to see what everyone else is running.
			{:else}
				No tasks yet. Each one you start shows up here, so you can jump between them.
			{/if}
		</div>
	{:else if fleet.loaded && !q}
		{#if pinned.length}
			{@render heading('Pinned')}
			{#each pinned as r (r.id)}{@render row(r, live(r))}{/each}
		{/if}
		{#if byProject}
			{#if pinned.length && shownGroups.length}{@render heading('Projects')}{:else}<div class="h-1.5"></div>{/if}
			{#each shownGroups as g (g.repo)}{@render project(g)}{/each}
			{#if hiddenCount}
				<button type="button" class="mt-2 px-2.5 py-1 text-[11.5px] text-faint hover:text-muted-foreground" onclick={() => projects.unhideAll()}>
					Show {hiddenCount} hidden project{hiddenCount === 1 ? '' : 's'}
				</button>
			{/if}
		{:else}
			{#if active.length}
				{@render heading('Active', active.length)}
				{#each active as r (r.id)}{@render row(r, true)}{/each}
			{/if}
			{#each groups as g (g.label)}
				{@render heading(g.label)}
				{#each g.runs as r (r.id)}{@render row(r, false)}{/each}
			{/each}
		{/if}
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

<ProjectIconPicker />

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
