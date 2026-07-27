//! Internal: translate a finished pi agent's NDJSON event stream into the
//! canonical `stream.jsonl` + a `session-summary.md`.
//!
//! Invoked by the spawn wrapper after `pi --mode json` exits. Reads the agent's
//! `raw_stream.jsonl` (pi's native NDJSON, falling back to `output.log`), sums
//! per-turn usage, and writes a canonical stream that carries REAL token/cost
//! figures — not the `usage:{input_tokens:0,output_tokens:0}` bookend the
//! generic wrapper used to emit — so the TUI, `wg show`, `wg spend`, and
//! `wg stats` reflect the pi task. Also writes the agent's `session-summary.md`
//! from the final assistant turn so `wg show <pi task>` isn't bare.
//!
//! Token-cost accounting (`task.token_usage`) is handled independently by
//! `graph::parse_token_usage`, which learned the same pi `turn_end` summation;
//! this command exists to populate the canonical event channel.

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

#[cfg(test)]
use worksgood::stream_event::StreamEvent;
use worksgood::stream_event::{self, StreamWriter};

/// Maximum characters of final assistant text to persist as the session
/// summary — a guard against a pathologically long final message.
const MAX_SUMMARY_CHARS: usize = 4000;

/// Follow the append-only capture while the exact Pi child is alive. Only
/// bounded projections enter watchdog state; raw bytes remain exclusively in
/// raw_stream.jsonl/output.log.
pub fn observe_live(agent_dir: &Path, follow_pid: u32) -> Result<()> {
    let raw_path = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);
    let stream_id = raw_path.to_string_lossy().into_owned();
    let mut cursor = open_watchdog_for_agent(agent_dir)
        .map(|watchdog| watchdog.native_stream_offset(&stream_id))
        .unwrap_or(0);
    let mut dead_stable_rounds = 0u8;
    let mut reconcile_needed = true;
    loop {
        let before = cursor;
        if raw_path.is_file() {
            let mut file = std::fs::File::open(&raw_path)?;
            let length = file.metadata()?.len();
            if length < cursor {
                anyhow::bail!(
                    "Pi native capture shrank from {} to {}; append-only proof failed",
                    cursor,
                    length
                );
            }
            file.seek(SeekFrom::Start(cursor))?;
            let mut reader = BufReader::new(file);
            loop {
                let line_start = cursor;
                let mut line = String::new();
                let read = reader.read_line(&mut line)?;
                if read == 0 {
                    break;
                }
                if !line.ends_with('\n') {
                    cursor = line_start;
                    break;
                }
                cursor = cursor.saturating_add(read as u64);
                if let Some(mut watchdog) = open_watchdog_for_agent(agent_dir) {
                    watchdog
                        .ingest_native_line(
                            line.trim_end_matches(['\r', '\n']),
                            &stream_id,
                            cursor,
                            chrono::Utc::now().timestamp(),
                        )
                        .map_err(anyhow::Error::new)?;
                    if reconcile_needed {
                        match watchdog.reconcile_session_journal(chrono::Utc::now().timestamp()) {
                            Ok(changed) => reconcile_needed = !changed,
                            Err(error) if error.code == "session_journal_missing" => {}
                            Err(error) => {
                                let _ = watchdog.observe(
                                    worksgood::pi_watchdog::Observation::GuardFailure(
                                        worksgood::pi_watchdog::GuardFailure::Session,
                                    ),
                                    chrono::Utc::now().timestamp(),
                                );
                                return Err(anyhow::Error::new(error));
                            }
                        }
                    }
                }
            }
        }
        let alive = process_alive(follow_pid);
        if !alive && cursor == before {
            dead_stable_rounds = dead_stable_rounds.saturating_add(1);
            if dead_stable_rounds >= 4 {
                break;
            }
        } else {
            dead_stable_rounds = 0;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Some(mut watchdog) = open_watchdog_for_agent(agent_dir)
        && let Err(error) = watchdog.reconcile_session_journal(chrono::Utc::now().timestamp())
    {
        let _ = watchdog.observe(
            worksgood::pi_watchdog::Observation::GuardFailure(
                worksgood::pi_watchdog::GuardFailure::Session,
            ),
            chrono::Utc::now().timestamp(),
        );
        return Err(anyhow::Error::new(error));
    }
    Ok(())
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // Signal 0 performs identity-free liveness only; the watchdog separately
    // owns the exact PID/start-ticks proof used for any disposition.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

pub fn run(agent_dir: &Path, exit_code: i32) -> Result<()> {
    let success = exit_code == 0;

    // Prefer pi's native NDJSON capture; fall back to the combined log (which
    // also contains the NDJSON when `raw_stream.jsonl` is absent).
    let raw_path = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);
    let log_path = agent_dir.join("output.log");
    let content = std::fs::read_to_string(&raw_path)
        .or_else(|_| std::fs::read_to_string(&log_path))
        .unwrap_or_default();

    let model_override = read_metadata_model(agent_dir);

    // Project native progress/provider/settled evidence into the durable,
    // process-epoch watchdog journal as well as the canonical UI stream. This
    // is evidence only; the lifecycle kernel remains the sole task-state
    // writer. Replay is idempotent at the watchdog action/receipt layer.
    if let Some(mut watchdog) = open_watchdog_for_agent(agent_dir) {
        let stream_id = raw_path.to_string_lossy();
        let mut offset = 0u64;
        for line in content.split_inclusive('\n') {
            offset = offset.saturating_add(line.len() as u64);
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                continue;
            }
            let at = chrono::Utc::now().timestamp();
            let _ = watchdog.ingest_native_line(line, &stream_id, offset, at);
        }
        watchdog
            .reconcile_session_journal(chrono::Utc::now().timestamp())
            .map_err(anyhow::Error::new)?;
    }

    // Pi is the accounting authority. Missing cost remains zero/unknown; WG
    // never substitutes registry pricing for Pi events.
    let tr = stream_event::translate_pi_stream(&content, model_override.as_deref(), success);

    // Write the canonical stream fresh, overwriting the 0/0 bookend the wrapper
    // may have written on a path where this command did not yet run.
    let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
    std::fs::write(&stream_path, "")
        .with_context(|| format!("truncate {}", stream_path.display()))?;
    let writer = StreamWriter::new(&stream_path);
    for event in &tr.events {
        writer.write_event(event);
    }

    // Session summary so `wg show` isn't bare.
    if let Some(text) = tr.final_text.as_deref() {
        let summary = if text.chars().count() > MAX_SUMMARY_CHARS {
            let cut = text
                .char_indices()
                .nth(MAX_SUMMARY_CHARS)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            format!("{}\n[...truncated]", &text[..cut])
        } else {
            text.to_string()
        };
        let summary_path = agent_dir.join("session-summary.md");
        if let Err(e) =
            worksgood::executor::native::resume::store_session_summary(&summary_path, &summary)
        {
            eprintln!("[pi-stream-bridge] warning: failed to write session summary: {e}");
        }
    }

    Ok(())
}

