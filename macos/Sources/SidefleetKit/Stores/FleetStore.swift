import Foundation
import Observation

public enum Connection: Sendable, Hashable { case connecting, ok, unreachable, unauthorized }

/// A small message with an optional Undo. macOS has nothing native for this.
public struct Toast: Sendable, Identifiable {
    public let id = UUID()
    public var title: String
    public var detail: String?
    public var isError = false
    public var actionTitle: String?
    public var action: (@MainActor @Sendable () -> Void)?
}

@MainActor
@Observable
public final class ToastCenter {
    public private(set) var toasts: [Toast] = []

    public init() {}

    public func show(_ title: String, detail: String? = nil, isError: Bool = false, actionTitle: String? = nil, action: (@MainActor @Sendable () -> Void)? = nil) {
        let toast = Toast(title: title, detail: detail, isError: isError, actionTitle: actionTitle, action: action)
        toasts.append(toast)
        if toasts.count > 3 { toasts.removeFirst(toasts.count - 3) }
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(isError ? 7 : 5))
            self?.dismiss(toast.id)
        }
    }

    public func error(_ title: String, _ error: Error) {
        show(title, detail: error.localizedDescription, isError: true)
    }

    public func dismiss(_ id: UUID) { toasts.removeAll { $0.id == id } }
}

/// One machine's recent CPU, collected while the app runs, and what just happened on the fleet.
public enum FleetEvent: Sendable, Hashable {
    case beat(worker: String)
    case dispatch(worker: String)
    case done(worker: String)
}

/// Everything the screens share, refreshed from the control plane. A port of `Fleet` from
/// web/src/lib/sf/state.svelte.ts.
@MainActor
@Observable
public final class FleetStore {
    public private(set) var workers: [Worker] = []
    public private(set) var myMachines: [MyMachine] = []
    public private(set) var runs: [RunSummary] = []
    public private(set) var added: Set<String>?
    public private(set) var setup: Setup?
    public private(set) var conn: Connection = .connecting
    public private(set) var loaded = false
    public private(set) var archived: [RunSummary]?
    public private(set) var me: Me?
    public private(set) var pending: [Enrollment] = []
    public private(set) var members: [Member] = []
    public private(set) var orgID: String
    /// The time of the latest refresh: enough for lists. Live durations tick on their own.
    public private(set) var now = Date()
    /// Last 40 CPU readings per machine (about ten minutes at one reading per heartbeat).
    public private(set) var cpu: [String: [Double]] = [:]
    /// Bumps with each new fleet event, so the map can draw them.
    public private(set) var recentEvents: [FleetEvent] = []
    public private(set) var eventCounter = 0
    public private(set) var updatesCleared: String

    public let api: API
    public let toasts: ToastCenter
    /// After each refresh that worked: the app keeps its badge, read marks and notifications up to date from here.
    @ObservationIgnored public var onRefreshed: (@MainActor () -> Void)?
    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private var pollTask: Task<Void, Never>?
    @ObservationIgnored private var active = true
    @ObservationIgnored private var membersFor = ""
    @ObservationIgnored private var membersAt = Date.distantPast
    @ObservationIgnored private var beats: [String: Date] = [:]
    @ObservationIgnored private var runStates: [String: RunStatus] = [:]
    @ObservationIgnored private var primed = false

    private static let orgKey = "sidefleet_org"
    private static let clearedKey = "sf.updatesCleared"

    public init(api: API, toasts: ToastCenter, defaults: UserDefaults = .standard) {
        self.api = api
        self.toasts = toasts
        self.defaults = defaults
        orgID = defaults.string(forKey: Self.orgKey) ?? ""
        updatesCleared = defaults.string(forKey: Self.clearedKey) ?? ""
        api.backend.setOrg(orgID)
    }

    // MARK: Derived

