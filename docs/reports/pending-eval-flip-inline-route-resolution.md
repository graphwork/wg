# PendingEval / FLIP inline-route stall: independent resolution design

**Task:** `study-pendingeval-flip`

**Date:** 2026-07-17

**Tree studied:** `fab8f297c7c51e165ea38a0b029cd748219c2e3f`

**Upstream incident intake:** `80bcbfabec53f0ad5fe4bc3f11c9cfe27dd86716`

**External evidence:** `spinozans/emender@1e6bb993c26a32eadddcc4fcd437c999fb9d2cde`

## Decision

The reported Codex failure is independently reproduced and deterministic. `eval_scaffold` resolves a route, decomposes it into `Task.model` plus `Task.provider`, and `spawn_eval_inline` later receives only `Task.model`. For `codex:gpt-5.4-mini` this becomes bare `gpt-5.4-mini`; for `nex:openrouter:z-ai/glm-5.2` it becomes bare `z-ai/glm-5.2`. The inline preflight correctly rejects both bare strings under the landed explicit-execution-system contract. The parent then remains `PendingEval` while the generic five-failure spawn breaker eventually leaves FLIP `Incomplete`.

This is not an SCC or edge cycle. The graph is the intended acyclic chain:

```text
parent -> .flip-parent -> .evaluate-parent
```

Dispatcher readiness already gives system satellites a narrow `PendingEval | FailedPendingEval` bypass. `wg why-blocked` does not use that predicate and therefore falsely calls the soft-done parent the root blocker.

A slow filesystem does **not** create or repair the invalid route. It delays the tick and atomic rename, lengthens lock occupancy and visibility lag, and increases the overlap window in which two coordinators can make duplicate **pre-claim scheduling attempts**. It does not create a semantic evaluator run in this reproducer. The repair must therefore address route identity and lifecycle idempotency independently of storage latency.

The implementation should not merely concatenate `provider:model` at this one call site. It should add one persisted, stage-aware `AgencyDispatchPlan`, use it at scaffold and invocation, add a verdict/pipeline identity fence, and reconcile historical rows with a compare-and-set transaction. No code should infer a provider, change handler/provider, invent a verdict, or manually mark hidden tasks done.

## 1. Independent reproduction

### 1.1 Isolation

All graph writes below were under `/tmp/wg-study-*`. No production graph, active profile, daemon, or lifecycle row was changed. Each process was started with `env -i`, a fresh `HOME`, fresh XDG paths, and an explicit neutral `WG_USER=study`. Thus no inherited `WG_DIR`, project/worktree identity, active profile, API key, model, handler, endpoint, or launcher environment survived. The clean environment printed only the deliberately supplied `WG_USER=study` among `WG_*` variables.

The installed binary was `/home/bot/.cargo/bin/wg` (`wg 0.1.0`). The source behavior was cross-checked against the tree named above.

### 1.2 Normal-storage Codex timeline

The essential disposable flow was:

```bash
scratch=$(mktemp -d /tmp/wg-study-normal-codex.XXXXXX)
mkdir -p "$scratch/home" && cd "$scratch" && git init -q
clean=(env -i HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/home/.config" \
  XDG_CACHE_HOME="$scratch/home/.cache" \
  PATH="/home/bot/.cargo/bin:/home/bot/.nvm/versions/node/v25.4.0/bin:/usr/local/bin:/usr/bin:/bin" \
  USER=study LOGNAME=study TERM=dumb WG_USER=study)

"${clean[@]}" wg init --route codex-cli
"${clean[@]}" wg config --auto-assign false --auto-evaluate true \
  --flip-enabled true --no-reload
"${clean[@]}" wg add 'Study parent codex' --id study-parent-codex \
  --no-place -d $'## Validation\n- [ ] lifecycle reaches Done'
"${clean[@]}" wg pause study-parent-codex
"${clean[@]}" wg service tick       # keep parent from being spawned
"${clean[@]}" wg resume study-parent-codex  # eagerly scaffolds the chain
"${clean[@]}" wg claim study-parent-codex
"${clean[@]}" wg done study-parent-codex
"${clean[@]}" wg ready
"${clean[@]}" wg why-blocked .flip-study-parent-codex
"${clean[@]}" wg service tick
```

Observed timeline:

1. Resume scaffolded `.flip-study-parent-codex` and `.evaluate-study-parent-codex`.
2. `wg done` changed the parent to `pending-eval` because the evaluator existed and was nonterminal.
3. Both hidden rows serialized `"model":"gpt-5.4-mini","provider":"codex","exec_mode":"bare"`.
4. `wg ready` listed `.flip-study-parent-codex`.
5. `wg why-blocked .flip-study-parent-codex` instead printed `study-parent-codex (status: PendingEval) <-- ROOT CAUSE`.
6. The next tick selected the FLIP and printed:

   ```text
   Spawning eval inline ... (model: gpt-5.4-mini)
   Failed to spawn eval ... invalid invocation-scoped evaluator route "gpt-5.4-mini"
   Tick complete: 0 alive, 1 ready, 0 spawned
   ```

7. FLIP stayed `Open`, `spawn_failures` became 1, and no `.wg/agents/agent-*` directory existed. The failure was before artifact creation and claim.

Five observed failed selections (three sequential harness steps plus two deliberately overlapping delayed ticks) produced exactly the existing breaker result: FLIP `Incomplete`, `spawn_failures=5`, no agent directory, no operation, and no verdict. This matches the production report's five ordinary daemon attempts, although the independent fifth attempt was accelerated by the overlap test described below.

