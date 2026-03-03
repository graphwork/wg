//! Native executor: Rust-native LLM client with tool-use loop.
//!
//! Supports multiple LLM providers through the `LlmClient` trait:
//! - Anthropic Messages API (`client.rs`)
//! - OpenAI-compatible APIs (`openai_client.rs`) — OpenRouter, OpenAI, Ollama, etc.
//!
//! Executes tools in-process. Eliminates external dependencies on
//! Claude CLI or Amplifier for agent execution.

pub mod agent;
pub mod bundle;
pub mod client;
pub mod openai_client;
pub mod tools;
