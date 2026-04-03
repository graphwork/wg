//! Tool registry and dispatch for the native executor.
//!
//! Provides `ToolRegistry` that maps tool names to implementations,
//! generates JSON Schema definitions for the API, and dispatches calls.

pub mod bash;
pub mod file;
pub mod file_cache;
pub mod wg;

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use serde_json;

use super::client::ToolDefinition;

/// Output from executing a tool.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn success(content: String) -> Self {
        Self {
            content,
            is_error: false,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            content: message,
            is_error: true,
        }
    }
}

/// Trait that all tools must implement.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The tool's name (used for dispatch and API registration).
    fn name(&self) -> &str;

    /// JSON Schema definition for the API.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with the given JSON input.
    async fn execute(&self, input: &serde_json::Value) -> ToolOutput;
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// Get JSON Schema definitions for all registered tools (for API request).
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, input: &serde_json::Value) -> ToolOutput {
        match self.tools.get(name) {
            Some(tool) => tool.execute(input).await,
            None => ToolOutput::error(format!("Unknown tool: {}", name)),
        }
    }

    /// Create a filtered registry containing only the named tools.
    pub fn filter(mut self, allowed: &[String]) -> ToolRegistry {
        let wildcard = allowed.iter().any(|s| s == "*");
        if wildcard {
            return self;
        }

        let mut filtered = ToolRegistry::new();
        for name in allowed {
            if let Some(tool) = self.tools.remove(name) {
                filtered.tools.insert(name.clone(), tool);
            }
        }
        filtered
    }

    /// Create the full default registry with all tools.
    pub fn default_all(workgraph_dir: &Path, working_dir: &Path) -> Self {
        let mut registry = Self::new();

        // File tools
        file::register_file_tools(&mut registry);

        // Bash tool
        bash::register_bash_tool(&mut registry, working_dir.to_path_buf());

        // Workgraph tools
        wg::register_wg_tools(&mut registry, workgraph_dir.to_path_buf());

        registry
    }
}

/// Maximum tool output size (100KB) to prevent context overflow.
const MAX_TOOL_OUTPUT_SIZE: usize = 100 * 1024;

/// Truncate tool output if it exceeds the maximum size.
pub fn truncate_output(output: String) -> String {
    if output.len() > MAX_TOOL_OUTPUT_SIZE {
        let truncated = &output[..output.floor_char_boundary(MAX_TOOL_OUTPUT_SIZE)];
        format!(
            "{}\n\n[Output truncated: {} bytes total, showing first {}]",
            truncated,
            output.len(),
            MAX_TOOL_OUTPUT_SIZE
        )
    } else {
        output
    }
}

/// Per-tool output size limits for smart truncation.
pub struct ToolTruncationConfig {
    /// Maximum character count before truncation kicks in.
    pub max_chars: usize,
}

impl ToolTruncationConfig {
    /// Returns the truncation config for a given tool name.
    pub fn for_tool(tool_name: &str) -> Self {
        let max_chars = match tool_name {
            "bash" => 8_000,
            "read_file" => 16_000,
            "grep" => 4_000,
            "glob" => 4_000,
            "wg_show" => 2_000,
            "wg_list" => 4_000,
            _ => MAX_TOOL_OUTPUT_SIZE,
        };
        Self { max_chars }
    }
}

/// Smart truncation with head+tail preservation.
///
/// When output exceeds `max_chars`, shows the first half and last half
/// with an omission notice in between.
pub fn truncate_tool_output(output: &str, max_chars: usize) -> String {
    if output.len() <= max_chars {
        return output.to_string();
    }

    let total_chars = output.len();
    let total_lines = output.lines().count();

    let half = max_chars / 2;
    let head_end = output.floor_char_boundary(half);
    let raw_tail_start = total_chars.saturating_sub(half);
    let tail_start = output.floor_char_boundary(raw_tail_start).max(head_end);

    let head = &output[..head_end];
    let tail = &output[tail_start..];
    let omitted_chars = total_chars - head.len() - tail.len();
    let head_lines = head.lines().count();
    let tail_lines = tail.lines().count();
    let omitted_lines = total_lines.saturating_sub(head_lines + tail_lines);

    format!(
        "{}\n\n[... {} chars omitted ({} lines). \
         Showing first/last ~{} chars. \
         Use read_file or grep for specific content. ...]\n\n{}",
        head, omitted_chars, omitted_lines, half, tail
    )
}

/// Apply smart truncation for a specific tool type.
pub fn truncate_for_tool(output: &str, tool_name: &str) -> String {
    let config = ToolTruncationConfig::for_tool(tool_name);
    truncate_tool_output(output, config.max_chars)
}

#[cfg(test)]
mod truncation_tests {
    use super::*;

    #[test]
    fn test_truncation_under_limit_passthrough() {
        let short = "hello world";
        let result = truncate_tool_output(short, 8_000);
        assert_eq!(result, short);
    }

    #[test]
    fn test_truncation_exact_limit_passthrough() {
        let exact = "a".repeat(8_000);
        let result = truncate_tool_output(&exact, 8_000);
        assert_eq!(result, exact);
    }

    #[test]
    fn test_truncation_preserves_head_tail() {
        let head_content = "HEAD_START\n".repeat(100);
        let middle_content = "MIDDLE_FILLER\n".repeat(500);
        let tail_content = "TAIL_END\n".repeat(100);
        let full = format!("{}{}{}", head_content, middle_content, tail_content);

        let result = truncate_tool_output(&full, 4_000);

        assert!(result.starts_with("HEAD_START"));
        assert!(result.ends_with("TAIL_END\n"));
        assert!(result.contains("chars omitted"));
        assert!(result.contains("lines)"));
        assert!(result.contains("Use read_file or grep"));
        assert!(result.len() < full.len());
    }

    #[test]
    fn test_truncation_bash() {
        let config = ToolTruncationConfig::for_tool("bash");
        assert_eq!(config.max_chars, 8_000);

        let big_output = "x".repeat(10_000);
        let result = truncate_tool_output(&big_output, config.max_chars);
        assert!(result.contains("chars omitted"));
        assert!(result.starts_with("xxxx"));
        assert!(result.ends_with("xxxx"));
    }

    #[test]
    fn test_truncation_configs() {
        assert_eq!(ToolTruncationConfig::for_tool("bash").max_chars, 8_000);
        assert_eq!(ToolTruncationConfig::for_tool("read_file").max_chars, 16_000);
        assert_eq!(ToolTruncationConfig::for_tool("grep").max_chars, 4_000);
        assert_eq!(ToolTruncationConfig::for_tool("glob").max_chars, 4_000);
        assert_eq!(ToolTruncationConfig::for_tool("wg_show").max_chars, 2_000);
        assert_eq!(ToolTruncationConfig::for_tool("wg_list").max_chars, 4_000);
        assert_eq!(ToolTruncationConfig::for_tool("unknown").max_chars, MAX_TOOL_OUTPUT_SIZE);
    }

    #[test]
    fn test_truncation_omission_notice_has_counts() {
        let lines: Vec<String> = (0..1000)
            .map(|i| format!("line {}: some content here", i))
            .collect();
        let big = lines.join("\n");
        let result = truncate_tool_output(&big, 2_000);

        assert!(result.contains("chars omitted"));
        assert!(result.contains("lines)"));
    }

    #[test]
    fn test_truncation_multibyte_safe() {
        let content = "\u{1f980}".repeat(5000);
        let result = truncate_tool_output(&content, 4_000);
        assert!(result.contains("chars omitted"));
    }
}
