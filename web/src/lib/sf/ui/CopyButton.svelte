<script lang="ts">
	import { cn } from '#lib/utils';
	import Check from '@lucide/svelte/icons/check';
	import Copy from '@lucide/svelte/icons/copy';
	import { copyText } from '../copy';
	let {
		text,
		label = 'Copy',
		class: className = '',
		onfail
	}: { text: () => string; label?: string; class?: string; onfail?: () => void } = $props();
	let state = $state<'idle' | 'ok' | 'fail'>('idle');
	let timer: ReturnType<typeof setTimeout>;
	async function copy() {
		const ok = await copyText(text());
		state = ok ? 'ok' : 'fail';
		if (!ok) onfail?.();
		clearTimeout(timer);
		timer = setTimeout(() => (state = 'idle'), ok ? 1600 : 3500);
	}
</script>

<button
	type="button"
	onclick={copy}
	class={cn(
		'inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium transition-colors',
		state === 'ok' ? 'bg-ok text-white' : state === 'fail' ? 'bg-err text-white' : 'bg-white/10 text-white hover:bg-white/20',
		className
	)}
	aria-live="polite"
>
	{#if state === 'ok'}<Check class="size-3.5" />Copied{:else if state === 'fail'}Press {navigator.platform.includes('Mac') ? '⌘' : 'Ctrl'}+C{:else}<Copy class="size-3.5" />{label}{/if}
</button>
