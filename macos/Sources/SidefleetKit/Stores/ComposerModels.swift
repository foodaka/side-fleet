import Foundation
import Observation

/// A piece of the sentence under the composer: plain, or a word that stands out, or a warning.
public struct HintPart: Sendable, Hashable {
    public enum Tone: Sendable { case plain, strong, warning }
    public var text: String
    public var tone: Tone = .plain

    public init(_ text: String, _ tone: Tone = .plain) {
        self.text = text
        self.tone = tone
    }
}

/// The new-task box's state and rules: a port of the logic in web/src/routes/+page.svelte.
@MainActor
@Observable
public final class NewTaskModel {
    public var task = ""
    public private(set) var agent: AgentID = .claude
    public private(set) var model = "sonnet"
    public private(set) var effort = ""
    public private(set) var machine = ""
    public var perm: String
    public var iso: String
    public private(set) var repo = ""
    /// Off by default and not remembered: it pushes the task's branch to origin.
    public var failover = false
    /// Not remembered: each task picks its own.
    public var startAt: Date?
    public var busy = false
    public var images: [AttachedImage] = []

    @ObservationIgnored private let fleet: FleetStore
    @ObservationIgnored private let prefs: PrefsStore
    @ObservationIgnored private let defaults: UserDefaults

    public init(fleet: FleetStore, prefs: PrefsStore, defaults: UserDefaults = .standard) {
        self.fleet = fleet
        self.prefs = prefs
        self.defaults = defaults
        perm = prefs.permission
        iso = prefs.workspace
        if let a = defaults.string(forKey: "sidefleet_agent").flatMap(AgentID.init(rawValue:)) { agent = a }
        model = initialModel(agent)
        effort = initialEffort(agent)
        machine = defaults.string(forKey: "sidefleet_machine") ?? ""
        repo = defaults.string(forKey: "sidefleet_repo") ?? ""
    }

    private func modelKey(_ a: AgentID) -> String { a == .claude ? "sidefleet_model" : "sidefleet_model_\(a.rawValue)" }
    private func effortKey(_ a: AgentID) -> String { "sidefleet_effort_\(a.rawValue)" }

    private func initialModel(_ a: AgentID) -> String {
        let m = defaults.string(forKey: modelKey(a)) ?? ""
        guard a == .claude else { return m }
        // A remembered full id (`claude-opus-5-5`) becomes its alias.
        return Format.models.first { m == $0.id || m.hasPrefix("claude-\($0.id)-") }?.id ?? Format.defaultModel(a)
    }

    private func initialEffort(_ a: AgentID) -> String {
        let e = defaults.string(forKey: effortKey(a)) ?? ""
        return Format.effortValid(agent: a, effort: e) ? e : ""
    }

    // MARK: Derived

    public var repos: [String] { fleet.composerRepos }
    /// Machines only run their owner's tasks, so the composer offers your own.
    public var onlineRepos: Set<String> { Set(fleet.myOnline.flatMap { $0.capabilities.repos.map(\.key) }) }
    public var withRepo: [Worker] { repo.isEmpty ? [] : fleet.myOnline.filter { $0.hasRepo(repo) } }
    public var machines: [Worker] { withRepo.filter { $0.hasAgent(agent) } }
    /// The remembered machine applies only where it has the repo and agent; otherwise any machine.
    public var pinned: Worker? { machines.first { $0.id == machine } }
    public var target: Worker? { pinned ?? machines.first }
    public var models: [String] { Format.machineModels(pinned.map { [$0] } ?? machines, agent: agent) }
    public var modelOK: Bool { Format.modelValid(agent: agent, model: model) }
    public var effectivePerm: String { Format.perms(for: agent).contains { $0.id == perm } ? perm : "acceptEdits" }
    public var canRun: Bool {
        !task.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !repo.isEmpty && !machines.isEmpty && modelOK && !busy
    }
    /// Live load of the machine the task lands on; only when there is one, not "whichever is free first".
    public var stats: SystemStats? { pinned != nil || machines.count == 1 ? target?.capabilities.system : nil }

    /// Why the chosen machine may fail the task, if it can tell.
    public var warning: String? {
        guard let a = target?.capabilities.agents else { return nil }
        if agent == .claude, let c = a.claude, c.signedIn != true { return "where Claude Code is not signed in" }
        if agent == .codex, a.codex?.signedIn == false { return "where Codex is not signed in (run `codex login` there)" }
        if agent == .opencode, let o = a.opencode, (o.modelsTotal ?? 0) == 0 { return "where opencode has no provider set up (run `opencode auth login` there)" }
        return nil
    }

