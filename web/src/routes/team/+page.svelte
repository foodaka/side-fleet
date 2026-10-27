<script lang="ts">
	// Who is in this org, and inviting more people. Teammates see each other's tasks;
	// machines only ever run their owner's.
	import { Button } from '#lib/components/ui/button';
	import * as Select from '#lib/components/ui/select';
	import { untrack } from 'svelte';
	import { toast } from 'svelte-sonner';
	import UserPlus from '@lucide/svelte/icons/user-plus';
	import Link from '@lucide/svelte/icons/link';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { until } from '#lib/sf/format';
	import type { Invite } from '#lib/sf/types';
	import Avatar from '#lib/sf/ui/Avatar.svelte';
	import CopyButton from '#lib/sf/ui/CopyButton.svelte';

	const org = $derived(fleet.org);
	const me = $derived(fleet.me?.user ?? null);
	/** Your personal org: the one named after you, where you are the only one. */
	const personal = $derived(!!org && !!me && org.slug === me.login.toLowerCase() && fleet.members.length <= 1);

	let invites = $state<Invite[] | null>(null);
	let fresh = $state<string | null>(null);
	let creating = $state(false);
	let expires = $state('7');
	let uses = $state('many');

	const link = (code: string) => `${location.origin}${location.pathname}#/join/${code}`;

	async function loadInvites(id: string) {
		try {
			invites = await api<Invite[]>(`/v1/orgs/${id}/invites`);
		} catch {
			invites = [];
		}
	}
	// Keyed on the org id alone: a background refresh hands back a new org object and
	// must not wipe a freshly made invite link.
	const orgId = $derived(org?.id ?? null);
	$effect(() => {
		const id = orgId;
		untrack(() => {
			invites = null;
			fresh = null;
			if (id) void fleet.reloadMembers();
		});
	});
	// Ownership can settle after the org loads; that only loads the list, never clears the link.
	$effect(() => {
		const id = orgId;
		if (id && fleet.isOwner) untrack(() => void loadInvites(id));
	});

	async function createInvite() {
		if (!org) return;
		creating = true;
		try {
			const r = await api<{ invite: Invite; code: string }>(`/v1/orgs/${org.id}/invites`, {
				method: 'POST',
				body: JSON.stringify({ role: 'member', expires_in_days: Number(expires), max_uses: uses === 'one' ? 1 : null })
			});
			fresh = r.code;
			await loadInvites(org.id);
		} catch (err) {
			toast.error('Could not create the invite', { description: (err as Error).message });
		}
		creating = false;
	}

	async function revoke(inv: Invite) {
		if (!org || !confirm('Revoke this invite link? Anyone who has it can no longer join with it.')) return;
		try {
			await api(`/v1/orgs/${org.id}/invites/${inv.id}`, { method: 'DELETE' });
			fresh = null;
			await loadInvites(org.id);
		} catch (err) {
			toast.error('Could not revoke', { description: (err as Error).message });
		}
	}

	async function remove(id: string, login: string) {
		if (!org) return;
		const self = id === me?.id;
		const q = self
			? `Leave ${org.name}? Your machines in it disconnect and your queued tasks there stop.`
			: `Remove @${login} from ${org.name}? Their machines in it disconnect and their queued tasks stop. Their threads stay.`;
		if (!confirm(q)) return;
		try {
			await api(`/v1/orgs/${org.id}/members/${id}`, { method: 'DELETE' });
			if (self) {
				toast(`You left ${org.name}`);
				fleet.switchOrg('');
				location.hash = '#/';
				return;
			}
			toast(`Removed @${login}`);
			await fleet.reloadMembers();
		} catch (err) {
			toast.error(self ? 'Could not leave' : 'Could not remove', { description: (err as Error).message });
		}
	}

	const pending = $derived((invites ?? []).filter((i) => !i.revoked_at && Date.parse(i.expires_at) > fleet.now && (i.max_uses == null || i.uses < i.max_uses)));
	const machinesOf = (id: string) => fleet.workers.filter((w) => w.owner?.id === id).length;
</script>

<svelte:head><title>Team · Sidefleet</title></svelte:head>

