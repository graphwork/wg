# Audit Report: `fix-auto-task-edges` Branch

**Branch:** `fix-auto-task-edges`
**Last commit:** 2026-03-06
**Audited against:** `main` (commit 58822e3)
**Date:** 2026-03-12

## Summary

The branch contains **4 commits** across 8 files: 688 insertions(+), 123 deletions(-). A trial merge (`git merge-tree`) shows **conflicts in 5 files**: `coordinator.rs`, `mod.rs`, `main.rs`, `event.rs`, and `state.rs`. The branch diverged before significant refactoring on main (eval_scaffold extraction, `.verify-flip-*` → `.verify-*` rename, restart command addition).

---

## Commit-by-Commit Analysis

### 1. `02391e1` — fix: wire bidirectional edges for auto-created system tasks
- **Files:** `src/commands/service/coordinator.rs` (+126, −7)
- **What it does:**
  - Fixes `build_auto_assign_tasks` to add `.assign-X` to target's `after` list
  - Fixes `build_auto_evaluate_tasks` to add `.evaluate-X` to source's `before` list
  - Fixes `build_flip_verification_tasks` to set `after: [X]` on verify task AND add to source's `before`
  - Adds `repair_auto_task_edges()` — a backfill pass running each coordinator tick (Phase 2.9) that scans all `.assign-*`, `.evaluate-*`, `.verify-flip-*` tasks and ensures both sides of every edge exist
- **Status: PARTIALLY SUPERSEDED / PARTIALLY VALUABLE**
  - **Assign edges — SUPERSEDED.** Main refactored assign task creation into `eval_scaffold::scaffold_assign_task()` which already sets `before: vec![task_id]` on the assign task AND adds `source.after.push(assign_task_id)`. Bidirectional wiring is correct on main for new assign tasks.
  - **Evaluate edges — VALUABLE.** Main's `eval_scaffold::scaffold_eval_task()` sets `after: eval_after` on the eval task but does NOT add the eval task to the source task's `before` list. The branch's fix for this is still needed.
  - **Verify/FLIP edges — VALUABLE (needs update).** Main renamed `.verify-flip-*` to `.verify-*` (commit `f24a9e2`), but `build_flip_verification_tasks` on main still does not add the verify task to the source's `before` list. The branch's fix is valuable but needs updating for the rename.
  - **`repair_auto_task_edges()` backfill — VALUABLE (needs update).** This function handles pre-existing tasks created before the fix. Still needed because main's eval/verify tasks lack bidirectional edges. However, it references `.verify-flip-*` which must be changed to `.verify-*` to match main.
  - **Phase 2.9 coordinator integration — VALUABLE.** The tick-based repair pass is a good pattern for data migration.
- **Conflicts:** HEAVY — `build_auto_evaluate_tasks` was refactored to use `scaffold_eval_tasks_batch`; `build_auto_assign_tasks` now delegates to `scaffold_assign_task`; `build_flip_verification_tasks` uses `.verify-*` naming. All three functions have significantly different structure on main.

### 2. `a58698b` — feat: show readable markdown chat transcript in Detail Output section
- **Files:** `src/tui/viz_viewer/event.rs` (−7), `src/tui/viz_viewer/state.rs` (+207, −109)
- **What it does:**
  - Adds `extract_chat_transcript()` to parse Claude CLI stream-json output into `ChatBlock` enum (Text, ToolUse, Result), deduplicating partial messages by message ID
  - Replaces `flatten_json_to_lines`/`humanize_key` with `render_chat_blocks`: text shown as markdown, tool calls as condensed one-liners (`⚙ ToolName(params)`)
  - Splits output into two always-visible sections: "Output" (readable, expanded) and "Output (raw)" (pretty JSON, collapsed)
  - Removes `detail_raw_json` field and `R` toggle keybinding
