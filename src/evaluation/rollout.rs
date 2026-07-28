//! Fail-closed staged rollout for the Pi evaluation plane.
//!
//! Deep read-only FLIP is the primary required pre-merge feedback gate. The
//! controller owns an ordered, content-addressed proof path to `flip-required`;
//! bounded evaluation remains an independent optional secondary product.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Config, EvaluationRolloutStage};

pub const CANARY_EVIDENCE_SCHEMA: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CanaryKind {
    FakePiLifecycle,
    BoundedLiveCanary,
    DeepReadonlyFlip,
    /// End-to-end required-gate canary: pending/reject/unavailable keep main
    /// unchanged and an accepted report advances it exactly once.
    FlipRequiredGate,
    SourceObservation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryEvidence {
    pub schema: u16,
    pub kind: CanaryKind,
    pub success: bool,
    pub route: String,
    #[serde(default)]
    pub source_completions: u64,
    #[serde(default)]
    pub evaluation_verdicts: u64,
    #[serde(default)]
    pub never_ran_evaluations: u64,
    #[serde(default)]
    pub stuck_pending_evaluations: u64,
    #[serde(default)]
    pub duplicate_records: u64,
    #[serde(default)]
    pub duplicate_verdicts: u64,
    #[serde(default)]
    pub worker_slots_used: u64,
    #[serde(default)]
    pub build_slots_used: u64,
    #[serde(default)]
    pub worktrees_created: u64,
    #[serde(default)]
    pub admission_deferrals_neutral: bool,
    #[serde(default)]
    pub native_codex_route_preserved: bool,
    #[serde(default)]
    pub observation_only: bool,
    #[serde(default)]
    pub latent_intent_findings: u64,
    #[serde(default)]
    pub counterfactual_findings: u64,
    #[serde(default)]
    pub cross_system_findings: u64,
    #[serde(default)]
    pub semantic_reject_preserved: bool,
    #[serde(default)]
    pub infrastructure_retry_converged: bool,
    #[serde(default)]
    pub restart_boundaries_proven: bool,
    #[serde(default)]
    pub main_unchanged_pending_reject_unavailable: bool,
    #[serde(default)]
    pub main_advanced_once_on_pass: bool,
    #[serde(default)]
    pub gate_left_disabled: bool,
    pub before_viz_cid: String,
    pub after_viz_cid: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedEvidence {
    pub evidence_id: String,
    pub recorded_at: String,
    #[serde(flatten)]
    pub evidence: CanaryEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackRecord {
    pub from_stage: EvaluationRolloutStage,
    pub reason: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutState {
    pub schema: u16,
    pub stage: EvaluationRolloutStage,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub evidence: Vec<RecordedEvidence>,
    #[serde(default)]
    pub rollbacks: Vec<RollbackRecord>,
}

impl RolloutState {
    fn new() -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            schema: CANARY_EVIDENCE_SCHEMA,
            stage: EvaluationRolloutStage::Disabled,
            started_at: now.clone(),
            updated_at: now,
            evidence: Vec::new(),
            rollbacks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RolloutStatus {
    pub schema: u16,
    pub stage: EvaluationRolloutStage,
    pub mode: &'static str,
    pub auto_evaluate: bool,
    pub eval_gate_all: bool,
    pub global_flip_enabled: bool,
    pub evidence: Vec<RecordedEvidence>,
    pub rollback_count: usize,
    pub state_path: String,
}

pub fn evidence_path(dir: &Path) -> PathBuf {
    dir.join("agency/evaluation-plane/canary-evidence.json")
}

pub fn managed_stage(dir: &Path) -> Result<Option<EvaluationRolloutStage>> {
    if !evidence_path(dir).exists() {
        return Ok(None);
    }
    Ok(Some(load_state(dir)?.stage))
}

fn load_state(dir: &Path) -> Result<RolloutState> {
    let path = evidence_path(dir);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "evaluation rollout has not started (missing {})",
            path.display()
        )
    })?;
    let state: RolloutState = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid evaluation rollout state at {}", path.display()))?;
    if state.schema != CANARY_EVIDENCE_SCHEMA {
        bail!(
            "evaluation rollout schema {} is unsupported (expected {})",
            state.schema,
            CANARY_EVIDENCE_SCHEMA
        );
    }
    Ok(state)
}

fn save_state(dir: &Path, state: &RolloutState) -> Result<()> {
    let path = evidence_path(dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::atomic_file::write_atomic(&path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

fn apply_flags(config: &mut Config, stage: EvaluationRolloutStage) {
    config.evaluation.managed_rollout = true;
    config.evaluation.rollout_stage = stage;
    // Bounded selection is never a prerequisite for FLIP. Historical advisory
    // ledgers retain their old behavior until the operator advances/rolls back.
    config.agency.auto_evaluate = stage == EvaluationRolloutStage::Advisory;
    config.agency.eval_gate_threshold = None;
    config.agency.eval_gate_all = false;
    config.agency.flip_enabled = stage == EvaluationRolloutStage::FlipRequired;
    if stage == EvaluationRolloutStage::FlipRequired {
        config.agency.flip_verification_threshold =
            Some(config.agency.flip_verification_threshold.unwrap_or(0.8));
    }
}

pub fn start(dir: &Path) -> Result<RolloutStatus> {
    let path = evidence_path(dir);
    if path.exists() {
        let state = load_state(dir)?;
        if state.stage != EvaluationRolloutStage::Disabled {
            bail!(
                "evaluation rollout already started at {}; use status, advance, or rollback",
                state.stage
            );
        }
    } else {
        save_state(dir, &RolloutState::new())?;
    }
    let mut config = Config::load(dir)?;
    apply_flags(&mut config, EvaluationRolloutStage::Disabled);
    config.save(dir)?;
    status(dir)
}

fn next_stage(stage: EvaluationRolloutStage) -> Option<EvaluationRolloutStage> {
    match stage {
        EvaluationRolloutStage::Disabled => Some(EvaluationRolloutStage::FakePiValidated),
        // Product order is Fake-Pi → deep observation canary → required gate.
        // The bounded stage is readable for old ledgers but never a prerequisite.
        EvaluationRolloutStage::FakePiValidated | EvaluationRolloutStage::BoundedCanaryPassed => {
            Some(EvaluationRolloutStage::DeepReadonlyCanaryPassed)
        }
        EvaluationRolloutStage::DeepReadonlyCanaryPassed | EvaluationRolloutStage::Advisory => {
            Some(EvaluationRolloutStage::FlipRequired)
        }
        EvaluationRolloutStage::FlipRequired => None,
    }
}

fn expected_kind(stage: EvaluationRolloutStage) -> Option<CanaryKind> {
    match stage {
        EvaluationRolloutStage::FakePiValidated => Some(CanaryKind::FakePiLifecycle),
        EvaluationRolloutStage::BoundedCanaryPassed => Some(CanaryKind::BoundedLiveCanary),
        EvaluationRolloutStage::DeepReadonlyCanaryPassed => Some(CanaryKind::DeepReadonlyFlip),
        EvaluationRolloutStage::FlipRequired => Some(CanaryKind::FlipRequiredGate),
        _ => None,
    }
}

fn validate_common(evidence: &CanaryEvidence) -> Result<()> {
    if evidence.schema != CANARY_EVIDENCE_SCHEMA {
        bail!("canary evidence schema must be {CANARY_EVIDENCE_SCHEMA}");
    }
    if !evidence.success {
        bail!("failed canary evidence cannot advance rollout");
    }
    if !evidence.route.starts_with("pi:") {
        bail!(
            "canary route must be an exact Pi route; Codex/Claude are not Pi fallback ({:?})",
            evidence.route
        );
    }
    if evidence.source_completions == 0 || evidence.evaluation_verdicts == 0 {
        bail!("canary must include at least one real source completion and evaluation verdict");
    }
    for (name, value) in [
        ("never_ran_evaluations", evidence.never_ran_evaluations),
        (
            "stuck_pending_evaluations",
            evidence.stuck_pending_evaluations,
        ),
        ("duplicate_records", evidence.duplicate_records),
        ("duplicate_verdicts", evidence.duplicate_verdicts),
        ("worker_slots_used", evidence.worker_slots_used),
        ("build_slots_used", evidence.build_slots_used),
        ("worktrees_created", evidence.worktrees_created),
    ] {
        if value != 0 {
            bail!("canary invariant {name} must be zero, got {value}");
        }
    }
    if !evidence.admission_deferrals_neutral {
        bail!("canary must prove admission deferral is neutral");
    }
    if !evidence.native_codex_route_preserved {
        bail!("canary must prove native Codex routing was preserved without Pi fallback");
    }
    if evidence.before_viz_cid.trim().is_empty() || evidence.after_viz_cid.trim().is_empty() {
        bail!("canary must record before/after Viz evidence CIDs");
    }
    Ok(())
}

fn read_and_validate_evidence(
    path: &Path,
    target: EvaluationRolloutStage,
) -> Result<RecordedEvidence> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read canary evidence {}", path.display()))?;
    let evidence: CanaryEvidence = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid canary evidence {}", path.display()))?;
    let expected = expected_kind(target).context("target stage does not accept canary evidence")?;
    if evidence.kind != expected {
        bail!(
            "stage {} requires {:?} evidence, found {:?}",
            target,
            expected,
            evidence.kind
        );
    }
    validate_common(&evidence)?;
    if matches!(
        evidence.kind,
        CanaryKind::DeepReadonlyFlip | CanaryKind::FlipRequiredGate
    ) && (!evidence.observation_only
        || evidence.latent_intent_findings == 0
        || evidence.counterfactual_findings == 0
        || evidence.cross_system_findings == 0)
    {
        bail!(
            "deep-readonly FLIP canary must prove observation-only authority, latent intent, genuine counterfactual analysis, and cross-system findings; a bounded grader is not FLIP"
        );
    }
    if evidence.kind == CanaryKind::FlipRequiredGate
        && (!evidence.semantic_reject_preserved
            || !evidence.infrastructure_retry_converged
            || !evidence.restart_boundaries_proven
            || !evidence.main_unchanged_pending_reject_unavailable
            || !evidence.main_advanced_once_on_pass
            || !evidence.gate_left_disabled)
    {
        bail!(
            "flip-required canary must prove semantic reject preservation, infrastructure-only retry convergence, every restart boundary, unchanged main for pending/reject/unavailable, exactly-once pass merge, and gate-left-disabled operator handoff"
        );
    }
    let value = serde_json::to_value(&evidence)?;
    Ok(RecordedEvidence {
        evidence_id: crate::identity::content_cid(&value),
        recorded_at: Utc::now().to_rfc3339(),
        evidence,
    })
}

pub fn advance(
    dir: &Path,
    target: EvaluationRolloutStage,
    evidence: Option<&Path>,
) -> Result<RolloutStatus> {
    let mut state = load_state(dir)?;
    let required =
        next_stage(state.stage).context("evaluation rollout is already flip-required")?;
    if target != required {
        bail!(
            "cannot advance evaluation rollout from {} to {}; next required stage is {}",
            state.stage,
            target,
            required
        );
    }
    if let Some(_) = expected_kind(target) {
        let path = evidence.context("this rollout stage requires --evidence <json>")?;
        state
            .evidence
            .push(read_and_validate_evidence(path, target)?);
    } else if evidence.is_some() {
        bail!("this rollout advancement does not accept new evidence");
    }
    state.stage = target;
    state.updated_at = Utc::now().to_rfc3339();
    save_state(dir, &state)?;
    let mut config = Config::load(dir)?;
    apply_flags(&mut config, target);
    config.save(dir)?;
    status(dir)
}

pub fn record_observation(dir: &Path, path: &Path) -> Result<RolloutStatus> {
    let mut state = load_state(dir)?;
    if !matches!(
        state.stage,
        EvaluationRolloutStage::Advisory | EvaluationRolloutStage::FlipRequired
    ) {
        bail!("real source observations may be recorded only after canary validation");
    }
    let bytes = fs::read(path)?;
    let evidence: CanaryEvidence = serde_json::from_slice(&bytes)?;
    if evidence.kind != CanaryKind::SourceObservation {
        bail!("observation evidence kind must be source-observation");
    }
    validate_common(&evidence)?;
    let value = serde_json::to_value(&evidence)?;
    state.evidence.push(RecordedEvidence {
        evidence_id: crate::identity::content_cid(&value),
        recorded_at: Utc::now().to_rfc3339(),
        evidence,
    });
    state.updated_at = Utc::now().to_rfc3339();
    save_state(dir, &state)?;
    status(dir)
}

pub fn rollback(dir: &Path, reason: &str) -> Result<RolloutStatus> {
    if reason.trim().is_empty() {
        bail!("rollback requires a non-empty operator reason");
    }
    let mut state = load_state(dir)?;
    state.rollbacks.push(RollbackRecord {
        from_stage: state.stage,
        reason: reason.trim().to_string(),
        recorded_at: Utc::now().to_rfc3339(),
    });
    state.stage = EvaluationRolloutStage::Disabled;
    state.updated_at = Utc::now().to_rfc3339();
    // Disable the dispatch/gate authority first. If the process crashes before
    // the ledger write, config/state mismatch fails closed and rollback is
    // safely retryable; there is never a window with a disabled ledger and a
    // still-enabled live selector.
    let mut config = Config::load(dir)?;
    apply_flags(&mut config, EvaluationRolloutStage::Disabled);
    config.save(dir)?;
    save_state(dir, &state)?;
    status(dir)
}

pub fn status(dir: &Path) -> Result<RolloutStatus> {
    let state = load_state(dir)?;
    let config = Config::load_merged(dir)?;
    validate_managed_config(dir, &config)?;
    Ok(RolloutStatus {
        schema: state.schema,
        stage: state.stage,
        mode: match state.stage {
            EvaluationRolloutStage::FlipRequired => "flip-required",
            EvaluationRolloutStage::Advisory => "bounded-advisory-historical",
            _ => "disabled",
        },
        auto_evaluate: config.agency.auto_evaluate,
        eval_gate_all: config.agency.eval_gate_all,
        global_flip_enabled: config.agency.flip_enabled,
        evidence: state.evidence,
        rollback_count: state.rollbacks.len(),
        state_path: evidence_path(dir).display().to_string(),
    })
}

/// Config/reload guard. Once rollout state exists, its stage is authoritative
/// and the managed feature flags are a closed safe set.
pub fn validate_managed_config(dir: &Path, config: &Config) -> Result<()> {
    let path = evidence_path(dir);
    if !path.exists() && !config.evaluation.managed_rollout {
        return Ok(());
    }
    let state = load_state(dir)?;
    if !config.evaluation.managed_rollout {
        bail!("evaluation rollout state exists but config disabled managed_rollout");
    }
    if config.evaluation.rollout_stage != state.stage {
        bail!(
            "evaluation rollout config stage {} does not match recorded canary stage {}; use `wg evaluate rollout advance` or `rollback`",
            config.evaluation.rollout_stage,
            state.stage
        );
    }
    if config.agency.eval_gate_all {
        bail!("eval_gate_all is irrelevant to managed FLIP and must remain false");
    }
    let flip_required = state.stage == EvaluationRolloutStage::FlipRequired;
    if config.agency.flip_enabled != flip_required {
        bail!(
            "global deep-readonly FLIP selection must be {} at rollout stage {}",
            flip_required,
            state.stage
        );
    }
    let expected_auto = state.stage == EvaluationRolloutStage::Advisory;
    if config.agency.auto_evaluate != expected_auto {
        bail!(
            "bounded auto_evaluate must be {} at rollout stage {}; it is independent of required FLIP",
            expected_auto,
            state.stage
        );
    }
    if flip_required && config.agency.flip_verification_threshold.is_none() {
        bail!("flip-required stage requires a snapshottable FLIP threshold");
    }
    Ok(())
}
