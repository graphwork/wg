# Cycle Detection Algorithms for wg

**Date:** 2026-02-21
**Task:** research-cycle-detection
**Status:** Complete

---

## Motivation

wg currently distinguishes two edge types:

- **`blocked_by`**: regular dependency edges (must complete before I start)
- **`loops_to`**: special back-edges that fire on completion, re-opening upstream tasks

This distinction is arguably specious. A cycle is a structural property of the graph, not an edge-level declaration. This research investigates whether we can detect cycles from graph analysis and derive loop behavior from structure, eliminating the need for a separate `loops_to` edge type.

---

## 1. Core Algorithms

### 1.1 Tarjan's SCC Algorithm (1972)

**Paper:** R.E. Tarjan, "Depth-First Search and Linear Graph Algorithms," SIAM Journal on Computing, 1(2):146-160, 1972.

**Problem:** Find all strongly connected components (SCCs) in a directed graph. An SCC is a maximal set of vertices where every vertex is reachable from every other vertex — i.e., every SCC with >1 vertex contains at least one cycle.

**Algorithm:**

```
algorithm TARJAN_SCC(G):
    index := 0
    S := empty stack
    for each vertex v in V:
        if v.index is undefined:
            STRONGCONNECT(v)

function STRONGCONNECT(v):
    v.index := index
    v.lowlink := index        // lowest index reachable from v's subtree
    index := index + 1
    S.push(v)
    v.on_stack := true

    for each edge (v, w):
        if w.index is undefined:       // tree edge
            STRONGCONNECT(w)
            v.lowlink := min(v.lowlink, w.lowlink)
        else if w.on_stack:            // back edge to ancestor in current SCC
            v.lowlink := min(v.lowlink, w.index)

    if v.lowlink == v.index:           // v is root of an SCC
        SCC := {}
        repeat:
            w := S.pop()
            w.on_stack := false
            SCC := SCC ∪ {w}
        until w == v
        emit SCC
```

**Key concepts:**
- **DFS tree**: The spanning tree produced by depth-first search
- **Back edge**: An edge from a descendant to an ancestor in the DFS tree — indicates a cycle
- **Cross edge**: An edge to a vertex in a different, already-completed DFS subtree
- **lowlink**: The lowest DFS index reachable from a vertex's subtree via back edges
- **SCC root**: A vertex where `lowlink == index`, meaning no ancestor is reachable — it's the topmost vertex of its SCC

**Complexity:** O(V + E) time, O(V) space.

**Properties relevant to wg:**
- Finds ALL cycles simultaneously (grouped into SCCs)
- Single-pass algorithm — efficient for static graph analysis
- The condensation graph (collapsing each SCC to a single node) is always a DAG
- Does NOT identify individual cycles within an SCC or determine cycle headers

**Limitations for wg:**
- Identifies *which* nodes participate in cycles but not *how* the cycles are structured
- Cannot distinguish a simple 3-node cycle from a complex SCC with multiple interlocking cycles
- No concept of loop headers, nesting, or iteration order

### 1.2 Kosaraju's Algorithm (1978)

**Problem:** Same as Tarjan — find all SCCs.

**Algorithm:**

```
algorithm KOSARAJU_SCC(G):
    1. DFS on G, recording vertices by exit time (finish order)
    2. Build G^T (transpose graph — reverse all edges)
    3. DFS on G^T in decreasing exit-time order
    4. Each DFS tree in step 3 is one SCC
```

**Complexity:** O(V + E) time, O(V + E) space (needs the transpose graph).

**Key theorem:** In the condensation graph, any edge goes from an SCC with larger exit time to one with smaller exit time. Processing vertices by decreasing exit time in G^T therefore visits SCC roots in topological order.

**Comparison to Tarjan:** Same time complexity, but requires two passes and storing the transpose graph. Conceptually simpler but uses more memory. Less suitable for incremental updates.

**Relevance to wg:** Equivalent to Tarjan for SCC discovery. The two-pass structure is less amenable to online/incremental use. Not preferred.

### 1.3 Havlak's Loop Nesting Forest (1997)

**Paper:** P. Havlak, "Nesting of Reducible and Irreducible Loops," ACM TOPLAS, 19(4):557-567, 1997.

**Problem:** Build a *loop nesting tree* (or forest) for a control-flow graph that may contain both reducible and irreducible loops. This is more structured than SCC decomposition — it identifies individual loops, their headers, and their nesting relationships.

**Key concepts:**

- **Reducible loop**: A loop with a single entry point (the header). The header dominates all nodes in the loop body. All back edges point to the header. This is the "nice" case — `while` loops, `for` loops in structured programs.

- **Irreducible loop**: A loop with multiple entry points. No single node dominates all others. Arises from `goto` statements or complex control flow. Example: two nodes that can each be entered from outside the loop and that reach each other.

- **Loop header**: The entry point(s) of a loop. For reducible loops, there's exactly one. For irreducible loops, Havlak selects one based on DFS ordering.

- **Loop nesting forest**: A tree where each node is either a loop header or a non-loop basic block. Children of a loop header are the nodes in that loop's body (which may themselves be loop headers of inner loops).

**Algorithm (simplified):**

```
algorithm HAVLAK_LOOP_NESTING(G, root):
    1. Compute DFS spanning tree from root
    2. For each node n in reverse DFS preorder:
       a. worklist := {m : m→n is a back edge}   // nodes that "loop back" to n
       b. If worklist is empty, n is not a loop header; continue
       c. n is a loop header. Find loop body:
          - BFS/DFS backward from worklist nodes
          - Stop at nodes already assigned to n's loop
          - Use UNION-FIND to track which loop each node belongs to
       d. For irreducible loops: detect multiple entry points
          and add them to worklist
    3. Build nesting tree from parent pointers
```

