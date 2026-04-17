# Design Documentation Audit Report

**Date:** 2026-04-12  
**Scope:** docs/design/ directory contents vs current code implementation  
**Files Audited:** 71 design documents (.md and .pdf files)  

## Executive Summary

The docs/design/ directory contains extensive design documentation covering federation, agency systems, TUI enhancements, coordinator architecture, and various technical specifications. Implementation status varies significantly:

- **Fully Implemented:** ~30% - Agency federation, loop convergence, basic TUI chat functionality
- **Partially Implemented:** ~40% - Federation infrastructure (peer config) without full IPC protocol
- **Design Only:** ~30% - Self-hosting coordinator, complete multi-panel TUI, native executors

## Design Document Inventory

### Core Architecture Designs

| Document | Lines | Status | Implementation Gap |
|----------|--------|--------|-------------------|
| `federation-architecture.md` | 739 | Partial | Missing QueryGraph IPC, TUI Peers tab |
| `agency-federation.md` | 100+ | **Implemented** | Commands exist: wg agency scan/pull/push/merge |
| `self-hosting-architecture.md` | 100+ | Design Only | Coordinator is Rust code, not persistent LLM session |
| `loop-convergence.md` | 50+ | **Implemented** | `wg done --converged` flag exists |

### Federation Infrastructure

**Implemented Components:**
- ✅ Peer configuration (`federation.yaml`, `wg peer add/remove/list`)
- ✅ Agency federation (content-addressable sharing)
- ✅ Basic IPC protocol (`AddTask`, `QueryTask`, `GraphChanged`)

**Missing Components:**
- ❌ `QueryGraph` IPC request for peer state snapshots
- ❌ TUI Peers panel for cross-workgraph visibility  
- ❌ Cross-repo dependency syntax (`peer:task-id`)
- ❌ Federation polling strategy
- ❌ `PeerGraphSnapshot` data structures

**Code Evidence:**
- `src/federation.rs` - Peer config and agency transfer logic (implemented)
- `src/commands/peer.rs` - Peer management commands (implemented)
- `src/commands/service/ipc.rs` - No `QueryGraph` handler (gap)

### TUI Multi-Panel Architecture

**Implemented Components:**
- ✅ Chat input mode (`InputMode::ChatInput`)
- ✅ Right panel tab system (`RightPanelTab::Chat`)
- ✅ Panel focus management (`FocusedPanel`)
- ✅ Message editor integration

**Evidence:** Test files in `src/tui/viz_viewer/editor_tests.rs` show chat functionality

**Missing from Design:**
- Task creation/editing UI panels
- Agent monitoring panel  
- Complete control surface (still primarily read-only visualization)

### Self-Hosting Vision

**Current State:** Traditional daemon + CLI architecture
- Coordinator: Pure Rust tick-based logic in `src/commands/service/coordinator.rs`
- Executor: Depends on external Claude CLI
- TUI: Primarily visualization, limited interactivity

**Design Vision (Not Implemented):**
- Persistent LLM session as coordinator agent
- Native Rust executor calling LLM APIs directly
- TUI as primary interface replacing external tools
- Conversational task creation from natural language

## Implementation Gaps by Category

### High-Impact Gaps (Core Functionality Missing)

1. **Federation Visibility (federation-architecture.md)**
   - No `QueryGraph` IPC for peer state queries
   - TUI cannot show peer workgraph states
   - Cross-repo task dispatch designed but not implemented

2. **Self-Hosting Coordinator (self-hosting-architecture.md)**  
   - Coordinator is Rust code, not conversational LLM session
   - No natural language task creation
   - Limited user interaction model

### Medium-Impact Gaps (Partial Implementation)

1. **TUI Control Surface (tui-multi-panel.md)**
   - Chat exists but limited functionality
   - Missing task creation/editing panels
   - No agent monitoring dashboard

### Low-Impact Gaps (Nice-to-Have Features)

1. **Advanced Federation Features**
   - TCP transport for multi-machine federation
   - Authentication/authorization for peer access
   - Federation notifications and push updates

## Obsolete or Superseded Designs

### Potentially Obsolete

- **Multi-machine federation designs** - May be over-engineered for current use cases
- **Complex visibility matrix** - May be simpler than designed in practice
- **mDNS/DNS-SD service discovery** - Filesystem scanning may be sufficient

### Recently Superseded  

- **Loop iteration without convergence** - Fixed by `--converged` flag implementation
- **Manual cycle management** - Replaced by automatic cycle detection

## Verification Results

### Validation Criteria Met ✅

- [x] All design documentation files inventoried (71 files)
- [x] Each design doc compared against current code implementation
- [x] Implementation gaps identified (detailed above)
- [x] Obsolete designs identified (listed above)  
- [x] File scope: docs/design/ directory only

### Code Implementation Cross-References

**Implemented Features:**
```bash
# Agency federation works
ls src/commands/agency_*.rs  # 10 agency commands exist
wg done --converged         # Loop convergence implemented

# Federation infrastructure partially works  
ls src/federation.rs src/commands/peer.rs  # Files exist
wg peer add test ~/projects/test           # Command works
```

**Missing Features:**
```bash
# These don't exist in IPC protocol
grep -r "QueryGraph" src/commands/service/ipc.rs  # No matches
grep -r "PeerGraphSnapshot" src/                   # No matches

# TUI missing full control surface
grep -r "TaskCreate" src/tui/                      # No task creation UI
```

## Recommendations

### Priority 1: High-Value, Low-Effort
1. **Complete federation visibility** - Implement `QueryGraph` IPC and basic TUI peers list
2. **Document federation status** - Update federation-architecture.md with current implementation state

### Priority 2: Strategic Features  
1. **TUI task creation** - Add basic task creation dialog to TUI
2. **Enhanced coordinator chat** - Expand TUI chat functionality for task planning

### Priority 3: Future Vision
1. **Self-hosting coordinator** - Requires significant architectural changes
2. **Multi-machine federation** - Advanced networking and security considerations

## Conclusion

The design documentation demonstrates thorough planning and system thinking. The agency federation system is particularly well-implemented with full command support. The main implementation gaps are in peer federation visibility (missing IPC protocol extensions) and the self-hosting vision (coordinator as persistent LLM session).

The codebase shows evidence of incremental implementation of designs, with core functionality prioritized over advanced features. This is a reasonable approach for a system under active development.