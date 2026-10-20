//! Capability detection: which agents are installed (and whether Claude Code is
//! signed in) and which git checkouts live under the repos root.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sidefleet_core::{Agents, Capabilities, ClaudeAgent, CodexAgent, OllamaAgent, RepoInfo};
use tokio::process::Command;

/// Environment variables stripped from every child process so a nested
/// Claude Code does not think it is running inside another session.
pub const STRIPPED_ENV: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"];

async fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Option<(bool, String, String)> {
    for k in STRIPPED_ENV {
        cmd.env_remove(k);
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output()).await.ok()?.ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
}

fn version_from(s: &str) -> String {
    // "2.1.302 (Claude Code)" -> "2.1.302"; "codex-cli 0.42.0" -> "0.42.0"
    s.split_whitespace()
        .find(|w| w.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
        .unwrap_or(s)
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.')
        .to_string()
}

pub async fn detect_claude(bin: &str, check_sign_in: bool) -> Option<ClaudeAgent> {
    let mut cmd = Command::new(bin);
    cmd.arg("--version");
    let (ok, out, _) = run_with_timeout(cmd, Duration::from_secs(15)).await?;
    if !ok {
        return None;
    }
    let version = version_from(&out);
    if !check_sign_in {
        return Some(ClaudeAgent { version, signed_in: false, sign_in_error: Some("not checked".into()) });
    }
    // A trivial prompt: succeeds only when the CLI has valid credentials.
    let mut cmd = Command::new(bin);
    cmd.args(["-p", "Reply with the single word OK.", "--output-format", "json", "--max-turns", "1"]);
    cmd.current_dir(std::env::temp_dir());
    match run_with_timeout(cmd, Duration::from_secs(60)).await {
        Some((true, out, _)) => {
            let is_error = serde_json::from_str::<serde_json::Value>(&out)
                .ok()
                .and_then(|v| v.get("is_error").and_then(|b| b.as_bool()))
                .unwrap_or(false);
            Some(ClaudeAgent { version, signed_in: !is_error, sign_in_error: is_error.then(|| sidefleet_core::redact(&out)) })
        }
        Some((false, out, err)) => {
            let msg = if err.is_empty() { out } else { err };
            Some(ClaudeAgent { version, signed_in: false, sign_in_error: Some(sidefleet_core::stream::truncate_bytes(&sidefleet_core::redact(&msg), 400)) })
        }
        None => Some(ClaudeAgent { version, signed_in: false, sign_in_error: Some("sign-in check timed out".into()) }),
    }
}

pub async fn detect_codex() -> Option<CodexAgent> {
    let mut cmd = Command::new("codex");
    cmd.arg("--version");
    let (ok, out, _) = run_with_timeout(cmd, Duration::from_secs(10)).await?;
    ok.then(|| CodexAgent { version: version_from(&out) })
}

pub async fn detect_ollama() -> Option<OllamaAgent> {
    let mut cmd = Command::new("ollama");
    cmd.arg("--version");
    let (ok, out, _) = run_with_timeout(cmd, Duration::from_secs(10)).await?;
    if !ok {
        return None;
    }
    let version = version_from(&out);
    let mut cmd = Command::new("ollama");
    cmd.arg("list");
    let models = match run_with_timeout(cmd, Duration::from_secs(10)).await {
        Some((true, out, _)) => out
            .lines()
            .skip(1)
            .filter_map(|l| l.split_whitespace().next().map(str::to_string))
            .collect(),
        _ => vec![],
    };
    Some(OllamaAgent { version, models })
}

/// Folders never worth descending into when looking for git checkouts.
const SKIP_DIRS: &[&str] = &[
    "Library", "Applications", "Music", "Movies", "Pictures", "Public", "Desktop.localized",
    "node_modules", "target", "vendor", "dist", "build", "venv", "env", "__pycache__",
    "go", "snap", "Trash",
];

/// Default scan depth when the root is the home folder.
pub const HOME_SCAN_DEPTH: usize = 3;

/// Git checkouts under `root`, up to `max_depth` levels deep (the root itself
/// counts when it is a repo). Names are paths relative to `root`. Hidden
/// folders and well-known noise (see `SKIP_DIRS`) are not entered.
pub fn scan_repos(root: &Path, max_depth: usize) -> Vec<RepoInfo> {
    let mut out = Vec::new();
    fn walk(root: &Path, dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<RepoInfo>) {
        if dir.join(".git").exists() {
            let name = if dir == root {
                ".".to_string()
            } else {
                dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().to_string()
            };
            out.push(RepoInfo { name, path: dir.to_string_lossy().to_string() });
            return; // do not descend into nested repos / submodules
        }
        if depth >= max_depth {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut children: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                p.is_dir() && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) && !p.is_symlink()
            })
            .collect();
        children.sort();
        for c in children {
            walk(root, &c, depth + 1, max_depth, out);
        }
    }
    walk(root, root, 0, max_depth, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub struct DetectOptions {
    pub claude_bin: String,
    pub repos_root: PathBuf,
    pub check_sign_in: bool,
    pub scan_depth: usize,
}

pub async fn detect_all(opts: &DetectOptions) -> Capabilities {
    let (claude, codex, ollama) = tokio::join!(
        detect_claude(&opts.claude_bin, opts.check_sign_in),
        detect_codex(),
        detect_ollama()
    );
    Capabilities {
        agents: Agents { claude, codex, ollama },
        repos: scan_repos(&opts.repos_root, opts.scan_depth),
        hostname: gethostname::gethostname().to_string_lossy().to_string(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(version_from("2.1.302 (Claude Code)"), "2.1.302");
        assert_eq!(version_from("codex-cli 0.42.0"), "0.42.0");
        assert_eq!(version_from("ollama version is 0.5.7"), "0.5.7");
    }

    #[test]
    fn scans_repos_two_levels_deep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("a/.git")).unwrap();
        std::fs::create_dir_all(root.join("group/b/.git")).unwrap();
        std::fs::create_dir_all(root.join("group/deep/c/.git")).unwrap();
        std::fs::create_dir_all(root.join(".hidden/d/.git")).unwrap();
        std::fs::create_dir_all(root.join("a/nested/.git")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg/.git")).unwrap();
        std::fs::create_dir_all(root.join("Library/x/.git")).unwrap();
        let names: Vec<String> = scan_repos(root, 2).into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["a", "group/b"]);
        let names: Vec<String> = scan_repos(root, 3).into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["a", "group/b", "group/deep/c"]);
    }
}
