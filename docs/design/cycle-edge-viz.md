# Design: Cycle/Loop Edge Visualization in TUI

## Problem

When a selected task participates in one or more cycles, the user has no visual
indication of which edges form the loop. Upstream edges are magenta, downstream
edges are cyan, but cycle edges — the most structurally significant — are
invisible among them.

## Goal

Color cycle edges **yellow** so the user can immediately see the loop structure
when navigating to a task that participates in a cycle.

---

## 1. How Cycle Membership Is Determined for the Selected Task

### Existing infrastructure

| Component | Location | What it provides |
|---|---|---|
| `tarjan_scc` | `src/cycle.rs:77` | All SCCs in the graph |
| `CycleAnalysis` | `src/graph.rs:750` | `cycles: Vec<DetectedCycle>`, `task_to_cycle: HashMap<String, usize>`, `back_edges: HashSet<(String, String)>` |
| `DetectedCycle` | `src/graph.rs:738` | `members: Vec<String>`, `header: String`, `reducible: bool` |
| `compute_cycle_analysis()` | `src/graph.rs:959` | Already called in `ascii.rs:49` for layout purposes |

### Algorithm

When the selected task changes (`recompute_trace()` in `state.rs:375`):

1. Look up `task_to_cycle[selected_id]` → get cycle index (or `None` if acyclic).
2. If `Some(idx)`, retrieve `cycles[idx].members` → the **SCC member set**.
3. Collect **all edges where both endpoints are in the SCC member set**. These
   are the cycle edges. This includes both:
   - Back-edges (the `back_edges` set in `CycleAnalysis`)
   - Forward edges within the cycle (tree edges that are part of the loop body)

### Why "all intra-SCC edges" not just back-edges

A cycle A→B→C→A has three edges. Only C→A is the back-edge, but B→C and A→B are
equally part of the loop. The user needs to see the complete ring, not just the
single back-edge arc.

### Multiple cycles / overlapping SCCs

`task_to_cycle` maps each task to exactly one SCC index (Tarjan guarantees
disjoint SCCs). A task cannot be in "overlapping" SCCs — SCCs are by definition
maximal. Within a single SCC there may be multiple elementary cycles, but we
color **all intra-SCC edges** yellow, which covers all of them. This is the
correct behavior: the SCC is the natural unit of "these tasks form a loop."

---

## 2. Which Edges Get Yellow Coloring

**Rule:** An edge `(src, tgt)` in `char_edge_map` is colored yellow if and only
if:
- The selected task is in an SCC (i.e., `task_to_cycle` has an entry for it), AND
- Both `src` and `tgt` are members of that same SCC.

This means:
- **All edges within the cycle ring** get yellow — tree edges, back-edge arcs,
  everything.
- **Edges entering or leaving the cycle** (one endpoint inside, one outside) are
  NOT yellow — they get the normal magenta/cyan trace coloring based on
  upstream/downstream membership.
- **Self-loops** (edge where `src == tgt`): colored yellow if the self-loop task
  is in an SCC (which it will be, since a self-loop forms a trivial SCC of
  size 1 if `include_self_loops` is true in `find_cycles`).

---

## 3. How Yellow Interacts with Magenta/Cyan Trace Colors

### Priority order (highest first)

