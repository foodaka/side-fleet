import Foundation
import Observation

/// Who is in the org, and inviting more people. A port of web/src/routes/team/+page.svelte's state.
@MainActor
@Observable
public final class TeamStore {
    /// Nil until loaded; only owners can read them.
    public private(set) var invites: [Invite]?
    /// The code of the invite just made: its link is shown once.
    public private(set) var fresh: String?
    public private(set) var creating = false
    public var expiresDays = 7
    public var onePerson = false

    @ObservationIgnored private let fleet: FleetStore

    public init(fleet: FleetStore) { self.fleet = fleet }

    /// Your personal org: the one named after you, where you are the only one.
    public var personal: Bool {
        guard let org = fleet.org, let me = fleet.me?.user else { return false }
        return org.slug == me.login.lowercased() && fleet.members.count <= 1
    }

    public var pending: [Invite] { (invites ?? []).filter { $0.isPending(now: fleet.now) } }
    public func machines(of user: String) -> Int { fleet.workers.filter { $0.owner?.id == user }.count }

    /// The last owner can't leave, and nobody leaves their personal org.
    public func canLeave(_ m: Member) -> Bool {
        !personal && !(m.role == "owner" && fleet.members.filter { $0.role == "owner" }.count == 1)
    }

    /// Another org: forget the link and the list.
    public func reset() {
        invites = nil
        fresh = nil
    }

    public func load() async {
        await fleet.reloadMembers()
        await loadInvites()
    }

    public func loadInvites() async {
        guard let org = fleet.org, fleet.isOwner else { return }
        invites = (try? await fleet.api.invites(org: org.id)) ?? []
    }

    public func createInvite() async {
        guard let org = fleet.org else { return }
        creating = true
        defer { creating = false }
        do {
            fresh = try await fleet.api.createInvite(org: org.id, days: expiresDays, onePerson: onePerson).code
            await loadInvites()
        } catch {
            fleet.toasts.error("Could not create the invite", error)
        }
    }

    public func revoke(_ invite: Invite) async {
        guard let org = fleet.org else { return }
        do {
            try await fleet.api.revokeInvite(org: org.id, id: invite.id)
            fresh = nil
            await loadInvites()
        } catch {
            fleet.toasts.error("Could not revoke", error)
        }
    }

    /// Remove a member, or leave. Returns true when it was you who left.
    public func remove(_ m: Member) async -> Bool {
        guard let org = fleet.org else { return false }
        let me = m.id == fleet.userID
        do {
            try await fleet.api.removeMember(org: org.id, user: m.id)
            if me {
                fleet.toasts.show("You left \(org.name)")
                return true
            }
            fleet.toasts.show("Removed @\(m.login)")
            await fleet.reloadMembers()
        } catch {
            fleet.toasts.error(me ? "Could not leave" : "Could not remove", error)
        }
        return false
    }

    /// The link to send: the page's join address, which the app opens too.
    public static func link(code: String, control: URL?) -> String {
        var base = control?.absoluteString ?? ""
        while base.hasSuffix("/") { base.removeLast() }
        return "\(base)/#/join/\(code)"
    }

    /// The code in an invite link, or in whatever was pasted: `…/#/join/<code>`, `sidefleet://join/<code>`, or the code.
    public static func code(from text: String) -> String? {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return nil }
        if let r = t.range(of: "/join/") ?? t.range(of: "join/") {
            let rest = t[r.upperBound...]
            let code = rest.prefix { !"/?#& ".contains($0) }
            return code.isEmpty ? nil : String(code)
        }
        return t.contains("/") || t.contains(" ") ? nil : t
    }
}

/// An invite link: who invited you where, and joining.
@MainActor
@Observable
public final class JoinModel {
    public let code: String
    public private(set) var info: InviteInfo?
    public private(set) var failed = false
    public private(set) var joining = false

    @ObservationIgnored private let fleet: FleetStore

    public init(code: String, fleet: FleetStore) {
        self.code = code
        self.fleet = fleet
    }

    /// You're in an org of this name already (other than your personal one).
    public var already: Bool {
        guard let info, let me = fleet.me else { return false }
        return me.orgs.contains { $0.name == info.orgName && $0.slug != me.user?.login.lowercased() }
    }

    public func load() async {
        info = nil
        failed = false
        do {
            info = try await fleet.api.invite(code: code)
        } catch {
            failed = true
        }
    }

    /// Returns the org joined.
    public func join() async -> Org? {
        joining = true
        defer { joining = false }
        do {
            let org = try await fleet.api.acceptInvite(code: code)
            fleet.toasts.show("Welcome to \(org.name)")
            return org
        } catch {
            fleet.toasts.error("Could not join", error)
            return nil
        }
    }
}

/// Chat apps: each connection is a personal API token. A port of web/src/routes/chat/+page.svelte's state.
@MainActor
@Observable
public final class ChatAppsStore {
    public enum How: Sendable { case ask, yourself }

