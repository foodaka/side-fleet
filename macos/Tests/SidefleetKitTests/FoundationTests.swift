import Foundation
import XCTest
@testable import SidefleetKit

final class TimestampTests: XCTestCase {
    func testParsesWhatRustWrites() {
        let base = Date(timeIntervalSince1970: 1_793_978_641) // 2026-11-06T...
        let s = Timestamp.format(base)
        XCTAssertEqual(Timestamp.parse(s), base)
        XCTAssertEqual(Timestamp.parse("2026-11-07T07:24:01Z")?.timeIntervalSince1970, 1_794_036_241)
        for (text, frac) in [("2026-11-07T07:24:01.5Z", 0.5), ("2026-11-07T07:24:01.123456Z", 0.123456), ("2026-11-07T07:24:01.123456789Z", 0.123456789)] {
            let d = Timestamp.parse(text)
            XCTAssertNotNil(d, text)
            XCTAssertEqual(d!.timeIntervalSince1970, 1_794_036_241 + frac, accuracy: 0.0001, text)
        }
    }

    func testOffsets() {
        XCTAssertEqual(Timestamp.parse("2026-11-07T09:24:01+02:00")?.timeIntervalSince1970, 1_794_036_241)
        XCTAssertEqual(Timestamp.parse("2026-11-07T02:24:01-05:00")?.timeIntervalSince1970, 1_794_036_241)
        XCTAssertEqual(Timestamp.parse("2026-11-07T07:24:01+00")?.timeIntervalSince1970, 1_794_036_241)
    }

    func testRefusesNonsense() {
        for bad in ["", "soon", "2026-11-07", "2026-13-07T07:24:01Z", "2026-11-07T07:24:01", "2026-11-07T07:24:01.Z", "2026-11-07T07:24:01Zjunk"] {
            XCTAssertNil(Timestamp.parse(bad), bad)
        }
    }
}

final class SSEParserTests: XCTestCase {
    func testFramesAcrossChunksEvenInsideACharacter() {
        let text = "event: assistant_text\nid: 7\ndata: {\"text\":\"héllo\"}\n\n"
        let bytes = Array(text.utf8)
        var parser = SSEParser()
        var frames: [SSEFrame] = []
        for b in bytes { frames += parser.feed([b]) }
        XCTAssertEqual(frames, [SSEFrame(event: "assistant_text", data: "{\"text\":\"héllo\"}", id: "7")])
    }

    func testKeepAliveCommentsAndLiveFrames() {
        var parser = SSEParser()
        let frames = parser.feed(Array(": keep-alive\n\nevent: assistant_live\ndata: {\"a\":1}\n\nevent: end\ndata: run finished\n\n".utf8))
        XCTAssertEqual(frames.count, 2)
        XCTAssertNil(frames[0].id, "live frames carry no id")
        XCTAssertEqual(frames[1].event, "end")
    }

    func testCRLFAndMultilineData() {
        var parser = SSEParser()
        let frames = parser.feed(Array("event: x\r\ndata: one\r\ndata: two\r\n\r\n".utf8))
        XCTAssertEqual(frames, [SSEFrame(event: "x", data: "one\ntwo")])
    }

    func testHoldsAnUnfinishedFrame() {
        var parser = SSEParser()
        XCTAssertTrue(parser.feed(Array("event: x\ndata: y\n".utf8)).isEmpty)
        XCTAssertEqual(parser.feed(Array("\n".utf8)), [SSEFrame(event: "x", data: "y")])
    }
}

final class DecodingTests: XCTestCase {
    private func decode<T: Decodable>(_ v: JSONValue, _ t: T.Type = T.self) throws -> T {
        try JSONDecoder().decode(T.self, from: JSONEncoder().encode(v))
    }

    func testAnOldControlPlaneLeavingFieldsOutIsFine() throws {
        let w: Worker = try decode(["id": "box", "capabilities": ["repos": [["name": "proj", "path": "/x"]]]])
        XCTAssertTrue(w.mine, "absent means yours")
        XCTAssertEqual(w.displayName, "box")
        XCTAssertNil(w.capabilities.system)
        XCTAssertEqual(w.capabilities.repos[0].key, "proj")
        XCTAssertFalse(w.hasAgent(.claude))
    }

