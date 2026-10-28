# Multiplayer Sidefleet

**Your team's agents in one live room. Everyone sees the work and can help steer it. Each
person's machines run only their own tasks.**

Status: proposal. Nothing here is built yet unless a section says so.

## Why

Sidefleet today is one person driving many agents. Teams already put several people in one org
(`orgs`, `org_members`, the org switcher), but the product still behaves as if one person is
there: tasks don't record who started them, every member's inbox shows every waiting task, and any
member can run anything, full access included, on any machine in the org. Multiplayer turns the org into a team
workspace where the work is shared and every machine runs only its owner's tasks.

## Principles

1. **Work is shared, machines are not.** Joining a team shows you the team's threads, diffs and
   PRs. A machine only ever runs its owner's tasks, under its owner's agent sign-in and GitHub
   login. To take over someone's work, you continue it on your own machine. Sharing machines is
   deferred; see [02](02-machines.md) §7.
2. **Every action has a person behind it.** Tasks, replies, approvals, stops and merges record who
   did them. An agent acts on behalf of the person who drives its thread, with no more power than
   they have.
3. **The thread is the unit of work.** One thread is one branch and, later, one PR. Discussion, review,
   CI and merge happen in it, so the thread is the record of why the code exists. See [04](04-thread-is-the-pr.md).
4. **Quiet by default.** Seeing everything doesn't mean being notified of everything. Your inbox
   holds what you drive, what you're mentioned in, and what needs your decision.
5. **Phone-first.** Every decision (approve a machine request, approve a tool call, hand off,
   merge) works in one tap from a push notification.
6. **Self-hosting keeps working.** The developer key and static worker tokens act with no user.
   Everything here degrades to today's behaviour for them (see "Self-hosted" below).

## The sections

| # | Section | Phase |
|---|---|---|
| [IMPLEMENTATION](IMPLEMENTATION.md) | **What the first build ships**: the API contract for server, worker and web | built |
| [01](01-members.md) | Members, roles and invites | 1 (invites, remove: built) |
| [02](02-machines.md) | Machines: each runs only its owner's tasks (sharing deferred) | 1 (built) |
| [03](03-shared-threads.md) | Shared threads: attribution, continue on my machine, a scoped inbox; later presence, notes, mentions, suggestions, handoff | 1 (built) · 2 |
| [09](09-rooms.md) | **Rooms**: a goal, the team's notes, tasks on each person's machine, merged into one room branch | 1 (built) |
| [04](04-thread-is-the-pr.md) | The thread is the PR: CI, reviews and merge in the thread | 2 |
| [05](05-fleet-board.md) | The fleet board and conflict warnings | 3 |
| [06](06-team-memory.md) | Team memory: search with receipts, agents that read past threads | 3 |
| [07](07-automations-and-plans.md) | Automations and races (plan fan-out moved into rooms) | 4 |
| [08](08-audit-and-limits.md) | Audit log, plans and limits | 2 |

## Phases

**Phase 1: invite someone and build together (built).**
- Invite links ([01](01-members.md)).
- Machine owners and owner-only claims ([02](02-machines.md)).
- `runs.created_by`, avatars, Mine / Team, an inbox scoped to you, **Continue on my machine**
  ([03](03-shared-threads.md)).
- **Rooms** ([09](09-rooms.md)).

**Phase 2: it feels multiplayer.**
- Presence, notes, @mentions, suggestions and handoff in threads ([03](03-shared-threads.md)).
- Plan → tasks in rooms, the room PR, published thread branches ([09](09-rooms.md), [03](03-shared-threads.md) §5).
- PR status, CI, reviews and merge in the thread ([04](04-thread-is-the-pr.md)).
- Audit log, per-member limits, admin role ([08](08-audit-and-limits.md), [01](01-members.md)).

**Phase 3: it's faster than working alone.**
- The fleet board with conflict warnings ([05](05-fleet-board.md)).
- Team search, and the `search_threads` tool for agents ([06](06-team-memory.md)).

**Phase 4: the fleet works while the team sleeps.**
- Automations and races ([07](07-automations-and-plans.md)).

## Glossary

- **Org / team**: an `orgs` row. A *personal* org has one member. A *team* org has several.
- **Thread / task**: a root run and its replies (`runs.parent_id`). Identified by the root's id.
- **Creator**: the user who started the thread (`runs.created_by` on the root).
- **Driver**: the thread's creator, the only person whose messages go to its agent.
- **Room**: a goal the team works on together; its tasks are threads merged into one room branch
  (see [09](09-rooms.md)).
- **Machine owner**: the user a machine belongs to (`workers.owner_id`). Null only for
  self-hosted static-token machines.
- **Follower**: someone who gets inbox items and push notifications for a thread.

## Self-hosted and the developer key

The developer key, static worker tokens and API tokens minted with it have no user:
- Runs they start have `created_by = NULL`, and machines connected with a static token have
  `owner_id = NULL`. "Owner matches" (null = null) keeps them working together exactly as today.
- The default org has no members, so a self-hosted install behaves exactly as today. Self-hosters
  who want multiplayer turn on GitHub sign-in (HOSTED.md) and invite people. Nothing else changes.

## Open questions (whole project)

1. **When to allow sharing machines.** Deferred for now ([02](02-machines.md) §7). Reviving it
   needs answers for agent sign-ins (a personal subscription shouldn't serve teammates), PRs as
   the driver rather than the machine owner, and isolation on personal machines.
2. **GitHub identity.** PRs and pushes use the machine's `gh` and git login. With owner-only
   machines, that's always the driver's own login, so this is fine for now. A GitHub App
   ([04](04-thread-is-the-pr.md) §6) is still worth it for webhooks instead of polling.
3. **One machine in several orgs.** Done for its owner: a machine serves its owner in every org
   they belong to ([02](02-machines.md) §6). Lending a machine to other people is still out of
   scope (§7 there).
