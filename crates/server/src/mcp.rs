//! Sidefleet as an MCP server, so a chat agent (Hermes, OpenClaw, Claude) can
//! run and follow tasks on the fleet: `POST /mcp` with an API token, Streamable
//! HTTP answered with plain JSON (no sessions, no server-sent stream).
//!
//! Every tool calls the developer API in-process with the caller's own
//! credentials, so a tool can do exactly what the token's scope allows on the
//! API, with the same checks and plan limits, and nothing more.

use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

use crate::auth::{Caller, ORG_HEADER};
use crate::tenancy::Scope;
use crate::AppState;

const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// How long `task_status` with `wait` follows a task before answering anyway;
/// well inside the usual 60-120 s tool-call timeouts.
const WAIT_SECS: u64 = 45;

const INSTRUCTIONS: &str = "Sidefleet runs coding agents (Claude Code, Codex, opencode) on the user's own machines. \
Use fleet_overview to see machines, repos and recent tasks. start_task gives an agent a task in a repo; tasks take minutes, \
so tell the user it started and follow it with task_status (wait: true waits up to 45 s for it to finish). \
reply continues a task's conversation. When a task waits for approval to run a command, ask the user, then approve or deny. \
open_pull_request opens a PR from a finished task. Every task has a link to its page; share it when useful.";

/// The tools, and the scope each needs.
fn tools() -> Vec<(Scope, Value)> {
    let task_id = json!({ "type": "string", "description": "The task's id (from start_task, list_tasks or fleet_overview)." });
    vec![
        (
            Scope::Read,
            json!({
                "name": "fleet_overview",
                "title": "Fleet overview",
                "description": "The user's machines (online or not, which agents and repos they have, how busy they are) and the most recent tasks. Start here.",
                "inputSchema": { "type": "object", "properties": {} },
                "annotations": { "readOnlyHint": true }
            }),
        ),
        (
            Scope::Read,
            json!({
                "name": "list_tasks",
                "title": "List tasks",
                "description": "Recent tasks, newest activity first, with their status.",
                "inputSchema": { "type": "object", "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "How many (default 10)." }
                } },
                "annotations": { "readOnlyHint": true }
            }),
        ),
        (
            Scope::Read,
            json!({
                "name": "task_status",
                "title": "Task status",
                "description": "Where a task stands: status, the agent's latest reply, files changed, commands waiting for approval, its pull request and page link.",
                "inputSchema": { "type": "object", "required": ["task_id"], "properties": {
                    "task_id": task_id,
                    "wait": { "type": "boolean", "description": "Wait up to 45 seconds for a running task to finish or ask for approval before answering." }
                } },
                "annotations": { "readOnlyHint": true }
            }),
        ),
        (
            Scope::Tasks,
            json!({
                "name": "start_task",
                "title": "Start a task",
                "description": "Give a coding agent a task in one of the user's repos. It runs on a machine that has the repo, in its own git branch, and starts within seconds when a machine is free.",
                "inputSchema": { "type": "object", "required": ["task"], "properties": {
                    "task": { "type": "string", "description": "What to do, in plain words, as you would tell a developer." },
                    "repo": { "type": "string", "description": "Which repo, e.g. side-fleet or foodaka/side-fleet. Optional when the fleet has only one." },
                    "machine": { "type": "string", "description": "Run on this machine (its name). Optional: the least busy machine with the repo." },
                    "agent": { "type": "string", "enum": ["claude", "codex", "opencode"], "description": "Which coding agent (default claude)." },
                    "model": { "type": "string", "description": "Model for that agent: for claude one of sonnet (default), opus, fable, haiku." },
                    "access": { "type": "string", "enum": ["edit", "ask", "read_only", "full"],
                        "description": "edit (default): reads and edits files, refuses shell commands. ask: edits files and asks for approval before commands (claude only). read_only: looks and answers, changes nothing. full: also runs commands without asking." }
                } }
            }),
        ),
        (
            Scope::Tasks,
            json!({
                "name": "reply",
                "title": "Reply to a task",
                "description": "Send the agent a follow-up message in the same task, like replying in a chat. If it is still working, the message waits its turn.",
                "inputSchema": { "type": "object", "required": ["task_id", "message"], "properties": {
                    "task_id": task_id,
                    "message": { "type": "string" }
                } }
            }),
        ),
        (
            Scope::Tasks,
            json!({
                "name": "stop_task",
                "title": "Stop a task",
                "description": "Stop the task's agent if it is running or waiting to start. Its changes so far stay on its branch.",
                "inputSchema": { "type": "object", "required": ["task_id"], "properties": { "task_id": task_id } },
                "annotations": { "destructiveHint": true }
            }),
        ),
        (
            Scope::Tasks,
            json!({
                "name": "open_pull_request",
                "title": "Open a pull request",
                "description": "Commit and push the finished task's branch and open a GitHub pull request for it (or find the open one). Takes a minute; task_status then shows the link.",
                "inputSchema": { "type": "object", "required": ["task_id"], "properties": { "task_id": task_id } }
            }),
        ),
        (
            Scope::Full,
            json!({
                "name": "approve",
                "title": "Approve a command",
                "description": "Let a task go ahead with a command or tool call it is waiting on (see pending approvals in task_status). Only after the user says yes.",
                "inputSchema": { "type": "object", "required": ["task_id", "approval_id"], "properties": {
                    "task_id": task_id,
                    "approval_id": { "type": "string" }
                } }
            }),
        ),
        (
            Scope::Full,
            json!({
                "name": "deny",
                "title": "Deny a command",
                "description": "Refuse a command or tool call a task is waiting on. The agent is told the reason and carries on without it.",
                "inputSchema": { "type": "object", "required": ["task_id", "approval_id"], "properties": {
                    "task_id": task_id,
                    "approval_id": { "type": "string" },
                    "reason": { "type": "string", "description": "What to tell the agent instead." }
                } }
            }),
        ),
    ]
}

