<script lang="ts">
	import './layout.css';
	import { page } from '$app/state';
	import { onMount, untrack } from 'svelte';
	import { Dialog as DialogPrimitive } from 'bits-ui';
	import { ModeWatcher } from 'mode-watcher';
	import { Toaster } from '#lib/components/ui/sonner';
	import * as Tooltip from '#lib/components/ui/tooltip';
	import { Kbd } from '#lib/components/ui/kbd';
	import Menu from '@lucide/svelte/icons/menu';
	import { LayoutGrid, PanelLeftOpen, Plus, ServerCog } from '#lib/components/movingicons';
	import Logo from '#lib/sf/ui/Logo.svelte';
	import StatusDot from '#lib/sf/ui/StatusDot.svelte';
	import Sidebar from '#lib/sf/ui/Sidebar.svelte';
	import SignIn from '#lib/sf/ui/SignIn.svelte';
	import NewOrgDialog from '#lib/sf/ui/NewOrgDialog.svelte';
	import RepoPicker from '#lib/sf/ui/RepoPicker.svelte';
	import InboxBell from '#lib/sf/ui/InboxBell.svelte';
	import { inbox } from '#lib/sf/inbox.svelte';
	import { fleet } from '#lib/sf/state.svelte';
	import { seen } from '#lib/sf/seen.svelte';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
	onMount(() => fleet.start());

	const path = $derived(page.url.hash.replace(/^#/, '') || '/');
	const running = $derived(fleet.runs.filter((r) => r.thread_status === 'running').length);

	// Opening a run marks it seen, and keeps it seen as new replies land while it's open.
	$effect(() => {
		if (fleet.loaded) untrack(() => seen.init(fleet.runs));
	});
	$effect(() => {
		const id = /^\/runs\/([^/?#]+)/.exec(path)?.[1];
		const r = id && fleet.runs.find((r) => r.id === id);
		if (r) untrack(() => seen.mark(r));
	});

	// The "waiting on you" inbox follows whoever is signed in, in the org the page acts in.
	const inboxKey = $derived(fleet.loaded && !fleet.needsSignIn ? fleet.orgId || '-' : null);
	$effect(() => {
		if (inboxKey) untrack(() => inbox.connect());
		else untrack(() => inbox.disconnect());
	});
	// Looking at a task that waits on you counts as seeing it (not while the tab is in the background).
	$effect(() => {
		const id = /^\/runs\/([^/?#]+)/.exec(path)?.[1];
		if (id && inbox.visible && inbox.byId.has(id)) untrack(() => void inbox.dismiss(id));
	});

	// Desktop: the sidebar can be tucked away to a slim rail; the choice sticks.
	const SIDEBAR = 'sidefleet_sidebar';
	let collapsed = $state(false);
	try { collapsed = localStorage.getItem(SIDEBAR) === 'collapsed'; } catch { /* storage blocked */ }
	function toggle() {
		collapsed = !collapsed;
		try { localStorage.setItem(SIDEBAR, collapsed ? 'collapsed' : 'open'); } catch { /* storage blocked */ }
	}
	const rail = [
		{ href: '#/runs', label: 'Overview', icon: LayoutGrid, on: (p: string) => p === '/runs' },
		{ href: '#/machines', label: 'Machines', icon: ServerCog, on: (p: string) => p.startsWith('/machines') }
	];
	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
	const toggleKeys = isMac ? '⌘B' : 'Ctrl B';
	function onkeydown(e: KeyboardEvent) {
		if ((isMac ? e.metaKey : e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'b') {
			e.preventDefault();
			toggle();
		}
	}

	// Phones: the sidebar is a drawer, closed again once you pick somewhere to go.
	let drawer = $state(false);
	$effect(() => {
		void path;
		untrack(() => (drawer = false));
	});
</script>

<svelte:window {onkeydown} />
<ModeWatcher defaultMode="system" />
<Toaster position="bottom-right" />

<Tooltip.Provider delayDuration={300}>
	<div class="flex min-h-dvh flex-col md:flex-row">
		<!-- Desktop -->
		{#if collapsed}
			<aside class="sticky top-0 hidden h-dvh w-13 shrink-0 flex-col items-center gap-1 border-r border-sidebar-border bg-sidebar py-3 md:flex">
				<Tooltip.Root>
					<Tooltip.Trigger class="mi-hover grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground" aria-label="Show sidebar" onclick={toggle}>
						<PanelLeftOpen size={14} />
					</Tooltip.Trigger>
					<Tooltip.Content side="right">Show sidebar<Kbd class="ml-1.5">{toggleKeys}</Kbd></Tooltip.Content>
				</Tooltip.Root>
				<Tooltip.Root>
					<Tooltip.Trigger>
						{#snippet child({ props })}
							<a {...props} href="#/" class="mi-hover mt-1 grid size-8 place-items-center rounded-lg border bg-card shadow-xs hover:bg-accent/60" aria-label="New task"><Plus size={14} /></a>
						{/snippet}
					</Tooltip.Trigger>
					<Tooltip.Content side="right">New task</Tooltip.Content>
				</Tooltip.Root>
				<InboxBell side="right" class="mt-1 size-8" />
				{#if running}
					<button type="button" onclick={toggle} class="tnum mt-2 flex items-center gap-1 rounded-full bg-run-soft px-1.5 py-0.5 text-[11px] font-medium text-run-ink" title="{running} running">
						<StatusDot status="running" />{running}
					</button>
				{/if}
				<nav class="mt-auto flex flex-col items-center gap-1" aria-label="Main">
					{#each rail as n (n.href)}
						<Tooltip.Root>
							<Tooltip.Trigger>
								{#snippet child({ props })}
									<a
										{...props}
										href={n.href}
										aria-label={n.label}
										aria-current={n.on(path) ? 'page' : undefined}
										class="mi-hover relative grid size-8 place-items-center rounded-md {n.on(path) ? 'bg-sidebar-accent text-foreground' : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground'}"
									>
										<n.icon size={14} />
										{#if n.href === '#/machines'}
											<StatusDot status={fleet.conn === 'unreachable' ? 'failed' : fleet.online.length ? 'online' : 'offline'} class="absolute top-1.5 right-1.5" />
										{/if}
									</a>
								{/snippet}
							</Tooltip.Trigger>
							<Tooltip.Content side="right">{n.label}</Tooltip.Content>
						</Tooltip.Root>
					{/each}
				</nav>
			</aside>
		{:else}
			<aside class="sticky top-0 hidden h-dvh w-76 shrink-0 border-r border-sidebar-border bg-sidebar md:block">
				<Sidebar {path} onClose={toggle} closeKeys={toggleKeys} />
			</aside>
		{/if}

		<!-- Phones -->
		<header class="sticky top-0 z-30 flex h-13 shrink-0 items-center gap-1 border-b border-sidebar-border bg-sidebar/90 px-2 backdrop-blur md:hidden">
			<button type="button" class="relative grid size-9 place-items-center rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground" aria-label="Open menu" onclick={() => (drawer = true)}>
				<Menu class="size-5" />
				{#if running}<StatusDot status="running" class="absolute top-2 right-1.5" />{/if}
			</button>
			<a href="#/" class="flex items-center gap-2 text-[15px] font-semibold tracking-tight"><Logo />Sidefleet</a>
			<InboxBell align="end" class="ml-auto size-9" />
			<a href="#/" class="mi-hover grid size-9 place-items-center rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground" aria-label="New task"><Plus size={18} /></a>
		</header>
		<DialogPrimitive.Root bind:open={drawer}>
			<DialogPrimitive.Portal>
				<DialogPrimitive.Overlay class="fixed inset-0 z-50 bg-black/25 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0 md:hidden" />
				<DialogPrimitive.Content class="fixed inset-y-0 left-0 z-50 w-72 max-w-[85vw] border-r border-sidebar-border bg-sidebar shadow-xl outline-none duration-200 data-open:animate-in data-open:slide-in-from-left data-closed:animate-out data-closed:slide-out-to-left md:hidden">
					<DialogPrimitive.Title class="sr-only">Navigation</DialogPrimitive.Title>
					<Sidebar {path} onClose={() => (drawer = false)} drawer />
				</DialogPrimitive.Content>
			</DialogPrimitive.Portal>
		</DialogPrimitive.Root>

		<main class="min-w-0 flex-1">
			{#if fleet.conn === 'unreachable' && fleet.loaded}
				<div class="border-b border-queue/30 bg-queue-soft px-4 py-2 text-center text-xs text-queue-ink" role="status">
					Lost contact with the control plane. Retrying every few seconds; what you see may be out of date.
				</div>
			{/if}
			{@render children()}
		</main>
	</div>
</Tooltip.Provider>

{#if fleet.needsSignIn}<SignIn />{/if}
<RepoPicker />
<NewOrgDialog />
