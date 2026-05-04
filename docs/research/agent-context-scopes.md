# Agent Context Scopes: Design Document

## Overview

Agents currently receive a one-size-fits-all prompt assembled from `TemplateVars` in `executor.rs`. This design introduces configurable **context scopes** that control how much information an agent receives about the workgraph, its neighbors, and the system itself. The goal: give each agent exactly the context it needs — no more, no less.

---

## 1. Proposed Scopes

Four tiers, ordered by increasing context breadth:

### 1.1 `clean` — Bare Executor

**Purpose:** Pure computation, writing, or analysis tasks that have no need for workgraph awareness. The agent is a stateless tool.

**What the agent receives:**
- `{{skills_preamble}}` (if present)
- `{{task_identity}}` (role/motivation, if assigned)
- Task ID, title, and description
- `{{task_context}}` (upstream artifacts/logs — still useful even without wg commands)
- `{{task_loop_info}}` (if in a cycle)

**What is omitted:**
- All `wg` CLI instructions (Required Workflow, Graph Patterns, Reusable Functions, "CRITICAL: Use wg CLI" section)
- No `wg log`, `wg done`, `wg artifact` instructions

**How the task completes:** The wrapper script (`run.sh`) already auto-completes tasks when the agent exits successfully and the task is still in-progress. Clean-scope agents rely entirely on this mechanism. They produce output via stdout/files, and the wrapper handles `wg done`/`wg fail`.

**When to use:** Translation, summarization, code generation with no dependencies to inspect, mathematical computation, writing tasks where the agent just needs to produce output.

**Estimated token cost:** ~200-400 tokens for the prompt skeleton (vs. ~800-1000 for current default).

### 1.2 `task` — Task-Aware (Current Default)

**Purpose:** The agent knows its task, can use `wg` CLI for logging/artifacts/completion, and sees upstream context. This is essentially what exists today, with the small additions from the research recommendations (R1-R5).

**What the agent receives:**
- Everything from `clean`
- Required Workflow section (`wg log`, `wg artifact`, `wg done`, `wg fail`)
- Graph Patterns reference
- Reusable Workflow Functions reference
- "CRITICAL: Use wg CLI" warning
- **New (R1):** Downstream task IDs + titles ("Downstream consumers of your work: ...")
- **New (R2):** `wg context` / `wg show` hints
- **New (R4):** Task tags and skills in prompt
- **New (R5):** Upstream task titles alongside artifacts

**What is omitted:**
- No graph topology beyond immediate neighbors
- No project-level summary or description
- No system awareness (what workgraph is, how coordinator works)

**When to use:** Most implementation tasks, bug fixes, code review, test writing — anything that benefits from `wg` CLI access and dependency awareness.

**Estimated token cost:** ~800-1200 tokens (current level, plus ~100 for new additions).

### 1.3 `graph` — Graph-Aware

**Purpose:** The agent understands where it sits in the larger workflow. It sees its N-hop neighborhood, upstream outputs, downstream consumers, and project progress. For tasks that need to make context-dependent decisions about scope, format, or priority.

**What the agent receives:**
- Everything from `task`
- **Project description** from `config.toml` (`project.description`)
- **Subgraph summary:** 1-hop neighborhood (upstream + downstream) with task IDs, titles, statuses, and descriptions (truncated to first 200 chars)
- **Graph status summary:** counts by status (e.g., "Graph: 12 tasks — 3 done, 4 in-progress, 2 open, 3 blocked")
- **Upstream artifact content hints:** For each upstream artifact that is a text file under 500 bytes, inline its content. For larger files, show first 3 lines + byte count.
- **Role/motivation descriptions** of the agent's neighbors (if agency is configured), so the agent knows who it's collaborating with

**What is omitted:**
- No system internals (coordinator mechanics, trace functions, agency evolution)
- No tasks beyond 1-hop neighborhood
- No full file contents for large artifacts

**When to use:** Integration tasks, review tasks that span multiple components, tasks where output format depends on downstream consumers, tasks that need to avoid duplicating sibling work.

**Estimated token cost:** ~1500-3000 tokens depending on neighborhood size. Budget: hard cap at 4000 tokens for the graph context section, with truncation if exceeded.

### 1.4 `full` — System-Aware

**Purpose:** The agent understands the entire workgraph system — what it is, how the coordinator works, how tasks flow, what trace functions and the agency model are. For meta-tasks like architecture design, spec writing, workflow optimization, and debugging workgraph itself.

