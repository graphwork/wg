# Structured Reasoning in Named Profiles

Task: `explain-profile-reasoning`

This report describes the current implementation after `implement-structured-pi`.
It is grounded in the current code paths:

- `ReasoningLevel`, `TierConfig`, `RoleModelConfig`, and resolution are in `src/config.rs`.
- `wg config --reasoning` / `--set-reasoning` writes normal config in `src/commands/config_cmd.rs`.
- named profiles are regular `Config` TOML snapshots plus optional `description` in `src/profile/named.rs`.
- `wg profile use` overlays profile routing keys onto `~/.wg/config.toml` in `src/commands/profile_cmd.rs` and `src/profile/named.rs`.
- `wg profile pi` only edits model tier keys, not reasoning keys, in `src/cli.rs` and `src/commands/profile_cmd.rs`.
- Pi receives structured reasoning as `--thinking <level>` in `src/commands/pi_handler.rs` and `src/commands/spawn/execution.rs`.

## Exact TOML Keys

Valid reasoning values are:

```toml
"off"
"minimal"
"low"
"medium"
"high"
"xhigh"
"max"
```

In `~/.wg/profiles/<name>.toml`, the file format is the normal `Config` TOML
schema with an optional top-level `description`. The reasoning keys are:

```toml
[models.default]
reasoning = "medium"

[models.<role>]
reasoning = "high"

[tiers]
fast_reasoning = "low"
standard_reasoning = "high"
premium_reasoning = "xhigh"
```

`<role>` is a `DispatchRole` name, for example:

```toml
task_agent
evaluator
flip_inference
flip_comparison
assigner
evolver
verification
triage
creator
compactor
placer
chat_compactor
reviewer
```

The profile's default model route itself is not a reasoning key. It remains:

```toml
[agent]
model = "pi:openai-codex:gpt-5.6-sol"

[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"

[models.default]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "medium"
```

Minimal Pi Codex profile example:

```toml
description = "Pi Codex example: Pi handler with independent reasoning budgets"

[agent]
model = "pi:openai-codex:gpt-5.6-sol"

[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"

[tiers]
fast = "openrouter:deepseek/deepseek-chat"
fast_reasoning = "low"
standard = "pi:openai-codex:gpt-5.6-sol"
standard_reasoning = "high"
premium = "pi:openai-codex:gpt-5.6-sol"
premium_reasoning = "xhigh"

[models.default]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "medium"

[models.task_agent]
model = "pi:openai-codex:gpt-5.6-sol"

[models.evaluator]
model = "openrouter:deepseek/deepseek-chat"

[models.assigner]
model = "openrouter:deepseek/deepseek-chat"

[models.flip_inference]
model = "openrouter:deepseek/deepseek-chat"

[models.flip_comparison]
model = "openrouter:deepseek/deepseek-chat"
```

This is syntactically valid for the current schema. It deliberately omits
`models.task_agent.reasoning`, so `task_agent` inherits `standard_reasoning =
"high"` rather than the default reasoning of `"medium"`.

## What `wg config` Mutates

`wg config --reasoning <level>` mutates a normal config file, not the named
profile file. By default, `wg config` writes the project-local `.wg/config.toml`.
With `--global`, it writes `~/.wg/config.toml`.

The write performed by `--reasoning` is:

```toml
[models.default]
reasoning = "<level>"

[models.task_agent]
reasoning = "<level>"
```

`wg config --set-reasoning <role> <level>` writes:

```toml
[models.<role>]
reasoning = "<level>"
```

This also writes to the selected normal config scope, defaulting to local
project config. It does not edit `~/.wg/profiles/<name>.toml`.

One important side effect: if the write is a direct global routing edit, the
active profile pointer is cleared. The implementation treats `--set-reasoning`
as global routing when `--global` is used, so `wg config --global
--set-reasoning task_agent high` clears `~/.wg/active-profile` after writing
`~/.wg/config.toml`. `wg config --global --reasoning high` does write global
reasoning, but in the current `direct_global_routing_change` predicate it is not
included, so it does not clear the active profile pointer. That is the current
implementation behavior.

Project-local config remains an overlay. A local `.wg/config.toml` reasoning
key can override the global config produced by an active profile until
`wg profile use <name>` clears local routing override tables.

Per-task overrides are separate task fields:

```bash
wg add "Task" --model pi:openai-codex:gpt-5.6-sol --reasoning high
wg edit <task> --reasoning xhigh
wg spawn <task> --reasoning max
```

Spawn resolution uses explicit spawn reasoning first, then the task's persisted
`reasoning`, then the planned model route's resolved reasoning.

## `wg profile pi` and the CLI Gap

`wg profile pi` does not expose reasoning flags today. Its CLI accepts only:

```text
wg profile pi [STRONG] [WEAK]
wg profile pi --strong <model>
wg profile pi --weak <model>
wg profile pi --show
wg profile pi --list
wg profile pi --dry-run
wg profile pi --no-reload
```

It writes the Pi profile model key sets only:

