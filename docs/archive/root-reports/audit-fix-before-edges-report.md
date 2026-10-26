# Audit Report: `fix-before-edges` Branch

**Branch:** `fix-before-edges`
**Last commit:** 2026-03-05
**Audited against:** `main` (commit 7e5381d)
**Date:** 2026-03-12

## Summary

The branch contains **11 commits** (not 5 as originally listed in the task description — 6 earlier commits were omitted). 15 files changed, 3435 insertions(+), 21 deletions(-). A trial merge shows **18 conflict markers across ~9 files** due to heavy divergence on main since the branch point.

---

## Commit-by-Commit Analysis

### 1. `5b962c4` — docs: design coordinator as graph citizen
- **Files:** `docs/design/coordinator-as-graph-citizen.md` (new, 534 lines)
- **Status: VALUABLE** — This design doc does not exist on main. Main has `c683880` (lifecycle tasks as graph citizens) which is a *code implementation* of a related but different concept. This design doc covers coordinator evaluation, prompt evolution, and turn-based lifecycle — none of which are documented elsewhere.
- **Conflicts:** None (new file, no equivalent on main).

### 2. `c6c1a6f` — feat: show live token count next to coordinator thinking indicator
- **Files:** `coordinator_agent.rs`, `event.rs`, `render.rs`, `state.rs`
- **Status: VALUABLE** — Main has no equivalent. Adds live token usage display (→12.3k/←0.8k) next to the thinking indicator, braille spinner animation, and `tick_count` for frame-based animation. No `thinking_tokens` or `tick_count` on main.
- **Conflicts:** Heavy — `coordinator_agent.rs` has 149+ lines changed on main; `state.rs` has ~4900 lines divergence; `event.rs` ~1800 lines divergence; `render.rs` heavily modified. Would require manual conflict resolution.

### 3. `4110ca3` — feat: stream coordinator response text incrementally to TUI
- **Files:** `chat.rs`, `coordinator_agent.rs`, `render.rs`, `state.rs`
- **Status: VALUABLE** — Main has no streaming response infrastructure. Adds `write_streaming`/`read_streaming`/`clear_streaming` to `chat.rs`, `collect_response_streaming()` to coordinator agent, and TUI polling+rendering of in-progress responses. This is a significant UX improvement.
- **Conflicts:** Heavy — `coordinator_agent.rs`, `render.rs`, and `state.rs` all have major divergence. `chat.rs` has moderate divergence.

### 4. `0ec62ca` — docs: coordinator-compactor binary system architecture
- **Files:** `docs/design/coordinator-compactor-architecture.md` (new, 425 lines)
- **Status: PARTIALLY SUPERSEDED** — Main has the *implementation* of compaction (commits `58c715c` surface-compaction-as, `abeba78` refactor-compaction-trigger, `2f430f2` fix-compaction-llm, `66b5955` surface-compaction-error, `b7df415` wire-compacted-context). The research doc still has value as architectural rationale, but the design decisions it proposes are already implemented differently on main.
- **Conflicts:** None (new file).

### 5. `59e8afc` — feat: fade-out animation for tasks removed by filtering
- **Files:** `render.rs`, `state.rs`
- **Status: VALUABLE** — Main has fade-*in* animation fixes (`20bb3ba`, `d82666b`) but no fade-*out* animation. This adds `AnimationKind::FadeOut`, ghost line splicing, cleanup scheduling, and smooth disappearance when toggling system task visibility. Novel feature.
- **Conflicts:** Heavy — both `render.rs` and `state.rs` have massive divergence on main.

### 6. `a45860d` — docs: compactor agent implementation spec
- **Files:** `docs/design/compactor-agent-spec.md` (new, 456 lines)
- **Status: PARTIALLY SUPERSEDED** — Same situation as commit 4. Main has the working compaction implementation. The spec has archival/rationale value but many of its proposals are already implemented (differently) on main.
- **Conflicts:** None (new file).

### 7. `e93f733` — feat: animation toggle keybinding (A) with config persistence
- **Files:** `event.rs`, `render.rs`, `state.rs`
- **Status: VALUABLE** — Main has no animation toggle. Adds `A` key to toggle animations on/off at runtime, remembers previous animation mode for toggle-back, persists to `config.toml`, shows notification, listed in help overlay.
- **Conflicts:** Heavy — all three TUI files have major divergence.

### 8. `1bc45ab` — fix: paste cursor position off-by-one in editor
- **Files:** `event.rs`
- **Status: SUPERSEDED** — Main has commit `47dca25` which fixes the exact same bug with a *better* approach: `paste_insert_mode()` function using `InsertChar`/`LineBreak` actions instead of the `col += 1` workaround on this branch. Main's fix is more robust (handles edge cases that col arithmetic might miss).
- **Conflicts:** Would conflict with main's superior fix.

### 9. `000b4cb` — test: TUI config panel editing and persistence
- **Files:** `config_tests.rs` (new, 1195 lines), `event.rs`, `mod.rs`
- **Status: VALUABLE** — Main has no `config_tests.rs` file. This adds 41 comprehensive tests covering toggle, text editing, choice editing, section collapse, navigation, persistence, reload, endpoint management, and edge cases. Also makes `handle_key` `pub(crate)` for test access.
- **Conflicts:** `event.rs` has major divergence. `config_tests.rs` is new so no conflict, but tests may reference APIs that have changed on main.

