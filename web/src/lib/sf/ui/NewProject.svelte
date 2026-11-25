<script lang="ts">
	// New project: start an empty git repo, or clone one, on one of your machines, then
	// start a task in it. The machine makes it in its repos folder and reports it back.
	import * as Dialog from '#lib/components/ui/dialog';
	import * as Select from '#lib/components/ui/select';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import FolderPlus from '@lucide/svelte/icons/folder-plus';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { toast } from 'svelte-sonner';
	import { goto } from '$app/navigation';
	import { untrack } from 'svelte';
	import { fleet } from '../state.svelte';
	import { projects } from '../projects.svelte';
	import { api } from '../api';

	type Mode = 'empty' | 'clone';
	interface Status { status: 'pending' | 'running' | 'done' | 'failed'; repo?: string; path?: string; error?: string; warning?: string }

	const MACHINE = 'sidefleet_new_project_machine';

	let mode = $state<Mode>('empty');
	let name = $state('');
	let url = $state('');
	/** The name was typed, so a pasted URL no longer fills it in. */
	let nameTyped = $state(false);
	let machineId = $state('');
	let busy = $state(false);
	let phase = $state<'' | 'waiting' | 'making'>('');
	let error = $state('');
	let nameBox: HTMLInputElement | null = $state(null);
	let urlBox: HTMLInputElement | null = $state(null);

	/** The folder a name becomes: spaces to dashes, anything else git or the shell would mind dropped. */
	function folder(n: string) {
		return n.trim().replace(/\s+/g, '-').replace(/[^A-Za-z0-9._-]/g, '').replace(/^\.+/, '').slice(0, 100);
	}
	/** A clone's folder: the URL's last part without `.git`. */
	function fromUrl(u: string) {
		const last = u.trim().replace(/\/+$/, '').split(/[/:]/).pop() ?? '';
		return folder(last.replace(/\.git$/, ''));
	}

	// Your online machines; the ones whose Sidefleet can make projects come first.
	const able = $derived(fleet.myOnline.filter((w) => w.capabilities.new_repos_root));
	const tooOld = $derived(fleet.myOnline.filter((w) => !w.capabilities.new_repos_root));
	const machine = $derived(able.find((w) => w.id === machineId) ?? able[0]);
	const target = $derived(folder(name));
	const path = $derived(machine?.capabilities.new_repos_root ? `${machine.capabilities.new_repos_root.replace(/\/+$/, '')}/${target || '…'}` : '');
	const taken = $derived(!!target && !!machine?.capabilities.repos?.some((r) => r.name.toLowerCase() === target.toLowerCase()));
	const canSubmit = $derived(!!machine && !!target && !taken && (mode === 'empty' || !!url.trim()) && !busy);

	// Fresh each time it opens, on the machine used last.
	$effect(() => {
		if (!fleet.newProjectOpen) return;
		untrack(() => {
			if (busy) return;
			name = '';
			url = '';
			nameTyped = false;
			error = '';
			try { machineId = localStorage.getItem(MACHINE) ?? ''; } catch { /* storage blocked */ }
		});
	});

	function pickMode(m: Mode) {
		mode = m;
		error = '';
		queueMicrotask(() => (m === 'clone' ? urlBox : nameBox)?.focus());
	}
	function pickMachine(id: string) {
		machineId = id;
		try { localStorage.setItem(MACHINE, id); } catch { /* storage blocked */ }
	}

	const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!canSubmit || !machine) return;
		const on = machine;
		busy = true;
		error = '';
		phase = 'waiting';
		try {
			const body = mode === 'clone' ? { name: target, url: url.trim() } : { name: target };
			const { id } = await api<{ id: string }>(`/v1/workers/${encodeURIComponent(on.id)}/new-repos`, { method: 'POST', body: JSON.stringify(body) });
			let s: Status = { status: 'pending' };
			// A clone can take minutes; the control plane gives up on a silent machine after 15.
			for (let i = 0; i < 1200 && (s.status === 'pending' || s.status === 'running'); i++) {
				await sleep(i < 20 ? 500 : 1500);
				s = await api<Status>(`/v1/new-repos/${id}`);
				if (s.status === 'running') phase = 'making';
			}
			if (s.status !== 'done' || !s.repo) throw new Error(s.error || 'The machine didn’t say how it went.');
			await fleet.refresh();
			fleet.newProjectOpen = false;
			toast.success(mode === 'clone' ? `Cloned ${target} on ${on.display_name}` : `Created ${target} on ${on.display_name}`, { description: s.path });
			if (s.warning) toast.warning(s.warning);
			projects.composeIn = s.repo;
			await goto('#/');
		} catch (err) {
			error = (err as Error).message;
		} finally {
			busy = false;
			phase = '';
		}
	}
</script>

