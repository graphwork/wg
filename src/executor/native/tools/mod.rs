//! Tool registry and dispatch for the native executor.
//!
//! Provides `ToolRegistry` that maps tool names to implementations,
//! generates JSON Schema definitions for the API, and dispatches calls.

pub mod bash;
pub mod bg;
pub mod chunk_map;
pub mod deep_research;
pub mod delegate;
pub mod file;
pub mod file_cache;
pub mod fuzzy_match;
pub mod helper_routing;
pub mod map;
pub mod progress;
pub mod reader;
pub mod research;
pub mod summarize;
pub mod todo;
pub mod web_fetch;
pub mod web_search;

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use futures_util::future::join_all;
use serde_json;
use tokio::sync::Semaphore;

use super::client::ToolDefinition;
use crate::config::NativeExecutorConfig;

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

/// Callback type for streaming tool output chunks.
pub type ToolStreamCallback = Box<dyn Fn(String) + Send + Sync>;

/// Trait that all tools must implement.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The tool's name (used for dispatch and API registration).
    fn name(&self) -> &str;

    /// JSON Schema definition for the API.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with the given JSON input.
    async fn execute(&self, input: &serde_json::Value) -> ToolOutput;

    /// Whether this tool is read-only (safe to execute concurrently).
    /// Read-only tools never modify files, state, or external systems.
    /// Default: false (conservative — unknown tools are treated as mutating).
    fn is_read_only(&self) -> bool {
        false
    }

    /// Execute the tool with streaming output support.
    ///
    /// The callback is invoked for each chunk of output as it arrives
    /// (e.g., each line for bash). Default implementation just calls
    /// `execute()` and streams nothing.
    async fn execute_streaming(
        &self,
        input: &serde_json::Value,
        on_chunk: ToolStreamCallback,
    ) -> ToolOutput {
        // Default: fall back to non-streaming
        let _ = on_chunk;
        self.execute(input).await
    }
}

/// Default maximum concurrent read-only tool executions.
pub const DEFAULT_MAX_CONCURRENT_TOOLS: usize = 10;

/// Canonical lean tool surface for `wg nex --minimal-tools`.
///
/// Single source of truth — both the CLI wiring (`src/commands/nex.rs`)
/// and its tests reference this list, so the allowlist cannot drift out
/// of sync with the registry. **Every name here MUST resolve to a tool
/// registered by `default_all_with_config_and_routing`** (enforced by
/// `minimal_tool_names_all_resolve`); adding a phantom name that no tool
/// implements would make `keep_only_tools` silently filter to nothing.
pub const MINIMAL_TOOL_NAMES: &[&str] = &[
    "read_file",
    "edit_file",
    "write_file",
    "bash",
    "grep",
    "glob",
    "todo_write",
];

/// Map a Claude Code PascalCase tool name to nex's canonical snake_case
/// tool name, or `None` if `name` is not a known alias.
///
/// nex's registered tools are snake_case (`read_file`, `edit_file`, …),
/// not Claude Code's PascalCase (`Read`, `Edit`, …). Models and harnesses
/// trained against Claude Code emit the PascalCase names; resolving them
/// here at dispatch time lets those prompts work unchanged without
/// doubling the advertised tool schema (the aliases are NOT added to
/// `definitions()`, so prefill cost is unaffected).
pub fn claude_code_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "Read" => "read_file",
        "Edit" => "edit_file",
        "Write" => "write_file",
        "Bash" => "bash",
        "Grep" => "grep",
        "Glob" => "glob",
        "TodoWrite" => "todo_write",
        "WebFetch" => "web_fetch",
        "WebSearch" => "web_search",
        _ => return None,
    })
}

/// A tool call request (name + input).
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub input: serde_json::Value,
}

