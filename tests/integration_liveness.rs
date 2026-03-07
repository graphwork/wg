//! Smoke tests for liveness detection: sleep-aware stuck agent handling.
//!
//! Exercises the liveness infrastructure from the library-crate level:
//! - Config defaults for sleep detection thresholds
//! - Stream event staleness tracking per agent
//! - 2-tick stale threshold before triage trigger (simulated)
//! - Grace period preventing false positives after wake
//! - Heartbeat auto-bump removal doesn't break normal operation

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;
use tempfile::TempDir;

use workgraph::config::Config;
use workgraph::stream_event::{self, StreamEvent, StreamWriter};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write a stream event file with events at given timestamps.
fn write_stream_events_at(dir: &Path, events: &[StreamEvent]) {
    let stream_path = dir.join(stream_event::STREAM_FILE_NAME);
    let writer = StreamWriter::new(&stream_path);
    for event in events {
        writer.write_event(event);
    }
}

/// Compute how stale an agent's last stream event is, in milliseconds.
/// Returns None if no stream events exist.
fn compute_staleness_ms(agent_dir: &Path) -> Option<i64> {
    let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
    if !stream_path.exists() {
        return None;
    }
    let (events, _) = stream_event::read_stream_events(&stream_path, 0).ok()?;
    let last_ts = events.last()?.timestamp_ms();
    let now = stream_event::now_ms();
    Some(now - last_ts)
}

/// Check if the last event in a stream is an unmatched ToolStart (in-progress tool).
fn last_in_progress_tool(agent_dir: &Path) -> Option<String> {
    let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
    if !stream_path.exists() {
        return None;
    }
    let (events, _) = stream_event::read_stream_events(&stream_path, 0).ok()?;
    let mut in_progress: Option<String> = None;
    for event in &events {
        match event {
            StreamEvent::ToolStart { name, .. } => {
                in_progress = Some(name.clone());
            }
            StreamEvent::ToolEnd { .. } => {
                in_progress = None;
            }
            _ => {}
        }
    }
    in_progress
}

// ---------------------------------------------------------------------------
// Simulated SleepTracker (mirrors binary crate's SleepTracker logic)
//
// The real SleepTracker lives in the binary crate (src/commands/service/liveness.rs)
// and isn't importable here. We replicate its core logic for integration testing.
// ---------------------------------------------------------------------------

struct SleepTracker {
    last_tick_wall: f64,
    last_tick_mono: f64,
    wake_grace_until: Option<Instant>,
    agent_stale_ticks: HashMap<String, u32>,
    agent_last_event_ms: HashMap<String, i64>,
}

impl SleepTracker {
    fn new() -> Self {
        Self {
            last_tick_wall: wall_secs(),
            last_tick_mono: mono_secs(),
            wake_grace_until: None,
            agent_stale_ticks: HashMap::new(),
            agent_last_event_ms: HashMap::new(),
        }
    }

    /// Simulate a coordinator tick. Returns detected sleep gap in seconds.
    fn tick(&mut self, config: &Config) -> f64 {
        let now_wall = wall_secs();
        let now_mono = mono_secs();

        let wall_elapsed = now_wall - self.last_tick_wall;
        let mono_elapsed = now_mono - self.last_tick_mono;
        let sleep_gap = wall_elapsed - mono_elapsed;

        let threshold = config.agent.sleep_gap_threshold.unwrap_or(30) as f64;

        if sleep_gap > threshold {
            let grace_secs = config.agent.wake_grace_period.unwrap_or(120);
            self.wake_grace_until =
                Some(Instant::now() + std::time::Duration::from_secs(grace_secs));
            self.agent_stale_ticks.clear();
        }

        self.last_tick_wall = now_wall;
        self.last_tick_mono = now_mono;

        sleep_gap.max(0.0)
    }

    fn in_grace_period(&self) -> bool {
        self.wake_grace_until
            .map(|deadline| Instant::now() < deadline)
            .unwrap_or(false)
    }

    fn prune_dead_agents(&mut self, alive_ids: &[&str]) {
        self.agent_stale_ticks
            .retain(|id, _| alive_ids.contains(&id.as_str()));
        self.agent_last_event_ms
            .retain(|id, _| alive_ids.contains(&id.as_str()));
    }
}

fn wall_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(unix)]
fn mono_secs() -> f64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1_000_000_000.0
}

