# Amplifier–workgraph Executor Gap Analysis

**Date**: 2026-02-18
**Task**: `analyze-wg-executors`

## 1. How workgraph Currently Configures Executors

Executor configuration lives in two places:

### 1.1 Global defaults in `.wg/config.toml`

```toml
[agent]
executor = "claude"     # Default executor name
model = "opus"          # Default model

[coordinator]
executor = "claude"     # Executor for daemon-spawned agents
model = ...             # Optional: override agent.model for service
```

The `coordinator.executor` field determines which executor the daemon uses
when spawning agents. It defaults to `"claude"` (`src/config.rs:214`).

### 1.2 Per-executor TOML files in `.wg/executors/<name>.toml`

Each executor is a TOML file with this schema (`src/service/executor.rs:184-219`):

```toml
[executor]
type = "claude" | "shell" | "custom"   # Required
command = "claude"                      # Required: binary to run
args = ["--print", "--verbose", ...]    # Optional: CLI arguments
working_dir = "{{working_dir}}"         # Optional: cwd for the process
timeout = 600                           # Optional: seconds

[executor.env]                          # Optional: environment variables
WG_TASK_ID = "{{task_id}}"

[executor.prompt_template]              # Optional: task prompt
template = "..."
```

### 1.3 Built-in defaults (no file required)

If `.wg/executors/<name>.toml` doesn't exist, `ExecutorRegistry::default_config()`
(`executor.rs:315-418`) provides hardcoded defaults for three names:

| Name      | Type      | Command  | Behavior                                        |
|-----------|-----------|----------|-------------------------------------------------|
| `claude`  | `claude`  | `claude` | Pipes prompt via stdin, passes `--print --verbose --permission-mode bypassPermissions --output-format stream-json` |
| `shell`   | `shell`   | `bash`   | Runs `bash -c <task.exec>` with env vars        |
| `default` | `default` | `echo`   | Just echoes the task ID                         |

Any name not in this list AND without a `.toml` file produces an error:
`"Unknown executor '<name>'. Available: claude, shell, default"`.

### 1.4 Executor resolution chain

At spawn time (`spawn.rs:190-191`):
```
ExecutorRegistry::new(dir).load_config(executor_name)
  -> if .wg/executors/<name>.toml exists: load it
  -> else: return hardcoded default or error
```

A user-provided `.toml` file **fully overrides** the built-in default for
that name (e.g., placing `claude.toml` replaces the built-in claude config).

## 2. The Spawn Protocol

### 2.1 Prompt generation

1. **Template variables** are built from the task (`executor.rs:34-57`):
   - `{{task_id}}`, `{{task_title}}`, `{{task_description}}`
   - `{{task_context}}` — aggregated logs/artifacts from `blocked_by` dependencies
   - `{{task_identity}}` — resolved from the task's assigned Agent entity (role + motivation)
   - `{{working_dir}}` — project root (parent of `.wg/`)
   - `{{skills_preamble}}` — content from `.claude/skills/using-superpowers/SKILL.md`

2. **Template application** (`executor.rs:259-286`): all `{{var}}` placeholders
   in the executor config (command, args, env, prompt_template, working_dir) are
   replaced with resolved values.

3. **Prompt file**: for `type = "claude"`, the rendered prompt template is written
   to `prompt.txt` in the agent output directory (`spawn.rs:237-239`).

### 2.2 Command construction

The inner command is built differently per executor type (`spawn.rs:221-261`):

- **`"claude"` type**: `cat prompt.txt | claude --print --verbose ...`
  - Prompt is piped via stdin from a file
  - Model flag appended if specified

- **`"shell"` type**: `bash -c '<task.exec>'`
  - Requires `task.exec` field to be set
  - No prompt piping

- **Any other type**: `command arg1 arg2 ...`
  - Args are joined with shell escaping
  - **No prompt piping** — stdin is null

### 2.3 The `run.sh` wrapper

Every spawn generates a `run.sh` wrapper script (`spawn.rs:268-303`) that:

1. Unsets `CLAUDECODE`/`CLAUDE_CODE_ENTRYPOINT` (allows nested sessions)
2. Runs the inner command, redirecting stdout+stderr to `output.log`
3. After the process exits, checks task status via `wg show --json`
4. If task is still in-progress:
   - Exit code 0 → `wg done`
   - Exit code ≠ 0 → `wg fail --reason "Agent exited with code N"`
