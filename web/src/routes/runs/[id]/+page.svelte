<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { Button } from '#lib/components/ui/button';
	import { Conversation, ConversationContent, ConversationScrollButton } from '#lib/components/ai-elements/conversation';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import GitFork from '@lucide/svelte/icons/git-fork';
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import Square from '@lucide/svelte/icons/square';
	import Paperclip from '@lucide/svelte/icons/paperclip';
	import { fleet } from '#lib/sf/state.svelte';
	import { api } from '#lib/sf/api';
	import type { AgentId, Run } from '#lib/sf/types';
	import { agentName, defaultModel, duration, hasAgent, machineModels, modelValid, repoLabel } from '#lib/sf/format';
	import { Thread } from '#lib/sf/thread.svelte';
	import StatusPill from '#lib/sf/ui/StatusPill.svelte';
	import StatusDot from '#lib/sf/ui/StatusDot.svelte';
	import CopyButton from '#lib/sf/ui/CopyButton.svelte';
	import PrIcon from '#lib/sf/ui/PrIcon.svelte';
	import AttachedImages from '#lib/sf/ui/AttachedImages.svelte';
	import AgentModelPicker from '#lib/sf/ui/AgentModelPicker.svelte';
	import ComposerOptions from '#lib/sf/ui/ComposerOptions.svelte';
	import { Attachments, IMAGE_TYPES } from '#lib/sf/attach.svelte';
	import TurnView from '#lib/sf/thread/TurnView.svelte';
	import ForkDialog from '#lib/sf/thread/ForkDialog.svelte';
	import DiffViewer, { type DiffScope } from '#lib/sf/diff/DiffViewer.svelte';
	import DiffStat from '#lib/sf/diff/DiffStat.svelte';
	import FileDiffIcon from '@lucide/svelte/icons/file-diff';

	let thread = $state<Thread | null>(null);
	let loaded = $state(false);
	const id = $derived(page.params.id);

	$effect(() => {
		const runId = id;
		if (!runId) return;
		const t = new Thread();
		untrack(() => { thread = t; loaded = false; });
		t.load(runId).then(() => {
			// Links to a follow-up land on its thread; keep the URL on the root.
			if (t.root && t.root.id !== runId) void goto('#/runs/' + t.root.id, { replaceState: true });
			if (thread === t) loaded = true;
		});
		return () => t.dispose();
	});

	// Fork asked for from the sidebar: from the latest finished message, once the thread has loaded.
	let forkOpen = $state(false);
	const forkTurn = $derived(thread?.turns.findLast((t) => ['succeeded', 'failed', 'stopped'].includes(t.run.status)));
	$effect(() => {
		if (!loaded || !fleet.forkThread || fleet.forkThread !== thread?.root?.id) return;
		untrack(() => {
			fleet.forkThread = null;
			if (forkTurn) forkOpen = true;
			else toast('Nothing to fork yet', { description: 'Fork once the first message has finished.' });
		});
	});

	const root = $derived(thread?.root);
	// The title of the thread this one was forked from.
	let forkSource = $state<string | null>(null);
	$effect(() => {
		const from = root?.forked_from;
		forkSource = null;
		if (from) api<Run[]>(`/v1/runs/${from}/thread`).then((t) => (forkSource = t[0]?.task ?? null)).catch(() => {});
	});
	const latest = $derived(thread?.current);
	const live = $derived(!!thread?.live);
	const machineOnline = $derived(!!fleet.workers.find((w) => w.id === thread?.machine)?.online);
	// The thread's pull request: the list's copy is re-checked by the machine, the thread's is set the moment one opens.
	const pr = $derived(fleet.runs.find((r) => r.id === root?.id)?.pr ?? root?.pr ?? null);
	const prLabel = { open: 'Open', merged: 'Merged', closed: 'Closed' };

	// The review panel: everything the thread changed, or one message's changes.
	let diffOpen = $state(false);
	let diffScope = $state('all');
	let diffFocus = $state<string | null>(null);
	const diffScopes = $derived.by((): DiffScope[] => {
		if (!thread?.changes) return [];
		const changed = thread.turns.filter((t) => t.diff);
		const all: DiffScope = { id: 'all', label: 'All changes', set: thread.changes };
		if (changed.length < 2) return [all];
		return [
			all,
			...changed.map((t) => ({
				id: t.run.id,
				label: t.run.parent_id ? `Message ${thread!.turns.indexOf(t) + 1}` : 'First message',
				hint: t.run.task,
				set: t.diff!.turn
			}))
		];
	});
	function openDiff(runId: string | null, path: string | null) {
		diffScope = runId && diffScopes.some((s) => s.id === runId) ? runId : 'all';
		diffFocus = path;
		diffOpen = true;
	}

	let reply = $state('');
	let perm = $state('');
	let sending = $state(false);
	let stopping = $state(false);
	let box: HTMLTextAreaElement | undefined = $state();
	let picker: HTMLInputElement | undefined = $state();
	const attachments = new Attachments();

	// The agent for the next message: the thread's latest one, or another to switch to.
	let agent = $state<AgentId>('claude');
	let model = $state('');
	let effort = $state('');
	const threadAgent = $derived(thread?.latest?.agent ?? null);
	$effect(() => {
		const a = threadAgent;
		if (!a) return;
		untrack(() => { agent = a; model = thread!.latest!.model; effort = thread!.latest!.effort ?? ''; });
	});
	const switching = $derived(!!threadAgent && agent !== threadAgent);
	// Only Claude Code can ask you about tool calls; the others fall back to editing files.
	const replyPerm = $derived(perm === 'ask' && agent !== 'claude' ? 'acceptEdits' : perm);
	const host = $derived(fleet.workers.filter((w) => w.id === thread?.machine));
	/** The model this agent last used in the thread, else its default. */
	const lastModel = (a: AgentId) => thread?.runs.findLast((r) => r.agent === a)?.model ?? defaultModel(a);
	const lastEffort = (a: AgentId) => thread?.runs.findLast((r) => r.agent === a)?.effort ?? '';
	// A new agent starts a session of its own, so a thread without one can carry on that way.
	const canReply = $derived(!!thread?.hasSession || live || switching);

	async function send(e?: Event) {
		e?.preventDefault();
		if (!thread || !reply.trim() || sending || !modelValid(agent, model)) return;
		sending = true;
		try {
			const pick = switching || model !== thread.latest?.model ? { agent, model } : undefined;
			await thread.reply(reply.trim(), replyPerm || undefined, await attachments.payload(), pick, effort);
			reply = '';
			attachments.clear();
			void fleet.refresh();
		} catch (err) {
			toast.error('Could not send', { description: (err as Error).message });
		}
		sending = false;
	}
	async function sendNow() {
		try { await thread?.interrupt(); } catch (err) { toast.error('Could not interrupt', { description: (err as Error).message }); }
	}
	async function cancel(id: string) {
		try { await thread?.cancel(id); } catch (err) { toast.error('Could not cancel', { description: (err as Error).message }); }
	}
	async function restore(runId: string) {
		try {
			await thread?.restore(runId);
			void fleet.refresh();
		} catch (err) {
			toast.error('Could not restore', { description: (err as Error).message });
		}
	}
	let opening = $state(false);
	async function openPr() {
		opening = true;
		try {
			await thread?.openPr();
			void fleet.refresh();
		} catch (err) {
			toast.error('Could not open a pull request', { description: (err as Error).message });
		}
		opening = false;
	}
	async function stop() {
		stopping = true;
		try { await thread?.stop(); } catch (err) { toast.error('Could not stop', { description: (err as Error).message }); }
		stopping = false;
	}
	function grow() {
		if (!box) return;
		box.style.height = 'auto';
		box.style.height = Math.min(220, Math.max(44, box.scrollHeight)) + 'px';
	}
	$effect(() => { void reply; grow(); });
	const hint = $derived(
		switching ? `Switches to ${agentName(agent)}: a new session with the conversation so far, same files.`
		: live ? 'Your message waits until this one finishes; Send now interrupts it.'
		: !thread?.hasSession ? 'There is no session to continue; start a new task instead.'
		: `Continues the same session on ${thread?.machine ?? 'that machine'}${thread?.branch ? `, on ${thread.branch}` : ''}.`
	);
