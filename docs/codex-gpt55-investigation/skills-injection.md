# codex gpt-5.5: Skills / System-Prompt Injection Investigation

**Task:** `codex-research-skills`  
**Date:** 2026-05-06  
**Question:** Does codex CLI receive the `wg agent-guide` universal worker-agent contract
(validation requirements, smoke-gate, no-built-in-Task-tool rules, completion contract)
the same way claude agents do? If not, what is the gap and how can it be fixed?

---

## Does claude get `wg agent-guide` injected?  YES — two separate paths

### Path 1: Worker tasks dispatched by daemon (`wg spawn claude`)

File: `src/commands/spawn/execution.rs`

1. `spawn_agent_inner` builds a `ScopeContext`
2. Line 401: `scope_ctx.wg_guide_content = super::context::build_tiered_guide(dir, model_tier, model_str)`
   — called for ALL executor types including claude and codex
3. `classify_model_tier` in `src/commands/spawn/context.rs:615` maps model strings to tiers:
   - `claude-sonnet-4-6` matches `"claude-sonnet"` → `KnowledgeTier::Full`
   - `claude-opus-4-7` matches `"claude-opus"` → `KnowledgeTier::Full`
   - `claude-haiku-*` matches `"claude-haiku"` → `KnowledgeTier::Core`
4. For `Full` tier, `build_full_guide()` produces ~40 KB of guidance including:
   - Decomposition pattern templates
   - Task lifecycle requirements (`wg done`, `wg fail`, smoke gate)
   - `## Validation` section convention
   - Agent communication protocol
   - Advanced graph patterns
   - The CLAUDE.md content (via `read_claude_md_content()` in `build_essential_guide`)
5. `build_prompt()` injects this as a `"## workgraph Usage Guide"` section at Task scope+
   (`service/executor.rs:1078-1082`)
6. The assembled prompt is piped as stdin to `claude --print`

### Path 2: `CLAUDE.md` file read by Claude CLI automatically

Claude Code reads `CLAUDE.md` from the project root at session start. The `CLAUDE.md`
in every workgraph project (written by `wg init` / `wg setup`) contains:

```
Run `wg agent-guide` at session start (or read its output from a previous
session) to get the universal role contract: ...
```

This instructs the claude agent to run `wg agent-guide` as a bash command, which
prints `AGENT_GUIDE_TEXT` (`src/text/agent_guide.md` — the canonical universal
contract with smoke-gate, no-built-in-Task-tool rules, etc.).

**Net result for claude:** Full tiered guide in spawn prompt + CLAUDE.md pointer that
leads to `AGENT_GUIDE_TEXT` if the agent follows the instruction.

---

## Does codex get `wg agent-guide` injected?  PARTIALLY — with critical gaps

### Path 1: Worker tasks dispatched by daemon (`wg spawn codex`)

Same `spawn_agent_inner` → `build_tiered_guide()` call as above. BUT:

`classify_model_tier("gpt-5.5")`:
- Does not match `"minimax"`, `"qwen-2.5"`, `"qwen2.5"` → not Essential via those checks
- Does not match `"deepseek"`, `"claude-haiku"` → not Core
- Does not match `"llama-3.1"`, `"llama3.1"`, `"claude-sonnet"`, `"claude-opus"` → not Full
- **Falls through to `KnowledgeTier::Essential`** (conservative default, line 638)

`KnowledgeTier::Essential` → `build_essential_guide()` → a compact ~8 KB guide that
covers core `wg` commands and decomposition patterns. It does NOT include:
- The `AGENT_GUIDE_TEXT` "STOP" banner with the three-roles contract
- Smoke gate contract (`wg done` refusing on failing scenarios)
- The "no built-in Task tool" warning (`TaskCreate` / `Task tool`)
- The full completion contract

The assembled prompt is piped as stdin to `codex exec --json --skip-git-repo-check
--dangerously-bypass-approvals-and-sandbox`. The prompt uses
`executor_uses_auto_prompt("codex")` = `true` (`execution.rs:909`), so the guide IS
included — just at the wrong (minimal) tier.

