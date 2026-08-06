use anyhow::{Context, Result};
use chrono::Utc;
use std::fs::{File, OpenOptions};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::pi_watchdog::{
    EffectAcknowledgement, ExitStatus, ManualGrant, Observation, PiWatchdog, ProcessIdentity,
    QosClass, RouteSnapshot, SessionProof, SourceTuple, TerminalDisposition, TerminalIntentReceipt,
    ToolContract, WatchdogPolicy,
};

use crate::cli::PiWatchdogCommands;

pub fn run(dir: &Path, command: PiWatchdogCommands, json: bool) -> Result<()> {
    match command {
        PiWatchdogCommands::Status { id } => status(dir, &id, json),
        PiWatchdogCommands::Resume {
            id,
            reason,
            grant_epochs,
            grant_elapsed_secs,
            ack_call,
            disposition,
            receipt,
        } => resume(
            dir,
            &id,
            reason,
            grant_epochs,
            grant_elapsed_secs,
            ack_call,
            disposition,
            receipt,
            json,
        ),
        PiWatchdogCommands::Abort { id, reason } => abort(dir, &id, &reason, json),
        PiWatchdogCommands::Bootstrap {
            id,
            agent_dir,
            pid,
            wrapper_pid,
        } => bootstrap(dir, &id, &agent_dir, pid, wrapper_pid),
        PiWatchdogCommands::ProcessExit { id, exit_code, pid } => {
            process_exit(dir, &id, exit_code, pid)
        }
        PiWatchdogCommands::CompactionKick { command } => {
            let _ = command;
            anyhow::bail!(
                "worker_control.capability_required: Pi compaction-kick operations are not operator commands"
            )
        }
        PiWatchdogCommands::FixtureInit { id, worktree, now } => {
            fixture_init(dir, &id, &worktree, now)
        }
        PiWatchdogCommands::FixtureObserve { id, event, now } => {
            fixture_observe(dir, &id, &event, now)
        }
        PiWatchdogCommands::FixtureTick { id, now } => fixture_tick(dir, &id, now),
    }
}

fn state_path(dir: &Path, task_id: &str) -> Result<PathBuf> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(task_id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("task has no current attempt")?;
    let runtime_key = worksgood::attempt_runtime::AttemptRuntimeKey::for_attempt(task, attempt);
    let canonical = worksgood::attempt_runtime::component_for_update(dir, &runtime_key, "pi")?
        .join("state.json");
    if canonical.exists() {
        return Ok(canonical);
    }
    // Compatibility for attempts started while the isolated-worktree observer
    // and watchdog roots were landing in separate commits.
    if let Some(agent) = task.assigned.as_deref() {
        let root = dir.parent().unwrap_or(dir);
        let compatibility = root
            .join(".wg-worktrees")
            .join(agent)
            .join(".wg-pi-watchdog/state.json");
        if compatibility.exists() {
            return Ok(compatibility);
        }
    }
    anyhow::bail!(
        "no Pi watchdog state for current attempt {} (expected {})",
        attempt.id,
        canonical.display()
    )
}

/// Serializes the complete read/verify/mutate/persist transaction. The raw
/// stream observer and the in-process plugin broker are distinct processes;
/// atomic rename alone prevents torn JSON but cannot prevent a stale observer
/// from overwriting a newly authorized kick between authorize and permit.
pub(crate) struct LockedWatchdog {
    watchdog: PiWatchdog,
    _lock: WatchdogTransactionLock,
}

impl Deref for LockedWatchdog {
    type Target = PiWatchdog;

    fn deref(&self) -> &Self::Target {
        &self.watchdog
    }
}

impl DerefMut for LockedWatchdog {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.watchdog
    }
}

struct WatchdogTransactionLock {
    #[cfg(unix)]
    file: File,
}

impl WatchdogTransactionLock {
    fn acquire(state_path: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;

            let lock_path = state_path.with_file_name("transaction.lock");
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .with_context(|| format!("open Pi watchdog lock {}", lock_path.display()))?;
            loop {
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                    break;
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error).with_context(|| {
                        format!("acquire Pi watchdog lock {}", lock_path.display())
                    });
                }
            }
            Ok(Self { file })
        }
        #[cfg(not(unix))]
        {
            let _ = state_path;
            Ok(Self {})
        }
    }
}

impl Drop for WatchdogTransactionLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

pub(crate) fn checked_open(dir: &Path, task_id: &str) -> Result<LockedWatchdog> {
    let path = state_path(dir, task_id)?;
    let lock = WatchdogTransactionLock::acquire(&path)?;
    let mut watchdog = PiWatchdog::open(&path).map_err(anyhow::Error::new)?;
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(task_id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("task has no current attempt")?;
    let source = &watchdog.state().source;
    if source.task_id != task.id
        || source.generation != task.lifecycle.generation
        || source.attempt_id != attempt.id
        || source.attempt_fence != task.lifecycle.fence
    {
        anyhow::bail!("stale_attempt: watchdog source tuple does not match the lifecycle kernel")
    }
    sync_lifecycle_process_authority(dir, task_id, &mut watchdog)?;
    watchdog
        .reconcile_pending_same_process_prompt(Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    reconcile_compaction_permit_outbox(dir, task_id, &mut watchdog)?;
    reconcile_compaction_action_outbox(dir, task_id, &mut watchdog)?;
    sync_lifecycle_continuation_authority(dir, task_id, &watchdog)?;
    Ok(LockedWatchdog {
        watchdog,
        _lock: lock,
    })
}

/// Repair the deliberate graph-CAS -> watchdog-outbox crash split. A lifecycle
/// epoch exactly one ahead is accepted only when its audit key names the one
/// immutable Authorized compaction action for that epoch. Reconciliation never
/// recreates fresh delivery authority: the original permit reply may have
/// crossed the process boundary before the crash.
fn reconcile_compaction_permit_outbox(
    dir: &Path,
    task_id: &str,
    watchdog: &mut PiWatchdog,
) -> Result<()> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(task_id)?;
    let lifecycle_epoch = task.lifecycle.pi_continuation_epoch;
    let watchdog_epoch = watchdog.state().continuation_epoch;
    if lifecycle_epoch <= watchdog_epoch {
        return Ok(());
    }
    if lifecycle_epoch != watchdog_epoch.saturating_add(1) {
        anyhow::bail!(
            "stale_continuation_epoch: lifecycle is more than one epoch ahead of watchdog"
        );
    }
    let Some(record) = watchdog
        .state()
        .compaction_kicks
        .iter()
        .find(|record| {
            record.state == worksgood::pi_watchdog::PiCompactionKickState::Authorized
                && record.authorized_from_continuation_epoch == watchdog_epoch
                && record.to_continuation_epoch == lifecycle_epoch
                && task
                    .lifecycle
                    .audit
                    .iter()
                    .any(|event| event.idempotency_key == record.action_id)
        })
        .cloned()
    else {
        anyhow::bail!(
            "stale_continuation_epoch: lifecycle is ahead without an audited compaction permit"
        );
    };
    watchdog
        .permit_compaction_kick(
            &record.action_id,
            lifecycle_epoch,
            false,
            Utc::now().timestamp(),
        )
        .map_err(anyhow::Error::new)?;
    Ok(())
}

/// Reconcile lifecycle-first compaction terminal bookkeeping into the
/// watchdog. Settlement deliberately commits the lifecycle removal before the
/// diagnostic projection; a crash between those writes must be repaired on
/// reopen rather than reported as a successful but split settlement.
fn reconcile_compaction_action_outbox(
    dir: &Path,
    task_id: &str,
    watchdog: &mut PiWatchdog,
) -> Result<()> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(task_id)?;
    let terminal = watchdog
        .state()
        .compaction_kicks
        .iter()
        .filter(|record| {
            matches!(
                record.state,
                worksgood::pi_watchdog::PiCompactionKickState::Acknowledged
                    | worksgood::pi_watchdog::PiCompactionKickState::Running
            ) && task
                .lifecycle
                .pi_kick_revoked_actions
                .contains(&record.action_id)
        })
        .map(|record| record.action_id.clone())
        .collect::<Vec<_>>();
    for action_id in terminal {
        watchdog
            .acknowledge_compaction_kick(&action_id, true, Utc::now().timestamp())
            .map_err(anyhow::Error::new)?;
    }
    let settled = watchdog
        .state()
        .compaction_kicks
        .iter()
        .filter(|record| {
            matches!(
                record.state,
                worksgood::pi_watchdog::PiCompactionKickState::Acknowledged
                    | worksgood::pi_watchdog::PiCompactionKickState::Running
            ) && !task
                .lifecycle
                .pi_kick_revoked_actions
                .contains(&record.action_id)
                && task.lifecycle.audit.iter().any(|event| {
                    event.idempotency_key == format!("pi-kick-settle:{}", record.action_id)
                })
        })
        .map(|record| record.action_id.clone())
        .collect::<Vec<_>>();
    for action_id in settled {
        watchdog
            .settle_compaction_kick(&action_id, Utc::now().timestamp())
            .map_err(anyhow::Error::new)?;
    }
    Ok(())
}