/// Result of a single tool call within a batch.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub name: String,
    pub output: ToolOutput,
    pub duration_ms: u64,
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    /// Per-tool denylist, consulted before `execute` runs. When a
    /// tool is on this list, every call to it returns a
    /// `ToolOutput::error("permission denied: ...")` without
    /// invoking the tool's `execute`. The agent sees the error in
    /// its tool_result block and can adapt.
    permissions: crate::config::ToolPermissionsConfig,
    /// Non-destructive active-tool view. `None` = every registered tool
    /// is active (the full surface). `Some(set)` = only the canonical
    /// tool names in `set` are surfaced to the model (`definitions()`)
    /// and allowed to `execute()`; the rest are hidden but **not
    /// dropped**. Because the underlying `tools` map is never mutated,
    /// the surface can be toggled live (the `/tools` slash command,
    /// `set_active_allowlist` / `clear_active_allowlist`) without
    /// rebuilding the registry or re-running MCP discovery — unlike
    /// `keep_only_tools`, which is destructive. Names stored here are
    /// canonical (post-alias) tool names.
    active_allowlist: Option<std::collections::HashSet<String>>,
}

/// Whether `canonical_name` is active under an active-allowlist `view`.
/// `None` = full surface (everything active); `Some(set)` = active iff in
/// the set. Free function so the parallel-batch closures (which capture a
/// cloned view rather than borrowing `&self`) can consult the same logic
/// as [`ToolRegistry::is_active`].
fn active_with_view(
    view: &Option<std::collections::HashSet<String>>,
    canonical_name: &str,
) -> bool {
    match view {
        None => true,
        Some(set) => set.contains(canonical_name),
    }
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
            permissions: crate::config::ToolPermissionsConfig::default(),
            active_allowlist: None,
        }
    }

    /// Install a denylist-based permissions config on this
    /// registry. Subsequent `execute` / `execute_streaming` calls
    /// check each requested tool against the list before running.
    pub fn with_permissions(mut self, permissions: crate::config::ToolPermissionsConfig) -> Self {
        self.permissions = permissions;
        self
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// Whether `canonical_name` is currently surfaced/executable given the
    /// active-allowlist view. `None` view = everything active; `Some(set)`
    /// = active iff present in the set. `canonical_name` must already be
    /// alias-resolved (see `resolve_tool_name`).
    fn is_active(&self, canonical_name: &str) -> bool {
        active_with_view(&self.active_allowlist, canonical_name)
    }

    /// Get JSON Schema definitions for the **active** tools (for API
    /// request). When a lean view is in effect, hidden tools are omitted
    /// from the surface sent to the model so their schemas don't inflate
    /// prefill — without dropping the tools, so the view can be widened
    /// again live.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|(name, _)| self.is_active(name))
            .map(|(_, t)| t.definition())
            .collect()
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, input: &serde_json::Value) -> ToolOutput {
        let name = self.resolve_tool_name(name);
        if !self.is_active(name) {
            return ToolOutput::error(format!(
                "tool {:?} is not in the active tool surface (this session is running the minimal tool set). Use `/tools full` to expand the surface.",
                name
            ));
        }
        if self.permissions.is_denied(name) {
            return ToolOutput::error(format!(
                "permission denied: tool {:?} is on the deny list (see `[native_executor.permissions].deny_tools` in config.toml)",
                name
            ));
        }
        match self.tools.get(name) {
            Some(tool) => tool.execute(input).await,
            None => ToolOutput::error(format!("Unknown tool: {}", name)),
        }
    }

    /// Execute a tool by name with streaming output support.
    pub async fn execute_streaming(
        &self,
        name: &str,
        input: &serde_json::Value,
        on_chunk: ToolStreamCallback,
    ) -> ToolOutput {
        let name = self.resolve_tool_name(name);
        if !self.is_active(name) {
            return ToolOutput::error(format!(
                "tool {:?} is not in the active tool surface (this session is running the minimal tool set). Use `/tools full` to expand the surface.",
                name
            ));
        }
        if self.permissions.is_denied(name) {
            return ToolOutput::error(format!(
                "permission denied: tool {:?} is on the deny list",
                name
            ));
        }
        match self.tools.get(name) {
            Some(tool) => tool.execute_streaming(input, on_chunk).await,
            None => ToolOutput::error(format!("Unknown tool: {}", name)),
        }
    }

    /// Apply a lean active view limited to the given canonical tool names.
    /// Non-destructive: hidden tools stay registered and can be restored
    /// with [`clear_active_allowlist`](Self::clear_active_allowlist).
    /// Names not registered simply never surface. Used by the
    /// probe-driven minimal-tools default and the `/tools minimal`
    /// runtime toggle.
    pub fn set_active_allowlist(&mut self, names: &[&str]) {
        self.active_allowlist = Some(names.iter().map(|s| s.to_string()).collect());
    }

    /// Restore the full surface (drop the active view; every registered
    /// tool becomes active again). Used by the `/tools full` toggle.
    pub fn clear_active_allowlist(&mut self) {
        self.active_allowlist = None;
    }

    /// True when a lean (allowlisted) view is currently active.
    pub fn is_minimal_view_active(&self) -> bool {
        self.active_allowlist.is_some()
    }

    /// All registered tool names regardless of the active view, sorted.
    pub fn all_tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Tool names currently surfaced to the model (respecting the active
    /// view), sorted.
    pub fn active_tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tools
            .keys()
            .filter(|n| self.is_active(n))
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Create a filtered registry containing only the named tools.
    pub fn filter(mut self, allowed: &[String]) -> ToolRegistry {
        let wildcard = allowed.iter().any(|s| s == "*");
        if wildcard {
            return self;
        }

        let mut filtered = ToolRegistry {
            tools: HashMap::new(),
            // Preserve permissions across filter — explicit denies
            // survive narrowing so a caller can't accidentally
            // re-enable a denied tool by filtering down to a
            // narrower allow-list that happens to include it.
            permissions: self.permissions.clone(),
            // A freshly-filtered registry starts with the full view of
            // whatever survived the narrowing (no active-allowlist gate).
            active_allowlist: None,
        };
        for name in allowed {
            if let Some(tool) = self.tools.remove(name) {
                filtered.tools.insert(name.clone(), tool);
            }
        }
        filtered
    }

    /// Remove tools by name.
    pub fn remove_tools(&mut self, names: &[&str]) {
        for name in names {
            self.tools.remove(*name);
        }
    }

    /// Resolve a possibly-aliased tool name to the canonical name used
    /// for dispatch. A name that is already a registered tool is returned
    /// unchanged; otherwise a known Claude Code PascalCase alias
    /// (`Read` → `read_file`, …) is mapped to its canonical snake_case
    /// name. Unknown names pass through unchanged so the normal
    /// "Unknown tool" error reports what the model actually sent.
    pub fn resolve_tool_name<'a>(&self, name: &'a str) -> &'a str {
        if self.tools.contains_key(name) {
            return name;
        }
        claude_code_alias(name).unwrap_or(name)
    }

    /// Keep only the specified tools by name, removing all others.
    /// Used by `wg nex --minimal-tools` to provide a lean tool surface
    /// for small local models (reduces prefill cost).
    pub fn keep_only_tools(&mut self, names: &[&str]) {
        let keep_set: std::collections::HashSet<&str> = names.iter().copied().collect();
        self.tools
            .retain(|name, _| keep_set.contains(name.as_str()));
    }

    /// Check whether a tool is read-only by name. Resolves aliases so
    /// an aliased call (e.g. `Read`) is classified by its canonical
    /// tool's read-only flag for batch parallelism.
    pub fn is_read_only(&self, name: &str) -> bool {
        let name = self.resolve_tool_name(name);
        self.tools.get(name).is_some_and(|t| t.is_read_only())
    }

    /// Return a new registry containing only read-only tools. Used by
    /// `wg nex --read-only` to provide a safe browsing mode where the
    /// agent can read files, search the web, and run non-destructive
    /// commands but cannot write files, edit code, or run arbitrary
    /// bash that modifies state.
    pub fn filter_read_only(self) -> Self {
        let mut filtered = ToolRegistry {
            tools: HashMap::new(),
            // Preserve permissions — same rationale as `filter`.
            permissions: self.permissions.clone(),
            // Read-only filtering is a destructive narrowing; the result
            // starts with the full view of whatever read-only tools remain.
            active_allowlist: None,
        };
        for (name, tool) in self.tools {
            if tool.is_read_only() {
                filtered.tools.insert(name, tool);
            }
        }
        filtered
    }

    /// Execute a batch of tool calls with parallelism for read-only tools.
    ///
    /// Partitions calls into read-only and mutating. Read-only calls execute
    /// concurrently (up to `max_concurrent`), then mutating calls execute serially.
    /// Results are returned in the original call order.
    pub async fn execute_batch(
        &self,
        calls: &[ToolCall],
        max_concurrent: usize,
    ) -> Vec<ToolCallResult> {
        // Separate into (index, call) pairs by type
        let mut read_only: Vec<(usize, &ToolCall)> = Vec::new();
        let mut mutating: Vec<(usize, &ToolCall)> = Vec::new();

        for (i, call) in calls.iter().enumerate() {
            if self.is_read_only(&call.name) {
                read_only.push((i, call));
            } else {
                mutating.push((i, call));
            }
        }

        let mut results: Vec<(usize, ToolCallResult)> = Vec::with_capacity(calls.len());

        // Execute read-only calls concurrently with semaphore-based cap.
        // Uses join_all (not tokio::spawn) so we borrow &self without 'static.
        if !read_only.is_empty() {
            let semaphore = Semaphore::new(max_concurrent);

            let futures: Vec<_> = read_only
                .iter()
                .map(|(idx, call)| {
                    let sem = &semaphore;
                    async move {
                        let _permit = sem.acquire().await.unwrap();
                        let start = std::time::Instant::now();
                        let resolved = self.resolve_tool_name(&call.name);
                        let output = if !self.is_active(resolved) {
                            ToolOutput::error(format!(
                                "tool {:?} is not in the active tool surface (this session is running the minimal tool set). Use `/tools full` to expand the surface.",
                                resolved
                            ))
                        } else {
                            match self.tools.get(resolved) {
                                Some(tool) => tool.execute(&call.input).await,
                                None => {
                                    ToolOutput::error(format!("Unknown tool: {}", call.name))
                                }
                            }
                        };
                        let duration_ms = start.elapsed().as_millis() as u64;
                        (
                            *idx,
                            ToolCallResult {
                                name: call.name.clone(),
                                output,
                                duration_ms,
                            },
                        )
                    }
                })
                .collect();

            let read_results = join_all(futures).await;
            results.extend(read_results);
        }

        // Execute mutating calls serially
        for (idx, call) in &mutating {
            let start = std::time::Instant::now();
            let output = self.execute(&call.name, &call.input).await;
            let duration_ms = start.elapsed().as_millis() as u64;
            results.push((
                *idx,
                ToolCallResult {
                    name: call.name.clone(),
                    output,
                    duration_ms,
                },
            ));
        }

        // Sort by original index to maintain call order
        results.sort_by_key(|(idx, _)| *idx);
        results.into_iter().map(|(_, r)| r).collect()
    }

    /// Execute a batch of tool calls with streaming output for each tool.
    ///
    /// Each tool's output is streamed via its own callback. This is used for
    /// bash tools where we want to see incremental output.
    pub async fn execute_batch_streaming(
        &self,
        calls: &[ToolCall],
        max_concurrent: usize,
        make_stream_callback: impl Fn(usize) -> ToolStreamCallback + Clone,
    ) -> Vec<ToolCallResult> {
        use std::sync::Arc;

        // Separate into (index, call) pairs by type
        let mut read_only: Vec<(usize, &ToolCall)> = Vec::new();
        let mut mutating: Vec<(usize, &ToolCall)> = Vec::new();

        for (i, call) in calls.iter().enumerate() {
            if self.is_read_only(&call.name) {
                read_only.push((i, call));
            } else {
                mutating.push((i, call));
            }
        }

        let mut results: Vec<(usize, ToolCallResult)> = Vec::with_capacity(calls.len());

        // Execute read-only calls concurrently with semaphore-based cap.
        if !read_only.is_empty() {
            let semaphore = Arc::new(Semaphore::new(max_concurrent));
            let tools = Arc::new(&self.tools);
            // Capture the active-allowlist view so each closure can gate
            // hidden tools without borrowing `&self` (mirrors
            // `is_active`); the parallel closures only hold `tools`.
            let view = Arc::new(self.active_allowlist.clone());

            let futures: Vec<_> = read_only
                .iter()
                .map(|(idx, call)| {
                    let sem = Arc::clone(&semaphore);
                    let tools = Arc::clone(&tools);
                    let view = Arc::clone(&view);
                    let cb = make_stream_callback(*idx);
                    async move {
                        let _permit = sem.acquire().await.unwrap();
                        let start = std::time::Instant::now();
                        // Resolve Claude Code aliases against the captured
                        // tool map (mirrors `resolve_tool_name`, which needs
                        // `&self` we don't hold inside this closure).
                        let resolved = if tools.contains_key(&call.name) {
                            call.name.as_str()
                        } else {
                            claude_code_alias(&call.name).unwrap_or(call.name.as_str())
                        };
                        let output = if !active_with_view(&view, resolved) {
                            ToolOutput::error(format!(
                                "tool {:?} is not in the active tool surface (this session is running the minimal tool set). Use `/tools full` to expand the surface.",
                                resolved
                            ))
                        } else {
                            match tools.get(resolved) {
                                Some(tool) => tool.execute_streaming(&call.input, cb).await,
                                None => {
                                    ToolOutput::error(format!("Unknown tool: {}", call.name))
                                }
                            }
                        };
                        let duration_ms = start.elapsed().as_millis() as u64;
                        (
                            *idx,
                            ToolCallResult {
                                name: call.name.clone(),
                                output,
                                duration_ms,
                            },
                        )
                    }
                })
                .collect();

            let read_results = join_all(futures).await;
            results.extend(read_results);
        }

        // Execute mutating calls serially
        for (idx, call) in &mutating {
            let start = std::time::Instant::now();
            let cb = make_stream_callback(*idx);
            let output = self.execute_streaming(&call.name, &call.input, cb).await;
            let duration_ms = start.elapsed().as_millis() as u64;
            results.push((
                *idx,
                ToolCallResult {
                    name: call.name.clone(),
                    output,
                    duration_ms,
                },
            ));
        }

        // Sort by original index to maintain call order
        results.sort_by_key(|(idx, _)| *idx);
        results.into_iter().map(|(_, r)| r).collect()
    }

    /// Create the full default registry with all tools.
    ///
    /// Uses default config values. Prefer `default_all_with_config` when a
    /// `NativeExecutorConfig` is available.
    pub fn default_all(workgraph_dir: &Path, working_dir: &Path) -> Self {
        Self::default_all_with_config(workgraph_dir, working_dir, &NativeExecutorConfig::default())
    }

    /// Create the full default registry with all tools, using the given config.
    pub fn default_all_with_config(
        workgraph_dir: &Path,
        working_dir: &Path,
        config: &NativeExecutorConfig,
    ) -> Self {
        Self::default_all_with_config_and_routing(
            workgraph_dir,
            working_dir,
            config,
            helper_routing::HelperRouting::default(),
        )
    }

    /// Create the full default registry with all tools, using the given
    /// config and active parent-session route for helper LLM calls.
    pub fn default_all_with_config_and_routing(
        workgraph_dir: &Path,
        working_dir: &Path,
        config: &NativeExecutorConfig,
        routing: helper_routing::HelperRouting,
    ) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            permissions: config.permissions.clone(),
            // Full surface by default; the probe-driven minimal-tools
            // default (and the `/tools` toggle) install a view later.
            active_allowlist: None,
        };

        // File tools
        file::register_file_tools(&mut registry);

        // Bash tool
        bash::register_bash_tool(&mut registry, working_dir.to_path_buf());

        // todo_write: in-context planning scratchpad (part of the minimal
        // local-dev surface; also the canonical target of the `TodoWrite`
        // Claude Code alias).
        todo::register_todo_write_tool(&mut registry);

        // Web search tool
        web_search::register_web_search_tool(&mut registry);

        // Arxiv search tool (separate from web_search fan-out — scholarly-only)
        web_search::register_arxiv_search_tool(&mut registry);

        // Web fetch tool
        web_fetch::register_web_fetch_tool_with_config(
            &mut registry,
            workgraph_dir.to_path_buf(),
            config.web.fetch_max_chars,
            config.web.fetch_timeout_secs,
        );

        // Background job tool
        bg::register_bg_tool(&mut registry, workgraph_dir.to_path_buf());

        // Delegate tool (in-process subtask delegation)
        delegate::register_delegate_tool_with_config(
            &mut registry,
            workgraph_dir.to_path_buf(),
            working_dir.to_path_buf(),
            config.delegate.delegate_max_turns,
            &config.delegate.delegate_model,
            routing.clone(),
        );

        // Summarize tool (recursive map-reduce summarization). Uses the
        // delegate model override if set, falling back to the active
        // parent-session model/endpoint at execute time.
        summarize::register_summarize_tool_with_routing(
            &mut registry,
            workgraph_dir.to_path_buf(),
            config.delegate.delegate_model.clone(),
            routing,
        );

        // Research tool (high-level: search + fetch + summarize in one call)
        research::register_research_tool(&mut registry, workgraph_dir.to_path_buf());

        // Deep research (decompose → fan out via research → synthesize)
        deep_research::register_deep_research_tool(&mut registry, workgraph_dir.to_path_buf());

        // Reader (sub-executor with working dir for large-file survey)
        reader::register_reader_tool(&mut registry, workgraph_dir.to_path_buf());

        // Map (sub-executor per item over a list of inputs)
        map::register_map_tool(&mut registry, workgraph_dir.to_path_buf());

        // chunk_map (split a file/text, then map a sub-agent over each chunk)
        chunk_map::register_chunk_map_tool(&mut registry, workgraph_dir.to_path_buf());

        // Note: what was `survey_file` is now split across two entry points:
        // - `read_file(path, query=...)` — single-shot LLM query, fails loudly
        //   if the file doesn't fit in one call
        // - `reader(path, task)` — sub-executor with working dir, for
        //   arbitrarily large files or tasks that want a persistent workspace

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
            "web_search" => 16_000,
            "web_fetch" => 16_000,
            "delegate" => 8_000,
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
        "{}\n\n[... {} chars omitted ({} lines). Showing first/last ~{} chars only. \
         To continue reading from the gap: call `read_file` again with `offset` set to \
         just past the head (the line numbers above tell you where). \
         For LLM-summarized content over the whole file: pass the path to `summarize` \
         (map-reduce summary) or `reader` (multi-turn traversal with a working dir). \
         For keyword lookup without reading: use `grep`. ...]\n\n{}",
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
        // Footer nudges the agent toward continuation paths.
        assert!(result.contains("offset"));
        assert!(result.contains("summarize"));
        assert!(result.contains("reader"));
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
        assert_eq!(
            ToolTruncationConfig::for_tool("read_file").max_chars,
            16_000
        );
        assert_eq!(ToolTruncationConfig::for_tool("grep").max_chars, 4_000);
        assert_eq!(ToolTruncationConfig::for_tool("glob").max_chars, 4_000);
        assert_eq!(
            ToolTruncationConfig::for_tool("unknown").max_chars,
            MAX_TOOL_OUTPUT_SIZE
        );
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

