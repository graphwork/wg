# Executor Arena Research

Date: 2026-05-31
Task: `exec-research-synthesis`

## Executive Brief

WG should treat external worker executors as a small family of one-shot,
non-interactive CLI adapters, not as coordinator backends. The stable adapter
shape is:

1. Resolve WG's `provider:model` spec once.
2. Normalize the model into the target CLI's model flag format.
3. Materialize the full WG task prompt to a file.
4. Invoke the CLI in non-interactive mode with explicit working directory,
   model, approval, and output flags.
5. Let the task finish through the existing WG contract (`wg log`,
   `wg artifact`, `wg done`, `wg fail`), not by interpreting the CLI's
   natural-language output as success.

Ranking by fit for WG worker execution:

| Rank | Candidate | Classification | Fit | Recommendation |
|---:|---|---|---|---|
| 0 | Rust-native WG native executor | Strategic baseline | Highest long-term fit, but not an external adapter | Keep as the primary control point for secrets, endpoint selection, tool policy, and OpenAI-compatible smoke tests. |
| 1 | OpenCode | Stable adapter | Best external CLI fit | Implement first external template after prompt-delivery support. It has `opencode run`, `--model provider/model`, JSON event output, permissions skip, OpenRouter/provider config, and an ACP path for later. |
| 2 | Goose | Stable adapter | Very strong automation fit | Implement alongside OpenCode or immediately after. `goose run` has `--provider`, `--model`, `--no-session`, stdin/file input, JSON/stream-json output, and official OpenRouter/custom OpenAI provider support. |
| 3 | Qwen Code | Stable adapter | Strong headless fit, provider config more involved | Good for OpenAI-compatible and Qwen-oriented routes. Use `modelProviders` in `~/.qwen/settings.json`, then run headless with `qwen -p` or stdin and stream-json output. |
| 4 | Cline | Stable adapter, younger CLI surface | Good headless and provider story | Useful because it has first-class provider/model flags, JSONL output, auto-approve, command permissions, and OpenRouter auth. Validate current flag spelling in CI because docs have both old and current CLI pages. |
| 5 | Crush | Stable adapter, verify before bundling | Good Unix CLI shape | `crush run` has prompt args/stdin, `--model`, `--small-model`, `--quiet`, `--yolo`, provider auto-updates, and OpenAI-compatible config. Verify with `crush run --help` before shipping because the most precise CLI reference is generated. |
| 6 | Aider | Stable CLI, weak WG-worker fit | Mature but workflow-mismatched | Keep as a niche template for edit-heavy tasks. It is optimized for pair programming, repo maps, and file edit formats rather than WG's task-agent contract. |
| 7 | Amplifier | Experimental template | Useful ecosystem, high integration cost | Keep optional. Existing research recommends native WG for the core loop and Amplifier only when its Python bundle ecosystem is specifically needed. |

Stable adapters to expose as first-class `ExecutorKind` variants:
`opencode`, `goose`, `qwen`, `cline`, `crush`, `aider`.

Experimental templates to keep out of the default route list:
`amplifier`, plus any raw "custom external executor" path that does not have
explicit prompt-delivery and model-normalization tests.

## WG Repo Findings

The existing design already frames task-agent executors as a config-driven
external class. `docs/design/external-executor-class.md:10-38` says task-agent
spawns load `ExecutorRegistry` configs and can launch custom TOML-backed
executors without Rust changes. The same doc draws the important boundary:
coordinator/chat agents need duplex IPC, while task agents are one-shot prompt
in, output out processes (`docs/design/external-executor-class.md:62-81`).

The native executor roadmap puts external executors behind MCP, compaction, and
token counting because native reliability is the self-bootstrap goal, while
external executors are an adapter consolidation layer
(`docs/design/nex-executor-improvements.md:27-39` and
`docs/design/nex-executor-improvements.md:209-239`).

