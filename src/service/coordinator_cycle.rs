//! Coordinator cycle validation — detects unsafe patterns before they cause deadlock.
//!
//! Validates the coordinator/compact/archive cycle structure to prevent regressions:
//! 1. No circular coordinator↔archive dependency (deadlock)
//! 2. heartbeat_interval > 0 on autonomous coordinators
//! 3. Context injection path exists (compact → context.md → coordinator)
//!
//! ## Safe Pattern
//!
//! ```text
//! .coordinator-N → .compact-N → .coordinator-N (OK: sequential cycle)
//! .archive-N     → (independent, NOT gated by coordinator)
//! ```
//!
//! ## Unsafe Pattern (Detected)
//!
//! ```text
//! .coordinator-N → .archive-N → .coordinator-N (DEADLOCK)
//! ```

use std::collections::HashSet;

use crate::config::Config;
use crate::graph::WorkGraph;

/// A warning issued during coordinator cycle validation.
#[derive(Debug, Clone)]
pub struct CoordinatorCycleWarning {
    /// Severity level
    pub severity: WarningSeverity,
    /// Human-readable message
    pub message: String,
    /// Task ID this warning relates to (if any)
    pub task_id: Option<String>,
}

/// Warning severity levels
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningSeverity {
    /// Issue that will cause immediate deadlock — should block coordinator creation
    Error,
    /// Issue that may cause problems under certain conditions — should warn
    Warning,
    /// Informational note — no action required
    Info,
}

/// Check if a task ID matches an archive task pattern.
fn is_archive_task(task_id: &str) -> bool {
    task_id.starts_with(".archive-")
}

/// Check if a task ID matches a coordinator task pattern.
fn is_coordinator_task(task_id: &str) -> bool {
    task_id.starts_with(".coordinator-") || task_id == ".coordinator"
}

/// Check if a task ID matches a compact task pattern.
fn is_compact_task(task_id: &str) -> bool {
    task_id.starts_with(".compact-")
}

/// Detect circular coordinator↔archive dependency.
///
/// This is the bug that hit Coordinator-22: archive was added as a dependency,
/// creating a deadlock:
///   .coordinator → .archive → .coordinator (deadlock)
///
/// Archive must run independently, NOT gated by coordinator.
fn check_circular_archive_dependency(
    graph: &WorkGraph,
    coordinator_id: &str,
) -> Option<CoordinatorCycleWarning> {
    let coordinator = graph.get_task(coordinator_id)?;

    // Check if coordinator has any archive task in its after list
    for dep in &coordinator.after {
        if is_archive_task(dep) {
            return Some(CoordinatorCycleWarning {
                severity: WarningSeverity::Error,
                message: format!(
                    "Coordinator '{}' depends on archive task '{}' — this creates a circular \
                     dependency deadlock. Archive tasks must run independently and NOT be \
                     blockers on coordinator iteration.",
                    coordinator_id, dep
                ),
                task_id: Some(coordinator_id.to_string()),
            });
        }
    }

    // Also check if coordinator depends on any task that transitively depends on coordinator
    // (full cycle detection)
    let mut visited: HashSet<String> = std::collections::HashSet::new();
    let mut to_check: Vec<String> = coordinator.after.clone();
    visited.insert(coordinator_id.to_string());

    while let Some(task_id) = to_check.pop() {
        if visited.contains(&task_id) {
            continue;
        }
        visited.insert(task_id.clone());

        if is_archive_task(&task_id) {
            return Some(CoordinatorCycleWarning {
                severity: WarningSeverity::Error,
                message: format!(
                    "Coordinator '{}' transitively depends on archive task '{}' — this creates \
                     a circular dependency deadlock. Archive tasks must run independently.",
                    coordinator_id, task_id
                ),
                task_id: Some(coordinator_id.to_string()),
            });
        }

        if let Some(task) = graph.get_task(&task_id) {
            for dep in &task.after {
                if !visited.contains(dep) {
                    to_check.push(dep.clone());
                }
            }
        }
    }

    None
}

/// Check heartbeat_interval configuration.
///
/// An autonomous coordinator with heartbeat_interval=0 and no trigger mechanism
/// will stall if no events fire. Warn users who might expect autonomous behavior.
fn check_heartbeat_configuration(
    graph: &WorkGraph,
    coordinator_id: &str,
) -> Option<CoordinatorCycleWarning> {
    let _coordinator = graph.get_task(coordinator_id)?;
    let config = Config::load_or_default(std::path::Path::new("."));

    // If heartbeat_interval is explicitly set to 0, check for trigger mechanisms
    if config.coordinator.heartbeat_interval == 0 {
        // Check if there's a user board or other trigger mechanism
        let has_user_board = graph
            .tasks()
            .any(|t| t.tags.iter().any(|tag| tag == "user-board"));

        if !has_user_board {
            return Some(CoordinatorCycleWarning {
                severity: WarningSeverity::Warning,
                message: format!(
                    "Coordinator '{}' has heartbeat_interval=0 (autonomous heartbeats disabled) \
                     and no user board detected. The coordinator will only run on GraphChanged \
                     events or manual messages. Set heartbeat_interval > 0 for autonomous \
                     operation, or ensure external trigger mechanisms exist.",
                    coordinator_id
                ),
                task_id: Some(coordinator_id.to_string()),
            });
        }
    }

    None
}

