import AppKit
import SidefleetKit
import SwiftUI
import UserNotifications

@main
struct SidefleetApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var session = AppSession()
    @StateObject private var updater = Updater.shared

    var body: some Scene {
        Window("Sidefleet", id: "main") {
            RootView(session: session)
                .onAppear { delegate.session = session }
                .onOpenURL { open($0) }
        }
        .defaultSize(width: 1280, height: 820)
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands {
            SidefleetCommands(session: session)
            UpdateCommands(updater: updater)
        }

        WindowGroup("Task", id: "thread", for: ThreadWindowID.self) { $window in
            if let id = window?.id, let stores = session.stores {
                ThreadScreen(runID: id)
                    .environment(stores)
                    .preferredColorScheme(stores.prefs.appearance.colorScheme)
                    .frame(minWidth: 620, minHeight: 480)
                    .overlay(alignment: .bottomTrailing) { ToastOverlay(center: stores.toasts) }
            }
        }
        .defaultSize(width: 820, height: 820)

        Settings {
            SettingsView(session: session)
        }

        MenuBarExtra {
            MenuBarContent(session: session)
        } label: {
            MenuBarLabel(session: session)
        }
        .menuBarExtraStyle(.window)
    }

    /// `sidefleet://runs/<id>`, as notifications and anything else link to a task; `sidefleet://rooms/<id>`
    /// for a room and `sidefleet://join/<code>` for an invite.
    private func open(_ url: URL) {
        guard url.scheme == "sidefleet", let id = url.pathComponents.last, id != "/", let stores = session.stores else { return }
        switch url.host {
        case "runs": stores.open(id)
        case "rooms": stores.route = .room(id)
        case "join": stores.route = .join(id)
        default: break
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    @MainActor var session: AppSession?

    func applicationDidFinishLaunching(_ note: Notification) {
        // `swift run` has no bundle, and with no bundle there's no notification center.
        if Bundle.main.bundleIdentifier != nil { UNUserNotificationCenter.current().delegate = self }
        let center = NSWorkspace.shared.notificationCenter
        center.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.session?.stores?.didWake() }
        }
    }

    func applicationDidBecomeActive(_ note: Notification) {
        MainActor.assumeIsolated { session?.stores?.setAppActive(true) }
    }

    func applicationDidResignActive(_ note: Notification) {
        MainActor.assumeIsolated { session?.stores?.setAppActive(false) }
    }

    /// Closing the window leaves it in the menu bar, where it can still notify you. Unless you'd rather not.
    func applicationShouldTerminateAfterLastWindowClosed(_ app: NSApplication) -> Bool {
        MainActor.assumeIsolated { !(session?.stores?.prefs.keepRunningInMenuBar ?? false) }
    }

    /// Show notifications even while the app is in front: the task isn't necessarily the one on screen.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification, withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .sound])
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse, withCompletionHandler done: @escaping () -> Void) {
        let thread = response.notification.request.content.userInfo["thread"] as? String
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                if let thread { self.session?.stores?.open(thread) }
                NSApp.activate(ignoringOtherApps: true)
                NSApp.windows.first { $0.identifier?.rawValue == "main" }?.makeKeyAndOrderFront(nil)
            }
            done()
        }
    }
}

// MARK: Root

struct RootView: View {
    var session: AppSession

    var body: some View {
        Group {
            if let stores = session.stores, !stores.fleet.needsSignIn {
                MainView()
                    .environment(stores)
                    .preferredColorScheme(stores.prefs.appearance.colorScheme)
                    .task { Tour.startIfAsked(stores: stores) }
            } else {
                SignInScreen(session: session)
            }
        }
        .frame(minWidth: 900, minHeight: 600)
    }
}

private struct MainView: View {
    @Environment(Stores.self) private var stores
    @State private var columns = NavigationSplitViewVisibility.all

