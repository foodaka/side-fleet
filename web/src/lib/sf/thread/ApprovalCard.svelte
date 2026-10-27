<script lang="ts">
	// A tool call the agent is waiting on ("Ask me"): what it wants to do, with Approve and
	// Deny. Once decided it folds to one line, so the thread keeps a record of who allowed what.
	import { toast } from 'svelte-sonner';
	import ShieldQuestion from '@lucide/svelte/icons/shield-question';
	import Check from '@lucide/svelte/icons/check';
	import X from '@lucide/svelte/icons/x';
	import Clock from '@lucide/svelte/icons/clock';
	import { Button } from '#lib/components/ui/button';
	import { fleet } from '../state.svelte';
	import { agentName } from '../format';
	import type { AgentId } from '../types';
	import type { Item } from '../thread.svelte';

	type Approval = Extract<Item, { kind: 'approval' }>;
	let {
		item,
		agent,
		live,
		ondecide
	}: { item: Approval; agent: AgentId; live: boolean; ondecide?: (item: Approval, approve: boolean) => Promise<void> } = $props();

	const mcp = $derived(item.name.match(/^mcp__(.+?)__(.+)$/));
	const ask = $derived(
		item.name === 'Bash' ? 'run a command'
		: /^(Edit|Write|MultiEdit|NotebookEdit)$/.test(item.name) ? 'change a file'
		: item.name === 'WebFetch' ? 'fetch a page'
		: item.name === 'WebSearch' ? 'search the web'
		: mcp ? `use ${mcp[2]} from ${mcp[1]}`
		: `use ${item.name}`
	);
	const input = $derived<Record<string, any>>(item.input ?? {});
	/** The part worth reading in full: the command, file, address or query. */
	const main = $derived(
		typeof input.command === 'string' ? input.command
		: typeof input.file_path === 'string' ? input.file_path
		: typeof input.url === 'string' ? input.url
		: typeof input.query === 'string' ? input.query
		: null
	);
	const detail = $derived(main === null ? JSON.stringify(input, null, 2) : null);
	const note = $derived(typeof input.description === 'string' ? input.description : typeof input.prompt === 'string' ? input.prompt : null);

	const pending = $derived(item.status === 'pending' && live);
	const left = $derived(Math.max(0, new Date(item.expiresAt).getTime() - fleet.now));
	const leftText = $derived(left >= 60_000 ? `${Math.ceil(left / 60_000)} min left` : `${Math.ceil(left / 1000)} s left`);

	let busy = $state<'approve' | 'deny' | null>(null);
	async function decide(approve: boolean) {
		if (!ondecide || busy) return;
		busy = approve ? 'approve' : 'deny';
		try {
			await ondecide(item, approve);
		} catch (e) {
			toast.error(approve ? 'Could not approve' : 'Could not deny', { description: (e as Error).message });
		}
		busy = null;
	}

	const outcome = $derived(
		item.status === 'approved' ? `Approved${item.decidedBy ? ` by ${item.decidedBy}` : ''}`
		: item.status === 'denied' ? `Denied${item.decidedBy ? ` by ${item.decidedBy}` : ''}`
		: item.status === 'expired' ? 'Denied: nobody answered in time'
		: item.status === 'cancelled' ? 'Not answered: the run ended'
		: 'Waiting for approval'
	);
</script>

{#if pending}
	<div class="my-1.5 rounded-xl border border-queue/40 bg-queue-soft/40 p-3 sm:p-3.5" role="group" aria-label="{agentName(agent)} asks to {ask}">
		<p class="flex items-center gap-2 text-[13.5px] font-medium text-foreground">
			<ShieldQuestion class="size-4 shrink-0 text-queue-ink" />
			<span>{agentName(agent)} wants to {ask}</span>
		</p>
		{#if main !== null}
			<pre class="mt-2 max-h-48 overflow-auto rounded-lg bg-code px-3 py-2 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap break-all text-foreground/90">{main}</pre>
		{:else if detail}
			<pre class="mt-2 max-h-48 overflow-auto rounded-lg bg-code px-3 py-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap break-all text-foreground/80">{detail}</pre>
		{/if}
		{#if note}<p class="mt-1.5 text-[12.5px] text-muted-foreground">{note}</p>{/if}
		<div class="mt-3 flex flex-wrap items-center gap-2">
			{#if !ondecide}
				<span class="text-[12.5px] text-muted-foreground">Only the task's owner can answer: it runs on their machine.</span>
			{:else}
			<Button size="sm" class="h-9 min-w-24 rounded-full px-4 max-sm:flex-1" disabled={!!busy || !ondecide} onclick={() => decide(true)}>
				<Check class="size-4" />{busy === 'approve' ? 'Approving…' : 'Approve'}
			</Button>
			<Button size="sm" variant="outline" class="h-9 min-w-24 rounded-full px-4 max-sm:flex-1" disabled={!!busy || !ondecide} onclick={() => decide(false)}>
				<X class="size-4" />{busy === 'deny' ? 'Denying…' : 'Deny'}
			</Button>
			{/if}
			<span class="tnum ml-auto inline-flex items-center gap-1 text-[11.5px] text-muted-foreground max-sm:w-full max-sm:justify-center" title="Denied automatically when the time runs out">
				<Clock class="size-3" />{leftText}
			</span>
		</div>
	</div>
{:else}
	<p class="flex min-w-0 items-center gap-2 py-1 text-[13px] {item.status === 'approved' ? 'text-ok-ink' : item.status === 'denied' ? 'text-err-ink' : 'text-muted-foreground'}">
		<span class="grid size-5 shrink-0 place-items-center rounded {item.status === 'approved' ? 'bg-ok-soft' : item.status === 'denied' ? 'bg-err-soft' : 'bg-muted'}">
			{#if item.status === 'approved'}<Check class="size-3.5" />{:else if item.status === 'pending'}<ShieldQuestion class="size-3.5" />{:else}<X class="size-3.5" />{/if}
		</span>
		<span class="shrink-0">{outcome}</span>
		<code class="min-w-0 truncate rounded bg-code px-1.5 py-0.5 font-mono text-[12px] text-foreground/85">{item.summary || item.name}</code>
	</p>
{/if}
