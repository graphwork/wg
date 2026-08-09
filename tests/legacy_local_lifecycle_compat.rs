use tempfile::tempdir;
use worksgood::graph::Status;
use worksgood::parser::{load_graph, save_graph};

#[test]
fn pre_receipt_pending_eval_graph_loads_and_round_trips_without_inference() {
    let dir = tempdir().unwrap();
    let graph_path = dir.path().join("graph.jsonl");
    std::fs::write(
        &graph_path,
        include_bytes!("fixtures/legacy-local-lifecycle.jsonl"),
    )
    .unwrap();

    let graph = load_graph(&graph_path).unwrap();
    assert_eq!(
        graph.get_task("legacy-pending").unwrap().status,
        Status::PendingEval
    );
    assert_eq!(
        graph.get_task("legacy-failed-pending").unwrap().status,
        Status::FailedPendingEval
    );
    let historical = graph.get_task(".evaluate-legacy-pending").unwrap();
    assert_eq!(historical.status, Status::Done);
    assert_eq!(historical.assigned.as_deref(), Some("legacy-evaluator"));

    // Loading old bytes is intentionally lossless. The loader neither promotes
    // a soft state nor guesses that a terminal evaluator row means acceptance.
    save_graph(&graph, &graph_path).unwrap();
    let round_trip = load_graph(&graph_path).unwrap();
    assert_eq!(
        round_trip.get_task("legacy-pending").unwrap().status,
        Status::PendingEval
    );
    assert_eq!(
        round_trip.get_task("legacy-failed-pending").unwrap().status,
        Status::FailedPendingEval
    );
    assert_eq!(
        round_trip
            .get_task(".evaluate-legacy-pending")
            .unwrap()
            .status,
        Status::Done
    );
}