    public func unavailable(_ a: AgentID) -> String? {
        !repo.isEmpty && !withRepo.contains(where: { $0.hasAgent(a) }) ? "Not on a machine with \(Format.repoLabel(repo))" : nil
    }

    /// Lines of the sentence under the box: what will happen, exactly.
    public func explanation(now: Date) -> [[HintPart]] {
        guard !repos.isEmpty else { return [] }
        if !repo.isEmpty, onlineRepos.contains(repo), target == nil {
            return [[HintPart("None of the machines with \(Format.repoLabel(repo)) has \(Format.agentName(agent)). Install it there, or pick another agent.")]]
        }
        guard let target else { return [] }
        var first: [HintPart] = []
        let name = Format.agentName(agent)
        if pinned == nil, machines.count > 1 {
            first.append(HintPart("Runs with \(name) on whichever of the \(machines.count) machines with this repo is free first"))
        } else {
            first.append(HintPart("Runs with \(name) on "))
            first.append(HintPart(target.displayName, .strong))
            if let warning {
                first.append(HintPart(", "))
                first.append(HintPart(warning, .warning))
            }
        }
        var tail = effectivePerm == "bypassPermissions" ? " with full access" : ""
        tail += iso == "worktree" ? ", on a new branch so your checkout stays untouched." : ", directly in the checkout."
        first.append(HintPart(tail))

        var lines = [first]
        if let startAt {
            lines.append([
                HintPart("Starts "),
                HintPart(Format.startTime(startAt, now: now), .strong),
                HintPart("; until then no machine picks it up, and you can start it early or cancel it from its page."),
            ])
        }
        if iso == "worktree", failover {
            lines.append([HintPart("If that machine drops, the task moves to another one; its branch is pushed to origin as it goes.")])
        }
        return lines
    }

    // MARK: Changes

    /// Keep the selection valid as machines come and go.
    public func syncSelection() {
        if !(repos.contains(repo) && onlineRepos.contains(repo)) {
            // A repo remembered by folder name (before identities): switch to its identity.
            let renamed = fleet.workers.flatMap(\.capabilities.repos).first { $0.name == repo && $0.key != repo }
            if let renamed, repos.contains(renamed.key) {
                repo = renamed.key
                defaults.set(repo, forKey: "sidefleet_repo")
            } else if let first = repos.first(where: onlineRepos.contains) {
                repo = first
            }
        }
        // When no machine with this repo has the chosen agent, switch to one that does (not remembered).
        if !withRepo.isEmpty, machines.isEmpty, let other = Format.agents.first(where: { a in withRepo.contains { $0.hasAgent(a.id) } }), other.id != agent {
            agent = other.id
            model = initialModel(other.id)
            effort = initialEffort(other.id)
        }
    }

    public func pick(repo r: String) {
        repo = r
        defaults.set(r, forKey: "sidefleet_repo")
    }

    public func pick(agent a: AgentID) {
        agent = a
        defaults.set(a.rawValue, forKey: "sidefleet_agent")
        model = initialModel(a)
        effort = initialEffort(a)
    }

    public func pick(model m: String) {
        model = m.trimmingCharacters(in: .whitespacesAndNewlines)
        defaults.set(model, forKey: modelKey(agent))
    }

    public func pick(effort e: String) {
        effort = e
        defaults.set(e, forKey: effortKey(agent))
    }

    public func pick(machine m: String) {
        machine = m
        defaults.set(m, forKey: "sidefleet_machine")
    }

    /// New tasks start from your defaults in Settings; changing them here is for this task only.
    public func resetFromPrefs() {
        perm = prefs.permission
        iso = prefs.workspace
    }

    public var body: JSONValue {
        var b: [String: JSONValue] = [
            "task": .string(task.trimmingCharacters(in: .whitespacesAndNewlines)),
            "agent": .string(agent.rawValue),
            "model": .string(model),
            "repo": .string(repo),
            "permission_mode": .string(effectivePerm),
            "isolation": .string(iso),
            "failover": .bool(iso == "worktree" && failover),
        ]
        if !effort.isEmpty { b["effort"] = .string(effort) }
        if let id = pinned?.id { b["worker_id"] = .string(id) }
        if !images.isEmpty { b["images"] = .array(images.map(\.payload)) }
        if let startAt { b["start_at"] = .string(Timestamp.format(startAt)) }
        return .object(b)
    }

