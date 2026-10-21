//! Mapping of Codex `codex exec --json` lines to run events.
//!
//! A line is one JSON object (see tests/fixtures/codex_exec.jsonl, captured
//! from codex-cli 0.162):
//!
//! * `{"type":"thread.started","thread_id":...}`: the session, resumable with `codex exec resume <id>`
//! * `{"type":"item.started"|"item.completed","item":{"id":...,"type":"agent_message"|"reasoning"|"command_execution"|"file_change"|"mcp_tool_call"|"web_search"|"todo_list"|"error",...}}`
//! * `{"type":"turn.completed","usage":{...}}` / `{"type":"turn.failed","error":{"message":...}}`
//! * `{"type":"error","message":...}`: a stream error (often a retry)
//!
//! Tool calls become the same `tool_use` / `tool_result` events Claude Code
//! produces, named like Claude Code's tools (Bash, Edit, ...) so the page
//! renders them the same way.

use std::collections::HashSet;
use std::time::Instant;

use crate::agent::relative_path;
use crate::redact::{redact, redact_json, summarize_tool_input};
use crate::stream::{truncate_bytes, MappedLine, TOOL_RESULT_MAX_BYTES};
use crate::NewEvent;
use serde_json::{json, Value};

pub struct CodexStream {
    model: String,
    cwd: String,
    started: Instant,
    session_id: Option<String>,
    last_text: String,
    announced: HashSet<String>,
    turns: u32,
}

impl CodexStream {
    pub fn new(model: &str, cwd: &str) -> Self {
        CodexStream {
            model: model.to_string(),
            cwd: cwd.to_string(),
            started: Instant::now(),
            session_id: None,
            last_text: String::new(),
            announced: HashSet::new(),
            turns: 0,
        }
    }

    pub fn map_line(&mut self, line: &str) -> MappedLine {
        let line = line.trim();
        if line.is_empty() {
            return MappedLine::Events(vec![]);
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return MappedLine::Raw(truncate_bytes(&redact(line), TOOL_RESULT_MAX_BYTES)),
        };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        let mut out = Vec::new();
        match ty {
            "thread.started" => {
                self.session_id = v.get("thread_id").and_then(Value::as_str).map(str::to_string);
                out.push(NewEvent {
                    kind: "agent_init".into(),
                    payload: json!({
                        "agent": "codex",
                        "model": if self.model.is_empty() { Value::Null } else { json!(self.model) },
                        "session_id": self.session_id,
                        "tools": Value::Null,
                        "cwd": self.cwd,
                    }),
                });
            }
            "turn.started" => self.turns += 1,
            "item.started" | "item.updated" => {
                if let Some(item) = v.get("item") {
                    if ty == "item.started" {
                        self.announce(item, &mut out);
                    }
                }
            }
            "item.completed" => {
                if let Some(item) = v.get("item") {
                    self.completed(item, &mut out);
                }
            }
            "turn.completed" => out.push(self.result(false, &self.last_text.clone(), v.get("usage").map(usage).as_ref())),
            "turn.failed" => {
                let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("Codex reported an error");
                out.push(self.result(true, &error_text(msg), None));
            }
            "error" => {
                let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
                out.push(NewEvent {
                    kind: "stderr".into(),
                    payload: json!({"text": truncate_bytes(&redact(&error_text(msg)), 2048), "stream": "agent"}),
                });
            }
            _ => out.push(NewEvent { kind: "agent_other".into(), payload: json!({"type": ty}) }),
        }
        MappedLine::Events(out)
    }

    fn result(&self, is_error: bool, text: &str, usage: Option<&Value>) -> NewEvent {
        NewEvent {
            kind: "result".into(),
            payload: json!({
                "subtype": if is_error { "error" } else { "success" },
                "is_error": is_error,
                "result": redact(text),
                "duration_ms": self.started.elapsed().as_millis() as u64,
                "num_turns": self.turns.max(1),
                "total_cost_usd": Value::Null,
                "usage": usage.cloned().unwrap_or(Value::Null),
                "session_id": self.session_id,
            }),
        }
    }

    /// A `tool_use` for an item that is a tool call, once per item id.
    fn announce(&mut self, item: &Value, out: &mut Vec<NewEvent>) {
        let Some((name, summary)) = self.tool_of(item) else { return };
        let id = item.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if !self.announced.insert(id.clone()) {
            return;
        }
        out.push(NewEvent { kind: "tool_use".into(), payload: json!({"id": id, "name": name, "summary": summary}) });
    }

