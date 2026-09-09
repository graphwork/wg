mod common;

use common::adaptive::*;
use std::fs;
use worksgood::adaptive_agency::*;

#[test]
fn rebuild_after_partial_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdaptiveStore::open(dir.path()).unwrap();
    let binding = candidate(1, "manifest-a");
    store
        .selection_sink()
        .select(binding.clone(), "2026-09-03T00:00:00Z")
        .unwrap();
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
    let expected = projector.performance_projection().unwrap();
    let cache = dir
        .path()
        .join("agency/adaptive/v1/performance-projections/terminal-episode-v1.json");
    fs::write(&cache, b"partial").unwrap();
    let rebuilt = projector.performance_projection().unwrap();
    assert_eq!(rebuilt, expected);
    assert_eq!(rebuilt.task_count, 1);
}
