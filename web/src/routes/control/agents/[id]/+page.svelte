<script lang="ts">
	// One agent: who owns it, what it may do, what it did, and its suspensions.
	import { page } from '$app/state';
	import { untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { Textarea } from '#lib/components/ui/textarea';
	import * as Select from '#lib/components/ui/select';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import Ban from '@lucide/svelte/icons/ban';
	import Power from '@lucide/svelte/icons/power';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import UserRoundCheck from '@lucide/svelte/icons/user-round-check';
	import KeyRound from '@lucide/svelte/icons/key-round';
	import Pencil from '@lucide/svelte/icons/pencil';
	import { fleet } from '#lib/sf/state.svelte';
	import { rel } from '#lib/sf/format';
	import {
		claimAgent,
		clock,
		getAgent,
		listActivity,
		listAudit,
		newAgentKey,
		reactivateAgent,
		suspendAgent,
		updateAgent,
		kindName,
		shortHash,
		writeError,
		ACTOR_KIND,
		DECISION,
		RISK,
		STATUS,
		type ActivityEvent,
		type AgentDetail,
		type AuditRecord,
		type Risk
	} from '#lib/sf/control';
	import { control } from '#lib/sf/control.svelte';
	import Chip from '#lib/sf/ui/control/Chip.svelte';
	import Tabs from '#lib/sf/ui/control/Tabs.svelte';
	import ActivityRows from '#lib/sf/ui/control/ActivityRows.svelte';
	import CopyButton from '#lib/sf/ui/CopyButton.svelte';

	const id = $derived(page.params.id ?? '');
	let data = $state<AgentDetail | null>(null);
	let missing = $state('');
	let events = $state<ActivityEvent[]>([]);
	let audit = $state<AuditRecord[]>([]);
	let tab = $state('overview');
	/** What the last suspension could not stop, said plainly until the page is left. */
	let notStopped = $state('');

	async function load(aid = id) {
		try {
			const [d, act, au] = await Promise.all([getAgent(aid), listActivity({ agent: aid, limit: 50 }), listAudit({ agent: aid, limit: 100 })]);
			if (aid !== id) return;
			data = d;
			events = act.events;
			audit = au.records;
			missing = '';
		} catch (e) {
			if (aid === id) missing = (e as { status?: number }).status === 404 ? 'There is no agent with that id in this org.' : (e as Error).message;
		}
	}
	$effect(() => {
		const aid = id;
		untrack(() => {
			data = null;
			missing = '';
			notStopped = '';
			tab = 'overview';
			void load(aid);
		});
		const t = setInterval(() => void load(aid), 15_000);
		return () => clearInterval(t);
	});

	const a = $derived(data?.agent ?? null);
	const meId = $derived(fleet.me?.user?.id ?? null);
	const canEdit = $derived(!!a && (fleet.isOwner || (!!meId && a.owner?.id === meId)));
	const tabs = [
		{ id: 'overview', label: 'Overview' },
		{ id: 'perms', label: 'Permissions & policies' },
		{ id: 'activity', label: 'Activity' },
		{ id: 'audit', label: 'Audit history' }
	];

	async function claim() {
		if (!a) return;
		try {
			await claimAgent(a.id);
			toast.success(`You now own ${a.name}. Its actions count as attributed from here on.`);
			await load();
		} catch (e) {
			toast.error('Could not assign the agent', { description: writeError(e) });
		}
	}

	async function reactivate() {
		if (!a) return;
		try {
			await reactivateAgent(a.id);
			toast.success(`${a.name} is active again.`, { description: a.source === 'registered' ? 'Revoked credentials stay revoked until reissued.' : undefined });
			notStopped = '';
			await load();
			void control.refresh();
		} catch (e) {
			toast.error('Could not reactivate', { description: writeError(e) });
		}
	}

	// ---- editing ----
	let editing = $state(false);
	let edit = $state({ description: '', team: '', environment: '', risk: 'unknown' as Risk });
	function startEdit() {
		if (!a) return;
		edit = { description: a.description, team: a.team, environment: a.environment, risk: a.risk };
		editing = true;
	}
	async function saveEdit(e: SubmitEvent) {
		e.preventDefault();
		if (!a) return;
		try {
			await updateAgent(a.id, { description: edit.description.trim(), team: edit.team.trim(), environment: edit.environment.trim(), risk: edit.risk });
			editing = false;
			toast.success('Saved', { description: edit.team.trim() !== a.team ? 'A new team changes which team policies apply. Recorded in the audit log.' : undefined });
			await load();
		} catch (err) {
			toast.error('Could not save', { description: writeError(err, 'an org owner or the agent’s owner') });
		}
	}

	// ---- agent key ----
	let freshKey = $state('');
	async function reissueKey() {
		if (!a || !confirm(`Issue a new key for ${a.name}? The current one stops working.`)) return;
		try {
			freshKey = (await newAgentKey(a.id)).key;
			await load();
		} catch (e) {
			toast.error('Could not issue a key', { description: writeError(e) });
		}
	}

	// ---- suspension ----
	let suspendOpen = $state(false);
	let reason = $state('');
	let revoke = $state(true);
	let cancelRunning = $state(true);
	let suspending = $state(false);
	function openSuspend() {
		reason = '';
		revoke = true;
		cancelRunning = !!a?.can_cancel;
		suspendOpen = true;
	}
	const cancelNote = $derived(
		!a ? ''
		: a.can_cancel ? `Supported here. The Sidefleet worker on ${a.host || 'its machine'} stops the task, as the Stop button does.`
		: a.source === 'registered' ? `Not possible: this agent runs outside a Sidefleet worker. Its next call through the gateway is denied, but the process keeps running until someone stops it${a.host ? ` on ${a.host}` : ''}.`
		: `Not supported on this runtime (${a.runtime}). The current step finishes; its next tool call is denied.`
	);
	async function suspend() {
		if (!a) return;
		suspending = true;
		try {
			const r = await suspendAgent(a.id, { reason: reason.trim() || 'No reason given.', revoke_credentials: revoke, cancel_running: cancelRunning && a.can_cancel });
			suspendOpen = false;
			notStopped = r.incident.not_stopped ?? '';
			const bits = [
				r.incident.revoked.length ? `Revoked ${r.incident.revoked.join(', ')}.` : '',
				r.incident.cancelled_approvals ? `Cancelled ${r.incident.cancelled_approvals} pending approval${r.incident.cancelled_approvals === 1 ? '' : 's'}.` : '',
				r.incident.cancelled_runs ? `Stopping ${r.incident.cancelled_runs} running task${r.incident.cancelled_runs === 1 ? '' : 's'}.` : ''
			].filter(Boolean).join(' ');
			toast.success(`Suspended ${a.name}. New tasks and tool calls are now refused.`, { description: bits || undefined });
			if (notStopped) toast.warning('A running process was not stopped', { description: notStopped, duration: 12_000 });
			await load();
			void control.refresh();
		} catch (e) {
			toast.error('Could not suspend', { description: writeError(e) });
		}
		suspending = false;
	}

	const fmtDate = (iso: string) => new Date(iso).toLocaleDateString([], { day: 'numeric', month: 'short', year: 'numeric' });
	const fmtWhen = (iso: string) => new Date(iso).toLocaleString([], { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hour12: false });
	const permCols = 'grid grid-cols-[minmax(180px,1.4fr)_minmax(140px,1fr)_minmax(140px,1fr)_minmax(160px,1.2fr)] items-center gap-3.5 min-w-[680px]';
	const auditCols = 'grid grid-cols-[96px_minmax(120px,1fr)_minmax(150px,1fr)_minmax(220px,2fr)_64px] items-center gap-3.5 min-w-[720px]';
</script>

<svelte:head><title>{a?.name ?? 'Agent'} · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-4.5 px-4 pt-8 pb-[calc(3rem+env(safe-area-inset-bottom))] md:px-8">
	<a href="#/control/agents" class="inline-flex items-center gap-1 self-start text-[13px] text-muted-foreground hover:text-foreground"><ArrowLeft class="size-3.5" />All agents</a>

	{#if missing}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">{missing}</p>
	{:else if !a || !data}
		<p class="text-[13px] text-muted-foreground">Loading…</p>
	{:else}
		<div class="flex flex-wrap items-start justify-between gap-4">
			<div class="flex min-w-0 flex-col gap-1.5">
				<div class="flex flex-wrap items-center gap-2.5">
					<h1 class="text-[22px] font-semibold tracking-tight break-all">{a.name}</h1>
					<Chip tone={STATUS[a.status].tone}>{STATUS[a.status].label}</Chip>
					<Chip tone={RISK[a.risk].tone} dot={false}>{RISK[a.risk].label} risk</Chip>
				</div>
				{#if a.description}<p class="text-[13px] text-muted-foreground">{a.description}</p>{/if}
				<div class="font-mono text-[12px] text-muted-foreground">{a.id} · {a.source === 'discovered' ? 'discovered' : 'registered'} {fmtDate(a.created_at)}</div>
			</div>
			<div class="flex flex-wrap gap-2">
				<Button variant="outline" size="sm" class="h-8 text-[13px]" onclick={() => (tab = 'perms')}>Edit permissions</Button>
				{#if a.status === 'suspended'}
					<Button size="sm" class="h-8 text-[13px]" onclick={reactivate} disabled={!fleet.isOwner} title={fleet.isOwner ? undefined : 'Only org owners can reactivate'}><Power class="size-3.5" />Reactivate</Button>
				{:else}
					<Button variant="outline" size="sm" class="h-8 text-[13px] text-err-ink" onclick={openSuspend} disabled={!fleet.isOwner} title={fleet.isOwner ? undefined : 'Only org owners can suspend'}><Ban class="size-3.5" />Suspend agent</Button>
				{/if}
			</div>
		</div>

		{#if a.suspension}
			<div class="flex items-start gap-3 rounded-[10px] bg-err-soft px-3.5 py-3 text-err-ink" role="status">
				<CircleAlert class="mt-0.5 size-4 shrink-0" />
				<div>
					<div class="font-semibold">Suspended {rel(a.suspension.at, fleet.now)} by {a.suspension.by}. {a.suspension.reason}</div>
					<div class="mt-0.5 text-[12px]">New tasks and tool calls are refused at the execution gateway and Sidefleet-managed credentials are revoked. Processes started outside Sidefleet are not stopped by a suspension.</div>
				</div>
			</div>
		{/if}
		{#if notStopped}
			<div class="flex items-start gap-3 rounded-[10px] bg-queue-soft px-3.5 py-3 text-queue-ink" role="alert">
				<CircleAlert class="mt-0.5 size-4 shrink-0" />
				<div><div class="font-semibold">A running process was not stopped.</div><div class="mt-0.5 text-[12px]">{notStopped}</div></div>
			</div>
		{/if}
		{#if !a.owner}
			<div class="flex flex-wrap items-center gap-3 rounded-[10px] bg-queue-soft px-3.5 py-3 text-queue-ink">
				<UserRoundCheck class="size-4 shrink-0" />
				<div class="flex-[1_1_280px]">
					<div class="font-semibold">No one owns this agent.</div>
					<div class="text-[12px]">{a.source === 'discovered' ? 'It was discovered from a machine of this org.' : 'It was registered without an owner.'} Its actions count as unattributed until someone takes responsibility for it.</div>
				</div>
				{#if fleet.me?.user}<Button variant="outline" size="sm" class="h-7 bg-card text-[12px] text-foreground" onclick={claim}>Assign to me</Button>{/if}
			</div>
		{/if}

		<Tabs {tabs} bind:value={tab} label="Agent" />

		{#if tab === 'overview'}
			<div class="grid grid-cols-[repeat(auto-fit,minmax(min(340px,100%),1fr))] gap-3">
				<div class="rounded-xl border bg-card px-5 pt-4 pb-2">
					<div class="mb-2 flex items-center justify-between">
						<h2 class="text-[14px] font-semibold">Identity &amp; configuration</h2>
						{#if canEdit && !editing}<button type="button" class="inline-flex items-center gap-1 text-[12px] text-muted-foreground hover:text-foreground" onclick={startEdit}><Pencil class="size-3" />Edit</button>{/if}
					</div>
					{#if editing}
						<form onsubmit={saveEdit} class="flex flex-col gap-2.5 pb-3">
							<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Description<Textarea bind:value={edit.description} rows={2} maxlength={500} /></label>
							<div class="grid grid-cols-2 gap-2.5">
								<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Team<Input bind:value={edit.team} class="h-9" placeholder="No team" /></label>
								<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Environment<Input bind:value={edit.environment} class="h-9" /></label>
							</div>
							<div class="flex flex-col gap-1 text-[12px] text-muted-foreground">Risk
								<Select.Root type="single" bind:value={edit.risk}>
									<Select.Trigger class="h-9 w-full" aria-label="Risk">{RISK[edit.risk].label}</Select.Trigger>
									<Select.Content>{#each Object.entries(RISK) as [rid, r] (rid)}<Select.Item value={rid} label={r.label}>{r.label}</Select.Item>{/each}</Select.Content>
								</Select.Root>
							</div>
							<div class="flex justify-end gap-2"><Button type="button" variant="outline" size="sm" onclick={() => (editing = false)}>Cancel</Button><Button type="submit" size="sm">Save</Button></div>
						</form>
					{:else}
						<dl class="m-0 text-[13px]">
							{#each [
								['Owner', a.owner?.login ?? 'Unassigned'],
								['Team', a.team || 'No team'],
								['Agent type', kindName(a.kind)],
								['Model', `${a.model || 'default'}${a.provider ? ` · ${a.provider}` : ''}`],
								['Runtime', a.runtime],
								['Host machine', a.host || '–'],
								['Environment', a.environment || 'Unknown'],
								['Last active', a.last_active_at ? rel(a.last_active_at, fleet.now) : 'Never'],
								['Credentials', a.credentials.length ? a.credentials.join(', ') : 'None held for this agent']
							] as [k, v] (k)}
								<div class="grid grid-cols-[130px_minmax(0,1fr)] gap-2.5 border-t py-2">
									<dt class="text-muted-foreground">{k}</dt>
									<dd class="m-0 min-w-0 [overflow-wrap:anywhere] {k === 'Owner' && !a.owner ? 'text-queue-ink' : ''} {k === 'Host machine' || k === 'Model' ? 'font-mono text-[12px]' : ''}">{v}</dd>
								</div>
							{/each}
						</dl>
						{#if a.source === 'registered' && canEdit}
							<div class="flex items-center justify-between gap-2 border-t py-2 text-[12px]">
								<span class="text-muted-foreground">Agent key for the gateway</span>
								<button type="button" class="inline-flex items-center gap-1 text-muted-foreground hover:text-foreground disabled:opacity-50" onclick={reissueKey} disabled={a.status === 'suspended'}><KeyRound class="size-3" />Issue a new key</button>
							</div>
							{#if freshKey}
								<div class="mb-2 flex items-center gap-2 rounded-md border border-ok/40 bg-ok-soft/40 px-2.5 py-1.5">
									<code class="min-w-0 flex-1 truncate font-mono text-[12px]">{freshKey}</code>
									<CopyButton text={() => freshKey} class="bg-primary text-primary-foreground hover:bg-primary/90" />
								</div>
							{/if}
						{/if}
					{/if}
				</div>
				<div class="rounded-xl border bg-card px-5 pt-4 pb-2">
					<div class="mb-2 flex items-center justify-between"><h2 class="text-[14px] font-semibold">What it can do</h2><button type="button" class="text-[12px] text-muted-foreground hover:text-foreground" onclick={() => (tab = 'perms')}>Details</button></div>
					{#each data.permissions as p (p.action)}
						<div class="flex items-center justify-between gap-2.5 border-t py-1.75 text-[13px]">
							<span>{p.action}</span>
							<Chip tone={DECISION[p.decision].tone}>{DECISION[p.decision].label}</Chip>
						</div>
					{/each}
				</div>
			</div>

			<div class="rounded-xl border bg-card">
				<div class="px-5 pt-4 pb-2.5"><h2 class="text-[14px] font-semibold">Recent activity</h2></div>
				<ActivityRows events={events.slice(0, 8)} head={false} owners={false} empty="No instrumented activity yet." />
			</div>

			{#if data.incidents.length}
				<div class="rounded-xl border bg-card">
					<div class="px-5 pt-4 pb-2.5"><h2 class="text-[14px] font-semibold">Incident history</h2></div>
					{#each data.incidents as inc (inc.id)}
						<div class="flex flex-col gap-1 border-t px-5 py-3 text-[13px]">
							<div class="flex flex-wrap items-center gap-2">
								<Chip tone={inc.reactivated_at ? 'off' : 'err'}>{inc.reactivated_at ? 'Resolved' : 'Open'}</Chip>
								<span class="font-medium">Suspended {fmtWhen(inc.suspended_at)} by {inc.suspended_by}</span>
								{#if inc.reactivated_at}<span class="text-muted-foreground">· reactivated {fmtWhen(inc.reactivated_at)}{inc.reactivated_by ? ` by ${inc.reactivated_by}` : ''}</span>{/if}
							</div>
							<div>{inc.reason}</div>
							<div class="text-[12px] text-muted-foreground">
								{inc.revoked.length ? `Revoked ${inc.revoked.join(', ')}` : 'No credentials revoked'} · {inc.cancelled_approvals} approval{inc.cancelled_approvals === 1 ? '' : 's'} cancelled · {inc.cancelled_runs} task{inc.cancelled_runs === 1 ? '' : 's'} stopped
							</div>
							{#if inc.not_stopped}<div class="text-[12px] text-queue-ink">{inc.not_stopped}</div>{/if}
						</div>
					{/each}
				</div>
			{/if}
		{:else if tab === 'perms'}
			<div class="overflow-x-auto rounded-xl border bg-card">
				<div class="px-4 pt-4 pb-1.5">
					<h2 class="text-[14px] font-semibold">Effective permissions</h2>
					<p class="mt-1 text-[12px] text-muted-foreground">Resolved from the policies below at every tool call. The lowest priority number that matches wins; nothing matched means denied.</p>
				</div>
				<div class="{permCols} px-4 py-2.5 text-[11.5px] font-medium text-muted-foreground"><span>Action</span><span>Tool</span><span>Decision</span><span>Decided by</span></div>
				{#each data.permissions as p (p.action)}
					<div class="{permCols} border-t px-4 py-2.5 text-[13px]">
						<span class="font-medium">{p.action}</span>
						<span class="font-mono text-[12px] text-muted-foreground">{p.tool}</span>
						<span><Chip tone={DECISION[p.decision].tone}>{DECISION[p.decision].label}</Chip></span>
						<span class="text-muted-foreground">{a.status === 'suspended' ? 'Agent suspended' : (p.policy?.name ?? 'Default deny')}</span>
					</div>
				{/each}
			</div>
			<div class="flex flex-col gap-2.5 rounded-xl border bg-card px-5 py-4">
				<div class="flex items-center justify-between gap-2"><h2 class="text-[14px] font-semibold">Assigned policies</h2><Button href="#/control/policies" variant="outline" size="sm" class="h-7 text-[12px]">Manage policies</Button></div>
				<div class="flex flex-wrap gap-1.5">
					{#each data.policies as p (p.id)}
						<Chip tone="off" dot={false}><span class="font-mono">P{p.priority}</span> {p.name} <span class="opacity-75">· {p.scope}</span></Chip>
					{/each}
					{#if !data.policies.length}<span class="text-[13px] text-muted-foreground">No active policy applies, so everything is denied.</span>{/if}
				</div>
				{#if !a.team}<p class="text-[12px] text-muted-foreground">Give it a team to apply that team's policies too.</p>{/if}
			</div>
		{:else if tab === 'activity'}
			<div class="rounded-xl border bg-card">
				<ActivityRows {events} owners={false} empty="No instrumented activity yet." />
			</div>
		{:else}
			<div class="overflow-x-auto rounded-xl border bg-card">
				<div class="{auditCols} px-4 py-2.5 text-[11.5px] font-medium text-muted-foreground"><span>Time</span><span>Actor</span><span>Event</span><span>Detail</span><span>Hash</span></div>
				{#each audit as r (r.id)}
					<div class="{auditCols} border-t px-4 py-2.5 text-[13px]">
						<span class="tnum font-mono text-[12px] text-muted-foreground">{clock(r.at, fleet.now)}</span>
						<div class="min-w-0"><div class="truncate">{r.actor}</div><div class="text-[12px] text-muted-foreground">{ACTOR_KIND[r.actor_kind] ?? r.actor_kind}</div></div>
						<a class="truncate font-mono text-[12px] hover:underline" href="#/control/audit?record={r.id}">{r.event}</a>
						<span class="min-w-0 truncate text-muted-foreground" title={r.detail}>{r.detail}</span>
						<span class="font-mono text-[12px] text-muted-foreground">{shortHash(r.hash)}</span>
					</div>
				{/each}
				{#if !audit.length}<div class="border-t px-5 py-5 text-[13px] text-muted-foreground">No governance records for this agent yet.</div>{/if}
			</div>
		{/if}
	{/if}
</section>

{#if a}
	<Dialog.Root bind:open={suspendOpen}>
		<Dialog.Content class="sm:max-w-[34rem]">
			<Dialog.Header>
				<Dialog.Title>Suspend {a.name}?</Dialog.Title>
				<Dialog.Description>Takes effect at the execution gateway straight away. You can reactivate it later; the suspension is kept in its incident history.</Dialog.Description>
			</Dialog.Header>
			<label class="flex flex-col gap-1 text-[12px] text-muted-foreground">Reason (goes in the audit log)<Textarea bind:value={reason} rows={3} maxlength={1000} placeholder="What did you see?" /></label>
			<div class="flex flex-col gap-3 text-[13px]">
				<div class="text-[12.5px] font-semibold">Stops new work</div>
				<label class="flex items-start gap-2.5"><input type="checkbox" checked disabled class="mt-0.75" /><span><span class="font-medium">Block new tasks and tool calls</span><span class="block text-[12px] text-muted-foreground">Always applies. Every call from this agent is denied with reason "agent suspended".</span></span></label>
				<label class="flex items-start gap-2.5"><input type="checkbox" bind:checked={revoke} disabled={!a.credentials.length} class="mt-0.75" /><span><span class="font-medium">Revoke Sidefleet-managed credentials</span><span class="block text-[12px] text-muted-foreground"><span class="font-mono">{a.credentials.length ? a.credentials.join(', ') : 'None held for this agent'}</span>. Credentials Sidefleet doesn't manage need rotating where they live.</span></span></label>
				<label class="flex items-start gap-2.5"><input type="checkbox" checked disabled class="mt-0.75" /><span><span class="font-medium">Cancel pending approvals ({a.pending_approvals})</span><span class="block text-[12px] text-muted-foreground">Requests waiting for a decision are marked cancelled.</span></span></label>
				<div class="pt-1 text-[12.5px] font-semibold">Stops work already running</div>
				<label class="flex items-start gap-2.5"><input type="checkbox" bind:checked={cancelRunning} disabled={!a.can_cancel} class="mt-0.75" /><span><span class="font-medium">Cancel the running task</span><span class="block text-[12px] {a.can_cancel ? 'text-muted-foreground' : 'text-queue-ink'}">{cancelNote}</span></span></label>
			</div>
			<div class="flex justify-end gap-2">
				<Button variant="outline" onclick={() => (suspendOpen = false)}>Cancel</Button>
				<Button class="bg-destructive text-white hover:bg-destructive/90" onclick={suspend} disabled={suspending}>{suspending ? 'Suspending…' : 'Suspend agent'}</Button>
			</div>
		</Dialog.Content>
	</Dialog.Root>
{/if}
