//! Derive a task's *current step* (what the agent is doing right now) from the
//! live NDJSON stream tail plus a graph-log fallback.
//!
//! This is a deliberately small, read-only, non-blocking derivation intended
//! for dense surfaces like `wg viz --compact`, `wg status`, and the TUI. It
//! reads ONLY the tail of `.wg/agents/<agent>/raw_stream.jsonl` (pi NDJSON),
//! seeks to the end, and never takes a lock, so repeated renders of N active
//! tasks cost O(N) bounded tail reads.
//!
//! The stream shape we consume (pi `--mode json`):
//! - `{"type":"tool_execution_start","toolName":"bash","args":{...}}`
//! - `{"type":"message_end","message":{"role":"assistant","content":[
//!      {"type":"toolCall","name":"edit","arguments":{"file_path":"..."}},
//!      {"type":"text","text":"..."}]}}`
//! - `{"type":"turn_end","message":{"role":"assistant","content":[...]}}`
//!
//! Any other event is ignored. When no tool has run yet we surface the last
//! assistant text line. When there is no usable stream (absent / partial /
//! rotating / gone agent) we fall back to the task's last graph log line.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::graph::Task;

/// Maximum number of bytes read from the tail of the stream file.
pub const TAIL_BYTES: u64 = 8 * 1024;

/// Maximum length (characters) of a summarised current step.
pub const STEP_MAX_CHARS: usize = 40;

/// Maximum length (characters) of a fallback graph-log line.
pub const LOG_MAX_CHARS: usize = 60;

/// Path to the live stream file for an agent directory.
pub fn stream_path(workgraph_dir: &Path, agent_id: &str) -> PathBuf {
    workgraph_dir
        .join("agents")
        .join(agent_id)
        .join("raw_stream.jsonl")
}

/// Derive the current step for `task`, preferring the live stream tail and
/// falling back to the last graph log line.
///
/// Returns `None` when neither source yields anything useful. Never blocks and
/// never panics on an absent/partial/rotating stream file.
pub fn current_step(workgraph_dir: &Path, task: &Task) -> Option<String> {
    task.assigned
        .as_deref()
        .and_then(|agent| read_stream_step(workgraph_dir, agent))
        .or_else(|| log_line_fallback(task))
}

/// Derive the current step from the live stream tail for an agent id.
pub fn current_step_for_agent(workgraph_dir: &Path, agent_id: &str) -> Option<String> {
    read_stream_step(workgraph_dir, agent_id)
}

fn read_stream_step(workgraph_dir: &Path, agent_id: &str) -> Option<String> {
    let path = stream_path(workgraph_dir, agent_id);
    let tail = read_tail(&path, TAIL_BYTES).ok()?;
    step_from_tail(&tail)
}

/// Read at most `cap` bytes from the end of `path`. Returns the bytes decoded
/// with lossy UTF-8 so a partial multi-byte boundary cannot fail the read.
fn read_tail(path: &Path, cap: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(cap);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    let mut buf = Vec::new();
    // Bound the read defensively: a concurrently-growing file could return more
    // than `cap` bytes; take at most `cap` so the cost stays bounded.
    let mut limited = file.take(cap);
    limited.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        // Drop the first (likely partial) line when we started mid-file.
        if let Some(pos) = text.find('\n') {
            Ok(text[pos + 1..].to_string())
        } else {
            // Whole tail was one partial line — nothing usable.
            Ok(String::new())
        }
    } else {
        Ok(text)
    }
}

