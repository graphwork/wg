# Research: Log View vs Chat View Scrollbar and Interaction Differences

## Relevant Source Files

| File | Purpose |
|------|---------|
| `src/tui/viz_viewer/render.rs` | All panel rendering including scrollbars and "new content" indicators |
| `src/tui/viz_viewer/event.rs` | Mouse event handling (click, drag, release) for scrollbars |
| `src/tui/viz_viewer/state.rs` | State structs (`LogPaneState`, `OutputPaneState`, `ScrollbarDragTarget`, etc.) |

### Key Functions

| Function | File:Line | Purpose |
|----------|-----------|---------|
| `draw_chat_tab` | `render.rs:2530` | Renders the Chat tab (coordinator chat messages) |
| `draw_log_tab` | `render.rs:3742` | Renders the Log tab (task activity log + agent output) |
| `draw_output_tab` | `render.rs:4326` | Renders the Output tab (live agent streaming output) |
| `draw_panel_scrollbar` | `render.rs:1925` | Shared scrollbar renderer for ALL right-panel tabs |
| `vscrollbar_jump_panel` | `event.rs:3214` | Maps scrollbar click/drag row to scroll position per tab |
| `handle_mouse` (mousedown) | `event.rs:2517-2531` | Starts `ScrollbarDragTarget::Panel` drag on scrollbar click |
| `handle_mouse` (drag) | `event.rs:2837-2839` | Continues `Panel` drag via `vscrollbar_jump_panel` |
| `handle_mouse` (release) | `event.rs:2878-2879` | Clears `scrollbar_drag` on mouse release |

## Finding 1: Scrollbar Dragging Already Works for the Log View

**Both the chat view and the log view use the exact same scrollbar infrastructure.**

The scrollbar rendering path is identical:
- **Chat** (`render.rs:3298-3307`): calls `draw_panel_scrollbar(frame, app, msg_area, ...)`
- **Log** (`render.rs:3951-3953`): calls `draw_panel_scrollbar(frame, app, area, ...)`
- **Output** (`render.rs:4588-4590`): calls `draw_panel_scrollbar(frame, app, content_area, ...)`

All three tabs store their scrollbar area in `app.last_panel_scrollbar_area` via `draw_panel_scrollbar` (`render.rs:1938`).

The mouse event handler treats all panel scrollbar interactions uniformly:
1. **MouseDown** on `last_panel_scrollbar_area` → sets `scrollbar_drag = Some(ScrollbarDragTarget::Panel)` and calls `vscrollbar_jump_panel` (`event.rs:2525-2531`)
2. **Drag** while `scrollbar_drag == Panel` → calls `vscrollbar_jump_panel` again (`event.rs:2837-2839`)
3. **MouseUp** → clears `scrollbar_drag` (`event.rs:2878-2879`)

The `vscrollbar_jump_panel` function dispatches per-tab at `event.rs:3234`:
- `RightPanelTab::Chat` → updates `app.chat.scroll` (inverted model: 0 = bottom)
- `RightPanelTab::Log` → updates `app.log_pane.scroll` + manages `auto_tail`
- `RightPanelTab::Output` → updates per-agent `scroll_state.scroll` + `auto_follow`

**Conclusion: The scrollbar handle IS clickable and draggable in the log view.** The infrastructure is shared and works identically across all right-panel tabs. If the user reports it doesn't work, possible causes are:

1. **The scrollbar auto-hides after 2 seconds** (`panel_scrollbar_visible()` at `state.rs:6734`). The user may be trying to click an area where the scrollbar has faded. The scrollbar only appears when `panel_scroll_activity` is within 2 seconds or while `scrollbar_drag == Panel`. You must scroll first (mouse wheel or keyboard) to make it visible, then click-drag within 2 seconds.
2. **The hit-test area (`last_panel_scrollbar_area`) is only 1 column wide** (`render.rs:1932-1937`). It can be hard to precisely click on a single column.
3. **A rendering overlap** could obscure the scrollbar. The "new content" indicator renders at the bottom-right corner, potentially overlapping the scrollbar track.

## Finding 2: "New Content" Indicator Is Not Interactive

