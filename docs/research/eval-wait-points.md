# Research: When Should Agents Wait on Evaluation Before Terminating?

**Date**: 2026-04-25
**Task**: research-when-should
**Status**: Research complete

---

## Executive Summary

Today's flow is: agent marks `wg done` → agent process exits → coordinator scaffolds `.evaluate-<task>` → evaluator runs post-hoc → verdict drives retry/rescue. The agent never sees its own evaluation.

The question: should some agents **block on their own evaluation** and self-correct before exiting, rather than relying on the current fresh-retry-on-failure pattern?

**Recommendation: No architectural change needed.** The existing post-hoc evaluation + rescue pipeline is the right default. Adding an in-process eval-wait mode would increase agent token spend, complicate lifecycle states, and provide marginal quality improvement for the common case. The one scenario where waiting adds value — iterative refinement of subjective/creative work — is better served by the existing **cycle** and **iterate** primitives.

However, a narrow extension is worth considering: a `self_check` hook that runs *before* `wg done` (agent-local, no coordinator involvement), distinct from the post-hoc evaluation pipeline. This is cheap, doesn't hold resources, and catches the "agent forgot to run tests" class of bugs.

---

## 1. Current Architecture (As-Is)

### Task lifecycle
```
.assign-<task> → <task> (agent runs) → wg done → .flip-<task> → .evaluate-<task>
```

Key properties:
- **Agent exits before evaluation begins.** The wrapper script (`src/commands/spawn/execution.rs:1253-1276`) checks task status after the agent process exits. If the agent called `wg done` and the process exited cleanly, the task is marked done (or `pending-validation` for `--verify`/`validation=llm` tasks).
- **Evaluation is asynchronous.** The coordinator's `build_auto_evaluate_tasks` (coordinator.rs:1559) creates `.evaluate-<task>` tasks only after the source task is done. These are dispatched as lightweight inline processes via `spawn_eval_inline` (coordinator.rs:2606).
- **Low-score evaluations trigger rescue.** The `check_eval_gate` function (evaluate.rs:1409) compares the evaluation score against `eval_gate_threshold` (default 0.7). Below threshold → `wg fail` + `wg rescue` creates a new first-class task with the evaluator's notes as the brief.
- **FLIP runs between done and evaluate.** The `.flip-<task>` stage (Fidelity via Latent Intent Probing) runs an independent task-reconstruction check, and low FLIP scores (below `flip_verification_threshold`) trigger `.verify-<task>` tasks.

### Existing "wait" patterns

The architecture already has several wait-before-done mechanisms:

1. **`--verify` (shell verification)**: Agent calls `wg done` → shell command runs → pass/fail gates completion. The agent IS still alive during this (verify runs in the `done` command), but it's a pass/fail gate, not a feedback loop.

2. **`validation = "llm"` (LLM gate)**: Task transitions to `PendingValidation` on `wg done`. Coordinator dispatches evaluator. Evaluator calls `wg approve` or `wg reject`. But the **original agent has already exited** — rejection spawns a new agent (via rescue or retry), not the same one.

3. **`validation = "external"`**: Same as above but human-adjudicated.

4. **`eval-gate` tag**: Evaluation score below `eval_gate_threshold` → task is failed + rescued. Again post-hoc, agent is gone.

5. **Cycles (`--max-iterations`)**: Task can be re-entered by the same or different agent after completion. But this is a separate iteration, not an in-process correction.

---

## 2. Task Category Survey

### Categories where **fresh-retry** (current pattern) is clearly better

