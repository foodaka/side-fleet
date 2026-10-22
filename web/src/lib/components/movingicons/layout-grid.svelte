<script lang="ts">
	// Adapted from movingicons.dev (MIT, @jis3r), the Svelte port of lucide-animated.
	// Animates while an ancestor with the `mi-hover` class is hovered, or when `animate` is set.
	let { size = 16, animate = false, class: className = "" }: { size?: number; animate?: boolean; class?: string } = $props();
</script>

<span class="mi {className}" aria-hidden="true">
	<svg xmlns="http://www.w3.org/2000/svg" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class:animate>
		<rect width="7" height="7" x="3" y="3" rx="1" class="tl" />
		<rect width="7" height="7" x="14" y="3" rx="1" class="tr" />
		<rect width="7" height="7" x="14" y="14" rx="1" class="br" />
		<rect width="7" height="7" x="3" y="14" rx="1" class="bl" />
	</svg>
</span>

<style>
	.mi { display: inline-flex; flex-shrink: 0; }
	svg { overflow: visible; }
	@media (prefers-reduced-motion: reduce) { svg, svg * { animation: none !important; transition: none !important; } }
	.animate .tr, :global(.mi-hover:hover) .tr { animation: from-top 0.35s ease-out both; }
	.animate .br, :global(.mi-hover:hover) .br { animation: from-right 0.35s ease-out 0.1s both; }
	.animate .bl, :global(.mi-hover:hover) .bl { animation: from-bottom 0.35s ease-out 0.2s both; }
	.animate .tl, :global(.mi-hover:hover) .tl { animation: from-left 0.35s ease-out 0.3s both; }
	@keyframes from-left { 0%, 50% { opacity: 0; transform: translateX(-10px); } 80% { opacity: 0.8; transform: translateX(2px); } 100% { opacity: 1; transform: none; } }
	@keyframes from-right { 0%, 50% { opacity: 0; transform: translateX(10px); } 80% { opacity: 0.8; transform: translateX(-2px); } 100% { opacity: 1; transform: none; } }
	@keyframes from-top { 0%, 50% { opacity: 0; transform: translateY(-10px); } 80% { opacity: 0.8; transform: translateY(2px); } 100% { opacity: 1; transform: none; } }
	@keyframes from-bottom { 0%, 50% { opacity: 0; transform: translateY(10px); } 80% { opacity: 0.8; transform: translateY(-2px); } 100% { opacity: 1; transform: none; } }
</style>
