import SidefleetKit
import SwiftUI

/// Reviewing what a thread changed: every file, unified or side by side, with a list to move around in.
/// Takes over the window's detail column; Esc goes back to the thread.
struct ReviewScreen: View {
    var thread: ThreadStore
    var request: ReviewRequest
    /// What's marked Viewed or folded. The thread's screen holds it, so it outlives this one.
    @Binding var state: ReviewState
    var onClose: () -> Void
    @Environment(Stores.self) private var stores
    @Environment(\.openSettings) private var openSettings
    @State private var scope = "all"
    @State private var active: String?
    @State private var jump: String?
    @State private var filter = ""
    /// Set while a jump scrolls into place, so the scrolling on the way doesn't move the highlight.
    @State private var jumping = false
    /// The file tree can be tucked away; the choice sticks.
    @AppStorage("sf.diffTree") private var showTree = true
    @FocusState private var keyFocus: Bool

    private struct Scope: Identifiable {
        var id: String
        var label: String
        var hint: String?
        var set: DiffSet
    }

    private var scopes: [Scope] {
        guard let all = thread.changes else { return [] }
        let changed = thread.turns.filter { $0.turnDiff != nil }
        var out = [Scope(id: "all", label: "All changes", set: all)]
        if changed.count >= 2 {
            for t in changed {
                let n = (thread.turns.firstIndex { $0.id == t.id } ?? 0) + 1
                out.append(Scope(id: t.id, label: t.run.parentID != nil ? "Message \(n)" : "First message", hint: t.run.task, set: t.turnDiff!))
            }
        }
        return out
    }

