# Iteration Navigator - Data Flow Specification

## Overview

This document specifies how iteration selection propagates from the navigation widget through the application state to affect tab content display.

## Global State Model

### Current State Variables

```rust
struct VizApp {
    // Global iteration browsing state
    viewing_iteration: Option<usize>,        // None = current, Some(idx) = archive index  
    iteration_archives: Vec<(String, PathBuf)>, // Cached list of available archives
    
    // Widget layout tracking
    last_iteration_nav_area: Rect,          // Click region for mouse handling
    
    // Tab-specific state
    hud_detail: Option<HudDetail>,           // Detail tab content
    log_pane: LogPaneState,                 // Log tab content
    messages_panel: MessagesPanelState,     // Messages tab content
}
```

### Archive Storage Structure
```
.wg/log/agents/{task_id}/
├── 20241201_143022_agent123/     <- iteration_archives[0] (oldest)
│   ├── prompt.txt
│   └── output.txt
├── 20241201_143155_agent456/     <- iteration_archives[1]  
│   ├── prompt.txt
│   └── output.txt
└── 20241201_143301_agent789/     <- iteration_archives[2] (newest archive)
    ├── prompt.txt
    └── output.txt
                                  <- "current" = viewing_iteration = None
```

## Data Flow Chains

### 1. User Interaction → State Update

```mermaid
graph TD
    A[User clicks ◀ or presses '['] --> B[iteration_prev() called]
    C[User clicks ▶ or presses ']'] --> D[iteration_next() called]
    
    B --> E{At oldest archive?}
    E -->|Yes| F[No state change, return false]
    E -->|No| G[Update viewing_iteration]
    
    D --> H{At current iteration?}  
    H -->|Yes| I[No state change, return false]
    H -->|No| J[Update viewing_iteration]
    
    G --> K[Return true - state changed]
    J --> K
    K --> L[Trigger content reload]
```

### 2. State Update → Content Reload

```rust
// Pseudo-code for state change handling
fn handle_iteration_navigation_change() {
    if iteration_state_changed {
        // Always reload Detail tab (current behavior)
        app.load_hud_detail();
        
        // Phase 1: Update tab headers with context
        update_log_tab_header();
        update_messages_tab_header(); 
        
        // Phase 2 (future): Reload tab content with filtering
        // app.load_log_pane_filtered();
        // app.load_messages_panel_filtered();
    }
}
```

### 3. Content Reload → Display Update

#### Detail Tab (Current Implementation)
```rust
fn load_hud_detail() {
    match app.viewing_iteration {
        None => {
            // Load current task state
            load_current_prompt_and_output()
        },
        Some(archive_idx) => {
            // Load archived iteration content  
            let archive_path = &app.iteration_archives[archive_idx].1;
            load_archived_content(archive_path)
        }
    }
}
```

#### Log Tab (New Implementation)
```rust
fn update_log_tab_header() -> String {
    let base_title = "Task Log";
    
    match app.viewing_iteration {
        None => base_title.to_string(),
        Some(idx) => {
            let total = app.iteration_archives.len() + 1;
            format!("{} [viewing iter {}/{}]", base_title, idx + 1, total)
        }
    }
}

// Data source unchanged: task.log entries (all iterations)
fn load_log_pane() -> LogPaneState {
    // Current implementation - no filtering by iteration
    LogPaneState {
        entries: task.log.clone(), // All log entries across all iterations
        // ... other fields
    }
}
```

#### Messages Tab (New Implementation)  
```rust
fn update_messages_tab_header() -> String {
    let base_title = "Messages";
    
    match app.viewing_iteration {
        None => base_title.to_string(),
        Some(idx) => {
            let total = app.iteration_archives.len() + 1;
            format!("{} [viewing iter {}/{}]", base_title, idx + 1, total)
        }
    }
}

// Data source unchanged: list_messages() (all iterations)  
fn load_messages_panel() -> MessagesPanelState {
    // Current implementation - no filtering by iteration
    MessagesPanelState {
        messages: list_messages(task_id), // All messages across all iterations
        // ... other fields
    }
}
```

## Phase 1 vs Phase 2 Implementation

### Phase 1: Header Context (Recommended Start)

**Scope**: Visual indicators only, no data model changes

**Implementation**:
- Modify tab header rendering to show iteration context
- No changes to data loading logic
- Users see which iteration they're "viewing" but data is aggregated

**Benefits**:
- Quick to implement
- No data migration required  
- Provides immediate UX improvement
- Backward compatible

### Phase 2: True Filtering (Future Enhancement)

**Scope**: Per-iteration data isolation

**Requirements**:
- Add `iteration_number` field to `LogEntry` struct
- Add iteration metadata to message storage
- Implement filtered data loading APIs
- Migration strategy for existing data

**Implementation**:
```rust
// Enhanced log entry with iteration tracking
struct LogEntry {
    timestamp: String,
    message: String,
    iteration_number: Option<u32>, // New field
}

// Filtered loading functions
fn load_log_entries_for_iteration(task_id: &str, iteration: Option<usize>) -> Vec<LogEntry> {
    match iteration {
        None => load_current_iteration_logs(task_id),
        Some(idx) => load_archived_iteration_logs(task_id, idx),
    }
}
```

## Widget State Synchronization

### Navigation Widget Updates

```rust
fn render_iteration_navigator() {
    let total = app.iteration_archives.len() + 1;
    let current_display = match app.viewing_iteration {
        None => total,        // "5/5" when viewing current
        Some(idx) => idx + 1, // "2/5" when viewing archive
    };
    
    let can_go_prev = match app.viewing_iteration {
        None => !app.iteration_archives.is_empty(),
        Some(idx) => idx > 0,
    };
    
    let can_go_next = match app.viewing_iteration {
        Some(idx) => idx + 1 < app.iteration_archives.len(),
        None => false,
    };
    
    render_widget(format!("◀ iter {}/{} ▶", current_display, total));
}
```

### Cross-Tab Consistency

**Key Principle**: All task-relative tabs reflect the same `viewing_iteration` state

**Consistency Guarantees**:
1. Changing iteration in any tab affects all tabs
2. Tab switching preserves iteration selection  
3. Task switching resets iteration to current
4. Widget always shows correct position

### Error Recovery

**Missing Archive Handling**:
```rust
fn safe_load_archive(archive_idx: usize) -> Result<IterationContent, ArchiveError> {
    if archive_idx >= app.iteration_archives.len() {
        // Reset to current iteration on invalid index
        app.viewing_iteration = None;
        return load_current_iteration();
    }
    
    match load_archive_content(archive_idx) {
        Ok(content) => Ok(content),
        Err(e) => {
            // Fall back to "unavailable" display, don't reset navigation
            Ok(IterationContent::unavailable(e))
        }
    }
}
```

**State Corruption Recovery**:
- Invalid iteration indices are clamped to valid range
- Missing archive files show "unavailable" rather than crashing
- Navigation bounds are enforced to prevent infinite loops

## Performance Considerations

### Lazy Loading
- Archive content loaded on-demand when iteration is selected
- Archive list cached and refreshed only when task changes
- No preloading of all iterations (memory efficient)

### Rendering Efficiency  
- Widget renders only when iteration state changes
- Tab headers updated incrementally
- No full tab content reload for header-only changes (Phase 1)

### Memory Usage
- Only current iteration and selected archive kept in memory
- Archive metadata cached (lightweight)
- Full archive content released when switching iterations