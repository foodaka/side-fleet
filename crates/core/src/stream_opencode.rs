//! Mapping of opencode `opencode run --format json` lines to run events.
//!
//! Every line is `{"type":...,"timestamp":ms,"sessionID":"ses_...", "part"|"error": ...}`
//! with one of six types (opencode 1.19, `packages/opencode/src/cli/cmd/run.ts`):
//!
//! * `step_start`: a model step began
//! * `text`: a finished text part, `part.text`
//! * `reasoning`: a finished reasoning part (only with `--thinking`)
//! * `tool_use`: a finished tool call, `part.tool`, `part.callID`,
//!   `part.state{status:"completed"|"error", input, output|error, title}`
//! * `step_finish`: `part.reason`, `part.cost` (USD for that step), `part.tokens`
//! * `error`: `error{name, data{message}}`
//!
//! There is no final result line; the process exits when the session goes idle
//! (exit 1 after a session error). [`OpencodeStream::finish`] synthesises the
//! `result` event from what was seen.

use std::time::Instant;

use crate::agent::relative_path;
use crate::redact::{redact, summarize_tool_input};
use crate::stream::{truncate_bytes, MappedLine, TOOL_RESULT_MAX_BYTES};
use crate::NewEvent;
use serde_json::{json, Map, Value};

pub struct OpencodeStream {
    model: String,
    cwd: String,
    started: Instant,
    session_id: Option<String>,
    last_text: String,
    error: Option<String>,
    cost: f64,
    steps: u32,
    tokens: Map<String, Value>,
    finished: bool,
}

impl OpencodeStream {
    pub fn new(model: &str, cwd: &str) -> Self {
        OpencodeStream {
            model: model.to_string(),
            cwd: cwd.to_string(),
            started: Instant::now(),
            session_id: None,
            last_text: String::new(),
            error: None,
            cost: 0.0,
            steps: 0,
            tokens: Map::new(),
            finished: false,
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
        let mut out = Vec::new();
        if self.session_id.is_none() {
            if let Some(sid) = v.get("sessionID").and_then(Value::as_str) {
                self.session_id = Some(sid.to_string());
                out.push(NewEvent {
                    kind: "agent_init".into(),
                    payload: json!({
                        "agent": "opencode",
                        "model": if self.model.is_empty() { Value::Null } else { json!(self.model) },
                        "session_id": sid,
                        "tools": Value::Null,
                        "cwd": self.cwd,
                    }),
                });
            }
        }
        let part = v.get("part").cloned().unwrap_or(Value::Null);
        let ps = |k: &str| part.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "step_start" => {}
            "text" => {
                let text = ps("text");
                if !text.trim().is_empty() && part.get("synthetic").and_then(Value::as_bool) != Some(true) {
                    self.last_text = text.clone();
                    out.push(NewEvent { kind: "assistant_text".into(), payload: json!({"text": redact(&text)}) });
                }
            }
            "reasoning" => out.push(NewEvent { kind: "assistant_thinking".into(), payload: json!({}) }),
            "tool_use" => self.tool(&part, &mut out),
            "step_finish" => {
                self.steps += 1;
                self.cost += part.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
                if let Some(Value::Object(t)) = part.get("tokens") {
                    add_tokens(&mut self.tokens, t);
                }
            }
            "error" => {
                let msg = error_message(v.get("error"));
                self.error = Some(msg.clone());
                out.push(NewEvent { kind: "stderr".into(), payload: json!({"text": truncate_bytes(&redact(&msg), 2048), "stream": "agent"}) });
            }
            other => out.push(NewEvent { kind: "agent_other".into(), payload: json!({"type": other}) }),
        }
        MappedLine::Events(out)
    }

    fn tool(&self, part: &Value, out: &mut Vec<NewEvent>) {
        let raw = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
        let id = part.get("callID").or_else(|| part.get("id")).cloned().unwrap_or(Value::Null);
        let state = part.get("state").cloned().unwrap_or(Value::Null);
        let input = state.get("input").cloned().unwrap_or(Value::Null);
        let (name, summary) = self.describe(raw, &input);
        out.push(NewEvent { kind: "tool_use".into(), payload: json!({"id": id, "name": name, "summary": summary}) });
        let is_error = state.get("status").and_then(Value::as_str) == Some("error");
        let content = if is_error {
            state.get("error").and_then(Value::as_str).unwrap_or("failed").to_string()
        } else {
            state.get("output").and_then(Value::as_str).unwrap_or("").to_string()
        };
        out.push(NewEvent {
            kind: "tool_result".into(),
            payload: json!({
                "tool_use_id": id,
                "is_error": is_error,
                "content": truncate_bytes(&redact(&content), TOOL_RESULT_MAX_BYTES),
                "truncated": content.len() > TOOL_RESULT_MAX_BYTES,
            }),
        });
    }