### 1.3 Non-Codex and Pi controls

The same isolated flow was run after explicitly setting the evaluator/FLIP roles and `tiers.fast`.

| Configured role route | Scaffolded FLIP row | Inline result | Finding |
|---|---|---|---|
| `codex:gpt-5.4-mini` | `model=gpt-5.4-mini`, `provider=codex` | rejected before claim | reproduced loss |
| `nex:openrouter:z-ai/glm-5.2` | `model=z-ai/glm-5.2`, `provider=openrouter` | rejected before claim | reproduced loss; not Codex-specific |
| `pi:openai-codex:gpt-5.6-luna` | `model=pi:openai-codex:gpt-5.6-luna`, no separate provider | route preflight and claim succeeded | useful control: the current parser happens to retain this full Pi string |

The Pi control is not evidence that the representation is sound. It shows that behavior depends on whether `resolve_model_for_role` decomposes a particular dialect. A `wg init --route pi` control also exposed stale generated role strings (`openrouter:deepseek/deepseek-chat`): those scaffolded as bare `deepseek/deepseek-chat` plus `provider=openrouter` and failed identically. More importantly, `provider=openrouter` cannot tell a historical reconciler whether the original handler was `pi` or `nex`. Guessing either would violate the explicit-system decision.

The qualified Pi process was allowed only through the preflight/claim observation and had already exited when cleanup checked it. No result from it is used as semantic evidence.

### 1.4 Source-level root cause

The source matches the observations:

- `src/commands/eval_scaffold.rs:202-216` resolves the evaluator and stores `flip_resolved.model` and `flip_resolved.provider` separately. The evaluator row repeats this at `:252-263`. Other scaffold helpers have the same shape.
- `ResolvedModel::spawn_model_spec` already documents the need to reattach provider for spawn planning (`src/config.rs:2105-2133`), but the scaffold does not call it.
- The inline dispatcher extracts only `task.model` (`src/commands/service/coordinator.rs:4261-4264`) and passes that to `spawn_eval_inline` (`:4304`).
- `spawn_eval_inline` validates that string alone with `execution_system_key` (`:3251-3279`). Route validation is intentionally before registry lock, artifacts, and claim (`:3282-3311`), explaining zero agents.
- The ordinary worker path later in the same function uses `effective_config_for_task` and `plan_spawn` as its declared single source of truth (`:4328-4380`); inline agency tasks bypass it.
- The wrapper currently executes the stored `wg evaluate ...` command without a route argument. `wg evaluate run` then resolves its own role route again unless `--evaluator-model` is supplied (`src/commands/evaluate.rs:281-285, 424-489`). Consequently, fixing only registry metadata would still allow the route actually called to differ from the route claimed by the inline task.
- FLIP has two calls (`FlipInference` and `FlipComparison`) and can configure them differently. One `Task.model` is therefore insufficient even if it is handler-qualified.

The current `park_agency_execution_error` only recognizes diagnostics containing `error[WG-EXEC-...]`. The string `invalid invocation-scoped evaluator route ...` does not carry that code, so this deterministic representation bug falls through to the generic spawn breaker (`coordinator.rs:4310-4320`).

## 2. Slow/pathological filesystem findings

### 2.1 Blocked graph lock

With FLIP at `spawn_failures=1`, a separate `flock -x .wg/graph.lock` holder slept for two seconds while a clean-env tick ran.

```text
before_spawn_failures=1
lock_acquired_ms=1784278736522
tick_start_ms=1784278736667
lock_releasing_ms=1784278738524
... invalid invocation-scoped evaluator route "gpt-5.4-mini"
tick_end_ms=1784278738552
elapsed_ms=2039
after_spawn_failures=2
agent_dir_count=0
```

The tick waited for the exclusive writer lock and then produced the same pure route error. There was one failure increment and no claim. A blocked lock contributes latency and extends `PendingEval`; it does not reject the route.

### 2.2 Delayed fsync and visibility

A test-only `LD_PRELOAD` shim delayed every `fsync` in the tick by 1500 ms. The graph began at failure count 2:

```text
tick_start_ms=1784278753914
mid_ms=1784278754316 mid_spawn_failures=2
... invalid invocation-scoped evaluator route "gpt-5.4-mini"
tick_end_ms=1784278756949 elapsed_ms=3035
after_spawn_failures=3
agent_dir_count=0
```

This is exactly the contract in `src/parser.rs:202-218,225-267`: readers use a nonblocking shared lock and see either the old file or the renamed new file; writers fsync a temporary file and rename atomically. The mid-tick reader saw the old count, never a partial JSONL file. Storage delay therefore contributes bounded visibility lag and longer exclusive-lock occupancy.

### 2.3 Overlapping ticks and duplicate scheduling attempts

Two manual ticks were then launched concurrently with a 750 ms fsync delay. Both had captured an `Open` ready snapshot before either failure update became visible. Each printed the same pre-claim route rejection. The final state was:

```text
before=open 3
both ticks: 0 spawned
after=incomplete 5
route_failure_logs=5
agent_dir_count=0
```

This identifies the precise duplicate mechanism:

- slow storage by itself did not duplicate an attempt;
- two overlapping coordinators/ticks plus stale-but-consistent snapshots did;
- `record_spawn_failure` increments under a graph lock but does not require the task still to match the snapshot generation/status that was selected;
- both attempts were scheduling/preflight attempts, not semantic LLM runs.

