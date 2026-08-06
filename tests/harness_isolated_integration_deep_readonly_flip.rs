//! Process-global-state isolation harness.
//! Kept separate so environment mutation cannot cross case-crate boundaries.

#[path = "integration_deep_readonly_flip.rs"]
mod integration_deep_readonly_flip;
