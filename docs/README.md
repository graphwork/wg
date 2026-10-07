# WG Documentation

WG (`wg`) is a task coordination system designed for both humans and AI agents. It provides a dependency-aware task graph that enables parallel work coordination, progress tracking, and project analysis.

## Table of Contents

- [Core Concepts](#core-concepts)
- [Quick Start](#quick-start)
  - [First-Time Setup](#first-time-setup)
- [Command Reference](./COMMANDS.md)
- [Agent Guide](./AGENT-GUIDE.md)
- [Storage Format](#storage-format)
- [JSON Output](#json-output)

## Core Concepts

### Tasks

Tasks are the fundamental units of work. Each task has:

- **id**: Unique identifier (auto-generated from title or specified manually)
- **title**: Human-readable description of the work
- **description**: Detailed body text, acceptance criteria, etc.
- **status**: Current state (open, blocked, in-progress, pending-validation, done, failed, abandoned, waiting)
- **after**: List of task IDs that must complete before this task can start
- **before**: List of task IDs that this task blocks
- **assigned**: Agent currently working on the task
- **estimate**: Optional hours and/or cost estimate
- **tags**: Labels for filtering and categorization
- **skills**: Required capabilities to complete the task
- **inputs**: Files or context needed to start work
- **deliverables**: Expected outputs when complete
- **artifacts**: Actual produced outputs (populated on completion)
- **model**: Preferred LLM model override (e.g., `opus`, `sonnet`, `haiku`)
- **provider**: LLM provider override (`anthropic`, `openai`, `openrouter`, `local`)
- **verify**: Validation criteria — triggers `pending-validation` gate on completion
- **exec_mode**: Agent capability tier (`full`, `light`, `bare`, `shell`)
- **visibility**: Trace export zone (`internal`, `public`, `peer`)
- **context_scope**: How much context the coordinator injects (`clean`, `task`, `graph`, `full`)
- **delay**: Delay before dispatch (e.g., `1h`, `30m`)
- **not_before**: Earliest dispatch time (ISO 8601)
- **placement_hints**: Hints for auto-placement (`place_near`, `place_before`); set `agency.auto_place=false` to disable inference
- **cycle_config**: Cycle iteration settings (`max_iterations`, convergence, etc.)
- **retry_count / max_retries**: Retry tracking and limits

### Status Flow

```
     ┌────────────────────────────────────────────────────────┐
     │                                                        │
     v                                                        │
   open ──────> in-progress ──────> done                      │
     │              │    │                                    │
     │              │    └──> pending-validation               │
     │              │              │         │                │
     │              │              v         v                │
     │              │            done    open (rejected)       │
     │              │                                         │
     │              ├──> waiting ──────> in-progress (resumed) │
     │              │                                         │
     │              v                                         │
     │          failed ──────────> (retry) ───────────────────┘
     │              │
     │              v
     │         abandoned
     │
     └────────────────────────────────────────────────────────> abandoned
```

- **open**: Task exists but work has not started; all dependencies are met
- **blocked**: Task has incomplete `after` dependencies (derived — transitions to open when unblocked)
- **in-progress**: Task has been claimed and is being worked on
- **pending-validation**: Agent called `wg done`, but `--verify` criteria require review (`wg approve` / `wg reject`)
- **done**: Task completed successfully (and validated, if applicable)
- **failed**: Task attempted but failed (can be retried with `wg retry`)
- **abandoned**: Task will not be completed (terminal state)
- **waiting**: Task is paused, waiting for an external event or manual intervention (`wg resume` to continue)

Only **ready** (open, unblocked) tasks appear in `wg ready`.

### Agents

Agents represent humans or AIs who perform work. An agent is a unified identity that combines:

- **name**: Display name
- **role + tradeoff**: What the agent does and why (required for AI, optional for human)
- **capabilities**: Skills for task matching
- **trust_level**: verified, provisional, or unknown
- **capacity**: Maximum concurrent task capacity
- **rate**: Hourly cost rate
- **contact**: Contact info (email, Matrix ID, etc.)
- **executor**: How the agent receives work (claude, matrix, email, shell)

### Resources

Resources represent consumable or limited assets:

- **id**: Unique identifier
- **type**: Category (money, compute, time, etc.)
- **available**: Current available amount
- **unit**: Unit of measurement (usd, hours, gpu-hours, etc.)

### Dependencies (The Graph)

Tasks form a directed graph through `after` dependency edges. Repeating workflows (review loops, retries, recurring work) are modeled as structural cycles — `after` back-edges with `CycleConfig` controlling iteration limits. Use `wg cycles` to inspect them.

Key graph concepts:

- **Ready tasks**: No incomplete blockers, can start immediately
- **Blocked tasks**: Waiting on one or more incomplete dependencies
- **Critical path**: Longest dependency chain determining minimum project duration
- **Bottlenecks**: Tasks blocking the most downstream work
- **Impact**: Forward view of what depends on a given task

### Graph Patterns

Dependencies naturally give rise to recurring structural patterns:

- **Pipeline**: A linear chain (A → B → C) where each task waits for its predecessor
- **Fan-out / Fan-in (Diamond)**: One task fans out to parallel children; a downstream integrator fans them back in
- **Scatter-Gather**: Multiple reviewers with different roles examine the same artifact in parallel
- **Review Cycle**: A structural cycle (write → review → revise → write) with a guard condition that breaks the loop on approval
- **Seed Task**: A task whose job is to *create other tasks*—it analyzes a problem, decomposes it, and fans out into subtasks that didn't exist before it ran. Seed tasks grow the graph rather than just executing it. (Also called *spark* or *generative task* in theoretical contexts.)

See the [manual](./manual/) for detailed explanations and diagrams of each pattern.

### Context Flow

Tasks can specify inputs and deliverables to establish an implicit data flow:

```
Task A                    Task B
├─ deliverables:          ├─ inputs:
│  └─ src/api.rs     ──────> └─ src/api.rs
│                         ├─ after:
│                              └─ task-a
```

The `wg context` command shows available inputs from completed dependencies.

### Trajectories

For AI agents with limited context windows, trajectories provide an optimal task ordering that minimizes context switching. The `wg trajectory` command computes paths through related tasks based on shared files and skills.

## Quick Start

A normal install provides `worksgood`, `wg`, and `nex`. For the attended human
lifecycle, enter a project and run `worksgood`; use `worksgood --help` for its
focused setup/status/stop/restart/TUI verbs. The complete expert commands below
remain under `wg`.

### Install

```bash
# primary documented path (needs the Rust toolchain)
cargo install --git https://github.com/graphwork/wg --locked

# additive npm route (Node 22.19+): prebuilt per-platform packages, no Rust
# toolchain, zero postinstall scripts; also installs the Pi coding agent CLI
npm install -g @worksgood/cli
```

> **macOS limitation:** the macOS binaries currently ship **unsigned** and
> **un-notarized** (Apple Developer ID secrets are not configured), so
> Gatekeeper may block the first run. Allow it with
> `xattr -d com.apple.quarantine "$(which wg)"` (repeat for `worksgood` and
> `nex`) or right-click → Open.

See [Install WG](./guides/install.md) for the full installer, channels, and
verification details.

### First-Time Setup

In a project directory, run the interactive setup wizard:

```bash
wg setup
```

The wizard is **project-scoped only** — it never changes machine-global
config or any active profile. It walks you through:

- **Route**: `pi` is the only smooth setup route (`SetupRoute::all()`,
  `src/config_defaults.rs`). The wizard also offers "Not now — keep this WG
  graph-only", which exits without writing anything.
- **Pi models**: a strong worker route (`pi:<provider>:<model>`) and an
  optional weak route for recoverable one-shots (`.flip` / `.assign` / eval).
  Pi owns its own login/providers/models; WG only detects readiness and never
  handles Pi credentials.
- **Agency**: whether to enable WG's automatic agent matching and completion
  review (off by default).
- **Max agents**: number of parallel agents the coordinator can spawn.

It writes the checked-in **project** document `<project-root>/worksgood.toml`,
for example:

```toml
[agent]
model = "pi:<provider>:<model>"

[dispatcher]
max_agents = 4

[agency]
auto_assign = false
auto_evaluate = true
```

Worker routes are explicit `pi:<provider>:<model>`, `claude:<model>`, or
`codex:<model>` specs (`parse_supported_execution_route`, `src/config.rs`);
the legacy `executor` key is deprecated. `worksgood.toml` is the single
authoritative project configuration document — `~/.wg/config.toml` is legacy
machine-global state and is not merged into project behavior. Re-change the
route any time with `wg config -m <route>`, `wg config --set-model <role>
<route>`, or `wg profile select <name>`, and inspect the effective
configuration with `wg config --list`.

### Initialize a New Project

```bash
wg init
```

Creates `.wg/graph.jsonl` in the current directory.

### Add Tasks

`wg add` always creates a visible paused draft. After adding and wiring tasks, release each intentionally with `wg publish <task-id> --only` (or release a connected batch with `--wcc`).

```bash
# Simple task
wg add "Design API schema"

# Task with dependencies
wg add "Implement API" --after design-api-schema

# Task with full metadata
wg add "Write API tests" \
  --after implement-api \
  --hours 4 \
  --skill testing \
  --deliverable tests/api_test.rs
```

### View Project Status

```bash
# What can I work on right now?
wg ready

# All tasks
wg list

# Project health overview
wg analyze
```

### Work on Tasks

```bash
# Claim a task
wg claim design-api-schema --actor erik

# Log progress
wg log design-api-schema "Defined initial endpoints"

# Complete the task
wg done design-api-schema
```

### Analyze the Project

```bash
# What's blocking this task?
wg why-blocked implement-api

# What depends on this task?
wg impact design-api-schema

# What are the bottlenecks?
wg bottlenecks

# When will we finish?
wg forecast
```

## Storage Format

All data is stored in `.wg/graph.jsonl` - a newline-delimited JSON file with one node per line. This format is:

- Human-readable and editable
- Version control friendly (line-based diffs)
- Easy to parse with standard tools

Example content:

```jsonl
{"kind":"task","id":"design-api","title":"Design API","status":"done","completed_at":"2026-01-15T10:00:00Z"}
{"kind":"task","id":"impl-api","title":"Implement API","status":"open","after":["design-api"]}
```

Configuration is stored in `<project-root>/worksgood.toml`:

```toml
[agent]
model = "pi:<provider>:<model>"
interval = 10

[project]
name = "My Project"
```

## JSON Output

All commands support `--json` for machine-readable output:

```bash
# Task list as JSON
wg list --json

# Single task details
wg show task-id --json

# Analysis results
wg analyze --json
```

This enables integration with scripts, CI/CD pipelines, and other tools:

```bash
# Count open tasks
wg list --json | jq '[.[] | select(.status == "open")] | length'

# Get IDs of ready tasks
wg ready --json | jq -r '.[].id'
```

### Peer WG Instances

WG instances in separate repositories can communicate via the peer system. Register peers with `wg peer add <name> <path>` and stage cross-repo drafts with `wg add "Task" --repo <peer>`; the command prints the explicit `wg --dir <peer-wg-dir> publish <id> --only` release step. See the [Command Reference](./COMMANDS.md) for details.

## See Also

- [Command Reference](./COMMANDS.md) - Complete command documentation
- [Agent Guide](./AGENT-GUIDE.md) - Autonomous agent operation guide
- [Agency System](./AGENCY.md) - Agency system documentation
- [Agent Service](./AGENT-SERVICE.md) - Service architecture
