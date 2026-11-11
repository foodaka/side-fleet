import Foundation
import XCTest
@testable import SidefleetKit
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

private func decode<T: Decodable>(_ v: JSONValue, _ t: T.Type = T.self) throws -> T {
    try JSONDecoder().decode(T.self, from: JSONEncoder().encode(v))
}

private func worker(_ id: String, agents: JSONValue) throws -> Worker {
    try decode(["id": .string(id), "online": true, "capabilities": ["agents": agents, "hostname": "h", "os": "linux", "version": "1"]])
}

/// What web/src/lib/sf/format.ts does since the picker took its versions from the machines.
final class ModelVersionTests: XCTestCase {
    func testClaudeModelsAreAMapAndTheOthersAList() throws {
        let w = try worker("a", agents: [
            "claude": ["version": "2", "signed_in": true, "models": ["fable": "claude-fable-5-2", "opus": "claude-opus-5-5"]],
            "codex": ["version": "1", "models": ["gpt-6-luna"]],
            "opencode": ["version": "1", "models": ["a/b", "c/d"], "models_total": 2],
        ])
        XCTAssertEqual(w.capabilities.agents?.claude?.aliases, ["fable": "claude-fable-5-2", "opus": "claude-opus-5-5"])
        XCTAssertEqual(w.capabilities.agents?.claude?.models, [], "the map isn't a list of models to pick from")
        XCTAssertEqual(w.capabilities.agents?.codex?.models, ["gpt-6-luna"])
        XCTAssertEqual(w.capabilities.agents?.codex?.aliases, [:])
        XCTAssertEqual(w.capabilities.agents?.opencode?.models, ["a/b", "c/d"])
    }

    func testAnOlderWorkerSaysNothing() throws {
        let w = try worker("a", agents: ["claude": ["version": "2", "signed_in": true]])
        XCTAssertEqual(w.capabilities.agents?.claude?.aliases, [:])
        XCTAssertEqual(Format.machineModels([w], agent: .claude), [])
        XCTAssertEqual(Format.pickerModelName("opus", models: Format.machineModels([w], agent: .claude)), "Opus")
    }

    func testMachineModelsForClaudeAreWhatTheAliasesRun() throws {
        let a = try worker("a", agents: ["claude": ["version": "2", "models": ["fable": "claude-fable-5-2", "opus": "claude-opus-5-5"]]])
        let b = try worker("b", agents: ["claude": ["version": "2", "models": ["fable": "claude-fable-5-1", "opus": "claude-opus-5-5"]]])
        XCTAssertEqual(Format.machineModels([a], agent: .claude), ["claude-fable-5-2", "claude-opus-5-5"])
        // Most common first; ties keep the order they were found in.
        XCTAssertEqual(Format.machineModels([a, b], agent: .claude), ["claude-opus-5-5", "claude-fable-5-2", "claude-fable-5-1"])
    }

    func testThePickerNamesTheVersionOnlyWhenMachinesAgree() {
        XCTAssertEqual(Format.pickerModelName("opus"), "Opus")
        XCTAssertEqual(Format.pickerModelName("fable", models: ["claude-fable-5-2", "claude-opus-5-5"]), "Fable 5.2")
        XCTAssertEqual(Format.pickerModelName("opus", models: ["claude-fable-5-2", "claude-opus-5-5"]), "Opus 5.5")
        XCTAssertEqual(Format.pickerModelName("haiku", models: ["claude-fable-5-2"]), "Haiku", "nobody said")
        XCTAssertEqual(Format.pickerModelName("fable", models: ["claude-fable-5-2", "claude-fable-5-1"]), "Fable", "they disagree")
        // A date isn't a minor version, and a major alone is fine.
        XCTAssertEqual(Format.pickerModelName("sonnet", models: ["claude-sonnet-4-20250514"]), "Sonnet 4")
        XCTAssertEqual(Format.pickerModelName("sonnet", models: ["claude-sonnet-4-5-20250929"]), "Sonnet 4.5")
        XCTAssertEqual(Format.pickerModelName("opus", models: ["claude-opus-5"]), "Opus 5")
        XCTAssertEqual(Format.pickerModelName("opus", models: ["claude-opus-10-12"]), "Opus 10.12")
        XCTAssertEqual(Format.pickerModelName("opus", models: ["claude-opus-latest"]), "Opus")
        // Anything that isn't an alias reads as modelName reads it.
        XCTAssertEqual(Format.pickerModelName("claude-opus-5-5", models: ["claude-opus-5-5"]), "Opus 5.5")
        XCTAssertEqual(Format.pickerModelName("gpt-6-luna"), "gpt-6-luna")
    }

