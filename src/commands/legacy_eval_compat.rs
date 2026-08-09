//! Load-only compatibility for pre-receipt synthetic agency tasks.
//!
//! Current assignment and review are receipts on the source attempt. This
//! module may retire a legacy row only when the graph proves it was never
//! claimed and carries no evaluation evidence. It never creates, schedules,
//! rearms, or interprets a synthetic `.assign-*`, `.flip-*`, or `.evaluate-*`
//! row.
//!
//! Removal condition: a versioned graph migration has rewritten every
//! supported pre-receipt graph and the loader deliberately rejects these rows
//! with a documented migration error.

use chrono::Utc;

use worksgood::graph::{Status, WorkGraph};
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};

/// Retire an unclaimed, evidence-free synthetic row and remove an assignment
/// dependency from its source. Claimed, started, terminal, or verdict-bearing
/// rows remain visible to the read-only evidence migration; this function
/// never guesses their outcome.
pub fn retire_safe_synthetic_rows(
    graph: &mut WorkGraph,
    source_id: &str,
    candidate_completion: bool,
) -> usize {
    let source_eligible = graph.get_task(source_id).is_some_and(|source| {
        candidate_completion
            || (source.status == Status::Open
                && source
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .is_none_or(|attempt| attempt.disposition.is_some()))
    });
    if !source_eligible {
        return 0;
    }

    let mut retired = 0;
    for (satellite_id, assignment) in [
        (format!(".assign-{source_id}"), true),
        (format!(".flip-{source_id}"), false),
        (format!(".evaluate-{source_id}"), false),
    ] {
        let safe = graph.get_task(&satellite_id).is_some_and(|satellite| {
            matches!(
                satellite.status,
                Status::Open | Status::Blocked | Status::Waiting
            ) && satellite.assigned.is_none()
                && satellite.started_at.is_none()
                && satellite
                    .evaluation_lifecycle
                    .as_ref()
                    .is_none_or(|lifecycle| {
                        lifecycle.linked_flip_verdict.is_none()
                            && lifecycle.linked_eval_verdict.is_none()
                            && lifecycle.consumed_verdict.is_none()
                    })
        });
        if !safe {
            continue;
        }
        let satellite = graph
            .get_task_mut(&satellite_id)
            .expect("safe check established satellite existence");
        let request = TransitionRequest::new(
            TransitionKind::Abandoned,
            LifecycleActor {
                kind: ActorKind::Operator,
                id: "agency-receipt-migration".to_string(),
            },
            "synthetic_agency_task_retired",
            format!("retire-synthetic-agency:{satellite_id}"),
        )
        .expecting(FenceExpectation::current(satellite));
        if apply_transition(satellite, request).is_err() {
            continue;
        }
        satellite.completed_at = Some(Utc::now().to_rfc3339());
        if assignment && let Some(source) = graph.get_task_mut(source_id) {
            source
                .after
                .retain(|dependency| dependency != &satellite_id);
        }
        retired += 1;
    }
    retired
}

#[cfg(test)]
mod tests {
    use super::*;
    use worksgood::graph::{Node, Task};

    #[test]
    fn retires_safe_synthetic_tasks_and_unwires_assignment() {
        let mut source = Task {
            id: "source".into(),
            title: "source".into(),
            after: vec![".assign-source".into()],
            ..Task::default()
        };
        source.status = Status::Open;
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(source));
        for id in [".assign-source", ".flip-source", ".evaluate-source"] {
            graph.add_node(Node::Task(Task {
                id: id.into(),
                title: id.into(),
                status: Status::Open,
                presentation: worksgood::graph::TaskPresentation::Plumbing,
                ..Task::default()
            }));
        }

        assert_eq!(retire_safe_synthetic_rows(&mut graph, "source", false), 3);
        assert!(graph.get_task("source").unwrap().after.is_empty());
        for id in [".assign-source", ".flip-source", ".evaluate-source"] {
            let task = graph.get_task(id).unwrap();
            assert_eq!(task.status, Status::Abandoned);
            assert_eq!(task.lifecycle.audit.len(), 1);
        }
    }

    #[test]
    fn claimed_synthetic_task_is_left_for_explicit_migration() {
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(Task {
            id: "source".into(),
            title: "source".into(),
            ..Task::default()
        }));
        graph.add_node(Node::Task(Task {
            id: ".assign-source".into(),
            title: "assign".into(),
            status: Status::Open,
            assigned: Some("agent-1".into()),
            ..Task::default()
        }));
        assert_eq!(retire_safe_synthetic_rows(&mut graph, "source", false), 0);
        assert_eq!(
            graph.get_task(".assign-source").unwrap().status,
            Status::Open
        );
    }
}
