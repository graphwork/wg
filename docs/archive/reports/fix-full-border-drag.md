# Full inspector boundary drag regression

## Result

Full inspector mode now keeps exactly one graph-facing boundary visible and makes that literal painted cell row/column the pointer and touch target. There is still one contextual row and no four-sided outer frame.

Physical geometry follows the resolved dock:

- Right: left `│` boundary; drag right to reveal Graph.
- Left: right `│` boundary; drag left.
- Bottom: top `─` boundary; drag down.
- Top: bottom `─` boundary; drag up.
- Auto: uses Right at wide geometry and Bottom at narrow/mobile geometry without rewriting the remembered Auto preference.

A boundary Down safely restores the remembered bounded Split immediately. Drag or touch-style Moved events then resize continuously from that ratio; Up persists and retires capture. A tap is therefore a defined exact-ratio restore, and a sub-percent short drag is consumed without a ratio jump. Workspace and the contextual `↔/↕ Split` control remain fallbacks.

## Input ownership and invalidation

Boundary routing occurs before contextual controls, Chat PTY/editor/content routing, and Graph selection/panning. While captured, only left-button Drag, touch Moved, and left-button Up are accepted; other events are swallowed.

The capture snapshot records viewport, resolved physical dock, desired dock/mode, graph identity, and authenticated service owner. Resize/rotation restores the pointer-down layout and cancels. Dock/mode changes, graph switches, responsive edge changes, service connection changes/restarts/disconnects, stale motion, and mouse-up drop capture and retire old hit coordinates.

## Repaint model

The renderer tracks the last painted layout seam separately from the live hit rectangle. It locally resets only those old cells before drawing the next frame, then fully resets and paints every cell of the current seam. This covers Full↔Split↔Hidden, ratio moves, dock changes, and rotation without a global terminal clear, duplicate seam, or ghost style.

## Validation evidence

- `tui::viz_viewer::event` tests derive Full targets from TestBackend-rendered `│`/`─` cells for Left, Right, Top, Bottom, Auto-wide, and Auto-narrow; they drive Down/Moved/Up and verify natural inverse geometry.
- Event tests define tap/short-drag behavior and verify retired streams, graph switch, mode/dock change, disconnect, and resize invalidation.
- `tui::viz_viewer::render` tests verify one contextual row, one exact one-cell seam target, zero outer frames, PTY sizing/identity, and complete glyph painting.
- `tests/smoke/scenarios/tui_inspector_drag_to_full.sh` builds the candidate binary and sends real SGR events through tmux under Termux+mosh transport detection. It locates and drags the literal Full left boundary in Right/Auto-wide Chat, Detail, and Log, plus the visible top boundary in explicit Bottom and Auto-narrow/mobile modes. It also checks fallback controls, PTY input confinement/PID stability, restart, and resize cancellation.
- `tests/smoke/scenarios/tui_split_seam_redraw.sh` verifies Full's boundary and its vacated cells across Full/Split/Hidden and desktop/Termux+mosh geometry without global clears or blank flicker frames.