    func testANewControlPlaneAddingValuesIsFine() throws {
        let r: Run = try decode(["id": "1", "status": "teleporting", "agent": "gemini", "kind": "dance", "brand_new": ["x": 1], "isolation": 7])
        XCTAssertEqual(r.status, .other("teleporting"))
        XCTAssertEqual(r.status.label, "teleporting")
        XCTAssertEqual(r.agent, .claude)
        XCTAssertEqual(r.kind, .agent)
        XCTAssertEqual(r.isolation, "worktree", "an odd type falls back rather than losing the list")
    }

    func testOneOddRowDoesntSinkTheField() throws {
        let i: InboxItem = try decode(["id": "a", "reason": "something_new", "label": "New thing", "since": "not a time", "agent": 5])
        XCTAssertEqual(i.reason, "something_new")
        XCTAssertEqual(i.agent, .claude)
    }

    func testLimitsSkipNulls() throws {
        let w: Worker = try decode(["id": "b", "capabilities": .object([:]), "limited_until": ["claude": "2026-11-07T07:24:01Z", "codex": .null]])
        XCTAssertEqual(w.limitedUntil.count, 1)
    }

    func testSummaryFallsBackToTheRun() throws {
        let s: RunSummary = try decode(["id": "r", "task": "t", "status": "running", "agent": "codex", "model": "m"])
        XCTAssertEqual(s.threadStatus, .running)
        XCTAssertEqual(s.latestAgent, .codex)
        XCTAssertEqual(s.latestModel, "m")
        XCTAssertEqual(s.task, "t", "reads through to the run")
    }
}

final class TurnTests: XCTestCase {
    private func run(_ extra: JSONValue = .object([:])) throws -> Run {
        var o: [String: JSONValue] = ["id": "r1", "task": "t", "status": "queued", "created_at": "2026-11-07T07:00:00Z"]
        for (k, v) in extra.object ?? [:] { o[k] = v }
        return try JSONDecoder().decode(Run.self, from: JSONEncoder().encode(JSONValue.object(o)))
    }

    private func ev(_ seq: Int?, _ kind: String, _ p: JSONValue = .object([:])) -> RunEvent {
        RunEvent(runID: "r1", seq: seq, kind: kind, payload: p, at: Date(timeIntervalSince1970: 1000))
    }

    func testAWholeTurn() throws {
        var t = Turn(run: try run())
        XCTAssertEqual(t.activity, "queued")
        t.apply(ev(0, "status", ["status": "running", "worker_id": "box"]))
        XCTAssertEqual(t.machine, "box")
        XCTAssertEqual(t.activity, "Starting Claude Code")
        t.apply(ev(1, "workspace", ["mode": "worktree", "branch": "sidefleet/x", "from": ["ref": "origin/main", "commit": "abc"]]))
        XCTAssertEqual(t.branch, "sidefleet/x")
        XCTAssertEqual(t.startedFromCommit, "abc")
        t.apply(ev(2, "agent_init", ["model": "claude-opus-5-5", "session_id": "s"]))
        XCTAssertEqual(t.run.sessionID, "s")
        t.apply(ev(nil, "assistant_live", ["text": "Hel"]))
        XCTAssertEqual(t.liveText, "Hel")
        XCTAssertTrue(t.items.isEmpty, "live text is never an item")
        t.apply(ev(3, "assistant_text", ["text": "Hello"]))
        XCTAssertEqual(t.liveText, "")
        t.apply(ev(4, "tool_use", ["id": "a", "name": "Bash", "summary": "ls"]))
        XCTAssertEqual(t.activity, "Ran…")
        t.apply(ev(5, "tool_use", ["id": "b", "name": "WebSearch", "summary": "swift"]))
        XCTAssertEqual(t.activity, "Searched the web…", "the trailing ' for' goes")
        t.apply(ev(6, "tool_result", ["tool_use_id": "a", "content": "boom\nmore", "is_error": true]))
        guard case .tools(_, let tools) = t.groups[1] else { return XCTFail("tools fold together") }
        XCTAssertEqual(tools.count, 2)
        XCTAssertEqual(tools[0].output, "boom\nmore")
        XCTAssertTrue(tools[0].isError)
        XCTAssertEqual(tools[0].firstOutputLine, "boom")
        XCTAssertEqual(ToolSummary.line(tools), "Ran a command, browsed a page")
        t.apply(ev(7, "stderr", ["text": "one"]))
        t.apply(ev(8, "stderr", ["text": "two"]))
        guard case .log(_, let lines)? = t.groups.last else { return XCTFail("stderr folds together") }
        XCTAssertEqual(lines, ["one", "two"])
        t.apply(ev(9, "result", ["subtype": "success", "result": "Hello", "duration_ms": 1500, "num_turns": 2, "total_cost_usd": 0.5, "context": ["used": 50, "limit": 200]]))
        XCTAssertEqual(t.result?.text, "", "the final text isn't repeated")
        XCTAssertEqual(t.result?.context?.percent, 25)
        XCTAssertNil(t.activity)
        t.apply(ev(10, "status", ["status": "succeeded"]))
        XCTAssertEqual(t.run.status, .succeeded)
    }

