<script lang="ts">
	// A task's budget: the key its machine made for it, waiting for you to sign it with
	// your passkey wallet (or `sidefleet wallet authorize` on a laptop). It waits in the
	// bell and sends a push like an approval; once decided it folds to one line.
	import { toast } from 'svelte-sonner';
	import Wallet from '@lucide/svelte/icons/wallet';
	import Fingerprint from '@lucide/svelte/icons/fingerprint';
	import Check from '@lucide/svelte/icons/check';
	import X from '@lucide/svelte/icons/x';
	import { Button } from '#lib/components/ui/button';
	import CopyButton from '../ui/CopyButton.svelte';
	import { api } from '../api';
	import { prefs } from '../prefs.svelte';
	import { fleet } from '../state.svelte';
	import { usd } from '../format';
	import { balance, createWallet, fund, passkeysWork, signBudget } from '../wallet';
	import type { Item } from '../thread.svelte';

	type Budget = Extract<Item, { kind: 'budget' }>;
	let { item, live }: { item: Budget; live: boolean } = $props();

	/** The key's expiry: `Thu 14:00`. */
	const until = (iso: string) => new Date(iso).toLocaleString([], { weekday: 'short', hour: '2-digit', minute: '2-digit', hour12: false });
	const command = $derived(`sidefleet wallet authorize ${item.runId} --control ${location.origin}`);
	const askAbove = $derived(item.askAboveMicros === null ? '' : `, asks you over ${usd(item.askAboveMicros)}`);
	const pending = $derived(live && !item.authorized && !item.declined);

	let busy = $state<'sign' | 'create' | 'deny' | null>(null);
	let funds = $state<number | null>(null);
	$effect(() => {
		const w = prefs.wallet;
		if (!pending || !w) return;
		balance(w.address).then((b) => (funds = b)).catch(() => (funds = null));
	});

	async function make() {
		busy = 'create';
		try {
			const wallet = await createWallet(`Sidefleet wallet${fleet.me?.user ? ` (${fleet.me.user.login})` : ''}`);
			prefs.set({ wallet });
			// Testnet: start it with test dollars.
			await fund(wallet.address).catch(() => {});
			funds = await balance(wallet.address).catch(() => null);
		} catch (e) {
			toast.error('Could not make a passkey wallet', { description: (e as Error).message });
		}
		busy = null;
	}

	async function sign() {
		const wallet = prefs.wallet;
		if (!wallet) return;
		busy = 'sign';
		try {
			const authorization = await signBudget(wallet, item, item.budgetMicros);
			await api(`/v1/runs/${item.runId}/pay-key/authorize`, {
				method: 'POST',
				body: JSON.stringify({ wallet: wallet.address, authorization, key_id: item.address })
			});
		} catch (e) {
			toast.error('Could not authorize the budget', { description: (e as Error).message });
		}
		busy = null;
	}

	async function decline() {
		if (!item.approvalId) return;
		busy = 'deny';
		try {
			await api(`/v1/runs/${item.runId}/approvals/${item.approvalId}/deny`, { method: 'POST', body: JSON.stringify({ message: 'The user started the task without a budget.' }) });
		} catch (e) {
			toast.error('Could not start without payments', { description: (e as Error).message });
		}
		busy = null;
	}
</script>

{#if pending}
	<div class="my-1.5 rounded-xl border border-queue/40 bg-queue-soft/40 p-3 sm:p-3.5" role="group" aria-label="Authorize this task's budget">
		<p class="flex items-center gap-2 text-[13.5px] font-medium text-foreground">
			<Wallet class="size-4 shrink-0 text-queue-ink" />
			<span>Authorize a budget of {usd(item.budgetMicros)}</span>
		</p>
		<p class="mt-1.5 text-[12.5px] text-muted-foreground">
			The machine made a key for this task. Signing it lets the agent spend up to {usd(item.budgetMicros)} until {until(item.expiresAt)}{askAbove}.
			Tempo stops the key at that limit, whatever happens on the machine.
		</p>
		{#if prefs.wallet && funds !== null && funds < item.budgetMicros}
			<p class="mt-1.5 text-[12.5px] text-err-ink">Your wallet has {usd(funds)}, less than the budget. Add test dollars in Settings.</p>
		{/if}
		<div class="mt-3 flex flex-wrap items-center gap-2">
			{#if !passkeysWork()}
				<span class="text-[12.5px] text-muted-foreground">Passkeys need a secure page (https or localhost); sign on your laptop instead.</span>
			{:else if prefs.wallet}
				<Button size="sm" class="h-9 rounded-full px-4 max-sm:flex-1" disabled={!!busy} onclick={sign}>
					<Fingerprint class="size-4" />{busy === 'sign' ? 'Signing…' : 'Authorize with passkey'}
				</Button>
			{:else}
				<Button size="sm" class="h-9 rounded-full px-4 max-sm:flex-1" disabled={!!busy} onclick={make}>
					<Fingerprint class="size-4" />{busy === 'create' ? 'Making your wallet…' : 'Make a passkey wallet'}
				</Button>
			{/if}
			{#if item.approvalId}
				<Button size="sm" variant="outline" class="h-9 rounded-full px-4 max-sm:flex-1" disabled={!!busy} onclick={decline}>
					<X class="size-4" />{busy === 'deny' ? 'Starting…' : 'Start without payments'}
				</Button>
			{/if}
		</div>
		<details class="mt-2.5 text-[12px] text-muted-foreground">
			<summary class="cursor-pointer select-none hover:text-foreground">Or sign on your laptop</summary>
			<div class="mt-1.5 flex items-start gap-2 rounded-lg bg-code px-3 py-2">
				<code class="min-w-0 flex-1 font-mono text-[12px] leading-relaxed break-all text-foreground/90">{command}</code>
				<CopyButton text={() => command} class="bg-foreground text-background hover:bg-foreground/85" />
			</div>
			<p class="mt-1">Needs <code class="font-mono">SIDEFLEET_API_TOKEN</code>: an API token with full access, from the Chat apps page.</p>
		</details>
	</div>
{:else if item.authorized}
	<p class="flex min-w-0 items-center gap-2 py-1 text-[13px] text-ok-ink">
		<span class="grid size-5 shrink-0 place-items-center rounded bg-ok-soft"><Check class="size-3.5" /></span>
		<span class="shrink-0">Budget authorized: {usd(item.budgetMicros)} until {until(item.expiresAt)}{askAbove}</span>
		<code class="min-w-0 truncate rounded bg-code px-1.5 py-0.5 font-mono text-[12px] text-foreground/85" title="The task's key">{item.address}</code>
	</p>
{:else}
	<p class="py-1 text-[13px] text-muted-foreground">The budget of {usd(item.budgetMicros)} wasn't authorized, so the agent ran without payments.</p>
{/if}
