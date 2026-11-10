import Foundation

/// A pretend control plane: made-up machines and tasks, no network. It answers the same routes and
/// streams the same events, so the app can't tell. Used by `--demo` and for screenshots.
public final class DemoBackend: Backend, Sendable {
    private let world: DemoWorld

    public init(speed: Double = 1) { world = DemoWorld(speed: speed) }

    public func setOrg(_ id: String) {}

    public func send(_ method: String, _ path: String, body: JSONValue?) async throws -> Data {
        try await world.handle(method, path, body)
    }

    public func stream(_ path: String, lastEventID: Int?) -> AsyncThrowingStream<SSEFrame, Error> {
        AsyncThrowingStream { continuation in
            let world = world
            let task = Task { await world.subscribe(path, after: lastEventID, continuation) }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

private struct DemoRun {
    var json: [String: JSONValue]
    var events: [(kind: String, payload: JSONValue, at: Date)] = []
    var script: [DemoStep] = []
    var waitingOnApproval = false
    var id: String { json["id"]?.string ?? "" }
    var isLive: Bool { ["queued", "running"].contains(json["status"]?.string ?? "") }
}

struct DemoStep: Sendable {
    var delay: Double
    var kind: String
    var payload: JSONValue

    init(_ delay: Double, _ kind: String, _ payload: JSONValue = .object([:])) {
        self.delay = delay
        self.kind = kind
        self.payload = payload
    }
}

private actor DemoWorld {
    private var runs: [String: DemoRun] = [:]
    /// Threads, newest first: each is its runs, root first.
    private var threads: [[String]] = []
    private var workers: [JSONValue]
    private var inbox: [JSONValue] = []
    private var repos: [String]
    private var images: [String: Data] = [:]
    private var subscribers: [String: [UUID: AsyncThrowingStream<SSEFrame, Error>.Continuation]] = [:]
    private var inboxSubscribers: [UUID: AsyncThrowingStream<SSEFrame, Error>.Continuation] = [:]
    private var nextID = 1000
    private var heartbeatTask: Task<Void, Never>?
    private let speed: Double
    private var booted = false
    /// Worktree cleanup, by machine: what's left on disk, the latest preview and the latest cleanup.
    private var team = DemoTeam(now: Date())
    private var worktrees: [String: [JSONValue]] = [:]
    private var previews: [String: JSONValue] = [:]
    private var cleanups: [String: JSONValue] = [:]

    /// The totals of a machine's last cleanup, as everyone polling the list sees them.
    private func withCleanup(_ w: JSONValue) -> JSONValue {
        guard case .object(var o) = w, let id = o["id"]?.string, case .object(var totals)? = cleanups[id] else { return w }
        totals["items"] = nil
        o["worktree_cleanup"] = .object(totals)
        return .object(o)
    }

    init(speed: Double) {
        self.speed = speed
        workers = DemoData.workers(now: Date())
        repos = ["github.com/acme/storefront", "github.com/acme/cli", "github.com/acme/docs"]
    }

    private func bootIfNeeded() {
        guard !booted else { return }
        booted = true
        for t in DemoData.threads(now: Date()) {
            var ids: [String] = []
            for r in t {
                var run = DemoRun(json: r.json)
                run.events = r.events.map { ($0.kind, $0.payload, $0.at) }
                run.script = r.script
                run.waitingOnApproval = r.waitingOnApproval
                runs[run.id] = run
                ids.append(run.id)
                if run.isLive, !run.script.isEmpty { play(run.id) }
            }
            threads.append(ids)
        }
        inbox = DemoData.inbox(now: Date())
    }

    // MARK: Routes

    func handle(_ method: String, _ rawPath: String, _ body: JSONValue?) async throws -> Data {
        bootIfNeeded()
        let parts = rawPath.split(separator: "?", maxSplits: 1)
        let path = String(parts.first ?? "")
        let query = parts.count > 1 ? String(parts[1]) : ""
        let seg = path.split(separator: "/").map(String.init)
        // A touch of latency, so loading states are real.
        try await Task.sleep(for: .milliseconds(Int(40 / max(speed, 0.01))))
        let now = Date()

        // Rooms, the team and chat apps.
        if method == "POST", seg.count == 4, seg[1] == "rooms", seg[3] == "tasks" {
            var task = body
            if case .object(var o)? = task {
                o["room_id"] = .string(seg[2])
                o["repo"] = "github.com/acme/storefront"
                task = .object(o)
            }
            let id = createThread(task, now: now)
            try team.addTask(room: seg[2], run: id, body: body, now: now)
            return try encode(["id": .string(id), "status": "queued"])
        }
        if method == "POST", seg.count == 4, seg[1] == "runs", seg[3] == "integrate", team.integrate(run: seg[2], now: now) {
            return try encode(["id": .string(seg[2])])
        }
        team.tick(now: now)
        if let data = try team.handle(method, seg, query: query, body: body, now: now) { return data }

        switch (method, seg.count > 1 ? seg[1] : "") {
        case ("GET", "me") where seg.count == 2:
            return try encode(DemoData.me)
        case ("GET", "me") where seg.last == "machines":
            return try encode(.array([]))
        case ("GET", "me") where seg.last == "prefs":
            return try encode(.object([:]))
        case ("PUT", "me"):
            return try encode(.object([:]))
        case ("GET", "workers") where seg.last == "clean-worktrees" && seg.count == 4:
            let id = seg[2].removingPercentEncoding ?? seg[2]
            return try encode(["pending": .null, "preview": previews[id] ?? .null, "cleanup": cleanups[id] ?? .null])
        case ("GET", "workers"):
            return try encode(.array(tickedWorkers(now).map(withCleanup)))
        case ("POST", "workers") where seg.last == "clean-worktrees" && seg.count == 4:
            let id = seg[2].removingPercentEncoding ?? seg[2]
            let left = (worktrees[id] ?? DemoData.worktrees(worker: id))
            if body?["mode"]?.string == "remove" {
                // As a worker does: only what was listed and is still safe goes.
                let ids = Set(body?["ids"]?.array?.compactMap(\.string) ?? [])
                let gone = left.filter { ids.contains($0["root_id"]?.string ?? "") && ["merged", "on_default"].contains($0["state"]?.string ?? "") }
                worktrees[id] = left.filter { !gone.contains($0) }
                cleanups[id] = DemoData.cleanup(removed: gone, now: now)
                previews[id] = nil
            } else {
                previews[id] = ["items": .array(left), "at": .string(Timestamp.format(now))]
            }
            return Data()
        case ("POST", "workers"):
            return try encode(.object([:]))
        case ("DELETE", "workers"):
            workers.removeAll { $0["id"]?.string == seg[2].removingPercentEncoding }
            return Data()
        case ("GET", "repos"):
            return try encode(["added": .array(repos.map { .string($0) })])
        case ("PUT", "repos"):
            repos = body?["added"]?.array?.compactMap(\.string) ?? repos
            return try encode(["added": .array(repos.map { .string($0) })])
        case ("GET", "enrollments"):
            return try encode(.array([]))
        case ("GET", "setup"):
            return try encode(["control_url": "https://sidefleet.example.com", "worker_token": "demo-token-not-real", "worker_token_count": 1, "version": "0.1.0", "build": "0.1.0 (2026-11-06 9fa511c59)", "hosted": false])
        case ("GET", "orgs"):
            if seg.last == "tokens" { return try encode(.array(DemoData.tokens)) }
            return try encode(.array([]))
        case ("POST", "orgs"), ("PATCH", "orgs"), ("DELETE", "orgs"):
            return try encode(.object([:]))
        case ("POST", "inbox"):
            if seg.count == 4, seg[3] == "dismiss" {
                inbox.removeAll { $0["id"]?.string == seg[2] }
            } else {
                inbox = []
            }
            broadcastInbox()
            return Data()
        case ("GET", "images"):
            guard seg.count > 2, let data = images[seg[2]] else { throw APIError.status(404, "No such image") }
            return data
        case ("GET", "runs") where seg.count == 2:
            return try encode(.array(summaries(archived: query.contains("archived=true"), now: now)))
        case ("POST", "runs") where seg.count == 2:
            return try encode(["id": .string(createThread(body, now: now)), "status": "queued"])
        case (_, "runs") where seg.count >= 3:
            return try handleRun(method, id: seg[2], rest: Array(seg.dropFirst(3)), body: body, now: now)
        default:
            throw APIError.status(404, "Not in the demo")
        }
    }

    private func handleRun(_ method: String, id: String, rest: [String], body: JSONValue?, now: Date) throws -> Data {
        guard let run = runs[id] else { throw APIError.status(404, "run not found") }
        let thread = threads.first { $0.contains(id) } ?? [id]
        let rootID = thread[0]
        switch (method, rest.first ?? "") {
        case ("GET", ""):
            return try encode(.object(run.json))
        case ("GET", "thread"):
            return try encode(.array(thread.compactMap { runs[$0].map { .object($0.json) } }))
        case ("POST", "messages"):
            guard thread.contains(where: { runs[$0]?.json["session_id"]?.string != nil }) || thread.contains(where: { runs[$0]?.isLive ?? false }) else {
                throw APIError.status(409, "This thread has no session to continue")
            }
            let new = makeRun(task: body?["task"]?.string ?? "", from: runs[thread.last ?? rootID]?.json ?? run.json, parent: thread.last ?? rootID, body: body, now: now)
            add(new, to: rootID)
            if thread.contains(where: { $0 != new.id && (runs[$0]?.isLive ?? false) }) {
                pendingAfter[rootID, default: []].append(new.id)
            } else {
                play(new.id, script: DemoData.followUpScript(machine: new.json["worker_id"]?.string ?? "mac-mini"))
            }
            return try encode(["id": .string(new.id), "status": "queued"])
        case ("POST", "stop"):
            stop(id, now: now)
            return try encode(.object(run.json))
        case ("POST", "start-now"):
            runs[id]?.json["start_at"] = .null
            append(id, "status", ["status": "queued", "started_early": true], now: now)
            play(id, script: DemoData.followUpScript(machine: "mac-mini"))
            return try encode(.object(runs[id]?.json ?? run.json))
        case ("POST", "restore"):
            let new = kindRun("restore", "Restoring the files to before this message", rootID: rootID, from: run.json, now: now)
            play(new.id, script: [DemoStep(1, "status", ["status": "running", "worker_id": run.json["worker_id"] ?? "mac-mini"]), DemoStep(1.2, "restored"), DemoStep(0.2, "status", ["status": "succeeded"])])
            return try encode(["id": .string(new.id), "status": "queued"])
        case ("POST", "pr"):
            let new = kindRun("pr", "Opening a pull request", rootID: rootID, from: run.json, now: now)
            let number = 142 + threads.count
            let url = "https://github.com/acme/storefront/pull/\(number)"
            play(new.id, script: [
                DemoStep(0.8, "status", ["status": "running", "worker_id": run.json["worker_id"] ?? "mac-mini"]),
                DemoStep(1.8, "pull_request", ["url": .string(url), "number": .number(Double(number)), "state": "open", "opened": true]),
                DemoStep(0.2, "status", ["status": "succeeded"]),
            ])
            runs[rootID]?.json["pr"] = ["url": .string(url), "number": .number(Double(number)), "state": "open"]
            return try encode(["id": .string(new.id), "status": "queued"])
        case ("POST", "fork"):
            let task = body?["task"]?.string ?? ""
            var json = makeRun(task: task, from: run.json, parent: nil, body: body, now: now).json
            json["forked_from"] = .string(id)
            let new = DemoRun(json: json)
            runs[new.id] = new
            threads.insert([new.id], at: 0)
            play(new.id, script: DemoData.followUpScript(machine: json["worker_id"]?.string ?? "mac-mini"))
            return try encode(["id": .string(new.id), "status": "queued"])
        case ("POST", "archive"):
            let archived = body?["archived"]?.bool ?? true
            runs[rootID]?.json["archived_at"] = archived ? .string(Timestamp.format(now)) : .null
            inbox.removeAll { $0["id"]?.string == rootID }
            broadcastInbox()
            return try encode(.object(runs[rootID]?.json ?? run.json))
        case ("POST", "snooze"):
            runs[rootID]?.json["snoozed_until"] = body?["until"] ?? .null
            if body?["until"]?.string != nil { inbox.removeAll { $0["id"]?.string == rootID } }
            broadcastInbox()
            return try encode(.object(runs[rootID]?.json ?? run.json))
        case ("POST", "approvals") where rest.count == 3:
            let approve = rest[2] == "approve"
            let approvalID = rest[1]
            let decision: JSONValue = ["id": .string(approvalID), "status": approve ? "approved" : "denied", "decided_by": "you", "message": .null, "tool_name": "Bash"]
            append(id, "approval_decided", decision, now: now)
            runs[id]?.waitingOnApproval = false
            play(id, script: DemoData.afterApproval(approved: approve))
            return try encode(decision)
        default:
            throw APIError.status(404, "Not in the demo")
        }
    }

    private var pendingAfter: [String: [String]] = [:]

    // MARK: Building runs

    private func makeRun(task: String, from: [String: JSONValue], parent: String?, body: JSONValue?, now: Date) -> DemoRun {
        nextID += 1
        var json = from
        json["id"] = .string("demo-\(nextID)")
        json["task"] = .string(task)
        json["status"] = "queued"
        json["created_at"] = .string(Timestamp.format(now))
        json["started_at"] = .null
        json["finished_at"] = .null
        json["result"] = .null
        json["parent_id"] = parent.map { .string($0) } ?? .null
        json["session_id"] = .null
        json["pr"] = .null
        json["archived_at"] = .null
        json["snoozed_until"] = .null
        json["start_at"] = .null
        json["kind"] = "agent"
        json["images"] = .array([])
        if let v = body?["agent"] { json["agent"] = v }
        if let v = body?["model"] { json["model"] = v }
        if let v = body?["effort"] { json["effort"] = v }
        if let v = body?["permission_mode"] { json["permission_mode"] = v }
        var refs: [JSONValue] = []
        for image in body?["images"]?.array ?? [] {
            guard let b64 = image["data"]?.string, let data = Data(base64Encoded: b64) else { continue }
            nextID += 1
            let id = "demo-image-\(nextID)"
            images[id] = data
            refs.append(["id": .string(id), "media_type": image["media_type"] ?? "image/png", "bytes": .number(Double(data.count))])
        }
        json["images"] = .array(refs)
        return DemoRun(json: json)
    }

    private func add(_ run: DemoRun, to root: String) {
        runs[run.id] = run
        if let i = threads.firstIndex(where: { $0.first == root }) { threads[i].append(run.id) }
    }

    private func kindRun(_ kind: String, _ task: String, rootID: String, from: [String: JSONValue], now: Date) -> DemoRun {
        var run = makeRun(task: task, from: from, parent: rootID, body: nil, now: now)
        run.json["kind"] = .string(kind)
        add(run, to: rootID)
        return run
    }

    private func createThread(_ body: JSONValue?, now: Date) -> String {
        var json = makeRun(task: body?["task"]?.string ?? "", from: DemoData.runDefaults, parent: nil, body: body, now: now).json
        if let v = body?["repo"] { json["repo"] = v }
        if let v = body?["isolation"] { json["isolation"] = v }
        if let v = body?["worker_id"] { json["pinned_worker_id"] = v }
        let startAt = body?["start_at"]?.string.flatMap(Timestamp.parse)
        if let startAt, startAt > now { json["start_at"] = .string(Timestamp.format(startAt)) }
        let worker = body?["worker_id"]?.string ?? "mac-mini"
        let run = DemoRun(json: json)
        runs[run.id] = run
        threads.insert([run.id], at: 0)
        if json["start_at"]?.string == nil { play(run.id, script: DemoData.followUpScript(machine: worker)) }
        return run.id
    }

    private func summaries(archived: Bool, now: Date) -> [JSONValue] {
        var out: [(Date, JSONValue)] = []
        for t in threads {
            guard let root = runs[t[0]], (root.json["archived_at"]?.isNull ?? true) != archived else { continue }
            let all = t.compactMap { runs[$0] }
            let latest = all.last { $0.json["status"]?.string == "running" } ?? all.last ?? root
            var at = Timestamp.parse(root.json["created_at"]?.string ?? "") ?? now
            for r in all {
                for key in ["finished_at", "started_at", "created_at"] {
                    if let d = r.json[key]?.string.flatMap(Timestamp.parse), d > at { at = d }
                }
                if let e = r.events.last, e.at > at { at = e.at }
            }
            var json = root.json
            json["follow_ups"] = .number(Double(t.count - 1))
            json["thread_status"] = latest.json["status"] ?? "queued"
            json["last_activity_at"] = .string(Timestamp.format(at))
            json["pending_approvals"] = .number(Double(all.filter(\.waitingOnApproval).count))
            json["latest_agent"] = latest.json["agent"] ?? "claude"
            json["latest_model"] = latest.json["model"] ?? ""
            json["latest_effort"] = latest.json["effort"] ?? .null
            json["latest_worker_id"] = latest.json["worker_id"] ?? .null
            // A list row reports the running message's machine.
            json["worker_id"] = latest.json["worker_id"] ?? .null
            if latest.json["status"]?.string == "running" { json["started_at"] = latest.json["started_at"] ?? .null }
            out.append((at, .object(json)))
        }
        return out.sorted { $0.0 > $1.0 }.map(\.1)
    }

    /// Machines wander a little, so the sparklines and the map have something to show.
    private func tickedWorkers(_ now: Date) -> [JSONValue] {
        let beat = Int(now.timeIntervalSince1970 / 5)
        return workers.enumerated().map { i, w in
            guard case .object(var o) = w, o["online"]?.bool == true, case .object(var caps)? = o["capabilities"],
                  case .object(var sys)? = caps["system"], let base = sys["cpu_percent"]?.double else { return w }
            let wobble = sin(Double(beat + i * 7) * 0.9) * 9 + sin(Double(beat) * 0.31 + Double(i)) * 5
            sys["cpu_percent"] = .number(max(2, min(98, base + wobble)))
            caps["system"] = .object(sys)
            let busy = threads.filter { t in t.contains { runs[$0]?.json["status"]?.string == "running" && runs[$0]?.json["worker_id"] == o["id"] } }.count
            if case .object(var slots)? = caps["slots"] {
                slots["active"] = .number(Double(busy))
                caps["slots"] = .object(slots)
            }
            o["capabilities"] = .object(caps)
            o["last_heartbeat_at"] = .string(Timestamp.format(now.addingTimeInterval(-3)))
            return .object(o)
        }
    }

    private func encode(_ value: JSONValue) throws -> Data { try JSONEncoder().encode(value) }

    // MARK: Streams

    func subscribe(_ path: String, after last: Int?, _ c: AsyncThrowingStream<SSEFrame, Error>.Continuation) async {
        bootIfNeeded()
        let id = UUID()
        if path.hasPrefix("/v1/inbox/events") {
            inboxSubscribers[id] = c
            c.yield(inboxFrame())
            await hold()
            inboxSubscribers[id] = nil
            return
        }
        let seg = path.split(separator: "/").map(String.init)
        guard seg.count >= 4, let run = runs[seg[2]] else {
            c.finish(throwing: APIError.status(404, "run not found"))
            return
        }
        let from = (last ?? -1) + 1
        for (seq, e) in run.events.enumerated() where seq >= from { c.yield(frame(run.id, seq, e.kind, e.payload, e.at)) }
        guard run.isLive else {
            c.yield(SSEFrame(event: "end", data: "run finished"))
            c.finish()
            return
        }
        subscribers[run.id, default: [:]][id] = c
        await hold()
        subscribers[run.id]?[id] = nil
    }

    private func hold() async {
        while !Task.isCancelled { try? await Task.sleep(for: .seconds(3600)) }
    }

    private func frame(_ runID: String, _ seq: Int?, _ kind: String, _ payload: JSONValue, _ at: Date) -> SSEFrame {
        var o: [String: JSONValue] = ["run_id": .string(runID), "kind": .string(kind), "payload": payload, "at": .string(Timestamp.format(at))]
        if let seq { o["seq"] = .number(Double(seq)) }
        let data = (try? JSONEncoder().encode(JSONValue.object(o))).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        return SSEFrame(event: kind, data: data, id: seq.map(String.init))
    }

    private func inboxFrame() -> SSEFrame {
        let data = (try? JSONEncoder().encode(JSONValue.object(["items": .array(inbox)]))).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        return SSEFrame(event: "inbox", data: data)
    }

    private func broadcastInbox() {
        let f = inboxFrame()
        inboxSubscribers.values.forEach { $0.yield(f) }
    }

    private func append(_ id: String, _ kind: String, _ payload: JSONValue, now: Date) {
        guard runs[id] != nil else { return }
        if kind == "assistant_live" {
            subscribers[id]?.values.forEach { $0.yield(frame(id, nil, kind, payload, now)) }
            return
        }
        let seq = runs[id]!.events.count
        runs[id]!.events.append((kind, payload, now))
        if kind == "status", let s = payload["status"]?.string {
            runs[id]!.json["status"] = .string(s)
            if s == "running" {
                runs[id]!.json["started_at"] = .string(Timestamp.format(now))
                runs[id]!.json["worker_id"] = payload["worker_id"] ?? runs[id]!.json["worker_id"] ?? .null
            }
            if !["running", "queued"].contains(s) { runs[id]!.json["finished_at"] = .string(Timestamp.format(now)) }
        }
        if kind == "agent_init", let s = payload["session_id"] { runs[id]!.json["session_id"] = s }
        if kind == "result", let s = payload["session_id"] { runs[id]!.json["session_id"] = s }
        if kind == "approval_requested" { runs[id]!.waitingOnApproval = true }
        let f = frame(id, seq, kind, payload, now)
        subscribers[id]?.values.forEach { $0.yield(f) }
        if kind == "status", let s = payload["status"]?.string, !["running", "queued"].contains(s) { finishRun(id, status: s, now: now) }
    }

    private func finishRun(_ id: String, status: String, now: Date) {
        runs[id]?.waitingOnApproval = false
        subscribers[id]?.values.forEach {
            $0.yield(SSEFrame(event: "end", data: "run finished"))
            $0.finish()
        }
        subscribers[id] = nil
        guard let thread = threads.first(where: { $0.contains(id) }) else { return }
        // A message queued behind this one goes next.
        if let root = thread.first, let next = pendingAfter[root]?.first {
            pendingAfter[root]?.removeFirst()
            play(next, script: DemoData.followUpScript(machine: runs[id]?.json["worker_id"]?.string ?? "mac-mini"))
            return
        }
        guard runs[id]?.json["kind"]?.string == "agent", status != "stopped", let root = thread.first, let rootRun = runs[root] else { return }
        inbox.removeAll { $0["id"]?.string == root }
        inbox.insert([
            "id": .string(root), "run_id": .string(id), "title": rootRun.json["task"] ?? "", "repo": rootRun.json["repo"] ?? "",
            "agent": runs[id]?.json["agent"] ?? "claude", "reason": status == "succeeded" ? "finished" : "failed",
            "label": status == "succeeded" ? "Finished, waiting for a reply" : "Failed", "since": .string(Timestamp.format(now)),
        ], at: 0)
        broadcastInbox()
    }

    private func stop(_ id: String, now: Date) {
        guard let run = runs[id], run.isLive else { return }
        runs[id]?.script = []
        scripted[id] = nil
        append(id, "stopped", .object([:]), now: now)
        append(id, "status", ["status": "stopped"], now: now)
    }

    private var scripted: [String: UUID] = [:]

    private func play(_ id: String, script: [DemoStep]? = nil) {
        if let script { runs[id]?.script = script }
        guard let steps = runs[id]?.script, !steps.isEmpty else { return }
        runs[id]?.script = []
        let token = UUID()
        scripted[id] = token
        Task { await self.run(id, steps, token) }
    }

    private func run(_ id: String, _ steps: [DemoStep], _ token: UUID) async {
        for step in steps {
            try? await Task.sleep(for: .milliseconds(Int(step.delay * 1000 / max(speed, 0.01))))
            guard scripted[id] == token else { return }
            if step.kind == "demo.wait" { return } // Waiting on you: the rest comes after a decision.
            append(id, step.kind, step.payload, now: Date())
        }
    }
}
