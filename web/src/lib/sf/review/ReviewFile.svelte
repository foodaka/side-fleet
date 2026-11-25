<script lang="ts">
	// One changed file on the review page: its hunks, unified, where tapping a line opens a
	// note on it. Notes show under their line until they're sent.
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import MessageSquarePlus from '@lucide/svelte/icons/message-square-plus';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import X from '@lucide/svelte/icons/x';
	import { Button } from '#lib/components/ui/button';
	import DiffStat from '../diff/DiffStat.svelte';
	import { omittedReason, parsePatch, type DiffFile, type Line } from '../diff/parse';
	import { noteKey, type Note } from './notes';

	let {
		file,
		notes,
		flag,
		flash = false,
		readonly = false,
		onadd,
		ondrop
	}: {
		file: DiffFile;
		/** Your unsent notes on this file, with their index in the task's list. */
		notes: { note: Note; index: number }[];
		/** The flag raised about this file, if any. */
		flag?: string;
		flash?: boolean;
		readonly?: boolean;
		onadd: (n: Note) => void;
		ondrop: (index: number) => void;
	} = $props();

	let open = $state(true);
	const parsed = $derived(file.patch && open ? parsePatch(file.patch) : null);
	const side = (l: Line): 'old' | 'new' => (l.kind === 'del' ? 'old' : 'new');
	const num = (l: Line) => (l.kind === 'del' ? l.old : l.new) ?? 0;
	const keyOf = (l: Line) => noteKey({ file: file.path, line: num(l), side: side(l) });

	let editing = $state<string | null>(null);
	let text = $state('');
	const focus = (el: HTMLElement) => el.focus({ preventScroll: true });
	function start(l: Line) {
		if (readonly) return;
		editing = keyOf(l);
		text = '';
	}
	function save(l: Line) {
		if (!text.trim()) return;
		onadd({ file: file.path, line: num(l), side: side(l), snippet: l.text, text: text.trim() });
		editing = null;
		text = '';
	}
</script>

<div id="file-{file.path}" class="scroll-mt-24 overflow-hidden rounded-2xl border bg-card transition-shadow {flash ? 'ring-2 ring-queue ring-offset-2 ring-offset-background' : ''} {flag ? 'border-queue/50' : ''}">
	<button type="button" class="flex w-full items-center gap-2 px-3 py-2 text-left text-[12.5px] {open ? 'border-b' : ''}" aria-expanded={open} onclick={() => (open = !open)}>
		<ChevronRight class="size-3.5 shrink-0 text-muted-foreground transition-transform {open ? 'rotate-90' : ''}" />
		<span class="min-w-0 flex-1 truncate font-mono">{file.old_path && file.old_path !== file.path ? `${file.old_path} → ` : ''}{file.path}</span>
		{#if flag}<span class="hidden shrink-0 items-center gap-1 rounded-full bg-queue-soft px-1.5 text-[10.5px] font-medium text-queue-ink sm:inline-flex"><TriangleAlert class="size-3" />{flag}</span>{/if}
		{#if file.status !== 'modified'}<span class="shrink-0 text-[11px] text-muted-foreground">{file.status}</span>{/if}
		<DiffStat add={file.additions} del={file.deletions} />
	</button>
	{#if open}
		{#if file.omitted || !file.patch}
			<p class="px-4 py-3 text-[12.5px] text-muted-foreground">{file.omitted ? omittedReason[file.omitted] : file.status === 'renamed' ? 'File renamed without changes.' : 'No content changes.'}</p>
		{:else if parsed}
			{#each parsed.hunks as h, hi (hi)}
				{#if h.section || hi > 0}<p class="truncate bg-muted/50 px-3 py-1 font-mono text-[11.5px] text-muted-foreground">{h.gap ? `${h.gap} unchanged lines · ` : ''}{h.section}</p>{/if}
				{#each h.lines as l, li (li)}
					{@const k = keyOf(l)}
					<button type="button" disabled={readonly} onclick={() => start(l)} aria-label="Add a note on line {num(l)}"
						class="group/line flex w-full text-left font-mono text-[12px] leading-[1.7] enabled:hover:bg-muted/70 {l.kind === 'add' ? 'bg-ok-soft/60' : l.kind === 'del' ? 'bg-err-soft/60' : ''}">
						<span class="tnum w-11 shrink-0 pr-2 text-right text-faint select-none">{num(l)}</span>
						<span class="w-4 shrink-0 select-none {l.kind === 'add' ? 'text-ok-ink' : l.kind === 'del' ? 'text-err-ink' : 'text-faint'}">{l.kind === 'add' ? '+' : l.kind === 'del' ? '−' : ''}</span>
						<span class="min-w-0 flex-1 pr-2 break-all whitespace-pre-wrap">{l.text}</span>
						{#if !readonly}<MessageSquarePlus class="mr-2 size-3.5 shrink-0 self-center text-muted-foreground opacity-0 group-hover/line:opacity-100 max-md:hidden" />{/if}
					</button>
					{#each notes.filter((n) => noteKey(n.note) === k) as n (n.index)}
						<div class="flex items-start gap-2 border-y border-run/25 bg-run-soft/40 px-3 py-2 text-[13px]">
							<span class="mt-0.5 shrink-0 rounded-full bg-run-soft px-1.5 text-[10.5px] font-medium text-run-ink">Your note</span>
							<p class="min-w-0 flex-1 whitespace-pre-wrap">{n.note.text}</p>
							<button type="button" aria-label="Delete note" class="grid size-6 shrink-0 place-items-center rounded-full text-muted-foreground hover:bg-muted" onclick={() => ondrop(n.index)}><X class="size-3.5" /></button>
						</div>
					{/each}
					{#if editing === k}
						<form class="flex flex-col gap-2 border-y bg-card px-3 py-2.5" onsubmit={(e) => { e.preventDefault(); save(l); }}>
							<label for="note-{k}" class="sr-only">Note on {file.path}:{num(l)}</label>
							<textarea id="note-{k}" use:focus bind:value={text} rows="2" placeholder="What should the agent do here?"
								onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); save(l); } if (e.key === 'Escape') editing = null; }}
								class="w-full resize-none rounded-lg border border-input bg-background px-3 py-2 text-[13px] outline-none focus:border-ring"></textarea>
							<div class="flex items-center justify-end gap-2">
								<span class="mr-auto text-xs text-faint">Kept here and sent with your other notes. Enter adds it.</span>
								<Button variant="ghost" size="sm" class="h-8 rounded-full" onclick={() => (editing = null)}>Cancel</Button>
								<Button type="submit" size="sm" class="h-8 rounded-full" disabled={!text.trim()}>Add note</Button>
							</div>
						</form>
					{/if}
				{/each}
			{/each}
		{/if}
	{/if}
</div>
