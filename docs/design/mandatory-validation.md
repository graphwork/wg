# Mandatory Validation Architecture

> **Historical design — superseded.** WG now defaults to acceptance criteria in
> `## Validation`, with worker-selected checks reported as evidence rather than
> silently promoted to completion authority. `--validation-command` remains an
> explicit operator/repository-authorized hard gate only. Agents must not invent
> or broaden one. See `docs/ops/completion-validation-evidence.md`.

## Problem

Agents frequently complete tasks without proper validation. Prompting alone is insufficient — the same agent that wrote code is biased toward thinking it works. We need structural enforcement: the system should make it impossible to consider a task "truly done" without independent verification.

## Current State

| Mechanism | Enforcement | Limitation |
|-----------|------------|------------|
| Prompt instructions ("run cargo test") | Soft | Agent can skip or fake it |
| `wg done` soft validation tip | Soft | Prints a tip, never blocks |
| `task.verify` field | Soft | Informational; `wg done` ignores it |
| `auto_evaluate` (evaluate-{task} tasks) | Structural | Runs AFTER completion, scores but doesn't gate |
| Wrapper script auto-`wg done` | Structural | Marks done on agent exit regardless of quality |

The auto-evaluate system is the closest existing mechanism. It creates `evaluate-{task-id}` tasks blocked by the original, dispatches an evaluator agent post-completion, and scores across correctness/completeness/efficiency/style. But it's **retrospective** — it never gates completion or forces rework.

## Design

### Core Idea: Validation as a Completion Gate

Instead of `done → evaluate (informational)`, we insert a validation phase between the agent declaring completion and the task being considered truly done:

```
Agent runs → agent calls wg done → status = PendingValidation
    → validator runs → APPROVE → status = Done
                     → REJECT  → status = Open (with feedback), agent re-dispatched
```

### New Task Status: `PendingValidation`

Add `PendingValidation` to the `Status` enum. Semantics:

- The working agent believes it's done
- Downstream tasks are NOT unblocked (unlike `Done`)
- A validation task becomes ready
- The task is not eligible for re-dispatch (unlike `Open`)

This is the key structural enforcement: downstream consumers only see `Done` after independent validation passes.

### Validation Modes

Configured per-task via `validation` field (or inherited from role/config defaults):

| Mode | Behavior | Use for |
|------|----------|---------|
| `none` | `wg done` → `Done` immediately (current behavior) | Research, design, exploration tasks |
| `integrated` | `wg done` succeeds only if validation log entries exist AND build/test pass | Trivial code tasks |
| `external` | `wg done` → `PendingValidation`, separate validator runs | Default for code tasks |

Default resolution order: `task.validation` > `role.default_validation` > config `default_validation` > `"none"` (backward compatible).

#### `integrated` mode details

When `validation = "integrated"`, `wg done` performs checks before allowing completion:

1. **Log check**: At least one log entry must contain "validat" (case-insensitive) — the existing soft check becomes a hard gate.
2. **Build check** (if task has `validation_commands`): Runs the commands (e.g., `cargo build`, `cargo test`) and fails `wg done` if any exit non-zero.

This is a lightweight gate that keeps the current single-agent model but adds teeth.

#### `external` mode details

When `validation = "external"`:

1. `wg done` transitions task to `PendingValidation` (not `Done`)
2. Coordinator detects `PendingValidation` tasks and creates `validate-{task-id}` task
3. Validator agent runs with the task's artifacts, diff, description, and logs
4. Validator calls `wg approve {task-id}` or `wg reject {task-id} --reason "..."`
5. Approve → `Done`; Reject → `Open` with feedback in log, agent unclaimed for re-dispatch

### Validation Tasks (`validate-{task-id}`)

Similar to existing `evaluate-{task-id}` but with different semantics:

| | `evaluate-{task}` | `validate-{task}` |
|---|---|---|
| **When created** | Any completed task (auto_evaluate) | Tasks with `validation = "external"` |
| **Input** | Artifacts, diff, logs, identity | Same + validation_commands |
| **Output** | Score (0-1) + dimensions | Binary: approve/reject + feedback |
| **Effect** | Updates performance records | Gates task completion |
| **exec_mode** | bare | light (needs to run tests) |
| **Can reject?** | No | Yes — reopens task |

### Validation Flow Diagram

