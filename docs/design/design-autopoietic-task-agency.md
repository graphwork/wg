# Autopoietic Task Agency Design

**Date:** 2026-04-10
**Status:** Draft — for implementation task creation
**Task:** research-autopoietic-task
**Pattern:** research / design

---

## Context

wg is built on autopoetic principles — tasks should have agency, can spawn dependents at any graph point, and complete independently. The native graph iteration design (`.flip-design-native-graph`) introduced task retry mechanics. This document expands the autopoetic vision into a concrete design covering five areas.

This document assumes familiarity with:
- The `CycleConfig` / `loop_iteration` mechanism in `src/graph.rs`
- The `restart_on_failure` / `max_failure_restarts` failure restart design (`docs/design-cycle-failure-restart.md`)
- The FLIP evaluation design (`.wg/research/flip-evaluation-design.md`)
- The `EvaluatorInput` / `EvaluatorDecision` types in `src/agency/prompt.rs`

---

## 1. Task Independence Protocol

### Problem

Currently, when a worker agent creates a task via `wg add`, the new task gets an implicit `after` dependency on the creating task. This models a parent-child sequential relationship well, but prevents true task autonomy — a spawned task that should run independently (fire-and-forget, or parallel work that doesn't depend on the parent's outcome) cannot be expressed without manual `wg edit` after creation.

### Design

Add an `--independent` flag to `wg add` that suppresses the implicit `after` dependency on the creating task.

**Syntax:**
```bash
wg add "Background scan" --independent -d "Scan for unused tasks"
```

**Equivalently, via explicit `--no-after`:**
```bash
wg add "Background scan" --no-after -d "Scan for unused tasks"
```

**JSON representation (graph.jsonl):**
```jsonl
{"id":"bg-scan-1","title":"Background scan","status":"open","after":[],"independent":true}
```

### Semantics

| Aspect | Default (dependent) | `--independent` |
|--------|-------------------|-----------------|
| Implicit `after` on creator | Yes: `after: [creator_id]` | No: `after: []` |
| Creator can monitor via `wg msg` | Yes | Yes |
| Creator's `wg show` shows spawned task | Yes | Yes |
| Task available to run immediately | Yes (if other deps satisfied) | Yes (same) |
| Dependent on creator's status for unblocking | Yes | No |

The spawned task is still "owned" by the creator in the sense that the creator's agent context can see it, message it, and the coordinator will dispatch it. The only difference is the dependency edge is absent.

**Critical invariant:** `--independent` does NOT mean "orphaned." The task can still have explicit `--after` dependencies on other tasks. It only means "no implicit dependency on the task that created me."

### Interaction with Creator Monitoring

The creator retains the ability to monitor and collect results:

1. **Via messages:** `wg msg send bg-scan-1 "status?"` still works — tasks communicate through the graph's message system regardless of dependency edges.

2. **Via artifact collection:** The creator can use `wg artifact read` on the independent task's artifacts after it completes.

3. **No automatic result forwarding:** Unlike a dependent task, results from an independent task are NOT automatically passed to the creator. If the creator needs the output, it should either use `after: [independent-task-id]` on a downstream task, or use `wg msg` to request the information.

### Concrete Example: Before and After

**Before (current behavior):**
```bash
# Agent on task-A adds task-B
$ wg add "Process data" --after task-A
# task-B gets: "after": ["task-A"] — implicit dependency

# Graph: task-A → task-B
# task-B cannot run until task-A is Done
```

**After (with `--independent`):**
```bash
# Agent on task-A adds task-B as independent
$ wg add "Cleanup logs" --independent
# task-B gets: "after": [] — no implicit dependency

# Graph: task-A, task-B (no edge)
# task-B can run immediately if other deps are satisfied
```

