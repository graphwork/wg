//! Direct-assignment history helpers.
//!
//! The coordinator-side lightweight assignment broker and its prompt/verdict
//! protocol are retired. Explicit `wg assign --auto` still ranks agents through
//! these read-only history partitions and writes assignment metadata directly;
//! it never creates a `.assign-*` graph task.

use worksgood::agency::{Agent, EvaluationRef};
use worksgood::graph::{Task, WorkGraph, is_system_task};

/// History partition used for explicit assignment evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssignmentHistoryClass {
    ActualWork,
    SystemAgency,
}

impl AssignmentHistoryClass {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ActualWork => "actual_work",
            Self::SystemAgency => "system_agency",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ScopedPerformance {
    pub avg_score: Option<f64>,
    pub task_count: u32,
}

pub(crate) fn classify_task_history(task: &Task) -> AssignmentHistoryClass {
    if is_system_task(&task.id) {
        AssignmentHistoryClass::SystemAgency
    } else {
        AssignmentHistoryClass::ActualWork
    }
}

pub(crate) fn history_class_for_assignment(task: &Task) -> AssignmentHistoryClass {
    if worksgood::assignment_eligibility::task_uses_work_pool(task) {
        AssignmentHistoryClass::ActualWork
    } else {
        AssignmentHistoryClass::SystemAgency
    }
}

fn classify_history_ref(
    evaluation: &EvaluationRef,
    graph: Option<&WorkGraph>,
) -> AssignmentHistoryClass {
    graph
        .and_then(|graph| graph.get_task(&evaluation.task_id))
        .map(classify_task_history)
        .unwrap_or_else(|| {
            if is_system_task(&evaluation.task_id) {
                AssignmentHistoryClass::SystemAgency
            } else {
                AssignmentHistoryClass::ActualWork
            }
        })
}

/// Compute performance from lifecycle-verified history in one partition.
pub(crate) fn scoped_performance_for_agent(
    agent: &Agent,
    graph: Option<&WorkGraph>,
    history_class: AssignmentHistoryClass,
) -> ScopedPerformance {
    let scores: Vec<f64> = agent
        .performance
        .evaluations
        .iter()
        .filter(|evaluation| classify_history_ref(evaluation, graph) == history_class)
        .filter(|evaluation| {
            graph
                .and_then(|graph| graph.get_task(&evaluation.task_id))
                .is_some_and(|task| task.graph_save_completion_disposition().is_some())
        })
        .map(|evaluation| evaluation.score)
        .filter(|score| score.is_finite())
        .collect();

    if scores.is_empty() {
        // Preserve bootstrap system-agent aggregates minted before per-task
        // references existed. This statistic cannot authorize graph mutation.
        if history_class == AssignmentHistoryClass::SystemAgency
            && agent.performance.evaluations.is_empty()
            && agent.performance.task_count > 0
        {
            return ScopedPerformance {
                avg_score: agent.performance.avg_score,
                task_count: agent.performance.task_count,
            };
        }
        return ScopedPerformance::default();
    }

    let average = scores.iter().sum::<f64>() / scores.len() as f64;
    ScopedPerformance {
        avg_score: average.is_finite().then_some(average),
        task_count: scores.len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use worksgood::agency::{Lineage, PerformanceRecord};
    use worksgood::graph::{Node, Status};

    fn agent(evaluations: Vec<EvaluationRef>) -> Agent {
        Agent {
            id: "agent".into(),
            role_id: "role".into(),
            tradeoff_id: "tradeoff".into(),
            name: "agent".into(),
            performance: PerformanceRecord {
                task_count: evaluations.len() as u32,
                avg_score: None,
                evaluations,
            },
            lineage: Lineage::default(),
            capabilities: Vec::new(),
            rate: None,
            capacity: None,
            trust_level: Default::default(),
            contact: None,
            executor: "claude".into(),
            preferred_model: None,
            preferred_provider: None,
            attractor_weight: 1.0,
            deployment_history: Vec::new(),
            staleness_flags: Vec::new(),
        }
    }

    fn evaluation(task_id: &str, score: f64) -> EvaluationRef {
        EvaluationRef {
            score,
            task_id: task_id.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            context_id: "role".into(),
        }
    }

    #[test]
    fn explicit_assignment_ignores_unverified_and_synthetic_history() {
        let verified = Task {
            id: "work".into(),
            title: "work".into(),
            status: Status::Done,
            ..Task::default()
        };
        let synthetic = Task {
            id: ".evaluate-work".into(),
            title: "legacy evaluator".into(),
            status: Status::Done,
            ..Task::default()
        };
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(verified));
        graph.add_node(Node::Task(synthetic));
        let agent = agent(vec![
            evaluation("work", 0.8),
            evaluation(".evaluate-work", 1.0),
        ]);

        assert_eq!(
            scoped_performance_for_agent(&agent, Some(&graph), AssignmentHistoryClass::ActualWork,),
            ScopedPerformance::default()
        );
        assert_eq!(
            scoped_performance_for_agent(
                &agent,
                Some(&graph),
                AssignmentHistoryClass::SystemAgency,
            ),
            ScopedPerformance::default()
        );
    }
}
