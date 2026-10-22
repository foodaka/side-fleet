<script lang="ts">
	// What a turn changed, at the end of its narration: the files with their
	// stats, each one a way into the full diff.
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import FileDiffIcon from '@lucide/svelte/icons/file-diff';
	import DiffStat from './DiffStat.svelte';
	import FileStatus from './FileStatus.svelte';
	import { splitPath, type DiffSet } from './parse';

	let { set, onopen }: { set: DiffSet; onopen: (path: string | null) => void } = $props();
	const SHOWN = 6;
	let all = $state(false);
	const files = $derived(all ? set.files : set.files.slice(0, SHOWN));
	const hidden = $derived(set.files.length - files.length);
</script>

<div class="mt-2 overflow-hidden rounded-xl border bg-card">
	<button
		type="button"
		class="group/cc flex w-full items-center gap-2.5 px-3.5 py-2.5 text-left transition-colors hover:bg-muted/50"
		onclick={() => onopen(null)}
	>
		<FileDiffIcon class="size-4 shrink-0 text-muted-foreground" />
		<span class="text-[13px] font-medium">{set.file_count.toLocaleString()} file{set.file_count === 1 ? '' : 's'} changed</span>
		<DiffStat add={set.additions} del={set.deletions} />
		<span class="ml-auto flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground transition-colors group-hover/cc:text-foreground">
			Review<ChevronRight class="size-3.5 transition-transform group-hover/cc:translate-x-0.5" />
		</span>
	</button>
	<ul class="border-t py-1">
		{#each files as f (f.path)}
			{@const [dir, name] = splitPath(f.path)}
			<li>
				<button type="button" class="flex w-full items-center gap-2.5 px-3.5 py-[5px] text-left transition-colors hover:bg-muted/60" onclick={() => onopen(f.path)}>
					<FileStatus status={f.status} />
					<span class="min-w-0 truncate font-mono text-[12px]"><span class="text-muted-foreground">{dir}</span><span class="text-foreground">{name}</span></span>
					<DiffStat add={f.additions} del={f.deletions} blocks={false} class="ml-auto shrink-0 text-[11px]" />
				</button>
			</li>
		{/each}
		{#if hidden > 0 || set.file_count > set.files.length}
			<li>
				<button type="button" class="w-full px-3.5 py-[5px] text-left text-xs text-muted-foreground hover:text-foreground" onclick={() => (hidden > 0 ? (all = true) : onopen(null))}>
					{hidden > 0 ? `Show ${hidden} more file${hidden === 1 ? '' : 's'}` : `${(set.file_count - set.files.length).toLocaleString()} more files not listed`}
				</button>
			</li>
		{/if}
	</ul>
</div>
