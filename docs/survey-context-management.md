# Context Management Survey: wg

**Date**: 2026-02-05
**Scope**: Full audit of context management, scoping, and information flow

---

## Executive Summary

wg's context management is **conceptually sound but operationally minimal**. The system provides a clean pipeline: dependency artifacts → template variables → prompt injection, with optional identity context from the agency system. However, it lacks enforcement (context_limit is defined but never checked), size management (no truncation, summarization, or budgeting), and content awareness (artifacts are path strings, not content). The biggest risks are **unbounded skill content** from URL/file sources and **unused context_limit** on actors.

---

## 1. Context Passing Mechanisms

### 1.1 Dependency Artifact Context

**Pipeline**: `blocked_by tasks` → `build_task_context()` → `TemplateVars.task_context` → `{{task_context}}` in prompt

**Source**: `src/commands/spawn.rs:84-108`

```
Task A (done) ─── artifacts: ["src/foo.rs", "tests/foo_test.rs"]
    │
    ▼
Task B (spawning, blocked_by: ["task-a"])
    │
    ▼
build_task_context() → "From task-a: artifacts: src/foo.rs, tests/foo_test.rs"
    │
    ▼
Prompt: "## Context from Dependencies\nFrom task-a: artifacts: src/foo.rs, tests/foo_test.rs"
```

**Characteristics**:
- Only direct dependencies examined (not transitive)
- Artifact paths listed, **not file content**
- Agents must read files themselves
- No validation that artifacts exist on disk
- Format: `"From {dep_id}: artifacts: {comma-separated paths}"`

### 1.2 Template Variable System

**Source**: `src/service/executor.rs:17-89`

```rust
pub struct TemplateVars {
    pub task_id: String,
    pub task_title: String,
    pub task_description: String,
    pub task_context: String,      // From dependency artifacts
    pub task_identity: String,     // Rendered role+motivation+skills
}
```

**Resolution**: Simple `{{placeholder}}` string replacement via `apply()` method. No conditionals, loops, escaping, or lazy evaluation. All templates resolved at spawn time.

**Applied to**: Prompt template, command args, environment variables, working directory.

### 1.3 Context Command

**Source**: `src/commands/context.rs:30-125`

The `wg context <task>` command provides static analysis of context availability:
- Collects artifacts from `blocked_by` dependencies
- Identifies missing inputs (declared but no matching artifact)
- Reverse query shows what downstream tasks need from this task
- **Advisory only** — does not block execution with missing inputs

### 1.4 Shell Executor Context

**Source**: `src/service/shell.rs:16-30, 163-169`

For non-LLM executors, context passes via environment variables:
- `WG_TASK_ID`, `WG_TASK_TITLE`, `WG_TASK_DESCRIPTION`
- `WG_TASK_CONTEXT` (same artifact list string)
- `WG_TASK_EXEC`, `WG_WORKDIR`

---

## 2. Scoping Boundaries

### 2.1 What an Agent Sees When Spawned

Full prompt structure (`src/service/executor.rs:440-502`, `src/service/claude.rs:18-40`):

```
1. Static preamble              (~200 chars, fixed)
2. {{task_identity}}            (variable: 0 to 50KB+)
   ├── Role name + description
   ├── All resolved skills      ← UNBOUNDED
   ├── Desired outcome
   ├── Acceptable trade-offs
   └── Non-negotiable constraints
3. Task ID, Title               (user-defined)
4. {{task_description}}         (user-defined)
5. {{task_context}}             (artifact paths from deps)
6. Workflow instructions        (~500 chars, fixed)
```

### 2.2 What Gets Excluded

| Excluded Information | Impact |
|---|---|
| Dependency task descriptions | Agent doesn't know *what* upstream tasks did, only what files they produced |
| Dependency task logs | No visibility into how artifacts were created |
| Transitive dependencies | Can't access "grandparent" artifacts unless explicitly chained |
| Task verification criteria | Agent sees description but not verification expectations |
| Performance history | Role/motivation performance records not in prompt |
| Other agents' work in progress | No visibility into parallel work |

