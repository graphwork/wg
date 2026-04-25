# Design: Copy-on-Write Worktrees for Agent Isolation

## 1. Survey of Current Worktree Usage

### Creation

Worktrees are created in `src/commands/spawn/worktree.rs:29-89` via `create_worktree()`:

```
git worktree add .wg-worktrees/<agent-id> -b wg/<agent-id>/<task-id> HEAD
```

Then `.workgraph/` is symlinked into the worktree so `wg` CLI works normally.

**Gating:** `src/commands/spawn/execution.rs:851-870` (`should_create_worktree()`) decides whether a task gets a worktree:
- **Always skip:** meta tasks (`.assign-*`, `.flip-*`, `.evaluate-*`, `.place-*`, `.compact-*`), `bare` exec mode, `light` exec mode.
- **Always create:** `full` and `shell` exec modes.
- **Conservative default:** unknown exec modes get a worktree.

**Environment setup** (`execution.rs:589-596`): worktree agents get `WG_WORKTREE_PATH`, `WG_BRANCH`, `WG_PROJECT_ROOT`, `WG_WORKTREE_ACTIVE=1`, and `CARGO_TARGET_DIR` pointing into the worktree to isolate cargo builds.

### Merge-back

Since `stigmergic-merge-on-done`, merge-back lives in `src/commands/done.rs:119-245` (`attempt_worktree_merge()`):
1. Detect worktree via env vars.
2. Count commits on branch vs HEAD.
3. If commits exist: acquire flock on `.wg-worktrees/.merge-lock`, `git merge --squash`, commit, release lock.
4. If conflict: abort merge, create `.merge-<task-id>` deferred task (`done.rs:247-300`).
5. If no commits: return `NoCommits` — no merge needed.

### Sweep / Cleanup

Two-tier cleanup in `src/commands/service/worktree.rs`:
- **Atomic sweep** (`sweep_cleanup_pending_worktrees`, line 725): coordinator tick reaps worktrees with `.wg-cleanup-pending` marker, dead agent, and terminal task.
- **Orphan cleanup** (`cleanup_orphaned_worktrees`, line 594): service startup scans `.wg-worktrees/` for dirs without live agents.
- **GC fallback** (`src/commands/worktree_gc.rs`): user-invoked `wg gc --worktrees`.

### Removal

`remove_worktree()` (line 134): remove `.workgraph` symlink → remove `target/` → `git worktree remove --force` → `git branch -D`.

## 2. Task Categories That Don't Need a Writable Worktree

### Already excluded (no worktree created)

| Category | ID pattern | Volume |
|----------|-----------|--------|
| Assignment | `.assign-*` | ~75 tasks done |
| FLIP evaluation | `.flip-*` | ~50 tasks done |
| LLM evaluation | `.evaluate-*` | ~30 tasks done |
| Placement | `.place-*` | ~20 tasks done |
| Compaction | `.compact-*` | ~10 tasks done |
| **Subtotal** | dot-prefix | **230/319 done tasks (72%)** |

These already skip worktree creation via the `task_id.starts_with('.')` check.

### Currently get worktrees but often don't write tracked files

| Category | Examples | Typical behavior |
|----------|---------|-----------------|
| Research tasks | `research-cow-worktrees`, `research-shell-verify`, `research-when-should` | Produce `docs/*.md` only — a tracked file, but no source code conflict risk |
| Autopoietic/cycle tasks | `autopoietic-reflect`, `autopoietic-pulse-v2`, `ci-fix-loop` | Inspect graph state, create subtasks; many iterations make zero code changes |
| Audit tasks | `audit-unmerged-branches` | Read git state, produce reports |
| Smoke tests | `smoke-*-echo-hello` | Run a command and report output |

**Evidence from current worktrees** (7 active, 2026-04-25):
- `agent-309` (autopoietic-reflect): 0 commits, 44MB — **pure read**
- `agent-312` (autopoietic-reflect): 0 commits, 32GB (cargo target built) — **pure read, wasted 32GB**
- `agent-318` (autopoietic-reflect): 0 commits, 44MB — **pure read**
- `agent-336` (default-wg-add): 0 commits, 44MB — **pure read** (or abandoned before writing)
- `agent-404` (research-cow-worktrees): 0 commits, 44MB — **read + docs only**
- `agent-286` (fix-paste-forwarding): 1 commit, 33GB — **write** (cargo target built)
- `agent-329` (cascade-abandonment-to): 1 commit, 35GB — **write** (cargo target built)

