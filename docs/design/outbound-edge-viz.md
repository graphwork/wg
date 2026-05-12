# Design: Outbound Non-Tree Edge Visualization

## Problem

`wg viz` renders non-tree edges as right-side arcs with `←───┐` at the upper node
and `───┘` at the lower node. The arrowhead (`←`) always goes at the upper node
because `draw_back_edge_arcs` normalizes arc direction with min/max:

```rust
let target = arc.target_line.min(arc.source_line);  // always upper
let source = arc.target_line.max(arc.source_line);  // always lower
```

This works when the dependency flows **upward** (lower node blocks upper node):
fan-in from below, back-edges in cycles. The `←` at the upper node correctly marks
it as the dependent ("something flows into me").

But it **breaks for forward edges** — edges where the blocker is above the dependent.
The min/max normalization puts `←` at the blocker instead of the dependent:

```
root  (open) ←───┐    ← WRONG: ← at root, but root is the BLOCKER
└→ mid  (open)   │
  └→ end  (open) ┘    ← end depends on root, should have ← here
```

This misrepresents the edge direction. The reader sees "end feeds into root" when
the reality is "root also blocks end."

### When does this happen?

Forward edges arise when a task depends on both an ancestor and an intermediate node:

```yaml
# end.after = [mid, root]  — end depends on both mid and root
# mid.after = [root]        — mid depends on root
# DFS: root → mid → end, then root tries end again → forward edge
```

The tree shows the path `root → mid → end`. The direct `root → end` edge is non-tree.
Because root is above end in the rendering, the arc goes **downward** — an "outbound"
edge from root's perspective. Current code renders this with `←` at root, which is wrong.

This is common in wg task graphs: a design task might block both an implementation
task (directly) and a verification task (which also depends on the implementation).

## Current Behavior

All non-tree edges are rendered identically:

```
upper_node  (open) ←───┐    ← arrowhead always here
  ...                  │
lower_node  (open) ────┘    ← foot always here
```

The `BackEdgeArc` struct stores `source_line` (the DFS parent / blocker) and
`target_line` (the already-rendered node / dependent). But `draw_back_edge_arcs`
discards this direction information by normalizing to upper/lower.

### Edge classification in DFS

| DFS edge type   | Example        | Blocker position | Arrowhead should be at |
|-----------------|----------------|------------------|------------------------|
| Back-edge       | verify → design | below           | above (dependent)      |
| Fan-in (cross)  | right → join    | below           | above (dependent)      |
| Forward skip    | root → end      | above           | below (dependent)      |

The first two are correct today. The third is wrong.

## Proposed Solution: Direction-Aware Right-Side Arcs

### Core principle

**The arrowhead (`←`) always marks the dependent node**, regardless of its vertical
position. The `BackEdgeArc` already stores the correct direction — the rendering
just needs to use it.

### Visual design

**Upward arc** (dependent above, blocker below) — current behavior, unchanged:

```
dependent  (open) ←──┐
  ...                │
blocker    (open) ───┘
```

**Downward arc** (dependent below, blocker above) — new:

```
blocker    (open) ───┐
  ...                │
dependent  (open) ←──┘
```

Same corner characters (`┐` at top, `┘` at bottom, `│` between). Only the `←`
position changes: it follows the dependent, wherever it is.

### Same-dependent collapse (generalization)

Currently, same-target arcs collapse into one column. This generalizes to
**same-dependent collapse**: all non-tree edges targeting the same dependent share
a column, regardless of whether their sources are above or below.

**All sources below** (current, unchanged):

```
dependent  ←──┐
  ...         │
source-a  ───┤
source-b  ───┘
```

**All sources above** (new):

```
source-a  ───┐
source-b  ───┤
  ...         │
dependent  ←──┘
```

**Mixed: sources both above and below** (new):

```
source-a  ───┐       ← blocker above
  ...         │
dependent  ←──┤       ← dependent in middle (← + ┤)
  ...         │
source-b  ───┘       ← blocker below
```

