# Connect this machine to AgentCloud

You are setting up this machine as an AgentCloud **worker**. A worker runs coding agents
that are already installed and signed in here, on behalf of a control plane at
`{{ORIGIN}}`. It only makes outbound HTTP calls to that address; no ports need to be open.
The person who asked you to do this will give you a **worker token**. Treat it as a secret:
never print it, never commit it, never put it on a command line that ends up in a log.

Do these steps in order and stop to ask if one fails.

## 1. Check the agent

Run `claude --version`. Then run `claude -p "Reply with OK" --max-turns 1` and confirm it
prints OK. If it errors with a sign-in message, stop and tell the user to sign in with
`claude` first; the worker uses this machine's existing sign-in and nothing else.

## 2. Get Rust and the source

- If `cargo` is missing, install it non-interactively:
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal`
  and then `source ~/.cargo/env`.
- Get the AgentCloud repository. If `~/Code/agentcloud` exists, `git pull` in it; otherwise ask
  the user for the repository URL or path and clone it to `~/Code/agentcloud`.
- Build the worker: `cd ~/Code/agentcloud && cargo build --release -p acw`.

## 3. Start it and confirm it registered

Put the token in the environment, not on the command line:

```
export ACW_TOKEN='<the token the user gave you>'
cd ~/Code/agentcloud && ./target/release/acw run --control {{ORIGIN}} --repos ~/Code
```

Run it in the background and read its log. Within about ten seconds it should print
`registered with control plane`. The `--repos` directory is scanned two levels deep for git
checkouts; use the directory where the user keeps their projects.

## 4. Keep it running across reboots

- macOS: write `~/Library/LaunchAgents/dev.agentcloud.worker.plist` with `ProgramArguments`
  = the command above, `EnvironmentVariables` containing `ACW_TOKEN`, `RunAtLoad` and
  `KeepAlive` true, `StandardOutPath`/`StandardErrorPath` under `~/Library/Logs/agentcloud/`.
  Then `launchctl load` it.
- Linux: write a systemd **user** unit `~/.config/systemd/user/agentcloud-worker.service`
  with `Environment=ACW_TOKEN=...` (file mode 600), `Restart=always`, and enable it with
  `systemctl --user enable --now agentcloud-worker` plus `loginctl enable-linger $USER`.

## 5. Report back

Tell the user the worker's id (its hostname), how many repos it found, and that it shows as
online at {{ORIGIN}}/#/machines. Do not include the token in your summary.
