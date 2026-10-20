//! Self-update: rebuild or re-download the binary, then restart the services
//! that run it. Used by `sidefleet update` and by the Update button on the page.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

pub const INSTALL_URL: &str = "https://raw.githubusercontent.com/foodaka/side-fleet/main/install.sh";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Where install.sh recorded how this binary was installed (`SIDEFLEET_SOURCE=<checkout>`).
pub fn install_env_path() -> PathBuf {
    home().join(".sidefleet").join("install.env")
}

pub fn source_checkout() -> Option<PathBuf> {
    let text = std::fs::read_to_string(install_env_path()).ok()?;
    let dir = text
        .lines()
        .find_map(|l| l.strip_prefix("SIDEFLEET_SOURCE="))
        .map(|v| PathBuf::from(v.trim()))?;
    dir.join("install.sh").exists().then_some(dir)
}

#[derive(Debug, Default)]
pub struct UpdateReport {
    pub source: String,
    pub pulled: String,
    pub installed_to: PathBuf,
    pub restarted: Vec<String>,
    pub needs_manual_restart: Vec<String>,
}

fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().with_context(|| format!("running {:?}", cmd.get_program()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        bail!("{:?} failed: {}{}", cmd.get_program(), stdout, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(stdout)
}

/// Pull (or download) and reinstall the binary over the one currently running,
/// then restart installed services. Returns what happened; the caller decides
/// how to print it. When this process is itself one of the restarted services,
/// it will be killed by the restart, which is the point.
pub fn update(restart: bool) -> Result<UpdateReport> {
    let exe = std::env::current_exe().context("locating the running binary")?;
    let bin_dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| home().join(".local/bin"));
    let mut report = UpdateReport { installed_to: exe.clone(), ..Default::default() };

    match source_checkout() {
        Some(src) => {
            report.source = src.display().to_string();
            report.pulled = run(Command::new("git").arg("-C").arg(&src).args(["pull", "--ff-only"]))?;
            run(Command::new("sh").arg(src.join("install.sh")).env("SIDEFLEET_BIN_DIR", &bin_dir))?;
        }
        None => {
            report.source = INSTALL_URL.into();
            let script = run(Command::new("curl").args(["-fsSL", INSTALL_URL]))?;
            let mut child = Command::new("sh")
                .env("SIDEFLEET_BIN_DIR", &bin_dir)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            use std::io::Write;
            child.stdin.take().unwrap().write_all(script.as_bytes())?;
            let out = child.wait_with_output()?;
            if !out.status.success() {
                bail!("install script failed: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
        }
    }

    if restart {
        restart_services(&mut report);
    }
    Ok(report)
}

fn uid() -> String {
    Command::new("id").arg("-u").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "501".into())
}

fn restart_services(report: &mut UpdateReport) {
    #[cfg(target_os = "macos")]
    {
        for label in ["io.sidefleet.worker", "io.sidefleet.serve"] {
            let plist = home().join("Library/LaunchAgents").join(format!("{label}.plist"));
            if plist.exists() {
                let target = format!("gui/{}/{label}", uid());
                let ok = Command::new("launchctl").args(["kickstart", "-k", &target]).status().map(|s| s.success()).unwrap_or(false);
                if ok { report.restarted.push(label.into()); } else { report.needs_manual_restart.push(label.into()); }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        for unit in ["sidefleet-worker", "sidefleet-serve"] {
            let path = home().join(".config/systemd/user").join(format!("{unit}.service"));
            if path.exists() {
                let ok = Command::new("systemctl").args(["--user", "restart", unit]).status().map(|s| s.success()).unwrap_or(false);
                if ok { report.restarted.push(unit.into()); } else { report.needs_manual_restart.push(unit.into()); }
            }
        }
    }
    // A `serve` or `run` started by hand in a terminal is not a service; tell the user.
    if let Ok(out) = Command::new("pgrep").args(["-fl", "sidefleet (serve|run)"]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if line.contains("serve") && !report.restarted.iter().any(|r| r.contains("serve")) {
                report.needs_manual_restart.push("sidefleet serve (running in a terminal)".into());
                break;
            }
        }
    }
}
