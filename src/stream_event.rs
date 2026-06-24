//! Unified stream event format for all executor types.
//!
//! All executors produce NDJSON events to `<agent_dir>/stream.jsonl`.
//! The coordinator reads these files for liveness detection, cost tracking,
//! and progress monitoring.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Unified stream event emitted by all executor types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// First event — session/run metadata.
    Init {
        executor_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        timestamp_ms: i64,
    },
    /// Agent completed one turn of the tool-use loop.
    Turn {
        turn_number: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools_used: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TurnUsage>,
        timestamp_ms: i64,
    },
    /// Tool execution started.
    ToolStart { name: String, timestamp_ms: i64 },
    /// Tool execution completed.
    ToolEnd {
        name: String,
        is_error: bool,
        duration_ms: u64,
        timestamp_ms: i64,
    },
    /// A chunk of streaming output from a tool (e.g., bash stdout/stderr line).
    ToolOutputChunk {
        tool: String,
        text: String,
        timestamp_ms: i64,
    },
    /// Periodic heartbeat.
    Heartbeat { timestamp_ms: i64 },
    /// A text chunk from real-time streaming output.
    TextChunk { text: String, timestamp_ms: i64 },
    /// A thinking/reasoning chunk from real-time streaming output.
    ThinkingChunk { text: String, timestamp_ms: i64 },
    /// Final event — aggregated usage and outcome.
    Result {
        success: bool,
        usage: TotalUsage,
        timestamp_ms: i64,
    },
}

/// Token usage for a single turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// Reasoning/thinking tokens consumed (subset of output_tokens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

/// Aggregated token usage for an entire run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TotalUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl StreamEvent {
    /// Get the timestamp_ms from any event variant.
    pub fn timestamp_ms(&self) -> i64 {
        match self {
            StreamEvent::Init { timestamp_ms, .. }
            | StreamEvent::Turn { timestamp_ms, .. }
            | StreamEvent::ToolStart { timestamp_ms, .. }
            | StreamEvent::ToolEnd { timestamp_ms, .. }
            | StreamEvent::ToolOutputChunk { timestamp_ms, .. }
            | StreamEvent::Heartbeat { timestamp_ms }
            | StreamEvent::TextChunk { timestamp_ms, .. }
            | StreamEvent::ThinkingChunk { timestamp_ms, .. }
            | StreamEvent::Result { timestamp_ms, .. } => *timestamp_ms,
        }
    }
}

/// Current millisecond timestamp.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── NDJSON file reading ─────────────────────────────────────────────────

/// Read stream events from an NDJSON file, starting at `offset` bytes.
///
/// Returns the parsed events and the new file offset for incremental reads.
/// Lines that fail to parse are silently skipped (partial writes, etc.).
pub fn read_stream_events(path: &Path, offset: u64) -> Result<(Vec<StreamEvent>, u64)> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let end = file.metadata()?.len();

    if offset >= end {
        return Ok((Vec::new(), offset));
    }

    file.seek(SeekFrom::Start(offset))?;
    let reader = BufReader::new(&file);

    let mut events = Vec::new();
    let mut new_offset = offset;

    for line in reader.lines() {
        let line = line?;
        new_offset += line.len() as u64 + 1; // +1 for newline
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<StreamEvent>(line) {
            events.push(event);
        }
    }

    Ok((events, new_offset))
}

// ── NDJSON file writing ─────────────────────────────────────────────────

/// Writer that appends StreamEvent records as NDJSON to a file.
#[derive(Clone)]
pub struct StreamWriter {
    path: std::path::PathBuf,
}

impl StreamWriter {
    /// Create a new stream writer for the given file path.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Write a single event to the stream file.
    pub fn write_event(&self, event: &StreamEvent) {
        if let Ok(json) = serde_json::to_string(event)
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        {
            let _ = writeln!(file, "{}", json);
        }
    }

    /// Write the Init event.
    pub fn write_init(&self, executor_type: &str, model: Option<&str>, session_id: Option<&str>) {
        self.write_event(&StreamEvent::Init {
            executor_type: executor_type.to_string(),
            model: model.map(String::from),
            session_id: session_id.map(String::from),
            timestamp_ms: now_ms(),
        });
    }

    /// Write a Turn event.
    pub fn write_turn(&self, turn_number: u32, tools_used: Vec<String>, usage: Option<TurnUsage>) {
        self.write_event(&StreamEvent::Turn {
            turn_number,
            tools_used,
            usage,
            timestamp_ms: now_ms(),
        });
    }

