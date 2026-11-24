import Foundation

/// A project's (repo's) icon: an emoji, its first letter on a colour, or a small picture. The org's,
/// kept on the control plane, so the page and the app show the same ones. A port of
/// web/src/lib/sf/projects.svelte.ts.
public struct ProjectIcon: Sendable, Hashable, Codable {
    public enum Kind: String, Sendable, Hashable, Codable, CaseIterable { case emoji, letter, image }

    public var kind: Kind
    /// The emoji, the tile's colour (`#rrggbb`), or a `data:` URL.
    public var value: String

    public init(kind: Kind, value: String) {
        self.kind = kind
        self.value = value
    }

    /// The tile colours a letter icon can take; a repo without an icon gets one of them.
    public static let tileColors = ["#7c5cff", "#e5484d", "#f08a2e", "#d4a20a", "#22a06b", "#12a5b0", "#3e63dd", "#d6409f", "#6b6a65"]

    /// A choice of emoji for the picker; any other can be pasted.
    public static let emoji = [
        "🪐", "🌙", "🚀", "⚡️", "🔥", "🌱", "🍋", "🍓", "🎯", "🧪", "🛠️", "📦", "🧭", "🗺️", "📱", "⌚️",
        "💳", "🔐", "🧾", "📝", "🎨", "🎧", "📷", "🏔️", "🌊", "🐙", "🦊", "🐝", "🐢", "🦉", "🧠", "✨",
    ]

    /// The icon's square size in pixels once an image is shrunk.
    public static let imageSize = 64
    /// What the control plane keeps at most for an image icon, data: URL and all.
    public static let maxImageBytes = 48 * 1024

    /// The page's string hash, `h = h * 31 + c` in 32 bits. It walks code points and takes each one's
    /// first UTF-16 unit (`for (const c of s) c.charCodeAt(0)`), so outside the BMP only the high surrogate counts.
    static func hash(_ s: String) -> UInt32 {
        var h: UInt32 = 7
        for scalar in s.unicodeScalars {
            let v = scalar.value
            let unit = v > 0xFFFF ? 0xD800 + ((v - 0x10000) >> 10) : v
            h = h &* 31 &+ unit
        }
        return h
    }

    /// The icon a repo shows when nobody set one: its first letter on a colour picked from its name.
    public static func defaultIcon(_ repo: String) -> ProjectIcon {
        ProjectIcon(kind: .letter, value: tileColors[Int(hash(repo) % UInt32(tileColors.count))])
    }

    /// The name shown for a repo: its last path part (`side-fleet` for github.com/acme/side-fleet).
    public static func name(_ repo: String) -> String {
        Format.repoLabel(repo).split(separator: "/").last.map(String.init) ?? repo
    }

    /// The letter a letter tile shows.
    public static func letter(_ repo: String) -> String {
        name(repo).first.map { String($0).uppercased() } ?? "?"
    }

    /// A letter tile's colour as `0xrrggbb`, if it is one.
    public var rgb: UInt32? {
        guard kind == .letter, value.count == 7, value.hasPrefix("#") else { return nil }
        return UInt32(value.dropFirst(), radix: 16)
    }

    /// An image icon's bytes, from its data: URL.
    public var imageData: Data? {
        guard kind == .image, value.hasPrefix("data:image/"), let comma = value.firstIndex(of: ",") else { return nil }
        guard value[..<comma].hasSuffix(";base64") else { return nil }
        return Data(base64Encoded: String(value[value.index(after: comma)...]))
    }

    /// An image icon for PNG bytes, if it is small enough for the control plane to keep.
    public static func image(png: Data) -> ProjectIcon? {
        let value = "data:image/png;base64," + png.base64EncodedString()
        return value.utf8.count <= maxImageBytes ? ProjectIcon(kind: .image, value: value) : nil
    }

    /// The first emoji typed or pasted, if it is one.
    public static func firstEmoji(_ text: String) -> String? {
        guard let first = text.trimmingCharacters(in: .whitespacesAndNewlines).first else { return nil }
        let isEmoji = first.unicodeScalars.contains { s in
            // Plain digits, `#` and `*` count as emoji in Unicode, but not here.
            s.properties.isEmojiPresentation || (s.properties.isEmoji && s.value > 0xFF)
        }
        return isEmoji ? String(first) : nil
    }
}

/// One repo's icon as `GET /v1/project-icons` lists it.
public struct StoredProjectIcon: Sendable, Hashable, Decodable {
    public var repo: String
    /// Nil for a kind this build doesn't know.
    public var icon: ProjectIcon?
    public var updatedAt: Date?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        repo = try f.req("repo")
        let kind: String = f.opt("kind") ?? ""
        let value: String = f.opt("value") ?? ""
        icon = ProjectIcon.Kind(rawValue: kind).map { ProjectIcon(kind: $0, value: value) }
        updatedAt = f.date("updated_at")
    }
}