/// Scan the tail (newest-first) for the most recent meaningful activity.
fn step_from_tail(tail: &str) -> Option<String> {
    for line in tail.lines().rev() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match val.get("type").and_then(|v| v.as_str()) {
            Some("tool_execution_start") => {
                let name = val
                    .get("toolName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                if let Some(step) = summarize_tool(name, val.get("args")) {
                    return Some(step);
                }
            }
            Some("message_end") | Some("turn_end") => {
                let Some(msg) = val.get("message") else {
                    continue;
                };
                if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                    continue;
                }
                // Prefer a toolCall in the message; otherwise the last text line.
                if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
                    if let Some(tool) = last_tool_call(content)
                        && let Some(step) = summarize_tool(
                            &tool.0,
                            Some(&serde_json::Value::Object(tool.1.clone())),
                        )
                    {
                        return Some(step);
                    }
                    if let Some(text) = last_text(content)
                        && let Some(step) = summarize_text(&text)
                    {
                        return Some(step);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the last `toolCall` block in an assistant content array.
fn last_tool_call(
    content: &[serde_json::Value],
) -> Option<(String, serde_json::Map<String, serde_json::Value>)> {
    let mut found = None;
    for block in content {
        if block.get("type").and_then(|v| v.as_str()) == Some("toolCall") {
            let name = block
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("tool")
                .to_string();
            let args = block
                .get("arguments")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            found = Some((name, args));
        }
    }
    found
}

/// Find the last `text` block in an assistant content array.
fn last_text(content: &[serde_json::Value]) -> Option<String> {
    let mut found = None;
    for block in content {
        if block.get("type").and_then(|v| v.as_str()) == Some("text")
            && let Some(text) = block.get("text").and_then(|v| v.as_str())
        {
            let text = text.trim();
            if !text.is_empty() {
                found = Some(text.to_string());
            }
        }
    }
    found
}

/// Summarise a tool invocation into a short human-readable step.
fn summarize_tool(name: &str, args: Option<&serde_json::Value>) -> Option<String> {
    let obj = args.and_then(|v| v.as_object());
    let str_arg = |keys: &[&str]| -> Option<String> {
        let obj = obj?;
        for key in keys {
            if let Some(v) = obj.get(*key).and_then(|v| v.as_str())
                && !v.trim().is_empty()
            {
                return Some(v.trim().to_string());
            }
        }
        None
    };
    let raw = match name {
        "bash" => {
            let cmd = str_arg(&["command"]).unwrap_or_default();
            let first = first_command_line(&cmd);
            let first = simplify_bash_command(&first);
            if first.is_empty() {
                "bash".to_string()
            } else {
                format!("running {}", first)
            }
        }
        "edit" | "write" | "read" => {
            let verb = match name {
                "edit" => "editing",
                "write" => "writing",
                _ => "reading",
            };
            let path = str_arg(&["file_path", "path", "file"]).unwrap_or_default();
            if path.is_empty() {
                name.to_string()
            } else {
                format!("{} {}", verb, path)
            }
        }
        _ => {
            let detail =
                str_arg(&["command", "file_path", "path", "pattern", "query"]).unwrap_or_default();
            if detail.is_empty() {
                name.to_string()
            } else {
                format!("{} {}", name, first_command_line(&detail))
            }
        }
    };
    Some(truncate_chars(&raw, STEP_MAX_CHARS))
}

/// Summarise the last assistant text line into a short step.
fn summarize_text(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("");
    if line.is_empty() {
        return None;
    }
    Some(truncate_chars(line, STEP_MAX_CHARS))
}

/// Fall back to the task's most recent graph log line.
fn log_line_fallback(task: &Task) -> Option<String> {
    let line = task
        .log
        .last()
        .map(|e| e.message.trim())
        .filter(|m| !m.is_empty())?;
    Some(truncate_chars(&first_command_line(line), LOG_MAX_CHARS))
}

/// Simplify a one-line bash command for the activity column: strip a leading
/// `cd <path> &&` / `cd <path>;` prefix (so the operator sees WHAT before
/// WHERE) and collapse any remaining absolute worktree path to a short form.
///
/// Deliberately bounded and deterministic: this only recognises the literal
/// `cd ` prefix plus the first `&&`/`;` terminator, and the worktree-path
/// collapse is a marker-based string substitution. No shell parsing.
fn simplify_bash_command(s: &str) -> String {
    let stripped = strip_leading_cd(s.trim());
    collapse_worktree_paths(&stripped)
}

/// Strip a leading `cd <path> &&` or `cd <path>;` prefix. A bare `cd <path>`
/// with no following command is left intact (it is a genuine command).
fn strip_leading_cd(s: &str) -> String {
    let Some(rest) = s.strip_prefix("cd ") else {
        return s.to_string();
    };
    // Find the earliest `&&` or `;` terminator after the path.
    let cut = [rest.find("&&"), rest.find(';')]
        .into_iter()
        .flatten()
        .min();
    let Some(idx) = cut else {
        return s.to_string();
    };
    let after = &rest[idx..];
    let after = after
        .strip_prefix("&&")
        .or_else(|| after.strip_prefix(';'))
        .unwrap_or(after)
        .trim();
    if after.is_empty() {
        s.to_string()
    } else {
        after.to_string()
    }
}

/// Collapse whitespace-separated tokens containing a `.wg-worktrees/` marker
/// to `…/<remainder-after-the-agent-dir>`, so an absolute worktree path does
/// not dominate the visible budget. Tokens without the marker are untouched.
fn collapse_worktree_paths(s: &str) -> String {
    const MARKER: &str = ".wg-worktrees/";
    s.split(' ')
        .map(|token| match token.find(MARKER) {
            Some(pos) => {
                let after_marker = &token[pos + MARKER.len()..];
                // Drop the agent/worktree directory component; keep the rest.
                let rest = after_marker
                    .split_once('/')
                    .map(|(_, tail)| tail)
                    .unwrap_or("");
                if rest.is_empty() {
                    "…".to_string()
                } else {
                    format!("…/{}", rest)
                }
            }
            None => token.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collapse a multi-line shell command to its first non-empty line and squeeze
/// internal whitespace so a summary stays a single readable line.
fn first_command_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// Truncate to `max` characters, appending `…` when clipped.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    // Reserve one column for the ellipsis.
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{LogEntry, Task};

    fn task_with_agent(agent: &str) -> Task {
        Task {
            id: "t1".to_string(),
            title: "Task".to_string(),
            assigned: Some(agent.to_string()),
            ..Task::default()
        }
    }

    fn write_stream(dir: &Path, agent: &str, body: &str) {
        let path = dir.join("agents").join(agent).join("raw_stream.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn bash_tool_start_summarises_command() {
        let dir = tempfile::tempdir().unwrap();
        write_stream(
            dir.path(),
            "a1",
            r#"{"type":"tool_execution_start","toolCallId":"x","toolName":"bash","args":{"command":"cargo test --lib"}}"#,
        );
        let step = current_step(dir.path(), &task_with_agent("a1"));
        assert_eq!(step.as_deref(), Some("running cargo test --lib"));
    }

    #[test]
    fn edit_message_end_toolcall_summarises_file_path() {
        let dir = tempfile::tempdir().unwrap();
        write_stream(
            dir.path(),
            "a2",
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","name":"edit","arguments":{"file_path":"src/commands/viz/ascii.rs"}}]}}"#,
        );
        let step = current_step(dir.path(), &task_with_agent("a2"));
        assert_eq!(step.as_deref(), Some("editing src/commands/viz/ascii.rs"));
    }

    #[test]
    fn write_tool_summarises_file_path() {
        let dir = tempfile::tempdir().unwrap();
        write_stream(
            dir.path(),
            "a3",
            r#"{"type":"tool_execution_start","toolName":"write","args":{"file_path":"docs/embed.md"}}"#,
        );
        assert_eq!(
            current_step(dir.path(), &task_with_agent("a3")).as_deref(),
            Some("writing docs/embed.md")
        );
    }

    #[test]
    fn last_tool_wins_over_earlier_ones() {
        let dir = tempfile::tempdir().unwrap();
        write_stream(
            dir.path(),
            "a4",
            concat!(
                r#"{"type":"tool_execution_start","toolName":"bash","args":{"command":"ls"}}"#,
                "\n",
                r#"{"type":"tool_execution_start","toolName":"read","args":{"path":"README.md"}}"#,
            ),
        );
        assert_eq!(
            current_step(dir.path(), &task_with_agent("a4")).as_deref(),
            Some("reading README.md")
        );
    }

    #[test]
    fn no_tool_yet_falls_back_to_last_assistant_text() {
        let dir = tempfile::tempdir().unwrap();
        write_stream(
            dir.path(),
            "a5",
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"I will now write the embed"}]}}"#,
        );
        assert_eq!(
            current_step(dir.path(), &task_with_agent("a5")).as_deref(),
            Some("I will now write the embed")
        );
    }

    #[test]
    fn absent_stream_falls_back_to_log_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut task = task_with_agent("missing-agent");
        task.log.push(LogEntry {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            actor: None,
            user: None,
            message: "Committed: abc123 — pushed to remote".to_string(),
        });
        assert_eq!(
            current_step(dir.path(), &task).as_deref(),
            Some("Committed: abc123 — pushed to remote")
        );
    }

    #[test]
    fn malformed_stream_falls_back_to_log_line() {
        let dir = tempfile::tempdir().unwrap();
        write_stream(dir.path(), "a6", "not json\n{ broken\n");
        let mut task = task_with_agent("a6");
        task.log.push(LogEntry {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            actor: None,
            user: None,
            message: "working on it".to_string(),
        });
        assert_eq!(
            current_step(dir.path(), &task).as_deref(),
            Some("working on it")
        );
    }

    #[test]
    fn no_stream_and_no_log_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(current_step(dir.path(), &task_with_agent("nobody")).is_none());
    }

    #[test]
    fn unassigned_task_uses_log_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut task = Task {
            id: "t".to_string(),
            title: "T".to_string(),
            ..Task::default()
        };
        task.log.push(LogEntry {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            actor: None,
            user: None,
            message: "kicking off".to_string(),
        });
        assert_eq!(
            current_step(dir.path(), &task).as_deref(),
            Some("kicking off")
        );
    }

    #[test]
    fn tail_read_is_bounded_and_ignores_partial_head() {
        let dir = tempfile::tempdir().unwrap();
        // Build a file larger than the tail cap. The newest event is the last
        // line; the read must find it even though the head is dropped.
        let filler = format!(
            "{{\"type\":\"noise\",\"pad\":\"{}\"}}\n",
            "x".repeat((TAIL_BYTES as usize) + 100)
        );
        let body = format!(
            "{}",
            format!(
                "{}{}",
                filler,
                "{\"type\":\"tool_execution_start\",\"toolName\":\"bash\",\"args\":{\"command\":\"echo hi\"}}\n"
            )
        );
        // Prepend one full filler line so the partial-head drop is exercised.
        let body = format!(
            "{{\"type\":\"noise\",\"pad\":\"{}\"}}\n{}",
            "y".repeat(64),
            body
        );
        write_stream(dir.path(), "a7", &body);
        assert_eq!(
            current_step(dir.path(), &task_with_agent("a7")).as_deref(),
            Some("running echo hi")
        );
    }

    #[test]
    fn truncate_chars_adds_ellipsis() {
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
        assert_eq!(truncate_chars("abc", 4), "abc");
    }

    #[test]
    fn bash_summary_strips_leading_cd_prefix() {
        // `cd <path> &&` and `cd <path>;` prefixes are dropped so the useful
        // command is what remains.
        assert_eq!(simplify_bash_command("cd a && cargo test"), "cargo test");
        assert_eq!(simplify_bash_command("cd a; cargo test"), "cargo test");
        assert_eq!(simplify_bash_command("cd a;cargo test"), "cargo test");
        assert_eq!(simplify_bash_command("cd a && git status"), "git status");
    }

    #[test]
    fn bash_summary_leaves_genuine_cd_alone() {
        // A `cd` later in the line (not a leading prefix) is untouched.
        assert_eq!(
            simplify_bash_command("cargo test && cd a"),
            "cargo test && cd a"
        );
        assert_eq!(simplify_bash_command("echo cd a"), "echo cd a");
        // A bare `cd <path>` with no following command is a genuine command.
        assert_eq!(simplify_bash_command("cd /tmp"), "cd /tmp");
        // A no-cd command is unchanged.
        assert_eq!(simplify_bash_command("git status"), "git status");
    }

    #[test]
    fn bash_summary_collapses_worktree_paths() {
        assert_eq!(
            simplify_bash_command("cd /home/bot/wg/.wg-worktrees/agent-183 && cargo test"),
            "cargo test"
        );
        assert_eq!(
            collapse_worktree_paths("cat /home/bot/wg/.wg-worktrees/agent-183/src/foo.rs"),
            "cat …/src/foo.rs"
        );
        assert_eq!(
            collapse_worktree_paths("ls /home/bot/wg/.wg-worktrees/agent-183"),
            "ls …"
        );
    }

    #[test]
    fn bash_tool_start_strips_cd_prefix_on_live_stream() {
        let dir = tempfile::tempdir().unwrap();
        write_stream(
            dir.path(),
            "cd1",
            r#"{"type":"tool_execution_start","toolName":"bash","args":{"command":"cd /home/bot/wg/.wg-worktrees/agent-183 && cargo test --lib"}}"#,
        );
        assert_eq!(
            current_step(dir.path(), &task_with_agent("cd1")).as_deref(),
            Some("running cargo test --lib")
        );
    }
}