/// Check that the context injection path exists.
///
/// For coordinator to receive compaction output, we need:
/// 1. A compact task in the graph
/// 2. Compact task completes successfully
/// 3. context.md is written to .workgraph/compactor/context.md
///
/// This is informational — the path is established by the cycle structure.
fn check_context_injection_path(
    graph: &WorkGraph,
    coordinator_id: &str,
) -> Option<CoordinatorCycleWarning> {
    let coordinator = graph.get_task(coordinator_id)?;

    // Check if there's a compact task that coordinator waits for
    let has_compact_dependency = coordinator.after.iter().any(|dep| is_compact_task(dep));

    if !has_compact_dependency {
        return Some(CoordinatorCycleWarning {
            severity: WarningSeverity::Warning,
            message: format!(
                "Coordinator '{}' does not depend on any compact task. Compaction output \
                 will not be injected into coordinator context. Add a compact task dependency \
                 (e.g., .compact-0) to enable automatic context distillation.",
                coordinator_id
            ),
            task_id: Some(coordinator_id.to_string()),
        });
    }

    None
}

/// Check that archive task does NOT have coordinator in its dependencies.
///
/// Archive should be independent — if it depends on coordinator, the cycle
/// check_circular_archive_dependency will catch the coordinator→archive case,
/// but we should also catch archive→coordinator (which is also wrong).
fn check_archive_independence(graph: &WorkGraph) -> Vec<CoordinatorCycleWarning> {
    let mut warnings = Vec::new();

    for task in graph.tasks() {
        if !is_archive_task(&task.id) {
            continue;
        }

        // Check if archive depends on coordinator
        for dep in &task.after {
            if is_coordinator_task(dep) {
                warnings.push(CoordinatorCycleWarning {
                    severity: WarningSeverity::Error,
                    message: format!(
                        "Archive task '{}' depends on coordinator task '{}'. Archive must be \
                         INDEPENDENT and NOT wait for coordinator. Remove this dependency to \
                         prevent deadlock.",
                        task.id, dep
                    ),
                    task_id: Some(task.id.clone()),
                });
            }
        }

        // Check if archive has no dependencies (correct for independent operation)
        // or only depends on things that don't form cycles
    }

    warnings
}

/// Validate coordinator cycle structure for a specific coordinator.
///
/// Returns a list of warnings/issues found. Empty list means the cycle is safe.
pub fn validate_coordinator_cycle(
    graph: &WorkGraph,
    coordinator_id: &str,
) -> Vec<CoordinatorCycleWarning> {
    let mut warnings = Vec::new();

    // Check for circular coordinator↔archive dependency
    if let Some(warning) = check_circular_archive_dependency(graph, coordinator_id) {
        warnings.push(warning);
    }

    // Check heartbeat_interval configuration
    if let Some(warning) = check_heartbeat_configuration(graph, coordinator_id) {
        warnings.push(warning);
    }

    // Check context injection path
    if let Some(warning) = check_context_injection_path(graph, coordinator_id) {
        warnings.push(warning);
    }

    warnings
}

/// Validate all coordinator cycles in the graph.
///
/// Checks all coordinators and also validates archive task independence.
pub fn validate_all_coordinator_cycles(graph: &WorkGraph) -> Vec<CoordinatorCycleWarning> {
    let mut warnings = Vec::new();

    // Validate each coordinator
    for task in graph.tasks() {
        if is_coordinator_task(&task.id) {
            warnings.extend(validate_coordinator_cycle(graph, &task.id));
        }
    }

    // Check archive task independence
    warnings.extend(check_archive_independence(graph));

    warnings
}

/// Check if any warnings have Error severity.
pub fn has_critical_warnings(warnings: &[CoordinatorCycleWarning]) -> bool {
    warnings
        .iter()
        .any(|w| w.severity == WarningSeverity::Error)
}

