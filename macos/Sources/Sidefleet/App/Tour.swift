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
            let stops: [(String, Route)] = [
                ("new-task", .newTask),
                ("thread-finished", .thread("demo-checkout")),
                ("thread-approval", .thread("demo-rate")),
                ("thread-live", .thread("demo-guide")),
                ("thread-failed", .thread("demo-stripe")),
                ("thread-scheduled", .thread("demo-audit")),
                // An open pull request: a failed check, changes requested, a conflict.
                ("thread-pr", .thread("demo-coupons")),
                // The sheet that picks the files and the title before a pull request opens.
                ("pr-sheet", .thread("demo-checkout")),
                ("pull-requests", .pullRequests),
                // The sidebar By project, then the icon picker over it.
                ("sidebar-projects", .overview),
                ("machines", .machines),
                ("overview", .overview),
                ("rooms", .rooms),
                ("room", .room("room-applepay")),
                ("team", .team),
                ("chat-apps", .chatApps),
            ]
            try? await Task.sleep(for: .seconds(2))
            for (name, route) in stops {
                if name == "sidebar-projects" { stores.fleet.projects.setView(.project) }
                if name == "pr-sheet" { stores.prSheetThread = "demo-checkout" }
                stores.route = route
                try? await Task.sleep(for: .seconds(name == "sidebar-projects" ? 3.5 : 2.5))
                if name == "pr-sheet" {
                    captureSheet(folder.appendingPathComponent("\(name)-\(suffix).png"))
                    // Close it: the next stop is another screen.
                    if let w = mainWindow, let sheet = w.attachedSheet { w.endSheet(sheet) }
                    try? await Task.sleep(for: .seconds(1))
                    continue
                }
                capture(folder.appendingPathComponent("\(name)-\(suffix).png"))
                if name == "new-task" {
                    // The menu of skills a slash opens.
                    stores.newTask.task = "/"
                    try? await Task.sleep(for: .seconds(2.5))
                    capture(folder.appendingPathComponent("new-task-slash-\(suffix).png"))
                    stores.newTask.task = ""
                }
                if name == "sidebar-projects" {
                    stores.fleet.projects.picking = "github.com/acme/storefront"
                    try? await Task.sleep(for: .seconds(2.5))
                    captureSheet(folder.appendingPathComponent("project-icon-\(suffix).png"))
                    stores.fleet.projects.picking = nil
                    // Back to the default, which the rest of the tour (and the next run) expects.
                    stores.fleet.projects.setView(.time)
                    try? await Task.sleep(for: .seconds(1))
                }
            }
            NSApp.terminate(nil)
        }
    }

    private static var mainWindow: NSWindow? {
        NSApp.windows.first { $0.isVisible && $0.contentView != nil && $0.frame.width > 400 && $0.sheetParent == nil }
    }

    /// The sheet open on the main window, which the window's own picture leaves out; the window if there is none.
    private static func captureSheet(_ url: URL) {
        capture(url, window: mainWindow?.attachedSheet ?? mainWindow)
    }

    private static func capture(_ url: URL) {
        capture(url, window: mainWindow)
    }

    private static func capture(_ url: URL, window: NSWindow?) {
        guard let window else { return }
        // The window server's picture, because `cacheDisplay` leaves out the sidebar's glass and the
        // toolbar and sometimes comes back blank. An app's own windows need no screen recording permission.
        guard let image = CGWindowListCreateImage(.null, .optionIncludingWindow, CGWindowID(window.windowNumber), [.boundsIgnoreFraming, .bestResolution]) else { return }
        try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: url)
    }
}
