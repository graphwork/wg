//! Summarize tool: recursive map-reduce summarization for arbitrarily-large text.
//!
//! Given a source (file path or inline text), produces a single focused
//! summary via the following algorithm:
//!
//! 1. Refuse if input exceeds `max_input_bytes` (default 1 MiB).
//! 2. If the input fits in a single model call (~40% of context window),
//!    summarize directly and return.
//! 3. Otherwise chunk the input on paragraph boundaries when possible,
//!    summarize each chunk independently with an `instruction`-aware
//!    focus prompt, concatenate the chunk summaries, and recurse.
//! 4. Terminate at `MAX_RECURSION_DEPTH` or a single-chunk base case.
//!
//! This is the cornerstone primitive from the reliability action plan
//! (L2): it lets agents reduce arbitrarily-large content hierarchically
//! without ever loading more than a single chunk into context at once.
//!
//! Unlike `delegate` (which runs a general sub-agent with tools),
//! `summarize` issues direct text-in/text-out LLM calls — no tool loop,
//! no recursion into other tools. That makes it cheap and predictable.

use std::path::{Path, PathBuf};
use std::pin::Pin;

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolOutput, truncate_tool_output};
use crate::executor::native::client::{
    ContentBlock, Message, MessagesRequest, Role, ToolDefinition,
};
use crate::executor::native::provider::Provider;

/// Default hard ceiling on input size (bytes). Prevents accidental
/// whole-codebase summarizations that would take forever.
const DEFAULT_MAX_INPUT_BYTES: usize = 1_000_000;

/// Max output tokens for each summarization LLM call.
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 1024;

/// Tool-level output truncation (the tool result itself).
const MAX_SUMMARIZE_OUTPUT_CHARS: usize = 8_000;

/// Fraction of context window to use per chunk. Leaves headroom for
/// system prompt + instruction + reasoning + completion.
const CHUNK_CONTEXT_FRACTION: f64 = 0.40;

/// Chars-per-token estimate for chunk sizing.
const CHARS_PER_TOKEN: f64 = 4.0;

/// Maximum recursion depth to prevent runaway trees if chunks don't shrink.
const MAX_RECURSION_DEPTH: usize = 8;

/// System prompt for summarization LLM calls. Intentionally terse — the
/// focus instruction carries the task-specific guidance.
const SUMMARIZE_SYSTEM_PROMPT: &str = "\
You are a text summarization agent. Given a chunk of text and a focus \
instruction, produce a concise summary that preserves the details the \
instruction asks for. Return only the summary text — no preamble, no \
commentary, no meta-discussion.";

/// The summarize tool.
pub struct SummarizeTool {
    workgraph_dir: PathBuf,
    /// Model override for summarization calls. Empty = use `WG_MODEL` env
    /// var (set by the coordinator at spawn time).
    model: String,
}

impl SummarizeTool {
    pub fn new(workgraph_dir: PathBuf, model: String) -> Self {
        Self {
            workgraph_dir,
            model,
        }
    }
}

