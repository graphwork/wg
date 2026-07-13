# Pi Codex Account Routing Decision Memo

Date: 2026-07-10
Task: `define-pi-codex`

## Decision

WG can support Pi's ChatGPT/Codex subscription route with an explicit
handler-first model spec:

```bash
wg config -m 'pi:openai-codex:gpt-5.6-sol'
wg add 'Implement X' --model 'pi:openai-codex:gpt-5.6-sol'
```

The recommended syntax is:

```text
pi:<pi-provider-id>:<pi-model-id>
```

For Codex subscription auth, the verified Pi provider id is `openai-codex`.
With installed Pi `0.80.6`, `gpt-5.6-sol` is a listed current model id, not a
custom/unlisted forward reference. The current Pi registry lists
`gpt-5.6-luna`, `gpt-5.6-sol`, and `gpt-5.6-terra` with 372K context, 128K max
output, reasoning enabled, and image input enabled.

Reasoning should be a separate WG setting, not encoded in the model string. Pi
does support `--model <id>:<thinking>` shorthand, but WG model specs already use
colon separators for handler/provider/model routing. Encoding reasoning as a
fourth colon suffix would make `pi:openai-codex:gpt-5.6-sol:xhigh` ambiguous and
harder to validate across handlers.

## Verified Facts

Installed Pi:

```bash
$ which pi
/home/bot/.nvm/versions/node/v25.4.0/bin/pi
$ pi --version
0.80.6
$ npm view @earendil-works/pi-coding-agent version
0.80.6
```

Pi CLI help in this install says:

```text
--provider <name>
--model <pattern>    supports "provider/id" and optional ":<thinking>"
--thinking <level>   off, minimal, low, medium, high, xhigh, max
```

Authoritative current Pi docs/source:

- Pi provider docs say subscription auth is configured with `/login`, including
  "ChatGPT Plus/Pro (Codex)", and tokens are stored in
  `~/.pi/agent/auth.json`.
  Source: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/providers.md
- Pi custom-provider docs list `openai-codex-responses` as the OpenAI Codex
  Responses API type and describe `thinkingLevelMap`.
  Source: https://pi.dev/docs/latest/custom-provider
- The installed `@earendil-works/pi-ai` OAuth source defines the provider:
  `id: "openai-codex"`, name `ChatGPT Plus/Pro (Codex Subscription)`, with
  browser and device-code login methods.
  Local source:
  `/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai/dist/utils/oauth/openai-codex.js:499`
- The installed model catalog defines provider `openai-codex`, API
  `openai-codex-responses`, base URL `https://chatgpt.com/backend-api`, and
  listed model ids `gpt-5.3-codex-spark`, `gpt-5.4`, `gpt-5.4-mini`,
  `gpt-5.5`, `gpt-5.6-luna`, `gpt-5.6-sol`, and `gpt-5.6-terra`.
  Local source:
  `/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai/dist/providers/openai-codex.models.js:3`
- Installed source for `gpt-5.6-sol` has `reasoning: true`,
  `thinkingLevelMap: { "xhigh": "xhigh", "max": "max", "minimal": "low" }`,
  image input support, 372K context, and 128K max output.

Installed Pi model listing, verified after the update to `0.80.6`:

```bash
$ pi --list-models codex --offline
provider      model                      context  max-out  thinking  images
openai-codex  gpt-5.3-codex-spark        128K     128K     yes       no
openai-codex  gpt-5.4                    272K     128K     yes       yes
openai-codex  gpt-5.4-mini               272K     128K     yes       yes
openai-codex  gpt-5.5                    272K     128K     yes       yes
openai-codex  gpt-5.6-luna               372K     128K     yes       yes
openai-codex  gpt-5.6-sol                372K     128K     yes       yes
openai-codex  gpt-5.6-terra              372K     128K     yes       yes
openrouter    openai/gpt-5-codex         400K     128K     yes       yes
openrouter    openai/gpt-5.1-codex       400K     128K     yes       yes
openrouter    openai/gpt-5.1-codex-max   400K     128K     yes       yes
openrouter    openai/gpt-5.1-codex-mini  400K     100K     yes       yes
openrouter    openai/gpt-5.2-codex       400K     128K     yes       yes
openrouter    openai/gpt-5.3-codex       400K     128K     yes       yes
openrouter    qwen/qwen3-coder-next      262.1K   262.1K   no        no
```

