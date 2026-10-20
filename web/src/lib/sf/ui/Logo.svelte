<script lang="ts">
	let { size = 20 }: { size?: number } = $props();
</script>

<!-- Three craft in formation: a fleet. -->
<svg width={size} height={size} viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
	<circle cx="5" cy="10" r="3.2" />
	<circle cx="14" cy="5.5" r="3.2" />
	<circle cx="14" cy="14.5" r="3.2" />
</svg>
