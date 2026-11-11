import AppKit
import SidefleetKit
import SwiftUI

/// The text box for tasks and replies. `TextEditor` can't honour the send key around IME composition,
/// or turn a pasted screenshot into an attachment, so this wraps `NSTextView`.
struct MessageEditor: NSViewRepresentable {
    @Binding var text: String
    @Binding var height: CGFloat
    var fontSize: CGFloat
    var minHeight: CGFloat
    var maxHeight: CGFloat
    var inset: NSSize
    var enabled = true
    var enterSends: () -> Bool
    var onSend: () -> Void
    var onPasteImages: ([AttachedImage]) -> Void
    var accessibility: String
    /// Bump to put the cursor in the box.
    var focusTick = 0

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = EditorScrollView()
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay

        let tv = SendingTextView()
        tv.delegate = context.coordinator
        tv.owner = context.coordinator
        tv.isRichText = false
        tv.allowsUndo = true
        tv.drawsBackground = false
        tv.importsGraphics = false
        // Tasks hold code and paths: no curly quotes or smart dashes behind your back.
        tv.isAutomaticQuoteSubstitutionEnabled = false
        tv.isAutomaticDashSubstitutionEnabled = false
        tv.isAutomaticTextReplacementEnabled = false
        tv.isAutomaticSpellingCorrectionEnabled = false
        tv.isContinuousSpellCheckingEnabled = true
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = false
        tv.autoresizingMask = [.width]
        tv.textContainer?.widthTracksTextView = true
        tv.textContainer?.containerSize = NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude)
        tv.minSize = NSSize(width: 0, height: 0)
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.textContainerInset = inset
        tv.font = .systemFont(ofSize: fontSize)
        tv.textColor = NSColor(SF.ink)
        tv.insertionPointColor = NSColor(SF.ink)
        tv.string = text
        tv.setAccessibilityLabel(accessibility)
        // Files dropped on the box attach, rather than pasting their paths.
        tv.unregisterDraggedTypes()
        scroll.documentView = tv
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let tv = scroll.documentView as? SendingTextView else { return }
        context.coordinator.parent = self
        // A text view made by hand starts with no width, and lays out nothing until it has one.
        if tv.frame.width != scroll.contentSize.width { tv.frame.size.width = scroll.contentSize.width }
        if tv.string != text, !tv.hasMarkedText() {
            tv.string = text
            context.coordinator.remeasure(tv)
        }
        if tv.isEditable != enabled { tv.isEditable = enabled }
        // Setting the colour restyles every character and lays the text out again, which can move
        // the scroll position: only do it when it changed (the theme), not on every keystroke.
        let ink = NSColor(SF.ink)
        if tv.textColor != ink { tv.textColor = ink }
        if context.coordinator.lastFocusTick != focusTick {
            context.coordinator.lastFocusTick = focusTick
            DispatchQueue.main.async { tv.window?.makeFirstResponder(tv) }
        }
        DispatchQueue.main.async { context.coordinator.remeasure(tv) }
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: MessageEditor
        var lastFocusTick = 0

        init(_ parent: MessageEditor) { self.parent = parent }

        func textDidChange(_ note: Notification) {
            guard let tv = note.object as? NSTextView else { return }
            parent.text = tv.string
            remeasure(tv)
        }

        func remeasure(_ tv: NSTextView) {
            guard let container = tv.textContainer, let layout = tv.layoutManager else { return }
            layout.ensureLayout(for: container)
            let used = layout.usedRect(for: container).height + tv.textContainerInset.height * 2
            let target = min(parent.maxHeight, max(parent.minHeight, ceil(used)))
            if abs(parent.height - target) > 0.5 { parent.height = target }
        }

        /// Return's meaning depends on the setting. Command-Return always sends. Returns true when handled.
        func handleReturn(_ flags: NSEvent.ModifierFlags) -> Bool {
            let mods = flags.intersection([.command, .shift, .option, .control])
            if mods == .command {
                parent.onSend()
                return true
            }
            if mods.isEmpty, parent.enterSends() {
                parent.onSend()
                return true
            }
            return false
        }

        func handlePaste(_ pb: NSPasteboard) -> Bool {
            let images = ImageImport.fromPasteboard(pb)
            guard !images.isEmpty else { return false }
            parent.onPasteImages(images)
            return true
        }
    }
}

/// The box grows with its text, but SwiftUI hands it the new height a beat after the text view has
/// grown. In between, the text view scrolls the caret's new line into view, and once the box is tall
/// enough that offset is left behind: the first line hides and the text jumps. So after each resize,
/// pin the text to the top while it all fits, and keep the caret in view once it doesn't.
final class EditorScrollView: NSScrollView {
    override func setFrameSize(_ newSize: NSSize) {
        let changed = newSize.height != frame.height
        super.setFrameSize(newSize)
        guard changed, let tv = documentView as? NSTextView else { return }
        if tv.frame.height <= contentSize.height + 0.5 {
            if contentView.bounds.origin != .zero {
                contentView.scroll(to: .zero)
                reflectScrolledClipView(contentView)
            }
        } else {
            tv.scrollRangeToVisible(tv.selectedRange())
        }
    }
}

final class SendingTextView: NSTextView {
    weak var owner: MessageEditor.Coordinator?

    override func keyDown(with event: NSEvent) {
        // 36 is Return, 76 is Enter on the keypad. While composing, Return confirms the composition.
        if event.keyCode == 36 || event.keyCode == 76, !hasMarkedText(), owner?.handleReturn(event.modifierFlags) == true { return }
        super.keyDown(with: event)
    }

    override func paste(_ sender: Any?) {
        if owner?.handlePaste(.general) == true { return }
        pasteAsPlainText(sender)
    }
}

/// A box's placeholder: `NSTextView` has none of its own.
struct EditorPlaceholder: View {
    var text: String
    var fontSize: CGFloat
    var inset: CGSize

    var body: some View {
        Text(text)
            .font(SF.font(fontSize))
            .foregroundStyle(SF.faint)
            .padding(.horizontal, inset.width + 5)
            .padding(.top, inset.height)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}