#[cfg(test)]
mod parallelism_tests {
    use super::*;

    /// Minimal test tool for unit tests.
    struct TestTool {
        tool_name: String,
        read_only: bool,
    }

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn definition(&self) -> crate::executor::native::client::ToolDefinition {
            crate::executor::native::client::ToolDefinition {
                name: self.tool_name.clone(),
                description: "test".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }

        fn is_read_only(&self) -> bool {
            self.read_only
        }

        async fn execute(&self, _input: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(format!("ok-{}", self.tool_name))
        }
    }

    #[test]
    fn test_is_read_only_query() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            tool_name: "reader".to_string(),
            read_only: true,
        }));
        registry.register(Box::new(TestTool {
            tool_name: "writer".to_string(),
            read_only: false,
        }));

        assert!(registry.is_read_only("reader"));
        assert!(!registry.is_read_only("writer"));
        assert!(!registry.is_read_only("missing"));
    }

    #[tokio::test]
    async fn denied_tool_returns_error_without_executing() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            tool_name: "dangerous".to_string(),
            read_only: false,
        }));
        registry.register(Box::new(TestTool {
            tool_name: "safe".to_string(),
            read_only: true,
        }));
        let registry = registry.with_permissions(crate::config::ToolPermissionsConfig {
            deny_tools: vec!["dangerous".to_string()],
        });

        // Denied tool: error surfaces to agent, doesn't execute.
        let out = registry.execute("dangerous", &serde_json::json!({})).await;
        assert!(out.is_error, "denied tool must return an error");
        assert!(
            out.content.contains("permission denied"),
            "error message must mention permission denied, got: {}",
            out.content
        );

        // Non-denied tool still works.
        let out = registry.execute("safe", &serde_json::json!({})).await;
        assert!(!out.is_error);
        assert_eq!(out.content, "ok-safe");

        // Denial also applies to execute_streaming.
        let cb: ToolStreamCallback = Box::new(|_| {});
        let out = registry
            .execute_streaming("dangerous", &serde_json::json!({}), cb)
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("permission denied"));
    }

    #[test]
    fn permissions_survive_filter() {
        // Filtering down to a narrower allow-list must NOT drop the
        // denylist — otherwise a caller could bypass denies by
        // filtering to an allow-list that happens to include the
        // denied tool.
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            tool_name: "dangerous".to_string(),
            read_only: false,
        }));
        let registry = registry
            .with_permissions(crate::config::ToolPermissionsConfig {
                deny_tools: vec!["dangerous".to_string()],
            })
            .filter(&["dangerous".to_string()]);
        assert!(registry.permissions.is_denied("dangerous"));
    }

    #[test]
    fn permissions_survive_filter_read_only() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            tool_name: "reader".to_string(),
            read_only: true,
        }));
        let registry = registry
            .with_permissions(crate::config::ToolPermissionsConfig {
                deny_tools: vec!["reader".to_string()],
            })
            .filter_read_only();
        assert!(registry.permissions.is_denied("reader"));
    }

    #[tokio::test]
    async fn test_execute_batch_preserves_order() {
        let mut registry = ToolRegistry::new();
        for name in &["a", "b", "c"] {
            registry.register(Box::new(TestTool {
                tool_name: name.to_string(),
                read_only: true,
            }));
        }

        let calls = vec![
            ToolCall {
                name: "c".to_string(),
                input: serde_json::json!({}),
            },
            ToolCall {
                name: "a".to_string(),
                input: serde_json::json!({}),
            },
            ToolCall {
                name: "b".to_string(),
                input: serde_json::json!({}),
            },
        ];

        let results = registry.execute_batch(&calls, 10).await;
        assert_eq!(results[0].name, "c");
        assert_eq!(results[1].name, "a");
        assert_eq!(results[2].name, "b");
    }

    #[tokio::test]
    async fn test_execute_batch_empty() {
        let registry = ToolRegistry::new();
        let results = registry.execute_batch(&[], 10).await;
        assert!(results.is_empty());
    }

    #[test]
    fn test_default_max_concurrent() {
        assert_eq!(DEFAULT_MAX_CONCURRENT_TOOLS, 10);
    }

    /// The `--minimal-tools` allowlist must contain no phantom names:
    /// every entry has to resolve to a tool the full default registry
    /// actually registers. Otherwise `keep_only_tools` silently filters
    /// the surface down to a name that never existed (the `todo_write`
    /// bug this guards against).
    #[test]
    fn minimal_tool_names_all_resolve() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let registry = ToolRegistry::default_all(tmp.path(), tmp.path());
        let registered: std::collections::HashSet<String> =
            registry.definitions().into_iter().map(|d| d.name).collect();

        for name in MINIMAL_TOOL_NAMES {
            assert!(
                registered.contains(*name),
                "minimal-tools allowlist names a tool that is not registered: {:?} \
                 (keep_only_tools would silently drop it). Registered: {:?}",
                name,
                registered
            );
        }
    }

    /// The active-allowlist view toggles the surface non-destructively:
    /// full -> minimal -> full restores every tool WITHOUT rebuilding the
    /// registry (the underlying `Box<dyn Tool>` instances are never
    /// dropped). This is the property that lets `/tools` switch live.
    #[test]
    fn active_allowlist_view_toggles_non_destructively() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let mut registry = ToolRegistry::default_all(tmp.path(), tmp.path());

        // Full surface to start: no view, definitions == all registered.
        assert!(!registry.is_minimal_view_active());
        let full: std::collections::HashSet<String> =
            registry.definitions().into_iter().map(|d| d.name).collect();
        assert!(full.contains("web_fetch") && full.contains("delegate"));
        let full_count = full.len();
        assert!(
            full_count > MINIMAL_TOOL_NAMES.len(),
            "full surface should be wider than the minimal allowlist"
        );

        // Install the lean view.
        registry.set_active_allowlist(MINIMAL_TOOL_NAMES);
        assert!(registry.is_minimal_view_active());
        let minimal: std::collections::HashSet<String> =
            registry.definitions().into_iter().map(|d| d.name).collect();
        let expected: std::collections::HashSet<String> =
            MINIMAL_TOOL_NAMES.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            minimal, expected,
            "lean view surfaces exactly the allowlist"
        );
        assert!(!minimal.contains("web_fetch"));
        // Non-destructive: every tool is still registered under the hood.
        assert_eq!(registry.all_tool_names().len(), full_count);

        // Toggle back to full — all tools restored, no rebuild.
        registry.clear_active_allowlist();
        assert!(!registry.is_minimal_view_active());
        let restored: std::collections::HashSet<String> =
            registry.definitions().into_iter().map(|d| d.name).collect();
        assert_eq!(
            restored, full,
            "full -> minimal -> full restores every tool"
        );
    }

    /// A tool hidden by the active view cannot be executed — `execute`
    /// returns an error pointing at `/tools full`, even though the tool is
    /// still registered. Defense-in-depth for a model that calls a tool it
    /// can no longer see.
    #[tokio::test]
    async fn hidden_tool_is_not_executable_under_lean_view() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            tool_name: "read_file".to_string(),
            read_only: true,
        }));
        registry.register(Box::new(TestTool {
            tool_name: "web_fetch".to_string(),
            read_only: true,
        }));
        registry.set_active_allowlist(&["read_file"]);

        // Visible tool still executes.
        let ok = registry.execute("read_file", &serde_json::json!({})).await;
        assert!(!ok.is_error, "active tool must run: {}", ok.content);

        // Hidden tool is refused with a discoverability hint.
        let blocked = registry.execute("web_fetch", &serde_json::json!({})).await;
        assert!(blocked.is_error);
        assert!(
            blocked.content.contains("active tool surface")
                && blocked.content.contains("/tools full"),
            "hidden-tool error should point at /tools full: {}",
            blocked.content
        );
    }

    /// An aliased call must dispatch to the canonical tool's `execute`,
    /// not fall through to the "Unknown tool" path.
    #[tokio::test]
    async fn aliased_call_dispatches_to_canonical_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            tool_name: "read_file".to_string(),
            read_only: true,
        }));

        let out = registry.execute("Read", &serde_json::json!({})).await;
        assert!(!out.is_error, "aliased call must dispatch: {}", out.content);
        assert_eq!(out.content, "ok-read_file");

        // Aliased classification routes through the read-only fast path.
        assert!(registry.is_read_only("Read"));

        // A denylist on the canonical name also blocks the alias.
        let denied = registry.with_permissions(crate::config::ToolPermissionsConfig {
            deny_tools: vec!["read_file".to_string()],
        });
        let out = denied.execute("Read", &serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("permission denied"));
    }

    /// Claude Code PascalCase tool names must resolve to nex's snake_case
    /// tools so prompts/harnesses written against Claude Code work
    /// unchanged. Aliases resolve at dispatch only — they are NOT added
    /// to the advertised tool surface.
    #[test]
    fn claude_code_aliases_resolve() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let registry = ToolRegistry::default_all(tmp.path(), tmp.path());

        for (alias, canonical) in [
            ("Read", "read_file"),
            ("Edit", "edit_file"),
            ("Write", "write_file"),
            ("Bash", "bash"),
            ("Grep", "grep"),
            ("Glob", "glob"),
            ("TodoWrite", "todo_write"),
        ] {
            assert_eq!(
                registry.resolve_tool_name(alias),
                canonical,
                "alias {alias:?} should resolve to {canonical:?}"
            );
        }

        // A canonical name passes through unchanged.
        assert_eq!(registry.resolve_tool_name("read_file"), "read_file");
        // An unknown name passes through unchanged (lets the normal
        // "Unknown tool" error path report the real name).
        assert_eq!(registry.resolve_tool_name("not_a_tool"), "not_a_tool");

        // Aliases must NOT bloat the advertised surface.
        let names: std::collections::HashSet<String> =
            registry.definitions().into_iter().map(|d| d.name).collect();
        assert!(!names.contains("Read"), "alias must not be advertised");
    }
}
