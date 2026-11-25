<script lang="ts">
	// The review desk: your finished tasks that need a decision, and nothing else. Running tasks
	// stay out of sight, and so do tasks working on notes you sent, until they come back.
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import CornerDownLeft from '@lucide/svelte/icons/corner-down-left';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import OctagonX from '@lucide/svelte/icons/octagon-x';
	import ShieldQuestion from '@lucide/svelte/icons/shield-question';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { Button } from '#lib/components/ui/button';
	import PrIcon from '#lib/sf/ui/PrIcon.svelte';
	import { fleet } from '#lib/sf/state.svelte';
	import { ago, repoLabel } from '#lib/sf/format';
	import { review, type DeskItem, type Section } from '#lib/sf/review/desk.svelte';

	$effect(() => review.sync());
	const title = (task: string) => task.split('\n')[0].slice(0, 140);
	const SECTIONS: { id: Section; label: string; hint?: string }[] = [
		{ id: 'approval', label: 'Waiting for your OK', hint: 'the agent is stopped on these' },
		{ id: 'merge', label: 'Ready to merge' },
		{ id: 'finished', label: 'Finished, no pull request yet' },
		{ id: 'stopped', label: 'Stopped' }
	];
	const by = $derived(Object.fromEntries(SECTIONS.map((s) => [s.id, review.queue.filter((i) => i.section === s.id)])) as Record<Section, DeskItem[]>);
	const running = $derived(fleet.runs.filter((r) => !r.archived_at && (r.thread_status === 'running' || r.thread_status === 'queued') && (!fleet.userId || r.created_by === fleet.userId)).length);
	const href = (i: DeskItem) => (i.section === 'approval' || i.section === 'stopped' ? `#/runs/${i.run.id}` : `#/review/${i.run.id}`);
	const heading = 'text-[11.5px] font-medium tracking-wide text-muted-foreground uppercase';
</script>

<svelte:head><title>Review · Sidefleet</title></svelte:head>

