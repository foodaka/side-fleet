import AppKit
import SidefleetKit
import SwiftUI

/// Files the user attached to a message (markdown, text, code, PDFs), as chips that download them.
struct AttachedFiles: View {
    var files: [FileRef]
    /// Under a reply's bubble, they line up on the right with it.
    var trailing = false
    @Environment(Stores.self) private var stores

    var body: some View {
        FlowRow(spacing: 8, lineSpacing: 8, trailing: trailing) {
            ForEach(files) { file in
                Button { download(file) } label: {
                    HStack(spacing: 8) {
                        Image(systemName: "doc.text").font(.system(size: 12)).foregroundStyle(SF.muted)
                        Text(file.name).font(SF.font(12.5)).foregroundStyle(SF.ink).lineLimit(1).truncationMode(.middle)
                        Text(Attachments.size(file.bytes)).font(SF.font(11)).monospacedDigit().foregroundStyle(SF.muted)
                    }
                    .padding(.horizontal, 10)
                    .frame(height: 30)
                    .frame(maxWidth: 256)
                    .background(SF.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(SF.border))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Download \(file.name)")
                .accessibilityLabel("Download \(file.name), \(Attachments.size(file.bytes))")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Attached files")
    }

    /// Always a download: the control plane serves them as plain bytes, never to be opened here.
    private func download(_ file: FileRef) {
        Task {
            do {
                let data = try await stores.api.file(file.id)
                let panel = NSSavePanel()
                panel.nameFieldStringValue = file.name
                panel.canCreateDirectories = true
                guard panel.runModal() == .OK, let url = panel.url else { return }
                try data.write(to: url)
            } catch {
                stores.toasts.error("Could not download \(file.name)", error)
            }
        }
    }
}
