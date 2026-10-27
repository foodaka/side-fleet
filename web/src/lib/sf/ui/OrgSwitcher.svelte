<script lang="ts">
	// Which org the page acts in, always shown to signed-in users so they know where their tasks go.
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
	import { fleet } from '../state.svelte';

	const orgs = $derived(fleet.me?.orgs ?? []);
</script>

{#if fleet.me?.user && fleet.org}
	<DropdownMenu.Root>
		<DropdownMenu.Trigger
			class="flex w-full min-w-0 items-center gap-2 rounded-md border border-sidebar-border bg-background/60 px-2 py-1.5 text-left text-[13px] hover:bg-sidebar-accent"
			aria-label="Switch org"
		>
			<span class="grid size-5 shrink-0 place-items-center rounded bg-foreground/10 text-[11px] font-semibold uppercase">{fleet.org.name.slice(0, 1)}</span>
			<span class="min-w-0 flex-1 truncate font-medium">{fleet.org.name}</span>
			<ChevronsUpDown class="size-3.5 shrink-0 text-muted-foreground" />
		</DropdownMenu.Trigger>
		<DropdownMenu.Content align="start" class="w-60">
			<DropdownMenu.Label class="text-xs font-normal text-muted-foreground">Orgs</DropdownMenu.Label>
			<DropdownMenu.RadioGroup value={fleet.org.id} onValueChange={(v) => v && v !== fleet.org?.id && fleet.switchOrg(v)}>
				{#each orgs as o (o.id)}
					<DropdownMenu.RadioItem value={o.id}>
						<span class="min-w-0 flex-1 truncate">{o.name}</span>
						<span class="ml-2 text-[11px] text-muted-foreground">{o.plan}{o.role === 'owner' ? '' : ' · member'}</span>
					</DropdownMenu.RadioItem>
				{/each}
			</DropdownMenu.RadioGroup>
			<DropdownMenu.Separator />
			<DropdownMenu.Item onclick={() => (fleet.machinesOpen = true)}>Your machines…</DropdownMenu.Item>
			{#if fleet.isOwner}<DropdownMenu.Item onclick={() => (fleet.renameOrgOpen = true)}>Rename {fleet.org.name}…</DropdownMenu.Item>{/if}
			<DropdownMenu.Item onclick={() => (fleet.newOrgOpen = true)}>New org…</DropdownMenu.Item>
		</DropdownMenu.Content>
	</DropdownMenu.Root>
{/if}