    fn completed(&mut self, item: &Value, out: &mut Vec<NewEvent>) {
        let s = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "agent_message" => {
                let text = s("text");
                if !text.trim().is_empty() {
                    self.last_text = text.clone();
                    out.push(NewEvent { kind: "assistant_text".into(), payload: json!({"text": redact(&text)}) });
                }
            }
            "reasoning" => out.push(NewEvent { kind: "assistant_thinking".into(), payload: json!({}) }),
            "error" => out.push(NewEvent {
                kind: "stderr".into(),
                payload: json!({"text": truncate_bytes(&redact(&s("message")), 2048), "stream": "agent"}),
            }),
            "command_execution" | "file_change" | "mcp_tool_call" | "web_search" | "todo_list" => {
                self.announce(item, out);
                let status = s("status");
                let (content, mut is_error) = self.output_of(item);
                is_error |= matches!(status.as_str(), "failed" | "declined");
                let truncated = content.len() > TOOL_RESULT_MAX_BYTES;
                out.push(NewEvent {
                    kind: "tool_result".into(),
                    payload: json!({
                        "tool_use_id": s("id"),
                        "is_error": is_error,
                        "content": truncate_bytes(&redact(&content), TOOL_RESULT_MAX_BYTES),
                        "truncated": truncated,
                    }),
                });
            }
            other => out.push(NewEvent { kind: "agent_other".into(), payload: json!({"type": other}) }),
        }
    }

    /// The Claude Code-style tool name and one-line summary for a tool item.
    fn tool_of(&self, item: &Value) -> Option<(String, String)> {
        let s = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Some(match item.get("type").and_then(Value::as_str)? {
            "command_execution" => ("Bash".into(), summarize_tool_input("Bash", &json!({"command": unwrap_shell(&s("command"))}))),
            "file_change" => {
                let paths: Vec<String> = changes(item).iter().map(|(_, p)| relative_path(p, &self.cwd)).collect();
                ("Edit".into(), summarize_tool_input("Edit", &json!({"file_path": paths.join(", ")})))
            }
            "mcp_tool_call" => {
                let name = format!("mcp__{}__{}", s("server"), s("tool"));
                let args = item.get("arguments").cloned().unwrap_or(Value::Null);
                let summary = summarize_tool_input(&name, &args);
                (name, summary)
            }
            "web_search" => ("WebSearch".into(), summarize_tool_input("WebSearch", &json!({"query": s("query")}))),
            "todo_list" => {
                let n = item.get("items").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
                ("TodoWrite".into(), format!("{n} steps"))
            }
            _ => return None,
        })
    }

    /// What a finished tool item produced, and whether it failed.
    fn output_of(&self, item: &Value) -> (String, bool) {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "command_execution" => {
                let out = item.get("aggregated_output").and_then(Value::as_str).unwrap_or("").to_string();
                let code = item.get("exit_code").and_then(Value::as_i64);
                (out, code.is_some_and(|c| c != 0))
            }
            "file_change" => {
                let lines: Vec<String> =
                    changes(item).iter().map(|(kind, p)| format!("{kind} {}", relative_path(p, &self.cwd))).collect();
                (lines.join("\n"), false)
            }
            "mcp_tool_call" => match item.get("error") {
                Some(e) if !e.is_null() => (e.get("message").and_then(Value::as_str).unwrap_or("failed").to_string(), true),
                _ => (text_of(item.get("result")), false),
            },
            "todo_list" => {
                let lines: Vec<String> = item
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|t| {
                                let done = t.get("completed").and_then(Value::as_bool).unwrap_or(false);
                                format!("[{}] {}", if done { "x" } else { " " }, t.get("text").and_then(Value::as_str).unwrap_or(""))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (lines.join("\n"), false)
            }
            _ => (String::new(), false),
        }
    }
}

/// Codex's usage with the keys renamed: names containing "token" are redacted as secrets.
fn usage(u: &Value) -> Value {
    let n = |k: &str| u.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "input": n("input_tokens"),
        "output": n("output_tokens"),
        "cache_read": n("cached_input_tokens"),
        "reasoning": n("reasoning_output_tokens"),
    })
}

fn changes(item: &Value) -> Vec<(String, String)> {
    item.get("changes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|c| {
                    let g = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    (g("kind"), g("path"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        // MCP results: {"content":[{"type":"text","text":...}]}
        Some(r) => match r.get("content").and_then(Value::as_array) {
            Some(blocks) => blocks
                .iter()
                .map(|b| b.get("text").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| b.to_string()))
                .collect::<Vec<_>>()
                .join("\n"),
            None => redact_json(r).to_string(),
        },
    }
}

/// Codex reports API errors as a JSON string inside `message`; show the inner message.
fn error_text(msg: &str) -> String {
    serde_json::from_str::<Value>(msg)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| msg.to_string())
}

