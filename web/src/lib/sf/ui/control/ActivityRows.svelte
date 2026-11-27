<script lang="ts">
	// Rows of the activity feed: time, agent and owner, event, tool and action, outcome, policy.
	import { clock, eventType, outcome, type ActivityEvent } from '../../control';
	import { fleet } from '../../state.svelte';
	import Chip from './Chip.svelte';

	let { events, head = true, owners = true, empty = 'No instrumented activity yet.' }: { events: ActivityEvent[]; head?: boolean; owners?: boolean; empty?: string } = $props();
	const cols = 'grid grid-cols-[72px_minmax(150px,1fr)_minmax(120px,.8fr)_minmax(240px,2.2fr)_minmax(140px,.9fr)_minmax(150px,1fr)] items-center gap-3.5 min-w-[960px]';
</script>

<div class="overflow-x-auto">
	{#if head}
		<div class="{cols} px-4 py-2.5 text-[11.5px] font-medium text-muted-foreground"><span>Time</span><span>Agent{owners ? ' · owner' : ''}</span><span>Event</span><span>Tool · action</span><span>Outcome</span><span>Policy</span></div>
	{/if}
	{#each events as e (e.id)}
		{@const o = outcome(e.outcome)}
		<div class="{cols} border-t px-4 py-2.5 text-[13px]">
			<span class="tnum font-mono text-[12px] text-muted-foreground" title={new Date(e.at).toLocaleString()}>{clock(e.at, fleet.now)}</span>
			<div class="min-w-0">
				{#if e.agent}<a class="block truncate font-medium hover:underline" href="#/control/agents/{e.agent.id}">{e.agent.name}</a>{:else}<span class="text-muted-foreground">Unknown agent</span>{/if}
				{#if owners}<div class="truncate text-[12px] {e.owner ? 'text-muted-foreground' : 'text-queue-ink'}">{e.owner ? e.owner.login : 'Unassigned'}{e.team ? ` · ${e.team}` : ''}</div>{/if}
			</div>
			<span class="text-muted-foreground">{eventType(e.type)}</span>
			<div class="min-w-0 truncate font-mono text-[12px]" title={e.action}><span class="text-muted-foreground">{e.tool}</span> {e.action}</div>
			<span><Chip tone={o.tone}>{o.label}</Chip></span>
			<span class="truncate text-[12px] text-muted-foreground">{e.policy?.name ?? 'None'}</span>
		</div>
	{/each}
	{#if !events.length}<div class="border-t px-5 py-5 text-[13px] text-muted-foreground">{empty}</div>{/if}
</div>
