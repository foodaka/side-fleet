//! Mapping of Claude Code `--output-format stream-json` lines to run events.
//!
//! A line is one JSON object. The shapes we care about:
//!
//! * `{"type":"system","subtype":"init","model":...,"tools":[...],"session_id":...}`
//! * `{"type":"assistant","message":{"content":[{"type":"text","text":...},{"type":"tool_use","name":...,"input":{...}}]}}`
//! * `{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":...,"content":...,"is_error":...}]}}`
//! * `{"type":"result","subtype":"success"|"error_*","is_error":bool,"result":...,"duration_ms":...,"total_cost_usd":...}`
//!
//! Anything else is forwarded as `agent_other` with only its `type`/`subtype`.

use crate::redact::{redact, redact_json, summarize_tool_input};
use crate::NewEvent;
use serde_json::{json, Value};

/// Tool results are truncated to this many bytes before being forwarded.
pub const TOOL_RESULT_MAX_BYTES: usize = 2048;

#[derive(Debug, Clone, PartialEq)]
pub enum MappedLine {
    /// Events to forward to the control plane.
    Events(Vec<NewEvent>),
    /// The line was not JSON; forwarded as raw (redacted, truncated) output.
    Raw(String),
}

/// Map one stdout line of the agent to run events.
pub fn map_stream_line(line: &str) -> MappedLine {
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
        "system" => {
            let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
            if subtype == "init" {
                out.push(NewEvent {
                    kind: "agent_init".into(),
                    payload: json!({
                        "model": v.get("model").cloned().unwrap_or(Value::Null),
                        "session_id": v.get("session_id").cloned().unwrap_or(Value::Null),
                        "tools": v.get("tools").cloned().unwrap_or(Value::Null),
                        "cwd": v.get("cwd").cloned().unwrap_or(Value::Null),
                    }),
                });
            }
            // hook_started / hook_response / compact_boundary etc. are noise for the viewer.
        }
        "rate_limit_event" => {}
        "assistant" => {
            for block in content_blocks(&v) {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                        if !text.trim().is_empty() {
                            out.push(NewEvent {
                                kind: "assistant_text".into(),
                                payload: json!({"text": redact(text)}),
                            });
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let input = block.get("input").cloned().unwrap_or(Value::Null);
                        out.push(NewEvent {
                            kind: "tool_use".into(),
                            payload: json!({
                                "id": block.get("id").cloned().unwrap_or(Value::Null),
                                "name": name,
                                "summary": summarize_tool_input(name, &input),
                            }),
                        });
                    }
                    Some("thinking") => {
                        // Thinking blocks are not forwarded verbatim; just note one happened.
                        out.push(NewEvent {
                            kind: "assistant_thinking".into(),
                            payload: json!({}),
                        });
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            for block in content_blocks(&v) {
                if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let content = tool_result_text(block.get("content"));
                    let truncated = content.len() > TOOL_RESULT_MAX_BYTES;
                    out.push(NewEvent {
                        kind: "tool_result".into(),
                        payload: json!({
                            "tool_use_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
                            "is_error": block.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                            "content": truncate_bytes(&redact(&content), TOOL_RESULT_MAX_BYTES),
                            "truncated": truncated,
                        }),
                    });
                }
            }
        }
        "result" => {
            let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
            let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                || subtype.starts_with("error");
            let text = match v.get("result") {
                Some(Value::String(s)) => s.clone(),
                Some(other) if !other.is_null() => other.to_string(),
                _ => String::new(),
            };
            out.push(NewEvent {
                kind: "result".into(),
                payload: json!({
                    "subtype": subtype,
                    "is_error": is_error,
                    "result": redact(&text),
                    "duration_ms": v.get("duration_ms").cloned().unwrap_or(Value::Null),
                    "num_turns": v.get("num_turns").cloned().unwrap_or(Value::Null),
                    "total_cost_usd": v.get("total_cost_usd").cloned().unwrap_or(Value::Null),
                    "session_id": v.get("session_id").cloned().unwrap_or(Value::Null),
                    "errors": redact_json(&v.get("errors").cloned().unwrap_or(Value::Null)),
                }),
            });
        }
        _ => {
            out.push(NewEvent {
                kind: "agent_other".into(),
                payload: json!({
                    "type": ty,
                    "subtype": v.get("subtype").cloned().unwrap_or(Value::Null),
                }),
            });
        }
    }
    MappedLine::Events(out)
}

