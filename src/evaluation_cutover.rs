//! Versioned retirement boundary for pre-receipt synthetic agency graph rows.
//!
//! `.assign-*`, `.flip-*`, and `.evaluate-*` rows remain readable historical
//! evidence, but a row carrying [`EVALUATION_CUTOVER_TAG`] has no scheduling or
//! dependency authority.  The explicit migration is the only writer of the
//! tag; merely loading an old graph never changes its meaning.

use crate::graph::{Node, Status, Task, WorkGraph};
use crate::identity::canonical_json;

pub const EVALUATION_CUTOVER_VERSION: u32 = 1;
pub const EVALUATION_CUTOVER_TAG: &str = "evaluation-cutover:v1:historical-inert";
pub const EVALUATION_CUTOVER_DIR: &str = "migrations/evaluation-cutover-v1";

pub fn is_retired_agency_task_id(task_id: &str) -> bool {
    task_id.starts_with(".assign-")
        || task_id.starts_with(".flip-")
        || task_id.starts_with(".evaluate-")
}

pub fn is_cutover_inert(task: &Task) -> bool {
    is_retired_agency_task_id(&task.id) && task.tags.iter().any(|tag| tag == EVALUATION_CUTOVER_TAG)
}

pub fn source_id(task_id: &str) -> Option<&str> {
    [".assign-", ".flip-", ".evaluate-"]
        .into_iter()
        .find_map(|prefix| task_id.strip_prefix(prefix))
}

/// Stable binding used by the explicit operator adjudication path.
///
/// Modern tasks bind to their immutable completion manifest.  A graph too old
/// to have one binds instead to canonical bytes of the complete legacy source
/// row.  In either case an unrelated `wg evaluate record` score cannot change
/// or satisfy this binding.
pub fn candidate_binding(task: &Task) -> String {
    if let Some(candidate) = task.completion_candidate.as_ref() {
        return candidate.manifest.content_digest.to_string();
    }
    let value = serde_json::to_value(Node::Task(task.clone())).expect("Task serializes");
    format!("b3:{}", blake3::hash(&canonical_json(&value)).to_hex())
}

pub fn pending_cutover_count(graph: &WorkGraph) -> usize {
    let unretired = graph
        .tasks()
        .filter(|task| is_retired_agency_task_id(&task.id) && !is_cutover_inert(task))
        .count();
    let ambiguous_sources = graph
        .tasks()
        .filter(|task| matches!(task.status, Status::PendingEval | Status::FailedPendingEval))
        .count();
    unretired + ambiguous_sources
}

/// Human-facing condition for `wg show`.
pub fn condition_for(graph: &WorkGraph, task_id: &str) -> Option<String> {
    let task = graph.get_task(task_id)?;
    if is_retired_agency_task_id(task_id) {
        return Some(if is_cutover_inert(task) {
            "retired synthetic agency row retained as inert historical evidence by evaluation-cutover v1; it cannot schedule work or satisfy/block dependencies".to_string()
        } else {
            "legacy synthetic agency row still has compatibility authority; run `wg migrate evaluation-cutover`".to_string()
        });
    }
    if matches!(task.status, Status::PendingEval | Status::FailedPendingEval) {
        return Some(format!(
            "legacy {} source requires evaluation-cutover v1; run `wg migrate evaluation-cutover` (ordinary `wg evaluate record` scores are advisory and cannot accept candidate {})",
            task.status,
            candidate_binding(task)
        ));
    }
    let eval_id = format!(".evaluate-{task_id}");
    graph.get_task(&eval_id).map(|eval| {
        if is_cutover_inert(eval) {
            format!(
                "legacy evaluator {eval_id} is retained as inert historical evidence (evaluation-cutover v1 applied)"
            )
        } else {
            format!(
                "legacy evaluator {eval_id} can still block downstream work; run `wg migrate evaluation-cutover`; ordinary `wg evaluate record` scores cannot accept an exact candidate"
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_scores_are_not_part_of_candidate_binding() {
        let task = Task {
            id: "source".into(),
            title: "source".into(),
            status: Status::PendingEval,
            ..Task::default()
        };
        let before = candidate_binding(&task);
        let unrelated_score_record = serde_json::json!({"task":"source","score":1.0});
        assert_eq!(unrelated_score_record["score"], 1.0);
        // The scored-evaluation store is not an input to the binding; only the
        // exact graph candidate (or modern manifest) is.
        assert_eq!(before, candidate_binding(&task));
    }
}
