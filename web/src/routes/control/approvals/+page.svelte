<script lang="ts">
	// Actions held at the execution boundary until someone allowed to approve decides.
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { Button } from '#lib/components/ui/button';
	import Check from '@lucide/svelte/icons/check';
	import X from '@lucide/svelte/icons/x';
	import Clock from '@lucide/svelte/icons/clock';
	import Lock from '@lucide/svelte/icons/lock';
	import ShieldCheck from '@lucide/svelte/icons/shield-check';
	import { fleet } from '#lib/sf/state.svelte';
	import { rel } from '#lib/sf/format';
	import { decideApproval, listApprovals, policyScopeLabel, remaining, shortDigest, writeError, APPROVAL_STATUS, type ControlApproval } from '#lib/sf/control';
	import { control } from '#lib/sf/control.svelte';
	import Chip from '#lib/sf/ui/control/Chip.svelte';
	import PageHead from '#lib/sf/ui/control/PageHead.svelte';
	import Tabs from '#lib/sf/ui/control/Tabs.svelte';

	let pending = $state<ControlApproval[] | null>(null);
	let resolved = $state<ControlApproval[] | null>(null);
	let counts = $state({ pending: 0, resolved: 0 });
	let error = $state('');
	let tab = $state('pending');
	let busy = $state<string | null>(null);

	async function load() {
		try {
			const [p, r] = await Promise.all([listApprovals('pending'), listApprovals('resolved')]);
			pending = p.approvals;
			resolved = r.approvals;
			counts = p.counts ?? { pending: p.approvals.length, resolved: r.approvals.length };
			error = '';
		} catch (e) {
			error = (e as Error).message;
		}
	}
	onMount(() => {
		void load();
		const t = setInterval(load, 5_000);
		return () => clearInterval(t);
	});

	async function decide(r: ControlApproval, verdict: 'approve' | 'reject') {
		busy = r.id;
		try {
			await decideApproval(r.id, verdict);
			const who = r.agent?.name ?? 'The agent';
			toast.success(`${verdict === 'approve' ? 'Approved' : 'Rejected'}. ${who} has been told, and the decision is in the audit log.`);
		} catch (e) {
			toast.error(verdict === 'approve' ? 'Could not approve' : 'Could not reject', { description: writeError(e, 'an org owner or the agent’s owner') });
		}
		busy = null;
		await load();
		void control.refresh();
	}

	const tabs = $derived([
		{ id: 'pending', label: `Pending (${counts.pending})` },
		{ id: 'resolved', label: `Resolved (${counts.resolved})` }
	]);
	const why = (r: ControlApproval) =>
		r.policy ? { name: r.policy.name, note: policyScopeLabel(r.policy) } : { name: 'Ask me', note: 'The task runs with "Ask me", so tool calls it would refuse come here' };
	const fmtWhen = (iso: string | null) => (iso ? new Date(iso).toLocaleString([], { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hour12: false }) : '');
	const resCols = 'grid grid-cols-[minmax(150px,1fr)_minmax(220px,2fr)_110px_minmax(150px,1fr)] items-center gap-3.5 min-w-[680px]';
</script>

