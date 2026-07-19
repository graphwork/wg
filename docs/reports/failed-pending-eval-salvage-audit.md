# FailedPendingEval lifecycle salvage audit

Date: 2026-07-19  
Task: `salvage-and-finish`  
Current-main base: `0460ac31`  
Preserved snapshot: `0fc1a79b` (parent `c2499f3e`)  
Normative design: `docs/reports/pending-eval-flip-inline-route-resolution.md`

## Provenance and staged recovery

Luca Pinello's satellite-completion predicate was retained as its own commit,
`9176849c`, by cherry-picking `c2499f3e` with the original author metadata. The
follow-up narrows Luca's dot-task predicate to only the owning `.flip-X` or
direct `.evaluate-X` relation; ordinary dependents and unrelated dot tasks stay
blocked.

The archived snapshot was applied without committing, audited, corrected, and
validated in stages. It was not merged wholesale. The first application failure
was the expected grow-only manifest conflict: current main had four scenarios
past the snapshot's manifest tail. All current entries were retained and the new
scenario was appended. The first focused validation failure was `cargo fmt
--check` on two new lifecycle layouts; formatting was applied. The first
semantic test failure was the retry-idempotency test: a consumed old verdict was
being compared to a rebound satellite plan. Reconciliation now short-circuits an
already-consumed pipeline before touching the new attempt.

The two archived worker runs did not record a Rust/test failure. Agent 522's
last command was still compiling the first focused bin test when its wrapper
exited code 1; the chained second `cargo test` also supplied two positional test
filters and therefore never constituted a valid completed validation stage.

## File-by-file disposition of `0fc1a79b`

| Snapshot path | Disposition | Audit result |
|---|---|---|
| `.wg-cleanup-pending` | **Discarded** | Ephemeral cleanup marker, never source. |
| `src/commands/add.rs` | Retained | Initializes the two backward-compatible optional task records. |
| `src/commands/critical_path.rs` | Retained | Test fixture initializer only. |
| `src/commands/done.rs` | Retained and corrected | Keeps Luca's bypass, makes it relation-aware, and refreshes source-attempt identity on each soft-eval entry. |
| `src/commands/eval_scaffold.rs` | Retained | Persists complete stage-aware plans and compatibility mirrors; FLIP has distinct inference/comparison calls. |
| `src/commands/evaluate.rs` | Retained | Internal wrapper identity pins the plan; exact planned routes execute; semantic evidence is durable before `wg done`; scoreless source mutation is removed. |
| `src/commands/evolve/deferred.rs` | Retained | Task fixture initializer only. |
| `src/commands/fail.rs` | Retained and corrected | FailedPendingEval now refreshes current source-attempt identity instead of retaining a consumed prior rescue attempt. |
| `src/commands/func_apply.rs` | Retained | Task initializer only. |
| `src/commands/notify.rs` | Retained | Test fixture initializer only. |
| `src/commands/service/coordinator.rs` | Retained and corrected | Removes scoreless terminal/missing-evaluator promotion; exact plan claim; bounded pre/post-claim budgets; fail-closed evidence load; CAS repair/consume; policy-aware advisory/gated outcome and bounded exact-route rescue. |
| `src/commands/service/ipc.rs` | Retained | Task initializer only. |
| `src/commands/why_blocked.rs` | Retained | Uses the same edge disposition as dispatch and reports the evaluation-system bypass rather than a false root blocker/SCC. |
| `src/eval_lifecycle.rs` | Retained and substantially corrected | Canonical plan/pipeline, tamper-evident durable verdict, referenced-Evaluation digest check, pre-claim-only historical repair, unambiguous legacy evaluation migration, idempotent linking/consumption, and exact-route rescue rebind. |
| `src/graph.rs` | Retained | Adds optional serde-defaulted plan/lifecycle records without changing old graph readability. |
| `src/lib.rs` | Retained | Exposes lifecycle primitives to bin and library users. |
| `src/query.rs` | Retained | One relation-aware `DependencyDisposition` now drives all readiness variants and diagnostics. |
| `src/service/executor.rs` | Retained | Test fixture initializer only. |
| `src/service/llm.rs` | Retained and corrected | Planned endpoint/reasoning/fallback invocation; exact handler-first route wins; legacy explicit Codex split canonicalizes once; ambient role routing is ignored after persistence. |
| `tests/integration_auto_assignment.rs` | Retained | Fixture initializer only. |
| `tests/smoke/manifest.toml` | Retained with current tail | Grow-only append; ownership moved to `salvage-and-finish`. |
| `tests/smoke/scenarios/pending_eval_lifecycle_route_recovery.sh` | Retained and expanded | Codex/Pi/Claude plan matrix, config-drift persistence, Luca positive/negative flow, honest `why-blocked`, no scoreless promotion, one-time historical repair, held-lock/concurrent/restarted ticks, and no agent growth. |
| `tests/test_cron_integration.rs` | Retained | Fixture initializer only. |
| `tests/test_cron_serialization.rs` | Retained | Fixture initializer only. |
| `tests/test_verify_timeout_functionality.rs` | Retained | Fixture initializer only. |

