import Foundation

// One message of a thread and everything it did: a port of `Turn` and `apply()` from
// web/src/lib/sf/thread.svelte.ts, wording included.

public struct ToolCall: Sendable, Hashable, Identifiable {
    public var key: String
    public var toolUseID: String
    public var name: String
    public var summary: String
    public var output: String?
    public var isError = false
    public var truncated = false
    public var images: [ImageRef] = []
    public var at: Date
    public var id: String { key }

    /// How the row starts: "Ran", "Read", "Used Foo".
    public var verb: String { Self.verbs[name] ?? "Used \(name)" }

    /// The first line worth reading from a failure.
    public var firstOutputLine: String {
        (output ?? "").split(separator: "\n").first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.map(String.init) ?? ""
    }

    static let verbs: [String: String] = [
        "Bash": "Ran", "Read": "Read", "Grep": "Searched", "Glob": "Listed", "Edit": "Edited", "Write": "Wrote",
        "MultiEdit": "Edited", "NotebookEdit": "Edited", "WebFetch": "Fetched", "WebSearch": "Searched the web for",
        "Agent": "Delegated", "Task": "Delegated", "TodoWrite": "Updated the plan",
    ]
}

public struct ApprovalItem: Sendable, Hashable, Identifiable {
    public var key: String
    public var id: String
    public var runID: String
    public var name: String
    public var summary: String
    public var input: JSONValue
    public var status: String
    public var message: String?
    public var decidedBy: String?
    public var expiresAt: Date?
    public var at: Date

    /// What the agent asks to do: "run a command", "use read from github".
    public var ask: String {
        switch name {
        case "Bash": return "run a command"
        case "Edit", "Write", "MultiEdit", "NotebookEdit": return "change a file"
        case "WebFetch": return "fetch a page"
        case "WebSearch": return "search the web"
        default:
            if name.hasPrefix("mcp__") {
                let rest = name.dropFirst(5)
                if let r = rest.range(of: "__") { return "use \(rest[r.upperBound...]) from \(rest[..<r.lowerBound])" }
            }
            return "use \(name)"
        }
    }

    /// The part worth reading in full: the command, file, address or query.
    public var main: String? {
        for key in ["command", "file_path", "url", "query"] {
            if let s = input[key]?.string { return s }
        }
        return nil
    }

    public var detail: String? { main == nil ? input.prettyPrinted() : nil }
    public var note: String? { input["description"]?.string ?? input["prompt"]?.string }

    public var outcome: String {
        switch status {
        case "approved": "Approved\(decidedBy.map { " by \($0)" } ?? "")"
        case "denied": "Denied\(decidedBy.map { " by \($0)" } ?? "")"
        case "expired": "Denied: nobody answered in time"
        case "cancelled": "Not answered: the run ended"
        default: "Waiting for approval"
        }
    }

    public func timeLeft(now: Date) -> String {
        let left = max(0, (expiresAt ?? now).timeIntervalSince(now))
        return left >= 60 ? "\(Int((left / 60).rounded(.up))) min left" : "\(Int(left.rounded(.up))) s left"
    }
}

public enum NoteTone: String, Sendable, Hashable { case muted, err, ok }

public struct Note: Sendable, Hashable, Identifiable {
    public var key: String
    public var tone: NoteTone
    public var text: String
    public var href: String?
    public var at: Date
    public var id: String { key }
}

public enum TurnItem: Sendable, Hashable, Identifiable {
    case text(key: String, text: String, at: Date)
    case tool(ToolCall)
    case approval(ApprovalItem)
    case note(Note)
    case stderr(key: String, text: String, at: Date)

    public var id: String {
        switch self {
        case .text(let key, _, _), .stderr(let key, _, _): key
        case .tool(let t): t.key
        case .approval(let a): a.key
        case .note(let n): n.key
        }
    }
}

/// Consecutive tool calls fold into one line, and consecutive stderr lines into one log, so neither
/// breaks up the narration.
public enum ItemGroup: Sendable, Hashable, Identifiable {
    case text(key: String, text: String)
    case tools(key: String, [ToolCall])
    case approval(ApprovalItem)
    case note(Note)
    case log(key: String, [String])