    var body: some View {
        @Bindable var stores = stores
        NavigationSplitView(columnVisibility: $columns) {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 220, ideal: SF.sidebarWidth, max: 400)
        } detail: {
            VStack(spacing: 0) {
                if stores.fleet.conn == .unreachable, stores.fleet.loaded {
                    Text("Lost contact with the control plane. Retrying every few seconds; what you see may be out of date.")
                        .font(SF.font(12))
                        .foregroundStyle(SF.queueInk)
                        .frame(maxWidth: .infinity)
                        .padding(8)
                        .background(SF.queueSoft)
                        .overlay(alignment: .bottom) { Rectangle().fill(SF.queue.opacity(0.3)).frame(height: 1) }
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
                switch stores.route {
                case .newTask:
                    NewTaskScreen()
                case .overview:
                    OverviewScreen()
                case .machines:
                    MachinesScreen()
                case .rooms:
                    RoomsScreen()
                case .room(let id):
                    RoomScreen(roomID: id).id(id)
                case .team:
                    TeamScreen()
                case .chatApps:
                    ChatAppsScreen()
                case .join(let code):
                    JoinScreen(code: code).id(code)
                case .thread(let id):
                    ThreadScreen(runID: id).id(id)
                }
            }
            .animation(.easeOut(duration: 0.2), value: stores.fleet.conn)
            .overlay(alignment: .bottomTrailing) { ToastOverlay(center: stores.toasts) }
            .navigationTitle(title)
        }
        // ⌘B, as on the page; the system's own ⌃⌘S works too.
        .focusedSceneValue(\.toggleSidebar) {
            withAnimation { columns = columns == .detailOnly ? .all : .detailOnly }
        }
        .sheet(isPresented: $stores.repoPickerOpen) { RepoPickerSheet() }
        .sheet(isPresented: $stores.addMachineOpen) { AddMachineSheet() }
        .sheet(isPresented: $stores.machinesOpen) { MyMachinesSheet() }
        .sheet(isPresented: $stores.renameOrgOpen) { RenameOrgSheet() }
        .sheet(isPresented: $stores.newOrgOpen) { NewOrgSheet() }
        .sheet(isPresented: $stores.feedbackOpen) { FeedbackSheet() }
    }

    private var title: String {
        switch stores.route {
        case .newTask: "New task"
        case .overview: "Overview"
        case .machines: "Machines"
        case .rooms, .room: "Rooms"
        case .team: "Team"
        case .chatApps: "Chat apps"
        case .join: "Join a team"
        case .thread(let id): stores.fleet.runs.first { $0.id == id }?.run.task ?? "Task"
        }
    }
}

// MARK: Menu bar

struct MenuBarLabel: View {
    var session: AppSession
    @Environment(\.openWindow) private var openWindow
    private static var launched = false

    var body: some View {
        // The count is only what needs you, not everything going on.
        let n = session.stores.map { $0.inbox.items.count + $0.fleet.waitingApprovals } ?? 0
        Image(nsImage: MenuBarMark.image)
            .accessibilityLabel("Sidefleet")
            .task {
                // SwiftUI doesn't present the main window at launch with this set of scenes, and this
                // label is the one view that exists by then.
                guard !Self.launched else { return }
                Self.launched = true
                NSApp.activate(ignoringOtherApps: true)
                openWindow(id: "main")
            }
        if n > 0 {
            Text("\(n)").monospacedDigit()
        }
    }
}

struct MenuBarContent: View {
    var session: AppSession
    @Environment(\.openWindow) private var openWindow

    private func show(_ then: (Stores) -> Void) {
        if let stores = session.stores { then(stores) }
        NSApp.activate(ignoringOtherApps: true)
        openWindow(id: "main")
    }

