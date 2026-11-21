<script lang="ts">
	// Audit entries, newest first: when, who, what, on which task or machine, and the
	// redacted detail behind a toggle. Used by Team → Activity log and a thread's Activity.
	import Server from '@lucide/svelte/icons/server';
	import Cog from '@lucide/svelte/icons/cog';
	import KeyRound from '@lucide/svelte/icons/key-round';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import { ago } from '../format';
	import { actorOf, describe, stamp } from '../audit';
	import type { AuditEvent } from '../types';
	import Avatar from './Avatar.svelte';

	let { events, now, showRun = true }: { events: AuditEvent[]; now: number; showRun?: boolean } = $props();

	let open = $state<Record<number, boolean>>({});
	const shown = (e: AuditEvent) => Object.entries(e.detail).filter(([, v]) => v !== null && v !== undefined && v !== '');
</script>

<ul class="divide-y rounded-xl border bg-card">
	{#each events as e (e.id)}
		{@const who = actorOf(e)}
		{@const details = shown(e)}
		<li class="flex flex-col gap-1 px-4 py-2.5 text-[13px]">
			<div class="flex items-start gap-3">
				<span class="mt-0.5 grid size-6 shrink-0 place-items-center">
					{#if who.kind === 'person'}
						<Avatar login={who.login} size={22} />
					{:else}
						<span class="grid size-[22px] place-items-center rounded-full bg-muted text-muted-foreground" title={who.label}>
							{#if who.kind === 'machine'}<Server class="size-3" />{:else if who.kind === 'key'}<KeyRound class="size-3" />{:else}<Cog class="size-3" />{/if}
						</span>
					{/if}
				</span>
				<p class="min-w-0 flex-1 leading-relaxed">
					<span class="font-medium" class:font-mono={who.kind === 'machine'}>{who.label}</span>
					<span class="text-foreground/85">{describe(e)}</span>
					{#if showRun && e.run_id}
						<a href="#/runs/{e.run_id}" class="ml-1 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline">Open task</a>
					{/if}
					{#if e.worker_id && who.kind !== 'machine' && !e.action.startsWith('machine.')}
						<span class="ml-1 font-mono text-[11.5px] text-muted-foreground">on {e.worker_id}</span>
					{/if}
				</p>
				<time datetime={e.at} title={stamp(e.at)} class="tnum shrink-0 text-xs text-muted-foreground">{ago(e.at, now)}</time>
			</div>
			<div class="flex flex-wrap items-center gap-x-2 pl-9 text-[11.5px] text-muted-foreground">
				<code class="font-mono">{e.action}</code>
				<span aria-hidden="true">·</span>
				<span class="tnum">{stamp(e.at)}</span>
				{#if details.length}
					<button
						type="button"
						class="inline-flex items-center gap-0.5 rounded px-1 hover:bg-muted hover:text-foreground"
						aria-expanded={!!open[e.id]}
						onclick={() => (open[e.id] = !open[e.id])}
					><ChevronRight class="size-3 transition-transform {open[e.id] ? 'rotate-90' : ''}" />Details</button>
				{/if}
			</div>
			{#if open[e.id]}
				<dl class="ml-9 grid grid-cols-[max-content_1fr] gap-x-3 gap-y-0.5 rounded-md bg-muted/60 px-3 py-2 font-mono text-[11.5px]">
					{#if e.target}<dt class="text-muted-foreground">target</dt><dd class="break-all">{e.target}</dd>{/if}
					{#each details as [k, v] (k)}
						<dt class="text-muted-foreground">{k}</dt>
						<dd class="break-all">{typeof v === 'string' ? v : JSON.stringify(v)}</dd>
					{/each}
				</dl>
			{/if}
		</li>
	{/each}
</ul>
