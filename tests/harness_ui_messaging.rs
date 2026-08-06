//! Consolidated integration-test domain harness.
//!
//! Keep top-level cases as modules so Cargo links this domain once rather than
//! producing one debuginfo-heavy executable per source file.

#[path = "integration_chat.rs"]
mod integration_chat;
#[path = "integration_chat_rename.rs"]
mod integration_chat_rename;
#[path = "integration_html.rs"]
mod integration_html;
#[path = "integration_last_interaction_at.rs"]
mod integration_last_interaction_at;
#[path = "integration_logging.rs"]
mod integration_logging;
#[path = "integration_messaging.rs"]
mod integration_messaging;
#[path = "integration_tui_perf_benchmarks.rs"]
mod integration_tui_perf_benchmarks;
#[path = "integration_user_board.rs"]
mod integration_user_board;
#[path = "test_prompt_from_components.rs"]
mod test_prompt_from_components;
#[path = "test_prompt_logging_debug.rs"]
mod test_prompt_logging_debug;
