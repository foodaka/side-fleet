import Foundation
import XCTest
@testable import SidefleetKit

/// The list By project and its icons, against the demo backend. (Every test is async: on Linux, XCTest
/// can't call a synchronous one on a main-actor class.)
@MainActor
final class ProjectsTests: XCTestCase {
    private func defaults() -> UserDefaults {
        let name = "sf-test-\(UUID().uuidString)"
        let d = UserDefaults(suiteName: name)!
        d.removePersistentDomain(forName: name)
        return d
    }

    private func world(_ d: UserDefaults? = nil) -> (FleetStore, UserDefaults) {
        let d = d ?? defaults()
        return (FleetStore(api: API(backend: DemoBackend(speed: 50)), toasts: ToastCenter(), defaults: d), d)
    }

    func testDefaultIconsMatchThePage() async {
        // Worked out with the page's own hash (projects.svelte.ts) in Node.
        XCTAssertEqual(ProjectIcon.hash("github.com/acme/side-fleet"), 4_086_088_571)
        XCTAssertEqual(ProjectIcon.defaultIcon("github.com/acme/side-fleet"), ProjectIcon(kind: .letter, value: "#f08a2e"))
        XCTAssertEqual(ProjectIcon.defaultIcon("github.com/acme/cli").value, "#7c5cff")
        XCTAssertEqual(ProjectIcon.defaultIcon("github.com/acme/storefront").value, "#12a5b0")
        XCTAssertEqual(ProjectIcon.defaultIcon("proj").value, "#d4a20a")
        XCTAssertEqual(ProjectIcon.hash("github.com/acme/café-🚀"), 529_420_084, "outside the BMP only the high surrogate counts, as on the page")
        XCTAssertEqual(ProjectIcon.defaultIcon("github.com/acme/café-🚀").value, "#d6409f")
    }

    func testNamesLettersAndValues() async {
        XCTAssertEqual(ProjectIcon.name("github.com/acme/side-fleet"), "side-fleet")
        XCTAssertEqual(ProjectIcon.name("proj"), "proj")
        XCTAssertEqual(ProjectIcon.letter("github.com/acme/storefront"), "S")
        XCTAssertEqual(ProjectIcon(kind: .letter, value: "#22a06b").rgb, 0x22a06b)
        XCTAssertNil(ProjectIcon(kind: .letter, value: "green").rgb)
        XCTAssertNil(ProjectIcon(kind: .emoji, value: "🚀").rgb)
        let png = Data([0x89, 0x50, 0x4e, 0x47])
        let image = ProjectIcon.image(png: png)
        XCTAssertEqual(image?.value, "data:image/png;base64,iVBORw==")
        XCTAssertEqual(image?.imageData, png)
        XCTAssertNil(ProjectIcon.image(png: Data(count: 40 * 1024)), "too big for the control plane")
        XCTAssertNil(ProjectIcon(kind: .image, value: "https://example.com/a.png").imageData)
        XCTAssertEqual(ProjectIcon.firstEmoji("  🦊 and more"), "🦊")
        XCTAssertEqual(ProjectIcon.firstEmoji("🇳🇱"), "🇳🇱")
        XCTAssertEqual(ProjectIcon.firstEmoji("⚡️"), "⚡️")
        XCTAssertNil(ProjectIcon.firstEmoji("a🦊"))
        XCTAssertNil(ProjectIcon.firstEmoji("1"))
        XCTAssertNil(ProjectIcon.firstEmoji("#"))
        XCTAssertNil(ProjectIcon.firstEmoji(""))
    }

    func testStoredIconsDecodeAndSkipUnknownKinds() async throws {
        let json = #"[{"repo":"a","kind":"emoji","value":"🚀","updated_at":"2026-11-20T10:00:00.123456Z"},{"repo":"b","kind":"sticker","value":"x","updated_at":"2026-11-20T10:00:00Z"}]"#
        let list = try JSONDecoder().decode([StoredProjectIcon].self, from: Data(json.utf8))
        XCTAssertEqual(list.first?.icon, ProjectIcon(kind: .emoji, value: "🚀"))
        XCTAssertNotNil(list.first?.updatedAt)
        XCTAssertNil(list.last?.icon)
    }

