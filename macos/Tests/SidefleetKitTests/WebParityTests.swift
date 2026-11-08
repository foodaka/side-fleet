import Foundation
import XCTest
@testable import SidefleetKit

/// Fixtures/web-golden.json holds what web/src/lib/sf's real TypeScript returned for these inputs, so
/// the Swift port is held to the page's behaviour rather than to my reading of it.
final class WebParityTests: XCTestCase {
    private func golden() throws -> JSONValue {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Fixtures/web-golden.json")
        return try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))
    }

    private func kind(_ k: DiffLineKind) -> String {
        switch k {
        case .context: "ctx"
        case .add: "add"
        case .del: "del"
        }
    }

    private func check(_ line: DiffLine, _ want: JSONValue, _ context: String) {
        XCTAssertEqual(kind(line.kind), want["k"]?.string, context)
        XCTAssertEqual(line.old, want["o"]?.int, context)
        XCTAssertEqual(line.new, want["n"]?.int, context)
        XCTAssertEqual(line.text, want["t"]?.string, context)
        let marks = (want["m"]?.array ?? []).map { pair -> Range<Int> in
            let p = pair.array ?? []
            return (p[0].int ?? 0)..<(p[1].int ?? 0)
        }
        XCTAssertEqual(line.marks, marks, "marks: \(context)")
    }

    func testPatchParsingMatchesTheWeb() throws {
        for case let item in try golden()["patches"]?.array ?? [] {
            let hunks = DiffParser.parse(item["patch"]?.string ?? "")
            let want = item["hunks"]?.array ?? []
            XCTAssertEqual(hunks.count, want.count)
            for (h, w) in zip(hunks, want) {
                XCTAssertEqual(h.oldStart, w["oldStart"]?.int)
                XCTAssertEqual(h.newStart, w["newStart"]?.int)
                XCTAssertEqual(h.section, w["section"]?.string)
                XCTAssertEqual(h.gap, w["gap"]?.int)
                XCTAssertEqual(DiffParser.splitRows(h.lines).count, w["split"]?.int)
                let lines = w["lines"]?.array ?? []
                XCTAssertEqual(h.lines.count, lines.count)
                for (l, wl) in zip(h.lines, lines) { check(l, wl, l.text) }
            }
        }
    }

    func testHidingWhitespaceMatchesTheWeb() throws {
        for item in try golden()["whitespace"]?.array ?? [] {
            let hunks = DiffParser.parse(item["patch"]?.string ?? "", hideWhitespace: true)
            let want = item["hunks"]?.array ?? []
            XCTAssertEqual(hunks.count, want.count)
            for (h, w) in zip(hunks, want) {
                let lines = w["lines"]?.array ?? []
                XCTAssertEqual(h.lines.count, lines.count)
                for (l, wl) in zip(h.lines, lines) { check(l, wl, l.text) }
            }
        }
    }

    func testWordDiffMatchesTheWeb() throws {
        for item in try golden()["words"]?.array ?? [] {
            let a = item["a"]?.string ?? "", b = item["b"]?.string ?? ""
            let got = DiffParser.wordDiff(a, b)
            let r = item["r"]
            if r?.isNull ?? true {
                XCTAssertNil(got, "\(a) / \(b)")
                continue
            }
            func ranges(_ v: JSONValue?) -> [Range<Int>] {
                (v?.array ?? []).map { ($0.array?[0].int ?? 0)..<($0.array?[1].int ?? 0) }
            }
            XCTAssertEqual(got?.0, ranges(r?.array?[0]), "\(a) / \(b)")
            XCTAssertEqual(got?.1, ranges(r?.array?[1]), "\(a) / \(b)")
        }
    }

    func testStatBlocksMatchTheWeb() throws {
        for item in try golden()["blocks"]?.array ?? [] {
            let r = DiffParser.statBlocks(add: item["a"]?.int ?? 0, del: item["d"]?.int ?? 0)
            let w = item["r"]?.array?.map { $0.int ?? -1 } ?? []
            XCTAssertEqual([r.green, r.red, r.grey], w)
        }
    }

    func testFormattersMatchTheWeb() throws {
        let f = try golden()["format"]
        func pairs(_ key: String) -> [(String, String)] {
            (f?[key]?.array ?? []).map { ($0.array?[0].string ?? "", $0.array?[1].string ?? "") }
        }
        for (i, o) in pairs("modelName") { XCTAssertEqual(Format.modelName(i), o, i) }
        for (i, o) in pairs("modelNameOpencode") { XCTAssertEqual(Format.modelName(i, agent: .opencode), o, i) }
        for (i, o) in pairs("picker") { XCTAssertEqual(Format.pickerModelName(i), o, i) }
        for (i, o) in pairs("repoLabel") { XCTAssertEqual(Format.repoLabel(i), o, i) }

        for item in f?["valid"]?.array ?? [] {
            let a = item.array ?? []
            let agent = AgentID(rawValue: a[0].string ?? "") ?? .claude
            XCTAssertEqual(Format.modelValid(agent: agent, model: a[1].string ?? ""), a[2].bool, "\(agent) \(a[1])")
        }

        let now = Timestamp.parse("2026-11-07T12:00:00Z")!
        for item in f?["rel"]?.array ?? [] {
            let a = item.array ?? []
            XCTAssertEqual(Format.rel(now.addingTimeInterval(-(a[0].double ?? 0)), now: now), a[1].string)
        }
        for item in f?["until"]?.array ?? [] {
            let a = item.array ?? []
            XCTAssertEqual(Format.until(now.addingTimeInterval(a[0].double ?? 0), now: now), a[1].string)
        }
        for item in f?["agoShort"]?.array ?? [] {
            let a = item.array ?? []
            XCTAssertEqual(Format.ago(now.addingTimeInterval(-(a[0].double ?? 0)), now: now), a[1].string)
        }
        for item in f?["uptime"]?.array ?? [] {
            let a = item.array ?? []
            XCTAssertEqual(Format.uptime(a[0].int ?? 0), a[1].string)
        }
        for item in f?["money"]?.array ?? [] {
            let a = item.array ?? []
            XCTAssertEqual(Format.money(a[0].double, digits: a[1].int ?? 2), a[2].string)
        }
        for item in f?["pct"]?.array ?? [] {
            let a = item.array ?? []
            XCTAssertEqual(Format.pct(a[0].double ?? 0, a[1].double ?? 0), a[2].int)
        }
    }
}
