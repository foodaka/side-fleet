# Connect this machine to Sidefleet

You are connecting this machine to a Sidefleet control plane at `{{ORIGIN}}`, so the person
who runs it can start coding-agent tasks here from a web page. The worker uses the Claude Code
sign-in that already exists on this machine, makes only outbound HTTP calls to that address, and
needs no open ports. The user will give you a **worker token**. Treat it as a secret: never print
it, never commit it, and keep it out of your final summary.

Do these steps in order and stop to ask if one fails.

## 1. Check Claude Code

Run `claude --version`, then `claude -p "Reply with OK" --max-turns 1` and confirm it prints OK.
If it reports a sign-in problem, stop and tell the user to run `claude` once and sign in; the
worker cannot work without that.

## 2. Install Sidefleet

```
curl -fsSL https://raw.githubusercontent.com/foodaka/side-fleet/main/install.sh | sh
```

It installs a single `sidefleet` binary into `~/.local/bin` (prebuilt when available, otherwise
built from source with Rust, which the script installs if missing). Make sure `~/.local/bin` is on
PATH for the current shell: `export PATH="$HOME/.local/bin:$PATH"`.

## 3. Connect

```
sidefleet connect --control {{ORIGIN}} --token <the token the user gave you>
```

It finds git checkouts under the home folder by itself (three levels deep; pass `--repos <folder>`
only if the user's projects live somewhere unusual). This checks the machine, registers it once, saves the settings to `~/.sidefleet/worker.env`
(mode 600) and installs a background service (launchd on macOS, a systemd user unit on Linux)
that keeps the worker running and restarts it after reboots. If it says the control plane is
unreachable, the address is probably only valid on the control plane's own machine: ask the user
for its LAN or Tailscale address and retry.

## 4. Confirm and report

Run `sidefleet status`. Tell the user the machine's name, how many repos were found, and that it
should now show as online at {{ORIGIN}}/#/machines. Do not include the token.
