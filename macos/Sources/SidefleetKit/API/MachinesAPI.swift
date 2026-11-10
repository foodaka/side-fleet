import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// Cleaning up a machine's worktrees, and feedback to the people who make Sidefleet.
extension API {
    private func cleanupPath(_ worker: String) -> String { "/v1/workers/\(Self.pathPart(worker))/clean-worktrees" }

    /// Ask a machine to list what a cleanup would do, deleting nothing. The answer arrives in `cleanupStatus`.
    public func previewCleanup(worker: String) async throws {
        try await call("POST", cleanupPath(worker), ["mode": "preview"])
    }

    /// Remove these threads' worktrees (root run ids). The machine checks each again first.
    public func removeWorktrees(worker: String, ids: [String]) async throws {
        try await call("POST", cleanupPath(worker), ["mode": "remove", "ids": .array(ids.map { .string($0) })])
    }

    /// The latest preview and cleanup. Only the machine's owner may ask.
    public func cleanupStatus(worker: String) async throws -> WorktreeCleanupStatus { try await get(cleanupPath(worker)) }
}

/// The feedback box. It posts to the Sidefleet site, which forwards it to the maintainer, so it
/// works from any self-hosted control plane. Nothing about the control plane goes with it but its build.
public enum Feedback {
    public static let endpoint = URL(string: "https://www.sidefleet.xyz/api/feedback")!

    public enum Failure: Error, LocalizedError, Equatable {
        case unreachable
        case refused(String)

        public var errorDescription: String? {
            switch self {
            case .unreachable: "Could not reach sidefleet.xyz"
            case .refused(let why): why
            }
        }
    }

    /// The request, on its own so it can be checked: no credential, no cookie.
    public static func request(message: String, email: String, build: String) throws -> URLRequest {
        var r = URLRequest(url: endpoint)
        r.httpMethod = "POST"
        r.httpShouldHandleCookies = false
        r.setValue("application/json", forHTTPHeaderField: "content-type")
        // `company` is the page's trap for bots: people leave it empty.
        let body: JSONValue = [
            "message": .string(message.trimmingCharacters(in: .whitespacesAndNewlines)),
            "email": .string(email.trimmingCharacters(in: .whitespacesAndNewlines)),
            "build": .string(build),
            "company": "",
        ]
        r.httpBody = try JSONEncoder().encode(body)
        return r
    }

    public static func send(message: String, email: String, build: String) async throws {
        let r = try request(message: message, email: email, build: build)
        // Its own session, which knows nothing of the control plane's key or cookie.
        let config = URLSessionConfiguration.ephemeral
        config.httpShouldSetCookies = false
        config.httpCookieAcceptPolicy = .never
        config.timeoutIntervalForRequest = 30
        let session = URLSession(configuration: config)
        defer { session.finishTasksAndInvalidate() }
        let (data, response): (Data, URLResponse?) = try await withCheckedThrowingContinuation { cont in
            session.dataTask(with: r) { data, response, error in
                if error != nil {
                    cont.resume(throwing: Failure.unreachable)
                } else {
                    cont.resume(returning: (data ?? Data(), response))
                }
            }.resume()
        }
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(code) else {
            let why = (try? JSONDecoder().decode(JSONValue.self, from: data))?["error"]?.string
            throw Failure.refused(why ?? "Could not send your feedback")
        }
    }
}
