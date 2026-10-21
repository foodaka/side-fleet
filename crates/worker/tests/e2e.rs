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
            'outer: while let Ok(Some(Ok(b))) = tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
                buf.push_str(&String::from_utf8_lossy(&b));
                while let Some(idx) = buf.find("\n\n") {
                    let frame = buf[..idx].to_string();
                    buf = buf[idx + 2..].to_string();
                    if frame.lines().any(|l| l.trim() == "event: end" || l.trim() == "event:end") {
                        break 'outer;
                    }
                    for line in frame.lines() {
                        if let Some(d) = line.strip_prefix("data:") {
                            if let Ok(ev) = serde_json::from_str::<RunEvent>(d.trim()) {
                                out.push((ev, std::time::Instant::now()));
                            }
                        }
                    }
                }
            }
            out
        }
    });

    let run = h.wait_status(id, RunStatus::Succeeded, Duration::from_secs(15)).await;
    assert_eq!(run.worker_id.as_deref(), Some("test-worker"));
    assert!(run.started_at.is_some() && run.finished_at.is_some());
    assert!(run.result.unwrap()["result"].as_str().unwrap().contains("Model was claude-haiku-5-5"));

    let events = collector.await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|(e, _)| e.kind.as_str()).collect();
    assert!(kinds.contains(&"stderr"), "{kinds:?}");
    // stderr interleaving relative to stdout is inherently unordered; check the stdout-derived order.
    let ordered: Vec<&str> = kinds.iter().copied().filter(|k| *k != "stderr").collect();
    assert_eq!(
        ordered,
        vec![
            "status", "status", "workspace", "started", "agent_init", "assistant_text", "tool_use", "tool_result",
            "assistant_text", "result", "status"
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
