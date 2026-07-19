use anyhow::Result;
use chrono::Utc;
use std::io::Read;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;
use worksgood::service::AgentRegistry;

/// Update an agent's last_heartbeat timestamp
///
/// This is for agent processes registered in the service registry.
/// Agent IDs are in the format "agent-N" (e.g., agent-1, agent-7).
pub fn run_agent(dir: &Path, agent_id: &str) -> Result<()> {
    let mut registry = AgentRegistry::load_locked(dir)?;

    let now = Utc::now().to_rfc3339();
    registry.update_heartbeat(agent_id)?;
    registry.save()?;

    println!("Agent heartbeat recorded for '{}' at {}", agent_id, now);
    Ok(())
}

/// Run heartbeat callbacks while `guard` remains open.
///
/// Generated wrappers connect `guard` to an anonymous pipe whose only writer
/// belongs to the wrapper shell. The executor command runs with that writer
/// file descriptor explicitly closed. Normal wrapper completion kills this
/// watcher; an untrappable wrapper death closes the pipe in the kernel, wakes
/// the reader immediately, and lets the watcher exit instead of leaving a
/// `sleep 120` orphan behind.
fn run_guarded_heartbeat<R, F>(mut guard: R, interval: Duration, mut heartbeat: F) -> Result<()>
where
    R: Read + Send + 'static,
    F: FnMut(),
{
    if interval.is_zero() {
        anyhow::bail!("heartbeat interval must be greater than zero");
    }

    let (closed_tx, closed_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        // No data is written to the guard. Reading until EOF is deliberate:
        // EOF is delivered by the kernel as soon as the wrapper's only write
        // descriptor closes, including when the wrapper is SIGKILLed.
        let _ = std::io::copy(&mut guard, &mut std::io::sink());
        let _ = closed_tx.send(());
    });

    loop {
        match closed_rx.recv_timeout(interval) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => heartbeat(),
        }
    }

    // The reader sent only after read returned, so this join cannot block.
    let _ = reader.join();
    Ok(())
}

/// Watch the generated wrapper's stdin guard and refresh its agent heartbeat.
///
/// This is an internal command used only by generated `run.sh` wrappers. A
/// heartbeat failure is intentionally non-fatal (matching the historical
/// `wg heartbeat ... || true` loop); the next cadence retries while the
/// wrapper is still alive.
pub fn run_watch(dir: &Path, agent_id: &str, interval_seconds: u64) -> Result<()> {
    if !is_agent_id(agent_id) {
        anyhow::bail!("heartbeat watcher requires an agent ID, got '{agent_id}'");
    }

    let dir = dir.to_path_buf();
    let agent_id = agent_id.to_string();
    run_guarded_heartbeat(
        std::io::stdin(),
        Duration::from_secs(interval_seconds),
        move || {
            let _ = run_agent(&dir, &agent_id);
        },
    )
}

/// Check if the given ID is an agent ID (starts with "agent-")
pub fn is_agent_id(id: &str) -> bool {
    id.starts_with("agent-")
}

/// Record heartbeat for an agent
///
/// Validates the ID is an agent ID (agent-N format) before recording.
///
/// External-trigger interop (impl-recurring-heartbeat-diagnostics): the ONLY
/// safe external heartbeat contract is `wg heartbeat agent-N` against a
/// LIVE agent PID the caller owns. Anything else (host cron doing `wg add`,
/// `wg done`, or graph edits to "poke" a recurring task) races the dispatcher
/// and is rejected here with a diagnostic pointing at the safe path. See
/// `docs/research/recurring-wakeup-heartbeat-gaps.md` §4.4 and
/// `docs/repro-weekly-wakeup-heartbeat.md` (external heartbeat interop).
pub fn run_auto(dir: &Path, id: &str) -> Result<()> {
    if is_agent_id(id) {
        run_agent(dir, id)
    } else {
        anyhow::bail!(
            "Unknown ID '{}'. `wg heartbeat` is an AGENT-PROCESS liveness ping, \
             not a recurring-task trigger. Actor nodes have been removed; only \
             agent IDs (e.g. agent-1) are accepted.\n\
             \n\
             External-trigger interop contract:\n\
             \n  • To keep a long-running agent alive from outside: `wg heartbeat \
             agent-N` (refreshes last_heartbeat; the service reaper respects \
             it ONLY while the agent PID is alive — a heartbeat for a gone \
             process does NOT resurrect it).\n  • To diagnose why a recurring \
             cron task did or did not wake: `wg cron` (next/last fire, weekday, \
             due/overdue/paused state, missed-fire count).\n  • Do NOT poke a \
             recurring task via `wg add` / `wg done` / graph edits from a host \
             cron — the dispatcher tick reverts those writes and the two loops \
             fight (the 'heartbeat fights the agent' symptom).",
            id
        )
    }
}

