# Repair: pre-Pi evaluation plans missing reasoning

**Task:** `repair-missing-agency-reasoning`
**Incident:** `make-hashed-project` stranded in `PendingEval` with
`WG-EVAL-PIPELINE-REPAIR-EXHAUSTED`.

## Incident summary

`make-hashed-project` completed its implementation, but its FLIP and evaluator
satellites were scaffolded **before** Pi reasoning became a mandatory part of
the model plane (`make-pi-the-2`). The persisted `AgencyDispatchPlan` calls
contained exact `pi:<provider>:<model>` routes but **no `reasoning`**. Pi-only
execution correctly fails closed at the execution edge
(`run_lightweight_llm_call_for_plan` → `WG-PI-REASONING-MISSING`,
`src/service/llm.rs`), and the lifecycle repair then **replayed the same
invalid bytes** byte-for-byte until its bounded repair budget was exhausted
(`WG-EVAL-PIPELINE-REPAIR-EXHAUSTED`), leaving the completed source worker
operator-blocked with no recovery path.

Authoritative incident identity (from the task): source attempt 1, pipeline
`evalp-cf8647216311909aacbaa9d2`; FLIP plan
`b3:d7cd2331644ef581ba7ab579def9113e7c8c5030c493b9bb6255e434a11e0a42`;
evaluator plan
`b3:f3642b022aabbb4816c3b118278e365356b5ce3d76b877dd72881a4e19ed7de2`.

## Fix: an explicit, bounded migration boundary

A new deterministic recovery path
(`worksgood::eval_lifecycle::migrate_missing_pi_reasoning`) runs in the
coordinator's single graph transaction **before** ordinary lifecycle repair, so
invalid missing-reasoning bytes are never replayed against the bounded repair
budget again.

### What it does

1. **Selects recoverable sources.** For each `PendingEval`/`FailedPendingEval`
   source whose diagnostic is absent or names one of the recoverable codes
   (`WG-EVAL-PIPELINE-REPAIR-EXHAUSTED`, `WG-PI-REASONING-MISSING`,
   `WG-EVAL-PI-REASONING-MIGRATION`), it collects the `.flip-*` and
   `.evaluate-*` satellites.
2. **Authenticates the old plans.** Each satellite's persisted plan is
   structurally validated (`validate_plan` — authenticates the historical hash)
   and its identity (source, attempt, pipeline, generation) is matched against
   the authoritative source lifecycle. An active satellite is never relabeled.
3. **Accepts only exact Pi routes.** Each call's route must parse as
   `pi:<provider>:<model>` (`parse_exact_pi_route`). Non-Pi / malformed legacy
   plans are **rejected with no cross-system fallback**.
4. **Resolves reasoning authoritatively.** Each absent effort is resolved from
   the stage role/tier configuration (`Config::resolve_reasoning_detail`). The
   resolved level **and its config provenance** (`models.<role>.reasoning` /
   `tiers.<tier>_reasoning`) are recorded. If reasoning cannot be resolved, it
   **fails closed** with an actionable diagnostic naming the key to set — it
   never synthesizes a Claude/Codex/Nex/native route or an implicit default.
5. **Mints a newly hashed generation atomically.** All satellites + source move
   to one new `route_generation` and a content-addressed `pipeline_id`
   (`reasoning_migration_pipeline_id`). Each new plan is re-hashed and must
   clear `validate_executable_plan` (the strict model-plane boundary) before
   anything is committed. The transaction is all-or-nothing per source.
6. **Rearms satellites without rerunning the source.** Satellites return to
   `Open`/unassigned with the new executable plan; the source's status, retry
   counters, artifacts, and commit are untouched — only its lifecycle identity
   advances.

### Invariants

- **Never execute / silently accept a missing-reasoning plan.** Defense in
  depth: `validate_executable_plan` (migration-time) and
  `run_lightlight_llm_call_for_plan` (execution-time) both reject missing
  reasoning. The migration is the only recovery path.