Focused Pi probes:

```bash
$ pi --provider openai-codex --model gpt-5.6-sol --thinking xhigh \
    --no-tools --no-session --offline -p 'Reply with ok'
ok

$ pi --provider openai-codex --model gpt-5.6-sol --thinking max \
    --no-tools --no-session --offline -p 'Reply with ok'
ok
```

These commands prove the installed CLI accepts the recommended provider/model
syntax and the `xhigh` and `max` reasoning levels for `gpt-5.6-sol` in this
authenticated environment. Earlier observations from Pi `0.80.3` are superseded
by the `0.80.6` registry above.

## Current WG Behavior

WG already treats the first token of a model spec as the handler. The central
handler resolver intercepts external CLI prefixes and routes `pi:*` to
`ExecutorKind::Pi`.

Relevant code:

- `src/dispatch/handler_for_model.rs:87`: `handler_for_model`.
- `src/commands/pi_handler.rs:220`: `pi_model_arg`.
- `src/commands/pi_handler.rs:623`: `rpc_spawn_args`.
- `src/commands/spawn/execution.rs:1080`: external CLI `pi` model splitting.
- `src/service/llm.rs:880`: Pi one-shot agency model splitting.

For `pi:openai-codex:gpt-5.6-sol`, `pi_model_arg`:

1. Strips the outer `pi:` executor prefix.
2. Splits the remaining `openai-codex:gpt-5.6-sol` at the first colon.
3. Leaves unknown Pi provider names alone because `openai-codex` is not a WG
   native provider alias.
4. Produces Pi argv:

```text
--provider openai-codex --model gpt-5.6-sol
```

The handler then spawns Pi RPC mode with those args:

```text
pi --mode rpc --provider openai-codex --model gpt-5.6-sol ...
```

Focused WG tests already cover the generic shape:

```bash
cargo test test_pi_model_arg_shapes
cargo test test_pi_external_cli_model_args_split_custom_provider_colon_model
```

Those tests passed in this worktree and assert custom
`pi:<provider>:<model>` splitting. They do not yet name `openai-codex`
specifically.

Important caveat: WG's dedicated `wg pi-handler` path currently has no
first-class reasoning argument. The shell/external-CLI path can inject model args
for `executor_type = "pi"`, but neither path derives or passes a WG reasoning
setting today.

## Reasoning Behavior

Verified Pi levels in installed `0.80.6`:

```text
off, minimal, low, medium, high, xhigh, max
```

Pi docs and installed source represent thinking as:

- CLI flag: `--thinking <level>`.
- Model shorthand: `--model <pattern>:<thinking>`.
- Settings key: `defaultThinkingLevel`.
- Extension API: `pi.getThinkingLevel()` / `pi.setThinkingLevel(...)`.
- Model metadata: `thinkingLevelMap`, where omitted means supported with default
  mapping, string means supported with provider-specific value, and `null` means
  unsupported.

Installed Pi exposes `xhigh` and `max` only when a model explicitly maps them.
For installed `openai-codex/gpt-5.6-sol`, both are explicitly mapped:
`xhigh -> xhigh` and `max -> max`; `minimal` maps to provider value `low`.
Source:
`/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai/dist/providers/openai-codex.models.js:97`

Installed Pi clamps unsupported thinking levels in
`@earendil-works/pi-ai/dist/models.js:206`. If a model is non-reasoning, only
`off` is supported. If a model does not explicitly map `xhigh` or `max`, those
levels are filtered out.

For Codex Responses, installed Pi's type declaration accepts:

```text
none, minimal, low, medium, high, xhigh, max
```

as `reasoningEffort` values. Source:
`/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai/dist/api/openai-codex-responses.d.ts:4`

## Recommended WG Contract

### Model Spec

Use:

```text
pi:openai-codex:<model-id>
```

Examples:

