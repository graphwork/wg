//! Consolidated integration-test domain harness.
//!
//! Keep top-level cases as modules so Cargo links this domain once rather than
//! producing one debuginfo-heavy executable per source file.

#[path = "completion_manifest_resolver.rs"]
mod completion_manifest_resolver;
#[path = "completion_review_valve.rs"]
mod completion_review_valve;
#[path = "completion_task_projection.rs"]
mod completion_task_projection;
#[path = "integration_accidental_cycle.rs"]
mod integration_accidental_cycle;
#[path = "integration_candidate_finalization.rs"]
mod integration_candidate_finalization;
#[path = "integration_cycle_detection.rs"]
mod integration_cycle_detection;
#[path = "integration_deprecate_pending_validation.rs"]
mod integration_deprecate_pending_validation;
#[path = "integration_done_uncommitted.rs"]
mod integration_done_uncommitted;
#[path = "integration_failed_pending_eval.rs"]
mod integration_failed_pending_eval;
#[path = "integration_failure_classification.rs"]
mod integration_failure_classification;
#[path = "integration_failure_injection.rs"]
mod integration_failure_injection;
#[path = "integration_journal.rs"]
mod integration_journal;
#[path = "integration_pending_eval_state.rs"]
mod integration_pending_eval_state;
#[path = "integration_replay_exhaustive.rs"]
mod integration_replay_exhaustive;
#[path = "integration_retire_compact_archive.rs"]
mod integration_retire_compact_archive;
#[path = "integration_runs_exhaustive.rs"]
mod integration_runs_exhaustive;
#[path = "integration_subtask.rs"]
mod integration_subtask;
#[path = "integration_task_lifecycle.rs"]
mod integration_task_lifecycle;
#[path = "integration_trace_exhaustive.rs"]
mod integration_trace_exhaustive;
#[path = "integration_trace_function_layers.rs"]
mod integration_trace_function_layers;
#[path = "integration_trace_functions.rs"]
mod integration_trace_functions;
#[path = "integration_trace_replay.rs"]
mod integration_trace_replay;
#[path = "integration_verify_first.rs"]
mod integration_verify_first;
#[path = "integration_yaml_graceful_degradation.rs"]
mod integration_yaml_graceful_degradation;
#[path = "legacy_completion_authority_retired.rs"]
mod legacy_completion_authority_retired;
#[path = "lifecycle_protocol_conformance.rs"]
mod lifecycle_protocol_conformance;
#[path = "save_transaction_conformance.rs"]
mod save_transaction_conformance;
#[path = "simple_land_conformance.rs"]
mod simple_land_conformance;
#[path = "simple_land_lean_oracle.rs"]
mod simple_land_lean_oracle;
#[path = "test_concurrent_head_reference.rs"]
mod test_concurrent_head_reference;
#[path = "test_crash_scenarios.rs"]
mod test_crash_scenarios;
#[path = "test_race_conditions.rs"]
mod test_race_conditions;
#[path = "test_verify_lint_integration.rs"]
mod test_verify_lint_integration;
#[path = "test_verify_timeout_basic.rs"]
mod test_verify_timeout_basic;
#[path = "test_verify_timeout_functionality.rs"]
mod test_verify_timeout_functionality;
