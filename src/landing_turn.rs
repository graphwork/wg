//! Persistent, ref-scoped FIFO landing-turn queue + renewable lease.
//!
//! Concurrent source agents may implement independently, but only one source
//! agent at a time may integrate against the canonical target. The source
//! agent that understands its change owns conflict resolution, renewed
//! validation, and resubmission; WG owns the *queue*, the *lease*, the
//! *fencing*, and the final compare-and-fast-forward.
//!
//! ## Authority model
//!
//! The persisted lease is the authority. An OS `flock` on a sidecar `.lock`
//! file protects only *short, bounded* queue/lease mutations — it is **never**
//! held across an unbounded model call. A worker requests its landing turn only
//! when its candidate is ready; it does not hold the lease during ordinary
//! implementation. If another landing owns the lease, the requester atomically
//! *parks* through the existing `AttemptParked` / `Waiting` machinery (see
//! [`crate::commands::completion_wait`]) with a typed `landing-turn` wait
//! condition and checkpoint, releasing worker/build capacity while retaining the
//! exact worktree, candidate, Pi session continuation, and queue ticket.
//!
//! ## Safety invariants
//!
//! - **Exact binding.** A ticket is bound to `(task, generation, attempt,
//!   fence, candidate_sequence, source_agent/session, integration_ref,
//!   observed_target_oid)`. Final publication is allowed only for the current
//!   lease owner *and* the exact target/candidate/fence.
//! - **FIFO fairness.** Tickets are ordered by a monotonic `seq`; acquisition
//!   is deterministic and starvation-free (every ticket eventually reaches the
//!   head because no lease can block forever).
//! - **No lease blocks forever.** Lease duration is bounded; renewal requires
//!   *proven* progress. Death, timeout, cancellation, stale attempt, or failed
//!   renewal fences the old owner and advances safely. An expired lease is
//!   auto-fenced on the next mutation.
//! - **Source-agent ownership.** Only the exact source agent/capability bound
//!   to a ticket may resolve and land its candidate; a content conflict returns
//!   ownership to the source agent (the queue keeps the ticket at the head)
//!   rather than stranding a released worker or invoking an unrelated generic
//!   merger by default.

use crate::lock::{RetryPolicy, is_transient_blocking, retry_acquire};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Schema version for the persisted landing-turn state. Bumped on incompatible
/// changes to [`LandingTurnState`] / [`LandingTicket`] / [`LandingLease`].
pub const LANDING_TURN_SCHEMA_VERSION: u32 = 2;

/// Default bound on a single lease term (wall-clock). Renewal may extend it,
/// but only against proven progress.
pub const DEFAULT_LEASE_TERM_SECS: u64 = 10 * 60;

/// Hard ceiling on lease renewals without proven progress: a lease that has not
/// made proven progress within this many consecutive renewals is fenced.
pub const DEFAULT_MAX_RENEWALS_WITHOUT_PROGRESS: u32 = 3;

/// A FIFO ticket binding. Exact, restart-safe: resumption must prove
/// byte-for-byte that the candidate/attempt/fence/agent are still the ones that
/// entered the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandingTicket {
    /// Stable ticket id (`ticket-<seq>`).
    pub ticket_id: String,
    /// Monotonic FIFO sequence. Lower `seq` lands first.
    pub seq: u64,
    pub task_id: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub fence: u64,
    /// Candidate sequence within the attempt (defends against a resubmitted
    /// manifest that changes integration bytes).
    pub candidate_sequence: u64,
    pub candidate_oid: String,
    /// Source agent id bound at request time (the managed worker capability).
    pub source_agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session: Option<String>,
    pub integration_ref: String,
    /// Target OID observed by the source agent when it requested the turn.
    pub observed_target_oid: String,
    pub created_at: String,
    /// Diagnostic acknowledgement that the daemon observed this ticket ready.
    /// Readiness remains retryable until the continuation reacquires it.
    #[serde(default)]
    pub resumed_once: bool,
}

impl LandingTicket {
    fn matches_binding(&self, binding: &TicketBinding) -> bool {
        self.task_id == binding.task_id
            && self.generation == binding.generation
            && self.attempt_id == binding.attempt_id
            && self.fence == binding.fence
            && self.candidate_sequence == binding.candidate_sequence
            && self.candidate_oid == binding.candidate_oid
            && self.source_agent == binding.source_agent
            && self.source_session == binding.source_session
            && self.integration_ref == binding.integration_ref
            && self.observed_target_oid == binding.observed_target_oid
    }

    fn matches_owner_epoch(&self, binding: &TicketBinding) -> bool {
        self.task_id == binding.task_id
            && self.generation == binding.generation
            && self.attempt_id == binding.attempt_id
            && self.fence == binding.fence
            && self.source_agent == binding.source_agent
            && self.source_session == binding.source_session
            && self.integration_ref == binding.integration_ref
    }

    fn matches_continuation(&self, binding: &TicketBinding) -> bool {
        self.task_id == binding.task_id
            && self.generation == binding.generation
            && self.integration_ref == binding.integration_ref
            && self.source_session.is_some()
            && self.source_session == binding.source_session
    }

    fn rebind(&mut self, binding: &TicketBinding) {
        self.attempt_id.clone_from(&binding.attempt_id);
        self.fence = binding.fence;
        self.candidate_sequence = binding.candidate_sequence;
        self.candidate_oid.clone_from(&binding.candidate_oid);
        self.source_agent.clone_from(&binding.source_agent);
        self.source_session.clone_from(&binding.source_session);
        self.observed_target_oid
            .clone_from(&binding.observed_target_oid);
    }
}

/// The active lease. The lease is the authority; the OS lock only guards short
/// mutations. A lease is acquired only by the head ticket when the lease is
/// free, and released/fenced on success, death, timeout, cancellation, stale
/// attempt, or failed renewal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandingLease {
    pub ticket_id: String,
    pub seq: u64,
    pub owner_agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session: Option<String>,
    pub task_id: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub fence: u64,
    pub candidate_sequence: u64,
    pub candidate_oid: String,
    pub integration_ref: String,
    pub target_oid: String,
    pub acquired_at: String,
    /// RFC 3339; the lease is auto-fenced after this instant.
    pub expires_at: String,
    #[serde(default)]
    pub last_renewed_at: Option<String>,
    #[serde(default)]
    pub renewals: u32,
    #[serde(default)]
    pub renewals_without_progress: u32,
    /// Opaque progress token supplied at the last successful renewal. Renewal
    /// is "proven" only when this changes; an unchanged token does not refresh
    /// `renewals_without_progress`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_token: Option<String>,
}

