//! `wg reset` — bulk-reset a subgraph from a seed (or set of seeds).
//!
//! Motivation: the user has a tangled section of the graph (failed
//! tasks, in-progress work that should be canceled, a thicket of
//! `.evaluate-*` / `.flip-*` / `.verify-*` / `.assign-*` meta-task
//! scaffolding) and wants to wipe it back to a fresh state to retry.
//! Doing this task-by-task is tedious; `wg reset` closes the set
//! around a seed and resets the whole region atomically.
//!
//! Semantics:
//!
//! 1. Compute the closure starting from `seeds`, following edges in
//!    the chosen `Direction`:
//!    - `Forward` (default): downstream — everything the seeds block
//!      (follow `task.before`).
//!    - `Backward`: upstream — everything the seeds depend on (follow
//!      `task.after`).
//!    - `Both`: union of the two.
//!
//!    The seeds themselves are always included.
//!
//! 2. For each task in the closure:
//!    - If in a non-terminal running state (InProgress, PendingValidation):
//!      status → Open. The coordinator's next tick notices the claim
//!      is gone and the agent will unclaim on its next heartbeat. A
//!      separate `wg kill` is still needed if the user wants to kill
//!      the worker process itself.
//!    - If terminal (Done, Failed, Abandoned, Waiting, Blocked):
//!      status → Open, clear `completed_at`, `failure_reason`,
//!      `retry_count`.
//!    - Leave the log alone — it's historical audit.
//!    - Append a log entry recording the reset.
//!
//! 3. If `--also-strip-meta`: identify all system (dot-prefixed) tasks
//!    that have at least one edge referencing a closure member, and
//!    delete them entirely. This wipes the agency-pipeline
//!    scaffolding (`.flip-*`, `.verify-*`, `.evaluate-*`, `.assign-*`,
//!    `.place-*`, `.verify-deferred-*`) from around the closure so the
//!    coordinator re-generates fresh ones on the next tick (rather
//!    than reviving stale status-done ones).
//!
//! 4. `--dry-run` (default recommended when first testing): prints the
//!    closure and the meta tasks that would be stripped, without
//!    mutating anything.
//!
//! 5. `--yes`: required when affecting more than one task AND not
//!    doing a dry run. Protects against "whoops I didn't realize
//!    `reset` was transitive".

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;

#[cfg(test)]
use worksgood::graph::Status;
use worksgood::graph::{LogEntry, WorkGraph};
use worksgood::lifecycle::LifecycleActor;
use worksgood::parser::{load_graph, modify_graph};

/// Edge direction to follow when computing the closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Follow `task.before` edges — everything the seeds block.
    Forward,
    /// Follow `task.after` edges — everything the seeds depend on.
    Backward,
    /// Union of both.
    Both,
}

impl std::str::FromStr for Direction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "forward" | "down" | "downstream" => Ok(Direction::Forward),
            "backward" | "up" | "upstream" => Ok(Direction::Backward),
            "both" => Ok(Direction::Both),
            other => Err(format!(
                "invalid direction '{}' — must be one of: forward, backward, both",
                other
            )),
        }
    }
}

pub struct ResetOptions {
    pub direction: Direction,
    pub also_strip_meta: bool,
    pub dry_run: bool,
    pub yes: bool,
}

/// Outcome describing what happened (or would happen, on dry run).
/// Fields are currently only read by integration-style callers (tests
/// and scripted drivers); the `Debug` impl is also load-bearing for
/// test inspection. Mark allow(dead_code) to keep clippy happy while
/// preserving the public surface.
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct ResetReport {
    pub closure: Vec<String>,
    pub meta_to_strip: Vec<String>,
    pub was_dry_run: bool,
    pub reset_count: usize,
    pub stripped_count: usize,
}

