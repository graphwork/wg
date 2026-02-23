# Cycle Topology Analysis: Automatic Detection and Agent Awareness

Research report for automatic cycle topology analysis in the workgraph coordinator service.

## 1. Current State of Cycle Detection and Annotation

### What exists today

**Cycle detection** is fully implemented via Tarjan's SCC algorithm in `src/cycle.rs`. The `CycleAnalysis` struct (`src/graph.rs:486`) provides:
- `cycles: Vec<DetectedCycle>` — SCCs with header, members, reducibility
- `task_to_cycle: HashMap<String, usize>` — task-to-cycle index mapping
- `back_edges: HashSet<(String, String)>` — structural back-edges

**Cycle iteration** is handled by `evaluate_cycle_iteration` (`src/graph.rs:738`), which supports two modes:
1. **SCC-detected cycles** — tasks connected by explicit back-edges in the graph
2. **Implicit cycles** — tasks with `cycle_config` but no SCC back-edges (recent fix from `fix-cycle-not`)

**Agent awareness** is partially implemented. The `TemplateVars` struct (`src/service/executor.rs:50-77`) injects a `task_loop_info` section into agent prompts, but only when:
- The task has `cycle_config` (header) — shows iteration N/max, explains `--converged`
- The task has `loop_iteration > 0` (non-header member after re-activation) — shows iteration number

### Gaps

1. **First-iteration members get no cycle info.** When a cycle starts (iteration 0), non-header tasks have `loop_iteration == 0` and no `cycle_config`, so `task_loop_info` is empty. The agent has no idea it's in a cycle.

2. **No automatic back-edge creation.** `wg add B --after A --max-iterations 3` creates a one-way edge A→B. No back-edge B→A is added. The implicit cycle handler in `evaluate_cycle_iteration` works around this, but `wg cycles` won't show these as cycles and `ready_tasks_cycle_aware` won't apply back-edge exemptions for them.

3. **No annotation on `wg add`.** When a user creates A→B→A (explicit back-edge), nothing annotates the edge or warns the user. The cycle only becomes visible via `wg cycles`.

4. **Cycle analysis is computed fresh on every coordinator tick.** `compute_cycle_analysis()` is called at multiple points in `src/commands/service.rs` (lines 383, 419, 911, 1027, 1169) — each a full O(V+E) computation from scratch. There's a cached `cycle_analysis: Option<CycleAnalysis>` field on `WorkGraph` (line 558) but it's invalidated on every mutation (add_node, update_task, etc.).

## 2. Proposed Automatic Analysis Triggers

### Option A: File-watch (inotify on graph.json)

**Mechanism:** The coordinator daemon watches `graph.json` for filesystem changes.

**Pros:**
- Catches manual edits to graph.json
- Decoupled from specific commands
- Already has precedent — `poll_interval` is the background safety-net

**Cons:**
- Redundant with IPC. The coordinator already receives `GraphChanged` IPC messages from every `wg` command that mutates the graph (`wg add`, `wg done`, `wg edit`, etc. — see `notify_graph_changed` calls in `src/commands/mod.rs:153`).
- Race conditions with concurrent writers
- Platform-specific (inotify is Linux-only; would need kqueue on macOS)
- Adds complexity for minimal benefit since IPC already covers the fast path

**Verdict:** Not recommended as primary trigger.

### Option B: On-add hook (in `wg add`)

**Mechanism:** Run cycle analysis in `wg add` after adding the task. If the new task creates or participates in a cycle, annotate the edge and/or task metadata.

**Pros:**
- Immediate feedback to the user ("Warning: this creates a cycle with tasks X, Y, Z")
- Can auto-annotate back-edges at creation time
- No coordinator dependency — works even without the service running
- Natural place to add `cycle_member: true` tags or similar metadata

**Cons:**
- Adds O(V+E) computation to every `wg add` call (but graphs are typically small)
- Only catches cycles created via `wg add`, not manual graph edits or `wg edit`

**Verdict:** Recommended as the primary annotation point.

### Option C: Coordinator tick

**Mechanism:** The coordinator already recomputes `CycleAnalysis` on each tick. Extend this to annotate tasks that are in cycles but lack awareness metadata.

**Pros:**
- Already happens — no new trigger needed
- Catches all cycle formation paths (add, edit, manual graph changes)
- Can update task descriptions or metadata in-place before spawning agents

**Cons:**
- Coordinator must be running for annotations to happen
- Modifying task descriptions in the coordinator is a side-effect that could surprise users
- Multiple `compute_cycle_analysis()` calls per tick are redundant (could cache within a tick)

**Verdict:** Recommended as a secondary safety net, but should not be the primary annotation path.

### Recommended trigger strategy

