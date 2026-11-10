import SidefleetKit
import SwiftUI

/// First run: the steps from signing in to the first task, ticked off as the fleet reports them.
struct OnboardingSteps: View {
    @Environment(Stores.self) private var stores
    @State private var orgName = ""
    @State private var saving = false
    /// Bumped when "Keep" is pressed, so the step re-reads what's remembered.
    @State private var keptTick = 0

    private enum Progress { case done, current, next }
    private enum Kind { case signedIn, name, connect, approve, run }

    private struct Step: Identifiable {
        var kind: Kind
        var title: String
        var detail: String
        var state: Progress
        var id: String { title }
    }

    private var fleet: FleetStore { stores.fleet }
    private var login: String? { fleet.me?.user?.login }
    /// Without a static worker token a machine asks to join, and someone here lets it in.
    private var approval: Bool { fleet.setup?.workerToken.isEmpty ?? true }
    private var asked: Bool { !fleet.pending.isEmpty }

    // Your first org is named after your GitHub login; offer to name it, once per org.
    private static let keptKey = "sidefleet_org_named_"
    private func kept(_ org: Org) -> Bool {
        _ = keptTick
        return UserDefaults.standard.bool(forKey: Self.keptKey + org.id)
    }

    private var offerName: Bool { login != nil && fleet.org != nil && fleet.isOwner }
    private var naming: Bool {
        guard let login, let org = fleet.org else { return false }
        return fleet.isOwner && org.name == login && !kept(org)
    }

    private var steps: [Step] {
        var out = [Step(
            kind: .signedIn, title: login != nil ? "Signed in with GitHub" : "Signed in",
            detail: login.map { "As @\($0). Sidefleet only reads your name and email, not your repos." } ?? "This app is your fleet.", state: .done
        )]
        if offerName, let org = fleet.org {
            out.append(Step(
                kind: .name, title: "Name your workspace",
                detail: naming
                    ? "It's called \(org.name) after your GitHub login. Teammates you invite see this name. You can change it later in Settings."
                    : "\(org.name). You can change it in Settings.",
                state: naming ? .current : .done
            ))
        }
        out.append(Step(
            kind: .connect, title: "Connect a machine",
            detail: "A laptop, a Mac mini or a VPS with Claude Code, Codex or opencode set up. It connects out to the control plane; nothing connects in.",
            state: asked ? .done : .current
        ))
        if approval {
            out.append(Step(
                kind: .approve, title: "Approve it here",
                detail: "The connect command prints a link and a code. Open the link, or enter the code below, so only machines you approve join.",
                state: asked ? .current : .next
            ))
        }
        out.append(Step(
            kind: .run, title: "Run your first task",
            detail: "Describe it in plain words. It runs in its own worktree on that machine and streams back here.", state: .next
        ))
        return out
    }

    var body: some View {
        let list = steps
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(list.enumerated()), id: \.element.id) { i, s in
                row(s, number: i + 1, last: i == list.count - 1)
            }
        }
    }

    private func row(_ s: Step, number: Int, last: Bool) -> some View {
        HStack(alignment: .top, spacing: 16) {
            badge(s.state, number)
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(s.title).font(SF.font(15, .medium)).foregroundStyle(s.state == .next ? SF.muted : SF.ink)
                    Text(s.detail).font(SF.font(13)).lineSpacing(3).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
                }
                extra(s)
            }
            .padding(.top, 3)
            .padding(.bottom, last ? 0 : 28)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        // The line down to the next step, green once this one is done.
        .overlay(alignment: .topLeading) {
            if !last {
                Rectangle()
                    .fill(s.state == .done ? SF.ok.opacity(0.5) : SF.border)
                    .frame(width: 1)
                    .padding(.top, 32)
                    .padding(.bottom, 4)
                    .padding(.leading, 13)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Step \(number)\(s.state == .done ? ", done" : s.state == .current ? ", now" : ""): \(s.title)")
    }

    private func badge(_ state: Progress, _ number: Int) -> some View {
        Group {
            if state == .done {
                Image(systemName: "checkmark").font(.system(size: 11, weight: .bold))
            } else {
                Text("\(number)").font(SF.font(12, .medium)).monospacedDigit()
            }
        }
        .foregroundStyle(state == .done ? Color.white : state == .current ? SF.inkInverse : SF.muted)
        .frame(width: 27, height: 27)
        .background(state == .done ? SF.ok : state == .current ? SF.ink : SF.card, in: Circle())
        .overlay(Circle().strokeBorder(state == .done ? SF.ok : state == .current ? SF.ink : SF.border))
        .accessibilityHidden(true)
    }

    @ViewBuilder private func extra(_ s: Step) -> some View {
        switch s.kind {
        case .name where naming:
            HStack(spacing: 8) {
                TextField("Acme", text: $orgName)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 224)
                    .onSubmit(saveName)
                    .accessibilityLabel("Workspace name")
                Button(saving ? "Saving…" : "Save", action: saveName)
                    .buttonStyle(PrimaryButtonStyle())
                    .disabled(saving || trimmed.isEmpty || trimmed == fleet.org?.name)
                Button("Keep \(fleet.org?.name ?? "")") {
                    if let org = fleet.org { UserDefaults.standard.set(true, forKey: Self.keptKey + org.id) }
                    keptTick += 1
                }
                .buttonStyle(.plain)
                .font(SF.font(13))
                .foregroundStyle(SF.muted)
            }
        case .connect where !asked:
            AddMachineSteps().padding(20).card(cornerRadius: 18)
        case .approve:
            PendingMachines(bare: true)
        case .run where !asked:
            HStack(spacing: 8) {
                StatusDot(status: .waiting)
                Text("Waiting for a machine to check in. This page updates by itself.").font(SF.font(13)).foregroundStyle(SF.muted)
            }
        default:
            EmptyView()
        }
    }

    private var trimmed: String { orgName.trimmingCharacters(in: .whitespacesAndNewlines) }

    private func saveName() {
        guard !saving, !trimmed.isEmpty, trimmed != fleet.org?.name else { return }
        saving = true
        let next = String(trimmed.prefix(80))
        Task {
            do {
                try await fleet.renameOrg(next)
                stores.toasts.show("Your workspace is \(next)")
            } catch {
                stores.toasts.error("Could not rename the workspace", error)
            }
            saving = false
        }
    }
}
