# Context Transfer: wg vs Amplifier

**Date**: 2026-02-18
**Sources**: `CONTEXT-TRANSFER.md` (amplifier-bundle-wg), `docs/research/amplifier-architecture.md`, `src/commands/spawn.rs`, `src/service/executor.rs`, `src/commands/context.rs`, `src/commands/artifact.rs`, `src/graph.rs`

## 1. How Amplifier's Bundle Passes Task Context

Amplifier's wg bundle uses a **prompt template** embedded in `executor/amplifier.toml` to pass structured context to each spawned session. The template uses six variables that wg replaces at spawn time:

| Variable | Content |
|---|---|
| `{{task_id}}` | The task's string ID |
| `{{task_title}}` | Human-readable title |
| `{{task_description}}` | Full description (body, acceptance criteria) |
| `{{task_context}}` | Aggregated context from completed `blocked_by` dependencies |
| `{{task_identity}}` | Agent identity block (role, motivation, skills) from the agency system |
| `{{working_dir}}` | Project root directory |

The `{{task_context}}` variable is the primary inter-task data channel. It is populated by `build_task_context()` in `spawn.rs:89-119`, which iterates over `task.blocked_by` dependencies and collects:

- **Artifact paths**: file paths recorded via `wg artifact <dep-id> <path>` — listed as comma-separated strings (`"From <dep-id>: artifacts: path1, path2"`)
- **Log entries**: the last 5 log messages from completed dependencies (`"From <dep-id> logs: <timestamp> <message>"`)

The rendered prompt is written to a file (`prompt.txt`) and piped via stdin to the executor command. For Amplifier specifically, a wrapper script (`amplifier-run.sh`) bridges stdin to a positional argument because `amplifier run --mode single` expects the prompt as an argument, not stdin.

The amplifier bundle also includes `context/wg-executor-protocol.md`, which is injected into the prompt template to teach agents the logging/artifact/done/fail protocol — so every spawned agent knows the full interaction contract without needing prior knowledge of wg.

**Key design**: Context transfer is **push at spawn time** (prompt template) with **pull during execution** (`wg context <id>`, `wg show <id>`). The push model ensures agents have what they need from moment one; pull exists as a fallback.

## 2. How wg Currently Constructs prompt.txt

The prompt is built in `spawn_agent_inner()` (`spawn.rs:123-457`). The sequence:

1. **Load graph** and validate task status (must be Open or Blocked)
2. **Build context** via `build_task_context()` — iterates `blocked_by`, collects artifacts and last 5 log entries from done dependencies
3. **Create TemplateVars** from the task, context string, and wg directory (`executor.rs:34-57`)
4. **Resolve agent identity** — if the task has an `agent` field, look up the Agent/Role/Motivation in `.wg/agency/` and render an identity prompt
5. **Resolve skills preamble** — if `.claude/skills/using-superpowers/SKILL.md` exists, include it
6. **Load executor config** — either from `.wg/executors/<name>.toml` or the built-in default
7. **Apply templates** — replace all `{{variables}}` in the config (command, args, env, prompt template, working dir)
8. **Write prompt.txt** to the agent's output directory
9. **Generate wrapper script** (`run.sh`) that pipes prompt.txt into the executor and auto-marks done/failed
10. **Spawn** the wrapper as a detached process

### What's included in the default claude executor prompt

The default prompt template (`executor.rs:331-382`) includes:
- Skills preamble (if available)
- Agent identity block (role, motivation, acceptable/non-negotiable constraints, resolved skills)
- Task ID, title, description
- Dependency context (artifacts + logs)
- Workflow instructions (log, artifact, done, fail commands)
- Critical rules (use wg CLI, not built-in tools)

### What's missing or limited

1. **Dependency descriptions and titles are not passed** — `build_task_context()` only extracts artifacts and logs from dependencies. It does not include the dependency's title, description, or acceptance criteria. An agent receiving context from `research-phase` only sees "From research-phase: artifacts: report.md" and some log snippets — not what `research-phase` was actually about.