- **Status: PARTIALLY SUPERSEDED**
  - Main already has `extract_assistant_text_from_log()` (line 8146 of `state.rs`) which does the same core job: parses stream-json, extracts text blocks, and renders tool-use as compact summaries.
  - Main's tool rendering is **more specific**: Bash→command, Read/Write/Edit→file_path, Grep/Glob→pattern. The branch uses generic `summarize_tool_input` which is less informative.
  - Main retains the `R` toggle between raw JSON and human-readable modes. The branch replaces this with two always-visible sections.
  - **Novel in branch:** Message deduplication by ID (keeps last/longest per assistant message), and the two-section UX. The deduplication could be valuable for very verbose outputs.
  - **Main is better at:** Tool-specific detail rendering and already removed `flatten_json_to_lines`/`humanize_key` (they don't exist on main).
- **Conflicts:** HEAVY — `state.rs` has ~4,500+ lines on main vs the branch's merge base. The output rendering section is significantly different.

### 3. `0b13a2b` — feat: log caller identity on service stop & add restart command
- **Files:** `src/cli.rs` (+14), `src/commands/service/ipc.rs` (+19, −4), `src/commands/service/mod.rs` (+63, −1), `src/main.rs` (+3)
- **What it does:**
  - Adds `triggered_by_agent`/`triggered_by_task` fields to `IpcRequest::Shutdown`, populated from `WG_AGENT_ID`/`WG_TASK_ID` env vars
  - Logs caller identity in daemon log on shutdown
  - Adds `wg service restart` with `--force` and `--kill-agents` flags
- **Status: PARTIALLY SUPERSEDED**
  - **Restart command — SUPERSEDED.** Main already has `wg service restart` (commit `1b4bf` + subsequent `e1c93dc` guard-service-stop). Main's version is simpler (no `--force`/`--kill-agents` flags), uses `run_stop_inner` to bypass agent guard, and is more integrated with the agent safety infrastructure.
  - **Caller identity logging — VALUABLE.** The `triggered_by_agent`/`triggered_by_task` fields on `IpcRequest::Shutdown` do NOT exist on main. This is useful observability — when an agent triggers a service stop, the daemon log shows which agent/task did it. Clean, backward-compatible change (uses `#[serde(default)]`).
  - The branch's `Restart { force, kill_agents }` CLI definition conflicts with main's `Restart` (no fields).
- **Conflicts:** MODERATE — `ipc.rs` auto-merges cleanly for the caller identity fields. `mod.rs`, `main.rs`, and `cli.rs` conflict on the restart command definition.

### 4. `abcfece` — feat: add Related Tasks section to TUI detail view
- **Files:** `src/tui/viz_viewer/event.rs` (+10, −3), `src/tui/viz_viewer/render.rs` (+18), `src/tui/viz_viewer/state.rs` (+230)
- **What it does:**
  - Adds "── Related Tasks ──" section to task detail view showing:
    - System tasks (`.assign-*`, `.evaluate-*`, `.verify-flip-*`, `.respond-to-*`) with Unicode icons (⊳, ∴, ✓, ↩) and status-colored indicators (green=done, yellow=in-progress, red=failed)
    - Evaluation scores displayed inline when available
    - Upstream (←) and downstream (→) task dependencies with status
    - Other dot-tasks referencing the current task
  - Enter key navigates to related tasks from the detail view
  - `parse_related_task_id()` helper extracts task IDs from formatted lines
  - `read_eval_score()` helper reads latest evaluation score from disk
  - Status-colored rendering in `render.rs` via `in_related_section` tracking
- **Status: FULLY VALUABLE** — Main has NO equivalent. This is a clean, self-contained feature that significantly improves task inspection UX. Shows the full "lifecycle constellation" of a task in one view.
- **Conflicts:** MODERATE-HEAVY — `event.rs` conflicts on the Enter key handler (main has different match structure). `state.rs` conflicts due to file-level divergence. `render.rs` likely auto-merges (additive change in a new section).
- **Note:** References `.verify-flip-*` which was renamed to `.verify-*` on main. The `known_prefixes` array in `build_related_tasks_lines` needs updating.

---

## Valuable

| Commit | Feature | Priority | Reason |
|--------|---------|----------|--------|
| `abcfece` | Related Tasks section in TUI detail | **High** | No equivalent on main; significant UX improvement for task inspection |
| `02391e1` (partial) | Eval/verify bidirectional edge wiring | **High** | Main's `scaffold_eval_task` and `build_flip_verification_tasks` don't wire source `before` edges; tasks float disconnected in viz |
| `02391e1` (partial) | `repair_auto_task_edges()` backfill | **Medium** | Fixes pre-existing broken edges; idempotent repair pass is a good migration pattern |
| `0b13a2b` (partial) | Caller identity on IPC Shutdown | **Medium** | Observability improvement — see which agent triggered service stop |

## Superseded

| Commit | Feature | Superseded By |
|--------|---------|---------------|
| `02391e1` (partial) | Assign task bidirectional edges | `eval_scaffold::scaffold_assign_task()` on main already wires both sides |
| `0b13a2b` (partial) | `wg service restart` command | Main's `run_restart()` (simpler, integrated with agent guard via `run_stop_inner`) |
| `a58698b` (mostly) | Chat transcript parsing | Main's `extract_assistant_text_from_log()` with better tool-specific rendering |

## Conflicts

A full merge produces conflicts in **5 of 8 files**:

| File | Conflict Source | Severity |
|------|----------------|----------|
| `src/commands/service/coordinator.rs` | `build_auto_*` functions refactored to use `eval_scaffold` | Heavy |
| `src/tui/viz_viewer/state.rs` | ~4,500+ line divergence; output rendering and detail sections restructured | Heavy |
| `src/tui/viz_viewer/event.rs` | Enter key handler restructured; R toggle context changed | Moderate |
| `src/commands/service/mod.rs` | Restart command implementation differs | Moderate |
| `src/main.rs` | Restart CLI variant fields differ | Low |

Clean merges: `src/cli.rs` (Restart variant additive), `src/commands/service/ipc.rs` (new fields with `serde(default)`), `src/tui/viz_viewer/render.rs` (additive section coloring).

---

## Recommendation

**Do NOT merge the branch as-is.** Conflicts in 5 files and several superseded features make a whole-branch merge high-risk.

**Cherry-pick / reimplement in priority order:**

1. **Related Tasks section (`abcfece`) — Reimplement on main.** This is the highest-value commit. The logic is clean and self-contained (~230 lines in `state.rs`, ~18 in `render.rs`, ~10 in `event.rs`). Reimplement against current main's `state.rs` structure. Update `.verify-flip-*` references to `.verify-*`. Estimated effort: 1-2 hours.

2. **Eval/verify bidirectional edge fix (`02391e1` partial) — Reimplement on main.** Extract the edge-wiring logic and apply to:
   - `eval_scaffold::scaffold_eval_task()` — add source `before` edge after `graph.add_node(eval_task)`
   - `build_flip_verification_tasks()` in `coordinator.rs` — add source `before` edge after creating `.verify-*` task
   - Port `repair_auto_task_edges()` as a new function, updating `.verify-flip-*` → `.verify-*`
   - Add Phase 2.9 tick integration
   - Estimated effort: 1-2 hours. Consider combining with the `fix-before-edges` branch's `normalize_before_edges()` approach for a comprehensive fix.

3. **Caller identity on shutdown (`0b13a2b` partial) — Cherry-pick `ipc.rs` changes only.** The `triggered_by_agent`/`triggered_by_task` fields on `IpcRequest::Shutdown` auto-merge cleanly. Also needs the env-var capture in `run_stop` (`mod.rs`) — minor conflict resolution needed. Estimated effort: 30 minutes.

4. **Chat transcript improvements (`a58698b`) — Mostly abandon.** Main's `extract_assistant_text_from_log` is equivalent and has better tool-specific rendering. The message-deduplication-by-ID logic could be extracted if duplicate output is observed in practice, but this is low priority. The two-section UX (always-visible Output + collapsed raw) could be revisited as a separate UX decision.
