import Foundation
import Observation

/// A slash command or skill Claude Code offers on a machine (`/v1/workers/{id}/commands`).
public struct SlashCommand: Sendable, Hashable, Decodable, Identifiable {
    /// Without the slash: `review`, `stripe:test-cards`.
    public var name: String
    public var description: String
    /// What it takes after its name, e.g. `[pr number]`.
    public var argumentHint: String

    public var id: String { name }

    public init(name: String, description: String = "", argumentHint: String = "") {
        self.name = name
        self.description = description
        self.argumentHint = argumentHint
    }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        name = f.str("name")
        description = f.str("description")
        argumentHint = f.str("argument_hint")
    }
}

extension API {
    /// The skills and slash commands Claude Code offers on `worker`, `repo`'s own first. Only for the machine's owner.
    public func slashCommands(worker: String, repo: String) async throws -> [SlashCommand] {
        let q = repo.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-._~/"))) ?? repo
        return try await get("/v1/workers/\(Self.pathPart(worker))/commands?repo=\(q)")
    }
}

/// The menu after `/` in a composer: a port of web/src/lib/sf/slash.ts and SlashMenu.svelte. It shows
/// while the message is just `/` and a word, as that's where Claude Code reads a command.
@MainActor
@Observable
public final class SlashMenuModel {
    /// nil while loading.
    public private(set) var commands: [SlashCommand]?
    public private(set) var failed = false
    /// What's typed after the slash, while the menu applies.
    public private(set) var query: String?
    /// The highlighted row in `matches`.
    public var active = 0
    /// Escape hides the menu until the text changes.
    private var dismissed: String?
    @ObservationIgnored private var text = ""
    @ObservationIgnored private var loaded = ""
    @ObservationIgnored private var loading: Task<Void, Never>?

    /// Lists fetched, per machine and repo; asked again after a minute.
    private static var cache: [String: (at: Date, list: [SlashCommand])] = [:]
    static let freshFor: TimeInterval = 60

    public init() {}

    public var isOpen: Bool { query != nil && dismissed == nil }
    public var matches: [SlashCommand] { Array(Self.match(commands ?? [], query: query ?? "").prefix(60)) }

    /// Call when the text, machine, repo or agent changes. Only Claude Code's commands are listed.
    public func update(text: String, worker: String?, repo: String, enabled: Bool, api: API) {
        if text != self.text { dismissed = nil }
        self.text = text
        let q = enabled && worker != nil ? Self.query(text) : nil
        if q != query {
            query = q
            active = 0
        }
        guard isOpen, let worker else { return }
        load(worker: worker, repo: repo, api: api)
    }

    private func load(worker: String, repo: String, api: API) {
        let key = "\(worker)\n\(repo)"
        if let hit = Self.cache[key], Date().timeIntervalSince(hit.at) < Self.freshFor {
            if loaded != key || commands != hit.list {
                loaded = key
                commands = hit.list
                failed = false
            }
            return
        }
        if loaded == key, loading != nil { return }
        if loaded != key { commands = nil }
        loaded = key
        failed = false
        loading?.cancel()
        loading = Task { [weak self] in
            let result: [SlashCommand]?
            do { result = try await api.slashCommands(worker: worker, repo: repo) } catch { result = nil }
            guard let self, !Task.isCancelled, self.loaded == key else { return }
            self.loading = nil
            if let result {
                Self.cache[key] = (Date(), result)
                self.commands = result
            } else {
                self.failed = true
                self.commands = []
            }
        }
    }

    public enum Key: Sendable { case up, down, tab, enter, escape }

    public enum KeyResult: Equatable, Sendable {
        /// Not the menu's: the box does what it always does.
        case pass
        case handled
        /// Put this in the box instead.
        case replace(String)
    }

    /// The composer passes its keys here first while the menu is open.
    public func handle(_ key: Key) -> KeyResult {
        guard isOpen else { return .pass }
        if key == .escape {
            dismissed = text
            return .handled
        }
        let list = matches
        guard !list.isEmpty else { return .pass }
        switch key {
        case .up, .down:
            active = (active + (key == .down ? 1 : -1) + list.count) % list.count
            return .handled
        case .tab, .enter:
            let c = list[min(active, list.count - 1)]
            // Enter on a command already typed in full sends it.
            if key == .enter, text == "/\(c.name)" { return .pass }
            return .replace(Self.completion(c))
        case .escape:
            return .handled
        }
    }

    /// What the box holds once `c` is picked.
    public static func completion(_ c: SlashCommand) -> String { "/\(c.name) " }

    /// What's typed after the slash while the message is only `/word`, else nil.
    public static func query(_ text: String) -> String? {
        guard text.hasPrefix("/") else { return nil }
        let rest = text.dropFirst()
        return rest.contains(where: { $0.isWhitespace || $0 == "/" }) ? nil : String(rest)
    }

    /// Names starting with the query, then names with a part starting with it, then names
    /// containing it, then descriptions mentioning it; the server's order within each.
    public static func match(_ list: [SlashCommand], query: String) -> [SlashCommand] {
        let q = query.lowercased()
        guard !q.isEmpty else { return list }
        func rank(_ c: SlashCommand) -> Int? {
            let n = c.name.lowercased()
            if n.hasPrefix(q) { return 0 }
            if n.split(whereSeparator: { ":-_".contains($0) }).contains(where: { $0.hasPrefix(q) }) { return 1 }
            if n.contains(q) { return 2 }
            if c.description.lowercased().contains(q) { return 3 }
            return nil
        }
        return list.enumerated()
            .compactMap { i, c in rank(c).map { (c, $0, i) } }
            .sorted { ($0.1, $0.2) < ($1.1, $1.2) }
            .map(\.0)
    }
}