### Path 2: `AGENTS.md` file potentially read by Codex CLI

The codex executor runs from the project root (or worktree root when worktrees are
enabled). The project root contains `AGENTS.md` which is byte-for-byte identical to
`CLAUDE.md` (both written by `wg init`; parity enforced by a regression test in
`agent_guide.rs:133`).

However, the codex executor uses `--skip-git-repo-check`. Whether codex CLI reads
`AGENTS.md` when this flag is set is **not confirmed by source inspection** (codex CLI
is an external binary). The flag is documented as bypassing git repository validation,
not as suppressing project-context loading.

Even if `AGENTS.md` IS read by codex CLI, it only contains a pointer:
```
Run `wg agent-guide` at session start (or read its output...)
```
The full `AGENT_GUIDE_TEXT` is not inlined. An agent that does not proactively run
`wg agent-guide` misses the complete contract.

### Codex chat/handler sessions (`wg spawn-task`)

For coordinator/chat sessions dispatched via `wg spawn-task .coordinator-N` or
`.chat-N`, the code path goes through `src/commands/codex_handler.rs`.

`build_handler_system_prompt` for non-coordinator codex sessions:
```rust
} else {
    String::from("You are a workgraph task agent.")
}
```
(`codex_handler.rs:231-239`)

This is **extremely minimal**. No wg guide, no validation contract, no smoke gate.

The `CODEX_CHAT_ADDENDUM` (a loud "STOP — You Are A Chat Agent" section) is injected
only for coordinator sessions where `coordinator_id.is_some()`. Worker-type chat
sessions get the minimal string.

---

## Summary: claude vs codex injection comparison

| Mechanism | claude | codex |
|-----------|--------|-------|
| Tiered guide in spawn prompt | Yes — Full tier (`build_full_guide`) | Yes — **Essential tier only** (model not in classify_model_tier) |
| `AGENT_GUIDE_TEXT` in spawn prompt | No (only CLAUDE.md pointer in tiered guide) | No (only AGENTS.md pointer in tiered guide, if included at all) |
| CLAUDE.md / AGENTS.md automatic read | Yes — Claude CLI reads CLAUDE.md always | Uncertain — depends on `--skip-git-repo-check` behavior |
| System prompt for non-coordinator sessions | `"You are a workgraph task agent."` (same stub) | `"You are a workgraph task agent."` (same stub) |
| Completion contract injected | Yes — via Full tiered guide + REQUIRED_WORKFLOW_SECTION | Partially — via Essential tiered guide (shorter) |
| Smoke gate contract | Yes — in Full guide | **No — not present in Essential guide** |
| Validation section requirement | Yes | **Partial — mentioned in Essential guide but without detail** |
| No-built-in-Task-tool warning | Yes | **No — not in Essential guide** |

---

## The core gap

`gpt-5.5` is classified `KnowledgeTier::Essential` because `classify_model_tier` in
`src/commands/spawn/context.rs:615-640` only recognizes specific model name fragments.
GPT family models hit the catch-all `KnowledgeTier::Essential`. This means codex
workers receive the minimum guide, missing the smoke gate, validation section
requirements, and the no-built-in-Task-tool rules that claude workers receive.

Additionally, even if `AGENTS.md` is read automatically by codex CLI, it only says
"run `wg agent-guide`" — it does not contain the contract inline. Codex agents that
do not proactively run the command miss the full contract. Claude agents are more
likely to follow the CLAUDE.md pointer because the Claude CLI UI surfaces it
prominently at session start.

---

## Proposed injection mechanisms (ranked by feasibility)

### 1. Add gpt-5.5 (and GPT family) to `KnowledgeTier::Full` in `classify_model_tier`
**Feasibility: High — one-line change**

File: `src/commands/spawn/context.rs:630-640`

