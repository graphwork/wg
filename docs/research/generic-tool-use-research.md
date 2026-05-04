# Research: Generic Tool Use for Lightweight LLM Tasks

**Date:** 2026-03-12
**Task:** research-generic-tool
**Status:** Complete

---

## 1. Current Executor Architecture

### 1.1 Executor Types

workgraph has four executor types, each providing a different level of tool access:

| Executor | Description | Tool Access | How Tools Work |
|----------|-------------|-------------|----------------|
| `claude` | Spawns Claude CLI (`claude` command) | Full Claude Code tools | Claude CLI provides built-in Bash, Read, Edit, etc. |
| `amplifier` | Spawns Amplifier CLI | Amplifier module tools | Amplifier mounts tool modules |
| `native` | Rust-native agent loop (`wg native-exec`) | In-process `ToolRegistry` | Tools are Rust `Tool` trait objects executed in-process |
| `shell` | Direct shell command | None (just runs a command) | No multi-turn, no tool use |

### 1.2 Exec Modes (Tiers)

Within each executor, `exec_mode` controls what tools are available:

| exec_mode | Claude executor | Native executor | Purpose |
|-----------|----------------|-----------------|---------|
| `bare` | `--tools Bash(wg:*)` | Bundle::bare() → wg_* tools only | Pure reasoning, synthesis, triage |
| `light` | `--allowedTools Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch` | Bundle::research() → read + wg tools | Research, code review |
| `full` | All tools (minus Agent) | Bundle::implementer() → all tools | Implementation, coding |
| `shell` | N/A | N/A | Direct command execution |

### 1.3 Tool Access Flow Diagram

```
Task with exec_mode
       │
       ▼
┌─────────────────────────────────────────────────────────────┐
│  src/commands/spawn/execution.rs:build_inner_command()      │
│  Routes by executor_type × exec_mode                       │
└────────┬───────────────┬──────────────────┬────────────────┘
         │               │                  │
    claude CLI      native executor    amplifier CLI
         │               │                  │
         ▼               ▼                  ▼
┌────────────┐  ┌────────────────┐  ┌─────────────┐
│ Claude Code │  │ wg native-exec │  │  amplifier   │
│ (subprocess)│  │ (subprocess)   │  │ (subprocess) │
│             │  │                │  │              │
│ Built-in    │  │ AgentLoop      │  │ Module-based │
│ tools from  │  │ (agent.rs)     │  │ tools        │
│ Claude CLI  │  │                │  │              │
│ --tools/    │  │ ToolRegistry   │  │ provider →   │
│ --allowed   │  │ (tools/mod.rs) │  │ orchestrator │
│ flags       │  │                │  │ → tools      │
│             │  │ Provider trait │  │              │
│ Anthropic   │  │ (provider.rs)  │  │ ChatRequest  │
│ model only  │  │                │  │ /Response    │
│             │  │ Anthropic or   │  │              │
│             │  │ OpenAI-compat  │  │              │
└────────────┘  └────────────────┘  └─────────────┘
```

### 1.4 The Bare Mode Gap

**Current state for `claude` executor, bare mode** (`src/commands/spawn/execution.rs:520-563`):

```
"claude" if exec_mode == "bare" => {
    // Passes: --tools Bash(wg:*) --allowedTools Bash(wg:*)
    // Result: Agent gets Bash tool but ONLY for wg:* prefixed commands
}
```

This actually DOES give the Claude executor bare-mode agents `wg` CLI access via `Bash(wg:*)`. The agent can run `wg edit`, `wg publish`, etc. through the Bash tool.

**For the `native` executor, bare mode** (`src/executor/native/bundle.rs:64-82`):

```rust
Bundle::bare() → tools: ["wg_show", "wg_list", "wg_add", "wg_done", "wg_fail", "wg_log", "wg_artifact"]
```

The native executor's bare bundle has in-process wg tools but is **missing key commands**: `wg edit`, `wg publish`, `wg msg`, `wg assign`, `wg evaluate`, etc. These operations require either:
- Expanding the native wg tools (implementing each command as a `Tool` trait impl), or
- Giving bare-mode agents the `bash` tool with `wg:*` filtering (not currently supported in native executor)

