//! Versioned, content-addressed evidence for atomic task completion.
//!
//! This module is deliberately pure.  It describes and verifies facts supplied
//! by adapters; it does not claim that a process is dead, bytes were fsynced,
//! a Git CAS happened, or a worktree was removed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{CompletionContract, CompletionDisposition};

pub const COMPLETION_SCHEMA_VERSION: u32 = 2;
pub const COMPLETION_PROTOCOL_MAJOR: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceHeader {
    pub schema_version: u32,
    pub protocol_major: u32,
    pub producer_build_id: String,
}

impl EvidenceHeader {
    pub fn v2(producer_build_id: impl Into<String>) -> Self {
        Self {
            schema_version: COMPLETION_SCHEMA_VERSION,
            protocol_major: COMPLETION_PROTOCOL_MAJOR,
            producer_build_id: producer_build_id.into(),
        }
    }

    fn verify(&self) -> Result<(), EvidenceError> {
        if self.schema_version != COMPLETION_SCHEMA_VERSION
            || self.protocol_major != COMPLETION_PROTOCOL_MAJOR
        {
            return Err(EvidenceError::new(
                "unsupported-protocol",
                format!(
                    "completion evidence requires schema {} / protocol {}, found {} / {}",
                    COMPLETION_SCHEMA_VERSION,
                    COMPLETION_PROTOCOL_MAJOR,
                    self.schema_version,
                    self.protocol_major
                ),
            ));
        }
        require("producer-build-id", &self.producer_build_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttemptSaveKey {
    pub graph_id: String,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub worktree_lease_epoch: u64,
    pub process_epoch: u32,
    pub wrapper_epoch: u32,
    pub route_snapshot_cid: String,
    pub session_proof_digest: String,
    pub worktree_identity_digest: String,
}

impl AttemptSaveKey {
    pub fn transaction_id(&self) -> Result<String, EvidenceError> {
        content_cid(self)
    }

    fn verify(&self) -> Result<(), EvidenceError> {
        for (name, value) in [
            ("graph-id", self.graph_id.as_str()),
            ("task-id", self.task_id.as_str()),
            ("attempt-id", self.attempt_id.as_str()),
            ("route-snapshot", self.route_snapshot_cid.as_str()),
            ("session-proof", self.session_proof_digest.as_str()),
            ("worktree-identity", self.worktree_identity_digest.as_str()),
        ] {
            require(name, value)?;
        }
        Ok(())
    }
}

/// The invariant key repeated by all post-capture evidence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EvidenceBinding {
    pub source: AttemptSaveKey,
    pub candidate_id: String,
    pub base_commit_oid: String,
}

impl EvidenceBinding {
    fn verify(&self) -> Result<(), EvidenceError> {
        self.source.verify()?;
        require("candidate-id", &self.candidate_id)?;
        require("base-commit", &self.base_commit_oid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionIntentReceipt {
    pub header: EvidenceHeader,
    pub source: AttemptSaveKey,
    pub contract: CompletionContract,
    pub terminal_reservation_cid: String,
    pub capture_policy_cid: String,
    pub validation_policy_cid: String,
    pub flip_policy_cid: String,
    pub smoke_policy_cid: String,
    pub deliverable_policy_cid: String,
    pub expected_target_ref: Option<String>,
    pub prepared_base_commit_oid: String,
    pub client_idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSaveReceipt {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub completion_intent_cid: String,
    pub quiescence_receipt_cid: String,
    pub worktree_root_identity: String,
    pub branch: Option<String>,
    pub worker_head_oid: String,
    pub prepared_base_commit_oid: String,
    pub clean: bool,
    pub rescue_commit_oid: String,
    pub saved_tree_oid: String,
    pub full_manifest_cid: String,
    pub delta_manifest_cid: String,
    pub immutable_ref: String,
    pub excluded_path_policy_cid: String,
    pub observer_manifest_digest: String,
    pub observer_sequence: u64,
    pub late_mutation_quarantine_cid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDescriptor {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub work_save_cid: String,
    pub candidate_version: u64,
    pub candidate_commit_oid: String,
    pub candidate_tree_oid: String,
    pub full_manifest_cid: String,
    pub delta_manifest_cid: String,
    pub inclusion_policy_cid: String,
    pub immutable_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceOutcome {
    Accepted,
    Rejected,
    Insufficient,
    Unavailable,
    NotRequired,
}

impl AcceptanceOutcome {
    fn is_accepting(self) -> bool {
        matches!(self, Self::Accepted | Self::NotRequired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReceipt {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub candidate_cid: String,
    pub policy_cid: String,
    pub outcome: AcceptanceOutcome,
    pub validator_identity: String,
}

/// An explicit FLIP result. `NotRequired` is evidence, never an absent slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlipReceipt {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub candidate_cid: String,
    pub policy_cid: String,
    pub route_snapshot_cid: String,
    pub outcome: AcceptanceOutcome,
    pub evaluator_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispositionReceipt {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub completion_intent_cid: String,
    pub candidate_cid: String,
    pub contract: CompletionContract,
    pub disposition: CompletionDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionReceipt {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub disposition_cid: String,
    pub action_key: String,
    pub target_ref: String,
    pub expected_old_commit_oid: String,
    pub observed_old_commit_oid: String,
    pub integration_commit_oid: String,
    pub result_tree_oid: String,
    pub result_manifest_cid: String,
    pub ref_cas_succeeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputReceipt {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub disposition_cid: String,
    pub action_key: String,
    pub immutable_output_ref: String,
    pub output_manifest_cid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EffectReceipt {
    Promotion(PromotionReceipt),
    Output(OutputReceipt),
}

impl EffectReceipt {
    fn binding(&self) -> &EvidenceBinding {
        match self {
            Self::Promotion(value) => &value.binding,
            Self::Output(value) => &value.binding,
        }
    }

    fn disposition_cid(&self) -> &str {
        match self {
            Self::Promotion(value) => &value.disposition_cid,
            Self::Output(value) => &value.disposition_cid,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupCommit {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub work_save_cid: String,
    pub effect_receipt_cid: String,
    pub cleanup_plan_cid: String,
    pub worktree_root_identity: String,
    pub worktree_lease_epoch: u64,
    pub result: CleanupResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupResult {
    Removed,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceCidSet {
    pub completion_intent: String,
    pub work_save: String,
    pub candidate: String,
    pub validation: String,
    pub flip: String,
    pub disposition: String,
    pub effect: String,
    pub cleanup: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSaveReceipt {
    pub header: EvidenceHeader,
    pub binding: EvidenceBinding,
    pub contract: CompletionContract,
    pub disposition: CompletionDisposition,
    pub evidence: EvidenceCidSet,
    pub bundle_digest: String,
    pub graph_revision_before_commit: u64,
    pub lifecycle_event_id: String,
}

/// Self-contained input to the pure verifier. Durable stores may keep each
/// member separately and materialize this value by CID before reduction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSaveBundle {
    pub receipt: GraphSaveReceipt,
    pub completion_intent: CompletionIntentReceipt,
    pub work_save: WorkSaveReceipt,
    pub candidate: CandidateDescriptor,
    pub validation: ValidationReceipt,
    pub flip: FlipReceipt,
    pub disposition: DispositionReceipt,
    pub effect: EffectReceipt,
    pub cleanup: CleanupCommit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedGraphSave {
    pub graph_save_cid: String,
    pub binding: EvidenceBinding,
    pub contract: CompletionContract,
    pub disposition: CompletionDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceError {
    pub code: String,
    pub message: String,
}

impl EvidenceError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for EvidenceError {}

pub fn content_cid<T: Serialize>(value: &T) -> Result<String, EvidenceError> {
    let value = serde_json::to_value(value)
        .map_err(|error| EvidenceError::new("serialization", error.to_string()))?;
    Ok(format!(
        "wgcid:v2:blake3:{}",
        blake3::hash(&canonical_json(&value)).to_hex()
    ))
}

fn canonical_json(value: &Value) -> Vec<u8> {
    crate::identity::canonical_json(value)
}

fn require(name: &str, value: &str) -> Result<(), EvidenceError> {
    if value.trim().is_empty() {
        Err(EvidenceError::new(
            "missing-evidence",
            format!("{name} is empty"),
        ))
    } else {
        Ok(())
    }
}

fn same_binding(
    expected: &EvidenceBinding,
    observed: &EvidenceBinding,
    kind: &str,
) -> Result<(), EvidenceError> {
    if expected == observed {
        Ok(())
    } else {
        Err(EvidenceError::new(
            "binding-mismatch",
            format!("{kind} does not agree on the exact attempt/candidate/base binding"),
        ))
    }
}

/// Verify the complete GS/WS invariant without performing I/O.
pub fn verify_graph_save_bundle(
    bundle: &GraphSaveBundle,
) -> Result<VerifiedGraphSave, EvidenceError> {
    let graph = &bundle.receipt;
    graph.header.verify()?;
    graph.binding.verify()?;
    require("lifecycle-event-id", &graph.lifecycle_event_id)?;

    for header in [
        &bundle.completion_intent.header,
        &bundle.work_save.header,
        &bundle.candidate.header,
        &bundle.validation.header,
        &bundle.flip.header,
        &bundle.disposition.header,
        match &bundle.effect {
            EffectReceipt::Promotion(v) => &v.header,
            EffectReceipt::Output(v) => &v.header,
        },
        &bundle.cleanup.header,
    ] {
        header.verify()?;
    }

    if bundle.completion_intent.source != graph.binding.source
        || bundle.completion_intent.prepared_base_commit_oid != graph.binding.base_commit_oid
    {
        return Err(EvidenceError::new(
            "binding-mismatch",
            "completion intent does not bind the graph save source/base",
        ));
    }
    for (kind, binding) in [
        ("work-save", &bundle.work_save.binding),
        ("candidate", &bundle.candidate.binding),
        ("validation", &bundle.validation.binding),
        ("flip", &bundle.flip.binding),
        ("disposition", &bundle.disposition.binding),
        ("effect", bundle.effect.binding()),
        ("cleanup", &bundle.cleanup.binding),
    ] {
        same_binding(&graph.binding, binding, kind)?;
    }

    let observed = EvidenceCidSet {
        completion_intent: content_cid(&bundle.completion_intent)?,
        work_save: content_cid(&bundle.work_save)?,
        candidate: content_cid(&bundle.candidate)?,
        validation: content_cid(&bundle.validation)?,
        flip: content_cid(&bundle.flip)?,
        disposition: content_cid(&bundle.disposition)?,
        effect: content_cid(&bundle.effect)?,
        cleanup: content_cid(&bundle.cleanup)?,
    };
    if graph.evidence != observed {
        return Err(EvidenceError::new(
            "cid-mismatch",
            "GraphSave evidence CID list does not match the supplied immutable objects",
        ));
    }
    if graph.bundle_digest != content_cid(&observed)? {
        return Err(EvidenceError::new(
            "bundle-digest-mismatch",
            "GraphSave bundle digest is not canonical",
        ));
    }

    if bundle.work_save.completion_intent_cid != observed.completion_intent
        || bundle.candidate.work_save_cid != observed.work_save
        || bundle.validation.candidate_cid != observed.candidate
        || bundle.flip.candidate_cid != observed.candidate
        || bundle.disposition.completion_intent_cid != observed.completion_intent
        || bundle.disposition.candidate_cid != observed.candidate
        || bundle.effect.disposition_cid() != observed.disposition
        || bundle.cleanup.work_save_cid != observed.work_save
        || bundle.cleanup.effect_receipt_cid != observed.effect
    {
        return Err(EvidenceError::new(
            "reference-mismatch",
            "an evidence object cites a different predecessor",
        ));
    }

    if bundle.work_save.prepared_base_commit_oid != graph.binding.base_commit_oid
        || bundle.work_save.binding.candidate_id != bundle.candidate.binding.candidate_id
        || bundle.candidate.candidate_version == 0
    {
        return Err(EvidenceError::new(
            "candidate-mismatch",
            "candidate is not the deterministic projection of this WorkSave",
        ));
    }

    if bundle.validation.policy_cid != bundle.completion_intent.validation_policy_cid
        || bundle.flip.policy_cid != bundle.completion_intent.flip_policy_cid
        || bundle.flip.route_snapshot_cid != graph.binding.source.route_snapshot_cid
        || !bundle.validation.outcome.is_accepting()
        || !bundle.flip.outcome.is_accepting()
    {
        return Err(EvidenceError::new(
            "acceptance-incomplete",
            "required validation/FLIP evidence is absent, stale, or non-accepting",
        ));
    }

    if bundle.completion_intent.contract != graph.contract
        || bundle.disposition.contract != graph.contract
        || bundle.disposition.disposition != graph.disposition
        || !graph.disposition.satisfies(graph.contract)
    {
        return Err(EvidenceError::new(
            "disposition-mismatch",
            "completion intent, disposition, and GraphSave contract disagree",
        ));
    }

    match (&bundle.effect, graph.disposition) {
        (EffectReceipt::Promotion(value), CompletionDisposition::Landed)
            if value.ref_cas_succeeded
                && value.expected_old_commit_oid == value.observed_old_commit_oid => {}
        (
            EffectReceipt::Output(value),
            CompletionDisposition::Delivered | CompletionDisposition::Reported,
        ) if !value.immutable_output_ref.trim().is_empty() => {}
        _ => {
            return Err(EvidenceError::new(
                "effect-mismatch",
                "effect is not the exact successful effect required by the disposition",
            ));
        }
    }
    if bundle.cleanup.worktree_lease_epoch != graph.binding.source.worktree_lease_epoch
        || bundle.cleanup.worktree_root_identity != bundle.work_save.worktree_root_identity
        || bundle.cleanup.cleanup_plan_cid.trim().is_empty()
    {
        return Err(EvidenceError::new(
            "cleanup-mismatch",
            "cleanup does not bind the saved worktree and durable plan",
        ));
    }

    Ok(VerifiedGraphSave {
        graph_save_cid: content_cid(graph)?,
        binding: graph.binding.clone(),
        contract: graph.contract,
        disposition: graph.disposition,
    })
}
