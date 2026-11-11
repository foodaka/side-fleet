import Foundation
import XCTest
@testable import SidefleetKit

/// The stores against the demo backend: the same routes and events as a control plane.
@MainActor
final class StoreTests: XCTestCase {
    private func defaults() -> UserDefaults {
        let name = "sf-test-\(UUID().uuidString)"
        let d = UserDefaults(suiteName: name)!
        d.removePersistentDomain(forName: name)
        return d
    }

    private func world() -> (API, FleetStore, PrefsStore, UserDefaults) {
        let api = API(backend: DemoBackend(speed: 50))
        let d = defaults()
        return (api, FleetStore(api: api, toasts: ToastCenter(), defaults: d), PrefsStore(defaults: d), d)
    }

    private func eventually(_ what: String, timeout: TimeInterval = 10, _ check: () -> Bool) async {
        let end = Date().addingTimeInterval(timeout)
        while Date() < end {
            if check() { return }
            try? await Task.sleep(for: .milliseconds(20))
        }
        XCTFail("never happened: \(what)")
    }

    func testFleetLoadsAndDerives() async {
        let (_, fleet, _, _) = world()
        await fleet.refresh()
        XCTAssertEqual(fleet.conn, .ok)
        XCTAssertEqual(fleet.workers.count, 3)
        XCTAssertEqual(fleet.online.count, 2)
        XCTAssertEqual(fleet.running, 2)
        XCTAssertEqual(fleet.waitingApprovals, 1)
        XCTAssertEqual(fleet.composerRepos, ["github.com/acme/cli", "github.com/acme/docs", "github.com/acme/storefront"])
        XCTAssertEqual(fleet.org?.name, "Acme")
        XCTAssertTrue(fleet.isOwner)
        XCTAssertTrue(fleet.multi)
        XCTAssertEqual(fleet.cpu["mac-mini"]?.count, 1)
    }

    func testSidebarSections() async {
        let (_, fleet, _, _) = world()
        await fleet.refresh()
        let s = SidebarSections(runs: fleet.runs, userID: nil, teamScope: true, filtering: false, now: Date())
        XCTAssertEqual(s.active.first?.id, "demo-rate", "waiting on you comes first")
        XCTAssertEqual(s.active.map(\.id), ["demo-rate", "demo-guide", "demo-audit"], "then running, then scheduled")
        XCTAssertEqual(s.snoozed.map(\.id), ["demo-review"])
        XCTAssertEqual(s.homeRepo, "github.com/acme/storefront")
        XCTAssertFalse(s.groups.isEmpty)
        XCTAssertFalse(s.groups.flatMap(\.runs).contains { $0.id == "demo-old" }, "archived stays out")
        XCTAssertEqual(SidebarSections.matches(query: "STRIPE", runs: fleet.runs, archived: nil).map(\.id), ["demo-stripe"])
        XCTAssertEqual(SidebarSections.matches(query: " ", runs: fleet.runs, archived: nil).count, 0)
    }

    func testAFinishedThreadReplays() async {
        let (api, _, _, _) = world()
        let thread = ThreadStore(api: api, liveInterval: .milliseconds(1))
        await thread.load("demo-checkout")
        XCTAssertTrue(thread.loaded)
        XCTAssertEqual(thread.turns.count, 1)
        XCTAssertEqual(thread.branch, "sidefleet/4e61fe7e")
        XCTAssertEqual(thread.machine, "hetzner-vps")
        XCTAssertEqual(thread.changes?.fileCount, 2)
        XCTAssertFalse(thread.live)
        XCTAssertTrue(thread.hasSession)
        XCTAssertEqual(thread.context?.fill?.percent, 41)
        XCTAssertNotNil(thread.forkTurn)
        let groups = thread.turns[0].groups
        XCTAssertEqual(groups.filter { if case .text = $0 { true } else { false } }.count, 4)
        guard case .tools(_, let tools) = groups[1] else { return XCTFail() }
        XCTAssertEqual(ToolSummary.line(tools), "Ran a command")
        let hunks = DiffParser.parse(thread.changes?.files[0].patch ?? "")
        XCTAssertTrue(DiffParser.changes(hunks))
        XCTAssertEqual(hunks.flatMap(\.lines).filter { $0.kind == .add }.count, thread.changes?.files[0].additions, "the demo's patch agrees with its stats")
        XCTAssertEqual(hunks.flatMap(\.lines).filter { $0.kind == .del }.count, thread.changes?.files[0].deletions)
        let spec = DiffParser.parse(thread.changes?.files[1].patch ?? "").flatMap(\.lines)
        XCTAssertEqual(spec.filter { $0.kind == .add }.count, thread.changes?.files[1].additions)
        XCTAssertEqual(spec.filter { $0.kind == .del }.count, thread.changes?.files[1].deletions)
        thread.dispose()
    }

