import Foundation

// Rooms, invites and chat apps: the parts of web/src/lib/sf/types.ts a team uses.

/// A link that lets someone join an org.
public struct Invite: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var role: String
    public var createdByLogin: String?
    public var expiresAt: Date?
    public var maxUses: Int?
    public var uses: Int
    public var revokedAt: Date?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        role = f.opt("role") ?? "member"
        createdByLogin = f.opt("created_by_login")
        expiresAt = f.date("expires_at")
        maxUses = f.opt("max_uses", Double.self).map { Int($0) }
        uses = f.int("uses")
        revokedAt = f.date("revoked_at")
    }

    /// Still lets someone in: not revoked, not expired, not used up.
    public func isPending(now: Date) -> Bool {
        revokedAt == nil && (expiresAt ?? .distantPast) > now && (maxUses.map { uses < $0 } ?? true)
    }
}

public struct CreatedInvite: Sendable, Decodable {
    public var invite: Invite
    public var code: String
}

/// What an invite link says about itself (`GET /v1/invites/:code`, no sign-in needed).
public struct InviteInfo: Sendable, Hashable, Decodable {
    public var orgName: String
    public var inviterLogin: String?
    public var role: String
    public var valid: Bool
    public var reason: String?

    public init(orgName: String = "", inviterLogin: String? = nil, role: String = "member", valid: Bool, reason: String? = nil) {
        self.orgName = orgName
        self.inviterLogin = inviterLogin
        self.role = role
        self.valid = valid
        self.reason = reason
    }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        orgName = f.str("org_name")
        inviterLogin = f.opt("inviter_login")
        role = f.opt("role") ?? "member"
        valid = f.bool("valid")
        reason = f.opt("reason")
    }

    /// Why a link that no longer works doesn't.
    public var problem: String {
        switch reason {
        case "expired": "This invite has expired."
        case "revoked": "This invite was revoked."
        case "used_up": "This invite has already been used."
        case "not_found": "This invite link doesn't exist."
        default: "This invite no longer works."
        }
    }
}

public struct AcceptedInvite: Sendable, Decodable {
    public var org: Org
}

/// A shared goal split into tasks, each on its owner's machine, merged into one room branch.
public struct Room: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var name: String
    public var goal: String
    public var repo: String
    public var branch: String
    public var createdBy: String?
    public var createdByLogin: String?
    public var createdAt: Date?
    public var archivedAt: Date?
    /// First successful integration; new tasks then start from the room branch.
    public var integratedAt: Date?
    public var lastActivityAt: Date?
    public var taskCount: Int
    public var activeCount: Int
    public var integratedCount: Int

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        name = f.str("name")
        goal = f.str("goal")
        repo = f.str("repo")
        branch = f.str("branch")
        createdBy = f.opt("created_by")
        createdByLogin = f.opt("created_by_login")
        createdAt = f.date("created_at")
        archivedAt = f.date("archived_at")
        integratedAt = f.date("integrated_at")
        lastActivityAt = f.date("last_activity_at")
        taskCount = f.int("task_count")
        activeCount = f.int("active_count")
        integratedCount = f.int("integrated_count")
    }
}

public struct RoomNote: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var authorID: String?
    public var authorLogin: String?
    public var body: String
    public var createdAt: Date?
    public var editedAt: Date?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        authorID = f.opt("author_id")
        authorLogin = f.opt("author_login")
        body = f.str("body")
        createdAt = f.date("created_at")
        editedAt = f.date("edited_at")
    }
}

/// How adding a task's work to the room branch went.
public struct Integration: Sendable, Hashable, Decodable {
    public var status: String
    public var at: Date?
    public var commit: String?
    public var error: String?
    public var conflicts: [String]

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        status = f.str("status")
        at = f.date("at")
        commit = f.opt("commit")
        error = f.opt("error")
        conflicts = f.opt("conflicts") ?? []
    }

    public var adding: Bool { status == "queued" || status == "running" }
    public var done: Bool { status == "succeeded" }
    public var failed: Bool { status == "failed" || status == "stopped" }
}

/// A task in a room: its thread, who took it and where its work stands.
public struct TaskCard: Sendable, Hashable, Decodable, Identifiable {
    /// The thread's root run id.
    public var id: String
    public var title: String
    public var createdBy: String?
    public var createdByLogin: String?
    public var agent: AgentID
    public var model: String
    public var workerID: String?
    public var status: RunStatus
    public var lastActivityAt: Date?
    public var files: [String]
    public var additions: Int
    public var deletions: Int
    public var integration: Integration?
    public var pr: PullRequest?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        title = f.str("title")
        createdBy = f.opt("created_by")
        createdByLogin = f.opt("created_by_login")
        agent = f.opt("agent") ?? .claude
        model = f.str("model")
        workerID = f.opt("worker_id")
        status = f.opt("status") ?? .other("")
        lastActivityAt = f.date("last_activity_at")
        files = f.opt("files") ?? []
        additions = f.int("additions")
        deletions = f.int("deletions")
        integration = f.opt("integration")
        pr = f.opt("pr")
    }

    public var isLive: Bool { status.isLive }

    /// Live first, then what failed to merge, then what isn't in the room yet, then what is.
    public var order: Int {
        if isLive { return 0 }
        if integration?.status == "failed" { return 1 }
        return integration == nil ? 2 : 3
    }
}

public struct RoomDetail: Sendable, Hashable, Decodable {
    public var room: Room
    public var notes: [RoomNote]
    public var tasks: [TaskCard]

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        room = try f.req("room")
        notes = f.opt("notes") ?? []
        tasks = f.opt("tasks") ?? []
    }

    public var sortedTasks: [TaskCard] {
        tasks.enumerated().sorted { a, b in
            if a.element.order != b.element.order { return a.element.order < b.element.order }
            let (x, y) = (a.element.lastActivityAt ?? .distantPast, b.element.lastActivityAt ?? .distantPast)
            return x != y ? x > y : a.offset < b.offset
        }.map(\.element)
    }
}

/// Updates in chat: Sidefleet tells the app when tasks finish or need an OK.
public struct ChatUpdates: Sendable, Hashable, Decodable {
    public var kind: String
    public var url: String
    /// Every task, not only the ones this app started or replied to.
    public var allTasks: Bool
    public var lastAt: Date?
    /// Why the last update didn't arrive.
    public var lastError: String?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        kind = f.str("kind")
        url = f.str("url")
        allTasks = f.bool("all_tasks")
        lastAt = f.date("last_at")
        lastError = f.opt("last_error")
    }
}

/// A personal token for scripts and chat agents (Hermes, OpenClaw, MCP clients): acts as its user in one org.
public struct ApiToken: Sendable, Hashable, Decodable, Identifiable {
    public var id: String
    public var name: String
    public var scope: String
    public var prefix: String
    public var createdAt: Date?
    public var revokedAt: Date?
    public var lastUsedAt: Date?
    public var updates: ChatUpdates?
    /// Only in the response that created it.
    public var token: String?
    /// Only in the response that turned updates on.
    public var secret: String?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        id = try f.req("id")
        name = f.str("name")
        scope = f.opt("scope") ?? "read"
        prefix = f.str("prefix")
        createdAt = f.date("created_at")
        revokedAt = f.date("revoked_at")
        lastUsedAt = f.date("last_used_at")
        updates = f.opt("updates")
        token = f.opt("token")
        secret = f.opt("secret")
    }
}
