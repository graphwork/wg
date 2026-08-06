//! Process-global-state isolation harness.
//! Kept separate so environment mutation cannot cross case-crate boundaries.

#[path = "integration_dedicated_pi_evaluation.rs"]
mod integration_dedicated_pi_evaluation;
