import SidefleetKit
import SwiftUI

// The sidebar's two menus: which org the app acts in, and Settings. They are popovers, not native
// menus, so the org rows can carry a second line and the Settings row can look like its neighbours.

/// A row in one of these menus.
private struct MenuItem: View {
    var title: String
    var checked = false
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
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

/// Which org the app acts in, always shown to signed-in users so they know where their tasks go.
struct OrgSwitcher: View {
    @Environment(Stores.self) private var stores
    @State private var open = false

    private var fleet: FleetStore { stores.fleet }

    var body: some View {
        if let me = fleet.me, me.user != nil, let org = fleet.org {
            Button {
                open.toggle()
            } label: {
                HStack(spacing: 8) {
                    Text(org.name.prefix(1).uppercased())
                        .font(SF.font(11, .semibold))
                        .frame(width: 20, height: 20)
                        .background(SF.fill, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    Text(org.name).font(SF.font(13, .medium)).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
                    Image(systemName: "chevron.up.chevron.down").font(.system(size: 9, weight: .semibold)).foregroundStyle(SF.muted)
                }
                .padding(.horizontal, 8)
                .frame(height: 32)
                .hoverBackground(active: open, cornerRadius: 8)
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(SF.border))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Switch org")
            .popover(isPresented: $open, arrowEdge: .bottom) {
                VStack(alignment: .leading, spacing: 0) {
                    OptionList(
                        options: me.orgs.map { PickerOption(id: $0.id, title: $0.name, hint: $0.plan + ($0.role == "owner" ? "" : " · member")) },
                        selected: org.id, heading: "Orgs", width: 240
                    ) { id in
                        open = false
                        if id != org.id { stores.switchOrg(id) }
                    }
                    Divider()
                    VStack(spacing: 1) {
                        MenuItem(title: "Your machines…") { pick { stores.machinesOpen = true } }
                        if fleet.isOwner { MenuItem(title: "Rename \(org.name)…") { pick { stores.renameOrgOpen = true } } }
                        MenuItem(title: "New org…") { pick { stores.newOrgOpen = true } }
                    }
                    .padding(5)
                }
                .frame(width: 240)
            }
        }
    }

    /// The popover goes first: a sheet can't come up while it is still in the way.
    private func pick(_ then: @escaping () -> Void) {
        open = false
        DispatchQueue.main.async(execute: then)
    }
}

/// The Settings row at the foot of the sidebar: the theme, the Settings window, and what you'd
/// otherwise go looking for there (your repos, your machines, your orgs, signing out, feedback).
struct SettingsMenuRow: View {
    @Environment(Stores.self) private var stores
    @Environment(\.openSettings) private var openSettings
    @State private var open = false

    private var fleet: FleetStore { stores.fleet }

    var body: some View {
        Button {
            open.toggle()
        } label: {
            HStack(spacing: 10) {
                Image(systemName: "gearshape").font(.system(size: 12)).frame(width: 16)
                Text("Settings").font(SF.font(13))
                Spacer()
            }
            .foregroundStyle(open ? SF.ink : SF.muted)
            .padding(.horizontal, 10)
            .frame(height: 32)
            .hoverBackground(active: open)
        }
        .buttonStyle(.plain)
        .popover(isPresented: $open, arrowEdge: .top) {
            VStack(alignment: .leading, spacing: 1) { items }
                .padding(5)
                .frame(width: 250)
        }
    }

    @ViewBuilder private var items: some View {
        MenuNote("Appearance")
        MenuItem(title: "Match system", checked: stores.prefs.appearance == .system) { stores.prefs.set(\.appearance, .system) }
        MenuItem(title: "Light", checked: stores.prefs.appearance == .light) { stores.prefs.set(\.appearance, .light) }
        MenuItem(title: "Dark", checked: stores.prefs.appearance == .dark) { stores.prefs.set(\.appearance, .dark) }
        Divider().padding(.vertical, 4)
        MenuItem(title: "All settings…") { pick { openSettings() } }
        MenuItem(title: "Choose repos…") { pick { stores.repoPickerOpen = true } }
        Divider().padding(.vertical, 4)
        if let user = fleet.me?.user {
            MenuNote("Signed in as \(user.login)" + (fleet.org.map { ", working in \($0.name)" } ?? ""))
            MenuItem(title: "Your machines…") { pick { stores.machinesOpen = true } }
            if fleet.isOwner, let org = fleet.org {
                MenuItem(title: "Rename \(org.name)…") { pick { stores.renameOrgOpen = true } }
            }
            MenuItem(title: "New org…") { pick { stores.newOrgOpen = true } }
            MenuItem(title: "Sign out") { pick { stores.onSignOut?() } }
        } else if !stores.isDemo {
            MenuItem(title: "Change developer key…") { pick { stores.onSignOut?() } }
        }
        MenuItem(title: "Send feedback…") { pick { stores.feedbackOpen = true } }
        if let build = build {
            Divider().padding(.vertical, 4)
            MenuNote("Control plane \(build.version), build \(build.sha) from \(build.date)")
        }
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
