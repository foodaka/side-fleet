<script lang="ts">
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import Plus from '@lucide/svelte/icons/plus';
	import { fleet } from '#lib/sf/state.svelte';
	import AddMachine from '#lib/sf/ui/AddMachine.svelte';
	import MachineCard from '#lib/sf/ui/MachineCard.svelte';
	import FleetMap from '#lib/sf/ui/FleetMap.svelte';
	import PendingMachines from '#lib/sf/ui/PendingMachines.svelte';
	import WorkerTokens from '#lib/sf/ui/WorkerTokens.svelte';

	let hover = $state<string | null>(null);

	// Yours first, then teammates' (which only run their owner's tasks).
	const sorted = $derived(
		[...fleet.workers].sort(
			(a, b) => Number(b.mine !== false) - Number(a.mine !== false) || Number(b.online) - Number(a.online) || a.display_name.localeCompare(b.display_name)
		)
	);
	const busy = $derived(fleet.runs.filter((r) => r.thread_status === 'running').length);
	const slotsTotal = $derived(fleet.online.reduce((a, w) => a + (w.capabilities.slots?.parallel ?? 0), 0));
	const outdated = $derived(fleet.workers.filter((w) => w.online && w.outdated && !w.update_requested).length);
	const limits = $derived(fleet.org?.limits);
</script>

<svelte:head><title>Machines · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-6 px-4 pt-8 pb-[calc(2rem+env(safe-area-inset-bottom))] md:px-8">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-[22px] font-semibold tracking-tight">Machines</h1>
			<p class="mt-1 text-[13px] text-muted-foreground">
				{`${fleet.online.length} of ${fleet.workers.length} online, ${busy} of ${slotsTotal} task slots busy.` + (outdated ? ` ${outdated} ${outdated === 1 ? 'runs' : 'run'} an older build than this control plane.` : '')}
			</p>
			{#if fleet.multi}
				<p class="mt-0.5 text-xs text-muted-foreground">Each machine runs only its owner's tasks, in every org its owner belongs to. Teammates' machines show here so you can see what the team is running.</p>
			{/if}
			{#if limits && fleet.org}
				<p class="mt-0.5 text-xs text-muted-foreground">
					{fleet.org.name} is on the {fleet.org.plan} plan: {fleet.workers.filter((w) => !w.guest).length} of {limits.machines} machines, up to {limits.parallel_runs} tasks queued or running at once.
				</p>
			{/if}
		</div>
		<div class="flex gap-2">
			<Button variant="outline" class="h-8 rounded-full text-[13px]" onclick={() => (fleet.pickerOpen = true)}>Choose repos</Button>
			<Button class="h-8 rounded-full text-[13px]" onclick={() => (fleet.addMachineOpen = true)}><Plus class="size-4" />Add machine</Button>
		</div>
	</header>

	<PendingMachines />

	{#if sorted.length}
		<FleetMap bind:hover />
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
			{#each sorted as w (w.id)}<MachineCard {w} hot={hover === w.id} onhover={(id) => (hover = id)} />{/each}
		</div>
	{:else}
		<div class="rounded-xl border bg-card p-6">
			<h2 class="mb-1 text-[15px] font-medium">No machines yet</h2>
			<p class="mb-5 text-[13px] text-muted-foreground">Connect one with a coding agent set up: Claude Code, Codex or opencode. It needs no open ports, only a way to reach this page.</p>
			<AddMachine />
		</div>
	{/if}

	<WorkerTokens />
</section>

<Dialog.Root bind:open={fleet.addMachineOpen}>
	<Dialog.Content class="sm:max-w-[40rem]">
		<Dialog.Header>
			<Dialog.Title>Add a machine</Dialog.Title>
			<Dialog.Description>Any Mac or Linux box with Claude Code, Codex or opencode set up. It connects out to this control plane; nothing connects in.</Dialog.Description>
		</Dialog.Header>
		<AddMachine />
	</Dialog.Content>
</Dialog.Root>
