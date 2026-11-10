import Foundation

/// The demo's team: rooms, notes, invites and chat apps. Made-up people and work, nothing real.
struct DemoTeam {
    static let userID = "u-ada"
    static let members: [JSONValue] = [
        ["id": "u-ada", "login": "ada", "role": "owner"],
        ["id": "u-grace", "login": "grace", "role": "member"],
        ["id": "u-linus", "login": "linus", "role": "member"],
    ]

    private var rooms: [JSONValue]
    private var notes: [String: [JSONValue]]
    private var cards: [String: [JSONValue]]
    private var invites: [JSONValue]
    private var apiTokens: [JSONValue]
    private var nextID = 100

    private static func iso(_ d: Date) -> JSONValue { .string(Timestamp.format(d)) }

    init(now: Date) {
        func ago(_ minutes: Double) -> JSONValue { Self.iso(now.addingTimeInterval(-minutes * 60)) }
        rooms = [
            [
                "id": "room-applepay", "name": "Apple Pay at checkout",
                "goal": "Shoppers on Safari and iOS can pay with Apple Pay from the cart and the checkout page.\n\nDone means: the button shows only where Apple Pay is available, the order total and shipping options match the regular flow, and the e2e suite covers a successful payment and a cancelled sheet. Keep the existing card form untouched.",
                "repo": "github.com/acme/storefront", "branch": "room/apple-pay-at-checkout", "created_by": "u-ada", "created_by_login": "ada",
                "created_at": ago(60 * 26), "archived_at": .null, "integrated_at": ago(60 * 3), "last_activity_at": ago(4),
                "task_count": 4, "active_count": 1, "integrated_count": 1,
            ],
            [
                "id": "room-billing", "name": "Move billing to Stripe",
                "goal": "Replace the home-grown invoicing with Stripe Billing. Subscriptions, proration and dunning come from Stripe; we keep our own plan table as the source of what each plan includes.",
                "repo": "github.com/acme/storefront", "branch": "room/move-billing-to-stripe", "created_by": "u-grace", "created_by_login": "grace",
                "created_at": ago(60 * 24 * 4), "archived_at": .null, "integrated_at": .null, "last_activity_at": ago(60 * 5),
                "task_count": 2, "active_count": 0, "integrated_count": 0,
            ],
            [
                "id": "room-docs", "name": "v3 API docs",
                "goal": "", "repo": "github.com/acme/docs", "branch": "room/v3-api-docs", "created_by": "u-linus", "created_by_login": "linus",
                "created_at": ago(60 * 24 * 9), "archived_at": .null, "integrated_at": ago(60 * 24 * 2), "last_activity_at": ago(60 * 30),
                "task_count": 3, "active_count": 0, "integrated_count": 3,
            ],
        ]
        notes = [
            "room-applepay": [
                ["id": "n1", "author_id": "u-ada", "author_login": "ada", "body": "I'll take the button and the payment sheet. Grace has the server side: merchant validation and the payment intent.", "created_at": ago(60 * 25), "edited_at": .null],
                ["id": "n2", "author_id": "u-grace", "author_login": "grace", "body": "Merchant validation is in. The endpoint is POST /api/apple-pay/session and it wants the validation URL from the sheet.", "created_at": ago(60 * 4), "edited_at": .null],
                ["id": "n3", "author_id": "u-linus", "author_login": "linus", "body": "Don't touch tests/e2e/checkout.spec.ts for now, I'm fixing its flaky wait in my task.", "created_at": ago(38), "edited_at": ago(31)],
            ],
        ]
        func card(_ id: String, _ title: String, by: String, agent: String = "claude", model: String = "opus", worker: String, status: String, minutes: Double,
                  files: [String] = [], add: Double = 0, del: Double = 0, integration: JSONValue = .null, pr: JSONValue = .null) -> JSONValue {
            [
                "id": .string(id), "title": .string(title), "created_by": .string("u-\(by)"), "created_by_login": .string(by), "agent": .string(agent),
                "model": .string(model), "worker_id": .string(worker), "status": .string(status), "last_activity_at": ago(minutes),
                "files": .array(files.map { .string($0) }), "additions": .number(add), "deletions": .number(del), "integration": integration, "pr": pr,
            ]
        }
        cards = [
            "room-applepay": [
                card("demo-room-button", "Add the Apple Pay button to the cart and checkout, shown only where it's available", by: "ada", worker: "hetzner-vps", status: "running", minutes: 4),
                card("demo-room-server", "Merchant validation endpoint and payment intent for Apple Pay", by: "grace", model: "sonnet", worker: "grace-mbp", status: "succeeded", minutes: 60 * 3,
                     files: ["src/routes/api/apple-pay/session.ts", "src/lib/payments/intent.ts", "src/lib/payments/intent.test.ts"], add: 184, del: 12,
                     integration: ["status": "succeeded", "at": ago(60 * 3), "commit": "4e61fe7e2b9a"]),
                card("demo-checkout", "The checkout e2e test is flaky on CI. Find out why and fix it", by: "linus", worker: "linus-desktop", status: "succeeded", minutes: 35,
                     files: ["src/lib/checkout/PayButton.tsx", "tests/e2e/checkout.spec.ts"], add: 8, del: 8,
                     integration: ["status": "failed", "at": ago(30), "conflicts": ["src/lib/checkout/PayButton.tsx"]]),
                card("demo-room-totals", "Send shipping options and the order total to the payment sheet", by: "ada", agent: "codex", model: "", worker: "mac-mini", status: "succeeded", minutes: 50,
                     files: ["src/lib/checkout/totals.ts", "src/lib/checkout/applePay.ts", "src/lib/checkout/applePay.test.ts", "src/lib/checkout/shipping.ts", "src/lib/cart/summary.ts"], add: 96, del: 21),
            ],
        ]
        invites = [
            ["id": "inv1", "role": "member", "created_by_login": "ada", "expires_at": Self.iso(now.addingTimeInterval(5 * 86_400)), "max_uses": .null, "uses": 2, "revoked_at": .null],
        ]
        apiTokens = [
            [
                "id": "at1", "name": "Hermes", "scope": "tasks", "prefix": "sfa_h3rm", "created_at": ago(60 * 24 * 12), "revoked_at": .null, "last_used_at": ago(22),
                "updates": ["kind": "hermes", "url": "https://hermes.example.com:8644/webhooks/sidefleet", "all_tasks": false, "last_at": ago(35), "last_error": .null],
            ],
        ]
    }

