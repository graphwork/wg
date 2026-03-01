# Workgraph Analysis: Practical Perspectives on Task Coordination Graphs

This document explores what "analysis" and "verification" should mean for a task coordination system like Workgraph. The focus is on practical, useful analysis that engineers and project managers actually need, rather than academic formal verification.

## Table of Contents

1. [Context: What Workgraph Is](#context-what-workgraph-is)
2. [Structural Analysis](#1-structural-analysis)
3. [Health Metrics](#2-health-metrics)
4. [Temporal Analysis](#3-temporal-analysis)
5. [Dependency Analysis](#4-dependency-analysis)
6. [Proposed Commands](#5-proposed-commands)
7. [Implementation Priorities](#6-implementation-priorities)

---

## Context: What Workgraph Is

Workgraph is a task coordination CLI with:

- **Tasks**: Work items with dependencies (`blocked_by`), estimates (hours/cost), status (open/in-progress/done/blocked), and assignments
- **Actors**: Humans and AI agents who can be assigned to tasks
- **Resources**: Budgets, compute capacity, and other constraints
- **Cycles are allowed**: Unlike traditional acyclic task managers, Workgraph allows cycles for recurring tasks

The current implementation already provides:
- `wg check` - Detects cycles (as warnings) and orphan references (as errors)
- `wg ready` - Lists tasks with all blockers done
- `wg blocked <id>` - Shows what's blocking a specific task
- `wg cost <id>` - Calculates total cost including transitive dependencies
- `wg plan` - Shows project summary and budget/hours-constrained planning
- `wg graph` - Outputs DOT format for visualization

---

## 1. Structural Analysis

### 1.1 Cycle Detection and Classification

**Current State**: `wg check` detects cycles and reports them as warnings.

**Enhancement Opportunity**: Not all cycles are equal. Some are intentional (recurring tasks), others are bugs.

#### Cycle Classification

| Cycle Type | Detection | Recommended Action |
|------------|-----------|-------------------|
| **Intentional recurrence** | Task has `recurring: true` or `cycle: intentional` tag | Report as info, not warning |
| **Short cycles (2 nodes)** | A blocks B, B blocks A | Likely a bug - flag as error |
| **Diamond dependencies** | A blocks C, B blocks C, C blocks A | Complex - needs manual review |
| **Long cycles (5+ nodes)** | Multi-step circular dependency | Likely unintentional - flag as warning |

**Proposed**: `wg loops` command to show all cycles with classification:

```
$ wg loops

Cycles detected: 3

1. INTENTIONAL RECURRENCE (2 nodes)
   daily-standup -> daily-standup-followup -> daily-standup

2. WARNING: Potential bug (2 nodes)
   api-design -> api-impl -> api-design
   Reason: Short cycle without recurrence tag

3. INFO: Complex dependency (4 nodes)
   feature-a -> test-a -> deploy-a -> validate-a -> feature-a
```

### 1.2 Critical Path Analysis

The **critical path** is the longest chain of dependent tasks that determines the minimum project duration.

**Why it matters:**
- Tasks on the critical path cannot be delayed without delaying the project
- Identifies where to focus resources for faster completion
- Helps with realistic deadline estimation

**Algorithm:**
1. Topologically sort tasks (treating cycles specially)
2. Calculate earliest start time for each task
3. Backward pass: calculate latest start time without delaying project
4. Tasks where earliest == latest are on critical path

**Proposed**: `wg critical-path` command:

```
$ wg critical-path

Critical path (5 tasks, estimated 47 hours):

1. [ready]   api-design (8h)
2. [blocked] api-impl (16h) <- blocked by api-design
3. [blocked] api-tests (8h) <- blocked by api-impl
4. [blocked] integration (12h) <- blocked by api-tests
5. [blocked] deploy (3h) <- blocked by integration

Slack analysis:
  frontend-work: 24h slack (can delay without affecting deadline)
  docs: 32h slack
```

### 1.3 Disconnected Subgraphs

A **disconnected subgraph** is a set of tasks with no dependency relationship to the rest of the graph.

**Why it matters:**
- Independent workstreams can be parallelized
- Orphan tasks may be forgotten or obsolete
- Can identify natural team boundaries

**Detection:**
```rust
fn find_connected_components(graph: &WorkGraph) -> Vec<Vec<&Task>> {
    // Union-find or BFS from each unvisited node
}
```

**Proposed**: `wg islands` command:

```
$ wg islands

Found 3 independent workstreams:

1. Main feature (12 tasks)
   Root: api-design
   Leaves: deploy, docs

2. Infrastructure (4 tasks)
   Root: setup-ci
   Leaves: monitoring-dashboard

3. Orphan tasks (2 tasks) [WARNING: No connections]
   - legacy-cleanup
   - investigate-bug-123
```

### 1.4 Dead Ends and Entry Points

| Concept | Definition | Significance |
|---------|------------|--------------|
| **Entry point** | Task that nothing depends on (no `blocks` pointing to it) | Starting points for work |
| **Dead end** | Task that blocks nothing else | Final deliverables or forgotten tasks |
| **Leaf task** | Task with no blockers | Ready to start immediately |
| **Root task** | Task that many others transitively depend on | High-impact bottleneck |

**Proposed**: `wg structure` command:

```
$ wg structure

Entry points (5 tasks):
  api-design, frontend-setup, ci-setup, docs-plan, research

Dead ends (3 tasks):
  deploy (expected - final deliverable)
  old-prototype (WARNING: no dependents, status=open)
  meeting-notes (expected - documentation)

High-impact roots (blocking 5+ tasks transitively):
  api-design: 8 tasks depend on this
  database-schema: 6 tasks depend on this
```

---

## 2. Health Metrics

### 2.1 Task Distribution by Status

**Current State**: `wg plan` shows basic counts.

**Enhancement**: More detailed breakdown with trends.

```
$ wg health

Task Status Distribution:
  Open:        15 (50%)  [||||||||||||||||          ]
  In Progress:  3 (10%)  [||||                      ]
  Done:        10 (33%)  [||||||||||||              ]
  Blocked:      2 (7%)   [||                        ]

Ready Queue: 4 tasks ready to start
  - api-design (8h, $800)
  - frontend-setup (4h, $400)
  - docs-plan (2h, $200)
  - ci-setup (4h, $400)

Stalled Tasks (in-progress > 7 days):
  - database-migration (in-progress for 12 days)
```

### 2.2 Bottleneck Analysis

A **bottleneck** is a task that blocks many other tasks, directly or transitively.

**Metrics:**
- **Direct blockers**: How many tasks list this in `blocked_by`
- **Transitive blockers**: How many tasks ultimately depend on this
- **Blocking depth**: How deep is the dependency chain from this task

```
$ wg bottlenecks

Top bottlenecks by transitive impact:

1. api-design
   Directly blocks: 2 tasks
   Transitively blocks: 8 tasks
   Status: OPEN (not started!)
   RECOMMENDATION: High priority - blocking 27% of project

2. database-schema
   Directly blocks: 3 tasks
   Transitively blocks: 6 tasks
   Status: in-progress
   Assigned: @alice

3. auth-service
   Directly blocks: 2 tasks
   Transitively blocks: 4 tasks
   Status: done (no longer blocking)
```

### 2.3 Resource Utilization

**Current State**: Resources can be defined with `available` capacity.

**Enhancement**: Track actual vs. planned utilization.

```
$ wg resources

Resource Utilization:

  engineering-budget ($50,000 available)
    Committed (open tasks): $32,000 (64%)
    Spent (done tasks): $18,000
    Remaining: $18,000

  api-compute (100 units available)
    Peak concurrent usage: 45 units
    Current usage: 20 units
    No over-allocation detected

  ALERT: design-budget ($5,000 available)
    Committed: $6,500 (130% - OVER BUDGET)
    Tasks at risk: logo-redesign, ui-mockups
```

### 2.4 Actor Workload Balance

Track assignment distribution and workload.

```
$ wg workload

Actor Workload (open + in-progress tasks):

  @alice
    Assigned: 5 tasks (23h estimated)
    In progress: 2 tasks
    Capacity: 40h/week
    Load: 58%

  @bob
    Assigned: 8 tasks (42h estimated)
    In progress: 1 task
    Capacity: 40h/week
    Load: 105% [WARNING: overloaded]

  @claude-agent
    Assigned: 3 tasks (12h estimated)
    Capacity: unlimited
    Load: N/A (agent)

Unassigned tasks: 12
  Ready & unassigned: 4 (potential parallelization)
```

---

## 3. Temporal Analysis

### 3.1 Task Age Analysis

Track how long tasks have been open.

```
$ wg aging

Task Age Distribution:

  < 1 day:     5 tasks  [|||||                     ]
  1-7 days:    8 tasks  [||||||||                  ]
  1-4 weeks:  10 tasks  [||||||||||                ]
  1-3 months:  4 tasks  [||||                      ] [WARNING]
  > 3 months:  3 tasks  [|||                       ] [CRITICAL]

Oldest open tasks:
  1. legacy-refactor (127 days) - @bob - blocked by: nothing
  2. perf-optimization (89 days) - unassigned - blocked by: nothing
  3. docs-overhaul (64 days) - @alice - blocked by: api-design

Stale in-progress tasks (started > 14 days ago):
  - database-migration (started 18 days ago) - @alice
```

### 3.2 Velocity Tracking

Measure tasks completed over time.

**Requirements**: Need to track completion timestamps (currently not stored).

```
$ wg velocity

Completion Velocity (last 30 days):

  Week 1: 8 tasks (32h)  [||||||||                  ]
  Week 2: 12 tasks (41h) [||||||||||||              ]
  Week 3: 5 tasks (18h)  [|||||                     ] [below average]
  Week 4: 9 tasks (28h)  [|||||||||                 ]

  Average: 8.5 tasks/week (30h/week)
  Trend: stable

At current velocity:
  Open tasks (15): ~2 weeks to clear
  Blocked tasks will unblock as: api-design done (estimated)
```

### 3.3 Projected Completion

Based on estimates and velocity.

```
$ wg forecast

Project Completion Forecast:

Remaining work:
  Open tasks: 15 (47h estimated)
  Blocked tasks: 2 (8h estimated)
  In progress: 3 (12h estimated)
  Total: 67h

Scenarios:

  Optimistic (all estimates accurate):
    Completion: Feb 15, 2025
    Critical path: api-design -> api-impl -> deploy

  Realistic (+30% buffer):
    Completion: Feb 22, 2025

  Pessimistic (+50% buffer):
    Completion: Mar 1, 2025

Blockers that could delay:
  - api-design (8h remaining, blocks 8 tasks)
  - database-schema (in-progress, blocks 6 tasks)
```

---

## 4. Dependency Analysis

### 4.1 Transitive Dependencies (Why is X blocked?)

**Current State**: `wg blocked <id>` shows direct blockers.

**Enhancement**: Show the full chain to understand root cause.

```
$ wg why-blocked deploy

Task: deploy
Status: blocked (transitively)

Blocking chain:

deploy
 └── blocked by: integration (status: blocked)
      └── blocked by: api-tests (status: blocked)
           └── blocked by: api-impl (status: blocked)
                └── blocked by: api-design (status: OPEN) <-- ROOT CAUSE

Root blockers (actionable now):
  - api-design: OPEN, unassigned, ready to start

Summary: deploy is blocked by 4 tasks; unblock api-design to make progress.
```

### 4.2 Impact Analysis (What does X affect?)

Inverse of why-blocked: if X is delayed, what suffers?

```
$ wg impact api-design

Task: api-design
Status: open
Estimated: 8h

Direct dependents (2):
  - api-impl (16h)
  - api-docs (4h)

Transitive dependents (8):
  - api-impl -> api-tests -> integration -> deploy
  - api-impl -> api-tests -> integration -> monitoring
  - api-impl -> api-tests -> performance-tests
  - api-docs -> user-guide

Impact summary:
  If api-design is delayed by 1 day:
    - 8 tasks delayed
    - Project completion delayed by 1 day (on critical path)
    - Total hours at risk: 47h
```

### 4.3 Shortest Path Between Tasks

Answer: "What's the relationship between X and Y?"

```
$ wg path api-design monitoring

Shortest dependency path (4 steps):

api-design
 -> api-impl (blocked_by)
     -> api-tests (blocked_by)
         -> integration (blocked_by)
             -> monitoring (blocked_by)

Path length: 4
Estimated time: api-design + api-impl + api-tests + integration = 44h
```

### 4.4 Dependency Graph Statistics

Overall graph metrics.

```
$ wg graph-stats

Graph Statistics:

Nodes:
  Tasks: 30
  Actors: 4
  Resources: 3

Edges:
  blocked_by: 45
  assigned: 18
  requires: 12

Graph metrics:
  Density: 0.12 (sparse - good)
  Average dependencies per task: 1.5
  Maximum dependencies: 5 (task: integration)
  Maximum dependents: 8 (task: api-design)

Cycles: 1 (marked as intentional)
Connected components: 2
Longest dependency chain: 5 tasks
```

---

## 5. Proposed Commands

### 5.1 Summary of New Commands

| Command | Purpose | Complexity |
|---------|---------|------------|
| `wg analyze` | Overall health report (combines several checks) | Medium |
| `wg why-blocked <id>` | Full dependency chain explaining why task is blocked | Low |
| `wg impact <id>` | What depends on this task (forward dependencies) | Low |
| `wg loops` | Show all cycles with classification | Low |
| `wg critical-path` | Identify the longest dependency chain | Medium |
| `wg bottlenecks` | Find tasks blocking the most work | Low |
| `wg islands` | Find disconnected subgraphs | Low |
| `wg structure` | Show entry points, dead ends, roots | Low |
| `wg aging` | Task age distribution | Medium (needs timestamps) |
| `wg velocity` | Completion rate over time | Medium (needs timestamps) |
| `wg forecast` | Project completion estimation | Medium |
| `wg workload` | Actor assignment balance | Low |
| `wg path <a> <b>` | Shortest path between two tasks | Low |
| `wg graph-stats` | Overall graph metrics | Low |

### 5.2 The `wg analyze` Command

A comprehensive health report combining key metrics.

```
$ wg analyze

=== Workgraph Health Report ===

SUMMARY
  Total tasks: 30 (15 open, 3 in-progress, 10 done, 2 blocked)
  Ready to start: 4 tasks
  Estimated remaining: 67h / $6,700

STRUCTURAL HEALTH
  [OK] No orphan references
  [OK] 1 cycle detected (marked as intentional)
  [OK] All tasks reachable
  [WARNING] 2 dead-end tasks with status=open (may be forgotten)

BOTTLENECKS
  [CRITICAL] api-design: blocks 8 tasks, status=open, unassigned
  [WARNING] database-schema: blocks 6 tasks, in-progress for 12 days

WORKLOAD
  [OK] 3/4 actors have balanced workload
  [WARNING] @bob at 105% capacity

AGING
  [WARNING] 3 tasks open > 3 months
  [WARNING] 1 task in-progress > 14 days

RECOMMENDATIONS
  1. Assign and start api-design (critical bottleneck)
  2. Check on database-migration (stalled)
  3. Review legacy-refactor (open 127 days)
  4. Redistribute 2 tasks from @bob
```

### 5.3 JSON Output for Scripting

All commands should support `--json` for integration with other tools.

```
$ wg analyze --json

{
  "summary": {
    "total_tasks": 30,
    "open": 15,
    "in_progress": 3,
    "done": 10,
    "blocked": 2,
    "ready": 4,
    "estimated_hours": 67,
    "estimated_cost": 6700
  },
  "structural": {
    "orphan_refs": [],
    "cycles": [{"nodes": ["daily-standup", "daily-followup"], "intentional": true}],
    "dead_ends": ["old-prototype", "meeting-notes"],
    "unreachable": []
  },
  "bottlenecks": [
    {"id": "api-design", "transitive_dependents": 8, "status": "open", "severity": "critical"}
  ],
  "issues": [
    {"type": "stale_task", "task": "legacy-refactor", "days_open": 127},
    {"type": "overloaded_actor", "actor": "bob", "load_percent": 105}
  ],
  "recommendations": [
    {"priority": 1, "action": "assign", "task": "api-design", "reason": "critical bottleneck"}
  ]
}
```

---

## 6. Implementation Priorities

### 6.1 Phase 1: Low-Hanging Fruit (Immediate Value)

These require minimal changes to the data model:

1. **`wg why-blocked <id>`** - Full transitive blocker chain
   - Already have `blocked_by` traversal
   - Just need to recurse and format output

2. **`wg impact <id>`** - Forward dependency analysis
   - Inverse of existing `cost_of` traversal
   - Build "blocks" index from `blocked_by` edges

3. **`wg bottlenecks`** - High-impact tasks
   - Run impact analysis for all tasks
   - Sort by transitive dependent count

4. **`wg structure`** - Entry points, dead ends
   - Graph traversal already implemented
   - Just need new output formatting

5. **`wg loops --classify`** - Enhanced cycle reporting
   - Already have cycle detection
   - Add classification logic

### 6.2 Phase 2: Temporal Features (Requires Timestamps)

These need additions to the data model:

1. **Add timestamps to tasks:**
   ```rust
   pub struct Task {
       // ...existing fields...
       pub created_at: Option<String>,      // ISO 8601
       pub started_at: Option<String>,      // When status changed to in-progress
       pub completed_at: Option<String>,    // When status changed to done
   }
   ```

2. **`wg aging`** - Task age analysis
3. **`wg velocity`** - Completion rate tracking
4. **`wg forecast`** - Project completion estimation

### 6.3 Phase 3: Advanced Analysis

1. **`wg critical-path`** - Requires topological sort with cycle handling
2. **`wg workload`** - Requires capacity field on actors
3. **`wg resources`** - Requires tracking resource consumption per task
4. **`wg analyze`** - Comprehensive report combining all of the above

### 6.4 Data Model Additions

To support all proposed features:

```rust
pub struct Task {
    // Existing fields...
    pub id: String,
    pub title: String,
    pub status: Status,
    pub assigned: Option<String>,
    pub estimate: Option<Estimate>,
    pub blocked_by: Vec<String>,
    pub blocks: Vec<String>,
    pub requires: Vec<String>,
    pub tags: Vec<String>,
    pub not_before: Option<String>,

    // New fields for temporal analysis
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,

    // New fields for cycle handling
    pub recurring: Option<bool>,           // Intentional cycle
    pub recurrence_interval: Option<String>, // "daily", "weekly", etc.
}

pub struct Actor {
    // Existing fields...
    pub id: String,
    pub name: Option<String>,
    pub role: Option<String>,
    pub rate: Option<f64>,
    pub capacity: Option<f64>,

    // New fields
    pub hours_per_week: Option<f64>,       // For workload calculation
}
```

---

## 7. Summary: What Would Be Most Useful?

Based on typical project management needs, here's a prioritized list:

### High Value, Low Effort
1. **`wg why-blocked`** - Everyone needs to understand why things are stuck
2. **`wg impact`** - Critical for prioritization decisions
3. **`wg bottlenecks`** - Identifies where to focus effort
4. **`wg analyze`** (basic) - One-command health check

### High Value, Medium Effort
5. **`wg critical-path`** - Essential for deadline estimation
6. **`wg forecast`** - Answers "when will we be done?"
7. **`wg workload`** - Prevents burnout and identifies capacity

### Medium Value, Low Effort
8. **`wg structure`** - Useful for understanding large graphs
9. **`wg loops --classify`** - Helps distinguish bugs from features
10. **`wg path`** - Occasionally useful for understanding relationships

### Nice to Have
11. **`wg aging`** - Helps identify forgotten work
12. **`wg velocity`** - Useful for retrospectives
13. **`wg islands`** - Useful for very large projects

---

## Appendix: Comparison with Formal Verification

The existing research documents cover Petri nets and CSP for formal verification. Here's how the practical analysis approach compares:

| Aspect | Formal Verification | Practical Analysis |
|--------|--------------------|--------------------|
| **Goal** | Prove properties mathematically | Identify actionable issues |
| **Completeness** | Exhaustive (all possible states) | Heuristic (common patterns) |
| **Complexity** | Exponential worst case | Linear to polynomial |
| **Output** | Pass/fail with counterexample | Warnings with recommendations |
| **User** | Verification engineer | Project manager/developer |
| **When to use** | Safety-critical workflows | Day-to-day project management |

**Recommendation**: Start with practical analysis (this document). Add formal verification as an optional `wg verify --exhaustive` command for users who need mathematical guarantees.

---

## References

- Existing workgraph implementation: `src/check.rs`, `src/query.rs`
- Petri nets research: `docs/petri-nets-research.md`
- CSP research: `docs/csp-process-algebra-research.md`
- Task format research: `docs/task-format-research.md`
