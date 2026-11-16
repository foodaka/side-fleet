<script lang="ts">
	// Files the user attached to a message (markdown, text, code, PDFs), as chips that download them.
	import FileText from '@lucide/svelte/icons/file-text';
	import { toast } from 'svelte-sonner';
	import { authHeaders } from '../api';
	import { fileSize } from '../attach.svelte';
	import type { FileRef } from '../thread.svelte';

	let { files, class: className = '' }: { files: FileRef[]; class?: string } = $props();

	/** A link cannot send the key or org header, so fetch the bytes and save them from a blob.
	 * Always as a download: the control plane serves them as octet-stream, never to be shown here. */
	async function download(f: FileRef) {
		try {
			const r = await fetch(`/v1/files/${f.id}`, { headers: authHeaders(), credentials: 'same-origin' });
			if (!r.ok) throw new Error(r.status === 404 ? 'File not found' : `Could not download it (${r.status})`);
			const url = URL.createObjectURL(new Blob([await r.arrayBuffer()], { type: 'application/octet-stream' }));
			const a = document.createElement('a');
			a.href = url;
			a.download = f.name;
			a.click();
			setTimeout(() => URL.revokeObjectURL(url), 1000);
		} catch (err) {
			toast.error(`Could not download ${f.name}`, { description: (err as Error).message });
		}
	}
</script>

<ul class="flex flex-wrap gap-2 {className}" aria-label="Attached files">
	{#each files as f (f.id)}
		<li>
			<button type="button" onclick={() => download(f)} title="Download {f.name}" class="flex max-w-64 items-center gap-2 rounded-lg border bg-card px-2.5 py-1.5 text-left text-[12.5px] transition-colors hover:bg-accent">
				<FileText class="size-4 shrink-0 text-muted-foreground" />
				<span class="min-w-0 truncate">{f.name}</span>
				<span class="shrink-0 text-[11px] text-muted-foreground">{fileSize(f.bytes)}</span>
			</button>
		</li>
	{/each}
</ul>
