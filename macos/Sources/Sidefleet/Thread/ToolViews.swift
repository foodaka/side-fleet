import AppKit
import SidefleetKit
import SwiftUI
import QuickLook

/// A run of consecutive tool calls folded into one line ("Ran 2 commands, read a file"). Opens to the
/// individual rows; images stay visible while folded.
struct ToolGroupView: View {
    var tools: [ToolCall]
    var live: Bool
    @State private var open = false

    private var failed: Int { tools.filter(\.isError).count }
    private var pending: Bool { live && tools.contains { $0.output == nil } }
    private var images: [ImageRef] { tools.flatMap(\.images) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.easeOut(duration: 0.18)) { open.toggle() }
            } label: {
                HStack(spacing: 6) {
                    if pending {
                        ProgressView().controlSize(.mini)
                    }
                    Text(ToolSummary.line(tools)).font(SF.font(13)).lineLimit(1)
                    if failed > 0 { Text("\(failed) failed").font(SF.font(12)).foregroundStyle(SF.errInk) }
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .opacity(0.6)
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .padding(.vertical, 4)
                .contentShape(Rectangle())
            }
            .buttonStyle(ToolTextButtonStyle())
            .accessibilityValue(open ? "Expanded" : "Collapsed")

            if open {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(tools) { ToolRow(item: $0, live: live) }
                }
                .padding(.leading, 12)
                .overlay(alignment: .leading) { Rectangle().fill(SF.border).frame(width: 1) }
                .padding(.top, 2)
                .padding(.bottom, 4)
                .transition(.opacity)
            } else if !images.isEmpty {
                ImageStrip(images: images).padding(.vertical, 4)
            }
        }
    }
}

/// Muted text that darkens on hover.
struct ToolTextButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Face(configuration: configuration)
    }

    private struct Face: View {
        var configuration: Configuration
        @State private var hovering = false

        var body: some View {
            configuration.label
                .foregroundStyle(hovering ? SF.ink : SF.muted)
                .onHover { hovering = $0 }
        }
    }
}

/// One tool call: a single line ("Ran `git status`") that opens to its output. Failures open by themselves.
private struct ToolRow: View {
    var item: ToolCall
    var live: Bool
    @State private var open: Bool?
    @State private var hovering = false

    private var isOpen: Bool { open ?? item.isError }
    private var pending: Bool { item.output == nil && live }

    private var symbol: String {
        switch ToolSummary.icon(item.name) {
        case .terminal: "terminal"
        case .file: "doc.text"
        case .search: "magnifyingglass"
        case .edit: "pencil"
        case .web: "globe"
        case .plan: "checklist"
        case .tool: "wrench"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.easeOut(duration: 0.15)) { open = !isOpen }
            } label: {
                HStack(spacing: 10) {
                    Group {
                        if pending {
                            ProgressView().controlSize(.mini)
                        } else {
                            Image(systemName: symbol).font(.system(size: 10.5))
                        }
                    }
                    .frame(width: 20, height: 20)
                    .background(item.isError ? SF.errSoft : SF.fill, in: RoundedRectangle(cornerRadius: 5))
                    .foregroundStyle(item.isError ? SF.errInk : SF.muted)
                    Text(item.verb).font(SF.font(13)).fixedSize()
                    Text(item.summary.isEmpty ? item.name : item.summary)
                        .font(SF.mono(12))
                        .foregroundStyle(SF.ink.opacity(0.85))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(SF.code, in: RoundedRectangle(cornerRadius: 4))
                    if item.isError { Text("failed").font(SF.font(12)).foregroundStyle(SF.errInk).fixedSize() }
                    Spacer(minLength: 4)
                    if item.output != nil {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 9, weight: .semibold))
                            .rotationEffect(.degrees(isOpen ? 90 : 0))
                            .opacity(isOpen || hovering ? 0.6 : 0)
                    }
                }
                .padding(.vertical, 3)
                .contentShape(Rectangle())
                .onHover { hovering = $0 }
            }
            .buttonStyle(ToolTextButtonStyle())
            .disabled(item.output == nil)

            if isOpen {
                OutputBlock(text: item.output?.isEmpty == false ? item.output! : "(no output)", isError: item.isError, truncated: item.truncated)
                    .padding(.leading, 30)
                    .padding(.top, 4)
                    .padding(.bottom, 8)
                    .transition(.opacity)
            } else if item.isError, !item.firstOutputLine.isEmpty {
                Text(item.firstOutputLine).font(SF.font(12)).foregroundStyle(SF.errInk).lineLimit(1).padding(.leading, 30)
            }
            if !item.images.isEmpty {
                ImageStrip(images: item.images).padding(.leading, 30).padding(.vertical, 4)
            }
        }
    }
}

struct OutputBlock: View {
    var text: String
    var isError = false
    var truncated = false
    var maxHeight: CGFloat = 320

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                Text(text)
                    .font(SF.mono(12))
                    .lineSpacing(3)
                    .foregroundStyle(isError ? SF.errInk : SF.ink.opacity(0.8))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                if truncated { Text("Output cut to 2 KB.").font(SF.font(12)).foregroundStyle(SF.faint) }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
        }
        .frame(maxHeight: maxHeight)
        .fixedSize(horizontal: false, vertical: true)
        .background(isError ? SF.errSoft.opacity(0.5) : SF.code, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(isError ? SF.err.opacity(0.3) : SF.border))
    }
}

