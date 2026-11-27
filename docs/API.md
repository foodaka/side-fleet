# Sidefleet API

## How a run works

1. `POST /v1/runs` stores the run as `queued` and the page opens its live view.
2. A worker that has that repo and the run's agent claims it (`running`), prepares a git worktree
   on a new branch (or uses the checkout for in-place tasks, one per repo at a time), and spawns
   the agent with `cwd` set to the repo:
   - `claude`: `claude -p <task> --output-format stream-json --verbose --model <m> --permission-mode <mode>`
     (for `ask`: `--permission-mode acceptEdits --mcp-config <…> --permission-prompt-tool mcp__sidefleet__approve`,
     see [Tool approvals](#tool-approvals-ask-me))
   - `codex`: `codex exec [resume] --json --skip-git-repo-check -c sandbox_mode=… [-m <m>] -- [<session>] <task>`
   - `opencode`: `opencode run --format json [--model <provider/model>] [--session <id>]`, task on stdin

   Each agent's output is mapped to the same events: `agent_init` (session id), `assistant_text`,
   `assistant_thinking`, `tool_use` (a Claude Code tool name such as `Bash` or `Edit`, plus a
   redacted one-line summary of the input), `tool_result` (truncated to 2 KB), `result` (final
   text, duration, turns, cost when the agent reports one, token `usage`). Images in a Claude Code
   tool result (a screenshot it read) travel with the event and are stored apart, up to 10 MB each.
3. Every event gets a per-run sequence number and is pushed to open SSE subscribers; the page
   renders it within milliseconds. Reloading replays from sequence 0; a dropped connection resumes
   with `Last-Event-ID`.
4. Stop sends `SIGTERM` to the agent's process group, then `SIGKILL` after 1.5 s, and marks the
   run `stopped`. A worker that stops heart-beating for 90 s (or restarts mid-run) has its running
   run marked `failed`, unless the run was created with `failover: true`:
   - While it runs in its worktree, the worker commits everything to the run's `sidefleet/<id>`
     branch ("sidefleet checkpoint", hooks skipped) and pushes that branch, and only that one, to
     `origin` after a `tool_result` of `Edit`, `Write` or `Bash`, at most once a minute. Each push
     is reported as a `checkpoint` event `{branch, sha}` (or `{branch, error}`).
   - When the machine is lost, the run is marked `failed` with reason `worker lost, moved` (its
     status event carries `moved_to`) and a new run is queued in the same thread with
     `moved_from` set, unpinned, with the same agent, model, repo and permission mode. Its task is
     the original message, a digest of the thread (messages, assistant text, tool calls; at most
     8 KB) and "Continue from branch `sidefleet/<id>` at `<sha>`". It carries
     `resume_from_branch`, so the worker that claims it fetches that branch from `origin` and
     creates its worktree from it instead of `HEAD`.
   - The machine that lost the run cannot claim the new one for 10 minutes; after that it may (so a
     single laptop that slept picks its task back up). A moved run does not move again; a stop
     that was pending wins over moving.
5. Before the agent starts and after it exits, the worker snapshots the working tree (tracked and
   untracked files, not ignored ones; a throwaway index, so the branch, index and stash are
   untouched). The changes go out as a `diff` event; the two snapshots are also kept as commits
   (parent: HEAD at the time) under `refs/sidefleet/snapshots/<root>/<run>/{before,after}` and
   reported as a `snapshot` event `{before, after}`.
6. Restore (`POST /v1/runs/:id/restore`) queues a run of kind `restore` in the thread, pinned to
   the machine, carrying the thread's session. The worker resets the worktree to the snapshot taken
   before `:id` started (HEAD back where it was, files it didn't have removed, its uncommitted
   changes back as unstaged edits; ignored files untouched) and reports `snapshot`, `diff` and
   `restored` events. Only `worktree` threads can be restored.
7. Fork (`POST /v1/runs/:id/fork`) starts a new thread (a root run with `forked_from`) with any
   agent and model. The control plane writes a transcript of the thread up to `:id` (messages,
   replies, one line per tool call; newest 32 KB) into `context`, which the worker puts ahead of
   the task. When `:id` has a snapshot, the fork is pinned to that machine and its worktree starts
   with the files after `:id` (`fork_commit`): a new branch at the commit HEAD was on, then the
   snapshot's files. Without one it starts from the repo's HEAD on any machine with the agent.

## Which machine runs a task

Workers pull work, so a run goes to a machine that asks for it, has the repo and the run's agent,
and has a free slot (`--parallel`). On top of that:

- **Least busy first.** Workers send their task slots (`{parallel, active}`) on every heartbeat
  and claim. For 10 s after a run is created, a machine is not given an unpinned run while another
  online machine that could run it has more free slots; after that any machine that can run it may
  take it, so a slow or busy "better" machine never stalls a task. Pinned runs and follow-ups
  (which run where their session lives) are not balanced.
- **Usage limits.** When an agent ends a run on the account's usage or rate limit (Claude Code's
  `You've hit your limit · resets 3pm` with its `rate_limit_event`, Codex's `usage limit`, a
  provider's `Rate limit exceeded` in opencode), the worker reports `usage_limit` with the finish.
  The control plane leaves that machine alone for that agent until the reset time the agent
  reported, or for an hour when it gave none, and the Machines page shows "usage limit until
  HH:MM". An unpinned run is put back in the queue once (`queued`, with a `usage_limit` event and
  `requeued_from`) when another online machine can take it; otherwise it is `failed`. Its other
  agents keep working on that machine. A pinned run on a limited machine waits for the reset.

Anything that looks like a credential (known key prefixes, bearer headers, `key=value`, JWTs, PEM
blocks, secret-looking JSON keys) is redacted on the worker before it leaves the machine, and
again on the control plane before it is stored.

## Developer endpoints

Bearer `SIDEFLEET_DEV_KEY`, acting in the default org; or the `sf_session` cookie from GitHub
sign-in, acting in the org named by `X-Sidefleet-Org`, else the user's personal org. Every route
is scoped to that org: another org's runs, images and machines read as 404.

Or a personal API token (`Bearer sfa_…`), for a script or a chat agent such as Hermes or
OpenClaw. It acts as the user who made it, in the org it was made in (another `X-Sidefleet-Org`
answers `403`), and stops working when revoked or when that user leaves the org. Its scope limits
what it can call; anything else answers `403`:

| Scope | Can |
|---|---|
| `read` | `GET` runs, threads, events, approvals, images, files, workers, repos and project icons; `GET /v1/me` shows the token's name and scope |
| `tasks` | also start runs (now or later), send messages, stop, start early, restore, fork, open PRs, archive, snooze, pin and set project icons; never with full access (`bypassPermissions`), including a reply or fork that would keep a thread's full access without naming another `permission_mode` |
| `full` | also approve or deny tool calls, and use full access |

No token manages orgs, members, worker or API tokens, enrollments or machines, or reads
`/v1/setup`. Tokens are made on the Chat apps page, or with these routes (session cookie or
developer key only; the developer key's tokens act in the default org):

| Method | Path | |
|---|---|---|
| `GET` | `/v1/api-tokens` | your tokens in this org, newest first; values are never listed |
| `POST` | `/v1/api-tokens` `{name, scope}` | `201` with `token`, shown this once; only its SHA-256 is stored |
| `DELETE` | `/v1/api-tokens/:id` | revoke; its next call gets `401` |

### MCP (chat agents)

`POST /mcp` is an MCP server (Streamable HTTP, answered with plain JSON; no sessions, `GET`
answers `405`), for a chat agent such as Hermes or OpenClaw, Claude Code or any other MCP client,
with an API token as `Authorization: Bearer sfa_…`. The Chat apps page hands out the setup for each.
Its tools, each listed only for tokens whose scope allows it:

| Tool | Scope | |
|---|---|---|
| `fleet_overview` | `read` | machines (online, agents, repos, busy slots, usage limits) and recent tasks |
| `list_tasks` `{limit?}` | `read` | recent threads with their status |
| `task_status` `{task_id, wait?}` | `read` | the thread's summary; with `wait`, follows a running task for up to 45 s until it finishes or asks for approval |
| `start_task` `{task, repo?, machine?, agent?, model?, access?}` | `tasks` | a new run. `repo` by identity, `owner/name` or just its name (optional with one repo), `machine` by name or id, `access` one of `edit` (`acceptEdits`, the default), `ask`, `read_only` (`plan`), `full` (`bypassPermissions`, `full` scope only) |
| `reply` `{task_id, message}` | `tasks` | a follow-up message |
| `stop_task` `{task_id}` | `tasks` | stop the thread's latest message |
| `open_pull_request` `{task_id}` | `tasks` | as `POST /v1/runs/:id/pr` |
| `approve`, `deny` `{task_id, approval_id, reason?}` | `full` | decide a pending tool approval |

### Updates in chat

A connection can have Sidefleet tell its app how tasks go, so the assistant brings it up in the
chat: when a task finishes (`task.finished`), fails (`task.failed`), waits for approval
(`task.needs_approval`, with the `approval`) or gets a pull request (`task.pull_request`).
Updates cover the threads the connection started or sent a message to, or every thread of the org
with `all_tasks`; a stopped run, or one with a follow-up queued behind it, sends none. Each is
`{event_type, text, task}`: `text` is written for the assistant (what happened, the agent's last
message, changed files, and what it can do next with the tools), `task` is the thread's
[summary](#developer-endpoints). Sent in the background, retried twice (after 2 s and 10 s).

| Method | Path | |
|---|---|---|
| `PUT` | `/v1/api-tokens/:id/updates` `{kind, url, all_tasks?}` | turn on or change; `kind` is `hermes`, `openclaw` or `webhook`. Returns the token with `secret`, made the first time and kept across changes |
| `DELETE` | `/v1/api-tokens/:id/updates` | turn off (the secret goes too) |
| `POST` | `/v1/api-tokens/:id/updates/test` | send a test update now; `502` with the reason when it didn't arrive |

- `hermes`: a Hermes webhook route (`hermes webhook subscribe sidefleet --prompt "{text}" --deliver telegram --mirror-to-session --secret <secret>`),
  signed "Generic V2": `X-Webhook-Timestamp`, and `X-Webhook-Signature-V2` = hex HMAC-SHA256 of
  `<timestamp>.<body>` with the secret.
- `openclaw`: OpenClaw's `POST /hooks/agent` with `Authorization: Bearer <secret>` (its
  `hooks.token`) and `{message: text, name: "Sidefleet"}`, plus an `Idempotency-Key`.
- `webhook`: the JSON as is, signed like `hermes`, with `X-Sidefleet-Event` and `X-Sidefleet-Delivery`.

A token's list entry shows `updates: {kind, url, all_tasks, last_at, last_error}`, never the secret.
On a hosted control plane, updates only go to public addresses: the name must resolve to public IPs
only, and the connection is made to the address that was checked. Redirects aren't followed.
`SIDEFLEET_PRIVATE_WEBHOOKS=1` allows private ones (the default when self-hosting).

Each tool calls the routes above in-process with the caller's own credentials, so it can do exactly
what the token can do on the API, with the same checks and plan limits. Ids are checked before they
go into a path.

| Method | Path | |
|---|---|---|
| `POST` | `/v1/runs` `{task, agent?, model, effort?, repo, permission_mode?, isolation?, worker_id?, failover?, images?, files?, start_at?}` | `202 {id, status}`; agent is `claude` (default), `codex` or `opencode`; for `claude` the model is one of `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`, for the others any model id the machine offers, or empty for its default; `effort` (reasoning) is for `claude` one of `low`, `medium`, `high`, `xhigh`, `max` (its `--effort`), `ultrathink` or `ultracode` (keywords added to the message; ultracode also runs at `xhigh`), for `codex` one of `minimal`, `low`, `medium`, `high`, `xhigh` (its `model_reasoning_effort`), absent for the agent's default, and not accepted for `opencode`; mode one of `acceptEdits`, `bypassPermissions`, `plan`, `ask` (Claude Code only; `400` with another agent), `default`, `dontAsk`; isolation `worktree` (default) or `checkout`; `worker_id` pins the run to that machine (any machine with the repo when omitted); `failover: true` moves the run to another machine if its machine drops, pushing checkpoints of its branch to `origin` (see step 4; default false; follow-ups inherit it); `images` is up to 8 `{media_type, data}` (base64 PNG, JPEG, GIF or WebP, 10 MB each) for the agent to look at; the run lists them as `images: [{id, media_type, bytes}]`; `files` is up to 8 `{name, media_type?, data}` (base64, any type such as markdown, text, code or PDF, 10 MB each) for the agent to read; the name keeps only its last path part, and the run lists them as `files: [{id, name, media_type, bytes}]`; `start_at` (RFC 3339) starts the task later: it stays `queued` and no machine claims it before then (at most 30 days ahead, `400` past that or in a room; a time already past means now), and the run lists it as `start_at` |
| `GET` | `/v1/runs?limit=50` | threads (root runs) with `thread_status`, `follow_ups`, `last_activity_at`; pinned threads first, then by `last_activity_at` |
| `GET` | `/v1/runs/:id` | the run |
| `GET` | `/v1/runs/:id/thread` | all runs in the thread, root first |
| `GET` | `/v1/runs/:id/summary` | the thread in a few lines, for scripts and chat agents: `status` (of its latest message), `machine`, `reply` (what the agent last said, the end of it when over 4,000 characters), `error`, `changed_files` (`count`, `additions`, `deletions`, up to 30 `files` with their stats), `pending_approvals` (`id`, `tool`, `input`), `pr` and the page's `url` |
| `POST` | `/v1/runs/:id/messages` `{task, agent?, model?, effort?, permission_mode?, images?, files?}` | `202`: a follow-up that resumes the thread's session with the same agent on the same machine. While an earlier message is queued or running, it waits behind it and resumes the latest session once claimed (stop the running one to send it now). With another `agent` the thread switches to it: the new agent starts its own session in the same worktree, and when claimed gets a transcript of the thread so far in `context` (as a fork does). Switching back later does the same, since the old session misses the other agent's turns. Without `model`, the model that agent last used in the thread, else its default; likewise without `effort` (`""` goes back to the agent's default). `400` when the thread's machine doesn't have the agent; `409` when the thread has no session to continue or 10 messages are already waiting |
| `GET` | `/v1/runs/:id/events` | SSE; `id:` is the seq, `event:` is the kind; replays then live; ends with `end` |
| `POST` | `/v1/runs/:id/restore` | `202`: put the thread's worktree back to before this run; `409` while a message in the thread is queued or running, for in-place threads, or when the run has no snapshot |
| `POST` | `/v1/runs/:id/fork` `{task, agent?, model, effort?, permission_mode?}` | `202 {id, status}`: a new thread from this message; `409` while it runs, `400` when the machine holding its files doesn't have the agent |
| `GET` | `/v1/files/:id` | a file attached to a message, always as a download (`application/octet-stream`, `Content-Disposition: attachment`) |
| `GET` | `/v1/images/:id` | an image from a tool result or attached to a message (PNG, JPEG, GIF or WebP); `tool_result` events and runs refer to them as `images: [{id, media_type, bytes}]` |
| `GET` | `/v1/runs/:id/approvals` | the run's tool approvals, oldest first (see [Tool approvals](#tool-approvals-ask-me)) |
| `POST` | `/v1/runs/:id/approvals/:approval/approve` | `200` with the approval, now `approved`: the agent's tool call goes ahead. `409` when it was already decided, expired or cancelled, or the run is no longer running; `410` when its time ran out first (it is now `expired`) |
| `POST` | `/v1/runs/:id/approvals/:approval/deny` `{message?}` | `200` with the approval, now `denied`; the agent is told `message`, or that the user denied it. Same `409`/`410` as approve |
| `POST` | `/v1/runs/:id/stop` | queued → `stopped` at once; running → `stop_requested` until the worker confirms |
| `POST` | `/v1/runs/:id/start-now` | a task scheduled with `start_at` drops its start time and is claimable at once; returns the run (unchanged once it has started) |
| `POST` | `/v1/runs/:id/snooze` `{until}` | `200` with the thread's root run: its `snoozed_until` is `until` (RFC 3339, in the future, at most 366 days ahead; `400` otherwise), or `null` to wake it now. Any run of the thread may be named. A snoozed thread stays in `GET /v1/runs` (the page lists it apart until `snoozed_until`) and leaves the inbox until then. It wakes early (`snoozed_until` back to `null`) on a reply, or when it starts waiting on you after it was snoozed: it finishes, fails or asks for a tool approval |
| `POST` | `/v1/runs/:id/pin` `{pinned}` | `200` with the thread's root run: its `pinned_at` is when it was pinned, or `null` once unpinned. Any run of the thread may be named. A pin belongs to the thread, so everyone in the org sees it; `GET /v1/runs` lists pinned threads first, so the limit never leaves one out, and the page keeps them in a Pinned section at the top |
| `GET` | `/v1/project-icons` | the icons set for this org's projects (repos): `[{repo, kind, value, updated_at}]`. `kind` is `emoji` (value: the emoji), `letter` (value: the tile colour, `#rrggbb`) or `image` (value: a PNG, JPEG or WebP `data:` URL, at most 48 KB). A repo with none set gets a default the page picks |
| `PUT` | `/v1/project-icons` `{repo, icon: {kind, value} \| null}` | set a repo's icon for everyone in the org, or `null` to go back to the default; `200` with every icon, as `GET` answers. `400` for a malformed icon |
| `GET` | `/v1/workers` | machines with capabilities, `online` (heartbeat < 45 s) and `limited_until` (`{agent: time}` for agents whose usage limit the machine hit) |
| `DELETE` | `/v1/workers/:id` | forget a machine (frees a plan slot); `409` while it runs a task |
| `POST` | `/v1/workers/:id/new-repos` `{name, url?}` | `202 {id, name}`: ask one of your machines to start a project in its repos folder: an empty git repo with a README and a first commit, or a clone of `url` (`https://`, `ssh://`, `git://` or `git@host:path`; `name` defaults to the URL's last part). Only the machine's owner may; `409` when it's offline, already making that name, or too old (no `capabilities.new_repos_root`); `400` for a bad name or URL |
| `GET` | `/v1/new-repos/:id` | `{id, worker_id, kind, name, status, repo, path, error, warning}`. `status` goes `pending` → `running` → `done` or `failed`. When done, `repo` is the new repo's key, already in the machine's repos and in the org's chosen repos (if it has chosen any); `warning` says what fell short, such as no first commit when git has no name or email there. A request the machine never takes fails after 10 minutes, one it never finishes after 15 |
| `GET` | `/v1/setup` | control URL and, in the default org, the first static worker token, for the page's connect command |
| `POST` | `/v1/orgs` `{name}` | create a team org (signed-in users) |
| `GET` `POST` | `/v1/orgs/:org/tokens` `{name}` | per-org worker tokens; the value is only in the `201` response; owners only |
| `DELETE` | `/v1/orgs/:org/tokens/:id` | revoke; workers using it get `401` from then on |
| `GET` `POST` | `/v1/orgs/:org/members` `{login, role?}` | members; adding needs an owner and a user who signed in once |
| `GET` | `/v1/enrollments` | machines waiting for approval in this org |
| `POST` | `/v1/enrollments/lookup` `{code}` | find a waiting machine by the code it printed |
| `POST` | `/v1/enrollments/:id/approve` `{code?}`, `/deny` | approve (mints a per-machine token) or deny; `402` over the plan's machine limit |
| `GET` | `/v1/inbox` | `{items: [{id, run_id, title, repo, agent, reason, label, detail?, since}]}`: tasks waiting on the caller, newest first (see below) |
| `GET` | `/v1/inbox/events` | SSE: an `inbox` event with the same `{items}` at once and whenever the list changes |
| `POST` | `/v1/inbox/:id/dismiss` | `204`: mark the task seen (any run id of the thread); the page does this when it opens a task |
| `POST` | `/v1/inbox/dismiss-all` | `204`: everything waiting now counts as seen |
| `GET` | `/v1/push` | `{available, public_key}`: whether this control plane can send web push, and its VAPID public key (base64url) |
| `POST` | `/v1/push/subscriptions` `{endpoint, keys: {p256dh, auth}}` | `201`: notify this browser (`PushSubscription.toJSON()`); the endpoint must be a browser push service; `409` without a VAPID key |
| `DELETE` | `/v1/push/subscriptions` `{endpoint}` | `204`: stop notifying it |

**The inbox.** A task (thread) waits on you when its latest message has one of these `reason`s:
`finished` (succeeded; waiting for a reply), `failed`, `usage_limit` (failed on the machine's
usage limit, or queued again until another machine takes it; `detail` says when it resets),
`machine_lost` (failed because its machine stopped responding, or moved and waiting for another
machine) or `approval_needed` (reserved for tool approvals). Running, queued replies, stopped
tasks, finished restores, archived tasks and snoozed ones (until their time, unless they started
waiting after they were snoozed) are not listed; the stream sends the list again when a snooze ends. `since` is when it started waiting; it
stays listed until the caller opens or dismisses it after that, or replies. "Seen" is kept per
viewer: each signed-in user has their own, and the developer key is one shared viewer. Tasks from
the last 30 days are considered. With `SIDEFLEET_VAPID_PRIVATE_KEY` set, each subscribed browser
of someone who can see the task and has not seen it gets a web push about 10 s after it starts
waiting (not if it was picked up or opened in the meantime), holding `{id, title, reason, label,
url}`: the title's first line and the reason, never agent output.

Plan limits answer `402` (machines; runs queued or running at once, for new tasks, messages,
restores and forks); owner-only actions answer `403` for members.

Public: `GET /` (the page), `GET /service-worker.js` (shows push notifications), `GET /connect.md` (instructions for an agent), `GET /healthz` (checks
the database), `GET /v1/auth` (which sign-in methods are on), `GET /v1/me` (the signed-in user and
their orgs; `401` when signed out), `GET /auth/github/login` and `/auth/github/callback`,
`POST /auth/logout`, `POST /v1/enroll` and `GET /v1/enroll/:id` (a machine asking to join, and
polling with its secret).

## Tool approvals ("Ask me")

A run with `permission_mode: "ask"` (Claude Code only) asks the person on the page before a tool
call that `acceptEdits` would refuse, instead of refusing it:

1. The worker starts Claude Code with `--permission-mode acceptEdits`,
   `--permission-prompt-tool mcp__sidefleet__approve` and an `--mcp-config` that runs
   `sidefleet approval-mcp --socket <path>`: a small stdio MCP server, spawned by Claude Code,
   whose one tool relays each question over a Unix socket (in a `0700` folder in the temp dir) to
   the worker. It holds no credentials.
2. The worker redacts the tool input (and clips strings to 4 KB) and calls
   `POST /v1/worker/runs/:id/approvals` `{tool_name, input, tool_use_id?, timeout_secs?}`. The
   control plane stores a `pending` approval and appends an `approval_requested` event (the
   approval plus a one-line `summary`), so the page shows Approve and Deny inline and the thread's
   `pending_approvals` in `GET /v1/runs` makes the sidebar say "Waiting on you".
3. Someone approves or denies; that appends `approval_decided` `{id, tool_name, tool_use_id,
   status, message, decided_by, decided_at}`.
4. The worker polls `GET /v1/worker/runs/:id/approvals/:approval` every second and answers
   Claude Code `{"behavior": "allow", "updatedInput": <the original input>}` or
   `{"behavior": "deny", "message": …}`. The original input never leaves the machine.

An approval changes once, from `pending` to one of: `approved`, `denied`, `expired` (nobody
decided within `timeout_secs`, 15 minutes by default, at most 24 hours; a late approve answers
`410`), or `cancelled` (the run finished, was stopped or was lost first). Anything that goes wrong
on the way, such as the control plane being unreachable, denies the tool call.

## Control plane: agents, policies, approvals, audit

Every route here is scoped to the caller's org, like the rest of the developer API. Reads are open
to every member. Changing policies, suspending and reactivating need an org owner (the developer
key counts as one). Approvals are decided by an org owner or the agent's owner. Anything else
answers `403`. No API token scope covers these routes yet, so tokens get `403` too.

### Agents

An agent is something that runs work: a Claude Code, Codex or opencode install on a Sidefleet
machine, or a custom agent that registered itself. Sidefleet-run agents are **discovered**: one per
org, machine and agent kind (`claude on build-02`), created when the machine registers or claims a
run, with no owner until someone claims it. Others are **registered** with `POST /v1/agents` and
get an agent key (`sfk_…`) for the gateway routes below.

An agent as the API returns it:

```json
{
  "id": "…", "name": "claude on build-02", "description": "", "kind": "claude",
  "provider": "Anthropic", "model": "claude-opus-5-5", "runtime": "Sidefleet worker",
  "host": "build-02", "worker_id": "build-02", "environment": "", "team": "",
  "owner": {"id": "…", "login": "riley"}, "risk": "unknown", "source": "discovered",
  "status": "running", "pending_approvals": 0, "credentials": [], "can_cancel": true,
  "suspension": null, "created_at": "…", "last_active_at": "…"
}
```

`status` is `suspended`, else `waiting` (a pending approval), else `running` (a running task, or a
registered agent that reported `task_started` in the last 10 minutes), else `idle`. `risk` is
`low`, `medium`, `high` or `unknown`. `credentials` names the Sidefleet-managed credentials the
agent holds (only registered agents' keys, for now). `can_cancel` says whether a suspension can
stop its running task: true on a Sidefleet worker, false for a registered agent, whose process
Sidefleet does not run. `suspension` is `{at, by, reason}` while suspended.

| Method | Path | |
|---|---|---|
| `GET` | `/v1/agents` | every agent in the org, most recently active first |
| `POST` | `/v1/agents` `{name, description?, kind?, provider?, model?, runtime?, host?, environment?, team?, risk?, owner_id?}` | register a custom agent (`kind` defaults to `custom`, the owner to the caller). `201 {agent, key}`: the key is shown this once and only its SHA-256 is stored |
| `GET` | `/v1/agents/:id` | `{agent, permissions, policies, incidents}`: what it may do now (one row per action category with the decision and the policy that made it), the active policies that apply to it, and its suspensions |
| `PATCH` | `/v1/agents/:id` `{name?, description?, team?, environment?, risk?, owner_id?}` | by an org owner or the agent's owner. Changing the team changes which team policies apply and is audited as `permission.changed` |
| `POST` | `/v1/agents/:id/claim` | become the owner of an agent nobody owns |
| `POST` | `/v1/agents/:id/suspend` `{reason, revoke_credentials?, cancel_running?}` | org owners. Blocks new tasks (the agent's machine no longer claims runs for that agent kind) and every tool call at the gateway, cancels its pending approvals, revokes its agent key when `revoke_credentials` (default true), and stops its running tasks when `cancel_running` (default true) and the runtime supports it. `{agent, incident}`; `incident.not_stopped` explains when a running process could not be stopped |
| `POST` | `/v1/agents/:id/reactivate` | org owners. Revoked keys stay revoked |
| `POST` | `/v1/agents/:id/key` | issue a new agent key for a registered agent (org owner or its owner; not while suspended). `{key}` once |

### Activity

`GET /v1/activity` lists agent events, newest first. Filters: `agent`, `owner` (user id), `team`,
`type`, `outcome`, `since`, `until` (RFC 3339), `limit` (default 100, at most 500) and `before`
(an event id, for the next page). The answer is `{events, counts}`; `counts` is the number of
events per outcome under the same filters without `outcome`.

```json
{
  "id": "…", "at": "…", "agent": {"id": "…", "name": "…"}, "owner": {"id": "…", "login": "…"},
  "team": "", "run_id": "…", "type": "tool_call", "tool": "shell.exec", "action": "pnpm test",
  "context": {"task": "…", "repo": "…", "branch": "…"}, "outcome": "allowed",
  "policy": {"id": "…", "name": "Shell in worktree"}, "decision": "allow", "metadata": {}
}
```

`type` is one of `task_started`, `task_completed`, `execution_failure`, `tool_call`,
`permission_request`, `approval_approved`, `approval_rejected`, `approval_expired`,
`approval_cancelled`, `agent_suspended`, `agent_reactivated`. `outcome` is one of `started`,
`succeeded`, `failed`, `stopped`, `allowed`, `approval_required`, `denied`, `approved`,
`rejected`, `expired`, `cancelled`, `suspended`, `reactivated`. Actions and inputs go through the
same redaction as run events before they are stored.

### Policies

A policy says what happens when an agent tries a matching action:

```json
{
  "id": "…", "name": "Production data access", "description": "…", "scope": "org",
  "applies_to": "", "tools": ["database.query", "shell.exec: psql, mysql"],
  "category": "Access production database", "decision": "require_approval",
  "priority": 10, "active": true, "builtin": false, "updated_at": "…", "updated_by": "riley"
}
```

`scope` is `org` (every agent), `team` (`applies_to` is a team name) or `agent` (`applies_to` is
an agent id). `decision` is `allow`, `require_approval` or `deny`. Each `tools` entry is a tool
pattern, optionally followed by `:` and comma-separated arguments: `shell.exec`, `deploy.*`,
`git.push: main, release/*`, `database.write: DELETE, DROP`. A pattern with arguments matches
when the action (or one of the shell command's parts) starts with one of them, word for word, or
matches it as a glob.

Tool calls are turned into actions before they are checked. Claude Code's `Read`, `Glob`, `Grep`
are `fs.read`; `Write`, `Edit`, `MultiEdit`, `NotebookEdit` are `fs.write` inside the task's
worktree and `fs.write.outside` elsewhere; `Bash` is `shell.exec`, plus `git.push` (with the
branch it pushes to), `database.query` and `database.write` (`psql`, `mysql` and similar, with
any `DELETE`, `DROP`, `TRUNCATE`), `secrets.read` (secret-looking environment variables and
secret manager commands) and `deploy.*` (`kubectl apply`, `helm upgrade`, `terraform apply`, …)
when the command does those; `WebFetch` and `WebSearch` are `net.fetch`; to-do lists, subagents
and plans are `agent.*`; MCP tools are `mcp.<server>.<tool>`. Custom agents name the action
themselves.

For each action the matching active policy with the lowest priority number decides; nothing
matching means deny. A tool call that is several actions gets the strictest of their decisions.
A suspended agent gets deny for everything. A new org starts with these policies, all editable:
Destructive production ops (deny, 1), Push to protected branches (deny, 5), Production data
access (approval, 10), External credentials (approval, 15), Production deploys (approval, 20),
Push task branches (allow, 60), Shell in worktree (allow, 90), Agent housekeeping (allow, 95),
Web access (allow, 96), MCP tools (approval, 97), Project workspace (allow, 100), Temporary files
(allow, 105) and Default deny (deny, 1000).

| Method | Path | |
|---|---|---|
| `GET` | `/v1/policies` | every policy, lowest priority first |
| `POST` | `/v1/policies` `{name, description?, scope, applies_to?, tools, category?, decision, priority, active?}` | org owners; `201` with the policy |
| `PATCH` | `/v1/policies/:id` | org owners; any of the fields above. Audited as `policy.updated`, or `policy.activated` / `policy.deactivated` |
| `DELETE` | `/v1/policies/:id` | org owners; the Default deny policy can be turned off but not deleted |
| `POST` | `/v1/policy/evaluate` `{agent_id?, tool, action}` or `{agent_id?, tool_name, input, workdir?}` | the decision the gateway would make, without acting on it: `{decision, policy, overridden, actions, reason, elapsed_ms}`. `policy` (`{id, name, priority, scope}`, or null for the built-in fallback) decided; `overridden` lists the other policies that matched, with their decisions |

### Approvals

Approvals are the "Ask me" approvals above plus the ones a `require_approval` policy creates.
A policy approval is bound to the exact action: it stores `digest`, the SHA-256 of the tool name
and its (redacted) input, and the gateway only lets through the call with that digest. A changed
call needs a new approval.

| Method | Path | |
|---|---|---|
| `GET` | `/v1/approvals?status=pending\|resolved` | `{approvals, counts: {pending, resolved}}`, newest first |
| `POST` | `/v1/approvals/:id/approve` `{message?}` | approve once |
| `POST` | `/v1/approvals/:id/reject` `{message?}` | reject; the message is what the agent is told |

Each approval has `id`, `kind` (`policy` or `ask`), `status` (`pending`, `approved`, `denied`
(shown as rejected), `expired`, `cancelled`), `agent`, `owner`, `runtime` (`Claude Code on
build-02`), `run_id`, `title`, `tool` (the action, `shell.exec`), `tool_name`, `command` (what the
agent wants to run, redacted), `digest`, `context` `{task, repo, branch}`, `policy`
`{id, name, scope, applies_to, priority}` (null for "Ask me"), `message`, `decided_by`, `created_at`,
`expires_at`, `decided_at` and `can_decide` (whether the caller may decide it). Suspending the
agent cancels its pending approvals.

### Audit log

An append-only record of agent registration and changes, policy changes, permission changes,
approval requests and decisions, denied actions, task cancellations, suspensions, credential
revocations and reactivations. The database refuses to update or delete a record. Each record
stores the SHA-256 of the record before it in the org (`prev_hash`) and its own `hash` over its
contents and `prev_hash`, so changing or removing one breaks the chain from there on.

```json
{
  "id": "…", "seq": 42, "at": "…", "actor": "riley", "actor_kind": "admin",
  "event": "agent.suspended", "agent_id": "…", "target": "claude on build-02",
  "detail": "Unexpected writes to the ledger table",
  "data": {"what": "…", "agent": "…", "owner": "…", "permissions": "…", "policy": "…",
           "approval": "…", "decided_by": "…", "outcome": "…"},
  "prev_hash": "…", "hash": "…"
}
```

`actor_kind` is `admin` (org owner), `member`, `agent`, `api_key` or `system`.

| Method | Path | |
|---|---|---|
| `GET` | `/v1/audit` | `{records, total}`, newest first. Filters: `q` (searches actor, event, target and detail), `event`, `agent`, `since`, `until`, `limit` (default 100, at most 500), `before` (a `seq`) |
| `GET` | `/v1/audit/verify` | walks the whole chain: `{intact, records, broken_at, head, verified_at}`; `broken_at` is the `seq` of the first record that does not fit |
| `GET` | `/v1/audit/export?format=csv\|jsonl` | the records under the same filters, oldest first, as a download |

### Dashboard

`GET /v1/control/summary` gives the dashboard its numbers: `agents` `{total, teams, unowned,
running, waiting, suspended}`, `approvals` `{pending, soonest_expiry}`, `decisions_24h` `{total,
allowed, approval, denied, hours: [{hour, allowed, approval, denied}]}` (24 hourly buckets, oldest
first), `governed_7d` `{total, governed, no_policy, unattributed}` (tool calls checked against a
policy other than the default deny and made by an owned agent; matched only the fallback; made by
an agent without an owner), and `engine` `{p95_ms}`.

### The execution gateway

Policies are enforced where tool calls run, not on the page.

**Claude Code on a Sidefleet machine.** Every Claude Code run gets a `PreToolUse` hook (through
`--settings`) that runs `sidefleet policy-hook --socket <path>` before each tool call. The hook
passes the call over the run's Unix socket to the worker, which asks
`POST /v1/worker/runs/:id/policy?worker_id=…` `{tool_name, input, tool_use_id?, workdir?,
timeout_secs?}` (input redacted). The control plane evaluates the call for the run's agent,
records the decision, and answers `{decision, policy, reason, digest, approval}`:

- `deny`: the hook blocks the call and the agent is told why.
- `require_approval`: `approval` is a new pending approval. The worker waits for it like an
  "Ask me" approval; approved (with the same digest) lets the call run, anything else blocks it.
- `allow`: the hook steps aside and Claude Code's own permission mode applies as before.

If the control plane cannot be reached, or the answer is unreadable, only reads (`fs.read`) and
agent housekeeping (`agent.*`) go ahead: everything else is denied. Codex and opencode have no
hook Sidefleet can use yet, so their tool calls are not checked; a suspension still stops their
new tasks and running ones.

**Custom agents** call the gateway themselves with their agent key, `Bearer sfk_…`:

| Method | Path | |
|---|---|---|
| `POST` | `/v1/gateway/evaluate` `{tool, action}` or `{tool_name, input}`, plus `context?` `{task, repo, branch}` and `timeout_secs?` | `{decision, policy, reason, digest, approval}`, recorded like any tool call. For `require_approval`, poll the approval |
| `GET` | `/v1/gateway/approvals/:id` | `{id, status, message, digest, expires_at}`; expires once past its deadline |
| `POST` | `/v1/gateway/events` `{type, tool?, action?, outcome?, context?, metadata?}` | report `task_started`, `task_completed`, `execution_failure` or a `tool_call`; `204` |

A suspended agent's key is revoked (`401` from then on) unless the suspension kept it; with a kept
key every evaluation answers `deny` with the reason "agent suspended".

## Worker endpoints

Bearer one of `SIDEFLEET_WORKER_TOKENS` (comma-separated, one per machine if you want to revoke
individually; compared as SHA-256 digests in constant time and never logged), acting in the
default org, or a per-org token, acting in its org:
`POST /v1/worker/register`, `POST /v1/worker/heartbeat`, `POST /v1/worker/claim` (sends the
agents installed on the machine, and `slots: {parallel, active}`; workers that send no agents
only get `claude` runs, workers that send no slots are not load balanced, and workers that send
no `kinds` (`agent`, `restore`, `fork`) only get plain agent runs; a worker that sends `new_repo` in `kinds` may get `202 {new_repos: [{id, name, kind: init | clone, url?}]}` in place of a run, and the heartbeat hands the same out as `new_repos` to a worker that reports `capabilities.new_repos_root`; it answers each with `POST /v1/worker/new-repos/:id` `{worker_id, repo?, error?, warning?}`),
`POST /v1/worker/runs/:id/events`, `GET /v1/worker/runs/:id/control`,
`POST /v1/worker/runs/:id/approvals` and `GET /v1/worker/runs/:id/approvals/:approval` (a tool
approval, above; asking is only allowed while the run is running on that worker, `409` otherwise), `GET /v1/worker/images/:id`
and `GET /v1/worker/files/:id` (a run's attached images and files, which the worker saves to a temporary folder for the agent),
`POST /v1/worker/runs/:id/finish` (`{status, result, usage_limit?: {resets_at?}}`; answers with
the run, which is `queued` again when a usage limit moved it to another machine).
