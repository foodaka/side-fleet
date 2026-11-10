# Sidefleet for Mac: spec

Status: built through phase 5 and compiled on GitHub's Mac runners; not yet used by hand against a
real control plane. See "Where this stands" below.
Evaluated against `main` at `5e168bb` on 2026-11-07, and brought level with the page at `fd75ee9` on 2026-11-10.

A native macOS app, written in Swift and SwiftUI, that does what the web page does and adds what
only a desktop app can: real notifications, a menu bar presence, a Dock badge, windows, keyboard
control. It talks to the same control plane with the same API. The web app stays: it is still how
you reach Sidefleet from a phone, and `sidefleet serve` still embeds it.

## Where this stands

| Part | State |
|---|---|
| `SidefleetKit`: API, streams, reducer, diffs, markdown, stores, demo | Builds under Swift 6 strict concurrency on Linux. 69 tests pass, 10 of them against a real `sidefleet serve`, and the diff and format code is held to the web code's real output. |
| `Sidefleet`: every screen in section 7, plus Rooms, Team, Chat apps and invites, menu bar, notifications, keyboard, sign-in | Compiles on macOS 15 runners and runs in demo mode there. **Not yet run by hand** against a real control plane. |
| App icon | Rendered and checked by eye. |
| Packaging, CI, demo-mode screenshots | Run on every push: the workflow uploads the app and a screenshot of each screen in light and dark. |

Question 1 was answered yes. Questions 3 and 4 are still open; the code assumes the recommendation for each.

Changes from the plan below: the stores moved into `SidefleetKit`, so far more of the app's logic is
tested. The app target stays in Swift 5 language mode for now, since strict concurrency errors can't
be seen from Linux. A third group of tests holds the Swift port to the web code's actual output.

## 1. Needs your call

These change the plan. Everything else is decided in section 3.

| # | Question | Recommendation |
|---|---|---|
| 1 | **May I push a branch so GitHub's macOS runners can build it?** This box is Linux, and SwiftUI only compiles on a Mac. Your MacBook is offline. Without a Mac build, the UI code is unproven and nobody has seen it rendered. The repo is public, so the code becomes public when pushed. | Yes. A `sidefleet/` branch runs `.github/workflows/macos.yml`, which builds, launches demo mode and uploads screenshots. It is the only way I can look at pixels and iterate on them. Or run `scripts/bundle.sh` on your Mac and send me the errors. |
| 2 | **Client only, or also the worker?** "This" could mean the page, or the whole product including running tasks on the Mac. | Client only. `sidefleet` already runs as a launchd service, and wrapping a Rust worker in the app adds a lot for little. The app can show whether this Mac is on the fleet and hand you the connect command. |
| 3 | **How do hosted users sign in?** The server sets an httpOnly cookie after GitHub OAuth and redirects to `/`, which a system browser can't hand back to an app. Option A: an embedded `WKWebView` sheet, reading the cookie from its store. Option B: a small server change, a one-time code redirect to `sidefleet://`. | A for now: no server change, and it works against every control plane already running. Password managers and passkeys are weaker in an embedded view, so B is the better long-term flow. |
| 4 | **Do you have an Apple Developer account?** Without Developer ID signing and notarization, downloads trip Gatekeeper, and Keychain re-prompts after every rebuild. | Tell me which. The build works either way; only distribution changes. Until then it's "build it yourself" or an ad-hoc-signed zip. |

## 2. What the evaluation found

**The architecture suits a native client.** `web/` is a thin client with no business logic the
server doesn't also enforce. It polls and streams; the control plane decides. Nothing about tasks,
agents or git happens in the client, so no server change is needed for a full client, apart from
question 3 if you choose B.

```
 Sidefleet.app ──HTTP/SSE──▶ sidefleet serve  ◀── outbound ── sidefleet run (workers)
 (new)                       (unchanged)                       (unchanged)
```

**Size of the port.** The client code that matters is about 9,800 lines of Svelte and TypeScript
in `web/src/lib/sf` and `web/src/routes`; the rest of the 13,245 lines is generic UI kit. Rough
estimate for Swift, leaving Rooms, Team and Chat apps for later: 3,000 lines of non-UI code and
8,000 of UI.

**What ports directly (logic to translate, with tests):**

