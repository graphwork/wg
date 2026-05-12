# Prompt Construction Analysis: Claude vs Native Executors

## Executive Summary

This analysis examines the prompt construction differences between "claude" and "native" executor types in the wg system. **Key finding**: Both executors use the same core `build_prompt()` function, but differ in specific context injection and command invocation patterns.

## Core Prompt Assembly

### Shared Foundation
Both claude and native executors use the same prompt assembly process when no custom `prompt_template` is defined:

**File**: `src/commands/spawn/execution.rs:322-329`
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

The `build_prompt()` function in `src/service/executor.rs:709` assembles prompts with these sections:
- System awareness preamble (Full scope)
- Skills preamble  
- Task assignment header
- Agent identity
- Task details & verification criteria
- Context from dependencies
- Workflow sections (git hygiene, message polling, etc.)

## Critical Differences

### 1. WG CLI Documentation Injection

**Native executors get explicit wg CLI documentation:**

**File**: `src/commands/spawn/execution.rs:315-317`
```rust
if settings.executor_type == "native" {
    scope_ctx.wg_guide_content = super::context::read_wg_guide(dir);
}
```

**File**: `src/service/executor.rs:799-805`
```rust
// Task+ scope: wg usage guide for non-Claude models
if scope >= ContextScope::Task && !ctx.wg_guide_content.is_empty() {
    parts.push(format!(
        "## wg Usage Guide\n\n{}",
        ctx.wg_guide_content
    ));
}
```

The injected content (`DEFAULT_WG_GUIDE` in `src/service/executor.rs:535-597`) includes:
- Task lifecycle explanation
- Core commands table (`wg show`, `wg log`, `wg done`, etc.)
- Dependencies with `--after` 
- Verification with `--verify`
- Decomposition guidance
- Environment variables

**Claude executors get CLAUDE.md instead:**

**File**: `src/service/executor.rs:845-851`
```rust
// Full scope: CLAUDE.md content
if scope >= ContextScope::Full && !ctx.claude_md_content.is_empty() {
    parts.push(format!(
        "## Project Instructions (CLAUDE.md)\n\n{}",
        ctx.claude_md_content
    ));
}
```

### 2. State Injection Systems

**Native executors have dynamic state injection:**

**File**: `src/executor/native/state_injection.rs`

The native executor includes `StateInjector` that provides ephemeral `<system-reminder>` blocks during agent execution with:
- Pending messages from other agents/coordinator
- Graph state changes (dependency completions) 
- Context pressure warnings
- Time budget awareness

**Claude executors have no equivalent state injection system.**

### 3. Command Invocation Patterns

**Native Executor Flow:**
1. Prompt written to `prompt.txt`
2. Invoked via: `wg native-exec --prompt-file prompt.txt --exec-mode <mode> --task-id <id>`
3. Runs agent loop in-process via Rust native implementation

**Claude Executor Flow:**
1. Prompt written to `prompt.txt` 
2. Invoked via: `claude <args>` with prompt piped from file
3. Multiple execution modes:
   - **Full mode**: `--disallowedTools Agent --disable-slash-commands`
   - **Light mode**: `--allowedTools <restricted-set>`  
   - **Bare mode**: `--system-prompt` with lightweight execution
   - **Resume mode**: `--resume <session-id>` for checkpoint recovery

### 4. Bundle Integration

Both executors support bundle-based tool filtering, but only native executors can append `system_prompt_suffix` from bundles:

**File**: `src/commands/native_exec.rs:64-77`
```rust
let system_suffix = if let Some(bundle) = resolve_bundle(exec_mode, workgraph_dir) {
    let suffix = bundle.system_prompt_suffix.clone();
    registry = bundle.filter_registry(registry);
    suffix
} else {
    String::new()
};

let system_prompt = if system_suffix.is_empty() {
    prompt
} else {
    format!("{}\n\n{}", prompt, system_suffix)
};
```

## File Location Summary

| Component | File Path | Key Lines |
|-----------|-----------|-----------|
| Executor type conditionals | `src/commands/spawn/execution.rs` | 315-317, 323-325 |
| Core prompt assembly | `src/service/executor.rs` | 709-855 |
| Native executor entry point | `src/commands/native_exec.rs` | 40, 64-77 |
| State injection (native only) | `src/executor/native/state_injection.rs` | Full file |
| Claude command construction | `src/commands/spawn/execution.rs` | 772-904 |
| WG guide content | `src/service/executor.rs` | 535-597 |
| WG guide injection logic | `src/commands/spawn/context.rs` | 561-569 |

## Key Architectural Insights

1. **Claude agents are "CLAUDE.md native"** - they receive project-specific instructions from CLAUDE.md files
2. **Native agents are "wg native"** - they receive explicit CLI documentation through DEFAULT_WG_GUIDE
3. **State management differs fundamentally** - native agents get live state updates, claude agents operate more statically  
4. **Bundle system provides executor-specific prompt augmentation** for specialized exec_modes

This analysis reveals that while both executors share core prompt assembly logic, they serve different architectural roles: Claude for general-purpose AI interaction with project context, and Native for wg-optimized task execution with live coordination features.