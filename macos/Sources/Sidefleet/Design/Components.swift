import AppKit
import SidefleetKit
import SwiftUI

// MARK: Status

enum DotStatus {
    case run(RunStatus), online, offline, waiting

    var color: Color {
        switch self {
        case .run(let s):
            switch s {
            case .running: SF.run
            case .queued: SF.queue
            case .succeeded: SF.ok
            case .failed, .stopped: SF.err
            case .other: SF.off
            }
        case .online: SF.ok
        case .offline: SF.off
        case .waiting: SF.queue
        }
    }

    var breathes: Bool {
        switch self {
        case .run(.running), .waiting: true
        default: false
        }
    }

    var label: String {
        switch self {
        case .run(let s): s.label
        case .online: "Online"
        case .offline: "Offline"
        case .waiting: "Waiting on you"
        }
    }
}

/// The running dot breathes: the one ambient motion in the app, and it means "live".
struct StatusDot: View {
    var status: DotStatus
    var size: CGFloat = 6
    var dimmed = false
    @State private var breathing = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Circle()
            .fill(status.color)
            .frame(width: size, height: size)
            .opacity(dimmed ? 0.45 : 1)
            .background {
                if status.breathes, !reduceMotion {
                    Circle()
                        .fill(status.color.opacity(breathing ? 0 : 0.55))
                        .frame(width: size, height: size)
                        .scaleEffect(breathing ? 2.4 : 1)
                        .animation(.easeOut(duration: 1.8).repeatForever(autoreverses: false), value: breathing)
                        .onAppear { breathing = true }
                }
            }
            .accessibilityLabel(status.label)
    }
}

struct StatusPill: View {
    var status: RunStatus
    var label: String?
    var extra = ""

    private var tint: (Color, Color) {
        switch status {
        case .running: (SF.runSoft, SF.runInk)
        case .queued: (SF.queueSoft, SF.queueInk)
        case .succeeded: (SF.okSoft, SF.okInk)
        case .failed, .stopped: (SF.errSoft, SF.errInk)
        case .other: (SF.fill, SF.muted)
        }
    }

    var body: some View {
        HStack(spacing: 6) {
            StatusDot(status: .run(status))
            Text(label ?? status.label)
            if !extra.isEmpty { Text(extra).monospacedDigit().opacity(0.75) }
        }
        .font(SF.font(12, .medium))
        .foregroundStyle(tint.1)
        .padding(.horizontal, 10)
        .frame(height: 24)
        .background(tint.0, in: Capsule())
        .fixedSize()
    }
}

/// `+11 −7`, and GitHub's five blocks when there's room.
struct DiffStatView: View {
    var add: Int
    var del: Int
    var blocks = true
    var size: CGFloat = 12

    var body: some View {
        HStack(spacing: 6) {
            HStack(spacing: 4) {
                Text("+\(add.formatted())").foregroundStyle(SF.diffAddInk)
                Text("−\(del.formatted())").foregroundStyle(SF.diffDelInk)
            }
            .font(SF.mono(size))
            if blocks {
                let b = DiffParser.statBlocks(add: add, del: del)
                HStack(spacing: 1.5) {
                    ForEach(0..<5, id: \.self) { i in
                        RoundedRectangle(cornerRadius: 1.5)
                            .fill(i < b.green ? Color(rgb: 0x2da44e) : i < b.green + b.red ? Color(rgb: 0xe5484d) : SF.fill)
                            .frame(width: 7, height: 7)
                    }
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(add) additions, \(del) deletions")
    }
}

/// Added, removed, renamed or changed.
struct FileStatusIcon: View {
    var status: String

    var body: some View {
        let (symbol, color): (String, Color) = switch status {
        case "added": ("plus.square", SF.diffAddInk)
        case "deleted": ("minus.square", SF.diffDelInk)
        case "renamed": ("arrow.right.square", SF.merged)
        default: ("smallcircle.filled.circle", SF.queue)
        }
        Image(systemName: symbol)
            .font(.system(size: 12))
            .foregroundStyle(color)
            .frame(width: 16)
            .accessibilityLabel(status)
    }
}

struct Gauge: View {
    var label: String
    var value: String
    var percent: Int

    private var tone: Color { percent >= 90 ? SF.err : percent >= 75 ? SF.queue : SF.ink.opacity(0.8) }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(label).foregroundStyle(SF.muted)
                Spacer(minLength: 0)
                Text(value).monospacedDigit().foregroundStyle(SF.ink.opacity(0.8))
            }
            .font(SF.font(12))
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(SF.fill)
                    Capsule().fill(tone).frame(width: geo.size.width * CGFloat(max(percent, 2)) / 100)
                        .animation(.easeOut(duration: 0.7), value: percent)
                }
            }
            .frame(height: 6)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(label): \(value)")
    }
}

