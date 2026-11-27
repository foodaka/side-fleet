<script lang="ts">
	// Filter pills with counts: one is chosen at a time.
	let { options, value = $bindable(), label }: { options: { id: string; label: string; n?: number }[]; value: string; label: string } = $props();
</script>

<div class="flex flex-wrap items-center gap-1.5" role="group" aria-label={label}>
	{#each options as o (o.id)}
		<button
			type="button"
			aria-pressed={value === o.id}
			class="inline-flex h-7 items-center gap-1.5 rounded-full border px-3 text-[12.5px] transition-colors {value === o.id ? 'border-primary bg-primary text-primary-foreground' : 'bg-card text-muted-foreground hover:text-foreground'}"
			onclick={() => (value = o.id)}
		>{o.label}{#if o.n !== undefined}<span class="tnum opacity-70">{o.n}</span>{/if}</button>
	{/each}
</div>
