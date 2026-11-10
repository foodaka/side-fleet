import SidefleetKit
import SwiftUI

/// Per-org worker tokens: one per machine, so a lost laptop is one revoke away. The value is shown
/// once, right after it is created. Owners only.
struct WorkerTokensSection: View {
    @Environment(Stores.self) private var stores
    /// Nil until the first load comes back.
    @State private var tokens: [WorkerToken]?
    @State private var name = ""
    @State private var busy = false
    @State private var fresh: WorkerToken?
    @State private var revoking: WorkerToken?

    private var fleet: FleetStore { stores.fleet }
    private var org: Org? { fleet.org }
    private var origin: String { stores.controlURL?.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/")) ?? "" }
    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// Live ones first, then the revoked.
    private var ordered: [WorkerToken] {
        let all = tokens ?? []
        return all.filter { $0.revokedAt == nil } + all.filter { $0.revokedAt != nil }
    }

    var body: some View {
        if let org, fleet.isOwner {
            VStack(alignment: .leading, spacing: 12) {
                HStack(alignment: .bottom, spacing: 12) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Worker tokens").font(SF.font(15, .semibold)).tracking(-0.15)
                        Text("For connecting machines from a script. Approving machines above is easier; it makes one of these per machine.")
                            .font(SF.font(13))
                            .foregroundStyle(SF.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 0)
                    TextField("Token name, e.g. CI runner", text: $name)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 224)
                        .onSubmit { create(org) }
                        .accessibilityLabel("Token name")
                    Button("Create token") { create(org) }
                        .buttonStyle(SecondaryButtonStyle())
                        .disabled(busy || trimmed.isEmpty)
                }
                if let fresh, let value = fresh.token {
                    VStack(alignment: .leading, spacing: 8) {
                        (Text(fresh.name).font(SF.font(13, .medium)) + Text(" created. Copy it now: it is not shown again.")).font(SF.font(13))
                        CommandBlock(shown: "sidefleet connect --control \(origin) --token \(value)", copy: "sidefleet connect --control \(origin) --token \(value)")
                        Button("Done, hide it") { self.fresh = nil }
                            .buttonStyle(.plain)
                            .font(SF.font(12))
                            .foregroundStyle(SF.muted)
                    }
                    .padding(16)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(SF.okSoft.opacity(0.4), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.ok.opacity(0.4)))
                }
                if tokens == nil {
                    Text("Loading…").font(SF.font(13)).foregroundStyle(SF.muted)
                } else if ordered.isEmpty {
                    Text("No tokens yet.")
                        .font(SF.font(13))
                        .foregroundStyle(SF.muted)
                        .padding(16)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [3, 2])))
                } else {
                    VStack(spacing: 0) {
                        ForEach(Array(ordered.enumerated()), id: \.element.id) { i, t in
                            if i > 0 { Divider() }
                            row(t)
                        }
                    }
                    .card()
                }
            }
            // Reload when the org changes or a machine joins (approving one mints a token).
            .task(id: "\(org.id) \(fleet.workers.count)") { await load(org.id) }
            .alert("Revoke \"\(revoking?.name ?? "")\"?", isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } })) {
                Button("Revoke", role: .destructive) {
                    if let t = revoking { revoke(t, org) }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Machines using it are disconnected on their next heartbeat.")
            }
        }
    }

    private func row(_ t: WorkerToken) -> some View {
        HStack(spacing: 16) {
            Text(t.name).font(SF.font(13, .medium)).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
            Text("\(t.prefix)…").font(SF.mono(12)).foregroundStyle(SF.muted)
            Group {
                Text("created \(Format.rel(t.createdAt, now: fleet.now))")
                Text(t.lastUsedAt.map { "used \(Format.rel($0, now: fleet.now))" } ?? "never used")
            }
            .font(SF.font(12))
            .foregroundStyle(SF.muted)
            if let revoked = t.revokedAt {
                Text("revoked \(Format.rel(revoked, now: fleet.now))").font(SF.font(12)).foregroundStyle(SF.errInk)
            } else {
                Button("Revoke") { revoking = t }
                    .buttonStyle(.plain)
                    .font(SF.font(12))
                    .foregroundStyle(SF.errInk)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .opacity(t.revokedAt == nil ? 1 : 0.6)
    }

    private func load(_ org: String) async {
        // A control plane that won't list them (a developer key, an older build) has none to show.
        tokens = (try? await stores.api.workerTokens(org: org)) ?? []
    }

    private func create(_ org: Org) {
        guard !busy, !trimmed.isEmpty else { return }
        busy = true
        Task {
            do {
                fresh = try await stores.api.createWorkerToken(org: org.id, name: String(trimmed.prefix(80)))
                name = ""
                await load(org.id)
            } catch {
                stores.toasts.error("Could not create the token", error)
            }
            busy = false
        }
    }

    private func revoke(_ t: WorkerToken, _ org: Org) {
        Task {
            do {
                try await stores.api.revokeWorkerToken(org: org.id, id: t.id)
                if fresh?.id == t.id { fresh = nil }
                stores.toasts.show("Revoked \(t.name)")
                await load(org.id)
            } catch {
                stores.toasts.error("Could not revoke", error)
            }
        }
    }
}
