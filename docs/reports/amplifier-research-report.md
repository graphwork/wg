# Amplifier Research Report

**Date**: 2026-03-02
**Task**: research-amplifier-bundles
**Author**: Research agent

---

## Executive Summary

Amplifier is Microsoft's modular AI agent framework, structured as a thin Python kernel (~2,600 lines) with a plugin ecosystem. The user has been actively using it with workgraph, building an integration bundle (`amplifier-bundle-workgraph`), patching the OpenAI provider module for OpenRouter compatibility, and designing multi-provider workflows. This report assesses the bundle system, architecture, compatibility posture, and recommends a workgraph integration path.

**Key finding**: Amplifier's architecture is well-designed (mechanism/policy separation, composable bundles, 5 clean module protocols), but the execution model is *fundamentally a Python wrapper around LLM API calls with tool routing*. A Rust-native replacement for workgraph's needs would be straightforward — the hard part isn't the agent loop, it's the ecosystem of tools and behaviors that Amplifier's bundle system provides.

**Recommendation**: Build a native executor in workgraph for the core tool-use loop (model URL + API key + model name → tool-use loop), and maintain the Amplifier executor as an option for when the full Amplifier ecosystem (bundles, agents, behaviors, recipes) is needed.

---

## 1. Bundle Format Specification (As Understood)

### 1.1 Bundle File Format

A bundle is a **Markdown file with YAML frontmatter** (`bundle.md` or `*.yaml`).

**Frontmatter structure:**

```yaml
---
bundle:
  name: <string>           # Required. Namespace for @mentions
  version: <semver>        # Required. "1.0.0"
  description: <string>    # Optional

includes:                  # Other bundles to compose with (like CSS imports)
  - bundle: <uri>          # git+https://..., local:path, or <ns>:<path>

session:                   # Session-level config
  debug: <bool>
  orchestrator:            # THE execution loop
    module: <module-id>    # e.g., "loop-streaming"
    source: <git-uri>
    config: { ... }
  context:                 # Memory management
    module: <module-id>    # e.g., "context-simple"
    source: <git-uri>
    config: { max_tokens: 300000, auto_compact: true }

providers:                 # LLM backends (list)
  - module: <module-id>    # e.g., "provider-anthropic", "provider-openai"
    source: <git-uri>
    config:
      default_model: <string>
      api_key: <string>    # Or use env vars
      base_url: <string>   # For OpenRouter/custom endpoints
      # ... provider-specific config

tools:                     # Agent capabilities (list)
  - module: <module-id>    # e.g., "tool-filesystem", "tool-bash"
    source: <git-uri>
    config: { ... }

hooks:                     # Lifecycle observers (list)
  - module: <module-id>    # e.g., "hook-shell"
    source: <git-uri>
    config: { enabled: true, timeout: 30 }

agents:                    # Named sub-agent configs (dict or include list)
  include:
    - <ns>:<agent-name>
  # or inline:
  <name>:
    instruction: <string>
    tools: [...]

spawn:                     # Child session config
  exclude_tools: [...]
---

# Markdown body becomes the system instruction
@<namespace>:<path>        # @-mentions expand to file contents
```

### 1.2 Bundle Composition

Bundles compose via `includes:` — later entries override earlier ones (like CSS cascade). The `Bundle.compose()` method deep-merges mount plans:

- **Providers, tools, hooks**: Lists are merged (deduped by module name)
- **Session config**: Deep-merged (later overrides)
- **Agents**: Merged by name
- **Instructions**: Concatenated
- **Context files**: Merged, with namespace tracking for @-mention resolution

### 1.3 Bundle Sources

Bundles can come from:
- **Git URIs**: `git+https://github.com/microsoft/amplifier-foundation@main`
- **Git subdirectory**: `...@main#subdirectory=behaviors/agents.yaml`
- **Local namespace**: `foundation:behaviors/sessions` (relative to bundle base path)
- **User registry**: `amplifier bundle add <uri> --name <alias>`

Bundles are cached in `~/.amplifier/cache/<name>-<hash>/`.

### 1.4 Agent Format

Agents use the **same file format** as bundles but with `meta:` instead of `bundle:` in frontmatter:

