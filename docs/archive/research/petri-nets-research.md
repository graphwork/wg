# Petri Nets Research: Verification Layer for workgraph

This document synthesizes research on Petri nets and their application to workflow verification, with practical guidance for implementing a verification layer in the workgraph project.

## Table of Contents

1. [What Are Petri Nets](#1-what-are-petri-nets)
2. [Key Properties and Verification](#2-key-properties-and-verification)
3. [Verification Algorithms](#3-verification-algorithms)
4. [Existing Implementations](#4-existing-implementations)
5. [Mapping Workflows to Petri Nets](#5-mapping-workflows-to-petri-nets)
6. [Lightweight Verification Approaches](#6-lightweight-verification-approaches)
7. [Recommendations for workgraph](#7-recommendations-for-workgraph)

---

## 1. What Are Petri Nets

### Core Concepts

A Petri net is a mathematical model for describing distributed systems, consisting of:

| Element | Symbol | Description |
|---------|--------|-------------|
| **Places** | Circles (○) | Represent conditions, resources, or states |
| **Transitions** | Rectangles/Bars (▢) | Represent events or actions that can occur |
| **Tokens** | Dots (●) | Represent the current state; reside in places |
| **Arcs** | Arrows (→) | Connect places to transitions (input) and transitions to places (output) |

**Basic Execution Rule (Firing):**
1. A transition is *enabled* when all its input places have at least one token
2. When a transition *fires*, it removes one token from each input place and adds one token to each output place

**Example:** A simple producer-consumer pattern:
```
[Ready] ──► (Produce) ──► [Buffer] ──► (Consume) ──► [Done]
   ●
```

### Markings and State

A **marking** is the distribution of tokens across all places at a given moment. The initial marking M₀ represents the starting state. The **reachability set** R(M₀) contains all markings reachable from M₀ through sequences of transition firings.

### Variants

| Variant | Extension |
|---------|-----------|
| **Place/Transition nets** | Basic Petri nets with integer token counts |
| **Colored Petri nets** | Tokens carry data values (colors) |
| **Timed Petri nets** | Transitions/places have time constraints |
| **Workflow nets (WF-nets)** | Special structure for business processes |
| **Free-choice nets** | Simplified conflict resolution |

---

## 2. Key Properties and Verification

### Structural Properties (independent of initial marking)

| Property | Definition | Significance |
|----------|------------|--------------|
| **Siphon** | A set of places S such that every transition with output in S also has input in S | Once emptied, stays empty forever |
| **Trap** | A set of places T such that every transition with input in T also has output in T | Once marked, stays marked forever |

### Behavioral Properties (depend on initial marking)

| Property | Definition | Verification Approach |
|----------|------------|----------------------|
| **Reachability** | Can marking M be reached from M₀? | State space exploration or structural analysis |
| **Boundedness** | Is there a maximum number of tokens in any place? | Coverability graph or place invariants |
| **Safeness** | Is every place 1-bounded (at most 1 token)? | Special case of boundedness |
| **Liveness** | Can every transition eventually fire from any reachable marking? | State space or siphon analysis |
| **Deadlock-freedom** | Is there always at least one enabled transition? | Siphon analysis or state space |
| **Reversibility** | Can M₀ be reached from any reachable marking? | State space analysis |

### Soundness for Workflow Nets

A workflow net is **sound** if:
1. **Option to complete**: From any reachable state, the final state can be reached
2. **Proper completion**: When the final state is reached, no tokens remain elsewhere
3. **No dead transitions**: Every transition can fire in at least one execution

**Key result**: For free-choice workflow nets, soundness can be verified in polynomial time.

---

## 3. Verification Algorithms

### 3.1 State Space Exploration (Reachability Graph)

**Approach**: Enumerate all reachable markings as a directed graph.

**Complexity**: The reachability problem for general Petri nets is:
- EXPSPACE-hard (Lipton, 1976)
- Decidable but not elementary (Czerwinski et al., 2018)
- Ackermann-complete (Leroux, 2021; Czerwinski & Orlikowski, 2021)

**State explosion problem**: The number of states grows exponentially with system size.

**Mitigations**:
- Partial order reduction
- Symmetry reduction
- Symbolic representation (BDDs)
- Unfolding techniques

### 3.2 Siphon-Based Analysis

**Key insight**: A Petri net is deadlock-free if all minimal siphons are "controlled" (contain a marked trap).

**Algorithm for siphon enumeration**:
1. Find all minimal siphons using depth-first search or SAT solving
2. For each siphon, check if it contains a marked trap
3. If an unmarked siphon exists with no trap, deadlock is possible

**Complexity**:
- Finding *a* siphon: Polynomial
- Finding *all minimal* siphons: NP-complete (can be exponential)

**Elementary siphon theory** (Li & Zhou): The number of elementary siphons is bounded by min(|P|, |T|), where P is places and T is transitions. Dependent siphons can be derived from elementary ones.

### 3.3 Structural Analysis (Invariants)

**Place invariants (P-invariants)**: Linear combinations of places whose weighted token count remains constant.

**Transition invariants (T-invariants)**: Sequences of transitions that return to the initial marking.

**Matrix equation approach**:
- Incidence matrix C where C[p,t] = output - input tokens
- P-invariants: y such that y^T * C = 0
- T-invariants: x such that C * x = 0

These provide necessary (but not sufficient) conditions for various properties.

### 3.4 Coverability Graph

For unbounded nets, construct a finite **coverability graph** by replacing unbounded places with ω (infinity). This preserves:
- Boundedness information
- Coverability (whether a marking is coverable)

But loses:
- Precise reachability
- Liveness information

### 3.5 Workflow Net Verification

For **free-choice workflow nets**, use the Rank Theorem:
- A free-choice WF-net is sound iff every proper siphon contains an initially marked trap

**Tools**: Woflan, WoPeD, and ProM implement these checks.

---

## 4. Existing Implementations

### 4.1 Rust Crates

| Crate | Description | Features |
|-------|-------------|----------|
| **[pnets](https://docs.rs/pnets)** | LAAS/CNRS library | Standard and timed nets, PNML support |
| **[pns](https://crates.io/crates/pns)** | C bindings | Minimal simulator, fast |
| **[petri-net-simulation](https://crates.io/crates/petri-net-simulation)** | Higher-level API | Built on pns, game/story modeling |
| **[ptnet-core](https://lib.rs/crates/ptnet-core)** | Trait-based | Place/Transition net modeling |
| **[cpnets](https://crates.io/crates/cpnets)** | Colored Petri nets | Token data values |
| **[netcrab](https://github.com/hlisdero/netcrab)** | Full tool suite | Creating, visualizing, analyzing |

**Recommendation**: `pnets` from LAAS/CNRS is the most comprehensive for verification work, with PNML import/export. `netcrab` provides visualization capabilities.

### 4.2 Python Libraries

| Library | Focus | Key Features |
|---------|-------|--------------|
| **[PM4Py](https://pm4py.fit.fraunhofer.de/)** | Process mining | Alpha/Inductive miners, BPMN/PNML, WF-net checking |
| **[SNAKES](https://github.com/fpom/snakes)** | General purpose | Python-colored nets, plugins, state space, ABCD algebra |

**PM4Py example**:
```python
from pm4py.objects.petri_net.obj import PetriNet, Marking
from pm4py.objects.petri_net.utils import petri_utils
from pm4py.algo.analysis.woflan import algorithm as woflan

# Check if a net is a sound workflow net
is_sound = woflan.apply(net, initial_marking, final_marking)
```

**SNAKES example**:
```python
from snakes.nets import *

n = PetriNet('example')
n.add_place(Place('p1', [Token()]))
n.add_transition(Transition('t1'))
n.add_input('p1', 't1', Value(1))
```

### 4.3 Java Tools

| Tool | Description | Verification |
|------|-------------|--------------|
| **[PIPE](https://pipe2.sourceforge.net/)** | Platform Independent Petri Net Editor | State space, siphons/traps, GSPN analysis |
| **[PIPE 5](https://github.com/sarahtattersall/PIPE)** | Rewrite (beta) | Missing analysis modules |

**PIPE capabilities**:
- Graphical editor with PNML support
- State space analysis (boundedness, deadlock-freedom, safeness)
- Siphon and trap enumeration
- Shortest path to deadlock
- Module system for custom analysis

### 4.4 Command-Line Model Checkers

#### LoLA (Low Level Petri Net Analyzer)

**Source**: [University of Rostock](https://theo.informatik.uni-rostock.de/theo-forschung/tools/lola/)

**Features**:
- Standard properties: liveness, reversibility, boundedness, reachability, deadlocks
- CTL and LTL model checking
- State space reduction: stubborn sets, symmetry, sweep-line
- ASCII file input, command-line interface
- GNU AGPL licensed

**Example usage**:
```bash
# Check for deadlock freedom
lola mynet.lola --formula="DEADLOCK"

# Check reachability
lola mynet.lola --formula="REACHABLE (p1 > 0)"

# Check CTL property
lola mynet.lola --formula="AG(p1 > 0 -> AF(p2 > 0))"
```

**LoLA input format**:
```
PLACE p1, p2, p3;
MARKING p1: 1;
TRANSITION t1
  CONSUME p1: 1;
  PRODUCE p2: 1;
TRANSITION t2
  CONSUME p2: 1;
  PRODUCE p3: 1;
```

#### Other Tools

| Tool | Features |
|------|----------|
| **[ITS-Tools](https://lip6.github.io/ITSTools-web/)** | Symbolic model checking, LTL/CTL |
| **[Neco](https://github.com/fpom/neco)** | Compiles SNAKES nets for efficient analysis |
| **[TAPAAL](http://www.tapaal.net/)** | Timed-arc Petri nets |

---

## 5. Mapping Workflows to Petri Nets

### 5.1 Task Dependency Graph to Petri Net

Given a JSONL workflow format like:
```jsonl
{"id": "task-1", "type": "task", "props": {"title": "Design API"}}
{"id": "task-2", "type": "task", "props": {"title": "Implement API"}}
{"from": "task-2", "to": "task-1", "rel": "blocks", "weight": 1.0}
```

**Basic transformation**:

| Workflow Element | Petri Net Element |
|------------------|-------------------|
| Task | Transition |
| Task state (pending/done) | Places |
| Dependency edge | Arc through intermediate place |
| Resource | Place with tokens |
| Actor assignment | Input arc from resource place |

**Transformation pattern**:

```
Workflow:  task-1 ─blocks─► task-2

Petri net:
                  [task-1      ]   [task-2      ]
[task-1-ready] ──►(task-1-exec)──►[task-1-done]──►(task-2-exec)──►[task-2-done]
       ●
```

### 5.2 Van der Aalst's Workflow Patterns

The seminal work by W.M.P. van der Aalst defines standard mappings:

**Sequence**:
```
[A-ready] ──► (A) ──► [B-ready] ──► (B) ──► [B-done]
```

**Parallel (AND-split/AND-join)**:
```
              ┌──► [B-ready] ──► (B) ──┐
[A-done] ──► (split)                  (join) ──► [D-ready]
              └──► [C-ready] ──► (C) ──┘
```

**Choice (XOR-split/XOR-join)**:
```
              ┌──► [B-ready] ──► (B) ──┐
[A-done] ──► (A')                     ──► [D-ready]
              └──► [C-ready] ──► (C) ──┘
```
(Note: A' has two output arcs to different places, non-deterministic choice)

**Conditional (guarded)**:
```
[A-done] + [condition] ──► (B-enabled) ──► ...
```

### 5.3 Resource Constraints

For limited resources (e.g., only 2 engineers available):
```
[engineers: ●●] ◄──┐
       │           │
       └──► (task) ─┘
```
The task consumes a token (engineer) and returns it upon completion.

### 5.4 Practical JSONL to Petri Net Mapping

**Algorithm**:
```
1. For each task T:
   - Create place T_ready, T_done
   - Create transition T_exec
   - Add arc T_ready → T_exec → T_done

2. For each dependency (from: A, to: B, rel: "blocks"):
   - Add arc A_done → B_exec (B needs A to be done)

3. For each resource R with capacity C:
   - Create place R_pool with C tokens
   - For tasks requiring R:
     - Add arc R_pool → T_exec → R_pool (borrow and return)

4. Initial marking:
   - Place one token in T_ready for each initially ready task
   - Place C tokens in each R_pool

5. Final marking:
   - All T_done places have tokens
```

---

## 6. Lightweight Verification Approaches

For a CLI tool like `wg check`, full model checking is often overkill. Here are simpler approaches:

### 6.1 Topological Sort for Deadlock Detection

**Key insight**: A task dependency graph is a DAG if and only if it can be topologically sorted. A cycle indicates a deadlock.

**Algorithm** (Kahn's algorithm):
```python
def detect_deadlock(tasks, dependencies):
    # Build adjacency list and in-degree count
    in_degree = {t: 0 for t in tasks}
    successors = {t: [] for t in tasks}

    for dep in dependencies:
        if dep.rel == "blocks":
            successors[dep.to].append(dep.from_)
            in_degree[dep.from_] += 1

    # Start with tasks that have no blockers
    queue = [t for t in tasks if in_degree[t] == 0]
    sorted_tasks = []

    while queue:
        task = queue.pop(0)
        sorted_tasks.append(task)
        for successor in successors[task]:
            in_degree[successor] -= 1
            if in_degree[successor] == 0:
                queue.append(successor)

    if len(sorted_tasks) != len(tasks):
        # Cycle detected - find tasks in the cycle
        cycle_tasks = [t for t in tasks if in_degree[t] > 0]
        return CycleDetected(cycle_tasks)

    return NoCycle(sorted_tasks)
```

**Complexity**: O(V + E) - linear in tasks and dependencies.

### 6.2 Wait-For Graph Analysis

For resource allocation deadlocks, build a wait-for graph:

**Algorithm**:
```python
def detect_resource_deadlock(tasks, resources, assignments):
    # Build wait-for graph
    # Edge A → B means: A is waiting for a resource held by B
    wait_for = defaultdict(set)

    for task in running_tasks:
        for needed_resource in task.waiting_for:
            holder = resource_holder[needed_resource]
            if holder and holder != task:
                wait_for[task].add(holder)

    # Detect cycle using DFS
    return find_cycle(wait_for)
```

**Complexity**: O(n^2) in worst case, O(n + m) typical.

### 6.3 Simple Siphon Check for Static Analysis

**Simplified siphon detection** for workflow graphs:

```python
def find_potential_deadlocks(net):
    """
    Find sets of tasks that could collectively get stuck.
    A siphon is a set S where:
    - If all tasks in S are waiting for something,
    - They're all waiting for things in S
    """
    deadlock_candidates = []

    for subset in subsets_of_tasks(net):  # Can limit to small subsets
        # Check if subset forms a siphon
        all_deps_internal = True
        for task in subset:
            for dep in task.blockers:
                if dep not in subset:
                    all_deps_internal = False
                    break

        if all_deps_internal and subset:
            # Check if any task in subset can make progress
            can_progress = any(
                all(b.status == 'done' for b in t.blockers)
                for t in subset
            )
            if not can_progress:
                deadlock_candidates.append(subset)

    return deadlock_candidates
```

### 6.4 Reachability via Forward Simulation

**Simple simulation-based check**:

```python
def check_completion_possible(workflow, max_steps=10000):
    """
    Simulate execution to check if workflow can complete.
    Returns: Completed, Deadlocked, or Inconclusive
    """
    state = initial_state(workflow)

    for _ in range(max_steps):
        if is_complete(state):
            return Completed(state.trace)

        enabled = get_enabled_tasks(state)
        if not enabled:
            return Deadlocked(state)

        # Pick a random enabled task (or try all for exhaustive)
        task = random.choice(enabled)
        state = fire(state, task)

    return Inconclusive()
```

### 6.5 Structural Checks Without Full Analysis

**Quick structural sanity checks**:

```python
def structural_checks(workflow):
    issues = []

    # 1. Check for orphan tasks (no path from start)
    reachable = bfs_from_start(workflow)
    orphans = [t for t in workflow.tasks if t not in reachable]
    if orphans:
        issues.append(f"Orphan tasks: {orphans}")

    # 2. Check for tasks that can't reach end
    can_reach_end = reverse_bfs_from_end(workflow)
    dead_ends = [t for t in workflow.tasks if t not in can_reach_end]
    if dead_ends:
        issues.append(f"Dead-end tasks: {dead_ends}")

    # 3. Check for missing dependencies
    for task in workflow.tasks:
        if task.requires and not task.blockers:
            issues.append(f"Task {task} requires resources but has no blockers")

    # 4. Check resource over-allocation
    for resource in workflow.resources:
        max_concurrent = max_concurrent_users(workflow, resource)
        if max_concurrent > resource.capacity:
            issues.append(f"Resource {resource} over-allocated")

    return issues
```

---

## 7. Recommendations for workgraph

### 7.1 Tiered Verification Strategy

| Tier | Check | Complexity | When to Run |
|------|-------|------------|-------------|
| 1 | Cycle detection (topological sort) | O(V+E) | Every `wg check` |
| 2 | Structural checks (orphans, dead-ends) | O(V+E) | Every `wg check` |
| 3 | Resource bound checking | O(V*R) | On resource changes |
| 4 | Simulation-based reachability | O(states) | `wg check --deep` |
| 5 | Full Petri net analysis | Exponential | `wg verify --exhaustive` |

### 7.2 Suggested Implementation

**Phase 1: Lightweight checks (implement first)**
```rust
// In Rust
pub fn check_workflow(wg: &workgraph) -> Vec<Issue> {
    let mut issues = vec![];

    // Tier 1: Cycle detection
    if let Some(cycle) = detect_cycle(&wg.tasks, &wg.dependencies) {
        issues.push(Issue::Cycle(cycle));
    }

    // Tier 2: Structural checks
    issues.extend(check_reachability(&wg));
    issues.extend(check_resource_bounds(&wg));

    issues
}
```

**Phase 2: Optional deep verification**
```rust
// Export to LoLA format for full model checking
pub fn export_lola(wg: &workgraph) -> String {
    let mut out = String::new();

    // Define places
    out.push_str("PLACE ");
    for task in &wg.tasks {
        out.push_str(&format!("{}_ready, {}_done, ", task.id, task.id));
    }
    out.push_str(";\n");

    // Define initial marking
    out.push_str("MARKING ");
    for task in wg.ready_tasks() {
        out.push_str(&format!("{}_ready: 1, ", task.id));
    }
    out.push_str(";\n");

    // Define transitions
    for task in &wg.tasks {
        out.push_str(&format!("TRANSITION {}\n", task.id));
        out.push_str(&format!("  CONSUME {}_ready: 1;\n", task.id));
        out.push_str(&format!("  PRODUCE {}_done: 1;\n", task.id));
    }

    out
}
```

### 7.3 Rust Library Recommendation

For the workgraph project, consider:

1. **Build lightweight checks in pure Rust** (no dependencies) for Tier 1-3
2. **Use `pnets` crate** for Petri net representation if needed
3. **Shell out to LoLA** for exhaustive verification (Tier 5)

**Minimal dependency approach**:
```rust
// Pure Rust, no external crates needed
pub mod verify {
    pub fn topological_sort(tasks: &[Task], deps: &[Dep]) -> Result<Vec<&Task>, Cycle>;
    pub fn find_orphans(tasks: &[Task], deps: &[Dep]) -> Vec<&Task>;
    pub fn check_resource_bounds(tasks: &[Task], resources: &[Resource]) -> Vec<Issue>;
}
```

### 7.4 Complexity Trade-offs

| Approach | Soundness | Completeness | Complexity |
|----------|-----------|--------------|------------|
| Topological sort | Detects all cycles | Only simple deadlocks | O(V+E) |
| Simulation | May miss paths | Finds one completion | O(steps) |
| Siphon analysis | Sound for structural deadlock | Not complete | O(2^n) worst |
| Full state space | Complete | Sound | O(2^n) or worse |

**Recommendation**: Start with topological sort + structural checks. Add simulation for `--deep` mode. Reserve full Petri net analysis for `--exhaustive` or export to external tools.

---

## References

### Academic

- van der Aalst, W.M.P. (1998). "The Application of Petri Nets to Workflow Management" - [Paper](https://users.cs.northwestern.edu/~robby/courses/395-495-2017-winter/Van%20Der%20Aalst%201998%20The%20Application%20of%20Petri%20Nets%20to%20Workflow%20Management.pdf)
- Hou, Y. & Barkaoui, K. (2017). "Deadlock analysis and control based on Petri nets: A siphon approach review" - [SAGE Journals](https://journals.sagepub.com/doi/10.1177/1687814017693542)
- Wolf, K. (2018). "Petri Net Model Checking with LoLA 2" - [SpringerLink](https://link.springer.com/chapter/10.1007/978-3-319-91268-4_18)

### Tools

- LoLA Model Checker - [University of Rostock](https://theo.informatik.uni-rostock.de/theo-forschung/tools/lola/)
- PIPE (Platform Independent Petri Net Editor) - [SourceForge](https://pipe2.sourceforge.net/) | [GitHub](https://github.com/sarahtattersall/PIPE)
- PM4Py - [Documentation](https://pm4py-source.readthedocs.io/)
- SNAKES - [HAL](https://hal.science/hal-01186407/document)
- pnets (Rust) - [docs.rs](https://docs.rs/pnets)
- netcrab (Rust) - [GitHub](https://github.com/hlisdero/netcrab)

### Further Reading

- Hoare, C.A.R. (1985). "Communicating Sequential Processes"
- Malone, T.W. & Crowston, K. (1994). "The Interdisciplinary Study of Coordination" - [MIT](http://ccs.mit.edu/papers/ccswp157.html)
- Jensen, K. "Coloured Petri Nets" - For weighted/typed nets
