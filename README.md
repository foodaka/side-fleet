# AgentCloud spike

A remote control for the coding agents already installed on your own machines.
You type a task and pick a model on a web page; a worker on the machine that has
Claude Code signed in and the repo checked out runs it and streams the agent's
narration back live.

This spike proves exactly one moment: **fire a Claude Code task from one machine
and watch it run on another**. No scheduler, no placement, no billing.

```
 browser ──HTTP/SSE──▶ acd (control plane, Axum + Postgres)
                          ▲
                          │ outbound HTTP only (register, heartbeat, claim, events, finish)
                          │
                        acw (worker) ──spawns──▶ claude -p "<task>" --output-format stream-json
                                                      (your own Claude Code sign-in, cwd = the repo)
```

| Crate | What it is |
|---|---|
| `crates/core` (`ac-core`) | Shared models, wire types, the stream-json → event mapper, secret redaction |
| `crates/acd` | Control plane: Postgres schema + migrations, runs/workers API, SSE stream, the static UI |
| `crates/acw` | Worker: detects Claude Code / Codex / Ollama and git checkouts, claims runs, executes them |

No API key is ever handled by AgentCloud. The worker invokes the `claude` CLI that is
already signed in on that machine; the control plane only ever sees the narration.

## Setup

Prerequisites: Rust (stable, 1.80+), Docker (for Postgres), and Claude Code installed
and signed in on every machine that will act as a worker.

```bash
git clone <this repo> && cd agentcloud
cp .env.example .env         # then edit the keys below
docker compose up -d --wait  # Postgres on localhost:5432
cargo build --release
```

`.env` (read by `acd`; `acw` takes flags or `ACW_*` env):

| Variable | Used by | Meaning |
|---|---|---|
| `DATABASE_URL` | acd | `postgres://acd:acd@localhost:5432/acd` by default |
| `ACD_BIND` | acd | listen address, default `0.0.0.0:8080` |
| `ACD_DEV_KEY` | acd, browser | bearer key for `/v1/runs` and `/v1/workers` (min 8 chars) |
| `ACD_WORKER_TOKENS` | acd | comma-separated tokens workers may present; issue one per machine |
| `ACW_TOKEN` | acw | the token this worker presents (same as `--token`) |

Generate keys with `openssl rand -hex 24`. Tokens are compared as SHA-256 digests in
constant time and are never written to logs.

## Running `acd` on one machine and `acw` on another

The worker only makes outbound HTTP calls, so it needs no inbound port and no public
address. It just has to reach the control plane. Tailscale is the easy way; a LAN works
the same.

**Machine A (control plane), e.g. a VPS or your desk Mac:**

```bash
docker compose up -d --wait
set -a; source .env; set +a
./target/release/acd
# acd listening on 0.0.0.0:8080; open http://<A's tailscale IP>:8080/ in a browser
```

**Machine B (worker), the machine with Claude Code signed in and your repos:**

```bash
./target/release/acw run \
  --control http://100.x.y.z:8080 \     # machine A's Tailscale or LAN address
  --token   <one of ACD_WORKER_TOKENS> \ # or export ACW_TOKEN=...
  --repos   ~/Code                       # scanned two levels deep for git checkouts
```

On start the worker:

1. runs `claude --version`, then a trivial `claude -p` (60 s timeout) to confirm the CLI is signed in;
2. runs `codex --version` and `ollama --version` / `ollama list` if present;
3. lists git checkouts under `--repos` (e.g. `golf/pgs-web`), re-scanned on every heartbeat;
4. registers all of that as capabilities and heartbeats every 15 s;
5. polls every 2 s for a queued run whose repo it has.

`acw detect --repos ~/Code` prints the capabilities as JSON without connecting anywhere.

Useful worker flags: `--permission-mode` (default `acceptEdits`; use `bypassPermissions`
if you want the agent to run shell commands unattended, `default` for read-only tools),
`--max-turns N`, `--claude-arg <extra>` (repeatable), `--id`/`--name`,
`--skip-sign-in-check`.

### First open and onboarding

The page is served at `/`. On first open it asks for `ACD_DEV_KEY` and explains where it
comes from. With no machine online it shows "Connect your first machine": the build and
`acw run` commands with this control plane's address and worker token filled in (read from
`GET /v1/setup`, developer-key only), and it flips to the compose box the moment the first
heartbeat arrives. The Machines page keeps the same install line with a Copy button.

The lazy way to add a machine: the page also offers a one-paragraph prompt to paste into
Claude Code on that machine. It points the agent at `GET /connect.md` (public, no secrets),
which walks it through checking the sign-in, installing Rust, building `acw`, starting it with
the token in the environment, and installing a launchd or systemd user service so it survives
reboots. The token travels only in the pasted prompt.

### Browser auth

The page asks for the developer key once and keeps it in `localStorage`. It talks to the
API with an `Authorization: Bearer` header, including for the event stream (SSE over
`fetch`, because `EventSource` cannot send headers). If you expose `acd` beyond a
private network, put TLS in front of it.

## End-to-end walkthrough

1. Start Postgres and `acd` on machine A; open `http://<A>:8080/` and paste `ACD_DEV_KEY`.
2. Start `acw run ...` on machine B. Within a couple of seconds the Workers panel shows B
   with its Claude Code version, "signed in", and its repo count; the Repo dropdown fills
   with B's checkouts.