```toml
# strong
[agent]
model = "..."

[dispatcher]
model = "..."

[models.default]
model = "..."

[models.task_agent]
model = "..."

[tiers]
standard = "..."
premium = "..."

# weak
[tiers]
fast = "..."

[models.evaluator]
model = "..."

[models.assigner]
model = "..."

[models.flip_inference]
model = "..."

[models.flip_comparison]
model = "..."
```

The supported workflow for durable profile reasoning today is to edit the
profile TOML directly, for example:

```bash
wg profile edit pi
# add [tiers].standard_reasoning, [tiers].premium_reasoning,
# [models.<role>].reasoning, etc.
wg profile use pi
```

If `pi` is active and a profile file is edited through a supported profile
command, the profile is re-applied to global config and the daemon is
hot-reloaded unless `--no-reload` is supplied. There is no
`wg profile set-reasoning` command and no `wg profile pi --strong-reasoning` /
`--weak-reasoning` surface today. That is the CLI gap.

`wg config --reasoning` is not a durable profile-edit workflow. It edits the
materialized config scope (`.wg/config.toml` or `~/.wg/config.toml`), so a later
`wg profile use pi` can overwrite the global routing tables from the profile
snapshot and clear local routing tables.

## Independent Precedence

Model and reasoning precedence are independent.

Model resolution has its own route cascade, including task/model overrides,
per-role `models.<role>.model`, `models.<role>.tier`, role default tier,
`models.default`, and `agent.model`.

Reasoning resolution is:

1. Explicit task/spawn override, handled by callers.
2. `[models.<role>].reasoning`.
3. `[models.<role>].tier` reasoning, if the role has a tier override.
4. The role's default tier reasoning from `[tiers]`.
5. `[models.default].reasoning`.
6. Omit the handler flag and let the handler default apply.

The current role default tiers are:

```text
standard: task_agent, default
premium: evolver, creator, verification
fast: evaluator, flip_inference, flip_comparison, assigner, triage,
      compactor, placer, chat_compactor, reviewer, coordinator_eval
```

Example policy:

```toml
[tiers]
fast = "openrouter:deepseek/deepseek-chat"
fast_reasoning = "low"
standard = "pi:openai-codex:gpt-5.6-sol"
standard_reasoning = "high"
premium = "pi:openai-codex:gpt-5.6-sol"
premium_reasoning = "xhigh"

[models.default]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "medium"

[models.task_agent]
model = "pi:openai-codex:gpt-5.6-sol"

[models.creator]
reasoning = "max"

[models.evaluator]
model = "openrouter:deepseek/deepseek-chat"
reasoning = "minimal"
```

Results:

- Chat/Pi handler with no role argument defaults to `task_agent` for reasoning,
  so it resolves `standard_reasoning = "high"` unless an explicit
  `--reasoning` is passed.
- Standard workers (`task_agent`) resolve `"high"` through their default
  standard tier even though `[models.default].reasoning = "medium"` exists,
  because tier reasoning outranks default reasoning.
- Premium work such as `verification` resolves `"xhigh"` from the premium tier.
- `creator` resolves `"max"` because role-specific reasoning outranks tier
  reasoning.
- Weak/fast agency work such as `evaluator` resolves `"minimal"` because the
  explicit role reasoning outranks `fast_reasoning = "low"`.
- Changing only `[models.task_agent].model` does not erase inherited reasoning;
  the tests assert that model and reasoning are independent.

For Pi, a non-`None` resolved reasoning value becomes `--thinking <level>`.
If reasoning is omitted, WG does not pass `--thinking`, preserving Pi's own
default. The smoke scenario `pi_structured_reasoning_codex` validates that
`pi:openai-codex:gpt-5.6-sol` plus `high` becomes separate Pi args:
`--provider openai-codex --model gpt-5.6-sol --thinking high`, and that reasoning
does not leak into the model string.

## `wg profile use pi` and Hot Reload

`wg profile use pi`:

1. Loads `~/.wg/profiles/pi.toml`, materializing the built-in starter if missing.
2. Overlays profile routing keys onto `~/.wg/config.toml`: `description`,
   `profile`, `[agent]`, `[dispatcher]`, `[tiers]`, `[models]`, and
   `[llm_endpoints]` only when the profile declares endpoints.
3. Preserves unrelated global state such as existing endpoint credentials when
   the profile omits `[llm_endpoints]`.
4. Clears project-local routing override tables from `.wg/config.toml`, including
   `[tiers]` and `[models]`, so local routing does not shadow the active profile.
5. Writes `~/.wg/active-profile` with `pi`.
6. Ensures the Pi plugin as a best-effort activation side effect if the profile
   contains a `pi:` route.
7. Sends daemon `Reconfigure { profile: Some("pi") }` unless `--no-reload` is
   used.

Reasoning values inside `[tiers]` and `[models]` are part of the overlaid
profile routing tables, so they are copied into the materialized global config
along with model routes. Hot reload does not mutate the profile file. It tells
the daemon to use the new active profile/config for the next spawned worker;
in-flight workers keep the model and reasoning they were spawned with.