{#if !fleet.reviewDesk}
	<section class="mx-auto flex w-full max-w-3xl flex-col gap-2 px-4 py-16 text-center">
		<h1 class="text-[18px] font-medium">Not on for you yet</h1>
		<p class="text-[13.5px] text-muted-foreground">The review desk is being tried out by a few people first.</p>
	</section>
{:else}
	<section class="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 py-8 md:px-8">
		<header class="flex flex-wrap items-end justify-between gap-3">
			<div class="flex flex-col gap-1">
				<h1 class="text-[22px] font-semibold tracking-tight">Review</h1>
				<p class="text-[13px] leading-relaxed text-muted-foreground">
					{#if review.count}{review.count} {review.count === 1 ? 'task needs' : 'tasks need'} a decision from you.{:else}Nothing needs a decision from you.{/if}
					{#if running}{running} {running === 1 ? 'task is' : 'tasks are'} running and {running === 1 ? 'doesn’t' : 'don’t'} need you.{/if}
				</p>
			</div>
			{#if review.queue.length}
				<Button href={href(review.queue[0])} class="h-9 rounded-full px-4">Start with the first<ChevronRight class="size-4" /></Button>
			{/if}
		</header>

		{#each SECTIONS as s (s.id)}
			{#if by[s.id].length}
				<section class="flex flex-col gap-2" aria-labelledby="h-{s.id}">
					<h2 id="h-{s.id}" class={heading}>{s.label}{#if s.hint}<span class="normal-case tracking-normal"> · {s.hint}</span>{/if}</h2>
					{#each by[s.id] as i (i.run.id)}
						{@const pr = i.run.pr}
						<a href={href(i)} class="flex items-center gap-3 rounded-2xl border bg-card px-4 py-3.5 hover:bg-muted/40">
							{#if s.id === 'approval'}<ShieldQuestion class="size-5 shrink-0 text-queue-ink" />
							{:else if s.id === 'stopped'}<OctagonX class="size-5 shrink-0 text-err-ink" />
							{:else if pr}<span class="grid size-5 shrink-0 place-items-center"><PrIcon state="open" draft={pr.draft} class="size-4" /></span>
							{:else}<span class="size-5 shrink-0"></span>{/if}
							<span class="flex min-w-0 flex-1 flex-col gap-1">
								<span class="truncate text-[14.5px] font-medium">{title(i.run.task)}</span>
								<span class="truncate text-xs text-muted-foreground">
									{repoLabel(i.run.repo)}{pr ? ` · #${pr.number}` : ''} · {i.back ? `round ${i.round?.n}` : ago(i.run.last_activity_at, fleet.now)}
									{#if s.id === 'approval'} · {i.run.pending_approvals || 1} waiting{/if}
								</span>
								{#if i.back || pr?.checks?.failed || pr?.conflicts || pr?.review === 'changes_requested'}
									<span class="flex flex-wrap gap-1.5">
										{#if i.back}<span class="inline-flex items-center gap-1 rounded-full bg-run-soft px-2 py-0.5 text-[11.5px] font-medium text-run-ink"><CornerDownLeft class="size-3" />Back from your {i.round?.notes.length === 1 ? 'note' : `${i.round?.notes.length} notes`}</span>{/if}
										{#if pr?.checks?.failed}<span class="inline-flex items-center gap-1 rounded-full bg-err-soft px-2 py-0.5 text-[11.5px] font-medium text-err-ink"><TriangleAlert class="size-3" />{pr.checks.failed} failing</span>{/if}
										{#if pr?.conflicts}<span class="inline-flex items-center gap-1 rounded-full bg-queue-soft px-2 py-0.5 text-[11.5px] font-medium text-queue-ink"><TriangleAlert class="size-3" />Conflicts</span>{/if}
										{#if pr?.review === 'changes_requested'}<span class="inline-flex items-center gap-1 rounded-full bg-queue-soft px-2 py-0.5 text-[11.5px] font-medium text-queue-ink">Changes requested</span>{/if}
									</span>
								{/if}
							</span>
							<ChevronRight class="size-4 shrink-0 text-muted-foreground" />
						</a>
					{/each}
				</section>
			{/if}
		{/each}

		{#if review.away.length}
			<section class="flex flex-col gap-2" aria-labelledby="h-away">
				<h2 id="h-away" class={heading}>With the agent · back here when it's done</h2>
				<ul class="flex flex-col divide-y rounded-2xl border bg-card">
					{#each review.away as r (r.id)}
						{@const n = review.state.rounds[r.id]?.notes.length ?? 0}
						<li><a href="#/runs/{r.id}" class="flex items-center gap-3 px-4 py-3 text-[13px] hover:bg-muted/40">
							<LoaderCircle class="size-4 shrink-0 animate-spin text-run-ink" />
							<span class="min-w-0 flex-1 truncate"><span class="font-medium">{title(r.task)}</span><span class="text-muted-foreground"> · working on your {n === 1 ? 'note' : `${n} notes`}</span></span>
						</a></li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if review.later.length}
			<section class="flex flex-col gap-2" aria-labelledby="h-later">
				<h2 id="h-later" class={heading}>Left for later · back by itself when something happens</h2>
				<ul class="flex flex-col divide-y rounded-2xl border bg-card">
					{#each review.later as r (r.id)}
						<li class="flex items-center gap-3 px-4 py-3 text-[13px]">
							<a href="#/review/{r.id}" class="min-w-0 flex-1 truncate font-medium hover:underline">{title(r.task)}</a>
							<button type="button" class="shrink-0 text-xs text-muted-foreground hover:text-foreground" onclick={() => review.bringBack(r.id)}>Bring back</button>
						</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if !review.count && !review.away.length}
			<p class="rounded-2xl border border-dashed px-4 py-8 text-center text-[13.5px] text-muted-foreground">When a task finishes, it shows up here with what changed and what Sidefleet noticed, ready for you to merge or send back.</p>
		{/if}
	</section>
{/if}