</script>

<svelte:head><title>{root ? root.task.slice(0, 60) : 'Run'} · Sidefleet</title></svelte:head>
<svelte:window
	ondragenter={(e) => canReply && attachments.dragenter(e)}
	ondragleave={(e) => canReply && attachments.dragleave(e)}
	ondragover={(e) => canReply ? attachments.dragover(e) : e.preventDefault()}
	ondrop={(e) => canReply ? attachments.drop(e) : e.preventDefault()}
/>

<div class="flex h-dvh flex-col max-md:h-[calc(100dvh-var(--sf-top))]">
	<header class="border-b bg-background/80 backdrop-blur">
		<div class="mx-auto flex w-full max-w-3xl flex-col gap-2.5 px-4 pt-5 pb-4 max-md:gap-2 max-md:pt-3 max-md:pb-3 short:gap-1.5 short:pt-2 short:pb-2">
			{#if thread?.error}
				<h1 class="text-xl font-medium">This run doesn't exist</h1>
				<p class="text-[13px] text-muted-foreground">{thread.error}</p>
			{:else if thread && root && latest}
				<div class="flex items-start justify-between gap-4">
					<h1 class="line-clamp-3 text-[19px] leading-snug font-medium tracking-tight max-md:line-clamp-2 max-md:text-[16px] short:line-clamp-1 short:text-[16px]">{root.task}</h1>
					{#if live}
						<Button variant="outline" size="sm" class="h-8 shrink-0 rounded-full border-err/40 text-err-ink hover:bg-err-soft" onclick={stop} disabled={stopping || latest.stop_requested}>
							<Square class="size-3 fill-current" />{stopping || latest.stop_requested ? 'Stopping…' : 'Stop'}
						</Button>
					{/if}
				</div>
				<div class="flex flex-wrap items-center gap-x-3 gap-y-2 text-xs text-muted-foreground max-md:-mx-4 max-md:flex-nowrap max-md:overflow-x-auto max-md:px-4 max-md:[scrollbar-width:none] max-md:[&>*]:shrink-0">
					<StatusPill status={latest.status} extra={latest.status === 'running' ? duration(latest.started_at, null, fleet.now) : ''} />
					{#if thread.machine}
						<span class="inline-flex items-center gap-1.5"><StatusDot status={machineOnline ? 'online' : 'offline'} /><span class="text-foreground/80">{thread.machine}</span></span>
					{/if}
					<span class="font-mono text-[11.5px] max-md:hidden">{repoLabel(root.repo)}</span>
					{#if thread.branch}
						<span class="inline-flex min-w-0 max-w-full items-center gap-1 rounded-md bg-muted py-0.5 pr-0.5 pl-1.5">
							<GitBranch class="size-3 shrink-0" /><span class="truncate font-mono text-[11.5px] text-foreground/80">{thread.branch}</span>
							<CopyButton text={() => thread?.branch ?? ''} label="" class="h-5 bg-transparent px-1 text-muted-foreground hover:bg-accent hover:text-foreground" />
						</span>
					{/if}
					{#if pr}
						<a href={pr.url} target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 rounded-md bg-muted px-1.5 py-0.5 hover:text-foreground" title="Open pull request #{pr.number} on GitHub">
							<PrIcon state={pr.state} /><span class="tnum text-foreground/80">#{pr.number}</span>{prLabel[pr.state]}
						</a>
					{/if}
					{#if thread.runs.length > 1}<span class="tnum max-md:hidden">{thread.runs.length} messages</span>{/if}
					{#if root.forked_from}
						<a href="#/runs/{root.forked_from}" class="inline-flex items-center gap-1 hover:text-foreground"><GitFork class="size-3" />Forked from <span class="max-w-[16rem] truncate text-foreground/80">{forkSource ?? 'another thread'}</span></a>
					{/if}
					{#if thread.changes}
						<button
							type="button"
							class="inline-flex items-center gap-2 rounded-md border bg-card py-0.5 pr-2 pl-1.5 transition-colors hover:bg-muted"
							onclick={() => openDiff(null, null)}
							title="Review changes"
						>
							<FileDiffIcon class="size-3.5" />
							<span class="tnum text-foreground/80">{thread.changes.file_count} file{thread.changes.file_count === 1 ? '' : 's'}</span>
							<DiffStat add={thread.changes.additions} del={thread.changes.deletions} blocks={false} class="text-[11px]" />
						</button>
					{/if}
				</div>
			{:else}
				<div class="h-6 w-2/3 animate-pulse rounded bg-muted"></div>
				<div class="h-4 w-1/3 animate-pulse rounded bg-muted"></div>
			{/if}
		</div>
	</header>

	{#if forkTurn}<ForkDialog bind:open={forkOpen} run={forkTurn.run} hasFiles={!!forkTurn.snapshot} />{/if}

	<Conversation class="min-h-0 flex-1">
		<div class="min-h-0 flex-1 overflow-y-auto overscroll-y-contain" data-scroller>
			<ConversationContent class="mx-auto w-full max-w-3xl gap-10 px-4 py-6">
				{#if thread}
					{#each thread.turns as turn, i (turn.run.id)}
						<TurnView
							{turn}
							first={i === 0}
							prev={thread.turns.slice(0, i).findLast((t) => t.run.kind === 'agent')}
							live={turn.run.id === thread.current?.id && live}
							waiting={thread.waiting(turn.run.id)}
							onsendnow={sendNow}
							oncancel={() => cancel(turn.run.id)}
							onopendiff={openDiff}
							onrestore={!live && turn.run.isolation === 'worktree' ? () => restore(turn.run.id) : undefined}
							ondecide={(item, approve) => thread!.decide(item, approve).then(() => void fleet.refresh())}
						/>
					{/each}
					{#if !live && thread.turns.length && thread.branch}
						{@const branch = thread.branch}
						<div class="flex items-center gap-3 rounded-lg border border-dashed px-3.5 py-2.5 text-xs leading-relaxed text-muted-foreground max-sm:flex-col max-sm:items-stretch">
							<p class="flex-1">
								{#if pr?.state === 'merged'}
									Merged in <a href={pr.url} target="_blank" rel="noopener noreferrer" class="text-foreground/80 underline underline-offset-2">pull request #{pr.number}</a>. The branch <span class="font-mono text-foreground/80">{branch}</span> is still on {thread.machine}.
								{:else if pr?.state === 'open'}
									<a href={pr.url} target="_blank" rel="noopener noreferrer" class="text-foreground/80 underline underline-offset-2">Pull request #{pr.number}</a> is open for <span class="font-mono text-foreground/80">{branch}</span>. Keep working here, then push the new changes to it.
								{:else}
									The changes are on branch <span class="font-mono text-foreground/80">{branch}</span> in {repoLabel(root?.repo)} on {thread.machine}.{#if pr?.state === 'closed'} <a href={pr.url} target="_blank" rel="noopener noreferrer" class="underline underline-offset-2">Pull request #{pr.number}</a> was closed.{/if} Open a pull request, or review, merge or push it from there.
								{/if}
							</p>
							{#if pr?.state !== 'merged'}
								<Button
									size="sm"
									variant={pr?.state === 'open' ? 'outline' : 'default'}
									class="h-8 shrink-0 rounded-full text-xs"
									onclick={openPr}
									disabled={opening || !machineOnline}
									title={machineOnline ? `Commits what's left, pushes ${branch} to origin and opens a pull request with the GitHub CLI on ${thread.machine}` : `${thread.machine} is offline`}
								>
									<GitPullRequest class="size-3.5" />{opening ? 'Starting…' : pr?.state === 'open' ? 'Push changes' : 'Open pull request'}
								</Button>
							{/if}
						</div>
					{/if}
				{/if}
			</ConversationContent>
		</div>
		<ConversationScrollButton class="size-10" aria-label="Scroll to the latest message" />
	</Conversation>

	{#if diffScopes.length}
		<DiffViewer bind:open={diffOpen} scopes={diffScopes} bind:scope={diffScope} focus={diffFocus} />
	{/if}

	{#if root}
		<div class="border-t bg-background">
			<form class="mx-auto flex w-full max-w-3xl flex-col gap-1.5 px-4 pt-3 pb-4 short:pt-2 short:pb-2 max-md:px-3 max-md:pt-2 max-md:pb-[max(0.75rem,env(safe-area-inset-bottom))]" onsubmit={send}>
				<div class="relative rounded-2xl border border-input bg-card focus-within:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]">
					{#if attachments.dragging}
						<div class="pointer-events-none absolute inset-0 z-10 grid place-items-center rounded-2xl border-2 border-dashed border-primary/40 bg-card/90 text-[13px] text-muted-foreground">Drop images to attach them</div>
					{/if}
					<label for="reply" class="sr-only">Reply</label>
					<textarea
						id="reply"
						bind:this={box}
						bind:value={reply}
						rows="1"
						disabled={!canReply}
						onpaste={attachments.paste}
						placeholder={switching ? `Message ${agentName(agent)}…` : live ? 'Queue your next message…' : 'Reply to continue on the same machine…'}
						onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey) send(e); }}
						class="block max-h-[220px] min-h-11 w-full resize-none bg-transparent px-4 pt-3 pb-1 text-[14px] leading-relaxed outline-none placeholder:text-faint"
					></textarea>
					<AttachedImages {attachments} class="px-4 pt-1 pb-2" />
					<div class="flex flex-wrap items-center gap-0.5 px-2 pb-2 min-[22rem]:max-sm:flex-nowrap">
						<AgentModelPicker
							bind:agent
							bind:model
							bind:effort
							models={machineModels(host, agent)}
							id="reply-model"
							size="sm"
							unavailable={(a) => (!host.length || host.some((w) => hasAgent(w, a)) ? '' : `Not on ${thread?.machine}`)}
							onagent={(a) => { model = lastModel(a); effort = lastEffort(a); }}
						/>
						<ComposerOptions {agent} perm={replyPerm} onperm={(v) => (perm = v)} defaultPerm="" sameAs={latest?.permission_mode ?? null} size="sm" />
						<span class="hidden min-w-0 flex-1 basis-0 truncate px-1 text-xs text-faint sm:inline" title={hint}>{hint}</span>
						<button type="button" aria-label="Attach images" title="Attach images (or drop or paste them)" disabled={!canReply} onclick={() => picker?.click()} class="ml-auto grid size-8 shrink-0 place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40">
							<Paperclip class="size-4" />
						</button>
						<input bind:this={picker} type="file" accept={IMAGE_TYPES.join(',')} multiple hidden onchange={(e) => { attachments.add(e.currentTarget.files ?? []); e.currentTarget.value = ''; }} />
						<button type="submit" aria-label={live ? 'Queue' : 'Send'} title={live ? 'Queue: sends when the current message finishes' : 'Send'} disabled={!reply.trim() || sending || !modelValid(agent, model)} class="grid size-8 shrink-0 place-items-center rounded-full bg-primary text-primary-foreground transition-opacity disabled:opacity-25">
							<ArrowUp class="size-4" strokeWidth={2.2} />
						</button>
					</div>
				</div>
			</form>
		</div>
	{/if}
</div>
