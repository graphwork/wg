# GPT-5.6 Codex and Pi profile defaults (2026-07-11)

## Scope and conclusion

Research performed 2026-07-11 (UTC). No live WG profile, daemon, task, source configuration, or GitHub state was changed. Authenticated probes were ephemeral/read-only.

**Recommendation:** use **GPT-5.6 Sol** for WG chat/default/strong workers and premium/hard work; use **GPT-5.6 Luna at low reasoning** for the short, well-specified `.assign`/`.flip`/evaluation one-shots. Terra is the sensible cost-saving everyday worker, but not the quality-first strong default. Luna is recommended because OpenAI explicitly positions it for clear, repeatable, high-volume work and because it is the fastest/cheapest 5.6 tier, not because of its name. Keep `gpt-5.4-mini` as the rollback/absolute-lowest-cost agency option: its published API price ($0.75/$4.50 per million input/output) remains below Luna ($1/$6).

The correct handler-first routes are:

- Direct Codex CLI: `codex:gpt-5.6-sol`, `codex:gpt-5.6-terra`, `codex:gpt-5.6-luna`.
- Pi using ChatGPT/Codex-account OAuth: `pi:openai-codex:gpt-5.6-sol`, `pi:openai-codex:gpt-5.6-terra`, `pi:openai-codex:gpt-5.6-luna`.
- `openai-codex:gpt-5.6-sol` is **not** handler-first WG syntax. It names a Pi provider without naming the Pi handler and must not be used.

## Official evidence (retrieved 2026-07-11)

