<script lang="ts" module>
	import { SvelteSet } from 'svelte/reactivity';
	/** Files marked Viewed, by scope and path; kept while the page is open. */
	const viewedFiles = new SvelteSet<string>();

	export interface DiffScope {
		id: string;
		label: string;
		hint?: string;
		set: DiffSet;
	}
</script>

<script lang="ts">
	// The review panel: every changed file, stacked, with a file tree to jump
	// around in. Slides over the thread from the right; j/k move between files.
	import { Dialog as DialogPrimitive } from 'bits-ui';
	import { tick, untrack } from 'svelte';
	import * as Select from '#lib/components/ui/select';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import X from '@lucide/svelte/icons/x';
	import Folder from '@lucide/svelte/icons/folder';
	import Check from '@lucide/svelte/icons/check';
	import Search from '@lucide/svelte/icons/search';
	import Rows from '@lucide/svelte/icons/rows-2';
	import Columns from '@lucide/svelte/icons/columns-2';
	import PanelLeftClose from '@lucide/svelte/icons/panel-left-close';
	import PanelLeftOpen from '@lucide/svelte/icons/panel-left-open';
	import FoldVertical from '@lucide/svelte/icons/fold-vertical';
	import UnfoldVertical from '@lucide/svelte/icons/unfold-vertical';
	import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
	import { goto } from '$app/navigation';
	import { prefs } from '../prefs.svelte';
	import DiffStat from './DiffStat.svelte';
	import FileDiff from './FileDiff.svelte';
	import FileStatus from './FileStatus.svelte';
	import type { DiffFile, DiffSet } from './parse';

	let {
		open = $bindable(false),
		scopes,
		scope = $bindable(''),
		focus = null
	}: { open?: boolean; scopes: DiffScope[]; scope?: string; focus?: string | null } = $props();

	const current = $derived(scopes.find((s) => s.id === scope) ?? scopes[0]);

	const view = $derived(prefs.diffView);
	let tree = $state(typeof localStorage === 'undefined' || localStorage.getItem('sf.diffTree') !== 'off');
	$effect(() => { localStorage.setItem('sf.diffTree', tree ? 'on' : 'off'); });

	// The tree, with single-child folders folded into one row ("src/lib/sf").
	type Row = { kind: 'dir'; name: string; depth: number; key: string } | { kind: 'file'; file: DiffFile; depth: number; key: string };
	type Node = { dirs: Map<string, Node>; files: DiffFile[] };
	const rows = $derived.by(() => {
		const root: Node = { dirs: new Map(), files: [] };
		for (const f of current?.set.files ?? []) {
			const parts = f.path.split('/');
			let n = root;
			for (const p of parts.slice(0, -1)) {
				if (!n.dirs.has(p)) n.dirs.set(p, { dirs: new Map(), files: [] });
				n = n.dirs.get(p)!;
			}
			n.files.push(f);
		}
		const out: Row[] = [];
		const walk = (n: Node, depth: number, prefix: string) => {
			for (const [name, child] of [...n.dirs].sort(([a], [b]) => a.localeCompare(b))) {
				let label = name, c = child, key = prefix + name + '/';
				while (!c.files.length && c.dirs.size === 1) {
					const [[k, v]] = [...c.dirs];
					label += '/' + k; key += k + '/'; c = v;
				}
				out.push({ kind: 'dir', name: label, depth, key });
				walk(c, depth + 1, key);
			}
			for (const f of [...n.files].sort((a, b) => a.path.localeCompare(b.path))) out.push({ kind: 'file', file: f, depth, key: f.path });
		};
		walk(root, 0, '');
		return out;
	});
	// Files in tree order, so the list and the tree read the same way down.
	const files = $derived(rows.flatMap((r) => (r.kind === 'file' ? [r.file] : [])));

	let filter = $state('');
	const shownRows = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return rows;
		const keep = rows.filter((r) => r.kind === 'file' && r.file.path.toLowerCase().includes(q));
		return rows.filter((r) => (r.kind === 'file' ? keep.includes(r) : keep.some((k) => k.key.startsWith(r.key))));
	});

	const vkey = (p: string) => `${current?.id}\u0000${p}`;
	const closed = new SvelteSet<string>();
	const isOpen = (p: string) => !closed.has(vkey(p)) && !viewedFiles.has(vkey(p));
	function setOpen(p: string, v: boolean) {
		if (v) { closed.delete(vkey(p)); viewedFiles.delete(vkey(p)); } else closed.add(vkey(p));
	}
	function setViewed(p: string, v: boolean) {
		if (v) viewedFiles.add(vkey(p)); else { viewedFiles.delete(vkey(p)); closed.delete(vkey(p)); }
	}
	const viewedCount = $derived(files.filter((f) => viewedFiles.has(vkey(f.path))).length);
	const allClosed = $derived(files.length > 0 && files.every((f) => !isOpen(f.path)));
	function toggleAll() {
		const close = !allClosed;
		for (const f of files) {
			if (close) closed.add(vkey(f.path));
			else { closed.delete(vkey(f.path)); }
		}
	}

	// With "start collapsed" on, each set of changes opens folded the first time (bar the file asked for).
	const folded = new Set<string>();
	$effect(() => {
		if (!open || !prefs.diffCollapsed || !current || files.length < 2) return;
		const id = current.id, paths = files.map((f) => f.path), target = focus;
		untrack(() => {
			if (folded.has(id)) return;
			folded.add(id);
			for (const p of paths) if (p !== target) closed.add(`${id}\u0000${p}`);
		});
	});

	let scroller = $state<HTMLElement>();
	let active = $state<string | null>(null);
	async function jump(path: string) {
		if (!isOpen(path) && !viewedFiles.has(vkey(path))) setOpen(path, true);
		active = path;
		await tick();
		scroller?.querySelector(`[data-path="${CSS.escape(path)}"]`)?.scrollIntoView({ block: 'start' });
	}
	// Opening on a file scrolls straight to it.
	$effect(() => {
		if (!open || !scroller) return;
		const target = focus;
		void current?.id;
		const el = scroller;
		untrack(() => {
			if (target) void jump(target);
			else el.scrollTop = 0;
		});
	});

	let frame = 0;
	function onscroll() {
		cancelAnimationFrame(frame);
		frame = requestAnimationFrame(() => {
			if (!scroller) return;
			const top = scroller.getBoundingClientRect().top + 48;
			let at: string | null = files[0]?.path ?? null;
			for (const el of scroller.querySelectorAll<HTMLElement>('[data-path]')) {
				if (el.getBoundingClientRect().top <= top) at = el.dataset.path!;
				else break;
			}
			active = at;
		});
	}

	function onkeydown(e: KeyboardEvent) {
		const t = e.target as HTMLElement;
		if (e.metaKey || e.ctrlKey || e.altKey || t.closest('input, textarea, [contenteditable]')) return;
		const i = files.findIndex((f) => f.path === active);
		if (e.key === 'j' || e.key === 'k') {
			e.preventDefault();
			const next = files[Math.max(0, Math.min(files.length - 1, i + (e.key === 'j' ? 1 : -1)))];
			if (next) void jump(next.path);
		} else if (e.key === 'v' && active) {
			setViewed(active, !viewedFiles.has(vkey(active)));
		} else if (e.key === 'w') {
			prefs.set({ diffHideWhitespace: !prefs.diffHideWhitespace });
		}
	}
