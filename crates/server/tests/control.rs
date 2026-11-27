//! The agent control plane: the inventory, policies decided at the gateway, approvals
//! bound to the exact action, suspension, the audit chain and tenant isolation. Needs
//! Postgres (docker compose up -d).

use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sidefleet_server::test_support::{TestEnv, DEV_KEY, WORKER_TOKEN};
use uuid::Uuid;

struct Api {
    c: Client,
    base: String,
}

impl Api {
    fn new(env: &TestEnv) -> Api {
        Api { c: Client::new(), base: env.base_url.clone() }
    }
    async fn call(&self, token: &str, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut r = self.c.request(m, format!("{}{path}", self.base)).bearer_auth(token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status();
        let text = r.text().await.unwrap_or_default();
        (s, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    async fn dev(&self, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call(DEV_KEY, m, path, body).await
    }
    async fn ok(&self, m: Method, path: &str, body: Option<Value>) -> Value {
        let (s, v) = self.dev(m.clone(), path, body).await;
        assert!(s.is_success(), "{m} {path}: {s} {v}");
        v
    }
    async fn worker(&self, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call(WORKER_TOKEN, m, path, body).await
    }

    /// Register machine `w`, queue a Claude Code run and have `w` claim it.
    async fn running(&self, w: &str) -> Uuid {
        let (s, _) = self.worker(Method::POST, "/v1/worker/register", Some(json!({"id": w, "display_name": w, "capabilities": {"agents": {"claude": {"version": "2.0.0", "signed_in": true}}, "repos": [{"name": "acme/api", "path": "/tmp/acme-api"}]}, "active_runs": []}))).await;
        assert_eq!(s, StatusCode::OK);
        let run = self.ok(Method::POST, "/v1/runs", Some(json!({"task": "Split invoice lines", "model": "claude-sonnet-5-5", "repo": "acme/api"}))).await;
        let id: Uuid = run["id"].as_str().unwrap().parse().unwrap();
        let (s, claimed) = self.worker(Method::POST, "/v1/worker/claim", Some(json!({"worker_id": w, "repos": ["acme/api"], "busy_repos": [], "agents": ["claude"]}))).await;
        assert_eq!(s, StatusCode::OK, "{claimed}");
        assert_eq!(claimed["id"], id.to_string());
        id
    }

    async fn check(&self, w: &str, run: Uuid, tool: &str, input: Value) -> (StatusCode, Value) {
        self.worker(Method::POST, &format!("/v1/worker/runs/{run}/policy?worker_id={w}"), Some(json!({"tool_name": tool, "input": input, "workdir": "/tmp/acme-api", "timeout_secs": 600}))).await
    }

    async fn agent_named(&self, name: &str) -> Value {
        let list = self.ok(Method::GET, "/v1/agents", None).await;
        list.as_array().unwrap().iter().find(|a| a["name"] == name).cloned().unwrap_or_else(|| panic!("no agent {name} in {list}"))
    }
}

#[tokio::test]
async fn tool_calls_are_decided_at_the_gateway_and_held_ones_wait_for_approval() {
    let env = TestEnv::start().await;
    let api = Api::new(&env);
    let run = api.running("build-02").await;

    // The machine's Claude Code is in the inventory, with nobody owning it.
    let agent = api.agent_named("Claude Code on build-02").await;
    assert_eq!(agent["source"], "discovered");
    assert_eq!(agent["owner"], Value::Null);
    assert_eq!(agent["status"], "running");
    assert_eq!(agent["can_cancel"], true);

    // Defaults: the shell is allowed, protected branches are not, production data waits.
    let (s, v) = api.check("build-02", run, "Bash", json!({"command": "pnpm test"})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["decision"], "allow");
    assert_eq!(v["policy"]["name"], "Shell in worktree");
    let (_, v) = api.check("build-02", run, "Bash", json!({"command": "git push origin main"})).await;
    assert_eq!(v["decision"], "deny");
    assert_eq!(v["policy"]["name"], "Push to protected branches");
    assert!(v["reason"].as_str().unwrap().contains("Push to protected branches"));
    let (_, v) = api.check("build-02", run, "Write", json!({"file_path": "/etc/hosts", "content": "x"})).await;
    assert_eq!(v["decision"], "deny", "writes outside the worktree fall to the default deny: {v}");

    let psql = json!({"command": "psql \"$PROD_DB\" -f migrations/0142.sql", "description": "migrate"});
    let (_, v) = api.check("build-02", run, "Bash", psql.clone()).await;
    assert_eq!(v["decision"], "require_approval", "{v}");
    assert_eq!(v["policy"]["name"], "Production data access");
    let digest = v["digest"].as_str().unwrap().to_string();
    assert!(digest.starts_with("sha256:"));
    assert_eq!(v["approval"]["digest"], digest.as_str(), "the approval is bound to this action");
    let approval = v["approval"]["id"].as_str().unwrap().to_string();

    // The Approvals page shows it, with its context; the agent is waiting.
    let list = api.ok(Method::GET, "/v1/approvals?status=pending", None).await;
    assert_eq!(list["counts"]["pending"], 1);
    let a = &list["approvals"][0];
    assert_eq!(a["id"], approval.as_str());
    assert_eq!(a["kind"], "policy");
    assert_eq!(a["tool"], "shell.exec");
    assert_eq!(a["command"], "psql \"$PROD_DB\" -f migrations/0142.sql");
    assert_eq!(a["context"]["repo"], "acme/api");
    assert_eq!(a["policy"]["name"], "Production data access");
    assert_eq!(a["runtime"], "Claude Code on build-02");
    assert_eq!(a["can_decide"], true);
    assert_eq!(api.agent_named("Claude Code on build-02").await["status"], "waiting");
    // The run's thread sees it like an "Ask me" approval.
    let (_, per_run) = api.dev(Method::GET, &format!("/v1/runs/{run}/approvals"), None).await;
    assert_eq!(per_run[0]["id"], approval.as_str());

    let v = api.ok(Method::POST, &format!("/v1/approvals/{approval}/approve"), None).await;
    assert_eq!(v["status"], "approved");
    assert_eq!(v["decided_by"], "developer key");
    let (_, polled) = api.worker(Method::GET, &format!("/v1/worker/runs/{run}/approvals/{approval}"), None).await;
    assert_eq!(polled["status"], "approved");
    assert_eq!(polled["digest"], digest.as_str());
    let (s, _) = api.dev(Method::POST, &format!("/v1/approvals/{approval}/reject"), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "decided once");

    // A changed command is a new action: it needs its own approval.
    let (_, v) = api.check("build-02", run, "Bash", json!({"command": "psql \"$PROD_DB\" -f migrations/0143.sql", "description": "migrate"})).await;
    assert_eq!(v["decision"], "require_approval");
    assert_ne!(v["digest"], digest.as_str());
    assert_ne!(v["approval"]["id"], approval.as_str());

    // Every decision is in the activity feed; the outcome counts follow the filters.
    let act = api.ok(Method::GET, &format!("/v1/activity?agent={}", agent["id"].as_str().unwrap()), None).await;
    let kinds: Vec<(&str, &str)> = act["events"].as_array().unwrap().iter().map(|e| (e["type"].as_str().unwrap(), e["outcome"].as_str().unwrap())).collect();
    for want in [("task_started", "started"), ("tool_call", "allowed"), ("tool_call", "denied"), ("permission_request", "approval_required"), ("approval_approved", "approved")] {
        assert!(kinds.contains(&want), "{want:?} in {kinds:?}");
    }
    assert_eq!(act["counts"]["denied"], 2);
    let denied = api.ok(Method::GET, "/v1/activity?outcome=denied", None).await;
    assert_eq!(denied["events"].as_array().unwrap().len(), 2);
    assert!(denied["events"].as_array().unwrap().iter().all(|e| e["owner"].is_null()));

    // ...and what the audit needs is in the chain, intact.
    let audit = api.ok(Method::GET, "/v1/audit?limit=500", None).await;
    let events: Vec<&str> = audit["records"].as_array().unwrap().iter().map(|r| r["event"].as_str().unwrap()).collect();
    for want in ["agent.registered", "policy.seeded", "policy.denied", "approval.requested", "approval.approved"] {
        assert!(events.contains(&want), "{want} in {events:?}");
    }
    let approved = audit["records"].as_array().unwrap().iter().find(|r| r["event"] == "approval.approved").unwrap();
    assert_eq!(approved["data"]["decided_by"], "developer key");
    assert!(approved["data"]["approval"].as_str().unwrap().contains(&digest));
    assert_eq!(api.ok(Method::GET, "/v1/audit/verify", None).await["intact"], true);

    // The dashboard counts the decisions.
    let sum = api.ok(Method::GET, "/v1/control/summary", None).await;
    assert_eq!(sum["decisions_24h"]["hours"].as_array().unwrap().len(), 24);
    assert_eq!(sum["decisions_24h"]["denied"], 2);
    assert_eq!(sum["decisions_24h"]["approval"], 2);
    assert_eq!(sum["agents"]["unowned"], 1);
    env.teardown().await;
}

#[tokio::test]
async fn policies_are_evaluated_by_priority_and_changes_are_audited() {
    let env = TestEnv::start().await;
    let api = Api::new(&env);
    let list = api.ok(Method::GET, "/v1/policies", None).await;
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names.first(), Some(&"Destructive production ops"));
    assert_eq!(names.last(), Some(&"Default deny"));

    // The mockup's "try an action": production data wins over the shell policy.
    let v = api.ok(Method::POST, "/v1/policy/evaluate", Some(json!({"tool": "shell.exec", "action": "psql \"$PROD_DB\" -c \"SELECT count(*) FROM invoices\""}))).await;
    assert_eq!(v["decision"], "require_approval");
    assert_eq!(v["policy"]["name"], "Production data access");
    let overridden: Vec<&str> = v["overridden"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(overridden, ["Shell in worktree", "Default deny"]);
    // The try-out records nothing.
    assert_eq!(api.ok(Method::GET, "/v1/activity", None).await["events"].as_array().unwrap().len(), 0);

    // Turn it off: the shell policy decides now.
    let pd = list.as_array().unwrap().iter().find(|p| p["name"] == "Production data access").unwrap()["id"].as_str().unwrap().to_string();
    let p = api.ok(Method::PATCH, &format!("/v1/policies/{pd}"), Some(json!({"active": false}))).await;
    assert_eq!(p["active"], false);
    let v = api.ok(Method::POST, "/v1/policy/evaluate", Some(json!({"tool": "shell.exec", "action": "psql -c 'select 1'"}))).await;
    assert_eq!(v["decision"], "allow");

    // A team policy applies to that team's agents only.
    let reg = api.ok(Method::POST, "/v1/agents", Some(json!({"name": "payments-bot", "team": "Payments", "risk": "high"}))).await;
    let bot = reg["agent"]["id"].as_str().unwrap().to_string();
    let created = api.ok(Method::POST, "/v1/policies", Some(json!({"name": "Payments: no network egress", "scope": "team", "applies_to": "Payments", "tools": ["net.fetch"], "decision": "deny", "priority": 30}))).await;
    let v = api.ok(Method::POST, "/v1/policy/evaluate", Some(json!({"agent_id": bot, "tool_name": "WebFetch", "input": {"url": "https://example.com"}}))).await;
    assert_eq!(v["decision"], "deny");
    assert_eq!(v["policy"]["name"], "Payments: no network egress");
    let v = api.ok(Method::POST, "/v1/policy/evaluate", Some(json!({"tool_name": "WebFetch", "input": {"url": "https://example.com"}}))).await;
    assert_eq!(v["decision"], "allow", "{v}");
    let detail = api.ok(Method::GET, &format!("/v1/agents/{bot}"), None).await;
    assert!(detail["policies"].as_array().unwrap().iter().any(|p| p["name"] == "Payments: no network egress"));
    let perms = detail["permissions"].as_array().unwrap();
    assert_eq!(perms.len(), 8);
    assert_eq!(perms.iter().find(|p| p["tool"] == "git.push").unwrap()["decision"], "deny");

    // Bad policies are refused; the default deny stays.
    let (s, _) = api.dev(Method::POST, "/v1/policies", Some(json!({"name": "x", "scope": "team", "tools": ["*"], "decision": "allow", "priority": 1}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a team policy needs its team");
    let dd = list.as_array().unwrap().iter().find(|p| p["builtin"] == true).unwrap()["id"].as_str().unwrap().to_string();
    let (s, _) = api.dev(Method::DELETE, &format!("/v1/policies/{dd}"), None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = api.dev(Method::DELETE, &format!("/v1/policies/{}", created["id"].as_str().unwrap()), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let audit = api.ok(Method::GET, "/v1/audit?limit=500", None).await;
    let events: Vec<&str> = audit["records"].as_array().unwrap().iter().map(|r| r["event"].as_str().unwrap()).collect();
    for want in ["policy.deactivated", "policy.created", "policy.deleted", "agent.registered"] {
        assert!(events.contains(&want), "{want} in {events:?}");
    }
    let search = api.ok(Method::GET, "/v1/audit?q=egress", None).await;
    assert_eq!(search["total"], 2, "created and deleted: {search}");
    env.teardown().await;
}

#[tokio::test]
async fn suspending_an_agent_stops_it_and_reactivating_lets_it_work_again() {
    let env = TestEnv::start().await;
    let api = Api::new(&env);
    let run = api.running("build-01").await;
    let (_, held) = api.check("build-01", run, "Bash", json!({"command": "terraform apply prod.tfplan"})).await;
    assert_eq!(held["decision"], "require_approval");
    let approval = held["approval"]["id"].as_str().unwrap().to_string();
    let agent = api.agent_named("Claude Code on build-01").await;
    let id = agent["id"].as_str().unwrap().to_string();
    assert_eq!(agent["pending_approvals"], 1);

    let v = api.ok(Method::POST, &format!("/v1/agents/{id}/suspend"), Some(json!({"reason": "Unexpected writes to the ledger table"}))).await;
    assert_eq!(v["agent"]["status"], "suspended");
    assert_eq!(v["agent"]["suspension"]["reason"], "Unexpected writes to the ledger table");
    assert_eq!(v["incident"]["cancelled_approvals"], 1);
    assert_eq!(v["incident"]["cancelled_runs"], 1);
    assert_eq!(v["incident"]["not_stopped"], Value::Null);
    let (s, _) = api.dev(Method::POST, &format!("/v1/agents/{id}/suspend"), Some(json!({"reason": "again"}))).await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Its approval is cancelled and its run told to stop.
    let (_, a) = api.worker(Method::GET, &format!("/v1/worker/runs/{run}/approvals/{approval}"), None).await;
    assert_eq!(a["status"], "cancelled");
    assert_eq!(a["message"], "The agent was suspended.");
    let (_, ctl) = api.worker(Method::GET, &format!("/v1/worker/runs/{run}/control"), None).await;
    assert_eq!(ctl["stop_requested"], true);

    // The machine gets no new tasks for it.
    let (s, _) = api.worker(Method::POST, &format!("/v1/worker/runs/{run}/finish?worker_id=build-01"), Some(json!({"status": "stopped", "result": {}}))).await;
    assert_eq!(s, StatusCode::OK);
    let next = api.ok(Method::POST, "/v1/runs", Some(json!({"task": "again", "model": "claude-sonnet-5-5", "repo": "acme/api"}))).await;
    let (s, _) = api.worker(Method::POST, "/v1/worker/claim", Some(json!({"worker_id": "build-01", "repos": ["acme/api"], "busy_repos": [], "agents": ["claude"]}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "a suspended agent claims nothing");
    // And the evaluator says why.
    let v = api.ok(Method::POST, "/v1/policy/evaluate", Some(json!({"agent_id": id, "tool": "fs.read", "action": "README.md"}))).await;
    assert_eq!(v["decision"], "deny");
    assert!(v["reason"].as_str().unwrap().contains("agent suspended"));
    let detail = api.ok(Method::GET, &format!("/v1/agents/{id}"), None).await;
    assert!(detail["permissions"].as_array().unwrap().iter().all(|p| p["decision"] == "deny"));

    let v = api.ok(Method::POST, &format!("/v1/agents/{id}/reactivate"), None).await;
    assert_eq!(v["status"], "idle");
    let (s, claimed) = api.worker(Method::POST, "/v1/worker/claim", Some(json!({"worker_id": "build-01", "repos": ["acme/api"], "busy_repos": [], "agents": ["claude"]}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(claimed["id"], next["id"]);

    // The incident stays in its history.
    let detail = api.ok(Method::GET, &format!("/v1/agents/{id}"), None).await;
    let inc = &detail["incidents"][0];
    assert_eq!(inc["reason"], "Unexpected writes to the ledger table");
    assert_eq!(inc["reactivated_by"], "developer key");
    let audit = api.ok(Method::GET, &format!("/v1/audit?agent={id}&limit=500"), None).await;
    let events: Vec<&str> = audit["records"].as_array().unwrap().iter().map(|r| r["event"].as_str().unwrap()).collect();
    for want in ["agent.suspended", "approval.cancelled", "task.cancelled", "agent.reactivated"] {
        assert!(events.contains(&want), "{want} in {events:?}");
    }
    env.teardown().await;
}

#[tokio::test]
async fn custom_agents_use_the_gateway_with_their_key() {
    let env = TestEnv::start().await;
    let api = Api::new(&env);
    let (s, reg) = api.dev(Method::POST, "/v1/agents", Some(json!({"name": "support-triage-bot", "runtime": "Kubernetes", "host": "k8s-agents", "model": "claude-haiku-5-5", "provider": "Anthropic", "team": "Support Eng"}))).await;
    assert_eq!(s, StatusCode::CREATED, "{reg}");
    let key = reg["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("sfk_"));
    let id = reg["agent"]["id"].as_str().unwrap().to_string();
    assert_eq!(reg["agent"]["can_cancel"], false);
    assert_eq!(reg["agent"]["credentials"].as_array().unwrap().len(), 1);

    let gw = |m: Method, path: &str, body: Option<Value>| {
        let key = key.clone();
        let api = &api;
        let path = path.to_string();
        async move { api.call(&key, m, &path, body).await }
    };
    let (s, _) = gw(Method::POST, "/v1/gateway/events", Some(json!({"type": "task_started", "action": "Triage the overnight queue"}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(api.agent_named("support-triage-bot").await["status"], "running");

    let (s, v) = gw(Method::POST, "/v1/gateway/evaluate", Some(json!({"tool": "secrets.read", "action": "ZENDESK_API_TOKEN", "context": {"task": "Triage"}}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["decision"], "require_approval");
    let approval = v["approval"]["id"].as_str().unwrap().to_string();
    let (_, p) = gw(Method::GET, &format!("/v1/gateway/approvals/{approval}"), None).await;
    assert_eq!(p["status"], "pending");
    let list = api.ok(Method::GET, "/v1/approvals?status=pending", None).await;
    assert_eq!(list["approvals"][0]["run_id"], Value::Null);
    assert_eq!(list["approvals"][0]["context"]["task"], "Triage");
    let v = api.ok(Method::POST, &format!("/v1/approvals/{approval}/reject"), Some(json!({"message": "Use the read-only token"}))).await;
    assert_eq!(v["status"], "denied");
    let (_, p) = gw(Method::GET, &format!("/v1/gateway/approvals/{approval}"), None).await;
    assert_eq!(p["status"], "denied");
    assert_eq!(p["message"], "Use the read-only token");

    // Suspending revokes the key; Sidefleet can't stop its process and says so.
    let v = api.ok(Method::POST, &format!("/v1/agents/{id}/suspend"), Some(json!({"reason": "Leaked a ticket"}))).await;
    assert_eq!(v["incident"]["revoked"].as_array().unwrap().len(), 1);
    assert!(v["incident"]["not_stopped"].as_str().unwrap().contains("outside a Sidefleet worker"), "{v}");
    let (s, _) = gw(Method::POST, "/v1/gateway/evaluate", Some(json!({"tool": "fs.read", "action": "x"}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = api.dev(Method::POST, &format!("/v1/agents/{id}/key"), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "no new key while suspended");
    api.ok(Method::POST, &format!("/v1/agents/{id}/reactivate"), None).await;
    let fresh = api.ok(Method::POST, &format!("/v1/agents/{id}/key"), None).await["key"].as_str().unwrap().to_string();
    let (s, v) = api.call(&fresh, Method::POST, "/v1/gateway/evaluate", Some(json!({"tool": "fs.read", "action": "README.md"}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["decision"], "allow");
    let (s, _) = api.call(&key, Method::POST, "/v1/gateway/evaluate", Some(json!({"tool": "fs.read", "action": "x"}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "the old key stays revoked");
    env.teardown().await;
}

#[tokio::test]
async fn the_audit_log_is_append_only_and_tampering_breaks_the_chain() {
    let env = TestEnv::start().await;
    let api = Api::new(&env);
    api.ok(Method::GET, "/v1/policies", None).await;
    for name in ["a", "b", "c"] {
        api.ok(Method::POST, "/v1/agents", Some(json!({"name": name}))).await;
    }
    let v = api.ok(Method::GET, "/v1/audit/verify", None).await;
    assert_eq!(v["intact"], true);
    assert_eq!(v["records"], 4);

    // The database refuses edits and deletions.
    assert!(sqlx::query("UPDATE audit_log SET detail = 'nothing to see'").execute(&env.pool).await.is_err());
    assert!(sqlx::query("DELETE FROM audit_log").execute(&env.pool).await.is_err());

    let csv = env_text(&api, "/v1/audit/export?format=csv").await;
    assert!(csv.starts_with("seq,id,at,actor"));
    assert_eq!(csv.lines().count(), 5);
    let jsonl = env_text(&api, "/v1/audit/export?format=jsonl").await;
    let first: Value = serde_json::from_str(jsonl.lines().next().unwrap()).unwrap();
    assert_eq!(first["event"], "policy.seeded");
    assert_eq!(first["prev_hash"], "0".repeat(64));

    // Someone with the database's owner role edits a record behind the log's back.
    let mut tx = env.pool.begin().await.unwrap();
    sqlx::query("ALTER TABLE audit_log DISABLE TRIGGER audit_log_no_update").execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE audit_log SET detail = 'nothing to see' WHERE event = 'agent.registered' AND target = 'b'").execute(&mut *tx).await.unwrap();
    sqlx::query("ALTER TABLE audit_log ENABLE TRIGGER audit_log_no_update").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let v = api.ok(Method::GET, "/v1/audit/verify", None).await;
    assert_eq!(v["intact"], false);
    let b_seq: i64 = sqlx::query_scalar("SELECT seq FROM audit_log WHERE target = 'b'").fetch_one(&env.pool).await.unwrap();
    assert_eq!(v["broken_at"], b_seq);
    env.teardown().await;
}

async fn env_text(api: &Api, path: &str) -> String {
    let r = api.c.get(format!("{}{path}", api.base)).bearer_auth(DEV_KEY).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers()["content-disposition"].to_str().unwrap().starts_with("attachment"));
    r.text().await.unwrap()
}

struct Browser {
    c: Client,
    base: String,
    cookie: String,
    org: String,
}

impl Browser {
    async fn login(env: &TestEnv, login: &str) -> Browser {
        let cookie = env.github_login(login).await;
        let mut b = Browser { c: Client::new(), base: env.base_url.clone(), cookie, org: String::new() };
        b.org = b.json(Method::GET, "/v1/me", None).await.1["orgs"][0]["id"].as_str().unwrap().to_string();
        b
    }
    async fn json(&self, m: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut r = self.c.request(m, format!("{}{path}", self.base)).header("cookie", format!("sf_session={}", self.cookie));
        if !self.org.is_empty() {
            r = r.header("x-sidefleet-org", &self.org);
        }
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn orgs_are_isolated_and_only_owners_govern() {
    let env = TestEnv::start_hosted().await;
    let alice = Browser::login(&env, "alice").await;
    let bob = Browser::login(&env, "bob").await;
    let _carol = Browser::login(&env, "carol").await;
    let (s, _) = alice.json(Method::POST, &format!("/v1/orgs/{}/members", alice.org), Some(json!({"login": "carol", "role": "member"}))).await;
    assert!(s.is_success());
    let mut carol = Browser::login(&env, "carol").await;
    carol.org = alice.org.clone();

    let (_, reg) = alice.json(Method::POST, "/v1/agents", Some(json!({"name": "alice-bot"}))).await;
    let id = reg["agent"]["id"].as_str().unwrap().to_string();
    assert_eq!(reg["agent"]["owner"]["login"], "alice");

    // Bob's org sees none of it.
    for path in [format!("/v1/agents/{id}"), "/v1/agents".into(), "/v1/audit".into(), "/v1/activity".into()] {
        let (s, v) = bob.json(Method::GET, &path, None).await;
        match s {
            StatusCode::NOT_FOUND => {}
            StatusCode::OK => assert!(!v.to_string().contains("alice-bot"), "{path}: {v}"),
            s => panic!("{path}: {s}"),
        }
    }
    let (s, _) = bob.json(Method::POST, &format!("/v1/agents/{id}/suspend"), Some(json!({"reason": "x"}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "bob owns his own org, not alice's agent");
    let (s, _) = bob.json(Method::PATCH, &format!("/v1/agents/{id}"), Some(json!({"name": "mine"}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let alice_policies = alice.json(Method::GET, "/v1/policies", None).await.1;
    let pid = alice_policies[0]["id"].as_str().unwrap();
    let (s, _) = bob.json(Method::PATCH, &format!("/v1/policies/{pid}"), Some(json!({"active": false}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let custom = reg["key"].as_str().unwrap().to_string();
    let (_, held) = env_gateway(&env, &custom, json!({"tool": "secrets.read", "action": "TOKEN"})).await;
    let approval = held["approval"]["id"].as_str().unwrap().to_string();
    let (s, _) = bob.json(Method::POST, &format!("/v1/approvals/{approval}/approve"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Carol is a member of alice's org: she reads, but can't change policies or suspend.
    let (s, v) = carol.json(Method::GET, "/v1/agents", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v.to_string().contains("alice-bot"));
    let (s, _) = carol.json(Method::PATCH, &format!("/v1/policies/{pid}"), Some(json!({"active": false}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = carol.json(Method::POST, &format!("/v1/agents/{id}/suspend"), Some(json!({"reason": "x"}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = carol.json(Method::GET, "/v1/approvals?status=pending", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["approvals"][0]["can_decide"], false, "not her agent");
    let (s, _) = carol.json(Method::POST, &format!("/v1/approvals/{approval}/approve"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Alice owns the agent and the org: she decides.
    let (s, v) = alice.json(Method::POST, &format!("/v1/approvals/{approval}/approve"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["decided_by"], "alice");
    env.teardown().await;
}

async fn env_gateway(env: &TestEnv, key: &str, body: Value) -> (StatusCode, Value) {
    let r = Client::new().post(format!("{}/v1/gateway/evaluate", env.base_url)).bearer_auth(key).json(&body).send().await.unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}
