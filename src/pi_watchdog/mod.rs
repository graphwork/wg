//! Evidence-driven Pi task-worker watchdog and same-session continuation state machine.
//!
//! This module deliberately owns only process/continuation epochs and their
//! evidence projection.  It never writes a task status.  Canonical lifecycle
//! dispositions remain requests to [`crate::lifecycle::LifecycleKernel`].

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const PROMPT_VERSION: &str = "WG_PI_CONTINUATION_V2";
pub const PRODUCTION_SOFT_SILENCE_SECS: u64 = 300;
pub const MIN_FREE_LOW_HARD_RESUME_SECS: u64 = 900;
pub const STOCK_PROMPT_TEMPLATE: &str = "[WG_PI_CONTINUATION_V2]\nWG observed `<OBSERVATION_CODE>` for this process epoch; no accepted terminal\nreceipt exists. Inspect the durable SAME Pi session, leased worktree, task\ncontract, candidate state, relevant tests, and supplied receipt summaries.\nDo not repeat a side effect; reconcile it from receipts/postconditions first.\nThen produce exactly one explicit outcome: `wg_done`, `wg_fail`, or the\ncorrelated `wg_wait` required by the task. This prompt is guidance, not proof.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Classification {
    Active,
    WaitingUser,
    LongTool,
    Suspect,
    HardResumeEligible,
    NeedsFinalization,
    Fencing,
    Resuming,
    StalledOperatorRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Unknown,
    ProviderRequestInFlight,
    ProviderResponseStream,
    Tool,
    Settled,
    Exited,
}

/// UI-safe numeric activity sample projected by the watchdog's canonical Pi
/// parser. It contains no provider text or reasoning content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeTokenSample {
    pub at: i64,
    pub tokens: u64,
}

/// Persisted UI-facing subset of native Pi evidence. Raw thinking, provider
/// errors, prompts, tool output, arguments, and file content are deliberately
/// unrepresentable here; the TUI consumes only this projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NativeActivityProjection {
    pub process_epoch: u32,
    pub event_seq: u64,
    pub last_activity_at: Option<i64>,
    pub thinking_activity_seq: u64,
    pub thinking_tokens: Option<u64>,
    pub output_activity_seq: u64,
    pub output_tokens: Option<u64>,
    pub last_output_activity_at: Option<i64>,
    #[serde(default)]
    pub output_samples: Vec<NativeTokenSample>,
    pub current_tool_label: Option<String>,
    pub current_tool_class: Option<String>,
    pub tool_progress: Option<u64>,
    pub tool_child_state: Option<String>,
    pub tool_receipt_state: Option<String>,
    pub usage_input: Option<u64>,
    pub usage_output: Option<u64>,
    pub usage_cache_read: Option<u64>,
    pub usage_cache_write: Option<u64>,
    pub usage_total: Option<u64>,
    pub usage_cost: Option<String>,
    pub usage_receipt_count: u64,
    #[serde(default)]
    usage_receipts: BTreeSet<String>,
    /// Per-capture byte cursors make replay/restart idempotent without
    /// deduplicating legitimate equal deltas. Keys are bounded stream digests,
    /// never filesystem paths or provider content.
    #[serde(default)]
    stream_offsets: BTreeMap<String, u64>,
}