```bash
wg config -m 'pi:openai-codex:gpt-5.6-sol'
wg add 'Refactor parser' --model 'pi:openai-codex:gpt-5.6-sol'
wg add 'Use cheaper Codex tier' --model 'pi:openai-codex:gpt-5.6-luna'
```

Do not recommend these:

```text
pi:openai-codex:gpt-5.6-sol:xhigh
pi:openai-codex:gpt-5.6-sol(high)
pi:openai-codex/gpt-5.6-sol
openai-codex:gpt-5.6-sol
```

The compact `model(high)` style is attractive for one-off human input, but WG
should not make it canonical. It conflates two independent decisions:
which provider/model to run and how much reasoning budget to request. It also
creates quoting and parsing edge cases in shell commands, TOML, status displays,
and future model ids that might themselves contain punctuation. Treat compact
forms, if ever accepted, as UI sugar that normalizes immediately to separate
structured fields:

```text
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "high"
```

`pi:openai-codex/gpt-5.6-sol` happens to fit the generic slash splitter in one
WG spawn path, but the dedicated `pi_handler::pi_model_arg` treats slash forms
primarily as OpenRouter-style routes. The canonical form should be
colon-separated provider/model after the `pi:` handler prefix.

### Reasoning Config

Add a separate nullable field, not a model-string suffix:

```toml
[models.default]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "high"

[models.task_agent]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "high"

[tiers.premium]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "xhigh"
```

Suggested field name: `reasoning`. Accepted values should include Pi `0.80.6`'s
common set:

```text
off, minimal, low, medium, high, xhigh, max
```

WG should keep this executor-agnostic where possible, then each handler maps,
rejects, or warns for unsupported values. For Pi, pass it as
`--thinking <level>` in RPC/spawn paths and as the equivalent env/host option in
the Node-host topology.

Recommended user-facing CLI shape if WG adds first-class reasoning:

```bash
wg config -m 'pi:openai-codex:gpt-5.6-sol' --reasoning high
wg add 'Refactor parser' --model 'pi:openai-codex:gpt-5.6-sol' --reasoning high
wg spawn 'Hard migration' --model 'pi:openai-codex:gpt-5.6-sol' --reasoning xhigh
```

Those commands should store or pass the same structured model/reasoning pair as
the TOML examples. They should not persist a compact string such as
`pi:openai-codex:gpt-5.6-sol(high)`.

### Precedence

Recommended precedence, highest first:

1. Explicit task override: `wg add --reasoning <level>` or future task field.
2. Role-specific `[models.<role>].reasoning`.
3. Tier default `[tiers.<tier>].reasoning`.
4. Profile default reasoning, if present.
5. Global/default `[models.default].reasoning`.
6. Handler default. For Pi, omit `--thinking` and let Pi use its own CLI/settings
   default.

Reasoning should inherit independently from model. A task can override reasoning
without repeating its model, and a model override without reasoning should not
erase the inherited reasoning unless explicitly set to `null` or `inherit`.

### Validation And Status

Validation should be warning-first:

- Accept `pi:<provider>:<model>` syntactically for any non-empty provider/model,
  because Pi supports custom providers and custom model ids.
- If `pi --list-models <filter> --offline` can verify a provider/model pair,
  status should show `listed`.
- If Pi accepts but warns "Using custom model id", status should show
  `custom/unlisted`.
- If a reasoning level is not in the common WG set, reject at config parse/lint.
- If the model is listed and Pi metadata says a reasoning level is unsupported,
  warn or fail during `wg config lint`; at spawn time, pass the requested level
  and let Pi clamp only when WG cannot inspect metadata.

Status display should include handler, Pi provider, Pi model, and reasoning:

```text
handler=pi provider=openai-codex model=gpt-5.6-sol reasoning=high model_status=listed auth=pi-oauth
handler=pi provider=openai-codex model=gpt-5.6-luna reasoning=low model_status=listed auth=pi-oauth
```

## Workload Recommendations

These are recommendations, not verified benchmark facts.