```yaml
---
meta:
  name: zen-architect
  description: "System design agent..."
---

# Agent instructions (markdown body)
```

Agents are config overlays — when spawned, they create a new `AmplifierSession` with merged config and `parent_id` linking.

### 1.5 Behavior Format

Behaviors are thin bundles (`.yaml`) that compose a set of related config. Example:

```yaml
bundle:
  name: workgraph
  version: 1.0.0
  description: "workgraph integration behavior"

context:
  include:
    - workgraph:context/workgraph-guide.md
    - workgraph:context/wg-executor-protocol.md
```

---

## 2. Amplifier Architecture Assessment

### 2.1 Layer Cake

```
┌─────────────────────────────────────────────┐
│ App Layer: amplifier-app-cli                │
│   CLI commands, REPL, session storage,      │
│   approval UI, @mention expansion           │
├─────────────────────────────────────────────┤
│ Library Layer: amplifier-foundation         │
│   Bundle composition, bundle loading,       │
│   agent configs, behaviors, recipes         │
├─────────────────────────────────────────────┤
│ Kernel: amplifier-core (~2,600 lines)       │
│   AmplifierSession, ModuleCoordinator,      │
│   ModuleLoader, HookRegistry, Events        │
├─────────────────────────────────────────────┤
│ Modules (swappable):                        │
│   Providers: anthropic, openai, azure,      │
│              ollama, gemini, vllm           │
│   Tools: filesystem, bash, web, search,     │
│          task (delegation), todo, mcp       │
│   Orchestrators: loop-basic, loop-streaming │
│   Contexts: context-simple, context-persist │
│   Hooks: logging, redaction, approval,      │
│          streaming-ui, shell, todo-reminder │
└─────────────────────────────────────────────┘
```

### 2.2 Execution Model

The core execution is:

1. **Session init**: Load mount plan from bundle → instantiate `AmplifierSession`
2. **Module loading**: Load orchestrator, context manager, providers, tools, hooks via `ModuleLoader`
3. **Execute**: `session.execute(prompt)` → delegates to orchestrator
4. **Orchestrator loop**: The orchestrator (e.g., `loop-streaming`) runs:
   - Get messages from context manager
   - Call provider (`ChatRequest` → `ChatResponse`)
   - Parse tool calls from response
   - Execute tools
   - Add results to context
   - Loop until no more tool calls
5. **Cleanup**: Reverse-order cleanup of all modules

**This is indeed "just a Python wrapper around API calls with tool routing"** — but a well-engineered one with clean separation of concerns.

### 2.3 Provider Protocol

Providers implement:
```python
class Provider(Protocol):
    name: str
    async def complete(request: ChatRequest, **kwargs) -> ChatResponse
    async def list_models() -> list[ModelInfo]
    def parse_tool_calls(response: ChatResponse) -> list[ToolCall]
    def get_info() -> ProviderInfo
```

The `ChatRequest`/`ChatResponse` are Pydantic models with typed content blocks (`TextContent`, `ThinkingContent`, `ToolCallContent`, etc.).

### 2.4 Tool Protocol

Tools implement:
```python
class Tool(Protocol):
    name: str
    description: str
    async def execute(input: dict[str, Any]) -> ToolResult
```

Tool specs are sent to the LLM as function definitions. The LLM decides which tools to call.

### 2.5 Module Loading

Modules are Python packages loaded via `uv` from git sources. The `ModuleLoader`:
1. Resolves source URI (git, local, entry point)
2. Downloads/caches package via `uv pip install`
3. Imports the module's `mount()` function
4. Calls `mount(coordinator, config)` which attaches to the coordinator's mount points

---

## 3. OpenRouter/OpenAI Compatibility Assessment

### 3.1 Current State

The user has been working on multi-model support through several channels:

