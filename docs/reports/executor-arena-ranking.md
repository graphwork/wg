# Executor Arena Ranking

- Task: `exec-rank-defaults`
- Date: 2026-06-01
- Primary smoke artifact: [executor-arena-smoke.md](executor-arena-smoke.md)
- Prior synthesis: [executor-arena-research.md](executor-arena-research.md)
- Post-smoke fix context: `fix-nex-openrouter` completed on 2026-06-01 and
  merged as `c4bfcbeb`, with env-backed Nex/OpenRouter credential smoke passing.

## Executive Recommendation

Keep the fresh-install, conservative production default on `claude:opus` through
the Claude CLI handler. It remains WG's most proven unattended worker path for
general task completion, stream translation, and protocol adherence.

Promote native/Nex to the recommended OpenRouter and cost-controlled default
once `exec-final-integration` reruns a cheap live OpenRouter smoke on current
`main`. The original arena smoke found a real Nex env-key failure, but that
failure has since been fixed and pinned by the `nex_wg_openrouter_endpoint_auth`
smoke scenario. Native/Nex is now the strongest technical fit for WG because it
keeps endpoint selection, secrets, tools, stream events, and lifecycle semantics
inside the Rust codebase.

Keep `codex:gpt-5.5` as the stable alternate default for users already
standardizing on Codex. Codex was the strongest live external OpenRouter result
in the original smoke: `codex exec --json --ephemeral` reached OpenRouter with
`deepseek/deepseek-v4-flash`, exited non-interactively, and produced parseable
JSONL without persistent CLI config changes.

Expose external worker CLIs as opt-ins, not global defaults. `opencode`,
`goose`, `qwen`, `cline`, and `aider` have credible command templates and model
normalization, but this host did not have their binaries installed for the
arena. `crush`, `amplifier`, Gemini, and Aether-style integrations should remain
experimental until they have repeatable install, model, output, and full WG
lifecycle smokes.

## Evidence Baseline

The ranking combines three layers of evidence:

1. Live smoke evidence from [executor-arena-smoke.md](executor-arena-smoke.md):
   direct OpenRouter passed, `wg endpoints test openrouter` passed, Codex over
   OpenRouter passed, Claude was installed but not live-spent, and most external
   CLIs were missing.
2. Post-smoke Nex fix evidence from `fix-nex-openrouter`: env-backed endpoint
   credentials now work for `wg nex --eval-mode` and `nex --wg --eval-mode`;
   the fix added `tests/smoke/scenarios/nex_wg_openrouter_endpoint_auth.sh`.
3. Adapter design evidence from
   [executor-arena-research.md](executor-arena-research.md), including command
   shapes, prompt delivery, model normalization, and structured-output support.

The original live smoke and the post-fix Nex smoke are not identical. The live
arena proved that the OpenRouter account, key, and cheap model route work. The
Nex follow-up proved the local credential propagation bug is fixed. A final
live run on current `main` is still the right gate before changing release
defaults.

## Scorecard

Scores are 1-5, where 5 is best. "Install friction" is scored as low friction:
5 means bundled or already installed, 1 means installation and setup are major
prerequisites. Smoke evidence outranks paper fit.

