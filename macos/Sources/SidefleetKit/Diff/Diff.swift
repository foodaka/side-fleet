import Foundation

// Unified diff patches from the worker (`diff` events), parsed into lines with numbers, word-level
// change ranges and side-by-side rows. A port of web/src/lib/sf/diff/parse.ts. Ranges count
// Characters here where the page counts UTF-16 units.

public struct DiffFile: Sendable, Hashable, Decodable, Identifiable {
    public var path: String
    public var oldPath: String?
    public var status: String
    public var additions: Int
    public var deletions: Int
    /// Hunks only (from the first `@@`); nil when the worker left it out.
    public var patch: String?
    public var omitted: String?
    public var id: String { path }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        path = try f.req("path")
        oldPath = f.opt("old_path")
        status = f.opt("status") ?? "modified"
        additions = f.int("additions")
        deletions = f.int("deletions")
        patch = f.opt("patch")
        omitted = f.opt("omitted")
    }

    public var omittedReason: String? {
        switch omitted {
        case nil: nil
        case "binary": "Binary file not shown."
        case "generated": "Generated file. Its diff is hidden."
        case "large": "This diff is too large to show here."
        case "budget": "Not shown: this change is too large to send in full."
        default: "This diff isn't shown."
        }
    }
}

public struct DiffSet: Sendable, Hashable, Decodable {
    public var files: [DiffFile]
    /// Files changed, which can exceed `files.count` on huge changes.
    public var fileCount: Int
    public var additions: Int
    public var deletions: Int
    /// The turn caught up with the main branch; those commits are not counted above.
    public var caughtUpCommits: Int?
    public var caughtUpUpstream: String?

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        files = f.opt("files") ?? []
        fileCount = f.opt("file_count", Double.self).map { Int($0) } ?? files.count
        additions = f.int("additions")
        deletions = f.int("deletions")
        let caught: JSONValue? = f.opt("caught_up")
        caughtUpCommits = caught?["commits"]?.int
        caughtUpUpstream = caught?["upstream"]?.string
    }
}

public enum DiffLineKind: Sendable, Hashable { case context, add, del }

public struct DiffLine: Sendable, Hashable, Identifiable {
    public var kind: DiffLineKind
    public var old: Int?
    public var new: Int?
    public var text: String
    /// Word-level changes against the line it replaced, in Characters.
    public var marks: [Range<Int>] = []
    public var id: String { "\(old ?? -1):\(new ?? -1):\(kind)" }
}

public struct DiffHunk: Sendable, Hashable, Identifiable {
    public var oldStart: Int
    public var newStart: Int
    /// The function or heading git found above the hunk.
    public var section: String
    /// Unchanged lines skipped since the previous hunk (or the top of the file).
    public var gap: Int
    public var lines: [DiffLine]
    public var id: String { "\(oldStart):\(newStart)" }
}

public struct SplitRow: Sendable, Hashable, Identifiable {
    public var left: DiffLine?
    public var right: DiffLine?
    public var id: String { "\(left?.id ?? "-")|\(right?.id ?? "-")" }
}

public enum DiffParser {
    public static func parse(_ patch: String, hideWhitespace: Bool = false) -> [DiffHunk] {
        var hunks: [DiffHunk] = []
        var old = 0, new = 0, prevOldEnd = 1
        var body = patch
        if body.hasSuffix("\n") { body.removeLast() }

        for raw in body.split(separator: "\n", omittingEmptySubsequences: false) {
            if let h = parseHeader(raw) {
                old = h.old
                new = h.new
                // A zero-length side starts *after* the given line number.
                let oldStart = h.oldZero ? old + 1 : old
                hunks.append(DiffHunk(oldStart: oldStart, newStart: h.newZero ? new + 1 : new, section: h.section, gap: max(0, oldStart - prevOldEnd), lines: []))
                if h.oldZero { old += 1 }
                if h.newZero { new += 1 }
                continue
            }
            guard !hunks.isEmpty else { continue }
            let text = String(raw.dropFirst())
            // An empty line is context whose leading space was stripped along the way.
            if raw.isEmpty || raw.first == " " {
                hunks[hunks.count - 1].lines.append(DiffLine(kind: .context, old: old, new: new, text: text))
                old += 1
                new += 1
            } else if raw.first == "-" {
                hunks[hunks.count - 1].lines.append(DiffLine(kind: .del, old: old, new: nil, text: text))
                old += 1
            } else if raw.first == "+" {
                hunks[hunks.count - 1].lines.append(DiffLine(kind: .add, old: nil, new: new, text: text))
                new += 1
            }
            // `\ No newline at end of file` and anything unexpected are skipped.
            prevOldEnd = old
        }
        for i in hunks.indices {
            if hideWhitespace { hunks[i].lines = dropWhitespace(hunks[i].lines) }
            markWords(&hunks[i].lines)
        }
        return hunks
    }

