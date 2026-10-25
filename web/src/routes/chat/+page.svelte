<script lang="ts">
	// Chat apps: connect the assistant you already chat with (Hermes, OpenClaw,
	// Claude Code) so it can run and check tasks on the fleet. Three steps on one
	// card: pick the app, pick what it may do, add the setup it gets; then the page
	// waits until the app first calls in, and offers updates in chat (the app
	// messages you when a task finishes or needs an OK). Each connection is a
	// personal API token.
	import { onDestroy } from 'svelte';
	import { Button } from '#lib/components/ui/button';
	import Check from '@lucide/svelte/icons/check';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { toast } from 'svelte-sonner';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { rel } from '#lib/sf/format';
	import Command from '#lib/sf/ui/Command.svelte';
	import ChatUpdates from '#lib/sf/ui/ChatUpdates.svelte';
	import { ACCESS, CHAT_APPS, EXAMPLES, accessOf, appOf, hasUpdates, setupMessage, setupSnippet, setupSteps, type ChatAppId } from '#lib/sf/chat';
	import type { ApiScope, ApiToken } from '#lib/sf/types';

	let tokens = $state<ApiToken[] | null>(null);
	let app = $state<ChatAppId | null>(null);
	let scope = $state<ApiScope>('tasks');
	let busy = $state(false);
	/** The connection just made: its key is only known now. */
	let fresh = $state<ApiToken | null>(null);
	let how = $state<'ask' | 'self'>('ask');
	/** Show the setup card even when there are connections already. */
	let adding = $state(false);
	let poll: ReturnType<typeof setInterval> | undefined;
	const org = $derived(fleet.org);

	async function load() {
		try {
			tokens = await api<ApiToken[]>('/v1/api-tokens');
		} catch {
			tokens = [];
		}
	}
	// The connections are per org: reload when it changes.
	$effect(() => {
		if (org) {
			void org.id;
			reset();
			void load();
		}
	});
	onDestroy(() => clearInterval(poll));

	const live = $derived((tokens ?? []).filter((t) => !t.revoked_at));
	const chosen = $derived(CHAT_APPS.find((a) => a.id === app));
	/** The connection being set up, as it is now (its first call, its updates). */
	const current = $derived(live.find((t) => t.id === fresh?.id));
	const connected = $derived(!!current?.last_used_at);
	const showSetup = $derived(adding || fresh !== null || (tokens !== null && live.length === 0));
	$effect(() => {
		if (connected) clearInterval(poll);
	});

	function pick(id: ChatAppId) {
		app = id;
		how = CHAT_APPS.find((a) => a.id === id)?.canSelfSetup ? 'ask' : 'self';
	}

	function reset() {
		clearInterval(poll);
		fresh = null;
		app = null;
		scope = 'tasks';
		adding = false;
	}

	async function connect() {
		if (!chosen) return;
		busy = true;
		try {
			const name = chosen.id === 'other' ? 'Other app' : chosen.name;
			fresh = await api<ApiToken>('/v1/api-tokens', { method: 'POST', body: JSON.stringify({ name, scope }) });
			await load();
			// Wait for the app's first call.
			clearInterval(poll);
			poll = setInterval(load, 3000);
		} catch (err) {
			toast.error('Could not connect', { description: (err as Error).message });
		} finally {
			busy = false;
		}
	}

	async function disconnect(t: ApiToken) {
		if (!confirm(`Disconnect ${t.name}? It can't reach your fleet from its next message. Tasks it started keep running.`)) return;
		try {
			await api(`/v1/api-tokens/${t.id}`, { method: 'DELETE' });
			if (fresh?.id === t.id) reset();
			toast(`Disconnected ${t.name}`);
			await load();
		} catch (err) {
			toast.error('Could not disconnect', { description: (err as Error).message });
		}
	}

	const step = (on: boolean) =>
		`flex size-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold ${on ? 'bg-foreground text-background' : 'bg-muted text-muted-foreground'}`;
	const tile = (on: boolean) =>
		`flex flex-col items-start gap-0.5 rounded-xl border px-3.5 py-3 text-left transition-colors ${on ? 'border-foreground bg-card ring-1 ring-foreground' : 'bg-card hover:border-foreground/40'}`;
</script>