#[async_trait]
impl Tool for SummarizeTool {
    fn name(&self) -> &str {
        "summarize"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "summarize".to_string(),
            description: "Recursively summarize a large text source via map-reduce. \
                Reads from a file path or inline text, chunks it to fit the model's \
                context window, summarizes each chunk independently with your focus \
                instruction, then merges — recursing if the merged summaries are still \
                too large. Use this when a source is too big to read directly (large \
                files, long logs, big tool outputs). Each level runs as direct text \
                LLM calls with no tool loop, so it's cheap and predictable."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Either a file path (relative or absolute) or inline text. If the string resolves to an existing file on disk, the file is loaded; otherwise the string itself is treated as the input text."
                    },
                    "instruction": {
                        "type": "string",
                        "description": "Focus instruction — what to preserve in the summary. E.g. 'extract public function signatures', 'focus on error handling', 'list the section headings'. Defaults to a generic 'summarize the text' if not provided."
                    },
                    "max_input_bytes": {
                        "type": "integer",
                        "description": "Hard ceiling on input size in bytes (default 1000000 = 1 MB). Sources larger than this are rejected to prevent runaway cost."
                    }
                },
                "required": ["source"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        // 1. Parse input
        let source = match input.get("source").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s,
            _ => {
                return ToolOutput::error(
                    "Missing or empty required parameter: source".to_string(),
                );
            }
        };
        let instruction = input
            .get("instruction")
            .and_then(|v| v.as_str())
            .unwrap_or("Summarize the text.");
        let max_input_bytes = input
            .get("max_input_bytes")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_INPUT_BYTES);

        // 2. Load content — file path or inline
        let content = {
            let as_path = Path::new(source);
            if as_path.exists() && as_path.is_file() {
                match std::fs::read_to_string(as_path) {
                    Ok(c) => c,
                    Err(e) => {
                        return ToolOutput::error(format!(
                            "Failed to read source file '{}': {}",
                            source, e
                        ));
                    }
                }
            } else {
                source.to_string()
            }
        };

        if content.len() > max_input_bytes {
            return ToolOutput::error(format!(
                "Input exceeds max_input_bytes: {} > {} (raise max_input_bytes to allow larger inputs)",
                content.len(),
                max_input_bytes
            ));
        }

        // 3. Create provider
        let model = if !self.model.is_empty() {
            self.model.clone()
        } else {
            std::env::var("WG_MODEL")
                .ok()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string())
        };

        let provider =
            match crate::executor::native::provider::create_provider(&self.workgraph_dir, &model) {
                Ok(p) => p,
                Err(e) => {
                    return ToolOutput::error(format!(
                        "Failed to create provider for summarize: {}",
                        e
                    ));
                }
            };

        eprintln!(
            "[summarize] starting: model={}, input_bytes={}, instruction='{}'",
            model,
            content.len(),
            instruction
        );

        // 4. Recursive summarize
        match recursive_summarize(provider.as_ref(), &content, instruction, 0).await {
            Ok(summary) => {
                let truncated = truncate_tool_output(&summary, MAX_SUMMARIZE_OUTPUT_CHARS);
                ToolOutput::success(truncated)
            }
            Err(e) => ToolOutput::error(format!("Summarize failed: {}", e)),
        }
    }
}

/// Estimate how many chars of input fit in one summarization LLM call,
/// leaving headroom for system prompt, instruction, reasoning, and output.
fn chunk_size_chars(window_size: usize) -> usize {
    ((window_size as f64) * CHUNK_CONTEXT_FRACTION * CHARS_PER_TOKEN) as usize
}

/// Split `text` into chunks of approximately `chunk_chars` bytes each.
/// Prefers to break on paragraph boundaries (`\n\n`) within the final 20%
/// of each chunk. Falls back to char-boundary truncation otherwise.
fn chunk_text(text: &str, chunk_chars: usize) -> Vec<String> {
    if text.len() <= chunk_chars {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut pos = 0;

    while pos < text.len() {
        let target_end = (pos + chunk_chars).min(text.len());
        let end = text.floor_char_boundary(target_end);

        let break_pt = if end == text.len() {
            end
        } else {
            // Prefer a paragraph boundary in the last 20% of the chunk.
            let min_break = pos + (chunk_chars * 4 / 5);
            if min_break < end {
                text[min_break..end]
                    .rfind("\n\n")
                    .map(|i| min_break + i + 2)
                    .unwrap_or(end)
            } else {
                end
            }
        };

        chunks.push(text[pos..break_pt].to_string());
        pos = break_pt;
    }

    chunks
}

/// Recursively summarize text via map-reduce.
///
/// `depth` is the recursion level; bail at `MAX_RECURSION_DEPTH` to
/// prevent runaway trees when summaries aren't shrinking fast enough.
///
/// This is exposed as `pub(crate)` because L3 compaction (in `agent.rs`)
/// calls it directly on a serialized representation of the agent's own
/// message history when the escalation ladder saturates.
pub(crate) fn recursive_summarize<'a>(
    provider: &'a dyn Provider,
    text: &'a str,
    instruction: &'a str,
    depth: usize,
) -> Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
    Box::pin(async move {
        if depth >= MAX_RECURSION_DEPTH {
            return Err(format!(
                "Max recursion depth ({}) exceeded — summaries are not shrinking. \
                 Input may be pathologically long or chunk_size too large.",
                MAX_RECURSION_DEPTH
            ));
        }

        let window_size = provider.context_window();
        let chunk_chars = chunk_size_chars(window_size);

        // Base case: fits in one call.
        if text.len() <= chunk_chars {
            eprintln!(
                "[summarize] depth={}: single call ({} bytes, window={})",
                depth,
                text.len(),
                window_size
            );
            return summarize_chunk(provider, text, instruction).await;
        }

        // Recursive case: chunk, map, reduce.
        let chunks = chunk_text(text, chunk_chars);
        eprintln!(
            "[summarize] depth={}: {} chunks from {} bytes (chunk_chars={})",
            depth,
            chunks.len(),
            text.len(),
            chunk_chars
        );

        // Map: summarize each chunk independently.
        let mut chunk_summaries = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            let chunk_instruction = format!(
                "This is part {} of {} of a larger document. {}",
                i + 1,
                chunks.len(),
                instruction
            );
            let summary = summarize_chunk(provider, chunk, &chunk_instruction).await?;
            eprintln!(
                "[summarize] depth={} chunk {}/{}: {} → {} bytes",
                depth,
                i + 1,
                chunks.len(),
                chunk.len(),
                summary.len()
            );
            chunk_summaries.push(summary);
        }

        // Reduce: merge chunk summaries.
        let merged = chunk_summaries.join("\n\n---\n\n");

        if merged.len() <= chunk_chars {
            // Final merge pass.
            let merge_instruction = format!(
                "These are {} partial summaries of a larger document. Merge them into \
                 one coherent, non-redundant summary. {}",
                chunks.len(),
                instruction
            );
            return summarize_chunk(provider, &merged, &merge_instruction).await;
        }

        // Merged summaries still too large — recurse.
        eprintln!(
            "[summarize] depth={}: merged summaries still too large ({} bytes), recursing",
            depth,
            merged.len()
        );
        recursive_summarize(provider, &merged, instruction, depth + 1).await
    })
}

