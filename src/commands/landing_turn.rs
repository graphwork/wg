//! `wg landing-turn` — the worker-owned landing-turn protocol CLI.
//!
//! A worker requests its landing turn only when its candidate is ready. If
//! another landing owns the lease, it atomically parks through the existing
//! `AttemptParked` / `Waiting` machinery with a typed `landing-turn` wait
//! condition and checkpoint, releasing worker/build capacity while retaining
//! the exact worktree, candidate, Pi session continuation, and queue ticket.
//! When the ticket reaches the head and the lease is free, the daemon
//! auto-resumes that same task; the resumed source agent synchronizes with
//! the current target, resolves conflicts in its own branch, reruns
//! target-dependent validation, and submits a new candidate/receipt binding
//! when bytes changed.
//!
//! The persisted lease is the authority (see [`worksgood::landing_turn`]); an
//! OS lock protects only short atomic queue/lease mutations. Operators get
//! `status` / `reclaim`; normal operation is automatic.

use anyhow::{Context, Result, bail};
use std::path::Path;
use worksgood::graph::{Status, Task, WaitCondition, WaitSpec};
use worksgood::landing_turn::{
    self, ReclaimOutcome, ReleaseOutcome, RequestOutcome, TicketBinding,
};
use worksgood::lifecycle::{
    FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest, apply_transition,
};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::service::registry::{AgentRegistry, AgentStatus};

use super::wait::attested_pi_session_id;

/// The coherent command family.
#[derive(Debug, clap::Subcommand)]
pub enum LandingTurnCommand {
    /// Request a landing turn for a ready candidate. Acquires the lease if free
    /// and this ticket is at the head; otherwise parks the task through
    /// `AttemptParked` with a typed `landing-turn` wait condition.
    Request {
        #[arg(value_name = "TASK")]
        id: String,
        /// Integration ref the candidate lands against (e.g. `refs/heads/main`).
        #[arg(long, value_name = "REF")]
        integration_ref: String,
        /// Target OID observed by the source agent when it requested the turn.
        #[arg(long, value_name = "OID")]
        observed_target: Option<String>,
        /// Checkpoint summary persisted with the park (progress so far).
        #[arg(long)]
        checkpoint: Option<String>,
    },
    /// Show the landing-turn queue and lease for an integration ref.
    Status {
        #[arg(value_name = "REF")]
        integration_ref: String,
        /// Scope the status to a single task.
        #[arg(long, value_name = "TASK")]
        task: Option<String>,
    },
    /// Renew the current lease. Renewal is bounded to proven progress: pass a
    /// `--progress` token that differs from the last renewal.
    Renew {
        #[arg(value_name = "TASK")]
        id: String,
        #[arg(long, value_name = "REF")]
        integration_ref: String,
        /// Opaque progress token. A token equal to the last renewal does not
        /// count as proven progress and may stall-fence the lease.
        #[arg(long)]
        progress: Option<String>,
    },
    /// Release the lease after a successful landing (or on giving up). Only the
    /// current lease owner with an exact binding may release.
    Release {
        #[arg(value_name = "TASK")]
        id: String,
        #[arg(long, value_name = "REF")]
        integration_ref: String,
    },
    /// Operator: force-fence the current lease (e.g. a dead/unresponsive owner)
    /// and advance to the next ticket. Auto-fences expired leases without force.
    Reclaim {
        #[arg(value_name = "REF")]
        integration_ref: String,
        /// Force-fence even a live, unexpired lease.
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "operator reclaim")]
        reason: String,
    },
    /// Cancel this task's queued ticket (the source agent gave up). If it held
    /// the lease, the lease is fenced and the next ticket is woken.
    Cancel {
        #[arg(value_name = "TASK")]
        id: String,
        #[arg(long, value_name = "REF")]
        integration_ref: String,
    },
}

