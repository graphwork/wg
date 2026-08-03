//! Crash-safe reopen fencing shared by retry/requeue/reset and the coordinator.
//!
//! An operator command records [`ReopenIntent`] under `graph.lock`.  That
//! transition fences late writes immediately but deliberately does not make the
//! task Open.  The coordinator (and the command's best-effort immediate pass)
//! consumes the intent only after the exact old process identity is gone.  A
//! restart at any boundary simply repeats this idempotent reconciliation.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use worksgood::attempt_runtime::AttemptRuntimeKey;
use worksgood::graph::Task;
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, ReopenIntent, TransitionKind, TransitionRejection,
    TransitionRequest, apply_transition,
};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::pi_watchdog::{PiWatchdog, ProcessIdentity};
use worksgood::service::{AgentRegistry, AgentStatus};

/// Record one durable reopen intent. A second caller for the exact same
/// operation/source coalesces; a different operation must wait rather than
/// silently changing retry semantics while ownership is draining.
pub fn request(
    task: &mut Task,
    operation: &str,
    discard_worktree: bool,
    preserve_session: bool,
    begin_source_attempt_reason: &str,
    actor: LifecycleActor,
    reason_code: &str,
) -> std::result::Result<(ReopenIntent, bool), TransitionRejection> {
    let intent = ReopenIntent::for_task(
        task,
        operation,
        discard_worktree,
        preserve_session,
        begin_source_attempt_reason,
    );
    if let Some(existing) = task.lifecycle.reopen_intent.as_ref() {
        if existing.operation == intent.operation
            && existing.source_generation == task.lifecycle.generation
            && existing.source_attempt_id
                == task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.id.clone())
            && existing.owner_id
                == task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.actor_id.clone())
                    .or_else(|| task.assigned.clone())
            && existing.process_epoch == task.lifecycle.pi_process_epoch
            && existing.process_identity_digest == task.lifecycle.pi_process_identity_digest
            && existing.discard_worktree == discard_worktree
            && existing.preserve_session == preserve_session
            && existing.begin_source_attempt_reason == begin_source_attempt_reason
        {
            return Ok((existing.clone(), false));
        }
        return Err(TransitionRejection {
            code: "reopen_already_pending".into(),
            message: format!(
                "{} already waits for exact prior-owner release",
                existing.operation
            ),
        });
    }
    let request = TransitionRequest::new(
        TransitionKind::ReopenRequested {
            intent: intent.clone(),
        },
        actor,
        reason_code,
        intent.id.clone(),
    )
    .expecting(FenceExpectation::current(task));
    apply_transition(task, request)?;
    Ok((intent, true))
}

fn current_boot_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|value| value.trim().to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn exact_process_is_live(process: &ProcessIdentity) -> bool {
    if !worksgood::service::is_process_alive(process.pid) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        current_boot_id().as_deref() == Some(process.boot_id.as_str())
            && worksgood::service::read_proc_start_ticks(process.pid) == Some(process.start_ticks)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Platforms without a kernel birth token must fail closed while the PID
        // exists; explicit operator reap remains available.
        true
    }
}

fn watchdog_process(dir: &Path, task_id: &str, intent: &ReopenIntent) -> Option<ProcessIdentity> {
    let attempt_id = intent.source_attempt_id.as_ref()?;
    let key = AttemptRuntimeKey::new(
        task_id,
        intent.source_generation,
        attempt_id.clone(),
        intent.source_fence,
        intent.source_fence,
    );
    let state_path =
        worksgood::attempt_runtime::resolve_component(dir, &key, "pi/state.json").ok()??;
    let watchdog = PiWatchdog::open(&state_path).ok()?;
    let state = watchdog.state();
    let source_matches = state.source.task_id == task_id
        && state.source.generation == intent.source_generation
        && state.source.attempt_id == *attempt_id
        && state.source.attempt_fence == intent.source_fence
        && state.source.worktree_lease_epoch == intent.source_fence
        && state.process_epoch == intent.process_epoch
        && (intent.process_identity_digest.is_empty()
            || state.process.digest() == intent.process_identity_digest);
    source_matches.then(|| state.process.clone())
}