    private mutating func newID(_ prefix: String) -> String {
        nextID += 1
        return "\(prefix)\(nextID)"
    }

    private func encode(_ v: JSONValue) throws -> Data { try JSONEncoder().encode(v) }

    private func roomIndex(_ id: String) throws -> Int {
        guard let i = rooms.firstIndex(where: { $0["id"]?.string == id }) else { throw APIError.status(404, "room not found") }
        return i
    }

    private mutating func touch(_ i: Int, now: Date, _ change: (inout [String: JSONValue]) -> Void = { _ in }) {
        guard case .object(var o) = rooms[i] else { return }
        o["last_activity_at"] = Self.iso(now)
        change(&o)
        rooms[i] = .object(o)
    }

    /// A task started in a room: its card, and the room's counts.
    mutating func addTask(room: String, run: String, body: JSONValue?, now: Date) throws {
        let i = try roomIndex(room)
        cards[room, default: []].insert([
            "id": .string(run), "title": body?["task"] ?? "", "created_by": .string(Self.userID), "created_by_login": "ada",
            "agent": body?["agent"] ?? "claude", "model": body?["model"] ?? "sonnet", "worker_id": body?["worker_id"] ?? "mac-mini", "status": "running",
            "last_activity_at": Self.iso(now), "files": [], "additions": 0, "deletions": 0, "integration": .null, "pr": .null,
        ], at: 0)
        touch(i, now: now) { o in
            o["task_count"] = .number(Double((o["task_count"]?.int ?? 0) + 1))
            o["active_count"] = .number(Double((o["active_count"]?.int ?? 0) + 1))
        }
    }

    /// Add a task's work to its room: in the demo it always merges.
    mutating func integrate(run: String, now: Date) -> Bool {
        for (room, list) in cards {
            guard let c = list.firstIndex(where: { $0["id"]?.string == run }), case .object(var card) = list[c] else { continue }
            let first = card["integration"]?["status"]?.string != "succeeded"
            card["integration"] = ["status": "succeeded", "at": Self.iso(now), "commit": "9fa511c59d02"]
            cards[room]?[c] = .object(card)
            if let i = try? roomIndex(room) {
                touch(i, now: now) { o in
                    if o["integrated_at"]?.string == nil { o["integrated_at"] = Self.iso(now) }
                    if first { o["integrated_count"] = .number(Double((o["integrated_count"]?.int ?? 0) + 1)) }
                }
            }
            return true
        }
        return false
    }

