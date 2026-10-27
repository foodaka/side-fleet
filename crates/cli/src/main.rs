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

const BUILD: &str = env!("SIDEFLEET_BUILD");

fn version_string() -> &'static str {
    // e.g. "0.1.0 (2026-10-20 8ec33ec)"
    Box::leak(format!("{} ({BUILD})", env!("CARGO_PKG_VERSION")).into_boxed_str())
}

#[derive(Parser)]
#[command(name = "sidefleet", version = version_string(), about = "Run coding agents on your own machines, from anywhere.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the control plane: the web page and API that workers connect to. Needs Postgres.
    Serve {
        /// Address to listen on.
        #[arg(long, env = "SIDEFLEET_BIND", default_value = "0.0.0.0:7373")]
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
        /// Do not run a worker on this machine as well (by default it does when a coding agent is installed).
        #[arg(long)]
        no_worker: bool,
        /// Folder with this machine's git checkouts, for the built-in worker (default: your home folder, scanned 3 levels deep).
        #[arg(long, env = "SIDEFLEET_REPOS", default_value = "~")]
        repos: String,
        /// Default permission mode for the built-in worker's tasks.
        #[arg(long, env = "SIDEFLEET_PERMISSION_MODE", default_value = "acceptEdits")]
        permission_mode: String,
        /// How many tasks the built-in worker runs at the same time.
        #[arg(long, env = "SIDEFLEET_PARALLEL", default_value_t = 3)]
        parallel: usize,
        /// Install serve as a background service (launchd / systemd user) with these flags, start it, and exit.
        #[arg(long)]
        install_service: bool,
    },
    /// Pull the latest code (or download the latest release), reinstall the binary and restart the services on this machine.
    Update {
        /// Only reinstall; do not restart services.
        #[arg(long)]
        no_restart: bool,
    },
    /// Make this machine a worker: check it, register once, save the settings and
    /// install a background service so it keeps running after reboots.
    Connect {
        /// Control plane address, e.g. http://100.64.0.1:7373
        #[arg(long, env = "SIDEFLEET_CONTROL")]
        control: String,
        /// Worker token (shown by `sidefleet serve` and on the Machines page). Without
        /// one, the machine asks to join and waits until someone approves it on the page.
        #[arg(long, env = "SIDEFLEET_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Folder with your git checkouts (default: your home folder, scanned 3 levels deep; hidden and build folders skipped).
        #[arg(long, env = "SIDEFLEET_REPOS", default_value = "~")]
        repos: String,
        /// Name shown on the Machines page (defaults to this machine's hostname).
        #[arg(long, env = "SIDEFLEET_NAME")]
        name: Option<String>,
        /// Default permission mode for tasks: acceptEdits (default), bypassPermissions, plan, ask, default.
        #[arg(long, env = "SIDEFLEET_PERMISSION_MODE", default_value = "acceptEdits")]
        permission_mode: String,
        /// Save settings but do not install or start the background service.
        #[arg(long)]
        no_service: bool,
        /// How many tasks may run at the same time on this machine.
        #[arg(long, env = "SIDEFLEET_PARALLEL", default_value_t = 3)]
        parallel: usize,
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
        /// Codex CLI executable.
        #[arg(long, env = "SIDEFLEET_CODEX_BIN", default_value = "codex")]
        codex_bin: String,
        /// opencode executable.
        #[arg(long, env = "SIDEFLEET_OPENCODE_BIN", default_value = "opencode")]
        opencode_bin: String,
        #[arg(long, env = "SIDEFLEET_PERMISSION_MODE")]
        permission_mode: Option<String>,
        /// Cap on agent turns per run (Claude Code only).
        #[arg(long, env = "SIDEFLEET_MAX_TURNS")]
        max_turns: Option<u32>,
        /// Extra arguments appended to the claude command line (repeatable).
        #[arg(long = "claude-arg")]
        claude_args: Vec<String>,
        /// Seconds between claim polls, when the control plane can't hold a claim
        /// open until a run is queued (an older one, or while a repo is busy here).
        #[arg(long, default_value_t = 2)]
        poll_interval: u64,
        /// Skip the sign-in checks (a trivial `claude -p`, `codex login status`) at start.
        #[arg(long)]
        skip_sign_in_check: bool,
        /// How many directory levels below --repos to scan for checkouts.
        #[arg(long, default_value_t = 3)]
        scan_depth: usize,
        /// How many tasks may run at the same time on this machine.
        #[arg(long, env = "SIDEFLEET_PARALLEL", default_value_t = 3)]
        parallel: usize,
        /// Seconds an "Ask me" tool call waits for approval on the page before it is denied.
        #[arg(long, env = "SIDEFLEET_APPROVAL_TIMEOUT", default_value_t = sidefleet_core::DEFAULT_APPROVAL_TIMEOUT_SECS)]
        approval_timeout: u64,
    },
    /// Internal: the MCP server Claude Code calls for "Ask me" permission prompts.
    #[command(hide = true)]
    ApprovalMcp {
        /// The worker's socket for this run.
        #[arg(long)]
        socket: PathBuf,
    },
    /// Show the saved worker settings and whether the background service is running.
    Status,
    /// Stop and remove the background service installed by `connect`. Settings are kept.
    Disconnect,
    /// Print what this machine can run (agents, sign-in state, repos) as JSON.
    Detect {
        #[arg(long, env = "SIDEFLEET_REPOS", default_value = "~")]
        repos: String,
        #[arg(long, env = "SIDEFLEET_CLAUDE_BIN", default_value = "claude")]
        claude_bin: String,
        #[arg(long, env = "SIDEFLEET_CODEX_BIN", default_value = "codex")]
        codex_bin: String,
        #[arg(long, env = "SIDEFLEET_OPENCODE_BIN", default_value = "opencode")]
        opencode_bin: String,
        #[arg(long)]
        skip_sign_in_check: bool,
        #[arg(long, default_value_t = 3)]
        scan_depth: usize,
    },
}

