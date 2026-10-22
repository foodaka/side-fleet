//! Spawns the agent CLI in its own process group, maps its stdout lines to
//! events, and kills the whole group on stop. Testable with any executable
//! that prints stream-json lines (see tests/fixtures/fake_claude.sh).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use sidefleet_core::{redact, result_usage_limit, Agent, AgentStream, MappedLine, NewEvent, RunStatus, UsageLimit};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

/// Process groups of agents currently running, so a shutting-down worker can
/// kill them instead of leaving orphans that keep consuming the user's plan.
static LIVE_GROUPS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

fn track(pid: i32) {
    LIVE_GROUPS.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
}
fn untrack(pid: i32) {
    LIVE_GROUPS.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| *p != pid);
}

/// Kill every agent this process started. Called on Ctrl-C / SIGTERM.
pub fn kill_all_agents() -> usize {
    let pids: Vec<i32> = LIVE_GROUPS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for pid in &pids {
        let _ = killpg(Pid::from_raw(*pid), Signal::SIGTERM);
    }
    if !pids.is_empty() {
        std::thread::sleep(Duration::from_millis(800));
        for pid in &pids {
            let _ = killpg(Pid::from_raw(*pid), Signal::SIGKILL);
        }
    }
    pids.len()
}

#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub agent: Agent,
    /// Executable, e.g. `claude` or an absolute path.
    pub bin: String,
    pub task: String,
    /// Empty means the agent's own default (Codex, opencode).
    pub model: String,
    pub cwd: PathBuf,
    /// Claude Code's permission mode names; mapped onto each agent's own controls.
    pub permission_mode: String,
    /// Claude Code only.
    pub max_turns: Option<u32>,
    /// Appended to the Claude Code command line (`--claude-arg`).
    pub extra_args: Vec<String>,
    /// Continue an earlier session of the same agent on this machine.
    pub resume_session_id: Option<String>,
}

impl AgentSpec {
    pub fn args(&self) -> Vec<String> {
        match self.agent {
            Agent::Claude => self.claude_args(),
            Agent::Codex => self.codex_args(),
            Agent::Opencode => self.opencode_args(),
        }
    }

    fn claude_args(&self) -> Vec<String> {
        let mut a = vec![
            "-p".to_string(),
            self.task.clone(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--model".into(),
            self.model.clone(),
            "--permission-mode".into(),
            self.permission_mode.clone(),
        ];
        if let Some(sid) = &self.resume_session_id {
            a.push("--resume".into());
            a.push(sid.clone());
        }
        if let Some(n) = self.max_turns {
            a.push("--max-turns".into());
            a.push(n.to_string());
        }
        a.extend(self.extra_args.iter().cloned());
        a
    }

    /// `codex exec [resume] --json ... -- [<session>] <task>`. Permissions map onto
    /// Codex's sandbox: read only, writes inside the workspace, or no sandbox.
    fn codex_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["exec".into()];
        if self.resume_session_id.is_some() {
            a.push("resume".into());
        }
        a.extend(["--json".into(), "--skip-git-repo-check".into()]);
        match self.permission_mode.as_str() {
            "bypassPermissions" => a.push("--dangerously-bypass-approvals-and-sandbox".into()),
            "plan" => a.extend(["-c".into(), "sandbox_mode=\"read-only\"".into()]),
            _ => a.extend(["-c".into(), "sandbox_mode=\"workspace-write\"".into()]),
        }
        if !self.model.is_empty() {
            a.extend(["-m".into(), self.model.clone()]);
        }
        // `--` so a task starting with `-` is never read as a flag.
        a.push("--".into());
        if let Some(sid) = &self.resume_session_id {
            a.push(sid.clone());
        }
        a.push(self.task.clone());
        a
    }