    /// Start the task. Returns its id, or nil when it couldn't (a toast says why).
    public func run() async -> String? {
        guard canRun else { return nil }
        busy = true
        defer { busy = false }
        do {
            let id = try await fleet.api.createRun(body)
            task = ""
            startAt = nil
            images = []
            await fleet.refresh()
            return id
        } catch {
            fleet.toasts.error("Could not start the task", error)
            return nil
        }
    }

    /// Presets for starting later, worked out when the menu opens so they're never in the past.
    public static func startPresets(now: Date, calendar: Calendar = .current) -> [(label: String, at: Date)] {
        func at(_ h: Int, days: Int = 0) -> Date {
            let day = calendar.date(byAdding: .day, value: days, to: now) ?? now
            return calendar.date(bySettingHour: h, minute: 0, second: 0, of: day) ?? day
        }
        var out: [(String, Date)] = []
        let inHour = now.addingTimeInterval(3600)
        out.append(("In an hour", calendar.date(bySetting: .second, value: 0, of: inHour) ?? inHour))
        if at(22).timeIntervalSince(now) > 30 * 60 { out.append(("Tonight", at(22))) }
        out.append(("Tomorrow morning", at(7, days: 1)))
        out.append(("Tomorrow, 9:00", at(9, days: 1)))
        return out
    }

    /// How far ahead the control plane accepts.
    public static let maxStartDays = 30
}

/// The reply box under a thread: a port of the logic in web/src/routes/runs/[id]/+page.svelte.
@MainActor
@Observable
public final class ReplyModel {
    public var text = ""
    /// '' means the same as the last message.
    public var perm = ""
    public var agent: AgentID = .claude
    public var model = ""
    public var effort = ""
    public var sending = false
    public var stopping = false
    public var images: [AttachedImage] = []

    @ObservationIgnored private let thread: ThreadStore
    @ObservationIgnored private let toasts: ToastCenter
    @ObservationIgnored private var syncedAgent: AgentID?

    public init(thread: ThreadStore, toasts: ToastCenter) {
        self.thread = thread
        self.toasts = toasts
    }

    /// The agent for the next message starts as the thread's latest.
    public func syncFromThread() {
        guard let latest = thread.latest, syncedAgent != latest.agent else { return }
        syncedAgent = latest.agent
        agent = latest.agent
        model = latest.model
        effort = latest.effort ?? ""
    }

    public var threadAgent: AgentID? { thread.latest?.agent }
    public var switching: Bool { threadAgent.map { $0 != agent } ?? false }
    /// Only Claude Code can ask you about tool calls; the others fall back to editing files.
    public var replyPerm: String { perm == "ask" && agent != .claude ? "acceptEdits" : perm }
    /// A new agent starts a session of its own, so a thread without one can carry on that way.
    public var canReply: Bool { thread.hasSession || thread.live || switching }
    public var canSend: Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !sending && Format.modelValid(agent: agent, model: model)
    }

    public var placeholder: String {
        if switching { return "Message \(Format.agentName(agent))…" }
        return thread.live ? "Queue your next message…" : "Reply to continue on the same machine…"
    }

    public var hint: String {
        if switching { return "Switches to \(Format.agentName(agent)): a new session with the conversation so far, same files." }
        if thread.live { return "Your message waits until this one finishes; Send now interrupts it." }
        if !thread.hasSession { return "There is no session to continue; start a new task instead." }
        return "Continues the same session on \(thread.machine ?? "that machine")\(thread.branch.map { ", on \($0)" } ?? "")."
    }

    /// When you pick another agent: the model and effort it last used in this thread, else its defaults.
    public func pick(agent a: AgentID) {
        agent = a
        let last = thread.runs.last { $0.agent == a }
        model = last?.model ?? Format.defaultModel(a)
        effort = last?.effort ?? ""
    }

    public func send() async {
        guard canSend else { return }
        sending = true
        defer { sending = false }
        do {
            let pick: (AgentID, String)? = switching || model != thread.latest?.model ? (agent, model) : nil
            try await thread.reply(text.trimmingCharacters(in: .whitespacesAndNewlines), permission: replyPerm, images: images, agent: pick, effort: effort)
            text = ""
            images = []
        } catch {
            toasts.error("Could not send", error)
        }
    }
}

/// Fork a thread at one message into a new thread: a port of the logic in ForkDialog.svelte.
@MainActor
@Observable
public final class ForkModel {
    public var agent: AgentID
    public var model: String
    public var effort: String
    public var perm: String
    public var task = ""
    public var machine = ""
    public var busy = false

