import Foundation

// Finding a repo in the picker: a port of the search in web/src/lib/sf/ui/RepoPicker.svelte.

public enum RepoSearch {
    /// What was typed, as words: a repo must hold every one.
    public static func terms(_ query: String) -> [String] {
        query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
    }

    /// Searches the readable name and each machine's folder name for the project.
    public static func matches(label: String, folders: [String], terms: [String]) -> Bool {
        let haystack = (label + " " + folders.joined(separator: " ")).lowercased()
        return terms.allSatisfy { haystack.contains($0) }
    }

    /// The name cut into runs, the ones a term matched marked: each term marks where it first appears.
    public static func parts(_ name: String, terms: [String]) -> [(text: String, marked: Bool)] {
        let chars = Array(name)
        var marks = [Bool](repeating: false, count: chars.count)
        for t in terms {
            guard let r = name.range(of: t, options: .caseInsensitive) else { continue }
            let start = name.distance(from: name.startIndex, to: r.lowerBound)
            let end = name.distance(from: name.startIndex, to: r.upperBound)
            for k in start..<min(end, chars.count) { marks[k] = true }
        }
        var out: [(text: String, marked: Bool)] = []
        for (i, c) in chars.enumerated() {
            if let last = out.last, last.marked == marks[i] {
                out[out.count - 1].text.append(c)
            } else {
                out.append((String(c), marks[i]))
            }
        }
        return out
    }

    /// Chosen repos first, then by name: frozen when the picker opens, so rows don't jump while ticking.
    public static func order(_ repos: [String], chosen: Set<String>) -> [String] {
        repos.sorted { a, b in
            let (x, y) = (chosen.contains(a), chosen.contains(b))
            if x != y { return x }
            return Format.repoLabel(a).localizedCompare(Format.repoLabel(b)) == .orderedAscending
        }
    }
}
