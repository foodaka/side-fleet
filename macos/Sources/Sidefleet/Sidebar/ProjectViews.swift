import AppKit
import SidefleetKit
import SwiftUI
import UniformTypeIdentifiers

// The list By project: a group per repo with its icon, and the icon picker. Ports of
// web/src/lib/sf/ui/ProjectIcon.svelte, ProjectIconPicker.svelte and RunList.svelte's `project` snippet.

/// A project's icon: the emoji, letter tile or picture the org set, else its default letter tile.
/// The caller resolves which (`projects.icon(repo)`), so it draws the same in a popover or a sheet.
struct ProjectIconView: View {
    var repo: String
    var icon: ProjectIcon
    var size: CGFloat = 18

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: max(3, (size * 0.26).rounded()), style: .continuous)
        ZStack {
            switch icon.kind {
            case .image:
                if let image = ProjectImages.image(icon) {
                    Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fill)
                } else {
                    shape.fill(SF.fill)
                }
            case .emoji:
                shape.fill(SF.selected)
                Text(icon.value).font(.system(size: (size * 0.68).rounded()))
            case .letter:
                shape.fill(Color(rgb: icon.rgb ?? 0x6b6a65))
                Text(ProjectIcon.letter(repo))
                    .font(.system(size: (size * 0.58).rounded(), weight: .semibold))
                    .foregroundStyle(.white)
            }
        }
        .frame(width: size, height: size)
        .clipShape(shape)
        .accessibilityHidden(true)
    }
}

/// Decoded image icons, so a list of rows doesn't decode base64 on every draw. NSCache is thread-safe.
enum ProjectImages {
    private static let cache = NSCache<NSString, NSImage>()

    static func image(_ icon: ProjectIcon) -> NSImage? {
        let key = icon.value as NSString
        if let hit = cache.object(forKey: key) { return hit }
        guard let data = icon.imageData, let image = NSImage(data: data) else { return nil }
        cache.setObject(image, forKey: key)
        return image
    }

    /// Crop a picture square and shrink it to `ProjectIcon.imageSize` as PNG bytes.
    static func shrink(_ url: URL) -> Data? {
        guard let source = NSImage(contentsOf: url), let cg = source.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return nil }
        let side = min(cg.width, cg.height)
        guard side > 0, let square = cg.cropping(to: CGRect(x: (cg.width - side) / 2, y: (cg.height - side) / 2, width: side, height: side)) else { return nil }
        let n = ProjectIcon.imageSize
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
              let ctx = CGContext(data: nil, width: n, height: n, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        ctx.interpolationQuality = .high
        ctx.draw(square, in: CGRect(x: 0, y: 0, width: n, height: n))
        guard let out = ctx.makeImage() else { return nil }
        return NSBitmapImageRep(cgImage: out).representation(using: .png, properties: [:])
    }
}

/// A segmented choice in the sidebar's style: Mine / Team, By time / By project, the picker's kinds.
struct SegmentTabs: View {
    struct Segment: Identifiable {
        var id: String
        var label: String
        var badge = 0
    }

    var segments: [Segment]
    var selected: String
    var onPick: (String) -> Void

    var body: some View {
        HStack(spacing: 2) {
            ForEach(segments) { s in
                Button {
                    onPick(s.id)
                } label: {
                    HStack(spacing: 4) {
                        Text(s.label).lineLimit(1)
                        if s.badge > 0 { Text("\(s.badge)").monospacedDigit().foregroundStyle(SF.faint) }
                    }
                    .font(SF.font(12, selected == s.id ? .medium : .regular))
                    .foregroundStyle(selected == s.id ? SF.ink : SF.muted)
                    .frame(maxWidth: .infinity)
                    .frame(height: 24)
                    .background(selected == s.id ? SF.background : .clear, in: RoundedRectangle(cornerRadius: 5))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selected == s.id ? .isSelected : [])
            }
        }
        .padding(2)
        .background(SF.selected.opacity(0.6), in: RoundedRectangle(cornerRadius: 7))
    }
}

/// A small count in a pill: orange for waiting on you, violet for running.
private struct CountBadge: View {
    var count: Int
    var ink: Color
    var soft: Color

    var body: some View {
        Text("\(count)")
            .font(SF.font(10.5, .semibold))
            .monospacedDigit()
            .foregroundStyle(ink)
            .padding(.horizontal, 4)
            .frame(minWidth: 16, minHeight: 16)
            .background(soft, in: Capsule())
    }
}

