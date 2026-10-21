# Sidefleet

Run coding agents on your own machines, from anywhere.

Type a task on a web page and pick a model. A machine you own picks it up, runs Claude Code
in the repo, and streams the agent's narration back to the page live. When it finishes, reply
to keep going in the same session.

- **No API keys.** It drives the Claude Code sign-in already on each machine.
- **No open ports.** Machines only make outbound calls to your control plane.
- **Your checkout stays clean.** Each task gets its own git worktree and branch.

![A task running on a VPS, streamed live to the page](docs/screenshots/run-live.png)

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

This puts `sidefleet` in `~/.local/bin`. It downloads a prebuilt binary when a release exists
for your platform; otherwise it builds from source (installing Rust if needed, a few minutes).

<details>
<summary>While the repository is private</summary>

GitHub returns 404 for the raw URL above. Use the GitHub CLI instead (`gh auth login` first);
the script then uses it for the release download and the source clone:

```bash
gh api repos/foodaka/side-fleet/contents/install.sh -H "Accept: application/vnd.github.raw" | sh
```

Or clone over SSH and run the script from the checkout. It builds from that folder and never
contacts GitHub:

```bash
git clone git@github.com:foodaka/side-fleet.git && sh side-fleet/install.sh
```

</details>

## Quick start

### 1. Start the control plane

Pick the machine that will host the page (your desk Mac, a Mac mini, a VPS). It needs Postgres;
the easiest way is Docker:

```bash
docker run -d --name sidefleet-db --restart unless-stopped \
  -e POSTGRES_USER=sidefleet -e POSTGRES_PASSWORD=sidefleet -e POSTGRES_DB=sidefleet \
  -p 5432:5432 -v sidefleet-pgdata:/var/lib/postgresql/data postgres:16-alpine
sidefleet serve
```

On first run it generates a developer key and a worker token, saves them to
`~/.sidefleet/server.env`, and prints:

```
Sidefleet control plane is running.

  Open the page:        http://192.168.1.20:7373/
  Developer key:        3f9c…
  Connect a machine:    sidefleet connect --control http://192.168.1.20:7373 --token 7b2e…
```

If this machine has a coding agent installed (Claude Code, Codex or Ollama), `serve` also runs a
worker on it, so a single box needs only this one command. `--no-worker` turns that off;
checkouts are found under the home folder unless you pass `--repos`. Only Claude Code
executes tasks in this version; Codex and Ollama are reported as capabilities.

### 2. Open the page

Open the printed URL and paste the developer key. You land on "Connect your first machine".

### 3. Connect more machines

On any machine with Claude Code signed in, run the install line, then the connect line the page
shows you:

```bash
sidefleet connect --control http://192.168.1.20:7373 --token 7b2e…
```

`connect` does the following:

1. Checks Claude Code and its sign-in.
2. Finds your git checkouts: the home folder, three levels deep, skipping hidden and build
   folders. `--repos <folder>` overrides this.
3. Registers the machine and saves the settings to `~/.sidefleet/worker.env`.
4. Installs a background service (launchd on macOS, a systemd user unit on Linux), so the worker
   keeps running and comes back after reboots.

The page switches from onboarding to the task box as soon as the first heartbeat lands.

**Shortcut:** the page has a "Copy prompt" button. Paste that prompt into Claude Code on the
other machine and it does the install and connect steps for you by reading
`http://<control plane>/connect.md`.

To host the control plane on a VPS or a spare machine, see [HOSTING.md](HOSTING.md). It
includes a prompt you can hand to Claude Code there to do the setup.

## Using it

![The task box and recent runs](docs/screenshots/home.png)

Type a task, then pick:

- **Model:** Opus, Sonnet or Haiku.
- **Repo:** matched by git remote, so the same project is found on every machine whatever its
  folder is called.
- **Machine:** any machine that has the repo (whichever is free first), or pin it to one.
- **Permissions:** read and edit files (the default), full access when the agent should run
  commands, or read only for questions.