The Amplifier report is still the best counterweight: it says Amplifier is
well-designed, but for WG's core needs it is a Python wrapper around an LLM
API plus tool routing, and recommends a Rust-native core loop with Amplifier
kept optional (`docs/reports/amplifier-research-report.md:9-15`). It also
documents the minimum native loop as model config, prompt, tool definitions,
tool-use loop, and OpenAI Chat Completions compatibility
(`docs/reports/amplifier-research-report.md:319-332`), then recommends OpenAI
Chat Completions as the target wire format
(`docs/reports/amplifier-research-report.md:416-431`).

One implementation gap is not just design-level. The current spawn path
auto-materializes the WG prompt only for `claude`, `codex`, and `native`;
the code warns that adding a built-in handler without listing it there means
the subprocess can receive an empty prompt
(`src/commands/spawn/execution.rs:900-911`). The generic fallback branch only
joins configured command args and does not write or pipe the prompt
(`src/commands/spawn/execution.rs:1151-1157`). Downstream implementation
should therefore not rely on `args = ["{{prompt}}"]` for real WG prompts.
Add an explicit prompt-delivery mechanism or type-specific command builder for
each new adapter.

## Adapter Categories

### Stable Adapters

These have documented non-interactive execution, documented model selection,
and a plausible no-human WG worker flow.

| Adapter | Command form | OpenRouter model flag format | Prompt delivery | Output mode | Notes |
|---|---|---|---|---|---|
| OpenCode | `opencode run [message..]` | `openrouter/<openrouter-model-id>`, for example `openrouter/deepseek/deepseek-v4-flash` | Message args; for long WG prompts, prefer adding prompt-file support and call `opencode run --file prompt.txt "Complete the attached WG task prompt"` | `--format json` | Official CLI docs list `run`, `--model/-m provider/model`, `--agent`, `--file`, `--format`, `--attach`, and `--dangerously-skip-permissions`. |
| Goose | `goose run --no-session -i <file>` or `goose run --no-session -t "<text>"` | `--provider openrouter --model <openrouter-model-id>` | `-i prompt.txt`, `-i -`, or `-t` | `--output-format json` or `stream-json` | Strongest clean subprocess shape after OpenCode. Also supports `--with-builtin developer` and custom extensions. |
| Qwen Code | `qwen -p "<prompt>"` or `cat prompt.txt \| qwen --output-format stream-json` | `--model <modelProviders entry id>`, usually the raw OpenRouter ID such as `deepseek/deepseek-v4-flash` after configuring `modelProviders.openai[].baseUrl` | `-p`, stdin, `--system-prompt`, `--append-system-prompt` | `--output-format json` or `stream-json` | Good headless mode and persistent retry. Needs a generated `~/.qwen/settings.json` entry for OpenRouter/custom OpenAI endpoints. |
| Cline | `cline [options] [prompt]` or `cat prompt.txt \| cline --json` | `-P openrouter -m <openrouter-model-id>` | Prompt arg or stdin | `--json` JSONL | Good automation flags: `--auto-approve`, `--cwd`, `--provider`, `--model`, `--key`, `--system`, `--timeout`, command permission env. |
| Crush | `crush run [prompt...]` | `--model openrouter/<openrouter-model-id>` when provider-qualified; raw `<openrouter-model-id>` may work if OpenRouter is default | Prompt args, stdin, or shell redirection | Plain stdout; use `--quiet` | Good terminal-agent ergonomics, OpenAI-compatible providers, and `--yolo`; weaker structured-output story than Goose/Qwen/Cline/OpenCode. |
| Aider | `aider --message-file prompt.txt --yes-always` | `--model openrouter/<openrouter-model-id>` | `--message` or `--message-file` | Text log-oriented | Mature and stable, but less WG-shaped. Disable/understand auto-commit behavior before using as a worker. |

### Experimental Templates

| Adapter | Why experimental | Command/config shape |
|---|---|---|
| Amplifier | Existing report identifies early-preview ecosystem, Python/uv module loading, provider naming gaps, and local patch history. It is useful when bundles/recipes/behaviors matter, not as the default WG worker path. | Current WG bundle flow is `amplifier run --mode single --output-format json --bundle wg "$PROMPT"`, with OpenRouter/default model in bundle/provider config rather than a clean WG model flag. |
| Generic custom executor TOML | The raw config schema can launch a process, but current spawn plumbing does not auto-write or pipe prompts for unknown executor types. | Do not document arbitrary TOML as "stable worker adapter" until prompt delivery and model normalization are first-class fields. |