impl LandingLease {
    fn matches_binding(&self, binding: &TicketBinding) -> bool {
        self.task_id == binding.task_id
            && self.generation == binding.generation
            && self.attempt_id == binding.attempt_id
            && self.fence == binding.fence
            && self.candidate_sequence == binding.candidate_sequence
            && self.candidate_oid == binding.candidate_oid
            && self.owner_agent == binding.source_agent
            && self.source_session == binding.source_session
            && self.integration_ref == binding.integration_ref
            && self.target_oid == binding.observed_target_oid
    }

    fn matches_owner_epoch(&self, binding: &TicketBinding) -> bool {
        self.task_id == binding.task_id
            && self.generation == binding.generation
            && self.attempt_id == binding.attempt_id
            && self.fence == binding.fence
            && self.owner_agent == binding.source_agent
            && self.source_session == binding.source_session
            && self.integration_ref == binding.integration_ref
    }

    fn rebind(&mut self, binding: &TicketBinding, lease_term: u64) {
        self.candidate_sequence = binding.candidate_sequence;
        self.candidate_oid.clone_from(&binding.candidate_oid);
        self.target_oid.clone_from(&binding.observed_target_oid);
        self.expires_at = now_plus(lease_term);
        self.progress_token = Some(format!("candidate:{}", binding.candidate_sequence));
        self.renewals_without_progress = 0;
    }
}

/// The persisted, ref-scoped landing-turn state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandingTurnState {
    pub schema_version: u32,
    pub integration_ref: String,
    /// FIFO by `seq`. The head is `tickets.first()`.
    pub tickets: Vec<LandingTicket>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LandingLease>,
    /// Next monotonic seq to assign.
    pub next_seq: u64,
    /// Optional override of the lease term (seconds). Defaults to
    /// [`DEFAULT_LEASE_TERM_SECS`] when zero.
    #[serde(default)]
    pub lease_term_secs: u64,
    #[serde(default)]
    pub max_renewals_without_progress: u32,
}

impl LandingTurnState {
    fn lease_term(&self) -> u64 {
        if self.lease_term_secs == 0 {
            DEFAULT_LEASE_TERM_SECS
        } else {
            self.lease_term_secs
        }
    }
    fn max_renewals_no_progress(&self) -> u32 {
        if self.max_renewals_without_progress == 0 {
            DEFAULT_MAX_RENEWALS_WITHOUT_PROGRESS
        } else {
            self.max_renewals_without_progress
        }
    }
}

/// The exact binding a source agent presents. The requestor *must* present the
/// binding of the task it is managed by; the queue refuses arbitrary task ids
/// (the caller is expected to bind to `$WG_TASK_ID` / the managed capability).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketBinding {
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: Option<String>,
    pub fence: u64,
    pub candidate_sequence: u64,
    pub candidate_oid: String,
    pub source_agent: String,
    pub source_session: Option<String>,
    pub integration_ref: String,
    pub observed_target_oid: String,
}

/// Outcome of [`request_turn`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "outcome")]
pub enum RequestOutcome {
    /// The requester acquired the lease and may proceed to integrate.
    Acquired {
        ticket_id: String,
        seq: u64,
        expires_at: String,
    },
    /// Another landing owns the lease (or an earlier ticket is queued); the
    /// requester must park through `AttemptParked` with a typed `landing-turn`
    /// wait condition and checkpoint.
    Parked {
        ticket_id: String,
        seq: u64,
        /// Position in the queue (1 = head, i.e. next to acquire).
        position: u64,
        owner: Option<String>,
        owner_expires_at: Option<String>,
    },
    /// The requester already holds the lease (idempotent re-request).
    AlreadyOwner {
        ticket_id: String,
        seq: u64,
        expires_at: String,
    },
}

/// Outcome of [`release_turn`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "outcome")]
pub enum ReleaseOutcome {
    Released {
        ticket_id: String,
        /// The next ticket to wake (head after pop), if any.
        next: Option<String>,
    },
    NotOwner {
        ticket_id: Option<String>,
        owner: Option<String>,
    },
    NotFound,
}

/// Outcome of [`renew_turn`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "outcome")]
pub enum RenewOutcome {
    Renewed {
        ticket_id: String,
        expires_at: String,
        renewals: u32,
    },
    NotOwner,
    Expired,
    /// Renewal refused: no proven progress for too many consecutive renewals.
    StalledFenced {
        ticket_id: String,
        next: Option<String>,
    },
}

/// Outcome of [`reclaim_turn`] (operator force-fence) and auto-fencing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "outcome")]
pub enum ReclaimOutcome {
    Fenced {
        ticket_id: String,
        fenced_owner: String,
        reason: String,
        next: Option<String>,
    },
    NoLease,
    NotExpired {
        ticket_id: String,
        owner: String,
        expires_at: String,
    },
}

/// Status snapshot for [`status`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandingTurnStatus {
    pub integration_ref: String,
    pub queue_len: usize,
    pub position: Option<u64>,
    pub ticket: Option<LandingTicket>,
    pub lease: Option<LandingLease>,
    pub expired: bool,
}

/// Where landing-turn state lives for a given graph dir + integration ref.
///
/// State is persisted under WG control state: `<graph_dir>/landing-turns/<slug>.json`
/// with a sidecar `<slug>.lock` for the short OS lock. The slug is derived from
/// the integration ref so different refs (e.g. `refs/heads/main` vs an
/// alternate integration branch) get independent queues.
pub fn state_path(dir: &Path, integration_ref: &str) -> PathBuf {
    dir.join("landing-turns")
        .join(format!("{}.json", slug(integration_ref)))
}

fn lock_path(dir: &Path, integration_ref: &str) -> PathBuf {
    dir.join("landing-turns")
        .join(format!("{}.lock", slug(integration_ref)))
}

fn slug(integration_ref: &str) -> String {
    let prefix: String = integration_ref
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '-',
        })
        .take(80)
        .collect();
    let digest = blake3::hash(integration_ref.as_bytes()).to_hex();
    format!("{prefix}-{}", &digest[..16])
}

/// A short OS lock held only across a bounded mutation. Never held across a
/// model call. Dropping it releases the flock.
struct QueueLock {
    _file: File,
}

impl QueueLock {
    fn acquire(dir: &Path, integration_ref: &str) -> Result<Self> {
        let lock = lock_path(dir, integration_ref);
        if let Some(parent) = lock.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create landing-turns dir {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock)
            .with_context(|| format!("open landing-turn lock {}", lock.display()))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = file.as_raw_fd();
            retry_acquire(&RetryPolicy::default(), is_transient_blocking, || {
                let result = unsafe { libc::flock(fd, libc::LOCK_EX) };
                if result == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            })
            .context("acquire landing-turn queue lock")?;
        }
        Ok(Self { _file: file })
    }
}

impl Drop for QueueLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

