# Hosting a Sidefleet control plane

The control plane is `sidefleet serve`: the web page, the API, and a Postgres database. Put it on
a machine your other devices can reach (a VPS, a Mac mini at home, a NAS). It needs no Claude
Code of its own; if it has one, it also acts as a worker.

The fastest way is to hand the job to a coding agent on that machine. Sign in there (SSH or a
terminal), start Claude Code, and paste the prompt below. Replace the two placeholders first.

## Prompt for Claude Code on the host machine

```
Set up this machine as a Sidefleet control plane. Sidefleet runs coding agents on machines
the user owns, driven from a web page; the control plane hosts that page and an API that
workers connect to over outbound HTTP. Do the steps in order and stop to ask if one fails.
Never print secrets (the developer key and worker token it generates) in your summary; tell
the user where they are instead.

1. Get the source. The repository is <REPO_URL> (private, so clone it over SSH; if the clone
   is refused, stop and tell the user to add this machine's SSH key to GitHub or to copy the
   folder here with rsync). Clone it to ~/code/side-fleet, or `git pull` if it is already there.

2. Install the binary: run `sh ~/code/side-fleet/install.sh`. It builds from the checkout and
   installs ~/.local/bin/sidefleet, adding Rust and a C toolchain first if they are missing.
   Make sure ~/.local/bin is on PATH for later steps and in the user's shell profile.

3. Postgres. If Docker is installed, start one:
   docker run -d --name sidefleet-db --restart unless-stopped \
     -e POSTGRES_USER=sidefleet -e POSTGRES_PASSWORD=sidefleet -e POSTGRES_DB=sidefleet \
     -p 127.0.0.1:5432:5432 -v sidefleet-pgdata:/var/lib/postgresql/data postgres:16-alpine
   If Docker is not installed and this is Debian or Ubuntu, install it with apt (docker.io)
   and then run that command. Otherwise ask the user how they want Postgres provided.

4. Network. If `tailscale` is installed and up, note its IPv4 (`tailscale ip -4`); that is the
   address other machines will use. If it is not installed, tell the user Tailscale is the
   recommended way for their other devices to reach this box, and continue; the LAN address
   works on the same network. Do not open port 7373 to the public internet; the page has a key
   but no TLS.

5. Run it as a service so it survives reboots. On Linux write a systemd user unit at
   ~/.config/systemd/user/sidefleet-serve.service:
     [Unit]
     Description=Sidefleet control plane
     After=network-online.target
     [Service]
     ExecStart=%h/.local/bin/sidefleet serve
     Restart=always
     RestartSec=3
     Environment=PATH=%h/.local/bin:%h/.cargo/bin:/usr/local/bin:/usr/bin:/bin
     [Install]
     WantedBy=default.target
   then `systemctl --user daemon-reload && systemctl --user enable --now sidefleet-serve` and
   `loginctl enable-linger $USER` so it runs without a login session. On macOS write a launchd
   agent at ~/Library/LaunchAgents/io.sidefleet.serve.plist with the same program, RunAtLoad
   and KeepAlive true, and load it with `launchctl bootstrap gui/$(id -u) <plist>`.
   If `serve` fails because port 7373 is taken, add `--bind 0.0.0.0:7374` and use that port
   everywhere below.

6. Verify: `curl -s http://127.0.0.1:7373/healthz` prints ok. The generated keys are in
   ~/.sidefleet/server.env; `journalctl --user -u sidefleet-serve` (Linux) shows the startup
   message with the page URL and the connect command.

7. Report: the page URL using the Tailscale or LAN address, that the developer key and worker
   token are in ~/.sidefleet/server.env on this machine (do not paste them), whether this
   machine also runs as a worker (it does when claude, codex or ollama is installed), and the
   one-line `sidefleet connect --control <url> --token <from server.env>` the user runs on
   other machines.
```

Replace `<REPO_URL>` with `git@github.com:foodaka/side-fleet.git` (or the HTTPS URL once the
repository is public).

## Doing it by hand

Same steps without an agent, on Debian or Ubuntu:

```bash
sudo apt-get install -y git docker.io
git clone git@github.com:foodaka/side-fleet.git ~/code/side-fleet && sh ~/code/side-fleet/install.sh
docker run -d --name sidefleet-db --restart unless-stopped -e POSTGRES_USER=sidefleet -e POSTGRES_PASSWORD=sidefleet -e POSTGRES_DB=sidefleet -p 127.0.0.1:5432:5432 -v sidefleet-pgdata:/var/lib/postgresql/data postgres:16-alpine
~/.local/bin/sidefleet serve
```

`serve` prints the page URL (Tailscale address first when there is one), the developer key, and
the `sidefleet connect` line for other machines. Keys live in `~/.sidefleet/server.env` and are
reused on every start. To keep it running after a reboot, use the systemd unit from step 5 above.

## Updating

```bash
cd ~/code/side-fleet && git pull && sh install.sh && systemctl --user restart sidefleet-serve
```

Migrations run automatically on start.
