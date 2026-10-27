//! Control plane integration tests: lifecycle transitions, event ordering and
//! SSE replay, stop semantics, redaction. Needs Postgres (docker compose up -d).

use std::time::Duration;

use sidefleet_server::test_support::{TestEnv, DEV_KEY, WORKER_TOKEN};
use sidefleet_core::{Agent, Capabilities, RepoInfo, Run, RunControl, RunEvent, RunStatus, RunSummary};
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
async fn live_text_reaches_subscribers_redacted_and_is_never_stored() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    let (_, body) = api.create_run("stream me", "r").await;
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    let live = |worker: &str, text: &str| {
        api.worker(reqwest::Method::POST, &format!("/v1/worker/runs/{id}/live?worker_id={worker}")).json(&json!({"text": text})).send()
    };

    // Nobody is watching: accepted and dropped.
    assert_eq!(live("w", "unseen ").await.unwrap().status(), StatusCode::NO_CONTENT);

    let mut sse = Sse::open(&api, id, None).await;
    for _ in 0..2 {
        sse.next_event(Duration::from_secs(1)).await.unwrap(); // queued, running
    }
    assert_eq!(live("other", "not yours ").await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(live("w", "Using token=abcdefgh12345 now ").await.unwrap().status(), StatusCode::NO_CONTENT);
    let (name, frame_id, data) = sse.next(Duration::from_secs(1)).await.unwrap();
    assert_eq!((name.as_str(), frame_id), ("assistant_live", None), "no id, so a resume never skips stored events");
    let v: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(v["kind"], "assistant_live");
    assert_eq!(v["run_id"], id.to_string());
    assert_eq!(v["payload"]["text"], "Using token=[REDACTED] now ");

    // The stored message follows in order; the live text was never an event.
    api.post_events("w", id, json!([{"kind": "assistant_text", "payload": {"text": "Using it now."}}])).await;
    let ev = sse.next_event(Duration::from_secs(1)).await.unwrap();
    assert_eq!((ev.seq, ev.kind.as_str()), (2, "assistant_text"));
    let stored = sidefleet_server::db::events_since(&env.pool, id, 0).await.unwrap();
    assert!(stored.iter().all(|e| e.kind != "assistant_live"), "{stored:?}");

    let too_long = "x".repeat(2 * sidefleet_core::LIVE_TEXT_MAX_BYTES + 1);
    assert_eq!(live("w", &too_long).await.unwrap().status(), StatusCode::PAYLOAD_TOO_LARGE);
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
        .json(&json!({"task": "first", "model": "claude-haiku-5-5", "repo": "r", "permission_mode": "bypassPermissions", "effort": "high"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let root: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(api.get_run(root).await.permission_mode.as_deref(), Some("bypassPermissions"));
    assert_eq!(api.get_run(root).await.effort.as_deref(), Some("high"));
    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "x", "model": "claude-haiku-5-5", "repo": "r", "effort": "minimal"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "an effort Claude Code doesn't take is rejected");
    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "x", "model": "claude-haiku-5-5", "repo": "r", "permission_mode": "sudo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "unknown permission mode rejected");

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
    assert_eq!(f1run.effort.as_deref(), Some("high"));

    // Another worker with the repo must not get it; the pinned one does.
    assert!(api.claim("w2", &["r"]).await.is_none(), "follow-up leaked to a worker without the session");
    let c = api.claim("w1", &["r"]).await.expect("pinned worker claims the follow-up");
    assert_eq!(c.id, f1);

    api.post_events("w1", f1, json!([{"kind": "result", "payload": {"subtype": "success", "is_error": false, "result": "ok", "session_id": "sess-abc"}}])).await;
    api.finish("w1", f1, "succeeded", json!({"result": "ok"})).await;

    // Messages can address any run in the thread and still attach to the root.
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{f1}/messages")).json(&json!({"task": "third", "permission_mode": "plan", "effort": ""})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let f2: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let f2run = api.get_run(f2).await;
    assert_eq!(f2run.parent_id, Some(root));
    assert_eq!(f2run.permission_mode.as_deref(), Some("plan"));
    assert_eq!(f2run.effort, None, "empty effort goes back to the agent's default");

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

#[tokio::test]
async fn a_failover_run_whose_machine_drops_moves_to_another_machine() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("laptop", &["r"]).await;
    api.register("desktop", &["r"]).await;

    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "fix the flaky test", "model": "claude-sonnet-5-5", "repo": "r", "failover": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let lost: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    assert!(api.get_run(lost).await.failover);
    let claimed = api.claim("laptop", &["r"]).await.unwrap();
    assert_eq!(claimed.id, lost);
    let (s, _) = api
        .post_events(
            "laptop",
            lost,
            json!([
                {"kind": "assistant_text", "payload": {"text": "The test races on the clock."}},
                {"kind": "tool_use", "payload": {"id": "t1", "name": "Edit", "summary": "src/clock.rs"}},
                {"kind": "tool_result", "payload": {"tool_use_id": "t1", "content": "ok"}},
                {"kind": "checkpoint", "payload": {"branch": "sidefleet/abcd1234", "sha": "0123456789abcdef"}}
            ]),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // A run without failover on the same machine is only failed.
    let (_, body) = api.create_run("plain", "r").await;
    let plain: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(api.claim("laptop", &["r"]).await.unwrap().id, plain);

    // The laptop sleeps: no heartbeats for two minutes, then the reaper runs.
    sqlx::query("UPDATE workers SET last_heartbeat_at = now() - interval '2 minutes' WHERE id = 'laptop'")
        .execute(&env.pool)
        .await
        .unwrap();
    sidefleet_server::db::reap_orphaned_runs(&env.pool, 90).await.unwrap();

    let run = api.get_run(lost).await;
    assert_eq!(run.status, RunStatus::Failed);
    let p = api.get_run(plain).await;
    assert_eq!(p.status, RunStatus::Failed);

    let thread: Vec<Run> = api.dev(reqwest::Method::GET, &format!("/v1/runs/{lost}/thread")).send().await.unwrap().json().await.unwrap();
    assert_eq!(thread.len(), 2);
    let moved = &thread[1];
    assert_eq!(moved.status, RunStatus::Queued);
    assert_eq!(moved.parent_id, Some(lost));
    assert_eq!(moved.moved_from, Some(lost));
    assert_eq!(moved.pinned_worker_id, None);
    assert_eq!(moved.resume_session_id, None);
    assert_eq!(moved.resume_from_branch.as_deref(), Some("sidefleet/abcd1234"));
    assert_eq!((moved.agent, moved.model.as_str(), moved.repo.as_str()), (run.agent, "claude-sonnet-5-5", "r"));
    assert!(moved.task.starts_with("fix the flaky test"));
    assert!(moved.task.contains("The test races on the clock."));
    assert!(moved.task.contains("- Edit src/clock.rs"));
    assert!(moved.task.contains("Continue from branch sidefleet/abcd1234 at 0123456789abcdef"));
    assert_eq!(run.result.unwrap()["moved_to"].as_str(), Some(moved.id.to_string().as_str()));

    // The page learns of the move from the lost run's stream.
    let mut sse = Sse::open(&api, lost, None).await;
    let mut saw_move = false;
    while let Some(ev) = sse.next_event(Duration::from_secs(1)).await {
        if ev.kind == "status" && ev.payload["status"] == "failed" {
            assert_eq!(ev.payload["reason"], "worker lost, moved");
            assert_eq!(ev.payload["moved_to"].as_str(), Some(moved.id.to_string().as_str()));
            saw_move = true;
            break;
        }
    }
    assert!(saw_move);

    // The laptop wakes up and cannot take it back; the desktop can.
    api.register("laptop", &["r"]).await;
    assert!(api.claim("laptop", &["r"]).await.is_none());
    let taken = api.claim("desktop", &["r"]).await.unwrap();
    assert_eq!(taken.id, moved.id);
    assert_eq!(taken.resume_from_branch.as_deref(), Some("sidefleet/abcd1234"));

    // A moved run does not move again: losing the desktop too just fails it.
    sqlx::query("UPDATE workers SET last_heartbeat_at = now() - interval '2 minutes' WHERE id = 'desktop'")
        .execute(&env.pool)
        .await
        .unwrap();
    sidefleet_server::db::reap_orphaned_runs(&env.pool, 90).await.unwrap();
    assert_eq!(api.get_run(moved.id).await.status, RunStatus::Failed);
    let thread: Vec<Run> = api.dev(reqwest::Method::GET, &format!("/v1/runs/{lost}/thread")).send().await.unwrap().json().await.unwrap();
    assert_eq!(thread.len(), 2);

    env.teardown().await;
}

#[tokio::test]
async fn a_failover_run_lost_to_a_worker_restart_moves_unless_a_stop_was_pending() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    let create = |task: &'static str| {
        api.dev(reqwest::Method::POST, "/v1/runs")
            .json(&json!({"task": task, "model": "claude-sonnet-5-5", "repo": "r", "failover": true}))
            .send()
    };
    let moving: Uuid = create("keep going").await.unwrap().json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let stopping: Uuid = create("stop me").await.unwrap().json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    api.claim("w", &["r"]).await.unwrap();
    api.stop(stopping).await;
    api.register("w", &["r"]).await; // restarted with nothing running

    assert_eq!(api.get_run(stopping).await.status, RunStatus::Stopped);
    let thread: Vec<Run> = api.dev(reqwest::Method::GET, &format!("/v1/runs/{stopping}/thread")).send().await.unwrap().json().await.unwrap();
    assert_eq!(thread.len(), 1);

    assert_eq!(api.get_run(moving).await.status, RunStatus::Failed);
    let thread: Vec<Run> = api.dev(reqwest::Method::GET, &format!("/v1/runs/{moving}/thread")).send().await.unwrap().json().await.unwrap();
    assert_eq!(thread.len(), 2);
    assert_eq!(thread[1].moved_from, Some(moving));
    // Nothing was pushed: the replacement starts from the repository, not a branch.
    assert_eq!(thread[1].resume_from_branch, None);
    assert!(thread[1].task.contains("No work was pushed"));
    assert!(api.claim("w", &["r"]).await.is_none());
    // After the grace period the machine that lost it may continue it after all.
    sqlx::query("UPDATE runs SET created_at = now() - interval '11 minutes' WHERE id = $1")
        .bind(thread[1].id)
        .execute(&env.pool)
        .await
        .unwrap();
    assert_eq!(api.claim("w", &["r"]).await.unwrap().id, thread[1].id);

    env.teardown().await;
}


async fn send_message(api: &Api, run: Uuid, task: &str) -> Uuid {
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{run}/messages")).json(&json!({"task": task})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED, "{}", r.text().await.unwrap());
    r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn messages_queue_behind_a_running_one_and_resume_its_session() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w1", &["r"]).await;
    api.register("w2", &["r"]).await;

    let (_, v) = api.create_run("first", "r").await;
    let root: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Queued while the root has not even started: no session or machine yet.
    let m1 = send_message(&api, root, "second").await;
    let m1run = api.get_run(m1).await;
    assert_eq!(m1run.status, RunStatus::Queued);
    assert_eq!(m1run.resume_session_id, None);
    assert_eq!(m1run.pinned_worker_id, None);

    assert_eq!(api.claim("w1", &["r"]).await.unwrap().id, root);
    assert!(api.claim("w1", &["r"]).await.is_none(), "a message must wait for the one before it");
    api.post_events("w1", root, json!([{"kind": "agent_init", "payload": {"session_id": "s1"}}])).await;
    let m2 = send_message(&api, root, "third").await;

    let list: Vec<RunSummary> = api.dev(reqwest::Method::GET, "/v1/runs").send().await.unwrap().json().await.unwrap();
    assert_eq!(list.iter().find(|s| s.run.id == root).unwrap().thread_status, RunStatus::Running, "the running message wins over queued ones");

    // Send now: stop the running message; the next one continues its session on the same machine.
    api.stop(root).await;
    api.finish("w1", root, "stopped", json!({})).await;
    assert!(api.claim("w2", &["r"]).await.is_none(), "the queued message must stay on the machine with the session");
    let c = api.claim("w1", &["r"]).await.expect("next message claimable once the previous finished");
    assert_eq!(c.id, m1);
    assert_eq!(c.resume_session_id.as_deref(), Some("s1"));
    assert_eq!(api.get_run(m1).await.pinned_worker_id.as_deref(), Some("w1"));
    assert!(api.claim("w1", &["r"]).await.is_none());
    api.post_events("w1", m1, json!([{"kind": "result", "payload": {"session_id": "s2", "result": "ok"}}])).await;
    api.finish("w1", m1, "succeeded", json!({})).await;
    let c = api.claim("w1", &["r"]).await.unwrap();
    assert_eq!(c.id, m2);
    assert_eq!(c.resume_session_id.as_deref(), Some("s2"), "resumes the latest session, settled at claim time");
    api.finish("w1", m2, "succeeded", json!({})).await;

    // A queued message behind a run that never started a session fails instead of starting fresh.
    let (_, v) = api.create_run("other", "r").await;
    let root2: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let n1 = send_message(&api, root2, "follow").await;
    assert_eq!(api.claim("w1", &["r"]).await.unwrap().id, root2);
    api.finish("w1", root2, "failed", json!({"error": "boom"})).await;
    assert!(api.claim("w1", &["r"]).await.is_none());
    let n1run = api.get_run(n1).await;
    assert_eq!(n1run.status, RunStatus::Failed);
    assert!(n1run.result.unwrap()["error"].as_str().unwrap().contains("nothing to continue"));

    // A finished thread without any session still refuses replies.
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{root2}/messages")).json(&json!({"task": "x"})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);

    env.teardown().await;
}

#[tokio::test]
async fn tool_result_images_are_stored_apart_and_served_to_the_page() {
    use base64::Engine;
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w1", &["r"]).await;
    let (_, v) = api.create_run("screenshot please", "r").await;
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    api.claim("w1", &["r"]).await.unwrap();

    // 3 MB: past axum's default body limit, which the events route raises.
    let png: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let (status, _) = api.post_events("w1", id, json!([
        {"kind": "tool_use", "payload": {"id": "t1", "name": "Read", "summary": "shot.png"}},
        {"kind": "tool_result", "payload": {"tool_use_id": "t1", "content": "[image]", "images": [
            {"media_type": "image/png", "data": b64},
            {"media_type": "image/svg+xml", "data": "PHN2Zz4="},
        ]}},
    ])).await;
    assert_eq!(status, StatusCode::OK);
    api.finish("w1", id, "succeeded", json!({})).await;

    let mut sse = Sse::open(&api, id, None).await;
    let mut refs = Vec::new();
    while let Some(ev) = sse.next_event(Duration::from_secs(5)).await {
        if ev.kind == "tool_result" {
            refs = ev.payload["images"].as_array().cloned().unwrap_or_default();
        }
    }
    assert_eq!(refs.len(), 1, "the SVG is dropped: {refs:?}");
    assert!(refs[0].get("data").is_none(), "image data must not be stored in the event");
    assert_eq!(refs[0]["media_type"], "image/png");
    assert_eq!(refs[0]["bytes"], 3_000_000);

    let img = refs[0]["id"].as_str().unwrap();
    let r = api.dev(reqwest::Method::GET, &format!("/v1/images/{img}")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()["content-type"], "image/png");
    assert_eq!(r.bytes().await.unwrap().as_ref(), png.as_slice());
    let r = Client::new().get(format!("{}/v1/images/{img}", env.base_url)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = api.dev(reqwest::Method::GET, &format!("/v1/images/{}", Uuid::new_v4())).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    env.teardown().await;
}

#[tokio::test]
async fn images_attached_to_tasks_and_replies_reach_the_worker() {
    use base64::Engine;
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w1", &["r"]).await;
    let png = vec![137u8, 80, 78, 71, 1, 2, 3];
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let body = |images: Value| json!({"task": "look", "model": "claude-sonnet-5-5", "repo": "r", "images": images});

    for (images, why) in [
        (json!([{"media_type": "image/svg+xml", "data": "PHN2Zz4="}]), "not a raster image"),
        (json!([{"media_type": "image/png", "data": "not base64!"}]), "not base64"),
        (json!(vec![json!({"media_type": "image/png", "data": b64}); 9]), "too many"),
    ] {
        let r = api.dev(reqwest::Method::POST, "/v1/runs").json(&body(images)).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{why}");
    }

    let id = create_id(&api, body(json!([{"media_type": "image/png", "data": b64}]))).await;
    let run = api.claim("w1", &["r"]).await.unwrap();
    assert_eq!(run.id, id);
    assert_eq!(run.images.len(), 1);
    assert_eq!(run.images[0].media_type, "image/png");
    assert_eq!(api.get_run(id).await.images, run.images, "the page sees them too");
    let r = api.worker(reqwest::Method::GET, &format!("/v1/worker/images/{}", run.images[0].id)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.bytes().await.unwrap().as_ref(), png.as_slice());

    // A reply carries its own images.
    api.post_events("w1", id, json!([{"kind": "agent_init", "payload": {"model": "claude-sonnet-5-5", "session_id": "s1"}}])).await;
    api.finish("w1", id, "succeeded", json!({"session_id": "s1"})).await;
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{id}/messages"))
        .json(&json!({"task": "and this one?", "images": [{"media_type": "image/png", "data": b64}, {"media_type": "image/png", "data": b64}]}))
        .send().await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let next = api.claim("w1", &["r"]).await.unwrap();
    assert_eq!(next.images.len(), 2);
    assert_ne!(next.images[0].id, run.images[0].id);

    env.teardown().await;
}

#[tokio::test]
async fn a_queued_message_follows_a_failover_run_to_its_new_machine() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("laptop", &["r"]).await;
    api.register("desktop", &["r"]).await;
    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "fix it", "model": "claude-sonnet-5-5", "repo": "r", "failover": true}))
        .send()
        .await
        .unwrap();
    let root: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(api.claim("laptop", &["r"]).await.unwrap().id, root);
    api.post_events("laptop", root, json!([{"kind": "agent_init", "payload": {"session_id": "on-laptop"}}])).await;
    let queued = send_message(&api, root, "and add a test").await;

    // The laptop drops; the run moves. The queued message waits for the moved run, not the dead machine.
    sqlx::query("UPDATE workers SET last_heartbeat_at = now() - interval '2 minutes' WHERE id = 'laptop'").execute(&env.pool).await.unwrap();
    sidefleet_server::db::reap_orphaned_runs(&env.pool, 90).await.unwrap();
    let moved = api.claim("desktop", &["r"]).await.expect("desktop takes the moved run");
    assert_ne!(moved.id, queued);
    assert!(moved.moved_from.is_some());
    assert!(api.claim("desktop", &["r"]).await.is_none(), "the message waits for the moved run");
    api.post_events("desktop", moved.id, json!([{"kind": "agent_init", "payload": {"session_id": "on-desktop"}}])).await;
    api.finish("desktop", moved.id, "succeeded", json!({})).await;

    let c = api.claim("desktop", &["r"]).await.expect("then the message runs where the thread now lives");
    assert_eq!(c.id, queued);
    assert_eq!(c.resume_session_id.as_deref(), Some("on-desktop"));
    env.teardown().await;
}