| WG workload | Recommended Pi reasoning | Rationale |
|---|---:|---|
| Chat / coordinator | `medium` | Better interaction quality than `low` without the latency/cost of `high` on every turn. |
| Standard workers | `high` | Coding tasks benefit from deliberate reasoning; cost is justified by fewer failed attempts. |
| Premium / complex workers | `xhigh` | Use for architecture, hard debugging, risky migrations, and long-horizon tasks. Expect higher latency and token spend. |
| Exceptional manual escalation | `max` | Available for `gpt-5.6-*`, but should be opt-in because it likely increases latency and spend. Do not make it a default tier. |
| Weak agency one-shots | `low` | Assignment/eval/flip verdicts are short and recoverable; keep them cheap. Use `medium` only if verdict quality becomes a bottleneck. |

For installed `openai-codex/gpt-5.6-sol`, avoid `minimal` as a distinct policy
knob: Pi maps it to provider value `low`, so `minimal` does not buy a clearly
separate Codex behavior in this catalog.

## Proposed Implementation Tasks

No production code was changed for this research task. If WG chooses to implement
the contract above, keep it small:

1. Add reasoning config plumbing.
   Files: `src/config.rs`, `src/config_defaults.rs`, `src/commands/config_cmd.rs`,
   profile templates.
   Acceptance: TOML round-trip tests for `[models.*].reasoning` and
   `[tiers.*].reasoning`; `wg config --models` displays inherited reasoning.

2. Pass reasoning to Pi.
   Files: `src/commands/pi_handler.rs`, `src/commands/spawn/execution.rs`,
   `src/service/llm.rs`, `pi-plugin/host/wg-pi-host.mjs` if Node topology needs
   an env/API bridge.
   Acceptance: tests assert `rpc_spawn_args` includes `--thinking high`; external
   Pi command construction includes `--thinking`; one-shot Pi agency calls use
   weak-tier reasoning.

3. Add Pi Codex-specific routing tests.
   Files: `src/commands/pi_handler.rs`, `src/commands/spawn/execution.rs`,
   `src/dispatch/handler_for_model.rs`.
   Acceptance: `pi:openai-codex:gpt-5.6-sol` routes to `ExecutorKind::Pi` and
   produces `--provider openai-codex --model gpt-5.6-sol` without rewriting the
   provider.

4. Add lint/status visibility.
   Files: `src/commands/config_cmd.rs`, `src/commands/model_cmd.rs` or status
   surfaces.
   Acceptance: `wg config lint` warns for unlisted Pi models when Pi is available
   but does not reject syntactically valid custom ids; status shows handler,
   provider, model, reasoning, and listed/custom state.

## User Command Examples

Login to Pi's Codex subscription provider:

```bash
pi
/login
# choose "ChatGPT Plus/Pro (Codex)"
```

Headless environments can use Pi's device-code option from that login flow.

Verify available installed Pi Codex models:

```bash
pi --list-models codex --offline
```

Run Pi directly with Codex subscription auth:

```bash
pi --provider openai-codex --model gpt-5.6-sol --thinking high \
  -p 'Inspect this repository and summarize the test strategy'
```

Exercise the highest listed reasoning level directly:

```bash
pi --provider openai-codex --model gpt-5.6-sol --thinking max \
  --no-tools --no-session --offline -p 'Reply with ok'
```

Configure WG to use the verified installed model:

```bash
wg config -m 'pi:openai-codex:gpt-5.6-sol'
wg add 'Implement the parser cleanup' --model 'pi:openai-codex:gpt-5.6-sol'
```

WG now has a first-class reasoning field, so use the same model identity with a
separate reasoning setting:

```bash
wg config -m 'pi:openai-codex:gpt-5.6-sol' --reasoning high
wg add 'Implement the parser cleanup' --model 'pi:openai-codex:gpt-5.6-sol' --reasoning high
wg spawn 'Hard migration' --model 'pi:openai-codex:gpt-5.6-sol' --reasoning xhigh
```

WG stores this as structured `model` plus `reasoning`, resolves them
independently, and passes Pi `--thinking <level>` only when reasoning is
configured. When reasoning is unset, WG omits `--thinking` and Pi keeps its own
default. WG should not encode reasoning in the model spec.