/// Reconcile a crash-safe process replacement outbox. The watchdog swaps the
/// exact identity first, which immediately makes all graph-facing consumers
/// fail closed; this CAS publishes the same epoch/identity in lifecycle before
/// checked consumers may resume. Replaying after either boundary is idempotent.
pub(crate) fn sync_lifecycle_process_authority(
    dir: &Path,
    task_id: &str,
    watchdog: &mut PiWatchdog,
) -> Result<()> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph.get_task_or_err(task_id)?;
    let watchdog_authority = watchdog.process_epoch_authority();
    // Schema-v1 continuation splits are repaired inside the watchdog only;
    // old releases had no legitimate replacement-process transition.
    if watchdog.state().schema_version == 1 {
        watchdog
            .attest_lifecycle_process_authority(
                task.lifecycle.pi_process_epoch,
                &task.lifecycle.pi_process_identity_digest,
                Utc::now().timestamp(),
            )
            .map_err(anyhow::Error::new)?;
        sync_worktree_observer_process_epoch(dir, watchdog)?;
        return Ok(());
    }
    if task.lifecycle.pi_process_epoch == watchdog_authority.process_epoch
        && (task.lifecycle.pi_process_identity_digest.is_empty()
            || task.lifecycle.pi_process_identity_digest
                == watchdog_authority.process_identity_digest)
    {
        sync_worktree_observer_process_epoch(dir, watchdog)?;
        return Ok(());
    }
    if task.lifecycle.pi_process_epoch.saturating_add(1) != watchdog_authority.process_epoch
        || task.lifecycle.pi_process_identity_digest.is_empty()
    {
        anyhow::bail!(
            "process_epoch_authority_mismatch: replacement is not the next exact lifecycle epoch"
        );
    }
    let expected_epoch = task.lifecycle.pi_process_epoch;
    let expected_digest = task.lifecycle.pi_process_identity_digest.clone();
    let next_epoch = watchdog_authority.process_epoch;
    let next_digest = watchdog_authority.process_identity_digest;
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiProcessEpochReplaced {
                expected_process_epoch: expected_epoch,
                expected_process_identity_digest: expected_digest.clone(),
                next_process_epoch: next_epoch,
                next_process_identity_digest: next_digest.clone(),
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-watchdog-process-outbox".into(),
            },
            "exact_process_replaced",
            format!(
                "pi-process-replaced:{}:{}",
                watchdog.state().source.attempt_id,
                next_epoch
            ),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    let graph = load_graph(&graph_path)?;
    let task = graph.get_task_or_err(task_id)?;
    watchdog
        .attest_lifecycle_process_authority(
            task.lifecycle.pi_process_epoch,
            &task.lifecycle.pi_process_identity_digest,
            Utc::now().timestamp(),
        )
        .map_err(anyhow::Error::new)?;
    sync_worktree_observer_process_epoch(dir, watchdog)?;
    Ok(())
}

fn sync_worktree_observer_process_epoch(dir: &Path, watchdog: &PiWatchdog) -> Result<()> {
    let source = &watchdog.state().source;
    let runtime_key = worksgood::attempt_runtime::AttemptRuntimeKey::new(
        &source.task_id,
        source.generation,
        &source.attempt_id,
        source.attempt_fence,
        source.worktree_lease_epoch,
    );
    let storage =
        worksgood::attempt_runtime::component_for_update(dir, &runtime_key, "worktree-observer")?;
    if !storage.exists() {
        return Ok(());
    }
    if !storage.join("state.json").is_file() {
        return Ok(());
    }
    let mut observer = worksgood::worktree_observer::WorktreeObserver::open(&storage)?;
    let current = observer.projection().source.identity.clone();
    let target = watchdog.state().process_epoch;
    if current.process_epoch == target {
        return Ok(());
    }
    if current.process_epoch > target {
        anyhow::bail!(
            "process_epoch_authority_mismatch: worktree observer epoch {} is ahead of watchdog {}",
            current.process_epoch,
            target
        );
    }
    observer.rebind_process_epoch_from_watchdog_at(&current, target, Utc::now().timestamp())?;
    Ok(())
}