    public var online: [Worker] { workers.filter(\.online) }
    /// Machines that run your tasks: each machine only runs its owner's.
    public var mine: [Worker] { workers.filter(\.mine) }
    public var myOnline: [Worker] { mine.filter(\.online) }
    /// The signed-in user's id; nil with the developer key.
    public var userID: String? { me?.user?.id }
    /// Several people share this org: show who started what.
    public var multi: Bool { members.count > 1 }
    public var needsSignIn: Bool { conn == .unauthorized }
    /// The org the app acts in.
    public var org: Org? { me?.orgs.first { $0.id == orgID } ?? me?.orgs.first }
    public var isOwner: Bool { org?.role == "owner" }
    public var running: Int { runs.filter { $0.threadStatus == .running }.count }
    public var waitingApprovals: Int { runs.filter { $0.isLive }.reduce(0) { $0 + $1.pendingApprovals } }

    /// A machine is updated by its owner; org owners only step in for one nobody owns.
    public func canUpdate(_ w: Worker) -> Bool { w.mine || (w.owner == nil && isOwner) }
    /// Its owner, or an org owner (to clean up after someone who left), may remove it.
    public func canRemove(_ w: Worker) -> Bool { w.mine || isOwner }

    /// Online machines on an older build that you may update and haven't asked to yet.
    public var updatable: [Worker] { workers.filter { $0.online && $0.outdated && !$0.updateRequested && canUpdate($0) } }
    public var updateNotice: Bool { !updatable.isEmpty && updatesCleared != (setup?.build ?? "") }

    /// A user's login, if they are in this org.
    public func login(_ id: String?) -> String? { id.flatMap { id in members.first { $0.id == id }?.login } }

    /// Whether a thread (or message) was started by someone else; unknown authors count as yours.
    public func othersRun(_ createdBy: String?) -> Bool {
        guard let createdBy, let userID else { return false }
        return createdBy != userID
    }

    /// Every repo any machine reports, online machines first.
    public var allRepos: [String: [Worker]] {
        var out: [String: [Worker]] = [:]
        for w in workers.sorted(by: { $0.online && !$1.online }) {
            for r in w.capabilities.repos where !(out[r.key]?.contains(where: { $0.id == w.id }) ?? false) {
                out[r.key, default: []].append(w)
            }
        }
        return out
    }

    public var onlineRepos: Set<String> { Set(online.flatMap { $0.capabilities.repos.map(\.key) }) }

    /// What the composer's repo menu offers: the chosen repos, or all of them when there are only a few.
    public var composerRepos: [String] {
        let picked: [String]
        if let added, !added.isEmpty {
            picked = Array(added)
        } else {
            picked = onlineRepos.count <= 8 ? Array(onlineRepos) : []
        }
        return picked.sorted()
    }

    public func cleared() {
        updatesCleared = setup?.build ?? ""
        defaults.set(updatesCleared, forKey: Self.clearedKey)
    }

    // MARK: Polling

