# Design: claim lifecycle for `wg reset` / `wg retry` / dispatcher heartbeat

**Task:** design-claim-lifecycle
**Author:** agent-979 (Default Evaluator, Opus)
**Date:** 2026-04-28

## TL;DR

Adopt **"Both" — Eager + Lazy with status-aware reconciler**:

1. **Eager on `wg retry`**: walk transitive downstream and clear `assigned` on
   any *non-terminal* downstream task whose claim references a dead agent.
   Mirrors the closure semantics that `wg reset` already has.
2. **Lazy in dispatcher**: extend `sweep::reconcile_orphaned_tasks` so it also
   handles `Status::Open` tasks with stale `assigned`, not just `InProgress`.
   This is the kill -9 / panic / crash safety net.
3. **Touch up `wg service status` hint** to mention stale-claim possibility
   (already done in this PR — 2 lines in `src/commands/service/mod.rs`).

`wg reset` already does the eager-on-self path correctly (commit `32522a398`,
`fold-unclaim-semantics`); the bug-reset filing was effectively against a
stale build. Tests (`reset_clears_assigned_field`,
`reset_with_strip_meta_still_clears_assigned`) and the
`reset_clears_claim` smoke scenario already pin this.

## Why "Both" — and why not the alternatives

### Why not Eager-only (A+C)
Eager catches user-initiated paths but misses every other way an agent can
die without telling the graph: `kill -9`, daemon crash, OOM, host reboot,
panic before the agent's own cleanup hook fires. The
`bug-retry-doesnt-clear-stale-downstream-claims` story — synthesis tasks
holding 14-hour-old agent IDs — is a poster child: those tasks weren't
created by `wg retry`, they were created by the agency assigner before the
upstream failed, and even after upstream completes successfully on a fresh
attempt, no eager path runs through them. A safety net is necessary.

### Why not Lazy-only (B)
Two reasons:
- **Latency.** Reconciliation runs once per dispatcher tick (`poll_interval`,
  default ~30s). A user who runs `wg reset` and `wg ready` in the same
  breath will see the task still claimed for up to one tick. Eager clearing
  is instant feedback and removes the "did the reset work?" confusion that
  motivated bug-reset in the first place.
- **Correctness vs. liveness.** Reconciler can only act on "agent is
  *currently* dead." But `wg retry <upstream>` is a *user signal* that the
  scheduling context for the entire downstream cone has changed; even if a
  downstream agent were technically still alive (e.g., spinning idle on a
  blocked dep), it should be unbound so the assigner can re-route. Eager
  encodes intent; lazy only encodes liveness.

### Why "Both" wins
Eager handles user-initiated transitions with low latency and intent
encoding. Lazy handles every other path. The two together are
**complementary, not redundant** — eager runs in single-call CLI scope (no
extra ticks needed), lazy runs once per tick on the rare residue. The
implementation cost for both is small because each piece already exists in
some form: `reset.rs` walks closures, `sweep::reconcile_orphaned_tasks`
already runs each tick.

## Field / column changes in `graph.jsonl`

**None.** The fields involved (`assigned`, `started_at`, `status`) already
exist and are written/read on every codepath. No schema migration, no
config flag, no new field.

(Considered: a `claim_heartbeat_at: <ISO8601>` field on each task to enable
TTL-based reconciliation. Rejected — `AgentRegistry` already tracks agent
liveness via PID and `AgentStatus`, and adding a redundant per-task
timestamp creates a second source of truth. Keep one.)

## Code locations the implementation task should touch

| Concern | File | Function | Change |
|---|---|---|---|
| Eager on `wg retry` | `src/commands/retry.rs` | `run` (Failed/Incomplete branch ~line 130–138) and `retry_in_progress` (~line 340–350) | After clearing target task's `assigned`, walk transitive `before` edges; for each non-terminal downstream task whose `assigned` references an agent that is `AgentStatus::Dead` (or absent from registry), clear `assigned` + `started_at` and append a log entry naming the upstream retry as the cause. |
| Closure walker reuse | `src/commands/reset.rs` | `compute_closure` (line 296) | Already reusable — `Direction::Forward` from a single seed gives the transitive downstream. The retry path can call this directly (consider extracting `compute_closure` to a sibling module to avoid `commands::reset` -> `commands::retry` cross-import; a small `commands::claim_lifecycle` module fits). |
| Lazy reconciler extension | `src/commands/sweep.rs` | `reconcile_orphaned_tasks` (line 304) | Currently filters on `task.status == Status::InProgress`. Extend to also cover `Status::Open` tasks where `task.assigned.is_some()` and the referenced agent is Dead-or-absent. The existing per-task mutation block (line 351) already does the right thing (`status = Open`, `assigned = None`, log entry); the only change is the predicate. |
| Dispatcher tick wiring | `src/commands/service/coordinator.rs` | `coordinator_tick` (line 4085) — already calls `reconcile_orphaned_tasks` at line 61 of the same file's preamble | No change needed; the extended reconciler picks up the new predicate automatically. |
| User-facing hint | `src/commands/service/mod.rs` | `run_status` (lines 3152, 3181) | **Already updated in this PR** to surface stale-claims as a possible cause alongside agent configuration, with concrete remediation commands (`wg list --status open`, `wg unclaim`, `wg reset --yes`). |
| `wg unclaim` reuse | `src/commands/` | (verify whether `unclaim.rs` exists) | The hint references `wg unclaim`; confirm the command is wired in `cli.rs` and the help text matches. The repro steps in both bug docs already use it as the manual workaround, so it must exist in main. |

## Backward-compatibility concerns