fn open_watchdog_for_agent(agent_dir: &Path) -> Option<worksgood::pi_watchdog::PiWatchdog> {
    let content = std::fs::read_to_string(agent_dir.join("metadata.json")).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    let attempt = val.get("attempt_id")?.as_str()?;
    let graph_dir = agent_dir.parent()?.parent()?;
    worksgood::pi_watchdog::PiWatchdog::open(
        &graph_dir
            .join("attempts")
            .join(attempt)
            .join("pi/state.json"),
    )
    .ok()
}

fn read_metadata_model(agent_dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(agent_dir.join("metadata.json")).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("model").and_then(|v| v.as_str()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn test_bridge_writes_nonzero_stream_and_summary() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path();
        write(
            agent_dir,
            "metadata.json",
            r#"{"executor":"pi","model":"openrouter:z-ai/glm-5.2"}"#,
        );
        let raw = [
            r#"{"type":"session","id":"sess-1","cwd":"/tmp"}"#,
            r#"{"type":"turn_end","message":{"role":"assistant","provider":"openrouter","model":"z-ai/glm-5.2","content":[{"type":"toolCall","name":"bash"}],"usage":{"input":200,"output":10,"cacheRead":50,"cacheWrite":0,"totalTokens":260,"cost":{"total":0.02}}}}"#,
            r#"{"type":"turn_end","message":{"role":"assistant","provider":"openrouter","model":"z-ai/glm-5.2","content":[{"type":"text","text":"all done, task complete"}],"usage":{"input":5,"output":7,"cacheRead":260,"cacheWrite":0,"totalTokens":272,"cost":{"total":0.03}}}}"#,
        ]
        .join("\n");
        write(agent_dir, "raw_stream.jsonl", &raw);

        run(agent_dir, 0).unwrap();

        // stream.jsonl now carries a NONZERO summed result.usage.
        let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
        let (events, _) = stream_event::read_stream_events(&stream_path, 0).unwrap();
        let result = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::Result { usage, .. } => Some(usage.clone()),
                _ => None,
            })
            .expect("a result event");
        assert_eq!(result.input_tokens, 205);
        assert_eq!(result.output_tokens, 17);
        assert!((result.cost_usd.unwrap() - 0.05).abs() < 1e-9);
        // Per-step events are present between init and result.
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Turn { .. })));

        // Session summary written from the final assistant text.
        let summary = std::fs::read_to_string(agent_dir.join("session-summary.md")).unwrap();
        assert!(summary.contains("all done, task complete"));
    }

    #[test]
    fn test_bridge_handles_missing_stream_gracefully() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path();
        // No raw_stream.jsonl, no output.log.
        run(agent_dir, 1).unwrap();
        let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
        let (events, _) = stream_event::read_stream_events(&stream_path, 0).unwrap();
        // Still emits init + result bookends (result success=false).
        assert!(matches!(events.first(), Some(StreamEvent::Init { .. })));
        assert!(matches!(
            events.last(),
            Some(StreamEvent::Result { success: false, .. })
        ));
    }
}