/// A matching Pi journal selected without rewriting or deleting any evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalSessionJournal {
    pub session_file: PathBuf,
    pub header_json: String,
    pub header_digest: String,
    pub branch_leaf: String,
    pub append_prefix_digest: String,
    pub append_prefix_len: u64,
    pub substantive: bool,
    pub bootstrap_evidence: Vec<(u64, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QosClass {
    Free,
    Low,
    Standard,
    Premium,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchdogPolicy {
    pub meaningful_silence_secs: u64,
    pub hard_resume_grace_secs: u64,
    pub max_hard_resume_grace_secs: u64,
    pub provider_request_hard_secs: BTreeMap<QosClass, u64>,
    pub provider_response_hard_secs: BTreeMap<QosClass, u64>,
    pub max_continuation_epochs: u32,
    pub max_continuation_elapsed_secs: u64,
    pub continuation_epoch_lease_secs: u64,
    pub term_grace_secs: u64,
    pub kill_grace_secs: u64,
}

impl Default for WatchdogPolicy {
    fn default() -> Self {
        let mut requests = BTreeMap::new();
        let mut responses = BTreeMap::new();
        for qos in [
            QosClass::Free,
            QosClass::Low,
            QosClass::Standard,
            QosClass::Premium,
        ] {
            requests.insert(qos, 900);
            responses.insert(qos, 900);
        }
        Self {
            meaningful_silence_secs: PRODUCTION_SOFT_SILENCE_SECS,
            hard_resume_grace_secs: 60,
            max_hard_resume_grace_secs: 180,
            provider_request_hard_secs: requests,
            provider_response_hard_secs: responses,
            max_continuation_epochs: 3,
            max_continuation_elapsed_secs: 1800,
            continuation_epoch_lease_secs: 600,
            term_grace_secs: 10,
            kill_grace_secs: 5,
        }
    }
}

impl WatchdogPolicy {
    pub fn validate(&self) -> Result<(), WatchdogError> {
        if self.meaningful_silence_secs != PRODUCTION_SOFT_SILENCE_SECS {
            return Err(WatchdogError::new(
                "invalid_soft_threshold",
                "production meaningful_silence_secs must equal 300",
            ));
        }
        if self.hard_resume_grace_secs == 0
            || self.hard_resume_grace_secs > self.max_hard_resume_grace_secs
        {
            return Err(WatchdogError::new(
                "invalid_hard_grace",
                "hard resume grace must be nonzero and at most its static cap",
            ));
        }
        for table in [
            &self.provider_request_hard_secs,
            &self.provider_response_hard_secs,
        ] {
            for qos in [QosClass::Free, QosClass::Low] {
                if table.get(&qos).copied().unwrap_or(0) < MIN_FREE_LOW_HARD_RESUME_SECS {
                    return Err(WatchdogError::new(
                        "invalid_hard_threshold",
                        "free/low Pi hard resume thresholds must be at least 900 seconds",
                    ));
                }
            }
        }
        if self.max_continuation_epochs == 0
            || self.max_continuation_elapsed_secs == 0
            || self.continuation_epoch_lease_secs == 0
        {
            return Err(WatchdogError::new(
                "invalid_continuation_budget",
                "Pi continuation budgets must be finite and nonzero",
            ));
        }
        Ok(())
    }

    fn hard_secs(&self, phase: Phase, qos: QosClass) -> Option<u64> {
        match phase {
            Phase::ProviderRequestInFlight => self.provider_request_hard_secs.get(&qos).copied(),
            Phase::ProviderResponseStream => self.provider_response_hard_secs.get(&qos).copied(),
            _ => None,
        }
    }
}

/// Test-only ordered policy. Conversion intentionally bypasses production
/// validation; no config loader or environment variable can construct it.
#[derive(Debug, Clone, Copy)]
pub struct TestPolicy;
impl TestPolicy {
    pub fn ordered() -> Self {
        Self
    }
}
impl From<TestPolicy> for WatchdogPolicy {
    fn from(_: TestPolicy) -> Self {
        WatchdogPolicy::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTuple {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub worktree_lease_epoch: u64,
    pub worktree_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteSnapshot {
    pub handler: String,
    pub provider: String,
    pub model: String,
    pub reasoning: Option<String>,
    pub endpoint_redacted: String,
    pub endpoint_hmac: String,
    pub qos: QosClass,
    pub pi_binary_digest: String,
    pub plugin_digest: String,
}
impl RouteSnapshot {
    pub fn digest(&self) -> String {
        digest_json(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionProof {
    pub session_id: String,
    pub branch_leaf: String,
    pub session_dir: PathBuf,
    pub session_file: PathBuf,
    pub header_digest: String,
    pub append_prefix_digest: String,
    pub append_prefix_len: u64,
}
impl SessionProof {
    pub fn digest(&self) -> String {
        digest_json(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub pgid: u32,
    pub start_ticks: u64,
    pub boot_id: String,
    pub nonce: String,
}
impl ProcessIdentity {
    pub fn digest(&self) -> String {
        digest_json(self)
    }
}

/// Durable authority for the exact process currently allowed to contribute
/// progress or terminal/exit receipts to one immutable source attempt.
/// Continuation prompts deliberately do not appear here: they advance
/// `continuation_epoch` while an in-process continuation retains this exact
/// PID/start/boot/nonce and process epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessEpochAuthority {
    pub process_epoch: u32,
    pub process_identity_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolEffect {
    ReadOnly,
    Idempotent,
    ReceiptBacked,
    NonIdempotent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContract {
    pub tool_call_id: String,
    pub effect: ToolEffect,
    pub lease_expires_at: Option<i64>,
    pub completion_receipt: Option<String>,
}
impl ToolContract {
    pub fn non_idempotent(id: impl Into<String>) -> Self {
        Self {
            tool_call_id: id.into(),
            effect: ToolEffect::NonIdempotent,
            lease_expires_at: None,
            completion_receipt: None,
        }
    }
    pub fn read_only(id: impl Into<String>, expires: i64) -> Self {
        Self {
            tool_call_id: id.into(),
            effect: ToolEffect::ReadOnly,
            lease_expires_at: Some(expires),
            completion_receipt: None,
        }
    }
    pub fn is_safe(&self) -> bool {
        matches!(self.effect, ToolEffect::ReadOnly | ToolEffect::Idempotent)
            || (self.effect == ToolEffect::ReceiptBacked && self.completion_receipt.is_some())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExitStatus {
    Code(i32),
    Signal(i32),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalDisposition {
    SuccessIntent,
    Failure,
    Park,
    Cancel,
    Abort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalIntentReceipt {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub attempt_fence: u64,
    pub process_epoch: u32,
    #[serde(default)]
    pub process_identity_digest: String,
    pub tool_call_id: String,
    pub disposition: TerminalDisposition,
    pub idempotency_key: String,
}
impl TerminalIntentReceipt {
    pub fn new(
        w: &PiWatchdog,
        process_epoch: u32,
        tool_call_id: impl Into<String>,
        disposition: TerminalDisposition,
    ) -> Self {
        let tool_call_id = tool_call_id.into();
        let s = &w.state.source;
        Self {
            task_id: s.task_id.clone(),
            generation: s.generation,
            attempt_id: s.attempt_id.clone(),
            attempt_fence: s.attempt_fence,
            process_epoch,
            process_identity_digest: w.state.process.digest(),
            idempotency_key: format!(
                "{}:{}:{}:{:?}",
                s.attempt_id, process_epoch, tool_call_id, disposition
            ),
            tool_call_id,
            disposition,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiQuiescenceReceipt {
    pub source: SourceTuple,
    pub process_epoch: u32,
    pub process_identity_digest: String,
    pub final_session_head: String,
    pub final_worktree_manifest_digest: String,
    pub process_group_empty: bool,
    pub nonce_pipe_eof: bool,
    pub reaped_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DoneProofV1 {
    pub terminal: Option<TerminalIntentReceipt>,
    pub quiescence: Option<PiQuiescenceReceipt>,
    pub candidate_checkpoint: Option<String>,
    pub validation: Option<String>,
    pub evaluation: Option<String>,
    pub finalization_event: Option<String>,
}
impl DoneProofV1 {
    pub fn is_complete_for(&self, w: &PiWatchdog) -> bool {
        let s = &w.state;
        self.terminal.as_ref().is_some_and(|r| {
            r.disposition == TerminalDisposition::SuccessIntent
                && r.process_epoch == s.process_epoch
                && (r.process_identity_digest.is_empty()
                    || r.process_identity_digest == s.process.digest())
                && receipt_matches(&s.source, r)
        }) && self.quiescence.as_ref().is_some_and(|r| {
            r.source == s.source
                && r.process_epoch == s.process_epoch
                && r.process_group_empty
                && r.nonce_pipe_eof
        }) && self
            .candidate_checkpoint
            .as_ref()
            .is_some_and(|v| !v.is_empty())
            && self.validation.as_ref().is_some_and(|v| !v.is_empty())
            && self.evaluation.as_ref().is_some_and(|v| !v.is_empty())
            && self
                .finalization_event
                .as_ref()
                .is_some_and(|v| !v.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DomainCounters {
    pub admission: u64,
    pub source_retry: u64,
    pub spawn_breaker: u64,
    pub provider_breaker: u64,
    pub evaluation_jobs: u64,
    pub accounting_attempts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiWatchdogState {
    pub schema_version: u32,
    pub source: SourceTuple,
    pub route: RouteSnapshot,
    pub session: SessionProof,
    pub process: ProcessIdentity,
    pub process_epoch: u32,
    pub continuation_epoch: u32,
    pub classification: Classification,
    pub phase: Phase,
    pub progress_seq: u64,
    pub progress_digest: String,
    pub last_meaningful_at: i64,
    pub last_meaningful_kind: String,
    pub suspect_at: Option<i64>,
    pub probe_action_id: Option<String>,
    pub probe_observed_at: Option<i64>,
    pub hard_resume_after_secs: Option<u64>,
    pub hard_eligible_at: Option<i64>,
    pub hard_grace_deadline: Option<i64>,
    pub terminal: bool,
    pub terminal_receipt: Option<TerminalIntentReceipt>,
    pub tool: Option<ToolContract>,
    pub wait_correlation: Option<String>,
    pub exact_guards: GuardState,
    pub epochs_used: u32,
    pub elapsed_reserved_secs: u64,
    pub manual_epochs_granted: u32,
    pub manual_elapsed_granted_secs: u64,
    pub prompt_action_id: Option<String>,
    pub prompt_digest: Option<String>,
    pub prompt_marker: Option<String>,
    pub prompt_count: u32,
    pub pending_actions: Vec<ActionRecord>,
    pub completed_action_ids: BTreeSet<String>,
    pub manual_grant_ids: BTreeSet<String>,
    pub reason_code: Option<String>,
    pub exact_route_error: Option<String>,
    pub possible_unattributed_cost: bool,
    #[serde(default)]
    pub native_activity: NativeActivityProjection,
    pub domain_counters: DomainCounters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardState {
    pub session: bool,
    pub route: bool,
    pub worktree: bool,
    pub pid_identity: bool,
    pub containment: bool,
    pub effect: bool,
    pub terminal_clear: bool,
}
impl Default for GuardState {
    fn default() -> Self {
        Self {
            session: true,
            route: true,
            worktree: true,
            pid_identity: true,
            containment: true,
            effect: true,
            terminal_clear: true,
        }
    }
}
impl GuardState {
    fn all(&self) -> bool {
        self.session
            && self.route
            && self.worktree
            && self.pid_identity
            && self.containment
            && self.effect
            && self.terminal_clear
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardFailure {
    Session,
    Route,
    Worktree,
    PidIdentity,
    Containment,
    Effect,
    TerminalReservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    ReadOnlyProbe,
    StartHardGrace,
    ReserveContinuation,
    FenceExactProcess,
    LaunchSameSession,
    AppendCompletionPrompt,
    QuiesceForFinalization,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub action_id: String,
    pub kind: ActionKind,
    pub state: ActionState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionState {
    Pending,
    Completed,
    Uncertain,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    ProviderRequestStarted {
        call_id: String,
    },
    ProviderResponseStarted,
    ProviderRetry,
    CompactionRetry,
    QueuedFollowUp,
    AgentEndWillRetry,
    ThinkingDelta,
    TokenDelta {
        tokens: u64,
    },
    ToolProgress {
        tool_call_id: String,
        progress: u64,
    },
    UsageReceipt {
        receipt: String,
    },
    SessionAdvanced {
        leaf: String,
        prefix_digest: String,
    },
    WorktreeProgress {
        manifest_digest: String,
    },
    Heartbeat,
    StatusPolled,
    OrdinaryMessage,
    ProbeTraffic,
    ProbeObserved {
        progress_seq: u64,
        session_leaf: String,
        alive: bool,
    },
    PhaseUnknown,
    AgentSettled,
    ProcessExited {
        status: ExitStatus,
        reaped: bool,
    },
    PipeEof {
        reaped: bool,
    },
    ToolIntent {
        contract: ToolContract,
    },
    ToolReceipt {
        tool_call_id: String,
        receipt: String,
    },
    ToolCompleted {
        tool_call_id: String,
        receipt: String,
    },
    WaitAccepted {
        correlation: String,
    },
    TerminalIntent(TerminalIntentReceipt),
    GuardFailure(GuardFailure),
    PromptMarkerUncertain,
    ContinuationLaunched,
    ExecutionPermitted,
    ReplayPendingActions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectAcknowledgement {
    pub tool_call_id: String,
    pub disposition: String,
    pub receipt: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualGrant {
    pub action_id: String,
    pub reason: String,
    pub epochs: u32,
    pub elapsed_secs: u64,
    pub effect_ack: Option<EffectAcknowledgement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashBarrier {
    AfterContinuationReserved,
    AfterPromptIntent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogError {
    pub code: String,
    pub message: String,
}
impl WatchdogError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for WatchdogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for WatchdogError {}

pub struct PiWatchdog {
    state: PiWatchdogState,
    policy: WatchdogPolicy,
    state_path: PathBuf,
    journal_path: PathBuf,
    crash_barrier: Option<CrashBarrier>,
}

impl PiWatchdog {
    pub fn new(
        source: SourceTuple,
        route: RouteSnapshot,
        session: SessionProof,
        process: ProcessIdentity,
        policy: WatchdogPolicy,
        now: i64,
    ) -> Result<Self, WatchdogError> {
        let state_path = source.worktree_path.join(".wg-pi-watchdog/state.json");
        Self::new_at(state_path, source, route, session, process, policy, now)
    }

    pub fn new_at(
        state_path: PathBuf,
        source: SourceTuple,
        route: RouteSnapshot,
        session: SessionProof,
        process: ProcessIdentity,
        policy: WatchdogPolicy,
        now: i64,
    ) -> Result<Self, WatchdogError> {
        policy.validate()?;
        if route.handler != "pi" {
            return Err(WatchdogError::new(
                "route_mismatch",
                "Pi watchdog requires handler=pi",
            ));
        }
        let root = state_path
            .parent()
            .ok_or_else(|| WatchdogError::new("watchdog_io", "state path has no parent"))?;
        fs::create_dir_all(root).map_err(io_error)?;
        let mut watchdog = Self {
            state: PiWatchdogState {
                schema_version: 2,
                source,
                route,
                session,
                process,
                process_epoch: 1,
                continuation_epoch: 0,
                classification: Classification::Active,
                phase: Phase::Unknown,
                progress_seq: 0,
                progress_digest: digest_bytes(b"bootstrap"),
                last_meaningful_at: now,
                last_meaningful_kind: "initial-execution-permit".into(),
                suspect_at: None,
                probe_action_id: None,
                probe_observed_at: None,
                hard_resume_after_secs: None,
                hard_eligible_at: None,
                hard_grace_deadline: None,
                terminal: false,
                terminal_receipt: None,
                tool: None,
                wait_correlation: None,
                exact_guards: GuardState::default(),
                epochs_used: 0,
                elapsed_reserved_secs: 0,
                manual_epochs_granted: 0,
                manual_elapsed_granted_secs: 0,
                prompt_action_id: None,
                prompt_digest: None,
                prompt_marker: None,
                prompt_count: 0,
                pending_actions: Vec::new(),
                completed_action_ids: BTreeSet::new(),
                manual_grant_ids: BTreeSet::new(),
                reason_code: None,
                exact_route_error: None,
                possible_unattributed_cost: false,
                native_activity: NativeActivityProjection {
                    process_epoch: 1,
                    ..NativeActivityProjection::default()
                },
                domain_counters: DomainCounters::default(),
            },
            policy,
            state_path: state_path.clone(),
            journal_path: state_path.with_file_name("progress.jsonl"),
            crash_barrier: None,
        };
        watchdog.persist("authorized", now)?;
        Ok(watchdog)
    }

    pub fn open(path: &Path) -> Result<Self, WatchdogError> {
        let bytes = fs::read(path).map_err(io_error)?;
        let persisted: Persisted = serde_json::from_slice(&bytes).map_err(json_error)?;
        persisted.policy.validate()?;
        Ok(Self {
            state: persisted.state,
            policy: persisted.policy,
            state_path: path.to_owned(),
            journal_path: path.with_file_name("progress.jsonl"),
            crash_barrier: None,
        })
    }
    pub fn state(&self) -> &PiWatchdogState {
        &self.state
    }

    /// Return the durable cursor for a bounded capture identity.
    pub fn native_stream_offset(&self, stream_id: &str) -> u64 {
        let stream_id = digest_bytes(stream_id.as_bytes());
        self.state
            .native_activity
            .stream_offsets
            .get(&stream_id)
            .copied()
            .unwrap_or(0)
    }

    /// Ingest one complete native line at its append-only end offset. Replays
    /// from the same capture are skipped by byte position; identical deltas at
    /// distinct positions remain distinct activity.
    pub fn ingest_native_line(
        &mut self,
        line: &str,
        stream_id: &str,
        end_offset: u64,
        now: i64,
    ) -> Result<Vec<ActionKind>, WatchdogError> {
        let stream_id = digest_bytes(stream_id.as_bytes());
        if end_offset
            <= self
                .state
                .native_activity
                .stream_offsets
                .get(&stream_id)
                .copied()
                .unwrap_or(0)
        {
            return Ok(Vec::new());
        }
        let value: serde_json::Value = serde_json::from_str(line).map_err(json_error)?;
        self.state
            .native_activity
            .stream_offsets
            .insert(stream_id, end_offset);
        self.ingest_native_value(&value, now)
    }

    /// Reconcile Pi's bootstrap header with the journal Pi actually appended.
    /// One substantive match wins over any number of header-only evidence
    /// files. Multiple substantive matches fail closed and no bytes move.
    pub fn reconcile_session_journal(&mut self, now: i64) -> Result<bool, WatchdogError> {
        let selected = select_canonical_session_journal(
            &self.state.session.session_dir,
            &self.state.session.session_id,
        )?;
        let changed = self.state.session.session_file != selected.session_file
            || self.state.session.branch_leaf != selected.branch_leaf
            || self.state.session.append_prefix_len != selected.append_prefix_len;
        if changed {
            self.state.session.session_file = selected.session_file;
            self.state.session.header_digest = selected.header_digest;
            self.state.session.branch_leaf = selected.branch_leaf;
            self.state.session.append_prefix_digest = selected.append_prefix_digest;
            self.state.session.append_prefix_len = selected.append_prefix_len;
            self.meaningful("session-journal-attested", b"canonical-journal", now);
            self.persist("session-journal-reconciled", now)?;
        }
        Ok(changed)
    }
    #[doc(hidden)]
    pub fn state_mut_for_test(&mut self) -> &mut PiWatchdogState {
        &mut self.state
    }
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }
    pub fn policy(&self) -> &WatchdogPolicy {
        &self.policy
    }

    /// Bind a reopened watchdog to the lifecycle projection before any
    /// receipt is accepted. Schema-v1 states may contain the historical split
    /// where an in-process continuation incorrectly advanced only the
    /// watchdog process epoch; that exact shape is repaired once and persisted.
    pub fn attest_lifecycle_process_authority(
        &mut self,
        lifecycle_process_epoch: u32,
        lifecycle_process_identity_digest: &str,
        now: i64,
    ) -> Result<ProcessEpochAuthority, WatchdogError> {
        if self.state.schema_version == 1
            && lifecycle_process_epoch > 0
            && self.state.process_epoch > lifecycle_process_epoch
            && self.state.continuation_epoch > 0
            && (lifecycle_process_identity_digest.is_empty()
                || lifecycle_process_identity_digest == self.state.process.digest())
        {
            self.state.process_epoch = lifecycle_process_epoch;
            self.state.native_activity.process_epoch = lifecycle_process_epoch;
            self.state.schema_version = 2;
            self.persist("legacy-same-process-epoch-repaired", now)?;
        }
        let authority = self.process_epoch_authority();
        if lifecycle_process_epoch != authority.process_epoch
            || (!lifecycle_process_identity_digest.is_empty()
                && lifecycle_process_identity_digest != authority.process_identity_digest)
        {
            return Err(WatchdogError::new(
                "process_epoch_authority_mismatch",
                "lifecycle and watchdog disagree on the current exact Pi process authority",
            ));
        }
        if self.state.schema_version < 2 {
            self.state.schema_version = 2;
            self.persist("process-epoch-schema-upgraded", now)?;
        }
        Ok(authority)
    }

    pub fn process_epoch_authority(&self) -> ProcessEpochAuthority {
        ProcessEpochAuthority {
            process_epoch: self.state.process_epoch,
            process_identity_digest: self.state.process.digest(),
        }
    }

    /// Atomically replace the exact process identity and advance its fence.
    /// A retry after a crash is idempotent when the replacement identity is
    /// already current. Any genuinely old or competing replacement is
    /// rejected before progress can be attributed to it.
    pub fn replace_process_epoch(
        &mut self,
        expected: &ProcessIdentity,
        replacement: ProcessIdentity,
        now: i64,
    ) -> Result<ProcessEpochAuthority, WatchdogError> {
        if self.state.process == replacement {
            return Ok(self.process_epoch_authority());
        }
        if &self.state.process != expected {
            return Err(WatchdogError::new(
                "stale_process_identity",
                "replacement expected an old or competing Pi process identity",
            ));
        }
        if self.state.terminal {
            return Err(WatchdogError::new(
                "attempt_already_terminal",
                "a terminal receipt won before process replacement",
            ));
        }
        if replacement.digest() == expected.digest() {
            return Err(WatchdogError::new(
                "process_identity_unchanged",
                "same exact PID/start/boot/nonce is a continuation, not a replacement process",
            ));
        }
        self.state.process_epoch = self.state.process_epoch.saturating_add(1);
        self.state.process = replacement;
        self.state.exact_guards.pid_identity = true;
        // Leave native_activity on the old epoch until the first record from
        // the replacement arrives; projection then clears per-process live
        // counters while retaining deduplicated accounting/cursors.
        self.persist("process-epoch-replaced", now)?;
        Ok(self.process_epoch_authority())
    }

    /// Ingest one native Pi JSON/RPC record without letting ordinary log or
    /// heartbeat traffic manufacture progress. Unknown records are inert.
    pub fn ingest_native_value(
        &mut self,
        value: &serde_json::Value,
        now: i64,
    ) -> Result<Vec<ActionKind>, WatchdogError> {
        // Project only bounded numeric/category evidence. This runs in the
        // watchdog (the canonical native parser), never in a UI renderer.
        let native_changed = self.project_native_activity(value, now);
        let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let observation = match ty {
            "turn_start" | "provider_request_start" | "model_request_start" => {
                Some(Observation::ProviderRequestStarted {
                    call_id: value
                        .get("callId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("provider")
                        .to_string(),
                })
            }
            "message_start" | "provider_response_start" | "model_response_start" => {
                Some(Observation::ProviderResponseStarted)
            }
            "provider_retry" => Some(Observation::ProviderRetry),
            "compaction_retry" => Some(Observation::CompactionRetry),
            "message_update" => match value
                .get("assistantMessageEvent")
                .and_then(|v| v.get("type"))
                .and_then(|v| v.as_str())
            {
                Some("thinking_start" | "thinking_delta" | "thinking_end") => {
                    Some(Observation::ThinkingDelta)
                }
                Some("text_delta" | "toolcall_start" | "toolcall_delta" | "toolcall_end") => {
                    Some(Observation::TokenDelta { tokens: 1 })
                }
                _ => None,
            },
            "tool_execution_start" => Some(Observation::ToolIntent {
                contract: ToolContract {
                    tool_call_id: value
                        .get("toolCallId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    effect: ToolEffect::NonIdempotent,
                    // Runtime is not a deadline. An open native tool remains
                    // visible and blocks automatic continuation until a
                    // receipt closes its effect.
                    lease_expires_at: None,
                    completion_receipt: None,
                },
            }),
            "tool_execution_update" => Some(Observation::ToolProgress {
                tool_call_id: value
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                progress: self.state.progress_seq + 1,
            }),
            "tool_execution_end" => Some(Observation::ToolCompleted {
                tool_call_id: value
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                receipt: digest_bytes(
                    format!(
                        "{}:{}",
                        value
                            .get("toolCallId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown"),
                        value
                            .get("isError")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    )
                    .as_bytes(),
                ),
            }),
            "turn_end" if native_changed => Some(Observation::UsageReceipt {
                receipt: value
                    .get("turnId")
                    .and_then(|v| v.as_str())
                    .map(|v| digest_bytes(v.as_bytes()))
                    .unwrap_or_else(|| digest_bytes(value.to_string().as_bytes())),
            }),
            "agent_end" if value.get("willRetry").and_then(|v| v.as_bool()) == Some(true) => {
                Some(Observation::AgentEndWillRetry)
            }
            "agent_settled" => Some(Observation::AgentSettled),
            _ => None,
        };
        match observation {
            Some(observation) => self.observe(observation, now),
            None if native_changed => {
                self.persist("native-activity-projection", now)?;
                Ok(Vec::new())
            }
            None => Ok(Vec::new()),
        }
    }

    fn project_native_activity(&mut self, value: &serde_json::Value, now: i64) -> bool {
        if self.state.terminal {
            return false;
        }
        let native = &mut self.state.native_activity;
        if native.process_epoch != self.state.process_epoch {
            let receipts = std::mem::take(&mut native.usage_receipts);
            let stream_offsets = std::mem::take(&mut native.stream_offsets);
            let totals = (
                native.usage_input,
                native.usage_output,
                native.usage_cache_read,
                native.usage_cache_write,
                native.usage_total,
                native.usage_cost.clone(),
                native.usage_receipt_count,
            );
            *native = NativeActivityProjection {
                process_epoch: self.state.process_epoch,
                usage_receipts: receipts,
                stream_offsets,
                usage_input: totals.0,
                usage_output: totals.1,
                usage_cache_read: totals.2,
                usage_cache_write: totals.3,
                usage_total: totals.4,
                usage_cost: totals.5,
                usage_receipt_count: totals.6,
                ..NativeActivityProjection::default()
            };
        }
        let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let mut changed = false;
        match ty {
            "turn_start" | "message_start" | "agent_end" => {
                native.event_seq = native.event_seq.saturating_add(1);
                native.last_activity_at = Some(now);
                changed = true;
            }
            "message_update" => {
                let event = value.get("assistantMessageEvent");
                match event.and_then(|v| v.get("type")).and_then(|v| v.as_str()) {
                    Some("thinking_start" | "thinking_delta" | "thinking_end") => {
                        native.event_seq = native.event_seq.saturating_add(1);
                        native.thinking_activity_seq =
                            native.thinking_activity_seq.saturating_add(1);
                        native.thinking_tokens = explicit_u64(
                            event,
                            &["thinkingTokens", "thinkingTokenCount", "tokenCount"],
                        );
                        native.last_activity_at = Some(now);
                        changed = true;
                    }
                    Some("text_delta" | "toolcall_start" | "toolcall_delta" | "toolcall_end") => {
                        native.event_seq = native.event_seq.saturating_add(1);
                        native.output_activity_seq = native.output_activity_seq.saturating_add(1);
                        native.last_activity_at = Some(now);
                        native.last_output_activity_at = Some(now);
                        native.output_tokens = explicit_u64(
                            event,
                            &["outputTokens", "outputTokenCount", "tokenCount"],
                        )
                        .or(native.output_tokens);
                        if let Some(tokens) = native.output_tokens {
                            if native
                                .output_samples
                                .last()
                                .is_some_and(|sample| tokens < sample.tokens)
                            {
                                native.output_samples.clear();
                            }
                            if native
                                .output_samples
                                .last()
                                .is_none_or(|sample| sample.at != now || sample.tokens != tokens)
                            {
                                native
                                    .output_samples
                                    .push(NativeTokenSample { at: now, tokens });
                            }
                            if native.output_samples.len() > 32 {
                                native.output_samples.remove(0);
                            }
                        }
                        changed = true;
                    }
                    _ => {}
                }
            }
            "tool_execution_start" => {
                native.event_seq = native.event_seq.saturating_add(1);
                native.last_activity_at = Some(now);
                let raw_name = value
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                native.current_tool_label = Some(safe_tool_label(raw_name));
                native.current_tool_class = Some(tool_class(value, raw_name));
                native.tool_progress = explicit_u64(Some(value), &["progress", "progressCount"]);
                native.tool_child_state = Some("started".into());
                native.tool_receipt_state = Some("pending".into());
                changed = true;
            }
            "tool_execution_update" => {
                native.event_seq = native.event_seq.saturating_add(1);
                native.last_activity_at = Some(now);
                native.tool_progress = explicit_u64(Some(value), &["progress", "progressCount"])
                    .or_else(|| native.tool_progress.map(|v| v.saturating_add(1)))
                    .or(Some(1));
                native.tool_child_state = Some(
                    value
                        .get("childState")
                        .and_then(|v| v.as_str())
                        .map(safe_state_label)
                        .unwrap_or_else(|| "running".into()),
                );
                changed = true;
            }
            "tool_execution_end" => {
                native.event_seq = native.event_seq.saturating_add(1);
                native.last_activity_at = Some(now);
                native.tool_child_state = Some("exited".into());
                native.tool_receipt_state = Some(
                    if value.get("isError").and_then(|v| v.as_bool()) == Some(true) {
                        "error"
                    } else {
                        "completed"
                    }
                    .into(),
                );
                native.current_tool_label = None;
                native.current_tool_class = None;
                changed = true;
            }
            "turn_end" => {
                if let Some(usage) = value.get("message").and_then(|m| m.get("usage")) {
                    let receipt = native_usage_receipt(value, usage);
                    if native.usage_receipts.insert(receipt) {
                        add_native_total(
                            &mut native.usage_input,
                            usage.get("input").and_then(|v| v.as_u64()),
                        );
                        add_native_total(
                            &mut native.usage_output,
                            usage.get("output").and_then(|v| v.as_u64()),
                        );
                        add_native_total(
                            &mut native.usage_cache_read,
                            usage.get("cacheRead").and_then(|v| v.as_u64()),
                        );
                        add_native_total(
                            &mut native.usage_cache_write,
                            usage.get("cacheWrite").and_then(|v| v.as_u64()),
                        );
                        add_native_total(
                            &mut native.usage_total,
                            usage.get("totalTokens").and_then(|v| v.as_u64()),
                        );
                        if let Some(cost) = usage
                            .get("cost")
                            .and_then(|c| c.get("total"))
                            .and_then(|v| v.as_f64())
                        {
                            let prior = native
                                .usage_cost
                                .as_deref()
                                .and_then(|v| v.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            native.usage_cost = Some(format!("{:.6}", prior + cost));
                        }
                        native.usage_receipt_count = native.usage_receipt_count.saturating_add(1);
                        native.event_seq = native.event_seq.saturating_add(1);
                        native.last_activity_at = Some(now);
                        changed = true;
                    }
                }
            }
            _ => {}
        }
        changed
    }

    pub fn observe(
        &mut self,
        observation: Observation,
        now: i64,
    ) -> Result<Vec<ActionKind>, WatchdogError> {
        if self.state.terminal {
            return match observation {
                Observation::TerminalIntent(ref r)
                    if self
                        .state
                        .terminal_receipt
                        .as_ref()
                        .is_some_and(|old| old.idempotency_key == r.idempotency_key) =>
                {
                    Ok(Vec::new())
                }
                Observation::TerminalIntent(_) => Err(WatchdogError::new(
                    "attempt_already_terminal",
                    "first terminal receipt already won",
                )),
                _ => Ok(Vec::new()),
            };
        }
        let mut actions = Vec::new();
        match observation {
            Observation::ProviderRequestStarted { call_id } => {
                self.state.phase = Phase::ProviderRequestInFlight;
                self.meaningful("provider-request-start", call_id.as_bytes(), now);
            }
            Observation::ProviderResponseStarted => {
                self.state.phase = Phase::ProviderResponseStream;
                self.meaningful("provider-response-start", b"response", now);
            }
            Observation::ProviderRetry => self.meaningful("provider-retry", b"retry", now),
            Observation::CompactionRetry => self.meaningful("compaction-retry", b"compact", now),
            Observation::QueuedFollowUp => self.meaningful("queued-follow-up", b"follow-up", now),
            Observation::AgentEndWillRetry => {
                self.meaningful("agent-end-will-retry", b"retry", now)
            }
            Observation::ThinkingDelta => self.meaningful("thinking-delta", b"thinking", now),
            Observation::TokenDelta { tokens } => {
                self.meaningful("token-delta", &tokens.to_le_bytes(), now)
            }
            Observation::ToolProgress {
                tool_call_id,
                progress,
            } => self.meaningful(
                "tool-progress",
                format!("{tool_call_id}:{progress}").as_bytes(),
                now,
            ),
            Observation::UsageReceipt { receipt } => {
                self.meaningful("usage-receipt", receipt.as_bytes(), now)
            }
            Observation::SessionAdvanced {
                leaf,
                prefix_digest,
            } => {
                self.state.session.branch_leaf = leaf;
                self.state.session.append_prefix_digest = prefix_digest;
                self.state.session.append_prefix_len += 1;
                self.meaningful("session-advanced", b"session", now);
            }
            Observation::WorktreeProgress { manifest_digest } => {
                self.meaningful("worktree-progress", manifest_digest.as_bytes(), now)
            }
            Observation::Heartbeat
            | Observation::StatusPolled
            | Observation::OrdinaryMessage
            | Observation::ProbeTraffic => {}
            Observation::ProbeObserved {
                progress_seq,
                session_leaf,
                alive: _,
            } => {
                if progress_seq != self.state.progress_seq
                    || session_leaf != self.state.session.branch_leaf
                {
                    self.hold("probe_new_or_mismatched_evidence");
                } else {
                    self.state.probe_observed_at = Some(now);
                    self.state.reason_code = Some("probe_no_progress".into());
                }
            }
            Observation::PhaseUnknown => {
                self.state.phase = Phase::Unknown;
                self.state.hard_resume_after_secs = None;
            }
            Observation::AgentSettled => {
                self.state.phase = Phase::Settled;
                actions = self.needs_finalization("needs_finalization_settled", now, true)?;
            }
            Observation::ProcessExited { status, reaped } => {
                self.state.phase = Phase::Exited;
                let reason = match status {
                    ExitStatus::Code(0) => "process_exit_zero_no_terminal",
                    ExitStatus::Code(_) => "process_exit_nonzero_no_terminal",
                    ExitStatus::Signal(_) => "needs_finalization_exit",
                    ExitStatus::Unknown => "reap_unproven",
                };
                actions = self.needs_finalization(reason, now, reaped)?;
            }
            Observation::PipeEof { reaped } => {
                self.state.phase = Phase::Exited;
                actions = self.needs_finalization("pipe_eof_no_terminal", now, reaped)?;
            }
            Observation::ToolIntent { contract } => {
                self.state.phase = Phase::Tool;
                self.state.classification = Classification::LongTool;
                self.state.exact_guards.effect = contract.is_safe();
                self.state.tool = Some(contract);
                self.meaningful("tool-intent", b"intent", now);
            }
            Observation::ToolReceipt {
                tool_call_id,
                receipt,
            } => {
                if let Some(tool) = self
                    .state
                    .tool
                    .as_mut()
                    .filter(|t| t.tool_call_id == tool_call_id)
                {
                    tool.completion_receipt = Some(receipt);
                    self.state.exact_guards.effect = tool.is_safe();
                    self.meaningful("tool-receipt", tool_call_id.as_bytes(), now);
                }
            }
            Observation::ToolCompleted {
                tool_call_id,
                receipt,
            } => {
                if let Some(tool) = self.state.tool.as_mut() {
                    tool.completion_receipt = Some(receipt);
                }
                self.state.tool = None;
                self.state.exact_guards.effect = true;
                self.state.classification = Classification::Active;
                self.state.phase = Phase::Unknown;
                self.meaningful("tool-complete", tool_call_id.as_bytes(), now);
            }
            Observation::WaitAccepted { correlation } => {
                self.state.classification = Classification::WaitingUser;
                self.state.wait_correlation = Some(correlation);
                self.state.reason_code = Some("wait_parked".into());
                self.cancel_pending();
            }
            Observation::TerminalIntent(receipt) => {
                actions = self.accept_terminal(receipt)?;
            }
            Observation::GuardFailure(guard) => {
                match guard {
                    GuardFailure::Session => self.state.exact_guards.session = false,
                    GuardFailure::Route => self.state.exact_guards.route = false,
                    GuardFailure::Worktree => self.state.exact_guards.worktree = false,
                    GuardFailure::PidIdentity => self.state.exact_guards.pid_identity = false,
                    GuardFailure::Containment => self.state.exact_guards.containment = false,
                    GuardFailure::Effect => self.state.exact_guards.effect = false,
                    GuardFailure::TerminalReservation => {
                        self.state.exact_guards.terminal_clear = false
                    }
                };
                self.hold(match guard {
                    GuardFailure::Session => "session_head_mismatch",
                    GuardFailure::Route => "route_mismatch",
                    GuardFailure::Worktree => "worktree_mismatch",
                    GuardFailure::PidIdentity => "pid_identity_ambiguous",
                    GuardFailure::Containment => "process_group_not_quiescent",
                    GuardFailure::Effect => "ambiguous_tool_side_effect",
                    GuardFailure::TerminalReservation => "terminal_won",
                });
            }
            Observation::PromptMarkerUncertain => self.hold("prompt_marker_uncertain"),
            Observation::ContinuationLaunched => {
                self.state.classification = Classification::Resuming;
                self.state.reason_code = Some("continuation_process_started_gated".into());
            }
            Observation::ExecutionPermitted => {
                self.state.phase = Phase::ProviderRequestInFlight;
                self.meaningful("continuation-execution-permit", b"permit", now);
            }
            Observation::ReplayPendingActions => { /* replay observes durable completion markers; it never blindly re-emits */
            }
        }
        self.persist("observation", now)?;
        Ok(actions)
    }

    pub fn tick(&mut self, now: i64) -> Result<Vec<ActionKind>, WatchdogError> {
        if self.state.terminal || self.state.classification == Classification::WaitingUser {
            return Ok(Vec::new());
        }
        if self
            .state
            .tool
            .as_ref()
            .and_then(|tool| tool.lease_expires_at)
            .is_some_and(|expires| now < expires)
        {
            self.state.classification = Classification::LongTool;
            return Ok(Vec::new());
        }
        let open_tool_blocks_resume = self.state.tool.as_ref().is_some_and(|tool| !tool.is_safe());
        let silence = now.saturating_sub(self.state.last_meaningful_at) as u64;
        let mut actions = Vec::new();
        if self.state.suspect_at.is_none() && silence >= self.policy.meaningful_silence_secs {
            self.state.classification = Classification::Suspect;
            self.state.suspect_at = Some(now);
            self.state.reason_code = Some("meaningful_silence_soft_suspect".into());
            let id = self.action_id(ActionKind::ReadOnlyProbe);
            self.state.probe_action_id = Some(id.clone());
            if self.complete_action(id, ActionKind::ReadOnlyProbe) {
                actions.push(ActionKind::ReadOnlyProbe);
            }
            if self.state.phase == Phase::Unknown {
                self.state.reason_code = Some("probe_no_progress".into());
            }
        }
        if open_tool_blocks_resume {
            // Still apply the meaningful-progress suspicion clock, but never
            // turn total tool runtime into a kill/resume deadline. The native
            // projection continues to show the bounded live tool state.
            self.state.hard_resume_after_secs = None;
            if self.state.suspect_at.is_none() {
                self.state.classification = Classification::LongTool;
            }
            self.persist("open-tool-observed", now)?;
            return Ok(actions);
        }
        let hard = self
            .policy
            .hard_secs(self.state.phase, self.state.route.qos);
        self.state.hard_resume_after_secs = hard;
        if hard.is_none() {
            if self.state.suspect_at.is_some() && silence > self.policy.meaningful_silence_secs {
                self.hold("unknown_phase_no_hard_policy");
            }
            self.persist("tick", now)?;
            return Ok(actions);
        }
        let hard = hard.unwrap();
        if self.state.hard_eligible_at.is_none() && silence >= hard {
            self.state.classification = Classification::HardResumeEligible;
            self.state.hard_eligible_at = Some(now);
            self.state.hard_grace_deadline = Some(now + self.policy.hard_resume_grace_secs as i64);
            self.state.reason_code = Some("hard_resume_phase_eligible".into());
            actions.push(ActionKind::StartHardGrace);
        }
        if self
            .state
            .hard_grace_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            if self
                .state
                .probe_observed_at
                .is_none_or(|at| at < self.state.hard_eligible_at.unwrap_or(now))
            {
                self.persist("await-fresh-probe", now)?;
                return Ok(actions);
            }
            if !self.state.exact_guards.all() {
                self.hold("continuation_guard_failed");
                self.persist("guard-hold", now)?;
                return Ok(actions);
            }
            if !self.budget_available(false) {
                self.hold(self.budget_reason());
                self.persist("budget-hold", now)?;
                return Ok(actions);
            }
            self.reserve_epoch(false, now)?;
            self.state.classification = Classification::Fencing;
            actions.push(ActionKind::ReserveContinuation);
            actions.push(ActionKind::FenceExactProcess);
        }
        self.persist("tick", now)?;
        Ok(actions)
    }

    pub fn manual_resume(
        &mut self,
        grant: ManualGrant,
        now: i64,
    ) -> Result<Vec<ActionKind>, WatchdogError> {
        if grant.reason.trim().is_empty() || grant.epochs == 0 || grant.elapsed_secs == 0 {
            return Err(WatchdogError::new(
                "invalid_manual_grant",
                "manual continuation grant must be reasoned and finite",
            ));
        }
        if !self.state.manual_grant_ids.insert(grant.action_id.clone()) {
            return Ok(Vec::new());
        }
        if let Some(ack) = grant.effect_ack {
            if let Some(tool) = self
                .state
                .tool
                .as_mut()
                .filter(|t| t.tool_call_id == ack.tool_call_id)
            {
                tool.completion_receipt = Some(ack.receipt);
                self.state.exact_guards.effect = true;
            }
        }
        self.state.manual_epochs_granted = self
            .state
            .manual_epochs_granted
            .saturating_add(grant.epochs);
        self.state.manual_elapsed_granted_secs = self
            .state
            .manual_elapsed_granted_secs
            .saturating_add(grant.elapsed_secs);
        self.state.reason_code = Some("operator_resume".into());
        self.persist("manual-grant", now)?;
        Ok(Vec::new())
    }

    /// Complete a persisted same-process prompt intent after restart. This is
    /// the crash-safe outbox consumer for both boundaries: epoch reserved but
    /// prompt intent absent, and prompt intent persisted but marker absent.
    pub fn reconcile_pending_same_process_prompt(
        &mut self,
        now: i64,
    ) -> Result<bool, WatchdogError> {
        if self.state.terminal
            || self.state.classification != Classification::NeedsFinalization
            || self.state.prompt_marker.is_some()
            || self.state.continuation_epoch == 0
            || self.state.prompt_count >= self.state.continuation_epoch
        {
            return Ok(false);
        }
        let reason = self
            .state
            .reason_code
            .clone()
            .unwrap_or_else(|| "needs_finalization_restart".into());
        self.emit_completion_prompt(&reason, now)?;
        Ok(true)
    }

    pub fn quiescence_receipt(&self, manifest: impl Into<String>, now: i64) -> PiQuiescenceReceipt {
        PiQuiescenceReceipt {
            source: self.state.source.clone(),
            process_epoch: self.state.process_epoch,
            process_identity_digest: self.state.process.digest(),
            final_session_head: self.state.session.branch_leaf.clone(),
            final_worktree_manifest_digest: manifest.into(),
            process_group_empty: true,
            nonce_pipe_eof: true,
            reaped_at: now,
        }
    }
    pub fn inject_crash_barrier(&mut self, barrier: CrashBarrier) -> Result<(), WatchdogError> {
        self.crash_barrier = Some(barrier);
        Ok(())
    }

    fn meaningful(&mut self, kind: &str, bytes: &[u8], now: i64) {
        self.state.progress_seq += 1;
        self.state.progress_digest = digest_bytes(
            format!(
                "{}:{}:{}",
                self.state.progress_digest,
                kind,
                digest_bytes(bytes)
            )
            .as_bytes(),
        );
        self.state.last_meaningful_at = now;
        self.state.last_meaningful_kind = kind.into();
        self.state.suspect_at = None;
        self.state.probe_action_id = None;
        self.state.probe_observed_at = None;
        self.state.hard_eligible_at = None;
        self.state.hard_grace_deadline = None;
        if self.state.classification != Classification::LongTool {
            self.state.classification = Classification::Active;
        }
    }

    fn needs_finalization(
        &mut self,
        reason: &str,
        now: i64,
        reaped: bool,
    ) -> Result<Vec<ActionKind>, WatchdogError> {
        if self.state.prompt_marker.is_some() {
            self.state.classification = Classification::NeedsFinalization;
            return Ok(Vec::new());
        }
        self.state.classification = Classification::NeedsFinalization;
        self.state.reason_code = Some(reason.into());
        if !reaped {
            self.hold("reap_unproven");
            return Ok(Vec::new());
        }
        if !self.state.exact_guards.effect || self.state.tool.as_ref().is_some_and(|t| !t.is_safe())
        {
            self.hold("ambiguous_tool_side_effect");
            return Ok(Vec::new());
        }
        if !self.state.exact_guards.session
            || !self.state.exact_guards.route
            || !self.state.exact_guards.worktree
        {
            self.hold("continuation_guard_failed");
            return Ok(Vec::new());
        }
        if !self.budget_available(false) {
            let reason = self.budget_reason().to_string();
            self.hold(&reason);
            return Ok(Vec::new());
        }
        self.reserve_epoch(false, now)?;
        if self.crash_barrier == Some(CrashBarrier::AfterContinuationReserved) {
            return Err(WatchdogError::new(
                "injected_crash",
                "after continuation reserved",
            ));
        }
        self.emit_completion_prompt(reason, now)?;
        Ok(vec![
            ActionKind::ReserveContinuation,
            ActionKind::LaunchSameSession,
            ActionKind::AppendCompletionPrompt,
        ])
    }

    fn emit_completion_prompt(&mut self, reason: &str, now: i64) -> Result<(), WatchdogError> {
        let prompt = render_stock_prompt(reason)?;
        let digest = digest_bytes(prompt.as_bytes());
        let action_id = format!(
            "prompt:{}:{}:{}:{}:{}",
            self.state.source.attempt_id,
            self.state.process_epoch,
            self.state.continuation_epoch,
            PROMPT_VERSION,
            digest
        );
        self.state.prompt_action_id = Some(action_id.clone());
        self.state.prompt_digest = Some(digest);
        self.persist("prompt-intent", now)?;
        if self.crash_barrier == Some(CrashBarrier::AfterPromptIntent) {
            return Err(WatchdogError::new("injected_crash", "after prompt intent"));
        }
        if !self.session_has_marker(&action_id)? {
            self.append_session_marker(&action_id, reason, &prompt)?;
            let bytes = fs::read(&self.state.session.session_file).map_err(io_error)?;
            self.state.session.branch_leaf = digest_bytes(&bytes);
            self.state.session.append_prefix_digest = digest_bytes(&bytes);
            self.state.session.append_prefix_len = bytes.len() as u64;
        }
        if self.state.completed_action_ids.insert(action_id.clone()) {
            self.state.prompt_marker = Some(action_id);
            self.state.prompt_count += 1;
        }
        self.persist("prompt-observed", now)?;
        Ok(())
    }

    fn session_has_marker(&self, action_id: &str) -> Result<bool, WatchdogError> {
        let bytes = fs::read(&self.state.session.session_file)
            .map_err(|error| WatchdogError::new("prompt_marker_uncertain", error.to_string()))?;
        let needle = format!("\"actionId\":\"{action_id}\"");
        Ok(String::from_utf8_lossy(&bytes).contains(&needle))
    }

    fn append_session_marker(
        &self,
        action_id: &str,
        reason: &str,
        prompt: &str,
    ) -> Result<(), WatchdogError> {
        let entry = serde_json::json!({
            "type": "custom",
            "customType": "wg-pi-continuation",
            "actionId": action_id,
            "promptVersion": PROMPT_VERSION,
            "observationCode": reason,
            "promptDigest": digest_bytes(prompt.as_bytes()),
            "processEpoch": self.state.process_epoch,
            "continuationEpoch": self.state.continuation_epoch,
        });
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.state.session.session_file)
            .map_err(|error| WatchdogError::new("prompt_marker_uncertain", error.to_string()))?;
        serde_json::to_writer(&mut file, &entry).map_err(json_error)?;
        file.write_all(b"\n")
            .and_then(|_| file.sync_all())
            .map_err(io_error)
    }

    fn accept_terminal(
        &mut self,
        receipt: TerminalIntentReceipt,
    ) -> Result<Vec<ActionKind>, WatchdogError> {
        if !receipt_matches(&self.state.source, &receipt) {
            return Err(WatchdogError::new(
                "stale_attempt",
                "terminal receipt source tuple does not match",
            ));
        }
        if receipt.process_epoch != self.state.process_epoch
            || (!receipt.process_identity_digest.is_empty()
                && receipt.process_identity_digest != self.state.process.digest())
        {
            return Err(WatchdogError::new(
                "stale_process_epoch",
                "terminal receipt came from an old process authority",
            ));
        }
        self.state.terminal = true;
        self.state.terminal_receipt = Some(receipt.clone());
        self.cancel_pending();
        self.state.reason_code = Some(
            match receipt.disposition {
                TerminalDisposition::SuccessIntent => "success_intent",
                TerminalDisposition::Failure => "failure_intent",
                TerminalDisposition::Park => "wait_parked",
                TerminalDisposition::Cancel => "cancelled",
                TerminalDisposition::Abort => "operator_abort",
            }
            .into(),
        );
        Ok(
            if receipt.disposition == TerminalDisposition::SuccessIntent {
                vec![ActionKind::QuiesceForFinalization]
            } else {
                Vec::new()
            },
        )
    }

    fn reserve_epoch(&mut self, manual: bool, now: i64) -> Result<(), WatchdogError> {
        if !self.budget_available(manual) {
            return Err(WatchdogError::new(
                self.budget_reason(),
                "finite continuation budget exhausted",
            ));
        }
        self.state.epochs_used += 1;
        self.state.elapsed_reserved_secs = self
            .state
            .elapsed_reserved_secs
            .saturating_add(self.policy.continuation_epoch_lease_secs);
        self.state.continuation_epoch += 1;
        // A prompt delivered inside the same live Pi process is not a process
        // replacement. Advancing the process fence here makes that exact
        // writer's later terminal/exit receipt appear stale and strands WIP.
        self.persist("continuation-epoch-reserved", now)
    }
    fn budget_available(&self, _manual: bool) -> bool {
        let epoch_limit = self
            .policy
            .max_continuation_epochs
            .saturating_add(self.state.manual_epochs_granted);
        let elapsed_limit = self
            .policy
            .max_continuation_elapsed_secs
            .saturating_add(self.state.manual_elapsed_granted_secs);
        self.state.epochs_used < epoch_limit
            && self
                .state
                .elapsed_reserved_secs
                .saturating_add(self.policy.continuation_epoch_lease_secs)
                <= elapsed_limit
    }
    fn budget_reason(&self) -> &'static str {
        if self.state.epochs_used
            >= self
                .policy
                .max_continuation_epochs
                .saturating_add(self.state.manual_epochs_granted)
        {
            "continuation_epoch_budget_exhausted"
        } else {
            "continuation_elapsed_budget_exhausted"
        }
    }
    fn hold(&mut self, reason: &str) {
        self.state.classification = Classification::StalledOperatorRequired;
        self.state.reason_code = Some(reason.into());
    }
    fn cancel_pending(&mut self) {
        for action in &mut self.state.pending_actions {
            if action.state == ActionState::Pending {
                action.state = ActionState::Cancelled;
            }
        }
    }
    fn action_id(&self, kind: ActionKind) -> String {
        format!(
            "{}:{}:{}:{}:{kind:?}",
            self.state.source.attempt_id,
            self.state.process_epoch,
            self.state.continuation_epoch,
            self.state.progress_seq
        )
    }
    fn complete_action(&mut self, id: String, kind: ActionKind) -> bool {
        if self.state.completed_action_ids.insert(id.clone()) {
            self.state.pending_actions.push(ActionRecord {
                action_id: id,
                kind,
                state: ActionState::Completed,
            });
            true
        } else {
            false
        }
    }

    fn persist(&mut self, event: &str, now: i64) -> Result<(), WatchdogError> {
        let payload = Persisted {
            state: self.state.clone(),
            policy: self.policy.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&payload).map_err(json_error)?;
        atomic_write(&self.state_path, &bytes)?;
        let frame = JournalFrame {
            schema_version: 1,
            at: now,
            event: event.into(),
            state_digest: digest_bytes(&bytes),
            previous_digest: journal_last_digest(&self.journal_path).unwrap_or_default(),
        };
        let mut line = serde_json::to_vec(&frame).map_err(json_error)?;
        let checksum = digest_bytes(&line);
        line.extend_from_slice(format!("\t{checksum}\n").as_bytes());
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.journal_path)
            .map_err(io_error)?;
        file.write_all(&line)
            .and_then(|_| file.sync_all())
            .map_err(io_error)
    }
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    state: PiWatchdogState,
    policy: WatchdogPolicy,
}
#[derive(Serialize, Deserialize)]
struct JournalFrame {
    schema_version: u32,
    at: i64,
    event: String,
    state_digest: String,
    previous_digest: String,
}

/// Select the one resumable journal for `session_id` without changing the
/// directory. Pi may leave WG's bootstrap header beside the timestamped file
/// it actually appends. Header-only matches are evidence, not competing
/// sessions, once exactly one substantive journal exists.
pub fn select_canonical_session_journal(
    session_dir: &Path,
    session_id: &str,
) -> Result<CanonicalSessionJournal, WatchdogError> {
    let entries = fs::read_dir(session_dir).map_err(io_error)?;
    let mut headers = Vec::new();
    let mut substantive = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        if !entry.file_type().map_err(io_error)?.is_file()
            || path.extension().and_then(|v| v.to_str()) != Some("jsonl")
        {
            continue;
        }
        let bytes = fs::read(&path).map_err(io_error)?;
        let first_end = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(bytes.len());
        let header_bytes = &bytes[..first_end];
        let Ok(header) = serde_json::from_slice::<serde_json::Value>(header_bytes) else {
            continue;
        };
        if header.get("type").and_then(|v| v.as_str()) != Some("session")
            || header.get("id").and_then(|v| v.as_str()) != Some(session_id)
        {
            continue;
        }
        let header_json = String::from_utf8_lossy(header_bytes).into_owned();
        let is_substantive = bytes
            .get(first_end.saturating_add(1)..)
            .is_some_and(|tail| tail.iter().any(|byte| !byte.is_ascii_whitespace()));
        let candidate = CanonicalSessionJournal {
            session_file: path,
            header_json,
            header_digest: digest_bytes(header_bytes),
            branch_leaf: digest_bytes(&bytes),
            append_prefix_digest: digest_bytes(&bytes),
            append_prefix_len: bytes.len() as u64,
            substantive: is_substantive,
            bootstrap_evidence: Vec::new(),
        };
        if is_substantive {
            substantive.push(candidate);
        } else {
            headers.push(candidate);
        }
    }
    if substantive.len() > 1 {
        return Err(WatchdogError::new(
            "ambiguous_substantive_session_journals",
            format!(
                "exact Pi session has {} substantive journals; refusing continuation without deleting evidence",
                substantive.len()
            ),
        ));
    }
    let mut selected = if let Some(selected) = substantive.pop() {
        selected
    } else {
        match headers.len() {
            1 => headers.pop().unwrap(),
            0 => {
                return Err(WatchdogError::new(
                    "session_journal_missing",
                    "no matching Pi session journal exists",
                ));
            }
            count => {
                return Err(WatchdogError::new(
                    "ambiguous_header_only_session_journals",
                    format!(
                        "exact Pi session has {count} header-only journals and no substantive journal"
                    ),
                ));
            }
        }
    };
    selected.bootstrap_evidence = headers
        .into_iter()
        .map(|header| (header.append_prefix_len, header.append_prefix_digest))
        .collect();
    Ok(selected)
}

pub fn render_stock_prompt(observation_code: &str) -> Result<String, WatchdogError> {
    const ALLOWED: &[&str] = &[
        "needs_finalization_settled",
        "needs_finalization_exit",
        "process_exit_zero_no_terminal",
        "process_exit_nonzero_no_terminal",
        "pipe_eof_no_terminal",
        "no_meaningful_progress_since_sequence",
        "operator_resume",
    ];
    if !ALLOWED.contains(&observation_code) {
        return Err(WatchdogError::new(
            "invalid_observation_code",
            "stock prompt accepts bounded reason codes only",
        ));
    }
    Ok(STOCK_PROMPT_TEMPLATE.replace("<OBSERVATION_CODE>", observation_code))
}

fn explicit_u64(value: Option<&serde_json::Value>, keys: &[&str]) -> Option<u64> {
    let value = value?;
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_u64()))
}

fn safe_tool_label(raw: &str) -> String {
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "bash" | "shell" | "read" | "write" | "edit" | "apply_patch" | "cargo" | "test"
        | "pytest" | "npm" | "make" => normalized,
        _ => format!("tool#{}", &digest_bytes(raw.as_bytes())[..8]),
    }
}

fn safe_state_label(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "started" => "started".into(),
        "running" => "running".into(),
        "waiting" => "waiting".into(),
        "exited" => "exited".into(),
        "completed" => "completed".into(),
        _ => "unknown".into(),
    }
}

fn tool_class(value: &serde_json::Value, raw_name: &str) -> String {
    match value
        .get("toolClass")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "test" | "testing" => return "test".into(),
        "write" | "writing" => return "write".into(),
        "edit" => return "edit".into(),
        "read" | "read-only" => return "read".into(),
        _ => {}
    }
    match raw_name.trim().to_ascii_lowercase().as_str() {
        "test" | "pytest" => "test",
        "write" => "write",
        "edit" | "apply_patch" => "edit",
        "read" => "read",
        _ => "tool",
    }
    .into()
}

fn native_usage_receipt(value: &serde_json::Value, usage: &serde_json::Value) -> String {
    let identity = value
        .get("turnId")
        .or_else(|| value.get("message").and_then(|m| m.get("id")))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let safe = serde_json::json!({
                "input": usage.get("input"),
                "output": usage.get("output"),
                "cacheRead": usage.get("cacheRead"),
                "cacheWrite": usage.get("cacheWrite"),
                "totalTokens": usage.get("totalTokens"),
                "cost": usage.get("cost").and_then(|c| c.get("total")),
                "timestamp": value.get("timestamp"),
            });
            safe.to_string()
        });
    digest_bytes(identity.as_bytes())
}

fn add_native_total(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0).saturating_add(value));
    }
}

fn receipt_matches(source: &SourceTuple, receipt: &TerminalIntentReceipt) -> bool {
    receipt.task_id == source.task_id
        && receipt.generation == source.generation
        && receipt.attempt_id == source.attempt_id
        && receipt.attempt_fence == source.attempt_fence
}
fn digest_json<T: Serialize>(value: &T) -> String {
    digest_bytes(&serde_json::to_vec(value).unwrap_or_default())
}
fn digest_bytes(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}
fn io_error(error: std::io::Error) -> WatchdogError {
    WatchdogError::new("watchdog_io", error.to_string())
}
fn json_error(error: serde_json::Error) -> WatchdogError {
    WatchdogError::new("watchdog_json", error.to_string())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), WatchdogError> {
    let parent = path
        .parent()
        .ok_or_else(|| WatchdogError::new("watchdog_io", "state path has no parent"))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let tmp = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(io_error)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(io_error)?;
    fs::rename(&tmp, path).map_err(io_error)?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}
fn journal_last_digest(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let line = bytes.split(|b| *b == b'\n').rfind(|v| !v.is_empty())?;
    line.split(|b| *b == b'\t')
        .nth(1)
        .and_then(|v| std::str::from_utf8(v).ok())
        .map(str::to_owned)
}
