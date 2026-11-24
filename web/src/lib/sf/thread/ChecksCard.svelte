<script lang="ts">
	// The repo's checks (`.sidefleet.json`) after the agent's turn: one line saying how they went,
	// opening to each check and the end of its output. A failed check opens by default.
	import * as Collapsible from '#lib/components/ui/collapsible';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import CircleX from '@lucide/svelte/icons/circle-x';
	import CircleMinus from '@lucide/svelte/icons/circle-minus';
	import { agentName, durationMs } from '../format';
	import type { ChecksItem } from '../thread.svelte';
	import type { AgentId, CheckResult } from '../types';

	let { item, agent }: { item: ChecksItem; agent: AgentId } = $props();

	const failed = $derived(item.checks.find((c) => !c.passed));
	const headline = $derived.by(() => {
		switch (item.verdict) {
			case 'passed':
				return item.checks.length === 1 ? `Check passed: ${item.checks[0].name}` : `All ${item.checks.length} checks passed`;
			case 'failed':
				return `Check failed: ${failed?.name ?? 'unknown'}${item.retrying ? ` · asked ${agentName(agent)} to fix it` : ''}`;
			case 'skipped':
				return `Checks not run: ${item.reason ?? 'not for this task'}`;
			default:
				return `Checks not run: ${item.reason ?? 'the check list could not be used'}`;
		}
	});
	const tone = $derived(item.verdict === 'passed' ? 'text-ok-ink' : item.verdict === 'skipped' ? 'text-muted-foreground' : 'text-err-ink');
	const how = (c: CheckResult) =>
		c.passed ? durationMs(c.duration_ms) : c.timed_out ? 'timed out' : c.exit_code != null ? `exit ${c.exit_code}` : 'killed';
</script>

<div class="my-1 rounded-lg border px-3 py-2">
	<p class="flex items-center gap-2 text-[13px] {tone}">
		{#if item.verdict === 'passed'}<CircleCheck class="size-4 shrink-0" />{:else if item.verdict === 'skipped'}<CircleMinus class="size-4 shrink-0" />{:else}<CircleX class="size-4 shrink-0" />{/if}
		<span class="min-w-0">{headline}</span>
	</p>
	{#if item.checks.length}
		<div class="mt-1.5 flex flex-col">
			{#each item.checks as c (c.name)}
				<Collapsible.Root open={!c.passed} class="group/check">
					<Collapsible.Trigger class="flex w-full items-center gap-2 rounded py-0.5 text-left text-[12.5px] text-muted-foreground hover:text-foreground">
						{#if c.passed}<CircleCheck class="size-3.5 shrink-0 text-ok-ink" />{:else}<CircleX class="size-3.5 shrink-0 text-err-ink" />{/if}
						<span class="shrink-0 text-foreground/85">{c.name}</span>
						<code class="min-w-0 truncate rounded bg-code px-1.5 py-0.5 font-mono text-[11.5px]">{c.run}</code>
						<span class="tnum ml-auto shrink-0 text-[11.5px] {c.passed ? '' : 'text-err-ink'}">{how(c)}</span>
						<ChevronRight class="size-3.5 shrink-0 opacity-50 transition-transform group-data-[state=open]/check:rotate-90" />
					</Collapsible.Trigger>
					<Collapsible.Content>
						<pre
							class="mt-1 mb-1.5 max-h-72 overflow-auto rounded-md border px-2.5 py-2 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap {c.passed ? 'bg-code text-foreground/80' : 'border-err/30 bg-err-soft/50 text-err-ink'}"
						>{c.output || '(no output)'}</pre>
					</Collapsible.Content>
				</Collapsible.Root>
			{/each}
		</div>
		{#if item.verdict === 'failed' && !item.retrying && item.attempt > 1}
			<p class="mt-1 text-xs text-muted-foreground">Still failing after {agentName(agent)}'s fix. Reply to have it try again, or take it from here.</p>
		{/if}
	{/if}
</div>
