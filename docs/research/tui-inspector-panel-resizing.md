# TUI Inspector Panel Resizing — Interaction Design Research

## 1. Ratatui/Crossterm Mouse Event Capabilities

### What's Available (crossterm 0.28 / ratatui 0.29)

Crossterm provides three mouse tracking modes, controlled via ANSI escape sequences:

| Mode | Escape Code | Events |
|------|-------------|--------|
| Normal (1000) | `\x1b[?1000h` | Click (Down/Up) only |
| Button (1002) | `\x1b[?1002h` | Click + Drag (button-held motion) |
| Any-event (1003) | `\x1b[?1003h` | Click + Drag + Moved (all motion) |

The wg TUI already uses modes 1002/1003 selectively (`event.rs:30`, `set_mouse_capture()`). Mode 1003 is enabled on Termux where touch drag events may lack the button-held flag.

**Relevant `MouseEventKind` variants:**
- `Down(MouseButton)` — button press at (col, row)
- `Drag(MouseButton)` — motion with button held
- `Up(MouseButton)` — button release
- `Moved` — motion without button (mode 1003 only)
- `ScrollUp/Down/Left/Right` — wheel events

**Key finding: Click-and-drag on panel edges is fully supported.** The TUI already implements divider drag resizing for the horizontal (left/right) split between graph and inspector panels (`event.rs:2574-2581`, `2835-2875`). The implementation uses delta-based percent calculation to avoid integer rounding jumps.