/// Issue a single summarization LLM call. Text-in/text-out, no tools.
async fn summarize_chunk(
    provider: &dyn Provider,
    text: &str,
    instruction: &str,
) -> Result<String, String> {
    let prompt = format!("Instruction: {}\n\n---\n\n{}", instruction, text);
    let request = MessagesRequest {
        model: provider.model().to_string(),
        max_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
        system: Some(SUMMARIZE_SYSTEM_PROMPT.to_string()),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: prompt }],
        }],
        tools: vec![],
        stream: false,
    };

    let response = provider
        .send(&request)
        .await
        .map_err(|e| format!("API error in summarize call: {}", e))?;

    let text: String = response
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    if text.trim().is_empty() {
        return Err("Empty summary response from provider".to_string());
    }

    Ok(text)
}

/// Register the summarize tool in the given registry.
pub fn register_summarize_tool(
    registry: &mut super::ToolRegistry,
    workgraph_dir: PathBuf,
    model: String,
) {
    registry.register(Box::new(SummarizeTool::new(workgraph_dir, model)));
}

// ───────────────────────────────────────────────────────────────────────
// L3: Summarize-based compaction of an agent's own message history.
// Used by `agent.rs` when the standard emergency_compact /
// hard_emergency_compact ladder has saturated (repeated no-op fires at
// the compact threshold).
// ───────────────────────────────────────────────────────────────────────

/// Number of recent messages to preserve verbatim when summarizing an
/// agent's history. The summary captures older turns; these stay intact
/// so the model keeps immediate working memory.
const L3_KEEP_RECENT_MESSAGES: usize = 2;

/// Instruction for the L3 history-summarization call. Designed to
/// preserve decisions, facts discovered, and open threads — not
/// verbatim text.
const L3_HISTORY_SUMMARY_INSTRUCTION: &str = "\
Summarize this agent conversation transcript. Preserve: \
(1) the original task, \
(2) key facts discovered (file contents, search results, command outputs), \
(3) decisions made, \
(4) any open questions or subtasks created, \
(5) errors encountered and their resolution. \
Omit: conversational filler, restated plans, tool-call echoing, \
redundant observations. Use a structured format with sections.";

/// Serialize a `Message` to a compact text representation suitable for
/// inclusion in a summarization prompt.
fn serialize_message_for_summary(msg: &Message) -> String {
    let role = match msg.role {
        Role::User => "USER",
        Role::Assistant => "ASSISTANT",
    };
    let mut parts = Vec::new();
    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                parts.push(text.clone());
            }
            ContentBlock::Thinking { thinking, .. } => {
                parts.push(format!("[thinking] {}", thinking));
            }
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(format!("[tool_use {}] {}", name, input));
            }
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                let prefix = if *is_error {
                    "[tool_result ERROR]"
                } else {
                    "[tool_result]"
                };
                parts.push(format!("{} {}", prefix, content));
            }
        }
    }
    format!("{}: {}", role, parts.join("\n"))
}

