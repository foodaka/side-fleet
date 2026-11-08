import Foundation

/// Made-up machines, repos and tasks. Nothing here is real.
enum DemoData {
    struct SeedRun {
        var json: [String: JSONValue]
        var events: [(kind: String, payload: JSONValue, at: Date)]
        var script: [DemoStep] = []
        var waitingOnApproval = false
    }

    static let me: JSONValue = [
        "user": .null,
        "orgs": [["id": "default", "name": "Acme", "slug": "acme", "plan": "pro", "role": "owner", "limits": .null]],
    ]

    static let tokens: [JSONValue] = [
        ["id": "t1", "name": "hetzner-vps", "prefix": "sfw_a1b2", "created_at": "2026-09-14T09:00:00Z", "revoked_at": .null, "last_used_at": "2026-11-07T07:20:00Z"],
    ]

    static let runDefaults: [String: JSONValue] = [
        "id": "demo", "task": "", "agent": "claude", "model": "sonnet", "effort": .null, "repo": "github.com/acme/storefront",
        "status": "queued", "worker_id": .null, "stop_requested": false, "created_at": "2026-11-07T07:00:00Z", "started_at": .null,
        "finished_at": .null, "result": .null, "parent_id": .null, "session_id": .null, "resume_session_id": .null,
        "pinned_worker_id": .null, "permission_mode": "acceptEdits", "isolation": "worktree", "failover": false, "moved_from": .null,
        "resume_from_branch": .null, "images": [], "kind": "agent", "restore_to": .null, "forked_from": .null, "fork_commit": .null,
        "archived_at": .null, "snoozed_until": .null, "pr": .null, "created_by": .null, "room_id": .null, "context": .null, "start_at": .null,
    ]

    private static func iso(_ d: Date) -> JSONValue { .string(Timestamp.format(d)) }

    static func workers(now: Date) -> [JSONValue] {
        func worker(_ id: String, _ name: String, os: String, cores: Int, cpu: Double, memUsed: Double, memTotal: Double, diskFree: Double, diskTotal: Double, up: Int, parallel: Int, online: Bool, agents: JSONValue, repos: [String], seen: Date) -> JSONValue {
            [
                "id": .string(id), "display_name": .string(name), "online": .bool(online), "update_requested": false, "outdated": false,
                "last_heartbeat_at": iso(seen), "limited_until": .object([:]), "owner": .null, "mine": true, "guest": false,
                "capabilities": [
                    "hostname": .string(id + ".local"), "os": .string(os), "version": "0.1.0 (2026-11-06 9fa511c59)", "agents": agents,
                    "repos": .array(repos.map { ["name": .string(String($0.split(separator: "/").last ?? "")), "path": .string("/home/dev/" + $0), "id": .string("github.com/" + $0)] }),
                    "system": ["cpu_cores": .number(Double(cores)), "cpu_percent": .number(cpu), "load_avg_1m": 1.2, "mem_total_mb": .number(memTotal * 1024), "mem_used_mb": .number(memUsed * 1024), "disk_total_gb": .number(diskTotal), "disk_free_gb": .number(diskFree), "uptime_secs": .number(Double(up))],
                    "slots": ["parallel": .number(Double(parallel)), "active": 0],
                ],
            ]
        }
        let claude: JSONValue = ["version": "2.4.1", "signed_in": true]
        let codex: JSONValue = ["version": "0.61.0", "signed_in": true, "models": ["gpt-6-luna", "gpt-6-luna-mini"]]
        let opencode: JSONValue = ["version": "0.9.2", "models": ["moonshotai/kimi-k2", "deepseek/deepseek-v4"], "models_total": 2]
        return [
            worker("hetzner-vps", "Hetzner VPS", os: "linux", cores: 8, cpu: 63, memUsed: 9.6, memTotal: 16, diskFree: 151, diskTotal: 320, up: 864_000, parallel: 2, online: true, agents: ["claude": claude, "opencode": opencode], repos: ["acme/storefront", "acme/cli"], seen: now),
            worker("mac-mini", "Mac mini", os: "macos", cores: 10, cpu: 18, memUsed: 13.8, memTotal: 32, diskFree: 412, diskTotal: 1000, up: 864_000, parallel: 3, online: true, agents: ["claude": claude, "codex": codex], repos: ["acme/storefront", "acme/cli", "acme/docs"], seen: now),
            worker("macbook-pro", "MacBook Pro", os: "macos", cores: 12, cpu: 4, memUsed: 11.7, memTotal: 36, diskFree: 300, diskTotal: 1000, up: 259_200, parallel: 2, online: false, agents: ["claude": claude, "codex": codex], repos: ["acme/storefront", "acme/cli", "acme/docs"], seen: now.addingTimeInterval(-3 * 3600)),
        ]
    }