    /// Write a ToolStart event.
    pub fn write_tool_start(&self, name: &str) {
        self.write_event(&StreamEvent::ToolStart {
            name: name.to_string(),
            timestamp_ms: now_ms(),
        });
    }

    /// Write a ToolEnd event.
    pub fn write_tool_end(&self, name: &str, is_error: bool, duration_ms: u64) {
        self.write_event(&StreamEvent::ToolEnd {
            name: name.to_string(),
            is_error,
            duration_ms,
            timestamp_ms: now_ms(),
        });
    }

    /// Write a ToolOutputChunk event for streaming tool output (e.g., bash lines).
    pub fn write_tool_output_chunk(&self, tool: &str, text: &str) {
        self.write_event(&StreamEvent::ToolOutputChunk {
            tool: tool.to_string(),
            text: text.to_string(),
            timestamp_ms: now_ms(),
        });
    }

    /// Write a Heartbeat event.
    pub fn write_heartbeat(&self) {
        self.write_event(&StreamEvent::Heartbeat {
            timestamp_ms: now_ms(),
        });
    }

    /// Write a TextChunk event for a streaming text delta.
    pub fn write_text_chunk(&self, text: &str) {
        self.write_event(&StreamEvent::TextChunk {
            text: text.to_string(),
            timestamp_ms: now_ms(),
        });
    }

    /// Write a ThinkingChunk event for a streaming reasoning/thinking delta.
    pub fn write_thinking_chunk(&self, text: &str) {
        self.write_event(&StreamEvent::ThinkingChunk {
            text: text.to_string(),
            timestamp_ms: now_ms(),
        });
    }

    /// Write the final Result event.
    pub fn write_result(&self, success: bool, usage: TotalUsage) {
        self.write_event(&StreamEvent::Result {
            success,
            usage,
            timestamp_ms: now_ms(),
        });
    }

