import Foundation

// The review panel's file tree and what it remembers: a port of the derived state in
// web/src/lib/sf/diff/DiffViewer.svelte.

/// A row of the tree: a folder (never clickable), or a changed file.
public enum FileTreeRow: Sendable, Hashable, Identifiable {
    case dir(name: String, depth: Int, key: String)
    case file(DiffFile, depth: Int)

    /// Folders end in `/`, so one can't collide with a file's path.
    public var key: String {
        switch self {
        case .dir(_, _, let key): key
        case .file(let file, _): file.path
        }
    }

    public var id: String { key }

    public var depth: Int {
        switch self {
        case .dir(_, let depth, _), .file(_, let depth): depth
        }
    }

    public var file: DiffFile? {
        if case .file(let file, _) = self { return file }
        return nil
    }
}

public enum FileTree {
    private final class Node {
        var dirs: [String: Node] = [:]
        var files: [DiffFile] = []
    }

    private static func ascending(_ a: String, _ b: String) -> Bool { a.localizedCompare(b) == .orderedAscending }

    /// The tree, folders before files at each level, with single-child folders folded into one row (`src/lib/sf`).
    public static func rows(_ files: [DiffFile]) -> [FileTreeRow] {
        let root = Node()
        for f in files {
            var n = root
            for part in f.path.split(separator: "/", omittingEmptySubsequences: false).dropLast() {
                let name = String(part)
                if n.dirs[name] == nil { n.dirs[name] = Node() }
                n = n.dirs[name]!
            }
            n.files.append(f)
        }
        var out: [FileTreeRow] = []
        func walk(_ n: Node, _ depth: Int, _ prefix: String) {
            for name in n.dirs.keys.sorted(by: ascending) {
                var label = name, key = prefix + name + "/"
                var c = n.dirs[name]!
                while c.files.isEmpty, c.dirs.count == 1, let only = c.dirs.first {
                    label += "/" + only.key
                    key += only.key + "/"
                    c = only.value
                }
                out.append(.dir(name: label, depth: depth, key: key))
                walk(c, depth + 1, key)
            }
            for f in n.files.sorted(by: { ascending($0.path, $1.path) }) { out.append(.file(f, depth: depth)) }
        }
        walk(root, 0, "")
        return out
    }

    /// Files in tree order, so the list and the tree read the same way down.
    public static func files(_ rows: [FileTreeRow]) -> [DiffFile] { rows.compactMap(\.file) }

    /// The files whose path holds the query, and the folders above them.
    public static func filter(_ rows: [FileTreeRow], query: String) -> [FileTreeRow] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !q.isEmpty else { return rows }
        let keep = rows.compactMap(\.file).map(\.path).filter { $0.lowercased().contains(q) }
        let kept = Set(keep)
        return rows.filter { row in
            switch row {
            case .file(let file, _): kept.contains(file.path)
            case .dir(_, _, let key): keep.contains { $0.hasPrefix(key) }
            }
        }
    }
}

extension DiffFile {
    /// Past this many changed lines a file waits for "Load diff", like GitHub.
    public static let largeChange = 800

    public var isLarge: Bool { additions + deletions > Self.largeChange }

    /// What an open file with no patch says instead.
    public var noPatchReason: String {
        omittedReason ?? (status == "renamed" ? "File renamed without changes." : "No content changes.")
    }

    /// A new or deleted file has one side only: split would leave half the width empty.
    public var oneSided: Bool { status == "added" || status == "deleted" }
}

extension DiffHunk {
    /// A hunk at the very top with no heading has nothing to say.
    public var hasHeader: Bool { gap > 0 || !section.isEmpty }
}

/// Which files of a review are marked Viewed or folded, by scope ("all", or one message) and path.
/// Kept while the thread is open, so closing the panel and coming back loses nothing.
public struct ReviewState: Sendable, Hashable {
    public private(set) var viewed: Set<String> = []
    public private(set) var closed: Set<String> = []
    /// Scopes that "open with files collapsed" has already folded once.
    private var folded: Set<String> = []

    public init() {}

    private static func key(_ scope: String, _ path: String) -> String { scope + "\u{0}" + path }

    public func isViewed(_ scope: String, _ path: String) -> Bool { viewed.contains(Self.key(scope, path)) }

    public func isOpen(_ scope: String, _ path: String) -> Bool {
        let k = Self.key(scope, path)
        return !closed.contains(k) && !viewed.contains(k)
    }

    /// Opening a file by hand takes its Viewed mark off, as that's what folded it.
    public mutating func setOpen(_ scope: String, _ path: String, _ open: Bool) {
        let k = Self.key(scope, path)
        if open {
            closed.remove(k)
            viewed.remove(k)
        } else {
            closed.insert(k)
        }
    }

    public mutating func setViewed(_ scope: String, _ path: String, _ on: Bool) {
        let k = Self.key(scope, path)
        if on {
            viewed.insert(k)
        } else {
            viewed.remove(k)
            closed.remove(k)
        }
    }

    /// Going to a file opens it, unless you marked it Viewed.
    public mutating func reveal(_ scope: String, _ path: String) {
        if !isOpen(scope, path), !isViewed(scope, path) { setOpen(scope, path, true) }
    }

    public func viewedCount(_ scope: String, _ paths: [String]) -> Int { paths.filter { isViewed(scope, $0) }.count }

    public func allClosed(_ scope: String, _ paths: [String]) -> Bool {
        !paths.isEmpty && paths.allSatisfy { !isOpen(scope, $0) }
    }

    /// Collapse all, or expand all when everything is folded already. Viewed files stay folded.
    public mutating func toggleAll(_ scope: String, _ paths: [String]) {
        let close = !allClosed(scope, paths)
        for p in paths {
            if close { closed.insert(Self.key(scope, p)) } else { closed.remove(Self.key(scope, p)) }
        }
    }

    /// With "open with files collapsed" on, each set of changes opens folded the first time (bar the file asked for).
    public mutating func foldOnce(_ scope: String, _ paths: [String], except focus: String?) {
        guard paths.count >= 2, !folded.contains(scope) else { return }
        folded.insert(scope)
        for p in paths where p != focus { closed.insert(Self.key(scope, p)) }
    }
}
