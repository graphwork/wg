# Prompt Construction Analysis: Claude vs Native Executor

## Executive Summary

The **same `build_prompt()` function** assembles prompts for **all** built-in executor types
(claude, native, amplifier, codex). The key differences are:

1. **WG Guide injection** — native executor gets an explicit wg usage guide; Claude executor relies on CLAUDE.md
2. **Prompt delivery mechanism** — Claude receives prompt via stdin pipe; native uses it as a system prompt
3. **User message** — native executor adds a short user-turn message; Claude treats the piped prompt as the initial user message
4. **State injection** — native executor has mid-turn `<system-reminder>` injection; Claude uses its own conversation management
5. **Tool availability** — native uses a Rust `ToolRegistry`; Claude uses the Claude Code harness built-in tools

---

## 1. Shared Prompt Assembly: `build_prompt()`

**File:** `src/service/executor.rs:709-880`

All built-in executors use the same `build_prompt(vars, scope, ctx)` function.
The prompt is assembled from sections based on the `ContextScope` enum:

| Scope    | Sections Included |
|----------|-------------------|
| `Clean`  | skills_preamble, task assignment header, agent identity, task details, pattern keywords, verification criteria, dependency context, triage mode, loop info |
| `Task`   | + discovered tests, tags/skills, queued messages, downstream awareness, wg usage guide (native only), workflow commands, git hygiene, message polling, ethos, decomposition guidance, research hints, graph patterns, reusable functions, wg CLI warning, context hints |
| `Graph`  | + project description, 1-hop subgraph summary |
| `Full`   | + system awareness preamble (top), full graph summary, CLAUDE.md content |

**The prompt text itself is identical** for claude/native/amplifier/codex at the same scope level, with one exception: the `wg_guide_content` field.

### Where the shared assembly is triggered

**File:** `src/commands/spawn/execution.rs:322-358`

```rust
if settings.prompt_template.is_none()
    && (settings.executor_type == "claude"
        || settings.executor_type == "amplifier"
        || settings.executor_type == "native")
{
    let prompt = build_prompt(&vars, scope, &scope_ctx);
    settings.prompt_template = Some(PromptTemplate { template: prompt });
}
```

---

## 2. Key Difference: WG Guide Injection (Native Only)

**File:** `src/commands/spawn/execution.rs:312-317`

```rust
// Claude agents get this context from CLAUDE.md; native executor models need it
// explicitly injected into the prompt.
if settings.executor_type == "native" {
    scope_ctx.wg_guide_content = super::context::read_wg_guide(dir);
}
```

**File:** `src/commands/spawn/context.rs:556-569`

The guide is loaded from `.wg/wg-guide.md` (user-customizable) or falls back to
the built-in `DEFAULT_WG_GUIDE` constant (`src/service/executor.rs:535-597`).

**Injected at:** `src/service/executor.rs:799-805` (Task+ scope):
```rust
if scope >= ContextScope::Task && !ctx.wg_guide_content.is_empty() {
    parts.push(format!("## wg Usage Guide\n\n{}", ctx.wg_guide_content));
}
```

### What the WG Guide contains that CLAUDE.md also covers:
- Task lifecycle states
- Core `wg` commands (show, log, artifact, done, fail, add, list, ready, msg)
- `--after` dependency usage
- `--verify` verification gates
- When to decompose vs implement directly
- Environment variables ($WG_AGENT_ID, $WG_TASK_ID, $WG_EXECUTOR_TYPE, $WG_MODEL)

### What CLAUDE.md contains that the WG Guide does NOT:
- `wg quickstart` orientation instruction
- `wg service start` dispatch instruction
- Orchestrating agent role restrictions
- Time budget guidance
- Task description validation template
- Cycle-specific instructions (CycleConfig, max-iterations, --converged)
- Agency system details
- Service configuration (coordinator-executor, model config)

---

## 3. Prompt Delivery Mechanism

### Claude Executor (full mode)
**File:** `src/commands/spawn/execution.rs:905-933`

