<script lang="ts">
	// The append-only record of governance changes, its chain check, and an export.
	import { onMount, untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import Download from '@lucide/svelte/icons/download';
	import Search from '@lucide/svelte/icons/search';
	import RotateCw from '@lucide/svelte/icons/rotate-cw';
	import ShieldCheck from '@lucide/svelte/icons/shield-check';
	import ShieldAlert from '@lucide/svelte/icons/shield-alert';
	import { fleet } from '#lib/sf/state.svelte';
	import { clock, exportAudit, listAudit, shortHash, verifyAudit, ACTOR_KIND, type AuditRecord, type AuditVerify } from '#lib/sf/control';
	import Chip from '#lib/sf/ui/control/Chip.svelte';
	import PageHead from '#lib/sf/ui/control/PageHead.svelte';

	let records = $state<AuditRecord[] | null>(null);
	let total = $state(0);
	let error = $state('');
	let q = $state('');
	let selected = $state<string | null>(new URLSearchParams(location.hash.split('?')[1] ?? '').get('record'));
	let check = $state<AuditVerify | null>(null);
	let checking = $state(false);
	let more = $state(false);
	const PAGE = 100;

	let seq = 0;
	async function load() {
		const mine = ++seq;
		try {
			const r = await listAudit({ q: q.trim() || undefined, limit: PAGE });
			if (mine !== seq) return;
			records = r.records;
			total = r.total;
			more = r.records.length === PAGE;
			error = '';
			if (!selected) selected = r.records[0]?.id ?? null;
		} catch (e) {
			if (mine === seq) error = (e as Error).message;
		}
	}
	async function loadMore() {
		const last = records?.at(-1);
		if (!last) return;
		try {
			const r = await listAudit({ q: q.trim() || undefined, limit: PAGE, before: last.seq });
			records = [...(records ?? []), ...r.records];
			more = r.records.length === PAGE;
		} catch (e) {
			error = (e as Error).message;
		}
	}
	async function verify() {
		checking = true;
		try {
			check = await verifyAudit();
		} catch (e) {
			toast.error('Could not check the chain', { description: (e as Error).message });
		}
		checking = false;
	}

	let debounce: ReturnType<typeof setTimeout>;
	$effect(() => {
		void q;
		clearTimeout(debounce);
		debounce = setTimeout(() => untrack(() => void load()), 250);
	});
	onMount(() => {
		void verify();
		const t = setInterval(load, 15_000);
		return () => clearInterval(t);
	});

	async function download(format: 'csv' | 'jsonl') {
		try {
			await exportAudit(format, { q: q.trim() || undefined });
		} catch (e) {
			toast.error('Could not export', { description: (e as Error).message });
		}
	}

	// A record opened from an agent's page may be older than the first page: then the newest shows.
	const au = $derived(records?.find((r) => r.id === selected) ?? records?.[0] ?? null);
	const prev = $derived(au?.prev_hash ?? '');
	const verifiedAt = $derived(check ? new Date(check.verified_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }) : '');
	const cols = 'grid grid-cols-[72px_minmax(110px,1fr)_minmax(130px,1fr)_minmax(180px,2fr)_56px] items-center gap-3 min-w-[620px]';
	const rows: [string, keyof NonNullable<AuditRecord['data']>][] = [
		['What happened', 'what'],
		['Agent', 'agent'],
		['Owner', 'owner'],
		['Permissions', 'permissions'],
		['Policy evaluated', 'policy'],
		['Approval', 'approval'],
		['Decided by', 'decided_by'],
		['Outcome', 'outcome']
	];
</script>

