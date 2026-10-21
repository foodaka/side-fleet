//! Control plane integration tests: lifecycle transitions, event ordering and
//! SSE replay, stop semantics, redaction. Needs Postgres (docker compose up -d).

use std::time::Duration;

use sidefleet_server::test_support::{TestEnv, DEV_KEY, WORKER_TOKEN};
use sidefleet_core::{Capabilities, RepoInfo, Run, RunControl, RunEvent, RunStatus, RunSummary};
use futures::StreamExt;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

struct Api {
    c: Client,
    base: String,
}

impl Api {
    fn new(base: &str) -> Self {
        Self { c: Client::new(), base: base.to_string() }
    }
    fn dev(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.c.request(method, format!("{}{}", self.base, path)).bearer_auth(DEV_KEY)
    }
    fn worker(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.c.request(method, format!("{}{}", self.base, path)).bearer_auth(WORKER_TOKEN)
    }
    async fn create_run(&self, task: &str, repo: &str) -> (StatusCode, Value) {
        let r = self
            .dev(reqwest::Method::POST, "/v1/runs")
            .json(&json!({"task": task, "model": "claude-sonnet-5-5", "repo": repo}))
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn get_run(&self, id: Uuid) -> Run {
        self.dev(reqwest::Method::GET, &format!("/v1/runs/{id}")).send().await.unwrap().json().await.unwrap()
    }
    async fn stop(&self, id: Uuid) -> Run {
        self.dev(reqwest::Method::POST, &format!("/v1/runs/{id}/stop")).send().await.unwrap().json().await.unwrap()
    }
    async fn register(&self, id: &str, repos: &[&str]) {
        let caps = Capabilities {
            repos: repos.iter().map(|r| RepoInfo { name: r.to_string(), path: format!("/tmp/{r}"), id: String::new() }).collect(),
            ..Default::default()
        };
        let r = self
            .worker(reqwest::Method::POST, "/v1/worker/register")
            .json(&json!({"id": id, "display_name": id, "capabilities": caps, "active_runs": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
    async fn claim(&self, worker: &str, repos: &[&str]) -> Option<Run> {
        let r = self
            .worker(reqwest::Method::POST, "/v1/worker/claim")
            .json(&json!({"worker_id": worker, "repos": repos, "busy_repos": []}))
            .send()
            .await
            .unwrap();
        match r.status() {
            StatusCode::NO_CONTENT => None,
            StatusCode::OK => Some(r.json().await.unwrap()),
            s => panic!("unexpected claim status {s}: {}", r.text().await.unwrap()),
        }
    }
    async fn post_events(&self, worker: &str, id: Uuid, events: Value) -> (StatusCode, Option<RunControl>) {
        let r = self
            .worker(reqwest::Method::POST, &format!("/v1/worker/runs/{id}/events?worker_id={worker}"))
            .json(&json!({"events": events}))
            .send()
            .await
            .unwrap();
        let s = r.status();
        if s == StatusCode::OK {
            (s, Some(r.json().await.unwrap()))
        } else {
            (s, None)
        }
    }
    async fn finish(&self, worker: &str, id: Uuid, status: &str, result: Value) -> (StatusCode, Value) {
        let r = self
            .worker(reqwest::Method::POST, &format!("/v1/worker/runs/{id}/finish?worker_id={worker}"))
            .json(&json!({"status": status, "result": result}))
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn control(&self, id: Uuid) -> RunControl {
        self.worker(reqwest::Method::GET, &format!("/v1/worker/runs/{id}/control"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

/// Minimal SSE client: yields (event name, id, data) frames.
struct Sse {
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
    buf: String,
}

impl Sse {
    async fn open(api: &Api, id: Uuid, last_event_id: Option<i64>) -> Sse {
        let mut req = api.dev(reqwest::Method::GET, &format!("/v1/runs/{id}/events"));
        if let Some(l) = last_event_id {
            req = req.header("Last-Event-ID", l.to_string());
        }
        let r = req.send().await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(r.headers().get("content-type").unwrap().to_str().unwrap().starts_with("text/event-stream"));
        Sse { stream: Box::pin(r.bytes_stream()), buf: String::new() }
    }

    /// Next frame within `timeout`, or None if the stream ended / timed out.
    async fn next(&mut self, timeout: Duration) -> Option<(String, Option<i64>, String)> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(idx) = self.buf.find("\n\n") {
                let chunk = self.buf[..idx].to_string();
                self.buf = self.buf[idx + 2..].to_string();
                let mut name = "message".to_string();
                let mut id = None;
                let mut data = String::new();
                for line in chunk.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        name = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("id:") {
                        id = v.trim().parse().ok();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    } else if line.starts_with(':') {
                        // comment / keep-alive
                    }
                }
                if chunk.starts_with(':') && data.is_empty() {
                    continue;
                }
                return Some((name, id, data));
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match tokio::time::timeout(remaining, self.stream.next()).await {
                Ok(Some(Ok(bytes))) => self.buf.push_str(&String::from_utf8_lossy(&bytes)),
                Ok(Some(Err(_))) | Ok(None) => return None,
                Err(_) => return None,
            }
        }
    }

    async fn next_event(&mut self, timeout: Duration) -> Option<RunEvent> {
        loop {
            let (name, _, data) = self.next(timeout).await?;
            if name == "end" {
                return None;
            }
            if let Ok(ev) = serde_json::from_str::<RunEvent>(&data) {
                return Some(ev);
            }
        }
    }
}

#[tokio::test]
async fn auth_is_required_for_both_audiences() {
    let env = TestEnv::start().await;
    let c = Client::new();
    let r = c.get(format!("{}/v1/workers", env.base_url)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = c.get(format!("{}/v1/workers", env.base_url)).bearer_auth(WORKER_TOKEN).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "worker token must not open the developer API");
    let r = c.post(format!("{}/v1/worker/claim", env.base_url)).bearer_auth(DEV_KEY).json(&json!({})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "dev key must not open the worker API");
    let r = c.get(format!("{}/", env.base_url)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()["cache-control"], "no-cache");
    let html = r.text().await.unwrap();
    assert!(html.contains("<title>Sidefleet</title>"), "{html}");
    // The embedded app's hashed assets are served with a long cache and compressed.
    let asset = html
        .split('"')
        .find(|s| s.contains("_app/immutable/") && s.ends_with(".js"))
        .expect("page references a built script")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string();
    let r = c.get(format!("{}/{asset}", env.base_url)).header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK, "{asset}");
    assert!(r.headers()["content-type"].to_str().unwrap().contains("javascript"));
    assert!(r.headers()["cache-control"].to_str().unwrap().contains("immutable"));
    let r = c.get(format!("{}/_app/immutable/nope.js", env.base_url)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    env.teardown().await;
}

#[tokio::test]
async fn run_lifecycle_transitions() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);

    // Validation.
    let (s, body) = api.create_run("", "repo-a").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "x", "model": "gpt-9", "repo": "repo-a"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    // queued
    let (s, body) = api.create_run("say hi", "repo-a").await;
    assert_eq!(s, StatusCode::ACCEPTED, "{body}");
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(body["status"], "queued");
    let run = api.get_run(id).await;
    assert_eq!(run.status, RunStatus::Queued);
    assert!(run.worker_id.is_none() && run.started_at.is_none());

    // A worker without that repo cannot claim it.
    api.register("w1", &["other"]).await;
    assert!(api.claim("w1", &["other"]).await.is_none());
    // Unknown worker cannot claim.
    let r = api
        .worker(reqwest::Method::POST, "/v1/worker/claim")
        .json(&json!({"worker_id": "ghost", "repos": ["repo-a"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    // running
    api.register("w2", &["repo-a"]).await;
    let claimed = api.claim("w2", &["repo-a"]).await.expect("claimed");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.status, RunStatus::Running);
    assert_eq!(claimed.worker_id.as_deref(), Some("w2"));
    assert!(claimed.started_at.is_some());
    assert!(api.claim("w2", &["repo-a"]).await.is_none(), "claimed twice");

    // Another worker cannot post events or finish it.
    let (s, _) = api.post_events("w1", id, json!([{"kind": "assistant_text", "payload": {"text": "hi"}}])).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = api.finish("w1", id, "succeeded", json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Non-terminal finish status is rejected.
    let (s, _) = api.finish("w2", id, "running", json!({})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // succeeded
    let (s, ctl) = api.post_events("w2", id, json!([{"kind": "assistant_text", "payload": {"text": "hi"}}])).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!ctl.unwrap().stop_requested);
    let (s, body) = api.finish("w2", id, "succeeded", json!({"result": "done", "cost": 0.01})).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let run = api.get_run(id).await;
    assert_eq!(run.status, RunStatus::Succeeded);
    assert!(run.finished_at.is_some());
    assert_eq!(run.result.unwrap()["result"], "done");

    // Terminal runs ignore further events, finishes and stops.
    let (s, ctl) = api.post_events("w2", id, json!([{"kind": "assistant_text", "payload": {"text": "late"}}])).await;
    assert_eq!(s, StatusCode::OK);
    assert!(ctl.unwrap().stop_requested, "late events get told to stop");
    let (s, body) = api.finish("w2", id, "failed", json!({})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "succeeded");
    assert_eq!(api.stop(id).await.status, RunStatus::Succeeded);

    // queued -> stopped directly.
    let (_, body) = api.create_run("never runs", "repo-a").await;
    let id2: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let stopped = api.stop(id2).await;
    assert_eq!(stopped.status, RunStatus::Stopped);
    assert!(stopped.finished_at.is_some());
    assert!(api.claim("w2", &["repo-a"]).await.is_none(), "stopped run must not be claimable");

    // failed
    let (_, body) = api.create_run("will fail", "repo-a").await;
    let id3: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    api.claim("w2", &["repo-a"]).await.unwrap();
    api.finish("w2", id3, "failed", json!({"error": "boom"})).await;
    let run = api.get_run(id3).await;
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.result.unwrap()["error"], "boom");

    // The events table recorded every transition in order.
    let evs: Vec<RunEvent> = sidefleet_server::db::events_since(&env.pool, id, 0).await.unwrap();
    let kinds: Vec<(i64, String)> = evs.iter().map(|e| (e.seq, format!("{}:{}", e.kind, e.payload["status"].as_str().unwrap_or("-")))).collect();
    assert_eq!(
        kinds,
        vec![
            (0, "status:queued".into()),
            (1, "status:running".into()),
            (2, "assistant_text:-".into()),
            (3, "status:succeeded".into()),
        ]
    );

    // Workers list shows the registered workers with their repos.
    let ws: Vec<Value> = api.dev(reqwest::Method::GET, "/v1/workers").send().await.unwrap().json().await.unwrap();
    assert_eq!(ws.len(), 2);
    let w2 = ws.iter().find(|w| w["id"] == "w2").unwrap();
    assert_eq!(w2["capabilities"]["repos"][0]["name"], "repo-a");
    assert_eq!(w2["online"], true);

    env.teardown().await;
}

#[tokio::test]
async fn event_ordering_and_sse_replay() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    let (_, body) = api.create_run("stream me", "r").await;
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();

    // Subscriber connected before anything happens sees queued immediately, then live events.
    let mut live = Sse::open(&api, id, None).await;
    let first = live.next_event(Duration::from_secs(2)).await.unwrap();
    assert_eq!((first.seq, first.kind.as_str(), first.payload["status"].as_str()), (0, "status", Some("queued")));

    api.claim("w", &["r"]).await.unwrap();
    let ev = live.next_event(Duration::from_secs(1)).await.unwrap();
    assert_eq!((ev.seq, ev.payload["status"].as_str()), (1, Some("running")));

    // Two batches, posted before anyone replays, must come back in order with consecutive seqs.
    api.post_events("w", id, json!([
        {"kind": "assistant_text", "payload": {"text": "one"}},
        {"kind": "tool_use", "payload": {"id": "t1", "name": "Bash", "summary": "ls"}},
    ])).await;
    api.post_events("w", id, json!([
        {"kind": "tool_result", "payload": {"tool_use_id": "t1", "content": "a\nb", "is_error": false}},
    ])).await;

    // Live subscriber gets them within a second each.
    for expected in [(2, "assistant_text"), (3, "tool_use"), (4, "tool_result")] {
        let t0 = std::time::Instant::now();
        let ev = live.next_event(Duration::from_secs(1)).await.expect("live event");
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!((ev.seq, ev.kind.as_str()), expected);
    }

    // Replay from 0 for a fresh subscriber: everything so far, in order.
    let mut replay = Sse::open(&api, id, None).await;
    let mut seqs = Vec::new();
    for _ in 0..5 {
        let ev = replay.next_event(Duration::from_secs(2)).await.expect("replayed event");
        seqs.push(ev.seq);
    }
    assert_eq!(seqs, vec![0, 1, 2, 3, 4]);

    // Resume with Last-Event-ID: only newer events.
    let mut resume = Sse::open(&api, id, Some(2)).await;
    let ev = resume.next_event(Duration::from_secs(2)).await.unwrap();
    assert_eq!(ev.seq, 3);
    let ev = resume.next_event(Duration::from_secs(2)).await.unwrap();
    assert_eq!(ev.seq, 4);

    // A late event reaches all three open streams, then finishing ends them.
    api.post_events("w", id, json!([{"kind": "assistant_text", "payload": {"text": "late"}}])).await;
    for s in [&mut live, &mut replay, &mut resume] {
        let ev = s.next_event(Duration::from_secs(1)).await.unwrap();
        assert_eq!((ev.seq, ev.payload["text"].as_str()), (5, Some("late")));
    }
    api.finish("w", id, "succeeded", json!({"result": "ok"})).await;
    for s in [&mut live, &mut replay, &mut resume] {
        let ev = s.next_event(Duration::from_secs(1)).await.unwrap();
        assert_eq!((ev.seq, ev.payload["status"].as_str()), (6, Some("succeeded")));
        assert!(s.next_event(Duration::from_secs(2)).await.is_none(), "stream must end after terminal status");
    }

    // A subscriber to a finished run gets the full history and an immediate end.
    let mut after = Sse::open(&api, id, None).await;
    let mut n = 0;
    while after.next_event(Duration::from_secs(2)).await.is_some() {
        n += 1;
    }
    assert_eq!(n, 7);

    env.teardown().await;
}

#[tokio::test]
async fn stop_while_running_is_signalled_to_the_worker() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    let (_, body) = api.create_run("long task", "r").await;
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    assert!(!api.control(id).await.stop_requested);

    let mut sse = Sse::open(&api, id, None).await;
    sse.next_event(Duration::from_secs(1)).await.unwrap(); // queued
    sse.next_event(Duration::from_secs(1)).await.unwrap(); // running

    let run = api.stop(id).await;
    assert_eq!(run.status, RunStatus::Running, "running run stays running until the worker confirms");
    assert!(run.stop_requested);
    let ev = sse.next_event(Duration::from_secs(1)).await.unwrap();
    assert_eq!(ev.kind, "stop_requested");

    // Worker learns about it from the control endpoint and from its next event post.
    assert!(api.control(id).await.stop_requested);
    let (_, ctl) = api.post_events("w", id, json!([{"kind": "stderr", "payload": {"text": "killing"}}])).await;
    assert!(ctl.unwrap().stop_requested);

    // A second stop is idempotent and adds no event.
    api.stop(id).await;
    let (_, _) = api.finish("w", id, "stopped", json!({"reason": "killed"})).await;
    let run = api.get_run(id).await;
    assert_eq!(run.status, RunStatus::Stopped);
    let kinds: Vec<String> = sidefleet_server::db::events_since(&env.pool, id, 0).await.unwrap().into_iter().map(|e| e.kind).collect();
    assert_eq!(kinds, vec!["status", "status", "stop_requested", "stderr", "status"]);

    env.teardown().await;
}

#[tokio::test]
async fn control_plane_redacts_secrets_in_stored_payloads() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    let (_, body) = api.create_run("leak", "r").await;
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    api.post_events("w", id, json!([
        {"kind": "tool_use", "payload": {"name": "Bash", "summary": "curl -H 'Authorization: Bearer sk-ant-abcdefghijklmnopqrstuvwxyz' x"}},
        {"kind": "tool_result", "payload": {"content": "GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz1234", "api_key": "whatever"}},
    ])).await;
    api.finish("w", id, "succeeded", json!({"result": "token=abcdefghijkl done"})).await;
    let evs = sidefleet_server::db::events_since(&env.pool, id, 0).await.unwrap();
    let all = serde_json::to_string(&evs).unwrap();
    assert!(!all.contains("sk-ant-abc"), "{all}");
    assert!(!all.contains("ghp_abc"), "{all}");
    assert!(!all.contains("whatever"), "{all}");
    let run = api.get_run(id).await;
    assert!(!run.result.unwrap().to_string().contains("abcdefghijkl"));
    env.teardown().await;
}


#[tokio::test]
async fn follow_ups_resume_the_session_on_the_same_worker() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w1", &["r"]).await;
    api.register("w2", &["r"]).await;

    // Root run with an explicit permission mode.
    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "first", "model": "claude-haiku-5-5", "repo": "r", "permission_mode": "bypassPermissions"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let root: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(api.get_run(root).await.permission_mode.as_deref(), Some("bypassPermissions"));
    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "x", "model": "claude-haiku-5-5", "repo": "r", "permission_mode": "sudo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "unknown permission mode rejected");

    // No session yet: a follow-up is refused while the root is queued (busy), and after a
    // finish without any session id (no session).
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{root}/messages")).json(&json!({"task": "again"})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);

    let claimed = api.claim("w1", &["r"]).await.unwrap();
    assert_eq!(claimed.id, root);
    api.post_events("w1", root, json!([
        {"kind": "agent_init", "payload": {"model": "claude-haiku-5-5", "session_id": "sess-abc"}},
        {"kind": "assistant_text", "payload": {"text": "hello"}},
    ])).await;
    assert_eq!(api.get_run(root).await.session_id.as_deref(), Some("sess-abc"), "session id captured from agent_init");
    api.finish("w1", root, "succeeded", json!({"result": "hello"})).await;

    // Follow-up: resumes sess-abc, pinned to w1, inherits model/repo/permissions.
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{root}/messages")).json(&json!({"task": "and then?"})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let f1: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let f1run = api.get_run(f1).await;
    assert_eq!(f1run.parent_id, Some(root));
    assert_eq!(f1run.resume_session_id.as_deref(), Some("sess-abc"));
    assert_eq!(f1run.pinned_worker_id.as_deref(), Some("w1"));
    assert_eq!(f1run.model, "claude-haiku-5-5");
    assert_eq!(f1run.repo, "r");
    assert_eq!(f1run.permission_mode.as_deref(), Some("bypassPermissions"));

    // Another worker with the repo must not get it; the pinned one does.
    assert!(api.claim("w2", &["r"]).await.is_none(), "follow-up leaked to a worker without the session");
    let c = api.claim("w1", &["r"]).await.expect("pinned worker claims the follow-up");
    assert_eq!(c.id, f1);

    // While it runs, a further message is refused as busy.
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{root}/messages")).json(&json!({"task": "more"})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    api.post_events("w1", f1, json!([{"kind": "result", "payload": {"subtype": "success", "is_error": false, "result": "ok", "session_id": "sess-abc"}}])).await;
    api.finish("w1", f1, "succeeded", json!({"result": "ok"})).await;

    // Messages can address any run in the thread and still attach to the root.
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{f1}/messages")).json(&json!({"task": "third", "permission_mode": "plan"})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let f2: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let f2run = api.get_run(f2).await;
    assert_eq!(f2run.parent_id, Some(root));
    assert_eq!(f2run.permission_mode.as_deref(), Some("plan"));

    // Thread lists root first; the runs list shows one thread with two follow-ups.
    let thread: Vec<Run> = api.dev(reqwest::Method::GET, &format!("/v1/runs/{f2}/thread")).send().await.unwrap().json().await.unwrap();
    assert_eq!(thread.iter().map(|r| r.id).collect::<Vec<_>>(), vec![root, f1, f2]);
    let list: Vec<RunSummary> = api.dev(reqwest::Method::GET, "/v1/runs").send().await.unwrap().json().await.unwrap();
    let mine: Vec<&RunSummary> = list.iter().filter(|s| s.run.id == root).collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].follow_ups, 2);
    assert_eq!(mine[0].thread_status, RunStatus::Queued);
    assert!(list.iter().all(|s| s.run.parent_id.is_none()), "follow-ups are not listed as threads");

    env.teardown().await;
}

#[tokio::test]
async fn added_repos_round_trip() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    let get = || async { api.dev(reqwest::Method::GET, "/v1/repos").send().await.unwrap().json::<Value>().await.unwrap() };
    assert_eq!(get().await, json!({"added": []}));

    let r = api.dev(reqwest::Method::PUT, "/v1/repos").json(&json!({"added": ["b", " a ", "b", ""]})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(get().await, json!({"added": ["a", "b"]}));

    // Replacing drops what is no longer listed.
    api.dev(reqwest::Method::PUT, "/v1/repos").json(&json!({"added": ["c", "a"]})).send().await.unwrap();
    assert_eq!(get().await, json!({"added": ["a", "c"]}));

    let r = Client::new().get(format!("{}/v1/repos", env.base_url)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    env.teardown().await;
}


#[tokio::test]
async fn runs_lost_by_a_worker_restart_are_finished_and_stops_on_offline_workers_apply_at_once() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;

    // Worker claims a run, then "restarts": it registers again with no active runs.
    let (_, body) = api.create_run("lost", "r").await;
    let lost: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    let mut sse = Sse::open(&api, lost, None).await;
    sse.next_event(Duration::from_secs(1)).await.unwrap(); // queued
    sse.next_event(Duration::from_secs(1)).await.unwrap(); // running
    api.register("w", &["r"]).await;
    let run = api.get_run(lost).await;
    assert_eq!(run.status, RunStatus::Failed);
    assert!(run.result.unwrap()["error"].as_str().unwrap().contains("restarted"));
    let ev = sse.next_event(Duration::from_secs(1)).await.unwrap();
    assert_eq!((ev.kind.as_str(), ev.payload["status"].as_str()), ("status", Some("failed")));

    // A heartbeat that omits a run the worker owns has the same effect; a listed run is untouched.
    let (_, body) = api.create_run("kept", "r").await;
    let kept: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let (_, body) = api.create_run("dropped", "r").await;
    let dropped: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    api.claim("w", &["r"]).await.unwrap();
    // Ask for a stop on the one about to be dropped: it must end as stopped, not failed.
    api.stop(dropped).await;
    let r = api
        .worker(reqwest::Method::POST, "/v1/worker/heartbeat")
        .json(&json!({"id": "w", "capabilities": {}, "active_runs": [kept]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(api.get_run(kept).await.status, RunStatus::Running);
    assert_eq!(api.get_run(dropped).await.status, RunStatus::Stopped);

    // Worker goes offline: a stop finishes the run immediately instead of waiting.
    sqlx::query("UPDATE workers SET last_heartbeat_at = now() - interval '2 minutes' WHERE id = 'w'")
        .execute(&env.pool)
        .await
        .unwrap();
    let run = api.stop(kept).await;
    assert_eq!(run.status, RunStatus::Stopped);
    assert!(run.finished_at.is_some());

    env.teardown().await;
}


#[tokio::test]
async fn update_can_be_requested_from_the_page_and_is_delivered_on_heartbeat() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;

    let hb = |caps_version: &str| {
        let api = &api;
        let v = caps_version.to_string();
        async move {
            let r = api
                .worker(reqwest::Method::POST, "/v1/worker/heartbeat")
                .json(&json!({"id": "w", "capabilities": {"version": v}, "active_runs": []}))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
            r.json::<Value>().await.unwrap()
        }
    };
    let h = hb("0.1.0 (old)").await;
    assert_eq!(h["update_requested"], false);
    assert_eq!(h["server_version"], "test");

    // The page sees the build and that it differs from the control plane's.
    let ws: Vec<Value> = api.dev(reqwest::Method::GET, "/v1/workers").send().await.unwrap().json().await.unwrap();
    assert_eq!(ws[0]["capabilities"]["version"], "0.1.0 (old)");
    assert_eq!(ws[0]["outdated"], true);
    assert_eq!(ws[0]["update_requested"], false);

    // Request an update: visible on the next heartbeat and in the workers list.
    let r = api.dev(reqwest::Method::POST, "/v1/workers/w/update").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let r = api.dev(reqwest::Method::POST, "/v1/workers/ghost/update").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let ws: Vec<Value> = api.dev(reqwest::Method::GET, "/v1/workers").send().await.unwrap().json().await.unwrap();
    assert_eq!(ws[0]["update_requested"], true);
    assert_eq!(hb("0.1.0 (old)").await["update_requested"], true);

    // The worker acknowledges before updating; the flag clears. A re-register clears it too.
    let r = api.worker(reqwest::Method::POST, "/v1/worker/update-ack").json(&json!({"id": "w"})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    assert_eq!(hb("test").await["update_requested"], false);
    let ws: Vec<Value> = api.dev(reqwest::Method::GET, "/v1/workers").send().await.unwrap().json().await.unwrap();
    assert_eq!(ws[0]["outdated"], false, "same build as the control plane");

    api.dev(reqwest::Method::POST, "/v1/workers/w/update").send().await.unwrap();
    api.register("w", &["r"]).await;
    assert_eq!(hb("test").await["update_requested"], false);

    env.teardown().await;
}

#[tokio::test]
async fn a_run_can_be_pinned_to_a_machine() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w1", &["r"]).await;
    api.register("w2", &["r"]).await;

    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "x", "model": "claude-haiku-5-5", "repo": "r", "worker_id": "nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "unknown machine rejected");

    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "on w2", "model": "claude-haiku-5-5", "repo": "r", "worker_id": "w2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let id: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(api.get_run(id).await.pinned_worker_id.as_deref(), Some("w2"));

    assert!(api.claim("w1", &["r"]).await.is_none(), "pinned run leaked to another machine");
    assert_eq!(api.claim("w2", &["r"]).await.expect("chosen machine claims it").id, id);
}

#[tokio::test]
async fn runs_carry_an_agent_and_only_machines_with_it_claim_them() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    let create = |body: Value| api.dev(reqwest::Method::POST, "/v1/runs").json(&body).send();

    // Validation: known agents only, and a model that suits the agent.
    let r = create(json!({"task": "x", "agent": "gemini", "model": "m", "repo": "r"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert!(r.text().await.unwrap().contains("claude, codex, opencode"));
    let r = create(json!({"task": "x", "agent": "claude", "model": "gpt-6-luna", "repo": "r"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "a Claude Code task takes a Claude model");
    let r = create(json!({"task": "x", "agent": "opencode", "model": "--yolo", "repo": "r"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "a model must not look like a flag");

    let id_of = |v: Value| -> Uuid { v["id"].as_str().unwrap().parse().unwrap() };
    let r = create(json!({"task": "kimi", "agent": "opencode", "model": "openrouter/moonshotai/kimi-k2", "repo": "r"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let oc = id_of(r.json().await.unwrap());
    let r = create(json!({"task": "codex default model", "agent": "codex", "repo": "r"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED, "codex may use the machine's default model");
    let cx = id_of(r.json().await.unwrap());
    let (_, body) = api.create_run("no agent given", "r").await;
    let cl = id_of(body);

    let run = api.get_run(oc).await;
    assert_eq!(run.agent, sidefleet_core::Agent::Opencode);
    assert_eq!(run.model, "openrouter/moonshotai/kimi-k2");
    assert_eq!(api.get_run(cx).await.model, "");
    assert_eq!(api.get_run(cl).await.agent, sidefleet_core::Agent::Claude, "claude is the default");

    let claim = |agents: Value| {
        api.worker(reqwest::Method::POST, "/v1/worker/claim")
            .json(&json!({"worker_id": "w", "repos": ["r"], "busy_repos": [], "agents": agents}))
            .send()
    };
    // A worker from before agents existed sends none and only gets Claude Code tasks.
    assert_eq!(api.claim("w", &["r"]).await.expect("old worker claims").id, cl);
    let r = claim(json!(["claude"])).await.unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT, "no claude tasks left");
    let r = claim(json!(["codex", "opencode"])).await.unwrap();
    assert_eq!(r.json::<Run>().await.unwrap().id, oc, "oldest first among the agents it has");
    let r = claim(json!(["codex"])).await.unwrap();
    assert_eq!(r.json::<Run>().await.unwrap().id, cx);

    // A follow-up keeps the thread's agent; its model is checked against that agent.
    api.post_events("w", oc, json!([{"kind": "agent_init", "payload": {"session_id": "ses_1"}}])).await;
    api.finish("w", oc, "succeeded", json!({})).await;
    let reply = |body: Value| api.dev(reqwest::Method::POST, &format!("/v1/runs/{oc}/messages")).json(&body).send();
    let r = reply(json!({"task": "more", "model": "claude-opus-5-5"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED, "any valid id is a valid opencode model");
    let f = id_of(r.json().await.unwrap());
    let run = api.get_run(f).await;
    assert_eq!(run.agent, sidefleet_core::Agent::Opencode);
    assert_eq!(run.resume_session_id.as_deref(), Some("ses_1"));
    env.teardown().await;
}

/// Claim with what a current worker sends: its agents and its task slots.
async fn claim_with(api: &Api, worker: &str, agents: &[&str], parallel: usize, active: usize) -> Option<Run> {
    let r = api
        .worker(reqwest::Method::POST, "/v1/worker/claim")
        .json(&json!({"worker_id": worker, "repos": ["r"], "busy_repos": [], "agents": agents,
                      "slots": {"parallel": parallel, "active": active}}))
        .send()
        .await
        .unwrap();
    match r.status() {
        StatusCode::NO_CONTENT => None,
        StatusCode::OK => Some(r.json().await.unwrap()),
        s => panic!("unexpected claim status {s}: {}", r.text().await.unwrap()),
    }
}

async fn create_id(api: &Api, body: Value) -> Uuid {
    let r = api.dev(reqwest::Method::POST, "/v1/runs").json(&body).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn unpinned_runs_go_to_the_least_busy_machine_first() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("busy", &["r"]).await;
    api.register("idle", &["r"]).await;
    let task = json!({"task": "x", "model": "claude-haiku-5-5", "repo": "r"});

    // `idle` reports 4 free slots (nothing queued yet); `busy` has 1 of 2 free.
    assert!(claim_with(&api, "idle", &["claude"], 4, 0).await.is_none());
    let id = create_id(&api, task.clone()).await;
    assert!(claim_with(&api, "busy", &["claude"], 2, 1).await.is_none(), "left for the machine with more free slots");
    assert_eq!(claim_with(&api, "idle", &["claude"], 4, 0).await.expect("least busy claims").id, id);

    // The claim counted: `idle` now has 3 free, still more than `busy`.
    let id = create_id(&api, task.clone()).await;
    assert!(claim_with(&api, "busy", &["claude"], 2, 1).await.is_none());
    // A tie goes to whoever asks first.
    assert_eq!(claim_with(&api, "busy", &["claude"], 4, 1).await.expect("equal free slots").id, id);

    // The better machine does not stall a run: after the grace period anyone may take it.
    let id = create_id(&api, task.clone()).await;
    assert!(claim_with(&api, "busy", &["claude"], 2, 1).await.is_none());
    sqlx::query("UPDATE runs SET created_at = now() - interval '11 seconds' WHERE id = $1").bind(id).execute(&env.pool).await.unwrap();
    assert_eq!(claim_with(&api, "busy", &["claude"], 2, 1).await.expect("after the grace period").id, id);

    // Pinned runs and workers that report no slots are not balanced.
    let id = create_id(&api, json!({"task": "x", "model": "claude-haiku-5-5", "repo": "r", "worker_id": "busy"})).await;
    assert_eq!(claim_with(&api, "busy", &["claude"], 2, 1).await.expect("pinned here").id, id);
    let id = create_id(&api, task.clone()).await;
    assert_eq!(api.claim("busy", &["r"]).await.expect("an older worker claims as before").id, id);

    // Only machines that could take the run count: online, with the repo and the agent.
    let id = create_id(&api, json!({"task": "x", "agent": "codex", "repo": "r"})).await;
    assert_eq!(claim_with(&api, "busy", &["claude", "codex"], 2, 1).await.expect("idle has no codex").id, id);
    sqlx::query("UPDATE workers SET last_heartbeat_at = now() - interval '1 minute' WHERE id = 'idle'").execute(&env.pool).await.unwrap();
    let id = create_id(&api, task.clone()).await;
    assert_eq!(claim_with(&api, "busy", &["claude"], 2, 1).await.expect("idle is offline").id, id);

    env.teardown().await;
}

#[tokio::test]
async fn a_usage_limit_moves_the_run_to_another_machine_once() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w1", &["r"]).await;
    api.register("w2", &["r"]).await;
    let task = json!({"task": "x", "model": "claude-haiku-5-5", "repo": "r"});
    let limits = || async {
        let ws: Vec<Value> = api.dev(reqwest::Method::GET, "/v1/workers").send().await.unwrap().json().await.unwrap();
        ws.into_iter().map(|w| (w["id"].as_str().unwrap().to_string(), w["limited_until"].clone())).collect::<std::collections::HashMap<_, _>>()
    };
    let finish = |worker: &'static str, id: Uuid, result: Value, limit: Value| {
        api.worker(reqwest::Method::POST, &format!("/v1/worker/runs/{id}/finish?worker_id={worker}"))
            .json(&json!({"status": "failed", "result": result, "usage_limit": limit}))
            .send()
    };

    // w1 runs the task and Claude Code reports the account's usage limit (captured output).
    let id = create_id(&api, task.clone()).await;
    assert_eq!(claim_with(&api, "w1", &["claude"], 2, 0).await.unwrap().id, id);
    let mut stream = sidefleet_core::AgentStream::new(sidefleet_core::Agent::Claude, "claude-haiku-5-5", "/tmp/r");
    let mut events = Vec::new();
    for line in include_str!("../../core/tests/fixtures/claude_usage_limit.jsonl").lines() {
        if let sidefleet_core::MappedLine::Events(evs) = stream.map_line(line) {
            events.extend(evs);
        }
    }
    let result = events.iter().rev().find(|e| e.kind == "result").unwrap().payload.clone();
    let limit = sidefleet_core::result_usage_limit(&result).expect("fixture is a usage limit");
    api.post_events("w1", id, serde_json::to_value(&events).unwrap()).await;
    let r = finish("w1", id, json!({"error": "agent reported an error", "result": result}), json!(limit)).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let run: Run = r.json().await.unwrap();
    assert_eq!(run.status, RunStatus::Queued, "re-queued for another machine");
    assert_eq!(run.worker_id, None);
    assert_eq!(run.session_id, None, "w1's session does not follow the run");

    // w1 is left alone for Claude Code until the limit resets (the fixture's reset has passed: an hour).
    let until: chrono::DateTime<chrono::Utc> = serde_json::from_value(limits().await["w1"]["claude"].clone()).expect("w1 limited");
    let left = until - chrono::Utc::now();
    assert!(left > chrono::Duration::minutes(55) && left <= chrono::Duration::minutes(60), "{left}");
    assert_eq!(limits().await["w2"], json!({}));
    assert!(claim_with(&api, "w1", &["claude"], 2, 0).await.is_none(), "limited machine skipped");
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM run_events WHERE run_id = $1 ORDER BY seq").bind(id).fetch_all(&env.pool).await.unwrap();
    assert!(kinds.contains(&"usage_limit".to_string()), "{kinds:?}");
    assert_eq!(kinds.last().unwrap(), "status");

    // w2 takes it. If w2 is limited too the run fails: it is re-queued only once.
    assert_eq!(claim_with(&api, "w2", &["claude"], 2, 0).await.expect("other machine claims").id, id);
    let resets = chrono::Utc::now() + chrono::Duration::hours(3);
    let r = finish("w2", id, json!({"error": "limit"}), json!({"resets_at": resets})).await.unwrap();
    assert_eq!(r.json::<Run>().await.unwrap().status, RunStatus::Failed);
    let until: chrono::DateTime<chrono::Utc> = serde_json::from_value(limits().await["w2"]["claude"].clone()).unwrap();
    assert!((until - resets).num_seconds().abs() <= 1, "the reported reset time is used");

    // Limits are per agent: w1 still takes Codex tasks.
    let claude = create_id(&api, task.clone()).await;
    let codex = create_id(&api, json!({"task": "x", "agent": "codex", "repo": "r"})).await;
    assert_eq!(claim_with(&api, "w1", &["claude", "codex"], 2, 0).await.unwrap().id, codex);
    assert!(claim_with(&api, "w2", &["claude"], 2, 0).await.is_none());

    // Once the limit resets the machine takes Claude Code tasks again.
    sqlx::query("UPDATE worker_limits SET until = now() - interval '1 second'").execute(&env.pool).await.unwrap();
    assert_eq!(limits().await["w1"], json!({}));
    assert_eq!(claim_with(&api, "w2", &["claude"], 2, 0).await.unwrap().id, claude);

    // A pinned run is never moved; nor is a run no other machine can take.
    let pinned = create_id(&api, json!({"task": "x", "model": "claude-haiku-5-5", "repo": "r", "worker_id": "w1"})).await;
    assert_eq!(claim_with(&api, "w1", &["claude"], 2, 0).await.unwrap().id, pinned);
    let r = finish("w1", pinned, json!({}), json!({})).await.unwrap();
    assert_eq!(r.json::<Run>().await.unwrap().status, RunStatus::Failed);
    let lone = create_id(&api, task.clone()).await;
    sqlx::query("UPDATE worker_limits SET until = now() - interval '1 second'").execute(&env.pool).await.unwrap();
    assert_eq!(claim_with(&api, "w1", &["claude"], 2, 0).await.unwrap().id, lone);
    sqlx::query("UPDATE workers SET last_heartbeat_at = now() - interval '1 minute' WHERE id = 'w2'").execute(&env.pool).await.unwrap();
    let r = finish("w1", lone, json!({}), json!({})).await.unwrap();
    assert_eq!(r.json::<Run>().await.unwrap().status, RunStatus::Failed, "w2 is offline");

    env.teardown().await;
}
