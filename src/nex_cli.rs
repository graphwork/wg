use clap::Args;
use std::path::PathBuf;

/// Shared command-line options for both `wg nex` and the standalone `nex` binary.
#[derive(Args, Clone, Debug)]
pub struct NexArgs {
    /// Standalone nex state directory. Only used by the standalone `nex`
    /// binary and eval/compatibility modes; `wg nex` remains WG-scoped.
    #[arg(long = "nex-dir")]
    pub nex_dir: Option<PathBuf>,

    /// Extra standalone nex config file to load at highest precedence.
    #[arg(long = "config")]
    pub config: Option<PathBuf>,

    /// Model to use (e.g., openrouter:qwen/qwen3-coder, ollama:llama3.2, sonnet)
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Named endpoint from config (e.g., `openrouter`, `local`)
    /// OR a bare URL like `http://localhost:11434` for zero-config
    /// local servers (Ollama, vLLM, llama.cpp). URLs use
    /// oai-compat + no auth by default; pass `-k` to override.
    #[arg(long, short = 'e')]
    pub endpoint: Option<String>,

    /// API key override for direct endpoint testing. Prefer configured
    /// endpoint credentials (`api_key_ref`, `api_key_file`, or `api_key_env`);
    /// command-line keys may be visible to local process inspection.
    #[arg(long = "api-key", short = 'k', value_name = "KEY")]
    pub api_key: Option<String>,

    /// Custom system prompt
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Initial message (skip the first prompt)
    pub message: Option<String>,

    /// Maximum conversation turns
    #[arg(long, default_value = "200")]
    pub max_turns: usize,

    /// Chatty mode: echo the full tool output content under each
    /// tool-call line, exactly as the model sees it (capped at
    /// 20 lines / 1600 bytes per call). Default shows only a
    /// one-line summary per call. Useful when actively following
    /// an agent's actions.
    #[arg(long, short = 'c')]
    pub chatty: bool,

    /// Verbose console output: implies `--chatty` and also emits
    /// compaction diagnostics, token accounting, and the
    /// session-log path banner. Useful for debugging the REPL
    /// itself. The on-disk NDJSON session log is always complete
    /// regardless of this flag.
    #[arg(long, short = 'v')]
    pub verbose: bool,

    /// Read-only safety mode: only expose tools that cannot modify
    /// state (read_file, grep, web_search, web_fetch, etc.). Tools
    /// like write_file, edit_file, and bash (which can run arbitrary
    /// commands) are removed from the registry. Use this when you
    /// want to browse, research, or explore without risk of the
    /// agent modifying any files.
    #[arg(long, short = 'r')]
    pub read_only: bool,

    /// Resume a previous nex session. Three shapes:
    ///
    ///   `nex --resume` / `wg nex --resume` — interactive picker
    ///                                       over all sessions,
    ///                                       most-recent first.
    ///   `nex --resume <pattern>`           — pattern-match the
    ///                                       most-recent session
    ///                                       whose alias / uuid
    ///                                       prefix / kind
    ///                                       contains `<pattern>`.
    ///   `nex --chat <uuid|alias>`          — address a specific
    ///                                       session directly
    ///                                       (works without
    ///                                       `--resume`).
    ///
    /// Bare `nex` or `wg nex` (no flags) starts a FRESH session every
    /// time — no auto-resume.
    #[arg(long, value_name = "PATTERN", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    /// Load an agency role/skill by name to augment the session.
    /// Searches `.wg/agency/primitives/components/` for a
    /// matching component and appends its content to the system
    /// prompt. Use "coordinator" to load the coordinator prompt;
    /// WG task management still happens through `wg` CLI
    /// commands run with bash.
    #[arg(long)]
    pub role: Option<String>,

    /// Run as a chat-tethered agent: read user turns from
    /// `<wg-dir>/chat/<id>/inbox.jsonl`, write streaming tokens
    /// to `<wg-dir>/chat/<id>/streaming`, append finalized
    /// assistant turns to `<wg-dir>/chat/<id>/outbox.jsonl`.
    /// Bypasses stdin/stderr. When set, the journal is stored at
    /// `<wg-dir>/chat/<id>/conversation.jsonl` so `--resume`
    /// picks up the right session automatically.
    ///
    /// Primary use case: this is how nex serves as the coordinator
    /// (spawned by the service / a graph task with a chat tether to
    /// the TUI). Pair with `--role coordinator` for the coordinator
    /// prompt.
    #[arg(long = "chat-id")]
    pub chat_id: Option<u32>,

    /// Bind this nex session to a chat dir by reference. Accepts
    /// a UUID, a UUID prefix (>=4 chars), or an alias like
    /// `coordinator-0` / `task-<id>` / a user-chosen handle.
    /// If the reference doesn't yet resolve to a session, a new
    /// session is created under that alias. Same effect as
    /// `--chat-id` except not limited to numeric ids.
    #[arg(long = "chat")]
    pub chat_ref: Option<String>,

    /// Run in autonomous mode — EndTurn exits the loop instead
    /// of prompting for next input. Used when a task-agent
    /// spawns nex as a one-shot executor.
    #[arg(long = "autonomous")]
    pub autonomous: bool,

    /// Skip the MCP server spawn/discover step at startup. Use
    /// this when MCP tooling is misconfigured or when you want a
    /// deterministic, minimal tool surface for debugging.
    #[arg(long = "no-mcp")]
    pub no_mcp: bool,

    /// Benchmark/evaluation mode — run nex as a non-interactive
    /// eval-harness target (SWE-bench, Terminal-Bench, etc.).
    ///
    /// Implies `--autonomous` and `--no-mcp`. Skips mounting the
    /// chat-file surface (no inbox.jsonl/outbox.jsonl/.streaming
    /// files dropped into the repo under eval). Suppresses the
    /// decorative banner on stderr. On clean exit, emits a
    /// single-line JSON summary to stdout so the harness can log
    /// turns/tokens/final-status without parsing ANSI output:
    ///   {"status":"ok","turns":N,"input_tokens":I,"output_tokens":O,"exit_reason":"..."}
    ///
    /// Process exit: 0 on EndTurn/clean completion, non-zero on
    /// max-turns/context-limit/error — same abnormal-exit rules
    /// the autonomous task-agent path already uses.
    #[arg(long = "eval-mode")]
    pub eval_mode: bool,

    /// Streaming idle timeout in seconds (default: 600). How long to
    /// wait for new chunks before aborting a streaming request.
    /// Useful for slow local models where prefill can take minutes.
    /// Also configurable via WG_STREAM_IDLE_TIMEOUT_SECS env var
    /// (flag takes precedence).
    #[arg(long = "idle-timeout-secs")]
    pub idle_timeout_secs: Option<u64>,

    /// Minimal tool surface: expose only the canonical local-dev
    /// tool set (Read, Edit, Write, Bash, Grep, Glob, TodoWrite)
    /// and omit everything else (WebFetch, WebSearch, NotebookEdit,
    /// Monitor, Task*, Remote*, Cron*, MCP tools). Dramatically
    /// reduces prefill cost for small local models. Implies --no-mcp.
    #[arg(long = "minimal-tools")]
    pub minimal_tools: bool,
}
