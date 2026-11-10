import Foundation
import Observation

/// The list of rooms in the org. A port of web/src/routes/rooms/+page.svelte's state.
@MainActor
@Observable
public final class RoomsStore {
    /// Nil until the first answer.
    public private(set) var rooms: [Room]?
    public private(set) var error: String?
    public var showArchived = false {
        didSet {
            guard showArchived != oldValue else { return }
            rooms = nil
            Task { await load() }
        }
    }

    @ObservationIgnored private let fleet: FleetStore

    public init(fleet: FleetStore) { self.fleet = fleet }

    public func load() async {
        let archived = showArchived
        do {
            let list = try await fleet.api.rooms(archived: archived)
            guard archived == showArchived else { return }
            rooms = list
            error = nil
        } catch is CancellationError {
            return
        } catch {
            self.error = error.localizedDescription
            if rooms == nil { rooms = [] }
        }
    }

    /// Another org: nothing here belongs to it.
    public func reset() {
        rooms = nil
        error = nil
    }

    /// Repos a new room can be about: your machines' first (where your tasks can run), then the ones
    /// you chose, then the team's.
    public var repoChoices: [String] {
        let mine = Set(fleet.mine.flatMap { $0.capabilities.repos.map(\.key) })
        let all = Set(fleet.allRepos.keys)
        let chosen = fleet.added.map { Set($0) } ?? []
        return all.sorted { a, b in
            if mine.contains(a) != mine.contains(b) { return mine.contains(a) }
            if chosen.contains(a) != chosen.contains(b) { return chosen.contains(a) }
            return a.localizedCompare(b) == .orderedAscending
        }
    }

    public var defaultRepo: String { fleet.composerRepos.first ?? repoChoices.first ?? "" }

    public func create(name: String, goal: String, repo: String) async -> Room? {
        do {
            let room = try await fleet.api.createRoom(name: name.trimmingCharacters(in: .whitespacesAndNewlines), goal: goal.trimmingCharacters(in: .whitespacesAndNewlines), repo: repo)
            await load()
            return room
        } catch {
            fleet.toasts.error("Could not create the room", error)
            return nil
        }
    }
}

/// One room: the goal, the tasks people took and the team's notes. A port of the state in
/// web/src/routes/rooms/[id]/+page.svelte.
@MainActor
@Observable
public final class RoomStore {
    public let id: String
    public private(set) var data: RoomDetail?
    /// Set when the room isn't there for you.
    public private(set) var missing: String?
    /// The task being added to the room or asked to fix itself.
    public private(set) var busyTask: String?
    public private(set) var posting = false

    @ObservationIgnored private let fleet: FleetStore

    public init(id: String, fleet: FleetStore) {
        self.id = id
        self.fleet = fleet
    }

    public var room: Room? { data?.room }
    public var notes: [RoomNote] { data?.notes ?? [] }
    public var tasks: [TaskCard] { data?.sortedTasks ?? [] }

    /// Its creator and org owners may rename, rewrite the goal and archive.
    public var canEdit: Bool {
        guard let room else { return false }
        return room.createdBy == nil || room.createdBy == fleet.userID || fleet.isOwner
    }

    public var compareURL: URL? {
        guard let room, room.integratedAt != nil else { return nil }
        return Rooms.compareURL(room, workers: fleet.workers)
    }

    public func isMine(_ t: TaskCard) -> Bool {
        guard let by = t.createdBy, let me = fleet.userID else { return true }
        return by == me
    }

    public func isMine(_ n: RoomNote) -> Bool { n.authorID != nil && n.authorID == fleet.userID }

    /// Its author, the room's creator or an org owner can take a note back.
    public func canDelete(_ n: RoomNote) -> Bool { n.authorID == nil || n.authorID == fleet.userID || canEdit }

    public func load() async {
        do {
            data = try await fleet.api.room(id)
            missing = nil
        } catch APIError.status(404, _) {
            missing = "This room does not exist, or it belongs to another org."
        } catch {
            // A dropped connection: keep what's on screen, the next poll tries again.
        }
    }

    // MARK: The room

