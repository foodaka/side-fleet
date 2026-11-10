import Foundation

// A port of web/src/lib/sf/format.ts. A new model family goes in both tables; versions come from the machines.

public struct AgentInfo: Sendable, Hashable, Identifiable {
    public let id: AgentID
    public let name: String
    public let hint: String
}

public struct ModelInfo: Sendable, Hashable, Identifiable {
    public let id: String
    public let name: String
    public let hint: String
}

public struct EffortInfo: Sendable, Hashable, Identifiable {
    public let id: String
    public let name: String
    public let hint: String?
}

public struct PermissionInfo: Sendable, Hashable, Identifiable {
    public let id: String
    public let name: String
    public let hint: String
}

public struct WorkspaceInfo: Sendable, Hashable, Identifiable {
    public let id: String
    public let name: String
    public let hint: String
}

public enum Format {
    public static let agents: [AgentInfo] = [
        AgentInfo(id: .claude, name: "Claude Code", hint: "Anthropic's agent, with the machine's Claude sign-in"),
        AgentInfo(id: .codex, name: "Codex", hint: "OpenAI's agent, with the machine's ChatGPT sign-in"),
        AgentInfo(id: .opencode, name: "opencode", hint: "Open source; any model it is set up for (OpenRouter, local…)"),
    ]

    public static func agentName(_ id: AgentID?) -> String { agents.first { $0.id == id }?.name ?? "Claude Code" }

    /// Claude Code's model aliases; Codex and opencode offer whatever each machine reports.
    /// Which version an alias runs comes from the machine: see `pickerModelName`.
    public static let models: [ModelInfo] = [
        ModelInfo(id: "sonnet", name: "Sonnet", hint: "Balanced"),
        ModelInfo(id: "opus", name: "Opus", hint: "Most capable"),
        ModelInfo(id: "fable", name: "Fable", hint: "Newest family"),
        ModelInfo(id: "haiku", name: "Haiku", hint: "Fastest"),
    ]

    /// A Claude alias as the picker shows it: `Opus 5.5` when the machines in `models` (`machineModels`)
    /// agree on what it runs, plain `Opus` when they don't or haven't said; anything else as `modelName` reads it.
    public static func pickerModelName(_ id: String, models running: [String] = []) -> String {
        guard let m = models.first(where: { $0.id == id }) else { return modelName(id) }
        let mine = running.filter { $0.hasPrefix("claude-\(id)-") }
        guard mine.count == 1, let version = aliasVersion(mine[0]) else { return m.name }
        return "\(m.name) \(version)"
    }

    /// `claude-fable-5-2` -> `5.2`, `claude-opus-5` -> `5`, `claude-sonnet-4-20250514` -> `4`: a minor
    /// version is one or two digits, so a date after the major isn't taken for one.
    private static func aliasVersion(_ id: String) -> String? {
        let parts = id.split(separator: "-", omittingEmptySubsequences: false)
        func digits(_ s: Substring) -> Bool { !s.isEmpty && s.allSatisfy { $0.isASCII && $0.isNumber } }
        guard parts.count >= 3, parts[0] == "claude", !parts[1].isEmpty, parts[1].allSatisfy({ $0.isASCII && $0.isLowercase }) else { return nil }
        // The page's pattern takes the digits the id's third part starts with, whatever follows them.
        let major = parts[2].prefix { $0.isASCII && $0.isNumber }
        guard !major.isEmpty else { return nil }
        guard major.count == parts[2].count, parts.count > 3 else { return String(major) }
        let minor = parts[3].prefix { $0.isASCII && $0.isNumber }
        guard digits(minor), minor.count <= 2 else { return String(major) }
        return "\(major).\(minor)"
    }

    /// `claude-opus-5-5` -> (`Opus`, `5.5`).
    private static func parseClaudeID(_ id: String) -> (String, String)? {
        let parts = id.split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count == 4, parts[0] == "claude", !parts[1].isEmpty, parts[1].allSatisfy({ $0.isLowercase && $0.isLetter }),
              Int(parts[2]) != nil, Int(parts[3]) != nil else { return nil }
        return (parts[1].prefix(1).uppercased() + parts[1].dropFirst(), "\(parts[2]).\(parts[3])")
    }