    static func inbox(now: Date) -> [JSONValue] {
        [
            ["id": "demo-checkout", "run_id": "demo-checkout", "title": "The checkout e2e test is flaky on CI. Find out why and fix it", "repo": "github.com/acme/storefront", "agent": "claude", "reason": "finished", "label": "Finished, waiting for a reply", "since": iso(now.addingTimeInterval(-35 * 60))],
            ["id": "demo-stripe", "run_id": "demo-stripe", "title": "Upgrade Stripe SDK to v18 and fix the breaking changes", "repo": "github.com/acme/storefront", "agent": "claude", "reason": "failed", "label": "Failed", "since": iso(now.addingTimeInterval(-3600))],
        ]
    }

    private static func run(_ id: String, _ task: String, status: String, created: Date, started: Date? = nil, finished: Date? = nil, worker: String? = nil, repo: String = "github.com/acme/storefront", model: String = "opus", agent: String = "claude", perm: String = "acceptEdits", session: String? = "demo-session", extra: [String: JSONValue] = [:]) -> [String: JSONValue] {
        var j = runDefaults
        j["id"] = .string(id)
        j["task"] = .string(task)
        j["status"] = .string(status)
        j["created_at"] = iso(created)
        j["started_at"] = started.map(iso) ?? .null
        j["finished_at"] = finished.map(iso) ?? .null
        j["worker_id"] = worker.map { .string($0) } ?? .null
        j["repo"] = .string(repo)
        j["model"] = .string(model)
        j["agent"] = .string(agent)
        j["permission_mode"] = .string(perm)
        j["session_id"] = session.map { .string($0) } ?? .null
        for (k, v) in extra { j[k] = v }
        return j
    }

    private static func diffFile(_ path: String, status: String = "modified", add: Int, del: Int, patch: String?, omitted: String? = nil) -> JSONValue {
        ["path": .string(path), "old_path": .null, "status": .string(status), "additions": .number(Double(add)), "deletions": .number(Double(del)), "patch": patch.map { .string($0) } ?? .null, "omitted": omitted.map { .string($0) } ?? .null]
    }

    static let checkoutPatch = """
    @@ -18,11 +18,18 @@ export function PayButton({ cart, onPay }: Props) {
       const quote = useTaxQuote(cart);
       const [submitting, setSubmitting] = useState(false);

    -  const disabled = submitting;
    +  // The total isn't known until the quote arrives, so paying has to wait for it.
    +  const pending = quote.status === 'pending';
    +  const disabled = submitting || pending;

       return (
    -    <button className="pay" disabled={disabled} onClick={() => onPay(cart)}>
    -      Pay {formatMoney(cart.total)}
    +    <button className="pay" disabled={disabled} aria-busy={pending} onClick={() => onPay(cart)}>
    +      {pending ? <Spinner size={14} /> : null}
    +      Pay {formatMoney(quote.total ?? cart.total)}
         </button>
       );
     }
    """

    static let specPatch = """
    @@ -41,14 +41,13 @@ test.describe('checkout', () => {
       test('pays with a saved card', async ({ page }) => {
         await page.goto('/checkout');
    -    await page.route('**/tax/quote', async (route) => {
    -      await new Promise((r) => setTimeout(r, 200));
    -      await route.fulfill({ json: { total: 4200 } });
    -    });
    +    const quote = page.waitForResponse('**/tax/quote');
         await page.getByLabel('Card').fill('4242 4242 4242 4242');
    -    await page.waitForTimeout(250);
    +    await quote;
         await page.getByRole('button', { name: /pay/i }).click();
         await expect(page.getByText('Thanks for your order')).toBeVisible();
       });
     });
    """

