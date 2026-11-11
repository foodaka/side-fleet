import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import XCTest
@testable import SidefleetKit

/// The client against a real `sidefleet serve`, with a pretend worker posting events through the real
/// worker API. Skipped unless `scripts/test-live.sh` (or you) point it at a throwaway control plane:
/// SIDEFLEET_TEST_URL, SIDEFLEET_TEST_DEV_KEY and SIDEFLEET_TEST_WORKER_TOKEN.
@MainActor
final class LiveServerTests: XCTestCase {
    private var url: URL!
    private var api: API!
    private var worker: HTTPBackend!
    private var workerID = ""

    override func setUp() async throws {
        let env = ProcessInfo.processInfo.environment
        guard let u = env["SIDEFLEET_TEST_URL"], let key = env["SIDEFLEET_TEST_DEV_KEY"], let token = env["SIDEFLEET_TEST_WORKER_TOKEN"] else {
            throw XCTSkip("no control plane to test against")
        }
        url = URL(string: u)!
        api = API(backend: HTTPBackend(baseURL: url, credential: .devKey(key)))
        worker = HTTPBackend(baseURL: url, credential: .devKey(token))
        workerID = "box-\(UUID().uuidString.prefix(6))"
        _ = try await worker.send("POST", "/v1/worker/register", body: [
            "id": .string(workerID), "display_name": .string(workerID), "active_runs": [],
            "capabilities": [
                "hostname": "box.local", "os": "linux", "version": "0.1.0 (2026-11-06 abc1234)",
                "repos": [["name": "proj", "path": "/tmp/proj", "id": "github.com/acme/proj"]],
                "agents": ["claude": ["version": "2.4.1", "signed_in": true]],
                "system": ["cpu_cores": 8, "cpu_percent": 12.5, "load_avg_1m": 0.5, "mem_total_mb": 16000, "mem_used_mb": 8000, "disk_total_gb": 100, "disk_free_gb": 50, "uptime_secs": 3600],
                "slots": ["parallel": 4, "active": 0],
            ],
        ])
    }

    private func claim() async throws -> Run {
        let body: JSONValue = ["worker_id": .string(workerID), "repos": ["github.com/acme/proj", "proj"], "busy_repos": [], "agents": ["claude"], "kinds": ["agent"], "slots": ["parallel": 4, "active": 0]]
        for _ in 0..<50 {
            let data = try await worker.send("POST", "/v1/worker/claim", body: body)
            if !data.isEmpty, let run = try? JSONDecoder().decode(Run.self, from: data) { return run }
            try await Task.sleep(for: .milliseconds(200))
        }
        throw APIError.transport("nothing to claim")
    }

    private func post(_ run: String, _ events: [(String, JSONValue)]) async throws {
        let list: [JSONValue] = events.map { ["kind": .string($0.0), "payload": $0.1] }
        _ = try await worker.send("POST", "/v1/worker/runs/\(run)/events?worker_id=\(workerID)", body: ["events": .array(list)])
    }

    private func finish(_ run: String, _ status: String = "succeeded") async throws {
        _ = try await worker.send("POST", "/v1/worker/runs/\(run)/finish?worker_id=\(workerID)", body: ["status": .string(status), "result": ["text": "done"]])
    }

    private func newRun(_ task: String, perm: String = "acceptEdits", extra: [String: JSONValue] = [:]) async throws -> String {
        var body: [String: JSONValue] = ["task": .string(task), "agent": "claude", "model": "sonnet", "repo": "github.com/acme/proj", "permission_mode": .string(perm), "isolation": "worktree"]
        for (k, v) in extra { body[k] = v }
        return try await api.createRun(.object(body))
    }

    private func eventually(_ what: String, timeout: TimeInterval = 15, _ check: () -> Bool) async {
        let end = Date().addingTimeInterval(timeout)
        while Date() < end {
            if check() { return }
            try? await Task.sleep(for: .milliseconds(25))
        }
        XCTFail("never happened: \(what)")
    }

    func testEverythingTheFleetLoadsDecodes() async throws {
        let d = UserDefaults(suiteName: "sf-live-\(UUID().uuidString)")!
        let fleet = FleetStore(api: api, toasts: ToastCenter(), defaults: d)
        await fleet.refresh()
        XCTAssertEqual(fleet.conn, .ok)
        XCTAssertNil(fleet.me?.user, "the developer key is nobody in particular")
        XCTAssertFalse(fleet.me?.orgs.isEmpty ?? true)
        let w = fleet.workers.first { $0.id == workerID }
        XCTAssertEqual(w?.online, true)
        XCTAssertEqual(w?.capabilities.system?.cpuCores, 8)
        XCTAssertEqual(w?.capabilities.slots?.parallel, 4)
        XCTAssertEqual(w?.capabilities.repos.first?.key, "github.com/acme/proj")
        XCTAssertNotNil(w?.lastHeartbeatAt, "the server's timestamps parse")
        XCTAssertTrue(fleet.onlineRepos.contains("github.com/acme/proj"))
        XCTAssertNotNil(fleet.setup)
        XCTAssertFalse(fleet.setup?.controlURL.isEmpty ?? true)
    }

