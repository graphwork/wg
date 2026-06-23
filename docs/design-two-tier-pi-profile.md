# Design: Two-Tier Pi Profile CLI (`strong` / `weak`)

**Task:** design-two-tier
**Date:** 2026-06-23
**Status:** Proposed (design only — names code to change, does NOT implement)
**Implementation tasks:** interactive-pi-model, model-scout-re, fix-route-agency
**Depends on:** [design-named-profiles.md](design-named-profiles.md), [config-ux-design.md](config-ux-design.md)

---

## TL;DR

Add one verb, `wg profile pi`, that sets/shows two **stable tier labels** for the Pi
profile:

- **`strong`** — chat + real work (quality matters): chat, worker (TaskAgent), and the
  heavy generative roles.
- **`weak`** — agency judgment one-shots where a wrong call is cheaply recoverable:
  `.flip` / `.assign` / post-flip evaluation / off-the-rails detection.

The two tiers are the **stable interface** that insulates the rest of WG from
OpenRouter model churn. The model *behind* each tier moves (the scout repoints it);
the tier label does not.

The verb accepts **both** input forms — you pick:

```
wg profile pi --strong <spec> --weak <spec>     # explicit, self-documenting, partial-update
wg profile pi <strong> <weak>                   # positional, terse
```

Every set (and every scout apply) **prints the resulting assignment** with
`old → new` / `unchanged`. That echo is what makes the positional form safe: a
transposed invocation is caught immediately because the output shows which tier got
which model.

**Storage is generic, not a new schema.** `strong`/`weak` are CLI-level labels that
project onto the *existing* `[tiers]` + `[models.<role>]` config keys (exactly the
layout `src/profile/templates/pi.toml` already hand-writes). No parallel config
system; `wg profile show` already renders the result.

---

## 1. Why this shape

### 1.1 The problem

Today the Pi profile encodes a two-tier split by hand across ~17 config keys
(`src/profile/templates/pi.toml`): `strong = pi:openrouter/z-ai/glm-5.2` written into
six work keys, `weak = openrouter:deepseek/deepseek-chat` written into `tiers.fast`
plus eleven `[models.<role>]` overrides. There is **no command** to set those two
values as a unit, and nothing prints "which model is strong, which is weak" — so
moving a tier means hand-editing TOML and hoping you hit every key.

