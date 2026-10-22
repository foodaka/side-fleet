<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import * as Select from '#lib/components/ui/select';
	import { Button } from '#lib/components/ui/button';
	import { Conversation, ConversationContent, ConversationScrollButton } from '#lib/components/ai-elements/conversation';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import Square from '@lucide/svelte/icons/square';
	import Paperclip from '@lucide/svelte/icons/paperclip';
	import { fleet } from '#lib/sf/state.svelte';
	import { duration, PERMISSIONS, permName, repoLabel } from '#lib/sf/format';
	import { Thread } from '#lib/sf/thread.svelte';
	import StatusPill from '#lib/sf/ui/StatusPill.svelte';
	import StatusDot from '#lib/sf/ui/StatusDot.svelte';
	import CopyButton from '#lib/sf/ui/CopyButton.svelte';
	import AttachedImages from '#lib/sf/ui/AttachedImages.svelte';
	import { Attachments, IMAGE_TYPES } from '#lib/sf/attach.svelte';
	import TurnView from '#lib/sf/thread/TurnView.svelte';
	import DiffViewer, { type DiffScope } from '#lib/sf/diff/DiffViewer.svelte';
	import DiffStat from '#lib/sf/diff/DiffStat.svelte';
	import FileDiffIcon from '@lucide/svelte/icons/file-diff';

	let thread = $state<Thread | null>(null);
	const id = $derived(page.params.id);

	$effect(() => {
		const runId = id;
		if (!runId) return;
		const t = new Thread();
		untrack(() => (thread = t));
		t.load(runId).then(() => {
			// Links to a follow-up land on its thread; keep the URL on the root.
			if (t.root && t.root.id !== runId) void goto('#/runs/' + t.root.id, { replaceState: true });
		});
		return () => t.dispose();
	});

	const root = $derived(thread?.root);
	const latest = $derived(thread?.current);
	const live = $derived(!!thread?.live);
	const machineOnline = $derived(!!fleet.workers.find((w) => w.id === thread?.machine)?.online);

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
	const canReply = $derived(!!thread?.hasSession || live);

	async function send(e?: Event) {
		e?.preventDefault();
		if (!thread || !reply.trim() || sending) return;
		sending = true;
		try {
			await thread.reply(reply.trim(), perm || undefined, await attachments.payload());
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
		live ? 'Your message waits until this one finishes; Send now interrupts it.'
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

<div class="flex h-dvh flex-col max-md:h-[calc(100dvh-3.25rem)]">
	<header class="border-b bg-background/80 backdrop-blur">
		<div class="mx-auto flex w-full max-w-3xl flex-col gap-2.5 px-4 pt-5 pb-4">
			{#if thread?.error}
				<h1 class="text-xl font-medium">This run doesn't exist</h1>
				<p class="text-[13px] text-muted-foreground">{thread.error}</p>
			{:else if thread && root && latest}
				<div class="flex items-start justify-between gap-4">
					<h1 class="line-clamp-3 text-[19px] leading-snug font-medium tracking-tight">{root.task}</h1>
					{#if live}
						<Button variant="outline" size="sm" class="h-8 shrink-0 rounded-full border-err/40 text-err-ink hover:bg-err-soft" onclick={stop} disabled={stopping || latest.stop_requested}>
							<Square class="size-3 fill-current" />{stopping || latest.stop_requested ? 'Stopping…' : 'Stop'}
						</Button>
					{/if}
				</div>
				<div class="flex flex-wrap items-center gap-x-3 gap-y-2 text-xs text-muted-foreground">
					<StatusPill status={latest.status} extra={latest.status === 'running' ? duration(latest.started_at, null, fleet.now) : ''} />
					{#if thread.machine}
						<span class="inline-flex items-center gap-1.5"><StatusDot status={machineOnline ? 'online' : 'offline'} /><span class="text-foreground/80">{thread.machine}</span></span>
					{/if}
					<span class="font-mono text-[11.5px]">{repoLabel(root.repo)}</span>
					{#if thread.branch}
						<span class="inline-flex items-center gap-1 rounded-md bg-muted py-0.5 pr-0.5 pl-1.5">
							<GitBranch class="size-3" /><span class="font-mono text-[11.5px] text-foreground/80">{thread.branch}</span>
							<CopyButton text={() => thread?.branch ?? ''} label="" class="h-5 bg-transparent px-1 text-muted-foreground hover:bg-accent hover:text-foreground" />
						</span>
					{/if}
					{#if thread.runs.length > 1}<span class="tnum">{thread.runs.length} messages</span>{/if}
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

	<Conversation class="min-h-0 flex-1">
		<div class="min-h-0 flex-1 overflow-y-auto" data-scroller>
			<ConversationContent class="mx-auto w-full max-w-3xl gap-10 px-4 py-6">
				{#if thread}
					{#each thread.turns as turn, i (turn.run.id)}
						<TurnView
							{turn}
							first={i === 0}
							live={turn.run.id === thread.current?.id && live}
							waiting={thread.waiting(turn.run.id)}
							onsendnow={sendNow}
							oncancel={() => cancel(turn.run.id)}
							onopendiff={openDiff}
						/>
					{/each}
					{#if !live && thread.turns.length && thread.branch}
						<p class="rounded-lg border border-dashed px-3.5 py-2.5 text-xs leading-relaxed text-muted-foreground">
							The changes are on branch <span class="font-mono text-foreground/80">{thread.branch}</span> in {repoLabel(root?.repo)} on {thread.machine}. Review, merge or push it from there.
						</p>
					{/if}
				{/if}
			</ConversationContent>
		</div>
		<ConversationScrollButton />
	</Conversation>

	{#if diffScopes.length}
		<DiffViewer bind:open={diffOpen} scopes={diffScopes} bind:scope={diffScope} focus={diffFocus} />
	{/if}

	{#if root}
		<div class="border-t bg-background">
			<form class="mx-auto flex w-full max-w-3xl flex-col gap-1.5 px-4 pt-3 pb-4" onsubmit={send}>
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
						placeholder={live ? 'Queue your next message…' : 'Reply to continue on the same machine…'}
						onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey) send(e); }}
						class="block max-h-[220px] min-h-11 w-full resize-none bg-transparent px-4 pt-3 pb-1 text-[14px] leading-relaxed outline-none placeholder:text-faint"
					></textarea>
					<AttachedImages {attachments} class="px-4 pt-1 pb-2" />
					<div class="flex items-center gap-2 px-2.5 pb-2.5">
						<Select.Root type="single" value={perm} onValueChange={(v) => (perm = v)}>
							<Select.Trigger size="sm" class="h-7 rounded-full border-transparent bg-muted text-xs shadow-none hover:bg-accent" aria-label="Permissions for this reply">
								{perm ? permName(perm) : `Same permissions${latest?.permission_mode ? ` (${permName(latest.permission_mode)})` : ''}`}
							</Select.Trigger>
							<Select.Content>
								<Select.Item value="" label="Same permissions">Same permissions</Select.Item>
								{#each PERMISSIONS as p (p.id)}<Select.Item value={p.id} label={p.name}><span class="flex flex-col"><span>{p.name}</span><span class="text-xs text-muted-foreground">{p.hint}</span></span></Select.Item>{/each}
							</Select.Content>
						</Select.Root>
						<span class="hidden min-w-0 truncate text-xs text-faint sm:inline">{hint}</span>
						<button type="button" aria-label="Attach images" title="Attach images (or drop or paste them)" disabled={!canReply} onclick={() => picker?.click()} class="ml-auto grid size-8 shrink-0 place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40">
							<Paperclip class="size-4" />
						</button>
						<input bind:this={picker} type="file" accept={IMAGE_TYPES.join(',')} multiple hidden onchange={(e) => { attachments.add(e.currentTarget.files ?? []); e.currentTarget.value = ''; }} />
						<button type="submit" aria-label={live ? 'Queue' : 'Send'} title={live ? 'Queue: sends when the current message finishes' : 'Send'} disabled={!reply.trim() || sending} class="grid size-8 shrink-0 place-items-center rounded-full bg-primary text-primary-foreground transition-opacity disabled:opacity-25">
							<ArrowUp class="size-4" strokeWidth={2.2} />
						</button>
					</div>
				</div>
			</form>
		</div>
	{/if}
</div>