pub fn run(dir: &Path, seeds: &[String], opts: ResetOptions) -> Result<ResetReport> {
    if seeds.is_empty() {
        anyhow::bail!("reset requires at least one seed task id");
    }

    let path = super::graph_path(dir);
    if !path.exists() {
        anyhow::bail!("WG not initialized. Run 'wg init' first.");
    }

    // First pass (read-only) to compute closure + meta tasks so we can
    // report / ask for confirmation before we mutate.
    let (closure, meta_to_strip, stale_eval_satellites, missing) = {
        let (g, _) = super::load_workgraph(dir)?;
        let missing: Vec<String> = seeds
            .iter()
            .filter(|s| g.get_task(s).is_none())
            .cloned()
            .collect();
        if !missing.is_empty() {
            anyhow::bail!("seed task(s) not found: {}", missing.join(", "));
        }
        let closure = compute_closure(&g, seeds, opts.direction);
        let meta = if opts.also_strip_meta {
            find_meta_attached_to_closure(&g, &closure)
        } else {
            HashSet::new()
        };
        // Always cancel stale agency satellites whose source is being reset,
        // independent of --also-strip-meta (see find_stale_eval_satellites_for_sources).
        let non_system_closure: HashSet<String> = closure
            .iter()
            .filter(|id| !worksgood::graph::is_system_task(id))
            .cloned()
            .collect();
        let stale_satellites = find_stale_eval_satellites_for_sources(&g, &non_system_closure);
        (closure, meta, stale_satellites, missing)
    };

    let _ = missing; // checked above

    // Sorted for deterministic reporting.
    let mut closure_sorted: Vec<String> = closure.iter().cloned().collect();
    closure_sorted.sort();
    let mut meta_sorted: Vec<String> = meta_to_strip.iter().cloned().collect();
    meta_sorted.sort();
    let mut satellites_sorted: Vec<String> = stale_eval_satellites.iter().cloned().collect();
    satellites_sorted.sort();

    // Dry-run / pre-confirm report.
    eprintln!(
        "\x1b[1mwg reset\x1b[0m — seeds={}, direction={:?}, closure={} task(s), \
         meta-to-strip={}, stale-eval-satellites={}",
        seeds.join(","),
        opts.direction,
        closure_sorted.len(),
        meta_sorted.len(),
        satellites_sorted.len(),
    );
    for id in &closure_sorted {
        eprintln!("  • {}", id);
    }
    if opts.also_strip_meta && !meta_sorted.is_empty() {
        eprintln!("  meta tasks that would be stripped:");
        for id in &meta_sorted {
            eprintln!("    - {}", id);
        }
    }
    if !satellites_sorted.is_empty() {
        eprintln!(
            "  stale agency satellites that would be cancelled (regenerate on next completion):"
        );
        for id in &satellites_sorted {
            eprintln!("    - {}", id);
        }
    }

    if opts.dry_run {
        eprintln!("\x1b[2m(dry run — no changes applied; drop --dry-run to execute)\x1b[0m");
        return Ok(ResetReport {
            closure: closure_sorted,
            meta_to_strip: meta_sorted,
            was_dry_run: true,
            reset_count: 0,
            stripped_count: 0,
        });
    }

    // Destructive confirmation: require --yes if touching more than one task.
    let destructive_count = closure_sorted.len() + meta_sorted.len() + satellites_sorted.len();
    if destructive_count > 1 && !opts.yes {
        anyhow::bail!(
            "refusing to reset {} tasks without --yes (use --dry-run first to \
             preview, then re-run with --yes)",
            destructive_count
        );
    }

    let closure_set: HashSet<String> = closure_sorted.iter().cloned().collect();
    let meta_set: HashSet<String> = meta_sorted.iter().cloned().collect();
    let satellites_set: HashSet<String> = satellites_sorted.iter().cloned().collect();
    let seeds_owned: Vec<String> = seeds.to_vec();

    let mut reset_count = 0usize;
    let mut stripped_count = 0usize;

    let _ = modify_graph(&path, |graph| {
        let user = worksgood::current_user();
        let now = Utc::now().to_rfc3339();

        // Reset the closure tasks.
        for id in &closure_set {
            if let Some(task) = graph.get_task_mut(id) {
                let prev = task.status;
                let prev_assigned = task.assigned.clone();
                let (_, newly_requested) = match super::reopen::request(
                    task,
                    "reset",
                    false,
                    false,
                    "explicit reset using retained work in a new attempt",
                    LifecycleActor::operator(user.clone()),
                    "explicit_reset",
                ) {
                    Ok(result) => result,
                    Err(rejection) => {
                        task.log.push(LogEntry {
                            timestamp: now.clone(),
                            actor: Some("reset".to_string()),
                            user: Some(user.clone()),
                            message: format!("reset rejected: {rejection}"),
                        });
                        continue;
                    }
                };
                if !newly_requested {
                    continue;
                }
                task.completed_at = None;
                task.failure_reason = None;
                task.retry_count = 0;
                // The exact prior claim remains visible while ownership drains;
                // the reopen reaper clears it in the same transaction that
                // enables the new generation.
                let claim_note = match &prev_assigned {
                    Some(a) => format!(" (cleared claim from @{})", a),
                    None => String::new(),
                };
                task.log.push(LogEntry {
                    timestamp: now.clone(),
                    actor: Some("reset".to_string()),
                    user: Some(user.clone()),
                    message: format!(
                        "reset via `wg reset {}`; was {:?}{}",
                        seeds_owned.join(","),
                        prev,
                        claim_note
                    ),
                });
                reset_count += 1;
            }
        }

        // Strip meta tasks outright (deletion — they'll regenerate if
        // the agency pipeline still wants them). Also clean up any
        // references to these stripped meta tasks from remaining tasks'
        // before/after lists so the graph stays edge-consistent.
        //
        // The stale-eval-satellite set is folded in here: those satellites
        // (`.evaluate-X` / `.flip-X` / `.assign-X` whose source was reset) are
        // deleted and edge-cleaned exactly like --also-strip-meta meta, but
        // they are cancelled ALWAYS (not gated on --also-strip-meta) because
        // a stale evaluation scaffold on a reset source is the zombie behind
        // the resolve-prophage-source incident regardless of the broader meta
        // strip opt-in.
        let mut purge_set: HashSet<String> = meta_set.clone();
        purge_set.extend(satellites_set.iter().cloned());
        let all_task_ids: Vec<String> = graph.tasks().map(|t| t.id.clone()).collect();
        for id in &purge_set {
            if graph.remove_node(id).is_some() {
                stripped_count += 1;
            }
            for tid in &all_task_ids {
                if purge_set.contains(tid) {
                    continue; // already deleted
                }
                if let Some(t) = graph.get_task_mut(tid) {
                    t.after.retain(|a| a != id);
                    t.before.retain(|b| b != id);
                }
            }
        }

        true
    });

    // Cross-cutting audit entry.
    let _ = worksgood::provenance::record(
        dir,
        "reset",
        None,
        Some("reset"),
        serde_json::json!({
            "seeds": seeds_owned,
            "direction": format!("{:?}", opts.direction),
            "closure": closure_sorted,
            "meta_stripped": meta_sorted,
            "reset_count": reset_count,
            "stripped_count": stripped_count,
        }),
        worksgood::provenance::DEFAULT_ROTATION_THRESHOLD,
    );

    super::notify_graph_changed(dir);
    let _ = super::reopen::reconcile_pending(dir)?;

    eprintln!(
        "\x1b[32m✓\x1b[0m reset {} task(s), stripped {} meta task(s)",
        reset_count, stripped_count
    );
    if let Ok(graph) = load_graph(&path) {
        for id in &closure_sorted {
            if let Some(intent) = graph
                .get_task(id)
                .and_then(|task| task.lifecycle.reopen_intent.as_ref())
            {
                eprintln!("{}", super::reopen::hold_label(intent));
            }
        }
    }

    Ok(ResetReport {
        closure: closure_sorted,
        meta_to_strip: meta_sorted,
        was_dry_run: false,
        reset_count,
        stripped_count,
    })
}

