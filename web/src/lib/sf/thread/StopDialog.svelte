<script lang="ts">
	// Asks before stopping a running task, so a stray tap on a phone doesn't end
	// it. Can be told not to ask again (turned back on in Settings).
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import Square from '@lucide/svelte/icons/square';
	import { toast } from 'svelte-sonner';
	import { goto } from '$app/navigation';
	import { prefs } from '../prefs.svelte';

	let { open = $bindable(false), agent, queued = 0, onstop }: {
		open?: boolean;
		agent: string;
		/** Messages waiting behind the running one, which stopping drops too. */
		queued?: number;
		onstop: () => void;
	} = $props();

	let quiet = $state(false);
	$effect(() => { if (open) quiet = false; });

	function confirm() {
		open = false;
		if (quiet) {
			prefs.set({ confirmStop: false });
			toast('Stop won’t ask again', { description: 'Turn it back on in Settings.', action: { label: 'Settings', onClick: () => void goto('#/settings') } });
		}
		onstop();
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-[26rem]" showCloseButton={false}>
		<Dialog.Header>
			<Dialog.Title>Stop this task?</Dialog.Title>
			<Dialog.Description>
				{agent} stops where it is. The files it changed so far stay as they are, and you can reply later to pick up from there.{#if queued}{' '}Your {queued === 1 ? 'queued message is' : `${queued} queued messages are`} dropped too.{/if}
			</Dialog.Description>
		</Dialog.Header>
		<label class="flex w-fit cursor-pointer items-center gap-2 text-[13px] text-muted-foreground select-none">
			<input type="checkbox" bind:checked={quiet} class="size-3.5 accent-foreground" />
			Don’t ask again
		</label>
		<Dialog.Footer>
			<Dialog.Close>
				{#snippet child({ props })}<Button {...props} variant="outline" class="rounded-full">Keep running</Button>{/snippet}
			</Dialog.Close>
			<!-- svelte-ignore a11y_autofocus -->
			<Button variant="destructive" class="rounded-full" autofocus onclick={confirm}><Square class="size-3 fill-current" />Stop</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
