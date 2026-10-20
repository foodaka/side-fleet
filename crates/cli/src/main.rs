//! `sidefleet`: one binary for both halves.
//!
//! * `sidefleet serve`      run the control plane (web page + API); needs Postgres
//! * `sidefleet connect`    make this machine a worker: validate, save settings, install a service
//! * `sidefleet run`        run the worker in the foreground (what the service runs)
//! * `sidefleet status`     show saved worker settings and whether the service is running
//! * `sidefleet disconnect` remove the service
//! * `sidefleet detect`     print what this machine can run

use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use rand::RngCore;
use sidefleet_server::{connect_and_migrate, AppState, Config};
use sidefleet_worker::worker::{default_worker_id, WorkerConfig};
use tracing_subscriber::EnvFilter;

const DEFAULT_DB: &str = "postgres://sidefleet:sidefleet@localhost:5432/sidefleet";

#[derive(Parser)]
#[command(name = "sidefleet", version, about = "Run coding agents on your own machines, from anywhere.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the control plane: the web page and API that workers connect to. Needs Postgres.
    Serve {
        /// Address to listen on.
        #[arg(long, env = "SIDEFLEET_BIND", default_value = "0.0.0.0:8080")]
        bind: String,
        /// Postgres connection string.
        #[arg(long, env = "DATABASE_URL", default_value = DEFAULT_DB)]
        database_url: String,
        /// Developer key for the page and API. Generated and saved on first run if omitted.
        #[arg(long, env = "SIDEFLEET_DEV_KEY", hide_env_values = true)]
        dev_key: Option<String>,
        /// Comma-separated worker tokens. Generated and saved on first run if omitted.
        #[arg(long, env = "SIDEFLEET_WORKER_TOKENS", hide_env_values = true)]
        worker_tokens: Option<String>,
    },
    /// Make this machine a worker: check it, register once, save the settings and
    /// install a background service so it keeps running after reboots.
    Connect {
        /// Control plane address, e.g. http://100.64.0.1:8080
        #[arg(long, env = "SIDEFLEET_CONTROL")]
        control: String,
        /// Worker token (shown by `sidefleet serve` and on the Machines page).
        #[arg(long, env = "SIDEFLEET_TOKEN", hide_env_values = true)]
        token: String,
        /// Folder scanned (two levels deep) for git checkouts.
        #[arg(long, env = "SIDEFLEET_REPOS", default_value = "~/Code")]
        repos: String,
        /// Name shown on the Machines page (defaults to this machine's hostname).
        #[arg(long, env = "SIDEFLEET_NAME")]
        name: Option<String>,
        /// Permission mode for Claude Code: acceptEdits (default), bypassPermissions, default, plan.
        #[arg(long, env = "SIDEFLEET_PERMISSION_MODE", default_value = "acceptEdits")]
        permission_mode: String,
        /// Save settings but do not install or start the background service.
        #[arg(long)]
        no_service: bool,
    },
    /// Run the worker in the foreground. Uses the settings saved by `connect` when flags are omitted.
    Run {
        #[arg(long, env = "SIDEFLEET_CONTROL")]
        control: Option<String>,
        #[arg(long, env = "SIDEFLEET_TOKEN", hide_env_values = true)]
        token: Option<String>,
        #[arg(long, env = "SIDEFLEET_REPOS")]
        repos: Option<String>,
        #[arg(long, env = "SIDEFLEET_ID")]
        id: Option<String>,
        #[arg(long, env = "SIDEFLEET_NAME")]
        name: Option<String>,
        /// Claude Code executable.
        #[arg(long, env = "SIDEFLEET_CLAUDE_BIN", default_value = "claude")]
        claude_bin: String,
        #[arg(long, env = "SIDEFLEET_PERMISSION_MODE")]
        permission_mode: Option<String>,
        /// Cap on agent turns per run.
        #[arg(long, env = "SIDEFLEET_MAX_TURNS")]
        max_turns: Option<u32>,
        /// Extra arguments appended to the claude command line (repeatable).
        #[arg(long = "claude-arg")]
        claude_args: Vec<String>,
        /// Seconds between claim polls.
        #[arg(long, default_value_t = 2)]
        poll_interval: u64,
        /// Skip the sign-in probe (a trivial `claude -p`) at start.
        #[arg(long)]
        skip_sign_in_check: bool,
        /// How many directory levels below --repos to scan for checkouts.
        #[arg(long, default_value_t = 2)]
        scan_depth: usize,
    },
    /// Show the saved worker settings and whether the background service is running.
    Status,
    /// Stop and remove the background service installed by `connect`. Settings are kept.
    Disconnect,
    /// Print what this machine can run (agents, sign-in state, repos) as JSON.
    Detect {
        #[arg(long, env = "SIDEFLEET_REPOS", default_value = "~/Code")]
        repos: String,
        #[arg(long, env = "SIDEFLEET_CLAUDE_BIN", default_value = "claude")]
        claude_bin: String,
        #[arg(long)]
        skip_sign_in_check: bool,
        #[arg(long, default_value_t = 2)]
        scan_depth: usize,
    },
}

