import SidefleetKit
import SwiftUI

/// Reviewing what a thread changed: every file, unified or side by side, with a list to move around in.
/// Takes over the window's detail column; Esc goes back to the thread.
struct ReviewScreen: View {
    var thread: ThreadStore
    var request: ReviewRequest
    var onClose: () -> Void
    @Environment(Stores.self) private var stores
    @State private var scope = "all"
    @State private var active: String?
    @State private var viewed: Set<String> = []
    @State private var closed: Set<String> = []
    @State private var jump: String?
    @State private var preparedCollapse: Set<String> = []
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
    private var files: [DiffFile] { current?.set.files ?? [] }
    private func key(_ path: String) -> String { "\(current?.id ?? ""):\(path)" }
    private func isOpen(_ path: String) -> Bool { !closed.contains(key(path)) && !viewed.contains(key(path)) }

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            Divider()
            HSplitView {
                fileList
                    .frame(minWidth: 200, idealWidth: 260, maxWidth: 380)
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
            let i = files.firstIndex { $0.path == active } ?? 0
            switch press.characters {
            case "j":
                if !files.isEmpty { go(files[min(files.count - 1, i + 1)].path) }
            case "k":
                if !files.isEmpty { go(files[max(0, i - 1)].path) }
            case "v":
                if let active { setViewed(active, !viewed.contains(key(active))) }
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
        }
    }

    /// With "open collapsed" on, each set of changes opens folded the first time (bar the file asked for).
    private func prepareCollapse() {
        guard stores.prefs.diffCollapsed, let current, files.count >= 2, !preparedCollapse.contains(current.id) else { return }
        preparedCollapse.insert(current.id)
        for f in files where f.path != request.focus { closed.insert(key(f.path)) }
    }

    private func go(_ path: String) {
        active = path
        if !isOpen(path), !viewed.contains(key(path)) { closed.remove(key(path)) }
        jump = path
    }

    private func setViewed(_ path: String, _ on: Bool) {
        if on {
            viewed.insert(key(path))
        } else {
            viewed.remove(key(path))
            closed.remove(key(path))
        }
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
            Text("Changes").font(SF.font(15, .semibold))
            if scopes.count > 1, let current {
                PickerPill(
                    options: scopes.map { PickerOption(id: $0.id, title: $0.label, hint: $0.hint.map { String($0.prefix(80)) }) },
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
            } label: {
                Image(systemName: "slider.horizontal.3")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .accessibilityLabel("Diff options")
        }
        .padding(.horizontal, 14)
        .frame(height: 52)
    }

    // MARK: File list

    private var fileList: some View {
        VStack(spacing: 0) {
            ScrollView {
                LazyVStack(spacing: 1) {
                    ForEach(files) { file in
                        let parts = Format.splitPath(file.path)
                        let seen = viewed.contains(key(file.path))
                        Button { go(file.path) } label: {
                            HStack(spacing: 8) {
                                FileStatusIcon(status: file.status)
                                VStack(alignment: .leading, spacing: 0) {
                                    Text(parts.name).font(SF.mono(12)).lineLimit(1).truncationMode(.middle)
                                    if !parts.dir.isEmpty {
                                        Text(parts.dir).font(SF.mono(10.5)).foregroundStyle(SF.faint).lineLimit(1).truncationMode(.head)
                                    }
                                }
                                Spacer(minLength: 4)
                                if seen {
                                    Image(systemName: "checkmark").font(.system(size: 11, weight: .semibold)).foregroundStyle(SF.okInk).accessibilityLabel("Viewed")
                                } else {
                                    DiffStatView(add: file.additions, del: file.deletions, blocks: false, size: 10.5).opacity(0.8)
                                }
                            }
                            .padding(.horizontal, 8)
                            .padding(.vertical, 5)
                            .hoverBackground(active: active == file.path)
                        }
                        .buttonStyle(.plain)
                        .help(file.path)
                    }
                }
                .padding(6)
            }
            Divider()
            HStack(spacing: 10) {
                Text("\(files.filter { viewed.contains(key($0.path)) }.count) of \(files.count) viewed").monospacedDigit()
                Spacer()
                HStack(spacing: 3) { Kbd(text: "j"); Kbd(text: "k"); Text("files") }
                HStack(spacing: 3) { Kbd(text: "v"); Text("viewed") }
            }
            .font(SF.font(11))
            .foregroundStyle(SF.faint)
            .padding(.horizontal, 12)
            .frame(height: 32)
        }
    }

    // MARK: Diffs

    private var diffs: some View {
        ScrollViewReader { proxy in
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
                            viewed: viewed.contains(key(file.path)),
                            layout: stores.prefs.diffView,
                            hideWhitespace: stores.prefs.diffHideWhitespace,
                            onToggle: { open in
                                if open {
                                    closed.remove(key(file.path))
                                    viewed.remove(key(file.path))
                                } else {
                                    closed.insert(key(file.path))
                                }
                            },
                            onViewed: { setViewed(file.path, $0) }
                        )
                        .id(file.path)
                        .onAppear { if active == nil { active = file.path } }
                    }
                    if let set = current?.set, set.fileCount > set.files.count {
                        Text("\((set.fileCount - set.files.count).formatted()) more files not listed.").font(SF.font(12)).foregroundStyle(SF.muted)
                    }
                }
                .padding(14)
            }
            .onChange(of: jump) { _, path in
                guard let path else { return }
                withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(path, anchor: .top) }
                jump = nil
            }
        }
    }
}

