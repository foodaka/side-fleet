//! Capability detection: which agents are installed (whether they are signed
//! in, which models they offer) and which git checkouts live under the repos root.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sidefleet_core::{repo_identity, Agents, Capabilities, ClaudeAgent, CodexAgent, OllamaAgent, OpencodeAgent, RepoInfo, SystemStats};
use std::sync::Mutex;
use tokio::process::Command;

/// Environment variables stripped from every child process so a nested
/// Claude Code does not think it is running inside another session.
pub const STRIPPED_ENV: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"];

/// Where npm, pnpm, bun, volta and the agents' own installers put executables.
/// A background service's PATH is the shell's at `sidefleet connect` time, so
/// an agent installed later (or under another nvm Node) is often not on it.
fn user_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        ".local/bin", ".npm-global/bin", ".local/share/pnpm", "Library/pnpm", ".bun/bin", ".volta/bin",
        ".opencode/bin", ".claude/local", ".cargo/bin",
    ]
    .iter()
    .map(|d| home.join(d))
    .collect();
    // nvm: every installed Node's bin, newest version first.
    if let Ok(rd) = std::fs::read_dir(home.join(".nvm/versions/node")) {
        let mut nodes: Vec<(Vec<u64>, PathBuf)> = rd
            .filter_map(|e| e.ok())
            .map(|e| {
                let v = e.file_name().to_string_lossy().trim_start_matches('v').split('.').filter_map(|n| n.parse().ok()).collect();
                (v, e.path().join("bin"))
            })
            .collect();
        nodes.sort_by(|a, b| b.0.cmp(&a.0));
        dirs.extend(nodes.into_iter().map(|(_, p)| p));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// The executable to run for `bin`: as given when it is a path or on PATH,
/// otherwise the first match in the usual user install folders.
pub fn resolve_bin(bin: &str) -> PathBuf {
    resolve_in(bin, std::env::var_os("PATH").as_deref(), &home_dir())
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn resolve_in(bin: &str, path: Option<&std::ffi::OsStr>, home: &Path) -> PathBuf {
    if bin.contains('/') || path.is_some_and(|p| std::env::split_paths(p).any(|d| is_executable(&d.join(bin)))) {
        return PathBuf::from(bin);
    }
    user_bin_dirs(home).into_iter().map(|d| d.join(bin)).find(|p| is_executable(p)).unwrap_or_else(|| PathBuf::from(bin))
}

/// A command for an agent CLI, found even when it is not on PATH. Its folder is
/// put first on the child's PATH: npm-installed CLIs start with
/// `#!/usr/bin/env node`, and the right `node` lives next to them.
pub fn agent_command(bin: &str) -> Command {
    let exe = resolve_bin(bin);
    let mut cmd = Command::new(&exe);
    if let Some(dir) = exe.parent().filter(|d| exe.is_absolute() && !d.as_os_str().is_empty()) {
        let mut dirs = vec![dir.to_path_buf()];
        dirs.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
        if let Ok(p) = std::env::join_paths(dirs) {
            cmd.env("PATH", p);
        }
    }
    cmd
}

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
    let mut cmd = agent_command(bin);
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
    let mut cmd = agent_command(bin);
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

/// Codex: version, sign-in (`codex login status`, no model call) and the models
/// its catalog lists for this account (`codex debug models`).
pub async fn detect_codex(bin: &str, check_sign_in: bool) -> Option<CodexAgent> {
    let mut cmd = agent_command(bin);
    cmd.arg("--version");
    let (ok, out, _) = run_with_timeout(cmd, Duration::from_secs(10)).await?;
    if !ok {
        return None;
    }
    let version = version_from(&out);
    let signed_in = if check_sign_in {
        let mut cmd = agent_command(bin);
        cmd.args(["login", "status"]);
        Some(matches!(run_with_timeout(cmd, Duration::from_secs(15)).await, Some((true, out, err)) if format!("{out}{err}").contains("Logged in")))
    } else {
        None
    };
    let mut cmd = agent_command(bin);
    cmd.args(["debug", "models"]);
    let models = match run_with_timeout(cmd, Duration::from_secs(15)).await {
        Some((true, out, _)) => codex_models(&out),
        _ => vec![],
    };
    Some(CodexAgent { version, signed_in, models })
}

/// The `list`-visible slugs from `codex debug models`, in Codex's own order.
pub fn codex_models(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let list = v.get("models").unwrap_or(&v).as_array().cloned().unwrap_or_default();
    let mut models: Vec<(i64, String)> = list
        .iter()
        .filter(|m| m.get("visibility").and_then(|x| x.as_str()).unwrap_or("list") == "list")
        .filter_map(|m| {
            let slug = m.get("slug").and_then(|x| x.as_str())?;
            Some((m.get("priority").and_then(|x| x.as_i64()).unwrap_or(i64::MAX), slug.to_string()))
        })
        .collect();
    models.sort();
    models.into_iter().map(|(_, s)| s).collect()
}

/// How many opencode models a machine reports; OpenRouter alone has hundreds.
pub const OPENCODE_MODELS_MAX: usize = 400;

/// opencode: version and the `provider/model` ids it can use here (only
/// providers that are configured or signed in are listed).
pub async fn detect_opencode(bin: &str) -> Option<OpencodeAgent> {
    let mut cmd = agent_command(bin);
    cmd.arg("--version");
    let (ok, out, _) = run_with_timeout(cmd, Duration::from_secs(15)).await?;
    if !ok {
        return None;
    }
    let version = version_from(&out);
    let mut cmd = agent_command(bin);
    cmd.arg("models");
    let all: Vec<String> = match run_with_timeout(cmd, Duration::from_secs(30)).await {
        Some((true, out, _)) => out
            .lines()
            .map(str::trim)
            .filter(|l| l.contains('/') && !l.contains(char::is_whitespace))
            .map(str::to_string)
            .collect(),
        _ => vec![],
    };
    let models_total = all.len();
    Some(OpencodeAgent { version, models: all.into_iter().take(OPENCODE_MODELS_MAX).collect(), models_total })
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

/// Identity for a remote URL. SSH remotes often use a host alias from
/// `~/.ssh/config` (`git@github.com-work:org/repo.git`); resolve it to the real
/// host so every machine agrees on the identity.
pub fn identity_for(url: &str) -> Option<String> {
    let id = repo_identity(url)?;
    if url.trim_start().starts_with("http") {
        return Some(id);
    }
    let (host, rest) = id.split_once('/')?;
    Some(format!("{}/{rest}", ssh_hostname(host)))
}

/// `ssh -G <alias>` prints the effective config without connecting; cached per host.
fn ssh_hostname(alias: &str) -> String {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(h) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(alias) {
        return h.clone();
    }
    let resolved = std::process::Command::new("ssh")
        .args(["-G", alias])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|l| l.strip_prefix("hostname ").map(|h| h.trim().to_ascii_lowercase()))
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| alias.to_string());
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(alias.to_string(), resolved.clone());
    resolved
}

/// The `origin` remote URL of a checkout, read from its git config without
/// spawning git (a machine can have hundreds of checkouts).
pub fn origin_url(repo: &Path) -> Option<String> {
    let dotgit = repo.join(".git");
    let config = if dotgit.is_dir() {
        dotgit.join("config")
    } else {
        // A linked worktree or submodule: `.git` is a file pointing at the real git dir.
        let text = std::fs::read_to_string(&dotgit).ok()?;
        let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        let gitdir = if gitdir.is_absolute() { gitdir } else { repo.join(gitdir) };
        // Worktrees keep the shared config in their `commondir`.
        let common = std::fs::read_to_string(gitdir.join("commondir")).ok().map(|c| gitdir.join(c.trim()));
        common.unwrap_or(gitdir).join("config")
    };
    parse_origin(&std::fs::read_to_string(config).ok()?)
}

fn parse_origin(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_origin = l.replace(char::is_whitespace, "") == "[remote\"origin\"]";
        } else if in_origin {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim() == "url" {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
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
            let id = origin_url(dir).and_then(|u| identity_for(&u)).unwrap_or_else(|| name.clone());
            out.push(RepoInfo { name, path: dir.to_string_lossy().to_string(), id });
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

/// Live machine numbers. Keeps one `sysinfo::System` so CPU usage is measured
/// between consecutive calls (the first call reports 0%).
pub fn system_stats(repos_root: &Path) -> SystemStats {
    use sysinfo::{Disks, System};
    static SYS: Mutex<Option<System>> = Mutex::new(None);
    let mut guard = SYS.lock().unwrap_or_else(|e| e.into_inner());
    let sys = guard.get_or_insert_with(System::new);
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let disks = Disks::new_with_refreshed_list();
    // The disk whose mount point is the longest prefix of the repos folder.
    let root = std::fs::canonicalize(repos_root).unwrap_or_else(|_| repos_root.to_path_buf());
    let disk = disks
        .iter()
        .filter(|d| root.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len());
    let gb = |b: u64| (b as f64) / 1_073_741_824.0;
    SystemStats {
        cpu_cores: sys.cpus().len(),
        cpu_percent: sys.global_cpu_usage(),
        load_avg_1m: System::load_average().one,
        mem_total_mb: sys.total_memory() / 1_048_576,
        mem_used_mb: sys.used_memory() / 1_048_576,
        disk_total_gb: disk.map(|d| gb(d.total_space())).unwrap_or(0.0),
        disk_free_gb: disk.map(|d| gb(d.available_space())).unwrap_or(0.0),
        uptime_secs: System::uptime(),
    }
}

#[derive(Clone)]
pub struct DetectOptions {
    pub claude_bin: String,
    pub codex_bin: String,
    pub opencode_bin: String,
    pub repos_root: PathBuf,
    pub check_sign_in: bool,
    pub scan_depth: usize,
}

pub async fn detect_all(opts: &DetectOptions) -> Capabilities {
    let (claude, codex, opencode, ollama) = tokio::join!(
        detect_claude(&opts.claude_bin, opts.check_sign_in),
        detect_codex(&opts.codex_bin, opts.check_sign_in),
        detect_opencode(&opts.opencode_bin),
        detect_ollama()
    );
    Capabilities {
        agents: Agents { claude, codex, opencode, ollama },
        repos: scan_repos(&opts.repos_root, opts.scan_depth),
        hostname: gethostname::gethostname().to_string_lossy().to_string(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        version: String::new(),
        system: Some(system_stats(&opts.repos_root)),
        slots: None,
    }
}

/// Looks for the agents missing from `current`; the updated set when any turned up.
pub async fn detect_missing(current: &Agents, opts: &DetectOptions) -> Option<Agents> {
    let (claude, codex, opencode, ollama) = tokio::join!(
        async { if current.claude.is_none() { detect_claude(&opts.claude_bin, opts.check_sign_in).await } else { None } },
        async { if current.codex.is_none() { detect_codex(&opts.codex_bin, opts.check_sign_in).await } else { None } },
        async { if current.opencode.is_none() { detect_opencode(&opts.opencode_bin).await } else { None } },
        async { if current.ollama.is_none() { detect_ollama().await } else { None } },
    );
    if claude.is_none() && codex.is_none() && opencode.is_none() && ollama.is_none() {
        return None;
    }
    let mut agents = current.clone();
    agents.claude = agents.claude.or(claude);
    agents.codex = agents.codex.or(codex);
    agents.opencode = agents.opencode.or(opencode);
    agents.ollama = agents.ollama.or(ollama);
    Some(agents)
}

#[cfg(test)]
mod tests {

    #[test]
    fn finds_agents_outside_path() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let exe = |p: &Path| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        exe(&home.join(".nvm/versions/node/v9.0.0/bin/codex"));
        exe(&home.join(".nvm/versions/node/v22.1.0/bin/codex"));
        exe(&home.join(".opencode/bin/opencode"));
        let empty = std::ffi::OsStr::new("/nonexistent");
        assert_eq!(resolve_in("codex", Some(empty), home), home.join(".nvm/versions/node/v22.1.0/bin/codex"), "newest Node first");
        assert_eq!(resolve_in("opencode", Some(empty), home), home.join(".opencode/bin/opencode"));
        assert_eq!(resolve_in("claude", Some(empty), home), PathBuf::from("claude"), "not found: left to PATH");
        assert_eq!(resolve_in("/opt/x/codex", Some(empty), home), PathBuf::from("/opt/x/codex"));
        let on_path = home.join(".nvm/versions/node/v9.0.0/bin");
        assert_eq!(resolve_in("codex", Some(on_path.as_os_str()), home), PathBuf::from("codex"), "PATH wins");
    }
    use super::*;

    #[test]
    fn reads_origin_and_derives_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("code/side-fleet/.git")).unwrap();
        std::fs::write(
            root.join("code/side-fleet/.git/config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:other/x.git\n[remote \"origin\"]\n\turl = git@github.com:foodaka/side-fleet.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("scratch/.git")).unwrap();
        std::fs::write(root.join("scratch/.git/config"), "[core]\n\tbare = false\n").unwrap();
        let repos = scan_repos(root, 3);
        let by = |n: &str| repos.iter().find(|r| r.name == n).unwrap().clone();
        assert_eq!(by("code/side-fleet").id, "github.com/foodaka/side-fleet");
        assert_eq!(by("scratch").id, "scratch", "no remote: the folder name is the identity");
    }

    #[test]
    fn https_remotes_skip_ssh_resolution_and_unknown_aliases_stay() {
        assert_eq!(identity_for("https://github.com/foodaka/side-fleet.git").as_deref(), Some("github.com/foodaka/side-fleet"));
        // An alias ssh can't resolve keeps its name rather than failing.
        assert_eq!(
            identity_for("git@no-such-alias.invalid:foodaka/side-fleet.git").as_deref(),
            Some("no-such-alias.invalid/foodaka/side-fleet")
        );
    }

    #[test]
    fn lists_codex_models_in_catalog_order() {
        let json = r#"{"models":[{"slug":"gpt-reserve","visibility":"hide","priority":4},{"slug":"gpt-5.6-terra","visibility":"list","priority":8},{"slug":"gpt-6-luna","visibility":"list","priority":5}]}"#;
        assert_eq!(codex_models(json), vec!["gpt-6-luna", "gpt-5.6-terra"]);
        assert!(codex_models("not json").is_empty());
    }

    #[test]
    fn parses_versions() {
        assert_eq!(version_from("2.1.302 (Claude Code)"), "2.1.302");
        assert_eq!(version_from("codex-cli 0.42.0"), "0.42.0");
        assert_eq!(version_from("ollama version is 0.5.7"), "0.5.7");
        assert_eq!(version_from("1.19.3"), "1.19.3");
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