**Hybrid: on-add (primary) + coordinator tick (safety net)**

1. `wg add` detects if the new task creates or joins a cycle, annotates edges/tasks immediately
2. The coordinator tick verifies cycle metadata is consistent before spawning agents, filling in any gaps

## 3. Agent Awareness Design

### Problem

Agents need to know three things:
1. **Am I in a cycle?** — determines whether `--converged` is relevant
2. **What iteration am I on?** — context for the agent's work
3. **What's the convergence criterion?** — when to use `--converged` vs plain `wg done`

Currently, only (2) is partially communicated via `task_loop_info`, and only for headers or after-first-iteration members.

### Option A: Inject into task description

**Mechanism:** When `wg add` detects a cycle, append cycle metadata to the task description.

**Pros:** Always visible, persists across coordinator restarts
**Cons:** Pollutes the user-authored description; hard to update on re-iterations

**Verdict:** Too intrusive. Descriptions should remain user-controlled.

### Option B: Enrich `task_loop_info` in prompt template

**Mechanism:** Expand the existing `task_loop_info` injection in `TemplateVars` to cover all cycle members, not just headers and post-iteration-0 members.

Currently (`src/service/executor.rs:50-77`):
```
if task.cycle_config.is_some() → "You are a cycle header (iteration N, max M)"
else if task.loop_iteration > 0 → "You are in cycle iteration N"
else → "" (no info)
```

Proposed expansion:
```
if task.cycle_config.is_some() → [header info, as today]
else if cycle_analysis shows task is in a cycle → "You are a cycle member (iteration N).
  The cycle header is <header-id> with max <M> iterations.
  Use `wg done <id> --converged` when your contribution is complete."
else if task has a cycle_member tag → [same as above, using tag metadata]
else → "" (no info)
```

**Pros:**
- Uses existing injection mechanism
- No changes to stored task data
- Coordinator has full CycleAnalysis available at spawn time

**Cons:**
- Requires the coordinator to be running (but agents are only spawned by the coordinator anyway)
- Needs access to cycle analysis at template resolution time

**Verdict:** Recommended. This is the natural place and requires minimal changes.

### Option C: Add "iteration N of M" header to prompt

**Mechanism:** A structured header block like:
```
## Cycle Status
- Cycle: write → review → write
- Role: member (not header)
- Iteration: 2 of 5
- Header task: review (controls iteration)
- Convergence: use `wg done <id> --converged` when satisfied
```

**Pros:** Clear, structured, easy for agents to parse
**Cons:** More verbose than current approach

**Verdict:** This is an enhancement of Option B, not a separate option. The `task_loop_info` section should be upgraded to this format.

### Option D: `wg context` output enrichment

**Mechanism:** When an agent runs `wg show <task>` or `wg context <task>`, include cycle membership info in the output.

**Pros:** Available on-demand; doesn't bloat prompts
**Cons:** Agent must know to ask; first-iteration agents don't know they're in a cycle

**Verdict:** Good supplement, but insufficient on its own.

### Recommended approach

**Primary: Enhanced `task_loop_info` (Options B+C)**

Upgrade the prompt injection to include:
- Cycle path visualization
- Current iteration / max iterations
- Whether this task is the header or a member
- Explicit `--converged` instructions for ALL cycle members (not just headers)
- Convergence criteria from the cycle_config guard, if any

**Secondary: Enrich `wg show` output with cycle info**

## 4. Convergence Protocol Integration

### Current state

The convergence protocol works as follows:
1. An agent decides its work is done and the cycle should stop iterating
2. It calls `wg done <task-id> --converged`
3. This adds a `"converged"` tag to the task
4. `reactivate_cycle` (`src/graph.rs:801-805`) checks for this tag on the config owner and stops iteration if present

### Gap: Only the header can converge

The `"converged"` tag check is only on the `config_owner_id` (the task with `cycle_config`). If a non-header member calls `wg done X --converged`, the tag is added to X but never checked — only the header's tag matters.

This is actually correct behavior: the header is the evaluator/decision-maker that determines convergence. But agents don't know this. A non-header agent might call `--converged` thinking it will stop the loop, when in fact only the header's `--converged` matters.

### Recommendations

1. **Clarify in prompts:** Tell non-header members that `--converged` on their task is informational — the header controls iteration. This prevents confusion.

2. **Surface guard conditions:** If the cycle has a guard (e.g., `TaskStatus { task: "sentinel", status: "failed" }`), include this in the agent prompt so it understands what drives iteration.

3. **Propagate convergence signal:** Consider having the coordinator inject a `previous_iteration_outputs` context for iteration > 0. This lets the header agent compare outputs across iterations to decide convergence.

