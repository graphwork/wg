//! Restart-idempotent in-flight reconciliation.
//!
//! ## Why this exists
//!
//! Restarting the daemon (`wg service start --force`) is routine — it happens
//! on every code landing/deploy. Before this pass, the dispatcher's in-flight
//! accounting did not survive the restart, so the first tick after a restart
//! re-emitted dispatch for work that was already claimed. That produced three
//! observable symptoms (all reproduced 2026-10-08):
//!
//! 1. **Worktree-protection retry loops** — a second attempt was minted for a
//!    task whose live attempt still owned the worktree, so the guard bailed
//!    with *"protected by authenticated live attempt …"* until the stale entry
//!    aged out.
//! 2. **Duplicate task ids** — `fix-console-plugin-2…-5`, `fleet-hint-line-trim`
//!    (siblings of an existing task, minted during daemon-down churn).
//! 3. **Duplicate claims** — a redundant dispatch raced a live claim and failed
//!    with *"Task already claimed by @agent-…"*.
//!
//! ## What this does
//!
//! This module is the **single place** a restart makes persisted in-flight
//! state authoritative again. Before the daemon's first dispatch tick it:
//!
//! * **LOADs** every persisted claim from the agent registry,
//! * classifies each by **process identity** (not status/heartbeat alone —
//!   `triage::attempt_process_is_live` reuses the same `detect_dead_reason`
//!   kernel start-time check the rest of WG uses),
//! * **REAPs** every proven-dead attempt by delegating to the canonical
//!   dead-agent triage reconciler ([`triage::cleanup_dead_agents`]), which owns
//!   the fenced `AttemptLost` transition and attempt-end cleanup, and
//! * **HONORs** every live attempt, so the dispatch loop refuses to mint a
//!   second identity/worktree for a task that already has one.
//!
//! The dispatch loop also calls [`live_attempt_for_task_in`] on every task it
//! is about to spawn, so idempotency holds even for `wg service tick` (which
//! never runs the daemon startup path) and for a restart that happens mid-tick.
//!
//! ## Fences are not weakened
//!
//! This pass never relaxes a fence. A genuinely concurrent second spawn is
//! still refused (the live-attempt guard here *is* the claim fence applied one
//! step earlier, with process identity); the different-task worktree guard in
//! `commands::spawn::execution` still fails closed. This removes the redundant
//! dispatch that made those fences fire, it does not loosen them.
//!
//! ## Composition with the existing attempt-loss fixes
//!
//! * `triage::cleanup_dead_agents` (per-tick) reaps dead attempts and requests
//!   the fenced `AttemptLost` transition; this startup pass is idempotent with
//!   it and shares the same `detect_dead_reason` identity check.
//! * The `service_kill_descendants` session+registry guard protects a *live*
//!   attempt's descendant processes across a restart; because we HONOR live
//!   attempts, a surviving worker is never re-dispatched or reaped here.
//! * `commands::sweep` (`CLAIM_GRACE_MINUTES` / stale-claim release) reopens an
//!   `InProgress` task whose claim is older than the grace window. That is the
//!   slow backstop; this pass is the immediate, restart-time one. Both key on
//!   the same process-identity liveness, so a restart is safe **here** rather
//!   than independently in three places.

use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;

use worksgood::config::Config;
use worksgood::graph::Status;
use worksgood::service::registry::{AgentEntry, AgentRegistry, AgentStatus};

use super::triage;

/// Outcome of the one-pass startup reconcile, surfaced so the daemon can log it
/// loudly (resumed N, reaped N) instead of leaking a stream of near-identical
/// dispatch failures.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupReconcileReport {
    /// Registry ids of live attempts that were honored (the dispatcher must NOT
    /// re-dispatch their tasks).
    pub resumed: Vec<String>,
    /// Registry ids of proven-dead attempts that were reaped (marked `Dead`).
    pub reaped: Vec<String>,
    /// Tasks that already have ≥1 honored live attempt (deduped).
    pub live_tasks: Vec<String>,
}

