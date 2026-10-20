//! Executor tests against a fake agent script that prints stream-json lines slowly.

use std::path::PathBuf;
use std::time::Duration;

use sidefleet_core::{NewEvent, RunStatus};
use sidefleet_worker::agent::{execute, process_group_alive, AgentSpec};
use tokio::sync::{mpsc, watch};

fn fake_bin() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fake_claude.sh")
        .to_string_lossy()
        .to_string()
}

fn spec(task: &str) -> AgentSpec {
    AgentSpec {
        bin: fake_bin(),
        task: task.into(),
        model: "claude-sonnet-5-5".into(),
        cwd: std::env::temp_dir(),
        permission_mode: "acceptEdits".into(),
        max_turns: None,
        extra_args: vec![],
    }
}

async fn drain(mut rx: mpsc::Receiver<NewEvent>) -> Vec<NewEvent> {
    let mut out = Vec::new();
    while let Some(e) = rx.recv().await {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn success_path_maps_events_in_order_and_redacts() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let (outcome, events) = tokio::join!(execute(spec("summarise"), tx, stop_rx), drain(rx));

    assert_eq!(outcome.status, RunStatus::Succeeded);
    assert!(outcome.result["result"].as_str().unwrap().starts_with("Done: one untracked file"));
    assert_eq!(outcome.result["total_cost_usd"], 0.0042);

    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&"stderr"), "{kinds:?}");
    let ordered: Vec<&str> = kinds.iter().copied().filter(|k| *k != "stderr").collect();
    assert_eq!(
        ordered,
        vec!["started", "agent_init", "assistant_text", "tool_use", "tool_result", "assistant_text", "result"]
    );
    let events: Vec<&NewEvent> = events.iter().filter(|e| e.kind != "stderr").collect();
    let all = serde_json::to_string(&events).unwrap();
    assert!(!all.contains("ghp_abc"), "tool input token leaked: {all}");
    assert!(!all.contains("supersecretvalue99"), "tool result secret leaked: {all}");
    assert!(!all.contains("abcdefghijklmnop"), "stderr token leaked: {all}");
    assert!(all.contains("git status --short"));
    assert_eq!(events[3].payload["summary"], "export GITHUB_TOKEN=[REDACTED] && git status --short");
    assert_eq!(events[1].payload["model"], "claude-sonnet-5-5");
    assert!(events[0].payload["command"].as_str().unwrap().contains("--output-format stream-json --verbose --model claude-sonnet-5-5 --permission-mode acceptEdits"));
    assert!(!events[0].payload["command"].as_str().unwrap().contains("summarise"), "task must not be echoed in the command");
}

#[tokio::test]
async fn stop_kills_the_whole_process_group() {
    let (tx, mut rx) = mpsc::channel(64);
    let (stop_tx, stop_rx) = watch::channel(false);
    let exec = tokio::spawn(execute(spec("slow task"), tx, stop_rx));

    // Wait until the agent is clearly running (it has emitted its tool call).
    let mut pid = 0;
    let mut seen = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event").expect("open");
        if ev.kind == "started" {
            pid = ev.payload["pid"].as_i64().unwrap() as i32;
        }
        seen.push(ev.kind.clone());
        if seen.contains(&"tool_result".to_string()) {
            break;
        }
    }
    assert!(pid > 0);
    assert!(process_group_alive(pid), "agent group should be alive before stop");

    let t0 = std::time::Instant::now();
    stop_tx.send(true).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), exec).await.expect("stops promptly").unwrap();
    assert_eq!(outcome.status, RunStatus::Stopped);
    assert!(t0.elapsed() < Duration::from_secs(5), "took {:?}", t0.elapsed());

    let rest = drain(rx).await;
    assert!(rest.iter().any(|e| e.kind == "stopped"), "{rest:?}");
    // The script's background `sleep 300` child must be gone too.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!process_group_alive(pid), "process group still alive after stop");
}

#[tokio::test]
async fn non_zero_exit_without_result_is_failed() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let (outcome, events) = tokio::join!(execute(spec("please fail"), tx, stop_rx), drain(rx));
    assert_eq!(outcome.status, RunStatus::Failed);
    assert!(outcome.result["error"].as_str().unwrap().contains("exited with"));
    assert!(outcome.result["stderr"].as_str().unwrap().contains("simulated crash"));
    assert!(events.iter().any(|e| e.kind == "error"));
}

#[tokio::test]
async fn agent_error_result_is_failed() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let (outcome, events) = tokio::join!(execute(spec("error-result"), tx, stop_rx), drain(rx));
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.result["result"]["subtype"], "error_max_turns");
    let last_stdout = events.iter().rev().find(|e| e.kind != "stderr").unwrap();
    assert_eq!(last_stdout.kind, "result");
}

#[tokio::test]
async fn missing_binary_is_failed_not_a_panic() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let mut s = spec("x");
    s.bin = "/nonexistent/claude".into();
    let (outcome, events) = tokio::join!(execute(s, tx, stop_rx), drain(rx));
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(events[0].kind, "error");
}
