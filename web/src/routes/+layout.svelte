<script lang="ts">
	import './layout.css';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { ModeWatcher, setMode, userPrefersMode } from 'mode-watcher';
	import { Toaster } from '#lib/components/ui/sonner';
	import * as Tooltip from '#lib/components/ui/tooltip';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import Plus from '@lucide/svelte/icons/plus';
	import ListTree from '@lucide/svelte/icons/list';
	import Server from '@lucide/svelte/icons/server';
	import Settings from '@lucide/svelte/icons/settings-2';
	import Logo from '#lib/sf/ui/Logo.svelte';
	import StatusDot from '#lib/sf/ui/StatusDot.svelte';
	import KeyGate from '#lib/sf/ui/KeyGate.svelte';
	import RepoPicker from '#lib/sf/ui/RepoPicker.svelte';
	import { fleet } from '#lib/sf/state.svelte';
	import { parseBuild } from '#lib/sf/format';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
	onMount(() => fleet.start());

	const path = $derived(page.url.hash.replace(/^#/, '') || '/');
	const nav = [
		{ href: '#/', label: 'New task', icon: Plus, on: (p: string) => p === '/' },
		{ href: '#/runs', label: 'Runs', icon: ListTree, on: (p: string) => p.startsWith('/runs') },
		{ href: '#/machines', label: 'Machines', icon: Server, on: (p: string) => p.startsWith('/machines') }
	];
	const running = $derived(fleet.runs.filter((r) => r.thread_status === 'running').length);
	const build = $derived(parseBuild(fleet.setup?.build));
	const connLabel = $derived(
		fleet.conn === 'unreachable' ? 'Control plane unreachable'
		: !fleet.loaded ? 'Connecting…'
		: `${fleet.online.length} of ${fleet.workers.length} machine${fleet.workers.length === 1 ? '' : 's'} online`
	);
</script>

<ModeWatcher defaultMode="system" />
<Toaster position="bottom-right" />

<Tooltip.Provider delayDuration={300}>
	<div class="flex min-h-dvh flex-col md:flex-row">
		<aside class="flex shrink-0 items-center gap-1 border-b border-sidebar-border bg-sidebar px-3 py-2 md:sticky md:top-0 md:h-dvh md:w-56 md:flex-col md:items-stretch md:gap-0.5 md:border-r md:border-b-0 md:px-3 md:py-4">
			<a href="#/" class="mr-2 flex items-center gap-2 rounded-md px-2 py-1.5 text-[15px] font-semibold tracking-tight md:mr-0 md:mb-4">
				<Logo />Sidefleet
			</a>
			<nav class="flex gap-0.5 md:flex-col" aria-label="Main">
				{#each nav as item (item.href)}
					<a
						href={item.href}
						aria-current={item.on(path) ? 'page' : undefined}
						class="flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-[13px] transition-colors {item.on(path)
							? 'bg-sidebar-accent font-medium text-foreground'
							: 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground'}"
					>
						<item.icon class="size-4" />
						<span class="hidden sm:inline">{item.label}</span>
						{#if item.href === '#/runs' && running}
							<span class="tnum ml-auto hidden rounded-full bg-run-soft px-1.5 text-[11px] font-medium text-run-ink md:inline">{running}</span>
						{/if}
					</a>
				{/each}
			</nav>

			<div class="ml-auto flex items-center gap-1 md:mt-auto md:ml-0 md:border-t md:border-sidebar-border md:pt-3">
				<a href="#/machines" class="flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-xs text-muted-foreground hover:text-foreground">
					<StatusDot status={fleet.conn === 'unreachable' ? 'failed' : fleet.online.length ? 'online' : 'offline'} />
					<span class="hidden truncate md:inline">{connLabel}</span>
				</a>
				<DropdownMenu.Root>
					<DropdownMenu.Trigger class="grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground" aria-label="Settings">
						<Settings class="size-4" />
					</DropdownMenu.Trigger>
					<DropdownMenu.Content align="end" side="top" class="w-56">
						<DropdownMenu.Label class="text-xs font-normal text-muted-foreground">Appearance</DropdownMenu.Label>
						<DropdownMenu.RadioGroup value={userPrefersMode.current} onValueChange={(v) => setMode(v as 'light' | 'dark' | 'system')}>
							<DropdownMenu.RadioItem value="system">Match system</DropdownMenu.RadioItem>
							<DropdownMenu.RadioItem value="light">Light</DropdownMenu.RadioItem>
							<DropdownMenu.RadioItem value="dark">Dark</DropdownMenu.RadioItem>
						</DropdownMenu.RadioGroup>
						<DropdownMenu.Separator />
						<DropdownMenu.Item onclick={() => (fleet.pickerOpen = true)}>Choose repos…</DropdownMenu.Item>
						<DropdownMenu.Item onclick={() => fleet.signOut()}>Change developer key…</DropdownMenu.Item>
						{#if build.sha}
							<DropdownMenu.Separator />
							<DropdownMenu.Label class="text-xs font-normal text-muted-foreground">Control plane {build.version}, build <span class="font-mono">{build.sha}</span> from {build.date}</DropdownMenu.Label>
						{/if}
					</DropdownMenu.Content>
				</DropdownMenu.Root>
			</div>
		</aside>

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

{#if fleet.needsKey}<KeyGate />{/if}
<RepoPicker />
