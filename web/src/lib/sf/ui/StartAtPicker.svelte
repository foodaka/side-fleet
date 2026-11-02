<script lang="ts">
	// Start a task later: a few presets and a date-and-time picker. '' means now.
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu';
	import AlarmClock from '@lucide/svelte/icons/alarm-clock';
	import X from '@lucide/svelte/icons/x';
	import { startTime } from '../format';

	let { value = $bindable(''), size = 'default' }: { value?: string; size?: 'default' | 'sm' } = $props();

	/** How far ahead the control plane accepts. */
	const MAX_DAYS = 30;
	let custom = $state(false);
	let picked = $state('');

	/** Today or tomorrow at hh:mm, local time. */
	function at(h: number, m = 0, days = 0) {
		const d = new Date();
		d.setDate(d.getDate() + days);
		d.setHours(h, m, 0, 0);
		return d;
	}
	// Worked out when the menu opens, so they are never in the past.
	let presets = $state<{ label: string; at: Date }[]>([]);
	function refresh() {
		const now = new Date();
		const inHour = new Date(now.getTime() + 3600_000);
		inHour.setSeconds(0, 0);
		presets = [
			{ label: 'In an hour', at: inHour },
			...(at(22).getTime() - now.getTime() > 30 * 60_000 ? [{ label: 'Tonight', at: at(22) }] : []),
			{ label: 'Tomorrow morning', at: at(7, 0, 1) },
			{ label: 'Tomorrow, 9:00', at: at(9, 0, 1) }
		];
	}

	/** `<input type="datetime-local">` wants local `YYYY-MM-DDTHH:mm`. */
	const local = (d: Date) => new Date(d.getTime() - d.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
	const min = $derived(local(new Date()));
	const max = $derived(local(new Date(Date.now() + MAX_DAYS * 86400_000)));

	function pick(d: Date) {
		value = d.toISOString();
		custom = false;
	}
	function pickCustom() {
		custom = true;
		picked = value ? local(new Date(value)) : local(at(7, 0, 1));
		value = new Date(picked).toISOString();
	}
	function clear() {
		value = '';
		custom = false;
	}
	$effect(() => {
		if (!custom || !picked) return;
		const d = new Date(picked);
		if (!isNaN(d.getTime())) value = d.toISOString();
	});

	const text = $derived(size === 'sm' ? 'text-xs' : 'text-[13px]');
	const hint = 'text-xs text-muted-foreground';
</script>

<span class="inline-flex min-w-0 items-center gap-1">
	<DropdownMenu.Root onOpenChange={(o) => o && refresh()}>
		<DropdownMenu.Trigger
			class="inline-flex {size === 'sm' ? 'h-7' : 'h-8'} min-w-0 items-center gap-1.5 rounded-full {value ? 'px-2.5' : 'w-8 justify-center'} {text} text-muted-foreground transition-colors hover:bg-accent hover:text-foreground data-[state=open]:bg-accent data-[state=open]:text-foreground"
			aria-label={value ? `Starts ${startTime(value)}` : 'Start later'}
			title="Start later"
		>
			<AlarmClock class="size-3.5 shrink-0" />{#if value && !custom}<span class="truncate text-foreground/80">Starts {startTime(value)}</span>{/if}
		</DropdownMenu.Trigger>
		<DropdownMenu.Content align="start" class="w-60">
			<DropdownMenu.Label class={hint}>Start later; no machine picks it up before then</DropdownMenu.Label>
			<DropdownMenu.Item onclick={clear}><span class="flex-1">Now</span>{#if !value}<span class={hint}>✓</span>{/if}</DropdownMenu.Item>
			<DropdownMenu.Separator />
			{#each presets as p (p.label)}
				<DropdownMenu.Item onclick={() => pick(p.at)}><span class="flex-1">{p.label}</span><span class="tnum {hint}">{startTime(p.at.toISOString())}</span></DropdownMenu.Item>
			{/each}
			<DropdownMenu.Separator />
			<DropdownMenu.Item onclick={pickCustom}>Pick a date and time…</DropdownMenu.Item>
		</DropdownMenu.Content>
	</DropdownMenu.Root>
	{#if custom}
		<input
			type="datetime-local"
			bind:value={picked}
			{min}
			{max}
			aria-label="Start at"
			class="tnum h-7 rounded-md border border-input bg-transparent px-1.5 text-xs text-foreground"
		/>
		<button type="button" aria-label="Start now instead" title="Start now instead" onclick={clear} class="grid size-6 place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground">
			<X class="size-3" />
		</button>
	{/if}
</span>
