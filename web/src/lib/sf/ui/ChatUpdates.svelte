<script lang="ts">
	// Updates in chat for one connected app: Sidefleet tells the app when a task
	// finishes, fails, needs an OK or gets a pull request, and the app messages you
	// about it, so you can answer right there. Off: an offer to set it up. Setting
	// up: where the app listens, then what to add on its side, then a test. On:
	// how the last update went, with Send a test, Change and Turn off.
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import Check from '@lucide/svelte/icons/check';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { toast } from 'svelte-sonner';
	import { api } from '../api';
	import { fleet } from '../state.svelte';
	import { rel } from '../format';
	import Command from './Command.svelte';
	import {
		CHAT_APPS,
		PLATFORMS,
		updatesAddress,
		updatesKind,
		updatesMessage,
		updatesPlaceholder,
		updatesSetup,
		updatesUrl,
		type ChatAppId
	} from '../chat';
	import type { ApiToken } from '../types';

	let { token, app, onchange, open = false }: { token: ApiToken; app: ChatAppId; onchange: () => void | Promise<void>; open?: boolean } = $props();

	const name = $derived(app === 'other' ? 'your app' : (CHAT_APPS.find((a) => a.id === app)?.name ?? 'your app'));
	const Name = $derived(name.charAt(0).toUpperCase() + name.slice(1));
	const on = $derived(token.updates);

	/** `form`: where it listens; `setup`: what to add on the app's side, then a test. */
	let stage = $state<'idle' | 'form' | 'setup'>('idle');
	// Opened for you once, right after the app connects; Cancel closes it for good.
	let opened = false;
	$effect(() => {
		if (open && !on && !opened) {
			opened = true;
			stage = 'form';
		}
	});
	let address = $state('');
	let platform = $state('telegram');
	let allTasks = $state(false);
	let secret = $state('');
	let how = $state<'ask' | 'self'>('ask');
	let busy = $state(false);
	let test = $state<{ ok: boolean; text: string } | null>(null);

	function edit() {
		address = on ? updatesAddress(app, on.url) : '';
		allTasks = on?.all_tasks ?? false;
		test = null;
		stage = 'form';
	}

	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const r = await api<ApiToken & { secret: string }>(`/v1/api-tokens/${token.id}/updates`, {
				method: 'PUT',
				body: JSON.stringify({ kind: updatesKind(app), url: updatesUrl(app, address), all_tasks: allTasks })
			});
			secret = r.secret;
			how = app === 'other' ? 'self' : 'ask';
			stage = 'setup';
			await onchange();
		} catch (err) {
			toast.error('Could not turn updates on', { description: (err as Error).message });
		} finally {
			busy = false;
		}
	}

	async function sendTest() {
		busy = true;
		test = null;
		try {
			await api(`/v1/api-tokens/${token.id}/updates/test`, { method: 'POST' });
			test = { ok: true, text: app === 'other' ? 'Sent: your app got it.' : `Sent. ${Name} should message you in a moment.` };
		} catch (err) {
			test = { ok: false, text: (err as Error).message };
		} finally {
			busy = false;
			await onchange();
		}
	}

	async function turnOff() {
		if (!confirm(`Stop updates in chat? ${Name} won't hear about tasks from Sidefleet any more. It can still run and check them.`)) return;
		try {
			await api(`/v1/api-tokens/${token.id}/updates`, { method: 'DELETE' });
			stage = 'idle';
			await onchange();
		} catch (err) {
			toast.error('Could not turn updates off', { description: (err as Error).message });
		}
	}

	const setup = $derived(updatesSetup(app, secret, platform));
	const local = $derived(!fleet.methods?.hosted);
</script>

