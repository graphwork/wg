mod common;

use common::adaptive::*;
use std::fs;
use worksgood::adaptive_agency::*;
use worksgood::completion_review::ReviewerKind;

#[test]
fn adaptive_capabilities_are_observation_only_and_leave_graph_bytes_unchanged() {
    let source = fs::read_to_string("src/adaptive_agency.rs").unwrap();
    for forbidden in [
        "use crate::graph",
        "use crate::lifecycle",
        "crate::lifecycle::",
        "std::process::Command",
        ".add_node(",
        ".add_dependency(",
    ] {
        assert!(
            !source.contains(forbidden),
            "adaptive package must not import/call forbidden authority: {forbidden}"
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let graph = dir.path().join("graph.jsonl");
    fs::write(&graph, b"immutable-source-lifecycle-bytes\n").unwrap();
    let before = blake3::hash(&fs::read(&graph).unwrap());
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let binding = candidate(1, "manifest-a");
    store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
    let attempt = start(
        &store,
        binding.clone(),
        ReviewerKind::Flip,
        0,
        "2026-09-03T00:00:00Z",
        "2026-09-03T00:00:01Z",
    );
    finish(
        &store,
        &attempt,
        ReviewOutcomeV1::Semantic(SemanticOutcome::Pass),
        "receipt",
        None,
    );
    let projector = store.learning_projector();
    let seal = projector
        .seal_trajectory(
            "graph-test",
            "task-a",
            2,
            "terminal-1",
            "2026-09-03T00:00:10Z",
        )
        .unwrap();
    projector
        .project(terminal_input(binding, "terminal-1"), &seal)
        .unwrap();
    assert_eq!(before, blake3::hash(&fs::read(&graph).unwrap()));
}

#[test]
fn virtual_alias_is_not_a_task_identifier() {
    let alias = virtual_alias(&candidate(1, "manifest-a"), ReviewerKind::Flip, 1);
    assert!(is_virtual_review_alias(&alias));
    assert!(non_authoritative_error(&alias).contains("WG-VIRTUAL-REVIEW-NON-AUTHORITATIVE"));
}
