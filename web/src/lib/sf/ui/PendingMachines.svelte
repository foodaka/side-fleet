<script lang="ts">
	// Machines that ran `sidefleet connect` without a token and wait to be let in.
	// Self-hosted, they are listed here directly. Hosted, a request belongs to no
	// org until someone enters the code its terminal shows (or opens its link).
	import { onMount } from 'svelte';
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { toast } from 'svelte-sonner';
	import { api } from '../api';
	import { fleet } from '../state.svelte';
	import type { Enrollment } from '../types';

	/** Requests found by code (not in the org's list until approved). */
	let found = $state<Enrollment[]>([]);
	let code = $state('');
	let looking = $state(false);
	let busy = $state<string | null>(null);
	const hosted = $derived(fleet.setup?.hosted ?? fleet.methods?.hosted ?? false);
	const items = $derived([...found, ...fleet.pending.filter((p) => !found.some((f) => f.id === p.id))]);

	async function lookup(c: string, quiet = false) {
		if (!c.trim()) return;
		looking = true;
		try {
			const e = await api<Enrollment>('/v1/enrollments/lookup', { method: 'POST', body: JSON.stringify({ code: c }) });
			found = [e, ...found.filter((f) => f.id !== e.id)];
			code = '';
		} catch (err) {
			if (!quiet || !(err instanceof Error && /no machine/.test(err.message))) toast.error((err as Error).message);
			else toast.error('That approval link has expired', { description: 'Run `sidefleet connect` on the machine again for a new code.' });
		} finally {
			looking = false;
		}
	}

	async function decide(e: Enrollment, approve: boolean) {
		busy = e.id;
		try {
			await api(`/v1/enrollments/${e.id}/${approve ? 'approve' : 'deny'}`, { method: 'POST', body: JSON.stringify({ code: e.code }) });
			found = found.filter((f) => f.id !== e.id);
			fleet.pending = fleet.pending.filter((p) => p.id !== e.id);
			if (approve) toast.success(`${e.machine_name} approved`, { description: 'It shows up here as soon as its first heartbeat lands.' });
			else toast(`${e.machine_name} denied`);
			void fleet.refresh();
		} catch (err) {
			toast.error(approve ? 'Could not approve' : 'Could not deny', { description: (err as Error).message });
		} finally {
			busy = null;
		}
	}

	function minutesLeft(e: Enrollment) {
		return Math.max(0, Math.ceil((new Date(e.expires_at).getTime() - fleet.now) / 60000));
	}

	onMount(() => {
		// The link `sidefleet connect` prints: #/machines?code=XXXX-XXXX
		const q = new URLSearchParams(location.hash.split('?')[1] ?? '');
		const c = q.get('code');
		if (c) {
			void lookup(c, true);
			history.replaceState(history.state, '', '#/machines');
		}
	});
</script>

{#if items.length || hosted}
	<section class="flex flex-col gap-3" aria-labelledby="pending-h">
		<div class="flex flex-wrap items-end justify-between gap-3">
			<div>
				<h2 id="pending-h" class="text-[15px] font-semibold tracking-tight">Pending machines</h2>
				<p class="mt-0.5 text-[13px] text-muted-foreground">
					{hosted
						? 'Run `sidefleet connect --control ' + location.origin + '` on a machine, then enter the code it prints.'
						: 'Machines that asked to join with `sidefleet connect` and no token.'}
				</p>
			</div>
			{#if hosted}
				<form class="flex gap-2" onsubmit={(e) => { e.preventDefault(); void lookup(code); }}>
					<Input bind:value={code} placeholder="ABCD-EFGH" maxlength={12} class="h-8 w-32 font-mono text-[13px] uppercase" aria-label="Code from the machine" />
					<Button type="submit" variant="outline" class="h-8 rounded-full text-[13px]" disabled={looking || !code.trim()}>Find</Button>
				</form>
			{/if}
		</div>
		{#each items as e (e.id)}
			{@const claude = e.capabilities.agents?.claude}
			<article class="flex flex-wrap items-center gap-x-6 gap-y-3 rounded-xl border border-queue/40 bg-queue-soft/40 p-4">
				<div class="min-w-0 flex-1">
					<h3 class="truncate text-[14px] font-semibold">{e.machine_name}</h3>
					<p class="mt-0.5 flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
						<span>{e.capabilities.os || 'unknown OS'}{e.capabilities.hostname && e.capabilities.hostname !== e.machine_name ? `, ${e.capabilities.hostname}` : ''}</span>
						{#if claude}
							<span class="inline-flex items-center gap-1 {claude.signed_in ? '' : 'text-err-ink'}">
								{#if claude.signed_in}<CircleCheck class="size-3.5 text-ok" />{:else}<TriangleAlert class="size-3.5" />{/if}Claude Code {claude.version}
							</span>
						{/if}
						<span class="tnum">{e.capabilities.repo_count ?? 0} repos</span>
						<span class="tnum">expires in {minutesLeft(e)} min</span>
					</p>
				</div>
				<div class="flex flex-col items-start">
					<span class="text-[11px] text-muted-foreground">Code on that machine</span>
					<span class="font-mono text-[15px] font-semibold tracking-wider">{e.code}</span>
				</div>
				<div class="flex gap-2">
					<Button variant="outline" class="h-8 rounded-full text-[13px]" disabled={busy === e.id} onclick={() => decide(e, false)}>Deny</Button>
					<Button class="h-8 rounded-full text-[13px]" disabled={busy === e.id} onclick={() => decide(e, true)}>Approve</Button>
				</div>
			</article>
		{/each}
	</section>
{/if}
