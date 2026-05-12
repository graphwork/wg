# Executor Weight Tiers: Right-Sizing Agent Execution

## Status: Design (March 2026)

## Problem Statement

Every task currently spawns a full Claude Code session — tool access, context loading, skill injection, the full stack. For tasks like "check if CI is green" or "assign this task to an agent," that's massively over-provisioned. We burn full sessions (and full cost) on things that could be a bare LLM prompt or even a shell script.

The system already has a partial answer — `exec_mode: bare` restricts tools to `Bash(wg:*)` and uses `--system-prompt` instead of piping a full prompt. But the current binary (full/bare) model is too coarse. There's a spectrum of execution needs, and we should let the system right-size each task.

## Current State Analysis

### What exists today

| Mechanism | Location | Purpose |
|-----------|----------|---------|
| `exec_mode` field | `graph.rs:248` | Binary: `full` (default) or `bare` |
| `context_scope` | `context_scope.rs` | Controls prompt context: clean < task < graph < full |
| Executor types | `executor.rs` | Named executors: `claude`, `shell`, `amplifier`, `default` |
| Agent `executor` field | `agency/types.rs:308` | Agent-level executor preference (default: "claude") |
| `task.exec` field | `graph.rs` | Shell command for `shell` executor |
| Eval inline spawn | `coordinator.rs:772` | Evaluations fork `wg evaluate` directly, bypassing the spawn stack |

### How `bare` mode works (`execution.rs:373-412`)

When `exec_mode == "bare"`:
- Uses `--system-prompt` to pass prompt (vs piping to stdin)
- Restricts tools to `Bash(wg:*)` — only wg CLI
- Pipes task title + description as user message
- Still uses Claude Code as executor (full session overhead for launching)

### How `shell` executor works

- Requires `task.exec` command
- Runs `bash -c <exec_command>` directly
- No LLM involved at all
- Already used: eval tasks use `spawn_eval_inline` which forks `wg evaluate run` directly

### How executor resolution works (`coordinator.rs:993-1002`)

```
effective_executor = if task.exec.is_some() {
    "shell"
} else {
    agent.executor > config.coordinator.executor
}
```

## Design: Unified Execution Weight System

### Core Insight

`exec_mode` and executor type are already two orthogonal dimensions that partially solve this problem. Rather than inventing a wholly new weight tier concept, we should:

1. **Expand `exec_mode`** from binary (full/bare) to a proper tier enum
2. **Let the coordinator + assigner set exec_mode automatically** based on task characteristics
3. **Add a `light` tier** between bare and full that gives read-only file access

### Tier Definitions

| Tier | exec_mode value | Executor | Tools | Context | Use Cases |
|------|----------------|----------|-------|---------|-----------|
| **shell** | `"shell"` | bash | None (shell only) | task.exec command | CI checks, git ops, test runs, `wg evaluate` |
| **bare** | `"bare"` | claude --system-prompt | `Bash(wg:*)` | Clean/Task scope | Synthesis, triage, assignment, review critique |
| **light** | `"light"` | claude --allowedTools | `Bash(wg:*),Read,Glob,Grep` | Task/Graph scope | Research, code review, analysis, exploration |
| **full** | `"full"` / None | claude (default) | All tools | Task/Graph/Full scope | Implementation, debugging, refactoring |

### Why not a separate "weight" field?

Adding a new field creates ambiguity: what happens when `exec_mode: bare` conflicts with a hypothetical `weight: full`? The existing `exec_mode` field already captures this concept. Expanding its value space is simpler and backward compatible — `None` defaults to `"full"`, and existing `"bare"` tasks keep working.

### Changes Required

#### 1. Expand `exec_mode` validation (`add.rs`, `edit.rs`)

Currently:
```rust
// add.rs:91-94
match mode {
    "full" | "bare" => {}
    _ => anyhow::bail!("Invalid exec_mode '{}'. Valid values: full, bare", mode),
}
```

Change to:
```rust
match mode {
    "full" | "light" | "bare" | "shell" => {}
    _ => anyhow::bail!("Invalid exec_mode '{}'. Valid values: full, light, bare, shell", mode),
}
```

#### 2. Add `light` mode execution path (`execution.rs`)

In `build_inner_command`, add a `"claude" if is_light` branch between `is_bare` and the default full path:

```rust
"claude" if is_light => {
    // Light mode: read-only file tools + wg CLI
    let prompt_file = output_dir.join("prompt.txt");
    // ... write prompt ...
    let mut cmd_parts = vec![shell_escape(&settings.command)];
    cmd_parts.push("--print".to_string());
    cmd_parts.push("--verbose".to_string());
    cmd_parts.push("--output-format".to_string());
    cmd_parts.push("stream-json".to_string());
    cmd_parts.push("--allowedTools".to_string());
    cmd_parts.push(shell_escape("Bash(wg:*),Read,Glob,Grep,WebFetch"));
    // ... model flag ...
    prompt_file_command(&prompt_file.to_string_lossy(), &claude_cmd)
}
```

Key difference from bare: light uses `--allowedTools` (explicit allowlist) while keeping the standard prompt-via-stdin flow (not `--system-prompt`). This gives agents read access to the codebase for research/review tasks without write access.

#### 3. Coordinator executor resolution should respect `exec_mode: shell`

Currently, `exec_mode` only affects *how* the Claude executor runs — it doesn't change *which* executor is used. When `exec_mode == "shell"`, the task should use the shell executor even without `task.exec`:

```rust
// coordinator.rs spawn_agents_for_ready_tasks
let effective_executor = if task.exec.is_some() || task.exec_mode.as_deref() == Some("shell") {
    "shell".to_string()
} else {
    // agent.executor > config.coordinator.executor
    task.agent.as_ref()
        .and_then(|h| find_agent_by_prefix(&agents_dir, h).ok())
        .map(|a| a.executor)
        .unwrap_or_else(|| executor.to_string())
};
```

