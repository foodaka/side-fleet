<script lang="ts">
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import Plus from '@lucide/svelte/icons/plus';
	import { fleet } from '#lib/sf/state.svelte';
	import AddMachine from '#lib/sf/ui/AddMachine.svelte';
	import MachineCard from '#lib/sf/ui/MachineCard.svelte';

	const sorted = $derived([...fleet.workers].sort((a, b) => Number(b.online) - Number(a.online) || a.display_name.localeCompare(b.display_name)));
	const outdated = $derived(fleet.workers.filter((w) => w.online && w.outdated && !w.update_requested).length);
</script>

<svelte:head><title>Machines · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-6 px-4 py-8 md:px-8">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-[22px] font-semibold tracking-tight">Machines</h1>
			<p class="mt-1 text-[13px] text-muted-foreground">
				{fleet.online.length} of {fleet.workers.length} online.{#if outdated} {outdated} {outdated === 1 ? 'runs' : 'run'} an older build than this control plane.{/if}
			</p>
		</div>
		<div class="flex gap-2">
			<Button variant="outline" class="h-8 rounded-full text-[13px]" onclick={() => (fleet.pickerOpen = true)}>Choose repos</Button>
			<Button class="h-8 rounded-full text-[13px]" onclick={() => (fleet.addMachineOpen = true)}><Plus class="size-4" />Add machine</Button>
		</div>
	</header>

	{#if sorted.length}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
			{#each sorted as w (w.id)}<MachineCard {w} />{/each}
		</div>
	{:else}
		<div class="rounded-xl border bg-card p-6">
			<h2 class="mb-1 text-[15px] font-medium">No machines yet</h2>
			<p class="mb-5 text-[13px] text-muted-foreground">Connect one that has Claude Code signed in. It needs no open ports, only a way to reach this page.</p>
			<AddMachine />
		</div>
	{/if}
</section>

<Dialog.Root bind:open={fleet.addMachineOpen}>
	<Dialog.Content class="sm:max-w-[40rem]">
		<Dialog.Header>
			<Dialog.Title>Add a machine</Dialog.Title>
			<Dialog.Description>Any Mac or Linux box with Claude Code signed in. It connects out to this control plane; nothing connects in.</Dialog.Description>
		</Dialog.Header>
		<AddMachine />
	</Dialog.Content>
</Dialog.Root>
