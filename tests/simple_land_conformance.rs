use std::fs;
use std::path::Path;

use serde::Deserialize;
use worksgood::simple_land::{
    ReviewVerdict, SIMPLE_LAND_TRACE_SCHEMA_VERSION, SimpleDecision, SimpleLandEvent,
    SimpleLandState, SimplePhase, replay_simple_land,
};

#[derive(Debug, Deserialize)]
struct FixtureFile {
    schema_version: u32,
    cases: Vec<FixtureCase>,
}

#[derive(Debug, Deserialize)]
struct FixtureCase {
    name: String,
    events: Vec<SimpleLandEvent>,
    expected_decisions: Vec<SimpleDecision>,
    expected: ExpectedProjection,
}

#[derive(Debug, Deserialize)]
struct ExpectedProjection {
    phase: SimplePhase,
    manifest: Option<u64>,
    flip: ReviewVerdict,
    eval: ReviewVerdict,
    publication: Option<u64>,
    publication_count: u32,
    failure_code: Option<u64>,
}

fn fixtures() -> FixtureFile {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("formal/fixtures/simple-land/v1/scenarios.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn rust_replays_the_simple_land_reference_scenarios() {
    let fixtures = fixtures();
    assert_eq!(fixtures.schema_version, SIMPLE_LAND_TRACE_SCHEMA_VERSION);
    assert_eq!(fixtures.cases.len(), 6);

    for case in fixtures.cases {
        let (state, decisions) = replay_simple_land(&SimpleLandState::default(), &case.events);
        assert_eq!(
            decisions, case.expected_decisions,
            "{} decisions",
            case.name
        );
        assert_eq!(state.phase, case.expected.phase, "{} phase", case.name);
        assert_eq!(
            state.manifest.as_ref().map(|manifest| manifest.id),
            case.expected.manifest,
            "{} manifest",
            case.name
        );
        assert_eq!(state.flip.verdict, case.expected.flip, "{} FLIP", case.name);
        assert_eq!(state.eval.verdict, case.expected.eval, "{} eval", case.name);
        assert_eq!(
            state
                .publication
                .as_ref()
                .map(|publication| publication.manifest),
            case.expected.publication,
            "{} publication",
            case.name
        );
        assert_eq!(
            state.publication_count, case.expected.publication_count,
            "{} publication count",
            case.name
        );
        assert_eq!(
            state.failure_code, case.expected.failure_code,
            "{} failure code",
            case.name
        );
    }
}

#[test]
fn terminal_replay_is_byte_stable_and_inert() {
    let fixtures = fixtures();
    let happy = fixtures
        .cases
        .into_iter()
        .find(|case| case.name == "happy_land")
        .unwrap();
    let (done, _) = replay_simple_land(&SimpleLandState::default(), &happy.events);
    let before = serde_json::to_vec(&done).unwrap();
    let (after, decisions) = replay_simple_land(
        &done,
        &[SimpleLandEvent::Fail { code: 9 }, SimpleLandEvent::Retry],
    );
    assert_eq!(decisions, vec![SimpleDecision::Noop, SimpleDecision::Noop]);
    assert_eq!(serde_json::to_vec(&after).unwrap(), before);
}
