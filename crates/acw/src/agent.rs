//! Spawns the agent CLI in its own process group, maps its stdout lines to
//! events, and kills the whole group on stop. Testable with any executable
//! that prints stream-json lines (see tests/fixtures/fake_claude.sh).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use ac_core::{map_stream_line, redact, MappedLine, NewEvent, RunStatus};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone)]
pub struct AgentSpec {
    /// Executable, e.g. `claude` or an absolute path.
    pub bin: String,
    pub task: String,
    pub model: String,
    pub cwd: PathBuf,
    pub permission_mode: String,
    pub max_turns: Option<u32>,
    pub extra_args: Vec<String>,
}

impl AgentSpec {
    pub fn args(&self) -> Vec<String> {
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
        if let Some(n) = self.max_turns {
            a.push("--max-turns".into());
            a.push(n.to_string());
        }
        a.extend(self.extra_args.iter().cloned());
        a
    }

    /// Redacted command line for the `started` event and logs (task omitted).
    pub fn display_command(&self) -> String {
        let shown: Vec<String> = self
            .args()
            .iter()
            .map(|a| if a == &self.task { "<task>".to_string() } else { a.clone() })
            .collect();
        redact(&format!("{} {}", self.bin, shown.join(" ")))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub status: RunStatus,
    pub result: Value,
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
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
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
            return Outcome { status: RunStatus::Failed, result: json!({"error": msg}) };
        }
    };
    let pid = child.id().unwrap_or(0) as i32;
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
                    Ok(Some(line)) => match map_stream_line(&line) {
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
                        let text = ac_core::stream::truncate_bytes(&redact(&line), 2048);
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

    if stopped {
        return Outcome {
            status: RunStatus::Stopped,
            result: json!({"reason": "stopped by request", "partial_result": final_result}),
        };
    }
    match (exit_status, final_result) {
        (Ok(st), Some(res)) if st.success() => {
            let is_error = res.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            if is_error {
                Outcome { status: RunStatus::Failed, result: json!({"error": "agent reported an error", "result": res}) }
            } else {
                Outcome { status: RunStatus::Succeeded, result: res }
            }
        }
        (Ok(st), res) => {
            let msg = format!("agent exited with {st}");
            send(NewEvent { kind: "error".into(), payload: json!({"message": msg}) }).await;
            Outcome {
                status: RunStatus::Failed,
                result: json!({"error": msg, "stderr": stderr_tail.join("\n"), "result": res}),
            }
        }
        (Err(e), _) => {
            let msg = format!("could not wait for agent: {e}");
            Outcome { status: RunStatus::Failed, result: json!({"error": msg}) }
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