    func testSignInDiscoveryAndABadKey() async throws {
        let methods = try await API.authMethods(at: url)
        XCTAssertTrue(methods.devKey)
        XCTAssertFalse(methods.github)
        let wrong = API(backend: HTTPBackend(baseURL: url, credential: .devKey("not-the-key")))
        do {
            _ = try await wrong.workers()
            XCTFail("a bad key should fail")
        } catch {
            XCTAssertEqual(error as? APIError, .unauthorized)
        }
    }

    func testTheServersOwnErrorIsWhatYouSee() async throws {
        do {
            _ = try await api.createRun(["task": "x", "agent": "claude", "model": "not a model", "repo": "proj"])
            XCTFail("should be refused")
        } catch let e as APIError {
            XCTAssertEqual(e.httpStatus, 400)
            XCTAssertTrue(e.localizedDescription.contains("model"), e.localizedDescription)
        }
        do {
            _ = try await api.run(UUID().uuidString)
            XCTFail("no such run")
        } catch let e as APIError {
            XCTAssertEqual(e.httpStatus, 404)
        }
    }

    func testAWholeRunStreamsIntoATurn() async throws {
        let id = try await newRun("Fix the flaky test")
        let thread = ThreadStore(api: api, liveInterval: .milliseconds(1), retryAfter: .milliseconds(100))
        var ended = false
        thread.onRunEnded = { ended = true }
        await thread.load(id)
        XCTAssertEqual(thread.root?.status, .queued)
        XCTAssertEqual(thread.root?.task, "Fix the flaky test")
        XCTAssertEqual(thread.turns[0].activity, "queued")

        let run = try await claim()
        XCTAssertEqual(run.id, id)
        await eventually("the claim shows") { thread.turns[0].run.status == .running }
        XCTAssertEqual(thread.machine, workerID)

        try await post(id, [
            ("workspace", ["mode": "worktree", "branch": "sidefleet/abc", "from": ["ref": "origin/main", "commit": "deadbeef"]]),
            ("agent_init", ["model": "claude-sonnet-5-5", "session_id": "real-session"]),
            ("assistant_text", ["text": "Looking into it."]),
            ("tool_use", ["id": "t1", "name": "Bash", "summary": "pnpm test"]),
        ])
        await eventually("events arrive") { thread.turns[0].items.count == 2 }
        XCTAssertEqual(thread.branch, "sidefleet/abc")
        XCTAssertEqual(thread.turns[0].activity, "Ran…")

        // Live text: not stored, no sequence number.
        _ = try await worker.send("POST", "/v1/worker/runs/\(id)/live?worker_id=\(workerID)", body: ["text": "Writing the fi"])
        await eventually("live text shows") { thread.turns[0].liveText == "Writing the fi" }

        try await post(id, [
            ("tool_result", ["tool_use_id": "t1", "content": "1 failed", "is_error": true]),
            ("assistant_text", ["text": "Fixed."]),
            ("diff", ["turn": ["files": [["path": "a.ts", "old_path": .null, "status": "modified", "additions": 1, "deletions": 1, "patch": "@@ -1 +1 @@\n-a\n+b\n", "omitted": .null]], "file_count": 1, "additions": 1, "deletions": 1]]),
            ("snapshot", ["before": "aaa", "after": "bbb"]),
            ("result", ["subtype": "success", "is_error": false, "result": "Fixed.", "duration_ms": 1234, "num_turns": 3, "total_cost_usd": 0.05, "session_id": "real-session"]),
        ])
        await eventually("the result lands") { thread.turns[0].result != nil }
        XCTAssertEqual(thread.turns[0].liveText, "", "the stored text replaced it")
        XCTAssertEqual(thread.changes?.fileCount, 1)
        XCTAssertTrue(thread.turns[0].hasSnapshot)

        try await finish(id)
        await eventually("it ends") { thread.turns[0].run.status == .succeeded }
        await eventually("the stream closes") { ended }
        XCTAssertTrue(thread.hasSession)
        guard case .tools(_, let tools) = thread.turns[0].groups[1] else { return XCTFail() }
        XCTAssertTrue(tools[0].isError)
        thread.dispose()

        // A fresh look replays to exactly the same turn.
        let again = ThreadStore(api: api)
        await again.load(id)
        XCTAssertEqual(again.turns[0].items.count, thread.turns[0].items.count)
        XCTAssertEqual(again.turns[0].result?.turns, 3)
        XCTAssertEqual(again.turns[0].branch, "sidefleet/abc")
        again.dispose()
    }