<section class="mx-auto flex w-full max-w-3xl flex-col gap-8 px-4 py-8 md:px-8">
	<header class="flex flex-col gap-1.5">
		<h1 class="text-[22px] font-semibold tracking-tight">{org?.name ?? 'Team'}</h1>
		<p class="text-[13px] leading-relaxed text-muted-foreground">
			Everyone here sees the team's tasks, rooms and changes. Machines only ever run their owner's tasks, so to pick up someone's work you continue it on your own machine.
		</p>
	</header>

	{#if !me}
		<p class="rounded-xl border border-dashed p-4 text-[13px] text-muted-foreground">Teams need GitHub sign-in. With the developer key, everything runs in the single default org.</p>
	{:else}
		{#if fleet.isOwner}
			<section class="flex flex-col gap-3 rounded-xl border bg-card p-5" aria-labelledby="invite-h">
				<div class="flex flex-wrap items-start justify-between gap-3">
					<div>
						<h2 id="invite-h" class="flex items-center gap-2 text-[15px] font-semibold tracking-tight"><UserPlus class="size-4" />Invite people</h2>
						<p class="mt-0.5 text-[13px] text-muted-foreground">
							{#if personal}This is your personal org. Invite people here, or create a team org from the org menu first.{:else}Send a link. They sign in with GitHub and land in {org?.name}.{/if}
						</p>
					</div>
				</div>
				<div class="flex flex-wrap items-center gap-2">
					<Select.Root type="single" bind:value={expires}>
						<Select.Trigger size="sm" class="h-8 rounded-full text-[13px]" aria-label="Expires">Expires in {expires === '1' ? '1 day' : `${expires} days`}</Select.Trigger>
						<Select.Content>
							<Select.Item value="1" label="1 day">1 day</Select.Item>
							<Select.Item value="7" label="7 days">7 days</Select.Item>
							<Select.Item value="30" label="30 days">30 days</Select.Item>
						</Select.Content>
					</Select.Root>
					<Select.Root type="single" bind:value={uses}>
						<Select.Trigger size="sm" class="h-8 rounded-full text-[13px]" aria-label="Uses">{uses === 'one' ? 'One person' : 'Anyone with the link'}</Select.Trigger>
						<Select.Content>
							<Select.Item value="one" label="One person">One person</Select.Item>
							<Select.Item value="many" label="Anyone with the link">Anyone with the link</Select.Item>
						</Select.Content>
					</Select.Root>
					<Button class="h-8 rounded-full text-[13px]" onclick={createInvite} disabled={creating}><Link class="size-3.5" />{creating ? 'Creating…' : 'Create invite link'}</Button>
				</div>
				{#if fresh}
					{@const url = link(fresh)}
					<div class="flex flex-col gap-2 rounded-lg border border-ok/40 bg-ok-soft/40 p-3">
						<p class="text-[13px]">Send this link. It is shown once; make another any time.</p>
						<div class="flex items-center gap-2 rounded-md bg-background px-2.5 py-1.5">
							<code class="min-w-0 flex-1 truncate font-mono text-[12px]">{url}</code>
							<CopyButton text={() => url} class="bg-primary text-primary-foreground hover:bg-primary/90" />
						</div>
					</div>
				{/if}
				{#if pending.length}
					<ul class="flex flex-col divide-y text-[13px]">
						{#each pending as inv (inv.id)}
							<li class="flex flex-wrap items-center gap-x-3 gap-y-1 py-2">
								<span class="flex-1 text-muted-foreground">
									{inv.max_uses === 1 ? 'One-person link' : 'Open link'}{inv.created_by_login ? ` by @${inv.created_by_login}` : ''}, expires {until(inv.expires_at, fleet.now)}{inv.uses ? `, used ${inv.uses}×` : ''}
								</span>
								<button type="button" class="text-xs text-err-ink hover:underline" onclick={() => revoke(inv)}>Revoke</button>
							</li>
						{/each}
					</ul>
				{/if}
			</section>
		{/if}

		<section class="flex flex-col gap-3" aria-labelledby="members-h">
			<h2 id="members-h" class="text-[15px] font-semibold tracking-tight">Members <span class="tnum font-normal text-muted-foreground">{fleet.members.length || ''}</span></h2>
			{#if !fleet.members.length}
				<p class="text-[13px] text-muted-foreground">Loading…</p>
			{:else}
				<ul class="divide-y rounded-xl border bg-card">
					{#each fleet.members as m (m.id)}
						{@const n = machinesOf(m.id)}
						<li class="flex items-center gap-3 px-4 py-3 text-[13px]">
							<Avatar login={m.login} size={28} />
							<span class="flex min-w-0 flex-1 flex-col">
								<span class="truncate font-medium">@{m.login}{#if m.id === me.id}<span class="ml-1.5 font-normal text-muted-foreground">(you)</span>{/if}</span>
								<span class="text-xs text-muted-foreground">{m.role === 'owner' ? 'Owner' : 'Member'}{n ? ` · ${n} machine${n === 1 ? '' : 's'}` : ''}</span>
							</span>
							{#if m.id === me.id}
								{#if !personal && !(m.role === 'owner' && fleet.members.filter((x) => x.role === 'owner').length === 1)}<button type="button" class="text-xs text-muted-foreground hover:text-err-ink" onclick={() => remove(m.id, m.login)}>Leave</button>{/if}
							{:else if fleet.isOwner}
								<button type="button" class="text-xs text-muted-foreground hover:text-err-ink" onclick={() => remove(m.id, m.login)}>Remove</button>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	{/if}
</section>
