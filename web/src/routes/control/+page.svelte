<script lang="ts">
	// The control plane at a glance: how many agents, what they're doing, what waits on a person.
	import { onMount } from 'svelte';
	import { Button } from '#lib/components/ui/button';
	import { fleet } from '#lib/sf/state.svelte';
	import { rel } from '#lib/sf/format';
	import {
		listActivity,
		listAgents,
		listApprovals,
		remaining,
		summary,
		kindName,
		type ActivityEvent,
		type Agent,
		type ControlApproval,
		type Summary,
		type Tone
	} from '#lib/sf/control';
	import Chip from '#lib/sf/ui/control/Chip.svelte';
	import PageHead from '#lib/sf/ui/control/PageHead.svelte';
	import ActivityRows from '#lib/sf/ui/control/ActivityRows.svelte';

	let sum = $state<Summary | null>(null);
	let agents = $state<Agent[]>([]);
	let pending = $state<ControlApproval[]>([]);
	let recent = $state<ActivityEvent[]>([]);
	let error = $state('');

	const NOTABLE = new Set(['denied', 'approval_required', 'failed', 'suspended', 'approved', 'rejected']);

	async function load() {
		try {
			const [s, a, p, act] = await Promise.all([summary(), listAgents(), listApprovals('pending'), listActivity({ limit: 100 })]);
			sum = s;
			agents = a;
			pending = p.approvals;
			recent = act.events.filter((e) => NOTABLE.has(e.outcome)).slice(0, 5);
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

	const orgName = $derived(fleet.org?.name ?? 'this org');
	const soonest = $derived(sum?.approvals.soonest_expiry ? `Soonest expires in ${remaining(sum.approvals.soonest_expiry, fleet.now)}` : 'Nothing waiting');

	type Item = { tone: Tone; kind: string; title: string; meta: string; btn: string; href: string };
	const attention = $derived.by(() => {
		const out: Item[] = [];
		for (const r of pending) {
			// "Run psql…" reads as "waiting: run psql…"; a tool name like WebFetch keeps its case.
			const t = !r.title ? r.tool : /^[A-Z][a-z]*(\s|$)/.test(r.title) ? r.title.charAt(0).toLowerCase() + r.title.slice(1) : r.title;
			out.push({ tone: 'warn', kind: 'Approval', title: `${r.agent?.name ?? 'An agent'} is waiting: ${t}`, meta: `Owner ${r.owner?.login ?? 'unassigned'} · expires in ${remaining(r.expires_at, fleet.now)}`, btn: 'Review', href: '#/control/approvals' });
		}
		for (const a of agents.filter((a) => !a.owner)) {
			out.push({ tone: 'run', kind: 'No owner', title: `An agent on ${a.host || 'an unknown host'} has no owner`, meta: `${kindName(a.kind)} · discovered ${rel(a.created_at, fleet.now)} · its actions are unattributed`, btn: 'Assign owner', href: `#/control/agents/${a.id}` });
		}
		for (const a of agents.filter((a) => a.status === 'suspended')) {
			out.push({ tone: 'err', kind: 'Suspended', title: `${a.name} is suspended`, meta: `By ${a.suspension?.by ?? 'an admin'}${a.suspension?.reason ? `: ${a.suspension.reason}` : ''}`, btn: 'Open incident', href: `#/control/agents/${a.id}` });
		}
		return out;
	});

	const g = $derived(sum?.governed_7d);
	const pct = (n: number, total: number) => (total ? Math.round((n / total) * 100) : 0);
	const hours = $derived(sum?.decisions_24h.hours ?? []);
	const peak = $derived(Math.max(1, ...hours.map((h) => h.allowed + h.approval + h.denied)));
	const hourLabel = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
	const ticks = $derived(hours.length ? [0, 6, 12, 18].map((i) => hours[i]).filter(Boolean).map((h) => hourLabel(h.hour)) : []);
	const n = (v: number | undefined) => (v ?? 0).toLocaleString();
</script>

<svelte:head><title>Control plane · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-5 px-4 pt-8 pb-[calc(3rem+env(safe-area-inset-bottom))] md:px-8">
	<PageHead title="Dashboard" sub="Every agent in {orgName}: who owns it, what it is doing, and what it is allowed to do." />

	{#if error && !sum}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">Could not load the control plane: {error}</p>
	{/if}

	<div class="grid grid-cols-[repeat(auto-fit,minmax(210px,1fr))] gap-3">
		<div class="flex flex-col gap-1 rounded-xl border bg-card px-4.5 py-4">
			<div class="text-[12px] text-muted-foreground">Registered agents</div>
			<div class="tnum text-[28px] font-semibold tracking-tight">{sum ? sum.agents.total : '–'}</div>
			<div class="text-[12px] text-muted-foreground">{sum?.agents.teams ?? 0} team{sum?.agents.teams === 1 ? '' : 's'}{#if sum?.agents.unowned}{' · '}<span class="text-queue-ink">{sum.agents.unowned} without an owner</span>{/if}</div>
		</div>
		<div class="flex flex-col gap-1 rounded-xl border bg-card px-4.5 py-4">
			<div class="text-[12px] text-muted-foreground">Active now</div>
			<div class="tnum text-[28px] font-semibold tracking-tight">{sum ? sum.agents.running + sum.agents.waiting : '–'}</div>
			<div class="text-[12px] text-muted-foreground">{sum?.agents.running ?? 0} running · {sum?.agents.waiting ?? 0} waiting on approval</div>
		</div>
		<div class="flex flex-col gap-1 rounded-xl border bg-card px-4.5 py-4">
			<div class="text-[12px] text-muted-foreground">Suspended</div>
			<div class="tnum text-[28px] font-semibold tracking-tight">{sum ? sum.agents.suspended : '–'}</div>
			<div class="text-[12px] text-muted-foreground">New tasks and tool calls blocked</div>
		</div>
		<div class="flex flex-col gap-1 rounded-xl border bg-card px-4.5 py-4">
			<div class="text-[12px] text-muted-foreground">Pending approvals</div>
			<div class="flex items-center justify-between gap-2">
				<span class="tnum text-[28px] font-semibold tracking-tight">{sum ? sum.approvals.pending : '–'}</span>
				<Button href="#/control/approvals" variant="outline" size="sm" class="h-7 text-[12px]">Review</Button>
			</div>
			<div class="text-[12px] text-muted-foreground">{soonest}</div>
		</div>
	</div>

	<div class="grid grid-cols-[repeat(auto-fit,minmax(min(340px,100%),1fr))] gap-3">
		<div class="flex flex-col gap-3.5 rounded-xl border bg-card p-5">
			<h2 class="text-[14px] font-semibold">Governed activity</h2>
			{#if g && g.total}
				<div class="flex flex-wrap items-baseline gap-3">
					<span class="tnum text-[40px] leading-none font-semibold tracking-tight">{pct(g.governed, g.total)}%</span>
					<span class="flex-[1_1_220px] text-[13px] text-muted-foreground">of agent actions in the last 7 days were visible, attributed to an owner, and checked against a policy.</span>
				</div>
				<div class="flex h-2.5 gap-0.5 overflow-hidden rounded-full" role="img" aria-label="{pct(g.governed, g.total)}% governed, {pct(g.no_policy, g.total)}% observed with no matching policy, {pct(g.unattributed, g.total)}% unattributed">
					<div style="flex: {g.governed}" class="bg-foreground"></div>
					<div style="flex: {g.no_policy}" class="bg-queue"></div>
					<div style="flex: {g.unattributed}" class="bg-err"></div>
				</div>
				<div class="flex flex-col gap-1.5 text-[12.5px]">
					<div class="flex justify-between gap-2"><span><span class="mr-1.5 inline-block size-2.5 rounded-xs bg-foreground align-[-1px]"></span>Governed</span><span class="tnum text-muted-foreground">{n(g.governed)} actions · {pct(g.governed, g.total)}%</span></div>
					<div class="flex justify-between gap-2"><span><span class="mr-1.5 inline-block size-2.5 rounded-xs bg-queue align-[-1px]"></span>Observed, no matching policy</span><span class="tnum text-muted-foreground">{n(g.no_policy)} · {pct(g.no_policy, g.total)}%</span></div>
					<div class="flex justify-between gap-2"><span><span class="mr-1.5 inline-block size-2.5 rounded-xs bg-err align-[-1px]"></span>Unattributed (agent has no owner)</span><span class="tnum text-muted-foreground">{n(g.unattributed)} · {pct(g.unattributed, g.total)}%</span></div>
				</div>
			{:else}
				<p class="text-[13px] text-muted-foreground">No tool calls in the last 7 days yet. Once agents run, this shows how many of their actions were checked against a policy and attributed to an owner.</p>
			{/if}
		</div>

		<div class="flex flex-col gap-3.5 rounded-xl border bg-card p-5">
			<div class="flex flex-wrap items-center justify-between gap-2">
				<h2 class="text-[14px] font-semibold">Policy decisions, last 24 hours</h2>
				<span class="tnum text-[12px] text-muted-foreground">{n(sum?.decisions_24h.total)} evaluated</span>
			</div>
			<div class="flex flex-wrap gap-4 text-[12px] text-muted-foreground">
				<span><span class="mr-1.5 inline-block size-2.5 rounded-xs bg-chart-3 align-[-1px]"></span>Allowed {n(sum?.decisions_24h.allowed)}</span>
				<span><span class="mr-1.5 inline-block size-2.5 rounded-xs bg-queue align-[-1px]"></span>Approval required {n(sum?.decisions_24h.approval)}</span>
				<span><span class="mr-1.5 inline-block size-2.5 rounded-xs bg-err align-[-1px]"></span>Denied {n(sum?.decisions_24h.denied)}</span>
			</div>
			<div class="flex h-34 items-end gap-[3px]" role="img" aria-label="Hourly policy decisions over the last 24 hours">
				{#each hours as h (h.hour)}
					{@const tot = h.allowed + h.approval + h.denied}
					<div class="flex h-full flex-1 flex-col justify-end gap-px" title="{hourLabel(h.hour)} · {h.allowed} allowed, {h.approval} approval, {h.denied} denied">
						{#if h.denied}<div class="rounded-[2px] bg-err" style="height: {Math.max(2, (h.denied / peak) * 100)}%"></div>{/if}
						{#if h.approval}<div class="rounded-[2px] bg-queue" style="height: {Math.max(2, (h.approval / peak) * 100)}%"></div>{/if}
						<div class="rounded-[2px] bg-chart-3" style="height: {tot ? Math.max(2, (h.allowed / peak) * 100) : 1}%"></div>
					</div>
				{/each}
			</div>
			<div class="tnum flex justify-between text-[12px] text-muted-foreground">
				{#each ticks as t (t)}<span>{t}</span>{/each}<span>Now</span>
			</div>
		</div>
	</div>

	<div class="rounded-xl border bg-card">
		<div class="flex items-center justify-between px-5 pt-4 pb-2.5"><h2 class="text-[14px] font-semibold">Needs attention</h2><span class="text-[12px] text-muted-foreground">{attention.length} item{attention.length === 1 ? '' : 's'}</span></div>
		{#each attention as t, i (i)}
			<div class="flex flex-wrap items-center gap-3.5 border-t px-5 py-3">
				<Chip tone={t.tone} class="min-w-27 justify-center">{t.kind}</Chip>
				<div class="min-w-0 flex-[1_1_280px]">
					<div class="font-medium">{t.title}</div>
					<div class="text-[12px] text-muted-foreground">{t.meta}</div>
				</div>
				<Button href={t.href} variant="outline" size="sm" class="h-7 text-[12px]">{t.btn}</Button>
			</div>
		{/each}
		{#if !attention.length}<div class="border-t px-5 py-5 text-[13px] text-muted-foreground">Nothing needs you: no approvals waiting, every agent has an owner, none is suspended.</div>{/if}
	</div>

	<div class="rounded-xl border bg-card">
		<div class="flex items-center justify-between px-5 pt-4 pb-2.5"><h2 class="text-[14px] font-semibold">Recent policy events</h2><Button href="#/control/activity" variant="outline" size="sm" class="h-7 text-[12px]">All activity</Button></div>
		<ActivityRows events={recent} head={false} empty="No denials, approvals or failures yet." />
	</div>
</section>