### 10. `ffa1e54` — fix: assign default_agent to tasks when auto_assign disabled
- **Files:** `execution.rs`, `config.rs`
- **Status: VALUABLE** — Main has no `default_agent` field in `AgencyConfig` and no fallback assignment logic. When `auto_assign` is disabled, evaluations are null-routed because `task.agent` is never set. This fix adds a configurable fallback. Also adds `detail_tail_lines` to `TuiConfig` (unrelated, bundled in same commit).
- **Conflicts:** `config.rs` has ~1300 lines divergence. `execution.rs` has moderate divergence (main's `83ed096` added `before` edge logic nearby).

### 11. `1e333ee` — fix: resolve before edges into graph — normalize into after edges
- **Files:** `graph.rs`, `parser.rs`, `query.rs`
- **Status: VALUABLE (CRITICAL FIX)** — This is the namesake commit and the most important one. Main has commit `83ed096` which *creates* `before` edges on `.assign-*` tasks, but **never resolves them into `after` edges**. This means `before`-declared dependencies are invisible to readiness checks, the reverse index, and `wg show`. This commit adds:
  - `add_node()` normalization at insertion time
  - `normalize_before_edges()` for bulk-load reconciliation in `parser.rs`
  - Defense-in-depth scanning of `before` fields in `build_reverse_index()` in `query.rs`
  - 6 comprehensive tests
- **Conflicts:** `graph.rs` has moderate divergence (PendingValidation status, other additions). `parser.rs` has minor divergence. `query.rs` has ~488 lines divergence.

---

## Valuable

| Commit | Title | Priority | Reason |
|--------|-------|----------|--------|
| `1e333ee` | Resolve before edges into graph | **Critical** | Fixes a real bug — `before` edges on `.assign-*` tasks are silently broken on main |
| `ffa1e54` | Assign default_agent fallback | High | Evaluations null-routed when auto_assign disabled — data loss |
| `4110ca3` | Stream coordinator response to TUI | High | Major UX improvement — no equivalent on main |
| `c6c1a6f` | Live token count display | Medium | Useful observability during coordinator thinking |
| `000b4cb` | TUI config panel tests | Medium | 41 tests, no coverage for this on main |
| `59e8afc` | Fade-out animation | Medium | Polish feature, complements existing fade-in |
| `e93f733` | Animation toggle keybinding | Low | Nice-to-have UX convenience |
| `5b962c4` | Design: coordinator as graph citizen | Low | Architectural rationale document |

## Superseded

| Commit | Title | Superseded By |
|--------|-------|---------------|
| `1bc45ab` | Paste cursor off-by-one | `47dca25` on main — better fix using `paste_insert_mode()` |

## Partially Superseded

| Commit | Title | Status |
|--------|-------|--------|
| `0ec62ca` | Coordinator-compactor architecture doc | Implementation exists on main; doc has archival value only |
| `a45860d` | Compactor agent spec | Implementation exists on main; doc has archival value only |

## Conflicts

A full merge would produce **18 conflict markers across ~9 files**. The most conflicted files:

| File | Divergence (lines) | Severity |
|------|-------------------|----------|
| `src/tui/viz_viewer/state.rs` | ~4,900 | Extreme |
| `src/tui/viz_viewer/event.rs` | ~1,800 | Heavy |
| `src/config.rs` | ~1,300 | Heavy |
| `src/tui/viz_viewer/render.rs` | Heavy | Heavy |
| `src/commands/service/coordinator_agent.rs` | Heavy | Heavy |
| `src/query.rs` | ~488 | Moderate |
| `src/graph.rs` | Moderate | Moderate |
| `src/chat.rs` | Moderate | Moderate |
| `src/parser.rs` | Minor | Low |

---

## Recommendation

**Do NOT merge the branch as-is.** The divergence is too large and the conflict resolution effort would be high-risk.

**Cherry-pick in priority order:**

1. **`1e333ee` (before-edges normalization) — Cherry-pick immediately.** This is a real bug fix. Main creates `before` edges (`83ed096`) but never resolves them, making `.assign-*` task dependencies invisible. Cherry-pick will have moderate conflicts in `graph.rs`, `parser.rs`, and `query.rs` but the changes are self-contained and well-tested. This should be the top priority.

2. **`ffa1e54` (default_agent fallback) — Cherry-pick soon.** The `config.rs` conflict will require care due to heavy divergence, but the `execution.rs` change is small. Unbundle the `detail_tail_lines` addition from the config changes (it's unrelated).

3. **`4110ca3` + `c6c1a6f` (streaming + live tokens) — Reimplement on main.** These touch `coordinator_agent.rs`, `state.rs`, `render.rs`, and `event.rs` which have diverged so heavily that cherry-picking would be impractical. The `chat.rs` streaming infrastructure (`write_streaming`/`read_streaming`/`clear_streaming`) could be cherry-picked cleanly as a foundation, then the coordinator and TUI integration reimplemented against current main.

4. **`000b4cb` (config panel tests) — Reimplement on main.** The test file itself is new (no conflict), but it references APIs via `handle_key` and config structures that may have changed. Extract the test file, update imports/APIs to match current main.

5. **`59e8afc` + `e93f733` (fade-out + animation toggle) — Reimplement on main.** Both heavily touch `state.rs` and `render.rs`. The logic is sound but needs to be rewritten against current main's animation system.

6. **`5b962c4`, `0ec62ca`, `a45860d` (design docs) — Cherry-pick as-is.** New files with no conflicts. They have historical/archival value even where the implementation has diverged.

7. **`1bc45ab` (paste cursor fix) — Abandon.** Already superseded by a better fix on main.

**Estimated effort:** Cherry-picks 1-2 are ~2 hours of careful conflict resolution. Items 3-5 are ~1-2 days of reimplementation. Items 6-7 are trivial.