/// Compute the closure of tasks reachable from `seeds` via the given
/// direction. Seeds are always included (even if they have no edges).
/// System (dot-prefixed) tasks encountered during traversal are NOT
/// added to the closure — they're handled separately by the meta-strip
/// path. Keeping them out here ensures `wg reset foo` does not
/// accidentally reset `.flip-foo` etc via the closure reset path (which
/// would just set them to Open and leave them lying around); the
/// correct behavior for a meta task during a reset is DELETE, and
/// that's gated behind `--also-strip-meta`.
fn compute_closure(graph: &WorkGraph, seeds: &[String], direction: Direction) -> HashSet<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = seeds
        .iter()
        .filter(|s| !worksgood::graph::is_system_task(s))
        .cloned()
        .collect();

    while let Some(id) = stack.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let task = match graph.get_task(&id) {
            Some(t) => t,
            None => continue,
        };
        let next_ids: Vec<String> = match direction {
            Direction::Forward => task.before.clone(),
            Direction::Backward => task.after.clone(),
            Direction::Both => {
                let mut c = task.before.clone();
                c.extend(task.after.iter().cloned());
                c
            }
        };
        for nid in next_ids {
            if !visited.contains(&nid) && !worksgood::graph::is_system_task(&nid) {
                stack.push(nid);
            }
        }
    }
    visited
}

