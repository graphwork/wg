//! Pure worker-owned completion reducer.
//!
//! This mirrors `formal/WGLifecycle/SimpleLand.lean`. It is intentionally
//! disconnected from production dispatch while the recovery design is built.
//! Git, output resolution, reviewers, and persistence are adapter facts; this
//! module only decides whether an observed transition is legal.

use serde::{Deserialize, Serialize};

pub const SIMPLE_LAND_SCHEMA_VERSION: u32 = 1;
pub const SIMPLE_LAND_TRACE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionContract {
    Land,
    Report,
    Explore,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimplePhase {
    Working,
    ReviewBlocked,
    ReviewUnavailable,
    Accepted,
    Published,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Absent,
    Pass,
    Reject,
    Unavailable,
    IncompleteEvidence,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionManifestProjection {
    pub id: u64,
    pub requirements: u64,
    pub contract: CompletionContract,
    pub output_digest: u64,
    pub validation_digest: u64,
    pub integrated_main: u64,
    pub all_resolvable: bool,
    pub protected_free: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewReceiptProjection {
    pub manifest: u64,
    pub requirements: u64,
    pub verdict: ReviewVerdict,
}

impl ReviewReceiptProjection {
    fn absent() -> Self {
        Self {
            manifest: 0,
            requirements: 0,
            verdict: ReviewVerdict::Absent,
        }
    }

    fn passing(manifest: &CompletionManifestProjection) -> Self {
        Self {
            manifest: manifest.id,
            requirements: manifest.requirements,
            verdict: ReviewVerdict::Pass,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicationReceiptProjection {
    pub manifest: u64,
    pub output_digest: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SimpleLandState {
    pub phase: SimplePhase,
    pub manifest: Option<CompletionManifestProjection>,
    pub flip: ReviewReceiptProjection,
    pub eval: ReviewReceiptProjection,
    pub publication: Option<PublicationReceiptProjection>,
    pub publication_count: u32,
    pub failure_code: Option<u64>,
}

impl Default for SimpleLandState {
    fn default() -> Self {
        Self {
            phase: SimplePhase::Working,
            manifest: None,
            flip: ReviewReceiptProjection::absent(),
            eval: ReviewReceiptProjection::absent(),
            publication: None,
            publication_count: 0,
            failure_code: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimpleLandEvent {
    SubmitManifest {
        manifest: CompletionManifestProjection,
    },
    RecordFlip {
        manifest: u64,
        requirements: u64,
        verdict: ReviewVerdict,
    },
    RecordEval {
        manifest: u64,
        requirements: u64,
        verdict: ReviewVerdict,
    },
    PublishObserved {
        manifest: u64,
        observed_main: u64,
        succeeded: bool,
        outputs_match: bool,
    },
    Complete {
        manifest: u64,
        outputs_still_resolve: bool,
    },
    Fail {
        code: u64,
    },
    Retry,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimpleDecision {
    Applied,
    Noop,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimpleTransition {
    pub state: SimpleLandState,
    pub decision: SimpleDecision,
}

fn transition(state: SimpleLandState, decision: SimpleDecision) -> SimpleTransition {
    SimpleTransition { state, decision }
}

fn rejected(state: &SimpleLandState) -> SimpleTransition {
    transition(state.clone(), SimpleDecision::Rejected)
}

fn review_phase(verdict: ReviewVerdict) -> SimplePhase {
    match verdict {
        ReviewVerdict::Reject | ReviewVerdict::IncompleteEvidence => SimplePhase::ReviewBlocked,
        ReviewVerdict::Unavailable => SimplePhase::ReviewUnavailable,
        ReviewVerdict::Absent | ReviewVerdict::Pass => SimplePhase::Working,
    }
}

fn manifest_is_valid(manifest: &CompletionManifestProjection) -> bool {
    manifest.all_resolvable
        && manifest.protected_free
        && manifest.id != 0
        && manifest.requirements != 0
        && manifest.output_digest != 0
        && manifest.validation_digest != 0
}

fn target_matches(manifest: &CompletionManifestProjection, observed_main: u64) -> bool {
    match manifest.contract {
        CompletionContract::Land => observed_main == manifest.integrated_main,
        CompletionContract::Report | CompletionContract::Explore => true,
    }
}

/// Apply one pure transition. Rejected and terminal-late events are state inert.
pub fn reduce_simple_land(state: &SimpleLandState, event: &SimpleLandEvent) -> SimpleTransition {
    if state.phase == SimplePhase::Done {
        return transition(state.clone(), SimpleDecision::Noop);
    }

    match event {
        SimpleLandEvent::SubmitManifest { manifest } => {
            if !manifest_is_valid(manifest) {
                return rejected(state);
            }
            if state.manifest.as_ref() == Some(manifest) {
                return transition(state.clone(), SimpleDecision::Noop);
            }
            transition(
                SimpleLandState {
                    phase: SimplePhase::Working,
                    manifest: Some(manifest.clone()),
                    flip: ReviewReceiptProjection::absent(),
                    eval: ReviewReceiptProjection::absent(),
                    publication: None,
                    publication_count: 0,
                    failure_code: None,
                },
                SimpleDecision::Applied,
            )
        }
        SimpleLandEvent::RecordFlip {
            manifest,
            requirements,
            verdict,
        } => {
            let Some(candidate) = state.manifest.as_ref() else {
                return rejected(state);
            };
            if candidate.id != *manifest
                || candidate.requirements != *requirements
                || *verdict == ReviewVerdict::Absent
            {
                return rejected(state);
            }
            let mut next = state.clone();
            next.phase = review_phase(*verdict);
            next.flip = ReviewReceiptProjection {
                manifest: *manifest,
                requirements: *requirements,
                verdict: *verdict,
            };
            next.eval = ReviewReceiptProjection::absent();
            next.publication = None;
            next.publication_count = 0;
            transition(next, SimpleDecision::Applied)
        }
        SimpleLandEvent::RecordEval {
            manifest,
            requirements,
            verdict,
        } => {
            let Some(candidate) = state.manifest.as_ref() else {
                return rejected(state);
            };
            if candidate.id != *manifest
                || candidate.requirements != *requirements
                || state.flip != ReviewReceiptProjection::passing(candidate)
                || *verdict == ReviewVerdict::Absent
            {
                return rejected(state);
            }
            let mut next = state.clone();
            next.phase = if *verdict == ReviewVerdict::Pass {
                SimplePhase::Accepted
            } else {
                review_phase(*verdict)
            };
            next.eval = ReviewReceiptProjection {
                manifest: *manifest,
                requirements: *requirements,
                verdict: *verdict,
            };
            next.publication = None;
            next.publication_count = 0;
            transition(next, SimpleDecision::Applied)
        }
        SimpleLandEvent::PublishObserved {
            manifest,
            observed_main,
            succeeded,
            outputs_match,
        } => {
            let Some(candidate) = state.manifest.as_ref() else {
                return rejected(state);
            };
            if state.phase != SimplePhase::Accepted
                || candidate.id != *manifest
                || state.flip != ReviewReceiptProjection::passing(candidate)
                || state.eval != ReviewReceiptProjection::passing(candidate)
                || !target_matches(candidate, *observed_main)
                || !succeeded
                || !outputs_match
            {
                return rejected(state);
            }
            let mut next = state.clone();
            next.phase = SimplePhase::Published;
            next.publication = Some(PublicationReceiptProjection {
                manifest: candidate.id,
                output_digest: candidate.output_digest,
            });
            next.publication_count = 1;
            transition(next, SimpleDecision::Applied)
        }
        SimpleLandEvent::Complete {
            manifest,
            outputs_still_resolve,
        } => {
            let (Some(candidate), Some(publication)) =
                (state.manifest.as_ref(), state.publication.as_ref())
            else {
                return rejected(state);
            };
            if state.phase != SimplePhase::Published
                || candidate.id != *manifest
                || publication.manifest != candidate.id
                || publication.output_digest != candidate.output_digest
                || state.flip != ReviewReceiptProjection::passing(candidate)
                || state.eval != ReviewReceiptProjection::passing(candidate)
                || !outputs_still_resolve
            {
                return rejected(state);
            }
            let mut next = state.clone();
            next.phase = SimplePhase::Done;
            next.failure_code = None;
            transition(next, SimpleDecision::Applied)
        }
        SimpleLandEvent::Fail { code } => {
            if state.phase == SimplePhase::Published {
                return transition(state.clone(), SimpleDecision::Noop);
            }
            let mut next = state.clone();
            next.phase = SimplePhase::Failed;
            next.failure_code = Some(*code);
            transition(next, SimpleDecision::Applied)
        }
        SimpleLandEvent::Retry => {
            if !matches!(
                state.phase,
                SimplePhase::Failed | SimplePhase::ReviewBlocked | SimplePhase::ReviewUnavailable
            ) {
                return rejected(state);
            }
            let mut next = state.clone();
            next.phase = SimplePhase::Working;
            next.flip = ReviewReceiptProjection::absent();
            next.eval = ReviewReceiptProjection::absent();
            next.publication = None;
            next.publication_count = 0;
            next.failure_code = None;
            transition(next, SimpleDecision::Applied)
        }
    }
}

pub fn replay_simple_land(
    initial: &SimpleLandState,
    events: &[SimpleLandEvent],
) -> (SimpleLandState, Vec<SimpleDecision>) {
    let mut state = initial.clone();
    let mut decisions = Vec::with_capacity(events.len());
    for event in events {
        let result = reduce_simple_land(&state, event);
        state = result.state;
        decisions.push(result.decision);
    }
    (state, decisions)
}

pub fn dependency_satisfied(state: &SimpleLandState) -> bool {
    state.phase == SimplePhase::Done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(contract: CompletionContract) -> CompletionManifestProjection {
        CompletionManifestProjection {
            id: 11,
            requirements: 22,
            contract,
            output_digest: 33,
            validation_digest: 44,
            integrated_main: 55,
            all_resolvable: true,
            protected_free: true,
        }
    }

    fn accepted(contract: CompletionContract) -> SimpleLandState {
        let manifest = manifest(contract);
        let events = [
            SimpleLandEvent::SubmitManifest {
                manifest: manifest.clone(),
            },
            SimpleLandEvent::RecordFlip {
                manifest: manifest.id,
                requirements: manifest.requirements,
                verdict: ReviewVerdict::Pass,
            },
            SimpleLandEvent::RecordEval {
                manifest: manifest.id,
                requirements: manifest.requirements,
                verdict: ReviewVerdict::Pass,
            },
        ];
        replay_simple_land(&SimpleLandState::default(), &events).0
    }

    #[test]
    fn happy_land_requires_both_reviews_and_exact_main() {
        let mut state = accepted(CompletionContract::Land);
        let manifest = state.manifest.clone().unwrap();
        let published = reduce_simple_land(
            &state,
            &SimpleLandEvent::PublishObserved {
                manifest: manifest.id,
                observed_main: manifest.integrated_main,
                succeeded: true,
                outputs_match: true,
            },
        );
        assert_eq!(published.decision, SimpleDecision::Applied);
        state = published.state;
        let done = reduce_simple_land(
            &state,
            &SimpleLandEvent::Complete {
                manifest: manifest.id,
                outputs_still_resolve: true,
            },
        );
        assert_eq!(done.state.phase, SimplePhase::Done);
        assert!(dependency_satisfied(&done.state));
    }

    #[test]
    fn moved_main_and_nonpassing_review_are_inert_rejections() {
        let state = accepted(CompletionContract::Land);
        let manifest = state.manifest.clone().unwrap();
        let moved = reduce_simple_land(
            &state,
            &SimpleLandEvent::PublishObserved {
                manifest: manifest.id,
                observed_main: manifest.integrated_main + 1,
                succeeded: true,
                outputs_match: true,
            },
        );
        assert_eq!(moved.decision, SimpleDecision::Rejected);
        assert_eq!(moved.state, state);

        let rejected = replay_simple_land(
            &SimpleLandState::default(),
            &[
                SimpleLandEvent::SubmitManifest {
                    manifest: manifest.clone(),
                },
                SimpleLandEvent::RecordFlip {
                    manifest: manifest.id,
                    requirements: manifest.requirements,
                    verdict: ReviewVerdict::Reject,
                },
            ],
        )
        .0;
        let publish = reduce_simple_land(
            &rejected,
            &SimpleLandEvent::PublishObserved {
                manifest: manifest.id,
                observed_main: manifest.integrated_main,
                succeeded: true,
                outputs_match: true,
            },
        );
        assert_eq!(publish.decision, SimpleDecision::Rejected);
        assert_eq!(publish.state, rejected);
    }

    #[test]
    fn changed_manifest_invalidates_both_receipts() {
        let state = accepted(CompletionContract::Report);
        let mut changed = state.manifest.clone().unwrap();
        changed.id += 1;
        changed.output_digest += 1;
        let result = reduce_simple_land(
            &state,
            &SimpleLandEvent::SubmitManifest { manifest: changed },
        );
        assert_eq!(result.state.flip.verdict, ReviewVerdict::Absent);
        assert_eq!(result.state.eval.verdict, ReviewVerdict::Absent);
        assert_eq!(result.state.publication, None);
    }

    #[test]
    fn failure_after_publication_cannot_revoke_success_path() {
        let state = accepted(CompletionContract::Explore);
        let manifest = state.manifest.clone().unwrap();
        let published = reduce_simple_land(
            &state,
            &SimpleLandEvent::PublishObserved {
                manifest: manifest.id,
                observed_main: 0,
                succeeded: true,
                outputs_match: true,
            },
        )
        .state;
        let late_failure = reduce_simple_land(&published, &SimpleLandEvent::Fail { code: 9 });
        assert_eq!(late_failure.decision, SimpleDecision::Noop);
        assert_eq!(late_failure.state, published);
    }
}
