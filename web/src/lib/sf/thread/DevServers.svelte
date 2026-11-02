<script lang="ts">
	// Servers this thread's agent left running on its machine, with Stop. Hidden when there are none.
	import { toast } from 'svelte-sonner';
	import Radio from '@lucide/svelte/icons/radio';
	import Square from '@lucide/svelte/icons/square';
	import { api } from '#lib/sf/api';
	import { fleet } from '#lib/sf/state.svelte';
	import { ago } from '#lib/sf/format';
	import type { DevServer } from '#lib/sf/types';

	let { runId, canStop }: { runId: string; canStop: boolean } = $props();

	let servers = $state<DevServer[]>([]);
	let asked = $state<number[]>([]);

	async function load(id: string) {
		try {
			const s = await api<DevServer[]>(`/v1/runs/${id}/dev-servers`);
			if (id === runId) servers = s;
		} catch {
			/* keep what we had */
		}
	}

	// Machines report with their heartbeat, so look again every few seconds; faster while one stops.
	$effect(() => {
		const id = runId;
		servers = [];
		asked = [];
		let timer: ReturnType<typeof setTimeout>;
		let gone = false;
		const tick = async () => {
			if (!document.hidden) await load(id);
			if (!gone) timer = setTimeout(tick, servers.some((s) => s.stopping) ? 3000 : 10000);
		};
		void tick();
		return () => { gone = true; clearTimeout(timer); };
	});

	async function stop(s: DevServer) {
		asked = [...asked, s.pid];
		try {
			servers = await api<DevServer[]>(`/v1/runs/${runId}/dev-servers/stop`, { method: 'POST', body: JSON.stringify({ pid: s.pid }) });
		} catch (err) {
			toast.error('Could not stop the server', { description: (err as Error).message });
			void load(runId);
		}
		asked = asked.filter((p) => p !== s.pid);
	}
</script>

{#if servers.length}
	<div class="border-t bg-background">
		<div class="mx-auto flex w-full max-w-3xl flex-col gap-1 px-4 pt-2.5 max-md:px-3">
			<p class="text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
				Running on {servers[0].worker_id}
			</p>
			<ul class="flex flex-col gap-1">
				{#each servers as s (s.pid)}
					{@const stopping = s.stopping || asked.includes(s.pid)}
					<li class="flex min-w-0 items-center gap-2.5 rounded-lg border bg-card py-1.5 pr-1.5 pl-2.5 text-xs">
						<Radio class="size-3.5 shrink-0 text-ok-ink" />
						<span class="tnum shrink-0 font-mono text-foreground/90">{s.ports.map((p) => ':' + p).join(' ')}</span>
						<span class="min-w-0 flex-1 truncate font-mono text-[11.5px] text-muted-foreground" title={s.command}>{s.command}</span>
						{#if s.started_at}<span class="tnum shrink-0 text-faint" title="Up since {new Date(s.started_at).toLocaleString()}">{ago(s.started_at, fleet.now)}</span>{/if}
						{#if canStop}
							<button
								type="button"
								class="inline-flex h-7 shrink-0 items-center gap-1 rounded-full border border-err/40 px-2.5 text-err-ink hover:bg-err-soft disabled:opacity-50"
								disabled={stopping}
								title="Stop this server (pid {s.pid}) and what it started"
								onclick={() => stop(s)}
							><Square class="size-2.5 fill-current" />{stopping ? 'Stopping…' : 'Stop'}</button>
						{/if}
					</li>
				{/each}
			</ul>
		</div>
	</div>
{/if}