A normal single daemon calls ticks serially, so overlap requires a second daemon, concurrent manual tick, or equivalent competing coordinator. Slow storage widens that overlap window. It can also reduce reconciliation cadence because maintenance and scaffolding are serialized behind graph writes. It does not explain the original invalid route, and no NFS premise is needed for the production sequence.

### 2.4 Contribution matrix

| Symptom | Deterministic route loss | Slow/blocked storage | Competing ticks |
|---|---:|---:|---:|
| bare route rejected | **yes** | no | no |
| graph/registry lock duration | no | **yes** | contention |
| old-state visibility until rename | no | **yes, consistent snapshot** | observes different snapshots |
| slower reconciliation cadence | no | **yes** | possible contention |
| duplicate pre-claim attempts | enables repeated selection | widens window only | **yes** |
| duplicate semantic verdict | not in this reproducer | not by itself | possible only after claim/result bugs; must be fenced |
| parent remains PendingEval | **yes** | lengthens duration | can reach breaker faster in attempt count |

There is no evidence that broken-pipe log noise caused this incident. Route validation failed before a child PID; the one-shot tick reproduces it without long-lived daemon IPC.

## 3. State-machine invariants

The implementation must make these invariants executable, not comments:

1. **Structural acyclicity:** `parent -> FLIP -> evaluator` remains an ordinary acyclic `after` chain. SCC/cycle code must not special-case it.
2. **Soft-state dependency semantics:** `PendingEval` and `FailedPendingEval` block ordinary dependents but satisfy only their own dot-prefixed evaluation satellites.
3. **One pipeline per source attempt:** every source execution attempt has one stable `pipeline_id`; at most one FLIP task and one evaluator task belong to it.
4. **One explicit route plan:** every LLM call has a complete handler-first route, optional named endpoint, reasoning, execution-system key, role/stage, and provenance. Provider is never inferred at invocation.
5. **Scaffold/invocation agreement:** the plan persisted by scaffold is the plan used by the child call. Registry metadata, diagnostics, and actual handler cannot re-resolve independently.
6. **Same-system only:** fallbacks, if explicitly declared, must retain `(handler, provider/wire)`. Route failure never switches handler/provider and never borrows an ambient default.
7. **Transport is not a verdict:** preflight, auth, endpoint, spawn, timeout, or wrapper failure records no semantic score and cannot promote/fail the source as if an evaluator judged it.
8. **Durable-before-terminal:** an evaluation satellite may be semantically completed only after a valid durable verdict for its `pipeline_id`, source attempt, and stage exists. An exhausted route remains visibly execution-blocked; it is not a fake evaluation failure.
9. **At-most-once consumption:** the parent stores the exact consumed verdict ID/CID in the same graph transaction as its status transition. A verdict cannot resolve two source attempts or be consumed twice.
10. **No scoreless promotion:** `PendingEval` cannot become `Done` merely because `.evaluate-X` is missing or terminal. Current `resolve_pending_eval_tasks` permits both and must be tightened.
11. **Bounded automatic work:** route attempts, format retries, and semantic attempts have distinct bounded counters and backoff. No status transition silently resets a counter for the same pipeline.
12. **Repair is monotonic:** reconciliation may add missing identity, normalize a provably equivalent route, link a durable verdict, or advance a state once. It never deletes a verdict or turns a consumed stage back to `Open`.

Recommended state sketch:

```text
source attempt N
  running
    -> PendingEval        (worker called done)
    -> FailedPendingEval  (worker exited, evaluation may rescue)

pipeline N
  RoutePending -> FlipReady -> FlipClaimed -> FlipVerdictDurable -> FlipConsumed
               -> EvalReady -> EvalClaimed -> EvalVerdictDurable -> ParentConsumed

Any pre-verdict execution failure:
  stage -> ExecutionWaiting(attempt, next_at)
        -> same explicit plan, bounded retry
        -> ExecutionBlocked(needs operator/config generation change)

ParentConsumed outcome (single graph CAS):
  PendingEval       + accepted/gate policy -> Done or Failed/Incomplete
  FailedPendingEval + score threshold      -> Done(rescued) or Failed/Incomplete
```

`ExecutionWaiting/Blocked` may initially be represented by existing `Waiting` plus a typed lifecycle record, but not by generic `Incomplete` (“needs evaluator review”), which recursively asks evaluation to evaluate its own unavailable evaluator.

## 4. Canonical agency dispatch representation and planner

### 4.1 Data model

A single model string cannot describe two-stage FLIP. Persist a stage-aware plan on the hidden task (with backward-compatible `#[serde(default)]`):

```rust
#[derive(Serialize, Deserialize, Clone)]
struct AgencyDispatchPlan {
    schema: u16,
    pipeline_id: String,
    source_task: String,
    source_attempt: u32,
    task_id: String,
    calls: Vec<AgencyCallPlan>,
    plan_hash: String,
}

struct AgencyCallPlan {
    stage: AgencyStage, // Assign, FlipInference, FlipComparison, Evaluate
    route: String,      // canonical handler-first raw spec
    endpoint: Option<String>, // named ref, not ambient default
    reasoning: Option<ReasoningLevel>,
    system: ExecutionSystemKey, // persisted audit value, re-derived and checked
    source: DispatchSelectionSource,
    fallbacks: Vec<String>, // already validated same-system, ordered
}
```