fn load_state(dir: &Path, integration_ref: &str) -> Result<Option<LandingTurnState>> {
    let path = state_path(dir, integration_ref);
    let mut file = match OpenOptions::new().read(true).open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("open landing-turn state {}", path.display()));
        }
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("read landing-turn state {}", path.display()))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let state: LandingTurnState = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse landing-turn state {}", path.display()))?;
    if state.schema_version != LANDING_TURN_SCHEMA_VERSION {
        bail!(
            "landing-turn state schema mismatch: expected {}, found {} (integration ref {})",
            LANDING_TURN_SCHEMA_VERSION,
            state.schema_version,
            integration_ref
        );
    }
    if state.integration_ref != integration_ref {
        bail!(
            "landing-turn state/ref mismatch: locked {:?}, found {:?}",
            integration_ref,
            state.integration_ref
        );
    }
    Ok(Some(state))
}

fn save_state(state: &LandingTurnState, dir: &Path) -> Result<()> {
    let path = state_path(dir, &state.integration_ref);
    let bytes = serde_json::to_vec_pretty(state)
        .with_context(|| format!("serialize landing-turn state for {}", state.integration_ref))?;
    crate::atomic_file::write_atomic(&path, &bytes)
        .with_context(|| format!("persist landing-turn state {}", path.display()))?;
    Ok(())
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("parse timestamp {s:?}"))
}

fn now_plus(seconds: u64) -> String {
    (Utc::now() + chrono::Duration::seconds(seconds as i64)).to_rfc3339()
}

fn is_expired(lease: &LandingLease, now: &DateTime<Utc>) -> bool {
    parse_ts(&lease.expires_at)
        .map(|exp| &exp < now)
        .unwrap_or(true)
}

/// Remove any ticket for this exact binding (used on release after landing).
fn remove_ticket(state: &mut LandingTurnState, binding: &TicketBinding) -> Option<LandingTicket> {
    let pos = state
        .tickets
        .iter()
        .position(|ticket| ticket.matches_binding(binding))?;
    Some(state.tickets.remove(pos))
}

fn head_ticket(state: &LandingTurnState) -> Option<&LandingTicket> {
    state.tickets.first()
}

fn lease_for_ticket(ticket: &LandingTicket, lease_term: u64, now: &DateTime<Utc>) -> LandingLease {
    LandingLease {
        ticket_id: ticket.ticket_id.clone(),
        seq: ticket.seq,
        owner_agent: ticket.source_agent.clone(),
        source_session: ticket.source_session.clone(),
        task_id: ticket.task_id.clone(),
        generation: ticket.generation,
        attempt_id: ticket.attempt_id.clone(),
        fence: ticket.fence,
        candidate_sequence: ticket.candidate_sequence,
        candidate_oid: ticket.candidate_oid.clone(),
        integration_ref: ticket.integration_ref.clone(),
        target_oid: ticket.observed_target_oid.clone(),
        acquired_at: now.to_rfc3339(),
        expires_at: now_plus(lease_term),
        last_renewed_at: None,
        renewals: 0,
        renewals_without_progress: 0,
        progress_token: None,
    }
}

fn prune_non_live_tickets(dir: &Path, state: &mut LandingTurnState) -> Result<bool> {
    let graph_path = dir.join("graph.jsonl");
    if !graph_path.exists() {
        return Ok(false);
    }
    let graph = crate::parser::load_graph(&graph_path)?;
    let before = state.tickets.len();
    state.tickets.retain(|ticket| {
        graph.get_task(&ticket.task_id).is_some_and(|task| {
            task.lifecycle.generation == ticket.generation
                && matches!(
                    task.status,
                    crate::graph::Status::Open
                        | crate::graph::Status::InProgress
                        | crate::graph::Status::Waiting
                )
        })
    });
    let mut removed = before != state.tickets.len();
    if state.lease.as_ref().is_some_and(|lease| {
        !state
            .tickets
            .iter()
            .any(|ticket| ticket.ticket_id == lease.ticket_id)
    }) {
        state.lease = None;
        removed = true;
    }
    Ok(removed)
}

/// Auto-fence an expired lease (if any) and return the reclaim outcome so the
/// caller can wake the next ticket. Called at the start of every mutation.
fn fence_if_expired(state: &mut LandingTurnState, now: &DateTime<Utc>) -> Option<ReclaimOutcome> {
    let lease = state.lease.as_ref()?;
    if !is_expired(lease, now) {
        return None;
    }
    let fenced_ticket_id = lease.ticket_id.clone();
    let fenced_owner = lease.owner_agent.clone();
    let fenced_seq = lease.seq;
    let expires_at = lease.expires_at.clone();
    // Drop the fenced ticket from the queue (its owner is dead/stalled).
    state.tickets.retain(|t| t.ticket_id != fenced_ticket_id);
    state.lease = None;
    let next = head_ticket(state).map(|t| t.ticket_id.clone());
    Some(ReclaimOutcome::Fenced {
        ticket_id: fenced_ticket_id,
        fenced_owner,
        reason: format!(
            "lease expired at {} (seq {}); auto-fenced at {}",
            expires_at,
            fenced_seq,
            now.to_rfc3339()
        ),
        next,
    })
}

