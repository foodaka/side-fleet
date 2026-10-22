<script lang="ts">
	// Adapted from movingicons.dev (MIT, @jis3r), the Svelte port of lucide-animated.
	// Animates while an ancestor with the `mi-hover` class is hovered, or when `animate` is set.
	let { size = 16, animate = false, class: className = "" }: { size?: number; animate?: boolean; class?: string } = $props();
</script>

<span class="mi {className}" aria-hidden="true">
	<svg xmlns="http://www.w3.org/2000/svg" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class:animate>
		<rect width="18" height="18" x="3" y="3" rx="2" />
		<path d="M9 3v18" class="line" />
		<path d="m14 9 3 3-3 3" class="chevron" />
	</svg>
</span>

<style>
	.mi { display: inline-flex; flex-shrink: 0; }
	svg { overflow: visible; }
	@media (prefers-reduced-motion: reduce) { svg, svg * { animation: none !important; transition: none !important; } }
	path { transition: transform 0.2s ease-in; }
	.animate .line, :global(.mi-hover:hover) .line { transform: translateX(2px); transition-delay: 0.05s; }
	.animate .chevron, :global(.mi-hover:hover) .chevron { transform: translateX(2px); }
</style>