| Web source | What it holds |
|---|---|
| `lib/sf/thread.svelte.ts` | The event reducer: `apply()` turns each stream event into a turn's items. The heart of the client. |
| `lib/sf/format.ts` | Agents, models, efforts, permissions, every time and number format |
| `lib/sf/state.svelte.ts` | Polling, derived fleet state, archive and snooze with rollback, org switching |
| `lib/sf/diff/parse.ts` | Patch parsing, word-level marks, split rows, whitespace hiding, diffstat blocks |
| `lib/sf/snooze.ts`, `prefs.svelte.ts`, `seen.svelte.ts`, `telemetry.svelte.ts` | Presets, send-key rules, unread marks, CPU history and fleet events |

**What has no direct equivalent and needs real work:**

- **Markdown while it streams.** The page uses Streamdown. SwiftUI's built-in markdown only does
  inline styles, so headings, lists, tables and code blocks need a block renderer.
- **Syntax highlighting in diffs.** The page uses Shiki, which is JavaScript. See section 3.
- **Text input.** `TextEditor` can't do this job: send-key rules need to respect IME composition,
  and pasting a screenshot has to attach it. It needs an `NSTextView` wrapper.
- **Long scrolling views.** Stick-to-bottom in a lazy stack is a known weak spot in SwiftUI, and
  it's the most-used behaviour in the app.

**Things the code does that the Swift client must not get wrong:**

- `assistant_live` frames have **no id** and aren't stored (`routes.rs:1030`). They must not move
  the resume position, and the next `assistant_text` replaces them.
- Frames are named after the event kind, so a run's `error` event and the stream's own error both
  arrive as `event: error`. The stream's own isn't JSON (`sse.ts`).
- Old control planes omit fields (`mine`, `guest`, `slots`, `system`) and new ones may add reasons
  and kinds. Decoding has to tolerate both; a strict `Codable` would break the whole list on one
  unknown value.
- Plain-HTTP control planes are normal: `attach.svelte.ts` notes they're "often reached over plain
  http (a Tailscale address)". App Transport Security blocks that by default.
