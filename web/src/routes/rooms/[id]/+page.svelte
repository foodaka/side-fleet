<script lang="ts">
	// One room: the goal, the tasks people took (each on its owner's machine), and the team's notes.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { tick, untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { Button } from '#lib/components/ui/button';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import GitMerge from '@lucide/svelte/icons/git-merge';
	import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import Pencil from '@lucide/svelte/icons/pencil';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import Check from '@lucide/svelte/icons/check';
	import Paperclip from '@lucide/svelte/icons/paperclip';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { api, ApiError } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { AGENTS, agentModel, agentName, defaultModel, hasAgent, machineModels, modelValid, permsFor, rel, repoCloneCommand, repoKey, repoLabel } from '#lib/sf/format';
	import { prefs } from '#lib/sf/prefs.svelte';
	import type { AgentId, RoomDetail, RoomNote, TaskCard } from '#lib/sf/types';
	import { compareUrl, fixConflictsPrompt } from '#lib/sf/rooms';
	import Avatar from '#lib/sf/ui/Avatar.svelte';
	import StatusPill from '#lib/sf/ui/StatusPill.svelte';
	import CopyButton from '#lib/sf/ui/CopyButton.svelte';
	import PrIcon from '#lib/sf/ui/PrIcon.svelte';
	import AgentModelPicker from '#lib/sf/ui/AgentModelPicker.svelte';
	import ComposerOptions from '#lib/sf/ui/ComposerOptions.svelte';
	import AttachedImages from '#lib/sf/ui/AttachedImages.svelte';
	import { Attachments, IMAGE_TYPES } from '#lib/sf/attach.svelte';
	import DiffStat from '#lib/sf/diff/DiffStat.svelte';

	const id = $derived(page.params.id ?? '');
	let data = $state<RoomDetail | null>(null);
	let missing = $state('');

	async function load(rid = id) {
		try {
			const d = await api<RoomDetail>(`/v1/rooms/${rid}`);
			if (rid === id) {
				data = d;
				missing = '';
			}
		} catch (e) {
			if (rid === id && e instanceof ApiError && e.status === 404) missing = 'This room does not exist, or it belongs to another org.';
		}
	}
	// Poll while the room is open: tasks move, notes arrive.
	$effect(() => {
		const rid = id;
		if (!rid || !fleet.loaded) return;
		untrack(() => { data = null; missing = ''; attachments.clear(); });
		void load(rid);
		const t = setInterval(() => { if (document.visibilityState === 'visible') void load(rid); }, 3000);
		return () => clearInterval(t);
	});

	const room = $derived(data?.room ?? null);
	const tasks = $derived(data?.tasks ?? []);
	const notes = $derived(data?.notes ?? []);
	const canEdit = $derived(!!room && (!room.created_by || room.created_by === fleet.userId || fleet.isOwner));
	const compare = $derived(room?.integrated_at ? compareUrl(room) : null);
	const live = (t: TaskCard) => t.status === 'running' || t.status === 'queued';
	const isMine = (t: TaskCard) => !t.created_by || !fleet.userId || t.created_by === fleet.userId;
	const order = (t: TaskCard) => (live(t) ? 0 : t.integration?.status === 'failed' ? 1 : !t.integration ? 2 : 3);
	const sortedTasks = $derived([...tasks].sort((a, b) => order(a) - order(b) || b.last_activity_at.localeCompare(a.last_activity_at)));

	// ---- editing the room ----
	let editing = $state<'name' | 'goal' | null>(null);
	let draft = $state('');
	function edit(what: 'name' | 'goal') {
		if (!room) return;
		draft = what === 'name' ? room.name : room.goal;
		editing = what;
	}
	async function saveEdit() {
		if (!room || !editing) return;
		const what = editing;
		const value = draft.trim();
		if (what === 'name' && !value) return;
		try {
			const r = await api(`/v1/rooms/${room.id}`, { method: 'PATCH', body: JSON.stringify({ [what]: value }) });
			if (data) data.room = r as typeof room;
			editing = null;
		} catch (e) {
			toast.error('Could not save', { description: (e as Error).message });
		}
	}
	async function setArchived(archived: boolean) {
		if (!room) return;
		try {
			await api(`/v1/rooms/${room.id}`, { method: 'PATCH', body: JSON.stringify({ archived }) });
			toast(archived ? 'Room archived' : 'Room restored');
			if (archived) await goto('#/rooms');
			else void load();
		} catch (e) {
			toast.error('Could not update the room', { description: (e as Error).message });
		}
	}

	// ---- tasks ----
	let busyTask = $state<string | null>(null);
	async function integrate(t: TaskCard) {
		busyTask = t.id;
		try {
			await api(`/v1/runs/${t.id}/integrate`, { method: 'POST' });
			toast('Adding to the room', { description: `${t.worker_id ?? 'Your machine'} merges it into ${room?.branch}.` });
			await load();
		} catch (e) {
			toast.error('Could not add it to the room', { description: (e as Error).message });
		}
		busyTask = null;
	}
	async function askToFix(t: TaskCard) {
		if (!room) return;
		busyTask = t.id;
		try {
			await api(`/v1/runs/${t.id}/messages`, { method: 'POST', body: JSON.stringify({ task: fixConflictsPrompt(room.branch, t.integration?.conflicts ?? []) }) });
			toast('Asked the agent to resolve it', { description: 'Add it to the room again once it finishes.' });
			await load();
		} catch (e) {
			toast.error('Could not send', { description: (e as Error).message });
		}
		busyTask = null;
	}

	// ---- new task composer (your own machines only) ----
	function stored(k: string) { try { return localStorage.getItem(k) ?? ''; } catch { return ''; } }
	let agent = $state<AgentId>(AGENTS.some((a) => a.id === stored('sidefleet_agent')) ? (stored('sidefleet_agent') as AgentId) : 'claude');
	let model = $state('');
	let effort = $state('');
	// Your room default from Settings, narrower than full access unless you chose otherwise: room tasks read teammates' notes.
	let perm = $state(prefs.roomPermission);
	$effect(() => { perm = prefs.roomPermission; });
	// Only Claude Code can ask you about tool calls; the others fall back to editing files.
	const effectivePerm = $derived(permsFor(agent).some((p) => p.id === perm) ? perm : 'acceptEdits');
	let machine = $state('');
	let task = $state('');
	let starting = $state(false);
	let box: HTMLTextAreaElement | undefined = $state();
	let picker: HTMLInputElement | undefined = $state();
	// Images for the new task: dropped anywhere on the page, pasted into the box, or picked with the paperclip.
	const attachments = new Attachments();
	const canAttach = $derived(!!room && !room.archived_at);
	$effect(() => {
		const a = agent;
		untrack(() => {
			const m = stored(a === 'claude' ? 'sidefleet_model' : `sidefleet_model_${a}`);
			model = modelValid(a, m) ? m : defaultModel(a);
			effort = '';
		});
	});
	const hasRepo = (w: (typeof fleet.workers)[number], repo: string) => (w.capabilities.repos ?? []).some((r) => repoKey(r) === repo || r.name === repo);
	const withRepo = $derived(room ? fleet.myOnline.filter((w) => hasRepo(w, room.repo)) : []);
	const machines = $derived(withRepo.filter((w) => hasAgent(w, agent)));
	const pinned = $derived(machines.find((w) => w.id === machine));
	const models = $derived(machineModels(pinned ? [pinned] : machines, agent));
	const canStart = $derived(!!task.trim() && machines.length > 0 && modelValid(agent, model) && !starting);
	/** Why the start button is off, shown on it; empty while it's only waiting for a task to be typed. */
	const blocked = $derived(
		!room ? '' :
		!fleet.myOnline.length ? 'None of your machines is online' :
		!withRepo.length ? `None of your online machines has ${repoLabel(room.repo)} checked out` :
		!machines.length ? `None of your machines with ${repoLabel(room.repo)} has ${agentName(agent)}` : ''
	);
	// No machine of yours has this agent for the repo: switch to one that does.
	$effect(() => {
		if (!withRepo.length || machines.length) return;
		const other = AGENTS.find((a) => withRepo.some((w) => hasAgent(w, a.id)));
		if (other && other.id !== agent) untrack(() => (agent = other.id));
	});

	async function startTask(e?: Event) {
		e?.preventDefault();
		if (!room || !canStart) return;
		starting = true;
		try {
			const r = await api<{ id: string }>(`/v1/rooms/${room.id}/tasks`, {
				method: 'POST',
				body: JSON.stringify({ task: task.trim(), agent, model, effort: effort || undefined, permission_mode: effectivePerm, worker_id: pinned?.id, images: await attachments.payload() })
			});
			task = '';
			attachments.clear();
			void fleet.refresh();
			await load();
			toast('Task started', { description: 'It runs on your machine with the room brief.', action: { label: 'Open', onClick: () => void goto('#/runs/' + r.id) } });
		} catch (err) {
			toast.error('Could not start the task', { description: (err as Error).message });
		}
		starting = false;
	}
	function grow(el: HTMLTextAreaElement | undefined, max: number) {
		if (!el) return;
		el.style.height = 'auto';
		el.style.height = Math.min(max, Math.max(44, el.scrollHeight)) + 'px';
	}
	$effect(() => { void task; grow(box, 240); });

	// ---- notes ----
	let note = $state('');
	let posting = $state(false);
	let noteBox: HTMLTextAreaElement | undefined = $state();
	let noteList: HTMLElement | undefined = $state();
	$effect(() => { void note; grow(noteBox, 160); });
	// Keep the conversation scrolled to the latest note as new ones arrive.
	let seenNotes = 0;
	$effect(() => {
		const n = notes.length;
		if (n === seenNotes) return;
		seenNotes = n;
		void tick().then(() => noteList?.scrollTo({ top: noteList.scrollHeight }));
	});
	async function postNote(e?: Event) {
		e?.preventDefault();
		if (!room || !note.trim() || posting) return;
		posting = true;
		try {
			await api(`/v1/rooms/${room.id}/notes`, { method: 'POST', body: JSON.stringify({ body: note.trim() }) });
			note = '';
			await load();
		} catch (err) {
			toast.error('Could not post the note', { description: (err as Error).message });
		}
		posting = false;
	}
	// Its author, the room's creator or an org owner can take a note back; agents stop getting it in their brief.
	const canDeleteNote = (n: RoomNote) => !n.author_id || n.author_id === fleet.userId || canEdit;
	// Only its author rewrites a note.
	let editingNote = $state<string | null>(null);
	let noteDraft = $state('');
	let savingNote = $state(false);
	function editNote(n: RoomNote) {
		noteDraft = n.body;
		editingNote = n.id;
	}
	async function saveNote(n: RoomNote) {
		const body = noteDraft.trim();
		if (!room || !body || savingNote) return;
		if (body === n.body) return void (editingNote = null);
		savingNote = true;
		try {
			const saved = await api<RoomNote>(`/v1/rooms/${room.id}/notes/${n.id}`, { method: 'PATCH', body: JSON.stringify({ body }) });
			if (data) data.notes = data.notes.map((x) => (x.id === n.id ? saved : x));
			editingNote = null;
		} catch (err) {
			toast.error('Could not save the note', { description: (err as Error).message });
		}
		savingNote = false;
	}
	async function deleteNote(n: RoomNote) {
		if (!room || !confirm('Delete this note? Agents starting tasks in the room stop seeing it.')) return;
		try {
			await api(`/v1/rooms/${room.id}/notes/${n.id}`, { method: 'DELETE' });
			if (data) data.notes = data.notes.filter((x) => x.id !== n.id);
		} catch (err) {
			toast.error('Could not delete the note', { description: (err as Error).message });
		}
	}