    /// Whether a parsed file still changes anything (it may not once whitespace is hidden).
    public static func changes(_ hunks: [DiffHunk]) -> Bool {
        hunks.contains { $0.lines.contains { $0.kind != .context } }
    }

    private static func parseHeader(_ raw: Substring) -> (old: Int, oldZero: Bool, new: Int, newZero: Bool, section: String)? {
        guard raw.hasPrefix("@@ -"), let close = raw.dropFirst(4).range(of: " @@") else { return nil }
        let ranges = raw[raw.index(raw.startIndex, offsetBy: 4)..<close.lowerBound].split(separator: " ")
        guard ranges.count == 2, ranges[1].first == "+" else { return nil }
        func side(_ s: Substring) -> (Int, Bool)? {
            let parts = s.split(separator: ",", omittingEmptySubsequences: false)
            guard let start = Int(parts[0]) else { return nil }
            if parts.count == 2 {
                guard let count = Int(parts[1]) else { return nil }
                return (start, count == 0)
            }
            return (start, false)
        }
        guard let o = side(ranges[0]), let n = side(ranges[1].dropFirst()) else { return nil }
        let section = raw[close.upperBound...].trimmingCharacters(in: .whitespaces)
        return (o.0, o.1, n.0, n.1, section)
    }

    private static func squash(_ s: String) -> String { s.filter { !$0.isWhitespace } }

    /// Like `git diff -w`: in each run of deletions and the additions after it, lines that match once
    /// all whitespace is ignored become context (numbered on both sides).
    private static func dropWhitespace(_ lines: [DiffLine]) -> [DiffLine] {
        var out: [DiffLine] = []
        var i = 0
        while i < lines.count {
            if lines[i].kind == .context {
                out.append(lines[i])
                i += 1
                continue
            }
            var dels: [DiffLine] = [], adds: [DiffLine] = []
            while i < lines.count, lines[i].kind == .del { dels.append(lines[i]); i += 1 }
            while i < lines.count, lines[i].kind == .add { adds.append(lines[i]); i += 1 }
            let a = dels.map { squash($0.text) }, b = adds.map { squash($0.text) }
            let n = a.count, m = b.count
            if n == 0 || m == 0 || n * m > 250_000 {
                out += dels + adds
                continue
            }
            var dp = [[Int32]](repeating: [Int32](repeating: 0, count: m + 1), count: n + 1)
            for x in stride(from: n - 1, through: 0, by: -1) {
                for y in stride(from: m - 1, through: 0, by: -1) {
                    dp[x][y] = a[x] == b[y] ? dp[x + 1][y + 1] + 1 : max(dp[x + 1][y], dp[x][y + 1])
                }
            }
            // Walk the match, keeping deletions before additions between matched lines.
            var x = 0, y = 0
            var pendingDel: [DiffLine] = [], pendingAdd: [DiffLine] = []
            func flush() {
                out += pendingDel + pendingAdd
                pendingDel = []
                pendingAdd = []
            }
            while x < n || y < m {
                if x < n, y < m, a[x] == b[y] {
                    flush()
                    out.append(DiffLine(kind: .context, old: dels[x].old, new: adds[y].new, text: adds[y].text))
                    x += 1
                    y += 1
                } else if x < n, y == m || dp[x + 1][y] >= dp[x][y + 1] {
                    pendingDel.append(dels[x])
                    x += 1
                } else {
                    pendingAdd.append(adds[y])
                    y += 1
                }
            }
            flush()
        }
        return out
    }

    /// Pair each run of deletions with the additions that follow it and mark the words that differ.
    private static func markWords(_ lines: inout [DiffLine]) {
        var i = 0
        while i < lines.count {
            guard lines[i].kind == .del else { i += 1; continue }
            let ds = i
            while i < lines.count, lines[i].kind == .del { i += 1 }
            let `as` = i
            while i < lines.count, lines[i].kind == .add { i += 1 }
            for k in 0..<min(`as` - ds, i - `as`) {
                if let (ra, rb) = wordDiff(lines[ds + k].text, lines[`as` + k].text) {
                    lines[ds + k].marks = ra
                    lines[`as` + k].marks = rb
                }
            }
        }
    }

