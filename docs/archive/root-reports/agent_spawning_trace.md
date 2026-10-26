# Agent Spawning Path Trace

## Executive Summary

This document traces the exact code path from when `wg service` detects a ready task to spawning an agent for different executor types (claude, native, amplifier, shell).

## Main Spawning Flow

### 1. Coordinator Tick Loop
**Location**: `src/commands/service/coordinator.rs:3573` - `coordinator_tick()`

The service coordinator runs in a loop, executing these phases on each tick:

1. **Cleanup dead agents** - `cleanup_and_count_alive()` at line 43
2. **Check max agents limit** - early return if `alive_count >= max_agents`  
3. **Find ready tasks** - Uses cycle-aware ready detection
4. **Process special tasks** - Handles evaluation, assignment, verification tasks
5. **Spawn agents** - Calls `spawn_agents_for_ready_tasks()` at line 3745

### 2. Ready Task Agent Spawning
**Location**: `src/commands/service/coordinator.rs:3165` - `spawn_agents_for_ready_tasks()`

For each ready task that isn't daemon-managed:

```rust
// Skip if already claimed
if task.assigned.is_some() { continue; }

// Skip daemon-managed loop tasks (lines 3189-3192)
if is_daemon_managed(task) { continue; }

// Respawn throttle check (line 3194)
if let Err(reason) = check_respawn_throttle(task, graph_path) {
    // Skip if too many rapid respawns
}

// Circuit breaker check (lines 3200-3209)  
if let Err(reason) = check_spawn_circuit_breaker(task, max_spawn_failures) {
    // Skip if spawn failure threshold exceeded
}
```

Then calls `spawn::spawn_agent()` at line 3359.

### 3. Spawn Agent Entry Point
**Location**: `src/commands/spawn/mod.rs:145` - `spawn_agent()`

Simple wrapper that delegates to the core implementation:

```rust
pub fn spawn_agent(dir: &Path, task_id: &str, executor_name: &str, timeout: Option<&str>, model: Option<&str>) -> Result<(String, u32)> {
    let result = execution::spawn_agent_inner(dir, task_id, executor_name, timeout, model, "coordinator")?;
    Ok((result.agent_id, result.pid))
}
```

### 4. Core Spawn Implementation
**Location**: `src/commands/spawn/execution.rs:30` - `spawn_agent_inner()`

This is where the main spawning logic lives. Key phases:

#### 4.1 Task Claiming (lines 150-210)
```rust
// Atomically claim the task
let mut claim_error: Option<String> = None;
workgraph::parser::modify_graph(graph_path, |graph| {
    // Check task exists and is open
    // Set status to InProgress, assigned to temp_agent_id
})?;
```

#### 4.2 Executor Configuration Loading (lines 204-206)
```rust
let executor_registry = ExecutorRegistry::new(dir);
let executor_config = executor_registry.load_config(executor_name)?;
```

#### 4.3 Model Resolution (lines 213-307)
Resolves the effective model through precedence hierarchy:
- Task-specific model/provider
- Agent preferred model/provider  
- Executor default
- Config default

#### 4.4 Context Injection (lines 312-318)
For native executor:
```rust
if settings.executor_type == "native" {
    scope_ctx.wg_guide_content = super::context::read_wg_guide(dir);
}
```

#### 4.5 Worktree Isolation (lines 288-320)
```rust
let worktree_info = if config.coordinator.worktree_isolation {
    match worktree::create_worktree(project_root, dir, &temp_agent_id, task_id) {
        Ok(info) => Some(info),
        Err(e) => None  // Falls back to shared working directory
    }
} else { None };
```

#### 4.6 Command Building (line 771)
**Location**: `build_inner_command()` function

Routes based on executor type:

```rust
let inner_command = match settings.executor_type.as_str() {
    "claude" if resume_session_id.is_some() => {
        // Resume mode: --resume <session_id>
    }
    "claude" | "codex" => {
        // Standard claude CLI with prompt piped from file
    }
    "amplifier" => {
        // Amplifier CLI with specific flags
    }
    "native" => {
        // wg native-exec with prompt file
    }
    "shell" => {
        // Direct shell execution of task.exec command
    }
    _ => {
        // Custom executor: use configured command + args
    }
}
```

## Executor Type Routing

### Claude Executor 
**Config**: `src/service/executor.rs:1223-1264`

```rust
ExecutorSettings {
    executor_type: "claude",
    command: "claude",
    args: ["--print", "--verbose", "--permission-mode", "bypassPermissions"],
}
```

