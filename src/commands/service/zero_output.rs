//! Zero-output observation adapter.
//!
//! Detects agents whose API call has produced no stream bytes for an extended
//! period and reports typed diagnostics. It has no persistence, process, graph,
//! retry, route, breaker, or global-pause authority.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};

#[cfg(test)]
use worksgood::service::registry::AgentStatus;
use worksgood::service::registry::{AgentEntry, AgentRegistry};
use worksgood::stream_event;

use crate::commands::is_process_alive;

/// Threshold after which a zero-output agent is considered a zombie and killed.
const ZERO_OUTPUT_KILL_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// Fraction of alive agents with zero output that triggers global API-down detection.
const GLOBAL_OUTAGE_RATIO: f64 = 0.5;

/// Minimum alive agents before global outage detection kicks in.
const GLOBAL_OUTAGE_MIN_AGENTS: usize = 2;

/// Result of a zero-output detection sweep.
#[derive(Debug, Default)]
pub struct ZeroOutputSweepResult {
    /// Agents whose typed observation was durably accepted by PlannerStore.
    pub observed: Vec<ZeroOutputEvidence>,
    /// Aggregate diagnostic only; it has no pause/routing authority.
    pub global_outage_detected: bool,
}

/// Details of one persisted zero-output observation.
#[derive(Debug)]
pub struct ZeroOutputEvidence {
    pub agent_id: String,
    pub task_id: String,
    pub pid: u32,
    pub age_secs: u64,
}

/// Migration-only legacy zero-output controller bytes.
///
/// New observations never read or mutate these counters/timers.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ZeroOutputState {
    /// Retired consecutive respawn counters, retained only for deserialization.
    pub task_respawn_counts: HashMap<String, u32>,
    /// Retired global backoff state.
    #[serde(default)]
    pub global_backoff: Option<GlobalBackoffState>,
}

/// Persistent global backoff state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlobalBackoffState {
    /// When the current backoff period expires (ISO 8601).
    pub resume_after: String,
    /// Current backoff duration in seconds.
    pub backoff_secs: u64,
    /// Whether a probe agent has been dispatched.
    pub probe_dispatched: bool,
}

impl ZeroOutputState {
    fn state_path(dir: &Path) -> std::path::PathBuf {
        dir.join("service").join("zero_output_state.json")
    }

    pub fn load(dir: &Path) -> Self {
        let path = Self::state_path(dir);
        if path.exists()
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(state) = serde_json::from_str(&content)
        {
            return state;
        }
        Self::default()
    }

    pub fn save(&self, dir: &Path) {
        let path = Self::state_path(dir);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, content);
        }
    }

    /// Discard the migration-only global timer. Exact route breakers own all
    /// active probe/backoff scheduling.
    pub fn clear_global_backoff(&mut self) {
        self.global_backoff = None;
    }
}

/// Check whether an agent has zero output (no stream events written).
///
/// Returns `Some(age_secs)` if the agent has zero output, has been alive
/// longer than the kill threshold, and has no active child processes.
/// Returns `None` otherwise.
fn check_zero_output(agent: &AgentEntry) -> Option<u64> {
    if !agent.is_alive() {
        return None;
    }

    let output_path = std::path::Path::new(&agent.output_file);
    let agent_dir = output_path.parent()?;

    // Check raw_stream.jsonl size (Claude CLI agents)
    let raw_path = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);
    let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);

    let has_output = (raw_path.exists() && file_has_content(&raw_path))
        || (stream_path.exists() && file_has_content(&stream_path));

    if has_output {
        return None;
    }

    // Agent has zero output — check how old it is
    let started = DateTime::parse_from_rfc3339(&agent.started_at).ok()?;
    let age = (Utc::now() - started.with_timezone(&Utc)).num_seconds();
    if age < 0 {
        return None;
    }

    let age_secs = age as u64;
    if age_secs >= ZERO_OUTPUT_KILL_THRESHOLD.as_secs() {
        // Don't flag as zero-output if agent has active child processes —
        // it may be waiting on a subprocess (e.g., slow model API startup,
        // compilation, or sub-agent initialization)
        if worksgood::service::has_active_children(agent.pid) {
            return None;
        }
        Some(age_secs)
    } else {
        None
    }
}