    /// Every 5 seconds in front, every 30 behind: a laptop's battery matters, and the inbox stream carries what's urgent.
    public func start() {
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                // Signed out: nothing more to learn until someone signs in, which starts afresh.
                if self?.conn == .unauthorized { break }
                let seconds = (self?.active ?? false) ? 5 : 30
                try? await Task.sleep(for: .seconds(seconds))
            }
        }
    }

    public func stop() {
        pollTask?.cancel()
        pollTask = nil
    }

    public func setActive(_ active: Bool) {
        self.active = active
        if active { Task { await refresh() } }
    }

    public func refresh(retry: Bool = true) async {
        do {
            async let me = api.me()
            async let workers = api.workers()
            async let runs = api.runs(limit: 100)
            // What isn't needed to run the app must not bring it down, on an older control plane especially.
            async let repos = try? api.addedRepos()
            async let pending = try? api.enrollments()
            async let machines = try? api.myMachines()
            let (m, w, r, added, enrollments, mine) = try await (me, workers, runs, repos, pending, machines)
            self.me = m
            self.workers = w
            self.runs = r
            self.pending = enrollments ?? []
            self.myMachines = mine ?? []
            if let added { self.added = Set(added) }
            now = Date()
            ingest(w, r)
            if setup == nil { setup = try? await api.setup() }
            await loadMembers()
            conn = .ok
            loaded = true
            onRefreshed?()
        } catch APIError.status(403, _) where !orgID.isEmpty && retry {
            // The remembered org is gone (left it, or another account): fall back to the first one.
            switchOrg("")
        } catch APIError.unauthorized {
            conn = .unauthorized
        } catch is CancellationError {
            return
        } catch {
            conn = .unreachable
        }
    }

    private func loadMembers(force: Bool = false) async {
        guard me?.user != nil, let org else {
            members = []
            return
        }
        if !force, membersFor == org.id, Date().timeIntervalSince(membersAt) < 60 { return }
        membersFor = org.id
        membersAt = Date()
        if let list = try? await api.members(org: org.id) { members = list }
    }

    private func ingest(_ workers: [Worker], _ runs: [RunSummary]) {
        var events: [FleetEvent] = []
        for w in workers {
            let prev = beats[w.id]
            guard prev != w.lastHeartbeatAt else { continue }
            beats[w.id] = w.lastHeartbeatAt
            if let sys = w.capabilities.system { cpu[w.id] = Array(((cpu[w.id] ?? []) + [sys.cpuPercent]).suffix(40)) }
            if primed, prev != nil, w.online { events.append(.beat(worker: w.id)) }
        }
        for r in runs {
            let prev = runStates[r.id]
            runStates[r.id] = r.threadStatus
            guard primed, prev != r.threadStatus, let worker = r.run.workerID else { continue }
            if r.threadStatus == .running {
                events.append(.dispatch(worker: worker))
            } else if prev == .running {
                events.append(.done(worker: worker))
            }
        }
        primed = true
        recentEvents = events
        if !events.isEmpty { eventCounter += events.count }
    }

    // MARK: Lists

    public func loadArchived() async {
        do {
            archived = try await api.archivedRuns()
        } catch {
            toasts.error("Could not load archived tasks", error)
        }
    }

    /// Archive a thread (or bring it back), moving it between the lists at once; archiving offers an undo.
    public func setArchived(_ r: RunSummary, archived isArchived: Bool) async {
        let beforeRuns = runs, beforeArchived = archived
        var moved = r
        moved.run.archivedAt = isArchived ? Date() : nil
        func newest(_ list: [RunSummary]) -> [RunSummary] { list.sorted { $0.lastActivityAt > $1.lastActivityAt } }
        if isArchived {
            runs.removeAll { $0.id == r.id }
            if let list = archived { archived = newest([moved] + list) }
        } else {
            archived?.removeAll { $0.id == r.id }
            runs = newest([moved] + runs)
        }
        do {
            try await api.setArchived(run: r.id, archived: isArchived)
            if isArchived {
                toasts.show("Archived", detail: String(r.run.task.prefix(80)), actionTitle: "Undo") { [weak self] in
                    Task { await self?.setArchived(moved, archived: false) }
                }
            }
        } catch {
            runs = beforeRuns
            archived = beforeArchived
            toasts.error(isArchived ? "Could not archive" : "Could not unarchive", error)
        }
        await refresh()
    }

    /// Snooze a thread until a time (or wake it with nil), at once; snoozing offers an undo.
    public func setSnoozed(_ r: RunSummary, until: Date?) async {
        let before = runs
        if let i = runs.firstIndex(where: { $0.id == r.id }) { runs[i].run.snoozedUntil = until }
        do {
            try await api.snooze(run: r.id, until: until)
            if let until {
                toasts.show("Snoozed until \(Format.wakeLabel(until, now: Date()))", detail: String(r.run.task.prefix(80)), actionTitle: "Undo") { [weak self] in
                    Task { await self?.setSnoozed(r, until: nil) }
                }
            }
        } catch {
            runs = before
            toasts.error(until != nil ? "Could not snooze" : "Could not unsnooze", error)
        }
        await refresh()
    }

    public func saveAdded(_ names: [String]) async throws {
        added = Set(try await api.saveAddedRepos(names))
    }

    // MARK: Machines

    public func requestUpdate(_ w: Worker) async {
        do {
            try await api.requestUpdate(worker: w.id)
            toasts.show("\(w.displayName) will update", detail: "It pulls, rebuilds and restarts as soon as it has no running tasks.")
            await refresh()
        } catch {
            toasts.error("Could not request the update", error)
        }
    }

    public func remove(_ w: Worker) async {
        do {
            try await api.removeWorker(w.id)
            toasts.show("Removed \(w.displayName)")
            await refresh()
        } catch {
            toasts.error("Could not remove the machine", error)
        }
    }

    // MARK: Orgs

    /// Act in another of the user's orgs: everything on screen is per org.
    public func switchOrg(_ id: String) {
        defaults.set(id, forKey: Self.orgKey)
        orgID = id
        api.backend.setOrg(id)
        workers = []
        runs = []
        archived = nil
        pending = []
        members = []
        membersFor = ""
        added = nil
        setup = nil
        loaded = false
        conn = .connecting
        beats = [:]
        runStates = [:]
        cpu = [:]
        primed = false
        Task { await refresh(retry: false) }
    }

    public func reloadMembers() async { await loadMembers(force: true) }

    public func renameOrg(_ name: String) async throws {
        guard let org else { return }
        try await api.renameOrg(org.id, name: name)
        await refresh()
    }
}

