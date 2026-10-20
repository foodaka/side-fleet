<script lang="ts">
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { Kbd } from '#lib/components/ui/kbd';
	import Search from '@lucide/svelte/icons/search';
	import { fleet } from '../state.svelte';
	import { toast } from 'svelte-sonner';
	import { untrack } from 'svelte';

	let { onsaved }: { onsaved?: (fresh: string[]) => void } = $props();

	let q = $state('');
	let sel = $state(new Set<string>());
	let order = $state<string[]>([]);
	let act = $state(0);
	let saving = $state(false);
	let list: HTMLDivElement | undefined = $state();
	let search: HTMLInputElement | undefined = $state();

	// Set up once per opening: freeze the order (chosen first) so rows don't jump while
	// ticking, and don't let the 5-second background refresh reset what you've typed or ticked.
	$effect(() => {
		if (!fleet.pickerOpen) return;
		untrack(() => {
			const repos = fleet.allRepos;
			const added = fleet.added ?? new Set();
			sel = new Set([...added].filter((n) => repos.has(n)));
			order = [...repos.keys()].sort((a, b) => Number(sel.has(b)) - Number(sel.has(a)) || a.localeCompare(b));
			q = '';
			act = 0;
		});
	});

	const terms = $derived(q.trim().toLowerCase().split(/\s+/).filter(Boolean));
	const shown = $derived(order.filter((n) => terms.every((t) => n.toLowerCase().includes(t))));
	const wasAdded = (n: string) => !!fleet.added?.has(n);
	const sections = $derived(!terms.length && order.some(wasAdded));

	function toggle(n: string) {
		const s = new Set(sel);
		s.has(n) ? s.delete(n) : s.add(n);
		sel = s;
	}
	function parts(name: string) {
		// split into [text, highlighted] runs for the search terms
		const lower = name.toLowerCase();
		const marks = new Array(name.length).fill(false);
		terms.forEach((t) => {
			const i = lower.indexOf(t);
			if (i >= 0) for (let k = i; k < i + t.length; k++) marks[k] = true;
		});
		const out: { t: string; m: boolean }[] = [];
		for (let i = 0; i < name.length; i++) {
			if (out.length && out[out.length - 1].m === marks[i]) out[out.length - 1].t += name[i];
			else out.push({ t: name[i], m: marks[i] });
		}
		return out;
	}
	function move(d: number) {
		if (!shown.length) return;
		act = (act + d + shown.length) % shown.length;
		list?.querySelector(`[data-i="${act}"]`)?.scrollIntoView({ block: 'nearest' });
	}
	function onkey(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') { e.preventDefault(); move(1); }
		else if (e.key === 'ArrowUp') { e.preventDefault(); move(-1); }
		else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); save(); }
		else if (e.key === 'Enter') { e.preventDefault(); if (shown[act]) toggle(shown[act]); }
	}
	async function save() {
		const before = fleet.added ?? new Set<string>();
		saving = true;
		try {
			const saved = await fleet.saveAdded([...sel]);
			fleet.pickerOpen = false;
			onsaved?.(saved.filter((n) => !before.has(n)));
		} catch (e) {
			toast.error('Could not save: ' + (e as Error).message);
		}
		saving = false;
	}
</script>

<Dialog.Root bind:open={fleet.pickerOpen}>
	<Dialog.Content
		class="flex max-h-[80vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-[34rem]"
		onOpenAutoFocus={(e) => { e.preventDefault(); search?.focus(); }}
	>
		<div class="flex flex-col gap-3 px-5 pt-5 pb-3">
			<Dialog.Header class="gap-1">
				<Dialog.Title>Choose your repos</Dialog.Title>
				<Dialog.Description>Only these appear in the repo menu when you start a task. Machines still report every checkout.</Dialog.Description>
			</Dialog.Header>
			<div class="relative">
				<Search class="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
				<input
					bind:this={search}
					class="h-10 w-full rounded-lg border border-input bg-background pr-3 pl-9 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
					placeholder="Search {fleet.allRepos.size} repos"
					bind:value={q}
					oninput={() => (act = 0)}
					onkeydown={onkey}
					autocomplete="off"
					spellcheck="false"
					aria-label="Search repos"
				/>
			</div>
			<div class="flex items-center justify-between text-xs text-muted-foreground">
				<span class="tnum">{sel.size} selected{terms.length ? ` · ${shown.length} match${shown.length === 1 ? '' : 'es'}` : ''}</span>
				<span class="flex gap-3">
					{#if terms.length && shown.length}<button type="button" class="underline-offset-2 hover:text-foreground hover:underline" onclick={() => (sel = new Set([...sel, ...shown]))}>Select matching</button>{/if}
					{#if sel.size}<button type="button" class="underline-offset-2 hover:text-foreground hover:underline" onclick={() => (sel = new Set())}>Clear</button>{/if}
				</span>
			</div>
		</div>
		<div bind:this={list} class="min-h-0 flex-1 overflow-y-auto border-t p-1.5" role="listbox" aria-multiselectable="true">
			{#each shown as n, i (n)}
				{#if sections && i === 0}<div class="px-2.5 pt-2 pb-1 text-xs font-medium text-muted-foreground">Chosen</div>{/if}
				{#if sections && !wasAdded(n) && (i === 0 || wasAdded(shown[i - 1]))}<div class="px-2.5 pt-3 pb-1 text-xs font-medium text-muted-foreground">Everything else</div>{/if}
				{@const where = (fleet.allRepos.get(n) ?? []).filter((w) => w.online).map((w) => w.display_name)}
				<div
					data-i={i}
					role="option"
					aria-selected={sel.has(n)}
					tabindex="-1"
					class="flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 select-none {i === act ? 'bg-accent' : ''}"
					onmousemove={() => (act = i)}
					onclick={() => toggle(n)}
					onkeydown={() => {}}
				>
					<input type="checkbox" tabindex="-1" checked={sel.has(n)} class="size-4 accent-foreground" />
					<span class="min-w-0 truncate font-mono text-[12.5px]">{#each parts(n) as p}{#if p.m}<mark class="rounded-sm bg-queue-soft text-inherit">{p.t}</mark>{:else}{p.t}{/if}{/each}</span>
					<span class="ml-auto shrink-0 text-xs text-faint">{where.length ? where.join(', ') : 'offline'}</span>
				</div>
			{:else}
				<p class="p-6 text-center text-sm text-muted-foreground">{order.length ? `No repo matches “${q}”.` : 'No machine has reported any git checkouts yet.'}</p>
			{/each}
		</div>
		<div class="flex items-center justify-between gap-3 border-t bg-muted/40 px-4 py-3">
			<span class="hidden items-center gap-1.5 text-xs text-muted-foreground sm:flex"><Kbd>↑</Kbd><Kbd>↓</Kbd> move <Kbd>↵</Kbd> select <Kbd>⌘↵</Kbd> save</span>
			<span class="ml-auto flex gap-2">
				<Button variant="outline" class="rounded-full" onclick={() => (fleet.pickerOpen = false)}>Cancel</Button>
				<Button class="rounded-full" onclick={save} disabled={saving}>Save{sel.size ? ` ${sel.size}` : ''}</Button>
			</span>
		</div>
	</Dialog.Content>
</Dialog.Root>
