# 04 · The thread is the PR

Phase 2. Borrowed from Buzz's "branch as room": the room where a change is discussed, reviewed,
tested and merged is the same room, and afterwards it is the record of why the code exists.

## Today

- A `pr` run (crates/worker/src/pr.rs) commits the thread's worktree, pushes its `sidefleet/<id>`
  branch and opens a PR with `gh`, **signed in as whoever runs the worker**.
- `runs.pr` stores `{url, number, state}`. The worker that opened it (`runs.pr_worker`) re-checks
  open PRs from time to time (`PrCheck` → `POST /v1/worker/prs`), so the sidebar shows open, merged or closed.
- Review comments, CI results and approvals live only on GitHub.

## 1. What the thread shows

A **PR panel** at the top of a thread that has a PR (it collapses to a chip on phones):

```
#412 Fix checkout rounding                      Open · opened as @mark
✓ 11 checks   ✗ 1 failing: e2e (chromium)       [Fix it]
Reviews: ✓ ana approved · 💬 sam commented (2)
Mergeable · squash                              [Merge]
```

And **PR events inline in the timeline**, as they happen:
- `pr_opened`, `pr_merged`, `pr_closed`, `pr_reopened`
- `checks_finished {passed, failed: [{name, url, summary}]}`. Only when a run of checks
  completes, never one event per check.
- `review {author, state: approved|changes_requested|commented, body, comments: [{path, line, body}]}`
- `comment {author, body}` for PR conversation comments
- `pushed {by, commits}` for pushes to the branch from outside Sidefleet (someone fixing it locally)

These are thread-level events, not agent output, so they go in their own table
rather than `run_events`, which belongs to a run and its agent:

```sql
CREATE TABLE thread_events (
    id         uuid PRIMARY KEY,
    org_id     uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    root_id    uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    kind       text NOT NULL,
    payload    jsonb NOT NULL,
    dedupe_key text,                      -- e.g. "review:123456"; unique per thread
    at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE (root_id, dedupe_key)
);
```

Notes (03 §6), handoffs and "continued on" links (03 §5) can share this table. It's one timeline of
everything that isn't the agent.

GitHub text (review bodies, comments) is untrusted input. It's stored and rendered as plain text
or sanitized markdown, and it reaches the agent only when the driver sends it (§2).

## 2. Acting on it from the thread

Every action is one tap and produces a normal reply from the driver, so they see exactly what
the agent is told, and it runs on their machine. Others get **Suggest** instead (03 §5).

- **Fix it** (on failing checks): queues a reply. "CI failed on `e2e (chromium)`. Here is the failing
  output: … Fix it and push." The log tail (last 200 lines, redacted the same way worker output
  is) comes from `gh run view --log-failed` on the worker. The agent pushes, and the PR updates.
- **Address review** (on a `changes_requested` review): queues a reply with the review's body and
  inline comments ("`src/cart.rs:45` (sam): handle the empty cart"). Like **Fix it**, it's for the driver.
- **Reply on GitHub**: from a note, an option "also post to the PR" posts it as a PR comment
  through your own machine's `gh`, so it's posted as you. Off by default.
- **Merge**: the driver, or org owners. It shows the repo's allowed methods (`gh repo view --json
  mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed`) and is disabled with the reason when GitHub says
  it isn't mergeable. The merge runs as a new run kind, `merge`, on the PR's worker
  (`gh pr merge --squash --delete-branch`), so it uses the same GitHub identity that opened it.
- **Auto-fix CI** (a thread setting, off by default): when checks fail, queue "Fix it" by itself,
  at most twice per PR, then hand it to the driver. It runs on
  their machine, as any reply would.

## 3. After merge

- The thread is marked **Merged** and archived from the active sidebar after 24 hours, but stays
  searchable ([06](06-team-memory.md)) as the record: the task, the conversation, the notes, the review and the
  checks.
- The worktree is removed on the machine (the branch is already deleted by `--delete-branch`).
  This needs a `cleanup` run kind, which the worker already partly has for archiving. Verify in
  worker.rs before building.
- An event, `merged {by, method, sha}`, is the last thing in the timeline.

## 4. Getting the data: phase 2a, from the worker

Extend the existing re-check rather than add infrastructure:

- `PrCheck` gains a `since` cursor per thread. The worker runs
  `gh pr view <url> --json state,mergeable,mergeStateStatus,reviewDecision,reviews,comments,statusCheckRollup,commits`
  and reports a **diff** since the cursor: new reviews, comments and commits, plus check-suite results
  that completed.
- `POST /v1/worker/prs` accepts the richer report, upserts `thread_events` by `dedupe_key`, and
  updates `runs.pr` with `{checks: {passed, failed, pending}, review_decision, mergeable}` for the
  panel.
- How often: every 30 seconds while someone is watching the thread (presence, 03 §4), every 2
  minutes while the PR is open and has pending checks, and every 10 minutes otherwise. The server
  includes the interval with each `PrCheck`.
- The data only flows while the PR's machine is online. If it's offline, the panel says "Last
  checked 3 h ago · MacBook-Pro is offline", and another of the driver's machines that has the
  repo can take over polling (`pr_worker` is reassigned).

## 5. Inbox

New reasons for the **driver**: `ChecksFailed`, `ChangesRequested`, `ReadyToMerge` (approved and
green). Mentions on GitHub (`@login` in a review) aren't mapped to Sidefleet users in phase 2.

## 6. Phase 2b: a GitHub App on the hosted control plane

Polling through workers is good enough to start and works for self-hosters. The hosted version
should install a **Sidefleet GitHub App** on the org's repos:
- **Webhooks** (`pull_request`, `pull_request_review`, `check_suite`, `issue_comment`, `push`)
  write `thread_events` in real time, with no worker involved.
- **PRs, comments and merges that don't need a machine online**, via user-to-server tokens from
  the driver's GitHub sign-in. It's also a prerequisite for ever sharing machines (02 §7).
- **Issues → tasks** ([07](07-automations-and-plans.md)) become possible.
- Scope: `pull_requests: write`, `checks: read`, `contents: read`, `issues: write`. The GitHub
  sign-in scopes stay as they are (`read:user user:email`). The App is a separate install.

## Tests

- A changes-requested review becomes one `review` event in the thread, deduplicated across polls.
- **Fix it** queues a reply whose text contains the failing check's log tail, redacted.
- Merge is refused for anyone but the driver and when GitHub says it isn't mergeable.
- PR text containing `<script>` renders inert.
- A thread whose PR worker is offline is re-assigned to another of the driver's machines.