    public var id: String {
        switch self {
        case .text(let key, _), .tools(let key, _), .log(let key, _): key
        case .approval(let a): a.key
        case .note(let n): n.key
        }
    }
}

public enum ToolSummary {
    private static let kinds: [(match: @Sendable (String) -> Bool, verb: String, one: String, many: String)] = [
        ({ $0 == "Bash" }, "ran", "a command", "commands"),
        ({ $0 == "Read" }, "read", "a file", "files"),
        ({ ["Edit", "Write", "MultiEdit", "NotebookEdit"].contains($0) }, "edited", "a file", "files"),
        ({ $0 == "Grep" || $0 == "Glob" }, "searched", "once", "times"),
        ({ $0.hasPrefix("Web") }, "browsed", "a page", "pages"),
        ({ $0 == "TodoWrite" }, "updated", "the plan", "the plan"),
        ({ $0 == "Agent" || $0 == "Task" }, "delegated", "a task", "tasks"),
    ]

    /// "Ran 2 commands, read a file".
    public static func line(_ tools: [ToolCall]) -> String {
        var counts: [Int: Int] = [:]
        for t in tools {
            let k = kinds.firstIndex { $0.match(t.name) } ?? kinds.count
            counts[k, default: 0] += 1
        }
        let parts = counts.sorted { $0.key < $1.key }.map { k, n -> String in
            if k == kinds.count { return "used \(n == 1 ? "a tool" : "\(n) tools")" }
            let kind = kinds[k]
            return "\(kind.verb) \(n == 1 || kind.many == "the plan" ? kind.one : "\(n) \(kind.many)")"
        }
        let s = parts.joined(separator: ", ")
        return s.prefix(1).uppercased() + s.dropFirst()
    }

    /// Which symbol a tool's row shows.
    public enum Icon: Sendable { case terminal, file, search, edit, web, plan, tool }

    public static func icon(_ name: String) -> Icon {
        switch name {
        case "Bash": .terminal
        case "Read": .file
        case "Grep", "Glob": .search
        case "Edit", "Write", "MultiEdit", "NotebookEdit": .edit
        case "TodoWrite": .plan
        default: name.hasPrefix("Web") ? .web : .tool
        }
    }
}

/// Tokens in the agent's context window, and the window's size when known.
public struct ContextFill: Sendable, Hashable {
    public var used: Int
    public var limit: Int?

    public var percent: Int? { limit.map { min(100, Int((Double(used) / Double($0) * 100).rounded())) } }

    init?(_ json: JSONValue?) {
        guard let used = json?["used"]?.double, used > 0 else { return nil }
        self.used = Int(used)
        limit = json?["limit"]?.int.flatMap { $0 > 0 ? $0 : nil }
    }
}

public struct TurnResult: Sendable, Hashable {
    public var ok: Bool
    public var text: String
    public var subtype: String
    public var durationMs: Double?
    public var turns: Int?
    public var cost: Double?
    public var tokens: Int?
    public var context: ContextFill?
}

/// Something a turn's event asks of the thread around it.
public enum ThreadEffect: Sendable, Hashable {
    /// The machine dropped and the run was queued again elsewhere: follow the new run too.
    case adopt(runID: String)
    case pullRequest(PullRequest)
    /// What an integration did is in its result, not its events.
    case refetchRun
}

public struct Turn: Sendable, Hashable, Identifiable {
    public var run: Run
    public var items: [TurnItem] = []
    public var machine: String?
    public var branch: String?
    public var workspaceNote: String?
    public var startedFromRef: String?
    public var startedFromCommit: String?
    public var checkpoint: String?
    public var movedFromMachine: String?
    public var command: String?
    public var sessionModel: String?
    /// What the agent is doing right now, while it runs.
    public var activity: String?
    public var result: TurnResult?
    public var lastText = ""
    /// The text the agent is writing right now, until its final message replaces it. Never stored.
    public var liveText = ""
    public var reconnecting = false
    public var turnDiff: DiffSet?
    public var totalDiff: DiffSet?
    public var snapshot: (before: String, after: String)? {
        get { snapshotBefore.flatMap { b in snapshotAfter.map { (b, $0) } } }
        set {
            snapshotBefore = newValue?.before
            snapshotAfter = newValue?.after
        }
    }
    private var snapshotBefore: String?
    private var snapshotAfter: String?

