<script lang="ts">
	import { Message, MessageContent } from '#lib/components/ai-elements/message';
	import { Response } from '#lib/components/ai-elements/response';
	import { Shimmer } from '#lib/components/ai-elements/shimmer';
	import * as Collapsible from '#lib/components/ui/collapsible';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import ArrowRightLeft from '@lucide/svelte/icons/arrow-right-left';
	import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
	import GitFork from '@lucide/svelte/icons/git-fork';
	import { agentModel, agentName, clock, permName, repoLabel } from '../format';
	import ToolGroup from './ToolGroup.svelte';
	import ApprovalCard from './ApprovalCard.svelte';
	import ToolImages from './ToolImages.svelte';
	import ChangesCard from '../diff/ChangesCard.svelte';
	import ForkDialog from './ForkDialog.svelte';
	import type { Turn } from '../thread.svelte';

	let {
		turn,
		first,
		prev,
		live,
		waiting = false,
		onsendnow,
		oncancel,
		onopendiff,
		onrestore,
		ondecide
	}: {
		turn: Turn;
		first: boolean;
		/** The thread's previous non-restore turn, to tell what this one changed. */
		prev?: Turn;
		live: boolean;
		/** A message queued behind one that is still running. */
		waiting?: boolean;
		onsendnow?: () => void;
		oncancel?: () => void;
		onopendiff?: (runId: string, path: string | null) => void;
		/** Put the files back to how they were before this turn; absent when that isn't possible now. */
		onrestore?: () => Promise<void>;
		/** Approve or deny a tool call the agent is waiting on. */
		ondecide?: (item: Extract<Turn['items'][number], { kind: 'approval' }>, approve: boolean) => Promise<void>;
	} = $props();
	const run = $derived(turn.run);
	const isRestore = $derived(run.kind === 'restore');
	const finished = $derived(run.status === 'succeeded' || run.status === 'failed' || run.status === 'stopped');
	let forking = $state(false);
	let restoring = $state(false);
	let confirming = $state(false);

	async function restore() {
		if (!onrestore) return;
		if (!confirming) {
			confirming = true;
			setTimeout(() => (confirming = false), 4000);
			return;
		}
		restoring = true;
		try { await onrestore(); } finally { restoring = false; confirming = false; }
	}
	// Consecutive tool calls fold into one line, and consecutive stderr lines into one log,
	// so neither breaks up the narration.
	type Tool = Extract<(typeof turn.items)[number], { kind: 'tool' }>;
	const groups = $derived.by(() => {
		const out: (
			| (typeof turn.items)[number]
			| { kind: 'stderr-group'; key: string; lines: string[] }
			| { kind: 'tool-group'; key: string; tools: Tool[] }
		)[] = [];
		for (const it of turn.items) {
			const last = out[out.length - 1];
			if (it.kind === 'stderr') {
				if (last && last.kind === 'stderr-group') last.lines.push(it.text);
				else out.push({ kind: 'stderr-group', key: it.key, lines: [it.text] });
			} else if (it.kind === 'tool') {
				if (last && last.kind === 'tool-group') last.tools.push(it);
				else out.push({ kind: 'tool-group', key: it.key, tools: [it] });
			} else out.push(it);
		}
		return out;
	});
	// A follow-up only says where it runs when that differs from the turn before; the header
	// already names the thread's machine and branch.
	const switched = $derived(!!prev && (run.agent !== prev.run.agent || run.model !== prev.run.model));
	const relocated = $derived(
		!prev || !run.parent_id || turn.machine !== prev.machine || (!!turn.branch && !!prev.branch && turn.branch !== prev.branch)
	);
	const meta = $derived(
		[run.parent_id ? null : clock(run.created_at), run.permission_mode ? permName(run.permission_mode) : null, !run.parent_id && run.isolation === 'checkout' ? 'In place' : null].filter(Boolean)
	);
</script>

