//! The audit log end to end: what gets written as runs, approvals, machines and people
//! change, that nothing secret lands in it, who may read which entries, paging, filters
//! and exports, and that rows can't be changed. Needs Postgres (docker compose up -d).

use std::time::Duration;

use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sidefleet_server::test_support::{TestEnv, DEV_KEY, WORKER_TOKEN};
use sqlx::Row;
use uuid::Uuid;

const DEFAULT_ORG: &str = "00000000-0000-0000-0000-000000000001";
const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123";

/// A caller: the developer key, or a GitHub session in an org.
struct Api {
    c: Client,
    base: String,
    cookie: Option<String>,
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
        let mut r = self.c.request(m, format!("{}{}", self.base, path));
        match &self.cookie {
            Some(c) => r = r.header("cookie", format!("sf_session={c}")),
            None => r = r.bearer_auth(DEV_KEY),
        }
        if let Some(o) = &self.org {
            r = r.header("x-sidefleet-org", o);
        }
        r
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
    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.call(Method::GET, path, None).await
    }
    /// The audit page at `query` (without the leading `?`), asserting it is 200.
    async fn audit(&self, org: &str, query: &str) -> Value {
        let (s, v) = self.get(&format!("/v1/orgs/{org}/audit?{query}")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }
}

fn actions(page: &Value) -> Vec<String> {
    page["events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap().to_string()).collect()
}

fn find<'a>(page: &'a Value, action: &str) -> &'a Value {
    page["events"].as_array().unwrap().iter().find(|e| e["action"] == action).unwrap_or_else(|| panic!("no {action} in {page}"))
}

/// A machine, by token.
struct Machine {
    c: Client,
    base: String,
    token: String,
    id: String,
}

