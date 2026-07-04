# Current OpenRouter Pi Profile Scout - 2026-07-04

Task: `scout-current-openrouter`

## Recommendation

Use this two-tier Pi profile:

```bash
wg profile pi --strong pi:openrouter/z-ai/glm-5.2 --weak openrouter:deepseek/deepseek-v4-flash
```

Rationale:

- Strong tier: `pi:openrouter/z-ai/glm-5.2`
  - Best current default for Pi/OpenRouter chat, worker, and coding-agent work.
  - OpenRouter describes GLM 5.2 as a 1M-context reasoning model suited for long-horizon agent workflows, project-level software engineering, and complex multi-step automation.
  - Current OpenRouter API metadata reports tool calling, parallel tool calls, structured outputs, reasoning controls, and a 1,048,576-token catalog context.
  - Independent and vendor benchmark evidence both point to GLM 5.2 as the current open-weight quality leader for long-horizon coding/agentic work.
  - Use the `pi:` route for strong tier so workers dispatch through the Pi handler and Pi's own OpenRouter login. Do not persist a raw `openrouter:` strong tier unless intentionally using WG's native OpenRouter client and a WG-side OpenRouter key.

- Weak tier: `openrouter:deepseek/deepseek-v4-flash`
  - Best current default for cheap, reliable agency one-shots.
  - It is far cheaper than GLM 5.2 and most frontier alternatives, has 1M context, supports tools/structured outputs/reasoning controls in OpenRouter metadata, and has cache-read pricing.
  - The weak tier should remain a native `openrouter:` route, not `pi:`, because `.assign`, `.evaluate`, and `.flip` are short one-shot calls where the native route is the intended cheap path with WG's loud keyless fallback behavior.

Keep these alternatives in mind:

- `pi:openrouter/deepseek/deepseek-v4-pro`: best fallback when GLM 5.2 regresses on a WG workload or when DeepSeek cache economics dominate. It is much cheaper than GLM 5.2 and has 1M context, but current benchmark evidence puts GLM 5.2 ahead on open-weight quality.
- `openrouter:deepseek/deepseek-v3.2`: conservative weak fallback if V4 Flash quality or availability is bad on a specific account. It is more mature and explicitly agentic/tool-use trained, but has only 131K catalog context and is more expensive than V4 Flash.
- `pi:openrouter/qwen/qwen3-coder-plus` or `pi:openrouter/qwen/qwen3-coder-flash`: coding-specialist fallback candidates. They have good OpenRouter metadata and caching fields, but were not as strong as GLM 5.2 in the current scout evidence.

## Current Pricing And Capabilities

Snapshot method:

- Date: 2026-07-04.
- Primary exact fields: `curl -s https://openrouter.ai/api/v1/models`.
- Human-readable descriptions/prices cross-checked against OpenRouter model pages.
- Prices below are converted from OpenRouter API per-token fields to USD per 1M tokens.

| Model spec | Tier fit | Input / 1M | Output / 1M | Cache read / 1M | Context | Tool support in OpenRouter metadata | Notes |
| --- | --- | ---: | ---: | ---: | ---: | --- | --- |
| `pi:openrouter/z-ai/glm-5.2` | Strong default | $0.84 API; page showed $0.9086 effective/list snapshot | $2.64 API; page showed $2.856 | $0.156 | 1,048,576 catalog; 1M page | `tools`, `tool_choice`, `parallel_tool_calls`, `structured_outputs`, `response_format`, `reasoning`, `reasoning_effort` | Best quality pick for long-horizon coding and agent work. |
| `openrouter:deepseek/deepseek-v4-flash` | Weak default | $0.09 | $0.18 | $0.018 | 1,048,576 | `tools`, `tool_choice`, `structured_outputs`, `response_format`, `reasoning` | Best weak-tier value. 1M context and very low output cost. |
| `pi:openrouter/deepseek/deepseek-v4-pro` | Strong fallback / value strong | $0.435 | $0.87 | $0.003625 | 1,048,576 | `tools`, `tool_choice`, `structured_outputs`, `response_format`, `reasoning` | Extremely strong cache-read economics; good fallback if GLM 5.2 underperforms locally. |
| `openrouter:deepseek/deepseek-v3.2` | Weak fallback | $0.2288 | $0.3432 | $0.02288 | 131,072 catalog; 131K page | `tools`, `tool_choice`, `structured_outputs`, `response_format`, `reasoning` | Mature agentic/tool-use model, but weaker context/cost than V4 Flash. |
| `pi:openrouter/moonshotai/kimi-k2.7-code` | Strong/value alternative | $0.74 | $3.50 | $0.15 | 262,144 | `tools`, `tool_choice`, `parallel_tool_calls`, `structured_outputs`, `response_format`, `reasoning` | Relevant coding contender, but smaller context and weaker current evidence than GLM 5.2. |
| `pi:openrouter/qwen/qwen3-coder-plus` | Strong/value alternative | $0.65 | $3.25 | $0.13 | 1,000,000 | `tools`, `tool_choice`, `structured_outputs`, `response_format` | Coding-specific, cacheable, lower input than GLM 5.2, but no stronger scout evidence. |
| `openrouter:qwen/qwen3-coder-flash` | Weak/value alternative | $0.195 | $0.975 | $0.039 | 1,000,000 | `tools`, `tool_choice`, `response_format` | Good coding weak fallback, but more expensive output than V4 Flash. |
| `pi:openrouter/x-ai/grok-4.20` | Frontier alternative | $1.25 | $2.50 | $0.20 | 2,000,000 | `tools`, `tool_choice`, `structured_outputs`, `response_format`, `reasoning` | Huge context and good tool metadata, but not the cost/value pick for WG Pi tiers. |
| `pi:openrouter/google/gemini-3.1-pro-preview` | Frontier alternative | $2.00 | $12.00 | $0.20 | 1,048,576 | `tools`, `tool_choice`, `structured_outputs`, `response_format`, `reasoning` | Strong multimodal frontier option; too expensive for the default Pi value profile. |
| `pi:openrouter/anthropic/claude-sonnet-5` | Closed frontier reference | $2.00 | $10.00 | $0.20 | 1,000,000 | `tools`, `tool_choice`, `structured_outputs`, `response_format`, `reasoning` | Strong closed reference, not the OpenRouter value target. |
| `pi:openrouter/anthropic/claude-opus-4.8` | Closed frontier reference | $5.00 | $25.00 | $0.50 | 1,000,000 | `tools`, `tool_choice`, `structured_outputs`, `response_format`, `reasoning` | Quality reference; too expensive for default Pi/OpenRouter tiers. |