private struct FileDiffCard: View {
    var file: DiffFile
    var open: Bool
    var viewed: Bool
    var layout: DiffLayout
    var hideWhitespace: Bool
    var onToggle: (Bool) -> Void
    var onViewed: (Bool) -> Void

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
        if let reason = file.omittedReason {
            Text(reason).font(SF.font(12)).foregroundStyle(SF.muted).frame(maxWidth: .infinity, alignment: .leading).padding(12)
        } else if let patch = file.patch {
            let hunks = DiffParser.parse(patch, hideWhitespace: hideWhitespace)
            if DiffParser.changes(hunks) {
                FileHunks(hunks: hunks, language: Highlighter.language(for: file.path), layout: layout)
            } else {
                Text("Only whitespace changed.").font(SF.font(12)).foregroundStyle(SF.muted).frame(maxWidth: .infinity, alignment: .leading).padding(12)
            }
        } else {
            Text("No changes to show.").font(SF.font(12)).foregroundStyle(SF.muted).frame(maxWidth: .infinity, alignment: .leading).padding(12)
        }
    }
}

private struct FileHunks: View {
    var hunks: [DiffHunk]
    var language: Language?
    var layout: DiffLayout

    var body: some View {
        // Rows are lazy, so a long diff only draws what's on screen.
        LazyVStack(alignment: .leading, spacing: 0) {
            ForEach(hunks) { hunk in
                HunkHeader(hunk: hunk)
                switch layout {
                case .unified:
                    ForEach(hunk.lines) { UnifiedLine(line: $0, language: language) }
                case .split:
                    ForEach(DiffParser.splitRows(hunk.lines)) { SplitLine(row: $0, language: language) }
                }
            }
        }
    }
}

private struct HunkHeader: View {
    var hunk: DiffHunk

    var body: some View {
        HStack(spacing: 8) {
            if hunk.gap > 0 {
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 9))
                Text("\(hunk.gap) unchanged line\(hunk.gap == 1 ? "" : "s")").font(SF.font(11.5))
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
private func styledLine(_ line: DiffLine, language: Language?) -> AttributedString {
    var inComment = false
    let tokens = language.map { Highlighter.tokenize(line.text, $0, &inComment) } ?? [Token(text: line.text, kind: .plain)]
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
    var language: Language?

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
            Text(styledLine(line, language: language))
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
    var language: Language?

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
                    Text(styledLine(line, language: language))
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
