import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// Rooms, the team and chat apps.
extension API {
    // MARK: Rooms

    public func rooms(archived: Bool = false) async throws -> [Room] { try await get("/v1/rooms?archived=\(archived)") }
    public func room(_ id: String) async throws -> RoomDetail { try await get("/v1/rooms/\(id)") }

    public func createRoom(name: String, goal: String, repo: String) async throws -> Room {
        try await post("/v1/rooms", ["name": .string(name), "goal": .string(goal), "repo": .string(repo)])
    }

    /// Rename it, rewrite its goal, or archive it: whichever fields are given.
    public func updateRoom(_ id: String, _ changes: JSONValue) async throws -> Room {
        try Self.decode(try await backend.send("PATCH", "/v1/rooms/\(id)", body: changes))
    }

    public func startRoomTask(room: String, body: JSONValue) async throws -> String {
        (try await post("/v1/rooms/\(room)/tasks", body) as CreatedRun).id
    }

    /// Merge a task's branch into the room branch, on its owner's machine.
    /// Returns the run that does the merge, for a thread to follow.
    @discardableResult
    public func integrate(run: String) async throws -> String { (try await post("/v1/runs/\(run)/integrate") as CreatedRun).id }

    public func postNote(room: String, body: String) async throws {
        try await call("POST", "/v1/rooms/\(room)/notes", ["body": .string(body)])
    }

    public func editNote(room: String, note: String, body: String) async throws -> RoomNote {
        try Self.decode(try await backend.send("PATCH", "/v1/rooms/\(room)/notes/\(note)", body: ["body": .string(body)]))
    }

    public func deleteNote(room: String, note: String) async throws {
        try await call("DELETE", "/v1/rooms/\(room)/notes/\(note)")
    }

    // MARK: Team

    public func invites(org: String) async throws -> [Invite] { try await get("/v1/orgs/\(org)/invites") }

    public func createInvite(org: String, days: Int, onePerson: Bool) async throws -> CreatedInvite {
        try await post("/v1/orgs/\(org)/invites", ["role": "member", "expires_in_days": .number(Double(days)), "max_uses": onePerson ? 1 : .null])
    }

    public func revokeInvite(org: String, id: String) async throws { try await call("DELETE", "/v1/orgs/\(org)/invites/\(id)") }
    public func removeMember(org: String, user: String) async throws { try await call("DELETE", "/v1/orgs/\(org)/members/\(user)") }

    /// What an invite link says about itself. A link nobody made reads as not found, not as an error.
    public func invite(code: String) async throws -> InviteInfo {
        do {
            return try await get("/v1/invites/\(Self.pathPart(code))")
        } catch APIError.status(404, _) {
            return InviteInfo(valid: false, reason: "not_found")
        }
    }

    public func acceptInvite(code: String) async throws -> Org {
        (try await post("/v1/invites/\(Self.pathPart(code))/accept") as AcceptedInvite).org
    }

    // MARK: Chat apps

    public func apiTokens() async throws -> [ApiToken] { try await get("/v1/api-tokens") }

    public func createApiToken(name: String, scope: String) async throws -> ApiToken {
        try await post("/v1/api-tokens", ["name": .string(name), "scope": .string(scope)])
    }

    public func revokeApiToken(_ id: String) async throws { try await call("DELETE", "/v1/api-tokens/\(id)") }

    /// Turn updates in chat on. The response carries the signing secret, once.
    public func setChatUpdates(token: String, kind: String, url: String, allTasks: Bool) async throws -> ApiToken {
        try Self.decode(try await backend.send("PUT", "/v1/api-tokens/\(token)/updates", body: ["kind": .string(kind), "url": .string(url), "all_tasks": .bool(allTasks)]))
    }

    public func clearChatUpdates(token: String) async throws { try await call("DELETE", "/v1/api-tokens/\(token)/updates") }
    public func testChatUpdates(token: String) async throws { try await call("POST", "/v1/api-tokens/\(token)/updates/test") }
}