    public private(set) var tokens: [ApiToken]?
    public private(set) var app: ChatAppID?
    public var scope = "tasks"
    public private(set) var busy = false
    /// The connection just made: its key is only known now.
    public private(set) var fresh: ApiToken?
    public var how: How = .ask
    /// Show the setup card even when there are connections already.
    public var adding = false

    @ObservationIgnored private let fleet: FleetStore

    public init(fleet: FleetStore) { self.fleet = fleet }

    public var live: [ApiToken] { (tokens ?? []).filter { $0.revokedAt == nil } }
    /// The list above the card leaves out the one being set up, so the card doesn't jump down as it appears.
    public var listed: [ApiToken] { live.filter { $0.id != fresh?.id } }
    public var chosen: ChatApp? { app.map(ChatApps.app) }
    /// The connection being set up, as it is now (its first call, its updates).
    public var current: ApiToken? { live.first { $0.id == fresh?.id } }
    public var connected: Bool { current?.lastUsedAt != nil }
    public var showSetup: Bool { adding || fresh != nil || (tokens != nil && live.isEmpty) }
    /// Waiting for the app's first call: worth asking again every few seconds.
    public var waiting: Bool { fresh != nil && !connected }

    public func load() async {
        tokens = (try? await fleet.api.apiTokens()) ?? []
    }

    public func pick(_ id: ChatAppID) {
        app = id
        how = ChatApps.app(id).canSelfSetup ? .ask : .yourself
    }

    public func reset() {
        fresh = nil
        app = nil
        scope = "tasks"
        adding = false
    }

    public func connect() async {
        guard let app else { return }
        busy = true
        defer { busy = false }
        do {
            fresh = try await fleet.api.createApiToken(name: ChatApps.tokenName(app), scope: scope)
            await load()
        } catch {
            fleet.toasts.error("Could not connect", error)
        }
    }

    public func disconnect(_ t: ApiToken) async {
        do {
            try await fleet.api.revokeApiToken(t.id)
            if fresh?.id == t.id { reset() }
            fleet.toasts.show("Disconnected \(t.name)")
            await load()
        } catch {
            fleet.toasts.error("Could not disconnect", error)
        }
    }
}

/// Updates in chat for one connected app. A port of web/src/lib/sf/ui/ChatUpdates.svelte's state.
@MainActor
@Observable
public final class ChatUpdatesModel {
    /// `form`: where it listens; `setup`: what to add on the app's side, then a test.
    public enum Stage: Sendable { case idle, form, setup }

    public let tokenID: String
    public let app: ChatAppID
    public var stage: Stage = .idle
    public var address = ""
    public var platform = "telegram"
    public var allTasks = false
    public private(set) var secret = ""
    public var how: ChatAppsStore.How = .ask
    public private(set) var busy = false
    public private(set) var test: (ok: Bool, text: String)?

    @ObservationIgnored private let fleet: FleetStore
    @ObservationIgnored private var opened = false

    public init(tokenID: String, app: ChatAppID, fleet: FleetStore) {
        self.tokenID = tokenID
        self.app = app
        self.fleet = fleet
    }

    public var name: String { ChatApps.spoken(app) }
    public var capitalName: String { name.prefix(1).uppercased() + name.dropFirst() }
    public var setup: ChatSetup { ChatApps.updatesSetup(app, secret: secret, platform: platform) }
    public var message: String { ChatApps.updatesMessage(app, secret: secret, platform: platform) }

    /// Opened for you once, right after the app connects; Cancel closes it for good.
    public func openOnce(on: ChatUpdates?) {
        guard on == nil, !opened else { return }
        opened = true
        stage = .form
    }

    public func edit(on: ChatUpdates?) {
        address = on.map { ChatApps.updatesAddress(app, url: $0.url) } ?? ""
        allTasks = on?.allTasks ?? false
        test = nil
        stage = .form
    }

    public func save() async {
        busy = true
        defer { busy = false }
        do {
            let r = try await fleet.api.setChatUpdates(token: tokenID, kind: ChatApps.updatesKind(app), url: ChatApps.updatesURL(app, address: address), allTasks: allTasks)
            secret = r.secret ?? ""
            how = app == .other ? .yourself : .ask
            stage = .setup
        } catch {
            fleet.toasts.error("Could not turn updates on", error)
        }
    }

    public func sendTest() async {
        busy = true
        test = nil
        defer { busy = false }
        do {
            try await fleet.api.testChatUpdates(token: tokenID)
            test = (true, app == .other ? "Sent: your app got it." : "Sent. \(capitalName) should message you in a moment.")
        } catch {
            test = (false, error.localizedDescription)
        }
    }

    /// Returns whether it's off now.
    public func turnOff() async -> Bool {
        do {
            try await fleet.api.clearChatUpdates(token: tokenID)
            stage = .idle
            return true
        } catch {
            fleet.toasts.error("Could not turn updates off", error)
            return false
        }
    }
}
