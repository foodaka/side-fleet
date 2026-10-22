# The hosted control plane

Sidefleet has two halves. The **worker** (`sidefleet connect` / `sidefleet run`) runs on your
machines, drives the Claude Code sign-in that is already there, and stays open source. The
**control plane** (`sidefleet serve`: the web page, the API and Postgres) is what you either host
yourself ([HOSTING.md](HOSTING.md)) or use as a service at `https://app.sidefleet.io`.

This document is about the second option: how the hosted, multi-tenant control plane runs, what
it stores, and how to try the multi-tenant mode locally.

## Modes

The same binary runs both ways. Which one depends on the environment:

| | Self-hosted (single-tenant) | Hosted (multi-tenant) |
|---|---|---|
| Turned on by | `SIDEFLEET_DEV_KEY` + `SIDEFLEET_WORKER_TOKENS` (generated on first run) | `SIDEFLEET_GITHUB_CLIENT_ID` + `SIDEFLEET_GITHUB_CLIENT_SECRET` |
| People sign in with | the developer key | GitHub |
| Data lives in | the built-in *default* org | one org per person (personal), plus team orgs |
| Machines connect with | a static worker token, or approval on the page | a per-org token, or approval on the page |
| Plan limits | none | free / pro, see below |

Both can be on at once: on a hosted control plane, a developer key and static tokens (if set) act
in the default org, which no GitHub user can see. Existing self-hosted installs keep working
unchanged; their data moves to the default org when migration `0014` runs.

## How it works

- **Sign-in.** `GET /auth/github/login` sends the browser to GitHub (scopes `read:user
  user:email`, nothing on repositories) with a random `state` kept in a short-lived cookie. The
  callback checks the state, exchanges the code, reads the profile, creates the user on first
  sign-in together with a personal org they own, and sets the `sf_session` cookie: random,
  `HttpOnly`, `SameSite=Lax`, `Secure` when `SIDEFLEET_PUBLIC_URL` is HTTPS, 30 days. The database
  keeps only its SHA-256. The GitHub access token is used for those two requests and dropped.
- **Orgs.** Every developer request resolves to one org: the developer key means the default org;
  a session means one of the user's orgs, chosen with the `X-Sidefleet-Org` header (id or slug)
  and defaulting to their personal org. The page sends the header from the org switcher, shown
  when someone belongs to more than one org. Owners can add members who have signed in once
  (`POST /v1/orgs/:id/members {login}`) and create team orgs (`POST /v1/orgs {name}`).
- **Isolation.** `workers`, `runs` and the chosen-repos table carry `org_id`, and every query
  filters on the caller's org. A run of another org reads as *not found* (404) for listing,
  reading, the thread, the event stream, stop and replies. Machines are keyed by `(org, id)`, so
  two orgs can both have a `MacBook-Pro`; a worker only claims, reports on, reads the control state
  of and finishes runs of its own token's org, whatever worker id it presents.
- **Worker tokens.** `POST /v1/orgs/:id/tokens {name}` returns `sfw_…` once; only its SHA-256 is
  stored. `GET` lists them (name, created, last used, revoked), `DELETE /v1/orgs/:id/tokens/:tid`
  revokes, and the next heartbeat with that token gets 401. Owners only (403 for members).
- **Approve-on-the-page enrollment.** `sidefleet connect --control https://app.sidefleet.io` with
  no token calls `POST /v1/enroll` with the machine's name and a capability summary (OS, agents,
  repo *count*, no paths). It prints a link and a code like `K7QD-M2XP`, then polls
  `GET /v1/enroll/:id` with a secret only it holds. On a hosted control plane the request belongs to
  no org: whoever opens the link (or types the code into Pending machines) and presses Approve
  takes it into their current org. On a self-hosted one it is listed on the Machines page at once.
  Approval mints a per-machine worker token, handed to the waiting `connect` exactly once; denial
  and the ten-minute expiry are reported to it too. Token-based `connect` works as before.
- **Plan limits.** Enforced on the server, never in the page:

  | Plan | Machines | Runs queued or running at once |
  |---|---|---|
  | free | 2 | 2 |
  | pro | 10 | 10 |
  | default org (self-hosting) | unlimited | unlimited |

  A new machine beyond the limit is refused at registration and at approval with **402**; a new
  run or reply beyond it with **402**; claims never start more runs than the plan allows. Removing
  a machine (`DELETE /v1/workers/:id`, "Remove" on an offline machine) frees its slot. There is no
  billing integration yet: the plan is the `orgs.plan` column, changed by an operator.