#[cfg(not(unix))]
fn mono_secs() -> f64 {
    wall_secs()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. Clock drift > 30s is detected as a sleep event.
#[test]
fn test_smoke_liveness_clock_drift_detection() {
    let mut tracker = SleepTracker::new();
    let config = Config::default();

    // Verify default sleep gap threshold is 30s
    assert_eq!(
        config.agent.sleep_gap_threshold,
        None,
        "Default should be None (uses 30s fallback)"
    );

    // Immediate tick — no sleep gap
    let gap = tracker.tick(&config);
    assert!(gap < 1.0, "No sleep gap expected, got {:.1}s", gap);
    assert!(!tracker.in_grace_period());

    // Simulate sleep: push last_tick_wall back by 60s while mono stays current.
    // This mimics what happens when the system sleeps — wall clock advances
    // but CLOCK_MONOTONIC pauses.
    tracker.last_tick_wall = wall_secs() - 60.0;

    let gap = tracker.tick(&config);
    assert!(
        gap > 50.0,
        "Expected large sleep gap (~60s), got {:.1}s",
        gap
    );
    assert!(
        tracker.in_grace_period(),
        "Should enter grace period after sleep detection"
    );
}

/// 2. Stream staleness is tracked per agent via stream event timestamps.
#[test]
fn test_smoke_liveness_stream_staleness_per_agent() {
    let tmp = TempDir::new().unwrap();

    // Agent A: recent events (fresh)
    let agent_a_dir = tmp.path().join("agent-a");
    fs::create_dir_all(&agent_a_dir).unwrap();
    let now_ms = stream_event::now_ms();
    write_stream_events_at(
        &agent_a_dir,
        &[
            StreamEvent::Init {
                executor_type: "shell".to_string(),
                model: None,
                session_id: None,
                timestamp_ms: now_ms - 5_000, // 5s ago
            },
            StreamEvent::Turn {
                turn_number: 1,
                tools_used: vec!["Bash".to_string()],
                usage: None,
                timestamp_ms: now_ms - 1_000, // 1s ago
            },
        ],
    );

    // Agent B: old events (stale — 15 minutes ago)
    let agent_b_dir = tmp.path().join("agent-b");
    fs::create_dir_all(&agent_b_dir).unwrap();
    write_stream_events_at(
        &agent_b_dir,
        &[StreamEvent::Init {
            executor_type: "shell".to_string(),
            model: None,
            session_id: None,
            timestamp_ms: now_ms - 15 * 60 * 1000, // 15 min ago
        }],
    );

    // Agent C: no stream file at all
    let agent_c_dir = tmp.path().join("agent-c");
    fs::create_dir_all(&agent_c_dir).unwrap();

    // Check staleness
    let stale_a = compute_staleness_ms(&agent_a_dir).unwrap();
    let stale_b = compute_staleness_ms(&agent_b_dir).unwrap();
    let stale_c = compute_staleness_ms(&agent_c_dir);

    // Agent A should be fresh (< 10s stale)
    assert!(
        stale_a < 10_000,
        "Agent A should be fresh, stale_ms={}",
        stale_a
    );

    // Agent B should be stale (>= 10 min)
    let stale_threshold_ms = 10 * 60 * 1000;
    assert!(
        stale_b >= stale_threshold_ms,
        "Agent B should be stale (>10min), stale_ms={}",
        stale_b
    );

    // Agent C has no stream events
    assert!(stale_c.is_none(), "Agent C should have no staleness data");
}

/// 3. Two consecutive stale ticks required before triage trigger.
#[test]
fn test_smoke_liveness_two_tick_threshold() {
    let config = Config::default();

    // Verify default stale_tick_threshold is 2
    assert_eq!(
        config.agent.stale_tick_threshold,
        None,
        "Default should be None (uses 2 fallback)"
    );
    let tick_threshold = config.agent.stale_tick_threshold.unwrap_or(2);
    assert_eq!(tick_threshold, 2);

    // Simulate the staleness tracking logic from check_stuck_agents
    let mut tracker = SleepTracker::new();
    let agent_id = "agent-test".to_string();
    let stale_threshold_ms: i64 = (config.agent.stale_threshold.unwrap_or(10) as i64) * 60 * 1000;

    // Simulate an agent whose last event is old (20 minutes ago)
    let old_event_ms = stream_event::now_ms() - 20 * 60 * 1000;
    tracker
        .agent_last_event_ms
        .insert(agent_id.clone(), old_event_ms);

    // Tick 1: agent is stale, but only 1 tick — should NOT trigger triage
    let stale_ms = stream_event::now_ms() - old_event_ms;
    assert!(stale_ms > stale_threshold_ms);

    // Increment stale counter (mimics check_stuck_agents logic)
    let count = tracker
        .agent_stale_ticks
        .entry(agent_id.clone())
        .or_insert(0);
    *count += 1;
    assert_eq!(*count, 1);
    assert!(
        *count < tick_threshold,
        "Should NOT trigger triage on first stale tick"
    );

    // Tick 2: still stale, now at 2 ticks — SHOULD trigger triage
    let count = tracker
        .agent_stale_ticks
        .entry(agent_id.clone())
        .or_insert(0);
    *count += 1;
    assert_eq!(*count, 2);
    assert!(
        *count >= tick_threshold,
        "Should trigger triage on second stale tick"
    );
}

/// 4. Stuck triage verdicts are valid JSON: wait, kill-done, kill-restart.
#[test]
fn test_smoke_liveness_triage_verdicts() {
    // Verify that verdict JSON can be parsed correctly for each verdict type.
    // This tests the contract between the triage LLM prompt and the coordinator.
    let verdicts = [
        (
            r#"{"verdict": "wait", "reason": "agent is building a large project"}"#,
            "wait",
        ),
        (
            r#"{"verdict": "kill-done", "reason": "agent finished but hung on exit"}"#,
            "kill-done",
        ),
        (
            r#"{"verdict": "kill-restart", "reason": "agent stuck in infinite loop"}"#,
            "kill-restart",
        ),
    ];

    for (json, expected_verdict) in &verdicts {
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(parsed["verdict"].as_str().unwrap(), *expected_verdict);
        assert!(
            !parsed["reason"].as_str().unwrap().is_empty(),
            "Reason should be present for verdict '{}'",
            expected_verdict
        );
    }

    // Verify JSON extraction from noisy LLM output (fenced blocks, surrounding text)
    let noisy_outputs = [
        // Plain JSON
        r#"{"verdict": "wait", "reason": "still working"}"#,
        // With markdown fences
        "```json\n{\"verdict\": \"kill-done\", \"reason\": \"done\"}\n```",
        // With surrounding text
        "Analysis:\n{\"verdict\": \"kill-restart\", \"reason\": \"stuck\"}\nEnd.",
    ];

    for raw in &noisy_outputs {
        let trimmed = raw.trim();
        // Try plain parse
        let json = if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            trimmed.to_string()
        } else if trimmed.starts_with("```") {
            // Strip fences
            trimmed
                .trim_start_matches("```json")
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim()
                .to_string()
        } else if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
            trimmed[start..=end].to_string()
        } else {
            panic!("Could not extract JSON from: {}", raw);
        };

        let parsed: serde_json::Value = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("Failed to parse extracted JSON '{}': {}", json, e));
        assert!(
            ["wait", "kill-done", "kill-restart"]
                .contains(&parsed["verdict"].as_str().unwrap_or("")),
            "Invalid verdict in: {}",
            json
        );
    }
}

