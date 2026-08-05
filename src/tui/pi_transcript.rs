//! Provider-native Pi transcript model shared by transcript surfaces.
//!
//! Embedded Pi chat is a PTY owned by Pi, so its terminal widget is not a
//! reusable Rust component. The reusable contract is Pi's documented JSON
//! event model plus WG's existing markdown renderer. This module owns that
//! model once: task Session Log consumes it directly, and parity tests compare
//! the same blocks against the chat-style presentation contract. Cumulative
//! tool updates are replacement snapshots keyed by `tool_call_id`.

use ratatui::text::Line;
use serde_json::Value;

use crate::tui::markdown::markdown_to_lines;

pub const LIVE_PROGRESS_MAX_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub enum PiToolPhase {
    Running { progress: Option<String> },
    Completed { result: String, is_error: bool },
}

#[derive(Clone, Debug)]
pub struct PiToolTranscript {
    pub tool_call_id: String,
    pub name: String,
    pub input: Value,
    pub phase: PiToolPhase,
}

#[derive(Clone, Debug)]
pub enum PiTranscriptBlock {
    AssistantMarkdown(String),
    Thinking(String),
    Tool(PiToolTranscript),
}

pub fn bounded_text(text: &str, max_bytes: usize) -> String {
    let text = text.trim();
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let tail_start = text.ceil_char_boundary(text.len() - max_bytes);
    format!("…{}", &text[tail_start..])
}

fn content_text(value: Option<&Value>) -> String {
    value
        .and_then(|value| value.get("content"))
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn progress_text(value: &Value) -> String {
    let content = content_text(value.get("partialResult"));
    if !content.trim().is_empty() {
        return bounded_text(&content, LIVE_PROGRESS_MAX_BYTES);
    }
    let state = value
        .get("childState")
        .and_then(Value::as_str)
        .unwrap_or("running");
    match value
        .get("progress")
        .or_else(|| value.get("progressCount"))
        .and_then(Value::as_u64)
    {
        Some(progress) => format!("{state} (progress {progress})"),
        None => state.to_string(),
    }
}

pub fn parse_tool(value: &Value) -> Option<PiToolTranscript> {
    let event_type = value.get("type")?.as_str()?;
    if !matches!(
        event_type,
        "tool_execution_start" | "tool_execution_update" | "tool_execution_end"
    ) {
        return None;
    }
    let tool_call_id = value
        .get("toolCallId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("legacy:{}", blake3::hash(value.to_string().as_bytes())));
    let name = value
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();
    let input = value.get("args").cloned().unwrap_or(Value::Null);
    let phase = match event_type {
        "tool_execution_start" => PiToolPhase::Running { progress: None },
        "tool_execution_update" => PiToolPhase::Running {
            progress: Some(progress_text(value)),
        },
        "tool_execution_end" => PiToolPhase::Completed {
            result: bounded_text(&content_text(value.get("result")), LIVE_PROGRESS_MAX_BYTES),
            is_error: value
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        _ => unreachable!("event type checked above"),
    };
    Some(PiToolTranscript {
        tool_call_id,
        name,
        input,
        phase,
    })
}

pub fn parse_turn(value: &Value) -> Vec<PiTranscriptBlock> {
    if value.get("type").and_then(Value::as_str) != Some("turn_end") {
        return Vec::new();
    }
    value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| {
            let block_type = block.get("type").and_then(Value::as_str)?;
            match block_type {
                "text" => block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(|text| PiTranscriptBlock::AssistantMarkdown(text.to_string())),
                "thinking" => block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(|text| PiTranscriptBlock::Thinking(text.to_string())),
                _ => None,
            }
        })
        .collect()
}

/// The same markdown presentation primitive used by WG's chat/output
/// transcript surfaces. Session Log delegates Pi assistant prose here rather
/// than maintaining a plain-text-only formatter.
pub fn render_assistant_markdown(markdown: &str, width: usize) -> Vec<Line<'static>> {
    markdown_to_lines(markdown, width.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cumulative_update_model_is_bounded_and_keeps_latest_tail() {
        let value = serde_json::json!({
            "type": "tool_execution_update",
            "toolCallId": "call-1",
            "toolName": "bash",
            "args": {"command": "cargo test"},
            "partialResult": {"content": [{"type": "text", "text": format!("{}LATEST", "x".repeat(5000))}]}
        });
        let tool = parse_tool(&value).unwrap();
        let PiToolPhase::Running {
            progress: Some(progress),
        } = tool.phase
        else {
            panic!("expected running progress")
        };
        assert!(progress.len() <= LIVE_PROGRESS_MAX_BYTES + '…'.len_utf8());
        assert!(progress.ends_with("LATEST"));
    }

    #[test]
    fn turn_preserves_assistant_markdown_and_thinking_as_distinct_blocks() {
        let value = serde_json::json!({
            "type": "turn_end",
            "message": {"content": [
                {"type": "thinking", "thinking": "check the **edge**"},
                {"type": "text", "text": "## Result\n\n- **passed**\n- `stable`"}
            ]}
        });
        let blocks = parse_turn(&value);
        assert!(matches!(blocks[0], PiTranscriptBlock::Thinking(_)));
        assert!(matches!(blocks[1], PiTranscriptBlock::AssistantMarkdown(_)));
        let PiTranscriptBlock::AssistantMarkdown(markdown) = &blocks[1] else {
            unreachable!()
        };
        let rendered = render_assistant_markdown(markdown, 80);
        assert!(rendered.iter().any(|line| line.spans.len() > 1));
    }
}