    func testApprovingCarriesTheTaskOn() async throws {
        let (api, fleet, _, _) = world()
        let thread = ThreadStore(api: api, liveInterval: .milliseconds(1))
        var ended = false
        thread.onRunEnded = { ended = true }
        await thread.load("demo-rate")
        await eventually("the approval arrives") { thread.turns.first?.activity == "Waiting for you to approve" }
        guard case .approval(let approval)? = thread.turns[0].items.last else { return XCTFail() }
        XCTAssertEqual(approval.main, "pnpm test src/routes/webhooks.test.ts")
        await fleet.refresh()
        XCTAssertEqual(fleet.waitingApprovals, 1)

        try await thread.decide(approval, approve: true)
        await eventually("it finishes") { thread.turns[0].run.status == .succeeded }
        guard case .approval(let done)? = thread.turns[0].items.first(where: { if case .approval = $0 { true } else { false } }) else { return XCTFail() }
        XCTAssertEqual(done.status, "approved")
        XCTAssertEqual(thread.changes?.fileCount, 1)
        await eventually("the list hears about it") { ended }
        await fleet.refresh()
        XCTAssertEqual(fleet.waitingApprovals, 0)
        thread.dispose()
    }

    func testRunningATaskThenReplying() async throws {
        let (api, fleet, prefs, d) = world()
        await fleet.refresh()
        let model = NewTaskModel(fleet: fleet, prefs: prefs, defaults: d)
        model.syncSelection()
        XCTAssertFalse(model.canRun, "nothing typed yet")
        model.pick(repo: "github.com/acme/docs")
        model.task = "  Tidy the intro  "
        XCTAssertTrue(model.canRun)
        XCTAssertEqual(model.machines.map(\.id), ["mac-mini"], "only the online machine with the repo")
        XCTAssertEqual(model.body["task"]?.string, "Tidy the intro")
        let text = model.explanation(now: Date())[0].map(\.text).joined()
        XCTAssertEqual(text, "Runs with Claude Code on Mac mini with full access, on a new branch so your checkout stays untouched.")

        let id = await model.run()
        XCTAssertNotNil(id)
        XCTAssertEqual(model.task, "", "the box empties")

        let thread = ThreadStore(api: api, liveInterval: .milliseconds(1))
        await thread.load(id!)
        await eventually("the first message finishes") { thread.turns[0].run.status == .succeeded }
        XCTAssertTrue(thread.hasSession)

        let reply = ReplyModel(thread: thread, toasts: fleet.toasts)
        reply.syncFromThread()
        XCTAssertTrue(reply.hint.hasPrefix("Continues the same session on mac-mini"), reply.hint)
        reply.text = "One more thing"
        await reply.send()
        XCTAssertEqual(thread.turns.count, 2)
        XCTAssertEqual(reply.text, "")
        await eventually("the reply finishes") { thread.turns[1].run.status == .succeeded }
        thread.dispose()
    }