| Rank | Executor | Current smoke status | Reliability | Non-interactive fit | OpenRouter support | Cost control | Install friction | Output parseability | WG protocol adherence | Recommendation |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | `native` / Nex | Original live arena failed on env-key auth; post-fix env-key Nex smoke passed | 4 | 5 | 5 | 5 | 5 | 5 | 5 | Make the OpenRouter/cost-controlled default after final live re-smoke on current `main`. |
| 2 | `claude` CLI | Installed; version/path smoke only in arena to avoid non-OpenRouter spend | 5 | 4 | 1 | 2 | 3 | 5 | 5 | Keep as conservative fresh-install and high-confidence production default. |
| 3 | `codex` CLI | PASS with live OpenRouter via `codex exec --json --ephemeral` | 4 | 5 | 5 | 4 | 3 | 4 | 4 | Stable alternate default and best proven external OpenRouter path. |
| 4 | `opencode` | BLOCKED: binary missing | 3 | 4 | 4 | 4 | 2 | 4 | 3 | First stable external opt-in after install and full lifecycle smoke. |
| 5 | `goose` | BLOCKED: binary missing | 3 | 5 | 4 | 4 | 2 | 4 | 3 | Strong headless external opt-in; smoke early once installed. |
| 6 | `qwen` / Qwen Code | BLOCKED: binary missing | 3 | 4 | 3 | 4 | 2 | 4 | 3 | Stable opt-in when OpenAI-compatible provider config is generated or documented. |
| 7 | `cline` | BLOCKED: binary missing | 3 | 4 | 4 | 4 | 2 | 4 | 3 | Stable opt-in, guarded by CLI flag/version smoke. |
| 8 | `aider` | BLOCKED: binary missing | 4 | 3 | 4 | 4 | 2 | 2 | 2 | Niche opt-in for edit-heavy tasks; not a general WG worker default. |
| 9 | `gemini` CLI | BLOCKED: binary missing; no OpenRouter route in arena | 3 | 4 | 1 | 2 | 2 | 3 | 3 | Provider-specific opt-in, not relevant to OpenRouter defaults. |
| 10 | `crush` | BLOCKED: binary missing | 2 | 3 | 3 | 4 | 2 | 2 | 2 | Experimental until `crush run --help` and output behavior are pinned. |
| 11 | `amplifier` | BLOCKED: binary missing | 2 | 3 | 3 | 3 | 1 | 3 | 2 | Experimental ecosystem opt-in; not a default worker path. |
| 12 | Aether-style runtime | Not implemented in WG arena | 2 | 4 | 4 | 4 | 2 | 3 | 2 | Research opt-in only until WG has an adapter and smoke coverage. |

## Criterion Comparison

### Reliability

`native` / Nex is the strongest technical WG fit after the credential fix, but
its original live arena result was a failure. That history matters: the code is
now fixed and locally smoked, but downstream integration should still run one
cheap live OpenRouter task on current `main` before making native OpenRouter a
release default.

`claude` is still the highest-confidence route for unattended WG completion
because WG's prompts, stream translation, and historical smoke gates are built
around Claude Code behavior. Its weakness in this arena is not quality; it is
that the smoke intentionally avoided live Claude spend and the adapter has no
cheap OpenRouter route.

`codex` has the strongest live evidence among external CLIs. It was installed
as `codex-cli 0.135.0`, ran non-interactively through `codex exec --json
--ephemeral`, reached OpenRouter, and returned the expected message. Its risk is
behavioral rather than transport-level: WG must keep the injected task contract
explicit so Codex logs, writes artifacts, commits, pushes, checks messages, and
calls `wg done`.

External CLIs that were missing on this host cannot outrank live-passed or
bundled routes. Their reliability is provisional until binary discovery, prompt
delivery, model flags, and WG graph mutations are smoke-tested end to end.

### Non-Interactive Fit

Best fits:

- `native` / Nex: built for WG automation, with `wg nex --eval-mode`,
  `nex --wg --eval-mode`, and native tool/event handling.
- `codex`: `codex exec --json --ephemeral` is a clean one-shot worker shape.
- `goose`: `goose run --no-session -i prompt.txt` is the best external CLI
  command form on paper.
- `opencode`: `opencode run --format json` is a good adapter shape once WG
  prompt-file delivery is verified.

Weaker fits:

- `aider`: mature, but optimized for pair-programming/edit loops and repo-map
  behavior rather than WG's task-agent lifecycle.
- `crush`: promising terminal-agent ergonomics, but less structured as an
  unattended worker.
- `amplifier`: useful for bundle ecosystems, not for a simple WG worker
  subprocess.

### OpenRouter Support

Direct OpenRouter is verified by the arena: `chat/completions` with
`deepseek/deepseek-v4-flash` returned `ok` with 37 total tokens and reported
cost around `$0.0000089199`.

Current executor support:

- Strongest WG-owned route: `native` / Nex, after `fix-nex-openrouter`, because
  WG owns endpoint resolution and can route `openrouter:*` model specs directly.
- Strongest live external route: `codex`, via OpenAI-compatible provider
  overrides using the OpenRouter base URL and `OPENAI_API_KEY` sourced from the
  OpenRouter key.
- Not OpenRouter-oriented: `claude`, whose WG handler is Claude CLI /
  Anthropic-auth based.
