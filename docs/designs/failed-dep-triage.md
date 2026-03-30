# Design: Failed-Dependency Triage Pattern

**Task:** design-failed-dep-triage
**Author:** Architect agent
**Date:** 2026-03-30

## Problem Statement

When a task becomes ready (all dependencies are terminal), the coordinator dispatches it regardless of whether those dependencies *succeeded* or *failed*. Today an agent spawned on a task with a failed dependency sees the failure in its dependency-context logs but has no structured protocol for handling it. The agent typically either:

1. Proceeds with its own work (ignoring the failure) — producing broken output
2. Calls `wg fail` immediately — propagating the failure but doing nothing useful
3. Tries to fix the issue itself — out-of-scope, unreliable, no coordination

We need a structured **triage protocol** where an agent detecting a failed dependency creates targeted fix tasks, requeues itself, and lets the system self-heal.

## Design Overview

The triage pattern introduces three components:

1. **Agent-side detection protocol** — how an agent recognizes and responds to failed deps
2. **`wg requeue` CLI command** — mechanism to reset a task to `open` with a log trail
3. **Coordinator-side triage counting** — loop-prevention via per-task triage budget

The key insight: **triage is NOT a cycle**. It's an ad-hoc self-healing mechanism. Cycles are structural (predefined iteration patterns). Triage is reactive (responds to unexpected failures). They use different infrastructure even though both involve task reactivation.

## 1. Agent Behavior Protocol

### Detection

When an agent spawns, it receives dependency context via `build_dependency_context()` (`src/commands/spawn/context.rs`). Currently, this only includes logs from `Done` dependencies:

```rust
if dep_task.status == Status::Done && !dep_task.log.is_empty() {
```

**Change:** Also include context for `Failed` dependencies — their `failure_reason`, last N log entries, and artifacts (if any). This gives the triage agent the information it needs.

### Decision Logic

The agent protocol (injected via executor prompt) is:

```
## Failed Dependency Protocol

Before starting your own work, check your dependency context.
If ANY dependency has status=Failed:

1. DO NOT proceed with your own task work.
2. Read the failure reason and logs from the failed dependency.
3. Assess whether you can create fix tasks:
   a. If the failure is clear and scoped → create fix task(s) via `wg add`
   b. If the failure is ambiguous or cascading → create a research/investigate task
   c. If you cannot determine a fix → `wg fail` with reason explaining the blocker
4. Create fix tasks that block yourself:
   `wg add "Fix: <description>" --before <your-task-id> -d "<details from failure logs>"`
5. Requeue yourself:
   `wg requeue <your-task-id> --reason "Created fix tasks for failed dep <dep-id>"`
6. Exit immediately (do not do any other work).
```

### What the Agent Sees

The executor prompt template gains a new section injected when failed deps exist:

```
## Failed Dependencies Detected — TRIAGE MODE

The following dependencies have FAILED:
- dep-task-id: "Task title" — Reason: <failure_reason>
  Last log: <truncated log entries>

You are in TRIAGE mode. Do NOT proceed with your normal task work.
Follow the Failed Dependency Protocol above.
```

### Multiple Failed Dependencies

When multiple deps have failed, the agent creates fix tasks for each (or a single umbrella fix task if the failures are related). The key invariant: every fix task uses `--before <your-task-id>` so the requeued task won't dispatch until all fixes complete.

### What the Agent Does NOT Do