/// Reconcile the crash-safe watchdog outbox into the lifecycle ledger. The
/// watchdog persists a continuation intent before session mutation; replaying
/// this function after any restart charges each prompt epoch exactly once.
pub(crate) fn sync_lifecycle_continuation_authority(
    dir: &Path,
    task_id: &str,
    watchdog: &PiWatchdog,
) -> Result<()> {
    let graph_path = dir.join("graph.jsonl");
    let target = watchdog.state().continuation_epoch;
    let process_epoch = watchdog.state().process_epoch;
    let process_identity_digest = watchdog.state().process.digest();
    let elapsed_charge_secs = watchdog.policy().continuation_epoch_lease_secs;
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        if task.lifecycle.pi_continuation_epoch > target {
            rejection = Some(anyhow::anyhow!(
                "stale_continuation_epoch: lifecycle is ahead of watchdog"
            ));
            return false;
        }
        let mut changed = false;
        while task.lifecycle.pi_continuation_epoch < target {
            let expected_continuation_epoch = task.lifecycle.pi_continuation_epoch;
            let next_continuation_epoch = expected_continuation_epoch.saturating_add(1);
            let request = TransitionRequest::new(
                TransitionKind::PiContinuationEpochReserved {
                    expected_process_epoch: process_epoch,
                    process_identity_digest: process_identity_digest.clone(),
                    expected_continuation_epoch,
                    next_continuation_epoch,
                    elapsed_charge_secs,
                },
                LifecycleActor {
                    kind: ActorKind::ProcessObserver,
                    id: "pi-watchdog-continuation-outbox".into(),
                },
                "same_process_continuation",
                format!(
                    "pi-continuation:{}:{}:{}",
                    watchdog.state().source.attempt_id,
                    process_epoch,
                    next_continuation_epoch
                ),
            )
            .expecting(FenceExpectation::current(task));
            if let Err(error) = apply_transition(task, request) {
                rejection = Some(anyhow::Error::new(error));
                return false;
            }
            changed = true;
        }
        changed
    })?;
    if let Some(error) = rejection {
        return Err(error);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct CompactionKickAuthorizeArgs {
    pub reason: String,
    pub will_retry: bool,
    pub compaction_entry_id: String,
    pub compaction_parent_id: String,
    pub session_id: String,
    pub session_file: String,
    pub session_leaf_id: String,
    pub pid: u32,
    pub provider: String,
    pub model: String,
    pub reasoning: Option<String>,
    pub plugin_compat: String,
    pub quiescent: bool,
    pub host_idle: bool,
    pub queue_empty: bool,
    pub tool_clear: bool,
}

fn ensure_compaction_kick_supported() -> Result<()> {
    #[cfg(not(unix))]
    anyhow::bail!(
        "compaction_kick.unsupported_platform: cross-process transaction locking is unavailable"
    );
    #[cfg(unix)]
    {
        if std::env::var("WG_PI_COMPACTION_KICK")
            .map(|value| value.trim() == "0")
            .unwrap_or(false)
        {
            anyhow::bail!("compaction_kick.feature_disabled");
        }
        Ok(())
    }
}

fn bounded_identity(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 1024
        || value
            .bytes()
            .any(|byte| byte == 0 || byte == b'\n' || byte == b'\r')
    {
        anyhow::bail!("compaction_kick.invalid_{label}");
    }
    Ok(())
}

fn lifecycle_unresolved_for_watchdog(task: &worksgood::graph::Task, watchdog: &PiWatchdog) -> bool {
    let source = &watchdog.state().source;
    task.status == worksgood::graph::Status::InProgress
        && task
            .lifecycle
            .current_attempt
            .as_ref()
            .is_some_and(|attempt| {
                attempt.id == source.attempt_id
                    && attempt.generation == source.generation
                    && attempt.fence == source.attempt_fence
                    && attempt.disposition.is_none()
            })
        && task.lifecycle.pi_terminal_reservation.is_none()
        && task.lifecycle.pi_kick_effect_leases.is_empty()
        && task
            .lifecycle
            .pi_continuation
            .as_ref()
            .is_some_and(|authorization| {
                authorization.state == worksgood::lifecycle::PiAuthorizationState::Active
                    && authorization.attempt_id == source.attempt_id
                    && authorization.attempt_fence == source.attempt_fence
                    && authorization.worktree_lease_epoch == source.worktree_lease_epoch
            })
}

/// Pull the native observer's bounded projection through the latest complete
/// capture line before an authority decision. The file follower is a separate
/// process and can lag the awaited Pi callback; without this reconciliation a
/// completed tool-end can transiently look like an unsafe open effect and make
/// an otherwise identical occurrence nondeterministically fail closed.
fn reconcile_native_capture_to_current_complete_line(watchdog: &mut PiWatchdog) -> Result<()> {
    let Some(agent_dir) = watchdog.state().session.session_dir.parent() else {
        return Ok(());
    };
    let raw_path = agent_dir.join(worksgood::stream_event::RAW_STREAM_FILE_NAME);
    let Ok(bytes) = std::fs::read(&raw_path) else {
        return Ok(());
    };
    let stream_id = raw_path.to_string_lossy().into_owned();
    let mut offset = watchdog.native_stream_offset(&stream_id);
    let start = usize::try_from(offset).context("Pi native stream offset does not fit")?;
    if start > bytes.len() {
        anyhow::bail!("compaction_kick.native_capture_shrank");
    }
    let complete_end = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(start);
    if complete_end <= start {
        return Ok(());
    }
    for line in bytes[start..complete_end].split_inclusive(|byte| *byte == b'\n') {
        offset = offset.saturating_add(line.len() as u64);
        let line = std::str::from_utf8(line)?.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        watchdog
            .ingest_native_line(line, &stream_id, offset, Utc::now().timestamp())
            .map_err(anyhow::Error::new)?;
    }
    Ok(())
}

/// Authenticate and persist one exact compaction occurrence. The worker
/// capability binding has already been validated by the IPC broker; this
/// function independently rechecks its tuple against the watchdog/lifecycle.
pub(crate) fn compaction_kick_authorize(
    dir: &Path,
    binding: &worksgood::worker_control::AttemptCapabilityBinding,
    args: CompactionKickAuthorizeArgs,
) -> Result<serde_json::Value> {
    ensure_compaction_kick_supported()?;
    for (value, label) in [
        (&args.compaction_entry_id, "entry_id"),
        (&args.compaction_parent_id, "parent_id"),
        (&args.session_id, "session_id"),
        (&args.session_file, "session_file"),
        (&args.session_leaf_id, "session_leaf"),
        (&args.provider, "provider"),
        (&args.model, "model"),
        (&args.plugin_compat, "plugin_compat"),
    ] {
        bounded_identity(value, label)?;
    }
    if args.reason != "threshold" || args.will_retry {
        anyhow::bail!("compaction_kick.not_qualifying");
    }
    if binding.task_id != binding.save_source.task_id
        || binding.generation != binding.save_source.generation
        || binding.attempt_id != binding.save_source.attempt_id
        || binding.fence != binding.save_source.attempt_fence
        || binding.lease_epoch != binding.save_source.worktree_lease_epoch
    {
        anyhow::bail!("compaction_kick.capability_binding_mismatch");
    }

    let mut watchdog = checked_open(dir, &binding.task_id)?;
    reconcile_native_capture_to_current_complete_line(&mut watchdog)?;
    let source = &watchdog.state().source;
    if source.generation != binding.generation
        || source.attempt_id != binding.attempt_id
        || source.attempt_fence != binding.fence
        || source.worktree_lease_epoch != binding.lease_epoch
    {
        anyhow::bail!("compaction_kick.stale_capability");
    }
    if !process_identity_matches_kernel(&watchdog.state().process)
        || (args.pid != watchdog.state().process.pid
            && !process_descends_from(args.pid, watchdog.state().process.pid))
    {
        anyhow::bail!("compaction_kick.process_proof_mismatch");
    }
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(&binding.task_id)?;
    if !lifecycle_unresolved_for_watchdog(task, &watchdog) {
        anyhow::bail!("compaction_kick.lifecycle_resolved_or_held");
    }

    let supplied_session_file = PathBuf::from(&args.session_file)
        .canonicalize()
        .context("canonicalize Pi session file")?;
    let selected = worksgood::pi_watchdog::select_canonical_session_journal(
        &watchdog.state().session.session_dir,
        &args.session_id,
    )
    .map_err(anyhow::Error::new)?;
    let selected_file = selected.session_file.canonicalize()?;
    if supplied_session_file != selected_file
        || args.session_id != watchdog.state().session.session_id
    {
        anyhow::bail!("compaction_kick.session_proof_mismatch");
    }
    let prior = watchdog.state().session.clone();
    let session_bytes = std::fs::read(&selected_file)?;
    if selected_file
        == prior
            .session_file
            .canonicalize()
            .unwrap_or_else(|_| prior.session_file.clone())
    {
        let prefix_len = usize::try_from(prior.append_prefix_len)
            .context("attested Pi prefix length does not fit")?;
        if prefix_len > session_bytes.len()
            || format!("b3:{}", blake3::hash(&session_bytes[..prefix_len]).to_hex())
                != prior.append_prefix_digest
        {
            anyhow::bail!("compaction_kick.session_prefix_mismatch");
        }
    } else if selected.header_digest != prior.header_digest {
        anyhow::bail!("compaction_kick.session_header_mismatch");
    }

    let mut matching_entry: Option<serde_json::Value> = None;
    let mut final_leaf: Option<String> = None;
    for (line_no, line) in session_bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(line)
            .with_context(|| format!("parse Pi session line {}", line_no + 1))?;
        if line_no == 0 {
            if value.get("type").and_then(|value| value.as_str()) != Some("session")
                || value.get("id").and_then(|value| value.as_str())
                    != Some(args.session_id.as_str())
            {
                anyhow::bail!("compaction_kick.session_header_mismatch");
            }
            continue;
        }
        if let Some(id) = value.get("id").and_then(|value| value.as_str()) {
            final_leaf = Some(id.to_string());
            if id == args.compaction_entry_id {
                if matching_entry.is_some() {
                    anyhow::bail!("compaction_kick.ambiguous_entry");
                }
                matching_entry = Some(value);
            }
        }
    }
    let entry = matching_entry.context("compaction_kick.entry_missing")?;
    if entry.get("type").and_then(|value| value.as_str()) != Some("compaction")
        || entry.get("parentId").and_then(|value| value.as_str())
            != Some(args.compaction_parent_id.as_str())
        || final_leaf.as_deref() != Some(args.session_leaf_id.as_str())
        || args.session_leaf_id != args.compaction_entry_id
    {
        anyhow::bail!("compaction_kick.entry_leaf_mismatch");
    }
    let compaction_entry_digest =
        format!("b3:{}", blake3::hash(&serde_json::to_vec(&entry)?).to_hex());
    let session_file_digest = format!("b3:{}", blake3::hash(&session_bytes).to_hex());
    watchdog
        .reconcile_session_journal(Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;

    let process_pid = watchdog.state().process.pid;
    let process_epoch = watchdog.state().process_epoch;
    let process_identity_digest = watchdog.state().process.digest();
    let route_snapshot_digest = watchdog.state().route.digest();
    let record = watchdog
        .authorize_compaction_kick(
            worksgood::pi_watchdog::VerifiedCompactionOccurrence {
                graph_id: binding.graph_id.clone(),
                compaction_entry_id: args.compaction_entry_id,
                compaction_parent_id: args.compaction_parent_id,
                compaction_entry_digest,
                session_id: args.session_id,
                session_file_digest,
                session_leaf_id: args.session_leaf_id,
                // The watchdog's exact launch authority is the generated
                // wrapper's gated Pi command process; with GNU timeout/stdin
                // plumbing the live Pi isolate is a verified descendant.
                // Preserve the authoritative epoch identity here after the
                // ancestry check above.
                process_pid,
                process_epoch,
                process_identity_digest,
                provider: args.provider,
                model: args.model,
                reasoning: args.reasoning,
                route_snapshot_digest,
                plugin_compat: args.plugin_compat,
                reason: args.reason,
                will_retry: args.will_retry,
                quiescent: args.quiescent,
                host_idle: args.host_idle,
                queue_empty: args.queue_empty,
                tool_clear: args.tool_clear,
            },
            Utc::now().timestamp(),
        )
        .map_err(anyhow::Error::new)?;
    Ok(serde_json::json!({
        "actionId": record.action_id,
        "occurrenceId": record.occurrence_id,
        "state": record.state,
        "occurrenceOrdinal": record.occurrence_ordinal,
    }))
}

/// Commit/reconcile the lifecycle epoch CAS and return prompt authority only
/// to the call that performed the fresh CAS.
pub(crate) fn compaction_kick_permit(
    dir: &Path,
    task_id: &str,
    action_id: &str,
    allow_fresh: bool,
) -> Result<serde_json::Value> {
    ensure_compaction_kick_supported()?;
    let mut watchdog = checked_open(dir, task_id)?;
    reconcile_native_capture_to_current_complete_line(&mut watchdog)?;
    let record = watchdog
        .compaction_kick(action_id)
        .cloned()
        .context("compaction_kick.action_missing")?;
    if record.state != worksgood::pi_watchdog::PiCompactionKickState::Authorized {
        let permit = watchdog
            .permit_compaction_kick(
                action_id,
                record.to_continuation_epoch,
                false,
                Utc::now().timestamp(),
            )
            .map_err(anyhow::Error::new)?;
        return Ok(compaction_permit_json(permit));
    }
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(task_id)?;
    if !lifecycle_unresolved_for_watchdog(task, &watchdog) {
        watchdog
            .suppress_compaction_kick(
                action_id,
                "terminal_or_wait_before_permit",
                Utc::now().timestamp(),
            )
            .map_err(anyhow::Error::new)?;
        anyhow::bail!("compaction_kick.lifecycle_resolved_before_permit");
    }
    if !process_identity_matches_kernel(&watchdog.state().process) {
        anyhow::bail!("compaction_kick.process_exited");
    }
    if watchdog.state().phase == worksgood::pi_watchdog::Phase::Tool
        || watchdog.state().tool.is_some()
        || !watchdog.state().exact_guards.session
        || !watchdog.state().exact_guards.route
        || !watchdog.state().exact_guards.worktree
        || !watchdog.state().exact_guards.pid_identity
        || !watchdog.state().exact_guards.containment
        || !watchdog.state().exact_guards.effect
        || !watchdog.state().exact_guards.terminal_clear
    {
        anyhow::bail!("compaction_kick.guard_changed_before_permit");
    }
    let selected = worksgood::pi_watchdog::select_canonical_session_journal(
        &watchdog.state().session.session_dir,
        &record.session_id,
    )
    .map_err(anyhow::Error::new)?;
    let session_bytes = std::fs::read(&selected.session_file)?;
    let session_digest = format!("b3:{}", blake3::hash(&session_bytes).to_hex());
    let final_leaf = session_bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .filter_map(|entry| {
            entry
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .next_back();
    if selected.session_file.canonicalize()?
        != watchdog.state().session.session_file.canonicalize()?
        || selected.header_digest != watchdog.state().session.header_digest
        || session_digest != record.session_file_digest
        || final_leaf.as_deref() != Some(record.session_leaf_id.as_str())
    {
        anyhow::bail!("compaction_kick.session_changed_before_permit");
    }
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    let mut fresh_cas = false;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        let duplicate = task
            .lifecycle
            .audit
            .iter()
            .any(|event| event.idempotency_key == action_id);
        let request = TransitionRequest::new(
            TransitionKind::PiContinuationEpochReserved {
                expected_process_epoch: record.process_epoch,
                process_identity_digest: record.process_identity_digest.clone(),
                expected_continuation_epoch: record.authorized_from_continuation_epoch,
                next_continuation_epoch: record.to_continuation_epoch,
                elapsed_charge_secs: watchdog.policy().continuation_epoch_lease_secs,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-compaction-kick".into(),
            },
            "threshold_compaction_kick",
            action_id.to_string(),
        )
        .expecting(FenceExpectation::current(task))
        .with_evidence(record.occurrence_id.clone());
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error);
            return false;
        }
        fresh_cas = !duplicate;
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    let permit = watchdog
        .permit_compaction_kick(
            action_id,
            record.to_continuation_epoch,
            allow_fresh && fresh_cas,
            Utc::now().timestamp(),
        )
        .map_err(anyhow::Error::new)?;
    Ok(compaction_permit_json(permit))
}

fn compaction_permit_json(
    permit: worksgood::pi_watchdog::PiCompactionKickPermit,
) -> serde_json::Value {
    serde_json::json!({
        "actionId": permit.action_id,
        "state": permit.state,
        "freshDeliveryGrant": permit.fresh_delivery_grant,
        "prompt": permit.prompt,
        "promptVersion": permit.prompt_version,
        "promptDigest": permit.prompt_digest,
    })
}

pub(crate) fn compaction_kick_ack(
    dir: &Path,
    task_id: &str,
    action_id: &str,
    prompt_version: &str,
    prompt_digest: &str,
) -> Result<serde_json::Value> {
    ensure_compaction_kick_supported()?;
    let mut watchdog = checked_open(dir, task_id)?;
    let record = watchdog
        .compaction_kick(action_id)
        .context("compaction_kick.action_missing")?;
    if record.prompt_version != prompt_version || record.prompt_digest != prompt_digest {
        anyhow::bail!("compaction_kick.prompt_proof_mismatch");
    }
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiCompactionKickAcknowledged {
                action_id: action_id.to_string(),
                process_epoch: record.process_epoch,
                process_identity_digest: record.process_identity_digest.clone(),
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-compaction-kick".into(),
            },
            "compaction_kick_selected",
            format!("pi-kick-ack:{action_id}"),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    let graph = load_graph(&graph_path)?;
    let terminal_won = graph
        .get_task_or_err(task_id)?
        .lifecycle
        .pi_terminal_reservation
        .is_some();
    let ack = watchdog
        .acknowledge_compaction_kick(action_id, terminal_won, Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    Ok(serde_json::json!({
        "actionId": ack.action_id,
        "state": ack.state,
        "abort": ack.abort,
    }))
}

/// Hold one bounded action-scoped cancellation subscription. The worker opens
/// the zero-wait probe before the native queue append, then keeps one long
/// poll outstanding while the recovery turn runs. Terminal/park and exact
/// process changes are lifecycle truth; this observer grants no continuation
/// or effect authority.
pub(crate) fn compaction_kick_terminal_watch(
    dir: &Path,
    task_id: &str,
    action_id: &str,
    wait_ms: u64,
) -> Result<serde_json::Value> {
    ensure_compaction_kick_supported()?;
    bounded_identity(action_id, "action_id")?;
    if wait_ms > 30_000 {
        anyhow::bail!("compaction_kick.terminal_watch_too_long");
    }
    let watchdog = checked_open(dir, task_id)?;
    let record = watchdog
        .compaction_kick(action_id)
        .cloned()
        .context("compaction_kick.action_missing")?;
    if !matches!(
        record.state,
        worksgood::pi_watchdog::PiCompactionKickState::DeliveryPermitted
            | worksgood::pi_watchdog::PiCompactionKickState::Acknowledged
            | worksgood::pi_watchdog::PiCompactionKickState::AcknowledgedTerminalRace
            | worksgood::pi_watchdog::PiCompactionKickState::Running
            | worksgood::pi_watchdog::PiCompactionKickState::TerminalObserved
            | worksgood::pi_watchdog::PiCompactionKickState::TerminalAbortAcknowledged
            | worksgood::pi_watchdog::PiCompactionKickState::SettledAfterKick
    ) {
        anyhow::bail!("compaction_kick.action_not_permitted");
    }
    let source = watchdog.state().source.clone();
    drop(watchdog);

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    loop {
        let graph = load_graph(dir.join("graph.jsonl"))?;
        let task = graph.get_task_or_err(task_id)?;
        let settled = task
            .lifecycle
            .audit
            .iter()
            .any(|event| event.idempotency_key == format!("pi-kick-settle:{action_id}"));
        let terminal = task.status != worksgood::graph::Status::InProgress
            || task.lifecycle.pi_terminal_reservation.is_some()
            || task.lifecycle.pi_kick_revoked_actions.contains(action_id)
            || task.lifecycle.generation != source.generation
            || task.lifecycle.fence != source.attempt_fence
            || task.lifecycle.pi_process_epoch != record.process_epoch
            || (!task.lifecycle.pi_process_identity_digest.is_empty()
                && task.lifecycle.pi_process_identity_digest != record.process_identity_digest)
            || task
                .lifecycle
                .current_attempt
                .as_ref()
                .is_none_or(|attempt| {
                    attempt.id != source.attempt_id || attempt.disposition.is_some()
                });
        if terminal || settled || std::time::Instant::now() >= deadline {
            return Ok(serde_json::json!({
                "actionId": action_id,
                "abort": terminal,
                "settled": settled,
                "timedOut": !terminal && !settled && wait_ms > 0,
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

pub(crate) fn compaction_kick_cancel(
    dir: &Path,
    task_id: &str,
    action_id: &str,
    reason: &str,
) -> Result<serde_json::Value> {
    ensure_compaction_kick_supported()?;
    let mut watchdog = checked_open(dir, task_id)?;
    let record = watchdog
        .suppress_compaction_kick(action_id, reason, Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    Ok(serde_json::json!({"actionId": record.action_id, "state": record.state}))
}

pub(crate) fn compaction_kick_settle(
    dir: &Path,
    task_id: &str,
    action_id: &str,
) -> Result<serde_json::Value> {
    ensure_compaction_kick_supported()?;
    let mut watchdog = checked_open(dir, task_id)?;
    let kick_state = watchdog
        .compaction_kick(action_id)
        .map(|record| record.state)
        .context("compaction_kick.action_missing")?;
    if !matches!(
        kick_state,
        worksgood::pi_watchdog::PiCompactionKickState::Acknowledged
            | worksgood::pi_watchdog::PiCompactionKickState::Running
            | worksgood::pi_watchdog::PiCompactionKickState::SettledAfterKick
    ) {
        anyhow::bail!("compaction_kick.action_not_running");
    }
    // Lifecycle is the effect/terminal authority, so remove the active action
    // there first. Only after that exact CAS succeeds may the watchdog report a
    // settled diagnostic state. A crash between the two durable writes is
    // repaired by reconcile_compaction_action_outbox on the next checked open.
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiCompactionKickSettled {
                action_id: action_id.to_string(),
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-compaction-kick".into(),
            },
            "compaction_kick_settled",
            format!("pi-kick-settle:{action_id}"),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    let record = watchdog
        .settle_compaction_kick(action_id, Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    Ok(serde_json::json!({"actionId": record.action_id, "state": record.state}))
}

pub(crate) fn compaction_kick_abort_ack(
    dir: &Path,
    task_id: &str,
    action_id: &str,
) -> Result<serde_json::Value> {
    ensure_compaction_kick_supported()?;
    let mut watchdog = checked_open(dir, task_id)?;
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph.get_task_or_err(task_id)?;
    if !task.lifecycle.pi_kick_revoked_actions.contains(action_id) {
        anyhow::bail!("compaction_kick.abort_without_terminal_revocation");
    }
    // A terminal subscription can observe lifecycle revocation after the
    // original acknowledgement RPC. Refresh the watchdog's diagnostic state
    // from that exact durable action before recording abort acknowledgement;
    // otherwise Acknowledged -> AbortAck would be an invalid transition and a
    // lost notification could never converge.
    if matches!(
        watchdog
            .compaction_kick(action_id)
            .map(|record| record.state),
        Some(
            worksgood::pi_watchdog::PiCompactionKickState::Acknowledged
                | worksgood::pi_watchdog::PiCompactionKickState::Running
        )
    ) {
        watchdog
            .acknowledge_compaction_kick(action_id, true, Utc::now().timestamp())
            .map_err(anyhow::Error::new)?;
    }
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiCompactionKickAbortAcknowledged {
                action_id: action_id.to_string(),
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-compaction-kick".into(),
            },
            "compaction_kick_abort_acknowledged",
            format!("pi-kick-abort-ack:{action_id}"),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    let record = watchdog
        .abort_ack_compaction_kick(action_id, Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    Ok(serde_json::json!({"actionId": record.action_id, "state": record.state}))
}

pub(crate) fn compaction_kick_effect(
    dir: &Path,
    task_id: &str,
    action_id: &str,
    tool_call_id: &str,
    begin: bool,
) -> Result<serde_json::Value> {
    ensure_compaction_kick_supported()?;
    bounded_identity(action_id, "action_id")?;
    bounded_identity(tool_call_id, "tool_call_id")?;
    let watchdog = checked_open(dir, task_id)?;
    let record = watchdog
        .compaction_kick(action_id)
        .context("compaction_kick.action_missing")?;
    if !matches!(
        record.state,
        worksgood::pi_watchdog::PiCompactionKickState::Acknowledged
            | worksgood::pi_watchdog::PiCompactionKickState::Running
    ) {
        anyhow::bail!("compaction_kick.action_not_running");
    }
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        let kind = if begin {
            TransitionKind::PiKickEffectLeaseOpened {
                lease: worksgood::lifecycle::PiKickEffectLease {
                    action_id: action_id.to_string(),
                    tool_call_id: tool_call_id.to_string(),
                    process_epoch: record.process_epoch,
                    process_identity_digest: record.process_identity_digest.clone(),
                },
            }
        } else {
            TransitionKind::PiKickEffectLeaseClosed {
                action_id: action_id.to_string(),
                tool_call_id: tool_call_id.to_string(),
                process_epoch: record.process_epoch,
            }
        };
        let request = TransitionRequest::new(
            kind,
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-compaction-kick-effect".into(),
            },
            if begin {
                "kick_effect_begin"
            } else {
                "kick_effect_end"
            },
            format!(
                "pi-kick-effect-{}:{}:{}",
                if begin { "begin" } else { "end" },
                action_id,
                tool_call_id
            ),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    Ok(serde_json::json!({
        "actionId": action_id,
        "toolCallId": tool_call_id,
        "permitted": true,
        "state": if begin { "opened" } else { "closed" },
    }))
}

fn status(dir: &Path, id: &str, json: bool) -> Result<()> {
    let watchdog = checked_open(dir, id)?;
    let state = watchdog.state();
    let now = Utc::now().timestamp();
    let silence = now.saturating_sub(state.last_meaningful_at);
    if json {
        let value = serde_json::json!({
            "state": state,
            "policy": watchdog.policy(),
            "silence_secs": silence,
            "soft_suspect_secs": watchdog.policy().meaningful_silence_secs,
            "hard_resume_after_secs": state.hard_resume_after_secs,
            "next_safe_operator_action": next_action(dir, state),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!(
        "Pi watchdog: {:?}{}",
        state.classification,
        if state.classification == worksgood::pi_watchdog::Classification::Suspect {
            " (soft observation; process intact)"
        } else {
            ""
        }
    );
    println!(
        "  source: task={} gen={} attempt={} fence={} worktree-lease={}",
        state.source.task_id,
        state.source.generation,
        state.source.attempt_id,
        state.source.attempt_fence,
        state.source.worktree_lease_epoch
    );
    println!(
        "  session: id={} leaf={} proof={} route=pi:{}:{}@{} qos={:?}",
        state.session.session_id,
        state.session.branch_leaf,
        state.session.digest(),
        state.route.provider,
        state.route.model,
        state.route.reasoning.as_deref().unwrap_or("default"),
        state.route.qos
    );
    println!(
        "  process: continuation-epoch={} epoch={} pid={} pgid={} start={} boot={} nonce={} exact={}",
        state.continuation_epoch,
        state.process_epoch,
        state.process.pid,
        state.process.pgid,
        state.process.start_ticks,
        state.process.boot_id,
        state.process.nonce,
        state.exact_guards.pid_identity
    );
    println!(
        "  progress: seq={} {} at {}; silence={}s / soft-suspect={}s",
        state.progress_seq,
        state.last_meaningful_kind,
        state.last_meaningful_at,
        silence,
        watchdog.policy().meaningful_silence_secs
    );
    println!(
        "  native: live/unproven seq={} at={:?} thinking-events={} output-events={} tool={}/{} child={} receipt={} usage-receipts={}",
        state.native_activity.event_seq,
        state.native_activity.last_activity_at,
        state.native_activity.thinking_activity_seq,
        state.native_activity.output_activity_seq,
        state
            .native_activity
            .current_tool_class
            .as_deref()
            .unwrap_or("none"),
        state
            .native_activity
            .current_tool_label
            .as_deref()
            .unwrap_or("none"),
        state
            .native_activity
            .tool_child_state
            .as_deref()
            .unwrap_or("none"),
        state
            .native_activity
            .tool_receipt_state
            .as_deref()
            .unwrap_or("none"),
        state.native_activity.usage_receipt_count,
    );
    println!(
        "  probe: action={:?} observed={:?}; progress-reset=no",
        state.probe_action_id, state.probe_observed_at
    );
    println!(
        "  hard-resume: phase={:?} threshold={} eligible={:?} grace-deadline={:?}",
        state.phase,
        state
            .hard_resume_after_secs
            .map(|v| format!("{v}s"))
            .unwrap_or_else(|| "none".into()),
        state.hard_eligible_at,
        state.hard_grace_deadline
    );
    println!(
        "  tool: {:?}; wait: {:?}; prompt-marker: {:?}",
        state.tool, state.wait_correlation, state.prompt_marker
    );
    println!(
        "  budget: epochs={}/{}+{} elapsed-reserved={}/{}+{}s (recovery only)",
        state.epochs_used,
        watchdog.policy().max_continuation_epochs,
        state.manual_epochs_granted,
        state.elapsed_reserved_secs,
        watchdog.policy().max_continuation_elapsed_secs,
        state.manual_elapsed_granted_secs
    );
    println!(
        "  compaction: reason={} success={} aborted={} will-retry={} queue={}/{} retry={} kicks={}",
        state
            .native_activity
            .compaction_reason
            .as_deref()
            .unwrap_or("none"),
        state
            .native_activity
            .compaction_succeeded
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into()),
        state
            .native_activity
            .compaction_aborted
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into()),
        state
            .native_activity
            .compaction_will_retry
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into()),
        state.native_activity.steering_queue_count,
        state.native_activity.follow_up_queue_count,
        state
            .native_activity
            .retry_state
            .as_deref()
            .unwrap_or("none"),
        state.compaction_kicks.len(),
    );
    for kick in state.compaction_kicks.iter().rev().take(4).rev() {
        println!(
            "    kick #{} action={} occurrence={} state={:?} entry={} epoch={}->{} reason={}",
            kick.occurrence_ordinal,
            kick.action_id,
            kick.occurrence_id,
            kick.state,
            kick.compaction_entry_id,
            kick.authorized_from_continuation_epoch,
            kick.to_continuation_epoch,
            kick.reason_code,
        );
    }
    println!(
        "  reason: {}; pending={:?}; exact-route-error={:?}",
        state.reason_code.as_deref().unwrap_or("none"),
        state.pending_actions,
        state.exact_route_error
    );
    println!("  next: {}", next_action(dir, state));
    Ok(())
}

fn next_action(dir: &Path, state: &worksgood::pi_watchdog::PiWatchdogState) -> String {
    use worksgood::pi_watchdog::Classification::*;
    match state.classification {
        Active => "continued observation; total runtime is not a deadline".into(),
        Suspect => "read-only probe/observe; no signal before hard policy + grace + proof".into(),
        HardResumeEligible => "await hard grace and fresh unchanged proof CAS".into(),
        NeedsFinalization => {
            let record = worksgood::service::ConvergenceState::load(dir)
                .ok()
                .and_then(|convergence| {
                    convergence
                        .goals
                        .get(&format!(
                            "{}#{}",
                            state.source.task_id, state.source.generation
                        ))
                        .cloned()
                });
            let deadline = record
                .as_ref()
                .map(|record| record.next_wake_at.as_str())
                .unwrap_or("next service pass");
            let action = record
                .as_ref()
                .and_then(|record| record.pending_convergence_action)
                .map(worksgood::service::FinishConvergenceAction::description)
                .unwrap_or(
                    "finish durable receipt or release exact dead owner and resume same session/worktree",
                );
            let rank = record
                .as_ref()
                .and_then(|record| record.finish_convergence_rank)
                .and_then(|rank| serde_json::to_value(rank).ok())
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unprojected".into());
            format!("{action}; rank={rank}; deadline={deadline}; silence never implies success")
        }
        StalledOperatorRequired => format!(
            "wg pi-watchdog resume {} --reason '<audited reason>'",
            state.source.task_id
        ),
        WaitingUser => "await the accepted correlation through normal lifecycle".into(),
        LongTool => "protect the valid long-tool lease; reconcile effects at expiry".into(),
        Fencing | Resuming => "allow the durable outbox to converge; do not launch manually".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn resume(
    dir: &Path,
    id: &str,
    reason: String,
    epochs: u32,
    elapsed: u64,
    ack_call: Option<String>,
    disposition: Option<String>,
    receipt: Option<String>,
    json: bool,
) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    let effect_ack = match (ack_call, disposition, receipt) {
        (None, None, None) => None,
        (Some(tool_call_id), Some(disposition), Some(receipt)) => Some(EffectAcknowledgement {
            tool_call_id,
            disposition,
            receipt,
        }),
        _ => anyhow::bail!("--ack-call, --disposition, and --receipt must be supplied together"),
    };
    let action_id = format!(
        "manual:{}:{}:{}:{}",
        id,
        watchdog.state().source.attempt_fence,
        watchdog.state().process_epoch,
        blake3::hash(reason.as_bytes()).to_hex()
    );
    watchdog
        .manual_resume(
            ManualGrant {
                action_id,
                reason,
                epochs,
                elapsed_secs: elapsed,
                effect_ack,
            },
            Utc::now().timestamp(),
        )
        .map_err(anyhow::Error::new)?;
    if json {
        println!("{}", serde_json::to_string_pretty(watchdog.state())?);
    } else {
        println!(
            "Manual same-session grant recorded for '{}'; route/session/attempt/worktree remain frozen",
            id
        );
    }
    Ok(())
}

fn bootstrap(
    dir: &Path,
    id: &str,
    agent_dir: &Path,
    pid: u32,
    wrapper_pid: Option<u32>,
) -> Result<()> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph.get_task_or_err(id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("Pi bootstrap requires current attempt")?
        .clone();
    let runtime_key = worksgood::attempt_runtime::AttemptRuntimeKey::for_attempt(task, &attempt);
    let state_dir = worksgood::attempt_runtime::component_for_update(dir, &runtime_key, "pi")?;
    let state_path = state_dir.join("state.json");
    if state_path.exists() {
        return Ok(());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(agent_dir.join("metadata.json"))?)?;
    let plan: serde_json::Value =
        serde_json::from_slice(&std::fs::read(agent_dir.join("pi-session-plan.json"))?)?;
    let worktree = metadata
        .get("worktree_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            metadata
                .get("effective_cwd")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        });
    let model = metadata
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("pi:unknown:unknown");
    let inner = model.strip_prefix("pi:").unwrap_or(model);
    let (provider, model_id) = inner.split_once(':').unwrap_or(("unknown", inner));
    let planned_session_file = PathBuf::from(
        plan["session_file"]
            .as_str()
            .context("session file missing")?,
    );
    let session_dir = PathBuf::from(
        plan["session_dir"]
            .as_str()
            .context("session dir missing")?,
    );
    let session_id = plan["session_id"].as_str().context("session id missing")?;
    let selected =
        worksgood::pi_watchdog::select_canonical_session_journal(&session_dir, session_id)
            .map_err(anyhow::Error::new)?;
    let planned_header_digest = plan["header_digest"]
        .as_str()
        .context("header digest missing")?;
    let planned_bytes = std::fs::read(&planned_session_file)?;
    let planned_header = planned_bytes
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let actual_planned_header_digest = format!("b3:{}", blake3::hash(planned_header).to_hex());
    if actual_planned_header_digest != planned_header_digest {
        anyhow::bail!("Pi bootstrap journal header does not match its launch attestation")
    }
    if selected.session_file != planned_session_file && !selected.substantive {
        anyhow::bail!("Pi bootstrap plan does not resolve to its attested journal")
    }
    if plan["resumed"].as_bool() == Some(true) {
        let prefix_len = plan["canonical_prefix_len"]
            .as_u64()
            .context("canonical prefix length missing")?;
        let expected_leaf = plan["canonical_leaf"]
            .as_str()
            .context("canonical leaf missing")?;
        let selected_bytes = std::fs::read(&selected.session_file)?;
        let prefix_len = usize::try_from(prefix_len).context("canonical prefix too large")?;
        if selected_bytes.len() < prefix_len
            || format!(
                "b3:{}",
                blake3::hash(&selected_bytes[..prefix_len]).to_hex()
            ) != expected_leaf
        {
            anyhow::bail!("Pi resumed journal does not extend its attested substantive leaf")
        }
    }
    let source = SourceTuple {
        task_id: id.into(),
        generation: task.lifecycle.generation,
        attempt_id: attempt.id.clone(),
        attempt_fence: task.lifecycle.fence,
        worktree_lease_epoch: task.lifecycle.fence,
        worktree_path: worktree,
    };
    let route = RouteSnapshot {
        handler: "pi".into(),
        provider: provider.into(),
        model: model_id.into(),
        reasoning: metadata
            .get("reasoning")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        endpoint_redacted: "pi-owned".into(),
        endpoint_hmac: format!("b3:{}", blake3::hash(model.as_bytes()).to_hex()),
        qos: QosClass::Low,
        pi_binary_digest: "pi-path-owned".into(),
        plugin_digest: worksgood::pi_plugin::WG_PI_PLUGIN_COMPAT_VERSION.into(),
    };
    let session = SessionProof {
        session_id: session_id.into(),
        branch_leaf: selected.branch_leaf,
        session_dir,
        session_file: selected.session_file,
        header_digest: selected.header_digest,
        append_prefix_digest: selected.append_prefix_digest,
        append_prefix_len: selected.append_prefix_len,
    };
    let process = capture_process(pid)?;
    let process_identity_digest = process.digest();
    let wrapper = wrapper_pid.map(capture_process).transpose()?;
    if let Some(wrapper) = wrapper.as_ref() {
        attest_native_child_of_wrapper(pid, wrapper.pid)?;
    }
    let mut watchdog = PiWatchdog::new_at(
        state_path,
        source.clone(),
        route.clone(),
        session.clone(),
        process,
        WatchdogPolicy::default(),
        Utc::now().timestamp(),
    )
    .map_err(anyhow::Error::new)?;
    if let Some(wrapper) = wrapper {
        watchdog
            .bind_terminal_wrapper(wrapper, Utc::now().timestamp())
            .map_err(anyhow::Error::new)?;
    }
    let authorization = worksgood::lifecycle::PiContinuationAuthorization {
        authorization_id: format!("pi-auth:{}", attempt.id),
        task_id: id.into(),
        generation: source.generation,
        attempt_id: source.attempt_id,
        attempt_fence: source.attempt_fence,
        worktree_lease_epoch: source.worktree_lease_epoch,
        session_proof_digest: session.digest(),
        route_snapshot_digest: route.digest(),
        state: worksgood::lifecycle::PiAuthorizationState::Active,
        max_replacement_epochs: 3,
        max_reserved_elapsed_secs: 1800,
        epochs_used: 0,
        elapsed_reserved_secs: 0,
        issued_by_policy: "pi-watchdog-static-v1".into(),
    };
    let expected = FenceExpectation::current(task);
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        if let Err(e) = apply_transition(
            task,
            TransitionRequest::new(
                TransitionKind::PiContinuationAuthorized {
                    authorization: authorization.clone(),
                    initial_process_epoch: 1,
                    initial_process_identity_digest: process_identity_digest.clone(),
                },
                LifecycleActor {
                    kind: ActorKind::Dispatcher,
                    id: "pi-spawn-bootstrap".into(),
                },
                "pi_authorized",
                format!("pi-auth:{}", attempt.id),
            )
            .expecting(expected.clone()),
        ) {
            rejection = Some(e);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    Ok(())
}

fn attest_native_child_of_wrapper(child_pid: u32, wrapper_pid: u32) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{child_pid}/stat"))?;
        let close = stat.rfind(')').context("invalid proc stat")?;
        let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        let parent: u32 = fields.get(1).context("proc parent missing")?.parse()?;
        if parent != wrapper_pid {
            anyhow::bail!(
                "invalid_process_topology: native Pi PID {child_pid} is not owned by wrapper PID {wrapper_pid}"
            );
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let caller_parent = unsafe { libc::getppid() as u32 };
        if caller_parent != wrapper_pid || child_pid == wrapper_pid {
            anyhow::bail!(
                "invalid_process_topology: bootstrap caller is not owned by wrapper PID {wrapper_pid} or child is not distinct"
            );
        }
    }
    Ok(())
}

fn capture_process(pid: u32) -> Result<ProcessIdentity> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let close = stat.rfind(')').context("invalid proc stat")?;
        let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        let pgid = fields
            .get(2)
            .context("proc process group missing")?
            .parse()?;
        let start_ticks = fields
            .get(19)
            .context("proc start ticks missing")?
            .parse()?;
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_string();
        Ok(ProcessIdentity {
            pid,
            pgid,
            start_ticks,
            boot_id,
            nonce: uuid::Uuid::now_v7().to_string(),
        })
    }
    #[cfg(not(target_os = "linux"))]
    Ok(ProcessIdentity {
        pid,
        pgid: pid,
        start_ticks: 0,
        boot_id: "platform".into(),
        nonce: uuid::Uuid::now_v7().to_string(),
    })
}

/// Reserve a worker terminal intent in the lifecycle/watchdog first-terminal
/// CAS. Candidate finalization consumes this receipt only after process exit;
/// this function never checkpoints, merges, or resumes Pi.
pub fn reserve_worker_terminal(
    dir: &Path,
    id: &str,
    disposition: TerminalDisposition,
    tool_call_id: &str,
) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    attest_worker_descends_from_current_process(&watchdog)?;
    let receipt = TerminalIntentReceipt::new(
        &watchdog,
        watchdog.state().process_epoch,
        tool_call_id,
        disposition,
    );
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    let receipt_for_graph = receipt.clone();
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiTerminalIntent {
                receipt: receipt_for_graph.clone(),
            },
            LifecycleActor {
                kind: ActorKind::Worker,
                id: task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|a| a.actor_id.clone())
                    .unwrap_or_else(|| "pi-worker".into()),
            },
            "worker_terminal_intent",
            receipt_for_graph.idempotency_key.clone(),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(error) = apply_transition(task, request) {
            // Exact duplicate is idempotent at the lifecycle layer. A
            // contradictory receipt remains evidence and cannot replace it.
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    watchdog
        .observe(Observation::TerminalIntent(receipt), Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    Ok(())
}

fn attest_worker_descends_from_current_process(watchdog: &PiWatchdog) -> Result<()> {
    if std::env::var("WG_EXECUTOR_TYPE").as_deref() != Ok("pi") {
        return Ok(());
    }
    let native = &watchdog.state().process;
    let wrapper = watchdog.state().terminal_wrapper.as_ref();
    #[cfg(target_os = "linux")]
    {
        let mut pid = std::process::id();
        for _ in 0..64 {
            if pid == native.pid && process_identity_matches_kernel(native) {
                return Ok(());
            }
            if let Some(wrapper) = wrapper
                && pid == wrapper.pid
                && process_identity_matches_kernel(wrapper)
            {
                return Ok(());
            }
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
            let close = stat.rfind(')').context("invalid proc stat")?;
            let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
            let parent: u32 = fields.get(1).context("proc parent missing")?.parse()?;
            if parent == 0 || parent == pid {
                break;
            }
            pid = parent;
        }
        anyhow::bail!(
            "stale_process_identity: terminal caller belongs to neither current epoch {} native PID {} nor its bound wrapper {}",
            watchdog.state().process_epoch,
            native.pid,
            wrapper.map_or_else(|| "none".into(), |value| value.pid.to_string())
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let parent = unsafe { libc::getppid() as u32 };
        if parent != native.pid && wrapper.is_none_or(|value| value.pid != parent) {
            anyhow::bail!(
                "stale_process_identity: terminal caller parent belongs to neither current epoch {} native nor wrapper",
                watchdog.state().process_epoch
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn process_descends_from(mut candidate: u32, ancestor: u32) -> bool {
    for _ in 0..64 {
        if candidate == ancestor {
            return true;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{candidate}/stat")) else {
            return false;
        };
        let Some(close) = stat.rfind(')') else {
            return false;
        };
        let Some(parent) = stat[close + 2..]
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u32>().ok())
        else {
            return false;
        };
        if parent == 0 || parent == candidate {
            return false;
        }
        candidate = parent;
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn process_descends_from(candidate: u32, ancestor: u32) -> bool {
    candidate == ancestor || unsafe { libc::getppid() as u32 } == ancestor
}

#[cfg(target_os = "linux")]
fn process_identity_matches_kernel(process: &ProcessIdentity) -> bool {
    let boot_matches = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .is_some_and(|value| value.trim() == process.boot_id);
    let ticks_match = std::fs::read_to_string(format!("/proc/{}/stat", process.pid))
        .ok()
        .and_then(|stat| {
            let close = stat.rfind(')')?;
            stat[close + 2..]
                .split_whitespace()
                .nth(19)?
                .parse::<u64>()
                .ok()
        })
        == Some(process.start_ticks);
    boot_matches && ticks_match
}

fn process_exit(dir: &Path, id: &str, exit_code: i32, attested_pid: Option<u32>) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    let state = watchdog.state().clone();
    if let Some(pid) = attested_pid
        && pid != state.process.pid
    {
        anyhow::bail!(
            "stale_process_identity: wrapper exit PID {pid} is not current epoch {} PID {}",
            state.process_epoch,
            state.process.pid
        );
    }
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiProcessEpochExited {
                process_epoch: state.process_epoch,
                process_identity_digest: state.process.digest(),
                exact_reap_proof: true,
                effect_safe: state.exact_guards.effect,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-watchdog".into(),
            },
            "needs_finalization_exit",
            format!(
                "pi-exit:{}:{}",
                state.source.attempt_id, state.process_epoch
            ),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(e) = apply_transition(task, request) {
            rejection = Some(e);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    watchdog
        .observe(
            Observation::ProcessExited {
                status: ExitStatus::Code(exit_code),
                reaped: true,
            },
            Utc::now().timestamp(),
        )
        .map_err(anyhow::Error::new)?;
    Ok(())
}

fn fixture_init(dir: &Path, id: &str, worktree: &Path, now: i64) -> Result<()> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph.get_task_or_err(id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("fixture task must be claimed")?
        .clone();
    let runtime_key = worksgood::attempt_runtime::AttemptRuntimeKey::for_attempt(task, &attempt);
    let state_dir = worksgood::attempt_runtime::component_for_update(dir, &runtime_key, "pi")?;
    let session_dir = state_dir.join("session");
    std::fs::create_dir_all(&session_dir)?;
    let session_file = session_dir.join("fake-session.jsonl");
    std::fs::write(
        &session_file,
        "{\"type\":\"session\",\"version\":3,\"id\":\"fake-session\"}\n",
    )?;
    let source = SourceTuple {
        task_id: id.into(),
        generation: task.lifecycle.generation,
        attempt_id: attempt.id.clone(),
        attempt_fence: task.lifecycle.fence,
        worktree_lease_epoch: task.lifecycle.fence,
        worktree_path: worktree.to_owned(),
    };
    let route = RouteSnapshot {
        handler: "pi".into(),
        provider: "fake-free".into(),
        model: "fake-slow".into(),
        reasoning: Some("high".into()),
        endpoint_redacted: "fake://local".into(),
        endpoint_hmac: "fixture-endpoint".into(),
        qos: QosClass::Free,
        pi_binary_digest: "fake-pi-v1".into(),
        plugin_digest: "fake-plugin-v1".into(),
    };
    let session = SessionProof {
        session_id: "fake-session".into(),
        branch_leaf: "leaf-0".into(),
        session_dir,
        session_file,
        header_digest: "fixture-header".into(),
        append_prefix_digest: "fixture-prefix".into(),
        append_prefix_len: 1,
    };
    let process = ProcessIdentity {
        pid: std::process::id(),
        pgid: std::process::id(),
        start_ticks: 1,
        boot_id: "fixture-boot".into(),
        nonce: "fixture-nonce".into(),
    };
    let process_identity_digest = process.digest();
    let state_path = state_dir.join("state.json");
    PiWatchdog::new_at(
        state_path,
        source.clone(),
        route.clone(),
        session.clone(),
        process,
        WatchdogPolicy::default(),
        now,
    )
    .map_err(anyhow::Error::new)?;
    let authorization = worksgood::lifecycle::PiContinuationAuthorization {
        authorization_id: format!("fixture-auth:{}", attempt.id),
        task_id: id.into(),
        generation: source.generation,
        attempt_id: source.attempt_id,
        attempt_fence: source.attempt_fence,
        worktree_lease_epoch: source.worktree_lease_epoch,
        session_proof_digest: session.digest(),
        route_snapshot_digest: route.digest(),
        state: worksgood::lifecycle::PiAuthorizationState::Active,
        max_replacement_epochs: 3,
        max_reserved_elapsed_secs: 1800,
        epochs_used: 0,
        elapsed_reserved_secs: 0,
        issued_by_policy: "pi-watchdog-static-v1".into(),
    };
    let expected = FenceExpectation::current(task);
    let task_id = id.to_string();
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(&task_id) else {
            return false;
        };
        apply_transition(
            task,
            TransitionRequest::new(
                TransitionKind::PiContinuationAuthorized {
                    authorization: authorization.clone(),
                    initial_process_epoch: 1,
                    initial_process_identity_digest: process_identity_digest.clone(),
                },
                LifecycleActor {
                    kind: ActorKind::Dispatcher,
                    id: "fake-pi-fixture".into(),
                },
                "pi_authorized",
                format!("fixture-auth:{}", attempt.id),
            )
            .expecting(expected.clone()),
        )
        .is_ok()
    })?;
    println!(
        "Fake-Pi initialized: production soft=300s free/low hard>=900s grace=60s session=fake-session attempt={}",
        attempt.id
    );
    Ok(())
}

fn fixture_observe(dir: &Path, id: &str, event: &str, now: i64) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    // Credential-free native Pi records for TUI projection smoke tests. Raw
    // text canaries are intentionally present here to prove they never enter
    // the persisted UI-safe projection.
    let native = match event {
        "thinking-native" => Some(
            serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"RAW_REASONING_CANARY_7f3b","thinkingTokens":7}}),
        ),
        "thinking-unknown" => Some(
            serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"RAW_REASONING_CANARY_7f3b_UNKNOWN"}}),
        ),
        "output-5" => Some(
            serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"HOSTILE_OUTPUT_CANARY_91ac","outputTokens":5}}),
        ),
        "output-11" => Some(
            serde_json::json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"HOSTILE_OUTPUT_CANARY_91ac","outputTokens":11}}),
        ),
        "write-native" => Some(
            serde_json::json!({"type":"tool_execution_start","toolName":"write","toolClass":"write","toolCallId":"write-1"}),
        ),
        "tool-native" => Some(
            serde_json::json!({"type":"tool_execution_start","toolName":"bash","toolClass":"tool","toolCallId":"tool-1"}),
        ),
        "test-native" => Some(
            serde_json::json!({"type":"tool_execution_start","toolName":"test","toolClass":"test","toolCallId":"test-1"}),
        ),
        "tool-end-native" => Some(
            serde_json::json!({"type":"tool_execution_end","toolName":"test","toolCallId":"test-1","isError":false}),
        ),
        "usage-native" => Some(
            serde_json::json!({"type":"turn_end","turnId":"fixture-turn-1","message":{"usage":{"input":10,"output":11,"cacheRead":3,"cacheWrite":2,"totalTokens":26,"cost":{"total":0.25}}}}),
        ),
        _ => None,
    };
    if let Some(native) = native {
        let actions = watchdog
            .ingest_native_value(&native, now)
            .map_err(anyhow::Error::new)?;
        println!(
            "event={event} classification={:?} phase={:?} actions={actions:?} progress={} native-seq={}",
            watchdog.state().classification,
            watchdog.state().phase,
            watchdog.state().progress_seq,
            watchdog.state().native_activity.event_seq,
        );
        return Ok(());
    }
    let mut terminal_receipt = None;
    let observation = match event {
        "provider-start" => Observation::ProviderRequestStarted {
            call_id: "provider-1".into(),
        },
        "provider-retry" => Observation::ProviderRetry,
        "token" => Observation::TokenDelta { tokens: 1 },
        "thinking" => Observation::ThinkingDelta,
        "unknown" => Observation::PhaseUnknown,
        "settled" => Observation::AgentSettled,
        "exit-zero" => Observation::ProcessExited {
            status: ExitStatus::Code(0),
            reaped: true,
        },
        "exit-nonzero" => Observation::ProcessExited {
            status: ExitStatus::Code(9),
            reaped: true,
        },
        "eof" => Observation::PipeEof { reaped: true },
        "wait" => Observation::WaitAccepted {
            correlation: "fixture-answer".into(),
        },
        "long-tool" => Observation::ToolIntent {
            contract: ToolContract::read_only("fixture-tool", now + 10_000),
        },
        "unsafe-tool" => Observation::ToolIntent {
            contract: ToolContract::non_idempotent("fixture-danger"),
        },
        "probe" => Observation::ProbeObserved {
            progress_seq: watchdog.state().progress_seq,
            session_leaf: watchdog.state().session.branch_leaf.clone(),
            alive: true,
        },
        "launched" => Observation::ContinuationLaunched,
        "permit" => Observation::ExecutionPermitted,
        "done" | "fail" | "park" => {
            let disposition = match event {
                "done" => TerminalDisposition::SuccessIntent,
                "fail" => TerminalDisposition::Failure,
                _ => TerminalDisposition::Park,
            };
            let receipt = TerminalIntentReceipt::new(
                &watchdog,
                watchdog.state().process_epoch,
                format!("fixture-{event}"),
                disposition,
            );
            terminal_receipt = Some(receipt.clone());
            Observation::TerminalIntent(receipt)
        }
        _ => anyhow::bail!("unknown Fake-Pi event {event}"),
    };
    if let Some(receipt) = terminal_receipt.as_ref() {
        let graph_path = dir.join("graph.jsonl");
        let mut rejection = None;
        let receipt_for_graph = receipt.clone();
        modify_graph(&graph_path, |graph| {
            let Some(task) = graph.get_task_mut(id) else {
                return false;
            };
            let request = TransitionRequest::new(
                TransitionKind::PiTerminalIntent {
                    receipt: receipt_for_graph.clone(),
                },
                LifecycleActor {
                    kind: ActorKind::Worker,
                    id: task
                        .lifecycle
                        .current_attempt
                        .as_ref()
                        .map(|a| a.actor_id.clone())
                        .unwrap_or_else(|| "fake-pi-fixture".into()),
                },
                "fixture_terminal_intent",
                receipt_for_graph.idempotency_key.clone(),
            )
            .expecting(FenceExpectation::current(task));
            if let Err(error) = apply_transition(task, request) {
                rejection = Some(error);
                return false;
            }
            true
        })?;
        if let Some(error) = rejection {
            return Err(anyhow::Error::new(error));
        }
    }
    let actions = watchdog
        .observe(observation, now)
        .map_err(anyhow::Error::new)?;
    sync_lifecycle_continuation_authority(dir, id, &watchdog)?;
    println!(
        "event={event} classification={:?} actions={actions:?} process_epoch={} continuation_epoch={} prompts={} terminal={}",
        watchdog.state().classification,
        watchdog.state().process_epoch,
        watchdog.state().continuation_epoch,
        watchdog.state().prompt_count,
        watchdog.state().terminal
    );
    Ok(())
}

