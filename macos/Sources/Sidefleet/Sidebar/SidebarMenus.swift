import SidefleetKit
import SwiftUI

// The sidebar's account menu. It is a popover, not a native menu, so the org rows can carry a
// second line and the row at the foot can show the org and who you are.

/// A row in one of these menus.
private struct MenuItem: View {
    var title: String
    var symbol: String?
    var checked = false
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if let symbol { Image(systemName: symbol).font(.system(size: 11)).frame(width: 14).foregroundStyle(SF.muted) }
                Text(title).font(SF.font(13)).lineLimit(1)
                Spacer(minLength: 4)
                if checked { Image(systemName: "checkmark").font(.system(size: 11, weight: .semibold)) }
            }
            .foregroundStyle(SF.ink)
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .hoverBackground()
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(checked ? .isSelected : [])
    }
}

private struct MenuNote: View {
    var text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The sidebar's foot, as on the web: which org you're in and who you are, and in its menu the org
/// switch and everything you set up rather than visit often (team, chat apps, settings, repos, machines,
/// appearance, feedback, signing out).
struct AccountMenu: View {
    @Environment(Stores.self) private var stores
    @Environment(\.openSettings) private var openSettings
    @State private var open = false

    private var fleet: FleetStore { stores.fleet }
    private var title: String { fleet.org?.name ?? "Sidefleet" }
    /// The pages this menu leads to: the row stays lit while you're on one.
    private var here: Bool {
        switch stores.route {
        case .team, .chatApps: true
        default: false
        }
    }

    var body: some View {
        Button {
            open.toggle()
        } label: {
            HStack(spacing: 10) {
                Text(title.prefix(1).uppercased())
                    .font(SF.font(12, .semibold))
                    .frame(width: 28, height: 28)
                    .background(SF.ink.opacity(0.1), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                VStack(alignment: .leading, spacing: 0) {
                    Text(title).font(SF.font(13, .medium)).foregroundStyle(SF.ink).lineLimit(1)
                    Text(fleet.me?.user?.login ?? "Settings").font(SF.font(11.5)).foregroundStyle(SF.muted).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 9, weight: .semibold)).foregroundStyle(SF.muted)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .hoverBackground(active: open || here)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Org, team and settings")
        .popover(isPresented: $open, arrowEdge: .top) {
            VStack(alignment: .leading, spacing: 0) { items }
                .frame(width: 256)
        }
    }

    @ViewBuilder private var items: some View {
        let user = fleet.me?.user
        if let me = fleet.me, user != nil, let org = fleet.org {
            OptionList(
                options: me.orgs.map { PickerOption(id: $0.id, title: $0.name, hint: $0.plan + ($0.role == "owner" ? "" : " · member")) },
                selected: org.id, heading: "Orgs", width: 256
            ) { id in
                open = false
                if id != org.id { stores.switchOrg(id) }
            }
            VStack(spacing: 1) {
                if fleet.isOwner { MenuItem(title: "Rename \(org.name)…") { pick { stores.renameOrgOpen = true } } }
                MenuItem(title: "New org…") { pick { stores.newOrgOpen = true } }
            }
            .padding(.horizontal, 5)
            .padding(.bottom, 5)
            Divider()
        }
        VStack(alignment: .leading, spacing: 1) {
            if user != nil {
                MenuItem(title: "Team", symbol: "person.2") { pick { stores.route = .team } }
            }
            MenuItem(title: "Chat apps", symbol: "bubble.left.and.bubble.right") { pick { stores.route = .chatApps } }
            MenuItem(title: "Settings", symbol: "gearshape") { pick { openSettings() } }
            if user != nil { MenuItem(title: "Your machines…") { pick { stores.machinesOpen = true } } }
            MenuItem(title: "Choose repos…") { pick { stores.repoPickerOpen = true } }
            Divider().padding(.vertical, 4)
            MenuNote("Appearance")
            MenuItem(title: "Match system", checked: stores.prefs.appearance == .system) { stores.prefs.set(\.appearance, .system) }
            MenuItem(title: "Light", checked: stores.prefs.appearance == .light) { stores.prefs.set(\.appearance, .light) }
            MenuItem(title: "Dark", checked: stores.prefs.appearance == .dark) { stores.prefs.set(\.appearance, .dark) }
            Divider().padding(.vertical, 4)
            MenuItem(title: "Send feedback…", symbol: "text.bubble") { pick { stores.feedbackOpen = true } }
            if user != nil {
                MenuItem(title: "Sign out") { pick { stores.onSignOut?() } }
            } else if !stores.isDemo {
                MenuItem(title: "Change developer key…") { pick { stores.onSignOut?() } }
            }
            if let build {
                Divider().padding(.vertical, 4)
                MenuNote("Control plane \(build.version), build \(build.sha) from \(build.date)")
            }
        }
        .padding(5)
    }

    private var build: (version: String, date: String, sha: String)? {
        let b = Format.parseBuild(fleet.setup?.build)
        return b.sha.isEmpty ? nil : b
    }

    /// The popover goes first: a sheet can't come up while it is still in the way.
    private func pick(_ then: @escaping () -> Void) {
        open = false
        DispatchQueue.main.async(execute: then)
    }
}
