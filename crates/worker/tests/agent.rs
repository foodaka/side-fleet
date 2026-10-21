//! Executor tests against fake agent scripts (tests/fixtures) that print each
//! agent's output format: Claude Code stream-json, Codex and opencode JSON lines.

use std::path::PathBuf;
use std::time::Duration;

use sidefleet_core::{Agent, NewEvent, RunStatus};
use sidefleet_worker::agent::{execute, process_group_alive, AgentSpec};
use tokio::sync::{mpsc, watch};

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .to_string()
}

fn fake_bin() -> String {
    fixture("fake_claude.sh")
}

fn spec(task: &str) -> AgentSpec {
    AgentSpec {
        agent: Agent::Claude,
        bin: fake_bin(),
        task: task.into(),
        model: "claude-sonnet-5-5".into(),
        cwd: std::env::temp_dir(),
        permission_mode: "acceptEdits".into(),
        max_turns: None,
        extra_args: vec![],
        resume_session_id: None,
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

#[test]
fn resume_adds_the_resume_flag() {
    let mut s = spec("continue");
    assert!(!s.args().iter().any(|a| a == "--resume"));
    s.resume_session_id = Some("sess-123".into());
    let a = s.args();
    let i = a.iter().position(|x| x == "--resume").expect("--resume present");
    assert_eq!(a[i + 1], "sess-123");
    assert!(s.display_command().contains("--resume sess-123"));
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

fn agent_spec(agent: Agent, task: &str) -> AgentSpec {
    let bin = match agent {
        Agent::Claude => fake_bin(),
        Agent::Codex => fixture("fake_codex.sh"),
        Agent::Opencode => fixture("fake_opencode.sh"),
    };
    AgentSpec { agent, bin, model: String::new(), ..spec(task) }
}

fn kinds(events: &[NewEvent]) -> Vec<&str> {
    events.iter().map(|e| e.kind.as_str()).filter(|k| *k != "stderr").collect()
}

#[tokio::test]
async fn codex_runs_map_to_the_same_events() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let (outcome, events) = tokio::join!(execute(agent_spec(Agent::Codex, "--help me fix it"), tx, stop_rx), drain(rx));
    assert_eq!(outcome.status, RunStatus::Succeeded, "{outcome:?}");
    assert_eq!(
        kinds(&events),
        vec!["started", "agent_init", "assistant_thinking", "tool_use", "tool_result", "assistant_text", "result"]
    );
    // A task that looks like a flag still reaches the agent as the prompt.
    assert_eq!(outcome.result["result"], "task=--help me fix it");
    assert_eq!(outcome.result["session_id"], "fake-thread");
    assert_eq!(events[3].payload["name"], "Bash");
    assert_eq!(events[3].payload["summary"], "git status --short");
    let all = serde_json::to_string(&events).unwrap();
    assert!(!all.contains("supersecretvalue99"), "{all}");
}

#[tokio::test]
async fn codex_resume_passes_the_session() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let mut s = agent_spec(Agent::Codex, "and now the tests");
    s.resume_session_id = Some("thread-42".into());
    let (outcome, _) = tokio::join!(execute(s, tx, stop_rx), drain(rx));
    assert_eq!(outcome.status, RunStatus::Succeeded);
    assert_eq!(outcome.result["session_id"], "thread-42");
    assert_eq!(outcome.result["result"], "task=and now the tests");
}

#[tokio::test]
async fn codex_failure_is_failed_with_the_reason() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let (outcome, events) = tokio::join!(execute(agent_spec(Agent::Codex, "please fail"), tx, stop_rx), drain(rx));
    assert_eq!(outcome.status, RunStatus::Failed);
    let r = events.iter().find(|e| e.kind == "result").expect("result event");
    assert_eq!(r.payload["is_error"], true);
    assert_eq!(r.payload["result"], "simulated model error");
}

#[test]
fn codex_args_map_permissions_onto_the_sandbox() {
    let mut s = agent_spec(Agent::Codex, "do it");
    s.model = "gpt-fake".into();
    let a = s.args().join(" ");
    assert!(a.starts_with("exec --json --skip-git-repo-check -c sandbox_mode=\"workspace-write\" -m gpt-fake -- do it"), "{a}");
    s.permission_mode = "plan".into();
    assert!(s.args().join(" ").contains("sandbox_mode=\"read-only\""));
    s.permission_mode = "bypassPermissions".into();
    assert!(s.args().contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    s.resume_session_id = Some("t1".into());
    let a = s.args();
    assert_eq!(&a[..2], &["exec", "resume"]);
    assert_eq!(&a[a.len() - 3..], &["--", "t1", "do it"]);
    assert!(!s.display_command().contains("do it"), "task must not be echoed");
}

#[tokio::test]
async fn opencode_gets_the_task_on_stdin_and_permissions_in_its_env() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let mut s = agent_spec(Agent::Opencode, "fix the \"flaky\" test");
    s.model = "openrouter/moonshotai/kimi-k2".into();
    let (outcome, events) = tokio::join!(execute(s, tx, stop_rx), drain(rx));
    assert_eq!(outcome.status, RunStatus::Succeeded, "{outcome:?}");
    assert_eq!(kinds(&events), vec!["started", "agent_init", "tool_use", "tool_result", "assistant_text", "result"]);
    assert_eq!(outcome.result["result"], r#"task=fix the "flaky" test perm={"bash":"deny"}"#);
    assert_eq!(outcome.result["session_id"], "ses_fakeNew");
    assert_eq!(outcome.result["num_turns"], 2);
    assert_eq!(outcome.result["total_cost_usd"], 0.003);
    let cmd = events[0].payload["command"].as_str().unwrap();
    assert!(cmd.ends_with("run --format json --model openrouter/moonshotai/kimi-k2 < <task>"), "{cmd}");
}

#[tokio::test]
async fn opencode_resume_and_read_only() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let mut s = agent_spec(Agent::Opencode, "what changed?");
    s.resume_session_id = Some("ses_old".into());
    s.permission_mode = "plan".into();
    assert!(s.args().join(" ").contains("--session ses_old --agent plan"));
    let (outcome, _) = tokio::join!(execute(s, tx, stop_rx), drain(rx));
    assert_eq!(outcome.result["session_id"], "ses_old");
    assert!(outcome.result["result"].as_str().unwrap().contains(r#"perm={"edit":"deny","bash":"deny","webfetch":"deny"}"#));
}

#[tokio::test]
async fn opencode_session_error_is_failed() {
    let (tx, rx) = mpsc::channel(64);
    let (_stop_tx, stop_rx) = watch::channel(false);
    let (outcome, events) = tokio::join!(execute(agent_spec(Agent::Opencode, "please fail"), tx, stop_rx), drain(rx));
    assert_eq!(outcome.status, RunStatus::Failed);
    let r = events.iter().find(|e| e.kind == "result").expect("synthesised result");
    assert_eq!(r.payload["is_error"], true);
    assert_eq!(r.payload["result"], "simulated provider error");
}
