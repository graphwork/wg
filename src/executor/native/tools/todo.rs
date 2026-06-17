//! `todo_write` tool: an in-context planning scratchpad.
//!
//! Mirrors Claude Code's `TodoWrite` (which maps here via the
//! `claude_code_alias` table in `mod.rs`): the model submits the **full**
//! todo list every call, we store it and echo back a rendered checklist so
//! the current plan stays visible in the conversation context.
//!
//! This is purely in-context state — no files are touched. Its value is
//! that it anchors explicit, up-front planning, which the Terminal-Bench
//! campaign found to be the single biggest score gain for small models
//! (see `docs/terminal-bench/REFERENCE-terminal-bench-campaign.md`).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;

use super::{Tool, ToolOutput, ToolRegistry};
use crate::executor::native::client::ToolDefinition;

/// One todo entry. We keep only the fields nex renders; unknown fields in
/// the model's input (e.g. Claude Code's `activeForm`) are accepted and
/// ignored so PascalCase-trained models don't error on shape drift.
#[derive(Clone)]
struct TodoItem {
    content: String,
    status: TodoStatus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    /// Parse a status string. Missing → `pending`. Returns `Err` with a
    /// helpful message for an unrecognized value so the model can correct.
    fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim) {
            None | Some("") | Some("pending") => Ok(Self::Pending),
            Some("in_progress") => Ok(Self::InProgress),
            Some("completed") => Ok(Self::Completed),
            Some(other) => Err(format!(
                "invalid status {other:?}; expected one of \
                 \"pending\", \"in_progress\", \"completed\""
            )),
        }
    }

    /// Checkbox marker for the rendered list.
    fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
        }
    }
}

/// Register the `todo_write` tool into the registry.
pub fn register_todo_write_tool(registry: &mut ToolRegistry) {
    registry.register(Box::new(TodoWriteTool {
        todos: Arc::new(Mutex::new(Vec::new())),
    }));
}

struct TodoWriteTool {
    /// Current plan. Each call replaces it wholesale (the model always
    /// sends the complete list), matching Claude Code's semantics.
    todos: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoWriteTool {
    /// Parse the `todos` array from tool input into validated items.
    fn parse_todos(input: &serde_json::Value) -> Result<Vec<TodoItem>, String> {
        let arr = input
            .get("todos")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                "missing required field \"todos\" (an array of {content, status} objects)"
                    .to_string()
            })?;

        let mut items = Vec::with_capacity(arr.len());
        for (i, entry) in arr.iter().enumerate() {
            let content = entry
                .get("content")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("todos[{i}] is missing a non-empty \"content\" string"))?;
            let status = TodoStatus::parse(entry.get("status").and_then(|v| v.as_str()))
                .map_err(|e| format!("todos[{i}]: {e}"))?;
            items.push(TodoItem {
                content: content.to_string(),
                status,
            });
        }
        Ok(items)
    }

    /// Render the stored list as a checklist with a one-line summary.
    fn render(items: &[TodoItem]) -> String {
        if items.is_empty() {
            return "Todo list cleared (0 items).".to_string();
        }
        let done = items
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count();
        let in_progress = items
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();
        let mut out = format!(
            "Todo list updated ({} items: {} completed, {} in progress, {} pending):\n",
            items.len(),
            done,
            in_progress,
            items.len() - done - in_progress,
        );
        for item in items {
            out.push_str(&format!("{} {}\n", item.status.marker(), item.content));
        }
        out
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_write".to_string(),
            description: "Record or update your task plan as a checklist. Submit the FULL list \
                          every call (it replaces the previous one). Use it to plan multi-step \
                          work up front and to mark progress as you go — exactly one item should \
                          be \"in_progress\" at a time. The plan stays visible in context."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The complete todo list. Replaces any previous list.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {
                                    "type": "string",
                                    "description": "What needs to be done."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "Defaults to \"pending\" if omitted."
                                }
                            },
                            "required": ["content"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let items = match Self::parse_todos(input) {
            Ok(items) => items,
            Err(e) => return ToolOutput::error(e),
        };
        let rendered = Self::render(&items);
        *self.todos.lock().await = items;
        ToolOutput::success(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> TodoWriteTool {
        TodoWriteTool {
            todos: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[tokio::test]
    async fn writes_and_renders_a_checklist() {
        let t = tool();
        let out = t
            .execute(&json!({
                "todos": [
                    {"content": "Write failing test", "status": "completed"},
                    {"content": "Implement tool", "status": "in_progress"},
                    {"content": "Run cargo test"}
                ]
            }))
            .await;

        assert!(!out.is_error, "valid input must succeed: {}", out.content);
        assert!(out.content.contains("3 items"));
        assert!(out.content.contains("1 completed"));
        assert!(out.content.contains("1 in progress"));
        assert!(out.content.contains("1 pending"));
        assert!(out.content.contains("[x] Write failing test"));
        assert!(out.content.contains("[~] Implement tool"));
        assert!(out.content.contains("[ ] Run cargo test"));

        // State was stored (full-list-replace semantics).
        assert_eq!(t.todos.lock().await.len(), 3);
    }

    #[tokio::test]
    async fn replaces_the_previous_list() {
        let t = tool();
        t.execute(&json!({"todos": [{"content": "old"}]})).await;
        t.execute(&json!({"todos": [{"content": "new a"}, {"content": "new b"}]}))
            .await;
        let stored = t.todos.lock().await;
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].content, "new a");
    }

    #[tokio::test]
    async fn missing_todos_field_is_an_error() {
        let out = tool().execute(&json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("todos"));
    }

    #[tokio::test]
    async fn invalid_status_is_an_error_naming_valid_values() {
        let out = tool()
            .execute(&json!({"todos": [{"content": "x", "status": "doing"}]}))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("in_progress"));
    }

    #[tokio::test]
    async fn empty_content_is_an_error() {
        let out = tool().execute(&json!({"todos": [{"content": "  "}]})).await;
        assert!(out.is_error);
        assert!(out.content.contains("content"));
    }

    #[tokio::test]
    async fn empty_list_clears() {
        let out = tool().execute(&json!({"todos": []})).await;
        assert!(!out.is_error);
        assert!(out.content.contains("cleared"));
    }
}
