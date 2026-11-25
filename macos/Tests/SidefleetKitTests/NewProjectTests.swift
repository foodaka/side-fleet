import Foundation
import XCTest
@testable import SidefleetKit

/// New project, against the demo backend. (Async throughout: on Linux, XCTest can't call a
/// synchronous test on a main-actor class.)
@MainActor
final class NewProjectTests: XCTestCase {
    private func world() async -> (FleetStore, NewProjectModel) {
        let name = "sf-test-\(UUID().uuidString)"
        let d = UserDefaults(suiteName: name)!
        d.removePersistentDomain(forName: name)
        let fleet = FleetStore(api: API(backend: DemoBackend(speed: 50)), toasts: ToastCenter(), defaults: d)
        await fleet.refresh()
        return (fleet, NewProjectModel(fleet: fleet, defaults: d))
    }

    func testFolderNamesMatchThePage() async {
        XCTAssertEqual(NewProject.folder("Recipe Box"), "Recipe-Box")
        XCTAssertEqual(NewProject.folder("  my app!? "), "my-app")
        XCTAssertEqual(NewProject.folder("..hidden"), "hidden")
        XCTAssertEqual(NewProject.folder("café"), "caf")
        XCTAssertEqual(NewProject.folder(fromURL: "git@github.com:acme/web-app.git"), "web-app")
        XCTAssertEqual(NewProject.folder(fromURL: "https://gitlab.com/acme/tools/"), "tools")
        XCTAssertEqual(NewProject.path(root: "/home/dev/code/", folder: "x"), "/home/dev/code/x")
        XCTAssertEqual(NewProject.path(root: "/home/dev/code", folder: ""), "/home/dev/code/…")
    }

    func testOnlyMachinesThatCanAreOffered() async {
        let (_, m) = await world()
        // The MacBook is offline and too old; the other two can.
        XCTAssertEqual(m.able.map(\.id).sorted(), ["hetzner-vps", "mac-mini"])
        m.machineID = "mac-mini"
        m.name = "Recipe Box"
        XCTAssertEqual(m.path, "/home/dev/code/Recipe-Box")
        XCTAssertTrue(m.renamed)
        XCTAssertTrue(m.canSubmit)
        m.name = "storefront"
        XCTAssertTrue(m.taken, "the machine has a storefront folder already")
        XCTAssertFalse(m.canSubmit)
    }

    func testAPastedURLNamesTheFolderUntilYouType() async {
        let (_, m) = await world()
        m.mode = .clone
        m.url = "git@github.com:acme/weekend-game.git"
        XCTAssertEqual(m.name, "weekend-game")
        m.name = "game"
        m.url = "git@github.com:acme/other.git"
        XCTAssertEqual(m.name, "game", "a typed name stays")
        m.reset()
        m.url = "git@github.com:acme/other.git"
        XCTAssertEqual(m.name, "other")
    }

    func testCreatingOneListsItOnTheMachine() async {
        let (fleet, m) = await world()
        m.machineID = "mac-mini"
        m.name = "recipe-box"
        let s = await m.submit(pollEvery: .milliseconds(1))
        XCTAssertEqual(s?.repo, "recipe-box")
        XCTAssertEqual(s?.path, "/home/dev/code/recipe-box")
        XCTAssertEqual(m.error, "")
        let mini = fleet.workers.first { $0.id == "mac-mini" }
        XCTAssertTrue(mini?.capabilities.repos.contains { $0.name == "recipe-box" } ?? false)
        // The same name again is refused by the form.
        m.name = "recipe-box"
        XCTAssertTrue(m.taken)
    }
}
