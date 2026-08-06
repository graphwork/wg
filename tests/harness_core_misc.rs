//! Consolidated integration-test domain harness.
//!
//! Keep top-level cases as modules so Cargo links this domain once rather than
//! producing one debuginfo-heavy executable per source file.

#[path = "contract_tests.rs"]
mod contract_tests;
#[path = "daemon_planner_conformance.rs"]
mod daemon_planner_conformance;
#[path = "integration_analytics.rs"]
mod integration_analytics;
#[path = "integration_atomic_writes.rs"]
mod integration_atomic_writes;
#[path = "integration_broken_pipe.rs"]
mod integration_broken_pipe;
#[path = "integration_check_context.rs"]
mod integration_check_context;
#[path = "integration_cli_commands.rs"]
mod integration_cli_commands;
#[path = "integration_cli_endpoints.rs"]
mod integration_cli_endpoints;
#[path = "integration_cli_workflows.rs"]
mod integration_cli_workflows;
#[path = "integration_dev_check.rs"]
mod integration_dev_check;
#[path = "integration_e2e_smoke.rs"]
mod integration_e2e_smoke;
#[path = "integration_error_paths.rs"]
mod integration_error_paths;
#[path = "integration_handler_stdout_pristine.rs"]
mod integration_handler_stdout_pristine;
#[path = "integration_init.rs"]
mod integration_init;
#[path = "integration_phantom_edge.rs"]
mod integration_phantom_edge;
#[path = "integration_pi_watchdog.rs"]
mod integration_pi_watchdog;
#[path = "integration_remove_validation_cli.rs"]
mod integration_remove_validation_cli;
#[path = "integration_resume.rs"]
mod integration_resume;
#[path = "integration_self_healing.rs"]
mod integration_self_healing;
#[path = "integration_smoke_gate.rs"]
mod integration_smoke_gate;
#[path = "integration_special_agents.rs"]
mod integration_special_agents;
#[path = "integration_strong_merge_resolution.rs"]
mod integration_strong_merge_resolution;
#[path = "integration_untested_commands.rs"]
mod integration_untested_commands;
#[path = "smoke_context.rs"]
mod smoke_context;
#[path = "smoke_no_org_eval.rs"]
mod smoke_no_org_eval;
#[path = "terminology_lint.rs"]
mod terminology_lint;
#[path = "test_edge_cases.rs"]
mod test_edge_cases;
#[path = "test_edit_file_edge_cases.rs"]
mod test_edit_file_edge_cases;
#[path = "test_edit_file_fix_build_repro.rs"]
mod test_edit_file_fix_build_repro;
#[path = "test_shell_retry_loop.rs"]
mod test_shell_retry_loop;