#[tokio::test]
async fn restores_go_only_to_workers_that_can_run_them_and_need_a_snapshot() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    let restore = |id: Uuid| api.dev(reqwest::Method::POST, &format!("/v1/runs/{id}/restore")).send();
    let before = "0123456789abcdef0123456789abcdef01234567";

    let (_, body) = api.create_run("task", "r").await;
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(restore(id).await.unwrap().status(), StatusCode::CONFLICT, "busy while queued");
    api.claim("w", &["r"]).await.unwrap();
    api.post_events("w", id, json!([{"kind": "agent_init", "payload": {"session_id": "s1"}}])).await;
    api.finish("w", id, "succeeded", json!({})).await;
    let r = restore(id).await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert!(r.text().await.unwrap().contains("no snapshot"));

    // With a snapshot: a restore run is queued, pinned to the machine, carrying the session.
    let (_, body) = api.create_run("task 2", "r").await;
    let id2: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    api.post_events("w", id2, json!([
        {"kind": "agent_init", "payload": {"session_id": "s2"}},
        {"kind": "snapshot", "payload": {"before": before, "after": before, "files": []}},
    ])).await;
    api.finish("w", id2, "succeeded", json!({})).await;
    let r = restore(id2).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let rid: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let run = api.get_run(rid).await;
    assert_eq!(run.kind, "restore");
    assert_eq!(run.restore_to.as_deref(), Some(before));
    assert_eq!(run.parent_id, Some(id2));
    assert_eq!(run.pinned_worker_id.as_deref(), Some("w"));
    assert_eq!(run.session_id.as_deref(), Some("s2"));

    // A worker that doesn't say it can restore never gets it; one that does, does.
    assert!(api.claim("w", &["r"]).await.is_none(), "older worker must not run a restore as a task");
    let r = api
        .worker(reqwest::Method::POST, "/v1/worker/claim")
        .json(&json!({"worker_id": "w", "repos": ["r"], "busy_repos": [], "agents": ["claude"], "kinds": ["agent", "restore"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.json::<Run>().await.unwrap().id, rid);
    api.finish("w", rid, "succeeded", json!({})).await;

    // A follow-up after the restore resumes the same session.
    let r = api.dev(reqwest::Method::POST, &format!("/v1/runs/{id2}/messages")).json(&json!({"task": "next"})).send().await.unwrap();
    let f: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(api.get_run(f).await.resume_session_id.as_deref(), Some("s2"));

    // In-place tasks can't be restored.
    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "inplace", "model": "claude-haiku-5-5", "repo": "r", "isolation": "checkout"}))
        .send()
        .await
        .unwrap();
    let ip: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let claimed = api
        .worker(reqwest::Method::POST, "/v1/worker/claim")
        .json(&json!({"worker_id": "w", "repos": ["r"], "busy_repos": [], "agents": ["claude"], "kinds": ["agent", "restore"]}))
        .send()
        .await
        .unwrap()
        .json::<Run>()
        .await
        .unwrap();
    // The queued follow-up is older; finish it first, then the in-place task.
    assert_eq!(claimed.id, f);
    api.finish("w", f, "succeeded", json!({})).await;
    api.claim("w", &["r"]).await.unwrap();
    api.post_events("w", ip, json!([{"kind": "snapshot", "payload": {"before": before}}])).await;
    api.finish("w", ip, "succeeded", json!({})).await;
    let r = restore(ip).await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert!(r.text().await.unwrap().contains("in the checkout"));

    // A message whose agent never started a session can still be restored: a restore starts no agent.
    let (_, body) = api.create_run("task 3", "r").await;
    let id3: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    api.post_events("w", id3, json!([{"kind": "snapshot", "payload": {"before": before, "after": before}}])).await;
    api.finish("w", id3, "failed", json!({"error": "agent crashed"})).await;
    let r = restore(id3).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let rid3: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let r = api
        .worker(reqwest::Method::POST, "/v1/worker/claim")
        .json(&json!({"worker_id": "w", "repos": ["r"], "busy_repos": [], "agents": ["claude"], "kinds": ["agent", "restore"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.json::<Run>().await.unwrap().id, rid3, "not failed for having no session to continue");
    env.teardown().await;
}

#[tokio::test]
async fn forks_start_a_thread_with_any_agent_where_the_files_are() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    // `w` has only Claude Code; `w2` has Codex too.
    for (id, agents) in [("w", json!({"claude": {"version": "1", "signed_in": true}})), ("w2", json!({"claude": {"version": "1", "signed_in": true}, "codex": {"version": "1"}}))] {
        let r = api
            .worker(reqwest::Method::POST, "/v1/worker/register")
            .json(&json!({"id": id, "display_name": id, "capabilities": {"repos": [{"name": "r", "path": "/tmp/r"}], "agents": agents}, "active_runs": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
    let fork = |id: Uuid, body: Value| api.dev(reqwest::Method::POST, &format!("/v1/runs/{id}/fork")).json(&body).send();
    let claim = |worker: &'static str, kinds: Value| {
        api.worker(reqwest::Method::POST, "/v1/worker/claim")
            .json(&json!({"worker_id": worker, "repos": ["r"], "busy_repos": [], "agents": ["claude", "codex"], "kinds": kinds}))
            .send()
    };
    let after = "89abcdef0123456789abcdef0123456789abcdef";

    let (_, body) = api.create_run("build the parser", "r").await;
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let r = fork(id, json!({"task": "x", "agent": "codex"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT, "not while it is queued");
    assert_eq!(claim("w", json!(["agent"])).await.unwrap().json::<Run>().await.unwrap().id, id);
    api.post_events("w", id, json!([
        {"kind": "assistant_text", "payload": {"text": "Parser done."}},
        {"kind": "snapshot", "payload": {"before": after, "after": after, "files": []}},
    ])).await;
    api.finish("w", id, "succeeded", json!({})).await;

    // Validation: agent, model, empty message.
    assert_eq!(fork(id, json!({"task": "x", "agent": "gemini"})).await.unwrap().status(), StatusCode::BAD_REQUEST);
    assert_eq!(fork(id, json!({"task": "x", "agent": "claude", "model": "gpt-fake"})).await.unwrap().status(), StatusCode::BAD_REQUEST);
    assert_eq!(fork(id, json!({"task": " ", "agent": "codex"})).await.unwrap().status(), StatusCode::BAD_REQUEST);
    // The files are on `w`, which has no Codex.
    let r = fork(id, json!({"task": "review it", "agent": "codex", "model": "gpt-fake"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert!(r.text().await.unwrap().contains("w doesn't have codex"));

    // Or on another machine, chosen: there it starts from that machine's HEAD, without the files.
    let r = fork(id, json!({"task": "review it", "agent": "codex", "worker_id": "w2"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let elsewhere = api.get_run(r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap()).await;
    assert_eq!((elsewhere.pinned_worker_id.as_deref(), elsewhere.fork_commit.as_deref()), (Some("w2"), None));
    assert!(elsewhere.context.unwrap().contains("Parser done."));
    api.stop(elsewhere.id).await;
    assert_eq!(fork(id, json!({"task": "x", "agent": "codex", "worker_id": "nope"})).await.unwrap().status(), StatusCode::BAD_REQUEST);
    // Choosing the machine with the files is the same as not choosing.
    let r = fork(id, json!({"task": "x", "agent": "claude", "model": "claude-opus-5-5", "worker_id": "w"})).await.unwrap();
    let same = api.get_run(r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap()).await;
    assert_eq!((same.pinned_worker_id.as_deref(), same.fork_commit.as_deref()), (Some("w"), Some(after)));
    api.stop(same.id).await;

    // With an agent that machine has: a new thread, pinned there, starting from the snapshot.
    let r = fork(id, json!({"task": "add tests", "agent": "claude", "model": "claude-opus-5-5", "permission_mode": "plan"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let fid: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let f = api.get_run(fid).await;
    assert_eq!((f.parent_id, f.forked_from), (None, Some(id)));
    assert_eq!(f.model, "claude-opus-5-5");
    assert_eq!(f.permission_mode.as_deref(), Some("plan"));
    assert_eq!(f.isolation, "worktree");
    assert_eq!(f.pinned_worker_id.as_deref(), Some("w"));
    assert_eq!(f.fork_commit.as_deref(), Some(after));
    let ctx = f.context.unwrap();
    assert!(ctx.contains("## User\nbuild the parser") && ctx.contains("Parser done."), "{ctx}");
    assert_eq!(f.task, "add tests");
    // It is listed as a thread, without the transcript.
    let list: Vec<Value> = api.dev(reqwest::Method::GET, "/v1/runs").send().await.unwrap().json().await.unwrap();
    let mine = list.iter().find(|s| s["id"] == fid.to_string()).expect("fork listed");
    assert!(mine.get("context").is_none());

    // Workers that don't say they can fork never get it.
    assert_eq!(claim("w", json!(["agent", "restore"])).await.unwrap().status(), StatusCode::NO_CONTENT);
    assert_eq!(claim("w", json!(["agent", "restore", "fork"])).await.unwrap().json::<Run>().await.unwrap().id, fid);
    api.finish("w", fid, "succeeded", json!({})).await;

    // A message without a snapshot forks from the repo's HEAD on any machine with the agent.
    let (_, body) = api.create_run("question", "r").await;
    let q: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    claim("w", json!(["agent"])).await.unwrap();
    api.finish("w", q, "succeeded", json!({})).await;
    let r = fork(q, json!({"task": "with codex", "agent": "codex"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let f2: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let f2run = api.get_run(f2).await;
    assert_eq!((f2run.pinned_worker_id, f2run.fork_commit), (None, None));
    env.teardown().await;
}

#[tokio::test]
async fn a_thread_can_switch_agents_and_the_new_one_gets_the_conversation() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    // `w` has Claude Code and Codex; `solo` only Claude Code.
    for (id, agents) in [("w", json!({"claude": {"version": "1", "signed_in": true}, "codex": {"version": "1"}})), ("solo", json!({"claude": {"version": "1", "signed_in": true}}))] {
        let r = api
            .worker(reqwest::Method::POST, "/v1/worker/register")
            .json(&json!({"id": id, "display_name": id, "capabilities": {"repos": [{"name": id, "path": "/tmp/r"}], "agents": agents}, "active_runs": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
    let api = &api;
    let send = |id: Uuid, body: Value| api.dev(reqwest::Method::POST, &format!("/v1/runs/{id}/messages")).json(&body).send();
    let claim = |worker: &'static str, kinds: Value| async move {
        let r = api
            .worker(reqwest::Method::POST, "/v1/worker/claim")
            .json(&json!({"worker_id": worker, "repos": [worker], "busy_repos": [], "agents": ["claude", "codex"], "kinds": kinds}))
            .send()
            .await
            .unwrap();
        if r.status() == StatusCode::NO_CONTENT { None } else { Some(r.json::<Run>().await.unwrap()) }
    };
    let id_of = |v: Value| -> Uuid { v["id"].as_str().unwrap().parse().unwrap() };
    let all = json!(["agent", "restore", "fork"]);

    let (_, body) = api.create_run("plan the parser", "w").await;
    let root = id_of(body);
    assert_eq!(claim("w", all.clone()).await.unwrap().id, root);
    api.post_events("w", root, json!([
        {"kind": "agent_init", "payload": {"session_id": "sess-claude"}},
        {"kind": "assistant_text", "payload": {"text": "Plan: a recursive descent parser."}},
    ])).await;
    api.finish("w", root, "succeeded", json!({})).await;

    // Validation: unknown agent, a model the agent doesn't take.
    assert_eq!(send(root, json!({"task": "x", "agent": "gemini"})).await.unwrap().status(), StatusCode::BAD_REQUEST);
    assert_eq!(send(root, json!({"task": "x", "agent": "claude", "model": "gpt-fake"})).await.unwrap().status(), StatusCode::BAD_REQUEST);

    // Switch to Codex: no session to resume; same machine; its default model.
    let r = send(root, json!({"task": "build it", "agent": "codex"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let c1 = id_of(r.json().await.unwrap());
    let run = api.get_run(c1).await;
    assert_eq!((run.agent, run.model.as_str(), run.parent_id), (Agent::Codex, "", Some(root)));
    assert_eq!((run.resume_session_id, run.pinned_worker_id.as_deref()), (None, Some("w")));
    // Claimed, it carries the conversation so far.
    let c = claim("w", all.clone()).await.unwrap();
    assert_eq!((c.id, c.resume_session_id), (c1, None));
    let ctx = c.context.unwrap();
    assert!(ctx.contains("## User\nplan the parser") && ctx.contains("recursive descent"), "{ctx}");
    api.post_events("w", c1, json!([
        {"kind": "agent_init", "payload": {"session_id": "sess-codex"}},
        {"kind": "assistant_text", "payload": {"text": "Built parser.rs."}},
    ])).await;
    api.finish("w", c1, "succeeded", json!({})).await;

    // Without an agent the thread carries on with Codex and its session.
    let c2 = send_message(api, root, "add tests").await;
    let run = api.get_run(c2).await;
    assert_eq!((run.agent, run.resume_session_id.as_deref()), (Agent::Codex, Some("sess-codex")));
    assert!(claim("w", all.clone()).await.unwrap().context.is_none());
    api.finish("w", c2, "succeeded", json!({})).await;

    // Back to Claude: its old session misses Codex's turns, so it gets a transcript instead,
    // and the model it used before. Queued behind a running message, the transcript is written
    // when it is claimed, so it includes that message too.
    let busy = send_message(api, root, "one more thing").await;
    assert_eq!(claim("w", all.clone()).await.unwrap().id, busy);
    let r = send(root, json!({"task": "review it", "agent": "claude"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let back = id_of(r.json().await.unwrap());
    let run = api.get_run(back).await;
    assert_eq!((run.agent, run.model.as_str(), run.resume_session_id), (Agent::Claude, "claude-sonnet-5-5", None));
    api.post_events("w", busy, json!([{"kind": "assistant_text", "payload": {"text": "Did the thing."}}])).await;
    api.finish("w", busy, "succeeded", json!({})).await;
    // A worker from before forks can't pass the transcript on: the message fails with a reason.
    let c = claim("w", json!(["agent"])).await;
    assert!(c.is_none());
    assert_eq!(api.get_run(back).await.status, RunStatus::Failed);
    let r = send(root, json!({"task": "review it", "agent": "claude"})).await.unwrap();
    let back = id_of(r.json().await.unwrap());
    let c = claim("w", all.clone()).await.unwrap();
    assert_eq!((c.id, c.agent, c.resume_session_id), (back, Agent::Claude, None));
    let ctx = c.context.unwrap();
    assert!(ctx.contains("(with Claude Code, then Codex)") && ctx.contains("## Codex\nBuilt parser.rs.") && ctx.contains("Did the thing."), "{ctx}");
    assert_eq!(ctx.matches("## User\nreview it").count(), 1, "the failed try, not the message itself");
    api.post_events("w", back, json!([{"kind": "agent_init", "payload": {"session_id": "sess-claude-2"}}])).await;
    api.finish("w", back, "succeeded", json!({})).await;
    // And Claude carries on with its new session.
    let next = send_message(api, root, "thanks").await;
    assert_eq!(api.get_run(next).await.resume_session_id.as_deref(), Some("sess-claude-2"));
    api.stop(next).await;

    // A machine without the agent can't switch to it: the thread's files are there.
    let (_, body) = api.create_run("hello", "solo").await;
    let solo = id_of(body);
    claim("solo", all.clone()).await.unwrap();
    api.post_events("solo", solo, json!([{"kind": "agent_init", "payload": {"session_id": "s"}}])).await;
    api.finish("solo", solo, "succeeded", json!({})).await;
    let r = send(solo, json!({"task": "x", "agent": "codex"})).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert!(r.text().await.unwrap().contains("solo doesn't have codex"));
    env.teardown().await;
}

#[tokio::test]
async fn threads_can_be_archived_and_come_back_with_a_new_message() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w1", &["r"]).await;
    let (_, body) = api.create_run("add a login page", "r").await;
    let root: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(api.claim("w1", &["r"]).await.unwrap().id, root);
    api.post_events("w1", root, json!([{"kind": "agent_init", "payload": {"session_id": "sess-1"}}])).await;
    api.finish("w1", root, "succeeded", json!({})).await;
    let f1 = send_message(&api, root, "now a logout button").await;
    api.stop(f1).await;

    let list = |archived: bool| {
        let api = &api;
        async move {
            let v: Vec<RunSummary> = api.dev(reqwest::Method::GET, &format!("/v1/runs?archived={archived}")).send().await.unwrap().json().await.unwrap();
            v.into_iter().map(|s| s.run.id).collect::<Vec<_>>()
        }
    };
    let archive = |id: Uuid, archived: bool| {
        api.dev(reqwest::Method::POST, &format!("/v1/runs/{id}/archive")).json(&json!({"archived": archived})).send()
    };
    // Any run of the thread names it; the root comes back.
    let r = archive(f1, true).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let got = r.json::<Run>().await.unwrap();
    assert_eq!(got.id, root);
    assert!(got.archived_at.is_some());
    assert_eq!(list(false).await, Vec::<Uuid>::new());
    assert_eq!(list(true).await, vec![root]);

    let r = archive(root, false).await.unwrap();
    assert!(r.json::<Run>().await.unwrap().archived_at.is_none());
    assert_eq!(list(false).await, vec![root]);

    // A new message in an archived thread brings it back.
    archive(root, true).await.unwrap();
    let f2 = send_message(&api, root, "and a test").await;
    assert_eq!(list(false).await, vec![root]);
    assert!(list(true).await.is_empty());
    api.stop(f2).await;

    assert_eq!(archive(Uuid::new_v4(), true).await.unwrap().status(), StatusCode::NOT_FOUND);
    env.teardown().await;
}

// ---- tool approvals ("Ask me") ----

async fn ask(api: &Api, worker: &str, run: Uuid, body: Value) -> (StatusCode, Value) {
    let r = api
        .worker(reqwest::Method::POST, &format!("/v1/worker/runs/{run}/approvals?worker_id={worker}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

async fn poll_approval(api: &Api, run: Uuid, approval: &str) -> Value {
    let r = api.worker(reqwest::Method::GET, &format!("/v1/worker/runs/{run}/approvals/{approval}")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    r.json().await.unwrap()
}

async fn decide(api: &Api, run: Uuid, approval: &str, verb: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = api.dev(reqwest::Method::POST, &format!("/v1/runs/{run}/approvals/{approval}/{verb}"));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let r = req.send().await.unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

async fn pending_in_list(api: &Api, root: Uuid) -> i64 {
    let list: Vec<RunSummary> = api.dev(reqwest::Method::GET, "/v1/runs").send().await.unwrap().json().await.unwrap();
    list.iter().find(|r| r.run.id == root).unwrap().pending_approvals
}

/// A running "Ask me" run claimed by worker `w`.
async fn running_ask_run(api: &Api) -> Uuid {
    api.register("w", &["r"]).await;
    let id = create_id(api, json!({"task": "t", "model": "claude-sonnet-5-5", "repo": "r", "permission_mode": "ask"})).await;
    let run = api.claim("w", &["r"]).await.unwrap();
    assert_eq!(run.permission_mode.as_deref(), Some("ask"));
    id
}

#[tokio::test]
async fn an_approval_waits_for_one_decision_and_shows_on_the_page() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    let id = running_ask_run(&api).await;

    let (s, a) = ask(&api, "w", id, json!({"tool_name": "Bash", "tool_use_id": "toolu_1",
        "input": {"command": "curl -H 'Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz' https://example.com", "description": "fetch"}})).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(a["status"], "pending");
    assert!(!a["input"]["command"].as_str().unwrap().contains("sk-abcdef"), "stored input is redacted: {a}");
    let aid = a["id"].as_str().unwrap().to_string();
    assert_eq!(pending_in_list(&api, id).await, 1, "the sidebar shows the thread waiting on you");

    // The page hears about it live, with a one-line summary.
    let evs = sidefleet_server::db::events_since(&env.pool, id, 0).await.unwrap();
    let req = evs.iter().find(|e| e.kind == "approval_requested").expect("approval_requested event");
    assert_eq!(req.payload["id"], aid.as_str());
    assert_eq!(req.payload["tool_use_id"], "toolu_1");
    assert!(req.payload["summary"].as_str().unwrap().starts_with("curl"));

    assert_eq!(poll_approval(&api, id, &aid).await["status"], "pending");

    let (s, a) = decide(&api, id, &aid, "approve", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(a["status"], "approved");
    assert_eq!(a["decided_by"], "developer key");
    // Decided once: neither a second approve nor a deny changes it.
    assert_eq!(decide(&api, id, &aid, "approve", None).await.0, StatusCode::CONFLICT);
    assert_eq!(decide(&api, id, &aid, "deny", None).await.0, StatusCode::CONFLICT);
    assert_eq!(poll_approval(&api, id, &aid).await["status"], "approved");
    assert_eq!(pending_in_list(&api, id).await, 0);

    let kinds: Vec<String> = sidefleet_server::db::events_since(&env.pool, id, 0).await.unwrap().into_iter().map(|e| e.kind).collect();
    assert_eq!(kinds, vec!["status", "status", "approval_requested", "approval_decided"]);

    // A denial tells the agent why: the person's words, or a default.
    let (_, b) = ask(&api, "w", id, json!({"tool_name": "WebFetch", "input": {"url": "https://example.com"}})).await;
    let (s, b) = decide(&api, id, b["id"].as_str().unwrap(), "deny", Some(json!({"message": "use the local docs"}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((b["status"].as_str(), b["message"].as_str()), (Some("denied"), Some("use the local docs")));
    let (_, c) = ask(&api, "w", id, json!({"tool_name": "Bash", "input": {"command": "rm -rf build"}})).await;
    let (_, c) = decide(&api, id, c["id"].as_str().unwrap(), "deny", None).await;
    assert!(c["message"].as_str().unwrap().contains("denied"));

    // Approvals are listed per run; unknown ones are 404.
    let list: Vec<Value> = api.dev(reqwest::Method::GET, &format!("/v1/runs/{id}/approvals")).send().await.unwrap().json().await.unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(decide(&api, id, &Uuid::new_v4().to_string(), "approve", None).await.0, StatusCode::NOT_FOUND);

    env.teardown().await;
}

#[tokio::test]
async fn an_approval_nobody_answers_in_time_expires() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    let id = running_ask_run(&api).await;

    let (_, a) = ask(&api, "w", id, json!({"tool_name": "Bash", "input": {"command": "make"}, "timeout_secs": 1})).await;
    let (_, b) = ask(&api, "w", id, json!({"tool_name": "Bash", "input": {"command": "make test"}, "timeout_secs": 1})).await;
    tokio::time::sleep(Duration::from_millis(1300)).await;

    // The worker asks after the deadline: expired, with a message for the agent.
    let a = poll_approval(&api, id, a["id"].as_str().unwrap()).await;
    assert_eq!(a["status"], "expired");
    assert!(a["message"].as_str().unwrap().contains("Nobody approved"), "{a}");
    // A late approval on the page does not go through either.
    let (s, _) = decide(&api, id, b["id"].as_str().unwrap(), "approve", None).await;
    assert_eq!(s, StatusCode::GONE);
    assert_eq!(poll_approval(&api, id, b["id"].as_str().unwrap()).await["status"], "expired");
    assert_eq!(pending_in_list(&api, id).await, 0);

    env.teardown().await;
}

#[tokio::test]
async fn approvals_end_with_their_run() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    let id = running_ask_run(&api).await;

    // A stop cancels what the agent was waiting on, so the worker stops waiting.
    let (_, a) = ask(&api, "w", id, json!({"tool_name": "Bash", "input": {"command": "sleep 1"}})).await;
    let aid = a["id"].as_str().unwrap().to_string();
    api.stop(id).await;
    let a = poll_approval(&api, id, &aid).await;
    assert_eq!(a["status"], "cancelled");
    // Nothing new can be asked once a stop is pending.
    assert_eq!(ask(&api, "w", id, json!({"tool_name": "Bash", "input": {}})).await.0, StatusCode::CONFLICT);

    // Finishing cancels any left pending.
    let id2 = create_id(&api, json!({"task": "t2", "model": "claude-sonnet-5-5", "repo": "r", "permission_mode": "ask"})).await;
    api.finish("w", id, "stopped", json!({})).await;
    api.claim("w", &["r"]).await.unwrap();
    let (_, b) = ask(&api, "w", id2, json!({"tool_name": "Bash", "input": {"command": "ls"}})).await;
    let bid = b["id"].as_str().unwrap().to_string();
    let (s, _) = api.finish("w", id2, "succeeded", json!({})).await;
    assert_eq!(s, StatusCode::OK);
    let (s, body) = decide(&api, id2, &bid, "approve", None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    let kinds: Vec<String> = sidefleet_server::db::events_since(&env.pool, id2, 0).await.unwrap().into_iter().map(|e| e.kind).collect();
    assert_eq!(kinds, vec!["status", "status", "approval_requested", "approval_decided", "status"]);

    // Only the run's own worker may ask, and only while it runs.
    assert_eq!(ask(&api, "other", id2, json!({"tool_name": "Bash", "input": {}})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(ask(&api, "w", id2, json!({"tool_name": "Bash", "input": {}})).await.0, StatusCode::CONFLICT);

    env.teardown().await;
}

#[tokio::test]
async fn ask_me_is_for_claude_code() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    let r = api
        .dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "t", "agent": "codex", "model": "", "repo": "r", "permission_mode": "ask"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    env.teardown().await;
}

#[tokio::test]
async fn pull_requests_are_opened_on_the_threads_machine_and_kept_on_the_thread() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    let open = |id: Uuid| api.dev(reqwest::Method::POST, &format!("/v1/runs/{id}/pr")).send();
    let claim_pr = || {
        api.worker(reqwest::Method::POST, "/v1/worker/claim")
            .json(&json!({"worker_id": "w", "repos": ["r"], "busy_repos": [], "agents": ["claude"], "kinds": ["agent", "restore", "fork", "pr"]}))
            .send()
    };

    let (_, body) = api.create_run("Fix the login bug\n\nIt loops forever.", "r").await;
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(open(id).await.unwrap().status(), StatusCode::CONFLICT, "busy while queued");
    api.claim("w", &["r"]).await.unwrap();
    api.post_events("w", id, json!([{"kind": "agent_init", "payload": {"session_id": "s1"}}])).await;
    api.finish("w", id, "succeeded", json!({})).await;

    // Queued on the thread's machine, titled after the task; only a worker that says it can, gets it.
    let r = open(id).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let pid: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    let run = api.get_run(pid).await;
    assert_eq!((run.kind.as_str(), run.parent_id, run.pinned_worker_id.as_deref()), ("pr", Some(id), Some("w")));
    assert!(run.context.as_deref().unwrap().starts_with("Fix the login bug\n\n"));
    assert!(api.claim("w", &["r"]).await.is_none(), "older worker must not run it as a task");
    assert_eq!(claim_pr().await.unwrap().json::<Run>().await.unwrap().id, pid);

    let report = |state: &str| {
        api.worker(reqwest::Method::POST, "/v1/worker/prs")
            .json(&json!({"worker_id": "w", "prs": [{"thread": id, "pr": {"url": "https://github.com/o/r/pull/7", "number": 7, "state": state}}]}))
            .send()
    };
    assert_eq!(report("open").await.unwrap().status(), StatusCode::NO_CONTENT);
    api.finish("w", pid, "succeeded", json!({})).await;
    let listed = |runs: Vec<Value>| runs.into_iter().find(|r| r["id"] == json!(id)).unwrap()["pr"].clone();
    let runs: Vec<Value> = api.dev(reqwest::Method::GET, "/v1/runs").send().await.unwrap().json().await.unwrap();
    assert_eq!(listed(runs), json!({"url": "https://github.com/o/r/pull/7", "number": 7, "state": "open"}));
    assert_eq!(api.get_run(pid).await.task, "Open a pull request");

    // The machine is asked to look again once it's due, not on every heartbeat.
    let hb = || async {
        api.worker(reqwest::Method::POST, "/v1/worker/heartbeat")
            .json(&json!({"id": "w", "capabilities": {"repos": [], "hostname": "w", "os": "linux", "version": "t"}}))
            .send().await.unwrap().json::<Value>().await.unwrap()
    };
    assert!(hb().await.get("check_prs").is_none(), "just reported");
    sqlx::query("UPDATE runs SET pr_checked_at = now() - interval '10 minutes'").execute(&env.pool).await.unwrap();
    assert_eq!(hb().await["check_prs"], json!([{"thread": id, "url": "https://github.com/o/r/pull/7"}]));
    assert!(hb().await.get("check_prs").is_none(), "handed out once");
    report("merged").await.unwrap();
    sqlx::query("UPDATE runs SET pr_checked_at = now() - interval '10 minutes'").execute(&env.pool).await.unwrap();
    assert!(hb().await.get("check_prs").is_none(), "merged ones aren't checked again");
    let runs: Vec<Value> = api.dev(reqwest::Method::GET, "/v1/runs").send().await.unwrap().json().await.unwrap();
    assert_eq!(listed(runs)["state"], "merged");

    // Nonsense is refused.
    let r = api.worker(reqwest::Method::POST, "/v1/worker/prs")
        .json(&json!({"worker_id": "w", "prs": [{"thread": id, "pr": {"url": "javascript:alert(1)", "number": 7, "state": "open"}}]}))
        .send().await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    // In-place tasks have no branch of their own.
    let r = api.dev(reqwest::Method::POST, "/v1/runs")
        .json(&json!({"task": "inplace", "model": "claude-haiku-5-5", "repo": "r", "isolation": "checkout"}))
        .send().await.unwrap();
    let ip: Uuid = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().parse().unwrap();
    api.claim("w", &["r"]).await.unwrap();
    api.finish("w", ip, "succeeded", json!({})).await;
    let r = open(ip).await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert!(r.text().await.unwrap().contains("in the checkout"));
    env.teardown().await;
}

/// A claim held open for up to `wait_secs`, started in the background.
fn claim_waiting(env: &TestEnv, worker: &str, wait_secs: u64) -> tokio::task::JoinHandle<Option<Run>> {
    let (api, worker) = (Api::new(&env.base_url), worker.to_string());
    tokio::spawn(async move {
        let r = api
            .worker(reqwest::Method::POST, "/v1/worker/claim")
            .json(&json!({"worker_id": worker, "repos": ["r"], "busy_repos": [], "wait_secs": wait_secs}))
            .send()
            .await
            .unwrap();
        match r.status() {
            StatusCode::NO_CONTENT => None,
            StatusCode::OK => Some(r.json().await.unwrap()),
            s => panic!("unexpected claim status {s}: {}", r.text().await.unwrap()),
        }
    })
}

#[tokio::test]
async fn a_waiting_claim_takes_work_as_soon_as_it_is_queued() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    api.register("w", &["r"]).await;
    // Well under the claim's 5s recheck, so only a wake-up gets the run there in time.
    let soon = Duration::from_millis(1500);

    // Nothing arrives: 204 once the wait is over, not before.
    let t = std::time::Instant::now();
    assert!(claim_waiting(&env, "w", 2).await.unwrap().is_none());
    assert!(t.elapsed() >= Duration::from_secs(2), "answered after {:?}", t.elapsed());
    assert!(t.elapsed() < Duration::from_secs(4), "answered after {:?}", t.elapsed());

    // A new task.
    let waiting = claim_waiting(&env, "w", 20);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let t = std::time::Instant::now();
    let id = create_id(&api, json!({"task": "go", "model": "claude-sonnet-5-5", "repo": "r"})).await;
    let run = tokio::time::timeout(soon, waiting).await.expect("claimed soon after it was queued").unwrap().unwrap();
    assert_eq!(run.id, id);
    assert!(t.elapsed() < soon);

    // A reply queued behind the running turn: claimable, and claimed, once that turn finishes.
    api.post_events("w", id, json!([{"kind": "agent_init", "payload": {"session_id": "s1"}}])).await;
    let reply = send_message(&api, id, "and a test").await;
    let waiting = claim_waiting(&env, "w", 20);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!waiting.is_finished(), "the reply must wait for the running turn");
    api.finish("w", id, "succeeded", json!({"session_id": "s1"})).await;
    let run = tokio::time::timeout(soon, waiting).await.expect("claimed soon after the turn finished").unwrap().unwrap();
    assert_eq!(run.id, reply);
    assert_eq!(run.resume_session_id.as_deref(), Some("s1"));

    // A reply to a finished thread.
    api.finish("w", reply, "succeeded", json!({"session_id": "s1"})).await;
    let waiting = claim_waiting(&env, "w", 20);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let next = send_message(&api, id, "one more").await;
    let run = tokio::time::timeout(soon, waiting).await.expect("claimed soon after it was sent").unwrap().unwrap();
    assert_eq!(run.id, next);
    env.teardown().await;
}

#[tokio::test]
async fn every_response_carries_a_request_id_for_the_logs() {
    let env = TestEnv::start().await;
    let api = Api::new(&env.base_url);
    let r = api.dev(reqwest::Method::GET, &format!("/v1/runs/{}", Uuid::new_v4())).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let id = r.headers().get("x-request-id").expect("request id").to_str().unwrap();
    assert!(Uuid::parse_str(id).is_ok(), "{id}");
    assert_eq!(r.json::<Value>().await.unwrap()["error"], "run not found");
    // A caller's own id is kept, so a client can tie its logs to ours.
    let r = api.dev(reqwest::Method::GET, "/v1/workers").header("x-request-id", "abc-123").send().await.unwrap();
    assert_eq!(r.headers().get("x-request-id").unwrap(), "abc-123");
}
