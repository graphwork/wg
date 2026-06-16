# Design: `wg publish --profile` — per-WCC profile propagation

**Task:** `design-wg-publish` · **Status:** design only (no production code) ·
**Downstream:** `implement-wg-publish`

## 1. Goal

Let an operator pin a **named profile** (`claude` / `codex` / `nex` / …) to a
*subgraph* at publish time so that every task in that subgraph's
**weakly-connected component (WCC)** — both the work tasks *and* their agency
satellites (`.assign-*`, `.evaluate-*`, `.flip-*`) — dispatches through that
profile's `(executor, model, endpoint)` routing, while the rest of the graph
keeps running on whatever the globally-active profile selects.

Concrete motivation: burn Anthropic/OpenAI credits on a chosen subgraph while
everything else stays on OpenRouter — *without* flipping the global profile and
disturbing in-flight work.

Today profile selection is **global** (`wg profile use <name>` overwrites
`~/.wg/config.toml`; see `src/profile/named.rs::apply_profile_as_global_config`).
There is no per-task or per-subgraph routing override.

---

## 2. How routing is resolved **today** (ground truth)

### 2.1 The single dispatch-time resolution point

**`workgraph::dispatch::plan_spawn`** in `src/dispatch/plan.rs:298` is the *one*
function that decides `{executor, model, endpoint}` for any spawn. Its module
doc is explicit: "Every spawn site calls it; nobody else decides." Callers:
`coordinator.rs:4120`, `coordinator_agent.rs:464/910`, `ipc.rs:728`,
`spawn/execution.rs:93`, `spawn_task.rs:227`.

Signature:

```rust
pub fn plan_spawn(
    task: &Task,
    config: &Config,
    agent_executor: Option<&str>,   // agency-derived effective executor, or None
    default_model: Option<&str>,    // dispatcher's already-cascaded TaskAgent model
) -> Result<SpawnPlan>
```

**Model precedence** (`plan.rs:311-322`):

1. `task.model` → source `"task.model"`
2. `default_model` (passed by caller) → `"dispatcher.default_model"`
3. `config.coordinator.model`
4. `config.agent.model` (fallback)

**Executor precedence** (`resolve_executor`, `plan.rs:596`): `task.exec`/shell →
`task.exec_mode` → `agent_executor` → `config.coordinator.executor` → `Claude`
default; then `enforce_model_compat` (claude→native if model is non-Anthropic)
and the executor-qualified-route override.

**Endpoint** (`plan.rs:409`) is executor-scoped: `None` for
claude/codex/shell/external CLIs; for `native`/`nex` it cascades
`task.endpoint` → openrouter-for-openrouter-model → `[llm_endpoints] is_default`.

> **Key:** `plan_spawn` already supports per-task overrides via `task.model` /
> `task.endpoint` / `task.exec_mode`, but it has **no concept of a profile**. It
> only sees one `&Config`. So a per-WCC profile must reach `plan_spawn` either
> as (a) a different `&Config` snapshot, or (b) pre-resolved `default_model` +
> `agent_executor` + a stamped `task.endpoint`.

### 2.2 The dispatcher tick that feeds `plan_spawn`

`spawn_agents_for_ready_tasks` (`coordinator.rs:3919`) is called once per tick
with a single `default_model` computed from the **global** config
(`coordinator.rs:4655`):

```rust
let effective_model = model.map(String::from).unwrap_or_else(|| {
    config.resolve_model_for_role(DispatchRole::TaskAgent).spawn_model_spec()
});
```

Inside the ready-task loop (`coordinator.rs:4093-4124`):

- `.assign-*` work-path tasks: `task_model = resolve_model_for_role(Assigner)`.
- everything else: `task_model = default_model` (the global TaskAgent model).
- `agent_executor = agent_entity.explicit_executor()` (agency abstains for
  default agents).
- then `plan_spawn(task, config, agent_executor, task_model)`.

`config` here is the **one** global `Config` for the whole tick. This is the
seam where a per-task effective config must be injected.

### 2.3 How agency tasks derive their model today

Agency satellites are **scaffolded at publish/resume time**, not at dispatch.
`resume.rs::scaffold_eval_for_unpaused` (`resume.rs:464`) →
`eval_scaffold::scaffold_full_pipeline_batch` (`eval_scaffold.rs:333`). Each
satellite **bakes** its model into `task.model` at creation:

- `.evaluate-*`: `model: Some(resolve_model_for_role(Evaluator).model)`,
  `provider: …` (`eval_scaffold.rs:514-529`).
