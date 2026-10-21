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
   text, duration, turns, cost when the agent reports one, token `usage`).
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
| `POST` | `/v1/runs` `{task, agent?, model, repo, permission_mode?, isolation?, worker_id?}` | `202 {id, status}`; agent is `claude` (default), `codex` or `opencode`; for `claude` the model is one of `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`, for the others any model id the machine offers, or empty for its default; mode one of `acceptEdits`, `bypassPermissions`, `plan`, `default`, `dontAsk`; isolation `worktree` (default) or `checkout`; `worker_id` pins the run to that machine (any machine with the repo when omitted) |
| `GET` | `/v1/runs?limit=50` | threads (root runs) with `thread_status`, `follow_ups`, `last_activity_at` |
| `GET` | `/v1/runs/:id` | the run |
| `GET` | `/v1/runs/:id/thread` | all runs in the thread, root first |
| `POST` | `/v1/runs/:id/messages` `{task, model?, permission_mode?}` | `202`: a follow-up that resumes the thread's session with the same agent on the same machine; `409` while the previous message runs |
| `GET` | `/v1/runs/:id/events` | SSE; `id:` is the seq, `event:` is the kind; replays then live; ends with `end` |
| `POST` | `/v1/runs/:id/stop` | queued → `stopped` at once; running → `stop_requested` until the worker confirms |
| `GET` | `/v1/workers` | machines with capabilities and `online` (heartbeat < 45 s) |
| `GET` | `/v1/setup` | control URL and the first worker token, for the page's connect command |

Public: `GET /` (the page), `GET /connect.md` (instructions for an agent), `GET /healthz`.

## Worker endpoints

Bearer one of `SIDEFLEET_WORKER_TOKENS` (comma-separated, one per machine if you want to revoke
individually; compared as SHA-256 digests in constant time and never logged):
`POST /v1/worker/register`, `POST /v1/worker/heartbeat`, `POST /v1/worker/claim` (sends the
agents installed on the machine; workers that send none only get `claude` runs),
`POST /v1/worker/runs/:id/events`, `GET /v1/worker/runs/:id/control`,
`POST /v1/worker/runs/:id/finish`.
