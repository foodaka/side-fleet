<script lang="ts">
	// Every agent in the org: the ones Sidefleet runs, found from its machines, and the ones that registered.
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { Textarea } from '#lib/components/ui/textarea';
	import * as Select from '#lib/components/ui/select';
	import Plus from '@lucide/svelte/icons/plus';
	import Search from '@lucide/svelte/icons/search';
	import { fleet } from '#lib/sf/state.svelte';
	import { rel } from '#lib/sf/format';
	import { listAgents, registerAgent, kindName, RISK, STATUS, writeError, type Agent, type AgentStatus, type Risk } from '#lib/sf/control';
	import { control } from '#lib/sf/control.svelte';
	import Chip from '#lib/sf/ui/control/Chip.svelte';
	import PageHead from '#lib/sf/ui/control/PageHead.svelte';
	import Pills from '#lib/sf/ui/control/Pills.svelte';
	import CopyButton from '#lib/sf/ui/CopyButton.svelte';

	let agents = $state<Agent[] | null>(null);
	let error = $state('');
	let query = $state('');
	let filter = $state<'all' | AgentStatus>('all');

	async function load() {
		try {
			agents = await listAgents();
			error = '';
		} catch (e) {
			error = (e as Error).message;
		}
	}
	onMount(() => {
		void load();
		const t = setInterval(load, 15_000);
		return () => clearInterval(t);
	});

	const count = (s: AgentStatus) => (agents ?? []).filter((a) => a.status === s).length;
	const pills = $derived([
		{ id: 'all', label: 'All', n: agents?.length ?? 0 },
		{ id: 'running', label: 'Running', n: count('running') },
		{ id: 'waiting', label: 'Waiting', n: count('waiting') },
		{ id: 'idle', label: 'Idle', n: count('idle') },
		{ id: 'suspended', label: 'Suspended', n: count('suspended') }
	]);
	const rows = $derived.by(() => {
		const q = query.trim().toLowerCase();
		return (agents ?? []).filter(
			(a) => (filter === 'all' || a.status === filter) && (!q || `${a.name} ${a.owner?.login ?? ''} ${a.id} ${a.team}`.toLowerCase().includes(q))
		);
	});

	// ---- register ----
	let open = $state(false);
	let busy = $state(false);
	let key = $state('');
	let form = $state({ name: '', description: '', kind: 'custom', provider: '', model: '', runtime: '', host: '', environment: '', team: '', risk: 'unknown' as Risk });
	function startRegister() {
		key = '';
		form = { name: '', description: '', kind: 'custom', provider: '', model: '', runtime: '', host: '', environment: '', team: '', risk: 'unknown' };
		open = true;
	}
	async function register(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const r = await registerAgent({ ...form, name: form.name.trim(), kind: form.kind as Agent['kind'] });
			key = r.key;
			toast.success(`Registered ${r.agent.name}`);
			await load();
			void control.refresh();
		} catch (err) {
			toast.error('Could not register the agent', { description: writeError(err) });
		}
		busy = false;
	}
	const cols = 'grid grid-cols-[minmax(170px,1.6fr)_minmax(120px,1.1fr)_minmax(120px,1.1fr)_minmax(120px,1fr)_minmax(150px,1fr)_minmax(80px,.7fr)_minmax(70px,.6fr)] items-center gap-3 min-w-[900px]';
</script>