Two additional reviewed source files changed after focused tests exposed
remaining current-main seams. `src/config.rs` now makes
`execution_system_key` reject bare Codex/OpenRouter/vendor forms; only
handler-first Codex/Nex routes (plus documented Claude aliases) identify an
execution system. `src/commands/retry.rs` rearms an already-consumed source's
existing satellites with the exact persisted routes and a new pipeline identity,
so an explicit retry cannot inherit old terminal satellites and bypass scoring.

## Rejected snapshot behavior

The following archived behavior was not accepted unchanged:

1. **Broad historical reopen.** The snapshot reopened every unassigned
   `Incomplete` satellite. Automatic repair now requires pre-claim evidence:
   no assignment/start time and a nonzero spawn-failure count.
2. **Unauthenticated verdict JSON.** Verdict records now carry and verify their
   own digest, filename identity, and the digest/source of the separately
   persisted Evaluation. Any corruption makes the whole maintenance phase fail
   closed.
3. **All low scores terminal-fail.** Advisory PendingEval evaluations still
   complete. Hard-gated low scores may re-open only within the configured bound,
   preserving the exact routes while minting a new pipeline/source attempt.
   FailedPendingEval remains rescue-by-score or terminal reject.
4. **Consumed-old-pipeline touching rebound tasks.** Restart reconciliation now
   treats the source's consumed verdict as the authoritative CAS fence and is a
   no-op before inspecting the next attempt's satellite plan.
5. **Unbounded/ambiguous migration.** Bare OpenRouter cannot identify Pi versus
   Nex and parks. Legacy Evaluation migration requires exactly one fresh,
   loop-matching candidate; zero/multiple/missing-time candidates are not
   guessed.
6. **Snapshot smoke scope.** The Codex-only/max-agent-zero test was expanded to
   the required handler matrix and slow-storage/restart/no-growth behavior.

## Invariants implemented

- Ordinary dependencies remain blocked by PendingEval/FailedPendingEval; only
  the owning evaluation satellite receives the dispatcher bypass.
- Scaffold, inline claim, child invocation, restart, and rescue retry share one
  hashed plan with exact handler/provider, stage, endpoint, reasoning, and
  explicitly validated same-system fallbacks.
- Pre-claim scheduling and claimed transport each have a bound of two per plan
  generation. Open-to-Waiting/Blocked is the reservation CAS, so concurrent
  stale ticks increment once.
- A verdict is durable before satellite completion, linked once by
  pipeline/source-attempt/stage, and consumed into the source in the same graph
  transaction as the outcome.
- A crash after verdict persistence is reconciliation-only; a crash before a
  verdict may retry only within the exact-plan execution budget.
- Terminal or missing evaluator rows never imply a pass.
- Historical Codex split rows rearm once only when pre-claim provenance is
  present. Ambiguous routes and verdict sets fail closed.

## Validation staging

All Cargo commands use the single isolated target
`/home/bot/wg/.cargo-target-agent-558`; no candidate is globally installed.
The permanent smoke is candidate-binary capable by prepending that target's
`debug/` directory to `PATH`. Final validation records are in the WG task log.