fn registry_owner_live(dir: &Path, task_id: &str, intent: &ReopenIntent) -> bool {
    let Some(owner_id) = intent.owner_id.as_deref() else {
        return false;
    };
    let Ok(registry) = AgentRegistry::load(dir) else {
        // A corrupt/unreadable registry is not proof of release.
        return true;
    };
    let Some(owner) = registry.get_agent(owner_id) else {
        return false;
    };
    if owner.task_id != task_id {
        return true;
    }
    // Registry status is advisory. A wrapper marked Dead can still be in its
    // exit/post-processing path, so require kernel quiescence of that exact
    // recorded process before releasing its worktree lease.
    worksgood::service::is_process_alive(owner.pid)
        && owner
            .started_at
            .parse::<chrono::DateTime<Utc>>()
            .map(|started| {
                worksgood::service::verify_process_identity(owner.pid, started.timestamp())
            })
            .unwrap_or(true)
}

fn owner_is_live(dir: &Path, task_id: &str, intent: &ReopenIntent) -> bool {
    // The Pi child owns the live session while the registered wrapper owns the
    // attempt/worktree lease and performs exit bookkeeping. Both must be gone;
    // checking only the child recreates the race during wrapper post-processing.
    watchdog_process(dir, task_id, intent)
        .as_ref()
        .is_some_and(exact_process_is_live)
        || registry_owner_live(dir, task_id, intent)
}

