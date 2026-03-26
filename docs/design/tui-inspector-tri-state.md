# TUI Inspector Tri-State Layout — Design Spec

## Current State Analysis

### Existing LayoutMode Enum (`state.rs:623`)

The TUI already has a five-state `LayoutMode` enum:

```rust
pub enum LayoutMode {
    ThirdInspector,      // 33% inspector
    HalfInspector,       // 50% inspector
    TwoThirdsInspector,  // 67% inspector (default)
    FullInspector,       // 100% inspector — graph hidden
    Off,                 // 0% inspector — graph only
}
```

**Cycle order** (key `=` / `Shift+Tab`): Third → Half → TwoThirds → Full → Off → Third.

### Existing Split Ratio Management

- **Field:** `VizApp.right_panel_percent: u16` (0–100) — `state.rs:2870`
- **Field:** `VizApp.layout_mode: LayoutMode` — `state.rs:2874`
- **Field:** `VizApp.right_panel_visible: bool` — `state.rs:2864`
- **Keyboard:** `i` grows inspector by +5%, `I` shrinks by -5% — `event.rs:1076–1080`
- **Mapping:** `layout_mode_for_percent()` maps percentage → nearest LayoutMode bracket — `state.rs:7111`

### Existing Divider Drag (`event.rs:2471–2486`)

On `MouseEventKind::Drag(MouseButton::Left)` with `ScrollbarDragTarget::Divider`:
1. Compute `total_width` from `last_graph_area.width + last_right_panel_area.width`
2. `panel_width = right_edge - mouse_column`
3. `pct = panel_width * 100 / total_width`
4. **Clamp: `pct.max(15).min(85)`** — neither panel goes below 15%
5. Update `right_panel_percent` and derive `layout_mode`

### Divider Hit Area (`render.rs:1661–1670`)

A 3-column-wide `Rect` centered on the left border of the inspector panel. Stored in `VizApp.last_divider_area`. Only set when both panels are visible (not in `FullInspector` mode). Hover state tracked in `VizApp.divider_hover`.

### The "Outer Bar" — Coordinator Tab Bar

The coordinator tab bar is rendered **inside** the Chat tab content area (`render.rs:2107–2244`). It is part of `draw_chat_tab()` and lives at `area.y` of the chat content area. It is **not** a top-level layout element — it only appears when the Chat tab is active in the inspector panel.

The top-level bars are:
- **Status bar** (row 0) — `render.rs:95` — always visible
- **Vitals bar** (row N-1) — `render.rs:97`
- **Action hints** (row N) — `render.rs:98`

So the "outer bar" the user likely refers to is the **status bar + vitals bar + hints bar** — the chrome that exists in all modes. In `FullInspector` mode, these 3 rows still consume space.

### FullInspector Behavior Today

- `draw_right_panel()` gets the entire `main_area` — `render.rs:397`
- Inspector is rendered edge-to-edge with **no borders** — `render.rs:1677–1678`
- Status/vitals/hints bars still occupy 3 rows
- `last_divider_area` is set to `Rect::default()` (no divider hit area)
- Focus is forced to `FocusedPanel::RightPanel` — `state.rs:7004–7005`

---

## Proposed Design: Tri-State Inspector

### State Definitions

The existing `LayoutMode` already covers the three requested states. No new enum variant is needed:

| Requested State | Existing LayoutMode | `right_panel_percent` | Behavior |
|---|---|---|---|
| **Normal** (draggable) | `ThirdInspector`, `HalfInspector`, `TwoThirdsInspector` | 15–85 (clamped) | Both panels visible, divider draggable |
| **Full-screen inspector** | `FullInspector` | 100 | Inspector takes entire main_area, no graph, edge-to-edge (no borders) |
| **Minimized** (collapsed strip) | `Off` | 0 | Graph takes entire main_area, inspector collapsed to 1-column strip |

### What Changes

#### Change 1: Minimized strip in `Off` mode

**Current behavior:** `Off` mode hides the inspector entirely — no visual remnant.

**New behavior:** When `layout_mode == Off`, render a **1-column-wide vertical strip** on the right edge of `main_area`. This strip:
- Has a subtle visual indicator (e.g., `▐` characters in `DarkGray`, or a thin colored line)
- On hover: brightens (e.g., `White` or `Yellow`)
- On click: transitions to the last "normal" LayoutMode (stored in a new field `last_split_mode`)