/// What the caller may use: an API token's scope, or everything for the page and the developer key.
fn allowed(caller: &Caller, need: Scope) -> bool {
    caller.scope.is_none_or(|s| s >= need)
}

/// `GET /mcp` would open a server-sent stream; this server sends none.
pub async fn get() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

/// `POST /mcp`: one JSON-RPC message (or a batch). Notifications get `202`.
pub async fn post(State(state): State<AppState>, Extension(caller): Extension<Caller>, headers: HeaderMap, body: Bytes) -> Response {
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(rpc_error(Value::Null, -32700, "parse error")).into_response(),
    };
    let api = Api { state: &state, headers: &headers };
    let out = match msg {
        Value::Array(msgs) => {
            let mut out = Vec::new();
            for m in msgs {
                if let Some(r) = handle(&api, &caller, m).await {
                    out.push(r);
                }
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
        m => handle(&api, &caller, m).await,
    };
    match out {
        Some(v) => Json(v).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

async fn handle(api: &Api<'_>, caller: &Caller, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned()?; // a notification: nothing to answer
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match msg.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = if PROTOCOL_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSIONS[0] };
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "sidefleet", "title": "Sidefleet", "version": api.state.config.build },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools().into_iter().filter(|(s, _)| allowed(caller, *s)).map(|(_, t)| t).collect::<Vec<_>>() }),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let (text, is_error) = match tools().into_iter().find(|(_, t)| t["name"] == name) {
                None => (format!("There is no tool named {name}."), true),
                Some((need, _)) if !allowed(caller, need) => {
                    (format!("This connection can't use {name}: its access is limited. The user can give it more on the Sidefleet Machines page."), true)
                }
                Some(_) => match call_tool(api, name, &args).await {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                },
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
        }
        other => return Some(rpc_error(id, -32601, &format!("method not found: {other}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// The developer API, called in-process as the caller.
struct Api<'a> {
    state: &'a AppState,
    headers: &'a HeaderMap,
}

impl Api<'_> {
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let mut req = Request::builder().method(method).uri(path);
        for name in [header::AUTHORIZATION, header::COOKIE, header::HOST] {
            if let Some(v) = self.headers.get(&name) {
                req = req.header(name, v.clone());
            }
        }
        if let Some(v) = self.headers.get(ORG_HEADER) {
            req = req.header(ORG_HEADER, v.clone());
        }
        let body = match body {
            Some(b) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let req = req.body(body).map_err(|e| e.to_string())?;
        let res = match crate::routes::router(self.state.clone()).oneshot(req).await {
            Ok(r) => r,
            Err(never) => match never {},
        };
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 16 * 1024 * 1024).await.map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).trim().to_string()));
        if status.is_success() {
            return Ok(v);
        }
        let msg = v.get("error").and_then(Value::as_str).map(str::to_string).or_else(|| v.as_str().map(str::to_string)).unwrap_or_default();
        Err(if msg.is_empty() { format!("Sidefleet answered {status}.") } else { msg })
    }
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// An id from the agent, checked before it goes into a path.
fn id_arg(args: &Value, key: &str) -> Result<Uuid, String> {
    let raw = arg(args, key).ok_or_else(|| format!("{key} is required."))?;
    Uuid::parse_str(raw).map_err(|_| format!("{key} {raw:?} is not a Sidefleet id; list_tasks shows them."))
}