// ---- paths ----

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
/// `~/.sidefleet`, or `SIDEFLEET_CONFIG_DIR` to keep a second worker's settings apart
/// (for example a test worker next to the one the service runs).
fn config_dir() -> PathBuf {
    match std::env::var_os("SIDEFLEET_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(d) => expand_home(&d.to_string_lossy()),
        None => home().join(".sidefleet"),
    }
}
fn worker_env_path() -> PathBuf {
    config_dir().join("worker.env")
}
/// Plain text by default (coloured only on a terminal, so service log files stay
/// clean); one JSON object per line with `SIDEFLEET_LOG_FORMAT=json`, which the
/// hosted control plane uses so its logs can be searched by field. Panics are
/// logged too, so they land in the same stream and format.
fn init_logging() {
    use std::io::IsTerminal;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "sidefleet=info,sidefleet_server=info,sidefleet_worker=info,tower_http=warn".into());
    if std::env::var("SIDEFLEET_LOG_FORMAT").is_ok_and(|f| f.eq_ignore_ascii_case("json")) {
        tracing_subscriber::fmt().with_env_filter(filter).json().flatten_event(true).with_current_span(true).with_span_list(false).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stdout().is_terminal()).init();
    }
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::capture();
        let backtrace = if backtrace.status() == std::backtrace::BacktraceStatus::Captured { backtrace.to_string() } else { String::new() };
        let location = info.location().map(|l| l.to_string()).unwrap_or_default();
        let message = info.payload().downcast_ref::<String>().map(String::as_str).or_else(|| info.payload().downcast_ref::<&str>().copied()).unwrap_or("");
        tracing::error!(%location, message, backtrace, "panic");
    }));
}

