<script lang="ts">
	// Personal API tokens: let a script or a chat agent (Hermes, OpenClaw, an MCP
	// client) drive the fleet as you, in this org. The scope limits what it can do;
	// the value is shown once, right after it is created. Each person sees their own.
	import { Button } from '#lib/components/ui/button';
	import { Input } from '#lib/components/ui/input';
	import { toast } from 'svelte-sonner';
	import { api } from '../api';
	import { fleet } from '../state.svelte';
	import { rel } from '../format';
	import Command from './Command.svelte';
	import type { ApiScope, ApiToken } from '../types';

	const SCOPES: { id: ApiScope; label: string; hint: string }[] = [
		{ id: 'read', label: 'Read', hint: 'See tasks, their progress and machines.' },
		{ id: 'tasks', label: 'Tasks', hint: 'Also start tasks, reply, stop, fork and open PRs. Never with full access.' },
		{ id: 'full', label: 'Full', hint: 'Also approve tool calls and run tasks with full access. Only for agents you trust with your machines.' }
	];

	let tokens = $state<ApiToken[] | null>(null);
	let name = $state('');
	let scope = $state<ApiScope>('tasks');
	let busy = $state(false);
	let fresh = $state<ApiToken | null>(null);
	const org = $derived(fleet.org);

	async function load() {
		try {
			tokens = await api<ApiToken[]>('/v1/api-tokens');
		} catch {
			tokens = [];
		}
	}
	// The list is per org: reload when it changes.
	$effect(() => {
		if (org) {
			void org.id;
			fresh = null;
			void load();
		}
	});

	async function create(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			fresh = await api<ApiToken>('/v1/api-tokens', { method: 'POST', body: JSON.stringify({ name: name.trim(), scope }) });
			name = '';
			await load();
		} catch (err) {
			toast.error('Could not create the token', { description: (err as Error).message });
		} finally {
			busy = false;
		}
	}

	async function revoke(t: ApiToken) {
		if (!confirm(`Revoke "${t.name}"? Anything using it is refused from its next call.`)) return;
		try {
			await api(`/v1/api-tokens/${t.id}`, { method: 'DELETE' });
			if (fresh?.id === t.id) fresh = null;
			toast(`Revoked ${t.name}`);
			await load();
		} catch (err) {
			toast.error('Could not revoke', { description: (err as Error).message });
		}
	}
	const live = $derived((tokens ?? []).filter((t) => !t.revoked_at));
	const revoked = $derived((tokens ?? []).filter((t) => t.revoked_at));
	const hint = $derived(SCOPES.find((s) => s.id === scope)?.hint);
</script>

{#if org}
	<section class="flex flex-col gap-3" aria-labelledby="api-tokens-h">
		<div class="flex flex-wrap items-end justify-between gap-3">
			<div>
				<h2 id="api-tokens-h" class="text-[15px] font-semibold tracking-tight">API tokens</h2>
				<p class="mt-0.5 text-[13px] text-muted-foreground">
					For a script or a chat agent such as Hermes or OpenClaw: it acts as you in {org.name}. It can't manage machines, members or tokens.
				</p>
			</div>
			<form class="flex flex-wrap gap-2" onsubmit={create}>
				<Input bind:value={name} placeholder="Token name, e.g. Hermes" maxlength={80} class="h-8 w-48 text-[13px]" aria-label="Token name" />
				<div class="flex rounded-lg bg-muted p-0.5" role="radiogroup" aria-label="Scope">
					{#each SCOPES as s (s.id)}
						<button
							type="button"
							role="radio"
							aria-checked={scope === s.id}
							title={s.hint}
							class="flex h-7 items-center rounded-md px-2.5 text-[12.5px] transition-colors {scope === s.id ? 'bg-card font-medium text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}"
							onclick={() => (scope = s.id)}
						>
							{s.label}
						</button>
					{/each}
				</div>
				<Button type="submit" variant="outline" class="h-8 rounded-full text-[13px]" disabled={busy || !name.trim()}>Create token</Button>
			</form>
		</div>
		<p class="-mt-1 text-right text-xs text-muted-foreground">{hint}</p>

		{#if fresh?.token}
			<div class="flex flex-col gap-2 rounded-xl border border-ok/40 bg-ok-soft/40 p-4">
				<p class="text-[13px]"><span class="font-medium">{fresh.name}</span> created. Copy it now: it is not shown again.</p>
				<Command shown={fresh.token} />
				<p class="text-xs text-muted-foreground">
					Send it as <code class="font-mono">Authorization: Bearer …</code> to {location.origin}/v1/…, as described in the API docs.
				</p>
				<button type="button" class="w-fit text-xs text-muted-foreground hover:text-foreground" onclick={() => (fresh = null)}>Done, hide it</button>
			</div>
		{/if}

		{#if tokens === null}
			<p class="text-[13px] text-muted-foreground">Loading…</p>
		{:else if !tokens.length}
			<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">No API tokens yet.</p>
		{:else}
			<ul class="divide-y rounded-xl border bg-card">
				{#each [...live, ...revoked] as t (t.id)}
					<li class="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5 text-[13px] {t.revoked_at ? 'opacity-60' : ''}">
						<span class="min-w-0 flex-1 truncate font-medium">{t.name}</span>
						<span class="rounded-full bg-muted px-2 py-0.5 text-xs capitalize {t.scope === 'full' ? 'text-err-ink' : 'text-muted-foreground'}">{t.scope}</span>
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