    public var id: String { run.id }

    public init(run: Run) {
        self.run = run
        machine = run.workerID
        activity = run.status == .queued ? "queued" : nil
    }

    public var hasSnapshot: Bool { snapshotBefore != nil }

    public var groups: [ItemGroup] {
        var out: [ItemGroup] = []
        for item in items {
            switch item {
            case .text(let key, let text, _):
                out.append(.text(key: key, text: text))
            case .tool(let tool):
                if case .tools(let key, var list)? = out.last {
                    list.append(tool)
                    out[out.count - 1] = .tools(key: key, list)
                } else {
                    out.append(.tools(key: tool.key, [tool]))
                }
            case .approval(let a):
                out.append(.approval(a))
            case .note(let n):
                out.append(.note(n))
            case .stderr(let key, let text, _):
                if case .log(let first, var lines)? = out.last {
                    lines.append(text)
                    out[out.count - 1] = .log(key: first, lines)
                } else {
                    out.append(.log(key: key, [text]))
                }
            }
        }
        return out
    }

    public mutating func setStatus(_ approvalID: String, status: String, decidedBy: String?, message: String?) {
        for i in items.indices {
            if case .approval(var a) = items[i], a.id == approvalID {
                a.status = status
                a.decidedBy = decidedBy
                a.message = message
                items[i] = .approval(a)
            }
        }
    }

    private var hasPendingApproval: Bool {
        items.contains { if case .approval(let a) = $0 { a.status == "pending" } else { false } }
    }