The `←┤` at the dependent is the key new glyph: arrowhead (`←`) marks the receiver,
T-junction (`┤`) indicates the vertical line continues past this node.

### Concrete before/after examples

#### Forward skip: A → B → C, A → C

Before (wrong):
```
A  (open) ←──┐
└→ B  (open) │
  └→ C  (op)─┘
```

After (correct):
```
A  (open) ───┐
└→ B  (open) │
  └→ C  (op)←┘
```

#### Diamond with forward skip: A → {B,C} → D, A → D

Before (wrong):
```
A  (open) ←──────┐
├→ B  (open)     │
│ └→ D  (open)   │ ←──┐
└→ C  (open) ────│────┘
                 └─???
```
(Two separate arcs: A→D wrong direction, C→D correct)

After (correct, collapsed):
```
A  (open) ───────┐
├→ B  (open)     │
│ └→ D  (open) ←─┤
└→ C  (open) ────┘
```

One column, `←` at D (the shared dependent), sources A (above) and C (below).

#### Back-edge in cycle: unchanged

```
design  (open) ←──┐
└→ verify  (op)───┘
```

Back-edges always have the dependent above the blocker, so the current rendering
is already correct.

## Alternatives Considered

### Left-side arcs for outbound edges

Reserve the left margin for downward arcs, right margin for upward arcs.

```
     A  (open) ←───┐
  ┌──├→ B  (open)  │
  │  │ └→ C  (op)  │
  │  └→ D  (open)──┘
  └──▶ E  (open)
```

**Rejected.** The left side is already dense with tree connectors (`├→`, `└→`, `│`,
indentation). Adding arc routing there creates visual clutter and is hard to
distinguish from the tree structure. It also requires tracking indentation levels
for arc column placement, which is more complex than the right-side approach.

### Different arrow characters for direction

Use `←` for inbound (upward) arcs and `▶` for outbound (downward) arcs:

```
blocker  (open) ───┐
  ...              │
dependent  (op) ▶──┘
```

**Rejected.** `▶` points away from the node, which reads as "something departs from
here" — the opposite of the intended meaning ("something arrives here"). Using `←`
consistently is clearer: it always means "this node receives a dependency."

### Separate notation / text annotations

Don't draw arcs for outbound edges. Instead, append text like `(→ D)` to the source:

```
A  (open)  (→ D)
└→ B  (open)
  └→ D  (open)
```

**Rejected.** This is what the old system did for all non-tree edges (before the arc
rendering was implemented). Arcs are strictly superior: they visually connect the
two nodes, making the relationship immediately scannable without reading text. The
original design doc (viz-cycle-edge-design.md) specifically replaced text annotations
with arcs for this reason.

### Separate left/right columns by direction

Outbound arcs on the far right, inbound arcs near the text.

**Rejected.** This prevents same-dependent collapse when a node has blockers both
above and below. It also doubles the horizontal space needed for arcs.

## Implementation

### Step 1: Preserve edge direction in BackEdgeArc

