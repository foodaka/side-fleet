//! Mapping of Claude Code `--output-format stream-json` lines to run events.
//!
//! A line is one JSON object. The shapes we care about:
//!
//! * `{"type":"system","subtype":"init","model":...,"tools":[...],"session_id":...}`
//! * `{"type":"assistant","message":{"content":[{"type":"text","text":...},{"type":"tool_use","name":...,"input":{...}}]}}`
//! * `{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":...,"content":...,"is_error":...}]}}`
//! * `{"type":"result","subtype":"success"|"error_*","is_error":bool,"result":...,"duration_ms":...,"total_cost_usd":...}`
//!
//! * `{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":...}}}`
//!   (with `--include-partial-messages`): the reply as it is generated, see [`ClaudeStream`].
//!
//! Anything else is forwarded as `agent_other` with only its `type`/`subtype`.

use crate::redact::{redact, redact_json, summarize_tool_input};
use crate::{NewEvent, UsageLimit};
use chrono::DateTime;
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

/// Kind of the event carrying the reply text while it is generated: `{"text": ...}`,
/// everything of the current text block so far (empty clears it). The worker sends
/// these to the page without storing them; the final `assistant_text` replaces it.
pub const LIVE_TEXT: &str = "assistant_live";

/// A text block past this many bytes stops updating live; the whole message still lands.
pub const LIVE_TEXT_MAX_BYTES: usize = 64 * 1024;

/// Map one stdout line of the agent to run events. Stateless: `stream_event`
/// lines map to nothing here; [`ClaudeStream`] turns them into live text.
pub fn map_stream_line(line: &str) -> MappedLine {
    match parse_line(line) {
        Ok(v) => map_value(&v),
        Err(mapped) => mapped,
    }
}

fn parse_line(line: &str) -> Result<Value, MappedLine> {
    let line = line.trim();
    if line.is_empty() {
        return Err(MappedLine::Events(vec![]));
    }
    serde_json::from_str(line).map_err(|_| MappedLine::Raw(truncate_bytes(&redact(line), TOOL_RESULT_MAX_BYTES)))
}

/// Claude Code's stdout with `--include-partial-messages`: [`map_stream_line`],
/// plus an [`LIVE_TEXT`] event each time the top-level reply's text grows.
#[derive(Debug, Default)]
pub struct ClaudeStream {
    /// The text block being generated, unredacted; None while the block is not text.
    text: Option<String>,
    /// What the last live event showed.
    shown: String,
}

impl ClaudeStream {
    pub fn map_line(&mut self, line: &str) -> MappedLine {
        let v = match parse_line(line) {
            Ok(v) => v,
            Err(mapped) => return mapped,
        };
        if v.get("type").and_then(Value::as_str) == Some("stream_event") {
            return MappedLine::Events(self.on_stream_event(&v).into_iter().collect());
        }
        map_value(&v)
    }

    fn on_stream_event(&mut self, v: &Value) -> Option<NewEvent> {
        // A subagent's messages stream too, interleaved with the main one's; they land whole.
        if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
            return None;
        }
        let Some(ev) = v.get("event") else { return None };
        match ev.get("type").and_then(Value::as_str)? {
            "content_block_start" => {
                let is_text = ev.pointer("/content_block/type").and_then(Value::as_str) == Some("text");
                self.text = is_text.then(String::new);
                self.show(String::new())
            }
            "content_block_delta" => {
                let text = self.text.as_mut()?;
                if ev.pointer("/delta/type").and_then(Value::as_str) != Some("text_delta") || text.len() > LIVE_TEXT_MAX_BYTES {
                    return None;
                }
                text.push_str(ev.pointer("/delta/text").and_then(Value::as_str).unwrap_or(""));
                let view = live_view(text);
                self.show(view)
            }
            "message_start" | "content_block_stop" | "message_stop" => {
                self.text = None;
                self.show(String::new())
            }
            _ => None,
        }
    }

    /// The live event for `view`, unless the page already shows it.
    fn show(&mut self, view: String) -> Option<NewEvent> {
        if view == self.shown {
            return None;
        }
        self.shown = view.clone();
        Some(NewEvent { kind: LIVE_TEXT.into(), payload: json!({"text": view}) })
    }
}

