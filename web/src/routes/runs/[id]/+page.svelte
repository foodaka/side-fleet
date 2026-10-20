<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import * as Select from '#lib/components/ui/select';
	import { Button } from '#lib/components/ui/button';
	import { Conversation, ConversationContent, ConversationScrollButton } from '#lib/components/ai-elements/conversation';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import Square from '@lucide/svelte/icons/square';
	import { fleet } from '#lib/sf/state.svelte';
	import { duration, PERMISSIONS, permName } from '#lib/sf/format';
	import { Thread } from '#lib/sf/thread.svelte';
	import StatusPill from '#lib/sf/ui/StatusPill.svelte';
	import StatusDot from '#lib/sf/ui/StatusDot.svelte';
	import CopyButton from '#lib/sf/ui/CopyButton.svelte';
	import TurnView from '#lib/sf/thread/TurnView.svelte';

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
	const latest = $derived(thread?.latest);
	const live = $derived(!!thread?.live);
	const machineOnline = $derived(!!fleet.workers.find((w) => w.id === thread?.machine)?.online);

	let reply = $state('');
	let perm = $state('');
	let sending = $state(false);
	let stopping = $state(false);
	let box: HTMLTextAreaElement | undefined = $state();

	async function send(e?: Event) {
		e?.preventDefault();
		if (!thread || !reply.trim() || live || sending) return;
		sending = true;
		try {
			await thread.reply(reply.trim(), perm || undefined);
			reply = '';
			void fleet.refresh();
		} catch (err) {
			toast.error('Could not send', { description: (err as Error).message });
		}
		sending = false;
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
		live ? 'You can reply once this finishes, or stop it.'
		: !thread?.hasSession ? 'There is no session to continue; start a new task instead.'
		: `Continues the same session on ${thread?.machine ?? 'that machine'}${thread?.branch ? `, on ${thread.branch}` : ''}.`
	);
</script>

<svelte:head><title>{root ? root.task.slice(0, 60) : 'Run'} · Sidefleet</title></svelte:head>

<div class="flex h-dvh flex-col md:h-dvh max-md:h-[calc(100dvh-3.25rem)]">
	<header class="border-b bg-background/80 backdrop-blur">
		<div class="mx-auto flex w-full max-w-3xl flex-col gap-2.5 px-4 pt-5 pb-4">
			<a href="#/runs" class="inline-flex w-fit items-center gap-1 text-xs text-muted-foreground hover:text-foreground"><ArrowLeft class="size-3.5" />Runs</a>
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
					<span class="font-mono text-[11.5px]">{root.repo}</span>
					{#if thread.branch}
						<span class="inline-flex items-center gap-1 rounded-md bg-muted py-0.5 pr-0.5 pl-1.5">
							<GitBranch class="size-3" /><span class="font-mono text-[11.5px] text-foreground/80">{thread.branch}</span>
							<CopyButton text={() => thread?.branch ?? ''} label="" class="h-5 bg-transparent px-1 text-muted-foreground hover:bg-accent hover:text-foreground" />
						</span>
					{/if}
					{#if thread.runs.length > 1}<span class="tnum">{thread.runs.length} messages</span>{/if}
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
						<TurnView {turn} first={i === 0} live={live && i === thread.turns.length - 1} />
					{/each}
					{#if !live && thread.turns.length && thread.branch}
						<p class="rounded-lg border border-dashed px-3.5 py-2.5 text-xs leading-relaxed text-muted-foreground">
							The changes are on branch <span class="font-mono text-foreground/80">{thread.branch}</span> in {root?.repo} on {thread.machine}. Review, merge or push it from there.
						</p>
					{/if}
				{/if}
			</ConversationContent>
		</div>
		<ConversationScrollButton />
	</Conversation>

	{#if root}
		<div class="border-t bg-background">
			<form class="mx-auto flex w-full max-w-3xl flex-col gap-1.5 px-4 pt-3 pb-4" onsubmit={send}>
				<div class="rounded-2xl border border-input bg-card focus-within:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]">
					<label for="reply" class="sr-only">Reply</label>
					<textarea
						id="reply"
						bind:this={box}
						bind:value={reply}
						rows="1"
						disabled={!thread?.hasSession && !live}
						placeholder={live ? 'Write your next message…' : 'Reply to continue on the same machine…'}
						onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey) send(e); }}
						class="block max-h-[220px] min-h-11 w-full resize-none bg-transparent px-4 pt-3 pb-1 text-[14px] leading-relaxed outline-none placeholder:text-faint"
					></textarea>
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
						<button type="submit" aria-label="Send" disabled={!reply.trim() || live || sending} class="ml-auto grid size-8 shrink-0 place-items-center rounded-full bg-primary text-primary-foreground transition-opacity disabled:opacity-25">
							<ArrowUp class="size-4" strokeWidth={2.2} />
						</button>
					</div>
				</div>
			</form>
		</div>
	{/if}
</div>
