#set document(title: "workgraph: Task Coordination for Humans and AI Agents")
#set page(paper: "us-letter", margin: 1in)
#set text(font: "New Computer Modern", size: 11pt)
#set par(justify: true)
#set heading(numbering: "1.1")

#align(center)[
  #text(size: 24pt, weight: "bold")[workgraph]

  #text(size: 14pt)[Task Coordination for Humans and AI Agents]

  #v(1em)

  #text(style: "italic")[A lightweight, git-friendly system for managing work dependencies in hybrid human-AI teams]
]

#v(2em)

= Abstract

workgraph is a task coordination system designed for the emerging reality of hybrid teams---where human engineers, project managers, and AI agents collaborate on complex projects. Unlike traditional project management tools that assume human-only workflows, workgraph treats all actors uniformly, whether flesh or silicon. The system stores data in a simple JSONL format that is human-readable, git-friendly, and easily parsed by both scripts and language models. workgraph's dependency graph supports cycles for recurring tasks, provides rich analysis commands for identifying bottlenecks and forecasting completion, and implements a claim/unclaim protocol that enables safe parallel execution by multiple agents.

= Introduction

The coordination of work has traditionally been a human endeavor. Project managers create tasks, assign them to engineers, track dependencies, and forecast completion dates. This model breaks down when AI agents enter the picture.

Modern AI agents can perform substantial programming work: implementing features, writing tests, fixing bugs, and refactoring code. But they need to coordinate with humans and with each other. They need to know what work is available, claim tasks to avoid conflicts, and signal when work is complete. Traditional project management tools---designed for human reaction times and manual updates---are poorly suited to this reality.

workgraph addresses this gap. It provides a minimal but complete model for task coordination that works equally well for humans and agents. The system is deliberately simple: tasks are stored as JSON lines in a file, dependencies are expressed as explicit references, and all operations are atomic file updates that integrate cleanly with git workflows.

The design philosophy prioritizes machine-readability without sacrificing human usability. A human can open the graph file in any text editor and understand the project state. An agent can parse the same file, identify available work, and make updates. Both can use the `wg` command-line tool for common operations.

= Design Philosophy

workgraph's design follows several core principles:

*Simplicity over features.* The data model contains only what is necessary: tasks, actors, resources, and their relationships. There are no sprints, story points, epics, or other project management abstractions. These can be layered on top if needed, but the core remains minimal.

*Git-friendly storage.* All data lives in `.wg/graph.jsonl`---one JSON object per line. This format has important properties: files diff cleanly in git, merge conflicts are localized to specific lines, and the full history of project evolution is preserved in version control. There is no external database to synchronize.

*Human-readable, machine-parseable.* The JSON format is verbose enough for humans to read and edit directly, yet structured enough for any programming language to parse. AI agents can understand the graph without specialized tooling.

*Cycles are allowed.* Unlike traditional DAG-based task systems, workgraph permits cycles in the dependency graph. This enables modeling of recurring tasks, review-revision loops, and other patterns where work naturally cycles. The system classifies cycles as intentional (tagged with `recurring` or `cycle:intentional`) or potentially problematic, helping users distinguish design choices from bugs.

= Data Model

The workgraph contains three types of nodes: tasks, actors, and resources.

== Tasks

Tasks are the fundamental unit of work. Each task has:

- *id*: A unique identifier, typically kebab-case (e.g., `implement-api`)
- *title*: Human-readable description of the work
- *status*: One of `open`, `in-progress`, `done`, or `blocked`
- *blocked_by*: List of task IDs that must complete before this task can start
- *assigned*: Optional reference to an actor performing the work
- *estimate*: Optional hours and cost estimates
- *tags*: Arbitrary labels for categorization
- *timestamps*: `created_at`, `started_at`, `completed_at` for temporal tracking

The `blocked_by` field establishes the dependency graph. If task B lists task A in its `blocked_by` array, then B cannot start until A is complete.

== Actors

Actors represent entities that perform work---humans or AI agents. Each actor has:

- *id*: Unique identifier (e.g., `erik`, `agent-1`)
- *name*: Display name
- *role*: Optional classification (engineer, pm, agent)
- *rate*: Optional hourly rate for cost calculations
- *capacity*: Optional available hours for workload balancing

