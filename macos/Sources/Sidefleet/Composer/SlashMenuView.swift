import SidefleetKit
import SwiftUI

/// The skills and slash commands Claude Code offers on the machine a message goes to, shown while the
/// message is just `/` and a word. The box sends its keys to `SlashMenuModel.handle` first.
struct SlashMenuView: View {
    var menu: SlashMenuModel
    var onPick: (SlashCommand) -> Void

    private let rowHeight: CGFloat = 30

    var body: some View {
        if menu.isOpen {
            content
                .padding(4)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(SF.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(SF.border))
                .shadow(color: .black.opacity(0.16), radius: 12, y: 4)
                .accessibilityElement(children: .contain)
                .accessibilityLabel("Skills and commands")
        }
    }

    @ViewBuilder private var content: some View {
        if menu.commands == nil {
            note(Text("Loading skills…"))
        } else if menu.failed {
            note(Text("Couldn't load this machine's skills."))
        } else if menu.commands?.isEmpty == true {
            note(Text("This machine hasn't listed its skills yet. Update Sidefleet there to see them here."))
        } else if menu.matches.isEmpty {
            note(Text("No skill or command matches ") + Text("/\(menu.query ?? "")").font(SF.mono(13)).foregroundColor(SF.ink) + Text("."))
        } else {
            let matches = menu.matches
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(Array(matches.enumerated()), id: \.element.name) { i, c in
                            row(c, active: i == menu.active).id(i).onHover { if $0 { menu.active = i } }
                        }
                    }
                }
                .frame(height: min(CGFloat(matches.count) * rowHeight, 288))
                .onChange(of: menu.active) { _, i in proxy.scrollTo(i) }
            }
        }
    }

    private func note(_ text: Text) -> some View {
        text.font(SF.font(13)).foregroundStyle(SF.muted).padding(.horizontal, 12).padding(.vertical, 8)
    }

    private func row(_ c: SlashCommand, active: Bool) -> some View {
        Button { onPick(c) } label: {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                (Text("/\(c.name)").foregroundColor(SF.ink) + Text(c.argumentHint.isEmpty ? "" : "  \(c.argumentHint)").foregroundColor(SF.faint))
                    .font(SF.mono(13))
                    .lineLimit(1)
                    .fixedSize()
                Text(c.description)
                    .font(SF.font(12.5))
                    .foregroundStyle(SF.muted)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, 10)
            .frame(height: rowHeight)
            .background(active ? SF.selected : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(c.description)
        .accessibilityLabel("/\(c.name)\(c.description.isEmpty ? "" : ", \(c.description)")")
        .accessibilityAddTraits(active ? .isSelected : [])
    }
}

extension SlashMenuModel {
    /// For a box's `onKey`: what the menu does with the key, putting a picked command in `text`.
    func take(_ key: Key, into text: inout String) -> Bool {
        switch handle(key) {
        case .pass: return false
        case .handled: return true
        case .replace(let t):
            text = t
            return true
        }
    }
}