</script>

<DialogPrimitive.Root bind:open>
	<DialogPrimitive.Portal>
		<DialogPrimitive.Overlay class="fixed inset-0 z-50 bg-black/25 duration-200 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0 dark:bg-black/50" />
		<DialogPrimitive.Content
			class="fixed inset-y-0 right-0 z-50 flex w-full flex-col border-l bg-background shadow-2xl outline-none duration-300 ease-out data-open:animate-in data-open:slide-in-from-right-8 data-open:fade-in-0 data-closed:animate-out data-closed:slide-out-to-right-8 data-closed:fade-out-0 lg:w-[min(1320px,calc(100vw-4rem))] lg:rounded-l-2xl"
			{onkeydown}
		>
			<header class="flex h-14 shrink-0 items-center gap-3 border-b px-3 sm:px-4">
				<button
					type="button"
					class="hidden size-8 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground lg:grid"
					aria-label={tree ? 'Hide file tree' : 'Show file tree'}
					aria-pressed={tree}
					onclick={() => (tree = !tree)}
				>
					{#if tree}<PanelLeftClose class="size-4" />{:else}<PanelLeftOpen class="size-4" />{/if}
				</button>
				<DialogPrimitive.Title class="text-[15px] font-semibold tracking-tight">Changes</DialogPrimitive.Title>
				{#if scopes.length > 1}
					<Select.Root type="single" value={current?.id} onValueChange={(v) => (scope = v)}>
						<Select.Trigger size="sm" class="h-8 max-w-56 rounded-lg text-[13px]" aria-label="Which changes">
							<span class="truncate">{current?.label}</span>
						</Select.Trigger>
						<Select.Content>
							{#each scopes as s (s.id)}
								<Select.Item value={s.id} label={s.label}>
									<span class="flex min-w-0 flex-col">
										<span>{s.label} <span class="tnum text-xs text-muted-foreground">· {s.set.file_count} file{s.set.file_count === 1 ? '' : 's'}</span></span>
										{#if s.hint}<span class="max-w-72 truncate text-xs text-muted-foreground">{s.hint}</span>{/if}
									</span>
								</Select.Item>
							{/each}
						</Select.Content>
					</Select.Root>
				{/if}
				{#if current}
					<span class="hidden items-center gap-2.5 text-[13px] text-muted-foreground sm:flex">
						<span class="tnum">{current.set.file_count.toLocaleString()} file{current.set.file_count === 1 ? '' : 's'}</span>
						<DiffStat add={current.set.additions} del={current.set.deletions} />
					</span>
				{/if}

				<span class="ml-auto flex items-center gap-1.5">
					<button
						type="button"
						class="hidden size-8 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground sm:grid"
						aria-label={allClosed ? 'Expand all files' : 'Collapse all files'}
						title={allClosed ? 'Expand all' : 'Collapse all'}
						onclick={toggleAll}
					>
						{#if allClosed}<UnfoldVertical class="size-4" />{:else}<FoldVertical class="size-4" />{/if}
					</button>
					<div class="hidden rounded-lg bg-muted p-0.5 md:flex" role="radiogroup" aria-label="Layout">
						{#each [{ id: 'unified', label: 'Unified', icon: Rows }, { id: 'split', label: 'Split', icon: Columns }] as o (o.id)}
							<button
								type="button"
								role="radio"
								aria-checked={view === o.id}
								class="flex h-7 items-center gap-1.5 rounded-md px-2.5 text-[12.5px] transition-colors {view === o.id ? 'bg-card font-medium text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}"
								onclick={() => prefs.set({ diffView: o.id as typeof view })}
							>
								<o.icon class="size-3.5" />{o.label}
							</button>
						{/each}
					</div>
					<DropdownMenu.Root>
						<DropdownMenu.Trigger
							class="relative grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground data-[state=open]:bg-muted data-[state=open]:text-foreground"
							aria-label="View options"
							title="View options"
						>
							<SlidersHorizontal class="size-4" />
							{#if prefs.diffHideWhitespace}<span class="absolute top-1.5 right-1.5 size-1.5 rounded-full bg-primary" aria-hidden="true"></span>{/if}
						</DropdownMenu.Trigger>
						<DropdownMenu.Content align="end" class="w-64">
							<DropdownMenu.CheckboxItem checked={prefs.diffHideWhitespace} onCheckedChange={(v) => prefs.set({ diffHideWhitespace: v })}>
								<span class="flex-1">Hide whitespace changes</span><kbd class="rounded border bg-card px-1 font-mono text-[10.5px] text-muted-foreground pointer-coarse:hidden">w</kbd>
							</DropdownMenu.CheckboxItem>
							<DropdownMenu.CheckboxItem checked={prefs.diffCollapsed} onCheckedChange={(v) => prefs.set({ diffCollapsed: v })}>
								Open with files collapsed
							</DropdownMenu.CheckboxItem>
							<DropdownMenu.Separator />
							<DropdownMenu.Item onclick={() => { open = false; void goto('#/settings'); }}>All settings…</DropdownMenu.Item>
						</DropdownMenu.Content>
					</DropdownMenu.Root>
					<DialogPrimitive.Close class="grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground" aria-label="Close">
						<X class="size-4" />
					</DialogPrimitive.Close>
				</span>
			</header>

			<div class="flex min-h-0 flex-1">
				{#if tree}
					<aside class="hidden w-72 shrink-0 flex-col border-r lg:flex">
						<div class="p-2.5">
							<label class="flex h-8 items-center gap-2 rounded-lg border bg-card px-2.5 text-muted-foreground focus-within:border-ring/40">
								<Search class="size-3.5 shrink-0" />
								<input bind:value={filter} placeholder="Filter files" class="min-w-0 flex-1 bg-transparent text-[12.5px] text-foreground outline-none placeholder:text-faint" />
							</label>
						</div>
						<nav class="min-h-0 flex-1 overflow-y-auto px-1.5 pb-3" aria-label="Changed files">
							{#each shownRows as r (r.key)}
								{#if r.kind === 'dir'}
									<div class="flex h-7 items-center gap-1.5 truncate pr-2 text-[12.5px] text-muted-foreground" style="padding-left: {r.depth * 14 + 8}px">
										<Folder class="size-3.5 shrink-0 opacity-70" /><span class="truncate">{r.name}</span>
									</div>
								{:else}
									{@const seen = viewedFiles.has(vkey(r.file.path))}
									<button
										type="button"
										class="flex h-7 w-full items-center gap-1.5 rounded-md pr-2 text-left text-[12.5px] transition-colors {active === r.file.path ? 'bg-accent text-foreground' : 'text-foreground/85 hover:bg-muted'}"
										style="padding-left: {r.depth * 14 + 8}px"
										aria-current={active === r.file.path ? 'true' : undefined}
										onclick={() => jump(r.file.path)}
									>
										<FileStatus status={r.file.status} />
										<span class="min-w-0 flex-1 truncate {seen ? 'text-muted-foreground' : ''}">{r.file.path.slice(r.file.path.lastIndexOf('/') + 1)}</span>
										{#if seen}<Check class="size-3.5 shrink-0 text-ok-ink" aria-label="Viewed" />{:else}<DiffStat add={r.file.additions} del={r.file.deletions} blocks={false} class="shrink-0 text-[10.5px] opacity-80" />{/if}
									</button>
								{/if}
							{:else}
								<p class="px-2 py-4 text-center text-xs text-muted-foreground">No files match.</p>
							{/each}
						</nav>
						<div class="flex items-center gap-3 border-t px-3.5 py-2.5 text-[11.5px] text-muted-foreground">
							<span class="tnum">{viewedCount} of {files.length} viewed</span>
							<span class="ml-auto flex items-center gap-1"><kbd class="rounded border bg-card px-1 font-mono text-[10.5px]">j</kbd><kbd class="rounded border bg-card px-1 font-mono text-[10.5px]">k</kbd> files</span>
							<span class="flex items-center gap-1"><kbd class="rounded border bg-card px-1 font-mono text-[10.5px]">v</kbd> viewed</span>
						</div>
					</aside>
				{/if}

				<div bind:this={scroller} class="min-w-0 flex-1 overflow-y-auto" {onscroll}>
					<div class="flex flex-col gap-3 p-3 sm:p-4">
						{#each files as f (vkey(f.path))}
							<FileDiff
								file={f}
								{view}
								bind:open={() => isOpen(f.path), (v) => setOpen(f.path, v)}
								viewed={viewedFiles.has(vkey(f.path))}
								onviewed={(v) => setViewed(f.path, v)}
								active={active === f.path && files.length > 1}
							/>
						{/each}
						{#if current && current.set.file_count > current.set.files.length}
							<p class="py-2 text-center text-xs text-muted-foreground">
								{(current.set.file_count - current.set.files.length).toLocaleString()} more changed files are not listed. See the branch for the full change.
							</p>
						{/if}
						<div class="h-[40vh]" aria-hidden="true"></div>
					</div>
				</div>
			</div>
		</DialogPrimitive.Content>
	</DialogPrimitive.Portal>
</DialogPrimitive.Root>