`Task.model/provider` remain compatibility/display fields during migration, but agency execution must use `agency_dispatch`. New scaffolds may mirror the primary route into `Task.model`; they must not decompose it. A small first patch should at least replace `resolved.model` with the full canonical route (`spawn_model_spec` or, preferably, the new planner output) in every scaffold site.

The parent also needs a compact lifecycle record:

```rust
struct EvaluationLifecycle {
    schema: u16,
    pipeline_id: String,
    source_attempt: u32,
    route_generation: String,
    schedule_attempts: u32,
    semantic_attempts: u32,
    linked_flip_verdict: Option<String>,
    linked_eval_verdict: Option<String>,
    consumed_verdict: Option<String>,
    repair_version: u16,
}
```

Evaluation files should add `pipeline_id`, `source_attempt`, `stage`, `producer_run_id`, and a content digest. Legacy fields remain readable.

### 4.2 One planner

Add `dispatch::plan_agency_task(task, effective_config, historical_policy) -> Result<AgencyDispatchPlan>`. It is the only function allowed to choose routes for scaffold, inline claim, restart, retry, and repair.

Precedence:

1. A valid persisted `agency_dispatch` plan for this exact task/pipeline. Re-derive every system key and validate its hash.
2. A handler-first historical `Task.model`, converted to a plan without changing handler/provider.
3. A legacy split row only through a **lossless** migration rule:
   - `provider=codex` plus model can map to `codex:<model>`;
   - an effective profile/role route may be used only when its decomposed model/provider/endpoint exactly matches the stored fields and its provenance is explicit;
   - ambiguous `provider=openrouter` without handler does not map to Pi or Nex. It becomes `NeedsRouteRepair`.
4. A task with no historical route evidence may resolve the applicable stage roles from the task's effective explicit profile/config. This is a one-time scaffold/repair decision, persisted before claim.
5. Otherwise return a stable `WG-EXEC-AGENCY-ROUTE-AMBIGUOUS/UNSELECTED` error and park visibly.

There is no provider default and no installed-handler fallback.

For a new `.flip-X`, the planner resolves both `FlipInference` and `FlipComparison`; for `.evaluate-X`, `Evaluator`; for `.assign-X`, `Assigner`. This also fixes the current scaffold's use of `Evaluator` for the FLIP task.

### 4.3 Invocation

Change `spawn_eval_inline(dir, task_id, Option<&str>)` to accept/use the persisted plan, not a model argument copied from a stale graph snapshot. Preflight occurs before artifacts, then a graph CAS claims `(task_id, pipeline_id, plan_hash, status=Open|due Waiting)` and creates a unique `run_id`.

The child must receive the plan identity, for example:

```text
wg evaluate run X --agency-task .flip-X --plan-hash <hash>
```

`evaluate run` reloads `.flip-X`, verifies the hash/pipeline, and uses each planned stage route via `run_lightweight_llm_call_for_route`. Do not re-resolve current config. Manual `wg evaluate run X --evaluator-model ...` remains a separate invocation-scoped path and does not silently consume a scaffolded pipeline.

Restart and retry read the same persisted plan. Changing configuration alone does not mutate an in-flight plan; an explicit repair/replan command records the old/new plan and increments `route_generation`.

## 5. Historical migration and reconciliation

### 5.1 Command and daemon primitive

Implement one library primitive used by both startup/tick and a visible operator command:

```text
wg repair eval-lifecycle [TASK] --dry-run
wg repair eval-lifecycle [TASK] --apply
wg repair eval-lifecycle TASK --apply --verdict <ID>  # only for an ambiguity shown by dry-run
```

The daemon may auto-apply only unambiguous cases. Dry-run emits old/new route, evidence, verdict IDs, counters, and the exact transition. It never calls `wg done` on a hidden task.

### 5.2 Algorithm

1. **Read immutable evidence.** Under an evaluation-store lock, enumerate verdict files, validate JSON and content digest, and index by source, stage, loop iteration, pipeline metadata, timestamp, and ID. Do not rewrite them.
2. **Enter one graph `modify_graph` transaction.** Re-read fresh rows and compare the expected statuses/generation from the analysis. If they changed, abort and recompute.
3. **Identify the chain.** Group `X`, `.flip-X`, and `.evaluate-X`. Assign a deterministic legacy `pipeline_id` from graph identity + source ID + source-attempt evidence; store it once.
4. **Classify route evidence.** Use the planner rules above. If ambiguous, set lifecycle health `NeedsRouteRepair`, preserve all statuses/verdicts, and stop automatic dispatch.
5. **Link verdicts conservatively.** New-format verdicts must match pipeline/attempt/stage exactly. A legacy verdict is linkable automatically only when exactly one valid candidate exists for that source/stage/loop and it is not older than a trustworthy source-attempt boundary. Missing/malformed timestamps or multiple candidates are not “pick newest”; they require `--verdict` operator selection, which records the choice.
6. **Repair pre-claim failures.** If a hidden stage is `Incomplete`/`Open`, has no claim/run/verdict, failed only route preflight, and has not consumed `repair_version`, persist the canonical plan, clear the generic spawn breaker once, set it `Open` (or due `Waiting`), and log `old route -> new route`. A second pass is a no-op.
7. **Preserve completed semantic work.** If a valid FLIP verdict exists, link it and terminalize/link the FLIP stage in the same transaction without rerunning FLIP. If a valid evaluator verdict exists, link it and terminalize/link the evaluator stage without rerunning it. “Terminalize” here is a typed lifecycle transition justified by the verdict ID, not manual hidden-task completion.
8. **Advance, never skip.** A linked FLIP verdict with no evaluator verdict opens only the evaluator. An evaluator verdict may be consumed only after all required prior stages are linked. Missing evaluator scaffold on a `PendingEval`/`FailedPendingEval` source is recreated from the canonical plan; the source is never promoted scorelessly.
9. **Consume parent verdict by CAS.** Require matching source status, pipeline/attempt, linked evaluator verdict, `consumed_verdict=None`, and configured gate policy. Atomically write `consumed_verdict=<ID>`, outcome, parent status, counters, and log. A conflicting existing consumed ID is quarantined as corruption.
10. **Registry cleanup after graph commit.** Mark stale wrapper entries terminal/obsolete by `run_id`. Registry failure may leave observability stale but cannot repeat semantic work because the graph/verdict CAS is authoritative.

