import AppKit
import SidefleetKit
import SwiftUI

extension Path {
    /// A parsed SVG path scaled from its artboard into `size`.
    init(commands: [PathCommand], artboard: CGFloat, size: CGSize) {
        self.init()
        let sx = size.width / artboard, sy = size.height / artboard
        for c in commands {
            switch c {
            case .move(let x, let y):
                move(to: CGPoint(x: x * sx, y: y * sy))
            case .line(let x, let y):
                addLine(to: CGPoint(x: x * sx, y: y * sy))
            case .curve(let x, let y, let c1x, let c1y, let c2x, let c2y):
                addCurve(to: CGPoint(x: x * sx, y: y * sy), control1: CGPoint(x: c1x * sx, y: c1y * sy), control2: CGPoint(x: c2x * sx, y: c2y * sy))
            case .close:
                closeSubpath()
            }
        }
    }
}

private let strandCommands = BrandPaths.strands.map { (commands: SVGPath.parse($0.path), hue: Color(rgb: $0.hue)) }
private let agentCommands: [AgentID: [PathCommand]] = [
    .claude: SVGPath.parse(BrandPaths.claude),
    .codex: SVGPath.parse(BrandPaths.codex),
    .opencode: SVGPath.parse(BrandPaths.opencode),
]

/// The page on the left, fibres fanning out to two machines: a fleet.
struct LogoMark: View {
    var size: CGFloat = 20
    /// Pulses run to each machine in turn. Only while connecting: motion means something is happening.
    var animated = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if animated, !reduceMotion {
            TimelineView(.animation) { timeline in
                canvas(time: timeline.date.timeIntervalSinceReferenceDate)
            }
        } else {
            canvas(time: nil)
        }
    }

    private func canvas(time: TimeInterval?) -> some View {
        Canvas { ctx, canvasSize in
            let width = 1.6 * canvasSize.width / 32
            for (i, strand) in strandCommands.enumerated() {
                let path = Path(commands: strand.commands, artboard: 32, size: canvasSize)
                ctx.stroke(path, with: .color(strand.hue.opacity(time == nil ? 0.9 : 0.42)), style: StrokeStyle(lineWidth: width, lineCap: .round, lineJoin: .round))
                if let time {
                    // Same loop as site/brand/mark-animated.svg: to the top machine, then the bottom one.
                    let phase = (time + (i < 3 ? 0 : 1.6)).truncatingRemainder(dividingBy: 3.2) / 1.6
                    if phase < 1 {
                        let eased = phase * phase * (3 - 2 * phase)
                        let head = path.trimmedPath(from: max(0, eased - 0.22), to: min(1, eased))
                        ctx.stroke(head, with: .color(strand.hue), style: StrokeStyle(lineWidth: width * 1.25, lineCap: .round))
                    }
                }
            }
            let r = BrandPaths.dotRadius * canvasSize.width / 32
            for dot in BrandPaths.dots {
                let c = CGPoint(x: dot.x * canvasSize.width / 32, y: dot.y * canvasSize.height / 32)
                ctx.fill(Path(ellipseIn: CGRect(x: c.x - r, y: c.y - r, width: r * 2, height: r * 2)), with: .color(SF.ink))
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// Claude's mark keeps its orange; the others follow the text.
struct AgentIcon: View {
    var agent: AgentID
    var size: CGFloat = 13

    var body: some View {
        Group {
            if agent == .claude {
                shape.fill(Color(rgb: 0xD97757), style: FillStyle(eoFill: true))
            } else {
                shape.fill(.foreground, style: FillStyle(eoFill: true))
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    private var shape: AgentShape { AgentShape(commands: agentCommands[agent] ?? []) }
}

private struct AgentShape: Shape {
    var commands: [PathCommand]
    func path(in rect: CGRect) -> Path { Path(commands: commands, artboard: 24, size: rect.size) }
}

/// A pull request's state as GitHub draws it: open, merged or closed.
struct PRIcon: View {
    var state: String
    var size: CGFloat = 11

    var body: some View {
        Image(systemName: state == "merged" ? "arrow.triangle.merge" : state == "closed" ? "xmark.circle" : "arrow.triangle.pull")
            .font(.system(size: size, weight: .medium))
            .foregroundStyle(state == "merged" ? SF.merged : state == "closed" ? SF.errInk : SF.okInk)
            .accessibilityLabel("Pull request \(state)")
    }
}

/// The mark for the menu bar: one colour, so macOS can tint it for light, dark and a selected item.
enum MenuBarMark {
    static let image: NSImage = {
        let size = NSSize(width: 18, height: 18)
        let image = NSImage(size: size, flipped: true) { _ in
            let scale = size.width / 32
            NSColor.black.setStroke()
            NSColor.black.setFill()
            for (commands, _) in strandCommands {
                let path = NSBezierPath()
                path.lineWidth = 0.9
                path.lineCapStyle = .round
                path.lineJoinStyle = .round
                for command in commands {
                    switch command {
                    case .move(let x, let y): path.move(to: NSPoint(x: x * scale, y: y * scale))
                    case .line(let x, let y): path.line(to: NSPoint(x: x * scale, y: y * scale))
                    default: break
                    }
                }
                path.stroke()
            }
            let r = BrandPaths.dotRadius * scale + 0.25
            for dot in BrandPaths.dots {
                NSBezierPath(ovalIn: NSRect(x: dot.x * scale - r, y: dot.y * scale - r, width: r * 2, height: r * 2)).fill()
            }
            return true
        }
        image.isTemplate = true
        return image
    }()
}
