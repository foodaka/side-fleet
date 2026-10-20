//! Shared types for the AgentCloud spike: run/worker models, the wire types
//! exchanged between the control plane (`acd`) and the worker (`acw`), the
//! mapping from Claude Code `stream-json` lines to run events, and redaction
//! of anything that looks like a secret.

pub mod models;
pub mod redact;
pub mod stream;

pub use models::*;
pub use redact::{redact, summarize_tool_input};
pub use stream::{map_stream_line, MappedLine, TOOL_RESULT_MAX_BYTES};
