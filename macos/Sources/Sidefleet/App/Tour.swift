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
                ("machines", .machines),
                ("overview", .overview),
            ]
            try? await Task.sleep(for: .seconds(2))
            for (name, route) in stops {
                stores.route = route
                try? await Task.sleep(for: .seconds(2.5))
                capture(folder.appendingPathComponent("\(name)-\(suffix).png"))
            }
            NSApp.terminate(nil)
        }
    }

    private static func capture(_ url: URL) {
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil && $0.frame.width > 400 }), let view = window.contentView else { return }
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: url)
    }
}