    private var current: Scope? { scopes.first { $0.id == scope } ?? scopes.first }
    private var scopeID: String { current?.id ?? "" }
    /// The tree, with single-child folders folded into one row.
    private var rows: [FileTreeRow] { FileTree.rows(current?.set.files ?? []) }
    /// Files in tree order, so the list and the tree read the same way down.
    private var files: [DiffFile] { FileTree.files(rows) }
    private func isOpen(_ path: String) -> Bool { state.isOpen(scopeID, path) }
    private func isViewed(_ path: String) -> Bool { state.isViewed(scopeID, path) }

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            Divider()
            HSplitView {
                if showTree {
                    FileTreeList(rows: rows, active: active, isViewed: isViewed, filter: $filter, onPick: go)
                        .frame(minWidth: 200, idealWidth: 280, maxWidth: 420)
                }
                diffs
                    .frame(minWidth: 420)
            }
        }
        .background(SF.background)
        .focusable()
        .focusEffectDisabled()
        .focused($keyFocus)
        .onExitCommand(perform: onClose)
        .onKeyPress(characters: .init(charactersIn: "jkvw")) { press in
            guard press.modifiers.isEmpty else { return .ignored }
            let files = files
            let i = files.firstIndex { $0.path == active } ?? 0
            switch press.characters {
            case "j":
                if !files.isEmpty { go(files[min(files.count - 1, i + 1)].path) }
            case "k":
                if !files.isEmpty { go(files[max(0, i - 1)].path) }
            case "v":
                if let active { state.setViewed(scopeID, active, !isViewed(active)) }
            case "w":
                stores.prefs.set(\.diffHideWhitespace, !stores.prefs.diffHideWhitespace)
            default:
                return .ignored
            }
            return .handled
        }
        .onAppear {
            scope = scopes.contains { $0.id == request.scope } ? request.scope : "all"
            active = request.focus ?? files.first?.path
            prepareCollapse()
            jump = request.focus
            keyFocus = true
        }
        .onChange(of: scope) { _, _ in
            active = files.first?.path
            prepareCollapse()
            // Another set of changes starts from its top.
            jump = files.first?.path
        }
    }

    /// With "open collapsed" on, each set of changes opens folded the first time (bar the file asked for).
    private func prepareCollapse() {
        guard stores.prefs.diffCollapsed, current != nil else { return }
        state.foldOnce(scopeID, files.map(\.path), except: request.focus)
    }

    private func go(_ path: String) {
        state.reveal(scopeID, path)
        active = path
        jumping = true
        jump = path
    }

    /// The file in view: the last one whose card has reached the top of the scroll area.
    private func follow(_ tops: [String: CGFloat]) {
        guard !jumping else { return }
        var at: String?
        for f in files {
            guard let y = tops[f.path] else { continue }
            if y <= 48 { at = f.path } else { break }
        }
        // Cards far above are let go of, so nothing may have reported: keep what's highlighted then.
        if let at, at != active { active = at }
    }

    // MARK: Toolbar

    private var toolbar: some View {
        HStack(spacing: 10) {
            Button(action: onClose) {
                Image(systemName: "chevron.left").font(.system(size: 12, weight: .medium)).frame(width: 28, height: 28)
            }
            .buttonStyle(QuietButtonStyle(cornerRadius: 6))
            .help("Back to the thread  Esc")
            .accessibilityLabel("Back to the thread")
            Button { showTree.toggle() } label: {
                Image(systemName: "sidebar.left").font(.system(size: 12)).frame(width: 28, height: 28)
            }
            .buttonStyle(QuietButtonStyle(cornerRadius: 6))
            .help(showTree ? "Hide file tree" : "Show file tree")
            .accessibilityLabel(showTree ? "Hide file tree" : "Show file tree")
            Text("Changes").font(SF.font(15, .semibold))
            if scopes.count > 1, let current {
                PickerPill(
                    options: scopes.map { s in
                        PickerOption(id: s.id, title: "\(s.label) · \(s.set.fileCount) file\(s.set.fileCount == 1 ? "" : "s")", hint: s.hint.map { String($0.prefix(80)) })
                    },
                    selected: current.id, height: 28, width: 300, accessibility: "Which changes", onPick: { scope = $0 }
                ) {
                    Text(current.label).font(SF.font(13))
                }
            }
            if let set = current?.set {
                Text("\(set.fileCount.formatted()) file\(set.fileCount == 1 ? "" : "s")").font(SF.font(12)).monospacedDigit().foregroundStyle(SF.muted)
                DiffStatView(add: set.additions, del: set.deletions)
            }
            Spacer()
            let allClosed = state.allClosed(scopeID, files.map(\.path))
            Button { state.toggleAll(scopeID, files.map(\.path)) } label: {
                Image(systemName: allClosed ? "rectangle.expand.vertical" : "rectangle.compress.vertical")
                    .font(.system(size: 12)).frame(width: 28, height: 28)
            }
            .buttonStyle(QuietButtonStyle(cornerRadius: 6))
            .help(allClosed ? "Expand all" : "Collapse all")
            .accessibilityLabel(allClosed ? "Expand all files" : "Collapse all files")
            Picker("Layout", selection: Binding(get: { stores.prefs.diffView }, set: { stores.prefs.set(\.diffView, $0) })) {
                Label("Unified", systemImage: "rectangle.grid.1x2").tag(DiffLayout.unified)
                Label("Split", systemImage: "rectangle.split.2x1").tag(DiffLayout.split)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 180)
            Menu {
                Toggle("Hide whitespace changes", isOn: Binding(get: { stores.prefs.diffHideWhitespace }, set: { stores.prefs.set(\.diffHideWhitespace, $0) }))
                Toggle("Open with files collapsed", isOn: Binding(get: { stores.prefs.diffCollapsed }, set: { stores.prefs.set(\.diffCollapsed, $0) }))
                Divider()
                Button("All settings…") { openSettings() }
            } label: {
                Image(systemName: "slider.horizontal.3")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            // A dot while whitespace changes are hidden: the diff isn't showing everything.
            .overlay(alignment: .topTrailing) {
                if stores.prefs.diffHideWhitespace {
                    Circle().fill(SF.ink).frame(width: 6, height: 6).offset(x: 2, y: -2).allowsHitTesting(false)
                }
            }
            .help("View options")
            .accessibilityLabel("View options")
        }
        .padding(.horizontal, 14)
        .frame(height: 52)
    }

    // MARK: Diffs

    private var diffs: some View {
        let files = files
        return ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if let set = current?.set, let upstream = set.caughtUpUpstream, let commits = set.caughtUpCommits {
                        Text("Also caught up with \(upstream) (\(commits.formatted()) commit\(commits == 1 ? "" : "s")), not counted above.")
                            .font(SF.font(12))
                            .foregroundStyle(SF.muted)
                    }
                    ForEach(files) { file in
                        FileDiffCard(
                            file: file,
                            open: isOpen(file.path),
                            viewed: isViewed(file.path),
                            active: active == file.path && files.count > 1,
                            layout: stores.prefs.diffView,
                            hideWhitespace: stores.prefs.diffHideWhitespace,
                            onToggle: { state.setOpen(scopeID, file.path, $0) },
                            onViewed: { state.setViewed(scopeID, file.path, $0) },
                            onShowWhitespace: { stores.prefs.set(\.diffHideWhitespace, false) }
                        )
                        .id(file.path)
                        .background {
                            GeometryReader { geo in
                                Color.clear.preference(key: FileTopsKey.self, value: [file.path: geo.frame(in: .named("review")).minY])
                            }
                        }
                        .onAppear { if active == nil { active = file.path } }
                    }
                    if let set = current?.set, set.fileCount > set.files.count {
                        Text("\((set.fileCount - set.files.count).formatted()) more changed files are not listed. See the branch for the full change.")
                            .font(SF.font(12))
                            .foregroundStyle(SF.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 8)
                    }
                    // Room below the last file, so it too can scroll up to the top and become the one in view.
                    Color.clear.frame(height: 320).accessibilityHidden(true)
                }
                .padding(14)
            }
            .coordinateSpace(name: "review")
            .onPreferenceChange(FileTopsKey.self) { follow($0) }
            .onChange(of: jump) { _, path in
                guard let path else { return }
                withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(path, anchor: .top) }
                jump = nil
                Task {
                    // Let the scroll land before the list follows the scrolling again.
                    try? await Task.sleep(for: .milliseconds(350))
                    jumping = false
                }
            }
        }
    }
}

