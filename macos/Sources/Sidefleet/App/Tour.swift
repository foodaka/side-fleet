import AppKit
import SidefleetKit
import SwiftUI

/// `--demo --tour <folder> [--dark]`: walks the screens and saves a PNG of the window at each stop, so
/// how the app looks can be checked and shown (a build machine, a pull request) without anyone clicking.
@MainActor
enum Tour {
    private static var started = false

    static func startIfAsked(stores: Stores) {
        let args = CommandLine.arguments
        guard !started, stores.isDemo, let i = args.firstIndex(of: "--tour"), i + 1 < args.count else { return }
        started = true
        let folder = URL(fileURLWithPath: args[i + 1], isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        if args.contains("--dark") { stores.prefs.appearance = .dark }
        if args.contains("--light") { stores.prefs.appearance = .light }
        let suffix = args.contains("--dark") ? "dark" : "light"

        Task {
            let demo = stores.api.backend as? DemoBackend
            let stops: [(String, Route, (@MainActor () async -> Void)?)] = [
                ("new-task", .newTask, nil),
                // A task with a file and a picture attached, before it's sent.
                ("new-task-files", .newTask, {
                    stores.newTask.task = "Follow the plan in plan.md and match the screenshot"
                    stores.newTask.attachments = DemoBackend.composerAttachments
                }),
                ("thread-finished", .thread("demo-checkout"), {
                    stores.newTask.task = ""
                    stores.newTask.attachments = []
                }),
                ("thread-approval", .thread("demo-rate"), nil),
                ("thread-live", .thread("demo-guide"), nil),
                ("thread-failed", .thread("demo-stripe"), nil),
                // A reply that couldn't go: it stays in the thread with Retry and Discard.
                ("thread-send-failed", .thread("demo-tax"), {
                    await demo?.setFailingMessages(true)
                    stores.tourReply = "Also check the shipping tax for Canada"
                }),
                ("thread-scheduled", .thread("demo-audit"), { await demo?.setFailingMessages(false) }),
                ("machines", .machines, nil),
                ("overview", .overview, nil),
                ("rooms", .rooms, nil),
                ("room", .room("room-applepay"), nil),
                ("team", .team, nil),
                ("chat-apps", .chatApps, nil),
                // Last: the first load failing, as when the control plane is down.
                ("connection-error", .newTask, {
                    await demo?.setUnreachable(true)
                    stores.fleet.switchOrg(stores.fleet.orgID)
                }),
            ]
            try? await Task.sleep(for: .seconds(2))
            for (name, route, setup) in stops {
                await setup?()
                stores.route = route
                try? await Task.sleep(for: .seconds(2.5))
                capture(folder.appendingPathComponent("\(name)-\(suffix).png"))
            }
            NSApp.terminate(nil)
        }
    }

    private static func capture(_ url: URL) {
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil && $0.frame.width > 400 }) else { return }
        // The window server's picture, because `cacheDisplay` leaves out the sidebar's glass and the
        // toolbar and sometimes comes back blank. An app's own windows need no screen recording permission.
        guard let image = CGWindowListCreateImage(.null, .optionIncludingWindow, CGWindowID(window.windowNumber), [.boundsIgnoreFraming, .bestResolution]) else { return }
        try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: url)
    }
}
