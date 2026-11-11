import Foundation

// The demo's worktrees: what Clean up on the Machines screen finds. Nothing here is real.
extension DemoData {
    /// Every worktree on a machine, with why a cleanup would remove or keep it.
    static func worktrees(worker: String) -> [JSONValue] {
        func item(_ root: String, _ task: String, repo: String, state: String, gb: Double) -> JSONValue {
            ["root_id": .string(root), "repo": .string(repo), "branch": .string("sidefleet/\(root.dropFirst(5))"), "state": .string(state), "bytes": .number(gb * 1e9), "task": .string(task)]
        }
        if worker == "hetzner-vps" {
            return [
                item("demo-images", "Compress hero images in the marketing pages", repo: "storefront", state: "merged", gb: 1.4),
                item("demo-old", "Remove the old feature flag for guest checkout", repo: "storefront", state: "on_default", gb: 0.9),
                item("demo-rate", "Add rate limiting to the public webhook endpoint", repo: "storefront", state: "in_use", gb: 1.1),
                item("demo-stripe", "Upgrade Stripe SDK to v18 and fix the breaking changes", repo: "storefront", state: "changes", gb: 1.6),
            ]
        }
        return [
            item("demo-json", "Add a `--json` flag to the `export` command", repo: "cli", state: "merged", gb: 0.42),
            item("demo-sitemap", "Generate the sitemap from the route table", repo: "docs", state: "on_default", gb: 0.21),
            item("demo-checkout", "The checkout e2e test is flaky on CI. Find out why and fix it", repo: "storefront", state: "unmerged", gb: 1.3),
            item("demo-guide", "Write a migration guide for the v3 API", repo: "docs", state: "in_use", gb: 0.2),
        ]
    }

    /// What a cleanup reports after removing `items`: the totals everyone sees, and the list for the owner.
    static func cleanup(removed items: [JSONValue], now: Date) -> JSONValue {
        [
            "removed": .number(Double(items.count)),
            "freed_bytes": .number(items.reduce(0) { $0 + ($1["bytes"]?.double ?? 0) }),
            "kept_in_use": 0, "kept_changes": 0, "kept_unmerged": 0, "failed": 0,
            "items": .array(items),
            "at": .string(Timestamp.format(now)),
        ]
    }
}
