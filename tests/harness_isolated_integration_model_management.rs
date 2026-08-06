//! Process-global-state isolation harness.
//! Kept separate so environment mutation cannot cross case-crate boundaries.

#[path = "integration_model_management.rs"]
mod integration_model_management;