**Graph state transitions:**
```
BEFORE:
  State 1: task-A (InProgress) → task-B (Open, blocked_by: [task-A])
  State 2: task-A (Done) → task-B (Open, ready)

AFTER:
  State 1: task-A (InProgress), task-B (Open, no deps on task-A)
  State 2: task-B (Open, ready immediately — not blocked by task-A)
```

### Edge Cases

1. **Self-reference:** A task cannot be `--independent` of itself. The parser rejects this.

2. **Circular independence:** Two tasks created as `--independent` of each other have no dependency edge between them — this is fine and they can run in parallel.

3. **Mixed deps:** `wg add "task-C" --independent --after task-X` creates a task with explicit dep on X but no implicit dep on the creator.

4. **Backwards compat:** Existing tasks without an `independent` field default to `false` (dependent behavior).

---

## 2. Evaluator Agency Model

### Problem

Currently the evaluator scores a task's output and returns a numeric score. The FLIP design adds a "gate" mechanism that can fail a task if the score is below threshold. But the evaluator is still fundamentally a passive scorer — it doesn't decide what to do about a poor score. The triage decision (retry? reassign? escalate?) is handled elsewhere.

This design elevates the evaluator to an active triage decision point: the evaluator can recommend a specific retry strategy, and the system enacts it.

### Design

The evaluator returns not just a score but a structured `EvaluatorDecision` that recommends an action. The coordinator (or a new "triage executor") enacts the recommended action.

**New type in `src/agency/types.rs`:**

```rust
/// What the evaluator recommends after scoring a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EvaluatorDecision {
    /// Task passes — record score and continue.
    Approve {
        notes: String,
    },
    /// Task should be retried with the same model/executor.
    Retry {
        notes: String,
        max_attempts: Option<u32>,
    },
    /// Task should be retried with a stronger model.
    RetryWithModel {
        notes: String,
        model: String,
        reason: String,
    },
    /// Task should be assigned to a different executor type.
    RetryWithExecutor {
        notes: String,
        executor: String,
        reason: String,
    },
    /// Task should be reassigned to a different agent (different role/composition).
    Reassign {
        notes: String,
        reason: String,
        suggested_role: Option<String>,
    },
    /// Task failed in a way that requires human review.
    Escalate {
        notes: String,
        reason: String,
    },
    /// Task is rejected — mark Failed, do not retry.
    Reject {
        notes: String,
        reason: String,
    },
}
```

**Evaluator prompt change (in `src/agency/prompt.rs`):**

The evaluator prompt is extended to produce both a score AND a decision:

```
## Your Evaluation

Score the task from 0.0 to 1.0 on each dimension, then provide an overall score
and your recommendation for what should happen next.

Respond with ONLY a JSON object:
{
  "score": <overall score 0.0-1.0>,
  "dimensions": { ... },
  "notes": "<explanation>",
  "decision": {
    "type": "retry_with_model" | "retry" | "reassign" | "escalate" | "reject" | "approve",
    "notes": "<your reasoning>",
    "reason": "<specific reason for the decision type>"  // required for non-approve types
  }
}
```

### Triage Protocol

When evaluation completes, the coordinator (or a new triage handler) interprets the decision:

```
Task completes (Done)
        │
        ▼
Evaluator scores → decision: { type, notes, reason }
        │
        ├── Approve → record score, task stays Done
        ├── Retry → schedule retry with same executor/model
        ├── RetryWithModel → schedule retry with specified model
        ├── RetryWithExecutor → schedule retry with specified executor type
        ├── Reassign → mark task Open, assign to different agent
        ├── Escalate → leave task Done, notify human, log escalation
        └── Reject → mark task Failed, propagate failure downstream
```

### Context Passing on Retry

When a task is retried based on evaluator decision, the retry task inherits context from the failed attempt:

**Metadata on the retry task:**
```rust
pub struct Task {
    // ... existing fields ...

    /// If this task is a retry/iteration, the ID of the task it retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retried_from: Option<String>,

    /// The evaluation that triggered this retry (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_trigger_eval: Option<String>,

    /// Number of retry attempts for this specific task (vs. cycle restarts).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retry_count: u32,
}
```

