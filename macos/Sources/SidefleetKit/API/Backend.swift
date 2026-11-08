import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

public enum APIError: Error, Sendable, Equatable, LocalizedError {
    case unauthorized
    /// The control plane explained, with its HTTP status (402 for plan limits, 403 for roles).
    case status(Int, String)
    case transport(String)
    case decoding(String)

    public var errorDescription: String? {
        switch self {
        case .unauthorized: "Not signed in."
        case .status(_, let message): message
        case .transport(let message): message
        case .decoding(let message): "Unexpected response: \(message)"
        }
    }

    public var httpStatus: Int? {
        if case .status(let code, _) = self { return code }
        return nil
    }
}

/// How a request proves who it's from.
public enum Credential: Sendable, Hashable {
    case devKey(String)
    /// The `sf_session` cookie's value from GitHub sign-in.
    case session(String)
}

/// Talks to a control plane, or to the demo's stand-in for one.
public protocol Backend: Sendable {
    func send(_ method: String, _ path: String, body: JSONValue?) async throws -> Data
    func stream(_ path: String, lastEventID: Int?) -> AsyncThrowingStream<SSEFrame, Error>
    func setOrg(_ id: String)
}

/// The real thing: HTTP to a control plane. Each stream gets its own session, so a window full of
/// live runs can't use up the connections normal requests need.
public final class HTTPBackend: Backend, @unchecked Sendable {
    public let baseURL: URL
    private let credential: Credential?
    private let lock = NSLock()
    private var org = ""
    private let session: URLSession
    private let guardDelegate: RedirectGuard

    public init(baseURL: URL, credential: Credential?, org: String = "") {
        self.baseURL = baseURL
        self.credential = credential
        self.org = org
        let config = URLSessionConfiguration.ephemeral
        config.httpShouldSetCookies = false
        config.httpCookieAcceptPolicy = .never
        config.timeoutIntervalForRequest = 30
        guardDelegate = RedirectGuard(origin: baseURL)
        session = URLSession(configuration: config, delegate: guardDelegate, delegateQueue: nil)
    }

    deinit { session.finishTasksAndInvalidate() }

    public func setOrg(_ id: String) {
        lock.lock()
        org = id
        lock.unlock()
    }

    private func request(_ method: String, _ path: String) throws -> URLRequest {
        guard let url = URL(string: path, relativeTo: baseURL)?.absoluteURL else {
            throw APIError.transport("Bad address: \(path)")
        }
        var r = URLRequest(url: url)
        r.httpMethod = method
        r.httpShouldHandleCookies = false
        switch credential {
        case .devKey(let key):
            r.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        case .session(let cookie):
            r.setValue("sf_session=\(cookie)", forHTTPHeaderField: "Cookie")
            lock.lock()
            let o = org
            lock.unlock()
            if !o.isEmpty { r.setValue(o, forHTTPHeaderField: "X-Sidefleet-Org") }
        case nil:
            break
        }
        return r
    }

    public func send(_ method: String, _ path: String, body: JSONValue?) async throws -> Data {
        var r = try request(method, path)
        if let body {
            r.httpBody = try JSONEncoder().encode(body)
            r.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response): (Data, URLResponse) = try await withCheckedThrowingContinuation { cont in
            session.dataTask(with: r) { data, response, error in
                if let error {
                    cont.resume(throwing: APIError.transport(error.localizedDescription))
                } else if let response {
                    cont.resume(returning: (data ?? Data(), response))
                } else {
                    cont.resume(throwing: APIError.transport("No response"))
                }
            }.resume()
        }
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        if code == 401 { throw APIError.unauthorized }
        guard (200..<300).contains(code) else {
            let message = (try? JSONDecoder().decode(JSONValue.self, from: data))?["error"]?.string
            throw APIError.status(code, message ?? HTTPURLResponse.localizedString(forStatusCode: code))
        }
        return data
    }

    public func stream(_ path: String, lastEventID: Int?) -> AsyncThrowingStream<SSEFrame, Error> {
        AsyncThrowingStream { continuation in
            let request: URLRequest
            do {
                var r = try self.request("GET", path)
                r.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                if let lastEventID { r.setValue(String(lastEventID), forHTTPHeaderField: "Last-Event-ID") }
                r.timeoutInterval = 60 * 60 * 24
                request = r
            } catch {
                continuation.finish(throwing: error)
                return
            }
            let config = URLSessionConfiguration.ephemeral
            config.httpShouldSetCookies = false
            config.httpCookieAcceptPolicy = .never
            // The server sends a keep-alive every 10 to 15 seconds; this is how long silence may last.
            config.timeoutIntervalForRequest = 45
            config.timeoutIntervalForResource = 60 * 60 * 24
            let delegate = StreamDelegate(origin: baseURL, continuation: continuation)
            let session = URLSession(configuration: config, delegate: delegate, delegateQueue: nil)
            let task = session.dataTask(with: request)
            continuation.onTermination = { _ in
                task.cancel()
                session.invalidateAndCancel()
            }
            task.resume()
        }
    }
}

/// Credentials go to the control plane and nowhere else, so a redirect may not leave its origin.
private func sameOrigin(_ a: URL?, _ b: URL?) -> Bool {
    a?.scheme == b?.scheme && a?.host == b?.host && a?.port == b?.port
}

private final class RedirectGuard: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    let origin: URL

    init(origin: URL) { self.origin = origin }

    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
        completionHandler(sameOrigin(origin, request.url) ? request : nil)
    }
}

private final class StreamDelegate: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    let origin: URL
    let continuation: AsyncThrowingStream<SSEFrame, Error>.Continuation
    private var parser = SSEParser()
    private var errorBody = Data()
    private var failure: (code: Int, message: String?)?

    init(origin: URL, continuation: AsyncThrowingStream<SSEFrame, Error>.Continuation) {
        self.origin = origin
        self.continuation = continuation
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
        completionHandler(sameOrigin(origin, request.url) ? request : nil)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
                    completionHandler: @escaping @Sendable (URLSession.ResponseDisposition) -> Void) {
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        if !(200..<300).contains(code) { failure = (code, nil) }
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        if failure != nil {
            errorBody.append(data)
            return
        }
        for frame in parser.feed(data) { continuation.yield(frame) }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if let failure {
            if failure.code == 401 {
                continuation.finish(throwing: APIError.unauthorized)
            } else {
                let message = (try? JSONDecoder().decode(JSONValue.self, from: errorBody))?["error"]?.string
                continuation.finish(throwing: APIError.status(failure.code, message ?? "Stream failed"))
            }
        } else if let error {
            continuation.finish(throwing: APIError.transport(error.localizedDescription))
        } else {
            continuation.finish()
        }
        session.finishTasksAndInvalidate()
    }
}
