//! "Ask me": Claude Code's permission prompts, answered on the page.
//!
//! Claude Code runs headless (`claude -p`), so a tool call its permission mode does
//! not allow would be denied. With `--permission-prompt-tool mcp__sidefleet__approve`
//! it calls that MCP tool instead, with `{tool_name, input, tool_use_id}`, and waits
//! for `{"behavior": "allow", "updatedInput": ...}` or `{"behavior": "deny", "message": ...}`
//! as the tool's text.
//!
//! The MCP server is this binary (`sidefleet approval-mcp --socket <path>`), which
//! Claude Code spawns over stdio from `--mcp-config`. It holds no credentials: each
//! question goes over a Unix socket to the worker that started the agent ([`Bridge`]),
//! which asks the control plane ([`ask_page`]) and waits for someone to decide or
//! for the time to run out (then it denies).
//!
//! The same socket carries the org's policies: every Claude Code run gets a `PreToolUse`
//! hook (`sidefleet policy-hook --socket <path>`, see [`hook_settings`]) that asks the
//! worker about each tool call before it runs. The worker checks it with the control
//! plane ([`check_policy`]): denied calls never run, calls a policy holds wait for an
//! approval, and allowed calls go on to Claude Code's own permission mode. When the
//! control plane can't be asked, only reads and the agent's own bookkeeping go ahead.

use std::future::Future;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidefleet_core::policy::{action_digest, classify, unprotected, Decision as Verdict};
use sidefleet_core::{redact_tool_input, Approval, ApprovalRequest, ApprovalStatus};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch;
use uuid::Uuid;

use crate::client::{Backoff, ControlClient};

/// Name of the MCP server in `--mcp-config`, and the prompt tool Claude Code calls.
pub const MCP_SERVER: &str = "sidefleet";
const TOOL: &str = "approve";
pub const PROMPT_TOOL: &str = "mcp__sidefleet__approve";
/// Strings in a tool input sent to the control plane are clipped to this many bytes.
const MAX_INPUT_STRING_BYTES: usize = 4096;

/// What Claude Code asks: may this tool call go ahead?
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub tool_name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// `policy` from the `PreToolUse` hook; absent for "Ask me" permission prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// The `kind` the policy hook sends.
pub const POLICY_KIND: &str = "policy";

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    /// Denied; the message is what the agent is told.
    Deny(String),
    /// The policy check has no objection: Claude Code's own permission mode decides.
    Defer,
}

impl Decision {
    /// The prompt tool's answer. An allowed call runs with its original input: only
    /// the redacted copy ever left the machine.
    pub fn reply(&self, ask: &Ask) -> Value {
        match self {
            Decision::Allow => json!({"behavior": "allow", "updatedInput": ask.input}),
            Decision::Deny(m) => json!({"behavior": "deny", "message": m}),
            Decision::Defer => json!({"behavior": "defer"}),
        }
    }
}

/// The `--mcp-config` that makes Claude Code spawn `<exe> approval-mcp --socket <socket>`.
pub fn mcp_config(exe: &Path, socket: &Path) -> String {
    json!({"mcpServers": {MCP_SERVER: {
        "type": "stdio",
        "command": exe.to_string_lossy(),
        "args": ["approval-mcp", "--socket", socket.to_string_lossy()],
    }}})
    .to_string()
}

/// The socket for a run: in a fresh private folder in the temp dir, short enough for
/// the 104-byte limit on Unix socket paths.
pub fn socket_dir(run_id: Uuid) -> PathBuf {
    std::env::temp_dir().join(format!("sf-ask-{}", &run_id.simple().to_string()[..16]))
}

// ---- worker side ----