### What's NOT Available
- No cursor shape change (can't show resize cursor like `↔` on hover in most terminals)
- No sub-character positioning (everything is cell-granular)
- Touch events are emulated as mouse events (works well on Termux per existing code)

## 2. Survey of Other TUI Apps' Resize Patterns

### lazygit
- **Model:** Fixed panels with keyboard-only resizing
- **Resize keys:** `+`/`-` to grow/shrink the focused panel
- **No mouse resize** — panels snap to predefined proportions
- **Philosophy:** Minimalist, vim-inspired

### bottom (btm)
- **Model:** Fixed widget grid, no user-resizable panels
- **Layout:** Configurable via config file, not runtime-adjustable
- **Mouse:** Used for widget selection only, not resizing

### Midnight Commander (mc)
- **Model:** Two vertical panels with a single divider
- **Mouse resize:** Click-and-drag on the vertical divider between panels
- **Keyboard resize:** No dedicated resize keys
- **Philosophy:** Mouse-first for layout, keyboard for file operations

### Zellij (terminal multiplexer)
- **Model:** Arbitrary pane tiling with borders
- **Mouse resize:** Click-and-drag any border between panes
- **Keyboard resize:** Enter "resize mode" (Ctrl+n), then arrow keys to move the nearest border
- **Visual feedback:** Border highlights on hover, mode indicator in status bar
- **Philosophy:** Modal approach — separate mode for resize to avoid keybinding bloat

### tmux
- **Model:** Panes within windows
- **Mouse resize:** `set -g mouse on`, then drag borders
- **Keyboard resize:** `resize-pane -D/U/L/R [amount]` (prefix + arrow keys in some configs)
- **Philosophy:** Command-based, not modal

## 3. Existing Inspector Layout Code

### Files & Structs

| File | Role |
|------|------|
| `src/tui/viz_viewer/state.rs:623-719` | `LayoutMode` enum (ThirdInspector, HalfInspector, TwoThirdsInspector, FullInspector, Off) |
| `src/tui/viz_viewer/state.rs:728-748` | `ResponsiveBreakpoint` enum (Compact <50, Narrow 50-80, Full >80) |
| `src/tui/viz_viewer/state.rs:3334-3354` | Layout state fields: `right_panel_visible`, `right_panel_percent`, `layout_mode`, `inspector_is_beside` |
| `src/tui/viz_viewer/state.rs:3498-3510` | Drag state: `scrollbar_drag`, `divider_drag_offset`, `divider_drag_start_pct/col` |
| `src/tui/viz_viewer/state.rs:7787-7829` | `grow_viz_pane()` / `shrink_viz_pane()` — 5% step resize |
| `src/tui/viz_viewer/state.rs:7682-7727` | `cycle_layout_mode()` / `apply_layout_mode()` |
| `src/tui/viz_viewer/render.rs:22-29` | `SIDE_MIN_WIDTH` (100) / `SIDE_RESTORE_WIDTH` (120) hysteresis constants |
| `src/tui/viz_viewer/render.rs:2169-2177` | Divider area computation (3-col grab zone) |
| `src/tui/viz_viewer/event.rs:2313-2355` | `handle_mouse()` — hit-testing for all interactive areas |
| `src/tui/viz_viewer/event.rs:2574-2581` | Divider click → start drag |
| `src/tui/viz_viewer/event.rs:2835-2875` | Divider drag → update `right_panel_percent` |
| `src/tui/viz_viewer/event.rs:2907-2926` | Drag release → finalize LayoutMode |

### Layout Behavior

The layout has two axes:
1. **Horizontal split** (side-by-side): Graph on left, inspector on right. Used when terminal width ≥ `SIDE_MIN_WIDTH` (100 cols).
2. **Vertical split** (stacked): Graph on top, inspector on bottom. Used when width < `SIDE_MIN_WIDTH` or in Narrow breakpoint.

The `inspector_is_beside` flag with hysteresis prevents oscillation at the boundary.

**Current resize mechanisms:**
- `i` key: `grow_viz_pane()` — increases `right_panel_percent` by 5%, wraps Off→5%→...→100%→Off
- `v` key: `shrink_viz_pane()` — decreases by 5%, wraps Off→100%→...→5%→Off  
- `=` / `Shift+Tab`: `cycle_layout_mode()` — snaps through 1/3→1/2→2/3→full→off
- Mouse drag on divider: Continuous percent adjustment (horizontal split only)
- Fullscreen border strips: Click to restore from FullInspector/Off mode

**What's missing:**
- No mouse drag for the **vertical** (stacked) split divider
- No drag affordance when inspector is below the graph
- No keyboard resize for the vertical axis (panel height when stacked)

### Key Handlers

| Context | Key | Action |
|---------|-----|--------|
| Graph panel | `i` | `grow_viz_pane()` |
| Graph panel | `v` | `shrink_viz_pane()` (note: `v` also used for config view in some contexts) |
| Graph panel | `=` / `BackTab` | `cycle_layout_mode()` |
| Graph panel | `\` | `toggle_right_panel()` |
| Right panel | `i` | `grow_viz_pane()` (same) |
| Right panel | `v` | `shrink_viz_pane()` (same) |
| Right panel | `=` / `BackTab` | `cycle_layout_mode()` (same) |
| Right panel | `\` | `toggle_right_panel()` (same) |

## 4. Keybinding Conflict Analysis

### Currently Bound Keys (Graph Panel, Normal Mode)

**Navigation:** `↑↓` (task select), `jk` (scroll), `hl` (h-scroll), `Ctrl-d/u` (page), `gG` (top/bottom), `Home/End`, `PageUp/Down`

**Panels:** `Tab` (focus switch), `Alt-↑↓` (focus switch), `Alt-←→` (cycle inspector views), `\` (toggle panel), `i` (grow viz), `v` (shrink viz), `=`/`BackTab` (cycle layout), `0-9` (switch tabs)

**Actions:** `a` (new task), `D` (done), `f` (fail), `x` (retry), `e` (edit), `A` (archive), `c`/`:` (chat), `Enter` (select)

**Tracing:** `t` (trace), `T` (tokens), `Shift-↑↓` (scroll detail)

**General:** `s` (sort), `r` (refresh), `m` (mouse toggle), `X` (swap scroll axis), `.` (system tasks), `<` (running system), `*` (touch echo), `L` (coord log), `J` (JSON mode), `R` (raw JSON), `n/N` (search next/prev), `/` (search), `?` (help), `q` (quit)

### Available Keys (Unbound in Graph Normal Mode)
Lowercase: `b`, `d`, `o`, `p`, `u`, `w`, `y`, `z`
Uppercase: `B`, `C`, `E`, `F`, `H`, `I`, `K`, `M`, `O`, `P`, `Q`, `S`, `U`, `V`, `W`, `Y`, `Z`
Symbols: `-`, `+`, `>`, `,`, `;`, `'`, `"`, `` ` ``
Modified: `Ctrl-arrows` (partially), `Shift-←→`, various `Ctrl+letter` combos

### Potential Conflicts
- `I` (uppercase i) — currently **unbound**, safe to use
- `+`/`-` — currently **unbound** in graph mode, used in service control panel (`+` grow, `-` shrink max_agents)
- `Ctrl+Arrow` — not currently bound, but some terminals intercept these (e.g., macOS Terminal uses Ctrl+Left/Right for word navigation)

## 5. Proposed Interaction Design

### Recommendation: Extend Existing Model + Add Vertical Divider Drag

The current design already has the right primitives. Rather than introducing a new resize mode or complex keybinding scheme, the recommendation is to:

1. **Add mouse drag for the vertical (bottom) divider** — parity with horizontal
2. **Keep `i`/`v` working for both orientations** — they already adjust `right_panel_percent` which drives both layouts
3. **Add `I`/`V` for coarse (preset) resize** — complement the 5% fine-grained `i`/`v`

### Detailed Design

#### A. Mouse Resize (both axes)

**Horizontal split (inspector beside)** — Already works.

**Vertical split (inspector below)** — New:

```
┌─────────────────────────────────┐
│                                 │
│         Graph Panel             │
│                                 │
├─────────────────────────────────┤  ← Draggable divider (1-row grab zone)
│         Inspector Panel         │
│                                 │
└─────────────────────────────────┘
```

Implementation:
- Track `last_horizontal_divider_area: Rect` in state (1-row strip between graph and inspector in vertical mode)
- On `MouseEventKind::Down` in this area → start drag (`ScrollbarDragTarget::HorizontalDivider`)
- On `MouseEventKind::Drag` → update `right_panel_percent` based on row delta
- Visual feedback: Highlight the divider row on hover (same pattern as vertical divider)

This mirrors the existing vertical divider implementation almost exactly.

#### B. Keyboard Resize

**Keep existing bindings unchanged:**

| Key | Action | Step |
|-----|--------|------|
| `i` | Grow inspector (shrink graph) | 5% per press |
| `v` | Shrink inspector (grow graph) | 5% per press |
| `=` / `BackTab` | Cycle presets | 1/3 → 1/2 → 2/3 → full → off |

These already work for both horizontal and vertical layouts because they modify `right_panel_percent`, which controls the split ratio regardless of orientation.

**New: `I` / `V` for larger jumps (optional enhancement):**

| Key | Action |
|-----|--------|
| `I` | Jump to next `LayoutMode` preset (grow inspector) |
| `V` | Jump to previous `LayoutMode` preset (shrink inspector) |

This gives coarse preset snapping (1/3→1/2→2/3) with `I`/`V` and fine 5% control with `i`/`v`. The help screen already shows `i/I` as "resize pane" (render.rs:6868), so users expect both to be related.

**Why NOT a resize mode:**
- Zellij's modal approach works for arbitrary pane grids (N borders). We have exactly ONE divider at a time (horizontal or vertical, never both). A mode for a single axis is overkill.
- The existing `i`/`v` + mouse drag covers the use case with zero learning curve.

#### C. Visual Affordances

**Horizontal divider (new, vertical layout):**
```
──────────────────────────────── (dim when idle)
══════════════════════════════ (bright when hovered/dragging)
```

Use `─` characters (or thin `━`) for the divider row. On hover (`divider_hover`), switch to bright/bold style. Same visual language as the existing vertical divider.

**Cursor hint (optional):** When hovering the divider, render a centered `↕` or `⇕` glyph in the divider row to suggest draggability.

**Vertical divider (existing):**
Already shows visual feedback via `divider_hover` state and bright styling during drag.

#### D. ASCII Mockup

**Side-by-side (wide terminal, ≥100 cols):**
```
┌──── Graph ──────────────║─── Inspector ─────────┐
│                         ║                        │
│  task-a ───► task-b     ║  Detail | Chat | Log   │
│       └───► task-c      ║  ─────────────────     │
│                         ║  Status: in-progress   │
│                         ║  Agent: agent-42       │
│                         ║                        │
│  ↑↓ navigate   i/v resize ←drag║→               │
└─────────────────────────║────────────────────────┘
```
The `║` column is the draggable vertical divider (3-col grab zone, already implemented).

**Stacked (narrow terminal, <100 cols):**
```
┌─────────────────────────────────┐
│         Graph Panel             │
│  task-a ───► task-b             │
│       └───► task-c              │
├─────────────────────────────────┤  ← NEW: draggable horizontal divider
│         Inspector Panel         │  ↕ drag to resize
│  Detail | Chat | Log           │
│  Status: in-progress           │
└─────────────────────────────────┘
```
The `─────` row becomes the draggable horizontal divider (1-row grab zone, **new**).

### Summary Matrix

| Interaction | Horizontal (side-by-side) | Vertical (stacked) |
|-------------|--------------------------|-------------------|
| Mouse drag | ✅ Already works | 🆕 Add horizontal divider drag |
| Fine resize (`i`/`v`) | ✅ Already works | ✅ Already works (same `right_panel_percent`) |
| Preset cycle (`=`) | ✅ Already works | ✅ Already works |
| Coarse snap (`I`/`V`) | 🆕 Optional | 🆕 Optional |
| Visual hover | ✅ Already works | 🆕 Add hover highlight |

### Implementation Priority

1. **P0 — Horizontal divider drag** (stacked layout mouse resize): ~100 LOC, mirrors existing vertical divider logic
2. **P0 — Horizontal divider hover visual**: ~30 LOC in render.rs
3. **P1 — `I`/`V` preset jump**: ~20 LOC, wire existing `cycle_layout_mode()` / `cycle_layout_mode_reverse()`
4. **P2 — Drag cursor hint** (`↕`/`↔` glyph on hover): ~10 LOC, polish

### Implementation Notes

- The `right_panel_percent` field already controls both layouts. No new state is needed for the ratio.
- Add `last_horizontal_divider_area: Rect` to `VizApp` state (parallel to `last_divider_area`).
- Add `horizontal_divider_hover: bool` (parallel to `divider_hover`).
- Add `HorizontalDivider` variant to `ScrollbarDragTarget` enum.
- In `render.rs`, compute `last_horizontal_divider_area` in the vertical split branches (where `top_height`/`panel_height` are calculated).
- In `event.rs:handle_mouse()`, add hit-testing for horizontal divider and drag handling (row-based instead of column-based).
- The delta-based drag calculation should use row delta and `main_area.height` instead of column delta and `total_width`.
