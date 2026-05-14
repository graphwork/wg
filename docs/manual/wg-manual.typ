// WG Manual
// A conceptual guide to task coordination for humans and AI agents

#set document(
  title: "WG: A Manual",
  author: "The WG Project",
)

#set text(font: "New Computer Modern", size: 11pt)
#set par(justify: true)
#set heading(numbering: "1.1")

// Title page
#page(numbering: none)[
  #v(4fr)
  #align(center)[
    #text(size: 32pt, weight: "bold")[wg]
    #v(8pt)
    #text(size: 16pt)[A Manual]
    #v(24pt)
    #text(size: 12pt, style: "italic")[
      Task coordination for humans and AI agents
    ]
  ]
  #v(6fr)
]

// Table of contents
#page(numbering: none)[
  #outline(title: "Contents", depth: 2, indent: auto)
]

// Start page numbering
#set page(numbering: "1")
#counter(page).update(1)

// ──────────────────────────────────────────────────
// Glossary
// ──────────────────────────────────────────────────

= Glossary <glossary>

The following terms have precise meanings throughout this manual. They are defined here for reference and used consistently in every section.

#table(
  columns: (auto, 1fr),
  align: (left, left),
  stroke: 0.5pt,
  inset: 8pt,
  table.header([*Term*], [*Definition*]),
  [*task*], [The fundamental unit of work. Has an ID, title, status, and may have dependencies, skills, inputs, deliverables, and other metadata. Tasks are nodes in the graph.],
  [*status*], [The lifecycle state of a task. One of eight values: _open_ (available for work), _in-progress_ (claimed by an agent), _done_ (completed successfully), _failed_ (attempted and failed; retryable), _abandoned_ (permanently dropped), _blocked_ (explicit, rarely used), _pending-validation_ (awaiting review after `wg done` on a task with `verify` criteria), or _waiting_ (parked by `wg wait` until a condition is met). The three _terminal_ statuses are done, failed, and abandoned—a terminal task no longer blocks its dependents. PendingValidation and Waiting are non-terminal intermediate states.],
  [*dependency*], [A directed edge between tasks expressed via the `after` field. Task B depends on task A means B cannot be ready until A reaches a terminal status.],
  [*after*], [The authoritative dependency list on a task. `task.after = ["dep"]` means the task comes after `dep`. A task is _waiting_ (in the derived sense) when any entry in its `after` list is non-terminal. In the CLI, specified via `--after` (alias: `--blocked-by`).],
  [*before*], [The computed inverse of `after`, maintained for bidirectional traversal. If B is after A, then A's `before` list includes B. Not checked by the scheduler—purely a convenience index.],
  [*ready*], [A task is _ready_ when it is open, not paused, past any time constraints, and every task in its `after` list is terminal. For cycle headers, back-edge predecessors are exempt.],
  [*structural cycle*], [A cycle formed by `after` edges, detected automatically by Tarjan's SCC algorithm. Each cycle has a header (entry point) with a `CycleConfig` controlling iteration. Replaces the former `loops_to` edge system.],
  [*CycleConfig*], [Configuration for cycle iteration, stored on the cycle header task. Fields: `max_iterations` (hard cap), `guard` (optional condition), `delay` (optional pacing between iterations).],
  [*guard*], [A condition on a cycle's `CycleConfig`. Three kinds: _Always_, _TaskStatus_, and _IterationLessThan_.],
  [*loop iteration*], [A counter tracking how many times a task has been re-activated by cycle iteration.],
  [*visibility*], [A field on each task controlling what information crosses organizational boundaries during trace exports. Three values: _internal_ (default, org-only), _public_ (sanitized sharing—task structure without agent output or logs), _peer_ (richer detail for trusted peers—includes evaluations but strips notes and detailed logs). Set via `wg add --visibility` or `wg edit`.],
  [*convergence*], [An agent-driven signal (`wg done --converged`) indicating that a cycle's iterative work has reached a stable state. Adds a `"converged"` tag to the cycle header (regardless of which member the agent completes). When the header carries this tag, the cycle does not iterate—even if iterations remain and guards are satisfied. Cleared on retry.],
  [*trace*], [The operations log (`operations.jsonl`) recording every mutation to the graph. The project's organizational memory—queryable via `wg trace`, exportable with visibility filtering, and importable from peers.],
  [*trace export*], [A filtered, shareable snapshot of the trace. Visibility filtering controls what is included: _internal_ exports everything, _public_ sanitizes, _peer_ provides richer detail for trusted peers. Produced by `wg trace export --visibility <zone>`.],
  [*function*], [A parameterized workflow template extracted from completed traces via `wg func extract`. Captures task structure, dependencies, and structural cycles. Applied via `wg func apply` to create new task graphs. Stored as YAML in `.wg/functions/`.],
  [*replay*], [Re-execution of previously completed or failed work. `wg replay` creates an immutable snapshot, then selectively resets tasks based on criteria. Supports `--plan-only` for previewing.],
  [*role*], [An agency entity defining _what_ an agent does. Contains a description, skills, and a desired outcome. Identified by a content-hash of its identity-defining fields.],
  [*motivation (tradeoff)*], [An agency entity defining _why_ an agent acts the way it does. Contains a description, acceptable trade-offs, and unacceptable trade-offs. Identified by a content-hash of its identity-defining fields. Called _tradeoff_ in the CLI (`wg tradeoff`); the older name _motivation_ is accepted as an alias.],
  [*agent*], [The unified identity in the agency system—a named pairing of a role and a motivation. Identified by a content-hash of `(role_id, motivation_id)`.],
  [*agency*], [The collective system of roles, motivations, and agents. Also refers to the storage directory (`.wg/agency/`).],
  [*content-hash ID*], [A SHA-256 hash of an entity's identity-defining fields. Deterministic, deduplicating, and immutable. Displayed as 8-character hex prefixes.],
  [*capability*], [A flat string tag on an agent used for task-to-agent matching at dispatch time. Distinct from role skills: capabilities are for _routing_, skills are for _prompt injection_.],
  [*skill*], [A capability reference attached to a role. Four types: _Name_, _File_, _Url_, _Inline_. Resolved at dispatch time and injected into the prompt.],
  [*trust level*], [A classification on an agent: _verified_, _provisional_ (default), or _unknown_. Verified agents receive a small scoring bonus in task matching.],
  [*executor*], [The backend that runs an agent's work. Built-in: _claude_ (AI), _shell_ (automated command). Custom executors can be defined as TOML files.],
  [*coordinator*], [The scheduling brain inside the service daemon. Runs a tick loop that finds ready tasks and spawns agents.],
  [*service daemon*], [The background process started by `wg service start`. Hosts the coordinator, listens on a Unix socket for IPC, and manages agent lifecycle.],
  [*tick*], [One iteration of the coordinator loop. Triggered by IPC or a safety-net poll timer.],
  [*dispatch*], [The full cycle of selecting a ready task and spawning an agent: claim + spawn + register.],
  [*claim*], [Marking a task as _in-progress_ and recording who is working on it. Distinct from _assignment_—claiming sets execution state.],
  [*assignment*], [Binding an agency agent identity to a task. Sets identity, not execution state.],
  [*auto-assign*], [A coordinator feature that creates `assign-{task-id}` meta-tasks for unassigned ready work.],
  [*auto-evaluate*], [A coordinator feature that creates `evaluate-{task-id}` meta-tasks for completed work.],
  [*evaluation*], [A scored assessment of an agent's work. Four dimensions: correctness (40%), completeness (30%), efficiency (15%), style adherence (15%). Scores propagate to the agent, its role, and its motivation.],
  [*evaluation source*], [A freeform string tag on each evaluation identifying its origin. Default: `"llm"` (internal auto-evaluator). Conventions: `"outcome:<metric>"` for external outcome data, `"ci:<suite>"` for CI results, `"vx:<peer-id>"` for peer evaluations. The evolver reads all evaluations regardless of source.],
  [*performance record*], [A running tally on each agent, role, and motivation: task count, average score, and evaluation references with context IDs.],
  [*evolution*], [The process of improving agency entities based on evaluation data. Triggered manually via `wg evolve`.],
  [*strategy*], [An evolution approach: _mutation_, _crossover_, _gap analysis_, _retirement_, _tradeoff tuning_, or _all_.],
  [*lineage*], [Evolutionary history on every role, motivation, and agent. Records parent IDs, generation number, creator identity, and timestamp.],
  [*generation*], [Steps from a manually-created ancestor. Generation 0 = human-created. Each evolution increments by one.],
  [*synergy matrix*], [A performance cross-reference of every (role, motivation) pair, showing average score and evaluation count.],
  [*meta-task*], [A task created by the coordinator to manage the agency loop. Assignment, evaluation, and evolution review tasks are meta-tasks.],
  [*map/reduce pattern*], [An emergent workflow: fan-out (one task completes, enabling parallel children) and fan-in (parallel tasks must all complete before a single aggregator). Arises from dependency edges, not a built-in primitive.],
  [*triage*], [An LLM-based assessment of a dead agent's output, classifying the result as _done_, _continue_, or _restart_.],
  [*wrapper script*], [The `run.sh` generated for each spawned agent. Runs the executor, captures output, and handles post-exit fallback logic.],
  [*federation*], [The system for sharing agency entities across wg projects. Operations: _scan_ (discover), _pull_ (import), _push_ (export). Named remotes stored in `.wg/federation.yaml`. Content-hash IDs make deduplication automatic.],
  [*remote*], [A named reference to another wg project's agency store, used for federation. Managed via `wg agency remote add/list/remove`.],
  [*provider*], [The LLM provider backing a task or agent: `anthropic`, `openai`, `openrouter`, or `local`. Set per-task via `--provider` on `wg add`/`wg edit`, or per-agent via `wg config`. The coordinator resolves providers through the same priority chain as models.],
  [*exec-mode*], [Controls the execution weight of an agent dispatched for a task. Four values: _full_ (default—complete tool access), _light_ (read-only tools), _bare_ (only `wg` CLI), _shell_ (no LLM—runs the task's `exec` field directly). Set via `--exec-mode` on `wg add`/`wg edit`.],
  [*placement*], [The coordinator's automatic positioning of newly created tasks in the dependency graph. Controlled by placement hints: `--no-place` (skip placement—make the task immediately available), `--place-near <IDS>` (place near specified tasks), `--place-before <IDS>` (insert before specified tasks). Automatic placement is configured via `wg config --auto-place`.],
  [*multi-coordinator*], [Support for running multiple coordinator sessions within a single service daemon. Each coordinator manages an independent scheduling context. Managed via `wg service create-coordinator`, `stop-coordinator`, `archive-coordinator`, and `delete-coordinator`. The maximum number of concurrent coordinators is set via `wg config --max-coordinators`.],
  [*compaction*], [The process of distilling graph state into a condensed summary (`context.md`). Triggered via `wg compact`. In the service daemon, compaction runs as the `.compact-0` task—a structural cycle where the coordinator introspects its own state.],
  [*sweep*], [Detection and recovery of orphaned in-progress tasks whose agents have died. `wg sweep` scans for tasks claimed by agents whose PIDs no longer exist and offers to reclaim them.],
  [*checkpoint*], [A snapshot of an agent's progress during long-running tasks. `wg checkpoint` saves the current state so that if the agent is interrupted, a replacement can resume from the checkpoint rather than starting over.],
  [*event stream*], [A real-time feed of graph mutations produced by `wg watch`. Events are typed (`task.created`, `task.completed`, `evaluation.recorded`, etc.) and filterable by category or task ID. Enables external adapters to observe and react without polling.],
  [*adapter*], [An external tool that translates between an external system's vocabulary and wg's ingestion points. The generic pattern: observe (via `wg watch`) → translate → ingest (via `wg` CLI) → react. A conceptual pattern, not a formal type.],
  [*dispatch role*], [A named system function with its own model and provider assignment. Roles include _default_, _task\_agent_, _evaluator_, _assigner_, _evolver_, _triage_, _verification_, _compactor_, _placer_, and others. Managed via `wg model routing` and `wg model set`. Enables cost-optimized model allocation: cheap models for routine roles, capable models for complex work.],
  [*peer*], [A registered reference to another wg project for cross-repo communication. Managed via `wg peer add/remove/list/status`. Tasks can be created in a peer's graph via `wg add --repo <peer-name>`. Distinct from federation (which shares agency identities)---peer communication shares _work_.],
  [*agency import*], [Importing agency primitives (roles, motivations) from external sources via `wg agency import`. Supports local CSV files, remote URLs (`--url`), and configured upstream bureaus (`--upstream`). Change detection via manifest hashing prevents redundant imports.],
  [*user board*], [A per-user conversation surface (`.user-NAME` task) for persistent human-in-the-loop communication. Created via `wg user init`, listed with `wg user list`, and archived with `wg user archive`. Each board is a task with a message queue---the coordinator reopens it when new messages arrive, enabling asynchronous dialogue between the human and the system.],
  [*provider profile*], [A named preset that maps model tiers (haiku, sonnet, opus) to specific provider/model combinations. Managed via `wg profile set`, `wg profile show`, and `wg profile list`. Profiles simplify model configuration: instead of setting each dispatch role's model individually, activate a profile and all roles resolve through its tier mappings. `wg profile refresh` updates rankings from OpenRouter.],
  [*spend*], [Token usage and estimated cost tracking. `wg spend` summarizes cumulative input/output tokens, cached tokens, and estimated USD cost across all agents. `--today` filters to the current day. The data is derived from agent output logs that capture API usage metadata.],
  [*requeue*], [Returning an in-progress task to open status for re-dispatch. `wg requeue --reason "..."` resets the task and records the reason---typically used when a task's dependency has failed and fix tasks have been created, allowing the requeued task to be dispatched after the fixes land. Distinct from `wg retry` (which handles failed tasks) and dead-agent triage (which is automatic).],
)

#pagebreak()

// ──────────────────────────────────────────────────
// Section 1: System Overview
// ──────────────────────────────────────────────────

#include "01-overview.typ"

#pagebreak()

// ──────────────────────────────────────────────────
// Section 2: The Task Graph
// ──────────────────────────────────────────────────

#include "02-task-graph.typ"

#pagebreak()

// ──────────────────────────────────────────────────
// Section 3: The Agency Model
// ──────────────────────────────────────────────────

#include "03-agency.typ"

#pagebreak()

// ──────────────────────────────────────────────────
// Section 4: Coordination & Execution
// ──────────────────────────────────────────────────

#include "04-coordination.typ"

#pagebreak()

// ──────────────────────────────────────────────────
// Section 5: Evolution & Improvement
// ──────────────────────────────────────────────────

#include "05-evolution.typ"