/// Listens on a run's socket and answers each question with `decide`. Removes the
/// socket and its folder when dropped.
pub struct Bridge {
    pub socket: PathBuf,
    dir: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Bridge {
    pub fn start<F, Fut>(dir: PathBuf, decide: F) -> std::io::Result<Bridge>
    where
        F: Fn(Ask) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Decision> + Send + 'static,
    {
        use std::os::unix::fs::DirBuilderExt;
        // Left over from a worker that died mid-run; ours, by its name.
        let _ = std::fs::remove_dir_all(&dir);
        // Only this user can reach the socket inside.
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let socket = dir.join("a.sock");
        let listener = tokio::net::UnixListener::bind(&socket)?;
        let decide = Arc::new(decide);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let decide = decide.clone();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut line = String::new();
                    if BufReader::new(read).read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let reply = match serde_json::from_str::<Ask>(&line) {
                        Ok(ask) => decide(ask.clone()).await.reply(&ask),
                        Err(e) => Decision::Deny(format!("Sidefleet could not read the permission request: {e}")).reply(&Ask::default()),
                    };
                    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
                });
            }
        });
        Ok(Bridge { socket, dir, task })
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Ask the person on the page and wait for their decision, the deadline, or a stop.
/// Anything that goes wrong denies: a tool call never runs unapproved.
pub async fn ask_page(client: &ControlClient, run_id: Uuid, ask: &Ask, timeout: Duration, mut stop: watch::Receiver<bool>) -> Decision {
    let req = ApprovalRequest {
        tool_name: ask.tool_name.clone(),
        input: redact_tool_input(&ask.input, MAX_INPUT_STRING_BYTES),
        tool_use_id: ask.tool_use_id.clone(),
        timeout_secs: Some(timeout.as_secs().max(1)),
    };
    let mut backoff = Backoff::new(Duration::from_secs(5));
    let mut attempt = 0;
    let approval = loop {
        match client.request_approval(run_id, &req).await {
            Ok(a) => break a,
            Err(e) if attempt < 4 => {
                attempt += 1;
                tracing::warn!(error = %format!("{e:#}"), %run_id, "asking for approval failed; retrying");
                tokio::time::sleep(backoff.next_delay()).await;
            }
            Err(e) => return Decision::Deny(format!("Sidefleet could not ask the user about this tool call ({e:#}), so it was denied.")),
        }
    };
    tracing::info!(%run_id, approval = %approval.id, tool = %ask.tool_name, "waiting for approval on the page");
    match wait_for(client, run_id, approval.id, timeout, &mut stop).await {
        Ok(_) => Decision::Allow,
        Err(d) => d,
    }
}

/// Poll an approval until it is decided, its deadline passes or the run stops. Ok is the
/// approved approval; Err the denial to give the agent.
async fn wait_for(client: &ControlClient, run_id: Uuid, id: Uuid, timeout: Duration, stop: &mut watch::Receiver<bool>) -> Result<Approval, Decision> {
    // The control plane decides when it expires; the grace covers a slow last poll.
    let deadline = tokio::time::Instant::now() + timeout + Duration::from_secs(5);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return Err(Decision::Deny("The run was stopped.".into()));
                }
            }
        }
        match client.approval(run_id, id).await {
            Ok(a) => match a.status {
                ApprovalStatus::Pending => {}
                ApprovalStatus::Approved => return Ok(a),
                _ => return Err(Decision::Deny(a.message.unwrap_or_else(|| format!("The tool call was {}.", a.status.as_str())))),
            },
            Err(e) => tracing::debug!(error = %format!("{e:#}"), "approval poll failed"),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Decision::Deny(format!("Nobody approved this tool call within {} minutes, so it was denied.", timeout.as_secs().div_ceil(60))));
        }
    }
}

/// The control plane's answer to a policy check.
#[derive(Debug, Deserialize)]
struct PolicyAnswer {
    decision: Verdict,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    approval: Option<Approval>,
}

/// What a tool call gets when its policy can't be checked: reads and the agent's own
/// bookkeeping go ahead (to Claude Code's permission mode), anything else is denied.
pub fn fail_closed(ask: &Ask, workdir: Option<&str>, why: &str) -> Decision {
    if unprotected(&classify(&ask.tool_name, &ask.input, workdir)) {
        Decision::Defer
    } else {
        Decision::Deny(format!("Sidefleet could not check this tool call against the org's policies ({why}), so it was denied."))
    }
}

