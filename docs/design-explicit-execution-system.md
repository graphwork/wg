# Design: explicit execution-system selection and same-system fallback

**Task:** `design-explicit-execution`

**Date:** 2026-07-13

**Status:** Accepted contract for implementation

**Related:** [Handler-first model specs](design-handler-first-model-spec.md), [canonical config UX](config-ux-design.md)

## 1. Decision

A fresh WG is **graph-only**. It has no active LLM execution system.

WG **MUST NOT** infer an execution system from a compiled default, an installed
binary, an environment variable, an API key, setup auto-detection, model
registry contents, or the mere existence of a starter profile. Before any
LLM-backed dispatch, the user or automation **MUST explicitly select** a
handler-first route.

WG **MUST NOT** recover from an execution failure by switching to Claude, Pi,
Codex, nex, another handler, or another provider unless the user explicitly
configured that exact fallback and it belongs to the same execution system as
the primary route. Pi is not the new implicit default. Claude is not the safety
net.

This changes the earlier “fresh installs run `claude:opus`” policy in
[config-ux-design.md](config-ux-design.md). Built-in model entries remain useful
as catalog metadata and setup suggestions, but they are inactive until selected.

## 2. Vocabulary and invariants

### 2.1 Route and execution-system key

A **route** is a canonical handler-first model spec plus any selected endpoint:

```text
<handler>:<handler-native-model> [@ endpoint-name]
```

Examples are `claude:opus`, `codex:gpt-5.5`,
`pi:openai-codex:gpt-5.6-sol`, and
`nex:openrouter:z-ai/glm-5.2`.

Each handler adapter **MUST** derive an **execution-system key**:

```text
ExecutionSystemKey = (handler, provider-or-wire)
```

Examples:

| Route | Execution-system key |
|---|---|
| `claude:opus` | `(claude, anthropic-cli)` |
| `codex:gpt-5.5` | `(codex, openai-codex-cli)` |
| `pi:openai-codex:gpt-5.6-sol` | `(pi, openai-codex)` |
| `pi:openrouter:z-ai/glm-5.2` | `(pi, openrouter)` |
| `nex:openrouter:z-ai/glm-5.2` | `(nex, openrouter)` |
| `nex:qwen3-coder` | `(nex, oai-compat)` |

Handlers with opaque native model syntax **MUST** implement this derivation in
their adapter. If WG cannot determine the provider/wire, it **MUST NOT** use a
fallback for that route; it may only retry the exact route.

A different handler is a different system even when it reaches the same model.
A different provider/wire is a different system even when the handler is the
same. Therefore `pi:openrouter:X` cannot fall back to `nex:openrouter:X`, and
`pi:openrouter:X` cannot fall back to `pi:openai-codex:Y`.

### 2.2 Selection, readiness, and fallback are separate

WG **MUST** track three independent facts:

1. **Selection:** did a user explicitly choose a handler-first route?
2. **Readiness:** is the chosen handler/binary/endpoint/credential usable now?
3. **Fallback policy:** which ordered, same-system alternatives did the user
   explicitly authorize?

A route can be selected but not ready. A detected credential can be ready but
not selected. Neither condition implies the other.

### 2.3 Core invariants

1. `Config::default()` **MUST represent `Unselected`** for dispatch purposes.
2. A value whose provenance is `Default` **MUST NOT satisfy selection**.
3. Environment variables, CLI installation, launcher history, registry entries,
   and endpoint discovery **MUST NOT satisfy selection**.
4. Every LLM entry point **MUST call one shared selection/readiness preflight**.
5. A route failure **MUST NOT change the execution-system key implicitly**.
6. An unavailable route **MUST produce a structured error and retryable state**;
   WG **MUST NOT invent an agency verdict or mark implementation work semantically
   failed merely because execution was unavailable**.
7. Shell execution and graph operations are outside this contract and remain
   credential-free.

## 3. What counts as explicit selection

### 3.1 Persistent selection

Any of the following successful actions counts:

- interactive `wg setup`, after the user chooses and confirms a route;
- `wg setup --route <route> --yes` (with required route parameters);
- `wg profile use <name>` or `wg profile use <name>:<model>`, when the activated
  profile resolves to a valid handler-first model;