**Implementation:**
- In `render.rs`, the `Off` branch (lines 400–414, 227–234) should allocate 1 column on the right for the strip
- Store the strip area in a new field `VizApp.last_minimized_strip_area: Rect`
- In `event.rs`, check clicks against `last_minimized_strip_area` → restore to `last_split_mode`

#### Change 2: Left-edge restore strip in `FullInspector` mode

**Current behavior:** `FullInspector` hides the graph entirely. No way to drag back with mouse — only keyboard (`=`, `i`).

**New behavior:** Render a **1-column-wide restore strip** on the left edge of `main_area`:
- Visual: thin `▌` line in `DarkGray`
- On hover: brightens to `Yellow` (same as divider hover)
- On click: start a divider drag (set `scrollbar_drag = Some(Divider)`) — the user can drag rightward to restore a split
- On simple click (no drag): restore to `last_split_mode`

**Implementation:**
- In `render.rs`, the `FullInspector` branch should allocate 1 column on the left for the strip
- Store the strip area in `VizApp.last_fullscreen_restore_area: Rect`
- In `event.rs`, check clicks against `last_fullscreen_restore_area`:
  - On `MouseDown` → set `scrollbar_drag = Some(Divider)` and transition to a normal split mode
  - On `MouseUp` without significant drag distance → restore to `last_split_mode`

#### Change 3: Drag-to-fullscreen / drag-to-minimize

**Current behavior:** Divider drag is clamped to 15–85%.

**New behavior:** Extend drag range to include state transitions:
- **Drag left past 85%:** Transition to `FullInspector` (inspector consumes 100%)
- **Drag right past 15%:** Transition to `Off` (inspector minimized to strip)

**Implementation:**
- In `event.rs:2471–2486`, modify the clamp logic:
  ```
  if raw_pct > 90 → set layout_mode = FullInspector
  if raw_pct < 10 → set layout_mode = Off, save current mode to last_split_mode
  else → clamp(15, 85) as before
  ```
- When entering FullInspector via drag, save the previous `layout_mode` and `right_panel_percent` in `last_split_mode`/`last_split_percent`

#### Change 4: New state fields

Add to `VizApp` struct (in `state.rs`):

```rust
/// The last "normal" split mode (ThirdInspector/HalfInspector/TwoThirdsInspector)
/// and percentage. Used to restore from FullInspector or Off modes.
pub last_split_mode: LayoutMode,
pub last_split_percent: u16,

/// Hit area for the minimized inspector strip (1-col, right edge, Off mode).
pub last_minimized_strip_area: Rect,

/// Hit area for the full-screen restore strip (1-col, left edge, FullInspector mode).
pub last_fullscreen_restore_area: Rect,

/// Whether the mouse is hovering over the minimized strip.
pub minimized_strip_hover: bool,

/// Whether the mouse is hovering over the full-screen restore strip.
pub fullscreen_restore_hover: bool,
```

Default `last_split_mode` to `TwoThirdsInspector`, `last_split_percent` to `67`.

### State Transition Diagram

```
                ┌──────────────────────────────┐
                │       FullInspector           │
                │  (inspector = 100% main_area) │
                │  Left edge: restore strip     │
                └──────┬───────────────────────┘
                       │
          click strip  │  drag past 90%
          or drag from │  from Normal
          strip        │
                       ▼
                ┌──────────────────────────────┐
                │         Normal                │
                │  (ThirdInsp/HalfInsp/TwoThi)  │
                │  Draggable divider 15%–85%    │
                └──────┬───────────────────────┘
                       │
          drag past    │  click strip
          10% from     │  from Off
          Normal       │
                       ▼
                ┌──────────────────────────────┐
                │            Off                │
                │  (inspector = 0%, strip only) │
                │  Right edge: minimized strip  │
                └───────────────────────────────┘
```

Keyboard shortcuts remain unchanged:
- `=` / `Shift+Tab`: cycle through all modes (Third → Half → TwoThirds → Full → Off)
- `i` / `I`: grow/shrink by 5% with wrapping to Off at extremes

### Mouse Interaction Model

