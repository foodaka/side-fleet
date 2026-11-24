import SidefleetKit
import SwiftUI

/// Before opening a pull request or pushing to one: which changed files go in, and for a new one
/// its title and description (or the agent writes them) and whether it's a draft. A port of
/// web/src/lib/sf/thread/PrDialog.svelte.
struct PRSheet: View {
    /// Push to the open pull request (its number), or open a new one.
    var openNumber: Int?
    var files: [DiffFile]
    var fileCount: Int
    var branch: String
    var machine: String
    /// Who writes the title and description: the thread's agent, by name.
    var agent: String
    /// The machine's Sidefleet can leave files out, write it and open drafts.
    var canManage: Bool
    var onSubmit: (PROptions) async -> Bool
    @Environment(\.dismiss) private var dismiss
    @State private var picked: Set<String> = []
    @State private var title = ""
    @State private var body_ = ""
    @State private var draft = false
    @State private var busy = false

    private var opening: Bool { openNumber == nil }
    private var unlisted: Int { max(0, fileCount - files.count) }
    private var exclude: [String] { files.map(\.path).filter { !picked.contains($0) } }
    private var writes: Bool { opening && canManage && title.trimmingCharacters(in: .whitespaces).isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text(opening ? "Open a pull request" : "Push changes to #\(openNumber ?? 0)").font(SF.font(15, .semibold))
                (Text("Commits the ticked files on ") + Text(branch).font(SF.mono(12)) + Text(" and pushes it with the GitHub CLI on \(machine). Files you untick stay in the worktree, uncommitted."))
                    .font(SF.font(12.5))
                    .foregroundStyle(SF.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !files.isEmpty { fileList }
            if opening {
                TextField(canManage ? "Title: leave empty and \(agent) writes it" : "Title: leave empty to use the task", text: $title)
                    .textFieldStyle(.roundedBorder)
                    .font(SF.font(13))
                TextEditor(text: $body_)
                    .font(SF.font(13))
                    .frame(height: 90)
                    .scrollContentBackground(.hidden)
                    .padding(6)
                    .background(SF.card, in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(SF.input))
                    .overlay(alignment: .topLeading) {
                        if body_.isEmpty {
                            Text(writes ? "Description: leave empty and \(agent) writes it from the changes" : "Description (optional)")
                                .font(SF.font(13)).foregroundStyle(SF.faint).padding(.horizontal, 11).padding(.vertical, 6).allowsHitTesting(false)
                        }
                    }
            }
            HStack(spacing: 12) {
                if opening {
                    Toggle("Open as a draft", isOn: $draft)
                        .toggleStyle(.checkbox)
                        .disabled(!canManage)
                        .help(canManage ? "" : "Update Sidefleet on \(machine) to open drafts")
                }
                if writes {
                    Label("\(agent) writes the title\(body_.isEmpty ? " and description" : "")", systemImage: "sparkles")
                        .font(SF.font(11.5))
                        .foregroundStyle(SF.muted)
                }
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button {
                    busy = true
                    Task {
                        var o = PROptions(exclude: exclude)
                        if opening {
                            o.title = title
                            o.body = body_
                            o.draft = draft
                        }
                        if await onSubmit(o) { dismiss() }
                        busy = false
                    }
                } label: {
                    Label(busy ? "Starting…" : opening ? "Open pull request" : "Push changes", systemImage: "arrow.triangle.pull")
                }
                .buttonStyle(PrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(busy || (opening && !files.isEmpty && picked.isEmpty && unlisted == 0))
            }
            .font(SF.font(12.5))
        }
        .padding(22)
        .frame(width: 560)
        .onAppear { picked = Set(files.map(\.path)) }
    }

    private var fileList: some View {
        VStack(alignment: .leading, spacing: 6) {
            VStack(spacing: 0) {
                HStack {
                    Text("\(picked.count) of \(files.count) file\(files.count == 1 ? "" : "s")").monospacedDigit()
                    Spacer()
                    if canManage {
                        Button("All") { picked = Set(files.map(\.path)) }.buttonStyle(.plain)
                        Button("None") { picked = [] }.buttonStyle(.plain)
                    }
                }
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(SF.fill.opacity(0.5))
                Divider()
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(files, id: \.path) { f in
                            Toggle(isOn: Binding(get: { picked.contains(f.path) }, set: { on in
                                if on { picked.insert(f.path) } else { picked.remove(f.path) }
                            })) {
                                HStack {
                                    Text(f.path)
                                        .font(SF.mono(12))
                                        .foregroundStyle(picked.contains(f.path) ? SF.ink : SF.muted)
                                        .strikethrough(!picked.contains(f.path), color: SF.muted.opacity(0.5))
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                    Spacer()
                                    Text("+\(f.additions)").foregroundStyle(SF.diffAddInk)
                                    Text("−\(f.deletions)").foregroundStyle(SF.diffDelInk)
                                }
                                .font(SF.font(11.5))
                                .monospacedDigit()
                            }
                            .toggleStyle(.checkbox)
                            .disabled(!canManage)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 4)
                        }
                    }
                }
                .frame(maxHeight: 200)
            }
            .background(SF.card, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(SF.border))
            .clipShape(RoundedRectangle(cornerRadius: 10))
            Text(!canManage
                 ? "Update Sidefleet on \(machine) to leave files out; everything changed goes in."
                 : (unlisted > 0 ? "\(unlisted) more changed file\(unlisted == 1 ? "" : "s") not listed here go in too. " : "") + "Files already pushed to the branch stay in it.")
                .font(SF.font(11.5))
                .foregroundStyle(SF.muted)
        }
    }
}
