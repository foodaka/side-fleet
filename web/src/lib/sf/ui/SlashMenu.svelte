<script lang="ts">
	// The skills and slash commands Claude Code offers on the machine a message
	// goes to, shown while the message is just `/` and a word. The composer
	// passes its keys through `keydown` first so arrows, Enter and Tab pick here.
	import { matchCommands, slashCommands, slashQuery } from '#lib/sf/slash';
	import type { SlashCommand } from '#lib/sf/types';

	let {
		value = $bindable(),
		box,
		worker,
		repo,
		enabled = true,
		placement = 'below',
		id = 'slash-menu'
	}: {
		value: string;
		box?: HTMLTextAreaElement;
		/** The machine the message runs on; none, no menu. */
		worker?: string;
		repo: string;
		/** Only Claude Code's commands are listed. */
		enabled?: boolean;
		placement?: 'above' | 'below';
		id?: string;
	} = $props();

	let list = $state<SlashCommand[] | null>(null);
	let failed = $state(false);
	let active = $state(0);
	// Escape hides the menu until the text changes.
	let dismissed = $state<string | null>(null);
	let menu: HTMLDivElement | undefined = $state();
	// The machine and repo `list` is for.
	let shown = '';

	const query = $derived(enabled && worker ? slashQuery(value) : null);
	const open = $derived(query !== null && dismissed !== value);
	const matches = $derived(list && query !== null ? matchCommands(list, query).slice(0, 60) : []);

	$effect(() => {
		if (!open || !worker) return;
		const w = worker;
		const r = repo;
		let gone = false;
		if (`${w}\n${r}` !== shown) { shown = `${w}\n${r}`; list = null; }
		failed = false;
		slashCommands(w, r).then(
			(l) => { if (!gone) list = l; },
			() => { if (!gone) { failed = true; list = []; } }
		);
		return () => { gone = true; };
	});
	// A new query starts at the top.
	$effect(() => { void query; active = 0; });
	$effect(() => {
		void active;
		menu?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: 'nearest' });
	});

	function choose(c: SlashCommand) {
		value = `/${c.name} `;
		box?.focus();
		requestAnimationFrame(() => box?.setSelectionRange(value.length, value.length));
	}

	/** Handles the key when the menu is open; true when it did. */
	export function keydown(e: KeyboardEvent): boolean {
		if (!open || e.isComposing) return false;
		if (e.key === 'Escape') {
			dismissed = value;
			e.preventDefault();
			return true;
		}
		if (!matches.length) return false;
		if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			const step = e.key === 'ArrowDown' ? 1 : -1;
			active = (active + step + matches.length) % matches.length;
			e.preventDefault();
			return true;
		}
		if ((e.key === 'Enter' || e.key === 'Tab') && !e.shiftKey) {
			const c = matches[active];
			// Enter on a command already typed in full sends it.
			if (e.key === 'Enter' && c && value === `/${c.name}`) return false;
			if (c) choose(c);
			e.preventDefault();
			return true;
		}
		return false;
	}
</script>

{#if open}
	<div
		bind:this={menu}
		{id}
		role="listbox"
		aria-label="Skills and commands"
		class="absolute inset-x-0 z-20 max-h-72 overflow-y-auto overscroll-contain rounded-xl border bg-popover p-1 text-popover-foreground shadow-lg {placement === 'above' ? 'bottom-full mb-2' : 'top-full mt-2'}"
	>
		{#if list === null}
			<p class="px-3 py-2 text-[13px] text-muted-foreground">Loading skills…</p>
		{:else if failed}
			<p class="px-3 py-2 text-[13px] text-muted-foreground">Couldn't load this machine's skills.</p>
		{:else if !list.length}
			<p class="px-3 py-2 text-[13px] text-muted-foreground">This machine hasn't listed its skills yet. Update Sidefleet there to see them here.</p>
		{:else if !matches.length}
			<p class="px-3 py-2 text-[13px] text-muted-foreground">No skill or command matches <span class="font-mono text-foreground">/{query}</span>.</p>
		{:else}
			{#each matches as c, i (c.name)}
				<button
					type="button"
					role="option"
					tabindex="-1"
					aria-selected={i === active}
					data-index={i}
					onmousedown={(e) => e.preventDefault()}
					onmousemove={() => (active = i)}
					onclick={() => choose(c)}
					class="flex w-full items-baseline gap-3 rounded-lg px-3 py-1.5 text-left {i === active ? 'bg-accent text-accent-foreground' : ''}"
				>
					<span class="shrink-0 font-mono text-[13px]">/{c.name}{#if c.argument_hint}<span class="ml-1.5 text-faint">{c.argument_hint}</span>{/if}</span>
					{#if c.description}<span class="min-w-0 flex-1 truncate text-[12.5px] text-muted-foreground">{c.description}</span>{/if}
				</button>
			{/each}
		{/if}
	</div>
{/if}
