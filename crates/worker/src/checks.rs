//! The check gate: after an agent's turn changes files in its worktree, run the
//! repo's own checks there (tests, lint, build) and attach the results to the run,
//! so a branch is only "ready" when they pass.
//!
//! The checks are listed in `.sidefleet.json` at the repo root:
//!
//! ```json
//! { "checks": [
//!     { "name": "tests", "run": "cargo test --workspace" },
//!     { "name": "lint",  "run": "cargo clippy --workspace -- -D warnings", "timeout_minutes": 20 }
//! ] }
//! ```
//!
//! The list is read from the checkout's committed history (`origin/HEAD`, else
//! `HEAD`), never from the agent's worktree, so an agent cannot change which
//! commands the worker runs by editing the file.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidefleet_core::{redact, NewEvent};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

pub const CONFIG_FILE: &str = ".sidefleet.json";
const DEFAULT_TIMEOUT_MINUTES: u64 = 15;
const MAX_TIMEOUT_MINUTES: u64 = 120;
const MAX_CHECKS: usize = 20;
/// How much of a check's output is kept: its last lines, then its last bytes.
const TAIL_LINES: usize = 200;
const TAIL_BYTES: usize = 16 * 1024;
const LINE_BYTES: usize = 2048;
/// Where the committed list is looked for, in order.
const CONFIG_REVS: &[&str] = &["refs/remotes/origin/HEAD", "HEAD"];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    /// A shell command, run with `sh -c` in the worktree.
    pub run: String,
    #[serde(default)]
    pub timeout_minutes: Option<u64>,
}

impl Check {
    fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_minutes.unwrap_or(DEFAULT_TIMEOUT_MINUTES) * 60)
    }
}

pub fn parse(text: &str) -> Result<Config, String> {
    let config: Config = serde_json::from_str(text).map_err(|e| format!("{CONFIG_FILE} is not valid: {e}"))?;
    if config.checks.len() > MAX_CHECKS {
        return Err(format!("{CONFIG_FILE} lists {} checks; at most {MAX_CHECKS} are run", config.checks.len()));
    }
    let mut names = std::collections::HashSet::new();
    for c in &config.checks {
        if c.name.trim().is_empty() || c.run.trim().is_empty() {
            return Err(format!("every check in {CONFIG_FILE} needs a name and a command to run"));
        }
        if !names.insert(c.name.trim()) {
            return Err(format!("{CONFIG_FILE} has two checks named {:?}", c.name.trim()));
        }
        if let Some(m) = c.timeout_minutes {
            if m == 0 || m > MAX_TIMEOUT_MINUTES {
                return Err(format!("check {:?}: timeout_minutes must be between 1 and {MAX_TIMEOUT_MINUTES}", c.name.trim()));
            }
        }
    }
    Ok(config)
}

/// The repo's committed check list: None when it has none, an error when it can't be used.
pub async fn load(repo: &Path) -> Option<Result<Config, String>> {
    for rev in CONFIG_REVS {
        let spec = format!("{rev}:{CONFIG_FILE}");
        if let Ok(text) = crate::checkpoint::git(repo, &["show", &spec], Some(Duration::from_secs(10))).await {
            return Some(parse(&text));
        }
    }
    None
}