async fn call_tool(api: &Api<'_>, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "fleet_overview" => {
            let workers = api.call(Method::GET, "/v1/workers", None).await?;
            let runs = api.call(Method::GET, "/v1/runs?limit=8", None).await?;
            Ok(format!("{}\n\n{}", machines_text(&workers), tasks_text(&runs, "Recent tasks")))
        }
        "list_tasks" => {
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(10).clamp(1, 50);
            let runs = api.call(Method::GET, &format!("/v1/runs?limit={limit}"), None).await?;
            Ok(tasks_text(&runs, "Tasks"))
        }
        "task_status" => {
            let id = id_arg(args, "task_id")?;
            let path = format!("/v1/runs/{id}/summary");
            let mut s = api.call(Method::GET, &path, None).await?;
            if args.get("wait").and_then(Value::as_bool).unwrap_or(false) {
                let until = tokio::time::Instant::now() + Duration::from_secs(WAIT_SECS);
                while busy(&s) && tokio::time::Instant::now() < until {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    s = api.call(Method::GET, &path, None).await?;
                }
            }
            Ok(summary_text(&s))
        }
        "start_task" => {
            let task = arg(args, "task").ok_or("task is required.")?;
            let workers = api.call(Method::GET, "/v1/workers", None).await?;
            let repo = pick_repo(&workers, arg(args, "repo"))?;
            let machine = match arg(args, "machine") {
                None => None,
                Some(m) => Some(pick_machine(&workers, m)?),
            };
            let agent = arg(args, "agent").unwrap_or("claude");
            let model = arg(args, "model").unwrap_or(if agent == "claude" { "sonnet" } else { "" });
            let mode = match arg(args, "access").unwrap_or("edit") {
                "edit" => "acceptEdits",
                "ask" => "ask",
                "read_only" => "plan",
                "full" => "bypassPermissions",
                other => return Err(format!("access {other:?} must be edit, ask, read_only or full.")),
            };
            let mut body = json!({ "task": task, "repo": repo, "agent": agent, "model": model, "permission_mode": mode });
            if let Some(m) = machine {
                body["worker_id"] = json!(m);
            }
            let r = api.call(Method::POST, "/v1/runs", Some(body)).await?;
            let id = r["id"].as_str().unwrap_or("");
            let s = api.call(Method::GET, &format!("/v1/runs/{id}/summary"), None).await.ok();
            let url = s.as_ref().and_then(|s| s["url"].as_str()).unwrap_or("");
            Ok(format!(
                "Started task {id} in {} with {}. It starts as soon as a machine with the repo is free and usually takes a few minutes.\n\
                 Follow it with task_status (wait: true). Page: {url}",
                repo_label(&repo),
                agent_name(agent)
            ))
        }
        "reply" => {
            let id = id_arg(args, "task_id")?;
            let message = arg(args, "message").ok_or("message is required.")?;
            api.call(Method::POST, &format!("/v1/runs/{id}/messages"), Some(json!({ "task": message }))).await?;
            Ok("Sent. The agent picks it up in the same conversation; follow it with task_status (wait: true).".into())
        }
        "stop_task" => {
            let id = id_arg(args, "task_id")?;
            let s = api.call(Method::GET, &format!("/v1/runs/{id}/summary"), None).await?;
            if !busy(&s) && s["status"] != "queued" {
                return Ok(format!("Nothing to stop: the task is {}.", status_word(&s)));
            }
            let latest = s["latest_message"]["id"].as_str().unwrap_or_default();
            api.call(Method::POST, &format!("/v1/runs/{latest}/stop"), None).await?;
            Ok("Stopping it. Its changes so far stay on its branch.".into())
        }
        "open_pull_request" => {
            let id = id_arg(args, "task_id")?;
            api.call(Method::POST, &format!("/v1/runs/{id}/pr"), None).await?;
            Ok("Opening a pull request: the machine commits and pushes the branch, then opens it. Check task_status in a minute for the link.".into())
        }
        "approve" | "deny" => {
            let id = id_arg(args, "task_id")?;
            let approval = id_arg(args, "approval_id")?;
            let s = api.call(Method::GET, &format!("/v1/runs/{id}/summary"), None).await?;
            let latest = s["latest_message"]["id"].as_str().unwrap_or_default();
            let body = arg(args, "reason").map(|r| json!({ "message": r }));
            api.call(Method::POST, &format!("/v1/runs/{latest}/approvals/{approval}/{name}"), body).await?;
            Ok(if name == "approve" { "Approved: the agent goes ahead." } else { "Denied: the agent is told and carries on without it." }.into())
        }
        _ => Err(format!("There is no tool named {name}.")),
    }
}