// MARK: Images

/// Fetches images from the control plane, which needs auth, and keeps them: they never change.
@MainActor
final class ImageCache {
    static let shared = ImageCache()
    private var images: [String: NSImage] = [:]

    func image(_ id: String) -> NSImage? { images[id] }
    func store(_ id: String, _ image: NSImage) { images[id] = image }
}

struct RemoteImage: View {
    var ref: ImageRef
    @Environment(Stores.self) private var stores
    @State private var image: NSImage?
    @State private var previewing: URL?

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .onTapGesture { preview(image) }
            } else {
                RoundedRectangle(cornerRadius: 8).fill(SF.fill)
                    .frame(width: 180, height: 110)
                    .overlay(ProgressView().controlSize(.small))
            }
        }
        .frame(maxWidth: 420, maxHeight: 300)
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(SF.border))
        .help("Click to look closer")
        .accessibilityLabel("Image")
        .quickLookPreview($previewing)
        .task(id: ref.id) {
            if let cached = ImageCache.shared.image(ref.id) {
                image = cached
            } else if let data = try? await stores.api.image(ref.id), let loaded = NSImage(data: data) {
                ImageCache.shared.store(ref.id, loaded)
                image = loaded
            }
        }
    }

    private func preview(_ image: NSImage) {
        guard let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff), let png = rep.representation(using: .png, properties: [:]) else { return }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("sidefleet-\(ref.id).png")
        try? png.write(to: url)
        previewing = url
    }
}

struct ImageStrip: View {
    var images: [ImageRef]

    var body: some View {
        ScrollView(.horizontal) {
            HStack(alignment: .top, spacing: 8) {
                ForEach(images) { RemoteImage(ref: $0) }
            }
        }
        .scrollIndicators(.never)
    }
}

// MARK: Approvals

/// A tool call the agent is waiting on ("Ask me"): what it wants to do, with Approve and Deny. Once
/// decided it folds to one line, so the thread keeps a record of who allowed what.
struct ApprovalCard: View {
    var item: ApprovalItem
    var agent: AgentID
    var live: Bool
    /// Nil when it's someone else's task: only its owner can answer.
    var onDecide: ((ApprovalItem, Bool) async throws -> Void)?
    @Environment(Stores.self) private var stores
    @State private var busy: Bool?

    var body: some View {
        if item.status == "pending", live {
            pending
        } else {
            decided
        }
    }

    private var pending: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "questionmark.diamond").font(.system(size: 14)).foregroundStyle(SF.queueInk)
                Text("\(Format.agentName(agent)) wants to \(item.ask)").font(SF.font(13.5, .medium))
            }
            let shown = item.main ?? item.detail
            if let shown {
                ScrollView {
                    Text(shown)
                        .font(SF.mono(12.5))
                        .lineSpacing(3)
                        .foregroundStyle(SF.ink.opacity(0.9))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                }
                .frame(maxHeight: 180)
                .fixedSize(horizontal: false, vertical: true)
                .background(SF.code, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            if let note = item.note { Text(note).font(SF.font(12.5)).foregroundStyle(SF.muted) }
            HStack(spacing: 8) {
                if onDecide != nil {
                    Button { decide(true) } label: {
                        Label(busy == true ? "Approving…" : "Approve", systemImage: "checkmark").frame(minWidth: 72)
                    }
                    .buttonStyle(PrimaryButtonStyle(height: 34))
                    .disabled(busy != nil)
                    Button { decide(false) } label: {
                        Label(busy == false ? "Denying…" : "Deny", systemImage: "xmark").frame(minWidth: 72)
                    }
                    .buttonStyle(SecondaryButtonStyle(height: 34))
                    .disabled(busy != nil)
                } else {
                    Text("Only the task's owner can answer: it runs on their machine.").font(SF.font(12.5)).foregroundStyle(SF.muted)
                }
                Spacer()
                LiveText { now in
                    Label(item.timeLeft(now: now), systemImage: "clock")
                        .font(SF.font(11.5))
                        .monospacedDigit()
                        .foregroundStyle(SF.muted)
                        .help("Denied automatically when the time runs out")
                }
            }
            .padding(.top, 4)
        }
        .padding(14)
        .background(SF.queueSoft.opacity(0.4), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(SF.queue.opacity(0.4)))
        .padding(.vertical, 6)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(Format.agentName(agent)) asks to \(item.ask)")
    }

    private var decided: some View {
        let approved = item.status == "approved", denied = item.status == "denied"
        return HStack(spacing: 8) {
            Image(systemName: approved ? "checkmark" : item.status == "pending" ? "questionmark.diamond" : "xmark")
                .font(.system(size: 10.5, weight: .medium))
                .frame(width: 20, height: 20)
                .background(approved ? SF.okSoft : denied ? SF.errSoft : SF.fill, in: RoundedRectangle(cornerRadius: 5))
            Text(item.outcome).font(SF.font(13)).fixedSize()
            Text(item.summary.isEmpty ? item.name : item.summary)
                .font(SF.mono(12))
                .foregroundStyle(SF.ink.opacity(0.85))
                .lineLimit(1)
                .truncationMode(.middle)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(SF.code, in: RoundedRectangle(cornerRadius: 4))
        }
        .foregroundStyle(approved ? SF.okInk : denied ? SF.errInk : SF.muted)
        .padding(.vertical, 4)
    }

    private func decide(_ approve: Bool) {
        guard let onDecide, busy == nil else { return }
        busy = approve
        Task {
            do {
                try await onDecide(item, approve)
            } catch {
                stores.toasts.error(approve ? "Could not approve" : "Could not deny", error)
            }
            busy = nil
        }
    }
}

