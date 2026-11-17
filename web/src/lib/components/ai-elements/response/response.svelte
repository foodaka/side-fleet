<script lang="ts">
	import { Streamdown, type StreamdownProps } from 'streamdown-svelte';
	// Add plugins as needed 
	// pnpm add @streamdown-svelte/code @streamdown-svelte/mermaid @streamdown-svelte/math @streamdown-svelte/cjk
	// import { code } from '@streamdown-svelte/code';
	// import { mermaid } from '@streamdown-svelte/mermaid';
	// import { math } from '@streamdown-svelte/math';
	// import { cjk } from '@streamdown-svelte/cjk';
	// import 'katex/dist/katex.min.css';

	import { mode } from 'mode-watcher';
	import githubDarkDefault from '@shikijs/themes/github-dark-default';
	import githubLightDefault from '@shikijs/themes/github-light-default';
	import { cn } from '#lib/utils';
	import StreamingCode from './streaming-code.svelte';
	type Props = StreamdownProps;

	let { content, class: className, components, ...restProps }: Props = $props();
	// Until shiki has loaded a language, a code block shows its lines as plain monospace text (same metrics and indentation as
	// the highlighted ones) instead of Streamdown's grey pulsing bars, so highlighting only recolours the text.
	const codeTheme = { code: { skeleton: 'rounded-none bg-transparent text-foreground scale-y-100 animate-none whitespace-pre' } };
	let currentTheme = $derived(
		mode.current === 'dark' ?  'github-dark-default' : 'github-light-default'
	);
</script>

<div class={cn('size-full [&>*:first-child]:mt-0 [&>*:last-child]:mb-0', className)}>
	<Streamdown
		{content}
		baseTheme="shadcn"
		theme={codeTheme}
		shikiTheme={currentTheme}
		shikiThemes={{
			'github-light-default': githubLightDefault,
			'github-dark-default': githubDarkDefault
		}}
		// plugins={{ code, mermaid, math, cjk }}
		components={{ code: StreamingCode, ...components }}
		{...restProps}
	/>
</div>