{#if stage === 'idle'}
	{#if on}
		<div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
			{#if on.last_error}
				<span class="text-err-ink">Updates in chat: the last one didn't arrive. {on.last_error}</span>
			{:else}
				<span class="flex items-center gap-1 text-muted-foreground"><Check class="size-3.5 text-ok-ink" />Updates in chat are on{on.last_at ? `, last sent ${rel(on.last_at, fleet.now)}` : ''}</span>
			{/if}
			<button type="button" class="text-muted-foreground hover:text-foreground disabled:opacity-50" disabled={busy} onclick={sendTest}>Send a test</button>
			<button type="button" class="text-muted-foreground hover:text-foreground" onclick={edit}>Change</button>
			<button type="button" class="text-muted-foreground hover:text-foreground" onclick={turnOff}>Turn off</button>
			{#if test}<span class={test.ok ? 'text-ok-ink' : 'text-err-ink'}>{test.text}</span>{/if}
		</div>
	{:else}
		<button type="button" class="w-fit text-xs text-muted-foreground hover:text-foreground" onclick={edit}>
			Get updates in chat: {name} tells you when a task finishes or needs your OK →
		</button>
	{/if}
{:else}
	<div class="flex flex-col gap-3 rounded-xl border bg-card p-4">
		<div>
			<p class="text-[13px] font-medium">Get updates in chat</p>
			<p class="text-xs text-muted-foreground">
				{Name} messages you when a task finishes, fails or needs your OK to run a command, and you answer right there: “yes, open a PR”.
			</p>
		</div>

		{#if stage === 'form'}
			<form class="flex flex-col gap-3" onsubmit={save}>
				<label class="flex flex-col gap-1.5">
					<span class="text-[13px]">{app === 'other' ? 'Address to send updates to' : `Address of the computer running ${Name}`}</span>
					<Input bind:value={address} required placeholder={updatesPlaceholder[app]} class="h-9 text-[13px]" inputmode="url" autocomplete="off" />
					<span class="text-xs text-muted-foreground">
						{#if app === 'hermes'}Hermes listens for updates on port 8644.{:else if app === 'openclaw'}OpenClaw listens on its gateway's port, 18789 unless you changed it.{/if}
						{#if local}
							Sidefleet needs to reach it: a local or Tailscale address is fine if both are on the same network.
						{:else}
							Sidefleet needs to reach it from the internet. If it runs at home, a tunnel such as Tailscale Funnel or Cloudflare Tunnel gives it an address.
						{/if}
					</span>
				</label>
				{#if app === 'hermes'}
					<div class="flex flex-col gap-1.5">
						<span class="text-[13px]">Where do you chat with Hermes?</span>
						<div class="flex flex-wrap gap-1.5" role="radiogroup" aria-label="Where you chat">
							{#each PLATFORMS as p (p.id)}
								<button
									type="button"
									role="radio"
									aria-checked={platform === p.id}
									class="rounded-full border px-3 py-1 text-[12.5px] {platform === p.id ? 'border-foreground bg-foreground text-background' : 'hover:border-foreground/40'}"
									onclick={() => (platform = p.id)}>{p.name}</button
								>
							{/each}
						</div>
					</div>
				{/if}
				<label class="flex items-start gap-2 text-[13px]">
					<input type="checkbox" bind:checked={allTasks} class="mt-0.5" />
					<span>Every task, not only the ones {name} started or replied to</span>
				</label>
				<div class="flex gap-2">
					<Button type="submit" class="h-8 rounded-full text-[13px]" disabled={busy || !address.trim()}>
						{#if busy}<LoaderCircle class="size-4 animate-spin" />{/if}Next
					</Button>
					<Button type="button" variant="ghost" class="h-8 rounded-full text-[13px]" onclick={() => (stage = 'idle')}>Cancel</Button>
				</div>
			</form>
		{:else}
			{#if app !== 'other'}
				<div class="flex w-fit rounded-full bg-muted p-0.5 text-[12.5px]" role="tablist">
					<button role="tab" aria-selected={how === 'ask'} class="rounded-full px-3 py-1 {how === 'ask' ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground'}" onclick={() => (how = 'ask')}>
						Ask {Name} to do it
					</button>
					<button role="tab" aria-selected={how === 'self'} class="rounded-full px-3 py-1 {how === 'self' ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground'}" onclick={() => (how = 'self')}>
						Do it yourself
					</button>
				</div>
			{/if}
			{#if how === 'ask' && app !== 'other'}
				<p class="text-[13px] text-muted-foreground">Send {Name} this message:</p>
				<Command shown={updatesMessage(app, secret, platform)} label="Copy message" />
			{:else}
				<p class="text-[13px] text-muted-foreground">{setup.before}</p>
				<Command shown={setup.snippet} />
				{#if setup.after}<p class="text-[13px] text-muted-foreground">{setup.after}</p>{/if}
			{/if}
			<div class="flex flex-wrap items-center gap-3">
				<Button class="h-8 rounded-full text-[13px]" disabled={busy} onclick={sendTest}>
					{#if busy}<LoaderCircle class="size-4 animate-spin" />{/if}Send a test update
				</Button>
				<!-- In the setup steps, the step's own Done finishes. -->
				{#if !open}
					<button type="button" class="text-xs text-muted-foreground hover:text-foreground" onclick={() => (stage = 'idle')}>{test?.ok ? 'Done' : 'Finish later'}</button>
				{/if}
			</div>
			{#if test}
				<p class="text-[13px] {test.ok ? 'text-ok-ink' : 'text-err-ink'}">{test.text}</p>
			{/if}
		{/if}
	</div>
{/if}