**Context passed to retry agent:**
The agent prompt for a retry task includes:
1. The original task description.
2. The failed/poor evaluation notes and score.
3. What specifically was identified as the problem.
4. Any partial work already done (artifacts from the previous attempt).

### Concrete Example: Before and After

**Before:**
```
Task: "Implement feature X"
  Attempt 1 → Evaluator: score 0.4, notes "Missing edge cases"
  → Coordinator must decide what to do (hard-coded logic)
  → Retry? Which model? Human decides?
```

**After:**
```
Task: "Implement feature X"
  Attempt 1 → Evaluator: score 0.4, decision: { type: "retry_with_model", model: "claude/opus-3", reason: "Current model lacks capacity for edge case handling" }
  → Coordinator enacts: creates retry task with model=opus-3, retried_from=original-task
  → Attempt 2 runs with stronger model, context includes what went wrong
```

### Interaction with FLIP

FLIP evaluation adds an orthogonal signal (`intent_fidelity` score). The evaluator decision can incorporate both:

```
Evaluator sees:
  Standard score: 0.6
  FLIP score: 0.3 (high scope drift — agent did wrong thing)

Decision: { type: "reassign", reason: "FLIP indicates scope drift — agent misunderstood task" }
```

### Risks

1. **Evaluator gaming:** An evaluator that always recommends "retry with stronger model" could inflate costs. Mitigate by tracking model升级 frequency and flagging anomalies.
2. **Decision quality:** The evaluator may not have enough context to decide retry strategy. The decision is a recommendation, not a binding command — the coordinator can override based on budget/external constraints.

---

## 3. Retry Propagation Strategy

### Problem

When a task retries (either via cycle restart, explicit retry, or evaluator decision), what happens to its dependents? Do they get paused/redone? Or do they proceed with potentially stale context?

### Design: Three Propagation Strategies

The retry propagation strategy determines how dependents react when an upstream task retries.

#### Strategy A: Conservative (default)

Dependents proceed normally. Only the retrying task re-executes. This is appropriate when:
- The retry is a localized fix (e.g., fixing a bug, not changing the spec).
- The dependent only needs the artifact output, not the internal process.

**Semantics:** No change to dependent tasks. The dependent reads the new artifact when it becomes available.

#### Strategy B: Aggressive (redo all)

When a task retries, ALL downstream dependents are re-opened and re-executed. This is appropriate when:
- The retry changes the task's output in a way that invalidates all downstream work.
- The task is a specification/design change that propagates.

**Semantics:** All transitively downstream tasks are marked `Open`, their `assigned`, `started_at`, `completed_at` are cleared, and they are re-dispatched.

#### Strategy C: Conditional

A per-task/edge policy that decides based on what changed:

```rust
pub enum PropagationTrigger {
    /// Always redo dependent when upstream retries
    Always,
    /// Never redo dependent (conservative)
    Never,
    /// Redo only if evaluation score changed by more than threshold
    ScoreDelta { threshold: f64 },
    /// Redo only if artifact content hash changed
    ArtifactChanged,
}
```

**Default for cycle iterations:** Conservative — dependents proceed with the new artifact.

**Default for evaluator-triggered retries:** Conditional on `ScoreDelta { threshold: 0.2 }`.

### Metadata: Iteration Context

Tasks track their position in an iteration chain:

```rust
pub struct Task {
    // ... existing fields ...

    /// If this task is part of an iteration chain, the ID of the original task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_anchor: Option<String>,

    /// The round number: 0 = original, 1 = first retry, 2 = second retry, etc.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub iteration_round: u32,

    /// The parent task ID for this specific attempt (previous round).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_parent: Option<String>,
}
```

**Stigmergic vs. Implicit context:**

