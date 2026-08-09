# Accepted terminal outcome → Agency observation projection

**Status:** implemented
**Policy/schema:** `accepted-terminal-outcome-v1` / `terminal_observation.schema_version = 1`

## Purpose and authority boundary

A receipt-backed terminal task is useful evidence for Agency learning even when
no one has assigned it a quality score. The completion/review plane and the
Agency plane therefore meet through a **separate observation projection**:

```text
immutable candidate + review evidence + publication
                    │
                    ▼
       lifecycle-accepted terminal Done
                    │ read only
                    ▼
 .wg/agency/terminal-observations/<id>.json
```

The projector can read the graph and immutable completion store and can create
one Agency record. It has no graph writer, lifecycle transition, publication,
retry, dispatch, message, or reviewer API. A projection failure leaves Done
unchanged and creates a visible reconciliation backlog; it cannot complete,
fail, reopen, or block a task. Completion FLIP/eval remains advisory unless the
operator explicitly selected strict completion review.

## Exactly-once identity

The stable key is:

```text
(policy_version,
 task_id,
 lifecycle_generation,
 source_attempt_id,
 source_attempt_fence,
 immutable_completion_receipt_digest)
```

Its canonical-JSON BLAKE3 digest is the observation ID. The record is published
with a synced temporary file plus an atomic no-replace hard link. A retry,
repeated/idempotent `wg done`, concurrent writer, daemon restart, service reload,
or operator reconciliation returns the existing record for the same key. A new
generation, attempt, fence, or terminal receipt is a different episode.

The immutable completion receipt already binds ordinary completion to the
manifest, requirements, FLIP/eval receipts, contract, publication evidence, and
completion time. Operator acceptance has its own immutable receipt and its own
`operator_accepted` observation kind.

## Eligibility and verification

The projector fails closed unless all applicable evidence verifies:

- task status is `Done`, with the contract's exact completion disposition;
- one lifecycle event binds the receipt to the current successful
  generation/attempt/fence;
- the receipt object exists at its BLAKE3 address and matches its task and time;
- ordinary completion re-resolves the selected manifest, requirements, summary,
  output bytes, attempt-bound review receipts, and publication receipt;
- Land's reviewed commit is still reachable from the receipt's integration ref;
- Report/Explore publication identities equal the immutable manifest outputs;
- operator acceptance matches the attributed operator lifecycle event and
  retains the required reason.

Failed, Waiting/Needs-review, stale-generation, unlanded, legacy-unbound, missing,
or unverifiable rows produce no observation. Operator acceptance is deliberately
marked `ordinary_publication_verified = false`: it is explicit human
adjudication, not silently relabeled ordinary reviewed completion.

## Recorded evidence

One observation preserves:

- Agency composition attribution (`agent`, `role`, `tradeoff`) or an explicit
  `uncomposed_direct_dispatch` / unresolved state;
- lifecycle actor, exact generation/attempt/fence, requested model/profile,
  actual executor/model/route when known;
- exact provider-reported source usage and cost when known (never estimated);
- current and superseded verified completion-review receipt identities,
  verdicts, routes, executors, usage/cost, timing, findings digests, and
  candidate state;
- current-candidate and whole-trajectory disagreement flags;
- ordinary manifest/publication provenance or reasoned operator-accept
  provenance.

Unknowns stay unknown. A terminal observation always has `score = null`,
`score_state = "unscored"`, and names the unknown quality, dimensions,
independent-ground-truth, assignment-reward, and reviewer-calibration fields.
Completion FLIP/eval verdicts are evidence; they are **not** converted into
Agency `Evaluation.score` values.

The explicit `wg evaluate run <done-task>` and `wg evaluate record` surfaces
are the only score-producing paths. `run` binds its separate immutable score to
the exact terminal observation after re-verifying completion/publication;
`record` retains an explicit external source. Existing evolution and
performance averages consume scored evaluations, not completion-review verdicts.

## Queries

`wg agency stats` prints terminal-observation, linked-scored,
without-linked-score, and operator-accepted counts separately from scored
evaluation count and average. The machine-readable form, `wg --json agency
stats`, includes:

- `overview.total_terminal_observations`;
- `overview.scored_terminal_observations`;
- `overview.unscored_terminal_observations`;
- `overview.operator_accepted_terminal_observations`;
- rich `scored_evaluations[]` envelopes;
- full immutable `terminal_outcomes[]` records whose own score remains null.

The observation itself never increments a `PerformanceRecord`. Only an explicit
create-once Evaluation linked to it changes performance.

## Crash recovery and bounded backfill

Ordinary and operator `wg done` attempt projection immediately after the Done
commit. Missing projections converge at the start of a coordinator tick. `wg
agency migrate` runs the same explicit backfill after Agency schema migration.

Backfill is bounded (`256` by default for migration, `64` per daemon tick),
newest-first, create-once, and idempotent. Only modern receipt-bound terminal
rows consume the budget. Historical evidence without an exact attempt or modern
completion receipt is preserved but not guessed into eligibility. Re-running
backfill is safe and reports created/existing/skipped/remaining/error counts.