- `wg config --global --model <handler>:<native-model>`;
- `wg config --local --model <handler>:<native-model>`;
- equivalent per-role handler-first configuration, but only for that role.

A manually edited global/local config containing a valid handler-first model is
also explicit. The source is the on-disk field, not its value. Thus an explicit
`agent.model = "claude:opus"` remains explicit even though `claude:opus` was once
also a built-in default.

`wg config init` **MUST require `--route` or `--model` to select execution**.
`wg config init --local --bare` remains graph-only and **MUST NOT silently choose
`claude-cli`**.

### 3.2 Invocation-scoped selection

An explicit handler-first `--model` on a manual LLM command counts for that
invocation only. A handler-first `task.model` counts for that task's worker
spawn only. Neither authorizes evaluator, reviewer, FLIP, assigner, chat, or
future service dispatch unless their own resolved route is explicit.

`wg service start --model <handler>:<native-model>` is an explicit selection for
that daemon invocation. It does not rewrite config unless the user runs a config
command.

### 3.3 What does not count

The following **MUST NOT** count:

- `Config::default().agent.model`, `effective_tiers()` defaults, or the built-in
  Claude registry;
- `wg init` without route/model flags;
- `wg profile init-starters`, a shipped profile template, or an inactive profile;
- `claude`, `codex`, `pi`, or another executable being on `PATH`;
- `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, or other ambient credentials;
- setup auto-detection, launcher history, an endpoint marked `is_default`, or a
  cached ranked-model list;
- legacy `[agent].executor` / `[dispatcher].executor` without a model;
- a bare provider prefix or bare model alias in new configuration.

## 4. Configuration and provenance contract

### 4.1 Computed selection record

Implementation **SHOULD** add a single resolver returning:

```rust
struct ExecutionSelection {
    state: SelectionState, // Unselected | Selected
    route: Option<CanonicalRoute>,
    system: Option<ExecutionSystemKey>,
    source: Option<ExecutionSelectionSource>,
}