fn request_graceful_exit(dir: &Path, task_id: &str, intent: &ReopenIntent) {
    let pid = watchdog_process(dir, task_id, intent)
        .filter(exact_process_is_live)
        .map(|process| process.pid)
        .or_else(|| {
            let owner = intent.owner_id.as_deref()?;
            let registry = AgentRegistry::load(dir).ok()?;
            let entry = registry.get_agent(owner)?;
            (entry.task_id == task_id && worksgood::service::is_process_alive(entry.pid))
                .then_some(entry.pid)
        });
    let Some(pid) = pid else { return };
    #[cfg(unix)]
    unsafe {
        // Non-blocking and non-escalating: the durable hold remains observable;
        // a stubborn process requires the explicit agent reap surface.
        libc::kill(pid as i32, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn mark_owner_reaped(dir: &Path, task_id: &str, intent: &ReopenIntent) -> Result<()> {
    let Some(owner_id) = intent.owner_id.as_deref() else {
        return Ok(());
    };
    let mut registry = AgentRegistry::load_locked(dir)?;
    if let Some(owner) = registry.get_agent_mut(owner_id)
        && owner.task_id == task_id
    {
        let exact_owner_live = worksgood::service::is_process_alive(owner.pid)
            && owner
                .started_at
                .parse::<chrono::DateTime<Utc>>()
                .map(|started| {
                    worksgood::service::verify_process_identity(owner.pid, started.timestamp())
                })
                .unwrap_or(true);
        if !exact_owner_live {
            owner.status = AgentStatus::Dead;
            if owner.completed_at.is_none() {
                owner.completed_at = Some(Utc::now().to_rfc3339());
            }
            registry.save_ref()?;
        }
    }
    Ok(())
}

fn discard_old_worktree(dir: &Path, task_id: &str) -> Result<()> {
    let Some(project_root) = dir.parent() else {
        return Ok(());
    };
    if let Some((path, _branch)) =
        super::spawn::worktree::find_worktree_for_task(project_root, task_id)
    {
        // The legacy reopen adapter has no exact WorkSave/cleanup-plan handle.
        // `--fresh` therefore cannot turn an age/path guess into authority to
        // erase retained bytes.  The WorkSave adapter/synthesis may replace
        // this hold only after it supplies the exact receipt to this call.
        anyhow::bail!(
            "fresh reopen retained worktree {}; capture an exact WorkSave and explicit discard receipt before retrying",
            path.display()
        );
    }
    Ok(())
}

/// Reconcile every pending reopen once. Returns the task IDs whose new
/// generation became runnable in this pass.
pub fn reconcile_pending(dir: &Path) -> Result<Vec<String>> {
    let graph_path = super::graph_path(dir);
    if !graph_path.exists() {
        return Ok(Vec::new());
    }
    let snapshot = load_graph(&graph_path)?;
    let pending: Vec<(String, ReopenIntent)> = snapshot
        .tasks()
        .filter_map(|task| {
            task.lifecycle
                .reopen_intent
                .clone()
                .map(|intent| (task.id.clone(), intent))
        })
        .collect();
    let mut released = Vec::new();

    for (task_id, intent) in pending {
        if owner_is_live(dir, &task_id, &intent) {
            request_graceful_exit(dir, &task_id, &intent);
            continue;
        }
        mark_owner_reaped(dir, &task_id, &intent)?;
        // Cache leases are rebuildable, unlike worktree/session evidence.
        let _ = worksgood::disk_sentinel::release_owned_cache_leases(
            dir,
            &task_id,
            intent.owner_id.as_deref(),
        );
        if intent.discard_worktree {
            discard_old_worktree(dir, &task_id)?;
        }

        let mut won = false;
        modify_graph(&graph_path, |graph| {
            let Some(task) = graph.get_task_mut(&task_id) else {
                return false;
            };
            if task.lifecycle.reopen_intent.as_ref().map(|value| &value.id) != Some(&intent.id) {
                return false;
            }
            let request = TransitionRequest::new(
                TransitionKind::ReopenOwnerReleased {
                    intent_id: intent.id.clone(),
                    exact_owner_reaped: true,
                },
                LifecycleActor {
                    kind: ActorKind::Reconciler,
                    id: "reopen-owner-reaper".into(),
                },
                "prior_owner_quiescent",
                format!("reopen-release:{}", intent.id),
            )
            .expecting(FenceExpectation::current(task));
            if apply_transition(task, request).is_err() {
                return false;
            }
            task.assigned = None;
            task.started_at = None;
            task.completed_at = None;
            if !intent.preserve_session {
                task.session_id = None;
                task.checkpoint = None;
            }
            if !intent.begin_source_attempt_reason.is_empty() {
                worksgood::eval_lifecycle::begin_source_attempt(
                    graph,
                    &task_id,
                    &intent.begin_source_attempt_reason,
                );
            }
            won = true;
            true
        })
        .with_context(|| format!("failed to release reopen hold for '{task_id}'"))?;
        if won {
            released.push(task_id);
        }
    }
    Ok(released)
}

/// Human-readable stable hold label shared by terminal/TUI surfaces.
pub fn hold_label(intent: &ReopenIntent) -> String {
    format!(
        "waiting-for-owner-release: {} source generation={} attempt={} fence={} owner={}",
        intent.operation,
        intent.source_generation,
        intent.source_attempt_id.as_deref().unwrap_or("none"),
        intent.source_fence,
        intent.owner_id.as_deref().unwrap_or("none")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use worksgood::graph::Status;

    #[test]
    fn identical_reopen_requests_coalesce_but_semantic_changes_do_not() {
        let mut task = Task {
            id: "coalesce".into(),
            status: Status::Open,
            ..Task::default()
        };
        let reserve = TransitionRequest::new(
            TransitionKind::AttemptReserved {
                owner_id: Some("agent-old".into()),
            },
            LifecycleActor::operator("test"),
            "reserve",
            "reserve-once",
        );
        apply_transition(&mut task, reserve).unwrap();

        let actor = LifecycleActor::operator("test");
        let (_, created) = request(
            &mut task,
            "retry",
            false,
            false,
            "retry source",
            actor.clone(),
            "retry",
        )
        .unwrap();
        assert!(created);
        let held_revision = task.lifecycle.revision;

        let (_, created_again) = request(
            &mut task,
            "retry",
            false,
            false,
            "retry source",
            actor.clone(),
            "retry",
        )
        .unwrap();
        assert!(!created_again);
        assert_eq!(task.lifecycle.revision, held_revision);

        let changed = request(
            &mut task,
            "retry",
            true,
            false,
            "fresh retry source",
            actor,
            "retry",
        )
        .unwrap_err();
        assert_eq!(changed.code, "reopen_already_pending");
        assert_eq!(task.lifecycle.revision, held_revision);
    }
}