    static func threads(now: Date) -> [[SeedRun]] {
        func ago(_ minutes: Double) -> Date { now.addingTimeInterval(-minutes * 60) }
        var out: [[SeedRun]] = []

        // Running, and streaming as you watch.
        let guideStart = ago(5.8)
        out.append([SeedRun(
            json: run("demo-guide", "Write a migration guide for the v3 API", status: "running", created: guideStart, started: guideStart, worker: "mac-mini", repo: "github.com/acme/docs", model: "sonnet", extra: ["effort": "high"]),
            events: [
                ("status", ["status": "running", "worker_id": "mac-mini"], guideStart),
                ("workspace", ["mode": "worktree", "branch": "sidefleet/9c2e41d0", "from": ["ref": "origin/main", "commit": "a41f09c27b"]], guideStart),
                ("started", ["command": "claude -p <task> --output-format stream-json --verbose --model sonnet --permission-mode acceptEdits"], guideStart),
                ("agent_init", ["model": "claude-sonnet-5-5", "session_id": "demo-session-guide"], guideStart),
                ("assistant_text", ["text": "I'll start from what actually changed between v2 and v3, so the guide covers real breaks rather than the changelog's summary."], ago(5.6)),
                ("tool_use", ["id": "g1", "name": "Grep", "summary": "deprecated|removed in docs/changelog"], ago(5.5)),
                ("tool_result", ["tool_use_id": "g1", "content": "docs/changelog/3.0.md:12: `Client.fetch` removed\ndocs/changelog/3.0.md:18: pagination is cursor-based", "is_error": false], ago(5.4)),
                ("tool_use", ["id": "g2", "name": "Read", "summary": "docs/changelog/3.0.md"], ago(5.3)),
                ("tool_result", ["tool_use_id": "g2", "content": "# 3.0\n\nFour breaking changes…", "is_error": false], ago(5.2)),
                ("assistant_text", ["text": "There are four breaking changes. Pagination is the one that bites hardest:\n\n- **Pagination** moved from `page` to opaque cursors\n- `Client.fetch` is gone; use `client.items.list()`\n- Errors are now typed, not strings\n- Timestamps are always UTC"], ago(4.9)),
                ("tool_use", ["id": "g3", "name": "Write", "summary": "docs/guides/migrate-to-v3.md"], ago(3.1)),
                ("tool_result", ["tool_use_id": "g3", "content": "Wrote 148 lines", "is_error": false], ago(3)),
            ],
            script: liveWriting())])

        // Waiting on you: a tool call to approve.
        let rateStart = ago(8.5)
        out.append([SeedRun(
            json: run("demo-rate", "Add rate limiting to the public webhook endpoint", status: "running", created: rateStart, started: rateStart, worker: "hetzner-vps", perm: "ask"),
            events: [
                ("status", ["status": "running", "worker_id": "hetzner-vps"], rateStart),
                ("workspace", ["mode": "worktree", "branch": "sidefleet/71ab03fe", "from": ["ref": "origin/main", "commit": "e03b19d4c1"]], rateStart),
                ("agent_init", ["model": "claude-opus-5-5", "session_id": "demo-session-rate"], rateStart),
                ("assistant_text", ["text": "A token bucket per signing secret fits here: webhooks arrive in bursts, and a fixed window would drop the legitimate ones."], ago(8.2)),
                ("tool_use", ["id": "r1", "name": "Read", "summary": "src/routes/webhooks.ts"], ago(8.1)),
                ("tool_result", ["tool_use_id": "r1", "content": "export async function POST(req) { … }", "is_error": false], ago(8)),
                ("tool_use", ["id": "r2", "name": "Edit", "summary": "src/routes/webhooks.ts"], ago(7)),
                ("tool_result", ["tool_use_id": "r2", "content": "Applied 1 edit", "is_error": false], ago(6.9)),
                ("assistant_text", ["text": "The limiter is in place. I'd like to run the webhook tests to check that bursts within the limit still get through."], ago(1)),
                ("approval_requested", ["id": "approval-1", "tool_name": "Bash", "summary": "pnpm test src/routes/webhooks.test.ts", "input": ["command": "pnpm test src/routes/webhooks.test.ts", "description": "Run the webhook tests"], "status": "pending", "expires_at": iso(now.addingTimeInterval(14 * 60))], ago(0.6)),
            ],
            waitingOnApproval: true)])

        // Scheduled for later.
        let tomorrow = Calendar.current.date(byAdding: .day, value: 1, to: Calendar.current.date(bySettingHour: 7, minute: 0, second: 0, of: now) ?? now) ?? now
        out.append([SeedRun(
            json: run("demo-audit", "Audit dependencies and open a PR for anything with a known vulnerability", status: "queued", created: ago(40), session: nil, extra: ["start_at": iso(tomorrow)]),
            events: [("status", ["status": "queued"], ago(40))])])

        // A read-only answer.
        let taxAt = ago(26)
        out.append([SeedRun(
            json: run("demo-tax", "Where do we compute shipping tax?", status: "succeeded", created: taxAt, started: taxAt, finished: ago(23), worker: "mac-mini", model: "haiku", perm: "plan"),
            events: [
                ("status", ["status": "running", "worker_id": "mac-mini"], taxAt),
                ("workspace", ["mode": "worktree", "branch": "sidefleet/52d7e8b3", "from": ["ref": "origin/main", "commit": "e03b19d4c1"]], taxAt),
                ("agent_init", ["model": "claude-haiku-5-5", "session_id": "demo-session-tax"], taxAt),
                ("tool_use", ["id": "t1", "name": "Grep", "summary": "shippingTax"], ago(25.8)),
                ("tool_result", ["tool_use_id": "t1", "content": "src/lib/tax/quote.ts:88\nsrc/lib/tax/rates.ts:14", "is_error": false], ago(25.7)),
                ("assistant_text", ["text": "In `src/lib/tax/quote.ts`, in `quoteOrder()` at line 88. It adds shipping to the taxable base only for destinations in `TAXES_SHIPPING` (`rates.ts:14`), so the same order can differ by state."], ago(24)),
                ("result", ["subtype": "success", "is_error": false, "result": "", "duration_ms": 182_400, "num_turns": 3, "total_cost_usd": 0.0212, "session_id": "demo-session-tax", "context": ["used": 18_400, "limit": 200_000]], ago(23)),
                ("status", ["status": "succeeded"], ago(23)),
            ])])

        // The full story: narration, tools, changes, ready for a pull request.
        let checkoutAt = ago(42.2)
        let checkoutDiff: JSONValue = [
            "files": [
                diffFile("src/lib/checkout/PayButton.tsx", add: 6, del: 3, patch: checkoutPatch),
                diffFile("tests/e2e/checkout.spec.ts", add: 2, del: 5, patch: specPatch),
            ],
            "file_count": 2, "additions": 8, "deletions": 8,
        ]
        out.append([SeedRun(
            json: run("demo-checkout", "The checkout e2e test is flaky on CI. Find out why and fix it", status: "succeeded", created: checkoutAt, started: checkoutAt, finished: ago(35), worker: "hetzner-vps", extra: ["effort": "high"]),
            events: [
                ("status", ["status": "running", "worker_id": "hetzner-vps"], checkoutAt),
                ("workspace", ["mode": "worktree", "branch": "sidefleet/4e61fe7e", "from": ["ref": "origin/main", "commit": "e03b19d4c1"]], checkoutAt),
                ("started", ["command": "claude -p <task> --output-format stream-json --verbose --model opus --effort high --permission-mode acceptEdits"], checkoutAt),
                ("agent_init", ["model": "claude-opus-5-5", "session_id": "demo-session-checkout"], checkoutAt),
                ("snapshot", ["before": "aaaaaaa", "after": "aaaaaaa"], checkoutAt),
                ("assistant_text", ["text": "I'll run the checkout spec a few times to reproduce the flake before changing anything."], ago(42)),
                ("tool_use", ["id": "c1", "name": "Bash", "summary": "pnpm playwright test checkout --repeat-each=20"], ago(41.9)),
                ("tool_result", ["tool_use_id": "c1", "content": "Running 20 tests using 4 workers\n  4 failed\n  16 passed (1.2m)\n\n  Timed out waiting for getByRole('button', { name: /pay/i }) to be enabled", "is_error": false], ago(40.7)),
                ("assistant_text", ["text": "It fails about one run in five, always waiting for the Pay button. The button is enabled only after the tax quote resolves, and the test mocks that request with a fixed 200 ms delay that sometimes lands after the card form re-renders."], ago(40.4)),
                ("tool_use", ["id": "c2", "name": "Read", "summary": "src/lib/checkout/PayButton.tsx"], ago(40.2)),
                ("tool_result", ["tool_use_id": "c2", "content": "export function PayButton({ cart, onPay }: Props) { … }", "is_error": false], ago(40.1)),
                ("tool_use", ["id": "c3", "name": "Edit", "summary": "tests/e2e/checkout.spec.ts"], ago(39.5)),
                ("tool_result", ["tool_use_id": "c3", "content": "Applied 2 edits", "is_error": false], ago(39.4)),
                ("tool_use", ["id": "c4", "name": "Bash", "summary": "pnpm playwright test checkout --repeat-each=20"], ago(39.3)),
                ("tool_result", ["tool_use_id": "c4", "content": "Running 20 tests using 4 workers\n  20 passed (1.1m)", "is_error": false], ago(38.2)),
                ("assistant_text", ["text": "20/20 passes after waiting for the tax response instead of a timer. The component also let you click Pay while the quote was pending, so real users could hit the same race."], ago(38)),
                ("tool_use", ["id": "c5", "name": "Edit", "summary": "src/lib/checkout/PayButton.tsx"], ago(37)),
                ("tool_result", ["tool_use_id": "c5", "content": "Applied 1 edit", "is_error": false], ago(36.9)),
                ("assistant_text", ["text": "Pay is now disabled with a spinner until the tax quote arrives, and the test waits for the response instead of sleeping. 50/50 local runs pass."], ago(35.2)),
                ("diff", ["turn": checkoutDiff], ago(35.1)),
                ("snapshot", ["before": "aaaaaaa", "after": "bbbbbbb"], ago(35.1)),
                ("result", ["subtype": "success", "is_error": false, "result": "", "duration_ms": 431_000, "num_turns": 18, "total_cost_usd": 1.12, "session_id": "demo-session-checkout", "context": ["used": 412_000, "limit": 1_000_000]], ago(35)),
                ("status", ["status": "succeeded"], ago(35)),
            ])])

        // A failure.
        let stripeAt = ago(70)
        out.append([SeedRun(
            json: run("demo-stripe", "Upgrade Stripe SDK to v18 and fix the breaking changes", status: "failed", created: stripeAt, started: stripeAt, finished: ago(60), worker: "mac-mini"),
            events: [
                ("status", ["status": "running", "worker_id": "mac-mini"], stripeAt),
                ("workspace", ["mode": "worktree", "branch": "sidefleet/c7f09e12", "from": ["ref": "origin/main", "commit": "e03b19d4c1"]], stripeAt),
                ("agent_init", ["model": "claude-opus-5-5", "session_id": "demo-session-stripe"], stripeAt),
                ("tool_use", ["id": "s1", "name": "Bash", "summary": "pnpm add stripe@18"], ago(69)),
                ("tool_result", ["tool_use_id": "s1", "content": "ERR_PNPM_FETCH_403 GET https://registry.example.com/stripe: Forbidden\n\nYour authentication token is missing or expired.", "is_error": true], ago(68.9)),
                ("assistant_text", ["text": "The registry refused the install, so I can't upgrade from this machine. Its npm token looks expired."], ago(61)),
                ("result", ["subtype": "error_during_execution", "is_error": true, "result": "", "duration_ms": 600_000, "num_turns": 4, "total_cost_usd": 0.31, "session_id": "demo-session-stripe"], ago(60)),
                ("status", ["status": "failed", "reason": "the agent exited with an error"], ago(60)),
            ])])

        // Merged yesterday, with a follow-up.
        let jsonAt = ago(60 * 26)
        var jsonRoot = run("demo-json", "Add a `--json` flag to the `export` command", status: "succeeded", created: jsonAt, started: jsonAt, finished: ago(60 * 25.8), worker: "mac-mini", repo: "github.com/acme/cli", model: "gpt-6-luna", agent: "codex", perm: "acceptEdits", extra: ["effort": "medium"])
        jsonRoot["pr"] = ["url": "https://github.com/acme/cli/pull/88", "number": 88, "state": "merged"]
        out.append([
            SeedRun(json: jsonRoot, events: [
                ("status", ["status": "running", "worker_id": "mac-mini"], jsonAt),
                ("workspace", ["mode": "worktree", "branch": "sidefleet/88e0ff21", "from": ["ref": "origin/main", "commit": "1bc90e3a77"]], jsonAt),
                ("agent_init", ["model": "gpt-6-luna", "session_id": "demo-session-json"], jsonAt),
                ("assistant_text", ["text": "Added `--json` to `export`: it prints one object per record, and errors still go to stderr so the output stays parseable."], ago(60 * 25.9)),
                ("result", ["subtype": "success", "is_error": false, "result": "", "duration_ms": 96_000, "num_turns": 6, "usage": ["input": 21_000, "output": 3_100], "session_id": "demo-session-json"], ago(60 * 25.8)),
                ("status", ["status": "succeeded"], ago(60 * 25.8)),
            ]),
            SeedRun(json: run("demo-json-2", "Also document it in the README", status: "succeeded", created: ago(60 * 25.6), started: ago(60 * 25.6), finished: ago(60 * 25.5), worker: "mac-mini", repo: "github.com/acme/cli", model: "gpt-6-luna", agent: "codex", extra: ["parent_id": "demo-json", "effort": "medium"]), events: [
                ("status", ["status": "running", "worker_id": "mac-mini"], ago(60 * 25.6)),
                ("assistant_text", ["text": "Documented, with an example piping the output to `jq`."], ago(60 * 25.55)),
                ("result", ["subtype": "success", "is_error": false, "result": "", "duration_ms": 41_000, "num_turns": 2, "usage": ["input": 4_100, "output": 600], "session_id": "demo-session-json"], ago(60 * 25.5)),
                ("status", ["status": "succeeded"], ago(60 * 25.5)),
            ]),
        ])

        // Earlier this week, one snoozed, one archived.
        func simple(_ id: String, _ task: String, hours: Double, machine: String, repo: String = "github.com/acme/storefront", extra: [String: JSONValue] = [:]) -> [SeedRun] {
            let at = now.addingTimeInterval(-hours * 3600)
            return [SeedRun(json: run(id, task, status: "succeeded", created: at, started: at, finished: at.addingTimeInterval(240), worker: machine, repo: repo, model: "sonnet", extra: extra), events: [
                ("status", ["status": "running", "worker_id": .string(machine)], at),
                ("assistant_text", ["text": "Done. The change is on its branch."], at.addingTimeInterval(200)),
                ("result", ["subtype": "success", "is_error": false, "result": "", "duration_ms": 240_000, "num_turns": 7, "total_cost_usd": 0.18, "session_id": .string("demo-\(id)")], at.addingTimeInterval(240)),
                ("status", ["status": "succeeded"], at.addingTimeInterval(240)),
            ])]
        }
        out.append(simple("demo-images", "Compress hero images in the marketing pages", hours: 51, machine: "hetzner-vps"))
        out.append(simple("demo-sitemap", "Generate the sitemap from the route table", hours: 76, machine: "mac-mini", repo: "github.com/acme/docs"))
        out.append(simple("demo-review", "Summarise what changed on main this week", hours: 30, machine: "mac-mini", extra: ["snoozed_until": iso(now.addingTimeInterval(3 * 3600))]))
        out.append(simple("demo-old", "Remove the old feature flag for guest checkout", hours: 200, machine: "hetzner-vps", extra: ["archived_at": iso(now.addingTimeInterval(-100 * 3600))]))
        return out
    }