    func testTheDemoShowsVersions() throws {
        let workers: [Worker] = try decode(.array(DemoData.workers(now: Date())))
        XCTAssertEqual(Format.pickerModelName("fable", models: Format.machineModels(workers, agent: .claude)), "Fable 5.2")
    }
}

final class WorktreeSetupTests: XCTestCase {
    private func turn() throws -> Turn {
        Turn(run: try decode(["id": "r1", "task": "t", "status": "running", "created_at": "2026-11-07T07:00:00Z"]))
    }

    private func ev(_ seq: Int, _ p: JSONValue) -> RunEvent {
        RunEvent(runID: "r1", seq: seq, kind: "worktree_setup", payload: p, at: Date(timeIntervalSince1970: 1000))
    }

    private func notes(_ t: Turn) -> [Note] {
        t.items.compactMap { if case .note(let n) = $0 { n } else { nil } }
    }

    func testRunningOnlySaysSo() throws {
        var t = try turn()
        t.apply(ev(1, ["state": "running"]))
        XCTAssertEqual(t.activity, "Setting up the worktree")
        XCTAssertTrue(t.items.isEmpty)
    }

    func testDoneSaysWhatItDid() throws {
        var t = try turn()
        t.apply(ev(2, ["state": "done", "copied": [".env.local", "node_modules"], "ran": "pnpm install --frozen-lockfile"]))
        XCTAssertEqual(notes(t).map(\.text), ["Set up the worktree: copied .env.local, node_modules and ran pnpm install --frozen-lockfile"])
        XCTAssertEqual(notes(t).first?.tone, .muted)
        XCTAssertEqual(notes(t).first?.key, "r1:2")
    }

    func testOnlyOneHalfAndNothingAtAll() throws {
        var t = try turn()
        t.apply(ev(1, ["copied": [".env"]]))
        t.apply(ev(2, ["ran": "make deps", "copied": []]))
        t.apply(ev(3, ["state": "done"]))
        XCTAssertEqual(notes(t).map(\.text), ["Set up the worktree: copied .env", "Set up the worktree: ran make deps"])
    }

    func testProblemsEachGetALine() throws {
        var t = try turn()
        t.apply(ev(4, ["ran": "x", "problems": ["`secrets/` is not ignored by git", "setup exited with 1"]]))
        let n = notes(t)
        XCTAssertEqual(n.map(\.text), ["Set up the worktree: ran x", "Worktree setup: `secrets/` is not ignored by git", "Worktree setup: setup exited with 1"])
        XCTAssertEqual(n.map(\.tone), [.muted, .err, .err])
        XCTAssertEqual(n.map(\.key), ["r1:4", "r1:4-0", "r1:4-1"])
    }
}

final class FileTreeTests: XCTestCase {
    private func files(_ paths: [String]) throws -> [DiffFile] {
        try paths.map { try decode(["path": .string($0), "status": "modified", "additions": 1, "deletions": 0]) }
    }

    private func show(_ rows: [FileTreeRow]) -> [String] {
        rows.map { row in
            switch row {
            case .dir(let name, let depth, _): "\(depth) \(name)/"
            case .file(let f, let depth): "\(depth) \(f.path)"
            }
        }
    }

