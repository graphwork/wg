//! Read-only selected-task live-progress projection for TUI/operator surfaces.
//!
//! This module consumes the lifecycle, Pi-watchdog, and isolated-worktree
//! persisted projections. It never reads raw provider streams, scans a
//! worktree, or writes lifecycle state. Filesystem observations remain
//! corroborating `observed/unproven` evidence and never select a Pi phase.

use crate::graph::{Status, Task, TokenUsage};
use crate::pi_watchdog::{Classification, NativeActivityProjection, Phase as PiPhase, PiWatchdog};
use crate::worktree_observer::{
    DEFAULT_MAX_OBSERVED_ONLY_EXTENSION_SECS, DEFAULT_MEANINGFUL_SILENCE_SECS,
    DEFAULT_OBSERVED_ACTIVITY_GRACE_SECS, ObserverProjection, read_projection,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::path::Path;

pub const UNKNOWN: &str = "Unknown";
pub const RATE_WINDOW_SECS: i64 = 30;
const MAX_SAMPLES: usize = 32;
const MAX_SAFE_LABEL_CHARS: usize = 32;
const MAX_SAFE_PATH_CHARS: usize = 80;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LivePhase {
    #[default]
    Unknown,
    WaitingProvider,
    Thinking,
    Generating,
    Writing,
    Tool,
    Testing,
    WaitingUser,
    Suspect,
    Fencing,
    Resuming,
    Stalled,
    Done,
    Failed,
    Cancelled,
}

impl LivePhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::WaitingProvider => "Waiting provider",
            Self::Thinking => "Thinking",
            Self::Generating => "Generating",
            Self::Writing => "Writing",
            Self::Tool => "Tool",
            Self::Testing => "Testing",
            Self::WaitingUser => "Waiting user",
            Self::Suspect => "Suspect",
            Self::Fencing => "Fencing",
            Self::Resuming => "Resuming",
            Self::Stalled => "Stalled",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTuple {
    pub task: String,
    pub generation: u64,
    pub attempt: String,
    pub attempt_fence: u64,
    pub worktree_lease_epoch: u64,
    pub process_epoch: u32,
    pub continuation_epoch: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressEvidence {
    pub seq: u64,
    pub observed_at: i64,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeEvidence {
    pub seq: u64,
    pub observed_at: Option<i64>,
    pub path: String,
    pub changed_files: usize,
    pub byte_delta: i64,
    pub digest: String,
    pub quarantine: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UsageProjection {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub total: Option<u64>,
    pub pi_reported_cost: Option<String>,
    pub receipt_count: u64,
    pub possible_unattributed_cost: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchdogProjection {
    pub meaningful_silence_secs: u64,
    pub observed_activity_grace_secs: u64,
    pub max_observed_only_extension_secs: u64,
    pub proof_deadline: Option<i64>,
    pub observed_deadline: Option<i64>,
    pub observed_only_extension_secs: Option<u64>,
    pub cap_consumed_secs: Option<u64>,
    pub probe_grace_deadline: Option<i64>,
    pub continuation_epoch: u32,
    pub continuation_count: u32,
    pub continuation_budget: String,
    pub eligibility_or_hold_reason: String,
    pub safe_next_action: String,
}

impl Default for WatchdogProjection {
    fn default() -> Self {
        Self {
            meaningful_silence_secs: DEFAULT_MEANINGFUL_SILENCE_SECS,
            observed_activity_grace_secs: DEFAULT_OBSERVED_ACTIVITY_GRACE_SECS,
            max_observed_only_extension_secs: DEFAULT_MAX_OBSERVED_ONLY_EXTENSION_SECS,
            proof_deadline: None,
            observed_deadline: None,
            observed_only_extension_secs: None,
            cap_consumed_secs: None,
            probe_grace_deadline: None,
            continuation_epoch: 0,
            continuation_count: 0,
            continuation_budget: UNKNOWN.into(),
            eligibility_or_hold_reason: UNKNOWN.into(),
            safe_next_action: UNKNOWN.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSummary {
    pub disposition: LivePhase,
    pub accepted_at: String,
    pub last_phase: LivePhase,
    pub pi_progress: Option<ProgressEvidence>,
    pub worktree_activity: Option<WorktreeEvidence>,
    pub usage: UsageProjection,
    pub continuation_count: u32,
    pub late_write_evidence: bool,
    pub hold_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveProgressProjection {
    pub source: Option<SourceTuple>,
    pub phase: LivePhase,
    pub phase_at: Option<i64>,
    pub native_event_seq: u64,
    pub native_last_activity_at: Option<i64>,
    pub thinking_activity: bool,
    pub thinking_activity_seq: u64,
    pub thinking_tokens: Option<u64>,
    pub output_activity_seq: u64,
    pub output_tokens: Option<u64>,
    pub output_rate_milli_tok_per_sec: Option<u64>,
    pub rate_window_secs: i64,
    pub last_output_activity_at: Option<i64>,
    pub pi_progress: Option<ProgressEvidence>,
    pub worktree_activity: Option<WorktreeEvidence>,
    pub tool_label: String,
    pub tool_class: String,
    pub tool_progress: Option<u64>,
    pub tool_child_state: String,
    pub tool_receipt_state: String,
    pub usage: UsageProjection,
    pub watchdog: WatchdogProjection,
    pub observer_health: String,
    pub ignored_churn: String,
    pub watcher_overflows: u64,
    pub unstable_scans: u64,
    pub late_write_evidence: bool,
    pub terminal_summary: Option<TerminalSummary>,
}

impl Default for LiveProgressProjection {
    fn default() -> Self {
        Self {
            source: None,
            phase: LivePhase::Unknown,
            phase_at: None,
            native_event_seq: 0,
            native_last_activity_at: None,
            thinking_activity: false,
            thinking_activity_seq: 0,
            thinking_tokens: None,
            output_activity_seq: 0,
            output_tokens: None,
            output_rate_milli_tok_per_sec: None,
            rate_window_secs: RATE_WINDOW_SECS,
            last_output_activity_at: None,
            pi_progress: None,
            worktree_activity: None,
            tool_label: UNKNOWN.into(),
            tool_class: UNKNOWN.into(),
            tool_progress: None,
            tool_child_state: UNKNOWN.into(),
            tool_receipt_state: UNKNOWN.into(),
            usage: UsageProjection::default(),
            watchdog: WatchdogProjection::default(),
            observer_health: UNKNOWN.into(),
            ignored_churn: "{}".into(),
            watcher_overflows: 0,
            unstable_scans: 0,
            late_write_evidence: false,
            terminal_summary: None,
        }
    }
}

/// Canonical safe events accepted by the deterministic reducer. These events
/// intentionally cannot carry provider prose, reasoning text, prompts, file
/// content, or tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    ProviderRequest {
        seq: u64,
        at: i64,
    },
    ProviderResponse {
        seq: u64,
        at: i64,
    },
    Thinking {
        seq: u64,
        at: i64,
        tokens: Option<u64>,
    },
    Output {
        seq: u64,
        at: i64,
        tokens: Option<u64>,
    },
    Writing {
        seq: u64,
        at: i64,
    },
    Tool {
        seq: u64,
        at: i64,
        class: String,
        label: String,
        progress: Option<u64>,
    },
    ToolClosed {
        seq: u64,
    },
    TurnSettled {
        seq: u64,
    },
    WaitingUser {
        seq: u64,
        at: i64,
    },
    Control {
        seq: u64,
        at: i64,
        phase: LivePhase,
        reason: String,
    },
    ControlCleared {
        seq: u64,
    },
    Worktree {
        seq: u64,
        at: i64,
        path: String,
        changed_files: usize,
        byte_delta: i64,
        digest: String,
        quarantine: bool,
    },
    Usage {
        seq: u64,
        receipt: String,
        input: Option<u64>,
        output: Option<u64>,
        cache_read: Option<u64>,
        cache_write: Option<u64>,
        total: Option<u64>,
        cost: Option<String>,
    },
    Terminal {
        seq: u64,
        at: String,
        disposition: LivePhase,
    },
}

#[derive(Debug, Default)]
pub struct LiveProgressReducer {
    projection: LiveProgressProjection,
    ordinary_phase: LivePhase,
    control_phase: Option<LivePhase>,
    waiting_user: bool,
    last_pi_seq: u64,
    usage_receipts: BTreeSet<String>,
    numeric_samples: VecDeque<(i64, u64)>,
}

impl LiveProgressReducer {
    pub fn projection(&self) -> &LiveProgressProjection {
        &self.projection
    }

    pub fn apply(&mut self, event: LiveEvent) {
        if self.projection.terminal_summary.is_some() {
            if let LiveEvent::Worktree {
                seq,
                at,
                path,
                changed_files,
                byte_delta,
                digest,
                quarantine,
                ..
            } = event
            {
                if quarantine {
                    self.projection.late_write_evidence = true;
                    self.projection.worktree_activity = Some(WorktreeEvidence {
                        seq,
                        observed_at: Some(at),
                        path: safe_path(&path),
                        changed_files,
                        byte_delta,
                        digest: safe_digest(&digest),
                        quarantine,
                    });
                }
            }
            return;
        }
        match event {
            LiveEvent::ProviderRequest { seq, at } => {
                self.pi(seq, at, "provider-request", LivePhase::WaitingProvider)
            }
            LiveEvent::ProviderResponse { seq, at } => {
                self.pi(seq, at, "provider-response", LivePhase::Generating)
            }
            LiveEvent::Thinking { seq, at, tokens } => {
                self.projection.thinking_activity = true;
                self.projection.thinking_tokens = tokens;
                self.pi(seq, at, "thinking", LivePhase::Thinking);
            }
            LiveEvent::Output { seq, at, tokens } => {
                self.projection.last_output_activity_at = Some(at);
                self.projection.output_tokens = tokens;
                if let Some(tokens) = tokens {
                    self.push_numeric_sample(at, tokens);
                }
                self.pi(seq, at, "output", LivePhase::Generating);
            }
            LiveEvent::Writing { seq, at } => self.pi(seq, at, "write-receipt", LivePhase::Writing),
            LiveEvent::Tool {
                seq,
                at,
                class,
                label,
                progress,
            } => {
                self.projection.tool_class = safe_label(&class);
                self.projection.tool_label = safe_label(&label);
                self.projection.tool_progress = progress;
                let phase = match class.to_ascii_lowercase().as_str() {
                    "test" | "testing" => LivePhase::Testing,
                    "write" | "edit" | "writing" => LivePhase::Writing,
                    _ => LivePhase::Tool,
                };
                self.pi(seq, at, "tool-progress", phase);
            }
            LiveEvent::ToolClosed { seq } | LiveEvent::TurnSettled { seq } => {
                if seq >= self.last_pi_seq {
                    self.last_pi_seq = seq;
                    self.ordinary_phase = LivePhase::Unknown;
                    self.refresh_phase();
                }
            }
            LiveEvent::WaitingUser { seq, at } => {
                if seq >= self.last_pi_seq {
                    self.last_pi_seq = seq;
                    self.waiting_user = true;
                    self.projection.phase_at = Some(at);
                    self.refresh_phase();
                }
            }
            LiveEvent::Control {
                seq,
                at,
                phase,
                reason,
            } => {
                if seq >= self.last_pi_seq {
                    self.last_pi_seq = seq;
                    self.control_phase = Some(phase);
                    self.projection.phase_at = Some(at);
                    self.projection.watchdog.eligibility_or_hold_reason = safe_label(&reason);
                    self.refresh_phase();
                }
            }
            LiveEvent::ControlCleared { seq } => {
                if seq >= self.last_pi_seq {
                    self.last_pi_seq = seq;
                    self.control_phase = None;
                    self.refresh_phase();
                }
            }
            LiveEvent::Worktree {
                seq,
                at,
                path,
                changed_files,
                byte_delta,
                digest,
                quarantine,
            } => {
                if self
                    .projection
                    .worktree_activity
                    .as_ref()
                    .is_none_or(|old| seq > old.seq)
                {
                    self.projection.worktree_activity = Some(WorktreeEvidence {
                        seq,
                        observed_at: Some(at),
                        path: safe_path(&path),
                        changed_files,
                        byte_delta,
                        digest: safe_digest(&digest),
                        quarantine,
                    });
                    self.projection.late_write_evidence |= quarantine;
                }
            }
            LiveEvent::Usage {
                receipt,
                input,
                output,
                cache_read,
                cache_write,
                total,
                cost,
                ..
            } => {
                if self.usage_receipts.insert(receipt) {
                    add_opt(&mut self.projection.usage.input, input);
                    add_opt(&mut self.projection.usage.output, output);
                    add_opt(&mut self.projection.usage.cache_read, cache_read);
                    add_opt(&mut self.projection.usage.cache_write, cache_write);
                    add_opt(&mut self.projection.usage.total, total);
                    add_decimal(&mut self.projection.usage.pi_reported_cost, cost);
                    self.projection.usage.receipt_count += 1;
                }
            }
            LiveEvent::Terminal {
                at, disposition, ..
            } => {
                let last_phase = self.projection.phase;
                self.projection.phase = disposition;
                self.projection.terminal_summary = Some(TerminalSummary {
                    disposition,
                    accepted_at: safe_label_long(&at, 48),
                    last_phase,
                    pi_progress: self.projection.pi_progress.clone(),
                    worktree_activity: self.projection.worktree_activity.clone(),
                    usage: self.projection.usage.clone(),
                    continuation_count: self.projection.watchdog.continuation_count,
                    late_write_evidence: self.projection.late_write_evidence,
                    hold_reason: self.projection.watchdog.eligibility_or_hold_reason.clone(),
                });
            }
        }
    }

    fn pi(&mut self, seq: u64, at: i64, kind: &str, phase: LivePhase) {
        if seq < self.last_pi_seq {
            return;
        }
        self.last_pi_seq = seq;
        self.ordinary_phase = phase;
        self.projection.phase_at = Some(at);
        self.projection.pi_progress = Some(ProgressEvidence {
            seq,
            observed_at: at,
            kind: kind.into(),
        });
        self.refresh_phase();
    }

    fn refresh_phase(&mut self) {
        self.projection.phase = self.control_phase.unwrap_or(if self.waiting_user {
            LivePhase::WaitingUser
        } else {
            self.ordinary_phase
        });
    }

    fn push_numeric_sample(&mut self, at: i64, tokens: u64) {
        if self
            .numeric_samples
            .back()
            .is_some_and(|(_, old)| tokens < *old)
        {
            self.numeric_samples.clear();
        }
        if self
            .numeric_samples
            .back()
            .is_none_or(|sample| *sample != (at, tokens))
        {
            self.numeric_samples.push_back((at, tokens));
        }
        while self.numeric_samples.len() > MAX_SAMPLES {
            self.numeric_samples.pop_front();
        }
        while self
            .numeric_samples
            .front()
            .is_some_and(|(t, _)| at.saturating_sub(*t) > RATE_WINDOW_SECS)
        {
            self.numeric_samples.pop_front();
        }
        self.projection.output_rate_milli_tok_per_sec = rolling_rate(&self.numeric_samples);
    }
}

fn rolling_rate(samples: &VecDeque<(i64, u64)>) -> Option<u64> {
    let (first_at, first) = *samples.front()?;
    let (last_at, last) = *samples.back()?;
    let elapsed = last_at.checked_sub(first_at)?;
    if samples.len() < 2 || elapsed <= 0 || last < first {
        return None;
    }
    Some(last.saturating_sub(first).saturating_mul(1000) / elapsed as u64)
}

fn add_opt(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0).saturating_add(value));
    }
}

fn add_decimal(total: &mut Option<String>, value: Option<String>) {
    let Some(value) = value.and_then(|v| v.parse::<f64>().ok()) else {
        return;
    };
    let old = total
        .as_deref()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    *total = Some(format!("{:.6}", old + value));
}

fn safe_label(value: &str) -> String {
    safe_label_long(value, MAX_SAFE_LABEL_CHARS)
}
fn safe_path(value: &str) -> String {
    safe_label_long(value, MAX_SAFE_PATH_CHARS)
}
fn safe_digest(value: &str) -> String {
    let clean = value
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '-'))
        .take(24)
        .collect::<String>();
    if clean.is_empty() {
        UNKNOWN.into()
    } else {
        clean
    }
}
fn safe_label_long(value: &str, max: usize) -> String {
    if value.chars().any(char::is_control) {
        return format!("redacted:{}", &blake3::hash(value.as_bytes()).to_hex()[..8]);
    }
    let mut out = String::new();
    for c in value.chars() {
        if out.chars().count() >= max {
            break;
        }
        if c.is_ascii_alphanumeric()
            || matches!(
                c,
                ' ' | '/' | '.' | '_' | '-' | ':' | '+' | '@' | '=' | '(' | ')'
            )
            || (!c.is_control() && !c.is_ascii())
        {
            out.push(c);
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() { UNKNOWN.into() } else { out }
}

fn terminal_phase(status: Status) -> Option<LivePhase> {
    match status {
        Status::Done => Some(LivePhase::Done),
        Status::Failed | Status::Incomplete | Status::FailedPendingEval => Some(LivePhase::Failed),
        Status::Abandoned => Some(LivePhase::Cancelled),
        _ => None,
    }
}

fn control_phase(classification: Classification) -> Option<LivePhase> {
    match classification {
        Classification::StalledOperatorRequired | Classification::NeedsFinalization => {
            Some(LivePhase::Stalled)
        }
        Classification::Fencing => Some(LivePhase::Fencing),
        Classification::Resuming => Some(LivePhase::Resuming),
        Classification::Suspect | Classification::HardResumeEligible => Some(LivePhase::Suspect),
        _ => None,
    }
}

fn ordinary_phase(pi_phase: PiPhase, native: &NativeActivityProjection, kind: &str) -> LivePhase {
    if native
        .current_tool_class
        .as_deref()
        .is_some_and(|c| c == "test")
    {
        return LivePhase::Testing;
    }
    if native
        .current_tool_class
        .as_deref()
        .is_some_and(|c| matches!(c, "write" | "edit"))
    {
        return LivePhase::Writing;
    }
    if native.current_tool_label.is_some() || pi_phase == PiPhase::Tool {
        return LivePhase::Tool;
    }
    match kind {
        "thinking-delta" => LivePhase::Thinking,
        "token-delta" | "provider-response-start" => LivePhase::Generating,
        "tool-intent" | "tool-progress" => LivePhase::Tool,
        _ => match pi_phase {
            PiPhase::ProviderRequestInFlight => LivePhase::WaitingProvider,
            PiPhase::ProviderResponseStream => LivePhase::Generating,
            _ => LivePhase::Unknown,
        },
    }
}

fn worktree_evidence(observer: &ObserverProjection) -> Option<WorktreeEvidence> {
    let activity = observer.last_activity.as_ref();
    let late = observer.late_mutations.last();
    if let Some(late) = late {
        let path = late
            .changed_paths
            .first()
            .map(|p| p.path.as_str())
            .unwrap_or(UNKNOWN);
        return Some(WorktreeEvidence {
            seq: observer.content_seq,
            observed_at: Some(late.observed_at),
            path: safe_path(path),
            changed_files: late.changed_paths.len(),
            byte_delta: late.changed_paths.iter().map(|p| p.byte_delta).sum(),
            digest: safe_digest(&late.new_manifest_digest),
            quarantine: true,
        });
    }
    activity.map(|activity| WorktreeEvidence {
        seq: activity.content_seq,
        observed_at: Some(activity.observed_at),
        path: safe_path(
            activity
                .changed_paths
                .first()
                .map(|p| p.path.as_str())
                .unwrap_or(UNKNOWN),
        ),
        changed_files: activity.changed_paths.len(),
        byte_delta: activity.changed_paths.iter().map(|p| p.byte_delta).sum(),
        digest: safe_digest(&observer.manifest_digest),
        quarantine: observer.quarantine_required,
    })
}

fn usage_from_native(
    native: &NativeActivityProjection,
    task_usage: Option<&TokenUsage>,
    unattributed: bool,
) -> UsageProjection {
    if native.usage_receipt_count > 0 {
        return UsageProjection {
            input: native.usage_input,
            output: native.usage_output,
            cache_read: native.usage_cache_read,
            cache_write: native.usage_cache_write,
            total: native.usage_total,
            pi_reported_cost: native.usage_cost.clone(),
            receipt_count: native.usage_receipt_count,
            possible_unattributed_cost: unattributed,
        };
    }
    task_usage.map_or_else(
        || UsageProjection {
            possible_unattributed_cost: unattributed,
            ..Default::default()
        },
        |usage| UsageProjection {
            input: Some(usage.input_tokens),
            output: Some(usage.output_tokens),
            cache_read: Some(usage.cache_read_input_tokens),
            cache_write: Some(usage.cache_creation_input_tokens),
            total: Some(
                usage
                    .input_tokens
                    .saturating_add(usage.output_tokens)
                    .saturating_add(usage.cache_read_input_tokens)
                    .saturating_add(usage.cache_creation_input_tokens),
            ),
            pi_reported_cost: (usage.cost_usd > 0.0).then(|| format!("{:.6}", usage.cost_usd)),
            receipt_count: 1,
            possible_unattributed_cost: unattributed,
        },
    )
}

/// Load the current selected-task projection. This is a read-only operation.
pub fn load_for_task(wg_dir: &Path, task: &Task) -> LiveProgressProjection {
    let mut out = LiveProgressProjection::default();
    let Some(attempt) = task.lifecycle.current_attempt.as_ref() else {
        out.phase = terminal_phase(task.status).unwrap_or(LivePhase::Unknown);
        return out;
    };
    let runtime_key = crate::attempt_runtime::AttemptRuntimeKey::for_attempt(task, attempt);
    let watchdog = crate::attempt_runtime::resolve_component(wg_dir, &runtime_key, "pi/state.json")
        .ok()
        .flatten()
        .and_then(|path| PiWatchdog::open(&path).ok());
    let observer =
        crate::attempt_runtime::resolve_component(wg_dir, &runtime_key, "worktree-observer")
            .ok()
            .flatten()
            .and_then(|path| read_projection(&path).ok())
            .filter(|o| {
                o.source.identity.task_id == task.id
                    && o.source.identity.generation == task.lifecycle.generation
                    && o.source.identity.attempt_id == attempt.id
                    && o.source.identity.attempt_fence == attempt.fence
                    && o.source.identity.worktree_lease_epoch == attempt.fence
            });

    if let Some(watchdog) = watchdog.as_ref() {
        let state = watchdog.state();
        if state.source.task_id == task.id
            && state.source.generation == task.lifecycle.generation
            && state.source.attempt_id == attempt.id
            && state.source.attempt_fence == attempt.fence
            && state.source.worktree_lease_epoch == attempt.fence
        {
            out.source = Some(SourceTuple {
                task: task.id.clone(),
                generation: state.source.generation,
                attempt: state.source.attempt_id.clone(),
                attempt_fence: state.source.attempt_fence,
                worktree_lease_epoch: state.source.worktree_lease_epoch,
                process_epoch: state.process_epoch,
                continuation_epoch: state.continuation_epoch,
            });
            out.phase = control_phase(state.classification).unwrap_or_else(|| {
                if state.classification == Classification::WaitingUser {
                    LivePhase::WaitingUser
                } else {
                    ordinary_phase(
                        state.phase,
                        &state.native_activity,
                        &state.last_meaningful_kind,
                    )
                }
            });
            out.phase_at = Some(state.last_meaningful_at);
            out.native_event_seq = state.native_activity.event_seq;
            out.native_last_activity_at = state.native_activity.last_activity_at;
            out.thinking_activity = state.native_activity.thinking_activity_seq > 0;
            out.thinking_activity_seq = state.native_activity.thinking_activity_seq;
            out.thinking_tokens = state.native_activity.thinking_tokens;
            out.output_activity_seq = state.native_activity.output_activity_seq;
            out.output_tokens = state.native_activity.output_tokens;
            out.output_rate_milli_tok_per_sec = rolling_rate(
                &state
                    .native_activity
                    .output_samples
                    .iter()
                    .map(|s| (s.at, s.tokens))
                    .collect(),
            );
            out.last_output_activity_at = state.native_activity.last_output_activity_at;
            out.pi_progress = Some(ProgressEvidence {
                seq: state.progress_seq,
                observed_at: state.last_meaningful_at,
                kind: safe_label(&state.last_meaningful_kind),
            });
            out.tool_label = state
                .native_activity
                .current_tool_label
                .clone()
                .unwrap_or_else(|| UNKNOWN.into());
            out.tool_class = state
                .native_activity
                .current_tool_class
                .clone()
                .unwrap_or_else(|| UNKNOWN.into());
            out.tool_progress = state.native_activity.tool_progress;
            out.tool_child_state = state
                .native_activity
                .tool_child_state
                .clone()
                .unwrap_or_else(|| UNKNOWN.into());
            out.tool_receipt_state = state
                .native_activity
                .tool_receipt_state
                .clone()
                .unwrap_or_else(|| UNKNOWN.into());
            out.usage = usage_from_native(
                &state.native_activity,
                task.token_usage.as_ref(),
                state.possible_unattributed_cost,
            );
            out.watchdog.meaningful_silence_secs = watchdog.policy().meaningful_silence_secs;
            // Deadlines are rendered only when an upstream persisted deadline
            // exists. The current watchdog persists its hard-grace deadline;
            // the TUI must not manufacture a proof deadline from local time.
            out.watchdog.probe_grace_deadline = state.hard_grace_deadline;
            out.watchdog.continuation_epoch = state.continuation_epoch;
            out.watchdog.continuation_count = state.epochs_used;
            out.watchdog.continuation_budget = format!(
                "{}/{} epochs; {}/{}s",
                state.epochs_used,
                watchdog
                    .policy()
                    .max_continuation_epochs
                    .saturating_add(state.manual_epochs_granted),
                state.elapsed_reserved_secs,
                watchdog
                    .policy()
                    .max_continuation_elapsed_secs
                    .saturating_add(state.manual_elapsed_granted_secs)
            );
            out.watchdog.eligibility_or_hold_reason = state
                .reason_code
                .as_deref()
                .map(safe_label)
                .unwrap_or_else(|| UNKNOWN.into());
            out.watchdog.safe_next_action = match state.classification {
                Classification::StalledOperatorRequired => format!(
                    "wg pi-watchdog resume {} --reason audited",
                    safe_label(&task.id)
                ),
                _ => format!("wg pi-watchdog status {}", safe_label(&task.id)),
            };
        }
    }
    if let Some(observer) = observer.as_ref() {
        out.worktree_activity = worktree_evidence(observer);
        out.watchdog.observed_activity_grace_secs =
            observer.timing_policy.observed_activity_grace_secs;
        out.watchdog.max_observed_only_extension_secs =
            observer.timing_policy.max_observed_only_extension_secs;
        out.observer_health = format!("{:?}", observer.health);
        out.ignored_churn = format!("{:?}", observer.ignored_churn);
        out.watcher_overflows = observer.watcher_overflows;
        out.unstable_scans = observer.unstable_scans;
        out.late_write_evidence =
            observer.quarantine_required || !observer.late_mutations.is_empty();
    }
    if let Some(disposition) = terminal_phase(task.status) {
        let last_phase = out.phase;
        out.phase = disposition;
        out.terminal_summary = Some(TerminalSummary {
            disposition,
            accepted_at: task
                .completed_at
                .as_deref()
                .map(|s| safe_label_long(s, 48))
                .unwrap_or_else(|| UNKNOWN.into()),
            last_phase,
            pi_progress: out.pi_progress.clone(),
            worktree_activity: out.worktree_activity.clone(),
            usage: out.usage.clone(),
            continuation_count: out.watchdog.continuation_count,
            late_write_evidence: out.late_write_evidence,
            hold_reason: out.watchdog.eligibility_or_hold_reason.clone(),
        });
    }
    out
}

fn opt_u64(value: Option<u64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| UNKNOWN.into())
}
fn opt_i64(value: Option<i64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| UNKNOWN.into())
}

impl LiveProgressProjection {
    /// Fixed-order accessible detail rows. Values are bounded/sanitized and no
    /// color, animation, or spinner is required to understand them.
    pub fn detail_lines(&self, width: usize) -> Vec<String> {
        let compact = width < 64;
        let tokens = format!(
            "  Tokens: output={} thinking={} rate={} tok/s; last output activity={}",
            opt_u64(self.output_tokens),
            opt_u64(self.thinking_tokens),
            self.output_rate_milli_tok_per_sec
                .map(|v| format!("{:.3}", v as f64 / 1000.0))
                .unwrap_or_else(|| UNKNOWN.into()),
            opt_i64(self.last_output_activity_at)
        );
        let native = format!(
            "  Native activity: live/unproven seq={} at={} thinking-events={} output-events={}",
            self.native_event_seq,
            opt_i64(self.native_last_activity_at),
            self.thinking_activity_seq,
            self.output_activity_seq,
        );
        let pi = self
            .pi_progress
            .as_ref()
            .map(|p| {
                format!(
                    "  Pi progress: receipt-proven seq={} {} at={}",
                    p.seq, p.kind, p.observed_at
                )
            })
            .unwrap_or_else(|| {
                "  Pi progress: receipt-proven seq=Unknown kind=Unknown at=Unknown".into()
            });
        let wt = self.worktree_activity.as_ref().map(|w| format!("  Worktree activity: observed/unproven seq={} {} files={} bytes={:+} digest={}{}", w.seq, w.path, w.changed_files, w.byte_delta, w.digest, if w.quarantine { " quarantine" } else { "" })).unwrap_or_else(|| "  Worktree activity: observed/unproven seq=Unknown path=Unknown".into());
        let tool = format!(
            "  Tool/Test: class={} name={} progress={} child={} receipt={}",
            self.tool_class,
            self.tool_label,
            opt_u64(self.tool_progress),
            self.tool_child_state,
            self.tool_receipt_state
        );
        let watchdog = format!(
            "  Watchdog/Resume: silence-policy={}s observed-grace={}s cap={}s proof-deadline={} observed-deadline={} extension={}s probe-grace={} continuation={} count={} budget={} reason={} next={}",
            self.watchdog.meaningful_silence_secs,
            self.watchdog.observed_activity_grace_secs,
            self.watchdog.max_observed_only_extension_secs,
            opt_i64(self.watchdog.proof_deadline),
            opt_i64(self.watchdog.observed_deadline),
            opt_u64(self.watchdog.observed_only_extension_secs),
            opt_i64(self.watchdog.probe_grace_deadline),
            self.watchdog.continuation_epoch,
            self.watchdog.continuation_count,
            self.watchdog.continuation_budget,
            self.watchdog.eligibility_or_hold_reason,
            self.watchdog.safe_next_action
        );
        let accounting = format!(
            "  Accounting: input={} output={} cache-read={} cache-write={} total={} Pi-reported cost={} receipts={}{}",
            opt_u64(self.usage.input),
            opt_u64(self.usage.output),
            opt_u64(self.usage.cache_read),
            opt_u64(self.usage.cache_write),
            opt_u64(self.usage.total),
            self.usage
                .pi_reported_cost
                .clone()
                .unwrap_or_else(|| UNKNOWN.into()),
            self.usage.receipt_count,
            if self.usage.possible_unattributed_cost {
                "; possible unattributed cost"
            } else {
                ""
            }
        );
        let mut lines = vec![
            "── Live progress ──".into(),
            format!("  Phase: {}", self.phase.label()),
            tokens,
            native,
            pi,
            wt,
            tool,
            watchdog,
        ];
        if !compact {
            lines.push(accounting);
            lines.push(format!(
                "  Observer: health={} ignored-churn={} overflows={} unstable={} late-write={}",
                self.observer_health,
                self.ignored_churn,
                self.watcher_overflows,
                self.unstable_scans,
                self.late_write_evidence
            ));
            if let Some(source) = &self.source {
                lines.push(format!("  Source: task={} generation={} attempt={} fence={} lease={} process-epoch={} continuation-epoch={}", safe_label(&source.task), source.generation, safe_label(&source.attempt), source.attempt_fence, source.worktree_lease_epoch, source.process_epoch, source.continuation_epoch));
            }
            if let Some(summary) = &self.terminal_summary {
                lines.push(format!("  Terminal summary: {} at={} last-phase={} continuations={} late-write={} usage-total={}", summary.disposition.label(), summary.accepted_at, summary.last_phase.label(), summary.continuation_count, summary.late_write_evidence, opt_u64(summary.usage.total)));
            }
        } else {
            lines.push("  More: accounting/source/deadlines below fold…".into());
        }
        lines.push(String::new());
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_progress_phase_reducer_precedence() {
        let mut r = LiveProgressReducer::default();
        r.apply(LiveEvent::Worktree {
            seq: 1,
            at: 1,
            path: "src/x.rs".into(),
            changed_files: 1,
            byte_delta: 4,
            digest: "b3:123".into(),
            quarantine: false,
        });
        assert_eq!(r.projection().phase, LivePhase::Unknown);
        r.apply(LiveEvent::ProviderRequest { seq: 1, at: 2 });
        assert_eq!(r.projection().phase, LivePhase::WaitingProvider);
        r.apply(LiveEvent::Thinking {
            seq: 2,
            at: 3,
            tokens: Some(7),
        });
        assert_eq!(r.projection().phase, LivePhase::Thinking);
        r.apply(LiveEvent::Thinking {
            seq: 3,
            at: 4,
            tokens: None,
        });
        assert_eq!(r.projection().thinking_tokens, None);
        r.apply(LiveEvent::Output {
            seq: 4,
            at: 5,
            tokens: Some(10),
        });
        assert_eq!(r.projection().phase, LivePhase::Generating);
        r.apply(LiveEvent::Writing { seq: 5, at: 6 });
        assert_eq!(r.projection().phase, LivePhase::Writing);
        r.apply(LiveEvent::Tool {
            seq: 6,
            at: 7,
            class: "tool".into(),
            label: "build".into(),
            progress: Some(2),
        });
        assert_eq!(r.projection().phase, LivePhase::Tool);
        r.apply(LiveEvent::Tool {
            seq: 7,
            at: 8,
            class: "test".into(),
            label: "cargo-test".into(),
            progress: Some(3),
        });
        assert_eq!(r.projection().phase, LivePhase::Testing);
        r.apply(LiveEvent::WaitingUser { seq: 8, at: 9 });
        assert_eq!(r.projection().phase, LivePhase::WaitingUser);
        for (seq, phase) in [
            (9, LivePhase::Suspect),
            (10, LivePhase::Fencing),
            (11, LivePhase::Resuming),
            (12, LivePhase::Stalled),
        ] {
            r.apply(LiveEvent::Control {
                seq,
                at: seq as i64,
                phase,
                reason: "bounded-code".into(),
            });
            assert_eq!(r.projection().phase, phase);
        }
        r.apply(LiveEvent::Terminal {
            seq: 13,
            at: "2026-01-01T00:00:00Z".into(),
            disposition: LivePhase::Done,
        });
        r.apply(LiveEvent::Control {
            seq: 14,
            at: 14,
            phase: LivePhase::Resuming,
            reason: "late".into(),
        });
        assert_eq!(r.projection().phase, LivePhase::Done);
    }

    #[test]
    fn live_progress_usage_is_deduplicated_and_unknown_is_honest() {
        let mut r = LiveProgressReducer::default();
        r.apply(LiveEvent::Output {
            seq: 1,
            at: 10,
            tokens: None,
        });
        assert_eq!(r.projection().output_rate_milli_tok_per_sec, None);
        assert!(
            r.projection()
                .detail_lines(120)
                .join("\n")
                .contains("rate=Unknown")
        );
        r.apply(LiveEvent::Output {
            seq: 2,
            at: 11,
            tokens: Some(5),
        });
        assert_eq!(r.projection().output_rate_milli_tok_per_sec, None);
        r.apply(LiveEvent::Output {
            seq: 3,
            at: 13,
            tokens: Some(11),
        });
        assert_eq!(r.projection().output_rate_milli_tok_per_sec, Some(3000));
        let usage = |receipt: &str| LiveEvent::Usage {
            seq: 4,
            receipt: receipt.into(),
            input: Some(10),
            output: Some(5),
            cache_read: None,
            cache_write: None,
            total: Some(15),
            cost: None,
        };
        r.apply(usage("turn-a"));
        r.apply(usage("turn-a"));
        assert_eq!(r.projection().usage.total, Some(15));
        assert_eq!(r.projection().usage.pi_reported_cost, None);
        let proven = r.projection().pi_progress.clone();
        r.apply(LiveEvent::Worktree {
            seq: 9,
            at: 99,
            path: "x".into(),
            changed_files: 1,
            byte_delta: 1,
            digest: "aa".into(),
            quarantine: false,
        });
        assert_eq!(r.projection().pi_progress, proven);
    }

    #[test]
    fn live_progress_never_renders_reasoning_or_hostile_output() {
        let canary = "RAW_REASONING_CANARY_7f3b";
        let mut r = LiveProgressReducer::default();
        r.apply(LiveEvent::Thinking {
            seq: 1,
            at: 1,
            tokens: None,
        });
        r.apply(LiveEvent::Tool {
            seq: 2,
            at: 2,
            class: "test".into(),
            label: format!("ok\u{1b}[31m{canary}"),
            progress: None,
        });
        r.apply(LiveEvent::Worktree {
            seq: 1,
            at: 2,
            path: format!("evil\n{canary}.rs"),
            changed_files: 1,
            byte_delta: 1,
            digest: "b3:abc".into(),
            quarantine: false,
        });
        let rendered = r.projection().detail_lines(120).join("\n");
        let serialized = serde_json::to_string(r.projection()).unwrap();
        assert!(!rendered.contains(canary));
        assert!(!serialized.contains(canary));
        assert!(rendered.contains("thinking=Unknown"));
    }

    #[test]
    fn live_progress_restart_and_late_fence_are_monotonic() {
        let mut r = LiveProgressReducer::default();
        assert_eq!(
            (
                r.projection().watchdog.meaningful_silence_secs,
                r.projection().watchdog.observed_activity_grace_secs,
                r.projection().watchdog.max_observed_only_extension_secs
            ),
            (300, 120, 600)
        );
        r.apply(LiveEvent::Output {
            seq: 2,
            at: 100,
            tokens: Some(2),
        });
        r.apply(LiveEvent::Terminal {
            seq: 3,
            at: "done".into(),
            disposition: LivePhase::Failed,
        });
        let frozen = r.projection().terminal_summary.clone();
        r.apply(LiveEvent::Worktree {
            seq: 99,
            at: 50,
            path: "late.rs".into(),
            changed_files: 1,
            byte_delta: 9,
            digest: "ff".into(),
            quarantine: true,
        });
        assert_eq!(r.projection().phase, LivePhase::Failed);
        assert_eq!(r.projection().terminal_summary, frozen);
        assert!(r.projection().late_write_evidence);
    }

    #[test]
    fn live_progress_layout_and_throttle_are_stable() {
        let mut r = LiveProgressReducer::default();
        for i in 1..=5000 {
            r.apply(LiveEvent::Output {
                seq: i,
                at: i as i64,
                tokens: Some(i),
            });
        }
        let wide = r.projection().detail_lines(160);
        let narrow = r.projection().detail_lines(40);
        for required in [
            "Phase:",
            "Tokens:",
            "Native activity: live/unproven",
            "Pi progress: receipt-proven",
            "Worktree activity: observed/unproven",
            "Tool/Test:",
            "Watchdog/Resume:",
        ] {
            assert!(wide.iter().any(|l| l.contains(required)));
            assert!(narrow.iter().any(|l| l.contains(required)));
        }
        assert!(narrow.iter().any(|l| l.contains("below fold")));
        assert_eq!(r.projection().pi_progress.as_ref().unwrap().seq, 5000);
    }
}
