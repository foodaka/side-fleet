<script lang="ts">
	// How full the agent's context window is: a ring and a percentage, the numbers on hover.
	import * as Tooltip from '#lib/components/ui/tooltip';
	import { cn } from '#lib/utils';
	import type { Context } from '../thread.svelte';
	import type { AgentId } from '../types';
	import { agentName } from '../format';

	let { context, agent, class: className = '' }: { context: Context | null; agent: AgentId; class?: string } = $props();

	const R = 6;
	const C = 2 * Math.PI * R;
	const percent = $derived(context?.limit ? Math.min(100, Math.round((context.used / context.limit) * 100)) : null);
	const tone = $derived(percent == null ? 'text-faint' : percent >= 90 ? 'text-err' : percent >= 75 ? 'text-queue' : 'text-foreground/70');
	const label = $derived(!context ? '' : percent != null ? `${percent}%` : short(context.used));
	const detail = $derived(
		!context
			? `${agentName(agent)} didn't report how full its context is. Older machines and some models don't.`
			: context.limit
				? `Context: ${context.used.toLocaleString()} of ${context.limit.toLocaleString()} tokens (${percent}%).`
				: `Context: ${context.used.toLocaleString()} tokens; ${agentName(agent)} didn't say how big the model's window is.`
	);
	const advice = $derived(percent != null && percent >= 75 ? ' When it fills up the agent condenses the conversation and forgets details; fork or start a new task to begin fresh.' : '');

	function short(n: number) {
		return n >= 1e6 ? `${+(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n);
	}
</script>

<Tooltip.Root>
	<Tooltip.Trigger
		type="button"
		class={cn('inline-flex h-8 shrink-0 items-center gap-1 rounded-full px-1.5 text-[11px] tnum', tone, className)}
		aria-label={detail + advice}
	>
		<svg viewBox="0 0 16 16" class="size-4 -rotate-90" aria-hidden="true">
			<circle cx="8" cy="8" r={R} fill="none" stroke="currentColor" stroke-opacity="0.2" stroke-width="2" />
			{#if percent != null}
				<circle cx="8" cy="8" r={R} fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-dasharray="{(Math.max(percent, 3) / 100) * C} {C}" class="transition-[stroke-dasharray] duration-700" />
			{/if}
		</svg>
		{#if label}<span>{label}</span>{:else}<span class="sr-only">Context unknown</span>{/if}
	</Tooltip.Trigger>
	<Tooltip.Content class="max-w-64">{detail}{advice}</Tooltip.Content>
</Tooltip.Root>
