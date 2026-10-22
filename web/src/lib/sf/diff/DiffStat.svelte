<script lang="ts">
	// "+12 −3 ■■■□□": counts, and GitHub's five blocks for the proportion.
	import { statBlocks } from './parse';

	let { add, del, blocks = true, class: className = '' }: { add: number; del: number; blocks?: boolean; class?: string } = $props();
	const b = $derived(statBlocks(add, del));
</script>

{#if add || del}
<span class="tnum inline-flex items-center gap-1.5 font-mono text-[11.5px] {className}">
	{#if add}<span class="text-diff-add-ink">+{add.toLocaleString()}</span>{/if}
	{#if del}<span class="text-diff-del-ink">−{del.toLocaleString()}</span>{/if}
	{#if blocks}
		<span class="flex gap-px" aria-hidden="true">
			{#each { length: b[0] } as _, i (i)}<span class="size-[7px] rounded-[1.5px] bg-diff-add"></span>{/each}
			{#each { length: b[1] } as _, i (i)}<span class="size-[7px] rounded-[1.5px] bg-diff-del"></span>{/each}
			{#each { length: b[2] } as _, i (i)}<span class="size-[7px] rounded-[1.5px] bg-foreground/12"></span>{/each}
		</span>
	{/if}
</span>
{/if}