<svelte:head><title>Audit log · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-6xl flex-col gap-4.5 px-4 pt-8 pb-[calc(3rem+env(safe-area-inset-bottom))] md:px-8">
	<PageHead title="Audit log" sub="Append-only record of every registration, policy change, approval, suspension and revocation.">
		{#snippet actions()}
			<Button variant="outline" size="sm" class="h-8 text-[13px]" onclick={() => download('csv')}><Download class="size-3.5" />Export CSV</Button>
			<Button variant="outline" size="sm" class="h-8 text-[13px]" onclick={() => download('jsonl')}><Download class="size-3.5" />Export JSONL</Button>
		{/snippet}
	</PageHead>

	<div class="flex flex-wrap items-center gap-3.5 rounded-xl border bg-card px-4.5 py-3.5">
		{#if !check}
			<Chip tone="off" dot={false}>{checking ? 'Checking…' : 'Not checked'}</Chip>
		{:else if check.intact}
			<Chip tone="ok" dot={false}><ShieldCheck class="size-3.5" />Chain intact</Chip>
		{:else}
			<Chip tone="err" dot={false}><ShieldAlert class="size-3.5" />Chain broken at record {check.broken_at}</Chip>
		{/if}
		<span class="flex-[1_1_300px] text-[13px] text-muted-foreground">
			<span class="tnum font-medium text-foreground">{(check?.records ?? total).toLocaleString()} records</span>
			· each one carries the hash of the record before it, so an edit or deletion breaks the chain.{#if verifiedAt}{' '}Last verified at {verifiedAt}.{/if}
			<button type="button" class="ml-1 inline-flex items-center gap-1 align-[-1px] hover:text-foreground disabled:opacity-50" onclick={verify} disabled={checking} aria-label="Verify again"><RotateCw class="size-3 {checking ? 'animate-spin' : ''}" /></button>
		</span>
		<label class="relative flex w-65 max-w-full items-center text-muted-foreground">
			<Search class="pointer-events-none absolute left-2.5 size-3.5" />
			<Input type="search" bind:value={q} placeholder="Search records" aria-label="Search audit records" class="h-8.5 pl-8 text-[13px]" />
		</label>
	</div>

	{#if error && !records}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">Could not load the audit log: {error}</p>
	{:else if !records}
		<p class="text-[13px] text-muted-foreground">Loading…</p>
	{:else}
		<div class="flex flex-wrap items-start gap-3">
			<div class="min-w-0 flex-[3_1_600px]">
				<div class="overflow-x-auto rounded-xl border bg-card">
					<div class="{cols} px-4 py-2.5 text-[11.5px] font-medium text-muted-foreground"><span>Time</span><span>Actor</span><span>Event</span><span>Target · detail</span><span>Hash</span></div>
					{#each records as r (r.id)}
						<div class="{cols} border-t px-4 py-2.5 text-[13px] {r.id === au?.id ? 'bg-accent' : ''}">
							<span class="tnum font-mono text-[12px] text-muted-foreground" title={new Date(r.at).toLocaleString()}>{clock(r.at, fleet.now)}</span>
							<div class="min-w-0"><div class="truncate">{r.actor}</div><div class="text-[12px] text-muted-foreground">{ACTOR_KIND[r.actor_kind] ?? r.actor_kind}</div></div>
							<button type="button" class="truncate text-left font-mono text-[12px] font-medium hover:underline" onclick={() => (selected = r.id)}>{r.event}</button>
							<div class="min-w-0"><div class="truncate font-medium">{r.target}</div><div class="truncate text-[12px] text-muted-foreground" title={r.detail}>{r.detail}</div></div>
							<span class="font-mono text-[12px] text-muted-foreground">{shortHash(r.hash)}</span>
						</div>
					{/each}
					{#if !records.length}<div class="border-t px-5 py-5 text-[13px] text-muted-foreground">{q ? 'No records match that search.' : 'No records yet.'}</div>{/if}
				</div>
				{#if more}<div class="mt-3 flex justify-center"><Button variant="outline" size="sm" onclick={loadMore}>Show older</Button></div>{/if}
			</div>

			<div class="min-w-0 flex-[1_1_300px] rounded-xl border bg-card px-5 py-4.5">
				<div class="mb-1.5 flex items-center justify-between gap-2"><h2 class="text-[14px] font-semibold">Investigate</h2>{#if au}<span class="font-mono text-[12px] text-muted-foreground">{au.event}</span>{/if}</div>
				{#if au}
					<dl class="m-0 text-[13px]">
						{#each rows as [label, k] (k)}
							<div class="grid grid-cols-[112px_minmax(0,1fr)] gap-2.5 border-t py-2">
								<dt class="text-muted-foreground">{label}</dt>
								<dd class="m-0 min-w-0 [overflow-wrap:anywhere]">{au.data?.[k] || (k === 'what' ? au.detail : 'Not applicable')}</dd>
							</div>
						{/each}
						<div class="grid grid-cols-[112px_minmax(0,1fr)] gap-2.5 border-t py-2">
							<dt class="text-muted-foreground">When</dt>
							<dd class="m-0">{new Date(au.at).toLocaleString()}</dd>
						</div>
						<div class="grid grid-cols-[112px_minmax(0,1fr)] gap-2.5 border-t py-2">
							<dt class="text-muted-foreground">Record</dt>
							<dd class="m-0 font-mono text-[12px]" title="{au.hash} ← {prev}">#{au.seq} · {shortHash(au.hash)} <span class="text-muted-foreground">← prev {prev ? shortHash(prev) : 'none (first record)'}</span></dd>
						</div>
					</dl>
				{:else}
					<p class="text-[13px] text-muted-foreground">Pick a record to see what happened.</p>
				{/if}
			</div>
		</div>
	{/if}
</section>
