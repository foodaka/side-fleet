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
    var changes: DiffSet?
    var onPush: () -> Void
    var onCheck: () -> Void
    var onReview: () -> Void
    /// Put a request to the agent in the reply box.
    var onAsk: (String) -> Void
    @Environment(Stores.self) private var stores

    private enum Action { case view, push, open }

    private var action: Action { theirs || pr?.state == "merged" ? .view : pr?.isOpen == true ? .push : .open }
    private var base: String { pr?.base ?? "the default branch" }

    /// Why the action can't run now, said under it rather than hidden in a tooltip.
    private var blocked: String? {
        if action == .view || pushing { return nil }
        if !machineOnline { return "\(machine) is offline, so it can't push the branch. It will once it's back." }
        if busy { return "Waiting for the agent to finish before pushing." }
        return nil
    }

    private var label: String {
        if pushing { return pr?.isOpen == true ? "Pushing…" : "Opening…" }
        switch action {
        case .push: return "Push changes"
        case .open: return pr?.state == "closed" ? "Open a new one" : "Open pull request"
        case .view: return "View on GitHub"
        }
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
            if let note = blocked ?? (pr?.state == "merged" ? "Merged. The branch \(branch) is still on \(machine)." : nil) {
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
                let button = Button(action: onPush) {
                    Label(label, systemImage: pushing ? "hourglass" : "arrow.triangle.pull")
                }
                .disabled(pushing || blocked != nil)
                .help(action == .push
                      ? "Commits what's left and pushes \(branch) to #\(pr?.number ?? 0), with the GitHub CLI on \(machine)"
                      : "Commits what's left, pushes \(branch) to origin and opens a pull request with the GitHub CLI on \(machine)")
                if pr == nil { button.buttonStyle(PrimaryButtonStyle()) } else { button.buttonStyle(SecondaryButtonStyle()) }
                if let pr {
                    Menu {
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
