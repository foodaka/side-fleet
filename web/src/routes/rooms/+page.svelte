<script lang="ts">
	// Rooms: a goal the team splits into tasks. Each runs on its owner's machine; finished work
	// lands on the room branch, and new tasks start from it.
	import { goto } from '$app/navigation';
	import * as Dialog from '#lib/components/ui/dialog';
	import * as Select from '#lib/components/ui/select';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { toast } from 'svelte-sonner';
	import Plus from '@lucide/svelte/icons/plus';
	import Boxes from '@lucide/svelte/icons/boxes';
	import GitMerge from '@lucide/svelte/icons/git-merge';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { rel, repoLabel } from '#lib/sf/format';
	import type { Room } from '#lib/sf/types';
	import Avatar from '#lib/sf/ui/Avatar.svelte';

	let rooms = $state<Room[] | null>(null);
	let archived = $state(false);
	let error = $state('');

	async function load() {
		try {
			rooms = await api<Room[]>(`/v1/rooms?archived=${archived}`);
			error = '';
		} catch (e) {
			error = (e as Error).message;
			rooms ??= [];
		}
	}
	$effect(() => {
		void archived;
		void fleet.orgId;
		if (!fleet.loaded) return;
		rooms = null;
		void load();
		const t = setInterval(() => void load(), 10_000);
		return () => clearInterval(t);
	});

	// New room
	let open = $state(false);
	let name = $state('');
	let goal = $state('');
	let repo = $state('');
	let busy = $state(false);
	/** Your repos first (where your tasks can run), then the team's. */
	const repos = $derived.by(() => {
		const mine = new Set(fleet.mine.flatMap((w) => (w.capabilities.repos ?? []).map((r) => r.id || r.name)));
		const all = [...fleet.allRepos.keys()];
		const chosen = fleet.added ? [...fleet.added] : [];
		return [...new Set([...chosen.filter((r) => all.includes(r)), ...all])].sort(
			(a, b) => Number(mine.has(b)) - Number(mine.has(a)) || Number(chosen.includes(b)) - Number(chosen.includes(a)) || a.localeCompare(b)
		);
	});
	$effect(() => {
		if (open && !repo) repo = fleet.composerRepos[0] ?? repos[0] ?? '';
	});

	async function create(e: SubmitEvent) {
		e.preventDefault();
		if (!name.trim() || !repo) return;
		busy = true;
		try {
			const r = await api<Room>('/v1/rooms', { method: 'POST', body: JSON.stringify({ name: name.trim(), goal: goal.trim(), repo }) });
			open = false;
			name = '';
			goal = '';
			await goto('#/rooms/' + r.id);
		} catch (err) {
			toast.error('Could not create the room', { description: (err as Error).message });
		}
		busy = false;
	}
</script>

