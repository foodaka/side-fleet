<script lang="ts">
	// An on/off switch. Label it with `aria-labelledby` or wrap it in a <label>.
	let { checked = false, onchange, id, ...rest }: {
		checked?: boolean;
		onchange: (v: boolean) => void;
		id?: string;
		'aria-labelledby'?: string;
		'aria-describedby'?: string;
	} = $props();
</script>

<button
	{id}
	type="button"
	role="switch"
	aria-checked={checked}
	{...rest}
	onclick={() => onchange(!checked)}
	class="relative inline-flex h-5.5 w-9.5 shrink-0 items-center rounded-full transition-colors duration-200 outline-none focus-visible:ring-3 focus-visible:ring-ring/50 {checked ? 'bg-primary' : 'bg-input dark:bg-input/80'}"
>
	<span class="size-4.5 rounded-full bg-background shadow-sm transition-transform duration-200 ease-out {checked ? 'translate-x-[18px]' : 'translate-x-0.5'}"></span>
</button>
