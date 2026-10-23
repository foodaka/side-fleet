<script lang="ts" module>
	import { authHeaders } from '../api';

	// Object URLs by image id: images never change, so each is fetched once per page.
	const cache = new Map<string, Promise<string>>();

	/** `<img src>` cannot send the key or org header, so fetch the bytes and show them from a blob URL. */
	function load(id: string): Promise<string> {
		let p = cache.get(id);
		if (!p) {
			p = fetch(`/v1/images/${id}`, { headers: authHeaders(), credentials: 'same-origin' }).then(async (r) => {
				if (!r.ok) throw new Error(r.status === 404 ? 'Image not found' : `Could not load the image (${r.status})`);
				return URL.createObjectURL(await r.blob());
			});
			p.catch(() => cache.delete(id));
			cache.set(id, p);
		}
		return p;
	}
</script>

<script lang="ts">
	// Images a tool returned (a screenshot the agent read), shown under its line in the narration,
	// and images the user attached to a message.
	import * as Dialog from '#lib/components/ui/dialog';
	import ImageOff from '@lucide/svelte/icons/image-off';
	import type { ImageRef } from '../thread.svelte';

	/** `inset` lines the images up under a tool row's text. */
	let { images, inset = true, alt = 'Returned by the tool' }: { images: ImageRef[]; inset?: boolean; alt?: string } = $props();
	let open = $state<string | null>(null);
</script>

<div class="mt-1 mb-2 flex flex-wrap gap-2 {inset ? 'ml-7.5' : ''}">
	{#each images as img (img.id)}
		{#await load(img.id)}
			<div class="h-40 w-56 animate-pulse rounded-lg bg-muted"></div>
		{:then src}
			<button type="button" class="overflow-hidden rounded-lg border bg-code transition-opacity hover:opacity-90" onclick={() => (open = src)} aria-label="Open the image full size">
				<img {src} {alt} class="block max-h-72 w-auto max-w-full object-contain" />
			</button>
		{:catch err}
			<p class="inline-flex items-center gap-1.5 text-xs text-muted-foreground"><ImageOff class="size-3.5" />{err.message}</p>
		{/await}
	{/each}
</div>

<Dialog.Root open={open !== null} onOpenChange={(o) => { if (!o) open = null; }}>
	<Dialog.Content class="max-h-[95dvh] w-auto max-w-[95vw] overflow-auto p-2 sm:max-w-[95vw]">
		<Dialog.Title class="sr-only">Image</Dialog.Title>
		{#if open}
			<a href={open} target="_blank" rel="noopener"><img src={open} alt="{alt}, full size" class="block h-auto max-w-full" /></a>
		{/if}
	</Dialog.Content>
</Dialog.Root>
