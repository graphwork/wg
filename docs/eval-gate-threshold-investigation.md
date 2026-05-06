# Investigation: why `eval_gate_threshold = 0.7` did not block `codex-research-flags`

**Task:** `investigate-eval-gate` · **Date:** 2026-05-06 · **Verdict:** working‑as‑designed but the config name is misleading; the default surface acts like the gate is off.

## TL;DR

`eval_gate_threshold` only gates a task when **either** `agency.eval_gate_all = true` **or** the task carries the literal `eval-gate` tag. The reproducer task `codex-research-flags` had neither (its tags are `codex-gpt55,research, eval-scheduled`), so `check_eval_gate` returned `Ok(false)` before ever comparing the score to the threshold. The transition from `PendingEval → Done` is then driven by `resolve_pending_eval_tasks`, whose "evaluator passed" predicate is **not** a score comparison — it just checks whether the matching `.evaluate-X` task reached a terminal status. Score‑agnostic.

So the configured threshold *did* apply to the rules in the code; the rules just say "ignore the score for ungated tasks". The user‑facing surface (config key name, log message, doc comment on `Status::PendingEval`) implies a stronger guarantee than the code provides.

## Reproducer recap

`codex-research-flags`:
- 14:25:48 agent reported done → `PendingEval`
- 14:26:20 FLIP score 0.43
- 14:26:49 LLM evaluator score 0.27 (intent_fidelity 0.43)
- 14:26:53 `PendingEval → Done (evaluator passed; downstream unblocks)`
- Tags: `codex-gpt55,research, eval-scheduled` *(no `eval-gate` tag)*
- Merged config: `agency.eval_gate_threshold = 0.7`, `agency.eval_gate_all` not set (defaults to `false`).

## Every read site of `eval_gate_threshold` in the codebase

| # | Site | What the threshold does there |
|---|------|-------------------------------|
| 1 | `src/config.rs:2718` (decl), `:2567` (default factory), `:2862` (Default impl), doc at `:2710‑2713` | Type/default. The doc comment correctly states the threshold *only* applies when `eval_gate_all = true` or the task is tagged `eval-gate`. |
| 2 | `src/cli.rs:1738`, `src/main.rs:2514`, `:2828`, `:2870`, `src/commands/config_cmd.rs:160‑161`, `:687`, `:915‑923`, `:2890` | CLI plumbing for `wg config --eval-gate-threshold` (read/print/set). No behaviour. |
| 3 | `src/tui/viz_viewer/state.rs:16109`, `:16113`, `:16785‑16786`, `:20867`, `:20899`, `:21019` | TUI config editor surface. No behaviour. |
| 4 | **`src/commands/evaluate.rs:1548`** — inside `check_eval_gate` | **The only score‑vs‑threshold comparison.** Guarded by `is_gated = config.agency.eval_gate_all \|\| task_tags.iter().any(\|t\| t == "eval-gate")` (`:1554`). If not gated, returns `Ok(false)` (`:1556`) **without** comparing the score. When gated and `score < threshold`, the function fails the source task with `fail::run_eval_reject` and (optionally) auto‑rescues. |
| 5 | `src/commands/evaluate.rs:1760`, `:2242` | Doc comment + a unit‑test setter. No behaviour. |
| 6 | **`src/commands/service/coordinator.rs:949`** — `resolve_failed_pending_eval_tasks` | Reads the threshold for the *separate* `FailedPendingEval` lifecycle (agent exited non‑zero without `wg done`). Score ≥ threshold → rescue to `Done(rescued=true)`; below → `Failed`. **Not** the path taken by the reproducer. |
| 7 | **`src/commands/service/coordinator.rs:2072‑2089`** — FLIP scheduler | Reads the threshold only to *suppress* a redundant `.flip-X` task when the eval gate already failed the task. Same `is_gated` predicate (`:2074`). No effect on the unblock decision. |
| 8 | `src/commands/done.rs:1328` | Doc comment on `pick_done_target_status` claims "the dispatcher will flip it to `Done` once the eval scores ≥ `eval_gate_threshold`." This comment is wrong for ungated tasks — the dispatcher flips on *terminal eval status*, not on the score. |
| 9 | `src/graph.rs:174`, `:180` | Doc comments on `Status::PendingEval` / `Status::FailedPendingEval` that say "On pass (≥ `eval_gate_threshold`) the task transitions to `Done`". Same drift — true for `FailedPendingEval` (the function at site 6 honours the threshold), false for `PendingEval` of an ungated task. |

## What "evaluator passed" actually means in code

The log line `"PendingEval → Done (evaluator passed; downstream unblocks)"` is emitted at **`src/commands/service/coordinator.rs:918`**, inside `resolve_pending_eval_tasks` (`:880`).

