<script lang="ts">
	// Adapted from movingicons.dev (MIT, @jis3r), the Svelte port of lucide-animated.
	// Animates while an ancestor with the `mi-hover` class is hovered, or when `animate` is set.
	let { size = 16, animate = false, class: className = "" }: { size?: number; animate?: boolean; class?: string } = $props();
</script>

<span class="mi {className}" aria-hidden="true">
	<svg xmlns="http://www.w3.org/2000/svg" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class:animate>
		<path d="M5 12h14" class="h" />
		<path d="M12 5v14" class="v" />
	</svg>
</span>

<style>
	.mi { display: inline-flex; flex-shrink: 0; }
	svg { overflow: visible; }
	@media (prefers-reduced-motion: reduce) { svg, svg * { animation: none !important; transition: none !important; } }
	.h, .v { stroke-dasharray: 14; }
	.animate .h, :global(.mi-hover:hover) .h { animation: draw 0.3s ease-out both; }
	.animate .v, :global(.mi-hover:hover) .v { animation: draw 0.3s ease-out 0.2s both; }
	@keyframes draw { 0%, 15% { opacity: 0; stroke-dashoffset: 14; } 16% { opacity: 1; } 100% { opacity: 1; stroke-dashoffset: 0; } }
</style>