5. Preserves the original exit code

### 2.4 Process spawning

The wrapper script is launched as a detached process (`spawn.rs:318-352`):
- `bash run.sh` with `setsid()` (new session, survives daemon restart)
- stdin/stdout/stderr all set to `Stdio::null()` (output goes via `>> output.log`)
- Environment variables from executor config are set
- `WG_TASK_ID` and `WG_AGENT_ID` are injected

### 2.5 Task claim atomics

Before spawn, the task is claimed (`spawn.rs:356-374`):
- Status → InProgress, assigned → agent ID
- If spawn fails, the claim is rolled back (`spawn.rs:377-413`)

## 3. What Would Need to Change to Support Arbitrary Executors

### 3.1 The stdin piping problem (critical)

The most significant gap is that **only `type = "claude"` executors receive the
prompt via stdin**. The branching logic in `spawn.rs:221-261`:

```rust
match settings.executor_type.as_str() {
    "claude" => {
        // Writes prompt.txt, generates: cat prompt.txt | command args
    }
    "shell" => {
        // bash -c <task.exec>  -- no prompt
    }
    _ => {
        // command args  -- no prompt piping
    }
}
```

For any non-`"claude"` type, the prompt template is **silently discarded**
even if `prompt_template` is configured. This is why the amplifier bundle
uses `type = "claude"` as a hack — it's the only way to get prompt piping.

**Fix**: Either:
- (a) Add a `prompt_mode` field: `"stdin"`, `"file"`, `"arg"`, `"none"`
- (b) Always write `prompt.txt` and let the executor choose how to consume it
- (c) Pipe stdin for all types that have a `prompt_template` configured

Option (b) is simplest: always write `prompt.txt` to the agent output dir,
and if `prompt_mode = "stdin"` (or `type = "claude"`), also pipe it.

### 3.2 The executor type enum is implicit

The `executor_type` field is a free-form string, but behavior is only
defined for `"claude"` and `"shell"`. Any other value falls through to a
generic branch that just joins command + args. There's no documentation of
what types are supported or how to add new ones.

**Fix**: Either formalize the type enum with documented behavior per type,
or (better) replace the type-based branching with composable capabilities:
`stdin_mode`, `prompt_delivery`, `working_dir_mode`, etc.

### 3.3 Model flag injection is claude-specific

The `--model <model>` flag is hardcoded for claude-type executors
(`spawn.rs:229-232`). Other executors that accept model selection
(like `amplifier run --model`) would need their own flag injection.

**Fix**: Add a `model_flag` field to executor config:
```toml
[executor]
model_flag = "--model"  # or "-m" for amplifier
```

