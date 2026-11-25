# Sidefleet for Mac

A native macOS client for Sidefleet, in SwiftUI. It talks to the same control plane as the web page,
over the same API, so it works with your existing machines and tasks. See [SPEC.md](SPEC.md) for what
it does and why it's built this way.

**[Download Sidefleet.dmg](https://github.com/foodaka/side-fleet/releases/latest/download/Sidefleet.dmg)**
from the latest release: macOS 14 or later, Apple silicon and Intel, signed and notarized.

> [!NOTE]
> **Written on Linux, built on GitHub's Mac runners.** The `macos` workflow compiles the app, runs
> the tests and saves a screenshot of every screen in demo mode. Nobody has yet used it by hand
> against a real control plane, so notifications, the Keychain, GitHub sign-in and sleep and wake
> are untried.

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
mirror `web/src/lib/sf/format.ts`, and the chat-app setup text in `Format/ChatApps.swift`, which mirrors
`web/src/lib/sf/chat.ts` (`Fixtures/chat-golden.json` holds that file's real output).

What the page has and the app leaves out on purpose: browser push (the app posts its own
notifications), the collapsed sidebar's icon rail (the menu bar shows what's running and waiting),
and the `#/machines?code=` approval link (type the code on the Machines screen). Invite links are web
addresses, so they open in a browser: paste one into Team > Join with a link, or open
`sidefleet://join/<code>`.

## Releasing

A `v*` tag builds `Sidefleet.dmg` (universal, Developer ID signed, notarized, stapled) and attaches it to
the GitHub release, through `.github/workflows/macos-release.yml`. Running that workflow by hand builds
the same file as an artifact. Locally: `scripts/release.sh`, with the variables at its top.

It also attaches `appcast.xml`, the feed installed copies check for updates (Sparkle, under the app menu's
Check for Updates…). The feed URL is the latest release's `appcast.xml`, so each release reaches everyone
on an older one. Sparkle compares `CFBundleVersion`, which is the tag's version.

It needs six repository secrets:

| Secret | What |
|---|---|
| `MACOS_CERTIFICATE_P12` | `base64 -i cert.p12` of the Developer ID Application certificate with its private key |
| `MACOS_CERTIFICATE_PASSWORD` | the password the `.p12` was exported with |
| `NOTARY_KEY_P8` | `base64 -i AuthKey_XXXX.p8` of an App Store Connect API key (Developer role) |
| `NOTARY_KEY_ID` | that key's ID |
| `NOTARY_ISSUER_ID` | the issuer ID shown above the keys list |
| `SPARKLE_PRIVATE_KEY` | base64 EdDSA private key that signs the update feed; its public half is `SUPublicEDKey` in `Resources/Info.plist`. Keep a copy offline: without it, installed apps can't be updated |

The app isn't sandboxed and needs no special entitlements, so no provisioning profile is involved.