    public let run: Run
    /// The message has a snapshot: the fork starts from its files, on its machine.
    public let hasFiles: Bool
    @ObservationIgnored private let fleet: FleetStore

    public init(run: Run, hasFiles: Bool, fleet: FleetStore, prefs: PrefsStore) {
        self.run = run
        self.hasFiles = hasFiles
        self.fleet = fleet
        agent = run.agent
        model = run.model
        effort = run.effort ?? ""
        perm = Format.permissions.contains { $0.id == run.permissionMode } ? run.permissionMode! : prefs.permission
    }

    public var effectivePerm: String { Format.perms(for: agent).contains { $0.id == perm } ? perm : "acceptEdits" }
    /// The machine holding the files after this message, if they were kept and it is yours.
    public var filesOn: String? {
        guard hasFiles, let w = run.workerID, fleet.mine.contains(where: { $0.id == w }) else { return nil }
        return w
    }
    /// Someone else's message: the fork carries on from it on one of your machines.
    public var theirs: Bool { fleet.othersRun(run.createdBy) }
    /// Machines to choose from: the one with the files (even offline), then your online ones with the repo.
    public var choices: [Worker] {
        fleet.mine.filter { w in
            w.id == filesOn || (w.online && w.capabilities.repos.contains { $0.key == run.repo || $0.name == run.repo })
        }.sorted { ($0.id == filesOn ? 1 : 0) > ($1.id == filesOn ? 1 : 0) }
    }
    public var chosen: Worker? { choices.first { $0.id == machine } }
    /// Where the fork can run: the chosen machine, else the one with the files, else any with the repo.
    public var machines: [Worker] {
        if let chosen { return [chosen] }
        if let filesOn { return choices.filter { $0.id == filesOn } }
        return choices
    }
    /// It starts from the message's files only on the machine that has them.
    public var withFiles: Bool { filesOn != nil && (chosen == nil || chosen?.id == filesOn) }
    public var whereLabel: String { chosen?.displayName ?? filesOn ?? "a machine with \(Format.repoLabel(run.repo))" }
    public var models: [String] { Format.machineModels(machines, agent: agent) }
    public var offline: Bool { machines.count == 1 && !machines[0].online }
    public var can: Bool {
        !task.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && Format.modelValid(agent: agent, model: model)
            && machines.contains { $0.hasAgent(agent) } && !busy
    }

    public func unavailable(_ a: AgentID) -> String? {
        machines.contains { $0.hasAgent(a) } ? nil : "Not on \(whereLabel)"
    }

    public func pick(agent a: AgentID) {
        agent = a
        model = a == run.agent ? run.model : Format.defaultModel(a)
        effort = a == run.agent ? (run.effort ?? "") : ""
    }

    public var explanation: String {
        if theirs {
            let who = fleet.login(run.createdBy).map { "@\($0)'s" } ?? "the"
            return "A new thread of yours, on one of your machines, that knows \(who) conversation up to this message. It starts from \(Format.repoLabel(run.repo)) as it is on your machine, without their changes. Their thread stays as it is."
        }
        return "A new thread that knows the conversation up to this message\(withFiles ? ", and starts with the files as they were after it, on its own branch" : ""). This thread stays as it is."
    }

    public var status: HintPart {
        if machines.isEmpty { return HintPart("None of your online machines has \(Format.repoLabel(run.repo)).", .warning) }
        if !machines.contains(where: { $0.hasAgent(agent) }) { return HintPart("\(Format.agentName(agent)) isn't on \(whereLabel).", .warning) }
        return HintPart("Runs with \(Format.agentName(agent)) on \(whereLabel)\(offline ? ", once it is back online" : "").")
    }

    /// Returns the new thread's id, or nil when it couldn't (a toast says why).
    public func submit() async -> String? {
        guard can else { return nil }
        busy = true
        defer { busy = false }
        var body: [String: JSONValue] = [
            "task": .string(task.trimmingCharacters(in: .whitespacesAndNewlines)),
            "agent": .string(agent.rawValue),
            "model": .string(model),
            "permission_mode": .string(effectivePerm),
        ]
        if !effort.isEmpty { body["effort"] = .string(effort) }
        if let id = chosen?.id { body["worker_id"] = .string(id) }
        do {
            let id = try await fleet.api.fork(run: run.id, body: .object(body))
            await fleet.refresh()
            return id
        } catch {
            fleet.toasts.error("Could not fork", error)
            return nil
        }
    }
}