    func testFoldersFirstAndSingleChildChainsFold() throws {
        let rows = FileTree.rows(try files(["web/src/lib/sf/b.ts", "README.md", "web/src/lib/sf/a.ts", "web/src/routes/+page.svelte", "crates/core/src/models.rs", "web/package.json"]))
        XCTAssertEqual(show(rows), [
            "0 crates/core/src/",
            "1 crates/core/src/models.rs",
            "0 web/",
            "1 src/",
            "2 lib/sf/",
            "3 web/src/lib/sf/a.ts",
            "3 web/src/lib/sf/b.ts",
            "2 routes/",
            "3 web/src/routes/+page.svelte",
            "1 web/package.json",
            "0 README.md",
        ])
        XCTAssertEqual(rows.map(\.key).prefix(4), ["crates/core/src/", "crates/core/src/models.rs", "web/", "web/src/"])
        XCTAssertEqual(FileTree.files(rows).map(\.path), ["crates/core/src/models.rs", "web/src/lib/sf/a.ts", "web/src/lib/sf/b.ts", "web/src/routes/+page.svelte", "web/package.json", "README.md"])
    }

    func testAFolderWithAFileOfItsOwnDoesntFold() throws {
        let rows = FileTree.rows(try files(["a/b/c.txt", "a/x.txt"]))
        XCTAssertEqual(show(rows), ["0 a/", "1 b/", "2 a/b/c.txt", "1 a/x.txt"])
    }

    func testFilterKeepsTheFoldersAbove() throws {
        let rows = FileTree.rows(try files(["web/src/lib/a.ts", "web/src/routes/page.svelte", "README.md"]))
        XCTAssertEqual(show(FileTree.filter(rows, query: " PAGE ")), ["0 web/src/", "1 routes/", "2 web/src/routes/page.svelte"])
        XCTAssertEqual(FileTree.filter(rows, query: "  ").count, rows.count)
        XCTAssertTrue(FileTree.filter(rows, query: "nothing").isEmpty)
    }

    func testWhatAFileSaysInsteadOfADiff() throws {
        let renamed: DiffFile = try decode(["path": "b", "old_path": "a", "status": "renamed"])
        let plain: DiffFile = try decode(["path": "b", "status": "modified"])
        let binary: DiffFile = try decode(["path": "b", "status": "modified", "omitted": "binary"])
        XCTAssertEqual(renamed.noPatchReason, "File renamed without changes.")
        XCTAssertEqual(plain.noPatchReason, "No content changes.")
        XCTAssertEqual(binary.noPatchReason, "Binary file not shown.")
        let big: DiffFile = try decode(["path": "b", "status": "added", "additions": 800, "deletions": 1])
        XCTAssertTrue(big.isLarge)
        XCTAssertTrue(big.oneSided)
        XCTAssertFalse(plain.isLarge)
        XCTAssertFalse(plain.oneSided)
    }

    func testHunkHeadersOnlyWhenTheySaySomething() {
        let hunks = DiffParser.parse("@@ -1,2 +1,2 @@\n-a\n+b\n c\n@@ -40,1 +40,1 @@ fn main() {\n-x\n+y\n")
        XCTAssertEqual(hunks.map(\.hasHeader), [false, true])
    }
}

final class ReviewStateTests: XCTestCase {
    func testViewedFoldsAndOpeningUnmarks() {
        var s = ReviewState()
        XCTAssertTrue(s.isOpen("all", "a"))
        s.setViewed("all", "a", true)
        XCTAssertFalse(s.isOpen("all", "a"))
        XCTAssertTrue(s.isOpen("r2", "a"), "each scope keeps its own marks")
        XCTAssertEqual(s.viewedCount("all", ["a", "b"]), 1)
        s.reveal("all", "a")
        XCTAssertFalse(s.isOpen("all", "a"), "going to a viewed file leaves it folded")
        s.setOpen("all", "a", true)
        XCTAssertFalse(s.isViewed("all", "a"))
        s.setOpen("all", "b", false)
        s.reveal("all", "b")
        XCTAssertTrue(s.isOpen("all", "b"))
    }

