# Generated evaluator provider-prefix loss

## Reproduction

With `[models.evaluator].model = "codex:gpt-5.6-luna"`, create an ordinary
implementation task and later create a merge task. Before this fix, each
generated `.flip-*` and `.evaluate-*` row stored `model = "gpt-5.6-luna"` and
`provider = "codex"`. The inline dispatcher consumes `Task.model` as its
invocation-scoped route, so preflight rejected the bare value. Repeated daemon
ticks counted the deterministic representation error as spawn failures and
eventually opened the circuit breaker.

The exact regression flow is executable as:

```sh
cargo install --path . --locked
bash tests/smoke/scenarios/generated_evaluator_preserves_codex_route.sh
```

## Root cause and fix

`Config::resolve_model_for_role` deliberately exposes model and provider as
separate fields for HTTP callers. Spawn planning instead requires a canonical
handler-first route. `ResolvedModel::spawn_model_spec()` already implements
that boundary, but the four eager evaluator scaffold paths moved only
`ResolvedModel.model` into `Task.model`.

All evaluator and FLIP scaffold paths now store `spawn_model_spec()` and embed
that identical route in `--evaluator-model`. Thus task storage, inline spawn
preflight, registry metadata, retries, and the eventual `wg evaluate` process
all consume `codex:gpt-5.6-luna`; none must reconstruct it from lossy fields or
re-resolve mutable configuration. Bare aliases that resolve through a known
provider are canonicalized by the same helper before task creation, while an
unresolved bare invocation remains rejected by existing inline preflight.

## Verification

```sh
cargo fmt --check
cargo test test_generated_evaluators_preserve_explicit_codex_route
cargo test --test flip_role_model_routing
cargo test --test integration_provider_model_format
bash tests/smoke/scenarios/generated_evaluator_preserves_codex_route.sh
```

The unit regression creates both the initial implementation lifecycle and a
later merge lifecycle and checks stored model, legacy provider mirror, and
the pinned inline invocation for both FLIP and evaluator tasks. The smoke
scenario repeats that flow through the installed CLI and serialized graph.
