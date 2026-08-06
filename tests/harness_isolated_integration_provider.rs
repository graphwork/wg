//! Process-global-state isolation harness.
//! Kept separate so environment mutation cannot cross case-crate boundaries.

#[path = "integration_provider.rs"]
mod integration_provider;