/// How the sidebar groups threads: a port of web/src/lib/sf/ui/RunList.svelte's derived state.
public struct SidebarSections: Sendable {
    public struct Group: Sendable, Identifiable {
        public var label: String
        public var runs: [RunSummary]
        public var id: String { label }
    }

    public var active: [RunSummary] = []
    public var groups: [Group] = []
    public var snoozed: [RunSummary] = []
    /// The repo most runs are in; rows from any other repo say which one.
    public var homeRepo = ""
    /// Teammates' tasks, for the Team tab's count.
    public var theirs = 0
    public var isEmpty: Bool { active.isEmpty && groups.isEmpty }

    public init(runs all: [RunSummary], userID: String?, teamScope: Bool, filtering: Bool, now: Date, calendar: Calendar = .current) {
        let scoped = filtering && !teamScope ? all.filter { $0.run.createdBy == userID } : all
        let awake = scoped.filter { !$0.isSnoozed(now: now) }
        snoozed = scoped.filter { $0.isSnoozed(now: now) }.sorted { ($0.run.snoozedUntil ?? now) < ($1.run.snoozedUntil ?? now) }
        theirs = filtering ? all.filter { $0.run.createdBy != userID && !$0.isSnoozed(now: now) }.count : 0

        var counts: [String: Int] = [:]
        for r in awake { counts[r.run.repo, default: 0] += 1 }
        homeRepo = counts.max { ($0.value, $1.key) < ($1.value, $0.key) }?.key ?? ""

        // Threads waiting on you first, then running, then queued, then those scheduled for later (soonest first).
        func rank(_ r: RunSummary) -> Int {
            if r.pendingApprovals > 0 { return 0 }
            if r.threadStatus == .running { return 1 }
            return r.run.isScheduled(now: now) ? 3 : 2
        }
        active = awake.filter(\.isLive).enumerated().sorted { a, b in
            let (ra, rb) = (rank(a.element), rank(b.element))
            if ra != rb { return ra < rb }
            if ra == 3, let x = a.element.run.startAt, let y = b.element.run.startAt, x != y { return x < y }
            return a.offset < b.offset
        }.map(\.element)

        for r in awake where !r.isLive {
            let label = Format.dayGroup(r.lastActivityAt, now: now, calendar: calendar).rawValue
            if groups.last?.label == label {
                groups[groups.count - 1].runs.append(r)
            } else {
                groups.append(Group(label: label, runs: [r]))
            }
        }
    }

    /// Search covers title, repo and machine, and archived tasks once their list has been opened.
    public static func matches(query: String, runs: [RunSummary], archived: [RunSummary]?) -> [RunSummary] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !q.isEmpty else { return [] }
        return (runs + (archived ?? [])).filter { r in
            [r.run.task, Format.repoLabel(r.run.repo), r.run.workerID ?? ""].contains { $0.lowercased().contains(q) }
        }
    }
}
