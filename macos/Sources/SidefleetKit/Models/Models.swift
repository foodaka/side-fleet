import Foundation

// Shapes returned by the Sidefleet control plane (crates/core/src/models.rs, web/src/lib/sf/types.ts).

public enum RunStatus: Sendable, Hashable, Decodable {
    case queued, running, succeeded, failed, stopped
    /// From a newer control plane.
    case other(String)

    public init(raw: String) {
        switch raw {
        case "queued": self = .queued
        case "running": self = .running
        case "succeeded": self = .succeeded
        case "failed": self = .failed
        case "stopped": self = .stopped
        default: self = .other(raw)
        }
    }

    public init(from decoder: Decoder) throws { self.init(raw: try decoder.singleValueContainer().decode(String.self)) }

    public var isTerminal: Bool { self != .queued && self != .running }
    public var isLive: Bool { !isTerminal }
    public var label: String {
        switch self {
        case .queued: "Queued"
        case .running: "Running"
        case .succeeded: "Done"
        case .failed: "Failed"
        case .stopped: "Stopped"
        case .other(let s): s
        }
    }
}

public enum AgentID: String, Sendable, Hashable, Decodable, CaseIterable, Identifiable {
    case claude, codex, opencode
    public var id: String { rawValue }

    /// Anything unknown reads as Claude Code, as the page does.
    public init(from decoder: Decoder) throws {
        self = AgentID(rawValue: (try? decoder.singleValueContainer().decode(String.self)) ?? "") ?? .claude
    }
}

public struct ImageRef: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var mediaType: String
    public var bytes: Int

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        mediaType = f.str("media_type")
        bytes = f.int("bytes")
    }
}

public struct PullRequest: Sendable, Hashable, Decodable {
    public var url: String
    public var number: Int
    public var state: String

    public init(url: String, number: Int, state: String) {
        self.url = url
        self.number = number
        self.state = state
    }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        url = f.str("url")
        number = f.int("number")
        state = f.opt("state") ?? "open"
    }
}

public enum RunKind: String, Sendable, Hashable {
    case agent, restore, pr, integrate
}

public struct Run: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var task: String
    public var agent: AgentID
    public var model: String
    public var effort: String?
    public var repo: String
    public var status: RunStatus
    public var workerID: String?
    public var stopRequested: Bool
    public var createdAt: Date
    public var startedAt: Date?
    public var finishedAt: Date?
    public var result: JSONValue?
    public var parentID: String?
    public var sessionID: String?
    public var pinnedWorkerID: String?
    public var permissionMode: String?
    public var isolation: String
    public var failover: Bool
    public var movedFrom: String?
    public var resumeFromBranch: String?
    public var images: [ImageRef]
    public var kind: RunKind
    public var forkedFrom: String?
    public var archivedAt: Date?
    public var snoozedUntil: Date?
    public var pr: PullRequest?
    public var createdBy: String?
    public var roomID: String?
    public var context: String?
    public var startAt: Date?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        task = f.str("task")
        agent = f.opt("agent") ?? .claude
        model = f.str("model")
        effort = f.opt("effort")
        repo = f.str("repo")
        status = f.opt("status") ?? .other("unknown")
        workerID = f.opt("worker_id")
        stopRequested = f.bool("stop_requested")
        createdAt = f.date("created_at") ?? Date(timeIntervalSince1970: 0)
        startedAt = f.date("started_at")
        finishedAt = f.date("finished_at")
        result = f.opt("result")
        parentID = f.opt("parent_id")
        sessionID = f.opt("session_id")
        pinnedWorkerID = f.opt("pinned_worker_id")
        permissionMode = f.opt("permission_mode")
        isolation = f.opt("isolation") ?? "worktree"
        failover = f.bool("failover")
        movedFrom = f.opt("moved_from")
        resumeFromBranch = f.opt("resume_from_branch")
        images = f.opt("images") ?? []
        kind = f.opt("kind", String.self).flatMap(RunKind.init(rawValue:)) ?? .agent
        forkedFrom = f.opt("forked_from")
        archivedAt = f.date("archived_at")
        snoozedUntil = f.date("snoozed_until")
        pr = f.opt("pr")
        createdBy = f.opt("created_by")
        roomID = f.opt("room_id")
        context = f.opt("context")
        startAt = f.date("start_at")
    }

    /// A queued run still waiting for its start time.
    public func isScheduled(now: Date) -> Bool {
        guard status == .queued, let startAt else { return false }
        return startAt > now
    }
}