```
                    ┌────────────┐
                    │  Agent     │
                    │  works on  │
                    │  task      │
                    └─────┬──────┘
                          │ wg done
                          ▼
                 ┌─────────────────┐
                 │ validation mode?│
                 └────┬────┬───┬──┘
            none │    │    │ external
                 │    │    │
                 ▼    │    ▼
              Done    │  PendingValidation
                      │    │
                      │    │ coordinator creates
                      │    │ validate-{task-id}
                      │    ▼
                      │  ┌──────────────┐
                      │  │  Validator    │
              integrated │  agent runs   │
                      │  └──┬───────┬───┘
                      │     │       │
                      ▼     ▼       ▼
                   checks  Approve  Reject
                   pass?     │       │
                    │        │       │
                   Y/N       ▼       ▼
                    │      Done    Open + feedback
                    ▼              (re-dispatch)
                  Done/Fail
```

### New CLI Commands

```bash
# Approve a validated task (validator calls this)
wg approve <task-id>

# Reject a validated task with feedback (validator calls this)
wg reject <task-id> --reason "Tests fail: 3 failures in auth module"

# Set validation mode on a task
wg edit <task-id> --validation external
wg edit <task-id> --validation none
wg edit <task-id> --validation integrated

# Set validation commands (run during integrated or external validation)
wg edit <task-id> --validation-commands "cargo build" --validation-commands "cargo test"

# Configure default validation mode
wg config --default-validation external
wg config --default-validation-commands "cargo build,cargo test"
```

### QA Agent Role

Define a QA role in the agency system for validators:

```yaml
# Role: Quality Assurance Validator
name: "QA Validator"
description: "Independently validates task completion by running tests, reviewing diffs, and checking against task requirements"
component_ids:
  - <code-review-component>
  - <test-execution-component>
  - <requirements-verification-component>
outcome_id: <validated-output-outcome>
default_exec_mode: "light"   # read-only + test execution
default_validation: "none"   # validators don't themselves get validated (avoid infinite regress)
```

```yaml
# Tradeoff: Skeptical
name: "Skeptical"
description: "Assumes code is broken until proven otherwise"
acceptable_tradeoffs:
  - "Slow — thoroughness over speed"
  - "Verbose feedback — explain every concern"
unacceptable_tradeoffs:
  - "Rubber-stamping — never approve without running tests"
  - "Scope creep — only validate what the task asked for"
```

The validator agent receives:

1. **Task description** — what was requested
2. **Artifact diff** — what changed (reuse existing `compute_artifact_diff`)
3. **Task logs** — what the agent reported doing
4. **Validation commands** — what to run (build/test)
5. **Upstream context** — for regressions

The validator prompt instructs it to:
1. Run all validation commands. If any fail → reject immediately.
2. Read the diff and compare against task requirements.
3. Check for obvious issues: unused imports, commented-out code, incomplete implementations.
4. Approve only if ALL checks pass.

### Evaluate vs. Validate

These are separate concerns:

| | Validation | Evaluation |
|---|---|---|
| **Question** | "Did this work?" | "How well did this work?" |
| **Output** | Binary (pass/fail) | Score (0-1) with dimensions |
| **Timing** | Gates completion | After completion |
| **Effect** | Blocks/unblocks downstream | Feeds evolution system |
| **Agent** | QA Validator (light exec) | Evaluator (bare exec) |

**Sequencing**: validate THEN evaluate. The evaluation task should depend on validation completing:

```
task → validate-task → evaluate-task
```

If validation rejects, the evaluate task never runs (the original task is reopened). Evaluation only scores work that passed validation — this improves evaluation signal quality since evaluators aren't wasting effort on broken work.

### Enforcing Test-Driven Approach

For code tasks, the validation system can enforce test-driven development through task description conventions and validator behavior:

1. **Task description template** (enforced by coordinator prompt):
   ```
   ## Acceptance criteria
   - [ ] Failing test demonstrating the bug/missing feature
   - [ ] Implementation that makes the test pass
   - [ ] No regressions (cargo test passes)
   ```

2. **Validator checks**:
   - Does the diff include test additions/modifications?
   - Do the tests reference the feature/fix described in the task?
   - Do ALL tests pass (not just new ones)?

3. **`test_required` flag** on tasks:
   ```bash
   wg add "Fix: auth token expiry" --test-required -d "..."
   ```
   When set, the validator rejects if no test files were modified in the diff.