// MARK: Controls

/// A row or chip that tints under the pointer.
struct HoverBackground: ViewModifier {
    var active = false
    var cornerRadius: CGFloat = 6
    var hoverColor: Color = SF.hover
    var activeColor: Color = SF.selected
    @State private var hovering = false

    func body(content: Content) -> some View {
        content
            .background(active ? activeColor : hovering ? hoverColor : .clear, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .onHover { hovering = $0 }
    }
}

extension View {
    func hoverBackground(active: Bool = false, cornerRadius: CGFloat = 6) -> some View {
        modifier(HoverBackground(active: active, cornerRadius: cornerRadius))
    }

    /// A hairline card on the app's white surface.
    func card(cornerRadius: CGFloat = 14) -> some View {
        background(SF.card, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous).strokeBorder(SF.border))
    }

    /// Keeps a column to its measure, centred in whatever is left.
    func column(_ width: CGFloat = SF.column) -> some View {
        frame(maxWidth: width).frame(maxWidth: .infinity)
    }
}

/// The app's main action: ink on the warm ground.
struct PrimaryButtonStyle: ButtonStyle {
    var height: CGFloat = 32

    func makeBody(configuration: Configuration) -> some View {
        Face(configuration: configuration, height: height)
    }

    private struct Face: View {
        var configuration: Configuration
        var height: CGFloat
        @Environment(\.isEnabled) private var isEnabled

        var body: some View {
            configuration.label
                .font(SF.font(13, .medium))
                .foregroundStyle(SF.inkInverse)
                .padding(.horizontal, 14)
                .frame(height: height)
                .background(SF.ink, in: Capsule())
                .contentShape(Capsule())
                .opacity(isEnabled ? (configuration.isPressed ? 0.8 : 1) : 0.3)
        }
    }
}

struct SecondaryButtonStyle: ButtonStyle {
    var height: CGFloat = 32
    var tint: Color?

    func makeBody(configuration: Configuration) -> some View {
        Face(configuration: configuration, height: height, tint: tint)
    }

    private struct Face: View {
        var configuration: Configuration
        var height: CGFloat
        var tint: Color?
        @Environment(\.isEnabled) private var isEnabled
        @State private var hovering = false

        var body: some View {
            configuration.label
                .font(SF.font(13))
                .foregroundStyle(tint ?? SF.ink)
                .padding(.horizontal, 14)
                .frame(height: height)
                .background(hovering ? SF.hover : .clear, in: Capsule())
                .overlay(Capsule().strokeBorder(tint?.opacity(0.4) ?? SF.input))
                .contentShape(Capsule())
                .opacity(isEnabled ? (configuration.isPressed ? 0.7 : 1) : 0.4)
                .onHover { hovering = $0 }
        }
    }
}

/// A small quiet button: tints on hover, no chrome at rest.
struct QuietButtonStyle: ButtonStyle {
    var cornerRadius: CGFloat = 14

    func makeBody(configuration: Configuration) -> some View {
        Face(configuration: configuration, cornerRadius: cornerRadius)
    }

    private struct Face: View {
        var configuration: Configuration
        var cornerRadius: CGFloat
        @Environment(\.isEnabled) private var isEnabled
        @State private var hovering = false

        var body: some View {
            configuration.label
                .foregroundStyle(hovering && isEnabled ? SF.ink : SF.muted)
                .background(hovering && isEnabled ? SF.hover : .clear, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
                .opacity(isEnabled ? (configuration.isPressed ? 0.7 : 1) : 0.4)
                .onHover { hovering = $0 }
        }
    }
}

/// A round send or stop button.
struct RoundIconButtonStyle: ButtonStyle {
    var size: CGFloat = 32
    var background: Color = SF.ink
    var foreground: Color = SF.inkInverse

    func makeBody(configuration: Configuration) -> some View {
        Face(configuration: configuration, size: size, background: background, foreground: foreground)
    }

    private struct Face: View {
        var configuration: Configuration
        var size: CGFloat
        var background: Color
        var foreground: Color
        @Environment(\.isEnabled) private var isEnabled

        var body: some View {
            configuration.label
                .font(.system(size: size * 0.42, weight: .semibold))
                .foregroundStyle(foreground)
                .frame(width: size, height: size)
                .background(background, in: Circle())
                .contentShape(Circle())
                .opacity(isEnabled ? (configuration.isPressed ? 0.75 : 1) : 0.25)
                .animation(.easeOut(duration: 0.15), value: isEnabled)
        }
    }
}

/// A keycap: `⌘` `↵`.
struct Kbd: View {
    var text: String