**The original problem**: Placement tasks (`.place-*`) were using the `claude` executor with `exec_mode=bare`, but when the agent lacked the Bash tool or the model couldn't use it, the agent died. The fix was partially applied — bare mode now uses `--tools Bash(wg:*)` — but the deeper issue remains: **non-Claude executors (native, or any model via OpenRouter) need the same capability.**

### 1.5 Where Tool Access is Granted/Restricted

| File | Lines | What it does |
|------|-------|-------------|
| `src/commands/spawn/execution.rs` | 492-705 | `build_inner_command()` — routes exec_mode to CLI flags for each executor |
| `src/executor/native/bundle.rs` | 64-82 | `Bundle::bare()` — defines which native tools bare agents get |
| `src/executor/native/bundle.rs` | 120-170 | `resolve_bundle()` — maps exec_mode → bundle → tool filtering |
| `src/executor/native/tools/mod.rs` | 92-105 | `ToolRegistry::filter()` — applies bundle whitelist to available tools |
| `src/executor/native/agent.rs` | 158-292 | `AgentLoop::run()` — the tool-use loop (sends tools in API request, executes on response) |
| `src/commands/native_exec.rs` | 26-128 | `wg native-exec` — creates provider + registry + agent loop, runs it |

---

## 2. Amplifier's Approach

### 2.1 Architecture Overview

Amplifier (Microsoft MADE:Explorations, MIT license) uses a **kernel + module** architecture inspired by the Linux kernel. Key components:

| Component | Purpose | Analog in workgraph |
|-----------|---------|---------------------|
| `amplifier-core` (~2600 LOC Python) | Session lifecycle, module loading, event bus | `executor/native/` |
| `Orchestrator` protocol | Drives the LLM interaction loop | `AgentLoop` |
| `Provider` protocol | Abstracts LLM API wire formats | `Provider` trait |
| `Tool` protocol | Abstracts tool implementations | `Tool` trait |
| `ContextManager` protocol | Memory/context management | System prompt construction |
| Bundles (YAML) | Composition of modules | Bundles (TOML) |

### 2.2 How Amplifier Handles Multi-Turn Tool Use

The `Orchestrator.execute()` method signature reveals the pattern:

```python
async def execute(self, prompt, context, providers, tools, hooks) -> str
```

The orchestrator is responsible for the tool-use loop:
1. Get messages from context manager
2. Send to provider (ChatRequest → ChatResponse)  
3. Parse tool calls from response (`provider.parse_tool_calls()`)
4. Execute tools (`tool.execute(input) → ToolResult`)
5. Add tool results to context
6. Loop until no more tool calls

**Key insight**: The orchestrator is **provider-agnostic**. It works with any provider that implements the `Provider` protocol. The provider handles wire-format translation. This is almost exactly what workgraph's native executor already does.

### 2.3 How Amplifier Handles OpenRouter

Amplifier's `provider-openai` module (`amplifier_module_provider_openai/__init__.py`) uses the OpenAI Python SDK to communicate with any OpenAI-compatible endpoint, including OpenRouter. Tool calls are handled via the standard OpenAI `function_calling` / `tool_calls` response format.

**OpenRouter-specific handling**: OpenRouter proxies tool-use calls transparently for most models. Models that support tool use (Claude, GPT-4, Gemini, etc.) work through OpenRouter without any special shim. The `_response_handling.py` module handles parsing tool calls from the response.

### 2.4 What workgraph Can Learn from Amplifier

1. **workgraph already has the right pattern.** The `AgentLoop` + `Provider` trait + `ToolRegistry` is structurally identical to Amplifier's `Orchestrator` + `Provider` + `Tool` pattern.

2. **The gap is not architectural — it's feature completeness.** workgraph's native executor already handles multi-turn tool use with both Anthropic and OpenAI-compatible providers. The gap is that bare-mode agents don't get enough tools.

3. **Bundle-based tool filtering is the right approach.** Both systems use bundles/tiers to control tool access. workgraph just needs to expand what's in the bare bundle.

---

## 3. Provider Tool-Use Capabilities

### 3.1 Anthropic (Claude)