enum ExecutionSelectionSource {
    Cli { flag: String },
    Task { task_id: String, field: String },
    Profile { name: String, path: PathBuf },
    Config { scope: GlobalOrLocal, path: PathBuf, key: String },
    LegacyExplicit { path: PathBuf, key: String },
}
```

The resolver **MUST inspect raw/source-annotated config**, not the post-serde
`Config` alone. `Config` currently fills `agent.model = "claude:opus"`, so value
comparison cannot distinguish an explicit file field from a built-in value.

The existing `ConfigSource::{Global, Local, Default}` and
`Config::load_with_sources` are the starting point. Execution provenance
**MUST** retain the winning file and dotted key, and add profile/CLI/task
sources. `wg config --list`, `wg config --models`, `wg status --json`, spawn
provenance, and error JSON **MUST expose** the selection source.

A separate persistent boolean is not sufficient: it can drift from the route.
If an implementation stores a setup receipt such as `[execution.selection]`, it
**MUST** be corroborating provenance, not the sole authority. The selected route
must still resolve from an explicit field/profile.

### 4.2 Fallback schema

Fallback is opt-in and ordered. Use a dedicated shape rather than treating tiers
or model rankings as failure policy:

```toml
[[execution.fallbacks]]
primary = "pi:openai-codex:gpt-5.6-terra"
models = ["pi:openai-codex:gpt-5.6-sol"]
```

An optional endpoint may be part of each candidate using the existing named
endpoint reference. Each declaration **MUST** match an exact canonical primary
route. Every candidate **MUST** have the same `ExecutionSystemKey` as `primary`.
A cross-system candidate is a fatal `config lint`/service-preflight error, not a
candidate WG silently skips.

`tiers.*`, `max_escalation_depth`, `.wg/profile_ranked_tiers.json`, registry
rankings, `is_default` endpoints, and OpenRouter's old `fallback_model` **MUST NOT
be interpreted as execution-failure fallbacks**. They may select quality for a
new call, but they do not authorize a route switch after failure.

## 5. Graph-only versus LLM-backed commands

`wg init` without execution flags **MUST** create the graph, graph metadata,
`.gitignore`, and templates only. It **MUST NOT** write a model route, activate a
profile, or invoke setup. `wg init --route ...` and `wg init --model ...` remain
convenience forms that both initialize the graph and explicitly select a route.

### 5.1 Command-state matrix

| Command or operation | No selection | Selection, not ready | Selected and ready |
|---|---|---|---|
| `wg init` (no route/model) | **succeeds graph-only** | succeeds; does not alter selection | succeeds; does not alter selection |
| `wg list/show/status/viz/context/trace` | succeeds | succeeds and reports readiness error | succeeds |
| `wg add/edit/rm-dep/add-dep/log/artifact/pause/resume/retry` | succeeds | succeeds | succeeds |
| `wg agency init`, role/tradeoff/agent CRUD | succeeds | succeeds | succeeds |
| federation/identity/message storage operations without model review | succeeds | succeeds | succeeds |
| `wg exec --shell`, shell tasks | succeeds | succeeds | succeeds |
| `wg done` with auto-evaluate enabled | marks work done; evaluation satellite becomes waiting for execution | same | dispatches evaluation normally |
| deterministic review Pass 0/1 or credential-free test stub | succeeds | succeeds | succeeds |
| live reviewer/assigner/evaluator/FLIP/triage/placer/creator/evolver call | errors before model call; preserves pending state | readiness error; preserves state | dispatches |
| manual `wg spawn`, `spawn-task`, non-shell worker | `WG-EXEC-UNSELECTED` before claim/worktree | readiness error before spawn | dispatches |
| chat creation or first LLM-backed chat turn | `WG-EXEC-UNSELECTED`; message not falsely acknowledged | readiness error | dispatches |
| `wg service start` | **fails before fork/state/socket** | **fails before fork/state/socket** | starts |

`wg service start` is conservatively LLM-backed because it can dispatch workers,
chat, and agency calls. `--no-coordinator-agent` does not make the dispatcher
graph-only. A future explicit `--shell-only` daemon mode may bypass this gate,
but it must refuse non-shell tasks rather than later selecting a model.

A graph mutation that merely schedules an LLM follow-up **MUST NOT be rolled
back**. The follow-up is recorded as waiting; no LLM dispatch occurs.

## 6. Shared preflight and error surface

### 6.1 Order

Every LLM entry point **MUST** run these steps before irreversible dispatch
state (claim, worktree, child process, verdict write, or daemon fork):

1. resolve explicit selection and provenance;
2. canonicalize the handler-first route and execution-system key;
3. validate fallback declarations;
4. check handler executable/plugin availability;
5. resolve the exact named endpoint for the selected provider only;
6. resolve credentials for that endpoint/provider only;
7. use a non-billing auth/status probe when the handler exposes one;
8. return `Ready`, `Unverified`, or a structured failure.

`Unverified` is permitted only when a self-authenticating CLI has no reliable
non-billing status command. A subsequent CLI auth failure is still a route
failure and follows Section 7; it never triggers another system.

Credential lookup **MUST NOT** use a default endpoint belonging to another
provider. Missing credentials for OpenRouter cannot borrow an OpenAI or
Anthropic endpoint key. Secret values must never appear in diagnostics.

### 6.2 Unselected error

Human-readable commands **MUST** include this actionable block (the prefix may
name the attempted operation):

```text
error[WG-EXEC-UNSELECTED]: no LLM execution system has been selected.
This WG is available for graph-only use, but this command requires an LLM route.

Choose one explicitly:
  wg setup                                      # interactive
  wg setup --route claude-cli --yes
  wg setup --route codex-cli --yes
  wg setup --route pi --yes
  wg setup --route openrouter --yes
  wg setup --route local --url http://localhost:11434/v1 --model llama3 --yes
  wg setup --route nex-custom --url <URL> --model <MODEL> --yes
  wg profile use <name>
  wg config --global --model <handler>:<native-model>
  wg config --local  --model <handler>:<native-model>

