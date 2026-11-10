import SidefleetKit
import SwiftUI

/// The review's file tree: folders, the changed files under them, a box to filter them by path, and
/// how many you've marked Viewed.
struct FileTreeList: View {
    /// The whole tree; the filter narrows what's shown.
    var rows: [FileTreeRow]
    var active: String?
    var isViewed: (String) -> Bool
    @Binding var filter: String
    var onPick: (String) -> Void

    private var files: [DiffFile] { FileTree.files(rows) }

    var body: some View {
        let shown = FileTree.filter(rows, query: filter)
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(SF.muted)
                TextField("Filter files", text: $filter)
                    .textFieldStyle(.plain)
                    .font(SF.font(12.5))
                    // Esc clears what's typed first; with nothing typed it goes on to close the review.
                    .onExitCommand(perform: filter.isEmpty ? nil : { filter = "" })
                if !filter.isEmpty {
                    Button { filter = "" } label: {
                        Image(systemName: "xmark.circle.fill").font(.system(size: 11)).foregroundStyle(SF.faint)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Clear filter")
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 32)
            .background(SF.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(SF.border))
            .padding(10)
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(shown) { row in
                        switch row {
                        case .dir(let name, let depth, _):
                            HStack(spacing: 6) {
                                Image(systemName: "folder").font(.system(size: 11)).opacity(0.7)
                                Text(name).lineLimit(1).truncationMode(.middle)
                                Spacer(minLength: 0)
                            }
                            .font(SF.font(12.5))
                            .foregroundStyle(SF.muted)
                            .padding(.leading, CGFloat(depth) * 14 + 8)
                            .padding(.trailing, 8)
                            .frame(height: 28)
                        case .file(let file, let depth):
                            fileRow(file, depth: depth)
                        }
                    }
                    if shown.isEmpty {
                        Text("No files match.")
                            .font(SF.font(12))
                            .foregroundStyle(SF.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 16)
                    }
                }
                .padding(.horizontal, 6)
                .padding(.bottom, 12)
            }
            Divider()
            HStack(spacing: 10) {
                Text("\(files.filter { isViewed($0.path) }.count) of \(files.count) viewed").monospacedDigit()
                Spacer()
                HStack(spacing: 3) { Kbd(text: "j"); Kbd(text: "k"); Text("files") }
                HStack(spacing: 3) { Kbd(text: "v"); Text("viewed") }
            }
            .font(SF.font(11))
            .foregroundStyle(SF.faint)
            .padding(.horizontal, 12)
            .frame(height: 32)
        }
        .accessibilityLabel("Changed files")
    }

    private func fileRow(_ file: DiffFile, depth: Int) -> some View {
        let seen = isViewed(file.path)
        return Button { onPick(file.path) } label: {
            HStack(spacing: 6) {
                FileStatusIcon(status: file.status)
                Text(Format.splitPath(file.path).name)
                    .font(SF.font(12.5))
                    .foregroundStyle(seen ? SF.muted : SF.ink.opacity(active == file.path ? 1 : 0.85))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 4)
                if seen {
                    Image(systemName: "checkmark").font(.system(size: 11, weight: .semibold)).foregroundStyle(SF.okInk).accessibilityLabel("Viewed")
                } else {
                    DiffStatView(add: file.additions, del: file.deletions, blocks: false, size: 10.5).opacity(0.8)
                }
            }
            .padding(.leading, CGFloat(depth) * 14 + 8)
            .padding(.trailing, 8)
            .frame(height: 28)
            .hoverBackground(active: active == file.path)
        }
        .buttonStyle(.plain)
        .help(file.path)
    }
}

/// A parsed, coloured diff, kept so that scrolling the review (which redraws its cards as the file
/// in view changes) doesn't parse and tokenize every open file again.
@MainActor
final class DiffRenderCache {
    static let shared = DiffRenderCache()

    struct Entry {
        var hunks: [DiffHunk]
        /// Per hunk, per line, in the hunk's order.
        var tokens: [[[Token]]]
    }

    private var entries: [String: Entry] = [:]

    func entry(for file: DiffFile, patch: String, hideWhitespace: Bool) -> Entry {
        let key = "\(hideWhitespace ? "w" : "-")\u{0}\(file.path)\u{0}\(patch.utf8.count)\u{0}\(patch.hashValue)"
        if let hit = entries[key] { return hit }
        let hunks = DiffParser.parse(patch, hideWhitespace: hideWhitespace)
        let entry = Entry(hunks: hunks, tokens: Highlighter.highlight(hunks: hunks, language: Highlighter.language(for: file.path)))
        // Threads come and go; nothing here is worth keeping for long.
        if entries.count > 300 { entries.removeAll() }
        entries[key] = entry
        return entry
    }
}

/// Where each file's card sits in the review's scroll area, so the list can follow the scrolling.
struct FileTopsKey: PreferenceKey {
    static let defaultValue: [String: CGFloat] = [:]

    static func reduce(value: inout [String: CGFloat], nextValue: () -> [String: CGFloat]) {
        value.merge(nextValue()) { $1 }
    }
}
