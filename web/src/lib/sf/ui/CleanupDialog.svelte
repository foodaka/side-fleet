<script lang="ts">
	import { untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { Button } from '#lib/components/ui/button';
	import * as Dialog from '#lib/components/ui/dialog';
	import { api } from '../api';
	import { fileSize, rel } from '../format';
	import { fleet } from '../state.svelte';
	import type { Worker, WorktreeCleanupStatus, WorktreeItem } from '../types';

	let { w, open = $bindable(false), view }: { w: Worker; open?: boolean; view: 'preview' | 'removed' } = $props();

	/** The machine walks every worktree to size it, which can take a while. */
	const WAIT_MS = 5 * 60 * 1000;
	const POLL_MS = 1500;
	const REASONS: Record<WorktreeItem['state'], string> = {
		merged: 'Pull request merged',
		on_default: 'Nothing beyond the default branch',
		in_use: 'In use',
		changes: 'Uncommitted files',
		unmerged: 'Unmerged commits',
		other_branch: 'On another branch',
		failed: 'Could not check'
	};

	let items = $state<WorktreeItem[] | null>(null);
	let at = $state<string | undefined>();
	let problem = $state('');
	let removing = $state(false);

	const url = $derived(`/v1/workers/${encodeURIComponent(w.id)}/clean-worktrees`);
	const removable = (i: WorktreeItem) => i.state === 'merged' || i.state === 'on_default';
	const bySize = (a: WorktreeItem, b: WorktreeItem) => b.bytes - a.bytes;
	const going = $derived((items ?? []).filter(removable).sort(bySize));
	const staying = $derived((items ?? []).filter((i) => !removable(i)).sort(bySize));
	const total = $derived(going.reduce((n, i) => n + i.bytes, 0));

	$effect(() => {
		if (!open) return;
		let stopped = false;
		untrack(() => {
			items = null;
			problem = '';
			void load(() => stopped);
		});
		return () => {
			stopped = true;
		};
	});

	async function load(stopped: () => boolean) {
		try {
			if (view === 'removed') {
				const s = await api<WorktreeCleanupStatus>(url);
				if (stopped()) return;
				items = s.cleanup?.items ?? [];
				at = s.cleanup?.at;
				return;
			}
			await api(url, { method: 'POST', body: JSON.stringify({ mode: 'preview' }) });
			const deadline = Date.now() + WAIT_MS;
			while (!stopped()) {
				const s = await api<WorktreeCleanupStatus>(url);
				if (stopped()) return;
				if (s.preview) {
					items = s.preview.items;
					return;
				}
				if (Date.now() > deadline) {
					problem = `${w.display_name} hasn't answered yet. Check that it's online, then try again.`;
					return;
				}
				await new Promise((r) => setTimeout(r, POLL_MS));
			}
		} catch (e) {
			if (!stopped()) problem = (e as Error).message;
		}
	}

	async function removeAll() {
		removing = true;
		try {
			await api(url, { method: 'POST', body: JSON.stringify({ mode: 'remove', ids: going.map((i) => i.root_id) }) });
			open = false;
			await fleet.refresh();
		} catch (e) {
			toast.error('Could not start the cleanup', { description: (e as Error).message });
		}
		removing = false;
	}
</script>

{#snippet rows(list: WorktreeItem[])}
	<ul class="divide-y">
		{#each list as i (i.root_id)}
			<li class="py-2">
				<div class="truncate text-[13px] font-medium" title={i.task ?? i.branch}>{i.task ?? i.branch}</div>
				<div class="flex items-center justify-between gap-3 text-xs text-muted-foreground">
					<span class="min-w-0 truncate">{REASONS[i.state]} · {i.repo}</span>
					<span class="tnum shrink-0">{fileSize(i.bytes)}</span>
				</div>
			</li>
		{/each}
	</ul>
{/snippet}

<Dialog.Root bind:open>
	<Dialog.Content class="flex max-h-[85dvh] flex-col gap-3 sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title>{view === 'removed' ? 'Removed worktrees' : `Clean up ${w.display_name}`}</Dialog.Title>
			<Dialog.Description>
				{#if view === 'removed'}
					{at ? `Cleaned up ${rel(at, fleet.now)}.` : ''} A follow-up on any of these tasks rebuilds its worktree.
				{:else}
					Files git ignores, like a local .env, go with the worktree. Branches stay, so a follow-up rebuilds it.
				{/if}
			</Dialog.Description>
		</Dialog.Header>

		<div class="min-h-0 flex-1 overflow-y-auto">
			{#if problem}
				<p class="py-6 text-center text-sm text-err-ink">{problem}</p>
			{:else if items === null}
				<p class="py-6 text-center text-sm text-muted-foreground">Checking every worktree on {w.display_name}…</p>
			{:else if view === 'removed'}
				{#if items.length}{@render rows(going)}{:else}<p class="py-6 text-center text-sm text-muted-foreground">Nothing was removed.</p>{/if}
			{:else}
				<h3 class="text-xs font-medium">Will remove ({going.length}) · up to {fileSize(total)}</h3>
				{#if going.length}{@render rows(going)}{:else}<p class="py-3 text-sm text-muted-foreground">Nothing is safe to remove right now.</p>{/if}
				{#if staying.length}
					<h3 class="mt-4 text-xs font-medium">Will keep ({staying.length})</h3>
					{@render rows(staying)}
				{/if}
			{/if}
		</div>

		{#if view === 'preview'}
			<Dialog.Footer>
				<Button variant="outline" onclick={() => (open = false)}>Cancel</Button>
				<Button variant="destructive" disabled={!going.length || removing} onclick={removeAll}>Remove {going.length} worktree{going.length === 1 ? '' : 's'}</Button>
			</Dialog.Footer>
		{/if}
	</Dialog.Content>
</Dialog.Root>
