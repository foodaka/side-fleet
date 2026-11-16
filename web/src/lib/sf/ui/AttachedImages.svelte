<script lang="ts">
	// What is about to be sent: thumbnails of images and chips of files, each with a remove button.
	import X from '@lucide/svelte/icons/x';
	import FileText from '@lucide/svelte/icons/file-text';
	import { fileSize, type Attachments } from '../attach.svelte';

	let { attachments, class: className = '' }: { attachments: Attachments; class?: string } = $props();
</script>

{#if attachments.items.length}
	<ul class="flex flex-wrap items-center gap-2 {className}" aria-label="Attachments">
		{#each attachments.items as a (a.key)}
			<li class="group relative">
				{#if a.kind === 'image'}
					<img src={a.url} alt={a.file.name || 'Attached image'} title={a.file.name} class="size-16 rounded-lg border object-cover" />
				{:else}
					<div title={a.file.name} class="flex h-16 w-44 flex-col justify-center gap-1 rounded-lg border bg-muted/40 px-2.5">
						<span class="flex min-w-0 items-center gap-1.5 text-[12.5px]"><FileText class="size-3.5 shrink-0 text-muted-foreground" /><span class="truncate">{a.file.name || 'file'}</span></span>
						<span class="text-[11px] text-muted-foreground">{fileSize(a.file.size)}</span>
					</div>
				{/if}
				<button
					type="button"
					aria-label="Remove {a.file.name || a.kind}"
					onclick={() => attachments.remove(a.key)}
					class="absolute -top-1.5 -right-1.5 grid size-5 place-items-center rounded-full border bg-background text-muted-foreground shadow-sm hover:text-foreground"
				>
					<X class="size-3" strokeWidth={2.4} />
				</button>
			</li>
		{/each}
	</ul>
{/if}