private struct FileDiffCard: View {
    var file: DiffFile
    var open: Bool
    var viewed: Bool
    /// The file in view, when there's more than one to tell apart.
    var active: Bool
    var layout: DiffLayout
    var hideWhitespace: Bool
    var onToggle: (Bool) -> Void
    var onViewed: (Bool) -> Void
    var onShowWhitespace: () -> Void
    /// You asked to see a large diff anyway.
    @State private var forced = false

    var body: some View {
        VStack(spacing: 0) {
            header
            if open {
                Divider()
                body(for: file)
            }
        }
        .card(cornerRadius: 12)
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            if active { RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(SF.ink.opacity(0.18)) }
        }
    }

    private func notice(_ text: String, action: String? = nil, perform: @escaping () -> Void = {}) -> some View {
        VStack(spacing: 8) {
            Text(text).font(SF.font(13)).foregroundStyle(SF.muted).multilineTextAlignment(.center)
            if let action {
                Button(action, action: perform).buttonStyle(SecondaryButtonStyle(height: 26))
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 16)
        .padding(.vertical, 24)
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button { onToggle(!open) } label: {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(SF.muted)
                        .rotationEffect(.degrees(open ? 90 : 0))
                    FileStatusIcon(status: file.status)
                    let parts = Format.splitPath(file.path)
                    (Text(parts.dir).foregroundColor(SF.muted) + Text(parts.name).foregroundColor(SF.ink))
                        .font(SF.mono(12))
                        .lineLimit(1)
                        .truncationMode(.head)
                    if let old = file.oldPath, old != file.path {
                        Text("from \(old)").font(SF.mono(11)).foregroundStyle(SF.faint).lineLimit(1)
                    }
                    Spacer(minLength: 6)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            DiffStatView(add: file.additions, del: file.deletions, blocks: false, size: 11)
            CopyButton(text: { file.path }, size: 10).help("Copy the path")
            Toggle("Viewed", isOn: Binding(get: { viewed }, set: onViewed))
                .toggleStyle(.checkbox)
                .font(SF.font(12))
                .foregroundStyle(SF.muted)
        }
        .padding(.horizontal, 12)
        .frame(height: 38)
    }

    @ViewBuilder private func body(for file: DiffFile) -> some View {
        if let patch = file.patch, !patch.isEmpty {
            if file.isLarge, !forced {
                notice("Large diffs are not shown by default. This one changes \((file.additions + file.deletions).formatted()) lines.", action: "Load diff") { forced = true }
            } else {
                let parsed = DiffRenderCache.shared.entry(for: file, patch: patch, hideWhitespace: hideWhitespace)
                if DiffParser.changes(parsed.hunks) {
                    // A new or deleted file has one side only: split would leave half the width empty.
                    FileHunks(hunks: parsed.hunks, tokens: parsed.tokens, layout: file.oneSided ? .unified : layout)
                } else {
                    notice("Only whitespace changed in this file.", action: "Show whitespace", perform: onShowWhitespace)
                }
            }
        } else {
            notice(file.noPatchReason)
        }
    }
}

private struct FileHunks: View {
    var hunks: [DiffHunk]
    /// Each line's syntax tokens, by hunk and then by line.
    var tokens: [[[Token]]]
    var layout: DiffLayout

    var body: some View {
        // Rows are lazy, so a long diff only draws what's on screen.
        LazyVStack(alignment: .leading, spacing: 0) {
            ForEach(Array(hunks.enumerated()), id: \.element.id) { i, hunk in
                // A hunk at the very top with no heading has nothing to say.
                if hunk.hasHeader { HunkHeader(hunk: hunk) }
                let byLine = Dictionary(zip(hunk.lines.map(\.id), i < tokens.count ? tokens[i] : []), uniquingKeysWith: { first, _ in first })
                switch layout {
                case .unified:
                    ForEach(hunk.lines) { UnifiedLine(line: $0, tokens: byLine[$0.id]) }
                case .split:
                    ForEach(DiffParser.splitRows(hunk.lines)) { SplitLine(row: $0, tokens: byLine) }
                }
            }
        }
    }
}

