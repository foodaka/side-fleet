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

use std::future::Future;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidefleet_core::{redact_tool_input, ApprovalRequest, ApprovalStatus};
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub tool_name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub tool_use_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    /// Denied; the message is what the agent is told.
    Deny(String),
}

impl Decision {
    /// The prompt tool's answer. An allowed call runs with its original input: only
    /// the redacted copy ever left the machine.
    pub fn reply(&self, ask: &Ask) -> Value {
        match self {
            Decision::Allow => json!({"behavior": "allow", "updatedInput": ask.input}),
            Decision::Deny(m) => json!({"behavior": "deny", "message": m}),
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
                        Err(e) => Decision::Deny(format!("Sidefleet could not read the permission request: {e}")).reply(&Ask {
                            tool_name: String::new(),
                            input: Value::Null,
                            tool_use_id: None,
                        }),
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
    // The control plane decides when it expires; the grace covers a slow last poll.
    let deadline = tokio::time::Instant::now() + timeout + Duration::from_secs(5);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return Decision::Deny("The run was stopped.".into());
                }
            }
        }
        match client.approval(run_id, approval.id).await {
            Ok(a) => match a.status {
                ApprovalStatus::Pending => {}
                ApprovalStatus::Approved => return Decision::Allow,
                _ => return Decision::Deny(a.message.unwrap_or_else(|| format!("The tool call was {}.", a.status.as_str()))),
            },
            Err(e) => tracing::debug!(error = %format!("{e:#}"), "approval poll failed"),
        }
        if tokio::time::Instant::now() >= deadline {
            return Decision::Deny(format!("Nobody approved this tool call within {} minutes, so it was denied.", timeout.as_secs().div_ceil(60)));
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
        let ask = Ask { tool_name: "Bash".into(), input: json!({"command": "echo $SECRET_TOKEN"}), tool_use_id: None };
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
        assert_eq!(asked.lock().unwrap()[0].tool_use_id.as_deref(), Some("toolu_1"));

        drop(bridge);
        assert!(!dir.exists(), "the socket folder is removed with the bridge");
    }

    #[test]
    fn without_a_worker_every_call_is_denied() {
        let v = relay(Path::new("/nonexistent/sf-ask/a.sock"), &json!({"tool_name": "Bash", "input": {}}));
        assert_eq!(v["behavior"], "deny");
    }
}