/// Check if a file exists and has more than 0 bytes of content.
fn file_has_content(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// Run the zero-output detection sweep.
///
/// This should be called from the coordinator tick, after the liveness cleanup.
/// It:
/// 1. Identifies agents with zero output past the threshold
/// 2. Emits typed, persisted planner observations only
/// 3. Leaves ownership, graph state, routing, and retry scheduling untouched
/// 4. Reports aggregate evidence for diagnostics only
pub fn sweep_zero_output_agents(dir: &Path) -> ZeroOutputSweepResult {
    let mut result = ZeroOutputSweepResult::default();

    // Load registry to find alive agents
    let registry = match AgentRegistry::load(dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[zero-output] Failed to load registry: {}", e);
            return result;
        }
    };

    let alive_agents: Vec<&AgentEntry> = registry
        .agents
        .values()
        .filter(|a| a.is_alive() && is_process_alive(a.pid))
        .collect();

    if alive_agents.is_empty() {
        return result;
    }

    // Identify zero-output agents
    let mut zero_output_agents: Vec<(&AgentEntry, u64)> = Vec::new();
    for agent in &alive_agents {
        if let Some(age_secs) = check_zero_output(agent) {
            zero_output_agents.push((agent, age_secs));
        }
    }

    // A zero-output quorum is evidence, not global scheduling authority. Each
    // killed run is classified through its exact spawn route by provider
    // health; the convergence scheduler owns one route-key probe and durable
    // falloff. Never pause unrelated routes here.
    if alive_agents.len() >= GLOBAL_OUTAGE_MIN_AGENTS {
        let zero_ratio = zero_output_agents.len() as f64 / alive_agents.len() as f64;
        if zero_ratio >= GLOBAL_OUTAGE_RATIO {
            result.global_outage_detected = true;
            eprintln!(
                "[zero-output] route-outage evidence: {}/{} agents have zero output ({}%); exact route breakers decide waking",
                zero_output_agents.len(),
                alive_agents.len(),
                (zero_ratio * 100.0) as u32,
            );
        }
    }
    if zero_output_agents.is_empty() {
        return result;
    }

    for (agent, age_secs) in zero_output_agents {
        result.observed.push(ZeroOutputEvidence {
            agent_id: agent.id.clone(),
            task_id: agent.task_id.clone(),
            pid: agent.pid,
            age_secs,
        });
        eprintln!(
            "[zero-output] observed agent {} on task {} after {}s; diagnostic has no ownership action",
            agent.id, agent.task_id, age_secs
        );
    }

    // Do not save legacy counter/backoff state. It remains readable only for
    // one-release migration and has no decision authority.
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_zero_output_state_save_load() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        std::fs::create_dir_all(dir.join("service")).unwrap();

        let mut state = ZeroOutputState::default();
        state.task_respawn_counts.insert("task-1".into(), 1);
        state.save(dir);

        let loaded = ZeroOutputState::load(dir);
        assert_eq!(loaded.task_respawn_counts.get("task-1"), Some(&1));
    }

    #[test]
    fn legacy_global_backoff_is_migration_data_only() {
        let mut state = ZeroOutputState {
            global_backoff: Some(GlobalBackoffState {
                resume_after: (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                backoff_secs: 900,
                probe_dispatched: true,
            }),
            ..ZeroOutputState::default()
        };
        state.clear_global_backoff();
        assert!(state.global_backoff.is_none());
    }

    #[test]
    fn test_file_has_content() {
        let temp = TempDir::new().unwrap();

        let empty_file = temp.path().join("empty.jsonl");
        std::fs::write(&empty_file, "").unwrap();
        assert!(!file_has_content(&empty_file));

        let content_file = temp.path().join("content.jsonl");
        std::fs::write(&content_file, "{\"type\":\"init\"}\n").unwrap();
        assert!(file_has_content(&content_file));

        let missing = temp.path().join("missing.jsonl");
        assert!(!file_has_content(&missing));
    }

    #[test]
    fn test_check_zero_output_dead_agent() {
        let agent = AgentEntry {
            id: "agent-1".into(),
            pid: 99999,
            task_id: "task-1".into(),
            executor: "claude".into(),
            started_at: "2020-01-01T00:00:00Z".into(),
            last_heartbeat: "2020-01-01T00:00:00Z".into(),
            status: AgentStatus::Dead,
            output_file: "/nonexistent/output.log".into(),
            model: None,
            completed_at: None,
            worktree_path: None,
        };
        // Dead agents should be ignored
        assert!(check_zero_output(&agent).is_none());
    }

    #[test]
    fn test_check_zero_output_with_content() {
        let temp = TempDir::new().unwrap();
        let agent_dir = temp.path();

        // Create raw_stream.jsonl with content
        let raw_stream = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);
        std::fs::write(&raw_stream, "{\"type\":\"content\"}\n").unwrap();

        let output_file = agent_dir.join("output.log");
        std::fs::write(&output_file, "").unwrap();

        let agent = AgentEntry {
            id: "agent-1".into(),
            pid: 99999,
            task_id: "task-1".into(),
            executor: "claude".into(),
            started_at: "2020-01-01T00:00:00Z".into(), // Very old
            last_heartbeat: Utc::now().to_rfc3339(),
            status: AgentStatus::Working,
            output_file: output_file.to_str().unwrap().into(),
            model: None,
            completed_at: None,
            worktree_path: None,
        };
        // Has content, so should return None
        assert!(check_zero_output(&agent).is_none());
    }

    #[test]
    fn test_check_zero_output_empty_stream_young() {
        let temp = TempDir::new().unwrap();
        let agent_dir = temp.path();

        // Create empty raw_stream.jsonl
        let raw_stream = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);
        std::fs::write(&raw_stream, "").unwrap();

        let output_file = agent_dir.join("output.log");
        std::fs::write(&output_file, "").unwrap();

        let agent = AgentEntry {
            id: "agent-1".into(),
            pid: 99999,
            task_id: "task-1".into(),
            executor: "claude".into(),
            started_at: Utc::now().to_rfc3339(), // Just started
            last_heartbeat: Utc::now().to_rfc3339(),
            status: AgentStatus::Working,
            output_file: output_file.to_str().unwrap().into(),
            model: None,
            completed_at: None,
            worktree_path: None,
        };
        // Too young, should return None
        assert!(check_zero_output(&agent).is_none());
    }

    #[test]
    fn test_check_zero_output_empty_stream_old() {
        let temp = TempDir::new().unwrap();
        let agent_dir = temp.path();

        // Create empty raw_stream.jsonl
        let raw_stream = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);
        std::fs::write(&raw_stream, "").unwrap();

        let output_file = agent_dir.join("output.log");
        std::fs::write(&output_file, "").unwrap();

        let agent = AgentEntry {
            id: "agent-1".into(),
            pid: 99999,
            task_id: "task-1".into(),
            executor: "claude".into(),
            started_at: "2020-01-01T00:00:00Z".into(), // Very old
            last_heartbeat: Utc::now().to_rfc3339(),
            status: AgentStatus::Working,
            output_file: output_file.to_str().unwrap().into(),
            model: None,
            completed_at: None,
            worktree_path: None,
        };
        // Old with zero output, should return Some
        let result = check_zero_output(&agent);
        assert!(result.is_some());
        assert!(result.unwrap() > ZERO_OUTPUT_KILL_THRESHOLD.as_secs());
    }

    #[test]
    fn test_sweep_empty_registry() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        std::fs::create_dir_all(dir.join("service")).unwrap();

        // Create empty registry
        let registry = AgentRegistry::new();
        registry.save(dir).unwrap();

        let result = sweep_zero_output_agents(dir);
        assert!(result.observed.is_empty());
        assert!(!result.global_outage_detected);
    }

    #[test]
    fn test_zero_output_state_persistence() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        std::fs::create_dir_all(dir.join("service")).unwrap();

        let mut state = ZeroOutputState::default();
        state.task_respawn_counts.insert("task-a".into(), 1);
        state.global_backoff = Some(GlobalBackoffState {
            resume_after: (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            backoff_secs: 60,
            probe_dispatched: false,
        });
        state.save(dir);

        let loaded = ZeroOutputState::load(dir);
        assert_eq!(loaded.task_respawn_counts.get("task-a"), Some(&1));
        assert!(
            loaded.global_backoff.is_some(),
            "legacy bytes still deserialize"
        );
    }
}
