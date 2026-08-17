//! Durable non-terminal waits for completion/finalization blockers.
//!
//! These waits are deliberately separate from interactive `wg wait`: an
//! immutable completion candidate no longer needs a live model continuation.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::path::Path;
use worksgood::graph::{
    CompletionBlocker, CompletionBlockerKind, LogEntry, Status, Task,
};
use worksgood::lifecycle::{
    FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest, apply_transition,
};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::service::registry::{AgentRegistry, AgentStatus};

#[derive(Clone, Debug)]
pub(crate) struct LandingWait<'a> {
    pub integration_ref: &'a str,
    pub target_ref_oid: &'a str,
    pub worker_worktree: &'a Path,
}

pub(crate) fn park_needs_review(dir: &Path, id: &str, reason: &str) -> Result<()> {
    park(dir, id, CompletionBlockerKind::NeedsReview, reason, None)
}

pub(crate) fn park_landing_pending(
    dir: &Path,
    id: &str,
    reason: &str,
    wait: LandingWait<'_>,
) -> Result<()> {
    park(
        dir,
        id,
        CompletionBlockerKind::LandingPending,
        reason,
        Some(wait),
    )
}

fn park(
    dir: &Path,
    id: &str,
    kind: CompletionBlockerKind,
    reason: &str,
    landing: Option<LandingWait<'_>>,
) -> Result<()> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let expected = graph
        .get_task(id)
        .with_context(|| format!("task '{id}' not found"))?
        .clone();
    let candidate = expected
        .completion_candidate
        .clone()
        .context("completion blocker requires an immutable candidate")?;

    // Exact live continuation is useful only when it can be proven. A settled
    // or terminal Pi watchdog intentionally yields no selector: the immutable
    // candidate, not a model process, is the resumable authority.
    let session_selector = super::wait::attested_pi_session_id(dir, &expected)
        .ok()
        .flatten();
    let attempt_id = expected
        .lifecycle
        .current_attempt
        .as_ref()
        .map(|attempt| attempt.id.clone());
    let safe_next = match kind {
        CompletionBlockerKind::NeedsReview => format!(
            "inspect immutable review activity with `wg show {id}`; then use `wg done {id} --operator-accept --reason <WHY>` or retry a new candidate"
        ),
        CompletionBlockerKind::LandingPending => format!(
            "preserve user changes, clean the attached integration checkout, then run `wg resume {id} --only`; the coordinator also retries after observing a clean checkout"
        ),
    };
    let blocker = CompletionBlocker {
        kind,
        reason: reason.to_string(),
        safe_next,
        task_id: id.to_string(),
        generation: expected.lifecycle.generation,
        attempt_id,
        fence: expected.lifecycle.fence,
        candidate,
        integration_ref: landing.as_ref().map(|wait| wait.integration_ref.to_string()),
        target_ref_oid: landing.as_ref().map(|wait| wait.target_ref_oid.to_string()),
        worker_worktree: landing
            .as_ref()
            .map(|wait| wait.worker_worktree.to_string_lossy().into_owned()),
        session_selector: session_selector.clone(),
        created_at: Utc::now().to_rfc3339(),
    };

    let mut error = None;
    let mut released_agent = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            error = Some(anyhow::anyhow!("task disappeared while parking completion"));
            return false;
        };
        if task.status == Status::Waiting
            && task.completion_blocker.as_ref().is_some_and(|current| {
                same_binding(current, &blocker) && current.kind == blocker.kind
            })
        {
            return false;
        }
        if task.status != Status::InProgress
            || task.lifecycle.generation != blocker.generation
            || task.lifecycle.fence != blocker.fence
            || task
                .lifecycle
                .current_attempt
                .as_ref()
                .map(|attempt| attempt.id.as_str())
                != blocker.attempt_id.as_deref()
            || task.completion_candidate.as_ref() != Some(&blocker.candidate)
        {
            error = Some(anyhow::anyhow!(
                "completion blocker binding changed before durable wait"
            ));
            return false;
        }

        let actor_id = task
            .assigned
            .clone()
            .unwrap_or_else(worksgood::current_user);
        let actor = if task.assigned.is_some() {
            LifecycleActor::worker(actor_id)
        } else {
            LifecycleActor::operator(actor_id)
        };
        let request = TransitionRequest::new(
            TransitionKind::AttemptParked,
            actor,
            match kind {
                CompletionBlockerKind::NeedsReview => "completion_needs_review",
                CompletionBlockerKind::LandingPending => "completion_landing_pending",
            },
            format!(
                "completion-wait:{id}:{}:{}:{}:{}",
                blocker.generation,
                blocker.attempt_id.as_deref().unwrap_or("none"),
                blocker.fence,
                blocker.candidate.manifest.content_digest
            ),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(rejection) = apply_transition(task, request) {
            error = Some(anyhow::anyhow!(rejection));
            return false;
        }

        released_agent = task.assigned.take();
        task.completion_blocker = Some(blocker.clone());
        task.session_id = session_selector.clone();
        task.wait_condition = None;
        task.message_wait = None;
        task.failure_reason = None;
        task.failure_class = None;
        task.failure_signal = None;
        task.checkpoint = Some(format!("{:?}: {reason}", kind));
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("completion-finalizer".to_string()),
            user: Some(worksgood::current_user()),
            message: format!(
                "Completion waiting/{kind:?}: {reason}. Candidate and receipts preserved; source worker released. Next: {}",
                blocker.safe_next
            ),
        });
        true
    })?;
    if let Some(error) = error {
        return Err(error);
    }

    if let Some(agent_id) = released_agent
        && let Ok(mut registry) = AgentRegistry::load_locked(dir)
    {
        if let Some(agent) = registry.registry.get_agent_mut(&agent_id) {
            agent.status = AgentStatus::Parked;
            agent.completed_at = Some(Utc::now().to_rfc3339());
        }
        let _ = registry.save();
    }
    let _ = worksgood::disk_sentinel::release_owned_cache_leases(dir, id, None);
    super::notify_graph_changed(dir);
    Ok(())
}

pub(crate) fn validate_current(task: &Task, blocker: &CompletionBlocker) -> Result<()> {
    if task.id != blocker.task_id
        || task.lifecycle.generation != blocker.generation
        || task.lifecycle.fence != blocker.fence
        || task
            .lifecycle
            .current_attempt
            .as_ref()
            .map(|attempt| attempt.id.as_str())
            != blocker.attempt_id.as_deref()
        || task.completion_candidate.as_ref() != Some(&blocker.candidate)
        || task.completion_blocker.as_ref() != Some(blocker)
    {
        bail!(
            "pending completion binding is stale: task/generation/attempt/fence/candidate changed"
        );
    }
    Ok(())
}

fn same_binding(left: &CompletionBlocker, right: &CompletionBlocker) -> bool {
    left.task_id == right.task_id
        && left.generation == right.generation
        && left.attempt_id == right.attempt_id
        && left.fence == right.fence
        && left.candidate == right.candidate
        && left.integration_ref == right.integration_ref
        && left.target_ref_oid == right.target_ref_oid
        && left.worker_worktree == right.worker_worktree
}