**Result: 5/7 (71%) of current worktrees have zero commits.** Two of those five built cargo target/ (33GB wasted). Only 2/7 actually wrote tracked files.

### Estimated read-only ratio across all tasks

- 230 dot-prefix meta tasks: already no worktree (72%)
- 89 user tasks completed: at least 42% are research/audit/smoke/autopoietic (likely read-only)
- **Conservative estimate: 80-85% of all tasks never need a writable worktree**

## 3. Mechanism Comparison

### Measured baseline costs

| Metric | Value |
|--------|-------|
| Worktree creation time | **0.18s** (git worktree add) |
| Source checkout size | **44MB** (1418 tracked files) |
| Cargo target build | **~33GB** (full debug build) |
| Total `.wg-worktrees/` usage | **100GB** across 7 worktrees |
| Worktrees with zero commits | **5/7 (71%)** |

The creation time (0.18s) and source checkout size (44MB) are negligible. The dominant cost is `CARGO_TARGET_DIR` isolation — each agent that runs `cargo build` pays ~33GB. The secondary cost is coordination complexity (flock merging, sweep loops, orphan cleanup).

### Comparison table

| Mechanism | Disk cost | Creation time | Tracked-file trigger | Crash safety | Tooling compat | Concurrency | Prior art | Complexity |
|-----------|----------|---------------|---------------------|-------------|---------------|-------------|-----------|-----------|
| **A. Lazy worktree via fs watcher** | Zero until write, then 44MB+target | 0s start, 0.18s on materialize | inotify on `git ls-files` paths — race window between detection and write completion | Partial write visible before worktree exists; agent must be paused during materialization | Requires running in main checkout first (git status sees other agents' changes) | Poor: multiple agents in main checkout see each other's untracked files | None found at this scale | High: inotify setup, tracked-file set maintenance, atomic swap |
| **B. Lazy worktree via shim** | Zero until write, then 44MB+target | 0s start, 0.18s on materialize | Intercept Edit/Write/sed tools; check `git ls-files` on target path | Good: write doesn't happen until worktree exists | Breaks for arbitrary commands (agent runs `sed -i`, `cargo fmt`, etc.) | Same as A — agents share main checkout initially | Bazel's action sandboxing intercepts file I/O, but at process level | Medium-High: shim layer, but can't cover all write paths |
| **C. Declarative isolation** | 0 or 44MB+target, decided at dispatch | 0s or 0.18s | None (upfront annotation: `--isolation read-only\|full`) | Same as current (full worktree when needed) | Full compatibility when worktree is created | Good: read-only agents share main checkout safely | Docker/K8s pod isolation levels; Nix build sandboxing | **Low**: annotation on task, dispatch-time decision |
| **D. Always-worktree, skip merge-back** | Same as today (44MB base, +target) | Same as today (0.18s) | N/A (doesn't reduce creation cost) | Same as current | Full compatibility | Same as current | `stigmergic-merge-on-done` already does this partially | **Minimal**: extend existing `NoCommits` path |
| **E. Overlayfs with tracked-only upper** | Zero until tracked write; upper layer is sparse | Near-zero (mount) | Kernel intercepts writes; upper layer captures only changed files | Upper layer is a standard directory — fully recoverable | **Problematic**: git sees overlayfs mount, not real files; cargo/rustc may behave differently; requires root/fuse-overlayfs | Excellent: each agent gets its own overlay, reads share lower layer | Docker overlay2 (production-proven); Incus/LXD (Navaris uses this); NixOS sandbox | High: requires root or fuse-overlayfs, mount management, umount on cleanup |

### Prior art references

- **Overlayfs (mechanism E):** Linux kernel since 3.18; Docker overlay2 storage driver is the production standard for container filesystem isolation. [kernel.org/doc/Documentation/filesystems/overlayfs.txt](https://docs.kernel.org/filesystems/overlayfs.html)
- **Bazel sandboxing (mechanism B):** Bazel uses `linux-sandbox` with mount namespaces and bind mounts to restrict builds to declared inputs/outputs. Intercepts at process boundary, not file-path level. [bazel.build/docs/sandboxing](https://bazel.build/docs/sandboxing)
- **jujutsu/jj (mechanisms A/C):** jj tracks working-copy changes without separate branches; its "operation log" provides undo semantics. Working copies are lightweight because jj doesn't use git's index for change tracking. [github.com/martinvonz/jj](https://github.com/martinvonz/jj)
- **Nix builds (mechanism E):** Nix uses per-build sandbox directories with bind mounts and chroot, similar to overlayfs isolation. Content-addressed store deduplicates outputs. [nixos.org/manual/nix/stable/command-ref/conf-file.html#conf-sandbox](https://nixos.org/manual/nix/stable/command-ref/conf-file.html#conf-sandbox)
- **gitoxide (mechanism D):** gitoxide's worktree handling is more efficient than git's (parallel checkout, memory-mapped index), but doesn't change the fundamental cost model. [github.com/Byron/gitoxide](https://github.com/Byron/gitoxide)

## 4. Recommendation: Declarative Isolation (Mechanism C) + Shared Cargo Target

### Rationale

The data reveals that **the problem is not worktree creation time** (0.18s is negligible) but **cargo target directory duplication** (33GB per agent that builds). The tracked-file-write trigger from the task description is a conceptually clean goal, but the runtime detection mechanisms (A, B) introduce race conditions, tooling incompatibilities, and high implementation complexity for marginal benefit over a simpler declarative approach.

**Mechanism C (declarative isolation)** is recommended because:

1. **Lowest complexity, highest reliability.** A `--isolation` annotation on task creation is a single enum field. No fs watchers, no shims, no kernel mounts. The `should_create_worktree()` function already implements a proto-version of this — it just needs to be exposed as a user-facing annotation rather than hardcoded.

2. **The data supports static classification.** 80-85% of tasks are classifiable as read-only at dispatch time. The exec_mode system (`bare`, `light`, `full`, `shell`) already approximates this. Adding an explicit `isolation` field that overrides exec_mode gives full control without runtime detection.

3. **Eliminates the dominant cost.** Agents marked `read-only` skip worktree creation entirely and run in the main checkout. This eliminates 44MB checkout + potential 33GB cargo target for ~70% of user tasks.

4. **Graceful degradation.** If classification is wrong (a "read-only" agent tries to write tracked files), the write goes to the main checkout — which is detected by git status on the next coordinator tick. The coordinator can then escalate: create a worktree, replay, or fail the task. This is no worse than today's world where agents occasionally stomp each other.

### Combined with: shared cargo target cache

The second part of the recommendation addresses the other dominant cost (cargo target duplication):

- **Shared read-only target cache:** Use `CARGO_TARGET_DIR` pointing to a shared location with `sccache` or cargo's built-in artifact caching. Agents that need to build get a private `target/` that benefits from incremental compilation against the shared cache.
- **Cost reduction:** From 33GB per building agent to ~2-5GB incremental (only recompiled crates).

### Cost/benefit summary

| Metric | Current | With declarative isolation + shared target |
|--------|---------|-------------------------------------------|
| Worktrees created per 10 tasks | ~3 user tasks | ~1 (only `full` isolation) |
| Disk per read-only agent | 44MB (worktree) | 0 (runs in main checkout) |
| Disk per write agent | 44MB + 33GB target | 44MB + ~3GB incremental target |
| Total .wg-worktrees for 7 agents | 100GB | ~10-15GB |
| Merge-back complexity | Same for all | Only for `full` isolation agents |
| Implementation effort | N/A | ~2-3 days (add isolation field, wire into spawn, shared target setup) |

## 5. Migration Plan

### Phase 1: Isolation annotation (non-breaking)

1. Add `isolation: Option<String>` field to `Task` struct (`src/graph.rs`). Values: `read-only`, `full`, `auto` (default).
2. Add `--isolation` flag to `wg add` CLI.
3. Wire `isolation` into `should_create_worktree()`:
   - `read-only` → never create worktree.
   - `full` → always create worktree (current behavior for `full`/`shell` exec modes).
   - `auto` (default) → use existing exec_mode heuristic.
4. No change to existing tasks — `auto` preserves current behavior.

### Phase 2: Heuristic improvement

1. Update coordinator dispatch to infer isolation level from task metadata:
   - Tasks with `--verify` containing `cargo test` → likely needs `full`.
   - Research/audit/observational tags → `read-only`.
   - Cycle iterations with no prior commits → `read-only` (after first iteration).
2. Agency roles inform isolation: Evaluator, Auditor → `read-only`; Programmer → `full`.

### Phase 3: Shared cargo target

1. Configure `CARGO_TARGET_DIR` to a shared location outside `.wg-worktrees/`.
2. Set up sccache or equivalent for cross-agent compilation caching.
3. Each worktree agent gets a symlink or overlay on the shared target.

### Phase 4 (optional): Runtime materialization

If the declarative approach proves insufficient (too many false negatives — agents classified as read-only that actually write), implement lazy materialization:
1. Start "read-only" agents in main checkout.
2. Monitor for tracked-file writes via a pre-exec hook on Edit/Write tools.
3. On first tracked-file write: pause agent, create worktree, `rsync` any untracked changes, resume.

This phase is explicitly deferred — the data suggests it won't be needed for 95%+ of cases.

### Backward compatibility

- All phases are additive — no existing task behavior changes.
- `auto` isolation preserves the current `should_create_worktree()` logic.
- In-flight tasks are unaffected (they already have or don't have worktrees).

## 6. Follow-up Implementation Tasks

These can be filed directly as workgraph tasks:

1. **`add-isolation-field`** — Add `isolation: Option<String>` to `Task` struct in `graph.rs`, add `--isolation` flag to `wg add` CLI in `cli.rs`. Wire into `should_create_worktree()` in `execution.rs`.
   - Scope: `src/graph.rs`, `src/cli.rs`, `src/commands/spawn/execution.rs`
   - Verify: `cargo test test_worktree_gate`

2. **`read-only-agent-in-main-checkout`** — When `isolation=read-only`, skip worktree creation and run agent in main checkout. Ensure `.workgraph/` access works (it's already in main checkout). Handle `WG_WORKTREE_ACTIVE` / `WG_WORKTREE_PATH` env vars correctly (unset for read-only agents).
   - Scope: `src/commands/spawn/execution.rs`
   - Verify: spawn a `bare` task, confirm no worktree created, task completes

3. **`isolation-inference-heuristic`** — Coordinator infers isolation level from task metadata (tags, exec_mode, verify command, role). Default to `full` for safety.
   - Scope: `src/commands/service/coordinator.rs`, `src/commands/spawn/execution.rs`
   - Verify: research task dispatched without worktree; code task gets worktree

4. **`shared-cargo-target`** — Configure a shared `CARGO_TARGET_DIR` at `.wg-shared-target/` for cross-agent compilation caching. Each agent gets incremental builds against the shared base.
   - Scope: `src/commands/spawn/execution.rs`, `src/config.rs`
   - Verify: two agents build concurrently, second reuses first's compilation artifacts

5. **`skip-merge-back-no-diff`** — When `wg done` detects a worktree with zero diff to main (no tracked-file changes), skip the merge-back entirely. Complements `stigmergic-merge-on-done`.
   - Scope: `src/commands/done.rs`
   - Verify: agent in worktree with no commits → `wg done` produces no merge commit

6. **`read-only-write-guard`** — For `isolation=read-only` agents running in main checkout, add a post-task check: if `git status` shows tracked-file modifications, log a warning and create a recovery branch. Prevents silent data loss from misclassified read-only tasks.
   - Scope: `src/commands/done.rs`, `src/commands/service/coordinator.rs`
   - Verify: read-only agent that modifies tracked file → warning logged, recovery branch created

7. **`worktree-disk-usage-metrics`** — Add `wg metrics worktrees` command showing per-worktree disk usage, commit count, and age. Helps operators identify waste.
   - Scope: `src/commands/metrics.rs`
   - Verify: `wg metrics worktrees` shows table with disk usage per agent