    /// The running task keeps writing, slowly enough to watch.
    private static func liveWriting() -> [DemoStep] {
        let text = "The guide is drafted. Each break gets a before and after, starting with pagination:\n\n```ts\n// v2\nconst items = await client.fetch('/items', { page: 2 });\n\n// v3\nconst { data, nextCursor } = await client.items.list({ cursor });\n```\n\nNext I'll check every snippet against the v3 types, so nothing in the guide fails to compile."
        var steps: [DemoStep] = [DemoStep(4, "assistant_thinking")]
        var written = ""
        for (i, word) in text.split(separator: " ", omittingEmptySubsequences: false).enumerated() {
            written += (i == 0 ? "" : " ") + word
            steps.append(DemoStep(0.14, "assistant_live", ["text": .string(written)]))
        }
        steps.append(DemoStep(0.4, "assistant_text", ["text": .string(text)]))
        steps.append(DemoStep(1, "tool_use", ["id": "g4", "name": "Bash", "summary": "pnpm typecheck docs/guides/snippets"]))
        // Long enough that the task is still going whenever you look.
        steps.append(DemoStep(600, "tool_result", ["tool_use_id": "g4", "content": "No type errors", "is_error": false]))
        return steps
    }

    /// What any task you start in the demo does.
    static func followUpScript(machine: String) -> [DemoStep] {
        let text = "This is the demo, so no agent is really working: the machines and tasks are made up. In real use, this is where the agent's narration streams in as it reads and edits your code."
        var steps: [DemoStep] = [
            DemoStep(0.9, "status", ["status": "running", "worker_id": .string(machine)]),
            DemoStep(0.2, "workspace", ["mode": "worktree", "branch": "sidefleet/demo0001", "from": ["ref": "origin/main", "commit": "e03b19d4c1"]]),
            DemoStep(0.3, "agent_init", ["model": "claude-sonnet-5-5", "session_id": "demo-session-new"]),
            DemoStep(0.8, "tool_use", ["id": "n1", "name": "Read", "summary": "README.md"]),
            DemoStep(0.8, "tool_result", ["tool_use_id": "n1", "content": "# Acme", "is_error": false]),
        ]
        var written = ""
        for (i, word) in text.split(separator: " ").enumerated() {
            written += (i == 0 ? "" : " ") + word
            steps.append(DemoStep(0.05, "assistant_live", ["text": .string(written)]))
        }
        steps.append(DemoStep(0.3, "assistant_text", ["text": .string(text)]))
        steps.append(DemoStep(0.4, "result", ["subtype": "success", "is_error": false, "result": "", "duration_ms": 8_000, "num_turns": 2, "total_cost_usd": 0.01, "session_id": "demo-session-new", "context": ["used": 12_000, "limit": 200_000]]))
        steps.append(DemoStep(0.1, "status", ["status": "succeeded"]))
        return steps
    }

