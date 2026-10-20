<script lang="ts">
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import Logo from './Logo.svelte';
	import { fleet } from '../state.svelte';
	import { getKey } from '../api';

	let key = $state('');
	let error = $state(getKey() ? 'The saved key no longer works. Paste the current one.' : '');
	let busy = $state(false);
	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!key.trim()) return;
		busy = true;
		const ok = await fleet.signIn(key.trim());
		busy = false;
		if (!ok) error = 'That key was not accepted. Check SIDEFLEET_DEV_KEY on the control plane.';
	}
</script>

<div class="fixed inset-0 z-50 grid place-items-center bg-background p-4">
	<form onsubmit={submit} class="flex w-full max-w-[26rem] flex-col gap-5">
		<div class="flex items-center gap-2 text-[15px] font-semibold tracking-tight"><Logo />Sidefleet</div>
		<div class="flex flex-col gap-2">
			<h1 class="text-2xl font-medium tracking-tight">Connect to your control plane</h1>
			<p class="text-[13px] leading-relaxed text-muted-foreground">
				Paste the developer key that <code class="font-mono text-[12px] text-foreground">sidefleet serve</code> printed when it started. It is also in
				<code class="font-mono text-[12px] text-foreground">~/.sidefleet/server.env</code> on that machine, and it stays in this browser.
			</p>
		</div>
		<div class="flex flex-col gap-2">
			<label for="devkey" class="text-xs font-medium">Developer key</label>
			<Input id="devkey" type="password" autocomplete="off" spellcheck={false} bind:value={key} placeholder="SIDEFLEET_DEV_KEY" class="h-10 font-mono" autofocus />
			{#if error}<p class="text-xs text-err-ink" role="alert">{error}</p>{/if}
		</div>
		<Button type="submit" class="h-10 rounded-full" disabled={busy || !key.trim()}>{busy ? 'Connecting…' : 'Connect'}</Button>
	</form>
</div>