/// Still working, with nothing waiting on the user.
fn busy(s: &Value) -> bool {
    matches!(s["status"].as_str(), Some("queued" | "running")) && s["pending_approvals"].as_array().is_none_or(|a| a.is_empty())
}

fn agent_name(a: &str) -> &str {
    match a {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "opencode",
        other => other,
    }
}

/// `github.com/foodaka/side-fleet` reads as `foodaka/side-fleet`.
fn repo_label(key: &str) -> String {
    let parts: Vec<&str> = key.split('/').collect();
    if parts.len() >= 3 && parts[0].contains('.') { parts[1..].join("/") } else { key.to_string() }
}

/// Every repo the machines have, by identity (what runs are matched on).
fn repos(workers: &Value) -> Vec<String> {
    let mut out: Vec<String> = workers
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|w| w["capabilities"]["repos"].as_array().cloned().unwrap_or_default())
        .filter_map(|r| r["id"].as_str().filter(|s| !s.is_empty()).or(r["name"].as_str()).map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn pick_repo(workers: &Value, wanted: Option<&str>) -> Result<String, String> {
    let all = repos(workers);
    let list = || all.iter().map(|r| repo_label(r)).collect::<Vec<_>>().join(", ");
    let Some(w) = wanted else {
        return match all.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err("No machine has a repo yet. Connect a machine on the Sidefleet page first.".into()),
            _ => Err(format!("Which repo? The machines have: {}.", list())),
        };
    };
    let w = w.trim_end_matches(".git").to_lowercase();
    let short = |r: &str| r.rsplit('/').next().unwrap_or(r).to_lowercase();
    let exact: Vec<&String> = all.iter().filter(|r| r.to_lowercase() == w || repo_label(r).to_lowercase() == w).collect();
    let found = if exact.is_empty() { all.iter().filter(|r| short(r) == short(&w)).collect() } else { exact };
    match found.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("No machine has a repo called {w}. The machines have: {}.", list())),
        many => Err(format!("{w} could be {}. Say which.", many.iter().map(|r| repo_label(r)).collect::<Vec<_>>().join(" or "))),
    }
}

fn pick_machine(workers: &Value, wanted: &str) -> Result<String, String> {
    let ws = workers.as_array().cloned().unwrap_or_default();
    let w = wanted.to_lowercase();
    ws.iter()
        .find(|m| m["id"].as_str().is_some_and(|id| id.to_lowercase() == w) || m["display_name"].as_str().is_some_and(|n| n.to_lowercase() == w))
        .and_then(|m| m["id"].as_str().map(str::to_string))
        .ok_or_else(|| format!("No machine called {wanted}. The machines are: {}.", ws.iter().filter_map(|m| m["display_name"].as_str()).collect::<Vec<_>>().join(", ")))
}