/// How full the agent's context window is: a ring and a percentage, the numbers on hover.
struct ContextRing: View {
    var fill: ContextFill?
    var agent: AgentID

    private var percent: Int? { fill?.percent }
    private var tone: Color {
        guard let percent else { return SF.faint }
        return percent >= 90 ? SF.err : percent >= 75 ? SF.queue : SF.ink.opacity(0.7)
    }

    private var label: String {
        guard let fill else { return "" }
        return percent.map { "\($0)%" } ?? Format.shortCount(fill.used)
    }

    private var detail: String {
        guard let fill else { return "\(Format.agentName(agent)) didn't report how full its context is. Older machines and some models don't." }
        guard let limit = fill.limit else { return "Context: \(fill.used.formatted()) tokens; \(Format.agentName(agent)) didn't say how big the model's window is." }
        var s = "Context: \(fill.used.formatted()) of \(limit.formatted()) tokens (\(percent ?? 0)%)."
        if (percent ?? 0) >= 75 { s += " When it fills up the agent condenses the conversation and forgets details; fork or start a new task to begin fresh." }
        return s
    }

    var body: some View {
        HStack(spacing: 4) {
            ZStack {
                Circle().stroke(tone.opacity(0.2), lineWidth: 2)
                if let percent {
                    Circle()
                        .trim(from: 0, to: CGFloat(max(percent, 3)) / 100)
                        .stroke(tone, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                        .animation(.easeOut(duration: 0.7), value: percent)
                }
            }
            .frame(width: 13, height: 13)
            if !label.isEmpty { Text(label).font(SF.font(11)).monospacedDigit() }
        }
        .foregroundStyle(tone)
        .padding(.horizontal, 6)
        .frame(height: 28)
        .help(detail)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(detail)
    }
}

/// What a turn changed, at the end of its narration: the files with their stats, each one a way into the diff.
struct ChangesCard: View {
    var set: DiffSet
    var onOpen: (String?) -> Void
    @State private var all = false

    private let shown = 6

    var body: some View {
        let files = all ? set.files : Array(set.files.prefix(shown))
        let hidden = set.files.count - files.count
        VStack(spacing: 0) {
            Button { onOpen(nil) } label: {
                HStack(spacing: 10) {
                    Image(systemName: "doc.badge.gearshape").font(.system(size: 13)).foregroundStyle(SF.muted)
                    Text("\(set.fileCount.formatted()) file\(set.fileCount == 1 ? "" : "s") changed").font(SF.font(13, .medium))
                    DiffStatView(add: set.additions, del: set.deletions)
                    Spacer()
                    HStack(spacing: 2) {
                        Text("Review").font(SF.font(12))
                        Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold))
                    }
                    .foregroundStyle(SF.muted)
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Divider()
            VStack(spacing: 0) {
                ForEach(files) { file in
                    Button { onOpen(file.path) } label: {
                        HStack(spacing: 10) {
                            FileStatusIcon(status: file.status)
                            let parts = Format.splitPath(file.path)
                            (Text(parts.dir).foregroundColor(SF.muted) + Text(parts.name).foregroundColor(SF.ink))
                                .font(SF.mono(12))
                                .lineLimit(1)
                                .truncationMode(.head)
                            Spacer(minLength: 6)
                            DiffStatView(add: file.additions, del: file.deletions, blocks: false, size: 11)
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 5)
                        .hoverBackground(cornerRadius: 0)
                    }
                    .buttonStyle(.plain)
                }
                if hidden > 0 || set.fileCount > set.files.count {
                    Button {
                        if hidden > 0 { all = true } else { onOpen(nil) }
                    } label: {
                        Text(hidden > 0 ? "Show \(hidden) more file\(hidden == 1 ? "" : "s")" : "\((set.fileCount - set.files.count).formatted()) more files not listed")
                            .font(SF.font(12))
                            .foregroundStyle(SF.muted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 5)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.vertical, 4)
            if let upstream = set.caughtUpUpstream, let commits = set.caughtUpCommits {
                Divider()
                Text("Also caught up with \(upstream) (\(commits.formatted()) commit\(commits == 1 ? "" : "s")), not counted above.")
                    .font(SF.font(12))
                    .foregroundStyle(SF.muted)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
            }
        }
        .card()
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .padding(.top, 8)
    }
}
