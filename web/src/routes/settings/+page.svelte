<script lang="ts">
	// How the app behaves for you: what new tasks start with, which key sends, how changes
	// are shown, what asks before it acts. Kept in this browser; every change applies at once.
	import { setMode, userPrefersMode } from 'mode-watcher';
	import { Kbd } from '#lib/components/ui/kbd';
	import Check from '@lucide/svelte/icons/check';
	import Rows from '@lucide/svelte/icons/rows-2';
	import Columns from '@lucide/svelte/icons/columns-2';
	import Monitor from '@lucide/svelte/icons/monitor';
	import Sun from '@lucide/svelte/icons/sun';
	import Moon from '@lucide/svelte/icons/moon';
	import * as Select from '#lib/components/ui/select';
	import Switch from '#lib/sf/ui/Switch.svelte';
	import { PERMISSIONS, WORKSPACES } from '#lib/sf/format';
	import { isMac, prefs, type SendKey, type Workspace } from '#lib/sf/prefs.svelte';

	const mod = isMac ? '⌘' : 'Ctrl';
	const sendOptions: { id: SendKey; name: string; tag?: string; task: string[]; reply: string[]; hint: string }[] = [
		{ id: 'smart', name: 'Smart', tag: 'Default', task: [mod, '↵'], reply: ['↵'], hint: `Replies send on Enter. A new task needs ${mod}+Enter, so a long brief can’t start halfway. On a phone, Return adds a line.` },
		{ id: 'enter', name: 'Enter', task: ['↵'], reply: ['↵'], hint: 'Enter sends everywhere. Shift+Enter adds a line.' },
		{ id: 'mod-enter', name: `${mod} Enter`, task: [mod, '↵'], reply: [mod, '↵'], hint: `Enter always adds a line. ${mod}+Enter sends.` }
	];
	const themes = [
		{ id: 'system', label: 'System', icon: Monitor },
		{ id: 'light', label: 'Light', icon: Sun },
		{ id: 'dark', label: 'Dark', icon: Moon }
	] as const;

	// A box to try the send key in.
	let tryAs = $state<'task' | 'reply'>('reply');
	let tryText = $state('');
	let sent = $state<string | null>(null);
	let sentTimer: ReturnType<typeof setTimeout>;
	function tryKey(e: KeyboardEvent) {
		if (!prefs.sends(e, tryAs)) return;
		e.preventDefault();
		sent = tryText.trim() ? 'Sent' : 'Sent (it was empty)';
		tryText = '';
		clearTimeout(sentTimer);
		sentTimer = setTimeout(() => (sent = null), 1600);
	}
</script>

<svelte:head><title>Settings · Sidefleet</title></svelte:head>