    func testCollapseAllThenExpandAll() {
        var s = ReviewState()
        let paths = ["a", "b", "c"]
        s.setViewed("all", "c", true)
        XCTAssertFalse(s.allClosed("all", paths))
        s.toggleAll("all", paths)
        XCTAssertTrue(s.allClosed("all", paths))
        s.toggleAll("all", paths)
        XCTAssertEqual(paths.map { s.isOpen("all", $0) }, [true, true, false], "viewed stays folded")
        XCTAssertFalse(s.allClosed("all", []))
    }

    func testOpenCollapsedFoldsEachScopeOnce() {
        var s = ReviewState()
        s.foldOnce("all", ["a", "b", "c"], except: "b")
        XCTAssertEqual(["a", "b", "c"].map { s.isOpen("all", $0) }, [false, true, false])
        s.setOpen("all", "a", true)
        s.foldOnce("all", ["a", "b", "c"], except: nil)
        XCTAssertTrue(s.isOpen("all", "a"), "only the first time")
        s.foldOnce("one", ["a"], except: nil)
        XCTAssertTrue(s.isOpen("one", "a"), "a single file has nothing to fold")
    }
}

final class DiffHighlightTests: XCTestCase {
    func testEachSideCarriesItsOwnComment() {
        let hunks = DiffParser.parse("@@ -1,4 +1,4 @@\n-/* old note\n+let a = 1;\n still here\n-end */\n+let b = 2;\n")
        let t = Highlighter.highlight(hunks: hunks, language: Highlighter.language(for: "x.ts"))[0]
        XCTAssertEqual(t[0], [Token(text: "/* old note", kind: .comment)])
        XCTAssertTrue(t[1].contains(Token(text: "let", kind: .keyword)), "the new side never opened a comment")
        XCTAssertFalse(t[2].contains { $0.kind == .comment }, "context reads as the new side does")
        XCTAssertEqual(t[3], [Token(text: "end */", kind: .comment)], "the old side was still inside its comment")
        XCTAssertTrue(t[4].contains(Token(text: "let", kind: .keyword)))
    }

    func testACommentOpenedInContextCoversBothSides() {
        let hunks = DiffParser.parse("@@ -1,3 +1,3 @@\n /**\n- * old\n+ * new\n  */\n")
        let t = Highlighter.highlight(hunks: hunks, language: Highlighter.language(for: "x.rs"))[0]
        XCTAssertEqual(t.map { $0.map(\.kind) }, [[.comment], [.comment], [.comment], [.comment]])
        XCTAssertEqual(t.map { $0.map(\.text).joined() }, hunks[0].lines.map(\.text))
    }

    func testHunksStartAfresh() {
        let hunks = DiffParser.parse("@@ -1,1 +1,1 @@\n-/* a\n+/* b\n@@ -90,1 +90,1 @@\n-x()\n+y()\n")
        let t = Highlighter.highlight(hunks: hunks, language: Highlighter.language(for: "x.go"))
        XCTAssertEqual(t[1][1], [Token(text: "y", kind: .function), Token(text: "()", kind: .plain)])
        XCTAssertEqual(Highlighter.highlight(hunks: hunks, language: nil)[0][0], [Token(text: "/* a", kind: .plain)])
    }

    func testFilesKnownByName() {
        XCTAssertNotNil(Highlighter.language(for: "docker/Dockerfile"))
        XCTAssertNotNil(Highlighter.language(for: "Makefile"))
        XCTAssertNotNil(Highlighter.language(for: "web/.env.local"))
        XCTAssertNotNil(Highlighter.language(for: "Cargo.toml"))
        XCTAssertNil(Highlighter.language(for: "notes.xyz"))
        var open = false
        XCTAssertEqual(Highlighter.tokenize("key = 1 # why", Highlighter.language(for: "a.toml")!, &open).last, Token(text: "# why", kind: .comment))
        XCTAssertEqual(Highlighter.tokenize("x = 1 -- why", Highlighter.language(for: "a.lua")!, &open).last, Token(text: "-- why", kind: .comment))
    }
}