    static func afterApproval(approved: Bool) -> [DemoStep] {
        if !approved {
            return [
                DemoStep(1.2, "assistant_text", ["text": "Understood, I won't run them. The limiter is in place, untested; you can run `pnpm test src/routes/webhooks.test.ts` yourself."]),
                DemoStep(0.4, "result", ["subtype": "success", "is_error": false, "result": "", "duration_ms": 512_000, "num_turns": 9, "total_cost_usd": 0.74, "session_id": "demo-session-rate"]),
                DemoStep(0.1, "status", ["status": "succeeded"]),
            ]
        }
        return [
            DemoStep(0.6, "tool_use", ["id": "r3", "name": "Bash", "summary": "pnpm test src/routes/webhooks.test.ts"]),
            DemoStep(2.4, "tool_result", ["tool_use_id": "r3", "content": "✓ 14 tests passed (2.1s)", "is_error": false]),
            DemoStep(0.8, "assistant_text", ["text": "All 14 pass, including a burst of 50 within the limit and the 51st getting a `429` with `Retry-After`."]),
            DemoStep(0.4, "diff", ["turn": ["files": [diffFile("src/routes/webhooks.ts", add: 3, del: 0, patch: "@@ -1,6 +1,8 @@\n import { verify } from '$lib/signing';\n+import { TokenBucket } from '$lib/limits';\n+\n+const buckets = new Map<string, TokenBucket>();\n \n export async function POST(req) {\n   const secret = await verify(req);\n")], "file_count": 1, "additions": 3, "deletions": 0]]),
            DemoStep(0.3, "result", ["subtype": "success", "is_error": false, "result": "", "duration_ms": 540_000, "num_turns": 11, "total_cost_usd": 0.92, "session_id": "demo-session-rate", "context": ["used": 164_000, "limit": 200_000]]),
            DemoStep(0.1, "status", ["status": "succeeded"]),
        ]
    }
}
