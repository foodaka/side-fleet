<script lang="ts">
	import * as Tooltip from '#lib/components/ui/tooltip';
	import { Kbd } from '#lib/components/ui/kbd';
	import Boxes from '@lucide/svelte/icons/boxes';
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import ClipboardCheck from '@lucide/svelte/icons/clipboard-check';
	import CircleArrowUp from '@lucide/svelte/icons/circle-arrow-up';
	import { LayoutGrid, PanelLeftClose, Plus, Search, ServerCog, X } from '#lib/components/movingicons';
	import Logo from './Logo.svelte';
	import StatusDot from './StatusDot.svelte';
	import RunList from './RunList.svelte';
	import AccountMenu from './AccountMenu.svelte';
	import InboxBell from './InboxBell.svelte';
	import { fleet } from '../state.svelte';
	import { review } from '../review/desk.svelte';

	let { path, onClose, drawer = false, closeKeys = '' }: {
		path: string;
		onClose: () => void;
		/** Shown as the phone drawer rather than docked beside the page. */
		drawer?: boolean;
		/** Shortcut shown in the close button's tooltip. */
		closeKeys?: string;
	} = $props();

	const closeLabel = $derived(drawer ? 'Close menu' : 'Hide sidebar');

	let query = $state('');
	let search = $state<HTMLInputElement | null>(null);

	const activeId = $derived(/^\/runs\/([^/?#]+)/.exec(path)?.[1] ?? null);
	const connLabel = $derived(
		fleet.conn === 'unreachable' ? 'Control plane unreachable'
		: !fleet.loaded ? 'Connecting…'
		: `${fleet.online.length} of ${fleet.workers.length} machine${fleet.workers.length === 1 ? '' : 's'} online`
			+ (fleet.updatable.length ? `, ${fleet.updatable.length} can update` : '')
	);
	// Places you go often sit above the tasks; setup (team, chat apps, settings) lives in the account menu below.
	const nav = $derived([
		...(fleet.reviewDesk ? [{ href: '#/review', label: 'Review', icon: ClipboardCheck, on: (p: string) => p.startsWith('/review') }] : []),
		{ href: '#/runs', label: 'Overview', icon: LayoutGrid, on: (p: string) => p === '/runs' },
		{ href: '#/pulls', label: 'Pull requests', icon: GitPullRequest, on: (p: string) => p.startsWith('/pulls') },
		{ href: '#/rooms', label: 'Rooms', icon: Boxes, on: (p: string) => p.startsWith('/rooms') },
		{ href: '#/machines', label: 'Machines', icon: ServerCog, on: (p: string) => p.startsWith('/machines') }
	]);

	function onkeydown(e: KeyboardEvent) {
		// `/` jumps to search, unless you're typing somewhere or this copy of the sidebar is hidden.
		if (e.key !== '/' || e.metaKey || e.ctrlKey || e.altKey || !search?.offsetParent) return;
		const t = e.target as HTMLElement;
		if (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return;
		e.preventDefault();
		search.focus();
	}
	const item = (on: boolean) =>
		`mi-hover flex h-8 items-center gap-2.5 rounded-md px-2.5 text-[13px] transition-colors ${on
			? 'bg-sidebar-accent font-medium text-foreground'
			: 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground'}`;
</script>

<svelte:window {onkeydown} />

<div class="flex h-full flex-col">
	<div class="flex items-center gap-1 pt-3.5 pr-2.5 pb-3 pl-4">
		<a href="#/" class="flex items-center gap-2 text-[15px] font-semibold tracking-tight"><Logo />Sidefleet</a>
		<span class="ml-auto"></span>
		<Tooltip.Root>
			<Tooltip.Trigger
				class="mi-hover grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground"
				aria-label={closeLabel}
				onclick={onClose}
			>
				{#if drawer}<X size={14} />{:else}<PanelLeftClose size={14} />{/if}
			</Tooltip.Trigger>
			<Tooltip.Content side="right">{closeLabel}{#if closeKeys}<Kbd class="ml-1.5">{closeKeys}</Kbd>{/if}</Tooltip.Content>
		</Tooltip.Root>
	</div>

	<div class="flex flex-col gap-2 px-3">
		<a
			href="#/"
			aria-current={path === '/' ? 'page' : undefined}
			class="mi-hover flex h-9 items-center gap-2 rounded-lg border bg-card px-3 text-[13px] font-medium shadow-xs transition-colors hover:bg-accent/60 aria-[current=page]:border-foreground/15"
		>
			<Plus size={14} />New task
		</a>
		<label class="mi-hover relative flex h-8 items-center rounded-md text-muted-foreground focus-within:text-foreground">
			<Search size={12} class="pointer-events-none absolute left-2.5" />
			<input
				bind:this={search}
				bind:value={query}
				type="search"
				placeholder="Search tasks"
				aria-label="Search tasks"
				autocomplete="off"
				spellcheck="false"
				class="peer h-full w-full rounded-md bg-transparent pr-8 pl-8 text-[13px] text-foreground outline-none placeholder:text-muted-foreground hover:bg-sidebar-accent/60 focus:bg-sidebar-accent [&::-webkit-search-cancel-button]:hidden"
				onkeydown={(e) => { if (e.key === 'Escape') { query = ''; search?.blur(); } }}
			/>
			{#if query}
				<button type="button" class="mi-hover absolute right-1.5 grid size-5 place-items-center rounded text-muted-foreground hover:text-foreground" aria-label="Clear search" onclick={() => { query = ''; search?.focus(); }}>
					<X size={12} />
				</button>
			{:else}
				<Kbd class="absolute right-2 bg-transparent text-faint peer-focus:hidden pointer-coarse:hidden">/</Kbd>
			{/if}
		</label>
	</div>

	<nav class="mt-1 flex flex-col gap-0.5 px-3" aria-label="Main">
		{#if !drawer}<InboxBell row side="right" />{/if}
		{#each nav as n (n.href)}
			<a href={n.href} aria-current={n.on(path) ? 'page' : undefined} class={item(n.on(path))}>
				<n.icon size={14} />
				<span class="flex-1">{n.label}</span>
				{#if n.href === '#/review' && review.count}
					<span class="tnum rounded-full bg-queue-soft px-1.5 text-[11px] font-medium text-queue-ink" aria-label="{review.count} to review">{review.count}</span>
				{/if}
				{#if n.href === '#/machines'}
					<span class="tnum flex items-center gap-1.5 text-[11.5px] font-normal text-muted-foreground" title={connLabel}>
						{#if fleet.conn === 'unreachable'}
							<StatusDot status="failed" /><span class="text-err-ink">Unreachable</span>
						{:else if fleet.loaded}
							{#if fleet.updatable.length}<CircleArrowUp class="size-3.5 text-queue-ink" aria-label="{fleet.updatable.length} can update" />{/if}
							<StatusDot status={fleet.online.length ? 'online' : 'offline'} />{fleet.online.length}/{fleet.workers.length}
						{/if}
					</span>
				{/if}
			</a>
		{/each}
	</nav>

	<RunList {query} {activeId} />

	<div class="border-t border-sidebar-border px-2 py-2">
		<AccountMenu {path} />
	</div>
</div>