    /// How a model reads: `Sonnet`, `Sonnet 5.5`, `gpt-6-luna`, `moonshotai/kimi-k2` (opencode's provider prefix dropped).
    public static func modelName(_ id: String, agent: AgentID? = nil) -> String {
        if id.isEmpty { return "default model" }
        if let m = models.first(where: { $0.id == id }) { return m.name }
        if let (name, version) = parseClaudeID(id) { return "\(name) \(version)" }
        if agent == .opencode, let slash = id.firstIndex(of: "/") { return String(id[id.index(after: slash)...]) }
        return id
    }

    /// Claude Code takes one of its aliases or a full id; Codex and opencode any id that can't pass as a flag.
    public static func modelValid(agent: AgentID, model: String) -> Bool {
        if agent == .claude {
            if models.contains(where: { $0.id == model }) { return true }
            guard model.hasPrefix("claude-"), model.count > 7 else { return false }
            return model.unicodeScalars.allSatisfy { CharacterSet.lowercaseLetters.contains($0) || CharacterSet.decimalDigits.contains($0) || $0 == "-" }
        }
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "._:/@+-"))
        return model.unicodeScalars.allSatisfy { allowed.contains($0) && $0.isASCII } && !model.hasPrefix("-")
    }

    public static func defaultModel(_ agent: AgentID) -> String { agent == .claude ? "sonnet" : "" }

    public static func agentModel(_ agent: AgentID?, _ model: String) -> String {
        "\(agentName(agent)) · \(modelName(model, agent: agent))"
    }

    public static func efforts(_ agent: AgentID) -> [EffortInfo] {
        switch agent {
        case .claude:
            [
                EffortInfo(id: "low", name: "Low", hint: nil),
                EffortInfo(id: "medium", name: "Medium", hint: nil),
                EffortInfo(id: "high", name: "High", hint: nil),
                EffortInfo(id: "xhigh", name: "Extra High", hint: nil),
                EffortInfo(id: "max", name: "Max", hint: nil),
                EffortInfo(id: "ultrathink", name: "Ultrathink", hint: "Deep thinking on this message"),
                EffortInfo(id: "ultracode", name: "Ultracode", hint: "xhigh effort plus multi-agent workflow orchestration"),
            ]
        case .codex:
            [
                EffortInfo(id: "minimal", name: "Minimal", hint: nil),
                EffortInfo(id: "low", name: "Low", hint: nil),
                EffortInfo(id: "medium", name: "Medium", hint: nil),
                EffortInfo(id: "high", name: "High", hint: nil),
                EffortInfo(id: "xhigh", name: "Extra High", hint: nil),
            ]
        case .opencode:
            []
        }
    }

    public static func effortValid(agent: AgentID, effort: String) -> Bool {
        effort.isEmpty || efforts(agent).contains { $0.id == effort }
    }

    public static func effortName(agent: AgentID?, effort: String?) -> String {
        efforts(agent ?? .claude).first { $0.id == effort }?.name ?? effort ?? ""
    }

    /// Models the given machines offer for an agent, most common first. For Claude Code: what its
    /// aliases run on them.
    public static func machineModels(_ workers: [Worker], agent: AgentID) -> [String] {
        var order: [String] = []
        var count: [String: Int] = [:]
        for w in workers {
            let capability = w.capabilities.agents?[agent]
            // The control plane sends the map sorted by alias; a dictionary here has no order of its own.
            let list = agent == .claude ? (capability?.aliases ?? [:]).sorted { $0.key < $1.key }.map(\.value) : capability?.models ?? []
            for m in list {
                if count[m] == nil { order.append(m) }
                count[m, default: 0] += 1
            }
        }
        // Stable: ties keep the order they were found in.
        return order.enumerated().sorted { (count[$0.element]!, $1.offset) > (count[$1.element]!, $0.offset) }.map(\.element)
    }

    public static let permissions: [PermissionInfo] = [
        PermissionInfo(id: "acceptEdits", name: "Read & edit files", hint: "Can change files; shell commands are refused"),
        PermissionInfo(id: "bypassPermissions", name: "Full access", hint: "Can also run commands on that machine"),
        PermissionInfo(id: "plan", name: "Read only", hint: "Looks and answers, changes nothing"),
        PermissionInfo(id: "ask", name: "Ask me", hint: "Edits files; asks you here before commands and anything else"),
    ]

    /// The modes an agent offers: only Claude Code can ask you yet.
    public static func perms(for agent: AgentID) -> [PermissionInfo] {
        permissions.filter { $0.id != "ask" || agent == .claude }
    }

    /// What a permission mode means for an agent: Codex maps them onto its sandbox.
    public static func permHint(_ id: String, agent: AgentID) -> String {
        if agent == .codex {
            if id == "acceptEdits" { return "Edits files and runs commands inside its sandbox, no network" }
            if id == "bypassPermissions" { return "No sandbox: runs anything on that machine" }
        }
        return permissions.first { $0.id == id }?.hint ?? ""
    }

    public static func permName(_ id: String?) -> String {
        if let p = permissions.first(where: { $0.id == id }) { return p.name }
        return id == "default" ? "Default permissions" : id ?? ""
    }

    public static let workspaces: [WorkspaceInfo] = [
        WorkspaceInfo(id: "worktree", name: "Own branch", hint: "A separate worktree and branch, so tasks run side by side"),
        WorkspaceInfo(id: "checkout", name: "In place", hint: "The checkout itself; one task per repo at a time"),
    ]

    // MARK: Times

    public static func rel(_ date: Date?, now: Date) -> String {
        guard let date else { return "" }
        let s = max(0, now.timeIntervalSince(date))
        if s < 10 { return "just now" }
        if s < 60 { return "\(Int(s.rounded()))s ago" }
        if s < 3600 { return "\(Int((s / 60).rounded()))m ago" }
        if s < 86_400 { return "\(Int((s / 3600).rounded()))h ago" }
        return "\(Int((s / 86_400).rounded()))d ago"
    }

    /// A time ahead: `in 7 days`, `in 5 hours`, `in 20 minutes`, `soon`.
    public static func until(_ date: Date?, now: Date) -> String {
        guard let date else { return "" }
        let s = date.timeIntervalSince(now)
        func n(_ v: Int, _ unit: String) -> String { "in \(v) \(unit)\(v == 1 ? "" : "s")" }
        if s < 60 { return "soon" }
        if s < 3600 { return n(Int((s / 60).rounded()), "minute") }
        if s < 86_400 { return n(Int((s / 3600).rounded()), "hour") }
        return n(Int((s / 86_400).rounded()), "day")
    }

    /// The compact form for tight spaces: `now`, `5m`, `2h`, `3d`, then the date.
    public static func ago(_ date: Date?, now: Date) -> String {
        guard let date else { return "" }
        let s = max(0, now.timeIntervalSince(date))
        if s < 60 { return "now" }
        if s < 3600 { return "\(Int(s / 60))m" }
        if s < 86_400 { return "\(Int(s / 3600))h" }
        if s < 7 * 86_400 { return "\(Int(s / 86_400))d" }
        return shortDate(date)
    }

    public enum DayGroup: String, Sendable { case today = "Today", yesterday = "Yesterday", week = "Previous 7 days", older = "Older" }

    /// Which calendar bucket a time falls in, by the viewer's local days.
    public static func dayGroup(_ date: Date, now: Date, calendar: Calendar = .current) -> DayGroup {
        let midnight = calendar.startOfDay(for: now)
        if date >= midnight { return .today }
        if date >= midnight.addingTimeInterval(-86_400) { return .yesterday }
        if date >= midnight.addingTimeInterval(-7 * 86_400) { return .week }
        return .older
    }

    public static func duration(start: Date?, end: Date? = nil, now: Date) -> String {
        guard let start else { return "" }
        let total = Int(max(0, (end ?? now).timeIntervalSince(start)).rounded())
        if total < 60 { return "\(total)s" }
        if total < 3600 { return "\(total / 60)m \(pad(total % 60))s" }
        return "\(total / 3600)h \(pad(Int((Double(total % 3600) / 60).rounded(.down))))m"
    }

    private static func pad(_ n: Int) -> String { n < 10 ? "0\(n)" : "\(n)" }

    private static func hm(_ date: Date, _ calendar: Calendar) -> String {
        let c = calendar.dateComponents([.hour, .minute], from: date)
        return "\(pad(c.hour ?? 0)):\(pad(c.minute ?? 0))"
    }

    private static func template(_ pattern: String, _ date: Date, _ calendar: Calendar) -> String {
        let f = DateFormatter()
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.locale = calendar.locale ?? .current
        f.setLocalizedDateFormatFromTemplate(pattern)
        return f.string(from: date)
    }

    public static func shortDate(_ date: Date, calendar: Calendar = .current) -> String { template("MMMd", date, calendar) }

    /// When a scheduled task starts: `14:30` today, `Tue 07:00` within a week, else `Nov 12, 07:00`.
    public static func startTime(_ date: Date, now: Date, calendar: Calendar = .current) -> String {
        let t = hm(date, calendar)
        if calendar.isDate(date, inSameDayAs: now) { return t }
        if date.timeIntervalSince(now) < 6 * 86_400 { return "\(template("EEE", date, calendar)) \(t)" }
        return "\(template("MMMd", date, calendar)), \(t)"
    }

    /// "18:00", "Tomorrow 09:00", "Tue 09:00" or "Nov 12 09:00": when a snooze ends, as short as is clear.
    public static func wakeLabel(_ date: Date, now: Date, calendar: Calendar = .current) -> String {
        let t = hm(date, calendar)
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: now), to: calendar.startOfDay(for: date)).day ?? 0
        if days == 0 { return t }
        if days == 1 { return "Tomorrow \(t)" }
        if days < 7 { return "\(template("EEE", date, calendar)) \(t)" }
        return "\(template("MMMd", date, calendar)) \(t)"
    }

    public static func clock(_ date: Date, calendar: Calendar = .current) -> String {
        let s = calendar.component(.second, from: date)
        return "\(hm(date, calendar)):\(pad(s))"
    }

    public static func money(_ n: Double?, digits: Int = 2) -> String {
        guard let n else { return "" }
        return "$" + String(format: "%.\(digits)f", n)
    }

    public static func pct(_ used: Double, _ total: Double) -> Int {
        total > 0 ? min(100, Int((used / total * 100).rounded())) : 0
    }

    public static func uptime(_ secs: Int) -> String {
        let d = secs / 86_400, h = secs % 86_400 / 3600
        return d > 0 ? "\(d)d \(h)h" : "\(h)h \(secs % 3600 / 60)m"
    }

    /// `1,234`, however the machine's locale groups digits.
    public static func grouped(_ n: Int) -> String { n.formatted() }

    /// "0.1.0 (2026-10-20 dec70b750)" -> (version, date, sha).
    public static func parseBuild(_ build: String?) -> (version: String, date: String, sha: String) {
        let s = build ?? ""
        let parts = s.split(separator: " ")
        guard parts.count == 3, parts[1].hasPrefix("("), parts[2].hasSuffix(")") else { return (s, "", "") }
        return (String(parts[0]), String(parts[1].dropFirst()), String(parts[2].dropLast()))
    }

    /// How a repo identity reads: `github.com/foodaka/side-fleet` -> `foodaka/side-fleet`.
    public static func repoLabel(_ key: String?) -> String {
        guard let key, !key.isEmpty else { return "" }
        let parts = key.split(separator: "/", omittingEmptySubsequences: false)
        if parts.count >= 3, parts[0].contains(".") { return parts.dropFirst().joined(separator: "/") }
        return key
    }

    /// `src/lib/x.ts` -> (`src/lib/`, `x.ts`).
    public static func splitPath(_ path: String) -> (dir: String, name: String) {
        guard let i = path.lastIndex(of: "/") else { return ("", path) }
        return (String(path[...i]), String(path[path.index(after: i)...]))
    }

    /// `1.2k`, `3M`: a token count for tight spaces.
    public static func shortCount(_ n: Int) -> String {
        if n >= 1_000_000 {
            let v = (Double(n) / 1e6 * 10).rounded() / 10
            return v == v.rounded() ? "\(Int(v))M" : "\(v)M"
        }
        return n >= 1000 ? "\(Int((Double(n) / 1000).rounded()))k" : "\(n)"
    }

    /// Only these open from agent output: it is untrusted, and `file:` or an app's scheme could do anything.
    public static func safeLink(_ raw: String?) -> URL? {
        guard let raw, let url = URL(string: raw.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme?.lowercased(), ["http", "https", "mailto"].contains(scheme) else { return nil }
        return url
    }
}
