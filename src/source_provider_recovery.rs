//! Explicit, bounded recovery for safely replayable source-provider failures.
//!
//! This is a projection on the existing task/lifecycle record, not a scheduler
//! or a second dispatch authority. The production coordinator supplies the
//! clock and exact `SpawnPlan` bindings, then applies the one lifecycle
//! `GenerationCreated` transition when a retry is due.

use crate::config::SourceProviderRetryConfig;
use crate::dispatch::SpawnPlan;
use crate::graph::{
    CompletionContract, ExecutionOutcome, FailureEvidenceKind, FailureReason, FailureSignal, Task,
};
use crate::lifecycle::AttemptRef;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SOURCE_PROVIDER_RECOVERY_SCHEMA: u32 = 1;
pub const JITTER_DIVISOR: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceProviderRecoveryState {
    Backoff,
    Authorized,
    Running,
    Recovered,
    NeedsAttention,
    Paused,
    Cancelled,
}

impl SourceProviderRecoveryState {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Backoff | Self::Authorized | Self::Running | Self::Paused
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceAttemptRef {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub owner_id: String,
    pub process_epoch: u32,
    pub lifecycle_revision: u64,
}

impl SourceAttemptRef {
    pub fn from_task(task: &Task, attempt: &AttemptRef, lifecycle_revision: u64) -> Self {
        Self {
            task_id: task.id.clone(),
            generation: attempt.generation,
            attempt_id: attempt.id.clone(),
            attempt_fence: attempt.fence,
            owner_id: attempt.actor_id.clone(),
            process_epoch: task.lifecycle.pi_process_epoch,
            lifecycle_revision,
        }
    }