    var body: some View {
        if let stores = session.stores {
            VStack(alignment: .leading, spacing: 0) {
                let running = stores.fleet.runs.filter { $0.threadStatus == .running }
                HStack {
                    LogoMark(size: 16)
                    Text("Sidefleet").font(SF.font(13, .semibold))
                    Spacer()
                    Text("\(stores.fleet.online.count)/\(stores.fleet.workers.count) online").font(SF.font(11.5)).monospacedDigit().foregroundStyle(SF.muted)
                }
                .padding(12)
                Divider()
                ScrollView {
                    VStack(alignment: .leading, spacing: 1) {
                        if !stores.inbox.items.isEmpty {
                            MenuHeading("Waiting on you")
                            ForEach(stores.inbox.items) { item in
                                MenuRow(title: item.title, detail: item.label, dot: AnyView(Circle().fill(item.reason.toneColor).frame(width: 8, height: 8))) {
                                    show { $0.open(item.id) }
                                }
                            }
                        }
                        if !running.isEmpty {
                            MenuHeading("Running")
                            ForEach(running) { r in
                                MenuRow(title: r.run.task, detail: r.latestWorkerID ?? "", live: r.run.startedAt, dot: AnyView(StatusDot(status: r.pendingApprovals > 0 ? .waiting : .run(.running)))) {
                                    show { $0.open(r.id) }
                                }
                            }
                        }
                        if stores.inbox.items.isEmpty, running.isEmpty {
                            Text("Nothing is running or waiting on you.")
                                .font(SF.font(12))
                                .foregroundStyle(SF.muted)
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 20)
                        }
                    }
                    .padding(4)
                }
                .frame(maxHeight: 380)
                .fixedSize(horizontal: false, vertical: true)
                Divider()
                HStack(spacing: 4) {
                    Button { show { $0.route = .newTask } } label: {
                        Label("New task", systemImage: "plus").font(SF.font(13)).padding(.horizontal, 8).padding(.vertical, 5)
                    }
                    .buttonStyle(QuietButtonStyle(cornerRadius: 6))
                    Spacer()
                    Button { show { _ in } } label: {
                        Text("Open").font(SF.font(13)).padding(.horizontal, 8).padding(.vertical, 5)
                    }
                    .buttonStyle(QuietButtonStyle(cornerRadius: 6))
                    Button { NSApp.terminate(nil) } label: {
                        Text("Quit").font(SF.font(13)).padding(.horizontal, 8).padding(.vertical, 5)
                    }
                    .buttonStyle(QuietButtonStyle(cornerRadius: 6))
                }
                .padding(6)
            }
            .frame(width: 320)
            .environment(stores)
            .preferredColorScheme(stores.prefs.appearance.colorScheme)
        } else {
            VStack(spacing: 8) {
                Text("Not signed in").font(SF.font(13))
                Button("Open Sidefleet") {
                    NSApp.activate(ignoringOtherApps: true)
                    openWindow(id: "main")
                }
                Button("Quit") { NSApp.terminate(nil) }
            }
            .padding(16)
            .frame(width: 220)
        }
    }
}

private struct MenuHeading: View {
    var text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text).font(SF.font(11, .medium)).foregroundStyle(SF.faint).padding(.horizontal, 8).padding(.top, 6).padding(.bottom, 2)
    }
}

private struct MenuRow: View {
    var title: String
    var detail: String
    var live: Date?
    var dot: AnyView
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 10) {
                dot.frame(width: 8, height: 16)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).font(SF.font(13)).lineLimit(1)
                    HStack(spacing: 4) {
                        Text(detail)
                        if let live {
                            LiveText { now in
                                Text("· \(Format.duration(start: live, now: now))").monospacedDigit()
                            }
                        }
                    }
                    .font(SF.font(11.5))
                    .foregroundStyle(SF.muted)
                    .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .hoverBackground()
        }
        .buttonStyle(.plain)
    }
}

// MARK: Commands

/// What the focused thread can do from the menu bar and its shortcuts.
struct ThreadActions {
    var stop: (() -> Void)?
    var review: (() -> Void)?
    var copyBranch: (() -> Void)?
}

private struct ThreadActionsKey: FocusedValueKey { typealias Value = ThreadActions }
private struct FocusSearchKey: FocusedValueKey { typealias Value = () -> Void }
private struct ToggleSidebarKey: FocusedValueKey { typealias Value = () -> Void }
private struct FocusComposerKey: FocusedValueKey { typealias Value = () -> Void }

