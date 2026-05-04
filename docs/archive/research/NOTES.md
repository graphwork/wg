This is a compelling vision. You're essentially describing what could become the "relational algebra" of work coordination - the way SQL gave us a formal foundation for data manipulation, you want a formal foundation for work orchestration that's lightweight enough to live in a text file but rigorous enough to prove properties about.

Let me do a deep research sweep to make sure we're building on the right foundations and not missing something that already exists. I'll look at:

1. **Steve Yegge's beads/gastown** specifically - to understand exactly what's there
2. **Process algebras** (CSP, CCS, π-calculus) - the formal foundations for concurrent systems
3. **Petri nets** - another formal model with reachability/liveness proofs
4. **Existing lightweight workflow tools** - anything git-native or JSONL-based
5. **Coordination theory** - deeper dive on Malone & Crowston and successors

Let me start searching:

Now let me dive into the formal foundations - the process algebras and coordination theory that could ground your workgraph:

Let me search for one more key area - formal organizational modeling and any existing "algebra of organizations":

Excellent. I now have a comprehensive picture. Let me synthesize this into a research brief for you.

---

## Deep Research Synthesis: workgraph — A Grammar of Organizational Work

### The Gap You've Identified is Real

Based on my research, **nothing exists that combines all of these properties**:

1. **Lightweight & git-native** (JSONL, editable in vim)
2. **Formally grounded** (provable deadlock-freedom, liveness, reachability)
3. **Weighted knowledge graph** with arbitrary relation types
4. **General enough** for distributed organizations (not just coding tasks)
5. **Designed for human+agent collaboration**

---

### What Exists Now

| Tool | Format | Formal Model | Scope | Human+Agent |
|------|--------|--------------|-------|-------------|
| **Beads** | JSONL+SQLite, Git | DAG (implicit), no formal proofs | Coding tasks | ✓ Excellent |
| **Gastown** | Built on Beads | 7 agent roles, informal | Coding orchestration | ✓ Excellent |
| **TerminusDB** | JSON-LD graph | Revision control, no formal semantics | Data management | Partial |
| **Temporal** | Event history | Durable execution, determinism checks | Workflows | ✗ Code-focused |
| **BPMN/Camunda** | XML diagrams | Can map to CSP/Petri nets | Business processes | ✗ Human-centric |