## What the hosted service stores

| Table | Contents |
|---|---|
| `users` | GitHub numeric id, login, primary verified email |
| `orgs`, `org_members` | org name, slug, plan; who belongs with which role |
| `sessions` | SHA-256 of the cookie, user, expiry |
| `worker_tokens` | org, name, SHA-256 of the value, created / last used / revoked times |
| `enrollments` | machine name and id, capability summary, code, SHA-256 of the poll secret, status; deleted a day after expiry |
| `workers` | machine id and name, capabilities reported on every heartbeat: agents and versions, Claude sign-in state, the list of git checkouts (folder name, path, normalised remote such as `github.com/acme/shop`), OS, hostname, CPU / memory / disk numbers, task slots, worker build |
| `runs` | the task text, model, repo identity, permission mode, status, timings, the final result (text, turns, duration, cost as Claude Code reports it), Claude session id |
| `run_events` | the agent's narration as it streams: assistant text, tool calls as a name plus a one-line redacted summary, tool results truncated to 2 KB, status changes |
| `added_repos` | which repos the org chose to show in the composer |

What it does **not** store or receive: repository contents beyond what the agent's narration
contains (its own text, and tool results capped at 2 KB each, which can include excerpts of files
the agent read or commands' output); API keys or Claude credentials (workers use the machine's own
Claude Code sign-in and never send it); worker token values, session cookie values or GitHub
access tokens (only digests, or nothing). Anything that looks like a credential is redacted on the
worker before it leaves the machine and again before it is stored; redaction is pattern-based, a
safety net rather than a guarantee. Runs and events are kept until deleted; there is no automatic
retention period yet.

## Deploying

The control plane ships as a container ([Dockerfile](Dockerfile)): a release build of
`sidefleet`, started as `sidefleet serve --no-worker`, listening on 8080, with `/healthz`
checking the database. [fly.toml](fly.toml) runs it on Fly.io with managed Postgres and Fly's TLS:

```bash
fly apps create sidefleet
fly mpg create --name sidefleet-db --region iad
fly mpg attach sidefleet-db --app sidefleet          # sets DATABASE_URL
fly secrets set SIDEFLEET_GITHUB_CLIENT_ID=… SIDEFLEET_GITHUB_CLIENT_SECRET=…
fly certs add app.sidefleet.io                       # then point DNS at the app
fly deploy --build-arg SIDEFLEET_GIT_SHA=$(git rev-parse --short=9 HEAD)
```

Create the GitHub OAuth app at github.com/settings/developers (or under the organisation that will
own the service): homepage `https://app.sidefleet.io`, callback
`https://app.sidefleet.io/auth/github/callback`.

Environment:

| Variable | |
|---|---|
| `DATABASE_URL` | Postgres 14 or newer. Migrations run on start. |
| `SIDEFLEET_GITHUB_CLIENT_ID`, `SIDEFLEET_GITHUB_CLIENT_SECRET` | the OAuth app; turn on hosted mode (`GITHUB_CLIENT_ID` / `GITHUB_CLIENT_SECRET` also work) |
| `SIDEFLEET_PUBLIC_URL` | `https://app.sidefleet.io`: OAuth redirect, connect commands, `Secure` cookies |
| `SIDEFLEET_BIND` | `0.0.0.0:8080` in the container |
| `SIDEFLEET_DEV_KEY`, `SIDEFLEET_WORKER_TOKENS` | optional on a hosted control plane; act in the default org (leave unset) |
| `RUST_LOG` | log filter; logs never contain tokens, cookies or task text beyond ids |

Run **one** instance. Live events reach open pages through an in-process broadcast, so a second
instance would serve pages that miss live updates from runs it does not own (they still appear on
reload). Fanning out through Postgres `LISTEN/NOTIFY` is the next step before scaling out.

`POST /v1/enroll` is public. It caps pending requests (20 per org, 5,000 in total) but has no
per-address rate limit yet; put the service behind a proxy that rate-limits it before a public launch.

Do not point this at, or restart, an existing self-hosted control plane that people rely on; give
the hosted service its own database.

