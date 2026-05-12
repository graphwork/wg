# Agent Spawning Path Analysis

## Overview

This document traces the full code path from task detection to agent spawning for different executor types in the wg system.

## High-Level Spawning Flow

The agent spawning process follows this general sequence:

1. **Task Detection**: `ready_tasks_with_peers_cycle_aware()` identifies ready tasks
2. **Spawning Decision**: `spawn_agents_for_ready_tasks()` processes ready tasks
3. **Agent Creation**: `spawn_agent_inner()` handles the core spawning logic
4. **Process Launch**: Different executor types use different command construction and execution strategies

## Detailed Code Paths

### 1. Task Detection Phase

**File**: `src/commands/service/coordinator.rs`  
**Function**: `coordinator_tick()`

```rust
// Line ~159: Check for ready tasks
let cycle_analysis = graph.compute_cycle_analysis();
let ready = ready_tasks_with_peers_cycle_aware(graph, dir, &cycle_analysis);
let spawnable_count = ready.iter().filter(|t| !is_daemon_managed(t)).count();
```

**Key Points**:
- Tasks must be in `Open` or `Blocked` status
- Dependency resolution through cycle-aware analysis
- Daemon-managed tasks (compact-loop, archive-loop) are excluded
- Respects `max_agents` concurrency limit

### 2. Agent Spawning Orchestration

**File**: `src/commands/service/coordinator.rs`  
**Function**: `spawn_agents_for_ready_tasks()`

```rust
// Line ~3165: Main spawning loop
for task in &final_ready {
    // Skip daemon-managed loop tasks
    if is_daemon_managed(task) { continue; }
    
    // Check respawn throttling
    if let Err(reason) = check_respawn_throttle(task, &gp) {
        // Fail task if too many rapid respawns
        continue;
    }
    
    // Determine effective executor
    let effective_executor = /* executor resolution logic */;
    
    // Spawn the agent
    match spawn::spawn_agent(dir, &task.id, &effective_executor, timeout, model) {
        Ok((agent_id, pid)) => { /* success */ }
        Err(e) => { /* handle failure */ }
    }
}
```

### 3. Core Spawning Logic

**File**: `src/commands/spawn/execution.rs`  
**Function**: `spawn_agent_inner()`

This is where the main differences between executor types emerge:

#### Phase 1: Context Assembly
```rust
// Line ~116: Resolve context scope and build context
let scope = resolve_task_scope(task, &config, dir);
let task_context = build_task_context(&graph, task);
let mut scope_ctx = build_scope_context(&graph, task, scope, &config, dir);
```

#### Phase 2: Model and Provider Resolution
```rust
// Line ~213: Unified model + provider resolution
let resolved = resolve_model_and_provider(
    task_model.clone(),
    task_provider.clone(),
    agent_preferred_model,
    agent_preferred_provider.clone(),
    executor_config.executor.model.clone(),
    /* ... more parameters ... */
);
```

#### Phase 3: Worktree Isolation (Optional)
```rust
// Line ~288: Create isolated worktree if enabled
let worktree_info = if config.coordinator.worktree_isolation {
    match worktree::create_worktree(project_root, dir, &temp_agent_id, task_id) {
        Ok(info) => Some(info),
        Err(e) => { /* handle failure */ }
    }
} else { None };
```

#### Phase 4: Executor-Specific Command Building
```rust
// Line ~432: Build the inner command
let inner_command = build_inner_command(
    &settings,
    exec_mode,
    &output_dir,
    &effective_model,
    &effective_provider,
    /* ... other parameters ... */
)?;
```

## Executor-Specific Spawning Patterns

### Claude Executor (`executor_type: "claude"`)

**Command Construction** (`src/commands/spawn/execution.rs:800-933`):

```rust
"claude" => {
    let mut cmd_parts = vec![shell_escape(&settings.command)];
    for arg in &settings.args {
        cmd_parts.push(shell_escape(arg));
    }
    // Prevent agents from spawning sub-agents outside wg
    cmd_parts.push("--disallowedTools".to_string());
    cmd_parts.push(shell_escape("Agent"));
    cmd_parts.push("--disable-slash-commands".to_string());
    
    // Add model flag if specified
    if let Some(m) = effective_model {
        cmd_parts.push("--model".to_string());
        cmd_parts.push(shell_escape(m));
    }
    
    // Write prompt to file and pipe to claude
    if let Some(ref prompt_template) = settings.prompt_template {
        let prompt_file = output_dir.join("prompt.txt");
        fs::write(&prompt_file, &prompt_template.template)?;
        prompt_file_command(&prompt_file.to_string_lossy(), &claude_cmd)
    }
}
```

**Context Injection**:
- Prompt assembled via `build_prompt()` with full scope context
- Uses CLAUDE.md for project instructions
- Environment variables: `WG_TASK_ID`, `WG_AGENT_ID`, `WG_MODEL`, etc.

**Execution Modes**:
- **Full mode**: All Claude Code tools enabled
- **Light mode**: Read-only tools + wg CLI (`--allowedTools "Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch"`)
- **Bare mode**: Minimal execution with system prompt (`--tools "Bash(wg:*)"`)
- **Resume mode**: Continue from checkpoint (`--resume <session_id>`)

### Native Executor (`executor_type: "native"`)

**Command Construction** (`src/commands/spawn/execution.rs:989-1031`):

