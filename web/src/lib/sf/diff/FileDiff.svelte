<script lang="ts">
	// One file of a diff: a sticky header (status, path, stats, Viewed) over its
	// hunks, unified or side by side. The body renders and highlights once it
	// nears the viewport, so a hundred-file change opens instantly.
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import UnfoldVertical from '@lucide/svelte/icons/unfold-vertical';
	import CopyButton from '../ui/CopyButton.svelte';
	import DiffStat from './DiffStat.svelte';
	import FileStatus from './FileStatus.svelte';
	import { highlight, segments, type Tok } from './highlight';
	import { changes, omittedReason, parsePatch, splitPath, splitRows, type DiffFile, type Line } from './parse';
	import { prefs } from '../prefs.svelte';

	let {
		file,
		view,
		open = $bindable(true),
		viewed = false,
		onviewed,
		active = false
	}: {
		file: DiffFile;
		view: 'unified' | 'split';
		open?: boolean;
		viewed?: boolean;
		onviewed?: (v: boolean) => void;
		active?: boolean;
	} = $props();

	/** Past this many changed lines a file waits for "Load diff", like GitHub. */
	const LARGE = 800;
	let forced = $state(false);
	const large = $derived(file.additions + file.deletions > LARGE && !forced);
	const parsed = $derived(file.patch && open && !large ? parsePatch(file.patch, { hideWhitespace: prefs.diffHideWhitespace }) : null);
	const [dir, name] = $derived(splitPath(file.path));
	const lastLine = $derived(parsed?.hunks.reduce((m, h) => Math.max(m, ...h.lines.map((l) => Math.max(l.old ?? 0, l.new ?? 0))), 1) ?? 1);
	const numW = $derived(`${String(lastLine).length + 2.5}ch`);

	let near = $state(false);
	let toks = $state<Tok[][][] | null>(null);
	$effect(() => {
		const p = parsed;
		if (!p || !near) return;
		let live = true;
		highlight(file.path, [p.oldText, p.newText]).then((t) => { if (live) toks = t; }, () => {});
		return () => { live = false; };
	});
	function nearViewport(node: HTMLElement) {
		const io = new IntersectionObserver(([e]) => { if (e.isIntersecting) { near = true; io.disconnect(); } }, { rootMargin: '800px 0px' });
		io.observe(node);
		return () => io.disconnect();
	}
	const lineToks = (l: Line, side: 'old' | 'new') =>
		side === 'old' ? (l.oi != null ? toks?.[0]?.[l.oi] : undefined) : (l.ni != null ? toks?.[1]?.[l.ni] : undefined);
	const sideOf = (l: Line): 'old' | 'new' => (l.kind === 'del' ? 'old' : 'new');
	const sign = (l: Line) => (l.kind === 'add' ? '+' : l.kind === 'del' ? '−' : '');
	// A new or deleted file has one side only: split would leave half the width empty.
	const layout = $derived(file.status === 'added' || file.status === 'deleted' ? 'unified' : view);
	const estimate = $derived((file.additions + file.deletions) * 1.4 * 20 + 40);
</script>

