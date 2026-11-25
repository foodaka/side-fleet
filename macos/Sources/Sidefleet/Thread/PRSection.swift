import AppKit
import SidefleetKit
import SwiftUI

/// The thread's pull request, at the end of the thread: where the branch goes, what GitHub says
/// about it (checks, review, conflicts) and the one thing to do next, with the rest in a menu.
struct PRSection: View {
    var pr: PullRequest?
    var branch: String
    var repo: String
    var machine: String
    var machineOnline: Bool
    /// A teammate's thread: you can look, not push.
    var theirs: Bool
    /// A message is running, so the branch can't be pushed yet.
    var busy: Bool
    /// The push (and opening the pull request) is running now.
    var pushing: Bool
    /// Merging it is running now.
    var merging = false
    /// Messages since the last push changed files, so pushing comes before merging.
    var unpushed = false
    /// The machine's Sidefleet can merge it from here.
    var canMerge = false
    var changes: DiffSet?
    /// Opens the sheet to choose the files (and, for a new one, its title).
    var onPush: () -> Void
    var onCheck: () -> Void
    var onReview: () -> Void
    /// Put a request to the agent in the reply box.
    var onAsk: (String) -> Void
    var onMerge: (MergeMethod) -> Void = { _ in }
    @Environment(Stores.self) private var stores
    @AppStorage("sidefleet_merge_method") private var methodRaw = MergeMethod.squash.rawValue
    /// Merging asks twice: the first click turns the button into "Confirm …" for a few seconds.
    @State private var confirming = false

    private enum Action { case view, push, open, merge }

    private var method: MergeMethod { MergeMethod(rawValue: methodRaw) ?? .squash }
    private var checks: PRChecks? { pr?.isOpen == true ? pr?.checks : nil }
    /// Ready to merge as far as GitHub has told us: branch protection still has the last word.
    private var ready: Bool {
        guard let pr, pr.isOpen, !pr.draft, !pr.conflicts, pr.review != "changes_requested" else { return false }
        return (checks?.failed ?? 0) == 0 && (checks?.pending ?? 0) == 0
    }
    private var action: Action {
        if theirs || pr?.state == "merged" { return .view }
        if pr?.isOpen == true { return ready && !unpushed && canMerge ? .merge : .push }
        return .open
    }
    private var running: Bool { pushing || merging }
    private var base: String { pr?.base ?? "the default branch" }

    /// Why the action can't run now, said under it rather than hidden in a tooltip.
    private var blocked: String? {
        if action == .view || running { return nil }
        if !machineOnline { return "\(machine) is offline, so it can't push or merge. It will once it's back." }
        if busy { return "Waiting for the agent to finish." }
        return nil
    }

    /// Why the merge items in the menu can't be used, if they can't.
    private var mergeOff: String? {
        if !canMerge { return "Update Sidefleet on \(machine) to merge from here" }
        if let blocked { return blocked }
        return unpushed ? "Push the new changes first" : nil
    }

    private var label: String {
        if merging { return "Merging…" }
        if pushing { return pr?.isOpen == true ? "Pushing…" : "Opening…" }
        switch action {
        case .merge: return confirming ? "Confirm \(method.verb)" : method.title
        case .push: return "Push changes"
        case .open: return pr?.state == "closed" ? "Open a new one" : "Open pull request"
        case .view: return "View on GitHub"
        }
    }

    private func askToMerge(_ m: MergeMethod) {
        methodRaw = m.rawValue
        confirming = true
        Task {
            try? await Task.sleep(for: .seconds(5))
            confirming = false
        }
    }