// ---- paths ----

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
fn config_dir() -> PathBuf {
    home().join(".sidefleet")
}
fn worker_env_path() -> PathBuf {
    config_dir().join("worker.env")
}
fn server_env_path() -> PathBuf {
    config_dir().join("server.env")
}
fn log_dir() -> PathBuf {
    config_dir().join("logs")
}
fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if p == "~" => home(),
        None => PathBuf::from(p),
    }
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn random_token() -> String {
    let mut b = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn mask(s: &str) -> String {
    if s.len() <= 6 {
        "••••••".into()
    } else {
        format!("{}••••••", &s[..4])
    }
}

/// The LAN address other machines can use to reach this one (best effort).
fn lan_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("10.255.255.255:80").ok()?; // no packet is sent
    Some(s.local_addr().ok()?.ip().to_string())
}

// ---- main ----

#[tokio::main]
async fn main() -> Result<()> {
    // Saved settings fill in anything not given on the command line or environment.
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(worker_env_path());
    let _ = dotenvy::from_path(server_env_path());

    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "sidefleet=info,sidefleet_server=info,sidefleet_worker=info,tower_http=warn".into()))
        .init();

    match cli.cmd {
        Cmd::Serve { bind, database_url, dev_key, worker_tokens } => serve(bind, database_url, dev_key, worker_tokens).await,
        Cmd::Connect { control, token, repos, name, permission_mode, no_service } => connect(control, token, repos, name, permission_mode, no_service).await,
        Cmd::Run { control, token, repos, id, name, claude_bin, permission_mode, max_turns, claude_args, poll_interval, skip_sign_in_check, scan_depth } => {
            let control = control.context("no control plane address: run `sidefleet connect --control <url> --token <token>` first, or pass --control")?;
            let token = token.context("no worker token: run `sidefleet connect` first, or pass --token / SIDEFLEET_TOKEN")?;
            let id = id.unwrap_or_else(default_worker_id);
            let cfg = WorkerConfig {
                control_url: control,
                token,
                display_name: name.unwrap_or_else(|| id.clone()),
                worker_id: id,
                repos_root: expand_home(&repos.unwrap_or_else(|| "~/Code".into())),
                claude_bin,
                permission_mode: permission_mode.unwrap_or_else(|| "acceptEdits".into()),
                max_turns,
                extra_args: claude_args,
                poll_interval: Duration::from_secs(poll_interval.max(1)),
                heartbeat_interval: Duration::from_secs(15),
                check_sign_in: !skip_sign_in_check,
                scan_depth,
                max_runs: None,
            };
            sidefleet_worker::worker::run(cfg).await
        }
        Cmd::Status => status(),
        Cmd::Disconnect => disconnect(),
        Cmd::Detect { repos, claude_bin, skip_sign_in_check, scan_depth } => {
            let caps = sidefleet_worker::detect::detect_all(&sidefleet_worker::detect::DetectOptions {
                claude_bin,
                repos_root: expand_home(&repos),
                check_sign_in: !skip_sign_in_check,
                scan_depth,
            })
            .await;
            println!("{}", serde_json::to_string_pretty(&caps)?);
            Ok(())
        }
    }
}

// ---- serve ----

