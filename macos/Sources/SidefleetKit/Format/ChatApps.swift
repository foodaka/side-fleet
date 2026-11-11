import Foundation

/// Chat apps: the assistants people already chat with (Hermes, OpenClaw, Claude Code) connect to the
/// fleet over MCP at `/mcp`, with an API token. What the screen offers and the setup it hands out for
/// each one. A port of web/src/lib/sf/chat.ts.
public enum ChatAppID: String, Sendable, Hashable, CaseIterable, Identifiable {
    case hermes, openclaw, claude, other
    public var id: String { rawValue }
}

public struct ChatApp: Sendable, Hashable, Identifiable {
    public let id: ChatAppID
    public let name: String
    public let hint: String
    /// It can follow a setup message itself (it has a terminal and edits its own config).
    public let canSelfSetup: Bool
}

public struct ChatAccess: Sendable, Hashable, Identifiable {
    public let scope: String
    public let name: String
    public let hint: String
    /// How a connection with this access reads in the list.
    public let short: String
    public var id: String { scope }
}

public struct ChatSetup: Sendable, Hashable {
    public var before: String
    public var snippet: String
    public var after: String
}

public enum ChatApps {
    public static let apps: [ChatApp] = [
        ChatApp(id: .hermes, name: "Hermes", hint: "Hermes Agent by Nous Research", canSelfSetup: true),
        ChatApp(id: .openclaw, name: "OpenClaw", hint: "Your OpenClaw assistant", canSelfSetup: true),
        ChatApp(id: .claude, name: "Claude Code", hint: "On your computer", canSelfSetup: true),
        ChatApp(id: .other, name: "Something else", hint: "Any app with MCP, or a script", canSelfSetup: false),
    ]

    public static let access: [ChatAccess] = [
        ChatAccess(scope: "read", name: "Check on tasks",
                   hint: "See what's running, how tasks went and which files changed. It can't start or change anything.",
                   short: "Can check on tasks"),
        ChatAccess(scope: "tasks", name: "Run tasks",
                   hint: "Also start tasks, reply to them, stop them and open pull requests. Tasks edit files but never run commands without asking.",
                   short: "Can run tasks"),
        ChatAccess(scope: "full", name: "Run tasks and approve commands",
                   hint: "Also say yes when a task asks to run a command, and start tasks that run commands without asking. Only for an assistant you trust with your machines.",
                   short: "Can run tasks and approve commands"),
    ]

    public static func app(_ id: ChatAppID) -> ChatApp { apps.first { $0.id == id } ?? apps[3] }
    public static func access(of scope: String) -> ChatAccess { access.first { $0.scope == scope } ?? access[0] }

    /// What the app is called in a sentence: its name, or "your app".
    public static func spoken(_ id: ChatAppID) -> String { id == .other ? "your app" : app(id).name }

    /// The name a connection is saved under.
    public static func tokenName(_ id: ChatAppID) -> String { id == .other ? "Other app" : app(id).name }

    public static func mcpURL(_ control: URL?) -> String {
        guard let control else { return "/mcp" }
        var base = control.absoluteString
        while base.hasSuffix("/") { base.removeLast() }
        return base + "/mcp"
    }

    /// JSON as JavaScript's `JSON.stringify` writes it, keys in the order given.
    private static func quoted(_ s: String) -> String {
        var out = "\""
        for u in s.unicodeScalars {
            switch u {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default:
                if u.value < 0x20 {
                    out += String(format: "\\u%04x", u.value)
                } else {
                    out.unicodeScalars.append(u)
                }
            }
        }
        return out + "\""
    }

    /// What to add to the app so it can reach Sidefleet.
    public static func setupSnippet(_ app: ChatAppID, token: String, control: URL?) -> String {
        let url = mcpURL(control)
        switch app {
        case .hermes:
            return "mcp_servers:\n  sidefleet:\n    url: \"\(url)\"\n    headers:\n      Authorization: \"Bearer \(token)\""
        case .openclaw:
            let json = "{\"url\":\(quoted(url)),\"transport\":\"streamable-http\",\"headers\":{\"Authorization\":\(quoted("Bearer \(token)"))}}"
            return "openclaw mcp set sidefleet '\(json)'"
        case .claude:
            return "claude mcp add --transport http --scope user sidefleet \(url) --header \"Authorization: Bearer \(token)\""
        case .other:
            return "URL: \(url)\nHeader: Authorization: Bearer \(token)"
        }
    }

    /// Where the snippet goes, said plainly.
    public static func setupSteps(_ app: ChatAppID) -> (before: String, after: String) {
        switch app {
        case .hermes:
            ("On the computer where Hermes runs, add this to ~/.hermes/config.yaml (if the file already has an mcp_servers: line, add the sidefleet part under it):",
             "Then send /reload-mcp to Hermes, or wait a minute: it picks the change up on its own.")
        case .openclaw:
            ("On the computer where OpenClaw runs, run this in a terminal:", "OpenClaw picks it up with your next message.")
        case .claude:
            ("Run this in a terminal:", "Then start Claude Code (or a new session). Sidefleet shows up in /mcp.")
        case .other:
            ("Add an MCP server to your app with this address and header (Streamable HTTP). A script can call the API at the same address with the same header.", "")
        }
    }