- **Native tool use** via Messages API: `tools` array in request, `tool_use` content blocks in response
- **workgraph leverages this** via `AnthropicClient` (`src/executor/native/client.rs`) which serializes `ToolDefinition` as Anthropic tool schemas and parses `tool_use` content blocks
- **Multi-turn**: Fully supported. Agent loop sends tool results as `tool_result` content blocks in the next user message.
- **All Claude models** support tool use (Haiku, Sonnet, Opus)

### 3.2 OpenRouter

- **Transparent proxy** for tool use: If the underlying model supports function calling, OpenRouter passes tool definitions and tool calls through
- **OpenAI-compatible format**: Uses `tools` array in request, `tool_calls` in response (same as OpenAI Chat Completions API)
- **workgraph leverages this** via `OpenAiClient` (`src/executor/native/openai_client.rs`) which translates between Anthropic-style canonical types and OpenAI wire format
- **Haiku on OpenRouter**: Yes, Claude Haiku via OpenRouter supports multi-turn tool use. The tool calls are proxied through the OpenAI-compatible format
- **Non-tool-use models** (e.g., DeepSeek R1): workgraph already handles this via `ModelRegistry.supports_tool_use()` — if false, tools are omitted from the request. These models get `supports_tools: false` in `AgentLoop`

### 3.3 OpenAI

- **Function calling / tool use**: `tools` array in request, `tool_calls` in assistant message
- **workgraph supports this** via the same `OpenAiClient` (OpenAI and OpenRouter use the same wire format)
- **All GPT-4 variants** support tool use

### 3.4 Common Abstraction Layer

workgraph already has a provider-agnostic abstraction: the `Provider` trait (`src/executor/native/provider.rs`):

```rust
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn model(&self) -> &str;
    fn max_tokens(&self) -> u32;
    async fn send(&self, request: &MessagesRequest) -> Result<MessagesResponse>;
}
```

Both `AnthropicClient` and `OpenAiClient` implement this trait. The `AgentLoop` works exclusively through this trait — it never sees provider-specific types.

**The canonical types** (`MessagesRequest`, `MessagesResponse`, `ContentBlock`, `ToolDefinition`, etc.) defined in `client.rs` serve as the common schema. The `OpenAiClient` translates between these canonical types and OpenAI wire format.

---

## 4. Gap Analysis: What's Missing

### 4.1 Problem Summary

The fundamental problem is NOT "models can't do tool use" — they can, and workgraph already supports it. The problem is:

1. **Bare bundle is too restrictive**: Only 7 wg tools, missing `wg edit`, `wg publish`, `wg msg`, `wg assign`, etc.
2. **No bash tool in bare mode**: The native executor's bare bundle excludes the `bash` tool entirely. Even a restricted bash (wg commands only) would solve the problem.
3. **Claude executor bare mode already works**: It uses `--tools Bash(wg:*)` which gives restricted bash access. The native executor needs the same.

### 4.2 Specific Missing Pieces

| What's Missing | Where | Impact |
|---------------|-------|--------|
| `wg edit` in native tools | `src/executor/native/tools/wg.rs` | Can't modify task descriptions |
| `wg publish` in native tools | `src/executor/native/tools/wg.rs` | Can't publish tasks |
| `wg msg send/read` in native tools | `src/executor/native/tools/wg.rs` | Can't communicate between tasks |
| `wg assign` in native tools | `src/executor/native/tools/wg.rs` | Can't assign agents |
| Bash tool with wg-only filter | `src/executor/native/tools/bash.rs` | No restricted shell access |
| `wg evaluate` in native tools | `src/executor/native/tools/wg.rs` | Can't run evaluations |

### 4.3 Models Without Tool Use

For models that don't support tool use (e.g., DeepSeek R1, some Ollama models):
- workgraph sets `supports_tools: false` and omits tools from the request
- The model gets a pure text prompt and produces a text response
- **No multi-turn interaction** is possible — the model can only reason and produce text
- This is fine for truly text-only tasks, but placement/assignment tasks need to run commands

---

## 5. Recommendation: Shortest Path to Universal Shell Access

### Option A: Expand the Bare Bundle (Recommended — Least Effort)

Add the `bash` tool to `Bundle::bare()` with a command whitelist. This is the simplest change:

