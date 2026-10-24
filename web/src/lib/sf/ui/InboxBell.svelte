<script lang="ts">
	import { goto } from '$app/navigation';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import Bell from '@lucide/svelte/icons/bell';
	import { fleet } from '../state.svelte';
	import { inbox, reasonTone } from '../inbox.svelte';
	import { ago, repoLabel } from '../format';

	let { side = 'bottom', align = 'start', class: className = '' }: {
		side?: 'bottom' | 'right';
		align?: 'start' | 'end';
		class?: string;
	} = $props();

	let open = $state(false);
	const count = $derived(inbox.items.length);
	const pushHint: Record<string, string> = {
		unsupported: 'This browser can’t show notifications. On iPhone, add Sidefleet to the Home Screen first.',
		insecure: 'Notifications need the page on HTTPS (or localhost).',
		unavailable: 'This control plane has no push key set up.',
		denied: 'Notifications are blocked for this site in the browser’s settings.'
	};

	function openTask(id: string) {
		open = false;
		void inbox.dismiss(id);
		void goto('#/runs/' + id);
	}

	function detail(d?: string) {
		// "resets <time>" from a usage limit reads better as a local time.
		const m = d && /^resets (.+)$/.exec(d);
		if (m) return 'resets ' + new Date(m[1]).toLocaleString([], { weekday: 'short', hour: 'numeric', minute: '2-digit' });
		return d ?? '';
	}
</script>

<DropdownMenu.Root bind:open>
	<DropdownMenu.Trigger
		class="mi-hover relative grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground data-[state=open]:bg-sidebar-accent data-[state=open]:text-foreground {className}"
		aria-label={count ? `Waiting on you: ${count}` : 'Waiting on you: nothing'}
		title="Waiting on you"
	>
		<Bell size={14} />
		{#if count}
			<span class="tnum absolute -top-0.5 -right-0.5 grid h-3.5 min-w-3.5 place-items-center rounded-full bg-err px-[3px] text-[9.5px] leading-none font-semibold text-white">
				{count > 99 ? '99+' : count}
			</span>
		{/if}
	</DropdownMenu.Trigger>
	<DropdownMenu.Content {side} {align} class="w-80 p-0">
		<div class="flex items-center justify-between gap-2 border-b px-3 py-2">
			<span class="text-[13px] font-medium">Waiting on you</span>
			{#if count}
				<button type="button" class="rounded px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground" onclick={() => void inbox.clearAll()}>
					Clear all
				</button>
			{/if}
		</div>
		<div class="max-h-[min(60vh,420px)] overflow-y-auto p-1 [scrollbar-width:thin]">
			{#each inbox.items as i (i.id)}
				<DropdownMenu.Item class="flex items-start gap-2.5 px-2 py-2" onclick={() => openTask(i.id)}>
					<span class="mt-[5px] size-2 shrink-0 rounded-full {reasonTone(i.reason)}" aria-hidden="true"></span>
					<span class="flex min-w-0 flex-1 flex-col gap-0.5">
						<span class="truncate text-[13px] text-foreground">{i.title}</span>
						<span class="truncate text-[11.5px] text-muted-foreground">
							{i.label}{#if i.detail}&nbsp;· {detail(i.detail)}{/if}
						</span>
						<span class="truncate font-mono text-[10.5px] text-faint">{repoLabel(i.repo)}</span>
					</span>
					<span class="tnum shrink-0 text-[11px] text-faint" title="Waiting since {new Date(i.since).toLocaleString()}">{ago(i.since, fleet.now)}</span>
				</DropdownMenu.Item>
			{:else}
				<p class="px-3 py-6 text-center text-xs text-muted-foreground">
					{inbox.loaded ? 'Nothing is waiting on you.' : 'Loading…'}
				</p>
			{/each}
		</div>
		<div class="border-t p-1">
			{#if inbox.push === 'on' || inbox.push === 'off'}
				<DropdownMenu.CheckboxItem
					checked={inbox.push === 'on'}
					disabled={inbox.pushBusy}
					closeOnSelect={false}
					onCheckedChange={(v) => void inbox.setPush(v)}
					class="text-[13px]"
				>
					<span class="flex flex-col">
						<span>Notify me</span>
						<span class="text-xs text-muted-foreground">A push on this device when a task starts waiting</span>
					</span>
				</DropdownMenu.CheckboxItem>
			{:else}
				<p class="px-2 py-1.5 text-xs text-muted-foreground">{pushHint[inbox.push]}</p>
			{/if}
			{#if inbox.pushError}<p class="px-2 pb-1.5 text-xs text-err-ink">{inbox.pushError}</p>{/if}
		</div>
	</DropdownMenu.Content>
</DropdownMenu.Root>