| Category | Examples from recent history | Why fresh-retry wins |
|----------|----------------------------|---------------------|
| **Code implementation** | `fix-claude-resume`, `fix-codex-resume`, `embedded-pty-chat`, `fix-tui-tab` | Deterministic verification (cargo test). If eval says "tests pass but code quality is poor," a fresh agent with the evaluator's notes can refactor without the cognitive baggage of the first attempt. Context window is cleaner. |
| **Infrastructure/config tasks** | `stop-auto-creating`, `fix-create-registry-v2` | Binary outcome. Either it works or it doesn't. No iterative refinement to be gained from in-process feedback. |
| **Assignment/FLIP/evaluation tasks** | `.assign-*`, `.flip-*`, `.evaluate-*` | System tasks. No agent to hold open — they're inline processes. |
| **Bug investigation** | `investigate-tui-coordinator`, `investigate-tui-pty` | Research tasks produce documents. If the research is incomplete, a fresh agent with "your predecessor missed X" is more effective than asking the same agent (with a polluted context window) to add what it already missed. |

### Categories where **in-process feedback** might help

| Category | Examples | Why it might help | Why it probably doesn't |
|----------|---------|-------------------|----------------------|
| **Research/writing with subjective quality bar** | `research-when-should` (this task), `synth-wg-nex-plan-of-attack` | The "right answer" is judgment-dependent. An evaluator might say "missing the cost analysis section" and the same agent, still holding all context, could add it faster than a fresh one rebuilding context. | Fresh agent gets the evaluator's notes + the incomplete artifact. It can read the doc and add what's missing. Context rebuild cost is low for docs (just read the file). And the fresh agent avoids confirmation bias — it's not defending its own work. |
| **Multi-step refactoring** | Large-scope code changes spanning multiple files | Mid-flight feedback could prevent wasted effort if the approach is fundamentally wrong. | The `--verify` mechanism already catches this. If tests fail, the agent knows immediately. Eval feedback on "approach quality" is too slow to be useful mid-implementation (eval takes 30-60s, agent has moved on). |
| **Creative/design work** | Design documents, architecture proposals | Iterative refinement based on structural feedback could improve quality. | This is exactly what **cycles** are for. `--max-iterations 3` with `wg done --converged` is the existing pattern for iterative refinement. |

### Verdict by category

| Category | Recommendation |
|----------|---------------|
| Code implementation | **Fresh retry** (current) |
| Infrastructure/config | **Fresh retry** (current) |
| System tasks | **N/A** (inline processes) |
| Bug investigation | **Fresh retry** (current) |
| Research/writing | **Fresh retry** (current), with cycles for quality iteration |
| Multi-step refactoring | **Fresh retry** (current), with `--verify` for mid-flight |
| Creative/design | **Cycles** (existing) |

No category clearly benefits from in-process eval wait.

---

## 3. Cost Analysis

### Holding an agent open through evaluation

**Costs:**
- **Token burn**: An idle Claude agent consumes ~$0.03-0.10/minute in cached context tokens just to stay alive. Evaluation takes 30-120 seconds. That's $0.015-0.20 per eval wait, per task. At scale (50 tasks/day), this is $0.75-10/day in pure wait overhead.
- **Slot consumption**: The coordinator's `max_agents` limit means a waiting agent blocks a slot that could run a ready task. With max_agents=4 and eval wait=60s, you lose ~25% of one agent-slot's throughput.
- **Context window pollution**: By the time the agent gets eval feedback, its context is full of implementation details. Adding evaluator feedback on top risks the agent ignoring it (recency bias) or misinterpreting it (context overload).
- **Complexity**: New lifecycle states needed (`waiting-for-eval`?), changes to the wrapper script, coordinator needs to route eval results back to a still-running process (currently no IPC channel for this), agent needs to know how to interpret and act on eval feedback (prompt changes).

**What you'd gain:**
- Avoid the context rebuild cost of spawning a fresh agent. For code tasks, this is 30-60 seconds of reading files + understanding the codebase. For research tasks, 10-20 seconds of reading the existing document.
- The agent already has the "why" of its decisions, which might help it self-correct more precisely than a fresh agent interpreting evaluator notes.

**Net assessment:** The context rebuild cost (10-60s) is comparable to the evaluation wait time (30-120s). You're not saving wall-clock time. You're spending more money (idle agent tokens) to get a marginally better correction (same agent vs fresh agent with notes). The fresh-retry pattern wins on cost-efficiency.

