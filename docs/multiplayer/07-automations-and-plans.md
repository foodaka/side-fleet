# 07 · Automations, plans and races

Phase 4. This section is a sketch. Each part should get its own detailed spec once phases 1–3
are live and real usage shows which matters most.

## 1. Automations

Tasks that start themselves.

| Trigger | Example |
|---|---|
| **Schedule** (cron, org time zone) | "Every weekday at 7:00, update dependencies in acme/shop and open a PR if the tests pass" |
| **CI failed on main** (GitHub App, 04 §6) | "Investigate and open a fix PR, and assign it to whoever merged last" |
| **Issue labelled** `sidefleet` | "Do the issue; driver = the person who labelled it" |
| **Webhook** (`POST /v1/hooks/:id` with a secret) | an error tracker alert → "Find the cause of this error" |
| **Thread merged** in repo X | "Update the docs site for this change" |

An automation has:
- a **prompt template** with variables (`{{issue.title}}`, `{{check.log_tail}}`),
- a **repo**, an **agent and model**, a **permission mode**, and optionally one of the owner's
  machines to pin to,
- an **owner**, who is the driver of every thread it starts. Its runs land on the owner's machines
  only (02), and the inbox (03 §8) works unchanged. Every automation needs an owner: "the team's
  nightly dependency bump" belongs to whoever set it up,
- **limits**: at most N open threads at once, at most N runs a day, and "skip if a thread from this
  automation is still open".

Every thread it starts is labelled with the automation, and it's paused automatically after 3 failures in
a row, with an inbox item for its owner.

```sql
CREATE TABLE automations (
    id         uuid PRIMARY KEY,
    org_id     uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    owner_id   uuid REFERENCES users(id) ON DELETE SET NULL,
    name       text NOT NULL,
    trigger    jsonb NOT NULL,   -- {kind: schedule|ci_failed|issue_label|webhook|merged, ...}
    template   jsonb NOT NULL,   -- task text, repo, agent, model, effort, permission_mode, pinned_worker_id
    limits     jsonb NOT NULL,
    paused_at  timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE runs ADD COLUMN automation_id uuid REFERENCES automations(id) ON DELETE SET NULL;
```

Chat apps (Hermes, OpenClaw) already let someone start tasks from chat. "Every morning do X"
said in chat should create an automation through the MCP tools, not a cron job inside the chat
app.

## 2. Goal → plan → fan-out

> Superseded by rooms ([09](09-rooms.md)): plan → tasks becomes a room feature. Kept for the details.

You write a goal ("Add Apple Pay to checkout") instead of a task.

1. **Plan**: a planning run (read-only, on any machine with the repo) reads the code and proposes a
   plan as structured output: tasks with titles, prompts, dependencies and suggested owners or agents.
2. **Review the plan together**: it shows as an editable checklist in a *plan thread*.
   Teammates comment with notes, the driver edits, reorders and assigns tasks to **people** (who
   then drive them) or to **agents**, which the driver drives.
3. **Start**: each task becomes a thread, linked to the plan (`runs.plan_id`). Tasks with
   dependencies wait until the tasks they depend on are merged, or **stack** on their branch (§3).
4. **Track**: the plan thread shows each task's state, using the same cards as the board, and the
   whole plan's progress. When every task has merged, the plan closes with a summary.

## 3. Stacked threads

A thread can start from **another thread's branch** instead of main. This is a fork at its latest
commit, which fork already supports, with "base = that thread". Merging the base offers to
rebase the stack onto main (as a reply to each stacked thread's agent), and their PRs target the base
branch until then. This is what lets a plan's dependent tasks start before the first one is merged.

## 4. Races

**Try with 3 agents**: one prompt, three threads (for example Claude Opus, Codex and Kimi), each on any
of the person's own machines. A **compare view** shows the three diffs side by side, with test
results and cost and time per agent. The team reacts or votes with notes, and the driver picks
one: the winner continues (PR), and the others are archived with a link to the winner. Races count
as N runs against limits, and the composer says so.

## 5. Guests

`guest` members (01 §1) can see only threads explicitly shared with them (`thread_guests(root_id,
user_id)`), can post notes and suggestions there, and can't drive or start anything. This is for a
client reviewing a feature or a contractor following one task.