Currently, `source_line` and `target_line` have ambiguous semantics (the DFS parent
and the already-rendered child, which doesn't consistently map to blocker/dependent).

Rename for clarity and add the dependent's line explicitly:

```rust
struct BackEdgeArc {
    blocker_line: usize,    // line of the blocking node (DFS parent)
    dependent_line: usize,  // line of the dependent node (already rendered)
}
```

In `render_tree`, the construction becomes:

```rust
// pid is the current DFS parent, id is the already-rendered child.
// In forward adjacency, pid → id means pid blocks id.
back_edge_arcs.push(BackEdgeArc {
    blocker_line: node_line_map[pid],
    dependent_line: node_line_map[id],
});
```

### Step 2: Group by dependent in draw_back_edge_arcs

Replace `by_target` (grouped by upper line) with `by_dependent`:

```rust
let mut by_dependent: HashMap<usize, Vec<usize>> = HashMap::new();
for arc in &real_arcs {
    by_dependent
        .entry(arc.dependent_line)
        .or_default()
        .push(arc.blocker_line);
}
```

### Step 3: Render with direction awareness

For each column (one per dependent):

1. Compute span: `top = min(dependent, min(blockers))`, `bottom = max(dependent, max(blockers))`
2. At top line:
   - If dependent: `←──┐` (arrowhead + corner)
   - If blocker: `───┐` (dash + corner)
3. At bottom line:
   - If dependent: `←──┘` (arrowhead + corner)
   - If blocker: `───┘` (dash + corner)
4. At intermediate lines:
   - If dependent: `←──┤` (arrowhead + T-junction)
   - If blocker: `───┤` (dash + T-junction)
   - Otherwise: `│` (vertical pass-through)

```rust
for (col_idx, column) in columns.iter().enumerate() {
    let col_x = margin_start + col_idx * 2;
    let top = column.top_line;
    let bottom = column.bottom_line;

    for line_idx in top..=bottom {
        let is_dep = line_idx == column.dependent;
        let is_blocker = column.blockers.contains(&line_idx);
        let is_top = line_idx == top;
        let is_bottom = line_idx == bottom;

        if is_top {
            if is_dep {
                // ←──┐
                render_arrowhead_corner_top(line, col_x);
            } else {
                // ───┐
                render_dash_corner_top(line, col_x);
            }
        } else if is_bottom {
            if is_dep {
                // ←──┘
                render_arrowhead_corner_bottom(line, col_x);
            } else {
                // ───┘
                render_dash_corner_bottom(line, col_x);
            }
        } else if is_dep || is_blocker {
            if is_dep {
                // ←──┤
                render_arrowhead_junction(line, col_x);
            } else {
                // ───┤
                render_dash_junction(line, col_x);
            }
        } else {
            // │
            render_vertical(line, col_x);
        }
    }
}
```

### Step 4: Column allocation

Sort columns by span (shortest first → innermost), same as today.
The span is `bottom - top`, where `top = min(dependent, min(blockers))`
and `bottom = max(dependent, max(blockers))`.

### Color treatment

Unchanged: arcs rendered in dim gray (`\x1b[90m`). The `←` could optionally use the
dependent's status color to make it pop, but that's a separate enhancement.

## Edge Cases

| Case | Handling |
|------|----------|
| Self-loop (A→A) | Unchanged: inline `↺` |
| All blockers below dependent | Same as current behavior |
| All blockers above dependent | New: `←` at bottom |
| Mixed above/below | `←┤` at dependent's line |
| Dependent and blocker on adjacent lines | Short arc (2 lines), works fine |
| Multiple columns overlapping vertically | Separate columns, no interference |
| Back-edge (cycle) | Unchanged: blocker below, dependent above → `←` at top |

## Testing Strategy

1. **Update existing tests**: `test_arc_back_edge_cycle`, `test_arc_fan_in_diamond`,
   `test_arc_same_target_collapse` — verify they still pass (upward arcs unchanged).

2. **New test: forward skip edge**. Graph: A→B→C, A→C. Assert `←` appears at C's
   line (bottom), not at A's line (top). Assert C's line contains `←` and A's line
   contains `┐` but not `←`.

3. **New test: mixed direction same-dependent**. Graph: A→B→D, C→D, A→D where C is
   rendered below D. Assert single column with `←┤` at D's line, `┐` at A's line,
   `┘` at C's line.

4. **New test: multiple forward edges from same source**. Graph: A→{B,C,D} where B,C,D
   are all rendered below A via other tree paths. Assert each gets its own column with
   `←` at the dependent.

## Summary

| Aspect | Current | Proposed |
|--------|---------|----------|
| Arrowhead position | Always at upper node | Always at dependent node |
| Upward arcs | Correct | Unchanged |
| Downward arcs | Wrong direction | Fixed |
| Same-target collapse | By upper node | By dependent node |
| Mixed-direction arcs | Not handled | ←┤ at dependent |
| Arc characters | ←┐ ┤ ┘ │ | Same set, position-aware |
| Right margin | Used | Unchanged |
| Left margin | Unused | Unchanged |
| Complexity | O(arcs) | O(arcs), same |
