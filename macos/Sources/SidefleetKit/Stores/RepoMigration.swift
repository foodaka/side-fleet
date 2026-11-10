import Foundation

/// Repos chosen before identities existed are stored by folder name; this converts them once.
/// A port of `#migrateAdded` in web/src/lib/sf/state.svelte.ts.
public enum RepoMigration {
    public enum Outcome: Sendable, Equatable {
        /// No machine has reported an identity yet: ask again after the next refresh.
        case notYet
        /// Every choice already names a repo by its identity.
        case unchanged
        case changed([String])
    }

    public static func migrate(added: [String], workers: [Worker]) -> Outcome {
        var byName: [String: String] = [:]
        for w in workers {
            for r in w.capabilities.repos {
                if let id = r.id, !id.isEmpty, id != r.name { byName[r.name] = id }
            }
        }
        guard !byName.isEmpty else { return .notYet }

        // Also repair ids whose host was an SSH alias (`github.com-work/org/repo`): same path, real host.
        func path(_ key: String) -> String { key.split(separator: "/", omittingEmptySubsequences: false).dropFirst().joined(separator: "/") }
        let keys = Set(workers.flatMap { $0.capabilities.repos.map(\.key) })
        var byPath: [String: [String]] = [:]
        for k in keys where !path(k).isEmpty { byPath[path(k), default: []].append(k) }

        func fix(_ n: String) -> String {
            if keys.contains(n) { return n }
            if let id = byName[n] { return id }
            if let same = byPath[path(n)], same.count == 1 { return same[0] }
            return n
        }

        var seen = Set<String>()
        let next = added.map(fix).filter { seen.insert($0).inserted }
        return next == added ? .unchanged : .changed(next)
    }
}