### 2.3 Assessment

**Too narrow**: Agents receive artifact *paths* but not *descriptions* of what upstream tasks accomplished. An agent working on task C (blocked by A→B) has no idea what task A produced unless B explicitly propagated those artifacts.

**Too broad**: Identity context with multiple resolved skills can inject 30-50KB of content that may be generic guidance rather than task-specific instruction. An agent solving a simple bug gets the same full role+skills prompt as one doing a major refactor.

---

## 3. Context Size and Limits

### 3.1 The `context_limit` Field

**Defined**: `src/graph.rs:170-173` on Actor struct, settable via `wg actor add --context-limit`

**Enforced**: **NOWHERE**

The field is:
- ✅ Stored in the graph
- ✅ Serialized/deserialized
- ✅ Displayed in actor info
- ❌ Never checked before spawn
- ❌ Never used in trajectory planning
- ❌ Never used for truncation

### 3.2 No Truncation or Prioritization

There is no:
- Token counting before prompt assembly
- Content truncation when context exceeds limits
- Priority ordering of context sections
- Summarization of large context blocks
- Warning when prompts are unusually large

### 3.3 Actual Size Risks

| Component | Typical Size | Max Size | Controlled? |
|---|---|---|---|
| Static template | ~700 chars | Fixed | ✅ Yes |
| Role description | ~500 chars | Unbounded | ❌ No |
| Motivation | ~400 chars | Unbounded | ❌ No |
| **URL-fetched skill** | **5-7KB each** | **UNLIMITED** | ❌ No |
| **File-based skill** | **5-7KB each** | **UNLIMITED** | ❌ No |
| Inline skill | User-defined | User-defined | ⚠️ Manual |
| Task description | ~200-2000 chars | Unbounded | ❌ No |
| Dependency context | ~50 chars/dep | Bounded by graph | ✅ Effectively |

**Worst case**: A role with 5 evolver-generated skills (observed in `.wg/agency/evolver-skills/`, each 5-7KB) adds **~35KB** to every prompt using that role. With URL-fetched skills pointing to external docs, there is **no upper bound**.

---

## 4. Trajectory-Based Context

### 4.1 How Trajectory Plans Paths

**Source**: `src/commands/trajectory.rs:50-135`

The trajectory system uses BFS from a root task to find context-efficient paths:

1. Build reverse dependency index
2. BFS from root, following dependency edges
3. For each step, track what it `receives` (predecessor artifacts) and `produces` (deliverables + artifacts)
4. Include tasks where inputs intersect with predecessor's produces

### 4.2 Context in Trajectory Scoring

**Source**: `src/commands/trajectory.rs:218-252`

Trajectory suggestions score actor-task fit:
- Skill match: +10 per matched skill, -3 per missing
- Perfect match bonus: +20
- **Context flow bonus: +5** (binary: has receives or not)
- Trust level: +5 for verified actors

### 4.3 Context Loss in Task Chains

**Problem**: Agents working on tasks later in a chain only see artifacts from *direct* dependencies, not the full chain history.

```
Task A → produces artifacts → Task B → produces artifacts → Task C
                                                              │
                                                              └── Sees Task B artifacts only
                                                                  Task A artifacts invisible
```

If Task B consumes but doesn't re-export Task A's artifacts, Task C loses that context entirely. There is no mechanism for:
- Transitive artifact propagation
- Context summaries from earlier in the chain
- "Project memory" that accumulates across a trajectory

### 4.4 Assessment

Trajectory planning is **topology-aware but not context-size-aware**. It finds good paths based on dependency structure and skill matching but:
- Doesn't measure total context size per step
- Doesn't optimize for context window fitting
- Doesn't consider context reuse across consecutive tasks
- Binary context bonus (+5) doesn't distinguish quality/quantity

---

## 5. Artifact-Mediated Data Flow

### 5.1 The Inputs/Deliverables/Artifacts Model

**Source**: `src/graph.rs:65-72`