fn server_env_path() -> PathBuf {
    config_dir().join("server.env")
}
fn log_dir() -> PathBuf {
    config_dir().join("logs")
}
fn worktrees_dir() -> PathBuf {
    config_dir().join("worktrees")
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

/// Add lines to a private file, creating it if needed.
fn append_private(path: &Path, lines: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let sep = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    write_private(path, &format!("{existing}{sep}{lines}"))
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

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

/// This machine's Tailscale address, if the tailscale CLI is installed and up.
fn tailscale_ip() -> Option<String> {
    let out = std::process::Command::new("tailscale").args(["ip", "-4"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let ip = String::from_utf8_lossy(&out.stdout).lines().next()?.trim().to_string();
    (!ip.is_empty()).then_some(ip)
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
    // Speaks MCP on stdout, so nothing else may print there.
    if let Cmd::ApprovalMcp { socket } = &cli.cmd {
        return Ok(sidefleet_worker::approval::serve_mcp_stdio(socket)?);
    }
    init_logging();

    match cli.cmd {
        Cmd::Serve { bind, database_url, dev_key, worker_tokens, no_worker, repos, permission_mode, parallel, install_service } => {
            if install_service {
                let mut args = vec!["serve".to_string(), "--bind".into(), bind.clone(), "--parallel".into(), parallel.to_string(), "--permission-mode".into(), permission_mode.clone()];
                if no_worker { args.push("--no-worker".into()); }
                args.push("--repos".into()); args.push(repos.clone());
                let exe = std::env::current_exe()?;
                std::fs::create_dir_all(log_dir())?;
                println!("{}", install_named_service("serve", &exe, &args, &log_dir().join("serve.log"))?);
                println!("Keys live in {}; `sidefleet update` restarts this service after an update.", server_env_path().display());
                return Ok(());
            }
            serve(ServeOpts { bind, database_url, dev_key, worker_tokens, no_worker, repos: expand_home(&repos), permission_mode, parallel }).await
        }
        Cmd::Update { no_restart } => {
            println!("Updating sidefleet {}…", version_string());
            let r = tokio::task::spawn_blocking(move || sidefleet_worker::updater::update(!no_restart)).await??;
            println!("  Source:      {}", r.source);
            if !r.pulled.is_empty() { println!("  git pull:    {}", r.pulled.lines().last().unwrap_or("")); }
            println!("  Installed:   {}", r.installed_to.display());
            for s in &r.restarted { println!("  Restarted:   {s}"); }
            for s in &r.needs_manual_restart { println!("  Restart yourself: {s}"); }
            if no_restart { println!("  Not restarting services (--no-restart); running ones keep the old binary until restarted."); }
            else if r.restarted.is_empty() && r.needs_manual_restart.is_empty() { println!("  Nothing was running as a service; start `sidefleet serve` or `sidefleet run` when ready."); }
            Ok(())
        }
        Cmd::Connect { control, token, repos, name, permission_mode, no_service, parallel } => connect(control, token, repos, name, permission_mode, no_service, parallel).await,
        Cmd::Run { control, token, repos, id, name, claude_bin, codex_bin, opencode_bin, permission_mode, max_turns, claude_args, poll_interval, skip_sign_in_check, scan_depth, parallel, approval_timeout } => {
            let control = control.context("no control plane address: run `sidefleet connect --control <url> --token <token>` first, or pass --control")?;
            let token = token.context("no worker token: run `sidefleet connect` first, or pass --token / SIDEFLEET_TOKEN")?;
            let id = id.unwrap_or_else(default_worker_id);
            let cfg = WorkerConfig {
                control_url: control,
                token,
                display_name: name.unwrap_or_else(|| id.clone()),
                worker_id: id,
                repos_root: expand_home(&repos.unwrap_or_else(|| "~".into())),
                claude_bin,
                codex_bin,
                opencode_bin,
                permission_mode: permission_mode.unwrap_or_else(|| "acceptEdits".into()),
                max_turns,
                extra_args: claude_args,
                poll_interval: Duration::from_secs(poll_interval.max(1)),
                heartbeat_interval: Duration::from_secs(15),
                check_sign_in: !skip_sign_in_check,
                scan_depth,
                max_runs: None,
                parallel,
                worktrees_dir: worktrees_dir(),
                version: version_string().to_string(),
                allow_remote_update: true,
                approval_exe: std::env::current_exe().ok(),
                approval_timeout: Duration::from_secs(approval_timeout.max(1)),
            };
            tokio::select! {
                r = sidefleet_worker::worker::run(cfg) => r,
                _ = shutdown_signal() => {
                    let n = sidefleet_worker::agent::kill_all_agents();
                    if n > 0 { eprintln!("stopped {n} running agent(s)"); }
                    Ok(())
                }
            }
        }
        Cmd::ApprovalMcp { .. } => unreachable!("handled before logging starts"),
        Cmd::Status => status(),
        Cmd::Disconnect => disconnect(),
        Cmd::Detect { repos, claude_bin, codex_bin, opencode_bin, skip_sign_in_check, scan_depth } => {
            let caps = sidefleet_worker::detect::detect_all(&sidefleet_worker::detect::DetectOptions {
                claude_bin,
                codex_bin,
                opencode_bin,
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

struct ServeOpts {
    bind: String,
    database_url: String,
    dev_key: Option<String>,
    worker_tokens: Option<String>,
    no_worker: bool,
    repos: PathBuf,
    permission_mode: String,
    parallel: usize,
}

async fn serve(opts: ServeOpts) -> Result<()> {
    let ServeOpts { bind, database_url, dev_key, worker_tokens, no_worker, repos, permission_mode, parallel } = opts;
    // Hosted (GitHub sign-in configured): people sign in and bring their own
    // tokens, so nothing is generated; a key or static tokens given are still honoured.
    let hosted = ["SIDEFLEET_GITHUB_CLIENT_ID", "GITHUB_CLIENT_ID"].iter().any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()));
    // First run: generate keys and keep them so the next start reuses them.
    let (dev_key, worker_tokens, generated) = match (dev_key, worker_tokens) {
        (d, w) if hosted => (d.unwrap_or_default(), w.unwrap_or_default(), false),
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
    // Web push needs a VAPID key pair that outlives restarts (browsers subscribe to its public half).
    if std::env::var("SIDEFLEET_VAPID_PRIVATE_KEY").map_or(true, |v| v.trim().is_empty()) {
        let k = sidefleet_server::push::generate_private_key();
        if let Err(e) = append_private(&server_env_path(), &format!("SIDEFLEET_VAPID_PRIVATE_KEY={k}\n")) {
            // Push still works until the next restart; browsers then need to turn it on again.
            eprintln!("Could not save the web push key ({e:#}); set SIDEFLEET_VAPID_PRIVATE_KEY to keep one.");
        }
        std::env::set_var("SIDEFLEET_VAPID_PRIVATE_KEY", &k);
    }
    std::env::set_var("SIDEFLEET_BUILD", version_string());
    // Tell the page which machine hosts the control plane (when it runs a worker too).
    // The built-in worker uses a static token, so a hosted control plane without one has none.
    let builtin_worker = !no_worker && !worker_tokens.is_empty() && ["claude", "codex", "opencode"].iter().any(|b| which(b)) && repos.is_dir();
    if builtin_worker {
        std::env::set_var("SIDEFLEET_HOST_WORKER", default_worker_id());
    }
    let config = Config::from_env()?;
    let first_token = config.worker_tokens.first().cloned().unwrap_or_default();
    let public_url = config.public_url.clone();

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
    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("Port {bind} is already in use by another program.");
            eprintln!("Pick another with:  sidefleet serve --bind 0.0.0.0:7374");
            eprintln!("(then use that port in the page URL and in `sidefleet connect --control …`)");
            std::process::exit(2);
        }
        Err(e) => return Err(e).with_context(|| format!("binding {bind}")),
    };
    let port = listener.local_addr()?.port();
    let lan = lan_ip().unwrap_or_else(|| "127.0.0.1".into());
    let ts = tailscale_ip();
    // Prefer the Tailscale address: it works from anywhere the user's devices are.
    let ip = ts.clone().unwrap_or_else(|| lan.clone());

    println!();
    println!("Sidefleet control plane {} is running.", version_string());
    println!();
    if hosted {
        let url = public_url.unwrap_or_else(|| format!("http://{ip}:{port}"));
        println!("  Hosted mode: people sign in with GitHub and work in their own orgs.");
        println!("  Open the page:        {url}/");
        println!("  Connect a machine:    sidefleet connect --control {url}   (then approve it on the Machines page)");
        if !dev_key.is_empty() {
            println!("  The developer key from SIDEFLEET_DEV_KEY also works; it acts in the default org.");
        }
    } else {
        println!("  Open the page:        http://{ip}:{port}/   (or http://localhost:{port}/ here)");
        println!("  Developer key:        {dev_key}");
        println!();
        println!("  Connect a machine:    sidefleet connect --control http://{ip}:{port} --token {first_token}");
        println!("                   or:  sidefleet connect --control http://{ip}:{port}   and approve it on the Machines page");
    }
    println!();
    if generated {
        println!("  Keys were generated and saved to {}", server_env_path().display());
    }
    match ts {
        _ if hosted => {}
        Some(_) => println!("  That is this machine's Tailscale address, reachable from your other devices on the tailnet. On the same LAN http://{lan}:{port} also works."),
        None => println!("  Other machines must be able to reach this address (same LAN). For access from anywhere, install Tailscale on both ends and restart serve."),
    }
    println!();

    // This machine is a worker too when it has any coding agent, so one box is one command.
    let agents: Vec<&str> = [("claude", "Claude Code"), ("codex", "Codex"), ("opencode", "opencode")]
        .iter()
        .filter(|(bin, _)| which(bin))
        .map(|(_, name)| *name)
        .collect();
    if no_worker {
        println!("  Built-in worker: off (--no-worker).");
    } else if worker_tokens.is_empty() {
        println!("  Built-in worker: off (no static worker token in hosted mode).");
    } else if agents.is_empty() {
        println!("  Built-in worker: off, no coding agent found here (claude, codex or opencode). Connect machines that have one.");
    } else if !repos.is_dir() {
        println!("  Built-in worker: off, {} does not exist. Pass --repos <folder with your git checkouts>.", repos.display());
    } else {
        println!("  Built-in worker: on ({} found). Up to {parallel} tasks at a time, each in its own git worktree under {}.", agents.join(", "), worktrees_dir().display());
        let id = default_worker_id();
        let cfg = WorkerConfig {
            control_url: format!("http://127.0.0.1:{port}"),
            token: first_token.clone(),
            display_name: id.clone(),
            worker_id: id,
            repos_root: repos,
            claude_bin: "claude".into(),
            codex_bin: "codex".into(),
            opencode_bin: "opencode".into(),
            permission_mode,
            max_turns: None,
            extra_args: vec![],
            poll_interval: Duration::from_secs(2),
            heartbeat_interval: Duration::from_secs(15),
            check_sign_in: true,
            scan_depth: 3,
            max_runs: None,
            parallel,
            worktrees_dir: worktrees_dir(),
            version: version_string().to_string(),
            allow_remote_update: true,
            approval_exe: std::env::current_exe().ok(),
            approval_timeout: Duration::from_secs(sidefleet_core::DEFAULT_APPROVAL_TIMEOUT_SECS),
        };
        tokio::spawn(async move {
            if let Err(e) = sidefleet_worker::worker::run(cfg).await {
                tracing::error!(error = %e, "built-in worker stopped");
            }
        });
    }
    println!();

    let claims = state.claims.clone();
    axum::serve(listener, sidefleet_server::router(state))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            // Answer workers' held claims, which would otherwise hold up the shutdown.
            claims.close();
            let n = sidefleet_worker::agent::kill_all_agents();
            if n > 0 {
                eprintln!("stopped {n} running agent(s)");
            }
        })
        .await?;
    Ok(())
}

/// Ctrl-C or SIGTERM (what launchd/systemd send on stop).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

// ---- connect / service ----

/// True when `token` is the one saved by an earlier `connect` to a different
/// control plane (saved settings are loaded into the environment at start).
fn token_saved_for_another_control(control: &str, token: &str) -> bool {
    let saved_token = std::env::var("SIDEFLEET_TOKEN").ok();
    let saved_control = std::env::var("SIDEFLEET_CONTROL").ok().map(|c| c.trim_end_matches('/').to_string());
    saved_token.as_deref() == Some(token) && saved_control.is_some_and(|c| c != control)
}

/// No token: ask the control plane to let this machine in, show the code and
/// the page to approve it on, and wait (up to ten minutes) for the token.
async fn wait_for_approval(control: &str, id: &str, display: &str, caps: &sidefleet_core::Capabilities) -> Result<String> {
    use sidefleet_worker::client::{enroll, enroll_status, EnrollStatus};
    let e = enroll(control, id, display, caps).await.map_err(|err| {
        anyhow::anyhow!("could not ask {control} to approve this machine: {err:#}\nIs `sidefleet serve` running there and reachable from here?")
    })?;
    println!();
    println!("This machine needs to be approved. Open:");
    println!();
    println!("    {}", e.approve_url);
    println!();
    println!("  and approve \"{display}\" with code  {}  (check that the page shows the same code).", e.code);
    println!("Waiting for approval (the request expires in 10 minutes)…");
    let mut failures = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(e.poll_interval_secs.clamp(1, 30))).await;
        match enroll_status(control, &e).await {
            Ok(EnrollStatus::Pending) => failures = 0,
            Ok(EnrollStatus::Approved { token, org }) => {
                println!("  Approved{}.", if org.is_empty() { String::new() } else { format!(" into \"{org}\"") });
                return Ok(token);
            }
            Ok(EnrollStatus::Denied) => bail!("the request was denied on the Machines page"),
            Ok(EnrollStatus::Expired) => bail!("nobody approved the request within 10 minutes; run `sidefleet connect` again"),
            Err(err) => {
                failures += 1;
                if failures >= 15 {
                    return Err(err.context("lost contact with the control plane while waiting for approval"));
                }
            }
        }
    }
}

async fn connect(control: String, token: Option<String>, repos: String, name: Option<String>, permission_mode: String, no_service: bool, parallel: usize) -> Result<()> {
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
        codex_bin: "codex".into(),
        opencode_bin: "opencode".into(),
        repos_root: repos_root.clone(),
        check_sign_in: true,
        scan_depth: 3,
    })
    .await;
    match &caps.agents.claude {
        Some(c) if c.signed_in => println!("  Claude Code {}: signed in", c.version),
        Some(c) => println!("  Claude Code {}: NOT signed in ({}). Run `claude` once to sign in; tasks will fail until then.", c.version, c.sign_in_error.as_deref().unwrap_or("unknown")),
        None => println!("  Claude Code: not found"),
    }
    match &caps.agents.codex {
        Some(c) if c.signed_in == Some(false) => println!("  Codex {}: NOT signed in. Run `codex login` once; Codex tasks will fail until then.", c.version),
        Some(c) => println!("  Codex {}: signed in, {} models", c.version, c.models.len()),
        None => println!("  Codex: not found"),
    }
    match &caps.agents.opencode {
        Some(o) if o.models_total == 0 => println!("  opencode {}: no providers set up. Run `opencode auth login` (e.g. OpenRouter) so it has models.", o.version),
        Some(o) => println!("  opencode {}: {} models", o.version, o.models_total),
        None => println!("  opencode: not found"),
    }
    if let Some(o) = &caps.agents.ollama {
        println!("  Ollama {} ({} models; add it as an opencode provider to run tasks with them)", o.version, o.models.len());
    }
    if caps.agents.runnable().is_empty() {
        println!("  No coding agent found. Install Claude Code, Codex or opencode and sign in, then run connect again; until then this machine takes no tasks.");
    }
    println!("  {} git checkouts under {}", caps.repos.len(), repos_root.display());
    if caps.repos.is_empty() {
        println!("  (none found; if your projects live somewhere unusual, pass --repos <folder>)");
    }

    let token = match token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) if !token_saved_for_another_control(&control, &t) => t,
        _ => wait_for_approval(&control, &id, &display, &caps).await?,
    };

    println!("Registering with {control}…");
    let client = sidefleet_worker::client::ControlClient::new(&control, &token, &id)?;
    match client.register(&display, &caps, &[]).await {
        Ok(_) => println!("  Registered as \"{display}\" ({id})"),
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("401") {
                bail!("the control plane rejected this token. Copy it from the Machines page or from where `sidefleet serve` printed it.");
            }
            if msg.contains("402") {
                bail!("the control plane refused this machine: {msg}");
            }
            bail!("could not reach the control plane at {control}: {msg}\nIs `sidefleet serve` running there, and is this address reachable from this machine (same LAN or Tailscale)?");
        }
    }

    let env = format!(
        "SIDEFLEET_CONTROL={control}\nSIDEFLEET_TOKEN={token}\nSIDEFLEET_REPOS={}\nSIDEFLEET_NAME={display}\nSIDEFLEET_PERMISSION_MODE={permission_mode}\nSIDEFLEET_PARALLEL={parallel}\n",
        repos_root.display()
    );
    // Keep anything else the user added (e.g. OPENROUTER_API_KEY for opencode).
    let kept: String = std::fs::read_to_string(worker_env_path())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("SIDEFLEET_"))
        .map(|l| format!("{l}\n"))
        .collect();
    write_private(&worker_env_path(), &format!("{env}{kept}"))?;
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