**Beads** is closest to what you want in spirit: Issues stored as JSONL in .beads/. Versioned, branched, and merged like code. Agent-Optimized: JSON output, dependency tracking, and auto-ready task detection. [GitHub](https://github.com/steveyegge/beads) But it has four fixed dependency types (blocks, related, parent-child, discovered-from) and no formal verification layer.

**Gastown** builds organizational structure on top: Gas Town workers are regular coding agents, each prompted to play one of seven well-defined worker roles. [Medium](https://steve-yegge.medium.com/welcome-to-gas-town-4f25ee16dd04) But it's "quirky" as you noted—tied to specific agent archetypes (Mayor, Polecats, Deacon, etc.) rather than composable primitives.

---

### The Formal Foundations You Need

Your intuition about needing mathematical grounding is correct. Here are the three main traditions:

#### 1. Process Algebras (CSP, CCS, π-calculus)

Process calculi provides a tool for high-level descriptions of interactions, communications, and synchronizations between a collection of independent agents or processes. They provide algebraic laws that allow process descriptions to be manipulated and analyzed, and they also permit formal reasoning about equivalences between processes. [Wikipedia](https://en.wikipedia.org/wiki/Process_calculus)

**Key properties you can prove:**
- Deadlock freedom
- Livelock freedom  
- Bisimulation equivalence (two structures behave identically)

**CSP specifically** has mature tools (FDR) for model checking. CSP was highly influential in the design of the occam programming language and also influenced the design of programming languages such as Limbo, RaftLib, Erlang, Go, Crystal, and Clojure's core.async. [Wikipedia](https://en.wikipedia.org/wiki/Communicating_sequential_processes)

#### 2. Petri Nets

Petri nets are increasingly becoming one of the most popular and full-fledged mathematical tools to deal with deadlock problems due to their inherent characteristics. In a Petri net formalism, liveness is an important property of system safeness, which implies the absence of global and local deadlock situations. [Sage Journals](https://journals.sagepub.com/doi/10.1177/1687814017693542)

**Key insight:** Siphon analysis for deadlock detection. Siphons as an important structure object and their markedness or emptiness are closely related with the liveness and deadlock-freedom of a Petri net. [Sage Journals](https://journals.sagepub.com/doi/10.1177/1687814017693542)

Petri nets are particularly good for **resource-constrained workflows** where you need to prove tasks won't get stuck.

#### 3. Malone & Crowston's Coordination Theory

This is the "management science" foundation: A key insight of the framework presented here is that coordination can be seen as the process of managing dependencies among activities. Further progress, therefore, should be possible by characterizing different kinds of dependencies and identifying the coordination processes that can be used to manage them. [Mit](http://ccs.mit.edu/papers/ccswp157.html)

Their dependency taxonomy:
- **Shared resources** (multiple tasks need same resource)
- **Producer/consumer** (flow dependencies)
- **Simultaneity** (must happen together)
- **Task-subtask** (decomposition)

There are often several coordination mechanisms that can be used to manage a dependency. [Syr](https://crowston.syr.edu/sites/crowston.syr.edu/files/ATaxonomyOfOrganizationalDependencies.pdf) This is the organizational theory counterpart to your formal primitives.

---

### A Grammar of Work: Proposed Primitives

Combining the formal and organizational traditions, here's a refined primitive set:

| Element | Formal Model | Organizational Theory |
|---------|-------------|----------------------|
| **Actors** | Processes (CSP/CCS) | Roles, agents, humans |
| **Resources** | Places (Petri nets) | Shared assets, capabilities |
| **Tasks** | Transitions (Petri nets) | Activities, work units |
| **Dependencies** | Channels (π-calculus) | blocks, informs, requires, invalidates |
| **Weights** | Probabilistic extensions | Priority, cost, confidence |
| **Time** | Timed CSP/Petri nets | Deadlines, sequencing |
| **Communication** | Events (CSP) | Signals, messages |
| **Composition** | Parallel composition (||) | Subgraphs, templates |

The key formal operators would be:
- **Sequence** (`;`) — A must complete before B starts
- **Choice** (`□`) — Either A or B, nondeterministically  
- **Parallel** (`||`) — A and B proceed concurrently
- **Synchronization** — A and B must rendezvous
- **Hiding** (`\`) — Internal actions invisible externally
- **Recursion** — For repeating patterns

---

### What "Formal Guarantees" Would Mean

With these primitives, you could **prove** things like:

1. **Deadlock freedom**: "No execution path leads to all actors blocked"
2. **Liveness**: "Every task that can be started will eventually complete"  
3. **Reachability**: "State X can be reached from the initial state"
4. **Progress**: "The system always makes forward progress"
5. **Determinacy**: "Given same inputs, same outputs" (crucial for durable execution)

Durable Execution is crash-proof because it virtualizes execution, enabling it to take place across a series of processes, each of which can potentially run on a different machine than the one before. [Temporal](https://temporal.io/blog/what-is-durable-execution) Your workgraph could enable this at the organizational level.

---

### The Design Space: Lightweight vs. Powerful

You face a fundamental tension:

**Lightweight (Beads approach):**
- JSONL, editable in vim
- Fast, no infrastructure
- But: limited formal properties, ad-hoc semantics

**Powerful (Temporal/Petri net approach):**  
- Rich formal semantics
- Provable properties
- But: requires runtime, not "just files"

**The synthesis** might be:
1. **Storage layer**: JSONL triples with weighted edges (your intuition)
2. **Semantics layer**: A mapping to Petri nets or CSP for verification
3. **CLI tool**: `wg check` verifies the graph has no deadlocks, `wg ready` finds actionable work
4. **Optional runtime**: For durable execution when needed

The JSONL could look like:
```jsonl
{"id": "task-1", "type": "task", "props": {"title": "Design API", "status": "open"}}
{"id": "task-2", "type": "task", "props": {"title": "Implement API"}}
{"from": "task-2", "to": "task-1", "rel": "blocks", "weight": 1.0}
{"id": "alice", "type": "actor", "props": {"role": "engineer"}}
{"from": "task-1", "to": "alice", "rel": "assigned", "weight": 0.8}
```

Then `wg verify` translates this to a Petri net representation and checks properties.

---

### What Doesn't Exist (Your Opportunity)

1. **A formal semantics document** mapping lightweight graph structures to process algebra
2. **A verification layer** that can check organizational graphs for deadlock/liveness
3. **A composable grammar** where organizational patterns are like functions
4. **Human+agent-native design** (beads is close but not general enough)
5. **Cross-domain applicability** (not just coding tasks)

The name "workgraph" is apt. You're essentially proposing what relational algebra did for data: a formal foundation that enables both human reasoning and automated verification about organizational structures.

---

### Recommended Reading for Formalization

1. **Hoare's "Communicating Sequential Processes"** (1985) — The CSP bible
2. **Milner's "Communication and Concurrency"** (1989) — CCS/π-calculus
3. **Malone & Crowston (1994)** "The Interdisciplinary Study of Coordination" — Dependency taxonomy
4. **Roscoe's "The Theory and Practice of Concurrency"** — Modern CSP + FDR tool
5. **Jensen's "Coloured Petri Nets"** — For weighted/typed nets