The system treats human and AI actors identically. This uniformity simplifies coordination logic and ensures agents are first-class participants in the workflow.

== Resources

Resources model constraints beyond task dependencies: budgets, compute allocation, API quotas, and similar. Each resource has:

- *id*: Unique identifier
- *type*: Classification (money, compute, time)
- *available*: Quantity available
- *unit*: Unit of measurement (usd, hours, gpu-hours)

Tasks can declare resource requirements via the `requires` field, enabling planning commands to determine what work fits within available constraints.

= Dependency Graph

== Blocked-by Semantics

The dependency relationship is expressed through the `blocked_by` field. When a task lists blockers, it declares: "I cannot start until these tasks are done." This is a soft constraint---the system will warn if you claim a blocked task, but will not prevent it. Real-world projects sometimes require starting work before dependencies are technically complete.

A task is considered *ready* when:
1. Its status is `open`
2. All tasks in its `blocked_by` list have status `done`
3. Any `not_before` timestamp has passed

The `ready` command lists all tasks meeting these criteria, enabling actors to quickly identify available work.

== Transitive Dependencies

Analysis commands compute transitive dependencies to provide deeper insights. The `why-blocked` command traces the full chain explaining why a task cannot start. The `cost` command sums estimates across all transitive dependencies to reveal the true cost of completing a feature.

== Cycle Handling

workgraph explicitly supports cycles in the dependency graph. This is intentional---many real workflows contain cycles:

- Code review: implement -> review -> revise -> review -> approve
- Iterative design: prototype -> test -> refine -> test
- Recurring maintenance: deploy -> monitor -> patch -> deploy

The `loops` command detects and classifies cycles:
- *Intentional*: Tagged with `recurring` or `cycle:intentional`
- *Warning*: Short cycles (2 nodes) without tags, likely bugs
- *Info*: Medium cycles that warrant review

This classification helps teams distinguish intentional design patterns from accidental circular dependencies.

= Temporal Tracking

Timestamps enable temporal analysis of project health:

- *created_at*: When the task was added to the graph
- *started_at*: When work began (status changed to in-progress)
- *completed_at*: When work finished (status changed to done)
- *not_before*: Earliest time the task should be worked on

These fields power several analysis commands:

*Aging analysis* groups open tasks by age: less than a day, 1-7 days, 1-4 weeks, 1-3 months, and over 3 months. Tasks older than a month receive warnings; those over three months are flagged as critical. The oldest open tasks are listed explicitly to highlight potential neglect.

*Velocity tracking* computes task completion rates over rolling windows. It shows tasks completed per week, hours completed per week, and trends (increasing, decreasing, stable). This provides an empirical basis for forecasting.

*Forecasting* combines velocity data with remaining work estimates to project completion dates. Three scenarios are provided: optimistic (estimates accurate), realistic (+30% buffer), and pessimistic (+50% buffer). The system also identifies the critical path and key blockers that could delay completion.

= Analysis Commands

workgraph provides a rich set of analysis commands beyond basic task management:

*ready*: Lists tasks that can be worked on now---no incomplete blockers, past any scheduling constraints.

*why-blocked*: Shows the full transitive chain explaining why a task is blocked, not just immediate dependencies.

*impact*: Reveals what tasks depend on a given task, both directly and transitively.

*bottlenecks*: Identifies tasks blocking the most downstream work. These are prioritization opportunities---completing high-impact blockers unlocks the most progress.

*structure*: Analyzes graph topology to find entry points (tasks with no blockers), dead ends (tasks nothing depends on), and high-impact roots.

*critical-path*: Computes the longest dependency chain by estimated hours---the sequence of tasks that determines minimum project duration.

*workload*: Shows how work is distributed across actors, highlighting imbalances.

*resources*: Displays resource utilization---committed versus available capacity.

*plan*: Given a budget or time constraint, determines which tasks can be accomplished, respecting dependencies.

*analyze*: Comprehensive health report combining all the above analyses.

All commands support `--json` output for programmatic consumption by scripts and agents.

= Agent Coordination

