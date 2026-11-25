<script lang="ts">
	// Pull requests: what you authored in the GitHub repos on your machines, open ones and the
	// latest closed, as your machines see it with the GitHub CLI. They look when this page asks.
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import { toast } from 'svelte-sonner';
	import Search from '@lucide/svelte/icons/search';
	import ArrowUpDown from '@lucide/svelte/icons/arrow-up-down';
	import ListFilter from '@lucide/svelte/icons/list-filter';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import CircleX from '@lucide/svelte/icons/circle-x';
	import CircleDot from '@lucide/svelte/icons/circle-dot';
	import UserCheck from '@lucide/svelte/icons/user-check';
	import UserRoundX from '@lucide/svelte/icons/user-round-x';
	import MessageSquare from '@lucide/svelte/icons/message-square';
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { rel } from '#lib/sf/format';
	import type { AuthoredPr, PullRequestsPage } from '#lib/sf/types';
	import PrIcon from '#lib/sf/ui/PrIcon.svelte';
	import { cn } from '#lib/utils';

	let data = $state<PullRequestsPage | null>(null);
	let error = $state('');
	let query = $state('');

	type Sort = 'updated' | 'newest' | 'oldest' | 'size';
	const SORTS: Record<Sort, string> = { updated: 'Recently updated', newest: 'Newest', oldest: 'Oldest', size: 'Most changed' };
	type Show = 'open' | 'merged' | 'closed' | 'all';
	const SHOWS: Record<Show, string> = { open: 'Open', merged: 'Merged', closed: 'Closed', all: 'All' };
	const KEY = 'sidefleet_pulls';
	const saved = (() => { try { return JSON.parse(localStorage.getItem(KEY) ?? '{}'); } catch { return {}; } })();
	let sort = $state<Sort>(saved.sort in SORTS ? saved.sort : 'updated');
	let show = $state<Show>(saved.show in SHOWS ? saved.show : 'all');
	let failing = $state(false);
	let changesRequested = $state(false);
	let hideDrafts = $state(false);
	let repo = $state('');
	$effect(() => {
		const v = JSON.stringify({ sort, show });
		try { localStorage.setItem(KEY, v); } catch { /* storage blocked */ }
	});

	async function load() {
		try {
			data = await api<PullRequestsPage>('/v1/pull-requests');
			error = '';
		} catch (e) {
			error = (e as Error).message;
		}
	}
	/** Ask your machines to look on GitHub again (they do within a heartbeat). */
	async function refresh(loud = false) {
		try {
			await api('/v1/pull-requests/refresh', { method: 'POST' });
			await load();
		} catch (e) {
			if (loud) toast.error('Could not ask your machines', { description: (e as Error).message });
		}
	}
	$effect(() => {
		void fleet.orgId;
		if (!fleet.loaded) return;
		data = null;
		void refresh();
		// While the page is open: the list every few seconds, a fresh look on GitHub every two minutes.
		const list = setInterval(() => void load(), 5_000);
		const look = setInterval(() => { if (!document.hidden) void refresh(); }, 120_000);
		return () => { clearInterval(list); clearInterval(look); };
	});

	const looking = $derived(!!data?.machines.some((m) => m.looking));
	const repos = $derived([...new Set((data?.prs ?? []).map((p) => p.repo))].sort());
	const filtered = $derived.by(() => {
		const q = query.trim().toLowerCase().replace(/^#/, '');
		let prs = (data?.prs ?? []).filter((p) => {
			if (show !== 'all' && p.state !== show) return false;
			if (repo && p.repo !== repo) return false;
			if (failing && !p.checks?.failed) return false;
			if (changesRequested && p.review !== 'changes_requested') return false;
			if (hideDrafts && p.draft) return false;
			if (!q) return true;
			return [p.title, p.repo, String(p.number), p.head ?? '', p.thread?.task ?? ''].some((s) => s.toLowerCase().includes(q));
		});
		const time = (s?: string) => (s ? new Date(s).getTime() : 0);
		prs = [...prs].sort((a, b) =>
			sort === 'newest' ? time(b.created_at) - time(a.created_at)
			: sort === 'oldest' ? time(a.created_at) - time(b.created_at)
			: sort === 'size' ? (b.additions ?? 0) + (b.deletions ?? 0) - ((a.additions ?? 0) + (a.deletions ?? 0))
			: time(b.updated_at) - time(a.updated_at)
		);
		return prs;
	});
	// With everything shown: open ones first, then what was merged or closed.
	const groups = $derived(
		show === 'all'
			? [
				{ label: 'Open', prs: filtered.filter((p) => p.state === 'open') },
				{ label: 'Merged and closed', prs: filtered.filter((p) => p.state !== 'open') }
			].filter((g) => g.prs.length)
			: [{ label: SHOWS[show], prs: filtered }]
	);
	const filters = $derived(Number(failing) + Number(changesRequested) + Number(hideDrafts) + Number(!!repo));

	/** One line on each machine that isn't simply up to date, keyed by machine: two machines can share a name. */
	const notes = $derived(
		(data?.machines ?? []).flatMap((m) => {
			if (!m.can_list) return [{ id: m.id, text: `${m.name} needs an update to list pull requests.` }];
			if (m.error) return [{ id: m.id, text: `${m.name} couldn't look: ${m.error}` }];
			if (!m.online && !m.fetched_at) return [{ id: m.id, text: `${m.name} is offline.` }];
			return [];
		})
	);
	const lastLooked = $derived(
		(data?.machines ?? []).map((m) => m.fetched_at).filter(Boolean).sort().at(-1) ?? null
	);

	function checksTitle(c: NonNullable<AuthoredPr['checks']>) {
		const total = c.passed + c.failed + c.pending;
		if (c.failed) return `${c.failed} of ${total} checks failed${c.failing?.length ? ': ' + c.failing.map((f) => f.name).join(', ') : ''}`;
		if (c.pending) return `${c.pending} of ${total} checks still running`;
		return total === 1 ? 'The check passed' : `All ${total} checks passed`;
	}
	const num = (n?: number) => (n ?? 0).toLocaleString();
</script>

<svelte:head><title>Pull requests · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-4xl flex-col gap-5 px-4 py-8 md:px-8">
	<header class="flex flex-col gap-1">
		<h1 class="text-[22px] font-semibold tracking-tight">Pull requests</h1>
		<p class="text-[13px] leading-relaxed text-muted-foreground">
			Yours, in the GitHub repos on your machines: open ones and the latest merged or closed. Your machines look with the GitHub CLI while this page is open.
		</p>
	</header>

	<div class="flex flex-wrap items-center gap-2">
		<label class="relative min-w-[14rem] flex-1">
			<span class="sr-only">Search pull requests</span>
			<Search class="pointer-events-none absolute top-1/2 left-3 size-3.5 -translate-y-1/2 text-muted-foreground" />
			<input
				bind:value={query}
				placeholder="Search pull requests"
				class="h-9 w-full rounded-full border border-input bg-card pr-3 pl-8.5 text-[13px] outline-none placeholder:text-faint focus:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]"
			/>
		</label>
		<div class="flex h-9 items-center rounded-full border bg-card p-0.5 text-[12.5px]" role="radiogroup" aria-label="Show">
			{#each Object.entries(SHOWS) as [k, label] (k)}
				<button
					type="button"
					role="radio"
					aria-checked={show === k}
					onclick={() => (show = k as Show)}
					class={cn('h-full rounded-full px-3 transition-colors', show === k ? 'bg-muted font-medium text-foreground' : 'text-muted-foreground hover:text-foreground')}
				>{label}</button>
			{/each}
		</div>
		<DropdownMenu.Root>
			<DropdownMenu.Trigger class="inline-flex h-9 items-center gap-1.5 rounded-full border bg-card px-3 text-[12.5px] text-muted-foreground hover:text-foreground">
				<ArrowUpDown class="size-3.5" />{SORTS[sort]}
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="end" class="w-44">
				<DropdownMenu.RadioGroup bind:value={sort}>
					{#each Object.entries(SORTS) as [k, label] (k)}<DropdownMenu.RadioItem value={k}>{label}</DropdownMenu.RadioItem>{/each}
				</DropdownMenu.RadioGroup>
			</DropdownMenu.Content>
		</DropdownMenu.Root>
		<DropdownMenu.Root>
			<DropdownMenu.Trigger class={cn('inline-flex h-9 items-center gap-1.5 rounded-full border bg-card px-3 text-[12.5px] hover:text-foreground', filters ? 'text-foreground' : 'text-muted-foreground')}>
				<ListFilter class="size-3.5" />Filters{#if filters}<span class="tnum rounded-full bg-muted px-1.5 text-[11px]">{filters}</span>{/if}
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="end" class="w-56">
				<DropdownMenu.CheckboxItem bind:checked={failing}>Checks failing</DropdownMenu.CheckboxItem>
				<DropdownMenu.CheckboxItem bind:checked={changesRequested}>Changes requested</DropdownMenu.CheckboxItem>
				<DropdownMenu.CheckboxItem bind:checked={hideDrafts}>Hide drafts</DropdownMenu.CheckboxItem>
				{#if repos.length > 1}
					<DropdownMenu.Separator />
					<DropdownMenu.Label class="text-xs text-muted-foreground">Repo</DropdownMenu.Label>
					<DropdownMenu.RadioGroup bind:value={repo}>
						<DropdownMenu.RadioItem value="">All repos</DropdownMenu.RadioItem>
						{#each repos as r (r)}<DropdownMenu.RadioItem value={r}><span class="truncate font-mono text-[12px]">{r}</span></DropdownMenu.RadioItem>{/each}
					</DropdownMenu.RadioGroup>
				{/if}
				{#if filters}
					<DropdownMenu.Separator />
					<DropdownMenu.Item onclick={() => { failing = false; changesRequested = false; hideDrafts = false; repo = ''; }}>Clear filters</DropdownMenu.Item>
				{/if}
			</DropdownMenu.Content>
		</DropdownMenu.Root>
		<button
			type="button"
			onclick={() => refresh(true)}
			disabled={looking}
			class="inline-flex size-9 items-center justify-center rounded-full border bg-card text-muted-foreground hover:text-foreground disabled:opacity-70"
			aria-label="Look again"
			title={looking ? 'Your machines are looking on GitHub…' : lastLooked ? `Looked ${rel(lastLooked, fleet.now)}. Look again` : 'Look again'}
		><RefreshCw class={cn('size-3.5', looking && 'animate-spin')} /></button>
	</div>

	{#if notes.length || looking || lastLooked}
		<div class="-mt-2 flex flex-col gap-0.5 text-xs text-muted-foreground" aria-live="polite">
			{#if looking}<p>Your machines are looking on GitHub…</p>{:else if lastLooked}<p>Looked {rel(lastLooked, fleet.now)}.</p>{/if}
			{#each notes as n (n.id)}<p class="text-queue-ink">{n.text}</p>{/each}
		</div>
	{/if}

	{#if error && !data}
		<p class="rounded-xl border border-err/30 bg-err-soft/40 p-4 text-[13px] text-err-ink">Couldn't load pull requests: {error}</p>
	{:else if data === null || (looking && !data.prs.length)}
		<div class="grid gap-2">
			{#each [0, 1, 2, 3] as i (i)}<div class="h-16 animate-pulse rounded-xl bg-muted"></div>{/each}
		</div>
	{:else if !data.machines.length}
		<div class="flex flex-col items-start gap-2 rounded-xl border border-dashed p-6">
			<GitPullRequest class="size-5 text-muted-foreground" />
			<h2 class="text-[15px] font-medium">No machines yet</h2>
			<p class="max-w-lg text-[13px] leading-relaxed text-muted-foreground">Connect a machine with your checkouts and the GitHub CLI; your pull requests show up here. <a href="#/machines" class="underline underline-offset-2">Machines</a></p>
		</div>
	{:else if !filtered.length}
		<div class="flex flex-col items-start gap-2 rounded-xl border border-dashed p-6">
			<GitPullRequest class="size-5 text-muted-foreground" />
			<h2 class="text-[15px] font-medium">{data.prs.length ? 'Nothing matches' : 'No pull requests yet'}</h2>
			<p class="max-w-lg text-[13px] leading-relaxed text-muted-foreground">
				{data.prs.length ? 'Try another search, or clear the filters.' : 'None by you in the GitHub repos your machines have. Open one from a finished task, and it shows here.'}
			</p>
		</div>
	{:else}
		{#each groups as g (g.label)}
			<div class="flex flex-col">
				<h2 class="flex items-center gap-2 pb-1.5 text-xs font-medium text-muted-foreground">{g.label}<span class="tnum font-normal">{g.prs.length}</span><span class="h-px flex-1 bg-border"></span></h2>
				<ul class="flex flex-col">
					{#each g.prs as p (p.url)}
						<li class="group/row relative flex items-start gap-3 rounded-xl px-2.5 py-2.5 transition-colors hover:bg-accent/40">
							<span class="mt-0.5"><PrIcon state={p.state} draft={p.draft} class="size-4" /></span>
							<div class="min-w-0 flex-1">
								<p class="flex min-w-0 items-center gap-1.5 text-[14px]">
									<span class="tnum shrink-0 text-muted-foreground">#{p.number}</span>
									<!-- The title covers the row: the whole row opens it on GitHub. -->
									<a href={p.url} target="_blank" rel="noopener noreferrer" class="min-w-0 truncate font-medium after:absolute after:inset-0 after:rounded-xl" title={p.title}>{p.title}</a>
									{#if p.checks}
										<span class="relative shrink-0" title={checksTitle(p.checks)}>
											{#if p.checks.failed}<CircleX class="size-3.5 text-err-ink" />{:else if p.checks.pending}<CircleDot class="size-3.5 text-queue-ink" />{:else}<CircleCheck class="size-3.5 text-ok-ink" />{/if}
										</span>
									{/if}
									{#if p.state === 'open' && p.review === 'approved'}
										<span class="relative shrink-0" title="Approved"><UserCheck class="size-3.5 text-ok-ink" /></span>
									{:else if p.state === 'open' && p.review === 'changes_requested'}
										<span class="relative shrink-0" title="Changes requested"><UserRoundX class="size-3.5 text-queue-ink" /></span>
									{/if}
									{#if p.draft && p.state === 'open'}<span class="shrink-0 rounded-full bg-muted px-1.5 text-[11px] text-muted-foreground">Draft</span>{/if}
								</p>
								<p class="mt-0.5 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
									<span class="font-mono text-[11.5px]">{p.repo}</span>
									{#if p.head}<span class="max-w-[16rem] truncate font-mono text-[11.5px]" title={p.head}>{p.head}</span>{/if}
									{#if p.thread}
										<a href="#/runs/{p.thread.id}" class="relative z-10 inline-flex max-w-[18rem] items-center gap-1 rounded-md bg-muted px-1.5 py-px text-foreground/80 hover:text-foreground" title="Open the task that made it: {p.thread.task}">
											<MessageSquare class="size-3 shrink-0" /><span class="truncate">{p.thread.task}</span>
										</a>
									{/if}
								</p>
							</div>
							<div class="tnum flex shrink-0 flex-col items-end gap-0.5 text-xs">
								<span><span class="text-diff-add-ink">+{num(p.additions)}</span> <span class="text-diff-del-ink">−{num(p.deletions)}</span></span>
								<span class="text-muted-foreground" title={p.updated_at ? new Date(p.updated_at).toLocaleString() : ''}>{rel(p.updated_at, fleet.now)}</span>
							</div>
						</li>
					{/each}
				</ul>
			</div>
		{/each}
	{/if}
</section>