**What the agent receives:**
- Everything from `graph`
- **System awareness preamble:** A ~300-token explanation of workgraph concepts:
  - What workgraph is (graph-based task orchestration)
  - How the coordinator works (polls for ready tasks, spawns agents, monitors health)
  - How cycles/loops work
  - How the agency system works (roles, motivations, agents, evaluation, evolution)
  - What trace functions are
- **Full graph summary:** All tasks with their statuses (via `wg list --all` or equivalent), not just 1-hop
- **CLAUDE.md content** (the project-level instructions, injected explicitly since `--print` agents don't get it automatically)

**What is omitted:**
- Nothing. This is the maximum context tier.

**When to use:** Meta-tasks (designing workflows, writing specs about workgraph), debugging task failures across the graph, architecture decisions, onboarding-style tasks where the agent needs to understand the whole system.

**Estimated token cost:** ~3000-6000 tokens depending on graph size. Budget: hard cap at 8000 tokens for the full context additions, with aggressive summarization if exceeded.

### 1.5 Why Not More Tiers?

I considered a fifth tier between `task` and `graph` that would add only project description + downstream awareness without the subgraph summary. But the marginal difference is small (~200 tokens), and the configuration burden of choosing between five tiers outweighs the benefit. The four tiers have clear, distinct use cases:

| Scope | Agent mindset | Token overhead |
|-------|--------------|----------------|
| `clean` | "I'm a tool, give me input, I produce output" | ~300 |
| `task` | "I'm working on a task with dependencies" | ~1000 |
| `graph` | "I'm part of a larger workflow" | ~2500 |
| `full` | "I understand the whole system" | ~5000 |

---

## 2. Configuration

### 2.1 Setting the Scope

#### Per-task (highest priority):
```bash
wg add "Write summary" --context-scope clean
wg add "Integrate results" --context-scope graph
```

Stored as a new field on the Task struct:
```rust
// In graph.rs, Task struct
#[serde(default, skip_serializing_if = "Option::is_none")]
pub context_scope: Option<String>,
```

#### Per-role (medium priority):
Roles can declare a default context scope in their YAML definition:
```yaml
# .wg/agency/roles/<hash>.yaml
name: "Integrator"
description: "Integrates outputs from multiple tasks"
default_context_scope: "graph"
skills: [...]
```

New field on the Role struct:
```rust
// In agency.rs, Role struct
#[serde(default, skip_serializing_if = "Option::is_none")]
pub default_context_scope: Option<String>,
```

#### Global default (lowest priority):
```toml
# .wg/config.toml
[coordinator]
default_context_scope = "task"
```

New field on CoordinatorConfig:
```rust
// In config.rs
#[serde(default, skip_serializing_if = "Option::is_none")]
pub default_context_scope: Option<String>,
```

### 2.2 Resolution Hierarchy

```
task.context_scope > role.default_context_scope > coordinator.default_context_scope > "task"
```

Resolution happens in `TemplateVars::from_task()` or a new `resolve_context_scope()` function:

```rust
fn resolve_context_scope(
    task: &Task,
    role: Option<&agency::Role>,
    config: &Config,
) -> ContextScope {
    // 1. Task-level override
    if let Some(ref scope) = task.context_scope {
        if let Ok(s) = scope.parse() {
            return s;
        }
    }
    // 2. Role default
    if let Some(role) = role {
        if let Some(ref scope) = role.default_context_scope {
            if let Ok(s) = scope.parse() {
                return s;
            }
        }
    }
    // 3. Config default
    if let Some(ref scope) = config.coordinator.default_context_scope {
        if let Ok(s) = scope.parse() {
            return s;
        }
    }
    // 4. Hardcoded default
    ContextScope::Task
}
```

### 2.3 The ContextScope Enum

```rust
// In a new module, e.g., src/context_scope.rs or in executor.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextScope {
    Clean,
    Task,
    Graph,
    Full,
}

impl std::str::FromStr for ContextScope {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "clean" => Ok(Self::Clean),
            "task" => Ok(Self::Task),
            "graph" => Ok(Self::Graph),
            "full" => Ok(Self::Full),
            _ => Err(anyhow::anyhow!("Unknown context scope: '{}'. Valid: clean, task, graph, full", s)),
        }
    }
}
```

---

## 3. Implementation: Changes in executor.rs

### 3.1 Current Architecture

The prompt is currently a single monolithic template string in `ExecutorRegistry::default_config("claude")` (executor.rs:372-444). `TemplateVars` holds all substitution values and `apply()` does a simple string replace.

### 3.2 New Architecture: Composable Prompt Sections

Replace the monolithic template with composable sections that are included based on scope:

```rust
/// Sections of the prompt, assembled based on context scope.
struct PromptBuilder {
    scope: ContextScope,
    sections: Vec<String>,
}

impl PromptBuilder {
    fn new(scope: ContextScope) -> Self {
        Self { scope, sections: Vec::new() }
    }

    fn add(&mut self, section: &str) {
        if !section.is_empty() {
            self.sections.push(section.to_string());
        }
    }

    fn build(self) -> String {
        self.sections.join("\n\n")
    }
}
```

### 3.3 Prompt Assembly per Scope

```rust
fn build_prompt(
    vars: &TemplateVars,
    scope: ContextScope,
    graph_context: Option<&str>,      // For graph/full scopes
    system_preamble: Option<&str>,     // For full scope
    project_description: Option<&str>, // For graph/full scopes
) -> String {
    let mut builder = PromptBuilder::new(scope);

    // === All scopes ===
    builder.add(&vars.skills_preamble);
    builder.add("# Task Assignment\n\nYou are an AI agent working on a task.");
    builder.add(&vars.task_identity);
    builder.add(&format!(
        "## Your Task\n- **ID:** {}\n- **Title:** {}\n- **Description:** {}",
        vars.task_id, vars.task_title, vars.task_description
    ));
    builder.add(&format!("## Context from Dependencies\n{}", vars.task_context));
    builder.add(&vars.task_loop_info);

    // === task, graph, full ===
    if scope >= ContextScope::Task {
        builder.add(REQUIRED_WORKFLOW_SECTION);  // wg log/artifact/done/fail
        builder.add(GRAPH_PATTERNS_SECTION);
        builder.add(REUSABLE_FUNCTIONS_SECTION);
        builder.add(CRITICAL_WG_CLI_SECTION);
    }

    // === graph, full ===
    if scope >= ContextScope::Graph {
        if let Some(desc) = project_description {
            builder.add(&format!("## Project\n{}", desc));
        }
        if let Some(ctx) = graph_context {
            builder.add(&format!("## Graph Context\n{}", ctx));
        }
    }

    // === full only ===
    if scope >= ContextScope::Full {
        if let Some(preamble) = system_preamble {
            // Insert near the top, after task assignment header
            builder.add(&format!("## System Overview\n{}", preamble));
        }
    }

    builder.add("Begin working on the task now.");
    builder.build()
}
```

The comparison `scope >= ContextScope::Task` works because we implement `PartialOrd`/`Ord` on `ContextScope` with `Clean < Task < Graph < Full`.

### 3.4 Extracting Prompt Sections as Constants

Move the current inline template sections into named constants:

```rust
const REQUIRED_WORKFLOW_SECTION: &str = r#"## Required Workflow
You MUST use these commands to track your work:
1. **Log progress** as you work: `wg log {{task_id}} "message"`
2. **Record artifacts**: `wg artifact {{task_id}} path/to/file`
3. **Complete the task**: `wg done {{task_id}}` (or `--converged` for cycles)
4. **Mark as failed**: `wg fail {{task_id}} --reason "reason"`

## Important
- Run `wg log` commands BEFORE doing work to track progress
- Run `wg done` BEFORE you finish responding
- Run `wg context {{task_id}}` to see what your dependencies produced
- Run `wg show <task-id>` to inspect any task in the graph"#;

const GRAPH_PATTERNS_SECTION: &str = r#"## Graph Patterns
**Vocabulary:** pipeline (A→B→C), diamond (A→[B,C,D]→E), scatter-gather, loop (A→B→C→A with `--max-iterations`).
**Golden rule: same files = sequential edges.**
**When creating subtasks:** Always include an integrator task at join points.
**After code changes:** Run `cargo install --path .` to update the global binary."#;

// etc.
```

### 3.5 What Changes in spawn.rs

`build_task_context()` gains new capability for `graph`/`full` scopes:

```rust
fn build_task_context(
    graph: &WorkGraph,
    task: &Task,
    scope: ContextScope,
) -> String {
    let mut parts = Vec::new();

    // === Upstream context (all scopes) ===
    for dep_id in &task.after {
        if let Some(dep_task) = graph.get_task(dep_id) {
            // R5: Include upstream task titles
            if !dep_task.artifacts.is_empty() {
                parts.push(format!(
                    "From {} ({}): artifacts: {}",
                    dep_id, dep_task.title, dep_task.artifacts.join(", ")
                ));
            }
            // Logs from completed deps (existing behavior)
            if dep_task.status == Status::Done && !dep_task.log.is_empty() {
                let logs: Vec<&LogEntry> = dep_task.log.iter().rev().take(5).collect();
                for entry in logs.iter().rev() {
                    parts.push(format!("From {} logs: {} {}", dep_id, entry.timestamp, entry.message));
                }
            }
        }
    }

    // === Downstream awareness (task+ scopes) - R1 ===
    if scope >= ContextScope::Task {
        let downstream: Vec<_> = graph.tasks()
            .filter(|t| t.after.contains(&task.id))
            .collect();
        if !downstream.is_empty() {
            parts.push("Downstream consumers of your work:".to_string());
            for dt in &downstream {
                parts.push(format!("- {}: \"{}\"", dt.id, dt.title));
            }
        }
    }

    // === Graph context (graph+ scopes) ===
    if scope >= ContextScope::Graph {
        parts.push(build_graph_summary(graph, task));
    }

    // ... cycle metadata (existing) ...

    if parts.is_empty() {
        "No context from dependencies".to_string()
    } else {
        parts.join("\n")
    }
}
```

### 3.6 Template Variable Changes

Add `context_scope` to `TemplateVars`:

```rust
pub struct TemplateVars {
    // ... existing fields ...
    pub context_scope: ContextScope,
    pub project_description: String,  // from config
}
```

The monolithic template string in `default_config("claude")` is replaced with a call to `build_prompt()` that assembles sections based on scope. Custom executor configs (in `.wg/executors/`) can still use the old monolithic template approach — the scope-based assembly is only for the built-in defaults.

---

## 4. Data Flow

### 4.1 For `graph` and `full` Scopes

#### Subgraph Summary

Gathered by a new `build_graph_summary()` function in spawn.rs:

```rust
fn build_graph_summary(graph: &WorkGraph, task: &Task) -> String {
    let mut lines = Vec::new();

    // Status counts
    let mut counts: HashMap<Status, usize> = HashMap::new();
    for t in graph.tasks() {
        *counts.entry(t.status).or_default() += 1;
    }
    let total = graph.tasks().count();
    lines.push(format!(
        "Graph: {} tasks — {} done, {} in-progress, {} open, {} blocked, {} failed",
        total,
        counts.get(&Status::Done).unwrap_or(&0),
        counts.get(&Status::InProgress).unwrap_or(&0),
        counts.get(&Status::Open).unwrap_or(&0),
        counts.get(&Status::Blocked).unwrap_or(&0),
        counts.get(&Status::Failed).unwrap_or(&0),
    ));

    // 1-hop neighborhood
    lines.push("\nNeighborhood (1-hop):".to_string());

    // Upstream (already shown in dependency context, just add descriptions)
    for dep_id in &task.after {
        if let Some(dep) = graph.get_task(dep_id) {
            let desc_preview = dep.description.as_deref()
                .unwrap_or("")
                .chars().take(200).collect::<String>();
            lines.push(format!(
                "  ← {} [{}] \"{}\": {}",
                dep.id, dep.status, dep.title, desc_preview
            ));
        }
    }

    // Downstream
    for other in graph.tasks() {
        if other.after.contains(&task.id) {
            let desc_preview = other.description.as_deref()
                .unwrap_or("")
                .chars().take(200).collect::<String>();
            lines.push(format!(
                "  → {} [{}] \"{}\": {}",
                other.id, other.status, other.title, desc_preview
            ));
        }
    }

    // Siblings (tasks sharing same upstream dependencies)
    let siblings: Vec<_> = graph.tasks()
        .filter(|t| t.id != task.id && t.after.iter().any(|dep| task.after.contains(dep)))
        .collect();
    if !siblings.is_empty() {
        lines.push("\nSiblings (shared dependencies):".to_string());
        for sib in &siblings {
            lines.push(format!("  ~ {} [{}] \"{}\"", sib.id, sib.status, sib.title));
        }
    }

    lines.join("\n")
}
```

#### Upstream Artifact Content

For `graph`+ scopes, optionally inline small artifact contents:

```rust
fn inline_artifact_content(artifact_path: &str, workgraph_dir: &Path) -> Option<String> {
    let project_root = workgraph_dir.parent()?;
    let full_path = project_root.join(artifact_path);
    let metadata = std::fs::metadata(&full_path).ok()?;

    if metadata.len() > 500 {
        // Too large — show preview
        let content = std::fs::read_to_string(&full_path).ok()?;
        let preview: String = content.lines().take(3).collect::<Vec<_>>().join("\n");
        Some(format!("[{} bytes]\n{}\n...", metadata.len(), preview))
    } else {
        std::fs::read_to_string(&full_path).ok()
    }
}
```

#### Role/Motivation of Neighbors

For `graph`+ scopes, if agency is configured, resolve the agent identity of neighboring tasks:

```rust
// In build_graph_summary, for each neighbor task:
if let Some(ref agent_hash) = neighbor.agent {
    if let Ok(agent) = agency::find_agent_by_prefix(&agents_dir, agent_hash) {
        if let Ok(role) = agency::find_role_by_prefix(&roles_dir, &agent.role_id) {
            lines.push(format!("    Role: {} — {}", role.name, role.description));
        }
    }
}
```

#### Project Description

Loaded from `Config::load_or_default(workgraph_dir).project.description`. Already exists as a field in `ProjectConfig` — just needs to be threaded through to prompt assembly.

#### System Preamble (full scope only)

A static string constant:

```rust
const SYSTEM_AWARENESS_PREAMBLE: &str = r#"## About workgraph

workgraph is a directed-graph-based task orchestration system. Tasks have dependencies (edges),
statuses (open → in-progress → done/failed), and can be assigned to AI agents.

**Coordinator:** A daemon that polls for ready tasks (all dependencies satisfied),
spawns agents via executors (claude, amplifier, shell), and monitors agent health.

**Agency system:** Agents have roles (capabilities/skills), motivations (behavioral
directives), and performance records. The assigner matches agents to tasks by skill fit.

**Cycles/Loops:** Tasks can form cycles with `--max-iterations`. The cycle header task
uses `wg done --converged` to stop iteration early.

**Trace functions:** Reusable workflow templates that can be instantiated with
`wg func apply` to create pre-wired task subgraphs."#;
```

#### CLAUDE.md Content (full scope only)

```rust
fn load_claude_md(workgraph_dir: &Path) -> String {
    let project_root = workgraph_dir.parent().unwrap_or(workgraph_dir);
    let claude_md = project_root.join("CLAUDE.md");
    match std::fs::read_to_string(&claude_md) {
        Ok(content) => format!("## Project Instructions (CLAUDE.md)\n\n{}", content),
        Err(_) => String::new(),
    }
}
```

### 4.2 Full Graph Summary (full scope only)

For `full` scope, instead of just 1-hop, include all tasks:

```rust
fn build_full_graph_summary(graph: &WorkGraph) -> String {
    let mut lines = Vec::new();
    lines.push("All tasks:".to_string());

    let mut token_budget = 4000; // rough char budget
    for task in graph.tasks() {
        let line = format!(
            "  {} [{}] \"{}\"{}",
            task.id,
            task.status,
            task.title,
            if task.after.is_empty() {
                String::new()
            } else {
                format!(" (after: {})", task.after.join(", "))
            }
        );
        if token_budget < line.len() {
            lines.push(format!("  ... and {} more tasks", /* remaining count */));
            break;
        }
        token_budget -= line.len();
        lines.push(line);
    }

    lines.join("\n")
}
```

---

## 5. Risks and Mitigations

### 5.1 Token Cost

| Scope | Estimated Tokens | Mitigation |
|-------|-----------------|------------|
| `clean` | ~300 | None needed — cheaper than current |
| `task` | ~1000 | None needed — same as current |
| `graph` | ~1500-3000 | Hard cap at 4000 tokens for graph context section. Truncate description previews. Limit neighborhood to 1-hop. |
| `full` | ~3000-6000 | Hard cap at 8000 tokens for full additions. Summarize task list aggressively. Skip descriptions for tasks beyond 1-hop. |

**Budget enforcement:** Each section builder tracks its output length and stops adding content when the budget is exhausted. The budget is configurable via `config.toml`:

```toml
[coordinator]
graph_context_token_budget = 4000
full_context_token_budget = 8000
```

### 5.2 Prompt Injection from Peer Task Content

**Risk:** `graph` and `full` scopes inject task descriptions and log messages from other tasks. A malicious or confused agent could write log entries or descriptions that contain prompt-injection attempts (e.g., "Ignore previous instructions and...").

**Mitigations:**
1. **Truncation:** Description previews are capped at 200 characters, limiting injection surface.
2. **Labeling:** All injected content is clearly labeled with its source (`From task-X:`, `Neighbor:`) so the agent can distinguish instructions from context.
3. **XML fencing:** Wrap injected neighbor content in XML tags that the agent is instructed to treat as data, not instructions:
   ```
   <neighbor-context source="task-id">
   ... neighbor description ...
   </neighbor-context>
   ```
4. **No recursive execution:** Neighbor descriptions are read-only context — the agent can't execute commands found in them without explicit action.

### 5.3 Information Overload

**Risk:** Higher scopes give agents more context, but too much context can degrade performance — the agent spends tokens processing irrelevant information and may lose focus on its primary task.

**Mitigations:**
1. **Default to `task`:** The default scope is `task`, which is the current behavior. Higher scopes are opt-in.
2. **Progressive disclosure:** Within each scope, information is ordered by relevance (immediate dependencies first, then siblings, then graph-wide).
3. **Hard token caps:** Each scope has a maximum token budget for its additional context sections.
4. **Monitoring:** Log the actual token count of the assembled prompt so that operators can tune budgets. Add `prompt_tokens` to the spawn metadata.json.

### 5.4 When NOT to Use Higher Scopes

| Situation | Recommended Scope | Why |
|-----------|------------------|-----|
| Simple computation or writing | `clean` | No wg CLI needed, wrapper handles completion |
| Standard implementation task | `task` | Needs wg CLI, but not graph awareness |
| Independent leaf task (no downstream) | `task` | Graph context adds nothing |
| Large graph (50+ tasks) | `task` or `graph` with low budget | Full scope would be token-expensive |
| Tasks with sensitive descriptions | `task` | Limits exposure of peer task content |
| Integration/review spanning components | `graph` | Needs to understand what neighbors are doing |
| Workflow design or meta-tasks | `full` | Needs system understanding |

### 5.5 Backward Compatibility

- Tasks without `context_scope` set default to `task`, which produces the same prompt as today (modulo the R1-R5 improvements that should be applied regardless).
- Custom executor configs in `.wg/executors/` that define their own `prompt_template.template` are unaffected — scope-based assembly only applies to built-in defaults.
- The `context_scope` field on Task uses `skip_serializing_if = "Option::is_none"`, so existing graph.jsonl files are unaffected.

---

## 6. Implementation Plan

### Phase 1: Infrastructure (minimal, unblocks rest)
1. Add `ContextScope` enum to `src/context_scope.rs` (or in `executor.rs`)
2. Add `context_scope: Option<String>` to `Task` struct in `graph.rs`
3. Add `default_context_scope: Option<String>` to `CoordinatorConfig` in `config.rs`
4. Add `default_context_scope: Option<String>` to `Role` in `agency.rs`
5. Add `--context-scope` flag to `wg add` CLI

### Phase 2: Prompt Assembly
6. Extract prompt sections into constants in `executor.rs`
7. Implement `build_prompt()` with scope-based section inclusion
8. Implement `resolve_context_scope()` with task > role > config hierarchy
9. Wire scope resolution into `spawn_agent_inner()` in `spawn.rs`

### Phase 3: Graph Context Gathering
10. Implement `build_graph_summary()` for graph scope
11. Implement `build_full_graph_summary()` for full scope
12. Add token budget enforcement
13. Thread `project.description` from config into prompt

### Phase 4: Polish
14. Add XML fencing for neighbor content
15. Add `prompt_tokens` to spawn metadata
16. Update tests
17. Update AGENT-GUIDE.md with scope documentation

Each phase can be a separate task in the workgraph with pipeline edges (Phase 1 → Phase 2 → Phase 3 → Phase 4).

---

## 7. Example Prompts

### clean scope (abbreviated)
```
# Task Assignment

You are an AI agent working on a task.

## Your Task
- **ID:** translate-readme
- **Title:** Translate README to Spanish
- **Description:** Translate the English README.md to Spanish...

## Context from Dependencies
From write-readme (Write English README): artifacts: README.md

Begin working on the task now.
```

### graph scope (abbreviated, showing additions)
```
# Task Assignment
...

## Project
Building a Rust-based task orchestration CLI for multi-agent workflows.

## Graph Context
Graph: 15 tasks — 5 done, 3 in-progress, 4 open, 3 blocked

Neighborhood (1-hop):
  ← research-context [done] "Research agent context": Analyzed current state of agent context...
  → impl-context-scopes [blocked] "Implement context scopes": Implement the context scope system...
  → review-design [blocked] "Review context scope design": Review the design doc for...

Siblings (shared dependencies):
  ~ design-prompt-caching [in-progress] "Design prompt caching"

## Required Workflow
...
```