<svelte:head><title>Chat apps · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 py-8 md:px-8">
	<header>
		<h1 class="text-[22px] font-semibold tracking-tight">Chat apps</h1>
		<p class="mt-1 text-[13px] text-muted-foreground">
			Run and check tasks from Telegram, WhatsApp, Discord or wherever you chat, through the AI assistant you already use. Tell it
			“fix the failing test in my web repo” and it starts the task on one of your machines, then tells you how it went.
		</p>
	</header>

	{#if tokens === null}
		<p class="text-[13px] text-muted-foreground">Loading…</p>
	{:else}
		{#if live.length}
			<section class="flex flex-col gap-2" aria-labelledby="connected-h">
				<div class="flex items-end justify-between gap-3">
					<h2 id="connected-h" class="text-[15px] font-semibold tracking-tight">Your chat apps</h2>
					{#if !showSetup}
						<Button variant="outline" class="h-8 rounded-full text-[13px]" onclick={() => (adding = true)}>Connect another</Button>
					{/if}
				</div>
				<ul class="divide-y rounded-xl border bg-card">
					{#each live as t (t.id)}
						<li class="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3 text-[13px]">
							<div class="min-w-0 flex-1">
								<p class="truncate font-medium">{t.name}</p>
								<p class="text-xs text-muted-foreground">{accessOf(t.scope).short}</p>
							</div>
							<span class="text-xs text-muted-foreground">
								{#if t.last_used_at}Last active {rel(t.last_used_at, fleet.now)}{:else}Not connected yet{/if}
							</span>
							<button type="button" class="text-xs text-err-ink hover:underline" onclick={() => disconnect(t)}>Disconnect</button>
							{#if hasUpdates(appOf(t.name)) && t.id !== fresh?.id}
								<div class="basis-full pt-1"><ChatUpdates token={t} app={appOf(t.name)} onchange={load} /></div>
							{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if showSetup}
			<section class="flex flex-col gap-6 rounded-2xl border bg-card/60 p-5 md:p-6" aria-labelledby="setup-h">
				<div class="flex items-start justify-between gap-3">
					<h2 id="setup-h" class="text-[15px] font-semibold tracking-tight">Connect a chat app</h2>
					{#if live.length && !fresh}
						<button type="button" class="text-xs text-muted-foreground hover:text-foreground" onclick={reset}>Cancel</button>
					{/if}
				</div>

				<!-- 1. Which app -->
				<div class="flex flex-col gap-3">
					<p class="flex items-center gap-3 text-[14px] font-medium"><span class={step(true)}>1</span>Which assistant do you chat with?</p>
					<div class="flex min-w-0 flex-col gap-3 sm:pl-9">
						<div class="grid grid-cols-2 gap-2 sm:grid-cols-4" role="radiogroup" aria-label="Assistant">
							{#each CHAT_APPS as a (a.id)}
								<button type="button" role="radio" aria-checked={app === a.id} disabled={!!fresh} class="{tile(app === a.id)} disabled:opacity-60" onclick={() => pick(a.id)}>
									<span class="text-[13px] font-medium">{a.name}</span>
									<span class="text-xs text-muted-foreground">{a.hint}</span>
								</button>
							{/each}
						</div>
					</div>
				</div>

				<!-- 2. What it may do -->
				<div class="flex flex-col gap-3 {app ? '' : 'opacity-50'}">
					<p class="flex items-center gap-3 text-[14px] font-medium"><span class={step(!!app)}>2</span>What can it do?</p>
					<div class="flex min-w-0 flex-col gap-3 sm:pl-9">
						{#if app}
							<div class="flex flex-col gap-2" role="radiogroup" aria-label="What it can do">
								{#each ACCESS as a (a.scope)}
									<button type="button" role="radio" aria-checked={scope === a.scope} disabled={!!fresh} class="{tile(scope === a.scope)} disabled:opacity-60" onclick={() => (scope = a.scope)}>
										<span class="flex items-center gap-2 text-[13px] font-medium">
											{a.name}
											{#if a.scope === 'tasks'}<span class="rounded-full bg-ok-soft px-2 py-0.5 text-[11px] font-normal text-ok-ink">Recommended</span>{/if}
										</span>
										<span class="text-xs text-muted-foreground">{a.hint}</span>
									</button>
								{/each}
							</div>
							{#if !fresh}
								<Button class="h-9 w-fit rounded-full text-[13px]" disabled={busy} onclick={connect}>
									{#if busy}<LoaderCircle class="size-4 animate-spin" />{/if}Connect {chosen?.id === 'other' ? 'it' : chosen?.name}
								</Button>
							{/if}
						{/if}
					</div>
				</div>

				<!-- 3. Add it to the app -->
				<div class="flex flex-col gap-3 {fresh ? '' : 'opacity-50'}">
					<p class="flex items-center gap-3 text-[14px] font-medium"><span class={step(!!fresh)}>3</span>Add Sidefleet to {chosen && chosen.id !== 'other' ? chosen.name : 'your app'}</p>
					<div class="flex min-w-0 flex-col gap-3 sm:pl-9">
						{#if fresh?.token && chosen && !connected}
							{#if chosen.canSelfSetup}
								<div class="flex w-fit rounded-full bg-muted p-0.5 text-[12.5px]" role="tablist">
									<button role="tab" aria-selected={how === 'ask'} class="rounded-full px-3 py-1 {how === 'ask' ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground'}" onclick={() => (how = 'ask')}>
										Ask {chosen.name} to do it
									</button>
									<button role="tab" aria-selected={how === 'self'} class="rounded-full px-3 py-1 {how === 'self' ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground'}" onclick={() => (how = 'self')}>
										Do it yourself
									</button>
								</div>
							{/if}
							{#if how === 'ask' && chosen.canSelfSetup}
								<p class="text-[13px] text-muted-foreground">Send {chosen.name} this message. It adds Sidefleet to its own settings.</p>
								<Command shown={setupMessage(chosen.id, fresh.token)} label="Copy message" />
							{:else}
								<p class="text-[13px] text-muted-foreground">{setupSteps(chosen.id).before}</p>
								<Command shown={setupSnippet(chosen.id, fresh.token)} />
								{#if setupSteps(chosen.id).after}<p class="text-[13px] text-muted-foreground">{setupSteps(chosen.id).after}</p>{/if}
							{/if}
							<p class="text-xs text-muted-foreground">
								This includes a private key that lets the app use your fleet as you. It's shown only now: if you lose it, connect again. You can
								disconnect the app here at any time.
							</p>
						{/if}
						{#if fresh && chosen}
							{#if connected}
								<div class="flex flex-col gap-2 rounded-xl border border-ok/40 bg-ok-soft/40 p-4">
									<p class="flex items-center gap-2 text-[13px] font-medium"><Check class="size-4 text-ok-ink" />{fresh.name} is connected.</p>
									<p class="text-[13px] text-muted-foreground">Try asking it:</p>
									<ul class="flex flex-wrap gap-1.5">
										{#each EXAMPLES as e (e)}<li class="rounded-full border bg-card px-3 py-1 text-[12.5px]">“{e}”</li>{/each}
									</ul>
									{#if !hasUpdates(chosen.id)}
										<button type="button" class="mt-1 w-fit text-xs text-muted-foreground hover:text-foreground" onclick={reset}>Done</button>
									{/if}
								</div>
							{:else}
								<div class="flex items-center justify-between gap-3 rounded-xl border border-dashed px-4 py-3">
									<p class="flex items-center gap-2 text-[13px] text-muted-foreground">
										<LoaderCircle class="size-4 animate-spin" />Waiting for {chosen.id === 'other' ? 'your app' : chosen.name} to connect…
									</p>
									<button type="button" class="text-xs text-muted-foreground hover:text-foreground" onclick={reset}>Finish later</button>
								</div>
							{/if}
						{/if}
					</div>
				</div>

				<!-- 4. Updates in chat (not for Claude Code: it has no chat to message) -->
				{#if chosen && hasUpdates(chosen.id) && current}
					<div class="flex flex-col gap-3 {connected ? '' : 'opacity-50'}">
						<p class="flex items-center gap-3 text-[14px] font-medium">
							<span class={step(connected)}>4</span>Let {chosen.id === 'other' ? 'it' : chosen.name} message you
							<span class="text-xs font-normal text-muted-foreground">Optional</span>
						</p>
						<div class="flex min-w-0 flex-col gap-3 sm:pl-9">
							{#if connected}
								<ChatUpdates token={current} app={chosen.id} onchange={load} open />
								<button type="button" class="w-fit text-xs text-muted-foreground hover:text-foreground" onclick={reset}>
									{current.updates ? 'Done' : 'Skip for now'}
								</button>
							{:else}
								<p class="text-[13px] text-muted-foreground">Once it's connected, it can also tell you when a task finishes or needs your OK.</p>
							{/if}
						</div>
					</div>
				{/if}
			</section>
		{/if}
	{/if}
</section>