### Rust-Native Counterpoint

Rust-native execution is not an adapter command; it is WG's internal worker
path. Its command equivalent is `wg service start` spawning a `native`
worker with a WG model spec such as
`openrouter:deepseek/deepseek-v4-flash`, then sending the raw
OpenAI-compatible request body with `model = "deepseek/deepseek-v4-flash"`.
This path should remain the reference behavior for endpoint selection, secret
injection, tool-loop semantics, and smoke-test assertions. External CLIs should
match its WG lifecycle effects, not become the lifecycle source of truth.

## Command And Model Normalization Matrix

WG model specs should remain `provider:model`, for example
`openrouter:deepseek/deepseek-v4-flash`. Adapter code should strip the WG
provider prefix and then apply the target CLI's provider syntax.

| WG spec | Native/OpenRouter API | OpenCode | Aider | Goose | Qwen Code | Cline | Crush | Amplifier |
|---|---|---|---|---|---|---|---|---|
| `openrouter:deepseek/deepseek-v4-flash` | `model = "deepseek/deepseek-v4-flash"` | `--model openrouter/deepseek/deepseek-v4-flash` | `--model openrouter/deepseek/deepseek-v4-flash` | `--provider openrouter --model deepseek/deepseek-v4-flash` | `--model deepseek/deepseek-v4-flash` after provider catalog entry | `-P openrouter -m deepseek/deepseek-v4-flash` | `--model openrouter/deepseek/deepseek-v4-flash` | bundle/provider `default_model: deepseek/deepseek-v4-flash` |
| `openrouter:deepseek/deepseek-v3.2` | `model = "deepseek/deepseek-v3.2"` | `--model openrouter/deepseek/deepseek-v3.2` | `--model openrouter/deepseek/deepseek-v3.2` | `--provider openrouter --model deepseek/deepseek-v3.2` | `--model deepseek/deepseek-v3.2` | `-P openrouter -m deepseek/deepseek-v3.2` | `--model openrouter/deepseek/deepseek-v3.2` | bundle/provider `default_model: deepseek/deepseek-v3.2` |
| `openrouter:minimax/minimax-m2.7` | `model = "minimax/minimax-m2.7"` | `--model openrouter/minimax/minimax-m2.7` | `--model openrouter/minimax/minimax-m2.7` | `--provider openrouter --model minimax/minimax-m2.7` | `--model minimax/minimax-m2.7` | `-P openrouter -m minimax/minimax-m2.7` | `--model openrouter/minimax/minimax-m2.7` | bundle/provider `default_model: minimax/minimax-m2.7` |

Implementation rule for `exec-openrouter-normalize`: do not pass API keys
through model strings. Normalize only model/provider names. Route secrets via
existing endpoint env injection (`OPENROUTER_API_KEY`, `WG_API_KEY`) or the
target CLI's own auth store.

## Cheap OpenRouter Smoke Models

Fresh check: `curl https://openrouter.ai/api/v1/models` on 2026-05-31.

| Model ID | Context | Tool parameters advertised | Prompt price | Completion price | Smoke-test role |
|---|---:|---|---:|---:|---|
| `deepseek/deepseek-v4-flash` | 1,048,576 | includes `tools` and `tool_choice` | $0.0983 / M tokens | $0.1966 / M tokens | First smoke target. Cheapest, huge context, tool-capable. |
| `deepseek/deepseek-v3.2` | 131,072 | includes `tools` and `tool_choice` | $0.252 / M tokens | $0.378 / M tokens | Second target. More established than V4 Flash and still cheap. |
| `minimax/minimax-m2.7` | 204,800 | includes `tools` and `tool_choice` | $0.26 / M tokens | $1.20 / M tokens | Third target. More expensive output, useful provider diversity. |