/// Compact an agent's message history via recursive summarization.
///
/// This is the L3 tier of the compaction escalation ladder. When
/// `emergency_compact` and `hard_emergency_compact` can no longer reduce
/// message tokens (the accumulation is in Text/Thinking/ToolUse content
/// the model itself produced), this function:
///
/// 1. Splits `messages` into `older` (everything except the last
///    `L3_KEEP_RECENT_MESSAGES`) and `recent` (the tail, kept verbatim).
/// 2. Serializes `older` to a text transcript.
/// 3. Invokes `recursive_summarize` to reduce the transcript to a
///    bounded-size summary (recursing as needed for very long histories).
/// 4. Returns a new message vec:
///    `[User("PRIOR CONVERSATION SUMMARY: <summary>"), recent...]`
///
/// Like the other compaction functions this preserves tool_use/tool_result
/// pairing in `recent`, and message count is replaced (not preserved) —
/// this is a more aggressive intervention that explicitly drops the
/// older-turn structure in exchange for a bounded-size replacement.
///
/// On failure (provider errors, empty summary) the original messages are
/// returned unchanged — compaction is best-effort, never a blocker.
pub async fn summarize_history_for_compaction(
    provider: &dyn Provider,
    messages: Vec<Message>,
) -> Vec<Message> {
    if messages.len() <= L3_KEEP_RECENT_MESSAGES + 1 {
        // Not enough history to bother summarizing — the standard
        // compact already handles small vecs.
        return messages;
    }

    let split = messages.len() - L3_KEEP_RECENT_MESSAGES;
    let older = &messages[..split];
    let recent = &messages[split..];

    let transcript: String = older
        .iter()
        .map(serialize_message_for_summary)
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    eprintln!(
        "[summarize-l3] compacting {} older messages ({} bytes transcript), keeping {} recent",
        older.len(),
        transcript.len(),
        recent.len()
    );

    let summary =
        match recursive_summarize(provider, &transcript, L3_HISTORY_SUMMARY_INSTRUCTION, 0).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[summarize-l3] recursive_summarize failed: {} — returning messages unchanged",
                    e
                );
                return messages;
            }
        };

    if summary.trim().is_empty() {
        eprintln!("[summarize-l3] empty summary — returning messages unchanged");
        return messages;
    }

    eprintln!(
        "[summarize-l3] summary produced: {} bytes (from {} bytes transcript)",
        summary.len(),
        transcript.len()
    );

    // Build the new message vec: summary as a user-role context message,
    // followed by the preserved recent messages verbatim.
    let mut compacted = Vec::with_capacity(recent.len() + 1);
    compacted.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!(
                "PRIOR CONVERSATION SUMMARY (older turns compacted to reduce context pressure):\n\n{}",
                summary
            ),
        }],
    });
    compacted.extend_from_slice(recent);
    compacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_size_chars_scales_with_window() {
        // At 32k window: 32000 * 0.40 * 4.0 = 51200 chars
        let small = chunk_size_chars(32_000);
        // At 200k window: 200000 * 0.40 * 4.0 = 320000 chars
        let large = chunk_size_chars(200_000);
        assert!(large > small);
        assert_eq!(small, 51_200);
        assert_eq!(large, 320_000);
    }

    #[test]
    fn test_chunk_text_short_input_single_chunk() {
        let text = "hello world";
        let chunks = chunk_text(text, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn test_chunk_text_long_input_splits_on_paragraph() {
        let para1 = "A".repeat(400);
        let para2 = "B".repeat(400);
        let para3 = "C".repeat(400);
        let text = format!("{}\n\n{}\n\n{}", para1, para2, para3);
        // Chunk size 500 → should split roughly at paragraph boundaries.
        let chunks = chunk_text(&text, 500);
        assert!(
            chunks.len() >= 2,
            "expected multiple chunks, got {}",
            chunks.len()
        );

        // Concatenation should reconstruct the original (no chars lost).
        let recombined: String = chunks.join("");
        assert_eq!(recombined, text);
    }

    #[test]
    fn test_chunk_text_long_input_no_paragraphs_falls_back_to_char_boundary() {
        let text = "x".repeat(2500);
        let chunks = chunk_text(&text, 1000);
        assert!(chunks.len() >= 3);
        let recombined: String = chunks.join("");
        assert_eq!(recombined, text);
    }

    #[test]
    fn test_chunk_text_respects_char_boundaries() {
        // Text with multi-byte chars at chunk boundary positions
        let text = "héllo wörld ".repeat(100);
        let chunks = chunk_text(&text, 50);
        let recombined: String = chunks.join("");
        assert_eq!(recombined, text);
    }

    #[test]
    fn test_tool_definition_has_source_required() {
        let tool = SummarizeTool::new(PathBuf::from("/tmp"), String::new());
        let def = tool.definition();
        assert_eq!(def.name, "summarize");
        let schema = def.input_schema.as_object().unwrap();
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("source")));
    }
}