    /// Returns whether it saved.
    public func save(name: String? = nil, goal: String? = nil) async -> Bool {
        var changes: [String: JSONValue] = [:]
        if let name {
            let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !n.isEmpty else { return false }
            changes["name"] = .string(n)
        }
        if let goal { changes["goal"] = .string(goal.trimmingCharacters(in: .whitespacesAndNewlines)) }
        do {
            let room = try await fleet.api.updateRoom(id, .object(changes))
            data?.room = room
            return true
        } catch {
            fleet.toasts.error("Could not save", error)
            return false
        }
    }

    /// Returns whether it changed.
    public func setArchived(_ archived: Bool) async -> Bool {
        do {
            _ = try await fleet.api.updateRoom(id, ["archived": .bool(archived)])
            fleet.toasts.show(archived ? "Room archived" : "Room restored")
            if !archived { await load() }
            return true
        } catch {
            fleet.toasts.error("Could not update the room", error)
            return false
        }
    }

    // MARK: Tasks

    public func integrate(_ t: TaskCard) async {
        busyTask = t.id
        defer { busyTask = nil }
        do {
            try await fleet.api.integrate(run: t.id)
            fleet.toasts.show("Adding to the room", detail: "\(t.workerID ?? "Your machine") merges it into \(room?.branch ?? "the room branch").")
            await load()
        } catch {
            fleet.toasts.error("Could not add it to the room", error)
        }
    }

    public func askToFix(_ t: TaskCard) async {
        guard let room else { return }
        busyTask = t.id
        defer { busyTask = nil }
        do {
            _ = try await fleet.api.sendMessage(root: t.id, body: ["task": .string(Rooms.fixConflictsPrompt(branch: room.branch, files: t.integration?.conflicts ?? []))])
            fleet.toasts.show("Asked the agent to resolve it", detail: "Add it to the room again once it finishes.")
            await load()
        } catch {
            fleet.toasts.error("Could not send", error)
        }
    }

    // MARK: Notes

    /// Returns whether it posted, so the box can clear.
    public func post(note: String) async -> Bool {
        let body = note.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty, !posting else { return false }
        posting = true
        defer { posting = false }
        do {
            try await fleet.api.postNote(room: id, body: body)
            await load()
            return true
        } catch {
            fleet.toasts.error("Could not post the note", error)
            return false
        }
    }

    /// Returns whether editing is over (saved, or nothing changed).
    public func save(note n: RoomNote, body: String) async -> Bool {
        let body = body.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return false }
        if body == n.body { return true }
        do {
            let saved = try await fleet.api.editNote(room: id, note: n.id, body: body)
            if let i = data?.notes.firstIndex(where: { $0.id == n.id }) { data?.notes[i] = saved }
            return true
        } catch {
            fleet.toasts.error("Could not save the note", error)
            return false
        }
    }

    public func delete(note n: RoomNote) async {
        do {
            try await fleet.api.deleteNote(room: id, note: n.id)
            data?.notes.removeAll { $0.id == n.id }
        } catch {
            fleet.toasts.error("Could not delete the note", error)
        }
    }
}

/// The box that starts a task in a room, on one of your own machines.
@MainActor
@Observable
public final class RoomTaskModel {
    public var task = ""
    public private(set) var agent: AgentID = .claude
    public private(set) var model = "sonnet"
    public var effort = ""
    public var perm: String
    public var machine = ""
    public var images: [AttachedImage] = []
    public private(set) var starting = false

    @ObservationIgnored private let fleet: FleetStore
    @ObservationIgnored private let prefs: PrefsStore
    @ObservationIgnored private let defaults: UserDefaults

    public init(fleet: FleetStore, prefs: PrefsStore, defaults: UserDefaults = .standard) {
        self.fleet = fleet
        self.prefs = prefs
        self.defaults = defaults
        perm = prefs.roomPermission
        if let a = defaults.string(forKey: "sidefleet_agent").flatMap(AgentID.init(rawValue:)) { agent = a }
        model = storedModel(agent)
    }

