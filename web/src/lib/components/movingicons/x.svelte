<script lang="ts">
	// Adapted from movingicons.dev (MIT, @jis3r), the Svelte port of lucide-animated.
	// Animates while an ancestor with the `mi-hover` class is hovered, or when `animate` is set.
	let { size = 16, animate = false, class: className = "" }: { size?: number; animate?: boolean; class?: string } = $props();
</script>

<span class="mi {className}" aria-hidden="true">
	<svg xmlns="http://www.w3.org/2000/svg" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class:animate>
		<path d="M18 6 6 18" class="a" />
		<path d="m6 6 12 12" class="b" />
	</svg>
</span>

<style>
	.mi { display: inline-flex; flex-shrink: 0; }
	svg { overflow: visible; }
	@media (prefers-reduced-motion: reduce) { svg, svg * { animation: none !important; transition: none !important; } }
	.a, .b { stroke-dasharray: 17; }
	.animate .a, :global(.mi-hover:hover) .a { animation: draw 0.3s ease-out both; }
	.animate .b, :global(.mi-hover:hover) .b { animation: draw 0.3s ease-out 0.2s both; }
	@keyframes draw { 0%, 15% { opacity: 0; stroke-dashoffset: 17; } 16% { opacity: 1; } 100% { opacity: 1; stroke-dashoffset: 0; } }
</style>
