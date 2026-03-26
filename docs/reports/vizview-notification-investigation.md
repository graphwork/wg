# VizView Notification Pop-ups — Investigation Report

## Files Involved

| File | Role |
|------|------|
| `src/tui/viz_viewer/state.rs:390-427` | `ToastSeverity` enum, `Toast` struct, `MAX_VISIBLE_TOASTS` constant |
| `src/tui/viz_viewer/state.rs:3977-4024` | `push_toast()`, `push_toast_dedup()`, `dismiss_error_toasts()`, `cleanup_toasts()` |
| `src/tui/viz_viewer/state.rs:2583-2600` | `CommandEffect` enum — `Notify(String)`, `RefreshAndNotify(String)` variants |
| `src/tui/viz_viewer/state.rs:2967-2973` | `toasts: Vec<Toast>` field on `VizApp` |
| `src/tui/viz_viewer/state.rs:7168-7197` | `CommandEffect` dispatch — converts `Notify`/`RefreshAndNotify` into `push_toast` calls |
| `src/tui/viz_viewer/state.rs:4390-4568` | Graph poll: deferred toasts for status transitions (Done, Failed, Assigned, Spawned) |
| `src/tui/viz_viewer/state.rs:8286-8357` | Agent monitoring: exit + stuck toasts |
| `src/tui/viz_viewer/render.rs:553-556` | Draw call site in main render function |
| `src/tui/viz_viewer/render.rs:5842-5857` | Status bar: echoes latest toast message inline |
| `src/tui/viz_viewer/render.rs:6573-6659` | `draw_toasts()` — the overlay rendering function |
| `src/tui/viz_viewer/event.rs:431-663` | Event handlers that produce `CommandEffect::Notify`/`RefreshAndNotify` |
| `src/tui/viz_viewer/event.rs:995-999` | Esc key: `dismiss_error_toasts()` before quit |

## Current Behavior

### What triggers toasts

| Category | Trigger | Severity | Example message |
|----------|---------|----------|-----------------|
| **Graph state changes** | Task status → Done | Info | `✅ Done: task-id (2m 34s)` |
| **Graph state changes** | Task status → Failed | Error | `❌ Failed: task-id` |
| **Graph state changes** | New task detected | Info | `New task: task-id` |
| **Graph state changes** | Agent assigned (`.assign-*` done) | Info | `⚡ Assigned: ...` |
| **Graph state changes** | Agent spawned (task → InProgress) | Info | `⚡ Spawned: agent on task` |
| **Agent monitoring** | Agent exited | Info | `🚪 Agent exited: id on task (duration)` |
| **Agent monitoring** | Agent stuck (>5min no output) | Warning (dedup) | `⏳ Agent stuck: id on task (Xm)` |
| **User actions** | Mark done, retry, unclaim, fail, edit, etc. | Info | `Marked 'task-id' done` |
| **User actions** | Message sent | Info | `Message sent to 'task-id'` |
| **User actions** | Sort changed | Info | `Sort: Chronological` |
| **User actions** | File attached / pasted | Info | `Attached: filename` |
| **User actions** | Message deleted | Info | `Message deleted` |
| **User actions** | Task form validation | Warning | `Task title is required` |
| **Service control** | Start/stop/pause/resume/restart | Info | `Service started` |
| **Service control** | Kill agent, panic kill | Info | `Killed agent-id` |
| **Coordinator ops** | Create/close/archive/stop coordinator | Info/Error | `Coordinator 1 created` |
| **Config panel** | Install config, endpoint/model validation | Info/Warning/Error | `Installed project config...` |
| **Errors** | Any `CommandEffect` failure | Error | `Error: <first non-empty output line>` |

### How long they stay

| Severity | Auto-dismiss | Fade behavior |
|----------|-------------|---------------|
| **Info** (green) | 5 seconds | Fades during last 1 second |
| **Warning** (yellow) | 10 seconds | Fades during last 1 second |
| **Error** (red) | Never — persists until Esc | No fade |

### How they are positioned

Toasts are rendered as **overlay widgets** in the **top-right corner** of `last_graph_area`:

```
x = graph_area.x + graph_area.width - toast_width - 1   (right-aligned)
y = graph_area.y + 1 + y_offset                          (stacking downward from top)
```

- Maximum 4 visible (`MAX_VISIBLE_TOASTS`).
- Newest toasts appear at the top; older ones stack below.
- Each toast is a single-line `Paragraph` widget rendered after `Clear` (erasing underlying content).
- Maximum width: `min(60, graph_area.width - 4)`.
- Messages longer than max width are truncated with `...`.

### Dual rendering

Toasts appear in **two places simultaneously**:

1. **Overlay**: `draw_toasts()` renders colored, fading pop-ups in the top-right of the graph view.
2. **Status bar**: The **latest** toast message is echoed inline in the bottom hints/status bar (line ~5842-5857 in render.rs), without fade, using bold colored text.