### Spawning a fresh retry agent (current pattern)

**Costs:**
- Context rebuild: 10-60 seconds (reading task description, prior attempt artifacts, evaluator notes).
- New agent spawn: ~5 seconds (fork + prompt assembly).
- Lost "tacit knowledge": The fresh agent doesn't know why the predecessor made certain decisions. But the evaluator's notes + prior attempt artifacts mitigate this.

**What you gain:**
- Clean context window — no implementation residue.
- Independent perspective — fresh agent doesn't defend prior choices.
- No idle token burn during evaluation.
- Simpler architecture — no new states, no IPC, no prompt changes.
- Existing infrastructure already handles it (`wg rescue`, `auto_rescue_on_eval_fail`).

---

## 4. Architectural Assessment

### What would need to change for in-process eval wait

1. **New task status or agent state**: Something like `waiting-for-eval` to distinguish "agent is alive, waiting for eval" from "agent exited, task is done." Currently, the wrapper script (execution.rs:1253) checks status after the process exits. An in-process wait would require the agent to poll or the coordinator to signal.

2. **IPC channel from coordinator to agent**: Currently, the only communication channel is `wg msg send/read`. The agent would need to poll for eval results. This is doable but adds complexity — and the agent's polling loop would consume tokens.

3. **Prompt changes**: Agents would need instructions for how to interpret and act on eval feedback. "The evaluator said your code has a security flaw in the auth handler. Fix it." This is a non-trivial prompt engineering challenge — the agent needs to know when to accept vs push back on eval feedback.

4. **Wrapper script changes**: The exit-status-based lifecycle (execution.rs:1256-1276) would need to handle the "agent is still running but wants eval" case. This means the `wg done` command can't be the trigger — you'd need a new command like `wg request-eval` or `wg done --wait-for-eval`.

5. **Coordinator changes**: `build_auto_evaluate_tasks` (coordinator.rs:1559) currently creates eval tasks only when the source task is Done. It would need to also create them for tasks in the new waiting state, and route results back.

### What the architecture already supports

1. **`wg msg send/read`**: Agents can already receive messages during execution. If the coordinator sent eval results as messages, agents could poll. But this conflates eval feedback with human/coordinator messages and adds non-trivial complexity to agent prompts.

2. **Cycles**: `--max-iterations N` already supports "do work → evaluate → do more work" patterns. The evaluation happens between iterations, not during. This is the designed solution for iterative refinement.

3. **`wg iterate`**: The iterate command (docs/design/iterate-vs-retry-design.md) supports spiral re-execution with accumulated context. Each iteration gets a structured handoff from the predecessor. This is the designed solution for "partial progress + feedback → better attempt."

4. **`eval-gate` + `auto_rescue_on_eval_fail`**: Already implements "evaluation drives remediation." The evaluator's notes become the rescue task's brief. This IS the feedback loop — just with a fresh agent.

5. **`validation = "llm"`**: The LLM gate (docs/design/llm-verification-gate.md) already provides a quality gate at `wg done` time. Tasks transition to `PendingValidation`, evaluator runs, `wg approve` or `wg reject`. Rejection can trigger rescue.

---

## 5. The `self_check` Alternative

Instead of waiting for post-hoc evaluation, the useful signal is actually **pre-done self-verification**. This already partially exists:

- **`--verify` (shell verification)**: Runs a command before marking done. If it fails, the task stays in-progress and the agent can fix it.
- **Agent prompt instructions**: The agent guide already tells agents to validate before calling `wg done` (run `cargo build`, `cargo test`, re-read task description).

What's missing is a middle ground: a lightweight, agent-local check that runs as part of `wg done` but doesn't require a separate evaluator process:

```
wg add "Implement X" --self-check "cargo test && cargo clippy"
```