/// The part of a text block in progress that may be shown: redacted, and without
/// what could still be the start of a secret, i.e. its unfinished last word and an
/// unfinished private key block. Each pattern [`redact`] knows then sees a secret
/// whole before any of it leaves the machine.
pub fn live_view(text: &str) -> String {
    let mut end = text.len();
    if let Some(i) = text.rfind("-----BEGIN") {
        if !text[i..].contains("-----END") {
            end = i;
        }
    }
    let mut text = &text[..end];
    if !text.ends_with(char::is_whitespace) {
        let word_start = text.char_indices().rev().find(|(_, c)| c.is_whitespace()).map(|(i, c)| i + c.len_utf8());
        text = &text[..word_start.unwrap_or(0)];
    }
    redact(text)
}

fn map_value(v: &Value) -> MappedLine {
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
        "rate_limit_event" | "stream_event" => {}
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
                    let mut payload = json!({
                        "tool_use_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
                        "is_error": block.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                        "content": truncate_bytes(&redact(&content), TOOL_RESULT_MAX_BYTES),
                        "truncated": truncated,
                    });
                    let images = tool_result_images(block.get("content"));
                    if !images.is_empty() {
                        payload["images"] = Value::Array(images);
                    }
                    out.push(NewEvent { kind: "tool_result".into(), payload });
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

/// Whether an agent's error text says the account hit its usage or rate limit:
/// Claude Code's `You've hit your limit · resets 3pm`, `5-hour limit reached`
/// and the older `Claude AI usage limit reached|<unix reset time>`, Codex's
/// `You've hit your usage limit`, a provider's `Rate limit exceeded` (opencode).
/// Only meaningful for a failed result: a successful answer may talk about limits.
pub fn usage_limit_in(text: &str) -> Option<UsageLimit> {
    const SIGNS: &[&str] = &[
        "usage limit",
        "hit your limit",
        "hour limit reached",
        "weekly limit reached",
        "opus limit reached",
        "session limit reached",
        "rate limit exceeded",
        "rate limit reached",
        "rate_limit_error",
        "rate_limit_exceeded",
        "insufficient_quota",
        "quota exceeded",
    ];
    let lower = text.to_lowercase();
    if !SIGNS.iter().any(|s| lower.contains(s)) {
        return None;
    }
    let resets_at = text
        .split_once('|')
        .and_then(|(_, t)| t.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse::<i64>().ok())
        .and_then(|t| DateTime::from_timestamp(t, 0));
    Some(UsageLimit { resets_at })
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

/// Base64 images in a tool result (`{"type":"image","source":{"type":"base64",...}}`),
/// as `{media_type, data}` for the control plane to store and the page to show.
fn tool_result_images(content: Option<&Value>) -> Vec<Value> {
    let Some(Value::Array(blocks)) = content else { return vec![] };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("image"))
        .filter_map(|b| {
            let src = b.get("source")?;
            if src.get("type").and_then(Value::as_str) != Some("base64") {
                return None;
            }
            Some(json!({"media_type": src.get("media_type")?.as_str()?, "data": src.get("data")?.as_str()?}))
        })
        .collect()
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
    fn tool_result_images_are_forwarded_unredacted() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}},{"type":"text","text":"shot"}]}]}}"#;
        let e = events(line);
        assert_eq!(e[0].payload["content"], "[image]\nshot");
        assert_eq!(e[0].payload["images"], serde_json::json!([{"media_type": "image/png", "data": "iVBORw0KGgo="}]));
        let plain = events(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"ok"}]}}"#);
        assert!(plain[0].payload.get("images").is_none());
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
    fn recognises_usage_limit_text() {
        assert!(usage_limit_in("You've hit your limit · resets 3pm (Europe/Berlin)").is_some());
        assert!(usage_limit_in("5-hour limit reached ∙ resets 1am").is_some());
        assert!(usage_limit_in("You've hit your usage limit. Upgrade to Pro or try again in 2 hours.").is_some());
        assert!(usage_limit_in("Rate limit exceeded").is_some());
        let l = usage_limit_in("Claude AI usage limit reached|1792591200").unwrap();
        assert_eq!(l.resets_at.unwrap().to_rfc3339(), "2026-10-21T14:00:00+00:00");
        assert_eq!(usage_limit_in("You've hit your limit").unwrap().resets_at, None);
        assert!(usage_limit_in("Reached max turns (3)").is_none());
        assert!(usage_limit_in("Prompt is too long").is_none());
    }

    fn claude_events(lines: &str) -> Vec<NewEvent> {
        let mut s = ClaudeStream::default();
        lines
            .lines()
            .flat_map(|l| match s.map_line(l) {
                MappedLine::Events(e) => e,
                MappedLine::Raw(r) => panic!("expected events, got raw {r}"),
            })
            .collect()
    }

    fn live(e: &NewEvent) -> &str {
        e.payload["text"].as_str().unwrap()
    }

    #[test]
    fn partial_messages_stream_live_text_and_keep_the_final_messages() {
        let evs = claude_events(include_str!("../tests/fixtures/claude_partial_messages.jsonl"));
        // Stored events are exactly what Claude Code without partial messages gives.
        let stored: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).filter(|k| *k != LIVE_TEXT).collect();
        assert_eq!(stored, ["agent_init", "assistant_thinking", "assistant_text", "tool_use", "tool_result", "assistant_text", "result"]);
        assert!(!evs.iter().any(|e| e.kind == "agent_other"), "stream_event lines are not forwarded as such");

        // Live text grows word by word ahead of each final message, then clears.
        let first_final = evs.iter().position(|e| e.kind == "assistant_text").unwrap();
        let before: Vec<&str> = evs[..first_final].iter().filter(|e| e.kind == LIVE_TEXT).map(live).collect();
        assert!(before.len() > 5, "{before:?}");
        // "R", then "ivers car": the unfinished "car" waits for the next delta.
        assert_eq!(before[0], "Rivers ");
        assert!(before.windows(2).all(|w| w[1].starts_with(w[0]) && w[1].len() > w[0].len()), "{before:?}");
        assert!(before.iter().all(|t| t.is_empty() || t.ends_with(' ')), "{before:?}");
        let final_text = evs[first_final].payload["text"].as_str().unwrap();
        assert!(final_text.starts_with(before.last().unwrap()), "{final_text:?}");
        assert_eq!(live(&evs[first_final + 1]), "", "cleared once the block ends");
        assert!(evs.iter().filter(|e| e.kind == LIVE_TEXT).all(|e| e.payload.as_object().unwrap().len() == 1));
    }

    fn delta(text: &str) -> String {
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}},"parent_tool_use_id":null}).to_string()
    }
    const TEXT_START: &str = r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"parent_tool_use_id":null}"#;

    #[test]
    fn live_text_never_shows_part_of_a_secret() {
        let mut lines = vec![TEXT_START.to_string()];
        for d in ["Set ", "GITHUB_", "TOKEN=ghp_abcd", "efghijklmnopqrstuvwxyz", "1234 and run ", "it. Bearer abc", "def123456 ok "] {
            lines.push(delta(d));
        }
        let evs = claude_events(&lines.join("\n"));
        let shown: Vec<&str> = evs.iter().map(live).collect();
        for t in &shown {
            assert!(!t.contains("ghp_") && !t.contains("abcd") && !t.contains("abcdef"), "{t:?}");
        }
        assert_eq!(*shown.last().unwrap(), "Set GITHUB_TOKEN=[REDACTED] and run it. Bearer [REDACTED] ok ");

        let pem = ["Key:\n-----BEGIN RSA PRIVATE KEY-----\n", "MIIEowIBAAKCAQEA\n", "-----END RSA PRIVATE KEY-----\n", "done "];
        let mut lines = vec![TEXT_START.to_string()];
        lines.extend(pem.iter().map(|d| delta(d)));
        let shown: Vec<String> = claude_events(&lines.join("\n")).iter().map(|e| live(e).to_string()).collect();
        assert!(shown.iter().all(|t| !t.contains("MIIE")), "{shown:?}");
        assert_eq!(shown.last().unwrap(), &format!("Key:\n{REDACTED}\ndone "));
    }

    #[test]
    fn live_text_skips_subagents_tools_and_thinking() {
        let sub = json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"sub agent words "}},"parent_tool_use_id":"toolu_1"}).to_string();
        let thinking = r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}},"parent_tool_use_id":null}"#;
        let think_delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"secret plans "}},"parent_tool_use_id":null}"#;
        let tool = r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t","name":"Bash","input":{}}},"parent_tool_use_id":null}"#;
        let json_delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\": \"ls \""}},"parent_tool_use_id":null}"#;
        let lines = [TEXT_START, &sub, thinking, think_delta, tool, json_delta, &delta("no block open ")].join("\n");
        assert!(claude_events(&lines).is_empty());
        // The stateless mapping ignores stream events altogether.
        assert!(events(&delta("hello there ")).is_empty());
    }

    #[test]
    fn live_view_holds_back_the_last_word() {
        assert_eq!(live_view(""), "");
        assert_eq!(live_view("Hello"), "");
        assert_eq!(live_view("Hello wor"), "Hello ");
        assert_eq!(live_view("Hello world\n"), "Hello world\n");
        assert_eq!(live_view("Grüße\u{3000}wél"), "Grüße\u{3000}");
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