- **Exactly once.** A source attempt may cross the boundary at most once
  (`MAX_REASONING_MIGRATIONS_PER_SOURCE_ATTEMPT = 1`). A restart, retry, or
  concurrent coordinator tick re-running the boundary is a deterministic no-op
  (the migrated generation is already installed; the budget is consumed).
- **Immutable history.** Original plans, producer ids, prior statuses, prior
  failures, and the prior source diagnostic are retained in append-only
  `EvaluationLifecycle::plan_migrations` audit rows
  (`AgencyPlanMigration`/`AgencyReasoningResolution`).
- **Stale verdicts never score.** The new pipeline id is generation-specific,
  so a durable verdict carrying the pre-migration pipeline id is filtered out
  by `reconcile_durable_verdicts`; new evidence is consumed exactly once.
- **Generation zero is hash-stable.** `AgencyDispatchPlan::route_generation` is
  `#[serde(default, skip_serializing_if = "is_zero_u32")]`, so pre-migration
  plan hashes remain byte-for-byte verifiable.

## `wg show` / status surface

`EvaluationHealthState` gains two states so operators can distinguish the
recovery posture at a glance:

| State | Meaning |
|-------|---------|
| `migration-required` | Exact Pi routes with missing reasoning, parked on a recoverable diagnostic; the coordinator will resolve reasoning at the boundary. |
| `migrated-rearmed` | Migration committed atomically; satellites are re-armed `Open`/unassigned and the source worker was not rerun. |
| `active-evaluation` | A migrated satellite has been claimed / is running. |
| `operator-required-ambiguity` | Missing reasoning that is non-Pi/malformed, or cannot be resolved from config — operator action required (no fallback). |

`EvaluationHealth` exposes `route_generation`, `migration_count`, and
`consumed_verdict`. `wg status` prints
`Evaluation: N active, … migration-required, … migrated/rearmed, …
operator-required ambiguity`.

## Auditability

Each `AgencyPlanMigration` row records: schema, boundary id, migrated-at,
source task/attempt/generation, task id, old + new pipeline ids, old + new
plans (full hashes), per-call reasoning resolution (level + provenance +
config source), and the prior producer run id / status / started / completed /
failure reason / source diagnostic.

## Validation

- [x] Fixture matching `make-hashed-project` migrates route-preservingly to
      explicit effective reasoning and completes FLIP/evaluation without
      rerunning source work
      (`migrate_missing_pi_reasoning_resolves_route_preservingly_and_rearms_without_rerunning_source`,
      `migrate_missing_pi_reasoning_consumes_new_generation_verdict_exactly_once`).
- [x] Missing reasoning with no authoritative role/tier value stays fail-closed
      and operator-actionable
      (`migrate_missing_pi_reasoning_fails_closed_without_authoritative_reasoning`).
- [x] Non-Pi/malformed legacy plans remain rejected; no cross-system fallback
      (`migrate_missing_pi_reasoning_rejects_non_pi_and_malformed_legacy_plans`).
- [x] Old and new plan hashes, reasoning, source attempt, pipeline/generation,
      producer run, and consumed verdict are auditable
      (asserted in the route-preserving + verdict tests; `AgencyPlanMigration`
      struct).
- [x] Restart/concurrent-tick regression proves bounded exactly-once migration
      and verdict consumption
      (`migrate_missing_pi_reasoning_is_bounded_and_idempotent_across_restart`,
      idempotent verdict consumption).
- [x] Existing retry-drift, durable evaluation, Pi-only model-plane, and
      low-score gate regressions pass; `cargo fmt --check`, `cargo clippy`
      clean.

## Files

- `src/eval_lifecycle.rs` — migration boundary, `validate_executable_plan`,
  `AgencyPlanMigration`/`AgencyReasoningResolution`, `route_generation`,
  `plan_migrations`, health states, and 7 new unit tests.
- `src/commands/service/coordinator.rs` — `coordinator_tick` calls
  `migrate_missing_pi_reasoning` before ordinary repair; lifecycle construction
  preserves the migrated generation.
- `src/commands/status.rs` — migration-required / migrated-rearmed counters and
  status line.
