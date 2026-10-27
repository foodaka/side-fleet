<script lang="ts">
	// An invite link: who invited you where, then GitHub sign-in (if needed) and joining.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { Button } from '#lib/components/ui/button';
	import { toast } from 'svelte-sonner';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { JOIN_KEY } from '#lib/sf/join';
	import type { InviteInfo, Org } from '#lib/sf/types';
	import Avatar from '#lib/sf/ui/Avatar.svelte';
	import Logo from '#lib/sf/ui/Logo.svelte';

	const code = $derived(page.params.code ?? '');
	let info = $state<InviteInfo | null>(null);
	let failed = $state(false);
	let joining = $state(false);

	$effect(() => {
		const c = code;
		info = null;
		failed = false;
		if (!c) return;
		fetch(`/v1/invites/${encodeURIComponent(c)}`)
			.then(async (r) => {
				if (r.ok) info = await r.json();
				else if (r.status === 404) info = { org_name: '', inviter_login: null, role: 'member', valid: false, reason: 'not_found' };
				else failed = true;
			})
			.catch(() => (failed = true));
	});

	const user = $derived(fleet.me?.user ?? null);
	const already = $derived(!!info && !!fleet.me?.orgs.some((o) => o.name === info!.org_name && o.slug !== user?.login.toLowerCase()));
	const why: Record<string, string> = {
		expired: 'This invite has expired.',
		revoked: 'This invite was revoked.',
		used_up: 'This invite has already been used.',
		not_found: "This invite link doesn't exist."
	};

	function signIn() {
		try { localStorage.setItem(JOIN_KEY, code); } catch { /* storage blocked: they'll need the link again */ }
		location.href = '/auth/github/login';
	}

	async function join() {
		joining = true;
		try {
			const r = await api<{ org: Org }>(`/v1/invites/${encodeURIComponent(code)}/accept`, { method: 'POST' });
			try { localStorage.removeItem(JOIN_KEY); } catch { /* ignore */ }
			fleet.switchOrg(r.org.id);
			toast.success(`Welcome to ${r.org.name}`);
			await goto('#/rooms');
		} catch (err) {
			toast.error('Could not join', { description: (err as Error).message });
		}
		joining = false;
	}
</script>

<svelte:head><title>Join {info?.org_name || 'a team'} · Sidefleet</title></svelte:head>

<div class="fixed inset-0 z-40 grid place-items-center bg-background p-4">
	<div class="flex w-full max-w-[26rem] flex-col gap-6">
		<div class="flex items-center gap-2 text-[15px] font-semibold tracking-tight"><Logo />Sidefleet</div>
		{#if failed}
			<p class="text-[13px] text-muted-foreground">Couldn't reach Sidefleet to read this invite. Check your connection and reload.</p>
		{:else if !info}
			<div class="h-8 w-3/4 animate-pulse rounded bg-muted"></div>
			<div class="h-10 animate-pulse rounded-full bg-muted"></div>
		{:else if !info.valid}
			<div class="flex flex-col gap-2">
				<h1 class="text-2xl font-medium tracking-tight">{why[info.reason ?? ''] ?? 'This invite no longer works.'}</h1>
				<p class="text-[13px] leading-relaxed text-muted-foreground">
					Ask {info.inviter_login ? `@${info.inviter_login}` : 'whoever sent it'} for a new link{info.org_name ? ` to ${info.org_name}` : ''}.
				</p>
			</div>
			<Button href="#/" variant="outline" class="h-10 rounded-full">Go to Sidefleet</Button>
		{:else}
			<div class="flex flex-col gap-3">
				{#if info.inviter_login}<Avatar login={info.inviter_login} size={44} />{/if}
				<h1 class="text-2xl font-medium leading-snug tracking-tight">
					{#if info.inviter_login}@{info.inviter_login} invited you to <span class="font-semibold">{info.org_name}</span>{:else}Join <span class="font-semibold">{info.org_name}</span>{/if}
				</h1>
				<p class="text-[13px] leading-relaxed text-muted-foreground">
					Work alongside the team: see what everyone's agents are doing, split goals into tasks in rooms, and run your part on your own machines. Nobody else's tasks ever run on your machines.
				</p>
			</div>
			{#if user}
				{#if already}
					<p class="text-[13px] text-muted-foreground">You're already in {info.org_name}.</p>
				{/if}
				<Button class="h-10 rounded-full" onclick={join} disabled={joining}>{joining ? 'Joining…' : already ? `Open ${info.org_name}` : `Join ${info.org_name} as @${user.login}`}</Button>
			{:else if fleet.me && !user}
				<p class="rounded-lg bg-muted px-3 py-2.5 text-[13px] text-muted-foreground">You're signed in with a developer key. Teams need GitHub sign-in: sign out from Settings, then open this link again.</p>
			{:else if fleet.methods && !fleet.methods.github}
				<p class="rounded-lg bg-muted px-3 py-2.5 text-[13px] text-muted-foreground">This control plane doesn't have GitHub sign-in turned on, so it can't take invites.</p>
			{:else if fleet.conn === 'connecting' && !fleet.methods}
				<div class="h-10 animate-pulse rounded-full bg-muted"></div>
			{:else}
				<Button class="h-10 gap-2 rounded-full" onclick={signIn}>
					<svg viewBox="0 0 16 16" class="size-4" aria-hidden="true" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" /></svg>
					Continue with GitHub to join
				</Button>
				<p class="text-xs leading-relaxed text-muted-foreground">We read your GitHub login and email. No repository access is requested.</p>
			{/if}
		{/if}
	</div>
</div>