    /// Drops the connection after a few frames, the way a laptop going to sleep does.
    final class FlakyBackend: Backend, @unchecked Sendable {
        let inner: Backend
        let lock = NSLock()
        var drops: Int
        let after: Int
        var connections = 0

        init(_ inner: Backend, drops: Int, after: Int) {
            self.inner = inner
            self.drops = drops
            self.after = after
        }

        func setOrg(_ id: String) { inner.setOrg(id) }
        func send(_ m: String, _ p: String, body: JSONValue?) async throws -> Data { try await inner.send(m, p, body: body) }

        func stream(_ path: String, lastEventID: Int?) -> AsyncThrowingStream<SSEFrame, Error> {
            lock.lock()
            connections += 1
            let drop = drops > 0
            if drop { drops -= 1 }
            lock.unlock()
            let source = inner.stream(path, lastEventID: lastEventID)
            let after = after
            return AsyncThrowingStream { c in
                let task = Task {
                    var n = 0
                    do {
                        for try await f in source {
                            c.yield(f)
                            n += 1
                            if drop, n >= after { throw APIError.transport("dropped") }
                        }
                        c.finish()
                    } catch {
                        c.finish(throwing: error)
                    }
                }
                c.onTermination = { _ in task.cancel() }
            }
        }
    }

    func testADroppedConnectionResumesWithoutMissingOrRepeating() async throws {
        let id = try await newRun("Survive a dropped connection")
        _ = try await claim()
        var events: [(String, JSONValue)] = []
        for i in 0..<20 { events.append(("assistant_text", ["text": .string("line \(i)")])) }
        try await post(id, events)

        let flaky = FlakyBackend(api.backend, drops: 3, after: 4)
        let thread = ThreadStore(api: API(backend: flaky), liveInterval: .milliseconds(1), retryAfter: .milliseconds(50))
        await thread.load(id)
        await eventually("all arrive across the drops") { thread.turns[0].items.count == 20 }
        XCTAssertGreaterThanOrEqual(flaky.connections, 4, "it really reconnected")
        let texts = thread.turns[0].items.compactMap { item -> String? in
            if case .text(_, let t, _) = item { t } else { nil }
        }
        XCTAssertEqual(texts, (0..<20).map { "line \($0)" }, "nothing missing, nothing repeated, in order")

        // More after the drops still comes through.
        try await post(id, [("assistant_text", ["text": "after"])])
        await eventually("live resumes") { thread.turns[0].items.count == 21 }
        try await finish(id)
        await eventually("it ends") { thread.turns[0].run.status == .succeeded }
        thread.dispose()
    }

    func testApprovalsEndToEnd() async throws {
        let id = try await newRun("Ask before running", perm: "ask")
        let thread = ThreadStore(api: api, liveInterval: .milliseconds(1))
        await thread.load(id)
        _ = try await claim()
        let data = try await worker.send("POST", "/v1/worker/runs/\(id)/approvals?worker_id=\(workerID)", body: ["tool_name": "Bash", "input": ["command": "rm -rf build", "description": "Clean"], "timeout_secs": 600])
        let approvalID = try JSONDecoder().decode(JSONValue.self, from: data)["id"]?.string ?? ""
        await eventually("the card shows") { thread.turns[0].items.contains { if case .approval(let a) = $0 { a.id == approvalID } else { false } } }
        guard case .approval(let item)? = thread.turns[0].items.last else { return XCTFail() }
        XCTAssertEqual(item.main, "rm -rf build")
        XCTAssertEqual(item.status, "pending")
        XCTAssertNotNil(item.expiresAt)
        XCTAssertEqual(thread.turns[0].activity, "Waiting for you to approve")

        let list = try await api.runs()
        XCTAssertEqual(list.first { $0.id == id }?.pendingApprovals, 1, "the sidebar says waiting on you")

        try await thread.decide(item, approve: true)
        await eventually("it is approved") {
            if case .approval(let a)? = thread.turns[0].items.last { a.status == "approved" } else { false }
        }
        let poll = try await worker.send("GET", "/v1/worker/runs/\(id)/approvals/\(approvalID)?worker_id=\(workerID)", body: nil)
        XCTAssertEqual(try JSONDecoder().decode(JSONValue.self, from: poll)["status"]?.string, "approved", "the worker hears it")
        try await finish(id)
        thread.dispose()
    }