### 5.3 State-specific behavior

| Historical state | No durable verdict | Durable FLIP only | Durable evaluator verdict |
|---|---|---|---|
| hidden stage `Incomplete`, pre-claim route failure | normalize/replan once and reopen boundedly | link FLIP; do not rerun | link required stages and consume once |
| parent `PendingEval` | scaffold/repair and wait; never auto-Done | open evaluator | consume according to normal gate |
| parent `FailedPendingEval` | scaffold/repair and wait within bounded execution budget | open evaluator | consume score once: rescue/retry/fail policy |
| source `Incomplete` before worker claim | leave source retry policy alone; no evaluator verdict exists | flag inconsistent evidence | quarantine unless explicitly associated with a real source attempt |

### 5.4 Crash boundaries and idempotency proof

| Crash point | Restart behavior |
|---|---|
| before plan/scaffold graph rename | no plan exists; deterministic scaffold creates one |
| after scaffold, before parent claim | same plan/IDs are reused; no duplicate nodes |
| after parent becomes PendingEval/FailedPendingEval | system readiness exposes only first incomplete stage |
| after stage claim, before model call | run lease expires; bounded execution retry uses same plan |
| after model result, before verdict rename | no durable verdict; bounded retry permitted |
| after verdict rename, before stage graph transition | reconciler links verdict and does not call model again |
| after FLIP linked, before evaluator claim | evaluator becomes the only ready stage |
| after evaluator verdict rename, before evaluator transition | reconciler links it |
| after evaluator linked, before parent transition | parent CAS consumes verdict |
| after parent graph rename, before registry update | `consumed_verdict` and terminal outcome already coexist; registry cleanup only |

At-most-once follows from three keys and two CAS conditions:

- verdict creation is create-if-absent by `(pipeline_id, source_attempt, stage, semantic_attempt)`; the same digest is idempotent and a different digest conflicts;
- stage linking requires `linked_*_verdict=None` or the same ID;
- parent transition requires `consumed_verdict=None` and writes the ID in the same atomic graph rename as the outcome.

A crash can therefore cause zero progress or a replayed no-op, not a second semantic consumption. Merely logging “exactly once” or relying on the parent becoming terminal, as the agent-385 WIP does, is not a proof and is insufficient for historical ambiguity.

## 6. Readiness, why-blocked, and lifecycle health

Export one dependent-aware explanation function, not another boolean:

```rust
enum DependencyDisposition {
    Satisfied,
    EvalSystemBypass { blocker_status: Status },
    Blocked { reason: BlockReason },
}
```

`ready_tasks*`, `wg why-blocked`, `wg done` blocker checks, and lifecycle diagnostics must consume it. The current correct semantics are split between private `blocker_satisfied_for_dependent` and public `is_blocker_satisfied_with_eval_gate` (`src/query.rs:339-374,423-445`). `why_blocked` instead includes every nonterminal local blocker and tests readiness with plain `is_blocker_satisfied` (`src/commands/why_blocked.rs:104-145,169-176`).

Expected output for the reproduced FLIP is not “root cause”:

```text
.flip-X is dispatcher-ready
  X: PendingEval — evaluation-system bypass (this satellite is part of X's gate)
Lifecycle health: route invalid (stored bare model gpt-5.4-mini; recorded provider codex)
```

For `.evaluate-X` behind `Incomplete` FLIP, `why-blocked` should still show FLIP as the immediate blocker, while lifecycle health names the causal invalid route and exhausted schedule attempts.

Add `wg check`/`wg status --json` lifecycle findings such as:

- `eval-route-ambiguous`
- `eval-stage-preclaim-incomplete`
- `eval-verdict-unlinked`
- `eval-parent-awaiting-verdict`
- `eval-execution-retry-exhausted`
- `eval-consumption-conflict`

`wg cycles` remains unchanged. It should continue to report no cycle for this chain; lifecycle health is not SCC analysis.

## 7. Bounded failure and retry policy

The >100-agent storm involved a second defect: readiness lets a FLIP run over `FailedPendingEval`, while `wg done` currently bypasses only `PendingEval` (`src/commands/done.rs:1456-1461`). A successful child could persist a verdict and then fail to become terminal, after which orphan reconciliation reopened it. Route loss and that completion asymmetry compose badly but are distinct.

Use separate budgets:

1. **Legacy route repair:** at most once per row/schema version. Ambiguity never retries automatically.
2. **Schedule/preflight attempts:** default 2 per `route_generation`, exponential timers (for example 1m, 5m), then `ExecutionBlocked`. A config/plan hash change or explicit operator replan starts a new generation. These attempts do not consume semantic evaluation budget.
3. **Claimed transport attempts:** bounded by the same execution budget and guarded by `run_id`; no verdict on failure.
4. **Format attempts:** the existing up-to-three JSON extraction loop is one claimed semantic run policy and must be visible. It may retry exact/same-system routes only as explicitly configured.
5. **Semantic verdict attempts:** one accepted verdict per stage key. Any policy allowing a second judge must mint a new semantic-attempt key and cap it explicitly.
6. **Source rescue attempts:** keep separate from evaluator availability and from verify-failure counters.

Every automatic transition must use expected `(status, pipeline_id, plan_hash, run_id, counter)` CAS. In particular, invalid-route parking must atomically change `Open -> Waiting/ExecutionBlocked` and increment once. A second tick holding an old snapshot then fails its CAS rather than incrementing again. Valid routes already have a claim CAS; invalid routes need the equivalent reservation.

Do not send `WG-EXEC-*` wrapper failures through ordinary `wg fail` as agent-385 proposed. The accepted explicit-system design requires agency work to remain retryable/blocked without a synthetic semantic verdict. Also do not leave a one-minute `Waiting` loop unbounded. Typed bounded execution state satisfies both requirements.

With one pipeline, one active run lease per stage, two execution attempts per route generation, and verdict reconciliation before reopening, agent count is bounded and cannot grow past the configured attempts, let alone 100.

## 8. Audit of prior candidate work

### 8.1 Luca `9a595528`

**Retain:**

- The core one-line policy is correct and still applicable: system-child completion must bypass both `PendingEval` and `FailedPendingEval`, matching readiness.
- Retain focused positive and negative tests: FLIP can complete over `FailedPendingEval`; an ordinary dependent remains blocked.
- Land this as a surgical adaptation to current `done.rs`, not as evidence the whole incident is fixed.

**Hidden dependencies:**

- `query.rs` already treats both soft states as satisfied for system children.
- `evaluate run` already accepts both states.
- The candidate assumes a child actually reached semantic success; it does not establish durable verdict identity.

**Does not solve:** route decomposition, pre-claim `Incomplete`, historical rows, missing/duplicate verdicts, parent at-most-once consumption, route retry limits, or diagnostics. Cherry-picking it alone leaves the reported Codex stall intact.

### 8.2 Failed agent-385 worktree

The worktree is uncommitted and spans 13 files (about 678 insertions/101 deletions in tracked files plus a new lifecycle helper and smoke). It must not be merged wholesale.

**Ideas worth retaining, rewritten behind the canonical model:**

- detect a durable stage result before reopening an orphan;
- fence verdicts to a source attempt and stage;
- rearm the existing hidden nodes rather than creating duplicates;
- expose attempt counters and run repeated ticks in a permanent smoke;
- add the `FailedPendingEval` system-child completion bypass;
- centralize parent outcome rather than allowing `check_eval_gate` and the dispatcher to race.

**Rejected or unsafe behavior:**

- Route failures are changed from `Waiting` to `wg fail`. That violates “transport is not a verdict” and the accepted explicit-system contract.
- `evaluation_is_from_current_attempt` accepts every legacy verdict when `started_at` is absent and otherwise uses only time/loop/source. It has no pipeline/semantic key and can attach a stale or ambiguous verdict.
- Parent logs say “consumed exactly once,” but no consumed verdict ID is persisted. Terminal status makes the happy path appear idempotent but does not prove crash-safe or historical at-most-once consumption.
- The WIP directly marks an orphaned satellite `Done` from the loose match. The desired repair must link a validated verdict ID in the same transaction and surface ambiguity.
- Low-score source retry is coupled to `coordinator.max_verify_failures`, an unrelated budget.
- It replays broad fields, including whole logs, from a stale triage graph into a fresh graph (`triage.rs` local WIP), risking clobber of concurrent changes.
- It does not repair the newly reported scaffold/inline route loss at all.
- The smoke manually rewrites JSONL without the graph lock and demonstrates selected cases, but not restart at each write boundary or pre/post-verdict failure.

**Minimal safe salvage slices:**

1. Adapt only the Luca-style `done.rs` predicate + focused tests.
2. Reuse the *test idea* “durable verdict before wrapper death means no rerun,” after adding pipeline/verdict IDs and CAS.
3. Reuse the *test shape* of four post-terminal ticks with no registry/dispatch growth.
4. Do not copy the route-failure-to-`wg fail`, stale triage replay, legacy timestamp fallback, or unrelated rescue-budget changes.

## 9. Concrete source patch map

