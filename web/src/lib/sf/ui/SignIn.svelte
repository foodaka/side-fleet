<script lang="ts">
	// Shown when nobody is signed in: GitHub sign-in on a hosted control plane,
	// the developer key on a self-hosted one (both when both are configured).
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import Logo from './Logo.svelte';
	import { fleet } from '../state.svelte';
	import { getKey } from '../api';

	const github = $derived(fleet.methods?.github ?? false);
	const devKey = $derived(fleet.methods?.dev_key ?? true);
	let showKey = $state(false);
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
	<div class="flex w-full max-w-[26rem] flex-col gap-5">
		<div class="flex items-center gap-2 text-[15px] font-semibold tracking-tight"><Logo />Sidefleet</div>
		{#if fleet.methods === null}
			<p class="text-[13px] text-muted-foreground">Connecting…</p>
		{:else if github}
			<div class="flex flex-col gap-2">
				<h1 class="text-2xl font-medium tracking-tight">Sign in to Sidefleet</h1>
				<p class="text-[13px] leading-relaxed text-muted-foreground">
					Run coding agents on your own machines, from this page. Your machines keep their own Claude Code sign-in; Sidefleet never sees an API key.
				</p>
			</div>
			<Button href="/auth/github/login" class="h-10 gap-2 rounded-full" data-sveltekit-reload>
				<svg viewBox="0 0 16 16" class="size-4" aria-hidden="true" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" /></svg>
				Continue with GitHub
			</Button>
			<p class="text-xs leading-relaxed text-muted-foreground">We read your GitHub login and email to create your account and a personal org. No repository access is requested.</p>
			{#if devKey && !showKey}
				<button type="button" class="w-fit text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline" onclick={() => (showKey = true)}>Use a developer key instead</button>
			{/if}
		{:else}
			<div class="flex flex-col gap-2">
				<h1 class="text-2xl font-medium tracking-tight">Connect to your control plane</h1>
				<p class="text-[13px] leading-relaxed text-muted-foreground">
					Paste the developer key that <code class="font-mono text-[12px] text-foreground">sidefleet serve</code> printed when it started. It is also in
					<code class="font-mono text-[12px] text-foreground">~/.sidefleet/server.env</code> on that machine, and it stays in this browser.
				</p>
			</div>
		{/if}
		{#if fleet.methods !== null && devKey && (!github || showKey)}
			<form onsubmit={submit} class="flex flex-col gap-5">
				<div class="flex flex-col gap-2">
					<label for="devkey" class="text-xs font-medium">Developer key</label>
					<!-- svelte-ignore a11y_autofocus -->
					<Input id="devkey" type="password" autocomplete="off" spellcheck={false} bind:value={key} placeholder="SIDEFLEET_DEV_KEY" class="h-10 font-mono" autofocus />
					{#if error}<p class="text-xs text-err-ink" role="alert">{error}</p>{/if}
				</div>
				<Button type="submit" variant={github ? 'outline' : 'default'} class="h-10 rounded-full" disabled={busy || !key.trim()}>{busy ? 'Connecting…' : 'Connect'}</Button>
			</form>
		{/if}
	</div>
</div>
