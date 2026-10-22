# Sidefleet API

## How a run works

1. `POST /v1/runs` stores the run as `queued` and the page opens its live view.
2. A worker that has that repo and the run's agent claims it (`running`), prepares a git worktree
   on a new branch (or uses the checkout for in-place tasks, one per repo at a time), and spawns
   the agent with `cwd` set to the repo:
   - `claude`: `claude -p <task> --output-format stream-json --verbose --model <m> --permission-mode <mode>`
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

Bearer `SIDEFLEET_DEV_KEY`:

| Method | Path | |
|---|---|---|
| `POST` | `/v1/runs` `{task, agent?, model, repo, permission_mode?, isolation?, worker_id?, failover?, images?}` | `202 {id, status}`; agent is `claude` (default), `codex` or `opencode`; for `claude` the model is one of `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`, for the others any model id the machine offers, or empty for its default; mode one of `acceptEdits`, `bypassPermissions`, `plan`, `default`, `dontAsk`; isolation `worktree` (default) or `checkout`; `worker_id` pins the run to that machine (any machine with the repo when omitted); `failover: true` moves the run to another machine if its machine drops, pushing checkpoints of its branch to `origin` (see step 4; default false; follow-ups inherit it); `images` is up to 8 `{media_type, data}` (base64 PNG, JPEG, GIF or WebP, 10 MB each) for the agent to look at; the run lists them as `images: [{id, media_type, bytes}]` |
| `GET` | `/v1/runs?limit=50` | threads (root runs) with `thread_status`, `follow_ups`, `last_activity_at` |
| `GET` | `/v1/runs/:id` | the run |
| `GET` | `/v1/runs/:id/thread` | all runs in the thread, root first |
| `POST` | `/v1/runs/:id/messages` `{task, agent?, model?, permission_mode?, images?}` | `202`: a follow-up that resumes the thread's session with the same agent on the same machine. While an earlier message is queued or running, it waits behind it and resumes the latest session once claimed (stop the running one to send it now). With another `agent` the thread switches to it: the new agent starts its own session in the same worktree, and when claimed gets a transcript of the thread so far in `context` (as a fork does). Switching back later does the same, since the old session misses the other agent's turns. Without `model`, the model that agent last used in the thread, else its default. `400` when the thread's machine doesn't have the agent; `409` when the thread has no session to continue or 10 messages are already waiting |
| `GET` | `/v1/runs/:id/events` | SSE; `id:` is the seq, `event:` is the kind; replays then live; ends with `end` |
| `POST` | `/v1/runs/:id/restore` | `202`: put the thread's worktree back to before this run; `409` while a message in the thread is queued or running, for in-place threads, or when the run has no snapshot |
| `POST` | `/v1/runs/:id/fork` `{task, agent?, model, permission_mode?}` | `202 {id, status}`: a new thread from this message; `409` while it runs, `400` when the machine holding its files doesn't have the agent |
| `GET` | `/v1/images/:id` | an image from a tool result or attached to a message (PNG, JPEG, GIF or WebP); `tool_result` events and runs refer to them as `images: [{id, media_type, bytes}]` |
| `POST` | `/v1/runs/:id/stop` | queued → `stopped` at once; running → `stop_requested` until the worker confirms |
| `GET` | `/v1/workers` | machines with capabilities, `online` (heartbeat < 45 s) and `limited_until` (`{agent: time}` for agents whose usage limit the machine hit) |
| `GET` | `/v1/setup` | control URL and the first worker token, for the page's connect command |

Public: `GET /` (the page), `GET /connect.md` (instructions for an agent), `GET /healthz`.

## Worker endpoints

Bearer one of `SIDEFLEET_WORKER_TOKENS` (comma-separated, one per machine if you want to revoke
individually; compared as SHA-256 digests in constant time and never logged):
`POST /v1/worker/register`, `POST /v1/worker/heartbeat`, `POST /v1/worker/claim` (sends the
agents installed on the machine, and `slots: {parallel, active}`; workers that send no agents
only get `claude` runs, workers that send no slots are not load balanced, and workers that send
no `kinds` (`agent`, `restore`, `fork`) only get plain agent runs),
`POST /v1/worker/runs/:id/events`, `GET /v1/worker/runs/:id/control`, `GET /v1/worker/images/:id`
(a run's attached images, which the worker saves to a temporary folder for the agent),
`POST /v1/worker/runs/:id/finish` (`{status, result, usage_limit?: {resets_at?}}`; answers with
the run, which is `queued` again when a usage limit moved it to another machine).