<svelte:head><title>Approvals · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-5xl flex-col gap-4.5 px-4 pt-8 pb-[calc(3rem+env(safe-area-inset-bottom))] md:px-8">
	<PageHead title="Approvals" sub="Actions held at the execution boundary until someone allowed to approve decides. Nothing runs while it waits." />
	<Tabs {tabs} bind:value={tab} label="Approvals" />

	{#if error && !pending}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">Could not load approvals: {error}</p>
	{:else if !pending || !resolved}
		<p class="text-[13px] text-muted-foreground">Loading…</p>
	{:else if tab === 'pending'}
		{#if !pending.length}
			<div class="flex flex-col items-center gap-1.5 rounded-xl border bg-card px-5 py-10 text-center">
				<ShieldCheck class="size-5 text-ok" />
				<div class="font-semibold">Nothing is waiting on you</div>
				<div class="text-[13px] text-muted-foreground">Every agent is running inside its policies.</div>
			</div>
		{/if}
		{#each pending as r (r.id)}
			{@const w = why(r)}
			{@const left = new Date(r.expires_at).getTime() - fleet.now}
			<article class="overflow-hidden rounded-xl border bg-card">
				<div class="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-3.5">
					<div class="min-w-0">
						<div class="flex flex-wrap items-center gap-2">
							{#if r.agent}<a class="font-medium hover:underline" href="#/control/agents/{r.agent.id}">{r.agent.name}</a>{:else}<span class="font-medium">Unknown agent</span>{/if}
							<span class="text-[12px] text-muted-foreground">{r.runtime}</span>
						</div>
						<div class="text-[12px] text-muted-foreground">Owner {r.owner?.login ?? 'unassigned'} · requested {rel(r.created_at, fleet.now)}</div>
					</div>
					<Chip tone={left < 60_000 ? 'err' : 'warn'} dot={false}><Clock class="size-3" />{left > 0 ? `Expires in ${remaining(r.expires_at, fleet.now)}` : 'Expiring'}</Chip>
				</div>
				<div class="flex flex-col gap-3.5 px-5 py-4">
					<h3 class="text-[15px] font-semibold">{r.title}</h3>
					<pre class="m-0 rounded-lg border bg-code px-3 py-2.5 font-mono text-[12px] leading-relaxed break-words whitespace-pre-wrap">{r.command}</pre>
					{#if r.digest}
						<div class="flex items-start gap-1.5 text-[12px] text-muted-foreground">
							<Lock class="mt-0.5 size-3.5 shrink-0" />
							<span>Approval is bound to this exact action, digest <span class="font-mono" title={r.digest}>{shortDigest(r.digest)}</span>. If the agent changes the command, it has to ask again.</span>
						</div>
					{/if}
					<div class="grid grid-cols-[repeat(auto-fit,minmax(190px,1fr))] gap-3.5 text-[13px]">
						<div class="min-w-0"><div class="mb-0.5 text-[11.5px] text-muted-foreground">Task</div><div class="line-clamp-3">{r.context?.task || 'Not reported'}</div></div>
						<div class="min-w-0"><div class="mb-0.5 text-[11.5px] text-muted-foreground">Repository · branch</div><div class="truncate font-mono text-[12px]">{r.context?.repo || 'no repository'}</div>{#if r.context?.branch}<div class="truncate font-mono text-[12px] text-muted-foreground">{r.context.branch}</div>{/if}</div>
						<div class="min-w-0"><div class="mb-0.5 text-[11.5px] text-muted-foreground">Tool</div><div class="font-mono text-[12px]">{r.tool || r.tool_name}</div>{#if r.tool_name && r.tool_name !== r.tool}<div class="text-[12px] text-muted-foreground">{r.tool_name}</div>{/if}</div>
						<div class="min-w-0"><div class="mb-0.5 text-[11.5px] text-muted-foreground">Why it needs approval</div><div>{w.name}</div><div class="text-[12px] text-muted-foreground">{w.note}</div></div>
					</div>
				</div>
				<div class="flex flex-wrap items-center justify-end gap-2 border-t bg-muted px-5 py-3">
					<span class="mr-auto text-[12px] text-muted-foreground">
						{#if r.can_decide}You can decide this as {fleet.isOwner ? 'an org owner' : 'the agent’s owner'}.{:else}Only an org owner or the agent’s owner can decide this.{/if}
					</span>
					<Button variant="outline" size="sm" class="h-8 bg-card" disabled={!r.can_decide || busy === r.id} onclick={() => decide(r, 'reject')}><X class="size-3.5" />Reject</Button>
					<Button size="sm" class="h-8" disabled={!r.can_decide || busy === r.id} onclick={() => decide(r, 'approve')}><Check class="size-3.5" />Approve once</Button>
				</div>
			</article>
		{/each}
	{:else}
		<div class="overflow-x-auto rounded-xl border bg-card">
			<div class="{resCols} px-4 py-2.5 text-[11.5px] font-medium text-muted-foreground"><span>Agent</span><span>Requested action</span><span>Decision</span><span>Decided by</span></div>
			{#each resolved as r (r.id)}
				{@const s = APPROVAL_STATUS[r.status]}
				<div class="{resCols} border-t px-4 py-2.5 text-[13px]">
					{#if r.agent}<a class="truncate font-medium hover:underline" href="#/control/agents/{r.agent.id}">{r.agent.name}</a>{:else}<span class="text-muted-foreground">Unknown agent</span>{/if}
					<div class="min-w-0"><div class="truncate" title={r.command}>{r.title}</div>{#if r.digest}<div class="font-mono text-[12px] text-muted-foreground">{shortDigest(r.digest)}</div>{/if}</div>
					<span><Chip tone={s.tone}>{s.label}</Chip></span>
					<div class="min-w-0">
						<div class="truncate">{r.decided_by ?? (r.status === 'expired' ? 'Nobody (the wait ran out)' : r.status === 'cancelled' ? 'Sidefleet' : '–')}</div>
						<div class="text-[12px] text-muted-foreground" title={r.message ?? undefined}>{fmtWhen(r.decided_at)}</div>
					</div>
				</div>
			{/each}
			{#if !resolved.length}<div class="border-t px-5 py-5 text-[13px] text-muted-foreground">No decided approvals yet.</div>{/if}
		</div>
	{/if}
</section>
