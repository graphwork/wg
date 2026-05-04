# Bug: `wg evolve run` bypasses `codex-cli` route and emits Claude model tasks

## Summary

In a repo configured for Codex, `wg evolve run` creates evolution fan-out tasks
with Claude executor/model settings (`claude` + `sonnet`/`haiku`/`opus`). This
contradicts the effective repo config and the user's expectation after setting
the project to the Codex route.

## Reproduction

In `/home/erik/poietic.life`, the repo is configured for Codex:

```sh
wg config --show --merged
```

Relevant effective config:

```text
[agent]
  model = "codex:gpt-5.5"

[dispatcher]
  executor = "codex"
  model = "codex:gpt-5.5"

[agency agents]
  task_agent = codex:gpt-5.5
  evaluator = codex:gpt-5.4-mini
  flip_inference = codex:gpt-5.4-mini
  flip_comparison = codex:gpt-5.4-mini
  assigner = codex:gpt-5.4-mini
  evolver = codex:gpt-5.5
  verification = codex:gpt-5.5
  creator = codex:gpt-5.5
```

This is the state expected after using the Codex route, e.g.:

```sh
wg init --route codex-cli
```

or equivalent local config using:

```sh
wg config -m codex:gpt-5.5
```

Then run:

```sh
wg evolve run
```

Observed output from the Poietic site graph:

```text
Evolution task graph created (run: run-20260504-172330):
  Analyzers: 8 tasks
    - mutation (7 evals, 1 roles, model: sonnet)
    - crossover (132 evals, 3 roles, model: sonnet)
    - gap-analysis (0 evals, 8 roles, model: opus)
    - motivation-tuning (139 evals, 8 roles, model: sonnet)
    - component-mutation (139 evals, 4 roles, model: sonnet)
    - randomisation (0 evals, 8 roles, model: haiku)
    - bizarre-ideation (0 evals, 4 roles, model: opus)
    - coordinator (33 evals, 8 roles, model: sonnet)
```

Inspecting a generated analyzer task confirms this is not only display text:

```sh
wg show .evolve-analyze-mutation-run-20260504-172330
```

Observed:

```text
Runtime:
  Executor: claude
  Model: sonnet

Log:
  Spawned by coordinator --executor claude --model sonnet [agent-143]
```

The synthesizer/apply/evaluate tasks also show `Model: sonnet (configured)`.

## Expected behavior

If the repo is configured with `wg init --route codex-cli` or equivalent Codex
model routing, evolution tasks should use the configured Codex model routing.

At minimum:

- Analyzer tasks should not emit bare Anthropic aliases (`sonnet`, `haiku`,
  `opus`) in a Codex-routed repo.
- Fan-out synthesis/apply/evaluate tasks should use the configured evolver model
  or the explicit `wg evolve run --model <model>` override.
- The executor recorded in task logs should match the resolved model provider
  (`codex` for `codex:gpt-5.5`), not `claude`.

## Actual behavior

Evolution fan-out tasks are created with Claude model aliases and run through the
Claude executor despite the effective config being Codex.

## Likely root cause

There are hardcoded model strings in the evolution fan-out implementation.

Observed in `src/commands/evolve/fanout.rs`:

```text
src/commands/evolve/fanout.rs:260:        model: Some("sonnet".to_string()),
src/commands/evolve/fanout.rs:304:        model: Some("sonnet".to_string()),
src/commands/evolve/fanout.rs:395:        model: Some("sonnet".to_string()),
```

The analyzer strategy setup also appears to choose `sonnet`/`haiku`/`opus`
directly instead of resolving through config tiers or role routing.

## Impact

This breaks project-local model routing and makes `wg init --route codex-cli`
misleading. A user can inspect config and see Codex everywhere, then `wg evolve
run` still spends work on Claude and records Claude-routed execution in the task
trace.

It also makes live systems harder to reason about: `wg status` reports the
dispatcher as `codex:gpt-5.5`, while evolution workers run as `claude:sonnet`.

## Suggested fix

1. In `wg evolve run`, resolve all evolution task models through the same model
   routing used elsewhere.
2. Treat `--model <MODEL>` as an override for every task in the evolution run,
   unless a strategy has an explicit and documented per-strategy override.
3. For strategy quality tiers, use configured tiers rather than bare aliases:
   fast -> configured fast tier, standard -> configured standard tier, premium
   -> configured premium tier.
4. Ensure task creation stores provider-qualified models such as
   `codex:gpt-5.5`, not bare `sonnet`.
5. Add a regression test for a Codex-routed temp graph:
   - initialize with `wg init --route codex-cli`;
   - run `wg evolve run --dry-run` or the task graph builder;
   - assert generated evolution tasks contain no bare `sonnet`, `haiku`, or
     `opus` models and no `claude` executor unless the route is Claude.

## Useful commands

```sh
wg config --show --merged
wg config --models
wg evolve run
wg show .evolve-analyze-mutation-run-<RUN_ID>
wg show .evolve-synthesize-run-<RUN_ID>
rg -n 'model: Some\\(\"(sonnet|haiku|opus)\"' src/commands/evolve
```