impl std::fmt::Display for CoordinatorCycleWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.severity {
            WarningSeverity::Error => write!(f, "[ERROR] {}", self.message),
            WarningSeverity::Warning => write!(f, "[WARNING] {}", self.message),
            WarningSeverity::Info => write!(f, "[INFO] {}", self.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Node, Status, Task};

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            status: Status::Open,
            after: vec![],
            tags: vec![],
            ..Default::default()
        }
    }

    fn build_graph(tasks: Vec<Task>) -> WorkGraph {
        let mut graph = WorkGraph::new();
        for task in tasks {
            graph.add_node(Node::Task(task));
        }
        graph
    }

    #[test]
    fn test_safe_coordinator_compact_cycle() {
        // Safe: coordinator → compact → coordinator
        let mut coordinator = make_task(".coordinator-0", "Coordinator 0");
        coordinator.tags.push("coordinator-loop".to_string());

        let mut compact = make_task(".compact-0", "Compact 0");
        compact.tags.push("compact-loop".to_string());
        compact.after = vec![".coordinator-0".to_string()];

        coordinator.after = vec![".compact-0".to_string()];

        let graph = build_graph(vec![coordinator, compact]);
        let warnings = validate_coordinator_cycle(&graph, ".coordinator-0");

        assert!(
            !has_critical_warnings(&warnings),
            "Safe pattern should have no errors: {:?}",
            warnings
        );
    }

    #[test]
    fn test_unsafe_coordinator_archive_dependency() {
        // Unsafe: coordinator → archive → coordinator (deadlock)
        let mut coordinator = make_task(".coordinator-0", "Coordinator 0");
        coordinator.tags.push("coordinator-loop".to_string());
        coordinator.after = vec![".archive-0".to_string()];

        let mut archive = make_task(".archive-0", "Archive 0");
        archive.tags.push("archive-loop".to_string());
        archive.after = vec![".coordinator-0".to_string()];

        let graph = build_graph(vec![coordinator, archive]);
        let warnings = validate_coordinator_cycle(&graph, ".coordinator-0");

        assert!(
            has_critical_warnings(&warnings),
            "Should detect circular dependency: {:?}",
            warnings
        );

        let error_msg = warnings
            .iter()
            .find(|w| w.severity == WarningSeverity::Error)
            .map(|w| w.message.clone());
        assert!(error_msg.is_some(), "Should have error message");
        assert!(
            error_msg.unwrap().contains("deadlock"),
            "Error should mention deadlock"
        );
    }

    #[test]
    fn test_unsafe_archive_depends_on_coordinator() {
        // Unsafe: archive depends on coordinator
        let mut coordinator = make_task(".coordinator-0", "Coordinator 0");
        coordinator.tags.push("coordinator-loop".to_string());

        let mut archive = make_task(".archive-0", "Archive 0");
        archive.tags.push("archive-loop".to_string());
        archive.after = vec![".coordinator-0".to_string()];

        let graph = build_graph(vec![coordinator, archive]);
        let warnings = check_archive_independence(&graph);

        assert!(
            has_critical_warnings(&warnings),
            "Should detect archive→coordinator dependency: {:?}",
            warnings
        );
    }

    #[test]
    fn test_safe_archive_independent() {
        // Safe: archive has no coordinator dependency
        let mut coordinator = make_task(".coordinator-0", "Coordinator 0");
        coordinator.tags.push("coordinator-loop".to_string());

        let mut archive = make_task(".archive-0", "Archive 0");
        archive.tags.push("archive-loop".to_string());
        // No after dependencies — archive runs independently

        let graph = build_graph(vec![coordinator, archive]);
        let warnings = check_archive_independence(&graph);

        assert!(
            warnings.is_empty(),
            "Independent archive should have no warnings: {:?}",
            warnings
        );
    }

    #[test]
    fn test_missing_compact_dependency_warning() {
        // Warning: coordinator has no compact dependency
        let mut coordinator = make_task(".coordinator-0", "Coordinator 0");
        coordinator.tags.push("coordinator-loop".to_string());
        // No compact in after list

        let graph = build_graph(vec![coordinator]);
        let warnings = validate_coordinator_cycle(&graph, ".coordinator-0");

        let has_warning = warnings
            .iter()
            .any(|w| w.severity == WarningSeverity::Warning && w.message.contains("compact"));
        assert!(
            has_warning,
            "Should warn about missing compact dependency: {:?}",
            warnings
        );
    }

    #[test]
    fn test_transitive_archive_dependency() {
        // Unsafe: coordinator → X → archive → ... (transitive archive dep)
        let mut coordinator = make_task(".coordinator-0", "Coordinator 0");
        coordinator.tags.push("coordinator-loop".to_string());
        coordinator.after = vec!["intermediate".to_string()];

        let mut intermediate = make_task("intermediate", "Intermediate");
        intermediate.after = vec![".archive-0".to_string()];

        let mut archive = make_task(".archive-0", "Archive 0");
        archive.tags.push("archive-loop".to_string());
        archive.after = vec![".coordinator-0".to_string()];

        let graph = build_graph(vec![coordinator, intermediate, archive]);
        let warnings = validate_coordinator_cycle(&graph, ".coordinator-0");

        assert!(
            has_critical_warnings(&warnings),
            "Should detect transitive archive dependency: {:?}",
            warnings
        );
    }

    #[test]
    fn test_coordinator_cycle_warning_display() {
        let warning = CoordinatorCycleWarning {
            severity: WarningSeverity::Error,
            message: "Test error".to_string(),
            task_id: Some("task-1".to_string()),
        };
        assert_eq!(format!("{}", warning), "[ERROR] Test error");

        let warning = CoordinatorCycleWarning {
            severity: WarningSeverity::Warning,
            message: "Test warning".to_string(),
            task_id: None,
        };
        assert_eq!(format!("{}", warning), "[WARNING] Test warning");
    }
}
