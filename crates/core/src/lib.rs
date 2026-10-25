//! Shared types for Sidefleet: run/worker models, the wire types exchanged
//! between the control plane (`sidefleet serve`) and the worker (`sidefleet run`),
//! the coding agents a task can run with and the mapping of each agent's output
//! to run events, and redaction of anything that looks like a secret.

pub mod agent;
pub mod models;
pub mod redact;
pub mod stream;
pub mod stream_codex;
pub mod stream_opencode;

pub use agent::{result_usage_limit, Agent, AgentStream, AGENTS, CLAUDE_EFFORTS, CLAUDE_MODELS, CODEX_EFFORTS};
pub use models::*;
pub use redact::{redact, redact_tool_input, summarize_tool_input};
pub use stream::{map_stream_line, usage_limit_in, MappedLine, TOOL_RESULT_MAX_BYTES};
