# Sidefleet

**Run coding agents on your own machines, from anywhere.**

Type a task on a web page, pick a model and a repo, and a machine you own picks it up: your desk
Mac, a Mac mini, a VPS. It runs Claude Code in its own git worktree and streams the agent's
narration back live; reply to keep the conversation going. No API keys: it drives the Claude Code
sign-in already on that machine. No open ports: machines only make outbound calls to your control
plane. One Rust binary, macOS and Linux.

![A task running on a VPS, streamed live to the page](docs/screenshots/run-live.png)

```
 browser ──HTTP/SSE──▶ sidefleet serve   (control plane: web page + API + Postgres)
                              ▲
                              │ outbound HTTP only (register, heartbeat, claim, events)
                              │
                       sidefleet run ──spawns──▶ claude -p "<task>" --output-format stream-json
                       (worker on your Mac, VPS, Linux box: cwd = the repo)
```

## Features

- **Your machines, one page**: every connected machine with its repos, agents, CPU and running
  tasks on a live fleet map; tasks go to any machine that has the repo, or pin one to a machine.
- **Isolated by default**: each task gets its own git worktree and branch (`sidefleet/<id>`), so
  several tasks on one repo run side by side without touching your checkout. Choose "In place"
  when a task needs installed dependencies or local config.
- **Live, and a conversation**: events stream over SSE within milliseconds, survive reloads and
  dropped connections, and replies resume the same Claude Code session like a chat.
- **Permission modes**: read and edit files (default), full access for unattended commands, or
  read only for questions.
- **Zero-config workers**: `connect` checks Claude Code and its sign-in, finds your git checkouts
  (repos match by their git remote, whatever the folder is called), and installs a launchd or
  systemd service that survives reboots. Or click "Copy prompt" and let Claude Code on the other
  machine connect itself.
- **Secrets stay put**: anything that looks like a credential is redacted on the worker and again
  on the control plane; tokens are stored as SHA-256 digests and never logged.

![The task box and recent runs](docs/screenshots/home.png)
![A finished thread, ready for a follow-up](docs/screenshots/run-thread.png)
![The Machines page](docs/screenshots/machines.png)

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/foodaka/side-fleet/main/install.sh | sh
```

Downloads a prebuilt `sidefleet` into `~/.local/bin` (no sudo), or builds from source when there
is no release for your platform.

<details>
<summary>Private repo? Install with the GitHub CLI or from a clone</summary>

While the repository is private, the raw URL returns 404. Use the GitHub CLI (`gh auth login`),
which the script then uses for the download:

```bash
gh api repos/foodaka/side-fleet/contents/install.sh -H "Accept: application/vnd.github.raw" | sh
```

Or clone over SSH and build from the checkout, never contacting GitHub:

```bash
git clone git@github.com:foodaka/side-fleet.git && sh side-fleet/install.sh
```

</details>

## Quick start

**1. Start the control plane** on the machine that will host the page. It needs Postgres:

```bash
docker run -d --name sidefleet-db --restart unless-stopped \
  -e POSTGRES_USER=sidefleet -e POSTGRES_PASSWORD=sidefleet -e POSTGRES_DB=sidefleet \
  -p 5432:5432 -v sidefleet-pgdata:/var/lib/postgresql/data postgres:16-alpine
sidefleet serve
```

```
Sidefleet control plane is running.

  Open the page:        http://192.168.1.20:7373/
  Developer key:        3f9c…
  Connect a machine:    sidefleet connect --control http://192.168.1.20:7373 --token 7b2e…
```

Keys are saved to `~/.sidefleet/server.env`. If the machine has a coding agent installed, `serve`
runs a worker on it too, so a single box is one command.

**2. Open the page** and paste the developer key.

**3. Connect more machines.** On any machine with Claude Code signed in, run the install line, then
the connect line the page shows you. The page switches to the compose box as soon as the first
heartbeat lands.

Across networks, put the control plane on [Tailscale](https://tailscale.com) and use its tailnet
IP: no port forwarding, no public exposure. Running it on a VPS or a spare machine is covered in
[HOSTING.md](HOSTING.md).

## Command line

```sh
sidefleet serve                       # control plane on :7373, plus a local worker
sidefleet serve --no-worker           # control plane only
sidefleet serve --install-service     # run serve as a background service
sidefleet connect --control <url> --token <t>   # register this machine, install the service
sidefleet connect … --name "Mac mini" --parallel 3 --permission-mode bypassPermissions
sidefleet status                      # saved settings, service and process state, log path
sidefleet disconnect                  # remove the service (settings kept)
sidefleet run                         # worker in the foreground with the saved settings
sidefleet detect                      # what this machine can run, as JSON
sidefleet update                      # pull the latest, reinstall, restart services
```

Every flag can also come from the environment (`SIDEFLEET_BIND`, `DATABASE_URL`,
`SIDEFLEET_DEV_KEY`, `SIDEFLEET_WORKER_TOKENS`, …), a `.env` in the current directory, or
`~/.sidefleet/server.env` / `worker.env`. `sidefleet <command> --help` lists them all. Worker logs
are in `~/.sidefleet/logs/`.

The HTTP API and the lifecycle of a run are documented in [docs/API.md](docs/API.md).

## Develop

```bash
docker compose up -d --wait      # Postgres for local runs and tests
cargo test --workspace           # integration tests create and drop sidefleet_test_* databases
cargo run -p sidefleet -- serve
```

The web UI's build output in `crates/server/web-dist/` is committed so Rust-only installs keep
working; run `pnpm build` in `web/` after changing it (CI fails if it is out of date). Tagging
`vX.Y.Z` builds release binaries for macOS (arm64, x86_64) and Linux (x86_64, arm64).

## Layout

- `crates/core`: shared models, stream-json → event mapping, redaction.
- `crates/server`: Axum control plane, Postgres via sqlx migrations; embeds the web app.
- `crates/worker`: machine detection, control-plane client, process-group executor, worker loop.
- `crates/cli`: the `sidefleet` binary.
- `web/`: the web UI, SvelteKit + shadcn-svelte. See [web/README.md](web/README.md).

## Known limitations

- **This drives your own Claude Code sign-in, headless.** Usage counts against that account's
  plan. Check the terms that apply to your subscription for automated or unattended use, and never
  share or resell access to the account through it. Anthropic's Remote Control and cloud sessions
  cover a similar moment natively; Sidefleet is the vendor-neutral, your-own-machines version.
- One shared developer key, no users or roles, no built-in TLS. Keep the control plane on a private
  network (LAN or Tailscale), or put TLS in front of it.
- Only Claude Code executes tasks; Codex and Ollama are detected but not run yet.
- Tool permission prompts are not relayed to the page; whatever the mode does not allow is denied.
- Worktrees start from the repo's `HEAD` without untracked or ignored files (`node_modules`,
  `.env`), and are kept until you remove them (`git worktree remove`, or delete the folder under
  `~/.sidefleet/worktrees` and run `git worktree prune`).
- A follow-up runs only on the machine that holds the session, and waits while it is offline.
- Redaction is pattern-based: a safety net, not a guarantee.
- Windows is not supported.

License: MIT.