```rust
// Tier 3: Full (40KB) — 128K+ context window models
else if model_lower.contains("llama-3.1")
    || model_lower.contains("llama3.1")
    || model_lower.contains("claude-sonnet")
    || model_lower.contains("claude-opus")
    || model_lower.contains("gpt-5")    // ← add this
    || model_lower.contains("gpt-4")    // ← add this
{
    KnowledgeTier::Full
}
```

This ensures codex gpt-5.5 workers receive the same Full guide as claude sonnet/opus
workers. Cheap to implement; immediately effective.

### 2. Inject `AGENT_GUIDE_TEXT` into `build_tiered_guide` output for codex models
**Feasibility: Medium**

File: `src/commands/spawn/context.rs`

In `build_tiered_guide`, detect when `_model` is a codex-family model and prepend
`crate::commands::agent_guide::AGENT_GUIDE_TEXT` to the guide output. This inlines
the universal contract directly in the spawn prompt, bypassing the `AGENTS.md` pointer
chain entirely.

Advantage: AGENTS.md-reading behavior of codex CLI becomes irrelevant.
Cost: Increases prompt size (~20 KB); could hit context limits for Essential/Core models.

### 3. Inject `AGENT_GUIDE_TEXT` in `codex_handler.rs` for worker sessions
**Feasibility: Medium**

File: `src/commands/codex_handler.rs`

In `assemble_first_turn_prompt`, for non-coordinator sessions (worker tasks that go
through the handler path), prepend `AGENT_GUIDE_TEXT` before the system prompt, similar
to how `CODEX_CHAT_ADDENDUM` is prepended for chat sessions.

```rust
fn assemble_first_turn_prompt(...) -> String {
    let mut out = String::new();
    if coordinator_id.is_none() {
        // Worker sessions: inject universal contract
        out.push_str(crate::commands::agent_guide::AGENT_GUIDE_TEXT);
        out.push_str("\n---\n\n");
    } else {
        out.push_str(CODEX_CHAT_ADDENDUM);
    }
    ...
}
```

Note: this only helps the chat-handler path (`wg spawn-task` for `.coordinator-N`/`.chat-N`).
Worker tasks dispatched by the daemon go through `spawn_agent_inner`, not `codex_handler`.

### 4. Pass `AGENTS.md` path explicitly to codex CLI
**Feasibility: Low-medium — depends on codex CLI feature support**

If the codex CLI version in use supports a flag like `--project-doc` or `--context-file`,
the codex executor config could be updated to explicitly pass `AGENTS.md` as a context
document, bypassing the `--skip-git-repo-check` concern.

Check `codex exec --help` for available flags. If supported, update
`src/service/executor.rs` (the built-in codex executor config) to add the flag.

---

## File and line references for claude injection path

| Component | File | Lines | What it does |
|-----------|------|--------|--------------|
| Tiered guide builder | `src/commands/spawn/context.rs` | 670-690 | Routes by model tier |
| Model tier classifier | `src/commands/spawn/context.rs` | 614-641 | `gpt-5.5` → Essential (gap) |
| Guide injection point | `src/commands/spawn/execution.rs` | 399-401 | Sets `wg_guide_content` |
| Prompt assembly | `src/service/executor.rs` | 1077-1083 | Injects guide as section |
| `AGENT_GUIDE_TEXT` source | `src/text/agent_guide.md` | all | Universal contract |
| CLAUDE.md writer | `src/commands/setup.rs` | 20-46, 419-430 | Writes AGENTS.md+CLAUDE.md pointer |
| Claude handler system prompt | `src/commands/claude_handler.rs` | 348-356 | `build_handler_system_prompt` |
| Codex handler system prompt | `src/commands/codex_handler.rs` | 231-239 | Returns minimal stub for non-coordinator |
| Codex chat addendum | `src/commands/codex_handler.rs` | 247-283 | Chat-only; not injected for workers |
| `executor_uses_auto_prompt` | `src/commands/spawn/execution.rs` | 909-911 | Confirms codex gets auto-prompt |