<svelte:head><title>Rooms · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-4xl flex-col gap-6 px-4 py-8 md:px-8">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div class="max-w-xl">
			<h1 class="text-[22px] font-semibold tracking-tight">Rooms</h1>
			<p class="mt-1 text-[13px] leading-relaxed text-muted-foreground">
				A room is one goal, split into tasks. Each task runs on its owner's machine; finished work is added to the room's branch, and new tasks start from it. One pull request at the end.
			</p>
		</div>
		<div class="flex items-center gap-2">
			<button type="button" class="text-xs text-muted-foreground hover:text-foreground" onclick={() => (archived = !archived)}>{archived ? 'Show active' : 'Show archived'}</button>
			<Button class="h-8 rounded-full text-[13px]" onclick={() => (open = true)}><Plus class="size-4" />New room</Button>
		</div>
	</header>

	{#if error && !rooms?.length}
		<p class="rounded-xl border border-err/30 bg-err-soft/40 p-4 text-[13px] text-err-ink">Couldn't load rooms: {error}</p>
	{:else if rooms === null}
		<div class="grid gap-3">
			{#each [0, 1, 2] as i (i)}<div class="h-24 animate-pulse rounded-xl bg-muted"></div>{/each}
		</div>
	{:else if !rooms.length}
		<div class="flex flex-col items-start gap-3 rounded-xl border border-dashed p-6">
			<Boxes class="size-5 text-muted-foreground" />
			<div>
				<h2 class="text-[15px] font-medium">{archived ? 'No archived rooms' : 'No rooms yet'}</h2>
				<p class="mt-1 max-w-lg text-[13px] leading-relaxed text-muted-foreground">
					{archived
						? 'Rooms you archive show up here.'
						: 'Start one for the next thing you and your team ship: "Apple Pay at checkout", "Move billing to Stripe". Then split it into tasks and take one each.'}
				</p>
			</div>
			{#if !archived}<Button class="h-8 rounded-full text-[13px]" onclick={() => (open = true)}><Plus class="size-4" />New room</Button>{/if}
		</div>
	{:else}
		<ul class="grid gap-3">
			{#each rooms as r (r.id)}
				<li>
					<a href="#/rooms/{r.id}" class="flex flex-col gap-2 rounded-xl border bg-card p-4 transition-colors hover:bg-accent/40">
						<div class="flex items-start justify-between gap-3">
							<h2 class="min-w-0 truncate text-[15px] font-semibold tracking-tight">{r.name}</h2>
							<span class="tnum shrink-0 text-xs text-muted-foreground">{rel(r.last_activity_at, fleet.now)}</span>
						</div>
						{#if r.goal}<p class="line-clamp-2 text-[13px] leading-relaxed text-muted-foreground">{r.goal}</p>{/if}
						<div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
							{#if r.created_by_login}<span class="inline-flex items-center gap-1.5"><Avatar login={r.created_by_login} size={16} class="ring-0" />@{r.created_by_login}</span>{/if}
							<span class="font-mono text-[11.5px]">{repoLabel(r.repo)}</span>
							<span class="tnum">{r.task_count} task{r.task_count === 1 ? '' : 's'}</span>
							{#if r.active_count}<span class="tnum rounded-full bg-run-soft px-1.5 text-run-ink">{r.active_count} working</span>{/if}
							{#if r.integrated_count}<span class="tnum inline-flex items-center gap-1"><GitMerge class="size-3" />{r.integrated_count} merged</span>{/if}
						</div>
					</a>
				</li>
			{/each}
		</ul>
	{/if}
</section>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-[32rem]">
		<Dialog.Header>
			<Dialog.Title>New room</Dialog.Title>
			<Dialog.Description>Everyone in {fleet.org?.name ?? 'the org'} can see it, add notes and take tasks.</Dialog.Description>
		</Dialog.Header>
		<form class="flex flex-col gap-3" onsubmit={create}>
			<label class="flex flex-col gap-1.5 text-xs font-medium">
				Name
				<Input bind:value={name} placeholder="Apple Pay at checkout" maxlength={120} class="h-9 text-[14px] font-normal" />
			</label>
			<label class="flex flex-col gap-1.5 text-xs font-medium">
				Goal
				<textarea
					bind:value={goal}
					rows="4"
					maxlength={4000}
					placeholder="What done looks like, constraints, decisions already made. Every agent in the room reads this."
					class="w-full resize-none rounded-lg border border-input bg-card px-3 py-2 text-[14px] font-normal leading-relaxed outline-none placeholder:text-faint focus:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]"
				></textarea>
			</label>
			<div class="flex flex-col gap-1.5 text-xs font-medium">
				Repo
				<Select.Root type="single" value={repo} onValueChange={(v) => (repo = v)}>
					<Select.Trigger class="h-9 w-full font-mono text-[13px] font-normal" aria-label="Repo"><span class="truncate">{repoLabel(repo) || 'Choose a repo'}</span></Select.Trigger>
					<Select.Content class="max-h-72">
						{#each repos as r (r)}<Select.Item value={r} label={repoLabel(r)}><span class="font-mono text-[12.5px]">{repoLabel(r)}</span></Select.Item>{/each}
					</Select.Content>
				</Select.Root>
				{#if !repos.length}<span class="font-normal text-muted-foreground">No machine in this org reports a git checkout yet. Connect one first.</span>{/if}
			</div>
			<Button type="submit" class="mt-1 h-9 rounded-full" disabled={busy || !name.trim() || !repo}>{busy ? 'Creating…' : 'Create room'}</Button>
		</form>
	</Dialog.Content>
</Dialog.Root>
