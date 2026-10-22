<script lang="ts">
	// One tool call in the narration: a single line ("Ran `git status`") that opens to its output.
	import * as Collapsible from '#lib/components/ui/collapsible';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import Terminal from '@lucide/svelte/icons/square-terminal';
	import FileText from '@lucide/svelte/icons/file-text';
	import Search from '@lucide/svelte/icons/search';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Globe from '@lucide/svelte/icons/globe';
	import Wrench from '@lucide/svelte/icons/wrench';
	import ListChecks from '@lucide/svelte/icons/list-checks';
	import { Loader } from '#lib/components/ai-elements/loader';
	import { toolVerb, type Item } from '../thread.svelte';
	import ToolImages from './ToolImages.svelte';

	let { item, live }: { item: Extract<Item, { kind: 'tool' }>; live: boolean } = $props();
	const icon = $derived(
		item.name === 'Bash' ? Terminal
		: /^(Read)$/.test(item.name) ? FileText
		: /^(Grep|Glob)$/.test(item.name) ? Search
		: /^(Edit|Write|MultiEdit|NotebookEdit)$/.test(item.name) ? Pencil
		: /^Web/.test(item.name) ? Globe
		: item.name === 'TodoWrite' ? ListChecks
		: Wrench
	);
	const Icon = $derived(icon);
	const pending = $derived(item.output === null && live);
	const firstLine = $derived((item.output ?? '').split('\n').find((l) => l.trim()) ?? '');
	// Failures open by default: they are what you need to read.
	let open = $state(false);
	$effect(() => { if (item.isError) open = true; });
</script>

<Collapsible.Root bind:open class="group/tool">
	<Collapsible.Trigger
		class="flex w-full items-center gap-2.5 rounded-md py-1 text-left text-[13px] text-muted-foreground transition-colors hover:text-foreground"
		disabled={item.output === null}
	>
		<span class="grid size-5 shrink-0 place-items-center rounded {item.isError ? 'bg-err-soft text-err-ink' : 'bg-muted'}">
			{#if pending}<Loader size={12} />{:else}<Icon class="size-3.5" />{/if}
		</span>
		<span class="shrink-0">{toolVerb(item.name)}</span>
		<code class="min-w-0 truncate rounded bg-code px-1.5 py-0.5 font-mono text-[12px] text-foreground/85">{item.summary || item.name}</code>
		{#if item.isError}<span class="shrink-0 text-xs text-err-ink">failed</span>{/if}
		{#if item.output !== null}
			<ChevronRight class="ml-auto size-3.5 shrink-0 opacity-0 transition-[transform,opacity] group-hover/tool:opacity-60 group-data-[state=open]/tool:rotate-90 group-data-[state=open]/tool:opacity-60" />
		{/if}
	</Collapsible.Trigger>
	<Collapsible.Content>
		<pre
			class="mt-1 mb-2 ml-7.5 max-h-80 overflow-auto rounded-lg border px-3 py-2.5 font-mono text-[12px] leading-relaxed whitespace-pre-wrap {item.isError ? 'border-err/30 bg-err-soft/50 text-err-ink' : 'bg-code text-foreground/80'}"
		>{item.output || '(no output)'}{#if item.truncated}<span class="block pt-2 text-faint">Output cut to 2 KB.</span>{/if}</pre>
	</Collapsible.Content>
</Collapsible.Root>
{#if !open && item.isError && firstLine}<p class="ml-7.5 truncate text-xs text-err-ink">{firstLine}</p>{/if}
{#if item.images.length}<ToolImages images={item.images} />{/if}
