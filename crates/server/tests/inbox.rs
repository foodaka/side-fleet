//! The "waiting on you" inbox end to end: tasks enter it when they finish, fail,
//! hit a usage limit or lose their machine, leave it when opened, replied to or
//! dismissed, per viewer; the SSE stream follows along and opted-in browsers get
//! a web push. Needs Postgres (docker compose up -d).

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures::StreamExt;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sidefleet_server::push::{generate_private_key, Vapid, DEFAULT_SUBJECT};
use sidefleet_server::test_support::{TestEnv, DEV_KEY, WORKER_TOKEN};
use tokio::sync::mpsc;

struct Api {
    c: Client,
    base: String,
    /// Session cookie of a signed-in user; the developer key otherwise.
    cookie: Option<String>,
    /// Org to act in (signed-in users).
    org: Option<String>,
}

impl Api {
    fn dev(env: &TestEnv) -> Api {
        Api { c: Client::new(), base: env.base_url.clone(), cookie: None, org: None }
    }
    async fn user(env: &TestEnv, login: &str) -> Api {
        Api { c: Client::new(), base: env.base_url.clone(), cookie: Some(env.github_login(login).await), org: None }
    }
    fn req(&self, m: Method, path: &str) -> reqwest::RequestBuilder {
        let r = self.c.request(m, format!("{}{}", self.base, path));
        let r = match &self.org {
            Some(o) => r.header("x-sidefleet-org", o),
            None => r,
        };
        match &self.cookie {
            Some(c) => r.header("cookie", format!("sf_session={c}")),
            None => r.bearer_auth(DEV_KEY),
        }
    }
    async fn call(&self, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut r = self.req(m, path);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn create_run(&self, task: &str, extra: Value) -> String {
        let mut body = json!({"task": task, "model": "claude-sonnet-5-5", "repo": "proj"});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let (s, v) = self.call(Method::POST, "/v1/runs", Some(body)).await;
        assert_eq!(s, StatusCode::ACCEPTED, "{v}");
        v["id"].as_str().unwrap().to_string()
    }
    async fn reply(&self, id: &str) -> String {
        let (s, v) = self.call(Method::POST, &format!("/v1/runs/{id}/messages"), Some(json!({"task": "and now this"}))).await;
        assert_eq!(s, StatusCode::ACCEPTED, "{v}");
        v["id"].as_str().unwrap().to_string()
    }
    /// (task id, reason) of each inbox item, newest first.
    async fn inbox(&self) -> Vec<(String, String)> {
        let (s, v) = self.call(Method::GET, "/v1/inbox", None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["items"].as_array().unwrap().iter().map(|i| (i["id"].as_str().unwrap().to_string(), i["reason"].as_str().unwrap().to_string())).collect()
    }
    async fn dismiss(&self, id: &str) {
        let (s, v) = self.call(Method::POST, &format!("/v1/inbox/{id}/dismiss"), None).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    }
}

/// A worker on the static token, in the default org; `token` for an org's own.
struct Machine {
    c: Client,
    base: String,
    token: String,
    id: String,
}

impl Machine {
    async fn start(env: &TestEnv, token: &str, id: &str) -> Machine {
        let m = Machine { c: Client::new(), base: env.base_url.clone(), token: token.into(), id: id.into() };
        m.register(&[]).await;
        m
    }
    async fn call(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let r = self.c.post(format!("{}{}", self.base, path)).bearer_auth(&self.token).json(&body).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    /// Register (again), saying which runs are still going; any other run it held is lost.
    async fn register(&self, active: &[&str]) {
        let caps = json!({"repos": [{"name": "proj", "path": "/tmp/proj", "id": ""}], "agents": {"claude": {"version": "1", "signed_in": true}}});
        let (s, v) = self.call("/v1/worker/register", json!({"id": self.id, "display_name": self.id, "capabilities": caps, "active_runs": active})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    async fn claim(&self) -> String {
        let (s, v) = self.call("/v1/worker/claim", json!({"worker_id": self.id, "repos": ["proj"], "busy_repos": [], "agents": ["claude"]})).await;
        assert_eq!(s, StatusCode::OK, "nothing to claim: {v}");
        v["id"].as_str().unwrap().to_string()
    }
    async fn finish(&self, run: &str, status: &str, extra: Value) {
        // Report a session, as an agent does, so the thread can be replied to.
        let (s, _) = self
            .call(&format!("/v1/worker/runs/{run}/events?worker_id={}", self.id), json!({"events": [{"kind": "agent_init", "payload": {"session_id": format!("s-{run}")}}]}))
            .await;
        assert_eq!(s, StatusCode::OK);
        let mut body = json!({"status": status, "result": {"text": "done"}});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let (s, v) = self.call(&format!("/v1/worker/runs/{run}/finish?worker_id={}", self.id), body).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
}

fn ids(items: &[(String, String)]) -> Vec<&str> {
    items.iter().map(|(i, _)| i.as_str()).collect()
}

#[tokio::test]
async fn tasks_enter_the_inbox_when_they_need_you_and_leave_when_handled() {
    let env = TestEnv::start().await;
    let api = Api::dev(&env);
    let m = Machine::start(&env, WORKER_TOKEN, "box").await;
    assert!(api.inbox().await.is_empty());

    // Queued and running: nothing to do yet.
    let a = api.create_run("Task A", json!({})).await;
    assert!(api.inbox().await.is_empty());
    assert_eq!(m.claim().await, a);
    assert!(api.inbox().await.is_empty());

    // Finished: waits for a reply.
    m.finish(&a, "succeeded", json!({})).await;
    assert_eq!(api.inbox().await, vec![(a.clone(), "finished".into())]);
    let (_, v) = api.call(Method::GET, "/v1/inbox", None).await;
    let item = &v["items"][0];
    assert_eq!(item["title"], "Task A");
    assert_eq!(item["label"], "Finished, waiting for your reply");
    assert!(item["since"].is_string() && item["run_id"] == a.as_str());

    // Opening it (the page dismisses what it opens) takes it out, and it stays out across reloads.
    api.dismiss(&a).await;
    assert!(api.inbox().await.is_empty());
    assert!(api.inbox().await.is_empty());

    // A reply is work again; when that finishes the task is back.
    let r = api.reply(&a).await;
    assert!(api.inbox().await.is_empty());
    assert_eq!(m.claim().await, r);
    m.finish(&r, "succeeded", json!({})).await;
    assert_eq!(api.inbox().await, vec![(a.clone(), "finished".into())]);
    // Dismissing by a follow-up's id dismisses its task.
    api.dismiss(&r).await;
    assert!(api.inbox().await.is_empty());

    // Failed.
    let b = api.create_run("Task B", json!({})).await;
    m.claim().await;
    m.finish(&b, "failed", json!({})).await;
    // A usage limit with nowhere else to go.
    let c = api.create_run("Task C", json!({})).await;
    m.claim().await;
    m.finish(&c, "failed", json!({"usage_limit": {}})).await;
    // Stopped by you: not news.
    let d = api.create_run("Task D", json!({})).await;
    api.call(Method::POST, &format!("/v1/runs/{d}/stop"), None).await;
    let items = api.inbox().await;
    assert_eq!(items, vec![(c.clone(), "usage_limit".into()), (b.clone(), "failed".into())], "newest first");

    // Archived tasks drop out.
    api.call(Method::POST, &format!("/v1/runs/{b}/archive"), Some(json!({"archived": true}))).await;
    assert_eq!(ids(&api.inbox().await), [c.as_str()]);

    // Clear all.
    let (s, _) = api.call(Method::POST, "/v1/inbox/dismiss-all", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(api.inbox().await.is_empty());

    // Unknown tasks cannot be dismissed.
    let (s, _) = api.call(Method::POST, &format!("/v1/inbox/{}/dismiss", uuid::Uuid::new_v4()), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_dropped_machine_puts_the_task_in_the_inbox() {
    let env = TestEnv::start().await;
    let api = Api::dev(&env);
    let m = Machine::start(&env, WORKER_TOKEN, "laptop").await;

    // Without failover the run fails when the machine comes back without it.
    let a = api.create_run("Stuck", json!({})).await;
    m.claim().await;
    m.register(&[]).await;
    assert_eq!(api.inbox().await, vec![(a.clone(), "machine_lost".into())]);
    api.dismiss(&a).await;

    // With failover it moves: waiting for another machine is in the inbox, being picked up is not.
    let b = api.create_run("Moves", json!({"failover": true})).await;
    m.claim().await;
    m.register(&[]).await;
    let (_, v) = api.call(Method::GET, "/v1/inbox", None).await;
    assert_eq!(v["items"][0]["id"], b.as_str());
    assert_eq!(v["items"][0]["reason"], "machine_lost");
    assert_eq!(v["items"][0]["detail"], "moved; waiting for another machine");
    let other = Machine::start(&env, WORKER_TOKEN, "desktop").await;
    other.claim().await;
    assert!(api.inbox().await.is_empty());
}

#[tokio::test]
async fn each_person_has_their_own_inbox() {
    let env = TestEnv::start_hosted().await;
    let alice = Api::user(&env, "alice").await;
    let mut carol = Api::user(&env, "carol").await;
    let (_, me) = alice.call(Method::GET, "/v1/me", None).await;
    let org = me["orgs"][0]["id"].as_str().unwrap().to_string();
    let (s, _) = alice.call(Method::POST, &format!("/v1/orgs/{org}/members"), Some(json!({"login": "carol"}))).await;
    assert_eq!(s, StatusCode::OK);
    carol.org = Some(org.clone());
    // Both have looked at the inbox before the task finishes.
    assert!(alice.inbox().await.is_empty());
    assert!(carol.inbox().await.is_empty());

    let (_, tok) = alice.call(Method::POST, &format!("/v1/orgs/{org}/tokens"), Some(json!({"name": "m"}))).await;
    let m = Machine::start(&env, tok["token"].as_str().unwrap(), "m1").await;
    let a = alice.create_run("Shared task", json!({})).await;
    m.claim().await;
    m.finish(&a, "succeeded", json!({})).await;
    assert_eq!(ids(&alice.inbox().await), [a.as_str()]);
    assert_eq!(ids(&carol.inbox().await), [a.as_str()]);

    // Alice opening it does not clear Carol's.
    alice.dismiss(&a).await;
    assert!(alice.inbox().await.is_empty());
    assert_eq!(ids(&carol.inbox().await), [a.as_str()]);

    // The developer key's inbox is the default org's: none of this.
    assert!(Api::dev(&env).inbox().await.is_empty());
}

/// Next `inbox` event's items from an SSE response, within `timeout`.
async fn next_inbox(stream: &mut (impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin), buf: &mut String, timeout: Duration) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        while let Some(i) = buf.find("\n\n") {
            let frame = buf[..i].to_string();
            *buf = buf[i + 2..].to_string();
            if frame.lines().any(|l| l == "event: inbox") {
                let data: String = frame.lines().filter_map(|l| l.strip_prefix("data: ")).collect();
                return Some(serde_json::from_str::<Value>(&data).unwrap()["items"].clone());
            }
        }
        let chunk = tokio::time::timeout_at(deadline, stream.next()).await.ok()??.ok()?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
    }
}

#[tokio::test]
async fn the_inbox_stream_follows_changes() {
    let env = TestEnv::start().await;
    let api = Api::dev(&env);
    let m = Machine::start(&env, WORKER_TOKEN, "box").await;
    let r = api.req(Method::GET, "/v1/inbox/events").send().await.unwrap();
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let mut stream = Box::pin(r.bytes_stream());
    let mut buf = String::new();
    let t = Duration::from_secs(5);
    assert_eq!(next_inbox(&mut stream, &mut buf, t).await.unwrap(), json!([]), "the list right away");

    let a = api.create_run("Streamed", json!({})).await;
    m.claim().await;
    m.finish(&a, "succeeded", json!({})).await;
    let items = next_inbox(&mut stream, &mut buf, t).await.expect("an update when the task finishes");
    assert_eq!(items[0]["id"], a.as_str());
    assert_eq!(items[0]["reason"], "finished");

    api.dismiss(&a).await;
    assert_eq!(next_inbox(&mut stream, &mut buf, t).await.unwrap(), json!([]), "an update when it is dismissed");
}

/// A stand-in push service: records each request's Authorization header and body;
/// `/gone/...` answers 410 like an expired subscription.
async fn fake_push_service() -> (String, mpsc::UnboundedReceiver<(String, String, Vec<u8>)>) {
    use axum::{body::Bytes, extract::OriginalUri, http::HeaderMap, routing::post, Router};
    let (tx, rx) = mpsc::unbounded_channel();
    let app = Router::new().route(
        "/{*path}",
        post(move |uri: OriginalUri, headers: HeaderMap, body: Bytes| {
            let tx = tx.clone();
            async move {
                let auth = headers.get("authorization").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
                assert_eq!(headers["content-encoding"], "aes128gcm");
                let _ = tx.send((uri.path().to_string(), auth, body.to_vec()));
                if uri.path().starts_with("/gone/") { StatusCode::GONE } else { StatusCode::CREATED }
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), rx)
}

/// A browser's push subscription: its secret key, auth secret, and what it sends the server.
fn browser(endpoint: &str) -> (p256::SecretKey, [u8; 16], Value) {
    let secret = p256::SecretKey::random(&mut rand::rngs::OsRng);
    let auth: [u8; 16] = rand::random();
    let sub = json!({"endpoint": endpoint, "keys": {
        "p256dh": URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes()),
        "auth": URL_SAFE_NO_PAD.encode(auth),
    }});
    (secret, auth, sub)
}

#[tokio::test]
async fn opted_in_browsers_get_a_push_with_only_title_and_reason() {
    let key = generate_private_key();
    let env = TestEnv::start_with(|c| c.vapid = Some(Vapid::from_private_key(&key, DEFAULT_SUBJECT).unwrap())).await;
    env.state.inbox.allow_any_push_host(true);
    env.state.inbox.set_push_delay(Duration::from_millis(300));
    let api = Api::dev(&env);
    let m = Machine::start(&env, WORKER_TOKEN, "box").await;
    let (push_base, mut pushes) = fake_push_service().await;

    let (_, v) = api.call(Method::GET, "/v1/push", None).await;
    assert_eq!(v["available"], true);
    let public_key = v["public_key"].as_str().unwrap().to_string();

    // Nobody subscribed: a finished task sends nothing.
    let quiet = api.create_run("Quiet", json!({})).await;
    m.claim().await;
    m.finish(&quiet, "succeeded", json!({})).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(pushes.try_recv().is_err());

    // Only real browser push services are accepted (unless the test allows any host).
    env.state.inbox.allow_any_push_host(false);
    let (s, _) = api.call(Method::POST, "/v1/push/subscriptions", Some(browser("http://127.0.0.1:1/x").2)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    env.state.inbox.allow_any_push_host(true);

    let (secret, auth, sub) = browser(&format!("{push_base}/push/browser-1"));
    let (s, v) = api.call(Method::POST, "/v1/push/subscriptions", Some(sub)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (_, _, gone) = browser(&format!("{push_base}/gone/browser-2"));
    api.call(Method::POST, "/v1/push/subscriptions", Some(gone)).await;

    let secret_task = "Rotate the API key sk-live-123\nand redeploy";
    let a = api.create_run(secret_task, json!({})).await;
    let run = m.claim().await;
    let (s, _) = m
        .call(&format!("/v1/worker/runs/{run}/events?worker_id=box"), json!({"events": [{"kind": "assistant_text", "payload": {"text": "the password is hunter2"}}]}))
        .await;
    assert_eq!(s, StatusCode::OK);
    m.finish(&a, "failed", json!({})).await;

    let mut got = Vec::new();
    for _ in 0..2 {
        got.push(tokio::time::timeout(Duration::from_secs(5), pushes.recv()).await.expect("a push").unwrap());
    }
    got.sort_by(|a, b| b.0.cmp(&a.0)); // /push/… before /gone/…
    let (path, authz, body) = got.remove(0);
    assert_eq!(path, "/push/browser-1");
    assert!(authz.starts_with("vapid t=") && authz.ends_with(&format!(", k={public_key}")), "{authz}");
    let plain = web_push_native::decrypt(body, &secret, web_push_native::Auth::from_slice(&auth)).expect("decrypts with the browser's keys");
    let msg: Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(msg["title"], "Rotate the API key sk-live-123…");
    assert_eq!(msg["reason"], "failed");
    assert_eq!(msg["url"], format!("/#/runs/{a}"));
    let text = String::from_utf8_lossy(&plain);
    assert!(!text.contains("hunter2") && !text.contains("redeploy"), "no agent output, only the title's first line: {text}");

    // The push service said the second browser is gone: it is forgotten.
    let mut n = 0;
    for _ in 0..50 {
        n = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM push_subscriptions").fetch_one(&env.pool).await.unwrap();
        if n == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(n, 1);

    // Opened before the push went out: no push.
    env.state.inbox.set_push_delay(Duration::from_millis(1500));
    let b = api.create_run("Seen quickly", json!({})).await;
    m.claim().await;
    m.finish(&b, "succeeded", json!({})).await;
    api.dismiss(&b).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(pushes.try_recv().is_err(), "a task opened right away is not pushed");

    // Turning it off stops pushes.
    let (s, _) = api.call(Method::DELETE, "/v1/push/subscriptions", Some(json!({"endpoint": format!("{push_base}/push/browser-1")}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions").fetch_one(&env.pool).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn push_is_off_without_a_vapid_key() {
    let env = TestEnv::start().await;
    let api = Api::dev(&env);
    let (_, v) = api.call(Method::GET, "/v1/push", None).await;
    assert_eq!(v, json!({"available": false, "public_key": null}));
    let (_, _, sub) = browser("https://fcm.googleapis.com/fcm/send/x");
    let (s, _) = api.call(Method::POST, "/v1/push/subscriptions", Some(sub)).await;
    assert_eq!(s, StatusCode::CONFLICT);
}
