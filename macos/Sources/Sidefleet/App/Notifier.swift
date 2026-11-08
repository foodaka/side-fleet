import AppKit
import SidefleetKit
import UserNotifications

/// Local notifications from the inbox stream. They carry the task's title and the reason, never agent output.
@MainActor
final class Notifier: NSObject {
    private var requested = false

    /// Only a real app bundle has a notification center: `swift run` would crash here.
    private var center: UNUserNotificationCenter? {
        Bundle.main.bundleIdentifier == nil ? nil : .current()
    }

    func requestPermission() {
        guard let center, !requested else { return }
        requested = true
        center.requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    func post(_ p: NotificationPlanner.Post) {
        guard let center else { return }
        requestPermission()
        let content = UNMutableNotificationContent()
        content.title = p.title
        content.body = p.body
        content.sound = .default
        content.threadIdentifier = p.threadID
        content.userInfo = ["thread": p.threadID]
        center.add(UNNotificationRequest(identifier: "\(p.threadID)-\(UUID().uuidString)", content: content, trigger: nil))
    }
}
