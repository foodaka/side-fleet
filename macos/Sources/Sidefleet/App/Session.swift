import AppKit
import Observation
import SidefleetKit
import SwiftUI

/// Where the main window is.
enum Route: Hashable {
    case newTask
    case overview
    case machines
    case rooms
    case room(String)
    case team
    case chatApps
    /// An invite link, opened in the app.
    case join(String)
    case thread(String)
}

/// Everything for one signed-in control plane.
@MainActor
@Observable
final class Stores {
    let api: API
    let toasts = ToastCenter()
    let fleet: FleetStore
    let inbox: InboxStore
    let prefs = PrefsStore()
    let seen = SeenStore()
    let notifier = Notifier()
    /// The new-task box keeps what you've typed while you look at something else.
    let newTask: NewTaskModel
    /// So does each thread's reply box.
    var replyDrafts: [String: String] = [:]
    let controlURL: URL?
    let isDemo: Bool
    var route: Route = .newTask {
        didSet { lookedAtThread() }
    }
    /// Asks the repo picker or Add machine to open, from wherever.
    var repoPickerOpen = false
    var addMachineOpen = false
    /// The same for the sheets the org switcher and the Settings menu open.
    var machinesOpen = false
    var renameOrgOpen = false
    var newOrgOpen = false
    var feedbackOpen = false
    /// Signing out ends the session this belongs to, so the session does it.
    @ObservationIgnored var onSignOut: (@MainActor () -> Void)?
    /// A thread whose screen should open its fork sheet (asked from the sidebar).
    var forkThread: String?
    /// The app is in front, so what's on screen counts as seen.
    private(set) var appActive = true

    @ObservationIgnored private var planner = NotificationPlanner()
    @ObservationIgnored private var inboxConnected = false
    @ObservationIgnored private var prefsConnected = false

    init(backend: Backend, controlURL: URL?, isDemo: Bool = false) {
        api = API(backend: backend)
        fleet = FleetStore(api: api, toasts: toasts)
        inbox = InboxStore(api: api)
        newTask = NewTaskModel(fleet: fleet, prefs: prefs)
        rooms = RoomsStore(fleet: fleet)
        team = TeamStore(fleet: fleet)
        chatApps = ChatAppsStore(fleet: fleet)
        self.controlURL = controlURL
        self.isDemo = isDemo
    }

    var openThreadID: String? {
        if case .thread(let id) = route { return id }
        return nil
    }

    func start() {
        fleet.onRefreshed = { [weak self] in self?.fleetRefreshed() }
        inbox.onChange = { [weak self] items in self?.inboxChanged(items) }
        fleet.start()
    }

    func stop() {
        fleet.stop()
        inbox.disconnect()
        fleet.onRefreshed = nil
        inbox.onChange = nil
        NSApp.dockTile.badgeLabel = nil
    }

    private func fleetRefreshed() {
        seen.start(fleet.runs)
        if !inboxConnected {
            inboxConnected = true
            inbox.connect()
        }
        // Signed in with GitHub: your settings follow you everywhere.
        if fleet.me?.user != nil, !prefsConnected {
            prefsConnected = true
            Task { await prefs.connect(api: api) }
        }
        lookedAtThread()
        updateBadge()
        post(planner.runsChanged(fleet.runs, visible: visibleThreadID))
    }

    private func inboxChanged(_ items: [InboxItem]) {
        lookedAtThread()
        updateBadge()
        post(planner.inboxChanged(items, visible: visibleThreadID))
    }

    private func post(_ list: [NotificationPlanner.Post]) {
        guard prefs.notifications else { return }
        for p in list { notifier.post(p) }
    }

    /// A task counts as seen only while you can see it.
    private var visibleThreadID: String? { appActive ? openThreadID : nil }

    private func lookedAtThread() {
        guard let id = visibleThreadID else { return }
        if let r = fleet.runs.first(where: { $0.id == id }) { seen.mark(r) }
        if inbox.item(for: id) != nil { Task { await inbox.dismiss(id) } }
    }

    private func updateBadge() {
        let n = inbox.items.count
        NSApp.dockTile.badgeLabel = n > 0 ? String(n) : nil
    }

    func setAppActive(_ active: Bool) {
        appActive = active
        fleet.setActive(active)
        if active {
            lookedAtThread()
            Task { await prefs.pull() }
        }
    }

    /// Back from sleep: the streams reconnect by themselves, and the lists catch up now.
    func didWake() {
        Task { await fleet.refresh() }
        if inboxConnected { inbox.connect() }
    }

    func switchOrg(_ id: String, to next: Route = .newTask) {
        route = next
        rooms.reset()
        team.reset()
        chatApps.reset()
        fleet.switchOrg(id)
        inbox.connect()
    }

    func open(_ thread: String) { route = .thread(thread) }

    /// The list of rooms outlives the screen, so coming back to it shows what was there.
    let rooms: RoomsStore
    let team: TeamStore
    let chatApps: ChatAppsStore
}

/// Signed in or out, and to where.
@MainActor
@Observable
final class AppSession {
    private(set) var stores: Stores?
    private static let urlKey = "sidefleet_control_url"

    var savedURL: String { UserDefaults.standard.string(forKey: Self.urlKey) ?? "" }

    init(arguments: [String] = CommandLine.arguments) {
        if arguments.contains("--demo") {
            activate(Stores(backend: DemoBackend(), controlURL: URL(string: "https://sidefleet.example.com"), isDemo: true))
            return
        }
        if let url = URL(string: savedURL), url.scheme != nil, let credential = Keychain.load() {
            activate(Stores(backend: HTTPBackend(baseURL: url, credential: credential), controlURL: url))
        }
    }

    private func activate(_ s: Stores) {
        stores = s
        s.onSignOut = { [weak self] in Task { await self?.signOut() } }
        s.start()
    }

    /// Signed out because the control plane said so (a revoked key, an ended session).
    var needsSignIn: Bool { stores?.fleet.needsSignIn ?? true }

    /// The saved credential the control plane just turned away, so the sign-in screen can say why it's showing.
    var rejectedCredential: Credential? { stores?.fleet.needsSignIn == true ? Keychain.load() : nil }

    /// Checks the credential before keeping it. Throws what the control plane said.
    func signIn(url: URL, credential: Credential) async throws {
        let backend = HTTPBackend(baseURL: url, credential: credential)
        _ = try await API(backend: backend).me()
        UserDefaults.standard.set(url.absoluteString, forKey: Self.urlKey)
        Keychain.save(credential)
        stores?.stop()
        activate(Stores(backend: backend, controlURL: url))
    }

    func signOut() async {
        guard let s = stores else { return }
        if s.fleet.me?.user != nil { try? await s.api.signOut() }
        s.stop()
        // Whoever signs in next starts in their own first org, not this one.
        s.fleet.forgetOrg()
        stores = nil
        Keychain.clear()
    }
}
