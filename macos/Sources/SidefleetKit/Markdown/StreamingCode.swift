import Foundation

/// A code block the agent is still writing. Its text arrives whole every ~100 ms, and highlighting the
/// whole block each time is work that grows with the block, for text nobody can read at that speed.
/// So while the fence is open the block shows only its finished lines (the one being written waits for
/// its newline) and takes on new ones at most every `gap`; once the fence closes, the whole block shows
/// at once. A port of web/src/lib/components/ai-elements/response/streaming-code.svelte.
public enum StreamingCode {
    /// The lines that are finished: everything before the last newline.
    public static func finished(_ text: String) -> String {
        guard let i = text.lastIndex(of: "\n") else { return "" }
        return String(text[..<i])
    }

    /// How long a block waits between updates: 150 ms plus 4 per line, at most half a second.
    public static func gap(_ text: String) -> Double {
        min(0.5, 0.15 + 0.004 * Double(text.split(separator: "\n", omittingEmptySubsequences: false).count))
    }

    public enum Next: Sendable, Equatable {
        /// What's shown is still right.
        case keep
        case show(String)
        /// Show the latest finished lines after this many seconds.
        case wait(Double)
    }

    /// What a block showing `shown` (since `sinceShown` seconds) does with the text it has now.
    public static func next(shown: String, text: String, open: Bool, sinceShown: Double) -> Next {
        let target = open ? finished(text) : text
        if target == shown { return .keep }
        guard open else { return .show(target) }
        let wait = gap(target) - sinceShown
        return wait <= 0 ? .show(target) : .wait(wait)
    }

    /// Whether markdown ends inside a fenced code block that hasn't been closed yet. Fences inside lists
    /// and quotes aren't followed, so those blocks just count as finished.
    public static func endsInOpenFence(_ source: String) -> Bool {
        var fence: (char: Character, count: Int)?
        for line in source.split(separator: "\n", omittingEmptySubsequences: false) {
            let indent = line.prefix { $0 == " " }.count
            guard indent < 4 else { continue }
            let rest = line.dropFirst(indent)
            guard let c = rest.first, c == "`" || c == "~" else { continue }
            let run = rest.prefix { $0 == c }.count
            guard run >= 3 else { continue }
            let after = rest.dropFirst(run)
            if let open = fence {
                if c == open.char, run >= open.count, after.allSatisfy(\.isWhitespace) { fence = nil }
            } else if c == "~" || !after.contains("`") {
                fence = (c, run)
            }
        }
        return fence != nil
    }
}
