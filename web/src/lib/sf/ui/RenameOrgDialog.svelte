<script lang="ts">
	// Renames the org the page acts in (owners), from Settings, the org switcher or the Team page.
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { toast } from 'svelte-sonner';
	import { fleet } from '../state.svelte';

	let name = $state('');
	let busy = $state(false);

	$effect(() => {
		if (fleet.renameOrgOpen) name = fleet.org?.name ?? '';
	});

	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			await fleet.renameOrg(name.trim());
			fleet.renameOrgOpen = false;
			toast.success(`Renamed to ${name.trim()}`);
		} catch (err) {
			toast.error('Could not rename the org', { description: (err as Error).message });
		} finally {
			busy = false;
		}
	}
</script>

<Dialog.Root bind:open={fleet.renameOrgOpen}>
	<Dialog.Content class="sm:max-w-[26rem]">
		<Dialog.Header>
			<Dialog.Title>Rename org</Dialog.Title>
			<Dialog.Description>Everyone in it sees the new name. Machines, tasks and invite links keep working.</Dialog.Description>
		</Dialog.Header>
		<form onsubmit={save} class="flex flex-col gap-4">
			<Input bind:value={name} maxlength={80} class="h-9" aria-label="Org name" />
			<Button type="submit" class="h-9 rounded-full" disabled={busy || !name.trim() || name.trim() === fleet.org?.name}>{busy ? 'Saving…' : 'Save'}</Button>
		</form>
	</Dialog.Content>
</Dialog.Root>