async fn serve(bind: String, database_url: String, dev_key: Option<String>, worker_tokens: Option<String>) -> Result<()> {
    // First run: generate keys and keep them so the next start reuses them.
    let (dev_key, worker_tokens, generated) = match (dev_key, worker_tokens) {
        (Some(d), Some(w)) => (d, w, false),
        (d, w) => {
            let d = d.unwrap_or_else(random_token);
            let w = w.unwrap_or_else(random_token);
            write_private(&server_env_path(), &format!("SIDEFLEET_DEV_KEY={d}\nSIDEFLEET_WORKER_TOKENS={w}\n"))?;
            (d, w, true)
        }
    };
    std::env::set_var("SIDEFLEET_DEV_KEY", &dev_key);
    std::env::set_var("SIDEFLEET_WORKER_TOKENS", &worker_tokens);
    let config = Config::from_env()?;
    let first_token = config.worker_tokens[0].clone();

    let pool = match connect_and_migrate(&database_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Could not reach Postgres at {database_url}\n  ({e:#})\n");
            eprintln!("Start one with Docker and try again:\n");
            eprintln!("  docker run -d --name sidefleet-db --restart unless-stopped \\\n    -e POSTGRES_USER=sidefleet -e POSTGRES_PASSWORD=sidefleet -e POSTGRES_DB=sidefleet \\\n    -p 5432:5432 -v sidefleet-pgdata:/var/lib/postgresql/data postgres:16-alpine\n");
            eprintln!("or point --database-url / DATABASE_URL at an existing database.");
            std::process::exit(2);
        }
    };
    let state = AppState::new(pool, config);
    tokio::spawn(sidefleet_server::reaper(state.clone()));
    let listener = tokio::net::TcpListener::bind(&bind).await.with_context(|| format!("binding {bind}"))?;
    let port = listener.local_addr()?.port();
    let ip = lan_ip().unwrap_or_else(|| "127.0.0.1".into());

    println!();
    println!("Sidefleet control plane is running.");
    println!();
    println!("  Open the page:        http://{ip}:{port}/   (or http://localhost:{port}/ here)");
    println!("  Developer key:        {dev_key}");
    println!();
    println!("  Connect a machine:    sidefleet connect --control http://{ip}:{port} --token {first_token}");
    println!();
    if generated {
        println!("  Keys were generated and saved to {}", server_env_path().display());
    }
    println!("  Other machines must be able to reach this address (same LAN or Tailscale).");
    println!();

    axum::serve(listener, sidefleet_server::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

// ---- connect / service ----

async fn connect(control: String, token: String, repos: String, name: Option<String>, permission_mode: String, no_service: bool) -> Result<()> {
    let control = control.trim_end_matches('/').to_string();
    let repos_root = expand_home(&repos);
    if !repos_root.is_dir() {
        bail!("{} is not a directory; pass --repos <folder with your git checkouts>", repos_root.display());
    }
    let id = default_worker_id();
    let display = name.clone().unwrap_or_else(|| id.clone());

    println!("Checking this machine…");
    let caps = sidefleet_worker::detect::detect_all(&sidefleet_worker::detect::DetectOptions {
        claude_bin: "claude".into(),
        repos_root: repos_root.clone(),
        check_sign_in: true,
        scan_depth: 2,
    })
    .await;
    match &caps.agents.claude {
        Some(c) if c.signed_in => println!("  Claude Code {}: signed in", c.version),
        Some(c) => println!("  Claude Code {}: NOT signed in ({}). Run `claude` once to sign in; tasks will fail until then.", c.version, c.sign_in_error.as_deref().unwrap_or("unknown")),
        None => println!("  Claude Code: not found on PATH. Install it and sign in, then run connect again."),
    }
    if let Some(c) = &caps.agents.codex {
        println!("  Codex {}", c.version);
    }
    if let Some(o) = &caps.agents.ollama {
        println!("  Ollama {} ({} models)", o.version, o.models.len());
    }
    println!("  {} git checkouts under {}", caps.repos.len(), repos_root.display());

    println!("Registering with {control}…");
    let client = sidefleet_worker::client::ControlClient::new(&control, &token, &id)?;
    match client.register(&display, &caps).await {
        Ok(_) => println!("  Registered as \"{display}\" ({id})"),
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("401") {
                bail!("the control plane rejected this token. Copy it from the Machines page or from where `sidefleet serve` printed it.");
            }
            bail!("could not reach the control plane at {control}: {msg}\nIs `sidefleet serve` running there, and is this address reachable from this machine (same LAN or Tailscale)?");
        }
    }

    let env = format!(
        "SIDEFLEET_CONTROL={control}\nSIDEFLEET_TOKEN={token}\nSIDEFLEET_REPOS={}\nSIDEFLEET_NAME={display}\nSIDEFLEET_PERMISSION_MODE={permission_mode}\n",
        repos_root.display()
    );
    write_private(&worker_env_path(), &env)?;
    println!("Saved settings to {}", worker_env_path().display());

    if no_service {
        println!("\nStart the worker with:  sidefleet run");
        return Ok(());
    }
    std::fs::create_dir_all(log_dir())?;
    let exe = std::env::current_exe()?;
    match install_service(&exe) {
        Ok(what) => {
            println!("{what}");
            println!("\nDone. This machine should show as online on the Machines page within a few seconds.");
            println!("Logs: {}", log_dir().join("worker.log").display());
        }
        Err(e) => {
            println!("Could not install a background service ({e:#}).");
            println!("Run the worker manually instead:  sidefleet run");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
const SERVICE_LABEL: &str = "io.sidefleet.worker";

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    home().join("Library/LaunchAgents").join(format!("{SERVICE_LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn install_service(exe: &Path) -> Result<String> {
    let log = log_dir().join("worker.log");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{SERVICE_LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string><string>run</string></array>
  <key>EnvironmentVariables</key><dict><key>PATH</key><string>{path}</string><key>HOME</key><string>{home}</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict></plist>
"#,
        exe = exe.display(),
        path = service_path(),
        home = home().display(),
        log = log.display()
    );
    let p = plist_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, plist)?;
    let domain = format!("gui/{}", nix_uid());
    let _ = std::process::Command::new("launchctl").args(["bootout", &domain, &p.to_string_lossy()]).output();
    let out = std::process::Command::new("launchctl").args(["bootstrap", &domain, &p.to_string_lossy()]).output()?;
    if !out.status.success() {
        bail!("launchctl bootstrap failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(format!("Installed launchd service {SERVICE_LABEL} (starts at login, restarts if it stops)."))
}

#[cfg(target_os = "macos")]
fn uninstall_service() -> Result<String> {
    let p = plist_path();
    if !p.exists() {
        return Ok("No service was installed.".into());
    }
    let domain = format!("gui/{}", nix_uid());
    let _ = std::process::Command::new("launchctl").args(["bootout", &domain, &p.to_string_lossy()]).output();
    std::fs::remove_file(&p)?;
    Ok(format!("Removed launchd service {SERVICE_LABEL}."))
}

#[cfg(target_os = "macos")]
fn service_installed() -> bool {
    plist_path().exists()
}

#[cfg(target_os = "linux")]
fn unit_path() -> PathBuf {
    home().join(".config/systemd/user/sidefleet-worker.service")
}

#[cfg(target_os = "linux")]
fn install_service(exe: &Path) -> Result<String> {
    let unit = format!(
        "[Unit]\nDescription=Sidefleet worker\nAfter=network-online.target\n\n[Service]\nExecStart={exe} run\nRestart=always\nRestartSec=3\nEnvironment=PATH={path}\nStandardOutput=append:{log}\nStandardError=append:{log}\n\n[Install]\nWantedBy=default.target\n",
        exe = exe.display(),
        path = service_path(),
        log = log_dir().join("worker.log").display()
    );
    let p = unit_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, unit)?;
    let run = |args: &[&str]| std::process::Command::new("systemctl").arg("--user").args(args).output();
    run(&["daemon-reload"])?;
    let out = run(&["enable", "--now", "sidefleet-worker"])?;
    if !out.status.success() {
        bail!("systemctl --user enable failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let _ = std::process::Command::new("loginctl").args(["enable-linger"]).output();
    Ok("Installed systemd user service sidefleet-worker (starts at boot, restarts if it stops).".into())
}

#[cfg(target_os = "linux")]
fn uninstall_service() -> Result<String> {
    let p = unit_path();
    if !p.exists() {
        return Ok("No service was installed.".into());
    }
    let _ = std::process::Command::new("systemctl").args(["--user", "disable", "--now", "sidefleet-worker"]).output();
    std::fs::remove_file(&p)?;
    let _ = std::process::Command::new("systemctl").args(["--user", "daemon-reload"]).output();
    Ok("Removed systemd user service sidefleet-worker.".into())
}

#[cfg(target_os = "linux")]
fn service_installed() -> bool {
    unit_path().exists()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_service(_exe: &Path) -> Result<String> {
    bail!("background services are only supported on macOS and Linux")
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn uninstall_service() -> Result<String> {
    Ok("No service support on this platform.".into())
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn service_installed() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn nix_uid() -> u32 {
    // launchctl needs the numeric uid; `id -u` avoids pulling in libc bindings.
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(501)
}

/// PATH for the service: the user's PATH at connect time, so `claude` is found.
fn service_path() -> String {
    let mut p = std::env::var("PATH").unwrap_or_default();
    for extra in ["/usr/local/bin", "/opt/homebrew/bin", &format!("{}/.local/bin", home().display()), &format!("{}/.cargo/bin", home().display())] {
        if !p.split(':').any(|x| x == extra) {
            p = format!("{p}:{extra}");
        }
    }
    p
}

fn status() -> Result<()> {
    let p = worker_env_path();
    if !p.exists() {
        println!("Not connected. Run: sidefleet connect --control <url> --token <token>");
        return Ok(());
    }
    println!("Settings ({})", p.display());
    for line in std::fs::read_to_string(&p)?.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = if k == "SIDEFLEET_TOKEN" { mask(v) } else { v.to_string() };
            println!("  {k} = {v}");
        }
    }
    println!("Service installed: {}", if service_installed() { "yes" } else { "no" });
    let running = std::process::Command::new("pgrep").args(["-f", "sidefleet run"]).output().map(|o| o.status.success()).unwrap_or(false);
    println!("Worker process running: {}", if running { "yes" } else { "no" });
    println!("Log: {}", log_dir().join("worker.log").display());
    Ok(())
}

fn disconnect() -> Result<()> {
    println!("{}", uninstall_service()?);
    println!("Settings kept at {}; delete that file to forget the token.", worker_env_path().display());
    Ok(())
}