**Command generation** (`src/commands/spawn/execution.rs:778-935`):
- Uses `claude --print` subprocess 
- Prompt piped from file via `cat prompt.txt | claude --print`
- JSONL stdout captured to `raw_stream.jsonl`
- stderr to `output.log`

### Native Executor
**Config**: `src/service/executor.rs:1304-1332`

```rust
ExecutorSettings {
    executor_type: "native", 
    command: "wg",
    args: ["native-exec"],
}
```

**Command generation** (`src/commands/spawn/execution.rs:960-996`):
```rust
let mut cmd_parts = vec![shell_escape(&settings.command)];
cmd_parts.push("native-exec".to_string());
cmd_parts.push("--prompt-file".to_string());
cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
cmd_parts.push("--exec-mode".to_string());
cmd_parts.push(shell_escape(exec_mode));
```

**Execution** (`src/commands/native_exec.rs:3`):
- Runs Rust-native LLM agent loop in-process
- Tool calls executed via `ToolRegistry` 
- Supports multiple providers (Anthropic, OpenAI-compatible)
- No external subprocess except for tool execution

### Shell Executor
**Config**: `src/service/executor.rs:1264-1280`

```rust
ExecutorSettings {
    executor_type: "shell",
    command: "bash", 
    args: ["-c", "{{task_context}}"],
}
```

**Command generation** (`src/commands/spawn/execution.rs:1010-1020`):
- Direct execution of `task.exec` command
- No LLM involvement - pure shell execution

### Amplifier Executor
**Config**: `src/service/executor.rs:1281-1303`

```rust
ExecutorSettings {
    executor_type: "amplifier",
    command: "amplifier",
    args: ["run", "--mode", "single", "--output-format", "jsonl"],
}
```

## Environment Setup Differences

### Common Environment Variables
All executors receive:
```bash
WG_TASK_ID=<task_id>
WG_AGENT_ID=<agent_id>  
WG_EXECUTOR_TYPE=<executor_type>
WG_MODEL=<effective_model>
WG_TASK_TIMEOUT_SECS=<timeout>
WG_SPAWN_EPOCH=<timestamp>
```

### Worktree Isolation (if enabled)
```bash
WG_WORKTREE_PATH=<worktree_path>
WG_BRANCH=<branch_name>  
WG_PROJECT_ROOT=<project_root>
```

### API Configuration
```bash
WG_ENDPOINT_URL=<endpoint>
WG_API_KEY=<key>
<PROVIDER>_API_KEY=<key>  # e.g., OPENROUTER_API_KEY
```

## Process Isolation Comparison

### Claude Executor
- **Subprocess**: `claude --print` external process
- **Working Directory**: Task working directory (or worktree if enabled)
- **Input**: Prompt piped via stdin
- **Output**: JSONL to stdout, logs to stderr  
- **Tool Execution**: Claude CLI handles tools internally

### Native Executor  
- **Subprocess**: `wg native-exec` (same binary, different subcommand)
- **Working Directory**: Task working directory (or worktree if enabled)
- **Input**: Prompt from file argument
- **Output**: Agent loop writes `stream.jsonl` directly
- **Tool Execution**: Rust ToolRegistry, tools run in-process or via subprocess

### Shell Executor
- **Subprocess**: `bash -c "<command>"`  
- **Working Directory**: Task working directory
- **Input**: Task exec command directly
- **Output**: Command stdout/stderr
- **Tool Execution**: N/A - no LLM tools

## Wrapper Script Generation

All executors get a bash wrapper script at `<agent_dir>/run.sh` that:

1. **Runs the inner command** with timeout
2. **Captures exit code** 
3. **Auto-completes task** on success (calls `wg done`)
4. **Handles merge-back** for worktree isolation (if enabled)
5. **Cleans up resources**

**Location**: `src/commands/spawn/execution.rs:1032` - `generate_wrapper_script()`

The wrapper differs slightly by executor type for output handling:

- **Claude/Codex**: Split JSONL stdout vs stderr  
- **Native**: Agent loop writes stream.jsonl directly
- **Others**: Basic stdout/stderr capture with synthetic stream events

## Summary

The spawning path is executor-agnostic until the command building phase. The key differences are:

1. **Claude**: External subprocess with stdin prompt piping
2. **Native**: Same binary (`wg native-exec`) with file-based prompt
3. **Shell**: Direct command execution, no LLM
4. **Amplifier**: External subprocess with specific flags

All use the same worktree isolation, environment setup, and wrapper script pattern.