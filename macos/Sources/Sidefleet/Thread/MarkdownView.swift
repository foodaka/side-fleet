import SidefleetKit
import SwiftUI

/// An agent's answer: real blocks, drawn natively, parsed again only when its text changes.
struct MarkdownView: View {
    var text: String
    var size: CGFloat = 14.5
    @State private var blocks: [MarkdownBlock] = []
    @State private var parsed: String?

    var body: some View {
        BlocksView(blocks: parsed == text ? blocks : MarkdownParser.parse(text), size: size)
            .task(id: text) {
                blocks = MarkdownParser.parse(text)
                parsed = text
            }
    }
}

private struct BlocksView: View {
    var blocks: [MarkdownBlock]
    var size: CGFloat

    var body: some View {
        VStack(alignment: .leading, spacing: size * 0.85) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                BlockView(block: block, size: size)
            }
        }
    }
}

private struct BlockView: View {
    var block: MarkdownBlock
    var size: CGFloat

    var body: some View {
        switch block {
        case .paragraph(let spans):
            InlineText(spans: spans, size: size)
        case .heading(let level, let spans):
            InlineText(spans: spans, size: level == 1 ? size + 5 : level == 2 ? size + 3 : size + 1, weight: .semibold)
                .tracking(-0.15)
                .padding(.top, size * 0.5)
        case .code(let language, let code):
            CodeBlock(language: language, code: code)
        case .quote(let inner):
            HStack(alignment: .top, spacing: 12) {
                RoundedRectangle(cornerRadius: 1).fill(SF.border).frame(width: 3)
                BlocksView(blocks: inner, size: size).foregroundStyle(SF.muted)
            }
        case .list(let ordered, let start, let items):
            VStack(alignment: .leading, spacing: size * 0.25) {
                ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        marker(ordered: ordered, number: start + i, item: item)
                        BlocksView(blocks: item.blocks, size: size)
                    }
                }
            }
        case .table(let header, let rows):
            TableBlock(header: header, rows: rows, size: size)
        case .rule:
            Rectangle().fill(SF.border).frame(height: 1).padding(.vertical, 4)
        }
    }

    @ViewBuilder private func marker(ordered: Bool, number: Int, item: MarkdownListItem) -> some View {
        if let checked = item.checked {
            Image(systemName: checked ? "checkmark.square.fill" : "square").font(.system(size: size - 1)).foregroundStyle(SF.muted)
        } else if ordered {
            Text("\(number).").font(SF.font(size)).monospacedDigit().foregroundStyle(SF.muted)
        } else {
            Text("•").font(SF.font(size)).foregroundStyle(SF.muted)
        }
    }
}

/// A run of text with its inline styles, selectable. Line height about 1.7, as the page sets it.
private struct InlineText: View {
    var spans: [InlineSpan]
    var size: CGFloat
    var weight: Font.Weight = .regular

    private var attributed: AttributedString {
        var out = AttributedString()
        for span in spans {
            var piece = AttributedString(span.text)
            let bold = span.style.contains(.bold)
            var font: Font = span.style.contains(.code)
                ? SF.mono(size * 0.86, bold ? .semibold : .regular)
                : SF.font(size, bold ? .semibold : weight)
            if span.style.contains(.italic) { font = font.italic() }
            piece.font = font
            if span.style.contains(.strike) { piece.strikethroughStyle = Text.LineStyle.single }
            if span.style.contains(.code) { piece.backgroundColor = SF.code }
            if let link = span.link {
                piece.link = link
                piece.underlineStyle = Text.LineStyle.single
                piece.foregroundColor = SF.ink
            }
            out += piece
        }
        return out
    }

    var body: some View {
        Text(attributed)
            .lineSpacing(size * 0.45)
            .tint(SF.ink)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// Code with its colours and a way to copy it.
struct CodeBlock: View {
    var language: String?
    var code: String
    @State private var hovering = false

    private var lines: [String] { code.components(separatedBy: "\n") }

    private var highlighted: AttributedString {
        var out = AttributedString()
        let tokens = Highlighter.highlight(lines: lines, language: Highlighter.language(for: language))
        for (i, line) in tokens.enumerated() {
            for token in line {
                var piece = AttributedString(token.text)
                piece.foregroundColor = token.kind.color
                out += piece
            }
            if i < tokens.count - 1 { out += AttributedString("\n") }
        }
        return out
    }

    var body: some View {
        ZStack(alignment: .topTrailing) {
            ScrollView(.horizontal) {
                Text(highlighted)
                    .font(SF.mono(12))
                    .lineSpacing(5)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: true)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 12)
            }
            .scrollIndicators(.automatic)
            .frame(maxWidth: .infinity, alignment: .leading)
            CopyButton(text: { code })
                .padding(6)
                .opacity(hovering ? 1 : 0)
        }
        .background(SF.code, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(SF.border))
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }
}

private struct TableBlock: View {
    var header: [[InlineSpan]]
    var rows: [[[InlineSpan]]]
    var size: CGFloat

    var body: some View {
        ScrollView(.horizontal) {
            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 8) {
                GridRow {
                    ForEach(Array(header.enumerated()), id: \.offset) { _, cell in
                        InlineText(spans: cell, size: size - 1, weight: .semibold)
                    }
                }
                Divider()
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    GridRow {
                        ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                            InlineText(spans: cell, size: size - 1)
                        }
                    }
                }
            }
            .padding(14)
        }
        .scrollIndicators(.automatic)
        .card(cornerRadius: 10)
    }
}
