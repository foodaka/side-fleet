<script lang="ts">
	// Your machines in your other orgs, each with a button that moves it into this one.
	// A machine takes tasks only from the org it is in; moving it needs no reconnecting.
	import { Button } from '#lib/components/ui/button';
	import { fleet } from '../state.svelte';
	import { rel } from '../format';
	import type { ElsewhereMachine } from '../types';

	/** Only machines with this repo (a repo key), e.g. a room's. */
	let { repo, class: cls = '' }: { repo?: string; class?: string } = $props();

	const items = $derived(
		fleet.elsewhere.filter((m) => !repo || m.repos.includes(repo)).sort((a, b) => Number(b.online) - Number(a.online))
	);
	let busy = $state<string | null>(null);

	async function move(m: ElsewhereMachine) {
		busy = m.id;
		await fleet.moveHere(m);
		busy = null;
	}
</script>

{#each items as m (m.org.id + m.id)}
	<div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-muted-foreground {cls}">
		<span class="min-w-0">
			Your machine <span class="font-medium text-foreground">{m.display_name}</span> is in
			<span class="font-medium text-foreground">{m.org.name}</span>{m.online ? '' : ` (offline, last seen ${rel(m.last_heartbeat_at, fleet.now)})`}, so it only takes tasks there.
		</span>
		<Button size="sm" variant="outline" class="h-7 rounded-full text-xs" disabled={busy === m.id} onclick={() => move(m)}>
			Move it to {fleet.org?.name ?? 'this org'}
		</Button>
	</div>
{/each}