<Dialog.Root bind:open={fleet.newProjectOpen}>
	<Dialog.Content
		class="sm:max-w-[30rem]"
		interactOutsideBehavior={busy ? 'ignore' : 'close'}
		escapeKeydownBehavior={busy ? 'ignore' : 'close'}
		onOpenAutoFocus={(e) => { e.preventDefault(); (mode === 'clone' ? urlBox : nameBox)?.focus(); }}
	>
		<Dialog.Header>
			<Dialog.Title>New project</Dialog.Title>
			<Dialog.Description>Start a git repo or clone one on your machine, then start a task in it.</Dialog.Description>
		</Dialog.Header>

		{#if !fleet.myOnline.length}
			<p class="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
				None of your machines is online. Start Sidefleet on one, or <button type="button" class="text-foreground underline underline-offset-2" onclick={() => { fleet.newProjectOpen = false; fleet.addMachineOpen = true; }}>add a machine</button>.
			</p>
		{:else if !able.length}
			<p class="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
				{tooOld.length === 1 ? tooOld[0].display_name + '’s' : 'Your machines’'} Sidefleet is too old to make projects from here. Update it from the <a href="#/machines" class="text-foreground underline underline-offset-2" onclick={() => (fleet.newProjectOpen = false)}>Machines</a> page.
			</p>
		{:else}
			<form onsubmit={submit} class="flex flex-col gap-4">
				<div class="flex rounded-lg bg-muted p-0.5" role="radiogroup" aria-label="Start from">
					{#each [{ id: 'empty', label: 'Empty repo', icon: FolderPlus }, { id: 'clone', label: 'Clone from URL', icon: GitBranch }] as const as o (o.id)}
						<button
							type="button"
							role="radio"
							aria-checked={mode === o.id}
							disabled={busy}
							class="flex h-8 flex-1 items-center justify-center gap-1.5 rounded-md text-[13px] transition-colors {mode === o.id ? 'bg-card font-medium text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}"
							onclick={() => pickMode(o.id)}
						><o.icon class="size-3.5" />{o.label}</button>
					{/each}
				</div>

				{#if mode === 'clone'}
					<label class="flex flex-col gap-1.5 text-[13px] font-medium">
						Git URL
						<Input
							bind:ref={urlBox}
							bind:value={url}
							oninput={() => { if (!nameTyped) name = fromUrl(url); }}
							placeholder="git@github.com:you/app.git"
							class="h-9 font-mono text-[12.5px]"
							autocomplete="off"
							spellcheck="false"
							disabled={busy}
						/>
					</label>
				{/if}
				<label class="flex flex-col gap-1.5 text-[13px] font-medium">
					{mode === 'clone' ? 'Folder' : 'Name'}
					<Input
						bind:ref={nameBox}
						bind:value={name}
						oninput={() => (nameTyped = !!name)}
						placeholder="my-app"
						maxlength={100}
						class="h-9"
						autocomplete="off"
						spellcheck="false"
						disabled={busy}
					/>
				</label>

				{#if able.length > 1}
					<div class="flex items-center justify-between gap-3 text-[13px]">
						<span class="font-medium">Machine</span>
						<Select.Root type="single" value={machine?.id ?? ''} onValueChange={pickMachine} disabled={busy}>
							<Select.Trigger size="sm" class="h-8 min-w-[10rem] rounded-full" aria-label="Machine">{machine?.display_name}</Select.Trigger>
							<Select.Content>
								{#each able as w (w.id)}<Select.Item value={w.id} label={w.display_name}>{w.display_name}</Select.Item>{/each}
							</Select.Content>
						</Select.Root>
					</div>
				{/if}

				<p class="-mt-1 min-h-10 text-xs leading-relaxed text-muted-foreground" aria-live="polite">
					{#if error}
						<span class="text-err-ink">{error}</span>
					{:else if taken}
						<span class="text-err-ink">{machine?.display_name} already has a {target} folder.</span> Pick another name, or <button type="button" class="text-foreground underline underline-offset-2" onclick={() => { fleet.newProjectOpen = false; fleet.pickerOpen = true; }}>choose it from your repos</button>.
					{:else if phase === 'waiting'}
						Waiting for {machine?.display_name} to pick this up…
					{:else if phase === 'making'}
						{mode === 'clone' ? 'Cloning' : 'Creating'} <span class="font-mono text-foreground">{path}</span>…
					{:else}
						{mode === 'clone' ? 'Clones into' : 'Creates'} <span class="font-mono text-foreground">{path}</span>{able.length === 1 ? ` on ${machine?.display_name}` : ''}{mode === 'empty' ? ', with a README and a first commit.' : '.'}
						{#if target && target !== name.trim() && name.trim()}<br />Spaces become dashes and other symbols are left out.{/if}
					{/if}
				</p>

				<div class="flex items-center justify-between gap-3">
					<button type="button" class="text-left text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline" disabled={busy} onclick={() => { fleet.newProjectOpen = false; fleet.pickerOpen = true; }}>
						Pick an existing repo
					</button>
					<Button type="submit" class="h-9 rounded-full px-4" disabled={!canSubmit}>
						{#if busy}<LoaderCircle class="size-4 animate-spin" />{/if}
						{busy ? (mode === 'clone' ? 'Cloning…' : 'Creating…') : mode === 'clone' ? 'Clone project' : 'Create project'}
					</Button>
				</div>
			</form>
		{/if}
	</Dialog.Content>
</Dialog.Root>