/// Bind the request to the managed task/capability rather than trusting an
/// arbitrary task id. The calling agent must be the one assigned to the task,
/// and the task must be live (`InProgress`) so the park transition is legal.
fn require_exact_worker_authority(dir: &Path, id: &str, require_in_progress: bool) -> Result<Task> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?
        .clone();
    if require_in_progress && task.status != Status::InProgress {
        bail!(
            "task '{id}' is not in-progress (status: {}); only a live source agent may request or renew a landing turn",
            task.status
        );
    }
    let env_task = std::env::var("WG_TASK_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .context(
            "landing-turn worker mutation requires WG_TASK_ID or an opaque worker capability",
        )?;
    let env_agent = std::env::var("WG_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .context(
            "landing-turn worker mutation requires WG_AGENT_ID or an opaque worker capability",
        )?;
    if env_task != id {
        bail!("landing turn requires the exact managed WG_TASK_ID capability binding");
    }
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("landing turn requires the exact current source attempt")?;
    if attempt.generation != task.lifecycle.generation
        || attempt.fence != task.lifecycle.fence
        || attempt.actor_id != env_agent
    {
        bail!("landing-turn caller does not own the exact current generation/attempt/fence");
    }
    if let Some(assigned) = task.assigned.as_deref()
        && assigned != env_agent
    {
        bail!(
            "task '{id}' is assigned to '{assigned}', not the calling agent '{env_agent}'; only the exact source agent may mutate its landing turn"
        );
    }
    Ok(task)
}

fn require_live_owned_task(dir: &Path, id: &str) -> Result<Task> {
    require_exact_worker_authority(dir, id, true)
}