| Claim | Official evidence |
|---|---|
| GPT-5.6 is generally available; Sol is flagship, Terra balanced/everyday, Luna fastest/most affordable. | [OpenAI launch post](https://openai.com/index/gpt-5-6/) |
| GPT-5.6 is available in Codex; Plus/Pro/Business/Enterprise can select Sol/Terra/Luna, while Free/Go get Terra. | [OpenAI launch post](https://openai.com/index/gpt-5-6/), [Help Center](https://help.openai.com/en/articles/20001325-a-preview-of-gpt-56-sol-terra-and-luna) |
| Codex's default **Power** setting is `gpt-5.6-sol` with medium reasoning; GPT-5.5 is described as previous-generation. | [Codex models](https://developers.openai.com/codex/models) |
| OpenAI says Sol is for complex/open-ended/high-value work, Terra is the everyday workhorse and natural GPT-5.5 replacement, and Luna is for clear/repeatable extraction, classification, transformation, and structured summaries. | [Codex models](https://developers.openai.com/codex/models) |
| The API's `gpt-5.6` alias resolves to Sol. | [Sol model metadata](https://developers.openai.com/api/docs/models/gpt-5.6-sol) |
| Sol/Terra/Luna API context is 1,050,000 with 128,000 maximum output; pricing is respectively $5/$30, $2.50/$15, $1/$6 per million input/output. | [Sol](https://developers.openai.com/api/docs/models/gpt-5.6-sol), [Terra](https://developers.openai.com/api/docs/models/gpt-5.6-terra), [Luna](https://developers.openai.com/api/docs/models/gpt-5.6-luna) |
| Official coding results are Sol 80.0, Terra 77.4, Luna 74.6, GPT-5.5 76.4 on Artificial Analysis Coding Agent Index; Sol leads the family, while Terra is close and Luna remains capable. | [OpenAI launch post](https://openai.com/index/gpt-5-6/) |
| Codex supports Low, Medium, High, Extra High; Max is single-agent extra compute; Ultra is multi-agent and is not a reasoning-effort synonym. | [Codex models](https://developers.openai.com/codex/models) |

**Answers to Questions 1–3.** GPT-5.5 has been superseded for current Codex recommendations: it remains accepted but the official page labels it previous-generation. The documented Codex Power default is Sol/medium. Sol, Terra, and Luna are now public GA identifiers for Codex CLI and the OpenAI API, not private codenames. They are also explicit models in the installed Pi `openai-codex` registry. Strong quality-first workers should use Sol. Terra is an optional routine/cost-optimized worker. Luna is the 5.6 agency choice because the official role matches bounded one-shot classification/judgment and it is 5x cheaper than Sol at API list price; use 5.4-mini instead only when minimizing price/quota matters more than staying on the 5.6 generation.

## Local versions and registries

| Item | Observed 2026-07-11 |
|---|---|
| Codex CLI | `codex-cli 0.144.1`; binary `/home/bot/.nvm/versions/node/v25.4.0/bin/codex` |
| Pi CLI / pi-ai registry | `0.80.6` / `0.80.6`; binary `/home/bot/.nvm/versions/node/v25.4.0/bin/pi` |
| Pi list | All three `openai-codex` IDs listed with 372K context, 128K max output, thinking and image input enabled. |
| Pi static metadata | `openai-codex.models.js` records 372,000 context and list-price accounting; `minimal -> low`, plus native `xhigh` and `max` mappings for all three models. See installed file lines 78–133. |
| Codex cache | Refreshed at `2026-07-11T20:53:05Z`, client `0.144.1`; still listed only GPT-5.5, 5.4, 5.4-mini, 5.3-Codex-Spark and auto-review, even though all authenticated 5.6 requests succeeded. This is rollout/cache inconsistency, not rejection. |

Do not advertise the API's 1.05M context for these subscription handlers. Pi deliberately exposes 372K, while the locally cached Codex metadata did not expose 5.6 context at all. Treat effective context as handler/account-specific until the client registries converge.

## Authenticated read-only probe matrix

Commands ran in fresh `/tmp` directories on 2026-07-11 around 20:49–20:52 UTC. Direct probes used `codex exec --ephemeral --ignore-user-config --ignore-rules --skip-git-repo-check -s read-only --json -c model_reasoning_effort=\"low\"`; Pi probes used `--mode json --print --no-session --no-tools --no-extensions --no-skills --no-prompt-templates --no-context-files --thinking low`. Prompt: reply exactly `OK`. Timing is one concurrent cold-ish sample, not a benchmark.

| WG route / actual handler dialect | Accepted? | Wall time | Usage/accounting observation |
|---|---:|---:|---|
| `codex:gpt-5.6-sol` / `codex -m gpt-5.6-sol` | Yes | 3.35s | 11,609 input, 8,960 cached, 5 output; Codex event has token counts but no dollar cost. |
| `codex:gpt-5.6-terra` / `codex -m gpt-5.6-terra` | Yes | 3.25s | 11,611 input, 9,472 cached, 5 output; no dollar cost. |
| `codex:gpt-5.6-luna` / `codex -m gpt-5.6-luna` | Yes | 2.75s | 10,420 input, 8,960 cached, 5 output; no dollar cost. |
| `pi:openai-codex:gpt-5.6-sol` / `pi --provider openai-codex --model gpt-5.6-sol` | Yes | 3.65s | 428 input, 5 output; Pi reported estimated cost $0.002290. |
| `pi:openai-codex:gpt-5.6-terra` / matching Pi args | Yes | 4.32s | 429 input, 5 output; estimated cost $0.0011475. |
| `pi:openai-codex:gpt-5.6-luna` / matching Pi args | Yes | 3.69s | 429 input, 5 output; estimated cost $0.000459. |

Pi's dollar value is registry/list-price accounting, not proof that a ChatGPT subscription was charged that amount. WG's Pi stream bridge can ingest Pi's per-turn usage/cost. Direct Codex emits tokens but not cost, so WG cannot derive an account charge from this probe.

## Reasoning and verbosity

Reasoning effort and response verbosity are independent controls. Codex metadata has `model_reasoning_effort` and `model_verbosity`; Pi/WG `reasoning` maps to Pi `--thinking` and does not set verbosity.

| Surface | Verified behavior | WG propagation today |
|---|---|---|
| Direct Codex CLI | Low succeeded for all models. Luna `max` succeeded. Luna `minimal` was rejected by the backend with HTTP 400; it named supported internal values `none`, `low`, `medium`, `high`, `xhigh`. Official Codex UX additionally documents Max. | **Missing adapter.** `append_external_cli_reasoning_args` returns unless executor is `pi` (`src/commands/spawn/execution.rs:1200–1212`). A WG task/profile `reasoning` is resolved and recorded but is not passed as Codex `-c model_reasoning_effort=...`. `off` would also need translation to Codex `none`; do not pass WG `minimal` verbatim. |
| Pi `openai-codex` | `off`, `minimal`, `low`, and `max` probes succeeded; Pi help accepts `off|minimal|low|medium|high|xhigh|max`. Registry explicitly maps `minimal` to `low` and `max` to `max`. | Supported for workers/chat and agency: WG appends `--thinking` for Pi external execution (`execution.rs:1200–1212`) and Pi one-shots (`src/service/llm.rs:896–914, 1036–1060`). |

Direct Codex profile reasoning should therefore be **omitted** today, leaving Codex's documented/model default medium. Adding reasoning keys would make `wg status/show` look configured without changing the direct CLI request. Operators can set a single global `model_reasoning_effort` in Codex's own config, but that is not role-aware and is not recommended as a substitute for a WG adapter.

## Decision table

| Role | Model | Desired reasoning | Direct Codex actual today | Why |
|---|---|---|---|---|
| Chat/default | Sol | medium | medium default (omit WG setting) | Official Power default; strong interactive quality. |
| Strong/task worker | Sol | high | medium default until adapter | Quality-first, ambiguous implementation work. |
| Premium/hard/verification | Sol | xhigh; Max only explicit escalation | medium default until adapter | Strongest family member; avoid routine Max quota/latency. |
| Routine cost-saving worker (optional) | Terra | high | medium default until adapter | Natural GPT-5.5 replacement at half Sol list price. |
| Weak agency (`assign`, `evaluate`, `flip`) | Luna | low | medium default until adapter | Officially suited to clear/repeatable classification/transformation; cheapest 5.6 tier. |

## Profile A: direct Codex CLI

Create without touching the built-in `codex` profile:

```bash
mkdir -p ~/.wg/profiles
cat > ~/.wg/profiles/codex-56.toml <<'TOML'
description = "Direct Codex CLI: GPT-5.6 Sol strong, Luna agency"

[agent]
model = "codex:gpt-5.6-sol"

[dispatcher]
model = "codex:gpt-5.6-sol"

[tiers]
fast = "codex:gpt-5.6-luna"
standard = "codex:gpt-5.6-sol"
premium = "codex:gpt-5.6-sol"

[models.default]
model = "codex:gpt-5.6-sol"

[models.task_agent]
model = "codex:gpt-5.6-sol"

# Reasoning intentionally omitted: WG does not yet adapt it to Codex CLI.
[models.evaluator]
model = "codex:gpt-5.6-luna"

[models.assigner]
model = "codex:gpt-5.6-luna"

[models.flip_inference]
model = "codex:gpt-5.6-luna"

[models.flip_comparison]
model = "codex:gpt-5.6-luna"
TOML
wg profile show codex-56
wg profile use codex-56
wg config --models
wg service status
```

This uses Sol rather than Terra for strong workers. To operate a cost-first standard pool, change only `tiers.standard` to `codex:gpt-5.6-terra`; keep `models.task_agent` on Sol if default workers must remain strong.

## Profile B: Pi + OpenAI Codex account

```bash
mkdir -p ~/.wg/profiles
cat > ~/.wg/profiles/pi-codex-56.toml <<'TOML'
description = "Pi OpenAI-Codex OAuth: GPT-5.6 Sol strong, Luna agency"

[agent]
model = "pi:openai-codex:gpt-5.6-sol"

[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"

[tiers]
fast = "pi:openai-codex:gpt-5.6-luna"
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
reasoning = "high"

[models.evaluator]
model = "pi:openai-codex:gpt-5.6-luna"
reasoning = "low"

[models.assigner]
model = "pi:openai-codex:gpt-5.6-luna"
reasoning = "low"

[models.flip_inference]
model = "pi:openai-codex:gpt-5.6-luna"
reasoning = "low"

[models.flip_comparison]
model = "pi:openai-codex:gpt-5.6-luna"
reasoning = "low"
TOML
wg profile show pi-codex-56
wg profile use pi-codex-56
wg pi-plugin status
wg config --models
wg service status
```

Do not use `wg profile pi --strong ... --weak ...` for this custom profile: that command specifically edits the named `pi` starter. The custom file avoids overwriting the current Pi/OpenRouter setup and makes rollback atomic.

## Existing task pins and daemon launch overrides

Profiles do not override explicit task model pins. Inventory before activation:

```bash
stamp=$(date -u +%Y%m%dT%H%M%SZ)
out="/tmp/wg-model-pins-before-$stamp.tsv"
for id in $(wg list --json | jq -r '.[].id'); do
  wg show "$id" --json | jq -r '[.id, (.status // ""), (.model // ""), (.resolved_reasoning // "")] | @tsv'
done | tee "$out"
rg 'codex:gpt-5\.(5|4)|openai-codex:gpt-5\.(5|4)|(^|\t)gpt-5\.(5|4)' "$out" || true
```

After reviewing the list, migrate only intended pending/open pins explicitly:

```bash
wg edit TASK_ID --model codex:gpt-5.6-sol                 # direct
wg edit TASK_ID --model pi:openai-codex:gpt-5.6-sol      # Pi wrapper
# Pi only; direct Codex WG reasoning is not propagated yet:
wg edit TASK_ID --reasoning high
```

A service launch `--model` override can outrank profile routing. Check both process and systemd state:

```bash
wg service status
wg config --models
ps -eo pid,args | rg '[w]g .*service (start|daemon)|[w]g .*--model'
systemctl --user cat wg.service 2>/dev/null || true
systemctl --user show wg.service -p ExecStart -p Environment -p FragmentPath -p DropInPaths 2>/dev/null || true
systemctl --user list-unit-files | rg -i '(^|-)wg.*service' || true
```

If `ExecStart`, a drop-in, shell wrapper, or manually launched process contains `--model`, remove that override through the same mechanism that created it, then `wg service restart`; do not merely activate a profile and assume it won.

## Rollback

Before activation record the current profile and task-pin TSV. Roll back atomically with the prior named profile:

```bash
old_profile=$(cat ~/.wg/active-profile 2>/dev/null || printf 'claude')
printf '%s\n' "$old_profile" > /tmp/wg-profile-before-gpt56
# ... activate/test new profile ...
wg profile use "$(cat /tmp/wg-profile-before-gpt56)"
wg service status
wg config --models
```

Restore each intentionally migrated task from the saved TSV with `wg edit TASK_ID --model OLD_HANDLER_FIRST_MODEL`; restore its old reasoning only if it had an explicit task value. For the agency tier, changing Luna back to `codex:gpt-5.4-mini` (direct) or `pi:openai-codex:gpt-5.4-mini` (Pi) is the lowest-risk cost rollback. Already-running workers keep their model; profile activation/reload affects subsequent spawns.

## Uncertainties and account-specific behavior

- Availability is plan, workspace-admin, region, rollout, trust/safety, and quota dependent. This Plus/Pro-authenticated environment accepted all six routes, but another account may expose only Terra or reject a tier.
- The local Codex registry was stale/incomplete relative to both official docs and successful authenticated calls. Test all routes on the target account before bulk pin migration.
- The trivial concurrent latency samples characterize startup/first-token only; they do not rank difficult-task latency or output quality. Official evals, intended-use text, and price are the basis of the tier decision.
- API list-price accounting is not the same as ChatGPT subscription quota/account charging. Pi emits estimated dollars from its registry; direct Codex emits no dollars.
- Pi's 372K subscription-handler context differs from official API metadata's 1.05M. Use the lower advertised handler limit.
- `max` behavior is evolving: official Codex docs support it and both direct/Pi probes accepted it, but a Codex `minimal` error enumerated only internal `none|low|medium|high|xhigh`. Use Max only for explicit escalation; never encode Ultra as reasoning because Ultra is multi-agent orchestration.

## Focused follow-up tasks

1. **Add a direct-Codex reasoning adapter.** Map WG `off -> -c model_reasoning_effort=\"none\"`, `minimal -> low`, pass low/medium/high/xhigh/max, cover workers and Codex agency one-shots, and separately support `model_verbosity` rather than conflating it with reasoning.
2. **Add a generic/custom two-tier profile CLI surface.** `wg profile pi` only edits the profile literally named `pi`; provide safe strong/weak updates for `pi-codex-56` or document `profile set-model` plus reasoning setters.
3. **Refresh starter metadata after staged rollout.** Once account availability is broad, update the built-in Codex starter from GPT-5.5/5.4-mini and add a model-registry drift test that notices when official/current Codex models are accepted but absent from `models_cache.json`.
