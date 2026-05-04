# Beads and Gastown Research

Comprehensive analysis of Steve Yegge's Beads memory system and Gastown multi-agent orchestrator, based on examination of actual code, documentation, and community feedback.

## 1. Repository Overview

- **Beads**: https://github.com/steveyegge/beads - "A memory upgrade for your coding agent"
- **Gastown**: https://github.com/steveyegge/gastown - "Multi-agent workspace manager"

Both projects were released in early January 2026 and are written primarily in Go.

---

## 2. Beads Data Model

### Storage Architecture

Beads uses a **three-layer model** combining the strengths of different storage systems:

1. **CLI Layer** - Commands like `bd create`, `bd list`, `bd update`
2. **SQLite Database** - Local working copy with fast queries and indexes (`.beads/beads.db`)
3. **JSONL File** - Git-tracked source of truth (`.beads/issues.jsonl`)

The design philosophy: "SQLite for speed, JSONL for git, Git for distribution."

### Directory Structure

```
.beads/
  beads.db          # SQLite cache (fast queries, not committed to git)
  issues.jsonl      # Git-tracked issue data (one JSON object per line)
  metadata.json     # Backend configuration
  config.yaml       # User configuration
  deletions.jsonl   # Tombstone manifest for tracking deleted issues
  bd.sock           # Unix socket for daemon communication
```

### Issue ID Format

Beads uses **hash-based IDs** to prevent merge collisions in multi-agent workflows:
- Format: `<prefix>-<hash>` (e.g., `bd-a3f8e9`)
- Supports hierarchical structures: `bd-a3f8` (epic) -> `bd-a3f8.1` (task) -> `bd-a3f8.1.1` (subtask)

### JSONL Schema

Each line in `issues.jsonl` is a complete JSON object representing one issue. Based on the documentation and Go package exports, an issue contains approximately 40 fields:

**Core Fields:**
```json
{
  "id": "bd-a3f8",
  "title": "Implement feature X",
  "description": "Detailed description...",
  "status": "in_progress",
  "priority": 1,
  "issue_type": "feature",
  "assignee": "agent-1",
  "created_at": "2026-01-10T10:00:00Z",
  "updated_at": "2026-01-10T12:00:00Z",
  "closed_at": null,
  "created_by": "mayor",
  "close_reason": null,
  "estimated": 60,
  "jsonl_content_hash": "abc123..."
}
```

**Status Values** (from Go package):
- `StatusOpen`
- `StatusInProgress`
- `StatusBlocked`
- `StatusDeferred`
- `StatusClosed`

Deleted issues use `status: "tombstone"` with a `deleted_at` timestamp.

**Issue Types** (from Go package):
- `TypeBug`
- `TypeFeature`
- `TypeTask`
- `TypeEpic`
- `TypeChore`

### Dependency Types

The system supports five dependency relationship types:

| Type | Description | Affects Ready Detection |
|------|-------------|------------------------|
| `DepBlocks` | A prevents B until A is resolved | Yes |
| `DepParentChild` | B belongs to epic A (hierarchical) | Yes (propagates blocking) |
| `DepRelated` | Bidirectional soft reference | No |
| `DepDiscoveredFrom` | Source tracking | No |
| `DepConditionalBlocks` | A blocks B unless A failed | Yes (conditionally) |
| `waits-for` | A waits for all of B's children | Yes |

**Critical constraint**: The system enforces **acyclic structure** via recursive CTEs for cycle detection. Cycles are rejected because they break ready work calculation.

---

## 3. Ready Task Detection Algorithm

### The Problem

Computing "which tasks are ready to work on" via recursive queries is expensive. For 10K issues, a naive approach using recursive CTEs took ~752ms per query.

### The Solution: Materialized Cache

Beads uses a `blocked_issues_cache` table that pre-computes which issues have open blockers:

```sql
-- Conceptual structure
CREATE TABLE blocked_issues_cache (
  issue_id TEXT PRIMARY KEY,
  blocked_by TEXT  -- JSON array of blocking issue IDs
);
```

**Ready work query becomes trivial:**
```sql
SELECT * FROM issues
WHERE id NOT IN (SELECT issue_id FROM blocked_issues_cache)
AND status IN ('open', 'in_progress');
```

### Performance Impact

| Metric | Without Cache | With Cache |
|--------|--------------|------------|
| Query time (10K issues) | 752ms | 29ms |
| Improvement | - | **25x faster** |

### Cache Maintenance

The cache rebuilds **completely** (not incrementally) whenever:
- A dependency is added or removed
- An issue status changes (especially closing a blocker)

This ensures consistency while still providing excellent performance.

### Blocking Semantics

An issue is blocked if:
1. **Direct blocking**: Has a `blocks` dependency on an open/in-progress issue
2. **Transitive blocking**: Parent is blocked (propagates through parent-child relationships)

