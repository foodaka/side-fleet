import SidefleetKit
import SwiftUI

// The sheets the org switcher and the Settings menu open: your machines, a new org, renaming this one.

/// Every machine of yours, once. Each connected to one org and runs your tasks in all of them.
struct MyMachinesSheet: View {
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss

    private var fleet: FleetStore { stores.fleet }
    private var several: Bool { (fleet.me?.orgs.count ?? 0) > 1 }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Your machines").font(SF.font(15, .semibold))
            Text(several
                ? "Each of your machines runs your tasks in every org you belong to, so you connect it once. It counts against the plan of the org it connected to."
                : "Your machines run your tasks here. Join or create another org and they work there too, with nothing to reconnect.")
                .font(SF.font(13))
                .lineSpacing(2)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            if fleet.myMachines.isEmpty {
                HStack(spacing: 4) {
                    Text("No machines of yours yet.")
                    Button("Connect one") {
                        dismiss()
                        stores.route = .machines
                    }
                    .buttonStyle(.plain)
                    .underline()
                    .foregroundStyle(SF.ink)
                    Text("to start running tasks.")
                }
                .font(SF.font(13))
                .foregroundStyle(SF.muted)
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [3, 2])))
            } else {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(Array(fleet.myMachines.enumerated()), id: \.element.key) { i, m in
                            if i > 0 { Divider() }
                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 8) {
                                    StatusDot(status: m.online ? .online : .offline)
                                    Text(m.displayName).font(SF.font(13.5, .medium)).lineLimit(1)
                                }
                                Text(detail(m)).font(SF.font(12)).foregroundStyle(SF.muted).lineLimit(1)
                            }
                            .padding(.horizontal, 14)
                            .padding(.vertical, 12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
                .frame(maxHeight: 360)
                .fixedSize(horizontal: false, vertical: true)
                .card(cornerRadius: 12)
            }
            HStack {
                Spacer()
                Button("Done") { dismiss() }.buttonStyle(PrimaryButtonStyle()).keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 520)
    }

    private func detail(_ m: MyMachine) -> String {
        let seen = m.online ? "Online" : "Last seen \(Format.rel(m.lastHeartbeatAt, now: fleet.now))"
        return "\(seen) · \(m.repos.count) repo\(m.repos.count == 1 ? "" : "s")" + (several ? " · connected to \(m.orgName)" : "")
    }
}

/// Creates a team org and switches to it.
struct NewOrgSheet: View {
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var busy = false
    @FocusState private var focused: Bool

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("New org").font(SF.font(15, .semibold))
            Text("An org has its own machines, runs and worker tokens. You can add people who have signed in to Sidefleet once.")
                .font(SF.font(13))
                .lineSpacing(2)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            TextField("Acme", text: $name)
                .textFieldStyle(.roundedBorder)
                .focused($focused)
                .onSubmit(create)
                .accessibilityLabel("Org name")
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(SecondaryButtonStyle()).keyboardShortcut(.cancelAction)
                Button(busy ? "Creating…" : "Create org", action: create)
                    .buttonStyle(PrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(busy || trimmed.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 400)
        .onAppear { focused = true }
    }

    private func create() {
        guard !busy, !trimmed.isEmpty else { return }
        busy = true
        Task {
            do {
                // Names are 80 characters at most, as the page's field allows.
                let org = try await stores.api.createOrg(name: String(trimmed.prefix(80)))
                dismiss()
                stores.switchOrg(org.id)
                stores.toasts.show("Created \(org.name)")
            } catch {
                stores.toasts.show(error.localizedDescription, isError: true)
            }
            busy = false
        }
    }
}

/// Renames the org the app acts in. Owners only.
struct RenameOrgSheet: View {
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var busy = false
    @FocusState private var focused: Bool

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Rename org").font(SF.font(15, .semibold))
            Text("Everyone in it sees the new name. Machines, tasks and invite links keep working.")
                .font(SF.font(13))
                .lineSpacing(2)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            TextField("Org name", text: $name)
                .textFieldStyle(.roundedBorder)
                .focused($focused)
                .onSubmit(save)
                .accessibilityLabel("Org name")
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(SecondaryButtonStyle()).keyboardShortcut(.cancelAction)
                Button(busy ? "Saving…" : "Save", action: save)
                    .buttonStyle(PrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(busy || trimmed.isEmpty || trimmed == stores.fleet.org?.name)
            }
        }
        .padding(20)
        .frame(width: 400)
        .onAppear {
            name = stores.fleet.org?.name ?? ""
            focused = true
        }
    }

    private func save() {
        guard !busy, !trimmed.isEmpty, trimmed != stores.fleet.org?.name else { return }
        busy = true
        let next = String(trimmed.prefix(80))
        Task {
            do {
                try await stores.fleet.renameOrg(next)
                dismiss()
                stores.toasts.show("Renamed to \(next)")
            } catch {
                stores.toasts.error("Could not rename the org", error)
            }
            busy = false
        }
    }
}
