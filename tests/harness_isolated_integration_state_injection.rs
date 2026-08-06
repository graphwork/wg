//! Process-global-state isolation harness.
//! Kept separate so environment mutation cannot cross case-crate boundaries.

#[path = "integration_state_injection.rs"]
mod integration_state_injection;