```rust
// In src/executor/native/bundle.rs
pub fn bare() -> Self {
    Bundle {
        name: "bare".to_string(),
        tools: vec![
            "wg_show", "wg_list", "wg_add", "wg_done", "wg_fail",
            "wg_log", "wg_artifact",
            "bash",  // Add bash tool — all wg commands are now available
        ],
        ..
    }
}
```

Optionally, add a command whitelist to the `BashTool` that limits it to `wg *` commands when running in a restricted bundle. This mirrors what the Claude executor already does with `Bash(wg:*)`.

**Changes needed:**
1. `src/executor/native/tools/bash.rs` — Add optional command prefix filter (`wg_only: bool` or `allowed_prefixes: Vec<String>`)
2. `src/executor/native/bundle.rs` — Add `bash` to bare bundle (possibly with config for the prefix filter)
3. `src/executor/native/tools/mod.rs` — Wire up the restricted bash variant

**Estimated scope:** ~50-80 lines of code changes.

### Option B: Add Missing wg Tools to Native Registry

Implement each missing `wg` subcommand as a native `Tool` trait impl:

```rust
// New tools in src/executor/native/tools/wg.rs:
WgEditTool, WgMsgSendTool, WgMsgReadTool, WgPublishTool, WgAssignTool, ...
```

**Pros:** More controlled, in-process execution (no subprocess overhead)
**Cons:** Much more code (each tool is 40-80 lines), must be kept in sync with CLI commands, doesn't cover edge cases like piped commands

**Estimated scope:** ~400-600 lines for 6-8 new tools.

### Option C: Hybrid — Restricted Bash + Key Native Tools

Add the `bash` tool with `wg`-only restriction (Option A) plus implement the 2-3 most critical missing native tools (e.g., `wg_msg_send`, `wg_msg_read`, `wg_edit`).

**This is the best balance** — bash gives immediate universal coverage while native tools provide faster, safer access for the most common operations.

### For Non-Tool-Use Models

For models that can't do function calling at all, there are two approaches:

1. **Text-based tool calling**: Parse the model's text output for `<tool>` tags or code blocks containing tool invocations. This is fragile but workable.
2. **Pre-computed execution**: For deterministic tasks (placement, assignment), don't use an LLM loop at all — use the `shell` executor type to run the `wg` command directly.

The current approach (placement tasks use exec_mode=bare with a tool-capable model) is the right one. The fix is just making sure the tools are actually available.

---

## 6. Architecture Comparison: workgraph vs Amplifier

| Aspect | workgraph (native executor) | Amplifier |
|--------|---------------------------|-----------|
| Language | Rust | Python |
| Provider abstraction | `Provider` trait | `Provider` protocol |
| Tool abstraction | `Tool` trait + `ToolRegistry` | `Tool` protocol + coordinator mount |
| Tool-use loop | `AgentLoop::run()` | `Orchestrator.execute()` |
| Tool filtering | `Bundle.filter_registry()` | Bundle YAML `tools:` list |
| Provider routing | `create_provider()` by model string | Module mount by config |
| Wire format translation | `OpenAiClient` translates canonical ↔ OpenAI | `provider-openai` module translates |
| Multi-turn | Yes (loop until EndTurn) | Yes (loop until no tool_calls) |
| OpenRouter support | Yes (via OpenAiClient) | Yes (via provider-openai) |

**Key takeaway**: workgraph's native executor is architecturally complete. It already has a generic, provider-agnostic tool-use layer. The only gap is the bare bundle's tool set being too restrictive.

---

## 7. Summary

| Research Question | Finding |
|------------------|---------|
| exec_mode bare vs full | Bare restricts tools to wg-only subset; full gives everything. Same pattern in both Claude and native executors. |
| Where is tool access granted? | `build_inner_command()` for Claude executor, `resolve_bundle()` + `filter_registry()` for native executor |
| How does Claude executor give Bash? | Via `--tools Bash(wg:*)` flag — already working |
| What would it take for bare-mode bash? | Add `bash` to bare bundle + optional command prefix filter (~50-80 LOC) |
| How does Amplifier handle tool use? | Orchestrator protocol drives provider-agnostic tool loop — same pattern as workgraph's AgentLoop |
| OpenRouter tool-use? | Transparent proxy — works with any model that supports function calling |
| Recommendation | **Option A**: Add bash to bare bundle with wg-prefix restriction. Minimal change, immediate fix. |
