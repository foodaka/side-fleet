<script lang="ts">
	// The thread's pull request, at the end of the thread: where the branch goes, what GitHub
	// says about it (checks, review, conflicts) and the one thing to do next, with the rest in a menu.
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import CircleX from '@lucide/svelte/icons/circle-x';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import UserCheck from '@lucide/svelte/icons/user-check';
	import UserRoundX from '@lucide/svelte/icons/user-round-x';
	import Eye from '@lucide/svelte/icons/eye';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import Link from '@lucide/svelte/icons/link';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import FileDiffIcon from '@lucide/svelte/icons/file-diff';
	import { toast } from 'svelte-sonner';
	import { cn } from '#lib/utils';
	import GitMerge from '@lucide/svelte/icons/git-merge';
	import type { MergeMethod, PullRequest } from '../types';
	import { copyText } from '../copy';
	import PrIcon from '../ui/PrIcon.svelte';
	import DiffStat from '../diff/DiffStat.svelte';

	let {
		pr,
		branch,
		repo,
		machine,
		machineOnline,
		theirs,
		busy,
		pushing,
		merging = false,
		unpushed = false,
		canMerge = false,
		changes,
		onpush,
		oncheck,
		onreview,
		onask,
		onmerge
	}: {
		pr: PullRequest | null;
		branch: string;
		repo: string;
		machine: string;
		machineOnline: boolean;
		/** A teammate's thread: you can look, not push. */
		theirs: boolean;
		/** A message is running or waiting, so the branch can't be pushed yet. */
		busy: boolean;
		/** The push (and opening the pull request) is running now. */
		pushing: boolean;
		/** Merging it is running now. */
		merging?: boolean;
		/** Messages since the last push changed files, so pushing comes before merging. */
		unpushed?: boolean;
		/** The machine's Sidefleet can merge it from here. */
		canMerge?: boolean;
		changes: { file_count: number; additions: number; deletions: number } | null;
		onpush: () => void;
		oncheck: () => void;
		onreview: () => void;
		/** Put a request to the agent in the reply box. */
		onask?: (text: string) => void;
		onmerge?: (method: MergeMethod) => void;
	} = $props();

	const open = $derived(pr?.state === 'open');
	const base = $derived(pr?.base ?? 'the default branch');
	const stateLabel = $derived(!pr ? '' : pr.state === 'open' ? (pr.draft ? 'Draft' : 'Open') : pr.state === 'merged' ? 'Merged' : 'Closed');
	const checks = $derived(open ? pr?.checks : undefined);
	const total = $derived(checks ? checks.passed + checks.failed + checks.pending : 0);

	// Ready to merge as far as GitHub has told us: branch protection still has the last word.
	const ready = $derived(!!pr && open && !pr.draft && !pr.conflicts && !(checks && (checks.failed || checks.pending)) && pr.review !== 'changes_requested');
	const METHODS: Record<MergeMethod, string> = { squash: 'Squash and merge', merge: 'Create a merge commit', rebase: 'Rebase and merge' };
	/** What the confirm says it does. */
	const VERBS: Record<MergeMethod, string> = { squash: 'squash and merge', merge: 'merge', rebase: 'rebase and merge' };
	const METHOD_KEY = 'sidefleet_merge_method';
	let method = $state<MergeMethod>(((): MergeMethod => {
		try {
			const m = localStorage.getItem(METHOD_KEY);
			return m === 'merge' || m === 'rebase' ? m : 'squash';
		} catch {
			return 'squash';
		}
	})());
	// Merging asks twice: the first click turns the button into "Confirm …?" for a few seconds.
	let confirming = $state(false);
	let confirmTimer: ReturnType<typeof setTimeout> | undefined;
	function askToMerge(m: MergeMethod) {
		method = m;
		try { localStorage.setItem(METHOD_KEY, m); } catch { /* private mode */ }
		confirming = true;
		clearTimeout(confirmTimer);
		confirmTimer = setTimeout(() => (confirming = false), 5000);
	}

	// The one action the button does; the rest live in its menu.
	const action = $derived(
		theirs || pr?.state === 'merged' ? 'view'
		: open ? (ready && !unpushed && canMerge && onmerge ? 'merge' : 'push')
		: 'open'
	);
	const running = $derived(pushing || merging);
	// Why the action can't run now, said under it rather than hidden in a tooltip.
	const blocked = $derived(
		action === 'view' || running ? ''
		: !machineOnline ? `${machine} is offline, so it can't push or merge. It will once it's back.`
		: busy ? 'Waiting for the agent to finish.'
		: ''
	);
	const label = $derived(
		merging ? 'Merging…'
		: pushing ? (open ? 'Pushing…' : 'Opening…')
		: action === 'merge' ? (confirming ? `Confirm ${VERBS[method]}` : METHODS[method])
		: action === 'push' ? 'Push changes'
		: action === 'open' ? (pr?.state === 'closed' ? 'Open a new one' : 'Open pull request')
		: 'View on GitHub'
	);
	const what = $derived(
		action === 'merge' ? `Merges #${pr?.number} into ${base} on GitHub, from ${machine}, if its last commit is still the one pushed from here`
		: action === 'push' ? `Choose the files, then commit and push ${branch} to #${pr?.number} with the GitHub CLI on ${machine}`
		: action === 'open' ? `Choose the files, title and description, then push ${branch} and open a pull request with the GitHub CLI on ${machine}`
		: ''
	);
	function primary() {
		if (action !== 'merge') return onpush();
		if (!confirming) return askToMerge(method);
		confirming = false;
		onmerge?.(method);
	}
	function mergeFromMenu(m: MergeMethod) {
		if (confirming && m === method) return primary();
		askToMerge(m);
	}
	/** Why the merge items in the menu can't be used, if they can't. */
	const mergeOff = $derived(
		!canMerge || !onmerge ? `Update Sidefleet on ${machine} to merge from here`
		: blocked || (unpushed ? 'Push the new changes first' : '')
	);

	async function copyLink() {
		if (!pr) return;
		if (await copyText(pr.url)) toast.success('Link copied');
		else toast.error('Could not copy the link');
	}
	function failingNames(c: NonNullable<PullRequest['checks']>) {
		return (c.failing ?? []).map((f) => f.name).join(', ');
	}