---

## 4. Gastown Agent Roles

Gastown implements a sophisticated multi-agent orchestration system with distinct roles organized at two levels:

### Town-Level Agents (Infrastructure)

| Role | Description | Lifecycle |
|------|-------------|-----------|
| **Mayor** | Chief-of-staff agent; coordinates work distribution across all Rigs; user's primary interface | Persistent |
| **Deacon** | Watchdog daemon running continuous Patrol cycles; ensures workers are active; triggers recovery | Persistent |
| **Dogs** | Maintenance agents for background infrastructure tasks; "Boot" dog checks Deacon every 5 minutes | Brief/helper |

### Rig-Level Agents (Project Work)

| Role | Description | Lifecycle |
|------|-------------|-----------|
| **Crew** | Long-lived, named agents maintaining context across sessions; human-directed; have their own clone | Persistent |
| **Polecats** | Ephemeral worker agents producing Merge Requests in isolated git worktrees; spawned by Witness | Ephemeral |
| **Refinery** | Manages the Merge Queue; intelligently merges changes and handles conflicts | Persistent |
| **Witness** | Monitors Polecats and Refinery progress; detects stuck agents; handles cleanup | Persistent |

### Polecat Lifecycle (Ephemeral Workers)

Polecats exist in exactly three states:

1. **Working** - Actively executing assigned tasks
2. **Stalled** - Session stopped mid-work without resuming
3. **Zombie** - Completed work but failed to exit cleanly

**Three-Layer Architecture:**

| Layer | Component | Lifecycle | Persistence |
|-------|-----------|-----------|-------------|
| Session | Claude instance (tmux) | Ephemeral | Cycles per handoff/crash |
| Sandbox | Git worktree | Persistent | Until nuke |
| Slot | Name from pool | Persistent | Until nuke |

**Self-Cleaning Model:**
- Execute work via molecule steps
- Signal completion with `gt done`
- Exit immediately without idle waiting
- Request self-deletion (nuke)

### Agent Orchestration Patterns

**Convoys** - Persistent tracking units for batched work across repositories:
- Named containers collecting related tasks
- Track issues from multiple rigs simultaneously
- States: Open (active) or Closed/Landed (completed)

**Swarms** - Ephemeral collections of workers actively executing convoy tasks

### Key Principles

**GUPP (Propulsion Principle)**: "If you find something on your hook, YOU RUN IT."
- Agents execute assigned work immediately without waiting for confirmation
- Eliminates polling delays
- Every moment of delay blocks downstream work

**Nondeterministic Idempotence (NDI)**:
- Accepts that the AI's path is chaotic and unpredictable
- Outcome must be idempotent
- If agent dies mid-task, a fresh agent looks at persistent state and finishes
- Focus shifts from "prompt engineering" to "system resilience"

**MEOW**: Breaking large goals into detailed, trackable instructions

---

## 5. Molecules: Work Graphs

Molecules represent the same concept as epics but with explicit execution semantics:

### Lifecycle States

```
Formula (TOML template)
    |
    v
Protomolecule (instantiated template)
    |
    v
Molecule (persistent)  OR  Wisp (ephemeral)
    |
    v
Digest (squashed summary)
```

### Execution Model

- Work = issues with dependencies
- Ready work executes in parallel unless blocked by dependencies
- Children are parallel by default - only add dependencies when order matters

### Key Commands

```bash
bd mol pour <proto>           # Instantiate molecule with variables
bd mol current                # Display progress (done, current, remaining)
bd close <step> --continue    # Close step and auto-advance to next
```

### Wisps vs Molecules

| Aspect | Molecule | Wisp |
|--------|----------|------|
| Persistence | Stored in .beads/ | Never synced |
| Use case | Discrete deliverables needing audit trail | Repetitive operational routines |
| Cleanup | Becomes digest when complete | Hard-deleted |

---

## 6. Daemon Architecture

Beads runs a background daemon per workspace for:
- Auto-sync between SQLite and JSONL
- RPC operations via Unix socket
- Real-time file monitoring

### Event-Driven Mode (v0.21.0+)

- Default: event-driven (replaces 5-second polling)
- <500ms latency for syncing
- ~60% less CPU usage
- Uses platform-native file watchers (inotify/FSEvents)

### Important Limitation

The daemon doesn't work correctly with `git worktree` unless sync-branch is configured, as it cannot track which branch each worktree has checked out.

---

## 7. Limitations and Criticisms

Based on community feedback and experience reports:

### Invasiveness

- Beads integrates extensively into projects
- The `.beads/` directory footprint is substantial
- Community member banteg wrote an uninstall script due to invasiveness
- "Beads was written by AI for AI - humans are not the primary target"

### Performance Concerns

- Some criticism of beads being "slow" or "not written with the taste that top software developers would find acceptable"
- Cost: A 60-minute Gastown session cost ~$100 in Claude tokens (10x typical Claude Code usage)

