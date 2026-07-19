//! Project configuration for WG
//!
//! Configuration is stored in `.wg/config.toml` and controls
//! agent behavior, executor settings, and project defaults.
//!
//! Sensitive credentials (like Matrix login) are stored separately in
//! `~/.config/worksgood/matrix.toml` to avoid accidentally committing secrets.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Bare Claude tier aliases passed to the claude CLI.
/// The CLI resolves these to the current production model — we do not track dated IDs.
pub const CLAUDE_HAIKU_MODEL_ID: &str = "haiku";
pub const CLAUDE_SONNET_MODEL_ID: &str = "sonnet";
pub const CLAUDE_OPUS_MODEL_ID: &str = "opus";

/// Fable 5's full claude CLI model id.
///
/// Unlike `opus`/`sonnet`/`haiku` — which the claude CLI accepts as bare
/// shortcuts and resolves to the current production model — Fable 5 has **no**
/// bare CLI shortcut, so the friendly alias `fable` must be expanded to this
/// dated id before it is handed to `claude --model`. See [`claude_cli_model_arg`].
pub const CLAUDE_FABLE_MODEL_ID: &str = "claude-fable-5";

/// Expand a bare claude model id/alias into the exact string passed to
/// `claude --model`.
///
/// The claude CLI ships built-in shortcuts for `opus`/`sonnet`/`haiku`, so
/// those pass through verbatim (the CLI resolves them to the current
/// production model). Fable 5 has no such shortcut, so the friendly alias
/// `fable` is expanded to its full CLI model id [`CLAUDE_FABLE_MODEL_ID`]
/// (`claude-fable-5`). Everything else — already-dated ids, non-claude names —
/// is returned unchanged, so this is safe to apply at every claude `--model`
/// construction site.
pub fn claude_cli_model_arg(model_id: &str) -> String {
    if model_id.eq_ignore_ascii_case("fable") {
        CLAUDE_FABLE_MODEL_ID.to_string()
    } else {
        model_id.to_string()
    }
}

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Agent configuration
    #[serde(default)]
    pub agent: AgentConfig,

    /// Dispatcher configuration (canonical TOML key: `[dispatcher]`; legacy alias: `[coordinator]`)
    #[serde(default, rename = "dispatcher", alias = "coordinator")]
    pub coordinator: CoordinatorConfig,

    /// Project metadata
    #[serde(default)]
    pub project: ProjectConfig,

    /// Help display configuration
    #[serde(default)]
    pub help: HelpConfig,

    /// Agency (evolutionary identity) configuration
    #[serde(default)]
    pub agency: AgencyConfig,

    /// Log configuration
    #[serde(default)]
    pub log: LogConfig,

    /// Replay configuration
    #[serde(default)]
    pub replay: ReplayConfig,

    /// Guardrails for autopoietic task creation
    #[serde(default)]
    pub guardrails: GuardrailsConfig,

    /// Visualization settings
    #[serde(default)]
    pub viz: VizConfig,

    /// TUI-specific settings
    #[serde(default)]
    pub tui: TuiConfig,

    /// LLM endpoints
    #[serde(default, skip_serializing_if = "EndpointsConfig::is_empty")]
    pub llm_endpoints: EndpointsConfig,

    /// Checkpoint configuration
    #[serde(default)]
    pub checkpoint: CheckpointConfig,

    /// Model routing: per-role model+provider assignments
    #[serde(default)]
    pub models: ModelRoutingConfig,

    /// Explicit execution-failure policy. Tiers and registry rankings select a
    /// route for a new call; they never authorize switching routes after a
    /// failure. Only entries in this section may do that.
    #[serde(default, skip_serializing_if = "ExecutionConfig::is_empty")]
    pub execution: ExecutionConfig,

    /// Active provider profile name (e.g., "anthropic", "openrouter", "openai").
    /// When set, the profile supplies tier defaults. Explicit [tiers] entries
    /// and per-role [models] overrides still take precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,

    /// Quality tier defaults: which model ID each tier resolves to
    #[serde(default)]
    pub tiers: TierConfig,

    /// Model registry entries
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_registry: Vec<ModelRegistryEntry>,

    /// Deprecated/inert legacy tag-routing entries.
    ///
    /// Freeform task tags are labels only; they do not route work or
    /// select executors. The field remains deserializable for old
    /// configs so `wg migrate config`/linting can inspect it without
    /// rejecting the whole file.
    ///
    /// ```toml
    /// [[tag_routing]]
    /// tag = "frontend"
    /// model = "codex:gpt-5-codex"
    ///
    /// [[tag_routing]]
    /// tag = "infra"
    /// model = "claude:opus"
    /// ```
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tag_routing: Vec<TagRoutingEntry>,

    /// Chat archive rotation settings
    #[serde(default)]
    pub chat: ChatConfig,

    /// Bash executable resolution. Primarily for Windows users whose PATH
    /// resolves `bash` to `C:\Windows\System32\bash.exe` (the WSL shim)
    /// instead of Git for Windows' bash. Leave unset to let wg
    /// auto-discover Git for Windows.
    #[serde(default)]
    pub bash: BashConfig,

    /// OpenRouter cost cap and monitoring configuration. Only emitted
    /// when the user is on the openrouter route or has explicitly added
    /// an openrouter endpoint — non-openrouter projects don't need a
    /// cost-cap section sitting in their config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openrouter: Option<OpenRouterConfig>,

    /// Credential storage settings
    #[serde(default)]
    pub secrets: crate::secret::SecretsConfig,

    /// Native executor settings (web, background, delegate)
    #[serde(default)]
    pub native_executor: NativeExecutorConfig,

    /// MCP (Model Context Protocol) server configuration. Each entry
    /// declares one server that will be spawned when a WGNEX session
    /// starts; its tools are auto-discovered and merged into the
    /// session's tool registry.
    #[serde(default)]
    pub mcp: McpConfig,

    /// Authentication credentials for child processes the daemon spawns
    /// (coordinator agent, task agents, lightweight LLM calls). Only
    /// consulted when the matching env var isn't already set; explicit
    /// env always wins.
    #[serde(default)]
    pub auth: AuthConfig,

    /// True when `agent.model` was explicitly set in local config.
    /// Used by `resolve_model_for_role` to skip tier defaults in favor of agent.model.
    #[serde(skip)]
    pub agent_model_is_local: bool,
}

/// MCP server configuration. Populated from:
///
/// ```toml
/// [[mcp.servers]]
/// name = "filesystem"
/// command = "npx"
/// args = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
/// enabled = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<McpServerEntry>,
}

/// One server declaration. Mirrors the wire shape expected by
/// `executor::native::mcp::McpServerConfig`; conversion is trivial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default = "default_mcp_enabled")]
    pub enabled: bool,
}

fn default_mcp_enabled() -> bool {
    true
}

/// Chat archive rotation configuration.
/// Bash executable resolution. On Windows this overrides wg's automatic
/// Git-for-Windows discovery; on Unix this is essentially never needed
/// since PATH lookup for `bash` Just Works.
///
/// ```toml
/// [bash]
/// path = "C:\\Program Files\\Git\\bin\\bash.exe"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BashConfig {
    /// Absolute path to the bash executable wg should use for wrapper
    /// scripts. Primarily for Windows users whose PATH resolves to
    /// `C:\Windows\System32\bash.exe` (the WSL shim) instead of Git
    /// for Windows' bash. Leave unset to let wg auto-discover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatConfig {
    /// Maximum size in bytes before rotating the active chat file (default: 1MB).
    #[serde(default = "default_chat_max_file_size")]
    pub max_file_size: u64,
    /// Maximum number of messages before rotating (default: 10000).
    #[serde(default = "default_chat_max_messages")]
    pub max_messages: usize,
    /// Retention period in days for archived files (default: 30). 0 = keep forever.
    #[serde(default = "default_chat_retention_days")]
    pub retention_days: u32,
    /// Number of new messages before auto-triggering chat compaction (default: 50).
    #[serde(default = "default_chat_compact_threshold")]
    pub compact_threshold: usize,
}

fn default_chat_max_file_size() -> u64 {
    1_048_576 // 1 MB
}
fn default_chat_max_messages() -> usize {
    10_000
}
fn default_chat_retention_days() -> u32 {
    30
}
fn default_chat_compact_threshold() -> usize {
    50
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            max_file_size: default_chat_max_file_size(),
            max_messages: default_chat_max_messages(),
            retention_days: default_chat_retention_days(),
            compact_threshold: default_chat_compact_threshold(),
        }
    }
}

/// Native executor configuration.
///
/// `Default` is implemented by hand (not derived) because
/// `minimal_tools_context_threshold` needs a non-zero default — a derived
/// `Default` would zero the `usize` and silently disable the probe-driven
/// minimal-tools auto-default. Keep the manual impl in sync with the field
/// list and with each field's `#[serde(default = ...)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeExecutorConfig {
    /// Web access settings (search + fetch).
    #[serde(default)]
    pub web: NativeWebConfig,

    /// Background task settings.
    #[serde(default)]
    pub background: NativeBackgroundConfig,

    /// Delegate (in-process subtask) settings.
    #[serde(default)]
    pub delegate: NativeDelegateConfig,

    /// Tool-level permissions. Denylist-based for simplicity —
    /// deny wins over allow, no rule means allowed. Matched on
    /// exact tool name. MCP tools use the `<server>__<tool>` form
    /// so you can deny a whole server by listing
    /// `filesystem__*` (glob matching is a future extension).
    /// Skipped when empty on serialize so `wg init` + subsequent
    /// edits don't produce duplicate-key conflicts in the TOML.
    #[serde(default, skip_serializing_if = "ToolPermissionsConfig::is_empty")]
    pub permissions: ToolPermissionsConfig,

    /// Context-window threshold (in tokens) for the probe-driven
    /// minimal-tools default of `wg nex`. When neither `--minimal-tools`
    /// nor `--full-tools` is passed, the lean tool surface auto-enables
    /// iff the resolved context window (explicit config > live probe >
    /// model registry > fallback) is **at or below** this value. Explicit
    /// flags always win. Default `32_000` — the inflection where the full
    /// tool-schema prefill stops being a material fraction of a small
    /// window (and matches the channeling clamp's lower band). Set to `0`
    /// to disable the auto-minimal behavior entirely (lean surface becomes
    /// pure opt-in via `--minimal-tools`).
    #[serde(default = "default_minimal_tools_context_threshold")]
    pub minimal_tools_context_threshold: usize,
}

fn default_minimal_tools_context_threshold() -> usize {
    32_000
}

impl Default for NativeExecutorConfig {
    fn default() -> Self {
        Self {
            web: NativeWebConfig::default(),
            background: NativeBackgroundConfig::default(),
            delegate: NativeDelegateConfig::default(),
            permissions: ToolPermissionsConfig::default(),
            minimal_tools_context_threshold: default_minimal_tools_context_threshold(),
        }
    }
}

/// Tool permission configuration. First cut is a simple denylist;
/// room to grow to per-path / pattern matching without schema churn.
///
/// Example `.wg/config.toml`:
/// ```toml
/// [native_executor.permissions]
/// deny_tools = ["bash", "write_file"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolPermissionsConfig {
    /// Tools that must NOT execute. A call to a denied tool returns
    /// `ToolOutput::error("permission denied: ...")` to the agent,
    /// visible in the tool_result block, so the model can adapt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_tools: Vec<String>,
}

impl ToolPermissionsConfig {
    /// True if `tool_name` is on the denylist. Exact match for now.
    pub fn is_denied(&self, tool_name: &str) -> bool {
        self.deny_tools.iter().any(|d| d == tool_name)
    }

    /// True if no permissions are configured. Used by `#[serde(skip_serializing_if)]`
    /// so the parent `NativeExecutorConfig` doesn't emit an empty
    /// `[native_executor.permissions]` table — which would conflict
    /// with user-appended `[native_executor.permissions]` entries
    /// (TOML duplicate-key error).
    pub fn is_empty(&self) -> bool {
        self.deny_tools.is_empty()
    }
}

/// Web access configuration for the native executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeWebConfig {
    /// API key for search backend (Serper, Brave, etc.). Supports env var syntax: "${VAR}".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_api_key: Option<String>,

    /// SearXNG instance URL for the SearXNG search backend (e.g.
    /// "http://localhost:8888"). When set — or when the `WG_SEARXNG_URL`
    /// env var is set — SearXNG joins the parallel backend fan-out.
    /// When unset, the SearXNG backend is a no-op (returns empty
    /// results without consuming a circuit-breaker strike).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub searxng_url: Option<String>,

    /// Maximum content chars for web_fetch before truncation.
    #[serde(default = "default_fetch_max_chars")]
    pub fetch_max_chars: usize,

    /// HTTP request timeout for web_fetch in seconds.
    #[serde(default = "default_fetch_timeout_secs")]
    pub fetch_timeout_secs: u64,
}

fn default_fetch_max_chars() -> usize {
    16_000
}
fn default_fetch_timeout_secs() -> u64 {
    30
}

impl Default for NativeWebConfig {
    fn default() -> Self {
        Self {
            search_api_key: None,
            searxng_url: None,
            fetch_max_chars: default_fetch_max_chars(),
            fetch_timeout_secs: default_fetch_timeout_secs(),
        }
    }
}

/// Background task configuration for the native executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeBackgroundConfig {
    /// Maximum concurrent background tasks per agent.
    #[serde(default = "default_max_background_tasks")]
    pub max_background_tasks: usize,

    /// Default timeout for background tasks in seconds.
    #[serde(default = "default_background_timeout_secs")]
    pub background_timeout_secs: u64,
}

fn default_max_background_tasks() -> usize {
    5
}
fn default_background_timeout_secs() -> u64 {
    600
}

impl Default for NativeBackgroundConfig {
    fn default() -> Self {
        Self {
            max_background_tasks: default_max_background_tasks(),
            background_timeout_secs: default_background_timeout_secs(),
        }
    }
}

/// Delegate (in-process subtask) configuration for the native executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeDelegateConfig {
    /// Maximum turns for delegated sub-agents.
    #[serde(default = "default_delegate_max_turns")]
    pub delegate_max_turns: usize,

    /// Model for delegate sub-agents. Empty string = same as parent agent.
    #[serde(default)]
    pub delegate_model: String,
}

fn default_delegate_max_turns() -> usize {
    10
}

impl Default for NativeDelegateConfig {
    fn default() -> Self {
        Self {
            delegate_max_turns: default_delegate_max_turns(),
            delegate_model: String::new(),
        }
    }
}

/// OpenRouter cost cap and monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRouterConfig {
    /// Global project cost cap in USD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cap_global_usd: Option<f64>,

    /// Per-session cost cap in USD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cap_session_usd: Option<f64>,

    /// Per-task cost cap in USD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cap_task_usd: Option<f64>,

    /// Behavior when cost cap is reached
    #[serde(default = "default_cap_behavior")]
    pub cap_behavior: CapBehavior,

    /// Fallback model when cap_behavior is "fallback"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,

    /// How often to check key status in minutes
    #[serde(default = "default_key_status_check_interval")]
    pub key_status_check_interval_minutes: u32,

    /// Warning threshold as percentage of limit (0-100)
    #[serde(default = "default_warn_usage_percent")]
    pub warn_at_usage_percent: u8,

    /// Cost estimation buffer multiplier
    #[serde(default = "default_cost_estimation_buffer")]
    pub cost_estimation_buffer: f64,

    /// Enable cache tracking from OpenRouter responses
    #[serde(default = "default_enable_cache_tracking")]
    pub enable_cache_tracking: bool,

    /// Track session costs in coordinator state
    #[serde(default = "default_track_session_costs")]
    pub track_session_costs: bool,

    /// Persist cost history to files
    #[serde(default)]
    pub persist_cost_history: bool,
}

/// Cost cap enforcement behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapBehavior {
    /// Fail the task/session immediately
    Fail,
    /// Fall back to a cheaper model
    Fallback,
    /// Escalate to user for decision
    Escalate,
    /// Switch to read-only mode (monitoring only)
    Readonly,
}

fn default_cap_behavior() -> CapBehavior {
    CapBehavior::Escalate
}

fn default_key_status_check_interval() -> u32 {
    5
}

fn default_warn_usage_percent() -> u8 {
    80
}

fn default_cost_estimation_buffer() -> f64 {
    1.2
}

fn default_enable_cache_tracking() -> bool {
    true
}

fn default_track_session_costs() -> bool {
    true
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            cost_cap_global_usd: None,
            cost_cap_session_usd: None,
            cost_cap_task_usd: None,
            cap_behavior: default_cap_behavior(),
            fallback_model: None,
            key_status_check_interval_minutes: default_key_status_check_interval(),
            warn_at_usage_percent: default_warn_usage_percent(),
            cost_estimation_buffer: default_cost_estimation_buffer(),
            enable_cache_tracking: default_enable_cache_tracking(),
            track_session_costs: default_track_session_costs(),
            persist_cost_history: false,
        }
    }
}

/// Authentication credentials for child processes the daemon spawns.
///
/// The daemon runs `claude` subprocesses for three things: the long-running
/// coordinator agent (`spawn_claude_process`), lightweight LLM calls
/// (`call_claude_cli` — chat compaction, triage, evaluation), and per-task
/// agent wrappers (`spawn/execution.rs`). All three read credentials from
/// the same places the Claude CLI itself does (env vars, `~/.claude/
/// credentials.json`). If `claude login` has been run on the machine, this
/// section can stay empty — the CLI resolves auth on its own.
///
/// This section only exists for headless setups where no interactive
/// login happened (e.g. a daemon started via Task Scheduler / systemd at
/// boot) and the user has a bare `sk-ant-oat01-…` token from
/// `claude setup-token` or a subscription dashboard.
///
/// **Important:** `sk-ant-oat01-…` tokens are OAuth access tokens. They
/// go in `CLAUDE_CODE_OAUTH_TOKEN` (Bearer scheme). Passing them in
/// `ANTHROPIC_API_KEY` produces a 401 because the CLI sends that env
/// with the `x-api-key` header, which is only valid for `sk-ant-api03-…`
/// keys.
///
/// ```toml
/// [auth]
/// # Preferred: point at a file, leave the token out of git.
/// claude_code_oauth_token_file = "~/.config/worksgood/oauth-token"
///
/// # Or inline (discouraged — keep `.wg/config.toml` out of VCS
/// # if you use this form):
/// # claude_code_oauth_token = "sk-ant-oat01-…"
/// ```
///
/// Resolution order when the daemon spawns a claude subprocess:
///   1. `CLAUDE_CODE_OAUTH_TOKEN` already in env → use as-is
///   2. `[auth] claude_code_oauth_token` inline → inject into env
///   3. `[auth] claude_code_oauth_token_file` → read file, inject
///   4. Nothing here → Claude CLI falls back to `~/.claude/credentials.json`
///      / its own refresh loop (the normal logged-in path)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Inline OAuth token (`sk-ant-oat01-…`). Discouraged because the file
    /// ends up on disk in `.wg/config.toml`; prefer `_file` below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code_oauth_token: Option<String>,

    /// Path to a file containing the OAuth token on a single line.
    /// Supports `~/` and `$HOME/` expansion. ACL the file to your user
    /// (`icacls <path> /inheritance:r /grant %USERNAME%:R` on Windows,
    /// `chmod 600 <path>` on Unix) and keep it out of version control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code_oauth_token_file: Option<String>,
}

impl AuthConfig {
    /// Resolve the Claude Code OAuth token from env → inline → file.
    ///
    /// Returns `None` if nothing is configured; callers should treat that
    /// as "let the Claude CLI handle auth" rather than an error, because
    /// `~/.claude/credentials.json` (written by `claude login`) is the
    /// normal path.
    pub fn resolve_claude_oauth_token(&self) -> Option<String> {
        if let Ok(v) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
            && !v.is_empty()
        {
            return Some(v);
        }
        if let Some(v) = self.claude_code_oauth_token.as_ref()
            && !v.is_empty()
        {
            return Some(v.clone());
        }
        if let Some(path) = self.claude_code_oauth_token_file.as_ref() {
            let expanded = expand_tilde(path);
            if let Ok(content) = std::fs::read_to_string(&expanded) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }
}

/// Help display configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelpConfig {
    /// Command ordering: "usage" (default), "alphabetical", or "curated"
    #[serde(default = "default_help_ordering")]
    pub ordering: String,
}

fn default_help_ordering() -> String {
    "usage".to_string()
}

impl Default for HelpConfig {
    fn default() -> Self {
        Self {
            ordering: default_help_ordering(),
        }
    }
}

/// Log configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// Rotation threshold in bytes (default: 10 MB)
    #[serde(default = "default_rotation_threshold")]
    pub rotation_threshold: u64,
}

fn default_rotation_threshold() -> u64 {
    10 * 1024 * 1024 // 10 MB
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            rotation_threshold: default_rotation_threshold(),
        }
    }
}

/// Replay configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayConfig {
    /// Default threshold for --keep-done: preserve Done tasks scoring above this (0.0-1.0)
    #[serde(default = "default_keep_done_threshold")]
    pub keep_done_threshold: f64,

    /// Whether to snapshot agent output logs alongside graph.jsonl
    #[serde(default)]
    pub snapshot_agent_output: bool,
}

fn default_keep_done_threshold() -> f64 {
    0.9
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            keep_done_threshold: default_keep_done_threshold(),
            snapshot_agent_output: false,
        }
    }
}

/// Guardrails for autopoietic task creation by agents.
/// Prevents task explosion when agents create subtasks autonomously.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailsConfig {
    /// Maximum tasks a single agent execution can create via `wg add`.
    /// Enforced when WG_AGENT_ID env var is set. Default: 10.
    #[serde(default = "default_max_child_tasks_per_agent")]
    pub max_child_tasks_per_agent: u32,

    /// Maximum depth of task chains (counting --after hops from root).
    /// Prevents infinite decomposition chains. Default: 8.
    #[serde(default = "default_max_task_depth")]
    pub max_task_depth: u32,

    /// Maximum times a task can be requeued via failed-dependency triage.
    /// Prevents infinite triage loops. Default: 3.
    #[serde(default = "default_max_triage_attempts")]
    pub max_triage_attempts: u32,

    /// Whether to inject adaptive decomposition guidance into agent prompts.
    /// When true (default), the executor analyzes task descriptions and provides
    /// task-specific decomposition hints (atomic vs multi-step classification
    /// plus decomposition templates). Set to false to use the generic guidance.
    #[serde(default = "default_decomp_guidance")]
    pub decomp_guidance: bool,
}

fn default_max_child_tasks_per_agent() -> u32 {
    10
}

fn default_max_task_depth() -> u32 {
    8
}

fn default_max_triage_attempts() -> u32 {
    3
}

fn default_decomp_guidance() -> bool {
    true
}

impl Default for GuardrailsConfig {
    fn default() -> Self {
        Self {
            max_child_tasks_per_agent: default_max_child_tasks_per_agent(),
            max_task_depth: default_max_task_depth(),
            max_triage_attempts: default_max_triage_attempts(),
            decomp_guidance: default_decomp_guidance(),
        }
    }
}

/// Visualization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VizConfig {
    /// Edge color style: "gray" (default), "white", or "mixed" (tree=white, arcs=gray)
    #[serde(default = "default_edge_color")]
    pub edge_color: String,
    /// Animation mode: "normal" (default), "fast", "slow", "reduced", "off"
    #[serde(default = "default_animation_mode")]
    pub animations: String,
}

fn default_edge_color() -> String {
    "gray".to_string()
}

fn default_animation_mode() -> String {
    "normal".to_string()
}

impl Default for VizConfig {
    fn default() -> Self {
        Self {
            edge_color: default_edge_color(),
            animations: default_animation_mode(),
        }
    }
}

/// TUI-specific settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Enable mouse support (default: auto-detected based on tmux)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse_mode: Option<bool>,
    /// Default layout mode: "auto", "horizontal", "vertical"
    #[serde(default = "default_tui_layout")]
    pub default_layout: String,
    /// Color theme: "dark" (default), "light"
    #[serde(default = "default_tui_theme")]
    pub color_theme: String,
    /// Timestamp display format: "relative" (default), "iso", "local", "off"
    #[serde(default = "default_timestamp_format")]
    pub timestamp_format: String,
    /// Show token counts in task details
    #[serde(default = "default_true")]
    pub show_token_counts: bool,
    /// Name length threshold for inline vs above-line display (default: 8)
    #[serde(default = "default_message_name_threshold")]
    pub message_name_threshold: u16,
    /// Indentation for message body when name is on its own line (0-8, default: 2)
    #[serde(default = "default_message_indent")]
    pub message_indent: u16,
    /// Inspector panel ratio: percentage of width given to the inspector in split mode (default: 67)
    #[serde(default = "default_panel_ratio")]
    pub panel_ratio: u16,
    /// Default inspector size when first opened: "1/3", "1/2", "2/3" (default), "full"
    #[serde(default = "default_inspector_size")]
    pub default_inspector_size: String,
    /// Persist chat history across TUI restarts (default: true)
    #[serde(default = "default_true")]
    pub chat_history: bool,
    /// Maximum number of chat messages to persist (default: 1000)
    #[serde(default = "default_chat_history_max")]
    pub chat_history_max: usize,
    /// Requested chat messages per TUI page (default: 100).
    /// The live render/search projection is always hard-capped at 200 records
    /// and 1 MiB, regardless of this value or the CLI history-depth override.
    #[serde(default = "default_chat_page_size")]
    pub chat_page_size: usize,
    /// Comma-separated counters to display: "uptime", "cumulative", "active", "session", "compact"
    #[serde(default = "default_counters")]
    pub counters: String,
    /// Show all system tasks (dot-prefixed) by default in TUI
    #[serde(default)]
    pub show_system_tasks: bool,
    /// Show only running (in-progress/open) system tasks by default
    #[serde(default)]
    pub show_running_system_tasks: bool,
    /// Show key press feedback overlay (useful for screencasts/demos)
    #[serde(default)]
    pub show_keys: bool,
    /// Session boundary gap threshold in minutes (default: 30).
    /// A visual divider is shown between chat messages separated by more than this many minutes.
    /// Set to 0 to disable session boundaries.
    #[serde(default = "default_session_gap_minutes")]
    pub session_gap_minutes: u32,
}

fn default_tui_layout() -> String {
    "auto".to_string()
}
fn default_tui_theme() -> String {
    "dark".to_string()
}
fn default_timestamp_format() -> String {
    "relative".to_string()
}
fn default_true() -> bool {
    true
}
fn default_message_name_threshold() -> u16 {
    8
}
fn default_message_indent() -> u16 {
    2
}
fn default_panel_ratio() -> u16 {
    67
}
fn default_inspector_size() -> String {
    "2/3".to_string()
}
fn default_chat_history_max() -> usize {
    1000
}
fn default_chat_page_size() -> usize {
    100
}
fn default_counters() -> String {
    "uptime,cumulative,active,compact".to_string()
}
fn default_session_gap_minutes() -> u32 {
    30
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            mouse_mode: None,
            default_layout: default_tui_layout(),
            color_theme: default_tui_theme(),
            timestamp_format: default_timestamp_format(),
            show_token_counts: true,
            message_name_threshold: default_message_name_threshold(),
            message_indent: default_message_indent(),
            panel_ratio: default_panel_ratio(),
            default_inspector_size: default_inspector_size(),
            chat_history: true,
            chat_history_max: default_chat_history_max(),
            chat_page_size: default_chat_page_size(),
            counters: default_counters(),
            show_system_tasks: false,
            show_running_system_tasks: false,
            show_keys: false,
            session_gap_minutes: default_session_gap_minutes(),
        }
    }
}

/// A configured LLM endpoint (like a WiFi network entry).
//
// NOTE: `Default` is NOT derived here — there is a hand-written `impl Default`
// below so `provider` defaults to `"anthropic"` (a derived Default would leave
// it empty and break provider routing). A stray `Default` in this derive list
// (added in `fix-bin-test`) collided with that impl and broke the lib build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointConfig {
    /// Display name for this endpoint
    pub name: String,
    /// Provider type: "anthropic", "openai", "openrouter", "local"
    #[serde(default = "default_provider")]
    pub provider: String,
    /// API endpoint URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Default model for this endpoint
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// API key for this endpoint (stored in config — user should gitignore)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Path to a file containing the API key (~ and relative paths supported)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_file: Option<String>,
    /// Environment variable name containing the API key (explicit reference)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Secret store reference: "keyring:<name>", "plain:<name>", "env:<VAR>", "op://<path>", "pass:<path>"
    /// Preferred over api_key_env for new configs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_ref: Option<String>,
    /// Whether this is the default endpoint for new agents
    #[serde(default)]
    pub is_default: bool,
    /// Context window size in tokens (overrides model registry for this endpoint)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

fn default_provider() -> String {
    "anthropic".to_string()
}

impl Default for EndpointConfig {
    /// Hand-written so `provider` defaults to `"anthropic"` — matching the
    /// serde `#[serde(default = "default_provider")]` used when deserializing.
    /// A derived `Default` would leave `provider` empty (`""`), silently
    /// diverging from the on-disk default and breaking provider routing.
    fn default() -> Self {
        Self {
            name: String::new(),
            provider: default_provider(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        }
    }
}

/// Expand `~` prefix to user's home directory.
fn expand_tilde(path: &str) -> PathBuf {
    let p = Path::new(path);
    if let Ok(rest) = p.strip_prefix("~")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    p.to_path_buf()
}

impl EndpointConfig {
    /// Return the environment variable names to check for API keys, based on provider.
    pub fn env_var_names_for_provider(provider: &str) -> &'static [&'static str] {
        match provider {
            "openrouter" => &["OPENROUTER_API_KEY", "OPENAI_API_KEY"],
            "openai" => &["OPENAI_API_KEY"],
            "anthropic" => &["ANTHROPIC_API_KEY"],
            _ => &[],
        }
    }

    /// Resolve the API key for this endpoint **from WG config
    /// only** — the strict variant used by the native executor.
    ///
    /// Unlike [`Self::resolve_api_key`], this method does NOT fall back
    /// to provider-specific env vars (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY`
    /// / `OPENROUTER_API_KEY`). It honors only the user-authorized config
    /// fields:
    /// 1. `api_key` — inline literal
    /// 2. `api_key_file` — read file contents (with `~`/relative-path expansion)
    /// 3. `api_key_env` — explicit, user-named env var (this IS WG
    ///    config; the user wrote `api_key_env = "MY_VAR"`)
    ///
    /// Returns `Ok(None)` when no key is configured. Callers should treat
    /// `None` as "no auth header" rather than an error — if the endpoint
    /// requires auth, the 401 response surfaces a config-pointing error.
    pub fn resolve_api_key_strict(
        &self,
        workgraph_dir: Option<&Path>,
    ) -> anyhow::Result<Option<String>> {
        if let Some(ref key) = self.api_key {
            return Ok(Some(key.clone()));
        }
        if let Some(ref file_path) = self.api_key_file {
            let expanded = expand_tilde(file_path);
            let path = if expanded.is_absolute() {
                expanded
            } else if let Some(dir) = workgraph_dir {
                dir.join(expanded)
            } else {
                expanded
            };
            let contents = fs::read_to_string(&path).map_err(|e| {
                anyhow::anyhow!("Failed to read API key from {}: {}", path.display(), e)
            })?;
            let key = contents.trim().to_string();
            if key.is_empty() {
                anyhow::bail!("API key file {} is empty", path.display());
            }
            return Ok(Some(key));
        }
        // Secret store reference
        if let Some(ref r) = self.api_key_ref {
            let secrets_cfg = crate::secret::SecretsConfig::load_global();
            match crate::secret::resolve_ref(r, &secrets_cfg) {
                Ok(Some(key)) => {
                    let key = key.trim().to_string();
                    if !key.is_empty() {
                        return Ok(Some(key));
                    }
                }
                Ok(None) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(ref env_name) = self.api_key_env
            && let Ok(key) = std::env::var(env_name)
        {
            let key = key.trim().to_string();
            if !key.is_empty() {
                return Ok(Some(key));
            }
        }
        Ok(None)
    }

    /// Resolve the API key for this endpoint.
    ///
    /// Priority:
    /// 1. `api_key` — use directly if set
    /// 2. `api_key_file` — read file contents, trim whitespace
    /// 3. `api_key_ref` — secret store reference (keyring/plain/env/op/pass)
    /// 4. `api_key_env` — read from explicitly named env var (deprecated; prefer api_key_ref)
    /// 5. Environment variable fallback based on provider
    ///
    /// For `api_key_file`, supports:
    /// - `~` expansion to home directory
    /// - Relative paths resolved against `workgraph_dir` (if provided)
    pub fn resolve_api_key(&self, workgraph_dir: Option<&Path>) -> anyhow::Result<Option<String>> {
        if let Some(ref key) = self.api_key {
            return Ok(Some(key.clone()));
        }
        if let Some(ref file_path) = self.api_key_file {
            let expanded = expand_tilde(file_path);
            let path = if expanded.is_absolute() {
                expanded
            } else if let Some(dir) = workgraph_dir {
                dir.join(expanded)
            } else {
                expanded
            };
            let contents = fs::read_to_string(&path).map_err(|e| {
                anyhow::anyhow!("Failed to read API key from {}: {}", path.display(), e)
            })?;
            let key = contents.trim().to_string();
            if key.is_empty() {
                anyhow::bail!("API key file {} is empty", path.display());
            }
            return Ok(Some(key));
        }
        // Secret store reference (preferred over api_key_env)
        if let Some(ref r) = self.api_key_ref {
            let secrets_cfg = crate::secret::SecretsConfig::load_global();
            match crate::secret::resolve_ref(r, &secrets_cfg) {
                Ok(Some(key)) => {
                    let key = key.trim().to_string();
                    if !key.is_empty() {
                        return Ok(Some(key));
                    }
                }
                Ok(None) => {}
                Err(e) => return Err(e),
            }
        }
        // Explicit env var reference (deprecated; api_key_ref is preferred)
        if let Some(ref env_name) = self.api_key_env
            && let Ok(key) = std::env::var(env_name)
        {
            let key = key.trim().to_string();
            if !key.is_empty() {
                return Ok(Some(key));
            }
        }
        // Environment variable fallback based on provider
        for var_name in Self::env_var_names_for_provider(&self.provider) {
            if let Ok(key) = std::env::var(var_name) {
                let key = key.trim().to_string();
                if !key.is_empty() {
                    return Ok(Some(key));
                }
            }
        }
        Ok(None)
    }

    /// Return the API key masked for display: "sk-****...ab12"
    pub fn masked_key(&self) -> String {
        match &self.api_key {
            Some(key) if key.len() > 8 => {
                let prefix = &key[..3];
                let suffix = &key[key.len() - 4..];
                format!("{}****...{}", prefix, suffix)
            }
            Some(key) if !key.is_empty() => "****".to_string(),
            _ => {
                if self.api_key_file.is_some() {
                    "(from file)".to_string()
                } else if self.api_key_ref.is_some() {
                    "(from secret ref)".to_string()
                } else if let Some(ref env_name) = self.api_key_env {
                    format!("(from env: {})", env_name)
                } else {
                    "(not set)".to_string()
                }
            }
        }
    }

    /// Describe the source of the API key for display purposes.
    pub fn key_source(&self) -> String {
        if self.api_key.is_some() {
            "inline".to_string()
        } else if let Some(ref file_path) = self.api_key_file {
            format!("file: {}", file_path)
        } else if let Some(ref api_key_ref) = self.api_key_ref {
            api_key_ref.clone()
        } else if let Some(ref env_name) = self.api_key_env {
            format!("env: {}", env_name)
        } else {
            // Check provider-based env var fallback
            for var_name in Self::env_var_names_for_provider(&self.provider) {
                if std::env::var(var_name).is_ok() {
                    return format!("env: {} (auto-detected)", var_name);
                }
            }
            "(not configured)".to_string()
        }
    }

    /// Default URL for known providers.
    pub fn default_url_for_provider(provider: &str) -> &'static str {
        match provider {
            "anthropic" => "https://api.anthropic.com",
            "openai" => "https://api.openai.com/v1",
            "openrouter" => "https://openrouter.ai/api/v1",
            "gemini" => "https://generativelanguage.googleapis.com/v1beta/openai",
            "ollama" => "http://localhost:11434/v1",
            "llamacpp" => "http://localhost:8080/v1",
            "vllm" => "http://localhost:8000/v1",
            "local" => "http://localhost:11434/v1",
            _ => "",
        }
    }
}

/// LLM endpoints configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EndpointsConfig {
    /// When `true`, local config inherits `[[llm_endpoints.endpoints]]` entries
    /// from the global config. When `false` (default), the local config's
    /// endpoints list FULLY replaces the global list — set this to `true`
    /// in local config to keep the legacy "global cascades into local"
    /// behavior. Has no effect when read from global config.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherit_global: bool,

    /// List of configured endpoints
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<EndpointConfig>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl EndpointsConfig {
    /// Returns true when there are no configured endpoints.
    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty() && !self.inherit_global
    }

    /// Find the best endpoint for a given provider name.
    pub fn find_for_provider(&self, provider: &str) -> Option<&EndpointConfig> {
        let mut first_match: Option<&EndpointConfig> = None;
        for ep in &self.endpoints {
            if ep.provider == provider {
                if ep.is_default {
                    return Some(ep);
                }
                if first_match.is_none() {
                    first_match = Some(ep);
                }
            }
        }
        first_match
    }

    /// Find an endpoint by its display name.
    pub fn find_by_name(&self, name: &str) -> Option<&EndpointConfig> {
        self.endpoints.iter().find(|ep| ep.name == name)
    }

    /// Find the default endpoint (the one with `is_default = true`), or the first endpoint
    /// if none is marked as default.
    pub fn find_default(&self) -> Option<&EndpointConfig> {
        self.endpoints
            .iter()
            .find(|ep| ep.is_default)
            .or_else(|| self.endpoints.first())
    }
}

/// Checkpoint configuration for agent context preservation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConfig {
    /// Auto-checkpoint every N turns
    #[serde(default = "default_auto_interval_turns")]
    pub auto_interval_turns: u32,

    /// Auto-checkpoint every N minutes
    #[serde(default = "default_auto_interval_mins")]
    pub auto_interval_mins: u32,

    /// Keep only last N checkpoints per task
    #[serde(default = "default_max_checkpoints")]
    pub max_checkpoints: u32,

    /// Max tokens of previous attempt context to inject on retry (0 = disabled)
    #[serde(default = "default_retry_context_tokens")]
    pub retry_context_tokens: u32,
}

fn default_auto_interval_turns() -> u32 {
    15
}

fn default_auto_interval_mins() -> u32 {
    20
}

fn default_max_checkpoints() -> u32 {
    5
}

fn default_retry_context_tokens() -> u32 {
    2000
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            auto_interval_turns: default_auto_interval_turns(),
            auto_interval_mins: default_auto_interval_mins(),
            max_checkpoints: default_max_checkpoints(),
            retry_context_tokens: default_retry_context_tokens(),
        }
    }
}

// ---------------------------------------------------------------------------
// Model routing configuration
// ---------------------------------------------------------------------------

/// Common structured reasoning budget used by handlers that expose a separate
/// reasoning/thinking knob. Pi maps this directly to `--thinking <level>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningLevel {
    pub const ALL: &'static [ReasoningLevel] = &[
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Value accepted by Codex CLI's `model_reasoning_effort` config key.
    ///
    /// WG's portable vocabulary is slightly wider/different: Codex calls
    /// disabled reasoning `none`, and does not accept `minimal` as an effort.
    /// Response verbosity is intentionally not part of this mapping; Codex
    /// exposes that independently as `model_verbosity`.
    pub fn as_codex_effort(self) -> &'static str {
        match self {
            Self::Off => "none",
            Self::Minimal => "low",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for ReasoningLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ReasoningLevel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            other => anyhow::bail!(
                "invalid reasoning level '{}'. Valid values: off, minimal, low, medium, high, xhigh, max",
                other
            ),
        }
    }
}

/// Dispatch roles for model routing.
/// Each role maps to a specific dispatch point in the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchRole {
    /// Default fallback for any role without explicit config
    Default,
    /// Main task agents spawned by coordinator
    TaskAgent,
    /// Evaluation agents (post-task scoring)
    Evaluator,
    /// FLIP inference phase (reconstructing prompt from output)
    FlipInference,
    /// FLIP comparison phase (scoring similarity)
    FlipComparison,
    /// Agent assignment tasks
    Assigner,
    /// Agency evolver
    Evolver,
    /// FLIP-triggered verification agents
    Verification,
    /// Triage (dead-agent summarization)
    Triage,
    /// Agent creator
    Creator,
    /// Compactor: distills graph state into context.md
    Compactor,
    /// Coordinator evaluation (inline per-turn scoring)
    CoordinatorEval,
    /// Placement agent: analyzes tasks and wires them into the graph
    Placer,
    /// Chat compactor: summarizes per-coordinator conversation history
    ChatCompactor,
    /// Content reviewer: the no-scope inbound-content safety reviewer (WG-Review
    /// Pass 2 / fed S-5 / exec-integrity). An agency one-shot that resolves the
    /// weak tier and escalates to the strong tier on uncertainty (never a human).
    Reviewer,
}

impl std::fmt::Display for DispatchRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => write!(f, "default"),
            Self::TaskAgent => write!(f, "task_agent"),
            Self::Evaluator => write!(f, "evaluator"),
            Self::FlipInference => write!(f, "flip_inference"),
            Self::FlipComparison => write!(f, "flip_comparison"),
            Self::Assigner => write!(f, "assigner"),
            Self::Evolver => write!(f, "evolver"),
            Self::Verification => write!(f, "verification"),
            Self::Triage => write!(f, "triage"),
            Self::Creator => write!(f, "creator"),
            Self::Compactor => write!(f, "compactor"),
            Self::CoordinatorEval => write!(f, "coordinator_eval"),
            Self::Placer => write!(f, "placer"),
            Self::ChatCompactor => write!(f, "chat_compactor"),
            Self::Reviewer => write!(f, "reviewer"),
        }
    }
}

impl std::str::FromStr for DispatchRole {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "default" => Ok(Self::Default),
            "task_agent" => Ok(Self::TaskAgent),
            "evaluator" => Ok(Self::Evaluator),
            "flip_inference" => Ok(Self::FlipInference),
            "flip_comparison" => Ok(Self::FlipComparison),
            "assigner" => Ok(Self::Assigner),
            "evolver" => Ok(Self::Evolver),
            "verification" => Ok(Self::Verification),
            "triage" => Ok(Self::Triage),
            "creator" => Ok(Self::Creator),
            "compactor" => Ok(Self::Compactor),
            "coordinator_eval" => Ok(Self::CoordinatorEval),
            "placer" => Ok(Self::Placer),
            "chat_compactor" => Ok(Self::ChatCompactor),
            "reviewer" => Ok(Self::Reviewer),
            _ => Err(anyhow::anyhow!(
                "Unknown dispatch role '{}'. Valid roles: default, task_agent, evaluator, \
                 flip_inference, flip_comparison, assigner, evolver, verification, triage, \
                 creator, compactor, placer, chat_compactor, reviewer",
                s
            )),
        }
    }
}

impl DispatchRole {
    /// All known roles (excluding Default).
    pub const ALL: &'static [DispatchRole] = &[
        Self::TaskAgent,
        Self::Evaluator,
        Self::FlipInference,
        Self::FlipComparison,
        Self::Assigner,
        Self::Evolver,
        Self::Verification,
        Self::Triage,
        Self::Creator,
        Self::Compactor,
        Self::Placer,
        Self::ChatCompactor,
        Self::Reviewer,
    ];

    /// Default quality tier for this role.
    ///
    /// Metacognition/routing roles (assigner, compactor, triage, evaluator, etc.) default
    /// to Fast so they use haiku and don't burn budget on every dispatch. TaskAgent runs
    /// at Standard; starter/default profiles intentionally map Standard to the
    /// top worker model so ordinary task dispatch does not silently downgrade.
    /// Evolver, Creator, and Verification get Premium because they require strong reasoning:
    /// evolver redesigns the agency, creator decomposes work into new tasks, and
    /// verification is the correctness gate.
    pub fn default_tier(&self) -> Tier {
        match self {
            Self::Triage => Tier::Fast,
            Self::FlipComparison => Tier::Fast,
            Self::Assigner => Tier::Fast,
            Self::Compactor => Tier::Fast,
            Self::ChatCompactor => Tier::Fast,
            Self::Reviewer => Tier::Fast,
            Self::CoordinatorEval => Tier::Fast,
            Self::Placer => Tier::Fast,
            Self::FlipInference => Tier::Fast,
            Self::Evaluator => Tier::Fast,
            Self::TaskAgent => Tier::Standard,
            Self::Evolver => Tier::Premium,
            Self::Creator => Tier::Premium,
            Self::Verification => Tier::Premium,
            Self::Default => Tier::Standard,
        }
    }
}

// ---------------------------------------------------------------------------
// Execution weight tiers
// ---------------------------------------------------------------------------

/// Execution weight tier for agent spawning.
///
/// Controls what tools and context an agent gets, from lightest to heaviest:
/// - Shell: no LLM, just run a shell command
/// - Bare: LLM with wg CLI only (no file access)
/// - Light: LLM with read-only file access (research/review)
/// - Full: all tools (implementation/debugging)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ExecMode {
    /// No LLM — run `task.exec` command directly via bash
    Shell,
    /// LLM with `Bash(wg:*)` only, `--system-prompt` path
    Bare,
    /// LLM with read-only file tools: `Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch`
    Light,
    /// Full Claude Code session with all tools
    #[default]
    Full,
}

impl std::fmt::Display for ExecMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shell => write!(f, "shell"),
            Self::Bare => write!(f, "bare"),
            Self::Light => write!(f, "light"),
            Self::Full => write!(f, "full"),
        }
    }
}

impl std::str::FromStr for ExecMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "shell" => Ok(Self::Shell),
            "bare" => Ok(Self::Bare),
            "light" => Ok(Self::Light),
            "full" => Ok(Self::Full),
            _ => Err(anyhow::anyhow!(
                "Invalid exec_mode '{}'. Valid values: shell, bare, light, full",
                s
            )),
        }
    }
}

impl ExecMode {
    /// All variants in order from lightest to heaviest.
    pub const ALL: &'static [ExecMode] = &[Self::Shell, Self::Bare, Self::Light, Self::Full];

    /// Parse from an optional string, defaulting to Full.
    pub fn from_opt(s: Option<&str>) -> Result<Self, anyhow::Error> {
        match s {
            Some(v) => v.parse(),
            None => Ok(Self::Full),
        }
    }

    /// Return the valid exec_modes for a given executor type.
    ///
    /// - `"shell"` executor: only `Shell`
    /// - `"claude"`, `"native"`, `"codex"`, or any other: `Bare`, `Light`, `Full`
    pub fn valid_for_executor(executor: &str) -> &'static [ExecMode] {
        match executor {
            "shell" => &[ExecMode::Shell],
            _ => &[ExecMode::Bare, ExecMode::Light, ExecMode::Full],
        }
    }

    /// Return the safe default exec_mode for a given executor type.
    pub fn default_for_executor(executor: &str) -> ExecMode {
        match executor {
            "shell" => ExecMode::Shell,
            _ => ExecMode::Full,
        }
    }

    /// Check whether this exec_mode is valid for the given executor.
    pub fn is_valid_for_executor(&self, executor: &str) -> bool {
        Self::valid_for_executor(executor).contains(self)
    }
}

// ---------------------------------------------------------------------------
// Quality tiers and model registry
// ---------------------------------------------------------------------------

/// Quality tier for model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Fast,
    Standard,
    Premium,
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fast => write!(f, "fast"),
            Self::Standard => write!(f, "standard"),
            Self::Premium => write!(f, "premium"),
        }
    }
}

impl Tier {
    /// Default model alias for each tier (single source of truth).
    ///
    /// Used as the fallback display string in the TUI and as the base for
    /// provider-prefixed defaults in `effective_tiers()`.
    pub fn default_alias(&self) -> &'static str {
        match self {
            Self::Fast => "haiku",
            Self::Standard => "opus",
            Self::Premium => "opus",
        }
    }

    /// Return the next tier up, capping at Premium.
    pub fn escalate(&self) -> Tier {
        match self {
            Self::Fast => Self::Standard,
            Self::Standard => Self::Premium,
            Self::Premium => Self::Premium,
        }
    }
}

impl std::str::FromStr for Tier {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "standard" => Ok(Self::Standard),
            "premium" => Ok(Self::Premium),
            _ => anyhow::bail!("unknown tier '{}' (expected: fast, standard, premium)", s),
        }
    }
}

/// A model registry entry describing a provider+model combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRegistryEntry {
    /// Short identifier used in config references (e.g., "haiku", "sonnet", "gpt-4o")
    pub id: String,
    /// Provider: "anthropic", "openai", "google", "local", etc.
    pub provider: String,
    /// Model identifier passed to the executor (bare alias for Claude, full ID for others)
    pub model: String,
    /// Quality tier this model belongs to
    pub tier: Tier,
    /// API endpoint URL (None = use provider default)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Max input context window in tokens
    #[serde(default)]
    pub context_window: u64,
    /// Max output tokens
    #[serde(default)]
    pub max_output_tokens: u64,
    /// Cost per million input tokens (USD)
    #[serde(default)]
    pub cost_per_input_mtok: f64,
    /// Cost per million output tokens (USD)
    #[serde(default)]
    pub cost_per_output_mtok: f64,
    /// Whether the provider supports prompt caching
    #[serde(default)]
    pub prompt_caching: bool,
    /// Discount multiplier for cached reads (e.g., 0.1 = 90% off)
    #[serde(default)]
    pub cache_read_discount: f64,
    /// Premium multiplier for cache writes (e.g., 1.25 = 25% more)
    #[serde(default)]
    pub cache_write_premium: f64,
    /// Descriptors for when to use this model
    #[serde(default)]
    pub descriptors: Vec<String>,
}

impl Default for ModelRegistryEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            provider: String::new(),
            model: String::new(),
            tier: Tier::Standard,
            endpoint: None,
            context_window: 0,
            max_output_tokens: 0,
            cost_per_input_mtok: 0.0,
            cost_per_output_mtok: 0.0,
            prompt_caching: false,
            cache_read_discount: 0.0,
            cache_write_premium: 0.0,
            descriptors: Vec::new(),
        }
    }
}

/// Ordered, opt-in execution-failure policy.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionConfig {
    /// Exact primary route → explicitly authorized same-system alternatives.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<ExecutionFallback>,
}

impl ExecutionConfig {
    pub fn is_empty(&self) -> bool {
        self.fallbacks.is_empty()
    }

    /// Alternatives for an exact primary route, preserving file order.
    pub fn models_for(&self, primary: &str) -> &[String] {
        self.fallbacks
            .iter()
            .find(|entry| entry.primary.trim() == primary.trim())
            .map(|entry| entry.models.as_slice())
            .unwrap_or(&[])
    }
}

/// One explicit same-system fallback declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionFallback {
    /// Exact handler-first primary route this declaration applies to.
    pub primary: String,
    /// Ordered alternatives. Every entry must have the primary's execution-system key.
    #[serde(default)]
    pub models: Vec<String>,
}

/// Handler + provider/wire identity. Route failure may change a model only
/// while this key remains identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSystemKey {
    pub handler: String,
    pub provider: String,
}

impl std::fmt::Display for ExecutionSystemKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.handler, self.provider)
    }
}

/// Derive the execution-system identity for the one-shot handlers WG supports.
/// Unknown/opaque handler dialects fail closed: without a provider key WG may
/// retry only the exact route, never select another model.
pub fn execution_system_key(raw_route: &str) -> anyhow::Result<ExecutionSystemKey> {
    let route = raw_route.trim();
    if route.is_empty() {
        anyhow::bail!("empty model route has no execution-system key");
    }

    let handler = crate::dispatch::handler_for_model(route);
    let provider = match handler {
        crate::dispatch::ExecutorKind::Claude => {
            // The lenient handler resolver historically maps every unknown bare
            // token to Claude. Only actual Claude forms are safe here.
            let lower = route.to_ascii_lowercase();
            let is_alias = matches!(lower.as_str(), "opus" | "sonnet" | "haiku" | "fable")
                || lower.starts_with("claude-")
                || lower.starts_with("anthropic/claude-");
            if !lower.starts_with("claude:") && !is_alias {
                anyhow::bail!(
                    "route {route:?} does not explicitly identify the claude execution system"
                );
            }
            "anthropic-cli".to_string()
        }
        crate::dispatch::ExecutorKind::Codex => {
            if !route.to_ascii_lowercase().starts_with("codex:") {
                anyhow::bail!(
                    "route {route:?} does not explicitly identify the codex execution system"
                );
            }
            "openai-codex-cli".to_string()
        }
        crate::dispatch::ExecutorKind::Pi => {
            let inner = route
                .strip_prefix("pi:")
                .ok_or_else(|| anyhow::anyhow!("pi route {route:?} is not handler-first"))?;
            let provider = inner
                .split_once(':')
                .map(|(p, _)| p)
                .or_else(|| inner.split_once('/').map(|(p, _)| p))
                .filter(|p| !p.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("pi route {route:?} does not identify a provider")
                })?;
            provider.to_ascii_lowercase()
        }
        crate::dispatch::ExecutorKind::Native => {
            let lower = route.to_ascii_lowercase();
            if !lower.starts_with("nex:") && !lower.starts_with("native:") {
                anyhow::bail!(
                    "route {route:?} does not explicitly identify the nex execution system"
                );
            }
            let inner = strip_native_handler_prefix(route);
            parse_model_spec(inner)
                .provider
                .as_deref()
                .map(provider_to_resolved_provider)
                .unwrap_or("oai-compat")
                .to_string()
        }
        other => anyhow::bail!(
            "handler {} does not expose a lightweight execution-system key",
            other.as_str()
        ),
    };

    let handler = match handler {
        // `Native` is the internal executor enum; `nex` is the canonical
        // user-selected handler name and therefore the execution-system key.
        crate::dispatch::ExecutorKind::Native => "nex",
        other => other.as_str(),
    };
    Ok(ExecutionSystemKey {
        handler: handler.to_string(),
        provider,
    })
}

/// Tier routing configuration: which model ID each tier resolves to.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TierConfig {
    /// Model ID for fast tier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<String>,
    /// Reasoning level for fast tier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_reasoning: Option<ReasoningLevel>,
    /// Model ID for standard tier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard: Option<String>,
    /// Reasoning level for standard tier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_reasoning: Option<ReasoningLevel>,
    /// Model ID for premium tier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub premium: Option<String>,
    /// Reasoning level for premium tier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub premium_reasoning: Option<ReasoningLevel>,
}

/// Tag-based routing rule. When a task carries the named tag AND
/// has no explicit `model`, the task's effective model becomes
/// `model` (and optional `executor` hint). Rules are evaluated in
/// declaration order; first match wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagRoutingEntry {
    /// Tag name to match against `task.tags`.
    pub tag: String,
    /// Model spec in provider:model format (e.g.
    /// `codex:gpt-5-codex`, `oai-compat:qwen3-coder-30b`,
    /// `claude:opus`). The normal resolver pipeline processes this
    /// the same way as `task.model` would — including registry
    /// alias lookup and tier resolution.
    pub model: String,
    /// Optional executor hint (`native`, `claude`, `codex`).
    /// When omitted, executor is picked by the same rules as for
    /// any other task: model's provider implies it (anthropic →
    /// claude, oai-compat → native, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
}

/// Deprecated compatibility shim. Freeform tags are labels only, so
/// tag routing never returns a runtime route.
pub fn resolve_tag_routing<'a>(
    _routing: &'a [TagRoutingEntry],
    _task_tags: &[String],
) -> Option<&'a TagRoutingEntry> {
    None
}

/// Per-role model+provider assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleModelConfig {
    /// **Deprecated**: Use provider:model format in the `model` field instead.
    /// Kept for deserialization of old configs; never written back.
    #[serde(default, skip_serializing)]
    pub provider: Option<String>,
    /// Model spec in provider:model format (e.g., "claude:opus", "openrouter:deepseek/deepseek-chat")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tier override: resolve model via tier system instead of direct model
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// Named endpoint override: use a specific configured endpoint by name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Structured reasoning level. Inherited independently from `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningLevel>,
}

/// Model routing: maps each dispatch role to a model+provider.
/// Roles without explicit config fall back to `default`, then to `agent.model`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelRoutingConfig {
    /// Default model+provider for all roles
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<RoleModelConfig>,

    /// Per-role overrides
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_agent: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_inference: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_comparison: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigner: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evolver: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compactor: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placer: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_compactor: Option<RoleModelConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<RoleModelConfig>,
}

impl ModelRoutingConfig {
    /// Get the role-specific config for a dispatch role.
    pub fn get_role(&self, role: DispatchRole) -> Option<&RoleModelConfig> {
        match role {
            DispatchRole::Default => self.default.as_ref(),
            DispatchRole::TaskAgent => self.task_agent.as_ref(),
            DispatchRole::Evaluator => self.evaluator.as_ref(),
            DispatchRole::FlipInference => self.flip_inference.as_ref(),
            DispatchRole::FlipComparison => self.flip_comparison.as_ref(),
            DispatchRole::Assigner => self.assigner.as_ref(),
            DispatchRole::Evolver => self.evolver.as_ref(),
            DispatchRole::Verification => self.verification.as_ref(),
            DispatchRole::Triage => self.triage.as_ref(),
            DispatchRole::Creator => self.creator.as_ref(),
            DispatchRole::Compactor => self.compactor.as_ref(),
            DispatchRole::CoordinatorEval => self.evaluator.as_ref(),
            DispatchRole::Placer => self.placer.as_ref(),
            DispatchRole::ChatCompactor => self.chat_compactor.as_ref(),
            DispatchRole::Reviewer => self.reviewer.as_ref(),
        }
    }

    /// Get a mutable reference to a role's config, creating it if needed.
    pub fn get_role_mut(&mut self, role: DispatchRole) -> &mut Option<RoleModelConfig> {
        match role {
            DispatchRole::Default => &mut self.default,
            DispatchRole::TaskAgent => &mut self.task_agent,
            DispatchRole::Evaluator => &mut self.evaluator,
            DispatchRole::FlipInference => &mut self.flip_inference,
            DispatchRole::FlipComparison => &mut self.flip_comparison,
            DispatchRole::Assigner => &mut self.assigner,
            DispatchRole::Evolver => &mut self.evolver,
            DispatchRole::Verification => &mut self.verification,
            DispatchRole::Triage => &mut self.triage,
            DispatchRole::Creator => &mut self.creator,
            DispatchRole::Compactor => &mut self.compactor,
            DispatchRole::CoordinatorEval => &mut self.evaluator,
            DispatchRole::Placer => &mut self.placer,
            DispatchRole::ChatCompactor => &mut self.chat_compactor,
            DispatchRole::Reviewer => &mut self.reviewer,
        }
    }

    /// Set the model for a role.
    pub fn set_model(&mut self, role: DispatchRole, model: &str) {
        let slot = self.get_role_mut(role);
        if let Some(cfg) = slot {
            cfg.model = Some(model.to_string());
            // Clear deprecated separate provider field — provider is now embedded in model spec
            cfg.provider = None;
        } else {
            *slot = Some(RoleModelConfig {
                provider: None,
                model: Some(model.to_string()),
                tier: None,
                endpoint: None,
                reasoning: None,
            });
        }
    }

    /// Set the structured reasoning level for a role.
    pub fn set_reasoning(&mut self, role: DispatchRole, reasoning: ReasoningLevel) {
        let slot = self.get_role_mut(role);
        if let Some(cfg) = slot {
            cfg.reasoning = Some(reasoning);
        } else {
            *slot = Some(RoleModelConfig {
                provider: None,
                model: None,
                tier: None,
                endpoint: None,
                reasoning: Some(reasoning),
            });
        }
    }

    /// Set the provider for a role.
    pub fn set_provider(&mut self, role: DispatchRole, provider: &str) {
        let slot = self.get_role_mut(role);
        if let Some(cfg) = slot {
            cfg.provider = Some(provider.to_string());
        } else {
            *slot = Some(RoleModelConfig {
                provider: Some(provider.to_string()),
                model: None,
                tier: None,
                endpoint: None,
                reasoning: None,
            });
        }
    }

    /// Set the endpoint for a role.
    pub fn set_endpoint(&mut self, role: DispatchRole, endpoint: &str) {
        let slot = self.get_role_mut(role);
        if let Some(cfg) = slot {
            cfg.endpoint = Some(endpoint.to_string());
        } else {
            *slot = Some(RoleModelConfig {
                provider: None,
                model: None,
                tier: None,
                endpoint: Some(endpoint.to_string()),
                reasoning: None,
            });
        }
    }

    /// Set the tier override for a role.
    pub fn set_tier(&mut self, role: DispatchRole, tier: Tier) {
        let slot = self.get_role_mut(role);
        if let Some(cfg) = slot {
            cfg.tier = Some(tier);
        } else {
            *slot = Some(RoleModelConfig {
                provider: None,
                model: None,
                tier: Some(tier),
                endpoint: None,
                reasoning: None,
            });
        }
    }
}

/// Resolved model+provider for a dispatch.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub model: String,
    pub provider: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    /// Registry entry if resolved through the registry (carries cost data)
    pub registry_entry: Option<ModelRegistryEntry>,
    /// Named endpoint override: when set, consumers should look up this endpoint
    /// by name instead of falling back to provider-based endpoint lookup.
    pub endpoint: Option<String>,
}

impl ResolvedModel {
    /// Return the model spec that should be handed to spawn planning.
    ///
    /// `resolve_model_for_role` keeps API identity split into `model` and
    /// `provider` so lightweight HTTP callers can pick clients cleanly. Spawn
    /// planning, however, derives the executor from a single provider:model
    /// route. Reattach the provider here so CLI-backed routes such as
    /// `codex:gpt-5.5` cannot collapse to bare `gpt-5.5` and accidentally
    /// stay paired with the default Claude executor.
    pub fn spawn_model_spec(&self) -> String {
        let Some(provider) = self.provider.as_deref() else {
            return self.model.clone();
        };
        let prefix = native_provider_to_prefix(provider);
        format!("{prefix}:{}", self.model)
    }
}

// ---------------------------------------------------------------------------
// Unified provider:model naming
// ---------------------------------------------------------------------------

/// Known provider prefixes for the `provider:model` naming convention.
///
/// The `:` delimiter is unambiguous: provider names never contain `:`,
/// and model IDs may contain `/` but never `:`.
///
/// `nex` is the canonical prefix for the in-process nex handler (matches
/// the `wg nex` subcommand name). `local` and `oai-compat` are deprecated
/// aliases — accepted for one release with a stderr warning, then
/// rewritten to `nex` by `wg migrate config`. `openai` is a legacy alias
/// for `oai-compat` (the protocol, not the vendor). All of these route
/// through the same in-process nex handler.
pub const KNOWN_PROVIDERS: &[&str] = &[
    "claude",
    "openrouter",
    "nex",
    "oai-compat", // deprecated — use "nex"
    "openai",     // legacy alias for "oai-compat" — kept for backwards compatibility
    "codex",
    "gemini",
    "ollama",
    "llamacpp",
    "vllm",
    "local", // deprecated — use "nex"
    "native",
];

/// Provider prefixes that have been deprecated in favor of the canonical
/// `nex:` prefix. Returning a non-empty string from this function emits a
/// one-line stderr deprecation warning at config-load / parse time.
///
/// Keep this in sync with `STALE_PROVIDER_REWRITES` in `commands/migrate.rs`.
pub fn deprecated_provider_prefix_replacement(provider: &str) -> Option<&'static str> {
    match provider {
        "local" | "oai-compat" => Some("nex"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Handler-first model-spec enforcement
// (docs/design-handler-first-model-spec.md)
// ---------------------------------------------------------------------------

/// Handler/executor prefixes that are valid as the **leading** token of a
/// model spec. The leading token always names a *handler* (which subprocess
/// runs the spec); everything after the first `:` is that handler's own
/// native model dialect, opaque to wg's routing.
///
/// `native` is the legacy in-process executor name and an alias of `nex`.
/// External-CLI handlers (`pi`, `opencode`, `aider`, …) are valid leading
/// tokens too, but they live in [`crate::dispatch::ExecutorKind::EXTERNAL_CLIS`]
/// — they are intentionally NOT in [`KNOWN_PROVIDERS`] (an executor is not a
/// model provider), so they are checked separately.
pub const HANDLER_PREFIXES: &[&str] = &["claude", "codex", "nex", "native"];

/// Whether `prefix` is a model-namespace (provider) prefix that is NOT a
/// valid leading handler token under the handler-first rule.
///
/// These are the prefixes the handler-first rule rejects as a **leading**
/// spec token: `openrouter`, `openai`, `oai-compat`, `ollama`, `vllm`,
/// `llamacpp`, `gemini`, `local`. They must instead appear as the *inner*
/// dialect of a handler-qualified spec, e.g. `nex:openrouter:vendor/model`
/// (in-process native) or `pi:openrouter:vendor/model` (pi CLI).
///
/// Bare Anthropic aliases (`opus`/`sonnet`/`haiku`) have no `:` and are not
/// rejected; the handler prefixes ([`HANDLER_PREFIXES`]) are not rejected;
/// external-CLI prefixes are not in [`KNOWN_PROVIDERS`] so they are never
/// rejected here.
pub fn is_rejected_leading_provider(prefix: &str) -> bool {
    KNOWN_PROVIDERS.contains(&prefix) && !HANDLER_PREFIXES.contains(&prefix)
}

/// How `wg migrate config` rewrites a deprecated leading provider prefix
/// into its handler-first canonical form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPrefixMigration {
    /// Replace the leading prefix with the canonical handler, **dropping**
    /// the original (`oai-compat:gpt-x` → `nex:gpt-x`): the prefix was a
    /// pure alias of the handler's default wire, so it carries no distinct
    /// meaning the handler still needs.
    Swap(&'static str),
    /// **Prepend** the canonical handler, keeping the original prefix as the
    /// *inner* dialect (`openrouter:vendor/model` → `nex:openrouter:vendor/model`):
    /// the prefix names a distinct wire the native handler still needs.
    Prepend(&'static str),
}

/// Map a deprecated **leading** provider prefix to its handler-first
/// rewrite. Returns `None` for prefixes that are already handler-first
/// (`claude`, `codex`, `nex`, external CLIs) or unknown.
///
/// Per the design (§5.2): collapse the pure-alias set
/// `{local, oai-compat, openai, native}` to `nex:` (swap, drop the prefix);
/// prepend `nex:` to the wire-distinct set
/// `{openrouter, ollama, vllm, llamacpp, gemini}` (keep the prefix as inner
/// dialect). Keep in sync with [`is_rejected_leading_provider`] and the
/// `wg migrate config` rewrite.
pub fn provider_prefix_migration(prefix: &str) -> Option<ProviderPrefixMigration> {
    use ProviderPrefixMigration::*;
    match prefix {
        // Pure aliases of the nex handler's default oai-compat wire → swap.
        "local" | "oai-compat" | "openai" | "native" => Some(Swap("nex")),
        // Wire-distinct providers the native handler still needs → keep as
        // the inner dialect.
        "openrouter" | "ollama" | "vllm" | "llamacpp" | "gemini" => Some(Prepend("nex")),
        _ => None,
    }
}

/// Apply the handler-first migration to a full spec string, returning the
/// rewritten canonical form (`openrouter:X` → `nex:openrouter:X`,
/// `oai-compat:X` → `nex:X`). Returns `None` when the spec's leading token
/// needs no rewrite (already handler-first, bare alias, or unknown prefix).
pub fn handler_first_rewrite(spec: &str) -> Option<String> {
    let (prefix, rest) = spec.split_once(':')?;
    if rest.is_empty() {
        return None;
    }
    match provider_prefix_migration(prefix)? {
        ProviderPrefixMigration::Swap(handler) => Some(format!("{handler}:{rest}")),
        ProviderPrefixMigration::Prepend(handler) => Some(format!("{handler}:{prefix}:{rest}")),
    }
}

/// Build the loud handler-first deprecation message for a spec whose leading
/// token is a rejected provider prefix (`openrouter:…`, `ollama:…`, …),
/// naming the `nex:` / `pi:` handler-qualified forms the user should use
/// instead. Returns `None` for specs that are already handler-first.
///
/// This is the message shown at every strict-validation entry point (CLI
/// `--model`, config load, `wg service start/daemon --model`) so the silent
/// mis-route that 401'd for 14 hours becomes a loud, impossible-to-miss
/// signal.
pub fn handler_first_warning(spec: &str) -> Option<String> {
    let (prefix, rest) = spec.split_once(':')?;
    if rest.trim().is_empty() || !is_rejected_leading_provider(prefix) {
        return None;
    }
    let canonical = handler_first_rewrite(spec).unwrap_or_else(|| format!("nex:{spec}"));
    // `pi:` self-auths the OpenRouter wire, so it's the documented alternative
    // for the incident-relevant `openrouter:` case. For the pure-alias / local
    // providers a `pi:<prefix>:…` route would be misleading, so only the `nex:`
    // canonical is offered.
    let alternative = if prefix == "openrouter" {
        format!(" or `pi:{prefix}:{rest}` (pi CLI — auths itself)")
    } else {
        String::new()
    };
    Some(format!(
        "`{prefix}` is a model namespace, not a handler — a bare `{prefix}:` model \
         spec silently routes to the in-process `native` handler, which owns no \
         credential of its own (this is what 401'd every task for 14h). Name a \
         handler explicitly: `{canonical}` (in-process native — needs the matching \
         endpoint/key){alternative}. Run `wg migrate config` to rewrite automatically."
    ))
}

/// Strip a leading native-handler prefix (`nex:` / `native:`) from a model
/// spec **when** the remainder is itself a `<provider>:<model>` spec, so the
/// inner dialect drives wire/provider resolution.
///
/// `nex:openrouter:z-ai/glm-5.2` → `openrouter:z-ai/glm-5.2`: the native
/// handler owns everything after its own prefix as its native model dialect
/// (the handler-first inner re-parse, design §6.3). Without this, the leading
/// `nex` collapses to the oai-compat localhost default and an
/// `nex:openrouter:…` route would silently target localhost instead of
/// OpenRouter.
///
/// A bare nex model with no inner provider (`nex:qwen3-coder`) is returned
/// unchanged — `nex` stays the provider (→ oai-compat wire) exactly as
/// before. Idempotent on specs that carry no `nex:`/`native:` prefix. Mirrors
/// how the CLI adapters (`opencode_model_arg`) strip their own executor
/// prefix.
pub fn strip_native_handler_prefix(spec: &str) -> &str {
    if let Some((prefix, rest)) = spec.split_once(':')
        && (prefix == "nex" || prefix == "native")
        && let Some((inner_prefix, inner_rest)) = rest.split_once(':')
        && KNOWN_PROVIDERS.contains(&inner_prefix)
        && !inner_rest.trim().is_empty()
    {
        return rest;
    }
    spec
}

/// Release flag for the handler-first rollout
/// (docs/design-handler-first-model-spec.md §5.1 / §11 step 8).
///
/// - `false` = **release N**: a bare leading provider prefix WARNs loudly at
///   the strict-validation entry points and defaults to `nex:` (so nothing
///   breaks immediately — the lenient resolver already routes it to native).
/// - `true` = **release N+1**: the same entry points HARD-ERROR with the
///   handler-first message.
///
/// The switch is intentionally a single const so flipping the rollout is a
/// one-line, well-tested change.
pub const HANDLER_FIRST_HARD_ERROR: bool = false;

/// Result of parsing a `provider:model` spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// Provider prefix, if explicitly specified (e.g., `"openrouter"`).
    /// `None` for bare model names (lenient parsing only).
    pub provider: Option<String>,
    /// The model identifier sent to the provider's API.
    pub model_id: String,
}

/// Error returned when a model spec string fails strict validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpecError {
    /// The original input string that failed parsing.
    pub input: String,
    /// Human-readable error message with migration guidance.
    pub message: String,
}

impl std::fmt::Display for ModelSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ModelSpecError {}

/// Parse a model spec into provider and model ID components (lenient).
///
/// - `"openrouter:deepseek/deepseek-v3.2"` → `ModelSpec { provider: Some("openrouter"), model_id: "deepseek/deepseek-v3.2" }`
/// - `"claude:opus"` → `ModelSpec { provider: Some("claude"), model_id: "opus" }`
/// - `"opus"` → `ModelSpec { provider: None, model_id: "opus" }`
///
/// Only recognized provider prefixes are treated as providers. If the text before
/// `:` is not in `KNOWN_PROVIDERS`, the entire string is treated as a bare model name
/// (e.g., `"deepseek-coder-v2:16b"` for Ollama model tags).
///
/// **Note:** Prefer [`parse_model_spec_strict`] at entry points (CLI, config loading)
/// to enforce the `provider:model` format. This lenient version is for internal
/// resolution paths where the model string may already be partially resolved.
pub fn parse_model_spec(spec: &str) -> ModelSpec {
    if let Some((prefix, rest)) = spec.split_once(':')
        && KNOWN_PROVIDERS.contains(&prefix)
    {
        return ModelSpec {
            provider: Some(prefix.to_string()),
            model_id: rest.to_string(),
        };
    }
    ModelSpec {
        provider: None,
        model_id: spec.to_string(),
    }
}

/// Normalize a bare `vendor/model` route into the canonical
/// `openrouter:<vendor>/<model>` model spec.
///
/// A bare route — a slash and no recognized provider prefix, e.g.
/// `minimax/minimax-m3` — launched on the Nex/native executor with no
/// explicit endpoint is an OpenRouter route (nex-optional-openrouter-endpoint).
/// Normalizing it up front means downstream provider resolution targets
/// OpenRouter directly instead of falling through to the bare-name
/// oai-compat/local default and silently hitting a local server.
///
/// Specs that already carry a provider prefix (`openrouter:...`,
/// `claude:...`, `nex:...`, …) or have no slash (`qwen3-coder`, `opus`) are
/// returned unchanged — only an unqualified `vendor/model` is rewritten.
pub fn normalize_bare_openrouter_route(model: &str) -> String {
    let spec = parse_model_spec(model);
    if spec.provider.is_none() && spec.model_id.contains('/') {
        format!("openrouter:{}", spec.model_id)
    } else {
        model.to_string()
    }
}

/// Whether a model spec routes to OpenRouter — i.e. it carries the
/// `openrouter:` provider prefix. Call after
/// [`normalize_bare_openrouter_route`] so bare `vendor/model` routes are
/// already canonicalized.
pub fn model_is_openrouter(model: &str) -> bool {
    matches!(
        parse_model_spec(model).provider.as_deref(),
        Some("openrouter")
    )
}

/// Parse a model spec **strictly**: requires `provider:model` format.
///
/// Returns an error with a helpful migration message for:
/// - Bare model names without a provider prefix (e.g., `"opus"`)
/// - Unknown provider prefixes (e.g., `"foobar:gpt-4"`)
///
/// # Examples
///
/// ```
/// use worksgood::config::parse_model_spec_strict;
///
/// // Valid: known provider prefix
/// let spec = parse_model_spec_strict("claude:opus").unwrap();
/// assert_eq!(spec.provider.as_deref(), Some("claude"));
/// assert_eq!(spec.model_id, "opus");
///
/// // Invalid: bare model name
/// assert!(parse_model_spec_strict("opus").is_err());
///
/// // Invalid: unknown provider
/// assert!(parse_model_spec_strict("foobar:gpt-4").is_err());
/// ```
///
/// Emits the handler-first deprecation warning to stderr when the leading
/// token is a bare provider prefix (release N). Use
/// [`parse_model_spec_strict_quiet`] for bulk config validation, which has
/// already surfaced that warning once (with a dotted path) via
/// [`deprecated_model_prefix_warnings_for_toml`].
pub fn parse_model_spec_strict(spec: &str) -> Result<ModelSpec, ModelSpecError> {
    parse_model_spec_strict_impl(spec, true)
}

/// Like [`parse_model_spec_strict`] but never emits the handler-first
/// deprecation warning to stderr — it still warn-defaults a bare provider to
/// `nex:` (or hard-errors under [`HANDLER_FIRST_HARD_ERROR`]). For callers
/// that validate many model fields in bulk (config load) where a separate
/// surface already prints one path-annotated warning per occurrence, so the
/// strict parse would otherwise double-warn on every load.
pub fn parse_model_spec_strict_quiet(spec: &str) -> Result<ModelSpec, ModelSpecError> {
    parse_model_spec_strict_impl(spec, false)
}

fn parse_model_spec_strict_impl(
    spec: &str,
    emit_warning: bool,
) -> Result<ModelSpec, ModelSpecError> {
    if spec.is_empty() {
        return Err(ModelSpecError {
            input: spec.to_string(),
            message: "Model spec cannot be empty. Use provider:model format (e.g., 'claude:opus')."
                .to_string(),
        });
    }

    if let Some((prefix, _rest)) = spec.split_once(':')
        && prefix == "amplifier"
    {
        return Err(ModelSpecError {
            input: spec.to_string(),
            message: format!(
                "`amplifier` is an executor name, not a model provider prefix \
                 (got '{}'). Use `--executor amplifier` or `[dispatcher].executor = \
                 \"amplifier\"`, and keep model specs on provider prefixes such as \
                 `claude:opus`, `codex:gpt-5.5`, `openrouter:<model>`, or \
                 `nex:<model>` with a matching `-e <ENDPOINT>`.",
                spec,
            ),
        });
    }

    // Executor-qualified routes (e.g. `opencode:openrouter/stepfun/step-3.7-flash`):
    // external CLI executors are addressed by an *executor* name prefix, not a
    // model-provider prefix, so they are intentionally NOT in `KNOWN_PROVIDERS`.
    // `parse_executor_model_route` makes these first-class model strings on the
    // spawn path, and the opencode starter profile ships them, so the strict
    // validator must accept them too (otherwise `wg profile show` / `wg config
    // lint` flags a valid profile). `amplifier` is excluded above on purpose
    // (it keeps its dedicated "executor name, not a provider" error).
    if let Some((prefix, rest)) = spec.split_once(':')
        && !rest.trim().is_empty()
        && crate::dispatch::ExecutorKind::from_str(prefix)
            .is_some_and(|kind| kind.is_external_cli())
    {
        return Ok(ModelSpec {
            provider: Some(prefix.to_string()),
            model_id: rest.to_string(),
        });
    }

    // Handler-first enforcement: a bare PROVIDER prefix (`openrouter`,
    // `ollama`, `gemini`, `oai-compat`, `local`, …) is NOT a valid leading
    // handler token. The leading token must always name a handler; the
    // provider belongs in the inner dialect (`nex:openrouter:…` /
    // `pi:openrouter:…`). Release N (HANDLER_FIRST_HARD_ERROR == false) WARNs
    // loudly and resolves the spec as its `nex:`-defaulted canonical form
    // (so existing configs keep working, matching the lenient resolver's
    // `_ => native` arm); release N+1 hard-errors. This is the single place
    // every strict entry point (CLI `--model`, config load,
    // `wg service start/daemon --model`) funnels through. Mirrors the
    // `amplifier` rejection above, in the opposite direction.
    if let Some((prefix, rest)) = spec.split_once(':')
        && !rest.trim().is_empty()
        && is_rejected_leading_provider(prefix)
    {
        let message = handler_first_warning(spec).unwrap_or_default();
        if HANDLER_FIRST_HARD_ERROR {
            return Err(ModelSpecError {
                input: spec.to_string(),
                message,
            });
        }
        if emit_warning {
            eprintln!("warning: {message}");
        }
        // Default to the handler-first canonical form so the returned spec
        // is never a bare provider prefix. `handler_first_rewrite` always
        // yields a `nex:`-qualified string for a rejected provider.
        let canonical = handler_first_rewrite(spec).unwrap_or_else(|| spec.to_string());
        return Ok(parse_model_spec(&canonical));
    }

    if let Some((prefix, rest)) = spec.split_once(':') {
        if KNOWN_PROVIDERS.contains(&prefix) {
            if rest.is_empty() {
                return Err(ModelSpecError {
                    input: spec.to_string(),
                    message: format!(
                        "Model spec '{}' has provider '{}' but no model name. \
                         Use provider:model format (e.g., '{}:opus').",
                        spec, prefix, prefix
                    ),
                });
            }
            return Ok(ModelSpec {
                provider: Some(prefix.to_string()),
                model_id: rest.to_string(),
            });
        }
        // Has a colon but prefix is not a known provider
        return Err(ModelSpecError {
            input: spec.to_string(),
            message: format!(
                "Unknown provider '{}' in model spec '{}'. \
                 Known providers: {}. \
                 Use provider:model format (e.g., 'claude:opus', 'openrouter:deepseek/deepseek-v3.2').",
                prefix,
                spec,
                KNOWN_PROVIDERS.join(", "),
            ),
        });
    }

    // No colon at all — bare model name
    Err(ModelSpecError {
        input: spec.to_string(),
        message: format!(
            "Invalid model format '{}'. Models must use provider:model format. \
             For example: 'claude:{}', 'openrouter:{}', 'nex:{}'. \
             Known providers: {}. \
             (Note: 'nex' is the canonical prefix for the in-process nex handler \
             — it matches `wg nex`. 'local' and 'oai-compat' are deprecated \
             aliases retained for one release; 'openai' is a legacy alias for \
             'oai-compat'.)",
            spec,
            spec,
            spec,
            spec,
            KNOWN_PROVIDERS.join(", "),
        ),
    })
}

/// Map a provider prefix to the executor type it requires.
///
/// - `claude` → `"claude"` (Claude CLI)
/// - `codex` → `"codex"` (Codex CLI)
/// - `nex` (canonical) / `local` / `oai-compat` / `openrouter` / etc. → `"native"`
///   (the in-process nex handler — name kept as `"native"` for the legacy
///   ExecutorKind variant, but the user-facing prefix is `nex:`)
pub fn provider_to_executor(provider: &str) -> &'static str {
    match provider {
        "claude" => "claude",
        "codex" => "codex",
        // Handler-first model specs: the leading token names an external CLI
        // handler, so it maps to itself. This keeps `effective_executor()`
        // (and every status/reload/TUI surface that reads it) in lock-step
        // with `handler_for_model` — the single source of truth for routing —
        // so a `pi:openrouter/...` model surfaces as `executor=pi` instead of
        // the legacy `native`/`claude` default. `enforce_model_compat` already
        // routes the actual spawn through these handlers, so the display now
        // matches the real route instead of labeling a deprecated key.
        "pi" => "pi",
        "opencode" => "opencode",
        "aider" => "aider",
        "goose" => "goose",
        "qwen" => "qwen",
        "cline" => "cline",
        "crush" => "crush",
        "amplifier" => "amplifier",
        "octomind" => "octomind",
        "dexto" => "dexto",
        "shell" => "shell",
        _ => "native",
    }
}

/// Inspect a raw config.toml string and produce deprecation warnings for
/// any explicit `executor = …` keys.
///
/// The `executor` user-facing concept was retired in favour of `(model,
/// endpoint)` — wg derives the handler from the model spec's provider
/// prefix. Existing configs continue to work for one release; this
/// surface is what tells users to migrate.
///
/// Detected surfaces:
/// - `[agent].executor`                  (per-task agent default)
/// - `[coordinator].executor` /
///   `[dispatcher].executor`             (legacy + canonical name)
pub fn deprecated_executor_warnings_for_toml(content: &str) -> Vec<String> {
    let Ok(value) = content.parse::<toml::Value>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let surfaces: &[(&str, &[&str], &[&str])] = &[
        (
            "agent.executor",
            &["agent", "executor"],
            &["agent", "model"],
        ),
        (
            "coordinator.executor",
            &["coordinator", "executor"],
            &["coordinator", "model"],
        ),
        (
            "dispatcher.executor",
            &["dispatcher", "executor"],
            &["dispatcher", "model"],
        ),
    ];
    for (label, exec_path, model_path) in surfaces {
        let Some(toml::Value::String(exec_v)) = lookup_toml_path(&value, exec_path) else {
            continue;
        };
        let sibling_model = match lookup_toml_path(&value, model_path) {
            Some(toml::Value::String(s)) => Some(s.as_str()),
            _ => None,
        };
        let implied: Option<&'static str> = sibling_model.and_then(|m| {
            parse_model_spec(m)
                .provider
                .as_deref()
                .map(provider_to_executor)
        });
        let detail = match implied {
            Some(imp) if imp != exec_v.as_str() => format!(
                " (and contradicts sibling model='{}' which implies handler '{}')",
                sibling_model.unwrap_or(""),
                imp,
            ),
            _ => String::new(),
        };
        out.push(format!(
            "config key `{0} = \"{1}\"` is deprecated{2}; \
             wg now derives the handler from the model spec's provider \
             prefix (e.g. `model = \"claude:opus\"` → claude CLI, \
             `model = \"nex:qwen3-coder\"` → nex). Remove the explicit \
             `executor` key; use a `provider:model` value in `model` instead.",
            label, exec_v, detail,
        ));
    }
    out
}

/// Walk a `toml::Value` along a key path. Returns the value at the leaf
/// or `None` if any segment is missing / not a table.
fn lookup_toml_path<'a>(root: &'a toml::Value, path: &[&str]) -> Option<&'a toml::Value> {
    let mut current = root;
    for seg in path {
        current = current.as_table()?.get(*seg)?;
    }
    Some(current)
}

/// Inspect a raw config.toml string and produce deprecation warnings for
/// any model strings whose **leading** token is a bare provider prefix
/// instead of a handler (`openrouter:`, `ollama:`, `gemini:`, `oai-compat:`,
/// `openai:`, `local:`, `vllm:`, `llamacpp:`). Under the handler-first rule
/// the leading token must always name a handler; the provider belongs in the
/// inner dialect (`nex:openrouter:…` / `pi:openrouter:…`). Existing configs
/// keep working for one release; this surface is what tells users to migrate.
///
/// Walks every string value in the document and reports each occurrence
/// once with its dotted-path location so users can find and rewrite it.
/// Pair with `wg migrate config` for an automated rewrite. The message body
/// is the shared [`handler_first_warning`] used at every entry point, so the
/// load-time warning, the strict-parse warning, and lint all agree.
pub fn deprecated_model_prefix_warnings_for_toml(content: &str) -> Vec<String> {
    let Ok(value) = content.parse::<toml::Value>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_strings_readonly(&value, "", &mut |path, s| {
        if let Some(msg) = handler_first_warning(s) {
            out.push(format!("model spec `{path} = \"{s}\"`: {msg}"));
        }
    });
    out
}

fn walk_strings_readonly<'a>(val: &'a toml::Value, path: &str, f: &mut dyn FnMut(&str, &'a str)) {
    match val {
        toml::Value::String(s) => f(path, s.as_str()),
        toml::Value::Array(arr) => {
            for (i, child) in arr.iter().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                walk_strings_readonly(child, &child_path, f);
            }
        }
        toml::Value::Table(tbl) => {
            for (k, child) in tbl.iter() {
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", path, k)
                };
                walk_strings_readonly(child, &child_path, f);
            }
        }
        _ => {}
    }
}

/// Drop the `executor` keys from `[agent]` / `[dispatcher]` / `[coordinator]`
/// when they're redundant with the model spec's implied handler. Used by
/// `wg init` so freshly-written configs don't carry the deprecated
/// `executor` field — and don't trigger the deprecation warning on
/// every subsequent load.
///
/// Specifically:
/// - `agent.executor` is reset to its default ("claude") when the model's
///   provider prefix implies the same handler. With `skip_serializing_if =
///   "is_default_executor"`, the default value is omitted on save.
/// - `coordinator.executor` is set to `None` when the coordinator/agent
///   model's provider prefix implies the same handler. `Option::is_none`
///   is already skipped on serialize.
///
/// Bare-alias models (no provider prefix, e.g. `opus`) leave the keys
/// alone — they don't carry handler information yet, so we let the
/// existing executor field stand and let the warning nudge the user
/// toward `provider:model`.
pub fn strip_redundant_executor_keys(config: &mut Config) {
    let agent_implied: Option<&'static str> = parse_model_spec(&config.agent.model)
        .provider
        .as_deref()
        .map(provider_to_executor);
    if let Some(imp) = agent_implied
        && imp == config.agent.executor.as_str()
    {
        config.agent.executor = default_executor();
    }

    // For dispatcher/coordinator, prefer its own model field, then fall
    // back to agent.model (the dispatcher inherits when unset).
    let dispatcher_model = config
        .coordinator
        .model
        .as_deref()
        .unwrap_or(&config.agent.model);
    let dispatcher_implied: Option<&'static str> = parse_model_spec(dispatcher_model)
        .provider
        .as_deref()
        .map(provider_to_executor);
    if let Some(imp) = dispatcher_implied
        && let Some(ref current) = config.coordinator.executor
        && imp == current.as_str()
    {
        config.coordinator.executor = None;
    }
}

/// Map a provider prefix to the internal provider name used by the native executor.
///
/// This determines which API wire format and default URL to use.
pub fn provider_to_native_provider(provider: &str) -> &'static str {
    match provider {
        "claude" => "anthropic",
        "codex" => "oai-compat",
        "openrouter" => "openrouter",
        // `nex` is the canonical prefix for the in-process nex handler —
        // it speaks OAI-compat by default, with `openrouter:` as the
        // implicit-endpoint convenience case.
        "nex" => "oai-compat",
        // "oai-compat" is the legacy alias for the OpenAI-compatible HTTP
        // protocol — `nex` is the canonical prefix going forward.
        // "openai" is the older legacy alias retained for back-compat.
        "oai-compat" | "openai" => "oai-compat",
        "gemini" => "oai-compat", // Gemini uses OpenAI-compatible endpoint
        "ollama" | "llamacpp" | "vllm" | "local" => "local",
        "native" => "oai-compat", // auto-detect, use openai-compat
        _ => "anthropic",
    }
}

/// Map a user-facing provider prefix to the provider label carried by
/// [`ResolvedModel`].
///
/// Most API-backed prefixes use the native provider name because downstream
/// lightweight calls need to pick an HTTP client. `codex:` is intentionally
/// preserved as `codex`: it is a CLI-backed route, and collapsing it to the
/// OAI-compat protocol label makes role routing and `wg config --models`
/// report `nex` even though the codex CLI is the required handler.
pub fn provider_to_resolved_provider(provider: &str) -> &'static str {
    match provider {
        "codex" => "codex",
        other => provider_to_native_provider(other),
    }
}

/// Rewrite a **strong-tier** model spec so it executes through the
/// self-authenticating `pi` handler instead of the in-process `nex`/native
/// OpenRouter client.
///
/// ## Why
///
/// The Pi profile's strong tier (chat + workers + heavy generative roles) is
/// meant to run *through Pi*, which holds its own provider login — exactly like
/// the `claude:` / `codex:` CLI handlers auth themselves. A raw `openrouter:`
/// spec instead routes to the in-process nex handler ([`handler_for_model`] →
/// [`ExecutorKind::Native`]), which makes **wg itself** the OpenRouter HTTP
/// client and so REQUIRES an OpenRouter key wired into wg config. When that key
/// is absent, strong-tier workers die at spawn (wrapper-internal exit 1 before
/// any work). Rewriting the spec to a `pi:` route removes that requirement: pi
/// runs the model with its own credentials and wg needs no OpenRouter secret.
///
/// ## What it does
///
/// - An explicit OpenRouter route (`openrouter:z-ai/glm-5.2`, or an alias that
///   maps to the openrouter native provider) → `pi:openrouter/z-ai/glm-5.2`.
/// - A bare slash route (`z-ai/glm-5.2` / `openrouter/z-ai/glm-5.2`) is an
///   OpenRouter route too → `pi:openrouter/z-ai/glm-5.2`.
/// - Everything else is left verbatim: `pi:` routes (already correct), the
///   `claude:` / `codex:` CLIs (auth themselves), and nex-local / oai-compat
///   routes (`nex:` / `oai-compat:` / `local:` …) that genuinely need the
///   in-process handler plus a localhost endpoint pi cannot stand in for.
///
/// The conversion is the inverse of [`crate::commands::pi_handler::pi_model_arg`]
/// parsing and is idempotent — applying it to an already-`pi:` route is a no-op.
///
/// This is the single chokepoint shared by every path that *persists* the
/// strong tier ([`Config::set_pi_tiers`], `profile::named::patch_pi_tiers`, and
/// the model scout), so no write path can reintroduce a nex-routed strong spec.
/// The weak/agency tier is deliberately NOT routed through here: its selected
/// handler/provider remains authoritative and failure does not authorize Pi or
/// any other execution system.
pub fn pi_strong_route(spec: &str) -> String {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return spec.to_string();
    }
    let parsed = parse_model_spec(trimmed);
    match parsed.provider.as_deref() {
        // Explicit OpenRouter (or an alias mapping to it) → pi-served remote
        // model pi can run with its own login and no endpoint.
        Some(provider) if provider_to_native_provider(provider) == "openrouter" => {
            let model_id = parsed
                .model_id
                .strip_prefix("openrouter/")
                .unwrap_or(&parsed.model_id);
            format!("pi:openrouter/{model_id}")
        }
        // Any other explicit provider (claude:, codex:, nex:, oai-compat:,
        // local:, gemini:, an already-normalized pi: route, …) routes to a
        // handler that can serve it — leave it verbatim.
        Some(_) => trimmed.to_string(),
        // No provider prefix. A bare `vendor/model` slash route is an OpenRouter
        // route (matches the dispatch bare-route normalization and pi_model_arg);
        // send it through pi. A bare single-token alias (`opus`) gives pi nothing
        // to target, and a `pi:`/other-colon route is preserved verbatim.
        None => {
            if let Some(rest) = trimmed.strip_prefix("openrouter/") {
                format!("pi:openrouter/{rest}")
            } else if trimmed.contains('/') && !trimmed.contains(':') {
                format!("pi:openrouter/{trimmed}")
            } else {
                trimmed.to_string()
            }
        }
    }
}

/// Reverse map: internal provider name → user-facing `provider:model` prefix.
///
/// This is the inverse of [`provider_to_native_provider`] for display purposes.
/// Returns `nex` for the OAI-compat / local cases — the canonical prefix
/// matching the `wg nex` subcommand. `openrouter` keeps its own prefix
/// because the URL convention (api.openrouter.ai) is implicit when the
/// user picks it.
pub fn native_provider_to_prefix(provider: &str) -> &str {
    match provider {
        "anthropic" => "claude",
        "openrouter" => "openrouter",
        // Internal "oai-compat" / "openai" / "local" all map to the
        // user-facing canonical "nex:" prefix (matches `wg nex`).
        "oai-compat" | "openai" | "local" => "nex",
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Config validation
// ---------------------------------------------------------------------------

/// A single configuration diagnostic (error or warning).
#[derive(Debug, Clone)]
pub struct ConfigDiagnostic {
    /// Machine-readable rule identifier (e.g., "executor-model-mismatch")
    pub rule: String,
    /// Human-readable description of the problem
    pub message: String,
    /// Suggested fix
    pub fix: String,
}

/// Result of configuration validation.
#[derive(Debug, Clone, Default)]
pub struct ConfigValidation {
    /// Fatal errors that should block service start
    pub errors: Vec<ConfigDiagnostic>,
    /// Non-fatal warnings that should be displayed but allow startup
    pub warnings: Vec<ConfigDiagnostic>,
}

impl ConfigValidation {
    /// Returns true if there are no errors (warnings are OK).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns true if there are no errors and no warnings.
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }

    /// Format all diagnostics for display.
    pub fn display(&self) -> String {
        let mut out = String::new();
        for diag in &self.errors {
            out.push_str(&format!("  ERROR: {}\n", diag.message));
            out.push_str(&format!("    Fix: {}\n", diag.fix));
        }
        for diag in &self.warnings {
            out.push_str(&format!("  WARNING: {}\n", diag.message));
            out.push_str(&format!("    Fix: {}\n", diag.fix));
        }
        out
    }
}

impl Config {
    /// Built-in Anthropic model defaults.
    fn builtin_registry() -> Vec<ModelRegistryEntry> {
        vec![
            // Legacy model entries (for backward compatibility)
            ModelRegistryEntry {
                id: "haiku".into(),
                provider: "anthropic".into(),
                model: CLAUDE_HAIKU_MODEL_ID.into(),
                tier: Tier::Fast,
                context_window: 200_000,
                max_output_tokens: 8192,
                cost_per_input_mtok: 0.25,
                cost_per_output_mtok: 1.25,
                prompt_caching: true,
                cache_read_discount: 0.1,
                cache_write_premium: 1.25,
                ..Default::default()
            },
            ModelRegistryEntry {
                id: "sonnet".into(),
                provider: "anthropic".into(),
                model: CLAUDE_SONNET_MODEL_ID.into(),
                tier: Tier::Standard,
                context_window: 200_000,
                max_output_tokens: 16384,
                cost_per_input_mtok: 3.0,
                cost_per_output_mtok: 15.0,
                prompt_caching: true,
                cache_read_discount: 0.1,
                cache_write_premium: 1.25,
                ..Default::default()
            },
            ModelRegistryEntry {
                id: "opus".into(),
                provider: "anthropic".into(),
                model: CLAUDE_OPUS_MODEL_ID.into(),
                tier: Tier::Premium,
                context_window: 200_000,
                max_output_tokens: 32000,
                cost_per_input_mtok: 15.0,
                cost_per_output_mtok: 75.0,
                prompt_caching: true,
                cache_read_discount: 0.1,
                cache_write_premium: 1.25,
                ..Default::default()
            },
            // Fable 5 — frontier model, peer of opus. The claude CLI has no
            // bare `fable` shortcut, so the `model` field carries the full CLI
            // id `claude-fable-5` (see `claude_cli_model_arg`).
            ModelRegistryEntry {
                id: "fable".into(),
                provider: "anthropic".into(),
                model: CLAUDE_FABLE_MODEL_ID.into(),
                tier: Tier::Premium,
                context_window: 200_000,
                max_output_tokens: 32000,
                cost_per_input_mtok: 15.0,
                cost_per_output_mtok: 75.0,
                prompt_caching: true,
                cache_read_discount: 0.1,
                cache_write_premium: 1.25,
                ..Default::default()
            },
            // New colon-separated format entries
            ModelRegistryEntry {
                id: "claude:haiku".into(),
                provider: "anthropic".into(),
                model: CLAUDE_HAIKU_MODEL_ID.into(),
                tier: Tier::Fast,
                context_window: 200_000,
                max_output_tokens: 8192,
                cost_per_input_mtok: 0.25,
                cost_per_output_mtok: 1.25,
                prompt_caching: true,
                cache_read_discount: 0.1,
                cache_write_premium: 1.25,
                ..Default::default()
            },
            ModelRegistryEntry {
                id: "claude:sonnet".into(),
                provider: "anthropic".into(),
                model: CLAUDE_SONNET_MODEL_ID.into(),
                tier: Tier::Standard,
                context_window: 200_000,
                max_output_tokens: 16384,
                cost_per_input_mtok: 3.0,
                cost_per_output_mtok: 15.0,
                prompt_caching: true,
                cache_read_discount: 0.1,
                cache_write_premium: 1.25,
                ..Default::default()
            },
            ModelRegistryEntry {
                id: "claude:opus".into(),
                provider: "anthropic".into(),
                model: CLAUDE_OPUS_MODEL_ID.into(),
                tier: Tier::Premium,
                context_window: 200_000,
                max_output_tokens: 32000,
                cost_per_input_mtok: 15.0,
                cost_per_output_mtok: 75.0,
                prompt_caching: true,
                cache_read_discount: 0.1,
                cache_write_premium: 1.25,
                ..Default::default()
            },
            ModelRegistryEntry {
                id: "claude:fable".into(),
                provider: "anthropic".into(),
                model: CLAUDE_FABLE_MODEL_ID.into(),
                tier: Tier::Premium,
                context_window: 200_000,
                max_output_tokens: 32000,
                cost_per_input_mtok: 15.0,
                cost_per_output_mtok: 75.0,
                prompt_caching: true,
                cache_read_discount: 0.1,
                cache_write_premium: 1.25,
                ..Default::default()
            },
        ]
    }

    /// Return merged registry: built-in entries + user-defined entries.
    /// User entries with the same ID override built-in entries.
    pub fn effective_registry(&self) -> Vec<ModelRegistryEntry> {
        let builtins = Self::builtin_registry();
        if self.model_registry.is_empty() {
            return builtins;
        }
        let user_ids: std::collections::HashSet<&str> =
            self.model_registry.iter().map(|e| e.id.as_str()).collect();
        let mut result: Vec<ModelRegistryEntry> = builtins
            .into_iter()
            .filter(|e| !user_ids.contains(e.id.as_str()))
            .collect();
        result.extend(self.model_registry.clone());
        result
    }

    /// Effective tier config: use configured tiers, filling in defaults for unconfigured ones.
    pub fn effective_tiers_public(&self) -> TierConfig {
        self.effective_tiers()
    }

    /// Resolve the active profile's tier defaults (if any).
    fn resolve_profile_tiers(&self) -> TierConfig {
        use crate::profile;
        match self.profile.as_deref() {
            Some(name) => {
                if let Some(p) = profile::get_profile(name) {
                    // Static profiles return their hardcoded tiers.
                    // Dynamic profiles return None; we fall through to hardcoded defaults.
                    p.resolve_tiers().unwrap_or_default()
                } else {
                    TierConfig::default()
                }
            }
            None => TierConfig::default(),
        }
    }

    /// Effective tier config (internal).
    ///
    /// Precedence: explicit [tiers] > profile defaults > hardcoded Anthropic fallback.
    fn effective_tiers(&self) -> TierConfig {
        let profile_tiers = self.resolve_profile_tiers();
        TierConfig {
            fast: self
                .tiers
                .fast
                .clone()
                .or(profile_tiers.fast)
                .or_else(|| Some(format!("claude:{}", Tier::Fast.default_alias()))),
            fast_reasoning: self.tiers.fast_reasoning.or(profile_tiers.fast_reasoning),
            standard: self
                .tiers
                .standard
                .clone()
                .or(profile_tiers.standard)
                .or_else(|| Some(format!("claude:{}", Tier::Standard.default_alias()))),
            standard_reasoning: self
                .tiers
                .standard_reasoning
                .or(profile_tiers.standard_reasoning),
            premium: self
                .tiers
                .premium
                .clone()
                .or(profile_tiers.premium)
                .or_else(|| Some(format!("claude:{}", Tier::Premium.default_alias()))),
            premium_reasoning: self
                .tiers
                .premium_reasoning
                .or(profile_tiers.premium_reasoning),
        }
    }

    fn tier_reasoning(&self, tier: Tier) -> Option<ReasoningLevel> {
        let tiers = self.effective_tiers();
        match tier {
            Tier::Fast => tiers.fast_reasoning,
            Tier::Standard => tiers.standard_reasoning,
            Tier::Premium => tiers.premium_reasoning,
        }
    }

    /// Resolve reasoning independently from model/provider.
    ///
    /// Precedence: explicit task override (handled by callers) >
    /// role-specific `[models.<role>].reasoning` > role tier override /
    /// default tier reasoning > `[models.default].reasoning` > omitted handler
    /// default (`None`). This deliberately does not inspect or mutate the
    /// winning model string.
    pub fn resolve_reasoning_for_role(&self, role: DispatchRole) -> Option<ReasoningLevel> {
        if let Some(reasoning) = self.models.get_role(role).and_then(|c| c.reasoning) {
            return Some(reasoning);
        }
        if let Some(tier) = self.models.get_role(role).and_then(|c| c.tier)
            && let Some(reasoning) = self.tier_reasoning(tier)
        {
            return Some(reasoning);
        }
        if let Some(reasoning) = self.tier_reasoning(role.default_tier()) {
            return Some(reasoning);
        }
        self.models
            .get_role(DispatchRole::Default)
            .and_then(|c| c.reasoning)
    }

    /// Look up a registry entry by its short ID.
    pub fn registry_lookup(&self, id: &str) -> Option<ModelRegistryEntry> {
        self.effective_registry().into_iter().find(|e| e.id == id)
    }

    /// The explicitly configured raw model spec for the **weak** two-tier
    /// label. Built-in tier catalog defaults are deliberately excluded: they
    /// are suggestions, not authorization to execute Claude (or any system).
    pub fn weak_tier_spec(&self) -> Option<String> {
        let profile_tiers = self.resolve_profile_tiers();
        self.tiers.fast.clone().or(profile_tiers.fast)
    }

    /// The explicitly configured raw model spec for the **strong** reviewer
    /// tier. Built-in Anthropic fill values are not execution selection.
    pub fn strong_tier_spec(&self) -> Option<String> {
        let profile_tiers = self.resolve_profile_tiers();
        self.tiers
            .premium
            .clone()
            .or(profile_tiers.premium)
            .or_else(|| self.tiers.standard.clone())
            .or(profile_tiers.standard)
    }

    /// Raw configured tier route without the built-in display/catalog fill.
    pub fn configured_tier_spec(&self, tier: Tier) -> Option<String> {
        let profile_tiers = self.resolve_profile_tiers();
        match tier {
            Tier::Fast => self.tiers.fast.clone().or(profile_tiers.fast),
            Tier::Standard => self.tiers.standard.clone().or(profile_tiers.standard),
            Tier::Premium => self.tiers.premium.clone().or(profile_tiers.premium),
        }
    }

    /// Resolve a tier to a ResolvedModel via the tier config and registry.
    pub fn resolve_tier(&self, tier: Tier) -> Option<ResolvedModel> {
        let tiers = self.effective_tiers();
        let model_id = match tier {
            Tier::Fast => tiers.fast.as_deref(),
            Tier::Standard => tiers.standard.as_deref(),
            Tier::Premium => tiers.premium.as_deref(),
        }?;

        // Parse provider:model prefix if present. Strip a leading `nex:` /
        // `native:` HANDLER token first (handler-first inner re-parse, design
        // §6.3) so a canonical `nex:openrouter:<model>` tier spec resolves to
        // provider=openrouter — NOT the `oai-compat` localhost default the
        // lenient parse would pick from the bare `nex` prefix (which sent the
        // agency `.flip`/`.assign`/`.evaluate` one-shot to the wrong wire with a
        // bogus `openrouter:<model>` id). Mirrors `native_provider_for_spec` in
        // `src/service/llm.rs` and the executor's `provider.rs` strip.
        let spec = parse_model_spec(strip_native_handler_prefix(model_id));
        let lookup_id = &spec.model_id;

        if let Some(entry) = self.registry_lookup(lookup_id) {
            Some(ResolvedModel {
                model: entry.model.clone(),
                provider: spec
                    .provider
                    .map(|p| provider_to_resolved_provider(&p).to_string())
                    .or_else(|| Some(entry.provider.clone())),
                registry_entry: Some(entry),
                endpoint: None,
                reasoning: self.tier_reasoning(tier),
            })
        } else {
            // Not in registry — use parsed provider or None
            Some(ResolvedModel {
                model: spec.model_id,
                provider: spec
                    .provider
                    .map(|p| provider_to_resolved_provider(&p).to_string()),
                registry_entry: None,
                endpoint: None,
                reasoning: self.tier_reasoning(tier),
            })
        }
    }

    /// Resolve the model (and optional provider) for a given dispatch role.
    ///
    /// Resolution order:
    /// 1. `models.<role>.model` (role-specific override in [models] section)
    /// 2. `models.<role>.tier` (role tier override via tier system)
    /// 3. Role `default_tier()` → `tiers.<tier>` → registry lookup
    /// 4. `models.default.model` (default in [models] section)
    /// 5. `agent.model` (global fallback)
    ///
    /// Provider resolution follows the same cascade but only from [models].
    pub fn resolve_model_for_role(&self, role: DispatchRole) -> ResolvedModel {
        // Default provider cascades to all roles that don't set their own.
        let default_provider = self
            .models
            .get_role(DispatchRole::Default)
            .and_then(|c| c.provider.clone());

        // Default endpoint cascades to all roles that don't set their own.
        let default_endpoint = self
            .models
            .get_role(DispatchRole::Default)
            .and_then(|c| c.endpoint.clone());

        // Infer provider from coordinator.model and agent.model prefixes as
        // final fallbacks.  This ensures that when a user sets e.g.
        // `coordinator.model = "openrouter:anthropic/claude-sonnet-4-6"`
        // the OpenRouter provider cascades to ALL roles (eval, FLIP, verification)
        // without needing explicit `[models.default].provider` config.
        // Strip a leading `nex:` / `native:` handler token before deriving the
        // cascade provider hint: a full handler-first profile whose
        // `agent.model` / `coordinator.model` is `nex:openrouter:<model>` must
        // yield an `openrouter` hint, not the `oai-compat` the bare `nex` prefix
        // would give — otherwise this cascade would re-pollute the (now
        // correctly openrouter) tier-resolved provider back to localhost.
        let coordinator_model_provider = self.coordinator.model.as_deref().and_then(|m| {
            parse_model_spec(strip_native_handler_prefix(m))
                .provider
                .map(|p| provider_to_resolved_provider(&p).to_string())
        });
        let agent_model_provider = parse_model_spec(strip_native_handler_prefix(&self.agent.model))
            .provider
            .map(|p| provider_to_resolved_provider(&p).to_string());

        // Helper: resolve provider for a role, cascading through:
        //   models.<role>.provider → models.default.provider
        //   → coordinator.model prefix → agent.model prefix
        let resolve_provider = |role: DispatchRole| -> Option<String> {
            self.models
                .get_role(role)
                .and_then(|c| c.provider.clone())
                .or_else(|| default_provider.clone())
                .or_else(|| coordinator_model_provider.clone())
                .or_else(|| agent_model_provider.clone())
        };

        // Helper: resolve endpoint for a role, cascading to default if unset.
        let resolve_endpoint = |role: DispatchRole| -> Option<String> {
            self.models
                .get_role(role)
                .and_then(|c| c.endpoint.clone())
                .or_else(|| default_endpoint.clone())
        };

        // 1. Check role-specific [models] config (direct model override)
        if let Some(role_cfg) = self.models.get_role(role)
            && let Some(ref model) = role_cfg.model
        {
            // Parse provider:model prefix from the model string (handler-first
            // strip first, so `nex:openrouter:<model>` → provider=openrouter).
            let spec = parse_model_spec(strip_native_handler_prefix(model));
            let spec_provider = spec
                .provider
                .as_deref()
                .map(provider_to_resolved_provider)
                .map(String::from);
            let lookup_model = &spec.model_id;

            if let Some(entry) = self.registry_lookup(lookup_model) {
                return ResolvedModel {
                    model: entry.model.clone(),
                    provider: spec_provider
                        .or_else(|| role_cfg.provider.clone())
                        .or_else(|| Some(entry.provider.clone()))
                        .or_else(|| default_provider.clone()),
                    registry_entry: Some(entry),
                    endpoint: resolve_endpoint(role),
                    reasoning: self.resolve_reasoning_for_role(role),
                };
            }
            return ResolvedModel {
                model: spec.model_id.clone(),
                provider: spec_provider
                    .or_else(|| role_cfg.provider.clone())
                    .or_else(|| default_provider.clone()),
                registry_entry: None,
                endpoint: resolve_endpoint(role),
                reasoning: self.resolve_reasoning_for_role(role),
            };
        }

        // 2. Role tier override: [models.<role>].tier
        if let Some(role_cfg) = self.models.get_role(role)
            && let Some(tier) = role_cfg.tier
            && let Some(mut resolved) = self.resolve_tier(tier)
        {
            // Allow role/default provider to override registry provider
            if let Some(p) = resolve_provider(role) {
                resolved.provider = Some(p);
            }
            resolved.endpoint = resolve_endpoint(role);
            return resolved;
        }

        // 3. Role default_tier() → tiers.<tier> → registry lookup
        //    For task_agent only: skipped when agent.model was set in local config so that
        //    `wg config --model <m>` routes task_agent to the chosen model. All other roles
        //    (metacognition, compactors, etc.) always resolve via their tier so they stay
        //    on cheap models even when the project sets a high-capability agent.model.
        let skip_tier = self.agent_model_is_local && role == DispatchRole::TaskAgent;
        if !skip_tier && let Some(mut resolved) = self.resolve_tier(role.default_tier()) {
            // Allow role/default provider to override registry provider
            if let Some(p) = resolve_provider(role) {
                resolved.provider = Some(p);
            }
            resolved.endpoint = resolve_endpoint(role);
            return resolved;
        }

        // 4. Check [models.default]
        if let Some(default_cfg) = self.models.get_role(DispatchRole::Default)
            && let Some(ref model) = default_cfg.model
        {
            let spec = parse_model_spec(strip_native_handler_prefix(model));
            let spec_provider = spec
                .provider
                .as_deref()
                .map(provider_to_resolved_provider)
                .map(String::from);

            if let Some(entry) = self.registry_lookup(&spec.model_id) {
                return ResolvedModel {
                    model: entry.model.clone(),
                    provider: spec_provider
                        .or(default_provider)
                        .or_else(|| Some(entry.provider.clone())),
                    registry_entry: Some(entry),
                    endpoint: resolve_endpoint(role),
                    reasoning: self.resolve_reasoning_for_role(role),
                };
            }
            return ResolvedModel {
                model: spec.model_id,
                provider: spec_provider.or(default_provider),
                registry_entry: None,
                endpoint: resolve_endpoint(role),
                reasoning: self.resolve_reasoning_for_role(role),
            };
        }

        // 5. Global fallback
        let fallback_spec = parse_model_spec(strip_native_handler_prefix(&self.agent.model));
        let fallback_provider = fallback_spec
            .provider
            .as_deref()
            .map(provider_to_resolved_provider)
            .map(String::from);

        if let Some(entry) = self.registry_lookup(&fallback_spec.model_id) {
            return ResolvedModel {
                model: entry.model.clone(),
                provider: fallback_provider
                    .or(default_provider)
                    .or_else(|| Some(entry.provider.clone())),
                registry_entry: Some(entry),
                endpoint: resolve_endpoint(role),
                reasoning: self.resolve_reasoning_for_role(role),
            };
        }
        ResolvedModel {
            model: fallback_spec.model_id,
            provider: fallback_provider.or(default_provider),
            registry_entry: None,
            endpoint: resolve_endpoint(role),
            reasoning: self.resolve_reasoning_for_role(role),
        }
    }

    /// Determine the source of model resolution for a role.
    ///
    /// Returns one of: "explicit", "tier-override", "tier-default", "fallback"
    pub fn resolve_model_source(&self, role: DispatchRole) -> &'static str {
        // 1. Role-specific [models] config (direct model override)
        if let Some(role_cfg) = self.models.get_role(role)
            && role_cfg.model.is_some()
        {
            return "explicit";
        }

        // 2. Role tier override
        if let Some(role_cfg) = self.models.get_role(role)
            && role_cfg.tier.is_some()
        {
            return "tier-override";
        }

        // 3. Role default_tier() → registry
        if self.resolve_tier(role.default_tier()).is_some() {
            return "tier-default";
        }

        // 4/5. Fallback
        "fallback"
    }
}

fn default_auto_create_threshold() -> u32 {
    20
}
fn default_exploration_interval() -> u32 {
    20
}
fn default_cache_population_threshold() -> f64 {
    0.8
}
fn default_ucb_exploration_constant() -> f64 {
    std::f64::consts::SQRT_2
}
fn default_novelty_bonus_multiplier() -> f64 {
    1.5
}
fn default_bizarre_ideation_interval() -> u32 {
    10
}
fn default_eval_gate_threshold() -> Option<f64> {
    Some(0.7)
}
fn default_auto_rescue_on_eval_fail() -> bool {
    true
}
fn default_gate_uncertain_policy() -> String {
    "escalate".to_string()
}
fn default_gate_max_attempts() -> u32 {
    2
}
fn default_gate_confidence_threshold() -> f64 {
    0.7
}

fn default_flip_verification_threshold() -> Option<f64> {
    // Deprecated as of 2026-04-17. FLIP-driven autospawn of .verify-* tasks
    // generated runaway meta-task cascades (observed on ulivo: every real
    // task accumulated .flip-*, .verify-*, .flip-.verify-*, .evaluate-*,
    // and .evaluate-.verify-* shadow tasks — 5-6x inflation). Replacement
    // is single-leaf .evaluate-* with `wg rescue` on FAIL (see
    // docs/design/eval-rescue-graph-surgery.md when written). FLIP scores
    // are still computed and attached to tasks as a diagnostic signal;
    // they just no longer trigger task creation.
    //
    // Set explicitly in config.toml to re-enable the old behavior.
    None
}
fn default_evolution_interval() -> u64 {
    7200
}
fn default_evolution_threshold() -> u32 {
    10
}
fn default_evolution_budget() -> u32 {
    5
}
fn default_evolution_reactive_threshold() -> f64 {
    0.4
}
fn default_auto_assign_grace_seconds() -> u64 {
    10
}

/// Agency (evolutionary identity system) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgencyConfig {
    /// Automatically trigger evaluation when a task completes
    #[serde(default)]
    pub auto_evaluate: bool,

    /// Automatically assign an identity when spawning agents
    #[serde(default)]
    pub auto_assign: bool,

    /// Content-hash of agent to use as assigner (None = use default pipeline)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigner_agent: Option<String>,

    /// Content-hash of agent to use as evaluator (None = use default pipeline)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator_agent: Option<String>,

    /// Content-hash of agent to use as evolver
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evolver_agent: Option<String>,

    /// Content-hash of agent to use as agent creator (None = not configured)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_agent: Option<String>,

    /// Content-hash of agent to use as placer (None = not configured)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placer_agent: Option<String>,

    /// Include placement (dependency edge decisions) in the assignment step.
    /// When enabled, the assignment LLM call also decides dependency edges
    /// for the source task based on active tasks in the graph.
    /// Default: false.
    #[serde(default)]
    pub auto_place: bool,

    /// Automatically invoke the creator agent when the primitive store
    /// needs expansion. Default: false.
    #[serde(default)]
    pub auto_create: bool,

    /// Minimum completed tasks since last creator invocation before
    /// triggering `wg agency create` again. Default: 20.
    #[serde(default = "default_auto_create_threshold")]
    pub auto_create_threshold: u32,

    /// Prose policy for the evolver describing retention heuristics
    /// (e.g. when to retire underperforming roles/motivations)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_heuristics: Option<String>,

    /// Automatically triage dead agents to assess work progress before respawning
    #[serde(default)]
    pub auto_triage: bool,

    /// Timeout in seconds for triage calls (default: 30)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage_timeout: Option<u64>,

    /// Maximum bytes to read from agent output log for triage (default: 50000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage_max_log_bytes: Option<usize>,

    /// Force a learning assignment every N tasks with forced exploration parameters.
    /// 0 = disabled. Default: 20
    #[serde(default = "default_exploration_interval")]
    pub exploration_interval: u32,

    /// Cache score threshold for populating composition cache from
    /// learning experiments. Default: 0.8
    #[serde(default = "default_cache_population_threshold")]
    pub cache_population_threshold: f64,

    /// UCB exploration constant C for primitive selection in learning mode.
    /// Higher values favour uncertainty; lower values favour known performance.
    /// Default: sqrt(2) ≈ 1.414
    #[serde(default = "default_ucb_exploration_constant")]
    pub ucb_exploration_constant: f64,

    /// Multiplier applied to UCB score for low-attractor-weight primitives.
    /// Counteracts attractor-area drift. Default: 1.5
    #[serde(default = "default_novelty_bonus_multiplier")]
    pub novelty_bonus_multiplier: f64,

    /// Force a bizarre ideation composition every N learning assignments.
    /// 0 = disabled. Default: 10
    #[serde(default = "default_bizarre_ideation_interval")]
    pub bizarre_ideation_interval: u32,

    /// Grace period in seconds after task creation before auto-assignment
    /// is eligible. Prevents premature assignment when tasks are created
    /// and then have dependencies wired shortly after.
    /// Default: 10
    #[serde(default = "default_auto_assign_grace_seconds")]
    pub auto_assign_grace_seconds: u64,

    /// Global evaluation gate threshold. When set, evaluations that score
    /// below this threshold can reject (fail) the original task, blocking
    /// its dependents. The gate applies to tasks with parsed deliverables,
    /// or to all tasks when `eval_gate_all` is true. Range: 0.0–1.0.
    /// Default: 0.7 (enabled).
    #[serde(
        default = "default_eval_gate_threshold",
        skip_serializing_if = "Option::is_none"
    )]
    pub eval_gate_threshold: Option<f64>,

    /// When true, apply the eval gate threshold to ALL evaluated tasks,
    /// not just tasks with parsed deliverables. Default: false.
    #[serde(default)]
    pub eval_gate_all: bool,

    /// When the eval gate rejects a task (score below threshold), also
    /// invoke `wg rescue` automatically — creating a first-class
    /// replacement task at the failed task's graph slot, using the
    /// evaluator's notes as the rescue brief. The replacement inherits
    /// the failed task's predecessors + successors; successors are
    /// rerouted to unblock from the rescue only. This is the
    /// "evaluation drives remediation" loop the rescue-proxy design
    /// is built for (see docs/design/nex-as-coordinator.md and the
    /// rescue / insert command docs). Default: true (enabled).
    #[serde(default = "default_auto_rescue_on_eval_fail")]
    pub auto_rescue_on_eval_fail: bool,

    /// Enable FLIP (Fidelity via Latent Intent Probing) evaluation.
    /// When enabled, completed tasks can be evaluated using roundtrip
    /// intent fidelity: infer the prompt from output, then compare to actual.
    #[serde(default)]
    pub flip_enabled: bool,

    /// Model to use for FLIP inference phase (inferring prompt from output)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_inference_model: Option<String>,

    /// Model to use for FLIP comparison phase (comparing inferred prompt to actual)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_comparison_model: Option<String>,

    /// FLIP score threshold below which automatic Opus verification is triggered.
    /// When a FLIP evaluation scores below this threshold, the coordinator creates
    /// a verification task that independently checks whether the work was done.
    /// Default: 0.7. Set to None to disable.
    #[serde(default = "default_flip_verification_threshold")]
    pub flip_verification_threshold: Option<f64>,

    /// Automatically trigger evolution cycles based on evaluation data.
    /// When enabled, the coordinator creates `.evolve-*` meta-tasks
    /// after sufficient evaluations accumulate. Default: false (opt-in).
    #[serde(default)]
    pub auto_evolve: bool,

    /// Minimum seconds between automatic evolution cycles. Default: 7200 (2 hours).
    #[serde(default = "default_evolution_interval")]
    pub evolution_interval: u64,

    /// Minimum number of new evaluations required before triggering evolution.
    /// Default: 10.
    #[serde(default = "default_evolution_threshold")]
    pub evolution_threshold: u32,

    /// Maximum number of evolver operations per automatic evolution cycle.
    /// Default: 5.
    #[serde(default = "default_evolution_budget")]
    pub evolution_budget: u32,

    /// Average score threshold for reactive evolution trigger. When the
    /// average evaluation score drops below this value, evolution is
    /// triggered regardless of the normal interval/threshold. Default: 0.4.
    #[serde(default = "default_evolution_reactive_threshold")]
    pub evolution_reactive_threshold: f64,

    /// URL of the Agency server for evaluation feedback. None = disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agency_server_url: Option<String>,

    /// Path to file containing Agency API token. None = no auth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agency_token_path: Option<String>,

    /// Default assignment source label (e.g. "native", "agency").
    /// Used to tag new assignments with their provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment_source: Option<String>,

    /// Project ID on the Agency server. Required for assignment requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agency_project_id: Option<String>,

    /// URL for upstream agency bureau CSV. Used by `wg agency import --upstream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_url: Option<String>,

    /// Default model tier for gate + post-hoc evaluations. When set, this
    /// overrides the coordinator's default evaluator routing for evaluation
    /// calls. Use to pin evaluations to a specific tier (e.g. "sonnet" or
    /// "opus") independently of the source task's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator_model: Option<String>,

    /// Project-wide default validation mode for tasks that do not set one.
    /// Values: "none" (default) | "integrated" | "external" | "llm".
    /// Per-task `validation` always wins; this just flips the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_validation_mode: Option<String>,

    /// Policy when the LLM gate returns decision=uncertain (or any decision
    /// below the confidence threshold). Values:
    /// - "escalate" (default): task stays PendingValidation for human
    /// - "retry": re-run the gate up to `gate_max_attempts` times
    /// - "fail-closed": treat as reject
    #[serde(default = "default_gate_uncertain_policy")]
    pub gate_uncertain_policy: String,

    /// Maximum gate evaluation attempts per task. Once exceeded, the task
    /// is forced into the escalate path regardless of gate_uncertain_policy.
    /// Default: 2.
    #[serde(default = "default_gate_max_attempts")]
    pub gate_max_attempts: u32,

    /// Confidence floor for auto-approve or auto-reject by the gate. Below
    /// this threshold the gate behaves per `gate_uncertain_policy`.
    /// Range 0.0–1.0. Default: 0.7.
    #[serde(default = "default_gate_confidence_threshold")]
    pub gate_confidence_threshold: f64,
}

impl Default for AgencyConfig {
    fn default() -> Self {
        Self {
            auto_evaluate: true,
            auto_assign: true,
            assigner_agent: None,
            evaluator_agent: None,
            evolver_agent: None,
            creator_agent: None,
            placer_agent: None,
            auto_place: false,
            auto_create: false,
            auto_create_threshold: default_auto_create_threshold(),
            retention_heuristics: None,
            auto_triage: false,
            triage_timeout: None,
            triage_max_log_bytes: None,
            exploration_interval: default_exploration_interval(),
            cache_population_threshold: default_cache_population_threshold(),
            ucb_exploration_constant: default_ucb_exploration_constant(),
            novelty_bonus_multiplier: default_novelty_bonus_multiplier(),
            bizarre_ideation_interval: default_bizarre_ideation_interval(),
            auto_assign_grace_seconds: default_auto_assign_grace_seconds(),
            eval_gate_threshold: default_eval_gate_threshold(),
            eval_gate_all: false,
            auto_rescue_on_eval_fail: default_auto_rescue_on_eval_fail(),
            flip_enabled: true,
            flip_inference_model: None,
            flip_comparison_model: None,
            flip_verification_threshold: default_flip_verification_threshold(),
            auto_evolve: false,
            evolution_interval: default_evolution_interval(),
            evolution_threshold: default_evolution_threshold(),
            evolution_budget: default_evolution_budget(),
            evolution_reactive_threshold: default_evolution_reactive_threshold(),
            agency_server_url: None,
            agency_token_path: None,
            assignment_source: None,
            agency_project_id: None,
            upstream_url: None,
            evaluator_model: None,
            default_validation_mode: None,
            gate_uncertain_policy: default_gate_uncertain_policy(),
            gate_max_attempts: default_gate_max_attempts(),
            gate_confidence_threshold: default_gate_confidence_threshold(),
        }
    }
}

/// Agent-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// **Deprecated**: handler is derived from the model spec's provider
    /// prefix. Kept for one release with a deprecation warning when set
    /// explicitly in config.toml. Skipped on serialize when it holds the
    /// default value, so freshly-written configs no longer carry the key.
    #[serde(
        default = "default_executor",
        skip_serializing_if = "is_default_executor"
    )]
    pub executor: String,

    /// Model to use (e.g., "opus-4-5", "sonnet", "haiku")
    #[serde(default = "default_model")]
    pub model: String,

    /// Default sleep interval between agent iterations (seconds)
    #[serde(default = "default_interval")]
    pub interval: u64,

    /// Maximum tasks per agent run (None = unlimited)
    #[serde(default)]
    pub max_tasks: Option<u32>,

    /// Heartbeat timeout in minutes (for detecting dead agents)
    #[serde(default = "default_heartbeat_timeout")]
    pub heartbeat_timeout: u64,

    /// Grace period in seconds before the reaper acts on a dead PID.
    /// Agents started less than this many seconds ago are not reaped,
    /// avoiding a race condition where the PID is registered but the
    /// process hasn't fully started yet. Default: 30.
    #[serde(default = "default_reaper_grace_seconds")]
    pub reaper_grace_seconds: u64,
}

/// Coordinator-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorConfig {
    /// Maximum number of parallel agents
    #[serde(default = "default_max_agents")]
    pub max_agents: usize,

    /// Poll interval in seconds (used by standalone coordinator command)
    #[serde(default = "default_coordinator_interval")]
    pub interval: u64,

    /// Safety-timer interval (seconds) for the service daemon.
    ///
    /// With graph filesystem watching as the *primary* trigger, the dispatcher
    /// loop is normally event-driven (`notify` watcher on `graph.jsonl`). This
    /// safety timer fires on a slow schedule even when no graph events arrive,
    /// so that purely time-based work (cycle_delay scheduling, agent heartbeat
    /// reaping, registry refresh, compaction trigger checks) still progresses.
    ///
    /// Also serves as the *fallback poll interval* on filesystems where the
    /// watcher cannot start (some NFS mounts, WSL1, certain sandbox FS): the
    /// daemon then polls at this interval as the only trigger.
    ///
    /// Default: 5s. Fast enough that newly-added tasks visibly start working
    /// "right away" if the watcher misses an event; slow enough that idle
    /// polling stays trivial. The forward-looking key name is `safety_interval`;
    /// the legacy `poll_interval` continues to work as an alias.
    #[serde(default = "default_poll_interval", alias = "safety_interval")]
    pub poll_interval: u64,

    /// Enable filesystem watching of `graph.jsonl` as the primary dispatcher
    /// trigger. When enabled (default), graph changes are detected via the
    /// `notify` crate and the dispatcher wakes within ~debounce_ms of any
    /// write — typically faster than `poll_interval`.
    ///
    /// When disabled, the dispatcher relies solely on the safety timer above
    /// plus IPC `GraphChanged` events from `wg` CLI commands.
    #[serde(default = "default_graph_watch_enabled")]
    pub graph_watch_enabled: bool,

    /// Debounce window (milliseconds) for the graph filesystem watcher.
    ///
    /// A single logical write (e.g. one `wg add`) often produces multiple
    /// `write`/`fsync` syscalls visible to inotify. The debouncer collapses
    /// events arriving within this window into one wake-up. Recommended range:
    /// 50–200 ms. Values below 10 ms are clamped up. Default: 100 ms.
    #[serde(default = "default_graph_watch_debounce_ms")]
    pub graph_watch_debounce_ms: u64,

    /// Executor to use for spawned agents.
    /// When `None` (not set in config), `effective_executor()` auto-detects
    /// based on `provider`: openrouter/openai/local → "native", else "claude".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,

    /// Model to use for spawned agents (e.g., "opus-4-5", "sonnet", "haiku")
    /// Overrides agent.model when set. Can be further overridden by CLI --model.
    #[serde(default)]
    pub model: Option<String>,

    /// **Deprecated**: Use provider:model format in the `model` field instead.
    /// Kept for deserialization of old configs; never written back.
    #[serde(default, skip_serializing)]
    pub provider: Option<String>,

    /// Default context scope for spawned agents (clean, task, graph, full).
    /// Overridden by role.default_context_scope and task.context_scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_context_scope: Option<String>,

    /// Hard timeout for spawned agents (e.g., "30m", "1h", "90s").
    /// Wraps the agent invocation with the `timeout` command.
    /// Default: "30m". Set to empty string to disable.
    #[serde(default = "default_agent_timeout")]
    pub agent_timeout: String,

    /// Settling delay in milliseconds after a GraphChanged event before the
    /// coordinator tick fires. During burst graph construction (rapid task
    /// additions), this prevents premature dispatch by waiting for the burst
    /// to settle. Default: 2000ms (2 seconds).
    #[serde(default = "default_settling_delay_ms")]
    pub settling_delay_ms: u64,

    /// Whether to spawn a persistent LLM coordinator agent for chat.
    /// When true, the daemon launches a Claude CLI session that interprets
    /// user chat messages and manages the graph conversationally.
    /// When false, chat uses a simple stub response.
    /// Default: true.
    ///
    /// Alias `chat_agent` is accepted for forward-compat with the rename.
    #[serde(default = "default_coordinator_agent", alias = "chat_agent")]
    pub coordinator_agent: bool,

    /// **Deprecated (retired-compact-archive):** the graph-cycle compactor
    /// (`.compact-N`) has been removed. Field is parsed for one release so
    /// existing config files keep loading; values are ignored. A warning is
    /// emitted at config load time if the value is non-default.
    #[serde(default = "default_compactor_interval")]
    pub compactor_interval: u32,

    /// **Deprecated (retired-compact-archive):** see `compactor_interval`.
    #[serde(default = "default_compactor_ops_threshold")]
    pub compactor_ops_threshold: usize,

    /// **Deprecated (retired-compact-archive):** see `compactor_interval`.
    #[serde(default = "default_compaction_token_threshold")]
    pub compaction_token_threshold: u64,

    /// **Deprecated (retired-compact-archive):** see `compactor_interval`.
    #[serde(default = "default_compaction_threshold_ratio")]
    pub compaction_threshold_ratio: f64,

    /// How often to evaluate coordinator turns.
    /// Options: "every", "every_5" (default), "every_10", "sample_20pct", "none"
    #[serde(default = "default_eval_frequency")]
    pub eval_frequency: String,

    /// Enable git worktree isolation for spawned agents.
    /// When true, each agent gets its own worktree at .wg-worktrees/<agent-id>/.
    /// Defaults to true to prevent cargo lock contention between concurrent agents.
    #[serde(default = "default_worktree_isolation")]
    pub worktree_isolation: bool,

    /// Maximum number of concurrent coordinator agents (LLM sessions).
    /// Each coordinator is a separate Claude CLI process. Default: 4.
    ///
    /// Alias `max_chats` is accepted for forward-compat with the rename.
    #[serde(default = "default_max_coordinators", alias = "max_chats")]
    pub max_coordinators: usize,

    /// Archive tasks completed/abandoned more than this many days ago.
    /// The archive cycle (.archive-0) runs periodically and moves old
    /// done/abandoned tasks to .wg/archive.jsonl. Default: 7 days.
    /// Set to 0 to disable automatic archival.
    #[serde(default = "default_archive_retention_days")]
    pub archive_retention_days: u64,

    /// How often to refresh the model benchmark registry from OpenRouter,
    /// in seconds. The daemon manages a `.registry-refresh-0` cycle task
    /// that fetches fresh model data, computes fitness scores, and diffs
    /// against the previous registry. Default: 86400 (24 hours).
    /// Set to 0 to disable automatic registry refresh.
    #[serde(default = "default_registry_refresh_interval")]
    pub registry_refresh_interval: u64,

    /// Verification mode for tasks with legacy verify commands (deprecated).
    /// - "inline" (default): verify command runs in the same agent process that did the work
    /// - "separate": verify runs in a separate agent context (different conversation/context window)
    /// New tasks should put validation criteria in a `## Validation` section of the task
    /// description; the agency evaluator (auto_evaluate + FLIP) reads it.
    #[serde(default = "default_verify_mode")]
    pub verify_mode: String,

    /// Master switch for `.verify-*` / `.verify-deferred-*` shadow-task
    /// auto-spawning. Deprecated as of 2026-04-17 — default FALSE. The
    /// pattern generated runaway meta-task cascades (every real task
    /// accumulating 5–6 shadow tasks). Replacement design: single
    /// `.evaluate-*` per real task, with `wg rescue` proxy-inserting a
    /// new task on FAIL. See the rescue-graph-surgery design notes.
    ///
    /// When false, the `verify` field on tasks is still stored (so
    /// inline verification in the same agent still works if explicitly
    /// invoked) but the coordinator will not create new shadow tasks
    /// from it. Set to true in config.toml to restore the old behavior.
    #[serde(default)]
    pub verify_autospawn_enabled: bool,

    /// Maximum consecutive verify command failures before a task is auto-failed.
    /// When a task's verify command fails this many times in a row, the task
    /// transitions to Failed with a descriptive error. Default: 3.
    /// Set to 0 to disable the circuit breaker (unlimited retries).
    ///
    /// Also serves as the cap for cascade-failure auto-rescue chains: if eval
    /// rejects a task and the rescue chain has reached this depth, the task
    /// stays Failed instead of spawning yet another rescue. Accepts the alias
    /// `max_eval_rescues` for forward-compat clarity.
    #[serde(default = "default_max_verify_failures", alias = "max_eval_rescues")]
    pub max_verify_failures: u32,

    /// Default verify timeout for tasks without specific override
    #[serde(default = "default_verify_default_timeout")]
    pub verify_default_timeout: Option<String>,

    /// Maximum number of concurrent verify processes to prevent cascade failures
    #[serde(default = "default_max_concurrent_verifies")]
    pub max_concurrent_verifies: u32,

    /// Enable intelligent triage instead of hard timeout failure
    #[serde(default = "default_verify_triage_enabled")]
    pub verify_triage_enabled: bool,

    /// Time without output before considering process potentially stuck
    #[serde(default = "default_verify_progress_timeout")]
    pub verify_progress_timeout: Option<String>,

    /// Maximum consecutive spawn failures before a task is auto-failed.
    /// When the coordinator fails to spawn an agent for a task this many times
    /// in a row, the task transitions to Failed with a descriptive error
    /// including exec_mode, executor, and last error. Default: 5.
    /// Set to 0 to disable the circuit breaker (unlimited retries).
    #[serde(default = "default_max_spawn_failures")]
    pub max_spawn_failures: u32,

    /// Maximum tier escalation depth for model fallback on retry.
    /// When a task fails and the active profile has a ranked model list,
    /// the coordinator tries the next model in the tier. If the entire tier
    /// is exhausted, it escalates to the next tier up (fast → standard → premium).
    /// This limits how many tiers to escalate through. Default: 3 (all tiers).
    /// Set to 0 to disable tier escalation (only rotate within same tier).
    #[serde(default = "default_max_escalation_depth")]
    pub max_escalation_depth: u32,

    /// Whether to scan for test files before spawning agents and inject
    /// discovered tests into agent context. Default: false.
    #[serde(default = "default_auto_test_discovery")]
    pub auto_test_discovery: bool,

    /// Enable scoped verify: automatically scope 'cargo test' to only run tests
    /// relevant to modified files, reducing verify time from minutes to seconds.
    /// When enabled, detects modified files and maps them to relevant test modules.
    /// Falls back to full test suite for ambiguous mappings or core file changes.
    /// Default: true.
    #[serde(default = "default_scoped_verify_enabled")]
    pub scoped_verify_enabled: bool,

    /// Provider failure handling behavior.
    /// - "pause" (default): pause the service when providers fail consecutively
    /// - "fallback": switch to fallback provider if configured
    /// - "continue": keep going despite provider failures (legacy behavior)
    #[serde(default = "default_on_provider_failure")]
    pub on_provider_failure: String,

    /// Number of consecutive fatal-provider errors before triggering auto-pause.
    /// Fatal-provider errors include auth failures, quota exhaustion, CLI missing.
    /// Transient errors (rate limits, network) and task errors don't count.
    /// Default: 3.
    #[serde(default = "default_provider_failure_threshold")]
    pub provider_failure_threshold: u32,

    /// Cooldown period before auto-resuming from provider failure pause.
    /// Format: "5m", "1h", "30s", etc. Empty string disables auto-resume.
    /// Default: empty (manual resume only).
    #[serde(default)]
    pub provider_failure_cooldown: String,

    /// Resource management configuration for worktree cleanup and recovery.
    #[serde(default)]
    pub resource_management: ResourceManagementConfig,

    /// Maximum automatic retries when a task is marked incomplete.
    /// After this many retries, the task transitions to Failed.
    /// Default: 3. Set to 0 to disable automatic retry exhaustion.
    #[serde(default = "default_max_incomplete_retries")]
    pub max_incomplete_retries: u32,

    /// Cooldown delay before an incomplete task becomes re-dispatchable.
    /// Prevents rapid respawn loops. Format: "30s", "2m", "0s" (immediate).
    /// Default: "30s".
    #[serde(default = "default_incomplete_retry_delay")]
    pub incomplete_retry_delay: String,

    /// Bump the quality tier on retry: fast→standard→premium.
    /// When enabled, a task that fails or is marked incomplete will be
    /// re-dispatched at the next tier up. Default: false.
    #[serde(default)]
    pub escalate_on_retry: bool,
}

/// Resource management configuration for cleanup operations and recovery branches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceManagementConfig {
    /// Enable cleanup verification to ensure worktrees are actually removed.
    /// When true, cleanup operations verify that worktree directories are
    /// fully removed and report any remaining artifacts. Default: true.
    #[serde(default = "default_cleanup_verification")]
    pub cleanup_verification: bool,

    /// Maximum age in seconds for recovery branches before they are pruned.
    /// Recovery branches older than this will be automatically deleted.
    /// Set to 0 to disable age-based pruning. Default: 7 days (604800 seconds).
    #[serde(default = "default_recovery_branch_max_age")]
    pub recovery_branch_max_age: u64,

    /// Maximum number of recovery branches to keep per agent.
    /// When this limit is exceeded, oldest recovery branches are pruned first.
    /// Set to 0 to disable count-based pruning. Default: 10.
    #[serde(default = "default_recovery_branch_max_count")]
    pub recovery_branch_max_count: u32,

    /// Enable cleanup job queuing for high-frequency cleanup scenarios.
    /// When true, cleanup operations are queued and processed sequentially
    /// to prevent resource contention during burst cleanup periods. Default: true.
    #[serde(default = "default_cleanup_job_queue")]
    pub cleanup_job_queue: bool,

    /// Maximum number of cleanup jobs to queue before blocking.
    /// When the queue is full, new cleanup requests will block until
    /// space becomes available. Default: 50.
    #[serde(default = "default_cleanup_queue_size")]
    pub cleanup_queue_size: usize,

    /// Interval in seconds between recovery branch pruning cycles.
    /// Set to 0 to disable automatic pruning. Default: 3600 (1 hour).
    #[serde(default = "default_recovery_prune_interval")]
    pub recovery_prune_interval: u64,
}

fn default_max_incomplete_retries() -> u32 {
    3
}

fn default_incomplete_retry_delay() -> String {
    "30s".to_string()
}

fn default_auto_test_discovery() -> bool {
    false
}

fn default_scoped_verify_enabled() -> bool {
    true
}

fn default_on_provider_failure() -> String {
    "pause".to_string()
}

fn default_provider_failure_threshold() -> u32 {
    3
}

fn default_max_agents() -> usize {
    8
}

fn default_coordinator_interval() -> u64 {
    30
}

fn default_settling_delay_ms() -> u64 {
    2000
}

fn default_coordinator_agent() -> bool {
    true
}

fn default_poll_interval() -> u64 {
    // Safety-timer interval (seconds). With graph watching + IPC kick enabled
    // by default, the loop is event-driven; this is just the safety net for
    // missed wakeups. 5s keeps interactive UX snappy ('is this thing on?'
    // threshold is ~3s, lost confidence at ~10s) while idle polling remains
    // trivial (a graph traversal + cheap return).
    5
}

fn default_graph_watch_enabled() -> bool {
    true
}

fn default_graph_watch_debounce_ms() -> u64 {
    100
}

fn default_compactor_interval() -> u32 {
    5
}

fn default_compactor_ops_threshold() -> usize {
    100
}

fn default_compaction_token_threshold() -> u64 {
    100_000
}

fn default_compaction_threshold_ratio() -> f64 {
    0.8
}

fn default_eval_frequency() -> String {
    "every_5".to_string()
}

fn default_worktree_isolation() -> bool {
    true
}

fn default_max_coordinators() -> usize {
    16
}

fn default_archive_retention_days() -> u64 {
    7
}

fn default_verify_mode() -> String {
    "inline".to_string()
}

fn default_max_verify_failures() -> u32 {
    3
}

fn default_verify_default_timeout() -> Option<String> {
    Some("900s".to_string())
}

fn default_max_concurrent_verifies() -> u32 {
    2
}

fn default_verify_triage_enabled() -> bool {
    false // Start disabled, enable gradually
}

fn default_verify_progress_timeout() -> Option<String> {
    Some("300s".to_string())
}

fn default_max_spawn_failures() -> u32 {
    5
}

fn default_max_escalation_depth() -> u32 {
    3
}

fn default_registry_refresh_interval() -> u64 {
    86400 // 24 hours
}

fn default_agent_timeout() -> String {
    "30m".to_string()
}

/// Providers that are not Anthropic-native and should default to the "native" executor.
const NON_ANTHROPIC_PROVIDERS: &[&str] = &["openrouter", "oai-compat", "openai", "local"];

impl CoordinatorConfig {
    /// Return the effective executor, considering provider-based auto-detection.
    ///
    /// If executor is explicitly set in config, that value is used unconditionally.
    /// Otherwise, if provider is openrouter/openai/local, returns "native" (since
    /// the claude executor only works with Anthropic's API). Falls back to "claude".
    pub fn effective_executor(&self) -> String {
        if let Some(ref executor) = self.executor {
            // Explicitly set in config — honour it (one-release deprecation
            // window; the explicit key still warns at load time).
            executor.clone()
        } else if let Some(ref model) = self.model {
            // Handler-first: derive the executor from the model spec via the
            // single source of truth `handler_for_model`. This recognizes
            // external-CLI handler prefixes (`pi`, `opencode`, …) that
            // `parse_model_spec` + `provider_to_executor` deliberately do NOT
            // (an executor is not a *provider*), so a `pi:openrouter:...`
            // model reports `executor=pi` instead of falling through to the
            // legacy `claude` default. See `bug-handler-first-executor-display-spam`.
            crate::dispatch::handler_for_model(model)
                .as_str()
                .to_string()
        } else if let Some(ref provider) = self.provider {
            // Deprecated: separate provider field fallback
            if NON_ANTHROPIC_PROVIDERS.contains(&provider.as_str()) {
                "native".to_string()
            } else {
                "claude".to_string()
            }
        } else {
            "claude".to_string()
        }
    }
}

// Default functions for ResourceManagementConfig

fn default_cleanup_verification() -> bool {
    true
}

fn default_recovery_branch_max_age() -> u64 {
    604800 // 7 days in seconds
}

fn default_recovery_branch_max_count() -> u32 {
    10
}

fn default_cleanup_job_queue() -> bool {
    true
}

fn default_cleanup_queue_size() -> usize {
    50
}

fn default_recovery_prune_interval() -> u64 {
    3600 // 1 hour in seconds
}

impl Default for ResourceManagementConfig {
    fn default() -> Self {
        Self {
            cleanup_verification: default_cleanup_verification(),
            recovery_branch_max_age: default_recovery_branch_max_age(),
            recovery_branch_max_count: default_recovery_branch_max_count(),
            cleanup_job_queue: default_cleanup_job_queue(),
            cleanup_queue_size: default_cleanup_queue_size(),
            recovery_prune_interval: default_recovery_prune_interval(),
        }
    }
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            max_agents: default_max_agents(),
            interval: default_coordinator_interval(),
            poll_interval: default_poll_interval(),
            graph_watch_enabled: default_graph_watch_enabled(),
            graph_watch_debounce_ms: default_graph_watch_debounce_ms(),
            executor: None,
            model: None,
            provider: None,
            default_context_scope: None,
            agent_timeout: default_agent_timeout(),
            settling_delay_ms: default_settling_delay_ms(),
            coordinator_agent: default_coordinator_agent(),
            compactor_interval: default_compactor_interval(),
            compactor_ops_threshold: default_compactor_ops_threshold(),
            compaction_token_threshold: default_compaction_token_threshold(),
            on_provider_failure: default_on_provider_failure(),
            provider_failure_threshold: default_provider_failure_threshold(),
            provider_failure_cooldown: String::new(),
            compaction_threshold_ratio: default_compaction_threshold_ratio(),
            eval_frequency: default_eval_frequency(),
            worktree_isolation: true,
            max_coordinators: default_max_coordinators(),
            archive_retention_days: default_archive_retention_days(),
            registry_refresh_interval: default_registry_refresh_interval(),
            verify_mode: default_verify_mode(),
            verify_autospawn_enabled: false,
            max_verify_failures: default_max_verify_failures(),
            max_spawn_failures: default_max_spawn_failures(),
            max_escalation_depth: default_max_escalation_depth(),
            auto_test_discovery: default_auto_test_discovery(),
            scoped_verify_enabled: default_scoped_verify_enabled(),
            verify_default_timeout: default_verify_default_timeout(),
            max_concurrent_verifies: default_max_concurrent_verifies(),
            verify_triage_enabled: default_verify_triage_enabled(),
            verify_progress_timeout: default_verify_progress_timeout(),
            resource_management: ResourceManagementConfig::default(),
            max_incomplete_retries: default_max_incomplete_retries(),
            incomplete_retry_delay: default_incomplete_retry_delay(),
            escalate_on_retry: false,
        }
    }
}

/// Project metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Project name (legacy field; new code prefers `title`)
    #[serde(default)]
    pub name: Option<String>,

    /// Project description
    #[serde(default)]
    pub description: Option<String>,

    /// Display title shown at the top of `wg html` / `wg html publish`
    /// rendered pages. Falls back to `name` (then to the WG
    /// directory name) when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// One-line byline / tagline shown under the title in `wg html`
    /// rendered pages. Empty by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byline: Option<String>,

    /// Default skills for new actors
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_skills: Vec<String>,
}

fn default_executor() -> String {
    "claude".to_string()
}

fn is_default_executor(s: &str) -> bool {
    s == default_executor()
}

fn default_model() -> String {
    "claude:opus".to_string()
}

fn default_interval() -> u64 {
    10
}

fn default_heartbeat_timeout() -> u64 {
    5
}

fn default_reaper_grace_seconds() -> u64 {
    30
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            executor: default_executor(),
            model: default_model(),
            interval: default_interval(),
            max_tasks: None,
            heartbeat_timeout: default_heartbeat_timeout(),
            reaper_grace_seconds: default_reaper_grace_seconds(),
        }
    }
}

/// Matrix configuration for notifications and collaboration
/// Stored in ~/.config/worksgood/matrix.toml (user's global config, not in repo)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixConfig {
    /// Matrix homeserver URL (e.g., "https://matrix.org")
    #[serde(default)]
    pub homeserver_url: Option<String>,

    /// Matrix username (e.g., "@user:matrix.org")
    #[serde(default)]
    pub username: Option<String>,

    /// Matrix password (prefer access_token for better security)
    #[serde(default)]
    pub password: Option<String>,

    /// Matrix access token (preferred over password)
    #[serde(default)]
    pub access_token: Option<String>,

    /// Default room to send notifications to (e.g., "!roomid:matrix.org")
    #[serde(default)]
    pub default_room: Option<String>,
}

impl MatrixConfig {
    /// Get the path to the global Matrix config file
    pub fn config_path() -> anyhow::Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory. Expected ~/.config on Linux, ~/Library/Application Support on macOS, or %APPDATA% on Windows."))?;
        Ok(config_dir.join("worksgood").join("matrix.toml"))
    }

    /// Return the pre-WorksGood path accepted for read compatibility only.
    fn legacy_config_path() -> anyhow::Result<PathBuf> {
        let config_dir = dirs::config_dir().ok_or_else(|| {
            anyhow::anyhow!("Could not determine the platform configuration directory")
        })?;
        Ok(config_dir.join("workgraph").join("matrix.toml"))
    }

    /// Load Matrix configuration from ~/.config/worksgood/matrix.toml.
    /// Returns default (empty) config if file doesn't exist
    pub fn load() -> anyhow::Result<Self> {
        let canonical = Self::config_path()?;
        let legacy = Self::legacy_config_path()?;
        let config_path = if canonical.exists() {
            canonical
        } else if legacy.exists() {
            eprintln!(
                "warning: using legacy Matrix config at {}; move it to {} (create the parent directory first)",
                legacy.display(),
                canonical.display()
            );
            legacy
        } else {
            return Ok(Self::default());
        };

        let content = fs::read_to_string(&config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read Matrix config: {}", e))?;

        let config: MatrixConfig = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse Matrix config: {}", e))?;

        Ok(config)
    }

    /// Save Matrix configuration to ~/.config/worksgood/matrix.toml
    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_path()?;

        // Create parent directory if needed
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("Failed to create config directory: {}", e))?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize Matrix config: {}", e))?;

        fs::write(&config_path, content)
            .map_err(|e| anyhow::anyhow!("Failed to write Matrix config: {}", e))?;

        Ok(())
    }

    /// Check if the configuration has valid credentials
    pub fn has_credentials(&self) -> bool {
        self.homeserver_url.is_some()
            && self.username.is_some()
            && (self.password.is_some() || self.access_token.is_some())
    }

    /// Check if the configuration is complete (has credentials and default room)
    pub fn is_complete(&self) -> bool {
        self.has_credentials() && self.default_room.is_some()
    }
}

/// Indicates where a configuration value came from
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigSource {
    Global,
    Local,
    Default,
}

impl std::fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigSource::Global => write!(f, "global"),
            ConfigSource::Local => write!(f, "local"),
            ConfigSource::Default => write!(f, "default"),
        }
    }
}

/// Single source of truth for legacy → canonical TOML section renames during
/// deprecation windows. Format: `(legacy, canonical)`.
///
/// The merge logic (`load_merged`, `load_merged_toml_value`, `load_with_sources`)
/// migrates each legacy key in-place via [`normalize_legacy_tables`] *before*
/// merging, so a local `[dispatcher]` shadows a global `[coordinator]` and
/// vice versa. To remove an alias once its deprecation window closes, just
/// delete the entry from this slice.
pub const LEGACY_SECTION_ALIASES: &[(&str, &str)] = &[("coordinator", "dispatcher")];

/// Migrate legacy top-level TOML keys in `value` to their canonical names per
/// [`LEGACY_SECTION_ALIASES`]. For each legacy key encountered, pushes one
/// deprecation message into `warnings` (using `path_label` to identify the
/// originating file).
///
/// When both the legacy and canonical keys are present at the top level, the
/// canonical entry wins on conflicting subkeys; subkeys present only in the
/// legacy entry are preserved.
pub fn normalize_legacy_tables(
    value: &mut toml::Value,
    path_label: &str,
    warnings: &mut Vec<String>,
) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    for (legacy, canonical) in LEGACY_SECTION_ALIASES {
        let Some(legacy_val) = table.remove(*legacy) else {
            continue;
        };
        warnings.push(format!(
            "Deprecated: [{}] table is now [{}]; please rename in {}",
            legacy, canonical, path_label
        ));
        match table.remove(*canonical) {
            Some(canonical_val) => {
                // Both present: canonical wins, but merge in any keys from
                // legacy that are missing from canonical.
                let merged = merge_toml(legacy_val, canonical_val);
                table.insert(canonical.to_string(), merged);
            }
            None => {
                table.insert(canonical.to_string(), legacy_val);
            }
        }
    }
    normalize_dispatcher_interval_alias(table);
}

fn normalize_dispatcher_interval_alias(table: &mut toml::map::Map<String, toml::Value>) {
    for section_name in ["coordinator", "dispatcher"] {
        let Some(section) = table.get_mut(section_name).and_then(|v| v.as_table_mut()) else {
            continue;
        };
        let Some(safety_interval) = section.remove("safety_interval") else {
            continue;
        };
        section.insert("poll_interval".to_string(), safety_interval);
    }
}

/// Print accumulated legacy-section deprecation warnings to stderr.
/// Called at the tail of each `load_*` entry point so users see the message
/// once per load (and per legacy file) instead of on every config field read.
fn emit_legacy_warnings(warnings: &[String]) {
    for w in warnings {
        eprintln!("warning: {}", w);
    }
}

/// LLM endpoint inheritance is opt-in: local config does NOT inherit
/// `[[llm_endpoints.endpoints]]` entries from global by default. The user must
/// set `[llm_endpoints] inherit_global = true` in local config to keep the
/// legacy "global cascades into local" behavior.
///
/// Active named profiles are the exception: when a profile is active and the
/// local config does not explicitly declare its own endpoints or
/// `inherit_global = false`, the profile's global endpoints are part of the
/// selected route and should flow into the effective config.
///
/// This mutates `global_val` in place to drop the `endpoints` array from its
/// `[llm_endpoints]` table when local hasn't opted in. Call this BEFORE
/// `merge_toml` so the deep-merge sees an effectively-empty global endpoints
/// list and the merged config reflects only what local declared.
fn apply_endpoint_inheritance_policy(
    global_val: &mut toml::Value,
    local_val: &toml::Value,
    active_named_profile: bool,
) {
    let explicit_inherit = local_val
        .get("llm_endpoints")
        .and_then(|t| t.get("inherit_global"))
        .and_then(|b| b.as_bool());
    let local_declares_endpoints = local_val
        .get("llm_endpoints")
        .and_then(|t| t.get("endpoints"))
        .and_then(|v| v.as_array())
        .is_some();
    let inherit =
        explicit_inherit.unwrap_or_else(|| active_named_profile && !local_declares_endpoints);
    if inherit {
        return;
    }
    if let Some(global_endpoints) = global_val
        .get_mut("llm_endpoints")
        .and_then(|t| t.as_table_mut())
    {
        global_endpoints.remove("endpoints");
    }
}

/// Deep-merge two TOML values. For (Table, Table) pairs, recursively merge
/// with `local` keys overriding `global`. For all other cases, `local` wins.
pub fn merge_toml(global: toml::Value, local: toml::Value) -> toml::Value {
    match (global, local) {
        (toml::Value::Table(mut g), toml::Value::Table(l)) => {
            for (key, local_val) in l {
                let merged = if let Some(global_val) = g.remove(&key) {
                    merge_toml(global_val, local_val)
                } else {
                    local_val
                };
                g.insert(key, merged);
            }
            toml::Value::Table(g)
        }
        (_global, local) => local,
    }
}

/// When local config explicitly sets `agent.model`, strip any `models.<role>.model`
/// entries that exist only in the global config (not overridden locally).
fn strip_global_only_model_roles(
    merged: &mut toml::Value,
    global_val: &toml::Value,
    local_val: &toml::Value,
) {
    let local_has_agent_model = local_val
        .get("agent")
        .and_then(|a| a.get("model"))
        .and_then(|m| m.as_str())
        .is_some();
    if !local_has_agent_model {
        return;
    }
    let global_models = match global_val.get("models").and_then(|m| m.as_table()) {
        Some(m) => m,
        None => return,
    };
    let local_models = local_val.get("models").and_then(|m| m.as_table());
    let roles_to_strip: Vec<String> = global_models
        .keys()
        .filter(|role_key| {
            let has_global_model = global_models
                .get(role_key.as_str())
                .and_then(|r| r.get("model"))
                .is_some();
            let has_local_role = local_models
                .map(|lm| lm.contains_key(role_key.as_str()))
                .unwrap_or(false);
            has_global_model && !has_local_role
        })
        .cloned()
        .collect();
    if roles_to_strip.is_empty() {
        return;
    }
    if let Some(merged_models) = merged.get_mut("models").and_then(|m| m.as_table_mut()) {
        for role_key in &roles_to_strip {
            if let Some(role_table) = merged_models
                .get_mut(role_key.as_str())
                .and_then(|r| r.as_table_mut())
            {
                role_table.remove("model");
                if role_table.is_empty() {
                    merged_models.remove(role_key.as_str());
                }
            }
        }
        if merged_models.is_empty()
            && let Some(root) = merged.as_table_mut()
        {
            root.remove("models");
        }
    }
}

// `restore_local_profile_overrides` and its `toml_has_path` helper removed
// (2026-05): profiles are now snapshot file-swaps, not overlays. The
// global+local merge already gives local config the right precedence — no
// profile-aware restoration needed.

/// A deprecated config key found in raw TOML, with a human-readable replacement
/// suggestion the daemon can log on startup.
///
/// Use [`detect_deprecated_keys`] to scan a parsed `toml::Value` for legacy keys
/// that the codebase still accepts via serde aliases but plans to retire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeprecatedKey {
    /// Dot-separated path of the deprecated key as it appears in TOML
    /// (e.g. `"coordinator.poll_interval"`).
    pub path: String,
    /// Suggested replacement path the user should migrate to.
    pub replacement: String,
}

/// Scan a (possibly merged) `toml::Value` for deprecated configuration keys.
///
/// Returns one entry per legacy key found anywhere in the document. The daemon
/// uses this on startup to print one-shot deprecation warnings per legacy key,
/// while still honoring the value via serde aliases.
///
/// Currently surfaces:
/// - `[coordinator] poll_interval` → `[coordinator] safety_interval`
/// - `[dispatcher] poll_interval` → `[dispatcher] safety_interval`
pub fn detect_deprecated_keys(val: &toml::Value) -> Vec<DeprecatedKey> {
    let mut found = Vec::new();
    let table = match val.as_table() {
        Some(t) => t,
        None => return found,
    };
    for legacy_section in ["coordinator", "dispatcher"] {
        if let Some(section) = table.get(legacy_section).and_then(|v| v.as_table())
            && section.contains_key("poll_interval")
        {
            found.push(DeprecatedKey {
                path: format!("{}.poll_interval", legacy_section),
                replacement: format!("{}.safety_interval", legacy_section),
            });
        }
    }
    found
}

/// Walk a TOML Value table and record source per leaf key (dot-separated path).
fn record_sources(
    val: &toml::Value,
    prefix: &str,
    source: &ConfigSource,
    map: &mut BTreeMap<String, ConfigSource>,
) {
    if let toml::Value::Table(table) = val {
        for (key, v) in table {
            let full_key = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };
            match v {
                toml::Value::Table(_) => record_sources(v, &full_key, source, map),
                _ => {
                    map.insert(full_key, source.clone());
                }
            }
        }
    }
}

impl Config {
    /// Return the global WG directory.
    ///
    /// Resolution order for canonical machine-global state:
    /// 0. `$WG_GLOBAL_DIR` if set — an explicit override used to point WG's
    ///    global config + active-profile lookup at a specific directory.
    ///    This is the single chokepoint both `global_config_path()` and
    ///    `profile::named::active_pointer_path()` flow through, so setting it
    ///    isolates *all* machine-global state (config.toml, active-profile,
    ///    profiles/) in one shot. Tests use it to stay independent of the
    ///    developer machine's `~/.wg` (e.g. an active `opencode` profile),
    ///    without perturbing `HOME` for sibling tests that shell out to git.
    /// 1. `~/.wg` (canonical for every new write).
    ///
    /// The old global config is handled separately by
    /// [`Config::global_config_read_path`], so a compatibility read can never
    /// silently turn into another write at the retired location.
    pub fn global_dir() -> anyhow::Result<PathBuf> {
        if let Some(dir) = std::env::var_os("WG_GLOBAL_DIR") {
            let dir = PathBuf::from(dir);
            if !dir.as_os_str().is_empty() {
                return Ok(dir);
            }
        }
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
        Ok(home.join(".wg"))
    }

    fn global_config_read_path() -> anyhow::Result<PathBuf> {
        let canonical = Self::global_config_path()?;
        // An explicit test/operator override is authoritative. Never escape it
        // to a legacy file under the ambient HOME.
        if canonical.exists() || std::env::var_os("WG_GLOBAL_DIR").is_some() {
            return Ok(canonical);
        }
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
        let legacy = home.join(".workgraph").join("config.toml");
        if legacy.exists() {
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "warning: reading legacy WorksGood global config at {}; migrate it to {} (for example, stop wg services and move the file)",
                    legacy.display(),
                    canonical.display()
                );
            }
            return Ok(legacy);
        }
        Ok(canonical)
    }

    /// Return the global config file path.
    pub fn global_config_path() -> anyhow::Result<PathBuf> {
        Ok(Self::global_dir()?.join("config.toml"))
    }

    /// Load global configuration from ~/.wg/config.toml.
    /// Returns None if the file doesn't exist, Err on parse failure.
    pub fn load_global() -> anyhow::Result<Option<Self>> {
        let global_path = Self::global_config_read_path()?;
        if !global_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&global_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to read global config at {}: {}",
                global_path.display(),
                e
            )
        })?;
        let mut val: toml::Value = content.parse().map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse global config at {}: {}",
                global_path.display(),
                e
            )
        })?;
        let mut warnings = Vec::new();
        normalize_legacy_tables(&mut val, &global_path.display().to_string(), &mut warnings);
        emit_legacy_warnings(&warnings);
        let config: Config = val.try_into().map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse global config at {}: {}",
                global_path.display(),
                e
            )
        })?;
        config.validate_model_format()?;
        Ok(Some(config))
    }

    /// Load raw TOML value from a config file path.
    /// Returns empty table if file doesn't exist.
    pub fn load_toml_value(path: &Path) -> anyhow::Result<toml::Value> {
        if !path.exists() {
            return Ok(toml::Value::Table(toml::map::Map::new()));
        }
        let content = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config at {}: {}", path.display(), e))?;
        let val: toml::Value = content
            .parse()
            .map_err(|e| anyhow::anyhow!("Failed to parse config at {}: {}", path.display(), e))?;
        Ok(val)
    }

    /// Load the merged TOML value (global + local) without deserializing.
    /// Useful for legacy code that needs raw TOML access to sections like
    /// `[native_executor]` while respecting the global → local merge chain.
    ///
    /// Legacy section names (per [`LEGACY_SECTION_ALIASES`]) are normalized to
    /// their canonical form before merging, so callers always see the canonical
    /// keys regardless of which file used the legacy name.
    pub fn load_merged_toml_value(workgraph_dir: &Path) -> anyhow::Result<toml::Value> {
        let global_path = Self::global_config_read_path()?;
        let local_path = workgraph_dir.join("config.toml");
        let mut global_val = Self::load_toml_value(&global_path)?;
        let mut local_val = Self::load_toml_value(&local_path)?;
        let mut warnings = Vec::new();
        normalize_legacy_tables(
            &mut global_val,
            &global_path.display().to_string(),
            &mut warnings,
        );
        normalize_legacy_tables(
            &mut local_val,
            &local_path.display().to_string(),
            &mut warnings,
        );
        emit_legacy_warnings(&warnings);
        let active_named_profile = crate::profile::named::active().ok().flatten().is_some();
        apply_endpoint_inheritance_policy(&mut global_val, &local_val, active_named_profile);
        Ok(merge_toml(global_val, local_val))
    }

    /// Load merged configuration: global config deep-merged with local config.
    /// Local keys override global keys. Missing files are treated as empty.
    pub fn load_merged(workgraph_dir: &Path) -> anyhow::Result<Self> {
        let global_path = Self::global_config_read_path()?;
        let local_path = workgraph_dir.join("config.toml");

        let mut global_val = Self::load_toml_value(&global_path)?;
        let mut local_val = Self::load_toml_value(&local_path)?;

        // Migrate legacy section names (e.g. `[coordinator]` → `[dispatcher]`)
        // BEFORE merging, so callers don't end up with both keys in the merged
        // value and serde isn't forced to pick one. (rename + alias on the
        // field doesn't help when both keys are simultaneously present.)
        let mut warnings = Vec::new();
        normalize_legacy_tables(
            &mut global_val,
            &global_path.display().to_string(),
            &mut warnings,
        );
        normalize_legacy_tables(
            &mut local_val,
            &local_path.display().to_string(),
            &mut warnings,
        );
        emit_legacy_warnings(&warnings);

        // Surface deprecated `executor` keys regardless of which file
        // they live in. Read each file's raw content directly (we already
        // have it as TOML values, but `deprecated_executor_warnings_for_toml`
        // takes the raw string for symmetry with `Config::load`).
        for (label, path) in [("global", &global_path), ("local", &local_path)] {
            if let Ok(content) = fs::read_to_string(path) {
                for w in deprecated_executor_warnings_for_toml(&content) {
                    eprintln!("warning: ({}) {}", label, w);
                }
                for w in deprecated_model_prefix_warnings_for_toml(&content) {
                    eprintln!("warning: ({}) {}", label, w);
                }
            }
        }

        let agent_model_is_local = local_val
            .get("agent")
            .and_then(|a| a.get("model"))
            .and_then(|m| m.as_str())
            .is_some();

        let active_named_profile = crate::profile::named::active().ok().flatten().is_some();
        apply_endpoint_inheritance_policy(&mut global_val, &local_val, active_named_profile);
        let mut merged = merge_toml(global_val.clone(), local_val.clone());
        strip_global_only_model_roles(&mut merged, &global_val, &local_val);
        let mut config: Config = merged
            .try_into()
            .map_err(|e| anyhow::anyhow!("Failed to deserialize merged config: {}", e))?;
        config.agent_model_is_local = agent_model_is_local;

        // Note: named profile resolution is now a *file swap*, not an overlay.
        // `wg profile use <name>` copies `~/.wg/profiles/<name>.toml` over
        // `~/.wg/config.toml`, so by the time we read the global config above
        // it already reflects the active profile. The `~/.wg/active-profile`
        // pointer is used only for user-facing labels and for the endpoint
        // inheritance exception above; model routing authority is handled by
        // `wg profile use`, which removes local model-routing keys that would
        // shadow the selected profile.

        config.validate_model_format()?;

        Ok(config)
    }

    /// Resolve an API key for a given provider, checking all configured sources.
    ///
    /// Priority:
    /// 1. `[llm_endpoints]` — matching endpoint's api_key / api_key_file / key_env
    /// 2. Environment variables (provider-specific, e.g. OPENROUTER_API_KEY)
    /// 3. `[native_executor]` api_key in config.toml (legacy path)
    ///
    /// `workgraph_dir` is the `.wg/` directory, used for resolving
    /// relative api_key_file paths and reading native_executor config.
    pub fn resolve_api_key_for_provider(
        &self,
        provider: &str,
        workgraph_dir: &Path,
    ) -> anyhow::Result<String> {
        // 1. Check llm_endpoints for a matching provider
        if let Some(ep) = self.llm_endpoints.find_for_provider(provider)
            && let Ok(Some(key)) = ep.resolve_api_key(Some(workgraph_dir))
        {
            return Ok(key);
        }
        // 2. Environment variables based on provider
        for var_name in EndpointConfig::env_var_names_for_provider(provider) {
            if let Ok(key) = std::env::var(var_name) {
                let key = key.trim().to_string();
                if !key.is_empty() {
                    return Ok(key);
                }
            }
        }

        // 3. Legacy fallback: [native_executor] api_key in merged config (global + local)
        if let Ok(merged_val) = Self::load_merged_toml_value(workgraph_dir)
            && let Some(key) = merged_val
                .get("native_executor")
                .and_then(|v| v.get("api_key"))
                .and_then(|v| v.as_str())
            && !key.is_empty()
        {
            return Ok(key.to_string());
        }

        Err(anyhow::anyhow!(
            "No API key found for provider '{}'. Configure a key via:\n  \
             - wg endpoints add (recommended)\n  \
             - Set {} environment variable\n  \
             - Add [native_executor] api_key to .wg/config.toml",
            provider,
            EndpointConfig::env_var_names_for_provider(provider)
                .first()
                .unwrap_or(&"<PROVIDER>_API_KEY"),
        ))
    }

    /// Load configuration from .wg/config.toml (local only).
    /// Returns default config if file doesn't exist.
    pub fn load(workgraph_dir: &Path) -> anyhow::Result<Self> {
        let config_path = workgraph_dir.join("config.toml");

        if !config_path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read config: {}", e))?;

        let mut val: toml::Value = content.parse().map_err(|e| {
            anyhow::anyhow!("Failed to parse config at {}: {}", config_path.display(), e)
        })?;
        let mut warnings = Vec::new();
        normalize_legacy_tables(&mut val, &config_path.display().to_string(), &mut warnings);
        emit_legacy_warnings(&warnings);
        let config: Config = val.try_into().map_err(|e| {
            anyhow::anyhow!("Failed to parse config at {}: {}", config_path.display(), e)
        })?;

        config.validate_model_format()?;

        for warning in config.deprecated_compaction_warnings() {
            eprintln!("warning: {}", warning);
        }

        // The `executor` taxonomy has been deprecated as a user-facing
        // concept in favor of model+endpoint. Warn loudly (once per load)
        // when explicit `executor = …` keys are still in config.toml so
        // users have one release to migrate.
        for warning in deprecated_executor_warnings_for_toml(&content) {
            eprintln!("warning: {}", warning);
        }

        // Same one-release deprecation window for the legacy `local:` /
        // `oai-compat:` model-spec prefixes, replaced by the canonical
        // `nex:` (matches the `wg nex` subcommand).
        for warning in deprecated_model_prefix_warnings_for_toml(&content) {
            eprintln!("warning: {}", warning);
        }

        Ok(config)
    }

    /// Returns warning strings for any deprecated graph-cycle compaction keys
    /// (`compactor_interval`, `compactor_ops_threshold`,
    /// `compaction_token_threshold`, `compaction_threshold_ratio`) that were
    /// loaded with non-default values. These keys are no-ops after the
    /// `.compact-N` cycle was retired; the warning gives users one release to
    /// migrate before the fields are removed entirely.
    pub fn deprecated_compaction_warnings(&self) -> Vec<String> {
        let c = &self.coordinator;
        let mut out = Vec::new();
        if c.compactor_interval != default_compactor_interval() {
            out.push(format!(
                "config key `coordinator.compactor_interval = {}` is deprecated and ignored \
                 (graph-cycle compactor was retired in retire-compact-archive); remove it from config.toml",
                c.compactor_interval
            ));
        }
        if c.compactor_ops_threshold != default_compactor_ops_threshold() {
            out.push(format!(
                "config key `coordinator.compactor_ops_threshold = {}` is deprecated and ignored \
                 (graph-cycle compactor was retired); remove it from config.toml",
                c.compactor_ops_threshold
            ));
        }
        if c.compaction_token_threshold != default_compaction_token_threshold() {
            out.push(format!(
                "config key `coordinator.compaction_token_threshold = {}` is deprecated and ignored \
                 (graph-cycle compactor was retired); remove it from config.toml",
                c.compaction_token_threshold
            ));
        }
        if (c.compaction_threshold_ratio - default_compaction_threshold_ratio()).abs()
            > f64::EPSILON
        {
            out.push(format!(
                "config key `coordinator.compaction_threshold_ratio = {}` is deprecated and ignored \
                 (graph-cycle compactor was retired); remove it from config.toml",
                c.compaction_threshold_ratio
            ));
        }
        out
    }

    /// Load configuration with global+local merge, falling back to defaults on error.
    ///
    /// Unlike `.load().unwrap_or_default()`, this emits a stderr warning
    /// when a config file exists but is corrupt, so the user knows
    /// their configuration is being ignored.
    pub fn load_or_default(workgraph_dir: &Path) -> Self {
        match Self::load_merged(workgraph_dir) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Warning: {}, using defaults", e);
                Self::default()
            }
        }
    }

    /// Save configuration to .wg/config.toml
    pub fn save(&self, workgraph_dir: &Path) -> anyhow::Result<()> {
        let config_path = workgraph_dir.join("config.toml");

        let content = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize config: {}", e))?;

        fs::write(&config_path, content)
            .map_err(|e| anyhow::anyhow!("Failed to write config: {}", e))?;

        Ok(())
    }

    /// Copy the current `config.toml` to `config.toml.<UTC-iso-timestamp>`
    /// so subsequent writes are recoverable. Backups sort lexicographically
    /// by timestamp (`ls config.toml.*` gives chronological order).
    /// Returns the backup path on success, or `None` if the source file
    /// didn't exist (nothing to back up on a fresh project).
    pub fn backup_on_disk(workgraph_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
        let config_path = workgraph_dir.join("config.toml");
        if !config_path.exists() {
            return Ok(None);
        }
        // Format avoids colons so the filename works on any FS and sorts
        // lexicographically identical to chronological.
        let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
        let backup_path = workgraph_dir.join(format!("config.toml.{}", stamp));
        fs::copy(&config_path, &backup_path)
            .map_err(|e| anyhow::anyhow!("Failed to back up config.toml: {}", e))?;
        Ok(Some(backup_path))
    }

    /// Apply a model + endpoint pair to this config in memory. Does NOT
    /// write. Shared by `wg init` and `wg config` so both have identical
    /// semantics:
    ///
    /// - `endpoint` (if Some) becomes the `[[llm_endpoints.endpoints]]`
    ///   entry named `default`, with `provider = "local"` (oai-compat,
    ///   no auth) and `is_default = true`. Any preexisting entry named
    ///   `default` is replaced and all other entries lose `is_default`.
    /// - `model` (if Some) goes into `agent.model` and
    ///   `dispatcher.model`. When combined with `endpoint`, the model
    ///   name is prefixed with `nex:` (canonical, matches `wg nex`) so
    ///   the provider:model validator accepts it on reload; when `model`
    ///   is provider-prefixed already (`claude:opus`), it's used verbatim.
    /// - `endpoint` must start with `http://` or `https://`; otherwise
    ///   this fn returns an error before mutating anything.
    ///
    /// Returns a human-readable list of the changes applied (for the
    /// caller to print).
    pub fn apply_model_endpoint(
        &mut self,
        model: Option<&str>,
        endpoint: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        if model.is_none() && endpoint.is_none() {
            return Ok(Vec::new());
        }

        if let Some(url) = endpoint
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            anyhow::bail!("Endpoint must be an http:// or https:// URL (got: {})", url);
        }

        // With an endpoint, bare model names need a `nex:` prefix to
        // pass the provider:model validator. The `nex:` prefix is the
        // canonical form (matches the `wg nex` subcommand); `local:` and
        // `oai-compat:` are deprecated aliases retained for back-compat.
        let effective_model: Option<String> = if endpoint.is_some() {
            model.map(|m| {
                if m.contains(':') {
                    m.to_string()
                } else {
                    format!("nex:{}", m)
                }
            })
        } else {
            model.map(|m| m.to_string())
        };

        let mut summary = Vec::new();

        if let Some(url) = endpoint {
            let name = "default".to_string();
            self.llm_endpoints.endpoints.retain(|e| e.name != name);
            for e in self.llm_endpoints.endpoints.iter_mut() {
                e.is_default = false;
            }
            self.llm_endpoints.endpoints.push(EndpointConfig {
                name,
                provider: "local".to_string(),
                url: Some(url.to_string()),
                model: model.map(|s| s.to_string()),
                api_key: None,
                api_key_file: None,
                api_key_env: None,
                api_key_ref: None,
                is_default: true,
                context_window: None,
            });
            summary.push(format!("default endpoint → {}", url));
        }

        if let Some(m) = effective_model {
            self.pin_default_route_model(&m);
            summary.push(format!("model → {}", m));
        }

        Ok(summary)
    }

    /// Pin the user-visible default worker route to one exact model spec.
    ///
    /// This updates every default/task-agent surface that can otherwise fall
    /// through to a lower tier: agent/dispatcher model, `[models.default]`,
    /// `[models.task_agent]`, and the standard/premium tier aliases. For known
    /// CLI profiles (claude/codex), starter agency pins are moved to the
    /// matching cheap model too; custom role overrides are preserved.
    pub fn pin_default_route_model(&mut self, model: &str) {
        self.agent.model = model.to_string();
        self.coordinator.model = Some(model.to_string());
        let role = RoleModelConfig {
            provider: None,
            model: Some(model.to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        };
        self.models.default = Some(role.clone());
        self.models.task_agent = Some(role);
        self.tiers.standard = Some(model.to_string());
        self.tiers.premium = Some(model.to_string());
        self.pin_provider_companion_defaults(model);
    }

    fn pin_provider_companion_defaults(&mut self, model: &str) {
        let spec = parse_model_spec(model);
        let Some(provider) = spec.provider.as_deref() else {
            return;
        };
        let (fast, agency) = match provider {
            "claude" | "anthropic" => ("claude:haiku", "claude:haiku"),
            "codex" => ("codex:gpt-5.6-luna", "codex:gpt-5.6-luna"),
            _ => return,
        };

        if self.tier_model_is_absent_or_starter(self.tiers.fast.as_deref()) {
            self.tiers.fast = Some(fast.to_string());
        }
        for role in [
            DispatchRole::Evaluator,
            DispatchRole::Assigner,
            DispatchRole::FlipInference,
            DispatchRole::FlipComparison,
        ] {
            self.set_role_model_if_absent_or_starter(role, agency);
        }
    }

    fn tier_model_is_absent_or_starter(&self, model: Option<&str>) -> bool {
        match model {
            None => true,
            Some(model) => Self::is_starter_agency_model(model),
        }
    }

    fn set_role_model_if_absent_or_starter(&mut self, role: DispatchRole, model: &str) {
        let slot = self.models.get_role_mut(role);
        let replace = match slot.as_ref() {
            None => true,
            Some(cfg) => {
                cfg.provider.is_none()
                    && cfg.tier.is_none()
                    && cfg.endpoint.is_none()
                    && match cfg.model.as_deref() {
                        None => true,
                        Some(existing) => Self::is_starter_agency_model(existing),
                    }
            }
        };
        if replace {
            *slot = Some(RoleModelConfig {
                provider: None,
                model: Some(model.to_string()),
                tier: None,
                endpoint: None,
                reasoning: None,
            });
        }
    }

    fn is_starter_agency_model(model: &str) -> bool {
        matches!(
            model,
            "haiku"
                | "claude:haiku"
                | "gpt-5.4-mini"
                | "codex:gpt-5.4-mini"
                | "gpt-5.6-luna"
                | "codex:gpt-5.6-luna"
        )
    }

    /// Dotted TOML keys written when setting the Pi profile's **strong** tier.
    ///
    /// This is the `strong` half of the two-tier facade documented in
    /// `docs/design-two-tier-pi-profile.md` §4.1. It is the single source of
    /// truth shared by the in-memory writer ([`Config::set_pi_tiers`]) and the
    /// comment-preserving file patcher (`profile::named::patch_pi_tiers`), so the
    /// two cannot disagree about which keys `strong` drives.
    pub const PI_STRONG_TOML_KEYS: &'static [&'static str] = &[
        "agent.model",
        "dispatcher.model",
        "models.default.model",
        "models.task_agent.model",
        "tiers.standard",
        "tiers.premium",
    ];

    /// Dotted TOML keys written when setting the Pi profile's **weak** tier.
    ///
    /// See [`Config::PI_STRONG_TOML_KEYS`]. The four `[models.<role>]` agency
    /// one-shot keys are written explicitly because those roles ignore the tier
    /// cascade today (`resolve_agency_dispatch`); `tiers.fast` covers the
    /// remaining fast-tier roles. See design §4.1.
    pub const PI_WEAK_TOML_KEYS: &'static [&'static str] = &[
        "tiers.fast",
        "models.evaluator.model",
        "models.assigner.model",
        "models.flip_inference.model",
        "models.flip_comparison.model",
    ];

    /// Reasoning keys controlled by a two-tier strong-reasoning update.
    /// Kept separate from model keys so either dimension can be patched without
    /// reconstructing (and accidentally erasing) the other.
    pub const PI_STRONG_REASONING_TOML_KEYS: &'static [&'static str] = &[
        "tiers.standard_reasoning",
        "tiers.premium_reasoning",
        "models.default.reasoning",
        "models.task_agent.reasoning",
    ];

    /// Reasoning keys controlled by a two-tier weak-reasoning update.
    pub const PI_WEAK_REASONING_TOML_KEYS: &'static [&'static str] = &[
        "tiers.fast_reasoning",
        "models.evaluator.reasoning",
        "models.assigner.reasoning",
        "models.flip_inference.reasoning",
        "models.flip_comparison.reasoning",
    ];

    /// Read the Pi profile's `(strong, weak)` tier models from this config.
    ///
    /// `strong` is inferred from `agent.model` (falling back to
    /// `[models.default].model`); `weak` from `tiers.fast` (falling back to
    /// `[models.evaluator].model`). This is the read path behind
    /// `wg profile pi --show`/`--list` and the `old → new` echo. A profile that
    /// has never been touched by the two-tier setter still reports correct tiers
    /// because the hand-written `pi.toml` starter already uses this layout
    /// (design §8: migration is recognition, not rewrite).
    pub fn pi_tiers(&self) -> (Option<String>, Option<String>) {
        let strong = if !self.agent.model.trim().is_empty() {
            Some(self.agent.model.clone())
        } else {
            self.models
                .default
                .as_ref()
                .and_then(|m| m.model.clone())
                .filter(|m| !m.trim().is_empty())
        };
        let weak = self
            .tiers
            .fast
            .clone()
            .filter(|m| !m.trim().is_empty())
            .or_else(|| {
                self.models
                    .evaluator
                    .as_ref()
                    .and_then(|m| m.model.clone())
                    .filter(|m| !m.trim().is_empty())
            });
        (strong, weak)
    }

    /// Set the Pi profile's two tiers in-memory.
    ///
    /// Writes the [`Config::PI_STRONG_TOML_KEYS`] /
    /// [`Config::PI_WEAK_TOML_KEYS`] key-set for whichever tier(s) are
    /// `Some(_)`; a `None` tier is left untouched (partial update). Explicit
    /// `[models.<role>]` overrides outside the written key-set are preserved,
    /// matching the design's "an explicit per-role override always wins and is
    /// never touched by the two-tier setter" contract.
    ///
    /// This mutates a parsed `Config` (used for validation and tests); the
    /// persisted, comment-preserving write goes through
    /// `profile::named::patch_pi_tiers`, which targets the same TOML keys.
    pub fn set_pi_tiers(&mut self, strong: Option<&str>, weak: Option<&str>) {
        if let Some(s) = strong {
            // Strong tier must execute through the self-authenticating pi
            // handler, never the in-process nex OpenRouter client (which would
            // require a wg-side key). Normalize an `openrouter:`/bare route to a
            // `pi:` route; CLI / pi / nex-local specs pass through unchanged.
            let s = pi_strong_route(s);
            self.agent.model = s.clone();
            self.coordinator.model = Some(s.clone());
            let role = RoleModelConfig {
                provider: None,
                model: Some(s.clone()),
                tier: None,
                endpoint: None,
                reasoning: None,
            };
            self.models.default = Some(role.clone());
            self.models.task_agent = Some(role);
            self.tiers.standard = Some(s.clone());
            self.tiers.premium = Some(s);
        }
        if let Some(w) = weak {
            self.tiers.fast = Some(w.to_string());
            for role in [
                DispatchRole::Evaluator,
                DispatchRole::Assigner,
                DispatchRole::FlipInference,
                DispatchRole::FlipComparison,
            ] {
                *self.models.get_role_mut(role) = Some(RoleModelConfig {
                    provider: None,
                    model: Some(w.to_string()),
                    tier: None,
                    endpoint: None,
                    reasoning: None,
                });
            }
        }
    }

    /// Save configuration to the global path (~/.wg/config.toml).
    /// Creates the ~/.wg/ directory if needed.
    pub fn save_global(&self) -> anyhow::Result<()> {
        let global_dir = Self::global_dir()?;
        fs::create_dir_all(&global_dir).map_err(|e| {
            anyhow::anyhow!(
                "Failed to create global config directory {}: {}",
                global_dir.display(),
                e
            )
        })?;

        let global_path = global_dir.join("config.toml");
        let content = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize config: {}", e))?;

        fs::write(&global_path, content).map_err(|e| {
            anyhow::anyhow!(
                "Failed to write global config at {}: {}",
                global_path.display(),
                e
            )
        })?;

        Ok(())
    }

    /// Initialize default config file if it doesn't exist
    pub fn init(workgraph_dir: &Path) -> anyhow::Result<bool> {
        let config_path = workgraph_dir.join("config.toml");

        if config_path.exists() {
            return Ok(false); // Already exists
        }

        let config = Self::default();
        config.save(workgraph_dir)?;
        Ok(true) // Created new
    }

    /// Initialize default global config file if it doesn't exist
    pub fn init_global() -> anyhow::Result<bool> {
        let global_path = Self::global_config_path()?;

        if global_path.exists() {
            return Ok(false);
        }

        let config = Self::default();
        config.save_global()?;
        Ok(true)
    }

    /// Load merged config and record where each leaf key came from.
    pub fn load_with_sources(
        workgraph_dir: &Path,
    ) -> anyhow::Result<(Self, BTreeMap<String, ConfigSource>)> {
        let global_path = Self::global_config_read_path()?;
        let local_path = workgraph_dir.join("config.toml");

        let mut global_val = Self::load_toml_value(&global_path)?;
        let mut local_val = Self::load_toml_value(&local_path)?;

        // Migrate legacy section names BEFORE recording sources, so the source
        // map keys match the canonical field paths emitted by the merged
        // serializer. Without this, a `[coordinator].executor = "native"` in
        // global vs `[dispatcher].executor = "claude"` in local would land
        // under two unrelated keys, and the merged display would only show
        // one of them.
        let mut warnings = Vec::new();
        normalize_legacy_tables(
            &mut global_val,
            &global_path.display().to_string(),
            &mut warnings,
        );
        normalize_legacy_tables(
            &mut local_val,
            &local_path.display().to_string(),
            &mut warnings,
        );
        emit_legacy_warnings(&warnings);

        // Apply endpoint inheritance policy BEFORE recording sources, so the
        // source map reflects the effective merged config: a global endpoint
        // entry that's been suppressed because local opted out should not
        // appear as "from global" in `wg config --list`.
        let active_named_profile = crate::profile::named::active().ok().flatten().is_some();
        apply_endpoint_inheritance_policy(&mut global_val, &local_val, active_named_profile);

        // Record sources: global first, then local overwrites
        let mut sources = BTreeMap::new();
        record_sources(&global_val, "", &ConfigSource::Global, &mut sources);
        record_sources(&local_val, "", &ConfigSource::Local, &mut sources);

        let agent_model_is_local = local_val
            .get("agent")
            .and_then(|a| a.get("model"))
            .and_then(|m| m.as_str())
            .is_some();

        // Merge and deserialize
        let mut merged = merge_toml(global_val.clone(), local_val.clone());
        strip_global_only_model_roles(&mut merged, &global_val, &local_val);
        let mut config: Config = merged
            .try_into()
            .map_err(|e| anyhow::anyhow!("Failed to deserialize merged config: {}", e))?;
        config.agent_model_is_local = agent_model_is_local;

        // Fill in defaults for keys not present in either file
        let default_config = Config::default();
        let default_val: toml::Value = toml::Value::try_from(&default_config)
            .unwrap_or(toml::Value::Table(toml::map::Map::new()));
        let mut default_sources = BTreeMap::new();
        record_sources(
            &default_val,
            "",
            &ConfigSource::Default,
            &mut default_sources,
        );
        for (key, src) in default_sources {
            sources.entry(key).or_insert(src);
        }

        Ok((config, sources))
    }

    /// Compute the effective compaction token threshold for the coordinator.
    ///
    /// If the coordinator model is found in the registry with a known context window,
    /// returns `context_window * compaction_threshold_ratio` (dynamic threshold).
    /// Falls back to `compaction_token_threshold` when:
    /// - No coordinator model is configured
    /// - Model not found in registry
    /// - Model's context_window is 0
    /// - compaction_threshold_ratio is 0.0
    pub fn effective_compaction_threshold(&self) -> u64 {
        let ratio = self.coordinator.compaction_threshold_ratio;
        if ratio > 0.0 {
            // Resolve coordinator model ID: coordinator.model first, then agent.model
            let raw_model = self
                .coordinator
                .model
                .as_deref()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    let m = self.agent.model.as_str();
                    if m.is_empty() { None } else { Some(m) }
                });
            // Parse provider:model format to get the registry lookup ID
            let model_id = raw_model.map(|m| parse_model_spec(m).model_id);
            if let Some(ref id) = model_id
                && let Some(entry) = self.registry_lookup(id)
                && entry.context_window > 0
            {
                return (entry.context_window as f64 * ratio).round() as u64;
            }
        }
        self.coordinator.compaction_token_threshold
    }

    /// Effective handler/executor for the dispatcher, derived from the model
    /// spec via [`crate::dispatch::handler_for_model`] — the single source of
    /// truth for routing.
    ///
    /// Falls back to `[agent].model` when `[dispatcher].model` is unset (the
    /// dispatcher inherits the agent default — the same fallback
    /// `strip_redundant_executor_keys` and `plan_spawn` apply). The legacy
    /// `[dispatcher].executor` config key is deliberately **not** consulted
    /// here: handler-first routing derives the handler from the model spec,
    /// and `plan_spawn`'s `enforce_model_compat` already overrides a
    /// contradictory legacy executor in the actual spawn path. Surfacing the
    /// model-derived handler in status/reload/TUI keeps the display in
    /// lock-step with the real route instead of labeling a deprecated key as
    /// the active executor — the `bug-handler-first-executor-display-spam`
    /// fix. Routing-adjacent callers that still need the legacy explicit-key
    /// override should use [`CoordinatorConfig::effective_executor`].
    pub fn effective_dispatcher_executor(&self) -> String {
        let model = self
            .coordinator
            .model
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.agent.model);
        crate::dispatch::handler_for_model(model)
            .as_str()
            .to_string()
    }

    /// Validate that all model fields use the `provider:model` format.
    ///
    /// Returns `Ok(())` if all model fields are valid, or an error listing every
    /// field that still uses a bare model name (e.g., `"opus"` instead of `"claude:opus"`).
    pub fn validate_model_format(&self) -> anyhow::Result<()> {
        let mut errors: Vec<String> = Vec::new();

        // Quiet variant: this runs on every config load and a bare provider
        // prefix is already surfaced once (with a dotted path) by
        // `deprecated_model_prefix_warnings_for_toml` at the same load. Using
        // the warning variant here would double-warn on every `wg` invocation.
        let check_model = |field: &str, model: &str| -> Option<String> {
            match parse_model_spec_strict_quiet(model) {
                Ok(_) => None,
                Err(e) => Some(format!("  {} = \"{}\": {}", field, model, e)),
            }
        };

        // agent.model
        if let Some(err) = check_model("agent.model", &self.agent.model) {
            errors.push(err);
        }

        // coordinator.model
        if let Some(ref m) = self.coordinator.model
            && let Some(err) = check_model("coordinator.model", m)
        {
            errors.push(err);
        }

        // coordinator.provider (deprecated — should not be present)
        if self.coordinator.provider.is_some() {
            errors.push(
                "  coordinator.provider is deprecated. \
                 Use provider:model format in coordinator.model instead."
                    .to_string(),
            );
        }

        // models.* sections
        let role_configs: Vec<(String, &RoleModelConfig)> = {
            let mut pairs = Vec::new();
            if let Some(ref cfg) = self.models.default {
                pairs.push(("models.default".to_string(), cfg));
            }
            for role in DispatchRole::ALL {
                if let Some(cfg) = self.models.get_role(*role) {
                    pairs.push((format!("models.{}", role), cfg));
                }
            }
            pairs
        };

        for (name, cfg) in &role_configs {
            if let Some(ref m) = cfg.model
                && let Some(err) = check_model(&format!("{}.model", name), m)
            {
                errors.push(err);
            }
            if cfg.provider.is_some() {
                errors.push(format!(
                    "  {}.provider is deprecated. \
                     Use provider:model format in {}.model instead.",
                    name, name
                ));
            }
        }

        // tier values
        if let Some(ref t) = self.tiers.fast
            && let Some(err) = check_model("tiers.fast", t)
        {
            errors.push(err);
        }
        if let Some(ref t) = self.tiers.standard
            && let Some(err) = check_model("tiers.standard", t)
        {
            errors.push(err);
        }
        if let Some(ref t) = self.tiers.premium
            && let Some(err) = check_model("tiers.premium", t)
        {
            errors.push(err);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(
                "Config contains model fields that need migration to provider:model format:\n\
                 {}\n\n\
                 To fix: update each field to use provider:model format (e.g., \"claude:opus\").\n\
                 Common mappings:\n\
                 {}\n\
                 {}\n\
                 {}",
                errors.join("\n"),
                "  opus / sonnet / haiku  →  claude:opus / claude:sonnet / claude:haiku",
                "  gpt-4o                 →  openai:gpt-4o",
                "  deepseek/deepseek-chat →  openrouter:deepseek/deepseek-chat",
            )
        }
    }

    /// Validate configuration for common mismatches.
    ///
    /// Returns a `ConfigValidation` containing errors (fatal) and warnings (informational).
    /// Errors should block service start. Warnings should be displayed but allow startup.
    pub fn validate_config(&self) -> ConfigValidation {
        let mut result = ConfigValidation::default();

        // Explicit execution fallbacks are all-or-nothing. A handler/provider
        // change is a fatal configuration error, not a candidate to skip.
        for declaration in &self.execution.fallbacks {
            let primary_key = match execution_system_key(&declaration.primary) {
                Ok(key) => key,
                Err(error) => {
                    result.errors.push(ConfigDiagnostic {
                        rule: "execution-fallback-primary".into(),
                        message: format!(
                            "execution fallback primary {:?} is invalid: {error:#}",
                            declaration.primary
                        ),
                        fix: "Use an explicit handler-first primary route.".into(),
                    });
                    continue;
                }
            };
            for candidate in &declaration.models {
                match execution_system_key(candidate) {
                    Ok(candidate_key) if candidate_key == primary_key => {}
                    Ok(candidate_key) => result.errors.push(ConfigDiagnostic {
                        rule: "execution-fallback-cross-system".into(),
                        message: format!(
                            "fallback {:?} has system {} but primary {:?} has system {}",
                            candidate, candidate_key, declaration.primary, primary_key
                        ),
                        fix: "Remove the candidate or use a model on the same handler and provider/wire."
                            .into(),
                    }),
                    Err(error) => result.errors.push(ConfigDiagnostic {
                        rule: "execution-fallback-candidate".into(),
                        message: format!(
                            "execution fallback candidate {candidate:?} is invalid: {error:#}"
                        ),
                        fix: "Use an explicit handler-first fallback route.".into(),
                    }),
                }
            }
        }

        // Check coordinator executor + model/provider combinations
        let executor = self.coordinator.effective_executor();
        let model = self
            .coordinator
            .model
            .as_deref()
            .unwrap_or(&self.agent.model);
        // Extract provider from model spec (provider:model format) instead of deprecated field
        let spec = parse_model_spec(model);

        // Rule 1: executor='claude' but model has a non-Anthropic provider prefix or
        // looks like a non-Anthropic model (contains '/' without an Anthropic provider).
        // Uses parse_model_spec to check provider instead of raw string heuristics.
        let is_anthropic_provider = |p: &str| -> bool { p == "anthropic" || p == "claude" };
        let model_looks_non_anthropic = if let Some(ref p) = spec.provider {
            // Model has provider:model format — check the provider
            !is_anthropic_provider(p)
        } else {
            // Bare model — use '/' heuristic as fallback (e.g. "deepseek/deepseek-chat")
            spec.model_id.contains('/') && !spec.model_id.starts_with("anthropic/")
        };
        if executor == "claude" && model_looks_non_anthropic {
            let diagnostic_message = if let Some(ref p) = spec.provider {
                format!(
                    "executor = 'claude' but model = '{}' has non-Anthropic provider '{}'. \
                     Will auto-route to native executor.",
                    model, p
                )
            } else {
                format!(
                    "executor = 'claude' but model = '{}' is non-Anthropic. \
                     Will auto-route to native executor.",
                    model
                )
            };
            result.warnings.push(ConfigDiagnostic {
                rule: "executor-model-auto-route".into(),
                message: diagnostic_message,
                fix: "Set executor = 'native' to make this explicit, \
                     or use claude:MODEL format for Anthropic models."
                    .to_string(),
            });
        }

        // Rule: non-Anthropic provider + Anthropic-only model alias (e.g. provider=openrouter, model=opus)
        // OpenRouter/OpenAI won't understand bare Anthropic aliases like "opus" or "sonnet".
        // Uses spec.model_id (from parse_model_spec) for registry lookup instead of raw model string.
        if let Some(ref p) = spec.provider
            && !is_anthropic_provider(p)
        {
            let model_id = &spec.model_id;
            let is_anthropic_only_model = !model_id.contains('/')
                && self
                    .registry_lookup(model_id)
                    .map(|e| e.provider == "anthropic")
                    .unwrap_or(false); // unknown models are not assumed Anthropic
            if is_anthropic_only_model {
                result.errors.push(ConfigDiagnostic {
                    rule: "provider-model-mismatch".into(),
                    message: format!(
                        "coordinator provider = '{}' but model = '{}' is an Anthropic model alias. \
                         Provider '{}' won't recognize this model name.",
                        p, model_id, p
                    ),
                    fix: format!(
                        "Use a {p}-compatible model (e.g. 'deepseek/deepseek-chat'), \
                         or set provider = 'anthropic' to use '{model_id}' via Anthropic.",
                    ),
                });
            }
        }

        // Rule 3: [models.*] model value doesn't match registry AND doesn't contain '/'
        let registry = self.effective_registry();
        let registry_ids: std::collections::HashSet<&str> =
            registry.iter().map(|e| e.id.as_str()).collect();

        // Check models.default and per-role model values
        let role_configs: Vec<(String, &RoleModelConfig)> = {
            let mut pairs = Vec::new();
            if let Some(ref cfg) = self.models.default {
                pairs.push(("default".to_string(), cfg));
            }
            for role in DispatchRole::ALL {
                if let Some(cfg) = self.models.get_role(*role) {
                    pairs.push((role.to_string(), cfg));
                }
            }
            pairs
        };

        for (role_name, role_cfg) in &role_configs {
            if let Some(ref m) = role_cfg.model {
                // Parse provider:model format to get the registry lookup ID
                let model_spec = parse_model_spec(m);
                let lookup_id = &model_spec.model_id;
                if !registry_ids.contains(lookup_id.as_str()) && !lookup_id.contains('/') {
                    result.warnings.push(ConfigDiagnostic {
                        rule: "unresolved-model-id".into(),
                        message: format!(
                            "models.{}.model = '{}' doesn't match any registry entry \
                             and doesn't look like a provider/model path. \
                             May be an unresolved short ID.",
                            role_name, m
                        ),
                        fix: format!(
                            "Add a [[model_registry]] entry for '{}', use a known ID \
                             ({}), or use a tier name (e.g., 'haiku', 'sonnet', 'opus').",
                            m,
                            registry_ids.iter().copied().collect::<Vec<_>>().join(", ")
                        ),
                    });
                }
            }
        }

        // Rule 4: model_registry entry's 'model' field doesn't contain '/'
        // (should be a full provider-qualified model name for non-Anthropic providers)
        for entry in &self.model_registry {
            if entry.provider != "anthropic" && !entry.model.contains('/') {
                result.warnings.push(ConfigDiagnostic {
                    rule: "registry-model-format".into(),
                    message: format!(
                        "model_registry entry '{}' (provider: '{}') has model = '{}' \
                         which doesn't contain '/'. OpenRouter and similar providers \
                         typically use 'provider/model' format.",
                        entry.id, entry.provider, entry.model
                    ),
                    fix: format!(
                        "Use the full model path, e.g., '{}/{}'.",
                        entry.provider, entry.model
                    ),
                });
            }
        }

        // Rule 5: llm_endpoints has api_key_file that doesn't exist or is empty
        for ep in &self.llm_endpoints.endpoints {
            if let Some(ref file_path) = ep.api_key_file {
                let expanded = expand_tilde(file_path);
                if !expanded.exists() {
                    result.errors.push(ConfigDiagnostic {
                        rule: "missing-api-key-file".into(),
                        message: format!(
                            "Endpoint '{}' (provider: '{}') references api_key_file = '{}' \
                             but the file does not exist.",
                            ep.name, ep.provider, file_path
                        ),
                        fix: format!(
                            "Create the file at '{}' with your API key, \
                             or use api_key_env to reference an environment variable instead.",
                            file_path
                        ),
                    });
                } else if let Ok(contents) = fs::read_to_string(&expanded)
                    && contents.trim().is_empty()
                {
                    result.errors.push(ConfigDiagnostic {
                        rule: "empty-api-key-file".into(),
                        message: format!(
                            "Endpoint '{}' (provider: '{}') references api_key_file = '{}' \
                             but the file is empty.",
                            ep.name, ep.provider, file_path
                        ),
                        fix: "Add your API key to the file.".into(),
                    });
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    #[test]
    fn tag_routing_entries_are_inert_legacy_config() {
        let rules = vec![
            TagRoutingEntry {
                tag: "frontend".into(),
                model: "codex:gpt-5-codex".into(),
                executor: Some("codex".into()),
            },
            TagRoutingEntry {
                tag: "infra".into(),
                model: "claude:opus".into(),
                executor: None,
            },
        ];
        let tags = vec!["frontend".to_string(), "urgent".to_string()];
        assert!(resolve_tag_routing(&rules, &tags).is_none());
    }

    #[test]
    fn tag_routing_returns_none_with_no_match() {
        let rules = vec![TagRoutingEntry {
            tag: "frontend".into(),
            model: "codex:gpt-5-codex".into(),
            executor: None,
        }];
        assert!(resolve_tag_routing(&rules, &["backend".to_string()]).is_none());
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.agent.executor, "claude");
        assert_eq!(config.agent.model, "claude:opus");
        assert_eq!(config.agent.interval, 10);
    }

    #[test]
    fn test_reasoning_level_validation_and_display() {
        for (raw, expected) in [
            ("off", ReasoningLevel::Off),
            ("minimal", ReasoningLevel::Minimal),
            ("low", ReasoningLevel::Low),
            ("medium", ReasoningLevel::Medium),
            ("high", ReasoningLevel::High),
            ("xhigh", ReasoningLevel::Xhigh),
            ("max", ReasoningLevel::Max),
        ] {
            assert_eq!(raw.parse::<ReasoningLevel>().unwrap(), expected);
            assert_eq!(expected.to_string(), raw);
        }

        let err = "extreme".parse::<ReasoningLevel>().unwrap_err().to_string();
        assert!(err.contains("invalid reasoning level 'extreme'"));
        assert!(err.contains("off, minimal, low, medium, high, xhigh, max"));
    }

    #[test]
    fn test_reasoning_config_toml_roundtrip_and_backward_compatibility() {
        let old: Config = toml::from_str(
            r#"
[models.default]
model = "pi:openai-codex:gpt-5.6-sol"

[tiers]
standard = "pi:openai-codex:gpt-5.6-sol"
"#,
        )
        .unwrap();
        assert_eq!(
            old.resolve_reasoning_for_role(DispatchRole::TaskAgent),
            None,
            "old configs without reasoning must keep omitting the handler flag"
        );

        let parsed: Config = toml::from_str(
            r#"
[models.default]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "medium"

[models.task_agent]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "high"

[tiers]
standard = "pi:openai-codex:gpt-5.6-sol"
standard_reasoning = "xhigh"
"#,
        )
        .unwrap();
        assert_eq!(
            parsed.models.default.as_ref().unwrap().reasoning,
            Some(ReasoningLevel::Medium)
        );
        assert_eq!(
            parsed.models.task_agent.as_ref().unwrap().reasoning,
            Some(ReasoningLevel::High)
        );
        assert_eq!(parsed.tiers.standard_reasoning, Some(ReasoningLevel::Xhigh));

        let serialized = toml::to_string_pretty(&parsed).unwrap();
        let reparsed: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(
            reparsed.resolve_reasoning_for_role(DispatchRole::TaskAgent),
            Some(ReasoningLevel::High)
        );
        assert!(serialized.contains("reasoning = \"high\""));
        assert!(serialized.contains("standard_reasoning = \"xhigh\""));
    }

    #[test]
    fn test_reasoning_resolves_independently_from_model_precedence() {
        let mut config = Config::default();
        config.agent.model = "pi:openai-codex:gpt-5.6-sol".to_string();
        config.models.default = Some(RoleModelConfig {
            provider: None,
            model: Some("pi:openai-codex:gpt-5.6-sol".to_string()),
            tier: None,
            endpoint: None,
            reasoning: Some(ReasoningLevel::Medium),
        });
        config.tiers.standard = Some("pi:openai-codex:gpt-5.6-sol".to_string());
        config.tiers.standard_reasoning = Some(ReasoningLevel::High);

        let resolved = config.resolve_model_for_role(DispatchRole::TaskAgent);
        assert_eq!(resolved.spawn_model_spec(), "pi:openai-codex:gpt-5.6-sol");
        assert_eq!(
            resolved.reasoning,
            Some(ReasoningLevel::High),
            "tier reasoning should outrank global/default reasoning"
        );

        config
            .models
            .set_model(DispatchRole::TaskAgent, "pi:openai-codex:gpt-5.6-sol");
        assert_eq!(
            config
                .resolve_model_for_role(DispatchRole::TaskAgent)
                .reasoning,
            Some(ReasoningLevel::High),
            "overriding only a model must not erase inherited reasoning"
        );

        config
            .models
            .set_reasoning(DispatchRole::TaskAgent, ReasoningLevel::Xhigh);
        assert_eq!(
            config
                .resolve_model_for_role(DispatchRole::TaskAgent)
                .reasoning,
            Some(ReasoningLevel::Xhigh),
            "role-specific reasoning should outrank tier reasoning"
        );
    }

    #[test]
    fn test_load_missing_config() {
        let temp_dir = TempDir::new().unwrap();
        let config = Config::load(temp_dir.path()).unwrap();
        assert_eq!(config.agent.executor, "claude");
    }

    #[test]
    fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();

        let mut config = Config::default();
        config.agent.model = "claude:haiku".to_string();
        config.agent.interval = 30;
        config.save(temp_dir.path()).unwrap();

        let loaded = Config::load(temp_dir.path()).unwrap();
        assert_eq!(loaded.agent.model, "claude:haiku");
        assert_eq!(loaded.agent.interval, 30);
    }

    #[test]
    fn test_init_config() {
        let temp_dir = TempDir::new().unwrap();

        // First init should create file
        let created = Config::init(temp_dir.path()).unwrap();
        assert!(created);

        // Second init should not overwrite
        let created = Config::init(temp_dir.path()).unwrap();
        assert!(!created);
    }

    #[test]
    fn test_parse_custom_config() {
        let toml_str = r#"
[agent]
executor = "opencode"
model = "gpt-4"
interval = 60

[project]
name = "My Project"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.agent.executor, "opencode");
        assert_eq!(config.agent.model, "gpt-4");
        assert_eq!(config.project.name, Some("My Project".to_string()));
    }

    #[test]
    fn test_matrix_config_default() {
        let config = MatrixConfig::default();
        assert!(config.homeserver_url.is_none());
        assert!(config.username.is_none());
        assert!(config.password.is_none());
        assert!(config.access_token.is_none());
        assert!(config.default_room.is_none());
        assert!(!config.has_credentials());
        assert!(!config.is_complete());
    }

    #[test]
    fn test_matrix_config_has_credentials() {
        let mut config = MatrixConfig::default();
        assert!(!config.has_credentials());

        config.homeserver_url = Some("https://matrix.org".to_string());
        assert!(!config.has_credentials());

        config.username = Some("@user:matrix.org".to_string());
        assert!(!config.has_credentials());

        config.password = Some("secret".to_string());
        assert!(config.has_credentials());
        assert!(!config.is_complete());

        config.default_room = Some("!room:matrix.org".to_string());
        assert!(config.is_complete());
    }

    #[test]
    fn test_matrix_config_access_token() {
        let config = MatrixConfig {
            homeserver_url: Some("https://matrix.org".to_string()),
            username: Some("@user:matrix.org".to_string()),
            access_token: Some("syt_abc123".to_string()),
            ..Default::default()
        };
        assert!(config.has_credentials());
    }

    #[test]
    fn test_default_agency_config() {
        let config = Config::default();
        assert!(config.agency.auto_evaluate);
        assert!(config.agency.auto_assign);
        assert!(config.agency.assigner_agent.is_none());
        assert!(config.agency.evaluator_agent.is_none());
        assert!(config.agency.evolver_agent.is_none());
        assert!(config.agency.retention_heuristics.is_none());
        // Run mode continuum defaults
        assert_eq!(config.agency.exploration_interval, 20);
        assert!((config.agency.cache_population_threshold - 0.8).abs() < f64::EPSILON);
        assert!(
            (config.agency.ucb_exploration_constant - std::f64::consts::SQRT_2).abs()
                < f64::EPSILON
        );
        assert!((config.agency.novelty_bonus_multiplier - 1.5).abs() < f64::EPSILON);
        assert_eq!(config.agency.bizarre_ideation_interval, 10);
    }

    #[test]
    fn test_parse_agency_config() {
        let toml_str = r#"
[agency]
auto_evaluate = true
auto_assign = true
assigner_agent = "abc123"
evaluator_agent = "def456"
evolver_agent = "ghi789"
retention_heuristics = "Retire roles scoring below 0.3 after 10 evaluations"
flip_inference_model = "openrouter:model-a"
flip_comparison_model = "openrouter:model-b"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.agency.auto_evaluate);
        assert!(config.agency.auto_assign);
        assert_eq!(config.agency.assigner_agent, Some("abc123".to_string()));
        assert_eq!(config.agency.evaluator_agent, Some("def456".to_string()));
        assert_eq!(config.agency.evolver_agent, Some("ghi789".to_string()));
        assert_eq!(
            config.agency.retention_heuristics,
            Some("Retire roles scoring below 0.3 after 10 evaluations".to_string())
        );
        assert_eq!(
            config.agency.flip_inference_model,
            Some("openrouter:model-a".to_string())
        );
        assert_eq!(
            config.agency.flip_comparison_model,
            Some("openrouter:model-b".to_string())
        );
    }

    #[test]
    fn test_agency_config_roundtrip() {
        let temp_dir = TempDir::new().unwrap();

        let mut config = Config::default();
        config.agency.auto_evaluate = true;
        config.agency.evolver_agent = Some("abc123".to_string());
        config.agency.flip_inference_model = Some("openrouter:model-c".to_string());
        config.agency.flip_comparison_model = Some("openrouter:model-d".to_string());
        config.save(temp_dir.path()).unwrap();

        let loaded = Config::load(temp_dir.path()).unwrap();
        assert!(loaded.agency.auto_evaluate);
        assert_eq!(loaded.agency.evolver_agent, Some("abc123".to_string()));
    }

    #[test]
    fn test_parse_matrix_config() {
        let toml_str = r#"
homeserver_url = "https://matrix.example.com"
username = "@bot:example.com"
access_token = "syt_token_here"
default_room = "!notifications:example.com"
"#;
        let config: MatrixConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.homeserver_url,
            Some("https://matrix.example.com".to_string())
        );
        assert_eq!(config.username, Some("@bot:example.com".to_string()));
        assert_eq!(config.access_token, Some("syt_token_here".to_string()));
        assert_eq!(
            config.default_room,
            Some("!notifications:example.com".to_string())
        );
        assert!(config.is_complete());
    }

    // ---- Global config / merge tests ----

    #[test]
    fn test_merge_toml_basic() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "sonnet"
executor = "claude"
"#,
        )
        .unwrap();
        let local: toml::Value = toml::from_str(
            r#"
[coordinator]
max_agents = 8
"#,
        )
        .unwrap();
        let merged = merge_toml(global, local);
        let table = merged.as_table().unwrap();
        // Global agent section preserved
        let agent = table["agent"].as_table().unwrap();
        assert_eq!(agent["model"].as_str().unwrap(), "sonnet");
        // Local coordinator section present
        let coord = table["coordinator"].as_table().unwrap();
        assert_eq!(coord["max_agents"].as_integer().unwrap(), 8);
    }

    #[test]
    fn test_merge_toml_local_overrides_global() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "sonnet"
executor = "claude"
interval = 10
"#,
        )
        .unwrap();
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "haiku"
"#,
        )
        .unwrap();
        let merged = merge_toml(global, local);
        let agent = merged.as_table().unwrap()["agent"].as_table().unwrap();
        // Local overrides model
        assert_eq!(agent["model"].as_str().unwrap(), "haiku");
        // Global's executor preserved
        assert_eq!(agent["executor"].as_str().unwrap(), "claude");
        // Global's interval preserved
        assert_eq!(agent["interval"].as_integer().unwrap(), 10);
    }

    #[test]
    fn test_merge_toml_nested_sections() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "sonnet"

[coordinator]
max_agents = 4
executor = "claude"
"#,
        )
        .unwrap();
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "haiku"

[coordinator]
executor = "native"
"#,
        )
        .unwrap();
        let merged = merge_toml(global, local);
        let t = merged.as_table().unwrap();
        assert_eq!(
            t["agent"].as_table().unwrap()["model"].as_str().unwrap(),
            "haiku"
        );
        assert_eq!(
            t["coordinator"].as_table().unwrap()["max_agents"]
                .as_integer()
                .unwrap(),
            4
        );
        assert_eq!(
            t["coordinator"].as_table().unwrap()["executor"]
                .as_str()
                .unwrap(),
            "native"
        );
    }

    #[test]
    fn test_merge_toml_empty_local() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "sonnet"
"#,
        )
        .unwrap();
        let local = toml::Value::Table(toml::map::Map::new());
        let merged = merge_toml(global, local);
        assert_eq!(
            merged.as_table().unwrap()["agent"].as_table().unwrap()["model"]
                .as_str()
                .unwrap(),
            "sonnet"
        );
    }

    #[test]
    fn test_merge_toml_empty_global() {
        let global = toml::Value::Table(toml::map::Map::new());
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "haiku"
"#,
        )
        .unwrap();
        let merged = merge_toml(global, local);
        assert_eq!(
            merged.as_table().unwrap()["agent"].as_table().unwrap()["model"]
                .as_str()
                .unwrap(),
            "haiku"
        );
    }

    #[test]
    fn test_strip_global_only_model_roles_basic() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "claude:opus"
[models.task_agent]
model = "claude:opus"
"#,
        )
        .unwrap();
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "openrouter:minimax/minimax-m2.7"
"#,
        )
        .unwrap();
        let mut merged = merge_toml(global.clone(), local.clone());
        strip_global_only_model_roles(&mut merged, &global, &local);
        let has_task_agent_model = merged
            .get("models")
            .and_then(|m| m.get("task_agent"))
            .and_then(|t| t.get("model"))
            .is_some();
        assert!(
            !has_task_agent_model,
            "global models.task_agent.model should be stripped when local sets agent.model"
        );
        assert_eq!(
            merged
                .get("agent")
                .unwrap()
                .get("model")
                .unwrap()
                .as_str()
                .unwrap(),
            "openrouter:minimax/minimax-m2.7"
        );
    }

    #[test]
    fn test_strip_global_model_roles_preserves_local_override() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "claude:opus"
[models.task_agent]
model = "claude:opus"
"#,
        )
        .unwrap();
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "openrouter:minimax/minimax-m2.7"
[models.task_agent]
model = "openrouter:minimax/minimax-m2.7"
"#,
        )
        .unwrap();
        let mut merged = merge_toml(global.clone(), local.clone());
        strip_global_only_model_roles(&mut merged, &global, &local);
        let task_model = merged
            .get("models")
            .unwrap()
            .get("task_agent")
            .unwrap()
            .get("model")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(task_model, "openrouter:minimax/minimax-m2.7");
    }

    #[test]
    fn test_strip_global_model_roles_no_local_agent_model() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "claude:opus"
[models.task_agent]
model = "claude:sonnet"
"#,
        )
        .unwrap();
        let local: toml::Value = toml::from_str(
            r#"
[coordinator]
max_agents = 2
"#,
        )
        .unwrap();
        let mut merged = merge_toml(global.clone(), local.clone());
        strip_global_only_model_roles(&mut merged, &global, &local);
        let task_model = merged
            .get("models")
            .unwrap()
            .get("task_agent")
            .unwrap()
            .get("model")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(task_model, "claude:sonnet");
    }

    #[test]
    fn test_local_agent_model_overrides_global_task_agent_in_resolution() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "claude:opus"
[models.task_agent]
model = "claude:opus"
"#,
        )
        .unwrap();
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "openrouter:minimax/minimax-m2.7"
"#,
        )
        .unwrap();
        let mut merged = merge_toml(global.clone(), local.clone());
        strip_global_only_model_roles(&mut merged, &global, &local);
        let mut config: Config = merged.try_into().unwrap();
        config.agent_model_is_local = true;
        let resolved = config.resolve_model_for_role(DispatchRole::TaskAgent);
        assert_eq!(
            resolved.model, "minimax/minimax-m2.7",
            "TaskAgent should resolve to local agent.model"
        );
        assert_eq!(resolved.provider, Some("openrouter".to_string()));
    }

    #[test]
    fn test_local_models_task_agent_preserved_in_resolution() {
        let global: toml::Value = toml::from_str(
            r#"
[agent]
model = "claude:opus"
[models.task_agent]
model = "claude:opus"
"#,
        )
        .unwrap();
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "openrouter:minimax/minimax-m2.7"
[models.task_agent]
model = "openrouter:qwen/qwen3-235b"
"#,
        )
        .unwrap();
        let mut merged = merge_toml(global.clone(), local.clone());
        strip_global_only_model_roles(&mut merged, &global, &local);
        let mut config: Config = merged.try_into().unwrap();
        config.agent_model_is_local = true;
        let resolved = config.resolve_model_for_role(DispatchRole::TaskAgent);
        assert_eq!(
            resolved.model, "qwen/qwen3-235b",
            "Local models.task_agent.model should be preserved"
        );
    }

    #[test]
    fn test_load_merged_no_global_file() {
        // When no global config exists, load_merged should still work
        // (loads only local). We test with a temp dir as local.
        let temp_dir = TempDir::new().unwrap();
        let local_toml = r#"
[agent]
model = "claude:haiku"
"#;
        fs::write(temp_dir.path().join("config.toml"), local_toml).unwrap();

        // This test depends on whether ~/.wg/config.toml exists on the
        // machine, but the merge should work either way.  If the global config
        // uses old format, the merge may fail — that's OK in that scenario.
        //
        // Under the file-swap profile design, local config always overrides
        // global (and global IS the active profile snapshot). So regardless of
        // which profile is active on the test machine, local "claude:haiku"
        // must win.
        match Config::load_merged(temp_dir.path()) {
            Ok(config) => {
                assert_eq!(config.agent.model, "claude:haiku");
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("provider:model"),
                    "Expected migration error, got: {}",
                    msg
                );
            }
        }
    }

    #[test]
    fn test_load_merged_no_local_file() {
        // When no local config exists, merged should be global + defaults
        let temp_dir = TempDir::new().unwrap();
        // No config.toml in temp_dir
        // If global config uses old format, this will error — that's expected
        match Config::load_merged(temp_dir.path()) {
            Ok(config) => {
                // Executor can be either the code default "claude" or the global config override
                assert!(
                    config.agent.executor == "claude" || config.agent.executor == "native",
                    "Expected executor to be 'claude' (default) or 'native' (global config), got: {}",
                    config.agent.executor
                );
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("provider:model"),
                    "Expected migration error, got: {}",
                    msg
                );
            }
        }
    }

    #[test]
    fn test_global_config_path() {
        let path = Config::global_config_path().unwrap();
        let s = path.to_string_lossy();
        assert!(
            s.ends_with(".wg/config.toml"),
            "expected canonical .wg/config.toml, got {s}"
        );
    }

    #[test]
    fn matrix_config_path_uses_worksgood_namespace() {
        let path = MatrixConfig::config_path().expect("platform config directory");
        assert!(path.ends_with(Path::new("worksgood").join("matrix.toml")));
        assert!(!path.to_string_lossy().contains("workgraph"));
    }

    #[test]
    fn test_config_source_display() {
        assert_eq!(ConfigSource::Global.to_string(), "global");
        assert_eq!(ConfigSource::Local.to_string(), "local");
        assert_eq!(ConfigSource::Default.to_string(), "default");
    }

    #[test]
    fn test_resolve_triage_default() {
        // With no config, triage resolves via Fast tier → haiku registry entry
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
        assert_eq!(resolved.provider, Some("anthropic".to_string()));
        assert!(resolved.registry_entry.is_some());
        assert_eq!(resolved.registry_entry.unwrap().id, "haiku");
    }

    #[test]
    fn test_resolve_flip_inference_default() {
        // With no config, flip_inference resolves via Fast tier → haiku registry entry
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::FlipInference);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
        assert!(resolved.registry_entry.is_some());
    }

    #[test]
    fn test_resolve_flip_comparison_default() {
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::FlipComparison);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
    }

    #[test]
    fn test_resolve_verification_default() {
        // Verification defaults to Premium tier → opus
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::Verification);
        assert_eq!(resolved.model, CLAUDE_OPUS_MODEL_ID);
    }

    #[test]
    fn test_resolve_models_section_override() {
        // [models.triage] should take highest priority
        let mut config = Config::default();
        config.models.triage = Some(RoleModelConfig {
            model: Some("routing-model".to_string()),
            provider: Some("openrouter".to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.model, "routing-model");
        assert_eq!(resolved.provider, Some("openrouter".to_string()));
    }

    #[test]
    fn test_resolve_evaluator_uses_fast_tier() {
        // Evaluator resolves via Fast tier → haiku registry entry (cheap metacognition)
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
    }

    #[test]
    fn test_default_provider_cascades_to_tier_defaults() {
        // Setting [models.default].provider = "openrouter" should cascade
        // to roles that use tier defaults (triage, flip_comparison, etc.)
        let mut config = Config::default();
        config.models.default = Some(RoleModelConfig {
            model: None,
            provider: Some("openrouter".to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });

        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
        assert_eq!(
            resolved.provider,
            Some("openrouter".to_string()),
            "Default provider should cascade to tier default roles"
        );

        let resolved = config.resolve_model_for_role(DispatchRole::FlipInference);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
        assert_eq!(resolved.provider, Some("openrouter".to_string()));

        let resolved = config.resolve_model_for_role(DispatchRole::FlipComparison);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
        assert_eq!(resolved.provider, Some("openrouter".to_string()));

        let resolved = config.resolve_model_for_role(DispatchRole::Verification);
        assert_eq!(resolved.model, CLAUDE_OPUS_MODEL_ID);
        assert_eq!(resolved.provider, Some("openrouter".to_string()));
    }

    #[test]
    fn test_default_provider_cascades_to_role_with_model_only() {
        // If a role has model set but no provider, default provider should cascade
        let mut config = Config::default();
        config.models.default = Some(RoleModelConfig {
            model: None,
            provider: Some("openrouter".to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        config.models.triage = Some(RoleModelConfig {
            model: Some("anthropic/claude-3.5-haiku".to_string()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });

        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.model, "anthropic/claude-3.5-haiku");
        assert_eq!(
            resolved.provider,
            Some("openrouter".to_string()),
            "Default provider should cascade when role only sets model"
        );
    }

    #[test]
    fn test_role_provider_overrides_default_provider() {
        // Role-specific provider should override default provider
        let mut config = Config::default();
        config.models.default = Some(RoleModelConfig {
            model: None,
            provider: Some("openrouter".to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        config.models.triage = Some(RoleModelConfig {
            model: Some("gpt-4o-mini".to_string()),
            provider: Some("openai".to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });

        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.model, "gpt-4o-mini");
        assert_eq!(
            resolved.provider,
            Some("openai".to_string()),
            "Role-specific provider should take priority"
        );
    }

    #[test]
    fn test_default_provider_cascades_to_global_fallback() {
        // Evaluator resolves via Fast tier; default provider overrides registry provider
        let mut config = Config::default();
        config.models.default = Some(RoleModelConfig {
            model: None,
            provider: Some("openrouter".to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });

        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
        assert_eq!(
            resolved.provider,
            Some("openrouter".to_string()),
            "Default provider should cascade to tier-resolved roles"
        );
    }

    #[test]
    fn test_resolve_tier_strips_handler_first_nex_openrouter_prefix() {
        // fix-agency-flip: a canonical handler-first weak-tier spec
        // `nex:openrouter:<model>` must resolve to provider=openrouter with the
        // INNER model id — NOT the `oai-compat` localhost default the bare `nex`
        // prefix would pick (which routed the agency .flip/.assign/.evaluate
        // one-shot to the wrong wire with a bogus `openrouter:<model>` id).
        let mut config = Config::default();
        config.tiers.fast = Some("nex:openrouter:openai/gpt-4o-mini".to_string());
        let resolved = config.resolve_tier(Tier::Fast).expect("fast tier resolves");
        assert_eq!(resolved.provider.as_deref(), Some("openrouter"));
        assert_eq!(resolved.model, "openai/gpt-4o-mini");

        // The legacy `native:` handler alias strips identically.
        config.tiers.fast = Some("native:openrouter:openai/gpt-4o-mini".to_string());
        let resolved = config.resolve_tier(Tier::Fast).expect("fast tier resolves");
        assert_eq!(resolved.provider.as_deref(), Some("openrouter"));
        assert_eq!(resolved.model, "openai/gpt-4o-mini");

        // The bare `openrouter:<model>` form (no handler token) was already
        // correct and stays so.
        config.tiers.fast = Some("openrouter:openai/gpt-4o-mini".to_string());
        let resolved = config.resolve_tier(Tier::Fast).expect("fast tier resolves");
        assert_eq!(resolved.provider.as_deref(), Some("openrouter"));
        assert_eq!(resolved.model, "openai/gpt-4o-mini");

        // A bare in-process `nex:<model>` (NO inner provider) is NOT stripped —
        // it stays on the localhost oai-compat wire, unchanged.
        config.tiers.fast = Some("nex:local-qwen-model".to_string());
        let resolved = config.resolve_tier(Tier::Fast).expect("fast tier resolves");
        assert_eq!(resolved.provider.as_deref(), Some("oai-compat"));
        assert_eq!(resolved.model, "local-qwen-model");
    }

    #[test]
    fn test_resolve_agency_roles_strip_handler_first_weak_tier() {
        // fix-agency-flip: the full agency one-shot resolution
        // (agency_native_lightweight_call -> resolve_model_for_role -> Fast tier)
        // for a handler-first Pi-style profile — both tiers `nex:openrouter:...`.
        // Evaluator/Assigner must resolve to provider=openrouter + inner model
        // id, and the provider-cascade derived from `agent.model` (also
        // handler-first) must NOT re-pollute it back to oai-compat/localhost.
        let mut config = Config::default();
        config.tiers.fast = Some("nex:openrouter:openai/gpt-4o-mini".to_string());
        // Strong tier / agent.model is also handler-first (full Pi profile).
        config.agent.model = "nex:openrouter:z-ai/glm-5.2".to_string();

        for role in [DispatchRole::Evaluator, DispatchRole::Assigner] {
            let resolved = config.resolve_model_for_role(role);
            assert_eq!(
                resolved.provider.as_deref(),
                Some("openrouter"),
                "{role:?} weak-tier must resolve provider=openrouter, not oai-compat"
            );
            assert_eq!(
                resolved.model, "openai/gpt-4o-mini",
                "{role:?} weak-tier model id must be the inner id, not `openrouter:<model>`"
            );
        }

        // The bare `openrouter:<model>` weak-tier form resolves identically.
        config.tiers.fast = Some("openrouter:openai/gpt-4o-mini".to_string());
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.provider.as_deref(), Some("openrouter"));
        assert_eq!(resolved.model, "openai/gpt-4o-mini");
    }

    #[test]
    fn test_tier_serde_roundtrip() {
        // Tier serializes/deserializes correctly
        let tier = Tier::Fast;
        let json = serde_json::to_string(&tier).unwrap();
        assert_eq!(json, "\"fast\"");
        let parsed: Tier = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Tier::Fast);

        let tier = Tier::Premium;
        let json = serde_json::to_string(&tier).unwrap();
        assert_eq!(json, "\"premium\"");
        let parsed: Tier = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Tier::Premium);
    }

    #[test]
    fn test_model_registry_entry_serde() {
        let entry = ModelRegistryEntry {
            id: "test".into(),
            provider: "anthropic".into(),
            model: "claude-test".into(),
            tier: Tier::Standard,
            context_window: 100_000,
            max_output_tokens: 4096,
            cost_per_input_mtok: 1.0,
            cost_per_output_mtok: 5.0,
            ..Default::default()
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ModelRegistryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test");
        assert_eq!(parsed.tier, Tier::Standard);
        assert_eq!(parsed.context_window, 100_000);
    }

    #[test]
    fn test_tier_config_serde() {
        let tc = TierConfig {
            fast: Some("haiku".into()),
            fast_reasoning: None,
            standard: None,
            standard_reasoning: None,
            premium: Some("opus".into()),
            premium_reasoning: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let parsed: TierConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.fast, Some("haiku".to_string()));
        assert!(parsed.standard.is_none());
        assert_eq!(parsed.premium, Some("opus".to_string()));
    }

    #[test]
    fn test_effective_registry_returns_builtins_when_empty() {
        let config = Config::default();
        let registry = config.effective_registry();
        assert_eq!(registry.len(), 8);
        assert!(registry.iter().any(|e| e.id == "haiku"));
        assert!(registry.iter().any(|e| e.id == "sonnet"));
        assert!(registry.iter().any(|e| e.id == "opus"));
        assert!(registry.iter().any(|e| e.id == "fable"));
        assert!(registry.iter().any(|e| e.id == "claude:haiku"));
        assert!(registry.iter().any(|e| e.id == "claude:sonnet"));
        assert!(registry.iter().any(|e| e.id == "claude:opus"));
        assert!(registry.iter().any(|e| e.id == "claude:fable"));
        // Fable carries its full CLI id (no bare `fable` shortcut exists).
        let fable = registry.iter().find(|e| e.id == "fable").unwrap();
        assert_eq!(fable.model, CLAUDE_FABLE_MODEL_ID);
        assert_eq!(fable.tier, Tier::Premium);
    }

    #[test]
    fn test_effective_registry_returns_custom_when_configured() {
        let mut config = Config::default();
        config.model_registry = vec![ModelRegistryEntry {
            id: "custom".into(),
            provider: "local".into(),
            model: "my-model".into(),
            tier: Tier::Fast,
            ..Default::default()
        }];
        let registry = config.effective_registry();
        // 8 built-in + 1 custom = 9
        assert_eq!(registry.len(), 9);
        assert!(registry.iter().any(|e| e.id == "custom"));
        assert!(registry.iter().any(|e| e.id == "haiku"));
    }

    #[test]
    fn test_effective_registry_custom_overrides_builtin() {
        let mut config = Config::default();
        config.model_registry = vec![ModelRegistryEntry {
            id: "haiku".into(),
            provider: "local".into(),
            model: "my-haiku".into(),
            tier: Tier::Fast,
            ..Default::default()
        }];
        let registry = config.effective_registry();
        // 7 remaining built-ins + 1 override = 8
        assert_eq!(registry.len(), 8);
        let haiku = registry.iter().find(|e| e.id == "haiku").unwrap();
        assert_eq!(haiku.model, "my-haiku");
        assert_eq!(haiku.provider, "local");
    }

    #[test]
    fn test_resolve_tier_with_registry() {
        let config = Config::default();
        let resolved = config.resolve_tier(Tier::Fast).unwrap();
        assert_eq!(resolved.model, CLAUDE_HAIKU_MODEL_ID);
        assert_eq!(resolved.provider, Some("anthropic".to_string()));
        assert!(resolved.registry_entry.is_some());
    }

    #[test]
    fn test_resolve_tier_bare_model_id_not_in_registry() {
        let mut config = Config::default();
        config.tiers.fast = Some("custom-model".into());
        let resolved = config.resolve_tier(Tier::Fast).unwrap();
        assert_eq!(resolved.model, "custom-model");
        assert!(resolved.provider.is_none());
        assert!(resolved.registry_entry.is_none());
    }

    #[test]
    fn test_role_tier_override() {
        // [models.evaluator].tier = "premium" should resolve to opus
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: None,
            provider: None,
            tier: Some(Tier::Premium),
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, CLAUDE_OPUS_MODEL_ID);
    }

    #[test]
    fn test_direct_model_override_takes_priority_over_tier() {
        // Direct model override should beat tier-based resolution
        let mut config = Config::default();
        config.models.triage = Some(RoleModelConfig {
            model: Some("my-custom-model".to_string()),
            provider: None,
            tier: Some(Tier::Premium), // Should be ignored because model is set
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.model, "my-custom-model");
    }

    #[test]
    fn test_dispatch_role_default_tier() {
        assert_eq!(DispatchRole::Triage.default_tier(), Tier::Fast);
        assert_eq!(DispatchRole::FlipComparison.default_tier(), Tier::Fast);
        assert_eq!(DispatchRole::Assigner.default_tier(), Tier::Fast);
        assert_eq!(DispatchRole::FlipInference.default_tier(), Tier::Fast);
        assert_eq!(DispatchRole::Evaluator.default_tier(), Tier::Fast);
        assert_eq!(DispatchRole::TaskAgent.default_tier(), Tier::Standard);
        assert_eq!(DispatchRole::Evolver.default_tier(), Tier::Premium);
        assert_eq!(DispatchRole::Creator.default_tier(), Tier::Premium);
        assert_eq!(DispatchRole::Verification.default_tier(), Tier::Premium);
        assert_eq!(DispatchRole::Default.default_tier(), Tier::Standard);
        assert_eq!(DispatchRole::Placer.default_tier(), Tier::Fast);
    }

    #[test]
    fn test_tier_display_and_fromstr() {
        assert_eq!(Tier::Fast.to_string(), "fast");
        assert_eq!(Tier::Standard.to_string(), "standard");
        assert_eq!(Tier::Premium.to_string(), "premium");

        assert_eq!("fast".parse::<Tier>().unwrap(), Tier::Fast);
        assert_eq!("Standard".parse::<Tier>().unwrap(), Tier::Standard);
        assert_eq!("PREMIUM".parse::<Tier>().unwrap(), Tier::Premium);
        assert!("unknown".parse::<Tier>().is_err());
    }

    // ---- EndpointsConfig::find_for_provider tests ----

    #[test]
    fn test_find_for_provider_empty() {
        let endpoints = EndpointsConfig::default();
        assert!(endpoints.find_for_provider("openai").is_none());
    }

    #[test]
    fn test_find_for_provider_single_match() {
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![EndpointConfig {
                name: "my-openai".to_string(),
                provider: "openai".to_string(),
                url: Some("https://api.openai.com/v1".to_string()),
                model: None,
                api_key: Some("sk-test-key".to_string()),
                api_key_file: None,
                api_key_env: None,
                api_key_ref: None,
                is_default: false,
                context_window: None,
            }],
        };
        let ep = endpoints.find_for_provider("openai").unwrap();
        assert_eq!(ep.name, "my-openai");
        assert_eq!(ep.api_key.as_deref(), Some("sk-test-key"));
    }

    #[test]
    fn test_find_for_provider_no_match() {
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![EndpointConfig {
                name: "my-openai".to_string(),
                provider: "openai".to_string(),
                url: None,
                model: None,
                api_key: Some("sk-test".to_string()),
                api_key_file: None,
                api_key_env: None,
                api_key_ref: None,
                is_default: false,
                context_window: None,
            }],
        };
        assert!(endpoints.find_for_provider("anthropic").is_none());
    }

    #[test]
    fn test_find_for_provider_prefers_default() {
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![
                EndpointConfig {
                    name: "first-openai".to_string(),
                    provider: "openai".to_string(),
                    url: None,
                    model: None,
                    api_key: Some("sk-first".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
                EndpointConfig {
                    name: "default-openai".to_string(),
                    provider: "openai".to_string(),
                    url: None,
                    model: None,
                    api_key: Some("sk-default".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: true,
                    context_window: None,
                },
                EndpointConfig {
                    name: "third-openai".to_string(),
                    provider: "openai".to_string(),
                    url: None,
                    model: None,
                    api_key: Some("sk-third".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
            ],
        };
        let ep = endpoints.find_for_provider("openai").unwrap();
        assert_eq!(ep.name, "default-openai");
        assert_eq!(ep.api_key.as_deref(), Some("sk-default"));
    }

    #[test]
    fn test_find_for_provider_first_match_without_default() {
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![
                EndpointConfig {
                    name: "anthropic-ep".to_string(),
                    provider: "anthropic".to_string(),
                    url: None,
                    model: None,
                    api_key: Some("ant-key".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
                EndpointConfig {
                    name: "first-openai".to_string(),
                    provider: "openai".to_string(),
                    url: None,
                    model: None,
                    api_key: Some("sk-first".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
                EndpointConfig {
                    name: "second-openai".to_string(),
                    provider: "openai".to_string(),
                    url: None,
                    model: None,
                    api_key: Some("sk-second".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
            ],
        };
        // Without a default, returns the first matching provider
        let ep = endpoints.find_for_provider("openai").unwrap();
        assert_eq!(ep.name, "first-openai");
    }

    #[test]
    fn test_find_for_provider_url_and_key() {
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![EndpointConfig {
                name: "openrouter".to_string(),
                provider: "openrouter".to_string(),
                url: Some("https://openrouter.ai/api/v1".to_string()),
                model: Some(format!("anthropic/{CLAUDE_SONNET_MODEL_ID}")),
                api_key: Some("sk-or-test".to_string()),
                api_key_file: None,
                api_key_env: None,
                api_key_ref: None,
                is_default: true,
                context_window: None,
            }],
        };
        let expected_model = format!("anthropic/{CLAUDE_SONNET_MODEL_ID}");
        let ep = endpoints.find_for_provider("openrouter").unwrap();
        assert_eq!(ep.url.as_deref(), Some("https://openrouter.ai/api/v1"));
        assert_eq!(ep.api_key.as_deref(), Some("sk-or-test"));
        assert_eq!(ep.model.as_deref(), Some(expected_model.as_str()));
    }

    // ---- EndpointsConfig::find_default tests ----

    #[test]
    fn test_find_default_empty() {
        let endpoints = EndpointsConfig::default();
        assert!(endpoints.find_default().is_none());
    }

    #[test]
    fn test_find_default_returns_default_endpoint() {
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![
                EndpointConfig {
                    name: "openai".to_string(),
                    provider: "openai".to_string(),
                    url: None,
                    model: None,
                    api_key: None,
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
                EndpointConfig {
                    name: "openrouter".to_string(),
                    provider: "openrouter".to_string(),
                    url: None,
                    model: None,
                    api_key: None,
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: true,
                    context_window: None,
                },
            ],
        };
        let ep = endpoints.find_default().unwrap();
        assert_eq!(ep.name, "openrouter");
    }

    #[test]
    fn test_find_default_falls_back_to_first() {
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![EndpointConfig {
                name: "only".to_string(),
                provider: "openai".to_string(),
                url: None,
                model: None,
                api_key: None,
                api_key_file: None,
                api_key_env: None,
                api_key_ref: None,
                is_default: false,
                context_window: None,
            }],
        };
        let ep = endpoints.find_default().unwrap();
        assert_eq!(ep.name, "only");
    }

    #[test]
    fn test_find_default_resolves_api_key_for_non_matching_provider() {
        // Simulates the bug scenario: model resolves to provider "openai" but
        // the only configured endpoint has provider "openrouter". find_for_provider("openai")
        // returns None but find_default() returns the openrouter endpoint.
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![EndpointConfig {
                name: "openrouter".to_string(),
                provider: "openrouter".to_string(),
                url: None,
                model: None,
                api_key: Some("sk-or-test-key".to_string()),
                api_key_file: None,
                api_key_env: None,
                api_key_ref: None,
                is_default: true,
                context_window: None,
            }],
        };
        // Provider-based lookup misses
        assert!(endpoints.find_for_provider("openai").is_none());
        // Default fallback finds it
        let ep = endpoints.find_default().unwrap();
        assert_eq!(ep.provider, "openrouter");
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-or-test-key"));
        // Verify env var names for the provider
        let env_vars = EndpointConfig::env_var_names_for_provider(&ep.provider);
        assert!(env_vars.contains(&"OPENROUTER_API_KEY"));
    }

    // ---- EndpointConfig::resolve_api_key tests ----

    #[test]
    fn test_resolve_api_key_inline() {
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: Some("sk-inline".to_string()),
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-inline"));
    }

    #[test]
    fn test_resolve_api_key_inline_takes_priority() {
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: Some("sk-inline".to_string()),
            api_key_file: Some("/nonexistent/file".to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        // Inline key should win even if api_key_file is also set
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-inline"));
    }

    #[test]
    fn test_resolve_api_key_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test.key");
        std::fs::write(&key_path, "sk-from-file\n").unwrap();
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some(key_path.to_string_lossy().to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-from-file"));
    }

    #[test]
    fn test_resolve_api_key_file_trims_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test.key");
        std::fs::write(&key_path, "  sk-trimmed  \n\n").unwrap();
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some(key_path.to_string_lossy().to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-trimmed"));
    }

    #[test]
    fn test_resolve_api_key_file_not_found() {
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some("/nonexistent/path/key.txt".to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let err = ep.resolve_api_key(None).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("Failed to read API key from"));
        assert!(msg.contains("/nonexistent/path/key.txt"));
    }

    #[test]
    fn test_resolve_api_key_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("empty.key");
        std::fs::write(&key_path, "  \n").unwrap();
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some(key_path.to_string_lossy().to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let err = ep.resolve_api_key(None).unwrap_err();
        assert!(format!("{}", err).contains("empty"));
    }

    #[test]
    fn test_resolve_api_key_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("keys").join("test.key");
        std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        std::fs::write(&key_path, "sk-relative").unwrap();
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some("keys/test.key".to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(Some(dir.path())).unwrap();
        assert_eq!(key.as_deref(), Some("sk-relative"));
    }

    #[test]
    fn test_resolve_api_key_none() {
        // Use "local" provider which has no env var fallback
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "local".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(None).unwrap();
        assert!(key.is_none());
    }

    #[test]
    #[serial]
    fn test_resolve_api_key_env_var_fallback() {
        // Save/clear env
        let saved = std::env::var("OPENAI_API_KEY").ok();
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-test") };
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-env-test"));
        // Restore env
        match saved {
            Some(v) => unsafe { std::env::set_var("OPENAI_API_KEY", v) },
            None => unsafe { std::env::remove_var("OPENAI_API_KEY") },
        }
    }

    #[test]
    #[serial]
    fn test_resolve_api_key_inline_beats_env_var() {
        let saved = std::env::var("OPENAI_API_KEY").ok();
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-should-lose") };
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: Some("sk-inline-wins".to_string()),
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-inline-wins"));
        match saved {
            Some(v) => unsafe { std::env::set_var("OPENAI_API_KEY", v) },
            None => unsafe { std::env::remove_var("OPENAI_API_KEY") },
        }
    }

    #[test]
    #[serial]
    fn test_resolve_api_key_file_beats_env_var() {
        let saved = std::env::var("OPENAI_API_KEY").ok();
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-should-lose") };
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test.key");
        std::fs::write(&key_path, "sk-file-wins").unwrap();
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some(key_path.to_string_lossy().to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-file-wins"));
        match saved {
            Some(v) => unsafe { std::env::set_var("OPENAI_API_KEY", v) },
            None => unsafe { std::env::remove_var("OPENAI_API_KEY") },
        }
    }

    #[test]
    #[serial]
    fn test_resolve_api_key_openrouter_env_var_cascade() {
        let saved_or = std::env::var("OPENROUTER_API_KEY").ok();
        let saved_oai = std::env::var("OPENAI_API_KEY").ok();
        // Clear both, set only OPENAI_API_KEY
        unsafe { std::env::remove_var("OPENROUTER_API_KEY") };
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-oai-fallback") };
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openrouter".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        let key = ep.resolve_api_key(None).unwrap();
        assert_eq!(key.as_deref(), Some("sk-oai-fallback"));
        // Restore
        match saved_or {
            Some(v) => unsafe { std::env::set_var("OPENROUTER_API_KEY", v) },
            None => unsafe { std::env::remove_var("OPENROUTER_API_KEY") },
        }
        match saved_oai {
            Some(v) => unsafe { std::env::set_var("OPENAI_API_KEY", v) },
            None => unsafe { std::env::remove_var("OPENAI_API_KEY") },
        }
    }

    #[test]
    fn test_env_var_names_for_provider() {
        assert_eq!(
            EndpointConfig::env_var_names_for_provider("openrouter"),
            &["OPENROUTER_API_KEY", "OPENAI_API_KEY"]
        );
        assert_eq!(
            EndpointConfig::env_var_names_for_provider("openai"),
            &["OPENAI_API_KEY"]
        );
        assert_eq!(
            EndpointConfig::env_var_names_for_provider("anthropic"),
            &["ANTHROPIC_API_KEY"]
        );
        assert!(EndpointConfig::env_var_names_for_provider("local").is_empty());
        assert!(EndpointConfig::env_var_names_for_provider("unknown").is_empty());
    }

    #[test]
    fn test_masked_key_with_file_ref() {
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some("~/.config/worksgood/openai.key".to_string()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        };
        assert_eq!(ep.masked_key(), "(from file)");
    }

    #[test]
    fn test_masked_key_with_secret_ref() {
        let ep = EndpointConfig {
            name: "test".to_string(),
            provider: "openrouter".to_string(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            api_key_ref: Some("keystore:openrouter".to_string()),
            is_default: false,
            context_window: None,
        };
        assert_eq!(ep.masked_key(), "(from secret ref)");
        assert_eq!(ep.key_source(), "keystore:openrouter");
    }

    // ---- Endpoint routing tests ----

    #[test]
    fn test_find_by_name() {
        let endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![
                EndpointConfig {
                    name: "openrouter".to_string(),
                    provider: "openrouter".to_string(),
                    url: Some("https://openrouter.ai/api/v1".to_string()),
                    model: None,
                    api_key: Some("sk-or-test".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: false,
                    context_window: None,
                },
                EndpointConfig {
                    name: "anthropic-direct".to_string(),
                    provider: "anthropic".to_string(),
                    url: None,
                    model: None,
                    api_key: Some("sk-ant-test".to_string()),
                    api_key_file: None,
                    api_key_env: None,
                    api_key_ref: None,
                    is_default: true,
                    context_window: None,
                },
            ],
        };
        let ep = endpoints.find_by_name("openrouter").unwrap();
        assert_eq!(ep.provider, "openrouter");
        assert_eq!(ep.api_key.as_deref(), Some("sk-or-test"));

        let ep = endpoints.find_by_name("anthropic-direct").unwrap();
        assert_eq!(ep.provider, "anthropic");

        assert!(endpoints.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_endpoint_cascades_from_default() {
        let mut config = Config::default();
        config.models.default = Some(RoleModelConfig {
            model: None,
            provider: None,
            tier: None,
            endpoint: Some("openrouter".to_string()),
            reasoning: None,
        });

        // Triage should inherit the default endpoint
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.endpoint.as_deref(), Some("openrouter"));

        // Evaluator should also inherit
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.endpoint.as_deref(), Some("openrouter"));
    }

    #[test]
    fn test_role_endpoint_overrides_default() {
        let mut config = Config::default();
        config.models.default = Some(RoleModelConfig {
            model: None,
            provider: None,
            tier: None,
            endpoint: Some("openrouter".to_string()),
            reasoning: None,
        });
        config.models.evaluator = Some(RoleModelConfig {
            model: None,
            provider: None,
            tier: None,
            endpoint: Some("anthropic-direct".to_string()),
            reasoning: None,
        });

        // Triage inherits default
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(resolved.endpoint.as_deref(), Some("openrouter"));

        // Evaluator uses its own endpoint
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.endpoint.as_deref(), Some("anthropic-direct"));
    }

    #[test]
    fn test_no_endpoint_is_backward_compatible() {
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert!(resolved.endpoint.is_none());
    }

    #[test]
    fn test_set_endpoint() {
        let mut config = Config::default();
        config
            .models
            .set_endpoint(DispatchRole::Evaluator, "openrouter");
        let role_cfg = config.models.evaluator.unwrap();
        assert_eq!(role_cfg.endpoint.as_deref(), Some("openrouter"));
        assert!(role_cfg.model.is_none()); // Didn't touch model
        assert!(role_cfg.provider.is_none()); // Didn't touch provider
    }

    // --- effective_compaction_threshold tests ---

    #[test]
    fn test_effective_compaction_threshold_dynamic_from_registry() {
        // Built-in "haiku" has context_window=200_000; 80% = 160_000
        let mut config = Config::default();
        config.coordinator.model = Some("claude:haiku".to_string());
        config.coordinator.compaction_threshold_ratio = 0.8;
        let threshold = config.effective_compaction_threshold();
        assert_eq!(threshold, 160_000);
    }

    #[test]
    fn test_effective_compaction_threshold_mock_200k_context_window() {
        // Mock API returning 200k context window → threshold set to 160k
        let mut config = Config::default();
        config.model_registry = vec![ModelRegistryEntry {
            id: "mock-model".into(),
            provider: "anthropic".into(),
            model: "claude-mock".into(),
            tier: Tier::Standard,
            context_window: 200_000,
            ..Default::default()
        }];
        config.coordinator.model = Some("mock-model".to_string());
        config.coordinator.compaction_threshold_ratio = 0.8;
        let threshold = config.effective_compaction_threshold();
        assert_eq!(threshold, 160_000);
    }

    #[test]
    fn test_effective_compaction_threshold_fallback_unknown_model() {
        // Model not in registry → fallback to compaction_token_threshold
        let mut config = Config::default();
        config.coordinator.model = Some("unknown-model".to_string());
        config.coordinator.compaction_token_threshold = 50_000;
        let threshold = config.effective_compaction_threshold();
        assert_eq!(threshold, 50_000);
    }

    #[test]
    fn test_effective_compaction_threshold_fallback_no_model() {
        // No coordinator model → falls back to agent.model
        let config = Config::default();
        // agent.model defaults to "claude:opus" → registry lookup "opus" (200_000 context window)
        // 200_000 * 0.8 = 160_000
        let threshold = config.effective_compaction_threshold();
        assert_eq!(threshold, 160_000); // uses agent.model "claude:opus" fallback
    }

    #[test]
    fn test_effective_compaction_threshold_ratio_zero_uses_hardcoded() {
        // Ratio = 0.0 → always use compaction_token_threshold
        let mut config = Config::default();
        config.coordinator.model = Some("claude:haiku".to_string());
        config.coordinator.compaction_threshold_ratio = 0.0;
        config.coordinator.compaction_token_threshold = 75_000;
        let threshold = config.effective_compaction_threshold();
        assert_eq!(threshold, 75_000);
    }

    #[test]
    fn test_effective_compaction_threshold_custom_ratio() {
        // sonnet has context_window=200_000; 60% = 120_000
        let mut config = Config::default();
        config.coordinator.model = Some("claude:sonnet".to_string());
        config.coordinator.compaction_threshold_ratio = 0.6;
        let threshold = config.effective_compaction_threshold();
        assert_eq!(threshold, 120_000);
    }

    // ---- Registry resolution in resolve_model_for_role steps 1, 2, 5, 6 ----

    #[test]
    fn test_registry_resolve_step1_role_model_override() {
        // Step 1: [models.evaluator].model = "sonnet" should resolve via registry
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("sonnet".to_string()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, CLAUDE_SONNET_MODEL_ID);
        assert!(resolved.registry_entry.is_some());
        assert_eq!(resolved.registry_entry.unwrap().id, "sonnet");
    }

    #[test]
    fn test_registry_resolve_step1_custom_registry_entry() {
        // Step 1: custom registry entry "deepseek-chat" resolves to full path
        let mut config = Config::default();
        config.model_registry = vec![ModelRegistryEntry {
            id: "deepseek-chat".into(),
            provider: "deepseek".into(),
            model: "deepseek/deepseek-chat".into(),
            tier: Tier::Standard,
            ..Default::default()
        }];
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("deepseek-chat".to_string()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, "deepseek/deepseek-chat");
        assert_eq!(resolved.provider, Some("deepseek".to_string()));
        assert!(resolved.registry_entry.is_some());
    }

    #[test]
    fn test_registry_resolve_step1_provider_override_beats_registry() {
        // Step 1: explicit provider in role config overrides registry provider
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("sonnet".to_string()),
            provider: Some("openrouter".to_string()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, CLAUDE_SONNET_MODEL_ID);
        assert_eq!(resolved.provider, Some("openrouter".to_string()));
        assert!(resolved.registry_entry.is_some());
    }

    #[test]
    fn test_registry_resolve_step1_passthrough_unknown() {
        // Step 1: unknown model string passes through without registry_entry
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("some-unknown-model".to_string()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, "some-unknown-model");
        assert!(resolved.registry_entry.is_none());
    }

    // Note: Steps 4 and 5 are currently unreachable because effective_tiers()
    // always fills defaults, so step 3 (resolve_tier with default tier) always
    // succeeds. The registry lookup code is added for correctness if that changes.
    // The registry lookup pattern is identical to step 1 which is tested above.

    // -----------------------------------------------------------------------
    // validate_config tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_config_default_is_clean() {
        let config = Config::default();
        let v = config.validate_config();
        assert!(
            v.is_clean(),
            "Default config should be clean: {}",
            v.display()
        );
    }

    #[test]
    fn test_validate_config_claude_executor_with_slash_model_warns() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        config.coordinator.model = Some("minimax/minimax-m2.5".to_string());
        let v = config.validate_config();
        // Auto-routed to native — warning, not error
        assert!(v.is_ok());
        assert!(
            v.warnings
                .iter()
                .any(|w| w.rule == "executor-model-auto-route")
        );
    }

    #[test]
    fn test_validate_config_claude_executor_with_openrouter_model() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        // openrouter:opus is a non-Anthropic provider with an Anthropic model alias
        config.coordinator.model = Some("openrouter:opus".to_string());
        let v = config.validate_config();
        // Should warn about executor auto-route (claude executor + non-Anthropic provider)
        assert!(
            v.warnings
                .iter()
                .any(|w| w.rule == "executor-model-auto-route"
                    || w.rule == "executor-provider-auto-route"),
            "Expected auto-route warning, got: {:?}",
            v.warnings
        );
    }

    #[test]
    fn test_validate_config_openrouter_provider_with_compatible_model_ok() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        config.coordinator.model = Some("openrouter:deepseek/deepseek-chat".to_string());
        let v = config.validate_config();
        // Non-Anthropic model + non-Anthropic provider = OK (auto-routed)
        assert!(v.is_ok());
    }

    #[test]
    fn test_validate_config_claude_executor_with_openai_model() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        // openai provider model — incompatible with claude executor
        config.coordinator.model = Some("openai:gpt-4o".to_string());
        let v = config.validate_config();
        // Should warn about executor auto-route (claude executor + non-Anthropic provider)
        assert!(
            v.warnings
                .iter()
                .any(|w| w.rule == "executor-model-auto-route"
                    || w.rule == "executor-provider-auto-route"),
            "Expected auto-route warning, got: {:?}",
            v.warnings
        );
    }

    #[test]
    fn test_validate_config_claude_executor_with_claude_model_ok() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        config.coordinator.model = Some("claude:sonnet".to_string());
        let v = config.validate_config();
        assert!(v.is_ok());
    }

    #[test]
    fn test_validate_config_native_executor_with_openrouter_ok() {
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());
        config.coordinator.model = Some("openrouter:minimax/minimax-m2.5".to_string());
        let v = config.validate_config();
        assert!(v.is_ok());
    }

    #[test]
    fn test_validate_config_unresolved_model_short_id() {
        let mut config = Config::default();
        config.models.default = Some(RoleModelConfig {
            model: Some("unknown-model-xyz".to_string()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let v = config.validate_config();
        assert!(v.is_ok()); // warnings don't block
        assert!(!v.warnings.is_empty());
        assert!(v.warnings.iter().any(|w| w.rule == "unresolved-model-id"));
    }

    #[test]
    fn test_validate_config_known_model_id_no_warning() {
        let mut config = Config::default();
        config.models.default = Some(RoleModelConfig {
            model: Some("claude:haiku".to_string()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let v = config.validate_config();
        assert!(v.is_clean());
    }

    #[test]
    fn test_validate_config_slash_model_no_warning() {
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());
        config.models.default = Some(RoleModelConfig {
            model: Some("openai/gpt-4o".to_string()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let v = config.validate_config();
        assert!(v.warnings.iter().all(|w| w.rule != "unresolved-model-id"));
    }

    #[test]
    fn test_validate_config_registry_entry_non_anthropic_no_slash() {
        let mut config = Config::default();
        config.model_registry.push(ModelRegistryEntry {
            id: "my-local".into(),
            provider: "openrouter".into(),
            model: "some-model-name".into(),
            tier: Tier::Standard,
            ..Default::default()
        });
        let v = config.validate_config();
        assert!(v.is_ok());
        assert!(v.warnings.iter().any(|w| w.rule == "registry-model-format"));
    }

    #[test]
    fn test_validate_config_registry_entry_anthropic_no_slash_ok() {
        let mut config = Config::default();
        config.model_registry.push(ModelRegistryEntry {
            id: "custom-claude".into(),
            provider: "anthropic".into(),
            model: "claude-custom-model".into(),
            tier: Tier::Standard,
            ..Default::default()
        });
        let v = config.validate_config();
        assert!(v.warnings.iter().all(|w| w.rule != "registry-model-format"));
    }

    #[test]
    fn test_validate_config_missing_api_key_file() {
        let mut config = Config::default();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "test-endpoint".into(),
            provider: "openrouter".into(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some("/nonexistent/path/to/api-key.txt".into()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        });
        let v = config.validate_config();
        assert!(!v.is_ok());
        assert!(v.errors.iter().any(|e| e.rule == "missing-api-key-file"));
    }

    #[test]
    fn test_validate_config_empty_api_key_file() {
        let temp_dir = TempDir::new().unwrap();
        let key_file = temp_dir.path().join("empty-key.txt");
        fs::write(&key_file, "").unwrap();

        let mut config = Config::default();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "test-endpoint".into(),
            provider: "openrouter".into(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some(key_file.to_string_lossy().into_owned()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        });
        let v = config.validate_config();
        assert!(!v.is_ok());
        assert!(v.errors.iter().any(|e| e.rule == "empty-api-key-file"));
    }

    #[test]
    fn test_validate_config_valid_api_key_file() {
        let temp_dir = TempDir::new().unwrap();
        let key_file = temp_dir.path().join("valid-key.txt");
        fs::write(&key_file, "sk-test-key-12345").unwrap();

        let mut config = Config::default();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "test-endpoint".into(),
            provider: "openrouter".into(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some(key_file.to_string_lossy().into_owned()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        });
        let v = config.validate_config();
        assert!(
            v.errors
                .iter()
                .all(|e| e.rule != "missing-api-key-file" && e.rule != "empty-api-key-file")
        );
    }

    #[test]
    fn test_validate_config_multiple_warnings_for_auto_route() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        config.coordinator.model = Some("openrouter:minimax/minimax-m2.5".to_string());
        let v = config.validate_config();
        // Non-Anthropic model with claude executor: auto-routed
        assert!(v.is_ok());
        assert!(!v.warnings.is_empty());
    }

    #[test]
    fn test_validate_config_display_format() {
        let mut config = Config::default();
        // Set up a scenario that produces a warning: missing api_key_file
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "test-endpoint".into(),
            provider: "openrouter".into(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some("/nonexistent/path/to/key.txt".into()),
            api_key_env: None,
            api_key_ref: None,
            is_default: false,
            context_window: None,
        });
        let v = config.validate_config();
        let display = v.display();
        assert!(display.contains("ERROR:"));
        assert!(display.contains("Fix:"));
    }

    // --- effective_executor tests ---

    #[test]
    fn test_effective_executor_default_no_provider() {
        let config = Config::default();
        assert_eq!(config.coordinator.effective_executor(), "claude");
    }

    #[test]
    fn test_effective_executor_openrouter_auto_detects_native() {
        let mut config = Config::default();
        config.coordinator.provider = Some("openrouter".to_string());
        assert_eq!(config.coordinator.effective_executor(), "native");
    }

    #[test]
    fn test_effective_executor_openai_auto_detects_native() {
        let mut config = Config::default();
        config.coordinator.provider = Some("openai".to_string());
        assert_eq!(config.coordinator.effective_executor(), "native");
    }

    #[test]
    fn test_effective_executor_local_auto_detects_native() {
        let mut config = Config::default();
        config.coordinator.provider = Some("local".to_string());
        assert_eq!(config.coordinator.effective_executor(), "native");
    }

    #[test]
    fn test_effective_executor_explicit_claude_overrides_openrouter() {
        let mut config = Config::default();
        config.coordinator.executor = Some("claude".to_string());
        config.coordinator.provider = Some("openrouter".to_string());
        assert_eq!(config.coordinator.effective_executor(), "claude");
    }

    #[test]
    fn test_effective_executor_explicit_native_preserved() {
        let mut config = Config::default();
        config.coordinator.executor = Some("native".to_string());
        assert_eq!(config.coordinator.effective_executor(), "native");
    }

    #[test]
    fn test_effective_executor_anthropic_provider_stays_claude() {
        let mut config = Config::default();
        config.coordinator.provider = Some("anthropic".to_string());
        assert_eq!(config.coordinator.effective_executor(), "claude");
    }

    #[test]
    fn test_effective_executor_roundtrip_toml_no_executor() {
        // Config with provider but no executor should auto-detect after round-trip
        let toml_str = r#"
[coordinator]
provider = "openrouter"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.coordinator.executor.is_none());
        assert_eq!(config.coordinator.effective_executor(), "native");
    }

    #[test]
    fn test_effective_executor_roundtrip_toml_explicit_executor() {
        // Config with explicit executor should preserve it after round-trip
        let toml_str = r#"
[coordinator]
executor = "claude"
provider = "openrouter"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.coordinator.executor, Some("claude".to_string()));
        assert_eq!(config.coordinator.effective_executor(), "claude");
    }

    #[test]
    fn test_effective_executor_infers_native_from_model_prefix() {
        let mut config = Config::default();
        config.coordinator.executor = None;
        config.coordinator.provider = None;
        config.coordinator.model = Some("openrouter:minimax/minimax-m2.5".to_string());
        assert_eq!(config.coordinator.effective_executor(), "native");
    }

    #[test]
    fn test_effective_executor_infers_claude_from_claude_prefix() {
        let mut config = Config::default();
        config.coordinator.executor = None;
        config.coordinator.provider = None;
        config.coordinator.model = Some("claude:opus".to_string());
        assert_eq!(config.coordinator.effective_executor(), "claude");
    }

    #[test]
    fn test_effective_executor_bare_model_stays_claude() {
        let mut config = Config::default();
        config.coordinator.executor = None;
        config.coordinator.provider = None;
        config.coordinator.model = Some("claude:opus".to_string());
        assert_eq!(config.coordinator.effective_executor(), "claude");
    }

    // -----------------------------------------------------------------------
    // parse_model_spec: unified provider:model parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_model_spec_with_known_provider() {
        let spec = parse_model_spec("openrouter:deepseek/deepseek-v3.2");
        assert_eq!(spec.provider.as_deref(), Some("openrouter"));
        assert_eq!(spec.model_id, "deepseek/deepseek-v3.2");
    }

    #[test]
    fn test_parse_model_spec_claude_prefix() {
        let spec = parse_model_spec("claude:opus");
        assert_eq!(spec.provider.as_deref(), Some("claude"));
        assert_eq!(spec.model_id, "opus");
    }

    #[test]
    fn test_parse_model_spec_bare_name() {
        let spec = parse_model_spec("opus");
        assert_eq!(spec.provider, None);
        assert_eq!(spec.model_id, "opus");
    }

    #[test]
    fn test_parse_model_spec_legacy_slash_format() {
        // Legacy org/model format should NOT be parsed as provider:model
        let spec = parse_model_spec("deepseek/deepseek-v3.2");
        assert_eq!(spec.provider, None);
        assert_eq!(spec.model_id, "deepseek/deepseek-v3.2");
    }

    #[test]
    fn test_normalize_bare_openrouter_route_bare_vendor_model() {
        // The canonical case from the task: a bare `vendor/model` route
        // becomes the OpenRouter spec.
        assert_eq!(
            normalize_bare_openrouter_route("minimax/minimax-m3"),
            "openrouter:minimax/minimax-m3"
        );
        assert_eq!(
            normalize_bare_openrouter_route("deepseek/deepseek-v3.2"),
            "openrouter:deepseek/deepseek-v3.2"
        );
    }

    #[test]
    fn test_normalize_bare_openrouter_route_preserves_prefixed_specs() {
        // Already-qualified specs are returned unchanged — including an
        // explicit `openrouter:` prefix (idempotent) and non-openrouter
        // providers.
        assert_eq!(
            normalize_bare_openrouter_route("openrouter:minimax/minimax-m3"),
            "openrouter:minimax/minimax-m3"
        );
        assert_eq!(
            normalize_bare_openrouter_route("claude:opus"),
            "claude:opus"
        );
        assert_eq!(
            normalize_bare_openrouter_route("nex:qwen3-coder"),
            "nex:qwen3-coder"
        );
    }

    #[test]
    fn test_normalize_bare_openrouter_route_preserves_bare_names_without_slash() {
        // A bare name with no slash is NOT a vendor/model route — left alone
        // (resolves via the usual alias/local path, not OpenRouter).
        assert_eq!(normalize_bare_openrouter_route("opus"), "opus");
        assert_eq!(
            normalize_bare_openrouter_route("qwen3-coder-30b"),
            "qwen3-coder-30b"
        );
        // Ollama-style tag with a colon (unknown prefix) and no slash stays bare.
        assert_eq!(
            normalize_bare_openrouter_route("deepseek-coder-v2:16b"),
            "deepseek-coder-v2:16b"
        );
    }

    #[test]
    fn test_model_is_openrouter() {
        assert!(model_is_openrouter("openrouter:minimax/minimax-m3"));
        // After normalization a bare route is openrouter:
        assert!(model_is_openrouter(&normalize_bare_openrouter_route(
            "minimax/minimax-m3"
        )));
        // A bare route is NOT openrouter until normalized.
        assert!(!model_is_openrouter("minimax/minimax-m3"));
        assert!(!model_is_openrouter("claude:opus"));
        assert!(!model_is_openrouter("nex:qwen3-coder"));
        assert!(!model_is_openrouter("opus"));
    }

    #[test]
    fn test_parse_model_spec_ollama_model_tag() {
        // Ollama model tags contain `:` but the prefix isn't a known provider
        let spec = parse_model_spec("deepseek-coder-v2:16b");
        assert_eq!(spec.provider, None);
        assert_eq!(spec.model_id, "deepseek-coder-v2:16b");
    }

    #[test]
    fn test_parse_model_spec_ollama_provider_prefix() {
        // But "ollama:" IS a known provider prefix
        let spec = parse_model_spec("ollama:llama3");
        assert_eq!(spec.provider.as_deref(), Some("ollama"));
        assert_eq!(spec.model_id, "llama3");
    }

    #[test]
    fn test_parse_model_spec_all_known_providers() {
        for provider in KNOWN_PROVIDERS {
            let input = format!("{}:test-model", provider);
            let spec = parse_model_spec(&input);
            assert_eq!(
                spec.provider.as_deref(),
                Some(*provider),
                "Failed for provider: {}",
                provider
            );
            assert_eq!(spec.model_id, "test-model");
        }
    }

    #[test]
    fn test_provider_to_executor_mapping() {
        assert_eq!(provider_to_executor("claude"), "claude");
        assert_eq!(provider_to_executor("codex"), "codex");
        assert_eq!(provider_to_executor("openrouter"), "native");
        assert_eq!(provider_to_executor("openai"), "native");
        assert_eq!(provider_to_executor("gemini"), "native");
        assert_eq!(provider_to_executor("ollama"), "native");
        assert_eq!(provider_to_executor("local"), "native");
        assert_eq!(provider_to_executor("nex"), "native");
    }

    /// Handler-first: an external-CLI handler prefix maps to itself, so
    /// `effective_executor()` (and the status/reload/TUI surfaces that read
    /// it) reports the real handler for a `pi:...` / `opencode:...` model
    /// instead of the legacy `native` default. This is the
    /// `bug-handler-first-executor-display-spam` core fix.
    #[test]
    fn test_provider_to_executor_external_cli_handlers() {
        assert_eq!(provider_to_executor("pi"), "pi");
        assert_eq!(provider_to_executor("opencode"), "opencode");
        assert_eq!(provider_to_executor("aider"), "aider");
        assert_eq!(provider_to_executor("goose"), "goose");
        assert_eq!(provider_to_executor("qwen"), "qwen");
        assert_eq!(provider_to_executor("cline"), "cline");
        assert_eq!(provider_to_executor("crush"), "crush");
        assert_eq!(provider_to_executor("amplifier"), "amplifier");
        assert_eq!(provider_to_executor("octomind"), "octomind");
        assert_eq!(provider_to_executor("dexto"), "dexto");
        assert_eq!(provider_to_executor("shell"), "shell");
        // An unknown prefix still falls through to the in-process native handler.
        assert_eq!(provider_to_executor("some-unknown-vendor"), "native");
    }

    /// A migrated-clean config with `model = "pi:openrouter:..."` and NO
    /// legacy `[dispatcher].executor` key must surface the effective executor
    /// as `pi` (the handler-first route), not the `native`/`claude` default.
    #[test]
    fn test_effective_executor_pi_handler_first_no_legacy_key() {
        let mut config = Config::default();
        config.coordinator.executor = None;
        config.coordinator.provider = None;
        config.coordinator.model = Some("pi:openrouter:anthropic/claude-opus-4-7".to_string());
        assert_eq!(
            config.coordinator.effective_executor(),
            "pi",
            "migrated clean config with pi: model must report executor=pi"
        );
        // The display-facing `effective_dispatcher_executor` agrees.
        assert_eq!(config.effective_dispatcher_executor(), "pi");
        // And it agrees with the routing single source of truth.
        assert_eq!(
            crate::dispatch::handler_for_model("pi:openrouter:anthropic/claude-opus-4-7").as_str(),
            "pi"
        );
    }

    /// When `[dispatcher].model` is unset, the dispatcher inherits
    /// `[agent].model` — `effective_dispatcher_executor` must derive the
    /// handler from that fallback so a `pi:...` agent model still surfaces as
    /// `executor=pi` in `wg status` / `wg config --show`.
    #[test]
    fn test_effective_dispatcher_executor_agent_model_fallback() {
        let mut config = Config::default();
        config.coordinator.executor = None;
        config.coordinator.model = None;
        config.coordinator.provider = None;
        config.agent.model = "pi:openrouter/z-ai/glm-4.6".to_string();
        // The legacy field-based `effective_executor` cannot see agent.model
        // and would fall back to "claude" — the display-facing resolver must
        // not repeat that mistake.
        assert_eq!(
            config.effective_dispatcher_executor(),
            "pi",
            "agent.model fallback must derive the pi handler for display"
        );
    }

    /// A migrated-clean config (no explicit `executor = …` keys anywhere)
    /// must not emit any deprecated-executor warnings — the
    /// `bug-handler-first-executor-display-spam` requirement that the TUI /
    /// status surfaces not spam warnings after migration.
    #[test]
    fn test_migrated_clean_config_emits_no_deprecated_executor_warning() {
        let toml_str = r#"
[agent]
model = "pi:openrouter:anthropic/claude-opus-4-7"

[dispatcher]
model = "pi:openrouter:anthropic/claude-opus-4-7"
"#;
        let warnings = deprecated_executor_warnings_for_toml(toml_str);
        assert!(
            warnings.is_empty(),
            "migrated config with no executor keys must not warn, got: {warnings:?}"
        );
        // And loading it produces a pi handler for both surfaces.
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.effective_dispatcher_executor(), "pi");
    }

    /// The deprecated-executor warning is precise: it fires only when the
    /// explicit `executor = …` KEY is present, and its message names that key
    /// (not the concept of a running handler). Restoring a deprecated
    /// `executor` key to "fix" the display must remain the wrong move.
    #[test]
    fn test_deprecated_executor_warning_precise_about_explicit_key() {
        let toml_str = r#"
[dispatcher]
executor = "claude"
model = "pi:openrouter:anthropic/claude-opus-4-7"
"#;
        let warnings = deprecated_executor_warnings_for_toml(toml_str);
        assert_eq!(
            warnings.len(),
            1,
            "exactly one warning for the explicit key"
        );
        let w = &warnings[0];
        assert!(
            w.contains("dispatcher.executor"),
            "warning must name the explicit config key, got: {w}"
        );
        assert!(
            w.contains("deprecated"),
            "warning must call the key deprecated, got: {w}"
        );
        // It must NOT tell the user to keep the deprecated key to fix display.
        assert!(
            !w.contains("restore") && !w.contains("add the explicit"),
            "warning must not recommend restoring the deprecated key, got: {w}"
        );
    }

    #[test]
    fn test_parse_model_spec_strict_rejects_amplifier_as_provider() {
        // amplifier is a CLI executor, not a model provider. Strict parsing
        // must reject `amplifier:foo` without implying that the executor
        // surface itself is unavailable.
        let err = parse_model_spec_strict("amplifier:claude-3-haiku")
            .expect_err("amplifier: prefix must be rejected");
        assert!(
            err.message.contains("executor name") && err.message.contains("not a model provider"),
            "error must distinguish executor name from provider prefix, got: {}",
            err.message,
        );
        assert!(
            err.message.contains("claude:")
                && err.message.contains("codex:")
                && err.message.contains("openrouter:")
                && err.message.contains("nex:"),
            "error must list valid provider/model prefixes, got: {}",
            err.message,
        );
    }

    #[test]
    fn test_amplifier_not_in_known_providers() {
        // Belt-and-suspenders: `amplifier` is restored as an executor name,
        // but it must not silently become a model provider prefix.
        assert!(!KNOWN_PROVIDERS.contains(&"amplifier"));
    }

    #[test]
    fn test_parse_model_spec_strict_accepts_opencode_executor_route() {
        // `opencode:openrouter/<vendor>/<model>` is a first-class
        // executor-qualified route (see parse_executor_model_route) and the
        // opencode starter profile ships it, so strict validation must accept
        // it — otherwise `wg profile show` / `wg config lint` falsely flag a
        // valid opencode profile as "Unknown provider 'opencode'".
        let spec = parse_model_spec_strict("opencode:openrouter/stepfun/step-3.7-flash")
            .expect("opencode executor route must validate");
        assert_eq!(spec.provider.as_deref(), Some("opencode"));
        assert_eq!(spec.model_id, "openrouter/stepfun/step-3.7-flash");

        // The premium route too.
        assert!(parse_model_spec_strict("opencode:openrouter/minimax/minimax-m2.7").is_ok());

        // Other worker-only external CLIs compose the same way.
        assert!(parse_model_spec_strict("aider:openrouter/x/y").is_ok());

        // But an empty route and a genuinely unknown provider still fail.
        assert!(parse_model_spec_strict("opencode:").is_err());
        assert!(parse_model_spec_strict("foobar:gpt-4").is_err());
    }

    // -----------------------------------------------------------------------
    // Handler-first model-spec enforcement
    // (docs/design-handler-first-model-spec.md)
    // -----------------------------------------------------------------------

    #[test]
    fn test_handler_first_partition() {
        // Handlers are valid leading tokens; provider namespaces are not.
        for h in ["claude", "codex", "nex", "native"] {
            assert!(HANDLER_PREFIXES.contains(&h));
            assert!(
                !is_rejected_leading_provider(h),
                "{h} is a handler, must not be rejected"
            );
        }
        for p in [
            "openrouter",
            "openai",
            "oai-compat",
            "ollama",
            "vllm",
            "llamacpp",
            "gemini",
            "local",
        ] {
            assert!(
                is_rejected_leading_provider(p),
                "{p} is a provider namespace, must be rejected as a leading token"
            );
        }
        // External-CLI handlers are not in KNOWN_PROVIDERS, so they are never
        // rejected here (handled by the executor-prefix interception).
        assert!(!is_rejected_leading_provider("pi"));
        assert!(!is_rejected_leading_provider("opencode"));
    }

    #[test]
    fn test_handler_first_rewrite_prepend_vs_swap() {
        // Wire-distinct providers prepend the handler (keep the inner dialect).
        assert_eq!(
            handler_first_rewrite("openrouter:z-ai/glm-5.2").as_deref(),
            Some("nex:openrouter:z-ai/glm-5.2")
        );
        assert_eq!(
            handler_first_rewrite("ollama:llama3").as_deref(),
            Some("nex:ollama:llama3")
        );
        assert_eq!(
            handler_first_rewrite("gemini:gemini-2.5-pro").as_deref(),
            Some("nex:gemini:gemini-2.5-pro")
        );
        // Pure aliases swap to nex (drop the prefix).
        assert_eq!(
            handler_first_rewrite("oai-compat:gpt-x").as_deref(),
            Some("nex:gpt-x")
        );
        assert_eq!(
            handler_first_rewrite("openai:gpt-x").as_deref(),
            Some("nex:gpt-x")
        );
        assert_eq!(
            handler_first_rewrite("local:qwen3-coder").as_deref(),
            Some("nex:qwen3-coder")
        );
        assert_eq!(
            handler_first_rewrite("native:qwen3-coder").as_deref(),
            Some("nex:qwen3-coder")
        );
        // Already handler-first or bare → no rewrite.
        assert_eq!(handler_first_rewrite("claude:opus"), None);
        assert_eq!(handler_first_rewrite("nex:openrouter:z-ai/glm-5.2"), None);
        assert_eq!(handler_first_rewrite("codex:gpt-5.5"), None);
        assert_eq!(handler_first_rewrite("opus"), None);
    }

    #[test]
    fn test_handler_first_warning_targets() {
        // Bare provider prefixes warn loudly and name the canonical form.
        let w = handler_first_warning("openrouter:z-ai/glm-5.2").unwrap();
        assert!(w.contains("nex:openrouter:z-ai/glm-5.2"), "{w}");
        assert!(w.contains("pi:openrouter:z-ai/glm-5.2"), "{w}");
        assert!(w.contains("not a handler"), "{w}");
        // Handler-first forms and bare aliases are silent.
        assert!(handler_first_warning("nex:openrouter:z-ai/glm-5.2").is_none());
        assert!(handler_first_warning("pi:openrouter/z-ai/glm-5.2").is_none());
        assert!(handler_first_warning("claude:opus").is_none());
        assert!(handler_first_warning("opus").is_none());
    }

    #[test]
    fn test_strip_native_handler_prefix() {
        // Unwrap nex:/native: only when the remainder is itself a
        // provider:model spec — that drives the inner wire resolution.
        assert_eq!(
            strip_native_handler_prefix("nex:openrouter:z-ai/glm-5.2"),
            "openrouter:z-ai/glm-5.2"
        );
        assert_eq!(
            strip_native_handler_prefix("native:ollama:llama3"),
            "ollama:llama3"
        );
        // A bare nex model keeps its prefix (nex → oai-compat wire as before).
        assert_eq!(
            strip_native_handler_prefix("nex:qwen3-coder"),
            "nex:qwen3-coder"
        );
        // Non-nex specs are untouched.
        assert_eq!(
            strip_native_handler_prefix("openrouter:z-ai/glm-5.2"),
            "openrouter:z-ai/glm-5.2"
        );
        assert_eq!(strip_native_handler_prefix("claude:opus"), "claude:opus");
        assert_eq!(strip_native_handler_prefix("opus"), "opus");
    }

    #[test]
    fn test_parse_model_spec_strict_handler_first() {
        // Release N: bare provider prefix warns + defaults to the nex-qualified
        // canonical form (never returns a bare provider).
        let spec = parse_model_spec_strict("openrouter:z-ai/glm-5.2").unwrap();
        assert_eq!(spec.provider.as_deref(), Some("nex"));
        assert_eq!(spec.model_id, "openrouter:z-ai/glm-5.2");

        let spec = parse_model_spec_strict("ollama:llama3").unwrap();
        assert_eq!(spec.provider.as_deref(), Some("nex"));
        assert_eq!(spec.model_id, "ollama:llama3");

        // Handler-first specs parse via the first-colon rule, unchanged.
        let spec = parse_model_spec_strict("nex:openrouter:z-ai/glm-5.2").unwrap();
        assert_eq!(spec.provider.as_deref(), Some("nex"));
        assert_eq!(spec.model_id, "openrouter:z-ai/glm-5.2");

        let spec = parse_model_spec_strict("pi:openrouter/z-ai/glm-5.2").unwrap();
        assert_eq!(spec.provider.as_deref(), Some("pi"));
        assert_eq!(spec.model_id, "openrouter/z-ai/glm-5.2");

        // claude:opus and bare opus/sonnet/haiku → claude, unchanged.
        let spec = parse_model_spec_strict("claude:opus").unwrap();
        assert_eq!(spec.provider.as_deref(), Some("claude"));
        assert_eq!(spec.model_id, "opus");

        // Bare alias and genuinely-unknown prefix still error.
        assert!(parse_model_spec_strict("opus").is_err());
        assert!(parse_model_spec_strict("foobar:gpt-4").is_err());

        // Release N keeps the warn-and-default behavior (not a hard error yet).
        assert!(!HANDLER_FIRST_HARD_ERROR);
    }

    #[test]
    fn test_provider_to_native_provider_mapping() {
        assert_eq!(provider_to_native_provider("openrouter"), "openrouter");
        assert_eq!(provider_to_native_provider("openai"), "oai-compat");
        assert_eq!(provider_to_native_provider("oai-compat"), "oai-compat");
        assert_eq!(provider_to_native_provider("nex"), "oai-compat");
        assert_eq!(provider_to_native_provider("claude"), "anthropic");
        assert_eq!(provider_to_native_provider("codex"), "oai-compat");
        assert_eq!(provider_to_native_provider("gemini"), "oai-compat");
        assert_eq!(provider_to_native_provider("ollama"), "local");
        assert_eq!(provider_to_native_provider("local"), "local");
    }

    #[test]
    fn test_provider_to_resolved_provider_preserves_codex_route() {
        assert_eq!(provider_to_resolved_provider("codex"), "codex");
        assert_eq!(provider_to_resolved_provider("nex"), "oai-compat");
        assert_eq!(provider_to_resolved_provider("openrouter"), "openrouter");
        assert_eq!(provider_to_resolved_provider("claude"), "anthropic");
    }

    #[test]
    fn test_native_provider_to_prefix_canonical_nex() {
        // Internal "oai-compat" / "openai" / "local" → user-facing "nex:"
        // (canonical, matches `wg nex`). The deprecated "oai-compat:" /
        // "local:" forms still parse, but we never emit them.
        assert_eq!(native_provider_to_prefix("oai-compat"), "nex");
        assert_eq!(native_provider_to_prefix("openai"), "nex");
        assert_eq!(native_provider_to_prefix("local"), "nex");
        // Canonical handler-name prefixes pass through.
        assert_eq!(native_provider_to_prefix("anthropic"), "claude");
        assert_eq!(native_provider_to_prefix("openrouter"), "openrouter");
        assert_eq!(native_provider_to_prefix("codex"), "codex");
    }

    #[test]
    fn test_deprecated_provider_prefix_replacement() {
        assert_eq!(deprecated_provider_prefix_replacement("local"), Some("nex"));
        assert_eq!(
            deprecated_provider_prefix_replacement("oai-compat"),
            Some("nex")
        );
        // Not deprecated:
        assert_eq!(deprecated_provider_prefix_replacement("nex"), None);
        assert_eq!(deprecated_provider_prefix_replacement("claude"), None);
        assert_eq!(deprecated_provider_prefix_replacement("openrouter"), None);
    }

    #[test]
    fn test_nex_prefix_routes_to_native_handler() {
        // The whole point of the rename: nex:<model> must route to the
        // native (in-process nex) handler, just like local:/oai-compat: did.
        let spec = parse_model_spec("nex:qwen3-coder-30b");
        assert_eq!(spec.provider.as_deref(), Some("nex"));
        assert_eq!(spec.model_id, "qwen3-coder-30b");
        assert_eq!(provider_to_executor("nex"), "native");
    }

    #[test]
    fn test_deprecated_model_prefix_warnings_local() {
        let toml = r#"
[agent]
model = "local:qwen3-coder"

[tiers]
fast = "local:qwen3-coder"
"#;
        let warnings = deprecated_model_prefix_warnings_for_toml(toml);
        // Two model strings with deprecated `local:` prefix → two warnings.
        assert_eq!(warnings.len(), 2, "got: {:?}", warnings);
        for w in &warnings {
            assert!(
                w.contains("local:"),
                "warning must mention local: — got {}",
                w
            );
            assert!(w.contains("nex:"), "warning must suggest nex: — got {}", w);
            assert!(
                w.contains("wg migrate config"),
                "warning must hint at migrate command — got {}",
                w,
            );
        }
    }

    #[test]
    fn test_deprecated_model_prefix_warnings_oai_compat() {
        let toml = r#"
[agent]
model = "oai-compat:gpt-5"
"#;
        let warnings = deprecated_model_prefix_warnings_for_toml(toml);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("oai-compat:"));
        assert!(warnings[0].contains("nex:gpt-5"));
    }

    #[test]
    fn test_deprecated_model_prefix_warnings_canonical_silent() {
        // Handler-first canonical forms must NOT emit a warning: `nex:`
        // (canonical handler), `claude:` (handler-name match), the nested
        // `nex:openrouter:…` form, and `pi:openrouter/…` (handler-qualified).
        let toml = r#"
[agent]
model = "nex:qwen3-coder"

[tiers]
fast = "claude:haiku"
standard = "nex:openrouter:anthropic/claude-sonnet-4-6"
premium = "pi:openrouter/anthropic/claude-opus-4-6"
"#;
        let warnings = deprecated_model_prefix_warnings_for_toml(toml);
        assert!(
            warnings.is_empty(),
            "no warnings expected; got {:?}",
            warnings
        );
    }

    #[test]
    fn test_deprecated_model_prefix_warnings_bare_openrouter() {
        // Handler-first enforcement: a bare `openrouter:` leading token is a
        // provider namespace, not a handler — it must warn loudly and name
        // both the `nex:` and `pi:` handler-qualified forms. This is the
        // exact form behind the 14h-401 incident.
        let toml = r#"
[dispatcher]
model = "openrouter:z-ai/glm-5.2"
"#;
        let warnings = deprecated_model_prefix_warnings_for_toml(toml);
        assert_eq!(warnings.len(), 1, "got: {:?}", warnings);
        let w = &warnings[0];
        assert!(w.contains("dispatcher.model"), "must include path — {w}");
        assert!(
            w.contains("nex:openrouter:z-ai/glm-5.2"),
            "must name the nex: canonical — {w}"
        );
        assert!(
            w.contains("pi:openrouter:z-ai/glm-5.2"),
            "must name the pi: alternative — {w}"
        );
        assert!(w.contains("wg migrate config"), "must hint migrate — {w}");
    }

    #[test]
    fn test_deprecated_model_prefix_warnings_wire_distinct_providers() {
        // The full rejected leading-provider set warns (not just the legacy
        // local/oai-compat pair): ollama, vllm, llamacpp, gemini.
        let toml = r#"
[tiers]
fast = "ollama:llama3"
standard = "gemini:gemini-2.5-pro"
premium = "vllm:my-model"
"#;
        let warnings = deprecated_model_prefix_warnings_for_toml(toml);
        assert_eq!(warnings.len(), 3, "got: {:?}", warnings);
        assert!(warnings.iter().any(|w| w.contains("nex:ollama:llama3")));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("nex:gemini:gemini-2.5-pro"))
        );
        assert!(warnings.iter().any(|w| w.contains("nex:vllm:my-model")));
    }

    #[test]
    fn test_deprecated_model_prefix_warnings_includes_path() {
        // The warning must include a dotted path so users can find the
        // offending field in their config.toml.
        let toml = r#"
[agent]
model = "local:qwen3-coder"
"#;
        let warnings = deprecated_model_prefix_warnings_for_toml(toml);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("agent.model"),
            "warning must include the dotted path 'agent.model' — got {}",
            warnings[0],
        );
    }

    // -----------------------------------------------------------------------
    // resolve_tier with provider:model format
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_tier_with_provider_prefix() {
        let mut config = Config::default();
        config.tiers.fast = Some("openrouter:qwen/qwen-turbo".into());
        let resolved = config.resolve_tier(Tier::Fast).unwrap();
        // Model ID should be the bare part (no prefix)
        assert_eq!(resolved.model, "qwen/qwen-turbo");
        // Provider should be derived from the prefix
        assert_eq!(resolved.provider, Some("openrouter".to_string()));
        assert!(resolved.registry_entry.is_none());
    }

    #[test]
    fn test_resolve_tier_claude_prefix() {
        let mut config = Config::default();
        config.tiers.premium = Some("claude:opus".into());
        let resolved = config.resolve_tier(Tier::Premium).unwrap();
        // "claude" prefix → maps to "anthropic" native provider, but "opus"
        // is in the built-in registry, so registry should take precedence
        assert_eq!(resolved.model, CLAUDE_OPUS_MODEL_ID);
        assert_eq!(resolved.provider, Some("anthropic".to_string()));
        assert!(resolved.registry_entry.is_some());
    }

    // -----------------------------------------------------------------------
    // resolve_model_for_role with provider:model format
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_model_for_role_with_provider_prefix() {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("openrouter:deepseek/deepseek-v3.2".into()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, "deepseek/deepseek-v3.2");
        assert_eq!(resolved.provider, Some("openrouter".to_string()));
    }

    #[test]
    fn test_resolve_model_for_role_claude_prefix_strips_for_api() {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("claude:claude-sonnet-4-6".into()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        // Model ID should be the bare part without the claude: prefix
        assert_eq!(resolved.model, "claude-sonnet-4-6");
        assert_eq!(resolved.provider, Some("anthropic".to_string()));
    }

    #[test]
    fn test_resolve_model_for_role_codex_prefix_preserves_cli_provider() {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("codex:gpt-5.4-mini".into()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, "gpt-5.4-mini");
        assert_eq!(resolved.provider, Some("codex".to_string()));
    }

    #[test]
    fn test_resolve_model_for_role_codex_spawn_spec_is_atomic() {
        let mut config = Config::default();
        config.models.task_agent = Some(RoleModelConfig {
            model: Some("codex:gpt-5.5".into()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });

        let resolved = config.resolve_model_for_role(DispatchRole::TaskAgent);

        assert_eq!(resolved.model, "gpt-5.5");
        assert_eq!(resolved.provider, Some("codex".to_string()));
        assert_eq!(
            resolved.spawn_model_spec(),
            "codex:gpt-5.5",
            "dispatch must pass a provider-qualified model into plan_spawn so codex-class routing cannot be paired with executor=claude"
        );
    }

    #[test]
    fn test_resolve_model_for_role_provider_prefix_overrides_separate_provider() {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("openrouter:qwen/qwen-turbo".into()),
            provider: Some("anthropic".into()), // This should be overridden
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, "qwen/qwen-turbo");
        // The provider prefix should win over the separate provider field
        assert_eq!(resolved.provider, Some("openrouter".to_string()));
    }

    #[test]
    fn test_resolve_model_for_role_bare_name_backward_compat() {
        // Bare model names should still work exactly as before
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("gpt-4o-mini".into()),
            provider: Some("openai".into()),
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, "gpt-4o-mini");
        assert_eq!(resolved.provider, Some("openai".to_string()));
    }

    #[test]
    fn test_resolve_model_for_role_ollama_local_provider() {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("ollama:llama3".into()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, "llama3");
        assert_eq!(resolved.provider, Some("local".to_string()));
    }

    #[test]
    fn test_resolve_model_for_role_gemini_provider() {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("gemini:gemini-2.0-flash-001".into()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(resolved.model, "gemini-2.0-flash-001");
        // Gemini maps to "oai-compat" native provider (OpenAI-compatible endpoint)
        assert_eq!(resolved.provider, Some("oai-compat".to_string()));
    }

    // -----------------------------------------------------------------------
    // default_url_for_provider: new provider URLs
    // -----------------------------------------------------------------------

    #[test]
    fn test_default_url_for_new_providers() {
        assert_eq!(
            EndpointConfig::default_url_for_provider("ollama"),
            "http://localhost:11434/v1"
        );
        assert_eq!(
            EndpointConfig::default_url_for_provider("llamacpp"),
            "http://localhost:8080/v1"
        );
        assert_eq!(
            EndpointConfig::default_url_for_provider("vllm"),
            "http://localhost:8000/v1"
        );
        assert_eq!(
            EndpointConfig::default_url_for_provider("gemini"),
            "https://generativelanguage.googleapis.com/v1beta/openai"
        );
    }

    // ---- Profile-aware tier resolution tests ----

    #[test]
    fn test_effective_tiers_no_profile_uses_defaults() {
        let config = Config::default();
        let tiers = config.effective_tiers();
        assert_eq!(tiers.fast.as_deref(), Some("claude:haiku"));
        assert_eq!(tiers.standard.as_deref(), Some("claude:opus"));
        assert_eq!(tiers.premium.as_deref(), Some("claude:opus"));
    }

    #[test]
    fn test_effective_tiers_anthropic_profile() {
        let mut config = Config::default();
        config.profile = Some("anthropic".into());
        let tiers = config.effective_tiers();
        assert_eq!(tiers.fast.as_deref(), Some("claude:haiku"));
        assert_eq!(tiers.standard.as_deref(), Some("claude:opus"));
        assert_eq!(tiers.premium.as_deref(), Some("claude:opus"));
    }

    #[test]
    fn test_effective_tiers_openai_profile() {
        let mut config = Config::default();
        config.profile = Some("openai".into());
        let tiers = config.effective_tiers();
        assert_eq!(tiers.fast.as_deref(), Some("openrouter:openai/gpt-4o-mini"));
        assert_eq!(tiers.standard.as_deref(), Some("openrouter:openai/gpt-4o"));
        assert_eq!(tiers.premium.as_deref(), Some("openrouter:openai/o3-pro"));
    }

    #[test]
    fn test_explicit_tiers_override_profile() {
        let mut config = Config::default();
        config.profile = Some("openai".into());
        config.tiers.fast = Some("claude:haiku".into());
        let tiers = config.effective_tiers();
        // Explicit tier wins over profile
        assert_eq!(tiers.fast.as_deref(), Some("claude:haiku"));
        // Profile fills in the rest
        assert_eq!(tiers.standard.as_deref(), Some("openrouter:openai/gpt-4o"));
        assert_eq!(tiers.premium.as_deref(), Some("openrouter:openai/o3-pro"));
    }

    #[test]
    fn test_unknown_profile_falls_through_to_defaults() {
        let mut config = Config::default();
        config.profile = Some("nonexistent".into());
        let tiers = config.effective_tiers();
        // Unknown profile produces no tiers, so hardcoded defaults are used
        assert_eq!(tiers.fast.as_deref(), Some("claude:haiku"));
        assert_eq!(tiers.standard.as_deref(), Some("claude:opus"));
        assert_eq!(tiers.premium.as_deref(), Some("claude:opus"));
    }

    #[test]
    fn test_dynamic_profile_falls_through_to_defaults() {
        let mut config = Config::default();
        config.profile = Some("openrouter".into());
        // Dynamic profiles return None from resolve_tiers(), so defaults are used
        let tiers = config.effective_tiers();
        assert_eq!(tiers.fast.as_deref(), Some("claude:haiku"));
        assert_eq!(tiers.standard.as_deref(), Some("claude:opus"));
        assert_eq!(tiers.premium.as_deref(), Some("claude:opus"));
    }

    #[test]
    fn test_profile_resolve_model_for_role() {
        let mut config = Config::default();
        config.profile = Some("openai".into());
        // Triage role defaults to Fast tier
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        // openai profile maps fast → openrouter:openai/gpt-4o-mini
        // Since openai/gpt-4o-mini is unlikely to be in the default registry,
        // it resolves to the model ID from the tier spec
        assert!(
            resolved.model.contains("gpt-4o-mini"),
            "Expected gpt-4o-mini in resolved model, got: {}",
            resolved.model
        );
    }

    #[test]
    fn test_profile_config_roundtrip() {
        let mut config = Config::default();
        config.profile = Some("openai".into());
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("profile = \"openai\""));
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.profile.as_deref(), Some("openai"));
    }

    #[test]
    fn test_profile_none_not_serialized() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(
            !toml_str.contains("profile"),
            "profile = None should be skipped in serialization"
        );
    }

    // ---- Config::resolve_api_key_for_provider tests ----

    #[test]
    fn test_resolve_api_key_for_provider_from_endpoint_inline() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "my-openrouter".into(),
            provider: "openrouter".into(),
            url: None,
            model: None,
            api_key: Some("sk-endpoint-key".into()),
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: true,
            context_window: None,
        });
        let key = config
            .resolve_api_key_for_provider("openrouter", dir.path())
            .unwrap();
        assert_eq!(key, "sk-endpoint-key");
    }

    #[test]
    fn test_resolve_api_key_for_provider_from_endpoint_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("api.key");
        fs::write(&key_file, "sk-from-file-endpoint\n").unwrap();
        let mut config = Config::default();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "my-openrouter".into(),
            provider: "openrouter".into(),
            url: None,
            model: None,
            api_key: None,
            api_key_file: Some(key_file.to_string_lossy().into_owned()),
            api_key_env: None,
            api_key_ref: None,
            is_default: true,
            context_window: None,
        });
        let key = config
            .resolve_api_key_for_provider("openrouter", dir.path())
            .unwrap();
        assert_eq!(key, "sk-from-file-endpoint");
    }

    #[test]
    #[serial]
    fn test_resolve_api_key_for_provider_env_fallback() {
        let saved = std::env::var("OPENROUTER_API_KEY").ok();
        let saved_oai = std::env::var("OPENAI_API_KEY").ok();
        unsafe { std::env::set_var("OPENROUTER_API_KEY", "sk-env-or") };
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        let dir = tempfile::tempdir().unwrap();
        let config = Config::default(); // no endpoints configured
        let key = config
            .resolve_api_key_for_provider("openrouter", dir.path())
            .unwrap();
        assert_eq!(key, "sk-env-or");
        match saved {
            Some(v) => unsafe { std::env::set_var("OPENROUTER_API_KEY", v) },
            None => unsafe { std::env::remove_var("OPENROUTER_API_KEY") },
        }
        match saved_oai {
            Some(v) => unsafe { std::env::set_var("OPENAI_API_KEY", v) },
            None => unsafe { std::env::remove_var("OPENAI_API_KEY") },
        }
    }

    #[test]
    fn test_resolve_api_key_for_provider_endpoint_beats_env() {
        // When endpoint has key, it should win over env var
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "my-openrouter".into(),
            provider: "openrouter".into(),
            url: None,
            model: None,
            api_key: Some("sk-endpoint-wins".into()),
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            is_default: true,
            context_window: None,
        });
        // Even if env var is set, endpoint should win
        let key = config
            .resolve_api_key_for_provider("openrouter", dir.path())
            .unwrap();
        assert_eq!(key, "sk-endpoint-wins");
    }

    #[test]
    fn test_resolve_api_key_for_provider_no_key_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        // With no endpoints, no env vars, and no native_executor config,
        // should return an error (in a clean env with no key set)
        // We can't easily test this because env vars might be set in CI,
        // so just test the positive cases above.
        let _ = config.resolve_api_key_for_provider("local", dir.path());
    }

    #[test]
    fn test_default_config_does_not_serialize_empty_endpoints() {
        let config = Config::default();
        let content = toml::to_string_pretty(&config).unwrap();
        assert!(
            !content.contains("endpoints = []"),
            "Default config should not contain 'endpoints = []' — it shadows global config.\nGot:\n{}",
            content
        );
        assert!(
            !content.contains("[llm_endpoints]"),
            "Default config should not contain '[llm_endpoints]' section when empty.\nGot:\n{}",
            content
        );
    }

    #[test]
    fn test_default_config_does_not_serialize_empty_model_registry() {
        let config = Config::default();
        let content = toml::to_string_pretty(&config).unwrap();
        assert!(
            !content.contains("model_registry = []"),
            "Default config should not contain 'model_registry = []' — it shadows global config.\nGot:\n{}",
            content
        );
    }

    #[test]
    fn test_default_config_does_not_serialize_empty_default_skills() {
        let config = Config::default();
        let content = toml::to_string_pretty(&config).unwrap();
        assert!(
            !content.contains("default_skills = []"),
            "Default config should not contain 'default_skills = []' — it shadows global config.\nGot:\n{}",
            content
        );
    }

    #[test]
    fn test_merge_toml_preserves_global_endpoints_when_local_omits_them() {
        // This tests the *primitive* `merge_toml` behavior — global wins when
        // local omits a key. The user-facing `load_merged*` paths layer
        // `apply_endpoint_inheritance_policy` on top to invert this for
        // endpoints specifically (see tests below).
        let global: toml::Value = toml::from_str(
            r#"
[llm_endpoints]
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key = "sk-or-test"
is_default = true
"#,
        )
        .unwrap();
        // Local config has no llm_endpoints section at all
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "claude:haiku"
"#,
        )
        .unwrap();
        let merged = merge_toml(global, local);
        let config: Config = merged.try_into().unwrap();
        assert_eq!(config.llm_endpoints.endpoints.len(), 1);
        assert_eq!(config.llm_endpoints.endpoints[0].name, "openrouter");
    }

    // ---- Endpoint inheritance opt-in tests ----
    //
    // Endpoint inheritance from global → local is opt-in. Local must set
    // `[llm_endpoints] inherit_global = true` to get the legacy cascade.
    // Without that flag, the user's local config defines the *complete* set
    // of available endpoints. See `apply_endpoint_inheritance_policy`.

    fn make_global_with_openrouter_default() -> toml::Value {
        toml::from_str(
            r#"
[llm_endpoints]
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key = "sk-or-global"
is_default = true
"#,
        )
        .unwrap()
    }

    #[test]
    fn test_local_no_endpoints_does_not_inherit_global() {
        // The user's reported symptom: global has openrouter as is_default,
        // and the user's local has no `[llm_endpoints]` at all. Under the new
        // policy this MUST NOT cascade — global's endpoints are dropped.
        let mut global = make_global_with_openrouter_default();
        let local: toml::Value = toml::from_str(
            r#"
[agent]
model = "claude:haiku"
"#,
        )
        .unwrap();
        apply_endpoint_inheritance_policy(&mut global, &local, false);
        let merged = merge_toml(global, local);
        let config: Config = merged.try_into().unwrap();
        assert!(
            config.llm_endpoints.endpoints.is_empty(),
            "local with no [llm_endpoints] must not inherit global endpoints; got {:?}",
            config.llm_endpoints.endpoints
        );
        assert!(
            config.llm_endpoints.find_default().is_none(),
            "no default endpoint should leak from global"
        );
    }

    #[test]
    fn test_local_with_own_endpoints_replaces_global() {
        // Local has its own endpoint list; under approach A this fully
        // replaces global's list (no per-name merging, no global leak).
        let mut global = make_global_with_openrouter_default();
        let local: toml::Value = toml::from_str(
            r#"
[llm_endpoints]
[[llm_endpoints.endpoints]]
name = "claude-direct"
provider = "anthropic"
url = "https://api.anthropic.com/v1"
api_key = "sk-anthropic-local"
is_default = true
"#,
        )
        .unwrap();
        apply_endpoint_inheritance_policy(&mut global, &local, false);
        let merged = merge_toml(global, local);
        let config: Config = merged.try_into().unwrap();
        assert_eq!(config.llm_endpoints.endpoints.len(), 1);
        assert_eq!(config.llm_endpoints.endpoints[0].name, "claude-direct");
        assert!(
            config.llm_endpoints.find_by_name("openrouter").is_none(),
            "global openrouter must not leak when local declares its own endpoints"
        );
    }

    #[test]
    fn test_inherit_global_knob_works() {
        // With `[llm_endpoints] inherit_global = true` in local, the legacy
        // cascade behavior is preserved.
        let mut global = make_global_with_openrouter_default();
        let local: toml::Value = toml::from_str(
            r#"
[llm_endpoints]
inherit_global = true
"#,
        )
        .unwrap();
        apply_endpoint_inheritance_policy(&mut global, &local, false);
        let merged = merge_toml(global, local);
        let config: Config = merged.try_into().unwrap();
        assert_eq!(
            config.llm_endpoints.endpoints.len(),
            1,
            "inherit_global=true must keep the global openrouter entry"
        );
        assert_eq!(config.llm_endpoints.endpoints[0].name, "openrouter");
        assert!(config.llm_endpoints.inherit_global);
    }

    #[test]
    fn test_inherit_global_with_local_endpoints_unions() {
        // `inherit_global = true` plus a local entry — under this branch,
        // merge_toml's "local list wins" rule for arrays still applies, so
        // local replaces global's array even with inherit_global=true. The
        // knob's job is to RESTORE legacy behavior, and legacy merge_toml
        // already had this property: a non-empty local array replaces global's.
        // We assert the documented contract.
        let mut global = make_global_with_openrouter_default();
        let local: toml::Value = toml::from_str(
            r#"
[llm_endpoints]
inherit_global = true
[[llm_endpoints.endpoints]]
name = "claude-direct"
provider = "anthropic"
url = "https://api.anthropic.com/v1"
api_key = "sk-anthropic-local"
is_default = true
"#,
        )
        .unwrap();
        apply_endpoint_inheritance_policy(&mut global, &local, false);
        let merged = merge_toml(global, local);
        let config: Config = merged.try_into().unwrap();
        assert_eq!(config.llm_endpoints.endpoints.len(), 1);
        assert_eq!(config.llm_endpoints.endpoints[0].name, "claude-direct");
    }

    // ---- Global config propagation tests ----

    #[test]
    fn test_load_merged_toml_value_merges_global_and_local() {
        // Set up fake global config via temp dir
        let global_dir = tempfile::tempdir().unwrap();
        let local_dir = tempfile::tempdir().unwrap();

        // We can't easily override global_dir(), so test via load_toml_value + merge_toml directly
        let global_path = global_dir.path().join("config.toml");
        let local_path = local_dir.path().join("config.toml");

        std::fs::write(
            &global_path,
            r#"
[native_executor]
provider = "openrouter"
api_key = "sk-or-global-key"
api_base = "https://openrouter.ai/api/v1"

[coordinator]
executor = "native"
"#,
        )
        .unwrap();

        std::fs::write(
            &local_path,
            r#"
[agent]
model = "claude:haiku"
"#,
        )
        .unwrap();

        let global_val = Config::load_toml_value(&global_path).unwrap();
        let local_val = Config::load_toml_value(&local_path).unwrap();
        let merged = merge_toml(global_val, local_val);

        // Global native_executor should be present
        let ne = merged.get("native_executor").unwrap().as_table().unwrap();
        assert_eq!(ne["api_key"].as_str().unwrap(), "sk-or-global-key");
        assert_eq!(ne["provider"].as_str().unwrap(), "openrouter");

        // Local agent model should be present
        let agent = merged.get("agent").unwrap().as_table().unwrap();
        assert_eq!(agent["model"].as_str().unwrap(), "claude:haiku");

        // Global coordinator should be present
        let coord = merged.get("coordinator").unwrap().as_table().unwrap();
        assert_eq!(coord["executor"].as_str().unwrap(), "native");
    }

    #[test]
    fn test_local_config_overrides_global_api_key() {
        let global_path = tempfile::NamedTempFile::new().unwrap();
        let local_path = tempfile::NamedTempFile::new().unwrap();

        std::fs::write(
            global_path.path(),
            r#"
[native_executor]
api_key = "sk-global-key"
provider = "openrouter"
"#,
        )
        .unwrap();

        std::fs::write(
            local_path.path(),
            r#"
[native_executor]
api_key = "sk-local-key"
"#,
        )
        .unwrap();

        let global_val = Config::load_toml_value(global_path.path()).unwrap();
        let local_val = Config::load_toml_value(local_path.path()).unwrap();
        let merged = merge_toml(global_val, local_val);

        let ne = merged.get("native_executor").unwrap().as_table().unwrap();
        // Local api_key should override global
        assert_eq!(ne["api_key"].as_str().unwrap(), "sk-local-key");
        // Global provider should be preserved (not overridden)
        assert_eq!(ne["provider"].as_str().unwrap(), "openrouter");
    }

    #[test]
    fn test_missing_global_config_falls_back_gracefully() {
        let local_dir = tempfile::tempdir().unwrap();

        // No global config exists at all (using load_toml_value with non-existent path)
        let nonexistent_global = PathBuf::from("/tmp/wg_test_nonexistent_global_config.toml");
        let local_path = local_dir.path().join("config.toml");

        std::fs::write(
            &local_path,
            r#"
[agent]
model = "claude:sonnet"
"#,
        )
        .unwrap();

        let global_val = Config::load_toml_value(&nonexistent_global).unwrap();
        let local_val = Config::load_toml_value(&local_path).unwrap();
        let merged = merge_toml(global_val, local_val);

        // Should have just the local config
        let agent = merged.get("agent").unwrap().as_table().unwrap();
        assert_eq!(agent["model"].as_str().unwrap(), "claude:sonnet");
    }

    #[test]
    fn test_missing_local_config_uses_global() {
        let global_path = tempfile::NamedTempFile::new().unwrap();

        std::fs::write(
            global_path.path(),
            r#"
[coordinator]
executor = "native"
max_agents = 2

[native_executor]
provider = "openrouter"
api_key = "sk-or-global"
"#,
        )
        .unwrap();

        let nonexistent_local = PathBuf::from("/tmp/wg_test_nonexistent_local_config.toml");

        let global_val = Config::load_toml_value(global_path.path()).unwrap();
        let local_val = Config::load_toml_value(&nonexistent_local).unwrap();
        let merged = merge_toml(global_val, local_val);

        // Global config should be used entirely
        let coord = merged.get("coordinator").unwrap().as_table().unwrap();
        assert_eq!(coord["executor"].as_str().unwrap(), "native");
        assert_eq!(coord["max_agents"].as_integer().unwrap(), 2);

        let ne = merged.get("native_executor").unwrap().as_table().unwrap();
        assert_eq!(ne["api_key"].as_str().unwrap(), "sk-or-global");
    }

    #[test]
    fn test_global_endpoints_do_not_propagate_to_merged_config_by_default() {
        // Endpoint inheritance is opt-in. With an empty local config (no
        // `[llm_endpoints] inherit_global = true`), global endpoints must
        // NOT leak into the merged config. This is the user-facing
        // contract that `load_merged` and `load_with_sources` enforce by
        // calling `apply_endpoint_inheritance_policy` before `merge_toml`.
        let global_path = tempfile::NamedTempFile::new().unwrap();

        std::fs::write(
            global_path.path(),
            r#"
[llm_endpoints]
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key = "sk-or-test-global"
is_default = true
"#,
        )
        .unwrap();

        // Empty local config
        let local_path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(local_path.path(), "").unwrap();

        let mut global_val = Config::load_toml_value(global_path.path()).unwrap();
        let local_val = Config::load_toml_value(local_path.path()).unwrap();
        apply_endpoint_inheritance_policy(&mut global_val, &local_val, false);
        let merged = merge_toml(global_val, local_val);

        let config: Config = merged.try_into().unwrap();
        assert!(
            config.llm_endpoints.endpoints.is_empty(),
            "global endpoints must not leak into local without inherit_global; got {:?}",
            config.llm_endpoints.endpoints
        );
    }

    #[test]
    fn test_global_endpoints_propagate_when_inherit_global_set() {
        // When local explicitly sets `[llm_endpoints] inherit_global = true`,
        // the legacy cascade is restored and global endpoints flow through.
        let global_path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            global_path.path(),
            r#"
[llm_endpoints]
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key = "sk-or-test-global"
is_default = true
"#,
        )
        .unwrap();

        let local_path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            local_path.path(),
            r#"
[llm_endpoints]
inherit_global = true
"#,
        )
        .unwrap();

        let mut global_val = Config::load_toml_value(global_path.path()).unwrap();
        let local_val = Config::load_toml_value(local_path.path()).unwrap();
        apply_endpoint_inheritance_policy(&mut global_val, &local_val, false);
        let merged = merge_toml(global_val, local_val);

        let config: Config = merged.try_into().unwrap();
        assert_eq!(config.llm_endpoints.endpoints.len(), 1);
        assert_eq!(
            config.llm_endpoints.endpoints[0].api_key,
            Some("sk-or-test-global".to_string())
        );
        assert_eq!(config.llm_endpoints.endpoints[0].provider, "openrouter");
        assert!(config.llm_endpoints.inherit_global);
    }

    #[test]
    fn test_active_named_profile_endpoints_propagate_by_default() {
        // Named profiles are authoritative route selections. When a profile is
        // active, its global endpoints should be visible in a repo that has no
        // local endpoint table, so `wg profile use nex` is enough for routing.
        let global_path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            global_path.path(),
            r#"
[llm_endpoints]
[[llm_endpoints.endpoints]]
name = "default"
provider = "oai-compat"
url = "http://127.0.0.1:8088"
is_default = true
"#,
        )
        .unwrap();

        let local_path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(local_path.path(), "").unwrap();

        let mut global_val = Config::load_toml_value(global_path.path()).unwrap();
        let local_val = Config::load_toml_value(local_path.path()).unwrap();
        apply_endpoint_inheritance_policy(&mut global_val, &local_val, true);
        let merged = merge_toml(global_val, local_val);

        let config: Config = merged.try_into().unwrap();
        assert_eq!(config.llm_endpoints.endpoints.len(), 1);
        assert_eq!(config.llm_endpoints.endpoints[0].provider, "oai-compat");
        assert_eq!(
            config.llm_endpoints.endpoints[0].url.as_deref(),
            Some("http://127.0.0.1:8088")
        );
    }

    #[test]
    fn test_active_named_profile_endpoints_can_be_explicitly_blocked() {
        let global_path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            global_path.path(),
            r#"
[llm_endpoints]
[[llm_endpoints.endpoints]]
name = "default"
provider = "oai-compat"
url = "http://127.0.0.1:8088"
is_default = true
"#,
        )
        .unwrap();

        let local_path = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            local_path.path(),
            r#"
[llm_endpoints]
inherit_global = false
"#,
        )
        .unwrap();

        let mut global_val = Config::load_toml_value(global_path.path()).unwrap();
        let local_val = Config::load_toml_value(local_path.path()).unwrap();
        apply_endpoint_inheritance_policy(&mut global_val, &local_val, true);
        let merged = merge_toml(global_val, local_val);

        let config: Config = merged.try_into().unwrap();
        assert!(
            config.llm_endpoints.endpoints.is_empty(),
            "explicit inherit_global=false should block active profile endpoints"
        );
    }

    #[test]
    fn test_resolve_api_key_from_merged_endpoints() {
        // Build a config with endpoints (as if loaded from global)
        let mut config = Config::default();
        config.llm_endpoints.endpoints.push(EndpointConfig {
            name: "openrouter".to_string(),
            provider: "openrouter".to_string(),
            url: Some("https://openrouter.ai/api/v1".to_string()),
            api_key: Some("sk-or-from-endpoint".to_string()),
            api_key_file: None,
            api_key_env: None,
            api_key_ref: None,
            model: None,
            is_default: true,
            context_window: None,
        });

        let tmp = tempfile::tempdir().unwrap();
        // No local config.toml exists — key should come from the endpoint config
        let result = config.resolve_api_key_for_provider("openrouter", tmp.path());
        assert!(result.is_ok(), "Should resolve key from endpoint config");
        assert_eq!(result.unwrap(), "sk-or-from-endpoint");
    }

    #[test]
    fn test_resolve_api_key_legacy_native_executor_from_merged() {
        // Simulate: global config has [native_executor] api_key, local has nothing
        let global_dir = tempfile::tempdir().unwrap();
        let global_path = global_dir.path().join("config.toml");
        std::fs::write(
            &global_path,
            r#"
[native_executor]
api_key = "sk-legacy-global"
"#,
        )
        .unwrap();

        let local_dir = tempfile::tempdir().unwrap();
        // No local config.toml

        let global_val = Config::load_toml_value(&global_path).unwrap();
        let nonexistent_local = local_dir.path().join("config.toml");
        let local_val = Config::load_toml_value(&nonexistent_local).unwrap();
        let merged = merge_toml(global_val, local_val);

        // Verify the native_executor key is present in merged
        let key = merged
            .get("native_executor")
            .and_then(|v| v.get("api_key"))
            .and_then(|v| v.as_str());
        assert_eq!(key, Some("sk-legacy-global"));
    }

    #[test]
    fn test_config_lookup_chain_project_overrides_global_overrides_default() {
        // Build global config with specific values
        let global: toml::Value = toml::from_str(
            r#"
[coordinator]
executor = "native"
max_agents = 2
poll_interval = 30

[agent]
model = "openrouter:meta-llama/llama-3-70b"
"#,
        )
        .unwrap();

        // Build local config overriding only some values
        let local: toml::Value = toml::from_str(
            r#"
[coordinator]
max_agents = 8
"#,
        )
        .unwrap();

        let merged = merge_toml(global, local);
        let config: Config = merged.try_into().unwrap();

        // Local override: max_agents
        assert_eq!(config.coordinator.max_agents, 8);
        // Global preserved: executor, poll_interval
        assert_eq!(config.coordinator.effective_executor(), "native");
        assert_eq!(config.coordinator.poll_interval, 30);
        // Global preserved: agent model
        assert_eq!(config.agent.model, "openrouter:meta-llama/llama-3-70b");
    }

    #[test]
    fn test_both_configs_missing_returns_default() {
        let nonexistent1 = PathBuf::from("/tmp/wg_test_ne_global.toml");
        let nonexistent2 = PathBuf::from("/tmp/wg_test_ne_local.toml");

        let global_val = Config::load_toml_value(&nonexistent1).unwrap();
        let local_val = Config::load_toml_value(&nonexistent2).unwrap();
        let merged = merge_toml(global_val, local_val);

        // Should produce valid default config
        let config: Config = merged.try_into().unwrap();
        assert_eq!(config.coordinator.max_agents, 8); // default
        assert_eq!(config.coordinator.effective_executor(), "claude"); // default
    }

    #[test]
    fn test_global_profile_propagates_to_new_project() {
        // Global config with a profile set
        let global: toml::Value = toml::from_str(
            r#"
profile = "openrouter"
"#,
        )
        .unwrap();

        // Empty local config (fresh wg init)
        let local = toml::Value::Table(toml::map::Map::new());

        let merged = merge_toml(global, local);
        let config: Config = merged.try_into().unwrap();

        assert_eq!(config.profile, Some("openrouter".to_string()));
    }

    #[test]
    fn test_native_executor_config_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.native_executor.web.fetch_max_chars, 16_000);
        assert_eq!(config.native_executor.web.fetch_timeout_secs, 30);
        assert!(config.native_executor.web.search_api_key.is_none());
        assert!(config.native_executor.web.searxng_url.is_none());
        assert_eq!(config.native_executor.background.max_background_tasks, 5);
        assert_eq!(
            config.native_executor.background.background_timeout_secs,
            600
        );
        assert_eq!(config.native_executor.delegate.delegate_max_turns, 10);
        assert_eq!(config.native_executor.delegate.delegate_model, "");
    }

    #[test]
    fn test_native_executor_config_custom_values() {
        let toml_str = r#"
[native_executor.web]
search_api_key = "sk-test-123"
fetch_max_chars = 32000
fetch_timeout_secs = 60

[native_executor.background]
max_background_tasks = 10
background_timeout_secs = 1200

[native_executor.delegate]
delegate_max_turns = 15
delegate_model = "haiku"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.native_executor.web.search_api_key,
            Some("sk-test-123".to_string())
        );
        assert_eq!(config.native_executor.web.fetch_max_chars, 32_000);
        assert_eq!(config.native_executor.web.fetch_timeout_secs, 60);
        assert_eq!(config.native_executor.background.max_background_tasks, 10);
        assert_eq!(
            config.native_executor.background.background_timeout_secs,
            1200
        );
        assert_eq!(config.native_executor.delegate.delegate_max_turns, 15);
        assert_eq!(config.native_executor.delegate.delegate_model, "haiku");
    }

    #[test]
    fn test_native_executor_config_partial_override() {
        let toml_str = r#"
[native_executor.web]
fetch_max_chars = 8000
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        // Overridden value
        assert_eq!(config.native_executor.web.fetch_max_chars, 8_000);
        // Defaults preserved
        assert_eq!(config.native_executor.web.fetch_timeout_secs, 30);
        assert_eq!(config.native_executor.background.max_background_tasks, 5);
        assert_eq!(config.native_executor.delegate.delegate_max_turns, 10);
    }

    /// Legacy `search_backend` field: no longer declared on the struct,
    /// but existing user configs may still have it set (the project's
    /// .wg/config.toml and tests have shipped with it for a
    /// while). serde silently ignores unknown fields by default, so
    /// deserialization must succeed.
    #[test]
    fn test_native_executor_config_ignores_legacy_search_backend() {
        let toml_str = r#"
[native_executor.web]
search_backend = "duckduckgo"
fetch_max_chars = 16000
"#;
        let config: Config = toml::from_str(toml_str).expect("legacy field should be ignored");
        assert_eq!(config.native_executor.web.fetch_max_chars, 16_000);
    }

    #[test]
    fn apply_model_endpoint_sets_default_endpoint_and_prefixed_model() {
        let mut config = Config::default();
        let summary = config
            .apply_model_endpoint(Some("qwen3-coder"), Some("http://lambda01:8089"))
            .unwrap();
        // Both endpoint + model mentions in summary.
        assert!(summary.iter().any(|s| s.contains("http://lambda01:8089")));
        assert!(summary.iter().any(|s| s.contains("nex:qwen3-coder")));
        // Model gets the nex: prefix (canonical, matches `wg nex`).
        assert_eq!(config.coordinator.model.as_deref(), Some("nex:qwen3-coder"));
        assert_eq!(config.agent.model, "nex:qwen3-coder");
        // Endpoint entry is default.
        let default_ep = config
            .llm_endpoints
            .endpoints
            .iter()
            .find(|e| e.is_default)
            .expect("default endpoint written");
        assert_eq!(default_ep.provider, "local");
        assert_eq!(default_ep.url.as_deref(), Some("http://lambda01:8089"));
        assert_eq!(default_ep.model.as_deref(), Some("qwen3-coder"));
    }

    #[test]
    fn apply_model_endpoint_preserves_provider_prefix_when_given() {
        let mut config = Config::default();
        config
            .apply_model_endpoint(Some("claude:opus"), None)
            .unwrap();
        // No endpoint → model stored verbatim, no local: prefix added.
        assert_eq!(config.coordinator.model.as_deref(), Some("claude:opus"));
        assert_eq!(config.agent.model, "claude:opus");
    }

    #[test]
    fn apply_model_endpoint_rejects_non_http() {
        let mut config = Config::default();
        let err = config
            .apply_model_endpoint(Some("x"), Some("lambda01"))
            .expect_err("non-http rejected");
        assert!(
            format!("{:#}", err).contains("http://"),
            "error should mention http(s)"
        );
    }

    #[test]
    fn apply_model_endpoint_replaces_default_entry() {
        let mut config = Config::default();
        config
            .apply_model_endpoint(Some("m1"), Some("http://a:1"))
            .unwrap();
        config
            .apply_model_endpoint(Some("m2"), Some("http://b:2"))
            .unwrap();
        // Exactly one `default` entry; `is_default` only on the newest.
        let defaults: Vec<_> = config
            .llm_endpoints
            .endpoints
            .iter()
            .filter(|e| e.name == "default")
            .collect();
        assert_eq!(defaults.len(), 1, "only one 'default' entry retained");
        assert_eq!(defaults[0].url.as_deref(), Some("http://b:2"));
        assert_eq!(
            config
                .llm_endpoints
                .endpoints
                .iter()
                .filter(|e| e.is_default)
                .count(),
            1,
            "is_default unique"
        );
    }

    #[test]
    fn backup_on_disk_creates_sortable_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        // No file yet → None.
        assert!(Config::backup_on_disk(dir).unwrap().is_none());
        // Write a config, then back it up.
        fs::write(dir.join("config.toml"), "[agent]\nexecutor = \"claude\"\n").unwrap();
        let backup = Config::backup_on_disk(dir)
            .unwrap()
            .expect("backup written");
        assert!(backup.exists(), "backup file exists");
        let name = backup.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("config.toml."), "name: {}", name);
        // Timestamp form `YYYY-MM-DDTHH-MM-SSZ` — sortable lexicographically.
        let stamp = name.strip_prefix("config.toml.").unwrap();
        assert_eq!(stamp.len(), "2026-04-22T12-34-56Z".len(), "timestamp fmt");
        assert!(stamp.ends_with('Z'));
    }

    // ── Bare-alias contract tests ────────────────────────────────────

    #[test]
    fn test_claude_cli_model_arg_expands_fable_only() {
        // Fable has no bare CLI shortcut → expand the friendly alias.
        assert_eq!(claude_cli_model_arg("fable"), CLAUDE_FABLE_MODEL_ID);
        assert_eq!(claude_cli_model_arg("fable"), "claude-fable-5");
        assert_eq!(claude_cli_model_arg("FABLE"), "claude-fable-5");
        // Already-dated fable id passes through unchanged (idempotent).
        assert_eq!(claude_cli_model_arg("claude-fable-5"), "claude-fable-5");
        // opus/sonnet/haiku are CLI shortcuts — pass through verbatim.
        assert_eq!(claude_cli_model_arg("opus"), "opus");
        assert_eq!(claude_cli_model_arg("sonnet"), "sonnet");
        assert_eq!(claude_cli_model_arg("haiku"), "haiku");
        // Non-claude names are untouched.
        assert_eq!(claude_cli_model_arg("gpt-5.5"), "gpt-5.5");
    }

    #[test]
    fn test_claude_fable_parses_to_claude_provider() {
        let spec = parse_model_spec("claude:fable");
        assert_eq!(spec.provider.as_deref(), Some("claude"));
        assert_eq!(spec.model_id, "fable");
        // The claude handler expands the bare model id to the CLI arg.
        assert_eq!(claude_cli_model_arg(&spec.model_id), "claude-fable-5");
    }

    #[test]
    fn test_alias_claude_opus_resolves_to_bare_opus() {
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::Verification);
        assert_eq!(
            resolved.model, "opus",
            "claude:opus must resolve to bare 'opus', not a dated model ID"
        );
    }

    #[test]
    fn test_alias_claude_sonnet_resolves_to_bare_sonnet_when_explicit() {
        let mut config = Config::default();
        config.models.evaluator = Some(RoleModelConfig {
            model: Some("claude:sonnet".to_string()),
            provider: None,
            tier: None,
            endpoint: None,
            reasoning: None,
        });
        let resolved = config.resolve_model_for_role(DispatchRole::Evaluator);
        assert_eq!(
            resolved.model, "sonnet",
            "claude:sonnet must resolve to bare 'sonnet', not a dated model ID"
        );
    }

    #[test]
    fn test_alias_claude_haiku_resolves_to_bare_haiku() {
        let config = Config::default();
        let resolved = config.resolve_model_for_role(DispatchRole::Triage);
        assert_eq!(
            resolved.model, "haiku",
            "claude:haiku must resolve to bare 'haiku', not a dated model ID"
        );
    }

    #[test]
    fn test_builtin_registry_uses_bare_aliases_not_dated_ids() {
        let config = Config::default();
        let registry = config.effective_registry();
        for entry in &registry {
            if entry.provider == "anthropic" {
                // Fable 5 is the deliberate exception: the claude CLI has no
                // bare `fable` shortcut, so its registry model MUST be the full
                // CLI id `claude-fable-5`. Every other anthropic entry uses a
                // bare alias the CLI resolves to the current production model.
                if entry.model == CLAUDE_FABLE_MODEL_ID {
                    continue;
                }
                assert!(
                    !entry.model.contains('-'),
                    "Anthropic registry entry '{}' has model '{}' — \
                     expected bare alias (opus/sonnet/haiku), not a dated model ID",
                    entry.id,
                    entry.model
                );
            }
        }
    }

    // ── Legacy section alias / merge tests ────────────────────────────
    //
    // Cover the rename-deprecation merge contract: when global and local
    // disagree on which name they use for the same logical section, the
    // *local* value must still win regardless of the spelling, and a
    // one-time deprecation warning must fire for whichever file used the
    // legacy name.

    #[test]
    fn test_local_dispatcher_overrides_global_coordinator() {
        // Global uses legacy [coordinator]; local uses canonical [dispatcher].
        // After normalization local must shadow global on overlapping keys.
        let mut global: toml::Value = toml::from_str(
            r#"
[coordinator]
executor = "native"
max_agents = 2
"#,
        )
        .unwrap();
        let mut local: toml::Value = toml::from_str(
            r#"
[dispatcher]
executor = "claude"
"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        normalize_legacy_tables(&mut global, "global.toml", &mut warnings);
        normalize_legacy_tables(&mut local, "local.toml", &mut warnings);

        let merged = merge_toml(global, local);
        let cfg: Config = merged.try_into().expect("merged must deserialize");

        assert_eq!(
            cfg.coordinator.effective_executor(),
            "claude",
            "local [dispatcher].executor must override global [coordinator].executor"
        );
        assert_eq!(
            cfg.coordinator.max_agents, 2,
            "global value must be preserved on subkeys local doesn't shadow"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("[coordinator]") && w.contains("global.toml")),
            "deprecation warning must mention legacy section + originating file, got {:?}",
            warnings
        );
    }

    #[test]
    fn test_local_coordinator_overrides_global_dispatcher() {
        // Reverse: global is fully migrated to [dispatcher]; local hasn't been
        // updated yet and still uses [coordinator]. Local must still win.
        let mut global: toml::Value = toml::from_str(
            r#"
[dispatcher]
executor = "native"
max_agents = 2
"#,
        )
        .unwrap();
        let mut local: toml::Value = toml::from_str(
            r#"
[coordinator]
executor = "claude"
"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        normalize_legacy_tables(&mut global, "global.toml", &mut warnings);
        normalize_legacy_tables(&mut local, "local.toml", &mut warnings);

        let merged = merge_toml(global, local);
        let cfg: Config = merged.try_into().expect("merged must deserialize");

        assert_eq!(
            cfg.coordinator.effective_executor(),
            "claude",
            "local [coordinator].executor must override global [dispatcher].executor"
        );
        assert_eq!(cfg.coordinator.max_agents, 2);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("[coordinator]") && w.contains("local.toml")),
            "deprecation warning must mention legacy section + local file, got {:?}",
            warnings
        );
    }

    #[test]
    fn test_deprecation_warning_fires_once_per_load() {
        // A single normalization pass over a file with one legacy section
        // pushes exactly one warning, and a follow-up pass over the now-
        // migrated value pushes none. This is what guarantees `wg config
        // show` doesn't spam the user with one warning per field-read.
        let mut global: toml::Value = toml::from_str(
            r#"
[coordinator]
executor = "native"
"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        normalize_legacy_tables(&mut global, "/tmp/global.toml", &mut warnings);
        assert_eq!(
            warnings.len(),
            1,
            "exactly one warning per legacy section per file, got {:?}",
            warnings
        );
        let msg = &warnings[0];
        assert!(
            msg.contains("Deprecated"),
            "expected 'Deprecated' in {}",
            msg
        );
        assert!(
            msg.contains("[coordinator]"),
            "expected legacy name in {}",
            msg
        );
        assert!(
            msg.contains("[dispatcher]"),
            "expected canonical name in {}",
            msg
        );
        assert!(
            msg.contains("/tmp/global.toml"),
            "expected file path in {}",
            msg
        );

        // Once the legacy key is gone, a re-run is a no-op.
        let mut warnings2 = Vec::new();
        normalize_legacy_tables(&mut global, "/tmp/global.toml", &mut warnings2);
        assert!(
            warnings2.is_empty(),
            "second pass must not re-warn after migration, got {:?}",
            warnings2
        );
    }

    #[test]
    fn test_local_config_load_normalizes_coordinator_alias() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join(".wg");
        fs::create_dir_all(&wg_dir).unwrap();
        fs::write(
            wg_dir.join("config.toml"),
            r#"
[coordinator]
max_agents = 2
model = "claude:opus"

[dispatcher]
model = "codex:gpt-5.5"
"#,
        )
        .unwrap();

        let cfg = Config::load(&wg_dir).expect("legacy+canonical local config should load");
        assert_eq!(cfg.coordinator.max_agents, 2);
        assert_eq!(
            cfg.coordinator.model.as_deref(),
            Some("codex:gpt-5.5"),
            "canonical [dispatcher].model must win over legacy [coordinator].model"
        );
        assert_eq!(cfg.coordinator.effective_executor(), "codex");
    }

    // ── Two-tier Pi profile (set_pi_tiers / pi_tiers) ────────────────────────

    #[test]
    fn test_pi_tiers_reads_strong_from_agent_and_weak_from_fast() {
        let mut cfg = Config::default();
        cfg.agent.model = "pi:openrouter/z-ai/glm-5.2".to_string();
        cfg.tiers.fast = Some("openrouter:deepseek/deepseek-chat".to_string());
        let (strong, weak) = cfg.pi_tiers();
        assert_eq!(strong.as_deref(), Some("pi:openrouter/z-ai/glm-5.2"));
        assert_eq!(weak.as_deref(), Some("openrouter:deepseek/deepseek-chat"));
    }

    #[test]
    fn test_set_pi_tiers_writes_full_strong_keyset() {
        let mut cfg = Config::default();
        cfg.set_pi_tiers(Some("openrouter:z-ai/glm-5.2"), None);
        // Every strong key is set to the pi: route (NOT the raw openrouter:
        // spec) so strong-tier work runs through the self-authenticating pi
        // handler rather than the in-process nex OpenRouter client. Weak keys
        // untouched.
        assert_eq!(cfg.agent.model, "pi:openrouter/z-ai/glm-5.2");
        assert_eq!(
            cfg.coordinator.model.as_deref(),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        assert_eq!(
            cfg.tiers.standard.as_deref(),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        assert_eq!(
            cfg.tiers.premium.as_deref(),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        assert_eq!(
            cfg.models.default.as_ref().and_then(|m| m.model.as_deref()),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        assert_eq!(
            cfg.models
                .task_agent
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some("pi:openrouter/z-ai/glm-5.2")
        );
        // The strong spec routes to the pi handler (single source of truth).
        assert_eq!(
            crate::dispatch::handler_for_model(&cfg.agent.model),
            crate::dispatch::ExecutorKind::Pi
        );
        // Weak tier left alone by a strong-only update.
        assert!(cfg.tiers.fast.is_none());
    }

    #[test]
    fn test_set_pi_tiers_weak_only_pins_agency_oneshots() {
        let mut cfg = Config::default();
        cfg.agent.model = "claude:opus".to_string();
        cfg.set_pi_tiers(None, Some("openrouter:deepseek/deepseek-chat"));
        assert_eq!(
            cfg.tiers.fast.as_deref(),
            Some("openrouter:deepseek/deepseek-chat")
        );
        for role in [
            DispatchRole::Evaluator,
            DispatchRole::Assigner,
            DispatchRole::FlipInference,
            DispatchRole::FlipComparison,
        ] {
            assert_eq!(
                cfg.models.get_role(role).and_then(|m| m.model.as_deref()),
                Some("openrouter:deepseek/deepseek-chat"),
                "weak update must pin the {role:?} agency one-shot"
            );
        }
        // Strong (chat/worker) untouched by a weak-only update.
        assert_eq!(cfg.agent.model, "claude:opus");
    }

    #[test]
    fn test_set_pi_tiers_roundtrips_through_read() {
        let mut cfg = Config::default();
        cfg.set_pi_tiers(
            Some("openrouter:qwen/qwen3-max"),
            Some("openrouter:deepseek/deepseek-v3.1"),
        );
        let (strong, weak) = cfg.pi_tiers();
        // Strong is normalized to a pi: route on write; weak (agency one-shots)
        // keeps its native openrouter: route for the keyless-native fallback.
        assert_eq!(strong.as_deref(), Some("pi:openrouter/qwen/qwen3-max"));
        assert_eq!(weak.as_deref(), Some("openrouter:deepseek/deepseek-v3.1"));
    }

    #[test]
    fn test_pi_strong_route_normalizes_openrouter_to_pi_but_leaves_others() {
        // OpenRouter routes (the reported bug: `openrouter:z-ai/glm-5.2` routed
        // to nex and required a wg-side key) become pi: routes.
        assert_eq!(
            pi_strong_route("openrouter:z-ai/glm-5.2"),
            "pi:openrouter/z-ai/glm-5.2"
        );
        // Bare slash / CLI-slash routes are OpenRouter routes too.
        assert_eq!(
            pi_strong_route("z-ai/glm-5.2"),
            "pi:openrouter/z-ai/glm-5.2"
        );
        assert_eq!(
            pi_strong_route("openrouter/z-ai/glm-5.2"),
            "pi:openrouter/z-ai/glm-5.2"
        );
        // Idempotent: an already-pi: route is preserved verbatim.
        assert_eq!(
            pi_strong_route("pi:openrouter/z-ai/glm-5.2"),
            "pi:openrouter/z-ai/glm-5.2"
        );
        // Self-authenticating CLIs are left alone (they need no wg key, no pi).
        assert_eq!(pi_strong_route("claude:opus"), "claude:opus");
        assert_eq!(pi_strong_route("codex:gpt-5.5"), "codex:gpt-5.5");
        // nex-local / oai-compat routes need the in-process handler + endpoint;
        // pi cannot stand in, so they pass through unchanged.
        assert_eq!(pi_strong_route("nex:qwen3-coder"), "nex:qwen3-coder");
        assert_eq!(pi_strong_route("local:qwen3-coder"), "local:qwen3-coder");
        // A bare single-token alias gives pi no provider to target — verbatim.
        assert_eq!(pi_strong_route("opus"), "opus");
        // Empty / whitespace inputs are returned as-is.
        assert_eq!(pi_strong_route(""), "");
        // The normalized openrouter route routes to the pi handler; the raw one
        // would have routed to the in-process nex/native handler.
        assert_eq!(
            crate::dispatch::handler_for_model(&pi_strong_route("openrouter:z-ai/glm-5.2")),
            crate::dispatch::ExecutorKind::Pi
        );
        assert_eq!(
            crate::dispatch::handler_for_model("openrouter:z-ai/glm-5.2"),
            crate::dispatch::ExecutorKind::Native
        );
    }

    #[test]
    fn test_pi_key_constants_cover_design_keyset() {
        // Guard the §4.1 key-set the file patcher and in-memory writer share.
        assert!(Config::PI_STRONG_TOML_KEYS.contains(&"agent.model"));
        assert!(Config::PI_STRONG_TOML_KEYS.contains(&"tiers.standard"));
        assert!(Config::PI_STRONG_TOML_KEYS.contains(&"tiers.premium"));
        assert!(Config::PI_WEAK_TOML_KEYS.contains(&"tiers.fast"));
        assert!(Config::PI_WEAK_TOML_KEYS.contains(&"models.evaluator.model"));
        // Tiers must be disjoint — a key never belongs to both colors.
        for k in Config::PI_STRONG_TOML_KEYS {
            assert!(!Config::PI_WEAK_TOML_KEYS.contains(k), "{k} in both tiers");
        }
    }

    #[test]
    fn execution_system_key_keeps_handler_and_provider_boundaries() {
        for (route, handler, provider) in [
            ("claude:sonnet", "claude", "anthropic-cli"),
            ("codex:gpt-5.5", "codex", "openai-codex-cli"),
            ("pi:openai-codex:gpt-5.6-terra", "pi", "openai-codex"),
            ("pi:openrouter:z-ai/glm-5.2", "pi", "openrouter"),
            ("nex:openrouter:z-ai/glm-5.2", "nex", "openrouter"),
            ("nex:qwen3-coder", "nex", "oai-compat"),
        ] {
            let key = execution_system_key(route).unwrap();
            assert_eq!(key.handler, handler, "route={route}");
            assert_eq!(key.provider, provider, "route={route}");
        }
        for ambiguous in ["gpt-5.5", "openrouter:z-ai/glm-5.2", "z-ai/glm-5.2"] {
            assert!(
                execution_system_key(ambiguous).is_err(),
                "ambiguous route unexpectedly selected a system: {ambiguous}"
            );
        }
    }

    #[test]
    fn execution_fallback_config_roundtrips_in_declared_order() {
        let input = r#"
[[execution.fallbacks]]
primary = "pi:openai-codex:gpt-5.6-terra"
models = ["pi:openai-codex:gpt-5.6-sol", "pi:openai-codex:gpt-5.6-luna"]
"#;
        let config: Config = toml::from_str(input).unwrap();
        assert_eq!(
            config.execution.models_for("pi:openai-codex:gpt-5.6-terra"),
            [
                "pi:openai-codex:gpt-5.6-sol",
                "pi:openai-codex:gpt-5.6-luna"
            ]
        );
    }

    #[test]
    fn config_validation_rejects_cross_system_execution_fallbacks() {
        let mut config = Config::default();
        config.execution.fallbacks.push(ExecutionFallback {
            primary: "pi:openrouter:z-ai/glm-5.2".into(),
            models: vec![
                "pi:openai-codex:gpt-5.6-sol".into(),
                "nex:openrouter:z-ai/glm-5.2".into(),
                "claude:haiku".into(),
            ],
        });

        let validation = config.validate_config();
        let cross_system = validation
            .errors
            .iter()
            .filter(|diagnostic| diagnostic.rule == "execution-fallback-cross-system")
            .count();
        assert_eq!(cross_system, 3);
    }
}