| File/module | Patch |
|---|---|
| `src/config.rs` | Preserve canonical handler-first raw route in resolution; stop requiring consumers to reconstruct handler from `provider`. Add serializable stage route and plan validation. Keep `ExecutionSystemKey` authoritative. |
| `src/service/llm.rs` | Promote/extend `AgencyDispatch` into the canonical call-plan type; expose same-system fallback validation/attempting without re-resolution. |
| `src/commands/eval_scaffold.rs` | Call the agency planner once; persist complete stage-aware plan for `.flip-*`/`.evaluate-*`; use full route in compatibility `Task.model`; assign correct FLIP stages. Idempotency compares pipeline/plan identity. |
| `src/commands/service/coordinator.rs` | Replace `spawn_eval_inline(..., task.model)` with plan lookup/preflight + claim CAS. Pass plan identity to child. Add bounded typed execution parking, expected-generation CAS, reconciler, and verdict-required parent consumption. Remove scoreless/missing-eval promotion. |
| `src/commands/evaluate.rs` | Internal scaffold invocation reads/verifies the persisted plan and uses exact routes per stage; writes keyed durable verdict before graph completion. Centralize parent outcome in reconciler. Manual invocation remains separate. |
| `src/graph.rs` | Add backward-compatible `agency_dispatch` and `evaluation_lifecycle` records (or equivalent typed fields), route generation, run/pipeline IDs, linked/consumed verdict IDs, and distinct counters. |
| `src/query.rs` | Export one `DependencyDisposition` used by readiness, diagnostics, and done blocker filtering. |
| `src/commands/why_blocked.rs` | Build the tree using dependent-aware disposition; show bypass vs actual blocker and lifecycle-health cause. |
| `src/commands/done.rs` | Surgical `PendingEval | FailedPendingEval` system bypass (Luca slice); require the shared disposition. Hidden completion path must carry verdict linkage when invoked by reconciler. |
| `src/commands/sweep.rs`, `service/triage.rs` | Reconcile verdict first under fresh graph CAS; never reopen a stage with a linked durable verdict; do not replay stale whole task/log snapshots. |
| new `src/commands/eval_lifecycle.rs` (name acceptable) | Own pipeline IDs, verdict indexing/validation, migration classification, CAS transition decisions, health findings, and dry-run report. |
| `src/commands/retry.rs`, `edit.rs`, recovery CLI | Replan/reset only explicitly, increment route generation, preserve verdicts, and refuse a retry that would duplicate linked semantic work. |
| `tests/...`, smoke manifest | Add matrix below; manifest entry is grow-only. |

A minimal emergency route patch (`model: Some(resolved.spawn_model_spec())` plus passing the exact route to `evaluate run`) can stop new Codex/Nex rows, but it is not the historical or at-most-once repair and should not be represented as complete.

## 10. Permanent validation matrix

### Unit

- Canonical route round-trip for `codex:gpt-5.4-mini`, `pi:openai-codex:gpt-5.6-luna`, `pi:openrouter:z-ai/glm-5.2`, `nex:openrouter:z-ai/glm-5.2`, and `claude:haiku`.
- FLIP plan contains distinct inference/comparison calls and preserves endpoint/reasoning/provenance.
- Scaffold, inline invocation, restart, retry, and repair return the same plan hash.
- Bare `model + provider` migration succeeds only for lossless/explicitly corroborated cases; ambiguous OpenRouter handler fails closed.
- Same-system fallback accepts model changes within one key and rejects handler/provider changes before any call.
- Concurrent invalid-route park CAS increments once; stale second tick is a no-op.
- Verdict create-if-absent: same digest idempotent, conflicting digest quarantined.
- Link and parent-consume CAS are idempotent and store the exact verdict ID.
- Legacy zero/one/multiple verdict classification; missing timestamps do not silently select newest.
- `PendingEval` with missing/terminal evaluator but no verdict does not promote.
- System bypass covers both soft states; ordinary dependent negative remains.
- `why-blocked` and ready query share the same dispositions.

### Integration (credential-free fake handlers)

For each of Codex, Pi, and handler-qualified Nex/OpenRouter:

- clean `env -i`/fresh `HOME` graph;
- scaffold full chain;
- assert persisted full plan and actual fake-handler invocation agree;
- parent reaches `PendingEval -> FLIP verdict -> evaluator verdict -> Done` once;
- fake handler invocation counter is one per semantic stage;
- no hard-coded Claude/Pi/Nex fallback appears.

Historical fixtures cover pre-claim hidden `Incomplete`, parent `PendingEval`, and parent `FailedPendingEval`, each with zero verdict, FLIP-only verdict, evaluator verdict, conflicting verdicts, stale attempt verdict, and already-consumed verdict.

### Restart/failure boundary matrix

Kill/restart after each boundary below and assert the final verdict IDs, handler counter, agent count, and parent outcome:

1. before scaffold save;
2. after scaffold save, before parent claim;
3. after parent completion, before FLIP claim;
4. after FLIP claim, before model call;
5. after FLIP result, before verdict rename;
6. after FLIP verdict rename, before FLIP graph transition;
7. after FLIP transition, before evaluator claim;
8. after evaluator claim, before result;
9. after evaluator result, before verdict rename;
10. after evaluator verdict rename, before evaluator transition;
11. after evaluator transition, before parent consumption;
12. after parent consumption, before registry cleanup.

This explicitly covers pre-verdict failure (retry allowed within execution budget) and post-verdict failure (no semantic rerun).

### Storage/concurrency integration

- held `graph.lock` delays but does not change route/outcome;
- delayed fsync exposes old/new snapshots only;
- two overlapping ticks select one invalid-route scheduling attempt by generation CAS;
- delayed verdict rename plus restart never creates two verdicts;
- long lock does not invert registry/graph lock order or lose a graph update.

### Smoke

Add one real-daemon, fake-handler scenario (for example `pending_eval_inline_route_recovery.sh`) owned by the implementation integrator. It should run all three handler classes, one historical pre-claim repair, one post-verdict wrapper crash, multiple extra ticks, and an overlapping delayed-tick probe. Assert:

- parent terminal outcome is correct;
- hidden chain has one task per stage;
- exact verdict IDs are linked/consumed once;
- no agent/dispatch growth after terminal state;
- retries are bounded and visible;
- `why-blocked` agrees with `wg ready`;
- `wg cycles` still says no structural cycle.

## 11. Safe operator recovery for stranded graphs

### Before the reconciler ships

There is no fully safe current command for every stranded shape. In particular, retrying a hidden task after it already wrote a verdict can duplicate semantic work, while manually running `wg done .flip-X/.evaluate-X` fabricates completion without a verdict. Do neither blindly.

For a confirmed **zero-run, zero-verdict, pre-claim route failure** only:

1. Stop competing coordinators and copy `.wg/graph.jsonl`, `.wg/agency/evaluations/`, and `.wg/service/registry.json`.
2. Run `wg config lint` and explicitly select same-system handler-first routes.
3. Verify the hidden task has zero agent runs/operations and no matching evaluation file.
4. Repair **both** hidden routes explicitly, e.g.:

   ```bash
   wg edit .flip-X     --model codex:gpt-5.4-mini
   wg edit .evaluate-X --model codex:gpt-5.4-mini
   ```

   Use the actual selected Pi/Nex route instead when applicable. `wg edit` currently clears `spawn_failures` when a field changes (`src/commands/edit.rs:676-677`).
5. Resume one coordinator and observe FLIP, evaluator, then parent. Do not manually mark hidden tasks done.

If any FLIP/evaluator verdict already exists, if an agent was claimed, if multiple verdicts exist, or if the handler is ambiguous (for example stored `provider=openrouter`), keep the service stopped/affected stages paused and wait for/use the verdict-aware repair primitive. Direct `wg evaluate run`, `wg retry`, and hidden `wg done` are not atomic chain repair.

### After the reconciler ships

Use:

```bash
wg repair eval-lifecycle X --dry-run
wg repair eval-lifecycle X --apply
wg service tick
wg check
wg why-blocked .flip-X
```

An ambiguity must name candidate verdicts/routes and require explicit selection; it must never choose a different system. The apply output must name the persisted plan hash, linked verdict IDs, consumed verdict ID (if any), counter budget, and every state transition.

## 12. Dependency-aware implementation breakdown

To minimize same-file conflicts, use this sequence rather than parallel edits to coordinator/graph:

1. **Luca safety slice:** adapt the `FailedPendingEval` completion bypass and focused tests. Independent, small prerequisite.
2. **Canonical route/plan types:** `config.rs` + `service/llm.rs`; unit tests for all handlers and fallback boundaries.
3. **Persisted pipeline/verdict schema:** `graph.rs` + evaluation serialization; create-if-absent verdict tests. Depends on plan types.
4. **Scaffold and exact invocation:** `eval_scaffold.rs`, `evaluate.rs`, then coordinator inline path. Depends on 2-3. Emergency new-row fix lands here.
5. **CAS reconciler/migration:** new lifecycle module plus coordinator maintenance and repair CLI. Depends on 3-4.
6. **Bounded execution lease/retry:** coordinator, wrapper, retry/edit/recovery; depends on 4-5 so it uses real plan/pipeline generations.
7. **Orphan/triage integration:** sweep and triage consume the reconciler API; depends on 5-6.
8. **Readiness/diagnostics:** query disposition, why-blocked, check/status. Can begin after invariant API is fixed, but integrate after 5.
9. **Integration and smoke join:** restart matrix, delayed-storage/overlap harness, three-handler fake calls, historical fixtures, and no-growth ticks. Depends on all implementation slices.
10. **Operator docs and migration rollout:** document dry-run/apply, backup, ambiguity handling, and rollback after smoke passes.

Each implementation task must name its file scope and a failing test first. The final integrator owns source files touched by multiple sequential slices and the grow-only smoke manifest entry.

## 13. Disposition of report commit `80bcbfab`

Land the commit as a documentation-only incident intake, not as a lifecycle patch. Its concise diagnosis is accurate for Codex, its external links are commit-pinned, and it changes only `docs/bugs/pending-eval-flip-inline-route-20260717.md`. It should either be cherry-picked after verifying the file is still absent or recreated verbatim in the integration branch, then amended by a **separate** cross-link to this resolution report.

Do not treat its short proposed reconciler (“normalize from current configuration/profile, reset once”) as the full implementation contract: current config can be a different system, OpenRouter provider metadata is handler-ambiguous, and existing verdicts require pipeline/consumption fencing. The report is valuable provenance; this document is the implementation-resolution authority.

## Conclusion

The incident is two deterministic lifecycle defects plus a diagnostic mismatch:

1. inline scaffold loses the handler for routes that decompose into model/provider;
2. successful rescue satellites can be blocked from completion over `FailedPendingEval`, enabling verdict-producing respawn storms;
3. `why-blocked` ignores the dispatcher-only soft-state bypass.

Storage latency is an amplifier, not the route cause. The safe repair is one explicit stage-aware plan, one pipeline/verdict identity, atomic at-most-once consumption, bounded typed execution retries, and one shared dependency explanation. That design repairs Codex without hard-coding Codex, preserves Pi/Nex system boundaries, recovers historical work without fake `done` operations, and prevents another unbounded agent storm.
