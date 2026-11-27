<script lang="ts">
	// What agents may do: policies checked at the tool-call boundary, lowest priority number first.
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { Textarea } from '#lib/components/ui/textarea';
	import * as Select from '#lib/components/ui/select';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { fleet } from '#lib/sf/state.svelte';
	import {
		createPolicy,
		deletePolicy,
		evaluate,
		listAgents,
		listPolicies,
		updatePolicy,
		writeError,
		DECISION,
		SCOPE,
		type Agent,
		type Decision,
		type Evaluation,
		type Policy,
		type PolicyInput,
		type PolicyScope
	} from '#lib/sf/control';
	import Chip from '#lib/sf/ui/control/Chip.svelte';
	import PageHead from '#lib/sf/ui/control/PageHead.svelte';
	import Switch from '#lib/sf/ui/Switch.svelte';

	let policies = $state<Policy[] | null>(null);
	let agents = $state<Agent[]>([]);
	let error = $state('');
	/** The policy in the editor; '' is a new one. */
	let selected = $state<string | null>(null);
	let draft = $state<PolicyInput & { tools_text: string }>(blank());
	let saving = $state(false);

	function blank(): PolicyInput & { tools_text: string } {
		return { name: '', description: '', scope: 'org', applies_to: '', tools: [], tools_text: '', category: '', decision: 'require_approval', priority: 50, active: true };
	}
	function pick(p: Policy) {
		selected = p.id;
		draft = { name: p.name, description: p.description, scope: p.scope, applies_to: p.applies_to, tools: p.tools, tools_text: p.tools.join('\n'), category: p.category, decision: p.decision, priority: p.priority, active: p.active };
	}
	function startNew() {
		selected = '';
		draft = blank();
	}

	async function load() {
		try {
			const ps = await listPolicies();
			policies = [...ps].sort((a, b) => a.priority - b.priority);
			error = '';
			if (selected === null) {
				const first = policies.find((p) => p.decision === 'require_approval') ?? policies[0];
				if (first) pick(first);
			}
		} catch (e) {
			error = (e as Error).message;
		}
	}
	onMount(() => {
		void load();
		void listAgents().then((a) => {
			agents = a;
			if (!tryAgent && a.length) tryAgent = a[0].id;
		}).catch(() => {});
	});

	const current = $derived(policies?.find((p) => p.id === selected) ?? null);
	const canWrite = $derived(fleet.isOwner);
	const appliesLabel = (p: Pick<Policy, 'scope' | 'applies_to'>) =>
		p.scope === 'org' ? 'All agents' : p.scope === 'team' ? p.applies_to || 'No team chosen' : (agents.find((a) => a.id === p.applies_to)?.name ?? 'One agent');

	async function toggle(p: Policy, on: boolean) {
		try {
			await updatePolicy(p.id, { active: on });
			toast.success(`${p.name} is ${on ? 'on' : 'off'}. Recorded in the audit log.`);
			await load();
			if (selected === p.id) draft.active = on;
		} catch (e) {
			toast.error('Could not change the policy', { description: writeError(e) });
		}
	}

	async function save(e: SubmitEvent) {
		e.preventDefault();
		const tools = draft.tools_text.split('\n').map((t) => t.trim()).filter(Boolean);
		if (!tools.length) {
			toast.error('Add at least one tool or action', { description: 'One per line, such as shell.exec: psql or git.push: main.' });
			return;
		}
		const body: PolicyInput = {
			name: draft.name.trim(),
			description: draft.description.trim(),
			scope: draft.scope,
			applies_to: draft.scope === 'org' ? '' : draft.applies_to.trim(),
			tools,
			category: draft.category.trim(),
			decision: draft.decision,
			priority: Math.max(1, Math.round(Number(draft.priority) || 1)),
			active: draft.active
		};
		saving = true;
		try {
			const p = selected ? await updatePolicy(selected, body) : await createPolicy(body);
			toast.success(`Saved ${p.name}. Agents pick it up on their next tool call.`);
			selected = p.id;
			await load();
			pick(p);
		} catch (err) {
			toast.error('Could not save the policy', { description: writeError(err) });
		}
		saving = false;
	}

	async function remove() {
		if (!current || !confirm(`Delete ${current.name}? Agents fall back to the next matching policy.`)) return;
		try {
			await deletePolicy(current.id);
			toast.success(`Deleted ${current.name}. Recorded in the audit log.`);
			selected = null;
			await load();
		} catch (e) {
			toast.error('Could not delete the policy', { description: writeError(e) });
		}
	}

	// ---- try an action ----
	let tryAgent = $state('');
	let tryTool = $state('shell.exec');
	let tryAction = $state('psql "$PROD_DB" -c "SELECT count(*) FROM invoices"');
	let result = $state<Evaluation | null>(null);
	let trying = $state(false);
	async function tryIt(e: SubmitEvent) {
		e.preventDefault();
		trying = true;
		try {
			result = await evaluate({ agent_id: tryAgent || undefined, tool: tryTool.trim(), action: tryAction });
		} catch (err) {
			toast.error('Could not evaluate', { description: (err as Error).message });
		}
		trying = false;
	}

	const DECISIONS: { id: Decision; label: string; on: string }[] = [
		{ id: 'allow', label: 'Allow', on: 'bg-ok-soft text-ok-ink font-semibold' },
		{ id: 'require_approval', label: 'Require approval', on: 'bg-queue-soft text-queue-ink font-semibold' },
		{ id: 'deny', label: 'Deny', on: 'bg-err-soft text-err-ink font-semibold' }
	];
	const teams = $derived([...new Set(agents.map((a) => a.team).filter(Boolean))].sort());
	const cols = 'grid grid-cols-[minmax(170px,2fr)_minmax(96px,.9fr)_minmax(128px,1fr)_48px_44px] items-center gap-3 min-w-[560px]';
