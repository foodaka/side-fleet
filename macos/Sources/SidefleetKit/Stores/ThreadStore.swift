import Foundation
import Observation

/// A conversation: the root run plus its follow-ups, each streamed into a turn. A port of `Thread` from
/// web/src/lib/sf/thread.svelte.ts.
@MainActor
@Observable
public final class ThreadStore {
    public private(set) var turns: [Turn] = []
    public private(set) var error: String?
    public private(set) var loaded = false

    /// A run that was live just ended; lists and machine slots have changed.
    @ObservationIgnored public var onRunEnded: (@MainActor () -> Void)?

    @ObservationIgnored private let api: API
    @ObservationIgnored private var tasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var disposed = false
    @ObservationIgnored private var pendingLive: [String: RunEvent] = [:]
    @ObservationIgnored private var flushing = false
    @ObservationIgnored private let liveInterval: Duration
    @ObservationIgnored private let retryAfter: Duration

    /// `assistant_live` can come in fast; the view only needs a few redraws a second.
    public init(api: API, liveInterval: Duration = .milliseconds(100), retryAfter: Duration = .milliseconds(1500)) {
        self.api = api
        self.liveInterval = liveInterval
        self.retryAfter = retryAfter
    }

    public var runs: [Run] { turns.map(\.run) }
    public var root: Run? { turns.first?.run }
    public var latest: Run? { turns.last?.run }
    public var live: Bool { latest?.status.isLive ?? false }
    /// The message being worked on: the running one, else the latest (messages can wait behind it).
    public var current: Run? { turns.last { $0.run.status == .running }?.run ?? latest }
    public var branch: String? { turns.first { $0.branch != nil }?.branch }
    public var machine: String? { turns.last { $0.machine != nil }?.machine }
    public var hasSession: Bool { turns.contains { $0.run.sessionID != nil } }

    /// Everything the thread has changed: the latest turn that changed anything knows the total.
    public var changes: DiffSet? {
        guard let t = turns.last(where: { $0.turnDiff != nil }) else { return nil }
        return t.totalDiff ?? t.turnDiff
    }

    /// How full the session's context is, after the latest message its agent finished.
    public var context: (agent: AgentID, fill: ContextFill?)? {
        guard let t = turns.last(where: { $0.run.kind == .agent && $0.result != nil }) else { return nil }
        return (t.run.agent, t.result?.context)
    }

    /// Whether this run is a message waiting for an earlier one to finish.
    public func isWaiting(_ id: String) -> Bool {
        guard let i = turns.firstIndex(where: { $0.run.id == id }), i > 0, turns[i].run.status == .queued else { return false }
        return turns[..<i].contains { $0.run.status.isLive }
    }

    /// The previous turn that ran the agent, to tell what this one changed.
    public func previousAgentTurn(before index: Int) -> Turn? {
        turns[..<index].last { $0.run.kind == .agent }
    }

    /// The latest finished turn: what a fork from the sidebar continues from.
    public var forkTurn: Turn? {
        turns.last { [.succeeded, .failed, .stopped].contains($0.run.status) }
    }

    /// Queued or running messages behind the first, which stopping drops too.
    public var queuedCount: Int { max(0, turns.filter { $0.run.status.isLive }.count - 1) }

    public func load(_ id: String) async {
        do {
            turns = try await api.thread(id).map(Turn.init(run:))
        } catch {
            self.error = error.localizedDescription
            loaded = true
            return
        }
        // Replay finished turns in order; live ones (running, or queued behind it) stream side by side.
        for turn in turns {
            let task = follow(turn.run.id, wasLive: turn.run.status.isLive)
            if turn.run.status.isTerminal { await task.value }
        }
        loaded = true
    }

    public func dispose() {
        disposed = true
        tasks.values.forEach { $0.cancel() }
        tasks = [:]
    }

    @discardableResult
    private func follow(_ id: String, wasLive: Bool) -> Task<Void, Never> {
        let api = api
        let retryAfter = retryAfter
        let task = Task { [weak self] in
            await api.followRun(id, retryAfter: retryAfter, onEvent: { ev in
                self?.receive(ev, runID: id)
            }, onReconnect: {
                self?.update(id) { $0.reconnecting = true }
            })
            guard !Task.isCancelled else { return }
            await MainActor.run {
                guard let self else { return }
                self.flushLive()
                if wasLive { self.onRunEnded?() }
            }
        }
        tasks[id] = task
        return task
    }

    private func update(_ id: String, _ change: (inout Turn) -> Void) {
        guard let i = turns.firstIndex(where: { $0.run.id == id }) else { return }
        change(&turns[i])
    }

    private func receive(_ ev: RunEvent, runID: String) {
        guard !disposed else { return }
        if ev.kind == "assistant_live" {
            pendingLive[runID] = ev
            scheduleFlush()
            return
        }
        // A stored event replaces the live text it was written from.
        pendingLive[runID] = nil
        apply(ev, runID: runID)
    }

    private func scheduleFlush() {
        guard !flushing else { return }
        flushing = true
        let interval = liveInterval
        Task { [weak self] in
            try? await Task.sleep(for: interval)
            self?.flushLive()
        }
    }

