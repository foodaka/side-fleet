<script lang="ts">
	// Settings: your passkey wallet for agent budgets (Tempo testnet).
	import { toast } from 'svelte-sonner';
	import Fingerprint from '@lucide/svelte/icons/fingerprint';
	import { Button } from '#lib/components/ui/button';
	import { prefs } from '../prefs.svelte';
	import { fleet } from '../state.svelte';
	import { usd } from '../format';
	import { balance, createWallet, fund, passkeysWork } from '../wallet';

	let funds = $state<number | null>(null);
	let busy = $state<'create' | 'fund' | null>(null);

	async function refresh() {
		const w = prefs.wallet;
		funds = w ? await balance(w.address).catch(() => null) : null;
	}
	$effect(() => {
		void prefs.wallet;
		void refresh();
	});

	async function make() {
		busy = 'create';
		try {
			const wallet = await createWallet(`Sidefleet wallet${fleet.me?.user ? ` (${fleet.me.user.login})` : ''}`);
			prefs.set({ wallet });
			await fund(wallet.address).catch(() => {});
			await refresh();
		} catch (e) {
			toast.error('Could not make a passkey wallet', { description: (e as Error).message });
		}
		busy = null;
	}

	async function addFunds() {
		if (!prefs.wallet) return;
		busy = 'fund';
		try {
			await fund(prefs.wallet.address);
			await new Promise((r) => setTimeout(r, 2000));
			await refresh();
		} catch (e) {
			toast.error('The faucet did not answer', { description: (e as Error).message });
		}
		busy = null;
	}
</script>

<div class="flex flex-col gap-3">
	<div>
		<h2 class="text-[15px] font-medium">Wallet</h2>
		<p class="mt-0.5 text-[12.5px] text-muted-foreground">
			Signs each task's budget, so its agent can pay for APIs. Your passkey is the wallet's key and stays on your device; Sidefleet only keeps its address. Tempo testnet: test dollars only.
		</p>
	</div>
	<div class="flex flex-wrap items-center gap-3 rounded-xl border bg-card p-4">
		{#if prefs.wallet}
			<div class="flex min-w-0 flex-1 flex-col gap-0.5">
				<span class="text-[13.5px] font-medium">Passkey wallet</span>
				<code class="truncate font-mono text-[12px] text-muted-foreground" title={prefs.wallet.address}>{prefs.wallet.address}</code>
			</div>
			<span class="tnum text-[15px] font-semibold">{funds === null ? '–' : usd(funds)}</span>
			<Button size="sm" variant="outline" class="h-8 rounded-full px-3.5" disabled={!!busy} onclick={addFunds}>{busy === 'fund' ? 'Adding…' : 'Add test dollars'}</Button>
			<Button size="sm" variant="ghost" class="h-8 rounded-full px-3 text-muted-foreground" disabled={!!busy} onclick={() => prefs.set({ wallet: null })}>Forget</Button>
		{:else if passkeysWork()}
			<p class="min-w-0 flex-1 text-[13px] text-muted-foreground">No wallet yet. Make one with a passkey; it starts with test dollars.</p>
			<Button size="sm" class="h-8 rounded-full px-3.5" disabled={!!busy} onclick={make}><Fingerprint class="size-4" />{busy === 'create' ? 'Making…' : 'Make a passkey wallet'}</Button>
		{:else}
			<p class="text-[13px] text-muted-foreground">Passkeys need a secure page (https or localhost). Use <code class="font-mono">sidefleet wallet</code> on your laptop instead.</p>
		{/if}
	</div>
</div>
