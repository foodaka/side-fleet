//! End to end: real worker loop against an in-process control plane and the
//! fake agents (Claude Code and Codex; no opencode). Needs Postgres (docker compose up -d).

use std::time::Duration;

use sidefleet_core::{Run, RunEvent, RunStatus};
use sidefleet_server::test_support::{TestEnv, DEV_KEY, WORKER_TOKEN};
use sidefleet_worker::worker::{run as run_worker, WorkerConfig};
use futures::StreamExt;
use reqwest::Client;
use serde_json::json;
use uuid::Uuid;

fn git_init(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .arg("-C").arg(dir).args(args)
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .output().unwrap();
        assert!(st.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&st.stderr));
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "demo\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
}

fn fixture(name: &str) -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .to_string()
}

fn fake_bin() -> String {
    fixture("fake_claude.sh")
}

struct Harness {
    env: TestEnv,
    http: Client,
    _repos: tempfile::TempDir,
    worker: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Harness {
    async fn start(max_runs: usize) -> Harness {
        let env = TestEnv::start().await;
        let repos = tempfile::tempdir().unwrap();
        // `demo` is a real git repo (worktrees work); `group/other` only looks like one (fallback path).
        std::fs::create_dir_all(repos.path().join("demo")).unwrap();
        git_init(&repos.path().join("demo"));
        std::fs::create_dir_all(repos.path().join("group/other/.git")).unwrap();
        let cfg = WorkerConfig {
            control_url: env.base_url.clone(),
            token: WORKER_TOKEN.into(),
            worker_id: "test-worker".into(),
            display_name: "Test Worker".into(),
            repos_root: repos.path().to_path_buf(),
            claude_bin: fake_bin(),
            codex_bin: fixture("fake_codex.sh"),
            opencode_bin: "/nonexistent/opencode".into(),
            permission_mode: "acceptEdits".into(),
            max_turns: None,
            extra_args: vec![],
            poll_interval: Duration::from_millis(200),
            heartbeat_interval: Duration::from_secs(15),
            check_sign_in: true,
            scan_depth: 2,
            max_runs: Some(max_runs),
            parallel: 2,
            worktrees_dir: repos.path().join(".worktrees"),
            version: "test-build".into(),
            allow_remote_update: false,
            // Never run: the fake agent talks to the worker's socket itself.
            approval_exe: Some("/nonexistent/sidefleet".into()),
            approval_timeout: Duration::from_secs(20),
        };
        let worker = tokio::spawn(run_worker(cfg));
        let h = Harness { env, http: Client::new(), _repos: repos, worker };
        // Wait for registration.
        for _ in 0..50 {
            let ws: Vec<serde_json::Value> = h.dev_get("/v1/workers").await;
            if !ws.is_empty() {
                let caps = &ws[0]["capabilities"];
                assert_eq!(caps["agents"]["claude"]["version"], "9.9.9");
                assert_eq!(caps["agents"]["claude"]["signed_in"], true, "{caps}");
                let names: Vec<&str> = caps["repos"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
                assert_eq!(names, vec!["demo", "group/other"]);
                return h;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("worker never registered");
    }

    async fn dev_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> T {
        self.http.get(format!("{}{path}", self.env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap().json().await.unwrap()
    }

    async fn create(&self, task: &str, repo: &str) -> Uuid {
        self.create_with(task, repo, "claude", "claude-haiku-5-5").await
    }

    async fn create_with(&self, task: &str, repo: &str, agent: &str, model: &str) -> Uuid {
        let r = self
            .http
            .post(format!("{}/v1/runs", self.env.base_url))
            .bearer_auth(DEV_KEY)
            .json(&json!({"task": task, "agent": agent, "model": model, "repo": repo}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 202);
        r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap()
    }

    async fn wait_status(&self, id: Uuid, want: RunStatus, timeout: Duration) -> Run {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let run: Run = self.dev_get(&format!("/v1/runs/{id}")).await;
            if run.status == want {
                return run;
            }
            assert!(tokio::time::Instant::now() < deadline, "run {id} is {:?}, wanted {want:?}", run.status);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Collect SSE events (as RunEvent) until the stream ends or `timeout` passes.
    async fn sse_collect(&self, id: Uuid, timeout: Duration) -> Vec<(RunEvent, std::time::Instant)> {
        let r = self
            .http
            .get(format!("{}/v1/runs/{id}/events", self.env.base_url))
            .bearer_auth(DEV_KEY)
            .send()
            .await
            .unwrap();
        let mut stream = r.bytes_stream();
        let mut buf = String::new();
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let chunk = match tokio::time::timeout(remaining, stream.next()).await {
                Ok(Some(Ok(b))) => b,
                _ => break,
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            let mut ended = false;
            while let Some(idx) = buf.find("\n\n") {
                let frame = buf[..idx].to_string();
                buf = buf[idx + 2..].to_string();
                let mut name = "message";
                let mut data = String::new();
                for line in frame.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        name = v.trim();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                if name == "end" {
                    ended = true;
                } else if let Ok(ev) = serde_json::from_str::<RunEvent>(&data) {
                    out.push((ev, std::time::Instant::now()));
                }
            }
            if ended {
                break;
            }
        }
        out
    }

    async fn teardown(self) {
        self.worker.abort();
        self.env.teardown().await;
    }
}

#[tokio::test]
async fn worker_claims_runs_and_streams_narration_live() {
    let h = Harness::start(2).await;

    // A run for a repo the worker does not have stays queued.
    let other = h.create("ignored", "missing-repo").await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(h.wait_status(other, RunStatus::Queued, Duration::from_secs(1)).await.status, RunStatus::Queued);

    let id = h.create("summarise the repo", "demo").await;
    let collector = tokio::spawn({
        let http = h.http.clone();
        let base = h.env.base_url.clone();
        async move {
            // Same logic as sse_collect but owned, so it runs concurrently with the run.
            let r = http.get(format!("{base}/v1/runs/{id}/events")).bearer_auth(DEV_KEY).send().await.unwrap();
            let mut stream = r.bytes_stream();
            let mut buf = String::new();
            let mut out: Vec<(RunEvent, std::time::Instant)> = Vec::new();
            let mut live: Vec<String> = Vec::new();
            'outer: while let Ok(Some(Ok(b))) = tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
                buf.push_str(&String::from_utf8_lossy(&b));
                while let Some(idx) = buf.find("\n\n") {
                    let frame = buf[..idx].to_string();
                    buf = buf[idx + 2..].to_string();
                    if frame.lines().any(|l| l.trim() == "event: end" || l.trim() == "event:end") {
                        break 'outer;
                    }
                    let is_live = frame.lines().any(|l| l.strip_prefix("event:").is_some_and(|n| n.trim() == "assistant_live"));
                    for line in frame.lines() {
                        if let Some(d) = line.strip_prefix("data:") {
                            if is_live {
                                let v: serde_json::Value = serde_json::from_str(d.trim()).unwrap();
                                live.push(v["payload"]["text"].as_str().unwrap().to_string());
                            } else if let Ok(ev) = serde_json::from_str::<RunEvent>(d.trim()) {
                                out.push((ev, std::time::Instant::now()));
                            }
                        }
                    }
                }
            }
            (out, live)
        }
    });

    let run = h.wait_status(id, RunStatus::Succeeded, Duration::from_secs(15)).await;
    assert_eq!(run.worker_id.as_deref(), Some("test-worker"));
    assert!(run.started_at.is_some() && run.finished_at.is_some());
    assert!(run.result.unwrap()["result"].as_str().unwrap().contains("Model was claude-haiku-5-5"));

    let (events, live) = collector.await.unwrap();
    // The last message streamed to the page as it was written, ahead of the stored one.
    let last = "The working tree has one untracked file.";
    assert!(live.iter().any(|t| t.len() > "The working ".len()), "{live:?}");
    assert!(live.iter().all(|t| last.starts_with(t.as_str())), "{live:?}");
    let kinds: Vec<&str> = events.iter().map(|(e, _)| e.kind.as_str()).collect();
    assert!(kinds.contains(&"stderr"), "{kinds:?}");
    // stderr interleaving relative to stdout is inherently unordered; check the stdout-derived order.
    let ordered: Vec<&str> = kinds.iter().copied().filter(|k| *k != "stderr").collect();
    assert_eq!(
        ordered,
        vec![
            "status", "status", "workspace", "started", "agent_init", "assistant_text", "tool_use", "tool_result",
            "assistant_text", "result", "snapshot", "status"
        ],
        "{kinds:?}"
    );
    // The task ran in its own worktree on its own branch, not in the checkout.
    let ws = &events.iter().find(|(e, _)| e.kind == "workspace").unwrap().0.payload;
    assert_eq!(ws["mode"], "worktree");
    let branch = ws["branch"].as_str().unwrap();
    assert!(branch.starts_with("sidefleet/"), "{ws}");
    let wt_path = std::path::PathBuf::from(ws["path"].as_str().unwrap());
    assert!(wt_path.join(".git").exists(), "worktree exists at {}", wt_path.display());
    assert!(wt_path.starts_with(h._repos.path().join(".worktrees")));
    let started = &events.iter().find(|(e, _)| e.kind == "started").unwrap().0.payload;
    assert_eq!(started["cwd"], ws["path"], "agent cwd is the worktree");
    let out = std::process::Command::new("git").arg("-C").arg(h._repos.path().join("demo")).args(["branch", "--list", branch]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains(branch), "branch exists in the repo");
    // Seqs are consecutive from 0.
    for (i, (e, _)) in events.iter().enumerate() {
        assert_eq!(e.seq, i as i64);
    }
    // Live delivery: each event reached the browser-side subscriber within a second of being produced.
    for (e, received_at) in &events {
        let produced = e.at;
        let received = chrono::Utc::now() - chrono::Duration::from_std(received_at.elapsed()).unwrap();
        let lag = (received - produced).num_milliseconds();
        assert!(lag < 1000, "event {} ({}) arrived {lag}ms after it was recorded", e.seq, e.kind);
    }
    assert_eq!(events[1].0.payload["worker_id"], "test-worker");
    let all = serde_json::to_string(&events.iter().map(|(e, _)| e).collect::<Vec<_>>()).unwrap();
    assert!(!all.contains("ghp_abc") && !all.contains("supersecretvalue99"), "{all}");

    // Replay after the fact returns the same history and ends immediately.
    let replay = h.sse_collect(id, Duration::from_secs(3)).await;
    assert_eq!(replay.len(), events.len());

    // The worker picks up the next run too; `group/other` is not a real git repo, so it
    // falls back to running in place and says so.
    let id2 = h.create("second", "group/other").await;
    h.wait_status(id2, RunStatus::Succeeded, Duration::from_secs(15)).await;
    let ws2 = sidefleet_server::db::events_since(&h.env.pool, id2, 0).await.unwrap().into_iter().find(|e| e.kind == "workspace").unwrap().payload;
    assert_eq!(ws2["mode"], "checkout");
    assert!(ws2["note"].as_str().unwrap().contains("could not create a worktree"), "{ws2}");

    h.teardown().await;
}

#[tokio::test]
async fn stop_while_running_kills_the_agent_and_marks_stopped() {
    let h = Harness::start(1).await;
    let id = h.create("a slow task", "demo").await;
    h.wait_status(id, RunStatus::Running, Duration::from_secs(10)).await;

    // Let it get past its tool call, then stop it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let pid = loop {
        let evs = sidefleet_server::db::events_since(&h.env.pool, id, 0).await.unwrap();
        if evs.iter().any(|e| e.kind == "tool_result") {
            break evs.iter().find(|e| e.kind == "started").unwrap().payload["pid"].as_i64().unwrap() as i32;
        }
        assert!(tokio::time::Instant::now() < deadline, "agent never produced a tool result");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(sidefleet_worker::agent::process_group_alive(pid));

    let t0 = std::time::Instant::now();
    let r = h.http.post(format!("{}/v1/runs/{id}/stop", h.env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let run = h.wait_status(id, RunStatus::Stopped, Duration::from_secs(10)).await;
    assert!(t0.elapsed() < Duration::from_secs(6), "stop took {:?}", t0.elapsed());
    assert_eq!(run.result.unwrap()["reason"], "stopped by request");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!sidefleet_worker::agent::process_group_alive(pid), "agent process group survived the stop");

    let kinds: Vec<String> = sidefleet_server::db::events_since(&h.env.pool, id, 0).await.unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"stop_requested".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"stopped".to_string()), "{kinds:?}");
    assert_eq!(kinds.last().unwrap(), "status");
    assert!(!kinds.contains(&"result".to_string()), "no final result after a kill: {kinds:?}");

    h.teardown().await;
}


#[tokio::test]
async fn parallel_tasks_on_one_repo_run_side_by_side_in_separate_worktrees() {
    let h = Harness::start(2).await;
    let a = h.create("slow one", "demo").await;
    let b = h.create("slow two", "demo").await;
    // Both must be running at the same time (parallel = 2), in different worktrees.
    let ra = h.wait_status(a, RunStatus::Running, Duration::from_secs(10)).await;
    let rb = h.wait_status(b, RunStatus::Running, Duration::from_secs(10)).await;
    assert_eq!(ra.status, RunStatus::Running);
    assert_eq!(rb.status, RunStatus::Running);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (wa, wb) = loop {
        let ea = sidefleet_server::db::events_since(&h.env.pool, a, 0).await.unwrap();
        let eb = sidefleet_server::db::events_since(&h.env.pool, b, 0).await.unwrap();
        let fa = ea.iter().find(|e| e.kind == "workspace").map(|e| e.payload.clone());
        let fb = eb.iter().find(|e| e.kind == "workspace").map(|e| e.payload.clone());
        if let (Some(x), Some(y)) = (fa, fb) { break (x, y); }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(wa["mode"], "worktree");
    assert_eq!(wb["mode"], "worktree");
    assert_ne!(wa["path"], wb["path"], "each task has its own worktree");
    assert_ne!(wa["branch"], wb["branch"]);
    // Stop both so the test ends quickly.
    for id in [a, b] {
        h.http.post(format!("{}/v1/runs/{id}/stop", h.env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap();
    }
    h.wait_status(a, RunStatus::Stopped, Duration::from_secs(10)).await;
    h.wait_status(b, RunStatus::Stopped, Duration::from_secs(10)).await;
    h.teardown().await;
}

#[tokio::test]
async fn in_place_tasks_on_the_same_repo_run_one_at_a_time() {
    let h = Harness::start(2).await;
    let mk = |task: &'static str| {
        let http = h.http.clone(); let base = h.env.base_url.clone();
        async move {
            let r = http.post(format!("{base}/v1/runs")).bearer_auth(DEV_KEY)
                .json(&json!({"task": task, "model": "claude-haiku-5-5", "repo": "demo", "isolation": "checkout"}))
                .send().await.unwrap();
            assert_eq!(r.status(), 202);
            r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse::<Uuid>().unwrap()
        }
    };
    let a = mk("slow first").await;
    let b = mk("second").await;
    h.wait_status(a, RunStatus::Running, Duration::from_secs(10)).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let rb: Run = h.dev_get(&format!("/v1/runs/{b}")).await;
    assert_eq!(rb.status, RunStatus::Queued, "second in-place task must wait for the first on the same repo");
    h.http.post(format!("{}/v1/runs/{a}/stop", h.env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap();
    h.wait_status(a, RunStatus::Stopped, Duration::from_secs(10)).await;
    h.wait_status(b, RunStatus::Succeeded, Duration::from_secs(15)).await;
    let ws = sidefleet_server::db::events_since(&h.env.pool, b, 0).await.unwrap().into_iter().find(|e| e.kind == "workspace").unwrap().payload;
    assert_eq!(ws["mode"], "checkout");
    h.teardown().await;
}

#[tokio::test]
async fn tasks_go_to_machines_with_their_agent_and_codex_threads_resume() {
    let h = Harness::start(3).await;
    let ws: Vec<serde_json::Value> = h.dev_get("/v1/workers").await;
    let codex = &ws[0]["capabilities"]["agents"]["codex"];
    assert_eq!(codex["signed_in"], true, "{codex}");
    assert_eq!(codex["models"], json!(["gpt-fake"]));
    assert!(ws[0]["capabilities"]["agents"].get("opencode").is_none());

    // No opencode on this machine: its task waits for a machine that has it.
    let oc = h.create_with("for opencode", "demo", "opencode", "openrouter/moonshotai/kimi-k2").await;
    let id = h.create_with("fix the build", "demo", "codex", "gpt-fake").await;
    let run = h.wait_status(id, RunStatus::Succeeded, Duration::from_secs(10)).await;
    assert_eq!(run.agent, sidefleet_core::Agent::Codex);
    assert_eq!(run.session_id.as_deref(), Some("fake-thread"));
    assert_eq!(run.result.as_ref().unwrap()["result"], "task=fix the build");
    let events = h.sse_collect(id, Duration::from_secs(3)).await;
    let init = events.iter().find(|(e, _)| e.kind == "agent_init").expect("agent_init").0.clone();
    assert_eq!(init.payload["agent"], "codex");
    let started = events.iter().find(|(e, _)| e.kind == "started").unwrap().0.clone();
    assert!(started.payload["command"].as_str().unwrap().contains("exec --json"), "{}", started.payload);

    // A reply resumes the Codex thread.
    let r = h
        .http
        .post(format!("{}/v1/runs/{id}/messages", h.env.base_url))
        .bearer_auth(DEV_KEY)
        .json(&json!({"task": "now add a test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    let follow: Uuid = r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let run = h.wait_status(follow, RunStatus::Succeeded, Duration::from_secs(10)).await;
    assert_eq!(run.agent, sidefleet_core::Agent::Codex);
    assert_eq!(run.resume_session_id.as_deref(), Some("fake-thread"));
    assert_eq!(run.result.as_ref().unwrap()["result"], "task=now add a test");

    assert_eq!(h.wait_status(oc, RunStatus::Queued, Duration::from_secs(1)).await.status, RunStatus::Queued);
    h.teardown().await;
}

#[tokio::test]
async fn each_turn_reports_its_changes_and_the_thread_total() {
    let h = Harness::start(2).await;
    let id = h.create("edit the readme", "demo").await;
    h.wait_status(id, RunStatus::Succeeded, Duration::from_secs(15)).await;
    let events = sidefleet_server::db::events_since(&h.env.pool, id, 0).await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).filter(|k| *k != "stderr").collect();
    // The snapshots and the diff land before the run's final status.
    assert_eq!(&kinds[kinds.len() - 4..], ["result", "snapshot", "diff", "status"], "{kinds:?}");
    let d = &events.iter().find(|e| e.kind == "diff").unwrap().payload;
    let files: Vec<(&str, &str)> = d["turn"]["files"].as_array().unwrap().iter().map(|f| (f["path"].as_str().unwrap(), f["status"].as_str().unwrap())).collect();
    assert_eq!(files, vec![("README.md", "modified"), ("notes.txt", "added")]);
    assert_eq!(d["turn"]["additions"], 3);
    assert_eq!(d["turn"]["deletions"], 0);
    assert!(d["turn"]["files"][0]["patch"].as_str().unwrap().contains("+edited by the agent"));
    assert!(d["total"].is_null(), "the first turn is the total");

    // A follow-up reports only its own change, and the thread's total so far.
    let r = h.http.post(format!("{}/v1/runs/{id}/messages", h.env.base_url)).bearer_auth(DEV_KEY).json(&json!({"task": "rename the notes"})).send().await.unwrap();
    let follow: Uuid = r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    h.wait_status(follow, RunStatus::Succeeded, Duration::from_secs(15)).await;
    let events = sidefleet_server::db::events_since(&h.env.pool, follow, 0).await.unwrap();
    let d = &events.iter().find(|e| e.kind == "diff").unwrap().payload;
    let turn: Vec<(&str, &str)> = d["turn"]["files"].as_array().unwrap().iter().map(|f| (f["path"].as_str().unwrap(), f["status"].as_str().unwrap())).collect();
    assert_eq!(turn, vec![("docs-notes.txt", "renamed")]);
    assert_eq!(d["turn"]["files"][0]["old_path"], "notes.txt");
    let total: Vec<&str> = d["total"]["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(total, vec!["README.md", "docs-notes.txt"]);
    h.teardown().await;
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn machine(env: &TestEnv, id: &str, root: &std::path::Path, heartbeat: Duration) -> WorkerConfig {
    WorkerConfig {
        control_url: env.base_url.clone(),
        token: WORKER_TOKEN.into(),
        worker_id: id.into(),
        display_name: id.into(),
        repos_root: root.to_path_buf(),
        claude_bin: fake_bin(),
        codex_bin: "/nonexistent/codex".into(),
        opencode_bin: "/nonexistent/opencode".into(),
        permission_mode: "acceptEdits".into(),
        max_turns: None,
        extra_args: vec![],
        poll_interval: Duration::from_millis(200),
        heartbeat_interval: heartbeat,
        check_sign_in: false,
        scan_depth: 2,
        max_runs: None,
        parallel: 1,
        worktrees_dir: root.join(".worktrees"),
        version: "test-build".into(),
        allow_remote_update: false,
        approval_exe: None,
        approval_timeout: Duration::from_secs(20),
    }
}

#[tokio::test]
async fn a_failover_run_pushes_checkpoints_and_continues_on_another_machine_when_its_machine_drops() {
    let env = TestEnv::start().await;
    let http = Client::new();
    let tmp = tempfile::tempdir().unwrap();
    // One project on a shared remote, cloned on two machines.
    let seed = tmp.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git_init(&seed);
    let origin = tmp.path().join("origin.git");
    git_out(tmp.path(), &["clone", "-q", "--bare", seed.to_str().unwrap(), origin.to_str().unwrap()]);
    let (laptop, desktop) = (tmp.path().join("laptop"), tmp.path().join("desktop"));
    for m in [&laptop, &desktop] {
        std::fs::create_dir_all(m).unwrap();
        git_out(m, &["clone", "-q", origin.to_str().unwrap(), "demo"]);
    }

    // The laptop takes the task; its heartbeat is long so the test decides when it "drops".
    let a = tokio::spawn(run_worker(machine(&env, "laptop", &laptop, Duration::from_secs(3600))));
    // A run without failover changes files too but never pushes (checked on origin below).
    let r = http
        .post(format!("{}/v1/runs", env.base_url))
        .bearer_auth(DEV_KEY)
        .json(&json!({"task": "write", "model": "claude-haiku-5-5", "repo": "demo"}))
        .send()
        .await
        .unwrap();
    let plain: Uuid = r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    for _ in 0..100 {
        let run: Run = http.get(format!("{}/v1/runs/{plain}", env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap().json().await.unwrap();
        if run.status.is_terminal() {
            assert_eq!(run.status, RunStatus::Succeeded);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM run_events WHERE run_id = $1 AND kind = 'checkpoint'").bind(plain).fetch_one(&env.pool).await.unwrap();
    assert_eq!(n, 0);
    let r = http
        .post(format!("{}/v1/runs", env.base_url))
        .bearer_auth(DEV_KEY)
        .json(&json!({"task": "write slow", "model": "claude-haiku-5-5", "repo": "demo", "failover": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    let id: Uuid = r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();

    // After the Bash call that wrote a file, the worker pushes a checkpoint of the task branch.
    let branch = format!("sidefleet/{}", &id.simple().to_string()[..8]);
    let mut checkpoint = None;
    for _ in 0..100 {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT payload FROM run_events WHERE run_id = $1 AND kind = 'checkpoint'").bind(id).fetch_optional(&env.pool).await.unwrap();
        if let Some((p,)) = row {
            checkpoint = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let checkpoint = checkpoint.expect("a checkpoint event");
    assert_eq!(checkpoint["branch"], branch.as_str(), "{checkpoint}");
    let sha = checkpoint["sha"].as_str().unwrap().to_string();
    assert_eq!(git_out(&origin, &["rev-parse", &format!("refs/heads/{branch}")]), sha);
    assert_eq!(git_out(&origin, &["show", &format!("{sha}:failover.txt")]), "half done");
    // Nothing but the task branch was pushed.
    assert_eq!(git_out(&origin, &["for-each-ref", "--format=%(refname)", "refs/heads"]), format!("refs/heads/main\nrefs/heads/{branch}"));

    // The laptop drops: its worker stops and it misses heartbeats; the reaper moves the run.
    a.abort();
    sqlx::query("UPDATE workers SET last_heartbeat_at = now() - interval '2 minutes' WHERE id = 'laptop'").execute(&env.pool).await.unwrap();
    sidefleet_server::db::reap_orphaned_runs(&env.pool, 90).await.unwrap();
    let thread: Vec<Run> = http
        .get(format!("{}/v1/runs/{id}/thread", env.base_url))
        .bearer_auth(DEV_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(thread.len(), 2);
    assert_eq!(thread[0].status, RunStatus::Failed);
    let moved = thread[1].clone();
    assert_eq!(moved.resume_from_branch.as_deref(), Some(branch.as_str()));

    // The desktop picks it up and starts from the pushed branch.
    let b = tokio::spawn(run_worker(machine(&env, "desktop", &desktop, Duration::from_secs(15))));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let run = loop {
        let run: Run = http.get(format!("{}/v1/runs/{}", env.base_url, moved.id)).bearer_auth(DEV_KEY).send().await.unwrap().json().await.unwrap();
        if run.status.is_terminal() {
            break run;
        }
        assert!(tokio::time::Instant::now() < deadline, "moved run is still {:?}", run.status);
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(run.worker_id.as_deref(), Some("desktop"));
    let (ws,): (serde_json::Value,) = sqlx::query_as("SELECT payload FROM run_events WHERE run_id = $1 AND kind = 'workspace'")
        .bind(moved.id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(ws["resumed_from"], branch.as_str(), "{ws}");
    let wt = std::path::PathBuf::from(ws["path"].as_str().unwrap());
    assert!(wt.starts_with(&desktop));
    assert_eq!(std::fs::read_to_string(wt.join("failover.txt")).unwrap(), "half done\n");
    git_out(&wt, &["merge-base", "--is-ancestor", &sha, "HEAD"]);

    b.abort();
    env.teardown().await;
}

#[tokio::test]
async fn each_run_is_snapshotted_and_a_thread_can_be_restored_to_before_a_message() {
    let h = Harness::start(3).await;
    let id = h.create("edit the readme", "demo").await;
    h.wait_status(id, RunStatus::Succeeded, Duration::from_secs(15)).await;
    let events = sidefleet_server::db::events_since(&h.env.pool, id, 0).await.unwrap();
    let wt = std::path::PathBuf::from(events.iter().find(|e| e.kind == "workspace").unwrap().payload["path"].as_str().unwrap().to_string());
    let snap = events.iter().find(|e| e.kind == "snapshot").expect("snapshot event").payload.clone();
    assert!(wt.join("notes.txt").exists());
    // The snapshots live under hidden refs in the repo, not on any branch.
    let repo = h._repos.path().join("demo");
    let refs = std::process::Command::new("git").arg("-C").arg(&repo).args(["for-each-ref", "--format=%(refname) %(objectname)", "refs/sidefleet/"]).output().unwrap();
    let refs = String::from_utf8_lossy(&refs.stdout);
    assert!(refs.contains(&format!("refs/sidefleet/snapshots/{id}/{id}/before {}", snap["before"].as_str().unwrap())), "{refs}");
    assert!(refs.contains(&format!("refs/sidefleet/snapshots/{id}/{id}/after {}", snap["after"].as_str().unwrap())), "{refs}");

    // Restore to before that message: its files are back as they were, and the restore shows what it undid.
    let r = h.http.post(format!("{}/v1/runs/{id}/restore", h.env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap();
    assert_eq!(r.status(), 202);
    let rid: Uuid = r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let restore = h.wait_status(rid, RunStatus::Succeeded, Duration::from_secs(15)).await;
    assert_eq!(restore.kind, "restore");
    assert_eq!(restore.parent_id, Some(id));
    assert!(!wt.join("notes.txt").exists(), "the agent's new file is gone");
    assert_eq!(std::fs::read_to_string(wt.join("README.md")).unwrap(), "demo\n");
    let revents = sidefleet_server::db::events_since(&h.env.pool, rid, 0).await.unwrap();
    let kinds: Vec<&str> = revents.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["status", "status", "workspace", "snapshot", "diff", "restored", "status"], "{kinds:?}");
    let undone: Vec<(&str, &str)> = revents[4].payload["turn"]["files"].as_array().unwrap().iter().map(|f| (f["path"].as_str().unwrap(), f["status"].as_str().unwrap())).collect();
    assert_eq!(undone, vec![("README.md", "modified"), ("notes.txt", "deleted")]);

    // The conversation continues on the same session after a restore.
    let r = h.http.post(format!("{}/v1/runs/{id}/messages", h.env.base_url)).bearer_auth(DEV_KEY).json(&json!({"task": "go on"})).send().await.unwrap();
    assert_eq!(r.status(), 202);
    let fid: Uuid = r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let f = h.wait_status(fid, RunStatus::Succeeded, Duration::from_secs(15)).await;
    assert_eq!(f.resume_session_id.as_deref(), Some("fake-session"));
    h.teardown().await;
}

#[tokio::test]
async fn a_thread_forks_into_a_new_thread_with_another_agent_and_the_files_so_far() {
    let h = Harness::start(2).await;
    let id = h.create("edit the readme", "demo").await;
    h.wait_status(id, RunStatus::Succeeded, Duration::from_secs(15)).await;
    let src_ws = sidefleet_server::db::events_since(&h.env.pool, id, 0).await.unwrap().into_iter().find(|e| e.kind == "workspace").unwrap().payload;

    // Fork into Codex with a chosen model.
    let r = h
        .http
        .post(format!("{}/v1/runs/{id}/fork", h.env.base_url))
        .bearer_auth(DEV_KEY)
        .json(&json!({"task": "now review it", "agent": "codex", "model": "gpt-fake"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    let fid: Uuid = r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let fork = h.wait_status(fid, RunStatus::Succeeded, Duration::from_secs(15)).await;
    assert_eq!(fork.parent_id, None, "a fork is a thread of its own");
    assert_eq!(fork.forked_from, Some(id));
    assert_eq!(fork.agent, sidefleet_core::Agent::Codex);
    assert_eq!(fork.model, "gpt-fake");
    assert_eq!(fork.pinned_worker_id.as_deref(), Some("test-worker"), "runs where the files are");

    // Codex got the earlier conversation ahead of the new message.
    let said = fork.result.as_ref().unwrap()["result"].as_str().unwrap().to_string();
    assert!(said.starts_with("task=This task continues a conversation"), "{said}");
    assert!(said.contains("## User\nedit the readme") && said.contains("The working tree has one untracked file."), "{said}");
    assert!(said.ends_with("New message:\n\nnow review it"), "{said}");

    // Its own worktree and branch, starting with the source's files; nothing for its own diff to show.
    let events = sidefleet_server::db::events_since(&h.env.pool, fid, 0).await.unwrap();
    let ws = events.iter().find(|e| e.kind == "workspace").unwrap().payload.clone();
    assert_eq!(ws["mode"], "worktree");
    assert_ne!(ws["path"], src_ws["path"]);
    assert_ne!(ws["branch"], src_ws["branch"]);
    let wt = std::path::PathBuf::from(ws["path"].as_str().unwrap());
    assert_eq!(std::fs::read_to_string(wt.join("notes.txt")).unwrap(), "one\ntwo\n");
    assert!(std::fs::read_to_string(wt.join("README.md")).unwrap().contains("edited by the agent"));
    assert!(events.iter().any(|e| e.kind == "snapshot"));
    assert!(!events.iter().any(|e| e.kind == "diff"), "the fork changed nothing itself");
    h.teardown().await;
}

async fn create_ask(h: &Harness, task: &str) -> Uuid {
    let r = h
        .http
        .post(format!("{}/v1/runs", h.env.base_url))
        .bearer_auth(DEV_KEY)
        .json(&json!({"task": task, "model": "claude-haiku-5-5", "repo": "demo", "permission_mode": "ask"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap()
}

async fn pending_approval(h: &Harness, id: Uuid) -> serde_json::Value {
    for _ in 0..100 {
        let list: Vec<serde_json::Value> = h.dev_get(&format!("/v1/runs/{id}/approvals")).await;
        if let Some(a) = list.iter().find(|a| a["status"] == "pending") {
            return a.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no approval was requested for {id}");
}

async fn decide(h: &Harness, id: Uuid, approval: &serde_json::Value, verb: &str) {
    let aid = approval["id"].as_str().unwrap();
    let r = h.http.post(format!("{}/v1/runs/{id}/approvals/{aid}/{verb}", h.env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
}

/// "Ask me": the agent's permission prompt reaches the page, waits, and the agent goes on
/// once someone approves; a denial reaches the agent as an error tool result.
#[tokio::test]
async fn ask_me_waits_for_approval_on_the_page() {
    let h = Harness::start(2).await;

    let id = create_ask(&h, "ask before cleaning up").await;
    let a = pending_approval(&h, id).await;
    assert_eq!(a["tool_name"], "Bash");
    assert_eq!(a["input"]["command"], "rm -rf build");
    assert_eq!(a["tool_use_id"], "tu_ask");
    // The agent waits: the run is still going, with no result yet.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.dev_get::<Run>(&format!("/v1/runs/{id}")).await.status, RunStatus::Running);
    decide(&h, id, &a, "approve").await;
    h.wait_status(id, RunStatus::Succeeded, Duration::from_secs(15)).await;
    let events = h.sse_collect(id, Duration::from_secs(5)).await;
    let result = events.iter().find(|(e, _)| e.kind == "tool_result" && e.payload["tool_use_id"] == "tu_ask").expect("tool result");
    assert_eq!(result.0.payload["content"], "approved and ran");
    let started = events.iter().find(|(e, _)| e.kind == "started").unwrap();
    let command = started.0.payload["command"].as_str().unwrap();
    assert!(command.contains("--permission-mode acceptEdits") && command.contains("--permission-prompt-tool mcp__sidefleet__approve"), "{command}");

    let id = create_ask(&h, "ask before cleaning up again").await;
    let a = pending_approval(&h, id).await;
    decide(&h, id, &a, "deny").await;
    h.wait_status(id, RunStatus::Succeeded, Duration::from_secs(15)).await;
    let events = h.sse_collect(id, Duration::from_secs(5)).await;
    let result = events.iter().find(|(e, _)| e.kind == "tool_result" && e.payload["tool_use_id"] == "tu_ask").expect("tool result");
    assert_eq!(result.0.payload["is_error"], true);
    assert!(result.0.payload["content"].as_str().unwrap().contains("denied"), "{}", result.0.payload);

    h.teardown().await;
}

#[tokio::test]
async fn an_idle_worker_starts_a_new_task_at_once() {
    let env = TestEnv::start().await;
    let http = Client::new();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("demo")).unwrap();
    git_init(&tmp.path().join("demo"));
    // The default poll interval: without a held claim a task would wait up to 2s for the next poll.
    let cfg = WorkerConfig { poll_interval: Duration::from_secs(2), ..machine(&env, "w", tmp.path(), Duration::from_secs(15)) };
    let worker = tokio::spawn(run_worker(cfg));
    for _ in 0..50 {
        let ws: Vec<serde_json::Value> = http.get(format!("{}/v1/workers", env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap().json().await.unwrap();
        if !ws.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Idle past a poll interval, so the worker is between polls (or holding a claim).
    tokio::time::sleep(Duration::from_millis(2500)).await;

    let asked = tokio::time::Instant::now();
    let r = http
        .post(format!("{}/v1/runs", env.base_url))
        .bearer_auth(DEV_KEY)
        .json(&json!({"task": "say hi", "model": "claude-haiku-5-5", "repo": "demo"}))
        .send()
        .await
        .unwrap();
    let id: Uuid = r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    loop {
        let run: Run = http.get(format!("{}/v1/runs/{id}", env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap().json().await.unwrap();
        if run.status != RunStatus::Queued {
            break;
        }
        assert!(asked.elapsed() < Duration::from_secs(1), "still queued after {:?}", asked.elapsed());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    worker.abort();
    env.teardown().await;
}