fn fixture_tick(dir: &Path, id: &str, now: i64) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    let actions = watchdog.tick(now).map_err(anyhow::Error::new)?;
    sync_lifecycle_continuation_authority(dir, id, &watchdog)?;
    println!(
        "tick={now} classification={:?} actions={actions:?} process_epoch={} continuation_epoch={} prompts={} budget={}/{}s",
        watchdog.state().classification,
        watchdog.state().process_epoch,
        watchdog.state().continuation_epoch,
        watchdog.state().prompt_count,
        watchdog.state().epochs_used,
        watchdog.state().elapsed_reserved_secs
    );
    Ok(())
}

fn abort(dir: &Path, id: &str, reason: &str, json: bool) -> Result<()> {
    if reason.trim().is_empty() {
        anyhow::bail!("operator abort requires --reason");
    }
    let mut watchdog = checked_open(dir, id)?;
    let state = watchdog.state().clone();
    let receipt = TerminalIntentReceipt {
        task_id: id.into(),
        generation: state.source.generation,
        attempt_id: state.source.attempt_id.clone(),
        attempt_fence: state.source.attempt_fence,
        process_epoch: state.process_epoch,
        process_identity_digest: state.process.digest(),
        tool_call_id: format!(
            "operator-abort:{}:{}",
            state.process_epoch,
            blake3::hash(reason.as_bytes()).to_hex()
        ),
        disposition: TerminalDisposition::Abort,
        idempotency_key: format!(
            "pi-operator-abort:{}:{}",
            state.source.attempt_id, state.process_epoch
        ),
    };
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    let receipt_for_graph = receipt.clone();
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiTerminalIntent {
                receipt: receipt_for_graph.clone(),
            },
            LifecycleActor {
                kind: ActorKind::Operator,
                id: worksgood::current_user(),
            },
            "operator_abort",
            receipt_for_graph.idempotency_key.clone(),
        )
        .expecting(FenceExpectation::current(task))
        .with_evidence(format!(
            "operator-reason:b3:{}",
            blake3::hash(reason.as_bytes()).to_hex()
        ));
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    watchdog
        .observe(Observation::TerminalIntent(receipt), Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    if json {
        println!(
            "{{\"task\":{},\"reason_code\":\"operator_abort\"}}",
            serde_json::to_string(id)?
        );
    } else {
        println!(
            "Operator abort accepted for '{}' by first-terminal-wins lifecycle CAS",
            id
        );
    }
    Ok(())
}
