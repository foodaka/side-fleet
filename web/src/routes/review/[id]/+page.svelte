<script lang="ts">
	// One finished task, laid out to check: what Sidefleet saw first, then the agent's own
	// account, then the diff. Notes go on the line they're about and reach the agent as one
	// message in the same thread. When it comes back, the page leads with how each note went
	// and shows only what changed since.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { tick, untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import Check from '@lucide/svelte/icons/check';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import CircleHelp from '@lucide/svelte/icons/circle-help';
	import CornerDownLeft from '@lucide/svelte/icons/corner-down-left';
	import Eye from '@lucide/svelte/icons/eye';
	import GitMerge from '@lucide/svelte/icons/git-merge';
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import Info from '@lucide/svelte/icons/info';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import Quote from '@lucide/svelte/icons/quote';
	import Send from '@lucide/svelte/icons/send';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import Upload from '@lucide/svelte/icons/upload';
	import { Button } from '#lib/components/ui/button';
	import { fleet } from '#lib/sf/state.svelte';
	import { api } from '#lib/sf/api';
	import { repoLabel } from '#lib/sf/format';
	import type { MergeMethod } from '#lib/sf/types';
	import type { DiffFile } from '#lib/sf/diff/parse';
	import { Thread } from '#lib/sf/thread.svelte';
	import PrIcon from '#lib/sf/ui/PrIcon.svelte';
	import ReviewFile from '#lib/sf/review/ReviewFile.svelte';
	import { review } from '#lib/sf/review/desk.svelte';
	import { factsFor, flagsFor, type Flag } from '#lib/sf/review/flags';
	import { noteMessage, readAnswers, where, type Note } from '#lib/sf/review/notes';

	$effect(() => review.sync());
	const id = $derived(page.params.id ?? '');
	let thread = $state<Thread | null>(null);
	let loaded = $state(false);
	$effect(() => {
		const runId = id;
		if (!runId) return;
		const t = new Thread();
		untrack(() => { thread = t; loaded = false; view = 'since'; showAll = false; });
		t.load(runId).then(() => {
			if (t.root && t.root.id !== runId) void goto('#/review/' + t.root.id, { replaceState: true });
			if (thread === t) loaded = true;
		});
		return () => t.dispose();
	});

	const root = $derived(thread?.root ?? null);
	// The list's copy of the PR is refreshed every few seconds; the thread's is as loaded.
	const pr = $derived(fleet.runs.find((r) => r.id === root?.id)?.pr ?? root?.pr ?? null);
	const live = $derived(!!thread?.live);
	const changes = $derived(thread?.changes ?? null);
	const flags = $derived(flagsFor(changes, pr));
	const looks = $derived(flags.filter((f) => f.weight === 'look'));
	const notesOnly = $derived(flags.filter((f) => f.weight === 'note'));
	const facts = $derived(factsFor(changes, pr));
	const flagByFile = $derived(new Map(looks.filter((f) => f.file).map((f) => [f.file!, f.text])));

	// A round: notes sent from here, and the turns since.
	const round = $derived(id ? (review.state.rounds[id] ?? null) : null);
	const roundTurns = $derived(round && thread ? thread.turns.filter((t) => t.run.kind === 'agent' && new Date(t.run.created_at).getTime() >= new Date(round.sentAt).getTime() - 2000) : []);
	const back = $derived(!!round && !live && roundTurns.length > 0 && roundTurns.every((t) => !!t.result));
	const away = $derived(!!round && !back);
	const replyText = $derived.by(() => { const t = roundTurns.at(-1); return t ? t.result?.text || t.lastText : ''; });
	const answers = $derived(round && back ? readAnswers(replyText, round.notes.length) : []);
	const kept = $derived(answers.filter((a) => a.state === 'kept').length);
	const sinceFiles = $derived(roundTurns.flatMap((t) => t.diff?.turn.files ?? []));
	// Older rounds stored ids only.
	const before = $derived((round?.flags ?? []).map((f) => (typeof f === 'string' ? { id: f, text: '' } : f)));
	const gone = $derived(back ? before.filter((f) => !flags.some((x) => x.id === f.id)) : []);
	const isNew = (f: Flag) => back && !before.some((x) => x.id === f.id);
	let view = $state<'since' | 'all'>('since');
	const since = $derived(back && view === 'since');
	const files = $derived<DiffFile[]>(since ? sinceFiles : (changes?.files ?? []));

	const seen = (f: Flag) => review.isSeen(id, f.id);
	const fresh = $derived(looks.filter((f) => !seen(f)));
	const seenBefore = $derived(back ? looks.filter(seen) : []);
	const shownFlags = $derived(back ? fresh : looks);
	let seenOpen = $state(false);

	// The agent's own account: its last message in the thread.
	const says = $derived.by(() => {
		if (back) return replyText;
		const t = thread?.turns.findLast((t) => t.run.kind === 'agent' && t.result);
		return t ? t.result?.text || t.lastText : '';
	});
	let showAll = $state(false);
	const long = $derived(says.length > 700);

	const notes = $derived(review.drafts(id));
	const notesIn = (path: string) => notes.map((note, index) => ({ note, index })).filter((n) => n.note.file === path);

	/** A message since the last push changed files: push before merging, or GitHub merges the old commits. */
	const unpushed = $derived.by(() => {
		const runs = thread?.runs ?? [];
		const last = runs.findLastIndex((r) => r.kind === 'pr' && r.status === 'succeeded');
		return (thread?.turns ?? []).slice(last + 1).some((t) => (t.run.kind === 'agent' || t.run.kind === 'restore') && !!t.diff?.turn?.file_count);
	});
	const canManagePrs = $derived(!!fleet.workers.find((w) => w.id === thread?.machine)?.capabilities?.can_manage_prs);
	const method = ((): MergeMethod => {
		try {
			const m = localStorage.getItem('sidefleet_merge_method');
			return m === 'merge' || m === 'rebase' ? m : 'squash';
		} catch {
			return 'squash';
		}
	})();
	const VERB: Record<MergeMethod, string> = { squash: 'Squash and merge', merge: 'Merge', rebase: 'Rebase and merge' };

	const rest = $derived(review.queue.filter((i) => i.run.id !== id));
	const position = $derived(review.queue.findIndex((i) => i.run.id === id));
	const nextHref = (i: (typeof rest)[number]) => (i.section === 'approval' || i.section === 'stopped' ? `#/runs/${i.run.id}` : `#/review/${i.run.id}`);

	let flash = $state('');
	async function show(f: Flag) {
		if (!f.file) return;
		if (since && !sinceFiles.some((x) => x.path === f.file)) view = 'all';
		await tick();
		document.getElementById(`file-${f.file}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
		flash = f.file;
		setTimeout(() => (flash = ''), 1600);
	}

	let busy = $state('');
	let confirming = $state(false);
	async function act(label: string, work: () => Promise<unknown>, done: string) {
		busy = label;
		try {
			await work();
		} catch (e) {
			toast.error(`Could not ${label.toLowerCase().replace('…', '')}`, { description: (e as Error).message });
			busy = '';
			return;
		}
		busy = '';
		confirming = false;
		void fleet.refresh();
		const next = rest[0];
		toast(done, next ? { description: `Next: ${next.run.task.split('\n')[0].slice(0, 80)}` } : { description: "That's everything for now." });
		void goto(next ? nextHref(next) : '#/review');
	}
	const merge = () => {
		if (!confirming) return void (confirming = true);
		void act('Merging…', async () => { await thread!.mergePr(method); review.decide(id, 'merged'); }, `Merging #${pr?.number}.`);
	};
	const push = () => act('Pushing…', () => thread!.openPr(), `Pushing the new commits to #${pr?.number}. It comes back here when they're up.`);
	const openPr = () => act('Opening…', () => thread!.openPr({ write: true }), 'Opening a pull request. It comes back here when it’s open.');
	const sendNotes = () => {
		const list = [...notes];
		void act('Sending…', async () => {
			// Sent and waited for here, not queued on the thread: the page moves on right after.
			await api(`/v1/runs/${root!.id}/messages`, { method: 'POST', body: JSON.stringify({ task: noteMessage(list, pr?.state === 'open' ? pr.number : null) }) });
			review.sent(id, list, looks);
		}, `Sent ${list.length} ${list.length === 1 ? 'note' : 'notes'}. It comes back here when the agent is done.`);
	};
	const later = () => act('Leaving…', async () => review.decide(id, 'later'), 'Left for later. New activity brings it back.');

	const heading = 'text-[11.5px] font-medium tracking-wide text-muted-foreground uppercase';
	const seg = (on: boolean) => `h-7 rounded-md px-2.5 text-xs ${on ? 'bg-card font-medium text-foreground shadow-xs' : 'text-muted-foreground hover:text-foreground'}`;
</script>

<svelte:head><title>{root ? root.task.split('\n')[0].slice(0, 60) : 'Review'} · Review · Sidefleet</title></svelte:head>

{#if !fleet.reviewDesk}
	<section class="mx-auto flex w-full max-w-3xl flex-col gap-2 px-4 py-16 text-center">
		<h1 class="text-[18px] font-medium">Not on for you yet</h1>
		<p class="text-[13.5px] text-muted-foreground">The review desk is being tried out by a few people first. <a class="underline" href="#/runs/{id}">Open the task</a>.</p>
	</section>
{:else}
	<div class="flex min-h-full flex-col">
		<div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 pt-6 pb-10 md:px-8">
			<div class="flex items-center justify-between gap-3 text-xs text-muted-foreground">
				<a href="#/review" class="inline-flex items-center gap-1 hover:text-foreground"><ArrowLeft class="size-3.5" />Review</a>
				{#if rest.length}
					<a href={nextHref(rest[position >= 0 && rest[position] ? position : 0])} class="inline-flex items-center gap-1 hover:text-foreground">
						{#if position >= 0}<span class="tnum">{position + 1} of {rest.length + 1}</span> · {/if}Skip<ChevronRight class="size-3.5" />
					</a>
				{/if}
			</div>

			{#if thread?.error}
				<p class="rounded-2xl border bg-err-soft px-4 py-3 text-[13.5px] text-err-ink">Could not load this task: {thread.error}</p>
			{:else if !loaded || !root}
				<p class="flex items-center gap-2 text-[13.5px] text-muted-foreground"><LoaderCircle class="size-4 animate-spin" />Loading the task…</p>
			{:else}
				<header class="flex flex-col gap-1.5">
					<h1 class="line-clamp-3 text-[21px] leading-snug font-semibold tracking-tight" title={root.task}>{root.task.split('\n')[0]}</h1>
					<p class="flex flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground">
						<span>{repoLabel(root.repo)}</span>
						{#if pr}<span>·</span><a href={pr.url} target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 hover:text-foreground"><PrIcon state={pr.state} draft={pr.draft} />#{pr.number}</a>{/if}
						{#if thread?.branch}<span>·</span><span class="font-mono">{thread.branch}</span>{/if}
						{#if back}<span>·</span><span>round {round?.n}, back from your notes</span>{/if}
						<span>·</span><a href="#/runs/{root.id}" class="hover:text-foreground">Open the thread</a>
					</p>
				</header>

				{#if away || live}
					<p class="flex items-center gap-2.5 rounded-2xl border border-run/30 bg-run-soft/50 px-4 py-3 text-[13.5px]">
						<LoaderCircle class="size-4 shrink-0 animate-spin text-run-ink" />
						<span class="min-w-0 flex-1">{away ? 'With the agent, working on your notes.' : 'The agent is working on this task.'} It comes back to Review when it's done.</span>
					</p>
				{/if}

				{#if back && round}
					<!-- What you asked for comes first on a later round: it's what you're checking. -->
					<section class="flex flex-col gap-2.5" aria-labelledby="notes-h">
						<h2 id="notes-h" class={heading}>Your {round.notes.length === 1 ? 'note' : `${round.notes.length} notes`}{kept ? ` · ${kept} kept as is` : ''}</h2>
						<ul class="flex flex-col gap-2">
							{#each round.notes as n, i (i)}
								{@const a = answers[i]}
								<li class="flex gap-3 rounded-2xl border px-4 py-3 {a?.state === 'kept' ? 'border-queue/35 bg-queue-soft/50' : 'bg-card'}">
									{#if a?.state === 'done'}<CircleCheck class="mt-0.5 size-4 shrink-0 text-ok-ink" />{:else if a?.state === 'kept'}<CornerDownLeft class="mt-0.5 size-4 shrink-0 text-queue-ink" />{:else}<CircleHelp class="mt-0.5 size-4 shrink-0 text-muted-foreground" />{/if}
									<div class="flex min-w-0 flex-1 flex-col gap-1">
										<p class="text-[12.5px] text-muted-foreground"><span class="font-mono text-[11.5px]">{where(n)}</span> · you wrote “{n.text}”</p>
										<p class="text-[14px]">
											{#if a?.state === 'done'}<span class="font-medium">Done.</span> {a.text}
											{:else if a?.state === 'kept'}<span class="font-medium">Kept as is.</span> {a.text}
											{:else}<span class="text-muted-foreground">No answer for this one in the agent's reply; it's below.</span>{/if}
										</p>
										<button type="button" class="self-start text-xs text-run-ink underline decoration-dotted underline-offset-2 hover:text-foreground" onclick={() => show({ id: '', weight: 'note', text: '', detail: '', seen: '', file: n.file })}>Show the file</button>
									</div>
								</li>
							{/each}
						</ul>
					</section>
				{/if}

				<!-- What Sidefleet saw: the only place flags come from. -->
				<section class="flex flex-col gap-2.5" aria-labelledby="saw-h">
					<h2 id="saw-h" class={heading}>Sidefleet saw</h2>
					{#each gone as g (g.id)}
						<p class="flex items-start gap-2.5 rounded-2xl border bg-card px-4 py-3 text-[13.5px]"><CircleCheck class="mt-0.5 size-4 shrink-0 text-ok-ink" /><span><span class="font-medium">Gone:</span> {#if g.text}<span class="line-through decoration-faint">{g.text}</span>.{' '}{/if}<span class="text-muted-foreground">The new commits undid it.</span></span></p>
					{/each}
					{#if shownFlags.length}
						<ul class="flex flex-col gap-2">
							{#each shownFlags as f (f.id)}
								{@const ok = seen(f)}
								<li class="flex gap-3 rounded-2xl border px-4 py-3 {ok ? 'bg-card' : 'border-queue/35 bg-queue-soft/50'}">
									{#if ok}<CircleCheck class="mt-0.5 size-4 shrink-0 text-ok-ink" />{:else}<TriangleAlert class="mt-0.5 size-4 shrink-0 text-queue-ink" />{/if}
									<div class="flex min-w-0 flex-1 flex-col gap-1">
										<p class="flex flex-wrap items-center gap-2 text-[14px] font-medium">{f.text}{#if isNew(f)}<span class="rounded-full bg-queue-soft px-1.5 text-[10.5px] font-medium text-queue-ink">New since you last looked</span>{/if}</p>
										<p class="text-[13px] leading-relaxed break-words text-muted-foreground">{f.detail}</p>
										<p class="text-[11.5px] text-faint">{f.seen}</p>
										<div class="flex flex-wrap gap-1.5 pt-1">
											{#if f.file}<Button variant="outline" size="sm" class="h-7 rounded-full text-xs" onclick={() => show(f)}><Eye class="size-3.5" />Show me</Button>{/if}
											<Button variant={ok ? 'secondary' : 'outline'} size="sm" class="h-7 rounded-full text-xs" aria-pressed={ok} onclick={() => review.toggleSeen(id, f.id)}><Check class="size-3.5" />{ok ? 'Looked at' : 'Looks right'}</Button>
										</div>
									</div>
								</li>
							{/each}
						</ul>
					{:else if changes}
						<p class="flex items-center gap-2 rounded-2xl border bg-card px-4 py-3 text-[13.5px]"><CircleCheck class="size-4 shrink-0 text-ok-ink" />{back ? 'Nothing new to look at.' : 'Nothing flagged: no edited tests, dependencies, migrations, CI or secret files, and no failing checks.'}</p>
					{/if}
					{#if seenBefore.length}
						<button type="button" class="flex items-center gap-1.5 self-start px-1 text-xs text-muted-foreground hover:text-foreground" aria-expanded={seenOpen} onclick={() => (seenOpen = !seenOpen)}>
							<ChevronRight class="size-3.5 transition-transform {seenOpen ? 'rotate-90' : ''}" />{seenBefore.length} {seenBefore.length === 1 ? 'flag' : 'flags'} you already looked at
						</button>
						{#if seenOpen}
							<ul class="flex flex-col gap-1 px-1">{#each seenBefore as f (f.id)}<li class="flex items-start gap-2 text-[13px] text-muted-foreground"><Check class="mt-0.5 size-3.5 shrink-0 text-ok-ink" /><span>{f.text}: {f.detail}</span></li>{/each}</ul>
						{/if}
					{/if}
					<ul class="flex flex-col gap-1 px-1">
						{#each facts as fact (fact)}<li class="flex items-start gap-2 text-[13px] text-muted-foreground"><Check class="mt-0.5 size-3.5 shrink-0 text-ok-ink" />{fact}</li>{/each}
						{#if pr?.state === 'open' && unpushed}<li class="flex items-start gap-2 text-[13px] text-muted-foreground"><Info class="mt-0.5 size-3.5 shrink-0" />The latest changes aren't on #{pr.number} yet: push them before merging.</li>{/if}
						{#each notesOnly as f (f.id)}<li class="flex items-start gap-2 text-[13px] text-muted-foreground"><Info class="mt-0.5 size-3.5 shrink-0" /><span>{f.text}: {f.detail}</span></li>{/each}
					</ul>
				</section>

				{#if says}
					<section class="flex flex-col gap-2" aria-labelledby="says-h">
						<h2 id="says-h" class={heading}>The agent says</h2>
						<blockquote class="flex gap-3 rounded-2xl bg-muted/50 px-4 py-3 text-[14px] leading-relaxed">
							<Quote class="mt-1 size-3.5 shrink-0 text-faint" />
							<span class="min-w-0 flex-1 break-words whitespace-pre-wrap">{long && !showAll ? says.slice(0, 700).trimEnd() + '…' : says}{#if long}{' '}<button type="button" class="text-xs text-run-ink underline decoration-dotted underline-offset-2" onclick={() => (showAll = !showAll)}>{showAll ? 'Show less' : 'Show all'}</button>{/if}</span>
						</blockquote>
					</section>
				{/if}

				<section class="flex flex-col gap-2" aria-labelledby="diff-h">
					<div class="flex flex-wrap items-center justify-between gap-2">
						<h2 id="diff-h" class={heading}>{since ? 'Changed since you last looked' : 'Changes'}{away || live ? '' : ' · tap a line to leave a note'}</h2>
						{#if back}
							<span class="flex gap-0.5 rounded-lg bg-muted p-0.5" role="group" aria-label="Which changes">
								<button type="button" class={seg(view === 'since')} aria-pressed={view === 'since'} onclick={() => (view = 'since')}>Since you last looked · {sinceFiles.length}</button>
								<button type="button" class={seg(view === 'all')} aria-pressed={view === 'all'} onclick={() => (view = 'all')}>Everything · {changes?.file_count ?? 0}</button>
							</span>
						{/if}
					</div>
					{#if !files.length}
						<p class="rounded-2xl border bg-card px-4 py-3 text-[13.5px] text-muted-foreground">{since ? 'No files changed since you last looked: the agent only replied.' : 'No changes to files.'}</p>
					{/if}
					{#each files as f, i (`${f.path}:${i}`)}
						<ReviewFile file={f} notes={notesIn(f.path)} flag={since ? undefined : flagByFile.get(f.path)} flash={flash === f.path} readonly={away || live}
							onadd={(n: Note) => review.addNote(id, n)} ondrop={(i: number) => review.dropNote(id, i)} />
					{/each}
				</section>
			{/if}
		</div>

		<!-- One decision, always in reach. -->
		{#if loaded && root && !away && !live}
			<div class="sticky bottom-0 border-t bg-background/95 backdrop-blur">
				<div class="mx-auto flex w-full max-w-3xl flex-col gap-2 px-4 py-3 md:px-8 max-md:pb-[max(0.75rem,env(safe-area-inset-bottom))]">
					<div class="flex flex-wrap items-center gap-2">
						{#if notes.length}
							<Button class="h-10 rounded-full px-5" disabled={!!busy} onclick={sendNotes}><Send class="size-4" />{busy || `Send ${notes.length} ${notes.length === 1 ? 'note' : 'notes'}`}</Button>
						{:else if pr?.state === 'open' && unpushed}
							<Button class="h-10 rounded-full px-5" disabled={!!busy || !canManagePrs} onclick={push}><Upload class="size-4" />{busy || `Push to #${pr.number}`}</Button>
						{:else if pr?.state === 'open'}
							<Button class="h-10 rounded-full px-5 {confirming ? 'bg-ok-ink hover:bg-ok-ink/90' : ''}" disabled={!!busy || !canManagePrs || !!pr.conflicts} onclick={merge}>
								<GitMerge class="size-4" />{busy || (confirming ? `Confirm: ${VERB[method].toLowerCase()} #${pr.number}` : `${VERB[method]} #${pr.number}`)}
							</Button>
						{:else if !pr && root.status !== 'failed'}
							<Button class="h-10 rounded-full px-5" disabled={!!busy || !canManagePrs} onclick={openPr}><GitPullRequest class="size-4" />{busy || 'Open a pull request'}</Button>
						{/if}
						<Button variant="outline" class="h-10 rounded-full" disabled={!!busy} onclick={later}>Not yet</Button>
						<span class="ml-auto text-xs text-faint max-sm:ml-0 max-sm:w-full">
							{#if notes.length}Same thread{pr ? `, same PR #${pr.number}` : ''}. It comes back here when the agent is done.
							{:else if confirming}Click again to merge. It runs on {thread?.machine ?? 'the task’s machine'}.
							{:else if pr?.conflicts}Conflicts: resolve them in the thread first.
							{:else if !canManagePrs}{thread?.machine ? `Update Sidefleet on ${thread.machine} to do this from here.` : ''}
							{:else if looks.length}{looks.length - fresh.length} of {looks.length} flags looked at{rest.length ? ' · then the next one opens' : ''}
							{:else if rest.length}Then the next one opens{/if}
						</span>
					</div>
				</div>
			</div>
		{/if}
	</div>
{/if}
