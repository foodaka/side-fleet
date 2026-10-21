<script lang="ts">
	import { Message, MessageContent } from '#lib/components/ai-elements/message';
	import { Response } from '#lib/components/ai-elements/response';
	import { Shimmer } from '#lib/components/ai-elements/shimmer';
	import * as Collapsible from '#lib/components/ui/collapsible';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import { agentModel, agentName, clock, permName, repoLabel } from '../format';
	import ToolRow from './ToolRow.svelte';
	import type { Turn } from '../thread.svelte';

	let { turn, first, live }: { turn: Turn; first: boolean; live: boolean } = $props();
	const run = $derived(turn.run);
	// Consecutive stderr lines are grouped so they don't break up the narration.
	const groups = $derived.by(() => {
		const out: (typeof turn.items[number] | { kind: 'stderr-group'; key: string; lines: string[] })[] = [];
		for (const it of turn.items) {
			const last = out[out.length - 1];
			if (it.kind === 'stderr') {
				if (last && last.kind === 'stderr-group') last.lines.push(it.text);
				else out.push({ kind: 'stderr-group', key: it.key, lines: [it.text] });
			} else out.push(it);
		}
		return out;
	});
	const meta = $derived(
		[run.parent_id ? null : clock(run.created_at), run.permission_mode ? permName(run.permission_mode) : null, !run.parent_id && run.isolation === 'checkout' ? 'In place' : null].filter(Boolean)
	);
</script>

<div class="flex flex-col gap-4">
	{#if !first}
		<Message from="user">
			<MessageContent class="max-w-[85%] rounded-2xl rounded-br-md bg-primary px-4 py-2.5 text-[14px] leading-relaxed whitespace-pre-wrap text-primary-foreground group-[.is-user]:bg-primary group-[.is-user]:text-primary-foreground">{run.task}</MessageContent>
			<span class="self-end text-[11px] text-faint">{meta.join(', ')}</span>
		</Message>
	{/if}

	<div class="flex flex-col gap-1.5">
		{#if turn.machine || turn.activity === 'queued'}
			<Collapsible.Root class="group/setup">
				<Collapsible.Trigger class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground">
					{#if turn.activity === 'queued' && !turn.machine}
						Waiting for {run.pinned_worker_id ?? `a machine with ${agentName(run.agent)} and ${repoLabel(run.repo)}`}…
					{:else}
						{run.resume_session_id ? 'Continued' : 'Started'} on <span class="text-foreground/80">{turn.machine}</span>
						{#if turn.branch}<span class="inline-flex items-center gap-1"><GitBranch class="size-3" /><span class="font-mono">{turn.branch}</span></span>{:else if turn.workspaceNote}<span>{turn.workspaceNote}</span>{/if}
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
				{:else if it.kind === 'tool'}
					<ToolRow item={it} {live} />
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

		{#if turn.result}
			{#if turn.result.text}
				<div class="sf-md py-1.5 text-[14.5px] leading-[1.7]"><Response content={turn.result.text} /></div>
			{/if}
			<p class="tnum flex flex-wrap gap-x-3 pt-1 text-xs text-muted-foreground">
				{#if !turn.result.ok}<span class="text-err-ink">{agentName(run.agent)} reported an error{turn.result.subtype && turn.result.subtype !== 'error' ? ` (${turn.result.subtype})` : ''}</span>{/if}
				{#if turn.result.durationMs != null}<span>{(turn.result.durationMs / 1000).toFixed(1)}s</span>{/if}
				{#if turn.result.turns != null}<span>{turn.result.turns} turn{turn.result.turns === 1 ? '' : 's'}</span>{/if}
				{#if turn.result.cost != null}<span>${turn.result.cost.toFixed(4)}</span>{:else if turn.result.tokens}<span>{turn.result.tokens.toLocaleString()} tokens</span>{/if}
			</p>
		{/if}
	</div>
</div>