    /// Apply one event from the run's stream.
    @discardableResult
    public mutating func apply(_ ev: RunEvent) -> [ThreadEffect] {
        let p = ev.payload
        let key = "\(run.id):\(ev.seq.map(String.init) ?? "live")"
        var effects: [ThreadEffect] = []

        switch ev.kind {
        case "status":
            guard let raw = p["status"]?.string else { break }
            let status = RunStatus(raw: raw)
            run.status = status
            if let movedTo = p["moved_to"]?.string {
                run.finishedAt = run.finishedAt ?? ev.at
                activity = nil
                let who = p["lost_worker"]?.string ?? machine ?? "The machine"
                items.append(.note(Note(key: key, tone: .muted, text: "\(who) stopped responding. Moving the task to another machine.", at: ev.at)))
                effects.append(.adopt(runID: movedTo))
                break
            }
            if status == .queued {
                activity = "queued"
                if p["started_early"]?.bool == true { run.startAt = nil }
                if p["moved_from"]?.string != nil { movedFromMachine = p["lost_worker"]?.string }
            } else if status == .running {
                run.startedAt = run.startedAt ?? ev.at
                run.workerID = p["worker_id"]?.string ?? run.workerID
                machine = run.workerID
                switch run.kind {
                case .restore: activity = "Restoring the files"
                case .pr: activity = "Pushing the branch and opening a pull request"
                case .integrate: activity = "Adding the changes to the room"
                case .agent: activity = "Starting \(Format.agentName(run.agent))"
                }
            } else {
                run.finishedAt = run.finishedAt ?? ev.at
                activity = nil
                if run.kind == .integrate, run.result == nil { effects.append(.refetchRun) }
                liveText = ""
                // Nobody can answer the agent any more.
                for i in items.indices {
                    if case .approval(var a) = items[i], a.status == "pending" {
                        a.status = "cancelled"
                        items[i] = .approval(a)
                    }
                }
                let reason = p["reason"]?.string
                if status != .succeeded || reason != nil {
                    let text = Self.endedNote(status: raw, reason: reason, machine: machine ?? run.workerID)
                    items.append(.note(Note(key: key, tone: status == .succeeded ? .ok : .err, text: text, at: ev.at)))
                }
            }
        case "checkpoint":
            if let sha = p["sha"]?.string {
                checkpoint = sha
            } else if let error = p["error"]?.string {
                items.append(.note(Note(key: key, tone: .err, text: "Could not push a checkpoint: \(error)", at: ev.at)))
            }
        case "workspace":
            if p["mode"]?.string == "worktree" {
                branch = p["branch"]?.string
                startedFromRef = p["from"]?["ref"]?.string
                startedFromCommit = p["from"]?["commit"]?.string
            } else {
                workspaceNote = p["note"]?.string ?? "in the checkout"
            }
        case "worktree_setup":
            // What the repo's `.sidefleet/worktree.json` brought into a new worktree.
            if p["state"]?.string == "running" {
                activity = "Setting up the worktree"
                break
            }
            let copied = (p["copied"]?.array ?? []).compactMap(\.string)
            let did = [copied.isEmpty ? nil : "copied \(copied.joined(separator: ", "))", (p["ran"]?.string).flatMap { $0.isEmpty ? nil : "ran \($0)" }]
                .compactMap { $0 }.joined(separator: " and ")
            if !did.isEmpty { items.append(.note(Note(key: key, tone: .muted, text: "Set up the worktree: \(did)", at: ev.at))) }
            for (i, problem) in (p["problems"]?.array ?? []).compactMap(\.string).enumerated() {
                items.append(.note(Note(key: "\(key)-\(i)", tone: .err, text: "Worktree setup: \(problem)", at: ev.at)))
            }
        case "started":
            command = p["command"]?.string
            activity = "Waiting for the first message"
        case "agent_init":
            sessionModel = p["model"]?.string
            run.sessionID = p["session_id"]?.string ?? run.sessionID
            activity = "Thinking"
        case "assistant_thinking":
            activity = "Thinking"
        case "assistant_live":
            liveText = p["text"]?.string ?? ""
        case "assistant_text":
            liveText = ""
            lastText = p["text"]?.string ?? ""
            items.append(.text(key: key, text: lastText, at: ev.at))
            activity = "Working"
        case "tool_use":
            liveText = ""
            let name = p["name"]?.string ?? "Tool"
            items.append(.tool(ToolCall(key: key, toolUseID: p["id"]?.string ?? "", name: name, summary: p["summary"]?.string ?? "", output: nil, at: ev.at)))
            var verb = ToolCall.verbs[name] ?? "Using \(name)"
            if verb.hasSuffix(" for") { verb.removeLast(4) }
            activity = verb + "…"
        case "tool_result":
            let id = p["tool_use_id"]?.string
            if let i = items.lastIndex(where: { if case .tool(let t) = $0 { t.toolUseID == id } else { false } }),
               case .tool(var tool) = items[i] {
                tool.output = p["content"]?.string ?? ""
                tool.isError = p["is_error"]?.bool ?? false
                tool.truncated = p["truncated"]?.bool ?? false
                tool.images = p["images"]?.decode([ImageRef].self) ?? []
                items[i] = .tool(tool)
            }
            activity = "Thinking"
        case "approval_requested":
            items.append(.approval(ApprovalItem(
                key: key, id: p["id"]?.string ?? "", runID: run.id, name: p["tool_name"]?.string ?? "tool",
                summary: p["summary"]?.string ?? "", input: p["input"] ?? .object([:]), status: p["status"]?.string ?? "pending",
                message: nil, decidedBy: nil, expiresAt: p["expires_at"]?.string.flatMap(Timestamp.parse), at: ev.at)))
            activity = "Waiting for you to approve"
        case "approval_decided":
            if let id = p["id"]?.string {
                setStatus(id, status: p["status"]?.string ?? "denied", decidedBy: p["decided_by"]?.string, message: p["message"]?.string)
            }
            if !hasPendingApproval { activity = "Thinking" }
        case "result":
            liveText = ""
            run.sessionID = p["session_id"]?.string ?? run.sessionID
            let text = p["result"]?.string ?? ""
            result = TurnResult(
                ok: !(p["is_error"]?.bool ?? false),
                text: !text.isEmpty && text != lastText ? text : "",
                subtype: p["subtype"]?.string ?? "",
                durationMs: p["duration_ms"]?.double,
                turns: p["num_turns"]?.int,
                cost: p["total_cost_usd"]?.double,
                tokens: Self.tokenCount(p["usage"]),
                context: ContextFill(p["context"]))
            activity = nil
        case "snapshot":
            if let before = p["before"]?.string, let after = p["after"]?.string { snapshot = (before, after) }
        case "restored":
            items.append(.note(Note(key: key, tone: .ok, text: "Restored the files", at: ev.at)))
            activity = nil
        case "pull_request":
            let number = p["number"]?.int ?? 0
            let url = p["url"]?.string
            items.append(.note(Note(key: key, tone: .ok, text: "\(p["opened"]?.bool == true ? "Opened" : "Pushed to") pull request #\(number)", href: url, at: ev.at)))
            activity = nil
            if let url { effects.append(.pullRequest(PullRequest(url: url, number: number, state: p["state"]?.string ?? "open"))) }
        case "diff":
            if let turnSet = p["turn"]?.decode(DiffSet.self) {
                turnDiff = turnSet
                totalDiff = p["total"]?.decode(DiffSet.self)
            }
        case "stop_requested":
            activity = "Stopping"
        case "stderr":
            items.append(.stderr(key: key, text: p["text"]?.string ?? "", at: ev.at))
        case "error":
            items.append(.note(Note(key: key, tone: .err, text: p["message"]?.string ?? "Error", at: ev.at)))
            activity = nil
        case "omitted":
            // The server refused part of the output (a firewall in front of it, most likely); the turn goes on.
            let status = p["status"]?.int.map { " (\($0))" } ?? ""
            items.append(.note(Note(key: key, tone: .muted, text: "Some of the agent's output isn't shown here: the server refused it\(status). A firewall in front of the server can mistake code or logs for an attack.", at: ev.at)))
        default:
            break
        }
        return effects
    }

