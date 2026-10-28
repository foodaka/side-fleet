<script lang="ts">
	// Enable a personal machine in the current organization.
	import { Button } from '#lib/components/ui/button';
	import { fleet } from '../state.svelte';
	import { rel } from '../format';
	import type { MyMachine } from '../types';

	/** Only machines with this repo (a repo key), e.g. a room's. */
	let { repo, class: cls = '' }: { repo?: string; class?: string } = $props();

	const items = $derived(
		fleet.elsewhere.filter((m) => !repo || m.repos.includes(repo)).sort((a, b) => Number(b.online) - Number(a.online))
	);
	let busy = $state<string | null>(null);

	async function enable(m: MyMachine) {
		busy = m.machine_id;
		await fleet.assignMachine(m);
		busy = null;
	}
</script>

{#each items as m (m.machine_id)}
	<div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-muted-foreground {cls}">
		<span class="min-w-0">
			Enable your machine <span class="font-medium text-foreground">{m.display_name}</span> here to run your tasks{m.online ? '' : ` (offline, last seen ${rel(m.last_heartbeat_at, fleet.now)})`}.
		</span>
		<Button size="sm" variant="outline" class="h-7 rounded-full text-xs" disabled={busy === m.machine_id} onclick={() => enable(m)}>
			Enable in {fleet.org?.name ?? 'this org'}
		</Button>
	</div>
{/each}