## Evidence Used

OpenRouter model pages:

- GLM 5.2: OpenRouter says it has text input/output, 1M context, high/xhigh reasoning efforts, and is suited for long-horizon agent workflows and project-level software engineering. Page: https://openrouter.ai/z-ai/glm-5.2
- DeepSeek V3.2: OpenRouter describes it as efficient, reasoning-oriented, and agentic tool-use capable, with DeepSeek Sparse Attention and a large-scale agentic task synthesis pipeline. Page: https://openrouter.ai/deepseek/deepseek-v3.2
- DeepSeek V4 Flash: OpenRouter describes it as a 284B/13B-active MoE, 1M-context, efficiency-optimized model for fast inference, coding assistants, chat systems, and agent workflows. Page: https://openrouter.ai/deepseek/deepseek-v4-flash
- DeepSeek V4 Pro: OpenRouter describes it as a 1.6T/49B-active MoE for advanced reasoning, coding, long-horizon agent workflows, full-codebase analysis, multi-step automation, and synthesis. Page: https://openrouter.ai/deepseek/deepseek-v4-pro

OpenRouter prompt caching docs:

- OpenRouter uses provider sticky routing for cached requests and supports explicit `session_id` for multi-turn agentic workflows. This matters for Pi because stable sessions can preserve provider/cache stickiness.
- DeepSeek prompt caching is automatic and needs no extra request configuration.
- Cache details can be inspected through `prompt_tokens_details.cached_tokens` and `cache_write_tokens`.
- Source: https://openrouter.ai/docs/guides/best-practices/prompt-caching

Benchmark/scout evidence:

- OpenRouter's June 2026 open-weight roundup put GLM 5.2 at Artificial Analysis Intelligence Index 51, the top open-weight entry in that snapshot, and called DeepSeek V4 Flash the lowest-cost frontier-class agentic coding option on the cost/performance frontier. Source: https://openrouter.ai/blog/insights/the-open-weight-models-that-matter-june-2026/
- Artificial Analysis' GLM 5.2 article reported GLM 5.2 as the leading open-weights model on GDPval-AA v2 and on the intelligence-vs-cost Pareto frontier. Source: https://artificialanalysis.ai/articles/glm-5-2-is-the-new-leading-open-weights-model-on-the-artificial-analysis-intelligence-index
- Z.ai's GLM 5.2 release notes report 1M context, long-horizon coding/agent training, strong long-horizon coding benchmark results, Terminal-Bench 2.1 at 81.0 vs. GLM 5.1's 63.5, and SWE-bench Pro at 62.1 vs. GLM 5.1's 58.4. Source: https://huggingface.co/blog/zai-org/glm-52-blog
- DeepSeek's V3.2 paper reports sparse attention for long-context efficiency, scalable RL, agentic task synthesis, and tool-use benchmark gains, while noting that V3.2 can generate long trajectories that exceed its 128K limit on some tool benchmarks. Source: https://arxiv.org/html/2512.02556v1
- A community Pi/OpenRouter trace analysis over 922 agentic tasks reported DeepSeek V4 Flash at roughly similar tokens/tool calls to Opus 4.7 but dramatically lower average cost, attributing the gap to cache hit rate and cache-read pricing. This is anecdotal, not a primary benchmark, but it is directly relevant to Pi-style agent loops. Source: https://www.reddit.com/r/LocalLLaMA/comments/1t5lywi/i_analyzed_922_agentic_task_trace_and_found_the/