final class RepoSearchTests: XCTestCase {
    func testEveryWordMustMatchNameOrFolder() {
        let terms = RepoSearch.terms("  Side  FLEET ")
        XCTAssertEqual(terms, ["side", "fleet"])
        XCTAssertTrue(RepoSearch.matches(label: "foodaka/side-fleet", folders: [], terms: terms))
        XCTAssertFalse(RepoSearch.matches(label: "foodaka/side", folders: [], terms: terms))
        XCTAssertTrue(RepoSearch.matches(label: "acme/app", folders: ["storefront-work"], terms: ["storefront"]), "a machine's folder name counts")
        XCTAssertTrue(RepoSearch.matches(label: "anything", folders: [], terms: []))
    }

    func testMarksWhereEachWordFirstAppears() {
        let p = RepoSearch.parts("foodaka/Side-fleet", terms: ["side", "food"])
        XCTAssertEqual(p.map(\.text), ["food", "aka/", "Side", "-fleet"])
        XCTAssertEqual(p.map(\.marked), [true, false, true, false])
        XCTAssertEqual(RepoSearch.parts("abc", terms: []).map(\.text), ["abc"])
        XCTAssertTrue(RepoSearch.parts("", terms: ["a"]).isEmpty)
    }

    func testChosenFirstThenByName() {
        XCTAssertEqual(RepoSearch.order(["github.com/z/b", "github.com/a/c", "github.com/m/a"], chosen: ["github.com/z/b"]), ["github.com/z/b", "github.com/a/c", "github.com/m/a"])
    }
}

/// Answers the three calls adding a thread to its room makes.
private final class IntegrateBackend: Backend, @unchecked Sendable {
    private let lock = NSLock()
    private var calls: [String] = []

    var seen: [String] {
        lock.lock()
        defer { lock.unlock() }
        return calls
    }

    private func run(_ id: String, kind: String, status: String) -> JSONValue {
        ["id": .string(id), "task": "t", "status": .string(status), "kind": .string(kind), "created_at": "2026-11-07T07:00:00Z", "room_id": "room1"]
    }

    func send(_ method: String, _ path: String, body: JSONValue?) async throws -> Data {
        lock.withLock { calls.append("\(method) \(path)") }
        switch (method, path) {
        case ("GET", "/v1/runs/root/thread"): return try JSONEncoder().encode(JSONValue.array([run("root", kind: "agent", status: "succeeded")]))
        case ("POST", "/v1/runs/root/integrate"): return try JSONEncoder().encode(JSONValue.object(["id": "merge", "status": "queued"]))
        case ("GET", "/v1/runs/merge"): return try JSONEncoder().encode(run("merge", kind: "integrate", status: "queued"))
        default: throw APIError.status(404, "not found")
        }
    }

    func stream(_ path: String, lastEventID: Int?) -> AsyncThrowingStream<SSEFrame, Error> {
        AsyncThrowingStream { continuation in
            if path == "/v1/runs/merge/events" {
                continuation.yield(SSEFrame(event: "status", data: #"{"run_id":"merge","seq":0,"kind":"status","payload":{"status":"running","worker_id":"box"},"at":"2026-11-07T07:00:01Z"}"#, id: "0"))
            }
            continuation.yield(SSEFrame(event: "end", data: "", id: nil))
            continuation.finish()
        }
    }

    func setOrg(_ id: String) {}
}

final class IntegrateTests: XCTestCase {
    @MainActor
    func testAddingToTheRoomFollowsTheNewRun() async throws {
        let backend = IntegrateBackend()
        let thread = ThreadStore(api: API(backend: backend))
        await thread.load("root")
        try await thread.integrate()
        XCTAssertEqual(thread.runs.map(\.id), ["root", "merge"])
        XCTAssertEqual(thread.latest?.kind, .integrate)
        XCTAssertTrue(backend.seen.contains("POST /v1/runs/root/integrate"))
        XCTAssertTrue(backend.seen.contains("GET /v1/runs/merge"))
        for _ in 0..<200 where thread.turns.last?.activity != "Adding the changes to the room" { try await Task.sleep(for: .milliseconds(10)) }
        XCTAssertEqual(thread.turns.last?.activity, "Adding the changes to the room")
        thread.dispose()
    }
}
