//! Multiplayer end to end: invite links, machines that only run their owner's
//! tasks, who may steer a thread, and rooms (tasks with a shared brief, merged
//! into one room branch). Needs Postgres (docker compose up -d).

use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sidefleet_server::test_support::TestEnv;

struct Api {
    c: Client,
    base: String,
    cookie: Option<String>,
    org: Option<String>,
}

impl Api {
    async fn user(env: &TestEnv, login: &str) -> Api {
        Api { c: Client::new(), base: env.base_url.clone(), cookie: Some(env.github_login(login).await), org: None }
    }
    fn anon(env: &TestEnv) -> Api {
        Api { c: Client::new(), base: env.base_url.clone(), cookie: None, org: None }
    }
    async fn call(&self, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut r = self.c.request(m, format!("{}{}", self.base, path));
        if let Some(o) = &self.org {
            r = r.header("x-sidefleet-org", o);
        }
        if let Some(c) = &self.cookie {
            r = r.header("cookie", format!("sf_session={c}"));
        }
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn me(&self) -> Value {
        self.call(Method::GET, "/v1/me", None).await.1
    }
    async fn task(&self, path: &str, task: &str, extra: Value) -> (StatusCode, Value) {
        let mut body = json!({"task": task, "model": "sonnet", "repo": "proj"});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.call(Method::POST, path, Some(body)).await
    }
}

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
        let (s, v) = m.call("/v1/worker/register", json!({"id": id, "display_name": id, "capabilities": caps, "active_runs": []})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        m
    }
    async fn call(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let r = self.c.post(format!("{}{}", self.base, path)).bearer_auth(&self.token).json(&body).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    /// The run this machine gets, if any.
    async fn claim(&self) -> Option<Value> {
        let kinds = json!(["agent", "restore", "fork", "pr", "integrate"]);
        let (s, v) = self.call("/v1/worker/claim", json!({"worker_id": self.id, "repos": ["proj"], "busy_repos": [], "agents": ["claude"], "kinds": kinds})).await;
        match s {
            StatusCode::OK => Some(v),
            StatusCode::NO_CONTENT => None,
            s => panic!("claim: {s} {v}"),
        }
    }
    async fn finish(&self, run: &str, result: Value) {
        let (s, _) = self
            .call(&format!("/v1/worker/runs/{run}/events?worker_id={}", self.id), json!({"events": [{"kind": "agent_init", "payload": {"session_id": format!("s-{run}")}}]}))
            .await;
        assert_eq!(s, StatusCode::OK);
        let (s, v) = self.call(&format!("/v1/worker/runs/{run}/finish?worker_id={}", self.id), json!({"status": "succeeded", "result": result})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
}

/// Alice's team org with Bob in it (joined through an invite link), each with a machine of their own.
async fn team(env: &TestEnv) -> (Api, Api, Machine, Machine) {
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

    // Alice mints a token for her machine; Bob's joins by approving it on the page.
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
    (alice, bob, alice_m, bob_m)
}

#[tokio::test]
async fn invite_links_bring_people_in_once_and_only_while_valid() {
    let env = TestEnv::start_hosted().await;
    let (alice, bob, _, _) = team(&env).await;
    let org = alice.org.clone().unwrap();

    let (s, members) = bob.call(Method::GET, &format!("/v1/orgs/{org}/members"), None).await;
    assert_eq!(s, StatusCode::OK);
    let logins: Vec<&str> = members.as_array().unwrap().iter().map(|m| m["login"].as_str().unwrap()).collect();
    assert_eq!(logins, ["alice", "bob"]);
    assert!(members[1]["id"].is_string());

    // Members can't invite; a one-use link works once; anyone can read a link before signing in.
    assert_eq!(bob.call(Method::POST, &format!("/v1/orgs/{org}/invites"), None).await.0, StatusCode::FORBIDDEN);
    let (_, inv) = alice.call(Method::POST, &format!("/v1/orgs/{org}/invites"), Some(json!({"max_uses": 1}))).await;
    let code = inv["code"].as_str().unwrap();
    let (s, info) = Api::anon(&env).call(Method::GET, &format!("/v1/invites/{}", code.to_lowercase()), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((info["org_name"].as_str(), info["inviter_login"].as_str(), info["valid"].as_bool()), (Some("Acme"), Some("alice"), Some(true)));
    assert_eq!(Api::anon(&env).call(Method::POST, &format!("/v1/invites/{code}/accept"), None).await.0, StatusCode::UNAUTHORIZED);

    let carol = Api::user(&env, "carol").await;
    assert_eq!(carol.call(Method::POST, &format!("/v1/invites/{code}/accept"), None).await.0, StatusCode::OK);
    assert_eq!(carol.call(Method::POST, &format!("/v1/invites/{code}/accept"), None).await.0, StatusCode::OK, "joining again is fine");
    let dave = Api::user(&env, "dave").await;
    assert_eq!(dave.call(Method::POST, &format!("/v1/invites/{code}/accept"), None).await.0, StatusCode::GONE);
    assert_eq!(Api::anon(&env).call(Method::GET, &format!("/v1/invites/{code}"), None).await.1["reason"], "used_up");

    // Revoked links stop working.
    let (_, inv2) = alice.call(Method::POST, &format!("/v1/orgs/{org}/invites"), None).await;
    let id2 = inv2["invite"]["id"].as_str().unwrap();
    assert_eq!(alice.call(Method::DELETE, &format!("/v1/orgs/{org}/invites/{id2}"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(dave.call(Method::POST, &format!("/v1/invites/{}/accept", inv2["code"].as_str().unwrap()), None).await.0, StatusCode::GONE);

    env.teardown().await;
}

#[tokio::test]
async fn machines_only_run_their_owners_tasks() {
    let env = TestEnv::start_hosted().await;
    let (alice, bob, alice_m, bob_m) = team(&env).await;

    let (_, workers) = bob.call(Method::GET, "/v1/workers", None).await;
    let mine: Vec<(&str, bool, &str)> = workers
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["id"].as_str().unwrap(), w["mine"].as_bool().unwrap(), w["owner"]["login"].as_str().unwrap()))
        .collect();
    assert!(mine.contains(&("alice-mac", false, "alice")) && mine.contains(&("bob-linux", true, "bob")), "{mine:?}");

    // Bob's task never goes to Alice's machine, even when she asks first.
    let (s, v) = bob.task("/v1/runs", "Bob's task", json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let bob_run = v["id"].as_str().unwrap().to_string();
    assert!(alice_m.claim().await.is_none());
    let claimed = bob_m.claim().await.expect("bob's machine runs bob's task");
    assert_eq!(claimed["id"], bob_run.as_str());
    assert_eq!(claimed["created_by"], bob.me().await["user"]["id"]);

    // Pinning to a teammate's machine is refused, and so is managing it.
    let (s, v) = bob.task("/v1/runs", "sneaky", json!({"worker_id": "alice-mac"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(bob.call(Method::DELETE, "/v1/workers/alice-mac", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob.call(Method::POST, "/v1/workers/alice-mac/update", None).await.0, StatusCode::FORBIDDEN);

    // Alice can read Bob's thread, but not steer it; she continues it on her own machine instead.
    bob_m.finish(&bob_run, json!({"text": "done"})).await;
    assert_eq!(alice.call(Method::GET, &format!("/v1/runs/{bob_run}/thread"), None).await.0, StatusCode::OK);
    let (s, v) = alice.call(Method::POST, &format!("/v1/runs/{bob_run}/messages"), Some(json!({"task": "also this"}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(v["error"].as_str().unwrap().contains("@bob"), "{v}");
    assert_eq!(alice.call(Method::POST, &format!("/v1/runs/{bob_run}/pr"), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(alice.call(Method::POST, &format!("/v1/runs/{bob_run}/fork"), Some(json!({"task": "try it my way", "model": "sonnet", "worker_id": "bob-linux"}))).await.0, StatusCode::FORBIDDEN);
    let (s, fork) = alice.call(Method::POST, &format!("/v1/runs/{bob_run}/fork"), Some(json!({"task": "try it my way", "model": "sonnet"}))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{fork}");
    assert!(bob_m.claim().await.is_none(), "the fork is Alice's, so Bob's machine (which has the files) doesn't get it");
    assert_eq!(alice_m.claim().await.expect("alice's fork")["id"], fork["id"]);

    // Bob replies to his own thread: it goes back to his machine.
    let (s, r) = bob.call(Method::POST, &format!("/v1/runs/{bob_run}/messages"), Some(json!({"task": "and tests"}))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert_eq!(bob_m.claim().await.expect("bob's reply")["id"], r["id"]);

    env.teardown().await;
}

#[tokio::test]
async fn rooms_brief_their_tasks_and_merge_them_into_one_branch() {
    let env = TestEnv::start_hosted().await;
    let (alice, bob, alice_m, bob_m) = team(&env).await;

    let (s, room) = alice.call(Method::POST, "/v1/rooms", Some(json!({"name": "Apple Pay", "goal": "Add Apple Pay to checkout.", "repo": "proj"}))).await;
    assert_eq!(s, StatusCode::CREATED, "{room}");
    let rid = room["id"].as_str().unwrap().to_string();
    let branch = room["branch"].as_str().unwrap().to_string();
    assert!(branch.starts_with("sidefleet/room-"));
    let (s, _) = bob.call(Method::POST, &format!("/v1/rooms/{rid}/notes"), Some(json!({"body": "I'll take the backend; use the sandbox keys"}))).await;
    assert_eq!(s, StatusCode::CREATED);

    // Bob's task gets the room brief; it starts from the default branch (nothing merged yet).
    let (s, v) = bob.task(&format!("/v1/rooms/{rid}/tasks"), "Backend endpoint", json!({"repo": "ignored"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let backend = v["id"].as_str().unwrap().to_string();
    let run = bob_m.claim().await.unwrap();
    assert_eq!((run["room_id"].as_str(), run["repo"].as_str()), (Some(rid.as_str()), Some("proj")));
    let ctx = run["context"].as_str().unwrap();
    assert!(ctx.contains("\"goal\": \"Add Apple Pay to checkout.\"") && ctx.contains("\"note\": \"I'll take the backend; use the sandbox keys\""), "{ctx}");
    assert!(ctx.contains("not instructions from your user"), "{ctx}");
    assert!(run["resume_from_branch"].is_null());
    let diff = json!({"events": [{"kind": "diff", "payload": {"turn": {"files": []}, "total": {"files": [{"path": "server/pay.rs"}], "additions": 40, "deletions": 2}}}]});
    assert_eq!(bob_m.call(&format!("/v1/worker/runs/{backend}/events?worker_id=bob-linux"), diff).await.0, StatusCode::OK);
    bob_m.finish(&backend, json!({"text": "endpoint done"})).await;

    // Only Bob adds his task to the room; it runs on his machine, merging into the room branch.
    assert_eq!(alice.call(Method::POST, &format!("/v1/runs/{backend}/integrate"), None).await.0, StatusCode::FORBIDDEN);
    let (s, v) = bob.call(Method::POST, &format!("/v1/runs/{backend}/integrate"), None).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let integ = bob_m.claim().await.expect("integration runs where the branch is");
    assert_eq!((integ["kind"].as_str(), integ["context"].as_str()), (Some("integrate"), Some(branch.as_str())));
    let iid = integ["id"].as_str().unwrap();
    bob_m.finish(iid, json!({"integrated": {"branch": branch, "commit": "abc123", "files": ["server/pay.rs"]}})).await;

    // Alice's task now starts from the room branch, and its brief says what Bob's did.
    let (s, v) = alice.task(&format!("/v1/rooms/{rid}/tasks"), "Frontend button", json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let run = alice_m.claim().await.unwrap();
    assert_eq!(run["resume_from_branch"], branch.as_str());
    let ctx = run["context"].as_str().unwrap();
    assert!(ctx.contains("\"title\": \"Backend endpoint\"") && ctx.contains("\"state\": \"merged into the room branch\"") && ctx.contains("\"server/pay.rs\""), "{ctx}");

    let (s, v) = alice.call(Method::GET, &format!("/v1/rooms/{rid}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((v["room"]["task_count"].as_i64(), v["room"]["integrated_count"].as_i64()), (Some(2), Some(1)));
    assert!(v["room"]["integrated_at"].is_string());
    let card = v["tasks"].as_array().unwrap().iter().find(|t| t["id"] == backend.as_str()).unwrap();
    assert_eq!(card["created_by_login"], "bob");
    assert_eq!(card["files"], json!(["server/pay.rs"]));
    assert_eq!((card["integration"]["status"].as_str(), card["integration"]["commit"].as_str()), (Some("succeeded"), Some("abc123")));
    assert_eq!(v["notes"][0]["author_login"], "bob");

    // Only a note's author edits it.
    let bob_note = v["notes"][0]["id"].as_str().unwrap().to_string();
    let edit = json!({"body": "I'll take the backend; use the live keys"});
    assert_eq!(alice.call(Method::PATCH, &format!("/v1/rooms/{rid}/notes/{bob_note}"), Some(edit.clone())).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob.call(Method::PATCH, &format!("/v1/rooms/{rid}/notes/{bob_note}"), Some(json!({"body": "  "}))).await.0, StatusCode::BAD_REQUEST);
    let (s, n) = bob.call(Method::PATCH, &format!("/v1/rooms/{rid}/notes/{bob_note}"), Some(edit)).await;
    assert_eq!(s, StatusCode::OK, "{n}");
    assert!(n["edited_at"].is_string());
    let (_, v) = alice.call(Method::GET, &format!("/v1/rooms/{rid}"), None).await;
    assert_eq!(v["notes"][0]["body"], "I'll take the backend; use the live keys");

    // A note can be deleted by its author or the room's creator, not by anyone else; later briefs lose it.
    let (_, n) = alice.call(Method::POST, &format!("/v1/rooms/{rid}/notes"), Some(json!({"body": "scratch that"}))).await;
    let alice_note = n["id"].as_str().unwrap().to_string();
    assert_eq!(bob.call(Method::DELETE, &format!("/v1/rooms/{rid}/notes/{alice_note}"), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(alice.call(Method::DELETE, &format!("/v1/rooms/{rid}/notes/{alice_note}"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(alice.call(Method::DELETE, &format!("/v1/rooms/{rid}/notes/{alice_note}"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(alice.call(Method::DELETE, &format!("/v1/rooms/{rid}/notes/{bob_note}"), None).await.0, StatusCode::NO_CONTENT);
    let (_, v) = alice.call(Method::GET, &format!("/v1/rooms/{rid}"), None).await;
    assert_eq!(v["notes"], json!([]));
    let (_, rooms) = bob.call(Method::GET, "/v1/rooms", None).await;
    assert_eq!(rooms.as_array().unwrap().len(), 1);

    // Only the creator (or an owner) renames or archives it; archived rooms take no new tasks.
    assert_eq!(bob.call(Method::PATCH, &format!("/v1/rooms/{rid}"), Some(json!({"name": "mine now"}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(alice.call(Method::PATCH, &format!("/v1/rooms/{rid}"), Some(json!({"archived": true}))).await.0, StatusCode::OK);
    assert_eq!(bob.task(&format!("/v1/rooms/{rid}/tasks"), "late", json!({})).await.0, StatusCode::CONFLICT);

    env.teardown().await;
}

#[tokio::test]
async fn removing_someone_cuts_off_their_machines_and_queued_tasks() {
    let env = TestEnv::start_hosted().await;
    let (alice, bob, _, bob_m) = team(&env).await;
    let org = alice.org.clone().unwrap();
    let (_, v) = bob.task("/v1/runs", "queued work", json!({})).await;
    let queued = v["id"].as_str().unwrap().to_string();
    let bob_id = bob.me().await["user"]["id"].as_str().unwrap().to_string();

    assert_eq!(bob.call(Method::DELETE, &format!("/v1/orgs/{org}/members/{}", alice.me().await["user"]["id"].as_str().unwrap()), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(alice.call(Method::DELETE, &format!("/v1/orgs/{org}/members/{bob_id}"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(alice.call(Method::GET, &format!("/v1/runs/{queued}"), None).await.1["status"], "stopped");
    let (s, _) = bob_m.call("/v1/worker/heartbeat", json!({"id": "bob-linux", "capabilities": {}, "active_runs": []})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // The last owner can't leave.
    let alice_id = alice.me().await["user"]["id"].as_str().unwrap().to_string();
    assert_eq!(alice.call(Method::DELETE, &format!("/v1/orgs/{org}/members/{alice_id}"), None).await.0, StatusCode::CONFLICT);

    env.teardown().await;
}

#[tokio::test]
async fn teammates_cannot_approve_archive_or_act_as_your_machine() {
    let env = TestEnv::start_hosted().await;
    let (alice, bob, alice_m, bob_m) = team(&env).await;
    let (_, v) = alice.task("/v1/runs", "migrate", json!({"permission_mode": "ask"})).await;
    let run = v["id"].as_str().unwrap().to_string();
    assert_eq!(alice_m.claim().await.unwrap()["id"], run.as_str());
    let (s, a) = alice_m.call(&format!("/v1/worker/runs/{run}/approvals?worker_id=alice-mac"), json!({"tool_name": "Bash", "input": {"command": "rm -rf build"}})).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let aid = a["id"].as_str().unwrap();

    // Only Alice decides what runs on her machine.
    assert_eq!(bob.call(Method::POST, &format!("/v1/runs/{run}/approvals/{aid}/approve"), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob.call(Method::POST, &format!("/v1/runs/{run}/approvals/{aid}/deny"), Some(json!({"message": "do this instead"}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob.call(Method::POST, &format!("/v1/runs/{run}/archive"), Some(json!({"archived": true}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(alice.call(Method::POST, &format!("/v1/runs/{run}/approvals/{aid}/approve"), None).await.0, StatusCode::OK);

    // Bob's machine token can't speak for Alice's machine, nor take over its id.
    let as_alice = |p: &str| format!("/v1/worker/runs/{run}/{p}?worker_id=alice-mac");
    assert_eq!(bob_m.call(&as_alice("events"), json!({"events": [{"kind": "agent_init", "payload": {"session_id": "evil"}}]})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob_m.call(&as_alice("finish"), json!({"status": "succeeded", "result": {}})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob_m.call(&as_alice("approvals"), json!({"tool_name": "Bash", "input": {}})).await.0, StatusCode::FORBIDDEN);
    let (s, _) = bob_m.call("/v1/worker/register", json!({"id": "alice-mac", "display_name": "alice-mac", "capabilities": {}, "active_runs": []})).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(alice.call(Method::GET, &format!("/v1/runs/{run}"), None).await.1["status"], "running", "Alice's run survived");
    let (_, workers) = alice.call(Method::GET, "/v1/workers", None).await;
    let mac = workers.as_array().unwrap().iter().find(|w| w["id"] == "alice-mac").unwrap().clone();
    assert_eq!(mac["owner"]["login"], "alice");

    // Bob's machine can't put a PR link on Alice's task either.
    bob_m.call("/v1/worker/prs", json!({"worker_id": "bob-linux", "prs": [{"thread": run, "pr": {"url": "https://evil.example/pull/1", "number": 1, "state": "open"}}]})).await;
    assert!(alice.call(Method::GET, &format!("/v1/runs/{run}"), None).await.1["pr"].is_null());

    env.teardown().await;
}

#[tokio::test]
async fn removing_someone_also_revokes_the_invites_they_made() {
    let env = TestEnv::start_hosted().await;
    let (alice, bob, _, _) = team(&env).await;
    let org = alice.org.clone().unwrap();
    // Bob is made an owner, mints an open owner link, and is then removed.
    let bob_id = bob.me().await["user"]["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE org_members SET role = 'owner' WHERE user_id = $1::uuid").bind(&bob_id).execute(&env.pool).await.unwrap();
    let (s, inv) = bob.call(Method::POST, &format!("/v1/orgs/{org}/invites"), Some(json!({"role": "owner"}))).await;
    assert_eq!(s, StatusCode::CREATED, "{inv}");
    assert_eq!(alice.call(Method::DELETE, &format!("/v1/orgs/{org}/members/{bob_id}"), None).await.0, StatusCode::NO_CONTENT);
    let code = inv["code"].as_str().unwrap();
    assert_eq!(bob.call(Method::POST, &format!("/v1/invites/{code}/accept"), None).await.0, StatusCode::GONE);
    env.teardown().await;
}
