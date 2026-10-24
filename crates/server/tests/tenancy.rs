//! Hosted, multi-tenant control plane: GitHub sign-in and session cookies,
//! tenant isolation for developers and workers, per-org worker tokens,
//! approve-on-the-page enrollment and plan limits. Needs Postgres
//! (docker compose up -d).

use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sidefleet_server::test_support::{TestEnv, DEV_KEY, WORKER_TOKEN};
use uuid::Uuid;

/// A signed-in browser: sends the session cookie and, optionally, the org to act in.
struct Browser {
    c: Client,
    base: String,
    cookie: String,
    org: Option<String>,
}

impl Browser {
    async fn login(env: &TestEnv, login: &str) -> Browser {
        let cookie = env.github_login(login).await;
        Browser { c: Client::new(), base: env.base_url.clone(), cookie, org: None }
    }
    fn req(&self, m: Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.c.request(m, format!("{}{}", self.base, path)).header("cookie", format!("sf_session={}", self.cookie));
        if let Some(o) = &self.org {
            r = r.header("x-sidefleet-org", o);
        }
        r
    }
    async fn json(&self, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut r = self.req(m, path);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn me(&self) -> Value {
        let (s, v) = self.json(Method::GET, "/v1/me", None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }
    async fn org_id(&self) -> String {
        self.me().await["orgs"][0]["id"].as_str().unwrap().to_string()
    }
    async fn create_run(&self, repo: &str) -> (StatusCode, Value) {
        self.json(Method::POST, "/v1/runs", Some(json!({"task": "do it", "model": "claude-sonnet-5-5", "repo": repo}))).await
    }
    async fn create_token(&self, org: &str, name: &str) -> (StatusCode, Value) {
        self.json(Method::POST, &format!("/v1/orgs/{org}/tokens"), Some(json!({"name": name}))).await
    }
}

/// A worker using one token.
struct Worker {
    c: Client,
    base: String,
    token: String,
}

impl Worker {
    fn new(env: &TestEnv, token: &str) -> Worker {
        Worker { c: Client::new(), base: env.base_url.clone(), token: token.to_string() }
    }
    async fn call(&self, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut r = self.c.request(m, format!("{}{}", self.base, path)).bearer_auth(&self.token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn register(&self, id: &str, repos: &[&str]) -> StatusCode {
        let repos: Vec<Value> = repos.iter().map(|r| json!({"name": r, "path": format!("/tmp/{r}"), "id": ""})).collect();
        self.call(Method::POST, "/v1/worker/register", Some(json!({"id": id, "display_name": id, "capabilities": {"repos": repos}, "active_runs": []})))
            .await
            .0
    }
    async fn heartbeat(&self, id: &str) -> StatusCode {
        self.call(Method::POST, "/v1/worker/heartbeat", Some(json!({"id": id, "capabilities": {}, "active_runs": []}))).await.0
    }
    async fn claim(&self, id: &str, repos: &[&str]) -> (StatusCode, Value) {
        self.call(Method::POST, "/v1/worker/claim", Some(json!({"worker_id": id, "repos": repos, "busy_repos": []}))).await
    }
}

async fn status(c: &Client, req: reqwest::RequestBuilder) -> StatusCode {
    let _ = c;
    req.send().await.unwrap().status()
}

#[tokio::test]
async fn session_cookie_auth_and_personal_orgs() {
    let env = TestEnv::start_hosted().await;
    let c = Client::new();

    // Public: the page learns how to sign in.
    let m: Value = c.get(format!("{}/v1/auth", env.base_url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(m, json!({"github": true, "dev_key": true, "hosted": true}));

    // No cookie, a made-up cookie: not signed in.
    assert_eq!(status(&c, c.get(format!("{}/v1/me", env.base_url))).await, StatusCode::UNAUTHORIZED);
    assert_eq!(status(&c, c.get(format!("{}/v1/runs", env.base_url))).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        status(&c, c.get(format!("{}/v1/runs", env.base_url)).header("cookie", "sf_session=forged")).await,
        StatusCode::UNAUTHORIZED
    );

    // The callback refuses a state that does not match this browser's cookie.
    let r = Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
        .get(format!("{}/auth/github/callback?code=mallory&state=abc", env.base_url))
        .header("cookie", "sf_oauth_state=xyz")
        .send().await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert!(r.headers().get("set-cookie").is_none());

    // Signing in creates the user and a personal org they own.
    let alice = Browser::login(&env, "alice").await;
    let me = alice.me().await;
    assert_eq!(me["user"]["login"], "alice");
    assert_eq!(me["user"]["email"], "alice@example.com");
    let orgs = me["orgs"].as_array().unwrap();
    assert_eq!(orgs.len(), 1);
    assert_eq!(orgs[0]["slug"], "alice");
    assert_eq!(orgs[0]["role"], "owner");
    assert_eq!(orgs[0]["plan"], "free");
    assert_eq!(orgs[0]["limits"], json!({"machines": 2, "parallel_runs": 2}));

    // Signing in again is the same user and the same single org.
    let again = Browser::login(&env, "alice").await;
    assert_eq!(again.me().await["user"]["id"], me["user"]["id"]);
    assert_eq!(again.me().await["orgs"].as_array().unwrap().len(), 1);

    // The cookie works for every developer route.
    assert_eq!(alice.json(Method::GET, "/v1/runs", None).await.0, StatusCode::OK);
    assert_eq!(alice.json(Method::GET, "/v1/workers", None).await.0, StatusCode::OK);

    // An org the user is not in: 403.
    let mut nosy = Browser::login(&env, "nosy").await;
    nosy.org = Some(alice.org_id().await);
    assert_eq!(nosy.json(Method::GET, "/v1/runs", None).await.0, StatusCode::FORBIDDEN);
    nosy.org = Some("default".into());
    assert_eq!(nosy.json(Method::GET, "/v1/runs", None).await.0, StatusCode::FORBIDDEN);

    // Logging out ends the session server-side.
    let (s, _) = again.json(Method::POST, "/auth/logout", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(again.json(Method::GET, "/v1/me", None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(alice.json(Method::GET, "/v1/me", None).await.0, StatusCode::OK, "other sessions stay");

    // An expired session is refused.
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 minute'").execute(&env.pool).await.unwrap();
    assert_eq!(alice.json(Method::GET, "/v1/runs", None).await.0, StatusCode::UNAUTHORIZED);

    // The developer key still works and acts in the default org.
    let v: Value = c.get(format!("{}/v1/me", env.base_url)).bearer_auth(DEV_KEY).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["user"], Value::Null);
    assert_eq!(v["orgs"][0]["slug"], "default");
    assert_eq!(v["orgs"][0]["limits"], Value::Null);

    env.teardown().await;
}

#[tokio::test]
async fn orgs_are_isolated_for_developers_and_workers() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let bob = Browser::login(&env, "bob").await;
    let (a_org, b_org) = (alice.org_id().await, bob.org_id().await);
    assert_ne!(a_org, b_org);

    let (s, a_tok) = alice.create_token(&a_org, "alice laptop").await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, b_tok) = bob.create_token(&b_org, "bob laptop").await;
    let wa = Worker::new(&env, a_tok["token"].as_str().unwrap());
    let wb = Worker::new(&env, b_tok["token"].as_str().unwrap());
    // Same hostname in both orgs: two different machines.
    assert_eq!(wa.register("MacBook-Pro", &["shop"]).await, StatusCode::OK);
    assert_eq!(wb.register("MacBook-Pro", &["shop"]).await, StatusCode::OK);
    let a_workers = alice.json(Method::GET, "/v1/workers", None).await.1;
    assert_eq!(a_workers.as_array().unwrap().len(), 1);

    // Alice queues a run on a repo bob's machine also has.
    let (s, run) = alice.create_run("shop").await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let id = run["id"].as_str().unwrap().to_string();

    // Bob cannot see, read, stream, stop or reply to it.
    assert!(bob.json(Method::GET, "/v1/runs", None).await.1.as_array().unwrap().is_empty());
    for path in [format!("/v1/runs/{id}"), format!("/v1/runs/{id}/thread"), format!("/v1/runs/{id}/events")] {
        assert_eq!(bob.json(Method::GET, &path, None).await.0, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(bob.json(Method::POST, &format!("/v1/runs/{id}/stop"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(bob.json(Method::POST, &format!("/v1/runs/{id}/messages"), Some(json!({"task": "hi"}))).await.0, StatusCode::NOT_FOUND);
    // Nor can the developer key (default org).
    let c = Client::new();
    assert_eq!(status(&c, c.get(format!("{}/v1/runs/{id}", env.base_url)).bearer_auth(DEV_KEY)).await, StatusCode::NOT_FOUND);
    // Nor pin his run to her machine's id in his org namespace and reach hers.
    assert_eq!(bob.json(Method::GET, "/v1/repos", None).await.1, json!({"added": []}));

    // Bob's worker (same id, same repo) cannot claim alice's run; neither can the static token.
    assert_eq!(wb.claim("MacBook-Pro", &["shop"]).await.0, StatusCode::NO_CONTENT);
    let ws = Worker::new(&env, WORKER_TOKEN);
    assert_eq!(ws.register("MacBook-Pro", &["shop"]).await, StatusCode::OK);
    assert_eq!(ws.claim("MacBook-Pro", &["shop"]).await.0, StatusCode::NO_CONTENT);

    // Alice's worker claims it.
    let (s, claimed) = wa.claim("MacBook-Pro", &["shop"]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(claimed["id"], run["id"]);

    // Bob's worker, with the same worker id, cannot post events, read control or finish it.
    let events = json!({"events": [{"kind": "assistant_text", "payload": {"text": "hijacked"}}]});
    assert_eq!(wb.call(Method::POST, &format!("/v1/worker/runs/{id}/events?worker_id=MacBook-Pro"), Some(events.clone())).await.0, StatusCode::NOT_FOUND);
    assert_eq!(wb.call(Method::GET, &format!("/v1/worker/runs/{id}/control"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        wb.call(Method::POST, &format!("/v1/worker/runs/{id}/finish?worker_id=MacBook-Pro"), Some(json!({"status": "failed"}))).await.0,
        StatusCode::NOT_FOUND
    );
    // Alice's worker can.
    assert_eq!(wa.call(Method::POST, &format!("/v1/worker/runs/{id}/events?worker_id=MacBook-Pro"), Some(events)).await.0, StatusCode::OK);
    let (s, done) = wa.call(Method::POST, &format!("/v1/worker/runs/{id}/finish?worker_id=MacBook-Pro"), Some(json!({"status": "succeeded"}))).await;
    assert_eq!(s, StatusCode::OK, "{done}");

    // Bob's own run goes to bob's machine only.
    let (_, brun) = bob.create_run("shop").await;
    assert_eq!(wa.claim("MacBook-Pro", &["shop"]).await.0, StatusCode::NO_CONTENT);
    let (s, bclaimed) = wb.claim("MacBook-Pro", &["shop"]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(bclaimed["id"], brun["id"]);

    // Each sees one thread, their own; machines and tokens stay apart too.
    let a_runs = alice.json(Method::GET, "/v1/runs", None).await.1;
    assert_eq!(a_runs.as_array().unwrap().len(), 1);
    assert_eq!(a_runs[0]["id"], run["id"]);
    assert_eq!(bob.json(Method::GET, &format!("/v1/orgs/{a_org}/tokens"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(bob.json(Method::POST, "/v1/workers/MacBook-Pro/update", None).await.0, StatusCode::ACCEPTED, "his own machine");
    let a_w = &alice.json(Method::GET, "/v1/workers", None).await.1[0];
    assert_eq!(a_w["update_requested"], false, "bob's update request did not touch alice's machine");

    env.teardown().await;
}

#[tokio::test]
async fn worker_tokens_are_shown_once_and_revocation_locks_the_worker_out() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let org = alice.org_id().await;

    let (s, created) = alice.create_token(&org, "  studio mac ").await;
    assert_eq!(s, StatusCode::CREATED);
    let value = created["token"].as_str().unwrap().to_string();
    assert!(value.starts_with("sfw_") && value.len() > 40);
    assert_eq!(created["name"], "studio mac");
    assert_eq!(alice.create_token(&org, "").await.0, StatusCode::BAD_REQUEST);

    // Listing never shows the value again, and nothing in the database holds it.
    let (_, list) = alice.json(Method::GET, &format!("/v1/orgs/{org}/tokens"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(&value));
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM worker_tokens").fetch_one(&env.pool).await.unwrap();
    assert_ne!(stored, value);

    let w = Worker::new(&env, &value);
    assert_eq!(w.register("studio", &["app"]).await, StatusCode::OK);
    assert_eq!(w.heartbeat("studio").await, StatusCode::OK);
    let (_, list) = alice.json(Method::GET, &format!("/v1/orgs/{org}/tokens"), None).await;
    assert!(list[0]["last_used_at"].is_string());

    // A member cannot manage tokens; only owners can.
    let carol = Browser::login(&env, "carol").await;
    let (s, _) = alice.json(Method::POST, &format!("/v1/orgs/{org}/members"), Some(json!({"login": "carol"}))).await;
    assert_eq!(s, StatusCode::OK);
    let mut carol_in_alice = carol;
    carol_in_alice.org = Some(org.clone());
    assert_eq!(carol_in_alice.create_token(&org, "sneaky").await.0, StatusCode::FORBIDDEN);
    assert_eq!(carol_in_alice.json(Method::GET, &format!("/v1/orgs/{org}/tokens"), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(carol_in_alice.json(Method::GET, "/v1/workers", None).await.1.as_array().unwrap().len(), 1, "members see the org's machines");

    // Revoke: the next heartbeat gets 401, and so does registering again.
    let tid = created["id"].as_str().unwrap();
    let (s, revoked) = alice.json(Method::DELETE, &format!("/v1/orgs/{org}/tokens/{tid}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(revoked["revoked_at"].is_string());
    assert_eq!(w.heartbeat("studio").await, StatusCode::UNAUTHORIZED);
    assert_eq!(w.register("studio", &["app"]).await, StatusCode::UNAUTHORIZED);
    assert_eq!(w.claim("studio", &["app"]).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(alice.json(Method::DELETE, &format!("/v1/orgs/{org}/tokens/{}", Uuid::new_v4()), None).await.0, StatusCode::NOT_FOUND);

    // A new org for the team: the switcher's second entry, separate from the personal org.
    let (s, team) = alice.json(Method::POST, "/v1/orgs", Some(json!({"name": "Acme Team"}))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(team["slug"], "acme-team");
    assert_eq!(alice.me().await["orgs"].as_array().unwrap().len(), 2);
    let mut in_team = Browser { c: Client::new(), base: env.base_url.clone(), cookie: alice.cookie.clone(), org: Some("acme-team".into()) };
    assert!(in_team.json(Method::GET, "/v1/workers", None).await.1.as_array().unwrap().is_empty());
    in_team.org = None;
    assert_eq!(in_team.json(Method::GET, "/v1/workers", None).await.1.as_array().unwrap().len(), 1, "personal org by default");

    env.teardown().await;
}

async fn enroll(env: &TestEnv, name: &str) -> Value {
    let r = Client::new()
        .post(format!("{}/v1/enroll", env.base_url))
        .json(&json!({"worker_id": name, "machine_name": name, "capabilities": {"hostname": name, "os": "linux", "repos": [{"name": "app", "path": "/secret/path/app", "id": ""}]}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    r.json().await.unwrap()
}

async fn poll(env: &TestEnv, e: &Value) -> (StatusCode, Value) {
    let r = Client::new().get(format!("{}/v1/enroll/{}", env.base_url, e["id"].as_str().unwrap())).bearer_auth(e["secret"].as_str().unwrap()).send().await.unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn enrollment_approve_deny_and_expiry() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let bob = Browser::login(&env, "bob").await;

    // A machine asks to join and waits.
    let e = enroll(&env, "build-box").await;
    let code = e["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 9);
    assert!(e["approve_url"].as_str().unwrap().ends_with(&format!("/#/machines?code={code}")));
    let (s, p) = poll(&env, &e).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(p["status"], "pending");
    // Polling needs the secret.
    let r = Client::new().get(format!("{}/v1/enroll/{}", env.base_url, e["id"].as_str().unwrap())).bearer_auth("guess").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    // In hosted mode it is in nobody's list until someone enters the code.
    assert!(alice.json(Method::GET, "/v1/enrollments", None).await.1.as_array().unwrap().is_empty());
    let (s, found) = alice.json(Method::POST, "/v1/enrollments/lookup", Some(json!({"code": code.to_lowercase().replace('-', "")}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(found["machine_name"], "build-box");
    assert_eq!(found["capabilities"]["repo_count"], 1);
    assert!(!found.to_string().contains("/secret/path"), "repo paths stay on the machine");
    assert_eq!(alice.json(Method::POST, "/v1/enrollments/lookup", Some(json!({"code": "AAAA-AAAA"}))).await.0, StatusCode::NOT_FOUND);

    // Approving needs the code (the id alone is not enough).
    let eid = e["id"].as_str().unwrap();
    assert_eq!(bob.json(Method::POST, &format!("/v1/enrollments/{eid}/approve"), Some(json!({}))).await.0, StatusCode::NOT_FOUND);
    let (s, approved) = alice.json(Method::POST, &format!("/v1/enrollments/{eid}/approve"), Some(json!({"code": code}))).await;
    assert_eq!(s, StatusCode::OK, "{approved}");
    assert_eq!(approved["status"], "approved");
    // Now it belongs to alice's org: bob cannot touch it even with the code.
    assert_eq!(bob.json(Method::POST, "/v1/enrollments/lookup", Some(json!({"code": code}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(bob.json(Method::POST, &format!("/v1/enrollments/{eid}/deny"), Some(json!({"code": code}))).await.0, StatusCode::NOT_FOUND);

    // The waiting connect gets a per-machine token exactly once.
    let (s, p) = poll(&env, &e).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p["org"]["slug"], "alice");
    let token = p["token"].as_str().unwrap().to_string();
    let (s, p2) = poll(&env, &e).await;
    assert_eq!(s, StatusCode::GONE);
    assert_eq!(p2["status"], "collected");
    assert!(p2.get("token").is_none());
    let w = Worker::new(&env, &token);
    assert_eq!(w.register("build-box", &["app"]).await, StatusCode::OK);
    let workers = alice.json(Method::GET, "/v1/workers", None).await.1;
    assert_eq!(workers[0]["id"], "build-box");
    let tokens = alice.json(Method::GET, &format!("/v1/orgs/{}/tokens", alice.org_id().await), None).await.1;
    assert_eq!(tokens[0]["name"], "build-box");
    assert!(bob.json(Method::GET, "/v1/workers", None).await.1.as_array().unwrap().is_empty());

    // Deny: the connect is told, and gets nothing.
    let d = enroll(&env, "stranger").await;
    let did = d["id"].as_str().unwrap();
    let (s, _) = bob.json(Method::POST, &format!("/v1/enrollments/{did}/deny"), Some(json!({"code": d["code"]}))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, p) = poll(&env, &d).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(p["status"], "denied");
    assert_eq!(bob.json(Method::POST, &format!("/v1/enrollments/{did}/approve"), Some(json!({"code": d["code"]}))).await.0, StatusCode::CONFLICT);

    // Expiry: after ten minutes nobody can approve it and the connect is told.
    let x = enroll(&env, "late").await;
    sqlx::query("UPDATE enrollments SET expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(Uuid::parse_str(x["id"].as_str().unwrap()).unwrap())
        .execute(&env.pool)
        .await
        .unwrap();
    assert_eq!(alice.json(Method::POST, "/v1/enrollments/lookup", Some(json!({"code": x["code"]}))).await.0, StatusCode::NOT_FOUND);
    let xid = x["id"].as_str().unwrap();
    assert_eq!(alice.json(Method::POST, &format!("/v1/enrollments/{xid}/approve"), Some(json!({"code": x["code"]}))).await.0, StatusCode::GONE);
    let (s, p) = poll(&env, &x).await;
    assert_eq!(s, StatusCode::GONE);
    assert_eq!(p["status"], "expired");

    env.teardown().await;
}

#[tokio::test]
async fn single_tenant_enrollment_waits_on_the_machines_page() {
    let env = TestEnv::start().await;
    let dev = |m: Method, p: &str| Client::new().request(m, format!("{}{p}", env.base_url)).bearer_auth(DEV_KEY);
    let m: Value = Client::new().get(format!("{}/v1/auth", env.base_url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(m, json!({"github": false, "dev_key": true, "hosted": false}));
    // No GitHub configured: the sign-in route says so instead of redirecting.
    assert_eq!(Client::new().get(format!("{}/auth/github/login", env.base_url)).send().await.unwrap().status(), StatusCode::NOT_FOUND);

    let e = enroll(&env, "nas").await;
    let list: Value = dev(Method::GET, "/v1/enrollments").send().await.unwrap().json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1, "listed without a code");
    assert_eq!(list[0]["code"], e["code"]);
    let r = dev(Method::POST, &format!("/v1/enrollments/{}/approve", e["id"].as_str().unwrap())).json(&json!({})).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let (s, p) = poll(&env, &e).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p["org"]["slug"], "default");
    let w = Worker::new(&env, p["token"].as_str().unwrap());
    assert_eq!(w.register("nas", &["app"]).await, StatusCode::OK);
    let workers: Value = dev(Method::GET, "/v1/workers").send().await.unwrap().json().await.unwrap();
    assert_eq!(workers[0]["id"], "nas");
    // The setup endpoint still hands out the static token in the default org.
    let setup: Value = dev(Method::GET, "/v1/setup").send().await.unwrap().json().await.unwrap();
    assert_eq!(setup["worker_token"], WORKER_TOKEN);
    assert_eq!(setup["hosted"], false);

    env.teardown().await;
}

#[tokio::test]
async fn plan_limits_are_enforced() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let org = alice.org_id().await;
    let (_, t) = alice.create_token(&org, "fleet").await;
    let w = Worker::new(&env, t["token"].as_str().unwrap());

    // Free plan: two machines. A third is refused with 402; re-registering a known one is fine.
    assert_eq!(w.register("one", &["app"]).await, StatusCode::OK);
    assert_eq!(w.register("two", &["app"]).await, StatusCode::OK);
    let (s, body) = w.call(Method::POST, "/v1/worker/register", Some(json!({"id": "three", "display_name": "three", "capabilities": {}, "active_runs": []}))).await;
    assert_eq!(s, StatusCode::PAYMENT_REQUIRED);
    assert!(body["error"].as_str().unwrap().contains("2 machines"));
    assert_eq!(w.register("one", &["app"]).await, StatusCode::OK);
    // Approving a third machine on the page is refused the same way.
    let e = enroll(&env, "three").await;
    let (s, body) = alice.json(Method::POST, &format!("/v1/enrollments/{}/approve", e["id"].as_str().unwrap()), Some(json!({"code": e["code"]}))).await;
    assert_eq!(s, StatusCode::PAYMENT_REQUIRED);
    assert!(body["error"].as_str().unwrap().contains("free plan"), "{body}");
    // Removing a machine frees the slot.
    assert_eq!(alice.json(Method::DELETE, "/v1/workers/two", None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(w.register("three", &["app"]).await, StatusCode::OK);

    // Two runs at once; the third is refused until one finishes.
    assert_eq!(alice.create_run("app").await.0, StatusCode::ACCEPTED);
    assert_eq!(alice.create_run("app").await.0, StatusCode::ACCEPTED);
    let (s, body) = alice.create_run("app").await;
    assert_eq!(s, StatusCode::PAYMENT_REQUIRED);
    assert!(body["error"].as_str().unwrap().contains("2 runs"));
    let (_, r1) = w.claim("one", &["app"]).await;
    let id = r1["id"].as_str().unwrap();
    w.call(Method::POST, &format!("/v1/worker/runs/{id}/finish?worker_id=one"), Some(json!({"status": "succeeded"}))).await;
    assert_eq!(alice.create_run("app").await.0, StatusCode::ACCEPTED);

    // Pro raises the limits; the default org (self-hosting) has none.
    sqlx::query("UPDATE orgs SET plan = 'pro' WHERE slug = 'alice'").execute(&env.pool).await.unwrap();
    assert_eq!(alice.create_run("app").await.0, StatusCode::ACCEPTED);
    let ws = Worker::new(&env, WORKER_TOKEN);
    for i in 0..12 {
        assert_eq!(ws.register(&format!("m{i}"), &["app"]).await, StatusCode::OK);
    }

    env.teardown().await;
}

/// A finished run with a session, a snapshot and a screenshot. Returns its id.
async fn finished_run_with_image(alice: &Browser, wa: &Worker, worker: &str) -> String {
    let (s, run) = alice.create_run("shop").await;
    assert_eq!(s, StatusCode::ACCEPTED, "{run}");
    let id = run["id"].as_str().unwrap().to_string();
    assert_eq!(wa.claim(worker, &["shop"]).await.0, StatusCode::OK);
    let snap = "0123456789abcdef0123456789abcdef01234567";
    let events = json!({"events": [
        {"kind": "agent_init", "payload": {"session_id": "s1"}},
        {"kind": "snapshot", "payload": {"before": snap, "after": snap, "files": []}},
        {"kind": "tool_result", "payload": {"tool_use_id": "t1", "content": "[image]", "images": [{"media_type": "image/png", "data": "iVBORw0KGgo="}]}},
    ]});
    assert_eq!(wa.call(Method::POST, &format!("/v1/worker/runs/{id}/events?worker_id={worker}"), Some(events)).await.0, StatusCode::OK);
    assert_eq!(wa.call(Method::POST, &format!("/v1/worker/runs/{id}/finish?worker_id={worker}"), Some(json!({"status": "succeeded"}))).await.0, StatusCode::OK);
    let (_, thread) = alice.json(Method::GET, &format!("/v1/runs/{id}/thread"), None).await;
    assert_eq!(thread.as_array().unwrap().len(), 1);
    id
}

#[tokio::test]
async fn restores_forks_archive_and_images_are_scoped_to_the_org() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let bob = Browser::login(&env, "bob").await;
    let (a_org, b_org) = (alice.org_id().await, bob.org_id().await);
    let (_, a_tok) = alice.create_token(&a_org, "a").await;
    let (_, b_tok) = bob.create_token(&b_org, "b").await;
    let wa = Worker::new(&env, a_tok["token"].as_str().unwrap());
    let wb = Worker::new(&env, b_tok["token"].as_str().unwrap());
    assert_eq!(wa.register("MacBook-Pro", &["shop"]).await, StatusCode::OK);
    assert_eq!(wb.register("MacBook-Pro", &["shop"]).await, StatusCode::OK);
    assert_eq!(wb.register("bobs-box", &["shop"]).await, StatusCode::OK);

    let id = finished_run_with_image(&alice, &wa, "MacBook-Pro").await;
    let img: Uuid = sqlx::query_scalar("SELECT id FROM run_images WHERE run_id = $1")
        .bind(id.parse::<Uuid>().unwrap())
        .fetch_one(&env.pool)
        .await
        .unwrap();

    // Bob can't restore, fork, archive or switch the agent of alice's thread.
    assert_eq!(bob.json(Method::POST, &format!("/v1/runs/{id}/restore"), None).await.0, StatusCode::NOT_FOUND);
    let fork = json!({"task": "go on", "model": "claude-sonnet-5-5"});
    assert_eq!(bob.json(Method::POST, &format!("/v1/runs/{id}/fork"), Some(fork.clone())).await.0, StatusCode::NOT_FOUND);
    assert_eq!(bob.json(Method::POST, &format!("/v1/runs/{id}/archive"), Some(json!({"archived": true}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        bob.json(Method::POST, &format!("/v1/runs/{id}/messages"), Some(json!({"task": "hi", "agent": "codex"}))).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(alice.json(Method::GET, &format!("/v1/runs/{id}"), None).await.1["archived_at"].is_null(), "bob's archive did nothing");

    // Nor read its screenshot, from the page or from his worker.
    assert_eq!(bob.req(Method::GET, &format!("/v1/images/{img}")).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    assert_eq!(wb.call(Method::GET, &format!("/v1/worker/images/{img}"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(alice.req(Method::GET, &format!("/v1/images/{img}")).send().await.unwrap().status(), StatusCode::OK);
    assert_eq!(wa.c.get(format!("{}/v1/worker/images/{img}", wa.base)).bearer_auth(&wa.token).send().await.unwrap().status(), StatusCode::OK);

    // Alice can't fork onto a machine that is only in bob's org.
    let (s, v) = alice.json(Method::POST, &format!("/v1/runs/{id}/fork"), Some(json!({"task": "go on", "model": "claude-sonnet-5-5", "worker_id": "bobs-box"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");

    // Her own restore, fork and archive work, and stay in her org.
    let (s, restore) = alice.json(Method::POST, &format!("/v1/runs/{id}/restore"), None).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{restore}");
    let rid = restore["id"].as_str().unwrap();
    assert_eq!(bob.json(Method::GET, &format!("/v1/runs/{rid}"), None).await.0, StatusCode::NOT_FOUND);
    // Bob's machine of the same name never gets the restore, even when it says it can run one.
    let claim = json!({"worker_id": "MacBook-Pro", "repos": ["shop"], "busy_repos": [], "agents": ["claude"], "kinds": ["agent", "restore", "fork"]});
    assert_eq!(wb.call(Method::POST, "/v1/worker/claim", Some(claim.clone())).await.0, StatusCode::NO_CONTENT);
    let (s, claimed) = wa.call(Method::POST, "/v1/worker/claim", Some(claim.clone())).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(claimed["id"], restore["id"]);
    assert_eq!(wa.call(Method::POST, &format!("/v1/worker/runs/{rid}/finish?worker_id=MacBook-Pro"), Some(json!({"status": "succeeded"}))).await.0, StatusCode::OK);

    let (s, forked) = alice.json(Method::POST, &format!("/v1/runs/{id}/fork"), Some(fork)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{forked}");
    let fid = forked["id"].as_str().unwrap();
    assert_eq!(bob.json(Method::GET, &format!("/v1/runs/{fid}"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(wb.call(Method::POST, "/v1/worker/claim", Some(claim.clone())).await.0, StatusCode::NO_CONTENT);
    assert_eq!(wa.call(Method::POST, "/v1/worker/claim", Some(claim)).await.1["id"], forked["id"]);

    assert_eq!(alice.json(Method::POST, &format!("/v1/runs/{id}/archive"), Some(json!({"archived": true}))).await.0, StatusCode::OK);
    let archived = alice.json(Method::GET, "/v1/runs?archived=true", None).await.1;
    assert_eq!(archived.as_array().unwrap().len(), 1);
    assert!(bob.json(Method::GET, "/v1/runs?archived=true", None).await.1.as_array().unwrap().is_empty());

    env.teardown().await;
}

#[tokio::test]
async fn routing_and_usage_limits_only_count_machines_of_the_runs_org() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let bob = Browser::login(&env, "bob").await;
    let (a_org, b_org) = (alice.org_id().await, bob.org_id().await);
    let (_, a_tok) = alice.create_token(&a_org, "a").await;
    let (_, b_tok) = bob.create_token(&b_org, "b").await;
    let wa = Worker::new(&env, a_tok["token"].as_str().unwrap());
    let wb = Worker::new(&env, b_tok["token"].as_str().unwrap());
    assert_eq!(wa.register("MacBook-Pro", &["shop"]).await, StatusCode::OK);
    // Bob has a machine of the same name and one of another name.
    assert_eq!(wb.register("MacBook-Pro", &["shop"]).await, StatusCode::OK);
    assert_eq!(wb.register("bobs-box", &["shop"]).await, StatusCode::OK);
    let claim = |worker: &str, parallel: usize, active: usize| {
        json!({"worker_id": worker, "repos": ["shop"], "busy_repos": [], "agents": ["claude"], "slots": {"parallel": parallel, "active": active}})
    };

    // Bob's idle machine (3 free slots) doesn't hold back alice's busier one (1 free):
    // least busy first only weighs machines in the run's org.
    assert_eq!(wb.call(Method::POST, "/v1/worker/claim", Some(claim("bobs-box", 3, 0))).await.0, StatusCode::NO_CONTENT);
    let (_, run) = alice.create_run("shop").await;
    let id = run["id"].as_str().unwrap();
    let (s, claimed) = wa.call(Method::POST, "/v1/worker/claim", Some(claim("MacBook-Pro", 3, 2))).await;
    assert_eq!(s, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["id"], run["id"]);

    // Alice's machine hits Claude's usage limit. Bob's machine could take the run, but it's
    // in another org, so the run fails instead of moving there.
    let (s, done) = wa
        .call(
            Method::POST,
            &format!("/v1/worker/runs/{id}/finish?worker_id=MacBook-Pro"),
            Some(json!({"status": "failed", "result": {"error": "usage limit"}, "usage_limit": {}})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{done}");
    assert_eq!(done["status"], "failed", "not re-queued for another org's machine");
    assert_eq!(wb.call(Method::POST, "/v1/worker/claim", Some(claim("bobs-box", 3, 0))).await.0, StatusCode::NO_CONTENT);

    // The limit is on alice's machine only, though bob's has the same name.
    let a_w = &alice.json(Method::GET, "/v1/workers", None).await.1[0];
    assert!(a_w["limited_until"]["claude"].is_string(), "{a_w}");
    let b_ws = bob.json(Method::GET, "/v1/workers", None).await.1;
    assert!(b_ws.as_array().unwrap().iter().all(|w| w["limited_until"] == json!({})), "{b_ws}");
    let (_, brun) = bob.create_run("shop").await;
    let (_, got) = wb.call(Method::POST, "/v1/worker/claim", Some(claim("MacBook-Pro", 3, 0))).await;
    assert_eq!(got["id"], brun["id"], "bob's MacBook-Pro is not limited");

    env.teardown().await;
}

#[tokio::test]
async fn a_new_sign_up_sends_one_telegram_message() {
    let (tg, sent) = sidefleet_server::test_support::FakeTelegram::start().await;
    let gh = sidefleet_server::test_support::FakeGithub::start().await;
    let env = TestEnv::start_with(|c| {
        c.github = Some(gh);
        c.telegram = Some(tg);
    })
    .await;

    // alice signs up, signs in again, then bob signs up: two messages, not three.
    env.github_login("alice").await;
    env.github_login("alice").await;
    env.github_login("bob").await;
    let mut msgs = Vec::new();
    for _ in 0..50 {
        msgs = sent.lock().unwrap().clone();
        if msgs.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(sent.lock().unwrap().len(), 2, "one message per new user: {msgs:?}");
    let mut texts: Vec<String> = msgs.iter().map(|(_, v)| v["text"].as_str().unwrap().to_string()).collect();
    texts.sort();
    for (token, v) in &msgs {
        assert_eq!(token, "123:test");
        assert_eq!(v["chat_id"], "42");
    }
    assert!(texts[0].contains("GitHub: alice (https://github.com/alice)") && texts[0].contains("Email: alice@example.com"), "{}", texts[0]);
    assert!(texts[1].contains("GitHub: bob") && texts[1].contains("Users: 2"), "{}", texts[1]);

    env.teardown().await;
}
