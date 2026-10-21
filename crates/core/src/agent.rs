//! The coding agents a task can run with. Each agent is a CLI on the worker
//! machine with its own sign-in, flags and output format; the worker spawns it
//! and maps its stdout to the same run events (see [`AgentStream`]).

use crate::stream::{map_stream_line, MappedLine};
use crate::{stream_codex, stream_opencode};
use serde::{Deserialize, Serialize};
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
pub struct AgentStream {
    inner: Inner,
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
        AgentStream { inner }
    }

    pub fn map_line(&mut self, line: &str) -> MappedLine {
        match &mut self.inner {
            Inner::Claude => map_stream_line(line),
            Inner::Codex(s) => s.map_line(line),
            Inner::Opencode(s) => s.map_line(line),
        }
    }

    /// Called when stdout closes. Agents that do not print a final result line
    /// (opencode) get one synthesised from what they said.
    pub fn finish(&mut self, exit_ok: bool) -> Vec<crate::NewEvent> {
        match &mut self.inner {
            Inner::Claude | Inner::Codex(_) => vec![],
            Inner::Opencode(s) => s.finish(exit_ok),
        }
    }
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

    #[test]
    fn makes_paths_relative_to_the_workspace() {
        assert_eq!(relative_path("/home/u/app/src/a.rs", "/home/u/app"), "src/a.rs");
        assert_eq!(relative_path("/home/u/app/src/a.rs", "/home/u/app/"), "src/a.rs");
        assert_eq!(relative_path("/etc/hosts", "/home/u/app"), "/etc/hosts");
        assert_eq!(relative_path("/home/u/application/x", "/home/u/app"), "/home/u/application/x");
    }
}