/// Whether the gate may run commands for a run with this permission mode. Read-only runs
/// change nothing, and "Ask me" runs expect every command to be approved first: the
/// checks would run the agent's code without asking.
pub fn allowed_for(permission_mode: &str) -> Result<(), &'static str> {
    match permission_mode {
        "acceptEdits" | "bypassPermissions" => Ok(()),
        "plan" => Err("this task only reads, so there is nothing to check"),
        _ => Err("this task asks before running commands, and the checks would run its code without asking"),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CheckResult {
    pub name: String,
    pub run: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// The end of what it printed (stdout and stderr together), redacted.
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Every check passed.
    Passed,
    /// A check failed (after the agent's retry, if it had one).
    Failed,
    /// The checks were not run: see the reason.
    Skipped,
    /// The check list could not be used.
    Error,
}

/// The `checks` event the page shows. `retrying`: the agent is asked to fix a failure next.
pub fn event(attempt: u32, verdict: Verdict, results: &[CheckResult], retrying: bool, reason: Option<&str>) -> NewEvent {
    NewEvent {
        kind: "checks".into(),
        payload: json!({
            "verdict": verdict,
            "attempt": attempt,
            "retrying": retrying,
            "reason": reason,
            "config": CONFIG_FILE,
            "checks": results,
        }),
    }
}

/// Sent before the checks start, so the page can say they are running.
pub fn running_event(attempt: u32, names: &[&str]) -> NewEvent {
    NewEvent { kind: "checks".into(), payload: json!({"verdict": "running", "attempt": attempt, "names": names, "config": CONFIG_FILE}) }
}

/// What goes in the run's result, for the run list.
pub fn summary(verdict: Verdict, attempts: u32, results: &[CheckResult]) -> Value {
    let failed: Vec<&str> = results.iter().filter(|r| !r.passed).map(|r| r.name.as_str()).collect();
    json!({ "verdict": verdict, "attempts": attempts, "passed": results.iter().filter(|r| r.passed).count(), "total": results.len(), "failed": failed })
}

/// The follow-up the agent gets when a check fails.
pub fn retry_task(failed: &CheckResult) -> String {
    let how = if failed.timed_out {
        "did not finish in time".to_string()
    } else {
        match failed.exit_code {
            Some(c) => format!("exited with code {c}"),
            None => "was killed".to_string(),
        }
    };
    format!(
        "Sidefleet ran this repo's checks (from {CONFIG_FILE}) on your changes, and the \"{}\" check failed: `{}` {how}. \
         The end of its output:\n\n```\n{}\n```\n\nFix the cause so the check passes. Don't skip, weaken or delete checks or tests to get there. \
         When you're done, say briefly what was wrong.",
        failed.name,
        failed.run,
        failed.output.trim_end()
    )
}

/// Run the checks in order in `cwd`, stopping at the first failure. None when the run was
/// stopped meanwhile.
pub async fn run_all(cwd: &Path, checks: &[Check], stop: &mut watch::Receiver<bool>) -> Option<Vec<CheckResult>> {
    let mut results = Vec::new();
    for check in checks {
        if *stop.borrow() {
            return None;
        }
        let r = run_one(cwd, check, check.timeout(), stop).await?;
        let passed = r.passed;
        results.push(r);
        if !passed {
            break;
        }
    }
    Some(results)
}

async fn run_one(cwd: &Path, check: &Check, timeout: Duration, stop: &mut watch::Receiver<bool>) -> Option<CheckResult> {
    let started = Instant::now();
    let result = |passed, exit_code, timed_out, output: String| CheckResult {
        name: check.name.trim().to_string(),
        run: check.run.clone(),
        passed,
        exit_code,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
        output,
    };
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(&check.run)
        .current_dir(cwd)
        .env("CI", "true")
        .env("SIDEFLEET_CHECK", check.name.trim())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    for k in crate::detect::STRIPPED_ENV {
        cmd.env_remove(k);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Some(result(false, None, false, format!("could not start the check: {e}"))),
    };
    let pid = child.id().unwrap_or(0) as i32;
    crate::agent::track(pid);
    let mut out = BufReader::new(child.stdout.take().expect("stdout piped")).lines();
    let mut err = BufReader::new(child.stderr.take().expect("stderr piped")).lines();
    let mut tail = Tail::default();
    let (mut out_done, mut err_done) = (false, false);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    enum End {
        Exited(Option<i32>),
        TimedOut,
        Stopped,
    }
    let end = loop {
        tokio::select! {
            biased;
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break End::Stopped;
                }
            }
            _ = &mut deadline => break End::TimedOut,
            line = out.next_line(), if !out_done => match line {
                Ok(Some(l)) => tail.push(&l),
                _ => out_done = true,
            },
            line = err.next_line(), if !err_done => match line {
                Ok(Some(l)) => tail.push(&l),
                _ => err_done = true,
            },
            status = child.wait(), if out_done && err_done => {
                break End::Exited(status.ok().and_then(|s| s.code()));
            }
        }
    };
    if !matches!(end, End::Exited(_)) {
        kill_group(pid).await;
        let _ = child.wait().await;
    }
    crate::agent::untrack(pid);
    match end {
        End::Stopped => None,
        End::TimedOut => {
            tail.push(&format!("[sidefleet] stopped the check: it ran longer than {}", human(timeout)));
            Some(result(false, None, true, tail.text()))
        }
        End::Exited(code) => Some(result(code == Some(0), code, false, tail.text())),
    }
}

fn human(d: Duration) -> String {
    let mins = d.as_secs() / 60;
    match mins {
        0 => format!("{} ms", d.as_millis()),
        1 => "1 minute".into(),
        m => format!("{m} minutes"),
    }
}

async fn kill_group(pid: i32) {
    if pid <= 0 {
        return;
    }
    let pg = Pid::from_raw(pid);
    let _ = killpg(pg, Signal::SIGTERM);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    if killpg(pg, None).is_ok() {
        let _ = killpg(pg, Signal::SIGKILL);
    }
}

/// The last lines a check printed, redacted and bounded.
#[derive(Default)]
struct Tail {
    lines: VecDeque<String>,
}

impl Tail {
    fn push(&mut self, line: &str) {
        if self.lines.len() == TAIL_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(sidefleet_core::stream::truncate_bytes(&redact::redact(line), LINE_BYTES));
    }