/// A project's heading in the list: folds it, and its menu sets the icon, starts a task there or hides it.
struct ProjectGroupHeader: View {
    var group: ProjectGroup
    @Environment(Stores.self) private var stores
    @State private var hovering = false

    private var projects: ProjectsStore { stores.fleet.projects }
    private var repo: String { group.repo }
    private var name: String { ProjectIcon.name(group.repo) }
    private var open: Bool { !projects.folded.contains(repo) }
    /// One of your online machines has the repo, so a new task can start there.
    private var canStart: Bool { stores.newTask.onlineRepos.contains(repo) }
    /// Something in it finished or changed since you looked, or waits on you.
    private var fresh: Bool {
        group.runs.contains { $0.id != stores.openThreadID && (stores.inbox.item(for: $0.id) != nil || stores.seen.isUnread($0)) }
    }

    var body: some View {
        Button {
            withAnimation(.easeOut(duration: 0.15)) { projects.toggleFolded(repo) }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 8.5, weight: .semibold))
                    .foregroundStyle(SF.faint)
                    .rotationEffect(.degrees(open ? 90 : 0))
                    .frame(width: 10)
                ProjectIconView(repo: repo, icon: projects.icon(repo), size: 18)
                Text(name)
                    .font(SF.font(13, .semibold))
                    .foregroundStyle(SF.ink)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                badges
                    // They keep their room while hidden, so the name doesn't shift under the pointer.
                    .opacity(hovering ? 0 : 1)
            }
            .padding(.leading, 6)
            .padding(.trailing, 10)
            .frame(height: 28)
            .hoverBackground()
        }
        .buttonStyle(.plain)
        .help(repo)
        .accessibilityValue(open ? "Unfolded" : "Folded")
        .contextMenu { menu }
        // The same menu, for those who don't think to right-click.
        .overlay(alignment: .trailing) {
            if hovering {
                Menu { menu } label: {
                    Image(systemName: "ellipsis").font(.system(size: 11, weight: .medium)).frame(width: 22, height: 22)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .foregroundStyle(SF.muted)
                .padding(.trailing, 6)
                .accessibilityLabel("Actions for \(name)")
            }
        }
        .onHover { hovering = $0 }
        .padding(.top, 6)
    }

    @ViewBuilder private var badges: some View {
        HStack(spacing: 4) {
            if group.waiting > 0 {
                CountBadge(count: group.waiting, ink: SF.queueInk, soft: SF.queueSoft).help("\(group.waiting) waiting on you")
            }
            if group.running > 0 {
                CountBadge(count: group.running, ink: SF.runInk, soft: SF.runSoft).help("\(group.running) running")
            }
            if !open, group.waiting == 0, group.running == 0 {
                if fresh {
                    Circle().fill(SF.ink).frame(width: 6, height: 6).accessibilityLabel("New")
                } else {
                    Text(Format.ago(group.latest, now: stores.fleet.now))
                        .font(SF.font(11))
                        .monospacedDigit()
                        .foregroundStyle(SF.faint)
                }
            }
        }
    }

    @ViewBuilder private var menu: some View {
        Button("Set Icon…") { projects.picking = repo }
        Button {
            stores.newTask.compose(in: repo)
            stores.route = .newTask
        } label: {
            // A second line of text is a menu item's subtitle.
            Text("New Task in \(name)")
            if !canStart { Text("None of your machines online has it") }
        }
        .disabled(!canStart)
        Divider()
        Button(open ? "Fold" : "Unfold") {
            withAnimation(.easeOut(duration: 0.15)) { projects.toggleFolded(repo) }
        }
        Button("Hide from the List") { projects.hide(repo) }
        Divider()
        Text(repo)
    }
}

extension View {
    /// A row inside a project's group: indented under its heading, with a hairline down the left.
    func inProjectGroup() -> some View {
        padding(.leading, 6)
            .overlay(alignment: .leading) { Rectangle().fill(SF.border).frame(width: 1) }
            .padding(.leading, 13)
    }
}

/// Which repo's icon is being picked, for `.sheet(item:)`.
struct PickingProject: Identifiable {
    var id: String
}

