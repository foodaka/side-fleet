import Foundation
import XCTest
@testable import SidefleetKit

/// Worktree cleanup, pending machines and the repo migration: what the Machines screen is built on.
final class MachinesTests: XCTestCase {
    private func decode<T: Decodable>(_ json: String, as: T.Type = T.self) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }

    private func worker(_ id: String, repos: [(name: String, id: String?)]) throws -> Worker {
        let list = repos.map { r in "{\"name\":\"\(r.name)\",\"path\":\"/x/\(r.name)\"" + (r.id.map { ",\"id\":\"\($0)\"" } ?? "") + "}" }.joined(separator: ",")
        return try decode("{\"id\":\"\(id)\",\"online\":true,\"capabilities\":{\"repos\":[\(list)]}}")
    }

    func testFileSizeReadsAsThePageDoes() {
        XCTAssertEqual(Format.fileSize(0), "under 1 MB")
        XCTAssertEqual(Format.fileSize(999_999), "under 1 MB")
        XCTAssertEqual(Format.fileSize(1_000_000), "1 MB")
        XCTAssertEqual(Format.fileSize(42_600_000), "43 MB")
        XCTAssertEqual(Format.fileSize(999_400_000), "999 MB")
        XCTAssertEqual(Format.fileSize(1_000_000_000), "1.0 GB")
        XCTAssertEqual(Format.fileSize(119_260_000_000), "119.3 GB")
    }

    func testWorkerCarriesItsCleanup() throws {
        let w: Worker = try decode("""
        {"id":"mini","online":true,"worktree_cleanup_pending":"remove",
         "worktree_cleanup":{"removed":3,"freed_bytes":2400000000,"kept_in_use":1,"kept_changes":0,"kept_unmerged":2,"failed":1,"at":"2026-11-09T10:00:00Z"},
         "capabilities":{"can_clean_worktrees":true,"agents":{"claude":{"version":"2.4.1","signed_in":true,"models":{"fable":"claude-fable-5-2"}}}}}
        """)
        XCTAssertTrue(w.capabilities.canCleanWorktrees)
        XCTAssertEqual(w.worktreeCleanupPending, "remove")
        XCTAssertEqual(w.worktreeCleanup?.removed, 3)
        XCTAssertEqual(w.worktreeCleanup?.kept, 4)
        // The map of aliases must not cost us the agent.
        XCTAssertEqual(w.capabilities.agents?.claude?.version, "2.4.1")
        let now = Timestamp.parse("2026-11-09T12:00:00Z")!
        XCTAssertEqual(w.worktreeCleanup?.note(now: now), "Cleaned up 2h ago: removed 3, freed 2.4 GB, kept 4 that changed")
    }

    func testWorkerFromAnOlderControlPlaneHasNoCleanup() throws {
        let w: Worker = try decode("{\"id\":\"old\",\"capabilities\":{}}")
        XCTAssertFalse(w.capabilities.canCleanWorktrees)
        XCTAssertNil(w.worktreeCleanupPending)
        XCTAssertNil(w.worktreeCleanup)
    }

    func testCleanupNoteLeavesOutWhatWasNotKept() throws {
        let t: WorktreeCleanupTotals = try decode("{\"removed\":1,\"freed_bytes\":5000000,\"at\":\"2026-11-09T11:59:58Z\"}")
        XCTAssertEqual(t.note(now: Timestamp.parse("2026-11-09T12:00:00Z")!), "Cleaned up just now: removed 1, freed 5 MB")
    }

    func testCleanupStatusSplitsWhatGoesFromWhatStays() throws {
        let s: WorktreeCleanupStatus = try decode("""
        {"pending":null,
         "preview":{"at":"2026-11-09T10:00:00Z","items":[
           {"root_id":"a","repo":"cli","branch":"sidefleet/a","state":"merged","bytes":100,"task":"Fix the flag"},
           {"root_id":"b","repo":"cli","branch":"sidefleet/b","state":"on_default","bytes":900},
           {"root_id":"c","repo":"docs","branch":"sidefleet/c","state":"changes","bytes":50},
           {"root_id":"d","repo":"docs","branch":"main","state":"other_branch","bytes":70},
           {"root_id":"e","repo":"docs","branch":"sidefleet/e","state":"something_new","bytes":10}]},
         "cleanup":{"removed":1,"freed_bytes":100,"kept_in_use":0,"kept_changes":0,"kept_unmerged":0,"failed":0,"at":"2026-11-08T10:00:00Z",
           "items":[{"root_id":"z","repo":"cli","branch":"sidefleet/z","state":"merged","bytes":100}]}}
        """)
        XCTAssertNil(s.pending)
        let items = try XCTUnwrap(s.preview?.items)
        XCTAssertEqual(items.going.map(\.rootID), ["b", "a"])
        XCTAssertEqual(items.staying.map(\.rootID), ["d", "c", "e"])
        XCTAssertEqual(items[0].title, "Fix the flag")
        XCTAssertEqual(items[1].title, "sidefleet/b")
        XCTAssertEqual(items.map(\.reason), ["Pull request merged", "Nothing beyond the default branch", "Uncommitted files", "On another branch", "something_new"])
        XCTAssertEqual(s.cleanup?.totals.removed, 1)
        XCTAssertEqual(s.cleanup?.items.map(\.rootID), ["z"])
    }

    func testCleanupStatusBeforeAnythingWasAsked() throws {
        let s: WorktreeCleanupStatus = try decode("{\"pending\":\"preview\",\"preview\":null,\"cleanup\":null}")
        XCTAssertEqual(s.pending, "preview")
        XCTAssertNil(s.preview)
        XCTAssertNil(s.cleanup)
    }

    func testDemoCleansUpWhatItPreviewed() async throws {
        let api = API(backend: DemoBackend(speed: 50))
        let before = try await api.cleanupStatus(worker: "mac-mini")
        XCTAssertNil(before.preview)
        try await api.previewCleanup(worker: "mac-mini")
        let previewed = try await api.cleanupStatus(worker: "mac-mini")
        let listed = try XCTUnwrap(previewed.preview?.items)
        XCTAssertEqual(listed.count, 4)
        XCTAssertEqual(listed.going.count, 2)
        // Asking for one that isn't safe leaves it alone, as a worker would.
        try await api.removeWorktrees(worker: "mac-mini", ids: listed.map(\.rootID))
        let after = try await api.cleanupStatus(worker: "mac-mini")
        XCTAssertNil(after.preview)
        XCTAssertEqual(after.cleanup?.totals.removed, 2)
        XCTAssertEqual(Set(after.cleanup?.items.map(\.rootID) ?? []), Set(listed.going.map(\.rootID)))
        let workers = try await api.workers()
        let mini = try XCTUnwrap(workers.first { $0.id == "mac-mini" })
        XCTAssertEqual(mini.worktreeCleanup?.removed, 2)
        XCTAssertTrue(mini.capabilities.canCleanWorktrees)
        try await api.previewCleanup(worker: "mac-mini")
        let again = try await api.cleanupStatus(worker: "mac-mini").preview?.items ?? []
        XCTAssertEqual(again.count, 2)
        XCTAssertTrue(again.going.isEmpty)
    }

    func testEnrollmentSaysWhatTheMachineRuns() throws {
        let e: Enrollment = try decode("""
        {"id":"e1","code":"ABCD-EFGH","machine_name":"mini","worker_id":"mini","status":"pending",
         "created_at":"2026-11-09T10:00:00Z","expires_at":"2026-11-09T10:10:00Z",
         "capabilities":{"hostname":"mini.local","os":"macos","version":"0.1.0","repo_count":7,
           "agents":{"claude":{"version":"2.4.1","signed_in":false}}}}
        """)
        XCTAssertEqual(e.claude?.version, "2.4.1")
        XCTAssertEqual(e.claude?.signedIn, false)
        XCTAssertEqual(e.repoCount, 7)
        XCTAssertEqual(e.version, "0.1.0")
        XCTAssertEqual(e.workerID, "mini")
        XCTAssertEqual(Format.minutesLeft(e.expiresAt, now: Timestamp.parse("2026-11-09T10:06:30Z")!), 4)
        XCTAssertEqual(Format.minutesLeft(e.expiresAt, now: Timestamp.parse("2026-11-09T11:00:00Z")!), 0)
        let bare: Enrollment = try decode("{\"id\":\"e2\",\"capabilities\":{}}")
        XCTAssertNil(bare.claude)
    }

    func testMyMachineKeepsTheOrgItConnectedTo() throws {
        let m: MyMachine = try decode("{\"id\":\"mini\",\"display_name\":\"Mac mini\",\"org\":{\"id\":\"o1\",\"name\":\"Acme\"},\"online\":true,\"repos\":[\"a\",\"b\"],\"agents\":[\"claude\"]}")
        XCTAssertEqual(m.orgID, "o1")
        XCTAssertEqual(m.orgName, "Acme")
        XCTAssertEqual(m.key, "o1mini")
    }

    func testMigrationWaitsForIdentities() throws {
        let old = try worker("a", repos: [("side-fleet", nil)])
        XCTAssertEqual(RepoMigration.migrate(added: ["side-fleet"], workers: [old]), .notYet)
        XCTAssertEqual(RepoMigration.migrate(added: ["side-fleet"], workers: []), .notYet)
    }

    func testMigrationTurnsFolderNamesIntoIdentities() throws {
        let w = try worker("a", repos: [("side-fleet", "github.com/foodaka/side-fleet"), ("notes", nil)])
        XCTAssertEqual(
            RepoMigration.migrate(added: ["side-fleet", "notes", "gone"], workers: [w]),
            .changed(["github.com/foodaka/side-fleet", "notes", "gone"])
        )
        XCTAssertEqual(RepoMigration.migrate(added: ["github.com/foodaka/side-fleet", "notes"], workers: [w]), .unchanged)
    }

    func testMigrationRepairsAnSSHAliasAndDropsDoubles() throws {
        let w = try worker("a", repos: [("side-fleet", "github.com/foodaka/side-fleet"), ("cli", "github.com/acme/cli"), ("cli2", "gitlab.com/acme/cli")])
        // Same path, real host; and the folder name that now means the same repo is dropped.
        XCTAssertEqual(
            RepoMigration.migrate(added: ["github.com-work/foodaka/side-fleet", "side-fleet"], workers: [w]),
            .changed(["github.com/foodaka/side-fleet"])
        )
        // Two hosts hold that path, so there's no telling which was meant.
        XCTAssertEqual(RepoMigration.migrate(added: ["alias/acme/cli"], workers: [w]), .unchanged)
    }

    func testLimitClockAndAgentNames() {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        cal.locale = Locale(identifier: "en_US")
        let now = Timestamp.parse("2026-11-09T12:00:00Z")!
        XCTAssertEqual(Format.limitClock(Timestamp.parse("2026-11-09T18:05:00Z")!, now: now, calendar: cal), "18:05")
        XCTAssertEqual(Format.limitClock(Timestamp.parse("2026-11-20T07:00:00Z")!, now: now, calendar: cal), "Fri 07:00")
        XCTAssertEqual(Format.limitedAgentName("codex"), "Codex")
        XCTAssertEqual(Format.limitedAgentName("gemini"), "gemini")
    }

    func testLoopbackAddressesAreSpotted() {
        XCTAssertTrue(Format.isLoopback(URL(string: "http://localhost:8787")))
        XCTAssertTrue(Format.isLoopback(URL(string: "http://127.0.0.1:8787")))
        XCTAssertTrue(Format.isLoopback(URL(string: "http://[::1]:8787")))
        XCTAssertFalse(Format.isLoopback(URL(string: "https://app.sidefleet.xyz")))
        XCTAssertFalse(Format.isLoopback(URL(string: "http://100.64.0.2:8787")))
        XCTAssertFalse(Format.isLoopback(nil))
    }

    func testFeedbackCarriesNoCredential() throws {
        let r = try Feedback.request(message: "  It froze  ", email: " a@b.c ", build: "0.1.0 (2026-11-06 9fa511c59)")
        XCTAssertEqual(r.url?.absoluteString, "https://www.sidefleet.xyz/api/feedback")
        XCTAssertEqual(r.httpMethod, "POST")
        XCTAssertNil(r.value(forHTTPHeaderField: "Authorization"))
        XCTAssertNil(r.value(forHTTPHeaderField: "Cookie"))
        XCTAssertNil(r.value(forHTTPHeaderField: "X-Sidefleet-Org"))
        let body = try JSONDecoder().decode(JSONValue.self, from: try XCTUnwrap(r.httpBody))
        XCTAssertEqual(body["message"]?.string, "It froze")
        XCTAssertEqual(body["email"]?.string, "a@b.c")
        XCTAssertEqual(body["build"]?.string, "0.1.0 (2026-11-06 9fa511c59)")
        XCTAssertEqual(body["company"]?.string, "")
    }
}
