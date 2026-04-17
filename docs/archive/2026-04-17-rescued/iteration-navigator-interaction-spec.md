# Iteration Navigator - Keyboard and Mouse Interaction Specification

## Keyboard Interactions

### Primary Navigation Bindings

| Key | Action | Scope | Behavior |
|-----|--------|-------|----------|
| `[` | Previous iteration | Task tabs (Detail, Log, Messages) | Calls `app.iteration_prev()`, updates global `viewing_iteration` state |
| `]` | Next iteration | Task tabs (Detail, Log, Messages) | Calls `app.iteration_next()`, updates global `viewing_iteration` state |

### Binding Context

**Current Implementation**: 
- `[` and `]` are only active in Detail, Chat, and Output tabs
- Different behaviors in graph navigation vs task tabs

**New Implementation**:
- Extend `[` and `]` to Log and Messages tabs when a task is selected
- Maintain existing behavior in other contexts
- No changes to existing Detail/Chat/Output tab behavior

### Conflict Resolution

**No conflicts detected**:
- `<` is used for system task toggle (unrelated)
- `>` appears unused
- `[` and `]` are contextual (only active in specific tabs)
- Navigation arrows (←/→/↑/↓) are used for different purposes

### Edge Cases

1. **No iterations available**: Keys are no-ops (safe)
2. **At iteration bounds**: Keys are no-ops (safe)
3. **Tab switching**: Iteration state persists across tab switches
4. **Task switching**: Iteration state resets when selecting different task

## Mouse Interactions

### Click Targets

#### Left Arrow `◀`
- **Location**: Left side of iteration navigator widget
- **Hit region**: Exact character position of `◀` symbol
- **Active state**: Bold yellow styling when clickable
- **Disabled state**: Dark gray when at oldest iteration
- **Action**: Same as `[` key - calls `app.iteration_prev()`

#### Right Arrow `▶`
- **Location**: Right side of iteration navigator widget  
- **Hit region**: Exact character position of `▶` symbol
- **Active state**: Bold yellow styling when clickable
- **Disabled state**: Dark gray when at newest iteration (current)
- **Action**: Same as `]` key - calls `app.iteration_next()`

#### Counter Text `X/Y`
- **Location**: Between the arrows
- **Action**: No-op (passive display only)
- **Styling**: Cyan text, no hover effects

### Visual Feedback

#### Hover States (Optional)
- Brief highlight on hover if terminal supports it
- Not required for core functionality

#### Disabled States
- Arrows shown in dark gray when navigation not possible
- Prevents user confusion about availability

### Click Region Implementation

```rust
// Store click regions during rendering
app.last_iteration_nav_area = Rect {
    x: right_aligned_x_position,
    y: tab_bar_y,
    width: navigator_total_width, // e.g., "◀ iter 2/5 ▶" = 13 chars
    height: 1,
};

// In mouse event handler
if app.last_iteration_nav_area.contains(click_pos) {
    let relative_x = click_pos.x - app.last_iteration_nav_area.x;
    
    match relative_x {
        0 => {
            // Left arrow click
            if can_go_prev {
                app.iteration_prev();
                reload_tab_content();
            }
        },
        right_arrow_offset => {
            // Right arrow click (position depends on counter length)
            if can_go_next {
                app.iteration_next(); 
                reload_tab_content();
            }
        },
        _ => {
            // Click on counter text - no action
        }
    }
}
```

### Layout-Dependent Positioning

#### Standard Layout (80+ columns)
```
◀ iter 2/5 ▶
^          ^
│          └─ Right arrow at relative position 11
└─ Left arrow at relative position 0
```

#### Compact Layout (40-79 columns)  
```
◀ 2/5 ▶
^     ^
│     └─ Right arrow at relative position 6
└─ Left arrow at relative position 0
```

#### Minimal Layout (<40 columns)
```
◀▶
^^
││└─ Right arrow at relative position 1
└─ Left arrow at relative position 0
```

## State Propagation

### Action Flow

1. **Input Event**: User clicks arrow or presses `[`/`]`
2. **State Update**: `iteration_prev()` or `iteration_next()` modifies global state
3. **Content Reload**: Affected tabs refresh their display
4. **Visual Update**: Widget reflects new iteration position

### Tab Impact

| Tab | Current Behavior | New Behavior |
|-----|-----------------|--------------|
| **Detail** | Shows archived prompt/output | Unchanged (already implemented) |
| **Log** | Shows all logs | Shows header indicator `[viewing iter X/Y]` |
| **Messages** | Shows all messages | Shows header indicator `[viewing iter X/Y]` |

### Error Handling

- Invalid iteration indices are safely ignored
- Missing archive files fall back to "unavailable" display
- Navigation bounds are enforced (no wraparound)

## Integration with Existing Systems

### Compatibility
- Fully backward compatible with existing iteration navigation
- No changes to current Detail tab behavior
- Extends functionality to Log and Messages tabs

### Performance
- No additional file I/O for Phase 1 implementation
- Minimal rendering overhead (single line of text)
- Click detection adds negligible processing time