</script>

<svelte:head><title>{room?.name ?? 'Room'} · Sidefleet</title></svelte:head>
<svelte:window
	ondragenter={(e) => canAttach && attachments.dragenter(e)}
	ondragleave={(e) => canAttach && attachments.dragleave(e)}
	ondragover={(e) => canAttach ? attachments.dragover(e) : e.preventDefault()}
	ondrop={(e) => canAttach ? attachments.drop(e) : e.preventDefault()}
/>

{#snippet taskCard(t: TaskCard)}
	{@const mine = isMine(t)}
	{@const i = t.integration}
	{@const adding = i && (i.status === 'queued' || i.status === 'running')}
	{@const done = i?.status === 'succeeded'}
	{@const failed = i?.status === 'failed' || i?.status === 'stopped'}
	<li class="flex flex-col gap-2.5 rounded-xl border bg-card p-3.5">
		<div class="flex items-start gap-2.5">
			<Avatar login={t.created_by_login} size={22} class="mt-0.5" />
			<div class="min-w-0 flex-1">
				<a href="#/runs/{t.id}" class="line-clamp-2 text-[13.5px] leading-snug font-medium hover:underline" title={t.title}>{t.title}</a>
				<p class="mt-0.5 truncate text-xs text-muted-foreground">
					{mine ? 'You' : `@${t.created_by_login ?? 'teammate'}`} · {agentModel(t.agent, t.model)}{t.worker_id ? ` · ${t.worker_id}` : ''} · {rel(t.last_activity_at, fleet.now)}
				</p>
			</div>
			<StatusPill status={t.status} class="shrink-0" />
		</div>

		{#if t.files.length}
			<div class="flex flex-wrap items-center gap-1.5 text-[11.5px]">
				{#each t.files.slice(0, 4) as f (f)}<span class="max-w-[14rem] truncate rounded bg-muted px-1.5 py-0.5 font-mono text-muted-foreground" title={f}>{f.split('/').pop()}</span>{/each}
				{#if t.files.length > 4}<span class="text-muted-foreground">+{t.files.length - 4} more</span>{/if}
				<DiffStat add={t.additions} del={t.deletions} blocks={false} class="ml-auto text-[11px]" />
			</div>
		{/if}

		{#if failed}
			<div class="flex flex-col gap-2 rounded-lg border border-err/30 bg-err-soft/40 p-2.5 text-xs">
				<p class="flex items-start gap-1.5 text-err-ink">
					<TriangleAlert class="mt-px size-3.5 shrink-0" />
					<span>{i?.conflicts?.length ? 'Conflicts with work already in the room:' : `Couldn't add it to the room${i?.error ? `: ${i.error}` : '.'}`}</span>
				</p>
				{#if i?.conflicts?.length}
					<div class="flex flex-wrap gap-1">{#each i.conflicts as f (f)}<span class="rounded bg-background px-1.5 py-0.5 font-mono text-[11px]">{f}</span>{/each}</div>
				{/if}
				{#if mine}
					<div class="flex flex-wrap gap-1.5">
						{#if i?.conflicts?.length}
							<Button size="sm" variant="outline" class="h-7 rounded-full text-xs" disabled={busyTask === t.id || live(t)} onclick={() => askToFix(t)}>Ask my agent to fix</Button>
						{/if}
						<Button size="sm" variant="ghost" class="h-7 rounded-full text-xs" disabled={busyTask === t.id || live(t)} onclick={() => integrate(t)}>Try again</Button>
					</div>
				{/if}
			</div>
		{/if}

		<div class="flex flex-wrap items-center gap-2 text-xs">
			{#if adding}
				<span class="inline-flex items-center gap-1.5 text-run-ink"><span class="size-1.5 rounded-full bg-run pulse-run"></span>Adding to the room…</span>
			{:else if done}
				<span class="inline-flex items-center gap-1 text-ok-ink"><Check class="size-3.5" />In the room{#if i?.commit}&nbsp;<span class="font-mono text-muted-foreground">{i.commit.slice(0, 7)}</span>{/if}</span>
			{/if}
			{#if t.pr}
				<a href={t.pr.url} target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 text-muted-foreground hover:text-foreground"><PrIcon state={t.pr.state} />#{t.pr.number}</a>
			{/if}
			<span class="ml-auto"></span>
			{#if mine && !live(t) && !adding && !failed && t.files.length}
				{#if done}
					<button type="button" class="text-muted-foreground hover:text-foreground disabled:opacity-50" disabled={busyTask === t.id} onclick={() => integrate(t)} title="Merge anything new on this task's branch into the room">Add new changes</button>
				{:else}
					<Button size="sm" class="h-7 rounded-full text-xs" disabled={busyTask === t.id} onclick={() => integrate(t)}><GitMerge class="size-3.5" />{busyTask === t.id ? 'Adding…' : 'Add to room'}</Button>
				{/if}
			{/if}
			<a href="#/runs/{t.id}" class="text-muted-foreground hover:text-foreground">{mine ? 'Open' : 'Watch'}</a>
		</div>
	</li>
{/snippet}

{#if missing}
	<section class="mx-auto flex w-full max-w-3xl flex-col gap-3 px-4 py-16">
		<h1 class="text-xl font-medium">Room not found</h1>
		<p class="text-[13px] text-muted-foreground">{missing}</p>
		<a href="#/rooms" class="text-[13px] underline underline-offset-2">All rooms</a>
	</section>
{:else if !room}
	<section class="mx-auto flex w-full max-w-6xl flex-col gap-4 px-4 py-8 md:px-8">
		<div class="h-7 w-1/2 animate-pulse rounded bg-muted"></div>
		<div class="h-16 animate-pulse rounded bg-muted"></div>
		<div class="h-48 animate-pulse rounded-xl bg-muted"></div>
	</section>
{:else}
	<section class="mx-auto flex w-full max-w-6xl flex-col gap-6 px-4 py-6 md:px-8 md:py-8">
		<header class="flex flex-col gap-3">
			<a href="#/rooms" class="inline-flex w-fit items-center gap-1 text-xs text-muted-foreground hover:text-foreground"><ArrowLeft class="size-3.5" />Rooms</a>
			<div class="flex items-start justify-between gap-3">
				{#if editing === 'name'}
					<form class="flex flex-1 gap-2" onsubmit={(e) => { e.preventDefault(); void saveEdit(); }}>
						<!-- svelte-ignore a11y_autofocus -->
						<input bind:value={draft} maxlength={120} autofocus class="h-9 min-w-0 flex-1 rounded-lg border border-input bg-card px-3 text-[18px] font-semibold tracking-tight outline-none" aria-label="Room name" onkeydown={(e) => e.key === 'Escape' && (editing = null)} />
						<Button type="submit" size="sm" class="h-9 rounded-full">Save</Button>
					</form>
				{:else}
					<h1 class="text-[22px] leading-tight font-semibold tracking-tight max-md:text-[19px]">
						{room.name}{#if room.archived_at}<span class="ml-2 align-middle text-xs font-normal text-muted-foreground">archived</span>{/if}
					</h1>
				{/if}
				<div class="flex shrink-0 items-center gap-1.5">
					{#if compare}
						<Button href={compare} target="_blank" rel="noopener noreferrer" size="sm" variant="outline" class="h-8 rounded-full text-[13px]"><GitPullRequest class="size-3.5" />Compare on GitHub</Button>
					{/if}
					{#if canEdit}
						<DropdownMenu.Root>
							<DropdownMenu.Trigger class="grid size-8 place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground" aria-label="Room actions"><Ellipsis class="size-4" /></DropdownMenu.Trigger>
							<DropdownMenu.Content align="end" class="w-44">
								<DropdownMenu.Item onclick={() => edit('name')}>Rename</DropdownMenu.Item>
								<DropdownMenu.Item onclick={() => edit('goal')}>Edit goal</DropdownMenu.Item>
								<DropdownMenu.Separator />
								<DropdownMenu.Item onclick={() => setArchived(!room?.archived_at)}>{room.archived_at ? 'Restore room' : 'Archive room'}</DropdownMenu.Item>
							</DropdownMenu.Content>
						</DropdownMenu.Root>
					{/if}
				</div>
			</div>

			{#if editing === 'goal'}
				<form class="flex flex-col gap-2" onsubmit={(e) => { e.preventDefault(); void saveEdit(); }}>
					<!-- svelte-ignore a11y_autofocus -->
					<textarea bind:value={draft} rows="5" maxlength={4000} autofocus class="w-full resize-y rounded-lg border border-input bg-card px-3 py-2 text-[14px] leading-relaxed outline-none" aria-label="Goal"></textarea>
					<div class="flex gap-2">
						<Button type="submit" size="sm" class="h-8 rounded-full">Save goal</Button>
						<Button type="button" size="sm" variant="ghost" class="h-8 rounded-full" onclick={() => (editing = null)}>Cancel</Button>
					</div>
				</form>
			{:else if room.goal}
				<div class="group relative max-w-3xl">
					<p class="text-[14px] leading-relaxed whitespace-pre-wrap text-foreground/85">{room.goal}</p>
					{#if canEdit}
						<button type="button" class="absolute -top-1 -right-7 grid size-6 place-items-center rounded text-muted-foreground opacity-0 group-hover:opacity-100 hover:text-foreground focus-visible:opacity-100 pointer-coarse:opacity-100" aria-label="Edit goal" onclick={() => edit('goal')}><Pencil class="size-3.5" /></button>
					{/if}
				</div>
			{:else if canEdit}
				<button type="button" class="w-fit text-[13px] text-muted-foreground underline-offset-2 hover:underline" onclick={() => edit('goal')}>Add a goal: what done looks like. Every agent in the room reads it.</button>
			{/if}

			<div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-muted-foreground">
				{#if room.created_by_login}<span class="inline-flex items-center gap-1.5"><Avatar login={room.created_by_login} size={16} class="ring-0" />Started by @{room.created_by_login}</span>{/if}
				{#if repoCloneCommand(room.repo)}
					<span class="inline-flex min-w-0 items-center gap-1 rounded-md bg-muted py-0.5 pr-0.5 pl-1.5" title="Copy a git clone command for this repo">
						<span class="truncate font-mono text-[11.5px] text-foreground/80">{repoLabel(room.repo)}</span>
						<CopyButton text={() => repoCloneCommand(room?.repo)} label="" class="h-5 bg-transparent px-1 text-muted-foreground hover:bg-accent hover:text-foreground" />
					</span>
				{:else}
					<span class="font-mono text-[11.5px]">{repoLabel(room.repo)}</span>
				{/if}
				<span class="inline-flex min-w-0 items-center gap-1 rounded-md bg-muted py-0.5 pr-0.5 pl-1.5">
					<GitBranch class="size-3 shrink-0" /><span class="truncate font-mono text-[11.5px] text-foreground/80">{room.branch}</span>
					<CopyButton text={() => room?.branch ?? ''} label="" class="h-5 bg-transparent px-1 text-muted-foreground hover:bg-accent hover:text-foreground" />
				</span>
				<span>{room.integrated_at ? `${room.integrated_count} task${room.integrated_count === 1 ? '' : 's'} merged; new tasks start from this branch` : 'Nothing added yet; tasks start from the default branch'}</span>
			</div>
		</header>

		<div class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_22rem] lg:items-start">
			<!-- Tasks -->
			<div class="flex min-w-0 flex-col gap-4">
				<form class="flex flex-col gap-1.5" onsubmit={startTask}>
					<div class="relative rounded-2xl border border-input bg-card focus-within:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]">
						{#if attachments.dragging}
							<div class="pointer-events-none absolute inset-0 z-10 grid place-items-center rounded-2xl border-2 border-dashed border-primary/40 bg-card/90 text-[13px] text-muted-foreground">Drop images to attach them</div>
						{/if}
						<label for="room-task" class="sr-only">New task in this room</label>
						<textarea
							id="room-task"
							bind:this={box}
							bind:value={task}
							rows="2"
							placeholder="Take a piece of the goal: what should your agent do?"
							onkeydown={(e) => { if (prefs.sends(e, 'task')) startTask(e); }}
							onpaste={attachments.paste}
							class="block max-h-[240px] min-h-11 w-full resize-none bg-transparent px-4 pt-3 pb-1 text-[14px] leading-relaxed outline-none placeholder:text-faint"
						></textarea>
						<AttachedImages {attachments} class="px-4 pt-1 pb-2" />
						<div class="flex flex-wrap items-center gap-0.5 px-2 pb-2">
							<AgentModelPicker
								bind:agent
								bind:model
								bind:effort
								{models}
								id="room-model"
								size="sm"
								unavailable={(a) => (withRepo.some((w) => hasAgent(w, a)) ? '' : `Not on a machine of yours with ${repoLabel(room?.repo)}`)}
							/>
							<ComposerOptions {agent} perm={effectivePerm} onperm={(v) => (perm = v)} defaultPerm={prefs.permissionFor(agent, true)} {machines} machine={pinned?.id ?? ''} onmachine={(v) => (machine = v)} size="sm" />
							<button type="button" aria-label="Attach images" title="Attach images (or drop or paste them)" disabled={!canAttach} onclick={() => picker?.click()} class="ml-auto grid size-8 shrink-0 place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40">
								<Paperclip class="size-4" />
							</button>
							<input bind:this={picker} type="file" accept={IMAGE_TYPES.join(',')} multiple hidden onchange={(e) => { attachments.add(e.currentTarget.files ?? []); e.currentTarget.value = ''; }} />
							<button type="submit" aria-label="Start task" title={blocked || undefined} disabled={!canStart} class="grid size-8 shrink-0 place-items-center rounded-full bg-primary text-primary-foreground transition-opacity disabled:opacity-25">
								<ArrowUp class="size-4" strokeWidth={2.2} />
							</button>
						</div>
					</div>
					{#if !withRepo.length}
						<div class="flex gap-2.5 rounded-xl border border-amber-500/30 bg-amber-500/5 px-3 py-2.5 text-xs leading-relaxed text-muted-foreground">
							<TriangleAlert class="mt-0.5 size-3.5 shrink-0 text-amber-600 dark:text-amber-400" />
							<div class="flex min-w-0 flex-col gap-1.5">
								{#if !fleet.myOnline.length}
									<p><span class="font-medium text-foreground">None of your machines is online.</span> <a href="#/machines" class="text-foreground underline underline-offset-2">Connect one</a> with {repoLabel(room.repo)} checked out to take a task; teammates' machines only run their own.</p>
								{:else}
									<p>
										<span class="font-medium text-foreground">{fleet.myOnline.length === 1 ? `${fleet.myOnline[0].display_name} doesn't` : "None of your online machines"} {fleet.myOnline.length === 1 ? 'have' : 'has'} {repoLabel(room.repo)} checked out,</span>
										so it can't take a task here. Clone it into the folder Sidefleet scans for repos (the one passed with <code class="font-mono">--repos</code>); it shows up here within about 15 seconds, no restart needed.
									</p>
									{#if repoCloneCommand(room.repo)}
										<span class="inline-flex w-fit max-w-full min-w-0 items-center gap-1 rounded-md bg-muted py-0.5 pr-0.5 pl-1.5">
											<span class="truncate font-mono text-[11.5px] text-foreground/80">{repoCloneCommand(room.repo)}</span>
											<CopyButton text={() => repoCloneCommand(room?.repo)} label="" class="h-5 bg-transparent px-1 text-muted-foreground hover:bg-accent hover:text-foreground" />
										</span>
									{/if}
								{/if}
							</div>
						</div>
					{:else}
						<p class="px-1 text-xs text-muted-foreground">
							{#if !machines.length}
								None of your machines with {repoLabel(room.repo)} has {agentName(agent)}.
							{:else}
								Runs on {pinned?.display_name ?? (machines.length === 1 ? machines[0].display_name : 'whichever of your machines is free first')}, {room.integrated_at ? `starting from ${room.branch}` : 'starting from the default branch'}. The agent gets the goal, the notes and what the other tasks are doing.
							{/if}
						</p>
					{/if}
				</form>

				{#if !tasks.length}
					<div class="rounded-xl border border-dashed p-5 text-[13px] leading-relaxed text-muted-foreground">
						<p class="font-medium text-foreground">No tasks yet</p>
						<p class="mt-1">Split the goal into tasks. Each runs on its owner's machine; finished work is added to the room branch, and new tasks start from it.</p>
					</div>
				{:else}
					<ul class="flex flex-col gap-2.5">
						{#each sortedTasks as t (t.id)}{@render taskCard(t)}{/each}
					</ul>
				{/if}
			</div>

			<!-- Notes -->
			<aside class="flex min-w-0 flex-col rounded-xl border bg-card lg:sticky lg:top-6 lg:max-h-[calc(100dvh-3rem)]" aria-label="Room notes">
				<h2 class="border-b px-4 py-3 text-[13px] font-semibold">Notes <span class="font-normal text-muted-foreground">· agents in the room read these</span></h2>
				<div bind:this={noteList} class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 py-4 max-lg:max-h-[50vh]">
					{#if !notes.length}
						<p class="text-[13px] leading-relaxed text-muted-foreground">Decide things here: who takes what, what to avoid, what's done. Every task's agent gets the latest notes.</p>
					{/if}
					{#each notes as n (n.id)}
						<div class="group flex gap-2.5">
							<Avatar login={n.author_login} size={24} class="mt-0.5" />
							<div class="min-w-0 flex-1">
								<p class="flex items-baseline gap-2 text-xs">
									<span class="font-medium">{n.author_id && n.author_id === fleet.userId ? 'You' : n.author_login ? `@${n.author_login}` : 'Someone'}</span>
									<span class="tnum text-faint">{rel(n.created_at, fleet.now)}{#if n.edited_at}<span title={`Edited ${rel(n.edited_at, fleet.now)}`}> · edited</span>{/if}</span>
									{#if editingNote !== n.id}
										<span class="ml-auto flex items-center gap-0.5 self-center transition-opacity [@media(hover:hover)]:opacity-0 [@media(hover:hover)]:group-focus-within:opacity-100 [@media(hover:hover)]:group-hover:opacity-100">
											{#if n.author_id && n.author_id === fleet.userId}
												<button type="button" aria-label="Edit note" title="Edit note" onclick={() => editNote(n)} class="rounded p-0.5 text-faint hover:text-foreground">
													<Pencil class="size-3.5" />
												</button>
											{/if}
											{#if canDeleteNote(n)}
												<button type="button" aria-label="Delete note" title="Delete note" onclick={() => deleteNote(n)} class="rounded p-0.5 text-faint hover:text-destructive">
													<Trash2 class="size-3.5" />
												</button>
											{/if}
										</span>
									{/if}
								</p>
								{#if editingNote === n.id}
									<form class="mt-1 flex flex-col gap-1.5" onsubmit={(e) => { e.preventDefault(); void saveNote(n); }}>
										<!-- svelte-ignore a11y_autofocus -->
										<textarea
											bind:value={noteDraft}
											autofocus
											rows="3"
											maxlength={4000}
											aria-label="Edit note"
											onkeydown={(e) => {
												if (e.key === 'Escape') editingNote = null;
												else if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); void saveNote(n); }
											}}
											class="block w-full resize-y rounded-lg bg-muted/60 px-2.5 py-1.5 text-[13.5px] leading-relaxed outline-none focus:bg-muted"
										></textarea>
										<div class="flex justify-end gap-1.5">
											<Button type="button" variant="ghost" size="sm" onclick={() => (editingNote = null)}>Cancel</Button>
											<Button type="submit" size="sm" disabled={!noteDraft.trim() || savingNote}>Save</Button>
										</div>
									</form>
								{:else}
									<p class="mt-0.5 text-[13.5px] leading-relaxed break-words whitespace-pre-wrap">{n.body}</p>
								{/if}
							</div>
						</div>
					{/each}
				</div>
				<form class="flex items-end gap-2 border-t p-2.5" onsubmit={postNote}>
					<label for="room-note" class="sr-only">Note</label>
					<textarea
						id="room-note"
						bind:this={noteBox}
						bind:value={note}
						rows="1"
						maxlength={4000}
						placeholder="Write a note to the room…"
						onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey) postNote(e); }}
						class="block max-h-40 min-h-9 flex-1 resize-none rounded-lg bg-muted/60 px-3 py-2 text-[13.5px] leading-relaxed outline-none placeholder:text-faint focus:bg-muted"
					></textarea>
					<button type="submit" aria-label="Post note" disabled={!note.trim() || posting} class="grid size-8 shrink-0 place-items-center rounded-full bg-primary text-primary-foreground transition-opacity disabled:opacity-25">
						<ArrowUp class="size-4" strokeWidth={2.2} />
					</button>
				</form>
			</aside>
		</div>
	</section>
{/if}