- `.flip-*`: `resolve_model_for_role(Evaluator)` (`eval_scaffold.rs:111/244/295`).
- `.assign-*`: baked likewise (`scaffold_assign*`).

Per `CLAUDE.md`, these roles are pinned to `claude:haiku` on the claude CLI and
**ignore** the `coordinator.model` provider cascade — `resolve_model_for_role`
resolves them via their own `[models.evaluator]`/`[models.assigner]` (or tier),
not via `coordinator.model`.

At **dispatch**, `.evaluate-*` / `.flip-*` run **inline** (`coordinator.rs:4025`
`is_inline_task`) — the daemon forks `wg evaluate run <id> --model <task.model>`
(`spawn_eval_inline`, `coordinator.rs:3105`; model = the baked `task.model`).
`.assign-*` runs inline too. So the agency model is whatever got baked into
`task.model` at scaffold time, plus a dispatch fallback of
`resolve_model_for_role(Assigner)` for assign.

**Consequence for this design:** to make a WCC profile infect agency tasks, the
cleanest lever is to resolve the agency role model **through the profile's
config** at *scaffold time* (so the right model is baked into `task.model`), and
to stamp the profile on the satellite so a dispatch-time backstop can re-resolve
if needed.

### 2.4 `wg publish` and `--wcc` already exist

`wg publish <id>` is **not** the rsync/HTML deploy command — that is
`wg html publish …` (`src/commands/publish.rs`). `wg publish <id>` lives in
`src/commands/resume.rs::publish` (`cli.rs:726`, `main.rs:1232`) and **already
ships a `--wcc` flag**:

```
Publish { id, only, wcc }      // cli.rs:727-735
```

`publish(dir, id, only, wcc)` → `run_inner(.., Mode::Wcc, is_publish=true)`
(`resume.rs:23`). In WCC mode it:

1. `discover_wcc(graph, seed)` — BFS over `after`+`before` as **undirected**
   edges, ignoring edges to non-existent nodes (`resume.rs:236`).
2. `validate_subgraph` (dangling deps, unconfigured cycles) (`resume.rs:383`).
3. `topo_sort_subset` then `unpause_task` each member (`paused = false`)
   (`resume.rs:120-128`).
4. `scaffold_eval_for_unpaused` — builds the agency pipeline for every
   newly-unpaused member (`resume.rs:131`).

This is the natural and intended home for `--profile`: the WCC walk, validation,
atomic graph write, and agency scaffolding are **already here**. The feature is
"stamp + bake during this same atomic pass."

---

## 3. Design decisions

### 3.1 Storage: per-task field vs WCC-level annotation

| Option | How | Pros | Cons |
|---|---|---|---|
| **A. Per-task `profile` field** | `Task.profile: Option<String>`, stamped on every WCC member at publish | Survives in `graph.json`; trivial dispatch lookup (`task.profile`); per-task provenance; reuses existing per-task override machinery | "Later-added tasks" need an explicit propagation hook |
| **B. WCC-level annotation, resolved at dispatch** | Store `{component_key → profile}`; recompute WCC per dispatch and look up | New tasks inherit automatically by joining the component | WCC has **no stable identity** (components merge/split as edges change); ambiguous when two profiles touch one component; O(V+E) per ready task per tick; nowhere natural to persist a "component" |

**Recommendation: Option A as the stored unit, with a propagation rule that
gives Option B's "auto-inherit" behavior.** A per-task field is the only thing
that maps cleanly onto WG's storage (`graph.json` is a flat task list — there is
no component object) and onto the existing per-task override path in
`plan_spawn`. We recover B's ergonomics with the propagation rules in §3.2,
*not* by inventing an unstable component key.

**Data model** (additive, backward-compatible — `Option`, `skip_serializing_if`):

```rust
// src/graph.rs, struct Task
/// Named profile pinned to this task's subgraph. Resolved at dispatch into a
/// per-task effective Config snapshot. None => use the globally-active profile.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub profile: Option<String>,
```

A profile **name** (not an inlined config) is stored. Routing is re-resolved
from the live profile file at dispatch, so editing `~/.wg/profiles/<name>.toml`
updates behavior without re-stamping the graph — matching how
`active_profile_model()` already reads the profile lazily.

### 3.2 Propagation: how the profile "infects" the WCC, including later-added tasks

Three complementary mechanisms; (1)+(2) are the primary contract, (3) is the
correctness backstop.

**(1) Eager stamp at publish (the WCC you can see now).**
`wg publish <seed> --profile <name> --wcc` walks `discover_wcc` and, in the same
`modify_graph` transaction that unpauses, sets `task.profile = Some(name)` on
**every** member. Provenance: a log entry per task. This is cheap, atomic, and
inspectable (`wg show <id>` shows the profile).