</script>

<svelte:head><title>Policies · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-4.5 px-4 pt-8 pb-[calc(3rem+env(safe-area-inset-bottom))] md:px-8">
	<PageHead title="Policies" sub="Checked at the tool-call boundary before anything runs. Lowest priority number wins. If evaluation is unavailable, protected actions are denied.">
		{#snippet actions()}<Button class="h-8 text-[13px]" onclick={startNew} disabled={!canWrite} title={canWrite ? undefined : 'Only org owners can change policies'}><Plus class="size-3.5" />New policy</Button>{/snippet}
	</PageHead>

	{#if error && !policies}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">Could not load policies: {error}</p>
	{:else if !policies}
		<p class="text-[13px] text-muted-foreground">Loading…</p>
	{:else}
		<div class="flex flex-wrap items-start gap-3">
			<div class="min-w-0 flex-[3_1_600px] overflow-x-auto rounded-xl border bg-card">
				<div class="{cols} px-4 py-2.5 text-[11.5px] font-medium text-muted-foreground"><span>Policy</span><span>Scope</span><span>Decision</span><span>Priority</span><span>Active</span></div>
				{#each policies as p (p.id)}
					{@const d = DECISION[p.decision]}
					<div class="{cols} border-t px-4 py-2.5 text-[13px] {p.id === selected ? 'bg-accent' : ''}">
						<div class="min-w-0"><button type="button" class="text-left font-medium hover:underline" onclick={() => pick(p)}>{p.name}</button><div class="line-clamp-2 text-[12px] text-muted-foreground">{p.description}</div></div>
						<div class="min-w-0"><div>{SCOPE[p.scope]}</div><div class="truncate text-[12px] text-muted-foreground">{appliesLabel(p)}</div></div>
						<span><Chip tone={p.active ? d.tone : 'off'}>{d.label}</Chip></span>
						<span class="tnum font-mono">{p.priority}</span>
						<Switch checked={p.active} onchange={(v) => toggle(p, v)} aria-label="{p.active ? 'Turn off' : 'Turn on'} {p.name}" />
					</div>
				{/each}
				{#if !policies.length}<div class="border-t px-5 py-5 text-[13px] text-muted-foreground">No policies. Everything is denied.</div>{/if}
			</div>

			{#if selected !== null}
				<form onsubmit={save} class="flex min-w-0 flex-[1_1_320px] flex-col gap-3.5 rounded-xl border bg-card px-5 py-4.5">
					<div class="flex items-center justify-between gap-2">
						<h2 class="text-[14px] font-semibold">{selected ? 'Edit policy' : 'New policy'}</h2>
						<Chip tone={draft.active ? 'ok' : 'off'} dot={false}>{draft.active ? 'Active' : 'Inactive'}</Chip>
					</div>
					<fieldset disabled={!canWrite} class="m-0 flex flex-col gap-3.5 border-0 p-0">
						<label class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Name<Input bind:value={draft.name} required maxlength={120} class="h-8.5 text-[13px]" /></label>
						<label class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Description<Textarea bind:value={draft.description} rows={2} maxlength={500} class="text-[13px]" /></label>
						<div class="grid grid-cols-2 gap-2.5">
							<div class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Scope
								<Select.Root type="single" value={draft.scope} onValueChange={(v) => { draft.scope = v as PolicyScope; draft.applies_to = ''; }}>
									<Select.Trigger class="h-8.5 w-full text-[13px] text-foreground" aria-label="Scope">{SCOPE[draft.scope]}</Select.Trigger>
									<Select.Content>{#each Object.entries(SCOPE) as [id, label] (id)}<Select.Item value={id} {label}>{label}</Select.Item>{/each}</Select.Content>
								</Select.Root>
							</div>
							<label class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Priority<Input type="number" min={1} bind:value={draft.priority} class="tnum h-8.5 text-[13px]" /></label>
						</div>
						<div class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Applies to
							{#if draft.scope === 'org'}
								<div class="text-[13px] text-foreground">All agents</div>
							{:else if draft.scope === 'team'}
								<Input bind:value={draft.applies_to} list="policy-teams" placeholder="Team name, as set on its agents" class="h-8.5 text-[13px]" />
								<datalist id="policy-teams">{#each teams as t (t)}<option value={t}></option>{/each}</datalist>
							{:else}
								<Select.Root type="single" bind:value={draft.applies_to}>
									<Select.Trigger class="h-8.5 w-full text-[13px] text-foreground" aria-label="Agent">{agents.find((a) => a.id === draft.applies_to)?.name ?? 'Choose an agent'}</Select.Trigger>
									<Select.Content>{#each agents as a (a.id)}<Select.Item value={a.id} label={a.name}>{a.name}</Select.Item>{/each}</Select.Content>
								</Select.Root>
							{/if}
						</div>
						<label class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Tools and actions, one per line
							<Textarea bind:value={draft.tools_text} rows={3} class="font-mono text-[12px]" placeholder={'shell.exec: psql, mysql\ngit.push: main, release/*'} />
						</label>
						<label class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Action category<Input bind:value={draft.category} maxlength={120} class="h-8.5 text-[13px]" placeholder="Access production database" /></label>
						<fieldset class="m-0 flex flex-col gap-1 border-0 p-0">
							<legend class="mb-1 p-0 text-[11.5px] text-muted-foreground">Decision</legend>
							<div class="flex">
								{#each DECISIONS as d, i (d.id)}
									<button
										type="button"
										aria-pressed={draft.decision === d.id}
										class="h-8.5 flex-1 border border-input px-1 text-[12px] whitespace-nowrap {i === 0 ? 'rounded-l-lg' : ''} {i === DECISIONS.length - 1 ? 'rounded-r-lg' : ''} {i ? 'border-l-0' : ''} {draft.decision === d.id ? d.on : 'bg-card text-muted-foreground'}"
										onclick={() => (draft.decision = d.id)}
									>{d.label}</button>
								{/each}
							</div>
						</fieldset>
						<label class="flex items-center gap-2 text-[13px]"><Switch checked={draft.active} onchange={(v) => (draft.active = v)} aria-label="Active" />Active</label>
					</fieldset>
					<div class="flex flex-wrap items-center justify-between gap-2.5 border-t pt-3">
						<span class="flex-[1_1_180px] text-[12px] text-muted-foreground">
							{#if canWrite}Saving writes a <span class="font-mono">{selected ? 'policy.updated' : 'policy.created'}</span> record to the audit log.{:else}Only org owners can change policies.{/if}
						</span>
						<div class="flex gap-2">
							{#if current && !current.builtin}<Button type="button" variant="outline" size="sm" class="h-8 text-err-ink" disabled={!canWrite} onclick={remove} aria-label="Delete policy"><Trash2 class="size-3.5" /></Button>{/if}
							<Button type="submit" size="sm" class="h-8" disabled={!canWrite || saving || !draft.name.trim()}>{saving ? 'Saving…' : selected ? 'Save changes' : 'Create policy'}</Button>
						</div>
					</div>
				</form>
			{/if}
		</div>

		<form onsubmit={tryIt} class="flex flex-col gap-3 rounded-xl border bg-card px-5 py-4.5">
			<div class="flex flex-wrap items-center justify-between gap-2">
				<h2 class="text-[14px] font-semibold">Try an action against these policies</h2>
				<span class="text-[12px] text-muted-foreground">Same check agents get from <span class="font-mono">POST /v1/policy/evaluate</span></span>
			</div>
			<div class="grid grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-2.5">
				<div class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Agent
					<Select.Root type="single" bind:value={tryAgent}>
						<Select.Trigger class="h-8.5 w-full font-mono text-[12px] text-foreground" aria-label="Agent">{agents.find((a) => a.id === tryAgent)?.name ?? 'Any agent (organisation policies)'}</Select.Trigger>
						<Select.Content>
							<Select.Item value="" label="Any agent">Any agent (organisation policies)</Select.Item>
							{#each agents as a (a.id)}<Select.Item value={a.id} label={a.name}>{a.name}</Select.Item>{/each}
						</Select.Content>
					</Select.Root>
				</div>
				<label class="flex flex-col gap-1 text-[11.5px] text-muted-foreground">Tool<Input bind:value={tryTool} class="h-8.5 font-mono text-[12px]" placeholder="shell.exec" /></label>
				<label class="flex flex-col gap-1 text-[11.5px] text-muted-foreground sm:col-span-2">Action<Input bind:value={tryAction} class="h-8.5 font-mono text-[12px]" placeholder="git push origin main" /></label>
			</div>
			<div class="flex justify-end"><Button type="submit" variant="outline" size="sm" class="h-8" disabled={trying || !tryTool.trim()}>{trying ? 'Checking…' : 'Check'}</Button></div>
			{#if result}
				{@const d = DECISION[result.decision]}
				<div class="flex flex-wrap items-center gap-3 rounded-[10px] bg-muted px-3.5 py-3">
					<Chip tone={d.tone}>{d.label}</Chip>
					<div class="flex-[1_1_300px] text-[13px]">
						<div>Matched <strong>{result.policy ? `${result.policy.name} (priority ${result.policy.priority})` : 'Default deny (nothing else matched)'}</strong></div>
						<div class="text-[12px] text-muted-foreground">
							{#if result.overridden.length}Also matched {result.overridden.map((o) => `${o.name} (priority ${o.priority}, ${DECISION[o.decision].label.toLowerCase()})`).join('; ')}. The lower number wins.{:else}{result.reason || 'No other policy matched.'}{/if}
						</div>
						{#if result.actions.length > 1}<div class="mt-1 font-mono text-[11.5px] text-muted-foreground">Checked as {result.actions.map((a) => a.tool).join(', ')}</div>{/if}
					</div>
					<span class="font-mono text-[12px] text-muted-foreground">{result.elapsed_ms} ms</span>
				</div>
			{/if}
		</form>
	{/if}
</section>
