import AppKit
import SidefleetKit
import SwiftUI

/// Who is in this org, and inviting more people. Teammates see each other's tasks; machines only
/// ever run their owner's.
struct TeamScreen: View {
    @Environment(Stores.self) private var stores
    @State private var removing: Member?
    @State private var revoking: Invite?
    @State private var joining = false

    private var fleet: FleetStore { stores.fleet }
    private var store: TeamStore { stores.team }
    private var me: User? { fleet.me?.user }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 28) {
                ScreenHeader(
                    title: fleet.org?.name ?? "Team",
                    detail: "Everyone here sees the team's tasks, rooms and changes. Machines only ever run their owner's tasks, so to pick up someone's work you continue it on your own machine."
                ) {
                    if me != nil {
                        Button("Join with a link…") { joining = true }.buttonStyle(SecondaryButtonStyle())
                    }
                }
                if me == nil {
                    Text("Teams need GitHub sign-in. With the developer key, everything runs in the single default org.")
                        .font(SF.font(13))
                        .foregroundStyle(SF.muted)
                        .padding(16)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
                } else {
                    if fleet.isOwner { invite }
                    members
                }
            }
            .column()
            .padding(.horizontal, 28)
            .padding(.top, 20)
            .padding(.bottom, 28)
        }
        .background(SF.background)
        // Keyed on the org alone: a background refresh must not wipe a freshly made invite link.
        .task(id: fleet.org?.id) {
            store.reset()
            await store.load()
        }
        // Ownership can settle after the org loads; that only loads the list, never clears the link.
        .onChange(of: fleet.isOwner) { _, owner in
            if owner { Task { await store.loadInvites() } }
        }
        .sheet(isPresented: $joining) { JoinLinkSheet() }
        .alert(removeTitle, isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })) {
            Button(removing?.id == me?.id ? "Leave" : "Remove", role: .destructive) {
                if let m = removing { remove(m) }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(removing?.id == me?.id
                ? "Your machines in it disconnect and your queued tasks there stop."
                : "Their machines in it disconnect and their queued tasks stop. Their threads stay.")
        }
        .alert("Revoke this invite link?", isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } })) {
            Button("Revoke", role: .destructive) {
                if let i = revoking { Task { await store.revoke(i) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Anyone who has it can no longer join with it.")
        }
    }

    private var removeTitle: String {
        guard let m = removing else { return "" }
        let org = fleet.org?.name ?? "the org"
        return m.id == me?.id ? "Leave \(org)?" : "Remove @\(m.login) from \(org)?"
    }

    private func remove(_ m: Member) {
        Task {
            // Leaving takes you back to your first org.
            if await store.remove(m) { stores.switchOrg("") }
        }
    }

    // MARK: Invites

    private var invite: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Label("Invite people", systemImage: "person.badge.plus").font(SF.font(15, .semibold))
                Text(store.personal
                    ? "This is your personal org. Invite people here, or create a team org from the org menu first."
                    : "Send a link. They sign in with GitHub and land in \(fleet.org?.name ?? "this org").")
                    .font(SF.font(13))
                    .foregroundStyle(SF.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            FlowRow(spacing: 8, lineSpacing: 8) {
                PickerPill(
                    options: [1, 7, 30].map { PickerOption(id: String($0), title: $0 == 1 ? "1 day" : "\($0) days") },
                    selected: String(store.expiresDays), width: 180, accessibility: "Expires",
                    onPick: { store.expiresDays = Int($0) ?? 7 }
                ) {
                    Text("Expires in \(store.expiresDays == 1 ? "1 day" : "\(store.expiresDays) days")").font(SF.font(13))
                }
                .overlay(Capsule().strokeBorder(SF.input))
                PickerPill(
                    options: [PickerOption(id: "one", title: "One person"), PickerOption(id: "many", title: "Anyone with the link")],
                    selected: store.onePerson ? "one" : "many", width: 220, accessibility: "Uses",
                    onPick: { store.onePerson = $0 == "one" }
                ) {
                    Text(store.onePerson ? "One person" : "Anyone with the link").font(SF.font(13))
                }
                .overlay(Capsule().strokeBorder(SF.input))
                Button {
                    Task { await store.createInvite() }
                } label: {
                    Label(store.creating ? "Creating…" : "Create invite link", systemImage: "link")
                }
                .buttonStyle(PrimaryButtonStyle())
                .disabled(store.creating)
            }
            if let code = store.fresh {
                let url = TeamStore.link(code: code, control: stores.controlURL)
                VStack(alignment: .leading, spacing: 8) {
                    Text("Send this link. It is shown once; make another any time.").font(SF.font(13))
                    HStack(spacing: 8) {
                        Text(url).font(SF.mono(12)).lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                        Spacer(minLength: 0)
                        CopyButton(text: { url }, label: "Copy")
                    }
                    .padding(.leading, 10)
                    .padding(.trailing, 4)
                    .padding(.vertical, 3)
                    .background(SF.background, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                }
                .padding(12)
                .background(SF.okSoft.opacity(0.5), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(SF.ok.opacity(0.4)))
                .transition(.opacity)
            }
            if !store.pending.isEmpty {
                VStack(spacing: 0) {
                    ForEach(Array(store.pending.enumerated()), id: \.element.id) { i, inv in
                        if i > 0 { Divider() }
                        HStack(spacing: 12) {
                            Text(describe(inv)).foregroundStyle(SF.muted).frame(maxWidth: .infinity, alignment: .leading)
                            Button("Revoke") { revoking = inv }
                                .buttonStyle(LinkButtonStyle(color: SF.errInk, hoverColor: SF.err))
                                .font(SF.font(12))
                        }
                        .font(SF.font(13))
                        .padding(.vertical, 8)
                    }
                }
            }
        }
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
        .animation(.easeOut(duration: 0.2), value: store.fresh)
    }

    private func describe(_ inv: Invite) -> String {
        var s = inv.maxUses == 1 ? "One-person link" : "Open link"
        if let by = inv.createdByLogin { s += " by @\(by)" }
        s += ", expires \(Format.until(inv.expiresAt, now: fleet.now))"
        if inv.uses > 0 { s += ", used \(inv.uses)×" }
        return s
    }

    // MARK: Members

    private var members: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text("Members").font(SF.font(15, .semibold))
                if !fleet.members.isEmpty {
                    Text("\(fleet.members.count)").font(SF.font(15)).monospacedDigit().foregroundStyle(SF.muted)
                }
            }
            if fleet.members.isEmpty {
                Text("Loading…").font(SF.font(13)).foregroundStyle(SF.muted)
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(fleet.members.enumerated()), id: \.element.id) { i, m in
                        if i > 0 { Divider() }
                        row(m)
                    }
                }
                .card()
            }
        }
    }

    private func row(_ m: Member) -> some View {
        let n = store.machines(of: m.id)
        let isMe = m.id == me?.id
        return HStack(spacing: 12) {
            InitialAvatar(login: m.login, size: 28)
            VStack(alignment: .leading, spacing: 1) {
                (Text("@\(m.login)").font(SF.font(13, .medium)) + Text(isMe ? "  (you)" : "").font(SF.font(13)).foregroundColor(SF.muted))
                    .lineLimit(1)
                Text((m.role == "owner" ? "Owner" : "Member") + (n > 0 ? " · \(n) \(n == 1 ? "machine" : "machines")" : ""))
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
            }
            Spacer(minLength: 0)
            if isMe {
                if store.canLeave(m) {
                    Button("Leave") { removing = m }
                        .buttonStyle(LinkButtonStyle(hoverColor: SF.errInk))
                        .font(SF.font(12))
                }
            } else if fleet.isOwner {
                Button("Remove") { removing = m }
                    .buttonStyle(LinkButtonStyle(hoverColor: SF.errInk))
                    .font(SF.font(12))
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }
}

/// Paste an invite link to open it here: links are web addresses, so they'd open in the browser otherwise.
private struct JoinLinkSheet: View {
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @FocusState private var focused: Bool

    private var code: String? { TeamStore.code(from: text) }

    private func open() {
        guard let code else { return }
        dismiss()
        stores.route = .join(code)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Join a team").font(SF.font(17, .semibold))
                Text("Paste the invite link someone sent you.").font(SF.font(13)).foregroundStyle(SF.muted)
            }
            TextField("https://…/#/join/…", text: $text)
                .font(SF.mono(12.5))
                .modifier(BoxedField())
                .focused($focused)
                .onSubmit(open)
                .accessibilityLabel("Invite link")
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(SecondaryButtonStyle()).keyboardShortcut(.cancelAction)
                Button("Continue", action: open).buttonStyle(PrimaryButtonStyle()).keyboardShortcut(.defaultAction).disabled(code == nil)
            }
        }
        .padding(24)
        .frame(width: 420)
        .background(SF.background)
        .onAppear {
            // An invite link already on the clipboard is almost certainly the one.
            if let clip = NSPasteboard.general.string(forType: .string), clip.contains("/join/"), TeamStore.code(from: clip) != nil {
                text = clip.trimmingCharacters(in: .whitespacesAndNewlines)
            }
            focused = true
        }
    }
}