4. **Role-level default**: The Programmer role can set `test_required: true` as a default, inherited by all tasks assigned to that role's agents.

### Configuration

```toml
[validation]
# Default validation mode for new tasks
default_mode = "none"           # "none", "integrated", "external"

# Default commands to run during validation
default_commands = ["cargo build", "cargo test"]

# Maximum validation attempts before escalating
max_rejections = 3              # after 3 rejections, task fails

# Model for validator agents
validator_model = "haiku"       # validators don't need expensive models

# Auto-create validation tasks (like auto_evaluate)
auto_validate = false           # opt-in; when true, code tasks get external validation

# Tags that trigger external validation by default
validation_tags = ["code", "implementation", "fix", "refactor"]
```

### Task Schema Changes

```yaml
# New fields on Task
validation: "external"          # none | integrated | external
validation_commands:            # commands to run during validation
  - "cargo build"
  - "cargo test"
test_required: false            # if true, validator rejects without test changes
max_rejections: 3               # override global max
```

### Graph Integration

The validation task is a structural dependency — it's wired into the graph just like evaluate tasks:

```
implement-feature ──→ validate-implement-feature ──→ evaluate-implement-feature
                                                  ──→ downstream-task-1
                                                  ──→ downstream-task-2
```

When the coordinator creates `validate-{task-id}`:
1. All edges FROM the original task are moved to the validate task
2. The validate task depends on the original task
3. The original task's status is `PendingValidation`
4. If approved: validate task marks itself done, original task → `Done`
5. If rejected: validate task marks itself done, original task → `Open`, unclaimed

This means downstream tasks naturally wait for validation without any special handling.

### Rejection Flow

When a validator rejects:

1. `wg reject {task-id} --reason "..."` is called
2. Original task status: `PendingValidation` → `Open`
3. `task.assigned` is cleared (unclaimed)
4. Rejection reason is added to task log
5. `task.rejection_count` is incremented
6. If `rejection_count >= max_rejections`: task → `Failed` with reason
7. Validate task → `Done` (it did its job)
8. Coordinator sees the task as ready again and re-dispatches

The re-dispatched agent sees the rejection feedback in the task log and can learn from it.

### Preventing Infinite Regress

Rules to prevent validation loops:
- Tasks tagged `validation` get `validation = "none"` (validators are not validated)
- Tasks tagged `evaluation` get `validation = "none"`
- Tasks tagged `assignment` get `validation = "none"`
- `max_rejections` provides a hard stop
- The validator's exec_mode is `light` — it can run tests but can't modify files

### Migration Path

**Phase 1: Foundation (non-breaking)**
- Add `PendingValidation` status
- Add `validation`, `validation_commands`, `test_required` fields to Task
- Add `wg approve` and `wg reject` commands
- Default `validation = "none"` everywhere — zero behavior change

**Phase 2: Coordinator support**
- Coordinator creates `validate-{task-id}` tasks for `PendingValidation` tasks
- Add `auto_validate` config flag
- Add `[validation]` config section
- Wire validation into task completion flow

**Phase 3: QA role + integrated mode**
- Create QA Validator role and Skeptical tradeoff via `wg agency init`
- Implement `integrated` mode checks in `wg done`
- Add `--validation` flag to `wg add`
- Add `test_required` enforcement

**Phase 4: Intelligence**
- Validator learns from rejection patterns (which agents, which kinds of mistakes)
- Validator gets evaluation scores that feed the evolution system
- Smart defaults: coordinator auto-sets `validation = "external"` for code tasks based on tags/skills

### Open Questions

1. **Should `PendingValidation` be visible to the working agent?** Currently, the wrapper script calls `wg done` and the agent exits. The agent never sees PendingValidation. This is probably fine — the re-dispatch on rejection creates a fresh agent with the feedback.

2. **Edge rewiring complexity**: Moving edges from the original task to the validate task is a graph mutation that could interact with cycle analysis. Need to handle carefully.

3. **Validate vs. evaluate ordering**: Should evaluation happen even on rejected work? Currently proposed: no. But evaluating rejected work could provide useful learning signal for evolution.

4. **Multi-project validation commands**: Different projects have different build/test commands. The `validation_commands` approach handles this, but we might want project-level defaults that apply automatically.

5. **Human-in-the-loop**: For high-stakes tasks, should there be a `validation = "human"` mode that notifies a human agent (via matrix/email executor) to approve?