/// A thread in the list: its first run plus what the whole thread is doing.
@dynamicMemberLookup
public struct RunSummary: Sendable, Hashable, Decodable, Identifiable {
    public var run: Run
    public var followUps: Int
    public var threadStatus: RunStatus
    public var lastActivityAt: Date
    public var pendingApprovals: Int
    public var latestAgent: AgentID
    public var latestModel: String
    public var latestEffort: String?
    public var latestWorkerID: String?

    public var id: String { run.id }

    public subscript<T>(dynamicMember path: WritableKeyPath<Run, T>) -> T {
        get { run[keyPath: path] }
        set { run[keyPath: path] = newValue }
    }

    public init(from decoder: Decoder) throws {
        run = try Run(from: decoder)
        let f = try Fields(decoder)
        followUps = f.int("follow_ups")
        threadStatus = f.opt("thread_status") ?? run.status
        lastActivityAt = f.date("last_activity_at") ?? run.createdAt
        pendingApprovals = f.int("pending_approvals")
        latestAgent = f.opt("latest_agent") ?? run.agent
        latestModel = f.opt("latest_model") ?? run.model
        latestEffort = f.opt("latest_effort") ?? run.effort
        latestWorkerID = f.opt("latest_worker_id") ?? run.workerID
    }

    public var isLive: Bool { threadStatus.isLive }
    public func isSnoozed(now: Date) -> Bool { (run.snoozedUntil ?? .distantPast) > now }
}

public struct RunEvent: Sendable, Hashable, Decodable {
    public var runID: String
    /// `assistant_live` frames aren't stored and have no sequence number.
    public var seq: Int?
    public var kind: String
    public var payload: JSONValue
    public var at: Date

    public init(runID: String, seq: Int?, kind: String, payload: JSONValue, at: Date) {
        self.runID = runID
        self.seq = seq
        self.kind = kind
        self.payload = payload
        self.at = at
    }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        runID = f.str("run_id")
        seq = f.opt("seq", Double.self).map { Int($0) }
        kind = f.str("kind")
        payload = f.opt("payload") ?? .object([:])
        at = f.date("at") ?? Date()
    }
}

public struct SystemStats: Sendable, Hashable, Decodable {
    public var cpuCores: Int
    public var cpuPercent: Double
    public var memTotalMB: Double
    public var memUsedMB: Double
    public var diskTotalGB: Double
    public var diskFreeGB: Double
    public var uptimeSecs: Int

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        cpuCores = f.int("cpu_cores")
        cpuPercent = f.opt("cpu_percent") ?? 0
        memTotalMB = f.opt("mem_total_mb") ?? 0
        memUsedMB = f.opt("mem_used_mb") ?? 0
        diskTotalGB = f.opt("disk_total_gb") ?? 0
        diskFreeGB = f.opt("disk_free_gb") ?? 0
        uptimeSecs = f.int("uptime_secs")
    }
}

public struct AgentCapability: Sendable, Hashable, Decodable {
    public var version: String
    public var signedIn: Bool?
    public var signInError: String?
    /// Codex and opencode: the models the machine offers.
    public var models: [String]
    /// Claude Code: what each alias runs here, `fable` to `claude-fable-5-2`. Empty from older
    /// workers, and when Claude Code wouldn't say.
    public var aliases: [String: String]
    public var modelsTotal: Int?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        version = f.str("version")
        signedIn = f.opt("signed_in")
        signInError = f.opt("sign_in_error")
        // One key, two shapes: a list for Codex and opencode, a map for Claude Code.
        models = f.opt("models") ?? []
        aliases = f.opt("models") ?? [:]
        modelsTotal = f.opt("models_total", Double.self).map { Int($0) }
    }
}

public struct AgentCapabilities: Sendable, Hashable, Decodable {
    public var claude: AgentCapability?
    public var codex: AgentCapability?
    public var opencode: AgentCapability?
    public var ollama: AgentCapability?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        claude = f.opt("claude")
        codex = f.opt("codex")
        opencode = f.opt("opencode")
        ollama = f.opt("ollama")
    }

    public subscript(agent: AgentID) -> AgentCapability? {
        switch agent {
        case .claude: claude
        case .codex: codex
        case .opencode: opencode
        }
    }
}