    private func primary() {
        guard action == .merge else { return onPush() }
        if !confirming { return askToMerge(method) }
        confirming = false
        onMerge(method)
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .center, spacing: 12) {
                HStack(alignment: .top, spacing: 10) {
                    Group {
                        if let pr { PRIcon(state: pr.state, draft: pr.draft, size: 12) } else {
                            Image(systemName: "arrow.triangle.branch").font(.system(size: 11)).foregroundStyle(SF.muted)
                        }
                    }
                    .frame(width: 26, height: 26)
                    .background(SF.fill, in: Circle())
                    titles
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                actionButton
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 11)

            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                Divider()
                row
            }
            if confirming, action == .merge {
                Divider()
                Text("Click again to \(method.verb) #\(pr?.number ?? 0) into \(base).")
                    .font(SF.font(11.5))
                    .foregroundStyle(SF.okInk)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
                    .background(SF.okSoft)
            } else if let note = blocked ?? (pr?.state == "merged" ? "Merged. The branch \(branch) is still on \(machine)."
                                            : pr?.isOpen == true && unpushed && ready && canMerge ? "The agent changed files since the last push: push them, then merge." : nil) {
                Divider()
                Text(note)
                    .font(SF.font(11.5))
                    .foregroundStyle(SF.muted)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
                    .background(SF.fill.opacity(0.5))
            }
        }
        .background(SF.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(SF.border))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .padding(.top, 28)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Pull request")
    }

    @ViewBuilder private var titles: some View {
        VStack(alignment: .leading, spacing: 3) {
            if let pr {
                Text(pr.title ?? "Pull request #\(pr.number)")
                    .font(SF.font(13, .medium))
                    .foregroundStyle(SF.ink)
                    .lineLimit(1)
                HStack(spacing: 6) {
                    Text("#\(pr.number)").monospacedDigit().foregroundStyle(SF.ink.opacity(0.8))
                    Text(pr.stateLabel)
                        .font(SF.font(11, .medium))
                        .foregroundStyle(stateInk(pr))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(stateFill(pr), in: Capsule())
                    (Text(pr.base ?? "base") + Text(" ← ") + Text(branch).foregroundColor(SF.ink.opacity(0.8)))
                        .font(SF.mono(11.5))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
            } else {
                Text("Ready for a pull request").font(SF.font(13, .medium)).foregroundStyle(SF.ink)
                (Text("The changes are on ") + Text(branch).font(SF.mono(11.5)).foregroundColor(SF.ink.opacity(0.8)) + Text(" in \(repo) on \(machine)."))
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder private var actionButton: some View {
        if action == .view {
            if let pr, let url = URL(string: pr.url) {
                Link(destination: url) { Label("View on GitHub", systemImage: "arrow.up.right.square") }
                    .buttonStyle(SecondaryButtonStyle())
            }
        } else {
            HStack(spacing: 6) {
                let button = Button(action: primary) {
                    Label(label, systemImage: running ? "hourglass" : action == .merge ? "arrow.triangle.merge" : "arrow.triangle.pull")
                }
                .disabled(running || blocked != nil)
                .help(action == .merge
                      ? "Merges #\(pr?.number ?? 0) into \(base) on GitHub, from \(machine), if its last commit is still the one pushed from here"
                      : action == .push
                      ? "Choose the files, then commit and push \(branch) to #\(pr?.number ?? 0) with the GitHub CLI on \(machine)"
                      : "Choose the files, title and description, then push \(branch) and open a pull request with the GitHub CLI on \(machine)")
                if action == .merge {
                    button.buttonStyle(MergeButtonStyle(confirming: confirming))
                } else if pr == nil {
                    button.buttonStyle(PrimaryButtonStyle())
                } else {
                    button.buttonStyle(SecondaryButtonStyle())
                }
                if let pr {
                    Menu {
                        if pr.isOpen {
                            if action == .merge {
                                Button("Push changes…", action: onPush).disabled(blocked != nil)
                            }
                            ForEach(MergeMethod.allCases, id: \.self) { m in
                                Button(m.title) {
                                    if confirming, m == method { primary() } else { askToMerge(m) }
                                }
                                .disabled(mergeOff != nil)
                            }
                            if let mergeOff { Text(mergeOff) }
                            Divider()
                        }
                        if let url = URL(string: pr.url) {
                            Button("View on GitHub") { NSWorkspace.shared.open(url) }
                        }
                        Button("Copy link") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(pr.url, forType: .string)
                            stores.toasts.show("Link copied")
                        }
                        if pr.isOpen {
                            Button("Check status now", action: onCheck)
                        }
                    } label: {
                        Image(systemName: "chevron.down").font(.system(size: 10, weight: .semibold))
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                    .help("More pull request actions")
                }
            }
        }
    }

    private var rows: [AnyView] {
        var out: [AnyView] = []
        if pr == nil, let changes {
            out.append(AnyView(row(icon: "doc.text", tint: SF.muted, text: "\(changes.fileCount) file\(changes.fileCount == 1 ? "" : "s") changed  +\(changes.additions) −\(changes.deletions)", link: ("Review", onReview))))
        }
        guard let pr, pr.isOpen else { return out }
        if let checks = pr.checks {
            if checks.failed > 0 {
                let names = checks.failing.map(\.name).joined(separator: ", ")
                let ask = "The checks failed on pull request #\(pr.number)\(names.isEmpty ? "" : " (\(names))"). Find out why and fix it."
                out.append(AnyView(row(icon: "xmark.circle", tint: SF.errInk, text: checks.summary, link: theirs ? nil : ("Ask to fix", { onAsk(ask) }))))
            } else if checks.pending > 0 {
                out.append(AnyView(row(icon: "clock", tint: SF.queueInk, text: checks.summary)))
            } else {
                out.append(AnyView(row(icon: "checkmark.circle", tint: SF.okInk, text: checks.summary)))
            }
        }
        switch pr.review {
        case "approved":
            out.append(AnyView(row(icon: "person.crop.circle.badge.checkmark", tint: SF.okInk, text: "Approved")))
        case "changes_requested":
            let open: () -> Void = { if let url = URL(string: pr.url) { NSWorkspace.shared.open(url) } }
            out.append(AnyView(row(icon: "person.crop.circle.badge.xmark", tint: SF.queueInk, text: "Changes requested", link: ("Read the review", open))))
        case "review_required":
            out.append(AnyView(row(icon: "eye", tint: SF.muted, text: "Waiting for a review")))
        default:
            break
        }
        if pr.conflicts {
            let ask = "Pull request #\(pr.number) conflicts with \(base). Merge \(base) into this branch and resolve the conflicts."
            out.append(AnyView(row(icon: "exclamationmark.triangle", tint: SF.errInk, text: "Conflicts with \(base)", link: theirs ? nil : ("Ask to fix", { onAsk(ask) }))))
        }
        return out
    }

    private func row(icon: String, tint: Color, text: String, link: (String, () -> Void)? = nil) -> some View {
        HStack(spacing: 8) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(tint).frame(width: 14)
            Text(text).font(SF.font(12)).foregroundStyle(SF.ink.opacity(0.8)).lineLimit(2)
            Spacer(minLength: 8)
            if let link {
                Button(link.0, action: link.1)
                    .buttonStyle(.plain)
                    .font(SF.font(12, .medium))
                    .foregroundStyle(SF.ink.opacity(0.8))
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 7)
    }

    private func stateInk(_ pr: PullRequest) -> Color {
        pr.state == "merged" ? SF.merged : pr.state == "closed" ? SF.errInk : pr.draft ? SF.muted : SF.okInk
    }

    private func stateFill(_ pr: PullRequest) -> Color {
        pr.state == "merged" ? SF.merged.opacity(0.12) : pr.state == "closed" ? SF.errSoft : pr.draft ? SF.fill : SF.okSoft
    }
}

/// Merge: the green of an open pull request, ringed while it waits for the second click.
private struct MergeButtonStyle: ButtonStyle {
    var confirming: Bool

    func makeBody(configuration: Configuration) -> some View {
        Face(configuration: configuration, confirming: confirming)
    }

    private struct Face: View {
        var configuration: Configuration
        var confirming: Bool
        @Environment(\.isEnabled) private var isEnabled

        var body: some View {
            configuration.label
                .font(SF.font(13, .medium))
                .foregroundStyle(.white)
                .padding(.horizontal, 14)
                .frame(height: 32)
                .background(SF.ok, in: Capsule())
                .overlay(Capsule().strokeBorder(SF.ok.opacity(confirming ? 0.45 : 0), lineWidth: 3).padding(-3))
                .contentShape(Capsule())
                .opacity(isEnabled ? (configuration.isPressed ? 0.8 : 1) : 0.4)
        }
    }
}