/// Request a landing turn for the exact [`TicketBinding`].
///
/// Returns [`RequestOutcome::Acquired`] if the lease is free and this ticket is
/// at the head, [`RequestOutcome::Parked`] otherwise. Idempotent: if the same
/// binding already holds the lease, returns [`RequestOutcome::AlreadyOwner`].
pub fn request_turn(dir: &Path, binding: &TicketBinding) -> Result<RequestOutcome> {
    let _lock = QueueLock::acquire(dir, &binding.integration_ref)?;
    let mut state =
        load_state(dir, &binding.integration_ref)?.unwrap_or_else(|| LandingTurnState {
            schema_version: LANDING_TURN_SCHEMA_VERSION,
            integration_ref: binding.integration_ref.clone(),
            tickets: Vec::new(),
            lease: None,
            next_seq: 1,
            lease_term_secs: 0,
            max_renewals_without_progress: 0,
        });
    let now = Utc::now();
    let expired =
        prune_non_live_tickets(dir, &mut state)? | fence_if_expired(&mut state, &now).is_some();

    // A source owner that resolved a conflict may bind a newer reviewed
    // candidate without losing its FIFO position. Rebinding the same sequence
    // to a changed target is forbidden: changed integration bytes require new
    // validation/review evidence.
    if let Some(lease) = state.lease.as_ref()
        && lease.matches_owner_epoch(binding)
        && binding.candidate_sequence > lease.candidate_sequence
    {
        let ticket_id = lease.ticket_id.clone();
        let lease_term = state.lease_term();
        state
            .tickets
            .iter_mut()
            .find(|ticket| ticket.ticket_id == ticket_id)
            .context("landing lease has no queue ticket")?
            .rebind(binding);
        let lease = state.lease.as_mut().expect("lease checked above");
        lease.rebind(binding, lease_term);
        let outcome = RequestOutcome::AlreadyOwner {
            ticket_id: lease.ticket_id.clone(),
            seq: lease.seq,
            expires_at: lease.expires_at.clone(),
        };
        save_state(&state, dir)?;
        return Ok(outcome);
    }

    // WaitSatisfied may start a replacement process/capability. Transfer only
    // a coordinator-acknowledged FIFO head, and only across the exact attested
    // Pi session/task/generation/ref with a newly reviewed candidate.
    if state.lease.is_none()
        && let Some(index) = state
            .tickets
            .iter()
            .position(|ticket| ticket.resumed_once && ticket.matches_continuation(binding))
    {
        if index != 0 {
            bail!("landing continuation ticket is not the FIFO head");
        }
        let old_sequence = state.tickets[index].candidate_sequence;
        if binding.candidate_sequence <= old_sequence {
            bail!(
                "landing continuation requires a newly validated candidate sequence (old {}, new {})",
                old_sequence,
                binding.candidate_sequence
            );
        }
        state.tickets[index].rebind(binding);
        let ticket = state.tickets[index].clone();
        let lease = lease_for_ticket(&ticket, state.lease_term(), &now);
        let outcome = RequestOutcome::Acquired {
            ticket_id: lease.ticket_id.clone(),
            seq: lease.seq,
            expires_at: lease.expires_at.clone(),
        };
        state.lease = Some(lease);
        save_state(&state, dir)?;
        return Ok(outcome);
    }

    // Idempotent: already the lease owner?
    if let Some(lease) = state.lease.as_ref()
        && lease.matches_binding(binding)
    {
        return Ok(RequestOutcome::AlreadyOwner {
            ticket_id: lease.ticket_id.clone(),
            seq: lease.seq,
            expires_at: lease.expires_at.clone(),
        });
    }

    // Idempotent: already queued? Return the existing ticket's parking view.
    if let Some(existing) = state
        .tickets
        .iter()
        .find(|ticket| ticket.matches_binding(binding))
        .cloned()
    {
        // The ticket is already queued. If it is now at the head and the lease
        // is free, promote it to the lease (the source agent is taking its
        // turn after being woken). Otherwise report the parked position.
        let is_head = state
            .tickets
            .first()
            .is_some_and(|head| head.ticket_id == existing.ticket_id);
        if is_head && state.lease.is_none() {
            let lease = lease_for_ticket(&existing, state.lease_term(), &now);
            let expires_at = lease.expires_at.clone();
            let ticket_id = lease.ticket_id.clone();
            let seq = lease.seq;
            state.lease = Some(lease);
            save_state(&state, dir)?;
            return Ok(RequestOutcome::Acquired {
                ticket_id,
                seq,
                expires_at,
            });
        }
        let position = state
            .tickets
            .iter()
            .position(|t| t.seq >= existing.seq)
            .map(|p| p as u64 + 1)
            .unwrap_or(state.tickets.len() as u64);
        let (owner, owner_expires) = state
            .lease
            .as_ref()
            .map(|l| (Some(l.owner_agent.clone()), Some(l.expires_at.clone())))
            .unwrap_or((None, None));
        if expired {
            save_state(&state, dir)?;
        }
        return Ok(RequestOutcome::Parked {
            ticket_id: existing.ticket_id,
            seq: existing.seq,
            position,
            owner,
            owner_expires_at: owner_expires,
        });
    }

    if state
        .tickets
        .iter()
        .any(|ticket| ticket.matches_owner_epoch(binding) || ticket.matches_continuation(binding))
    {
        if expired {
            save_state(&state, dir)?;
        }
        bail!(
            "landing ticket binding changed without a newer continuation candidate; retain the existing ticket and renew source validation"
        );
    }

    let seq = state.next_seq;
    state.next_seq += 1;
    let ticket = LandingTicket {
        ticket_id: format!("ticket-{seq}"),
        seq,
        task_id: binding.task_id.clone(),
        generation: binding.generation,
        attempt_id: binding.attempt_id.clone(),
        fence: binding.fence,
        candidate_sequence: binding.candidate_sequence,
        candidate_oid: binding.candidate_oid.clone(),
        source_agent: binding.source_agent.clone(),
        source_session: binding.source_session.clone(),
        integration_ref: binding.integration_ref.clone(),
        observed_target_oid: binding.observed_target_oid.clone(),
        created_at: now.to_rfc3339(),
        resumed_once: false,
    };

    // If the lease is free and this is the head, acquire immediately.
    if state.lease.is_none() && state.tickets.is_empty() {
        let lease = lease_for_ticket(&ticket, state.lease_term(), &now);
        state.lease = Some(lease.clone());
        state.tickets.push(ticket);
        save_state(&state, dir)?;
        return Ok(RequestOutcome::Acquired {
            ticket_id: lease.ticket_id,
            seq: lease.seq,
            expires_at: lease.expires_at,
        });
    }

    // Otherwise queue and park.
    state.tickets.push(ticket.clone());
    let position = state.tickets.len() as u64;
    let (owner, owner_expires) = state
        .lease
        .as_ref()
        .map(|l| (Some(l.owner_agent.clone()), Some(l.expires_at.clone())))
        .unwrap_or((None, None));
    save_state(&state, dir)?;
    Ok(RequestOutcome::Parked {
        ticket_id: ticket.ticket_id,
        seq: ticket.seq,
        position,
        owner,
        owner_expires_at: owner_expires,
    })
}

/// Relinquish an exact live lease while retaining its FIFO ticket at the
/// queue head. Durable `LandingPending` uses this before releasing the source
/// worker: restart may reacquire this exact ticket, but no later ticket can
/// overtake it. A forged or stale binding has no effect.
pub fn park_owned_turn(dir: &Path, integration_ref: &str, binding: &TicketBinding) -> Result<bool> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let Some(mut state) = load_state(dir, integration_ref)? else {
        return Ok(false);
    };
    let expired = fence_if_expired(&mut state, &Utc::now()).is_some();
    let Some(lease) = state.lease.as_ref() else {
        if expired {
            save_state(&state, dir)?;
        }
        return Ok(false);
    };
    if !lease.matches_binding(binding) {
        return Ok(false);
    }
    state.lease = None;
    save_state(&state, dir)?;
    Ok(true)
}

/// Release the lease after a successful landing (or on giving up). Only the
/// current lease owner with an exact binding may release. Pops the head ticket
/// and returns the next ticket id to wake (if any).
pub fn release_turn(
    dir: &Path,
    integration_ref: &str,
    binding: &TicketBinding,
) -> Result<ReleaseOutcome> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state = match load_state(dir, integration_ref)? {
        Some(s) => s,
        None => return Ok(ReleaseOutcome::NotFound),
    };
    let now = Utc::now();
    let expired = fence_if_expired(&mut state, &now).is_some();

    let Some(lease) = state.lease.as_ref() else {
        if expired {
            save_state(&state, dir)?;
        }
        return if state
            .tickets
            .iter()
            .any(|ticket| ticket.matches_binding(binding))
        {
            Ok(ReleaseOutcome::NotOwner {
                ticket_id: None,
                owner: None,
            })
        } else {
            Ok(ReleaseOutcome::NotFound)
        };
    };

    if !lease.matches_binding(binding) {
        return Ok(ReleaseOutcome::NotOwner {
            ticket_id: Some(lease.ticket_id.clone()),
            owner: Some(lease.owner_agent.clone()),
        });
    }

    let released_ticket_id = lease.ticket_id.clone();
    state.lease = None;
    remove_ticket(&mut state, binding);
    let next = head_ticket(&state).map(|t| t.ticket_id.clone());
    save_state(&state, dir)?;
    Ok(ReleaseOutcome::Released {
        ticket_id: released_ticket_id,
        next,
    })
}

