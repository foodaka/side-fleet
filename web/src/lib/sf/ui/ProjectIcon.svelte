<script lang="ts">
	// A project's (repo's) icon: the emoji, letter tile or picture the org set, else its default letter tile.
	import { cn } from '#lib/utils';
	import { projects, projectName, type ProjectIcon } from '../projects.svelte';

	let { repo, size = 18, icon: given, class: className = '' }: { repo: string; size?: number; icon?: ProjectIcon | null; class?: string } = $props();

	const icon = $derived(given ?? projects.icon(repo));
	const radius = $derived(Math.max(3, Math.round(size * 0.26)));
</script>

<span
	aria-hidden="true"
	class={cn('inline-grid shrink-0 place-items-center overflow-hidden leading-none select-none', icon.kind === 'emoji' && 'bg-sidebar-accent', className)}
	style="width: {size}px; height: {size}px; border-radius: {radius}px; {icon.kind === 'letter' ? `background: ${icon.value}; color: #fff;` : ''} font-size: {Math.round(size * (icon.kind === 'emoji' ? 0.68 : 0.58))}px"
>
	{#if icon.kind === 'image'}
		<img src={icon.value} alt="" class="size-full object-cover" />
	{:else if icon.kind === 'emoji'}
		{icon.value}
	{:else}
		<span class="font-semibold">{projectName(repo).slice(0, 1).toUpperCase()}</span>
	{/if}
</span>
