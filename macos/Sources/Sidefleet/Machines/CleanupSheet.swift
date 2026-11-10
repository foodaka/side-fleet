import SidefleetKit
import SwiftUI

/// Which of a machine's cleanup lists to show.
enum CleanupView: String, Identifiable {
    /// What a cleanup would remove and keep, asked of the machine now.
    case preview
    /// What the last cleanup removed.
    case removed
    var id: String { rawValue }
}

/// The line on a machine's card about its worktrees, and the button that cleans them up.
struct CleanupRow: View {
    var worker: Worker
    @Environment(Stores.self) private var stores
    @State private var showing: CleanupView?

    private var fleet: FleetStore { stores.fleet }
    private var canUpdate: Bool { fleet.canUpdate(worker) }
    /// Only a worker whose build answers, and only its owner may ask.
    private var canClean: Bool { canUpdate && worker.online && worker.capabilities.canCleanWorktrees }

    var body: some View {
        if canClean || worker.worktreeCleanup != nil {
            HStack(alignment: .center, spacing: 12) {
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text(worker.worktreeCleanup?.note(now: fleet.now) ?? "Worktrees of finished tasks stay on this machine until cleaned up")
                        .foregroundStyle(SF.muted)
                        .fixedSize(horizontal: false, vertical: true)
                    if let cleanup = worker.worktreeCleanup, cleanup.removed > 0, canUpdate {
                        Button("View") { showing = .removed }
                            .buttonStyle(.plain)
                            .underline()
                            .foregroundStyle(SF.muted)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if worker.worktreeCleanupPending == "remove" {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.mini)
                        Text("Cleaning up")
                    }
                    .foregroundStyle(SF.runInk)
                    .fixedSize()
                } else if canClean {
                    Button("Clean up") { showing = .preview }
                        .buttonStyle(SecondaryButtonStyle(height: 26))
                        .fixedSize()
                }
            }
            .font(SF.font(12))
            .sheet(item: $showing) { CleanupSheet(worker: worker, view: $0) }
        }
    }
}

/// A machine's worktrees before anything is removed: each with its task, its size and why it
/// would go or stay. Removing sends the ids that were on screen, and the machine checks each again.
struct CleanupSheet: View {
    var worker: Worker
    var view: CleanupView
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var items: [WorktreeItem]?
    @State private var at: Date?
    @State private var problem = ""
    @State private var removing = false

    /// The machine walks every worktree to size it, which can take a while.
    private static let wait: TimeInterval = 5 * 60
    private static let poll: Duration = .milliseconds(1500)

    private var going: [WorktreeItem] { (items ?? []).going }
    private var staying: [WorktreeItem] { (items ?? []).staying }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(view == .removed ? "Removed worktrees" : "Clean up \(worker.displayName)").font(SF.font(15, .semibold))
            Text(description)
                .font(SF.font(13))
                .lineSpacing(2)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) { lists }
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: 420)
            .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                if view == .preview {
                    Button("Cancel") { dismiss() }.buttonStyle(SecondaryButtonStyle()).keyboardShortcut(.cancelAction)
                    Button("Remove \(going.count) worktree\(going.count == 1 ? "" : "s")", role: .destructive, action: removeAll)
                        .buttonStyle(SecondaryButtonStyle(tint: SF.errInk))
                        .disabled(going.isEmpty || removing)
                } else {
                    Button("Done") { dismiss() }.buttonStyle(PrimaryButtonStyle()).keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(20)
        .frame(width: 500)
        // Ends with the sheet, so closing it stops asking.
        .task { await load() }
    }

    private var description: String {
        if view == .removed {
            return (at.map { "Cleaned up \(Format.rel($0, now: stores.fleet.now)). " } ?? "") + "A follow-up on any of these tasks rebuilds its worktree."
        }
        return "Files git ignores, like a local .env, go with the worktree. Branches stay, so a follow-up rebuilds it."
    }

    @ViewBuilder private var lists: some View {
        if !problem.isEmpty {
            note(problem, color: SF.errInk)
        } else if items == nil {
            note("Checking every worktree on \(worker.displayName)…", color: SF.muted)
        } else if view == .removed {
            if going.isEmpty {
                note("Nothing was removed.", color: SF.muted)
            } else {
                rows(going)
            }
        } else {
            Text("Will remove (\(going.count)) · up to \(Format.fileSize(going.reduce(0) { $0 + $1.bytes }))").font(SF.font(12, .medium))
            if going.isEmpty {
                Text("Nothing is safe to remove right now.").font(SF.font(13)).foregroundStyle(SF.muted).padding(.vertical, 12)
            } else {
                rows(going)
            }
            if !staying.isEmpty {
                Text("Will keep (\(staying.count))").font(SF.font(12, .medium)).padding(.top, 16)
                rows(staying)
            }
        }
    }

    private func note(_ text: String, color: Color) -> some View {
        Text(text)
            .font(SF.font(13))
            .foregroundStyle(color)
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 24)
    }

    private func rows(_ list: [WorktreeItem]) -> some View {
        ForEach(Array(list.enumerated()), id: \.element.id) { i, item in
            if i > 0 { Divider() }
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title).font(SF.font(13, .medium)).lineLimit(1).help(item.title)
                HStack(spacing: 12) {
                    Text("\(item.reason) · \(item.repo)").lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
                    Text(Format.fileSize(item.bytes)).monospacedDigit().fixedSize()
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
            }
            .padding(.vertical, 8)
        }
    }

    private func load() async {
        let api = stores.api
        do {
            if view == .removed {
                let status = try await api.cleanupStatus(worker: worker.id)
                items = status.cleanup?.items ?? []
                at = status.cleanup?.totals.at
                return
            }
            try await api.previewCleanup(worker: worker.id)
            let deadline = Date().addingTimeInterval(Self.wait)
            while !Task.isCancelled {
                let status = try await api.cleanupStatus(worker: worker.id)
                if Task.isCancelled { return }
                if let preview = status.preview {
                    items = preview.items
                    return
                }
                if Date() > deadline {
                    problem = "\(worker.displayName) hasn't answered yet. Check that it's online, then try again."
                    return
                }
                try await Task.sleep(for: Self.poll)
            }
        } catch is CancellationError {
            return
        } catch {
            if !Task.isCancelled { problem = error.localizedDescription }
        }
    }

    private func removeAll() {
        guard !removing, !going.isEmpty else { return }
        removing = true
        let ids = going.map(\.rootID)
        Task {
            do {
                try await stores.api.removeWorktrees(worker: worker.id, ids: ids)
                dismiss()
                await stores.fleet.refresh()
            } catch {
                stores.toasts.error("Could not start the cleanup", error)
            }
            removing = false
        }
    }
}