- Template support but unvalidated locally: `opencode`, `aider`, `goose`,
  `qwen`, and `cline`.
- Experimental or provider-specific: `crush`, `amplifier`, Gemini, and
  Aether-style adapters.

### Cost Control

Native/Nex should be the preferred cost-control surface because WG can choose
cheap models, route by task or tier, resolve endpoint credentials, and account
for usage in one Rust path. That is the core reason to promote native OpenRouter
after final live smoke.

Codex is the current proven external cost-control route. It can use cheap
OpenRouter models with ephemeral CLI overrides, which is valuable for smoke
testing and users who already trust Codex.

Claude CLI is reliable but weaker for cost control in this arena. It supports
Claude tiering, but not OpenRouter's cheap model market through WG's current
Claude adapter.

External CLIs can control cost when they accept OpenRouter model flags and
inherit the resolved key, but each one adds a separate provider/auth surface.
That makes them good per-agent opt-ins and poor global defaults.

### Install Friction

Lowest friction:

- `native` / Nex ships with WG and Nex, so no separate agent binary is needed.
- `claude` and `codex` were installed on the smoke host, but still require each
  CLI's auth or provider setup.

Higher friction:

- `opencode`, `aider`, `goose`, `qwen`, `cline`, `gemini`, `crush`, and
  `amplifier` were missing. WG can generate command templates, but users still
  need binary install and tool-specific auth.
- Aether-style integration is not an install-only problem; WG needs an adapter,
  event contract, and completion semantics first.

### Output Parseability

Strong:

- `native` / Nex emits WG-native stream events and is the cleanest parse target.
- `claude` stream-json is already translated into WG's stream model.
- `codex` emits JSONL through `exec --json`, and WG can parse Codex usage
  events.
- `goose`, `opencode`, `qwen`, and `cline` have JSON or JSONL command shapes in
  the adapter research.

Weak:

- `aider` is text-log and edit-flow oriented.
- `crush` has simpler stdout than a TUI, but not a WG-grade structured stream
  by default.
- `amplifier` can be bookended by WG, but its native output is not WG's stream
  protocol.
- Aether-style runtimes may expose headless or SDK events, but WG has not
  normalized them yet.

### WG Protocol Adherence

Best:

- `native` / Nex owns WG's tools, graph lifecycle, secret handling, and task
  completion semantics directly.
- `claude` is battle-tested with WG's agent guide and command discipline.
- `codex` is viable with explicit prompt injection and JSONL parsing, but its
  default behavior is still external to WG.

External workers must be judged by graph effects, not natural-language final
answers. A successful WG lifecycle smoke should prove the worker reads
messages, logs progress, writes an artifact, commits, pushes, checks messages
again, and calls `wg done` without leaking secrets.

## Stable Defaults

Recommended default policy:

1. Keep fresh installs on `claude:opus` until native OpenRouter has one final
   live smoke on current `main`. This avoids changing the safest general worker
   default during the executor-arena rollout.
2. Make `openrouter:*` model specs route through native/Nex as the stable
   cost-controlled default once final integration confirms live external
   OpenRouter generation after `fix-nex-openrouter`.
3. Keep the Codex starter profile as a stable first-class alternate:
   `wg profile use codex` or `wg profile use codex:gpt-5.5`.
4. Keep Codex-over-OpenRouter as the standard cheap external smoke route, using
   ephemeral CLI config overrides rather than rewriting user Codex auth.
5. Keep external worker CLIs out of global defaults. Promote each one only after
   binary discovery, model normalization, prompt-file delivery, output parsing,
   and a full WG lifecycle smoke pass.

## Experimental Opt-Ins

Stable-template opt-ins after install and smoke:

- `opencode`: best external CLI adapter shape on paper; promote after a live
  task-agent run proves prompt-file delivery, model normalization, and graph
  completion.
- `goose`: strongest classic headless one-shot candidate; useful for users
  already invested in Goose providers/extensions.
- `qwen`: good for OpenAI-compatible provider users, but setup should generate
  or document the provider entry explicitly.
- `cline`: useful when CLI auth/provider setup is already present; guard
  against flag drift.
- `aider`: keep for edit-heavy tasks where Aider's workflow is desired.

Experimental-only:

- `gemini`: provider-specific and missing locally; not useful for OpenRouter
  default decisions.
- `crush`: verify the installed CLI's `run --help`, model syntax, and output
  mode before unattended runs.
- `amplifier`: use when the bundle ecosystem is the point; otherwise WG's
  native loop is simpler.
- Aether-style: promising as a Rust/headless/ACP ecosystem, but no WG adapter
  or smoke exists yet.

## Rust-Native Counterpoint

The strategic question is not "which subprocess can WG launch?" It is "which
runtime should own WG's task lifecycle, secrets, tools, cost accounting, and
stream semantics?"

### Nex / Native

Nex/native is the preferred long-term answer because it is part of WG. It can
avoid duplicated prompt rendering, keep API keys out of process args, emit
native stream events, use WG tools directly, and support OpenAI-compatible
providers without depending on another agent CLI's conventions. The original
smoke failure did not weaken that architecture; it identified a credential path
that has now been fixed and pinned by smoke coverage.

Defaulting implication: native/Nex should own `openrouter:*` once final
integration reruns a live cheap model on current `main`.

### Codex

Codex is the strongest external counterpoint. It combines a capable agent model
with a clean `exec --json` batch mode and, in the arena smoke, working
OpenRouter compatibility through OpenAI-compatible config overrides. The trade
off is control: WG is wrapping an external agent with its own policies, logs,
auth model, and behavior defaults. That is acceptable as a stable alternate,
but Codex should not become the source of truth for WG protocol or endpoint
semantics.

Defaulting implication: keep Codex as a stable profile and smoke route, not as
the global WG default.

### Goose

Goose is the strongest classic external-worker counterpoint. Its command shape
maps well to WG: no-session runs, prompt-file input, provider/model flags, and
JSON-capable output. The missing piece is live evidence in this repo. Once
installed, Goose should be one of the first external CLIs to receive a full WG
lifecycle smoke.

Defaulting implication: stable opt-in after install and smoke, not a default.

### Aether-Style Runtimes

Aether-style runtimes are not just CLIs; they are embeddable agent runtimes with
headless, SDK, ACP/MCP, and provider:model concepts. That makes them relevant
if WG eventually wants an embedded agent substrate rather than shell-wrapped
subprocesses. They should stay experimental until WG answers three questions:

1. Does the headless mode produce stable machine-readable events and exit
   codes?
2. Can WG inject its full task prompt, secrets, working directory, and model
   without tool-specific persistent config?
3. Can completion be verified by WG graph mutations rather than final text?

Defaulting implication: research opt-in only.

## Follow-Up Gates

Before changing release defaults:

1. Run final live OpenRouter smoke on current `main` after `c4bfcbeb`:
   direct OpenRouter, `wg endpoints test openrouter`, `wg nex`, standalone
   `nex --wg`, and `nex --eval-mode --nex-dir <scratch>`.
2. Run a full WG lifecycle smoke through the proposed default: read messages,
   log progress, write an artifact, commit, push, read messages again, and call
   `wg done`.
3. Confirm no API key appears in argv, output logs, artifacts, or committed
   files.
4. Confirm token/cost extraction works for the selected default, or label the
   default "no cost telemetry."
5. Test external worker adapters with binary versions discovered on PATH, not
   only command-template unit tests.

## Sources

- [executor-arena-smoke.md](executor-arena-smoke.md) - live smoke evidence for
  OpenRouter, Nex/native pre-fix behavior, Codex, Claude, and missing CLIs.
- [executor-arena-research.md](executor-arena-research.md) - adapter command
  shapes, prompt delivery, model normalization, and prior Rust-native analysis.
- `wg show fix-nex-openrouter` - post-smoke env-key fix, commit `c4bfcbeb`, and
  smoke validation logs.
- [../guides/executor-arena.md](../guides/executor-arena.md) - current stable
  and experimental adapter documentation.
- [../guides/openrouter-setup.md](../guides/openrouter-setup.md) - endpoint,
  key, and external executor OpenRouter setup.
- [../design-pan-executor.md](../design-pan-executor.md) - native executor
  strategy, stream model, and target state.
- [amplifier-research-report.md](amplifier-research-report.md) - counterpoint
  on Amplifier-style framework integration versus WG-owned Rust execution.