    var body: some View {
        Text(text)
            .font(SF.mono(10.5))
            .foregroundStyle(SF.faint)
            .padding(.horizontal, 4)
            .frame(minWidth: 16, minHeight: 16)
            .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(SF.border))
    }
}

/// A square that copies when clicked, and says so.
struct CopyButton: View {
    var text: () -> String
    var label: String?
    var size: CGFloat = 11
    @State private var copied = false

    var body: some View {
        Button {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text(), forType: .string)
            copied = true
            Task {
                try? await Task.sleep(for: .seconds(1.4))
                copied = false
            }
        } label: {
            HStack(spacing: 4) {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    .font(.system(size: size))
                    .foregroundStyle(copied ? SF.okInk : SF.muted)
                    .contentTransition(.symbolEffect(.replace))
                if let label { Text(copied ? "Copied" : label).font(SF.font(12)) }
            }
            .padding(4)
        }
        .buttonStyle(QuietButtonStyle(cornerRadius: 4))
        .help(copied ? "Copied" : "Copy")
        .accessibilityLabel(copied ? "Copied" : (label ?? "Copy"))
    }
}

/// The line that says the agent is working: a light moving over the words.
struct Shimmer: View {
    var text: String
    @State private var phase: CGFloat = -0.4
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Text(text)
            .font(SF.font(13))
            .foregroundStyle(SF.muted)
            .overlay {
                if !reduceMotion {
                    GeometryReader { geo in
                        LinearGradient(colors: [.clear, SF.ink, .clear], startPoint: .leading, endPoint: .trailing)
                            .frame(width: geo.size.width * 0.5)
                            .offset(x: geo.size.width * phase)
                    }
                    .mask(Text(text).font(SF.font(13)))
                    .onAppear {
                        withAnimation(.linear(duration: 2).repeatForever(autoreverses: false)) { phase = 1 }
                    }
                }
            }
    }
}

/// Moving stripes in the running colour: a busy task slot.
struct BusyStripes: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: reduceMotion)) { timeline in
            Canvas { ctx, size in
                ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(SF.run.opacity(0.55)))
                let t = timeline.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 0.9) / 0.9
                let band: CGFloat = 6, period = band * 2
                var x = -size.height - period + CGFloat(reduceMotion ? 0 : t) * period
                while x < size.width {
                    var p = Path()
                    p.move(to: CGPoint(x: x, y: size.height))
                    p.addLine(to: CGPoint(x: x + size.height, y: 0))
                    p.addLine(to: CGPoint(x: x + size.height + band, y: 0))
                    p.addLine(to: CGPoint(x: x + band, y: size.height))
                    p.closeSubpath()
                    ctx.fill(p, with: .color(SF.run))
                    x += period
                }
            }
        }
    }
}

struct SkeletonBar: View {
    var width: CGFloat?
    var height: CGFloat = 12
    @State private var on = false

    var body: some View {
        RoundedRectangle(cornerRadius: 4)
            .fill(SF.fill)
            .frame(width: width, height: height)
            .opacity(on ? 0.5 : 1)
            .animation(.easeInOut(duration: 1).repeatForever(), value: on)
            .onAppear { on = true }
    }
}

/// A teammate's picture at the small sizes rows use. The demo's logins are made up, so it shows initials there.
struct InitialAvatar: View {
    var login: String?
    var size: CGFloat = 16
    @Environment(Stores.self) private var stores: Stores?

    var body: some View {
        Avatar(login: login, size: size, loads: stores.map { !$0.isDemo } ?? false)
    }
}

// MARK: Pickers

/// A choice in a picker popover: a name and a line saying what it means.
struct PickerOption: Identifiable {
    var id: String
    var title: String
    var hint: String?
    var disabledReason: String?
    var mono = false
    var icon: AnyView?
}

/// A quiet pill that opens a list of options. Native menus can't show a second line, and these
/// choices need one: "Full access / Can also run commands on that machine".
struct PickerPill<Label: View>: View {
    var options: [PickerOption]
    var selected: String?
    var heading: String?
    var height: CGFloat = 32
    var width: CGFloat = 280
    var accessibility: String
    var onPick: (String) -> Void
    @ViewBuilder var label: () -> Label
    @State private var open = false

