import Foundation
import Observation

// New project: start an empty git repo, or clone one, on one of your machines, then start a
// task in it. A port of web/src/lib/sf/ui/NewProject.svelte.

/// How a request to make a project is getting on (`GET /v1/new-repos/{id}`).
public struct NewRepoStatus: Sendable, Hashable, Decodable {
    /// `pending`, `running`, `done` or `failed`.
    public var status: String
    /// When done: the repo's key and its folder on the machine.
    public var repo: String?
    public var path: String?
    public var error: String?
    /// Made, but something short of it went wrong (the first commit, say).
    public var warning: String?

    public init(status: String, repo: String? = nil, path: String? = nil, error: String? = nil, warning: String? = nil) {
        self.status = status
        self.repo = repo
        self.path = path
        self.error = error
        self.warning = warning
    }

    public init(from decoder: Decoder) throws {
        let f = try Fields(decoder)
        status = f.str("status")
        repo = f.opt("repo")
        path = f.opt("path")
        error = f.opt("error")
        warning = f.opt("warning")
    }

    public var finished: Bool { status == "done" || status == "failed" }
}

private struct CreatedNewRepo: Decodable {
    var id: String
}

extension API {
    /// Ask machine `worker` to make a project: clone `url` into `name`, or start an empty repo
    /// called `name`. Only the machine's owner may. Answers the request's id.
    public func createNewRepo(worker: String, name: String, url: String?) async throws -> String {
        var body: [String: JSONValue] = ["name": .string(name)]
        if let url { body["url"] = .string(url) }
        return (try await post("/v1/workers/\(Self.pathPart(worker))/new-repos", .object(body)) as CreatedNewRepo).id
    }

    public func newRepoStatus(id: String) async throws -> NewRepoStatus { try await get("/v1/new-repos/\(Self.pathPart(id))") }
}

public enum NewProject {
    public enum Mode: String, Sendable, CaseIterable { case empty, clone }

    /// The folder a name becomes: spaces to dashes, anything else git or the shell would mind dropped.
    public static func folder(_ name: String) -> String {
        let dashed = name.trimmingCharacters(in: .whitespacesAndNewlines).split(whereSeparator: \.isWhitespace).joined(separator: "-")
        let kept = dashed.unicodeScalars.filter { $0.isASCII && (CharacterSet.alphanumerics.contains($0) || "._-".unicodeScalars.contains($0)) }
        let s = String(String.UnicodeScalarView(kept)).drop { $0 == "." }
        return String(s.prefix(100))
    }

    /// A clone's folder: the URL's last part without `.git`.
    public static func folder(fromURL url: String) -> String {
        var u = url.trimmingCharacters(in: .whitespacesAndNewlines)
        while u.hasSuffix("/") { u.removeLast() }
        var last = String(u.split(whereSeparator: { $0 == "/" || $0 == ":" }).last ?? "")
        if last.hasSuffix(".git") { last.removeLast(4) }
        return folder(last)
    }

    /// Where it will go on the machine.
    public static func path(root: String, folder: String) -> String {
        var r = root
        while r.hasSuffix("/") { r.removeLast() }
        return r + "/" + (folder.isEmpty ? "…" : folder)
    }
}

/// The New project sheet's state: what is typed, where it goes, and the request once sent.
@MainActor
@Observable
public final class NewProjectModel {
    public enum Phase: Sendable { case idle, waiting, making }

    private let fleet: FleetStore
    private let defaults: UserDefaults
    private static let machineKey = "sidefleet_new_project_machine"

    public var mode: NewProject.Mode = .empty
    public var name = "" { didSet { if !settingFromURL { nameTyped = !name.isEmpty } } }
    public var url = "" {
        didSet {
            guard !nameTyped else { return }
            settingFromURL = true
            name = NewProject.folder(fromURL: url)
            settingFromURL = false
        }
    }
    /// The name was typed, so a pasted URL no longer fills it in.
    public private(set) var nameTyped = false
    @ObservationIgnored private var settingFromURL = false
    public var machineID: String {
        didSet { defaults.set(machineID, forKey: Self.machineKey) }
    }
    public private(set) var busy = false
    public private(set) var phase: Phase = .idle
    public var error = ""

    public init(fleet: FleetStore, defaults: UserDefaults = .standard) {
        self.fleet = fleet
        self.defaults = defaults
        machineID = defaults.string(forKey: Self.machineKey) ?? ""
    }

    /// Back to empty, for a new opening; keeps the machine.
    public func reset() {
        guard !busy else { return }
        nameTyped = false
        name = ""
        url = ""
        nameTyped = false
        error = ""
    }

    /// Your online machines whose Sidefleet can make projects.
    public var able: [Worker] { fleet.myOnline.filter { $0.capabilities.newReposRoot != nil } }
    /// Your online machines too old to.
    public var tooOld: [Worker] { fleet.myOnline.filter { $0.capabilities.newReposRoot == nil } }
    public var machine: Worker? { able.first { $0.id == machineID } ?? able.first }
    public var target: String { NewProject.folder(name) }
    public var path: String { machine?.capabilities.newReposRoot.map { NewProject.path(root: $0, folder: target) } ?? "" }
    /// The machine already has a folder by that name.
    public var taken: Bool {
        let t = target.lowercased()
        return !t.isEmpty && (machine?.capabilities.repos.contains { $0.name.lowercased() == t } ?? false)
    }
    /// The name had to change to become a folder.
    public var renamed: Bool { !target.isEmpty && target != name.trimmingCharacters(in: .whitespacesAndNewlines) }
    public var canSubmit: Bool {
        machine != nil && !target.isEmpty && !taken && (mode == .empty || !url.trimmingCharacters(in: .whitespaces).isEmpty) && !busy
    }

    /// Send it and wait for the machine. The repo's key when made; nil (and `error` set) when not.
    public func submit(pollEvery: Duration = .milliseconds(500)) async -> NewRepoStatus? {
        guard canSubmit, let on = machine else { return nil }
        busy = true
        error = ""
        phase = .waiting
        defer {
            busy = false
            phase = .idle
        }
        do {
            let id = try await fleet.api.createNewRepo(worker: on.id, name: target, url: mode == .clone ? url.trimmingCharacters(in: .whitespaces) : nil)
            var s = NewRepoStatus(status: "pending")
            // A clone can take minutes; the control plane gives up on a silent machine after 15.
            var tries = 0
            while !s.finished, tries < 1200 {
                tries += 1
                try await Task.sleep(for: tries < 20 ? pollEvery : pollEvery * 3)
                s = try await fleet.api.newRepoStatus(id: id)
                if s.status == "running" { phase = .making }
            }
            guard s.status == "done", s.repo != nil else {
                error = s.error ?? "The machine didn’t say how it went."
                return nil
            }
            await fleet.refresh()
            return s
        } catch {
            self.error = error.localizedDescription
            return nil
        }
    }
}