public struct RepoRef: Sendable, Hashable, Decodable {
    public var name: String
    public var path: String
    public var id: String?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        name = f.str("name")
        path = f.str("path")
        id = f.opt("id")
    }

    /// A repo's identity across machines; older workers only report the folder name.
    public var key: String { (id?.isEmpty ?? true) ? name : id! }
}

public struct Slots: Sendable, Hashable, Decodable {
    public var parallel: Int
    public var active: Int

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        parallel = f.int("parallel")
        active = f.int("active")
    }
}

public struct Capabilities: Sendable, Hashable, Decodable {
    public var agents: AgentCapabilities?
    public var repos: [RepoRef]
    public var hostname: String
    public var os: String
    public var version: String
    public var system: SystemStats?
    public var slots: Slots?
    /// This worker's build answers Clean up.
    public var canCleanWorktrees: Bool

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        agents = f.opt("agents")
        repos = f.opt("repos") ?? []
        hostname = f.str("hostname")
        os = f.str("os")
        version = f.str("version")
        system = f.opt("system")
        slots = f.opt("slots")
        canCleanWorktrees = f.bool("can_clean_worktrees")
    }
}

public struct Owner: Sendable, Hashable, Decodable {
    public var id: String
    public var login: String

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = f.str("id")
        login = f.str("login")
    }
}

public struct Worker: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var displayName: String
    public var capabilities: Capabilities
    public var lastHeartbeatAt: Date?
    public var online: Bool
    public var updateRequested: Bool
    public var outdated: Bool
    public var limitedUntil: [String: Date]
    public var owner: Owner?
    /// Absent on older control planes, which means yours.
    public var mine: Bool
    public var guest: Bool
    /// What it's been asked to do with its worktrees and hasn't reported back on: `preview` or `remove`.
    public var worktreeCleanupPending: String?
    /// What the last cleanup did.
    public var worktreeCleanup: WorktreeCleanupTotals?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        displayName = f.opt("display_name") ?? id
        worktreeCleanupPending = f.opt("worktree_cleanup_pending")
        worktreeCleanup = f.opt("worktree_cleanup")
        capabilities = try f.req("capabilities")
        lastHeartbeatAt = f.date("last_heartbeat_at")
        online = f.bool("online")
        updateRequested = f.bool("update_requested")
        outdated = f.bool("outdated")
        limitedUntil = (f.opt("limited_until", [String: String?].self) ?? [:]).reduce(into: [:]) { out, kv in
            if let s = kv.value, let d = Timestamp.parse(s) { out[kv.key] = d }
        }
        owner = f.opt("owner")
        mine = f.opt("mine") ?? true
        guest = f.bool("guest")
    }

    public func hasAgent(_ agent: AgentID) -> Bool { capabilities.agents?[agent] != nil }
    public func hasRepo(_ key: String) -> Bool { capabilities.repos.contains { $0.key == key } }
}

public struct MyMachine: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var displayName: String
    /// The org it connected to; it serves you in all of yours.
    public var orgID: String
    public var orgName: String
    public var online: Bool
    public var lastHeartbeatAt: Date?
    public var repos: [String]
    public var agents: [AgentID]

    /// Machine ids are only unique within an org.
    public var key: String { orgID + id }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        displayName = f.opt("display_name") ?? id
        orgID = f.opt("org", JSONValue.self)?["id"]?.string ?? ""
        orgName = f.opt("org", JSONValue.self)?["name"]?.string ?? ""
        online = f.bool("online")
        lastHeartbeatAt = f.date("last_heartbeat_at")
        repos = f.opt("repos") ?? []
        agents = f.opt("agents") ?? []
    }
}

public struct Member: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var login: String
    public var role: String

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        login = f.str("login")
        role = f.opt("role") ?? "member"
    }
}

public struct Setup: Sendable, Hashable, Decodable {
    public var controlURL: String
    public var workerToken: String
    public var version: String
    public var build: String?
    public var hostWorker: String?
    public var hosted: Bool

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        controlURL = f.str("control_url")
        workerToken = f.str("worker_token")
        version = f.str("version")
        build = f.opt("build")
        hostWorker = f.opt("host_worker")
        hosted = f.bool("hosted")
    }
}