1. **amplifier-module-provider-openai**: The official OpenAI provider (Microsoft repo), which the user has been contributing to. Key features:
   - Responses API integration (not just Chat Completions)
   - Reasoning state preservation (encrypted content re-insertion)
   - Auto-continuation for incomplete responses
   - Deep research / background mode support
   - **Compatibility layer** (PR #17 / commit c430f27): Per-feature config flags for custom endpoints

2. **OpenRouter provider YAML**: Already in amplifier-foundation as `providers/openrouter.yaml`:
   ```yaml
   providers:
     - module: provider-openai
       config:
         base_url: https://openrouter.ai/api/v1
         default_model: deepseek/deepseek-chat-v3-0324
         enable_native_tools: true
         enable_reasoning_replay: true
         enable_store: false
         enable_background: false
   ```

3. **Compatibility flags** added to provider-openai:
   - `enable_native_tools`: Toggle OpenAI-native tools (web_search, apply_patch, etc.)
   - `enable_reasoning_replay`: Toggle reasoning state preservation
   - `enable_store`: Toggle stateful conversation features
   - `enable_background`: Toggle deep research / background mode
   - **Auto-detection**: When `base_url` is set, all flags default to `false`; explicit overrides work

### 3.2 Multi-Provider Issues

From the user's own testing and design docs (`MULTI_PROVIDER_DESIGN.md`, `MULTI_PROVIDER_REVIEW.md`):

| Issue | Status |
|-------|--------|
| `--provider` uses module shorthand, not `name:` field | **Open** — design exists for fix |
| Same-module providers can't coexist | **Open** — needs kernel mount name change |
| `provider current` shows config priority, not runtime health | **Open** — design exists |
| No `amplifier provider configured` command | **Designed** but unimplemented |

### 3.3 Upstream Status

The compatibility layer (config flags) was merged upstream. The multi-provider design docs exist in the workgraph bundle repo but have NOT been upstreamed. The fundamental limitation — providers hardcode their mount name — requires a coordinated change across kernel, foundation, and CLI.

### 3.4 Local Patches

The amplifier repo has a local change: `.amplifier/modules/provider-openai` submodule pointer has been updated to a dirty commit (`f6be738...dirty`), likely pointing to a local fork with additional patches. The `amplifier-module-provider-openai` repo itself has no local uncommitted changes.

---

## 4. Bundle Ecosystem Assessment

### 4.1 Official Bundles (Microsoft)

| Bundle | Purpose |
|--------|---------|
| `amplifier-foundation` | Default bundle with tools, agents, behaviors |
| `amplifier-bundle-recipes` | Multi-step workflow orchestration |
| `amplifier-bundle-design-intelligence` | Component design agent |
| `amplifier-bundle-shadow` | Shadow/audit capabilities |
| `amplifier-bundle-skills` | Skill-based agents |
| `amplifier-bundle-browser-tester` | Browser testing |
| `amplifier-bundle-python-dev` | Python development |
| `amplifier-bundle-filesystem` | Enhanced filesystem (apply_patch) |
| `amplifier-bundle-modes` | Mode switching |

### 4.2 User's Bundles

| Bundle | Purpose |
|--------|---------|
| `amplifier-bundle-workgraph` | workgraph integration (bi-directional) |

### 4.3 Community Assessment

Amplifier is Microsoft Research/MADE:Explorations — early preview, not accepting external contributions yet. The ecosystem is small (< 30 repos). Bundles are well-structured but tightly coupled to the Microsoft org. There is no "bundle registry" — everything is git URIs.

**Compatibility with Amplifier's bundle ecosystem is NOT worth pursuing as a primary goal.** The value is in understanding their *patterns* (composition, context injection, behavior layering) and replicating what makes sense natively in workgraph.

---

## 5. Minimal Viable Native Executor Assessment

### 5.1 What the Amplifier Executor Currently Does

When workgraph dispatches a task via the Amplifier executor:

1. workgraph renders a prompt template with task context (ID, title, description, dependency artifacts)
2. `amplifier-run.sh` wrapper reads prompt from stdin
3. Runs `amplifier run --mode single --output-format json --bundle workgraph "$PROMPT"`
4. Amplifier loads the workgraph bundle (foundation + workgraph behavior + hook-shell)
5. Session runs the orchestrator loop (LLM → tool calls → results → loop)
6. Agent uses `wg` CLI commands to log progress, record artifacts, mark done/fail
7. Amplifier exits, workgraph detects completion

### 5.2 What a Rust-Native Replacement Needs

At minimum:

1. **Model configuration**: API endpoint URL + API key + model name
2. **System prompt**: The rendered task prompt (already done by workgraph's template system)
3. **Tool definitions**: JSON schemas for tools the agent can call
4. **Tool-use loop**:
   - Send messages + tool specs to LLM API
   - Parse response for tool calls
   - Execute tools (shell commands, file operations)
   - Add results to message history
   - Loop until no more tool calls or max iterations
5. **API compatibility**: OpenAI Chat Completions API (supported by OpenAI, OpenRouter, Anthropic via their OpenAI-compatible endpoint, vLLM, Ollama, etc.)

### 5.3 Complexity Assessment

| Component | Complexity | Notes |
|-----------|-----------|-------|
| HTTP client for OpenAI API | Low | `reqwest` + JSON serialization |
| Message history management | Low | Vec of messages, simple append |
| Tool call parsing | Medium | JSON response parsing, function call extraction |
| Tool execution (shell) | Low | `std::process::Command` |
| Tool execution (filesystem) | Low | `std::fs` |
| Streaming support | Medium | SSE parsing |
| Context window management | Medium | Token counting, truncation |
| Multi-provider support | Low | Just different base_url + auth |
| Reasoning/thinking blocks | Medium | Provider-specific parsing |
| Error handling / retry | Medium | Rate limits, timeouts, retries |

**Yes, it really is that simple for the core loop.** The hard parts are:

1. **Token counting** — Need a tokenizer or rough heuristic
2. **Provider-specific quirks** — Each API has slight differences in tool call format
3. **Streaming** — Nice to have but not required for workgraph executor
4. **Context compaction** — For long sessions, need to summarize/drop old messages

### 5.4 What You'd Lose vs. Amplifier

| Amplifier Feature | Impact of Loss |
|-------------------|---------------|
| Bundle composition | Replace with workgraph's own config |
| @mention expansion | Not needed for task execution |
| Agent delegation (sub-sessions) | workgraph already has this natively |
| Session persistence/resume | workgraph agents are single-shot |
| Recipe system | Not used in executor context |
| Hook system | Replace with workgraph's own logging |
| Approval UI | Not needed for automated execution |
| 14+ specialized agents | The agent *is* the prompt template |

---

## 6. Recommendations

### 6.1 Dual-Track Strategy

**Track A: Native Executor (Primary)**

Build a minimal Rust-native tool-use loop in workgraph for the common case:

- Configure: model URL + API key + model name
- Define available tools (shell, file read/write, wg CLI)
- Inject task prompt as system/user message
- Run tool-use loop until completion
- Parse `wg done`/`wg fail` commands as termination signals

This covers 90% of workgraph's executor needs with zero Python dependency.

**Track B: Amplifier Executor (Optional)**

Keep the `amplifier-bundle-workgraph` executor for when users want:
- Full Amplifier ecosystem (bundles, agents, behaviors)
- Specific LLM features (Anthropic extended thinking, OpenAI reasoning, etc.)
- Session persistence and resume
- Recipe-driven workflows within tasks

### 6.2 Native Executor Design Sketch

```
[workgraph config]
  executor = "native"
  model_url = "https://openrouter.ai/api/v1"
  api_key_env = "OPENROUTER_API_KEY"
  model = "anthropic/claude-sonnet-4"

[tools]
  shell = true       # Execute shell commands
  read_file = true   # Read files
  write_file = true  # Write files
  edit_file = true   # Edit files (search/replace)

[limits]
  max_turns = 100
  max_tokens = 200000
  timeout = 600
```

### 6.3 API Format to Target

Use **OpenAI Chat Completions API** format as the wire protocol. This is supported by:
- OpenAI directly
- Anthropic (via their OpenAI-compatible endpoint)
- OpenRouter (100+ models)
- vLLM, Ollama, LiteLLM (local/self-hosted)

The tool-use format in Chat Completions is well-documented and stable:
```json
{
  "model": "...",
  "messages": [...],
  "tools": [{"type": "function", "function": {"name": "...", "parameters": {...}}}],
  "tool_choice": "auto"
}
```

### 6.4 What NOT to Build

- **Bundle composition system** — workgraph's config is sufficient
- **@mention expansion** — Not needed; task context comes from wg templates
- **Provider abstraction layer** — Just use OpenAI-compatible API everywhere
- **Session persistence** — Agents are single-shot; logs go through `wg log`
- **Hook system** — workgraph has its own event/logging system
- **Agent delegation** — workgraph IS the delegation system

### 6.5 Migration Path

1. Build native executor with OpenAI Chat Completions API support
2. Test with OpenRouter (cheapest way to access multiple models)
3. Keep Amplifier executor as `executor = "amplifier"` option
4. Default new projects to native executor
5. Eventually: allow per-task executor override for mixed setups

---

## 7. Key Files Reference

| File | Purpose |
|------|---------|
| `~/amplifier/README.md` | Amplifier overview and quick start |
| `~/amplifier/bundle.md` | Root bundle definition |
| `~/amplifier/pyproject.toml` | Dependencies: amplifier-core + amplifier-app-cli |
| `~/amplifier-app-cli/amplifier_app_cli/session_runner.py` | Session initialization |
| `~/amplifier-app-cli/amplifier_app_cli/main.py` | CLI entry point |
| `~/amplifier-module-provider-openai/amplifier_module_provider_openai/__init__.py` | OpenAI provider (2000+ lines) |
| `~/amplifier-module-provider-openai/amplifier_module_provider_openai/_constants.py` | Provider defaults |
| `~/amplifier-bundle-workgraph/bundle.md` | workgraph bundle definition |
| `~/amplifier-bundle-workgraph/executor/amplifier.toml` | Executor config for workgraph |
| `~/amplifier-bundle-workgraph/executor/amplifier-run.sh` | Wrapper script |
| `~/amplifier-bundle-workgraph/DESIGN.md` | Setup improvements design |
| `~/amplifier-bundle-workgraph/docs/OPENROUTER_PROVIDER_DESIGN.md` | OpenRouter provider design |
| `~/amplifier-bundle-workgraph/docs/MULTI_PROVIDER_DESIGN.md` | Multi-provider UX design |
| `~/amplifier-bundle-workgraph/docs/MULTI_PROVIDER_REVIEW.md` | Multi-provider test results |
| `~/.amplifier/cache/amplifier-foundation-*/bundle.md` | Foundation bundle (default) |
| `~/.amplifier/cache/amplifier-foundation-*/providers/openrouter.yaml` | OpenRouter provider config |
| `~/.local/share/uv/tools/amplifier/.../amplifier_core/session.py` | Kernel session |
| `~/.local/share/uv/tools/amplifier/.../amplifier_core/interfaces.py` | Module protocols |
| `~/.local/share/uv/tools/amplifier/.../amplifier_core/coordinator.py` | Module coordinator |
| `~/.local/share/uv/tools/amplifier/.../amplifier_foundation/bundle.py` | Bundle composition |

---

## 8. Glossary

| Term | Definition |
|------|-----------|
| **Bundle** | Composable configuration package (markdown + YAML frontmatter) |
| **Mount plan** | Config dict specifying which modules to load into a session |
| **Orchestrator** | The execution loop module (LLM → tools → loop) |
| **Provider** | LLM backend module (Anthropic, OpenAI, etc.) |
| **Tool** | Agent capability module (filesystem, bash, etc.) |
| **Hook** | Lifecycle observer module (logging, approval, etc.) |
| **Context manager** | Memory/conversation management module |
| **Behavior** | Thin bundle that composes a set of related configs |
| **Agent** | Config overlay for spawning specialized sub-sessions |
| **Recipe** | Multi-step workflow definition (YAML) |
| **@mention** | File reference system (`@namespace:path`) for context injection |
| **Coordinator** | Kernel component managing mount points and inter-module communication |

---

*Report generated from direct source code analysis of ~/amplifier, ~/amplifier-app-cli, ~/amplifier-bundle-workgraph, ~/amplifier-module-provider-openai, and installed amplifier-core and amplifier-foundation packages.*
