<script lang="ts">
	// Settings > Your machines: choose each machine’s organization assignments.
	import * as Dialog from '#lib/components/ui/dialog';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import { Button } from '#lib/components/ui/button';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import { fleet } from '../state.svelte';
	import { rel } from '../format';
	import StatusDot from './StatusDot.svelte';
	import type { MyMachine } from '../types';

	const orgs = $derived(fleet.me?.orgs ?? []);
	let busy = $state<string | null>(null);

	async function toggle(m: MyMachine, to: string) {
		busy = m.machine_id;
		await fleet.assignMachine(m, to, !m.orgs.some((o) => o.id === to));
		busy = null;
	}
</script>

<Dialog.Root bind:open={fleet.machinesOpen}>
	<Dialog.Content class="sm:max-w-[34rem]">
		<Dialog.Header>
			<Dialog.Title>Your machines</Dialog.Title>
			<Dialog.Description>
				Choose which organizations each machine takes your tasks from. Only you can run tasks on your machines. Disabling an organization lets running tasks finish. Each machine counts only against its home organization’s plan.
			</Dialog.Description>
		</Dialog.Header>
		{#if fleet.myMachines.length}
			<ul class="flex flex-col divide-y rounded-xl border">
				{#each fleet.myMachines as m (m.machine_id)}
					<li class="flex items-center gap-3 px-3.5 py-3">
						<div class="min-w-0 flex-1">
							<p class="flex items-center gap-2 truncate text-[13.5px] font-medium">
								<StatusDot status={m.online ? 'online' : 'offline'} />{m.display_name}
							</p>
							<p class="mt-0.5 truncate text-xs text-muted-foreground">
								<span class="text-foreground">{m.orgs.map((o) => o.name).join(', ') || 'No organizations enabled'}</span> · {m.online ? 'online' : `last seen ${rel(m.last_heartbeat_at, fleet.now)}`} · {m.repos.length} repo{m.repos.length === 1 ? '' : 's'} · home: {m.home_org?.name ?? 'none'}
							</p>
						</div>
						{#if orgs.length > 0}
							<DropdownMenu.Root>
								<DropdownMenu.Trigger>
									{#snippet child({ props })}
										<Button {...props} size="sm" variant="outline" class="h-7 shrink-0 rounded-full text-xs" disabled={busy === m.machine_id}>
											Organizations<ChevronDown class="size-3.5" />
										</Button>
									{/snippet}
								</DropdownMenu.Trigger>
								<DropdownMenu.Content align="end" class="w-52">
									{#each orgs as o (o.id)}
										<DropdownMenu.CheckboxItem checked={m.orgs.some((assigned) => assigned.id === o.id)} disabled={busy === m.machine_id} onCheckedChange={() => toggle(m, o.id)}>{o.name}</DropdownMenu.CheckboxItem>
									{/each}
								</DropdownMenu.Content>
							</DropdownMenu.Root>
						{/if}
					</li>
				{/each}
			</ul>
		{:else}
			<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">
				No machines of yours yet. <a href="#/machines" class="text-foreground underline underline-offset-2" onclick={() => (fleet.machinesOpen = false)}>Connect one</a> to start running tasks.
			</p>
		{/if}
	</Dialog.Content>
</Dialog.Root>
