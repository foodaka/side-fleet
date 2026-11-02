//! Dev servers a task left running: processes listening on TCP whose working
//! directory is inside one of this machine's task worktrees
//! (`<worktrees_dir>/<repo-slug>/<root run id>`). Only those are ever reported or
//! stopped: never a process in a checkout (an in-place task, or the person's own),
//! never an agent this worker runs, never the worker itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid as NixPid;
use sidefleet_core::{redact, DevServer, DevServerStop};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use uuid::Uuid;

/// Commands are shortened to this many characters.
const COMMAND_MAX_CHARS: usize = 200;
/// How long a stopped server gets to exit on SIGTERM before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// The thread whose worktree `cwd` is in, if it is in one.
pub fn thread_of(worktrees_dir: &Path, cwd: &Path) -> Option<Uuid> {
    let rest = cwd.strip_prefix(worktrees_dir).ok()?;
    let mut parts = rest.components();
    let _slug = parts.next()?;
    parts.next()?.as_os_str().to_str()?.parse().ok()
}

fn canonical(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

fn processes() -> System {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always).with_cmd(UpdateKind::Always),
    );
    sys
}

/// Pids that must never count as a dev server: this worker and the agents it runs.
fn protected() -> HashSet<u32> {
    let mut p: HashSet<u32> = crate::agent::live_agents().into_iter().map(|p| p as u32).collect();
    p.insert(std::process::id());
    p
}

/// Processes in a task worktree, by pid, with their thread.
fn in_worktrees(sys: &System, worktrees_dir: &Path) -> HashMap<u32, Uuid> {
    let skip = protected();
    sys.processes()
        .iter()
        .filter(|(pid, _)| !skip.contains(&pid.as_u32()))
        .filter_map(|(pid, p)| Some((pid.as_u32(), thread_of(worktrees_dir, p.cwd()?)?)))
        .collect()
}

fn command(p: &sysinfo::Process) -> String {
    let cmd: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let line = if cmd.is_empty() { p.name().to_string_lossy().into_owned() } else { cmd.join(" ") };
    let line = redact(line.trim());
    if line.chars().count() > COMMAND_MAX_CHARS {
        format!("{}…", line.chars().take(COMMAND_MAX_CHARS - 1).collect::<String>())
    } else {
        line
    }
}

fn started(p: &sysinfo::Process) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(p.start_time() as i64, 0).filter(|_| p.start_time() > 0)
}

/// Every dev server in this machine's task worktrees. Cheap when there are none:
/// sockets are only looked up for processes already found in a worktree.
pub fn scan(worktrees_dir: &Path) -> Vec<DevServer> {
    let dir = canonical(worktrees_dir);
    let sys = processes();
    let candidates = in_worktrees(&sys, &dir);
    if candidates.is_empty() {
        return Vec::new();
    }
    let pids: Vec<u32> = candidates.keys().copied().collect();
    let mut out: Vec<DevServer> = listening_ports(&pids)
        .into_iter()
        .filter_map(|(pid, mut ports)| {
            let p = sys.process(Pid::from_u32(pid))?;
            ports.sort_unstable();
            ports.dedup();
            Some(DevServer { thread: candidates[&pid], pid, ports, command: command(p), started_at: started(p) })
        })
        .collect();
    out.sort_by_key(|s| (s.thread, s.pid));
    out
}

/// What a stop did.
#[derive(Debug, PartialEq)]
pub enum Stopped {
    /// It exited on SIGTERM.
    Term,
    /// It needed SIGKILL.
    Kill,
    /// It had already gone.
    Gone,
}