| Approach | Pros | Cons |
|----------|------|------|
| **Stigmergic** (written to graph) | Visible to all agents, persists across coordinator restarts, queryable | Larger task state, more storage |
| **Implicit** (in agent prompt only) | Smaller task state, simpler | Not visible to coordinator/other agents, lost on restart |

**Decision: Stigmergic.** Write iteration metadata to the task itself, and include it in the agent prompt. This makes iteration state observable by the coordinator and other agents, not just the currently-running agent.

### Concrete Example: Before and After

**Before (no propagation strategy):**
```
Graph: A → B → C
A fails and retries.
  → What happens to B and C?
  → Current behavior: B and C proceed if they can (A is Failed/terminal)
  → But if A is reopened via cycle restart, B and C are not reopened
```

**After (with conditional propagation):**
```
Graph: A → B → C (both B and C have propagation_policy: ScoreDelta{0.2})

Round 0: A (score: 0.3, evaluator said: "missing error handling")
  → EvaluatorDecision: RetryWithModel
  → A re-opens as iteration_round=1, iteration_parent=A_round0
  → B: score_delta = N/A (B hasn't run yet), stays Open
  → C: score_delta = N/A, stays Open

Round 1: A completes (score: 0.8)
  → B runs, completes (score: 0.7)
  → C runs, completes (score: 0.6)
  → B's evaluation is 0.7, changed from 0.3 threshold was 0.2, so C re-opens
  → C re-executes because B's output changed meaningfully
```

**Graph state transition:**
```
BEFORE (no propagation):
  State 1: A (Failed), B (Done), C (Done) — C may have used stale output from A

AFTER (conservative with evaluator retry):
  State 1: A_round0 (Failed, retried_from=A_round0), B (Done), C (Done)
  State 2: A_round1 (InProgress), B (Done, retried_from=null), C (Done, retried_from=null)
  State 3: A_round1 (Done), B (Done), C (Done) — C proceeds with A_round1's artifact
```

### Implementation Notes

The propagation logic lives in `src/graph.rs` alongside existing cycle restart logic:

```rust
/// Propagate a retry/restart to downstream dependents based on their policy.
pub fn propagate_retry_to_dependents(
    graph: &mut wg,
    source_task_id: &str,
    source_round: u32,
    propagation_strategy: PropagationStrategy,
) -> Vec<String> {
    // For each direct dependent:
    //   1. Check its propagation policy
    //   2. If policy matches (Always, Never, ScoreDelta, ArtifactChanged), decide
    //   3. If redo, mark Open, clear execution metadata, record propagation
    //   4. Recurse transitively
}
```

The propagation strategy is set per-task via a new field:

```rust
pub struct Task {
    // ... existing fields ...

    /// How this task's dependents should react when this task retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub propagation_policy: Option<PropagationPolicy>,
}
```

CLI syntax:
```bash
wg add "B" --after A --propagation score_delta:0.2
wg edit C --propagation always
```

---

## 4. Iteration Metadata Schema

### Full Schema

Consolidating the iteration tracking fields across all mechanisms:

