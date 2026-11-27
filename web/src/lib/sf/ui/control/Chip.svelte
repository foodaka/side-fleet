<script lang="ts">
	// A status chip: a soft background in the tone's colour, an optional dot, and a short label.
	import type { Snippet } from 'svelte';
	import { cn } from '#lib/utils';
	import type { Tone } from '../../control';

	let { tone = 'off', dot = true, class: className = '', children }: { tone?: Tone | null; dot?: boolean; class?: string; children: Snippet } = $props();

	const bg: Record<Tone, string> = {
		ok: 'bg-ok-soft text-ok-ink',
		warn: 'bg-queue-soft text-queue-ink',
		err: 'bg-err-soft text-err-ink',
		run: 'bg-run-soft text-run-ink',
		off: 'bg-muted text-muted-foreground'
	};
	const dotBg: Record<Tone, string> = { ok: 'bg-ok', warn: 'bg-queue', err: 'bg-err', run: 'bg-run pulse-run', off: 'bg-off' };
	const t = $derived(tone ?? 'off');
</script>

<span class={cn('inline-flex h-5.5 max-w-full shrink-0 items-center gap-1.5 rounded-full px-2 text-[12px] font-medium whitespace-nowrap', bg[t], className)}>
	{#if dot}<span aria-hidden="true" class="size-1.5 shrink-0 rounded-full {dotBg[t]}"></span>{/if}
	{@render children()}
</span>
