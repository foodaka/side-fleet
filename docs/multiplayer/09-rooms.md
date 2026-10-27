# 09 · Rooms: working together toward one goal

Phase 1. The core of the first build: see IMPLEMENTATION.md for exactly what shipped.

## Why rooms, not shared threads

The obvious version of multiplayer is two people's agents in one thread on one branch. We
designed it ("lanes") and set it aside:
- Two agents editing one branch at the same time pull each other's half-finished work mid-task, see
  files change under them, and hit conflicts constantly.
- An interleaved timeline of several agents is hard to follow, especially on a phone.
- It skips the part that matters: agreeing on the goal, splitting the work, and putting it back
  together.

Human teams don't share one branch either. They work on separate branches and integrate. Rooms do
the same, with agents: **people talk in the room, and agents work in tasks.**

## The model

- **A room** is a goal in one repo: a name, a goal, the team's conversation (notes), and the tasks
  under it. Any member can create one.
- **A task** is an ordinary thread (03) with `room_id` set. It runs on its creator's machine in its
  own worktree and branch. Only its creator steers it, and anyone can watch it or continue it on
  their own machine.
- **The room branch**, `sidefleet/room-<8 hex>`, is where finished work comes together. A task's
  owner presses **Add to room**, which queues an `integrate` run on the task's machine. It merges
  the task's branch into the room branch and pushes, never force-pushing, retrying if someone
  pushed first, and aborting with the conflicted files on a conflict.
- **New tasks start from the room branch** once anything has been merged into it
  (`resume_from_branch`, the same mechanism failover uses). So the second task builds on the first.
- **One PR at the end**: the room shows **Compare on GitHub** for the room branch. Opening the PR
  from the room in one tap comes later.

## Shared context: the room brief

Every task's agent knows about the room. The server writes it into `runs.context`, which workers
already put ahead of the message:
- **Before a task's first message**: what the room is and which branch it merges into, the goal,
  the other tasks (owner, status, files touched, whether they're merged), and the team's recent notes.
- **Before each reply**: a short **room update** with the other tasks' current state and the
  latest notes, without the goal.

It's capped at 12 KB. The cards and brief come from `rooms::tasks` and `rooms::brief`
(crates/server/src/rooms.rs). The files come from each thread's latest `diff` event, which
workers already send.

## Conflicts

When **Add to room** hits a conflict, the task card shows the conflicted files and **Ask my agent
to fix**. That sends a reply asking the agent to merge `origin/<room branch>` into its branch,
resolve the conflicts keeping both behaviours, and stop. The owner then presses Add to room again.
Conflicts surface early, at the point of integration, and they always go to the owner of the
task, who has the context to resolve them.

## The page

- `#/rooms`: the list (name, goal, repo, counts, last activity) and **New room**.
- `#/rooms/<id>`, two columns (stacked on a phone):
  - **Tasks**: cards with title, owner, agent, status, files with additions and deletions, and
    integration state. They have **Add to room** (your own finished tasks), conflict help, and
    a **New task in this room** composer, limited to your machines.
  - **Conversation**: the team's notes.
  - **Header**: the goal (editable by its creator), repo, branch, and Compare on GitHub.
- The page polls every 3 seconds while it's open. SSE comes later.
- A thread in a room links back to it, and its `integrate` runs show as "Added to room" with the
  commit, or the conflicts.

## API and data

See IMPLEMENTATION.md for the endpoints and payloads, and migration
`0021_multiplayer.sql` for `rooms`, `room_notes` and `runs.room_id`. The worker side is
crates/worker/src/integrate.rs.

## Next

- **Plan → tasks**: write the goal, and a planning run proposes tasks to split it into. The room
  becomes the plan's checklist (from 07 §2).
- **Open the room PR** from the page, and show its checks in the room (04).
- **Conflict warnings** before integration: tasks in a room touching the same files (05 §2). The
  room is the natural place for them.
- **Mentions and push** for room notes (03 §7).
- **SSE** for the room page, in place of polling.