extension FocusedValues {
    var threadActions: ThreadActions? {
        get { self[ThreadActionsKey.self] }
        set { self[ThreadActionsKey.self] = newValue }
    }

    var focusSearch: (() -> Void)? {
        get { self[FocusSearchKey.self] }
        set { self[FocusSearchKey.self] = newValue }
    }

    var toggleSidebar: (() -> Void)? {
        get { self[ToggleSidebarKey.self] }
        set { self[ToggleSidebarKey.self] = newValue }
    }

    var focusComposer: (() -> Void)? {
        get { self[FocusComposerKey.self] }
        set { self[FocusComposerKey.self] = newValue }
    }
}

/// "Check for Updates…" under About, as Mac apps have it.
struct UpdateCommands: Commands {
    let updater: Updater

    var body: some Commands {
        CommandGroup(after: .appInfo) {
            if updater.available { CheckForUpdatesButton(updater: updater) }
        }
    }
}

/// A view of its own, so the menu item follows `canCheck` as it changes.
private struct CheckForUpdatesButton: View {
    @ObservedObject var updater: Updater

    var body: some View {
        Button("Check for Updates…") { updater.check() }.disabled(!updater.canCheck)
    }
}

@MainActor
struct SidefleetCommands: Commands {
    var session: AppSession
    @FocusedValue(\.threadActions) private var thread
    @FocusedValue(\.focusSearch) private var focusSearch
    @FocusedValue(\.toggleSidebar) private var toggleSidebar
    @FocusedValue(\.focusComposer) private var focusComposer
    @Environment(\.openWindow) private var openWindow

    private var stores: Stores? { session.stores }

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Task") {
                stores?.route = .newTask
                openWindow(id: "main")
                focusComposer?()
            }
            .keyboardShortcut("n")
            .disabled(stores == nil)
        }
        CommandGroup(after: .sidebar) {
            Divider()
            Button("New Task") { stores?.route = .newTask }.keyboardShortcut("1")
            Button("Overview") { stores?.route = .overview }.keyboardShortcut("2")
            Button("Machines") { stores?.route = .machines }.keyboardShortcut("3")
            Button("Rooms") { stores?.route = .rooms }.keyboardShortcut("4")
            Button("Chat Apps") { stores?.route = .chatApps }.keyboardShortcut("5")
            if stores?.fleet.me?.user != nil {
                Button("Team") { stores?.route = .team }.keyboardShortcut("6")
            }
            Divider()
            Button("Search Tasks") { focusSearch?() }.keyboardShortcut("f").disabled(focusSearch == nil)
            Button("Toggle Sidebar") { toggleSidebar?() }.keyboardShortcut("b").disabled(toggleSidebar == nil)
        }
        CommandMenu("Task") {
            Button("Previous Task") { move(-1) }.keyboardShortcut(.upArrow, modifiers: [.option, .command])
            Button("Next Task") { move(1) }.keyboardShortcut(.downArrow, modifiers: [.option, .command])
            Divider()
            Button("Review Changes") { thread?.review?() }
                .keyboardShortcut("d", modifiers: [.command, .shift])
                .disabled(thread?.review == nil)
            Button("Copy Branch Name") { thread?.copyBranch?() }
                .keyboardShortcut("c", modifiers: [.command, .shift])
                .disabled(thread?.copyBranch == nil)
            Divider()
            Button("Stop") { thread?.stop?() }
                .keyboardShortcut(".")
                .disabled(thread?.stop == nil)
        }
    }

    /// Up and down the sidebar's order, skipping what's snoozed.
    private func move(_ by: Int) {
        guard let stores else { return }
        let s = SidebarSections(runs: stores.fleet.runs, userID: stores.fleet.userID, teamScope: true, filtering: false, now: Date())
        let order = s.active + s.groups.flatMap(\.runs)
        guard !order.isEmpty else { return }
        let i = order.firstIndex { $0.id == stores.openThreadID } ?? (by > 0 ? -1 : order.count)
        stores.open(order[max(0, min(order.count - 1, i + by))].id)
    }
}