Or allow `{{model}}` as a template variable in args (currently not
supported — the template vars don't include model).

### 3.4 Missing `{{model}}` template variable

The `TemplateVars` struct (`executor.rs:17-25`) does not include a `model`
field. The effective model is determined in `spawn.rs:218` but never
exposed as a template variable. This means executor configs can't reference
`{{model}}` in args or env.

### 3.5 Output capture assumes text on stdout/stderr

The `run.sh` wrapper captures output via `>> "$OUTPUT_FILE" 2>&1`. This
works for text-based executors but would need adjustment for executors
that produce structured output (JSON streams, binary, etc.).

## 4. Gap Between amplifier.toml and wg's Executor Model

### 4.1 Current workarounds in the amplifier executor

The existing amplifier bundle ([ramparte/amplifier-bundle-workgraph](https://github.com/ramparte/amplifier-bundle-workgraph))
uses these workarounds:

| Gap | Workaround |
|-----|------------|
| Only `type = "claude"` gets stdin | Uses `type = "claude"` even though it's Amplifier |
| Amplifier wants prompt as positional arg | `amplifier-run.sh` wrapper: reads stdin → passes as arg |
| No bundle/provider selection in executor config | Wrapper parses extra `--bundle`/`--model` flags |
| `amplifier run` output format differs from claude | Uses `--output-format json`, output still captured |

### 4.2 Amplifier's invocation model vs. wg's assumptions

| Aspect | wg's claude executor | Amplifier |
|--------|---------------------|-----------|
| Prompt delivery | stdin (piped from file) | Positional argument |
| Mode selection | Implicit (always non-interactive) | `--mode single` flag |
| Output format | `--output-format stream-json` | `--output-format json` or `text` |
| Bundle/profile | N/A | `--bundle <name>` or `--provider <name>` |
| Session management | None (one-shot) | Sessions with resume (`--resume <id>`) |
| Agent delegation | Single agent per task | Multi-agent within session |
| Model selection | `--model <name>` | `--model <name>` (same flag) |

### 4.3 Missing executor capabilities

Things Amplifier supports that wg's executor model can't express:

1. **Bundle selection**: no way to specify which Amplifier bundle/profile per task or globally
2. **Session resume**: no concept of resumable sessions across retries
3. **Provider selection**: no way to choose Anthropic vs. OpenAI vs. Azure
4. **Structured output parsing**: no way to extract structured results from executor output
5. **Multi-agent configuration**: Amplifier agents can delegate to sub-agents; wg can't configure this per-executor

### 4.4 Things that already work well

- **Template variables**: The `{{task_id}}`, `{{task_title}}`, etc. contract works for both
- **Environment variables**: `WG_TASK_ID` is properly passed
- **Working directory**: `{{working_dir}}` correctly resolves
- **Prompt template**: The full prompt template system works (via the `type = "claude"` hack)
- **Wrapper script pattern**: `run.sh` auto-completion works for any executor

## 5. Specific Code Locations That Would Need Modification

### Must-change for arbitrary executor support

| File | Lines | What | Why |
|------|-------|------|-----|
| `src/commands/spawn.rs` | 221-261 | `match settings.executor_type.as_str()` | Add prompt delivery for non-claude types |
| `src/commands/spawn.rs` | 229-232 | Model flag injection | Make model flag configurable per executor |
| `src/service/executor.rs` | 17-25 | `TemplateVars` struct | Add `model` field |
| `src/service/executor.rs` | 170-180 | `TemplateVars::apply()` | Add `{{model}}` substitution |
| `src/service/executor.rs` | 191-219 | `ExecutorSettings` struct | Add `prompt_mode`, `model_flag` fields |
| `src/service/executor.rs` | 315-418 | `default_config()` | Add `"amplifier"` built-in default |

### Should-change for better ergonomics

| File | Lines | What | Why |
|------|-------|------|-----|
| `src/commands/service.rs` | 777-784 | `effective_executor` resolution | Support per-agent executor from Agency system (already partially done) |
| `src/config.rs` | 117-144 | `AgentConfig` | Deprecate `command_template` (superseded by executor TOML system) |
| `src/service/executor.rs` | 315-418 | `default_config()` | Better error message for unknown executors, suggest creating TOML file |

### Nice-to-have for amplifier integration

| File | Lines | What | Why |
|------|-------|------|-----|
| `src/commands/spawn.rs` | 268-303 | Wrapper script generation | Support `prompt_mode = "arg"` (pass prompt as CLI arg instead of stdin) |
| `src/service/executor.rs` | 184-219 | `ExecutorConfig` | Add optional `extra_args` or `flags` for per-task flag injection |
| `src/graph.rs` | (Task struct) | Task fields | Add optional `executor` field per task (currently only per-agent) |

## 6. Recommended Minimal Changes

To support Amplifier (and any future executor) without the `type = "claude"` hack:

1. **Add `prompt_mode` to `ExecutorSettings`**:
   ```toml
   [executor]
   prompt_mode = "stdin"   # "stdin" (default for claude), "file", "arg", "none"
   ```

2. **Always write `prompt.txt`** regardless of executor type (it's cheap and
   useful for debugging).

3. **Add `{{model}}` template variable** so executor configs can include the
   model anywhere in their args: `args = ["--model", "{{model}}"]`.

4. **Add a built-in `"amplifier"` default config** that uses `prompt_mode = "arg"`
   and wraps the `amplifier run --mode single` invocation.

5. **Decouple stdin piping from executor type**: move the `cat prompt.txt | command`
   logic to be controlled by `prompt_mode` rather than `executor_type`.

These changes are backward-compatible: existing `type = "claude"` configs
continue to work unchanged, and the amplifier bundle could drop its wrapper
script entirely.
