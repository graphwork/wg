//! Native executor: Rust-native LLM client with tool-use loop.
//!
//! Calls the Anthropic Messages API directly with tool use support,
//! executing tools in-process. Eliminates external dependencies on
//! Claude CLI or Amplifier for agent execution.

pub mod agent;
pub mod bundle;
pub mod client;
pub mod tools;