fn content_blocks(v: &Value) -> Vec<Value> {
    match v.pointer("/message/content") {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) => vec![json!({"type": "text", "text": s})],
        _ => vec![],
    }
}

fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") => b.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                Some(other) => format!("[{other}]"),
                None => b.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Truncate to at most `max` bytes on a char boundary, appending a marker.
pub fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated {} bytes]", &s[..end], s.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redact::REDACTED;

    fn events(line: &str) -> Vec<NewEvent> {
        match map_stream_line(line) {
            MappedLine::Events(e) => e,
            MappedLine::Raw(r) => panic!("expected events, got raw {r}"),
        }
    }

    #[test]
    fn maps_init() {
        let e = events(r#"{"type":"system","subtype":"init","model":"claude-sonnet-5-5","session_id":"s1","tools":["Bash"]}"#);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].kind, "agent_init");
        assert_eq!(e[0].payload["model"], "claude-sonnet-5-5");
    }

    #[test]
    fn maps_assistant_text_and_tool_use() {
        let e = events(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Looking at the repo."},{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls -la"}}]}}"#);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].kind, "assistant_text");
        assert_eq!(e[0].payload["text"], "Looking at the repo.");
        assert_eq!(e[1].kind, "tool_use");
        assert_eq!(e[1].payload["name"], "Bash");
        assert_eq!(e[1].payload["summary"], "ls -la");
        assert!(e[1].payload.get("input").is_none(), "raw input must not be forwarded");
    }

    #[test]
    fn tool_use_input_is_redacted() {
        let e = events(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz1234 && gh auth status"}}]}}"#);
        let s = e[0].payload["summary"].as_str().unwrap();
        assert!(!s.contains("ghp_abc"), "{s}");
        assert!(s.contains(REDACTED));
        let e = events(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu2","name":"mcp__x__call","input":{"api_key":"plainlookingvalue","q":"hi"}}]}}"#);
        let s = e[0].payload["summary"].as_str().unwrap();
        assert!(!s.contains("plainlookingvalue"), "{s}");
        assert!(s.contains("\"q\":\"hi\""), "{s}");
    }

    #[test]
    fn tool_result_is_truncated_and_redacted() {
        let big = "x".repeat(5000) + " token=abcdefgh12345";
        let line = serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":big,"is_error":false}]}}).to_string();
        let e = events(&line);
        assert_eq!(e[0].kind, "tool_result");
        let c = e[0].payload["content"].as_str().unwrap();
        assert!(c.len() <= TOOL_RESULT_MAX_BYTES + 40, "len {}", c.len());
        assert!(c.contains("[truncated"));
        assert_eq!(e[0].payload["truncated"], true);
        let small = serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":[{"type":"text","text":"password=hunter2hunter2"}]}]}}).to_string();
        let e = events(&small);
        let c = e[0].payload["content"].as_str().unwrap();
        assert!(!c.contains("hunter2"), "{c}");
    }

    #[test]
    fn maps_result() {
        let e = events(r#"{"type":"result","subtype":"success","is_error":false,"result":"Done.","duration_ms":1234,"num_turns":3,"total_cost_usd":0.01,"session_id":"s1"}"#);
        assert_eq!(e[0].kind, "result");
        assert_eq!(e[0].payload["is_error"], false);
        assert_eq!(e[0].payload["result"], "Done.");
        let e = events(r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#);
        assert_eq!(e[0].payload["is_error"], true);
    }

    #[test]
    fn drops_hook_and_rate_limit_noise() {
        assert!(events(r#"{"type":"system","subtype":"hook_started","hook_name":"SessionStart"}"#).is_empty());
        assert!(events(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#).is_empty());
        let e = events(r#"{"type":"something_new","subtype":"x"}"#);
        assert_eq!(e[0].kind, "agent_other");
    }

    #[test]
    fn non_json_is_raw_and_blank_is_nothing() {
        assert_eq!(map_stream_line("   "), MappedLine::Events(vec![]));
        match map_stream_line("warning: something sk-abcdefghijklmnopqrstu") {
            MappedLine::Raw(r) => assert!(!r.contains("sk-abc")),
            _ => panic!(),
        }
    }
}
