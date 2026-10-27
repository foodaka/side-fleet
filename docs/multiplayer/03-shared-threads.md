# 03 · Shared threads

Phase 1: §1 attribution, §3 who can do what, §5 "Continue on my machine", §8 the scoped inbox.
Phase 2: §2 visibility, §4 presence, the rest of §5, §6 notes, §7 mentions.

A thread is one person's agent working on one thing, on that person's machine. Everyone in the
team can watch it, and in phase 2 talk in it and suggest to it. Only its creator, the **driver**,
tells its agent what to do. When several people work toward one goal, they each run their own
threads inside a **room** ([09](09-rooms.md)), rather than sharing one thread.

## 1. Attribution

```sql
ALTER TABLE runs ADD COLUMN created_by uuid REFERENCES users(id) ON DELETE SET NULL;
```

- Set on every run from `Caller::user_id()`: new tasks, replies, forks, restores, and runs moved
  to another machine, which keep the moved run's value. API-token runs get the token's user
  (`api_tokens.user_id`). Developer key: `NULL`.
- The root's `created_by` is the thread's **creator**. A reply's `created_by` is who it acts for,
  which is always the thread's creator (§3), and it's the identity the claim check in
  [02](02-machines.md) §3 uses.
- Existing runs: in personal orgs, set to the only member. In team orgs, leave `NULL`, shown as
  "Someone".
- `approvals.decided_by` (text today) gets a sibling `decided_by_user uuid`. `enrollments.decided_by` is already a user.

The page shows avatars (GitHub `https://github.com/<login>.png?size=64`, cached by the browser) in:
- **The sidebar and thread list**: the creator's avatar.
- **The thread**: on every human message, so you can see who said what to the agent.
- **The thread header**: the creator, their machine, and watchers (§4).

`GET /v1/runs` and `/thread` return `created_by: {id, login}` for each run. `GET /v1/runs` gains
`?mine=1`, `?by=<login>`, `?following=1`.

## 2. Visibility

```sql
ALTER TABLE runs ADD COLUMN visibility text NOT NULL DEFAULT 'team' CHECK (visibility IN ('team', 'private'));
```

Visibility is set on the root and inherited by the thread. Forks get their own (default `team`).

- **Team** (default in team orgs): every member sees the thread, its events, its diff and its PR.
- **Private**: only the creator sees it. Others see nothing. It isn't listed, and links to it are 404.
  Admins see that it exists (title hidden as "Private task by Sam", status, machine) so the board
  and limits make sense. Content is hidden from them.
- Personal orgs are unaffected: one member sees everything.

There's a toggle in the composer (a lock icon) and in the thread menu. Going private → team
is allowed any time. Going team → private is allowed for the creator, and it hides the thread from
people who may already have read it, which the confirmation states.

Every endpoint that resolves a run by id (`get_run`, `/thread`, `/events`, `/summary`,
`/approvals`, images, `/messages`, stop, restore, fork, pr, archive) goes through one
`visible_run(caller, id)` helper that applies org scoping (as today) and then visibility.
A hidden run gets the same 404 as another org's run. The inbox, MCP tools and chat updates use the same helper.

## 3. Who can do what in a thread

| Action | Who |
|---|---|
| Read the thread, events, diff | anyone who can see it (§2) |
| Post a **note** (§6) | anyone who can see it |
| **Reply to the agent** | the driver (its creator) only. For others it becomes a suggestion (§5) |
| Approve or deny a **tool call** | the driver only |
| Stop | the driver, org owners (a stop is always safe) |
| Restore, open a PR, add to the room ([09](09-rooms.md)) | the driver |
| Fork / continue on my machine | anyone who can see it |
| Archive, change visibility | the driver, org owners |

The rule behind the table: **you make your own agent do things on your own machine.** For
everyone else's agents you suggest, and their owner decides.

## 4. Presence

People in the thread show as avatars in the header, labelled "Watching" (live SSE connected) or
"Typing…".

- Presence is in memory on the control plane: `HashMap<root_id, HashMap<user_id, last_seen>>`,
  updated while a user holds the thread's SSE stream open, and expired 30 seconds after it closes.
- The thread SSE stream gains a `presence` event, `{users: [{id, login, typing}]}`, sent on change.
- `POST /v1/runs/:id/typing` (debounced by the page to one call every 3 seconds) marks typing for 5 seconds.
- Single instance only, like the claim signal (`ClaimSignal`). If the control plane ever runs
  several instances, this moves to Postgres `LISTEN/NOTIFY`, the same as the claim signal would.

## 5. Suggestions, continuing and handing off

### Suggestions (phase 2)

When someone other than the driver writes in the reply box, the button reads **Suggest**. Their
message shows in the thread as a card, "Sam suggests: *also handle refunds*", with **Send**,
**Edit & send** and **Dismiss** for the driver. A sent suggestion is an ordinary reply from the
driver, so it runs on the driver's machine, and it shows Sam as its author ("Sam, sent by Mark").

```sql
ALTER TABLE runs ADD COLUMN authored_by uuid REFERENCES users(id) ON DELETE SET NULL; -- when it differs from created_by
CREATE TABLE suggestions (
    id         uuid PRIMARY KEY,
    org_id     uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    root_id    uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    author_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    body       text NOT NULL,
    status     text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'sent', 'dismissed')),
    run_id     uuid REFERENCES runs(id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    decided_at timestamptz
);
```

### Continue on my machine (phase 1, built)