<div class="flex flex-col gap-4">
	{#if first && run.images?.length}
		<ToolImages images={run.images} inset={false} alt="Attached to the task" />
	{/if}
	{#if run.moved_from}
		<p class="flex items-center gap-1.5 text-[13px] text-muted-foreground">
			<ArrowRightLeft class="size-3.5 shrink-0" />
			<span>
				Moved{turn.movedFrom ? ` from ${turn.movedFrom}` : ''} to {turn.machine ?? 'another machine'}{#if run.resume_from_branch}, continuing from <span class="font-mono text-[12px]">{run.resume_from_branch}</span>{:else}, starting over (no work had been pushed){/if}
			</span>
		</p>
	{:else if isRestore}
		<p class="flex items-center justify-center gap-1.5 text-xs text-muted-foreground"><RotateCcw class="size-3" />{run.task}</p>
	{:else if !first}
		<Message from="user">
			<MessageContent class="max-w-[85%] rounded-2xl rounded-br-md bg-primary px-4 py-2.5 text-[14px] leading-relaxed whitespace-pre-wrap text-primary-foreground group-[.is-user]:bg-primary group-[.is-user]:text-primary-foreground">{run.task}</MessageContent>
			{#if run.images?.length}<div class="self-end"><ToolImages images={run.images} inset={false} alt="Attached to the message" /></div>{/if}
			{#if waiting}
				<span class="flex items-center gap-2 self-end text-[11px] text-muted-foreground">
					<span>Queued, sends when the current message finishes</span>
					<button type="button" class="rounded-full bg-primary px-2.5 whitespace-nowrap py-1 font-medium text-primary-foreground hover:opacity-90" onclick={onsendnow}>Send now</button>
					<button type="button" class="rounded-full px-2 py-1 whitespace-nowrap hover:bg-accent hover:text-foreground" onclick={oncancel}>Cancel</button>
				</span>
			{:else}
				<span class="self-end text-[11px] text-faint">{meta.join(', ')}</span>
			{/if}
		</Message>
	{/if}

	<div class="flex flex-col gap-1.5">
		{#if !waiting && !isRestore && ((turn.activity === 'queued' && !turn.machine) || (turn.machine && (relocated || switched)))}
			<Collapsible.Root class="group/setup">
				<Collapsible.Trigger class="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-left text-xs text-muted-foreground hover:text-foreground">
					{#if turn.activity === 'queued' && !turn.machine}
						Waiting for {run.pinned_worker_id ?? `${run.moved_from ? 'another' : 'a'} machine with ${agentName(run.agent)} and ${repoLabel(run.repo)}`}…
					{:else if !relocated}
						<span>Switched to {agentModel(run.agent, run.model)}</span>
					{:else}
						{run.parent_id ? 'Continued' : 'Started'} on <span class="text-foreground/80">{turn.machine}</span>
						{#if turn.branch}<span class="inline-flex items-center gap-1"><GitBranch class="size-3" /><span class="font-mono">{turn.branch}</span></span>{:else if turn.workspaceNote}<span>{turn.workspaceNote}</span>{/if}
						{#if turn.checkpoint}<span title="Last checkpoint pushed to origin">pushed <span class="font-mono">{turn.checkpoint.slice(0, 7)}</span></span>{/if}
						<span>with {agentModel(run.agent, run.model)}</span>
					{/if}
				</Collapsible.Trigger>
				{#if turn.command}
					<Collapsible.Content>
						<code class="mt-1.5 block rounded-md bg-code px-2.5 py-1.5 font-mono text-[11.5px] break-all text-muted-foreground">{turn.command}</code>
					</Collapsible.Content>
				{/if}
			</Collapsible.Root>
		{/if}

		<div class="flex flex-col gap-1">
			{#each groups as it (it.key)}
				{#if it.kind === 'text'}
					<div class="sf-md py-1.5 text-[14.5px] leading-[1.7]"><Response content={it.text} /></div>
				{:else if it.kind === 'tool-group'}
					<ToolGroup tools={it.tools} {live} />
				{:else if it.kind === 'approval'}
					<ApprovalCard item={it} agent={run.agent} {live} {ondecide} />
				{:else if it.kind === 'note'}
					<p class="py-1 text-[13px] {it.tone === 'err' ? 'text-err-ink' : it.tone === 'ok' ? 'text-ok-ink' : 'text-muted-foreground'}">{it.text}</p>
				{:else if it.kind === 'stderr-group'}
					<details class="py-0.5 text-xs text-muted-foreground">
						<summary class="cursor-pointer select-none hover:text-foreground">{it.lines.length} line{it.lines.length === 1 ? '' : 's'} of agent log</summary>
						<pre class="mt-1 max-h-48 overflow-auto rounded-md bg-code px-2.5 py-2 font-mono text-[11.5px] whitespace-pre-wrap">{it.lines.join('\n')}</pre>
					</details>
				{/if}
			{/each}
		</div>

		{#if live && turn.activity && turn.activity !== 'queued'}
			<div class="flex items-center gap-2 py-1.5" role="status">
				<span class="size-1.5 rounded-full bg-run pulse-run"></span>
				<Shimmer as="span" class="text-[13px]" content_length={turn.activity.length}>{turn.activity}</Shimmer>
				{#if turn.reconnecting}<span class="text-xs text-queue-ink">reconnecting…</span>{/if}
			</div>
		{/if}

		{#if turn.result?.text}
			<div class="sf-md py-1.5 text-[14.5px] leading-[1.7]"><Response content={turn.result.text} /></div>
		{/if}

		{#if turn.diff && onopendiff}
			<ChangesCard set={turn.diff.turn} onopen={(path) => onopendiff(run.id, path)} />
		{/if}

		{#if turn.result}
			<p class="tnum flex flex-wrap gap-x-3 pt-1 text-xs text-muted-foreground">
				{#if !turn.result.ok}<span class="text-err-ink">{agentName(run.agent)} reported an error{turn.result.subtype && turn.result.subtype !== 'error' ? ` (${turn.result.subtype})` : ''}</span>{/if}
				{#if turn.result.durationMs != null}<span>{(turn.result.durationMs / 1000).toFixed(1)}s</span>{/if}
				{#if turn.result.turns != null}<span>{turn.result.turns} turn{turn.result.turns === 1 ? '' : 's'}</span>{/if}
				{#if turn.result.cost != null}<span>${turn.result.cost.toFixed(4)}</span>{:else if turn.result.tokens}<span>{turn.result.tokens.toLocaleString()} tokens</span>{/if}
			</p>
		{/if}

		{#if finished && !live}
			<div class="flex justify-end gap-1">
				{#if onrestore && turn.snapshot}
					<button
						type="button"
						onclick={restore}
						disabled={restoring}
						title="Put the files back to how they were before this message. The conversation carries on."
						class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs transition-colors disabled:opacity-50 {confirming ? 'bg-err-soft text-err-ink' : 'text-muted-foreground hover:bg-muted hover:text-foreground'}"
					><RotateCcw class="size-3" />{restoring ? 'Restoring…' : confirming ? 'Click again to restore' : 'Restore to before'}</button>
				{/if}
				<button
					type="button"
					onclick={() => (forking = true)}
					title="Start a new thread from here, with any agent and model"
					class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
				><GitFork class="size-3" />Fork from here</button>
			</div>
			<ForkDialog bind:open={forking} {run} hasFiles={!!turn.snapshot} />
		{/if}
	</div>
</div>
