# Audit Findings: Manual Section 2 - Task Graph

## Overview
Audited `docs/manual/02-task-graph.md` and `docs/manual/02-task-graph.typ` against current CLI help and runtime behavior.

**Date:** 2026-04-12  
**Files Audited:**
- docs/manual/02-task-graph.md (lines 1-450)
- docs/manual/02-task-graph.typ (lines 1-442)

## Verification Results

### ✅ VERIFIED: Core Task Management Commands

All documented commands exist and function as described:

| Command | Documentation Claim | CLI Help Status | Notes |
|---------|-------------------|-----------------|-------|
| `wg add` | Creates tasks with dependencies | ✅ Verified | All documented flags present |
| `wg edit` | Modifies existing tasks | ✅ Verified | Includes --add-after for cycle creation |
| `wg done` | Marks task complete | ✅ Verified | Has --converged flag for cycle termination |
| `wg fail` | Marks task failed | ✅ Verified | Has --reason flag as documented |
| `wg abandon` | Marks task abandoned | ✅ Verified | Has --reason flag |
| `wg approve` | Approves pending validation | ✅ Verified | Part of verification workflow |
| `wg reject` | Rejects pending validation | ✅ Verified | Has --reason flag as documented |
| `wg retry` | Retries failed task | ✅ Verified | Resets to open status |
| `wg pause` | Pauses task execution | ✅ Verified | Coordinator skips until resumed |
| `wg resume` | Resumes paused task | ✅ Verified | Has --only flag for single task |
| `wg wait` | Parks task until condition met | ✅ Verified | Has --until and --checkpoint flags |

### ✅ VERIFIED: Dependency Syntax

**`--after` flag (lines 119-143 in docs)**
- ✅ Documented: "B comes after A" expressed via `--after A` on task B
- ✅ CLI Help: `wg add` has `--after <AFTER>...` (multiple values allowed)
- ✅ CLI Help: `wg edit` has `--add-after` and `--remove-after`

**Bidirectional references (lines 137-139 in docs)**
- ✅ Documented: "`before` is computed inverse of `after`"
- Note: CLI doesn't expose --before flag (correctly, as it's computed)

### ✅ VERIFIED: Cycle Configuration

**Cycle Configuration Fields (lines 172-185 in docs)**

| Field | Documentation | CLI Status | Notes |
|-------|---------------|------------|-------|
| `max_iterations` | Hard cap on iterations | ✅ `--max-iterations` | On both add/edit |
| `guard` | Condition for iteration | ✅ `--cycle-guard` | task:id=status format |
| `delay` | Duration between iterations | ✅ `--cycle-delay` | Human readable format |
| `no_converge` | Prevents early convergence | ✅ `--no-converge` | On both add/edit |
| `restart_on_failure` | Auto-restart on failure | ✅ `--no-restart-on-failure` | On both add/edit |

**Convergence Signaling (lines 272-282 in docs)**
- ✅ Documented: `wg done <task-id> --converged` for early termination
- ✅ CLI Help: `wg done` has `--converged` flag with correct description

### ✅ VERIFIED: Analysis Commands

**Graph Analysis Tools (lines 403-425 in docs)**

| Command | Documentation Claim | CLI Status | Notes |
|---------|-------------------|------------|-------|
| `wg critical-path` | Longest dependency chain | ✅ Verified | Shows critical path |
| `wg bottlenecks` | Tasks blocking most work | ✅ Verified | Ranks by transitive dependents |
| `wg impact <task>` | What depends on this task | ✅ Verified | Traces dependents |
| `wg viz` | Text graph visualization | ✅ Verified | Multiple output formats |
| `wg ready` | Tasks ready for dispatch | ✅ Verified | Lists ready tasks |
| `wg list` | All tasks | ✅ Verified | Has --status filter |
| `wg show` | Task details | ✅ Verified | Shows full task info |
| `wg check` | Graph validation | ✅ Verified | Finds cycles, orphan refs |
| `wg cycles` | Cycle analysis | ✅ Verified | Tarjan's SCC algorithm |

### ✅ VERIFIED: Placement Hints

**Automatic Placement (lines 294-302 in docs)**

| Feature | Documentation | CLI Status | Notes |
|---------|---------------|------------|-------|
| `--no-place` | Skip auto-placement | ✅ Verified | On wg add |
| `--place-near <IDS>` | Place near specified tasks | ✅ Verified | On wg add |
| `--place-before <IDS>` | Insert before tasks | ✅ Verified | On wg add |

### ✅ VERIFIED: Timing Controls

**Time Constraints (lines 150-156 in docs)**

| Feature | Documentation | CLI Status | Notes |
|---------|---------------|------------|-------|
| `delay` field | Duration before ready | ✅ `--delay` | On add/edit |
| `not_before` field | Absolute dispatch time | ✅ `--not-before` | ISO 8601 format |

### ✅ VERIFIED: Task Fields

**Table of Task Fields (lines 11-40 in docs)**
Cross-referenced task field descriptions against CLI help:

All documented task fields have corresponding CLI flags:
- ✅ `--id`, `--description`, `--tag`, `--skill`, `--input`, `--deliverable`
- ✅ `--model`, `--provider`, `--exec-mode`, `--verify`, `--assign` 
- ✅ `--visibility`, `--context-scope`, `--hours`, `--cost`
- ✅ `--paused`, `--exec`, `--timeout`

### ✅ VERIFIED: Status Lifecycle

**Eight Task Statuses (lines 44-106 in docs)**
All documented statuses align with CLI behavior:
- Open, InProgress, Done, Failed, Abandoned, PendingValidation, Waiting, Blocked

**State Transitions**
- ✅ `wg done` → Done (or PendingValidation if --verify set)
- ✅ `wg fail --reason` → Failed  
- ✅ `wg abandon --reason` → Abandoned
- ✅ `wg wait --until` → Waiting
- ✅ `wg approve` → Done (from PendingValidation)
- ✅ `wg reject --reason` → Open (from PendingValidation)

## Minor Documentation Enhancements Identified

### 1. CLI Flag Details Missing in Examples

**Lines 230-240 (cycle creation example):**
```bash
wg add "write-draft" --max-iterations 5 --cycle-guard "task:review-draft=failed"
```
- ✅ Syntax is correct
- ℹ️ Could mention that guard format supports "always" as documented in CLI help

### 2. Additional CLI Flags Not Documented

Found in CLI help but not mentioned in manual:
- `--allow-phantom` - allows forward-reference dependencies
- `--independent` - suppresses implicit --after dependency  
- `--propagation` - retry propagation policy
- `--retry-strategy` - retry strategy configuration
- `--max-retries` - maximum retries allowed
- `--verify-timeout` - verification timeout override
- `--max-failure-restarts` - cycle restart limits
- `--cron` - cron schedule expression

These are advanced features that may intentionally be omitted from the core manual.

### 3. Visualization Options

**Line 416 (wg viz description):**
- ✅ Documents text output and Unicode box-drawing
- ℹ️ Could mention additional formats available: --dot, --mermaid, --graph, --tui

## Summary

**✅ VERIFICATION COMPLETE**

Manual section 2 accurately documents the current CLI behavior for:
- All core task management commands and flags
- Dependency syntax with --after
- Complete cycle configuration system  
- All analysis and visualization tools
- Placement hints and timing controls
- Task field structure and status lifecycle

**No critical discrepancies found.** Documentation is current and accurate as of 2026-04-12.

Minor enhancements could include mentioning some advanced CLI flags, but core functionality is fully documented and verified.