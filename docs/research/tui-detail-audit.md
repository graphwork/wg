# TUI 1:Detail view + evaluation visibility audit (research-tui-detail)

Audit of four UX issues reported against the TUI's `1:Detail` panel and surrounding eval surfaces. **No code changes.**

All file references are relative to repo root unless noted. All TUI line numbers are HEAD as of 2026-04-27 (commit `e1441c225`).

## TL;DR — fix surface per issue

| # | Issue | Smallest fix surface |
| --- | --- | --- |
| 1 | "Evaluating" status flashes only momentarily | `STICKY_ANNOTATION_HOLD_SECS` at `src/tui/viz_viewer/state.rs:192` (raise from 3s, or rework so the badge persists for the whole `.evaluate-X` lifetime instead of just while it's `InProgress`/`PendingValidation`/`PendingEval`). |
| 2 | Evaluations missing in 1:Detail for chat-redesign batch | **Data IS missing from the graph**, not a render bug. `wg evaluate run` bails when parent status is `PendingEval` (`src/commands/evaluate.rs:198-207`); the `.evaluate-X` task fails, but `resolve_pending_eval_tasks` (`src/commands/service/coordinator.rs:883-930`) treats *any terminal* `.evaluate-X` as "eval passed" and promotes parent → Done. Net: no `eval-{parent}-*.json` is ever written. Fix is on the producer side, not the renderer. |
| 3 | Cannot hide panes in 1:Detail by clicking them | `src/tui/viz_viewer/event.rs:3953-3957`. Click handler only toggles when the click row coincides with a section *header* line (`toggle_detail_section_at_screen_row`). Clicking inside the section body is a no-op. Fix = expand the hit zone to include section body rows (map row → enclosing section header, then toggle that section). |
| 4 | Description pane specifically problematic | Composite of #3 plus: `Description` is *not* in the default-collapsed set (`src/tui/viz_viewer/state.rs:4778`); it is rendered through markdown wrapping (`draw_detail_tab` at `src/tui/viz_viewer/render.rs:2685-2768`) which inflates line count; the entire 1:Detail body shares one `hud_scroll`, so a long Description pushes everything else off-screen. Smallest fix = add `Description` to the default-collapsed set, or rely on the broader fix from #3 making it easy to dismiss. |

---

## Issue 1 — `[∴ evaluating]` flashes momentarily

### Where the badge is produced
The annotation `[∴ evaluating]` (and siblings `[⊞ assigning]`, `[∴ validating]`) is computed in **`src/commands/viz/mod.rs:207-216`** (`compute_phase_annotation`) when a system task (`.assign-X` / `.evaluate-X` / `.flip-X` / `.verify-X`) is pipeline-active. Pipeline-active is defined at **`src/commands/viz/mod.rs:196-201`** as `InProgress | PendingValidation | PendingEval`.

The annotation is attached to the **parent** task ID via `annotation_map` in `viz_output` (`src/commands/viz/mod.rs:754`), which the TUI consumes during graph reload (`src/tui/viz_viewer/state.rs:5001`).

### Why it flashes
`STICKY_ANNOTATION_HOLD_SECS = 3` (**`src/tui/viz_viewer/state.rs:192`**) is the only mechanism that keeps the badge visible after `.evaluate-X` exits its active window. Sticky logic at **`state.rs:5014-5032`** keeps a stale annotation in `annotation_map` for 3 seconds past last-seen, then drops it.

For typical evaluator tasks (haiku-class, ~30-90s of `InProgress`), the badge is visible for the duration of the evaluation. But the actual sequence is:

1. Parent transitions `InProgress → PendingEval` (set by **`coordinator.rs:1271`**, `Some(eval_task) if !eval_task.status.is_terminal() => Status::PendingEval`).
2. `.evaluate-X` task spawns shortly after, transitions Open → InProgress.
3. `.evaluate-X` completes (Done or Failed), badge disappears within 3s.

Whether the badge is visible long enough to read depends on the dispatcher tick cadence and how fast the evaluator returns. For shell-exec evals that fail fast (see issue 2), the entire window is sub-second.

### Should it persist longer?
Yes. The user reads "evaluating" as "this parent task is currently undergoing the eval phase," which logically lasts from `PendingEval` start until the parent transitions out of `PendingEval`. A simple, defensible rule: while parent task is in `PendingEval` *and* `.evaluate-X` is not yet terminal, keep the badge live; do not rely on a 3-second sticky.

### Smallest fix surface
- **Option A (one-line):** raise `STICKY_ANNOTATION_HOLD_SECS` to 30-60s (`state.rs:192`).
- **Option B (correct):** rework `compute_phase_annotation` (`viz/mod.rs:207`) so the badge is emitted whenever the **parent** is in `PendingEval` (regardless of `.evaluate-X` substate), not whenever `.evaluate-X` is active. This makes the badge match the user's mental model and removes the sticky-cache crutch.

---

## Issue 2 — Evaluations hard to see / missing in 1:Detail

### Where eval data is rendered in 1:Detail
**`src/tui/viz_viewer/state.rs:7462-7602`** (inside `load_hud_detail`):

1. Reads `.wg/agency/evaluations/`.
2. Filters files starting with `eval-{task.id}-` (line 7465).
3. Sorts by reverse filename (effectively reverse timestamp, line 7473).
4. Renders only the **first** entry (line 7474, `eval_files.first()`), as either `── Evaluation ──` or `── Evaluation (FLIP) ──` depending on `source` field.

### "Sometimes seem to be missing" — verified empirically
For the chat-redesign batch (parent task IDs):

| Parent | parent eval files (`eval-{id}-*`) |
| --- | --- |
| implement-tui-modal | 0 |
| implement-tui-tabs | 0 |
| implement-tui-tab | 0 |
| implement-tui-open | 0 |
| implement-tui-visual | 0 |
| integrate-tui-chat | 0 |
| research-tui-chat | 0 |
| fix-tui-hud | 0 |
| cleanup-sweep-stale | 0 |
| tui-tab-bar | 0 |

System-task meta-evals (`eval-.evaluate-X-*`, `eval-.flip-X-*`, `eval-.assign-X-*`) DO exist for these tasks. Older parent tasks (e.g. `add-pendingeval-state`, `agency-picks-claude`, `chat-agent-loops`) DO have parent eval files. So the regression is recent and matches the introduction of `PendingEval` state.

**Conclusion: data is missing from the graph, not from the renderer.**

### Root cause (not strictly required for this research task — flagged for the implementer)
The `.evaluate-X` task is a shell exec (`exec: "wg evaluate run X"`, `src/commands/eval_scaffold.rs:303,527`). The `.evaluate-X` runs while parent is in `PendingEval`. But `wg evaluate run` rejects parents that are not `Done` or `Failed`:

```text
src/commands/evaluate.rs:198-207
match task.status {
    Status::Done | Status::Failed => {}
    ref other => bail!("Task '{}' has status {:?} — must be done or failed to evaluate", task_id, other),
}
```

Confirmed by inspecting the `.evaluate-implement-tui-modal` task log:

```text
2026-04-27T17:01:04 Spawned eval inline --model haiku
2026-04-27T17:01:04 Eval stderr: Error: Task 'implement-tui-modal' has status PendingEval — must be done or failed to evaluate
2026-04-27T17:01:04 Task marked as failed: wg evaluate exited with code 1
```

The dispatcher's `resolve_pending_eval_tasks` (`src/commands/service/coordinator.rs:883-930`) then treats this terminal-failed `.evaluate-X` as "eval passed" because it only inspects `is_terminal()`, not `Done` vs `Failed`:

```text
match eval_status {
    Some(s) if s.is_terminal() => Some(t.id.clone()),  // promotes parent to Done
```

Net effect: parent transitions Done with no eval ever recorded.

### Smallest fix surface for #2 (just the rendering side)
Render is already correct given the data. The only render-side improvement would be to **also surface that `.evaluate-X` exists and what status it's in** — i.e. when no `eval-{parent}-*.json` exists but a sibling `.evaluate-X` is present, show a row like:

```
── Evaluation ──
  .evaluate-implement-tui-modal: failed (no score recorded)
```

This is a narrow, additive change in `load_hud_detail` (state.rs:7462-7602): if `eval_files` is empty, fall back to looking up the `.evaluate-X` / `.flip-X` siblings in the in-memory graph and printing their status. Two-three new lines per missing eval, no schema change.

The full fix (writing real evals) is out of scope for the renderer and belongs in the producer (evaluate.rs / coordinator.rs).

---

## Issue 3 — Cannot hide panes inside 1:Detail by clicking them

### Pane enumeration
1:Detail is built by `load_hud_detail` (**`state.rs:7127-7865`**) as a flat sequence of `── Section ──` headers + body. The renderer (`draw_detail_tab` at **`render.rs:2502-2792`**) tracks each header's wrapped-line index in `app.detail_section_header_lines`. Sections, in render order:

| # | Section | When rendered | Default state | MD-rendered |
| --- | --- | --- | --- | --- |
| 1 | (no header) `Title / Status / Agent / Identity / Role / Tradeoff` | always | not-collapsible (no header) | no |
| 2 | `Runtime` | when executor/model/session known | expanded | no |
| 3 | `Compaction` | when compaction snapshot exists | expanded | no |
| 4 | `Description` | when task has a description | **expanded** | **yes** |
| 5 | `Prompt` (or `Prompt (iteration N)`) | when prompt.txt exists | **collapsed by default** | **yes** |
| 6 | `Output` / `Output (raw)` | when output.log/output.txt exists | **collapsed by default** | **yes** |
| 7 | `Evaluation` / `Evaluation (FLIP)` | when an `eval-{id}-*.json` exists (issue 2) | expanded | no |
| 8 | `Tokens` | when `task.token_usage` is set | expanded | no |
| 9 | `§ Agency Costs` | when any lifecycle task has token usage | expanded | no |
| 10 | `Dependencies` | when `after`/`before` non-empty | expanded | no |
| 11 | `Timing` | when any timestamp is set | expanded | no |
| 12 | `Cycle` | when `cycle_config` set | expanded | no |
| 13 | `Iterations` (or `Attempts`) | when archives or loop_iteration > 0 or retry_count > 0 | expanded | no |
| 14 | `Failure` | when `failure_reason` set | expanded | no |

Default-collapsed set is hard-coded at **`state.rs:4778`**: `["Output", "Output (raw)", "Prompt"]`.

### Header / click behavior today
- **Header is clickable, body is not.** `event.rs:3953-3957` calls `app.toggle_detail_section_at_screen_row(content_row)`, and `state.rs:8284-8294` only toggles when the screen row index appears in `detail_section_header_lines`. Click anywhere in the section body → no-op (well, focuses the right panel but does not toggle).
- **`Space`** toggles whichever section the current scroll line is inside (`state.rs:8244-8270`, `event.rs:2971-2973`).
- **`R`** toggles `Output` raw-JSON mode (`event.rs:2964-2968`).
- No "collapse all" / "expand all" keybinding.

### Smallest fix surface
Modify `toggle_detail_section_at_screen_row` (**`state.rs:8284-8294`**) to map any row to its **enclosing section** (the most recent `detail_section_header_lines` entry with `idx <= line_idx`), not only an exact-match header row. Two-line change. The rendering side already publishes section_header_positions in order; just walk it for the largest `idx <= line_idx`.

Also worth adding visible affordance: render the `▸/▾` indicator on the header line in a brighter style and/or render a subtle hover/click cursor when over a body row. Out of scope for the smallest fix but worth a follow-up.

---

## Issue 4 — Description pane specifically problematic

### What's wrong
Three separate things, all visible on `research-tui-detail` itself (which has a long bullet-heavy description):

1. **Not collapsed by default.** `state.rs:4778` collapses only `Output`, `Output (raw)`, `Prompt`. Description stays open even though it's typically the longest section.
2. **Markdown rendering inflates line count.** `draw_detail_tab` (`render.rs:2685-2768`) flushes accumulated description lines through `markdown_to_lines` + `wrap_line_spans`. Bulleted, code-fenced, and paragraph-wrapped content can multiply line count by ~1.5-3×.
3. **No independent scroll.** The whole 1:Detail body shares one `hud_scroll` (`state.rs:7143`, `render.rs:2780-2792`). A 60-line Description pushes everything below it (Runtime, Eval, Tokens, Costs, Deps, Timing, …) past the bottom of a typical viewport unless the user scrolls.

The user's complaint "blocks other panes, won't collapse" is the conjunction of (1) and (3): they have to scroll past Description to see anything else, and clicking the body to dismiss it doesn't work (issue 3).

### Smallest fix surfaces, in order of effort
- **One-line:** add `"Description"` to the default-collapsed set at `state.rs:4778`. Pro: works immediately, mirrors existing behavior for `Output`/`Prompt`. Con: a freshly-opened Detail panel for a short-description task hides the description even though it would have fit fine. Acceptable trade for chat/coordinator tasks where the description is long.
- **Slightly more:** make the default-collapsed set adaptive: collapse `Description` only when the rendered line count exceeds, say, 1/3 of `hud_detail_viewport_height`. Adds one branch in `load_hud_detail` (post-flush) or in the initial-default constructor.
- **Bigger:** give Description its own scroll viewport (independent `Paragraph` with own scroll offset). Out of scope for "smallest fix"; the issue-3 fix combined with default-collapsed handles 90% of the user's pain.

The cheapest, highest-leverage combination is: **fix #3 (click body to collapse) + add Description to default-collapsed (#4)**. Together those make Detail behave the way the user expects without further refactor.

---

## Validation checklist (against task acceptance criteria)

- [x] file:line identified for each of 4 issues.
- [x] #2 distinguishes "data missing from graph" (confirmed by directory listing — 0 parent eval files for chat-redesign batch) vs "data present but not rendered" (would-be renderer is correct given the data).
- [x] All 1:Detail sections enumerated with current header / click / default-collapse behavior.
- [x] Smallest fix surface proposed for each.
- [x] No code changes.

