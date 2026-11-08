import Foundation

// Snoozing a thread: hidden from the list and the inbox until a time, or until it starts waiting on
// you for something new. The menu's choices are presets you can edit in Settings (web/src/lib/sf/snooze.ts).

public enum SnoozeWhen: Sendable, Hashable {
    case hours(Double)
    /// `time` is `HH:mm`; `day` is `today`, `tomorrow` or a weekday (`mon`…`sun`).
    case at(time: String, day: String)
}

public struct SnoozePreset: Sendable, Hashable, Identifiable {
    public var label: String
    public var when: SnoozeWhen
    public var id: String { label + String(describing: when) }

    public init(label: String, when: SnoozeWhen) {
        self.label = label
        self.when = when
    }

    public static let days: [(id: String, name: String)] = [
        ("today", "Today"), ("tomorrow", "Tomorrow"), ("mon", "Next Monday"), ("tue", "Next Tuesday"), ("wed", "Next Wednesday"),
        ("thu", "Next Thursday"), ("fri", "Next Friday"), ("sat", "Next Saturday"), ("sun", "Next Sunday"),
    ]

    public static let defaults: [SnoozePreset] = [
        SnoozePreset(label: "1 hour", when: .hours(1)),
        SnoozePreset(label: "3 hours", when: .hours(3)),
        SnoozePreset(label: "This evening", when: .at(time: "18:00", day: "today")),
        SnoozePreset(label: "Tomorrow morning", when: .at(time: "09:00", day: "tomorrow")),
        SnoozePreset(label: "Next week", when: .at(time: "09:00", day: "mon")),
    ]

    public static let maxCount = 8
    private static let weekdays = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]

    private static func validTime(_ t: String) -> Bool {
        let parts = t.split(separator: ":")
        guard parts.count == 2, parts[0].count == 2, parts[1].count == 2, let h = Int(parts[0]), let m = Int(parts[1]) else { return false }
        return (0..<24).contains(h) && (0..<60).contains(m)
    }

    /// When this preset, picked at `now`, wakes the thread.
    public func time(now: Date, calendar: Calendar = .current) -> Date {
        switch when {
        case .hours(let h):
            return now.addingTimeInterval(h * 3600)
        case .at(let time, let day):
            let parts = time.split(separator: ":").compactMap { Int($0) }
            let h = parts.first ?? 9, m = parts.count > 1 ? parts[1] : 0
            let base = calendar.date(bySettingHour: h, minute: m, second: 0, of: now) ?? now
            func plus(_ n: Int) -> Date { calendar.date(byAdding: .day, value: n, to: base) ?? base }
            switch day {
            case "today":
                return base <= now ? plus(1) : base
            case "tomorrow":
                return plus(1)
            default:
                // The next such weekday, never today.
                let target = Self.weekdays.firstIndex(of: day) ?? 1
                let current = calendar.component(.weekday, from: base) - 1
                let ahead = (target - current + 7) % 7
                return plus(ahead == 0 ? 7 : ahead)
            }
        }
    }

    // MARK: JSON, as the page stores them with your account

    public var json: JSONValue {
        switch when {
        case .hours(let h): ["label": .string(label), "hours": .number(h)]
        case .at(let time, let day): ["label": .string(label), "time": .string(time), "day": .string(day)]
        }
    }

    /// Presets as stored, with anything malformed dropped; the defaults when there are none.
    public static func clean(_ value: JSONValue?) -> [SnoozePreset] {
        guard let list = value?.array else { return defaults }
        var out: [SnoozePreset] = []
        for p in list {
            guard let label = p["label"]?.string?.trimmingCharacters(in: .whitespacesAndNewlines), !label.isEmpty else { continue }
            let short = String(label.prefix(40))
            if let h = p["hours"]?.double, h > 0, h <= 24 * 365 {
                out.append(SnoozePreset(label: short, when: .hours(h)))
            } else if let t = p["time"]?.string, validTime(t), let d = p["day"]?.string, days.contains(where: { $0.id == d }) {
                out.append(SnoozePreset(label: short, when: .at(time: t, day: d)))
            }
        }
        return out.isEmpty ? defaults : Array(out.prefix(maxCount))
    }
}
