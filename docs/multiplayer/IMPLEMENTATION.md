# Multiplayer MVP: the build contract

What this first build ships, so the server, worker and web app can be built side by side. It's the
tryable slice of the spec: invites, machine owners, attribution, and rooms. Things not listed
here (presence, mentions, suggestions, the board, search, the GitHub App) come later.

## Rules

- **A machine only runs its owner's tasks.** The claim condition is
  `w.owner_id IS NOT DISTINCT FROM r.created_by`.
- `runs.created_by` is the signed-in user (or the API token's user). The developer key and static
  tokens give `NULL`, so self-hosted installs behave as before.
- Only a thread's creator may reply, restore, open a PR, integrate, or stop it (org owners may
  stop it too). Anyone in the org may **fork** any thread. A fork from someone else's thread is
  never pinned to their machine.

## API additions (all under the developer API, `X-Sidefleet-Org` as today)

### Members and invites
| | | |
|---|---|---|
| `GET /v1/orgs/:org/members` | any member | `[{id, login, role}]` (`id` is new) |
| `DELETE /v1/orgs/:org/members/:user_id` | owner (or the user themselves, to leave) | 204. Revokes the user's worker tokens in that org and stops their queued runs. You can't remove the last owner. |
| `POST /v1/orgs/:org/invites` | owner | body `{role?: "member"\|"owner", expires_in_days?: 7, max_uses?: number\|null}` → `{invite, code}`. The join link is `<origin>/#/join/<code>`. |
| `GET /v1/orgs/:org/invites` | owner | `[{id, role, created_by_login, expires_at, max_uses, uses, revoked_at}]`, pending first |
| `DELETE /v1/orgs/:org/invites/:id` | owner | revoke |
| `GET /v1/invites/:code` | **public** | `{org_name, inviter_login, role, valid, reason?}`. `reason` is `expired`, `revoked`, `used_up` or `not_found`. |
| `POST /v1/invites/:code/accept` | signed-in user (session) | `{org}` (an `OrgMembership`, like the entries in `/v1/me` `orgs`). Idempotent. |

### Runs
- `Run` (and so `RunSummary`, thread entries) gains `created_by: string|null` (a user id) and
  `room_id: string|null`.
- `POST /v1/runs/:id/messages`, `/restore`, `/pr`, `/integrate` and `/stop` by someone other than the
  creator → **403** with an explanation (stop is also allowed for org owners).
- `POST /v1/runs/:id/integrate` → 202 `{id, status}`. It queues a run of kind `integrate` in the
  thread. The thread must belong to a room and must not be running.
- A run of kind `integrate`: `context` holds the room branch, and `result` is
  `{"integrated": {"branch", "commit", "files": [..]}}` or `{"error": "...", "conflicts": [..]}`.

### Machines
- `GET /v1/workers` entries gain `owner: {id, login} | null` and `mine: boolean`.
  `mine` is true for your own machines, and also for owner-less machines when the caller has no user (the developer key).
- `DELETE /v1/workers/:id` and `POST /v1/workers/:id/update` are limited to the machine's owner and
  org owners (403 otherwise).
- Enrollment approval: any member may approve, and **the approver becomes the machine's owner**.

### Rooms
Room branch: `sidefleet/room-<first 8 hex of the room id>`.

| | | |
|---|---|---|
| `GET /v1/rooms?archived=false` | members | `[Room]`, newest activity first |
| `POST /v1/rooms` | members | `{name, goal, repo}` → `Room` |
| `GET /v1/rooms/:id` | members | `{room: Room, notes: [Note], tasks: [TaskCard]}` |
| `PATCH /v1/rooms/:id` | creator or org owner | `{name?, goal?, archived?: bool}` → `Room` |
| `POST /v1/rooms/:id/notes` | members | `{body}` → `Note` |
| `POST /v1/rooms/:id/tasks` | members | like `POST /v1/runs`, minus `repo` (the room's is used): `{task, agent?, model, effort?, permission_mode?, worker_id?, images?, isolation?}` → 202 `{id, status}` |

```ts
type Room = {
  id: string; name: string; goal: string; repo: string; branch: string;
  created_by: string | null; created_by_login: string | null;
  created_at: string; archived_at: string | null;
  integrated_at: string | null;        // first successful integration; tasks then start from the room branch
  last_activity_at: string;
  task_count: number; active_count: number; integrated_count: number;
};
type Note = { id: string; author_id: string | null; author_login: string | null; body: string; created_at: string };
type TaskCard = {
  id: string;                          // the thread's root run id (open /#/runs/<id>)
  title: string;                       // the root's task text
  created_by: string | null; created_by_login: string | null;
  agent: string; model: string; worker_id: string | null;
  status: string;                      // thread status: queued | running | succeeded | failed | stopped
  last_activity_at: string;
  files: string[]; additions: number; deletions: number;   // from the thread's latest diff event
  integration: null | { status: string; at: string; commit?: string; error?: string; conflicts?: string[] };
  pr: null | { url: string; number: number; state: string };
};
```

Agents in a room get a **room brief** ahead of their first message, and a **room update** ahead
of each reply (through `runs.context`, which the worker already prepends). It contains the
room's name and goal, recent notes, and the other tasks with their owner, status, files and
integration state.

The page polls `GET /v1/rooms/:id` every 3 s while a room is open. No new SSE is needed for the MVP.