/// The execution gateway for one tool call of `run_id`: ask the control plane, and wait
/// for an approval when a policy holds the call. Fails closed.
pub async fn check_policy(client: &ControlClient, run_id: Uuid, ask: &Ask, workdir: Option<&str>, timeout: Duration, mut stop: watch::Receiver<bool>) -> Decision {
    let input = redact_tool_input(&ask.input, MAX_INPUT_STRING_BYTES);
    let digest = action_digest(&ask.tool_name, &input);
    let req = json!({
        "tool_name": ask.tool_name, "input": input, "tool_use_id": ask.tool_use_id,
        "workdir": workdir, "timeout_secs": timeout.as_secs().max(1),
    });
    let mut answer = None;
    let mut last_error = String::new();
    for attempt in 0..3 {
        match client.check_policy(run_id, &req).await {
            Ok(v) => {
                answer = Some(v);
                break;
            }
            Err(e) => {
                last_error = format!("{e:#}");
                tracing::warn!(error = %last_error, %run_id, attempt, "policy check failed");
                tokio::time::sleep(Duration::from_millis(300 * (attempt + 1))).await;
            }
        }
    }
    let Some(v) = answer else { return fail_closed(ask, workdir, &last_error) };
    let a: PolicyAnswer = match serde_json::from_value(v) {
        Ok(a) => a,
        Err(e) => return fail_closed(ask, workdir, &format!("unreadable answer: {e}")),
    };
    match a.decision {
        Verdict::Allow => Decision::Defer,
        Verdict::Deny => Decision::Deny(if a.reason.is_empty() { "A Sidefleet policy denied this tool call.".into() } else { a.reason }),
        Verdict::RequireApproval => {
            let Some(held) = a.approval else { return Decision::Deny("A Sidefleet policy needs approval for this call, but no approval was opened.".into()) };
            tracing::info!(%run_id, approval = %held.id, tool = %ask.tool_name, "a policy holds this tool call for approval");
            match wait_for(client, run_id, held.id, timeout, &mut stop).await {
                // Approved for exactly this call, or not at all.
                Ok(ok) if ok.digest.as_deref() == Some(digest.as_str()) => Decision::Allow,
                Ok(_) => Decision::Deny("The approval was for a different action, so this call was denied.".into()),
                Err(d) => d,
            }
        }
    }
}

// ---- the PreToolUse hook Claude Code runs ----

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `--settings` that make Claude Code run `<exe> policy-hook --socket <socket>` before
/// every tool call. `wait` is the longest a call may be held for approval.
pub fn hook_settings(exe: &Path, socket: &Path, wait: Duration) -> String {
    let command = format!("{} policy-hook --socket {}", shell_quote(&exe.to_string_lossy()), shell_quote(&socket.to_string_lossy()));
    json!({"hooks": {"PreToolUse": [{"matcher": "*", "hooks": [{
        "type": "command", "command": command, "timeout": (wait + Duration::from_secs(60)).as_secs(),
    }]}]}})
    .to_string()
}

/// What the hook tells Claude Code.
#[derive(Debug, Clone, PartialEq)]
pub enum HookReply {
    /// Approved on Sidefleet: run it without asking again.
    Allow(String),
    /// Blocked; the reason goes to the agent.
    Deny(String),
    /// No objection: Claude Code's permission mode decides.
    Defer,
}

/// Answer one `PreToolUse` call (its JSON on `input`) by asking the worker on `socket`.
pub fn run_hook(input: &str, socket: &Path) -> HookReply {
    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => return HookReply::Deny(format!("Sidefleet could not read the tool call ({e}), so it was denied.")),
    };
    let ask = Ask {
        tool_name: v.get("tool_name").and_then(Value::as_str).unwrap_or_default().to_string(),
        input: v.get("tool_input").cloned().unwrap_or(json!({})),
        tool_use_id: v.get("tool_use_id").and_then(Value::as_str).map(str::to_string),
        kind: Some(POLICY_KIND.into()),
    };
    let cwd = v.get("cwd").and_then(Value::as_str);
    let asked = (|| -> std::io::Result<Value> {
        let mut s = std::os::unix::net::UnixStream::connect(socket)?;
        writeln!(s, "{}", serde_json::to_string(&ask)?)?;
        let mut line = String::new();
        std::io::BufReader::new(s).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    let reply = match asked {
        Ok(r) => r,
        Err(e) => {
            return match fail_closed(&ask, cwd, &format!("its worker could not be reached: {e}")) {
                Decision::Deny(m) => HookReply::Deny(m),
                _ => HookReply::Defer,
            };
        }
    };
    match reply.get("behavior").and_then(Value::as_str) {
        Some("allow") => HookReply::Allow("Approved on Sidefleet".into()),
        Some("defer") => HookReply::Defer,
        _ => HookReply::Deny(reply.get("message").and_then(Value::as_str).unwrap_or("Sidefleet denied this tool call.").to_string()),
    }
}

/// `sidefleet policy-hook --socket <path>`: read the call on stdin, answer as Claude Code's
/// hooks expect. A block exits 2 with the reason on stderr, which every version honours.
pub fn serve_hook_stdio(socket: &Path) -> i32 {
    let mut input = String::new();
    if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input) {
        eprintln!("Sidefleet could not read the tool call ({e}), so it was denied.");
        return 2;
    }
    match run_hook(&input, socket) {
        HookReply::Allow(reason) => {
            println!("{}", json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow", "permissionDecisionReason": reason}}));
            0
        }
        HookReply::Defer => 0,
        HookReply::Deny(reason) => {
            eprintln!("{reason}");
            2
        }
    }
}

// ---- the MCP server Claude Code spawns ----

/// Serve the prompt tool over MCP (JSON-RPC lines on `input`/`output`) until `input`
/// closes. Each tool call is relayed to the worker on `socket` on its own thread, so
/// questions about parallel tool calls wait side by side.
pub fn serve_mcp(input: impl BufRead, output: Arc<Mutex<dyn Write + Send>>, socket: &Path) -> std::io::Result<()> {
    let send = |out: &Arc<Mutex<dyn Write + Send>>, v: Value| {
        let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(o, "{v}");
        let _ = o.flush();
    };
    let mut threads = Vec::new();
    for line in input.lines() {
        let line = line?;
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue }; // a notification
        let Some(method) = msg.get("method").and_then(Value::as_str) else { continue }; // a response
        let result = match method {
            "initialize" => {
                let version = msg.pointer("/params/protocolVersion").cloned().unwrap_or(json!("2025-06-18"));
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": MCP_SERVER, "version": env!("CARGO_PKG_VERSION")},
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": [{
                "name": TOOL,
                "description": "Asks the user on the Sidefleet page whether a tool call may run.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tool_name": {"type": "string"},
                        "input": {"type": "object"},
                        "tool_use_id": {"type": "string"},
                    },
                    "required": ["tool_name", "input"],
                },
            }]}),
            "tools/call" => {
                let args = msg.pointer("/params/arguments").cloned().unwrap_or(Value::Null);
                let (socket, out) = (socket.to_path_buf(), output.clone());
                threads.push(std::thread::spawn(move || {
                    let reply = relay(&socket, &args);
                    let result = json!({"content": [{"type": "text", "text": reply.to_string()}]});
                    send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
                }));
                continue;
            }
            _ => {
                send(&output, json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method not found: {method}")}}));
                continue;
            }
        };
        send(&output, json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }
    for t in threads {
        let _ = t.join();
    }
    Ok(())
}

