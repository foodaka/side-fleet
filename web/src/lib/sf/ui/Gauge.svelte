<script lang="ts">
	import { cn } from '#lib/utils';
	let { label, value, percent }: { label: string; value: string; percent: number } = $props();
	const tone = $derived(percent >= 90 ? 'bg-err' : percent >= 75 ? 'bg-queue' : 'bg-foreground/80');
</script>

<div class="flex flex-col gap-1.5">
	<div class="flex items-baseline justify-between gap-3 text-xs">
		<span class="text-muted-foreground">{label}</span>
		<span class="tnum text-foreground/80">{value}</span>
	</div>
	<div class="h-1.5 overflow-hidden rounded-full bg-muted" role="meter" aria-label={label} aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100}>
		<div class={cn('h-full rounded-full transition-[width] duration-700', tone)} style="width:{Math.max(percent, 2)}%"></div>
	</div>
</div>
