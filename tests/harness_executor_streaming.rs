//! Consolidated integration-test domain harness.
//!
//! Keep top-level cases as modules so Cargo links this domain once rather than
//! producing one debuginfo-heavy executable per source file.

#[path = "codex_handler_oai_compat.rs"]
mod codex_handler_oai_compat;
#[path = "integration_anthropic_streaming.rs"]
mod integration_anthropic_streaming;
#[path = "integration_context_pressure.rs"]
mod integration_context_pressure;
#[path = "integration_dual_executor.rs"]
mod integration_dual_executor;
#[path = "integration_executor_arena_surfaces.rs"]
mod integration_executor_arena_surfaces;
#[path = "integration_init_codex_runtime.rs"]
mod integration_init_codex_runtime;
#[path = "integration_native_executor.rs"]
mod integration_native_executor;
#[path = "integration_native_executor_async_web.rs"]
mod integration_native_executor_async_web;
#[path = "integration_nex_entrypoint.rs"]
mod integration_nex_entrypoint;
#[path = "integration_nex_streaming_resilience.rs"]
mod integration_nex_streaming_resilience;
#[path = "integration_openrouter.rs"]
mod integration_openrouter;
#[path = "integration_openrouter_flow.rs"]
mod integration_openrouter_flow;
#[path = "integration_openrouter_smoke.rs"]
mod integration_openrouter_smoke;
#[path = "integration_session_summary.rs"]
mod integration_session_summary;
#[path = "integration_simplify_executor_taxonomy.rs"]
mod integration_simplify_executor_taxonomy;
#[path = "integration_streaming.rs"]
mod integration_streaming;
#[path = "integration_tool_parallelism.rs"]
mod integration_tool_parallelism;
#[path = "llm_integration.rs"]
mod llm_integration;
#[path = "mock_executor_integration.rs"]
mod mock_executor_integration;
#[path = "smoke_native_executor.rs"]
mod smoke_native_executor;
#[path = "smoke_openrouter_errors.rs"]
mod smoke_openrouter_errors;
#[path = "smoke_openrouter_tool_loop.rs"]
mod smoke_openrouter_tool_loop;
#[path = "test_context_pressure_agent.rs"]
mod test_context_pressure_agent;
#[path = "test_nex.rs"]
mod test_nex;
#[path = "test_streaming_agent_loop.rs"]
mod test_streaming_agent_loop;
