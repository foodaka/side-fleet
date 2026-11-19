import Foundation
import XCTest
@testable import SidefleetKit

/// The menu after `/`, against the demo backend.
@MainActor
final class SlashMenuTests: XCTestCase {
    private let api = API(backend: DemoBackend(speed: 50))

    private func loaded(_ menu: SlashMenuModel) async {
        let end = Date().addingTimeInterval(10)
        while menu.commands == nil, Date() < end { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertNotNil(menu.commands, "never loaded")
    }

    func testOnlyAWordAfterTheSlashOpensIt() {
        XCTAssertEqual(SlashMenuModel.query("/"), "")
        XCTAssertEqual(SlashMenuModel.query("/rev"), "rev")
        XCTAssertEqual(SlashMenuModel.query("/stripe:test-cards"), "stripe:test-cards")
        XCTAssertNil(SlashMenuModel.query("/review now"))
        XCTAssertNil(SlashMenuModel.query("fix /tmp"))
        XCTAssertNil(SlashMenuModel.query("/tmp/x"))
        XCTAssertNil(SlashMenuModel.query(""))
    }

    func testMatchesNamesBeforePartsBeforeDescriptions() {
        let list = [
            SlashCommand(name: "code-review"),
            SlashCommand(name: "ship", description: "Run the tests, then review"),
            SlashCommand(name: "review"),
            SlashCommand(name: "preview"),
            SlashCommand(name: "qa"),
        ]
        XCTAssertEqual(SlashMenuModel.match(list, query: "Rev").map(\.name), ["review", "code-review", "preview", "ship"])
        XCTAssertEqual(SlashMenuModel.match(list, query: "").count, 5)
    }

    func testKeysMoveAndPick() async {
        let menu = SlashMenuModel()
        menu.update(text: "/", worker: "mac-mini", repo: "github.com/acme/storefront", enabled: true, api: api)
        XCTAssertTrue(menu.isOpen)
        await loaded(menu)
        XCTAssertEqual(menu.matches.prefix(3).map(\.name), ["deploy", "fix-flaky", "review"], "the repo's own first")
        XCTAssertEqual(menu.handle(.down), .handled)
        XCTAssertEqual(menu.handle(.down), .handled)
        XCTAssertEqual(menu.handle(.enter), .replace("/review "))
        XCTAssertEqual(menu.handle(.up), .handled)
        XCTAssertEqual(menu.active, 1)

        menu.update(text: "/review", worker: "mac-mini", repo: "github.com/acme/storefront", enabled: true, api: api)
        XCTAssertEqual(menu.active, 0, "a new query starts at the top")
        XCTAssertEqual(menu.handle(.enter), .pass, "Enter on a command typed in full sends it")
        XCTAssertEqual(menu.handle(.tab), .replace("/review "))

        XCTAssertEqual(menu.handle(.escape), .handled)
        XCTAssertFalse(menu.isOpen)
        XCTAssertEqual(menu.handle(.down), .pass)
        menu.update(text: "/revie", worker: "mac-mini", repo: "github.com/acme/storefront", enabled: true, api: api)
        XCTAssertTrue(menu.isOpen, "typing again brings it back")

        menu.update(text: "/review it", worker: "mac-mini", repo: "github.com/acme/storefront", enabled: true, api: api)
        XCTAssertFalse(menu.isOpen)
        XCTAssertEqual(menu.handle(.enter), .pass)
    }

    func testOnlyForClaudeCodeOnAMachine() {
        let menu = SlashMenuModel()
        menu.update(text: "/", worker: "mac-mini", repo: "", enabled: false, api: api)
        XCTAssertFalse(menu.isOpen)
        menu.update(text: "/", worker: nil, repo: "", enabled: true, api: api)
        XCTAssertFalse(menu.isOpen)
    }

    func testAnotherRepoHasNoneOfItsOwn() async {
        let menu = SlashMenuModel()
        menu.update(text: "/", worker: "mac-mini", repo: "github.com/acme/docs", enabled: true, api: api)
        await loaded(menu)
        XCTAssertEqual(menu.matches.first?.name, "review")
    }
}