The prompt is written to `prompt.txt` and piped via stdin:
```bash
cat /path/to/prompt.txt | claude --print --verbose --permission-mode bypassPermissions \
  --output-format stream-json --disallowedTools Agent --disable-slash-commands
```

The Claude CLI treats this piped input as the **user's initial message**. Claude Code's
own system prompt, CLAUDE.md, and harness instructions are added by the Claude Code
harness itself — these are invisible to wg.

**Key insight:** Claude agents get TWO layers of instructions:
1. **wg prompt** (via stdin) — task details, workflow commands, etc.
2. **Claude Code harness** (invisible) — CLAUDE.md, system-reminders, tool definitions, etc.

### Claude Executor (bare mode)
**File:** `src/commands/spawn/execution.rs:828-871`

Bare mode uses `--system-prompt` explicitly AND pipes a simpler user message:
```bash
cat /path/to/user_message.txt | claude --print ... --system-prompt "<prompt content>" \
  --tools "Bash(wg:*)" --allowedTools "Bash(wg:*)"
```

### Claude Executor (light mode)
**File:** `src/commands/spawn/execution.rs:872-904`

Light mode restricts tools to read-only + wg CLI:
```bash
cat /path/to/prompt.txt | claude --print ... \
  --allowedTools "Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch" \
  --disallowedTools "Edit,Write,NotebookEdit,Agent"
```

### Native Executor
**File:** `src/commands/spawn/execution.rs:989-1031`

The prompt is passed as an argument to `wg native-exec`:
```bash
wg native-exec --prompt-file /path/to/prompt.txt --exec-mode full \
  --task-id <id> --model <model> --provider <provider> ...
```

In `src/commands/native_exec.rs:40-77`:
- The prompt file is read into a string
- It becomes the **system prompt** for the API call
- An optional bundle suffix is appended

Then at `src/commands/native_exec.rs:168-173`:
```rust
rt.block_on(agent.run(&format!(
    "You are working on task '{}'. Complete the task as described in your system prompt. \
     When done, use the wg_done tool with task_id '{}'. \
     If you cannot complete the task, use the wg_fail tool with a reason.",
    task_id, task_id
)))
```

**The initial user message for native is this short instruction**, while for Claude
the entire assembled prompt IS the user message.

---

## 4. State Injection (Native Only)

**File:** `src/executor/native/state_injection.rs`

The native executor has a `StateInjector` that runs before each API turn:
- **Pending messages**: Checks `wg msg` for new messages from other agents
- **Graph state changes**: Detects dependency completion, new tasks, blocker changes
- **Context pressure**: Warns when approaching context limits

These are injected as ephemeral `<system-reminder>` blocks that appear for one turn only.

Claude agents get similar information through the Claude Code harness's own system-reminder
mechanism, but wg does not control that — it happens at a different layer.

---

## 5. Tool Availability

### Claude Executor
Tools are provided by the Claude Code harness:
- `Bash`, `Read`, `Write`, `Edit`, `Glob`, `Grep`, `WebFetch`, `WebSearch`, etc.
- `Agent` is **disallowed** (`--disallowedTools Agent`)
- Tool filtering varies by exec_mode (bare: only `Bash(wg:*)`; light: read-only + wg)

### Native Executor
**File:** `src/commands/native_exec.rs:61-66`

Tools come from the Rust `ToolRegistry`:
```rust
let mut registry = ToolRegistry::default_all(workgraph_dir, &working_dir);
let bundle = resolve_bundle(exec_mode, workgraph_dir);
registry = bundle.filter_registry(registry);
```

The native tools are implemented in `src/executor/native/tools/`:
- `bash_tool` — shell execution
- `read_file` — file reading
- `write_file` — file writing
- `edit_file` — surgical file editing
- `glob_tool` — file pattern matching
- `grep_tool` — content search
- `wg_done`, `wg_fail`, `wg_log`, `wg_add`, etc. — wg operations
- `web_search` — DuckDuckGo search
- `web_fetch` — URL fetching

---

## 6. Default Executor Configurations

