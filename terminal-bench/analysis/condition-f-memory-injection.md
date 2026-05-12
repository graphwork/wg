# Condition F Memory Injection Design

**Task:** tb-f-memory-research
**Date:** 2026-04-06
**Sources synthesized:**
1. `/home/erik/executors/REPORT-claude-code-memory-systems.md` (280 lines)
2. `/home/erik/.claude/projects/-home-erik-wg/memory/MEMORY.md` (92 lines)
3. `/home/erik/workgraph/terminal-bench/wg/adapter.py` (916 lines)

---

## 1. Report Findings: Applicable Memory Patterns

Claude Code has a 10-layer memory stack. Three layers matter for the condition F gap:

| Layer | What It Provides | Available to Open Models? |
|-------|-----------------|--------------------------|
| CLAUDE.md (repo) | Role definition, critical warnings, build instructions (3.4KB) | Accessible if agent reads file |
| SKILL.md (repo) | Complete CLI reference, decomposition patterns (40KB) | Accessible but too large to inject |
| MEMORY.md (memdir) | Architecture, conventions, lifecycle, file paths (5.5KB) | **NOT accessible** — stored in `~/.claude/` |

**Key insight from the report (§4):** "Nothing in MEMORY.md is unique. Every fact is derivable from reading CLAUDE.md + SKILL.md + the source code + git log. It's a convenience summary." But open models don't have the session history to derive it — they need the summary injected.

**The report's prescription (§5):** Including MEMORY.md content in prompt assembly closes 90% of the gap. The remaining 10% (session transcripts, file cache, speculation) is optimization.

**Pattern to apply:** Static distilled memory injected into the system prompt. This mirrors what Claude Code does automatically via `loadMemoryPrompt()` — MEMORY.md is always injected into the system prompt. We do the same, but with a curated subset sized to fit open-model context budgets.

---

## 2. Distilled Memory Document (~1.2K tokens)

```
## wg Project Memory (Distilled)

### Architecture
- **Graph storage**: `.wg/graph.jsonl` — one JSON object per line, append-only, human-readable
- **Task lifecycle**: open → in-progress → done | failed | abandoned | blocked | waiting
  - Tasks with `--verify` gates pass through `pending-validation` before `done`
- **Dependencies**: Directed graph (supports cycles). Use `--after <task-id>` to declare edges
- **Service model**: `wg service start` spawns agents on ready tasks (max_agents configurable)
- **Agent isolation**: Each concurrent agent gets its own git worktree

### Key Conventions
- **Task IDs**: kebab-case, auto-generated from title (e.g., "Build parser" → `build-parser`)
- **TDD pattern**: Write failing test first, implement until it passes, verify no regressions
- **Dependency edges are mandatory**: Every step that depends on a previous step MUST use `--after`. Flat task lists without edges are an anti-pattern
- **Verification gates**: `--verify "command"` attaches a machine-checkable pass/fail gate. The command must exit 0 for the task to complete
- **Same files = sequential edges**: NEVER parallelize tasks that modify the same files

### Project Structure
```
.wg/
├── graph.jsonl          # Task graph (source of truth)
├── config.toml          # Coordinator/agent/model config
├── agency/              # Roles, tradeoffs, agents, evaluations
├── service/             # Daemon state, logs, coordinator state
├── agents/              # Per-agent working directories + stream.jsonl
└── functions/           # Reusable workflow templates
```

### Build & Test
- Language: Rust (Cargo)
- Build: `cargo build`
- Test: `cargo test`
- Install globally after changes: `cargo install --path .`

### Essential Commands
| Command | Purpose |
|---------|---------|
| `wg add "title" --after dep --verify "cmd"` | Create task with deps + validation |
| `wg done <id>` | Mark task complete |
| `wg fail <id> --reason "why"` | Mark task failed |
| `wg log <id> "msg"` | Journal progress |
| `wg artifact <id> path` | Record output file |
| `wg show <id>` | Full task details |
| `wg list` | All tasks |
| `wg ready` | Tasks ready to work on |

### Common Pitfalls
1. **Forgetting `--after`**: Creates race conditions. Every dependent step needs an edge
2. **Not running `cargo install --path .`**: After code changes, the global `wg` binary is stale
3. **Using built-in task tools**: Claude's TaskCreate/TaskUpdate are a separate system — always use `wg` CLI
4. **Flat decomposition**: Tasks without dependency edges run in arbitrary order and fail
5. **Skipping verification**: Always run `cargo build && cargo test` before marking done
```

**Token count estimate:** ~450 words ≈ ~600 tokens (well under the 2K target).

---

## 3. Injection Design for `adapter.py`

### Current State

The adapter has a thin `WG_QUICK_GUIDE` constant (lines 377-386) with 4 generic lines:
```python
WG_QUICK_GUIDE = """## WG Quick Reference (Distilled)
You are working inside a task environment. Complete the task described below.
### Guidelines
- Read the task instructions carefully
- Write code and create files as requested
- Test your work before considering it done
- Focus on correctness and completeness
"""
```