/// `/bin/zsh -lc "git status"` -> `git status`: Codex wraps every command in the user's shell.
pub fn unwrap_shell(cmd: &str) -> String {
    let Some((shell, rest)) = cmd.split_once(" -lc ").or_else(|| cmd.split_once(" -c ")) else {
        return cmd.to_string();
    };
    let shell = shell.rsplit('/').next().unwrap_or(shell);
    if !matches!(shell, "zsh" | "bash" | "sh" | "fish" | "dash") {
        return cmd.to_string();
    }
    let rest = rest.trim();
    if rest.len() >= 2 && rest.starts_with('\'') && rest.ends_with('\'') {
        return rest[1..rest.len() - 1].replace("'\\''", "'");
    }
    if rest.len() >= 2 && rest.starts_with('"') && rest.ends_with('"') {
        let inner = &rest[1..rest.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some(n @ ('"' | '\\' | '$' | '`')) => out.push(n),
                    Some(n) => {
                        out.push('\\');
                        out.push(n);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        return out;
    }
    rest.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &str) -> Vec<NewEvent> {
        let mut s = CodexStream::new("gpt-6-luna", "/home/u/code/app");
        lines
            .lines()
            .flat_map(|l| match s.map_line(l) {
                MappedLine::Events(e) => e,
                MappedLine::Raw(r) => panic!("raw line {r}"),
            })
            .collect()
    }

    #[test]
    fn maps_a_real_exec_stream() {
        let e = run(include_str!("../tests/fixtures/codex_exec.jsonl"));
        let kinds: Vec<&str> = e.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["agent_init", "assistant_text", "tool_use", "tool_result", "tool_use", "tool_result", "assistant_text", "result"]
        );
        assert_eq!(e[0].payload["session_id"], "01a15d87-b42a-7e40-a8ef-059317c5b5d9");
        assert_eq!(e[0].payload["model"], "gpt-6-luna");
        assert_eq!(e[2].payload["name"], "Bash");
        assert_eq!(e[2].payload["summary"], "ls");
        assert_eq!(e[3].payload["tool_use_id"], e[2].payload["id"]);
        assert_eq!(e[3].payload["content"], "a.txt\nerr.txt\nout.jsonl\n");
        assert_eq!(e[3].payload["is_error"], false);
        assert_eq!(e[4].payload["summary"], r"printf 'world\n' >> a.txt");
        let r = &e[7].payload;
        assert_eq!(r["is_error"], false);
        assert_eq!(r["result"], "done");
        assert_eq!(r["session_id"], "01a15d87-b42a-7e40-a8ef-059317c5b5d9");
        assert_eq!(r["usage"]["output"], 223);
        assert_eq!(r["usage"], crate::redact::redact_json(&r["usage"]), "usage survives the control plane's redaction");
    }

    #[test]
    fn maps_file_changes_with_relative_paths() {
        let e = run(include_str!("../tests/fixtures/codex_resume.jsonl"));
        let kinds: Vec<&str> = e.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, vec!["agent_init", "tool_use", "tool_result", "assistant_text", "result"]);
        assert_eq!(e[1].payload["name"], "Edit");
        assert_eq!(e[1].payload["summary"], "b.txt");
        assert_eq!(e[2].payload["content"], "add b.txt");
    }

    #[test]
    fn completed_without_started_still_announces_the_tool() {
        let e = run(r#"{"type":"item.completed","item":{"id":"i9","type":"command_execution","command":"bash -lc 'exit 2'","aggregated_output":"boom","exit_code":2,"status":"failed"}}"#);
        assert_eq!(e[0].kind, "tool_use");
        assert_eq!(e[0].payload["summary"], "exit 2");
        assert_eq!(e[1].kind, "tool_result");
        assert_eq!(e[1].payload["is_error"], true);
    }

    #[test]
    fn failures_and_warnings() {
        // Real lines from a run with a model the account cannot use.
        let e = run(concat!(
            r#"{"type":"thread.started","thread_id":"t1"}"#, "\n",
            r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Model metadata for `x` not found."}}"#, "\n",
            r#"{"type":"turn.started"}"#, "\n",
            r#"{"type":"error","message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'x' model is not supported when using Codex with a ChatGPT account.\"}}"}"#, "\n",
            r#"{"type":"turn.failed","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'x' model is not supported when using Codex with a ChatGPT account.\"}}"}}"#,
        ));
        let kinds: Vec<&str> = e.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, vec!["agent_init", "stderr", "stderr", "result"]);
        assert_eq!(e[2].payload["text"], "The 'x' model is not supported when using Codex with a ChatGPT account.");
        assert_eq!(e[3].payload["is_error"], true);
        assert_eq!(e[3].payload["result"], "The 'x' model is not supported when using Codex with a ChatGPT account.");
    }

    #[test]
    fn redacts_commands_and_output() {
        let e = run(r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"/bin/zsh -lc \"export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz1234 && gh pr list\"","aggregated_output":"API_KEY=supersecretvalue99","exit_code":0,"status":"completed"}}"#);
        let all = serde_json::to_string(&e).unwrap();
        assert!(!all.contains("ghp_abc"), "{all}");
        assert!(!all.contains("supersecretvalue99"), "{all}");
        assert!(all.contains("gh pr list"));
    }

    #[test]
    fn unwraps_shell_wrappers() {
        assert_eq!(unwrap_shell("/bin/zsh -lc ls"), "ls");
        assert_eq!(unwrap_shell(r#"/bin/bash -lc "echo \"hi\" && cd \$HOME""#), r#"echo "hi" && cd $HOME"#);
        assert_eq!(unwrap_shell("bash -lc 'it'\\''s'"), "it's");
        assert_eq!(unwrap_shell("python -c 'print(1)'"), "python -c 'print(1)'");
        assert_eq!(unwrap_shell("git status"), "git status");
    }
}