    func testApprovalsCancelWhenTheRunEnds() throws {
        var t = Turn(run: try run(["status": "running"]))
        t.apply(ev(0, "approval_requested", ["id": "x", "tool_name": "Bash", "input": ["command": "rm -rf build", "description": "Clean"], "summary": "rm"]))
        XCTAssertEqual(t.activity, "Waiting for you to approve")
        guard case .approval(let a)? = t.items.last else { return XCTFail() }
        XCTAssertEqual(a.ask, "run a command")
        XCTAssertEqual(a.main, "rm -rf build")
        XCTAssertEqual(a.note, "Clean")
        t.apply(ev(1, "status", ["status": "stopped"]))
        guard case .approval(let after) = t.items[0] else { return XCTFail() }
        XCTAssertEqual(after.status, "cancelled")
        XCTAssertEqual(after.outcome, "Not answered: the run ended")
    }

    func testApprovalWording() throws {
        func item(_ name: String, _ input: JSONValue = .object([:])) -> ApprovalItem {
            ApprovalItem(key: "k", id: "i", runID: "r", name: name, summary: "", input: input, status: "pending", at: Date())
        }
        XCTAssertEqual(item("mcp__github__read_issue").ask, "use read_issue from github")
        XCTAssertEqual(item("Write").ask, "change a file")
        XCTAssertEqual(item("Other").ask, "use Other")
        XCTAssertNotNil(item("Other", ["x": 1]).detail)
        XCTAssertEqual(item("Other").timeLeft(now: Date()), "0 s left")
    }

    func testAMovedRunAsksToBeFollowed() throws {
        var t = Turn(run: try run(["status": "running", "worker_id": "laptop"]))
        let effects = t.apply(ev(0, "status", ["status": "failed", "moved_to": "r2", "lost_worker": "laptop"]))
        XCTAssertEqual(effects, [.adopt(runID: "r2")])
        guard case .note(let n)? = t.items.last else { return XCTFail() }
        XCTAssertEqual(n.text, "laptop stopped responding. Moving the task to another machine.")
    }

    func testEndingsSayWhatHappened() throws {
        XCTAssertEqual(Turn.endedNote(status: "failed", reason: "worker lost", machine: "box"), "Failed: box stopped responding while the task ran.")
        XCTAssertEqual(Turn.endedNote(status: "stopped", reason: "worker restarted", machine: nil), "Stopped. The machine restarted while the task was stopping.")
        XCTAssertEqual(Turn.endedNote(status: "failed", reason: "oops", machine: "box"), "Failed: oops")
        XCTAssertEqual(Turn.endedNote(status: "succeeded", reason: nil, machine: "box"), "Done")
    }