    /// A message to send the assistant so it sets itself up.
    public static func setupMessage(_ app: ChatAppID, token: String, control: URL?) -> String {
        let how: String
        switch app {
        case .hermes: how = "Add this MCP server to your config in ~/.hermes/config.yaml (under mcp_servers), then reload your MCP servers:"
        case .openclaw: how = "Run this command to add the Sidefleet MCP server:"
        case .claude: how = "Run this command to add the Sidefleet MCP server for all my projects:"
        case .other: return setupSnippet(app, token: token, control: control)
        }
        return [
            "Please connect yourself to my Sidefleet, so you can run coding tasks on my machines for me.",
            how,
            setupSnippet(app, token: token, control: control),
            "When it's connected, use Sidefleet's fleet_overview tool and tell me which machines you can see.",
        ].joined(separator: "\n\n")
    }

    /// What to try once it's connected.
    public static let examples = [
        "What's running on my fleet?",
        "Fix the failing test in my web repo",
        "How did that task go? Show me what changed",
        "Open a pull request for it",
    ]

    // MARK: Updates in chat
    // Sidefleet tells the app when a task finishes, fails, needs an OK or gets a pull request; the app
    // turns that into a message in the chat. Claude Code has no chat to message, so it doesn't get these.

    /// The app a connection was made for (its name is the app's).
    public static func appOf(name: String) -> ChatAppID {
        apps.first { $0.id != .other && $0.name.lowercased() == name.lowercased() }?.id ?? .other
    }

    public static func hasUpdates(_ app: ChatAppID) -> Bool { app != .claude }

    public static func updatesKind(_ app: ChatAppID) -> String {
        app == .hermes ? "hermes" : app == .openclaw ? "openclaw" : "webhook"
    }

    /// Where the app listens by default, to show as an example.
    public static func updatesPlaceholder(_ app: ChatAppID) -> String {
        switch app {
        case .hermes: "https://my-server.example.com:8644"
        case .openclaw: "https://my-server.example.com:18789"
        case .claude: ""
        case .other: "https://example.com/sidefleet-webhook"
        }
    }

    /// The full address updates go to: the app's own path on the address given.
    public static func updatesURL(_ app: ChatAppID, address: String) -> String {
        var a = address.trimmingCharacters(in: .whitespacesAndNewlines)
        while a.hasSuffix("/") { a.removeLast() }
        if app == .hermes, !a.contains("/webhooks/") { return a + "/webhooks/sidefleet" }
        if app == .openclaw, !a.contains("/hooks/") { return a + "/hooks/agent" }
        return a
    }

    /// The address as the person typed it, from the stored URL.
    public static func updatesAddress(_ app: ChatAppID, url: String) -> String {
        func dropping(_ suffix: String) -> String { url.hasSuffix(suffix) ? String(url.dropLast(suffix.count)) : url }
        switch app {
        case .hermes: return dropping("/webhooks/sidefleet")
        case .openclaw: return dropping("/hooks/agent")
        default: return url
        }
    }

    /// Where Hermes delivers its messages.
    public static let platforms: [(id: String, name: String)] = [
        ("telegram", "Telegram"), ("whatsapp", "WhatsApp"), ("discord", "Discord"), ("slack", "Slack"), ("signal", "Signal"),
    ]

    private static func hermesSubscribe(_ secret: String, _ platform: String) -> String {
        "hermes webhook subscribe sidefleet --prompt \"{text}\" --deliver \(platform) --mirror-to-session --secret \(secret) --description \"Sidefleet task updates\""
    }

    private static func openclawHooks(_ secret: String) -> String {
        "hooks: {\n  enabled: true,\n  token: \"\(secret)\",\n  path: \"/hooks\",\n},"
    }

    /// What to do on the app's side so updates arrive.
    public static func updatesSetup(_ app: ChatAppID, secret: String, platform: String) -> ChatSetup {
        switch app {
        case .hermes:
            ChatSetup(before: "On the computer where Hermes runs, turn on its webhooks if they are off (add WEBHOOK_ENABLED=true to ~/.hermes/.env and restart Hermes). Then run:",
                      snippet: hermesSubscribe(secret, platform), after: "Then press Send a test update below.")
        case .openclaw:
            ChatSetup(before: "On the computer where OpenClaw runs, add this to ~/.openclaw/openclaw.json (inside the outer { }), then run openclaw gateway restart:",
                      snippet: openclawHooks(secret), after: "Then press Send a test update below.")
        default:
            ChatSetup(before: "Sidefleet POSTs JSON (event_type, text, task) to your address. To check it came from Sidefleet: X-Webhook-Signature-V2 is the hex HMAC-SHA256 of \"<X-Webhook-Timestamp>.<body>\" with this secret:",
                      snippet: secret, after: "")
        }
    }

    /// A message to send the assistant so it sets updates up itself.
    public static func updatesMessage(_ app: ChatAppID, secret: String, platform: String) -> String {
        let hello = "Please set yourself up so Sidefleet can tell you about my coding tasks, and you can tell me here."
        let bye = "When a Sidefleet update arrives, tell me about it in a short message and help me act on it with the Sidefleet tools."
        if app == .hermes {
            return [
                hello,
                "1. Make sure your webhook server is on: WEBHOOK_ENABLED=true in ~/.hermes/.env (port 8644). If you have to change it, restart your gateway.",
                "2. Run this command. If you know this chat's id, add --deliver-chat-id with it so updates come to this chat:\n\n\(hermesSubscribe(secret, platform))",
                bye,
            ].joined(separator: "\n\n")
        }
        return [
            hello,
            "Turn on your webhooks: in your config (~/.openclaw/openclaw.json) set\n\n\(openclawHooks(secret))\n\nthen restart your gateway (openclaw gateway restart).",
            bye,
        ].joined(separator: "\n\n")
    }
}
