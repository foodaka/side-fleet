<script lang="ts">
	// Thumbnails of the images about to be sent, each with a remove button.
	import X from '@lucide/svelte/icons/x';
	import type { Attachments } from '../attach.svelte';

	let { attachments, class: className = '' }: { attachments: Attachments; class?: string } = $props();
</script>

{#if attachments.items.length}
	<ul class="flex flex-wrap gap-2 {className}" aria-label="Attached images">
		{#each attachments.items as a (a.key)}
			<li class="group relative">
				<img src={a.url} alt={a.file.name || 'Attached image'} title={a.file.name} class="size-16 rounded-lg border object-cover" />
				<button
					type="button"
					aria-label="Remove {a.file.name || 'image'}"
					onclick={() => attachments.remove(a.key)}
					class="absolute -top-1.5 -right-1.5 grid size-5 place-items-center rounded-full border bg-background text-muted-foreground shadow-sm hover:text-foreground"
				>
					<X class="size-3" strokeWidth={2.4} />
				</button>
			</li>
		{/each}
	</ul>
{/if}
