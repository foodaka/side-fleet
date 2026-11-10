import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// The developer API (docs/API.md), typed.
public struct API: Sendable {
    public let backend: Backend

    public init(backend: Backend) { self.backend = backend }

    func get<T: Decodable>(_ path: String) async throws -> T {
        try Self.decode(try await backend.send("GET", path, body: nil))
    }

    func post<T: Decodable>(_ path: String, _ body: JSONValue? = nil) async throws -> T {
        try Self.decode(try await backend.send("POST", path, body: body))
    }

    func call(_ method: String, _ path: String, _ body: JSONValue? = nil) async throws {
        _ = try await backend.send(method, path, body: body)
    }

    static func decode<T: Decodable>(_ data: Data) throws -> T {
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(String(describing: error))
        }
    }

    // MARK: Fleet

    public func me() async throws -> Me { try await get("/v1/me") }
    public func workers() async throws -> [Worker] { try await get("/v1/workers") }
    public func runs(limit: Int = 100) async throws -> [RunSummary] { try await get("/v1/runs?limit=\(limit)") }
    public func archivedRuns(limit: Int = 100) async throws -> [RunSummary] { try await get("/v1/runs?archived=true&limit=\(limit)") }
    public func addedRepos() async throws -> [String] { (try await get("/v1/repos") as AddedRepos).added }
    public func enrollments() async throws -> [Enrollment] { try await get("/v1/enrollments") }
    public func myMachines() async throws -> [MyMachine] { try await get("/v1/me/machines") }
    public func setup() async throws -> Setup { try await get("/v1/setup") }
    public func members(org: String) async throws -> [Member] { try await get("/v1/orgs/\(org)/members") }

    public func saveAddedRepos(_ names: [String]) async throws -> [String] {
        let data = try await backend.send("PUT", "/v1/repos", body: ["added": .array(names.map { .string($0) })])
        return (try Self.decode(data) as AddedRepos).added
    }

    // MARK: Threads

    public func thread(_ id: String) async throws -> [Run] { try await get("/v1/runs/\(id)/thread") }
    public func run(_ id: String) async throws -> Run { try await get("/v1/runs/\(id)") }

    public func createRun(_ body: JSONValue) async throws -> String { (try await post("/v1/runs", body) as CreatedRun).id }
    public func sendMessage(root: String, body: JSONValue) async throws -> String { (try await post("/v1/runs/\(root)/messages", body) as CreatedRun).id }
    public func fork(run: String, body: JSONValue) async throws -> String { (try await post("/v1/runs/\(run)/fork", body) as CreatedRun).id }
    public func restore(run: String) async throws -> String { (try await post("/v1/runs/\(run)/restore") as CreatedRun).id }
    public func openPR(root: String) async throws -> String { (try await post("/v1/runs/\(root)/pr") as CreatedRun).id }
    public func stop(run: String) async throws { try await call("POST", "/v1/runs/\(run)/stop") }
    public func startNow(run: String) async throws -> Run { try await post("/v1/runs/\(run)/start-now") }

    public func setArchived(run: String, archived: Bool) async throws {
        try await call("POST", "/v1/runs/\(run)/archive", ["archived": .bool(archived)])
    }

    public func snooze(run: String, until: Date?) async throws {
        try await call("POST", "/v1/runs/\(run)/snooze", ["until": until.map { .string(Timestamp.format($0)) } ?? .null])
    }

    public func decide(run: String, approval: String, approve: Bool) async throws -> ApprovalDecision {
        try await post("/v1/runs/\(run)/approvals/\(approval)/\(approve ? "approve" : "deny")")
    }

    public func image(_ id: String) async throws -> Data { try await backend.send("GET", "/v1/images/\(id)", body: nil) }

    // MARK: Inbox

    public func dismissInbox(_ id: String) async throws { try await call("POST", "/v1/inbox/\(id)/dismiss") }
    public func dismissAllInbox() async throws { try await call("POST", "/v1/inbox/dismiss-all") }

    // MARK: Machines

    public func requestUpdate(worker: String) async throws { try await call("POST", "/v1/workers/\(Self.pathPart(worker))/update") }
    public func removeWorker(_ id: String) async throws { try await call("DELETE", "/v1/workers/\(Self.pathPart(id))") }
    public func lookupEnrollment(code: String) async throws -> Enrollment { try await post("/v1/enrollments/lookup", ["code": .string(code)]) }

    /// Let a machine in, or turn it away. A request found by its code belongs to no org yet, so the
    /// code goes with either answer; `inOrg` approves it into another of your orgs.
    public func decideEnrollment(_ id: String, code: String?, approve: Bool, inOrg org: String? = nil) async throws {
        let body: JSONValue = code.map { ["code": .string($0)] } ?? .object([:])
        _ = try await backend.send("POST", "/v1/enrollments/\(id)/\(approve ? "approve" : "deny")", body: body, inOrg: org)
    }

    public func workerTokens(org: String) async throws -> [WorkerToken] { try await get("/v1/orgs/\(org)/tokens") }
    public func createWorkerToken(org: String, name: String) async throws -> WorkerToken { try await post("/v1/orgs/\(org)/tokens", ["name": .string(name)]) }
    public func revokeWorkerToken(org: String, id: String) async throws { try await call("DELETE", "/v1/orgs/\(org)/tokens/\(id)") }

    // MARK: Orgs and preferences

    public func createOrg(name: String) async throws -> Org { try await post("/v1/orgs", ["name": .string(name)]) }

    public func renameOrg(_ id: String, name: String) async throws {
        try await call("PATCH", "/v1/orgs/\(id)", ["name": .string(name)])
    }

    public func prefs() async throws -> JSONValue { try await get("/v1/me/prefs") }
    public func savePrefs(_ prefs: JSONValue) async throws { try await call("PUT", "/v1/me/prefs", prefs) }
    public func signOut() async throws { try await call("POST", "/auth/logout") }

    /// Worker ids are machine names, which can hold anything.
    static func pathPart(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? s
    }

    // MARK: Streams

    /// Follow a run's events: replays from the start, then live, resuming after a dropped connection.
    /// Returns when the run ends or the task is cancelled.
    public func followRun(_ id: String, retryAfter: Duration = .milliseconds(1500),
                          onEvent: @escaping @MainActor @Sendable (RunEvent) -> Void,
                          onReconnect: @escaping @MainActor @Sendable () -> Void) async {
        // The server replays what comes after this id, so -1 means from the start.
        var last = -1
        while !Task.isCancelled {
            do {
                for try await frame in backend.stream("/v1/runs/\(id)/events", lastEventID: last) {
                    if frame.event == "end" { return }
                    guard !frame.data.isEmpty, let data = frame.data.data(using: .utf8),
                          // Frames are named after their event's kind, so a run's `error` events come as `event: error`
                          // too; the stream's own error frame isn't JSON and is dropped here.
                          let ev = try? JSONDecoder().decode(RunEvent.self, from: data) else { continue }
                    // Only stored events carry an id: `assistant_live` frames must not move the resume point.
                    if let frameID = frame.id, let seq = Int(frameID) { last = seq }
                    await onEvent(ev)
                }
            } catch is CancellationError {
                return
            } catch {
                if Task.isCancelled { return }
                await onReconnect()
            }
            if Task.isCancelled { return }
            try? await Task.sleep(for: retryAfter)
        }
    }

    /// Follow the inbox of whoever is signed in. Stops on sign-out.
    public func followInbox(retryAfter: Duration = .seconds(3),
                            onItems: @escaping @MainActor @Sendable ([InboxItem]) -> Void) async {
        while !Task.isCancelled {
            do {
                for try await frame in backend.stream("/v1/inbox/events", lastEventID: nil) where frame.event == "inbox" {
                    guard let data = frame.data.data(using: .utf8),
                          let payload = try? JSONDecoder().decode(InboxPayload.self, from: data) else { continue }
                    await onItems(payload.items)
                }
            } catch APIError.unauthorized {
                return
            } catch {
                if Task.isCancelled { return }
            }
            if Task.isCancelled { return }
            try? await Task.sleep(for: retryAfter)
        }
    }

    // MARK: Before signing in

    /// Which ways in a control plane offers. Needs no credential.
    public static func authMethods(at baseURL: URL) async throws -> AuthMethods {
        let backend = HTTPBackend(baseURL: baseURL, credential: nil)
        let data = try await backend.send("GET", "/v1/auth", body: nil)
        return try decode(data)
    }
}
