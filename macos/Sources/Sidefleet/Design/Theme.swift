import AppKit
import SidefleetKit
import SwiftUI

// Sidefleet's tokens (web/src/routes/layout.css). A warm grey ground, near-black ink, white surfaces.
// Status colours carry meaning only: running = violet, queued = amber, succeeded = green, failed = red.

extension NSColor {
    convenience init(rgb: UInt32) {
        self.init(srgbRed: CGFloat((rgb >> 16) & 0xff) / 255, green: CGFloat((rgb >> 8) & 0xff) / 255, blue: CGFloat(rgb & 0xff) / 255, alpha: 1)
    }
}

extension Color {
    init(rgb: UInt32) { self.init(nsColor: NSColor(rgb: rgb)) }

    /// Follows the window's light or dark appearance by itself.
    static func adaptive(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            NSColor(rgb: appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light)
        })
    }
}

/// `amount` of `a` over `b`, as the page's line tints mix into the card.
private func mix(_ a: UInt32, _ amount: Double, into b: UInt32) -> UInt32 {
    func channel(_ shift: UInt32) -> UInt32 {
        let x = Double((a >> shift) & 0xff), y = Double((b >> shift) & 0xff)
        return UInt32((x * amount + y * (1 - amount)).rounded()) << shift
    }
    return channel(16) | channel(8) | channel(0)
}

enum SF {
    static let background = Color.adaptive(0xf7f7f5, 0x161615)
    static let card = Color.adaptive(0xffffff, 0x1d1d1b)
    static let ink = Color.adaptive(0x191919, 0xececE8)
    static let inkInverse = Color.adaptive(0xffffff, 0x161615)
    static let muted = Color.adaptive(0x6b6a65, 0xa09f99)
    static let faint = Color.adaptive(0x8f8e88, 0x7c7b75)
    static let border = Color.adaptive(0xe6e6e2, 0x2c2c29)
    static let input = Color.adaptive(0xdeded9, 0x34342f)
    static let fill = Color.adaptive(0xefefeb, 0x262624)
    static let hover = Color.adaptive(0xececE8, 0x2a2a27)
    static let selected = Color.adaptive(0xe7e7e2, 0x2a2a27)
    static let code = Color.adaptive(0xf3f3f0, 0x1a1a18)

    static let run = Color.adaptive(0x8b5cf6, 0xa78bfa)
    static let runSoft = Color.adaptive(0xf1eafb, 0x2a2238)
    static let runInk = Color.adaptive(0x5b3aa8, 0xc9b5fb)
    static let queue = Color.adaptive(0xf08a2e, 0xf5a25a)
    static let queueSoft = Color.adaptive(0xfdf1e6, 0x33261a)
    static let queueInk = Color.adaptive(0x94470a, 0xf7c08a)
    static let ok = Color.adaptive(0x13a05a, 0x3ccf86)
    static let okSoft = Color.adaptive(0xe6f6ee, 0x17291f)
    static let okInk = Color.adaptive(0x0b6b3a, 0x7fe0ae)
    static let err = Color.adaptive(0xe5484d, 0xf2555a)
    static let errSoft = Color.adaptive(0xfdecec, 0x331b1c)
    static let errInk = Color.adaptive(0xa32323, 0xf8a0a3)
    static let off = Color.adaptive(0xb4b2a9, 0x5b5a55)
    static let merged = Color.adaptive(0x7c3aed, 0xb794f6)

    // Diffs: green for what was added, red for what was removed. Line tints mix into the card.
    private static let addL: UInt32 = 0x2da44e, addD: UInt32 = 0x3fb950, delL: UInt32 = 0xe5484d, delD: UInt32 = 0xf85149
    private static let cardL: UInt32 = 0xffffff, cardD: UInt32 = 0x1d1d1b
    static let diffAddInk = Color.adaptive(0x1a7f37, 0x6fdd8b)
    static let diffDelInk = Color.adaptive(0xc4262e, 0xff9b95)
    static let diffAddLine = Color.adaptive(mix(addL, 0.09, into: cardL), mix(addD, 0.11, into: cardD))
    static let diffAddNum = Color.adaptive(mix(addL, 0.17, into: cardL), mix(addD, 0.20, into: cardD))
    static let diffAddWord = Color.adaptive(mix(addL, 0.30, into: cardL), mix(addD, 0.34, into: cardD))
    static let diffDelLine = Color.adaptive(mix(delL, 0.08, into: cardL), mix(delD, 0.11, into: cardD))
    static let diffDelNum = Color.adaptive(mix(delL, 0.15, into: cardL), mix(delD, 0.20, into: cardD))
    static let diffDelWord = Color.adaptive(mix(delL, 0.27, into: cardL), mix(delD, 0.34, into: cardD))
    static let diffHunk = Color.adaptive(mix(0x191919, 0.035, into: cardL), mix(0xececE8, 0.04, into: cardD))

    // GitHub's syntax colours, as the page's Shiki themes use them.
    static let synKeyword = Color.adaptive(0xcf222e, 0xff7b72)
    static let synString = Color.adaptive(0x0a3069, 0xa5d6ff)
    static let synComment = Color.adaptive(0x6e7781, 0x8b949e)
    static let synNumber = Color.adaptive(0x0550ae, 0x79c0ff)
    static let synType = Color.adaptive(0x953800, 0xffa657)
    static let synFunction = Color.adaptive(0x8250df, 0xd2a8ff)

    // Column widths, as the page's max-w-3xl and max-w-6xl at its 14 px root.
    static let column: CGFloat = 672
    static let wideColumn: CGFloat = 1008
    static let sidebarWidth: CGFloat = 266

    static func font(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font { .system(size: size, weight: weight) }
    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font { .system(size: size, weight: weight, design: .monospaced) }
}

extension TokenKind {
    var color: Color {
        switch self {
        case .plain: SF.ink
        case .keyword: SF.synKeyword
        case .string: SF.synString
        case .comment: SF.synComment
        case .number: SF.synNumber
        case .type: SF.synType
        case .function: SF.synFunction
        }
    }
}