```
inputs:       What a task declares it needs (before execution)
deliverables: What a task declares it will produce (before execution)
artifacts:    What a task actually produced (after execution)
```

### 5.2 How Artifacts Bridge Tasks

1. Task A completes, agent runs `wg artifact task-a src/module.rs`
2. Task B (blocked_by: [task-a]) becomes ready
3. `build_task_context()` collects: `"From task-a: artifacts: src/module.rs"`
4. Agent for Task B receives this path in prompt, reads file itself

### 5.3 Limitations of Current Model

**Artifacts are opaque strings**:
- No content hash, size, format, or creation timestamp
- No validation that path exists or is readable
- No semantic matching between `inputs` and `artifacts`
- No distinction between code files, data files, documentation

**Missing metadata would enable**:
```rust
// Current
artifacts: Vec<String>  // ["src/foo.rs", "data/output.json"]

// With metadata (proposed)
struct Artifact {
    path: String,
    content_hash: String,
    size_bytes: u64,
    format: Option<String>,    // "rust", "json", "markdown"
    created_at: String,
    summary: Option<String>,   // One-line description
}
```

**No lifecycle management**: Artifacts never expire, aren't versioned, and have no conflict resolution if a dependency re-runs.

---

## 6. Identity Context

### 6.1 Identity Prompt Structure

**Source**: `src/agency.rs:245-283`

When a task has an `identity` (role + motivation), the rendered prompt block is:

```markdown
## Agent Identity

### Role: {name}
{description}

#### Skills
### {skill.name}
{skill.content}
[...repeated for each resolved skill...]

#### Desired Outcome
{desired_outcome}

### Operational Parameters

#### Acceptable Trade-offs
- {tradeoff1}
- {tradeoff2}

#### Non-negotiable Constraints
- {constraint1}
- {constraint2}
```

### 6.2 Skill Resolution

**Source**: `src/agency.rs:163-236`

| Skill Type | Resolution | Size Risk |
|---|---|---|
| `Name(string)` | Returns name as content | Minimal (~20 chars) |
| `File(path)` | Reads entire file | **Unbounded** |
| `Url(string)` | HTTP GET, full body | **Unbounded** |
| `Inline(string)` | Returns embedded content | User-controlled |

**URL skills** (`src/agency.rs:201-220`): No size limit, no timeout override, no caching, no truncation. Failed resolution prints warning but doesn't abort.

### 6.3 Context Bloat from Skills

Observed in `.wg/agency/evolver-skills/`:
- `gap-analysis.md`: 6.7KB
- `motivation-tuning.md`: 7.2KB
- `retirement.md`: 5.6KB
- `role-crossover.md`: 5.9KB
- `role-mutation.md`: 5.6KB
- **Total**: ~31KB for 5 skills

The evolver system creates roles programmatically and can reference these skills. A role with all 5 evolver skills adds **31KB to every agent prompt** using that role.

### 6.4 Auto-Assignment Ignores Size

**Source**: `src/commands/assign.rs:109-164`, `src/agency.rs:699-843`

Role matching scores by keyword overlap (30%), skill overlap (40%), tag matching (10%), and historical performance (20%). **No consideration of resulting prompt size**.

---

## 7. Gaps and Opportunities

### 7.1 Where Context Is Lost

| Gap | Location | Impact |
|---|---|---|
| **Transitive dependencies invisible** | spawn.rs:84-108 | Tasks can't see grandparent artifacts |
| **No dependency descriptions** | spawn.rs build_task_context | Agent doesn't know what upstream tasks *did* |
| **No task logs in context** | spawn.rs, executor.rs | Can't see how artifacts were created |
| **Artifact existence not verified** | artifact.rs | Agent may receive paths to deleted files |
| **Context lost on re-spawn** | claude.rs spawn() | If agent fails and retries, no memory of prior attempt |

### 7.2 Where Irrelevant Context Is Included

