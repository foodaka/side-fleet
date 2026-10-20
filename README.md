# Sidefleet

Run coding agents on your own machines, from anywhere.

You type a task on a web page and pick a model. A machine you own that has Claude Code signed
in and the repo checked out picks it up, runs it, and streams the agent's narration back live.
No API keys: it drives the Claude Code sign-in that is already on that machine. No open ports:
machines only make outbound calls to your control plane.

```
 browser ──HTTP/SSE──▶ sidefleet serve   (control plane: web page + API + Postgres)
                              ▲
                              │ outbound HTTP only (register, heartbeat, claim, events)
                              │
                       sidefleet run ──spawns──▶ claude -p "<task>" --output-format stream-json
                       (worker on your Mac, VPS, Linux box: cwd = the repo)
```

## Install

One binary, no sudo, macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/foodaka/side-fleet/main/install.sh | sh
```

It downloads a prebuilt `sidefleet` into `~/.local/bin` when a release exists for your platform,
otherwise it builds from source (installing Rust if needed, a few minutes).

While the repository is private, GitHub returns 404 for that raw URL. Either make the repo public,
or install with the GitHub CLI signed in (`gh auth login`), which the script then uses for the
release download and the source clone:

```bash
gh api repos/foodaka/side-fleet/contents/install.sh -H "Accept: application/vnd.github.raw" | sh
```

Or clone it (over SSH with your GitHub key) and run the script from the checkout; it then builds
from that folder and never contacts GitHub:

```bash
git clone git@github.com:foodaka/side-fleet.git && sh side-fleet/install.sh
```

## Set up in three steps

**1. Start the control plane** on the machine that will host the page (your desk Mac, a Mac mini,
a VPS). It needs Postgres; the easiest is Docker:

```bash
docker run -d --name sidefleet-db --restart unless-stopped \
  -e POSTGRES_USER=sidefleet -e POSTGRES_PASSWORD=sidefleet -e POSTGRES_DB=sidefleet \
  -p 5432:5432 -v sidefleet-pgdata:/var/lib/postgresql/data postgres:16-alpine
sidefleet serve
```

On first run it generates a developer key and a worker token, saves them to
`~/.sidefleet/server.env`, and prints:

If that machine has a coding agent installed (Claude Code, Codex or Ollama), `serve` also runs
a worker on it, so a single box is one command (`--no-worker` turns that off; checkouts are
found under the home folder, `--repos` overrides). Only Claude Code executes tasks in this version; the others are
reported as capabilities.

```
Sidefleet control plane is running.

  Open the page:        http://192.168.1.20:7373/
  Developer key:        3f9c…
  Connect a machine:    sidefleet connect --control http://192.168.1.20:7373 --token 7b2e…
```

**2. Open the page** and paste the developer key. You land on "Connect your first machine".

**3. Connect a machine.** On any machine with Claude Code signed in (the control plane machine
itself is fine), run the install line, then the connect line the page shows you:

```bash
sidefleet connect --control http://192.168.1.20:7373 --token 7b2e…
```

`connect` checks Claude Code and its sign-in, finds your git checkouts (home folder, three levels
deep, skipping hidden and build folders; `--repos <folder>` overrides), registers
once, saves the settings to `~/.sidefleet/worker.env`, and installs a background service
(launchd on macOS, a systemd user unit on Linux) so the worker keeps running and comes back after
reboots. The page flips from onboarding to the compose box the moment the first heartbeat lands.

Even lazier: the page has a "Copy prompt" button. Paste that into Claude Code on the other
machine and it does steps 2 and 3 for you by reading `http://<control plane>/connect.md`.

Then type a task, pick a model and a repo, press the arrow, and watch it run.

### Reaching the control plane from other machines