The "passed" predicate is `src/commands/service/coordinator.rs:892`:

```rust
Some(s) if s.is_terminal() => Some(t.id.clone()),
```

i.e. the matching `.evaluate-<task>` reached any terminal status (`Done`, `Failed`, `Abandoned`). **No score is read here.** The function justifies this with the comment at `:887‑891` — "If it would have rejected, the source would already be Failed (handled by `check_eval_gate`)" — but that statement is only true for *gated* tasks, because `check_eval_gate` early‑returns at `:1556` for ungated ones. So for an ungated task with a score of 0.27, the chain is:

1. Evaluator finishes with score 0.27 → `.evaluate-X` becomes `Done` (terminal).
2. `record_evaluation_with_inference` calls `check_eval_gate` (`evaluate.rs:659`).
3. `check_eval_gate` sees `is_gated = false` → returns `Ok(false)` *without touching the source task*.
4. Next coordinator tick, `resolve_pending_eval_tasks` sees `.evaluate-X` is terminal and flips the source `PendingEval → Done` with the misleading log line.

## Diagnosis

**Verdict: working‑as‑designed but config‑misapplied / misleading.**

It is not a code bug — `check_eval_gate` and `resolve_pending_eval_tasks` both behave exactly as their source intends. The mismatch is between the *config surface* and the *operator's reasonable expectation*:

- The key is named `eval_gate_threshold`, suggesting "if I set a threshold, the gate is on."
- Default behaviour requires opt‑in per task (the `eval-gate` tag) or a project‑wide opt‑in (`eval_gate_all`). Neither is present here.
- The success log line says "evaluator passed" which strongly implies a score‑based pass/fail decision; in reality it means "the evaluator finished".
- Two doc comments in load‑bearing files (`src/graph.rs:174` and `src/commands/done.rs:1328`) document the gated semantics as if they were the universal semantics, reinforcing the misunderstanding.
- For the only auto‑evaluation surfaces that *do* honour the threshold unconditionally (`FailedPendingEval` and the FLIP suppression check), the scope is narrow and not the lifecycle most users see.

The result is a quiet floor: agents that score 0.27 still unblock downstream work, defeating the point of configuring a threshold in the first place.

## Recommendation

Smallest behaviour‑preserving change that aligns the surface with the code (no behaviour change in this task — diagnostic only):

1. **Tighten the success log line** at `src/commands/service/coordinator.rs:918` so it stops claiming "evaluator passed" for ungated tasks. Suggested wording: `"PendingEval → Done (eval finished; gate not configured for this task)"` when `!is_gated`, and `"PendingEval → Done (eval score ≥ threshold)"` when gated. This requires `resolve_pending_eval_tasks` to load the latest `.evaluate-X` evaluation and check `is_gated`, but it is purely a logging change.
2. **Fix the doc drift** at `src/graph.rs:174` and `src/commands/done.rs:1328` to reflect that the score gate only applies when the task is gated (tag or `eval_gate_all`).

Smallest user‑facing fix that makes the configured threshold actually gate the reproducer's transition (a **behaviour** change — propose as a follow‑up task, do not implement here):

3. **Flip the default of `eval_gate_all` to `true`** *or* **rename `eval_gate_threshold` to `eval_gate_threshold_when_tagged`** and introduce a new `eval_gate_default_threshold` that applies project‑wide. Either makes the config name match what an operator setting `0.7` expects. Option 3a (default to `true`) is the smallest diff.

Concrete recommendation in 1‑3 sentences: change the default of `agency.eval_gate_all` from `false` to `true` in `src/config.rs:2863` so a configured `eval_gate_threshold` actually gates every evaluated task by default, and update the success log line at `src/commands/service/coordinator.rs:918` plus the stale doc comments at `src/graph.rs:174` and `src/commands/done.rs:1328` so the post‑condition is described accurately. Operators who want today's opt‑in semantics can still set `eval_gate_all = false` explicitly.

## Suggested follow‑up tasks

- `fix-eval-gate-default-on` — flip `eval_gate_all` default to `true`, update tests, ship a `wg migrate config` note.
- `fix-eval-gate-log-and-docs` — correct the "evaluator passed" log line and the two doc comments listed above (no behaviour change; safe to land independently).

## Validation

- [x] Markdown report at `docs/eval-gate-threshold-investigation.md`
- [x] Lists every `eval_gate_threshold` read site with `file:line`
- [x] Explains what 'evaluator passed' actually means in code (predicate at `src/commands/service/coordinator.rs:892`)
- [x] Verdict: **wad‑misleading** (working‑as‑designed but the config name + log line + doc comments imply a stronger guarantee than the code provides)
- [x] Concrete recommendation in 1‑3 sentences (above)