    /// Answers a team route, or nil when the route isn't one of these.
    mutating func handle(_ method: String, _ seg: [String], query: String, body: JSONValue?, now: Date) throws -> Data? {
        guard seg.count > 1 else { return nil }
        switch (method, seg[1]) {
        case ("GET", "rooms") where seg.count == 2:
            let archived = query.contains("archived=true")
            let list = rooms.filter { ($0["archived_at"]?.string != nil) == archived }
                .sorted { ($0["last_activity_at"]?.string ?? "") > ($1["last_activity_at"]?.string ?? "") }
            return try encode(.array(list))
        case ("POST", "rooms") where seg.count == 2:
            let name = body?["name"]?.string ?? "Room"
            let slug = name.lowercased().map { $0.isLetter || $0.isNumber ? String($0) : "-" }.joined()
            let room: JSONValue = [
                "id": .string(newID("room-")), "name": .string(name), "goal": body?["goal"] ?? "", "repo": body?["repo"] ?? "", "branch": .string("room/\(slug)"),
                "created_by": .string(Self.userID), "created_by_login": "ada", "created_at": Self.iso(now), "archived_at": .null, "integrated_at": .null,
                "last_activity_at": Self.iso(now), "task_count": 0, "active_count": 0, "integrated_count": 0,
            ]
            rooms.append(room)
            return try encode(room)
        case ("GET", "rooms") where seg.count == 3:
            let i = try roomIndex(seg[2])
            return try encode(["room": rooms[i], "notes": .array(notes[seg[2]] ?? []), "tasks": .array(cards[seg[2]] ?? [])])
        case ("PATCH", "rooms") where seg.count == 3:
            let i = try roomIndex(seg[2])
            touch(i, now: now) { o in
                if let v = body?["name"] { o["name"] = v }
                if let v = body?["goal"] { o["goal"] = v }
                if let a = body?["archived"]?.bool { o["archived_at"] = a ? Self.iso(now) : .null }
            }
            return try encode(rooms[i])
        case ("POST", "rooms") where seg.count == 4 && seg[3] == "notes":
            let i = try roomIndex(seg[2])
            let note: JSONValue = ["id": .string(newID("n")), "author_id": .string(Self.userID), "author_login": "ada", "body": body?["body"] ?? "", "created_at": Self.iso(now), "edited_at": .null]
            notes[seg[2], default: []].append(note)
            touch(i, now: now)
            return try encode(note)
        case ("PATCH", "rooms") where seg.count == 5 && seg[3] == "notes":
            guard let n = notes[seg[2]]?.firstIndex(where: { $0["id"]?.string == seg[4] }), case .object(var note)? = notes[seg[2]]?[n] else {
                throw APIError.status(404, "note not found")
            }
            note["body"] = body?["body"] ?? ""
            note["edited_at"] = Self.iso(now)
            notes[seg[2]]?[n] = .object(note)
            return try encode(.object(note))
        case ("DELETE", "rooms") where seg.count == 5 && seg[3] == "notes":
            notes[seg[2]]?.removeAll { $0["id"]?.string == seg[4] }
            return Data()

        case ("GET", "orgs") where seg.last == "members":
            return try encode(.array(Self.members))
        case ("GET", "orgs") where seg.last == "invites":
            return try encode(.array(invites))
        case ("POST", "orgs") where seg.last == "invites":
            let days = body?["expires_in_days"]?.double ?? 7
            let invite: JSONValue = [
                "id": .string(newID("inv")), "role": "member", "created_by_login": "ada", "expires_at": Self.iso(now.addingTimeInterval(days * 86_400)),
                "max_uses": body?["max_uses"] ?? .null, "uses": 0, "revoked_at": .null,
            ]
            invites.insert(invite, at: 0)
            return try encode(["invite": invite, "code": "demo-invite-not-real"])
        case ("DELETE", "orgs") where seg.count == 5 && seg[3] == "invites":
            invites.removeAll { $0["id"]?.string == seg[4] }
            return Data()
        case ("GET", "invites") where seg.count == 3:
            return try encode(["org_name": "Globex", "inviter_login": "grace", "role": "member", "valid": true])
        case ("POST", "invites"):
            throw APIError.status(400, "Joining another team isn't part of the demo")

        case ("GET", "api-tokens"):
            return try encode(.array(apiTokens))
        case ("POST", "api-tokens") where seg.count == 2:
            var token: [String: JSONValue] = [
                "id": .string(newID("at")), "name": body?["name"] ?? "Other app", "scope": body?["scope"] ?? "tasks", "prefix": "sfa_demo",
                "created_at": Self.iso(now), "revoked_at": .null, "last_used_at": .null, "updates": .null,
            ]
            apiTokens.append(.object(token))
            token["token"] = "sfa_demo-key-not-real"
            return try encode(.object(token))
        case ("DELETE", "api-tokens") where seg.count == 3:
            apiTokens.removeAll { $0["id"]?.string == seg[2] }
            return Data()
        case (_, "api-tokens") where seg.count >= 4 && seg[3] == "updates":
            guard let i = apiTokens.firstIndex(where: { $0["id"]?.string == seg[2] }), case .object(var token) = apiTokens[i] else {
                throw APIError.status(404, "token not found")
            }
            if seg.count == 5 {
                // The test update "arrives".
                if case .object(var u)? = token["updates"] {
                    u["last_at"] = Self.iso(now)
                    token["updates"] = .object(u)
                    apiTokens[i] = .object(token)
                }
                return Data()
            }
            if method == "DELETE" {
                token["updates"] = .null
                apiTokens[i] = .object(token)
                return Data()
            }
            token["updates"] = ["kind": body?["kind"] ?? "webhook", "url": body?["url"] ?? "", "all_tasks": body?["all_tasks"] ?? false, "last_at": .null, "last_error": .null]
            apiTokens[i] = .object(token)
            token["secret"] = "demo-secret-not-real"
            return try encode(.object(token))
        default:
            return nil
        }
    }

    /// The app "calls in" a few seconds after it's connected, so the setup steps can be walked through.
    mutating func tick(now: Date) {
        for (i, t) in apiTokens.enumerated() {
            guard case .object(var o) = t, o["last_used_at"]?.string == nil,
                  let made = o["created_at"]?.string.flatMap(Timestamp.parse), now.timeIntervalSince(made) > 6 else { continue }
            o["last_used_at"] = Self.iso(now)
            apiTokens[i] = .object(o)
        }
    }
}
