import Foundation

/// The night shift's hours: a window like 01:00–07:00 in this Mac's time zone, set in Settings. A task
/// put on the night shift starts at the window's start (or now, when it's already open), only on a
/// machine with nothing else running, and is stopped at its end if still going. A port of
/// web/src/lib/sf/night.ts.
public enum NightShift {
    public static let defaultStart = "01:00"
    public static let defaultEnd = "07:00"
    /// The control plane takes a night of at most this long.
    public static let maxHours = 16

    /// `HH:MM`, 24-hour.
    public static func isValid(_ t: String) -> Bool {
        let parts = t.split(separator: ":")
        guard parts.count == 2, parts[0].count == 2, parts[1].count == 2, let h = Int(parts[0]), let m = Int(parts[1]) else { return false }
        return (0..<24).contains(h) && (0..<60).contains(m)
    }

    /// How long a window lasts, in minutes. The same start and end is a whole day.
    public static func minutes(start: String, end: String) -> Int {
        func mins(_ t: String) -> Int { (Int(t.prefix(2)) ?? 0) * 60 + (Int(t.suffix(2)) ?? 0) }
        let length = (mins(end) - mins(start) + 1440) % 1440
        return length == 0 ? 1440 : length
    }

    /// The next night shift from `now`. `open`: the window has already begun, so the task can start
    /// right away and only the end applies. A window may cross midnight (22:00–06:00).
    public static func next(start: String, end: String, now: Date = Date(), calendar: Calendar = .current) -> (start: Date, end: Date, open: Bool) {
        let s = isValid(start) ? start : defaultStart
        let e = isValid(end) ? end : defaultEnd
        func at(_ hm: String, days: Int) -> Date {
            let day = calendar.date(byAdding: .day, value: days, to: now) ?? now
            return calendar.date(bySettingHour: Int(hm.prefix(2)) ?? 0, minute: Int(hm.suffix(2)) ?? 0, second: 0, of: day) ?? day
        }
        // Tonight's window, and the one that began yesterday (which may still be open after midnight).
        for day in -1...1 {
            let from = at(s, days: day)
            var to = at(e, days: day)
            if to <= from { to = at(e, days: day + 1) }
            if now < to { return now >= from ? (now, to, true) : (from, to, false) }
        }
        // Unreachable: tomorrow's window always ends after now.
        return (at(s, days: 1), at(e, days: 2), false)
    }
}
