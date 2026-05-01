# Worktree-Based Isolation for Concurrent Agents

## Research Report

**Task:** research-worktree-based
**Date:** 2026-02-28
**Author:** Architecture-Learning agent (Thorough)

---

## 1. Problem Statement

When multiple agents work on the same codebase concurrently, they operate on a single shared working directory. This causes:

1. **Build breaks** — Agent A's half-finished removal of types causes Agent B's build to fail
2. **Commit conflicts** — Agents that complete work can't commit because another agent modified the same files
3. **Stale state** — Changes that land successfully get overwritten by concurrent agents working from stale state
4. **Cascade failures** — Build breaks propagate through the entire pipeline

The root cause: the coordinator spawns multiple agents that all operate on the same working directory simultaneously.

## 2. Git Worktree Capabilities

### 2.1 How Worktrees Work

Git worktrees allow multiple working trees to share the same `.git` repository. Each worktree:

- Has its own **independent working tree** (files on disk)
- Has its own **HEAD**, **index**, and **per-worktree refs**
- Shares the **object store**, **refs**, **config**, and **hooks** with the main repo
- Is linked via a `.git` file (not directory) pointing to `.git/worktrees/<name>/`

```
# Main repo
/home/erik/workgraph/
├── .git/                    # Full git directory
│   └── worktrees/
│       ├── agent-1/         # Admin files for worktree 1
│       └── agent-2/         # Admin files for worktree 2
└── ...

# Linked worktree
/home/erik/workgraph/.wg-worktrees/agent-1/
├── .git                     # File: "gitdir: /home/erik/workgraph/.git/worktrees/agent-1"
├── src/                     # Independent copy of all files
└── ...
```

### 2.2 Key Capabilities

| Feature | Detail |
|---------|--------|
| Create | `git worktree add <path> -b <branch>` — creates new worktree with new branch |
| List | `git worktree list` — shows all worktrees |
| Remove | `git worktree remove <path>` — cleans up worktree and admin files |
| Lock | `git worktree lock` — prevents pruning |
| Prune | `git worktree prune` — removes stale admin entries |

### 2.3 Constraints