    func testGroupsGoByUrgencyThenLatest() async {
        let (fleet, _) = world()
        await fleet.refresh()
        let s = SidebarSections(runs: fleet.runs, userID: nil, teamScope: true, filtering: false, now: Date())
        XCTAssertEqual(s.projects.map(\.repo), ["github.com/acme/storefront", "github.com/acme/docs", "github.com/acme/cli"], "waiting on you, then running, then the rest")
        let store = s.projects[0]
        XCTAssertEqual(store.urgency, 0)
        XCTAssertEqual(store.waiting, 1)
        XCTAssertEqual(store.running, 0, "the one waiting on you isn't counted as running too")
        XCTAssertEqual(store.runs.first?.id, "demo-rate")
        XCTAssertFalse(store.runs.contains { $0.id == "demo-images" }, "pinned stays in Pinned")
        XCTAssertFalse(store.runs.contains { $0.id == "demo-review" }, "snoozed stays apart")
        XCTAssertEqual(store.runs.count, 7)
        XCTAssertEqual(store.visible(showAll: false, openID: nil).count, ProjectGroup.shown)
        XCTAssertEqual(store.visible(showAll: false, openID: "demo-contrast").last?.id, "demo-contrast", "the open thread is always shown")
        XCTAssertEqual(store.visible(showAll: true, openID: nil).count, 7)
        let finished = store.runs.filter { !$0.isLive }
        XCTAssertEqual(finished.map(\.lastActivityAt), finished.map(\.lastActivityAt).sorted(by: >), "then the latest first")
        XCTAssertEqual(s.projects[1].running, 1)
        XCTAssertEqual(s.projects[1].urgency, 1)
        XCTAssertEqual(s.projects[2].urgency, 4)

        let order = s.projectOrder(hidden: ["github.com/acme/docs"], folded: ["github.com/acme/cli"], showAll: [], openID: nil)
        XCTAssertEqual(order.first?.id, "demo-images", "pinned first")
        XCTAssertEqual(order.count, 1 + ProjectGroup.shown, "hidden and folded groups leave their rows out")
    }

    func testViewFoldsAndHiddenAreRemembered() async {
        let d = defaults()
        let (fleet, _) = world(d)
        let p = fleet.projects
        XCTAssertEqual(p.view, .time, "by time until you choose")
        p.setView(.project)
        p.toggleFolded("a")
        p.toggleShowAll("b")
        p.hide("c")
        p.hide("d")
        let again = ProjectsStore(api: fleet.api, toasts: ToastCenter(), defaults: d)
        XCTAssertEqual(again.view, .project)
        XCTAssertEqual(again.folded, ["a"])
        XCTAssertEqual(again.showAll, ["b"])
        XCTAssertEqual(again.hidden, ["c", "d"])
        XCTAssertEqual(d.string(forKey: "sidefleet_runs_view"), "project", "the page's key")
        again.toggleFolded("a")
        again.unhideAll()
        let third = ProjectsStore(api: fleet.api, toasts: ToastCenter(), defaults: d)
        XCTAssertTrue(third.folded.isEmpty)
        XCTAssertTrue(third.hidden.isEmpty)
    }

    func testIconsLoadWithTheFleetAndSaveForTheOrg() async {
        let (fleet, _) = world()
        await fleet.refresh()
        let p = fleet.projects
        XCTAssertEqual(p.icon("github.com/acme/storefront"), ProjectIcon(kind: .emoji, value: "🚀"))
        XCTAssertEqual(p.icon("github.com/acme/docs"), ProjectIcon(kind: .letter, value: "#22a06b"))
        XCTAssertEqual(p.icon("github.com/acme/cli"), ProjectIcon.defaultIcon("github.com/acme/cli"), "none set: the default")

        await p.setIcon("github.com/acme/cli", ProjectIcon(kind: .emoji, value: "🦊"))
        XCTAssertEqual(p.icons["github.com/acme/cli"]?.value, "🦊")
        await p.load(force: true)
        XCTAssertEqual(p.icons["github.com/acme/cli"]?.value, "🦊", "the backend kept it")
        await p.setIcon("github.com/acme/cli", nil)
        XCTAssertNil(p.icons["github.com/acme/cli"])

        // A malformed icon is turned away: the list goes back to what it was, with an error.
        await p.setIcon("github.com/acme/docs", ProjectIcon(kind: .emoji, value: ""))
        XCTAssertEqual(p.icon("github.com/acme/docs"), ProjectIcon(kind: .letter, value: "#22a06b"))
        XCTAssertEqual(fleet.toasts.toasts.last?.title, "Could not set the icon")

        fleet.switchOrg("other")
        XCTAssertTrue(p.icons.isEmpty, "another org's icons are its own")
    }

    func testNewTaskInARepoOffersIt() async {
        let (fleet, d) = world()
        await fleet.refresh()
        let model = NewTaskModel(fleet: fleet, prefs: PrefsStore(defaults: d), defaults: d)
        try? await fleet.saveAdded(["github.com/acme/storefront"])
        XCTAssertEqual(model.repos, ["github.com/acme/storefront"])
        model.compose(in: "github.com/acme/docs")
        XCTAssertEqual(model.repo, "github.com/acme/docs")
        XCTAssertEqual(model.repos, ["github.com/acme/docs", "github.com/acme/storefront"], "offered though it isn't one of the chosen")
        model.syncSelection()
        XCTAssertEqual(model.repo, "github.com/acme/docs", "and it stays chosen")
    }
}
