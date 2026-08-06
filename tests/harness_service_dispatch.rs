//! Consolidated integration-test domain harness.
//!
//! Keep top-level cases as modules so Cargo links this domain once rather than
//! producing one debuginfo-heavy executable per source file.

#[path = "integration_coordinator_agent.rs"]
mod integration_coordinator_agent;
#[path = "integration_coordinator_spawn_template.rs"]
mod integration_coordinator_spawn_template;
#[path = "integration_cron_dispatch.rs"]
mod integration_cron_dispatch;
#[path = "integration_cross_repo_dispatch.rs"]
mod integration_cross_repo_dispatch;
#[path = "integration_dispatch_boot.rs"]
mod integration_dispatch_boot;
#[path = "integration_error_recovery.rs"]
mod integration_error_recovery;
#[path = "integration_heartbeat.rs"]
mod integration_heartbeat;
#[path = "integration_multi_coordinator.rs"]
mod integration_multi_coordinator;
#[path = "integration_multi_user_watcher.rs"]
mod integration_multi_user_watcher;
#[path = "integration_reap_kill_tree.rs"]
mod integration_reap_kill_tree;
#[path = "integration_scheduled_dispatch.rs"]
mod integration_scheduled_dispatch;
#[path = "integration_service.rs"]
mod integration_service;
#[path = "integration_service_control_permissions.rs"]
mod integration_service_control_permissions;
#[path = "integration_service_coordinator.rs"]
mod integration_service_coordinator;
#[path = "integration_triage.rs"]
mod integration_triage;
#[path = "integration_triage_smoke.rs"]
mod integration_triage_smoke;
#[path = "integration_worktree.rs"]
mod integration_worktree;
#[path = "integration_worktree_observer.rs"]
mod integration_worktree_observer;
#[path = "spawn_site_isolation.rs"]
mod spawn_site_isolation;
#[path = "test_coordinator_lifecycle.rs"]
mod test_coordinator_lifecycle;
#[path = "test_coordinator_special_agents.rs"]
mod test_coordinator_special_agents;
#[path = "test_cron_integration.rs"]
mod test_cron_integration;
#[path = "test_cron_serialization.rs"]
mod test_cron_serialization;
#[path = "test_orphaned_cleanup.rs"]
mod test_orphaned_cleanup;
#[path = "test_recovery_verification.rs"]
mod test_recovery_verification;