`wg init`, graph reads, and graph edits do not require a model or credentials.
```

The error **MUST NOT** put one route first as “the default,” and **MUST NOT** say
“falling back to Claude/Pi.” JSON mode must return at least:

```json
{
  "code": "WG-EXEC-UNSELECTED",
  "operation": "service-start",
  "selection": "unselected",
  "setup_commands": ["wg setup", "wg setup --route claude-cli --yes", "..."]
}
```

### 6.3 Readiness errors

Use stable codes: `WG-EXEC-HANDLER-MISSING`, `WG-EXEC-CREDENTIAL-MISSING`,
`WG-EXEC-CREDENTIAL-INVALID`, `WG-EXEC-ENDPOINT-UNREACHABLE`, and
`WG-EXEC-ROUTE-FAILED`. Each error **MUST** include the selected route, its
source, attempted same-system fallbacks, and an exact repair/check command (for
example `wg login openrouter`, `wg login openrouter --check`, `pi /login`, or the
handler's install command). It **MUST NOT** recommend a different system as an
automatic repair.

## 7. Deterministic fallback algorithm

For primary canonical route `P`:

1. Compute `K = system_key(P)`. If selection or key derivation fails, stop.
2. Preflight and attempt `P`. Exact-route retries for explicitly classified
   transient errors are allowed under the existing bounded retry policy.
3. On failure, load only the `[[execution.fallbacks]]` declaration whose
   canonical `primary == P`.
4. Preserve listed order. Reject the whole declaration if any candidate `C`
   has `system_key(C) != K`.
5. Skip candidates already attempted. Preflight `C`, then attempt it. Record
   route, source, reason, and outcome in spawn/agency provenance.
6. Stop on first success.
7. If the list is absent/exhausted, return `WG-EXEC-ROUTE-FAILED`. Do not inspect
   tiers, profile rankings, installed CLIs, ambient keys, or built-in models.

Pseudocode:

```text
attempts = [P] + explicitly_configured_fallbacks_for(P)
assert every system_key(attempt) == system_key(P)
for route in attempts:
    bounded_retry_exact_route(route)
    if success: return result
return retryable_execution_failure(attempts)
```

### 7.1 Retryable state by operation

- **Worker task:** preflight failure leaves it `Open`/unclaimed. A failure after
  claim returns it to a retryable waiting/open state with `last_execution_error`;
  it does not consume semantic failure/rescue budget.
- **Agency satellite (`.assign-*`, `.evaluate-*`, `.flip-*`):** remains
  `Open`/`Waiting` for the selected execution system. No empty or synthetic
  verdict is written.
- **Evaluation target:** retains its pre-evaluation state (including
  `FailedPendingEval` where applicable); absence of an evaluator route is not a
  FAIL verdict.
- **Content review:** live-model transport failure is fail-closed as
  `quarantine/pending-review`, never `accept`. Deterministic passes may still
  reject independently.
- **Chat:** a new synchronous message fails before enqueue, or an already queued
  daemon message remains queued with `last_execution_error`. WG must not append
  a fake assistant response.
- **Service:** route-wide readiness failure pauses dispatch for that system and
  surfaces the error; it must not churn tasks through spawn-failure counters.

This contract does not redesign `FailedPendingEval` respawn behavior and does
not address Node `EPIPE` process failures. Those bugs may produce a route
failure, but execution selection/fallback **MUST NOT** be coupled to their state
machines.

## 8. Setup and profile UX

### 8.1 Interactive setup

Detection may annotate choices (“Claude CLI installed”, “OpenRouter key
found”), but **MUST NOT choose a route**. On a fresh installation the picker
must default to `Not now — keep this WG graph-only`, or require an explicit
confirmation that names the detected route. Pressing Enter on an automatically
selected Claude route is not acceptable.

For an already selected installation, setup may preselect the current route for
editing because provenance proves it was explicit. The final confirmation must
show handler, provider/wire, model, endpoint, credential status, scope, and
source to be written.

Setup writes route config only after confirmation. If credential validation is
required and fails, setup must not claim “ready.” `--skip-validation` is an
explicit waiver: it may persist `Selected / Unverified` and must print the exact
check command.

### 8.2 Non-interactive and CI

`wg setup --yes` without `--route` **MUST fail**. CI uses an explicit route, for
example:

```sh
wg init
wg setup --route codex-cli --scope local --yes
# or
wg config --local --model pi:openai-codex:gpt-5.6-sol
```

Routes needing URL/model inputs must continue to require them. Secret refs or
environment names may be supplied, but secret presence alone does not select a
route. `--dry-run` never records selection.

### 8.3 Starter profiles

Starter profiles remain shipped suggestions. `wg profile init-starters` **MUST
NOT activate one**. `wg profile use <name>` is explicit and must record the
profile name/path and resolved handler-first route. Unknown, empty, or invalid
profiles fail without falling through to built-in tiers.

Dynamic ranked lists are advisory quality data, not fallback authorization.
Profile authors may include explicit same-system fallback declarations; profile
activation validates them exactly like normal config.

### 8.4 Evidence-gated Pi tier example

The policy is generic; model names are not hard-coded by this design. If the
separate Terra probe establishes that these Pi routes are real and suitable, a
profile may explicitly choose:

```toml
[tiers]
fast = "pi:openai-codex:gpt-5.6-luna"       # cheap, bounded work
standard = "pi:openai-codex:gpt-5.6-terra" # evaluator/reviewer/FLIP
premium = "pi:openai-codex:gpt-5.6-sol"    # strong work/escalation