/// Check for stale agents (no heartbeat within threshold)
///
/// This checks agent processes registered in the service registry.
pub fn run_check_agents(dir: &Path, threshold_minutes: u64, json: bool) -> Result<()> {
    let registry = AgentRegistry::load(dir)?;
    let threshold_secs = threshold_minutes.saturating_mul(60) as i64;

    let mut stale_agents = Vec::new();
    let mut active_agents = Vec::new();
    let mut dead_agents = Vec::new();

    for agent in registry.list_agents() {
        // Already marked as dead
        if agent.status == worksgood::service::AgentStatus::Dead {
            dead_agents.push((
                agent.id.clone(),
                agent.task_id.clone(),
                agent.last_heartbeat.clone(),
            ));
            continue;
        }

        // Not alive (done, failed, stopping)
        if !agent.is_alive() {
            continue;
        }

        if let Some(secs) = agent.seconds_since_heartbeat() {
            let mins = secs / 60;
            if secs > threshold_secs {
                stale_agents.push((
                    agent.id.clone(),
                    agent.task_id.clone(),
                    agent.last_heartbeat.clone(),
                    mins,
                ));
            } else {
                active_agents.push((
                    agent.id.clone(),
                    agent.task_id.clone(),
                    agent.last_heartbeat.clone(),
                    mins,
                ));
            }
        } else {
            // Can't parse heartbeat - consider stale
            stale_agents.push((
                agent.id.clone(),
                agent.task_id.clone(),
                agent.last_heartbeat.clone(),
                -1,
            ));
        }
    }

    if json {
        let output = serde_json::json!({
            "threshold_minutes": threshold_minutes,
            "stale": stale_agents.iter().map(|(id, task, last_hb, mins)| {
                serde_json::json!({
                    "id": id,
                    "task_id": task,
                    "last_heartbeat": last_hb,
                    "minutes_ago": mins,
                })
            }).collect::<Vec<_>>(),
            "active": active_agents.iter().map(|(id, task, last_hb, mins)| {
                serde_json::json!({
                    "id": id,
                    "task_id": task,
                    "last_heartbeat": last_hb,
                    "minutes_ago": mins,
                })
            }).collect::<Vec<_>>(),
            "dead": dead_agents.iter().map(|(id, task, last_hb)| {
                serde_json::json!({
                    "id": id,
                    "task_id": task,
                    "last_heartbeat": last_hb,
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "Agent heartbeat status (threshold: {} minutes):",
            threshold_minutes
        );
        println!();

        if !active_agents.is_empty() {
            println!("Active agents:");
            for (id, task, _, mins) in &active_agents {
                println!("  {} on '{}' (heartbeat {} min ago)", id, task, mins);
            }
        }

        if !stale_agents.is_empty() {
            println!();
            println!("Stale agents (may be dead):");
            for (id, task, last_hb, mins) in &stale_agents {
                if *mins < 0 {
                    println!("  {} on '{}' (invalid heartbeat: {})", id, task, last_hb);
                } else {
                    println!("  {} on '{}' (last heartbeat {} min ago)", id, task, mins);
                }
            }
        }

        if !dead_agents.is_empty() {
            println!();
            println!("Dead agents:");
            for (id, task, _) in &dead_agents {
                println!("  {} was on '{}'", id, task);
            }
        }

        if active_agents.is_empty() && stale_agents.is_empty() && dead_agents.is_empty() {
            println!("No agents registered.");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc as test_mpsc};
    use std::time::Instant;
    use tempfile::TempDir;
    use worksgood::graph::WorkGraph;
    use worksgood::parser::save_graph;

    fn setup_with_agent() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        // Create a graph file first
        let path = temp_dir.path().join("graph.jsonl");
        let graph = WorkGraph::new();
        save_graph(&graph, &path).unwrap();

        // Register an agent
        let mut registry = AgentRegistry::new();
        registry.register_agent(12345, "test-task", "claude", "/tmp/output.log");
        registry.save(temp_dir.path()).unwrap();

        temp_dir
    }

    struct ChannelGuard(test_mpsc::Receiver<()>);

    impl Read for ChannelGuard {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.recv() {
                Ok(()) if !buf.is_empty() => {
                    buf[0] = 0;
                    Ok(1)
                }
                Ok(()) => Ok(0),
                Err(_) => Ok(0),
            }
        }
    }

    #[test]
    fn guarded_heartbeat_keeps_cadence_and_stops_promptly_on_eof() {
        let (guard_tx, guard_rx) = test_mpsc::channel();
        let beats = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&beats);
        let closer_observed = Arc::clone(&beats);
        let closer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while closer_observed.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            drop(guard_tx);
        });

        let started = Instant::now();
        run_guarded_heartbeat(ChannelGuard(guard_rx), Duration::from_millis(15), || {
            observed.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        closer.join().unwrap();

        assert!(
            beats.load(Ordering::SeqCst) >= 3,
            "watcher did not preserve periodic heartbeat cadence"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "guard EOF did not stop heartbeat watcher promptly"
        );
    }

    #[test]
    fn guarded_heartbeat_closed_before_start_emits_no_heartbeat() {
        let (guard_tx, guard_rx) = test_mpsc::channel();
        drop(guard_tx);
        let mut beats = 0;
        run_guarded_heartbeat(ChannelGuard(guard_rx), Duration::from_millis(10), || {
            beats += 1
        })
        .unwrap();
        assert_eq!(beats, 0);
    }

    #[test]
    fn test_heartbeat_non_agent_fails() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("graph.jsonl");
        let graph = WorkGraph::new();
        save_graph(&graph, &path).unwrap();

        // Actor nodes no longer exist, so heartbeat for non-agent IDs should fail
        let result = run_auto(temp_dir.path(), "test-agent");
        assert!(result.is_err());
    }

    #[test]
    fn test_check_agents_no_agents() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("graph.jsonl");
        let graph = WorkGraph::new();
        save_graph(&graph, &path).unwrap();

        // Should succeed with no agents registered
        let result = run_check_agents(temp_dir.path(), 5, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_agent_id() {
        assert!(is_agent_id("agent-1"));
        assert!(is_agent_id("agent-42"));
        assert!(is_agent_id("agent-999"));
        assert!(!is_agent_id("erik"));
        assert!(!is_agent_id("test-agent"));
        assert!(!is_agent_id("claude-agent"));
    }

    #[test]
    fn test_agent_heartbeat() {
        let temp_dir = setup_with_agent();

        // Get initial heartbeat
        let registry = AgentRegistry::load(temp_dir.path()).unwrap();
        let original_hb = registry
            .get_agent("agent-1")
            .unwrap()
            .last_heartbeat
            .clone();

        // Wait a tiny bit
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Record heartbeat
        let result = run_agent(temp_dir.path(), "agent-1");
        assert!(result.is_ok());

        // Verify heartbeat was updated
        let registry = AgentRegistry::load(temp_dir.path()).unwrap();
        let new_hb = registry
            .get_agent("agent-1")
            .unwrap()
            .last_heartbeat
            .clone();
        assert_ne!(original_hb, new_hb);
    }

    #[test]
    fn test_agent_heartbeat_unknown() {
        let temp_dir = setup_with_agent();

        let result = run_agent(temp_dir.path(), "agent-999");
        assert!(result.is_err());
    }

    #[test]
    fn test_run_auto_with_agent() {
        let temp_dir = setup_with_agent();

        // Should detect agent-1 as an agent ID and use run_agent
        let result = run_auto(temp_dir.path(), "agent-1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_auto_with_non_agent_fails() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("graph.jsonl");
        let graph = WorkGraph::new();
        save_graph(&graph, &path).unwrap();

        // Non-agent IDs now fail since Actor nodes are removed
        let result = run_auto(temp_dir.path(), "test-agent");
        assert!(result.is_err());
    }

    #[test]
    fn test_check_agents_empty() {
        let temp_dir = TempDir::new().unwrap();
        // Create graph file
        let path = temp_dir.path().join("graph.jsonl");
        let graph = WorkGraph::new();
        save_graph(&graph, &path).unwrap();

        // No agents registered
        let result = run_check_agents(temp_dir.path(), 5, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_agents_with_active() {
        let temp_dir = setup_with_agent();

        // Agent was just registered, should be active
        let result = run_check_agents(temp_dir.path(), 5, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_agents_json() {
        let temp_dir = setup_with_agent();

        // Should output valid JSON
        let result = run_check_agents(temp_dir.path(), 5, true);
        assert!(result.is_ok());
    }
}
