import SidefleetKit
import SwiftUI

/// The feedback box. It goes to the Sidefleet site, not the control plane, so it works from any
/// self-hosted one; `Feedback.send` uses a session that carries no key or cookie.
struct FeedbackSheet: View {
    @Environment(Stores.self) private var stores
    @Environment(\.dismiss) private var dismiss
    @State private var message = ""
    @State private var email = ""
    @State private var height: CGFloat = 112
    @State private var busy = false

    private var trimmed: String { message.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Send feedback").font(SF.font(15, .semibold))
            Text("A bug, an idea, something that got in the way. It goes straight to the person who makes Sidefleet.")
                .font(SF.font(13))
                .lineSpacing(2)
                .foregroundStyle(SF.muted)
                .fixedSize(horizontal: false, vertical: true)
            ZStack(alignment: .topLeading) {
                if message.isEmpty { EditorPlaceholder(text: "What's on your mind?", fontSize: 13, inset: CGSize(width: 8, height: 8)) }
                MessageEditor(
                    text: $message, height: $height, fontSize: 13, minHeight: 112, maxHeight: 240, inset: NSSize(width: 8, height: 8),
                    // Return adds a line here: feedback is a paragraph, and the button sends it.
                    enterSends: { false },
                    onSend: send,
                    onPasteImages: { _ in }, accessibility: "Feedback"
                )
                .frame(height: height)
            }
            .card(cornerRadius: 8)
            TextField("Email, if you'd like a reply (optional)", text: $email)
                .textFieldStyle(.roundedBorder)
                .accessibilityLabel("Email")
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(SecondaryButtonStyle()).keyboardShortcut(.cancelAction)
                Button(busy ? "Sending…" : "Send feedback", action: send)
                    .buttonStyle(PrimaryButtonStyle())
                    .disabled(busy || trimmed.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 440)
    }

    private func send() {
        guard !busy, !trimmed.isEmpty else { return }
        busy = true
        // The page's fields stop at these lengths, and the site turns away more.
        let text = String(trimmed.prefix(4000)), reply = String(email.prefix(254))
        let build = stores.fleet.setup?.build ?? ""
        Task {
            do {
                try await Feedback.send(message: text, email: reply, build: build)
                dismiss()
                stores.toasts.show("Thanks, your feedback was sent")
            } catch {
                stores.toasts.show(error.localizedDescription, isError: true)
            }
            busy = false
        }
    }
}