1. **No two worktrees can have the same branch checked out.** Each worktree must be on a unique branch (or detached HEAD). This is enforced by git.
2. **Creating a worktree is fast** — it's essentially `cp -r` of the working tree + index setup. No object copying.
3. **Disk usage** — each worktree is a full copy of the working tree. For a Rust project like workgraph (~25MB source), this is negligible. For large repos, it can matter.
4. **Build artifacts are NOT shared.** Each worktree gets its own `target/` directory (assuming it's gitignored). This means each agent pays the full build cost. For Rust, this is significant (~2-5 min for initial build).

### 2.4 Concurrency Safety

Git operations are generally safe across worktrees because:
- The object store uses atomic writes (pack files, loose objects)
- Refs use lockfiles for atomic updates
- Each worktree has its own index, so `git add`/`git commit` don't interfere

However, **concurrent pushes to the same remote branch** can still conflict — but since each worktree is on its own branch, this is avoided by design.

## 3. Worktree-Per-Agent Model Design

### 3.1 Architecture Overview

```
Coordinator spawns Agent X for task-id "implement-foo"
  │
  ├─ 1. Create worktree:
  │     git worktree add .wg-worktrees/agent-{id} -b wg/{agent-id}/{task-id}
  │
  ├─ 2. Symlink .workgraph:
  │     ln -s /absolute/path/to/.wg .wg-worktrees/agent-{id}/.workgraph
  │
  ├─ 3. Set working_dir to worktree path
  │     Agent runs inside .wg-worktrees/agent-{id}/
  │
  ├─ 4. Agent does work (edits files, runs builds, commits)
  │
  ├─ 5. On completion: merge changes back
  │     git checkout main && git merge --no-ff wg/{agent-id}/{task-id}
  │     (or rebase, or squash — configurable)
  │
  └─ 6. Cleanup:
        git worktree remove .wg-worktrees/agent-{id}
        git branch -d wg/{agent-id}/{task-id}
```

### 3.2 Branch Naming Convention

```
wg/{agent-id}/{task-id}
```

Examples:
- `wg/agent-42/implement-foo`
- `wg/agent-43/fix-build-warnings`

Rationale:
- `wg/` prefix makes workgraph branches instantly identifiable
- Agent ID ensures uniqueness even if the same task is retried
- Task ID provides human-readable context

### 3.3 Worktree Location

Two options:

**Option A: Inside repo (recommended)**
```
.wg-worktrees/
├── agent-42/     # Full working tree for agent-42
└── agent-43/     # Full working tree for agent-43
```
Add `.wg-worktrees/` to `.gitignore`. Benefits:
- Easy to find and inspect
- No path confusion
- Cleanup on `git worktree prune` is natural

**Option B: In /tmp**
```
/tmp/wg-{project-hash}/agent-42/
```
Benefits:
- No clutter in project directory
- tmpfs = faster I/O
- Auto-cleanup on reboot

**Recommendation:** Option A for discoverability and debuggability. Agents often need to be inspected during development.

### 3.4 Worktree Lifecycle

```
┌─────────────────────────────────────────────────────────┐
│                    SPAWN PHASE                          │
│                                                         │
│  1. git worktree add .wg-worktrees/{agent-id}          │
│     -b wg/{agent-id}/{task-id} HEAD                    │
│  2. ln -s {abs}/.wg                                    │
│     .wg-worktrees/{agent-id}/.workgraph                │
│  3. Set working_dir = .wg-worktrees/{agent-id}         │
│  4. Launch agent process in that directory              │
└───────────────────────┬─────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────┐
│                   AGENT RUNS                            │
│                                                         │
│  Agent works in isolated worktree:                      │
│  - Edits files (no interference with other agents)      │
│  - Runs cargo build (own target/ directory)             │
│  - Can commit to its branch                             │
│  - wg commands use symlinked .workgraph (shared state)  │
└───────────────────────┬─────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────┐
│                 COMPLETION PHASE                        │
│                                                         │
│  On success (wg done / wrapper detects exit 0):         │
│  1. Commit all changes on agent's branch                │
│  2. Merge branch into main (strategy: configurable)     │
│  3. Remove worktree                                     │
│  4. Delete branch                                       │
│                                                         │
│  On failure (wg fail / wrapper detects nonzero exit):   │
│  1. Commit partial changes (optional, for recovery)     │
│  2. Remove worktree                                     │
│  3. Delete branch                                       │
│  (Or: keep branch for debugging if configured)          │
└─────────────────────────────────────────────────────────┘
```

### 3.5 Merge Strategy

Three strategies, configurable via `wg config`:

| Strategy | Command | Pros | Cons |
|----------|---------|------|------|
| **Merge commit** | `git merge --no-ff wg/...` | Preserves history, easy to revert | Noisy history |
| **Squash** (recommended) | `git merge --squash wg/...` | Clean history, one commit per task | Loses intermediate commits |
| **Rebase** | `git rebase main wg/...` | Linear history | Can fail with conflicts |

**Recommendation:** Squash merge by default. Each task produces a single commit on main with a message like:
```
[wg/agent-42] implement-foo: Add user authentication endpoint

Squashed from wg/agent-42/implement-foo
```

### 3.6 Conflict Resolution

When merging back to main, conflicts can occur if two agents modified the same file. Strategies:

1. **First merger wins** — The first agent to merge back succeeds. Subsequent agents that conflict must be retried. The retry agent sees the merged state and can resolve.

2. **Auto-retry on conflict** — If merge fails, automatically set the task back to Open (like triage "restart") so the coordinator re-dispatches it. The new agent works from the updated main.

3. **Manual resolution** — Flag the task as needing human attention.

**Recommendation:** Strategy 2 (auto-retry) for implementation tasks. The retry count already exists in the task model (`retry_count`, `max_retries`), so this integrates naturally.

```rust
// Pseudo-code for merge-back
fn merge_agent_branch(task_id: &str, agent_id: &str) -> Result<MergeResult> {
    let branch = format!("wg/{}/{}", agent_id, task_id);

    // Attempt squash merge
    let result = Command::new("git")
        .args(["merge", "--squash", &branch])
        .output()?;

    if !result.status.success() {
        // Conflict detected
        Command::new("git").args(["merge", "--abort"]).output()?;
        return Ok(MergeResult::Conflict);
    }

    // Commit the squashed changes
    Command::new("git")
        .args(["commit", "-m", &format!("[{}] {}", agent_id, task_id)])
        .output()?;

    Ok(MergeResult::Success)
}
```

## 4. Multi-Agent System Precedents

### 4.1 Claude Code's EnterWorktree

Claude Code has built-in worktree support via the `EnterWorktree` tool:
- Creates worktrees in `.claude/worktrees/<name>`
- Uses a random name or user-specified name
- Creates a new branch `worktree-<name>` from HEAD
- On session exit, prompts user to keep or remove the worktree
- The Agent tool also supports `isolation: "worktree"` for subagents

Key observation: Claude Code's worktree model is **manual and user-initiated**, not automated. Our model needs to be fully automated and coordinator-driven.

### 4.2 Existing .claude Worktree in This Repo

This repo already has a Claude Code worktree at `.claude/worktrees/agent-a4f43401`:
- Full working tree copy
- `.git` file pointing to `.git/worktrees/agent-a4f43401`
- **No `.wg` directory** — this is the shared state problem in action

## 5. Integration Points in Workgraph

### 5.1 Where Worktree Creation Happens

**File: `src/commands/spawn/execution.rs`** — `spawn_agent_inner()`

This is where the agent process is launched. The worktree creation must happen **before** the process spawn, after the task is claimed but before `cmd.spawn()`.

Integration point (around line 198):
```rust
// CURRENT: just set working_dir from executor config
if let Some(ref wd) = settings.working_dir {
    cmd.current_dir(wd);
}

// PROPOSED: create worktree if isolation is enabled
let effective_working_dir = if config.coordinator.worktree_isolation {
    let worktree_path = create_agent_worktree(dir, &temp_agent_id, task_id)?;
    Some(worktree_path)
} else {
    settings.working_dir.clone().map(PathBuf::from)
};

if let Some(ref wd) = effective_working_dir {
    cmd.current_dir(wd);
}
```

### 5.2 Where Merge-Back Happens

**File: `src/commands/service/triage.rs`** — `cleanup_dead_agents()`

This is where the coordinator detects that an agent's process has exited. This is the integration point for merge-back:

```rust
// After determining the task is done/completed:
if worktree_exists_for_agent(agent_id) {
    match merge_agent_worktree(dir, agent_id, task_id) {
        Ok(MergeResult::Success) => { /* normal flow */ }
        Ok(MergeResult::Conflict) => {
            // Set task back to Open for retry
            task.status = Status::Open;
            task.assigned = None;
            task.retry_count += 1;
        }
        Err(e) => { /* log error, keep task as-is */ }
    }
    cleanup_agent_worktree(agent_id)?;
}
```

Additionally, **`src/commands/done.rs`** — `run()` could trigger merge-back when the agent itself calls `wg done`, though the wrapper script approach (triage handles it) may be cleaner.

### 5.3 Wrapper Script Changes

**File: `src/commands/spawn/execution.rs`** — `write_wrapper_script()`

The wrapper script runs after the agent exits. It currently checks task status and calls `wg done`/`wg fail`. It needs a new step:

```bash
# After agent exits and status is determined:
if [ -n "$WG_WORKTREE_PATH" ]; then
    # Commit any uncommitted changes on the agent branch
    cd "$WG_WORKTREE_PATH"
    git add -A
    git commit -m "[wg/$AGENT_ID] $TASK_ID: agent work" --allow-empty 2>/dev/null

    # Merge back to main
    cd "$PROJECT_ROOT"
    git merge --squash "$WG_BRANCH" 2>> "$OUTPUT_FILE"
    MERGE_EXIT=$?

    if [ $MERGE_EXIT -ne 0 ]; then
        git merge --abort
        echo "[wrapper] Merge conflict, marking task for retry" >> "$OUTPUT_FILE"
        wg fail "$TASK_ID" --reason "Merge conflict on integration"
    else
        git commit -m "[wg/$AGENT_ID] $TASK_ID" 2>> "$OUTPUT_FILE"
    fi

    # Cleanup worktree
    git worktree remove "$WG_WORKTREE_PATH" 2>/dev/null
    git branch -D "$WG_BRANCH" 2>/dev/null
fi
```

### 5.4 Output Capture Changes

**File: `src/agency/output.rs`** — `capture_git_diff()`

Currently captures `git diff` from the shared working tree. With worktrees, the diff is on the agent's branch:

```rust
// Instead of diffing the working tree, diff the branch
let diff = Command::new("git")
    .args(["diff", &format!("main...{}", agent_branch)])
    .current_dir(&project_root)
    .output()?;
```

Or, since we squash-merge, the diff is simply the squash commit itself — even simpler.

### 5.5 New Environment Variables

The wrapper script and agent need these additional env vars:

| Variable | Value | Purpose |
|----------|-------|---------|
| `WG_WORKTREE_PATH` | `/path/to/.wg-worktrees/agent-42` | Agent's working directory |
| `WG_BRANCH` | `wg/agent-42/implement-foo` | Agent's branch name |
| `WG_PROJECT_ROOT` | `/home/erik/workgraph` | Main repo root (for merge-back) |

## 6. The `.workgraph` Shared State Problem

### 6.1 The Problem

`.wg/` contains:
- `graph.jsonl` — task state (shared between all agents)
- `agents/` — agent registry and output
- `agency/` — roles, motivations, agent configs
- `executors/` — executor TOML configs
- `config.toml` — project configuration
- `output/` — captured task outputs

All agents must read/write the same task state. If each worktree has its own `.wg/`, changes are invisible to other agents and the coordinator.

### 6.2 Solution: Symlink `.workgraph`

**Recommended approach:** Symlink `.workgraph` in each worktree to the main repo's `.wg/`:

```bash
# During worktree creation:
ln -s /home/erik/workgraph/.wg \
      /home/erik/workgraph/.wg-worktrees/agent-42/.workgraph
```

This means:
- All `wg` commands in any worktree read/write the same `graph.jsonl`
- Agent registry is shared (coordinator can track all agents)
- Config and executor settings are shared
- Output capture goes to the shared location

Since `.wg/` is already in `.gitignore`, it won't appear in any worktree's git status.

### 6.3 Alternative: `WG_DIR` Environment Variable

Instead of symlinks, set `WG_DIR` pointing to the absolute path:

```bash
export WG_DIR=/home/erik/workgraph/.wg
```

This requires the `wg` CLI to check `WG_DIR` before searching for `.workgraph` in the current directory. Minor code change in `src/main.rs` where `workgraph_dir` is resolved.

**Assessment:** The symlink approach is simpler and requires no code changes to `wg` itself. The `WG_DIR` approach is more explicit but requires a code change. **Recommend symlink for v1, add `WG_DIR` support as a follow-up.**

### 6.4 Concurrency Safety of `graph.jsonl`

Currently, `graph.jsonl` access is not protected by file locks (except in agent registry which uses `load_locked`). With worktree isolation, multiple agents still write to the same `graph.jsonl` via `wg log`, `wg done`, etc.

This is an **existing problem** that worktrees don't make worse — it exists today with or without worktrees. The current system relies on:
- Most graph mutations happening through the coordinator (single process)
- Agent mutations (`wg log`, `wg done`) being append-only or atomic field updates
- The wrapper script running sequentially after the agent exits

A proper file-locking solution for `graph.jsonl` is orthogonal and should be a separate task.

## 7. Cost/Benefit Analysis

### 7.1 Benefits

| Benefit | Impact |
|---------|--------|
| **No more build interference** | High — eliminates the primary pain point |
| **No file-level conflicts during work** | High — each agent has its own copy |
| **Clean git history** | Medium — squash merges produce readable history |
| **Natural conflict detection** | Medium — git merge surfaces true conflicts at integration time |
| **Retry semantics improve** | Medium — retry starts from clean main HEAD, not messy shared state |

### 7.2 Costs

| Cost | Impact | Mitigation |
|------|--------|------------|
| **Disk space** | Low — ~25MB per worktree for this project | Cleanup on completion |
| **Build time per agent** | High — each worktree needs its own `target/` | Shared cargo cache, or `CARGO_TARGET_DIR` pointing to shared location |
| **Merge conflicts on integration** | Medium — same-file edits cause retries | Graph pattern: "same files = sequential edges" already documented |
| **Complexity** | Medium — new worktree lifecycle to manage | Well-isolated in spawn/triage code |
| **Worktree creation time** | Low — git worktree add is ~instant | Negligible vs. agent runtime |

### 7.3 The Build Cache Problem

The most significant cost is build time. Each worktree gets its own `target/` directory, so:
- Agent 1 builds from scratch: ~3-5 min (full Rust build)
- Agent 2 builds from scratch: ~3-5 min
- vs. today: shared `target/`, incremental builds ~10-30s per agent

**Mitigation options:**

1. **Shared `CARGO_TARGET_DIR`** — All worktrees share one target directory. Cargo handles concurrent builds via file locks on `target/`. However, this can cause lock contention and doesn't help with incremental builds being invalidated by different worktrees.

2. **Copy `target/` on worktree creation** — `cp -r` or `rsync` the target directory when creating the worktree. This gives each agent a warm build cache. Cost: ~1-2GB copy, ~5s.

3. **Hardlink `target/` dependencies** — Use `cp -al` to hardlink the target directory. Zero disk cost, instant. Dependencies are immutable, so hardlinks are safe. Only recompiled crates need new space.

4. **Accept the cost** — For research/docs tasks, there's no build. For code tasks, the build is part of the agent's work anyway.

**Recommendation:** Option 2 (copy target) for v1. It's simple and effective. Option 3 is an optimization for later.

### 7.4 When to Use Worktrees vs. Shared Directory

| Scenario | Recommendation |
|----------|---------------|
| Multiple agents modifying different files | Worktree (prevents accidental interference) |
| Multiple agents modifying the same files | Sequential (worktree won't help — merge conflicts) |
| Single agent | Shared directory (no isolation needed) |
| Research/docs tasks (no code changes) | Shared directory (no file conflicts possible) |
| `exec_mode: bare` tasks | Shared directory (no file I/O) |

**Recommendation:** Make worktree isolation opt-in via config, defaulting to **off**. The coordinator enables it when `max_agents > 1` and the tasks being spawned are code-modifying tasks.

```toml
# .wg/config.toml
[coordinator]
worktree_isolation = true    # false = shared working dir (current behavior)
merge_strategy = "squash"    # "merge", "squash", "rebase"
```

Tasks can also opt out individually:

```yaml
# In task definition
isolation: false  # Don't create a worktree for this task
```

## 8. Recommended Architecture

### 8.1 Configuration

```toml
# .wg/config.toml
[coordinator]
worktree_isolation = false   # opt-in, default off
merge_strategy = "squash"    # how to integrate agent branches
worktree_dir = ".wg-worktrees"  # where to create worktrees
copy_target = true           # copy target/ for warm build cache
cleanup_on_fail = true       # remove worktree on task failure
```

### 8.2 Implementation Plan

#### Phase 1: Core Worktree Lifecycle (estimated: 2-3 tasks)

1. **`src/commands/spawn/worktree.rs`** (new module)
   - `create_agent_worktree(dir, agent_id, task_id) -> Result<PathBuf>`
   - `cleanup_agent_worktree(dir, agent_id) -> Result<()>`
   - `merge_agent_worktree(dir, agent_id, task_id, strategy) -> Result<MergeResult>`
   - Handles: git worktree add, .workgraph symlink, optional target/ copy

2. **Modify `src/commands/spawn/execution.rs`**
   - In `spawn_agent_inner()`: call `create_agent_worktree()` when isolation enabled
   - Set `cmd.current_dir()` to worktree path
   - Add `WG_WORKTREE_PATH`, `WG_BRANCH`, `WG_PROJECT_ROOT` to env

3. **Modify `write_wrapper_script()`**
   - Add merge-back logic after agent exits
   - Handle merge conflicts (auto-retry or fail)

#### Phase 2: Integration with Triage & Cleanup (1-2 tasks)

4. **Modify `src/commands/service/triage.rs`**
   - In `cleanup_dead_agents()`: attempt merge-back for dead agents
   - Clean up orphaned worktrees

5. **Add `wg worktree` commands**
   - `wg worktree list` — show active agent worktrees
   - `wg worktree clean` — remove orphaned worktrees
   - `wg worktree status` — show merge status

#### Phase 3: Configuration & Polish (1 task)

6. **Config changes**
   - Add `worktree_isolation`, `merge_strategy`, etc. to `CoordinatorConfig`
   - Add `isolation` field to `Task` struct for per-task override

7. **Update `.gitignore`**
   - Add `.wg-worktrees/` to default `.gitignore` template

### 8.3 File Change Summary

| File | Change |
|------|--------|
| `src/commands/spawn/worktree.rs` | **New** — worktree lifecycle management |
| `src/commands/spawn/mod.rs` | Add `mod worktree` |
| `src/commands/spawn/execution.rs` | Call worktree creation, set env vars |
| `src/commands/service/triage.rs` | Add merge-back on dead agent cleanup |
| `src/config.rs` | Add worktree config fields to `CoordinatorConfig` |
| `src/graph.rs` | Add `isolation` field to `Task` (optional) |
| `src/agency/output.rs` | Adjust `capture_git_diff` for worktree branches |
| `.gitignore` | Add `.wg-worktrees/` |

## 9. Risk Assessment & Mitigation

### 9.1 Risks

| Risk | Severity | Likelihood | Mitigation |
|------|----------|------------|------------|
| Merge conflicts block pipeline | Medium | Medium | Auto-retry with conflict as reason; graph pattern "same files = sequential" |
| Orphaned worktrees consume disk | Low | Medium | `wg worktree clean` command; cleanup in triage |
| Build cache miss costs time | Medium | High | Copy target/ on creation; shared cargo cache |
| Git operations fail (corrupt repo) | High | Low | Git worktree is mature and well-tested; we only use basic operations |
| Agent creates files outside worktree | Medium | Low | `wg artifact` paths are relative; agent runs in worktree dir |
| Concurrent graph.jsonl writes | Medium | Medium | Existing problem; orthogonal fix (file locking) |
| `.workgraph` symlink issues on some filesystems | Low | Low | Fall back to `WG_DIR` env var if symlink fails |

### 9.2 Rollback Plan

Worktree isolation is opt-in (`worktree_isolation = false` by default). If problems arise:
1. Set `worktree_isolation = false` in config
2. Clean up orphaned worktrees: `wg worktree clean`
3. System reverts to current shared-directory behavior

No data migration needed — the graph format doesn't change.

## 10. Guidance: When to Use Worktrees vs. Shared Directory

### Use Worktrees When:
- Running 2+ agents simultaneously on code tasks
- Agents modify different parts of the codebase
- Build stability is critical (CI-like environments)
- Agent retries are common (worktrees give clean state)

### Use Shared Directory When:
- Running a single agent at a time
- Tasks are research-only (no file modifications)
- Tasks are `exec_mode: bare` (pure reasoning)
- Sequential pipeline with strict ordering
- Build time is the bottleneck and tasks modify few files

### Golden Rule

**Same files = sequential edges.** Worktrees solve the *accidental interference* problem (agents touching *different* files), not the *intentional coordination* problem (agents touching the *same* files). For the latter, use graph dependency edges to enforce ordering.

---

## Appendix A: Implementation Sequence

Suggested task decomposition for implementation:

```
wg add "Implement worktree lifecycle module" -d "Create src/commands/spawn/worktree.rs with create/cleanup/merge functions"
wg add "Integrate worktree into spawn" --after impl-worktree -d "Modify execution.rs to create worktrees when config.worktree_isolation is true"
wg add "Add merge-back to wrapper script" --after integrate-spawn -d "Modify write_wrapper_script to handle merge-back on agent exit"
wg add "Add worktree cleanup to triage" --after integrate-spawn -d "Modify triage.rs to merge-back dead agent worktrees"
wg add "Add worktree config fields" -d "Add worktree_isolation, merge_strategy to CoordinatorConfig"
wg add "Add wg worktree subcommand" --after impl-worktree -d "wg worktree list/clean/status commands"
wg add "Update .gitignore and docs" --after all -d "Add .wg-worktrees/ to .gitignore, update AGENT-GUIDE.md"
```

## Appendix B: Comparison with Claude Code's Approach

| Aspect | Claude Code | Workgraph (Proposed) |
|--------|-------------|---------------------|
| Trigger | User-initiated (`EnterWorktree` tool) | Automatic (coordinator config) |
| Location | `.claude/worktrees/<name>` | `.wg-worktrees/<agent-id>` |
| Branch | `worktree-<name>` | `wg/<agent-id>/<task-id>` |
| Merge-back | Manual (user decides) | Automatic (squash merge on completion) |
| Shared state | None (independent session) | `.workgraph` symlinked |
| Cleanup | User prompted on exit | Automatic on task done/fail |
| Conflict handling | None (no auto-merge) | Auto-retry on conflict |

## Appendix C: `.wg-worktrees` Lifecycle Example

```bash
# Agent-42 spawned for task "implement-auth"
$ ls .wg-worktrees/
agent-42/

$ git worktree list
/home/erik/workgraph                          08a3395 [main]
/home/erik/workgraph/.wg-worktrees/agent-42   08a3395 [wg/agent-42/implement-auth]

# Agent-42 completes work
$ git log --oneline wg/agent-42/implement-auth
a1b2c3d Add authentication middleware
08a3395 improvement-loop-3: ...

# Wrapper script squash-merges
$ git merge --squash wg/agent-42/implement-auth
$ git commit -m "[agent-42] implement-auth: Add authentication middleware"

# Cleanup
$ git worktree remove .wg-worktrees/agent-42
$ git branch -d wg/agent-42/implement-auth
```