- Does NOT retry the failed task itself (that's `wg retry`)
- Does NOT modify the failed task's status
- Does NOT attempt to complete its own task's work
- Does NOT enter triage mode if the dep is `Abandoned` (abandoned = intentional, not fixable)

## 2. CLI Surface: `wg requeue`

### Why Not `wg done --reset`?

`wg done` has strong semantics: "my work is complete." A triage agent that creates fix tasks and requeues hasn't done its work — it's deferring. Overloading `done` with `--reset` would confuse cycle evaluation (`evaluate_cycle_iteration` checks for Done status), contaminate token-usage tracking, and send misleading notifications.

### Why Not Reuse Existing Mechanisms?

- **`wg fail`** is wrong — the task hasn't failed, it's deferred.
- **Cycle restart** is wrong — there's no cycle; this is a one-time reactive requeue.
- **`wg retry`** is for retrying the *same* task after failure, not for deferring pending fix tasks.

### `wg requeue` Semantics

```
wg requeue <task-id> --reason "explanation"
```

**Effect:**
1. Status: `InProgress` → `Open`
2. Clears: `assigned`, `started_at`, `session_id`
3. Preserves: `loop_iteration`, `cycle_config`, `tags`, `retry_count`
4. Increments: `triage_count` (new field, u32, default 0)
5. Adds log entry: `"Requeued (triage {n}/{max}): {reason}"`
6. Does NOT trigger cycle evaluation (not a completion)
7. Does NOT count as a retry (different budget)

**Preconditions:**
- Task must be `InProgress` (only a running agent can requeue its own task)
- `triage_count < max_triage_attempts` (see loop prevention below)

**Error cases:**
- Task not in-progress: error with message
- Triage budget exhausted: error "Triage budget exhausted ({n}/{max}). Use `wg fail` instead."

### Implementation Location

New file: `src/commands/requeue.rs`, registered in `src/main.rs` alongside `done`, `fail`, `retry`.

The command is simple — it's essentially the "unclaim" portion of dead-agent cleanup but initiated by the agent itself, with triage tracking.

## 3. Loop Prevention

### Per-Task Triage Budget

New field on `Task`:

```rust
/// Number of times this task has been requeued via triage
#[serde(default, skip_serializing_if = "is_zero")]
pub triage_count: u32,
```

New config field in `[guardrails]`:

```toml
[guardrails]
max_triage_attempts = 3  # default
```

When `triage_count >= max_triage_attempts`, `wg requeue` returns an error and the agent must either `wg fail` or attempt the work anyway.

### Why Per-Task, Not Per-Dependency?

A task might have multiple failed deps across multiple triage cycles. Tracking per-task is simpler and provides a hard cap on total triage iterations regardless of which dep failed.

### Interaction with Existing Loop Prevention

| Mechanism | Scope | Purpose |
|---|---|---|
| `max_iterations` (CycleConfig) | Structural cycles | Caps planned iteration loops |
| `max_failure_restarts` (CycleConfig) | Structural cycles | Caps failure-triggered restarts within a cycle |
| `respawn_throttle` | Per-task | Detects rapid agent death/respawn |
| `max_triage_attempts` (new) | Per-task | Caps failed-dep triage requeues |

These are orthogonal. A task in a cycle that also encounters a failed dep uses both `triage_count` and `cycle_failure_restarts` independently.

### Cascade Breaker

If a triage fix task itself fails, and that failure cascades to another task entering triage, each task has its own `triage_count`. The cascade terminates when any task in the chain exhausts its triage budget and fails, which propagates up via normal dependency semantics.

Maximum cascade depth: `max_triage_attempts` per task × chain length. In practice, cascading failures resolve or exhaust budgets quickly because each fix task is targeted at a specific failure.

## 4. Coordinator Responsibilities

### What the Coordinator Does

The coordinator's role is **minimal** — triage is agent-driven:

1. **Context injection:** When building dependency context for a task with failed deps, include failure information (failure_reason, last log entries). This is a change to `build_dependency_context()` in `src/commands/spawn/context.rs`.

2. **Triage scope injection:** When any dependency is Failed, set a flag in `TemplateVars` that triggers the triage-mode prompt section. The agent doesn't discover the failure — it's told upfront.

3. **Normal dispatch:** The coordinator does NOT skip tasks with failed deps. It dispatches them normally, letting the agent protocol handle triage. This preserves the current behavior where `is_terminal()` includes `Failed`.

4. **Budget enforcement:** Enforced in `wg requeue`, not the coordinator. The coordinator never looks at `triage_count`.

### What the Coordinator Does NOT Do

- Does NOT decide whether to enter triage mode (agent decides)
- Does NOT create fix tasks (agent creates them)
- Does NOT prevent dispatch of tasks with failed deps (dispatch is correct; the agent triages)
- Does NOT inject a "triage" context scope (the executor prompt template handles this via a conditional section based on failed deps existing)

### Interface Contract Summary

| Responsibility | Owner |
|---|---|
| Detect failed deps in context | Coordinator (context injection) |
| Inject triage-mode prompt | Executor (prompt template) |
| Decide what fix tasks to create | Agent (LLM reasoning) |
| Create fix tasks | Agent (`wg add --before`) |
| Requeue self | Agent (`wg requeue`) |
| Enforce triage budget | `wg requeue` command |
| Track triage count | Task field (`triage_count`) |
| Dispatch requeued task | Coordinator (normal ready-task dispatch) |

## 5. Scenarios

### Scenario A: Single Failed Dependency

**Setup:** Task B depends on Task A. Task A fails with reason "cargo test failed: test_parse_config assertion error".

```
Timeline:
1. A: open → in-progress → failed (reason: "cargo test failed: ...")
2. B: open → ready (A is terminal) → dispatched
3. B agent spawns, sees:
     ## Failed Dependencies Detected — TRIAGE MODE
     - A: "Implement config parser" — Reason: cargo test failed: test_parse_config assertion error
       Last log: "Implemented parser but test_parse_config fails on nested keys"
4. B agent creates:
     wg add "Fix: config parser nested key handling" --before B \
       -d "## Description
     Task A (implement-config-parser) failed because test_parse_config assertion
     fails on nested keys. The parser likely doesn't recurse into nested TOML sections.

     ## Validation
     - [ ] cargo test test_parse_config passes
     - [ ] cargo test passes with no regressions"
5. B agent runs:
     wg requeue B --reason "Created fix task for failed dep A (config parser nested keys)"
6. B agent exits.
7. Fix task dispatches → agent fixes the issue → marks done
8. B dispatches again. This time A is still Failed, but the fix task is Done.
   B agent sees both contexts and proceeds with its work (fix task addressed the issue).
```

**Wait — B still sees A as Failed on re-dispatch.**

This is the critical design question. After the fix task completes, A is still `Failed`. Option analysis:

**Option 1: Fix task retries A.** The fix task's description includes "fix and retry A." After fixing, the fix agent runs `wg retry A`. Then A reruns and (hopefully) succeeds. B dispatches once A reaches Done.

- **Pro:** Clean — B only dispatches when A is truly Done.
- **Con:** The fix task must know about A and explicitly retry it. More complex fix-task authoring.

**Option 2: Fix task is self-contained; B checks fix-task status.** B's agent checks if the fix task (which is --before B) succeeded, and if so, proceeds with its work regardless of A's status.

- **Pro:** Fix task is simpler — just fix the code.
- **Con:** B proceeds without A being re-validated. Risky.

**Option 3 (Recommended): Triage agent retries the failed dep AND requeues itself.** The triage protocol becomes:

```
1. Create fix tasks (--before <failed-dep-id>)
2. Retry the failed dep: wg retry <failed-dep-id>
3. Requeue self: wg requeue <your-task-id>
```

Now the fix tasks block A's retry. A reruns after fixes, and if it succeeds, B dispatches cleanly with A=Done.

- **Pro:** B only dispatches when A is actually Done. Fix tasks target the root cause. A is re-validated.
- **Con:** Slightly more complex protocol, but clear and mechanical.

**We go with Option 3.** The triage protocol creates fix tasks `--before` the failed dep, retries the failed dep, then requeues itself.

### Revised Scenario A (with Option 3)

```
Timeline:
1. A: failed (reason: "test_parse_config fails on nested keys")
2. B: dispatched → triage mode
3. B agent:
     wg add "Fix: config parser nested key handling" --before A \
       --verify "cargo test test_parse_config passes" \
       -d "..."
     wg retry A
     wg requeue B --reason "Created fix for failed dep A, retried A"
4. B exits.
5. Fix task dispatches (A is blocked by fix task because of --before A)
6. Fix agent: fixes code, marks done
7. A: re-dispatches (fix task done, A was retried so it's open again)
8. A agent: runs, test_parse_config passes, marks done
9. B: all deps terminal and Done → dispatches → does its actual work
```

### Scenario B: Cascading Failure

**Setup:** C depends on B depends on A. A fails.

```
Timeline:
1. A: failed
2. B: dispatched → triage mode
3. B agent:
     wg add "Fix: ..." --before A
     wg retry A
     wg requeue B --reason "Triage: fix for A"
4. Fix task fails (wrong diagnosis, or deeper issue)
5. A: re-dispatches after fix (fix is done, even though it failed to actually fix)
   Wait — fix task failed, so it's terminal. A was retried → open.
   But A has --after fix-task? No — fix was --before A, meaning A has fix in its `after`.
   Fix is Failed (terminal), so A dispatches. A runs again, fails again.
6. B: dispatched again (A is terminal=Failed). Triage mode again.
   triage_count = 1.
7. B agent creates another fix task, retries A, requeues (triage_count = 2).
8. Second fix task runs, succeeds this time.
9. A re-dispatches, succeeds.
10. B dispatches, all deps Done, proceeds.
```

**Cascade with C:**
If B's own execution fails after triage succeeds, C enters triage for B. Each level has its own triage budget. The chain terminates when budgets are exhausted (tasks fail) or fixes succeed.

### Scenario C: Agent Fails During Triage

If the triage agent itself crashes (OOM, timeout, etc.) before running `wg requeue`:

1. Dead-agent cleanup detects the dead agent
2. Existing triage system (auto_triage in `triage.rs`) assesses the dead agent's output
3. Task is unclaimed and reset to Open
4. Re-dispatched → enters triage mode again (same triage_count, since `wg requeue` never ran)

This is the existing dead-agent recovery path. No special handling needed.

### Scenario D: Multiple Failed Dependencies

Task C depends on A (failed) and B (failed):

```
1. C dispatched → triage mode, sees both A and B failed
2. C agent:
     wg add "Fix: issue in A" --before A --verify "..."
     wg retry A
     wg add "Fix: issue in B" --before B --verify "..."
     wg retry B
     wg requeue C --reason "Triage: fix for A and B"
3. Both fix tasks dispatch in parallel
4. Both fixes succeed → A and B re-dispatch → both succeed
5. C dispatches with all deps Done
```

If only one fix succeeds, C re-enters triage for the remaining failure. `triage_count` increments each time.

## 6. Triage Context Scope

### No Dedicated Context Scope

Rather than a new "triage" context scope (which would require changes to `ContextScope` enum, scope resolution, and every scope comparison), we inject triage information as a **conditional section** in the executor prompt when failed deps are detected.

This is implemented in `build_dependency_context()` and `build_prompt()`:

```rust
// In build_dependency_context() — already iterates deps
if dep_task.status == Status::Failed {
    context_parts.push(format!(
        "From {} (FAILED): reason: {}\n  Last logs: ...",
        dep_id,
        dep_task.failure_reason.as_deref().unwrap_or("unknown")
    ));
}

// In build_prompt() or TemplateVars — set a flag
if has_failed_deps {
    parts.push(TRIAGE_MODE_SECTION.to_string());
}
```

The triage mode section is a constant string template that describes the protocol. It's injected alongside (not instead of) the normal workflow sections.

### Re-dispatch After Triage

When a requeued task dispatches again and all deps are now Done, the triage-mode section is NOT injected (no failed deps detected). The agent proceeds normally.

When a requeued task dispatches and some deps are still Failed, triage mode is injected again. The agent sees its `triage_count` in context and acts accordingly.

## 7. Implementation Plan

### Phase 1: Data Model (graph.rs)
- Add `triage_count: u32` field to `Task` (with `skip_serializing_if = "is_zero"`)
- Add `max_triage_attempts` to `guardrails` config (default 3)

### Phase 2: CLI Command (commands/requeue.rs)
- New `wg requeue <task-id> --reason "..."` command
- Validates: task is InProgress, triage budget not exhausted
- Resets: status→Open, clears assigned/started_at/session_id
- Increments: triage_count
- Logs the requeue event

### Phase 3: Context Injection (commands/spawn/context.rs)
- Extend `build_dependency_context()` to include Failed dep info
- Include `failure_reason` and last 3 log entries

### Phase 4: Prompt Template (service/executor.rs)
- Add `TRIAGE_MODE_SECTION` constant with the agent protocol
- Add `has_failed_deps: bool` to `TemplateVars`
- Conditionally inject triage section in `build_prompt()`

### Phase 5: Testing
- Unit test: `wg requeue` happy path, budget exhaustion
- Integration test: single failed dep scenario end-to-end
- Integration test: cascading failure scenario

## 8. Open Questions and Decisions

### Decision: `wg retry` semantics with `--before` fix tasks

`wg retry` currently resets a Failed task to Open. When fix tasks are added `--before` the failed task, the retried task gains new blockers. The retry resets the task to Open, but it won't dispatch until the fix tasks complete. This works correctly with existing infrastructure — no changes to `wg retry` needed.

**Confirmed:** `wg retry` + `wg add --before` compose correctly.

### Decision: Should requeue clear the `agent` field?

No. The agent assignment should persist across triage cycles. The same agent identity should handle the task when it re-dispatches. Only `assigned` (the agent instance ID) is cleared.

### Decision: Should triage_count reset on cycle iteration?

Yes. When a cycle reactivates a task (via `reactivate_cycle`), `triage_count` should reset to 0. Each cycle iteration gets a fresh triage budget. This mirrors how `failure_reason` is cleared on cycle restart.

### Decision: Notification on triage events?

Triage requeues should generate a notification event (same as task status changes). The existing notification dispatch infrastructure handles this — the `Open` status transition fires the event.

## 9. Relationship to Existing Triage System

The existing `auto_triage` system (`src/commands/service/triage.rs`) handles **dead agents** — processes that crashed or were killed. It uses an LLM to assess whether the dead agent's work was complete.

The new failed-dep triage is **complementary**, not overlapping:

| | Dead-Agent Triage (existing) | Failed-Dep Triage (new) |
|---|---|---|
| Trigger | Agent process dies | Agent detects failed dep |
| Actor | Coordinator (LLM call) | Agent itself |
| Decision | Was work complete? | What fix tasks to create? |
| Outcome | Done / restart / fail | Fix tasks + requeue |
| Config | `agency.auto_triage` | `guardrails.max_triage_attempts` |

They can compose: if an agent in triage mode crashes, dead-agent triage handles the crash, and the task re-dispatches into triage mode again.

## 10. Summary of Changes

| Component | Change | Scope |
|---|---|---|
| `graph.rs` | Add `triage_count` to Task | Small |
| `config.rs` | Add `max_triage_attempts` to guardrails | Small |
| `cli.rs` | Add `Requeue` subcommand | Small |
| `commands/requeue.rs` | New command implementation | Medium (~80 lines) |
| `main.rs` | Wire up Requeue command | Small |
| `commands/spawn/context.rs` | Include Failed dep info | Small |
| `service/executor.rs` | Add triage prompt section + flag | Medium |
| `graph.rs` (reactivate_cycle) | Reset `triage_count` on cycle iteration | Trivial |
| Tests | Unit + integration tests | Medium |