    /// `opencode run --format json ...` with the task on stdin: opencode quotes
    /// argv words that contain spaces before sending them to the model.
    fn opencode_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["run".into(), "--format".into(), "json".into()];
        if !self.model.is_empty() {
            a.extend(["--model".into(), self.model.clone()]);
        }
        if let Some(sid) = &self.resume_session_id {
            a.extend(["--session".into(), sid.clone()]);
        }
        match self.permission_mode.as_str() {
            "plan" => a.extend(["--agent".into(), "plan".into()]),
            // `--auto` is newer (1.18); this alias works from 1.4.
            "bypassPermissions" => a.push("--dangerously-skip-permissions".into()),
            _ => {}
        }
        a
    }

    /// Environment for the agent process, on top of the worker's own.
    pub fn env(&self) -> Vec<(&'static str, String)> {
        match self.agent {
            Agent::Opencode => {
                // Merged into opencode's permission config. Its default allows
                // everything, so "Read & edit files" denies the shell like Claude
                // Code's acceptEdits, and "Read only" denies everything that writes.
                let perms = match self.permission_mode.as_str() {
                    "plan" => Some(r#"{"edit":"deny","bash":"deny","webfetch":"deny"}"#),
                    "acceptEdits" => Some(r#"{"bash":"deny"}"#),
                    _ => None,
                };
                let mut env = vec![("OPENCODE_DISABLE_AUTOUPDATE", "1".to_string())];
                if let Some(p) = perms {
                    env.push(("OPENCODE_PERMISSION", p.to_string()));
                }
                env
            }
            Agent::Claude | Agent::Codex => vec![],
        }
    }

    /// Text written to the agent's stdin (then closed); None means no stdin.
    pub fn stdin(&self) -> Option<&str> {
        matches!(self.agent, Agent::Opencode).then_some(self.task.as_str())
    }

    /// Redacted command line for the `started` event and logs (task omitted).
    pub fn display_command(&self) -> String {
        let shown: Vec<String> = self
            .args()
            .iter()
            .map(|a| if a == &self.task { "<task>".to_string() } else { a.clone() })
            .collect();
        let stdin = if self.stdin().is_some() { " < <task>" } else { "" };
        redact(&format!("{} {}{stdin}", self.bin, shown.join(" ")))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub status: RunStatus,
    pub result: Value,
    /// The run failed on the account's usage limit (reported to the control plane).
    pub usage_limit: Option<UsageLimit>,
}

impl Outcome {
    pub fn new(status: RunStatus, result: Value) -> Self {
        Outcome { status, result, usage_limit: None }
    }
}

/// Run the agent to completion (or until `stop` becomes true). Events are
/// sent to `sink` as they happen. Returns the final status and result.
pub async fn execute(
    spec: AgentSpec,
    sink: mpsc::Sender<NewEvent>,
    mut stop: watch::Receiver<bool>,
) -> Outcome {
    let send = |ev: NewEvent| {
        let sink = sink.clone();
        async move {
            let _ = sink.send(ev).await;
        }
    };

    let mut cmd = Command::new(&spec.bin);
    cmd.args(spec.args())
        .envs(spec.env())
        .current_dir(&spec.cwd)
        .stdin(if spec.stdin().is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0) // new process group so we can kill children too
        .kill_on_drop(true);
    for k in crate::detect::STRIPPED_ENV {
        cmd.env_remove(k);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("failed to start {}: {e}", spec.bin);
            send(NewEvent { kind: "error".into(), payload: json!({"message": msg}) }).await;
            return Outcome::new(RunStatus::Failed, json!({"error": msg}));
        }
    };
    let pid = child.id().unwrap_or(0) as i32;
    track(pid);
    if let (Some(text), Some(mut stdin)) = (spec.stdin().map(str::to_string), child.stdin.take()) {
        // Written from a task so a large task cannot block reading stdout; dropping closes it.
        tokio::spawn(async move {
            let _ = stdin.write_all(text.as_bytes()).await;
        });
    }
    let mut stream = AgentStream::new(spec.agent, &spec.model, &spec.cwd.to_string_lossy());
    send(NewEvent {
        kind: "started".into(),
        payload: json!({"command": spec.display_command(), "cwd": spec.cwd.to_string_lossy(), "pid": pid}),
    })
    .await;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let mut out_lines = BufReader::new(stdout).lines();
    let mut err_lines = BufReader::new(stderr).lines();

    let mut final_result: Option<Value> = None;
    let mut stderr_tail: Vec<String> = Vec::new();
    let mut stopped = false;
    let mut stdout_done = false;
    let mut stderr_done = false;

    let exit_status = loop {
        tokio::select! {
            biased;
            changed = stop.changed(), if !stopped => {
                if changed.is_ok() && *stop.borrow() {
                    stopped = true;
                    kill_group(pid, &send).await;
                }
            }
            line = out_lines.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(line)) => match stream.map_line(&line) {
                        MappedLine::Events(evs) => {
                            for ev in evs {
                                if ev.kind == "result" {
                                    final_result = Some(ev.payload.clone());
                                }
                                send(ev).await;
                            }
                        }
                        MappedLine::Raw(text) => {
                            send(NewEvent { kind: "stderr".into(), payload: json!({"text": text, "stream": "stdout"}) }).await;
                        }
                    },
                    _ => stdout_done = true,
                }
            }
            line = err_lines.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(line)) => {
                        let text = sidefleet_core::stream::truncate_bytes(&redact(&line), 2048);
                        if stderr_tail.len() < 50 {
                            stderr_tail.push(text.clone());
                        }
                        send(NewEvent { kind: "stderr".into(), payload: json!({"text": text, "stream": "stderr"}) }).await;
                    }
                    _ => stderr_done = true,
                }
            }
            status = child.wait(), if stdout_done && stderr_done => {
                break status;
            }
        }
    };

    untrack(pid);
    // Agents without a final result line (opencode) get one from what they printed.
    if !stopped {
        let ok = exit_status.as_ref().map(|s| s.success()).unwrap_or(false);
        for ev in stream.finish(ok) {
            final_result = Some(ev.payload.clone());
            send(ev).await;
        }
    }
    if stopped {
        return Outcome::new(RunStatus::Stopped, json!({"reason": "stopped by request", "partial_result": final_result}));
    }
    let usage_limit = final_result.as_ref().and_then(result_usage_limit);
    if let Some(l) = &usage_limit {
        tracing::warn!(agent = %spec.agent, resets_at = ?l.resets_at, "the agent hit this machine's usage limit");
    }
    let mut outcome = outcome_of(exit_status, final_result, &stderr_tail, &send).await;
    if outcome.status == RunStatus::Failed {
        outcome.usage_limit = usage_limit;
    }
    outcome
}

