<script lang="ts">
	import { goto } from '$app/navigation';
	import * as Select from '#lib/components/ui/select';
	import { Kbd } from '#lib/components/ui/kbd';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import { toast } from 'svelte-sonner';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { MODELS, PERMISSIONS, WORKSPACES, modelName, rel, duration, repoKey, repoLabel } from '#lib/sf/format';
	import AddMachine from '#lib/sf/ui/AddMachine.svelte';
	import StatusDot from '#lib/sf/ui/StatusDot.svelte';

	const PICK = '__pick__';
	const ANY = '__any__';
	let task = $state('');
	let model = $state(MODELS.some((m) => m.id === load('sidefleet_model')) ? load('sidefleet_model') : 'claude-sonnet-5-5');
	let machine = $state(load('sidefleet_machine'));
	let perm = $state('acceptEdits');
	let iso = $state('worktree');
	let repo = $state(load('sidefleet_repo'));
	let busy = $state(false);
	let box: HTMLTextAreaElement | undefined = $state();

	function load(k: string) { try { return localStorage.getItem(k) ?? ''; } catch { return ''; } }
	function save(k: string, v: string) { try { localStorage.setItem(k, v); } catch { /* ignore */ } }

	const repos = $derived(fleet.composerRepos);
	const online = $derived(fleet.onlineRepos);
	// Keep the selection valid as machines come and go.
	$effect(() => {
		if (repo && repos.includes(repo) && online.has(repo)) return;
		// A repo remembered by folder name (before identities): switch to its identity.
		const renamed = fleet.workers.flatMap((w) => w.capabilities.repos ?? []).find((r) => r.name === repo && repoKey(r) !== repo);
		if (renamed && repos.includes(repoKey(renamed))) { repo = repoKey(renamed); save('sidefleet_repo', repo); return; }
		const first = repos.find((r) => online.has(r));
		if (first) repo = first;
	});
	/** Online machines that have the repo; the menu only shows when there is a choice. */
	const machines = $derived(repo ? fleet.online.filter((w) => (w.capabilities.repos ?? []).some((r) => repoKey(r) === repo)) : []);
	// The remembered machine applies only where it has the repo; otherwise any machine.
	const pinned = $derived(machines.find((w) => w.id === machine));
	const target = $derived(pinned ?? (repo ? fleet.workerFor(repo) : undefined));
	const claude = $derived(target?.capabilities.agents.claude);
	const canRun = $derived(!!task.trim() && !!repo && online.has(repo) && !busy);

	function pickRepo(v: string) {
		if (v === PICK) { fleet.pickerOpen = true; return; }
		repo = v;
		save('sidefleet_repo', v);
	}

	function pickModel(v: string) {
		model = v;
		save('sidefleet_model', v);
	}

	function pickMachine(v: string) {
		machine = v === ANY ? '' : v;
		save('sidefleet_machine', machine);
	}

	async function run(e?: Event) {
		e?.preventDefault();
		if (!canRun) return;
		busy = true;
		try {
			const r = await api<{ id: string }>('/v1/runs', {
				method: 'POST',
				body: JSON.stringify({ task: task.trim(), model, repo, permission_mode: perm, isolation: iso, worker_id: pinned?.id })
			});
			task = '';
			void fleet.refresh();
			await goto('#/runs/' + r.id);
		} catch (err) {
			toast.error('Could not start the task', { description: (err as Error).message });
		}
		busy = false;
	}

	function keydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) run(e);
	}
	function grow() {
		if (!box) return;
		box.style.height = 'auto';
		box.style.height = Math.min(window.innerHeight * 0.55, Math.max(88, box.scrollHeight)) + 'px';
	}
	$effect(() => { void task; grow(); });

	const recent = $derived(fleet.runs.slice(0, 6));
	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
	const noMachines = $derived(fleet.loaded && fleet.online.length === 0);
	const examples = [
		'Find why the login test is flaky and fix it',
		'Summarise what changed on main this week',
		'Add a dark mode toggle to the settings page'
	];
</script>

<svelte:head><title>Sidefleet</title></svelte:head>

