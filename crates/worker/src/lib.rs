//! Sidefleet worker: detects the coding agents installed on this machine,
//! registers with the control plane, claims runs for repos it has checked out,
//! and executes them with the local Claude Code CLI, streaming narration back.

pub mod agent;
pub mod checkpoint;
pub mod client;
pub mod detect;
pub mod updater;
pub mod worker;
