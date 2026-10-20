<script lang="ts">
	// A terminal command: shown on one line (scrolls sideways rather than breaking
	// mid-word), copied in full. `shown` may mask a secret that `copy` contains.
	import CopyButton from './CopyButton.svelte';
	let { shown, copy, label = 'Copy', wrap = false }: { shown: string; copy?: string; label?: string; wrap?: boolean } = $props();
	let code: HTMLElement;
	function selectAll() {
		const r = document.createRange();
		r.selectNodeContents(code);
		const s = getSelection();
		s?.removeAllRanges();
		s?.addRange(r);
	}
</script>

<div class="flex items-center gap-3 rounded-lg bg-[#191919] py-2 pr-2 pl-3.5 text-[#ececE8] dark:bg-black/40 dark:ring-1 dark:ring-white/10">
	<code
		bind:this={code}
		class="min-w-0 flex-1 font-mono text-[12.5px] leading-6 {wrap ? 'break-words whitespace-pre-wrap' : 'overflow-x-auto whitespace-nowrap [scrollbar-width:thin]'}"
	>{shown}</code>
	<CopyButton text={() => copy ?? shown} {label} onfail={selectAll} />
</div>