    func testPullRequestsReachTheThread() throws {
        var t = Turn(run: try run())
        let effects = t.apply(ev(0, "pull_request", ["url": "https://x/1", "number": 12, "state": "open", "opened": true]))
        XCTAssertEqual(effects, [.pullRequest(PullRequest(url: "https://x/1", number: 12, state: "open"))])
        guard case .note(let n)? = t.items.last else { return XCTFail() }
        XCTAssertEqual(n.text, "Opened pull request #12")
    }

    func testUnknownEventsAreIgnored() throws {
        var t = Turn(run: try run())
        XCTAssertEqual(t.apply(ev(0, "from_the_future", ["x": 1])), [])
        XCTAssertTrue(t.items.isEmpty)
    }
}

final class MarkdownTests: XCTestCase {
    func testBlocks() {
        let blocks = MarkdownParser.parse("""
        # Title

        Some **bold** and `code` and [link](https://example.com).

        - one
        - [x] done

        1. first

        ```ts
        const a = 1;
        ```

        > quoted

        | a | b |
        |---|---|
        | 1 | 2 |

        ---
        """)
        guard blocks.count == 8 else { return XCTFail("\(blocks)") }
        XCTAssertEqual(blocks[0], .heading(level: 1, [InlineSpan(text: "Title")]))
        guard case .paragraph(let spans) = blocks[1] else { return XCTFail() }
        XCTAssertTrue(spans.contains(InlineSpan(text: "bold", style: .bold)))
        XCTAssertTrue(spans.contains(InlineSpan(text: "code", style: .code)))
        XCTAssertTrue(spans.contains(InlineSpan(text: "link", link: URL(string: "https://example.com"))))
        guard case .list(false, _, let items) = blocks[2] else { return XCTFail() }
        XCTAssertNil(items[0].checked)
        XCTAssertEqual(items[1].checked, true)
        XCTAssertEqual(blocks[4], .code(language: "ts", text: "const a = 1;"))
        guard case .table(let head, let rows) = blocks[6] else { return XCTFail() }
        XCTAssertEqual(head.count, 2)
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(blocks[7], .rule)
    }

    func testUnsafeLinksDontBecomeLinks() {
        guard case .paragraph(let spans)? = MarkdownParser.parse("[bad](file:///etc/passwd) [worse](javascript:alert(1))").first else { return XCTFail() }
        XCTAssertTrue(spans.allSatisfy { $0.link == nil }, "\(spans)")
    }

    func testRemoteImagesDontLoadTheyLink() {
        guard case .paragraph(let spans)? = MarkdownParser.parse("![a chart](https://tracker.example.com/p.png)").first else { return XCTFail() }
        XCTAssertEqual(spans, [InlineSpan(text: "a chart", link: URL(string: "https://tracker.example.com/p.png"))])
    }

    func testAHalfWrittenDocumentStillParses() {
        guard case .code(_, let text)? = MarkdownParser.parse("```swift\nlet a").first else { return XCTFail() }
        XCTAssertEqual(text, "let a")
    }

    func testRawHTMLIsNeverRendered() {
        XCTAssertEqual(MarkdownParser.parse("<script>alert(1)</script>").first, .code(language: "html", text: "<script>alert(1)</script>"))
    }
}

final class HighlighterTests: XCTestCase {
    private func kinds(_ line: String, _ lang: String) -> [Token] {
        var open = false
        return Highlighter.tokenize(line, Highlighter.language(for: lang)!, &open)
    }

    func testTokensRebuildTheLine() {
        for line in ["const a = \"x\" // done", "fn main() { let n = 0x1f; }", "  'unterminated", "", "é ü → 🙂 = 1"] {
            XCTAssertEqual(kinds(line, "ts").map(\.text).joined(), line)
        }
    }

    func testKinds() {
        let t = kinds("const total = add(1, 'x'); // sum", "ts")
        XCTAssertTrue(t.contains(Token(text: "const", kind: .keyword)))
        XCTAssertTrue(t.contains(Token(text: "add", kind: .function)))
        XCTAssertTrue(t.contains(Token(text: "1", kind: .number)))
        XCTAssertTrue(t.contains(Token(text: "'x'", kind: .string)))
        XCTAssertTrue(t.contains(Token(text: "// sum", kind: .comment)))
    }

