import AppKit
import Combine
import Sparkle

/// Sparkle: checks the release feed in the background and offers a newer version, which it downloads,
/// installs and relaunches into. Only for the bundled app: `swift run` has no Info.plist to read the
/// feed and key from, and demo mode must stay off the network.
@MainActor
final class Updater: ObservableObject {
    static let shared = Updater()

    @Published private(set) var canCheck = false
    private let controller: SPUStandardUpdaterController?

    private init() {
        let bundled = Bundle.main.bundleURL.pathExtension == "app"
            && Bundle.main.object(forInfoDictionaryKey: "SUPublicEDKey") != nil
        guard bundled, !CommandLine.arguments.contains("--demo") else {
            controller = nil
            return
        }
        let controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil)
        self.controller = controller
        controller.updater.publisher(for: \.canCheckForUpdates).receive(on: RunLoop.main).assign(to: &$canCheck)
    }

    var available: Bool { controller != nil }

    func check() { controller?.checkForUpdates(nil) }
}