/// An invite link: who invited you where, and joining.
struct JoinScreen: View {
    let code: String
    @Environment(Stores.self) private var stores
    @State private var model: JoinModel?

    private var fleet: FleetStore { stores.fleet }

    var body: some View {
        VStack(alignment: .leading, spacing: 24) {
            HStack(spacing: 8) {
                LogoMark(size: 18)
                Text("Sidefleet").font(SF.font(15, .semibold))
            }
            if let model {
                if model.failed {
                    Text("Couldn't reach Sidefleet to read this invite. Check your connection and try again.")
                        .font(SF.font(13))
                        .foregroundStyle(SF.muted)
                    Button("Try again") { Task { await model.load() } }.buttonStyle(SecondaryButtonStyle(height: 40))
                } else if let info = model.info {
                    if info.valid {
                        valid(info, model)
                    } else {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(info.problem).font(SF.font(24, .medium)).tracking(-0.3).fixedSize(horizontal: false, vertical: true)
                            Text("Ask \(info.inviterLogin.map { "@\($0)" } ?? "whoever sent it") for a new link\(info.orgName.isEmpty ? "" : " to \(info.orgName)").")
                                .font(SF.font(13))
                                .foregroundStyle(SF.muted)
                        }
                        Button("Go to Sidefleet") { stores.route = .newTask }
                            .buttonStyle(SecondaryButtonStyle(height: 40))
                    }
                } else {
                    SkeletonBar(width: 280, height: 30)
                    SkeletonBar(height: 40)
                }
            }
        }
        .frame(maxWidth: 364, alignment: .leading)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(SF.background)
        .task(id: code) {
            let m = JoinModel(code: code, fleet: fleet)
            model = m
            await m.load()
        }
    }

    @ViewBuilder private func valid(_ info: InviteInfo, _ model: JoinModel) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            if let inviter = info.inviterLogin { InitialAvatar(login: inviter, size: 44) }
            (info.inviterLogin.map { Text("@\($0) invited you to ") } ?? Text("Join "))
                .font(SF.font(24, .medium))
                .tracking(-0.3)
                + Text(info.orgName).font(SF.font(24, .semibold)).tracking(-0.3)
            Text("Work alongside the team: see what everyone's agents are doing, split goals into tasks in rooms, and run your part on your own machines. Nobody else's tasks ever run on your machines.")
                .font(SF.font(13))
                .lineSpacing(3)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        if let user = fleet.me?.user {
            if model.already {
                Text("You're already in \(info.orgName).").font(SF.font(13)).foregroundStyle(SF.muted)
            }
            Button {
                Task {
                    if let org = await model.join() { stores.switchOrg(org.id, to: .rooms) }
                }
            } label: {
                Text(model.joining ? "Joining…" : model.already ? "Open \(info.orgName)" : "Join \(info.orgName) as @\(user.login)")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(PrimaryButtonStyle(height: 40))
            .disabled(model.joining)
        } else {
            Text("You're signed in with a developer key. Teams need GitHub sign-in: sign out from Settings, sign in with GitHub, then open this link again.")
                .font(SF.font(13))
                .lineSpacing(3)
                .foregroundStyle(SF.muted)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .background(SF.fill, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