| Bloat Source | Location | Impact |
|---|---|---|
| **Full skill content for all role skills** | agency.rs:245-283 | 5-35KB of potentially generic guidance |
| **All trade-offs/constraints listed** | agency.rs:268-280 | May not be relevant to specific task |
| **Fixed workflow instructions** | executor.rs:460-497 | Same boilerplate on every task (~500 chars) |
| **All dependency artifacts** | spawn.rs:84-108 | No filtering by task's declared inputs |

### 7.3 Recommendations

#### Critical (Should implement first)

1. **Enforce `context_limit`**: The field exists on Actor. Add a check in `spawn.rs` that measures prompt size and warns/fails if it exceeds the limit. Minimally: `prompt.len()` check. Better: approximate token count.

2. **Skill size limits**: Add a `max_skill_size` config option (default 8KB). Truncate skills that exceed it with a `[truncated at {n} bytes]` marker. This prevents unbounded URL/file content.

3. **Filter dependency artifacts by inputs**: In `build_task_context()`, if the task declares `inputs`, only include artifacts that match those input paths rather than dumping all artifacts from all dependencies.

#### High Priority

4. **Artifact metadata**: Add `size_bytes` and `summary` fields to artifacts. The summary (one line) can be included in context instead of requiring agents to read every file.

5. **Transitive context option**: Add a `--depth` or `context_depth` setting for how many levels of dependency artifacts to include. Default 1 (current behavior), but allow 2-3 for complex chains.

6. **Context budget allocation**: Implement a simple budget system:
   ```
   Total budget: context_limit tokens
   - Fixed overhead: ~200 tokens (template chrome)
   - Identity budget: 30% of remaining
   - Task description: 20% of remaining
   - Dependency context: 50% of remaining
   ```
   Truncate sections that exceed their budget.

#### Medium Priority

7. **Dependency summaries**: When building task context, include a one-line summary of each dependency task (title or first line of description) alongside artifact paths. This gives agents understanding of *what* upstream tasks accomplished.

8. **Skill caching**: Cache resolved URL/file skills with a TTL. Avoid re-fetching on every spawn. Store in `.wg/cache/skills/`.

9. **Context-aware trajectory scoring**: Replace binary context bonus (+5) with a graduated score based on:
   - Number of inputs satisfied (not just any/none)
   - Context size fit (penalty if total context would exceed limit)
   - Context reuse from previous trajectory step

#### Lower Priority

10. **Context summarization**: For tasks deep in a chain, generate a summary of the chain's progress so far. Could use a small model to summarize logs/artifacts from completed predecessor chain.

11. **Selective identity rendering**: Instead of including all skills, match skills to the specific task and only include relevant ones. The skill-matching logic already exists in `match_role_to_task()`.

12. **Artifact versioning**: Track which version of an artifact was used by a downstream task. Enables detection of stale context when dependencies re-run.

---

## Appendix: Key File References

| Component | File | Lines |
|---|---|---|
| Context building | `src/commands/spawn.rs` | 84-108 |
| Template variables | `src/service/executor.rs` | 17-89 |
| Template application | `src/service/executor.rs` | 165-193 |
| Default Claude prompt | `src/service/executor.rs` | 440-502 |
| Alt Claude prompt | `src/service/claude.rs` | 18-40 |
| Context command | `src/commands/context.rs` | 30-125 |
| Shell env vars | `src/service/shell.rs` | 16-30, 163-169 |
| Identity rendering | `src/agency.rs` | 245-283 |
| Skill resolution | `src/agency.rs` | 163-236 |
| URL skill fetch | `src/agency.rs` | 201-220 |
| Role matching | `src/agency.rs` | 699-843 |
| Trajectory planning | `src/commands/trajectory.rs` | 50-135 |
| Trajectory scoring | `src/commands/trajectory.rs` | 218-252 |
| Artifact management | `src/commands/artifact.rs` | 7-108 |
| Task struct | `src/graph.rs` | 40-109 |
| Actor context_limit | `src/graph.rs` | 170-173 |
| Next task scoring | `src/commands/next.rs` | 68-110 |
| Agency config | `src/config.rs` | 58-123 |
| Identity assignment | `src/commands/assign.rs` | 109-164 |