Both the log view (`render.rs:3957-3976`) and output view (`render.rs:4600-4619`) render a `"▼ new output"` indicator when `has_new_content` is true and auto-tail/auto-follow is off.

The indicator is rendered as a `Paragraph` widget at a computed `ind_area` Rect positioned at the bottom-right of the content area:

```rust
// Log view (render.rs:3961-3966)
let ind_area = Rect {
    x: area.x + area.width - indicator_len - 1,
    y: area.y + area.height.saturating_sub(1),
    width: indicator_len + 1,
    height: 1,
};
```

**The `ind_area` is NOT stored in any app state field.** There is no hit-test area recorded, no mouse click handler that checks for clicks on this indicator, and no keyboard shortcut mapped to it.

**What's missing:**
1. No state field like `last_log_new_content_area: Rect` to store the indicator's position for hit-testing
2. No mouse click handler in `handle_mouse` that checks if a click falls within the indicator area
3. No keyboard shortcut (e.g., pressing Enter or a specific key while the indicator is visible) to scroll to bottom

## Recommended Fix Approach

### Issue 1: Scrollbar Drag (May Not Actually Be Broken)

The scrollbar drag **already works** for the log view through the shared `ScrollbarDragTarget::Panel` mechanism. Before implementing a fix, verify the bug:

1. Open the TUI, switch to the Log tab with enough content to scroll
2. Scroll up (mouse wheel or `k`/`Up`) to make the scrollbar visible
3. Within 2 seconds, click and drag the scrollbar track

If the bug is confirmed, the most likely cause is the **auto-hide timeout**. Potential fixes:
- **Option A**: Keep the scrollbar visible while hovering over the `last_panel_scrollbar_area` (requires tracking hover state in `handle_mouse` for `MouseEventKind::Moved`)
- **Option B**: Increase the fade timeout from 2s to something longer, or make it configurable
- **Option C**: Always show the scrollbar when content overflows (remove the auto-hide entirely for the log view)

### Issue 2: Interactive "New Content" Indicator

This requires three changes:

**Step 1: Store the indicator area for hit-testing** (`state.rs`)

Add a field to `VizApp`:
```rust
pub last_new_content_indicator_area: Rect,
```

Reset it each frame alongside the other scrollbar areas (in `render.rs:85-87`):
```rust
app.last_new_content_indicator_area = Rect::default();
```

**Step 2: Record the area when rendering** (`render.rs`)

In both `draw_log_tab` (~line 3961) and `draw_output_tab` (~line 4604), after computing `ind_area`, store it:
```rust
app.last_new_content_indicator_area = ind_area;
```

**Step 3: Handle mouse clicks** (`event.rs`)

In the `MouseDown(Left)` handler, add a check before the general right-panel click handling:
```rust
if app.last_new_content_indicator_area.height > 0
    && app.last_new_content_indicator_area.contains(pos)
{
    // Scroll to bottom and clear indicator
    right_panel_scroll_to_bottom(app);
    // Consumed — don't process as a regular click
}
```

The existing `right_panel_scroll_to_bottom` function (`event.rs:2257`) already handles scrolling to the bottom for each tab type and clearing `has_new_content` / setting `auto_tail`.

**Step 4 (optional): Keyboard shortcut**

Consider mapping a key (e.g., `G` or `End`) when the indicator is visible to call `right_panel_scroll_to_bottom`. Note: `G` already scrolls to bottom in the log view via existing keyboard handling, so this may already work. The indicator just needs the mouse click path.

### Files to Modify

| File | Change |
|------|--------|
| `src/tui/viz_viewer/state.rs` | Add `last_new_content_indicator_area: Rect` field to `VizApp` |
| `src/tui/viz_viewer/render.rs` | Store `ind_area` in app state; reset each frame |
| `src/tui/viz_viewer/event.rs` | Add click handler for the indicator area |

### Estimated Complexity

- **Issue 1** (scrollbar drag): Likely a false positive — verify first. If real, small change (~10-20 lines).
- **Issue 2** (interactive indicator): Small change (~15-25 lines across 3 files). Low risk, purely additive.
