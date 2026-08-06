//! Process-global-state isolation harness.
//! Kept separate so environment mutation cannot cross case-crate boundaries.

#[path = "smoke_openrouter_routing.rs"]
mod smoke_openrouter_routing;
