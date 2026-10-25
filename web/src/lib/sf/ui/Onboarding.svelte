<script lang="ts">
	// First run: the steps from signing in to the first task, ticked off as the fleet reports them.
	import Check from '@lucide/svelte/icons/check';
	import { fleet } from '../state.svelte';
	import AddMachine from './AddMachine.svelte';
	import PendingMachines from './PendingMachines.svelte';
	import StatusDot from './StatusDot.svelte';

	type Step = { title: string; detail: string; state: 'done' | 'current' | 'next' };

	// Without a static worker token a machine asks to join, and someone here lets it in.
	const approval = $derived(!fleet.setup?.worker_token);
	const login = $derived(fleet.me?.user?.login);
	const asked = $derived(fleet.pending.length > 0);
	const steps = $derived<Step[]>([
		{
			title: login ? 'Signed in with GitHub' : 'Signed in',
			detail: login ? `As @${login}. Sidefleet only reads your name and email, not your repos.` : 'This page is your fleet.',
			state: 'done'
		},
		{
			title: 'Connect a machine',
			detail: 'A laptop, a Mac mini or a VPS with Claude Code, Codex or opencode set up. It connects out to this page; nothing connects in.',
			state: asked ? 'done' : 'current'
		},
		...(approval
			? [{
				title: 'Approve it here',
				detail: 'The connect command prints a link and a code. Open the link, or enter the code below, so only machines you approve join.',
				state: asked ? 'current' : 'next'
			} satisfies Step]
			: []),
		{
			title: 'Run your first task',
			detail: 'Describe it in plain words. It runs in its own worktree on that machine and streams back here.',
			state: 'next'
		}
	]);
</script>

<ol class="flex flex-col">
	{#each steps as s, i (s.title)}
		{@const last = i === steps.length - 1}
		<li class="relative flex gap-4 {last ? '' : 'pb-7'}">
			{#if !last}<span class="absolute top-8 bottom-1 left-[13px] w-px {s.state === 'done' ? 'bg-ok/50' : 'bg-border'}" aria-hidden="true"></span>{/if}
			<span
				class="tnum relative grid size-[27px] shrink-0 place-items-center rounded-full border text-[12px] font-medium
					{s.state === 'done' ? 'border-ok bg-ok text-white' : s.state === 'current' ? 'border-foreground bg-foreground text-background' : 'bg-card text-muted-foreground'}"
				aria-hidden="true"
			>
				{#if s.state === 'done'}<Check class="size-3.5" strokeWidth={3} />{:else}{i + 1}{/if}
			</span>
			<div class="flex min-w-0 flex-1 flex-col gap-3 pt-0.5">
				<div>
					<h2 class="text-[15px] font-medium {s.state === 'next' ? 'text-muted-foreground' : ''}">
						<span class="sr-only">Step {i + 1}{s.state === 'done' ? ', done' : s.state === 'current' ? ', now' : ''}: </span>{s.title}
					</h2>
					<p class="mt-0.5 text-[13px] leading-relaxed text-muted-foreground">{s.detail}</p>
				</div>
				{#if s.title === 'Connect a machine' && !asked}
					<div class="rounded-2xl border bg-card p-5 shadow-[0_1px_2px_rgb(0_0_0/0.03)]">
						<AddMachine />
					</div>
				{:else if s.title === 'Approve it here'}
					<PendingMachines bare />
				{:else if s.title === 'Run your first task' && !asked}
					<p class="flex items-center gap-2 text-[13px] text-muted-foreground" role="status">
						<StatusDot status="queued" class="pulse-run" />Waiting for a machine to check in. This page updates by itself.
					</p>
				{/if}
			</div>
		</li>
	{/each}
</ol>
