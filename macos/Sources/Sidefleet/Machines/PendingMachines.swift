import SidefleetKit
import SwiftUI

/// Machines that ran `sidefleet connect` without a token and wait to be let in. Self-hosted, they
/// are listed here directly. Hosted, a request belongs to no org until someone enters the code its
/// terminal shows: it is then shown here to approve or deny, never approved unseen.
struct PendingMachines: View {
    /// Just the code box and the requests, under a heading someone else gives (onboarding).
    var bare = false
    @Environment(Stores.self) private var stores
    /// Requests found by code (not in the org's list until approved).
    @State private var found: [Enrollment] = []
    @State private var code = ""
    @State private var looking = false
    @State private var busy: String?
    /// Which org each found request joins, when it isn't this one.
    @State private var into: [String: String] = [:]

    private var fleet: FleetStore { stores.fleet }
    private var hosted: Bool { fleet.setup?.hosted ?? false }
    private var orgs: [Org] { fleet.me?.orgs ?? [] }
    private var items: [Enrollment] { found + fleet.pending.filter { p in !found.contains { $0.id == p.id } } }
    private var origin: String { stores.controlURL?.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/")) ?? "" }

    private func isFound(_ e: Enrollment) -> Bool { found.contains { $0.id == e.id } }
    /// A request found by code joins no org yet: you pick which of yours it joins (this one unless you change it).
    private func target(_ e: Enrollment) -> String { into[e.id] ?? fleet.org?.id ?? "" }
    private func orgName(_ id: String) -> String { orgs.first { $0.id == id }?.name ?? "this org" }

    var body: some View {
        if !items.isEmpty || hosted {
            VStack(alignment: .leading, spacing: 12) {
                if !bare || hosted {
                    HStack(alignment: .bottom, spacing: 12) {
                        if !bare {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("Pending machines").font(SF.font(15, .semibold)).tracking(-0.15)
                                Text(hosted
                                    ? "Run `sidefleet connect --control \(origin)` on a machine, then enter the code it prints."
                                    : "Machines that asked to join with `sidefleet connect` and no token.")
                                    .font(SF.font(13))
                                    .foregroundStyle(SF.muted)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        Spacer(minLength: 0)
                        if hosted { codeBox }
                    }
                }
                ForEach(items) { row($0) }
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Pending machines")
        }
    }

    private var codeBox: some View {
        HStack(spacing: 8) {
            TextField("ABCD-EFGH", text: $code)
                .textFieldStyle(.roundedBorder)
                .font(SF.mono(13))
                .frame(width: 128)
                .onSubmit(lookup)
                .accessibilityLabel("Code from the machine")
            Button("Find", action: lookup)
                .buttonStyle(SecondaryButtonStyle())
                .disabled(looking || code.trimmingCharacters(in: .whitespaces).isEmpty)
        }
    }

    private func row(_ e: Enrollment) -> some View {
        HStack(spacing: 24) {
            VStack(alignment: .leading, spacing: 2) {
                Text(e.machineName).font(SF.font(14, .semibold)).lineLimit(1)
                HStack(spacing: 8) {
                    Text((e.os.isEmpty ? "unknown OS" : e.os) + (!e.hostname.isEmpty && e.hostname != e.machineName ? ", \(e.hostname)" : ""))
                    if let claude = e.claude {
                        HStack(spacing: 4) {
                            if claude.signedIn == true {
                                Image(systemName: "checkmark.circle").font(.system(size: 11)).foregroundStyle(SF.ok)
                            } else {
                                Image(systemName: "exclamationmark.triangle").font(.system(size: 11))
                            }
                            Text("Claude Code \(claude.version)")
                        }
                        .foregroundStyle(claude.signedIn == true ? SF.muted : SF.errInk)
                        .help(claude.signedIn == true ? "Signed in; tasks run with this account" : "Not signed in: run `claude` on this machine once")
                    }
                    Text("\(e.repoCount) repos").monospacedDigit()
                    Text("expires in \(Format.minutesLeft(e.expiresAt, now: fleet.now)) min").monospacedDigit()
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
                .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(alignment: .leading, spacing: 0) {
                Text("Code on that machine").font(SF.font(11)).foregroundStyle(SF.muted)
                Text(e.code).font(SF.mono(15)).bold().tracking(1).textSelection(.enabled)
            }
            .fixedSize()
            HStack(spacing: 8) {
                if orgs.count > 1, isFound(e) {
                    Menu {
                        ForEach(orgs) { o in
                            Button(o.name) { into[e.id] = o.id }
                        }
                    } label: {
                        Text("Into \(orgName(target(e)))")
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                    .accessibilityLabel("Org it joins")
                }
                Button("Deny") { decide(e, approve: false) }.buttonStyle(SecondaryButtonStyle()).disabled(busy == e.id)
                Button("Approve") { decide(e, approve: true) }.buttonStyle(PrimaryButtonStyle()).disabled(busy == e.id)
            }
        }
        .padding(16)
        .background(SF.queueSoft.opacity(0.4), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.queue.opacity(0.4)))
    }

    private func lookup() {
        let c = code.trimmingCharacters(in: .whitespaces)
        guard !c.isEmpty, !looking else { return }
        looking = true
        Task {
            do {
                let e = try await fleet.api.lookupEnrollment(code: c)
                found = [e] + found.filter { $0.id != e.id }
                code = ""
            } catch {
                stores.toasts.show(error.localizedDescription, isError: true)
            }
            looking = false
        }
    }

    private func decide(_ e: Enrollment, approve: Bool) {
        busy = e.id
        let org = target(e)
        let elsewhere = !org.isEmpty && org != fleet.org?.id
        Task {
            do {
                // The code goes with either answer: a request found by code is in no org's list.
                try await fleet.api.decideEnrollment(e.id, code: e.code, approve: approve, inOrg: elsewhere ? org : nil)
                found.removeAll { $0.id == e.id }
                into[e.id] = nil
                if approve, elsewhere {
                    stores.toasts.show("\(e.machineName) approved into \(orgName(org))", detail: "Switch to \(orgName(org)) to see it and give it tasks.")
                } else if approve {
                    stores.toasts.show("\(e.machineName) approved", detail: "It shows up here as soon as its first heartbeat lands.")
                } else {
                    stores.toasts.show("\(e.machineName) denied")
                }
                await fleet.refresh()
            } catch {
                stores.toasts.error(approve ? "Could not approve" : "Could not deny", error)
            }
            busy = nil
        }
    }
}