/// Install `<exe> <args…>` as a background service named after `kind` ("worker" or "serve").
#[cfg(target_os = "macos")]
fn install_named_service(kind: &str, exe: &Path, args: &[String], log: &Path) -> Result<String> {
    let label = format!("io.sidefleet.{kind}");
    let argv: String = std::iter::once(exe.to_string_lossy().to_string()).chain(args.iter().cloned())
        .map(|a| format!("<string>{}</string>", a.replace('&', "&amp;").replace('<', "&lt;"))).collect();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key><array>{argv}</array>
  <key>EnvironmentVariables</key><dict><key>PATH</key><string>{path}</string><key>HOME</key><string>{home}</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict></plist>
"#,
        path = service_path(), home = home().display(), log = log.display()
    );
    let p = home().join("Library/LaunchAgents").join(format!("{label}.plist"));
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, plist)?;
    let domain = format!("gui/{}", nix_uid());
    let _ = std::process::Command::new("launchctl").args(["bootout", &domain, &p.to_string_lossy()]).output();
    let out = std::process::Command::new("launchctl").args(["bootstrap", &domain, &p.to_string_lossy()]).output()?;
    if !out.status.success() {
        bail!("launchctl bootstrap failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(format!("Installed launchd service {label} (starts at login, restarts if it stops). Log: {}", log.display()))
}

#[cfg(target_os = "linux")]
fn install_named_service(kind: &str, exe: &Path, args: &[String], log: &Path) -> Result<String> {
    let unit_name = format!("sidefleet-{kind}");
    let unit = format!(
        "[Unit]\nDescription=Sidefleet {kind}\nAfter=network-online.target\n\n[Service]\nExecStart={exe} {args}\nRestart=always\nRestartSec=3\nEnvironment=PATH={path}\nStandardOutput=append:{log}\nStandardError=append:{log}\n\n[Install]\nWantedBy=default.target\n",
        exe = exe.display(), args = args.join(" "), path = service_path(), log = log.display()
    );
    let p = home().join(".config/systemd/user").join(format!("{unit_name}.service"));
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, unit)?;
    let run = |a: &[&str]| std::process::Command::new("systemctl").arg("--user").args(a).output();
    run(&["daemon-reload"])?;
    let out = run(&["enable", "--now", &unit_name])?;
    if !out.status.success() {
        bail!("systemctl --user enable failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let _ = std::process::Command::new("loginctl").args(["enable-linger"]).output();
    Ok(format!("Installed systemd user service {unit_name} (starts at boot, restarts if it stops). Log: {}", log.display()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_named_service(_kind: &str, _exe: &Path, _args: &[String], _log: &Path) -> Result<String> {
    bail!("background services are only supported on macOS and Linux")
}

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

/// PATH for the service: the user's PATH at connect time, so `claude`, `codex` and `opencode` are found.
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
            let secret = ["TOKEN", "KEY", "SECRET", "PASSWORD"].iter().any(|w| k.to_ascii_uppercase().contains(w));
            let v = if secret { mask(v) } else { v.to_string() };
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
