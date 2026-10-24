<script lang="ts">
	// The dialog that creates a team org (from the org switcher or the settings menu).
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { toast } from 'svelte-sonner';
	import { fleet } from '../state.svelte';
	import { api } from '../api';
	import type { Org } from '../types';

	let name = $state('');
	let busy = $state(false);

	async function create(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const org = await api<Org>('/v1/orgs', { method: 'POST', body: JSON.stringify({ name: name.trim() }) });
			fleet.newOrgOpen = false;
			name = '';
			fleet.switchOrg(org.id);
			toast.success(`Created ${org.name}`);
		} catch (err) {
			toast.error(err instanceof Error ? err.message : 'Could not create the org');
		} finally {
			busy = false;
		}
	}
</script>

<Dialog.Root bind:open={fleet.newOrgOpen}>
	<Dialog.Content class="sm:max-w-[26rem]">
		<Dialog.Header>
			<Dialog.Title>New org</Dialog.Title>
			<Dialog.Description>An org has its own machines, runs and worker tokens. You can add people who have signed in to Sidefleet once.</Dialog.Description>
		</Dialog.Header>
		<form onsubmit={create} class="flex flex-col gap-4">
			<Input bind:value={name} placeholder="Acme" maxlength={80} class="h-9" aria-label="Org name" />
			<Button type="submit" class="h-9 rounded-full" disabled={busy || !name.trim()}>{busy ? 'Creating…' : 'Create org'}</Button>
		</form>
	</Dialog.Content>
</Dialog.Root>
