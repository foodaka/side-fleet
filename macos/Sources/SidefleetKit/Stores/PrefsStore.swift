import Foundation
import Observation

/// Which key sends from a message box. `smart`: replies go on Return, a new task (longer, and it takes
/// a machine) on Command-Return.
public enum SendKey: String, Sendable, CaseIterable, Identifiable {
    case smart, enter
    case modEnter = "mod-enter"
    public var id: String { rawValue }
}

public enum DiffLayout: String, Sendable, CaseIterable, Identifiable {
    case unified, split
    public var id: String { rawValue }
}

public enum Appearance: String, Sendable, CaseIterable, Identifiable {
    case system, light, dark
    public var id: String { rawValue }
}

public enum MessageBox: Sendable { case task, reply }

/// Your preferences. Signed in, the ones about tasks and sending are also kept with your account, so
/// the app and every browser start the same. A port of web/src/lib/sf/prefs.svelte.ts.
@MainActor
@Observable
public final class PrefsStore {
    public var sendKey: SendKey = .smart
    public var diffView: DiffLayout = .unified
    public var diffHideWhitespace = false
    public var diffCollapsed = false
    public var confirmStop = true
    public var permission = "bypassPermissions"
    public var roomPermission = "acceptEdits"
    public var workspace = "worktree"
    public var snoozePresets = SnoozePreset.defaults
    // Only on this Mac.
    public var appearance: Appearance = .system
    public var notifications = true
    public var keepRunningInMenuBar = true

    /// Signed in with GitHub: the synced ones are kept with your account too.
    public private(set) var account = false

    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private var api: API?
    private static let key = "sidefleet_prefs"
    private static let modes = ["acceptEdits", "bypassPermissions", "plan", "ask"]

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if let data = defaults.data(forKey: Self.key), let stored = try? JSONDecoder().decode(JSONValue.self, from: data) {
            apply(stored)
        }
    }

    private func apply(_ p: JSONValue) {
        if let s = p["sendKey"]?.string, let v = SendKey(rawValue: s) { sendKey = v }
        if let s = p["diffView"]?.string, let v = DiffLayout(rawValue: s) { diffView = v }
        if let b = p["diffHideWhitespace"]?.bool { diffHideWhitespace = b }
        if let b = p["diffCollapsed"]?.bool { diffCollapsed = b }
        if let b = p["confirmStop"]?.bool { confirmStop = b }
        if let s = p["permission"]?.string, Self.modes.contains(s) { permission = s }
        if let s = p["roomPermission"]?.string, Self.modes.contains(s) { roomPermission = s }
        if let s = p["workspace"]?.string { workspace = s == "checkout" ? "checkout" : "worktree" }
        if let v = p["snoozePresets"] { snoozePresets = SnoozePreset.clean(v) }
        if let s = p["appearance"]?.string, let v = Appearance(rawValue: s) { appearance = v }
        if let b = p["notifications"]?.bool { notifications = b }
        if let b = p["keepRunningInMenuBar"]?.bool { keepRunningInMenuBar = b }
    }

    /// What follows you between devices. How the Changes panel looks (and the theme) suits each screen, so it stays put.
    private var synced: JSONValue {
        [
            "permission": .string(permission),
            "roomPermission": .string(roomPermission),
            "workspace": .string(workspace),
            "sendKey": .string(sendKey.rawValue),
            "confirmStop": .bool(confirmStop),
            "snoozePresets": .array(snoozePresets.map(\.json)),
        ]
    }

    private var stored: JSONValue {
        var o = synced.object ?? [:]
        o["diffView"] = .string(diffView.rawValue)
        o["diffHideWhitespace"] = .bool(diffHideWhitespace)
        o["diffCollapsed"] = .bool(diffCollapsed)
        o["appearance"] = .string(appearance.rawValue)
        o["notifications"] = .bool(notifications)
        o["keepRunningInMenuBar"] = .bool(keepRunningInMenuBar)
        return .object(o)
    }

    /// Change a preference and save it.
    public func set<T>(_ path: ReferenceWritableKeyPath<PrefsStore, T>, _ value: T) {
        let before = synced
        self[keyPath: path] = value
        if let data = try? JSONEncoder().encode(stored) { defaults.set(data, forKey: Self.key) }
        // What only suits this Mac (the diff's layout, the theme) has nowhere to go.
        let body = synced
        if account, let api, body != before {
            // Kept here if this fails; it goes with the next change.
            Task { try? await api.savePrefs(body) }
        }
    }

    /// Signed in: take your account's preferences over this Mac's. The first time, this Mac's become your account's.
    public func connect(api: API) async {
        self.api = api
        account = true
        await pull()
    }

    /// Pick up changes made elsewhere, when you come back to the app.
    public func pull() async {
        guard let api else { return }
        // Offline, or nobody in particular: this Mac's copy stands.
        guard let theirs = try? await api.prefs() else { return }
        let keys = Set(synced.object?.keys.map { $0 } ?? [])
        let relevant = (theirs.object ?? [:]).filter { keys.contains($0.key) }
        if relevant.isEmpty {
            if defaults.data(forKey: Self.key) != nil { try? await api.savePrefs(synced) }
            return
        }
        apply(.object(relevant))
        if let data = try? JSONEncoder().encode(stored) { defaults.set(data, forKey: Self.key) }
    }

    /// The mode a new task starts with for this agent: only Claude Code can ask you, so the others edit files instead.
    public func permission(for agent: AgentID) -> String {
        permission == "ask" && agent != .claude ? "acceptEdits" : permission
    }

    /// The same for a task in a room, which starts narrower: room tasks read teammates' notes.
    public func roomPermission(for agent: AgentID) -> String {
        roomPermission == "ask" && agent != .claude ? "acceptEdits" : roomPermission
    }

    /// Whether plain Return sends from a box of this kind.
    public func enterSends(_ box: MessageBox) -> Bool {
        switch sendKey {
        case .enter: true
        case .modEnter: false
        case .smart: box == .reply
        }
    }

    /// The keys that send from a box of this kind, for a hint.
    public func sendKeys(_ box: MessageBox) -> String { enterSends(box) ? "↵" : "⌘↵" }
}

