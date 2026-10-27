<script lang="ts">
	// A teammate's GitHub picture, with their initial while it loads or if it can't.
	import { cn } from '#lib/utils';

	let { login, size = 20, class: className = '', title }: { login: string | null | undefined; size?: number; class?: string; title?: string } = $props();
	let failed = $state(false);
	$effect(() => {
		void login;
		failed = false;
	});
	const initial = $derived((login ?? '?').slice(0, 1).toUpperCase());
</script>

<span
	class={cn('relative inline-grid shrink-0 place-items-center overflow-hidden rounded-full bg-foreground/10 font-semibold text-foreground/70 ring-1 ring-background', className)}
	style="width: {size}px; height: {size}px; font-size: {Math.max(9, Math.round(size * 0.45))}px"
	title={title ?? (login ? `@${login}` : undefined)}
>
	{initial}
	{#if login && !failed}
		<img
			src="https://github.com/{encodeURIComponent(login)}.png?size={size * 2 > 64 ? 128 : 64}"
			alt=""
			loading="lazy"
			class="absolute inset-0 size-full object-cover"
			onerror={() => (failed = true)}
		/>
	{/if}
</span>
