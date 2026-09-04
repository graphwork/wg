//! Append-only adaptive review and learning evidence.
//!
//! This module is intentionally an observation package.  Its public writers
//! accept immutable values and append create-once objects below
//! `agency/adaptive/v1`; they never receive a `WorkGraph`, `Task`, lifecycle
//! request, dispatcher, publication handle, or command runner.  The completion
//! controller remains the only component allowed to consume review evidence
//! into a source lifecycle transition.

use crate::completion_review::{ReviewFinding, ReviewUsage, ReviewerKind};
use crate::identity::canonical_json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

pub const ADAPTIVE_SCHEMA_VERSION: u16 = 1;
pub const ADAPTIVE_POLICY_VERSION: &str = "adaptive-v1";
const ROOT: &str = "agency/adaptive/v1";

#[derive(Debug, Error)]
pub enum AdaptiveError {
    #[error("adaptive agency I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("adaptive agency JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("adaptive agency identity collision/corruption: {0}")]
    Collision(String),
    #[error("adaptive agency binding refused: {0}")]
    Binding(String),
    #[error("adaptive agency lock timed out: {0}")]
    Lock(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceBindingV1 {
    pub graph_identity: String,
    pub task_id: String,
    pub generation: u64,
    pub source_attempt_id: String,
    pub source_fence: u64,
    pub assignment_receipt_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CandidateBindingV1 {
    pub source: SourceBindingV1,
    pub candidate_sequence: u64,
    pub manifest_digest: String,
    pub requirements_digest: String,
    pub source_revision: String,
    pub dependency_revision_digest: String,
    pub output_digests: Vec<String>,
    pub validation_evidence_digest: String,
}

impl CandidateBindingV1 {
    pub fn normalized(mut self) -> Self {
        self.output_digests.sort();
        self.output_digests.dedup();
        self
    }

    pub fn digest(&self) -> Result<String, AdaptiveError> {
        identity("wg-candidate-binding-v1", &self.clone().normalized())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PolicySnapshot {
    pub policy_id: String,
    pub policy_digest: String,
    pub strict: bool,
    pub max_infrastructure_attempts: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouteSnapshot {
    pub handler: String,
    pub provider: String,
    pub model: String,
    pub exact_route: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub adapter: String,
    pub adapter_version: String,
    pub route_generation: u32,
    pub route_digest: String,
}

impl RouteSnapshot {
    pub fn exact(
        exact_route: impl Into<String>,
        reasoning: Option<String>,
        adapter: impl Into<String>,
        adapter_version: impl Into<String>,
        route_generation: u32,
    ) -> Result<Self, AdaptiveError> {
        let exact_route = exact_route.into();
        let mut parts = exact_route.split(':');
        let handler = parts.next().unwrap_or("unknown").to_string();
        let provider = parts.next().unwrap_or("unknown").to_string();
        let model = parts.collect::<Vec<_>>().join(":");
        let adapter = adapter.into();
        let adapter_version = adapter_version.into();
        let route_digest = identity(
            "wg-review-route-v1",
            &(
                &handler,
                &provider,
                &model,
                &exact_route,
                &reasoning,
                &adapter,
                &adapter_version,
                route_generation,
            ),
        )?;
        Ok(Self {
            handler,
            provider,
            model,
            exact_route,
            reasoning,
            adapter,
            adapter_version,
            route_generation,
            route_digest,
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageV1 {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    /// `None` means provider usage/cost was unavailable.  It is never treated
    /// as a reported zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cost: Option<f64>,
    pub currency: String,
    pub source: String,
}

impl From<&ReviewUsage> for UsageV1 {
    fn from(value: &ReviewUsage) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cache_read_tokens: value.cache_read_input_tokens,
            cache_write_tokens: value.cache_creation_input_tokens,
            total_tokens: value
                .input_tokens
                .saturating_add(value.output_tokens)
                .saturating_add(value.cache_read_input_tokens)
                .saturating_add(value.cache_creation_input_tokens),
            provider_cost: Some(value.cost_usd),
            currency: "USD".to_string(),
            source: "provider-reported".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewProduct {
    Completion,
    Bounded,
    DeepReadonly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticOutcome {
    Pass,
    Reject,
    Inconclusive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InfrastructureOutcome {
    Timeout,
    AdapterUnavailable,
    ProcessFailed,
    MalformedOutput,
    RouteDrift,
    EvidenceUnavailable,
    InsufficientEvidence,
    BudgetExceeded,
    InterruptedUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "class", content = "outcome", rename_all = "snake_case")]
pub enum ReviewOutcomeV1 {
    Semantic(SemanticOutcome),
    Infrastructure(InfrastructureOutcome),
}

impl ReviewOutcomeV1 {
    pub fn is_semantic(&self) -> bool {
        matches!(self, Self::Semantic(_))
    }

    pub fn is_infrastructure(&self) -> bool {
        matches!(self, Self::Infrastructure(_))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "at", rename_all = "snake_case")]
pub enum TimeEvidence {
    Observed(String),
    UnknownLegacy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumptionEffect {
    AcceptedEvidence,
    RejectedEvidence,
    AdvisoryOnly,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "event_kind", rename_all = "snake_case")]
pub enum CandidateLedgerEventV1 {
    CandidateSelected {
        schema: u16,
        event_id: String,
        binding: CandidateBindingV1,
        selected_at: String,
    },
    ReviewAttemptStarted {
        schema: u16,
        event_id: String,
        review_run_id: String,
        review_attempt_id: String,
        ordinal: u32,
        reviewer_kind: ReviewerKind,
        product: ReviewProduct,
        binding: CandidateBindingV1,
        policy: PolicySnapshot,
        route: RouteSnapshot,
        capability_manifest_digest: String,
        started_at: TimeEvidence,
        lease_expires_at: Option<String>,
        supersedes_attempt: Option<String>,
    },
    ReviewAttemptFinished {
        schema: u16,
        event_id: String,
        started_event_id: String,
        review_run_id: String,
        review_attempt_id: String,
        binding: CandidateBindingV1,
        policy_digest: String,
        route_digest: String,
        capability_manifest_digest: String,
        outcome: ReviewOutcomeV1,
        completed_at: String,
        duration_ms: u64,
        response_digest: Option<String>,
        findings_digest: Option<String>,
        inspected_output_digests: Vec<String>,
        usage: Option<UsageV1>,
        stop_reason: Option<String>,
        provider_reported_route: Option<String>,
        receipt_digest: String,
    },
    CandidateSuperseded {
        schema: u16,
        event_id: String,
        binding_digest: String,
        superseded_by_binding_digest: String,
        superseded_at: String,
        reason: String,
    },
    ReviewConsumed {
        schema: u16,
        event_id: String,
        review_attempt_id: String,
        binding_digest: String,
        receipt_digest: String,
        controller_policy_digest: String,
        source_fence: u64,
        consumed_at: String,
        effect: ConsumptionEffect,
    },
}

impl CandidateLedgerEventV1 {
    pub fn event_id(&self) -> &str {
        match self {
            Self::CandidateSelected { event_id, .. }
            | Self::ReviewAttemptStarted { event_id, .. }
            | Self::ReviewAttemptFinished { event_id, .. }
            | Self::CandidateSuperseded { event_id, .. }
            | Self::ReviewConsumed { event_id, .. } => event_id,
        }
    }

    pub fn binding(&self) -> Option<&CandidateBindingV1> {
        match self {
            Self::CandidateSelected { binding, .. }
            | Self::ReviewAttemptStarted { binding, .. }
            | Self::ReviewAttemptFinished { binding, .. } => Some(binding),
            Self::CandidateSuperseded { .. } | Self::ReviewConsumed { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerAuthor {
    CandidateSelection,
    ReviewAttempt,
    CompletionController,
    LegacyImport,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct EventEnvelope {
    author: LedgerAuthor,
    event: CandidateLedgerEventV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewAttemptHandle {
    pub started_event_id: String,
    pub review_run_id: String,
    pub review_attempt_id: String,
    pub ordinal: u32,
    pub binding_digest: String,
}

#[derive(Clone, Debug)]
pub struct ReviewFinishInput {
    pub outcome: ReviewOutcomeV1,
    pub completed_at: String,
    pub duration_ms: u64,
    pub response_digest: Option<String>,
    pub findings_digest: Option<String>,
    pub inspected_output_digests: Vec<String>,
    pub usage: Option<UsageV1>,
    pub stop_reason: Option<String>,
    pub provider_reported_route: Option<String>,
    pub receipt_digest: String,
}

#[derive(Clone, Debug)]
pub struct CandidateSelectionSink {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ReviewAttemptSink {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ReviewConsumptionSink {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct LearningProjector {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct AdaptiveReadStore {
    root: PathBuf,
}

/// Composition root.  It yields disjoint sinks instead of a generic event
/// writer so a reviewer cannot manufacture candidate selection or consumption.
#[derive(Clone, Debug)]
pub struct AdaptiveStore {
    root: PathBuf,
}

impl AdaptiveStore {
    pub fn open(workgraph_dir: &Path) -> Result<Self, AdaptiveError> {
        let root = workgraph_dir.join(ROOT);
        fs::create_dir_all(root.join("candidate-ledger/events"))?;
        fs::create_dir_all(root.join("candidate-ledger/locks"))?;
        fs::create_dir_all(root.join("assignment-receipts"))?;
        fs::create_dir_all(root.join("trajectory-seals"))?;
        fs::create_dir_all(root.join("terminal-episodes"))?;
        fs::create_dir_all(root.join("outcome-assessments"))?;
        fs::create_dir_all(root.join("performance-projections"))?;
        Ok(Self { root })
    }

    /// Open only when the adaptive store already exists. Read-only status,
    /// list, spend, and inspect paths use this constructor so observation does
    /// not create control-plane files.
    pub fn open_existing(workgraph_dir: &Path) -> Option<Self> {
        let root = workgraph_dir.join(ROOT);
        root.is_dir().then_some(Self { root })
    }

    pub fn selection_sink(&self) -> CandidateSelectionSink {
        CandidateSelectionSink {
            root: self.root.clone(),
        }
    }

    pub fn review_sink(&self) -> ReviewAttemptSink {
        ReviewAttemptSink {
            root: self.root.clone(),
        }
    }

    /// This constructor is deliberately explicit: callers should hand this
    /// sink only to the completion-controller composition root.
    pub fn completion_consumption_sink(&self) -> ReviewConsumptionSink {
        ReviewConsumptionSink {
            root: self.root.clone(),
        }
    }

    pub fn learning_projector(&self) -> LearningProjector {
        LearningProjector {
            root: self.root.clone(),
        }
    }

    pub fn reader(&self) -> AdaptiveReadStore {
        AdaptiveReadStore {
            root: self.root.clone(),
        }
    }

    pub fn ensure_uncomposed_assignment(
        &self,
        graph_identity: &str,
        task_id: &str,
        generation: u64,
        attempt_id: &str,
        attempt_fence: u64,
        reason: &str,
    ) -> Result<AssignmentReceiptV1, AdaptiveError> {
        let material = (
            graph_identity,
            task_id,
            generation,
            attempt_id,
            attempt_fence,
            "compatibility-uncomposed",
        );
        let receipt_id = identity("wg-assignment-receipt-v1", &material)?;
        let receipt = AssignmentReceiptV1 {
            schema: ADAPTIVE_SCHEMA_VERSION,
            receipt_id: receipt_id.clone(),
            graph_identity: graph_identity.to_string(),
            task_id: task_id.to_string(),
            generation,
            attempt_id: attempt_id.to_string(),
            attempt_fence,
            decision: AssignmentDecisionV1::Uncomposed {
                reason: reason.to_string(),
            },
            created_at: "unknown-legacy".to_string(),
        };
        write_create_once(
            &self
                .root
                .join("assignment-receipts")
                .join(file_id(&receipt_id)),
            &receipt,
            |existing| existing.receipt_id == receipt_id,
        )?;
        Ok(receipt)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AssignmentReceiptV1 {
    pub schema: u16,
    pub receipt_id: String,
    pub graph_identity: String,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub decision: AssignmentDecisionV1,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssignmentDecisionV1 {
    Explicit { composition_digest: String },
    Automatic { composition_digest: String },
    Uncomposed { reason: String },
}

impl CandidateSelectionSink {
    pub fn select(
        &self,
        binding: CandidateBindingV1,
        selected_at: impl Into<String>,
    ) -> Result<String, AdaptiveError> {
        let binding = binding.normalized();
        let binding_digest = binding.digest()?;
        let _lock = DirectoryLock::acquire(&self.root.join("candidate-ledger/locks/select.lock"))?;
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        let current = reader.current_candidate_for_source(&binding.source)?;
        if let Some(current) = current {
            let current_digest = current.digest()?;
            if current_digest == binding_digest {
                return identity("wg-candidate-selected-v1", &binding_digest);
            }
            let superseded_id = identity(
                "wg-candidate-superseded-v1",
                &(current_digest.clone(), binding_digest.clone()),
            )?;
            let event = CandidateLedgerEventV1::CandidateSuperseded {
                schema: ADAPTIVE_SCHEMA_VERSION,
                event_id: superseded_id.clone(),
                binding_digest: current_digest,
                superseded_by_binding_digest: binding_digest.clone(),
                superseded_at: selected_at.into(),
                reason: "new immutable candidate selected".to_string(),
            };
            write_event(&self.root, LedgerAuthor::CandidateSelection, event)?;
            // Use the persisted supersession timestamp for selection too.
            let selected_at = reader
                .event(&superseded_id)?
                .and_then(|event| match event {
                    CandidateLedgerEventV1::CandidateSuperseded { superseded_at, .. } => {
                        Some(superseded_at)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| Utc::now().to_rfc3339());
            return self.write_selected(binding, binding_digest, selected_at);
        }
        self.write_selected(binding, binding_digest, selected_at.into())
    }

    fn write_selected(
        &self,
        binding: CandidateBindingV1,
        binding_digest: String,
        selected_at: String,
    ) -> Result<String, AdaptiveError> {
        let event_id = identity("wg-candidate-selected-v1", &binding_digest)?;
        let event = CandidateLedgerEventV1::CandidateSelected {
            schema: ADAPTIVE_SCHEMA_VERSION,
            event_id: event_id.clone(),
            binding,
            selected_at,
        };
        write_event(&self.root, LedgerAuthor::CandidateSelection, event)?;
        Ok(event_id)
    }
}

impl ReviewAttemptSink {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        binding: CandidateBindingV1,
        reviewer_kind: ReviewerKind,
        product: ReviewProduct,
        policy: PolicySnapshot,
        route: RouteSnapshot,
        capability_manifest_digest: String,
        started_at: String,
        lease_expires_at: String,
    ) -> Result<ReviewAttemptHandle, AdaptiveError> {
        if DateTime::parse_from_rfc3339(&started_at).is_err()
            || DateTime::parse_from_rfc3339(&lease_expires_at).is_err()
        {
            return Err(AdaptiveError::Binding(
                "live review attempts require observed RFC3339 start and lease".to_string(),
            ));
        }
        let binding = binding.normalized();
        let binding_digest = binding.digest()?;
        let review_run_id = identity(
            "wg-review-run-v1",
            &(
                &binding_digest,
                reviewer_kind,
                &product,
                &policy.policy_digest,
                route.route_generation,
                &route.route_digest,
            ),
        )?;
        let lock_path = self
            .root
            .join("candidate-ledger/locks")
            .join(format!("{}.lock", safe_hash(&review_run_id)));
        let _lock = DirectoryLock::acquire(&lock_path)?;
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        let attempts = reader.attempts_for_run(&review_run_id)?;
        let ordinal = attempts
            .iter()
            .map(|attempt| attempt.ordinal)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let supersedes_attempt = attempts.last().map(|value| value.review_attempt_id.clone());
        let review_attempt_id = identity("wg-review-attempt-v1", &(&review_run_id, ordinal))?;
        let event_id = identity("wg-review-attempt-started-v1", &review_attempt_id)?;
        let event = CandidateLedgerEventV1::ReviewAttemptStarted {
            schema: ADAPTIVE_SCHEMA_VERSION,
            event_id: event_id.clone(),
            review_run_id: review_run_id.clone(),
            review_attempt_id: review_attempt_id.clone(),
            ordinal,
            reviewer_kind,
            product,
            binding,
            policy,
            route,
            capability_manifest_digest,
            started_at: TimeEvidence::Observed(started_at),
            lease_expires_at: Some(lease_expires_at),
            supersedes_attempt,
        };
        write_event(&self.root, LedgerAuthor::ReviewAttempt, event)?;
        Ok(ReviewAttemptHandle {
            started_event_id: event_id,
            review_run_id,
            review_attempt_id,
            ordinal,
            binding_digest,
        })
    }

    pub fn finish(
        &self,
        handle: &ReviewAttemptHandle,
        mut input: ReviewFinishInput,
    ) -> Result<String, AdaptiveError> {
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        let started = reader
            .event(&handle.started_event_id)?
            .ok_or_else(|| AdaptiveError::Binding("review start is missing".to_string()))?;
        let CandidateLedgerEventV1::ReviewAttemptStarted {
            review_run_id,
            review_attempt_id,
            binding,
            policy,
            route,
            capability_manifest_digest,
            ..
        } = started
        else {
            return Err(AdaptiveError::Binding(
                "handle does not identify a review start".to_string(),
            ));
        };
        if review_run_id != handle.review_run_id
            || review_attempt_id != handle.review_attempt_id
            || binding.digest()? != handle.binding_digest
        {
            return Err(AdaptiveError::Binding(
                "review finish does not match its immutable start".to_string(),
            ));
        }
        input.inspected_output_digests.sort();
        input.inspected_output_digests.dedup();
        let event_id = identity("wg-review-attempt-finished-v1", &review_attempt_id)?;
        let event = CandidateLedgerEventV1::ReviewAttemptFinished {
            schema: ADAPTIVE_SCHEMA_VERSION,
            event_id: event_id.clone(),
            started_event_id: handle.started_event_id.clone(),
            review_run_id,
            review_attempt_id,
            binding,
            policy_digest: policy.policy_digest,
            route_digest: route.route_digest,
            capability_manifest_digest,
            outcome: input.outcome,
            completed_at: input.completed_at,
            duration_ms: input.duration_ms,
            response_digest: input.response_digest,
            findings_digest: input.findings_digest,
            inspected_output_digests: input.inspected_output_digests,
            usage: input.usage,
            stop_reason: input.stop_reason,
            provider_reported_route: input.provider_reported_route,
            receipt_digest: input.receipt_digest,
        };
        write_event(&self.root, LedgerAuthor::ReviewAttempt, event)?;
        Ok(event_id)
    }

    /// Settle every expired, un-finished start as interrupted-unknown.  This
    /// never invents semantic evidence.  A subsequent retry obtains the next
    /// ordinal and repeats the persisted route.
    pub fn settle_expired(&self, now: &str) -> Result<Vec<String>, AdaptiveError> {
        let now = DateTime::parse_from_rfc3339(now)
            .map_err(|_| AdaptiveError::Binding("invalid recovery time".to_string()))?;
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        let events = reader.events()?;
        let finished = events
            .iter()
            .filter_map(|event| match event {
                CandidateLedgerEventV1::ReviewAttemptFinished {
                    review_attempt_id, ..
                } => Some(review_attempt_id.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut settled = Vec::new();
        for event in events {
            let CandidateLedgerEventV1::ReviewAttemptStarted {
                event_id,
                review_run_id,
                review_attempt_id,
                ordinal,
                binding,
                lease_expires_at: Some(expires),
                ..
            } = event
            else {
                continue;
            };
            if finished.contains(&review_attempt_id) {
                continue;
            }
            let Ok(expires) = DateTime::parse_from_rfc3339(&expires) else {
                continue;
            };
            if expires > now {
                continue;
            }
            let handle = ReviewAttemptHandle {
                started_event_id: event_id,
                review_run_id,
                review_attempt_id,
                ordinal,
                binding_digest: binding.digest()?,
            };
            settled.push(self.finish(
                &handle,
                ReviewFinishInput {
                    outcome: ReviewOutcomeV1::Infrastructure(
                        InfrastructureOutcome::InterruptedUnknown,
                    ),
                    completed_at: now.to_rfc3339(),
                    duration_ms: 0,
                    response_digest: None,
                    findings_digest: None,
                    inspected_output_digests: Vec::new(),
                    usage: None,
                    stop_reason: Some("lease expired without durable receipt".to_string()),
                    provider_reported_route: None,
                    receipt_digest: identity(
                        "wg-review-interrupted-unknown-v1",
                        &handle.review_attempt_id,
                    )?,
                },
            )?);
        }
        Ok(settled)
    }
}

impl ReviewConsumptionSink {
    #[allow(clippy::too_many_arguments)]
    pub fn consume(
        &self,
        review_attempt_id: &str,
        binding: &CandidateBindingV1,
        receipt_digest: &str,
        controller_policy_digest: &str,
        source_fence: u64,
        effect: ConsumptionEffect,
        consumed_at: &str,
    ) -> Result<String, AdaptiveError> {
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        let binding_digest = binding.digest()?;
        let current = reader
            .current_candidate_for_source(&binding.source)?
            .ok_or_else(|| AdaptiveError::Binding("no selected candidate".to_string()))?;
        if current.digest()? != binding_digest || binding.source.source_fence != source_fence {
            return Err(AdaptiveError::Binding(
                "only the exact current candidate/fence may be consumed".to_string(),
            ));
        }
        let matching = reader.events()?.into_iter().any(|event| {
            matches!(
                event,
                CandidateLedgerEventV1::ReviewAttemptFinished {
                    review_attempt_id: ref attempt,
                    receipt_digest: ref receipt,
                    binding: ref finished_binding,
                    ..
                } if attempt == review_attempt_id
                    && receipt == receipt_digest
                    && finished_binding == binding
            )
        });
        if !matching {
            return Err(AdaptiveError::Binding(
                "consumption requires an exact finished receipt".to_string(),
            ));
        }
        let event_id = identity(
            "wg-review-consumed-v1",
            &(
                review_attempt_id,
                &binding_digest,
                receipt_digest,
                controller_policy_digest,
                source_fence,
                &effect,
            ),
        )?;
        if reader.event(&event_id)?.is_some() {
            return Ok(event_id);
        }
        let event = CandidateLedgerEventV1::ReviewConsumed {
            schema: ADAPTIVE_SCHEMA_VERSION,
            event_id: event_id.clone(),
            review_attempt_id: review_attempt_id.to_string(),
            binding_digest,
            receipt_digest: receipt_digest.to_string(),
            controller_policy_digest: controller_policy_digest.to_string(),
            source_fence,
            consumed_at: consumed_at.to_string(),
            effect,
        };
        write_event(&self.root, LedgerAuthor::CompletionController, event)?;
        Ok(event_id)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDispositionV1 {
    Done,
    Failed,
    Abandoned,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AssignmentProvenanceV1 {
    BoundReceipt(String),
    NoAttempt,
    ImportedUncomposed(String),
    UnknownLegacy(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TerminalProvenanceV1 {
    CompletionReceipt(String),
    FailureEvent(String),
    CancellationEvent(String),
    OperatorAcceptance(String),
    UnknownLegacy(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceQualityEligibilityV1 {
    Eligible,
    Ineligible { reason: String },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SemanticTrajectoryV1 {
    pub passes: u32,
    pub rejects: u32,
    pub inconclusive: u32,
    pub candidate_count: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct InfrastructureSummaryV1 {
    pub attempts: u32,
    pub timeouts: u32,
    pub unavailable: u32,
    pub malformed: u32,
    pub route_drift: u32,
    pub interrupted_unknown: u32,
    pub other: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrajectorySealV1 {
    pub schema: u16,
    pub seal_id: String,
    pub graph_identity: String,
    pub task_id: String,
    pub generation: u64,
    pub terminal_event_id: String,
    pub candidate_ledger_head: Option<String>,
    pub ordered_event_ids: Vec<String>,
    pub trajectory_digest: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalEpisodeInputV1 {
    pub graph_identity: String,
    pub task_id: String,
    pub generation: u64,
    pub terminal_event_id: String,
    pub terminal_disposition: TerminalDispositionV1,
    pub source_attempt_id: Option<String>,
    pub source_fence: Option<u64>,
    pub assignment_provenance: AssignmentProvenanceV1,
    pub terminal_provenance: TerminalProvenanceV1,
    pub terminal_candidate_binding: Option<CandidateBindingV1>,
    pub source_quality_eligibility: SourceQualityEligibilityV1,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningEpisodeV1 {
    pub schema: u16,
    pub episode_id: String,
    pub policy_version: String,
    pub graph_identity: String,
    pub task_id: String,
    pub generation: u64,
    pub terminal_event_id: String,
    pub terminal_disposition: TerminalDispositionV1,
    pub source_attempt_id: Option<String>,
    pub source_fence: Option<u64>,
    pub assignment_provenance: AssignmentProvenanceV1,
    pub terminal_provenance: TerminalProvenanceV1,
    pub terminal_candidate_binding: Option<CandidateBindingV1>,
    pub trajectory_seal_id: String,
    pub trajectory_event_ids: Vec<String>,
    pub trajectory_digest: String,
    pub semantic_trajectory: SemanticTrajectoryV1,
    pub infrastructure_summary: InfrastructureSummaryV1,
    pub source_quality_eligibility: SourceQualityEligibilityV1,
    pub created_at: String,
}

impl LearningProjector {
    pub fn seal_trajectory(
        &self,
        graph_identity: &str,
        task_id: &str,
        generation: u64,
        terminal_event_id: &str,
        created_at: &str,
    ) -> Result<TrajectorySealV1, AdaptiveError> {
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        let all_events = reader.events()?;
        let source_binding_digests = all_events
            .iter()
            .filter_map(|event| event.binding())
            .filter(|binding| {
                binding.source.graph_identity == graph_identity
                    && binding.source.task_id == task_id
                    && binding.source.generation == generation
            })
            .filter_map(|binding| binding.digest().ok())
            .collect::<HashSet<_>>();
        let mut events = all_events
            .into_iter()
            .filter(|event| match event {
                CandidateLedgerEventV1::CandidateSelected { binding, .. }
                | CandidateLedgerEventV1::ReviewAttemptStarted { binding, .. }
                | CandidateLedgerEventV1::ReviewAttemptFinished { binding, .. } => {
                    binding.source.graph_identity == graph_identity
                        && binding.source.task_id == task_id
                        && binding.source.generation == generation
                }
                CandidateLedgerEventV1::CandidateSuperseded {
                    binding_digest,
                    superseded_by_binding_digest,
                    ..
                } => {
                    source_binding_digests.contains(binding_digest)
                        || source_binding_digests.contains(superseded_by_binding_digest)
                }
                CandidateLedgerEventV1::ReviewConsumed { binding_digest, .. } => {
                    source_binding_digests.contains(binding_digest)
                }
            })
            .collect::<Vec<_>>();
        events.sort_by(|left, right| event_sort_key(left).cmp(&event_sort_key(right)));
        let ordered_event_ids = events
            .iter()
            .map(|event| event.event_id().to_string())
            .collect::<Vec<_>>();
        let trajectory_digest = identity("wg-trajectory-v1", &ordered_event_ids)?;
        let seal_id = identity(
            "wg-trajectory-seal-v1",
            &(graph_identity, task_id, generation, terminal_event_id),
        )?;
        let seal = TrajectorySealV1 {
            schema: ADAPTIVE_SCHEMA_VERSION,
            seal_id: seal_id.clone(),
            graph_identity: graph_identity.to_string(),
            task_id: task_id.to_string(),
            generation,
            terminal_event_id: terminal_event_id.to_string(),
            candidate_ledger_head: ordered_event_ids.last().cloned(),
            ordered_event_ids,
            trajectory_digest,
            created_at: created_at.to_string(),
        };
        write_create_once(
            &self.root.join("trajectory-seals").join(file_id(&seal_id)),
            &seal,
            |existing| existing == &seal,
        )?;
        Ok(seal)
    }

    pub fn project(
        &self,
        input: TerminalEpisodeInputV1,
        seal: &TrajectorySealV1,
    ) -> Result<LearningEpisodeV1, AdaptiveError> {
        if seal.graph_identity != input.graph_identity
            || seal.task_id != input.task_id
            || seal.generation != input.generation
            || seal.terminal_event_id != input.terminal_event_id
        {
            return Err(AdaptiveError::Binding(
                "trajectory seal does not bind terminal episode".to_string(),
            ));
        }
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        let events = seal
            .ordered_event_ids
            .iter()
            .map(|id| {
                reader.event(id)?.ok_or_else(|| {
                    AdaptiveError::Binding(format!("trajectory event {id} is missing"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if identity("wg-trajectory-v1", &seal.ordered_event_ids)? != seal.trajectory_digest {
            return Err(AdaptiveError::Collision(seal.seal_id.clone()));
        }
        let (semantic_trajectory, infrastructure_summary) = summarize(&events);
        let episode_id = identity(
            "wg-learning-episode-v1",
            &(
                ADAPTIVE_POLICY_VERSION,
                &input.graph_identity,
                &input.task_id,
                input.generation,
                &input.terminal_event_id,
            ),
        )?;
        // First-terminal-wins: another episode for this graph/task/generation
        // is lifecycle corruption, even when it has a different event ID.
        for existing in reader.episodes()? {
            if existing.graph_identity == input.graph_identity
                && existing.task_id == input.task_id
                && existing.generation == input.generation
                && existing.terminal_event_id != input.terminal_event_id
            {
                return Err(AdaptiveError::Collision(format!(
                    "generation {} already projected from terminal event {}",
                    input.generation, existing.terminal_event_id
                )));
            }
        }
        let episode = LearningEpisodeV1 {
            schema: ADAPTIVE_SCHEMA_VERSION,
            episode_id: episode_id.clone(),
            policy_version: ADAPTIVE_POLICY_VERSION.to_string(),
            graph_identity: input.graph_identity,
            task_id: input.task_id,
            generation: input.generation,
            terminal_event_id: input.terminal_event_id,
            terminal_disposition: input.terminal_disposition,
            source_attempt_id: input.source_attempt_id,
            source_fence: input.source_fence,
            assignment_provenance: input.assignment_provenance,
            terminal_provenance: input.terminal_provenance,
            terminal_candidate_binding: input.terminal_candidate_binding,
            trajectory_seal_id: seal.seal_id.clone(),
            trajectory_event_ids: seal.ordered_event_ids.clone(),
            trajectory_digest: seal.trajectory_digest.clone(),
            semantic_trajectory,
            infrastructure_summary,
            source_quality_eligibility: input.source_quality_eligibility,
            created_at: input.created_at,
        };
        write_create_once(
            &self
                .root
                .join("terminal-episodes")
                .join(file_id(&episode_id)),
            &episode,
            |existing| existing == &episode,
        )?;
        Ok(episode)
    }

    pub fn record_assessment(
        &self,
        input: OutcomeAssessmentInputV1,
    ) -> Result<OutcomeAssessmentV1, AdaptiveError> {
        if !(0.0..=1.0).contains(&input.score) || !input.score.is_finite() {
            return Err(AdaptiveError::Binding(
                "outcome score must be finite and in [0,1]".to_string(),
            ));
        }
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        if !reader
            .episodes()?
            .iter()
            .any(|episode| episode.episode_id == input.episode_id)
        {
            return Err(AdaptiveError::Binding("episode is missing".to_string()));
        }
        let mut reasons = Vec::new();
        for forbidden in [
            input.source_principal.as_deref(),
            input.assigner_principal.as_deref(),
            input.evolver_principal.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if forbidden == input.scorer_principal {
                reasons.push(format!("scorer principal equals {forbidden}"));
            }
        }
        if input
            .calibrated_reviewer_principals
            .iter()
            .any(|principal| principal == &input.scorer_principal)
        {
            reasons.push("scorer is a completion reviewer being calibrated".to_string());
        }
        if input.source_route_cohort == input.scorer_route_cohort {
            reasons.push("scorer route cohort equals source route cohort".to_string());
        }
        if !input.fresh_context {
            reasons.push("scorer context is not fresh".to_string());
        }
        if !input.read_only_capabilities {
            reasons.push("scorer capabilities are not read-only".to_string());
        }
        let independence = if reasons.is_empty() {
            AssessmentIndependenceV1::Independent
        } else {
            AssessmentIndependenceV1::NonIndependent { reasons }
        };
        let assessment_id = identity(
            "wg-outcome-assessment-v1",
            &(
                &input.episode_id,
                &input.scorer_policy_id,
                &input.evidence_digest,
                input.route.route_generation,
            ),
        )?;
        let assessment = OutcomeAssessmentV1 {
            schema: ADAPTIVE_SCHEMA_VERSION,
            assessment_id: assessment_id.clone(),
            episode_id: input.episode_id,
            scorer_policy_id: input.scorer_policy_id,
            scorer_principal: input.scorer_principal,
            route: input.route,
            evidence_digest: input.evidence_digest,
            score: input.score,
            dimensions: input.dimensions,
            notes_digest: input.notes_digest,
            usage: input.usage,
            usage_state: input.usage_state,
            independence,
            created_at: input.created_at,
        };
        write_create_once(
            &self
                .root
                .join("outcome-assessments")
                .join(file_id(&assessment_id)),
            &assessment,
            |existing| existing == &assessment,
        )?;
        Ok(assessment)
    }

    pub fn performance_projection(&self) -> Result<PerformanceProjectionV1, AdaptiveError> {
        let reader = AdaptiveReadStore {
            root: self.root.clone(),
        };
        let eligible = reader
            .episodes()?
            .into_iter()
            .filter(|episode| {
                episode.source_quality_eligibility == SourceQualityEligibilityV1::Eligible
            })
            .collect::<Vec<_>>();
        let ids = eligible
            .iter()
            .map(|episode| episode.episode_id.clone())
            .collect::<BTreeSet<_>>();
        let assessments = reader.assessments()?;
        let scores = assessments
            .iter()
            .filter(|assessment| {
                ids.contains(&assessment.episode_id)
                    && assessment.independence == AssessmentIndependenceV1::Independent
            })
            .fold(BTreeMap::<String, f64>::new(), |mut active, assessment| {
                // Deterministic policy: lexically-last create-once assessment
                // for an episode wins; task_count remains the episode count.
                active.insert(assessment.episode_id.clone(), assessment.score);
                active
            });
        let avg_score =
            (!scores.is_empty()).then(|| scores.values().sum::<f64>() / scores.len() as f64);
        let projection = PerformanceProjectionV1 {
            schema: ADAPTIVE_SCHEMA_VERSION,
            policy_version: "terminal-episode-v1".to_string(),
            task_count: ids.len() as u64,
            scored_episode_count: scores.len() as u64,
            avg_score,
            episode_ids: ids.into_iter().collect(),
            input_digest: identity("wg-performance-input-v1", &(eligible, assessments))?,
        };
        let path = self
            .root
            .join("performance-projections/terminal-episode-v1.json");
        write_replace_atomic(&path, &projection)?;
        Ok(projection)
    }
}

#[derive(Clone, Debug)]
pub struct OutcomeAssessmentInputV1 {
    pub episode_id: String,
    pub scorer_policy_id: String,
    pub scorer_principal: String,
    pub route: RouteSnapshot,
    pub evidence_digest: String,
    pub score: f64,
    pub dimensions: BTreeMap<String, f64>,
    pub notes_digest: String,
    pub usage: Option<UsageV1>,
    pub usage_state: UsageStateV1,
    pub source_principal: Option<String>,
    pub assigner_principal: Option<String>,
    pub evolver_principal: Option<String>,
    pub calibrated_reviewer_principals: Vec<String>,
    pub source_route_cohort: String,
    pub scorer_route_cohort: String,
    pub fresh_context: bool,
    pub read_only_capabilities: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum UsageStateV1 {
    Reported,
    Unavailable(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "reasons", rename_all = "snake_case")]
pub enum AssessmentIndependenceV1 {
    Independent,
    NonIndependent { reasons: Vec<String> },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OutcomeAssessmentV1 {
    pub schema: u16,
    pub assessment_id: String,
    pub episode_id: String,
    pub scorer_policy_id: String,
    pub scorer_principal: String,
    pub route: RouteSnapshot,
    pub evidence_digest: String,
    pub score: f64,
    pub dimensions: BTreeMap<String, f64>,
    pub notes_digest: String,
    pub usage: Option<UsageV1>,
    pub usage_state: UsageStateV1,
    pub independence: AssessmentIndependenceV1,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PerformanceProjectionV1 {
    pub schema: u16,
    pub policy_version: String,
    pub task_count: u64,
    pub scored_episode_count: u64,
    pub avg_score: Option<f64>,
    pub episode_ids: Vec<String>,
    pub input_digest: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct LaneAccountingV1 {
    pub attempt_count: u64,
    pub attempts_with_reported_usage: u64,
    pub unknown_cost_attempts: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub provider_cost: f64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AdaptiveAccountingV1 {
    pub completion_flip: LaneAccountingV1,
    pub completion_eval: LaneAccountingV1,
    pub outcome_scorer: LaneAccountingV1,
    pub all_agency_provider_cost: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReviewAttemptViewV1 {
    pub alias: String,
    pub review_run_id: String,
    pub review_attempt_id: String,
    pub ordinal: u32,
    pub reviewer_kind: ReviewerKind,
    pub binding: CandidateBindingV1,
    pub route: RouteSnapshot,
    pub started_at: TimeEvidence,
    pub lease_expires_at: Option<String>,
    pub outcome: Option<ReviewOutcomeV1>,
    pub duration_ms: Option<u64>,
    pub findings_digest: Option<String>,
    pub usage: Option<UsageV1>,
    pub receipt_digest: Option<String>,
    pub consumed: bool,
    pub current_candidate: bool,
}

impl AdaptiveReadStore {
    pub fn events(&self) -> Result<Vec<CandidateLedgerEventV1>, AdaptiveError> {
        let dir = self.root.join("candidate-ledger/events");
        let mut events = Vec::new();
        if !dir.exists() {
            return Ok(events);
        }
        for path in json_files(&dir)? {
            let bytes = fs::read(&path)?;
            let envelope: EventEnvelope = serde_json::from_slice(&bytes)?;
            verify_event_author(&envelope)?;
            let expected = self
                .root
                .join("candidate-ledger/events")
                .join(file_id(envelope.event.event_id()));
            if path != expected {
                return Err(AdaptiveError::Collision(path.display().to_string()));
            }
            events.push(envelope.event);
        }
        events.sort_by(|left, right| event_sort_key(left).cmp(&event_sort_key(right)));
        Ok(events)
    }

    pub fn event(&self, id: &str) -> Result<Option<CandidateLedgerEventV1>, AdaptiveError> {
        let path = self.root.join("candidate-ledger/events").join(file_id(id));
        if !path.exists() {
            return Ok(None);
        }
        let envelope: EventEnvelope = serde_json::from_slice(&fs::read(path)?)?;
        verify_event_author(&envelope)?;
        if envelope.event.event_id() != id {
            return Err(AdaptiveError::Collision(id.to_string()));
        }
        Ok(Some(envelope.event))
    }

    pub fn current_candidate_for_source(
        &self,
        source: &SourceBindingV1,
    ) -> Result<Option<CandidateBindingV1>, AdaptiveError> {
        let mut selected = Vec::new();
        let mut superseded = HashSet::new();
        for event in self.events()? {
            match event {
                CandidateLedgerEventV1::CandidateSelected { binding, .. }
                    if binding.source == *source =>
                {
                    selected.push(binding)
                }
                CandidateLedgerEventV1::CandidateSuperseded { binding_digest, .. } => {
                    superseded.insert(binding_digest);
                }
                _ => {}
            }
        }
        let mut current = selected
            .into_iter()
            .filter(|binding| {
                binding
                    .digest()
                    .map(|digest| !superseded.contains(&digest))
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        current.sort_by_key(|binding| binding.candidate_sequence);
        if current.len() > 1 {
            return Err(AdaptiveError::Collision(
                "multiple current candidates for one source attempt".to_string(),
            ));
        }
        Ok(current.pop())
    }

    pub fn attempts_for_run(
        &self,
        review_run_id: &str,
    ) -> Result<Vec<ReviewAttemptHandle>, AdaptiveError> {
        let mut attempts = self
            .events()?
            .into_iter()
            .filter_map(|event| match event {
                CandidateLedgerEventV1::ReviewAttemptStarted {
                    event_id,
                    review_run_id: run,
                    review_attempt_id,
                    ordinal,
                    binding,
                    ..
                } if run == review_run_id => {
                    Some(binding.digest().map(|binding_digest| ReviewAttemptHandle {
                        started_event_id: event_id,
                        review_run_id: run,
                        review_attempt_id,
                        ordinal,
                        binding_digest,
                    }))
                }
                _ => None,
            })
            .collect::<Result<Vec<_>, _>>()?;
        attempts.sort_by_key(|attempt| attempt.ordinal);
        Ok(attempts)
    }

    pub fn review_attempts(&self) -> Result<Vec<ReviewAttemptViewV1>, AdaptiveError> {
        let events = self.events()?;
        let mut finished = HashMap::new();
        let mut consumed = HashSet::new();
        for event in &events {
            match event {
                CandidateLedgerEventV1::ReviewAttemptFinished {
                    review_attempt_id, ..
                } => {
                    finished.insert(review_attempt_id.clone(), event.clone());
                }
                CandidateLedgerEventV1::ReviewConsumed {
                    review_attempt_id, ..
                } => {
                    consumed.insert(review_attempt_id.clone());
                }
                _ => {}
            }
        }
        let mut views = Vec::new();
        for event in events {
            let CandidateLedgerEventV1::ReviewAttemptStarted {
                review_run_id,
                review_attempt_id,
                ordinal,
                reviewer_kind,
                binding,
                route,
                started_at,
                lease_expires_at,
                ..
            } = event
            else {
                continue;
            };
            let current_candidate = self
                .current_candidate_for_source(&binding.source)?
                .is_some_and(|candidate| candidate == binding);
            let alias = virtual_alias(&binding, reviewer_kind, ordinal);
            let (outcome, duration_ms, findings_digest, usage, receipt_digest) = finished
                .get(&review_attempt_id)
                .and_then(|event| match event {
                    CandidateLedgerEventV1::ReviewAttemptFinished {
                        outcome,
                        duration_ms,
                        findings_digest,
                        usage,
                        receipt_digest,
                        ..
                    } => Some((
                        Some(outcome.clone()),
                        Some(*duration_ms),
                        findings_digest.clone(),
                        usage.clone(),
                        Some(receipt_digest.clone()),
                    )),
                    _ => None,
                })
                .unwrap_or((None, None, None, None, None));
            views.push(ReviewAttemptViewV1 {
                alias,
                review_run_id,
                review_attempt_id: review_attempt_id.clone(),
                ordinal,
                reviewer_kind,
                binding,
                route,
                started_at,
                lease_expires_at,
                outcome,
                duration_ms,
                findings_digest,
                usage,
                receipt_digest,
                consumed: consumed.contains(&review_attempt_id),
                current_candidate,
            });
        }
        views.sort_by(|left, right| {
            left.binding
                .task_id()
                .cmp(right.binding.task_id())
                .then(left.binding.generation().cmp(&right.binding.generation()))
                .then(
                    left.binding
                        .candidate_sequence
                        .cmp(&right.binding.candidate_sequence),
                )
                .then(left.ordinal.cmp(&right.ordinal))
        });
        Ok(views)
    }

    pub fn episodes(&self) -> Result<Vec<LearningEpisodeV1>, AdaptiveError> {
        load_objects(&self.root.join("terminal-episodes"))
    }

    pub fn assessments(&self) -> Result<Vec<OutcomeAssessmentV1>, AdaptiveError> {
        load_objects(&self.root.join("outcome-assessments"))
    }

    pub fn accounting(&self) -> Result<AdaptiveAccountingV1, AdaptiveError> {
        let mut accounting = AdaptiveAccountingV1::default();
        let attempts = self.review_attempts()?;
        for attempt in attempts {
            let lane = match attempt.reviewer_kind {
                ReviewerKind::Flip => &mut accounting.completion_flip,
                ReviewerKind::Eval => &mut accounting.completion_eval,
            };
            add_attempt(lane, attempt.usage.as_ref());
        }
        for assessment in self.assessments()? {
            add_attempt(&mut accounting.outcome_scorer, assessment.usage.as_ref());
        }
        accounting.all_agency_provider_cost = accounting.completion_flip.provider_cost
            + accounting.completion_eval.provider_cost
            + accounting.outcome_scorer.provider_cost;
        Ok(accounting)
    }

    pub fn backlog(&self, now: &str) -> Result<LearningBacklogV1, AdaptiveError> {
        let now = DateTime::parse_from_rfc3339(now)
            .map_err(|_| AdaptiveError::Binding("invalid backlog time".to_string()))?;
        let events = self.events()?;
        let finished = events
            .iter()
            .filter_map(|event| match event {
                CandidateLedgerEventV1::ReviewAttemptFinished {
                    review_attempt_id, ..
                } => Some(review_attempt_id.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let expired_unsettled_attempts = events
            .iter()
            .filter(|event| match event {
                CandidateLedgerEventV1::ReviewAttemptStarted {
                    review_attempt_id,
                    lease_expires_at: Some(expires),
                    ..
                } => {
                    !finished.contains(review_attempt_id.as_str())
                        && DateTime::parse_from_rfc3339(expires)
                            .map(|expires| expires <= now)
                            .unwrap_or(true)
                }
                _ => false,
            })
            .count();
        Ok(LearningBacklogV1 {
            expired_unsettled_attempts,
            invalid_objects: 0,
        })
    }
}

impl CandidateBindingV1 {
    fn task_id(&self) -> &str {
        &self.source.task_id
    }
    fn generation(&self) -> u64 {
        self.source.generation
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningBacklogV1 {
    pub expired_unsettled_attempts: usize,
    pub invalid_objects: usize,
}

pub fn virtual_alias(binding: &CandidateBindingV1, kind: ReviewerKind, ordinal: u32) -> String {
    let prefix = match kind {
        ReviewerKind::Flip => "flip",
        ReviewerKind::Eval => "evaluate",
    };
    format!(
        ".{prefix}-{}@g{}/a{}/c{}/r{}",
        binding.source.task_id,
        binding.source.generation,
        binding.source.source_attempt_id,
        binding.candidate_sequence,
        ordinal
    )
}

pub fn is_virtual_review_alias(value: &str) -> bool {
    (value.starts_with(".flip-") || value.starts_with(".evaluate-"))
        && value.contains("@g")
        && value.contains("/a")
        && value.contains("/c")
        && value.contains("/r")
}

pub fn non_authoritative_error(alias: &str) -> String {
    format!(
        "WG-VIRTUAL-REVIEW-NON-AUTHORITATIVE: '{alias}' is a virtual review projection, not a graph task; use `wg reviews show {alias}` or target the source task with a lifecycle command"
    )
}

fn add_attempt(lane: &mut LaneAccountingV1, usage: Option<&UsageV1>) {
    lane.attempt_count += 1;
    if let Some(usage) = usage {
        lane.attempts_with_reported_usage += 1;
        lane.input_tokens = lane.input_tokens.saturating_add(usage.input_tokens);
        lane.output_tokens = lane.output_tokens.saturating_add(usage.output_tokens);
        lane.cache_read_tokens = lane
            .cache_read_tokens
            .saturating_add(usage.cache_read_tokens);
        lane.cache_write_tokens = lane
            .cache_write_tokens
            .saturating_add(usage.cache_write_tokens);
        if let Some(cost) = usage.provider_cost {
            lane.provider_cost += cost;
        } else {
            lane.unknown_cost_attempts += 1;
        }
    } else {
        lane.unknown_cost_attempts += 1;
    }
}

fn summarize(events: &[CandidateLedgerEventV1]) -> (SemanticTrajectoryV1, InfrastructureSummaryV1) {
    let mut semantic = SemanticTrajectoryV1::default();
    let mut infrastructure = InfrastructureSummaryV1::default();
    let mut candidates = HashSet::new();
    for event in events {
        match event {
            CandidateLedgerEventV1::CandidateSelected { binding, .. } => {
                if let Ok(digest) = binding.digest() {
                    candidates.insert(digest);
                }
            }
            CandidateLedgerEventV1::ReviewAttemptFinished { outcome, .. } => match outcome {
                ReviewOutcomeV1::Semantic(SemanticOutcome::Pass) => semantic.passes += 1,
                ReviewOutcomeV1::Semantic(SemanticOutcome::Reject) => semantic.rejects += 1,
                ReviewOutcomeV1::Semantic(SemanticOutcome::Inconclusive) => {
                    semantic.inconclusive += 1
                }
                ReviewOutcomeV1::Infrastructure(outcome) => {
                    infrastructure.attempts += 1;
                    match outcome {
                        InfrastructureOutcome::Timeout => infrastructure.timeouts += 1,
                        InfrastructureOutcome::AdapterUnavailable => {
                            infrastructure.unavailable += 1
                        }
                        InfrastructureOutcome::MalformedOutput => infrastructure.malformed += 1,
                        InfrastructureOutcome::RouteDrift => infrastructure.route_drift += 1,
                        InfrastructureOutcome::InterruptedUnknown => {
                            infrastructure.interrupted_unknown += 1
                        }
                        _ => infrastructure.other += 1,
                    }
                }
            },
            _ => {}
        }
    }
    semantic.candidate_count = candidates.len() as u32;
    (semantic, infrastructure)
}

fn event_sort_key(event: &CandidateLedgerEventV1) -> (String, u8, String) {
    match event {
        CandidateLedgerEventV1::CandidateSelected {
            selected_at,
            event_id,
            ..
        } => (selected_at.clone(), 0, event_id.clone()),
        CandidateLedgerEventV1::ReviewAttemptStarted {
            started_at,
            event_id,
            ..
        } => (
            match started_at {
                TimeEvidence::Observed(at) => at.clone(),
                TimeEvidence::UnknownLegacy => String::new(),
            },
            1,
            event_id.clone(),
        ),
        CandidateLedgerEventV1::ReviewAttemptFinished {
            completed_at,
            event_id,
            ..
        } => (completed_at.clone(), 2, event_id.clone()),
        CandidateLedgerEventV1::CandidateSuperseded {
            superseded_at,
            event_id,
            ..
        } => (superseded_at.clone(), 3, event_id.clone()),
        CandidateLedgerEventV1::ReviewConsumed {
            consumed_at,
            event_id,
            ..
        } => (consumed_at.clone(), 4, event_id.clone()),
    }
}

fn verify_event_author(envelope: &EventEnvelope) -> Result<(), AdaptiveError> {
    let valid = matches!(
        (&envelope.author, &envelope.event),
        (
            LedgerAuthor::CandidateSelection,
            CandidateLedgerEventV1::CandidateSelected { .. }
                | CandidateLedgerEventV1::CandidateSuperseded { .. }
        ) | (
            LedgerAuthor::ReviewAttempt,
            CandidateLedgerEventV1::ReviewAttemptStarted { .. }
                | CandidateLedgerEventV1::ReviewAttemptFinished { .. }
        ) | (
            LedgerAuthor::CompletionController,
            CandidateLedgerEventV1::ReviewConsumed { .. }
        )
    );
    if !valid {
        return Err(AdaptiveError::Binding(
            "ledger event was authored by the wrong capability sink".to_string(),
        ));
    }
    Ok(())
}

fn write_event(
    root: &Path,
    author: LedgerAuthor,
    event: CandidateLedgerEventV1,
) -> Result<(), AdaptiveError> {
    let envelope = EventEnvelope { author, event };
    verify_event_author(&envelope)?;
    let id = envelope.event.event_id().to_string();
    write_create_once(
        &root.join("candidate-ledger/events").join(file_id(&id)),
        &envelope,
        |existing| existing.event.event_id() == id && existing == &envelope,
    )
}

fn identity<T: Serialize>(domain: &str, value: &T) -> Result<String, AdaptiveError> {
    let mut bytes = Vec::with_capacity(domain.len() + 128);
    bytes.extend_from_slice(domain.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&canonical_json(&serde_json::to_value(value)?));
    Ok(format!("b3:{}", blake3::hash(&bytes).to_hex()))
}

fn safe_hash(id: &str) -> String {
    id.strip_prefix("b3:").unwrap_or(id).to_string()
}

fn file_id(id: &str) -> PathBuf {
    PathBuf::from(format!("{}.json", safe_hash(id)))
}

fn write_create_once<T, F>(path: &Path, value: &T, same: F) -> Result<(), AdaptiveError>
where
    T: Serialize + for<'de> Deserialize<'de>,
    F: FnOnce(&T) -> bool,
{
    let bytes = canonical_json(&serde_json::to_value(value)?);
    match crate::atomic_file::write_atomic_create_new(path, &bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing: T = serde_json::from_slice(&fs::read(path)?)?;
            if same(&existing) {
                Ok(())
            } else {
                Err(AdaptiveError::Collision(path.display().to_string()))
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn write_replace_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), AdaptiveError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = canonical_json(&serde_json::to_value(value)?);
    crate::atomic_file::write_atomic(path, &bytes)?;
    Ok(())
}

fn load_objects<T>(dir: &Path) -> Result<Vec<T>, AdaptiveError>
where
    T: for<'de> Deserialize<'de>,
{
    if !dir.exists() {
        return Ok(Vec::new());
    }
    json_files(dir)?
        .into_iter()
        .map(|path| Ok(serde_json::from_slice(&fs::read(path)?)?))
        .collect()
}

fn json_files(dir: &Path) -> Result<Vec<PathBuf>, AdaptiveError> {
    let mut files = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

struct DirectoryLock {
    path: PathBuf,
}

impl DirectoryLock {
    fn acquire(path: &Path) -> Result<Self, AdaptiveError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let start = Instant::now();
        loop {
            match fs::create_dir(path) {
                Ok(()) => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if start.elapsed() >= Duration::from_secs(5) {
                        return Err(AdaptiveError::Lock(path.display().to_string()));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

/// Convert bounded completion findings to a deterministic digest without
/// copying attacker-controlled prose into ledger identities.
pub fn findings_digest(findings: &[ReviewFinding]) -> Result<String, AdaptiveError> {
    identity("wg-review-findings-v1", &findings)
}
