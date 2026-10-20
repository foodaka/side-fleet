<script lang="ts">
	// How to put a machine on the fleet. Used inline for onboarding and as a dialog on Machines.
	import Command from './Command.svelte';
	import { agentPrompt, connectCmd, installCmd } from '../state.svelte';
	let { compact = false }: { compact?: boolean } = $props();
	let mode = $state<'terminal' | 'agent'>('terminal');
	const origin = location.origin;
	const local = /\/\/(localhost|127\.0\.0\.1|\[::1\])/.test(origin);
</script>

<div class="flex flex-col gap-4">
	<div class="inline-flex w-fit rounded-full bg-muted p-0.5 text-[13px]" role="tablist" aria-label="How to set it up">
		<button role="tab" aria-selected={mode === 'terminal'} class="rounded-full px-3 py-1 {mode === 'terminal' ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground'}" onclick={() => (mode = 'terminal')}>In a terminal</button>
		<button role="tab" aria-selected={mode === 'agent'} class="rounded-full px-3 py-1 {mode === 'agent' ? 'bg-background font-medium shadow-sm' : 'text-muted-foreground'}" onclick={() => (mode = 'agent')}>Ask Claude Code</button>
	</div>

	{#if mode === 'terminal'}
		<ol class="flex flex-col gap-4">
			<li class="flex flex-col gap-2">
				<span class="text-[13px]"><span class="tnum mr-2 text-muted-foreground">1</span>Install Sidefleet on that machine. No sudo; one binary in <code class="font-mono text-[12px]">~/.local/bin</code>.</span>
				<Command shown={installCmd} />
			</li>
			<li class="flex flex-col gap-2">
				<span class="text-[13px]"><span class="tnum mr-2 text-muted-foreground">2</span>Connect it. It checks Claude Code, finds your git checkouts, and stays connected after a reboot.</span>
				<Command shown={connectCmd(true)} copy={connectCmd(false)} />
			</li>
		</ol>
	{:else}
		<div class="flex flex-col gap-2">
			<span class="text-[13px]">Paste this into Claude Code on that machine. It reads the setup steps from this control plane and does the install and connect for you.</span>
			<Command shown={agentPrompt(true)} copy={agentPrompt(false)} label="Copy prompt" wrap />
		</div>
	{/if}

	{#if local && !compact}
		<p class="text-xs leading-relaxed text-muted-foreground">
			This page is open at <code class="font-mono">{origin}</code>, which other machines can't reach. Use this machine's Tailscale or LAN address in the connect command instead.
		</p>
	{/if}
</div>
