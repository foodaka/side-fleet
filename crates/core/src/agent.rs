//! The coding agents a task can run with. Each agent is a CLI on the worker
//! machine with its own sign-in, flags and output format; the worker spawns it
//! and maps its stdout to the same run events (see [`AgentStream`]).

use crate::stream::{map_stream_line, usage_limit_in, MappedLine};
use crate::{stream_codex, stream_opencode, NewEvent, UsageLimit};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// Anthropic's Claude Code (`claude -p`), using the machine's Claude sign-in.
    Claude,
    /// OpenAI's Codex CLI (`codex exec`), using the machine's ChatGPT sign-in or API key.
    Codex,
    /// opencode (`opencode run`): open source, any provider it is configured for
    /// (OpenRouter, Ollama, Anthropic, OpenAI, ...).
    Opencode,
}

pub const AGENTS: &[Agent] = &[Agent::Claude, Agent::Codex, Agent::Opencode];

/// Models Claude Code tasks may use.
pub const CLAUDE_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"];

impl Agent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Opencode => "opencode",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        AGENTS.iter().copied().find(|a| a.as_str() == s)
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Opencode => "opencode",
        }
    }

    /// Executable looked up on PATH when none is configured.
    pub fn default_bin(&self) -> &'static str {
        self.as_str()
    }

    /// Check a model id for this agent. Claude Code takes one of [`CLAUDE_MODELS`];
    /// Codex and opencode take whatever the machine offers (`gpt-6-luna`,
    /// `openrouter/moonshotai/kimi-k2`), or empty for the machine's default.
    pub fn check_model(&self, model: &str) -> Result<(), String> {
        match self {
            Agent::Claude if CLAUDE_MODELS.contains(&model) => Ok(()),
            Agent::Claude => Err(format!("model must be one of {}", CLAUDE_MODELS.join(", "))),
            Agent::Codex | Agent::Opencode if model.is_empty() => Ok(()),
            Agent::Codex | Agent::Opencode => {
                let ok = model.len() <= 200
                    && !model.starts_with('-')
                    && model.chars().all(|c| c.is_ascii_alphanumeric() || "._:/@+-".contains(c));
                if ok {
                    Ok(())
                } else {
                    Err(format!("{model:?} is not a valid {} model id", self.display_name()))
                }
            }
        }
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Maps one agent's stdout, line by line, to run events. Codex and opencode
/// need a little state (the session id, the last message, which tool calls
/// were announced); Claude Code's lines map one to one.
///
/// A failed `result` whose text says the account hit its usage limit gets a
/// `usage_limit` field (`{"resets_at": ...}`), which the worker reports when it
/// finishes the run so the control plane can move work to another machine.
pub struct AgentStream {
    inner: Inner,
    /// Claude Code said the account's rate limit rejected the request, resetting at this time.
    rejected: Option<Option<DateTime<Utc>>>,
}

enum Inner {
    Claude,
    Codex(stream_codex::CodexStream),
    Opencode(stream_opencode::OpencodeStream),
}

impl AgentStream {
    /// `model` is what the task asked for (reported in `agent_init` when the agent
    /// does not say); `cwd` makes file paths in tool calls relative.
    pub fn new(agent: Agent, model: &str, cwd: &str) -> Self {
        let inner = match agent {
            Agent::Claude => Inner::Claude,
            Agent::Codex => Inner::Codex(stream_codex::CodexStream::new(model, cwd)),
            Agent::Opencode => Inner::Opencode(stream_opencode::OpencodeStream::new(model, cwd)),
        };
        AgentStream { inner, rejected: None }
    }

    pub fn map_line(&mut self, line: &str) -> MappedLine {
        let mut mapped = match &mut self.inner {
            Inner::Claude => {
                if line.contains("\"rate_limit_event\"") {
                    self.note_rate_limit(line);
                }
                map_stream_line(line)
            }
            Inner::Codex(s) => s.map_line(line),
            Inner::Opencode(s) => s.map_line(line),
        };
        if let MappedLine::Events(evs) = &mut mapped {
            evs.iter_mut().for_each(|e| self.mark_usage_limit(e));
        }
        mapped
    }

    /// Called when stdout closes. Agents that do not print a final result line
    /// (opencode) get one synthesised from what they said.
    pub fn finish(&mut self, exit_ok: bool) -> Vec<NewEvent> {
        let mut evs = match &mut self.inner {
            Inner::Claude | Inner::Codex(_) => vec![],
            Inner::Opencode(s) => s.finish(exit_ok),
        };
        evs.iter_mut().for_each(|e| self.mark_usage_limit(e));
        evs
    }

    /// `{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":<unix secs>,...}}`
    fn note_rate_limit(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let Some(info) = v.get("rate_limit_info") else { return };
        if info.get("status").and_then(Value::as_str) == Some("rejected") {
            let resets_at = info.get("resetsAt").and_then(Value::as_i64).and_then(|t| {
                let secs = if t > 100_000_000_000 { t / 1000 } else { t };
                DateTime::from_timestamp(secs, 0)
            });
            self.rejected = Some(resets_at);
        } else {
            self.rejected = None;
        }
    }

    fn mark_usage_limit(&self, ev: &mut NewEvent) {
        if ev.kind != "result" || ev.payload.get("is_error").and_then(Value::as_bool) != Some(true) {
            return;
        }
        let text = ev.payload.get("result").and_then(Value::as_str).unwrap_or("");
        let errors = ev.payload.get("errors").filter(|e| !e.is_null()).map(Value::to_string).unwrap_or_default();
        let limit = match (usage_limit_in(&format!("{text} {errors}")), self.rejected) {
            (Some(l), rejected) => UsageLimit { resets_at: l.resets_at.or(rejected.flatten()) },
            (None, Some(resets_at)) => UsageLimit { resets_at },
            (None, None) => return,
        };
        ev.payload["usage_limit"] = serde_json::to_value(limit).unwrap_or(Value::Null);
    }
}

/// The usage limit a `result` event reports (see [`AgentStream`]), if any.
pub fn result_usage_limit(result: &Value) -> Option<UsageLimit> {
    result.get("usage_limit").and_then(|l| serde_json::from_value(l.clone()).ok())
}

/// A path as the user thinks of it: relative to the task's working directory.
pub(crate) fn relative_path(path: &str, cwd: &str) -> String {
    let cwd = cwd.trim_end_matches('/');
    if !cwd.is_empty() {
        if let Some(rest) = path.strip_prefix(cwd).and_then(|r| r.strip_prefix('/')) {
            return rest.to_string();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_names_agents() {
        assert_eq!(Agent::parse("codex"), Some(Agent::Codex));
        assert_eq!(Agent::parse("Claude"), None);
        assert_eq!(serde_json::to_string(&Agent::Opencode).unwrap(), "\"opencode\"");
        assert_eq!(Agent::Claude.display_name(), "Claude Code");
    }

    #[test]
    fn checks_models_per_agent() {
        assert!(Agent::Claude.check_model("claude-sonnet-5-5").is_ok());
        assert!(Agent::Claude.check_model("gpt-6-luna").is_err());
        assert!(Agent::Claude.check_model("").is_err());
        assert!(Agent::Codex.check_model("").is_ok(), "empty means the machine's default");
        assert!(Agent::Codex.check_model("gpt-6-luna").is_ok());
        assert!(Agent::Opencode.check_model("openrouter/moonshotai/kimi-k2").is_ok());
        assert!(Agent::Opencode.check_model("ollama/qwen3-coder:30b").is_ok());
        assert!(Agent::Opencode.check_model("--dangerously-skip").is_err(), "must not look like a flag");
        assert!(Agent::Codex.check_model("gpt 6; rm -rf").is_err());
        assert!(Agent::Codex.check_model(&"x".repeat(201)).is_err());
    }

    fn results(agent: Agent, lines: &str, exit_ok: bool) -> Vec<Value> {
        let mut s = AgentStream::new(agent, "", "/tmp/app");
        let mut out = Vec::new();
        for line in lines.lines() {
            if let MappedLine::Events(evs) = s.map_line(line) {
                out.extend(evs);
            }
        }
        out.extend(s.finish(exit_ok));
        out.into_iter().filter(|e| e.kind == "result").map(|e| e.payload).collect()
    }

    #[test]
    fn claude_usage_limit_result_reports_the_reset_time() {
        let r = results(Agent::Claude, include_str!("../tests/fixtures/claude_usage_limit.jsonl"), true);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["is_error"], true);
        let limit = result_usage_limit(&r[0]).expect("usage limit detected");
        assert_eq!(limit.resets_at.unwrap().to_rfc3339(), "2026-10-21T14:00:00+00:00", "from the rate_limit_event");
    }

    #[test]
    fn usage_limits_of_other_agents_and_non_limits() {
        let codex = r#"{"type":"thread.started","thread_id":"t1"}
{"type":"turn.failed","error":{"message":"You've hit your usage limit. Upgrade to Pro or try again in 3 hours."}}"#;
        let r = results(Agent::Codex, codex, false);
        assert_eq!(result_usage_limit(&r[0]), Some(UsageLimit { resets_at: None }));
        let opencode = r#"{"type":"error","timestamp":1,"sessionID":"ses_1","error":{"name":"APIError","data":{"message":"Rate limit exceeded"}}}"#;
        let r = results(Agent::Opencode, opencode, false);
        assert!(result_usage_limit(&r[0]).is_some());
        // A successful answer that talks about rate limits is not a limit.
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"result":"Added a check for rate limit exceeded errors."}"#;
        assert_eq!(result_usage_limit(&results(Agent::Claude, ok, true)[0]), None);
        // Other failures are not limits either.
        let turns = r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#;
        assert_eq!(result_usage_limit(&results(Agent::Claude, turns, true)[0]), None);
    }

    #[test]
    fn makes_paths_relative_to_the_workspace() {
        assert_eq!(relative_path("/home/u/app/src/a.rs", "/home/u/app"), "src/a.rs");
        assert_eq!(relative_path("/home/u/app/src/a.rs", "/home/u/app/"), "src/a.rs");
        assert_eq!(relative_path("/etc/hosts", "/home/u/app"), "/etc/hosts");
        assert_eq!(relative_path("/home/u/application/x", "/home/u/app"), "/home/u/application/x");
    }
}
