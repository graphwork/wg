//! Evidence test for `prove-the-recoverable`: run the production
//! `classify_semantic_rejection` over the real findings from the live
//! `implement-flip-fidelity` Eval semantic rejection (shared graph,
//! 2026-09-20), plus the classification boundaries, and print the result.
//!
//! This is evidence only; it does not change production behavior.

use worksgood::completion_review::ReviewFinding;
use worksgood::completion_validation::{SemanticRejectionClass, classify_semantic_rejection};

fn reserved(code: &str, message: &str, evidence: &str) -> ReviewFinding {
    ReviewFinding {
        code: code.to_string(),
        message: message.to_string(),
        evidence: Some(evidence.to_string()),
    }
}

#[test]
fn real_live_rejection_classification_evidence() {
    let real = vec![
        reserved(
            "completion.missing_authoritative_runtime_evidence",
            "Acceptance validation requires `cargo test --lib` to be green, but the only host-captured authoritative envelope is the baseline `git diff --check`.",
            "cargo test --lib",
        ),
        reserved(
            "completion.missing_authoritative_runtime_evidence",
            "Acceptance validation requires `cargo fmt --check` to be clean; no host-captured envelope records this run.",
            "cargo fmt --check",
        ),
        reserved(
            "completion.missing_authoritative_runtime_evidence",
            "Acceptance validation requires `cargo clippy` to be clean; no host-captured envelope records this run.",
            "cargo clippy --all-targets --all-features -- -D warnings",
        ),
    ];
    let real_class = classify_semantic_rejection(&real);
    println!("REAL_LIVE_MULTI_CHECK_REJECTION => {real_class:?}");

    let one_command = vec![
        reserved("completion.missing_authoritative_runtime_evidence", "missing", "cargo test --lib"),
        reserved("completion.missing_authoritative_runtime_evidence", "missing", "cargo test --lib"),
    ];
    println!("SINGLE_EXACT_COMMAND_EVIDENCE_GAP => {:?}", classify_semantic_rejection(&one_command));

    println!(
        "SUBSTANTIVE_GAP => {:?}",
        classify_semantic_rejection(&[ReviewFinding::new("eval.substantive-gap", "missing section")])
    );
    println!(
        "DEFECT => {:?}",
        classify_semantic_rejection(&[ReviewFinding::new("eval.incomplete-implementation", "x")])
    );
    println!(
        "MIXED => {:?}",
        classify_semantic_rejection(&[
            reserved("completion.missing_authoritative_runtime_evidence", "missing", "cargo test"),
            ReviewFinding::new("eval.substantive-gap", "missing section"),
        ])
    );

    // Pin the finding that matters for the live proof: the observed real
    // multi-check evidence-gap rejection is NOT the single-command EvidenceGap
    // class; the production classifier returns Recoverable for it.
    assert_eq!(real_class, SemanticRejectionClass::Recoverable);
    assert_eq!(
        classify_semantic_rejection(&one_command),
        SemanticRejectionClass::EvidenceGap
    );
}
