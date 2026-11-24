<script lang="ts">
	// "Set icon…" for a project: an emoji, a coloured letter or a small picture, for everyone in the org.
	import { untrack } from 'svelte';
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { toast } from 'svelte-sonner';
	import ImageUp from '@lucide/svelte/icons/image-up';
	import ProjectIcon from './ProjectIcon.svelte';
	import { EMOJI, IMAGE_SIZE, TILE_COLORS, projects, projectName, shrinkImage, type IconKind, type ProjectIcon as Icon } from '../projects.svelte';

	const repo = $derived(projects.picking);
	/** What Save would set; null is the default. */
	let draft = $state<Icon | null>(null);
	let tab = $state<IconKind>('emoji');
	let file = $state<HTMLInputElement | null>(null);
	let custom = $state('');

	// Start from the current icon each time the picker opens.
	$effect(() => {
		const r = repo;
		if (!r) return;
		untrack(() => {
			draft = projects.icons.get(r) ?? null;
			tab = draft?.kind ?? 'emoji';
			custom = '';
		});
	});

	async function upload(e: Event) {
		const f = (e.target as HTMLInputElement).files?.[0];
		(e.target as HTMLInputElement).value = '';
		if (!f) return;
		try {
			draft = { kind: 'image', value: await shrinkImage(f) };
		} catch {
			toast.error("Couldn't read that image", { description: 'Try a PNG or JPEG.' });
		}
	}

	/** The first emoji typed or pasted, if it is one. */
	function typed(v: string): string {
		const seg = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
		const first = [...seg.segment(v.trim())][0]?.segment ?? '';
		return /\p{Extended_Pictographic}|\p{Regional_Indicator}/u.test(first) ? first : '';
	}

	function save(e: Event) {
		e.preventDefault();
		if (!repo) return;
		void projects.setIcon(repo, draft);
		projects.picking = null;
	}

	const tabs: [IconKind, string][] = [['emoji', 'Emoji'], ['letter', 'Letter'], ['image', 'Image']];
	const cell = 'grid aspect-square place-items-center rounded-md transition-colors hover:bg-accent';
</script>

<Dialog.Root open={!!repo} onOpenChange={(o) => { if (!o) projects.picking = null; }}>
	<Dialog.Content class="sm:max-w-[22rem]">
		{#if repo}
			<Dialog.Header>
				<div class="flex items-center gap-3">
					<ProjectIcon {repo} icon={draft ?? undefined} size={36} />
					<div class="flex min-w-0 flex-col gap-0.5 text-left">
						<Dialog.Title class="truncate">{projectName(repo)}</Dialog.Title>
						<Dialog.Description class="text-xs">Shown next to it in the task list, for everyone in the org</Dialog.Description>
					</div>
				</div>
			</Dialog.Header>
			<form class="flex flex-col gap-3" onsubmit={save}>
				<div class="flex gap-1" role="tablist" aria-label="Kind of icon">
					{#each tabs as [k, label] (k)}
						<button
							type="button"
							role="tab"
							aria-selected={tab === k}
							class="rounded-full px-3 py-1 text-[12.5px] transition-colors {tab === k ? 'bg-accent font-medium text-foreground' : 'text-muted-foreground hover:text-foreground'}"
							onclick={() => (tab = k)}
						>{label}</button>
					{/each}
				</div>

				{#if tab === 'emoji'}
					<div class="grid grid-cols-8 gap-0.5">
						{#each EMOJI as e (e)}
							<button
								type="button"
								class="{cell} text-[19px] {draft?.kind === 'emoji' && draft.value === e ? 'bg-accent ring-1 ring-run' : ''}"
								aria-label="Use {e}"
								aria-pressed={draft?.kind === 'emoji' && draft.value === e}
								onclick={() => (draft = { kind: 'emoji', value: e })}
							>{e}</button>
						{/each}
					</div>
					<input
						bind:value={custom}
						oninput={() => { const e = typed(custom); if (e) draft = { kind: 'emoji', value: e }; }}
						placeholder="Or type or paste any emoji"
						aria-label="Any emoji"
						class="h-8 rounded-md border border-input bg-card px-2.5 text-[13px] outline-none placeholder:text-muted-foreground focus:border-foreground/30"
					/>
				{:else if tab === 'letter'}
					<p class="text-[12.5px] text-muted-foreground">The first letter of {projectName(repo)} on a colour.</p>
					<div class="flex flex-wrap gap-1.5">
						{#each TILE_COLORS as c (c)}
							{@const on = draft?.kind === 'letter' && draft.value === c}
							<button
								type="button"
								class="rounded-md {on ? 'ring-2 ring-foreground ring-offset-2 ring-offset-background' : ''}"
								aria-label="Colour {c}"
								aria-pressed={on}
								onclick={() => (draft = { kind: 'letter', value: c })}
							><ProjectIcon {repo} icon={{ kind: 'letter', value: c }} size={26} /></button>
						{/each}
					</div>
				{:else}
					<button
						type="button"
						class="flex flex-col items-center gap-2 rounded-lg border border-dashed border-border px-4 py-5 text-center text-[12.5px] text-muted-foreground transition-colors hover:border-foreground/30 hover:text-foreground"
						onclick={() => file?.click()}
					>
						{#if draft?.kind === 'image'}
							<img src={draft.value} alt="" class="size-12 rounded-[10px] object-cover" />Choose another
						{:else}
							<ImageUp class="size-5" />Choose an image
							<span class="text-[11.5px] text-faint">Cropped square and shrunk to {IMAGE_SIZE}px</span>
						{/if}
					</button>
					<input bind:this={file} type="file" accept="image/png,image/jpeg,image/webp,image/gif" class="hidden" onchange={upload} />
				{/if}

				<div class="flex items-center justify-between pt-1">
					<Button type="button" variant="ghost" size="sm" class="text-muted-foreground" disabled={!draft} onclick={() => (draft = null)}>Reset to default</Button>
					<Button type="submit" size="sm" class="rounded-full px-4">Save</Button>
				</div>
			</form>
		{/if}
	</Dialog.Content>
</Dialog.Root>