For shell exec_mode without `task.exec`, the shell executor would need a reasonable default (perhaps use the task description as the command, or fail with a clear error).

#### 4. Assigner should set exec_mode

The assignment task instructions (`coordinator.rs:416-452`) already tell the assigner to set `context_scope`. Add parallel guidance for `exec_mode`:

```
### Step 6c: Set Execution Weight

After assigning the agent, determine the appropriate execution weight (exec_mode)
for the task. This controls what tools the spawned agent has access to.

- **shell**: No LLM. Task has `exec` command that runs directly. For: CI checks,
  test runs, git operations, simple scripts.
  Signals: task.exec is set, task description says "run", "check", "verify status"

- **bare**: LLM with wg CLI only. No file access. For: synthesis, triage,
  summarization, assignment, abstract reasoning, critique.
  Signals: task doesn't need to read/write files, task is about decision-making
  or text generation

- **light**: LLM with read-only file access. For: research, code review,
  exploration, analysis, documentation review.
  Signals: task needs to read code but not modify it, task is tagged "research"
  or "review"

- **full** (default): Full Claude Code session. For: implementation, debugging,
  refactoring, test writing, any task that modifies files.
  Signals: task creates or modifies code/files

Set the exec_mode (skip if `full` is appropriate):
```
wg edit <task-id> --exec-mode <mode>
```
```

#### 5. Role-level default exec_mode (`agency/types.rs`)

Roles already have `default_context_scope`. Add a parallel `default_exec_mode`:

```rust
pub struct Role {
    // ... existing fields ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_context_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_exec_mode: Option<String>,
}
```

Resolution hierarchy (mirrors context_scope):
```
task.exec_mode > role.default_exec_mode > "full"
```

#### 6. Display in `wg show` (`show.rs`)

Already handled — `show.rs:232` displays `exec_mode` when set. No changes needed.

### What NOT to change

- **The coordinator stays generic.** It doesn't need to know about weight tiers. The executor adapter (`build_inner_command` in `execution.rs`) handles the tier-specific logic.
- **No new ExecutorConfig types.** Weight tiers are handled within the existing Claude executor via `--allowedTools` and `--system-prompt` flags.
- **No cost tracking changes needed yet.** `TokenUsage` already captures cost per task. Comparing cost-per-tier is a report/query concern, not a schema change.

### Escalation: light → full

If a `light` executor discovers it needs to write files, it should:

1. Log a message: `wg log <task-id> "Need write access, creating implementation subtask"`
2. Create a subtask with `exec_mode: full`: `wg add "Implement: ..." --after <task-id> --exec-mode full`
3. Complete the research task with findings

This is already the natural decomposition pattern. No special escalation mechanism needed.

## Implementation Plan

### Phase 1: Expand exec_mode validation (small, safe)
- **Files:** `src/commands/add.rs`, `src/commands/edit.rs`
- **Change:** Accept "light" and "shell" as valid exec_mode values
- **Risk:** None — additive validation change

### Phase 2: Implement light mode execution path
- **Files:** `src/commands/spawn/execution.rs`
- **Change:** Add `is_light` branch in `build_inner_command` with `--allowedTools` restriction
- **Risk:** Low — parallel to existing `is_bare` path

### Phase 3: Coordinator exec_mode awareness
- **Files:** `src/commands/service/coordinator.rs`
- **Change:** Respect `exec_mode: shell` in executor resolution
- **Risk:** Low — extends existing `task.exec.is_some()` check

### Phase 4: Assigner exec_mode guidance
- **Files:** `src/commands/service/coordinator.rs` (assignment task description)
- **Change:** Add Step 6c to assigner instructions
- **Risk:** None — prompt text change only

### Phase 5: Role default_exec_mode
- **Files:** `src/agency/types.rs`, `src/commands/spawn/context.rs` (resolve_task_scope equivalent)
- **Change:** Add `default_exec_mode` to Role, wire into resolution hierarchy
- **Risk:** Low — mirrors existing `default_context_scope` pattern

### Phase 6: Documentation
- **Files:** `docs/AGENT-GUIDE.md`, `CLAUDE.md`, skill docs
- **Change:** Document weight tiers and when to use each

## Cost Impact Estimate

| Tier | Estimated cost per task | Speedup vs full |
|------|------------------------|-----------------|
| shell | ~$0 | ~instant |
| bare | ~$0.02-0.05 | 5-10x faster |
| light | ~$0.05-0.15 | 2-5x faster |
| full | ~$0.10-0.50+ | baseline |

Primary savings come from:
- Assignment tasks → bare (currently full)
- Evaluation tasks → already optimized (inline spawn)
- Research tasks → light
- CI/test verification → shell

## Open Questions

1. **Should the assigner itself run as bare?** Assignment tasks are pure-reasoning + `wg` CLI. They don't need file access. Setting `exec_mode: bare` on auto-created assignment tasks would immediately save cost. *Recommendation: yes, do this in Phase 4.*

2. **Should `--allowedTools` for light mode include `WebFetch`?** Research tasks may benefit from web access. *Recommendation: include it — it's read-only.*

3. **What about Amplifier executor?** Amplifier has its own bundle scoping mechanism. Weight tiers could map to different bundles, but that's executor-specific and can be handled in the amplifier adapter. *Recommendation: defer — focus on Claude Code first.*

4. **Should we auto-detect exec_mode from task characteristics?** The coordinator could heuristically set exec_mode based on task tags, skills, and description patterns — but this risks misclassification. *Recommendation: let the assigner decide (it already evaluates task characteristics), and provide role defaults as fallback.*