    func testBlockCommentsCarryAcrossLines() {
        let out = Highlighter.highlight(lines: ["a /* open", "still", "end */ b"], language: Highlighter.language(for: "x.rs"))
        XCTAssertEqual(out[1], [Token(text: "still", kind: .comment)])
        XCTAssertEqual(out[2].first, Token(text: "end */", kind: .comment))
    }

    func testUnknownLanguagesStayPlain() {
        XCTAssertNil(Highlighter.language(for: "notes.xyz"))
        XCTAssertEqual(Highlighter.highlight(lines: ["if x"], language: nil), [[Token(text: "if x", kind: .plain)]])
    }
}

final class SnoozeTests: XCTestCase {
    private var calendar: Calendar {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "UTC")!
        return c
    }

    func testPresets() {
        let now = Timestamp.parse("2026-11-07T12:00:00Z")! // a Saturday
        func at(_ when: SnoozeWhen) -> String { Timestamp.format(SnoozePreset(label: "x", when: when).time(now: now, calendar: calendar)) }
        XCTAssertEqual(at(.hours(3)), "2026-11-07T15:00:00Z")
        XCTAssertEqual(at(.at(time: "18:00", day: "today")), "2026-11-07T18:00:00Z")
        XCTAssertEqual(at(.at(time: "09:00", day: "today")), "2026-11-08T09:00:00Z", "gone by today: tomorrow")
        XCTAssertEqual(at(.at(time: "09:00", day: "tomorrow")), "2026-11-08T09:00:00Z")
        XCTAssertEqual(at(.at(time: "09:00", day: "mon")), "2026-11-09T09:00:00Z")
        XCTAssertEqual(at(.at(time: "09:00", day: "sat")), "2026-11-14T09:00:00Z", "never today")
    }

    func testCleaningDropsWhatsMalformed() {
        let clean = SnoozePreset.clean([
            ["label": "ok", "hours": 2],
            ["label": "  ", "hours": 2],
            ["label": "bad time", "time": "25:00", "day": "today"],
            ["label": "bad day", "time": "09:00", "day": "someday"],
            ["label": "fine", "time": "09:00", "day": "fri"],
        ])
        XCTAssertEqual(clean.map(\.label), ["ok", "fine"])
        XCTAssertEqual(SnoozePreset.clean(nil), SnoozePreset.defaults)
        XCTAssertEqual(SnoozePreset.clean([]), SnoozePreset.defaults)
    }

    func testWakeLabels() {
        let now = Timestamp.parse("2026-11-07T12:00:00Z")!
        XCTAssertEqual(Format.wakeLabel(now.addingTimeInterval(3600), now: now, calendar: calendar), "13:00")
        XCTAssertEqual(Format.wakeLabel(now.addingTimeInterval(86_400), now: now, calendar: calendar), "Tomorrow 12:00")
    }
}

final class FormatTests: XCTestCase {
    func testDurationsNeverRollPast59() {
        let start = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(Format.duration(start: start, now: start.addingTimeInterval(59.6)), "1m 00s")
        XCTAssertEqual(Format.duration(start: start, now: start.addingTimeInterval(3599.6)), "1h 00m")
        XCTAssertEqual(Format.duration(start: nil, now: start), "")
    }

    func testBuilds() {
        let b = Format.parseBuild("0.1.0 (2026-10-20 dec70b750)")
        XCTAssertEqual(b.version, "0.1.0")
        XCTAssertEqual(b.sha, "dec70b750")
        XCTAssertEqual(Format.parseBuild("weird").version, "weird")
    }

