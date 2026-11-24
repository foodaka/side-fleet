<script lang="ts">
	// One payment the agent made from its budget, or one that was refused or failed.
	import Check from '@lucide/svelte/icons/check';
	import Ban from '@lucide/svelte/icons/ban';
	import { usd } from '../format';
	import type { Item } from '../thread.svelte';

	type Payment = Extract<Item, { kind: 'payment' }>;
	let { item }: { item: Payment } = $props();

	const paid = $derived(item.outcome === 'paid');
	const verb = $derived(paid ? 'Paid' : item.outcome === 'failed' ? 'Payment failed:' : 'Not paid:');
</script>

<div class="flex min-w-0 flex-col py-1 text-[13px]">
	<p class="flex min-w-0 items-center gap-2 {paid ? 'text-ok-ink' : 'text-err-ink'}">
		<span class="grid size-5 shrink-0 place-items-center rounded {paid ? 'bg-ok-soft' : 'bg-err-soft'}">
			{#if paid}<Check class="size-3.5" />{:else}<Ban class="size-3.5" />{/if}
		</span>
		<span class="tnum shrink-0">{verb} {usd(item.amountMicros)}</span>
		<code class="min-w-0 truncate rounded bg-code px-1.5 py-0.5 font-mono text-[12px] text-foreground/85" title={item.url}>{item.host}</code>
		{#if item.reference}<span class="ml-auto shrink-0 truncate font-mono text-[11px] text-muted-foreground" title="Receipt: {item.reference}">receipt {item.reference.slice(0, 10)}…</span>{/if}
	</p>
	{#if item.reason || item.detail}
		<p class="mt-0.5 pl-7 text-[12px] text-muted-foreground">{item.detail ?? item.reason}</p>
	{/if}
</div>