/// Stop a dev server and the processes it started, after checking it is still
/// the same process (pid, start time) in the same thread's worktree.
pub async fn stop(worktrees_dir: &Path, req: &DevServerStop) -> Result<Stopped> {
    let dir = canonical(worktrees_dir);
    let sys = processes();
    let Some(p) = sys.process(Pid::from_u32(req.pid)) else { return Ok(Stopped::Gone) };
    if req.started_at.is_some_and(|t| started(p) != Some(t)) {
        return Ok(Stopped::Gone); // a later process got the pid
    }
    if protected().contains(&req.pid) {
        bail!("pid {} is an agent or the worker itself", req.pid);
    }
    if p.cwd().and_then(|c| thread_of(&dir, c)) != Some(req.thread) {
        bail!("pid {} is not in the worktree of thread {}", req.pid, req.thread);
    }
    // The server and what it started (a bundler's watcher, say), children first.
    let mut targets = descendants(&sys, req.pid);
    targets.push(req.pid);
    let protect = protected();
    targets.retain(|p| !protect.contains(p));
    for &pid in &targets {
        let _ = kill(NixPid::from_raw(pid as i32), Signal::SIGTERM);
    }
    let deadline = tokio::time::Instant::now() + STOP_GRACE;
    while tokio::time::Instant::now() < deadline {
        if !targets.iter().any(|&p| alive(p)) {
            return Ok(Stopped::Term);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for &pid in &targets {
        let _ = kill(NixPid::from_raw(pid as i32), Signal::SIGKILL);
    }
    Ok(Stopped::Kill)
}

/// Whether `pid` is still running (a zombie waiting to be reaped counts as gone).
fn alive(pid: u32) -> bool {
    if kill(NixPid::from_raw(pid as i32), None).is_err() {
        return false;
    }
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // `pid (comm) S ...`: the state follows the last `)`.
        return stat.rsplit(')').next().and_then(|s| s.trim_start().chars().next()) != Some('Z');
    }
    true
}

fn descendants(sys: &System, root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, p) in sys.processes() {
        if let Some(parent) = p.parent() {
            children.entry(parent.as_u32()).or_default().push(pid.as_u32());
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut seen = HashSet::from([root]);
    while let Some(p) = stack.pop() {
        for &c in children.get(&p).into_iter().flatten() {
            if seen.insert(c) {
                out.push(c);
                stack.push(c);
            }
        }
    }
    out.reverse();
    out
}

/// TCP ports each of `pids` listens on (pids without any are left out).
#[cfg(target_os = "linux")]
fn listening_ports(pids: &[u32]) -> HashMap<u32, Vec<u16>> {
    let mut inodes = HashMap::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(table) {
            inodes.extend(parse_tcp_table(&text));
        }
    }
    let mut out: HashMap<u32, Vec<u16>> = HashMap::new();
    if inodes.is_empty() {
        return out;
    }
    for &pid in pids {
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        for fd in fds.flatten() {
            let Ok(link) = std::fs::read_link(fd.path()) else { continue };
            if let Some(port) = socket_inode(&link.to_string_lossy()).and_then(|i| inodes.get(&i)) {
                out.entry(pid).or_default().push(*port);
            }
        }
    }
    out
}

/// macOS (and other Unixes): `lsof`, limited to the candidate pids.
#[cfg(not(target_os = "linux"))]
fn listening_ports(pids: &[u32]) -> HashMap<u32, Vec<u16>> {
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let Ok(out) = std::process::Command::new("lsof").args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-p", &list, "-Fpn"]).output() else {
        return HashMap::new();
    };
    parse_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// `lsof -Fpn` output: `p<pid>` starts a process, `n<addr>:<port>` is one of its sockets.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn parse_lsof(text: &str) -> HashMap<u32, Vec<u16>> {
    let mut out: HashMap<u32, Vec<u16>> = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(n), Some(pid)) = (line.strip_prefix('n'), pid) {
            if let Some(port) = n.rsplit(':').next().and_then(|p| p.parse().ok()) {
                out.entry(pid).or_default().push(port);
            }
        }
    }
    out
}

/// Socket inode -> local port for the listening sockets in a `/proc/net/tcp{,6}` table.
pub fn parse_tcp_table(text: &str) -> HashMap<u64, u16> {
    const LISTEN: &str = "0A";
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != LISTEN {
                return None;
            }
            let port = u16::from_str_radix(f[1].rsplit(':').next()?, 16).ok()?;
            Some((f[9].parse().ok()?, port))
        })
        .collect()
}

