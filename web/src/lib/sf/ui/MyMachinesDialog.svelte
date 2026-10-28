<script lang="ts">
	// Settings > Your machines: every machine of yours, once. Each connected to one org and
	// runs your tasks in all of them.
	import * as Dialog from '#lib/components/ui/dialog';
	import { fleet } from '../state.svelte';
	import { rel } from '../format';
	import StatusDot from './StatusDot.svelte';

	const several = $derived((fleet.me?.orgs.length ?? 0) > 1);
</script>

<Dialog.Root bind:open={fleet.machinesOpen}>
	<Dialog.Content class="sm:max-w-[34rem]">
		<Dialog.Header>
			<Dialog.Title>Your machines</Dialog.Title>
			<Dialog.Description>
				{several
					? 'Each of your machines runs your tasks in every org you belong to, so you connect it once. It counts against the plan of the org it connected to.'
					: 'Your machines run your tasks here. Join or create another org and they work there too, with nothing to reconnect.'}
			</Dialog.Description>
		</Dialog.Header>
		{#if fleet.myMachines.length}
			<ul class="flex flex-col divide-y rounded-xl border">
				{#each fleet.myMachines as m (m.org.id + m.id)}
					<li class="px-3.5 py-3">
						<p class="flex items-center gap-2 truncate text-[13.5px] font-medium">
							<StatusDot status={m.online ? 'online' : 'offline'} />{m.display_name}
						</p>
						<p class="mt-0.5 truncate text-xs text-muted-foreground">
							{m.online ? 'Online' : `Last seen ${rel(m.last_heartbeat_at, fleet.now)}`} · {m.repos.length} repo{m.repos.length === 1 ? '' : 's'}{several ? ` · connected to ${m.org.name}` : ''}
						</p>
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