2. **Artifact content is not inlined** — Only file *paths* are passed. The agent must read the files itself. For small artifacts (a summary paragraph, a config snippet), inlining content would save a tool-call round trip.

3. **No structured artifact metadata** — Artifacts are bare strings (file paths). There's no way to attach a description, MIME type, or semantic label to an artifact. "From dep-1: artifacts: output.txt" tells you nothing about what output.txt contains.

4. **Log truncation is aggressive** — Only the last 5 log entries per dependency are included. For long-running tasks with 20+ log entries, critical early context is lost.

5. **No transitive context** — Only direct `blocked_by` dependencies contribute context. If task C depends on B which depends on A, task C sees nothing from A. This is by design (avoids context bloat) but means important artifacts from A must be explicitly re-exported by B.

6. **`inputs` and `deliverables` fields are unused at spawn time** — Tasks can declare `inputs` (files they need) and `deliverables` (files they'll produce), but `build_task_context()` doesn't use these to validate or filter what context is passed. The `wg context` command *does* use them, but only as a pull mechanism.

7. **No acceptance criteria in prompt** — The `verify` field exists but isn't included in the prompt template, so agents don't know what "done" means beyond the description.

## 3. The Artifact System

### Recording artifacts

`wg artifact <task-id> <path>` (`artifact.rs:11-35`):
- Appends the path string to `task.artifacts: Vec<String>`
- Deduplicates (won't add if already present)
- Persists to the graph file

Artifacts are just strings — typically relative file paths, but there's no validation. An agent can record `src/main.rs`, `https://example.com`, or `"the answer is 42"` as an artifact.

### Consuming artifacts

**At spawn time** (push): `build_task_context()` in `spawn.rs:89-119` iterates blocked_by dependencies and formats artifact paths into the `{{task_context}}` variable as `"From <dep-id>: artifacts: path1, path2"`.

**During execution** (pull): `wg context <task-id>` (`context.rs:27-113`):
- Iterates `blocked_by` dependencies
- Collects artifacts from each dependency
- Cross-references with the task's declared `inputs` to show `[available]` vs `[missing]` status
- Supports JSON output for machine consumption

**Reverse query**: `wg context --dependents <task-id>` shows what downstream tasks need from a task's outputs, cross-referencing artifacts/deliverables with dependent tasks' `inputs`.

### Data model

From `graph.rs:148+`, the Task struct has three artifact-related fields:

| Field | Type | Purpose |
|---|---|---|
| `inputs` | `Vec<String>` | Declared input files/paths this task needs |
| `deliverables` | `Vec<String>` | Expected output files this task should produce |
| `artifacts` | `Vec<String>` | Actual produced artifact paths (populated at runtime) |

The `inputs`↔`artifacts` connection between tasks creates a typed data flow graph overlaid on the dependency graph. However, this connection is only used by `wg context` (pull) and not by the spawn-time context builder (push).

## 4. Recommendations for Enriching wg's Context Transfer

### R1: Include dependency metadata in context (low effort, high impact)

`build_task_context()` should include each dependency's **title** and optionally its **description snippet** alongside artifacts and logs:

```
From research-phase (title: "Research amplifier architecture"):
  artifacts: docs/research/amplifier-architecture.md
  logs: [last 5 entries]
```

This gives the downstream agent semantic context about what each dependency was doing, not just what files it produced.

### R2: Validate inputs against upstream artifacts (medium effort, medium impact)

At spawn time, cross-reference the task's `inputs` field against the artifacts of its `blocked_by` dependencies. If a declared input isn't available from any dependency, either warn in the prompt or log it. This catches graph misconfiguration early.

### R3: Allow artifact annotations (medium effort, medium impact)

Extend `wg artifact` to support an optional description:

```bash
wg artifact my-task report.md --description "Architectural analysis of amplifier bundle"
```

This metadata would flow through `{{task_context}}` so downstream agents know what each file contains without having to read it.

### R4: Make log entry count configurable (low effort, low impact)

The hardcoded `take(5)` in `build_task_context()` should be configurable (e.g., via `config.toml` or a `--context-depth` flag), defaulting to 5 but overridable for contexts where more history matters.

### R5: Support optional artifact content inlining (high effort, high impact)

For small artifacts (< N bytes, configurable), inline the content directly in the prompt:

```
From dep-1: artifact report.md (243 bytes):
  ```
  [file content here]
  ```
```

This eliminates the need for agents to read small output files and is especially valuable for text artifacts like summaries, configs, or structured data.

### R6: Include acceptance criteria / verify field (low effort, low impact)

If the task has a `verify` field, include it in the prompt so the agent knows what "done" means beyond the description.

### R7: Add `{{task_inputs}}` and `{{task_deliverables}}` template variables (low effort, medium impact)

Expose the task's declared `inputs` and `deliverables` as template variables so executor configs can include them in prompts. This helps agents understand what files they're expected to consume and produce.

## 5. Should wg Adopt Amplifier's Context Protocol or Vice Versa?

### The current state

The two systems have **compatible but distinct** context models:

| Dimension | wg (native) | amplifier bundle |
|---|---|---|
| **Context injection** | Prompt template with `{{task_context}}` | Same — uses wg's template system |
| **Context source** | `build_task_context()` in spawn.rs | Same function — amplifier is a wg executor |
| **Runtime context** | `wg context`, `wg show` | Same — agents call wg CLI |
| **Artifact model** | Bare path strings | Same — uses wg's artifact system |
| **Identity** | Agency system (role/motivation/skills) | Amplifier's own agent/behavior model |
| **Protocol docs** | Inline in prompt template | Separate `wg-executor-protocol.md` file |

The amplifier bundle **wraps** wg's context system rather than replacing it. The only amplifier-specific layer is the stdin→arg bridge (`amplifier-run.sh`) and the behavior/agent YAML that teaches Amplifier agents about wg.

### Recommendation: wg should improve its own context system; amplifier inherits the improvements

The amplifier bundle is a **thin adapter** over wg's executor interface. There is no separate "amplifier context protocol" to adopt — amplifier uses wg's `{{task_context}}` variable and `wg artifact`/`wg context` commands directly.

The correct path is:

1. **Enrich wg's `build_task_context()`** (recommendations R1-R5 above). All executors — claude, amplifier, future ones — automatically benefit.

2. **Standardize the executor prompt interface** beyond `type = "claude"`. The current `Stdio::null()` for non-claude types forces amplifier to use `type = "claude"` as a hack. Adding a `stdin_mode = "pipe"` option or making stdin piping the default for all executor types with a prompt_template would eliminate this workaround.

3. **Keep the protocol document approach** from amplifier's bundle. The `wg-executor-protocol.md` pattern — a standalone document defining the agent↔wg interaction contract — is cleaner than embedding it inline in the default prompt template. wg could ship a similar file at `.wg/context/executor-protocol.md` that any executor config can reference via `{{include:executor-protocol.md}}`.

4. **Don't adopt amplifier's behavior/agent model into wg**. wg already has the agency system (roles, motivations, agents). Amplifier has its own behavior/agent YAML format. These should remain separate — each system manages identity in its own way, and the bridge is the prompt template where `{{task_identity}}` renders wg's agency data while amplifier adds its own behavior context.

### What amplifier should adopt from wg

The amplifier bundle should:
- Use `wg context --json` for structured context queries rather than parsing `wg show` output
- Consider surfacing `inputs`/`deliverables` in its prompt template so agents know the data flow contract
- Include `wg-executor-protocol.md` in the behavior's `context.include` (currently excluded), or make its inclusion configurable, so Amplifier agents spawned *by* wg also get the protocol even if not using the executor prompt template

### Summary

There is no protocol adoption needed in either direction. The systems already share the same context mechanisms (wg's template variables + artifact/log system). The work is in **enriching wg's native context pipeline** (which amplifier inherits automatically) and **fixing the executor stdin limitation** (which removes amplifier's main workaround).
