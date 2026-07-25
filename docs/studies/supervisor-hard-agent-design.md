# Study: a long-lived "supervisor" hard-agent for graph health and auto-reset

**Status:** Proposed design (study deliverable, not an accepted implementation plan).
**Date:** 2026-07-25
**Owner task:** `study-long-lived`
**Tags:** research, design, daemon, lifecycle

## 0. TL;DR

The WG daemon already has **three** reactive health mechanisms that fire inside
the coordinator tick: the dead-agent reaper (`triage::cleanup_dead_agents`),
the orphan-task sweep (`sweep::reconcile_orphaned_tasks`), and the eval-lifecycle
reconciler (`eval_lifecycle::reconcile_durable_verdicts`, the `auto_rescue_on_eval_fail`
path). Each one fixes *exactly one* class of problem, the moment it is detected,
and each forgets what it did the moment the tick ends.

This study proposes a **fourth, slower, stateful** layer: a long-lived
"supervisor" hard-agent — a Casa-style persistent persona (see
`docs/design-casa-wgfed-adapter.md`, §3 *persistent agent/persona*) rather than an
ephemeral per-task worker — that wakes on a *slow* tick (≈ every 2–5 minutes,
deliberately slower than the coordinator's 5 s `poll_interval`), scans the graph
for tasks that are stuck **for dumb reasons**, and resets/requeues them. Its
distinguishing property is **persistent memory**: it remembers which tasks it
already touched, how many times, and with what result, so it neither flaps
(reset→die→reset) nor double-acts with the existing reaper.

The supervisor is **not** a replacement for the reaper/sweep/reconciler. It owns
only the gap they leave open:

- tasks that are *terminal-again-revertible* (`Failed`, `FailedPendingEval`) that
  the reactive layer cannot reopen because it has no policy for "this failed for a
  dumb reason N minutes ago, but the upstream cause is now gone"; and
- tasks that are *stuck non-terminal* (`PendingEval`/`Open` with a dead or absent
  eval satellite, retry storms that out-ran the respawn throttle, intentional-crash
  test tasks) that the reactive layer keeps re-arming into an unbounded loop.

The whole thing ships behind a **dry-run / audit-first** rollout with a hard
kill switch, a bounded blast radius (one task per supervisor action by default,
and a global per-tick cap), and explicit liveness handling for the supervisor's
*own* failure mode.

---

## 1. Why the existing layer is not enough

The reactive layer is event-coupled to the coordinator tick
(`src/commands/service/coordinator.rs::coordinator_tick`, ~line 4793; the daemon
loop that calls it is `src/commands/service/mod.rs` ~lines 2633–3300, gated by
`daemon_cfg.poll_interval`, default 5 s — `src/config.rs::default_poll_interval`).
On every tick it runs, in order:

1. `triage::cleanup_dead_agents(dir, graph_path)` (`src/commands/service/triage.rs:233`)
   — detects dead PIDs past `reaper_grace_seconds` (default 30), unclaims
   `InProgress` tasks, optionally LLM-triages them (`auto_triage`), escalates the
   model, and re-arms a fresh eval source-attempt via
   `eval_lifecycle::begin_source_attempt`.
2. `sweep::reconcile_orphaned_tasks` (`src/commands/sweep.rs:57`, invoked at
   `coordinator.rs:74`) — recovers `InProgress` tasks whose agent is gone/missing.
3. `eval_lifecycle::reconcile_durable_verdicts(... auto_rescue_on_eval_fail ...)`
   (`src/eval_lifecycle.rs:2089`, invoked at `coordinator.rs:4908`) — consumes
   durable eval verdicts; on a hard reject of a `PendingEval` source, with
   `auto_rescue=true` and `rescue_count < max_rescues`, resets the source to
   `Open` (`eval_lifecycle.rs:2240–2290`).

Each is correct for its trigger. The gaps, all confirmed in the live graph during
this session, are:

### 1.1 The failed-pending-eval tar pit (worked example: `deduplicate-config-deprecation`)

A task exits without `wg done`, enters `FailedPendingEval`; the eval satellite
scores it below threshold (`0.63 < 0.70`), so `reconcile_durable_verdicts` moves
it to terminal `Failed` (`eval_lifecycle.rs:2251–2261`). It then **sits there
forever** — the reactive layer never reopens a terminal `Failed` task. The only
way out is a human running `wg recover` (the log literally records
`Reset by \`wg recover\` — reason: mass failure ...`).

The supervisor owns: *"a task that is `Failed` for a reversible/dumb reason and
has been quiet for ≥ N minutes; reopen it once, bounded by a memory of prior
attempts."*

### 1.2 The respawn storm (worked example: `fix-low-score-eval-gate`, `retry_count = 6`)

`check_respawn_throttle` (`src/commands/service/coordinator.rs:~3990`,
`RESPAWN_MAX_RAPID = 5`, `RESPAWN_WINDOW_SECS = 300`) counts **death log entries**
in a rolling 5-minute window and fails the task at 5 deaths. But the counter is
derived from log scraping, not from `retry_count`, and a storm that is spread
across model escalations / pauses / manual resets can re-arm past it: the worked
example shows `retry_count = 6` with six consecutive "process exited →
`begin_source_attempt`" cycles (attempts 4–7) and the task is *still `Open`*.

The supervisor owns: *"a task whose recent log shows a repeating die→reset
pattern that the throttle's log-window has aged out of; detect it from
`retry_count` + the supervisor's own memory, not from a 5-minute log window, and
either back off, escalate to human, or stop re-arming."*

### 1.3 The intentional-crash test task (worked example: `storm-source`)

`storm-source` is a deliberate crash task ("storm source crashes on purpose").
Every spawn dies immediately, the reaper unclaims it and re-arms eval, the
coordinator respawns it, forever (`retry_count = 1`, `max_retries = None`). No
existing mechanism recognises "this task always crashes the same way within
seconds of starting; it is *meant* to, or the failure is structural."

The supervisor owns: *"a task with a short, recurring, identical-shape crash
signature; leave it alone (or fail it loudly for a human) instead of re-arming."*

### 1.4 Eval-satellite debris / orphan satellites

When a source task is abandoned/failed/archived out from under its `.evaluate-*`
/ `.flip-*` satellites, those satellites can be left `Open` with no parent to
drive them — visible in this graph as a long tail of `.evaluate-*` / `.flip-*`
tasks in `Open`. The reactive layer has no "sweep stranded satellites" pass.

The supervisor owns: *"stranded agency satellites whose source is terminal and
which themselves have no durable verdict; abandon them."*

---

## 2. Wake cadence

**Decision: hybrid, tick-dominant, with an optional urgent-kick.**

| Mode | Trigger | Source | Cadence |
|---|---|---|---|
| **Tick (default)** | slow safety-net timer | new `supervisor.interval` config (default **180 s**) | every interval |
| **Urgent kick (opt-in, off by default)** | IPC `GraphChanged` storm detector | reuses the daemon's existing graph-watch pipe | debounced, ≥ `supervisor.min_interval` apart |
| **Manual** | `wg supervisor run [--dry-run]` | CLI | on demand |

### Rationale

- **The tick is deliberately slower than the coordinator's `poll_interval` (5 s).**
  The reaper/sweep/reconciler are *fast* reactive layers that already run every
  tick. The supervisor is a *slow* stateful layer. If it woke every 5 s it would
  either (a) race the reaper on the same dead PID, or (b) re-decide the same
  `Failed` task every tick. A 2–5×-slower cadence (default 180 s) keeps it out of
  the reaper's blast radius while still catching a tar pit well inside a human
  attention span.
- **Avoid the thundering herd / double-acting.** The supervisor must *never* act
  on a condition the reaper is about to fix. The single rule that makes this safe:
  **the supervisor only touches tasks the reactive layer has stopped touching.**
  Concretely it skips any task that is `InProgress`, or that has a log entry from
  `triage`/`eval-lifecycle-reconcile`/`sweep` newer than `supervisor.react_settle`
  (default 90 s). That 90 s window is ≥ 3 coordinator ticks, so "the reactive layer
  had its chance" is a safe assumption.
- **No event-driven mode by default.** Event-driven wake would re-introduce the
  exact race the reaper already owns. The optional urgent-kick exists only for the
  case where a burst of `GraphChanged` events (many tasks failing at once, e.g.
  credit exhaustion) justifies an early pass; it is gated behind
  `supervisor.urgent_kick` (default `false`) and a `min_interval` floor so it can
  never fire more than once per minute even when enabled.

### Where the timer lives

The supervisor reuses the **same daemon loop** (`src/commands/service/mod.rs`,
the `last_coordinator_tick.elapsed() >= daemon_cfg.poll_interval` block ~line
3138), adding a parallel `last_supervisor_pass` check at a longer interval. It is
*not* a separate process and *not* a separate OS thread — it runs inline on the
daemon's event thread, behind the same `graph.lock`, exactly like the coordinator
tick. This is the same pattern `cron::reset_cron_task`
(`src/cron.rs:167`) already uses: the daemon's tick is the single scheduler.

---

## 3. The dumb-failure inventory: per-class reset vs leave-for-human

For each class: **signal** (what the supervisor scans for), **policy** (reset /
back off / escalate / leave), and **the counter that bounds it**.

| # | Class | Signal | Policy | Bound / counter |
|---|---|---|---|---|
| **C1** | **failed-pending-eval tar pit** | `status == FailedPendingEval` **or** `Failed`, with `failure_reason` containing an eval-reject signature (`score=... < threshold`), quiet for ≥ `supervisor.eval_tar_pit_min` (default 10 min), and the failure is *not* a suppressable structural class (`ExecutorConfig`, `WrapperInternal`, `ApiError400Document`) | **Reset once** to `Open`, clear `failure_reason`, bump a new `supervisor_reset_count`. Model-escalate. On 2nd+ occurrence: **leave for human** (log + surface, no reset) | `supervisor_reset_count` per task (default cap 1 auto-reset, then escalate) |
| **C2** | **agent-exit-nonzero crash loop** (`AgentExitNonzero`, `AgentHardTimeout`) | task is `Open` or `Failed`, recent log shows ≥ K "process exited" within `supervisor.crash_window` (default 15 min), AND `supervisor` memory says it reset this same task within the window | **Back off**: do nothing this pass; if `supervisor_reset_count >= cap`, **escalate to human** (set a `needs_human` marker + surface in `wg status`). Never reset a crash loop more than once | `supervisor_reset_count` + supervisor memory (§4) |
| **C3** | **intentional / structural crash task** (`storm-source`) | crash signature is *identical* across ≥ 3 consecutive deaths (same `failure_reason`, death within `supervisor.instant_death_secs` of spawn, default 60 s), and `max_retries` is unset | **Leave for human**: stop re-arming; mark the task `needs_human`; *optionally* set a suggested `max_retries` in the log so the reaper's own throttle can catch it next time. Do **not** reset | one-time recognition, recorded in memory so it is not re-evaluated |
| **C4** | **dead-agent / no-agent orphan already in Open** | `status == Open`, `assigned == None`, no live or dead agent references it, quiet ≥ `supervisor.orphan_min` (default 15 min) | This is the sweep's job — **skip** unless the supervisor memory shows sweep has run and left it. If so, it is a *zombie Open* (e.g. a task whose agent was reaped but whose `Open` was never re-claimed because admission deferred it): leave for the coordinator, but record it as a health signal for the rate-limit controller (§7) | n/a (informational) |
| **C5** | **stranded agency satellites** (`.evaluate-*`, `.flip-*`, `.assign-*` whose source is `Done`/`Failed`/`Abandoned`/`Canceled`) | satellite is non-terminal, source is terminal, no durable verdict exists for the satellite's pipeline | **Abandon** the satellite (mirror `wg recover`'s `AbandonFollowup` path, `src/commands/recover.rs`). Terminal satellites are left alone | idempotent — abandoned satellites are terminal |
| **C6** | **PendingEval with a dead/failed eval satellite and no durable verdict** | source `PendingEval`/`FailedPendingEval`, its `.evaluate-*` is `Failed`/`Abandoned`/dead, no verdict file under `.wg/agency/eval-lifecycle/verdicts/`, `evaluation_lifecycle.repair_attempts >= MAX_PIPELINE_REPAIRS_PER_SOURCE_ATTEMPT` | **Reset the source to Open** once (re-arm via `begin_source_attempt`), bumping `meta_eval_attempts`. On 2nd occurrence escalate to human | `meta_eval_attempts` (already on `Task`, `src/graph.rs:~660`) |
| **C7** | **resource-exhausted (disk)** | `failure_class == ResourceExhaustedDisk` and disk is now below the sentinel watermark | Re-queue (`status = Open`) **only after** `disk_sentinel` reports headroom; this is already half-handled by the reaper's ENOSPC branch (`triage.rs:~330`) — supervisor only handles the case where the task landed terminal `Failed` because the reaper's in-place retry also hit ENOSPC | `supervisor_reset_count`, gated on live disk headroom |
| **C8** | **respawn storm that out-ran the throttle** | `retry_count >= RESPAWN_MAX_RAPID` *but* the throttle's 5-min log window has aged out (task still `Open`, still dying) | **Back off hard**: set a supervisor-enforced cooldown in memory so neither the supervisor nor (via a log marker) the coordinator re-spawn for `supervisor.storm_cooldown` (default 30 min). Escalate to human if it persists past the cooldown | supervisor memory cooldown + `retry_count` |