## Why Not DeepSeek V4 Pro As Strong Default?

DeepSeek V4 Pro is a very plausible strong-tier value model:

- 1M context.
- Tool support and structured outputs in OpenRouter metadata.
- Very low API price: $0.435 input / $0.87 output per 1M.
- Extremely low cache-read price: $0.003625 per 1M.

The reason not to make it the default strong tier today is quality calibration. The current OpenRouter/Artificial Analysis evidence puts GLM 5.2 ahead as the open-weight quality leader. For WG workers, failed or meandering agent runs can cost more in wall-clock time and retries than the token price delta. Use GLM 5.2 as default, then run local WG task traces against V4 Pro before switching strong globally.

## Why DeepSeek V4 Flash For Weak?

The weak tier handles short, recoverable one-shot judgments: assignment, evaluation, comparison, and similar agency tasks. The ideal model is cheap, reliable, tool/JSON-capable when needed, and cache-friendly.

DeepSeek V4 Flash fits:

- $0.09 input / $0.18 output per 1M.
- $0.018 cache read per 1M.
- 1M context, useful when agency prompts include task context.
- OpenRouter metadata includes tools, structured outputs, response format, and reasoning controls.
- OpenRouter's own June 2026 roundup identifies it as the cost/performance frontier choice for low-cost agentic coding.

DeepSeek V3.2 remains a fallback because it has explicit agentic training and is older/more proven, but its 131K context and higher price are worse for the weak default.

## `wg model-scout` vs Manual Pinned Commands

Use manual pinned commands for this update.

Reason:

- `wg model-scout --json --no-cache` was attempted in this environment and failed because no OpenRouter API key is configured:

```text
Error: OpenRouter API key required to scout (set one via `wg secret` / config, or OPENROUTER_API_KEY)
```

- The exact model catalog was still available through OpenRouter's public `/api/v1/models`, and the decision depends on current external benchmark/scout evidence that should be reviewed by a human anyway.
- `wg model-scout` is still useful as a re-runnable refresh tool after a key is configured, especially for detecting market churn and printing an apply command. Treat its proposal as a dry-run candidate, not as an automatic replacement for workload-specific judgment.

Recommended refresh workflow when an OpenRouter key is available:

```bash
wg model-scout --json --no-cache
wg model-scout --apply
wg profile pi --show
```

For this scout, apply manually:

```bash
wg profile pi --strong pi:openrouter/z-ai/glm-5.2 --weak openrouter:deepseek/deepseek-v4-flash
```

## Apply And Verify On Another System

Prerequisites:

- Pi CLI authenticated for OpenRouter, because the strong tier routes through `pi:`.
- WG-side OpenRouter key configured if the weak native `openrouter:` agency tier should run on OpenRouter instead of falling back loudly to `claude:haiku`.
- Matching WG/pi plugin setup if this system runs actual Pi workers:

```bash
wg pi-plugin status
wg profile use pi --no-reload
```

Preview without writing:

```bash
wg profile pi --strong pi:openrouter/z-ai/glm-5.2 --weak openrouter:deepseek/deepseek-v4-flash --dry-run
```

Apply:

```bash
wg profile pi --strong pi:openrouter/z-ai/glm-5.2 --weak openrouter:deepseek/deepseek-v4-flash
```

Activate Pi profile for future workers:

```bash
wg profile use pi
```

Verify tier assignment:

```bash
wg profile pi --show
wg profile show
wg config --models
wg config lint
```

Expected routing shape:

```text
strong = pi:openrouter/z-ai/glm-5.2
weak   = openrouter:deepseek/deepseek-v4-flash
```

Verify the strong route does not require a WG-side OpenRouter key:

```bash
wg profile pi --show | rg 'pi:openrouter/z-ai/glm-5.2'
wg config --models | rg 'handler=pi|pi:openrouter/z-ai/glm-5.2'
```

Verify the weak route has a WG-side key or will fall back loudly:

```bash
wg secret check keyring:openrouter || true
printenv OPENROUTER_API_KEY >/dev/null && echo "OPENROUTER_API_KEY set" || echo "No OPENROUTER_API_KEY in env"
```

Optional live smoke:

```bash
wg add "Smoke: Pi profile model route" -d "Ask the worker to report WG_MODEL and complete without code changes."
wg ready
```

One-command revert to the previous starter-style weak tier:

```bash
wg profile pi --strong pi:openrouter/z-ai/glm-5.2 --weak openrouter:deepseek/deepseek-chat
```

## Validation Checklist

- [x] Reported recommended strong and weak model specs in WG command form.
- [x] Included pricing, context, tool support, cache-read pricing, and benchmark/scout evidence.
- [x] Explained `wg model-scout` vs manual pinned commands and recorded the local `wg model-scout` key blocker.
- [x] Included exact commands to apply and verify the profile on another system.