The claim/unclaim protocol enables multiple agents to work in parallel without conflicts.

== The Claim Protocol

When an agent (or human) wants to work on a task:

1. Run `wg ready` to see available tasks
2. Run `wg claim <task-id> --actor <agent-id>` to claim a task
3. The task's status changes to `in-progress` and `assigned` is set
4. Work on the task
5. Run `wg done <task-id>` when complete

The claim operation is atomic and will fail if the task is already in-progress or done. This prevents two agents from accidentally working on the same task.

== The Unclaim Protocol

If an agent cannot complete a task (blocked by external factors, needs human input, or simply moving on), it can run `wg unclaim <task-id>`. This reverts the task to `open` status and clears the assignment, making it available for others.

== Coordination Command

The `wg coordinate` command provides a complete picture for parallel execution:
- Progress summary (done/total)
- Tasks currently in-progress and who is working on them
- Tasks ready for parallel execution
- Blocked tasks and what they're waiting on

Agents can run this command, parse the JSON output, select an available task, claim it, perform the work, and mark it done---all without human intervention.

= Use Cases

== Software Development

A development team uses workgraph to coordinate feature work:

```
wg add "Design API schema"
wg add "Implement API endpoints" --blocked-by design-api-schema
wg add "Write API tests" --blocked-by implement-api-endpoints
wg add "Update documentation" --blocked-by implement-api-endpoints
```

Multiple developers can run `wg ready`, claim different tasks, and work in parallel. The graph ensures dependencies are respected while maximizing parallelism.

== Research Projects

A research lab tracks experiments and paper writing:

```
wg add "Run baseline experiments"
wg add "Analyze baseline results" --blocked-by run-baseline-experiments
wg add "Design improved method" --blocked-by analyze-baseline-results
wg add "Run comparison experiments" --blocked-by design-improved-method
wg add "Write paper" --blocked-by run-comparison-experiments
```

The `forecast` command estimates when the paper might be ready based on historical velocity.

== AI Agent Swarms

Multiple AI agents work on a large codebase refactoring:

```bash
# Orchestrator creates tasks
wg add "Refactor module-auth" --hours 4
wg add "Refactor module-payments" --hours 6
wg add "Refactor module-notifications" --hours 3
wg add "Integration tests" --blocked-by refactor-module-auth \
    --blocked-by refactor-module-payments \
    --blocked-by refactor-module-notifications
```

Each agent runs in a loop:
1. `wg coordinate --json` to get available work
2. Claim a ready task
3. Perform the refactoring
4. Run tests
5. `wg done <task>` and commit
6. Repeat

The workgraph ensures agents don't duplicate effort and that integration tests only run after all modules are refactored.

= Future Work

Several extensions could enhance workgraph's capabilities:

*Priority and scheduling*: Adding priority fields and more sophisticated scheduling algorithms to optimize task ordering beyond simple dependency traversal.

*Resource constraints*: Deeper integration of resource modeling, where tasks declare resource requirements and the system prevents over-commitment.

*Event sourcing*: Storing changes as an append-only event log rather than mutating the graph file, enabling richer history and easier conflict resolution.

*Distributed coordination*: Supporting multiple graph replicas with conflict-free merge semantics for truly distributed teams.

*Agent-specific extensions*: Metadata fields for agent capabilities, estimated completion times, and confidence levels to enable smarter task assignment.

*Visualization*: Interactive graph visualization for humans to understand complex dependency structures at a glance.

= Conclusion

workgraph occupies a specific niche: task coordination that is simple enough for AI agents to use reliably, yet expressive enough for real project management needs. By storing data in git-friendly JSONL, supporting dependency cycles, providing rich analysis commands, and implementing a clean claim/unclaim protocol, workgraph enables hybrid human-AI teams to coordinate effectively.

The system does not try to replace sophisticated project management tools for large organizations. Instead, it provides a foundation for the emerging pattern of AI-assisted development: humans defining goals and constraints, agents executing work, and everyone coordinating through a shared, machine-readable task graph.

As AI agents become more capable and autonomous, the need for such coordination infrastructure will only grow. workgraph offers one pragmatic answer to the question: how do we organize work when some of the workers are artificial intelligences?