{#snippet code(l: Line, side: 'old' | 'new')}
	{#each segments(l.text, lineToks(l, side), l.marks) as s, i (i)}{#if s.mark}<mark style={s.style}>{s.content}</mark>{:else if s.style}<span style={s.style}>{s.content}</span>{:else}{s.content}{/if}{/each}{#if !l.text}{'\n'}{/if}
{/snippet}

{#snippet hunkRow(h: (typeof parsed & {})['hunks'][number], cols: number)}
	<!-- A hunk at the very top with no heading has nothing to say. -->
	{#if h.gap || h.section}<tr class="hunk">
		<td colspan={cols} class="px-3">
			<span class="flex items-center gap-2 truncate">
				<UnfoldVertical class="size-3.5 shrink-0 opacity-50" />
				{#if h.gap}<span class="shrink-0 font-sans text-[11.5px]">{h.gap.toLocaleString()} unchanged line{h.gap === 1 ? '' : 's'}</span>{/if}
				{#if h.section}<span class="truncate text-foreground/65">{h.section}</span>{/if}
			</span>
		</td>
	</tr>{/if}
{/snippet}

<section
	id={'diff-' + file.path}
	data-path={file.path}
	class="scroll-mt-3 overflow-clip rounded-xl border bg-card transition-shadow {active ? 'shadow-[0_0_0_1px_color-mix(in_oklab,var(--foreground)_18%,transparent)]' : ''}"
	{@attach nearViewport}
>
	<header class="group/fh sticky top-0 z-10 flex h-10 items-center gap-2 bg-card/95 pr-2 pl-1.5 backdrop-blur supports-[backdrop-filter]:bg-card/85 {open ? 'border-b' : ''}">
		<button
			type="button"
			class="grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
			aria-label={open ? 'Collapse file' : 'Expand file'}
			aria-expanded={open}
			onclick={() => (open = !open)}
		>
			<ChevronRight class="size-4 transition-transform duration-150 {open ? 'rotate-90' : ''}" />
		</button>
		<FileStatus status={file.status} />
		<button type="button" class="flex min-w-0 items-baseline gap-1.5 text-left font-mono text-[12.5px]" onclick={() => (open = !open)}>
			{#if file.old_path}
				<span class="truncate text-muted-foreground line-through decoration-foreground/25">{file.old_path}</span>
				<span class="shrink-0 text-faint">→</span>
			{/if}
			<span class="truncate"><span class="text-muted-foreground">{dir}</span><span class="font-semibold text-foreground">{name}</span></span>
		</button>
		<CopyButton text={() => file.path} label="" class="h-6 shrink-0 bg-transparent px-1 text-muted-foreground opacity-0 group-hover/fh:opacity-100 hover:bg-muted hover:text-foreground focus-visible:opacity-100" />
		<span class="ml-auto flex shrink-0 items-center gap-3">
			<DiffStat add={file.additions} del={file.deletions} class="max-sm:[&>span:last-child]:hidden" />
			{#if onviewed}
				<label class="flex cursor-pointer items-center gap-1.5 rounded-md border px-2 py-0.5 text-[11.5px] select-none {viewed ? 'border-transparent bg-muted text-muted-foreground' : 'text-foreground/80 hover:bg-muted'}">
					<input type="checkbox" class="size-3 accent-foreground" checked={viewed} onchange={(e) => onviewed?.(e.currentTarget.checked)} />
					Viewed
				</label>
			{/if}
		</span>
	</header>

	{#if open}
		{#if !file.patch}
			<p class="px-4 py-6 text-center text-[13px] text-muted-foreground">
				{file.omitted ? omittedReason[file.omitted] : file.status === 'renamed' ? 'File renamed without changes.' : 'No content changes.'}
			</p>
		{:else if large}
			<div class="flex flex-col items-center gap-2 px-4 py-7 text-center">
				<p class="text-[13px] text-muted-foreground">Large diffs are not shown by default. This one changes {(file.additions + file.deletions).toLocaleString()} lines.</p>
				<button type="button" class="rounded-md border bg-background px-3 py-1 text-[12.5px] font-medium hover:bg-muted" onclick={() => (forced = true)}>Load diff</button>
			</div>
		{:else if parsed && !changes(parsed)}
			<div class="flex flex-col items-center gap-2 px-4 py-6 text-center">
				<p class="text-[13px] text-muted-foreground">Only whitespace changed in this file.</p>
				<button type="button" class="rounded-md border bg-background px-3 py-1 text-[12.5px] font-medium hover:bg-muted" onclick={() => prefs.set({ diffHideWhitespace: false })}>Show whitespace</button>
			</div>
		{:else if parsed && !near}
			<div style="height: {estimate}px"></div>
		{:else if parsed && layout === 'unified'}
			<table class="sf-diff">
				<colgroup><col style="width: {numW}" /><col style="width: {numW}" /><col style="width: 20px" /><col /></colgroup>
				<tbody>
					{#each parsed.hunks as h, hi (hi)}
						{@render hunkRow(h, 4)}
						{#each h.lines as l, li (li)}
							<tr>
								<td class="num {l.kind}">{l.old ?? ''}</td>
								<td class="num {l.kind}">{l.new ?? ''}</td>
								<td class="sign {l.kind}">{sign(l)}</td>
								<td class="code {l.kind}">{@render code(l, sideOf(l))}</td>
							</tr>
						{/each}
					{/each}
				</tbody>
			</table>
		{:else if parsed}
			<table class="sf-diff">
				<colgroup><col style="width: {numW}" /><col style="width: 18px" /><col /><col style="width: {numW}" /><col style="width: 18px" /><col /></colgroup>
				<tbody>
					{#each parsed.hunks as h, hi (hi)}
						{@render hunkRow(h, 6)}
						{#each splitRows(h.lines) as r, ri (ri)}
							<tr>
								{#if r.left}
									<td class="num {r.left.kind}">{r.left.old}</td>
									<td class="sign {r.left.kind}">{sign(r.left)}</td>
									<td class="code split-l {r.left.kind}">{@render code(r.left, r.left.kind === 'ctx' ? 'new' : 'old')}</td>
								{:else}
									<td class="empty"></td><td class="empty"></td><td class="empty split-l"></td>
								{/if}
								{#if r.right}
									<td class="num {r.right.kind}">{r.right.new}</td>
									<td class="sign {r.right.kind}">{sign(r.right)}</td>
									<td class="code {r.right.kind}">{@render code(r.right, 'new')}</td>
								{:else}
									<td class="empty"></td><td class="empty"></td><td class="empty"></td>
								{/if}
							</tr>
						{/each}
					{/each}
				</tbody>
			</table>
		{/if}
	{/if}
</section>