OpenRouter model identity is a moving target. We want a **stable** `strong`/`weak`
interface so the rest of WG (and humans) never refer to a concrete model id; the
[model-scout](#7-the-scout-wg-profile-pi---scout) repoints the tiers when the market
moves.

### 1.2 The key realization: two-tier is a 2-coloring of the existing three tiers

WG already has three quality tiers (`fast` / `standard` / `premium`) and 13 dispatch
roles that map onto them via `DispatchRole::default_tier()` (`src/config.rs:1244`).
The two-tier model is just a **2-coloring** of that existing system:

| 3-tier   | → | 2-tier     |
|----------|---|------------|
| premium  | → | **strong** |
| standard | → | **strong** |
| fast     | → | **weak**   |

This is the whole design. `strong` drives everything the `standard` and `premium`
tiers drive; `weak` drives everything the `fast` tier drives. We do **not** invent
new config keys — we write the existing ones. `wg profile pi` is a *facade* over the
generic tier/role machinery.

### 1.2a The strong tier executes through the pi handler (not nex)

**The strong tier is always persisted as a `pi:` route, so strong-tier work runs
through the self-authenticating `pi` handler — wg never becomes the OpenRouter
HTTP client.** This is the `fix-strong-tier` fix.

The mapping that decides which subprocess executes a model is
[`handler_for_model`](../src/dispatch/handler_for_model.rs) (the single source of
truth). It routes a *raw* `openrouter:` spec to the **in-process nex / `native`**
handler — which makes **wg itself** the OpenRouter HTTP client and so REQUIRES an
OpenRouter key wired into wg config. With no key, strong-tier workers die at spawn
with a wrapper-internal exit 1 *before any work*. By contrast a `pi:` route maps to
`ExecutorKind::Pi`, which runs the model through pi using **pi's own login** —
exactly like the `claude:` / `codex:` CLI handlers auth themselves. wg then needs no
OpenRouter secret of its own.

So the strong tier's spec must be a `pi:` route:

| strong spec (input)                | persisted / dispatched           | handler                    |
|------------------------------------|----------------------------------|----------------------------|
| `openrouter:z-ai/glm-5.2`          | `pi:openrouter/z-ai/glm-5.2`     | `pi` (pi auths itself)     |
| `z-ai/glm-5.2` (bare slash route)  | `pi:openrouter/z-ai/glm-5.2`     | `pi`                       |
| `pi:openrouter/z-ai/glm-5.2`       | `pi:openrouter/z-ai/glm-5.2`     | `pi` (already correct)     |
| `claude:opus` / `codex:gpt-5.5`    | unchanged                        | the CLI (auths itself)     |
| `nex:qwen3-coder` (local)          | unchanged                        | `native` (needs endpoint)  |

The normalization lives in **one** function, `config::pi_strong_route`, applied at
*every* path that persists the strong tier so none can reintroduce a nex-routed
strong spec: `Config::set_pi_tiers`, `profile::named::patch_pi_tiers` (the
comment-preserving file writer), and the [model-scout](#7-the-scout-wg-profile-pi---scout)
(`--apply` write + the copy-pasteable echo). It is idempotent (a `pi:` route in →
the same `pi:` route out). The hand-written `pi.toml` starter already uses the `pi:`
form, so it needs no rewrite.

**The weak/agency tier is deliberately NOT routed through pi.** It keeps its native
`openrouter:` route (e.g. `openrouter:deepseek/deepseek-chat`) and the agency
resolver's loud keyless-native `claude:haiku` fallback (`resolve_agency_dispatch`) —
those are short, recoverable one-shots, and the native path is faster and cheaper for
that traffic.

### 1.3 What we are NOT building

- **No `[pi]` config section / no new schema.** `strong`/`weak` are projections onto
  existing keys. (See [§5](#5-generic-vs-pi-scoped-resolved).)
- **No third input grammar.** Exactly two: explicit flags and ordered positional.
- **No removal of the three-tier system.** Power users keep `[tiers]` and
  `[models.<role>]`; the two-tier verb is sugar that writes a defined subset.
- **No per-role override removal.** Explicit `[models.<role>]` still wins over the
  tier (today and after `fix-route-agency`).

---

## 2. The CLI surface (the exact grammar)

One noun (`wg profile pi`), one set/show/scout verb family:

```
wg profile pi [STRONG] [WEAK]            # positional set (exactly 0 or 2 tokens)
              [--strong <spec>]          # explicit strong (partial-update friendly)
              [--weak   <spec>]          # explicit weak
              [--show]                   # print current tiers (also the no-arg default)
              [--scout]                  # propose tiers from OpenRouter (dry-run default)
              [--apply]                  # with --scout: write the proposal
              [--dry-run]                # print what would change; do NOT write
              [--json]                   # machine-readable (inherits global --json)
              [--no-reload]              # stage write without poking the daemon
```

### 2.1 Grammar rules (precise)

1. **Positional takes exactly two tokens**, in the order `STRONG WEAK`. A literal
   `-` in either slot means **"leave this tier unchanged"**.
   - `wg profile pi A B` → set both.
   - `wg profile pi - B` → set weak only.
   - `wg profile pi A -` → set strong only.
2. **A single bare positional token is an error** (it can't say which tier it is):
   ```
   error: one positional argument is ambiguous — it could be strong or weak.
          Use two tokens with '-' to skip a tier, or a named flag:
            wg profile pi <strong> -          # strong only
            wg profile pi - <weak>            # weak only
            wg profile pi --weak <weak>       # strong only via flag
   ```
   This is what keeps positional safe: a tier is only ever set positionally when its
   slot position is unambiguous.
3. **Flags are the partial-update path.** Either flag alone updates just that tier;
   the other is untouched:
   - `wg profile pi --weak <spec>` → set weak only (this is the scout's common case).
   - `wg profile pi --strong <spec>` → set strong only.
4. **Flags and positional must not set the same tier twice.** Mixing is rejected:
   ```
   wg profile pi A B --strong C
   error: 'strong' specified both positionally ('A') and via --strong ('C'). Pick one.
   ```
   (A flag for one tier + a positional `-` skip for the other is fine but redundant;
   prefer pure flags or pure positional.)
5. **No args at all** = `--show`.

### 2.2 Why support both forms (not force one)

- **Positional** is the terse muscle-memory path once you know the order
  (`strong weak`, mirroring "best first"). The always-on echo makes a transposition
  self-correcting.
- **Flags** are self-documenting and are the natural home for **partial updates**
  (set just `weak`, which the scout does) without a placeholder.

Both reduce to the same internal `(Option<strong>, Option<weak>)` update.

---

## 3. The always-on echo (the "say what we are doing" requirement)

**Every** set, every scout apply, and every dry-run prints the resulting (or
proposed) assignment. This is non-negotiable: legibility over silent writes.

### 3.1 Exact output format — set

```
$ wg profile pi openrouter:z-ai/glm-5.2 openrouter:deepseek/deepseek-chat
Pi profile tiers  (profile: pi)
  strong = pi:openrouter/z-ai/glm-5.2           (pi:openrouter/z-ai/glm-4.6 → pi:openrouter/z-ai/glm-5.2)
  weak   = openrouter:deepseek/deepseek-chat    (unchanged)

  routing: chat, worker, evolver, creator, verification → strong
           .flip, .assign, eval, triage, off-the-rails, compaction → weak

Wrote ~/.wg/profiles/pi.toml
Daemon reloaded — next worker uses the new tiers (in-flight workers keep theirs).
```

Note the strong tier is echoed/persisted as a `pi:` route even though the input was a
raw `openrouter:` spec — see [§1.2a](#12a-the-strong-tier-executes-through-the-pi-handler-not-nex).
The weak tier keeps its native `openrouter:` route.

Format rules for the per-tier line:
- `  <label> = <new spec>` left-block, then a parenthetical:
  - `(<old> → <new>)` when the value changed,
  - `(unchanged)` when this tier was not part of this update,
  - `(new)` when the profile had no prior value for that tier.
- The strong spec is shown in its normalized `pi:` form (what gets persisted); the
  weak spec is shown verbatim.
- The `routing:` block is the [§4 table](#4-routing-table-tier--role) collapsed to one
  line per tier, so the user always sees what each tier actually drives.
- The persistence/reload footer reflects what happened (see [§6](#6-persistence--reload)).

### 3.2 Partial update echo

```
$ wg profile pi --weak openrouter:deepseek/deepseek-v3.1
Pi profile tiers  (profile: pi)
  strong = pi:openrouter/z-ai/glm-5.2           (unchanged)
  weak   = openrouter:deepseek/deepseek-v3.1    (openrouter:deepseek/deepseek-chat → openrouter:deepseek/deepseek-v3.1)
  ...
```

### 3.3 Show

```
$ wg profile pi --show          # or: wg profile pi
Pi profile tiers  (profile: pi)   [active]
  strong = pi:openrouter/z-ai/glm-5.2
  weak   = openrouter:deepseek/deepseek-chat

  routing: chat, worker, evolver, creator, verification → strong
           .flip, .assign, eval, triage, off-the-rails, compaction → weak

  source: ~/.wg/profiles/pi.toml   (strong ← agent.model; weak ← tiers.fast)
```

`[active]` is shown when `~/.wg/active-profile == "pi"`. `--json` emits
`{"profile":"pi","active":true,"strong":"…","weak":"…","routing":{…}}`.

### 3.4 Dry-run (preview that is itself a copy-pasteable apply)

```
$ wg profile pi --strong openrouter:qwen/qwen3-max --dry-run
DRY RUN — no files written.
Pi profile tiers  (profile: pi)
  strong = pi:openrouter/qwen/qwen3-max         (pi:openrouter/z-ai/glm-5.2 → pi:openrouter/qwen/qwen3-max)
  weak   = openrouter:deepseek/deepseek-chat    (unchanged)

Apply with:
  wg profile pi --strong pi:openrouter/qwen/qwen3-max
```

The dry-run's "Apply with:" line is the exact command (in whichever form was used)
that performs the write — copy-pasteable, one command to revert by re-running with
the old value.

---

## 4. Routing table (tier → role)

This is the explicit map the design commits to. The **required five** rows
(chat / worker / .flip / .assign / eval) are at the top; the full 13-role table
follows. Roles are colored by `DispatchRole::default_tier()` (`src/config.rs:1244`)
collapsed per [§1.2](#12-the-key-realization-two-tier-is-a-2-coloring-of-the-existing-three-tiers).

| Surface / role            | DispatchRole               | 3-tier   | **2-tier** | Why |
|---------------------------|----------------------------|----------|------------|-----|
| **chat**                  | (Default / TaskAgent route)| standard | **strong** | user-facing quality |
| **worker**                | `TaskAgent`                | standard | **strong** | produces the work product |
| **`.flip` inference**     | `FlipInference`            | fast     | **weak**   | recoverable judgment one-shot |
| **`.flip` comparison**    | `FlipComparison`           | fast     | **weak**   | recoverable judgment one-shot |
| **`.assign`**             | `Assigner`                 | fast     | **weak**   | recoverable routing call |
| **eval** (post-flip)      | `Evaluator`                | fast     | **weak**   | recoverable scoring |

Full table (all roles):

| DispatchRole       | 3-tier   | **2-tier** | Notes |
|--------------------|----------|------------|-------|
| `Default`          | standard | **strong** | fallback for chat/work |
| `TaskAgent`        | standard | **strong** | the worker |
| `Evolver`          | premium  | **strong** | redesigns the agency — high blast radius |
| `Creator`          | premium  | **strong** | decomposes work into the graph |
| `Verification`     | premium  | **strong** | correctness gate |
| `Evaluator`        | fast     | **weak**   | post-task / post-flip scoring |
| `FlipInference`    | fast     | **weak**   | reconstructs prompt from output |
| `FlipComparison`   | fast     | **weak**   | scores similarity |
| `Assigner`         | fast     | **weak**   | agent assignment |
| `Triage`           | fast     | **weak**   | dead-agent summarization |
| `Placer`           | fast     | **weak**   | wires tasks into the graph |
| `CoordinatorEval`  | fast     | **weak**   | inline per-turn / off-the-rails detection (shares the `evaluator` slot, `src/config.rs:1603`) |
| `Compactor`        | fast     | **weak**   | context.md distillation |
| `ChatCompactor`    | fast     | **weak**   | chat-history summarization |

**Intentional change vs the current `pi.toml`:** the hand-written starter currently
routes the *premium* roles (`evolver`, `creator`, `verification`) to the cheap
DeepSeek model. Under this design they ride **strong** (premium → strong), because
they are quality-critical generative/gating work, not recoverable one-shots. This is
a deliberate correction surfaced by the migration ([§8](#8-migration-of-the-current-hardcoded-choices)).
Anyone who *wants* those on the cheap model keeps the escape hatch: an explicit
`[models.evolver]` / `[models.creator]` / `[models.verification]` override always
wins over the tier and is never touched by the two-tier setter.

### 4.1 Which config keys each tier writes

`strong = <spec>` writes:

| key | role driven |
|-----|-------------|
| `agent.model` | worker / chat fallback |
| `coordinator.model` (dispatcher) | dispatcher |
| `[models.default].model` | Default |
| `[models.task_agent].model` | TaskAgent (worker) |
| `tiers.standard` | standard-tier roles |
| `tiers.premium` | premium-tier roles (evolver/creator/verification) |

`weak = <spec>` writes:

| key | role driven |
|-----|-------------|
| `tiers.fast` | all fast-tier roles |
| `[models.evaluator].model` | Evaluator + CoordinatorEval |
| `[models.assigner].model` | Assigner |
| `[models.flip_inference].model` | FlipInference |
| `[models.flip_comparison].model` | FlipComparison |

The four explicit `[models.<agency role>]` weak writes are required **today**
because agency one-shots are pinned to `claude:haiku` and ignore the tier cascade
(`resolve_agency_dispatch`, `src/service/llm.rs:111`). They are exactly how
`pi.toml` makes "DeepSeek for agency" real right now. Once `fix-route-agency` makes
`resolve_agency_dispatch` fall back to the **weak tier** (`tiers.fast`) instead of
hardcoded haiku, these four become redundant; the setter MAY stop writing them and
rely on `tiers.fast` alone. Until then it writes them for correctness and
legibility. This is the seam that ties this design to `fix-route-agency`.

The other fast-tier roles (`triage`, `placer`, `compactor`, `chat_compactor`) and
the premium roles ride their tier (`tiers.fast` / `tiers.premium`) and need no
explicit per-role key — they are not agency one-shots and so already honor the
cascade.

---

## 5. Generic vs Pi-scoped (resolved)

**Decision: pi-scoped *command*, generic *storage*.**

- **Command is pi-scoped** (`wg profile pi`). That matches the user's mental model
  ("the Pi profile's two tiers"), matches the downstream tasks (`interactive-pi-model`,
  update-pi-starter, the scout), and is the stable surface that insulates callers
  from OpenRouter churn.
- **Storage is generic.** `strong`/`weak` are *not* a new config schema; they are a
  labeled projection onto the existing `[tiers]` + `[models.<role>]` keys
  ([§4.1](#41-which-config-keys-each-tier-writes)). Consequences:
  - No parallel config system to keep in sync; `wg profile show` and
    `resolve_model_for_role` already work unchanged.
  - `fix-route-agency` is a one-place change to `resolve_agency_dispatch` (read the
    weak tier), independent of whether the user ever ran `wg profile pi`.
  - **Extensibility for free:** because the operation is "write a defined key-set
    from a tier label", generalizing to other profiles later (`wg profile tier
    <name> --strong/--weak`) is a rename, not a redesign.

Why not store `[pi.strong]` / `[pi.weak]`? It would duplicate state that already
lives in the tier/role keys, require every resolver to learn a second lookup path,
and re-introduce the "two sources of truth" drift the 2026-05 profile-as-snapshot
pivot removed.

Why pi-scoped command and not generic `wg profile tier` now? YAGNI + the task scopes
this to Pi. The generic storage means we can add the generic verb later without
migration. We name it `pi` today; the implementation is one line away from generic.

---

## 6. Persistence & reload

Consistent with existing `wg profile` behavior (profiles are full config snapshots;
the active one is swapped over `~/.wg/config.toml`; hot-reload via
`IpcRequest::Reconfigure`):

1. **Target file.** `wg profile pi …` reads/writes the **`pi` named profile**
   (`~/.wg/profiles/pi.toml`). If it does not exist, it is seeded from the baked-in
   starter template (`src/profile/templates/pi.toml`) first, then patched — so a
   first run never fails on a missing file.
2. **Surgical key writes.** Only the [§4.1](#41-which-config-keys-each-tier-writes)
   keys for the tier(s) being set are rewritten; comments, ordering, and unrelated
   keys are preserved (same value-level patching `create_profile` already uses).
3. **If `pi` is the active profile** (`~/.wg/active-profile == "pi"`): after writing
   the profile file, re-apply it as global config
   (`named_profile::apply_profile_as_global_config`) and **hot-reload** the daemon
   (`trigger_daemon_reload` → `IpcRequest::Reconfigure`), exactly like
   `wg profile edit` does. In-flight workers keep their model; the next spawned
   worker picks up the new tier. The footer says which happened
   ("Daemon reloaded …" / "Daemon not running — applies on next `wg service start`").
4. **If `pi` is not active:** write the file only; footer notes it will take effect
   on `wg profile use pi`. No daemon poke.
5. **`--no-reload`** stages the write without the IPC, matching the existing flag on
   `use`/`edit`.
6. **Chat sessions** read their profile at startup and stay on it for their lifetime
   (per design-named-profiles §6); a live chat picks up new tiers on
   `wg chat <id> --restart`. The echo footer mentions this when `pi` is active and a
   chat is running.

---

## 7. The scout: `wg profile pi --scout`

`--scout` is the CLI entry point for the re-runnable model-scout
(implemented by `model-scout-re`). This design fixes its contract and output; the
research/selection logic lives in that task.

- **`wg profile pi --scout`** (dry-run default): researches OpenRouter, bootstraps
  from the *current* tiers as the baseline to beat, and prints a proposal that **is a
  copy-pasteable apply command**:
  ```
  $ wg profile pi --scout
  Scouting OpenRouter (baseline: strong=openrouter:z-ai/glm-5.2, weak=openrouter:deepseek/deepseek-chat)…

  Proposed Pi profile tiers  (profile: pi)
    strong = openrouter:z-ai/glm-5.2              (unchanged — still the best coding model)
    weak   = openrouter:deepseek/deepseek-v3.1    (openrouter:deepseek/deepseek-chat → openrouter:deepseek/deepseek-v3.1
                                                    cheaper at equal judgment reliability)

  Apply with:
    wg profile pi --weak openrouter:deepseek/deepseek-v3.1

  (dry run — nothing written. Re-run with --apply to write, or copy the command above.)
  ```
  Note the scout commonly proposes a **single-tier** change (just `weak`); the
  proposal uses the partial-update flag form for exactly that reason.
- **`wg profile pi --scout --apply`**: performs the write and prints the normal set
  echo ([§3.1](#31-exact-output-format--set)), so an applied scout looks identical to
  a manual set — same legibility, same `old → new`.
- The dry-run report and the apply command are **the same strings** (the echo is
  generated once and reused), satisfying "the always-on echo doubles as the scout
  dry-run report". This is the legibility ⇄ reversibility guarantee: what it shows is
  exactly what `--apply` writes, and one re-run with the old value reverts.

`--scout` selection criteria (documented for `model-scout-re`, not hardcoded to
model ids): `strong` = best coding/work model available now; `weak` = cheapest model
that is *reliable enough* for judgment calls (flip/assign/eval/off-the-rails), i.e.
the cheap tier optimizes reliability-at-low-cost, not raw capability.

---

## 8. Migration of the current hardcoded choices

The current `src/profile/templates/pi.toml` **already is** the two-tier layout — it
just lacks a command. Migration is therefore *recognition*, not rewrite:

- **Read path.** `wg profile pi --show` infers the current tiers from existing keys:
  `strong ← agent.model` (or `[models.default].model`), `weak ← tiers.fast`
  (or `[models.evaluator].model`). A pre-existing pi.toml shows correct tiers with no
  edit.
- **Write path.** `wg profile pi <strong> <weak>` rewrites the full
  [§4.1](#41-which-config-keys-each-tier-writes) key-set consistently, so a profile
  that drifted (some keys updated, others stale) is reconciled in one command, and
  the echo shows every `old → new`.
- **The one behavior change** is premium roles (`evolver`/`creator`/`verification`)
  moving from DeepSeek → strong ([§4](#4-routing-table-tier--role)). This is surfaced
  by the `--show`/dry-run echo and reversible via explicit `[models.<role>]`
  overrides. The updated starter template should drop the explicit DeepSeek pins on
  those three roles so they ride `tiers.premium = strong` (a follow-up for
  update-pi-starter / interactive-pi-model).
- **No config-version bump needed** — no schema changed. Existing pi.toml files keep
  working; the verb just edits them.

---

## 9. Concrete examples (all required cases)

```bash
# set BOTH — positional (terse)
wg profile pi openrouter:z-ai/glm-5.2 openrouter:deepseek/deepseek-chat

# set BOTH — flags (self-documenting)
wg profile pi --strong openrouter:z-ai/glm-5.2 --weak openrouter:deepseek/deepseek-chat

# set ONE tier — weak only, flag (the scout's common case)
wg profile pi --weak openrouter:deepseek/deepseek-v3.1

# set ONE tier — strong only, positional with '-' skip placeholder
wg profile pi openrouter:qwen/qwen3-max -

# set ONE tier — weak only, positional with '-' skip placeholder
wg profile pi - openrouter:deepseek/deepseek-v3.1

# show current
wg profile pi                 # no-arg default
wg profile pi --show          # explicit
wg profile pi --show --json   # machine-readable

# preview without writing (copy-pasteable apply line in output)
wg profile pi --strong openrouter:qwen/qwen3-max --dry-run

# scout proposes (dry-run) — prints a copy-pasteable apply command
wg profile pi --scout

# scout applies a tier (writes + echoes like a normal set)
wg profile pi --scout --apply

# error: lone positional is ambiguous
wg profile pi openrouter:z-ai/glm-5.2
#   → error: one positional argument is ambiguous (strong or weak?) — use two
#     tokens with '-' to skip, or --strong/--weak.

# error: same tier set twice
wg profile pi A B --strong C
#   → error: 'strong' specified both positionally ('A') and via --strong ('C').
```

---

## 10. Existing code & config that must change (NOT implemented here)

Implementation is owned by `interactive-pi-model` (the setter) and `fix-route-agency`
(the agency resolver). This design only names the touch points.

**New / changed CLI & command code**

- `src/cli.rs` — add a `Pi` variant to `enum ProfileCommands` (`src/cli.rs:3289`):
  two optional positionals `strong`/`weak` plus `--strong`/`--weak`/`--show`/
  `--scout`/`--apply`/`--dry-run`/`--no-reload`. Wire help text mirroring
  [§2](#2-the-cli-surface-the-exact-grammar).
- `src/main.rs` — add a `ProfileCommands::Pi { … }` match arm next to the existing
  arms (`src/main.rs:2429–2490`) dispatching to a new `profile_cmd::pi(…)`.
- `src/commands/profile_cmd.rs` — add `pub fn pi(...)` implementing set/show/dry-run
  and the [§3](#3-the-always-on-echo-the-say-what-we-are-doing-requirement) echo;
  the grammar-validation in [§2.1](#21-grammar-rules-precise); reuse
  `apply_tier_pins`, `trigger_daemon_reload`, and the `named_profile::*` load/apply
  helpers already in this file. `--scout` delegates to the scout entry point.

**Config plumbing**

- `src/config.rs` — add a `Config::set_pi_tiers(strong: Option<&str>, weak:
  Option<&str>)` writer and a `Config::pi_tiers() -> (Option<String>,
  Option<String>)` reader, modeled on `pin_default_route_model` (`src/config.rs:4489`)
  and the role-fan-out in `split_role_models_routing`
  (`src/config_defaults.rs:526`). These centralize the
  [§4.1](#41-which-config-keys-each-tier-writes) key-set so the setter and `--show`
  cannot disagree.
- `src/profile/named.rs` — surgical TOML patch helpers to write the tier key-set into
  `~/.wg/profiles/pi.toml` while preserving comments/order (the value-patch approach
  `create_profile` uses); seed-from-template when the file is absent.

**Agency routing (owned by `fix-route-agency`, named here as the seam)**

- `src/service/llm.rs` — `resolve_agency_dispatch` (`:111`) and
  `is_agency_oneshot_role` (`:44`): change the no-explicit-override fallback from the
  hardcoded `claude:haiku` to the **weak tier** (`tiers.fast`), preserving (a)
  explicit `[models.<role>]` wins, and (b) loud-fail / `claude:haiku` fallback when
  the resolved weak model needs an OpenRouter key that is missing — never a silent
  drop of agency verdicts. After this lands, the setter's four explicit agency-role
  weak writes ([§4.1](#41-which-config-keys-each-tier-writes)) become optional.

**Starter template & docs**

- `src/profile/templates/pi.toml` — becomes the canonical two-tier source/migration
  target; drop the explicit DeepSeek pins on `evolver`/`creator`/`verification` so
  they ride `tiers.premium = strong` ([§8](#8-migration-of-the-current-hardcoded-choices)).
- `CLAUDE.md` / `AGENTS.md` (kept in lock-step) — document `wg profile pi` in
  "Service Configuration" and update the "Agency tasks run on claude CLI" section to
  reflect the weak-tier routing once `fix-route-agency` lands.
- `docs/config-ux-design.md` — note the two-tier facade alongside the existing
  profile UX.

---

## 11. Validation mapping

| design-two-tier validation item | where satisfied |
|---|---|
| Exact CLI accepting BOTH flag + positional, single-tier story for each | [§2](#2-the-cli-surface-the-exact-grammar), [§2.1](#21-grammar-rules-precise) |
| Every set/scout echoes resulting strong/weak; exact output format | [§3](#3-the-always-on-echo-the-say-what-we-are-doing-requirement), [§7](#7-the-scout-wg-profile-pi---scout) |
| Concrete examples: set both (flags+positional), set one, show, scout-applies | [§9](#9-concrete-examples-all-required-cases) |
| Explicit routing table chat/worker/.flip/.assign/eval → strong vs weak | [§4](#4-routing-table-tier--role) |
| Names existing code/config to change (no implementation) | [§10](#10-existing-code--config-that-must-change-not-implemented-here) |
| Resolves generic-vs-pi-scoped and persistence/reload | [§5](#5-generic-vs-pi-scoped-resolved), [§6](#6-persistence--reload) |
| Partial updates without clobbering the other tier | [§2.1](#21-grammar-rules-precise) (flags + `-` placeholder) |
| Legibility = scout dry-run is a copy-pasteable apply | [§3.4](#34-dry-run-preview-that-is-itself-a-copy-pasteable-apply), [§7](#7-the-scout-wg-profile-pi---scout) |
| Extensibility: future middle tier must not break invocations | [§12](#12-extensibility-a-future-middle-tier) |

---

## 12. Extensibility: a future middle tier

The grammar is forward-compatible with a third tier (e.g. `mid` for standard-only
work, splitting strong back into premium vs standard):

- **Flags extend cleanly:** add `--mid <spec>`; existing `--strong`/`--weak`
  invocations are unaffected.
- **Positional stays at two by default;** a third positional would only be consumed
  when `--mid` is also implied — but the safer path is to keep positional as the
  two-tier shorthand and require the flag for the third tier. Either way, **every
  existing `wg profile pi A B` and `wg profile pi --strong/--weak` invocation keeps
  working unchanged.**
- Because storage is the generic three-tier system ([§5](#5-generic-vs-pi-scoped-resolved)),
  a `mid` tier is just "stop collapsing standard into strong" — `mid → tiers.standard`,
  `strong → tiers.premium + work keys` — no new schema.
- The `-` skip placeholder generalizes to any arity (`wg profile pi A - C`).

This is why the 2-coloring framing matters: the two-tier verb is a *view* over a
system that already has the headroom for more tiers.