impl StartupReconcileReport {
    pub(crate) fn resumed_count(&self) -> usize {
        self.resumed.len()
    }

    pub(crate) fn reaped_count(&self) -> usize {
        self.reaped.len()
    }
}

/// Find the live attempt (if any) for `task_id` within an already-loaded
/// registry, using process-identity liveness.
///
/// This is the restart-idempotency guard the dispatch loop consults before
/// spawning: a task with a live attempt must not be re-dispatched, no matter
/// what its graph status says (a `wg retry` can reset a task to `Open` while
/// the previous attempt's process is still alive).
pub(crate) fn live_attempt_for_task_in<'a>(
    registry: &'a AgentRegistry,
    task_id: &str,
    grace_period_secs: i64,
) -> Option<&'a AgentEntry> {
    registry
        .all()
        .filter(|agent| agent.task_id == task_id && agent.is_alive())
        .find(|agent| triage::attempt_process_is_live(agent, grace_period_secs))
}

/// Convenience wrapper that loads the registry from disk. Prefer
/// [`live_attempt_for_task_in`] when the caller already holds a registry.
pub(crate) fn live_attempt_for_task(dir: &Path, task_id: &str) -> Option<AgentEntry> {
    let registry = AgentRegistry::load(dir).ok()?;
    let grace = Config::load_or_default(dir).agent.reaper_grace_seconds as i64;
    live_attempt_for_task_in(&registry, task_id, grace).cloned()
}