    func testQueuedMessagesWaitAndStopDropsThem() async throws {
        let (api, _, _, _) = world()
        let thread = ThreadStore(api: api, liveInterval: .milliseconds(1))
        await thread.load("demo-guide")
        XCTAssertTrue(thread.live)
        let reply = ReplyModel(thread: thread, toasts: ToastCenter())
        reply.syncFromThread()
        XCTAssertEqual(reply.placeholder, "Queue your next message…")
        reply.text = "Then this"
        await reply.send()
        XCTAssertEqual(thread.turns.count, 2)
        XCTAssertTrue(thread.isWaiting(thread.turns[1].run.id))
        XCTAssertEqual(thread.queuedCount, 1)

        try await thread.stop()
        await eventually("both stop") { thread.turns.allSatisfy { $0.run.status == .stopped } }
        XCTAssertEqual(thread.queuedCount, 0)
        thread.dispose()
    }

    func testSwitchingAgentInAReply() async throws {
        let (api, _, _, _) = world()
        let thread = ThreadStore(api: api)
        await thread.load("demo-tax")
        let reply = ReplyModel(thread: thread, toasts: ToastCenter())
        reply.syncFromThread()
        XCTAssertEqual(reply.agent, .claude)
        XCTAssertEqual(reply.model, "haiku")
        reply.pick(agent: .codex)
        XCTAssertTrue(reply.switching)
        XCTAssertEqual(reply.placeholder, "Message Codex…")
        XCTAssertEqual(reply.model, "", "its default, as this thread never used it")
        reply.perm = "ask"
        XCTAssertEqual(reply.replyPerm, "acceptEdits", "only Claude Code can ask")
        thread.dispose()
    }

    func testAComposerFollowsTheMachines() async {
        let (_, fleet, prefs, d) = world()
        d.set("codex", forKey: "sidefleet_agent")
        await fleet.refresh()
        let model = NewTaskModel(fleet: fleet, prefs: prefs, defaults: d)
        model.syncSelection()
        model.pick(repo: "github.com/acme/storefront")
        // The Mac mini has Codex and the VPS doesn't.
        XCTAssertEqual(model.machines.map(\.id), ["mac-mini"])
        XCTAssertEqual(model.models.first, "gpt-6-luna")
        XCTAssertEqual(model.effectivePerm, "bypassPermissions")
        model.perm = "ask"
        XCTAssertEqual(model.effectivePerm, "acceptEdits", "only Claude Code can ask")
        XCTAssertNotNil(model.body["permission_mode"])
        model.pick(agent: .opencode)
        XCTAssertEqual(model.machines.map(\.id), ["hetzner-vps"])
        XCTAssertEqual(model.unavailable(.opencode), nil)
        model.pick(model: "bad model")
        XCTAssertFalse(model.modelOK)
    }

    func testForkModelExplains() async throws {
        let (api, fleet, prefs, _) = world()
        await fleet.refresh()
        let runs = try await api.thread("demo-checkout")
        let fork = ForkModel(run: runs[0], hasFiles: true, fleet: fleet, prefs: prefs)
        XCTAssertEqual(fork.filesOn, "hetzner-vps")
        XCTAssertTrue(fork.withFiles)
        XCTAssertTrue(fork.explanation.contains("with the files as they were after it"))
        XCTAssertFalse(fork.can)
        fork.task = "Try it another way"
        XCTAssertTrue(fork.can)
        let id = await fork.submit()
        XCTAssertNotNil(id)
    }

    func testArchiveAndUndo() async {
        let (_, fleet, _, _) = world()
        await fleet.refresh()
        let r = fleet.runs.first { $0.id == "demo-tax" }!
        await fleet.setArchived(r, archived: true)
        XCTAssertFalse(fleet.runs.contains { $0.id == "demo-tax" })
        XCTAssertEqual(fleet.toasts.toasts.last?.title, "Archived")
        await fleet.setArchived(r, archived: false)
        XCTAssertTrue(fleet.runs.contains { $0.id == "demo-tax" })
    }