    /// opencode's tool names and camelCase inputs, as Claude Code's tools so the page renders them alike.
    fn describe(&self, tool: &str, input: &Value) -> (String, String) {
        let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let path = |k: &str| relative_path(&s(k), &self.cwd);
        let (name, input) = match tool {
            "bash" => ("Bash", json!({"command": s("command")})),
            "read" => ("Read", json!({"file_path": path("filePath")})),
            "edit" | "apply_patch" | "patch" | "multiedit" => ("Edit", json!({"file_path": path("filePath")})),
            "write" => ("Write", json!({"file_path": path("filePath")})),
            "grep" => ("Grep", json!({"pattern": s("pattern"), "path": input.get("path").and_then(Value::as_str).map(|p| relative_path(p, &self.cwd))})),
            "glob" => ("Glob", json!({"pattern": s("pattern")})),
            "list" => ("Glob", json!({"pattern": relative_path(&s("path"), &self.cwd)})),
            "webfetch" => ("WebFetch", json!({"url": s("url")})),
            "websearch" => ("WebSearch", json!({"query": s("query")})),
            "task" => ("Task", json!({"description": s("description")})),
            "todowrite" | "todoread" => {
                let n = input.get("todos").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
                return ("TodoWrite".into(), format!("{n} steps"));
            }
            other => return (other.to_string(), summarize_tool_input(other, input)),
        };
        // Drop a null grep path rather than printing "in null".
        let input = match input {
            Value::Object(m) => Value::Object(m.into_iter().filter(|(_, v)| !v.is_null()).collect()),
            v => v,
        };
        let summary = summarize_tool_input(name, &input);
        // apply_patch has no single file; fall back to the first patched path in its text.
        if summary.is_empty() && tool == "apply_patch" {
            let first = s("patchText").lines().find_map(|l| l.split_once(" File: ").map(|(_, p)| p.trim().to_string()));
            return (name.into(), first.map(|p| relative_path(&p, &self.cwd)).unwrap_or_default());
        }
        (name.into(), summary)
    }

    /// The `result` event, once stdout has closed. `exit_ok` is the process's exit status.
    pub fn finish(&mut self, exit_ok: bool) -> Vec<NewEvent> {
        if self.finished || self.session_id.is_none() && exit_ok && self.steps == 0 {
            return vec![];
        }
        self.finished = true;
        let is_error = self.error.is_some() || !exit_ok;
        let text = match (&self.error, is_error) {
            (Some(e), _) => e.clone(),
            (None, true) => "opencode exited with an error".to_string(),
            (None, false) => self.last_text.clone(),
        };
        vec![NewEvent {
            kind: "result".into(),
            payload: json!({
                "subtype": if is_error { "error" } else { "success" },
                "is_error": is_error,
                "result": redact(&text),
                "duration_ms": self.started.elapsed().as_millis() as u64,
                "num_turns": self.steps,
                "total_cost_usd": if self.steps > 0 { json!((self.cost * 1e6).round() / 1e6) } else { Value::Null },
                "usage": Value::Object(self.tokens.clone()),
                "session_id": self.session_id,
            }),
        }]
    }
}

fn add_tokens(acc: &mut Map<String, Value>, t: &Map<String, Value>) {
    for (k, v) in t {
        match v {
            Value::Number(n) => {
                let prev = acc.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                acc.insert(k.clone(), json!((prev + n.as_f64().unwrap_or(0.0)) as u64));
            }
            Value::Object(inner) => {
                let mut sub = match acc.remove(k) {
                    Some(Value::Object(m)) => m,
                    _ => Map::new(),
                };
                add_tokens(&mut sub, inner);
                acc.insert(k.clone(), Value::Object(sub));
            }
            _ => {}
        }
    }
}