/// Renew the lease. Renewal is bounded to proven progress: `progress_token`
/// must differ from the last recorded token, or it counts toward
/// `renewals_without_progress`. When that exceeds the cap, the lease is fenced
/// and the next ticket is woken (starvation-freedom for queued owners).
pub fn renew_turn(
    dir: &Path,
    integration_ref: &str,
    binding: &TicketBinding,
    progress_token: Option<&str>,
) -> Result<RenewOutcome> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state = match load_state(dir, integration_ref)? {
        Some(s) => s,
        None => return Ok(RenewOutcome::NotOwner),
    };
    let now = Utc::now();
    if let Some(ReclaimOutcome::Fenced { next, .. }) = fence_if_expired(&mut state, &now) {
        save_state(&state, dir)?;
        return Ok(RenewOutcome::StalledFenced {
            ticket_id: binding.task_id.clone(),
            next,
        });
    }
    let max_no_progress = state.max_renewals_no_progress();
    let lease_term = state.lease_term();
    let Some(lease) = state.lease.as_mut() else {
        return Ok(RenewOutcome::NotOwner);
    };
    if !lease.matches_binding(binding) {
        return Ok(RenewOutcome::NotOwner);
    }
    let proven = progress_token.is_some() && progress_token != lease.progress_token.as_deref();
    lease.renewals = lease.renewals.saturating_add(1);
    if proven {
        lease.renewals_without_progress = 0;
        lease.progress_token = progress_token.map(|s| s.to_string());
    } else {
        lease.renewals_without_progress = lease.renewals_without_progress.saturating_add(1);
        if lease.renewals_without_progress > max_no_progress {
            let fenced_ticket_id = lease.ticket_id.clone();
            state.tickets.retain(|t| t.ticket_id != fenced_ticket_id);
            state.lease = None;
            let next = head_ticket(&state).map(|t| t.ticket_id.clone());
            save_state(&state, dir)?;
            return Ok(RenewOutcome::StalledFenced {
                ticket_id: fenced_ticket_id,
                next,
            });
        }
    }
    lease.last_renewed_at = Some(now.to_rfc3339());
    lease.expires_at = now_plus(lease_term);
    let expires_at = lease.expires_at.clone();
    let renewals = lease.renewals;
    let ticket_id = lease.ticket_id.clone();
    save_state(&state, dir)?;
    Ok(RenewOutcome::Renewed {
        ticket_id,
        expires_at,
        renewals,
    })
}

/// Operator (or auto-expiry) force-fence of the current lease. Fences the old
/// owner and advances to the next ticket, returning the next ticket id.
pub fn reclaim_turn(
    dir: &Path,
    integration_ref: &str,
    reason: &str,
    force: bool,
) -> Result<ReclaimOutcome> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state = match load_state(dir, integration_ref)? {
        Some(s) => s,
        None => return Ok(ReclaimOutcome::NoLease),
    };
    let now = Utc::now();
    let Some(lease) = state.lease.as_ref() else {
        return Ok(ReclaimOutcome::NoLease);
    };
    if !force && !is_expired(lease, &now) {
        return Ok(ReclaimOutcome::NotExpired {
            ticket_id: lease.ticket_id.clone(),
            owner: lease.owner_agent.clone(),
            expires_at: lease.expires_at.clone(),
        });
    }
    let fenced_ticket_id = lease.ticket_id.clone();
    let fenced_owner = lease.owner_agent.clone();
    state.tickets.retain(|t| t.ticket_id != fenced_ticket_id);
    state.lease = None;
    let next = head_ticket(&state).map(|t| t.ticket_id.clone());
    save_state(&state, dir)?;
    Ok(ReclaimOutcome::Fenced {
        ticket_id: fenced_ticket_id,
        fenced_owner,
        reason: reason.to_string(),
        next,
    })
}

/// Snapshot status for a given integration ref, optionally scoped to a task.
/// Auto-fences an expired lease first (so `status` never reports a dead owner
/// as live).
pub fn status(
    dir: &Path,
    integration_ref: &str,
    task_id: Option<&str>,
) -> Result<LandingTurnStatus> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state = match load_state(dir, integration_ref)? {
        Some(s) => s,
        None => {
            return Ok(LandingTurnStatus {
                integration_ref: integration_ref.to_string(),
                queue_len: 0,
                position: None,
                ticket: None,
                lease: None,
                expired: false,
            });
        }
    };
    let now = Utc::now();
    let fenced =
        prune_non_live_tickets(dir, &mut state)? | fence_if_expired(&mut state, &now).is_some();
    if fenced {
        save_state(&state, dir)?;
    }
    let lease = state.lease.clone();
    let expired = false;
    let ticket = match task_id {
        Some(id) => state.tickets.iter().find(|t| t.task_id == id).cloned(),
        None => state.tickets.first().cloned(),
    };
    let position = match task_id {
        Some(id) => state
            .tickets
            .iter()
            .position(|t| t.task_id == id)
            .map(|p| p as u64 + 1),
        None => state.tickets.first().map(|_| 1u64),
    };
    Ok(LandingTurnStatus {
        integration_ref: state.integration_ref,
        queue_len: state.tickets.len(),
        position,
        ticket,
        lease,
        expired,
    })
}

/// Atomically test whether an exact ticket is the resumable FIFO head.
/// Readiness remains true until the continuation reacquires the ticket, so a
/// crash between queue observation and graph persistence is restart-safe.
pub fn ticket_ready(dir: &Path, integration_ref: &str, ticket_id: &str) -> Result<bool> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state = match load_state(dir, integration_ref)? {
        Some(state) => state,
        None => return Ok(false),
    };
    let now = Utc::now();
    let fenced =
        prune_non_live_tickets(dir, &mut state)? | fence_if_expired(&mut state, &now).is_some();
    if state.lease.is_some() {
        if fenced {
            save_state(&state, dir)?;
        }
        return Ok(false);
    }
    let ready = state
        .tickets
        .first()
        .is_some_and(|ticket| ticket.ticket_id == ticket_id);
    if ready {
        state.tickets[0].resumed_once = true;
    }
    if ready || fenced {
        save_state(&state, dir)?;
    }
    Ok(ready)
}

