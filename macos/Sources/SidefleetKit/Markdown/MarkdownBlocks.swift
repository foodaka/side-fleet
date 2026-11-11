import Foundation
import Markdown

// Agent answers as blocks the app can draw. Parsing again as text streams in is cheap, and half a
// document (an open code fence, a table with no rows yet) still parses to something readable.

public struct InlineStyle: OptionSet, Sendable, Hashable {
    public let rawValue: Int
    public init(rawValue: Int) { self.rawValue = rawValue }
    public static let bold = InlineStyle(rawValue: 1)
    public static let italic = InlineStyle(rawValue: 2)
    public static let code = InlineStyle(rawValue: 4)
    public static let strike = InlineStyle(rawValue: 8)
}

public struct InlineSpan: Sendable, Hashable {
    public var text: String
    public var style: InlineStyle
    /// Only addresses safe to open: see `Format.safeLink`.
    public var link: URL?

    public init(text: String, style: InlineStyle = [], link: URL? = nil) {
        self.text = text
        self.style = style
        self.link = link
    }
}

public struct MarkdownListItem: Sendable, Hashable {
    public var blocks: [MarkdownBlock]
    /// A task list's box: nil when the item has none.
    public var checked: Bool?
}

public indirect enum MarkdownBlock: Sendable, Hashable {
    case paragraph([InlineSpan])
    case heading(level: Int, [InlineSpan])
    case code(language: String?, text: String)
    case quote([MarkdownBlock])
    case list(ordered: Bool, start: Int, items: [MarkdownListItem])
    case table(header: [[InlineSpan]], rows: [[[InlineSpan]]])
    case rule
}

public enum MarkdownParser {
    public static func parse(_ source: String) -> [MarkdownBlock] {
        blocks(Document(parsing: source).blockChildren)
    }

    private static func blocks(_ children: some Sequence<any BlockMarkup>) -> [MarkdownBlock] {
        children.compactMap(block)
    }

    private static func block(_ m: any BlockMarkup) -> MarkdownBlock? {
        switch m {
        case let p as Paragraph:
            return .paragraph(inlines(p.inlineChildren, []))
        case let h as Heading:
            return .heading(level: h.level, inlines(h.inlineChildren, []))
        case let c as CodeBlock:
            var text = c.code
            if text.hasSuffix("\n") { text.removeLast() }
            return .code(language: c.language.flatMap { $0.isEmpty ? nil : $0 }, text: text)
        case let q as BlockQuote:
            return .quote(blocks(q.blockChildren))
        case let l as UnorderedList:
            return .list(ordered: false, start: 1, items: l.listItems.map(item))
        case let l as OrderedList:
            return .list(ordered: true, start: Int(l.startIndex), items: l.listItems.map(item))
        case is ThematicBreak:
            return .rule
        case let t as Table:
            let header: [[InlineSpan]] = t.head.cells.map { inlines($0.inlineChildren, []) }
            let rows: [[[InlineSpan]]] = t.body.rows.map { row in row.cells.map { inlines($0.inlineChildren, []) } }
            return .table(header: header, rows: rows)
        case let h as HTMLBlock:
            // Raw HTML shows as written: never rendered.
            var text = h.rawHTML
            if text.hasSuffix("\n") { text.removeLast() }
            return .code(language: "html", text: text)
        default:
            return nil
        }
    }

    private static func item(_ i: ListItem) -> MarkdownListItem {
        let checked: Bool? = switch i.checkbox {
        case .checked: true
        case .unchecked: false
        case nil: nil
        }
        return MarkdownListItem(blocks: blocks(i.blockChildren), checked: checked)
    }

    private static func inlines(_ children: some Sequence<any InlineMarkup>, _ style: InlineStyle, link: URL? = nil) -> [InlineSpan] {
        var out: [InlineSpan] = []
        func add(_ text: String, _ s: InlineStyle, _ l: URL?) {
            if let last = out.last, last.style == s, last.link == l {
                out[out.count - 1].text += text
            } else {
                out.append(InlineSpan(text: text, style: s, link: l))
            }
        }
        for child in children {
            switch child {
            case let t as Text:
                add(t.string, style, link)
            case let c as InlineCode:
                add(c.code, style.union(.code), link)
            case let e as Emphasis:
                for s in inlines(e.inlineChildren, style.union(.italic), link: link) { add(s.text, s.style, s.link) }
            case let s as Strong:
                for span in inlines(s.inlineChildren, style.union(.bold), link: link) { add(span.text, span.style, span.link) }
            case let s as Strikethrough:
                for span in inlines(s.inlineChildren, style.union(.strike), link: link) { add(span.text, span.style, span.link) }
            case let l as Link:
                let url = Format.safeLink(l.destination)
                for span in inlines(l.inlineChildren, style, link: url) { add(span.text, span.style, span.link) }
            case let i as Image:
                // Remote images don't load by themselves, or a repo's content could ping an outside server.
                let alt = i.plainText.isEmpty ? "Image" : i.plainText
                add(alt, style, Format.safeLink(i.source))
            case is SoftBreak:
                add(" ", style, link)
            case is LineBreak:
                add("\n", style, link)
            case let h as InlineHTML:
                add(h.rawHTML, style.union(.code), link)
            default:
                if let plain = (child as? any PlainTextConvertibleMarkup)?.plainText { add(plain, style, link) }
            }
        }
        return out
    }
}