Workers need to reach the address you gave `--control`. On one LAN the printed IP works. Across
networks, put the control plane on [Tailscale](https://tailscale.com) and use its tailnet IP; no
port forwarding, no public exposure. If you do expose it publicly, put TLS in front of it.

### Worker commands

| Command | What it does |
|---|---|
| `sidefleet connect --control <url> --token <t> [--repos <folder>] [--name "Mac mini"] [--permission-mode …] [--no-service]` | validate, register, save settings, install the service |
| `sidefleet status` | saved settings (token masked), service installed, process running, log path |
| `sidefleet disconnect` | remove the service (settings kept) |
| `sidefleet run` | run the worker in the foreground using the saved settings |
| `sidefleet detect` | print what this machine can run, as JSON |

Permission mode defaults to `acceptEdits`: the agent can read and edit files but shell commands
are auto-denied. Use `--permission-mode bypassPermissions` on a machine where you want the agent
to run commands unattended. Logs: `~/.sidefleet/logs/worker.log`.

### Server options

`sidefleet serve` takes `--bind` (default `0.0.0.0:7373`), `--database-url`, `--dev-key`,
`--worker-tokens` (comma-separated, one per machine if you want to revoke individually),
`--no-worker`, `--repos` and `--permission-mode` for the built-in worker. All are
also read from the environment (`SIDEFLEET_BIND`, `DATABASE_URL`, `SIDEFLEET_DEV_KEY`,
`SIDEFLEET_WORKER_TOKENS`), from a `.env` in the current directory, and from
`~/.sidefleet/server.env`. Tokens are compared as SHA-256 digests in constant time and never logged.

## How a run works

1. `POST /v1/runs` stores the run as `queued` and the page opens its live view.
2. A worker that has that repo claims it (`running`), spawns
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

## API

Developer (bearer `SIDEFLEET_DEV_KEY`):

| Method | Path | |
|---|---|---|
| `POST` | `/v1/runs` `{task, model, repo}` | `202 {id, status}`; model is one of `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5` |
| `GET` | `/v1/runs?limit=50` | recent runs |
| `GET` | `/v1/runs/:id` | the run |
| `GET` | `/v1/runs/:id/events` | SSE; `id:` is the seq, `event:` is the kind; replays then live; ends with `end` |
| `POST` | `/v1/runs/:id/stop` | queued → `stopped` at once; running → `stop_requested` until the worker confirms |
| `GET` | `/v1/workers` | machines with capabilities and `online` (heartbeat < 45 s) |
| `GET` | `/v1/setup` | control URL and the first worker token, for the page's connect command |

Public: `GET /` (the page), `GET /connect.md` (instructions for an agent), `GET /healthz`.

Worker (bearer one of `SIDEFLEET_WORKER_TOKENS`): `POST /v1/worker/register`,
`POST /v1/worker/heartbeat`, `POST /v1/worker/claim`, `POST /v1/worker/runs/:id/events`,
`GET /v1/worker/runs/:id/control`, `POST /v1/worker/runs/:id/finish`.

## Developing

```
crates/core     shared models, stream-json → event mapping, redaction   (sidefleet-core)
crates/server   Axum control plane, Postgres via sqlx migrations, static page (sidefleet-server)
crates/worker   detection, control-plane client, process-group executor, loop (sidefleet-worker)
crates/cli      the `sidefleet` binary: serve / connect / run / status / disconnect / detect
```

```bash
docker compose up -d --wait      # Postgres for local runs and tests
cargo build --workspace
cargo test --workspace           # integration tests create and drop sidefleet_test_* databases
cargo run -p sidefleet -- serve
```

Tests cover redaction, stream mapping and truncation (core); auth, lifecycle transitions, event
ordering with SSE replay and resume, stop signalling and control-plane redaction (server); the
executor against a fake stream-json agent including killing the whole process group on stop, and a
real worker loop end to end against an in-process control plane (worker). CI runs them on every
push; tagging `vX.Y.Z` builds release binaries for macOS (arm64, x86_64) and Linux (x86_64, arm64).

## Known limitations

- **This drives your own Claude Code sign-in, headless.** The worker runs the `claude` CLI with
  whatever account is signed in on that machine, with `-p` and no interactive prompts. Usage counts
  against that account's plan. Check the terms of service that apply to your subscription for
  automated or unattended use before relying on this, and never share or resell access to the
  account through it. Anthropic's own Remote Control and cloud sessions cover a similar moment
  natively; Sidefleet exists for the vendor-neutral, your-own-machines version.
- One run at a time per machine; no scheduling, priorities or placement beyond "has the repo".
- Codex and Ollama are detected and shown but not executed yet.
- A single shared developer key, no users or roles, no TLS. Keep the control plane on a private
  network (LAN or Tailscale).
- Tool permission prompts are not relayed to the page; whatever the permission mode does not
  allow is denied and the agent carries on or gives up.
- No follow-up messages to a run; each run is a fresh `claude -p` session (the session id is kept
  in the events for a future `--resume`).
- Redaction is pattern-based. It is a safety net, not a guarantee.
- Windows is not supported.