/// "Set Icon…" for a project: an emoji, a coloured letter or a small picture, for everyone in the org.
struct ProjectIconSheet: View {
    var repo: String
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    /// What Save would set; nil is the default.
    @State private var draft: ProjectIcon?
    @State private var tab: ProjectIcon.Kind = .emoji
    @State private var custom = ""

    private var projects: ProjectsStore { stores.fleet.projects }
    private var name: String { ProjectIcon.name(repo) }

    init(repo: String, current: ProjectIcon?) {
        self.repo = repo
        // Start from the current icon each time the picker opens.
        _draft = State(initialValue: current)
        _tab = State(initialValue: current?.kind ?? .emoji)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                ProjectIconView(repo: repo, icon: draft ?? ProjectIcon.defaultIcon(repo), size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(name).font(SF.font(15, .semibold)).lineLimit(1).truncationMode(.tail)
                    Text("Shown next to it in the task list, for everyone in the org")
                        .font(SF.font(12))
                        .foregroundStyle(SF.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            SegmentTabs(
                segments: [.init(id: "emoji", label: "Emoji"), .init(id: "letter", label: "Letter"), .init(id: "image", label: "Image")],
                selected: tab.rawValue
            ) { id in
                if let k = ProjectIcon.Kind(rawValue: id) { tab = k }
            }
            switch tab {
            case .emoji: emojiTab
            case .letter: letterTab
            case .image: imageTab
            }
            HStack(spacing: 8) {
                Button {
                    draft = nil
                } label: {
                    Text("Reset to Default").font(SF.font(12.5)).padding(.horizontal, 8).padding(.vertical, 4)
                }
                .buttonStyle(QuietButtonStyle(cornerRadius: 6))
                .disabled(draft == nil)
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(SecondaryButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button("Save") {
                    let icon = draft
                    Task { await projects.setIcon(repo, icon) }
                    dismiss()
                }
                .buttonStyle(PrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 344)
    }

    private var emojiTab: some View {
        VStack(alignment: .leading, spacing: 10) {
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 2), count: 8), spacing: 2) {
                ForEach(ProjectIcon.emoji, id: \.self) { e in
                    let on = draft == ProjectIcon(kind: .emoji, value: e)
                    Button {
                        draft = ProjectIcon(kind: .emoji, value: e)
                    } label: {
                        Text(e)
                            .font(.system(size: 19))
                            .frame(maxWidth: .infinity)
                            .frame(height: 34)
                            .hoverBackground(active: on)
                            .overlay {
                                if on { RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(SF.run) }
                            }
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Use \(e)")
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            TextField("Or type or paste any emoji", text: $custom)
                .textFieldStyle(.roundedBorder)
                .font(SF.font(13))
                .onChange(of: custom) { _, value in
                    if let e = ProjectIcon.firstEmoji(value) { draft = ProjectIcon(kind: .emoji, value: e) }
                }
        }
    }

    private var letterTab: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("The first letter of \(name) on a colour.").font(SF.font(12.5)).foregroundStyle(SF.muted)
            HStack(spacing: 4) {
                ForEach(ProjectIcon.tileColors, id: \.self) { c in
                    let icon = ProjectIcon(kind: .letter, value: c)
                    let on = draft == icon
                    Button {
                        draft = icon
                    } label: {
                        ProjectIconView(repo: repo, icon: icon, size: 24)
                            .padding(2)
                            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(on ? SF.ink : .clear, lineWidth: 2))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Colour \(c)")
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
        }
    }

    private var imageTab: some View {
        Button(action: choose) {
            VStack(spacing: 8) {
                if let draft, draft.kind == .image {
                    ProjectIconView(repo: repo, icon: draft, size: 48)
                    Text("Choose another").font(SF.font(12.5))
                } else {
                    Image(systemName: "photo.badge.plus").font(.system(size: 18))
                    Text("Choose an image").font(SF.font(12.5))
                    Text("Cropped square and shrunk to \(ProjectIcon.imageSize)px").font(SF.font(11.5)).foregroundStyle(SF.faint)
                }
            }
            .foregroundStyle(SF.muted)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 20)
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(SF.border, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [.png, .jpeg, .webP, .gif, .heic, .tiff]
        panel.prompt = "Choose"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        if let png = ProjectImages.shrink(url), let icon = ProjectIcon.image(png: png) {
            draft = icon
        } else {
            stores.toasts.show("Couldn't read that image", detail: "Try a PNG or JPEG.", isError: true)
        }
    }
}
