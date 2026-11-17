<script lang="ts">
	// Shown instead of the page while the first load fails: a skeleton that never
	// ends says nothing, this says what went wrong and lets you try again.
	import CloudOff from '@lucide/svelte/icons/cloud-off';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { Button } from '#lib/components/ui/button';
	import { fleet } from '../state.svelte';
</script>

<div class="mx-auto flex min-h-[60vh] max-w-md flex-col items-center justify-center gap-4 px-6 py-16 text-center" role="alert">
	<span class="grid size-11 place-items-center rounded-full bg-err-soft text-err-ink"><CloudOff class="size-5" /></span>
	<div class="flex flex-col gap-1.5">
		<h1 class="text-[18px] font-semibold tracking-tight">Can't reach the control plane</h1>
		<p class="text-[13px] text-muted-foreground">{fleet.connError}</p>
	</div>
	<Button class="h-9 rounded-full px-4 text-[13px]" disabled={fleet.retrying} onclick={() => fleet.retry()}>
		{#if fleet.retrying}<LoaderCircle class="size-4 animate-spin" />{/if}Try again
	</Button>
	<p class="text-xs text-muted-foreground">Retrying on its own every few seconds.</p>
</div>
