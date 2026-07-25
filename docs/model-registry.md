# Model registry & non-OpenRouter providers

How `wg` resolves model specs, how to declare a provider that is **not** in
the built-in Anthropic catalog, and why dispatch never depends on the
OpenRouter catalog refresh.

This is the user-facing reference for the fix to
`wg-bug-openrouter-model-resolution` (a non-built-in provider such as
`pi:zai:glm-5.2` used to wedge the dispatcher when the OpenRouter refresh
was unavailable).

## TL;DR — the three ways a model resolves (no OpenRouter required)

A model spec is **resolved with no network** when any of these is true:

1. **It is handler-first** — its leading token is a handler/executor that
   owns model resolution: `pi:`, `claude:`, `codex:`, `nex:`/`native:`,
   `opencode:`, … The executor resolves the rest itself. Example:
   `pi:zai:glm-5.2` → the `pi` handler reaches `zai` natively.
2. **It matches a `[[model_registry]]` entry** you declared (see below).
3. **It is a built-in alias** in the registry (`haiku`, `sonnet`, `opus`,
   `fable`, …).

Anything else (a bare short id with no handler prefix, no registry entry,
no `/`) is *unresolved* and `wg` will warn about it loudly.

The OpenRouter catalog refresh is **optional metadata** (pricing/rankings).
A failed or disabled refresh **never** blocks spawning.

## Declaring a provider statically: `[[model_registry]]`

Use `[[model_registry]]` to teach `wg` about a provider/model it has no
built-in knowledge of, with no network fetch:

```toml
# .wg/config.toml  (or global ~/.wg/config.toml)

[[model_registry]]
id = "glm-5.2"          # short id you reference elsewhere (tier names, etc.)
provider = "zai"        # the provider identity
model = "glm-5.2"       # the model id passed to the executor
tier = "standard"       # fast | standard | premium
context_window = 131072 # optional: max input tokens
max_output_tokens = 8192
cost_per_input_mtok = 0.0
cost_per_output_mtok = 0.0
prompt_caching = false
```

Once declared, a spec that resolves to this entry's identity is considered
resolved **without** contacting OpenRouter. Both spellings match:

- `pi:zai:glm-5.2`  (handler-first route — the common case)
- `pi:zai/glm-5.2`  (slash form)

For OpenRouter/OpenAI-style providers whose wire uses `vendor/model`, set
`model = "vendor/model"` (the slash form) so the registry-model-format
check stays happy.

### Schema fields

| Field                 | Required | Notes |
|-----------------------|----------|-------|
| `id`                  | yes      | Short identifier used in tier/role references |
| `provider`            | yes      | `"anthropic"`, `"zai"`, `"openrouter"`, `"openai"`, … |
| `model`               | yes      | Model id passed to the executor (`vendor/model` for OR/OpenAI wires) |
| `tier`                | yes      | `fast` / `standard` / `premium` |
| `endpoint`            | no       | URL; omit to use the provider default |
| `context_window`      | no       | Max input tokens |
| `max_output_tokens`   | no       | Max output tokens |
| `cost_per_input_mtok` | no       | USD per 1M input tokens |
| `cost_per_output_mtok`| no       | USD per 1M output tokens |
| `prompt_caching`      | no       | Whether the provider supports prompt caching |
| `cache_read_discount` | no       | Multiplier for cached reads (e.g. `0.1`) |
| `cache_write_premium` | no       | Multiplier for cache writes (e.g. `1.25`) |
| `descriptors`         | no       | Free-form tags ("reasoning", "fast", …) |

## Disabling the OpenRouter refresh entirely

If your deployment is provider-complete without OpenRouter (e.g. a pure-`pi`
setup where every model is `pi:<provider>:<model>` and the executor resolves
the provider itself), turn the refresh off:

```toml
[registry]
openrouter_refresh = false   # default: true (backward compatible)
```

Equivalent in effect to `coordinator.registry_refresh_interval = 0`, but this
is the documented, semantic switch for "OpenRouter is not part of this
deployment". Either way the daemon skips the refresh cleanly — **no error,
no 60-minute cooldown, no impact on dispatch**.

Even with the refresh enabled, if no OpenRouter API key/endpoint is
resolvable the daemon skips it silently instead of erroring into a cooldown.
The refresh is metadata; its absence cannot wedge the dispatcher.

## How dispatch stays decoupled

`plan_spawn` (`src/dispatch/plan.rs`) is the single source of truth for the
spawn decision. It **never** consults the model registry or the OpenRouter
refresh — it carries the model spec verbatim for the executor to resolve.
So:

- `pi:zai:glm-5.2` spawns under the `pi` handler with the inner `zai:glm-5.2`
  dialect, regardless of whether OpenRouter is reachable.
- A failing catalog refresh trips a breaker that only throttles *future
  refresh attempts*, never spawns.

## When `wg service start` speaks up

At `wg service start`, `wg` surfaces any configured model that is **not**
resolved by a handler/executor or a `[[model_registry]]` entry — *before*
the daemon forks — so a genuinely unresolvable model is reported loudly on
the terminal instead of silently wedging the dispatcher. A handler-first
route or a declared registry entry starts cleanly with no warning.