private struct HunkHeader: View {
    var hunk: DiffHunk

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "chevron.up.chevron.down").font(.system(size: 9)).opacity(0.5)
            if hunk.gap > 0 {
                Text("\(hunk.gap.formatted()) unchanged line\(hunk.gap == 1 ? "" : "s")").font(SF.font(11.5))
            }
            if !hunk.section.isEmpty {
                Text(hunk.section).font(SF.mono(11.5)).lineLimit(1)
            }
            Spacer()
        }
        .foregroundStyle(SF.muted)
        .padding(.horizontal, 12)
        .frame(height: 28)
        .background(SF.diffHunk)
    }
}

/// A line's text with its syntax colours and word-level marks.
private func styledLine(_ line: DiffLine, tokens: [Token]?) -> AttributedString {
    // The tokens were worked out down the whole hunk, so a comment opened lines above still colours this one.
    let tokens = tokens ?? [Token(text: line.text, kind: .plain)]
    var out = AttributedString()
    var at = 0
    let word = line.kind == .add ? SF.diffAddWord : SF.diffDelWord
    for token in tokens {
        // Split the token wherever a mark starts or ends.
        var rest = Substring(token.text)
        while !rest.isEmpty {
            let inMark = line.marks.first { $0.contains(at) }
            let nextEdge = line.marks.flatMap { [$0.lowerBound, $0.upperBound] }.filter { $0 > at }.min() ?? .max
            let take = min(rest.count, nextEdge - at)
            var piece = AttributedString(String(rest.prefix(take)))
            piece.foregroundColor = token.kind.color
            if inMark != nil { piece.backgroundColor = word }
            out += piece
            rest = rest.dropFirst(take)
            at += take
        }
    }
    return out
}

private struct UnifiedLine: View {
    var line: DiffLine
    var tokens: [Token]?

    private var lineColor: Color {
        switch line.kind {
        case .add: SF.diffAddLine
        case .del: SF.diffDelLine
        case .context: .clear
        }
    }

    private var numberColor: Color {
        switch line.kind {
        case .add: SF.diffAddNum
        case .del: SF.diffDelNum
        case .context: .clear
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            LineNumber(value: line.old, kind: line.kind).background(numberColor)
            LineNumber(value: line.new, kind: line.kind).background(numberColor)
            Sign(kind: line.kind)
            Text(styledLine(line, tokens: tokens))
                .font(SF.mono(12))
                .lineSpacing(3)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.trailing, 16)
                .padding(.vertical, 2)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(lineColor)
    }
}

private struct SplitLine: View {
    var row: SplitRow
    /// The hunk's tokens by line.
    var tokens: [String: [Token]]

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            side(row.left, isLeft: true)
            Rectangle().fill(SF.border).frame(width: 1)
            side(row.right, isLeft: false)
        }
    }

    private func side(_ line: DiffLine?, isLeft: Bool) -> some View {
        Group {
            if let line {
                HStack(alignment: .top, spacing: 0) {
                    LineNumber(value: isLeft ? line.old : line.new, kind: line.kind)
                        .background(line.kind == .add ? SF.diffAddNum : line.kind == .del ? SF.diffDelNum : .clear)
                    Sign(kind: line.kind)
                    Text(styledLine(line, tokens: tokens[line.id]))
                        .font(SF.mono(12))
                        .lineSpacing(3)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.trailing, 12)
                        .padding(.vertical, 2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .background(line.kind == .add ? SF.diffAddLine : line.kind == .del ? SF.diffDelLine : .clear)
            } else {
                // Nothing on this side: faint hatching, as the page draws it.
                HatchFill()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

private struct HatchFill: View {
    var body: some View {
        Canvas { ctx, size in
            var x = -size.height
            while x < size.width {
                var p = Path()
                p.move(to: CGPoint(x: x, y: size.height))
                p.addLine(to: CGPoint(x: x + size.height, y: 0))
                ctx.stroke(p, with: .color(SF.ink.opacity(0.05)), lineWidth: 1)
                x += 6
            }
        }
        .frame(minHeight: 20)
    }
}

private struct LineNumber: View {
    var value: Int?
    var kind: DiffLineKind

    var body: some View {
        Text(value.map(String.init) ?? "")
            .font(SF.mono(11))
            .monospacedDigit()
            .foregroundStyle(kind == .add ? SF.diffAddInk.opacity(0.75) : kind == .del ? SF.diffDelInk.opacity(0.75) : SF.faint)
            .frame(width: 44, alignment: .trailing)
            .padding(.trailing, 8)
            .padding(.vertical, 3)
            .accessibilityHidden(true)
    }
}

private struct Sign: View {
    var kind: DiffLineKind

    var body: some View {
        Text(kind == .add ? "+" : kind == .del ? "−" : " ")
            .font(SF.mono(12))
            .foregroundStyle(kind == .add ? SF.diffAddInk : kind == .del ? SF.diffDelInk : SF.faint)
            .frame(width: 18)
            .padding(.vertical, 2)
            .accessibilityHidden(true)
    }
}