/// 5. Grace period prevents false positives after wake-up.
#[test]
fn test_smoke_liveness_grace_period_prevents_false_positives() {
    let mut tracker = SleepTracker::new();
    let config = Config::default();

    // Verify default grace period is 120s
    assert_eq!(
        config.agent.wake_grace_period,
        None,
        "Default should be None (uses 120s fallback)"
    );

    // Not in grace period initially
    assert!(!tracker.in_grace_period());

    // Simulate sleep detection → enters grace period
    tracker.last_tick_wall = wall_secs() - 60.0;
    tracker.tick(&config);
    assert!(
        tracker.in_grace_period(),
        "Should be in grace period after sleep"
    );

    // During grace period, stale agents should NOT be triaged
    tracker
        .agent_stale_ticks
        .insert("agent-1".to_string(), 5);
    // Simulate a sleep detection that clears stale ticks
    tracker.last_tick_wall = wall_secs() - 60.0;
    tracker.tick(&config);
    assert!(
        tracker.agent_stale_ticks.is_empty(),
        "Sleep detection should clear stale tick counters"
    );

    // Expired grace period
    tracker.wake_grace_until =
        Some(Instant::now() - std::time::Duration::from_secs(1));
    assert!(
        !tracker.in_grace_period(),
        "Should NOT be in grace period after expiry"
    );
}

