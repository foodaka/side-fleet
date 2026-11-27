<script lang="ts">
	// Underlined tabs, as the control plane pages use them.
	let { tabs, value = $bindable(), label }: { tabs: { id: string; label: string }[]; value: string; label: string } = $props();
</script>

<div class="flex flex-wrap gap-1 border-b" role="tablist" aria-label={label}>
	{#each tabs as t (t.id)}
		<button
			type="button"
			role="tab"
			aria-selected={value === t.id}
			class="-mb-px h-9 border-b-2 px-3 text-[13px] transition-colors {value === t.id ? 'border-foreground font-medium text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground'}"
			onclick={() => (value = t.id)}
		>{t.label}</button>
	{/each}
</div>