### Complexity

- Confusing Mad Max-themed metaphors that don't map intuitively
- "At first this all seems like gibberish, and it is"
- Steep learning curve for the conceptual model
- Users report needing to restart Gastown due to breakage

### Autonomy vs Control

- Gastown merged PRs autonomously despite failing integration tests
- System operates faster than users can monitor or manage
- "Riding a wild stallion that needed to be tamed"
- None of four generated PRs in one test session were acceptable

### Work Generation Problem

- System "churns through implementation plans so quickly that you have to do a LOT of design and planning to keep the engine fed"
- Requires deliberate roadmap creation
- Monitoring state of workers across multiple tabs is "too much effort"

### Dual-Persistence Finickiness

- "You have this potentially finicky sync mechanism between SQLite and JSONL"
- Maintaining consistency between two storage systems adds complexity

### Git Worktree Issues

- Daemon doesn't work correctly with git worktrees without special configuration
- This conflicts with Gastown's heavy use of worktrees for polecats

---

## 8. Opportunities for workgraph

Based on this analysis, workgraph could address several limitations:

### 1. Simpler Data Model

- Single source of truth instead of dual-persistence (SQLite + JSONL)
- Avoid the sync complexity between two storage systems
- Consider: Native graph database or simpler file format

### 2. Less Invasive Integration

- Minimize footprint in user projects
- Support external storage of task graph
- Optional integration rather than mandatory hooks

### 3. Clearer Execution Semantics

- More intuitive terminology than Mad Max metaphors
- Explicit state machine for task lifecycle
- Better visibility into what's happening and why

### 4. Better Control Mechanisms

- Human approval gates before autonomous actions (especially merges)
- Configurable autonomy levels
- Better monitoring/observability of agent activities

### 5. Cost Efficiency

- More efficient task decomposition to reduce token usage
- Caching of intermediate results
- Smarter batching of agent work

### 6. Improved Ready Detection

- Beads' materialized cache approach is sound
- Consider incremental cache updates instead of full rebuilds
- Support for more complex scheduling constraints (not just blocking)

### 7. Alternative Dependency Types

Beads supports:
- blocks
- parent-child
- related
- discovered-from
- conditional-blocks
- waits-for

Consider additional types:
- Soft dependencies (prefer but don't require)
- Time-based constraints
- Resource constraints (only N tasks of type X at once)
- Data flow dependencies (output of A feeds input of B)

### 8. Native Multi-Repository Support

- First-class support without the complexity of Gastown's rig/convoy model
- Cross-repository dependency tracking
- Unified view of work across projects

### 9. Simpler Agent Model

- Less role proliferation (Mayor, Deacon, Dogs, Polecats, Crew, Witness, Refinery)
- Clearer responsibilities
- Easier to understand and debug

### 10. Graceful Degradation

- Work effectively with single agent or multiple
- Scale up/down smoothly
- Don't require full infrastructure for simple use cases

---

## 9. Key Takeaways

### What Beads Gets Right

1. **Git as database** - Version control for task state is valuable
2. **Hash-based IDs** - Prevents merge conflicts in distributed workflows
3. **Materialized cache for ready detection** - Significant performance improvement
4. **Structured memory for agents** - Solving real context loss problem
5. **Dependency DAG enforcement** - Prevents cycles that break scheduling

### What Gastown Gets Right

1. **Persistent work state** - Survives agent crashes and restarts
2. **Clear role separation** - Different agents for different responsibilities
3. **Nondeterministic Idempotence** - Designing for failure and recovery
4. **Propulsion principle** - Keep work flowing without blocking on confirmation

### What Could Be Improved

1. **Simpler storage model** - Dual-persistence adds complexity
2. **Less invasive** - Don't require extensive project modification
3. **Better human oversight** - More control over autonomous actions
4. **Clearer conceptual model** - Terminology that maps to intuition
5. **Cost efficiency** - Token usage is very high
6. **Stability** - System breaks and requires restarts
7. **Scalable monitoring** - Can't effectively watch many agents at once

---

## Sources

- [Beads GitHub Repository](https://github.com/steveyegge/beads)
- [Gastown GitHub Repository](https://github.com/steveyegge/gastown)
- [Beads Go Package Documentation](https://pkg.go.dev/github.com/steveyegge/beads)
- [DeepWiki Beads Documentation](https://deepwiki.com/steveyegge/beads)
- [A Day in Gas Town - DoltHub Blog](https://www.dolthub.com/blog/2026-01-15-a-day-in-gas-town/)
- [Wrapping my head around Gas Town - Justin Abrahms](https://justin.abrah.ms/blog/2026-01-05-wrapping-my-head-around-gas-town.html)
- [beads and the future of programming - Edgar Tools](https://www.edgartools.io/beads-and-the-future-of-programming/)