Use these for adapter smoke tests in this order:

1. `deepseek/deepseek-v4-flash`
2. `deepseek/deepseek-v3.2`
3. `minimax/minimax-m2.7`

Minimum smoke gate for each adapter:

1. Spawn in a temporary WG task worktree.
2. Ask the adapter to run `wg log`, write one tiny artifact, commit it, and
   mark the task done.
3. Confirm the WG graph state changed, not just that the CLI exited zero.
4. Confirm no API key appears in process args, output logs, or artifacts.

## Implementation Recommendations

1. Add `ExecutorKind` variants for `opencode`, `aider`, `goose`, `qwen`,
   `cline`, `crush`, and `amplifier`, but gate default profile exposure to
   stable adapters.
2. Add an `ExternalCliAdapter` table or enum method that records:
   `command`, `prompt_delivery`, `model_flag_style`, `output_format`,
   `approval_flags`, and `supports_structured_output`.
3. Extend `executor_uses_auto_prompt` or replace it with a prompt-delivery
   field so every first-class adapter gets a `prompt.txt`.
4. Add one normalizer per model flag style:
   `ProviderPrefixedSlash`, `ProviderFlagPlusRawModel`, `RawModelFromCatalog`,
   and `BundleConfigOnly`.
5. For initial smoke tests, prefer adapters with stdin/file prompt delivery:
   Goose, Qwen Code, Cline, Crush, and Aider are simpler than OpenCode here.
   OpenCode still ranks first overall because its model/provider and automation
   surface is the cleanest once prompt-file delivery is handled.
6. Keep native WG/OpenRouter smoke tests as the baseline; external adapters
   should prove they can satisfy WG's task lifecycle, not replace the native
   executor's endpoint and secret handling.

## Source Checks

Repo sources:

- `docs/design/external-executor-class.md:10-38` - existing task-agent
  executor config class.
- `docs/design/external-executor-class.md:62-81` - one-shot task agents vs
  duplex coordinator IPC.
- `docs/design/nex-executor-improvements.md:43-62` - peer executors have MCP,
  and native WG should close that gap.
- `docs/design/nex-executor-improvements.md:209-239` - external executor
  class is step 4, lower priority than native reliability.
- `docs/reports/amplifier-research-report.md:9-15` - native primary,
  Amplifier optional.
- `docs/reports/amplifier-research-report.md:319-354` - native loop minimum
  and hard parts.
- `docs/reports/amplifier-research-report.md:416-431` - OpenAI Chat
  Completions target wire format.
- `src/commands/spawn/execution.rs:900-911` - auto-prompt whitelist.
- `src/commands/spawn/execution.rs:1076-1140` - codex/native prompt handling.
- `src/commands/spawn/execution.rs:1151-1157` - generic executor fallback.

Upstream docs checked:

- OpenCode CLI docs: `https://dev.opencode.ai/docs/cli/`
- OpenCode provider docs: `https://dev.opencode.ai/docs/providers/`
- Aider options reference: `https://aider.chat/docs/config/options.html`
- Aider OpenRouter docs: `https://aider.chat/docs/llms/openrouter.html`
- Goose running tasks: `https://goose-docs.ai/docs/guides/running-tasks/`
- Goose provider docs: `https://goose-docs.ai/docs/getting-started/providers/`
- Qwen Code headless mode: `https://qwenlm.github.io/qwen-code-docs/en/users/features/headless/`
- Qwen Code model providers: `https://github.com/QwenLM/qwen-code/blob/main/docs/users/configuration/model-providers.md`
- Cline CLI reference: `https://docs.cline.bot/cli/cli-reference`
- Cline OpenRouter/provider setup: `https://docs.cline.bot/provider-config/openrouter`
- Crush README/config: `https://github.com/charmbracelet/crush`
- Crush `run` command reference: `https://www.mintlify.com/charmbracelet/crush/cli/run`
- OpenRouter chat completion API: `https://openrouter.ai/docs/api-reference/chat-completion`
- OpenRouter models API docs: `https://openrouter.ai/docs/guides/overview/models`