async fn outcome_of<F, Fut>(
    exit_status: std::io::Result<std::process::ExitStatus>,
    final_result: Option<Value>,
    stderr_tail: &[String],
    send: &F,
) -> Outcome
where
    F: Fn(NewEvent) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    match (exit_status, final_result) {
        (Ok(st), Some(res)) if st.success() => {
            let is_error = res.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            if is_error {
                Outcome::new(RunStatus::Failed, json!({"error": "agent reported an error", "result": res}))
            } else {
                Outcome::new(RunStatus::Succeeded, res)
            }
        }
        (Ok(st), res) => {
            let msg = format!("agent exited with {st}");
            send(NewEvent { kind: "error".into(), payload: json!({"message": msg}) }).await;
            Outcome::new(RunStatus::Failed, json!({"error": msg, "stderr": stderr_tail.join("\n"), "result": res}))
        }
        (Err(e), _) => {
            let msg = format!("could not wait for agent: {e}");
            Outcome::new(RunStatus::Failed, json!({"error": msg}))
        }
    }
}

async fn kill_group<F, Fut>(pid: i32, send: &F)
where
    F: Fn(NewEvent) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if pid <= 0 {
        return;
    }
    let pg = Pid::from_raw(pid);
    let _ = killpg(pg, Signal::SIGTERM);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // Escalate if anything in the group is still alive.
    let still_alive = nix::sys::signal::killpg(pg, None).is_ok();
    let signal = if still_alive {
        let _ = killpg(pg, Signal::SIGKILL);
        "SIGKILL"
    } else {
        "SIGTERM"
    };
    send(NewEvent { kind: "stopped".into(), payload: json!({"signal": signal, "pid": pid}) }).await;
}

/// True when any process in the group is still alive.
pub fn process_group_alive(pid: i32) -> bool {
    nix::sys::signal::killpg(Pid::from_raw(pid), None).is_ok()
}