public struct AuthMethods: Sendable, Hashable, Decodable {
    public var github: Bool
    public var devKey: Bool
    public var hosted: Bool

    public init(github: Bool, devKey: Bool, hosted: Bool) {
        self.github = github
        self.devKey = devKey
        self.hosted = hosted
    }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        github = f.bool("github")
        devKey = f.opt("dev_key") ?? true
        hosted = f.bool("hosted")
    }
}

public struct OrgLimits: Sendable, Hashable, Decodable {
    public var machines: Int
    public var parallelRuns: Int

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        machines = f.int("machines")
        parallelRuns = f.int("parallel_runs")
    }
}

public struct Org: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var name: String
    public var slug: String
    public var plan: String
    public var role: String
    public var limits: OrgLimits?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        name = f.str("name")
        slug = f.str("slug")
        plan = f.opt("plan") ?? "free"
        role = f.opt("role") ?? "member"
        limits = f.opt("limits")
    }
}

public struct User: Sendable, Hashable, Decodable {
    public var id: String
    public var login: String

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        login = f.str("login")
    }
}

public struct Me: Sendable, Hashable, Decodable {
    /// Nil with the developer key.
    public var user: User?
    public var orgs: [Org]

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        user = f.opt("user")
        orgs = f.opt("orgs") ?? []
    }
}

public struct WorkerToken: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var name: String
    public var prefix: String
    public var createdAt: Date?
    public var revokedAt: Date?
    public var lastUsedAt: Date?
    public var token: String?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        name = f.str("name")
        prefix = f.str("prefix")
        createdAt = f.date("created_at")
        revokedAt = f.date("revoked_at")
        lastUsedAt = f.date("last_used_at")
        token = f.opt("token")
    }
}

public struct Enrollment: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var code: String
    public var machineName: String
    public var hostname: String
    public var os: String
    public var repoCount: Int
    public var status: String
    public var expiresAt: Date?
    public var createdAt: Date?
    public var workerID: String
    /// The Sidefleet build asking to join.
    public var version: String
    /// Claude Code on that machine, and whether it's signed in.
    public var claude: AgentCapability?

    private struct Agents: Decodable {
        var agents: AgentCapabilities?

        init(from decoder: Decoder) throws {
            agents = (try Fields(decoder)).opt("agents")
        }
    }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        code = f.str("code")
        machineName = f.str("machine_name")
        createdAt = f.date("created_at")
        workerID = f.str("worker_id")
        claude = f.opt("capabilities", Agents.self)?.agents?.claude
        let caps: JSONValue? = f.opt("capabilities")
        version = caps?["version"]?.string ?? ""
        hostname = caps?["hostname"]?.string ?? ""
        os = caps?["os"]?.string ?? ""
        repoCount = caps?["repo_count"]?.int ?? 0
        status = f.opt("status") ?? "pending"
        expiresAt = f.date("expires_at")
    }
}

/// Why a task waits on you (crates/server/src/attention.rs).
public struct InboxItem: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var runID: String
    public var title: String
    public var repo: String
    public var agent: AgentID
    /// Unknown reasons may appear from newer control planes; show their label.
    public var reason: String
    public var label: String
    public var detail: String?
    public var since: Date

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        runID = f.opt("run_id") ?? id
        title = f.str("title")
        repo = f.str("repo")
        agent = f.opt("agent") ?? .claude
        reason = f.str("reason")
        label = f.str("label")
        detail = f.opt("detail")
        since = f.date("since") ?? Date()
    }
}

struct InboxPayload: Decodable {
    var items: [InboxItem]

    init(from decoder: Decoder) throws {
        items = (try Fields(decoder)).opt("items") ?? []
    }
}

struct CreatedRun: Decodable {
    var id: String

    init(from decoder: Decoder) throws {
        id = try (try Fields(decoder)).req("id")
    }
}

struct AddedRepos: Decodable {
    var added: [String]

    init(from decoder: Decoder) throws {
        added = (try Fields(decoder)).opt("added") ?? []
    }
}

public struct ApprovalDecision: Sendable, Hashable, Decodable {
    public var status: String
    public var decidedBy: String?
    public var message: String?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        status = f.str("status")
        decidedBy = f.opt("decided_by")
        message = f.opt("message")
    }
}