```rust
/// Configuration for a retry/iteration chain.
/// Attached to the original (anchor) task; all retries reference it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationConfig {
    /// The maximum number of retry attempts (not counting cycle iterations).
    /// None = unlimited (bounded by budget).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// The propagation strategy for this iteration chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub propagation: Option<PropagationPolicy>,

    /// The retry strategy recommended by the evaluator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_strategy: Option<RetryStrategy>,
}

/// Propagation policy for dependents on retry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PropagationPolicy {
    /// Always redo dependents (aggressive).
    Always,
    /// Never redo dependents (conservative).
    Never,
    /// Redo if score changed by more than threshold.
    ScoreDelta { threshold: f64 },
    /// Redo if artifact hash changed.
    ArtifactChanged,
}

/// Retry strategy for the iteration chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RetryStrategy {
    /// Retry with the same model.
    SameModel,
    /// Retry with a stronger model (escalate).
    UpgradeModel { suggested_model: String },
    /// Retry with a different executor type.
    ChangeExecutor { executor_type: String },
    /// Reassign to a different agent.
    Reassign { reason: String },
}

/// A task node — extended with iteration fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub after: Vec<String>,

    // ... many existing fields ...

    // --- Iteration / Retry fields ---

    /// If true, this task was created without an implicit dependency on its creator.
    #[serde(default, skip_serializing_if = "is_false")]
    pub independent: bool,

    /// The ID of the original/anchor task for this iteration chain.
    /// None if this is the anchor (first attempt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_anchor: Option<String>,

    /// Round number: 0 = original/anchor, 1 = first retry, 2 = second retry, etc.
    /// Incremented each time a task is retried (via any mechanism).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub iteration_round: u32,

    /// The task ID this one retried from (previous round).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_parent: Option<String>,

    /// Configuration for the entire iteration chain.
    /// Only present on the anchor task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_config: Option<IterationConfig>,

    /// The evaluation ID that triggered this retry (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_trigger_eval: Option<String>,

    /// Number of retry attempts for this specific task.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retry_count: u32,

    /// How this task's dependents should react when this task retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub propagation_policy: Option<PropagationPolicy>,
}
```

### CLI Interface

```bash
# Create an independent task with iteration config
wg add "Implement feature" --independent --max-retries 3 --propagation score_delta:0.2

# View iteration state
wg show my-task --include-iteration

# Query iteration chain
wg log my-task --iteration-chain

# Retry with strategy
wg retry my-task --strategy upgrade_model --model claude/opus-3
```

### Stigmergic Query

Agents and coordinators can query iteration state:

```bash
# "Am I an iteration?"
wg show $WG_TASK_ID --format json | jq '.iteration_round'

# "What's different from the prior attempt?"
wg show $WG_TASK_ID --format json | jq '{anchor: .iteration_anchor, parent: .iteration_parent, round: .iteration_round, trigger: .retry_trigger_eval}'

# "What did the previous attempt produce?"
# Follow iteration_parent to get the prior task, then read its artifacts
```

---

## 5. Integration Points

### 5.1 Integration with FLIP

FLIP provides an orthogonal evaluation signal. The evaluator decision can incorporate both:

```
Standard eval score: 0.5 (good quality work)
FLIP score: 0.25 (wrong task — scope drift)

EvaluatorDecision: { type: "reassign", reason: "FLIP scope drift: agent addressed wrong problem" }
```

**Integration point:** The `EvaluatorInput` struct in `src/agency/prompt.rs` already receives FLIP metrics as `flip_score` and `flip_metrics`. The evaluator prompt should be updated to factor these into the decision.

**Key question:** Should FLIP retries be a separate mechanism from native iteration retries?

**Decision: No — unified.** Both standard eval and FLIP eval feed into the same `EvaluatorDecision`. A task that fails FLIP gets the same retry/reassign mechanism as one that fails standard eval. This keeps the retry infrastructure unified.

**Can FLIP and native iteration coexist?**

Yes. FLIP = test-driven iteration (did the agent do the RIGHT thing?), native iteration = task-driven iteration (did the agent do the thing RIGHT?). They are orthogonal:
- FLIP catches wrong direction (reassign to different agent or approach).
- Native iteration catches right direction but poor execution (retry with stronger model).

### 5.2 Integration with Coordinator Reassignment

When the evaluator recommends `Reassign`, the coordinator must:

1. Mark the task as `Open` (or keep it `Done`/`Failed` depending on gate config).
2. Clear `assigned` and `started_at`.
3. Re-dispatch via the normal assignment pipeline — possibly with a different role/composition hint from the evaluator.

