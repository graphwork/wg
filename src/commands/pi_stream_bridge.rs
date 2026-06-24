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

use std::path::Path;

use anyhow::{Context, Result};

use worksgood::stream_event::{self, StreamEvent, StreamWriter};

/// Maximum characters of final assistant text to persist as the session
/// summary — a guard against a pathologically long final message.
const MAX_SUMMARY_CHARS: usize = 4000;

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

    let mut tr = stream_event::translate_pi_stream(&content, model_override.as_deref(), success);

    // Cost fallback: when pi reported no cost but we harvested tokens, estimate
    // from the model-registry per-token rates for the resolved model.
    let has_cost = tr.total.cost_usd.is_some_and(|c| c > 0.0);
    if !has_cost && (tr.total.input_tokens > 0 || tr.total.output_tokens > 0) {
        let est = worksgood::graph::estimate_agent_cost_usd(
            &log_path,
            tr.total.input_tokens,
            tr.total.output_tokens,
            tr.total.cache_read_input_tokens.unwrap_or(0),
        );
        if est > 0.0 {
            tr.total.cost_usd = Some(est);
            if let Some(StreamEvent::Result { usage, .. }) = tr.events.last_mut() {
                usage.cost_usd = Some(est);
            }
        }
    }

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