**(2) Inheritance-on-attach (tasks added later).**
The "infect later-added tasks" semantic is satisfied at **edge-creation time**:
when a new task is linked into a component that already carries a profile, it
inherits. Concretely, add a helper

```rust
// src/commands/add.rs (and wherever edges are created)
fn inherit_profile_from_neighbors(graph: &WorkGraph, after: &[String], before: &[String]) -> Option<String>
```

that, for a new task's `--after`/`--before` targets, returns the profile of any
adjacent task (deterministic tie-break: see §5). Call sites:

- `wg add … --after X` / `--before Y` (`src/commands/add.rs`): if the new task
  has no explicit `--profile` and a neighbor is profiled, stamp the neighbor's
  profile.
- `wg link` / `wg edit` edge edits.
- **Agent decomposition** — when a worker runs `wg add 'subtask' --after
  $WG_TASK_ID`, the subtask inherits the parent's profile through the exact same
  `wg add` path. This is how a profiled subgraph keeps its routing as agents
  grow it (`The Graph is Alive`).

**(3) Dispatch-time WCC-consistency backstop (optional, behind a flag).**
For edges created by paths that bypass (2), the dispatcher can, when a ready
task has `profile == None`, check whether any member of its WCC is profiled and
adopt it (memoized once per tick via `compute_cycle_analysis`-style caching).
This guarantees the invariant "a WCC has at most one effective profile" even if
a stamp was missed, at the cost of one WCC walk per unstamped ready task.
Recommend shipping (1)+(2) first; add (3) only if drift is observed.

> Net effect: the profile is **sticky to the component**. Publish stamps the
> current members; new members inherit as they attach; the backstop heals gaps.

### 3.3 Dispatch-time injection point (the cleanest seam)

The whole point of `plan_spawn` being the single resolver is that we should
**not** scatter profile logic across spawn sites. Inject **one** function that
turns the global config into a per-task effective config, and call it just
before `plan_spawn`:

```rust
// src/dispatch/profile.rs  (new)
/// Return the effective Config for a task: if task.profile is set, load that
/// named profile snapshot; else return the global config unchanged. Cheap,
/// memoized per (profile-name) within a tick.
pub fn effective_config_for_task<'a>(task: &Task, global: &'a Config, cache: &mut ProfileCache)
    -> Cow<'a, Config>
```

In the dispatcher loop (`coordinator.rs:4093-4124`), replace the single shared
`config`/`default_model` with per-task values:

```rust
let eff = effective_config_for_task(task, config, &mut profile_cache);
let task_model = if task.id.starts_with(".assign-") {
    Some(eff.resolve_model_for_role(DispatchRole::Assigner).spawn_model_spec())
} else if eff_has_profile {
    Some(eff.resolve_model_for_role(DispatchRole::TaskAgent).spawn_model_spec())
} else {
    default_model.map(String::from)   // unchanged global path
};
let plan = plan_spawn(task, &eff, agent_executor, task_model.as_deref())?;
```

Because a profile file is a *complete* `Config` snapshot (`src/profile/named.rs`
doc: "profiles are no longer overlays … a complete Config snapshot"), swapping
`&eff` for `&config` transparently carries the profile's `coordinator.executor`,
`coordinator.model`, `[models.*]` role pins, **and** `[llm_endpoints]` — so
`plan_spawn`'s existing executor/model/endpoint cascade just works against the
profile. No change to `plan_spawn`'s body is required; the change is "pick which
`Config` to hand it."

This same `effective_config_for_task` is reused at the other `plan_spawn`
callers that can spawn profiled tasks (`ipc.rs`, `spawn/execution.rs`,
`spawn_task.rs`) so direct/IPC spawns honor the profile too.

`SpawnProvenance.model_source` / `endpoint_source` (`plan.rs:239`) get a
`"profile:<name>"` annotation so the one-line spawn log explains *why* a task
routed to a non-global model — preserving the "silent routing is impossible"
guarantee.

### 3.4 Agency-task inheritance (`.assign-*` / `.evaluate-*` / `.flip-*`)

Two coordinated changes:

**Bake-time (primary):** thread the parent work task's effective config into the
scaffolders so the satellite's `task.model`/`provider` is resolved from the
**profile's** `[models.evaluator]`/`[models.assigner]` rather than the global
config. In `resume.rs::scaffold_eval_for_unpaused` (`resume.rs:470`) the config
is currently `Config::load_or_default(dir)`; instead resolve each candidate's
profile (it was just stamped in step (1)) and load that snapshot, passing it
into `scaffold_full_pipeline_batch`/`scaffold_eval_task`/`scaffold_flip_task`/
`scaffold_assign_task`. Also stamp `satellite.profile = parent.profile` so the
satellite is itself a profiled WCC member.

This means: under a WCC pinned to `claude`, eval/flip/assign resolve
`claude:haiku` (the profile's evaluator pin) on the claude CLI exactly as today;
under a WCC pinned to `nex`, they resolve the **nex profile's** evaluator
role/tier and run through the nex endpoint — i.e., the WCC profile overrides the
"pinned to claude:haiku" default *for that component only*. The override is the
profile's own `[models.evaluator]`; explicit per-role pins inside the profile
still win (matching the existing "explicit overrides win, cascade does not"
rule in `CLAUDE.md`).

**Dispatch-time (backstop):** `spawn_eval_inline` is invoked with
`task.model.as_deref()` (`coordinator.rs:4032`). Since the satellite carries the
baked profile model *and* `task.profile`, the inline-eval branch can re-resolve
`evaluator_model` from `effective_config_for_task` if `task.profile` is set,
covering satellites that were scaffolded before the parent was (re)profiled.

> Why bake-time is primary: agency satellites are created during the *same*
> atomic publish pass that stamps the WCC, so the profile is known at scaffold
> time — baking avoids any dispatch-time profile lookup for the common path and
> keeps the inline `wg evaluate run --model …` invocation correct.

### 3.5 Backward compatibility

- `Task.profile` is `Option`, `#[serde(default, skip_serializing_if = …)]`:
  existing `graph.json` rows deserialize with `profile = None`. No migration.
- `task.profile == None` ⇒ `effective_config_for_task` returns the global config
  **unchanged** ⇒ identical to today's behavior (global active profile). Every
  existing `plan_spawn` test (`plan.rs:634-1547`) stays green because they
  construct tasks with `profile = None` via `Task::default()`.
- `wg publish <id>` / `--wcc` / `--only` with **no** `--profile` behave exactly
  as today (no stamping).
- Unknown/missing profile name ⇒ hard error at publish (validate against
  `~/.wg/profiles/` + `STARTER_NAMES`) so a typo never silently falls back to
  global. Selecting it is a one-time publish-time check, not a dispatch-time
  surprise.

### 3.6 CLI surface

Extend the **existing** `wg publish` (and its resume sibling), not the rsync
`wg html publish`:

```
wg publish <id> --profile <name>            # implies --wcc: pin the whole
                                            #   weakly-connected component
wg publish <id> --profile <name> --wcc      # explicit, same as above
wg publish <id> --profile <name> --only     # pin + release ONLY this task
wg publish <id> --profile <name> --no-release  # stamp profile WITHOUT unpausing
                                            #   (annotate a staged subgraph)
```

- `--profile <name>` defaults to **WCC scope** (the user's stated intent:
  "propagates to the ENTIRE WCC"). `--only` narrows it to the seed.
- Validate `<name>` exists; error listing available profiles otherwise.
- `--profile` is also accepted on the routes that *create* subgraphs so you can
  pin at authoring time, propagating via §3.2(2):
  - `wg add <title> --profile <name>` (and inherited by `--after`/`--before`
    children).
- Management / introspection:
  - `wg show <id>` prints `Profile: <name>` when set.
  - `wg profile pin <id> [--wcc|--only] <name>` and `wg profile unpin <id>
    [--wcc]` — adjust an already-published component without re-publishing
    (re-stamps members; clears to `None` = revert to global).
  - `wg html` / viz: badge profiled tasks so a pinned subgraph is visible.

Flag wiring: add `profile: Option<String>` and `no_release: bool` to the
`Commands::Publish` variant (`cli.rs:727`); thread through
`commands::resume::publish` (`main.rs:1233`) into `run_inner`.

---

## 4. Implementation sketch (file-by-file, for `implement-wg-publish`)

1. **`src/graph.rs`** — add `Task.profile: Option<String>` (additive, defaulted).
2. **`src/cli.rs:727`** — add `--profile <name>`, `--no-release` to `Publish`;
   add `--profile` to `Add`; new `profile pin/unpin` subcommands.
3. **`src/commands/resume.rs`** — `publish(dir, id, only, wcc, profile,
   no_release)`; in WCC/subgraph/only arms, stamp `task.profile` on members in
   the same `modify_graph` txn; validate the profile name; thread the resolved
   profile into `scaffold_eval_for_unpaused`.
4. **`src/dispatch/profile.rs`** (new) — `effective_config_for_task` +
   `ProfileCache` (memoize name→Config per tick). Loads via
   `profile::named::load(name)?.config`.
5. **`src/commands/service/coordinator.rs:4093`** — call
   `effective_config_for_task` per ready task; pass `&eff` to `plan_spawn` and
   resolve `task_model` from `eff`; in the inline-eval branch re-resolve model
   from `eff` when `task.profile.is_some()`.
6. **`src/commands/eval_scaffold.rs`** — accept an effective `&Config` (or a
   resolved profile) so satellites bake the profile's role model; stamp
   `satellite.profile = parent.profile`.
7. **`src/dispatch/plan.rs`** — only provenance strings change
   (`"profile:<name>"`); the resolution body is untouched.
8. **`src/commands/add.rs`** — `inherit_profile_from_neighbors` for §3.2(2).
9. Tests: extend `resume.rs` WCC tests to assert `profile` stamped on all
   members + satellites; `plan.rs` test that a profiled task routes via the
   profile's executor/model/endpoint; an add-inherits-profile test.

No change to the `plan_spawn` contract or to global-profile behavior.

---

## 5. Edge cases & conflict rules

- **Two profiles meet in one WCC** (publish profile=A on a component that an
  inherited profile=B already touched, or two stamped subgraphs later get joined
  by an edge): define a deterministic resolution. Recommended: **last explicit
  publish/pin wins** for the members it names; for inheritance-on-attach,
  tie-break by (a) seed/explicit annotations over inherited, then (b)
  lexicographically smallest profile name, and **`log()` a warning** naming both
  profiles so the operator notices. Never silently pick at dispatch.
- **Cycles**: `discover_wcc` already treats edges undirected, so cyclic
  components are one WCC and get one profile; `validate_subgraph` still enforces
  `--max-iterations`.
- **Federation / dangling refs**: `discover_wcc` ignores edges to non-existent
  nodes, so a profile never leaks across a federation boundary (remote refs are
  not local tasks).
- **`paused` interaction**: stamping is orthogonal to unpausing. `--no-release`
  stamps without unpausing (annotate-then-publish-later workflows).
- **Profile deleted after stamping**: dispatch falls back to global config and
  the spawn-log provenance records `profile:<name> (missing → global)`; surface
  via `wg doctor`/lint rather than failing the spawn.
- **claude↔codex backend mismatch**: a WCC profile that selects a `codex:` model
  for a task already pinned to a `claude:` model is caught by the existing
  `validate_cli_backend_match` (`plan.rs:561`) — fails loudly before launch, as
  intended.

---

## 6. Validation checklist (task acceptance criteria → where addressed)

- [x] **Exact dispatch-time profile-resolution point (file/function)** —
  `dispatch::plan_spawn` (`src/dispatch/plan.rs:298`); fed by
  `coordinator.rs:4093-4124`. Injection seam =
  `effective_config_for_task` immediately before `plan_spawn` (§2.1, §3.3).
- [x] **How profile metadata is stored & propagated across the WCC, incl.
  later-added tasks** — per-task `Task.profile` (§3.1); eager WCC stamp at
  publish + inheritance-on-attach + dispatch backstop (§3.2).
- [x] **Agency-task inheritance** — bake profile role model at scaffold via the
  profile's `[models.evaluator]`/`[models.assigner]` + stamp `satellite.profile`
  + inline-eval dispatch backstop (§3.4).
- [x] **CLI surface (flag names + behavior)** — `wg publish <id> --profile
  <name>` (WCC-default), `--wcc`/`--only`/`--no-release`, `wg add --profile`,
  `wg profile pin/unpin` (§3.6).
- [x] **Backward compatibility (no profile → active profile)** — `Option` field,
  `None` ⇒ unchanged global path; no migration (§3.5).
- [x] **Design recorded as a task artifact/log** — this file
  (`docs/design-wg-publish-profile-wcc.md`), registered via `wg artifact`.

---

## 7. Open questions for `implement-wg-publish`

1. Ship the dispatch-time WCC backstop (§3.2-3) in v1, or rely on
   stamp+inherit and add the backstop only if drift appears? (Recommend: defer.)
2. Should `wg profile unpin --wcc` walk the live WCC (which may have grown) or
   only the originally-stamped members? (Recommend: live WCC, for symmetry with
   publish.)
3. Surface a per-WCC profile in the TUI status line, or only in `wg show` /
   `wg html`? (Cosmetic; out of scope for the routing change.)