    func testSnoozeHidesThenWakes() async {
        let (_, fleet, _, _) = world()
        await fleet.refresh()
        let r = fleet.runs.first { $0.id == "demo-tax" }!
        await fleet.setSnoozed(r, until: Date().addingTimeInterval(3600))
        XCTAssertTrue(fleet.runs.first { $0.id == "demo-tax" }!.isSnoozed(now: Date()))
        await fleet.setSnoozed(r, until: nil)
        XCTAssertFalse(fleet.runs.first { $0.id == "demo-tax" }!.isSnoozed(now: Date()))
    }

    func testInboxFollowsAndDismisses() async {
        let api = API(backend: DemoBackend(speed: 50))
        let inbox = InboxStore(api: api)
        inbox.connect()
        await eventually("it loads") { inbox.loaded }
        XCTAssertEqual(inbox.items.map(\.id), ["demo-checkout", "demo-stripe"])
        XCTAssertEqual(inbox.item(for: "demo-stripe")?.reason, "failed")
        await inbox.dismiss("demo-checkout")
        XCTAssertEqual(inbox.items.map(\.id), ["demo-stripe"])
        await inbox.clearAll()
        // A frame from before can still be on its way; the stream settles on the truth.
        await eventually("it empties") { inbox.items.isEmpty }
        inbox.disconnect()
    }

    func testNotificationsOnlyForWhatsNew() async throws {
        func item(_ id: String, since: String) throws -> InboxItem {
            try JSONDecoder().decode(InboxItem.self, from: JSONEncoder().encode(["id": .string(id), "title": "Line one\nline two", "label": "Failed", "since": .string(since), "reason": "failed"] as JSONValue))
        }
        var planner = NotificationPlanner()
        let a = try item("a", since: "2026-11-07T07:00:00Z")
        XCTAssertEqual(planner.inboxChanged([a], visible: nil), [], "launching doesn't set off a burst")
        let b = try item("b", since: "2026-11-07T07:01:00Z")
        XCTAssertEqual(planner.inboxChanged([b, a], visible: nil), [.init(threadID: "b", title: "Line one", body: "Failed")], "only the title's first line, never output")
        let c = try item("c", since: "2026-11-07T07:02:00Z")
        XCTAssertEqual(planner.inboxChanged([c, b, a], visible: "c"), [], "not while you're looking at it")
        let again = try item("a", since: "2026-11-07T07:05:00Z")
        XCTAssertEqual(planner.inboxChanged([again, c, b], visible: nil).map(\.threadID), ["a"], "waiting again counts")
    }

    func testPrefsPersistAndOnlySyncedOnesGoUp() async {
        let d = defaults()
        let prefs = PrefsStore(defaults: d)
        XCTAssertEqual(prefs.sendKeys(.task), "⌘↵")
        XCTAssertEqual(prefs.sendKeys(.reply), "↵")
        prefs.set(\.sendKey, .modEnter)
        prefs.set(\.diffView, .split)
        prefs.set(\.permission, "ask")
        XCTAssertEqual(prefs.sendKeys(.reply), "⌘↵")
        XCTAssertEqual(prefs.permission(for: .codex), "acceptEdits")
        let again = PrefsStore(defaults: d)
        XCTAssertEqual(again.sendKey, .modEnter)
        XCTAssertEqual(again.diffView, .split)
        XCTAssertEqual(again.permission, "ask")
    }

    func testSeenMarksRead() async throws {
        let d = defaults()
        let (_, fleet, _, _) = world()
        await fleet.refresh()
        let seen = SeenStore(defaults: d)
        seen.start(fleet.runs)
        let r = fleet.runs.first { $0.id == "demo-tax" }!
        XCTAssertFalse(seen.isUnread(r), "the first launch marks everything seen")
        var later = r
        later.lastActivityAt = Date().addingTimeInterval(1000)
        XCTAssertTrue(seen.isUnread(later))
        seen.mark(later)
        XCTAssertFalse(seen.isUnread(later))
        XCTAssertFalse(SeenStore(defaults: d).isUnread(later), "it sticks")
    }
}