impl Machine {
    async fn start(env: &TestEnv, token: &str, id: &str) -> Machine {
        let m = Machine { c: Client::new(), base: env.base_url.clone(), token: token.into(), id: id.into() };
        let caps = json!({"repos": [{"name": "proj", "path": "/tmp/proj", "id": "github.com/acme/proj"}], "agents": {"claude": {"version": "1", "signed_in": true}}});
        let (s, v) = m.call(Method::POST, "/v1/worker/register", json!({"id": id, "display_name": id, "capabilities": caps, "active_runs": []})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        m
    }
    async fn call(&self, method: Method, path: &str, body: Value) -> (StatusCode, Value) {
        let mut r = self.c.request(method.clone(), format!("{}{}", self.base, path)).bearer_auth(&self.token);
        if method != Method::GET {
            r = r.json(&body);
        }
        let r = r.send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn claim(&self) -> Value {
        let (s, v) = self
            .call(Method::POST, "/v1/worker/claim", json!({"worker_id": self.id, "repos": ["proj"], "busy_repos": [], "agents": ["claude"], "kinds": ["agent", "restore", "fork", "pr"]}))
            .await;
        assert_eq!(s, StatusCode::OK, "claim: {v}");
        v
    }
    async fn events(&self, run: &str, events: Value) {
        let (s, v) = self.call(Method::POST, &format!("/v1/worker/runs/{run}/events?worker_id={}", self.id), json!({ "events": events })).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    async fn ask(&self, run: &str, body: Value) -> Value {
        let (s, v) = self.call(Method::POST, &format!("/v1/worker/runs/{run}/approvals?worker_id={}", self.id), body).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v
    }
    async fn finish(&self, run: &str, status: &str) {
        let (s, v) = self.call(Method::POST, &format!("/v1/worker/runs/{run}/finish?worker_id={}", self.id), json!({"status": status, "result": {}})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
}

/// Every stored row as text, for "it never contains ..." checks.
async fn all_rows(env: &TestEnv) -> String {
    let rows = sqlx::query("SELECT concat_ws('|', actor, action, target, worker_id, detail::text) AS t FROM audit_events").fetch_all(&env.pool).await.unwrap();
    rows.iter().map(|r| r.get::<String, _>("t")).collect::<Vec<_>>().join("\n")
}

#[tokio::test]
async fn the_schema_is_append_only() {
    let env = TestEnv::start().await;
    let user: Uuid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, github_id, login) VALUES ($1, 4242, 'dora')").bind(user).execute(&env.pool).await.unwrap();
    sqlx::query("INSERT INTO audit_events (org_id, actor_id, actor, action) VALUES ($1::uuid, $2, 'dora', 'run.created')")
        .bind(DEFAULT_ORG)
        .bind(user)
        .execute(&env.pool)
        .await
        .unwrap();
    // Defaults.
    let r = sqlx::query("SELECT detail, at FROM audit_events").fetch_one(&env.pool).await.unwrap();
    assert_eq!(r.get::<Value, _>("detail"), json!({}));

    let e = sqlx::query("UPDATE audit_events SET action = 'run.finished'").execute(&env.pool).await.unwrap_err();
    assert!(e.to_string().contains("append-only"), "{e}");
    let e = sqlx::query("UPDATE audit_events SET actor_id = NULL, actor = 'someone else'").execute(&env.pool).await.unwrap_err();
    assert!(e.to_string().contains("append-only"), "{e}");
    assert!(sqlx::query("INSERT INTO audit_events (org_id, actor, action, detail) VALUES ($1::uuid, 'x', 'run.created', '[]')").bind(DEFAULT_ORG).execute(&env.pool).await.is_err(), "detail is an object");

    // Deleting a person keeps their entries, still named, without the link.
    sqlx::query("DELETE FROM users WHERE id = $1").bind(user).execute(&env.pool).await.unwrap();
    let r = sqlx::query("SELECT actor_id, actor FROM audit_events").fetch_one(&env.pool).await.unwrap();
    assert_eq!(r.get::<Option<Uuid>, _>("actor_id"), None);
    assert_eq!(r.get::<String, _>("actor"), "dora");
    env.teardown().await;
}

#[tokio::test]
async fn a_runs_life_and_its_approvals_are_logged_without_secrets() {
    let env = TestEnv::start().await;
    let api = Api::dev(&env);
    let w = Machine::start(&env, WORKER_TOKEN, "w").await;

    let task = format!("unique-task-text: deploy with {SECRET}");
    let (s, v) = api.call(Method::POST, "/v1/runs", Some(json!({"task": task, "model": "sonnet", "repo": "proj", "permission_mode": "ask"}))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let claimed = w.claim().await;
    assert_eq!(claimed["id"], id.as_str());
    w.events(&id, json!([{"kind": "agent_init", "payload": {"session_id": "s1"}}, {"kind": "assistant_text", "payload": {"text": "unique-agent-output"}}])).await;

    // One denied with a message, one left to time out.
    let a = w.ask(&id, json!({"tool_name": "Bash", "input": {"command": format!("curl -H 'Authorization: Bearer {SECRET}' https://example.com")}})).await;
    let aid = a["id"].as_str().unwrap();
    let (s, v) = api.call(Method::POST, &format!("/v1/runs/{id}/approvals/{aid}/deny"), Some(json!({"message": "unique-denial-words"}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let b = w.ask(&id, json!({"tool_name": "Write", "input": {"file_path": "/tmp/x", "content": "unique-file-content"}, "timeout_secs": 1})).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let (s, v) = w.call(Method::GET, &format!("/v1/worker/runs/{id}/approvals/{}", b["id"].as_str().unwrap()), json!({})).await;
    assert_eq!((s, v["status"].as_str()), (StatusCode::OK, Some("expired")));
    w.finish(&id, "succeeded").await;

    // A reply in another permission mode.
    let (s, v) = api.call(Method::POST, &format!("/v1/runs/{id}/messages"), Some(json!({"task": "go on", "permission_mode": "plan"}))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let reply = v["id"].as_str().unwrap().to_string();

    let page = api.audit(DEFAULT_ORG, "limit=200").await;
    let got = actions(&page);
    for a in [
        "machine.enrolled",
        "run.created",
        "run.claimed",
        "run.started",
        "approval.requested",
        "approval.denied",
        "approval.timed_out",
        "run.finished",
        "run.permission_mode_changed",
    ] {
        assert!(got.iter().any(|g| g == a), "{a} missing from {got:?}");
    }
    // Newest first.
    assert_eq!(got[0], "run.permission_mode_changed", "{got:?}");

    let created = find(&page, "run.created");
    assert_eq!(created["actor"], "developer key");
    assert_eq!(created["actor_id"], Value::Null);
    let claimed = find(&page, "run.claimed");
    assert_eq!((claimed["actor"].as_str(), claimed["worker_id"].as_str(), claimed["run_id"].as_str()), (Some("worker:w"), Some("w"), Some(id.as_str())));
    let denied = find(&page, "approval.denied");
    assert_eq!(denied["actor"], "developer key");
    assert_eq!(denied["detail"]["tool_name"], "Bash");
    assert_eq!(denied["detail"]["with_message"], true);
    assert!(denied["detail"]["summary"].as_str().unwrap().starts_with("curl"));
    assert_eq!(find(&page, "approval.timed_out")["actor"], "system");
    let finished = find(&page, "run.finished");
    assert_eq!(finished["detail"]["status"], "succeeded");
    let mode = find(&page, "run.permission_mode_changed");
    assert_eq!((mode["detail"]["from"].as_str(), mode["detail"]["to"].as_str(), mode["run_id"].as_str()), (Some("ask"), Some("plan"), Some(reply.as_str())));

    // Never secrets, task text, agent output, tool input or the person's words to the agent.
    let stored = all_rows(&env).await;
    for needle in ["sk-ant-api03", "unique-task-text", "unique-agent-output", "unique-file-content", "unique-denial-words"] {
        assert!(!stored.contains(needle), "{needle} reached the audit log:\n{stored}");
    }

    // Filters.
    let by_run = api.audit(DEFAULT_ORG, &format!("run={id}")).await;
    assert!(by_run["events"].as_array().unwrap().iter().all(|e| e["run_id"] == id.as_str()));
    assert!(actions(&by_run).contains(&"approval.requested".to_string()));
    let thread = api.audit(DEFAULT_ORG, &format!("thread={id}")).await;
    let runs: std::collections::HashSet<&str> = thread["events"].as_array().unwrap().iter().map(|e| e["run_id"].as_str().unwrap()).collect();
    assert_eq!(runs, [id.as_str(), reply.as_str()].into_iter().collect(), "the whole thread, nothing else");
    let approvals = api.audit(DEFAULT_ORG, "action=approval.*").await;
    assert!(actions(&approvals).iter().all(|a| a.starts_with("approval.")) && actions(&approvals).len() == 4, "{approvals}");
    let worker = api.audit(DEFAULT_ORG, "actor=worker:w").await;
    assert!(worker["events"].as_array().unwrap().iter().all(|e| e["actor"] == "worker:w"));
    let machine = api.audit(DEFAULT_ORG, "machine=w&action=run.claimed").await;
    assert_eq!(actions(&machine), vec!["run.claimed"]);
    let (s, page) = api.get("/v1/workers/w/audit?action=machine.enrolled").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(actions(&page), vec!["machine.enrolled"]);
    // Reserved names are accepted (and match nothing yet); unknown ones are not.
    assert_eq!(api.audit(DEFAULT_ORG, "action=payment.made").await["events"], json!([]));
    assert_eq!(api.get(&format!("/v1/orgs/{DEFAULT_ORG}/audit?action=nonsense")).await.0, StatusCode::BAD_REQUEST);
    env.teardown().await;
}

#[tokio::test]
async fn pages_time_ranges_and_exports() {
    let env = TestEnv::start().await;
    let api = Api::dev(&env);
    for i in 0..5 {
        let (s, _) = api.call(Method::POST, "/v1/runs", Some(json!({"task": format!("t{i}"), "model": "sonnet", "repo": "proj"}))).await;
        assert_eq!(s, StatusCode::ACCEPTED);
    }
    // Page through two at a time.
    let mut seen = Vec::new();
    let mut before: Option<i64> = None;
    loop {
        let q = match before {
            Some(b) => format!("action=run.created&limit=2&before={b}"),
            None => "action=run.created&limit=2".into(),
        };
        let page = api.audit(DEFAULT_ORG, &q).await;
        let events = page["events"].as_array().unwrap();
        assert!(events.len() <= 2);
        seen.extend(events.iter().map(|e| e["id"].as_i64().unwrap()));
        match page["next"].as_i64() {
            Some(n) => before = Some(n),
            None => break,
        }
    }
    assert_eq!(seen.len(), 5, "{seen:?}");
    let mut sorted = seen.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(seen, sorted, "newest first, no repeats");

    // Time ranges.
    let past = "2000-01-01T00:00:00Z";
    assert_eq!(api.audit(DEFAULT_ORG, &format!("until={past}")).await["events"], json!([]));
    assert_eq!(api.audit(DEFAULT_ORG, &format!("since={past}&action=run.created")).await["events"].as_array().unwrap().len(), 5);
    assert_eq!(api.get(&format!("/v1/orgs/{DEFAULT_ORG}/audit?since=2001-01-01T00:00:00Z&until={past}")).await.0, StatusCode::BAD_REQUEST);

    // CSV: a header, one line per entry, as a download.
    let r = api.req(Method::GET, &format!("/v1/orgs/{DEFAULT_ORG}/audit/export?action=run.created")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/csv"));
    assert!(r.headers()["content-disposition"].to_str().unwrap().starts_with("attachment; filename=\"sidefleet-audit-default-"));
    assert_eq!(r.headers()["x-sidefleet-truncated"], "false");
    let csv = r.text().await.unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "id,at,actor,actor_id,action,target,run_id,worker_id,detail");
    assert_eq!(lines.len(), 6, "{csv}");
    assert!(lines[1..].iter().all(|l| l.contains(",developer key,,run.created,")), "{csv}");

    // JSON: the same entries.
    let r = api.req(Method::GET, &format!("/v1/orgs/{DEFAULT_ORG}/audit/export?format=json&action=run.created")).send().await.unwrap();
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("application/json"));
    let rows: Vec<Value> = r.json().await.unwrap();
    assert_eq!(rows.iter().map(|e| e["id"].as_i64().unwrap()).collect::<Vec<_>>(), seen);
    assert_eq!(api.get(&format!("/v1/orgs/{DEFAULT_ORG}/audit/export?format=xml")).await.0, StatusCode::BAD_REQUEST);
    env.teardown().await;
}

/// Alice's team org with Bob in it (joined through an invite link), each with a machine.
/// Returns the org id too.
async fn team(env: &TestEnv) -> (Api, Api, Machine, Machine, String) {
    let mut alice = Api::user(env, "alice").await;
    let (s, org) = alice.call(Method::POST, "/v1/orgs", Some(json!({"name": "Acme"}))).await;
    assert!(s.is_success(), "{org}");
    let org_id = org["id"].as_str().unwrap().to_string();
    alice.org = Some(org_id.clone());

    let (s, inv) = alice.call(Method::POST, &format!("/v1/orgs/{org_id}/invites"), Some(json!({}))).await;
    assert_eq!(s, StatusCode::CREATED, "{inv}");
    let code = inv["code"].as_str().unwrap().to_string();
    let mut bob = Api::user(env, "bob").await;
    let (s, joined) = bob.call(Method::POST, &format!("/v1/invites/{code}/accept"), None).await;
    assert_eq!(s, StatusCode::OK, "{joined}");
    bob.org = Some(org_id.clone());

    let (_, tok) = alice.call(Method::POST, &format!("/v1/orgs/{org_id}/tokens"), Some(json!({"name": "alice-mac"}))).await;
    let alice_m = Machine::start(env, tok["token"].as_str().unwrap(), "alice-mac").await;
    let e: Value = Client::new()
        .post(format!("{}/v1/enroll", env.base_url))
        .json(&json!({"worker_id": "bob-linux", "machine_name": "bob-linux", "capabilities": {}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let (s, v) = bob.call(Method::POST, &format!("/v1/enrollments/{}/approve", e["id"].as_str().unwrap()), Some(json!({"code": e["code"]}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let polled: Value = Client::new()
        .get(format!("{}/v1/enroll/{}", env.base_url, e["id"].as_str().unwrap()))
        .bearer_auth(e["secret"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let bob_m = Machine::start(env, polled["token"].as_str().unwrap(), "bob-linux").await;
    (alice, bob, alice_m, bob_m, org_id)
}

#[tokio::test]
async fn owners_see_the_org_members_see_their_machines_and_other_orgs_see_nothing() {
    let env = TestEnv::start_hosted().await;
    let (alice, bob, _alice_m, bob_m, org) = team(&env).await;

    // Bob runs a task on his machine; Alice, an owner, stops it.
    let (s, v) = bob.call(Method::POST, "/v1/runs", Some(json!({"task": "t", "model": "sonnet", "repo": "proj"}))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(bob_m.claim().await["id"], id.as_str());
    let (s, v) = alice.call(Method::POST, &format!("/v1/runs/{id}/stop"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Alice (owner) sees the whole org.
    let all = alice.audit(&org, "limit=200").await;
    let got = actions(&all);
    for a in ["org.created", "invite.created", "member.joined", "worker_token.created", "machine.enrollment_approved", "machine.enrolled", "run.created", "run.claimed", "run.stopped_by_other"] {
        assert!(got.iter().any(|g| g == a), "{a} missing from {got:?}");
    }
    let stop = find(&all, "run.stopped_by_other");
    assert_eq!((stop["actor"].as_str(), stop["worker_id"].as_str()), (Some("alice"), Some("bob-linux")));
    let joined = find(&all, "member.joined");
    assert_eq!(joined["actor"], "bob");
    let invite = find(&all, "invite.created");
    assert_eq!(invite["detail"]["role"], "member");
    assert!(invite["detail"].get("code").is_none(), "an invite's code is the invite: never logged");
    let enrolled: Vec<&Value> = all["events"].as_array().unwrap().iter().filter(|e| e["action"] == "machine.enrolled").collect();
    assert!(enrolled.iter().any(|e| e["worker_id"] == "bob-linux" && e["detail"]["owner"] == "bob"), "{enrolled:?}");

    // Bob (member) sees what he did and what happened on his machine, not Alice's tokens or invites.
    let mine = bob.audit(&org, "limit=200").await;
    let got = actions(&mine);
    assert!(got.contains(&"run.stopped_by_other".to_string()), "it happened on his machine: {got:?}");
    assert!(got.contains(&"member.joined".to_string()), "{got:?}");
    assert!(!got.contains(&"worker_token.created".to_string()), "{got:?}");
    assert!(!got.contains(&"invite.created".to_string()), "{got:?}");
    assert!(mine["events"].as_array().unwrap().iter().all(|e| e["actor"] == "bob" || e["worker_id"] == "bob-linux"), "{mine}");
    // Filters can't widen it.
    assert!(!actions(&bob.audit(&org, "machine=alice-mac").await).iter().any(|a| a == "machine.enrolled"));
    // His machine's history, whoever caused it; not Alice's.
    let (s, h) = bob.get("/v1/workers/bob-linux/audit").await;
    assert_eq!(s, StatusCode::OK);
    assert!(actions(&h).contains(&"run.stopped_by_other".to_string()));
    assert_eq!(bob.get("/v1/workers/alice-mac/audit").await.0, StatusCode::FORBIDDEN);
    assert_eq!(alice.get("/v1/workers/bob-linux/audit").await.0, StatusCode::OK);
    // Exports follow the same rules.
    let csv = bob.req(Method::GET, &format!("/v1/orgs/{org}/audit/export")).send().await.unwrap().text().await.unwrap();
    assert!(!csv.contains("worker_token.created") && csv.contains("run.stopped_by_other"), "{csv}");

    // Someone outside the org reads nothing of it.
    let mut carol = Api::user(&env, "carol").await;
    assert_eq!(carol.get(&format!("/v1/orgs/{org}/audit")).await.0, StatusCode::NOT_FOUND);
    assert_eq!(carol.get(&format!("/v1/orgs/{org}/audit/export")).await.0, StatusCode::NOT_FOUND);
    carol.org = Some(org.clone());
    assert_eq!(carol.get(&format!("/v1/orgs/{org}/audit")).await.0, StatusCode::FORBIDDEN);
    carol.org = None;
    let me = carol.get("/v1/me").await.1;
    let own = me["orgs"][0]["id"].as_str().unwrap().to_string();
    let theirs = carol.audit(&own, "limit=200").await;
    assert!(theirs["events"].as_array().unwrap().iter().all(|e| e["actor"] == "carol"), "{theirs}");
    // The developer key acts in the default org only.
    let dev = Api::dev(&env);
    assert_eq!(dev.get(&format!("/v1/orgs/{org}/audit")).await.0, StatusCode::NOT_FOUND);
    env.teardown().await;
}
