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

/// A script or chat agent using a personal API token.
async fn with_token(env: &TestEnv, token: &str, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut r = Client::new().request(m, format!("{}{}", env.base_url, path)).bearer_auth(token);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let r = r.send().await.unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn api_tokens_act_as_their_user_in_one_org_within_their_scope() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let org = alice.org_id().await;
    // Room for the handful of runs below.
    sqlx::query("UPDATE orgs SET plan = 'pro' WHERE id = $1::uuid").bind(&org).execute(&env.pool).await.unwrap();
    let run = |mode: Option<&str>| {
        let mut b = json!({"task": "do it", "model": "claude-sonnet-5-5", "repo": "app"});
        if let Some(m) = mode {
            b["permission_mode"] = json!(m);
        }
        b
    };

    // Minted on the page; the value is in the 201 only.
    let mint = |scope: &'static str| alice.json(Method::POST, "/v1/api-tokens", Some(json!({"name": format!("hermes {scope}"), "scope": scope})));
    let (s, read) = mint("read").await;
    assert_eq!(s, StatusCode::CREATED, "{read}");
    let read_tok = read["token"].as_str().unwrap().to_string();
    assert!(read_tok.starts_with("sfa_"));
    assert_eq!(read["scope"], "read");
    assert_eq!(read["prefix"], &read_tok[..8]);
    let tasks_tok = mint("tasks").await.1["token"].as_str().unwrap().to_string();
    let full_tok = mint("full").await.1["token"].as_str().unwrap().to_string();
    assert_eq!(alice.json(Method::POST, "/v1/api-tokens", Some(json!({"name": "x", "scope": "admin"}))).await.0, StatusCode::BAD_REQUEST);
    let (_, list) = alice.json(Method::GET, "/v1/api-tokens", None).await;
    assert_eq!(list.as_array().unwrap().len(), 3);
    assert!(!list.to_string().contains(&read_tok), "values are never listed");
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM api_tokens").fetch_all(&env.pool).await.unwrap();
    assert!(!stored.contains(&read_tok), "only digests are stored");

    // It is alice, in her org only.
    let (s, me) = with_token(&env, &read_tok, Method::GET, "/v1/me", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["user"]["login"], "alice");
    assert_eq!(me["orgs"].as_array().unwrap().len(), 1);
    assert_eq!(me["orgs"][0]["id"], org.as_str());
    assert_eq!(me["token"]["scope"], "read");

    // read: looks, never acts.
    assert_eq!(with_token(&env, &read_tok, Method::GET, "/v1/runs", None).await.0, StatusCode::OK);
    assert_eq!(with_token(&env, &read_tok, Method::GET, "/v1/workers", None).await.0, StatusCode::OK);
    assert_eq!(with_token(&env, &read_tok, Method::POST, "/v1/runs", Some(run(None))).await.0, StatusCode::FORBIDDEN);

    // tasks: starts work in alice's org, but never with full access.
    let (s, r) = with_token(&env, &tasks_tok, Method::POST, "/v1/runs", Some(run(Some("acceptEdits")))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    let (_, threads) = alice.json(Method::GET, "/v1/runs", None).await;
    assert!(threads.as_array().unwrap().iter().any(|t| t["id"] == r["id"]), "the page sees the token's run");
    assert_eq!(with_token(&env, &tasks_tok, Method::POST, "/v1/runs", Some(run(Some("bypassPermissions")))).await.0, StatusCode::FORBIDDEN);
    let rid = r["id"].as_str().unwrap();
    let approve = format!("/v1/runs/{rid}/approvals/{}/approve", Uuid::new_v4());
    assert_eq!(with_token(&env, &tasks_tok, Method::POST, &approve, None).await.0, StatusCode::FORBIDDEN);

    // A thread alice started with full access: tasks can't continue it as is, only in a safer mode.
    let (_, b) = alice.json(Method::POST, "/v1/runs", Some(run(Some("bypassPermissions")))).await;
    let bid = b["id"].as_str().unwrap();
    let reply = |mode: Option<&str>| {
        let mut v = json!({"task": "and then"});
        if let Some(m) = mode {
            v["permission_mode"] = json!(m);
        }
        v
    };
    assert_eq!(with_token(&env, &tasks_tok, Method::POST, &format!("/v1/runs/{bid}/messages"), Some(reply(None))).await.0, StatusCode::FORBIDDEN);
    let (s, v) = with_token(&env, &tasks_tok, Method::POST, &format!("/v1/runs/{bid}/messages"), Some(reply(Some("acceptEdits")))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    // Forking it is the same.
    assert_eq!(alice.json(Method::POST, &format!("/v1/runs/{bid}/stop"), None).await.0, StatusCode::OK);
    let fork = |mode: Option<&str>| {
        let mut v = json!({"task": "try another way", "model": "claude-sonnet-5-5"});
        if let Some(m) = mode {
            v["permission_mode"] = json!(m);
        }
        v
    };
    assert_eq!(with_token(&env, &tasks_tok, Method::POST, &format!("/v1/runs/{bid}/fork"), Some(fork(None))).await.0, StatusCode::FORBIDDEN);
    let (s, v) = with_token(&env, &tasks_tok, Method::POST, &format!("/v1/runs/{bid}/fork"), Some(fork(Some("plan")))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");

    // full: may use full access and decide approvals.
    let (s, v) = with_token(&env, &full_tok, Method::POST, "/v1/runs", Some(run(Some("bypassPermissions")))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(with_token(&env, &full_tok, Method::POST, &approve, None).await.0, StatusCode::NOT_FOUND, "allowed, but no such approval");

    // No token manages orgs, tokens, members or machines, or sees the setup (it carries a worker token).
    for (m, p) in [
        (Method::GET, "/v1/setup".to_string()),
        (Method::GET, "/v1/api-tokens".to_string()),
        (Method::GET, format!("/v1/orgs/{org}/tokens")),
        (Method::GET, format!("/v1/orgs/{org}/members")),
        (Method::GET, "/v1/enrollments".to_string()),
    ] {
        assert_eq!(with_token(&env, &full_tok, m.clone(), &p, None).await.0, StatusCode::FORBIDDEN, "{m} {p}");
    }
    assert_eq!(
        with_token(&env, &full_tok, Method::POST, "/v1/api-tokens", Some(json!({"name": "more", "scope": "full"}))).await.0,
        StatusCode::FORBIDDEN
    );

    // Pointing it at another org: 403.
    let bob = Browser::login(&env, "bob").await;
    let bob_org = bob.org_id().await;
    let r = Client::new().get(format!("{}/v1/runs", env.base_url)).bearer_auth(&read_tok).header("x-sidefleet-org", &bob_org).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // Only its owner sees and revokes it.
    let read_id = read["id"].as_str().unwrap();
    assert_eq!(bob.json(Method::GET, "/v1/api-tokens", None).await.1, json!([]));
    assert_eq!(bob.json(Method::DELETE, &format!("/v1/api-tokens/{read_id}"), None).await.0, StatusCode::NOT_FOUND);
    let (s, v) = alice.json(Method::DELETE, &format!("/v1/api-tokens/{read_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["revoked_at"].is_string());
    assert_eq!(with_token(&env, &read_tok, Method::GET, "/v1/runs", None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(with_token(&env, "sfa_made_up", Method::GET, "/v1/runs", None).await.0, StatusCode::UNAUTHORIZED);

    // Leaving the org ends its tokens there.
    sqlx::query("DELETE FROM org_members WHERE org_id = $1::uuid").bind(&org).execute(&env.pool).await.unwrap();
    assert_eq!(with_token(&env, &tasks_tok, Method::GET, "/v1/runs", None).await.0, StatusCode::UNAUTHORIZED);

    // The developer key mints tokens for the default org.
    let (s, v) = with_token(&env, DEV_KEY, Method::POST, "/v1/api-tokens", Some(json!({"name": "openclaw", "scope": "read"}))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (s, me) = with_token(&env, v["token"].as_str().unwrap(), Method::GET, "/v1/me", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["user"], Value::Null);
    assert_eq!(me["orgs"][0]["slug"], "default");
    assert_eq!(with_token(&env, DEV_KEY, Method::GET, "/v1/api-tokens", None).await.1.as_array().unwrap().len(), 1);

    env.teardown().await;
}

/// A chat agent talking MCP: one JSON-RPC call with an API token.
async fn mcp(env: &TestEnv, token: &str, method: &str, params: Value) -> Value {
    let (s, v) = with_token(env, token, Method::POST, "/mcp", Some(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))).await;
    assert_eq!(s, StatusCode::OK, "{method}: {v}");
    assert_eq!(v["id"], 1);
    v["result"].clone()
}

/// A tool's text, and whether it reported an error.
async fn tool(env: &TestEnv, token: &str, name: &str, args: Value) -> (String, bool) {
    let r = mcp(env, token, "tools/call", json!({"name": name, "arguments": args})).await;
    (r["content"][0]["text"].as_str().unwrap_or_default().to_string(), r["isError"] == true)
}

#[tokio::test]
async fn chat_agents_run_and_follow_tasks_over_mcp_within_their_scope() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let org = alice.org_id().await;
    let wt = alice.create_token(&org, "mac").await.1["token"].as_str().unwrap().to_string();
    let w = Worker::new(&env, &wt);
    let repos: Vec<Value> = vec![json!({"name": "code/shop", "path": "/tmp/shop", "id": "github.com/alice/shop"})];
    let reg = json!({"id": "mac", "display_name": "Mac mini", "capabilities": {"repos": repos, "agents": {"claude": {"version": "1", "signed_in": true}}}, "active_runs": []});
    assert_eq!(w.call(Method::POST, "/v1/worker/register", Some(reg)).await.0, StatusCode::OK);
    let mint = |scope: &'static str| alice.json(Method::POST, "/v1/api-tokens", Some(json!({"name": scope, "scope": scope})));
    let read = mint("read").await.1["token"].as_str().unwrap().to_string();
    let tasks = mint("tasks").await.1["token"].as_str().unwrap().to_string();
    let full = mint("full").await.1["token"].as_str().unwrap().to_string();

    // Handshake: the agent learns what Sidefleet is; notifications get 202.
    let init = mcp(&env, &tasks, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hermes", "version": "1"}})).await;
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "sidefleet");
    assert!(init["instructions"].as_str().unwrap().contains("start_task"));
    let (s, _) = with_token(&env, &tasks, Method::POST, "/mcp", Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(with_token(&env, &tasks, Method::GET, "/mcp", None).await.0, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(Client::new().post(format!("{}/mcp", env.base_url)).json(&json!({})).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);

    // Each token sees the tools its scope allows.
    let names = |r: Value| r["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(mcp(&env, &read, "tools/list", json!({})).await), ["fleet_overview", "list_tasks", "task_status"]);
    assert_eq!(names(mcp(&env, &tasks, "tools/list", json!({})).await).len(), 7);
    assert_eq!(names(mcp(&env, &full, "tools/list", json!({})).await).len(), 9);

    // The overview names the machine and its repo.
    let (t, err) = tool(&env, &read, "fleet_overview", json!({})).await;
    assert!(!err, "{t}");
    assert!(t.contains("Mac mini (online") && t.contains("alice/shop"), "{t}");

    // read can't start work; tasks can, by the repo's short name, but never with full access.
    let (t, err) = tool(&env, &read, "start_task", json!({"task": "fix it", "repo": "shop"})).await;
    assert!(err && t.contains("limited"), "{t}");
    let (t, err) = tool(&env, &tasks, "start_task", json!({"task": "fix it", "repo": "shop", "access": "full"})).await;
    assert!(err && t.contains("full scope"), "{t}");
    let (t, err) = tool(&env, &tasks, "start_task", json!({"task": "Fix the checkout bug\nIt double-charges.", "repo": "shop", "machine": "mac mini"})).await;
    assert!(!err, "{t}");
    let id = t.split_whitespace().nth(2).unwrap().to_string();
    assert!(t.contains(&format!("/#/runs/{id}")), "{t}");
    let (_, threads) = alice.json(Method::GET, "/v1/runs", None).await;
    assert_eq!(threads[0]["repo"], "github.com/alice/shop");
    assert_eq!(threads[0]["permission_mode"], "acceptEdits");
    let (t, _) = tool(&env, &read, "task_status", json!({"task_id": id})).await;
    assert!(t.contains("Status: waiting for a machine"), "{t}");

    // The machine runs it; task_status with wait follows it to the end.
    assert_eq!(w.claim("mac", &["github.com/alice/shop"]).await.0, StatusCode::OK);
    let events = json!({"events": [
        {"kind": "agent_init", "payload": {"session_id": "s1"}},
        {"kind": "assistant_text", "payload": {"text": "Looking at the checkout."}},
        {"kind": "diff", "payload": {"turn": {"files": [{"path": "src/pay.rs", "status": "modified", "additions": 3, "deletions": 1}], "file_count": 1, "additions": 3, "deletions": 1}, "total": null}},
        {"kind": "result", "payload": {"result": "Fixed: the retry charged twice."}},
    ]});
    let finish = async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(w.call(Method::POST, &format!("/v1/worker/runs/{id}/events?worker_id=mac"), Some(events)).await.0, StatusCode::OK);
        assert_eq!(w.call(Method::POST, &format!("/v1/worker/runs/{id}/finish?worker_id=mac"), Some(json!({"status": "succeeded"}))).await.0, StatusCode::OK);
    };
    let ((t, err), ()) = tokio::join!(tool(&env, &read, "task_status", json!({"task_id": id, "wait": true})), finish);
    assert!(!err, "{t}");
    assert!(t.contains("Status: done on mac"), "{t}");
    assert!(t.contains("Fixed: the retry charged twice."), "{t}");
    assert!(t.contains("src/pay.rs (+3 -1)"), "{t}");
    let (t, _) = tool(&env, &read, "list_tasks", json!({})).await;
    assert!(t.contains("[succeeded] Fix the checkout bug (alice/shop, on mac)"), "{t}");

    // Replies continue it; stop stops it.
    let (t, err) = tool(&env, &tasks, "reply", json!({"task_id": id, "message": "also add a test"})).await;
    assert!(!err && t.starts_with("Sent."), "{t}");
    let (t, err) = tool(&env, &tasks, "stop_task", json!({"task_id": id})).await;
    assert!(!err && t.starts_with("Stopping"), "{t}");
    let (t, _) = tool(&env, &tasks, "stop_task", json!({"task_id": id})).await;
    assert!(t.starts_with("Nothing to stop"), "{t}");

    // Bad input reads as a tool error, never a request elsewhere.
    let (t, err) = tool(&env, &full, "task_status", json!({"task_id": "../../v1/setup"})).await;
    assert!(err && t.contains("not a Sidefleet id"), "{t}");
    let (t, err) = tool(&env, &full, "start_task", json!({"task": "x", "repo": "nope"})).await;
    assert!(err && t.contains("The machines have: alice/shop"), "{t}");
    let (t, err) = tool(&env, &full, "approve", json!({"task_id": id, "approval_id": Uuid::new_v4().to_string()})).await;
    assert!(err, "{t}");

    env.teardown().await;
}

/// Stands in for Hermes or OpenClaw: records each request, answers with `status`.
#[derive(Clone)]
struct Receiver {
    url: String,
    got: std::sync::Arc<std::sync::Mutex<Vec<(axum::http::HeaderMap, Value)>>>,
    status: std::sync::Arc<std::sync::atomic::AtomicU16>,
}

impl Receiver {
    async fn start() -> Receiver {
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let status = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(202));
        let (g, st) = (got.clone(), status.clone());
        let app = axum::Router::new().fallback(move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
            let (g, st) = (g.clone(), st.clone());
            async move {
                g.lock().unwrap().push((headers, serde_json::from_slice(&body).unwrap_or(Value::Null)));
                axum::http::StatusCode::from_u16(st.load(std::sync::atomic::Ordering::SeqCst)).unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Receiver { url, got, status }
    }
    fn count(&self) -> usize {
        self.got.lock().unwrap().len()
    }
    /// The `n`th request, waiting up to 5 s for it.
    async fn nth(&self, n: usize) -> (axum::http::HeaderMap, Value) {
        for _ in 0..100 {
            if let Some(r) = self.got.lock().unwrap().get(n) {
                return r.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no request #{n} arrived");
    }
}

/// A Hermes route's check: hex HMAC-SHA256 of `<timestamp>.<body>`.
fn signed_by(h: &axum::http::HeaderMap, body: &Value, secret: &str) -> bool {
    let ts = h["x-webhook-timestamp"].to_str().unwrap();
    let want = sidefleet_server::chat_updates::hex(&sidefleet_server::chat_updates::hmac_sha256(secret.as_bytes(), format!("{ts}.{body}").as_bytes()));
    h["x-webhook-signature-v2"].to_str().unwrap() == want
}

#[tokio::test]
async fn chat_apps_hear_when_their_tasks_finish_or_need_approval() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let org = alice.org_id().await;
    let w = Worker::new(&env, alice.create_token(&org, "mac").await.1["token"].as_str().unwrap());
    let reg = json!({"id": "mac", "display_name": "Mac mini", "capabilities": {"repos": [{"name": "shop", "path": "/tmp/shop", "id": ""}], "agents": {"claude": {"version": "1", "signed_in": true}}}, "active_runs": []});
    assert_eq!(w.call(Method::POST, "/v1/worker/register", Some(reg)).await.0, StatusCode::OK);
    let (hermes, openclaw, hook) = (Receiver::start().await, Receiver::start().await, Receiver::start().await);
    let mint = |name: &'static str, scope: &'static str| alice.json(Method::POST, "/v1/api-tokens", Some(json!({"name": name, "scope": scope})));
    let h = mint("Hermes", "full").await.1;
    let (h_id, h_tok) = (h["id"].as_str().unwrap().to_string(), h["token"].as_str().unwrap().to_string());
    let o_id = mint("OpenClaw", "tasks").await.1["id"].as_str().unwrap().to_string();
    let k_id = mint("Watcher", "read").await.1["id"].as_str().unwrap().to_string();

    // Turning updates on hands out the secret the app checks them with; listing never shows it.
    async fn put(b: &Browser, id: &str, kind: &str, url: String, all: bool) -> (StatusCode, Value) {
        b.json(Method::PUT, &format!("/v1/api-tokens/{id}/updates"), Some(json!({"kind": kind, "url": url, "all_tasks": all}))).await
    }
    let on = |id: &str, kind: &'static str, url: String, all: bool| {
        let id = id.to_string();
        let alice = &alice;
        async move { put(alice, &id, kind, url, all).await }
    };
    let (s, v) = on(&h_id, "hermes", format!("{}/webhooks/sidefleet", hermes.url), false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let h_secret = v["secret"].as_str().unwrap().to_string();
    assert_eq!(v["updates"]["kind"], "hermes");
    let (_, v) = on(&h_id, "hermes", format!("{}/webhooks/sidefleet", hermes.url), false).await;
    assert_eq!(v["secret"], h_secret.as_str(), "changing the address keeps the secret");
    let o_secret = on(&o_id, "openclaw", format!("{}/hooks/agent", openclaw.url), false).await.1["secret"].as_str().unwrap().to_string();
    let k_secret = on(&k_id, "webhook", format!("{}/sidefleet", hook.url), true).await.1["secret"].as_str().unwrap().to_string();
    assert_eq!(on(&k_id, "slack", hook.url.clone(), false).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(on(&k_id, "webhook", "file:///etc/passwd".into(), false).await.0, StatusCode::BAD_REQUEST);
    let (_, list) = alice.json(Method::GET, "/v1/api-tokens", None).await;
    assert!(!list.to_string().contains(&h_secret));
    // Tokens can't change where updates go.
    assert_eq!(with_token(&env, &h_tok, Method::PUT, &format!("/v1/api-tokens/{h_id}/updates"), Some(json!({"kind": "webhook", "url": "http://x"}))).await.0, StatusCode::FORBIDDEN);

    // A test: Hermes gets a signed payload, OpenClaw its hook token and a message.
    assert_eq!(alice.json(Method::POST, &format!("/v1/api-tokens/{h_id}/updates/test"), None).await.0, StatusCode::OK);
    let (hd, body) = hermes.nth(0).await;
    assert_eq!(body["event_type"], "test");
    assert!(signed_by(&hd, &body, &h_secret));
    assert!(!signed_by(&hd, &body, "wrong"));
    assert_eq!(alice.json(Method::POST, &format!("/v1/api-tokens/{o_id}/updates/test"), None).await.0, StatusCode::OK);
    let (od, body) = openclaw.nth(0).await;
    assert_eq!(od["authorization"].to_str().unwrap(), format!("Bearer {o_secret}"));
    assert_eq!(body["name"], "Sidefleet");
    assert!(body["message"].as_str().unwrap().contains("test update"));
    // A refused test says why, and the page shows it.
    openclaw.status.store(401, std::sync::atomic::Ordering::SeqCst);
    let (s, v) = alice.json(Method::POST, &format!("/v1/api-tokens/{o_id}/updates/test"), None).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    assert!(v["error"].as_str().unwrap().contains("secret doesn't match"), "{v}");
    let (_, list) = alice.json(Method::GET, "/v1/api-tokens", None).await;
    let o = list.as_array().unwrap().iter().find(|t| t["id"] == o_id.as_str()).unwrap().clone();
    assert!(o["updates"]["last_error"].as_str().unwrap().contains("secret"));
    openclaw.status.store(202, std::sync::atomic::Ordering::SeqCst);

    // Hermes starts a task over MCP; the machine asks to run a command: Hermes hears it, with the ids to decide.
    let (t, err) = tool(&env, &h_tok, "start_task", json!({"task": "Fix the checkout bug", "repo": "shop", "access": "ask"})).await;
    assert!(!err, "{t}");
    let id = t.split_whitespace().nth(2).unwrap().to_string();
    assert_eq!(w.claim("mac", &["shop"]).await.0, StatusCode::OK);
    let (s, a) = w.call(Method::POST, &format!("/v1/worker/runs/{id}/approvals?worker_id=mac"), Some(json!({"tool_name": "Bash", "input": {"command": "pnpm test"}}))).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let (hd, body) = hermes.nth(1).await;
    assert_eq!(body["event_type"], "task.needs_approval");
    assert!(signed_by(&hd, &body, &h_secret));
    let text = body["text"].as_str().unwrap();
    assert!(text.contains("`pnpm test`") && text.contains(a["id"].as_str().unwrap()) && text.contains(&id), "{text}");
    // ...and approves it once the user says yes.
    let (t, err) = tool(&env, &h_tok, "approve", json!({"task_id": id, "approval_id": a["id"]})).await;
    assert!(!err, "{t}");

    // It finishes: Hermes and the all-tasks webhook hear; OpenClaw (not its task) doesn't.
    let events = json!({"events": [
        {"kind": "agent_init", "payload": {"session_id": "s1"}},
        {"kind": "diff", "payload": {"turn": {"files": [{"path": "src/pay.rs", "status": "modified", "additions": 3, "deletions": 1}], "file_count": 1, "additions": 3, "deletions": 1}, "total": null}},
        {"kind": "result", "payload": {"result": "Fixed: the retry charged twice."}},
    ]});
    assert_eq!(w.call(Method::POST, &format!("/v1/worker/runs/{id}/events?worker_id=mac"), Some(events)).await.0, StatusCode::OK);
    assert_eq!(w.call(Method::POST, &format!("/v1/worker/runs/{id}/finish?worker_id=mac"), Some(json!({"status": "succeeded"}))).await.0, StatusCode::OK);
    let (_, body) = hermes.nth(2).await;
    assert_eq!(body["event_type"], "task.finished");
    let text = body["text"].as_str().unwrap();
    assert!(text.contains("Fixed: the retry charged twice.") && text.contains("src/pay.rs") && text.contains("open_pull_request"), "{text}");
    assert_eq!(hook.nth(0).await.1["event_type"], "task.needs_approval", "it wants every task");
    let (kd, body) = hook.nth(1).await;
    assert_eq!(body["event_type"], "task.finished");
    assert_eq!(body["task"]["id"], id.as_str());
    assert!(signed_by(&kd, &body, &k_secret));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(openclaw.count(), 2, "only the two tests");

    // A task started on the page: only the all-tasks webhook hears it.
    let (_, r) = alice.create_run("shop").await;
    let pid = r["id"].as_str().unwrap();
    assert_eq!(w.claim("mac", &["shop"]).await.0, StatusCode::OK);
    assert_eq!(w.call(Method::POST, &format!("/v1/worker/runs/{pid}/finish?worker_id=mac"), Some(json!({"status": "failed", "result": {"error": "boom"}}))).await.0, StatusCode::OK);
    let (_, body) = hook.nth(2).await;
    assert_eq!(body["event_type"], "task.failed");
    assert!(body["text"].as_str().unwrap().contains("Error: boom"));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(hermes.count(), 3);

    // Off means off; a disconnected app hears nothing more.
    assert_eq!(alice.json(Method::DELETE, &format!("/v1/api-tokens/{k_id}/updates"), None).await.0, StatusCode::OK);
    assert_eq!(alice.json(Method::POST, &format!("/v1/api-tokens/{k_id}/updates/test"), None).await.0, StatusCode::NOT_FOUND);
    env.teardown().await;

    // Hosted with the default: no private or local addresses.
    let env = TestEnv::start_with(|c| {
        c.github = Some(sidefleet_server::GithubOAuth::new("x".into(), "y".into()));
        c.private_webhooks = false;
    })
    .await;
    let (s, v) = with_token(&env, DEV_KEY, Method::POST, "/v1/api-tokens", Some(json!({"name": "Hermes", "scope": "read"}))).await;
    assert_eq!(s, StatusCode::CREATED);
    let id = v["id"].as_str().unwrap();
    let (s, v) = with_token(&env, DEV_KEY, Method::PUT, &format!("/v1/api-tokens/{id}/updates"), Some(json!({"kind": "hermes", "url": hermes.url}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("private or local"), "{v}");
    env.teardown().await;
}

#[tokio::test]
async fn preferences_follow_you_to_every_browser_and_stay_yours() {
    let env = TestEnv::start_hosted().await;
    let laptop = Browser::login(&env, "alice").await;
    let phone = Browser::login(&env, "alice").await;
    let bob = Browser::login(&env, "bob").await;

    // Nothing saved yet.
    assert_eq!(laptop.json(Method::GET, "/v1/me/prefs", None).await, (StatusCode::OK, json!({})));

    // Saved on the laptop, there on the phone; Bob's are his own.
    let p = json!({"permission": "bypassPermissions", "roomPermission": "acceptEdits", "workspace": "checkout"});
    assert_eq!(laptop.json(Method::PUT, "/v1/me/prefs", Some(p.clone())).await, (StatusCode::OK, p.clone()));
    assert_eq!(phone.json(Method::GET, "/v1/me/prefs", None).await, (StatusCode::OK, p.clone()));
    assert_eq!(bob.json(Method::GET, "/v1/me/prefs", None).await, (StatusCode::OK, json!({})));

    // Only an object, and not a big one.
    assert_eq!(laptop.json(Method::PUT, "/v1/me/prefs", Some(json!(["x"]))).await.0, StatusCode::BAD_REQUEST);
    let big = json!({ "x": "a".repeat(20_000) });
    assert_eq!(laptop.json(Method::PUT, "/v1/me/prefs", Some(big)).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(phone.json(Method::GET, "/v1/me/prefs", None).await.1, p);

    // The developer key is nobody in particular, and a chat app's token can't touch them.
    assert_eq!(with_token(&env, DEV_KEY, Method::GET, "/v1/me/prefs", None).await.0, StatusCode::NOT_FOUND);
    let (_, tok) = laptop.json(Method::POST, "/v1/api-tokens", Some(json!({"name": "hermes", "scope": "full"}))).await;
    let tok = tok["token"].as_str().unwrap();
    assert_eq!(with_token(&env, tok, Method::GET, "/v1/me/prefs", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(with_token(&env, tok, Method::PUT, "/v1/me/prefs", Some(json!({}))).await.0, StatusCode::FORBIDDEN);
    env.teardown().await;
}