```rust
// In coordinator.rs — handle EvaluatorDecision::Reassign
fn handle_reassign(task_id: &str, decision: &EvaluatorDecision, graph: &mut wg) {
    if let EvaluatorDecision::Reassign { reason, suggested_role, .. } = decision {
        // Clear execution state
        if let Some(task) = graph.get_task_mut(task_id) {
            task.assigned = None;
            task.started_at = None;
            task.status = Status::Open;
            task.log.push(LogEntry {
                message: format!("Evaluator reassign: {}. Suggested role: {:?}", reason, suggested_role),
                // ...
            });
        }
        // Re-dispatch (coordinator tick will pick it up)
    }
}
```

### 5.3 Integration with Task State Machine

The existing task state machine (`Status` enum) handles the primary lifecycle. Iteration adds a layer on top:

```
Open → InProgress → Done → (if evaluator says retry) → Open (iteration_round+1)
                          → (if evaluator says reject) → Failed
                          → (if cycle restart) → Open (same or new iteration_round)

InProgress → Failed → (if restart_on_failure) → Open (via cycle failure restart)
```

The iteration metadata is orthogonal to status — a task can be `Open` with `iteration_round=3`. The coordinator dispatches it with full context of its iteration history.

### 5.4 Integration with Cycle Failure Restart

The `restart_on_failure` mechanism (already implemented in `src/graph.rs`) is a specific case of iteration propagation:

- **Cycle failure restart:** When a cycle member fails and `restart_on_failure=true`, the entire cycle restarts. All members are re-opened.
- **Evaluator retry:** When a task fails eval and evaluator says "retry," only that task re-opens (propagation policy governs dependents).

These are different mechanisms with different triggers, but they share the `iteration_round` / `iteration_parent` metadata schema.

**Conflict resolution:** If a cycle member fails AND the evaluator recommends reassign, which wins?
- Recommendation: Cycle failure restart takes precedence for cycle members. The evaluator's decision applies to the next iteration of the task.
- The evaluator's `reason` and `suggested_role` are stored on the task and applied when the cycle restarts.

### 5.5 Data Model Summary

```
Task
├── independent: bool
├── iteration_anchor: Option<String>
├── iteration_round: u32
├── iteration_parent: Option<String>
├── iteration_config: Option<IterationConfig>
│   ├── max_retries: Option<u32>
│   ├── propagation: Option<PropagationPolicy>
│   └── retry_strategy: Option<RetryStrategy>
├── retry_trigger_eval: Option<String>
├── retry_count: u32
└── propagation_policy: Option<PropagationPolicy>

Evaluation
├── score: f64
├── dimensions: HashMap
├── notes: String
├── decision: Option<EvaluatorDecision>  ← NEW
└── flip_score: Option<f64>

EvaluatorDecision
├── Approve
├── Retry { notes, max_attempts }
├── RetryWithModel { notes, model, reason }
├── RetryWithExecutor { notes, executor, reason }
├── Reassign { notes, reason, suggested_role }
├── Escalate { notes, reason }
└── Reject { notes, reason }

PropagationPolicy (on Task or IterationConfig)
├── Always
├── Never
├── ScoreDelta { threshold: f64 }
└── ArtifactChanged
```

---

## 6. Implementation Phases

### Phase 1: Task Independence Protocol

**Files:**
- `src/parser.rs` — add `independent` field to task parsing/serialization
- `src/commands/add.rs` — add `--independent` / `--no-after` flag
- `src/commands/edit.rs` — add `--independent` flag for editing
- `src/graph.rs` — add `independent: bool` to `Task` struct
- `src/commands/show.rs` — display `independent` flag

**Validation:**
- `wg add "test" --independent` creates task with `independent: true`, no `after` edge to creator
- Backwards compat: existing tasks without field parse correctly

### Phase 2: Evaluator Decision Types

**Files:**
- `src/agency/types.rs` — add `EvaluatorDecision` enum
- `src/agency/prompt.rs` — update `render_evaluator_prompt` to request decision
- `src/commands/evaluate.rs` — parse decision from evaluator output, act on it
- `src/commands/coordinate.rs` — handle decision types in coordinator tick