</script>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape') confirming = false; }} />

<section aria-label="Pull request" class="overflow-hidden rounded-xl border bg-card text-xs text-muted-foreground">
	<div class="flex items-center gap-3 px-3.5 py-3 max-sm:flex-col max-sm:items-stretch">
		<div class="flex min-w-0 flex-1 items-start gap-2.5">
			<span class="mt-0.5 grid size-7 shrink-0 place-items-center rounded-full bg-muted">
				{#if pr}<PrIcon state={pr.state} draft={pr.draft} class="size-3.5" />{:else}<GitBranch class="size-3.5" />{/if}
			</span>
			<div class="min-w-0 flex-1">
				{#if pr}
					<a href={pr.url} target="_blank" rel="noopener noreferrer" class="block truncate text-[13px] font-medium text-foreground hover:underline" title={pr.title ?? `Pull request #${pr.number}`}>
						{pr.title ?? `Pull request #${pr.number}`}
					</a>
					<p class="mt-0.5 flex min-w-0 flex-wrap items-center gap-x-1.5">
						<span class="tnum text-foreground/80">#{pr.number}</span>
						<span
							class={cn(
								'rounded-full px-1.5 py-px text-[11px] font-medium',
								pr.state === 'merged' ? 'bg-merged-ink/12 text-merged-ink' : pr.state === 'closed' ? 'bg-err-soft text-err-ink' : pr.draft ? 'bg-muted text-muted-foreground' : 'bg-ok-soft text-ok-ink'
							)}
						>{stateLabel}</span>
						<span class="inline-flex min-w-0 items-center gap-1 font-mono text-[11.5px]" title="{branch} into {base}">
							<span class="truncate">{pr.base ?? 'base'}</span><span aria-hidden="true">←</span><span class="truncate text-foreground/80">{branch}</span>
						</span>
					</p>
				{:else}
					<p class="text-[13px] font-medium text-foreground">Ready for a pull request</p>
					<p class="mt-0.5">
						The changes are on <span class="font-mono text-[11.5px] text-foreground/80">{branch}</span> in {repo} on {machine}.
					</p>
				{/if}
			</div>
		</div>

		{#if action === 'view'}
			{#if pr}<a href={pr.url} target="_blank" rel="noopener noreferrer" class="inline-flex h-8 shrink-0 items-center justify-center gap-1.5 rounded-full border bg-background px-3 text-xs font-medium text-foreground hover:bg-muted">
				<ExternalLink class="size-3.5" />View on GitHub
			</a>{/if}
		{:else}
			<div class="flex shrink-0 items-stretch max-sm:w-full">
				<button
					type="button"
					onclick={primary}
					disabled={running || !!blocked}
					title={what}
					aria-live="polite"
					class={cn(
						'inline-flex h-8 items-center justify-center gap-1.5 px-3 text-xs font-medium transition-colors disabled:opacity-60 max-sm:flex-1',
						action === 'merge' ? 'rounded-full bg-ok text-white hover:bg-ok/90' + (confirming ? ' ring-2 ring-ok/40 ring-offset-1 ring-offset-card' : '')
						: pr ? 'rounded-full border bg-background text-foreground hover:bg-muted' : 'rounded-full bg-primary text-primary-foreground hover:bg-primary/90',
						pr && 'rounded-r-none',
						pr && action !== 'merge' && 'border-r-0'
					)}
				>
					{#if running}<LoaderCircle class="size-3.5 animate-spin" />{:else if action === 'merge'}<GitMerge class="size-3.5" />{:else}<GitPullRequest class="size-3.5" />{/if}{label}
				</button>
				{#if pr}
					<DropdownMenu.Root>
						<DropdownMenu.Trigger
							class={cn('inline-flex h-8 items-center rounded-r-full pr-2.5 pl-2', action === 'merge' ? 'border-l border-white/25 bg-ok text-white hover:bg-ok/90' : 'border bg-background text-foreground hover:bg-muted')}
							aria-label="More pull request actions"
						>
							<ChevronDown class="size-3.5" />
						</DropdownMenu.Trigger>
						<DropdownMenu.Content align="end" class="w-56">
							{#if open && action === 'merge'}
								<DropdownMenu.Item onclick={onpush} disabled={!!blocked}><GitPullRequest class="size-3.5" />Push changes…</DropdownMenu.Item>
							{/if}
							{#if open}
								{#each Object.entries(METHODS) as [m, name] (m)}
									<DropdownMenu.Item onclick={() => mergeFromMenu(m as MergeMethod)} disabled={!!mergeOff}>
										<GitMerge class="size-3.5" />{name}{#if m === method && action === 'merge'}<span class="ml-auto text-[11px] text-muted-foreground">default</span>{/if}
									</DropdownMenu.Item>
								{/each}
								{#if mergeOff}<p class="px-2 pb-1.5 text-[11px] text-muted-foreground">{mergeOff}</p>{/if}
								<DropdownMenu.Separator />
							{/if}
							<DropdownMenu.Item onclick={() => window.open(pr.url, '_blank', 'noopener')}><ExternalLink class="size-3.5" />View on GitHub</DropdownMenu.Item>
							<DropdownMenu.Item onclick={copyLink}><Link class="size-3.5" />Copy link</DropdownMenu.Item>
							{#if open}
								<DropdownMenu.Item onclick={oncheck}><RefreshCw class="size-3.5" />Check status now</DropdownMenu.Item>
							{/if}
						</DropdownMenu.Content>
					</DropdownMenu.Root>
				{/if}
			</div>
		{/if}
	</div>

	{#if checks || (open && (pr?.review || pr?.conflicts)) || (!pr && changes)}
		<ul class="divide-y border-t">
			{#if !pr && changes}
				<li class="flex items-center gap-2 px-3.5 py-2">
					<FileDiffIcon class="size-3.5 shrink-0" />
					<span class="tnum text-foreground/80">{changes.file_count} file{changes.file_count === 1 ? '' : 's'} changed</span>
					<DiffStat add={changes.additions} del={changes.deletions} blocks={false} class="text-[11px]" />
					<button type="button" class="ml-auto font-medium text-foreground/80 hover:text-foreground" onclick={onreview}>Review</button>
				</li>
			{/if}
			{#if checks}
				<li class="flex items-start gap-2 px-3.5 py-2">
					{#if checks.failed}
						<CircleX class="mt-px size-3.5 shrink-0 text-err-ink" />
						<span class="min-w-0 flex-1">
							<span class="text-foreground/80">{checks.failed} of {total} check{total === 1 ? '' : 's'} failed{#if checks.failing?.length}:{/if}</span>
							{#if checks.failing?.length}
								<span>
									{#each checks.failing as f, i (i)}{#if i}, {/if}{#if f.url}<a href={f.url} target="_blank" rel="noopener noreferrer" class="underline underline-offset-2 hover:text-foreground">{f.name}</a>{:else}{f.name}{/if}{/each}
								</span>
							{/if}
							{#if checks.pending}<span> · {checks.pending} still running</span>{/if}
						</span>
						{#if onask && !theirs}
							<button type="button" class="shrink-0 font-medium text-foreground/80 hover:text-foreground" onclick={() => onask(`The checks failed on pull request #${pr?.number}${checks.failing?.length ? ` (${failingNames(checks)})` : ''}. Find out why and fix it.`)}>Ask to fix</button>
						{/if}
					{:else if checks.pending}
						<LoaderCircle class="mt-px size-3.5 shrink-0 animate-spin text-queue-ink" />
						<span class="text-foreground/80">{checks.pending} of {total} check{total === 1 ? '' : 's'} still running</span>
					{:else}
						<CircleCheck class="mt-px size-3.5 shrink-0 text-ok-ink" />
						<span class="text-foreground/80">{total === 1 ? 'The check passed' : `All ${total} checks passed`}</span>
					{/if}
				</li>
			{/if}
			{#if open && pr?.review}
				<li class="flex items-center gap-2 px-3.5 py-2">
					{#if pr.review === 'approved'}
						<UserCheck class="size-3.5 shrink-0 text-ok-ink" /><span class="text-foreground/80">Approved</span>
					{:else if pr.review === 'changes_requested'}
						<UserRoundX class="size-3.5 shrink-0 text-queue-ink" /><span class="text-foreground/80">Changes requested</span>
						<a href={pr.url} target="_blank" rel="noopener noreferrer" class="ml-auto font-medium text-foreground/80 hover:text-foreground">Read the review</a>
					{:else}
						<Eye class="size-3.5 shrink-0" /><span>Waiting for a review</span>
					{/if}
				</li>
			{/if}
			{#if open && pr?.conflicts}
				<li class="flex items-center gap-2 px-3.5 py-2">
					<TriangleAlert class="size-3.5 shrink-0 text-err-ink" />
					<span class="flex-1 text-foreground/80">Conflicts with <span class="font-mono text-[11.5px]">{base}</span></span>
					{#if onask && !theirs}
						<button type="button" class="shrink-0 font-medium text-foreground/80 hover:text-foreground" onclick={() => onask(`Pull request #${pr?.number} conflicts with ${base}. Merge ${base} into this branch and resolve the conflicts.`)}>Ask to fix</button>
					{/if}
				</li>
			{/if}
		</ul>
	{/if}

	{#if blocked}
		<p class="border-t bg-muted/40 px-3.5 py-1.5 text-[11.5px]">{blocked}</p>
	{:else if confirming && action === 'merge'}
		<p class="border-t bg-ok-soft px-3.5 py-1.5 text-[11.5px] text-ok-ink">Click again to {VERBS[method]} #{pr?.number} into {base}. Esc cancels.</p>
	{:else if open && unpushed && ready && canMerge}
		<p class="border-t bg-muted/40 px-3.5 py-1.5 text-[11.5px]">The agent changed files since the last push: push them, then merge.</p>
	{:else if pr?.state === 'merged'}
		<p class="border-t bg-muted/40 px-3.5 py-1.5 text-[11.5px]">Merged. The branch <span class="font-mono">{branch}</span> is still on {machine}.</p>
	{/if}
</section>
