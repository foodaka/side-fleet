<script lang="ts">
	// A run of consecutive tool calls folded into one line ("Ran 2 commands, read a file"),
	// like Claude's own app. Opens to the individual rows; images stay visible while folded.
	import * as Collapsible from '#lib/components/ui/collapsible';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import { Loader } from '#lib/components/ai-elements/loader';
	import type { Item } from '../thread.svelte';
	import ToolRow from './ToolRow.svelte';
	import ToolImages from './ToolImages.svelte';

	type Tool = Extract<Item, { kind: 'tool' }>;
	let { tools, live }: { tools: Tool[]; live: boolean } = $props();

	const kinds: [RegExp, string, string, string][] = [
		[/^Bash$/, 'ran', 'a command', 'commands'],
		[/^Read$/, 'read', 'a file', 'files'],
		[/^(Edit|Write|MultiEdit|NotebookEdit)$/, 'edited', 'a file', 'files'],
		[/^(Grep|Glob)$/, 'searched', 'once', 'times'],
		[/^Web/, 'browsed', 'a page', 'pages'],
		[/^TodoWrite$/, 'updated', 'the plan', 'the plan'],
		[/^(Agent|Task)$/, 'delegated', 'a task', 'tasks']
	];
	const summary = $derived.by(() => {
		const counts = new Map<number, number>();
		for (const t of tools) {
			const i = kinds.findIndex(([re]) => re.test(t.name));
			const k = i < 0 ? kinds.length : i;
			counts.set(k, (counts.get(k) ?? 0) + 1);
		}
		const parts = [...counts].sort((a, b) => a[0] - b[0]).map(([k, n]) => {
			if (k === kinds.length) return `used ${n === 1 ? 'a tool' : `${n} tools`}`;
			const [, verb, one, many] = kinds[k];
			return `${verb} ${n === 1 || many === 'the plan' ? one : `${n} ${many}`}`;
		});
		const s = parts.join(', ');
		return s.charAt(0).toUpperCase() + s.slice(1);
	});
	const failed = $derived(tools.filter((t) => t.isError).length);
	const pending = $derived(live && tools.some((t) => t.output === null));
	const images = $derived(tools.flatMap((t) => t.images));
	let open = $state(false);
</script>

<Collapsible.Root bind:open class="group/tools">
	<Collapsible.Trigger class="flex w-full items-center gap-1.5 rounded-md py-1 text-left text-[13px] text-muted-foreground transition-colors hover:text-foreground">
		{#if pending}<Loader size={12} />{/if}
		<span class="min-w-0 truncate">{summary}</span>
		{#if failed}<span class="shrink-0 text-xs text-err-ink">{failed} failed</span>{/if}
		<ChevronRight class="size-3.5 shrink-0 opacity-60 transition-transform group-data-[state=open]/tools:rotate-90" />
	</Collapsible.Trigger>
	<Collapsible.Content>
		<div class="mt-0.5 mb-1 flex flex-col border-l pl-3">
			{#each tools as t (t.key)}<ToolRow item={t} {live} />{/each}
		</div>
	</Collapsible.Content>
</Collapsible.Root>
{#if !open && images.length}<ToolImages {images} inset={false} />{/if}
