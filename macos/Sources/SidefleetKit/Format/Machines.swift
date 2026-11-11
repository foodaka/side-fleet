import Foundation

// What the Machines screen says about a machine (web/src/lib/sf/format.ts, MachineCard.svelte).

extension Format {
    /// Disk space in the units people read it in.
    public static func fileSize(_ bytes: Double) -> String {
        if bytes >= 1e9 { return String(format: "%.1f GB", bytes / 1e9) }
        if bytes >= 1e6 { return "\(Int((bytes / 1e6).rounded())) MB" }
        return "under 1 MB"
    }

    /// When a usage limit resets: `14:30` today, else `Tue 14:30`.
    public static func limitClock(_ date: Date, now: Date, calendar: Calendar = .current) -> String {
        let c = calendar.dateComponents([.hour, .minute], from: date)
        let time = String(format: "%02d:%02d", c.hour ?? 0, c.minute ?? 0)
        if calendar.isDate(date, inSameDayAs: now) { return time }
        let f = DateFormatter()
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.locale = calendar.locale ?? .current
        f.setLocalizedDateFormatFromTemplate("EEE")
        return "\(f.string(from: date)) \(time)"
    }

    /// The agent a usage limit is for. One this build doesn't know reads as the control plane named it.
    public static func limitedAgentName(_ key: String) -> String {
        AgentID(rawValue: key).map { agentName($0) } ?? key
    }

    /// Whether an address is this computer's own, which no other machine can reach.
    public static func isLoopback(_ url: URL?) -> Bool {
        guard let host = url?.host?.lowercased() else { return false }
        return host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]"
    }

    /// Minutes until a machine's request to join expires, never below zero.
    public static func minutesLeft(_ expires: Date?, now: Date) -> Int {
        guard let expires else { return 0 }
        return max(0, Int((expires.timeIntervalSince(now) / 60).rounded(.up)))
    }
}