/// Legacy state-machine helper. Production uses retryable [`ticket_ready`].
pub fn take_resumable_head(dir: &Path, integration_ref: &str) -> Result<Option<LandingTicket>> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state = match load_state(dir, integration_ref)? {
        Some(s) => s,
        None => return Ok(None),
    };
    let now = Utc::now();
    let fenced = fence_if_expired(&mut state, &now).is_some();
    // Only resume the head if the lease is free (a live lease belongs to its
    // owner, who is presumed still integrating).
    if state.lease.is_some() {
        if fenced {
            save_state(&state, dir)?;
        }
        return Ok(None);
    }
    let Some(head) = state.tickets.first_mut() else {
        if fenced {
            save_state(&state, dir)?;
        }
        return Ok(None);
    };
    if head.resumed_once {
        return Ok(None);
    }
    head.resumed_once = true;
    let ticket = head.clone();
    save_state(&state, dir)?;
    Ok(Some(ticket))
}

/// Cancel a queued ticket (e.g. the source agent gave up / the attempt was
/// abandoned). Only the exact binding may cancel its own ticket. If the ticket
/// held the lease, the lease is fenced and the next ticket is woken.
pub fn cancel_turn(
    dir: &Path,
    integration_ref: &str,
    binding: &TicketBinding,
) -> Result<ReleaseOutcome> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state = match load_state(dir, integration_ref)? {
        Some(s) => s,
        None => return Ok(ReleaseOutcome::NotFound),
    };
    let now = Utc::now();
    let _ = fence_if_expired(&mut state, &now);
    let owned_lease = state
        .lease
        .as_ref()
        .is_some_and(|l| l.matches_binding(binding));
    if owned_lease {
        state.lease = None;
    }
    let removed = remove_ticket(&mut state, binding);
    let next = head_ticket(&state).map(|t| t.ticket_id.clone());
    save_state(&state, dir)?;
    match removed {
        Some(t) => Ok(ReleaseOutcome::Released {
            ticket_id: t.ticket_id,
            next,
        }),
        None => Ok(ReleaseOutcome::NotFound),
    }
}

/// Execute one short, bounded publication mutation while holding the queue
/// lock after validating the exact live lease binding. This is the final
/// compare-and-fast-forward fence; callers must not perform model work or
/// unbounded validation in the closure.
pub fn with_lease_owner<T>(
    dir: &Path,
    integration_ref: &str,
    binding: &TicketBinding,
    publish: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state =
        load_state(dir, integration_ref)?.context("no landing-turn state for publication")?;
    let now = Utc::now();
    if fence_if_expired(&mut state, &now).is_some() {
        save_state(&state, dir)?;
        bail!("landing-turn lease expired before publication; old owner fenced");
    }
    let lease = state
        .lease
        .as_ref()
        .context("landing-turn publication requires an active lease")?;
    if !lease.matches_binding(binding) {
        bail!(
            "landing-turn publication refused: task/candidate/target/fence/source binding is not the current lease owner"
        );
    }
    publish()
}

