# 05 · The fleet board and conflict warnings

Phase 3. This is the screen that only Sidefleet can build: every task's worktree is visible to
the control plane while it's being written, not only once it's a PR.

## 1. The board

A new page, `/board`, and the default home for team orgs once it ships. Personal orgs keep
today's home.

**Columns by state** (the default view):

| Queued | Working | Waiting on someone | In review | Merged today |
|---|---|---|---|---|
| queued runs | running | finished and waiting on the driver, approvals, suggestions | open PR | merged in the last 24 h |

**Or rows by repo** (a toggle), with the same cards in a horizontal lane per repo. This is better on desktop
when a team works across several repos.

**A card**:
```
Fix checkout rounding                    ⚠ overlaps with Ana
[avatar] Mark → driving · MacBook-Pro · Claude Opus
src/cart.rs  src/money.rs  +2 more        +84 −12
● Running 4m · ran tests → 1248 passed
```
- The title, driver, machine, agent and model.
- **Files touched**: from the thread's latest `diff` event (`total.files[].path`, which the worker
  already reports each turn; see crates/worker/src/diff.rs).
- **Last meaningful step**, written as verb, object and outcome (see §4).
- Status pill, time in state, PR checks if any, and the waiting reason in the "Waiting" column.

**Filters**: Mine · People · Repos · Machines · Agents. Private threads appear only to those who can
see them (03 §2), and to admins as "Private task by Sam" cards without files.

**Live**: one SSE stream, `GET /v1/board/events`, sends card upserts. It's fed by the same event bus
(`EventBus::subscribe_all`) the inbox already watches, filtered per viewer.

**Phone**: the columns become a segmented list (Working · Waiting · Review), and a card opens
the thread.

### API

`GET /v1/board?group=state|repo&repo=&user=` → `{cards: [...]}`. Each card is built from the
thread's root and latest run, the cached file list (§3), and the PR summary from 04.

## 2. Conflict warnings

Two threads touching the same files in the same repo will conflict at merge. Today you find out
at merge time. The board finds out while both are still being written.

**What counts:**
- **Active threads**: in the same repo, not archived and not merged, and their latest run
  is queued or running, or they have an open PR.
- **File overlap**: the same path in both threads' `total.files`. Renames count by both their old and
  new path.
- **Hunk overlap** (stronger): the same path *and* overlapping line ranges in the patches. The `diff`
  event carries the patches, truncated for large files, so this check is best effort.

| Level | Shown as |
|---|---|
| Same file, different places | "Also editing `cart.rs`: Ana's *Add coupons*", in yellow |
| Overlapping lines | "Likely conflict in `cart.rs` with Ana's *Add coupons*", in orange |
| Other thread already merged to main | "`cart.rs` changed on main since this started: rebase?", in orange |

**Where it shows:**
- A badge on both cards on the board, and a banner in both threads.
- The drivers of both threads get **one** inbox item per pair when the overlap first appears (reason
  `Conflict`), not on every turn.
- In the composer, when you start a task in a repo with active threads: "3 tasks are working in
  acme/shop right now", which expands to show their files. That helps decide whether to start
  this now.

**What you can do from the banner:**
- **Rebase now**: a reply to the agent: "main (or Ana's branch) changed `cart.rs`: fetch and rebase
  onto origin/main, resolve conflicts, keep both behaviours." It's only enabled after the other
  thread merged.
- **Ask Ana**: a note in both threads mentioning each driver.
- **Stack on Ana's**: fork this thread onto Ana's branch instead of main (phase 4, see 07 §3).
- **Dismiss** for this pair.

### Computation

```sql
CREATE TABLE thread_files (
    root_id    uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    org_id     uuid NOT NULL,
    repo       text NOT NULL,
    path       text NOT NULL,
    ranges     int4range[] NOT NULL DEFAULT '{}',   -- changed line ranges, new side
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (root_id, path)
);
CREATE INDEX thread_files_repo_path_idx ON thread_files (org_id, repo, path);

CREATE TABLE thread_conflicts (
    org_id    uuid NOT NULL,
    a_root    uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    b_root    uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,  -- a_root < b_root
    level     text NOT NULL CHECK (level IN ('file', 'lines', 'main')),
    paths     text[] NOT NULL,
    first_at  timestamptz NOT NULL,
    dismissed_by uuid[] NOT NULL DEFAULT '{}',
    PRIMARY KEY (a_root, b_root)
);
```

- When a `diff` event is appended (`append_events`), replace that thread's `thread_files` rows
  from `total.files`, then look up overlaps with one indexed query on `(org_id, repo, path)`
  against other active threads. Then upsert `thread_conflicts`. The cost scales with the number
  of files in a turn, not with the number of threads.
- **"Changed on main"**: when a thread merges (04 §3), its `thread_files` are checked against
  other active threads in the repo at that moment, which yields `level = 'main'`. No polling of main is
  needed for changes merged through Sidefleet. Changes merged elsewhere are caught in phase 2b
  through the GitHub App's `push` webhook.
- Threads leave the conflict tables when they merge, close or are archived.

## 3. Per-person view

`/board?user=sam`, or tap a teammate's avatar anywhere. It shows what Sam's agents are doing, what's
waiting on Sam, and what Sam merged this week. The view is for catching up, not monitoring: it's the same
data as the board, and it shows no hours, keystrokes or "activity score".

## 4. Making the step line readable

The card's last-step line and a future team feed both need agent activity turned into
sentences. Buzz's activity-feed principles are the guide: verb, object and outcome; failures rise
and reads recede; a running step updates in place; silence is a state ("waiting 3m"), never a blank.
Most of this is in the thread view already. The board needs one server-side helper,
`summary::step_line(&RunEvent) -> Option<String>`, so the board, chat updates and push all use the
same wording.

## Tests

- Two threads in the same repo editing `a.rs`: one `file` conflict row, one inbox item per
  driver, and no duplicates on the next turn.
- Overlapping ranges escalate the row to `lines`. Dismissing hides it for that user only.
- Merging thread A creates a `main` conflict for active thread B with overlapping files.
- Private threads never appear in another member's board or conflict banner, by title or path.
  The conflict is still shown to the owner of the visible thread as "a private task".