On any thread you can see, **Continue on my machine** forks it from its latest finished
message into a new thread of your own, on one of your machines. The fork carries a transcript of
the conversation. `create_fork` only pins a fork to the machine with the files when that machine
is the forker's own, so a fork of a teammate's thread starts from your checkout plus the
transcript, without their changes.

To bring the files along too, team orgs should publish thread branches. After each message, the
worker pushes `sidefleet/<id>` to origin, using the failover checkpoints' code, and the fork
continues from it with `resume_from_branch`. That's phase 2. In a room, finished work already
travels through the room branch ([09](09-rooms.md)), which covers the common case.

### Handoff (phase 2)

**Pass to…** (driver only) sends a teammate "Mark passed you *Fix checkout rounding*" with a
summary (summary.rs) and **Continue on my machine** / **Decline**. Nothing runs until they
accept, and then it runs on their machine with their agent sign-in.

## 6. Notes

A **note** is a message to the people in the thread, not to the agent: "I think this is the
wrong approach, see #123". It's the team's discussion layer, so you don't need a separate chat
tool.

```sql
CREATE TABLE thread_notes (
    id         uuid PRIMARY KEY,
    org_id     uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    root_id    uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    author_id  uuid REFERENCES users(id) ON DELETE SET NULL,
    body       text NOT NULL,
    reply_to   uuid REFERENCES thread_notes(id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    edited_at  timestamptz,
    deleted_at timestamptz
);
CREATE INDEX thread_notes_root_idx ON thread_notes (root_id, created_at);
```

- The reply box has a mode switch: **To my agent** / **Note to team**. Before you've joined, it
  shows **Join** / **Note**.
- Notes render inline in the timeline, by time, visually distinct (no agent avatar, a muted
  background).
- They are delivered over the thread SSE stream as a `note` event.
- Agents see notes through the team update (§5), so a note like "don't touch the payments
  schema" reaches every lane's next turn.
- Emoji reactions on notes, on agent messages and on suggestions are phase 2 polish. A 👍 from
  the lane's owner on a tool-approval card approves it, which is quick from a phone.

## 7. Mentions

`@login` in a note, a suggestion or a reply mentions a member. The server parses mentions against
`org_members` and stores them:

```sql
CREATE TABLE mentions (
    id         uuid PRIMARY KEY,
    org_id     uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    root_id    uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    by_user    uuid REFERENCES users(id) ON DELETE SET NULL,
    source     text NOT NULL CHECK (source IN ('note', 'suggestion', 'reply')),
    source_id  uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
```

- A mention makes the person a follower (§8) and puts an inbox item and a push notification in front
  of them: "Mark mentioned you in *Fix checkout rounding*".
- In a reply to the agent the `@login` text is sent to the agent unchanged, which is harmless and
  sometimes useful ("ask @sam's earlier thread…").
- Mentioning someone who can't see a private thread: the composer warns, and on send nothing is
  delivered to them.
- The composer autocompletes `@` from the member list.

## 8. A scoped inbox

Today `inbox::list` returns every waiting thread of the org to every viewer. In a team, that's
everyone's finished tasks in everyone's bell.

```sql
CREATE TABLE thread_followers (
    root_id  uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    user_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    reason   text NOT NULL CHECK (reason IN ('creator', 'mentioned', 'noted', 'handoff', 'manual')),
    muted    boolean NOT NULL DEFAULT false,
    since    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (root_id, user_id)
);
```

You follow a thread when you created it, are mentioned in it, wrote a suggestion or note in it,
were handed it, or pressed **Follow**. **Mute** keeps you following but stops inbox items.

The inbox shows:
- **Your threads waiting** for the existing reasons (`Finished`, `Failed`, `UsageLimit`,
  `MachineLost`, `ApprovalNeeded`). **Built in phase 1**: `inbox::list` and push recipients only
  include a thread for its creator. Other followers get these as feed items, not inbox items.
- **New reasons** (appended to `attention::Reason`, as the enum's comment asks): `Mentioned`,
  `HandedToYou` and `SuggestionWaiting`.

Personal orgs: unchanged, because there's one member.

Push notifications (push.rs) follow the inbox. Chat updates (chat_updates.rs) go to the
token's user only for threads they follow. Today they go for threads the token started.

## 9. Approvals while you're away

"Ask me" approvals wait on the driver, and only the driver can decide them, because the tool
runs on their machine. Teammates watching see the pending approval read-only ("Waiting for Mark to
approve `pnpm test`"), and can nudge with a note that mentions Mark. If the driver hasn't decided
within 5 minutes, they get one reminder push.

## 10. The page

- **Sidebar**: **Mine** / **Team** filter, and the creator's avatar on each thread (built).
- **Thread header**: the creator. For others, the reply box is replaced by "This is @sam's task
  and it runs on their machine" and **Continue on my machine** (built). Restore, PR, approve and
  stop are hidden for them.
- **Room link**: a thread in a room links back to it ([09](09-rooms.md)).
- Phase 2: watchers, notes and suggestions inline, Follow / Mute, the visibility lock.

## Tests

- Built: a non-creator's reply, restore, PR and integrate are refused (403, naming the creator), and
  stop too unless they're an org owner. A fork is the forker's and never lands on the creator's
  machine. A finished thread is in its creator's inbox only (crates/server/tests/multiplayer.rs,
  inbox.rs).
- Phase 2: `visible_run` makes a private thread 404 for other members everywhere. A sent
  suggestion runs as the driver. Presence expires after the stream closes.