This would:
1. Run the command inline at `wg done` time (like `--verify` today).
2. On failure, print the error and return non-zero (agent stays alive, can fix).
3. On success, proceed to mark done normally.

This captures 80% of the value of "agent sees eval feedback" without any of the architectural complexity. The agent gets immediate, deterministic feedback before exiting. The post-hoc LLM evaluation handles the remaining 20% (subjective quality, completeness, approach correctness).

**This is essentially what `--verify` already does.** The question of "should agents wait on evaluation" reduces to "should we keep `--verify` working well" — and the answer is yes, the deprecation plan (docs/design/verify-deprecation-plan.md) is replacing `--verify` with `validation = "llm"`, not removing pre-done checks.

---

## 6. Recommendation

### Primary recommendation: **Do nothing.** The current architecture is correct.

The post-hoc evaluation + rescue pipeline is the right pattern because:

1. **Clean separation of concerns**: Agent implements, evaluator evaluates, rescue agent remediates. No role confusion.
2. **Independent evaluation**: A fresh evaluator avoids the bias of judging your own work.
3. **Cost-efficient**: No idle token burn during evaluation latency.
4. **Simpler architecture**: No new lifecycle states, no IPC, no complex prompt changes.
5. **Already handles the feedback loop**: `eval-gate` + `auto_rescue_on_eval_fail` + `wg rescue` = evaluation drives remediation via a fresh, clean-context agent.

### Secondary recommendation: Ensure the existing primitives cover the gap

For tasks that genuinely need iterative quality refinement:
- **Use cycles** (`--max-iterations N`) for planned iteration.
- **Use `wg iterate`** for unplanned re-execution with accumulated context.
- **Use `validation = "llm"`** for tasks that need a quality gate before downstream unblocking.
- **Use `eval-gate`** for tasks where low evaluation scores should trigger automatic rescue.

### What NOT to build

- A `wait_for_eval` flag on tasks or roles.
- A new `waiting-for-eval` lifecycle state.
- An IPC channel from coordinator to running agents for eval results.
- Any mechanism for agents to self-correct based on their own evaluation.

The existing primitives (cycles, iterate, eval-gate, rescue, validation modes) already cover every identified use case. Adding wait-for-eval would increase complexity without measurable quality improvement.

---

## 7. References

### Concrete tasks/agents from recent history

| Task | Eval Score | Outcome | Would wait-for-eval have helped? |
|------|-----------|---------|--------------------------------|
| `embedded-pty-chat` | FLIP 0.90 | Done, high quality | No — passed first time |
| `fix-tui-tab` | Evaluated | Done | No — deterministic fix, tests verify |
| `fix-claude-resume` | Eval-scheduled | Done | No — session targeting fix, tests verify |
| `narrow-pending-validation` | Evaluated | Done | No — schema change, tests verify |
| `investigate-tui-coordinator` | Evaluated | Done (research) | No — document produced, completeness is best judged post-hoc |
| `stop-auto-creating` | FLIP evaluated | Done | No — tag filtering, tests verify |

No recent task would have benefited from in-process eval wait. The current pipeline handled all cases correctly — high-quality tasks passed through, and the rescue mechanism exists for low-quality ones.

### Architecture files examined

- `src/commands/spawn/execution.rs:1250-1335` — wrapper script lifecycle
- `src/commands/service/coordinator.rs:1546-1620` — `build_auto_evaluate_tasks`
- `src/commands/service/coordinator.rs:2595-2825` — `spawn_eval_inline`
- `src/commands/evaluate.rs:1397-1528` — `check_eval_gate` + rescue flow
- `src/commands/eval_scaffold.rs:1-322` — lifecycle task scaffolding
- `src/commands/done.rs:898-1397` — PendingValidation transitions
- `src/commands/rescue.rs:1-80` — rescue task creation
- `docs/design/llm-verification-gate.md` — LLM gate design
- `docs/research/iterate-vs-retry-design.md` — iterate vs retry semantics
- `docs/research/validation-synthesis.md` — validation gap analysis