4. **No changes to core protocol:** The current design where only the header's `--converged` tag matters is sound. The fix is in communication, not mechanism.

## 5. Implementation Complexity Estimate

### Change 1: Enhanced `task_loop_info` in executor.rs
**Complexity: Low (1-2 hours)**
- Modify `TemplateVars::new()` to accept `&CycleAnalysis`
- Look up the task in `cycle_analysis.task_to_cycle` to determine membership
- Generate richer loop info including cycle path, role, and convergence instructions
- Currently `TemplateVars::new()` doesn't receive cycle analysis — need to thread it through

### Change 2: Cycle annotation in `wg add`
**Complexity: Medium (2-4 hours)**
- After adding a task, run `compute_cycle_analysis()` on the updated graph
- If the task is in a new cycle: print a warning/info message
- Optionally: add a `cycle_member` tag to participating tasks
- Optionally: auto-create back-edges for `--max-iterations` patterns (but the implicit cycle fix may make this unnecessary)

### Change 3: Cache cycle analysis within coordinator tick
**Complexity: Low (1 hour)**
- Compute `CycleAnalysis` once per tick, pass it to all functions that need it
- Currently computed 3-5 times per tick at different callsites in `service.rs`

### Change 4: Enrich `wg show` with cycle info
**Complexity: Low (1 hour)**
- Compute cycle analysis, check if task is in a cycle
- Display cycle membership, header, iteration info

### Total estimate: 5-8 hours of implementation work

### Algorithm complexity

- `CycleAnalysis::from_graph()` is O(V+E) via Tarjan's SCC — this is optimal
- Incremental updates (re-running on each `wg add`) are not significantly cheaper than full recomputation for typical workgraph sizes (tens to hundreds of tasks)
- No need for incremental cycle detection — full recomputation is fast enough

## 6. Recommended Approach

### Phase 1: Agent awareness (highest impact, lowest effort)

**Goal:** Every agent in a cycle knows it's in a cycle, what iteration it's on, and how convergence works.

1. Thread `CycleAnalysis` into `TemplateVars::new()` in `src/service/executor.rs`
2. Expand the `task_loop_info` block to cover all cycle members (not just headers and post-iteration-0):
   ```
   ## Cycle Information

   This task is part of a cycle: write → review → write
   - Your role: member | header
   - Current iteration: 0 (first run) | N of M
   - Cycle header: <header-id> (controls iteration)

   When your work is complete, use:
     wg done <task-id> --converged

   The cycle will iterate until the header task uses --converged
   or max iterations (M) is reached.
   ```
3. For implicit cycles (no SCC back-edge), use the same logic as `evaluate_cycle_iteration`'s Mode 2 to determine membership

### Phase 2: On-add detection and annotation

**Goal:** Users get immediate feedback when creating cycles.

1. After `wg add`, compute cycle analysis
2. If the new task is in a cycle, print: `Note: task '<id>' is part of a cycle with <members>`
3. If `--max-iterations` is set without a detected SCC, print: `Note: implicit cycle detected — <id> and its dependencies will iterate`
4. Optionally add a `cycle` tag to cycle members for easy filtering

### Phase 3: Coordinator optimization

**Goal:** Reduce redundant computation.

1. Compute `CycleAnalysis` once per coordinator tick, store in a local variable
2. Pass to all functions: `check_ready_or_return`, `build_auto_assign_tasks`, `triage_dead_agents`, and `coordinator_tick`
3. This is a pure refactor — no behavior change, just efficiency

### What NOT to do

- **Don't add file watching.** The IPC `GraphChanged` mechanism already provides fast-path notification. The `poll_interval` background tick is the safety net. Adding inotify would be redundant complexity.
- **Don't auto-create back-edges in `wg add`.** The `fix-cycle-not` implicit cycle handling makes this unnecessary and back-edges in `wg add` were shown to break first-iteration execution order (see fix-cycle-not logs).
- **Don't modify task descriptions.** Descriptions are user-authored content. Cycle metadata belongs in the prompt template, not in stored task data.
- **Don't require `--max-iterations` for cycle detection.** `wg cycles` already detects structural SCCs regardless of configuration. The annotation system should surface these to agents whether or not `--max-iterations` is set.

### Open question: Should cycles without `--max-iterations` be annotated?

Currently, if a user creates A→B→A without `--max-iterations`, the tasks will deadlock (each blocks the other, neither can start). The `ready_tasks_cycle_aware` function only exempts back-edge blockers for tasks with `cycle_config`.

**Recommendation:** Yes, detect and warn on `wg add`. A cycle without `--max-iterations` is almost certainly a mistake. The warning should suggest: `"Cycle detected but no --max-iterations set. Tasks will deadlock. Add --max-iterations to enable iteration."`