/// Read-only: is this exact binding the current lease owner?
pub fn is_lease_owner(dir: &Path, integration_ref: &str, binding: &TicketBinding) -> Result<bool> {
    let _lock = QueueLock::acquire(dir, integration_ref)?;
    let mut state = match load_state(dir, integration_ref)? {
        Some(s) => s,
        None => return Ok(false),
    };
    let now = Utc::now();
    let fenced = fence_if_expired(&mut state, &now).is_some();
    let owner = state
        .lease
        .as_ref()
        .is_some_and(|lease| lease.matches_binding(binding));
    if fenced {
        save_state(&state, dir)?;
    }
    Ok(owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn binding(task: &str, agent: &str, seq: u64, target: &str) -> TicketBinding {
        TicketBinding {
            task_id: task.to_string(),
            generation: 1,
            attempt_id: Some(format!("attempt-1-{seq}")),
            fence: 1,
            candidate_sequence: seq,
            candidate_oid: format!("candidate-{seq}"),
            source_agent: agent.to_string(),
            source_session: Some("session-x".to_string()),
            integration_ref: "refs/heads/main".to_string(),
            observed_target_oid: target.to_string(),
        }
    }

    #[test]
    fn fifo_acquire_release_wakes_next() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let b = binding("t-b", "agent-b", 2, "oid-0");
        let c = binding("t-c", "agent-c", 3, "oid-0");

        // First requester acquires immediately.
        let r = request_turn(d, &a).unwrap();
        let a_ticket = match r {
            RequestOutcome::Acquired { ticket_id, .. } => ticket_id,
            other => panic!("expected Acquired, got {other:?}"),
        };

        // Second and third park in FIFO order.
        let park_b = request_turn(d, &b).unwrap();
        let (b_ticket, b_pos) = match park_b {
            RequestOutcome::Parked {
                ticket_id,
                position,
                ..
            } => (ticket_id, position),
            other => panic!("expected Parked, got {other:?}"),
        };
        assert_eq!(b_pos, 2);
        let park_c = request_turn(d, &c).unwrap();
        let (c_ticket, c_pos) = match park_c {
            RequestOutcome::Parked {
                ticket_id,
                position,
                ..
            } => (ticket_id, position),
            other => panic!("expected Parked, got {other:?}"),
        };
        assert_eq!(c_pos, 3);

        // Status reports the owner.
        let st = status(d, "refs/heads/main", Some("t-a")).unwrap();
        assert_eq!(st.lease.as_ref().unwrap().owner_agent, "agent-a");

        // Re-request by the owner is idempotent.
        match request_turn(d, &a).unwrap() {
            RequestOutcome::AlreadyOwner { ticket_id, .. } => assert_eq!(ticket_id, a_ticket),
            other => panic!("expected AlreadyOwner, got {other:?}"),
        }

        // A non-owner release is refused.
        match release_turn(d, "refs/heads/main", &b).unwrap() {
            ReleaseOutcome::NotOwner { .. } => {}
            other => panic!("expected NotOwner, got {other:?}"),
        }

        // Owner releases; the next ticket (b) should be woken.
        let rel = release_turn(d, "refs/heads/main", &a).unwrap();
        match rel {
            ReleaseOutcome::Released { next, .. } => {
                assert_eq!(next.as_deref(), Some(b_ticket.as_str()))
            }
            other => panic!("expected Released, got {other:?}"),
        }
        // b is now at the head but the lease is NOT auto-acquired (the source
        // agent must actively take its turn — see take_resumable_head / request).
        let st = status(d, "refs/heads/main", None).unwrap();
        assert!(st.lease.is_none());
        assert_eq!(st.queue_len, 2);

        // b re-requests and acquires (it is head, lease free).
        match request_turn(d, &b).unwrap() {
            RequestOutcome::Acquired { ticket_id, .. } => assert_eq!(ticket_id, b_ticket),
            other => panic!("expected Acquired for b, got {other:?}"),
        }
        release_turn(d, "refs/heads/main", &b).unwrap();
        match request_turn(d, &c).unwrap() {
            RequestOutcome::Acquired { ticket_id, .. } => assert_eq!(ticket_id, c_ticket),
            other => panic!("expected Acquired for c, got {other:?}"),
        }
        release_turn(d, "refs/heads/main", &c).unwrap();
        // Queue drained.
        let st = status(d, "refs/heads/main", None).unwrap();
        assert_eq!(st.queue_len, 0);
        assert!(st.lease.is_none());
    }

    #[test]
    fn parked_request_is_idempotent() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let b = binding("t-b", "agent-b", 2, "oid-0");
        request_turn(d, &a).unwrap();
        let first = request_turn(d, &b).unwrap();
        let second = request_turn(d, &b).unwrap();
        // Both must report the same ticket/seq (idempotent), not enqueue twice.
        let (s1, s2) = match (&first, &second) {
            (
                RequestOutcome::Parked {
                    ticket_id: t1,
                    seq: s1,
                    ..
                },
                RequestOutcome::Parked {
                    ticket_id: t2,
                    seq: s2,
                    ..
                },
            ) => {
                assert_eq!(t1, t2);
                (s1, s2)
            }
            other => panic!("expected Parked twice, got {other:?}"),
        };
        assert_eq!(s1, s2);
        let st = status(d, "refs/heads/main", None).unwrap();
        assert_eq!(st.queue_len, 2);
    }

    #[test]
    fn expiry_auto_fences_and_advances() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let b = binding("t-b", "agent-b", 2, "oid-0");
        request_turn(d, &a).unwrap();
        request_turn(d, &b).unwrap();

        // Force the lease into the past.
        let _lock = QueueLock::acquire(d, "refs/heads/main").unwrap();
        let mut state = load_state(d, "refs/heads/main").unwrap().unwrap();
        state.lease.as_mut().unwrap().expires_at =
            (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        save_state(&state, d).unwrap();
        drop(_lock);

        // The next request auto-fences a and lets b acquire.
        match request_turn(d, &b).unwrap() {
            RequestOutcome::Acquired { .. } => {}
            other => panic!("expected Acquired for b after fence, got {other:?}"),
        }
        // a's stale ticket is gone.
        let st = status(d, "refs/heads/main", Some("t-a")).unwrap();
        assert!(st.ticket.is_none());
    }

    #[test]
    fn reclaim_force_fences_live_owner() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let b = binding("t-b", "agent-b", 2, "oid-0");
        request_turn(d, &a).unwrap();
        request_turn(d, &b).unwrap();

        // Without force, a live lease is not reclaimed.
        match reclaim_turn(d, "refs/heads/main", "operator reclaim", false).unwrap() {
            ReclaimOutcome::NotExpired { .. } => {}
            other => panic!("expected NotExpired, got {other:?}"),
        }
        // With force, a is fenced and b becomes the woken next.
        match reclaim_turn(d, "refs/heads/main", "operator reclaim", true).unwrap() {
            ReclaimOutcome::Fenced {
                fenced_owner, next, ..
            } => {
                assert_eq!(fenced_owner, "agent-a");
                assert!(next.is_some());
            }
            other => panic!("expected Fenced, got {other:?}"),
        }
        // b can now acquire.
        match request_turn(d, &b).unwrap() {
            RequestOutcome::Acquired { .. } => {}
            other => panic!("expected Acquired for b after force-fence, got {other:?}"),
        }
    }

    #[test]
    fn renewal_requires_proven_progress() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let b = binding("t-b", "agent-b", 2, "oid-0");
        request_turn(d, &a).unwrap();
        request_turn(d, &b).unwrap();

        // Cap renewals without progress to a tight value for the test.
        let _lock = QueueLock::acquire(d, "refs/heads/main").unwrap();
        let mut state = load_state(d, "refs/heads/main").unwrap().unwrap();
        state.max_renewals_without_progress = 1;
        save_state(&state, d).unwrap();
        drop(_lock);

        // A renewal with a fresh progress token succeeds.
        match renew_turn(d, "refs/heads/main", &a, Some("step-1")).unwrap() {
            RenewOutcome::Renewed { renewals, .. } => assert_eq!(renewals, 1),
            other => panic!("expected Renewed, got {other:?}"),
        }
        // A renewal with the SAME token (no proven progress) is the first
        // no-progress renewal (cap=1, so the *next* no-progress renewal fences).
        match renew_turn(d, "refs/heads/main", &a, Some("step-1")).unwrap() {
            RenewOutcome::Renewed { .. } => {}
            other => panic!("expected Renewed (first no-progress within cap), got {other:?}"),
        }
        // The next no-progress renewal exceeds the cap and fences a.
        match renew_turn(d, "refs/heads/main", &a, Some("step-1")).unwrap() {
            RenewOutcome::StalledFenced { next, .. } => assert!(next.is_some()),
            other => panic!("expected StalledFenced, got {other:?}"),
        }
        // b can now acquire.
        match request_turn(d, &b).unwrap() {
            RequestOutcome::Acquired { .. } => {}
            other => panic!("expected Acquired for b after stall fence, got {other:?}"),
        }
    }

    #[test]
    fn cancel_drops_queued_ticket() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let b = binding("t-b", "agent-b", 2, "oid-0");
        let c = binding("t-c", "agent-c", 3, "oid-0");
        request_turn(d, &a).unwrap();
        request_turn(d, &b).unwrap();
        request_turn(d, &c).unwrap();
        // b cancels its own queued ticket.
        match cancel_turn(d, "refs/heads/main", &b).unwrap() {
            ReleaseOutcome::Released { .. } => {}
            other => panic!("expected Released, got {other:?}"),
        }
        let st = status(d, "refs/heads/main", None).unwrap();
        assert_eq!(st.queue_len, 2);
        // a still holds the lease.
        assert_eq!(st.lease.as_ref().unwrap().owner_agent, "agent-a");
    }

    #[test]
    fn exact_binding_required_to_release() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let impostor = TicketBinding {
            source_agent: "agent-impostor".to_string(),
            ..binding("t-a", "agent-impostor", 1, "oid-0")
        };
        request_turn(d, &a).unwrap();
        // A different source agent cannot release a's lease.
        match release_turn(d, "refs/heads/main", &impostor).unwrap() {
            ReleaseOutcome::NotOwner { .. } => {}
            other => panic!("expected NotOwner, got {other:?}"),
        }
        // a can.
        assert!(matches!(
            release_turn(d, "refs/heads/main", &a).unwrap(),
            ReleaseOutcome::Released { .. }
        ));
    }

    #[test]
    fn forged_or_stale_binding_cannot_renew_release_cancel_or_publish() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let owner = binding("t-a", "agent-a", 1, "oid-0");
        request_turn(d, &owner).unwrap();
        let mutations = [
            TicketBinding {
                source_agent: "forged-agent".into(),
                ..owner.clone()
            },
            TicketBinding {
                attempt_id: Some("stale-attempt".into()),
                ..owner.clone()
            },
            TicketBinding {
                fence: owner.fence + 1,
                ..owner.clone()
            },
            TicketBinding {
                candidate_oid: "changed-bytes".into(),
                ..owner.clone()
            },
        ];
        for forged in mutations {
            assert!(matches!(
                renew_turn(d, "refs/heads/main", &forged, Some("progress")).unwrap(),
                RenewOutcome::NotOwner
            ));
            assert!(matches!(
                release_turn(d, "refs/heads/main", &forged).unwrap(),
                ReleaseOutcome::NotOwner { .. }
            ));
            assert!(matches!(
                cancel_turn(d, "refs/heads/main", &forged).unwrap(),
                ReleaseOutcome::NotFound
            ));
            let mut called = false;
            assert!(
                with_lease_owner(d, "refs/heads/main", &forged, || {
                    called = true;
                    Ok(())
                })
                .is_err()
            );
            assert!(!called, "publication closure ran for forged authority");
        }
        assert!(is_lease_owner(d, "refs/heads/main", &owner).unwrap());
    }

    #[test]
    fn restart_recovery_resumes_head_only_when_lease_free() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let b = binding("t-b", "agent-b", 2, "oid-0");
        request_turn(d, &a).unwrap();
        request_turn(d, &b).unwrap();
        // a holds the lease: nothing resumable.
        assert!(take_resumable_head(d, "refs/heads/main").unwrap().is_none());
        // a durably parks its exact head ticket before restart.
        assert!(park_owned_turn(d, "refs/heads/main", &a).unwrap());
        // The same head (a), not a later waiter, is resumable exactly once.
        let head = take_resumable_head(d, "refs/heads/main").unwrap().unwrap();
        assert_eq!(head.task_id, "t-a");
        // Idempotent: a second take does not re-wake.
        assert!(take_resumable_head(d, "refs/heads/main").unwrap().is_none());
    }

    #[test]
    fn exact_session_continuation_reacquires_parked_head_after_resume() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let owner = binding("owner", "agent-owner", 1, "oid-0");
        let parked = binding("source", "agent-old", 2, "oid-0");
        request_turn(d, &owner).unwrap();
        let ticket_id = match request_turn(d, &parked).unwrap() {
            RequestOutcome::Parked { ticket_id, .. } => ticket_id,
            other => panic!("expected parked source, got {other:?}"),
        };
        release_turn(d, "refs/heads/main", &owner).unwrap();
        assert!(ticket_ready(d, "refs/heads/main", &ticket_id).unwrap());
        let resumed = TicketBinding {
            attempt_id: Some("attempt-1-9".to_string()),
            fence: 9,
            candidate_sequence: 9,
            candidate_oid: "candidate-resumed".to_string(),
            source_agent: "agent-replacement-process".to_string(),
            ..parked
        };
        assert!(matches!(
            request_turn(d, &resumed).unwrap(),
            RequestOutcome::Acquired { .. }
        ));
        assert!(is_lease_owner(d, "refs/heads/main", &resumed).unwrap());
    }

    #[test]
    fn independent_refs_get_independent_queues() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a_main = TicketBinding {
            integration_ref: "refs/heads/main".to_string(),
            ..binding("t-a", "agent-a", 1, "oid-0")
        };
        let a_alt = TicketBinding {
            integration_ref: "refs/heads/alt".to_string(),
            ..binding("t-a", "agent-a", 1, "oid-0")
        };
        // Both acquire independently — different queues, no contention.
        assert!(matches!(
            request_turn(d, &a_main).unwrap(),
            RequestOutcome::Acquired { .. }
        ));
        assert!(matches!(
            request_turn(d, &a_alt).unwrap(),
            RequestOutcome::Acquired { .. }
        ));
    }

    #[test]
    fn starvation_freedom_every_ticket_lands() {
        // With renewals bounded by proven progress, no lease blocks forever,
        // so every queued ticket eventually reaches the head.
        let dir = tempdir().unwrap();
        let d = dir.path();
        let owner = binding("t-owner", "agent-o", 1, "oid-0");
        request_turn(d, &owner).unwrap();
        // Queue 5 tickets.
        let queued: Vec<_> = (1..=5)
            .map(|i| {
                binding(
                    &format!("t-{i}"),
                    &format!("agent-{i}"),
                    (i + 1) as u64,
                    "oid-0",
                )
            })
            .collect();
        for q in &queued {
            request_turn(d, q).unwrap();
        }
        // The owner stalls (never makes progress). Force-fence it.
        reclaim_turn(d, "refs/heads/main", "owner stalled", true).unwrap();
        // Every queued ticket acquires in FIFO order.
        for q in &queued {
            assert!(
                matches!(request_turn(d, q).unwrap(), RequestOutcome::Acquired { .. }),
                "ticket for {} did not acquire",
                q.task_id
            );
            release_turn(d, "refs/heads/main", q).unwrap();
        }
        let st = status(d, "refs/heads/main", None).unwrap();
        assert_eq!(st.queue_len, 0);
    }

    #[test]
    fn retained_source_rebinds_new_candidate_and_fences_old_publication() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        let old = binding("t-a", "agent-a", 1, "oid-0");
        assert!(matches!(
            request_turn(d, &old).unwrap(),
            RequestOutcome::Acquired { .. }
        ));
        let renewed = TicketBinding {
            candidate_sequence: 2,
            candidate_oid: "candidate-2".to_string(),
            observed_target_oid: "oid-1".to_string(),
            ..old.clone()
        };
        assert!(matches!(
            request_turn(d, &renewed).unwrap(),
            RequestOutcome::AlreadyOwner { .. }
        ));
        let old_publish = with_lease_owner(d, "refs/heads/main", &old, || Ok(()));
        assert!(
            old_publish.is_err(),
            "old candidate/target/fence must be stale"
        );
        with_lease_owner(d, "refs/heads/main", &renewed, || Ok(())).unwrap();
    }

    #[test]
    fn changed_target_oid_does_not_steal_live_lease() {
        // A late request with a fresher observed target OID must not displace a
        // live owner; it parks behind it. (The source agent re-synchronizes on
        // its turn — the queue does not pre-judge target validity.)
        let dir = tempdir().unwrap();
        let d = dir.path();
        let a = binding("t-a", "agent-a", 1, "oid-0");
        let b_late = binding("t-b", "agent-b", 2, "oid-9");
        request_turn(d, &a).unwrap();
        match request_turn(d, &b_late).unwrap() {
            RequestOutcome::Parked { position, .. } => assert_eq!(position, 2),
            other => panic!("expected Parked, got {other:?}"),
        }
    }
}
