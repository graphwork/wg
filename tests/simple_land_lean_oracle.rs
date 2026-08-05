use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;
use serde_json::{Value, json};
use worksgood::simple_land::{
    CompletionContract, CompletionManifestProjection, ReviewVerdict, SimpleDecision,
    SimpleLandEvent, SimpleLandState, replay_simple_land,
};

#[derive(Debug, Deserialize)]
struct FixtureFile {
    cases: Vec<FixtureCase>,
}

#[derive(Debug, Deserialize)]
struct FixtureCase {
    name: String,
    events: Vec<SimpleLandEvent>,
}

fn fixtures() -> FixtureFile {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("formal/fixtures/simple-land/v1/scenarios.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn oracle_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("formal/.lake/build/bin/simple-land-oracle")
}

fn contract_name(contract: CompletionContract) -> &'static str {
    match contract {
        CompletionContract::Land => "land",
        CompletionContract::Report => "report",
        CompletionContract::Explore => "explore",
    }
}

fn verdict_name(verdict: ReviewVerdict) -> &'static str {
    match verdict {
        ReviewVerdict::Absent => "absent",
        ReviewVerdict::Pass => "pass",
        ReviewVerdict::Reject => "reject",
        ReviewVerdict::Unavailable => "unavailable",
        ReviewVerdict::IncompleteEvidence => "incompleteEvidence",
    }
}

fn decision_name(decision: SimpleDecision) -> &'static str {
    match decision {
        SimpleDecision::Applied => "applied",
        SimpleDecision::Noop => "noop",
        SimpleDecision::Rejected => "rejected",
    }
}

fn manifest_json(manifest: &CompletionManifestProjection) -> Value {
    json!({
        "id": manifest.id,
        "requirements": manifest.requirements,
        "contract": contract_name(manifest.contract),
        "outputDigest": manifest.output_digest,
        "validationDigest": manifest.validation_digest,
        "integratedMain": manifest.integrated_main,
        "allResolvable": manifest.all_resolvable,
        "protectedFree": manifest.protected_free,
    })
}

fn event_json(event: &SimpleLandEvent) -> Value {
    match event {
        SimpleLandEvent::SubmitManifest { manifest } => {
            json!({"submitManifest": {"manifest": manifest_json(manifest)}})
        }
        SimpleLandEvent::RecordFlip {
            manifest,
            requirements,
            verdict,
        } => json!({"recordFlip": {
            "manifest": manifest,
            "requirements": requirements,
            "verdict": verdict_name(*verdict),
        }}),
        SimpleLandEvent::RecordEval {
            manifest,
            requirements,
            verdict,
        } => json!({"recordEval": {
            "manifest": manifest,
            "requirements": requirements,
            "verdict": verdict_name(*verdict),
        }}),
        SimpleLandEvent::PublishObserved {
            manifest,
            observed_main,
            succeeded,
            outputs_match,
        } => json!({"publishObserved": {
            "manifest": manifest,
            "observedMain": observed_main,
            "succeeded": succeeded,
            "outputsMatch": outputs_match,
        }}),
        SimpleLandEvent::Complete {
            manifest,
            outputs_still_resolve,
        } => json!({"complete": {
            "manifest": manifest,
            "outputsStillResolve": outputs_still_resolve,
        }}),
        SimpleLandEvent::Fail { code } => json!({"fail": {"code": code}}),
        SimpleLandEvent::Retry => json!("retry"),
    }
}

fn state_json(state: &SimpleLandState) -> Value {
    json!({
        "phase": match state.phase {
            worksgood::simple_land::SimplePhase::Working => "working",
            worksgood::simple_land::SimplePhase::ReviewBlocked => "reviewBlocked",
            worksgood::simple_land::SimplePhase::ReviewUnavailable => "reviewUnavailable",
            worksgood::simple_land::SimplePhase::Accepted => "accepted",
            worksgood::simple_land::SimplePhase::Published => "published",
            worksgood::simple_land::SimplePhase::Done => "done",
            worksgood::simple_land::SimplePhase::Failed => "failed",
        },
        "manifest": state.manifest.as_ref().map(manifest_json),
        "flip": {
            "manifest": state.flip.manifest,
            "requirements": state.flip.requirements,
            "verdict": verdict_name(state.flip.verdict),
        },
        "eval": {
            "manifest": state.eval.manifest,
            "requirements": state.eval.requirements,
            "verdict": verdict_name(state.eval.verdict),
        },
        "publication": state.publication.as_ref().map(|publication| json!({
            "manifest": publication.manifest,
            "outputDigest": publication.output_digest,
        })),
        "publicationCount": state.publication_count,
        "failureCode": state.failure_code,
    })
}

fn run_oracle(state: &SimpleLandState, events: &[SimpleLandEvent]) -> Value {
    let path = oracle_path();
    assert!(
        path.is_file(),
        "Lean oracle missing at {}; run `cd formal && lake build simple-land-oracle`",
        path.display()
    );
    let request = json!({
        "state": state_json(state),
        "events": events.iter().map(event_json).collect::<Vec<_>>(),
    });
    let mut child = Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&request).unwrap().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Lean oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn lean_oracle_and_rust_reducer_agree_on_reference_scenarios() {
    for case in fixtures().cases {
        let initial = SimpleLandState::default();
        let (rust_state, rust_decisions) = replay_simple_land(&initial, &case.events);
        let lean = run_oracle(&initial, &case.events);
        let expected_decisions = rust_decisions
            .iter()
            .map(|decision| Value::String(decision_name(*decision).to_string()))
            .collect::<Vec<_>>();
        assert_eq!(
            lean.get("decisions")
                .and_then(Value::as_array)
                .unwrap()
                .as_slice(),
            expected_decisions.as_slice(),
            "{} decisions",
            case.name
        );
        assert_eq!(
            lean.get("state"),
            Some(&state_json(&rust_state)),
            "{} final state",
            case.name
        );
    }
}