    func testLinksMustBeSafe() {
        XCTAssertNotNil(Format.safeLink("https://example.com"))
        XCTAssertNotNil(Format.safeLink("mailto:a@example.com"))
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "sidefleet://runs/1", "ftp://x", nil] as [String?] {
            XCTAssertNil(Format.safeLink(bad), String(describing: bad))
        }
    }

    func testAttachmentLimits() {
        func image(_ name: String, type: String = "image/png", mb: Double = 1) -> AttachedImage {
            AttachedImage(name: name, mediaType: type, data: Data(count: Int(mb * 1024 * 1024)))
        }
        var items: [AttachedImage] = []
        XCTAssertEqual(Attachments.add([image("a.svg", type: "image/svg+xml")], to: &items), ["a.svg is not a PNG, JPEG, GIF or WebP image"])
        XCTAssertEqual(Attachments.add([image("big", mb: 11)], to: &items), ["big is over 10 MB"])
        XCTAssertEqual(Attachments.add((0..<9).map { image("\($0)") }, to: &items), ["At most 8 images per message"])
        XCTAssertEqual(items.count, 8)
        items = (0..<4).map { image("\($0)", mb: 10) }
        XCTAssertEqual(Attachments.add([image("over", mb: 1)], to: &items), ["The images add up to more than 40 MB"])
    }
}

final class SVGPathTests: XCTestCase {
    func testRelativeAndAbsolute() {
        XCTAssertEqual(SVGPath.parse("M22 24H2V0h20zM17 4.8H7v14.4h10z").prefix(5), [.move(22, 24), .line(2, 24), .line(2, 0), .line(22, 0), .close])
    }

    func testCompactNumbersAndCurves() {
        let p = SVGPath.parse("m1.5.5 1-.5c.5 0 1 .5 1 1s.5 1 1 1")
        XCTAssertEqual(p[0], .move(1.5, 0.5))
        XCTAssertEqual(p[1], .line(2.5, 0))
        XCTAssertEqual(p[2], .curve(x: 3.5, y: 1, c1x: 3, c1y: 0, c2x: 3.5, c2y: 0.5))
        XCTAssertEqual(p[3], .curve(x: 4.5, y: 2, c1x: 3.5, c1y: 1.5, c2x: 4, c2y: 2))
    }

    func testArcsHonourCompactFlagsAndLandOnTheirEnd() {
        // A half circle from (0,0) to (2,0), flags running straight into the numbers.
        let p = SVGPath.parse("M0 0a1 1 0 012 0")
        guard case .curve(let x, let y, _, _, _, _)? = p.last else { return XCTFail("\(p)") }
        XCTAssertEqual(x, 2, accuracy: 1e-9)
        XCTAssertEqual(y, 0, accuracy: 1e-9)
        // Its middle bulges to the side the sweep flag says: y-down, so sweep 1 goes through y = -1.
        guard case .curve(let mx, let my, _, _, _, _) = p[1] else { return XCTFail("\(p)") }
        XCTAssertEqual(mx, 1, accuracy: 1e-9)
        XCTAssertEqual(my, -1, accuracy: 1e-9)
    }

    /// The real icons parse to the end and stay on their artboards.
    func testBrandIconsAreComplete() {
        for (name, d) in [("claude", BrandPaths.claude), ("codex", BrandPaths.codex), ("opencode", BrandPaths.opencode)] {
            let p = SVGPath.parse(d)
            XCTAssertGreaterThan(p.count, 3, name)
            var maxV = 0.0, minV = 0.0
            for c in p {
                switch c {
                case .move(let x, let y), .line(let x, let y):
                    maxV = max(maxV, x, y)
                    minV = min(minV, x, y)
                case .curve(let x, let y, _, _, _, _):
                    maxV = max(maxV, x, y)
                    minV = min(minV, x, y)
                case .close:
                    break
                }
            }
            XCTAssertLessThanOrEqual(maxV, 24.5, name)
            XCTAssertGreaterThanOrEqual(minV, -0.5, name)
            XCTAssertEqual(p.last, .close, "\(name) reached its end")
        }
        XCTAssertEqual(BrandPaths.strands.count, 6)
        XCTAssertEqual(SVGPath.parse(BrandPaths.strands[0].path).first, .move(6, 16))
    }
}