**Complexity:** Havlak claimed O(V·α(V,E)) — almost linear via UNION-FIND. However, Ramalingam (1999) showed the original algorithm is actually O(V·E) in the worst case due to how irreducible loops are handled.

**Properties relevant to wg:**
- Identifies loop *headers* — the natural "entry point" to each loop
- Captures nesting hierarchy — loops inside loops
- Handles irreducible loops (multiple entry points)
- Assigns every node to its innermost enclosing loop

**Limitations:**
- Requires a rooted graph (single entry point), which wg may not have
- Depends on the DFS spanning tree used — different trees can yield different headers for irreducible loops
- Havlak proposes a normalization to maximize reducible loops discovered, but this adds complexity

### 1.4 Ramalingam's Improvements (1999/2002)

**Paper:** G. Ramalingam, "Identifying Loops in Almost Linear Time," ACM TOPLAS, 21(2):175-188, 1999. Also: "On Loops, Dominators, and Dominance Frontiers," ACM TOPLAS, 24(5):455-490, 2002.

**Problem:** Improve the worst-case complexity of loop nesting forest algorithms.

**Key results:**

1. **Havlak's algorithm is quadratic:** Ramalingam showed that Havlak's original algorithm has O(V·E) worst-case time, not the claimed almost-linear time. The issue is in how irreducible loop bodies are computed.

2. **Fixed Havlak:** Ramalingam showed how to modify Havlak's algorithm to achieve true O(V·α(V,E)) time by using UNION-FIND more carefully during the backward search for irreducible loop bodies.

3. **Fixed Sreedhar-Gao-Lee:** The DJ-graph based algorithm (see Section 2.3) also has quadratic worst-case time. Ramalingam showed how to fix it as well.

4. **Steensgaard's forest:** Ramalingam also discussed Steensgaard's (1993) alternative loop nesting definition and how to compute it efficiently.

**Complexity:** O(V·α(V,E)) ≈ O(V+E) for all practical purposes (α is the inverse Ackermann function, effectively ≤ 4 for any realistic input).

**Relevance to wg:** If we pursue loop nesting forests, Ramalingam's corrected algorithm is the one to implement. The complexity is effectively linear.

---

## 2. Related Work

### 2.1 Johnson's Algorithm — Finding All Elementary Circuits (1975)

**Paper:** D.B. Johnson, "Finding All the Elementary Circuits of a Directed Graph," SIAM J. Computing, 4(1):77-84, 1975.

**Problem:** Enumerate every simple (elementary) cycle in a directed graph.

**Algorithm:** Systematically explores cycles starting from each vertex. Uses a "blocking" mechanism to avoid redundant work: a vertex is blocked after being added to the current path and stays blocked as long as every path from it to the starting vertex intersects the current path.

**Complexity:** O((V + E)(C + 1)) time, O(V + E) space, where C is the number of elementary circuits.

**Relevance to wg:** This is the *wrong* algorithm for wg. The number of elementary cycles can be exponential in graph size (e.g., a complete graph on n vertices has O(n!) cycles). We don't need to enumerate all cycles — we need to identify cycle *structure* (headers, nesting). SCC decomposition + loop nesting is the right approach.

### 2.2 Nuutila & Soisalon-Soininen — Improved Tarjan (1994)

**Paper:** E. Nuutila, E. Soisalon-Soininen, "On Finding the Strongly Connected Components in a Directed Graph," Information Processing Letters, 49(1):9-14, 1994.

**Key improvements over Tarjan:**
- Avoids pushing vertices onto the stack when they're trivial (single-node) components
- Handles sparse graphs and graphs with many trivial components more economically
- Reduces space from v(2 + 5w) to v(1 + 4w) bits (where w is word size)
- Presented an efficient transitive closure algorithm as an application

**Relevance to wg:** Marginal improvement. wg's task graphs are small (hundreds to low thousands of nodes). The space savings are irrelevant at this scale. Petgraph already implements an optimized Tarjan variant.

### 2.3 Sreedhar, Gao & Lee — DJ-Graphs for Loop Identification (1996)

**Paper:** V.C. Sreedhar, G.R. Gao, Y.-F. Lee, "Identifying Loops Using DJ Graphs," ACM TOPLAS, 18(6):649-658, 1996.

**Approach:** Augments the dominator tree with "join edges" (J-edges) — edges in the original graph that are not dominator tree edges. Back edges in the DJ-graph correspond to loops. The algorithm traverses the dominator tree bottom-up to identify loops.

**Key requirement:** Needs the dominator tree, which requires a rooted graph (single entry point).

**Complexity:** O(V·E) worst-case (quadratic), but Ramalingam showed how to fix it to almost-linear.

**Relevance to wg:** Interesting because it explicitly uses dominators, which are well-understood and available in petgraph. However, it requires a single entry point, which wg graphs may not have. The dominator-based approach is more naturally suited to control-flow graphs (which always have a single entry) than task dependency graphs.

### 2.4 Pearce — Space-Efficient SCC (2016)

**Paper:** D.J. Pearce, "A Space-Efficient Algorithm for Finding Strongly Connected Components," Information Processing Letters, 116(1):47-52, 2016.

**Key contribution:** Reduces space to v(1 + 3w) bits by combining the index and lowlink arrays into a single `rindex` array. This is the algorithm used by petgraph's `tarjan_scc` implementation.

**Relevance to wg:** This is what we'd get "for free" by using petgraph. Optimal space efficiency at the same O(V+E) time complexity.

### 2.5 Incremental Cycle Detection (2018-2024)

The problem of detecting cycles as edges are added to a graph (without recomputing from scratch) has seen significant recent progress:

