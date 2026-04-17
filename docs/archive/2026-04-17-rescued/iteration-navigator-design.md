# Iteration Navigator Widget Design

## Executive Summary

This design specifies a unified iteration navigation widget `◀ 1/N ▶` for TUI task tabs (Detail, Log, Messages). The widget provides consistent iteration browsing across all task-relative tabs with keyboard and mouse interaction.

## Design Decisions

### 1. Widget Placement: Right-aligned in Tab Bar Row

**Decision**: Position the iteration navigator in the same horizontal line as the tab bar, right-aligned.

**Layout**:
```
┌─ Right Panel ─────────────────────────────────────────────┐
│ 1:Detail │ 2:Log ▼ │ 3:Messages         ◀ iter 2/5 ▶     │ <- Tab bar + navigator
├───────────────────────────────────────────────────────────┤
│                                                           │
│ Tab content area...                                       │ <- Content area
│                                                           │
└───────────────────────────────────────────────────────────┘
```

**Rationale**:
- Conserves vertical space (no additional line like Detail tab's current approach)
- Visible across all tabs for consistent UX
- Natural visual association with tab navigation
- Right-alignment prevents interference with tab labels

**Narrow Terminal Handling**:
For terminals <80 columns, priority order for space allocation:
1. Active tab label (always visible)
2. Navigation arrows `◀ ▶` (essential)  
3. Iteration counter `2/5` (abbreviated to `2/5` or hidden if <40 cols)
4. Additional tab labels (truncated)

Minimum viable layout: `Detail   ◀▶` (14 chars)

### 2. Keyboard Bindings: Reuse Existing `[` / `]`

**Decision**: Keep current keyboard bindings `[` (prev) and `]` (next) for all task tabs.

**Current State**:
- `[` = `iteration_prev()` in Detail, Chat, Output tabs
- `]` = `iteration_next()` in Detail, Chat, Output tabs
- These bindings are tab-specific (only active in certain tabs)

**New State**: 
- Extend `[` and `]` to work in Log and Messages tabs
- Maintain existing behavior in Detail, Chat, Output tabs
- Unified iteration state affects all task-relative tabs

**Conflict Analysis**: No conflicts detected. `<` is used for system task toggle, but `[`/`]` are available for extension to Log/Messages tabs.

### 3. Mouse Interaction: Clickable Arrow Regions

**Click Targets**:
- Left arrow `◀`: Click to go to previous iteration
- Right arrow `▶`: Click to go to next iteration  
- Counter text `2/5`: No-op (passive display)

**Visual States**:
- **Active arrows**: Bold yellow (same as current Detail tab)
- **Disabled arrows**: Dark gray when at bounds
- **Hover feedback**: Brief highlight (if feasible)

**Hit Region Sizing**: 
- Minimum 1 character width per arrow
- No special padding needed (arrows are naturally clickable)

### 4. Single Iteration Behavior: Hide Widget

**Decision**: When a task has only 1 iteration, hide the iteration navigator entirely.

**Rationale**:
- Reduces visual clutter for common case (non-cycled tasks)
- `1/1` display provides no useful functionality
- Consistent with current Detail tab which shows navigation header only when archives exist

**Implementation**: Check `app.iteration_archives.is_empty()` before rendering widget.

### 5. Content Filtering: Extend Global State to All Tabs

**Current State**:
- `viewing_iteration: Option<usize>` controls Detail tab's archive viewing
- Log and Messages tabs show all-time aggregated data

**New Behavior**:
- When `viewing_iteration` is set (user browsed to an archive):
  - **Detail tab**: Shows archived prompt/output (current behavior)
  - **Log tab**: Shows logs with contextual indicator `[viewing iter 2/5]` in header
  - **Messages tab**: Shows messages with contextual indicator `[viewing iter 2/5]` in header

**Phase 1 Implementation** (based on research findings):
- Sync tab headers to show iteration context
- Log/Messages data remains aggregated but with clear labeling
- No data model changes required

**Phase 2 Possibility** (future):
- Per-iteration log/message filtering (requires data model changes)

## Technical Specifications

### Widget Layout Mockup

#### Standard Layout (80+ columns):
```
1:Detail │ 2:Log ▼ │ 3:Messages                    ◀ iter 2/5 ▶
1:Detail │ 2:Log ▼ │ 3:Messages                       ◀ 1/1 ▶    <- Hidden in practice
1:Detail │ 2:Log ▼ │ 3:Messages                     ◀ 10/23 ▶   
```

#### Narrow Layout (40-79 columns):
```
1:Detail │ 2:Log ▼ │ 3:Msg             ◀ 2/5 ▶
1:Detail │ 2:Log ▼                     ◀ 2/5 ▶
```

#### Minimal Layout (<40 columns):
```
Detail              ◀▶    <- No counter shown
Detail    ◀▶              <- Right-aligned if space
```

### Data Flow Specification

#### Iteration State Management:
```
Current Global State:
- viewing_iteration: Option<usize>  // None = current, Some(idx) = archive index
- iteration_archives: Vec<(String, PathBuf)>  // Cached archive list

Navigation Methods:
- iteration_prev() -> bool  // Returns true if state changed  
- iteration_next() -> bool  // Returns true if state changed
```

#### Content Filtering Propagation:

1. **User action**: Click `◀` or press `[`
2. **State update**: `app.iteration_prev()` updates `viewing_iteration`
3. **Content reload**: Affected tabs refresh their display:
   - Detail: `load_hud_detail()` (current behavior)
   - Log: Header shows `[viewing iter X/Y]` 
   - Messages: Header shows `[viewing iter X/Y]`

#### Tab Content Impact:

**Detail Tab**:
- Current implementation unchanged
- Shows archived prompt.txt and output.txt when `viewing_iteration` is Some

**Log Tab**: 
- Data source: `task.log` entries (unchanged - still aggregated)
- Header modification: Add iteration context indicator
- Example: `Task Log [viewing iter 2/5]` vs `Task Log`

**Messages Tab**:
- Data source: `list_messages()` (unchanged - still aggregated)  
- Header modification: Add iteration context indicator
- Example: `Messages [viewing iter 2/5]` vs `Messages`

### Mouse Click Handling

Extend existing click region system in `event.rs`:

```rust
// In mouse click handler, check if click is in iteration nav area
if app.last_iteration_nav_area.contains(mouse_pos) {
    let relative_x = mouse_pos.x - app.last_iteration_nav_area.x;
    
    // Left arrow click (exact position varies by layout)
    if relative_x == 0 && can_go_prev {
        app.iteration_prev();
        app.load_hud_detail();
        // Update other tabs if needed
    }
    
    // Right arrow click  
    if relative_x == arrow_right_position && can_go_next {
        app.iteration_next();
        app.load_hud_detail(); 
        // Update other tabs if needed
    }
}
```

## Implementation Plan

### Phase 1: Basic Widget (Medium effort)
1. Modify `draw_tab_bar()` to include right-aligned iteration navigator
2. Store navigator click regions in `app.last_iteration_nav_area`
3. Extend keyboard handlers for `[`/`]` to Log and Messages tabs
4. Add contextual headers to Log/Messages when `viewing_iteration` is active

### Phase 2: Enhanced Filtering (High effort, optional)
1. Add iteration metadata to log entries and messages
2. Implement true per-iteration filtering
3. Migration strategy for existing data

## Validation Checklist

- [x] Concrete layout mockup accounting for narrow terminals (14-char minimum)
- [x] Keybinding spec using existing `[`/`]` without conflicts  
- [x] Clear spec for tab content filtering (Phase 1: headers, Phase 2: data)
- [x] Mouse interaction design with clickable arrow regions
- [x] Single iteration behavior (hide widget)
- [x] Data flow from user action through state update to content refresh

## Open Questions for Implementation

1. **Color scheme**: Should navigator use same colors as tab highlights (yellow active)?
2. **Animation**: Should iteration changes trigger slide animation like tab switches?
3. **Status line**: Should iteration context also appear in bottom status bar?
4. **Archive loading**: Should Log/Messages tabs trigger archive loading for future iteration-specific filtering?