## Problem Analysis

### Issue 1: Top-right placement obscures graph content

The toasts render at `graph_area.y + 1`, which is the **first row of actual graph content**. For graphs with many tasks, the top-right corner is where the first few task nodes typically appear. The `Clear` widget erases whatever graph content was underneath.

### Issue 2: No visual separation from graph

Toasts are single-line spans with a subtle colored background (`rgb(15,40,15)` for Info). On a dark terminal, these can blend with or look like part of the graph content — there's no border, no box, no distinctive visual frame.

### Issue 3: Redundant dual display

The same toast message appears both as an overlay AND in the status bar. The status bar echo has no fade and persists until the toast is cleaned up, creating visual redundancy.

### Issue 4: High volume during active graphs

During active service operation, toasts fire for every status transition (assigned, spawned, done, failed, exited). With multiple agents, the 4-toast limit means rapid cycling and visual noise. The user can't process them faster than they arrive.

## Improvement Proposals

### Proposal A: Bottom-right positioning with visual frame

**Change**: Move toasts from top-right to **bottom-right** of the graph area, stacking **upward** from the bottom. Add a thin box-drawing border around each toast for visual distinction.

```
y = graph_area.y + graph_area.height - 2 - y_offset   (bottom, above hints bar)
```

**Tradeoffs**:
- (+) Bottom-right is the standard location for desktop notification toasts (macOS, Windows, GNOME).
- (+) Bottom of the graph is typically less information-dense (fewer task nodes drawn there when the graph is scrolled to the top).
- (+) Border makes toasts visually distinct from graph content.
- (-) If the graph is scrolled to the bottom, toasts could still overlap. But this is less common than top overlap.
- (-) Slight overlap risk with the status/hints bar — need a 1-row margin.

**Implementation**:
- Modify `draw_toasts()` in `render.rs:6624-6626` — reverse the y calculation.
- Add `Block::bordered()` or manual box-drawing chars around each toast (increases height to 3 lines per toast, so reduce `MAX_VISIBLE_TOASTS` to 2-3).
- Consider: remove the status bar echo (render.rs:5842-5857) since the overlay is now in a less-disruptive location.

**Files to change**: `src/tui/viz_viewer/render.rs` (draw_toasts, status bar section).

### Proposal B: Status-bar-only toasts (no overlay)

**Change**: Remove the overlay toasts entirely. Instead, show toast messages **only** in the status bar, with a severity-colored icon prefix and a brief fade/timeout that clears the message.

**Design**:
- Status bar gets a dedicated right-aligned "notification zone" (e.g., last 40 chars).
- Messages rotate through with the same auto-dismiss timings.
- Error messages persist with a red `[!]` prefix until Esc.
- Multiple pending messages: show count badge `(+3)` and rotate on a 2-second timer, or show only the most recent.

**Tradeoffs**:
- (+) Zero graph content obscured — toasts never overlay the graph.
- (+) Simpler rendering — no overlay positioning, no `Clear`, no fade math.
- (+) Consistent with many TUI apps (vim, helix, kakoune) where notifications live in the status line.
- (-) Less visual prominence — users may miss important notifications.
- (-) Status bar real estate is limited; long messages may be truncated.
- (-) Loses the ability to show multiple toasts simultaneously (though with the status bar rotation, the count badge mitigates this).

**Files to change**: `src/tui/viz_viewer/render.rs` (remove `draw_toasts()`, enhance status bar section), `src/tui/viz_viewer/state.rs` (simplify toast management — can remove `MAX_VISIBLE_TOASTS` and stacking logic).

### Comparison

| Criterion | Proposal A (bottom-right box) | Proposal B (status-bar only) |
|-----------|------------------------------|------------------------------|
| Graph obscuration | Low (bottom-right, rare overlap) | None |
| Discoverability | High (visible overlay) | Medium (must look at status bar) |
| Implementation effort | Low (repositioning + border) | Medium (rework status bar, remove overlay) |
| Multi-toast handling | Good (stacked, max 3-4) | Limited (1 visible + count badge) |
| Visual distinctness | High (bordered box, colored) | Medium (colored text in bar) |
| Consistency with TUI norms | Desktop-like (modern) | Editor-like (vim/helix) |

### Recommendation

**Proposal A** (bottom-right with visual frame) is the lower-risk, higher-impact change. It preserves the existing toast infrastructure, solves the primary complaint (overlapping graph content at the top), and adds visual distinction with borders. It can be implemented by modifying ~20 lines in `draw_toasts()`.

If the user wants a more radical simplification, Proposal B eliminates overlay complexity entirely but requires more rework and trades off notification visibility.

A hybrid approach is also viable: use Proposal A positioning for Error/Warning toasts (they need prominence) and Proposal B (status-bar only) for Info toasts (they're informational and high-volume).