    /// Get the path to the stream file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ── Claude CLI JSONL translation ────────────────────────────────────────

/// Translate a Claude CLI raw JSONL line into a StreamEvent.
///
/// Claude CLI emits events like `{"type":"assistant","message":{...,"usage":{...}}}`,
/// `{"type":"result","total_cost_usd":...,"usage":{...}}`, etc.
/// We translate the ones we care about into our unified format.
pub fn translate_claude_event(line: &str) -> Option<StreamEvent> {
    let val: serde_json::Value = serde_json::from_str(line).ok()?;
    let event_type = val.get("type")?.as_str()?;

    match event_type {
        "system" => {
            // Init-like event — extract session info
            let session_id = val
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(String::from);
            let model = val.get("model").and_then(|v| v.as_str()).map(String::from);
            Some(StreamEvent::Init {
                executor_type: "claude".to_string(),
                model,
                session_id,
                timestamp_ms: now_ms(),
            })
        }
        "assistant" => {
            // Turn completed — extract usage from message.usage
            let usage = val.get("message").and_then(|m| m.get("usage"));
            let turn_usage = usage.map(|u| TurnUsage {
                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                cache_read_input_tokens: u
                    .get("cache_read_input_tokens")
                    .or_else(|| u.get("cacheReadInputTokens"))
                    .and_then(|v| v.as_u64()),
                cache_creation_input_tokens: u
                    .get("cache_creation_input_tokens")
                    .or_else(|| u.get("cacheCreationInputTokens"))
                    .and_then(|v| v.as_u64()),
                reasoning_tokens: None,
            });

            // Extract tool names from content blocks
            let tools_used: Vec<String> = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                        .filter_map(|b| b.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            Some(StreamEvent::Turn {
                turn_number: 0, // Claude CLI doesn't number turns
                tools_used,
                usage: turn_usage,
                timestamp_ms: now_ms(),
            })
        }
        "result" => {
            // Final result — extract total usage and cost
            let usage = val.get("usage");
            let cost = val.get("total_cost_usd").and_then(|v| v.as_f64());

            let total_usage = TotalUsage {
                input_tokens: usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_read_input_tokens: usage
                    .and_then(|u| {
                        u.get("cache_read_input_tokens")
                            .or_else(|| u.get("cacheReadInputTokens"))
                    })
                    .and_then(|v| v.as_u64()),
                cache_creation_input_tokens: usage
                    .and_then(|u| {
                        u.get("cache_creation_input_tokens")
                            .or_else(|| u.get("cacheCreationInputTokens"))
                    })
                    .and_then(|v| v.as_u64()),
                cost_usd: cost,
                model: None,
            };

            Some(StreamEvent::Result {
                success: true,
                usage: total_usage,
                timestamp_ms: now_ms(),
            })
        }
        _ => None,
    }
}

/// Translate a file of raw Claude CLI JSONL into StreamEvents.
///
/// Reads `raw_stream.jsonl` from `offset`, translates each line, and returns
/// the StreamEvents plus the new offset.
pub fn translate_claude_stream(path: &Path, offset: u64) -> Result<(Vec<StreamEvent>, u64)> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let end = file.metadata()?.len();

    if offset >= end {
        return Ok((Vec::new(), offset));
    }

    file.seek(SeekFrom::Start(offset))?;
    let reader = BufReader::new(&file);

    let mut events = Vec::new();
    let mut new_offset = offset;

    for line in reader.lines() {
        let line = line?;
        new_offset += line.len() as u64 + 1;
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        if let Some(event) = translate_claude_event(line) {
            events.push(event);
        }
    }

    Ok((events, new_offset))
}

// ── Pi CLI JSON-mode translation ────────────────────────────────────────

/// Map a pi `usage` object to canonical [`TurnUsage`].
///
/// Pi's usage schema is `{input, output, cacheRead, cacheWrite, totalTokens,
/// cost}` — different field names from Claude's `input_tokens`/`output_tokens`.
/// A naive read of the canonical names finds nothing and yields 0/0, which is
/// exactly the "pi shows zero tokens" bug; map the fields EXPLICITLY here.
///
/// Pi's `input` already EXCLUDES the cache-read portion
/// (`input + cacheRead + output == totalTokens`), so the fields map straight
/// across with no subtraction (unlike codex, whose `input_tokens` is inclusive).
pub fn pi_usage_to_turn(usage: &serde_json::Value) -> TurnUsage {
    let u = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let cache_read = u("cacheRead");
    let cache_write = u("cacheWrite");
    TurnUsage {
        input_tokens: u("input"),
        output_tokens: u("output"),
        cache_read_input_tokens: (cache_read > 0).then_some(cache_read),
        cache_creation_input_tokens: (cache_write > 0).then_some(cache_write),
        reasoning_tokens: None,
    }
}

/// Per-turn cost in USD from a pi `usage` object (`usage.cost.total`).
/// Returns 0.0 when the provider did not report a cost (caller may then fall
/// back to model-registry per-token rates).
pub fn pi_usage_cost(usage: &serde_json::Value) -> f64 {
    usage
        .get("cost")
        .and_then(|c| c.get("total"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

/// Outcome of translating a pi NDJSON event stream into canonical events.
pub struct PiTranslation {
    /// Canonical stream events in order: `Init`, per-step events, final `Result`.
    pub events: Vec<StreamEvent>,
    /// Summed usage across all turns (cost from pi's own per-turn `cost.total`;
    /// callers apply a registry fallback when it is zero).
    pub total: TotalUsage,
    /// Number of `turn_end` events summed (the turn count).
    pub turn_count: u32,
    /// Final assistant text — used for the agent's `session-summary.md`.
    pub final_text: Option<String>,
}

/// Translate a pi `--mode json` NDJSON event stream into canonical
/// [`StreamEvent`]s.
///
/// Pi emits the SAME per-turn usage snapshot on `message_update` (many per
/// turn), `message_end`, AND `turn_end`. To avoid double/triple-counting we
/// harvest usage from `turn_end` ONLY — once per turn — so the summed total
/// equals the sum of per-turn `turn_end.message.usage.totalTokens`. Per-step
/// events (tool start/end, assistant text/thinking) are forwarded so the TUI
/// events pane and `wg log` are populated between `init` and `result`.
///
/// `success` sets the final `Result.success`. `model_override` (e.g. from the
/// agent's `metadata.json`) wins for the `Init`/`Result` model; otherwise the
/// model is derived from the stream's `provider`/`model` fields.
pub fn translate_pi_stream(
    content: &str,
    model_override: Option<&str>,
    success: bool,
) -> PiTranslation {
    let mut steps: Vec<StreamEvent> = Vec::new();
    let mut total = TotalUsage::default();
    let mut cost = 0.0f64;
    let mut turn_count = 0u32;
    let mut session_id: Option<String> = None;
    let mut stream_model: Option<String> = None;
    let mut final_text: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match val.get("type").and_then(|v| v.as_str()) {
            Some("session") => {
                if let Some(id) = val.get("id").and_then(|v| v.as_str()) {
                    session_id = Some(id.to_string());
                }
            }
            Some("tool_execution_start") => {
                let name = val
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                steps.push(StreamEvent::ToolStart {
                    name: name.to_string(),
                    timestamp_ms: now_ms(),
                });
            }
            Some("tool_execution_end") => {
                let name = val
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                let is_error = val
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                steps.push(StreamEvent::ToolEnd {
                    name: name.to_string(),
                    is_error,
                    duration_ms: 0,
                    timestamp_ms: now_ms(),
                });
            }
            Some("turn_end") => {
                turn_count += 1;
                let msg = val.get("message");

                // Derive the canonical model spec from the assistant message
                // (`provider`/`model`) the first time we see it.
                if stream_model.is_none()
                    && let Some(m) = msg
                    && let Some(mdl) = m.get("model").and_then(|v| v.as_str())
                {
                    let prov = m.get("provider").and_then(|v| v.as_str());
                    stream_model = Some(match prov {
                        Some(p) if !mdl.contains(':') => format!("{}:{}", p, mdl),
                        _ => mdl.to_string(),
                    });
                }

                // Forward assistant text / thinking content as per-step events.
                if let Some(content_arr) = msg
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content_arr {
                        match block.get("type").and_then(|v| v.as_str()) {
                            Some("text") => {
                                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                    let t = t.trim();
                                    if !t.is_empty() {
                                        steps.push(StreamEvent::TextChunk {
                                            text: t.to_string(),
                                            timestamp_ms: now_ms(),
                                        });
                                        final_text = Some(t.to_string());
                                    }
                                }
                            }
                            Some("thinking") => {
                                if let Some(t) = block.get("thinking").and_then(|v| v.as_str()) {
                                    let t = t.trim();
                                    if !t.is_empty() {
                                        steps.push(StreamEvent::ThinkingChunk {
                                            text: t.to_string(),
                                            timestamp_ms: now_ms(),
                                        });
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                let tools_used: Vec<String> = msg
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("toolCall"))
                            .filter_map(|b| {
                                b.get("name").and_then(|n| n.as_str()).map(String::from)
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                // Harvest usage from `turn_end` ONLY (the dedup point).
                let usage_val = msg.and_then(|m| m.get("usage"));
                let turn_usage = usage_val.map(pi_usage_to_turn);
                if let Some(u) = usage_val {
                    let tu = pi_usage_to_turn(u);
                    total.input_tokens += tu.input_tokens;
                    total.output_tokens += tu.output_tokens;
                    if let Some(cr) = tu.cache_read_input_tokens {
                        *total.cache_read_input_tokens.get_or_insert(0) += cr;
                    }
                    if let Some(cc) = tu.cache_creation_input_tokens {
                        *total.cache_creation_input_tokens.get_or_insert(0) += cc;
                    }
                    cost += pi_usage_cost(u);
                }

                steps.push(StreamEvent::Turn {
                    turn_number: turn_count,
                    tools_used,
                    usage: turn_usage,
                    timestamp_ms: now_ms(),
                });
            }
            _ => {}
        }
    }

    let model = model_override.map(String::from).or(stream_model);
    if cost > 0.0 {
        total.cost_usd = Some(cost);
    }
    total.model = model.clone();

    let mut events = Vec::with_capacity(steps.len() + 2);
    events.push(StreamEvent::Init {
        executor_type: "pi".to_string(),
        model: model.clone(),
        session_id,
        timestamp_ms: now_ms(),
    });
    events.append(&mut steps);
    events.push(StreamEvent::Result {
        success,
        usage: total.clone(),
        timestamp_ms: now_ms(),
    });

    PiTranslation {
        events,
        total,
        turn_count,
        final_text,
    }
}

// ── Liveness detection ──────────────────────────────────────────────────

/// Stream state tracked per agent by the coordinator.
#[derive(Debug, Clone, Default)]
pub struct AgentStreamState {
    /// Byte offset into the stream file (for incremental reads).
    pub offset: u64,
    /// Timestamp (ms) of the last event seen.
    pub last_event_ms: Option<i64>,
    /// Number of turns observed.
    pub turn_count: u32,
    /// Current tool being executed (if any).
    pub current_tool: Option<String>,
    /// Accumulated usage across all turns.
    pub accumulated_usage: TotalUsage,
}

impl AgentStreamState {
    /// Update state with a batch of new events.
    pub fn ingest(&mut self, events: &[StreamEvent], new_offset: u64) {
        for event in events {
            self.last_event_ms = Some(event.timestamp_ms());

            match event {
                StreamEvent::Init { model, .. } => {
                    self.accumulated_usage.model = model.clone();
                }
                StreamEvent::Turn {
                    turn_number, usage, ..
                } => {
                    self.turn_count = *turn_number;
                    self.current_tool = None;
                    if let Some(u) = usage {
                        self.accumulated_usage.input_tokens += u.input_tokens;
                        self.accumulated_usage.output_tokens += u.output_tokens;
                        if let Some(cr) = u.cache_read_input_tokens {
                            *self
                                .accumulated_usage
                                .cache_read_input_tokens
                                .get_or_insert(0) += cr;
                        }
                        if let Some(cc) = u.cache_creation_input_tokens {
                            *self
                                .accumulated_usage
                                .cache_creation_input_tokens
                                .get_or_insert(0) += cc;
                        }
                    }
                }
                StreamEvent::ToolStart { name, .. } => {
                    self.current_tool = Some(name.clone());
                }
                StreamEvent::ToolEnd { .. } => {
                    self.current_tool = None;
                }
                StreamEvent::ToolOutputChunk { .. } => {}
                StreamEvent::Heartbeat { .. }
                | StreamEvent::TextChunk { .. }
                | StreamEvent::ThinkingChunk { .. } => {}
                StreamEvent::Result { usage, .. } => {
                    // Final usage overwrites accumulated
                    self.accumulated_usage = usage.clone();
                }
            }
        }
        self.offset = new_offset;
    }

    /// Returns true if the stream is stale (no events for the given duration).
    pub fn is_stale(&self, stale_threshold_ms: i64) -> bool {
        match self.last_event_ms {
            Some(last) => now_ms() - last > stale_threshold_ms,
            None => false, // No events yet — not stale, just not started
        }
    }

    /// Convert accumulated usage to a `TokenUsage` for storage in the graph.
    pub fn to_token_usage(&self) -> crate::graph::TokenUsage {
        crate::graph::TokenUsage {
            cost_usd: self.accumulated_usage.cost_usd.unwrap_or(0.0),
            input_tokens: self.accumulated_usage.input_tokens,
            output_tokens: self.accumulated_usage.output_tokens,
            cache_read_input_tokens: self.accumulated_usage.cache_read_input_tokens.unwrap_or(0),
            cache_creation_input_tokens: self
                .accumulated_usage
                .cache_creation_input_tokens
                .unwrap_or(0),
        }
    }
}

/// The standard stream file name within an agent's output directory.
pub const STREAM_FILE_NAME: &str = "stream.jsonl";

/// The raw Claude CLI output file (before translation).
pub const RAW_STREAM_FILE_NAME: &str = "raw_stream.jsonl";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CLAUDE_SONNET_MODEL_ID;
    use tempfile::TempDir;

    #[test]
    fn test_stream_event_roundtrip() {
        let events = vec![
            StreamEvent::Init {
                executor_type: "claude".to_string(),
                model: Some(CLAUDE_SONNET_MODEL_ID.to_string()),
                session_id: Some("sess-123".to_string()),
                timestamp_ms: 1000,
            },
            StreamEvent::Turn {
                turn_number: 1,
                tools_used: vec!["Bash".to_string(), "Read".to_string()],
                usage: Some(TurnUsage {
                    input_tokens: 500,
                    output_tokens: 200,
                    cache_read_input_tokens: Some(100),
                    cache_creation_input_tokens: None,
                    reasoning_tokens: None,
                }),
                timestamp_ms: 2000,
            },
            StreamEvent::ToolStart {
                name: "Bash".to_string(),
                timestamp_ms: 3000,
            },
            StreamEvent::ToolEnd {
                name: "Bash".to_string(),
                is_error: false,
                duration_ms: 150,
                timestamp_ms: 3150,
            },
            StreamEvent::Heartbeat { timestamp_ms: 4000 },
            StreamEvent::Result {
                success: true,
                usage: TotalUsage {
                    input_tokens: 1000,
                    output_tokens: 500,
                    cache_read_input_tokens: Some(200),
                    cache_creation_input_tokens: Some(50),
                    cost_usd: Some(0.05),
                    model: Some(CLAUDE_SONNET_MODEL_ID.to_string()),
                },
                timestamp_ms: 5000,
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: StreamEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, event);
        }
    }

    #[test]
    fn test_write_and_read_stream() {
        let dir = TempDir::new().unwrap();
        let stream_path = dir.path().join("stream.jsonl");

        let writer = StreamWriter::new(&stream_path);
        writer.write_init("native", Some("gpt-4"), None);
        writer.write_turn(
            1,
            vec!["Bash".to_string()],
            Some(TurnUsage {
                input_tokens: 100,
                output_tokens: 50,
                ..Default::default()
            }),
        );
        writer.write_heartbeat();
        writer.write_result(
            true,
            TotalUsage {
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: Some(0.01),
                ..Default::default()
            },
        );

        let (events, offset) = read_stream_events(&stream_path, 0).unwrap();
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], StreamEvent::Init { .. }));
        assert!(matches!(events[1], StreamEvent::Turn { .. }));
        assert!(matches!(events[2], StreamEvent::Heartbeat { .. }));
        assert!(matches!(events[3], StreamEvent::Result { .. }));
        assert!(offset > 0);

        // Incremental read from offset should yield nothing new
        let (events2, offset2) = read_stream_events(&stream_path, offset).unwrap();
        assert!(events2.is_empty());
        assert_eq!(offset2, offset);
    }

    #[test]
    fn test_translate_claude_assistant_event() {
        let line = r#"{"type":"assistant","message":{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}}],"usage":{"input_tokens":500,"output_tokens":100,"cache_read_input_tokens":50}}}"#;
        let event = translate_claude_event(line).unwrap();
        match event {
            StreamEvent::Turn {
                tools_used, usage, ..
            } => {
                assert_eq!(tools_used, vec!["Bash"]);
                let u = usage.unwrap();
                assert_eq!(u.input_tokens, 500);
                assert_eq!(u.output_tokens, 100);
                assert_eq!(u.cache_read_input_tokens, Some(50));
            }
            _ => panic!("Expected Turn event"),
        }
    }

    #[test]
    fn test_translate_claude_result_event() {
        let line = r#"{"type":"result","total_cost_usd":0.123,"usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":200}}"#;
        let event = translate_claude_event(line).unwrap();
        match event {
            StreamEvent::Result { success, usage, .. } => {
                assert!(success);
                assert_eq!(usage.input_tokens, 1000);
                assert_eq!(usage.output_tokens, 500);
                assert_eq!(usage.cost_usd, Some(0.123));
            }
            _ => panic!("Expected Result event"),
        }
    }

    #[test]
    fn test_translate_claude_system_event() {
        let line = format!(
            r#"{{"type":"system","session_id":"abc123","model":"{CLAUDE_SONNET_MODEL_ID}"}}"#
        );
        let event = translate_claude_event(&line).unwrap();
        match event {
            StreamEvent::Init {
                executor_type,
                model,
                session_id,
                ..
            } => {
                assert_eq!(executor_type, "claude");
                assert_eq!(model.as_deref(), Some(CLAUDE_SONNET_MODEL_ID));
                assert_eq!(session_id.as_deref(), Some("abc123"));
            }
            _ => panic!("Expected Init event"),
        }
    }

    #[test]
    fn test_translate_unknown_event_returns_none() {
        let line = r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}"#;
        assert!(translate_claude_event(line).is_none());
    }

    #[test]
    fn test_agent_stream_state_ingest() {
        let events = vec![
            StreamEvent::Init {
                executor_type: "claude".to_string(),
                model: Some("sonnet".to_string()),
                session_id: None,
                timestamp_ms: 1000,
            },
            StreamEvent::Turn {
                turn_number: 1,
                tools_used: vec!["Bash".to_string()],
                usage: Some(TurnUsage {
                    input_tokens: 500,
                    output_tokens: 200,
                    cache_read_input_tokens: Some(100),
                    cache_creation_input_tokens: None,
                    reasoning_tokens: None,
                }),
                timestamp_ms: 2000,
            },
            StreamEvent::ToolStart {
                name: "Read".to_string(),
                timestamp_ms: 3000,
            },
        ];

        let mut state = AgentStreamState::default();
        state.ingest(&events, 500);

        assert_eq!(state.last_event_ms, Some(3000));
        assert_eq!(state.turn_count, 1);
        assert_eq!(state.current_tool.as_deref(), Some("Read"));
        assert_eq!(state.accumulated_usage.input_tokens, 500);
        assert_eq!(state.accumulated_usage.output_tokens, 200);
        assert_eq!(state.accumulated_usage.model.as_deref(), Some("sonnet"));
        assert_eq!(state.offset, 500);
    }

    #[test]
    fn test_agent_stream_state_staleness() {
        let mut state = AgentStreamState::default();
        // No events yet → not stale
        assert!(!state.is_stale(5000));

        // Recent event → not stale
        state.last_event_ms = Some(now_ms());
        assert!(!state.is_stale(5000));

        // Old event → stale
        state.last_event_ms = Some(now_ms() - 10_000);
        assert!(state.is_stale(5000));
    }

    #[test]
    fn test_to_token_usage() {
        let state = AgentStreamState {
            accumulated_usage: TotalUsage {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_input_tokens: Some(200),
                cache_creation_input_tokens: Some(50),
                cost_usd: Some(0.05),
                model: Some("test".to_string()),
            },
            ..Default::default()
        };

        let token_usage = state.to_token_usage();
        assert_eq!(token_usage.input_tokens, 1000);
        assert_eq!(token_usage.output_tokens, 500);
        assert_eq!(token_usage.cache_read_input_tokens, 200);
        assert_eq!(token_usage.cache_creation_input_tokens, 50);
        assert_eq!(token_usage.cost_usd, 0.05);
    }

    #[test]
    fn test_read_stream_events_with_bad_lines() {
        let dir = TempDir::new().unwrap();
        let stream_path = dir.path().join("stream.jsonl");

        // Write a mix of valid and invalid lines
        let content = r#"{"type":"heartbeat","timestamp_ms":1000}
not json
{"type":"unknown_type","data":"foo"}
{"type":"heartbeat","timestamp_ms":2000}
"#;
        std::fs::write(&stream_path, content).unwrap();

        let (events, _) = read_stream_events(&stream_path, 0).unwrap();
        // Only the two heartbeats should parse — unknown_type doesn't match our enum
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_translate_claude_stream_file() {
        let dir = TempDir::new().unwrap();
        let raw_path = dir.path().join("raw_stream.jsonl");

        let content = format!(
            r#"{{"type":"system","session_id":"s1","model":"{CLAUDE_SONNET_MODEL_ID}"}}
{{"type":"assistant","message":{{"content":[{{"type":"text","text":"hi"}}],"usage":{{"input_tokens":100,"output_tokens":50}}}}}}
{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":100,"output_tokens":50}}}}
"#
        );
        std::fs::write(&raw_path, content).unwrap();

        let (events, offset) = translate_claude_stream(&raw_path, 0).unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], StreamEvent::Init { .. }));
        assert!(matches!(events[1], StreamEvent::Turn { .. }));
        assert!(matches!(events[2], StreamEvent::Result { .. }));
        assert!(offset > 0);
    }

    // ── Pi translation ──────────────────────────────────────────────────

    #[test]
    fn test_pi_usage_to_turn_maps_distinct_field_names() {
        // pi uses {input, output, cacheRead, cacheWrite} — NOT the canonical
        // input_tokens/output_tokens. A naive read would yield 0/0.
        let usage = serde_json::json!({
            "input": 22219, "output": 87, "cacheRead": 2048, "cacheWrite": 0,
            "totalTokens": 24354,
            "cost": {"input": 0.022219, "output": 0.000348, "total": 0.02293564}
        });
        let turn = pi_usage_to_turn(&usage);
        assert_eq!(turn.input_tokens, 22219);
        assert_eq!(turn.output_tokens, 87);
        assert_eq!(turn.cache_read_input_tokens, Some(2048));
        assert_eq!(turn.cache_creation_input_tokens, None); // cacheWrite 0 -> None
        // pi's input already excludes cacheRead: input+cacheRead+output==total.
        assert_eq!(turn.input_tokens + turn.output_tokens + 2048, 24354);
        assert!((pi_usage_cost(&usage) - 0.02293564).abs() < 1e-9);
    }

    /// A captured-shape pi event stream: the SAME per-turn usage snapshot
    /// appears on `message_update` (repeated, many per turn), `message_end`,
    /// AND `turn_end`. The harvest must sum `turn_end` ONCE per turn and never
    /// accumulate `message_update`/`message_end`.
    fn pi_fixture_two_turns() -> String {
        [
            r#"{"type":"session","version":3,"id":"sess-abc","cwd":"/tmp"}"#,
            r#"{"type":"agent_start"}"#,
            r#"{"type":"turn_start"}"#,
            // repeated streaming snapshots carrying usage — MUST be ignored
            r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_start","partial":{"usage":{"input":100,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":101,"cost":{"total":0.001}}}}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","partial":{"usage":{"input":100,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":101,"cost":{"total":0.001}}}}}"#,
            r#"{"type":"tool_execution_start","toolCallId":"t1","toolName":"bash","args":{"command":"ls"}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"bash","result":{"content":[{"type":"text","text":"ok"}]},"isError":false}"#,
            // message_end repeats the same usage — MUST be ignored
            r#"{"type":"message_end","message":{"role":"assistant","usage":{"input":200,"output":10,"cacheRead":50,"cacheWrite":0,"totalTokens":260,"cost":{"total":0.02}}}}"#,
            // turn_end #1 — the authoritative usage record
            r#"{"type":"turn_end","message":{"role":"assistant","provider":"openrouter","model":"z-ai/glm-5.2","content":[{"type":"thinking","thinking":"plan"},{"type":"toolCall","name":"bash","arguments":{"command":"ls"}}],"usage":{"input":200,"output":10,"cacheRead":50,"cacheWrite":0,"totalTokens":260,"cost":{"total":0.02}}}}"#,
            r#"{"type":"turn_start"}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","partial":{"usage":{"input":5,"output":7,"cacheRead":260,"cacheWrite":0,"totalTokens":272,"cost":{"total":0.03}}}}}"#,
            // turn_end #2 — final answer text
            r#"{"type":"turn_end","message":{"role":"assistant","provider":"openrouter","model":"z-ai/glm-5.2","content":[{"type":"text","text":"final answer"}],"usage":{"input":5,"output":7,"cacheRead":260,"cacheWrite":0,"totalTokens":272,"cost":{"total":0.03}}}}"#,
            r#"{"type":"agent_end","messages":[]}"#,
        ]
        .join("\n")
    }

    #[test]
    fn test_translate_pi_stream_sums_turn_end_once_no_double_count() {
        let tr = translate_pi_stream(&pi_fixture_two_turns(), None, true);

        // Two turns harvested.
        assert_eq!(tr.turn_count, 2);
        // Summed totals == sum of per-turn turn_end usage ONLY:
        //   input:  200 + 5   = 205
        //   output: 10  + 7   = 17
        //   cacheRead: 50 + 260 = 310
        // NOT inflated by the repeated message_update / message_end snapshots.
        assert_eq!(tr.total.input_tokens, 205);
        assert_eq!(tr.total.output_tokens, 17);
        assert_eq!(tr.total.cache_read_input_tokens, Some(310));
        // Cost summed from per-turn cost.total: 0.02 + 0.03 = 0.05
        assert!((tr.total.cost_usd.unwrap() - 0.05).abs() < 1e-9);
        // Total tokens equals the sum of per-turn totalTokens (260 + 272).
        assert_eq!(
            tr.total.input_tokens
                + tr.total.output_tokens
                + tr.total.cache_read_input_tokens.unwrap(),
            260 + 272
        );
        // Model derived from provider/model in the stream.
        assert_eq!(tr.total.model.as_deref(), Some("openrouter:z-ai/glm-5.2"));
        // Final assistant text captured for the session summary.
        assert_eq!(tr.final_text.as_deref(), Some("final answer"));
    }

    #[test]
    fn test_translate_pi_stream_event_shape() {
        let tr = translate_pi_stream(&pi_fixture_two_turns(), None, true);
        // First event Init(pi), last event Result with the summed usage.
        assert!(matches!(
            &tr.events[0],
            StreamEvent::Init { executor_type, session_id, .. }
                if executor_type == "pi" && session_id.as_deref() == Some("sess-abc")
        ));
        let last = tr.events.last().unwrap();
        match last {
            StreamEvent::Result { success, usage, .. } => {
                assert!(success);
                assert_eq!(usage.input_tokens, 205);
                assert_eq!(usage.output_tokens, 17);
            }
            other => panic!("expected Result, got {:?}", other),
        }
        // Per-step events between init and result populate the events pane.
        assert!(
            tr.events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolStart { name, .. } if name == "bash"))
        );
        assert!(
            tr.events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolEnd { name, .. } if name == "bash"))
        );
        assert!(
            tr.events
                .iter()
                .any(|e| matches!(e, StreamEvent::Turn { turn_number: 2, .. }))
        );
        assert!(
            tr.events.iter().any(
                |e| matches!(e, StreamEvent::TextChunk { text, .. } if text == "final answer")
            )
        );
    }

    #[test]
    fn test_translate_pi_stream_empty_still_yields_bookends() {
        let tr = translate_pi_stream("", Some("openrouter:z-ai/glm-5.2"), false);
        assert_eq!(tr.events.len(), 2); // init + result, no steps
        assert_eq!(tr.turn_count, 0);
        assert_eq!(tr.total.input_tokens, 0);
        assert!(matches!(&tr.events[0], StreamEvent::Init { .. }));
        assert!(matches!(
            &tr.events[1],
            StreamEvent::Result { success: false, .. }
        ));
        // model_override is honored when the stream carries no model.
        assert_eq!(tr.total.model.as_deref(), Some("openrouter:z-ai/glm-5.2"));
    }
}
