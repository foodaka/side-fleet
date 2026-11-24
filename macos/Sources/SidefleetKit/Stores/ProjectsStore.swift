import Foundation
import Observation

/// Projects: the task list grouped by repo, each with an icon. The icons are the org's, kept on the
/// control plane so everyone (and the page) sees the same ones; how the list is grouped and which
/// groups are folded is yours, kept on this Mac. A port of web/src/lib/sf/projects.svelte.ts.
@MainActor
@Observable
public final class ProjectsStore {
    public enum ListView: String, Sendable, Hashable { case time, project }

    /// Icons set in this org, by repo.
    public private(set) var icons: [String: ProjectIcon] = [:]
    public private(set) var view: ListView = .time
    /// Groups folded shut.
    public private(set) var folded: Set<String> = []
    /// Groups showing every task rather than the latest few.
    public private(set) var showAll: Set<String> = []
    /// Groups left out of the list (they come back from the foot of it).
    public private(set) var hidden: Set<String> = []
    /// The repo whose icon is being picked; nil when the picker is shut.
    public var picking: String?

    @ObservationIgnored private let api: API
    @ObservationIgnored private let toasts: ToastCenter
    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private var loadedAt: Date?
    /// Bumps on an org switch, so icons still on their way from the last org are dropped.
    @ObservationIgnored private var generation = 0

    // The page's localStorage keys.
    private static let viewKey = "sidefleet_runs_view"
    private static let foldedKey = "sidefleet_projects_folded"
    private static let showAllKey = "sidefleet_projects_all"
    private static let hiddenKey = "sidefleet_projects_hidden"

    public init(api: API, toasts: ToastCenter, defaults: UserDefaults = .standard) {
        self.api = api
        self.toasts = toasts
        self.defaults = defaults
        if defaults.string(forKey: Self.viewKey) == ListView.project.rawValue { view = .project }
        folded = Self.readSet(defaults, Self.foldedKey)
        showAll = Self.readSet(defaults, Self.showAllKey)
        hidden = Self.readSet(defaults, Self.hiddenKey)
    }

    private static func readSet(_ d: UserDefaults, _ key: String) -> Set<String> {
        Set(d.stringArray(forKey: key) ?? [])
    }

    private func write(_ s: Set<String>, _ key: String) {
        defaults.set(s.sorted(), forKey: key)
    }

    public func icon(_ repo: String) -> ProjectIcon { icons[repo] ?? ProjectIcon.defaultIcon(repo) }

    public func setView(_ v: ListView) {
        view = v
        defaults.set(v.rawValue, forKey: Self.viewKey)
    }

    public func toggleFolded(_ repo: String) {
        if folded.remove(repo) == nil { folded.insert(repo) }
        write(folded, Self.foldedKey)
    }

    public func toggleShowAll(_ repo: String) {
        if showAll.remove(repo) == nil { showAll.insert(repo) }
        write(showAll, Self.showAllKey)
    }

    public func hide(_ repo: String) {
        hidden.insert(repo)
        write(hidden, Self.hiddenKey)
    }

    public func unhideAll() {
        hidden = []
        write(hidden, Self.hiddenKey)
    }

    private func take(_ list: [StoredProjectIcon]) {
        icons = list.reduce(into: [:]) { out, s in
            if let icon = s.icon { out[s.repo] = icon }
        }
    }

    /// Fetch the org's icons, at most once a minute unless forced.
    public func load(force: Bool = false) async {
        if !force, let at = loadedAt, Date().timeIntervalSince(at) < 60 { return }
        loadedAt = Date()
        let gen = generation
        // An older control plane, or offline: defaults until the next try.
        guard let list = try? await api.projectIcons(), gen == generation else { return }
        take(list)
    }

    /// Forget the last org's icons.
    public func reset() {
        generation += 1
        icons = [:]
        loadedAt = nil
        picking = nil
    }

    /// Set a repo's icon for the whole org (nil: back to the default), at once.
    public func setIcon(_ repo: String, _ icon: ProjectIcon?) async {
        let before = icons
        icons[repo] = icon
        let gen = generation
        do {
            let list = try await api.setProjectIcon(repo: repo, icon: icon)
            if gen == generation { take(list) }
        } catch {
            if gen == generation { icons = before }
            toasts.error("Could not set the icon", error)
        }
    }
}

/// One repo's group in the list By project.
public struct ProjectGroup: Sendable, Identifiable {
    public var repo: String
    /// Waiting on you, running, queued, scheduled, then the latest.
    public var runs: [RunSummary]
    /// 0 waiting on you, 1 running, 2 queued, 3 scheduled, 4 nothing live.
    public var urgency: Int
    public var latest: Date
    public var id: String { repo }

    /// A group shows this many tasks until "Show N more".
    public static let shown = 5

    /// Live threads waiting on you.
    public var waiting: Int { runs.filter { $0.pendingApprovals > 0 && $0.isLive }.count }
    public var running: Int { runs.filter { $0.threadStatus == .running && $0.pendingApprovals == 0 }.count }

    /// The rows it shows: the first few, and the open thread wherever it is.
    public func visible(showAll: Bool, openID: String?) -> [RunSummary] {
        showAll ? runs : runs.enumerated().filter { $0.offset < Self.shown || $0.element.id == openID }.map(\.element)
    }

    /// One group per repo, the ones waiting on you or busy first, then the latest.
    static func make(_ runs: [RunSummary], now: Date) -> [ProjectGroup] {
        var by: [String: [RunSummary]] = [:]
        var order: [String] = []
        for r in runs {
            if by[r.run.repo] == nil { order.append(r.run.repo) }
            by[r.run.repo, default: []].append(r)
        }
        func before(_ a: RunSummary, _ b: RunSummary) -> Bool {
            let (ra, rb) = (SidebarSections.rank(a, now: now), SidebarSections.rank(b, now: now))
            if ra != rb { return ra < rb }
            if a.run.isScheduled(now: now), b.run.isScheduled(now: now), let x = a.run.startAt, let y = b.run.startAt, x != y { return x < y }
            return a.lastActivityAt > b.lastActivityAt
        }
        let groups = order.map { repo -> ProjectGroup in
            let rs = by[repo] ?? []
            return ProjectGroup(
                repo: repo,
                runs: rs.sorted(by: before),
                urgency: rs.map { $0.isLive ? SidebarSections.rank($0, now: now) : 4 }.min() ?? 4,
                latest: rs.map(\.lastActivityAt).max() ?? .distantPast
            )
        }
        return groups.sorted { a, b in
            if a.urgency != b.urgency { return a.urgency < b.urgency }
            if a.latest != b.latest { return a.latest > b.latest }
            return a.repo < b.repo
        }
    }
}