<svelte:head><title>Agents · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-4.5 px-4 pt-8 pb-[calc(3rem+env(safe-area-inset-bottom))] md:px-8">
	<PageHead title="Agents">
		{#snippet sub()}Agents launched through Sidefleet register themselves. Others register with <span class="font-mono">POST /v1/agents</span>.{/snippet}
		{#snippet actions()}<Button class="h-8 text-[13px]" onclick={startRegister}><Plus class="size-3.5" />Register agent</Button>{/snippet}
	</PageHead>

	<div class="flex flex-wrap items-center gap-2.5">
		<label class="relative flex w-75 max-w-full items-center text-muted-foreground">
			<Search class="pointer-events-none absolute left-2.5 size-3.5" />
			<Input type="search" bind:value={query} placeholder="Filter by name, owner, team or ID" aria-label="Filter agents" class="h-8.5 bg-card pl-8 text-[13px]" />
		</label>
		<Pills options={pills} bind:value={filter} label="Status" />
	</div>

	{#if error && !agents}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">Could not load agents: {error}</p>
	{:else}
		<div class="overflow-x-auto rounded-xl border bg-card">
			<div class="{cols} px-4 py-2.5 text-[11.5px] font-medium text-muted-foreground"><span>Agent</span><span>Owner</span><span>Runtime · host</span><span>Model</span><span>Status</span><span>Last activity</span><span>Risk</span></div>
			{#if agents === null}
				<div class="border-t px-4 py-8 text-center text-[13px] text-muted-foreground">Loading…</div>
			{/if}
			{#each rows as a (a.id)}
				<div class="{cols} border-t px-4 py-2.5 text-[13px]">
					<div class="min-w-0"><a class="font-medium hover:underline" href="#/control/agents/{a.id}">{a.name}</a><div class="truncate font-mono text-[12px] text-muted-foreground">{a.id.slice(0, 8)}</div></div>
					<div class="min-w-0"><div class="truncate {a.owner ? '' : 'text-queue-ink'}">{a.owner?.login ?? 'Unassigned'}</div><div class="truncate text-[12px] text-muted-foreground">{a.team || 'No team'}</div></div>
					<div class="min-w-0"><div class="truncate">{kindName(a.kind)}</div><div class="truncate text-[12px] text-muted-foreground">{a.host || a.runtime}</div></div>
					<span class="truncate font-mono text-[12px]">{a.model || '–'}</span>
					<span><Chip tone={STATUS[a.status].tone}>{STATUS[a.status].label}</Chip></span>
					<span class="text-muted-foreground">{a.last_active_at ? rel(a.last_active_at, fleet.now) : 'Never'}</span>
					<span><Chip tone={RISK[a.risk].tone} dot={false}>{RISK[a.risk].label}</Chip></span>
				</div>
			{/each}
			{#if agents && !rows.length}
				<div class="border-t px-4 py-8 text-center text-[13px] text-muted-foreground">
					{agents.length ? 'No agents match that filter.' : 'No agents yet. They appear here when a machine connects, or when one registers.'}
				</div>
			{/if}
		</div>
	{/if}
</section>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-[32rem]">
		<Dialog.Header>
			<Dialog.Title>{key ? 'Agent registered' : 'Register an agent'}</Dialog.Title>
			<Dialog.Description>
				{#if key}Give the agent this key. It calls the gateway with it (<span class="font-mono">Bearer sfk_…</span>). It is shown once; only its digest is stored.{:else}For an agent Sidefleet doesn't run, such as a bot in your own cluster. It gets a key for the execution gateway, and you become its owner.{/if}
			</Dialog.Description>
		</Dialog.Header>
		{#if key}
			<div class="flex items-center gap-2 rounded-md border bg-background px-2.5 py-1.5">
				<code class="min-w-0 flex-1 truncate font-mono text-[12px]">{key}</code>
				<CopyButton text={() => key} class="bg-primary text-primary-foreground hover:bg-primary/90" />
			</div>
			<Button class="h-9" onclick={() => (open = false)}>Done</Button>
		{:else}
			<form onsubmit={register} class="flex flex-col gap-3">
				<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Name<Input bind:value={form.name} required maxlength={120} class="h-9" placeholder="support-triage-bot" /></label>
				<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Description<Textarea bind:value={form.description} rows={2} maxlength={500} placeholder="What it does" /></label>
				<div class="grid grid-cols-2 gap-2.5">
					<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Team<Input bind:value={form.team} class="h-9" placeholder="Support Eng" /></label>
					<div class="flex flex-col gap-1 text-[12px] text-muted-foreground">Risk
						<Select.Root type="single" bind:value={form.risk}>
							<Select.Trigger class="h-9 w-full" aria-label="Risk">{RISK[form.risk].label}</Select.Trigger>
							<Select.Content>
								{#each Object.entries(RISK) as [id, r] (id)}<Select.Item value={id} label={r.label}>{r.label}</Select.Item>{/each}
							</Select.Content>
						</Select.Root>
					</div>
					<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Model provider<Input bind:value={form.provider} class="h-9" placeholder="Anthropic" /></label>
					<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Model<Input bind:value={form.model} class="h-9 font-mono" placeholder="claude-haiku-5-5" /></label>
					<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Runtime<Input bind:value={form.runtime} class="h-9" placeholder="Kubernetes" /></label>
					<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Host<Input bind:value={form.host} class="h-9 font-mono" placeholder="k8s-agents" /></label>
					<label class="col-span-2 flex flex-col gap-1 text-[12px] text-muted-foreground">Environment<Input bind:value={form.environment} class="h-9" placeholder="Production" /></label>
				</div>
				<Button type="submit" class="h-9" disabled={busy || !form.name.trim()}>{busy ? 'Registering…' : 'Register agent'}</Button>
			</form>
		{/if}
	</Dialog.Content>
</Dialog.Root>