1. **Existing graphs with stale claims at upgrade time.** First dispatcher
   tick after the new code lands will sweep all `Open + assigned-but-dead`
   tasks and unclaim them. This is desirable — operators who hit the bug
   pre-upgrade get auto-recovery on next start. Risk is logging noise:
   bound it with a single summary line per tick (e.g.,
   `"[dispatcher] Reconciliation: recovered N orphaned task(s) (M from Open)"`)
   instead of one line per task. The existing log format already does this.

2. **Eager on retry could touch a *currently-active* downstream agent.**
   Mitigation: only act when the registry agent is `AgentStatus::Dead` *or*
   `is_process_alive(pid) == false` *or* the agent is absent entirely. If
   the agent is alive, the eager path leaves the claim alone; the lazy
   reconciler will pick it up if/when the agent dies. This matches the
   conservative posture of the existing `reconcile_orphaned_tasks` predicate.

3. **Cycles.** `compute_closure` already handles them via `visited`. The
   transitive walk from a retry seed will not loop on a back-edge.

4. **Meta tasks (`.flip-*`, `.evaluate-*`, `.assign-*`).** `compute_closure`
   already excludes system tasks from the closure (line 300 / 322); retry's
   eager walk inherits this. The agency pipeline regenerates them; we don't
   want to mutate them mid-flight.

5. **`max_agents` accounting.** Reconciler decrements alive count
   indirectly by transitioning tasks Open. No registry mutation needed —
   the dead agent is already Dead in the registry. No double-counting.

## Concrete repro / smoke scenarios this fix MUST ship with

These scenarios extend `tests/smoke/manifest.toml` (grow-only). Two are
required by the task; a third is included because the gap was specifically
the *Open*-with-stale-claim path the existing `reset_clears_claim`
scenario doesn't cover.

### Scenario 1: `retry_clears_downstream_stale_claims`
**Owners:** `fix-claim-lifecycle`, `design-claim-lifecycle`, `smoke-gate-is`

Pure registry+graph assertions, no LLM credentials needed.

```bash
# Setup: handcraft graph.jsonl with
#   upstream (Failed)  →  downstream (Open, assigned=agent-dead-1)
# Setup: handcraft registry.json with agent-dead-1 status=Dead, completed_at past
# Run:   wg retry upstream --reason "smoke"
# Assert (a) downstream.assigned == None
# Assert (b) downstream.status == Open
# Assert (c) downstream.log contains "stale-claim cleared via retry of upstream"
# Assert (d) upstream.status == Open
```

### Scenario 2: `reconciler_clears_open_with_dead_agent`
**Owners:** `fix-claim-lifecycle`, `design-claim-lifecycle`, `smoke-gate-is`

Covers the lazy path — handles paths neither retry nor reset reach (kill -9
of an agent whose task was scheduled but never started, panic-on-startup,
etc.). Drives a real dispatcher tick.

```bash
# Setup: handcraft graph.jsonl with
#   ready-task (Open, assigned=agent-zombie-1, status=Open, deps_met=true)
# Setup: handcraft registry.json with agent-zombie-1 status=Alive but PID=99999 (unreachable)
# Run:   wg service start --max-agents 1; sleep 2*poll_interval
# Assert (a) ready-task.assigned == None
# Assert (b) registry.agent-zombie-1.status == Dead (reaped by triage)
# Assert (c) tick log shows "Reconciliation: recovered N orphaned task(s)"
# Assert (d) a fresh agent was spawned on the now-unclaimed task
```

### Scenario 3: `reset_clears_downstream_claims_too`
**Owners:** `fix-claim-lifecycle`, `design-claim-lifecycle`, `smoke-gate-is`

Belt-and-suspenders against regression — the existing `reset_clears_claim`
scenario only checks the seed task's own claim. A future refactor of
`compute_closure` could silently drop downstream and the existing scenario
wouldn't notice.

```bash
# Setup: graph with seed → downstream (both InProgress, both with dead-agent claims)
# Run:   wg reset seed --yes  (default direction=Forward includes downstream)
# Assert seed.assigned == None AND downstream.assigned == None
# Assert both transitioned to Open
```

## Sequencing for the implementation task

1. Extract `compute_closure` to `src/commands/claim_lifecycle.rs` (or accept
   a shared utility module location reviewers prefer). Keep `reset.rs`
   importing from it.
2. Add the eager-walk to both branches of `retry.rs` (failed/incomplete
   path and `retry_in_progress`). One log entry per cleared downstream.
3. Extend `sweep::reconcile_orphaned_tasks`'s status filter to
   `InProgress | Open`. Tighten the log line to a single summary.
4. Add the three smoke scenarios under `tests/smoke/scenarios/`.
5. Add `owners = ["fix-claim-lifecycle", "design-claim-lifecycle"]` rows
   in `tests/smoke/manifest.toml`.
6. `cargo build && cargo test && cargo install --path .` (the global
   binary is what the smoke scenarios invoke).
7. Run `wg done fix-claim-lifecycle --full-smoke` locally before push to
   exercise all three new scenarios plus the existing `reset_clears_claim`.

## Out of scope (explicit non-goals)

- A `claimed_at` heartbeat field with TTL-based expiry. Rejected — the
  agent registry already tracks liveness; a separate timestamp duplicates
  truth.
- A `--keep-claim` flag on `wg reset` / `wg retry` for the rare "I want to
  preserve the assignment" use case. The existing `wg assign` (post-reset)
  covers this, and a flag we don't need today is a maintenance tax.
- Touching `.assign-*` / `.flip-*` / `.evaluate-*` claim semantics. They
  re-generate on demand; no eager downstream walk should mutate them.
- Renaming `assigned` to `claimed_by` for terminology consistency with the
  bug docs. The bug docs use the user-facing word "claim"; the field name
  `assigned` is fine and changing it is a 100+ file ripple. Leave alone.
