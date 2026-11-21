<script lang="ts">
	// A thread's audit entries: who started, stopped, approved and ran each of its
	// messages, and on which machine. Same log and rules as Team → Activity log.
	import * as Dialog from '#lib/components/ui/dialog';
	import { Button } from '#lib/components/ui/button';
	import { untrack } from 'svelte';
	import { api } from '../api';
	import { auditQuery } from '../audit';
	import { fleet } from '../state.svelte';
	import type { AuditEvent, AuditPage } from '../types';
	import AuditList from '../ui/AuditList.svelte';

	let { open = $bindable(false), thread }: { open?: boolean; thread: string } = $props();

	let events = $state<AuditEvent[] | null>(null);
	let next = $state<number | null>(null);
	let error = $state('');
	let loading = $state(false);

	async function load(more: boolean) {
		const org = fleet.org?.id;
		if (!org) return;
		loading = true;
		try {
			const page = await api<AuditPage>(`/v1/orgs/${org}/audit${auditQuery({ thread }, { limit: 100, before: more ? next : null })}`);
			events = more ? [...(events ?? []), ...page.events] : page.events;
			next = page.next;
			error = '';
		} catch (err) {
			error = (err as Error).message;
			if (!more) events = [];
		}
		loading = false;
	}

	// Fresh each time it opens: a running thread keeps adding to it.
	$effect(() => {
		if (open)
			untrack(() => {
				events = null;
				void load(false);
			});
	});
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="max-h-[85dvh] overflow-y-auto sm:max-w-2xl">
		<Dialog.Header>
			<Dialog.Title>Activity</Dialog.Title>
			<Dialog.Description>
				{fleet.isOwner ? 'Who did what in this thread, newest first.' : 'What you did in this thread and what happened on your machines. Owners of the org see all of it.'}
				{' '}
				<a href="#/team/activity" class="underline underline-offset-2 hover:text-foreground" onclick={() => (open = false)}>Open the activity log</a>
			</Dialog.Description>
		</Dialog.Header>
		{#if error}
			<p class="text-[13px] text-err-ink">Could not load it: {error}</p>
		{:else if events === null}
			<p class="text-[13px] text-muted-foreground">Loading…</p>
		{:else if !events.length}
			<p class="text-[13px] text-muted-foreground">Nothing you can see here yet.</p>
		{:else}
			<AuditList {events} now={fleet.now} showRun={false} />
			{#if next !== null}
				<Button variant="outline" size="sm" class="h-8 w-fit self-center rounded-full text-[13px]" disabled={loading} onclick={() => load(true)}>{loading ? 'Loading…' : 'Older'}</Button>
			{/if}
		{/if}
	</Dialog.Content>
</Dialog.Root>