**Bender, Fineman & Gilbert (2009/2016):** Incremental cycle detection and topological ordering in O(min(m^{1/2}, n^{2/3}) · m) total time.

**Bhattacharya & Kulkarni (SODA 2020):** O(m^{4/3}) total expected update time for incremental cycle detection and topological ordering in sparse graphs. Breaks a longstanding barrier for the case m = O(n).

**Bernstein, Probst, Wulff-Nilsen (STOC 2024):** "Almost-Linear Time Algorithms for Incremental Graphs" — O(m^{1+o(1)}) total time for cycle detection, SCC maintenance, s-t shortest path, and minimum-cost flow. This is nearly optimal (can't do better than O(m) since you must read all edges).

**McCauley, Moseley et al. (2024):** "Incremental Topological Ordering and Cycle Detection with Predictions" — leverages ML predictions about graph structure to achieve O(mη) time where η is prediction error. Experiments show 36x cost reduction with even mildly accurate predictions.

**Relevance to wg:**
Highly relevant. wg graphs change dynamically as tasks and dependencies are added/removed. Full recomputation (O(V+E) Tarjan) after every edge addition is cheap for small graphs but wasteful for large ones. However, given wg's current scale (typically <1000 tasks), the practical benefit of incremental algorithms is minimal — a full Tarjan pass takes microseconds. The incremental algorithms become important only at scale (>100K nodes).

**Recommendation:** Start with full recomputation (Tarjan). If profiling shows cycle detection is a bottleneck, the STOC 2024 algorithm provides a nearly-optimal incremental solution. For practical use, maintaining a topological sort and detecting violations on edge insertion is simpler and sufficient.

### 2.6 Natural Loop Detection in Compilers

The compiler optimization literature defines loops differently from graph theory:

**Natural loop:** Given a back edge n→h (where h dominates n), the natural loop is the set of all nodes m such that h dominates m and m can reach n without going through h, plus h itself. h is the loop *header*.

**Algorithm:**
```
1. Compute dominator tree
2. Identify back edges: edge n→h where h dominates n
3. For each back edge n→h:
   a. Loop body := {h}
   b. Worklist := {n}
   c. While worklist not empty:
      - Remove m from worklist
      - If m not in loop body:
        - Add m to loop body
        - Add all predecessors of m to worklist
```

**Properties:**
- Two natural loops are either disjoint, nested, or share the same header
- If they share the same header, they can be merged into a single loop
- Loop nesting is always well-defined for reducible graphs

**Relevance to wg:** This is the most directly applicable model. In a task graph:
- The "header" is the task that gets re-opened when the loop iterates
- The "back edge" is the dependency that creates the cycle
- The "loop body" is the set of tasks that re-execute each iteration
- Nesting corresponds to inner loops (sub-cycles within a larger cycle)

The key challenge is that natural loop detection requires dominators, which require a single entry point. wg graphs can have multiple root tasks. Solutions: (1) add a virtual root node, (2) compute dominators per connected component, or (3) use SCC-based loop detection instead.

### 2.7 Petri Net Cycle Analysis

Petri nets model concurrent systems with places (states), transitions (events), and tokens (resources). They're widely used for workflow modeling.

**Cycle properties in Petri nets:**
- **Liveness:** A Petri net is live if every transition can eventually fire again — requires cycles in the reachability graph
- **Soundness:** A workflow net is sound if every case eventually completes and no tasks are left running — incompatible with unbounded cycles
- **Invariants:** Place invariants (P-invariants) identify sets of places whose total token count is conserved — cycle analysis helps find these

**Relevance to wg:**
- Petri nets distinguish between *structure* (the net topology) and *behavior* (token flow) — analogous to distinguishing graph structure from execution semantics
- The concept of "soundness" maps to wg's need for bounded loops (max_iterations)
- P-invariants could identify conservation laws in task cycles (e.g., "exactly one task in this cycle is active at any time")
- However, Petri net analysis is significantly more complex than what wg needs — it solves a more general concurrency problem

---

## 3. Application to wg

### 3.1 Current Model

```
Tasks are nodes.
blocked_by creates forward edges (A blocked_by B means B → A).
loops_to creates separate back-edges with metadata:
  - target: which task to re-open
  - guard: condition for firing (TaskStatus, IterationLessThan, Always)
  - max_iterations: hard cap
  - delay: optional time delay before re-activation

evaluate_loop_edges() fires after wg done:
  1. Check for "converged" tag → skip if present
  2. For each LoopEdge: check guard + iteration limit
  3. If loop fires: re-open target, clear assigned/timestamps, increment iteration
  4. Find and re-open intermediate tasks via BFS
  5. Re-open source task itself

Key properties:
  - loops_to edges are NOT in blocked_by → don't affect ready_tasks()
  - Iteration tracking is per-task (loop_iteration field)
  - Convergence via --converged flag (adds tag, prevents loop firing)
  - max_iterations is mandatory (no unbounded loops)
```

### 3.2 Proposed Model — Cycles from Structure

The proposed model eliminates `loops_to` as a separate edge type:

```
Only blocked_by edges exist.
Cycles in blocked_by are natural: A blocked_by B, B blocked_by C, C blocked_by A.
System detects cycles via SCC decomposition.
Loop behavior is derived from cycle structure.
```

### 3.3 Analysis: What Cycle Detection Gives Us

**SCC decomposition tells us:**
- Which tasks participate in cycles (SCC size > 1)
- Which groups of tasks form tightly-coupled cycles
- The condensation graph (DAG of SCCs) gives the acyclic "skeleton"

**Loop nesting analysis tells us:**
- Which task is the loop "header" (entry point)
- How cycles are nested (inner loops vs. outer loops)
- Which edges are "back edges" (create the cycle) vs. "forward edges" (normal flow)

**What we still need to declare explicitly:**
- `max_iterations` — cannot be inferred from structure
- Convergence conditions — semantic, not structural
- Delay between iterations — operational, not structural
- Guards — depend on runtime state, not graph topology

### 3.4 Identifying the Loop Header

**Question:** In a cycle `A → B → C → A`, which task is the "header" — i.e., which one gets re-opened to start a new iteration?

**Approaches:**

1. **Dominator-based (compiler approach):** The header is the node that dominates all others in the loop. Requires computing dominators.
   - **Problem:** If the graph has multiple roots, we need a virtual root. And in an irreducible loop, no single node dominates all others.

2. **Entry-node heuristic:** The header is the node with incoming edges from outside the SCC. In a task graph, this is the task that gets "entered" from the acyclic portion.
   - **Problem:** Multiple entry points are possible. Which one is "the" header?

3. **Explicit annotation:** The user marks one task in the cycle as the header.
   - **Problem:** Defeats the purpose of automatic detection.

4. **DFS-based (Tarjan/Havlak approach):** The header is the first node in the SCC encountered during DFS. Depends on DFS ordering.
   - **Problem:** Non-deterministic — different orderings give different headers.

5. **Topological position:** The header is the node in the SCC with the most incoming edges from outside the SCC, or the "earliest" in the dependency chain.
   - **Problem:** Heuristic, may not match user intent.

**Recommendation:** Use the *entry node* heuristic: the header is the node in the SCC that has at least one predecessor outside the SCC (or no predecessors at all). If multiple entry nodes exist, treat the cycle as having multiple entry points (irreducible) and require explicit annotation. For the common case of a simple cycle entered from one point, this works automatically.

### 3.5 Handling Nested Loops

**Scenario:** An inner loop inside an outer loop.

```
Outer: A → B → C → A (3-task cycle)
Inner: B → D → B (2-task cycle, nested inside the outer loop)
```

**With loop nesting forest:**
- Outer loop header: A
- Inner loop header: B
- D is in the inner loop only
- When B completes, the inner loop fires first (D re-opens)
- When the inner loop converges, B stays done, C becomes ready
- When C completes and A re-opens, the outer loop iterates

**Challenge:** `max_iterations` applies per-loop, not per-edge. Each loop (identified by its SCC or loop header) needs its own iteration counter.

**Design implication:** Iteration tracking moves from per-task to per-cycle. Each detected cycle (or loop nesting tree node) gets:
- `cycle_id`: identifier for the cycle
- `iteration`: current iteration count
- `max_iterations`: cap
- `converged`: boolean

### 3.6 Handling Irreducible Loops

**Scenario:** Multiple entry points to a cycle.

```
X → A → B → A   (A is an entry from X)
Y → B → A → B   (B is an entry from Y)
```

Both A and B are entry points. There is no single header.

**Options:**
1. **Reject:** Require all cycles to be reducible (single entry point). Flag irreducible cycles as errors in `wg check`.
2. **Pick one:** Arbitrarily choose a header (e.g., the one with the lowest ID). May not match user intent.
3. **Allow multiple headers:** Each entry point can independently trigger the loop. More complex iteration tracking.

**Recommendation:** For v1, reject irreducible loops. wg's use cases (review-revise, CI retry, monitor-fix-verify) are all reducible — they have a clear starting point. If irreducible loops are needed later, option 3 can be added.

### 3.7 Dynamic Graph Changes

**Question:** What happens when tasks are added or removed while the graph is running?

**Scenarios:**
1. **New task added to an existing cycle:** SCC decomposition changes. The new task becomes part of the cycle body.
2. **Task removed from a cycle:** The cycle may break, reducing to a DAG path.
3. **New dependency creates a cycle:** Previously acyclic tasks become cyclic.

**Strategy:** Recompute cycle detection on every graph mutation. Given wg's scale (<1000 tasks), a full Tarjan pass is O(V+E) ≈ microseconds. No need for incremental algorithms.

**Caching:** Store the cycle analysis result alongside the graph. Invalidate on any structural change (add/remove task, add/remove dependency).

### 3.8 Coordinator Dispatch Integration

Currently, `ready_tasks()` requires ALL blockers to be Done. In a cycle, this is impossible — at least one blocker is always not-Done.

**Proposed change to `ready_tasks()`:**

```rust
fn ready_tasks(graph: &wg, cycle_info: &CycleInfo) -> Vec<&Task> {
    graph.tasks().filter(|task| {
        if task.status != Status::Open { return false; }
        if task.paused { return false; }
        if !is_time_ready(task) { return false; }

        task.blocked_by.iter().all(|blocker_id| {
            // Normal check: blocker is terminal
            if graph.get_task(blocker_id)
                .map(|t| t.status.is_terminal())
                .unwrap_or(true) {
                return true;
            }
            // NEW: If blocker is in the same SCC and this task is the
            // cycle header, treat the back-edge blocker as satisfied
            if cycle_info.is_back_edge(blocker_id, &task.id) {
                return true;
            }
            false
        })
    }).collect()
}
```

This is the critical semantic change: back-edges within an SCC are treated as "soft" dependencies that don't prevent readiness. Only the header task gets this treatment — non-header tasks in the cycle still wait for their predecessors normally.

### 3.9 Intermediate Task Re-opening

When a cycle iterates, intermediate tasks must be re-opened. This behavior is already implemented in `find_intermediate_tasks()` (BFS between loop target and source).

In the proposed model, this logic is preserved but derived from cycle structure:
1. Cycle header is re-opened (status → Open, iteration incremented)
2. All tasks in the same SCC that depend (directly or transitively) on the header have their status reset to Open
3. Tasks outside the SCC are unaffected

### 3.10 What max_iterations and Guards Attach To

In the current model, `max_iterations` and guards are properties of the `LoopEdge`. In the proposed model, they need a new home:

**Option A: Properties of the cycle header task**
```yaml
id: write-draft
cycle:
  max_iterations: 5
  guard: { task_status: { task: review-draft, status: Failed } }
```

**Option B: Properties of the cycle (stored separately)**
```yaml
cycles:
  - id: review-loop
    header: write-draft
    members: [write-draft, review-draft, revise-draft]
    max_iterations: 5
    guard: ...
```

**Option C: Properties of the back-edge (the specific blocked_by that creates the cycle)**
```yaml
id: write-draft
blocked_by:
  - id: revise-draft
    is_back_edge: true  # auto-detected
    max_iterations: 5
    guard: ...
```

**Recommendation:** Option C is most natural. The `blocked_by` field already specifies dependencies. Annotating the specific dependency that creates the cycle (the back-edge) with loop metadata is clean and doesn't require a separate data structure. The back-edge is auto-detected; the metadata is user-specified.

However, Option A has the advantage of simplicity — the header task is what the user interacts with, so putting cycle metadata there is intuitive. The `--converged` flag already targets the task, not an edge.

**Practical recommendation:** Option A for v1 (simple, header-centric), with the cycle membership computed automatically via SCC analysis.

---

## 4. Proposed Architecture

### 4.1 Overview

```
┌─────────────────────────────────────────┐
│              wg                   │
│                                         │
│  tasks: HashMap<String, Task>           │
│  blocked_by edges (may contain cycles)  │
│                                         │
│  ┌──────────────────────────────────┐   │
│  │       CycleAnalysis (cached)      │   │
│  │                                   │   │
│  │  sccs: Vec<SCC>                   │   │
│  │  headers: HashMap<CycleId, TaskId>│   │
│  │  membership: HashMap<TaskId, CycleId>│ │
│  │  back_edges: HashSet<(TaskId, TaskId)>│ │
│  │  nesting: LoopNestingForest       │   │
│  └──────────────────────────────────┘   │
│                                         │
│  CycleConfig (per-header or per-cycle): │
│    max_iterations: u32                  │
│    guard: Option<LoopGuard>             │
│    delay: Option<Duration>              │
│    iteration: u32                       │
└─────────────────────────────────────────┘
```

### 4.2 Cycle Analysis Pipeline

```
1. Build adjacency list from blocked_by edges
2. Run Tarjan's SCC (via petgraph::algo::tarjan_scc)
3. Filter to non-trivial SCCs (size > 1)
4. For each SCC:
   a. Identify entry nodes (nodes with predecessors outside the SCC)
   b. If exactly one entry node → reducible loop, entry = header
   c. If multiple entry nodes → irreducible, warn/error
   d. If no entry nodes → isolated cycle, pick node with lowest ID as header
5. Identify back-edges: edges within the SCC that point to the header
   or that complete the cycle (from last node back to first in topo order)
6. Build nesting tree if SCCs overlap on the condensation graph
7. Cache result; invalidate on graph mutation
```

### 4.3 Data Structures

```rust
/// Cached cycle analysis, recomputed on graph mutations
struct CycleAnalysis {
    /// Non-trivial SCCs (cycles)
    cycles: Vec<Cycle>,
    /// Which cycle each task belongs to (if any)
    task_to_cycle: HashMap<String, CycleId>,
    /// Edges classified as back-edges (create the cycle)
    back_edges: HashSet<(String, String)>,
}

struct Cycle {
    id: CycleId,
    /// All task IDs in this cycle
    members: Vec<String>,
    /// The entry point / loop header
    header: String,
    /// Is this a simple (reducible) cycle?
    reducible: bool,
    /// Nesting: parent cycle ID if this cycle is nested inside another
    parent: Option<CycleId>,
}

/// Per-cycle configuration, stored on the header task
struct CycleConfig {
    max_iterations: u32,
    guard: Option<LoopGuard>,
    delay: Option<Duration>,
}

/// Per-cycle runtime state, stored on the header task
struct CycleState {
    iteration: u32,
    converged: bool,
}
```

### 4.4 Modified Dispatch Flow

```
on task_completion(task_id):
    1. Mark task as Done
    2. analysis := get_or_compute_cycle_analysis(graph)
    3. If task_id is the "last" node in a cycle (all other cycle members are Done):
       a. cycle := analysis.get_cycle(task_id)
       b. header := cycle.header
       c. config := get_cycle_config(header)
       d. state := get_cycle_state(header)
       e. If state.converged → do nothing, cycle terminates
       f. If state.iteration >= config.max_iterations → do nothing
       g. If config.guard is Some and !evaluate_guard(config.guard, graph) → do nothing
       h. Otherwise: iterate the cycle
          - Set header.status = Open
          - Increment state.iteration
          - Re-open all cycle members
          - Apply delay if configured
    4. Normal downstream unblocking (tasks outside the cycle)
```

### 4.5 Migration Path

The transition from `loops_to` to structural cycle detection can be done incrementally:

**Phase 1: Add cycle analysis (non-breaking)**
- Add `CycleAnalysis` computation (Tarjan SCC via petgraph)
- Add `wg cycles` command to show detected cycles
- Keep `loops_to` working as-is

**Phase 2: Support natural cycles in blocked_by**
- Modify `ready_tasks()` to handle cycle headers (back-edge awareness)
- Add cycle config (max_iterations, guard) as task metadata
- Allow cycles in `blocked_by` to execute

**Phase 3: Migrate loops_to to blocked_by cycles**
- Provide migration tool: convert `loops_to` edges to regular `blocked_by` edges
- Deprecate `loops_to`

**Phase 4: Remove loops_to**
- Remove `LoopEdge` struct, `loops_to` field, `evaluate_loop_edges()`
- All loop behavior derived from cycle analysis

---

## 5. Open Questions and Design Decisions

### 5.1 Should We Actually Do This?

**Arguments for structural cycle detection:**
- Eliminates a special edge type (`loops_to`) — simpler model
- Cycles are a graph property, not an edge property — more principled
- Users can create cycles naturally with `blocked_by` without learning a new concept
- Cycle analysis provides richer information (nesting, headers, condensation)

**Arguments against (keeping `loops_to`):**
- **`loops_to` works.** The current system is mature, well-tested (100+ tests), and handles all use cases. Rewriting it is risk without clear user-facing benefit.
- **Explicit is better than implicit.** `loops_to` makes the user's intent clear: "I want this to loop." A cycle in `blocked_by` could be accidental — the system can't distinguish intent from error without annotation.
- **Separation of concerns.** `blocked_by` means "don't start until this is done." A back-edge in a cycle means something fundamentally different: "re-open this when I'm done." Overloading `blocked_by` with two meanings is arguably worse than having two edge types.
- **max_iterations must live somewhere.** Whether it's on a `LoopEdge` or a `CycleConfig`, the user must still declare loop bounds. The structural approach doesn't eliminate configuration — it just moves it.
- **Performance cost.** Every graph mutation triggers cycle reanalysis. With `loops_to`, there's no analysis needed — the edges are explicit.

### 5.2 Hybrid Approach

A middle ground: keep `loops_to` for user intent but validate/enhance it with cycle analysis.

- **Validation:** If a user creates a `loops_to` edge, verify that the source and target are in the same SCC (or would be with the loop edge included). Warn if the loop edge is "impossible" (source can't reach target).
- **Auto-detection:** If a cycle exists in `blocked_by` AND has cycle metadata (max_iterations) on the header, treat it as an intentional loop without requiring `loops_to`.
- **Migration:** Support both modes during a transition period.

### 5.3 Cycle Identity Stability

When the graph changes, cycle analysis may change. A cycle that existed before might split into two cycles, or merge with another. This affects:
- Iteration counters (which cycle does the counter belong to?)
- Convergence state
- User expectations ("I set max_iterations on this cycle, but the cycle changed")

**Mitigation:** Use the header task ID as the stable cycle identifier. As long as the header is in a cycle, the cycle state is associated with it. If the header is removed from the cycle (e.g., a dependency is removed), the cycle state is lost.

### 5.4 Multiple Cycles Through the Same Node

A task can participate in multiple cycles (e.g., A→B→A and A→C→A). With `loops_to`, this is handled naturally — each loop edge is independent. With structural detection, the task is in a single SCC but may be part of multiple elementary cycles within that SCC.

**Challenge:** Which cycle's max_iterations applies? Which guard? The loop nesting forest resolves some of this (nesting), but siblings are harder.

**Mitigation:** If a task is in an SCC, the SCC is treated as one cycle with one header, one max_iterations, and one guard. Individual elementary cycles within the SCC are not distinguished. This is a simplification but matches the current `loops_to` semantics (where the source task has one set of loop edges, not per-cycle configuration).

### 5.5 When Is a Cycle "Complete" (Ready to Iterate)?

With `loops_to`, the trigger is simple: the source task completes → evaluate loop edges. With structural detection, the trigger is less clear:

- **All-Done trigger:** The cycle iterates when ALL tasks in the cycle are Done. This is clean but inflexible — what if only the "last" task matters?
- **Header completion trigger:** The cycle iterates when the header task completes and all other cycle members are Done. But the header might not be "last."
- **Any-Done trigger:** The cycle iterates when ANY task in the cycle completes. Too aggressive — intermediate tasks completing shouldn't trigger iteration.
- **Back-edge trigger:** The cycle iterates when the task at the tail of the back-edge completes. This is closest to the current model and the most natural.

**Recommendation:** Back-edge trigger. The task at the tail end of the back-edge (the "last" task in the cycle before looping back) is the trigger point. This preserves the current semantics of `loops_to`.

---

## 6. Rust Implementation Considerations

### 6.1 Petgraph Ecosystem

Petgraph provides the following relevant algorithms:

| Algorithm | Function | Complexity | Notes |
|-----------|----------|------------|-------|
| Tarjan's SCC | `tarjan_scc()` | O(V+E) | Pearce's space-efficient variant |
| Kosaraju's SCC | `kosaraju_scc()` | O(V+E) | Two-pass, needs transpose |
| Cycle detection | `is_cyclic_directed()` | O(V+E) | Boolean only |
| Condensation | `condensation()` | O(V+E) | Collapses SCCs into DAG |
| Dominator tree | `dominators::simple_fast()` | O(V^2) | Cooper et al. algorithm |
| Topological sort | `toposort()` | O(V+E) | Fails on cycles |
| Has path | `has_path_connecting()` | O(V+E) | BFS reachability |

**What petgraph does NOT have:**
- Havlak's loop nesting forest
- Natural loop detection (compiler-style)
- Back-edge classification
- Incremental SCC maintenance

### 6.2 Implementation Strategy

**Phase 1 (minimal, using petgraph):**
```rust
use petgraph::algo::tarjan_scc;
use petgraph::graph::DiGraph;

fn analyze_cycles(graph: &wg) -> CycleAnalysis {
    // 1. Build petgraph DiGraph from blocked_by edges
    let mut pg = DiGraph::<&str, ()>::new();
    let mut node_map: HashMap<&str, NodeIndex> = HashMap::new();
    for task in graph.tasks() {
        let idx = pg.add_node(&task.id);
        node_map.insert(&task.id, idx);
    }
    for task in graph.tasks() {
        for blocker in &task.blocked_by {
            if let (Some(&from), Some(&to)) = (node_map.get(blocker.as_str()), node_map.get(task.id.as_str())) {
                pg.add_edge(from, to, ());
            }
        }
    }

    // 2. Run Tarjan's SCC
    let sccs = tarjan_scc(&pg);

    // 3. Filter to non-trivial SCCs and identify headers
    let mut analysis = CycleAnalysis::new();
    for scc in sccs {
        if scc.len() <= 1 { continue; }
        // Identify header: node with in-edges from outside SCC
        // Build cycle struct
        // Classify back-edges
    }
    analysis
}
```

**Phase 2 (if needed — custom loop nesting):**
If simple SCC analysis isn't sufficient (e.g., nested loops need explicit support), implement Havlak's algorithm (with Ramalingam's fix) on top of petgraph:

```rust
fn build_loop_nesting_forest(graph: &DiGraph, root: NodeIndex) -> LoopNestingForest {
    let doms = dominators::simple_fast(graph, root);
    // Classify edges as tree/forward/back/cross using DFS
    // For each back edge n→h where h dominates n:
    //   Create natural loop with header h
    //   Find loop body via backward DFS from n
    // Build nesting tree from loop containment
}
```

This requires:
1. A single root node (add virtual root if needed)
2. Dominator tree computation (available in petgraph)
3. ~200-300 lines of Rust code for the loop nesting construction

### 6.3 Incremental Update Strategy

For wg's current scale, full recomputation is fine:

```rust
impl WorkGraph {
    fn invalidate_cycle_cache(&mut self) {
        self.cycle_analysis = None;
    }

    fn cycle_analysis(&mut self) -> &CycleAnalysis {
        if self.cycle_analysis.is_none() {
            self.cycle_analysis = Some(analyze_cycles(self));
        }
        self.cycle_analysis.as_ref().unwrap()
    }

    fn add_dependency(&mut self, task_id: &str, blocker_id: &str) {
        // ... existing logic ...
        self.invalidate_cycle_cache();
    }

    fn remove_dependency(&mut self, task_id: &str, blocker_id: &str) {
        // ... existing logic ...
        self.invalidate_cycle_cache();
    }
}
```

If wg grows to >10K tasks and graph mutations are frequent, consider:
1. **Dirty flag per-SCC:** Only recompute SCCs that are affected by the changed edge
2. **Online topological sort with cycle detection:** Maintain a topological order; on edge insertion, check if it creates a cycle by verifying if the target precedes the source. O(affected_vertices) per update.
3. **Full incremental SCC:** Bernstein et al. 2024 algorithm, but this is research-grade and complex to implement.

### 6.4 Other Rust Crates

| Crate | Relevance | Notes |
|-------|-----------|-------|
| `petgraph` | High | SCC, condensation, dominators, topological sort |
| `graph-cycles` | Low | Enumerates all cycles (Johnson's algorithm) — wrong approach |
| `pathfinding` | Low | Shortest paths, not cycle detection |
| `daggy` | None | Explicitly DAG-only, rejects cycles |

**Recommendation:** Use petgraph for SCC and dominator computation. Implement loop nesting (if needed) as custom code on top of petgraph's data structures.

---

## 7. Conclusion and Recommendations

### Assessment

The current `loops_to` system is well-designed and battle-tested. Replacing it with structural cycle detection is technically sound but involves significant risk for modest conceptual gain.

### Recommended Path

1. **Short-term: Add cycle analysis as a diagnostic layer.** Use petgraph's `tarjan_scc` to detect cycles in `blocked_by` edges. Add a `wg cycles` command. Use this for validation and visualization, not execution.

2. **Medium-term: Allow natural cycles in blocked_by alongside loops_to.** If users create a cycle in `blocked_by` and annotate the header with cycle metadata, it works. `loops_to` continues to work as before. Users can choose either model.

3. **Long-term: Evaluate whether loops_to should be deprecated.** After gaining experience with structural cycles, decide whether the simplification justifies the migration cost. The answer may be "no" — having both models provides flexibility.

4. **Do NOT implement Havlak/Ramalingam unless needed.** SCC decomposition is sufficient for wg's use cases. Loop nesting forests add complexity that's only justified for deeply nested cycles (rare in task graphs).

5. **Do NOT implement incremental cycle detection.** wg's scale doesn't justify it. Full Tarjan on every mutation is microseconds for <1000 tasks.

### Implementation Status (2026-02-21)

> **Update:** The Phase 1 implementation in `src/cycle.rs` went beyond the minimal recommendations above:
>
> - **Custom std-only implementation** — petgraph was not used. A custom iterative Tarjan SCC (~160 lines) avoids adding a dependency for a straightforward algorithm.
> - **All four algorithms implemented:** Tarjan SCC, Havlak Loop Nesting Forest, Incremental Cycle Detection, and Cycle Metadata Extraction.
> - **53 tests passing** covering edge cases, performance, and wg-specific scenarios.
> - Recommendations 4 and 5 (don't implement Havlak/incremental) were overridden during implementation — both are useful for diagnostic analysis and the implementation is clean. They remain read-only analysis tools (Phase 1 scope) and don't affect execution behavior.
>
> Validation report: All 559 unit tests + 4 doc-tests pass. No regressions.

---

## 8. Annotated Bibliography

### Core Papers

1. **Tarjan, R.E. (1972).** "Depth-First Search and Linear Graph Algorithms." *SIAM Journal on Computing*, 1(2):146-160.
   The foundational algorithm for SCC detection. O(V+E) time via single-pass DFS with lowlink values. Used as the basis for all subsequent cycle detection work.
   Available at: https://www.cs.cmu.edu/~cdm/resources/Tarjan1972-sccs.pdf

2. **Havlak, P. (1997).** "Nesting of Reducible and Irreducible Loops." *ACM TOPLAS*, 19(4):557-567.
   Constructs loop nesting forests for arbitrary control-flow graphs. Extends Tarjan's interval-finding to handle irreducible loops. Claimed almost-linear time but later shown to be quadratic by Ramalingam.
   Available at: https://dl.acm.org/doi/10.1145/262004.262005

3. **Ramalingam, G. (1999).** "Identifying Loops in Almost Linear Time." *ACM TOPLAS*, 21(2):175-188.
   Shows Havlak's algorithm is quadratic in the worst case and provides a true almost-linear fix. Also fixes the Sreedhar-Gao-Lee algorithm. The definitive reference for efficient loop nesting computation.
   Available at: https://dl.acm.org/doi/10.1145/358438.349330

4. **Ramalingam, G. (2002).** "On Loops, Dominators, and Dominance Frontiers." *ACM TOPLAS*, 24(5):455-490.
   Extended treatment unifying loop detection, dominator computation, and dominance frontiers. Provides the theoretical framework for compiler loop optimizations.
   Available at: https://dl.acm.org/doi/10.1145/570886.570887

### Related Algorithms

5. **Johnson, D.B. (1975).** "Finding All the Elementary Circuits of a Directed Graph." *SIAM J. Computing*, 4(1):77-84.
   Enumerates all simple cycles in O((V+E)(C+1)) time. Not suitable for wg — the number of cycles can be exponential. Useful only when you need to *list* all cycles.
   Available at: https://www.cs.tufts.edu/comp/150GA/homeworks/hw1/Johnson%2075.PDF

6. **Nuutila, E. & Soisalon-Soininen, E. (1994).** "On Finding the Strongly Connected Components in a Directed Graph." *Information Processing Letters*, 49(1):9-14.
   Space improvements to Tarjan's SCC algorithm. Avoids unnecessary stack operations for trivial (single-node) components. Reduces space from v(2+5w) to v(1+4w) bits.
   Available at: https://www.sciencedirect.com/science/article/abs/pii/0020019094900477

7. **Sreedhar, V.C., Gao, G.R. & Lee, Y.-F. (1996).** "Identifying Loops Using DJ Graphs." *ACM TOPLAS*, 18(6):649-658.
   Loop identification via DJ-graphs (dominator tree + join edges). Bottom-up traversal of dominator tree. Quadratic worst-case, fixed by Ramalingam (1999).
   Available at: https://dl.acm.org/doi/10.1145/236114.236115

8. **Pearce, D.J. (2016).** "A Space-Efficient Algorithm for Finding Strongly Connected Components." *Information Processing Letters*, 116(1):47-52.
   Further space reduction to v(1+3w) bits by combining index and lowlink into a single array. This is the algorithm used by petgraph's `tarjan_scc`.
   Available at: https://whileydave.com/publications/Pea16_IPL_preprint.pdf

### Incremental/Dynamic Algorithms

9. **Bender, M.A., Fineman, J.T. & Gilbert, S. (2016).** "A New Approach to Incremental Cycle Detection and Related Problems." *ACM TALG*, 12(2), Article 14.
   O(min(m^{1/2}, n^{2/3}) · m) total time for incremental cycle detection. Foundational work on the incremental problem.

10. **Bhattacharya, S. & Kulkarni, A. (SODA 2020).** "An Improved Algorithm for Incremental Cycle Detection and Topological Ordering in Sparse Graphs."
    O(m^{4/3}) total expected update time. Breaks a longstanding barrier for sparse graphs.
    Available at: https://doi.org/10.1137/1.9781611975994.153

11. **Bernstein, A., Probst, M. & Wulff-Nilsen, C. (STOC 2024).** "Almost-Linear Time Algorithms for Incremental Graphs: Cycle Detection, SCCs, s-t Shortest Path, and Minimum-Cost Flow."
    O(m^{1+o(1)}) total time — nearly optimal. State-of-the-art for incremental cycle detection.
    Available at: https://dl.acm.org/doi/10.1145/3618260.3649745 and https://arxiv.org/abs/2311.18295

12. **McCauley, S., Moseley, B. et al. (2024).** "Incremental Topological Ordering and Cycle Detection with Predictions."
    Learning-augmented algorithms achieving O(mη) time with prediction error η. Demonstrates practical 36x speedup with 5% training data.
    Available at: https://arxiv.org/abs/2402.11028

### Compiler and Workflow Literature

13. **Cooper, K.D., Harvey, T.J. & Kennedy, K. (2001).** "A Simple, Fast Dominance Algorithm." *Software Practice and Experience*. The dominator algorithm implemented in petgraph. O(V^2) but fast in practice.

14. **van der Aalst, W.M.P. (1998).** "The Application of Petri Nets to Workflow Management." *J. Circuits, Systems and Computers*, 8(1):21-66.
    Comprehensive treatment of Petri nets for workflow modeling. Discusses cycle analysis, liveness, and soundness in workflow contexts.
    Available at: https://www.worldscientific.com/doi/10.1142/S0218126698000043

15. **Lengauer, T. & Tarjan, R.E. (1979).** "A Fast Algorithm for Finding Dominators in a Flowgraph." *ACM TOPLAS*, 1(1):121-141.
    The classic O(E log V) dominator algorithm. More efficient than Cooper et al. but more complex to implement. petgraph uses Cooper's simpler algorithm.