    func testInboxArrivesAndDismisses() async throws {
        let inbox = InboxStore(api: api)
        inbox.connect()
        await eventually("the stream opens") { inbox.loaded }
        let id = try await newRun("Finish and wait for me")
        _ = try await claim()
        try await post(id, [("agent_init", ["session_id": "s"])])
        try await finish(id)
        await eventually("it waits on you") { inbox.item(for: id) != nil }
        XCTAssertEqual(inbox.item(for: id)?.reason, "finished")
        await inbox.dismiss(id)
        XCTAssertNil(inbox.item(for: id))
        try await Task.sleep(for: .milliseconds(400))
        XCTAssertNil(inbox.item(for: id), "and it stays gone")
        inbox.disconnect()
    }

    func testRepliesStopSnoozeArchiveAndImages() async throws {
        // A 1x1 PNG, attached, and read back through the authenticated image route.
        let png = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==")!
        let image = AttachedImage(name: "a.png", mediaType: "image/png", data: png)
        let id = try await newRun("With a picture", extra: ["images": .array([image.payload])])
        let thread = ThreadStore(api: api, liveInterval: .milliseconds(1))
        await thread.load(id)
        let ref = try XCTUnwrap(thread.root?.images.first)
        XCTAssertEqual(ref.mediaType, "image/png")
        let fetched = try await api.image(ref.id)
        XCTAssertEqual(fetched, png)

        _ = try await claim()
        try await post(id, [("agent_init", ["model": "claude-sonnet-5-5", "session_id": "s1"])])

        // A reply while it runs waits behind it.
        let reply = ReplyModel(thread: thread, toasts: ToastCenter())
        await eventually("the session is known") { thread.hasSession }
        reply.syncFromThread()
        reply.text = "And another thing"
        await reply.send()
        XCTAssertEqual(thread.turns.count, 2)
        XCTAssertEqual(thread.turns[1].run.parentID, id)
        XCTAssertTrue(thread.isWaiting(thread.turns[1].run.id))

        // Stop drops the waiting one and asks the running one to stop.
        try await thread.stop()
        await eventually("the waiting one is stopped") { thread.turns[1].run.status == .stopped }
        try await finish(id, "stopped")
        await eventually("the first ends") { thread.turns[0].run.status == .stopped }

        let d = UserDefaults(suiteName: "sf-live-\(UUID().uuidString)")!
        let fleet = FleetStore(api: api, toasts: ToastCenter(), defaults: d)
        await fleet.refresh()
        let summary = try XCTUnwrap(fleet.runs.first { $0.id == id })
        XCTAssertEqual(summary.followUps, 1)

        let until = Date().addingTimeInterval(3600)
        await fleet.setSnoozed(summary, until: until)
        let snoozed = try XCTUnwrap(fleet.runs.first { $0.id == id })
        XCTAssertEqual(snoozed.run.snoozedUntil?.timeIntervalSince1970 ?? 0, until.timeIntervalSince1970, accuracy: 1.5, "the server took our time format")
        await fleet.setSnoozed(summary, until: nil)
        XCTAssertNil(fleet.runs.first { $0.id == id }?.run.snoozedUntil)

        await fleet.setArchived(summary, archived: true)
        XCTAssertFalse(fleet.runs.contains { $0.id == id })
        await fleet.loadArchived()
        XCTAssertTrue(fleet.archived?.contains { $0.id == id } ?? false)
        await fleet.setArchived(summary, archived: false)
        XCTAssertTrue(fleet.runs.contains { $0.id == id })
        thread.dispose()
    }

    func testScheduledTasksAndStartNow() async throws {
        let at = Date().addingTimeInterval(3600)
        let id = try await newRun("Later", extra: ["start_at": .string(Timestamp.format(at))])
        let thread = ThreadStore(api: api, liveInterval: .milliseconds(1))
        await thread.load(id)
        XCTAssertTrue(thread.root?.isScheduled(now: Date()) ?? false)
        try await thread.startNow(id)
        XCTAssertNil(thread.root?.startAt)
        let run = try await claim()
        XCTAssertEqual(run.id, id)
        try await finish(id)
        thread.dispose()
    }

    func testRepoListRoundTrip() async throws {
        let saved = try await api.saveAddedRepos(["github.com/acme/proj"])
        XCTAssertEqual(saved, ["github.com/acme/proj"])
        let again = try await api.addedRepos()
        XCTAssertEqual(again, saved)
    }
}