    fn text(&self) -> String {
        let all = self.lines.iter().cloned().collect::<Vec<_>>().join("\n");
        if all.len() <= TAIL_BYTES {
            return all;
        }
        let mut cut = all.len() - TAIL_BYTES;
        while !all.is_char_boundary(cut) {
            cut += 1;
        }
        all[cut..].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, run: &str) -> Check {
        Check { name: name.into(), run: run.into(), timeout_minutes: None }
    }

    #[test]
    fn parses_a_check_list() {
        let c = parse(r#"{"checks":[{"name":"tests","run":"cargo test"},{"name":"lint","run":"cargo clippy","timeout_minutes":30}]}"#).unwrap();
        assert_eq!(c.checks.len(), 2);
        assert_eq!(c.checks[1].timeout(), Duration::from_secs(30 * 60));
        assert_eq!(c.checks[0].timeout(), Duration::from_secs(DEFAULT_TIMEOUT_MINUTES * 60));
        assert!(parse("{}").unwrap().checks.is_empty());
    }

    #[test]
    fn rejects_lists_it_cannot_use() {
        for bad in [
            "not json",
            r#"{"checks":[{"name":"","run":"x"}]}"#,
            r#"{"checks":[{"name":"a","run":"  "}]}"#,
            r#"{"checks":[{"name":"a","run":"x"},{"name":"a","run":"y"}]}"#,
            r#"{"checks":[{"name":"a","run":"x","timeout_minutes":0}]}"#,
            r#"{"checks":[{"name":"a","run":"x","timeout_minutes":500}]}"#,
            r#"{"checks":[{"name":"a","command":"x"}]}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_runs_for_tasks_that_may_already_run_things() {
        assert!(allowed_for("acceptEdits").is_ok());
        assert!(allowed_for("bypassPermissions").is_ok());
        assert!(allowed_for("plan").is_err());
        assert!(allowed_for(sidefleet_core::ASK_MODE).is_err());
        assert!(allowed_for("default").is_err());
    }

    #[tokio::test]
    async fn stops_at_the_first_failure_and_keeps_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let (_tx, mut rx) = watch::channel(false);
        let checks = [check("ok", "echo fine"), check("broken", "echo out; echo boom >&2; exit 3"), check("never", "echo no")];
        let r = run_all(dir.path(), &checks, &mut rx).await.unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].passed && r[0].output == "fine");
        assert!(!r[1].passed);
        assert_eq!(r[1].exit_code, Some(3));
        assert!(r[1].output.contains("out") && r[1].output.contains("boom"));
        assert_eq!(summary(Verdict::Failed, 2, &r)["failed"], json!(["broken"]));
        let task = retry_task(&r[1]);
        assert!(task.contains("\"broken\"") && task.contains("exited with code 3") && task.contains("boom"));
    }

    #[tokio::test]
    async fn runs_in_the_worktree_and_says_it_is_a_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), "here").unwrap();
        let (_tx, mut rx) = watch::channel(false);
        let r = run_all(dir.path(), &[check("env", "cat marker; echo \" $CI $SIDEFLEET_CHECK\"")], &mut rx).await.unwrap();
        assert_eq!(r[0].output, "here true env");
    }

    #[tokio::test]
    async fn a_slow_check_times_out_and_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let (_tx, mut rx) = watch::channel(false);
        let started = Instant::now();
        let r = run_one(dir.path(), &check("slow", "echo begun; sleep 30"), Duration::from_millis(300), &mut rx).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!r.passed && r.timed_out && r.exit_code.is_none());
        assert!(r.output.starts_with("begun\n[sidefleet] stopped the check"), "{}", r.output);
        assert!(retry_task(&r).contains("did not finish in time"));
    }

    #[tokio::test]
    async fn a_stop_kills_the_running_check() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = watch::channel(false);
        let started = Instant::now();
        let stopper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = tx.send(true);
            tx
        });
        assert!(run_all(dir.path(), &[check("slow", "sleep 30")], &mut rx).await.is_none());
        assert!(started.elapsed() < Duration::from_secs(10));
        drop(stopper.await);
    }

    #[test]
    fn keeps_only_the_end_of_long_output() {
        let mut t = Tail::default();
        for i in 0..(TAIL_LINES + 50) {
            t.push(&format!("line {i}"));
        }
        let text = t.text();
        assert!(text.starts_with("line 50\n"));
        assert!(text.ends_with(&format!("line {}", TAIL_LINES + 49)));
        let mut big = Tail::default();
        for _ in 0..TAIL_LINES {
            big.push(&"é".repeat(1000));
        }
        assert!(big.text().len() <= TAIL_BYTES);
    }
}
