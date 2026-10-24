<script lang="ts">
	import { goto } from '$app/navigation';
	import * as Select from '#lib/components/ui/select';
	import { Kbd } from '#lib/components/ui/kbd';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import ArrowRightLeft from '@lucide/svelte/icons/arrow-right-left';
	import Paperclip from '@lucide/svelte/icons/paperclip';
	import Cpu from '@lucide/svelte/icons/cpu';
	import MemoryStick from '@lucide/svelte/icons/memory-stick';
	import HardDrive from '@lucide/svelte/icons/hard-drive';
	import { toast } from 'svelte-sonner';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { AGENTS, MODELS, PERMISSIONS, WORKSPACES, permsFor, agentName, hasAgent, machineModels, modelValid, permHint, rel, duration, repoKey, repoLabel } from '#lib/sf/format';
	import type { AgentId } from '#lib/sf/types';
	import AddMachine from '#lib/sf/ui/AddMachine.svelte';
	import AttachedImages from '#lib/sf/ui/AttachedImages.svelte';
	import { Attachments, IMAGE_TYPES } from '#lib/sf/attach.svelte';
	import AgentModelPicker from '#lib/sf/ui/AgentModelPicker.svelte';
	import StatusDot from '#lib/sf/ui/StatusDot.svelte';

	const PICK = '__pick__';
	const ANY = '__any__';
	let task = $state('');
	const savedAgent: AgentId = AGENTS.some((a) => a.id === load('sidefleet_agent')) ? (load('sidefleet_agent') as AgentId) : 'claude';
	let agent = $state<AgentId>(savedAgent);
	// One remembered model per agent; the Claude one keeps its old key.
	const modelKey = (a: AgentId) => (a === 'claude' ? 'sidefleet_model' : `sidefleet_model_${a}`);
	const initialModel = (a: AgentId) => {
		const m = load(modelKey(a));
		return a === 'claude' ? (MODELS.some((x) => x.id === m) ? m : 'claude-sonnet-5-5') : m;
	};
	let model = $state(initialModel(savedAgent));
	let machine = $state(load('sidefleet_machine'));
	let perm = $state(PERMISSIONS.some((p) => p.id === load('sidefleet_perm')) ? load('sidefleet_perm') : 'acceptEdits');
	let iso = $state(WORKSPACES.some((w) => w.id === load('sidefleet_iso')) ? load('sidefleet_iso') : 'worktree');
	// Only Claude Code can ask you about tool calls; the others fall back to editing files.
	const effectivePerm = $derived(permsFor(agent).some((p) => p.id === perm) ? perm : 'acceptEdits');
	let repo = $state(load('sidefleet_repo'));
	// Off by default and not remembered: it pushes the task's branch to origin.
	let failover = $state(false);
	let busy = $state(false);
	let box: HTMLTextAreaElement | undefined = $state();
	let picker: HTMLInputElement | undefined = $state();
	const attachments = new Attachments();

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
	/** Online machines that have the repo. */
	const withRepo = $derived(repo ? fleet.online.filter((w) => (w.capabilities.repos ?? []).some((r) => repoKey(r) === repo)) : []);
	/** ...and the agent; the machine menu only shows when there is a choice. */
	const machines = $derived(withRepo.filter((w) => hasAgent(w, agent)));
	// The remembered machine applies only where it has the repo and agent; otherwise any machine.
	const pinned = $derived(machines.find((w) => w.id === machine));
	const target = $derived(pinned ?? machines[0]);
	const models = $derived(machineModels(pinned ? [pinned] : machines, agent));
	const modelOk = $derived(modelValid(agent, model));
	const canRun = $derived(!!task.trim() && !!repo && machines.length > 0 && modelOk && !busy);
	/** Live load of the machine the task lands on; only when there is one, not "whichever is free first". */
	const stats = $derived(pinned || machines.length === 1 ? target?.capabilities.system : undefined);
	/** Why the chosen machine may fail the task, if it can tell. */
	const warning = $derived.by(() => {
		const a = target?.capabilities.agents;
		if (!a) return '';
		if (agent === 'claude' && a.claude && !a.claude.signed_in) return 'where Claude Code is not signed in';
		if (agent === 'codex' && a.codex?.signed_in === false) return 'where Codex is not signed in (run `codex login` there)';
		if (agent === 'opencode' && a.opencode && !a.opencode.models_total) return 'where opencode has no provider set up (run `opencode auth login` there)';
		return '';
	});

	function pickRepo(v: string) {
		if (v === PICK) { fleet.pickerOpen = true; return; }
		repo = v;
		save('sidefleet_repo', v);
	}

	// When no machine with this repo has the chosen agent, switch to one that does (not remembered).
	$effect(() => {
		if (!withRepo.length || machines.length) return;
		const other = AGENTS.find((a) => withRepo.some((w) => hasAgent(w, a.id)));
		if (other && other.id !== agent) { agent = other.id; model = initialModel(other.id); }
	});

	function pickAgent(a: AgentId) {
		save('sidefleet_agent', a);
		model = initialModel(a);
	}

	function pickModel(m: string) {
		save(modelKey(agent), m);
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
				body: JSON.stringify({ task: task.trim(), agent, model, repo, permission_mode: effectivePerm, isolation: iso, worker_id: pinned?.id, failover: iso === 'worktree' && failover, images: await attachments.payload() })
			});
			task = '';
			attachments.clear();
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
<svelte:window ondragenter={attachments.dragenter} ondragleave={attachments.dragleave} ondragover={attachments.dragover} ondrop={attachments.drop} />

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
					Sidefleet runs tasks with the coding agents already set up on your own machines (Claude Code, Codex or opencode) and streams what they do back here.
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
			<div class="relative rounded-2xl border border-input bg-card shadow-[0_1px_3px_rgb(0_0_0/0.04)] transition-shadow focus-within:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]">
				{#if attachments.dragging}
					<div class="pointer-events-none absolute inset-0 z-10 grid place-items-center rounded-2xl border-2 border-dashed border-primary/40 bg-card/90 text-[14px] text-muted-foreground">Drop images to attach them</div>
				{/if}
				<label for="task" class="sr-only">Task</label>
				<textarea
					id="task"
					bind:this={box}
					bind:value={task}
					onkeydown={keydown}
					onpaste={attachments.paste}
					rows="3"
					placeholder="Describe the task. It runs in a repo on one of your machines."
					class="block max-h-[55vh] min-h-[88px] w-full resize-none bg-transparent px-4 pt-4 pb-2 text-[15px] leading-relaxed outline-none placeholder:text-faint"
				></textarea>
				<AttachedImages {attachments} class="px-4 pt-1 pb-3" />
				<div class="flex items-end gap-2 px-3 pb-3">
					<div class="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
					<AgentModelPicker
						bind:agent
						bind:model
						{models}
						unavailable={(a) => (repo && !withRepo.some((w) => hasAgent(w, a)) ? `Not on a machine with ${repoLabel(repo)}` : '')}
						onagent={pickAgent}
						onmodel={pickModel}
					/>

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

					<Select.Root type="single" value={effectivePerm} onValueChange={(v) => { perm = v; save('sidefleet_perm', v); }}>
						<Select.Trigger size="sm" class="h-8 rounded-full border-transparent bg-muted text-[13px] shadow-none hover:bg-accent" aria-label="Permissions">{PERMISSIONS.find((p) => p.id === effectivePerm)?.name}</Select.Trigger>
						<Select.Content>
							{#each permsFor(agent) as p (p.id)}<Select.Item value={p.id} label={p.name}><span class="flex flex-col"><span>{p.name}</span><span class="text-xs text-muted-foreground">{permHint(p.id, agent)}</span></span></Select.Item>{/each}
						</Select.Content>
					</Select.Root>

					<Select.Root type="single" value={iso} onValueChange={(v) => { iso = v; save('sidefleet_iso', v); }}>
						<Select.Trigger size="sm" class="h-8 rounded-full border-transparent bg-muted text-[13px] shadow-none hover:bg-accent" aria-label="Workspace">{WORKSPACES.find((w) => w.id === iso)?.name}</Select.Trigger>
						<Select.Content>
							{#each WORKSPACES as w (w.id)}<Select.Item value={w.id} label={w.name}><span class="flex flex-col"><span>{w.name}</span><span class="text-xs text-muted-foreground">{w.hint}</span></span></Select.Item>{/each}
						</Select.Content>
					</Select.Root>

					{#if iso === 'worktree'}
						<button
							type="button"
							aria-pressed={failover}
							onclick={() => (failover = !failover)}
							title="Move to another machine if this one drops. Pushes checkpoints of the task's branch to origin while it runs."
							class="inline-flex h-8 items-center gap-1.5 rounded-full px-3 text-[13px] {failover ? 'bg-primary/10 text-foreground ring-1 ring-primary/25' : 'bg-muted text-muted-foreground hover:bg-accent hover:text-foreground'}"
						>
							<ArrowRightLeft class="size-3.5" />Move if it drops
						</button>
					{/if}

					</div>
					<span class="flex shrink-0 items-center gap-2">
						{#if task.length > 2000}<span class="tnum text-xs text-faint">{task.length.toLocaleString()} characters</span>{/if}
						<button type="button" aria-label="Attach images" title="Attach images (or drop or paste them)" onclick={() => picker?.click()} class="grid size-8 place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground">
							<Paperclip class="size-4" />
						</button>
						<input bind:this={picker} type="file" accept={IMAGE_TYPES.join(',')} multiple hidden onchange={(e) => { attachments.add(e.currentTarget.files ?? []); e.currentTarget.value = ''; }} />
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
				{:else if repo && online.has(repo) && !target}
					None of the machines with {repoLabel(repo)} has {agentName(agent)}. Install it there, or pick another agent.
				{:else if target}
					{#if !pinned && machines.length > 1}
						Runs with {agentName(agent)} on whichever of the {machines.length} machines with this repo is free first
					{:else}
						Runs with {agentName(agent)} on <span class="text-foreground">{target.display_name}</span>{#if warning}, <span class="text-err-ink">{warning}</span>{/if}
					{/if}{iso === 'worktree' ? ', on a new branch so your checkout stays untouched.' : ', directly in the checkout.'}
					{#if iso === 'worktree' && failover}<br />If that machine drops, the task moves to another one; its branch is pushed to origin as it goes.{/if}
				{/if}
			</p>
			{#if stats}
				<div class="tnum -mt-1 flex flex-wrap justify-center gap-1.5 text-xs text-muted-foreground" aria-label="{target?.display_name} right now">
					<span class="inline-flex h-6 items-center gap-1.5 rounded-full border bg-card px-2.5" title="CPU busy, {stats.cpu_cores} cores"><Cpu class="size-3.5" />{Math.round(stats.cpu_percent)}% of {stats.cpu_cores} cores</span>
					<span class="inline-flex h-6 items-center gap-1.5 rounded-full border bg-card px-2.5" title="Memory in use"><MemoryStick class="size-3.5" />{(stats.mem_used_mb / 1024).toFixed(1)} / {Math.round(stats.mem_total_mb / 1024)} GB</span>
					<span class="inline-flex h-6 items-center gap-1.5 rounded-full border bg-card px-2.5" title="Free disk where the repos live"><HardDrive class="size-3.5" />{Math.round(stats.disk_free_gb).toLocaleString()} GB free</span>
				</div>
			{/if}
		</form>

		{#if recent.length}
			<!-- On wider screens the sidebar lists these. -->
			<div class="flex flex-col md:hidden">
				<div class="flex items-baseline justify-between px-1 pb-2">
					<h2 class="text-[13px] font-medium text-muted-foreground">Recent</h2>
					<a href="#/runs" class="text-xs text-muted-foreground hover:text-foreground">Overview</a>
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