```rust
"native" => {
    let mut cmd_parts = vec![shell_escape(&settings.command)];
    cmd_parts.push("native-exec".to_string());
    cmd_parts.push("--prompt-file".to_string());
    cmd_parts.push(shell_escape(&prompt_file.to_string_lossy()));
    cmd_parts.push("--exec-mode".to_string());
    cmd_parts.push(shell_escape(exec_mode));
    cmd_parts.push("--task-id".to_string());
    cmd_parts.push(shell_escape(&vars.task_id));
    
    // Model/provider/endpoint parameters
    if let Some(m) = effective_model {
        cmd_parts.push("--model".to_string());
        cmd_parts.push(shell_escape(m));
    }
    // ... additional parameters ...
}
```

**Context Injection**:
- Explicit wg usage guide injection (`scope_ctx.wg_guide_content = read_wg_guide(dir)`)
- Prompt written to file and passed as `--prompt-file`
- Tool registry filtered based on execution mode bundle

**Execution Process** (`src/commands/native_exec.rs`):
```rust
// Line ~60: Build tool registry and apply bundle filtering
let mut registry = ToolRegistry::default_all(workgraph_dir, &working_dir);
if let Some(bundle) = resolve_bundle(exec_mode, workgraph_dir) {
    registry = bundle.filter_registry(registry);
}

// Line ~95: Create LLM provider
let client = create_provider_ext(
    workgraph_dir,
    &effective_model,
    effective_provider.as_deref(),
    effective_endpoint.as_deref(),
    api_key,
)?;

// Line ~136: Create and run agent loop
let mut agent = AgentLoop::with_tool_support(
    client, registry, system_prompt, max_turns, output_log, supports_tools
)
.with_journal(journal_path, task_id.to_string())
.with_resume(!no_resume);
```

### Amplifier Executor (`executor_type: "amplifier"`)

**Command Construction** (`src/commands/spawn/execution.rs:957-988`):

```rust
"amplifier" => {
    let mut cmd_parts = vec![shell_escape(&settings.command)];
    for arg in &settings.args {
        cmd_parts.push(shell_escape(arg));
    }
    
    // Handle provider:model format
    if let Some(m) = effective_model {
        if let Some((provider, model)) = m.split_once(':') {
            cmd_parts.push("-p".to_string());
            cmd_parts.push(shell_escape(provider));
            cmd_parts.push("-m".to_string());
            cmd_parts.push(shell_escape(model));
        } else {
            cmd_parts.push("-m".to_string());
            cmd_parts.push(shell_escape(m));
        }
    }
}
```

### Shell Executor (`executor_type: "shell"`)

**Command Construction** (`src/commands/spawn/execution.rs:1032-1040`):

```rust
"shell" => {
    format!(
        "{} -c {}",
        shell_escape(&settings.command),
        shell_escape(task_exec.as_ref().ok_or_else(|| {
            anyhow::anyhow!("shell executor requires task exec command")
        })?)
    )
}
```

**Key Differences**:
- Requires task to have `exec` command field
- No AI agent - just executes shell command directly
- Uses task's `exec` field as the command to run

## Environment Variable Injection

**Common Environment Variables** (all executors):
```bash
WG_TASK_ID=<task-id>
WG_AGENT_ID=<agent-id>
WG_EXECUTOR_TYPE=<executor-type>
WG_USER=<current-user>
WG_SPAWN_EPOCH=<unix-timestamp>
```

**Model/Provider Variables**:
```bash
WG_MODEL=<effective-model>
WG_LLM_PROVIDER=<provider>
WG_ENDPOINT=<endpoint-name>
WG_ENDPOINT_URL=<endpoint-url>
WG_API_KEY=<api-key>
# Provider-specific: OPENROUTER_API_KEY, ANTHROPIC_API_KEY, etc.
```

**Worktree Variables** (when worktree isolation enabled):
```bash
WG_WORKTREE_PATH=<worktree-path>
WG_BRANCH=<branch-name>
WG_PROJECT_ROOT=<project-root>
```

## Key Differences Between Executor Types

### Prompt Construction
- **Claude**: Uses scope-based prompt assembly with CLAUDE.md integration
- **Native**: Explicit wg guide injection + bundle-filtered tool registry
- **Amplifier/Shell**: Basic template substitution

### Tool Access
- **Claude**: Tool restrictions via `--allowedTools`/`--disallowedTools` flags
- **Native**: Tool filtering via bundle system (`resolve_bundle()`)
- **Amplifier**: Depends on amplifier's own tool system
- **Shell**: No AI tools, just command execution

### Process Management
- **All**: Use wrapper scripts for lifecycle management
- **All**: Support timeout via `timeout` command
- **All**: Automatic task completion/failure via wrapper script

### Context Scope Handling
- **Claude**: Full scope context in prompt assembly
- **Native**: Explicit injection of required context sections
- **Amplifier**: Standard template variable substitution
- **Shell**: No context injection needed

## Wrapper Script Generation

**File**: `src/commands/spawn/execution.rs:1052+`

All executors get wrapped by a shell script that:
1. Runs the inner command with timeout
2. Captures exit codes and handles timeouts
3. Automatically calls `wg done` or `wg fail` based on result
4. Handles worktree merging and cleanup (if enabled)
5. Provides logging and error reporting

## Summary

The agent spawning system provides a unified interface while allowing executor-specific behavior through:

1. **Command Construction**: Each executor type builds its command line differently
2. **Context Injection**: Different strategies for providing context to agents  
3. **Tool Access**: Executor-specific tool filtering and access control
4. **Environment Setup**: Common environment variables plus executor-specific additions
5. **Process Management**: Shared wrapper script pattern for lifecycle management

The system achieves flexibility while maintaining consistency in agent lifecycle management, logging, and error handling across all executor types.