/// One question to the worker; a deny when it cannot be asked.
fn relay(socket: &Path, args: &Value) -> Value {
    let ask: Ask = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return json!({"behavior": "deny", "message": format!("Sidefleet could not read the permission request: {e}")}),
    };
    let answer = (|| -> std::io::Result<Value> {
        let mut s = std::os::unix::net::UnixStream::connect(socket)?;
        writeln!(s, "{}", serde_json::to_string(&ask)?)?;
        let mut line = String::new();
        std::io::BufReader::new(s).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    answer.unwrap_or_else(|e| {
        json!({"behavior": "deny", "message": format!("Sidefleet could not reach its worker to ask the user ({e}), so the tool call was denied.")})
    })
}

/// `sidefleet approval-mcp --socket <path>`: serve on stdin/stdout.
pub fn serve_mcp_stdio(socket: &Path) -> std::io::Result<()> {
    serve_mcp(std::io::stdin().lock(), Arc::new(Mutex::new(std::io::stdout())), socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_carry_the_original_input() {
        let ask = Ask { tool_name: "Bash".into(), input: json!({"command": "echo $SECRET_TOKEN"}), ..Default::default() };
        assert_eq!(Decision::Allow.reply(&ask), json!({"behavior": "allow", "updatedInput": {"command": "echo $SECRET_TOKEN"}}));
        assert_eq!(Decision::Deny("no".into()).reply(&ask), json!({"behavior": "deny", "message": "no"}));
    }

    #[test]
    fn socket_paths_fit_the_unix_limit() {
        let p = socket_dir(Uuid::new_v4()).join("a.sock");
        assert!(p.as_os_str().len() < 104, "{}", p.display());
    }

    #[test]
    fn mcp_config_spawns_this_binary() {
        let c: Value = serde_json::from_str(&mcp_config(Path::new("/usr/local/bin/sidefleet"), Path::new("/tmp/sf-ask-1/a.sock"))).unwrap();
        let s = &c["mcpServers"]["sidefleet"];
        assert_eq!(s["command"], "/usr/local/bin/sidefleet");
        assert_eq!(s["args"], json!(["approval-mcp", "--socket", "/tmp/sf-ask-1/a.sock"]));
    }

    /// Claude Code's side of the conversation, over the real socket bridge.
    #[tokio::test]
    async fn the_mcp_server_relays_tool_calls_to_the_worker() {
        let dir = socket_dir(Uuid::new_v4());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        let bridge = Bridge::start(dir.clone(), move |ask: Ask| {
            seen.lock().unwrap().push(ask.clone());
            async move {
                if ask.tool_name == "Bash" { Decision::Allow } else { Decision::Deny("not on the page".into()) }
            }
        })
        .unwrap();
        let socket = bridge.socket.clone();
        let input = [
            json!({"jsonrpc": "2.0", "id": "server-discover-probe-1", "method": "server/discover", "params": {}}),
            json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "approve", "arguments": {"tool_name": "Bash", "input": {"command": "ls"}, "tool_use_id": "toolu_1"}}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "approve", "arguments": {"tool_name": "WebFetch", "input": {"url": "https://example.com"}}}}),
        ]
        .iter()
        .map(|v| v.to_string() + "\n")
        .collect::<String>();
        let out: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let out2: Arc<Mutex<dyn Write + Send>> = out.clone();
        tokio::task::spawn_blocking(move || serve_mcp(std::io::Cursor::new(input), out2, &socket)).await.unwrap().unwrap();

        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let replies: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let by_id = |id: Value| replies.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("no reply to {id}: {text}")).clone();
        assert_eq!(by_id(json!("server-discover-probe-1"))["error"]["code"], -32601);
        assert_eq!(by_id(json!(0))["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(by_id(json!(1))["result"]["tools"][0]["name"], "approve");
        let answer = |id| serde_json::from_str::<Value>(by_id(json!(id))["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(answer(2), json!({"behavior": "allow", "updatedInput": {"command": "ls"}}));
        assert_eq!(answer(3), json!({"behavior": "deny", "message": "not on the page"}));
        assert_eq!(replies.len(), 5, "no reply to the notification: {text}");
        // The two calls are relayed side by side, so either may arrive first.
        let bash = asked.lock().unwrap().iter().find(|a| a.tool_name == "Bash").cloned().expect("the Bash call was asked about");
        assert_eq!(bash.tool_use_id.as_deref(), Some("toolu_1"));

        drop(bridge);
        assert!(!dir.exists(), "the socket folder is removed with the bridge");
    }

    #[test]
    fn the_hook_fails_closed_without_its_worker() {
        let socket = Path::new("/nonexistent/sf-ask/a.sock");
        let call = |tool: &str, input: Value| run_hook(&json!({"tool_name": tool, "tool_input": input, "cwd": "/w"}).to_string(), socket);
        assert_eq!(call("Read", json!({"file_path": "/w/a.rs"})), HookReply::Defer);
        assert!(matches!(call("Bash", json!({"command": "ls"})), HookReply::Deny(_)));
        assert!(matches!(call("Write", json!({"file_path": "/w/a.rs"})), HookReply::Deny(_)));
        assert!(matches!(run_hook("not json", socket), HookReply::Deny(_)));
    }

    /// The hook's side, over the real socket bridge: each answer maps to what Claude Code expects.
    #[tokio::test]
    async fn the_hook_relays_policy_decisions() {
        let dir = socket_dir(Uuid::new_v4());
        let bridge = Bridge::start(dir.clone(), move |ask: Ask| async move {
            assert_eq!(ask.kind.as_deref(), Some(POLICY_KIND));
            match ask.input["command"].as_str() {
                Some("git push origin main") => Decision::Deny("Denied by the Sidefleet policy \"Push to protected branches\".".into()),
                Some("psql") => Decision::Allow,
                _ => Decision::Defer,
            }
        })
        .unwrap();
        let socket = bridge.socket.clone();
        let call = move |cmd: &'static str| {
            let socket = socket.clone();
            tokio::task::spawn_blocking(move || run_hook(&json!({"tool_name": "Bash", "tool_input": {"command": cmd}}).to_string(), &socket))
        };
        assert_eq!(call("ls").await.unwrap(), HookReply::Defer);
        assert_eq!(call("psql").await.unwrap(), HookReply::Allow("Approved on Sidefleet".into()));
        assert!(matches!(call("git push origin main").await.unwrap(), HookReply::Deny(m) if m.contains("Push to protected branches")));
    }

    #[test]
    fn hook_settings_run_this_binary_before_every_tool() {
        let s: Value = serde_json::from_str(&hook_settings(Path::new("/opt/side fleet/sidefleet"), Path::new("/tmp/sf-ask-1/a.sock"), Duration::from_secs(900))).unwrap();
        let h = &s["hooks"]["PreToolUse"][0];
        assert_eq!(h["matcher"], "*");
        assert_eq!(h["hooks"][0]["command"], "'/opt/side fleet/sidefleet' policy-hook --socket '/tmp/sf-ask-1/a.sock'");
        assert_eq!(h["hooks"][0]["timeout"], 960);
    }

    #[test]
    fn without_a_worker_every_call_is_denied() {
        let v = relay(Path::new("/nonexistent/sf-ask/a.sock"), &json!({"tool_name": "Bash", "input": {}}));
        assert_eq!(v["behavior"], "deny");
    }
}