This is injected into the system prompt at line 482:
```python
if not cfg.get("exclude_wg_tools"):
    system_parts.append(WG_QUICK_GUIDE)
```

### Proposed Changes

**Step 1: Replace `WG_QUICK_GUIDE` with `CONDITION_F_MEMORY`**

Replace the `WG_QUICK_GUIDE` constant (adapter.py:377-386) with a new `CONDITION_F_MEMORY` constant containing the distilled memory from §2 above. This is a direct string replacement — same location, richer content.

```python
# adapter.py line 377 — replace WG_QUICK_GUIDE entirely
CONDITION_F_MEMORY = """## wg Project Memory (Distilled)

### Architecture
- **Graph storage**: `.wg/graph.jsonl` — one JSON object per line, append-only
- **Task lifecycle**: open → in-progress → done | failed | abandoned | blocked | waiting
  - Tasks with `--verify` gates pass through `pending-validation` before `done`
- **Dependencies**: Directed graph. Use `--after <task-id>` to declare edges
- **Service model**: `wg service start` spawns agents on ready tasks
- **Agent isolation**: Each concurrent agent gets its own git worktree

### Key Conventions
- **Task IDs**: kebab-case, auto-generated from title
- **TDD pattern**: Write failing test first, implement until it passes
- **Dependency edges are mandatory**: Use `--after` for every dependent step
- **Verification gates**: `--verify "command"` — must exit 0 for task to complete
- **Same files = sequential edges**: NEVER parallelize tasks modifying the same files

### Project Structure
.wg/graph.jsonl — task graph (source of truth)
.wg/config.toml — coordinator/agent/model config
.wg/agency/ — roles, tradeoffs, agents, evaluations

### Build & Test
- Build: `cargo build`
- Test: `cargo test`
- Install after changes: `cargo install --path .`

### Essential Commands
- `wg add "title" --after dep --verify "cmd"` — create task
- `wg done <id>` / `wg fail <id> --reason "why"` — complete or fail
- `wg log <id> "msg"` — journal progress
- `wg show <id>` / `wg list` / `wg ready` — inspect state

### Common Pitfalls
1. Forgetting `--after` creates race conditions
2. Not running `cargo install --path .` after code changes
3. Flat task lists without dependency edges fail unpredictably
4. Always run `cargo build && cargo test` before marking done
"""
```

**Step 2: Wire it into the system prompt (adapter.py:479-483)**

Change the condition check to use the new constant for condition F specifically, while keeping the old behavior for other conditions:

```python
# adapter.py line 479-483 — update system prompt construction
system_parts = ["You are a skilled software engineer. Complete the task below."]
if condition == "F":
    system_parts.append(CONDITION_F_MEMORY)
elif not cfg.get("exclude_wg_tools"):
    system_parts.append(WG_QUICK_GUIDE)  # keep for B/C/D/E if WG_QUICK_GUIDE is retained
```

Alternatively, if `WG_QUICK_GUIDE` is fully replaced and all wg-tool conditions should get the richer memory, just do:

```python
if not cfg.get("exclude_wg_tools"):
    system_parts.append(CONDITION_F_MEMORY)
```

**Recommendation:** Use the condition-specific branch. The experiment design calls for condition F to have "distilled context injection" as its differentiator. Other conditions should not receive this memory — it would contaminate the comparison. Keep `WG_QUICK_GUIDE` as-is for B/C/D/E and inject `CONDITION_F_MEMORY` only for F.

### Summary of Changes

| File | Location | Change |
|------|----------|--------|
| `terminal-bench/wg/adapter.py` | Line 377-386 | Add `CONDITION_F_MEMORY` constant (keep `WG_QUICK_GUIDE` for other conditions) |
| `terminal-bench/wg/adapter.py` | Line 479-483 | Branch on `condition == "F"` to inject `CONDITION_F_MEMORY` |

**No other files need changes.** The distilled memory is a static string constant — no external file reads, no runtime dependencies.

---

## 4. Design Rationale

- **Why static injection, not file read?** The REPORT shows Claude Code injects MEMORY.md via `loadMemoryPrompt()` at session start — it's effectively a static include. A constant string is simpler, faster, and has no failure modes (no missing file, no permission errors).
- **Why ~600 tokens, not the full 5.5KB MEMORY.md?** Open models (minimax-m2.7) have smaller effective context windows than Claude. The distilled version keeps only what's actionable for a Terminal Bench task: architecture, conventions, commands, and pitfalls. Agency system details, federation, evolution, and notification backends are irrelevant to benchmark tasks.
- **Why condition-specific branching?** The experiment measures whether distilled context injection helps. Giving it to all conditions would eliminate the independent variable.
