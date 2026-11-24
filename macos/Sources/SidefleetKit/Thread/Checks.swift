import Foundation

// The repo's checks (`.sidefleet.json`) after the agent's turn: a port of `ChecksItem` and the
// `checks` event in web/src/lib/sf/thread.svelte.ts, `ChecksCard.svelte`'s wording, and
// `ChecksSummary` in web/src/lib/sf/types.ts.

/// One check, run in the message's worktree.
public struct CheckResult: Sendable, Hashable, Identifiable {
    public var name: String
    public var run: String
    public var passed: Bool
    public var exitCode: Int?
    public var timedOut: Bool
    public var durationMs: Double
    /// The end of what it printed.
    public var output: String
    public var id: String { name }

    init?(_ json: JSONValue) {
        guard let name = json["name"]?.string else { return nil }
        self.name = name
        run = json["run"]?.string ?? ""
        passed = json["passed"]?.bool ?? false
        exitCode = json["exit_code"]?.int
        timedOut = json["timed_out"]?.bool ?? false
        durationMs = json["duration_ms"]?.double ?? 0
        output = json["output"]?.string ?? ""
    }

    /// How it ended, on its row: `12s`, `timed out`, `exit 101`, `killed`.
    public var how: String {
        if passed { return Format.duration(start: Date(timeIntervalSince1970: 0), end: Date(timeIntervalSince1970: durationMs / 1000), now: Date()) }
        if timedOut { return "timed out" }
        return exitCode.map { "exit \($0)" } ?? "killed"
    }
}

public enum ChecksVerdict: String, Sendable, Hashable, Decodable {
    case passed, failed, skipped, error
}

/// How the repo's checks went after a turn.
public struct ChecksItem: Sendable, Hashable, Identifiable {
    public var key: String
    public var at: Date
    public var verdict: ChecksVerdict
    public var attempt: Int
    /// The agent was asked to fix the failure, and its fix follows.
    public var retrying: Bool
    /// Why they didn't run (skipped), or what is wrong with the list (error).
    public var reason: String?
    public var checks: [CheckResult]
    public var id: String { key }

    init?(key: String, at: Date, payload p: JSONValue) {
        guard let verdict = p["verdict"]?.string.flatMap(ChecksVerdict.init(rawValue:)) else { return nil }
        self.key = key
        self.at = at
        self.verdict = verdict
        attempt = p["attempt"]?.int ?? 1
        retrying = p["retrying"]?.bool ?? false
        reason = p["reason"]?.string
        checks = (p["checks"]?.array ?? []).compactMap(CheckResult.init)
    }

    public var failed: CheckResult? { checks.first { !$0.passed } }

    /// The card's first line.
    public func headline(agent: AgentID) -> String {
        switch verdict {
        case .passed:
            return checks.count == 1 ? "Check passed: \(checks[0].name)" : "All \(checks.count) checks passed"
        case .failed:
            return "Check failed: \(failed?.name ?? "unknown")\(retrying ? " · asked \(Format.agentName(agent)) to fix it" : "")"
        case .skipped:
            return "Checks not run: \(reason ?? "not for this task")"
        case .error:
            return "Checks not run: \(reason ?? "the check list could not be used")"
        }
    }

    /// Under the checks, when the agent's fix didn't make them pass.
    public func stillFailing(agent: AgentID) -> String? {
        guard verdict == .failed, !retrying, attempt > 1 else { return nil }
        return "Still failing after \(Format.agentName(agent))'s fix. Reply to have it try again, or take it from here."
    }
}

/// How a message's checks went, as kept in its result (`result.checks`), for the run list.
public struct ChecksSummary: Sendable, Hashable, Decodable {
    public var verdict: ChecksVerdict
    public var attempts: Int
    public var passed: Int
    public var total: Int
    public var failed: [String]

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        verdict = try f.req("verdict")
        attempts = f.int("attempts")
        passed = f.int("passed")
        total = f.int("total")
        failed = f.opt("failed") ?? []
    }
}
