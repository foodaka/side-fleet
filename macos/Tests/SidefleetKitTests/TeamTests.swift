import Foundation
import XCTest
@testable import SidefleetKit

/// Rooms, the team and chat apps: the ported helpers against the web code's own output, and the
/// stores against the demo backend.
@MainActor
final class TeamTests: XCTestCase {
    private func defaults() -> UserDefaults {
        let name = "sf-test-\(UUID().uuidString)"
        let d = UserDefaults(suiteName: name)!
        d.removePersistentDomain(forName: name)
        return d
    }

    private func fleet() async -> FleetStore {
        let fleet = FleetStore(api: API(backend: DemoBackend(speed: 50)), toasts: ToastCenter(), defaults: defaults())
        await fleet.refresh()
        return fleet
    }

    // MARK: Chat apps, held to web/src/lib/sf/chat.ts

    /// Fixtures/chat-golden.json is what chat.ts returned with the page at https://fleet.example.com.
    func testChatSetupMatchesTheWeb() throws {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Fixtures/chat-golden.json")
        let g = try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))
        let control = URL(string: "https://fleet.example.com/")
        let token = "sfa_\"q\\uo'te"
        for app in ChatAppID.allCases {
            let a = app.rawValue
            XCTAssertEqual(ChatApps.setupSnippet(app, token: token, control: control), g["snippet"]?[a]?.string, "snippet \(a)")
            XCTAssertEqual(ChatApps.setupMessage(app, token: token, control: control), g["message"]?[a]?.string, "message \(a)")
            XCTAssertEqual(ChatApps.setupSteps(app).before, g["steps"]?[a]?["before"]?.string)
            XCTAssertEqual(ChatApps.setupSteps(app).after, g["steps"]?[a]?["after"]?.string)
            let setup = ChatApps.updatesSetup(app, secret: "sec", platform: "discord")
            XCTAssertEqual(setup.before, g["updatesSetup"]?[a]?["before"]?.string)
            XCTAssertEqual(setup.snippet, g["updatesSetup"]?[a]?["snippet"]?.string)
            XCTAssertEqual(setup.after, g["updatesSetup"]?[a]?["after"]?.string)
            XCTAssertEqual(ChatApps.updatesKind(app), g["kind"]?[a]?.string)
            let inputs = [" https://h.example.com:8644// ", "https://h.example.com/webhooks/x", "https://h.example.com/hooks/y"]
            XCTAssertEqual(inputs.map { ChatApps.updatesURL(app, address: $0) }, g["url"]?[a]?.array?.compactMap(\.string), "url \(a)")
            let stored = ["https://h.example.com/webhooks/sidefleet", "https://h.example.com/hooks/agent", "https://h.example.com"]
            XCTAssertEqual(stored.map { ChatApps.updatesAddress(app, url: $0) }, g["address"]?[a]?.array?.compactMap(\.string), "address \(a)")
        }
        for app in [ChatAppID.hermes, .openclaw] {
            XCTAssertEqual(ChatApps.updatesMessage(app, secret: "sec", platform: "slack"), g["updatesMessage"]?[app.rawValue]?.string)
        }
        for name in ["Hermes", "openclaw", "Claude Code", "Other app", "Something else"] {
            XCTAssertEqual(ChatApps.appOf(name: name).rawValue, g["appOf"]?[name]?.string, name)
        }
    }

    // MARK: Rooms

    func testRoomHelpers() throws {
        XCTAssertEqual(Format.repoCloneCommand("github.com/acme/web"), "git clone https://github.com/acme/web.git")
        XCTAssertEqual(Format.repoCloneCommand("web"), "")
        XCTAssertEqual(Format.repoCloneCommand("acme/web"), "")
        XCTAssertEqual(Format.repoCloneCommand(nil), "")
        XCTAssertEqual(
            Rooms.fixConflictsPrompt(branch: "room/pay", files: ["a.ts", "b.ts"]),
            "Adding this task to the room failed: its changes conflict with work already on the room branch (a.ts, b.ts). Fetch origin and merge origin/room/pay into this branch, resolve the conflicts so both behaviours are kept, make sure it builds and the tests pass, and commit. Then I'll add it to the room again."
        )
        XCTAssertTrue(Rooms.fixConflictsPrompt(branch: "r", files: []).hasPrefix("Adding this task to the room failed: its changes conflict with work already on the room branch. Fetch"))

        let room = try JSONDecoder().decode(Room.self, from: Data(#"{"id":"r","name":"n","repo":"github.com/acme/web","branch":"room/apple pay#1"}"#.utf8))
        XCTAssertEqual(Rooms.compareURL(room, workers: [])?.absoluteString, "https://github.com/acme/web/compare/room/apple%20pay%231?expand=1")
        let bare = try JSONDecoder().decode(Room.self, from: Data(#"{"id":"r","name":"n","repo":"web","branch":"room/x"}"#.utf8))
        XCTAssertNil(Rooms.compareURL(bare, workers: []))
    }

    func testRoomDetailDecodesAndSortsTasks() async throws {
        let fleet = await fleet()
        let store = RoomStore(id: "room-applepay", fleet: fleet)
        await store.load()
        let room = try XCTUnwrap(store.room)
        XCTAssertEqual(room.name, "Apple Pay at checkout")
        XCTAssertNotNil(room.integratedAt)
        XCTAssertEqual(store.notes.count, 3)
        XCTAssertNotNil(store.notes[2].editedAt)
        // Live first, then what failed to merge, then what isn't in the room yet, then what is.
        XCTAssertEqual(store.tasks.map(\.id), ["demo-room-button", "demo-checkout", "demo-room-totals", "demo-room-server"])
        XCTAssertEqual(store.tasks[1].integration?.conflicts, ["src/lib/checkout/PayButton.tsx"])
        XCTAssertTrue(store.tasks[1].integration?.failed ?? false)
        XCTAssertTrue(store.canEdit)
        XCTAssertTrue(store.isMine(store.tasks[0]))
        XCTAssertFalse(store.isMine(store.tasks[1]))
        XCTAssertTrue(store.isMine(store.notes[0]))
        XCTAssertFalse(store.isMine(store.notes[1]))
        // The room's creator can take anyone's note back.
        XCTAssertTrue(store.canDelete(store.notes[1]))
        XCTAssertEqual(store.compareURL?.absoluteString, "https://github.com/acme/storefront/compare/room/apple-pay-at-checkout?expand=1")

        let gone = RoomStore(id: "nope", fleet: fleet)
        await gone.load()
        XCTAssertNotNil(gone.missing)
        XCTAssertNil(gone.room)
    }

    func testRoomEditsNotesAndTasks() async throws {
        let fleet = await fleet()
        let store = RoomStore(id: "room-applepay", fleet: fleet)
        await store.load()

        let renamed = await store.save(name: "  Apple Pay  ")
        XCTAssertTrue(renamed)
        XCTAssertEqual(store.room?.name, "Apple Pay")
        let blank = await store.save(name: "   ")
        XCTAssertFalse(blank)

        let posted = await store.post(note: " ship it ")
        XCTAssertTrue(posted)
        XCTAssertEqual(store.notes.last?.body, "ship it")
        let note = try XCTUnwrap(store.notes.last)
        let saved = await store.save(note: note, body: "ship it tomorrow")
        XCTAssertTrue(saved)
        XCTAssertEqual(store.notes.last?.body, "ship it tomorrow")
        XCTAssertNotNil(store.notes.last?.editedAt)
        await store.delete(note: store.notes.last!)
        XCTAssertEqual(store.notes.count, 3)

        // Adding a finished task merges it and the room counts it.
        let totals = try XCTUnwrap(store.tasks.first { $0.id == "demo-room-totals" })
        XCTAssertNil(totals.integration)
        await store.integrate(totals)
        XCTAssertTrue(store.tasks.first { $0.id == "demo-room-totals" }?.integration?.done ?? false)
        XCTAssertEqual(store.room?.integratedCount, 2)

        // Starting a task from the room's box.
        let model = RoomTaskModel(fleet: fleet, prefs: PrefsStore(defaults: defaults()), defaults: defaults())
        let room = try XCTUnwrap(store.room)
        XCTAssertFalse(model.canStart(room))
        XCTAssertNil(model.blocked(room))
        XCTAssertEqual(model.perm, "acceptEdits")
        model.task = "Cover the cancelled sheet in the e2e suite"
        XCTAssertTrue(model.canStart(room))
        XCTAssertTrue(model.hint(room).contains("starting from room/apple-pay-at-checkout"))
        let id = await model.start(in: room)
        XCTAssertNotNil(id)
        XCTAssertEqual(model.task, "")
        await store.load()
        XCTAssertEqual(store.tasks.first?.id, id)
        XCTAssertEqual(store.room?.taskCount, 5)
    }

    func testRoomsListAndCreate() async {
        let fleet = await fleet()
        let rooms = RoomsStore(fleet: fleet)
        XCTAssertNil(rooms.rooms)
        await rooms.load()
        XCTAssertEqual(rooms.rooms?.map(\.id), ["room-applepay", "room-billing", "room-docs"])
        XCTAssertEqual(rooms.repoChoices.count, 3)
        let room = await rooms.create(name: " Search ", goal: "", repo: "github.com/acme/cli")
        XCTAssertEqual(room?.name, "Search")
        XCTAssertEqual(rooms.rooms?.first?.name, "Search")
    }

    // MARK: Team

    func testInvites() async throws {
        let fleet = await fleet()
        XCTAssertTrue(fleet.multi)
        let team = TeamStore(fleet: fleet)
        await team.load()
        XCTAssertEqual(team.pending.count, 1)
        XCTAssertFalse(team.personal)
        XCTAssertEqual(team.machines(of: "u-grace"), 0)
        team.onePerson = true
        await team.createInvite()
        XCTAssertEqual(team.fresh, "demo-invite-not-real")
        XCTAssertEqual(team.pending.count, 2)
        XCTAssertEqual(team.pending.first?.maxUses, 1)
        await team.revoke(team.pending[0])
        XCTAssertNil(team.fresh)
        XCTAssertEqual(team.pending.count, 1)

        let me = try XCTUnwrap(fleet.members.first { $0.id == fleet.userID })
        // Not the only owner's to leave.
        XCTAssertFalse(team.canLeave(me))

        let expired = try JSONDecoder().decode(Invite.self, from: Data(#"{"id":"i","expires_at":"2020-01-01T00:00:00Z","uses":0}"#.utf8))
        XCTAssertFalse(expired.isPending(now: Date()))
        let used = try JSONDecoder().decode(Invite.self, from: Data(#"{"id":"i","expires_at":"2999-01-01T00:00:00Z","uses":1,"max_uses":1}"#.utf8))
        XCTAssertFalse(used.isPending(now: Date()))
    }

    func testInviteLinks() {
        let control = URL(string: "https://fleet.example.com/")
        XCTAssertEqual(TeamStore.link(code: "abc", control: control), "https://fleet.example.com/#/join/abc")
        XCTAssertEqual(TeamStore.code(from: " https://fleet.example.com/#/join/abc-123 \n"), "abc-123")
        XCTAssertEqual(TeamStore.code(from: "sidefleet://join/xyz"), "xyz")
        XCTAssertEqual(TeamStore.code(from: "https://fleet.example.com/#/join/abc?x=1"), "abc")
        XCTAssertEqual(TeamStore.code(from: "abc123"), "abc123")
        XCTAssertNil(TeamStore.code(from: "https://fleet.example.com/"))
        XCTAssertNil(TeamStore.code(from: "  "))
        XCTAssertNil(TeamStore.code(from: "https://fleet.example.com/#/join/"))
        XCTAssertEqual(InviteInfo(valid: false, reason: "used_up").problem, "This invite has already been used.")
        XCTAssertEqual(InviteInfo(valid: false, reason: "new_reason").problem, "This invite no longer works.")
    }

    // MARK: Chat apps

    func testConnectingAChatApp() async throws {
        let fleet = await fleet()
        let chat = ChatAppsStore(fleet: fleet)
        await chat.load()
        XCTAssertEqual(chat.listed.map(\.name), ["Hermes"])
        XCTAssertNotNil(chat.listed[0].updates)
        XCTAssertFalse(chat.showSetup)

        chat.adding = true
        chat.pick(.other)
        XCTAssertEqual(chat.how, .yourself)
        chat.pick(.openclaw)
        XCTAssertEqual(chat.how, .ask)
        await chat.connect()
        XCTAssertEqual(chat.fresh?.token, "sfa_demo-key-not-real")
        XCTAssertEqual(chat.fresh?.name, "OpenClaw")
        // The one being set up stays out of the list above the card.
        XCTAssertEqual(chat.listed.count, 1)
        XCTAssertTrue(chat.waiting)
        XCTAssertFalse(chat.connected)

        let current = try XCTUnwrap(chat.current)
        let updates = ChatUpdatesModel(tokenID: current.id, app: .openclaw, fleet: fleet)
        updates.openOnce(on: nil)
        XCTAssertEqual(updates.stage, .form)
        updates.stage = .idle
        updates.openOnce(on: nil)
        XCTAssertEqual(updates.stage, .idle, "opened for you only once")
        updates.edit(on: nil)
        updates.address = "https://claw.example.com:18789/"
        await updates.save()
        XCTAssertEqual(updates.stage, .setup)
        XCTAssertEqual(updates.setup.snippet, "hooks: {\n  enabled: true,\n  token: \"demo-secret-not-real\",\n  path: \"/hooks\",\n},")
        await chat.load()
        XCTAssertEqual(chat.current?.updates?.url, "https://claw.example.com:18789/hooks/agent")
        await updates.sendTest()
        XCTAssertEqual(updates.test?.ok, true)
        XCTAssertEqual(updates.test?.text, "Sent. OpenClaw should message you in a moment.")
        let off = await updates.turnOff()
        XCTAssertTrue(off)

        await chat.disconnect(current)
        XCTAssertNil(chat.fresh)
        XCTAssertEqual(chat.live.count, 1)
    }
}
