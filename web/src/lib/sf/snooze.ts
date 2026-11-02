// Snoozing a thread: hidden from the list and the inbox until a time, or until it
// starts waiting on you for something new (it finishes, fails, or asks to run a
// command). A reply brings it back too. The menu's choices are presets you can edit
// in Settings.

export type SnoozeDay = 'today' | 'tomorrow' | 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';

/** A choice in the Snooze menu: some hours from now, or a time of day on a day. */
export type SnoozePreset = { label: string; hours: number } | { label: string; time: string; day: SnoozeDay };

export const SNOOZE_DAYS: { id: SnoozeDay; name: string }[] = [
	{ id: 'today', name: 'Today' },
	{ id: 'tomorrow', name: 'Tomorrow' },
	{ id: 'mon', name: 'Next Monday' },
	{ id: 'tue', name: 'Next Tuesday' },
	{ id: 'wed', name: 'Next Wednesday' },
	{ id: 'thu', name: 'Next Thursday' },
	{ id: 'fri', name: 'Next Friday' },
	{ id: 'sat', name: 'Next Saturday' },
	{ id: 'sun', name: 'Next Sunday' }
];

export const DEFAULT_SNOOZE_PRESETS: SnoozePreset[] = [
	{ label: '1 hour', hours: 1 },
	{ label: '3 hours', hours: 3 },
	{ label: 'This evening', time: '18:00', day: 'today' },
	{ label: 'Tomorrow morning', time: '09:00', day: 'tomorrow' },
	{ label: 'Next week', time: '09:00', day: 'mon' }
];

export const MAX_SNOOZE_PRESETS = 8;
const WEEKDAYS: SnoozeDay[] = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;

/** Presets as stored, with anything malformed dropped; the defaults when there are none. */
export function cleanPresets(v: unknown): SnoozePreset[] {
	if (!Array.isArray(v)) return DEFAULT_SNOOZE_PRESETS;
	const out: SnoozePreset[] = [];
	for (const p of v) {
		if (!p || typeof p !== 'object' || typeof p.label !== 'string' || !p.label.trim()) continue;
		const label = p.label.trim().slice(0, 40);
		if (typeof p.hours === 'number' && p.hours > 0 && p.hours <= 24 * 365) out.push({ label, hours: p.hours });
		else if (typeof p.time === 'string' && TIME.test(p.time) && SNOOZE_DAYS.some((d) => d.id === p.day)) out.push({ label, time: p.time, day: p.day });
	}
	return out.length ? out.slice(0, MAX_SNOOZE_PRESETS) : DEFAULT_SNOOZE_PRESETS;
}

/** When a preset picked at `now` wakes the thread. */
export function presetTime(p: SnoozePreset, now: number): Date {
	if ('hours' in p) return new Date(now + p.hours * 3_600_000);
	const [h, m] = p.time.split(':').map(Number);
	const d = new Date(now);
	d.setHours(h, m, 0, 0);
	if (p.day === 'today') {
		if (d.getTime() <= now) d.setDate(d.getDate() + 1);
	} else if (p.day === 'tomorrow') {
		d.setDate(d.getDate() + 1);
	} else {
		// The next such weekday, never today.
		const ahead = (WEEKDAYS.indexOf(p.day) - d.getDay() + 7) % 7 || 7;
		d.setDate(d.getDate() + ahead);
	}
	return d;
}

/** "18:00", "Tue 09:00" or "12 Nov 09:00": when a snooze ends, as short as is clear. */
export function wakeLabel(at: Date | string, now: number): string {
	const d = typeof at === 'string' ? new Date(at) : at;
	const time = d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
	const today = new Date(now);
	const days = Math.round((new Date(d).setHours(0, 0, 0, 0) - today.setHours(0, 0, 0, 0)) / 86_400_000);
	if (days === 0) return time;
	if (days === 1) return `Tomorrow ${time}`;
	if (days < 7) return `${d.toLocaleDateString(undefined, { weekday: 'short' })} ${time}`;
	return `${d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })} ${time}`;
}

/** A date for an `<input type="datetime-local">`, in local time. */
export function localInput(d: Date): string {
	const p = (n: number) => String(n).padStart(2, '0');
	return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** Whether a thread is snoozed right now. */
export function isSnoozed(r: { snoozed_until?: string | null }, now: number): boolean {
	return !!r.snoozed_until && Date.parse(r.snoozed_until) > now;
}