/// `{"name":"APIError","data":{"message":"Rate limit exceeded"}}` -> `Rate limit exceeded`.
fn error_message(e: Option<&Value>) -> String {
    let Some(e) = e else { return "opencode reported an error".into() };
    let msg = e.pointer("/data/message").and_then(Value::as_str).or_else(|| e.get("message").and_then(Value::as_str));
    match (msg, e.get("name").and_then(Value::as_str)) {
        (Some(m), Some("ProviderAuthError")) => format!("Not signed in to this provider: {m}"),
        (Some(m), _) => m.to_string(),
        (None, Some(n)) => n.to_string(),
        (None, None) => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "ses_5f3a1b2c4d5eAbCdEfGhIjKlMn";

    fn feed(s: &mut OpencodeStream, lines: &str) -> Vec<NewEvent> {
        lines
            .lines()
            .flat_map(|l| match s.map_line(l) {
                MappedLine::Events(e) => e,
                MappedLine::Raw(r) => panic!("raw line {r}"),
            })
            .collect()
    }

    #[test]
    fn maps_a_run_and_synthesises_the_result() {
        let mut s = OpencodeStream::new("openrouter/moonshotai/kimi-k2", "/home/u/app");
        let mut e = feed(&mut s, include_str!("../tests/fixtures/opencode_run.jsonl"));
        e.extend(s.finish(true));
        let kinds: Vec<&str> = e.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["agent_init", "assistant_text", "tool_use", "tool_result", "tool_use", "tool_result", "tool_use", "tool_result", "assistant_text", "result"]
        );
        assert_eq!(e[0].payload["session_id"], SID);
        assert_eq!(e[0].payload["model"], "openrouter/moonshotai/kimi-k2");
        assert_eq!(e[2].payload["name"], "Bash");
        assert_eq!(e[2].payload["summary"], "cargo test");
        assert_eq!(e[3].payload["tool_use_id"], "toolu_01");
        assert_eq!(e[3].payload["content"], "test result: ok. 12 passed");
        assert_eq!(e[4].payload["name"], "Read");
        assert_eq!(e[4].payload["summary"], "src/lib.rs");
        assert_eq!(e[6].payload["name"], "Edit");
        assert_eq!(e[7].payload["is_error"], true);
        assert_eq!(e[7].payload["content"], "oldString not found in content");
        let r = &e[9].payload;
        assert_eq!(r["is_error"], false);
        assert_eq!(r["result"], "Fixed the flaky test.");
        assert_eq!(r["session_id"], SID);
        assert_eq!(r["num_turns"], 2);
        assert_eq!(r["total_cost_usd"], 0.0153);
        assert_eq!(r["usage"]["input"], 27000);
        assert_eq!(r["usage"]["cache"]["read"], 20000);
    }

    #[test]
    fn session_errors_fail_the_run() {
        let mut s = OpencodeStream::new("", "/home/u/app");
        let e = feed(
            &mut s,
            &format!(r#"{{"type":"error","timestamp":1,"sessionID":"{SID}","error":{{"name":"ProviderAuthError","data":{{"providerID":"openrouter","message":"OPENROUTER_API_KEY is not set"}}}}}}"#),
        );
        assert_eq!(e[0].kind, "agent_init");
        assert_eq!(e[0].payload["model"], Value::Null);
        assert_eq!(e[1].kind, "stderr");
        let r = s.finish(false);
        assert_eq!(r[0].payload["is_error"], true);
        assert_eq!(r[0].payload["result"], "Not signed in to this provider: OPENROUTER_API_KEY is not set");
        assert!(s.finish(false).is_empty(), "only one result");
    }

    #[test]
    fn a_crash_before_any_output_still_reports_a_result() {
        let mut s = OpencodeStream::new("x/y", "/home/u/app");
        let r = s.finish(false);
        assert_eq!(r[0].payload["is_error"], true);
        assert_eq!(r[0].payload["session_id"], Value::Null);
    }

    #[test]
    fn redacts_tool_input_and_output() {
        let mut s = OpencodeStream::new("", "/home/u/app");
        let e = feed(
            &mut s,
            &format!(r#"{{"type":"tool_use","timestamp":1,"sessionID":"{SID}","part":{{"type":"tool","callID":"c1","tool":"bash","state":{{"status":"completed","input":{{"command":"export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz1234 && gh pr list"}},"output":"API_KEY=supersecretvalue99"}}}}}}"#),
        );
        let all = serde_json::to_string(&e).unwrap();
        assert!(!all.contains("ghp_abc"), "{all}");
        assert!(!all.contains("supersecretvalue99"), "{all}");
        assert!(all.contains("gh pr list"));
    }
}