[[execution.fallbacks]]
primary = "pi:openai-codex:gpt-5.6-terra"
models = ["pi:openai-codex:gpt-5.6-sol"]
```

This example is **non-normative until probe evidence lands**. The fallback is
allowed only because both routes have key `(pi, openai-codex)` and the profile
lists it explicitly. The tier mapping alone would not authorize fallback.

## 9. Migration and compatibility

### 9.1 Fresh and implicit-default installations

A missing global/local routing field and no active profile migrates to
`Unselected`. The first LLM command fails with Section 6.2. WG **MUST NOT create
or activate a Claude or Pi profile during migration**.

`wg migrate config --dry-run` and `wg config lint` must explain:

```text
execution-selection: missing
This installation previously relied on WG's implicit claude:opus default.
Choose explicitly, for example:
  wg setup --route claude-cli --yes
  wg profile use claude
  wg config --global --model claude:opus
No change was made automatically.
```

There is no compatibility grace that silently dispatches Claude: the user
decision requires explicit selection before the next LLM call.

### 9.2 Existing explicit configurations

Existing files with a valid handler-first model from `Global` or `Local` source
are grandfathered as `LegacyExplicit` and continue to work. This preserves
explicit Claude, Codex, Pi, nex, OpenCode, and other installations even when the
chosen value equals an old built-in default. `wg migrate config` may add richer
provenance without changing the route.

An active named profile is preserved and counts as explicit after its model and
fallback policy validate. Profile files are not rewritten merely to change the
selected provider.

Legacy bare aliases/provider prefixes are handled by the existing handler-first
migration first. A model field the user actually stored may be canonicalized
(`opus` to `claude:opus`, deprecated provider prefix to its `nex:` form) and then
counts as `LegacyExplicit`. An executor-only config is ambiguous and remains
unselected; lint names the exact setup/config command.

Old full snapshots generated by WG may contain explicit-looking Claude fields.
Because intent cannot be reconstructed perfectly, an on-disk model field is
accepted for compatibility, marked `LegacyExplicit`, and warned until migration
records provenance. Crucially, an absent field can never be filled from a
built-in and accepted.

### 9.3 Config lint

`wg config lint` **MUST** report:

- selection state, canonical route, execution-system key, and exact source;
- fields whose source is `Default` as `inactive suggestion`, not selected;
- selected-but-missing binary/plugin/endpoint/credential;
- invalid or cross-system fallback candidates as errors;
- legacy implicit-default and executor-only configs;
- conflicting global/local/profile routing sources and the winner;
- default endpoints whose provider does not match the selected route;
- deprecated bare model/provider forms and migration commands.

Lint is read-only and credential values remain redacted.

## 10. Current-code audit and implementation actions

Line numbers describe the 2026-07-13 tree; function names are authoritative if
lines move.

### 10.1 `src/service/llm.rs`: all Claude default/fallback paths

| Current site | Current behavior | Required action |
|---|---|---|
| `AGENCY_CLAUDE_HAIKU_SPEC` (~40) | global agency rescue constant | delete; no replacement default constant |
| `agency_dispatch_for_spec` (~136) | reroutes any Claude-family model, including an OpenRouter-qualified model, to Claude CLI | honor the leading handler/provider; model-family recognition may normalize only inside an explicitly selected Claude route |
| `native_provider_for_spec` (~172) | prefixless spec defaults to Anthropic | require a selected/canonical provider; return error when unknown |
| `resolve_agency_dispatch` (~249-268) | missing weak tier and missing native credential both become `claude:haiku` | return `Result<AgencyDispatch>`; require explicit role/selected weak route; credential failure is preflight error |
| `run_review_llm_call` strong route (~307-310) | missing strong tier becomes Opus | require explicit strong route or return unselected |
| `run_review_llm_call` native/Pi failures (~326-358) | fall back to Haiku on Claude CLI | run Section 7 same-system list, else quarantine/pending-review error |
| `run_review_llm_call` catch-all (~363-364) | unsupported handler becomes Haiku | return unsupported-handler error |
| `agency_native_lightweight_call` doc/caller (~452, ~548-565) | native error triggers Haiku | return original structured failure to fallback engine |
| `run_lightweight_llm_call` Pi arm (~568-593) | Pi error triggers Haiku | same-system fallback only; no handler switch |
| external/unsupported agency handler catch-all (~597-611) | degrades to Haiku | hard error naming unsupported one-shot handler |
| non-agency native calls (~627-670) | native failure falls through to `call_claude_cli(model, ...)` | return native failure or same-system fallback; call Claude only when selection key is Claude |
| `pi_env_pairs_for` (~965-985) | matching endpoint may fall back to any default endpoint | remove cross-provider endpoint fallback; use provider-matching/named endpoint only |
| tests from ~1481 onward | assert default Haiku and missing-key/Pi-to-Haiku behavior | replace with unselected, source, same-system success, and loud-failure tests |

The direct `call_claude_cli` implementation is not itself a problem. It remains
valid when the explicitly selected route uses the Claude handler.

### 10.2 `src/config.rs`: default-model and provenance sites

| Current site | Required action |
|---|---|
| `Tier::default_alias` (~1638) | catalog/display suggestion only; never dispatch authority |
| `Config::builtin_registry` (~2864) | keep catalog entries, label them built-in/inactive |
| `effective_tiers` (~3028) | remove hard-coded Anthropic fill for dispatch; unresolved tiers stay `None` unless profile/config is explicit |
| `weak_tier_spec` / `strong_tier_spec` (~3108) | permit `None`; callers must handle unselected |
| `resolve_model_for_role` (~3179) | return selection provenance/result; its final `agent.model` branch cannot use a `Default`-source value for dispatch |
| `CoordinatorConfig::effective_executor` (~4206) | remove final `"claude"`; return optional/error derived from selected route |
| `default_executor` / `default_model` (~4340) | no active Claude selection; retain only compatibility storage if the central gate makes it inactive |
| `Config::load_or_default` (~5090) | corrupt config may fall back for graph reads, but LLM preflight must fail; never dispatch defaults after parse error |
| `ConfigSource` / `load_with_sources` (~4470, ~5495) | add execution source detail for file/key/profile/CLI/task and retain `Default` distinction |
| `effective_dispatcher_executor` (~5617) | return optional/error; do not derive from inactive default `agent.model` |
| `validate_config` (~5737) | add selection/fallback/readiness diagnostics; default config remains valid for graph use but invalid for LLM preflight |
| `resolve_api_key_for_provider` (~4900) | remove cross-provider `find_default()` credential fallback |
| default-assuming tests (`test_default_config`, effective tiers/executor tests around ~8193 and ~9127) | assert graph-valid `Unselected`, not Claude dispatch |

Changing `AgentConfig.model` to `Option<String>` is optional if too invasive in
the first implementation. Keeping the compatibility string is acceptable only
when all LLM resolution goes through source-aware preflight and displays the
built-in value as inactive.

### 10.3 `src/commands/setup.rs`: auto-detection/default sites

| Current site | Current behavior | Required action |
|---|---|---|
| `run_non_interactive` provider (~749) | omitted provider becomes Anthropic | require explicit `--route`/legacy `--provider`; `--yes` alone errors |
| `run_non_interactive` model (~794) | provider-specific default model chosen silently | defaults are allowed only after the explicit provider/route flag; record selection source |
| `resolve_key_from_args` (~1030) | omitted provider becomes Anthropic | pass selected provider explicitly; never infer it |
| `run_with_args` (~1263) | non-TTY legacy flow can reach provider defaults | non-TTY requires `--route` or explicit handler-first model |
| `run_route` (~1283) | route-driven path | keep; record selection only after successful write/validation policy |
| interactive `current_route` (~1733-1743) | existing config, detected key/CLI, then unconditional Claude default | detection only annotates; fresh default is graph-only/not-now or explicit confirmation |
| interactive executor override (~1768-1828) | permits handler/provider mismatch | derive handler from chosen handler-first route; custom route must be explicit and lintable |
| OpenRouter manual model (~2219) | missing current model defaults to Claude Opus via OpenRouter | route was explicitly selected, so a picker suggestion is acceptable; label it suggestion and require confirmation |
| `configure_anthropic` (~2545) | uses default-filled `existing.agent.model` as current | use it only when provenance is explicit; otherwise show unselected picker |
| setup tests around ~3091 onward | assume Claude default configuration | distinguish explicit Claude route tests from fresh unselected tests |

### 10.4 Other implementation files that must use the same gate

- `src/commands/init.rs`: no-arg `wg init` currently calls
  `default_route_from_global_or_claude`; remove the Claude fallback and make
  no-route initialization graph-only. An explicitly selected global route may
  remain inherited only if provenance says it was explicit.
- `src/config_defaults.rs`: route templates remain valid **after** explicit
  route selection; they are not `Config::default` authority.
- `src/commands/config_cmd.rs`: `config init` must not default to Claude; show
  selection/source/readiness and lint fallback declarations.
- `src/profile/named.rs` and `src/profile/template.rs`: mere starter existence is
  inert. `escalate_model` must not turn dynamic OpenRouter rankings into failure
  fallback candidates.
- `src/commands/service/mod.rs`: run shared preflight in `run_start` before fork,
  state file, or socket; repeat in `run_daemon` defensively.
- `src/commands/spawn/execution.rs`, `dispatch/plan.rs`: preflight before claim /
  worktree / wrapper; record route and fallback provenance.
- `src/chat_command.rs`, `commands/service/ipc.rs`,
  `commands/service/coordinator_agent.rs`: remove `unwrap_or("claude")` defaults.
- `src/commands/agency_init.rs`: stop pinning evaluator/assigner to
  `claude:haiku`; leave roles unresolved or copy only an explicitly selected
  weak route.
- `src/commands/evolve/fanout.rs` and `partition.rs`: remove formatted
  `claude:<tier>` fallback.
- evaluator, reviewer, FLIP, assignment, triage, placement, creator, and evolver
  call sites: consume structured preflight errors and preserve Section 7.1
  state.
- `src/commands/exec_fed_cmd.rs`: remove task model
  `unwrap_or("claude:opus")`; remote execution requires an explicit grant model.
- `src/commands/pi_handler.rs` and native provider key resolution: default
  endpoint lookup must not cross providers.
- docs/quickstart/AGENTS/CLAUDE: replace “fresh install already runs Claude”
  language with graph-only plus explicit setup examples.

## 11. Implementation sequence

1. Add source-aware `ExecutionSelection`, canonical `ExecutionSystemKey`, stable
   errors, and unit tests.
2. Make no-arg init/config-init graph-only; adjust default displays.
3. Add selection and credential preflight to service start, manual spawn, and
   chat before mutation.
4. Convert `service::llm` resolution to `Result` and remove every Claude/Pi
   catch-all in Section 10.1.
5. Add fallback schema, lint validation, deterministic engine, and provenance.
6. Wire retryable state into worker and agency dispatcher paths.
7. Update setup/profile UX and migration/lint.
8. Update remaining LLM call sites and documentation; add a repository invariant
   test against new hard-coded fallback calls.

During rollout there must be no window in which defaults are removed from one
path but another path can still invoke Claude as a catch-all. The shared
preflight should land before individual fallback deletion.

## 12. Acceptance test plan

### 12.1 Unit tests

1. `Config::default()` is graph-valid but `execution_selection()` is
   `Unselected`; built-in Claude registry entries do not change it.
2. Global/local explicit `claude:opus`, `codex:*`, `pi:*`, and `nex:*` fields
   resolve `Selected` with exact file/key source.
3. Installed binaries, env keys, launcher history, endpoint defaults, and
   starter-profile files do not select a route.
4. Active valid profile selects; unknown/empty profile fails instead of using
   Claude tiers.
5. Every handler adapter derives the expected `ExecutionSystemKey`.
6. Same-handler/same-provider fallback validates; handler or provider changes
   fail config validation.
7. No fallback declaration yields `WG-EXEC-ROUTE-FAILED`; tiers/ranked cache are
   ignored.
8. Credential resolution never uses a default endpoint with a mismatched
   provider.
9. Each old `src/service/llm.rs` fallback test is replaced with a test asserting
   loud failure or explicitly configured same-system fallback.
10. A source scan/invariant test rejects new production calls that hard-code
    Claude/Pi as a fallback (allow catalog IDs, explicit route templates, tests,
    and the Claude adapter itself).

### 12.2 CLI/integration tests in isolated `HOME`

1. `wg init` with empty `HOME` creates graph files and no active route;
   `wg add/list/edit/show` all succeed without credentials.
2. In that repo, `wg service start` exits nonzero with
   `WG-EXEC-UNSELECTED`, prints all exact setup commands, and creates no daemon,
   socket, PID/state, claim, or worktree.
3. `wg spawn` and `wg chat` fail with the same code and no graph/chat mutation.
4. `wg done` succeeds graph-only while auto-evaluation becomes waiting and no
   verdict is written.
5. `wg setup --yes` fails; each explicit `--route ... --yes` fixture records the
   correct selection source. `--dry-run` does not select.
6. `wg profile init-starters` remains unselected; `wg profile use codex` selects
   Codex; `wg profile use pi` selects Pi. Neither activation silently replaces
   the other on failure.
7. `wg config --local --model ...` overrides global selection with visible
   source; removing the local field reveals the explicit global source, not a
   default.
8. Missing/invalid credential produces the selected provider's exact repair
   command and never spawns another handler.
9. Fault-inject primary failure with two explicitly configured same-system
   candidates: attempts follow file order, first success wins, provenance lists
   both.
10. Fault-inject primary failure with no fallback: worker remains retryable,
    evaluation has no verdict, review is quarantined, queued chat has an error,
    and no semantic failure budget is consumed.
11. A cross-handler and a same-handler/cross-provider fallback each make
    `wg config lint` and `wg service start` fail before dispatch.
12. Legacy explicit configs for every supported handler continue unchanged;
    missing-route and executor-only legacy configs become unselected with exact
    migration commands.
13. A corrupt config cannot fall back to Claude for service/spawn even though
    graph read commands may use structural defaults.

### 12.3 User-visible smoke scenario

Add `tests/smoke/scenarios/explicit_execution_selection.sh` and a grow-only
manifest entry owned by the implementation tasks. Drive the real terminal flow
(with `script`, PTY, or `expect` where interaction is tested):

1. fresh `HOME` → `wg init` → graph CRUD succeeds;
2. real `wg service start` fails visibly/actionably and leaves no daemon state;
3. interactive `wg setup` shows graph-only/not-now as the fresh default and
   requires a route choice;
4. select a credential-free mocked handler route, start the service, and run one
   worker/agency call;
5. kill/fail the primary and prove only an explicit same-system fallback runs;
6. prove a configured cross-system fallback is rejected.

The scenario must fail against the old implicit-Claude behavior. Normal CI also
runs `cargo fmt --check`, `cargo clippy`, `cargo build`, and `cargo test`.

## 13. Explicit non-goals

- Choosing which vendor/model is “best.”
- Making Terra, Sol, Luna, Pi, Claude, or any other route a default.
- Redesigning `FailedPendingEval` respawn or Node `EPIPE` handling.
- Treating content-review fail-closed policy as permission to change providers.
- Removing built-in model metadata or starter profile templates.

The contract is deliberately small: graph use is always available; LLM use is
explicit; failure stays within the selected handler/provider unless the user
listed a same-system alternative; otherwise WG fails loudly and preserves work
for retry.