    private func flushLive() {
        flushing = false
        let pending = pendingLive
        pendingLive = [:]
        for (id, ev) in pending { apply(ev, runID: id) }
    }

    private func apply(_ ev: RunEvent, runID: String) {
        guard let i = turns.firstIndex(where: { $0.run.id == runID }) else { return }
        turns[i].reconnecting = false
        for effect in turns[i].apply(ev) {
            switch effect {
            case .adopt(let id):
                Task { [weak self] in await self?.adopt(id) }
            case .pullRequest(let pr):
                if !turns.isEmpty { turns[0].run.pr = pr }
            case .refetchRun:
                let api = api
                Task { [weak self] in
                    if let run = try? await api.run(runID) { self?.update(runID) { $0.run.result = run.result } }
                }
            }
        }
    }

    private func adopt(_ id: String) async {
        guard !turns.contains(where: { $0.run.id == id }), let run = try? await api.run(id) else { return }
        guard !disposed, !turns.contains(where: { $0.run.id == id }) else { return }
        append(run)
    }

    private func append(_ run: Run) {
        turns.append(Turn(run: run))
        follow(run.id, wasLive: true)
    }

    // MARK: Actions

    /// Send a follow-up on the same session. While a message runs, it waits behind it. With another
    /// agent the thread switches to it.
    public func reply(_ text: String, permission: String?, images: [AttachedImage] = [], agent: (AgentID, String)? = nil, effort: String? = nil) async throws {
        guard let root else { return }
        var body: [String: JSONValue] = ["task": .string(text)]
        if let permission, !permission.isEmpty { body["permission_mode"] = .string(permission) }
        // "" goes back to the agent's default; absent keeps the thread's.
        if let effort { body["effort"] = .string(effort) }
        if let agent {
            body["agent"] = .string(agent.0.rawValue)
            body["model"] = .string(agent.1)
        }
        if !images.isEmpty { body["images"] = .array(images.map(\.payload)) }
        let id = try await api.sendMessage(root: root.id, body: .object(body))
        append(try await api.run(id))
    }

    /// Put the files back to how they were before `runID` started; the session carries on.
    public func restore(_ runID: String) async throws {
        append(try await api.run(try await api.restore(run: runID)))
    }

    /// Push the branch and open a pull request for it (or update the open one).
    public func openPR() async throws {
        guard let root else { return }
        append(try await api.run(try await api.openPR(root: root.id)))
    }

    /// Stop everything: drop the waiting messages first so none starts once the running one ends.
    public func stop() async throws {
        for run in runs.filter({ $0.status.isLive }).reversed() { try await api.stop(run: run.id) }
    }

    /// Send now: stop the running message so the waiting ones go next, in the same session.
    public func interrupt() async throws {
        for run in runs where run.status == .running { try await api.stop(run: run.id) }
    }

    /// Start a task scheduled for later right away.
    public func startNow(_ id: String) async throws {
        let run = try await api.startNow(run: id)
        update(id) { $0.run.startAt = run.startAt }
    }

    /// Drop one waiting message.
    public func cancel(_ id: String) async throws { try await api.stop(run: id) }

    /// Approve or deny a tool call the agent is waiting on. The event stream confirms it.
    public func decide(_ item: ApprovalItem, approve: Bool) async throws {
        let d = try await api.decide(run: item.runID, approval: item.id, approve: approve)
        update(item.runID) { $0.setStatus(item.id, status: d.status, decidedBy: d.decidedBy, message: d.message) }
    }
}

/// An image attached to a task or reply.
public struct AttachedImage: Sendable, Hashable, Identifiable {
    public let id: UUID
    public var name: String
    public var mediaType: String
    public var data: Data

    public init(name: String, mediaType: String, data: Data) {
        id = UUID()
        self.name = name
        self.mediaType = mediaType
        self.data = data
    }

    var payload: JSONValue { ["media_type": .string(mediaType), "data": .string(data.base64EncodedString())] }
}

/// What the control plane accepts (raster only: an SVG could run script), with its limits.
public enum Attachments {
    public static let types = ["image/png", "image/jpeg", "image/gif", "image/webp"]
    public static let maxCount = 8
    public static let maxBytes = 10 * 1024 * 1024
    public static let maxTotalBytes = 40 * 1024 * 1024

    /// Adds what fits and says why the rest didn't.
    public static func add(_ new: [AttachedImage], to items: inout [AttachedImage]) -> [String] {
        var skipped: [String] = []
        for image in new {
            let name = image.name.isEmpty ? nil : image.name
            if !types.contains(image.mediaType) {
                skipped.append("\(name ?? "That file") is not a PNG, JPEG, GIF or WebP image")
            } else if image.data.count > maxBytes {
                skipped.append("\(name ?? "That image") is over 10 MB")
            } else if items.count >= maxCount {
                skipped.append("At most \(maxCount) images per message")
            } else if items.reduce(image.data.count, { $0 + $1.data.count }) > maxTotalBytes {
                skipped.append("The images add up to more than 40 MB")
            } else {
                items.append(image)
            }
        }
        var seen = Set<String>()
        return skipped.filter { seen.insert($0).inserted }
    }
}