{#snippet keys(ks: string[])}
	<span class="inline-flex items-center gap-0.5">{#each ks as k (k)}<Kbd>{k}</Kbd>{/each}</span>
{/snippet}

{#snippet segmented(label: string, options: readonly { id: string; label: string; icon: typeof Rows }[], value: string, pick: (v: string) => void)}
	<div class="flex shrink-0 rounded-lg bg-muted p-0.5" role="radiogroup" aria-label={label}>
		{#each options as o (o.id)}
			<button
				type="button"
				role="radio"
				aria-checked={value === o.id}
				class="flex h-7 items-center gap-1.5 rounded-md px-2.5 text-[12.5px] transition-colors {value === o.id ? 'bg-card font-medium text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}"
				onclick={() => pick(o.id)}
			>
				<o.icon class="size-3.5" />{o.label}
			</button>
		{/each}
	</div>
{/snippet}

{#snippet choice(id: string, title: string, hint: string, value: string, options: readonly { id: string; name: string; hint: string }[], pick: (v: string) => void)}
	{@const current = options.find((o) => o.id === value)}
	<div class="flex flex-wrap items-center justify-between gap-x-6 gap-y-2.5 px-4 py-3.5 sm:px-5">
		<div class="min-w-0 flex-1 basis-64">
			<p id="{id}-label" class="text-[13.5px] font-medium">{title}</p>
			<p class="mt-0.5 text-[12.5px] leading-relaxed text-muted-foreground">{hint}</p>
		</div>
		<Select.Root type="single" {value} onValueChange={(v) => v && pick(v)}>
			<Select.Trigger class="h-8 w-48 shrink-0 rounded-lg text-[13px]" aria-labelledby="{id}-label">{current?.name}</Select.Trigger>
			<Select.Content align="end" class="w-80">
				{#each options as o (o.id)}
					<Select.Item value={o.id} label={o.name}><span class="flex flex-col"><span>{o.name}</span><span class="text-xs text-muted-foreground">{o.hint}</span></span></Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
	</div>
{/snippet}

{#snippet row(id: string, title: string, hint: string, checked: boolean, onchange: (v: boolean) => void)}
	<div class="flex items-start justify-between gap-6 px-4 py-3.5 sm:px-5">
		<div class="min-w-0">
			<p id="{id}-label" class="text-[13.5px] font-medium">{title}</p>
			<p id="{id}-hint" class="mt-0.5 text-[12.5px] leading-relaxed text-muted-foreground">{hint}</p>
		</div>
		<Switch {checked} {onchange} aria-labelledby="{id}-label" aria-describedby="{id}-hint" />
	</div>
{/snippet}

<section class="mx-auto flex w-full max-w-2xl flex-col gap-8 px-4 py-8 md:px-8">
	<header>
		<h1 class="text-[22px] font-semibold tracking-tight">Settings</h1>
		<p class="mt-1 text-[13px] text-muted-foreground">How Sidefleet behaves for you. Changes apply right away and are kept in this browser.</p>
	</header>

	<!-- New tasks -->
	<div class="flex flex-col gap-3">
		<div>
			<h2 class="text-[15px] font-medium">New tasks</h2>
			<p class="mt-0.5 text-[12.5px] text-muted-foreground">What every new task starts with, wherever you start it. The composer’s options menu can change them for one task without changing these.</p>
		</div>
		<div class="divide-y rounded-xl border bg-card">
			{@render choice('perm', 'Permissions', 'For tasks you start from Home, and forks of a message that has no mode of its own.', prefs.permission, PERMISSIONS, (v) => prefs.set({ permission: v }))}
			{@render choice('room-perm', 'Permissions in rooms', 'Room tasks read your teammates’ notes, so a note could steer what they do. Full access lets that reach commands on your machine.', prefs.roomPermission, PERMISSIONS, (v) => prefs.set({ roomPermission: v }))}
			{@render choice('iso', 'Workspace', 'Own branch lets tasks run side by side. In place works in the checkout, one task per repo at a time.', prefs.workspace, WORKSPACES, (v) => prefs.set({ workspace: v as Workspace }))}
		</div>
		<p class="text-[12px] text-muted-foreground">Ask me works with Claude Code only; other agents start with Read &amp; edit files instead. A reply keeps the mode its task was using, and a fork keeps its message’s.</p>
	</div>

	<!-- Sending -->
	<div class="flex flex-col gap-3">
		<div>
			<h2 class="text-[15px] font-medium">Sending messages</h2>
			<p class="mt-0.5 text-[12.5px] text-muted-foreground">Which key sends a new task or a reply. {mod}+Enter always sends.</p>
		</div>
		<div class="grid gap-2 sm:grid-cols-3" role="radiogroup" aria-label="Send key">
			{#each sendOptions as o (o.id)}
				{@const on = prefs.sendKey === o.id}
				<button
					type="button"
					role="radio"
					aria-checked={on}
					onclick={() => prefs.set({ sendKey: o.id })}
					class="group relative flex flex-col gap-3 rounded-xl border bg-card p-3.5 text-left transition-[border-color,box-shadow] {on ? 'border-foreground/25 shadow-[0_0_0_3px_color-mix(in_oklab,var(--ring)_10%,transparent)]' : 'hover:border-foreground/15'}"
				>
					<span class="flex items-center gap-2">
						<span class="grid size-4 place-items-center rounded-full border transition-colors {on ? 'border-primary bg-primary text-primary-foreground' : 'border-input'}">
							{#if on}<Check class="size-2.5" strokeWidth={3} />{/if}
						</span>
						<span class="text-[13.5px] font-medium">{o.name}</span>
						{#if o.tag}<span class="ml-auto rounded-full bg-muted px-1.5 py-px text-[10.5px] text-muted-foreground">{o.tag}</span>{/if}
					</span>
					<span class="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-1.5 text-[12px] text-muted-foreground">
						<span>New task</span><span class="justify-self-end">{@render keys(o.task)}</span>
						<span>Reply</span><span class="justify-self-end">{@render keys(o.reply)}</span>
					</span>
					<span class="text-[12px] leading-relaxed text-muted-foreground">{o.hint}</span>
				</button>
			{/each}
		</div>

		<div class="rounded-xl border bg-card">
			<div class="flex items-center gap-2 border-b px-3.5 py-2">
				<span class="text-[12.5px] font-medium">Try it</span>
				<span class="text-[12px] text-muted-foreground">as a</span>
				<div class="flex rounded-md bg-muted p-0.5" role="radiogroup" aria-label="Try as">
					{#each [{ id: 'reply', label: 'reply' }, { id: 'task', label: 'new task' }] as t (t.id)}
						<button type="button" role="radio" aria-checked={tryAs === t.id} onclick={() => (tryAs = t.id as typeof tryAs)} class="h-6 rounded px-2 text-[12px] transition-colors {tryAs === t.id ? 'bg-card font-medium text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}">{t.label}</button>
					{/each}
				</div>
				<span class="ml-auto flex items-center gap-1.5 text-[12px] text-muted-foreground" aria-live="polite">
					{#if sent}
						<span class="flex items-center gap-1 text-ok-ink"><Check class="size-3.5" />{sent}</span>
					{:else}
						<span class="pointer-coarse:hidden">sends on</span><span class="pointer-coarse:hidden">{@render keys(prefs.sendKeys(tryAs))}</span>
					{/if}
				</span>
			</div>
			<textarea
				bind:value={tryText}
				onkeydown={tryKey}
				rows="2"
				aria-label="Try the send key"
				placeholder="Type something, then press the key you’d send with. Nothing is sent anywhere."
				class="block w-full resize-none bg-transparent px-3.5 py-2.5 text-[13.5px] leading-relaxed outline-none placeholder:text-faint"
			></textarea>
		</div>
	</div>

	<!-- Reviewing changes -->
	<div class="flex flex-col gap-3">
		<div>
			<h2 class="text-[15px] font-medium">Reviewing changes</h2>
			<p class="mt-0.5 text-[12.5px] text-muted-foreground">How the Changes panel shows what a task edited. Also under <span class="text-foreground/80">View options</span> in the panel itself.</p>
		</div>
		<div class="divide-y rounded-xl border bg-card">
			<div class="flex flex-wrap items-center justify-between gap-x-6 gap-y-2.5 px-4 py-3.5 sm:px-5">
				<div class="min-w-0">
					<p class="text-[13.5px] font-medium">Layout</p>
					<p class="mt-0.5 text-[12.5px] leading-relaxed text-muted-foreground">Split puts old and new side by side on wide screens.</p>
				</div>
				{@render segmented('Layout', [{ id: 'unified', label: 'Unified', icon: Rows }, { id: 'split', label: 'Split', icon: Columns }], prefs.diffView, (v) => prefs.set({ diffView: v as 'unified' | 'split' }))}
			</div>
			{@render row('ws', 'Hide whitespace changes', 'Lines where only indentation or spacing changed show as unchanged, so a reformat doesn’t bury the real edit. Press W in the panel to flip it.', prefs.diffHideWhitespace, (v) => prefs.set({ diffHideWhitespace: v }))}
			{@render row('fold', 'Open with files collapsed', 'Each file starts folded so you can pick what to read on a big change. A file you open the panel from still opens.', prefs.diffCollapsed, (v) => prefs.set({ diffCollapsed: v }))}
		</div>
	</div>

	<!-- Safety -->
	<div class="flex flex-col gap-3">
		<div>
			<h2 class="text-[15px] font-medium">Confirmations</h2>
			<p class="mt-0.5 text-[12.5px] text-muted-foreground">What asks before it acts. Archiving needs no question: it offers Undo instead.</p>
		</div>
		<div class="rounded-xl border bg-card">
			{@render row('stop', 'Ask before stopping a task', 'Stop shows a question first, so a stray tap on a phone doesn’t end a run halfway.', prefs.confirmStop, (v) => prefs.set({ confirmStop: v }))}
		</div>
	</div>

	<!-- Appearance -->
	<div class="flex flex-col gap-3">
		<h2 class="text-[15px] font-medium">Appearance</h2>
		<div class="rounded-xl border bg-card">
			<div class="flex flex-wrap items-center justify-between gap-x-6 gap-y-2.5 px-4 py-3.5 sm:px-5">
				<div class="min-w-0">
					<p class="text-[13.5px] font-medium">Theme</p>
					<p class="mt-0.5 text-[12.5px] leading-relaxed text-muted-foreground">System follows your device’s light or dark setting.</p>
				</div>
				{@render segmented('Theme', themes, userPrefersMode.current ?? 'system', (v) => setMode(v as 'light' | 'dark' | 'system'))}
			</div>
		</div>
	</div>
</section>