/// Which finished threads you've looked at since they last changed, so the sidebar can flag the ones
/// that finished while you were elsewhere. On this Mac only. The first launch marks everything seen.
@MainActor
@Observable
public final class SeenStore {
    private var since: Date?
    private var runs: [String: Date] = [:]
    @ObservationIgnored private let defaults: UserDefaults
    private static let key = "sidefleet_seen"
    private static let keep = 300

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if let data = defaults.data(forKey: Self.key), let s = try? JSONDecoder().decode(JSONValue.self, from: data), let t = s["since"]?.double {
            since = Date(timeIntervalSince1970: t)
            runs = (s["runs"]?.object ?? [:]).reduce(into: [:]) { out, kv in
                if let v = kv.value.double { out[kv.key] = Date(timeIntervalSince1970: v) }
            }
        }
    }

    /// Call once runs have loaded: on a first launch, everything so far counts as seen.
    public func start(_ list: [RunSummary]) {
        guard since == nil else { return }
        since = max(Date(), list.map(\.lastActivityAt).max() ?? .distantPast)
        save()
    }

    /// Finished, and changed since you last opened it.
    public func isUnread(_ r: RunSummary) -> Bool {
        guard let since, !r.isLive else { return false }
        // A date read back from storage can be off by rounding, and must not bring a read thread back.
        return r.lastActivityAt.timeIntervalSince(runs[r.id] ?? since) > 0.001
    }

    public func mark(_ r: RunSummary) {
        if let seen = runs[r.id], r.lastActivityAt.timeIntervalSince(seen) <= 0.001 { return }
        runs[r.id] = r.lastActivityAt
        if runs.count > Self.keep {
            for (id, _) in runs.sorted(by: { $0.value < $1.value }).prefix(runs.count - Self.keep) { runs[id] = nil }
        }
        save()
    }

    private func save() {
        let body: JSONValue = [
            "since": .number(since?.timeIntervalSince1970 ?? 0),
            "runs": .object(runs.mapValues { .number($0.timeIntervalSince1970) }),
        ]
        if let data = try? JSONEncoder().encode(body) { defaults.set(data, forKey: Self.key) }
    }
}