**Classes the supervisor explicitly does NOT own** (left to existing machinery):
the initial dead-PID reaping (C4's *detection* — the reaper), the eval-verdict
*consumption* (the reconciler), the per-spawn model escalation (the reaper's
`try_escalate_model`), and credit-exhaustion mass recovery (`wg recover`, which is
the human-initiated blast-reset the supervisor must not shadow).

---

## 4. Memory model

The supervisor's whole reason to exist is that it **remembers**. Three layers,
cheapest first:

### 4.1 Per-task memory: extend the graph row (authoritative)

Add two fields to `Task` (`src/graph.rs`, the counter block around line 640–720):

```rust
/// Number of times the supervisor hard-agent has reset this task.
/// Bounded by `supervisor.max_resets_per_task`; once exceeded the task
/// is escalated to human and no further auto-reset occurs.
#[serde(default, skip_serializing_if = "is_zero")]
pub supervisor_reset_count: u32,

/// RFC3339 timestamp of the last supervisor action on this task (reset,
/// back-off, or escalation). Used to enforce min-interval and dedupe.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub last_supervisor_action: Option<String>,
```

These ride the existing atomic graph transaction (`parser::modify_graph`), so they
are crash-safe and visible in `wg show` / the TUI with no new store. This is the
**dedupe + loop-prevention** substrate: the supervisor never resets a task whose
`last_supervisor_action` is newer than `supervisor.min_task_interval` (default
15 min), and never resets past `max_resets_per_task` (default 1 for C1/C6, 0 for
C2/C3/C8).

### 4.2 Cross-task / pattern memory: a sidecar journal (operational)

Some decisions need history the task row shouldn't carry (e.g. "I have seen this
*exact* crash signature on this task 3 times" for C3, or a per-task cooldown for
C8). That lives in a single append-only sidecar:

```
.wg/supervisor/journal.jsonl   # one JSON object per supervisor action
.wg/supervisor/state.json      # rolled-up mutable state (cooldowns, last_pass)
```

`state.json` is the **rollup** (current active cooldowns, `last_pass`,
`ticks_skipped`), written atomically the same way
`.wg/service/coordinator-state-N.json` is (`src/commands/service/mod.rs`). The
`journal.jsonl` is the **audit trail** — append-only, bounded by size, never the
source of truth for graph state (it is rebuildable, mirroring the Casa "read
model, not ledger" rule from `docs/design-casa-wgfed-adapter.md` §1/§7).

Each journal entry is a bounded record (no attacker/task text beyond a short
reason code, echoing the WG-Review bounded-category-code discipline):

```json
{"ts":"2026-07-25T14:11:00Z","task":"fix-low-score-eval-gate","class":"C2",
 "action":"backoff","reason":"crash-loop","attempts":6,"reset_count":1,
 "pass":42,"dry_run":false}
```

### 4.3 Recognition memory (C3): a content hash of the crash signature

For "is this the *same* crash as last time", the supervisor stores a short hash of
`(failure_class, failure_reason[:120], exit_code, died_within_secs_bucket)` in the
journal, keyed by task id. Three identical hashes ⇒ C3 (structural/intentional
crash). This is intentionally a *bucketed* hash, not the full reason, so a
deterministic crash is recognised even if a timestamp or PID varies.

### Why not a graph task?

The task description asks "graph task? sidecar file?". The answer is **both, split
by concern**: the *per-task* memory goes on the graph row (so it is atomic with
the reset and visible everywhere); the *supervisor's own operational* memory
(cooldowns, journal, signature hashes) goes in the sidecar (so it never pollutes
the task graph, never blocks `modify_graph`, and is trivially wipeable for a
clean reset). A `.supervisor-*` graph task was considered and rejected: it would
make the supervisor a participant in the very graph it is repairing (circular —
what resets the supervisor's own stuck task?), and it would force eval-lifecycle
onto an internal bookkeeping row.

---

## 5. Loop prevention

Three independent bounds, all must hold before any reset:

1. **Per-task reset cap.** `supervisor_reset_count < max_resets_per_task` (the cap
   is per-class: 1 for tar pits, 0 for crash loops). Exceeding it ⇒ escalate.
2. **Per-task min-interval.** `now - last_supervisor_action >= min_task_interval`
   (default 15 min). Prevents two supervisor passes from both resetting the same
   task within a flap.
3. **Global per-pass cap.** `supervisor.max_actions_per_pass` (default **3**).
   Even if 200 tasks look resettable, the supervisor reopens at most 3 per pass.
   This is the blast-radius bound: a bug in the supervisor can corrupt at most 3
   tasks per 3 minutes, and a `wg recover` can always undo it.

Escalation (when any bound is hit) is **loud but non-destructive**: set
`needs_human` (surfaced in `wg status` / `wg bottlenecks`), append a journal
entry, and stop. It never auto-fails a task the human might still want.

---

## 6. Boundary vs the existing reactive layer

| Concern | Owner | Supervisor's role |
|---|---|---|
| Dead PID detection + unclaim | **reaper** (`triage::cleanup_dead_agents`) | none — supervisor skips `InProgress` and anything touched by triage < `react_settle` ago |
| Orphan (`InProgress`, no agent) recovery | **sweep** (`sweep::reconcile_orphaned_tasks`) | none for the active case; informational only if sweep has run and left a zombie `Open` |
| Eval verdict consumption + auto-rescue of `PendingEval` | **reconciler** (`eval_lifecycle::reconcile_durable_verdicts`, `auto_rescue_on_eval_fail`) | none while `rescue_count < max_rescues`; supervisor only acts on C6 when the reconciler's own `repair_attempts` cap is exhausted |
| Per-spawn model escalation | **reaper** (`try_escalate_model`) | supervisor delegates to it; never escalates the model itself |
| Respawn throttle (5-min window) | **coordinator** (`check_respawn_throttle`) | supervisor extends it with a *memory-backed* cooldown (C8) for storms that age out of the 5-min window |
| Credit-exhaustion mass reset | **`wg recover`** (human) | supervisor never shadow-runs `recover`; it surfaces mass-failure to the human instead |
| Cycle failure restart | **`graph::evaluate_cycle_on_failure`** + `cycle_failure_restarts` | none — supervisor respects `FailureClass::suppresses_cycle_failure_restart` and never restarts a cycle the owner task has suppressed |
| **Reopening a *terminal* `Failed`/`FailedPendingEval` task that the reactive layer has abandoned** | **supervisor only** (C1, C6, C7) | — |
| **Stopping an unbounded re-arm loop the throttle missed** | **supervisor only** (C2, C3, C8) | — |
| **Stranded-satellite cleanup** | **supervisor only** (C5) | — |

The single boundary sentence: **the supervisor acts only on tasks the reactive
layer has stopped touching, and only on conditions the reactive layer has no
policy for.** Everything else is the reaper/sweep/reconciler/recover.

---

## 7. Feeding the rate-limit / parallelism controller (sibling studies)

The supervisor is the natural **detection producer** for the rate-limit and
parallelism controllers. It already computes, every pass, exactly the signals
those controllers need:

- **Per-task failure pressure**: `supervisor_reset_count`, crash-loop flag (C2/C3),
  storm-cooldown flag (C8). A task the supervisor has backed off is a task the
  parallelism controller should **not** count toward "free slots" and the
  rate-limit controller should **de-prioritise**.
- **Global health signal**: `needs_human` count, `backoff` count, stranded-satellite
  count. A rising `needs_human` is the rate-limit controller's "stop admitting new
  work" signal.
- **Eval-pipeline saturation**: the C6 count (PendingEval sources stuck waiting for
  a verdict) is a direct measure of eval-pipeline pressure — the parallelism
  controller's input for sizing the eval tier.

The contract: the supervisor writes a **bounded health snapshot** to
`.wg/supervisor/state.json` each pass:

```json
{"pass":42,"ts":"...","counts":{
  "needs_human":2,"backoff":1,"storm_cooldown":1,
  "stranded_satellites":0,"pending_eval_stuck":3,
  "failed_revertible":1},
 "throttle_hints":{"admit_slowdown":false,"eval_tier_pressure":"medium"}}
```

The rate-limit / parallelism controllers **read** this snapshot (they do not call
the supervisor). This keeps the dependency one-way (supervisor → controllers) and
keeps the supervisor free of any dispatch authority — it is an *observer + graph
fixer*, never a scheduler. (If the sibling studies instead expose a
`wg health snapshot` CLI, the supervisor is the writer behind it.)

---

## 8. Failure mode of the supervisor itself

The supervisor is the highest-leverage agent in the daemon; a buggy one can
silently churn the graph. Its own failure handling is therefore first-class:

### 8.1 Liveness

- The supervisor runs **inline on the daemon thread**, so its liveness *is* the
  daemon's liveness (monitored via `coordinator-state`'s `ticks`/`last_tick`, same
  as today). No separate watchdog process.
- Each pass is wrapped in a **hard timeout** (`supervisor.pass_timeout`, default
  30 s). If a pass overruns (e.g. a pathological graph scan), it is aborted and
  logged; the next pass runs normally. This is enforced with a deadline check,
  not a thread kill, so it can never leave the graph lock held.
- `state.json` records `last_pass`; a `last_pass` older than `3 × interval` with
  the daemon alive is surfaced by `wg status` as **"supervisor stalled"**.

### 8.2 Bounded blast radius

- **One task per action, ≤ 3 actions per pass** (§5). A misfiring supervisor
  corrupts at most 3 tasks per pass.
- Every mutation is logged to the journal *and* to the task's own `log` with
  `actor = "supervisor"`, so `wg show <task>` always reveals what the supervisor
  did and when.
- The supervisor **never** deletes worktrees, never touches `.wg/agents/`, never
  runs `wg recover`, and never reaps PIDs. Its only graph mutations are status
  transitions (`Failed`/`FailedPendingEval`/`Open` ↔ `Open`/`Abandoned`) and the
  two new fields.

### 8.3 Mis-reset recovery

- Because every reset bumps `supervisor_reset_count` and logs `actor="supervisor"`,
  a human can find every supervisor-touched task with one grep and undo it with
  `wg edit <id> --status failed` (or `wg recover --filter actor=supervisor`-style).
- A `wg supervisor revert <task>` convenience command undoes the most recent
  supervisor action on a task (status + `supervisor_reset_count -= 1`) using the
  journal.

### 8.4 Kill switch

- `[supervisor] enabled = false` (default **false** until §9's rollout promotes it)
  disables the whole subsystem at config load; the daemon loop skips it entirely.
- `WG_SUPERVISOR_DISABLE=1` env var overrides config to off (for emergencies).
- `wg supervisor pause` / `resume` (writes a flag into `state.json`) for a live
  toggle without a daemon reload.

---

## 9. Rollout: dry-run / audit-first

The supervisor ships in four stages. At every stage the **default is off or
dry-run**; promotion is an explicit operator action gated on audit evidence.

### Stage 0 — Observer (default **on**, no mutations)

The supervisor wakes, scans, and **writes only the journal + `state.json` health
snapshot**. It records what it *would* do for each class, but mutates no graph
row. Purpose: collect real signal, validate the detection heuristics against the
live graph, feed the rate-limit controller (§7) with no risk. This stage alone
delivers most of the observability value.

### Stage 1 — Dry-run with diffs (default **off**, opt-in)

`[supervisor] enabled = true, dry_run = true`. The supervisor computes each action
and emits a human-readable diff (`task X: Failed → Open (class C1, reason …)`),
still writing nothing to the graph. Purpose: operator reviews proposed actions
before any real mutation; ran against the worked examples (C1 `deduplicate-*`, C2
`fix-low-score-eval-gate`, C3 `storm-source`) to confirm the per-class policy.

### Stage 2 — Limited live (default **off**, opt-in, narrow scope)

`[supervisor] enabled = true, dry_run = false, classes = ["C1","C5","C6"]`. Only
the **safe, high-value** classes go live: reopen tar pits (C1), abandon stranded
satellites (C5), reset verdict-less PendingEval (C6). `max_actions_per_pass = 1`.
The crash/loop classes (C2/C3/C8) stay observer-only — their failure mode
(stopping work the human wants) is the riskiest. Purpose: prove the mutation path
and loop-prevention on the lowest-risk classes.

### Stage 3 — Full live (operator decision)

All classes live, `max_actions_per_pass` raised to its steady-state default (3).
Promoted only after Stage 2 has run for ≥ N passes on the real graph with zero
unintended mutations (audited via the journal).

**Exit criteria for the whole study → implementation handoff:** Stage 0 evidence
(the journal's classification accuracy on the live graph) plus a smoke scenario
(`tests/smoke/scenarios/supervisor_*.sh`, owner `study-long-lived`) proving the
loop-prevention bounds (per-task cap, min-interval, per-pass cap) and the
kill-switch under the smoke gate.

---

## 10. Concrete code map (what would be touched / added)

All paths relative to repo root.

### New module

- **`src/supervisor/mod.rs`** — the persona: `Supervisor` struct, `run_pass(dir, graph_path, config, dry_run)`, the per-class matcher table (§3), loop-prevention guards (§5), and the liveness timeout (§8.1). Pure logic + graph I/O via the existing `parser::modify_graph`; no new threading.
- **`src/supervisor/memory.rs`** — the sidecar journal + state rollup (§4.2/§4.3): `load_state`, `record_action`, `snapshot_health` (the §7 producer). Mirrors the atomic-write pattern of `.wg/service/coordinator-state-N.json`.
- **`src/supervisor/policy.rs`** — the per-class policy table as data (so adding a class is a table row, not a code branch), with the `FailureClass` allow/deny lists reusing `src/graph.rs:129`.

### Touched

- **`src/graph.rs`** (~line 640–720) — add `supervisor_reset_count: u32` and `last_supervisor_action: Option<String>` to `Task`, plus their serde attrs and the `Default`/helper round-trip in the `TaskHelper` block (~line 1770–1960). Also a `needs_human: bool` marker surfaced by `wg status`.
- **`src/config.rs`** — new `[supervisor]` section: `enabled` (default false), `dry_run` (default true), `interval` (180), `min_interval` (60), `min_task_interval` (900), `max_resets_per_task` (1), `max_actions_per_pass` (3), `react_settle` (90), `urgent_kick` (false), `pass_timeout` (30), `classes` (Vec<String>), plus `default_*` fns mirroring `default_poll_interval`/`default_reaper_grace_seconds`.
- **`src/commands/service/mod.rs`** (~line 3138, the `last_coordinator_tick.elapsed()` block) — add a parallel `last_supervisor_pass` check + `supervisor::run_pass(...)` call inside the daemon loop, gated by `config.supervisor.enabled` and the `paused` flag.
- **`src/commands/supervisor_cmd.rs` (new)** — `wg supervisor {run,status,pause,resume,revert}` CLI; `run --dry-run` reuses Stage 0/1 logic.
- **`src/commands/mod.rs`** — register the new command module.
- **`src/commands/status.rs`** (or wherever `wg status` renders) — surface `needs_human` count + "supervisor stalled" + last-pass summary from `state.json`.

### Reused, unchanged

- `triage::cleanup_dead_agents` (the reaper) — supervisor reads its log actor but never calls it.
- `sweep::reconcile_orphaned_tasks` — same.
- `eval_lifecycle::{reconcile_durable_verdicts, begin_source_attempt, rearm_satellites_for_source}` — supervisor *calls* `begin_source_attempt` for C6 only, inside its own `modify_graph` transaction, exactly as the reaper does (`triage.rs:~510`).
- `recover`'s abandon-followup logic (`src/commands/recover.rs`) — C5 mirrors `PlanAction::AbandonFollowup`.
- `cron`'s jitter/interval math (`src/cron.rs`) — reused for the tick schedule if a cron-style cadence is later desired.

### Tests

- Unit: each class matcher against fixture tasks (reuse the `Task` builder helpers already in `eval_lifecycle.rs` tests, e.g. `src/eval_lifecycle.rs:~2790`); loop-prevention (cap, min-interval, per-pass cap); idempotency of repeated passes; kill-switch.
- Smoke: `tests/smoke/scenarios/supervisor_loop_prevention.sh` (owner `study-long-lived`) — seed a C1 tar pit and a C2 storm, run passes, assert bounded resets + escalation; assert dry-run writes no graph mutations; assert the kill switch.

---

## 11. Open questions (for the implementation task, not this study)

1. Should the supervisor's `needs_human` marker *pause* coordinator admission for
   the affected task's downstream dependents, or only surface in `wg status`?
   (Leaning: surface only — pausing admission is the rate-limit controller's job.)
2. C3 (intentional crash) recognition: 3 identical signatures is a heuristic; is
   there a cheaper signal (e.g. a task explicitly tagged `crash-test` / a
   `--max-retries 0` convention) the supervisor should prefer?
3. Multi-instance: two WG daemons on one graph (two `$HOME`, shared `--dir`? — the
   federation study keeps them FS-isolated). If it ever happens, the supervisor
   needs a `graph.lock`-style advisory lock so two supervisors don't both reset.
   Out of scope until that topology exists.
4. Should the journal rotate (size cap + archive) or stay append-only until the
   operator clears it? (Leaning: rotate at 10 MB into `.wg/supervisor/archive/`.)

---

## 12. References

- `docs/design-casa-wgfed-adapter.md` §1, §3, §7 — the long-lived persistent-persona pattern and the "read model, not ledger" rule the supervisor's memory mirrors.
- `src/commands/service/triage.rs:233` — `cleanup_dead_agents` (the reaper); `:165` `detect_dead_reason`; `:1099` `apply_triage_verdict`; `:994` `run_triage`.
- `src/commands/service/coordinator.rs:4793` — `coordinator_tick`; `:3990` `check_respawn_throttle`, `:3979` `RESPAWN_MAX_RAPID`.
- `src/commands/service/mod.rs:2633–3300` — the daemon loop; `:3138` the `poll_interval` safety-net tick the supervisor slots into.
- `src/eval_lifecycle.rs:2089` — `reconcile_durable_verdicts` (`auto_rescue_on_eval_fail`); `:1590` `begin_source_attempt`; `:1563` `rearm_satellites_for_source`; `:1700` `MAX_PIPELINE_REPAIRS_PER_SOURCE_ATTEMPT`; `:2251–2290` the `FailedPendingEval → Failed` / rescue branches.
- `src/commands/sweep.rs:57` — `find_orphaned_tasks`; `coordinator.rs:74` its invocation.
- `src/commands/recover.rs` — `wg recover` (`RecoverOptions`, `build_plan`, `AbandonFollowup`).
- `src/graph.rs:129` — `FailureClass` (+ `suppresses_cycle_failure_restart`); `:197` `Status`; `:640–720` the counter block the new fields join; `:2806` `evaluate_cycle_on_failure`.
- `src/config.rs` — `default_poll_interval` (`:4503`), `default_reaper_grace_seconds` (`:4853`), `default_auto_rescue_on_eval_fail` (`:3693`), the `[agent]`/`[agency]`/`[coordinator]` sections.
- `src/cron.rs:167` — `reset_cron_task` (the existing in-daemon periodic-wake precedent).
- Worked examples from this graph (this session): `deduplicate-config-deprecation` (C1), `fix-low-score-eval-gate` `retry_count=6` (C2/C8), `storm-source` (C3).