1. **Yellow** — cycle edge (both endpoints in the selected task's SCC)
2. **Magenta** — upstream edge (both endpoints in `upstream_set ∪ {selected}`)
3. **Cyan** — downstream edge (both endpoints in `downstream_set ∪ {selected}`)
4. **Original style** — everything else

### Rationale

Yellow overrides magenta and cyan because:
- Cycle edges are a **strict subset** of the upstream+downstream edges (every
  cycle member is both upstream and downstream of every other cycle member).
- The loop structure is the most important topological feature to communicate.
  The user already knows these tasks are connected; the cycle coloring tells
  them *how* (in a loop).
- Without override, cycle edges would show as magenta or cyan, hiding the loop.

### Implementation location

The priority logic lives in `apply_per_char_trace_coloring()` in
`src/tui/viz_viewer/render.rs:216`. The existing code checks `is_upstream_edge`
then `is_downstream_edge`. The change adds a `is_cycle_edge` check **before**
both:

```rust
// In apply_per_char_trace_coloring, inside the edge-character branch:
let is_cycle_edge = edges.iter().any(|(src, tgt)| in_cycle(src) && in_cycle(tgt));
let is_upstream_edge = edges.iter().any(|(src, tgt)| in_upstream(src) && in_upstream(tgt));
let is_downstream_edge = edges.iter().any(|(src, tgt)| in_downstream(src) && in_downstream(tgt));

if is_cycle_edge {
    let mut s = *base_style;
    s.fg = Some(Color::Yellow);
    s
} else if is_upstream_edge {
    // ... existing magenta
} else if is_downstream_edge {
    // ... existing cyan
} else {
    *base_style
}
```

---

## 4. Required Changes

### 4.1 `VizOutput` (`src/commands/viz/mod.rs:15`)

**No change needed.** The cycle membership computation happens in the TUI layer,
not in the viz output. The TUI already has access to the wg directory and
can load the graph to compute `CycleAnalysis`. The `char_edge_map` already
contains `(source_id, target_id)` pairs — we just need to know which IDs are
cycle members to color them.

### 4.2 `VizApp` state (`src/tui/viz_viewer/state.rs`)

Add a new field:

```rust
/// Set of task IDs in the same SCC as the selected task.
/// Empty if the selected task is not in any cycle.
pub cycle_set: HashSet<String>,
```

### 4.3 `recompute_trace()` (`src/tui/viz_viewer/state.rs:375`)

After computing `upstream_set` and `downstream_set`, compute `cycle_set`:

```rust
// Compute cycle membership for the selected task.
self.cycle_set.clear();
if let Ok(graph) = load_graph_from_dir(&self.workgraph_dir) {
    let cycle_analysis = graph.compute_cycle_analysis();
    if let Some(&cycle_idx) = cycle_analysis.task_to_cycle.get(&selected_id) {
        for member in &cycle_analysis.cycles[cycle_idx].members {
            self.cycle_set.insert(member.clone());
        }
    }
}
```

**Performance note:** `load_graph` + `compute_cycle_analysis` is already called
during `load_viz()` (inside `generate_viz` → `generate_ascii`). For the trace
recomputation, we need to call it again because `recompute_trace()` doesn't
have access to the graph. Two options:

- **Option A (recommended):** Pass cycle analysis data through `VizOutput`. Add
  a `cycle_members: HashMap<String, HashSet<String>>` field mapping each task
  to its SCC members. Populate it in `generate_ascii` where `cycle_analysis`
  is already computed. The TUI stores it and uses it in `recompute_trace()`.
  This avoids re-parsing the graph file.

- **Option B:** Re-load the graph in `recompute_trace()`. Simpler but does
  redundant I/O on every arrow-key press.

**Recommendation: Option A.** The `VizOutput` already carries structured metadata
(`forward_edges`, `reverse_edges`, `char_edge_map`). Adding cycle membership is
consistent with this pattern.

#### VizOutput addition (Option A)

```rust
pub struct VizOutput {
    // ... existing fields ...
    /// Cycle membership: task_id → set of all task IDs in the same SCC.
    /// Only populated for tasks that are in non-trivial SCCs.
    pub cycle_members: HashMap<String, HashSet<String>>,
}
```

Populated in `generate_ascii` after the existing `compute_cycle_analysis()` call:

```rust
let mut cycle_members: HashMap<String, HashSet<String>> = HashMap::new();
for cycle in &cycle_analysis.cycles {
    let member_set: HashSet<String> = cycle.members.iter().cloned().collect();
    for member in &cycle.members {
        cycle_members.insert(member.clone(), member_set.clone());
    }
}
```

### 4.4 `apply_per_char_trace_coloring()` (`src/tui/viz_viewer/render.rs:216`)

Add `cycle_set` to the function's inputs (it already takes `app: &VizApp`
which will have the `cycle_set` field). Add the `in_cycle` closure and the
priority check as shown in section 3.

### 4.5 `load_viz()` (`src/tui/viz_viewer/state.rs:207`)

Store `cycle_members` from `VizOutput`:

```rust
self.cycle_members = viz_output.cycle_members;
```

And use it in `recompute_trace()`:

```rust
self.cycle_set.clear();
if let Some(members) = self.cycle_members.get(&selected_id) {
    self.cycle_set = members.clone();
}
```

### 4.6 Task text coloring for cycle members

When the selected task is in a cycle, cycle-member task text should also get a
visual distinction. The `classify_task_line` function (`render.rs:62`) currently
returns `Selected`, `Upstream`, `Downstream`, or `Unrelated`. Add a new variant:

```rust
enum LineTraceCategory {
    Selected,
    CycleMember,  // NEW
    Upstream,
    Downstream,
    Unrelated,
}
```

In `classify_task_line`, after the selected-task check but before upstream/downstream:

```rust
if app.cycle_set.contains(id) {
    return LineTraceCategory::CycleMember;
}
```

For rendering, `CycleMember` task text could get a yellow foreground (matching
the edge color) via `apply_per_char_trace_coloring`'s text-range logic, or just
follow the upstream/downstream coloring (since cycle members are both).
**Recommendation:** Do NOT override task text color for cycle members. Yellow
edges are sufficient signal. Task text should retain its status color (green for
done, yellow for in-progress, etc.) since that information is independently
useful. This keeps the design minimal — only edge/connector characters change.

**Simplified design: skip the `CycleMember` variant entirely.** The edge
coloring alone is sufficient.

---

## 5. Edge Cases

### Task in no cycle
`cycle_set` is empty. No yellow edges. Behavior is identical to current.

### Task in a 2-node cycle (A→B→A)
Both A and B are in the SCC. Edges A→B and B→A are both yellow. The tree edge
and the back-edge arc both get yellow.

### Self-loop (A→A)
`find_cycles` with `include_self_loops=true` reports this as a single-node SCC.
`cycle_set = {A}`. The self-loop arc edge has `src == tgt == A`, both in
`cycle_set`, so it gets yellow. Note: `CycleAnalysis::from_graph` uses
`NamedGraph::analyze_cycles` which calls `extract_cycle_metadata` — need to
verify self-loops are handled. If `include_self_loops` is false in
`find_cycles`, self-loops won't appear as SCCs and the self-loop arc won't be
yellow. This is acceptable — self-loops are uncommon in wg and the arc
rendering already distinguishes them visually.

### Large SCC (e.g., 5 tasks in a ring)
All 5 tasks are members. When any of them is selected, all intra-SCC edges are
yellow. This correctly shows the entire loop structure.

### SCC with internal structure (e.g., A→B→C→A plus B→D→A)
All of {A, B, C, D} are in one SCC (Tarjan merges them). All intra-SCC edges
are yellow. This is correct: the user sees the full cycle structure including
the shortcut path through D.

### Nested cycles that share nodes
Not possible — SCCs are disjoint by definition. A node belongs to exactly one
SCC. Nested loops (as detected by Havlak) are within the same SCC.

### Trace disabled (Tab toggle)
When `trace_visible` is false, no coloring is applied (existing behavior).
`cycle_set` is still computed but not used for rendering.

### Selected task at SCC boundary (has edges both inside and outside the SCC)
- Intra-SCC edges: yellow
- Edges from upstream non-SCC tasks into SCC: magenta (upstream trace)
- Edges from SCC to downstream non-SCC tasks: cyan (downstream trace)
This is the correct behavior — the boundary is clearly visible.

---

## 6. Summary of File Changes

| File | Change | Lines (est.) |
|---|---|---|
| `src/commands/viz/mod.rs` | Add `cycle_members` field to `VizOutput` | ~5 |
| `src/commands/viz/ascii.rs` | Populate `cycle_members` from existing `cycle_analysis` | ~10 |
| `src/tui/viz_viewer/state.rs` | Add `cycle_set: HashSet<String>`, `cycle_members: HashMap<String, HashSet<String>>` fields; update `load_viz()` and `recompute_trace()` | ~15 |
| `src/tui/viz_viewer/render.rs` | Add `is_cycle_edge` check in `apply_per_char_trace_coloring` before upstream/downstream checks | ~10 |

**Total: ~40 lines of code changes.** No new files. No new dependencies.

---

## 7. Testing Strategy

1. **Unit test in ascii.rs:** Create a cycle graph (A→B→C→A), verify
   `cycle_members` in `VizOutput` maps each member to `{A, B, C}`.

2. **TUI trace test in ascii.rs test suite:** Use the existing `TraceChecker`
   pattern (seen in tests starting at line ~2250) to verify that for a cycle
   graph with selected task in the cycle, all intra-SCC edges are classified
   as "cycle" (a new classification alongside "upstream"/"downstream").

3. **Snapshot test:** Add a prompt snapshot test that renders a graph with a
   cycle and verifies the output is stable.

4. **Manual verification:** `wg watch` on a wg with cycles, navigate
   to cycle members, confirm yellow edges appear.
