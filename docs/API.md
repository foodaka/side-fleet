# Sidefleet API

## How a run works

1. `POST /v1/runs` stores the run as `queued` and the page opens its live view.
2. A worker that has that repo claims it (`running`), prepares a git worktree on a new branch
   (or uses the checkout for in-place tasks, one per repo at a time), spawns
   `claude -p <task> --output-format stream-json --verbose --model <m> --permission-mode <mode>`
   with `cwd` set to the repo, and forwards every stream-json line as an event: `agent_init`,
   `assistant_text`, `tool_use` (name plus a redacted one-line summary of the input),
   `tool_result` (truncated to 2 KB), `result` (final text, duration, turns, cost).
3. Every event gets a per-run sequence number and is pushed to open SSE subscribers; the page
   renders it within milliseconds. Reloading replays from sequence 0; a dropped connection resumes
   with `Last-Event-ID`.
4. Stop sends `SIGTERM` to the agent's process group, then `SIGKILL` after 1.5 s, and marks the
   run `stopped`. A worker that stops heart-beating for 90 s has its running run marked `failed`.

Anything that looks like a credential (known key prefixes, bearer headers, `key=value`, JWTs, PEM
blocks, secret-looking JSON keys) is redacted on the worker before it leaves the machine, and
again on the control plane before it is stored.

## Developer endpoints

Bearer `SIDEFLEET_DEV_KEY`:

| Method | Path | |
|---|---|---|
| `POST` | `/v1/runs` `{task, model, repo, permission_mode?, isolation?, worker_id?}` | `202 {id, status}`; model is one of `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`; mode one of `acceptEdits`, `bypassPermissions`, `plan`, `default`, `dontAsk`; isolation `worktree` (default) or `checkout`; `worker_id` pins the run to that machine (any machine with the repo when omitted) |
| `GET` | `/v1/runs?limit=50` | threads (root runs) with `thread_status`, `follow_ups`, `last_activity_at` |
| `GET` | `/v1/runs/:id` | the run |
| `GET` | `/v1/runs/:id/thread` | all runs in the thread, root first |
| `POST` | `/v1/runs/:id/messages` `{task, model?, permission_mode?}` | `202`: a follow-up that resumes the thread's session on the same machine; `409` while the previous message runs |
| `GET` | `/v1/runs/:id/events` | SSE; `id:` is the seq, `event:` is the kind; replays then live; ends with `end` |
| `POST` | `/v1/runs/:id/stop` | queued → `stopped` at once; running → `stop_requested` until the worker confirms |
| `GET` | `/v1/workers` | machines with capabilities and `online` (heartbeat < 45 s) |
| `GET` | `/v1/setup` | control URL and the first worker token, for the page's connect command |

Public: `GET /` (the page), `GET /connect.md` (instructions for an agent), `GET /healthz`.

## Worker endpoints

Bearer one of `SIDEFLEET_WORKER_TOKENS` (comma-separated, one per machine if you want to revoke
individually; compared as SHA-256 digests in constant time and never logged):
`POST /v1/worker/register`, `POST /v1/worker/heartbeat`, `POST /v1/worker/claim`,
`POST /v1/worker/runs/:id/events`, `GET /v1/worker/runs/:id/control`,
`POST /v1/worker/runs/:id/finish`.