## Local walkthrough: two GitHub users, two orgs, isolation and enrollment

What you need: Postgres (`docker compose up -d --wait`), a build of this branch
(`cargo build -p sidefleet`), Claude Code signed in on this machine, and two GitHub accounts (use
a private window for the second). Every command below sets `SIDEFLEET_CONFIG_DIR`, so the test
control plane and workers keep their settings in `/tmp/sf-*` and never touch `~/.sidefleet` (where
a real worker's settings may live).

1. **Create a GitHub OAuth app** for local use: homepage `http://localhost:7373`, callback
   `http://localhost:7373/auth/github/callback`. Note its client id and generate a secret.

2. **Start a hosted control plane** on its own database, without a built-in worker:

   ```bash
   docker compose exec postgres createdb -U sidefleet sidefleet_hosted
   SIDEFLEET_CONFIG_DIR=/tmp/sf-serve \
   DATABASE_URL=postgres://sidefleet:sidefleet@localhost:5432/sidefleet_hosted \
   SIDEFLEET_GITHUB_CLIENT_ID=<client id> SIDEFLEET_GITHUB_CLIENT_SECRET=<secret> \
   SIDEFLEET_DEV_KEY= SIDEFLEET_WORKER_TOKENS= \
   ./target/debug/sidefleet serve --no-worker --bind 127.0.0.1:7373
   ```

   It prints "Hosted mode". Open `http://localhost:7373/`: the page offers "Continue with GitHub".

3. **Alice signs in** (normal window). She lands in her personal org, `alice`, on the free plan,
   and sees "No machines yet".

4. **Bob signs in** (private window). He has his own org, `bob`. Neither sees the other's org.

5. **Alice connects a machine by approval.** In a terminal:

   ```bash
   export SIDEFLEET_CONFIG_DIR=/tmp/sf-alice
   ./target/debug/sidefleet connect --control http://localhost:7373 --no-service --name "Alice box"
   ```

   It prints `http://localhost:7373/#/machines?code=XXXX-XXXX` and waits. Alice opens that link:
   the Machines page shows "Alice box" under Pending machines with the same code. She presses
   Approve; the terminal prints `Approved into "alice"` and saves a per-machine token in
   `/tmp/sf-alice/worker.env`. Start the worker in the same terminal with `./target/debug/sidefleet run`. The machine goes online on Alice's page only, and
   her Worker tokens list now has an "Alice box" token.

6. **Bob connects a machine with a token.** In Bob's window, Machines → Worker tokens → name it
   "Bob box" → Create token, and copy the command (the value is shown once). In a second terminal
   run it as a foreground worker with its own settings folder:

   ```bash
   SIDEFLEET_CONFIG_DIR=/tmp/sf-bob ./target/debug/sidefleet run --control http://localhost:7373 --token sfw_… --name "Bob box"
   ```

   Both workers report the same hostname as their id; each org sees one machine.

7. **Isolation.** Alice starts a task on a repo both workers have checked out. Only Alice's
   machine picks it up (Bob's worker polls the same repo and gets nothing). Bob's Runs page stays
   empty; opening Alice's run URL in Bob's window shows nothing (the API answers 404 for the run,
   its thread, its event stream, stop and replies).

8. **Denial and expiry.** In a third terminal, with a fresh settings folder (in `/tmp/sf-alice`
   `connect` would simply reuse the token it saved), ask to join and press Deny on the page:

   ```bash
   SIDEFLEET_CONFIG_DIR=/tmp/sf-carol ./target/debug/sidefleet connect --control http://localhost:7373 --no-service
   ```

   The terminal says the request was denied. Run it once more and wait ten minutes: it says the
   request expired, and the code no longer finds anything on the page.

9. **Revocation and limits.** Bob revokes "Bob box": his worker's next heartbeat gets 401 and the
   machine goes offline. Alice creates a team org from the settings menu ("New org…"); the org
   switcher appears, and the new org starts empty. On the free plan a third machine is refused
   with "this org's plan allows 2 machines".

The same flow runs automatically, against a stand-in for GitHub, in
[crates/server/tests/tenancy.rs](crates/server/tests/tenancy.rs) and
[crates/worker/tests/enroll.rs](crates/worker/tests/enroll.rs).
