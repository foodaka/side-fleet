<script lang="ts">
	import { goto } from '$app/navigation';
	import { setMode, userPrefersMode } from 'mode-watcher';
	import * as Tooltip from '#lib/components/ui/tooltip';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import { Kbd } from '#lib/components/ui/kbd';
	import MessageSquare from '@lucide/svelte/icons/message-square';
	import MessagesSquare from '@lucide/svelte/icons/messages-square';
	import UsersRound from '@lucide/svelte/icons/users-round';
	import Boxes from '@lucide/svelte/icons/boxes';
	import { LayoutGrid, PanelLeftClose, Plus, Search, ServerCog, Settings, X } from '#lib/components/movingicons';
	import Logo from './Logo.svelte';
	import StatusDot from './StatusDot.svelte';
	import RunList from './RunList.svelte';
	import OrgSwitcher from './OrgSwitcher.svelte';
	import InboxBell from './InboxBell.svelte';
	import { fleet } from '../state.svelte';
	import { parseBuild } from '../format';

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
	let settingsOpen = $state(false);
	let search = $state<HTMLInputElement | null>(null);

	const activeId = $derived(/^\/runs\/([^/?#]+)/.exec(path)?.[1] ?? null);
	const build = $derived(parseBuild(fleet.setup?.build));
	const connLabel = $derived(
		fleet.conn === 'unreachable' ? 'Control plane unreachable'
		: !fleet.loaded ? 'Connecting…'
		: `${fleet.online.length} of ${fleet.workers.length} machine${fleet.workers.length === 1 ? '' : 's'} online`
	);
	const nav = $derived([
		{ href: '#/runs', label: 'Overview', icon: LayoutGrid, on: (p: string) => p === '/runs' },
		{ href: '#/rooms', label: 'Rooms', icon: Boxes, on: (p: string) => p.startsWith('/rooms') },
		{ href: '#/machines', label: 'Machines', icon: ServerCog, on: (p: string) => p.startsWith('/machines') },
		...(fleet.me?.user ? [{ href: '#/team', label: 'Team', icon: UsersRound, on: (p: string) => p.startsWith('/team') }] : []),
		{ href: '#/chat', label: 'Chat apps', icon: MessagesSquare, on: (p: string) => p.startsWith('/chat') }
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
		{#if !drawer}<InboxBell />{/if}
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
		<OrgSwitcher />
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

	<RunList {query} {activeId} />

	<nav class="flex flex-col gap-0.5 border-t border-sidebar-border px-3 py-2.5" aria-label="Main">
		{#each nav as n (n.href)}
			<a href={n.href} aria-current={n.on(path) ? 'page' : undefined} class={item(n.on(path))}>
				<n.icon size={14} />
				<span class="flex-1">{n.label}</span>
				{#if n.href === '#/machines'}
					<span class="tnum flex items-center gap-1.5 text-[11.5px] font-normal text-muted-foreground" title={connLabel}>
						{#if fleet.conn === 'unreachable'}
							<StatusDot status="failed" /><span class="text-err-ink">Unreachable</span>
						{:else if fleet.loaded}
							<StatusDot status={fleet.online.length ? 'online' : 'offline'} />{fleet.online.length}/{fleet.workers.length}
						{/if}
					</span>
				{/if}
			</a>
		{/each}
		<DropdownMenu.Root bind:open={settingsOpen}>
			<DropdownMenu.Trigger class="{item(path === '/settings')} w-full data-[state=open]:bg-sidebar-accent data-[state=open]:text-foreground">
				<Settings size={14} animate={settingsOpen} />Settings
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="start" side="top" class="w-60">
				<DropdownMenu.Label class="text-xs font-normal text-muted-foreground">Appearance</DropdownMenu.Label>
				<DropdownMenu.RadioGroup value={userPrefersMode.current} onValueChange={(v) => setMode(v as 'light' | 'dark' | 'system')}>
					<DropdownMenu.RadioItem value="system">Match system</DropdownMenu.RadioItem>
					<DropdownMenu.RadioItem value="light">Light</DropdownMenu.RadioItem>
					<DropdownMenu.RadioItem value="dark">Dark</DropdownMenu.RadioItem>
				</DropdownMenu.RadioGroup>
				<DropdownMenu.Separator />
				<DropdownMenu.Item onclick={() => goto('#/settings')}>All settings…</DropdownMenu.Item>
				<DropdownMenu.Item onclick={() => (fleet.pickerOpen = true)}>Choose repos…</DropdownMenu.Item>
				{#if fleet.me?.user}
					<DropdownMenu.Separator />
					<DropdownMenu.Label class="text-xs font-normal text-muted-foreground">Signed in as {fleet.me.user.login}{fleet.org ? `, working in ${fleet.org.name}` : ''}</DropdownMenu.Label>
					<DropdownMenu.Item onclick={() => (fleet.machinesOpen = true)}>Your machines…</DropdownMenu.Item>
					{#if fleet.isOwner && fleet.org}<DropdownMenu.Item onclick={() => (fleet.renameOrgOpen = true)}>Rename {fleet.org.name}…</DropdownMenu.Item>{/if}
					<DropdownMenu.Item onclick={() => (fleet.newOrgOpen = true)}>New org…</DropdownMenu.Item>
					<DropdownMenu.Item onclick={() => fleet.signOut()}>Sign out</DropdownMenu.Item>
				{:else}
					<DropdownMenu.Item onclick={() => fleet.signOut()}>Change developer key…</DropdownMenu.Item>
				{/if}
				{#if build.sha}
					<DropdownMenu.Separator />
					<DropdownMenu.Label class="text-xs font-normal text-muted-foreground">Control plane {build.version}, build <span class="font-mono">{build.sha}</span> from {build.date}</DropdownMenu.Label>
				{/if}
			</DropdownMenu.Content>
		</DropdownMenu.Root>
		<button type="button" class="{item(false)} w-full" onclick={() => (fleet.feedbackOpen = true)}>
			<MessageSquare size={14} />Feedback
		</button>
	</nav>
</div>
