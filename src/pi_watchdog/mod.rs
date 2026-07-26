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
                schema_version: 1,
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

    /// Ingest one native Pi JSON/RPC record without letting ordinary log or
    /// heartbeat traffic manufacture progress. Unknown records are inert.
    pub fn ingest_native_value(
        &mut self,
        value: &serde_json::Value,
        now: i64,
    ) -> Result<Vec<ActionKind>, WatchdogError> {
        let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let observation = match ty {
            "provider_request_start" | "model_request_start" => {
                Some(Observation::ProviderRequestStarted {
                    call_id: value
                        .get("callId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("provider")
                        .to_string(),
                })
            }
            "provider_retry" => Some(Observation::ProviderRetry),
            "compaction_retry" => Some(Observation::CompactionRetry),
            "message_update" => match value
                .get("assistantMessageEvent")
                .and_then(|v| v.get("type"))
                .and_then(|v| v.as_str())
            {
                Some("thinking_delta") => Some(Observation::ThinkingDelta),
                Some("text_delta") => Some(Observation::TokenDelta { tokens: 1 }),
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
            "agent_end" if value.get("willRetry").and_then(|v| v.as_bool()) == Some(true) => {
                Some(Observation::AgentEndWillRetry)
            }
            "agent_settled" => Some(Observation::AgentSettled),
            _ => None,
        };
        match observation {
            Some(observation) => self.observe(observation, now),
            None => Ok(Vec::new()),
        }
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
        if let Some(tool) = &self.state.tool {
            if tool.lease_expires_at.is_some_and(|expires| now < expires) {
                self.state.classification = Classification::LongTool;
                return Ok(Vec::new());
            }
            if !tool.is_safe() {
                self.hold("ambiguous_tool_side_effect");
                self.persist("tool-expired-hold", now)?;
                return Ok(Vec::new());
            }
        }
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
        }
        if self.state.completed_action_ids.insert(action_id.clone()) {
            self.state.prompt_marker = Some(action_id);
            self.state.prompt_count += 1;
        }
        self.persist("prompt-observed", now)?;
        Ok(vec![
            ActionKind::ReserveContinuation,
            ActionKind::LaunchSameSession,
            ActionKind::AppendCompletionPrompt,
        ])
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
        if receipt.process_epoch != self.state.process_epoch {
            return Err(WatchdogError::new(
                "stale_process_epoch",
                "terminal receipt came from an old process epoch",
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
        self.state.process_epoch += 1;
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
