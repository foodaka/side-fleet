<script lang="ts">
	// The sidebar's footer: which org you're in, and everything you set up rather than visit often.
	import { goto } from '$app/navigation';
	import { setMode, userPrefersMode } from 'mode-watcher';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
	import UsersRound from '@lucide/svelte/icons/users-round';
	import MessagesSquare from '@lucide/svelte/icons/messages-square';
	import MessageSquare from '@lucide/svelte/icons/message-square';
	import Settings from '@lucide/svelte/icons/settings';
	import { fleet } from '../state.svelte';
	import { parseBuild } from '../format';

	let { path }: { path: string } = $props();

	let open = $state(false);
	const orgs = $derived(fleet.me?.orgs ?? []);
	const user = $derived(fleet.me?.user);
	const build = $derived(parseBuild(fleet.setup?.build));
	// The pages this menu leads to: the row stays lit while you're on one.
	const here = $derived(['/team', '/chat', '/settings'].some((p) => path.startsWith(p)));
	const title = $derived(fleet.org?.name ?? 'Sidefleet');
</script>

<DropdownMenu.Root bind:open>
	<DropdownMenu.Trigger
		class="mi-hover flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-sidebar-accent/60 data-[state=open]:bg-sidebar-accent {here ? 'bg-sidebar-accent' : ''}"
		aria-label="Org, team and settings"
	>
		<span class="grid size-7 shrink-0 place-items-center rounded-md bg-foreground/10 text-[12px] font-semibold uppercase">{title.slice(0, 1)}</span>
		<span class="flex min-w-0 flex-1 flex-col">
			<span class="truncate text-[13px] font-medium text-foreground">{title}</span>
			<span class="truncate text-[11.5px] text-muted-foreground">{user ? user.login : 'Settings'}</span>
		</span>
		<ChevronsUpDown class="size-3.5 shrink-0 text-muted-foreground" />
	</DropdownMenu.Trigger>
	<DropdownMenu.Content align="start" side="top" class="w-64">
		{#if user && fleet.org}
			<DropdownMenu.Label class="text-xs font-normal text-muted-foreground">Orgs</DropdownMenu.Label>
			<DropdownMenu.RadioGroup value={fleet.org.id} onValueChange={(v) => v && v !== fleet.org?.id && fleet.switchOrg(v)}>
				{#each orgs as o (o.id)}
					<DropdownMenu.RadioItem value={o.id}>
						<span class="min-w-0 flex-1 truncate">{o.name}</span>
						<span class="ml-2 text-[11px] text-muted-foreground">{o.plan}{o.role === 'owner' ? '' : ' · member'}</span>
					</DropdownMenu.RadioItem>
				{/each}
			</DropdownMenu.RadioGroup>
			{#if fleet.isOwner}<DropdownMenu.Item onclick={() => (fleet.renameOrgOpen = true)}>Rename {fleet.org.name}…</DropdownMenu.Item>{/if}
			<DropdownMenu.Item onclick={() => (fleet.newOrgOpen = true)}>New org…</DropdownMenu.Item>
			<DropdownMenu.Separator />
		{/if}
		{#if user}
			<DropdownMenu.Item onclick={() => goto('#/team')}><UsersRound class="size-3.5" />Team</DropdownMenu.Item>
		{/if}
		<DropdownMenu.Item onclick={() => goto('#/chat')}><MessagesSquare class="size-3.5" />Chat apps</DropdownMenu.Item>
		<DropdownMenu.Item onclick={() => goto('#/settings')}><Settings class="size-3.5" />Settings</DropdownMenu.Item>
		{#if user}<DropdownMenu.Item onclick={() => (fleet.machinesOpen = true)}>Your machines…</DropdownMenu.Item>{/if}
		<DropdownMenu.Item onclick={() => (fleet.newProjectOpen = true)}>New project…</DropdownMenu.Item>
		<DropdownMenu.Item onclick={() => (fleet.pickerOpen = true)}>Choose repos…</DropdownMenu.Item>
		<DropdownMenu.Separator />
		<DropdownMenu.Label class="text-xs font-normal text-muted-foreground">Appearance</DropdownMenu.Label>
		<DropdownMenu.RadioGroup value={userPrefersMode.current} onValueChange={(v) => setMode(v as 'light' | 'dark' | 'system')}>
			<DropdownMenu.RadioItem value="system">Match system</DropdownMenu.RadioItem>
			<DropdownMenu.RadioItem value="light">Light</DropdownMenu.RadioItem>
			<DropdownMenu.RadioItem value="dark">Dark</DropdownMenu.RadioItem>
		</DropdownMenu.RadioGroup>
		<DropdownMenu.Separator />
		<DropdownMenu.Item onclick={() => (fleet.feedbackOpen = true)}><MessageSquare class="size-3.5" />Send feedback</DropdownMenu.Item>
		{#if user}
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
