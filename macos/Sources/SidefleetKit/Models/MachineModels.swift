import Foundation

// Cleaning up a machine's worktrees (crates/core/src/models.rs, web/src/lib/sf/types.ts).

/// A task's worktree on a machine, and why a cleanup would remove or keep it.
public struct WorktreeItem: Sendable, Hashable, Decodable, Identifiable {
    /// The thread's first run.
    public var rootID: String
    public var repo: String
    public var branch: String
    /// `merged`, `on_default`, `in_use`, `changes`, `unmerged`, `other_branch` or `failed`.
    public var state: String
    public var bytes: Double
    /// The thread's first message, when this org has it.
    public var task: String?

    public var id: String { rootID }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        rootID = try f.req("root_id")
        repo = f.str("repo")
        branch = f.str("branch")
        state = f.str("state")
        bytes = f.opt("bytes") ?? 0
        task = f.opt("task")
    }

    /// Only these go: everything else has work that isn't anywhere else, or couldn't be checked.
    public var removable: Bool { state == "merged" || state == "on_default" }
    public var title: String { task ?? branch }

    /// Why it goes or stays. A state from a newer worker reads as itself.
    public var reason: String {
        switch state {
        case "merged": "Pull request merged"
        case "on_default": "Nothing beyond the default branch"
        case "in_use": "In use"
        case "changes": "Uncommitted files"
        case "unmerged": "Unmerged commits"
        case "other_branch": "On another branch"
        case "failed": "Could not check"
        default: state
        }
    }
}

/// What the last cleanup did on a machine. Everyone in the org sees these totals.
public struct WorktreeCleanupTotals: Sendable, Hashable, Decodable {
    public var removed: Int
    public var freedBytes: Double
    public var keptInUse: Int
    public var keptChanges: Int
    public var keptUnmerged: Int
    public var failed: Int
    public var at: Date?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        removed = f.int("removed")
        freedBytes = f.opt("freed_bytes") ?? 0
        keptInUse = f.int("kept_in_use")
        keptChanges = f.int("kept_changes")
        keptUnmerged = f.int("kept_unmerged")
        failed = f.int("failed")
        at = f.date("at")
    }

    /// Worktrees that were listed for removal but had gained work, or couldn't be removed.
    public var kept: Int { keptInUse + keptChanges + keptUnmerged + failed }

    /// The line under a machine's agents: what the last cleanup did.
    public func note(now: Date) -> String {
        "Cleaned up \(Format.rel(at, now: now)): removed \(removed), freed \(Format.fileSize(freedBytes))" + (kept > 0 ? ", kept \(kept) that changed" : "")
    }
}

/// `GET /v1/workers/:id/clean-worktrees`: only the machine's owner sees the lists.
public struct WorktreeCleanupStatus: Sendable, Hashable, Decodable {
    public struct Preview: Sendable, Hashable, Decodable {
        public var items: [WorktreeItem]
        public var at: Date?

        public init(from decoder: Decoder) throws {
            let f = try Fields(decoder)
            items = f.opt("items") ?? []
            at = f.date("at")
        }
    }

    public struct Cleanup: Sendable, Hashable, Decodable {
        public var totals: WorktreeCleanupTotals
        /// The worktrees removed.
        public var items: [WorktreeItem]

        public init(from decoder: Decoder) throws {
            totals = try WorktreeCleanupTotals(from: decoder)
            items = (try Fields(decoder)).opt("items") ?? []
        }
    }

    /// `preview` or `remove` while the machine hasn't reported back.
    public var pending: String?
    public var preview: Preview?
    public var cleanup: Cleanup?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        pending = f.opt("pending")
        preview = f.opt("preview")
        cleanup = f.opt("cleanup")
    }
}

extension Array where Element == WorktreeItem {
    /// What a cleanup removes, largest first.
    public var going: [WorktreeItem] { filter(\.removable).sorted { $0.bytes > $1.bytes } }
    /// What it leaves alone, largest first.
    public var staying: [WorktreeItem] { filter { !$0.removable }.sorted { $0.bytes > $1.bytes } }
}
