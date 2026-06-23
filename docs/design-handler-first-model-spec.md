# Design: Handler-First Model Spec (`<handler>:<native-model>`)

**Task:** design-handler-first
**Date:** 2026-06-23
**Status:** Proposed (design only — names code to change, does NOT implement)
**Follow-up impl task:** described in [§11 Implementation plan](#11-implementation-plan)
**Related:** [config-ux-design.md](config-ux-design.md), `src/dispatch/handler_for_model.rs` module doc

---

## TL;DR

A model spec is **`<handler>:<handler-native-model>`**. wg parses **only the
leading token** (the handler/executor name) and passes everything after the
first `:` to that handler **verbatim** — the remainder is the handler's own
native model dialect, opaque to wg's routing decision.

The leading token MUST name a **handler** (`claude`, `codex`, `nex`, `pi`,
`opencode`, `aider`, …). It MUST NOT name a **provider**
(`openrouter`, `openai`, `oai-compat`, `ollama`, `vllm`, `llamacpp`, `gemini`,
`local`). To run an OpenRouter / OAI-compat model you name a handler and put the
provider in the *inner* dialect:

```
nex:openrouter:z-ai/glm-5.2     # in-process native handler, OpenRouter wire
pi:openrouter:z-ai/glm-5.2      # pi CLI handler, OpenRouter wire (pi auths itself)
```

Bare Anthropic aliases (`opus` / `sonnet` / `haiku` → claude) are **unchanged** —
claude is unambiguous; this change targets PROVIDER prefixes only.

The rollout reuses the existing `local:` / `oai-compat:` → `nex:` deprecation
playbook **exactly**: one release that WARNs + defaults to `nex:`, then a
hard-error at the strict-validation entry points, with `wg migrate config`
auto-rewriting and `wg config lint` flagging the stale form. Resolution is made
**visible** (`config --models` / spawn provenance echo the resolved handler) so
the silent mis-route that caused the originating incident can't recur.

**No runtime behavior changes in this task — design only.**

---

## 1. Why this shape

### 1.1 The problem: the prefix is overloaded

The model-spec prefix names BOTH the wire provider AND selects the handler,
collapsed into one token. That is fine where the mapping is 1:1:

| Spec | Handler | Wire |
|---|---|---|
| `claude:opus` | claude CLI | Anthropic |
| `codex:gpt-5.5` | codex CLI | OAI-compat |
| `nex:qwen3-coder` | native (in-process) | OAI-compat |

It breaks for **open models more than one handler can serve**.
`openrouter:z-ai/glm-5.2` is silently routed to the **native** in-process handler
by `provider_to_executor` (`src/config.rs:2002`), with no way to express "run this
through **pi**" except the already-working `pi:openrouter/…` form.

### 1.2 The incident this came from

A dispatcher set to `openrouter:z-ai/glm-5.2` ran on `native`, had no OpenRouter
credential, and **every task died at ~1s with HTTP 401**. Two failures, not one:

1. **Surprise routing** — "why is it `native` when I wanted pi?" The provider
   prefix silently picked a handler the user never named.
2. **Invisible resolution** — nothing echoed `handler=native` until an agent
   died. The mis-route was undiagnosable until the symptom.

This design fixes (1) by forbidding provider prefixes as leading tokens (you
always name the handler) and (2) by echoing the resolved handler everywhere a
spec is shown or spawned (§7).

### 1.3 The key realization (scopes the change down)

The string AFTER the handler prefix is just **that handler's own native model
dialect**:

- claude wants `opus`
- codex wants `gpt-5.5`
- nex / pi want a `provider:model` route like `openrouter:z-ai/glm-5.2`

So a spec is really **`<handler>:<handler-native-model>`**, and wg should parse
**only the leading handler token**, passing the remainder through verbatim.

This is **not a new architecture** — the CLI/external handlers ALREADY work this
way:

- `handler_for_model` (`src/dispatch/handler_for_model.rs:80-88`) first intercepts
  external-CLI executor prefixes (`pi`, `opencode`, `aider`, `goose`, `qwen`, …)
  via `ExecutorKind::from_str(prefix) + is_external_cli()` and routes by executor
  name. `pi:openrouter/…` → `ExecutorKind::Pi` works **today**.
- The adapters (`opencode_model_arg` / `octomind_model_arg` in
  `src/chat_command.rs:65,114`) strip their own leading executor prefix and parse
  the remainder with `parse_model_spec` — i.e. exactly "split leading handler,
  pass rest."

The **single rule-violator** is the fallback path in `handler_for_model`
(`src/dispatch/handler_for_model.rs:90-95`): after the external-CLI interception,
it calls `parse_model_spec` then `provider_to_executor(prefix)`
(`src/config.rs:2002`), which maps provider prefixes
(`openrouter` / `openai` / `oai-compat` / `local` / `ollama` / `vllm` /
`llamacpp` / `gemini` / `native`) → an executor. **That is the one place wg
pretends a provider is a handler.** Penning it in is the whole change.

---

## 2. The rule

1. **The leading token of a model spec MUST be a handler/executor name** —
   `claude`, `codex`, `nex` (and its legacy alias `native`), or an external CLI
   (`pi`, `opencode`, `aider`, `goose`, `qwen`, `cline`, `crush`, `amplifier`,
   `octomind`, `dexto`). Everything after the first `:` is the handler's native
   model string, passed through verbatim.

2. **Provider names are NOT valid leading handler tokens** — `openrouter`,
   `openai`, `oai-compat`, `ollama`, `vllm`, `llamacpp`, `gemini`, `local`. To run
   an OpenRouter / OAI-compat model you name a handler:
   `nex:openrouter:vendor/model` (in-process native) or `pi:openrouter:vendor/model`
   (pi CLI).

3. **Bare Anthropic aliases stay unchanged** — `opus` / `sonnet` / `haiku` → claude.
   claude is unambiguous; this deprecation targets PROVIDER prefixes only. The
   lenient resolver's bare-name → claude default
   (`src/dispatch/handler_for_model.rs:96-100`) is untouched.

### 2.1 Two parse layers — keep them separate

The change cleanly separates two responsibilities that the overloaded prefix
conflated:

| Layer | Question | Input | Owner |
|---|---|---|---|
| **Handler selection** | which subprocess/handler runs this? | the LEADING token only | `handler_for_model` |
| **Native-model resolution** | which wire protocol / endpoint / model id? | the INNER dialect (after the first `:`) | the handler adapter (`provider_to_native_provider`, `opencode_model_arg`, `pi_model_arg`, …) |

`provider_to_executor` is a **handler-selection** function and is the thing that
must stop accepting provider prefixes. `provider_to_native_provider`
(`src/config.rs:2191`) is a **native-resolution** function and is **unchanged** —
it stays the native handler's internal dialect parser
(`openrouter`/`oai-compat`/`ollama`/… → wire protocol).

---

## 3. The parse grammar

```
spec        := handler ":" native-model
             | bare-alias                  # opus | sonnet | haiku  → claude

handler     := claude | codex | nex | native        # native = legacy alias of nex
             | pi | opencode | aider | goose | qwen  # external CLIs
             | cline | crush | amplifier | octomind | dexto

native-model := <opaque to wg; the handler's own dialect>
```

**Parse rule:** split on the **FIRST** `:` only. The handler token is the part
before it; the native model is everything after — `/` and further `:` stay inside
the native model.

```
pi:openrouter/anthropic/claude-3.5-haiku   → handler=pi,  native="openrouter/anthropic/claude-3.5-haiku"
pi:openrouter:anthropic/claude-3.5-haiku   → handler=pi,  native="openrouter:anthropic/claude-3.5-haiku"
nex:openrouter:z-ai/glm-5.2                → handler=nex, native="openrouter:z-ai/glm-5.2"
nex:ollama:llama3                          → handler=nex, native="ollama:llama3"
claude:opus                                → handler=claude, native="opus"
opus                                        → bare alias → claude
```

### 3.1 Inner spelling is handler-specific (and already round-trips)

The inner dialect's spelling is the **handler's**, not wg's:

- **nex (in-process):** wg's canonical `provider:model` colon form,
  e.g. `openrouter:z-ai/glm-5.2`. Parsed by the existing `parse_model_spec` +
  `provider_to_native_provider`.
- **pi (CLI):** pi's `openrouter/<vendor>/<model>` slash form. `pi_model_arg`
  (`src/commands/pi_handler.rs`) / `pi_strong_route` (`src/config.rs:2261`) emit
  the slash form; `parse_executor_model_route` (`src/dispatch/plan.rs:242`)
  already normalizes `openrouter/…` → `openrouter:…` when ingesting.

Because `parse_executor_model_route` normalizes the slash spelling and the
adapters convert back, **both inner spellings of pi already round-trip today**.
The grammar accepts either; wg's internal canonical inner form is the colon form.

### 3.2 `nex:openrouter:…` — the one new inner-parse step

`pi:openrouter:…` already works (pi is an external CLI; `handler_for_model`
intercepts the prefix and the adapter parses the rest). `nex:openrouter:…` is the
new shape that needs a small handler-side change, because `nex`/`native` are NOT
external CLIs and so are NOT handled by `parse_executor_model_route`
(`src/dispatch/plan.rs:245` gates on `is_external_cli()`).

The mechanics: `parse_model_spec("nex:openrouter:z-ai/glm-5.2")` splits one colon
→ `provider=Some("nex")`, `model_id="openrouter:z-ai/glm-5.2"`. The native
handler must then **re-parse `model_id`** to recover the `openrouter` sub-provider
(another `parse_model_spec` → `provider_to_native_provider`). This is the same
"strip leading handler token, parse the rest" move the CLI adapters already do
(`opencode_model_arg` at `src/chat_command.rs:68`,
`octomind_model_arg` at `src/chat_command.rs:115`). See touch-point §6.3.

---

## 4. Rejected-provider-prefix policy

The leading token is validated against the **handler set**, not `KNOWN_PROVIDERS`.
Conceptually `KNOWN_PROVIDERS` (`src/config.rs:1763`) splits into two roles:

| Role | Members | Valid as leading token? | Valid as inner dialect? |
|---|---|---|---|
| **Handler names** | `claude`, `codex`, `nex`, `native` | ✅ yes | n/a |
| **Native sub-providers** | `openrouter`, `openai`, `oai-compat`, `ollama`, `vllm`, `llamacpp`, `gemini`, `local` | ❌ **rejected** | ✅ yes (after `nex:`) |

Plus the external CLIs (`EXTERNAL_CLIS` in `src/dispatch/plan.rs:107`) are valid
leading tokens (already handled).

The rejected set is therefore `KNOWN_PROVIDERS \ {claude, codex, nex, native}`.

**Hard-error message** (release N+1, at strict entry points):

```
`openrouter` is a model namespace, not a handler — use `nex:openrouter:…` or `pi:openrouter:…`.
```

This mirrors the existing `amplifier` rejection in `parse_model_spec_strict`
(`src/config.rs:1910-1923`: "`amplifier` is an executor name, not a model
provider prefix"), just in the opposite direction (provider-not-handler vs
handler-not-provider).

---

## 5. Migration / deprecation (reuse the established playbook)

Mirror the `local:` / `oai-compat:` → `nex:` deprecation **exactly** (the
existing infrastructure is `deprecated_provider_prefix_replacement` at
`src/config.rs:1783`, `deprecated_model_prefix_warnings_for_toml` at
`src/config.rs:2097`, the migrate rewrite at `src/commands/migrate.rs:861`, and
`lint_config` at `src/commands/config_cmd.rs:2684`).

### 5.1 Two-release timeline

**Release N (warn + default to `nex:`):**
- A bare leading provider prefix (`openrouter:X`, `openai:X`, `ollama:X`, …)
  WARNs on stderr at config load / strict parse and **DEFAULTS to `nex:`** — i.e.
  resolves as `nex:openrouter:X`, so nothing breaks immediately.
- The lenient resolver (`handler_for_model` / `provider_to_executor`) stays
  **tolerant and silent** — its existing `_ => "native"` arm already maps these to
  the native handler, which is exactly `nex:` behavior. (The warnings come from
  the deprecation surfaces, not the hot resolver path — same division as today's
  `local:`/`oai-compat:` handling.)

**Release N+1 (hard error):**
- Strict-validation entry points (CLI `--model` parse, config load) **hard-error**
  with the §4 message.
- The lenient resolver may stay tolerant permanently (it is documented as the
  lenient internal path; strict validation is the entry points' job) — but by N+1
  no bare-provider spec survives the entry points to reach it.

### 5.2 `wg migrate config` rewrite targets

This is where the new deprecation **differs in shape** from `local:`/`oai-compat:`.
Those were *pure aliases* of `nex:` (no distinct sub-provider), so the rewrite is a
**swap** (`local:X` → `nex:X`). The new provider prefixes mostly carry a wire
meaning the native handler still needs, so the rewrite is a **prepend**
(`openrouter:X` → `nex:openrouter:X`) — the provider becomes the *inner* dialect.

| Legacy top-level prefix | Migration target | Rewrite kind | Rationale |
|---|---|---|---|
| `local:X` | `nex:X` | **swap** (existing) | `local` was always the nex localhost alias |
| `oai-compat:X` | `nex:X` | **swap** (existing) | oai-compat is the default `nex:` wire |
| `openai:X` | `nex:X` | **swap** | legacy alias of oai-compat (`provider_to_native_provider("openai")="oai-compat"`); default `nex:` wire is oai-compat → behavior-preserving |
| `native:X` | `nex:X` | **swap** | `native` is the legacy executor name for the nex handler |
| `openrouter:X` | `nex:openrouter:X` | **prepend** | OpenRouter wire is distinct; keep it as inner dialect |
| `ollama:X` | `nex:ollama:X` | **prepend** | distinct `local` wire flavor |
| `vllm:X` | `nex:vllm:X` | **prepend** | distinct `local` wire flavor |
| `llamacpp:X` | `nex:llamacpp:X` | **prepend** | distinct `local` wire flavor |
| `gemini:X` | `nex:gemini:X` | **prepend** | distinct provider label |

Clean rule: **collapse the pure-alias set `{local, oai-compat, openai, native}`
to `nex:` (drop the prefix); prepend `nex:` to the wire-distinct set
`{openrouter, ollama, vllm, llamacpp, gemini}` (keep the prefix as inner dialect).**

> **Implementation note for `wg migrate config`:** the existing
> `deprecated_provider_prefix_replacement` (`src/config.rs:1783`) returns a single
> replacement prefix and `fix_stale_model_strings` (`src/commands/migrate.rs:861`)
> does a swap (`format!("{replacement}:{rest}")`). The prepend cases need a richer
> rewrite — either a second helper (e.g. `provider_prefix_migration(prefix) ->
> Migration::{Swap(&str), Prepend("nex")}`) or extend the existing one to return
> the full rewritten string. Keep `STALE_PROVIDER_REWRITES` in sync (the
> `migrate.rs` comment at line 1782 already requires this).

### 5.3 `wg config lint` (read-only)

`lint_config` (`src/commands/config_cmd.rs:2684`) reuses `migrate_one(path,
dry_run=true)`, so once the migrate rewrite (§5.2) lands, **lint flags bare
provider prefixes automatically** — no separate code path. The
`deprecated_model_prefix_warnings_for_toml` walk (`src/config.rs:2097`) should be
extended to the same prefix set so the load-time warning and the lint agree.

---

## 6. Code touch-points (exact files/functions)

### 6.1 `handler_for_model` — the lenient resolver
`src/dispatch/handler_for_model.rs:71` (function) + module-doc mapping table
(lines 26-49).
- The external-CLI interception (lines 80-88) is **already correct** — leave it.
- The fallback (lines 90-95) calling `provider_to_executor` is the rule-violator.
  **Decision (§9.1): keep it tolerant for release N** (it silently maps provider
  prefixes → native = `nex:` behavior), and update the **module-doc table** to
  mark `openrouter:*` / `openai:*` / `ollama:*` / … rows as *deprecated leading
  forms* (canonical: `nex:openrouter:*`).

### 6.2 `provider_to_executor` and `parse_model_spec` / `parse_model_spec_strict`
`src/config.rs:2002` (`provider_to_executor`), `:1830` (`parse_model_spec`,
lenient), `:1900` (`parse_model_spec_strict`).
- `provider_to_executor` is the handler-selection function; its `_ => "native"`
  arm is what makes provider prefixes resolve to native. Leave it tolerant for
  release N (per §9.1); the enforcement lands in `parse_model_spec_strict`.
- `parse_model_spec_strict` gains a **rejected-provider** branch: a leading token
  in the rejected set (§4) errors with the §4 message. This sits next to the
  existing `amplifier` rejection (lines 1910-1923) and the external-CLI accept
  (lines 1933-1942). The known-provider accept branch (lines 1944-1960) narrows to
  the handler set `{claude, codex, nex, native}`.
- `parse_model_spec` (lenient) is unchanged in structure — it still recognizes the
  inner-dialect prefixes so the native handler can re-parse `nex:openrouter:…`.

### 6.3 Native-handler inner re-parse (`nex:openrouter:…`)
The native handler must strip a leading `nex:` / `native:` and re-parse the
remainder for its sub-provider. Today the inner-dialect parse happens via
`provider_to_native_provider` (`src/config.rs:2191`) fed from
`parse_model_spec`. The cleanest implementation is to **generalize
`parse_executor_model_route`** (`src/dispatch/plan.rs:242`) so it also splits
`nex:` / `native:` (drop the `is_external_cli()`-only gate at line 245 for these
two, mapping them to `ExecutorKind::Native` with the rest as the model), OR add a
one-line strip in the native handler's model resolution mirroring
`opencode_model_arg` (`src/chat_command.rs:68`). Either way the result is
`provider_to_native_provider("openrouter")` getting `openrouter`, not `nex`.
Recommend generalizing `parse_executor_model_route` for symmetry with the CLI
handlers.

### 6.4 Spawn path normalization
`parse_executor_model_route` (`src/dispatch/plan.rs:242`) is already called in
`plan_spawn` (`src/dispatch/plan.rs:345`) and already handles
`pi:openrouter/…` (external CLI). Confirm it handles `nex:openrouter:…` and
`pi:openrouter:…` (split leading handler, pass rest) per §6.3.
`normalize_bare_openrouter_route` (`src/config.rs:1858`) and `enforce_model_compat`
(`src/dispatch/plan.rs:560`) are unaffected — they operate on already-routed
specs; verify the `nex:openrouter:…` form flows through `enforce_model_compat`'s
`handler_for_model` check correctly (it should: `handler_for_model("nex:…")` →
Native via the existing path).

### 6.5 Strict-validation entry points (where warn → hard-error lands)
All call `parse_model_spec_strict`, so the §6.2 change propagates to them. They
are the surfaces that get the release-N warn and the N+1 hard error:
- `src/commands/add.rs:34,44,59,93` (`wg add --model` resolution)
- `src/commands/edit.rs:67` (`wg edit` model field)
- `src/commands/recover.rs:447`
- `src/commands/profile_cmd.rs:44` (profile activation / `wg profile use <name>:<spec>`)
- config load: `src/config.rs:4959` (and the load-time warning walk at
  `src/config.rs:4276-4281`, `:4409-4416`).

### 6.6 `wg migrate config` + `wg config lint`
`src/commands/migrate.rs:861` (`fix_stale_model_strings`) + the
`STALE_PROVIDER_REWRITES` sync comment (`:1782`); `lint_config`
(`src/commands/config_cmd.rs:2684`); `deprecated_provider_prefix_replacement`
(`src/config.rs:1783`) and `deprecated_model_prefix_warnings_for_toml`
(`src/config.rs:2097`). See §5.2 (the prepend-vs-swap richer rewrite) and §5.3.

### 6.7 Docs
- The `handler_for_model.rs` module-doc mapping table (lines 26-49): mark provider
  rows as deprecated leading forms; add `nex:openrouter:*` / `pi:openrouter:*`
  canonical rows.
- The "Service Configuration" section of **both** `CLAUDE.md` and `AGENTS.md`
  (kept in lock-step — divergence is a bug). Update the `(model, endpoint)`
  examples to the handler-first forms (`nex:openrouter:vendor/model`) and note the
  rejected bare provider prefixes.

---

## 7. Make resolution visible (folds in half the original bug)

The 401 incident was invisible until an agent died. Echo the resolved handler
wherever a spec is shown or spawned:

1. **`wg config --models` / `show_model_routing`** (`src/commands/config_cmd.rs:1382`):
   the table already has ROLE / TIER / MODEL / PROVIDER / ENDPOINT / SOURCE — add a
   **HANDLER** column (or annotate MODEL) computed via `handler_for_model`, e.g.
   `openrouter:z-ai/glm-5.2 → handler=native (default; override with nex: or pi:)`.
2. **Spawn provenance** (`SpawnProvenance` / `log_line` at
   `src/dispatch/plan.rs:261,277`): already logs `executor=… model=… endpoint=…`
   per spawn. During release N, when a spec was resolved via the deprecated
   provider-default, include that in `executor_source` (e.g.
   `"provider-default (deprecated: openrouter → nex)"`) so the daemon log shows
   the implicit resolution.
3. **`wg status`** (`src/commands/status.rs:221`): already surfaces
   `effective_executor()`; ensure it reflects the handler the model resolves to,
   not just the configured `[dispatcher].executor`.

This is design-only here; the impl task wires the column + provenance string.

---

## 8. What explicitly does NOT change

- **Bare `opus` / `sonnet` / `haiku` → claude** (`handler_for_model.rs:96-100`).
- **`provider_to_native_provider`** (`src/config.rs:2191`) — the native handler's
  inner-dialect parser stays; `openrouter`/`oai-compat`/`ollama`/… remain valid
  **inner** dialects.
- **`pi:openrouter/…`** and the other external-CLI executor-prefix routes — they
  already follow the handler-first rule.
- **The `local:` / `oai-compat:` deprecation already in flight** — this change
  extends the *same* playbook to the remaining provider prefixes; it does not
  reset or alter the in-progress local/oai-compat sunset.
- **Runtime behavior in *this* task** — design only.

---

## 9. Resolved open decisions

### 9.1 Penned-in-resolver vs strict-entry-point enforcement
**Recommendation: enforce at the strict entry points; keep the lenient resolver
tolerant during the one-release window.**

Rationale: this is precisely how `local:`/`oai-compat:` were handled.
`handler_for_model` / `provider_to_executor` are on hot spawn/resolve paths and
are documented as the *lenient* layer (`handler_for_model.rs:63` "Parses the
provider prefix (lenient)"); strict validation is the entry points' job
(`parse_model_spec_strict` doc, `src/config.rs:1827`). Keeping the resolver
tolerant means release N breaks nothing — a bare provider prefix keeps resolving
to native (= `nex:`) — while the WARN at the entry points + `migrate`/`lint` drive
the migration. The hard error then lands in one well-tested place
(`parse_model_spec_strict`) that every entry point already funnels through (§6.5),
rather than scattering rejection across the hot paths.

### 9.2 Does `pi` become a default for anything?
**Recommendation: NO.**

Rationale: once bare provider prefixes are rejected, there is **no implicit
default to flip** — you always name the handler explicitly. `openrouter:X` no
longer silently picks *a* handler; the user writes `nex:openrouter:X` or
`pi:openrouter:X`. So pi-as-worker readiness becomes a **per-use correctness
requirement** (does the named pi route actually run? — already covered by
`pi_route_lint`, `src/commands/config_cmd.rs:2658`), not a routing-default risk.
Making pi a default would reintroduce exactly the "a handler I didn't name got
picked" surprise this design removes.

### 9.3 Exact migration target for each legacy provider prefix
**Resolved in §5.2.** Summary: collapse the pure-alias set
`{local, oai-compat, openai, native}` → `nex:` (swap, drop prefix); prepend `nex:`
to the wire-distinct set `{openrouter, ollama, vllm, llamacpp, gemini}` (keep the
prefix as inner dialect). The two existing entries (`local`, `oai-compat`) keep
their current swap behavior; the rest are new.

---

## 10. Examples (before → after)

| User intent | Old (overloaded) | New (handler-first) |
|---|---|---|
| GLM via in-process nex | `openrouter:z-ai/glm-5.2` | `nex:openrouter:z-ai/glm-5.2` |
| GLM via pi CLI | `pi:openrouter/z-ai/glm-5.2` | `pi:openrouter:z-ai/glm-5.2` (or slash form) |
| local Ollama | `ollama:llama3` | `nex:ollama:llama3` |
| OAI-compat localhost | `oai-compat:gpt-x` | `nex:gpt-x` |
| Anthropic via claude CLI | `claude:opus` / `opus` | unchanged |
| Codex CLI | `codex:gpt-5.5` | unchanged |
| OpenCode worker | `opencode:openrouter/…` | unchanged |

Rejected at strict entry points (release N+1):
`openrouter:…`, `openai:…`, `oai-compat:…`, `ollama:…`, `vllm:…`, `llamacpp:…`,
`gemini:…`, `local:…` as **leading** tokens.

---

## 11. Implementation plan (turnkey for the follow-up impl task)

Ordered steps. Each is small and independently testable; the whole thing is a
parser/validation + migration change with no new runtime subsystem.

1. **Define the handler/provider split.** In `src/config.rs`, add
   `HANDLER_PREFIXES = {claude, codex, nex, native}` and a predicate
   `is_rejected_leading_provider(prefix) -> bool` over
   `KNOWN_PROVIDERS \ HANDLER_PREFIXES` (excluding external CLIs, which are
   handlers). Unit-test the partition.

2. **Strict rejection branch.** In `parse_model_spec_strict`
   (`src/config.rs:1900`), add a branch: leading token is a rejected provider →
   `Err` with the §4 message. Keep it **behind a release-N flag/feature** that
   currently WARNs-and-accepts (resolving as `nex:<prefix>:<rest>`) and flips to
   hard-error in N+1. Tests: `openrouter:x` warns in N, errors in N+1; `claude:opus`,
   `nex:openrouter:x`, `pi:openrouter/x`, bare `opus` all still pass.

3. **`nex:openrouter:…` inner re-parse.** Generalize `parse_executor_model_route`
   (`src/dispatch/plan.rs:242`) to also split `nex:` / `native:` (→
   `ExecutorKind::Native`, rest = model), or add the equivalent strip in the
   native handler's model resolution (mirror `opencode_model_arg`,
   `src/chat_command.rs:68`). Test: `nex:openrouter:z-ai/glm-5.2` resolves to
   handler=native + native-provider=openrouter (round-trips through
   `provider_to_native_provider`).

4. **Migration rewrites (`wg migrate config`).** Add the prepend-vs-swap logic
   from §5.2 to `fix_stale_model_strings` (`src/commands/migrate.rs:861`) — likely
   a new `provider_prefix_migration(prefix) -> Option<Migration>` helper next to
   `deprecated_provider_prefix_replacement` (`src/config.rs:1783`). Keep
   `STALE_PROVIDER_REWRITES` (`:1782` comment) in sync. Tests: `openrouter:x →
   nex:openrouter:x`, `ollama:x → nex:ollama:x`, `local:x → nex:x` (unchanged),
   `openai:x → nex:x`.

5. **Load-time warning + lint.** Extend
   `deprecated_model_prefix_warnings_for_toml` (`src/config.rs:2097`) to the full
   rejected set so the load warning matches migrate/lint. `lint_config`
   (`src/commands/config_cmd.rs:2684`) inherits the migrate predicates
   automatically; add a test asserting a config with `openrouter:` is flagged.

6. **Visibility echo.** Add the HANDLER column/annotation to `show_model_routing`
   (`src/commands/config_cmd.rs:1382`) and the deprecated-resolution note to
   `SpawnProvenance` (`src/dispatch/plan.rs:258`). Verify `wg status`
   (`src/commands/status.rs:221`) reflects the resolved handler.

7. **Docs.** Update the `handler_for_model.rs` module-doc table (lines 26-49) and
   the "Service Configuration" section of **both** `CLAUDE.md` and `AGENTS.md`
   (lock-step). Add a smoke scenario if any user-visible CLI surface changes (e.g.
   `wg config --models` showing the handler echo, or `wg add --model
   openrouter:x` warning then erroring) under `tests/smoke/scenarios/` listed in
   `tests/smoke/manifest.toml`.

8. **Release-flag flip (N → N+1).** A one-line change flips the step-2 branch from
   warn-and-accept to hard-error. Land it in the N+1 release commit so N ships the
   warning window first.

**Suggested fan-out for the impl task** (file-disjoint where possible):
- *Parser/validation* — `src/config.rs` (steps 1, 2, 5) + `src/dispatch/plan.rs`
  (step 3).
- *Migration/lint* — `src/commands/migrate.rs` (step 4) — depends on step 1's
  helper.
- *Visibility* — `src/commands/config_cmd.rs`, `src/dispatch/plan.rs` (step 6).
- *Docs + smoke* — `docs/`, `CLAUDE.md`, `AGENTS.md`, `tests/smoke/` (step 7).
- *Integrator* — flip the release flag (step 8) + full `cargo test` + smoke.

---

## 12. Validation of this design doc

- [x] Covers: the rule (§2), parse grammar (§3), rejection policy (§4),
      migration/deprecation (§5), visibility echo (§7), code touch-points (§6),
      resolved open decisions (§9), impl plan (§11).
- [x] Each open decision resolved with a stated recommendation + rationale (§9.1–9.3).
- [x] Cites real symbols/lines: `handler_for_model`
      (`src/dispatch/handler_for_model.rs:71`), `provider_to_executor` /
      `parse_model_spec` / `parse_model_spec_strict` (`src/config.rs:2002 / 1830 /
      1900`), `parse_executor_model_route` (`src/dispatch/plan.rs:242`),
      `wg migrate config` (`src/commands/migrate.rs:861`), `wg config lint`
      (`src/commands/config_cmd.rs:2684`).
- [x] Preserves bare `opus`/`sonnet`/`haiku` → claude (§2.3, §8) and reuses the
      `local:`/`oai-compat:` → `nex:` deprecation playbook (§5, §8).
- [x] No source/behavior changes in this task; follow-up impl task described (§11).