    /// What the thread says when a turn ends on its own terms or the machine's.
    static func endedNote(status: String, reason: String?, machine: String?) -> String {
        let it = machine ?? "The machine"
        switch reason {
        case "worker unresponsive":
            return "Stopped. \(it) didn't act on Stop within a minute, so Sidefleet stopped the task here; the machine frees it the next time it checks in."
        case "worker offline":
            return "Stopped. \(it) was offline, so the task was stopped here."
        case "worker lost":
            return "Failed: \(it) stopped responding while the task ran."
        case "worker restarted":
            return status == "stopped" ? "Stopped. \(it) restarted while the task was stopping." : "Failed: \(it) restarted while the task ran."
        default:
            let head = status == "stopped" ? "Stopped" : status == "failed" ? "Failed" : "Done"
            return reason.map { "\(head): \($0)" } ?? head
        }
    }

    /// Input plus output tokens from Codex or opencode usage.
    private static func tokenCount(_ usage: JSONValue?) -> Int? {
        guard let usage else { return nil }
        let n = (usage["input"]?.double ?? 0) + (usage["output"]?.double ?? 0)
        return n > 0 ? Int(n) : nil
    }

    public static func == (a: Turn, b: Turn) -> Bool {
        a.run == b.run && a.items == b.items && a.liveText == b.liveText && a.activity == b.activity
            && a.result == b.result && a.turnDiff == b.turnDiff && a.totalDiff == b.totalDiff
            && a.machine == b.machine && a.branch == b.branch && a.reconnecting == b.reconnecting
            && a.snapshotBefore == b.snapshotBefore && a.checkpoint == b.checkpoint
    }

    public func hash(into h: inout Hasher) {
        h.combine(run.id)
        h.combine(items.count)
        h.combine(liveText)
    }
}