**Validation:**
- Evaluator returns decision JSON, coordinator enacts it
- `EvaluatorDecision::RetryWithModel` creates retry task with correct model

### Phase 3: Iteration Metadata Schema

**Files:**
- `src/graph.rs` — add all iteration fields to `Task` struct
- `src/agency/types.rs` — add `IterationConfig`, `PropagationPolicy`, `RetryStrategy`
- `src/parser.rs` — deserialize/serialize new fields
- `src/commands/add.rs` — add `--max-retries`, `--propagation` flags
- `src/commands/edit.rs` — add iteration field editors
- `src/commands/show.rs` — display iteration state

**Validation:**
- Task created with `--max-retries 3 --propagation score_delta:0.2` has correct config
- `iteration_round`, `iteration_anchor`, `iteration_parent` are set correctly on retry

### Phase 4: Retry Propagation

**Files:**
- `src/graph.rs` — implement `propagate_retry_to_dependents()`
- `src/commands/coordinate.rs` — call propagation after retry decision enacted
- `src/commands/retry.rs` — handle retry CLI (new command)

**Validation:**
- Task A with `--propagation always` → B → C
- A retries → B and C re-open automatically
- Task D with `--propagation never` → E
- D retries → E does NOT re-open

### Phase 5: Integration with FLIP and Coordinator

**Files:**
- `src/commands/evaluate.rs` — factor FLIP score into evaluator decision
- `src/commands/coordinate.rs` — handle `Reassign`, `Escalate`, `Reject` decisions
- `src/agency/eval.rs` — `record_evaluation` stores decision
- Tests in `tests/`

**Validation:**
- FLIP score 0.2 + standard score 0.7 → evaluator recommends `Reassign`
- Coordinator enacts reassign: task re-dispatched with new agent

---

## 7. Open Questions

1. **Retry budget vs. cycle budget:** `max_retries` on `IterationConfig` bounds evaluator-triggered retries. `max_iterations` on `CycleConfig` bounds cycle iterations. These are separate budgets. Is this the right model, or should they be unified?

2. **Evaluator override:** The coordinator can override evaluator decisions based on external constraints (budget, deadlines). Should this be explicit in the coordinator config, or implicit in how it interprets `EvaluatorDecision`?

3. **Human escalation:** `EvaluatorDecision::Escalate` leaves the task Done but notifies a human. How is this notification delivered? (Email? Slack? A separate "escalations" task?) The notification mechanism is out of scope for this design but should be noted.

4. **Iteration context in prompts:** How much of the prior attempt's context should be injected into the retry prompt? Full history (all log entries)? Just the failed evaluation? The artifact diff? This affects token cost and prompt complexity.

---

## 8. Decision Summary

| Question | Decision | Rationale |
|----------|----------|-----------|
| Flag name for independent tasks | `--independent` | Clear semantics; alternative `--no-after` as synonym |
| Evaluator as triage point | Yes — `EvaluatorDecision` enum | Unifies retry/reassign/escalate logic |
| Propagation strategies | Three: Conservative (default), Aggressive, Conditional | Covers the main use cases; conditional covers FLIP-driven propagation |
| Iteration metadata | Stigmergic (in task) + implicit (in prompt) | Visible to coordinator, persists across restarts |
| FLIP + native iteration coexistence | Unified retry mechanism, orthogonal eval signals | FLIP catches wrong direction; native catches poor execution |
| Cycle restart vs. evaluator retry | Separate triggers, shared metadata schema | Different semantics but same iteration tracking |

---

## 9. Validation Checklist

- [ ] Design covers all 5 areas with concrete examples
- [ ] Document is ≥1500 words
- [ ] Examples show task graph state transitions
- [ ] Each mechanism has "before" and "after" states
- [ ] Implementation phases are concrete enough to create tasks from
- [ ] Open questions are identified and documented