    private func storedModel(_ a: AgentID) -> String {
        let m = defaults.string(forKey: a == .claude ? "sidefleet_model" : "sidefleet_model_\(a.rawValue)") ?? ""
        if a == .claude { return Format.models.first { m == $0.id || m.hasPrefix("claude-\($0.id)-") }?.id ?? Format.defaultModel(a) }
        return Format.modelValid(agent: a, model: m) ? m : Format.defaultModel(a)
    }

    /// Only Claude Code can ask you about tool calls; the others fall back to editing files.
    public var effectivePerm: String { Format.perms(for: agent).contains { $0.id == perm } ? perm : "acceptEdits" }
    public var defaultPerm: String { prefs.roomPermission(for: agent) }

    public func withRepo(_ room: Room) -> [Worker] { fleet.myOnline.filter { Rooms.has($0, repo: room.repo) } }
    public func machines(_ room: Room) -> [Worker] { withRepo(room).filter { $0.hasAgent(agent) } }
    public func pinned(_ room: Room) -> Worker? { machines(room).first { $0.id == machine } }

    public func models(_ room: Room) -> [String] {
        Format.machineModels(pinned(room).map { [$0] } ?? machines(room), agent: agent)
    }

    public func canStart(_ room: Room) -> Bool {
        !task.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !machines(room).isEmpty
            && Format.modelValid(agent: agent, model: model) && !starting && room.archivedAt == nil
    }

    /// Why the start button is off; nil while it's only waiting for a task to be typed.
    public func blocked(_ room: Room) -> String? {
        if fleet.myOnline.isEmpty { return "None of your machines is online" }
        if withRepo(room).isEmpty { return "None of your online machines has \(Format.repoLabel(room.repo)) checked out" }
        if machines(room).isEmpty { return "None of your machines with \(Format.repoLabel(room.repo)) has \(Format.agentName(agent))" }
        return nil
    }

    public func unavailable(_ a: AgentID, in room: Room) -> String? {
        withRepo(room).contains { $0.hasAgent(a) } ? nil : "Not on a machine of yours with \(Format.repoLabel(room.repo))"
    }

    /// Where a task started now would run and from which branch.
    public func hint(_ room: Room) -> String {
        let machines = machines(room)
        if machines.isEmpty { return "None of your machines with \(Format.repoLabel(room.repo)) has \(Format.agentName(agent))." }
        let target = pinned(room)?.displayName ?? (machines.count == 1 ? machines[0].displayName : "whichever of your machines is free first")
        let from = room.integratedAt != nil ? "starting from \(room.branch)" : "starting from the default branch"
        return "Runs on \(target), \(from). The agent gets the goal, the notes and what the other tasks are doing."
    }

    public func pick(agent a: AgentID) {
        agent = a
        model = storedModel(a)
        effort = ""
    }

    public func pick(model m: String) { model = m.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// No machine of yours has this agent for the repo: switch to one that does.
    public func sync(_ room: Room) {
        let with = withRepo(room)
        guard !with.isEmpty, machines(room).isEmpty,
              let other = Format.agents.first(where: { a in with.contains { $0.hasAgent(a.id) } }), other.id != agent else { return }
        pick(agent: other.id)
    }

    public func resetFromPrefs() { perm = prefs.roomPermission }

    /// Start the task. Returns its id, or nil when it couldn't (a toast says why).
    public func start(in room: Room) async -> String? {
        guard canStart(room) else { return nil }
        starting = true
        defer { starting = false }
        var body: [String: JSONValue] = [
            "task": .string(task.trimmingCharacters(in: .whitespacesAndNewlines)),
            "agent": .string(agent.rawValue),
            "model": .string(model),
            "permission_mode": .string(effectivePerm),
        ]
        if !effort.isEmpty { body["effort"] = .string(effort) }
        if let id = pinned(room)?.id { body["worker_id"] = .string(id) }
        if !images.isEmpty { body["images"] = .array(images.map(\.payload)) }
        do {
            let id = try await fleet.api.startRoomTask(room: room.id, body: .object(body))
            task = ""
            images = []
            await fleet.refresh()
            return id
        } catch {
            fleet.toasts.error("Could not start the task", error)
            return nil
        }
    }
}
