# How to Set Up a New wg Repo with OpenRouter as the Default Executor

**Task:** research-current-new
**Date:** 2026-04-03

---

## TL;DR — The Happy Path

```bash
# 1. One-time global setup (writes ~/.wg/config.toml)
export OPENROUTER_API_KEY="sk-or-v1-..."
wg setup --provider openrouter --model anthropic/claude-sonnet-4

# 2. Per-project init
cd my-project && git init  # if not already a repo
wg init

# 3. Start working
wg add "My first task" --verify "echo ok"
wg service start
```

That's 4 commands (including the export). Below is a deeper breakdown of each step, the config keys involved, and pain points.

---

## Step 1: Obtain an OpenRouter API Key

Sign up at [openrouter.ai](https://openrouter.ai/) and create an API key from the **Keys** page. The key looks like `sk-or-v1-...`.

---

## Step 2: Global Setup — `wg setup`

### Option A: Interactive wizard (recommended for first time)

```bash
wg setup
```

The wizard auto-detects your environment (installed CLIs, existing API keys, etc.) and walks you through:

1. **Provider selection** → choose "OpenRouter"
2. **Executor** → auto-set to `native` (since OpenRouter is non-Anthropic)
3. **API key** → choose between env var (`OPENROUTER_API_KEY`) or key file (`~/.config/openrouter/key`)
4. **Model** → pick from popular defaults (opus, sonnet, haiku via OpenRouter) or enter a custom model ID like `minimax/minimax-m2.7`
5. **Agency** → enable/disable auto-assign + auto-evaluate
6. **Max agents** → how many parallel agents
7. **API key validation** → hits OpenRouter `/models` endpoint, optionally auto-discovers and registers models
8. **Skill installation** → for `native` executor, skipped (wg skill is for Claude Code)
9. **Notifications** → optional Matrix/Telegram/email/Slack/webhook setup

Writes to: `~/.wg/config.toml`

### Option B: Non-interactive one-liner

```bash
wg setup --provider openrouter --model anthropic/claude-sonnet-4
```

Or with a custom model:

```bash
wg setup --provider openrouter --model minimax/minimax-m2.7
```

Additional flags:
- `--api-key-file ~/.config/openrouter/key` — point to key file instead of env var
- `--api-key-env OPENROUTER_API_KEY` — specify which env var (default for openrouter)
- `--url https://openrouter.ai/api/v1` — override endpoint URL (rarely needed)
- `--skip-validation` — skip the API key connectivity check

### What `wg setup` writes to `~/.wg/config.toml`

After running `wg setup --provider openrouter --model anthropic/claude-sonnet-4`, the global config looks like:

```toml
[coordinator]
executor = "native"
model = "openrouter:anthropic/claude-sonnet-4"
max_agents = 4

[agent]
executor = "native"
model = "openrouter:anthropic/claude-sonnet-4"

[models.default]
model = "openrouter:anthropic/claude-sonnet-4"

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
is_default = true

[agency]
auto_assign = true
auto_evaluate = true
```

### Key config fields explained

| Config key | Purpose |
|---|---|
| `coordinator.executor` | Which executor spawns agents. `native` = OpenAI-compatible API. `claude` = Claude Code CLI. |
| `coordinator.model` | Default model for the coordinator LLM (triage, assignment, etc.). Format: `provider:model` |
| `agent.executor` | Executor for spawned task agents |
| `agent.model` | Default model for task agents. Format: `provider:model` |
| `models.default.model` | Fallback default model for non-Anthropic providers |
| `llm_endpoints.endpoints[].name` | Display name for the endpoint |
| `llm_endpoints.endpoints[].provider` | Provider type: `openrouter`, `openai`, `anthropic`, `local` |
| `llm_endpoints.endpoints[].url` | API base URL (defaults to `https://openrouter.ai/api/v1` for openrouter) |
| `llm_endpoints.endpoints[].api_key_env` | Environment variable for the API key |
| `llm_endpoints.endpoints[].api_key_file` | Path to file containing the API key (alternative to env var) |
| `llm_endpoints.endpoints[].is_default` | Whether this is the default endpoint for its provider |
| `agency.auto_assign` | Auto-match agent identities to tasks |
| `agency.auto_evaluate` | Auto-evaluate task completions |

---

## Step 3: API Key Configuration

The API key is resolved in this priority order:

1. `OPENROUTER_API_KEY` environment variable (highest priority)
2. `OPENAI_API_KEY` environment variable (fallback)
3. Matching endpoint entry in config (`api_key_env` or `api_key_file` field)
4. `[native_executor]` section in config (legacy fallback)

**Recommended approach:** Set `OPENROUTER_API_KEY` in your shell profile:

```bash
# ~/.bashrc or ~/.zshrc
export OPENROUTER_API_KEY="sk-or-v1-..."
```

**Alternative:** Store in a file and reference it:

```bash
mkdir -p ~/.config/openrouter
echo "sk-or-v1-..." > ~/.config/openrouter/key
chmod 600 ~/.config/openrouter/key
wg endpoints add openrouter --provider openrouter --api-key-file ~/.config/openrouter/key --default --global
```

---

## Step 4: Per-Project Init — `wg init`

```bash
cd my-project
wg init
```

This creates:
- `.wg/` directory
- `.wg/graph.jsonl` — empty task graph
- `.wg/.gitignore` — excludes `agents/`, `service/`, credentials
- `.wg/agency/` — seeded roles, tradeoffs, and default agents (unless `--no-agency`)
- Adds `.wg` to the repo-level `.gitignore`

**`wg init` does NOT create a local `config.toml`** — it inherits the global config from `~/.wg/config.toml`. This is intentional: global config sets the provider/executor/model, and projects inherit it.

If you need project-specific overrides:

```bash
wg config --local --coordinator-model openrouter:minimax/minimax-m2.7
```

This creates `.wg/config.toml` with only the overridden fields. The merged config (global + local) is what wg uses at runtime.

---

## Step 5: Model Selection

### Model ID format

OpenRouter uses `provider/model` naming: `anthropic/claude-sonnet-4`, `minimax/minimax-m2.7`, `deepseek/deepseek-chat-v3`, etc.

In wg config/CLI, models are specified as `provider_prefix:model_id`:

```
openrouter:anthropic/claude-sonnet-4
openrouter:minimax/minimax-m2.7
openrouter:deepseek/deepseek-chat-v3
```

The `openrouter:` prefix tells wg to route through the OpenRouter endpoint.

### Known provider prefixes

`claude`, `openrouter`, `openai`, `codex`, `gemini`, `ollama`, `llamacpp`, `vllm`, `local`, `native`

### Setting the default model

```bash
# Global default (all projects)
wg config --global --coordinator-model openrouter:minimax/minimax-m2.7

# Per-project override
wg config --local --coordinator-model openrouter:minimax/minimax-m2.7

# Per-task override
wg add "My task" --model openrouter:minimax/minimax-m2.7
```

### Model precedence chain (highest → lowest)

1. **Task model** (`wg add --model` or `wg edit --model`)
2. **Agent preferred model** (`wg agent create --model`)
3. **Executor config model** (model field in executor config file)
4. **Coordinator model** (`coordinator.model` in config.toml)
5. **Executor default** (no `--model` flag passed)

### Executor auto-detection

When `coordinator.executor` is not explicitly set, wg infers it from the model:
- `openrouter:...` → `native` executor
- `openai:...` → `native` executor
- `claude:...` → `claude` executor
- bare model name → `claude` executor (default)

So setting `coordinator.model = "openrouter:minimax/minimax-m2.7"` automatically selects the `native` executor without needing to explicitly set `coordinator.executor`.

---

## Step 6: Verify and Use

```bash
# Test endpoint connectivity
wg endpoints test openrouter

# Check config
wg config --show

# View merged config with source annotations
wg config --list

# Search available models
wg models search "minimax"
wg models remote

# Create a task and start
wg add "Hello world task" --verify "echo ok"
wg service start
```

---

## Alternative: Manual Endpoint Configuration

Instead of `wg setup`, you can manually add an endpoint:

```bash
wg endpoints add openrouter \
  --provider openrouter \
  --api-key-file ~/.config/openrouter/key \
  --default \
  --global

wg config --global --coordinator-executor native
wg config --global --coordinator-model openrouter:anthropic/claude-sonnet-4
wg config --global --model openrouter:anthropic/claude-sonnet-4
```

This is more steps but gives fine-grained control.

---

## Environment Variables Reference

| Variable | Purpose |
|---|---|
| `OPENROUTER_API_KEY` | API key (highest priority for OpenRouter provider) |
| `OPENAI_API_KEY` | Fallback API key |
| `WG_LLM_PROVIDER` | Override provider detection |
| `WG_ENDPOINT_URL` | Override base URL for API requests |
| `WG_ENDPOINT_NAME` | Select a named endpoint from config |
| `OPENROUTER_BASE_URL` | Alternative base URL (fallback) |
| `WG_EXECUTOR_TYPE` | Set on spawned agents to indicate executor context |
| `WG_MODEL` | Set on spawned agents to indicate model |

---

## Gaps and Pain Points

### 1. No unified `wg init --provider openrouter` flag
**Issue:** `wg init` creates the `.wg/` directory but does NOT configure the provider/executor/model. Those must be set separately via `wg setup` (global) or `wg config` (local). A new user must know to run `wg setup` first, or manually edit config.toml.

**Ideal:** `wg init --provider openrouter --model minimax/minimax-m2.7` should do everything in one command: create `.wg/`, set up the endpoint, configure the executor, and save a local config.toml.

### 2. Two-config dance (global vs local)
**Issue:** `wg setup` writes to `~/.wg/config.toml` (global). `wg init` doesn't create a local config. If a user wants per-project model defaults, they must separately run `wg config --local --coordinator-model ...`. This is non-obvious.

**Ideal:** `wg init` could optionally create a `.wg/config.toml` with the project's model/endpoint settings. Or `wg setup --local` could write to the current project's config.

### 3. provider:model format is confusing for OpenRouter models
**Issue:** OpenRouter models already have a slash (`anthropic/claude-sonnet-4`). Adding the wg provider prefix creates `openrouter:anthropic/claude-sonnet-4`. This looks like three levels of nesting and is confusing. Users might try `minimax/minimax-m2.7` without the `openrouter:` prefix, which would be treated as a bare model name and default to the `claude` executor.

**Ideal:** wg could auto-detect that a model containing `/` should be routed through OpenRouter (or the configured default endpoint) without requiring an explicit prefix.

### 4. No `wg setup --local` for project-scoped config
**Issue:** `wg setup` always writes to global config. There's no `--local` flag to run the wizard for a project-specific config.

### 5. Skill installation not relevant for native executor
**Minor:** `wg init` checks for the Claude Code skill and hints about it even when the executor is `native`. The hint is slightly confusing for OpenRouter users since the `wg skill install` command installs a Claude Code skill, not something relevant to the native executor.

### 6. Model validation requires explicit `wg models search`
**Issue:** When setting a model like `openrouter:minimax/minimax-m2.7`, there's no inline validation that the model exists on OpenRouter. The user won't know until the first API call fails.

**Ideal:** `wg config --coordinator-model openrouter:minimax/minimax-m2.7` could optionally validate against the OpenRouter models API.

### 7. No quick "status" for OpenRouter readiness
**Issue:** After setup, there's no single command that confirms "your OpenRouter config is complete and working." You need to run `wg endpoints test openrouter` + check `wg config --show` separately.

**Ideal:** `wg doctor` or `wg status --check-llm` that validates the full chain: config → endpoint → API key → model availability.

---

## Summary: Minimum Steps for a Fresh Repo

| Step | Command | What it does |
|---|---|---|
| 1 | `export OPENROUTER_API_KEY="sk-or-v1-..."` | Set API key in environment |
| 2 | `wg setup --provider openrouter --model openrouter:minimax/minimax-m2.7` | Write global config: native executor, OpenRouter endpoint, model |
| 3 | `cd my-project && wg init` | Create `.wg/`, inherit global config |
| 4 | `wg endpoints test openrouter` | Verify connectivity (optional but recommended) |
| 5 | `wg add "First task" --verify "..."` | Create a task |
| 6 | `wg service start` | Launch the coordinator |

Steps 2 is a one-time operation (per machine). Steps 3–6 repeat per project.