- Rules from recent fixes that live only in the client's conditionals, such as who may update a
  machine (#75) and who may stop, restore, approve or open a PR on a teammate's task. Section 7
  lists them.

**API tokens can't drive this app.** `scope_allows` (`auth.rs:143`) gives `sfa_` tokens no access
to the inbox, `/v1/me/prefs`, `/v1/me/machines`, `PUT /v1/repos`, enrollments or `/v1/setup`. So
sign-in is the same two ways as the page: developer key or GitHub session.

**Small drift found in `docs/API.md`** (not fixed as part of this): it lists only full Claude model
ids, while the server takes aliases (`crates/core/src/agent.rs:28`). It also omits `/v1/runs/:id/pr`,
`/archive`, `/v1/repos`, `/v1/me/prefs`, `/v1/me/machines` and `/v1/workers/:id/update`.

**The ongoing cost.** There will be two clients. Each new web feature needs a Swift port or a
deliberate decision not to, and the model and effort tables in `format.ts` have to be bumped in
both places.

## 3. Decisions made

Say if you disagree with any of these.

| Topic | Decision | Why |
|---|---|---|
| Platform | macOS 14 and later, SwiftUI with AppKit where SwiftUI falls short | `@Observable` needs 14. Standard controls pick up the current system look on newer macOS by themselves. |
| Not a web clone | Native structure and controls, Sidefleet's colours and restraint | A wrapped-looking app is the failure to avoid. See section 8. |
| Project form | Swift Package in `macos/`, plus a script that assembles `Sidefleet.app` | Reviewable as text, buildable from the command line and CI. No `.xcodeproj` to merge. Xcode still opens the package. |
| Structure | `SidefleetKit` (Foundation only) and `Sidefleet` (UI) | All logic can be compiled and tested on Linux, which is what I have here. |
| Dependencies | One: Apple's `swift-markdown` for block parsing | Everything else is the system. Fewer things to keep updated. |
| Syntax highlighting | A small tokenizer in Swift for the top languages (TS/JS, Rust, Python, Go, Swift, shell, JSON, YAML, CSS/HTML, SQL), GitHub's light and dark colours, plain text otherwise | Tree-sitter is heavy, and running Shiki in JavaScriptCore is a web view by another name. It will be less exact than Shiki on hard cases. |
| Notifications | Local notifications driven by the inbox stream | Server push accepts browser push services only, and APNs needs a developer account and server work. So they fire only while the app runs, which is why the app keeps running in the menu bar. |
| One control plane at a time | Switch by signing out | Same as the page. Multiple accounts can come later. |
| Rooms, Team, Chat apps | Native, as of 2026-11-10 | Asked for: the app should do what the page does. |

### Non-goals

iOS or iPadOS, Windows or Linux desktop, running a worker or control plane inside the app, APNs,
widgets, Shortcuts, Spotlight, offline editing, and any server change (unless you pick 3B).

## 4. Architecture

```
macos/
  Package.swift
  Sources/
    SidefleetKit/          no UI; builds on macOS and Linux
      API/                 Client, Endpoints, Auth, SSEParser, EventStream
      Models/              Run, RunSummary, RunEvent, Worker, Inbox, Me, JSONValue, lenient decoding
      Thread/              Turn, Item, reducer (port of apply()), tool-group summaries
      Diff/                parser, word diff, split rows, stat blocks
      Markdown/            markdown -> block model
      Highlight/           tokenizer
      Format/              times, models, efforts, permissions, repo labels, snooze, SVG paths
      Stores/              Fleet, Thread, Inbox, Prefs, Seen, Toasts, the composer models
      Demo/                a pretend control plane, for demo mode, screenshots and tests
    Sidefleet/             SwiftUI app; macOS only
      App/                 scenes, commands, menu bar, notifications, deep links, session, Keychain
      Design/              tokens, status dot and pill, pills, gauges, logo, agent icons
      Sidebar/ Composer/ Thread/ Review/ Machines/ Overview/ Settings/ SignIn/
  Tests/SidefleetKitTests/
  Resources/               Info.plist, entitlements, icon sources
  scripts/bundle.sh        builds and assembles Sidefleet.app
```

- **Concurrency:** Swift 6 language mode. `Client` is an actor, stores are `@MainActor @Observable`.
- **One `FleetStore`** owns what `state.svelte.ts` owns. A `ThreadStore` per open thread owns its
  streams and is dropped when the thread closes, like `Thread.dispose()`.
- **Time labels:** the page ticks a global `now` every second. In SwiftUI that would re-render every
  row, so each time label sits in its own `TimelineView`.
- **Live text:** `assistant_live` can arrive quickly. Coalesce to at most 10 updates a second and
  re-parse markdown at that rate, not per frame.

## 5. Control plane contract

**Auth.** Every request sends one of:
- `Authorization: Bearer <developer key>`, and no org header.
- `Cookie: sf_session=<value>` and `X-Sidefleet-Org: <id>` once an org is chosen. Cookie handling on
  the session is off and the header is set by hand, so nothing is shared with a web view.

Credentials live in the Keychain. On a redirect to another origin the client doesn't forward them.
`401` goes to sign-in. `403` on a remembered org falls back to the first org, as `refresh()` does.

**Polling, every 5 s while the app is active, 30 s when it's in the background:**
`/v1/me`, `/v1/workers`, `/v1/runs?limit=100`, `/v1/repos`, `/v1/enrollments`, `/v1/me/machines`;
`/v1/setup` until loaded; `/v1/orgs/:org/members` about once a minute. Refresh at once on wake,
on activation and when a followed run ends.

**Streams** use a dedicated `URLSession` with long timeouts and a raised per-host connection limit,
because each live run holds one open and several windows could otherwise starve normal requests.

| Stream | Rules |
|---|---|
| `GET /v1/runs/:id/events` | Send `Last-Event-ID`, starting at `-1`. `id:` is the seq; only update it from frames that have one. Ends on `event: end`. Retry after 1.5 s and show "reconnecting…". |
| `GET /v1/inbox/events` | `event: inbox` with the full `{items}`. Stop on `401`. Retry after 3 s. |

Ignore comment keep-alives. Reconnect on `NSWorkspace.didWakeNotification`, since a sleeping laptop
is the normal case here.

**Actions**

| Action | Call |
|---|---|
| New task | `POST /v1/runs` `{task, agent, model, effort?, repo, permission_mode, isolation, worker_id?, failover, images, start_at?}` |
| Reply, or switch agent | `POST /v1/runs/:root/messages` `{task, agent?, model?, effort?, permission_mode?, images?}`. Absent `effort` keeps it, `""` is the agent's default. |
| Stop all | `POST /v1/runs/:id/stop` for every non-terminal run, newest first, so nothing queued starts |
| Send now | Stop only the running one |
| Start now, cancel scheduled | `POST /start-now`, `POST /stop` |
| Restore, fork, open PR | `POST /restore`, `POST /fork`, `POST /:root/pr` |
| Approve, deny | `POST /approvals/:id/approve`, `/deny` |
| Archive, snooze | `POST /archive {archived}`, `POST /snooze {until}`, optimistic with rollback and Undo |
| Inbox | `POST /v1/inbox/:id/dismiss`, `POST /v1/inbox/dismiss-all` |
| Machines | `POST /v1/workers/:id/update`, `DELETE /v1/workers/:id`, `POST /v1/enrollments/lookup`, `/approve`, `/deny` |
| Repos | `PUT /v1/repos {added}` |
| Worker tokens | `GET`/`POST`/`DELETE /v1/orgs/:org/tokens` |
| Prefs | `GET`/`PUT /v1/me/prefs`, synced keys only: `permission`, `roomPermission`, `workspace`, `sendKey`, `confirmStop`, `snoozePresets` |
| Sign-in discovery | `GET /v1/auth` `{github, dev_key, hosted}` |

**Errors.** Non-2xx bodies carry `{error}`; show that text. `402` is a plan limit, `409` a state
conflict, `410` an expired approval. `204` and an empty `202` have no body.

**Decoding.** Payloads are free-form, so `RunEvent.payload` is a `JSONValue` enum. Unknown statuses,
reasons and kinds decode to `.other(String)` and display their label. Timestamps come from Rust's
`DateTime<Utc>`, usually with more fractional digits than the standard formatter reliably parses, so
they get a dedicated parser, tested against real server output.

**Images.** `GET /v1/images/:id` needs auth, so `AsyncImage` can't be used. A small loader caches
them; they never change. Limits for attaching: 8 images, 10 MB each, 40 MB per message; PNG, JPEG,
GIF or WebP. Clipboard TIFF becomes PNG, and HEIC from an iPhone becomes JPEG (the page rejects
HEIC; here it just works).

## 6. Thread model

Port `apply()` from `thread.svelte.ts` kind by kind, including the wording of its notes. Tests replay
event sequences recorded from a real control plane and assert the resulting turns.

| Event | Effect on the turn |
|---|---|
| `status` | Run status and times. `moved_to` adds a note and follows the new run. Finishing cancels pending approvals and clears live text. |
| `workspace`, `started`, `agent_init`, `checkpoint`, `snapshot` | Branch or note, command, session and model, checkpoint sha, restore points |
| `assistant_thinking`, `assistant_live`, `assistant_text` | Activity line; live text; a text item that replaces the live text |
| `tool_use`, `tool_result` | Tool item, matched by `tool_use_id`, with output, error flag, truncation, images |
| `approval_requested`, `approval_decided` | Approval item and its outcome |
| `result` | Duration, turns, cost or tokens, context fill |
| `diff` | `{turn, total}` |
| `restored`, `pull_request`, `error`, `omitted`, `stderr` | Notes, with a link for the PR; stderr groups into a log |

Replay finished runs in order and follow live ones side by side, as `Thread.load()` does. Consecutive
tools fold into one line ("Ran 2 commands, read a file") using the table in `ToolGroup.svelte`.
Failed tools open by themselves.

## 7. Screens

Copy is carried over word for word: the page's voice is part of the product. Sizes are in points
and match the page's rendered sizes unless noted.

**Window.** `NavigationSplitView`: a 266 pt sidebar, collapsible, and a detail column. Default window
1280×820, minimum 900×600. Lost contact shows the page's amber banner at the top of the detail:
"Lost contact with the control plane. Retrying every few seconds; what you see may be out of date."

**Sign-in** (`SignIn.svelte`). Asks for the control plane URL first (new, since the page knows its
own origin), defaulting to the hosted one, then calls `/v1/auth`. Offers GitHub and/or a developer
key accordingly. Warns if plain HTTP is going to a public address. Animated logo while connecting.

**Sidebar** (`Sidebar.svelte`, `RunList.svelte`). Logo and name, inbox bell, org switcher, New task,
search, then the list, then nav.
- Sections: Active (with a count), Today, Yesterday, Previous 7 days, Older, then collapsed Snoozed
  and Archived. Archived loads on first open.
- Active sorts: waiting on you, running, queued, scheduled soonest first.
- Row: status dot, title, then a detail line. Active rows show machine and duration, "Waiting on
  you · 2 tool calls", "Scheduled · Tue 07:00" or "Waiting for a machine". Others show repo (only if
  it isn't the most common one), machine (only if there are several), model and agent icon.
- Markers: a coloured dot when waiting on you, an ink dot when unread and finished, PR number and
  icon when there is one. Finished and read runs dim their dot.
- Context menu in place of the hover ⋯: Fork, Snooze with presets and wake times, Pick a date and
  time, Archive or Unarchive, Open in New Window.
- Mine and Team switch when the org has several members. The choice sticks.
- Search covers title, repo and machine, plus archived once loaded.
- Nav: Overview, Machines with `● 2/3` and an update icon, Rooms, Team (GitHub sign-in only), Chat
  apps, Settings.

**New task** (`routes/+page.svelte`). "What should run?" centred, 672 pt column, about 14% from the top.
- Growing text box up to 55% of the window height. Placeholder and character count (over 2,000) as on the page.
- Pills: agent and model picker, effort, repo, options (machine, permissions, workspace, "Move if
  it drops"), start later, attach, send. The options control is an icon at its defaults and names
  what differs when changed.
- The line under the box says exactly what happens: "Runs with Claude Code on hetzner-vps with full
  access, on a new branch so your checkout stays untouched." It covers every branch in the source,
  including scheduling, failover, sign-in warnings and no machine having the agent.
- Live CPU, memory and disk chips when one machine is the target.
- Remembered per agent: model and effort. Remembered: agent, repo, machine. Not remembered: failover,
  start time. Permission and workspace start from Settings each time.
- Selection stays valid as machines come and go; if no machine with the repo has the agent, switch
  agent without remembering it.
- First-run steps and example tasks when there are no runs. "Put your first machine to work" and
  "Your machines are offline" replace the box when appropriate.

**Thread** (`routes/runs/[id]/+page.svelte`, `TurnView.svelte`). Header: title (up to 3 lines), status
pill with a live duration, owner when several members, room, machine with dot, repo, branch with copy,
PR state, message count, forked-from link, changes button.
- Turn: user bubble in ink, right-aligned, except the first message (the title is the first message).
  Attached images. A line of meta. "Started on … with …", which expands to the exact command.
- Items: markdown, tool groups, approval cards, notes, stderr log.
- Activity line with a breathing dot and shimmer: "Thinking", "Ran…", "Waiting for you to approve".
- End of turn: changes card, stats ("431.0s  18 turns  $1.1200"), Restore to before (asks for a
  second click within 4 s), Fork from here.
- Queued messages show "Send now" and "Cancel". Scheduled tasks show "Start now" and "Cancel".
- End of thread: the dashed note about the branch, with Open pull request or Push changes.
- Stays pinned to the bottom unless you scroll up; a button returns you. Opening a follow-up's id
  lands on its root.

**Reply composer.** Agent, model, effort, "Same permissions", hint line, context ring, attach, send.
- While running with an empty box, Stop replaces Send (asking first, per Settings). With a draft it
  is Queue.
- Switching agent changes the placeholder and hint. It's disabled when there's no session to continue.

**Teammate's task.** No composer: "This is @x's task. It runs on their machine, so only they can
reply to it." with Continue on my machine. No restore, start-now or approval buttons. Owners get Stop.

**Approval card** (`ApprovalCard.svelte`). "Claude Code wants to run a command", the command in full,
Approve, Deny, and a countdown. Folds to one line once decided, naming who decided.

**Fork** (`ForkDialog.svelte`) and **Stop** (`StopDialog.svelte`) are sheets. Fork's explanation and
machine list change depending on whose message it is and whether its files were kept. Stop says how
many queued messages it drops, and has "Don't ask again".

**Review** (`diff/`). Takes over the detail column, with Esc to go back and an option to open in its
own window. The page uses a nearly full-width drawer, which on a Mac would just cover the thread.
- File list on the left, with status, stat and viewed tick. Scope menu: all changes or per message.
- Unified or split, hide whitespace, open collapsed. Word-level marks, line numbers, hunk headers with
  skipped-line counts, per-file collapse.
- Omitted files give their reason. "Caught up with main" is noted.
- Keys: `j`/`k` file, `v` viewed, `w` whitespace. "3 of 7 viewed" in the footer.

**Machines** (`routes/machines`, `MachineCard.svelte`, `FleetMap.svelte`). Summary line, Choose repos,
Add machine. Pending machines. Fleet map. Cards, yours first, then online, then by name. Worker tokens.
- Card: name, OS, uptime, online state. CPU number and sparkline of the last 40 readings. Memory and
  disk gauges. Task slots with moving stripes when busy, and what's running. Agent chips with sign-in
  state and tooltips, usage limit chips, repo count. Footer: build, Update to latest, "Updating when
  idle" or Remove.
- Map: `Canvas` in a `TimelineView`, the same orbits and node sizes. Green dot for a heartbeat, violet
  for a task. Motion only means something happened. Stops when not visible.
- Update only when the machine is yours, or it has no owner and you own the org.

**Overview** (`routes/runs/+page.svelte`). Four stats, filter pills, a native `Table` of tasks with
sortable columns (the page's list isn't sortable).

**Settings** (`routes/settings`). A standard Settings scene (⌘,) with tabs: New tasks, Sending (with
the try-it box), Reviewing, Snooze presets, Confirmations, Appearance, Account. Adds notification
choices and "keep running in the menu bar".

**Others.** Inbox popover from the bell, the same rows and Clear all. Repo picker sheet with search
and space to tick. Add machine with terminal and agent tabs; the connect command masks its token and
the copy doesn't. New org, rename org.

**Rules that must carry over**

| Rule | Source |
|---|---|
| Machines only run their owner's tasks, so the composer offers only yours | `+page.svelte` |
| Only Claude Code has "Ask me"; other agents fall back to edit | `permsFor` |
| Codex permissions have different hints | `permHint` |
| A task counts as seen only while you can see it: key window and app active | `+layout.svelte`, `inbox.visible` |
| Opening a task marks it read; first launch marks everything read | `seen.svelte.ts` |
| Switching org clears everything and reloads | `switchOrg` |
| Notifications carry title and reason only, never agent output | README |

## 8. Design

The product's own tokens say it best: "Status colours carry meaning only." The look is warm grey,
near-black ink and very little else. Nothing is coloured to decorate.

**Native conventions over web ones:** system font (SF Pro and SF Mono, not Inter), a real vibrant
sidebar, unified toolbar, native menus, sheets, popovers and tooltips, standard focus rings, native
context menus instead of hover ⋯ buttons. Toasts stay, because macOS has no equivalent for Undo: a
small bottom-right overlay.

**Colour tokens**, in an asset-free `Color` set with light and dark values from `layout.css`:

| Token | Light | Dark | Meaning |
|---|---|---|---|
| `background` | `#f7f7f5` | `#161615` | Ground |
| `card` | `#ffffff` | `#1d1d1b` | Surfaces |
| `ink` | `#191919` | `#ececE8` | Text, primary buttons, the app's tint |
| `muted` | `#6b6a65` | `#a09f99` | Secondary text |
| `faint` | `#8f8e88` | `#7c7b75` | Tertiary text |
| `border` | `#e6e6e2` | `#2c2c29` | Hairlines |
| `code` | `#f3f3f0` | `#1a1a18` | Code ground |
| `run`, soft, ink | `#8b5cf6` `#f1eafb` `#5b3aa8` | `#a78bfa` `#2a2238` `#c9b5fb` | Running |
| `queue`, soft, ink | `#f08a2e` `#fdf1e6` `#94470a` | `#f5a25a` `#33261a` `#f7c08a` | Queued, waiting, limits |
| `ok`, soft, ink | `#13a05a` `#e6f6ee` `#0b6b3a` | `#3ccf86` `#17291f` `#7fe0ae` | Done, online |
| `err`, soft, ink | `#e5484d` `#fdecec` `#a32323` | `#f2555a` `#331b1c` `#f8a0a3` | Failed, stopped |
| `off` | `#b4b2a9` | `#5b5a55` | Offline |
| `diffAdd`, `diffDel` | `#2da44e` `#e5484d` | `#3fb950` `#f85149` | Tints blended into `card` at the page's percentages |
| `merged` | `#7c3aed` | `#b794f6` | Merged PR |

The sidebar uses the system's material rather than `#f7f7f5`, with a neutral selected row in place
of the system's blue, so it feels like macOS and still like Sidefleet.

**Type** (points; `.monospacedDigit()` wherever numbers change)

| Use | Size |
|---|---|
| Page heading ("What should run?") | 28 medium, tight tracking |
| Screen title, thread title | 22 semibold, 19 medium |
| Thread prose | 14.5, line height about 1.7 |
| Composer | 15 for a new task, 14 for a reply |
| Row title, controls | 13 |
| Row detail, meta | 11.5 to 12 |
| Section heading | 11 medium, `faint` |
| Code and diffs | 12 SF Mono, 20 pt lines |

**Shape.** Cards 14, composer 18, pills fully round, one-pixel hairlines. Soft shadows only on the
composer and popovers. Column widths 672 (composer, thread) and 1008 (Machines, Overview).

**Motion**, kept rare, and all of it off with Reduce Motion:
- The running dot breathes, 1.8 s: the page calls it "the one ambient motion in the app, and it means live".
- The shimmer on the activity line, and moving stripes on busy slots.
- Quick and small: rows entering, tool groups opening, a chevron turning. 150 to 250 ms, no bounce.
- The logo's pulse only on sign-in and while connecting, not forever in the sidebar as on the page.

**Marks.** The logo's strands come from `Logo.svelte` as `Path`s. The app icon starts from
`site/brand/app-icon.svg` but needs regenerating: that one is full-bleed, whereas a Mac icon sits on
Apple's grid with margins and a shadow (about 824 in 1024). The menu bar uses a template image from
`mark-small.svg`. Agent icons come from `AgentIcon.svelte`. Other icons are SF Symbols.

**Accessibility.** VoiceOver labels on every dot and icon-only button, carrying what the page's
`aria-label`s say. Status never by colour alone. Dynamic contrast in both themes. Full keyboard use.

## 9. What only the native app does

**Menu bar.** The mark, with a count when something waits. The menu: Waiting on you, Running with
live durations, New Task, Open. Closing the window keeps the app alive, which is what keeps
notifications working; this can be turned off in Settings.

**Notifications.** Fire when an inbox item newly appears, and when `pending_approvals` rises. Clicking
opens the task. Suppressed if you're already looking at it. Never on first load, so launching doesn't
produce a burst. Permission is asked when you first turn them on, not at launch.

**Dock.** Badge shows the inbox count.

**Windows.** A thread or review in its own window, restored on relaunch.

**Links.** `sidefleet://runs/<id>`, used by notifications.

**Keyboard**

| Keys | Action |
|---|---|
| ⌘N | New task |
| ⌘F | Search tasks |
| ⌘⇧I | Inbox |
| ⌘1 to ⌘3 | Overview, Machines, New task |
| ⌘⌃S, also ⌘B as on the page | Toggle sidebar |
| ⌥⌘↑, ⌥⌘↓ | Previous or next task in the sidebar |
| ⌘⏎ | Send (always); ⏎ follows the send-key setting |
| ⌘. | Stop |
| ⌘⇧D | Review changes |
| ⌘⇧C | Copy branch name |
| ⌘, | Settings |
| Esc | Leave review, clear search |
| `j` `k` `v` `w` | In review, as on the page |

Enter never sends during IME composition. All of these appear in the menu bar, so they can be found.

**Drag and paste.** Images dropped anywhere on a composer's screen, or pasted. Click an inline image
to preview it with Quick Look.

**Security details that differ from a browser**
- Agent output is untrusted. Links open in the browser only for `http`, `https` and `mailto`.
- Remote images in agent markdown don't load by themselves, since that would let a repo's content
  ping an outside server. They show as links. Images stored on the control plane do load.
- `NSAllowsArbitraryLoads` is needed for plain-HTTP self-hosting, with the warning at sign-in.
- Logs never include tokens, cookies or task content.

## 10. How it gets verified

**What I can and can't verify from here.** This box is Linux. I can compile and run `SidefleetKit`,
but I can't compile SwiftUI or see it. So the split below is a real limit, not a formality.

| Layer | Where | How |
|---|---|---|
| Reducer, parser, diff, formats, decoding | Linux, Docker `swift` image (image is available; build not yet tried) | Unit tests, including word-diff cases cross-checked against `parse.ts` |
| Real API and streams | Linux | Integration tests against a real `sidefleet serve` with a throwaway Postgres: create a run, stream with reconnect, inbox, approvals. A fake worker posts events. |
| SwiftUI code compiles | macOS CI or your Mac | `swift build`. A stub SwiftUI module on Linux would catch only my own typos, so it's a stopgap at most. |
| How it looks | macOS CI or your Mac | Demo mode (`--demo`): fixtures and made-up machines and tasks, no network. A script walks the screens in light and dark, screenshots go to CI artifacts, and I read them and iterate. |
| Real behaviour | Your Mac | Against your control plane: notifications, Keychain, menu bar, sleep and wake, GitHub sign-in. These can't be checked in CI. |

The screenshot step on GitHub's runners is itself unproven. If screen capture doesn't work there, the
fallback is rendering windows in-process, which doesn't show materials accurately.

**Not done until:**
- A task can be started, followed live, replied to, stopped, approved, reviewed, restored and forked
  in the app against a real control plane.
- A 5,000-line diff and a 1,000-item thread scroll smoothly. If `LazyVStack` can't, those views move to
  `NSTableView`.
- Dropping the network or sleeping the Mac mid-run recovers without missing or repeating an event.
- Every screen has been looked at in light and dark, and with the window at its minimum size.
- The app is usable with the keyboard alone, and with VoiceOver on the main flows.

## 11. Packaging and CI

- `macos/scripts/bundle.sh` builds release, assembles the `.app` with `Info.plist`, icon and URL
  scheme, and signs ad hoc, or with Developer ID when one is given. Notifications need a real bundle,
  so `swift run` isn't enough for testing them.
- `.github/workflows/macos.yml`, filtered to `macos/**`: build, test, bundle, demo-mode screenshots,
  upload artifacts. Separate from `ci.yml`, so web and Rust changes don't spend macOS minutes.
- Release: later, a `.dmg` or zip attached to GitHub releases. Requires question 4. Updates: a notice
  from the releases page first; Sparkle only if you want it, since it would be a second dependency.
- The repo's rule that `web/` PRs show before and after shots doesn't strictly apply, but the PR will
  show demo-mode screenshots anyway, with made-up data and labelled as such.

## 12. Plan

Each phase ends with something you can run.

| Phase | Delivers | Done when |
|---|---|---|
| 0. Foundation | Package, `SidefleetKit`, API client, SSE, models, reducer, tests, macOS workflow | Tests pass on Linux against a real server; empty app builds on macOS |
| 1. Watch | Sign-in, sidebar, thread replay and live, markdown, tool groups, demo mode | You can follow a real task in the app |
| 2. Act | New task, replies, attachments, stop, approvals, inbox, notifications, Dock badge, fork, restore, PR, snooze, archive | You can run your day from it |
| 3. Review and fleet | Diff review, Machines and map, Overview, Settings, repo picker, add machine, org switch | The page is no longer needed on a Mac, apart from Rooms, Team and Chat apps |
| 4. Finish | Menu bar, windows, keyboard, icon, motion, accessibility, performance, a look at every screen | Section 10's list is met; a bundle you can install |
| 5. Parity | Rooms, Team, Chat apps, invites, worktree cleanup, and the smaller gaps a file-by-file comparison with the page found | Done 2026-11-10 |

## 13. Risks

| Risk | Mitigation |
|---|---|
| UI code written without a Mac in the loop has bugs that only show on a Mac | Question 1. This is the largest risk, and it's why demo-mode screenshots come in phase 1, not at the end. |
| SwiftUI scroll performance on long threads and diffs | Measured limits in section 10, with `NSTableView` as a planned fallback |
| GitHub rejects or degrades sign-in in a `WKWebView` | Developer key still works; option 3B is the way out |
| Embedded sign-in doesn't suit passkeys | Same |
| Ad-hoc signing: Keychain prompts on each rebuild | Only affects development. Developer ID solves it. |
| Parity drift between the web app and the Mac app | Named here as an ongoing cost. Keep `format.ts`'s tables and the Swift ones together in review. |
| No notifications when the app is quit | Menu bar presence by default, and stated plainly in Settings |