**File:** `src/service/executor.rs:1247-1344`

| Executor   | Command        | Default Args                                              |
|------------|----------------|-----------------------------------------------------------|
| `claude`   | `claude`       | `--print --verbose --permission-mode bypassPermissions --output-format stream-json` |
| `native`   | `wg`           | `native-exec`                                             |
| `amplifier`| `amplifier`    | `run --mode single --output-format text`                  |
| `codex`    | `codex`        | `exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox` |
| `shell`    | `bash`         | `-c {{task_context}}`                                     |

---

## 7. CLAUDE.md Propagation Path

### For Claude Executor
1. `wg init` or `wg setup` writes wg directives to `~/.claude/CLAUDE.md` and project-level `CLAUDE.md`
   - **File:** `src/commands/setup.rs:215-282`
2. The Claude Code harness reads CLAUDE.md automatically at startup
3. At `Full` context scope, CLAUDE.md content is ALSO included in the assembled prompt
   - **File:** `src/service/executor.rs:846-851`
   - Read from: `src/commands/spawn/context.rs:540-553`

**Result:** Claude agents at Full scope get CLAUDE.md content TWICE — once from the harness, once in the prompt.

### For Native Executor
1. CLAUDE.md is not automatically loaded (no Claude Code harness)
2. At `Full` scope, CLAUDE.md content IS included in the assembled prompt (same code path)
3. Additionally, the WG Guide (`DEFAULT_WG_GUIDE`) is injected at Task+ scope

**Result:** Native agents get CLAUDE.md in the prompt at Full scope, plus the WG Guide at Task+ scope.

---

## 8. Concrete Prompt Comparison

### Claude Executor at Task Scope (typical)
```
# Task Assignment

You are an AI agent working on a task in a wg project.

## Agent Identity
[role/tradeoff from agency system]

## Your Task
- **ID:** example-task
- **Title:** Example task
- **Description:** ...

## Verification Required
...

## Discovered Test Files
...

## Context from Dependencies
...

## Required Workflow
[wg log/artifact/done/fail instructions]

## Git Hygiene
...

## Messages
...

## The Graph is Alive
...

## Task Decomposition
...

## Research Before Implementing
...

## Graph Patterns
...

## Reusable Workflow Functions
...

## CRITICAL: Use wg CLI, NOT built-in tools
...

## Additional Context
...

Begin working on the task now.
```

### Native Executor at Task Scope (typical)
Same as above, PLUS this additional section injected after "Context from Dependencies":

```
## wg Usage Guide

**wg (wg)** is a task coordination graph for AI agents...

### Task Lifecycle
Tasks move through: open → in-progress → done / failed / abandoned.

### Core Commands
| Command | Purpose |
|---------|---------|
| wg show <id> | View task details... |
...

### Dependencies with --after
...

### Verification with --verify
...

### When to Decompose vs Implement Directly
...

### Environment Variables
...
```

---

## 9. Summary of Differences

| Aspect | Claude Executor | Native Executor |
|--------|----------------|-----------------|
| Prompt content | `build_prompt()` output | `build_prompt()` output |
| WG Guide section | NOT injected (CLAUDE.md covers this) | INJECTED at Task+ scope |
| Prompt role | User message (piped via stdin) | System prompt (API parameter) |
| User message | The full prompt IS the user message | Short "Complete the task" instruction |
| CLAUDE.md (Full scope) | In prompt AND loaded by harness (double) | In prompt only |
| CLAUDE.md (Task scope) | Loaded by harness only (not in prompt) | NOT present (WG Guide substitutes) |
| Tool source | Claude Code harness | Rust ToolRegistry |
| State injection | Claude Code harness (system-reminders) | Custom StateInjector (system-reminders) |
| Mid-session context | Managed by Claude Code | Journal-based resume + context pressure |
| Exec mode bare | `--system-prompt` + `--tools Bash(wg:*)` | Bundle filters ToolRegistry |
| Exec mode light | `--allowedTools` read-only subset | Bundle filters ToolRegistry |