{#if !fleet.loaded}
	<div class="mx-auto mt-[18vh] w-full max-w-2xl px-4">
		<div class="h-8 w-56 animate-pulse rounded-md bg-muted"></div>
		<div class="mt-6 h-40 animate-pulse rounded-2xl bg-muted"></div>
	</div>
{:else if noMachines}
	<section class="mx-auto flex w-full max-w-2xl flex-col gap-8 px-4 pt-[10vh] pb-16">
		<header class="flex flex-col gap-3">
			<h1 class="text-[28px] leading-tight font-medium tracking-tight">
				{fleet.workers.length ? 'Your machines are offline' : 'Put your first machine to work'}
			</h1>
			<p class="max-w-xl text-[15px] leading-relaxed text-muted-foreground">
				{#if fleet.workers.length}
					{fleet.workers.map((w) => w.display_name).join(', ')} {fleet.workers.length === 1 ? 'has' : 'have'} stopped checking in.
					Run <code class="font-mono text-[13px] text-foreground">sidefleet status</code> there, or connect another machine below.
				{:else}
					Sidefleet runs tasks with the Claude Code that's already signed in on your own machines, and streams what it does back here.
					Connect a laptop, a Mac mini or a VPS; it needs no open ports.
				{/if}
			</p>
		</header>
		<div class="rounded-2xl border bg-card p-5 shadow-[0_1px_2px_rgb(0_0_0/0.03)]">
			<AddMachine />
		</div>
		<p class="flex items-center gap-2 text-[13px] text-muted-foreground" role="status">
			<StatusDot status="queued" class="pulse-run" />Waiting for a machine to check in. This page updates by itself.
		</p>
	</section>
{:else}
	<section class="mx-auto flex w-full max-w-2xl flex-col gap-10 px-4 pt-[14vh] pb-16">
		<form class="flex flex-col gap-3" onsubmit={run}>
			<h1 class="text-center text-[28px] leading-tight font-medium tracking-tight">What should run?</h1>
			<div class="rounded-2xl border border-input bg-card shadow-[0_1px_3px_rgb(0_0_0/0.04)] transition-shadow focus-within:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]">
				<label for="task" class="sr-only">Task</label>
				<textarea
					id="task"
					bind:this={box}
					bind:value={task}
					onkeydown={keydown}
					rows="3"
					placeholder="Describe the task. It runs in a repo on one of your machines."
					class="block max-h-[55vh] min-h-[88px] w-full resize-none bg-transparent px-4 pt-4 pb-2 text-[15px] leading-relaxed outline-none placeholder:text-faint"
				></textarea>
				<div class="flex items-end gap-2 px-3 pb-3">
					<div class="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
					<Select.Root type="single" value={model} onValueChange={pickModel}>
						<Select.Trigger size="sm" class="h-8 rounded-full border-transparent bg-muted text-[13px] shadow-none hover:bg-accent" aria-label="Model">{modelName(model)}</Select.Trigger>
						<Select.Content>
							{#each MODELS as m (m.id)}<Select.Item value={m.id} label={m.name}><span class="flex flex-col"><span>{m.name}</span><span class="text-xs text-muted-foreground">{m.hint}</span></span></Select.Item>{/each}
						</Select.Content>
					</Select.Root>

					<Select.Root type="single" value={repo} onValueChange={pickRepo}>
						<Select.Trigger size="sm" class="h-8 max-w-[15rem] rounded-full border-transparent bg-muted font-mono text-[12.5px] shadow-none hover:bg-accent" aria-label="Repo">
							<span class="truncate">{repoLabel(repo) || 'Choose a repo'}</span>
						</Select.Trigger>
						<Select.Content class="max-h-80">
							{#each repos as r (r)}
								<Select.Item value={r} label={repoLabel(r)} disabled={!online.has(r)}><span class="font-mono text-[12.5px]">{repoLabel(r)}</span>{#if !online.has(r)}<span class="ml-2 text-xs text-muted-foreground">offline</span>{/if}</Select.Item>
							{/each}
							{#if repos.length}<Select.Separator />{/if}
							<Select.Item value={PICK} label="Choose repos">{repos.length ? 'Add or remove repos…' : `Choose from ${online.size} repos…`}</Select.Item>
						</Select.Content>
					</Select.Root>

					{#if machines.length > 1}
						<Select.Root type="single" value={pinned?.id ?? ANY} onValueChange={pickMachine}>
							<Select.Trigger size="sm" class="h-8 max-w-[12rem] rounded-full border-transparent bg-muted text-[13px] shadow-none hover:bg-accent" aria-label="Machine">
								<span class="truncate">{pinned?.display_name ?? 'Any machine'}</span>
							</Select.Trigger>
							<Select.Content>
								<Select.Item value={ANY} label="Any machine"><span class="flex flex-col"><span>Any machine</span><span class="text-xs text-muted-foreground">The first one with a free slot</span></span></Select.Item>
								<Select.Separator />
								{#each machines as w (w.id)}
									<Select.Item value={w.id} label={w.display_name}>
										<span class="flex flex-col"><span>{w.display_name}</span>{#if w.capabilities.slots}<span class="tnum text-xs text-muted-foreground">{w.capabilities.slots.active} of {w.capabilities.slots.parallel} slots busy</span>{/if}</span>
									</Select.Item>
								{/each}
							</Select.Content>
						</Select.Root>
					{/if}

					<Select.Root type="single" value={perm} onValueChange={(v) => (perm = v)}>
						<Select.Trigger size="sm" class="h-8 rounded-full border-transparent bg-muted text-[13px] shadow-none hover:bg-accent" aria-label="Permissions">{PERMISSIONS.find((p) => p.id === perm)?.name}</Select.Trigger>
						<Select.Content>
							{#each PERMISSIONS as p (p.id)}<Select.Item value={p.id} label={p.name}><span class="flex flex-col"><span>{p.name}</span><span class="text-xs text-muted-foreground">{p.hint}</span></span></Select.Item>{/each}
						</Select.Content>
					</Select.Root>

					<Select.Root type="single" value={iso} onValueChange={(v) => (iso = v)}>
						<Select.Trigger size="sm" class="h-8 rounded-full border-transparent bg-muted text-[13px] shadow-none hover:bg-accent" aria-label="Workspace">{WORKSPACES.find((w) => w.id === iso)?.name}</Select.Trigger>
						<Select.Content>
							{#each WORKSPACES as w (w.id)}<Select.Item value={w.id} label={w.name}><span class="flex flex-col"><span>{w.name}</span><span class="text-xs text-muted-foreground">{w.hint}</span></span></Select.Item>{/each}
						</Select.Content>
					</Select.Root>

					</div>
					<span class="flex shrink-0 items-center gap-2">
						{#if task.length > 2000}<span class="tnum text-xs text-faint">{task.length.toLocaleString()} characters</span>{/if}
						<span class="hidden items-center gap-1 text-xs text-faint sm:flex"><Kbd>{isMac ? '⌘' : 'Ctrl'}</Kbd><Kbd>↵</Kbd></span>
						<button type="submit" disabled={!canRun} aria-label="Run task" class="grid size-9 place-items-center rounded-full bg-primary text-primary-foreground transition-opacity disabled:opacity-25">
							<ArrowUp class="size-4" strokeWidth={2.2} />
						</button>
					</span>
				</div>
			</div>
			<p class="min-h-5 text-center text-xs text-muted-foreground">
				{#if !repos.length}
					Your machines have {online.size} git checkouts. <button type="button" class="text-foreground underline underline-offset-2" onclick={() => (fleet.pickerOpen = true)}>Choose the ones you work on</button> to fill the repo menu.
				{:else if target}
					{#if !pinned && machines.length > 1}
						Runs on whichever of the {machines.length} machines with this repo is free first
					{:else}
						Runs on <span class="text-foreground">{target.display_name}</span>{#if claude && !claude.signed_in}, <span class="text-err-ink">where Claude Code is not signed in</span>{/if}
					{/if}{iso === 'worktree' ? ', on a new branch so your checkout stays untouched.' : ', directly in the checkout.'}
				{/if}
			</p>
		</form>

		{#if recent.length}
			<div class="flex flex-col">
				<div class="flex items-baseline justify-between px-1 pb-2">
					<h2 class="text-[13px] font-medium text-muted-foreground">Recent</h2>
					<a href="#/runs" class="text-xs text-muted-foreground hover:text-foreground">All runs</a>
				</div>
				<ul class="flex flex-col">
					{#each recent as r (r.id)}
						<li>
							<a href="#/runs/{r.id}" class="group flex items-center gap-3 rounded-lg px-2 py-2.5 text-[13px] hover:bg-accent/60">
								<StatusDot status={r.thread_status} />
								<span class="min-w-0 flex-1 truncate">{r.task}</span>
								{#if r.follow_ups}<span class="tnum shrink-0 text-xs text-muted-foreground">{r.follow_ups + 1} messages</span>{/if}
								<span class="hidden shrink-0 font-mono text-[12px] text-muted-foreground sm:inline">{repoLabel(r.repo)}</span>
								<span class="tnum w-16 shrink-0 text-right text-xs text-muted-foreground">
									{r.thread_status === 'running' ? duration(r.started_at, null, fleet.now) : rel(r.last_activity_at, fleet.now)}
								</span>
							</a>
						</li>
					{/each}
				</ul>
			</div>
		{:else}
			<div class="flex flex-wrap justify-center gap-2">
				{#each examples as ex}
					<button type="button" class="rounded-full border px-3 py-1.5 text-[13px] text-muted-foreground hover:bg-accent hover:text-foreground" onclick={() => { task = ex; box?.focus(); }}>{ex}</button>
				{/each}
			</div>
		{/if}
	</section>
{/if}