- **Workspace:** its own branch (the default) or in place.

Press the arrow and watch it run.

**Own branch.** Each task gets its own git worktree and branch (`sidefleet/<id>`) under
`~/.sidefleet/worktrees` on that machine. Several tasks on the same repo run side by side without
touching each other or your checkout. When a task finishes, merge or push its branch.

**In place.** Use this for work that needs the real checkout, such as installed dependencies or
local config. In-place tasks run one at a time per repo.

A machine runs up to three tasks at once (`--parallel` changes this).

**Follow-ups.** When a task finishes, reply in the box at the bottom. The reply continues the
same Claude Code session on the same machine and branch, and the page shows the whole thread
like a chat.

![A finished thread, ready for a follow-up](docs/screenshots/run-thread.png)

**Machines.** The Machines page shows every connected machine: whether it is online, CPU, memory,
disk, busy task slots, the agents it has and its Sidefleet build, with a live map of heartbeats
and tasks.

![The Machines page](docs/screenshots/machines.png)

## Reference

### Reaching the control plane from other machines

Workers need to reach the address you gave `--control`. On one LAN the printed IP works. Across
networks, put the control plane on [Tailscale](https://tailscale.com) and use its tailnet IP: no
port forwarding and no public exposure. If you do expose it publicly, put TLS in front of it.

### Worker commands

| Command | What it does |
|---|---|
| `sidefleet connect --control <url> --token <t> [--repos <folder>] [--name "Mac mini"] [--permission-mode …] [--parallel 3] [--no-service]` | validate, register, save settings, install the service |
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

## API

Developer (bearer `SIDEFLEET_DEV_KEY`):

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

Worker (bearer one of `SIDEFLEET_WORKER_TOKENS`): `POST /v1/worker/register`,
`POST /v1/worker/heartbeat`, `POST /v1/worker/claim`, `POST /v1/worker/runs/:id/events`,
`GET /v1/worker/runs/:id/control`, `POST /v1/worker/runs/:id/finish`.

## Developing

```
crates/core     shared models, stream-json → event mapping, redaction   (sidefleet-core)
crates/server   Axum control plane, Postgres via sqlx migrations; embeds the web app (sidefleet-server)
web/            the web UI: SvelteKit + shadcn-svelte; `pnpm build` writes crates/server/web-dist
crates/worker   detection, control-plane client, process-group executor, loop (sidefleet-worker)
crates/cli      the `sidefleet` binary: serve / connect / run / status / disconnect / detect
```

```bash
docker compose up -d --wait      # Postgres for local runs and tests
cargo build --workspace
cargo test --workspace           # integration tests create and drop sidefleet_test_* databases
cargo run -p sidefleet -- serve
```

The web UI lives in `web/` (see [web/README.md](web/README.md)). Its build output in
`crates/server/web-dist/` is committed, so Rust-only installs keep working; run `pnpm build`
in `web/` after changing it. CI fails if the committed build is out of date.

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
- Up to `--parallel` runs per machine (default 3); no priorities or placement beyond "has the repo".
- Worktrees start from the repo's current `HEAD` and do not include untracked or ignored files
  (`node_modules`, `.env`); a task that needs those must install them or run in place.
- Worktrees and their branches are kept until you delete them (`git worktree remove`, or remove
  the folder under `~/.sidefleet/worktrees` and run `git worktree prune`).
- Codex and Ollama are detected and shown but not executed yet.
- A single shared developer key, no users or roles, no TLS. Keep the control plane on a private
  network (LAN or Tailscale).
- Tool permission prompts are not relayed to the page; whatever the permission mode does not
  allow is denied and the agent carries on or gives up.
- A follow-up can only run on the machine that holds the session; if that machine is offline the
  reply waits as queued until it is back.
- Redaction is pattern-based. It is a safety net, not a guarantee.
- Windows is not supported.