    pub fn still_matches(&self, task: &Task) -> bool {
        task.lifecycle.generation == self.generation
            && task.lifecycle.fence == self.attempt_fence
            && task.lifecycle.pi_process_epoch == self.process_epoch
            && task
                .lifecycle
                .current_attempt
                .as_ref()
                .is_some_and(|attempt| {
                    attempt.id == self.attempt_id
                        && attempt.generation == self.generation
                        && attempt.fence == self.attempt_fence
                        && attempt.actor_id == self.owner_id
                })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProviderRetryPolicySnapshot {
    pub max_automatic_retries: u32,
    pub recovery_window_seconds: u64,
    pub base_seconds: u64,
    pub delay_cap_seconds: u64,
    pub jitter_divisor: u64,
}

impl From<&SourceProviderRetryConfig> for SourceProviderRetryPolicySnapshot {
    fn from(value: &SourceProviderRetryConfig) -> Self {
        Self {
            max_automatic_retries: value.max_automatic_retries,
            recovery_window_seconds: value.recovery_window_seconds,
            base_seconds: value.base_seconds,
            delay_cap_seconds: value.delay_cap_seconds,
            jitter_divisor: JITTER_DIVISOR,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProviderRecoveryV1 {
    pub schema: u32,
    pub episode_id: String,
    pub state: SourceProviderRecoveryState,
    pub goal_requirements_digest: String,
    pub completion_contract: CompletionContract,
    pub origin: SourceAttemptRef,
    pub last_failed: SourceAttemptRef,
    pub current_failure_id: String,
    pub failure_evidence_digest: String,
    pub operation_id: String,
    #[serde(default)]
    pub evidence_kind: FailureEvidenceKind,
    #[serde(default)]
    pub execution_outcome: ExecutionOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    pub exact_route: String,
    pub executor: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_revision: Option<String>,
    pub route_id: String,
    pub plan_id: String,
    pub first_failure_at: DateTime<Utc>,
    pub latest_failure_at: DateTime<Utc>,
    pub recovery_deadline_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_not_before: Option<DateTime<Utc>>,
    pub automatic_retries_used: u32,
    pub automatic_retry_limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<DateTime<Utc>>,
    pub policy_snapshot: SourceProviderRetryPolicySnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorized_generation: Option<u64>,
    pub reason_code: String,
    pub next_action: String,
}

impl SourceProviderRecoveryV1 {
    pub fn attempts_remaining(&self) -> u32 {
        self.automatic_retry_limit
            .saturating_sub(self.automatic_retries_used)
    }

    pub fn window_remaining_seconds(&self, now: DateTime<Utc>) -> u64 {
        self.recovery_deadline_at
            .signed_duration_since(now)
            .num_seconds()
            .max(0) as u64
    }

    pub fn retry_number(&self) -> Option<u32> {
        self.state
            .is_active()
            .then_some(self.automatic_retries_used.saturating_add(1))
    }

    pub fn pause(&mut self, reason: &str) {
        if matches!(
            self.state,
            SourceProviderRecoveryState::Backoff | SourceProviderRecoveryState::Authorized
        ) {
            self.state = SourceProviderRecoveryState::Paused;
            self.reason_code = reason.to_string();
            self.next_action =
                "resume the task; the original recovery window is not extended".into();
        }
    }

    pub fn cancel(&mut self, reason: &str) {
        if self.state.is_active() {
            self.state = SourceProviderRecoveryState::Cancelled;
            self.reason_code = reason.to_string();
            self.next_retry_at = None;
            self.next_action =
                "inspect retained work; only an explicit operator retry may create a new episode"
                    .into();
        }
    }

    pub fn recover(&mut self) {
        if matches!(
            self.state,
            SourceProviderRecoveryState::Authorized | SourceProviderRecoveryState::Running
        ) {
            self.state = SourceProviderRecoveryState::Recovered;
            self.reason_code = "source-provider-recovered".into();
            self.next_retry_at = None;
            self.next_action = "continue the existing completion/review flow".into();
        }
    }

    pub fn resume_policy(&mut self, task_status_failed: bool, generation: u64) {
        if self.state != SourceProviderRecoveryState::Paused
            || self.reason_code != "policy-disabled"
        {
            return;
        }
        if self.authorization_id.is_some() && self.authorized_generation == Some(generation) {
            self.state = SourceProviderRecoveryState::Authorized;
            self.reason_code = "automatic-retry-authorized".into();
            self.next_action = "the dispatcher will launch this exact recorded route".into();
        } else if task_status_failed {
            self.state = SourceProviderRecoveryState::Backoff;
            self.reason_code = "source-provider-retry-resumed".into();
            self.next_action =
                "wait for the bounded exact-route retry; no operator action is needed".into();
        } else {
            self.needs_attention(
                "stale-policy-pause",
                "reconcile the current lifecycle owner before any explicit retry",
            );
        }
    }

    pub fn needs_attention(&mut self, reason: &str, action: &str) {
        self.state = SourceProviderRecoveryState::NeedsAttention;
        self.reason_code = reason.to_string();
        self.next_retry_at = None;
        self.authorization_id = None;
        self.next_action = action.to_string();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureBinding {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub graph_id: String,
    pub goal_requirements_digest: String,
    pub exact_route: String,
    pub executor: String,
    pub model: String,
    /// Project authority revision used to construct the immutable plan. Older
    /// bindings deserialize as `None`; every new attempt writes the revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_revision: Option<String>,
    pub route_id: String,
    pub plan_id: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationOutcome {
    Ineligible,
    Duplicate,
    Scheduled,
    NeedsAttention,
}

const LAUNCH_BINDING_COMPONENT: &str = "source-provider-recovery";
const LAUNCH_BINDING_FILE: &str = "launch-binding.json";

fn graph_identity(dir: &Path) -> String {
    dir.canonicalize()
        .ok()
        .map(|path| {
            format!(
                "b3:{}",
                blake3::hash(path.to_string_lossy().as_bytes()).to_hex()
            )
        })
        .unwrap_or_else(|| {
            format!(
                "b3:{}",
                blake3::hash(dir.to_string_lossy().as_bytes()).to_hex()
            )
        })
}

pub fn build_launch_binding(dir: &Path, task: &Task, plan: &SpawnPlan) -> Result<FailureBinding> {
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("source-provider launch binding requires a current attempt")?;
    let route_id = crate::service::HealthRouteKey::from_spawn_plan(plan).id();
    let graph_id = graph_identity(dir);
    Ok(FailureBinding {
        task_id: task.id.clone(),
        generation: attempt.generation,
        attempt_id: attempt.id.clone(),
        attempt_fence: attempt.fence,
        graph_id,
        goal_requirements_digest: goal_requirements_digest(task),
        exact_route: format!("{}:{}", plan.executor.as_str(), plan.model.raw),
        executor: plan.executor.as_str().to_string(),
        model: plan.model.raw.clone(),
        config_revision: plan.config_revision.clone(),
        route_id: crate::dispatch::spawn_route_binding_id(&route_id),
        plan_id: crate::dispatch::spawn_plan_binding_id(plan, &route_id),
        operation_id: operation_id(task, attempt),
    })
}

pub fn persist_launch_binding(dir: &Path, task: &Task, plan: &SpawnPlan) -> Result<FailureBinding> {
    let binding = build_launch_binding(dir, task, plan)?;
    let key = crate::attempt_runtime::AttemptRuntimeKey::current(task)?;
    let component =
        crate::attempt_runtime::component_for_write(dir, &key, LAUNCH_BINDING_COMPONENT)?;
    std::fs::create_dir_all(&component)?;
    let path = component.join(LAUNCH_BINDING_FILE);
    let bytes = serde_json::to_vec_pretty(&binding)?;
    match crate::atomic_file::write_atomic_create_new(&path, &bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = std::fs::read(&path)?;
            if existing != bytes {
                bail!(
                    "source-provider launch binding changed for exact attempt at {}; evidence preserved",
                    path.display()
                );
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(binding)
}

pub fn load_launch_binding(dir: &Path, task: &Task) -> Result<FailureBinding> {
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("source-provider recovery requires a current attempt")?;
    let key = crate::attempt_runtime::AttemptRuntimeKey::for_attempt(task, attempt);
    let component = crate::attempt_runtime::resolve_component(dir, &key, LAUNCH_BINDING_COMPONENT)?
        .context("exact attempt has no persisted source-provider launch binding")?;
    let binding: FailureBinding =
        serde_json::from_slice(&std::fs::read(component.join(LAUNCH_BINDING_FILE))?)?;
    if binding.task_id != task.id
        || binding.generation != attempt.generation
        || binding.attempt_id != attempt.id
        || binding.attempt_fence != attempt.fence
        || binding.graph_id != graph_identity(dir)
        || binding.operation_id != operation_id(task, attempt)
        || binding.goal_requirements_digest != goal_requirements_digest(task)
    {
        bail!("persisted source-provider launch binding does not match the exact failed attempt");
    }
    Ok(binding)
}

pub fn goal_requirements_digest(task: &Task) -> String {
    let value = serde_json::json!({
        "title": task.title,
        "description": task.description,
        "completion_contract": task.completion_contract,
        "after": task.after,
        "requires": task.requires,
        "skills": task.skills,
        "inputs": task.inputs,
        "deliverables": task.deliverables,
        // Task-local dispatch inputs are part of the immutable retry source.
        // This makes an operator route/profile edit race fail closed at the
        // final launch boundary without reconstructing mutable global config.
        "profile": task.profile,
        "agent": task.agent,
        "model": task.model,
        "reasoning": task.reasoning,
        "provider": task.provider,
        "endpoint": task.endpoint,
        // These do not participate in SpawnPlan identity, but they change the
        // prompt, tool/capability surface, or execution deadline. A retry must
        // never silently inherit edits made while the original source is in
        // backoff.
        "context_scope": task.context_scope,
        "exec_mode": task.exec_mode,
        "exec": task.exec,
        "timeout": task.timeout,
        "validation_commands": crate::completion_validation::configured_validation_commands(task),
        "completion_repair_policy": task.completion_repair_policy,
    });
    format!(
        "b3:{}",
        blake3::hash(&crate::identity::canonical_json(&value)).to_hex()
    )
}

pub fn operation_id(task: &Task, attempt: &AttemptRef) -> String {
    digest(&[
        "wg-source-provider-operation-v1",
        &task.id,
        &attempt.generation.to_string(),
        &attempt.id,
        &attempt.fence.to_string(),
        &attempt.actor_id,
    ])
}

pub fn failure_id(task: &Task, attempt: &AttemptRef, binding: &FailureBinding) -> String {
    debug_assert_eq!(binding.task_id, task.id);
    debug_assert_eq!(binding.generation, attempt.generation);
    debug_assert_eq!(binding.attempt_id, attempt.id);
    debug_assert_eq!(binding.attempt_fence, attempt.fence);
    digest(&[
        "wg-source-provider-failure-v1",
        &binding.graph_id,
        &task.id,
        &attempt.generation.to_string(),
        &attempt.id,
        &attempt.fence.to_string(),
        &binding.operation_id,
        &binding.route_id,
        &binding.plan_id,
    ])
}

pub fn authorization_id(episode_id: &str, failure_id: &str, retry_number: u32) -> String {
    digest(&[
        "wg-source-provider-retry-authorization-v1",
        episode_id,
        failure_id,
        &retry_number.to_string(),
    ])
}

fn digest(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("b3:{}", hasher.finalize().to_hex())
}

fn evidence_rank(kind: FailureEvidenceKind) -> u8 {
    match kind {
        FailureEvidenceKind::ProviderEnvelope => 5,
        FailureEvidenceKind::HttpResponse => 4,
        FailureEvidenceKind::TransportError => 3,
        FailureEvidenceKind::ProcessOutcome => 2,
        FailureEvidenceKind::LegacyText => 1,
        FailureEvidenceKind::Unknown => 0,
    }
}

fn classification_conflicts(signal: &FailureSignal) -> bool {
    let status_class = signal.http_status.map(|status| match status {
        401 | 403 => FailureReason::Auth,
        402 => FailureReason::CreditExhausted,
        429 => FailureReason::RateLimit,
        500 | 502 => FailureReason::Transient5xx,
        503 => FailureReason::ProviderUnavailable,
        504 => FailureReason::Timeout,
        529 => FailureReason::ProviderOverloaded,
        400..=499 => FailureReason::Hard,
        _ => FailureReason::Unknown,
    });
    status_class.is_some_and(|status_reason| {
        let transient = |reason| {
            matches!(
                reason,
                FailureReason::RateLimit
                    | FailureReason::Transient5xx
                    | FailureReason::Timeout
                    | FailureReason::ProviderUnavailable
                    | FailureReason::ProviderOverloaded
            )
        };
        status_reason != FailureReason::Unknown
            && signal.reason != FailureReason::Unknown
            && status_reason != signal.reason
            && !(transient(status_reason) && transient(signal.reason))
    })
}

fn is_direct_evidence(kind: FailureEvidenceKind) -> bool {
    matches!(
        kind,
        FailureEvidenceKind::HttpResponse
            | FailureEvidenceKind::ProviderEnvelope
            | FailureEvidenceKind::TransportError
    )
}

fn direct_transient_class(signal: &FailureSignal) -> bool {
    let direct = is_direct_evidence(signal.evidence_kind);
    let eligible_reason = matches!(
        signal.reason,
        FailureReason::RateLimit
            | FailureReason::ProviderUnavailable
            | FailureReason::ProviderOverloaded
            | FailureReason::Transient5xx
    );
    let eligible_status = signal
        .http_status
        .is_some_and(|status| matches!(status, 429 | 500 | 502 | 503 | 504 | 529));
    direct
        && (eligible_reason || eligible_status)
        && !matches!(signal.http_status, Some(401..=403))
        && !classification_conflicts(signal)
}

pub fn direct_transient(signal: &FailureSignal) -> bool {
    direct_transient_class(signal) && signal.execution_outcome.is_safe_to_replay()
}

fn authoritative_retry_after(
    signal: &FailureSignal,
    observed_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if !matches!(
        signal.evidence_kind,
        FailureEvidenceKind::HttpResponse | FailureEvidenceKind::ProviderEnvelope
    ) {
        return None;
    }
    let seconds = signal.retry_after_secs?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let ceil = seconds.ceil();
    if ceil > i64::MAX as f64 {
        return None;
    }
    observed_at.checked_add_signed(chrono::Duration::seconds(ceil as i64))
}

fn signal_observed_at(signal: &FailureSignal, fallback: DateTime<Utc>) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(signal.detected_at_ms)
        .single()
        .unwrap_or(fallback)
}

fn evidence_digest(signal: &FailureSignal) -> String {
    let safe = serde_json::json!({
        "kind": signal.evidence_kind,
        "reason": signal.reason,
        "http_status": signal.http_status,
        "error_type": signal.error_type,
        "transport_code": signal.transport_code,
        "provider_request_id": signal.provider_request_id,
        "execution_outcome": signal.execution_outcome,
    });
    format!(
        "b3:{}",
        blake3::hash(&crate::identity::canonical_json(&safe)).to_hex()
    )
}

fn retry_delay(record: &SourceProviderRecoveryV1) -> u64 {
    let r = record.automatic_retries_used;
    let shift = r.min(63);
    let raw = record
        .policy_snapshot
        .base_seconds
        .saturating_mul(1u64 << shift)
        .min(record.policy_snapshot.delay_cap_seconds);
    let jitter_window = raw / record.policy_snapshot.jitter_divisor.max(1);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"wg-source-provider-retry-jitter-v1");
    hasher.update(record.episode_id.as_bytes());
    hasher.update(record.current_failure_id.as_bytes());
    hasher.update(&u64::from(r).to_le_bytes());
    let bytes = hasher.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&bytes.as_bytes()[..8]);
    let jitter = u64::from_le_bytes(first) % jitter_window.saturating_add(1);
    raw.saturating_add(jitter)
        .min(record.policy_snapshot.delay_cap_seconds)
}

fn schedule(record: &mut SourceProviderRecoveryV1) -> ObservationOutcome {
    record.authorization_id = None;
    record.authorized_generation = None;
    if record.automatic_retries_used >= record.automatic_retry_limit {
        record.needs_attention(
            "automatic-retries-exhausted",
            "inspect this task, then explicitly run `wg retry TASK --reason <WHY>` only if replay is safe",
        );
        return ObservationOutcome::NeedsAttention;
    }
    let delay = retry_delay(record);
    let computed = record
        .latest_failure_at
        .checked_add_signed(chrono::Duration::seconds(delay.min(i64::MAX as u64) as i64))
        .unwrap_or(DateTime::<Utc>::MAX_UTC);
    let candidate = record
        .retry_after_not_before
        .map_or(computed, |retry_after| retry_after.max(computed));
    if candidate > record.recovery_deadline_at {
        let reason = if record
            .retry_after_not_before
            .is_some_and(|retry_after| retry_after > record.recovery_deadline_at)
        {
            "retry-after-exceeds-window"
        } else {
            "recovery-window-expired"
        };
        record.needs_attention(
            reason,
            "inspect retained work, then explicitly run `wg retry TASK --reason <WHY>` only if replay is safe",
        );
        return ObservationOutcome::NeedsAttention;
    }
    record.state = SourceProviderRecoveryState::Backoff;
    record.reason_code = concise_reason_code(record);
    record.next_retry_at = Some(candidate);
    record.next_action =
        "wait for the bounded exact-route retry; no operator action is needed".into();
    ObservationOutcome::Scheduled
}

fn concise_reason_code(record: &SourceProviderRecoveryV1) -> String {
    if record.reason_code.starts_with("direct-provider-") {
        record.reason_code.clone()
    } else {
        "transient-source-provider".into()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn observe_failure(
    task: &mut Task,
    policy: &SourceProviderRetryConfig,
    signal: &FailureSignal,
    attempt: &AttemptRef,
    accepted_lifecycle_revision: u64,
    binding: &FailureBinding,
    now: DateTime<Utc>,
) -> ObservationOutcome {
    let current_failure_id = failure_id(task, attempt, binding);
    let observed_at = signal_observed_at(signal, now);
    let retry_after = authoritative_retry_after(signal, observed_at);
    if let Some(record) = task.source_provider_recovery.as_mut()
        && record.current_failure_id == current_failure_id
    {
        // Legacy/process observations can fold under an already accepted
        // direct terminal record but can neither weaken it nor alter timing.
        if !is_direct_evidence(signal.evidence_kind) {
            return ObservationOutcome::Duplicate;
        }
        let raised_retry_after = match (record.retry_after_not_before, retry_after) {
            (Some(old), Some(new)) if new > old => {
                record.retry_after_not_before = Some(new);
                true
            }
            (None, Some(new)) => {
                record.retry_after_not_before = Some(new);
                true
            }
            _ => false,
        };
        let at_least_as_strong =
            evidence_rank(signal.evidence_kind) >= evidence_rank(record.evidence_kind);
        if !direct_transient_class(signal)
            || (at_least_as_strong && !signal.execution_outcome.is_safe_to_replay())
        {
            let (reason, action) = ineligible_attention(signal);
            record.needs_attention(reason, action);
            return ObservationOutcome::NeedsAttention;
        }
        if at_least_as_strong && signal.execution_outcome.is_safe_to_replay() {
            record.failure_evidence_digest = evidence_digest(signal);
            record.evidence_kind = signal.evidence_kind;
            record.execution_outcome = signal.execution_outcome;
            record.provider_request_id = signal.provider_request_id.clone();
            record.transport_code = signal.transport_code.clone();
            record.http_status = signal.http_status;
        }
        if raised_retry_after {
            return if record.state == SourceProviderRecoveryState::Backoff {
                schedule(record)
            } else if matches!(
                record.state,
                SourceProviderRecoveryState::Authorized | SourceProviderRecoveryState::Running
            ) {
                record.needs_attention(
                    "retry-after-arrived-after-authorization",
                    "reconcile whether the authorized retry contacted the provider before any explicit retry",
                );
                ObservationOutcome::NeedsAttention
            } else {
                ObservationOutcome::Duplicate
            };
        }
        return ObservationOutcome::Duplicate;
    }

    let failed_ref = SourceAttemptRef::from_task(task, attempt, accepted_lifecycle_revision);

    if let Some(record) = task.source_provider_recovery.as_mut()
        && record.state.is_active()
    {
        if record.authorized_generation != Some(attempt.generation)
            || record.goal_requirements_digest != binding.goal_requirements_digest
            || record.completion_contract != task.completion_contract
            || record.route_id != binding.route_id
            || record.plan_id != binding.plan_id
            || record.executor != binding.executor
            || record.model != binding.model
            || record.config_revision != binding.config_revision
        {
            record.needs_attention(
                "ambiguous-or-stale-execution",
                "reconcile the exact recorded operation before any explicit retry",
            );
            return ObservationOutcome::NeedsAttention;
        }
        record.last_failed = failed_ref;
        record.current_failure_id = current_failure_id;
        record.failure_evidence_digest = evidence_digest(signal);
        record.operation_id = binding.operation_id.clone();
        record.evidence_kind = signal.evidence_kind;
        record.execution_outcome = signal.execution_outcome;
        record.provider_request_id = signal.provider_request_id.clone();
        record.transport_code = signal.transport_code.clone();
        record.http_status = signal.http_status;
        record.latest_failure_at = observed_at;
        record.retry_after_not_before = match (record.retry_after_not_before, retry_after) {
            (Some(old), Some(new)) => Some(old.max(new)),
            (old, new) => old.or(new),
        };
        if !direct_transient(signal) {
            let (reason, action) = ineligible_attention(signal);
            record.needs_attention(reason, action);
            return ObservationOutcome::NeedsAttention;
        }
        return schedule(record);
    }

    if !policy.enabled || policy.max_automatic_retries == 0 || !direct_transient(signal) {
        return ObservationOutcome::Ineligible;
    }

    let policy_snapshot = SourceProviderRetryPolicySnapshot::from(policy);
    let deadline = observed_at
        .checked_add_signed(chrono::Duration::seconds(
            policy_snapshot.recovery_window_seconds.min(i64::MAX as u64) as i64,
        ))
        .unwrap_or(DateTime::<Utc>::MAX_UTC);
    let first_failure_id = current_failure_id.clone();
    let episode_id = digest(&[
        "wg-source-provider-recovery-episode-v1",
        &binding.graph_id,
        &task.id,
        &binding.goal_requirements_digest,
        &task.completion_contract.to_string(),
        &binding.route_id,
        &binding.plan_id,
        &first_failure_id,
    ]);
    let reason_code = match signal.http_status {
        Some(status) => format!("direct-provider-http-{status}"),
        None => format!("direct-provider-{}", signal.reason.as_str()),
    };
    let mut record = SourceProviderRecoveryV1 {
        schema: SOURCE_PROVIDER_RECOVERY_SCHEMA,
        episode_id,
        state: SourceProviderRecoveryState::Backoff,
        goal_requirements_digest: binding.goal_requirements_digest.clone(),
        completion_contract: task.completion_contract,
        origin: failed_ref.clone(),
        last_failed: failed_ref,
        current_failure_id,
        failure_evidence_digest: evidence_digest(signal),
        operation_id: binding.operation_id.clone(),
        evidence_kind: signal.evidence_kind,
        execution_outcome: signal.execution_outcome,
        provider_request_id: signal.provider_request_id.clone(),
        transport_code: signal.transport_code.clone(),
        http_status: signal.http_status,
        exact_route: binding.exact_route.clone(),
        executor: binding.executor.clone(),
        model: binding.model.clone(),
        config_revision: binding.config_revision.clone(),
        route_id: binding.route_id.clone(),
        plan_id: binding.plan_id.clone(),
        first_failure_at: observed_at,
        latest_failure_at: observed_at,
        recovery_deadline_at: deadline,
        retry_after_not_before: retry_after,
        automatic_retries_used: 0,
        automatic_retry_limit: policy_snapshot.max_automatic_retries,
        next_retry_at: None,
        policy_snapshot,
        authorization_id: None,
        authorized_generation: None,
        reason_code,
        next_action: String::new(),
    };
    let outcome = schedule(&mut record);
    task.source_provider_recovery = Some(record);
    outcome
}

/// Fold all exact-attempt direct observations in deterministic precedence
/// order. A lower-precedence unsafe observation may be superseded by stronger
/// safe proof, while same/higher-precedence unsafe or contradictory evidence
/// always lands after a safe observation and fails the episode closed.
#[allow(clippy::too_many_arguments)]
pub fn observe_failures(
    task: &mut Task,
    policy: &SourceProviderRetryConfig,
    signals: &[FailureSignal],
    attempt: &AttemptRef,
    accepted_lifecycle_revision: u64,
    binding: &FailureBinding,
    now: DateTime<Utc>,
) -> ObservationOutcome {
    let mut ordered = signals.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|signal| {
        (
            evidence_rank(signal.evidence_kind),
            !direct_transient(signal),
            evidence_digest(signal),
        )
    });
    let mut aggregate = ObservationOutcome::Ineligible;
    for signal in ordered {
        let outcome = observe_failure(
            task,
            policy,
            signal,
            attempt,
            accepted_lifecycle_revision,
            binding,
            now,
        );
        if outcome != ObservationOutcome::Ineligible {
            aggregate = outcome;
        }
    }
    aggregate
}

fn ineligible_attention(signal: &FailureSignal) -> (&'static str, &'static str) {
    if classification_conflicts(signal) {
        (
            "ambiguous-execution",
            "reconcile conflicting direct provider evidence before any explicit retry",
        )
    } else if matches!(signal.reason, FailureReason::Auth)
        || matches!(signal.http_status, Some(401 | 403))
    {
        (
            "auth-config",
            "fix or authenticate the exact route, then explicitly run `wg retry TASK --reason <WHY>`",
        )
    } else if matches!(signal.reason, FailureReason::CreditExhausted)
        || signal.http_status == Some(402)
    {
        (
            "credit-exhausted",
            "restore provider credit, then explicitly run `wg retry TASK --reason <WHY>`",
        )
    } else if !signal.execution_outcome.is_safe_to_replay()
        || matches!(signal.reason, FailureReason::Timeout)
        || signal.evidence_kind == FailureEvidenceKind::TransportError
    {
        (
            "ambiguous-execution",
            "reconcile the exact recorded operation before any explicit retry",
        )
    } else {
        (
            "non-transient-source-failure",
            "inspect retained evidence; automation will not retry this failure",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::plan::ExecutorKind;
    use crate::graph::{ExecutionOutcome, FailureEvidenceKind, Status};
    use crate::lifecycle::{AttemptDisposition, AttemptRef};

    fn at(second: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(second, 0).unwrap()
    }

    fn task_with_attempt() -> Task {
        let mut task = Task {
            id: "source".into(),
            title: "source".into(),
            status: Status::Failed,
            ..Task::default()
        };
        task.lifecycle.current_attempt = Some(AttemptRef {
            id: "attempt-0-1".into(),
            generation: 0,
            fence: 1,
            actor_id: "agent-1".into(),
            disposition: Some(AttemptDisposition::Failed),
        });
        task.lifecycle.fence = 1;
        task
    }

    fn policy() -> SourceProviderRetryConfig {
        SourceProviderRetryConfig {
            enabled: true,
            max_automatic_retries: 3,
            recovery_window_seconds: 900,
            base_seconds: 30,
            delay_cap_seconds: 300,
        }
    }

    fn binding(task: &Task) -> FailureBinding {
        FailureBinding {
            task_id: task.id.clone(),
            generation: 0,
            attempt_id: "attempt-0-1".into(),
            attempt_fence: 1,
            graph_id: "graph".into(),
            goal_requirements_digest: goal_requirements_digest(task),
            exact_route: "pi:openrouter:test/model".into(),
            executor: "pi".into(),
            model: "openrouter:test/model".into(),
            config_revision: Some("b3:test-revision".into()),
            route_id: "route".into(),
            plan_id: "plan".into(),
            operation_id: "operation-1".into(),
        }
    }

    fn direct(status: u16, now: i64) -> FailureSignal {
        FailureSignal {
            reason: if status == 429 {
                FailureReason::RateLimit
            } else {
                FailureReason::Transient5xx
            },
            confidence: 1.0,
            http_status: Some(status),
            executor: ExecutorKind::Pi,
            detected_at_ms: now * 1000,
            evidence_kind: FailureEvidenceKind::ProviderEnvelope,
            execution_outcome: ExecutionOutcome::DefinitiveFailure,
            ..FailureSignal::default()
        }
    }

    #[test]
    fn disabled_is_fail_stop_and_text_only_never_enrolls() {
        let mut task = task_with_attempt();
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let mut disabled = policy();
        disabled.enabled = false;
        let evidence = direct(429, 100);
        let route = binding(&task);
        assert_eq!(
            observe_failure(
                &mut task,
                &disabled,
                &evidence,
                &attempt,
                1,
                &route,
                at(100)
            ),
            ObservationOutcome::Ineligible
        );
        assert!(task.source_provider_recovery.is_none());

        let mut text = direct(429, 100);
        text.evidence_kind = FailureEvidenceKind::LegacyText;
        let route = binding(&task);
        assert_eq!(
            observe_failure(&mut task, &policy(), &text, &attempt, 1, &route, at(100)),
            ObservationOutcome::Ineligible
        );
        assert!(task.source_provider_recovery.is_none());
    }

    #[test]
    fn exact_window_boundary_and_retry_after_lower_bound() {
        let mut task = task_with_attempt();
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let mut evidence = direct(429, 100);
        evidence.retry_after_secs = Some(900.0);
        let route = binding(&task);
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &evidence,
                &attempt,
                1,
                &route,
                at(100)
            ),
            ObservationOutcome::Scheduled
        );
        let record = task.source_provider_recovery.as_ref().unwrap();
        assert_eq!(record.next_retry_at, Some(at(1000)));
        assert_eq!(record.recovery_deadline_at, at(1000));

        let mut beyond = task_with_attempt();
        let attempt = beyond.lifecycle.current_attempt.clone().unwrap();
        evidence.retry_after_secs = Some(900.01);
        let route = binding(&beyond);
        assert_eq!(
            observe_failure(
                &mut beyond,
                &policy(),
                &evidence,
                &attempt,
                1,
                &route,
                at(100)
            ),
            ObservationOutcome::NeedsAttention
        );
        let record = beyond.source_provider_recovery.unwrap();
        assert_eq!(record.reason_code, "retry-after-exceeds-window");
        assert!(record.next_retry_at.is_none());
    }

    #[test]
    fn duplicate_evidence_and_restart_do_not_reset_budget_or_jitter() {
        let mut task = task_with_attempt();
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let evidence = direct(503, 100);
        let binding = binding(&task);
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &evidence,
                &attempt,
                2,
                &binding,
                at(100)
            ),
            ObservationOutcome::Scheduled
        );
        let before = task.source_provider_recovery.clone().unwrap();
        let mut stronger = evidence.clone();
        stronger.retry_after_secs = Some(500.0);
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &stronger,
                &attempt,
                2,
                &binding,
                at(100)
            ),
            ObservationOutcome::Scheduled
        );
        let after = task.source_provider_recovery.as_ref().unwrap();
        assert_eq!(before.episode_id, after.episode_id);
        assert_eq!(before.automatic_retries_used, after.automatic_retries_used);
        assert_eq!(after.next_retry_at, Some(at(600)));

        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &stronger,
                &attempt,
                2,
                &binding,
                at(500)
            ),
            ObservationOutcome::Duplicate
        );
        let retained = task.source_provider_recovery.clone().unwrap();
        let mut legacy = stronger.clone();
        legacy.evidence_kind = FailureEvidenceKind::LegacyText;
        legacy.retry_after_secs = Some(800.0);
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &legacy,
                &attempt,
                2,
                &binding,
                at(600)
            ),
            ObservationOutcome::Duplicate
        );
        assert_eq!(task.source_provider_recovery.as_ref().unwrap(), &retained);
    }

