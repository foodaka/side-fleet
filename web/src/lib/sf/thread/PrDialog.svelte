<script lang="ts">
	// Before opening a pull request or pushing to one: which changed files go in, and for a
	// new one its title and description (or the agent writes them) and whether it's a draft.
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import Sparkles from '@lucide/svelte/icons/sparkles';
	import type { DiffFile } from '../diff/parse';
	import type { PrOptions } from '../types';
	import DiffStat from '../diff/DiffStat.svelte';

	let {
		open = $bindable(false),
		mode,
		number,
		files,
		fileCount,
		branch,
		machine,
		agent,
		canManage,
		onsubmit
	}: {
		open: boolean;
		/** Open a new pull request, or push to the open one (`number`). */
		mode: 'open' | 'push';
		number?: number;
		/** The files the thread changed, as the review shows them. */
		files: DiffFile[];
		/** Every changed file, which can be more than `files` lists. */
		fileCount: number;
		branch: string;
		machine: string;
		/** Who writes the title and description: the thread's agent, by name. */
		agent: string;
		/** The machine's Sidefleet can leave files out, write it and open drafts. */
		canManage: boolean;
		onsubmit: (opts: PrOptions) => Promise<void>;
	} = $props();

	let picked = $state<Set<string>>(new Set());
	let title = $state('');
	let body = $state('');
	let draft = $state(false);
	let busy = $state(false);

	// Every file ticked, fields empty, each time it opens.
	$effect(() => {
		if (!open) return;
		picked = new Set(files.map((f) => f.path));
		title = '';
		body = '';
		draft = false;
		busy = false;
	});

	const exclude = $derived(files.map((f) => f.path).filter((p) => !picked.has(p)));
	const unlisted = $derived(Math.max(0, fileCount - files.length));
	function toggle(path: string) {
		const next = new Set(picked);
		if (next.has(path)) next.delete(path);
		else next.add(path);
		picked = next;
	}
	function all(on: boolean) {
		picked = new Set(on ? files.map((f) => f.path) : []);
	}

	async function submit(e: Event) {
		e.preventDefault();
		busy = true;
		const opts: PrOptions = {};
		if (exclude.length) opts.exclude = exclude;
		if (mode === 'open') {
			if (title.trim()) opts.title = title.trim();
			if (body.trim()) opts.body = body.trim();
			if (draft) opts.draft = true;
		}
		try {
			await onsubmit(opts);
			open = false;
		} finally {
			busy = false;
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-[36rem]">
		<Dialog.Header>
			<Dialog.Title>{mode === 'open' ? 'Open a pull request' : `Push changes to #${number}`}</Dialog.Title>
			<Dialog.Description>
				Commits the ticked files on <span class="font-mono text-[12px]">{branch}</span> and pushes it with the GitHub CLI on {machine}. Files you untick stay in the worktree, uncommitted.
			</Dialog.Description>
		</Dialog.Header>
		<form class="flex flex-col gap-3" onsubmit={submit}>
			{#if files.length}
				<fieldset class="overflow-hidden rounded-xl border" disabled={!canManage}>
					<legend class="sr-only">Files to commit</legend>
					<div class="flex items-center gap-2 border-b bg-muted/40 px-3 py-1.5 text-xs text-muted-foreground">
						<span class="tnum flex-1">{picked.size} of {files.length} file{files.length === 1 ? '' : 's'}</span>
						{#if canManage}
							<button type="button" class="hover:text-foreground" onclick={() => all(true)}>All</button>
							<button type="button" class="hover:text-foreground" onclick={() => all(false)}>None</button>
						{/if}
					</div>
					<ul class="max-h-60 overflow-y-auto py-1">
						{#each files as f (f.path)}
							<li>
								<label class="flex cursor-pointer items-center gap-2.5 px-3 py-1 text-[12.5px] hover:bg-muted/50 has-[:disabled]:cursor-default">
									<input type="checkbox" checked={picked.has(f.path)} onchange={() => toggle(f.path)} class="size-3.5 shrink-0 accent-foreground" />
									<span class="min-w-0 flex-1 truncate font-mono {picked.has(f.path) ? '' : 'text-muted-foreground line-through decoration-muted-foreground/50'}" title={f.path}>{f.path}</span>
									<DiffStat add={f.additions} del={f.deletions} blocks={false} class="shrink-0 text-[11px]" />
								</label>
							</li>
						{/each}
					</ul>
				</fieldset>
				<p class="-mt-1 text-xs text-muted-foreground">
					{#if !canManage}
						Update Sidefleet on {machine} to leave files out; everything changed goes in.
					{:else}
						{unlisted ? `${unlisted} more changed file${unlisted === 1 ? '' : 's'} not listed here go in too. ` : ''}Files already pushed to the branch stay in it.
					{/if}
				</p>
			{/if}
			{#if mode === 'open'}
				<label for="pr-title" class="sr-only">Title</label>
				<input
					id="pr-title"
					bind:value={title}
					maxlength="300"
					placeholder={canManage ? `Title: leave empty and ${agent} writes it` : 'Title: leave empty to use the task'}
					class="h-10 w-full rounded-xl border border-input bg-card px-3.5 text-[14px] outline-none placeholder:text-faint focus:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]"
				/>
				<label for="pr-body" class="sr-only">Description</label>
				<textarea
					id="pr-body"
					bind:value={body}
					rows="4"
					maxlength="20000"
					placeholder={canManage && !title.trim() ? `Description: leave empty and ${agent} writes it from the changes` : 'Description (optional)'}
					onkeydown={(e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) submit(e); }}
					class="w-full resize-none rounded-xl border border-input bg-card px-3.5 py-3 text-[14px] leading-relaxed outline-none placeholder:text-faint focus:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]"
				></textarea>
			{/if}
			<div class="flex flex-wrap items-center gap-3">
				{#if mode === 'open'}
					<label class="flex items-center gap-2 text-xs text-muted-foreground has-[:disabled]:opacity-60" title={canManage ? '' : `Update Sidefleet on ${machine} to open drafts`}>
						<input type="checkbox" bind:checked={draft} disabled={!canManage} class="size-3.5 accent-foreground" />Open as a draft
					</label>
				{/if}
				<p class="min-w-0 flex-1 text-xs text-muted-foreground">
					{#if mode === 'open' && canManage && !title.trim()}
						<Sparkles class="mr-0.5 inline size-3 -translate-y-px" />{agent} writes the title{body.trim() ? '' : ' and description'} from the changes.
					{/if}
				</p>
				<Button type="button" variant="ghost" size="sm" class="rounded-full" onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" size="sm" class="rounded-full" disabled={busy || (!!files.length && picked.size === 0 && !unlisted && mode === 'open')}>
					<GitPullRequest class="size-3.5" />{busy ? 'Starting…' : mode === 'open' ? 'Open pull request' : 'Push changes'}
				</Button>
			</div>
		</form>
	</Dialog.Content>
</Dialog.Root>
