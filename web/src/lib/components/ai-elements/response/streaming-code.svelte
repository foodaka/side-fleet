<script lang="ts">
	import Code from 'streamdown-svelte/code';
	import { getContext, onDestroy, untrack, type ComponentProps } from 'svelte';

	let { token, id, isIncomplete = false }: ComponentProps<typeof Code> = $props();

	// Streamdown puts each block's state in this context (the key is its STREAMDOWN_BLOCK_CONTEXT).
	const block = getContext<{ isIncompleteCodeFence: boolean } | undefined>('STREAMDOWN_BLOCK');
	const open = $derived(Boolean(block ? block.isIncompleteCodeFence : isIncomplete));

	// While a fence is still open the agent's text arrives whole every ~100 ms, and Streamdown would run shiki
	// over the entire block each time: work that grows with the block, for text nobody can read at that speed.
	// Instead the block takes on new lines at most every `gap` ms, and only the lines that are finished
	// (the one being written waits for its newline). Once the fence closes, the whole block is highlighted once.
	const finished = (t: string) => t.slice(0, Math.max(0, t.lastIndexOf('\n')));
	const gap = (t: string) => Math.min(500, 150 + 4 * t.split('\n').length);

	// The token handed to Code. It is only replaced when the text or language changes: a new object on every
	// update would make Code highlight again even though nothing it shows has changed.
	let view = $state.raw(untrack(() => ({ ...token, text: open ? finished(token.text) : token.text })));
	let latest = untrack(() => token);
	let last = 0;
	let timer: ReturnType<typeof setTimeout> | undefined;

	function show(text: string) {
		clearTimeout(timer);
		timer = undefined;
		last = performance.now();
		view = { ...latest, text };
	}

	$effect(() => {
		latest = token;
		const text = token.text;
		const lang = token.lang;
		const streaming = open;
		untrack(() => {
			const next = streaming ? finished(text) : text;
			if (next === view.text) {
				if (view.lang !== lang) show(next);
				return;
			}
			const wait = streaming ? gap(next) - (performance.now() - last) : 0;
			if (wait <= 0) show(next);
			else if (!timer) timer = setTimeout(() => show(streaming ? finished(latest.text) : latest.text), wait);
		});
	});
	onDestroy(() => clearTimeout(timer));
</script>

<Code {id} {isIncomplete} token={view} />
