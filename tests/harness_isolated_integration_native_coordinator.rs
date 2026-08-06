//! Process-global-state isolation harness.
//! Kept separate so environment mutation cannot cross case-crate boundaries.

#[path = "integration_native_coordinator.rs"]
mod integration_native_coordinator;
