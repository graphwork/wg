//! Consolidated integration-test domain harness.
//!
//! Keep top-level cases as modules so Cargo links this domain once rather than
//! producing one debuginfo-heavy executable per source file.

#[path = "integration_context_scope.rs"]
mod integration_context_scope;
#[path = "integration_exec.rs"]
mod integration_exec;
#[path = "integration_fed_wire.rs"]
mod integration_fed_wire;
#[path = "integration_placement.rs"]
mod integration_placement;
#[path = "integration_scope_guard.rs"]
mod integration_scope_guard;
