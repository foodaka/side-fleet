// The night shift's hours: a window like 01:00–07:00 in this browser's time zone, set in
// Settings. A task put on the night shift starts at the window's start (or now, when the
// window is already open), only on a machine with nothing else running, and is stopped at
// its end if still going.

export const DEFAULT_NIGHT = { start: '01:00', end: '07:00' };

const HM = /^([01]\d|2[0-3]):([0-5]\d)$/;
export const validTime = (t: unknown): t is string => typeof t === 'string' && HM.test(t);

function at(day: Date, hm: string, addDays = 0): Date {
	const [h, m] = hm.split(':').map(Number);
	const d = new Date(day);
	d.setDate(d.getDate() + addDays);
	d.setHours(h, m, 0, 0);
	return d;
}

/**
 * The next night shift from `now`. `open`: the window has already begun, so the task can
 * start right away and only the end applies. A window may cross midnight (22:00–06:00).
 */
export function nextNight(start: string, end: string, now = new Date()): { start: Date; end: Date; open: boolean } {
	const s = validTime(start) ? start : DEFAULT_NIGHT.start;
	const e = validTime(end) ? end : DEFAULT_NIGHT.end;
	// Tonight's window, and the one that began yesterday (which may still be open after midnight).
	for (const day of [-1, 0, 1]) {
		const from = at(now, s, day);
		let to = at(now, e, day);
		if (to <= from) to = at(now, e, day + 1);
		if (now < to) return now >= from ? { start: now, end: to, open: true } : { start: from, end: to, open: false };
	}
	// Unreachable: tomorrow's window always ends after now.
	return { start: at(now, s, 1), end: at(now, e, 2), open: false };
}