| State | Area | Mouse Action | Result |
|---|---|---|---|
| **Normal** | Divider (3-col hit area) | Click + drag | Resize split (15–85%) |
| **Normal** | Divider | Drag left past 90% | → FullInspector |
| **Normal** | Divider | Drag right past 10% | → Off (minimized) |
| **FullInspector** | Left restore strip (1-col) | Click (no drag) | → last Normal mode |
| **FullInspector** | Left restore strip | Click + drag right | Start divider drag from left edge, enter Normal |
| **Off** | Right minimized strip (1-col) | Click | → last Normal mode |
| **Off** | Right minimized strip | Click + drag left | Start divider drag from right edge, enter Normal |

### Edge Cases

1. **Keyboard focus in FullInspector**: Already handled — `apply_layout_mode()` sets `focused_panel = RightPanel` (`state.rs:7005`). No change needed.

2. **Keyboard focus when restoring from FullInspector**: Set focus to `RightPanel` (inspector was in use). The user can Tab to switch.

3. **Keyboard focus in Off (minimized)**: Already handled — `apply_layout_mode()` sets `focused_panel = Graph` (`state.rs:7009`).

4. **Compact/Narrow responsive breakpoints**: The strips should **not** appear in Compact mode (< 50 cols) — there isn't room. In Narrow mode (50–80 cols), the strips could be shown but the 1-col overhead is negligible.

5. **`right_panel_visible` flag**: Currently `Off` sets `right_panel_visible = false`. This should remain, but the strip is drawn regardless. The strip is a "handle" for restoring the panel, not panel content.

6. **Drag from restore strip enters Normal**: When the user starts dragging from the FullInspector's left strip, immediately transition to a normal split mode (e.g., `TwoThirdsInspector` at 67%) and set `scrollbar_drag = Some(Divider)` so subsequent `Drag` events adjust the ratio naturally.

7. **Status/vitals/hints bars in FullInspector**: These 3 rows remain visible in FullInspector. The user request mentions "the outer bar disappears entirely." If this means the status/vitals/hints bars too, we could add a "zen mode" variant. However, the simpler interpretation is that the graph panel's border/divider disappears — which already happens. **Recommendation:** Keep the 3 chrome bars for now; add a follow-up task for zen mode if needed.

8. **Tab/divider styling**: The minimized strip should use the same color vocabulary — `DarkGray` at rest, `Yellow` on hover (matching divider hover color).

9. **save/restore `last_split_mode` on keyboard transitions**: `cycle_layout_mode()` and `grow_viz_pane()`/`shrink_viz_pane()` should save the current normal mode before transitioning to Full or Off, so that click-to-restore returns to the user's preferred split.

---

## Files to Modify

| File | Changes |
|---|---|
| `src/tui/viz_viewer/state.rs` | Add `last_split_mode`, `last_split_percent`, `last_minimized_strip_area`, `last_fullscreen_restore_area`, `minimized_strip_hover`, `fullscreen_restore_hover` fields to `VizApp`. Update `apply_layout_mode()`, `grow_viz_pane()`, `shrink_viz_pane()` to save/restore split state. |
| `src/tui/viz_viewer/render.rs` | In `Off` branches: allocate 1-col right strip, draw it, store area. In `FullInspector` branches: allocate 1-col left strip, draw it, store area. Both strips: hover highlight. |
| `src/tui/viz_viewer/event.rs` | Modify divider drag clamp logic (lines 2471–2486) to trigger Full/Off at extremes. Add click handlers for `last_minimized_strip_area` and `last_fullscreen_restore_area`. Track hover state for strips on `Moved` events. |

No new files required. No changes to `mod.rs`, `file_browser.rs`, `screen_dump.rs`, `trace.rs`, or `markdown.rs`.

---

## Implementation Notes

- The `last_split_mode` / `last_split_percent` save-restore pattern is similar to how `hud_size` already works (`state.rs:6976–6978`).
- The strips should be `1` column wide. In very narrow terminals (< 50 cols, Compact mode), skip drawing them to avoid wasting space.
- The divider's existing 3-column hit area pattern can be reused: the strip itself is 1 col, but the hit area can be 2 cols (strip + 1 adjacent) for easier grabbing.
- All existing tests for divider drag, layout cycling, and grow/shrink should continue to pass. New tests should cover: drag-to-fullscreen threshold, drag-to-minimize threshold, click-strip-to-restore, hover state on strips.