3. Type a task, e.g. *"Summarise what this repository does in three sentences, then list
   the three largest source files."*, pick `claude-haiku-5-5` and a repo, press **Run**.
   The API returns `202` with the run id and the view shows `queued`.
4. Within one poll interval B claims the run (`running`, with B's id), spawns
   `claude -p <task> --output-format stream-json --verbose --model claude-haiku-5-5 --permission-mode acceptEdits`
   with `cwd` set to the repo, and forwards each stream-json line as it is read:
   - `started` (redacted command line, cwd, pid), `agent_init` (session id, model, tools)
   - `assistant_text` paragraphs
   - `tool_use` with the tool name and a redacted one-line summary of its input
   - `tool_result` truncated to 2 KB, collapsed in the UI
   - `result` with the final text, duration, turns and cost
5. Each event is appended to `run_events` with a per-run `seq`, broadcast to open SSE
   subscribers, and rendered in the browser. The e2e test asserts every event reaches a
   subscriber within a second of being stored; on a LAN it is typically tens of milliseconds.
6. Press **Stop** at any time: the run gets `stop_requested`, the worker sees it on its next
   event post or its 1 s control poll, sends `SIGTERM` to the agent's process group,
   escalates to `SIGKILL` after 1.5 s, and reports `stopped`.
7. Reload the page or open the run from "Recent runs": the stream replays from `seq 0`
   (`Last-Event-ID` resumes a dropped connection) and ends with an `end` event.

Same thing from the shell:

```bash
curl -s -X POST http://<A>:8080/v1/runs -H "Authorization: Bearer $ACD_DEV_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"task":"Summarise this repo in three sentences.","model":"claude-haiku-5-5","repo":"agentcloud"}'
# {"id":"<uuid>","status":"queued"}
curl -N http://<A>:8080/v1/runs/<uuid>/events -H "Authorization: Bearer $ACD_DEV_KEY"
curl -X POST http://<A>:8080/v1/runs/<uuid>/stop -H "Authorization: Bearer $ACD_DEV_KEY"
```

## API

Developer (bearer `ACD_DEV_KEY`):

| Method | Path | |
|---|---|---|
| `POST` | `/v1/runs` `{task, model, repo}` | `202 {id, status}`; model must be one of `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5` |
| `GET` | `/v1/runs?limit=50` | recent runs |
| `GET` | `/v1/runs/:id` | the run |
| `GET` | `/v1/runs/:id/events` | SSE; `id:` is the seq, `event:` is the kind, replays from 0 or `Last-Event-ID`+1, then live; ends with `end` |
| `POST` | `/v1/runs/:id/stop` | queued → `stopped` immediately; running → `stop_requested` until the worker confirms |
| `GET` | `/v1/workers` | workers with capabilities and `online` (heartbeat < 45 s) |
| `GET` | `/v1/setup` | control URL and the first worker token, for the page's connect command |

Worker (bearer one of `ACD_WORKER_TOKENS`): `POST /v1/worker/register`,
`POST /v1/worker/heartbeat`, `POST /v1/worker/claim` (`200` run or `204`),
`POST /v1/worker/runs/:id/events?worker_id=` (returns `{status, stop_requested}`),
`GET /v1/worker/runs/:id/control`, `POST /v1/worker/runs/:id/finish?worker_id=`.

Run status transitions: `queued → running → succeeded | failed | stopped`, and
`queued → stopped`. A running run whose worker stops heart-beating for 90 s is marked
`failed` by a reaper.

## Tests

```bash
docker compose up -d --wait   # integration tests create and drop acd_test_* databases
cargo test --workspace
```

- `core`: redaction (prefixes, bearer, key=value, JWT, PEM, JSON keys), stream-json mapping, 2 KB truncation
- `acd/tests/api.rs`: auth, lifecycle transitions, event ordering + SSE replay/resume, stop while running, control-plane-side redaction
- `acw/tests/agent.rs`: executor against `tests/fixtures/fake_claude.sh` (prints stream-json slowly): success, stop kills the whole process group, crash, agent error, missing binary
- `acw/tests/e2e.rs`: the real worker loop against an in-process control plane: claim, live streaming within a second, replay, stop while running

Set `TEST_DATABASE_URL` if Postgres is not at the default address.

## Known limitations

- **This drives your own Claude Code sign-in, headless.** The worker runs the `claude`
  CLI with whatever account is signed in on that machine, with `-p` (no interactive
  prompts) and, by default, `--permission-mode acceptEdits`. Usage counts against that
  account's plan. Check the terms of service that apply to your subscription for
  automated or unattended use before relying on this, and never share or resell access
  to the account through it. Anthropic's own Remote Control and cloud sessions cover a
  similar moment natively; this spike exists to prove the vendor-neutral, your-own-machines
  version.
- One run at a time per worker; no scheduling, placement or queue priorities.
- Codex and Ollama are detected and reported but not executed.
- The developer key is a single shared secret kept in `localStorage`; there is no TLS,
  no users, no roles. Keep `acd` on a private network.
- Tool permission prompts are not relayed: anything the permission mode does not allow is
  denied by the CLI and the agent carries on or gives up.
- No follow-up messages to a run; each run is a fresh `claude -p` session (the session id
  is recorded in `agent_init` / `result` for a future `--resume`).
- Redaction is pattern-based (known key prefixes, bearer headers, `key=value`, JWTs, PEM
  blocks, secret-looking JSON keys). It is a safety net, not a guarantee.
- Workers are trusted once they hold a token; a worker can only touch runs it claimed,
  but tokens are not scoped per repo.