    var body: some View {
        Button {
            open.toggle()
        } label: {
            HStack(spacing: 5) {
                label()
                Image(systemName: "chevron.down")
                    .font(.system(size: 8.5, weight: .semibold))
                    .opacity(0.5)
            }
            .padding(.horizontal, 10)
            .frame(height: height)
            .hoverBackground(active: open, cornerRadius: height / 2)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(accessibility)
        .popover(isPresented: $open, arrowEdge: .bottom) {
            OptionList(options: options, selected: selected, heading: heading, width: width) { id in
                open = false
                onPick(id)
            }
        }
    }
}

/// The list inside a picker, with arrow keys and Return working as in a menu.
struct OptionList: View {
    var options: [PickerOption]
    var selected: String?
    var heading: String?
    var width: CGFloat = 280
    var onPick: (String) -> Void
    @State private var active: String?
    @FocusState private var focused: Bool

    private var enabled: [PickerOption] { options.filter { $0.disabledReason == nil } }

    private func move(_ by: Int) {
        guard !enabled.isEmpty else { return }
        let i = enabled.firstIndex { $0.id == active } ?? (by > 0 ? -1 : enabled.count)
        active = enabled[(i + by + enabled.count) % enabled.count].id
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 1) {
                    if let heading {
                        Text(heading)
                            .font(SF.font(11, .medium))
                            .foregroundStyle(SF.faint)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                    }
                    ForEach(options) { o in
                        Button {
                            if o.disabledReason == nil { onPick(o.id) }
                        } label: {
                            HStack(alignment: .top, spacing: 8) {
                                if let icon = o.icon { icon.padding(.top, 1.5) }
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(o.title).font(o.mono ? SF.mono(12.5) : SF.font(13))
                                    if let hint = o.disabledReason ?? o.hint {
                                        Text(hint).font(SF.font(12)).foregroundStyle(SF.muted).fixedSize(horizontal: false, vertical: true)
                                    }
                                }
                                Spacer(minLength: 4)
                                if o.id == selected {
                                    Image(systemName: "checkmark").font(.system(size: 11, weight: .semibold)).padding(.top, 2)
                                }
                            }
                            .padding(.horizontal, 8)
                            .padding(.vertical, 5)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .hoverBackground(active: active == o.id)
                            .opacity(o.disabledReason == nil ? 1 : 0.45)
                        }
                        .buttonStyle(.plain)
                        .id(o.id)
                    }
                }
                .padding(5)
            }
            .frame(width: width)
            .frame(maxHeight: 340)
            .fixedSize(horizontal: false, vertical: true)
            .focusable()
            .focusEffectDisabled()
            .focused($focused)
            .onKeyPress(.downArrow) {
                move(1)
                return .handled
            }
            .onKeyPress(.upArrow) {
                move(-1)
                return .handled
            }
            .onKeyPress(.return) {
                if let active { onPick(active) }
                return .handled
            }
            .onChange(of: active) { _, id in
                if let id { proxy.scrollTo(id) }
            }
            .onAppear {
                active = selected
                focused = true
            }
        }
    }
}

// MARK: Toasts

/// Small messages at the bottom right, some with an Undo. macOS has nothing of its own for this.
struct ToastOverlay: View {
    var center: ToastCenter

    var body: some View {
        VStack(alignment: .trailing, spacing: 8) {
            ForEach(center.toasts) { toast in
                HStack(alignment: .top, spacing: 12) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(toast.title).font(SF.font(13, .medium)).foregroundStyle(toast.isError ? SF.errInk : SF.ink)
                        if let detail = toast.detail {
                            Text(detail).font(SF.font(12)).foregroundStyle(SF.muted).lineLimit(3).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    if let title = toast.actionTitle, let action = toast.action {
                        Button(title) {
                            action()
                            center.dismiss(toast.id)
                        }
                        .buttonStyle(SecondaryButtonStyle(height: 24))
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .frame(maxWidth: 360, alignment: .leading)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(SF.border))
                .shadow(color: .black.opacity(0.12), radius: 12, y: 4)
                .transition(.move(edge: .trailing).combined(with: .opacity))
                .onTapGesture { center.dismiss(toast.id) }
                .accessibilityElement(children: .combine)
            }
        }
        .padding(16)
        .animation(.easeOut(duration: 0.2), value: center.toasts.map(\.id))
    }
}

/// Times that keep up with the clock, without redrawing whatever they sit in.
struct LiveText<Content: View>: View {
    var every: TimeInterval = 1
    @ViewBuilder var content: (Date) -> Content

    var body: some View {
        TimelineView(.periodic(from: .now, by: every)) { timeline in
            content(timeline.date)
        }
    }
}
