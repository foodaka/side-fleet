<script lang="ts">
	// A terminal command, copied in full. It wraps so the whole thing is visible,
	// preferring spaces and only splitting a long URL when it must.
	// `shown` may mask a secret that `copy` contains.
	import CopyButton from './CopyButton.svelte';
	let { shown, copy, label = 'Copy' }: { shown: string; copy?: string; label?: string; wrap?: boolean } = $props();
	let code: HTMLElement;
	function selectAll() {
		const r = document.createRange();
		r.selectNodeContents(code);
		const s = getSelection();
		s?.removeAllRanges();
		s?.addRange(r);
	}
</script>

<div class="flex items-start gap-3 rounded-lg bg-[#191919] py-2 pr-2 pl-3.5 text-[#ececE8] dark:bg-black/40 dark:ring-1 dark:ring-white/10">
	<code
		bind:this={code}
		class="min-w-0 flex-1 font-mono text-[12.5px] leading-6 whitespace-pre-wrap [overflow-wrap:anywhere]"
	>{shown}</code>
	<CopyButton text={() => copy ?? shown} {label} onfail={selectAll} class="mt-px" />
</div>
