# Sidefleet for Mac

A native macOS client for Sidefleet, in SwiftUI. It talks to the same control plane as the web page,
over the same API, so it works with your existing machines and tasks. See [SPEC.md](SPEC.md) for what
it does and why it's built this way.

> [!WARNING]
> **Not yet built on a Mac.** It was written on Linux. The non-UI package is compiled and tested
> there, including against a real control plane, but the app has only been type-checked against a
> stand-in for SwiftUI. Expect some compile fixes the first time, and nobody has seen it run. The
> `macos` workflow does that build as soon as this is on GitHub.

## Build and run

You need macOS 14 or later and Xcode 16 or later (or its command line tools).

```sh
cd macos
scripts/bundle.sh                                  # builds dist/Sidefleet.app
open dist/Sidefleet.app                            # sign in to your control plane
dist/Sidefleet.app/Contents/MacOS/Sidefleet --demo # or look around with made-up data
```

`swift run Sidefleet --demo` works too, and so does opening `Package.swift` in Xcode. Notifications
need the real bundle.

Signing in takes your control plane's address, then a GitHub sign-in or the developer key from
`~/.sidefleet/server.env`. The credential goes in the Keychain.

## Layout

```
Sources/SidefleetKit/   everything that isn't drawing: API, streams, the turn reducer, diffs,
                        markdown, syntax colours, stores, the demo. Builds on macOS and Linux.
Sources/Sidefleet/      the SwiftUI app. Mac only.
Tests/                  unit tests, parity tests against the web code, live-server tests.
scripts/                bundle.sh, screenshots.sh, test-live.sh.
```

## Tests

```sh
swift test               # on a Mac
scripts/test-live.sh     # also runs the live-server tests: a real `sidefleet serve` with a throwaway Postgres

# On Linux, through the official Swift image:
SWIFT_DOCKER=1 scripts/test-live.sh
```

`Tests/SidefleetKitTests/Fixtures/web-golden.json` holds what the web app's real `parse.ts` and
`format.ts` return for a set of inputs, and `WebParityTests` holds the Swift port to it. Regenerate it
if those change on purpose.

## Screenshots

```sh
scripts/screenshots.sh dist/screenshots
```

Runs demo mode (`--demo --tour <folder> --light|--dark`) and saves a PNG of each screen. All the data
is made up. The `macos` workflow uploads these as an artifact on every run.

## When the web app changes

The two clients are separate code, so new features need porting or a decision not to. The tables most
likely to go stale are the models and efforts in `Sources/SidefleetKit/Format/Format.swift`, which
mirror `web/src/lib/sf/format.ts`. Rooms, Team and Chat apps aren't native yet: the sidebar opens them
in your browser.
