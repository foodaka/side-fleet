import Foundation
import Observation

/// A message you just sent that the control plane hasn't taken yet, or couldn't. It shows at once, in
/// place of its turn.
public struct Outgoing: Sendable, Hashable, Identifiable {
    public enum Status: Sendable, Hashable { case sending, failed }
    public let id: UUID
    public var text: String
    public var status: Status
    public var error: String?
}

/// A conversation: the root run plus its follow-ups, each streamed into a turn. A port of `Thread` from
/// web/src/lib/sf/thread.svelte.ts.
@MainActor
@Observable
public final class ThreadStore {
    public private(set) var turns: [Turn] = []
    public private(set) var error: String?
    public private(set) var loaded = false
    /// Messages on their way: shown straight away, then replaced by their turn once the server has them.
    public private(set) var outbox: [Outgoing] = []

    /// A run that was live just ended; lists and machine slots have changed.
    @ObservationIgnored public var onRunEnded: (@MainActor () -> Void)?
    /// A message went through: the list has a new message in this thread.
    @ObservationIgnored public var onSent: (@MainActor () -> Void)?

    @ObservationIgnored private let api: API
    @ObservationIgnored private var tasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var disposed = false
    @ObservationIgnored private var pendingLive: [String: RunEvent] = [:]
    @ObservationIgnored private var flushing = false
    @ObservationIgnored private let liveInterval: Duration
    @ObservationIgnored private let retryAfter: Duration
    @ObservationIgnored private var bodies: [UUID: JSONValue] = [:]
    /// The id the server gave a message that went through, if loading its run failed: a retry must not send it twice.
    @ObservationIgnored private var sent: [UUID: String] = [:]
    /// Messages go out one at a time, so they queue in the order they were sent.
    @ObservationIgnored private var lastSend: Task<Void, Never>?

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
        guard !turns.contains(where: { $0.run.id == run.id }) else { return }
        turns.append(Turn(run: run))
        follow(run.id, wasLive: true)
    }

    // MARK: Actions

    /// Send a follow-up on the same session. While a message runs, it waits behind it. With another
    /// agent the thread switches to it. Returns at once: the message sits in `outbox` until the server
    /// has it, or says why it couldn't.
    @discardableResult
    public func reply(_ text: String, permission: String?, attachments: [Attachment] = [], agent: (AgentID, String)? = nil, effort: String? = nil) -> UUID {
        var body: [String: JSONValue] = ["task": .string(text)]
        if let permission, !permission.isEmpty { body["permission_mode"] = .string(permission) }
        // "" goes back to the agent's default; absent keeps the thread's.
        if let effort { body["effort"] = .string(effort) }
        if let agent {
            body["agent"] = .string(agent.0.rawValue)
            body["model"] = .string(agent.1)
        }
        body.merge(Attachments.body(attachments)) { $1 }
        let key = UUID()
        bodies[key] = .object(body)
        outbox.append(Outgoing(id: key, text: text, status: .sending, error: nil))
        deliver(key)
        return key
    }

    /// Try a message that failed again.
    public func retry(_ key: UUID) {
        guard let i = outbox.firstIndex(where: { $0.id == key }), outbox[i].status == .failed else { return }
        outbox[i].status = .sending
        outbox[i].error = nil
        deliver(key)
    }

    /// Drop a message that failed. Returns its text, so it can go back in the box.
    @discardableResult
    public func discard(_ key: UUID) -> String? {
        let text = outbox.first { $0.id == key }?.text
        outbox.removeAll { $0.id == key }
        bodies[key] = nil
        sent[key] = nil
        return text
    }

    /// Waits until every message sent so far has gone through or failed.
    public func delivered() async {
        await lastSend?.value
    }

    private func deliver(_ key: UUID) {
        let previous = lastSend
        lastSend = Task { [weak self] in
            await previous?.value
            await self?.send(key)
        }
    }

    private func send(_ key: UUID) async {
        guard !disposed, let root, let body = bodies[key] else { return }
        do {
            let id: String
            if let done = sent[key] {
                id = done
            } else {
                id = try await api.sendMessage(root: root.id, body: body)
                sent[key] = id
            }
            let run = try await api.run(id)
            guard !disposed else { return }
            // Swap in one go, so the bubble never blinks out or shows twice.
            append(run)
            outbox.removeAll { $0.id == key }
            bodies[key] = nil
            sent[key] = nil
            onSent?()
        } catch {
            guard let i = outbox.firstIndex(where: { $0.id == key }) else { return }
            outbox[i].status = .failed
            outbox[i].error = error.localizedDescription
        }
    }

    /// Put the files back to how they were before `runID` started; the session carries on.
    public func restore(_ runID: String) async throws {
        append(try await api.run(try await api.restore(run: runID)))
    }

    /// Merge the thread's branch into its room's branch.
    public func integrate() async throws {
        guard let root else { return }
        append(try await api.run(try await api.integrate(run: root.id)))
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

/// An image or file attached to a task or reply. Images (PNG, JPEG, GIF, WebP) go to the agent as
/// images; anything else (markdown, text, code, PDFs) as a file it reads.
public struct Attachment: Sendable, Hashable, Identifiable {
    public enum Kind: Sendable, Hashable { case image, file }

    public let id: UUID
    public var name: String
    public var mediaType: String
    public var data: Data
    public var kind: Kind
    /// Its size. A file too big to send is never read, so its data stays empty and this says how big it is.
    public var bytes: Int

    /// An image when its type is one the agent takes as an image, else a file.
    public init(name: String, mediaType: String, data: Data, kind: Kind? = nil, bytes: Int? = nil) {
        id = UUID()
        self.name = name
        self.mediaType = mediaType
        self.data = data
        self.kind = kind ?? (Attachments.imageTypes.contains(mediaType) ? .image : .file)
        self.bytes = bytes ?? data.count
    }

    /// One entry of the request body's `images` or `files`.
    public var payload: JSONValue {
        switch kind {
        case .image:
            ["media_type": .string(mediaType), "data": .string(data.base64EncodedString())]
        case .file:
            ["name": .string(name.isEmpty ? "file" : name), "media_type": .string(mediaType), "data": .string(data.base64EncodedString())]
        }
    }
}

/// What a message may carry: a port of web/src/lib/sf/attach.svelte.ts.
public enum Attachments {
    /// Sent as images (raster only: an SVG could run script, so it goes as a file).
    public static let imageTypes = ["image/png", "image/jpeg", "image/gif", "image/webp"]
    public static let maxImages = 8
    public static let maxFiles = 8
    public static let maxBytes = 10 * 1024 * 1024
    /// Everything a message carries together, well under the request limit once base64.
    public static let maxTotalBytes = 40 * 1024 * 1024

    /// Adds what fits and says why the rest didn't.
    public static func add(_ new: [Attachment], to items: inout [Attachment]) -> [String] {
        var skipped: [String] = []
        for a in new {
            let count = items.filter { $0.kind == a.kind }.count
            if a.bytes > maxBytes {
                skipped.append("\(a.name.isEmpty ? "That file" : a.name) is over 10 MB")
            } else if a.kind == .image, count >= maxImages {
                skipped.append("At most \(maxImages) images per message")
            } else if a.kind == .file, count >= maxFiles {
                skipped.append("At most \(maxFiles) files per message")
            } else if items.reduce(a.bytes, { $0 + $1.bytes }) > maxTotalBytes {
                skipped.append("The attachments add up to more than 40 MB")
            } else {
                items.append(a)
            }
        }
        var seen = Set<String>()
        return skipped.filter { seen.insert($0).inserted }
    }

    /// The request body's `images` and `files`, each only when there are any.
    public static func body(_ items: [Attachment]) -> [String: JSONValue] {
        var out: [String: JSONValue] = [:]
        let images = items.filter { $0.kind == .image }, files = items.filter { $0.kind == .file }
        if !images.isEmpty { out["images"] = .array(images.map(\.payload)) }
        if !files.isEmpty { out["files"] = .array(files.map(\.payload)) }
        return out
    }

    /// `12 KB`, `3.4 MB`, as the page writes a file's size.
    public static func size(_ bytes: Int) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        if bytes < 1024 * 1024 { return "\(Int((Double(bytes) / 1024).rounded())) KB" }
        return String(format: "%.1f MB", Double(bytes) / 1024 / 1024)
    }
}