pub(crate) fn binding_from_task(
    dir: &Path,
    task: &Task,
    integration_ref: &str,
    observed_target: Option<&str>,
) -> Result<TicketBinding> {
    let bound_task = std::env::var("WG_TASK_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .context("no WG_TASK_ID worker authority is present")?;
    if bound_task != task.id {
        bail!(
            "landing-turn caller is bound to task '{bound_task}', not '{}'",
            task.id
        );
    }
    let source_agent = std::env::var("WG_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .context("no source agent capability (WG_AGENT_ID) is present")?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("landing turn requires a current source attempt")?;
    if attempt.actor_id != source_agent
        || attempt.generation != task.lifecycle.generation
        || attempt.fence != task.lifecycle.fence
    {
        bail!("source agent does not own the exact current attempt/fence");
    }
    let source_session = std::env::var("PI_SESSION_ID")
        .ok()
        .filter(|session| !session.trim().is_empty())
        .or_else(|| task.session_id.clone());
    binding_from_authority(
        dir,
        task,
        integration_ref,
        observed_target,
        source_agent,
        source_session,
    )
}

pub(crate) fn binding_from_authority(
    dir: &Path,
    task: &Task,
    integration_ref: &str,
    observed_target: Option<&str>,
    source_agent: String,
    source_session: Option<String>,
) -> Result<TicketBinding> {
    let candidate = task
        .completion_candidate
        .as_ref()
        .context("landing turn requires a ready completion candidate (run `wg submit` first)")?;
    let review_binding = candidate
        .review_binding
        .as_ref()
        .context("landing candidate has no exact source/review binding")?;
    let current_attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("landing turn requires a current source attempt")?;
    if review_binding.task_id != task.id
        || review_binding.generation != task.lifecycle.generation
        || review_binding.attempt_id.as_deref() != Some(current_attempt.id.as_str())
        || review_binding.attempt_fence != task.lifecycle.fence
    {
        bail!(
            "landing candidate binding is stale for task/generation/attempt/fence; rerun validation and submit a new candidate"
        );
    }
    let candidate_sequence = review_binding.candidate_sequence;
    // The immutable manifest CID identifies the exact reviewed candidate bytes;
    // the manifest in turn pins the Git commit OID used at publication.
    let candidate_oid = candidate.manifest.content_digest.to_string();
    let attempt_id = Some(current_attempt.id.clone());
    let observed_target_oid = if let Some(target) = observed_target {
        target.to_string()
    } else {
        let project_root = dir
            .parent()
            .context("workgraph directory has no project root")?;
        let output = std::process::Command::new("git")
            .args(["rev-parse", integration_ref])
            .current_dir(project_root)
            .output()?;
        if !output.status.success() {
            bail!(
                "resolve integration ref {}: {}",
                integration_ref,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        String::from_utf8(output.stdout)?.trim().to_string()
    };
    Ok(TicketBinding {
        task_id: task.id.clone(),
        generation: task.lifecycle.generation,
        attempt_id,
        fence: task.lifecycle.fence,
        candidate_sequence,
        candidate_oid,
        source_agent,
        source_session,
        integration_ref: integration_ref.to_string(),
        observed_target_oid,
    })
}

/// Park the task through `AttemptParked` with a typed `landing-turn` wait
/// condition. Mirrors `wg wait` but with the pre-built condition, so the
/// coordinator can auto-resume the exact task when its ticket reaches the head.
fn binding_for_existing_ticket(
    dir: &Path,
    task: &Task,
    integration_ref: &str,
) -> Result<TicketBinding> {
    let status = landing_turn::status(dir, integration_ref, Some(&task.id))?;
    let target = status
        .lease
        .as_ref()
        .filter(|lease| lease.task_id == task.id)
        .map(|lease| lease.target_oid.as_str())
        .or_else(|| {
            status
                .ticket
                .as_ref()
                .map(|ticket| ticket.observed_target_oid.as_str())
        })
        .context("task has no exact landing ticket/lease binding")?;
    binding_from_task(dir, task, integration_ref, Some(target))
}

pub(crate) fn park_landing_turn(
    dir: &Path,
    id: &str,
    integration_ref: &str,
    ticket_id: &str,
    checkpoint: Option<&str>,
    source_session: Option<&str>,
) -> Result<()> {
    let path = super::graph_path(dir);
    let mut error: Option<anyhow::Error> = None;
    let mut assigned_agent: Option<String> = None;
    let mut session_selector: Option<String> = None;
    modify_graph(&path, |graph| {
        let task = match graph.get_task(id) {
            Some(t) => t.clone(),
            None => {
                error = Some(anyhow::anyhow!("Task '{}' not found", id));
                return false;
            }
        };
        if task.status != Status::InProgress {
            error = Some(anyhow::anyhow!(
                "Cannot park landing-turn on '{}': status is '{}', expected 'in-progress'",
                id,
                task.status
            ));
            return false;
        }
        match attested_pi_session_id(dir, &task) {
            Ok(attested) => {
                if let (Some(claimed), Some(actual)) = (source_session, attested.as_deref())
                    && claimed != actual
                {
                    error = Some(anyhow::anyhow!(
                        "landing source session does not match the attested Pi continuation"
                    ));
                    return false;
                }
                session_selector = attested.or_else(|| source_session.map(str::to_string));
            },
            Err(e) => {
                error = Some(e);
                return false;
            }
        }
        let wait_spec = WaitSpec::All(vec![WaitCondition::LandingTurn {
            integration_ref: integration_ref.to_string(),
            ticket_id: ticket_id.to_string(),
        }]);
        let actor_id = std::env::var("WG_AGENT_ID")
            .ok()
            .or_else(|| task.assigned.clone())
            .unwrap_or_else(worksgood::current_user);
        let generation = task.lifecycle.generation;
        let request = TransitionRequest::new(
            TransitionKind::AttemptParked,
            LifecycleActor::worker(actor_id),
            "landing_turn_wait",
            format!("landing-turn:{id}:{generation}:{integration_ref}"),
        )
        .expecting(FenceExpectation::current(&task));
        let task = graph.get_task_mut(id).expect("task verified above");
        if let Err(rejection) = apply_transition(task, request) {
            error = Some(anyhow::anyhow!(rejection));
            return false;
        }
        if let Some(s) = &session_selector {
            task.session_id = Some(s.clone());
        }
        task.wait_condition = Some(wait_spec);
        if let Some(cp) = checkpoint {
            task.checkpoint = Some(cp.to_string());
        }
        task.log.push(worksgood::graph::LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            actor: task.assigned.clone(),
            user: Some(worksgood::current_user()),
            message: format!(
                "Landing turn parked: queued for integration ref {integration_ref}. The daemon resumes this task when its ticket reaches the head and the lease is free."
            ),
        });
        assigned_agent = task.assigned.clone();
        true
    })
    .context("Failed to modify graph while parking landing turn")?;
    if let Some(e) = error {
        return Err(e);
    }
    if let Some(ref assigned) = assigned_agent
        && let Ok(mut registry) = AgentRegistry::load_locked(dir)
    {
        if let Some(agent) = registry.registry.get_agent_mut(assigned) {
            agent.status = AgentStatus::Parked;
            agent.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
        let _ = registry.save();
    }
    let lease_owner = worksgood::disk_sentinel::caller_agent_for_task(id);
    let _ = worksgood::disk_sentinel::release_owned_cache_leases(dir, id, lease_owner.as_deref());
    super::notify_graph_changed(dir);
    let _ = session_selector;
    Ok(())
}

pub fn run(dir: &Path, command: LandingTurnCommand) -> Result<()> {
    match command {
        LandingTurnCommand::Request {
            id,
            integration_ref,
            observed_target,
            checkpoint,
        } => {
            let task = require_live_owned_task(dir, &id)?;
            let binding =
                binding_from_task(dir, &task, &integration_ref, observed_target.as_deref())?;
            let outcome = landing_turn::request_turn(dir, &binding)?;
            match outcome {
                RequestOutcome::Acquired {
                    ticket_id,
                    seq,
                    expires_at,
                } => {
                    println!(
                        "Landing turn acquired for task '{id}' (ticket {ticket_id}, seq {seq}); expires at {expires_at}. Proceed to integrate against {integration_ref}, then run `wg landing-turn release {id} --integration-ref {integration_ref}` on success."
                    );
                }
                RequestOutcome::AlreadyOwner {
                    ticket_id,
                    seq,
                    expires_at,
                } => {
                    println!(
                        "Landing turn already held by task '{id}' (ticket {ticket_id}, seq {seq}); expires at {expires_at}. Proceed with integration."
                    );
                }
                RequestOutcome::Parked {
                    ticket_id,
                    seq,
                    position,
                    owner,
                    owner_expires_at,
                } => {
                    park_landing_turn(
                        dir,
                        &id,
                        &integration_ref,
                        &ticket_id,
                        checkpoint.as_deref(),
                        binding.source_session.as_deref(),
                    )?;
                    println!(
                        "Landing turn parked for task '{id}' (ticket {ticket_id}, seq {seq}, queue position {position})."
                    );
                    if let Some(owner) = owner {
                        println!("Current lease owner: {owner}");
                    }
                    if let Some(exp) = owner_expires_at {
                        println!("Current lease expires at: {exp}");
                    }
                    println!(
                        "Worker released; candidate, worktree, and queue ticket retained. The daemon resumes this task automatically when its turn arrives."
                    );
                }
            }
        }
        LandingTurnCommand::Status {
            integration_ref,
            task,
        } => {
            let st = landing_turn::status(dir, &integration_ref, task.as_deref())?;
            if cli_json_enabled() {
                println!("{}", serde_json::to_string_pretty(&st)?);
            } else {
                println!("Integration ref: {}", st.integration_ref);
                println!("Queue length: {}", st.queue_len);
                if let Some(pos) = st.position {
                    println!("Position: {pos}");
                }
                if let Some(t) = &st.ticket {
                    println!(
                        "Ticket: {} (task {}, seq {}, agent {}, candidate_seq {})",
                        t.ticket_id, t.task_id, t.seq, t.source_agent, t.candidate_sequence
                    );
                }
                if let Some(l) = &st.lease {
                    println!(
                        "Lease: owner {} (task {}, ticket {}, seq {}); acquired {} expires {}{}",
                        l.owner_agent,
                        l.task_id,
                        l.ticket_id,
                        l.seq,
                        l.acquired_at,
                        l.expires_at,
                        if st.expired { " [EXPIRED]" } else { "" }
                    );
                    println!(
                        "  renewals: {} (without progress: {})",
                        l.renewals, l.renewals_without_progress
                    );
                } else {
                    println!("Lease: (free)");
                }
                if st.expired {
                    println!(
                        "Note: lease is expired and will be auto-fenced on the next mutation."
                    );
                }
            }
        }
        LandingTurnCommand::Renew {
            id,
            integration_ref,
            progress,
        } => {
            let task = require_live_owned_task(dir, &id)?;
            let binding = binding_for_existing_ticket(dir, &task, &integration_ref)?;
            match landing_turn::renew_turn(dir, &integration_ref, &binding, progress.as_deref())? {
                worksgood::landing_turn::RenewOutcome::Renewed {
                    ticket_id,
                    expires_at,
                    renewals,
                } => {
                    println!(
                        "Landing turn renewed for task '{id}' (ticket {ticket_id}); expires at {expires_at}; renewals={renewals}."
                    );
                }
                worksgood::landing_turn::RenewOutcome::NotOwner => {
                    bail!(
                        "task '{id}' does not currently hold the landing-turn lease for {integration_ref}"
                    );
                }
                worksgood::landing_turn::RenewOutcome::Expired => {
                    bail!(
                        "landing-turn lease for {integration_ref} expired before renewal; the task was fenced"
                    );
                }
                worksgood::landing_turn::RenewOutcome::StalledFenced { ticket_id, next } => {
                    println!(
                        "Landing turn for task '{id}' (ticket {ticket_id}) stall-fenced: no proven progress for too many renewals. Next ticket: {next:?}."
                    );
                }
            }
        }
        LandingTurnCommand::Release {
            id,
            integration_ref,
        } => {
            let task = require_exact_worker_authority(dir, &id, false)?;
            let binding = binding_for_existing_ticket(dir, &task, &integration_ref)?;
            match landing_turn::release_turn(dir, &integration_ref, &binding)? {
                ReleaseOutcome::Released { ticket_id, next } => {
                    println!(
                        "Landing turn released for task '{id}' (ticket {ticket_id}). Next ticket to wake: {next:?}."
                    );
                }
                ReleaseOutcome::NotOwner { owner, .. } => {
                    bail!(
                        "task '{id}' is not the current landing-turn lease owner for {integration_ref} (owner: {owner:?})"
                    );
                }
                ReleaseOutcome::NotFound => {
                    println!(
                        "No landing-turn state found for {integration_ref}; nothing to release."
                    );
                }
            }
        }
        LandingTurnCommand::Reclaim {
            integration_ref,
            force,
            reason,
        } => match landing_turn::reclaim_turn(dir, &integration_ref, &reason, force)? {
            ReclaimOutcome::Fenced {
                ticket_id,
                fenced_owner,
                reason,
                next,
            } => {
                println!(
                    "Landing turn reclaimed for {integration_ref}: fenced ticket {ticket_id} (owner {fenced_owner}); reason: {reason}. Next ticket to wake: {next:?}."
                );
            }
            ReclaimOutcome::NoLease => {
                println!("No active landing-turn lease for {integration_ref}.");
            }
            ReclaimOutcome::NotExpired {
                ticket_id,
                owner,
                expires_at,
            } => {
                bail!(
                    "landing-turn lease for {integration_ref} (ticket {ticket_id}, owner {owner}) is not expired (expires {expires_at}); pass --force to fence a live owner"
                );
            }
        },
        LandingTurnCommand::Cancel {
            id,
            integration_ref,
        } => {
            let task = require_exact_worker_authority(dir, &id, false)?;
            let binding = binding_for_existing_ticket(dir, &task, &integration_ref)?;
            match landing_turn::cancel_turn(dir, &integration_ref, &binding)? {
                ReleaseOutcome::Released { ticket_id, next } => {
                    println!(
                        "Landing-turn ticket cancelled for task '{id}' (ticket {ticket_id}). Next ticket to wake: {next:?}."
                    );
                }
                ReleaseOutcome::NotOwner { .. } => {
                    bail!("task '{id}' has no matching landing-turn ticket to cancel");
                }
                ReleaseOutcome::NotFound => {
                    println!(
                        "No landing-turn state found for {integration_ref}; nothing to cancel."
                    );
                }
            }
        }
    }
    Ok(())
}

fn cli_json_enabled() -> bool {
    std::env::var("WG_JSON").as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::completion_manifest::{
        CompletionArtifactStore, CompletionManifestRef, ImmutableLocator,
    };
    use worksgood::completion_review::CompletionReviewBinding;
    use worksgood::completion_task::CompletionCandidateRefs;
    use worksgood::graph::{Node, WorkGraph};
    use worksgood::lifecycle::{AttemptDisposition, AttemptRef};
    use worksgood::parser::save_graph;

    fn candidate(store: &CompletionArtifactStore, seq: u64) -> CompletionCandidateRefs {
        let requirements = store
            .put_bytes(b"requirements", "application/json")
            .unwrap();
        let worker_summary = store.put_bytes(b"summary", "text/plain").unwrap();
        let manifest_object = store.put_bytes(b"manifest", "application/json").unwrap();
        CompletionCandidateRefs {
            manifest: CompletionManifestRef {
                content_digest: manifest_object.content_digest.clone(),
                immutable_locator: ImmutableLocator::CompletionObject {
                    digest: manifest_object.content_digest,
                },
                size: manifest_object.size,
            },
            requirements,
            worker_summary,
            dependency_outputs: Vec::new(),
            review_binding: Some(CompletionReviewBinding {
                task_id: "t-a".to_string(),
                generation: 1,
                attempt_id: Some("attempt-1-1".to_string()),
                attempt_fence: 1,
                candidate_sequence: seq,
            }),
            flip_receipt: None,
            eval_receipt: None,
        }
    }

    fn setup_task(dir: &std::path::Path, task_id: &str, agent: &str) {
        let store_dir = dir.join("store");
        std::fs::create_dir_all(&store_dir).unwrap();
        let store = CompletionArtifactStore::open(&store_dir).unwrap();
        let candidate = candidate(&store, 1);
        let mut task = worksgood::graph::Task {
            id: task_id.to_string(),
            title: "Landing turn".to_string(),
            status: Status::InProgress,
            assigned: Some(agent.to_string()),
            completion_candidate: Some(candidate),
            ..worksgood::graph::Task::default()
        };
        task.lifecycle.generation = 1;
        task.lifecycle.fence = 1;
        task.lifecycle.current_attempt = Some(AttemptRef {
            id: "attempt-1-1".to_string(),
            generation: 1,
            fence: 1,
            actor_id: agent.to_string(),
            disposition: Some(AttemptDisposition::Succeeded),
        });
        let graph_path = dir.join("graph.jsonl");
        let mut graph = if graph_path.exists() {
            load_graph(&graph_path).unwrap()
        } else {
            WorkGraph::new()
        };
        graph.add_node(Node::Task(task));
        save_graph(&graph, graph_path).unwrap();
    }

    #[test]
    #[serial_test::serial(landing_turn_env)]
    fn request_acquires_when_uncontended() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        setup_task(d, "t-a", "agent-a");
        unsafe {
            std::env::set_var("WG_TASK_ID", "t-a");
            std::env::set_var("WG_AGENT_ID", "agent-a");
        }
        let task = load_graph(d.join("graph.jsonl"))
            .unwrap()
            .get_task("t-a")
            .unwrap()
            .clone();
        let binding = binding_from_task(d, &task, "refs/heads/main", Some("oid-0")).unwrap();
        match landing_turn::request_turn(d, &binding).unwrap() {
            RequestOutcome::Acquired { .. } => {}
            other => panic!("expected Acquired, got {other:?}"),
        }
        unsafe {
            std::env::remove_var("WG_AGENT_ID");
            std::env::remove_var("WG_TASK_ID");
        }
    }

    #[test]
    #[serial_test::serial(landing_turn_env)]
    fn request_parks_when_contended() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        setup_task(d, "t-a", "agent-a");
        unsafe {
            std::env::set_var("WG_TASK_ID", "t-a");
            std::env::set_var("WG_AGENT_ID", "agent-a");
        }
        let task_a = load_graph(d.join("graph.jsonl"))
            .unwrap()
            .get_task("t-a")
            .unwrap()
            .clone();
        let binding_a = binding_from_task(d, &task_a, "refs/heads/main", Some("oid-0")).unwrap();
        landing_turn::request_turn(d, &binding_a).unwrap();

        setup_task(d, "t-b", "agent-b");
        unsafe {
            std::env::set_var("WG_TASK_ID", "t-b");
            std::env::set_var("WG_AGENT_ID", "agent-b");
        }
        let task_b = load_graph(d.join("graph.jsonl"))
            .unwrap()
            .get_task("t-b")
            .unwrap()
            .clone();
        let binding_b = TicketBinding {
            task_id: "t-b".to_string(),
            generation: 1,
            attempt_id: Some("attempt-1-1".to_string()),
            fence: 1,
            candidate_sequence: 2,
            candidate_oid: "candidate-b".to_string(),
            source_agent: "agent-b".to_string(),
            source_session: None,
            integration_ref: "refs/heads/main".to_string(),
            observed_target_oid: "oid-0".to_string(),
        };
        match landing_turn::request_turn(d, &binding_b).unwrap() {
            RequestOutcome::Parked { position, .. } => assert_eq!(position, 2),
            other => panic!("expected Parked, got {other:?}"),
        }
        unsafe {
            std::env::remove_var("WG_AGENT_ID");
            std::env::remove_var("WG_TASK_ID");
        }
    }

    #[test]
    #[serial_test::serial(landing_turn_env)]
    fn arbitrary_agent_cannot_request() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        setup_task(d, "t-a", "agent-a");
        unsafe {
            std::env::remove_var("WG_AGENT_ID");
            std::env::remove_var("WG_TASK_ID");
        }
        let err = require_live_owned_task(d, "t-a").err();
        assert!(err.is_some(), "arbitrary caller should be refused");
    }

    #[test]
    #[serial_test::serial(landing_turn_env)]
    fn binding_binds_candidate_sequence() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        setup_task(d, "t-a", "agent-a");
        unsafe {
            std::env::set_var("WG_TASK_ID", "t-a");
            std::env::set_var("WG_AGENT_ID", "agent-a");
        }
        let task = load_graph(d.join("graph.jsonl"))
            .unwrap()
            .get_task("t-a")
            .unwrap()
            .clone();
        let binding = binding_from_task(d, &task, "refs/heads/main", Some("oid-0")).unwrap();
        assert_eq!(binding.candidate_sequence, 1);
        unsafe {
            std::env::remove_var("WG_AGENT_ID");
            std::env::remove_var("WG_TASK_ID");
        }
    }

    #[test]
    #[serial_test::serial(landing_turn_env)]
    fn cli_status_reports_queue_and_lease() {
        let dir = tempdir().unwrap();
        let d = dir.path();
        setup_task(d, "t-a", "agent-a");
        unsafe {
            std::env::set_var("WG_TASK_ID", "t-a");
            std::env::set_var("WG_AGENT_ID", "agent-a");
        }
        let task = load_graph(d.join("graph.jsonl"))
            .unwrap()
            .get_task("t-a")
            .unwrap()
            .clone();
        let binding = binding_from_task(d, &task, "refs/heads/main", Some("oid-0")).unwrap();
        landing_turn::request_turn(d, &binding).unwrap();
        let st = landing_turn::status(d, "refs/heads/main", Some("t-a")).unwrap();
        assert_eq!(st.queue_len, 1);
        assert_eq!(st.lease.as_ref().unwrap().owner_agent, "agent-a");
        assert_eq!(st.position, Some(1));
        unsafe {
            std::env::remove_var("WG_AGENT_ID");
            std::env::remove_var("WG_TASK_ID");
        }
    }
}