    #[test]
    fn contradictory_duplicate_evidence_escalates_without_spending_budget() {
        let mut task = task_with_attempt();
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let evidence = direct(503, 100);
        let route = binding(&task);
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &evidence,
                &attempt,
                2,
                &route,
                at(100)
            ),
            ObservationOutcome::Scheduled
        );
        let episode = task
            .source_provider_recovery
            .as_ref()
            .unwrap()
            .episode_id
            .clone();
        let mut contradictory = evidence.clone();
        contradictory.execution_outcome = ExecutionOutcome::Ambiguous;
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &contradictory,
                &attempt,
                2,
                &route,
                at(101)
            ),
            ObservationOutcome::NeedsAttention
        );
        let record = task.source_provider_recovery.as_ref().unwrap();
        assert_eq!(record.episode_id, episode);
        assert_eq!(record.automatic_retries_used, 0);
        assert_eq!(record.state, SourceProviderRecoveryState::NeedsAttention);
    }

    #[test]
    fn contradictory_new_episode_is_order_independent() {
        let safe = direct(503, 100);
        let mut ambiguous = safe.clone();
        ambiguous.execution_outcome = ExecutionOutcome::Ambiguous;

        for signals in [
            vec![safe.clone(), ambiguous.clone()],
            vec![ambiguous.clone(), safe.clone()],
        ] {
            let mut task = task_with_attempt();
            let attempt = task.lifecycle.current_attempt.clone().unwrap();
            let route = binding(&task);
            assert_eq!(
                observe_failures(&mut task, &policy(), &signals, &attempt, 2, &route, at(100)),
                ObservationOutcome::NeedsAttention
            );
            let record = task.source_provider_recovery.as_ref().unwrap();
            assert_eq!(record.state, SourceProviderRecoveryState::NeedsAttention);
            assert_eq!(record.reason_code, "ambiguous-execution");
            assert_eq!(record.automatic_retries_used, 0);
        }
    }

    #[test]
    fn policy_pause_resumes_without_backfill_or_budget_reset() {
        let mut task = task_with_attempt();
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let route = binding(&task);
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &direct(503, 100),
                &attempt,
                1,
                &route,
                at(100)
            ),
            ObservationOutcome::Scheduled
        );
        let record = task.source_provider_recovery.as_mut().unwrap();
        record.automatic_retries_used = 1;
        let episode = record.episode_id.clone();
        record.pause("policy-disabled");
        record.resume_policy(true, 0);
        assert_eq!(record.state, SourceProviderRecoveryState::Backoff);
        assert_eq!(record.automatic_retries_used, 1);
        assert_eq!(record.episode_id, episode);

        record.pause("task-paused");
        record.resume_policy(true, 0);
        assert_eq!(record.state, SourceProviderRecoveryState::Paused);
        assert_eq!(record.reason_code, "task-paused");
    }

    #[test]
    fn explicit_retry_intent_atomically_clears_episode() {
        let mut task = task_with_attempt();
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let route = binding(&task);
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &direct(503, 100),
                &attempt,
                0,
                &route,
                at(100),
            ),
            ObservationOutcome::Scheduled
        );
        let intent =
            crate::lifecycle::ReopenIntent::for_task(&task, "retry", false, true, "explicit retry");
        let request = crate::lifecycle::TransitionRequest::new(
            crate::lifecycle::TransitionKind::ReopenRequested { intent },
            crate::lifecycle::LifecycleActor::operator("operator"),
            "explicit_retry",
            "test-explicit-retry-clears-recovery",
        )
        .expecting(crate::lifecycle::FenceExpectation::current(&task));
        crate::lifecycle::apply_transition(&mut task, request).unwrap();
        assert!(task.source_provider_recovery.is_none());
    }

    #[test]
    fn budget_is_independent_and_fourth_retry_is_never_scheduled() {
        let mut task = task_with_attempt();
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let evidence = direct(503, 100);
        let route = binding(&task);
        assert_eq!(
            observe_failure(
                &mut task,
                &policy(),
                &evidence,
                &attempt,
                2,
                &route,
                at(100)
            ),
            ObservationOutcome::Scheduled
        );
        let record = task.source_provider_recovery.as_mut().unwrap();
        record.automatic_retries_used = 3;
        record.current_failure_id = "new-failure".into();
        assert_eq!(schedule(record), ObservationOutcome::NeedsAttention);
        assert_eq!(record.reason_code, "automatic-retries-exhausted");
    }

    #[test]
    fn safe_pre_request_transport_outage_is_eligible() {
        let mut task = task_with_attempt();
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let signal = FailureSignal {
            reason: FailureReason::ProviderUnavailable,
            confidence: 1.0,
            executor: ExecutorKind::Native,
            detected_at_ms: 100_000,
            evidence_kind: FailureEvidenceKind::TransportError,
            execution_outcome: ExecutionOutcome::NotSent,
            transport_code: Some("connect-before-request-no-prior-effects".into()),
            ..FailureSignal::default()
        };
        let route = binding(&task);
        assert_eq!(
            observe_failure(&mut task, &policy(), &signal, &attempt, 1, &route, at(100)),
            ObservationOutcome::Scheduled
        );
        let recovery = task.source_provider_recovery.unwrap();
        assert_eq!(recovery.evidence_kind, FailureEvidenceKind::TransportError);
        assert_eq!(recovery.execution_outcome, ExecutionOutcome::NotSent);
    }

    #[test]
    fn auth_credit_hard_timeout_unknown_and_ambiguous_transport_are_never_automatic() {
        for (reason, status, kind, outcome) in [
            (
                FailureReason::Auth,
                Some(401),
                FailureEvidenceKind::HttpResponse,
                ExecutionOutcome::DefinitiveFailure,
            ),
            (
                FailureReason::Auth,
                Some(403),
                FailureEvidenceKind::HttpResponse,
                ExecutionOutcome::DefinitiveFailure,
            ),
            (
                FailureReason::CreditExhausted,
                Some(402),
                FailureEvidenceKind::ProviderEnvelope,
                ExecutionOutcome::DefinitiveFailure,
            ),
            (
                FailureReason::Hard,
                Some(400),
                FailureEvidenceKind::ProviderEnvelope,
                ExecutionOutcome::DefinitiveFailure,
            ),
            (
                FailureReason::HardTimeout,
                None,
                FailureEvidenceKind::ProcessOutcome,
                ExecutionOutcome::Ambiguous,
            ),
            (
                FailureReason::Unknown,
                None,
                FailureEvidenceKind::Unknown,
                ExecutionOutcome::Ambiguous,
            ),
            (
                FailureReason::Timeout,
                None,
                FailureEvidenceKind::TransportError,
                ExecutionOutcome::Ambiguous,
            ),
            (
                FailureReason::ProviderUnavailable,
                None,
                FailureEvidenceKind::TransportError,
                ExecutionOutcome::Ambiguous,
            ),
        ] {
            let mut task = task_with_attempt();
            let attempt = task.lifecycle.current_attempt.clone().unwrap();
            let signal = FailureSignal {
                reason,
                confidence: 1.0,
                http_status: status,
                executor: ExecutorKind::Pi,
                detected_at_ms: 100_000,
                evidence_kind: kind,
                execution_outcome: outcome,
                ..FailureSignal::default()
            };
            let route = binding(&task);
            assert_eq!(
                observe_failure(&mut task, &policy(), &signal, &attempt, 1, &route, at(100)),
                ObservationOutcome::Ineligible
            );
            assert!(task.source_provider_recovery.is_none());
        }
    }

    #[test]
    fn stale_process_epoch_cannot_reuse_failure_authority() {
        let mut task = task_with_attempt();
        task.lifecycle.pi_process_epoch = 3;
        let attempt = task.lifecycle.current_attempt.clone().unwrap();
        let accepted = SourceAttemptRef::from_task(&task, &attempt, task.lifecycle.revision);
        assert!(accepted.still_matches(&task));

        task.lifecycle.pi_process_epoch = 4;
        assert!(!accepted.still_matches(&task));
    }

    #[test]
    fn execution_semantic_edits_change_the_exact_retry_digest() {
        let task = task_with_attempt();
        let original = goal_requirements_digest(&task);
        let mutations: [fn(&mut Task); 4] = [
            |task: &mut Task| task.context_scope = Some("clean".into()),
            |task: &mut Task| task.exec_mode = Some("light".into()),
            |task: &mut Task| task.exec = Some("printf changed".into()),
            |task: &mut Task| task.timeout = Some("5m".into()),
        ];
        for mutate in mutations {
            let mut changed = task.clone();
            mutate(&mut changed);
            assert_ne!(goal_requirements_digest(&changed), original);
        }
    }
}
