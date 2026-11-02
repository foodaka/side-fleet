<script lang="ts">
	// Settings: the choices in a task's Snooze menu. Each is some hours from now, or a
	// time of day on a day; every change is saved at once.
	import { fleet } from '../state.svelte';
	import { prefs } from '../prefs.svelte';
	import { DEFAULT_SNOOZE_PRESETS, MAX_SNOOZE_PRESETS, SNOOZE_DAYS, presetTime, wakeLabel, type SnoozeDay, type SnoozePreset } from '../snooze';
	import X from '@lucide/svelte/icons/x';
	import Plus from '@lucide/svelte/icons/plus';

	const field = 'h-8 rounded-lg border border-input bg-card px-2.5 text-[13px] outline-none focus:shadow-[0_0_0_4px_color-mix(in_oklab,var(--ring)_8%,transparent)]';

	function save(list: SnoozePreset[]) {
		prefs.set({ snoozePresets: list });
	}
	function change(i: number, p: SnoozePreset) {
		save(prefs.snoozePresets.map((x, j) => (j === i ? p : x)));
	}
	function setKind(i: number, kind: string) {
		const p = prefs.snoozePresets[i];
		change(i, kind === 'hours' ? { label: p.label, hours: 1 } : { label: p.label, time: '09:00', day: 'tomorrow' });
	}
	const isDefault = $derived(JSON.stringify(prefs.snoozePresets) === JSON.stringify(DEFAULT_SNOOZE_PRESETS));
</script>

<div class="divide-y rounded-xl border bg-card">
	{#each prefs.snoozePresets as p, i (i)}
		<div class="flex flex-wrap items-center gap-2 px-4 py-2.5 sm:px-5">
			<input
				class="{field} min-w-0 flex-1 basis-36"
				value={p.label}
				maxlength={40}
				aria-label="Name"
				onchange={(e) => {
					const label = e.currentTarget.value.trim();
					if (label) change(i, { ...p, label });
					else e.currentTarget.value = p.label;
				}}
			/>
			<select class="{field} w-28" aria-label="Kind" value={'hours' in p ? 'hours' : 'at'} onchange={(e) => setKind(i, e.currentTarget.value)}>
				<option value="hours">In hours</option>
				<option value="at">At a time</option>
			</select>
			{#if 'hours' in p}
				<input
					type="number"
					min="0.25"
					max="8760"
					step="0.25"
					class="{field} tnum w-20"
					aria-label="Hours from now"
					value={p.hours}
					onchange={(e) => {
						const hours = Number(e.currentTarget.value);
						if (hours > 0 && hours <= 8760) change(i, { label: p.label, hours });
						else e.currentTarget.value = String(p.hours);
					}}
				/>
			{:else}
				<input
					type="time"
					class="{field} tnum w-32"
					aria-label="Time"
					value={p.time}
					onchange={(e) => {
						if (/^\d\d:\d\d$/.test(e.currentTarget.value)) change(i, { ...p, time: e.currentTarget.value });
					}}
				/>
				<select class="{field} w-36" aria-label="Day" value={p.day} onchange={(e) => change(i, { ...p, day: e.currentTarget.value as SnoozeDay })}>
					{#each SNOOZE_DAYS as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
				</select>
			{/if}
			<span class="tnum ml-auto text-right text-[12px] whitespace-nowrap text-muted-foreground" title="Picked now, it would come back then">{wakeLabel(presetTime(p, fleet.now), fleet.now)}</span>
			<button
				type="button"
				class="grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-40"
				aria-label="Remove {p.label}"
				disabled={prefs.snoozePresets.length <= 1}
				onclick={() => save(prefs.snoozePresets.filter((_, j) => j !== i))}
			>
				<X class="size-3.5" />
			</button>
		</div>
	{/each}
	<div class="flex items-center gap-3 px-4 py-2.5 sm:px-5">
		<button
			type="button"
			class="flex items-center gap-1.5 text-[12.5px] font-medium text-foreground/80 hover:text-foreground disabled:opacity-40"
			disabled={prefs.snoozePresets.length >= MAX_SNOOZE_PRESETS}
			onclick={() => save([...prefs.snoozePresets, { label: 'Later today', hours: 4 }])}
		>
			<Plus class="size-3.5" />Add a choice
		</button>
		{#if !isDefault}
			<button type="button" class="ml-auto text-[12.5px] text-muted-foreground hover:text-foreground" onclick={() => save(DEFAULT_SNOOZE_PRESETS)}>Reset to the defaults</button>
		{/if}
	</div>
</div>