/// The inode of an fd link like `socket:[12345]`.
pub fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_paths_map_to_their_thread() {
        let dir = Path::new("/home/me/.sidefleet/worktrees");
        let id = Uuid::new_v4();
        assert_eq!(thread_of(dir, &dir.join("acme-shop").join(id.to_string())), Some(id));
        assert_eq!(thread_of(dir, &dir.join("acme-shop").join(id.to_string()).join("web")), Some(id));
        assert_eq!(thread_of(dir, &dir.join("acme-shop")), None);
        assert_eq!(thread_of(dir, &dir.join("acme-shop").join("not-a-run")), None);
        assert_eq!(thread_of(dir, Path::new("/home/me/code/acme-shop")), None);
        assert_eq!(thread_of(dir, Path::new("/home/me/.sidefleet/worktrees-other/x").join(id.to_string()).as_path()), None);
    }

    #[test]
    fn tcp_table_lists_only_listening_sockets() {
        let v4 = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0
   1: 0100007F:A4F2 0100007F:0BB8 01 00000000:00000000 00:00000000 00000000  1000        0 4343 1 0000000000000000 20 4 30 10 -1
";
        let v6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5151 1 0000000000000000 100 0 0 10 0
";
        let mut m = parse_tcp_table(v4);
        m.extend(parse_tcp_table(v6));
        assert_eq!(m, HashMap::from([(4242, 3000), (5151, 8080)]));
    }

    #[test]
    fn socket_links_and_lsof_output_parse() {
        assert_eq!(socket_inode("socket:[4242]"), Some(4242));
        assert_eq!(socket_inode("pipe:[4242]"), None);
        assert_eq!(socket_inode("/dev/null"), None);
        let lsof = "p501\nf20\nn*:3000\nf21\nn[::1]:3001\np777\nf5\nn127.0.0.1:5173\n";
        assert_eq!(parse_lsof(lsof), HashMap::from([(501, vec![3000, 3001]), (777, vec![5173])]));
    }

    async fn listening(dir: &Path) -> (tokio::process::Child, u16) {
        let mut child = tokio::process::Command::new("python3")
            .args(["-c", "import socket,sys,time\ns=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); print(s.getsockname()[1], flush=True); time.sleep(600)"])
            .current_dir(dir)
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("python3");
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        tokio::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).await.unwrap();
        (child, line.trim().parse().unwrap())
    }

    /// A real server in a task worktree is found and stopped; one elsewhere is never seen.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn finds_and_stops_a_server_in_a_worktree_only() {
        let tmp = tempfile::tempdir().unwrap();
        let wts = tmp.path().join("worktrees");
        let thread = Uuid::new_v4();
        let wt = wts.join("acme-shop").join(thread.to_string());
        let outside = tmp.path().join("code").join("acme-shop");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let (mut inside, port) = listening(&wt).await;
        let (mut other, other_port) = listening(&outside).await;
        let pid = inside.id().unwrap();

        let found = scan(&wts);
        assert_eq!(found.len(), 1, "{found:?}");
        let s = &found[0];
        assert_eq!((s.thread, s.pid, s.ports.clone()), (thread, pid, vec![port]));
        assert!(s.command.starts_with("python3 -c"), "{}", s.command);
        assert!(s.started_at.is_some());
        assert!(!found.iter().any(|s| s.ports.contains(&other_port)));

        // The wrong thread, or a stale start time, stops nothing.
        let wrong = DevServerStop { thread: Uuid::new_v4(), pid, started_at: s.started_at };
        assert!(stop(&wts, &wrong).await.is_err());
        let stale = DevServerStop { thread, pid, started_at: s.started_at.map(|t| t - chrono::Duration::hours(1)) };
        assert_eq!(stop(&wts, &stale).await.unwrap(), Stopped::Gone);
        // Nor does pointing at the one outside.
        let theirs = DevServerStop { thread, pid: other.id().unwrap(), started_at: None };
        assert!(stop(&wts, &theirs).await.is_err());
        assert!(alive(pid) && alive(other.id().unwrap()));

        let req = DevServerStop { thread, pid, started_at: s.started_at };
        assert_eq!(stop(&wts, &req).await.unwrap(), Stopped::Term);
        assert!(inside.wait().await.is_ok());
        assert!(scan(&wts).is_empty());
        assert!(other.try_wait().unwrap().is_none(), "the server outside the worktree keeps running");
        other.kill().await.unwrap();
    }
}
