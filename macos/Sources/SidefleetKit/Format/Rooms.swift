import Foundation

/// Rooms: a shared goal split into tasks. A port of web/src/lib/sf/rooms.ts.
public enum Rooms {
    private static func isGitHub(_ key: String) -> Bool {
        let parts = key.split(separator: "/", omittingEmptySubsequences: false)
        return parts.count == 3 && parts[0] == "github.com" && !parts[1].isEmpty && !parts[2].isEmpty
    }

    /// `github.com/owner/repo` for the room's repo: its identity, or one a machine reports for that folder.
    public static func githubRepo(_ repo: String, workers: [Worker]) -> String? {
        if isGitHub(repo) { return repo }
        for w in workers {
            for r in w.capabilities.repos where r.name == repo {
                if let id = r.id, isGitHub(id) { return id }
            }
        }
        return nil
    }

    /// GitHub's compare page for the room branch, where a pull request is one click away.
    public static func compareURL(_ room: Room, workers: [Worker]) -> URL? {
        guard let gh = githubRepo(room.repo, workers: workers) else { return nil }
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_.!~*'()"))
        let branch = room.branch.split(separator: "/", omittingEmptySubsequences: false)
            .map { String($0).addingPercentEncoding(withAllowedCharacters: allowed) ?? String($0) }
            .joined(separator: "/")
        return URL(string: "https://\(gh)/compare/\(branch)?expand=1")
    }

    /// The reply that asks a task's agent to take in the room's work and resolve the conflicts.
    public static func fixConflictsPrompt(branch: String, files: [String]) -> String {
        "Adding this task to the room failed: its changes conflict with work already on the room branch"
            + (files.isEmpty ? "" : " (\(files.joined(separator: ", ")))")
            + ". Fetch origin and merge origin/\(branch) into this branch, resolve the conflicts so both behaviours are kept, "
            + "make sure it builds and the tests pass, and commit. Then I'll add it to the room again."
    }

    /// Whether a machine has the room's repo, by identity or by folder name.
    public static func has(_ w: Worker, repo: String) -> Bool {
        w.capabilities.repos.contains { $0.key == repo || $0.name == repo }
    }
}

extension Format {
    /// `git clone https://github.com/acme/web.git` for a repo known by its remote; nothing for a bare folder name.
    public static func repoCloneCommand(_ key: String?) -> String {
        guard let key, !key.isEmpty else { return "" }
        let parts = key.split(separator: "/", omittingEmptySubsequences: false)
        guard parts.count >= 3, parts[0].contains(".") else { return "" }
        return "git clone https://\(key).git"
    }
}
