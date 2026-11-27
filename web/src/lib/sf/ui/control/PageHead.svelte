<script lang="ts">
	// The title row of a control plane page: heading, one line on what it is, and actions on the right.
	import type { Snippet } from 'svelte';
	let { title, sub, actions }: { title: string; sub?: string | Snippet; actions?: Snippet } = $props();
</script>

<header class="flex flex-wrap items-end justify-between gap-4">
	<div class="min-w-0">
		<h1 class="text-[22px] font-semibold tracking-tight">{title}</h1>
		{#if typeof sub === 'string'}<p class="mt-1 text-[13px] text-muted-foreground">{sub}</p>{:else if sub}<p class="mt-1 text-[13px] text-muted-foreground">{@render sub()}</p>{/if}
	</div>
	{#if actions}<div class="flex flex-wrap items-center gap-2">{@render actions()}</div>{/if}
</header>