/// One-pass startup reconcile. See the module docs for the full contract.
///
/// Reaping is delegated to the **canonical** dead-agent triage reconciler
/// ([`triage::cleanup_dead_agents`]) rather than marking entries `Dead` here:
/// it owns the fenced `AttemptLost` transition, token extraction, and attempt-
/// end cache cull, so doing the reap ourselves would *mask* that accounting.
/// The startup pass therefore just runs it once before the first tick (it also
/// runs every tick as Phase 1, so this is idempotent) and reports the honored
/// live attempts. Idempotent: a second call reconciles nothing new.
pub(crate) fn reconcile_inflight_on_startup(
    dir: &Path,
    graph_path: &Path,
) -> Result<StartupReconcileReport> {
    let config = Config::load_or_default(dir);
    let grace_secs = config.agent.reaper_grace_seconds as i64;

    // (1) Reap dead attempts with the canonical reconciler.
    let reaped = triage::cleanup_dead_agents(dir, graph_path)
        .context("startup reconcile: reap dead attempts")?;

    // (2) Honor live attempts: report them so the dispatcher's per-task guard
    // refuses to mint a second identity. We never mutate a live entry.
    let mut report = StartupReconcileReport {
        reaped,
        ..StartupReconcileReport::default()
    };
    if let Ok(registry) = AgentRegistry::load(dir) {
        for agent in registry.all() {
            if agent.is_alive() && triage::attempt_process_is_live(agent, grace_secs) {
                report.resumed.push(agent.id.clone());
                if !report.live_tasks.contains(&agent.task_id) {
                    report.live_tasks.push(agent.task_id.clone());
                }
            }
        }
    }

    // Loud, one-line, greppable. The operator sees churn as counts, not as a
    // stream of near-identical per-attempt failures.
    if report.reaped_count() > 0 || report.resumed_count() > 0 {
        eprintln!(
            "[startup-reconcile] restart-idempotent in-flight reconcile: resumed(honored)={} reaped(dead)={} live_tasks={:?} — a task with a live attempt will not be re-dispatched",
            report.resumed_count(),
            report.reaped_count(),
            report.live_tasks
        );
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn scratch_dir(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wg-startup-reconcile-{tag}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("service")).unwrap();
        root
    }

    fn live_entry(id: &str, task_id: &str, pid: u32, started_at: String) -> AgentEntry {
        AgentEntry {
            id: id.to_string(),
            pid,
            task_id: task_id.to_string(),
            executor: "pi".to_string(),
            started_at,
            last_heartbeat: Utc::now().to_rfc3339(),
            status: AgentStatus::Working,
            output_file: "stream.jsonl".to_string(),
            model: None,
            completed_at: None,
            worktree_path: None,
            pgid: None,
        }
    }

    #[test]
    fn startup_reconcile_reaps_dead_pid_and_honors_live_pid() {
        let dir = scratch_dir("reap-live");
        // A pid that is verifiably not running: use a very large, unlikely pid.
        let dead_pid = 3_999_999;
        let live_pid = std::process::id();

        let mut registry = AgentRegistry::new();
        // Backdate so the reaper grace period cannot mask the death.
        let dead = live_entry(
            "agent-1",
            "task-dead",
            dead_pid,
            "2000-01-01T00:00:00Z".to_string(),
        );
        // Our own live PID with a current start time verifies against the
        // kernel start-time identity (the 120s slack absorbs test skew).
        let alive = live_entry("agent-2", "task-live", live_pid, Utc::now().to_rfc3339());
        registry.agents.insert("agent-1".to_string(), dead);
        registry.agents.insert("agent-2".to_string(), alive);
        registry.save(&dir).unwrap();

        let graph_path = dir.join("graph.jsonl");
        let mut graph = worksgood::WorkGraph::new();
        graph.add_node(worksgood::graph::Node::Task(worksgood::graph::Task {
            id: "task-dead".to_string(),
            title: "dead".to_string(),
            status: Status::Open,
            ..Default::default()
        }));
        graph.add_node(worksgood::graph::Node::Task(worksgood::graph::Task {
            id: "task-live".to_string(),
            title: "live".to_string(),
            status: Status::InProgress,
            ..Default::default()
        }));
        worksgood::parser::save_graph(&graph, &graph_path).unwrap();

        let report = reconcile_inflight_on_startup(&dir, &graph_path).unwrap();
        assert_eq!(report.reaped, vec!["agent-1".to_string()]);
        assert_eq!(report.resumed, vec!["agent-2".to_string()]);

        // The dead entry is now terminal, so no later guard can treat it as a
        // live owner.
        let after = AgentRegistry::load(&dir).unwrap();
        assert_eq!(
            after.get_agent("agent-1").unwrap().status,
            AgentStatus::Dead
        );
        assert!(after.get_agent("agent-2").unwrap().is_alive());

        // And the live-attempt guard now honors the live one / ignores the dead one.
        assert!(live_attempt_for_task_in(&after, "task-live", 0).is_some());
        assert!(live_attempt_for_task_in(&after, "task-dead", 0).is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_reconcile_is_idempotent() {
        let dir = scratch_dir("idempotent");
        let dead_pid = 3_999_998;
        let mut registry = AgentRegistry::new();
        let dead = live_entry(
            "agent-9",
            "task-x",
            dead_pid,
            "2000-01-01T00:00:00Z".to_string(),
        );
        registry.agents.insert("agent-9".to_string(), dead);
        registry.save(&dir).unwrap();

        let graph_path = dir.join("graph.jsonl");
        let mut graph = worksgood::WorkGraph::new();
        graph.add_node(worksgood::graph::Node::Task(worksgood::graph::Task {
            id: "task-x".to_string(),
            title: "x".to_string(),
            status: Status::Open,
            ..Default::default()
        }));
        worksgood::parser::save_graph(&graph, &graph_path).unwrap();

        let first = reconcile_inflight_on_startup(&dir, &graph_path).unwrap();
        assert_eq!(first.reaped_count(), 1);
        let second = reconcile_inflight_on_startup(&dir, &graph_path).unwrap();
        assert_eq!(
            second.reaped_count(),
            0,
            "already-reaped entries are terminal"
        );
        assert_eq!(second.resumed_count(), 0);

        let _ = fs::remove_dir_all(&dir);
    }
}
