# Amplifier Bundle for WG: Architectural Summary

**Source**: [ramparte/amplifier-bundle-wg](https://github.com/ramparte/amplifier-bundle-wg)
**Date**: 2026-02-18

## 1. What Amplifier Is and What "Bundles" Are

**Amplifier** is Microsoft's agent orchestration framework. It provides:
- Multi-agent delegation and session management
- A bundle/recipe ecosystem for packaging agent behaviors, context, and tools
- Execution modes including `--mode single` (non-interactive, one-shot) and interactive sessions
- Output formats including `--output-format json` for structured results

**Bundles** are Amplifier's packaging unit — a self-contained collection of behaviors, agents, context documents, and configuration that can be installed into an Amplifier environment. A bundle is defined by a `bundle.md` file with YAML frontmatter:

```yaml
---
bundle:
  name: wg
  version: 0.1.0
  description: "wg integration for Amplifier"
includes:
  - bundle: wg:behaviors/wg
---
```

Bundles are referenced by namespace (`wg:path/to/resource`) and can be installed from git URLs:

```bash
amplifier bundle add git+https://github.com/ramparte/amplifier-bundle-wg
```

The WG bundle provides: a behavior definition, a planner agent, and context documents that teach Amplifier agents how to use `wg`.

## 2. The Bidirectional Integration Model

This is the core architectural insight. The integration works in **two independent directions**, and they compose:

### Direction A: Amplifier → WG

Add WG awareness to Amplifier sessions. When an Amplifier agent encounters a task with non-linear dependencies (multiple parallel workstreams with ordering constraints), it decomposes it into a WG task graph:

1. Agent detects complex task structure (heuristic: 4+ subtasks, parallelism opportunity, data dependencies between subtasks)
2. Runs `wg init` and `wg add` to build the task graph
3. Runs `wg service start` to launch the wg daemon
4. Daemon dispatches tasks to executors as they become ready

This direction is enabled by installing the **behavior** (`behaviors/wg.yaml`) and its associated **context** (`context/wg-guide.md`) and **agent** (`agents/wg-planner.md`).

### Direction B: wg → Amplifier

Install Amplifier as a wg executor so the service daemon spawns full Amplifier sessions for each task:

1. Install `executor/amplifier.toml` and `executor/amplifier-run.sh` into `.wg/executors/`
2. Set `coordinator.executor = "amplifier"` in `.wg/config.toml`
3. When `wg service start` dispatches a task, it spawns an Amplifier session instead of (or alongside) a Claude CLI session
4. Each task gets the full Amplifier ecosystem — bundles, tools, recipes, multi-agent delegation

### Composed: Amplifier ↔ wg

When both directions are active, the architecture becomes recursive:

```
User
  |
  v
Amplifier Session (with wg behavior)
  |
  |--> detects complex task
  |--> wg init / wg add (builds task graph)
  |--> wg service start (launches daemon)
  |
  v
wg Service Daemon
  |
  |--> dispatches ready tasks
  |--> spawns Amplifier executor for each
  |
  +---> Amplifier Session (task A) ---> wg done task-a
  +---> Amplifier Session (task B) ---> wg done task-b
  +---> Amplifier Session (task C) ---> wg done task-c
  |
  |--> detects completions, unblocks dependents
  |--> spawns next wave
  v
All tasks done --> reports back to user
```

An Amplifier agent decomposes work into a wg, then wg spawns Amplifier sessions to execute each piece. The child sessions could theoretically themselves detect sub-tasks requiring wg decomposition, creating nested coordination (though this isn't explicitly encouraged).

## 3. The Executor Config Format (`amplifier.toml`)

The executor config is a standard wg TOML file placed at `.wg/executors/amplifier.toml`. It has three sections:

### `[executor]` — Core Configuration

```toml
[executor]
type = "claude"                                    # See note below
command = ".wg/executors/amplifier-run.sh"  # The wrapper script
args = []                                          # Extra args (e.g. ["-B", "my-bundle"])
working_dir = "{{working_dir}}"                    # Template variable
timeout = 600                                      # Seconds (default 10 min)
```

**Critical design note**: `type = "claude"` is used **not** because the executor is Claude, but because this is the only executor type in wg's codebase that generates `cat prompt.txt | command` (stdin piping). For all other `type` values, wg sets `stdin = Stdio::null()` at `spawn.rs:336`, silently dropping the prompt. This is a pragmatic hack documented in `CONTEXT-TRANSFER.md`.

### `[executor.env]` — Environment Variables

```toml
[executor.env]
WG_TASK_ID = "{{task_id}}"
```

wg passes the task ID via environment variable so the wrapper script and agent can reference it.

### `[executor.prompt_template]` — Rendered Task Prompt

```toml
[executor.prompt_template]
template = """
{{task_identity}}

# Task Assignment
**Task ID**: `{{task_id}}`
**Title**: {{task_title}}

## Description
{{task_description}}

## Context from Completed Dependencies
{{task_context}}

## wg Protocol
...
"""
```

**Template variables** replaced at spawn time:
- `{{task_id}}` — The task's ID string
- `{{task_title}}` — Human-readable title
- `{{task_description}}` — Full task description
- `{{task_context}}` — Aggregated context from completed upstream dependencies (artifacts, logs, outputs)
- `{{task_identity}}` — Agent identity block (role, skills, etc.)
- `{{working_dir}}` — Project root directory

The prompt template embeds the full wg executor protocol inline — logging, artifact recording, done/fail marking — so each spawned agent knows exactly how to interact with wg regardless of what other context it has.

## 4. The Context Transfer Mechanism

Context flows from completed tasks to their dependents through several channels:

### At Spawn Time (Push)

When wg renders the prompt template for a new task, `{{task_context}}` is replaced with aggregated context from all completed `blocked_by` dependencies. This includes:
- **Artifacts**: File paths recorded via `wg artifact <id> <path>`
- **Logs**: Progress messages recorded via `wg log <id> "..."`
- **Description/title**: What the upstream task was supposed to do

This is a **push** model — context is injected into the prompt before the agent starts.

### During Execution (Pull)

Agents can pull additional context at any time:

```bash
wg context <TASK_ID>    # Aggregated context from upstream dependencies
wg show <TASK_ID>       # Full task details including description, acceptance criteria
```

### Artifacts as the Primary Transfer Medium

The artifact system (`wg artifact <id> <path>`) is the primary mechanism for inter-task data transfer. When task A records `wg artifact task-a src/schema.sql`, task B (which is `--blocked-by task-a`) receives that file path in its `{{task_context}}` and can read/use the file.

### The stdin→arg Bridge Problem

A key technical challenge documented in `CONTEXT-TRANSFER.md`:

1. wg pipes the rendered prompt to the executor command's **stdin** (but only for `type = "claude"`)
2. `amplifier run --mode single` expects the prompt as a **positional argument**, not stdin
3. The `amplifier-run.sh` wrapper bridges this gap: reads stdin into a variable, then passes it as the last positional arg to `amplifier run`

```bash
# amplifier-run.sh (simplified)
PROMPT=$(cat)                          # Read stdin (from wg)
exec amplifier run --mode single \     # Pass as positional arg
     --output-format json \
     "${EXTRA_ARGS[@]}" \
     "$PROMPT"
```

## 5. The Behavior/Agent Model

### Behavior: `behaviors/wg.yaml`

The behavior is what Amplifier loads to give agents wg awareness:

```yaml
bundle:
  name: wg-behavior
  version: 0.1.0
  description: "Adds wg task graph awareness and tooling to Amplifier agents"

agents:
  include:
    - wg:wg-planner

context:
  include:
    - wg:context/wg-guide.md
```

This does two things:
1. **Loads the wg-planner agent** — a specialized agent for task decomposition that can be delegated to
2. **Loads the wg-guide.md context** — a comprehensive reference document injected into the agent's context window

Note: `context/wg-executor-protocol.md` is **not** included in the behavior's `context.include`. It is only used in the executor's prompt template. This is intentional — the executor protocol is for agents spawned *by* wg, while the behavior is for agents using wg *from* Amplifier.

### Agent: `agents/wg-planner.md`

A specialized agent with YAML frontmatter metadata:

```yaml
meta:
  name: wg-planner
  description: "Decomposes complex tasks into wg dependency graphs..."
```

The planner agent's responsibilities:
1. **Analyze** tasks for natural subtasks
2. **Identify dependencies** — which subtasks need others first
3. **Find parallelism** — independent subtasks that can run concurrently
4. **Assign skills** — what capabilities each subtask needs (`architecture`, `frontend`, `backend`, `testing`, etc.)
5. **Set verification** — `--verify` criteria for quality gates
6. **Build the graph** — execute `wg add` commands with proper `--blocked-by` chains

The planner documents three common decomposition patterns:
- **Fan-out / Fan-in**: One design task → N parallel implementations → integration testing
- **Pipeline**: Sequential phases with parallel sub-branches
- **Iterative**: Loop edges for write → review → revise cycles (using `--loops-to`)

Task sizing guidance: 5–30 minutes per agent. Smaller loses to overhead, larger loses parallelism.

### Context: `context/wg-guide.md`

A comprehensive reference loaded into every Amplifier agent that has the wg behavior. Covers:

- **When to use wg** (detection heuristics: 4+ subtasks, parallelism, data dependencies)
- **When NOT to** (simple sequential, single-file, no parallelism)
- **Full CLI reference** — every `wg` command organized by category (task management, querying, analysis, service daemon)
- **Task lifecycle** — Open → InProgress → Done/Failed, with retry and blocking semantics
- **Decomposition patterns** — Fan-out/fan-in, pipeline, iterative
- **Configuration reference** — `.wg/config.toml` format

### Context: `context/wg-executor-protocol.md`

Used **only** in the executor prompt template (not in the behavior). Defines the protocol for agents spawned by wg's daemon:

1. **Log progress**: `wg log <ID> "..."`
2. **Record artifacts**: `wg artifact <ID> <path>`
3. **Mark done**: `wg done <ID>` (unblocks dependents)
4. **Mark failed**: `wg fail <ID> --reason "..."` (specific reason for triage)
5. **Read context**: `wg context <ID>` for upstream outputs

Key rules: always mark done/fail before exiting, log frequently, stay focused on your task only, don't modify the graph.

## 6. Installation and Distribution (`install.sh`)

### What `install.sh` Does

```bash
./executor/install.sh [project-dir]
```

1. Resolves the target directory (default: current directory)
2. Validates that `.wg/` exists (fails with error if not — must `wg init` first)
3. Creates `.wg/executors/` if it doesn't exist
4. Copies `amplifier.toml` to `.wg/executors/amplifier.toml`
5. Copies `amplifier-run.sh` to `.wg/executors/amplifier-run.sh`
6. Makes the wrapper script executable (`chmod +x`)
7. Prints instructions for setting as default executor

After installation, the user sets the executor as default:
```bash
wg config coordinator.executor amplifier
```

### Bundle Distribution

Bundles are distributed via git URLs:
```bash
amplifier bundle add git+https://github.com/ramparte/amplifier-bundle-wg
```

Or via the `includes` mechanism in other bundle definitions:
```yaml
includes:
  - git+https://github.com/graphwork/amplifier-bundle-wg#subdirectory=behaviors/wg.yaml
```

Resources within bundles are referenced by namespace: `wg:wg-planner`, `wg:context/wg-guide.md`.

## 7. Bugs and Design Constraints (from CONTEXT-TRANSFER.md)

Several bugs were discovered and fixed during development (commit `fbd612a`):

1. **Executor type hack**: wg only pipes stdin for `type = "claude"` executors. Custom types get `Stdio::null()`. The amplifier executor uses `type = "claude"` as a workaround, with a wrapper script that bridges stdin to positional args.

2. **Bundle YAML format**: Initial versions had incorrect YAML structure. The correct format uses `agents.include` and `context.include` as lists, and namespace paths without file extensions.

3. **Include URI format**: `bundle.md` must use `bundle: wg:behaviors/wg` (with namespace prefix and `bundle:` key), not bare relative paths.

4. **Non-blocking spawn**: `wg spawn` is non-blocking — tests must poll for task completion rather than checking status immediately.

### Future Considerations

- Coordinating with wg upstream on a standardized non-Claude executor prompt-passing mechanism
- Whether to publish under `graphwork/` or keep under `ramparte/`
- `wg-executor-protocol.md` is not included in the behavior's `context.include` — agents only get `wg-guide.md` by default
- The `wg` binary must be on PATH when executor sessions run

## 8. Test Suite

21 quick tests (no LLM calls) + E2E lifecycle tests:

- **TOML validity**: Parses `amplifier.toml`, checks required fields (`type`, `command`, `args`, `working_dir`, `prompt_template`)
- **Template variables**: Verifies `{{task_id}}`, `{{task_title}}`, `{{task_description}}`, `{{task_context}}` present in template
- **Install script**: Tests installation into valid WG project, verifies files match source
- **Install validation**: Rejects directories without `.wg/`
- **Wrapper script**: Tests flag forwarding (`--model`, `--bundle`) and prompt passing via stdin→positional-arg bridge
- **Bundle structure**: Verifies all expected files exist, bundle.md has YAML frontmatter, agent has meta frontmatter
- **E2E lifecycle** (slow): Creates a project, installs executor, adds a trivial task, spawns Amplifier session, polls for completion (120s timeout), verifies artifact creation

## 9. Key Takeaways for wg Integration

1. **The executor interface is the integration point**: Any system can become a wg executor by providing a TOML config with `type = "claude"` (for stdin piping) and a wrapper that reads the rendered prompt. The executor receives full task context including dependency outputs.

2. **Context-driven, not tool-driven**: The bundle adds value by teaching agents the wg mental model through context documents, not by providing custom tool integrations. Agents use `bash` to call `wg` CLI directly — "ruthless simplicity."

3. **The `type = "claude"` constraint is a real limitation**: wg currently has a hardcoded assumption that only Claude-type executors need stdin piping. This forces all custom executors to use `type = "claude"` and wrapper scripts. This should be addressed upstream.

4. **Template variables provide the contract**: The set of `{{variables}}` in the prompt template defines the data contract between wg and its executors. Currently: `task_id`, `task_title`, `task_description`, `task_context`, `task_identity`, `working_dir`.

5. **The planner agent is the decomposition brain**: When Amplifier detects a complex task, it delegates to `wg-planner` which has deep knowledge of dependency patterns, task sizing, and skill assignment.
