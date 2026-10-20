use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "acw", about = "AgentCloud worker: runs coding agents installed on this machine")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Register with the control plane and execute runs.
    Run {
        /// Control plane base URL, e.g. http://100.64.0.1:8080
        #[arg(long, env = "ACW_CONTROL")]
        control: String,
        /// Worker token (matches one of ACD_WORKER_TOKENS on the control plane).
        #[arg(long, env = "ACW_TOKEN", hide_env_values = true)]
        token: String,
        /// Directory scanned for git checkouts.
        #[arg(long, env = "ACW_REPOS", default_value = "~/Code")]
        repos: String,
        /// Worker id (defaults to the hostname).
        #[arg(long, env = "ACW_ID")]
        id: Option<String>,
        /// Human-friendly name shown in the UI (defaults to the id).
        #[arg(long, env = "ACW_NAME")]
        name: Option<String>,
        /// Claude Code executable.
        #[arg(long, env = "ACW_CLAUDE_BIN", default_value = "claude")]
        claude_bin: String,
        /// Permission mode passed to `claude -p` (default, acceptEdits, bypassPermissions, plan, dontAsk).
        #[arg(long, env = "ACW_PERMISSION_MODE", default_value = "acceptEdits")]
        permission_mode: String,
        /// Cap on agent turns per run.
        #[arg(long, env = "ACW_MAX_TURNS")]
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
    /// Print detected capabilities and exit.
    Detect {
        #[arg(long, env = "ACW_REPOS", default_value = "~/Code")]
        repos: String,
        #[arg(long, env = "ACW_CLAUDE_BIN", default_value = "claude")]
        claude_bin: String,
        #[arg(long)]
        skip_sign_in_check: bool,
        #[arg(long, default_value_t = 2)]
        scan_depth: usize,
    },
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "acw=info".into()))
        .init();
    match Cli::parse().cmd {
        Cmd::Run {
            control,
            token,
            repos,
            id,
            name,
            claude_bin,
            permission_mode,
            max_turns,
            claude_args,
            poll_interval,
            skip_sign_in_check,
            scan_depth,
        } => {
            let id = id.unwrap_or_else(acw::worker::default_worker_id);
            let cfg = acw::worker::WorkerConfig {
                control_url: control,
                token,
                display_name: name.unwrap_or_else(|| id.clone()),
                worker_id: id,
                repos_root: expand_home(&repos),
                claude_bin,
                permission_mode,
                max_turns,
                extra_args: claude_args,
                poll_interval: Duration::from_secs(poll_interval.max(1)),
                heartbeat_interval: Duration::from_secs(15),
                check_sign_in: !skip_sign_in_check,
                scan_depth,
                max_runs: None,
            };
            acw::worker::run(cfg).await
        }
        Cmd::Detect { repos, claude_bin, skip_sign_in_check, scan_depth } => {
            let caps = acw::detect::detect_all(&acw::detect::DetectOptions {
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
