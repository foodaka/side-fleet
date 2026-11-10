import AppKit
import SwiftUI

/// `/` jumps to search, as on the page, unless you're typing somewhere. A menu shortcut can't do
/// this: one with no modifier would take the key from every text box too.
struct SlashFocusesSearch: ViewModifier {
    var focus: () -> Void
    @State private var monitor: Any?

    func body(content: Content) -> some View {
        content
            .onAppear {
                guard monitor == nil else { return }
                monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
                    guard event.charactersIgnoringModifiers == "/",
                          event.modifierFlags.intersection([.command, .control, .option]).isEmpty,
                          // Only in the main window (not a task's own window, a sheet or Settings)...
                          let window = event.window, window.identifier?.rawValue.hasPrefix("main") == true,
                          // ...and not while a text box has the keys: the field editor is a text view too.
                          !(window.firstResponder is NSText)
                    else { return event }
                    focus()
                    return nil
                }
            }
            .onDisappear {
                if let monitor { NSEvent.removeMonitor(monitor) }
                monitor = nil
            }
    }
}