/// 6. Normal agent operation is unaffected by heartbeat removal.
///    Stream events are the ground truth for liveness — heartbeats
///    are informational only and should not be required for detection.
#[test]
fn test_smoke_liveness_heartbeat_removal_no_impact() {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent-hb");
    fs::create_dir_all(&agent_dir).unwrap();

    let now_ms = stream_event::now_ms();

    // Agent with normal stream events but NO heartbeat events
    write_stream_events_at(
        &agent_dir,
        &[
            StreamEvent::Init {
                executor_type: "claude".to_string(),
                model: Some("opus".to_string()),
                session_id: Some("session-1".to_string()),
                timestamp_ms: now_ms - 30_000,
            },
            StreamEvent::Turn {
                turn_number: 1,
                tools_used: vec!["Read".to_string()],
                usage: None,
                timestamp_ms: now_ms - 20_000,
            },
            StreamEvent::ToolStart {
                name: "Bash".to_string(),
                detail: None,
                timestamp_ms: now_ms - 10_000,
            },
            StreamEvent::ToolEnd {
                name: "Bash".to_string(),
                is_error: false,
                duration_ms: 5000,
                output_summary: None,
                timestamp_ms: now_ms - 5_000,
            },
            StreamEvent::Turn {
                turn_number: 2,
                tools_used: vec!["Bash".to_string()],
                usage: None,
                timestamp_ms: now_ms - 1_000,
            },
        ],
    );

    // Staleness should be computed from the last stream event, not heartbeat
    let staleness = compute_staleness_ms(&agent_dir).unwrap();
    assert!(
        staleness < 10_000,
        "Agent with recent stream events (no heartbeats) should be fresh, stale_ms={}",
        staleness
    );

    // No in-progress tool (last ToolStart has matching ToolEnd)
    assert!(
        last_in_progress_tool(&agent_dir).is_none(),
        "No in-progress tool expected"
    );

    // Agent with heartbeat events — same result
    let agent_hb_dir = tmp.path().join("agent-with-hb");
    fs::create_dir_all(&agent_hb_dir).unwrap();
    write_stream_events_at(
        &agent_hb_dir,
        &[
            StreamEvent::Init {
                executor_type: "claude".to_string(),
                model: None,
                session_id: None,
                timestamp_ms: now_ms - 30_000,
            },
            StreamEvent::Heartbeat {
                timestamp_ms: now_ms - 15_000,
            },
            StreamEvent::Turn {
                turn_number: 1,
                tools_used: vec![],
                usage: None,
                timestamp_ms: now_ms - 2_000,
            },
        ],
    );

    let staleness_hb = compute_staleness_ms(&agent_hb_dir).unwrap();
    assert!(
        staleness_hb < 10_000,
        "Agent with heartbeats should also be fresh, stale_ms={}",
        staleness_hb
    );
}

/// In-progress tool detection: an unmatched ToolStart extends the stale window.
#[test]
fn test_smoke_liveness_in_progress_tool_detection() {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent-tool");
    fs::create_dir_all(&agent_dir).unwrap();

    let now_ms = stream_event::now_ms();

    // Agent has a ToolStart without a matching ToolEnd — tool is in progress
    write_stream_events_at(
        &agent_dir,
        &[
            StreamEvent::Init {
                executor_type: "shell".to_string(),
                model: None,
                session_id: None,
                timestamp_ms: now_ms - 60_000,
            },
            StreamEvent::ToolStart {
                name: "Bash".to_string(),
                detail: None,
                timestamp_ms: now_ms - 30_000,
            },
            // No ToolEnd — tool still running
        ],
    );

    let tool = last_in_progress_tool(&agent_dir);
    assert_eq!(
        tool.as_deref(),
        Some("Bash"),
        "Should detect in-progress Bash tool"
    );

    // Now add a ToolEnd — tool is complete
    let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
    let writer = StreamWriter::new(&stream_path);
    writer.write_event(&StreamEvent::ToolEnd {
        name: "Bash".to_string(),
        is_error: false,
        duration_ms: 25_000,
        output_summary: None,
        timestamp_ms: now_ms - 5_000,
    });

    let tool = last_in_progress_tool(&agent_dir);
    assert!(
        tool.is_none(),
        "Should have no in-progress tool after ToolEnd"
    );
}

