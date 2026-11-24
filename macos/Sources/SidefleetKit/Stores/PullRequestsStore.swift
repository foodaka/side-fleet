import Foundation
import Observation

/// A pull request you authored, as one of your machines saw it on GitHub.
public struct AuthoredPR: Sendable, Hashable, Decodable, Identifiable {
    public var url: String
    public var number: Int
    /// `owner/name`.
    public var repo: String
    public var title: String
    /// `open`, `merged` or `closed`.
    public var state: String
    public var draft: Bool
    public var additions: Int
    public var deletions: Int
    public var head: String
    public var createdAt: Date?
    public var updatedAt: Date?
    public var review: String?
    public var checks: PRChecks?
    /// The machine that saw it.
    public var machine: String
    /// The Sidefleet thread that opened it, if one did.
    public var threadID: String?
    public var threadTask: String?
    public var id: String { url }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        url = f.str("url")
        number = f.int("number")
        repo = f.str("repo")
        title = f.str("title")
        state = f.opt("state") ?? "open"
        draft = f.bool("draft")
        additions = f.int("additions")
        deletions = f.int("deletions")
        head = f.str("head")
        createdAt = f.date("created_at")
        updatedAt = f.date("updated_at")
        review = f.opt("review")
        checks = f.opt("checks")
        machine = f.str("machine")
        let thread: JSONValue? = f.opt("thread")
        threadID = thread?["id"]?.string
        threadTask = thread?["task"]?.string
    }
}

/// `GET /v1/pull-requests`: your machines, how recently each looked, and what they found.
public struct PullRequestsPage: Sendable, Hashable, Decodable {
    public struct Machine: Sendable, Hashable, Decodable, Identifiable {
        public var id: String
        public var name: String
        public var online: Bool
        public var canList: Bool
        public var fetchedAt: Date?
        public var error: String?
        /// Asked to look and hasn't answered yet.
        public var looking: Bool

        public init(from decoder: Decoder) throws {
            let f = try Fields(decoder)
            id = f.str("id")
            name = f.str("name")
            online = f.bool("online")
            canList = f.bool("can_list")
            fetchedAt = f.date("fetched_at")
            error = f.opt("error")
            looking = f.bool("looking")
        }
    }

    public var machines: [Machine]
    public var prs: [AuthoredPR]

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        machines = f.opt("machines") ?? []
        prs = f.opt("prs") ?? []
    }
}

/// The Pull requests screen. A port of web/src/routes/pulls/+page.svelte's state.
@MainActor
@Observable
public final class PullRequestsStore {
    public enum Show: String, CaseIterable, Sendable {
        case open, merged, closed, all
        public var title: String { rawValue == "all" ? "All" : rawValue.capitalized }
    }

    public enum Sort: String, CaseIterable, Sendable {
        case updated, newest, oldest, size
        public var title: String {
            switch self {
            case .updated: "Recently updated"
            case .newest: "Newest"
            case .oldest: "Oldest"
            case .size: "Most changed"
            }
        }
    }

    public struct Group: Identifiable, Sendable {
        public var title: String
        public var prs: [AuthoredPR]
        public var id: String { title }
    }

    /// Nil until the first answer.
    public private(set) var page: PullRequestsPage?
    public private(set) var error: String?
    public var query = ""
    public var show: Show = .all
    public var sort: Sort = .updated
    public var failing = false
    public var changesRequested = false
    public var hideDrafts = false
    /// One repo (`owner/name`), or every one.
    public var repo: String?

    @ObservationIgnored private let api: API

    public init(api: API) { self.api = api }

    public func load() async {
        do {
            page = try await api.pullRequests()
            error = nil
        } catch is CancellationError {
            return
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// Ask your machines to look on GitHub again; they do within a heartbeat.
    public func refresh() async throws {
        try await api.refreshPullRequests()
        await load()
    }

    public func reset() {
        page = nil
        error = nil
    }

    public var looking: Bool { page?.machines.contains { $0.looking } ?? false }
    public var lastLooked: Date? { page?.machines.compactMap(\.fetchedAt).max() }
    public var repos: [String] { Array(Set(page?.prs.map(\.repo) ?? [])).sorted() }
    public var filterCount: Int { [failing, changesRequested, hideDrafts, repo != nil].filter { $0 }.count }

    public func clearFilters() {
        failing = false
        changesRequested = false
        hideDrafts = false
        repo = nil
    }

    public var filtered: [AuthoredPR] {
        var q = query.trimmingCharacters(in: .whitespaces).lowercased()
        if q.hasPrefix("#") { q.removeFirst() }
        let prs = (page?.prs ?? []).filter { p in
            if show != .all, p.state != show.rawValue { return false }
            if let repo, p.repo != repo { return false }
            if failing, (p.checks?.failed ?? 0) == 0 { return false }
            if changesRequested, p.review != "changes_requested" { return false }
            if hideDrafts, p.draft { return false }
            if q.isEmpty { return true }
            return [p.title, p.repo, String(p.number), p.head, p.threadTask ?? ""].contains { $0.lowercased().contains(q) }
        }
        let distant = Date.distantPast
        switch sort {
        case .updated: return prs.sorted { ($0.updatedAt ?? distant) > ($1.updatedAt ?? distant) }
        case .newest: return prs.sorted { ($0.createdAt ?? distant) > ($1.createdAt ?? distant) }
        case .oldest: return prs.sorted { ($0.createdAt ?? distant) < ($1.createdAt ?? distant) }
        case .size: return prs.sorted { $0.additions + $0.deletions > $1.additions + $1.deletions }
        }
    }

    /// With everything shown: open ones first, then what was merged or closed.
    public var groups: [Group] {
        let prs = filtered
        guard show == .all else { return prs.isEmpty ? [] : [Group(title: show.title, prs: prs)] }
        return [
            Group(title: "Open", prs: prs.filter { $0.state == "open" }),
            Group(title: "Merged and closed", prs: prs.filter { $0.state != "open" }),
        ].filter { !$0.prs.isEmpty }
    }

    /// One line on each machine that isn't simply up to date.
    public var notes: [String] {
        (page?.machines ?? []).compactMap { m in
            if !m.canList { return "\(m.name) needs an update to list pull requests." }
            if let e = m.error { return "\(m.name) couldn't look: \(e)" }
            if !m.online, m.fetchedAt == nil { return "\(m.name) is offline." }
            return nil
        }
    }
}
