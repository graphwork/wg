//! Fail-closed staged rollout for the Pi evaluation plane.
//!
//! This controller is intentionally operational rather than scheduler state:
//! it owns the feature flags, an ordered stage, and content-addressed canary
//! evidence. Evaluation runners may read the resulting advisory policy but
//! cannot advance it.

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
    config.agency.auto_evaluate = stage == EvaluationRolloutStage::Advisory;
    // This release is advisory-only. A threshold, global gate, or eager FLIP
    // must not survive any managed transition, including rollback.
    // The historical optional threshold can be inherited from global/default
    // config and TOML has no explicit-null override. Managed rollout policy
    // therefore makes it inert in `LazyEvaluationSelection`; eval_gate_all is
    // still structurally rejected below.
    config.agency.eval_gate_threshold = None;
    config.agency.eval_gate_all = false;
    config.agency.flip_enabled = false;
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
        EvaluationRolloutStage::FakePiValidated => {
            Some(EvaluationRolloutStage::BoundedCanaryPassed)
        }
        EvaluationRolloutStage::BoundedCanaryPassed => {
            Some(EvaluationRolloutStage::DeepReadonlyCanaryPassed)
        }
        EvaluationRolloutStage::DeepReadonlyCanaryPassed => Some(EvaluationRolloutStage::Advisory),
        EvaluationRolloutStage::Advisory => None,
    }
}

fn expected_kind(stage: EvaluationRolloutStage) -> Option<CanaryKind> {
    match stage {
        EvaluationRolloutStage::FakePiValidated => Some(CanaryKind::FakePiLifecycle),
        EvaluationRolloutStage::BoundedCanaryPassed => Some(CanaryKind::BoundedLiveCanary),
        EvaluationRolloutStage::DeepReadonlyCanaryPassed => Some(CanaryKind::DeepReadonlyFlip),
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
    if evidence.kind == CanaryKind::DeepReadonlyFlip
        && (!evidence.observation_only
            || evidence.latent_intent_findings == 0
            || evidence.counterfactual_findings == 0
            || evidence.cross_system_findings == 0)
    {
        bail!(
            "deep-readonly FLIP canary must prove observation-only authority, latent intent, genuine counterfactual analysis, and cross-system findings; a bounded grader is not FLIP"
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
    let required = next_stage(state.stage).context("evaluation rollout is already advisory")?;
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
        bail!(
            "advisory advancement does not accept new evidence; prior canaries are authoritative"
        );
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
    if state.stage != EvaluationRolloutStage::Advisory {
        bail!("real source observations may be recorded only at advisory stage");
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
    save_state(dir, &state)?;
    // Load local config deliberately: rollback must repair even a forged
    // merged stage that normal daemon reload correctly refuses.
    let mut config = Config::load(dir)?;
    apply_flags(&mut config, EvaluationRolloutStage::Disabled);
    config.save(dir)?;
    status(dir)
}

pub fn status(dir: &Path) -> Result<RolloutStatus> {
    let state = load_state(dir)?;
    let config = Config::load_merged(dir)?;
    validate_managed_config(dir, &config)?;
    Ok(RolloutStatus {
        schema: state.schema,
        stage: state.stage,
        mode: if state.stage == EvaluationRolloutStage::Advisory {
            "bounded-advisory"
        } else {
            "disabled"
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
        bail!(
            "global evaluation hard gate is forbidden in this rollout; eval_gate_all must remain false"
        );
    }
    if config.agency.flip_enabled {
        bail!("global/eager FLIP is forbidden; request selective deep-readonly FLIP explicitly");
    }
    let expected_auto = state.stage == EvaluationRolloutStage::Advisory;
    if config.agency.auto_evaluate != expected_auto {
        if expected_auto {
            bail!("recorded canaries require auto_evaluate=true in advisory stage");
        }
        bail!(
            "auto_evaluate cannot be enabled before recorded Fake-Pi, bounded, and deep-readonly canary success"
        );
    }
    Ok(())
}