    /// Words, runs of whitespace, or single other characters.
    static func tokens(_ s: String) -> [String] {
        var out: [String] = []
        var word = ""
        var spaces = ""
        func isWord(_ c: Character) -> Bool { c.isLetter || c.isNumber || c == "_" }
        for c in s {
            if isWord(c) {
                if !spaces.isEmpty { out.append(spaces); spaces = "" }
                word.append(c)
            } else if c.isWhitespace {
                if !word.isEmpty { out.append(word); word = "" }
                spaces.append(c)
            } else {
                if !word.isEmpty { out.append(word); word = "" }
                if !spaces.isEmpty { out.append(spaces); spaces = "" }
                out.append(String(c))
            }
        }
        if !word.isEmpty { out.append(word) }
        if !spaces.isEmpty { out.append(spaces) }
        return out
    }

    /// Changed ranges in each of two lines, or nil when they share too little to be worth marking.
    public static func wordDiff(_ a: String, _ b: String) -> ([Range<Int>], [Range<Int>])? {
        if a == b { return nil }
        let ta = tokens(a), tb = tokens(b)
        let n = ta.count, m = tb.count
        if n == 0 || m == 0 || n * m > 40_000 { return nil }
        // Longest common subsequence of tokens.
        var dp = [[UInt16]](repeating: [UInt16](repeating: 0, count: m + 1), count: n + 1)
        for x in stride(from: n - 1, through: 0, by: -1) {
            for y in stride(from: m - 1, through: 0, by: -1) {
                dp[x][y] = ta[x] == tb[y] ? dp[x + 1][y + 1] + 1 : max(dp[x + 1][y], dp[x][y + 1])
            }
        }
        var ra: [Range<Int>] = [], rb: [Range<Int>] = []
        var x = 0, y = 0, pa = 0, pb = 0, same = 0
        func push(_ r: inout [Range<Int>], _ s: Int, _ e: Int) {
            if let last = r.last, last.upperBound == s {
                r[r.count - 1] = last.lowerBound..<e
            } else {
                r.append(s..<e)
            }
        }
        while x < n || y < m {
            if x < n, y < m, ta[x] == tb[y] {
                if !ta[x].allSatisfy(\.isWhitespace) { same += ta[x].count }
                pa += ta[x].count
                pb += tb[y].count
                x += 1
                y += 1
            } else if y < m, x == n || dp[x][y + 1] >= dp[x + 1][y] {
                let e = pb + tb[y].count
                push(&rb, pb, e)
                pb = e
                y += 1
            } else {
                let e = pa + ta[x].count
                push(&ra, pa, e)
                pa = e
                x += 1
            }
        }
        // Mostly rewritten lines read better without confetti.
        let visible = max(a.trimmingCharacters(in: .whitespaces).count, b.trimmingCharacters(in: .whitespaces).count)
        if visible == 0 || Double(same) / Double(visible) < 0.4 { return nil }
        return (ra, rb)
    }

    /// Side-by-side rows: context on both sides, deletions beside the additions that replaced them.
    public static func splitRows(_ lines: [DiffLine]) -> [SplitRow] {
        var rows: [SplitRow] = []
        var i = 0
        while i < lines.count {
            if lines[i].kind == .context {
                rows.append(SplitRow(left: lines[i], right: lines[i]))
                i += 1
                continue
            }
            var dels: [DiffLine] = [], adds: [DiffLine] = []
            while i < lines.count, lines[i].kind == .del { dels.append(lines[i]); i += 1 }
            while i < lines.count, lines[i].kind == .add { adds.append(lines[i]); i += 1 }
            for k in 0..<max(dels.count, adds.count) {
                rows.append(SplitRow(left: k < dels.count ? dels[k] : nil, right: k < adds.count ? adds[k] : nil))
            }
        }
        return rows
    }

    /// GitHub's five-block diffstat: how many blocks are green, red and grey.
    public static func statBlocks(add: Int, del: Int) -> (green: Int, red: Int, grey: Int) {
        let total = add + del
        if total == 0 { return (0, 0, 5) }
        let colored = min(5, total)
        var g = Int((Double(colored) * Double(add) / Double(total)).rounded())
        if add > 0, g == 0 { g = 1 }
        if del > 0, g == colored { g = colored - 1 }
        return (g, colored - g, 5 - colored)
    }
}