/// Find all system (dot-prefixed) tasks that have at least one edge
/// (after or before) pointing to a closure member. These are the
/// agency-pipeline scaffolding around the closure and get stripped
/// with `--also-strip-meta`.
fn find_meta_attached_to_closure(graph: &WorkGraph, closure: &HashSet<String>) -> HashSet<String> {
    graph
        .tasks()
        .filter(|t| worksgood::graph::is_system_task(&t.id))
        .filter(|t| {
            t.after.iter().any(|a| closure.contains(a))
                || t.before.iter().any(|b| closure.contains(b))
        })
        .map(|t| t.id.clone())
        .collect()
}

/// Find the agency-pipeline satellites (`.evaluate-X` / `.flip-X` /
/// `.assign-X`) that are stale because their source `X` is being reset.
///
/// These are the zombies behind the resolve-prophage-source incident: a
/// reset/retry of `X` left an already-scheduled `.evaluate-X` behind, which
/// respawned against the now-open source and failed instantly with "task is
/// open — can't evaluate". Cancelling them on reset (so the agency pipeline
/// regenerates a fresh one when `X` next completes) is required for a clean
/// retry regardless of `--also-strip-meta` — the broader meta strip is
/// opt-in, but a stale evaluation scaffold on a reset source is always a bug.
///
/// Only satellites whose source is a NON-SYSTEM closure member are returned:
/// resetting `.evaluate-t` must not cascade onto `.evaluate-.evaluate-t`.
fn find_stale_eval_satellites_for_sources(
    graph: &WorkGraph,
    closure_sources: &HashSet<String>,
) -> HashSet<String> {
    let prefixes = [".evaluate-", ".flip-", ".assign-"];
    graph
        .tasks()
        .filter(|t| prefixes.iter().any(|p| t.id.starts_with(p)))
        .filter(|t| {
            t.id.strip_prefix(".evaluate-")
                .or_else(|| t.id.strip_prefix(".flip-"))
                .or_else(|| t.id.strip_prefix(".assign-"))
                .is_some_and(|source| {
                    !worksgood::graph::is_system_task(source) && closure_sources.contains(source)
                })
        })
        .map(|t| t.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use worksgood::graph::Task;
    use worksgood::parser::load_graph;
    use worksgood::test_helpers::{make_task_with_status, setup_workgraph};

    fn make(id: &str, status: Status) -> Task {
        make_task_with_status(id, id, status)
    }

    fn write_chain(dir: &Path) {
        //  p ──► t ──► s
        //  plus meta: .flip-t, .evaluate-t (both attached to t)
        let mut p = make("p", Status::Done);
        let mut t = make("t", Status::Failed);
        let mut s = make("s", Status::Open);
        p.before = vec!["t".into()];
        t.after = vec!["p".into()];
        t.before = vec!["s".into()];
        s.after = vec!["t".into()];

        let mut flip = make(".flip-t", Status::Done);
        flip.after = vec!["t".into()];
        let mut eval = make(".evaluate-t", Status::Open);
        eval.after = vec!["t".into()];

        setup_workgraph(dir, vec![p, t, s, flip, eval]);
    }

    #[test]
    fn forward_closure_includes_seeds_and_downstream() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());
        let (g, _) = crate::commands::load_workgraph(dir.path()).unwrap();
        let c = compute_closure(&g, &["t".to_string()], Direction::Forward);
        let mut ids: Vec<String> = c.into_iter().collect();
        ids.sort();
        assert_eq!(ids, vec!["s".to_string(), "t".to_string()]);
    }

    #[test]
    fn backward_closure_includes_seeds_and_upstream() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());
        let (g, _) = crate::commands::load_workgraph(dir.path()).unwrap();
        let c = compute_closure(&g, &["t".to_string()], Direction::Backward);
        let mut ids: Vec<String> = c.into_iter().collect();
        ids.sort();
        assert_eq!(ids, vec!["p".to_string(), "t".to_string()]);
    }

    #[test]
    fn both_closure_includes_everything_reachable() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());
        let (g, _) = crate::commands::load_workgraph(dir.path()).unwrap();
        let c = compute_closure(&g, &["t".to_string()], Direction::Both);
        let mut ids: Vec<String> = c.into_iter().collect();
        ids.sort();
        assert_eq!(ids, vec!["p".to_string(), "s".to_string(), "t".to_string()]);
    }

    #[test]
    fn closure_skips_system_tasks() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());
        let (g, _) = crate::commands::load_workgraph(dir.path()).unwrap();
        let c = compute_closure(&g, &["t".to_string()], Direction::Both);
        // .flip-t and .evaluate-t should NOT be in the closure
        assert!(!c.contains(".flip-t"));
        assert!(!c.contains(".evaluate-t"));
    }

    #[test]
    fn meta_attached_to_closure_is_found() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());
        let (g, _) = crate::commands::load_workgraph(dir.path()).unwrap();
        let closure: HashSet<String> = ["t", "s"].iter().map(|s| s.to_string()).collect();
        let meta = find_meta_attached_to_closure(&g, &closure);
        let mut ids: Vec<String> = meta.into_iter().collect();
        ids.sort();
        assert_eq!(ids, vec![".evaluate-t".to_string(), ".flip-t".to_string()]);
    }

    #[test]
    fn dry_run_mutates_nothing() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());

        let _ = run(
            dir.path(),
            &["t".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: true,
                dry_run: true,
                yes: true,
            },
        )
        .unwrap();

        let g = load_graph(&super::super::graph_path(dir.path())).unwrap();
        // Target still failed, meta still present
        assert_eq!(g.get_task("t").unwrap().status, Status::Failed);
        assert!(g.get_task(".flip-t").is_some());
        assert!(g.get_task(".evaluate-t").is_some());
    }

    #[test]
    fn full_reset_clears_statuses_and_strips_meta() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());

        let report = run(
            dir.path(),
            &["t".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: true,
                dry_run: false,
                yes: true,
            },
        )
        .unwrap();

        assert_eq!(report.reset_count, 2, "t + s should be reset");
        assert_eq!(
            report.stripped_count, 2,
            ".flip-t + .evaluate-t should be stripped"
        );

        let g = load_graph(&super::super::graph_path(dir.path())).unwrap();
        // t reset from Failed → Open, log entry added
        let t = g.get_task("t").unwrap();
        assert_eq!(t.status, Status::Open);
        assert!(t.failure_reason.is_none());
        assert!(t.log.iter().any(|e| e.message.contains("reset via")));
        // s reset (was already Open but gets a log entry)
        let s = g.get_task("s").unwrap();
        assert_eq!(s.status, Status::Open);
        // meta tasks gone
        assert!(g.get_task(".flip-t").is_none());
        assert!(g.get_task(".evaluate-t").is_none());
        // p unchanged — it's backward from t, not in forward closure
        assert_eq!(g.get_task("p").unwrap().status, Status::Done);
    }

    #[test]
    fn refuses_multi_task_reset_without_yes() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());

        let result = run(
            dir.path(),
            &["t".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: false,
                dry_run: false,
                yes: false,
            },
        );
        assert!(result.is_err());
        let err = format!("{}", result.err().unwrap());
        assert!(err.contains("--yes"));
    }

    #[test]
    fn unknown_seed_errors_cleanly_without_mutation() {
        let dir = tempdir().unwrap();
        write_chain(dir.path());

        let result = run(
            dir.path(),
            &["nonexistent".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: false,
                dry_run: true,
                yes: false,
            },
        );
        assert!(result.is_err());
        let g = load_graph(&super::super::graph_path(dir.path())).unwrap();
        // No mutation
        assert_eq!(g.get_task("t").unwrap().status, Status::Failed);
    }

    #[test]
    fn reset_clears_assigned_field() {
        // A task that was claimed by an agent (status=InProgress, assigned=Some)
        // should have `assigned` cleared after reset, so the dispatcher can
        // re-claim it on the next tick without manual `wg unclaim`.
        let dir = tempdir().unwrap();
        let mut t = make("t", Status::InProgress);
        t.assigned = Some("agent-dead".to_string());
        t.started_at = Some("2026-04-27T00:00:00Z".to_string());
        setup_workgraph(dir.path(), vec![t]);

        let _ = run(
            dir.path(),
            &["t".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: false,
                dry_run: false,
                yes: true,
            },
        )
        .unwrap();

        let g = load_graph(&super::super::graph_path(dir.path())).unwrap();
        let t = g.get_task("t").unwrap();
        assert_eq!(t.status, Status::Open);
        assert!(
            t.assigned.is_none(),
            "reset must clear `assigned` so the task is ready for fresh dispatch"
        );
        assert!(
            t.started_at.is_none(),
            "reset should also clear `started_at` since task is no longer in progress"
        );
    }

    #[test]
    fn reset_with_strip_meta_still_clears_assigned() {
        // Regression check: --also-strip-meta path must not skip the
        // assigned/started_at clearing on the closure tasks.
        let dir = tempdir().unwrap();
        write_chain(dir.path());
        // Mark t as claimed by a dead agent.
        let path = super::super::graph_path(dir.path());
        let mut g = load_graph(&path).unwrap();
        if let Some(t) = g.get_task_mut("t") {
            t.assigned = Some("agent-dead".to_string());
            t.started_at = Some("2026-04-27T00:00:00Z".to_string());
        }
        worksgood::parser::save_graph(&g, &path).unwrap();

        let _ = run(
            dir.path(),
            &["t".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: true,
                dry_run: false,
                yes: true,
            },
        )
        .unwrap();

        let g = load_graph(&path).unwrap();
        let t = g.get_task("t").unwrap();
        assert!(
            t.assigned.is_none(),
            "--also-strip-meta must also clear assigned"
        );
        assert!(t.started_at.is_none());
        // meta tasks gone (regression check on existing strip behavior)
        assert!(g.get_task(".flip-t").is_none());
        assert!(g.get_task(".evaluate-t").is_none());
    }

    #[test]
    fn reset_on_unclaimed_task_is_noop_for_claim_fields() {
        // A task with no claim should reset cleanly without crashing
        // and without spuriously setting/changing claim fields.
        let dir = tempdir().unwrap();
        let t = make("t", Status::Failed); // no assigned, no started_at
        setup_workgraph(dir.path(), vec![t]);

        let _ = run(
            dir.path(),
            &["t".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: false,
                dry_run: false,
                yes: true,
            },
        )
        .unwrap();

        let g = load_graph(&super::super::graph_path(dir.path())).unwrap();
        let t = g.get_task("t").unwrap();
        assert_eq!(t.status, Status::Open);
        assert!(t.assigned.is_none());
        assert!(t.started_at.is_none());
    }

    /// TDD for bug-reset-leaves-stale-claims (named to match the
    /// validation criterion in `fix-claim-lifecycle`).
    /// Asserts the full repro: a task claimed by a dead agent gets
    /// status=Open AND assigned=None after `wg reset`, so the
    /// dispatcher's "ready" check finds it on the next tick.
    #[test]
    fn test_wg_reset_clears_stale_claims() {
        let dir = tempdir().unwrap();
        let mut t = make("stuck", Status::InProgress);
        t.assigned = Some("agent-dead-1".to_string());
        t.started_at = Some("2026-01-01T00:00:00Z".to_string());
        setup_workgraph(dir.path(), vec![t]);

        let _ = run(
            dir.path(),
            &["stuck".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: false,
                dry_run: false,
                yes: true,
            },
        )
        .unwrap();

        let g = load_graph(&super::super::graph_path(dir.path())).unwrap();
        let t = g.get_task("stuck").unwrap();
        assert_eq!(
            t.status,
            Status::Open,
            "reset must move status back to Open"
        );
        assert!(
            t.assigned.is_none(),
            "reset must clear the stale claim — dispatcher won't dispatch otherwise"
        );
        assert!(t.started_at.is_none());
    }

    #[test]
    fn direction_parses_aliases() {
        use std::str::FromStr;
        assert_eq!(Direction::from_str("forward").unwrap(), Direction::Forward);
        assert_eq!(Direction::from_str("down").unwrap(), Direction::Forward);
        assert_eq!(
            Direction::from_str("upstream").unwrap(),
            Direction::Backward
        );
        assert_eq!(Direction::from_str("both").unwrap(), Direction::Both);
        assert!(Direction::from_str("sideways").is_err());
    }

    // ===== Stale evaluation-satellite cancellation (fix-evaluation-satellite) =====

    fn write_source_with_eval_satellite(dir: &Path) {
        // source X (Failed) with a stale .evaluate-X / .flip-X / .assign-X
        // already scaffolded against it (the resolve-prophage-source shape).
        let mut x = make("X", Status::Failed);
        x.failure_reason = Some("boom".to_string());
        let mut eval = make(".evaluate-X", Status::Open);
        eval.after = vec!["X".into()];
        let mut flip = make(".flip-X", Status::Open);
        flip.after = vec!["X".into()];
        let mut assign = make(".assign-X", Status::Open);
        assign.before = vec!["X".into()];
        setup_workgraph(dir, vec![x, eval, flip, assign]);
    }

    #[test]
    fn reset_cancels_stale_eval_satellites_without_strip_meta_flag() {
        // Bulk-retry shape: `wg reset X` (NO --also-strip-meta) must still
        // cancel the stale `.evaluate-X` / `.flip-X` / `.assign-X` so a fresh
        // agency pipeline regenerates when X next completes. Without this, the
        // zombie satellite respawns against the now-open X and fails instantly
        // with "task is open - can't evaluate".
        let dir = tempdir().unwrap();
        write_source_with_eval_satellite(dir.path());

        let report = run(
            dir.path(),
            &["X".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: false,
                dry_run: false,
                yes: true,
            },
        )
        .unwrap();

        let g = load_graph(&super::super::graph_path(dir.path())).unwrap();
        assert_eq!(g.get_task("X").unwrap().status, Status::Open);
        assert!(
            g.get_task(".evaluate-X").is_none(),
            "stale .evaluate-X must be cancelled on reset"
        );
        assert!(
            g.get_task(".flip-X").is_none(),
            "stale .flip-X must be cancelled on reset"
        );
        assert!(
            g.get_task(".assign-X").is_none(),
            "stale .assign-X must be cancelled on reset"
        );
        // The three cancelled satellites are reported in the stripped count.
        assert_eq!(report.stripped_count, 3);
    }

    #[test]
    fn reset_dry_run_reports_stale_satellites_without_mutating() {
        let dir = tempdir().unwrap();
        write_source_with_eval_satellite(dir.path());

        let report = run(
            dir.path(),
            &["X".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: false,
                dry_run: true,
                yes: true,
            },
        )
        .unwrap();
        assert!(report.was_dry_run);
        let g = load_graph(&super::super::graph_path(dir.path())).unwrap();
        assert!(g.get_task(".evaluate-X").is_some());
        assert!(g.get_task(".flip-X").is_some());
    }

    #[test]
    fn reset_does_not_cancel_unrelated_eval_satellites() {
        // Resetting X must not touch `.evaluate-Y` for an unrelated source Y.
        let dir = tempdir().unwrap();
        write_source_with_eval_satellite(dir.path());
        let y = make("Y", Status::Done);
        let mut eval_y = make(".evaluate-Y", Status::Open);
        eval_y.after = vec!["Y".into()];
        let path = super::super::graph_path(dir.path());
        let mut g = load_graph(&path).unwrap();
        g.add_node(worksgood::graph::Node::Task(y));
        g.add_node(worksgood::graph::Node::Task(eval_y));
        worksgood::parser::save_graph(&g, &path).unwrap();

        let _ = run(
            dir.path(),
            &["X".to_string()],
            ResetOptions {
                direction: Direction::Forward,
                also_strip_meta: false,
                dry_run: false,
                yes: true,
            },
        )
        .unwrap();

        let g = load_graph(&path).unwrap();
        assert!(
            g.get_task(".evaluate-Y").is_some(),
            "unrelated .evaluate-Y must survive resetting X"
        );
        assert_eq!(g.get_task("Y").unwrap().status, Status::Done);
    }
}
