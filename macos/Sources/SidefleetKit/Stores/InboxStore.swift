import Foundation
import Observation

/// The "waiting on you" inbox. The control plane keeps what you've seen, so it matches every device.
@MainActor
@Observable
public final class InboxStore {
    public private(set) var items: [InboxItem] = []
    public private(set) var loaded = false

    @ObservationIgnored public var onChange: (@MainActor ([InboxItem]) -> Void)?
    @ObservationIgnored private let api: API
    @ObservationIgnored private var task: Task<Void, Never>?

    public init(api: API) { self.api = api }

    /// By the task's id or its latest run's.
    public func item(for id: String) -> InboxItem? { items.first { $0.id == id || $0.runID == id } }

    /// Follow the inbox of whoever is signed in, in the current org. Call again after switching org.
    public func connect() {
        task?.cancel()
        items = []
        loaded = false
        let api = api
        task = Task { [weak self] in
            await api.followInbox { items in
                self?.items = items
                self?.loaded = true
                self?.onChange?(items)
            }
        }
    }

    public func disconnect() {
        task?.cancel()
        task = nil
        items = []
    }

    /// Seen: out of your inbox until it needs you again. Any run id of the task works.
    public func dismiss(_ id: String) async {
        let item = item(for: id)
        if let item { items.removeAll { $0 == item } }
        // The stream puts it back if it did not stick.
        try? await api.dismissInbox(item?.id ?? id)
    }

    public func clearAll() async {
        items = []
        try? await api.dismissAllInbox()
    }

    public enum Tone: Sendable { case ok, queue, run, err }

    /// Colour of a reason's marker.
    public nonisolated static func tone(_ reason: String) -> Tone {
        switch reason {
        case "finished": .ok
        case "usage_limit": .queue
        case "approval_needed": .run
        default: .err
        }
    }

    /// "resets <time>" from a usage limit reads better as a local time.
    public nonisolated static func detail(_ text: String?) -> String {
        guard let text else { return "" }
        if text.hasPrefix("resets "), let date = Timestamp.parse(String(text.dropFirst(7))) {
            return "resets " + Format.startTime(date, now: Date())
        }
        return text
    }
}

/// Which notifications to post. Only title and reason, never agent output, as the page's web push.
public struct NotificationPlanner: Sendable {
    public struct Post: Sendable, Hashable {
        public var threadID: String
        public var title: String
        public var body: String
    }

    private var known: Set<String>?
    private var approvals: [String: Int]?

    public init() {}

    private static func firstLine(_ s: String) -> String {
        String(s.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: true).first ?? "").trimmingCharacters(in: .whitespaces)
    }

    /// What has newly started waiting. The first look only remembers, so launching doesn't set off a burst.
    public mutating func inboxChanged(_ items: [InboxItem], visible: String?) -> [Post] {
        let keys = Set(items.map { "\($0.id)|\(Timestamp.format($0.since))" })
        defer { known = keys }
        guard let known else { return [] }
        return items.filter { !known.contains("\($0.id)|\(Timestamp.format($0.since))") && $0.id != visible }
            .map { Post(threadID: $0.id, title: Self.firstLine($0.title), body: $0.label) }
    }

    /// Tool calls newly asking for your OK: the inbox doesn't carry these yet.
    public mutating func runsChanged(_ runs: [RunSummary], visible: String?) -> [Post] {
        let counts = runs.reduce(into: [String: Int]()) { $0[$1.id] = $1.isLive ? $1.pendingApprovals : 0 }
        defer { approvals = counts }
        guard let approvals else { return [] }
        return runs.filter { (counts[$0.id] ?? 0) > (approvals[$0.id] ?? 0) && $0.id != visible }
            .map { Post(threadID: $0.id, title: Self.firstLine($0.run.task), body: "Waiting for your approval") }
    }
}
