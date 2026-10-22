<script lang="ts">
	// Per-org worker tokens: one per machine, so a lost laptop is one revoke away.
	// The value is shown once, right after it is created. Owners only.
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { toast } from 'svelte-sonner';
	import { api } from '../api';
	import { fleet } from '../state.svelte';
	import { rel } from '../format';
	import Command from './Command.svelte';
	import type { WorkerToken } from '../types';

	let tokens = $state<WorkerToken[] | null>(null);
	let name = $state('');
	let busy = $state(false);
	let fresh = $state<WorkerToken | null>(null);
	const org = $derived(fleet.org);

	async function load(id: string) {
		try {
			tokens = await api<WorkerToken[]>(`/v1/orgs/${id}/tokens`);
		} catch {
			tokens = [];
		}
	}
	// Reload when the org changes or a machine joins (approving one mints a token).
	$effect(() => {
		void fleet.workers.length;
		if (org && fleet.isOwner) void load(org.id);
	});

	async function create(e: SubmitEvent) {
		e.preventDefault();
		if (!org) return;
		busy = true;
		try {
			fresh = await api<WorkerToken>(`/v1/orgs/${org.id}/tokens`, { method: 'POST', body: JSON.stringify({ name: name.trim() }) });
			name = '';
			await load(org.id);
		} catch (err) {
			toast.error('Could not create the token', { description: (err as Error).message });
		} finally {
			busy = false;
		}
	}

	async function revoke(t: WorkerToken) {
		if (!org || !confirm(`Revoke "${t.name}"? Machines using it are disconnected on their next heartbeat.`)) return;
		try {
			await api(`/v1/orgs/${org.id}/tokens/${t.id}`, { method: 'DELETE' });
			if (fresh?.id === t.id) fresh = null;
			toast(`Revoked ${t.name}`);
			await load(org.id);
		} catch (err) {
			toast.error('Could not revoke', { description: (err as Error).message });
		}
	}
	const live = $derived((tokens ?? []).filter((t) => !t.revoked_at));
	const revoked = $derived((tokens ?? []).filter((t) => t.revoked_at));
</script>

{#if org && fleet.isOwner}
	<section class="flex flex-col gap-3" aria-labelledby="tokens-h">
		<div class="flex flex-wrap items-end justify-between gap-3">
			<div>
				<h2 id="tokens-h" class="text-[15px] font-semibold tracking-tight">Worker tokens</h2>
				<p class="mt-0.5 text-[13px] text-muted-foreground">For connecting machines from a script. Approving machines above is easier; it makes one of these per machine.</p>
			</div>
			<form class="flex gap-2" onsubmit={create}>
				<Input bind:value={name} placeholder="Token name, e.g. CI runner" maxlength={80} class="h-8 w-56 text-[13px]" aria-label="Token name" />
				<Button type="submit" variant="outline" class="h-8 rounded-full text-[13px]" disabled={busy || !name.trim()}>Create token</Button>
			</form>
		</div>

		{#if fresh?.token}
			<div class="flex flex-col gap-2 rounded-xl border border-ok/40 bg-ok-soft/40 p-4">
				<p class="text-[13px]"><span class="font-medium">{fresh.name}</span> created. Copy it now: it is not shown again.</p>
				<Command shown={`sidefleet connect --control ${location.origin} --token ${fresh.token}`} />
				<button type="button" class="w-fit text-xs text-muted-foreground hover:text-foreground" onclick={() => (fresh = null)}>Done, hide it</button>
			</div>
		{/if}

		{#if tokens === null}
			<p class="text-[13px] text-muted-foreground">Loading…</p>
		{:else if !tokens.length}
			<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">No tokens yet.</p>
		{:else}
			<ul class="divide-y rounded-xl border bg-card">
				{#each [...live, ...revoked] as t (t.id)}
					<li class="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5 text-[13px] {t.revoked_at ? 'opacity-60' : ''}">
						<span class="min-w-0 flex-1 truncate font-medium">{t.name}</span>
						<span class="font-mono text-xs text-muted-foreground">{t.prefix}…</span>
						<span class="text-xs text-muted-foreground">created {rel(t.created_at, fleet.now)}</span>
						<span class="text-xs text-muted-foreground">{t.last_used_at ? `used ${rel(t.last_used_at, fleet.now)}` : 'never used'}</span>
						{#if t.revoked_at}
							<span class="text-xs text-err-ink">revoked {rel(t.revoked_at, fleet.now)}</span>
						{:else}
							<button type="button" class="text-xs text-err-ink hover:underline" onclick={() => revoke(t)}>Revoke</button>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</section>
{/if}