fn machines_text(workers: &Value) -> String {
    let ws = workers.as_array().cloned().unwrap_or_default();
    if ws.is_empty() {
        return "No machines connected yet. The user adds one on the Sidefleet page (Machines, Add machine).".into();
    }
    let online = ws.iter().filter(|w| w["online"] == true).count();
    let mut out = format!("Machines ({online} of {} online):", ws.len());
    for w in &ws {
        let c = &w["capabilities"];
        let name = w["display_name"].as_str().or(w["id"].as_str()).unwrap_or("?");
        let state = if w["online"] == true {
            match (c["slots"]["active"].as_i64(), c["slots"]["parallel"].as_i64()) {
                (Some(a), Some(p)) => format!("online, {a} of {p} task slots busy"),
                _ => "online".into(),
            }
        } else {
            "offline".into()
        };
        let agents: Vec<&str> = ["claude", "codex", "opencode"].into_iter().filter(|a| !c["agents"][*a].is_null()).map(agent_name).collect();
        let repos: Vec<String> = c["repos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["id"].as_str().filter(|s| !s.is_empty()).or(r["name"].as_str()).map(repo_label))
            .collect();
        out.push_str(&format!("\n- {name} ({state}). Agents: {}. Repos: {}.", or_none(&agents.join(", ")), or_none(&repos.join(", "))));
        if let Some(l) = w["limited_until"].as_object().filter(|l| !l.is_empty()) {
            let l: Vec<String> = l.iter().map(|(a, t)| format!("{} until {}", agent_name(a), t.as_str().unwrap_or("?"))).collect();
            out.push_str(&format!(" Usage limit: {}.", l.join(", ")));
        }
    }
    out
}

fn or_none(s: &str) -> &str {
    if s.is_empty() { "none" } else { s }
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > max { format!("{}…", line.chars().take(max).collect::<String>()) } else { line.to_string() }
}

fn tasks_text(runs: &Value, heading: &str) -> String {
    let rs = runs.as_array().cloned().unwrap_or_default();
    if rs.is_empty() {
        return format!("{heading}: none yet.");
    }
    let mut out = format!("{heading}:");
    for r in &rs {
        let mut status = r["thread_status"].as_str().unwrap_or("?").to_string();
        if r["pending_approvals"].as_i64().unwrap_or(0) > 0 {
            status = "waiting for approval".into();
        }
        let machine = r["worker_id"].as_str().map(|m| format!(", on {m}")).unwrap_or_default();
        out.push_str(&format!(
            "\n- [{status}] {} ({}{machine}) id {}",
            first_line(r["task"].as_str().unwrap_or(""), 90),
            repo_label(r["repo"].as_str().unwrap_or("")),
            r["id"].as_str().unwrap_or("")
        ));
    }
    out
}

fn status_word(s: &Value) -> String {
    match s["status"].as_str().unwrap_or("") {
        "queued" => "waiting for a machine".into(),
        "succeeded" => "done".into(),
        other => other.to_string(),
    }
}

fn summary_text(s: &Value) -> String {
    let mut out = format!("Task: {} (id {})\n", first_line(s["task"].as_str().unwrap_or(""), 120), s["id"].as_str().unwrap_or(""));
    let machine = s["machine"].as_str().map(|m| format!(" on {m}")).unwrap_or_default();
    let model = s["model"].as_str().filter(|m| !m.is_empty()).map(|m| format!(" ({m})")).unwrap_or_default();
    out.push_str(&format!(
        "Status: {}{machine}. Repo {}, {}{model}. {} message(s).\n",
        status_word(s),
        repo_label(s["repo"].as_str().unwrap_or("")),
        agent_name(s["agent"].as_str().unwrap_or("")),
        s["messages"]
    ));
    if let Some(e) = s["error"].as_str() {
        out.push_str(&format!("Error: {e}\n"));
    }
    if let Some(pending) = s["pending_approvals"].as_array().filter(|a| !a.is_empty()) {
        out.push_str("Waiting for approval (ask the user, then approve or deny):\n");
        for a in pending {
            out.push_str(&format!("- approval_id {}: {} {}\n", a["id"].as_str().unwrap_or(""), a["tool"].as_str().unwrap_or("?"), tool_input(&a["input"])));
        }
    }
    if let Some(r) = s["reply"].as_str() {
        out.push_str(&format!("Latest reply from the agent:\n{r}\n"));
    }
    let c = &s["changed_files"];
    if c["count"].as_i64().unwrap_or(0) > 0 {
        let files: Vec<String> = c["files"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| format!("{} (+{} -{})", f["path"].as_str().unwrap_or("?"), f["additions"], f["deletions"]))
            .collect();
        out.push_str(&format!("Changed files ({}, +{} -{}): {}\n", c["count"], c["additions"], c["deletions"], files.join(", ")));
    }
    if let Some(url) = s["pr"]["url"].as_str() {
        out.push_str(&format!("Pull request #{} ({}): {url}\n", s["pr"]["number"], s["pr"]["state"].as_str().unwrap_or("")));
    }
    out.push_str(&format!("Page: {}", s["url"].as_str().unwrap_or("")));
    out
}

/// A tool call's input in one line: the command or path when there is one.
fn tool_input(input: &Value) -> String {
    for k in ["command", "file_path", "url", "pattern", "path"] {
        if let Some(v) = input[k].as_str() {
            return format!("`{}`", first_line(v, 200));
        }
    }
    let s = input.to_string();
    if s.chars().count() > 200 { format!("{}…", s.chars().take(200).collect::<String>()) } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fleet() -> Value {
        json!([
            { "id": "mac", "display_name": "Mac mini", "online": true, "capabilities": { "repos": [
                { "name": "code/side-fleet", "id": "github.com/foodaka/side-fleet" },
                { "name": "notes", "id": "" }
            ] } },
            { "id": "vps", "display_name": "vps", "online": false, "capabilities": { "repos": [
                { "name": "side-fleet", "id": "github.com/foodaka/side-fleet" },
                { "name": "site", "id": "github.com/other/side-fleet" }
            ] } }
        ])
    }

    #[test]
    fn repos_are_found_by_any_reasonable_name() {
        let f = fleet();
        assert_eq!(pick_repo(&f, Some("foodaka/side-fleet")).unwrap(), "github.com/foodaka/side-fleet");
        assert_eq!(pick_repo(&f, Some("github.com/foodaka/side-fleet.git")).unwrap(), "github.com/foodaka/side-fleet");
        assert_eq!(pick_repo(&f, Some("notes")).unwrap(), "notes");
        // Two repos end in side-fleet: ask.
        assert!(pick_repo(&f, Some("side-fleet")).unwrap_err().contains("foodaka/side-fleet or other/side-fleet"));
        assert!(pick_repo(&f, None).unwrap_err().starts_with("Which repo?"));
        assert!(pick_repo(&f, Some("nope")).unwrap_err().contains("No machine has a repo called nope"));
        let one = json!([{ "id": "m", "capabilities": { "repos": [{ "name": "a", "id": "" }] } }]);
        assert_eq!(pick_repo(&one, None).unwrap(), "a");
    }

    #[test]
    fn machines_are_found_by_id_or_name() {
        let f = fleet();
        assert_eq!(pick_machine(&f, "mac mini").unwrap(), "mac");
        assert_eq!(pick_machine(&f, "VPS").unwrap(), "vps");
        assert!(pick_machine(&f, "laptop").unwrap_err().contains("Mac mini, vps"));
    }

    #[test]
    fn ids_never_reach_a_path_unchecked() {
        assert!(id_arg(&json!({ "task_id": "../../setup" }), "task_id").is_err());
        assert!(id_arg(&json!({}), "task_id").is_err());
        let id = Uuid::new_v4();
        assert_eq!(id_arg(&json!({ "task_id": id.to_string() }), "task_id").unwrap(), id);
    }
}