/// Config defaults for liveness fields are sensible.
#[test]
fn test_smoke_liveness_config_defaults() {
    let config = Config::default();

    // All liveness fields should be None (meaning use fallback defaults)
    assert!(config.agent.stale_threshold.is_none());
    assert!(config.agent.wake_grace_period.is_none());
    assert!(config.agent.sleep_gap_threshold.is_none());
    assert!(config.agent.stale_tick_threshold.is_none());

    // Verify the fallback values used in the liveness code
    assert_eq!(config.agent.stale_threshold.unwrap_or(10), 10); // 10 minutes
    assert_eq!(config.agent.wake_grace_period.unwrap_or(120), 120); // 120 seconds
    assert_eq!(config.agent.sleep_gap_threshold.unwrap_or(30), 30); // 30 seconds
    assert_eq!(config.agent.stale_tick_threshold.unwrap_or(2), 2); // 2 ticks
}

/// SleepTracker prunes tracking state for dead agents.
#[test]
fn test_smoke_liveness_prune_dead_agents() {
    let mut tracker = SleepTracker::new();

    tracker
        .agent_stale_ticks
        .insert("alive-1".to_string(), 1);
    tracker
        .agent_stale_ticks
        .insert("dead-1".to_string(), 3);
    tracker
        .agent_last_event_ms
        .insert("alive-1".to_string(), 100);
    tracker
        .agent_last_event_ms
        .insert("dead-1".to_string(), 200);
    tracker
        .agent_last_event_ms
        .insert("dead-2".to_string(), 300);

    tracker.prune_dead_agents(&["alive-1"]);

    assert!(tracker.agent_stale_ticks.contains_key("alive-1"));
    assert!(!tracker.agent_stale_ticks.contains_key("dead-1"));
    assert!(tracker.agent_last_event_ms.contains_key("alive-1"));
    assert!(!tracker.agent_last_event_ms.contains_key("dead-1"));
    assert!(!tracker.agent_last_event_ms.contains_key("dead-2"));
}

/// New stream events reset the stale counter.
#[test]
fn test_smoke_liveness_new_events_reset_stale_counter() {
    let mut tracker = SleepTracker::new();
    let agent_id = "agent-active".to_string();

    // Agent has been stale for 1 tick
    tracker.agent_stale_ticks.insert(agent_id.clone(), 1);
    tracker
        .agent_last_event_ms
        .insert(agent_id.clone(), 1000);

    // New event arrives (timestamp > previous)
    let new_event_ts = 2000_i64;
    let prev = tracker.agent_last_event_ms.get(&agent_id).copied();
    tracker
        .agent_last_event_ms
        .insert(agent_id.clone(), new_event_ts);

    if prev.map(|p| new_event_ts > p).unwrap_or(false) {
        // Reset stale counter — mirrors check_stuck_agents logic
        tracker.agent_stale_ticks.remove(&agent_id);
    }

    assert!(
        !tracker.agent_stale_ticks.contains_key(&agent_id),
        "Stale counter should be reset when new events arrive"
    );
}

/// Stream event reading works correctly for staleness computation.
#[test]
fn test_smoke_liveness_stream_event_round_trip() {
    let tmp = TempDir::new().unwrap();
    let stream_path = tmp.path().join(stream_event::STREAM_FILE_NAME);

    let writer = StreamWriter::new(&stream_path);
    let now_ms = stream_event::now_ms();

    // Write a sequence of events
    writer.write_event(&StreamEvent::Init {
        executor_type: "test".to_string(),
        model: None,
        session_id: Some("s1".to_string()),
        timestamp_ms: now_ms - 10_000,
    });
    writer.write_event(&StreamEvent::Turn {
        turn_number: 1,
        tools_used: vec!["Read".to_string()],
        usage: None,
        timestamp_ms: now_ms - 5_000,
    });
    writer.write_event(&StreamEvent::Heartbeat {
        timestamp_ms: now_ms - 2_000,
    });

    // Read them back
    let (events, _offset) = stream_event::read_stream_events(&stream_path, 0).unwrap();
    assert_eq!(events.len(), 3);

    // Last event should have the most recent timestamp
    let last_ts = events.last().unwrap().timestamp_ms();
    assert_eq!(last_ts, now_ms - 2_000);

    // Staleness should be about 2 seconds
    let staleness = now_ms - last_ts;
    assert!(
        staleness >= 1_000 && staleness < 10_000,
        "Staleness should be ~2s, got {}ms",
        staleness
    );
}
