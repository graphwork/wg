# Task-owned finish transaction

**Status:** Implemented

**Supersedes:** the normal detached merge/repair path in
[Candidate finalization transaction](design-candidate-finalization-transaction.md).
Its immutable candidate and crash-safe evidence formats remain the storage
substrate and its legacy commands remain narrowly available for transactions
created before this protocol.

## Contracts

Every task has a backward-compatible completion contract:

- `land` (the default): the exact accepted integrated commit must be on `main`,
  owned scratch must be removed, and a cleanup receipt must exist.
- `deliver`: publish a retained `refs/wg/contributions/<task>/v<N>` ref and
  output receipt without changing `main`, then clean.
- `report`: publish a retained `refs/wg/reports/<task>/v<N>` ref and output
  receipt without changing `main`, then clean.

Ordinary `after` edges require a landed prerequisite. A delivered prerequisite
satisfies only an explicitly typed contribution edge, created with:

```text
wg finish input <synthesis-task> --from <deliver-task>
```

A synthesis task merges or otherwise consumes retained contribution refs in its
own worktree. Producer worktrees are not storage.

## Land protocol

The original task agent owns the entire successful path:

```text
wg finish begin <task>
# integrate returned current-main base and resolve conflicts in this worktree
wg finish submit <task> --lease <lease-id> --commit HEAD
# wrapper leaves cwd
wg finish cleanup <task>
```

`begin` takes the one persisted repository finish lease. The lease expires and
is fenced by task, generation, attempt, process epoch, worktree identity, and
worktree lease epoch. Another worker may keep editing, but no WG merge authority
may advance `main` while the lease is valid.

`submit` rechecks integration, seals the current commit/tree/manifest, records a
bound validation result, and requests selected evaluation products. Evaluators
receive immutable candidate evidence and publish receipts only. A required
acceptance receipt must bind the same candidate before the protected authority
performs a compare-and-swap from the leased base to that exact commit.

A conflict, semantic rejection, or evaluation infrastructure outcome returns an
error to the same source attempt and worktree. Semantic rejection is distinct
from `InsufficientEvidence`/`Unavailable`; infrastructure consumes evaluation
retry policy, not a source retry. A changed candidate creates a new candidate,
validation result, and evaluation binding.

## Cleanup and restart

Promotion/delivery/report is durable before cleanup. The generated wrapper exits
the worktree cwd and synchronously removes the owned worktree, temporary branch,
and owned build state, then records a content-addressed cleanup receipt. Only
then does the lifecycle expose `Done` with `Landed`, `Delivered`, or `Reported`.

Restart reconciliation is intentionally one-way. If a durable merge/output
receipt exists and no live owner remains, it may run cleanup and commit terminal
status. It cannot rerun implementation, evaluation, or promotion. Cleanup and
promotion are idempotent by receipt.

The detailed transaction phases remain in `.wg/finalization`; `wg show` projects
`Working`, `WaitingEvaluation`, `RepairNeeded`, and terminal disposition without
adding graph status variants.

## Migration

Existing task JSON without a completion contract deserializes as `land`.
Existing ordinary dependency edges keep landed-success semantics. No graph
migration is required.

New worktree-backed `wg done` calls use task-owned finish. Transactions created
by the previous finalizer without a finish lease retain a compatibility path so
restart can settle already-sealed work, but the compatibility merge authority
fails while a valid finish lease exists. New success paths do not dispatch merge
agents or repair generations.

Operator/configuration changes:

- choose a non-default contract before assignment with
  `wg finish contract <task> deliver|report`;
- create synthesis inputs explicitly with `wg finish input`;
- use `wg finish status` for the durable candidate/evaluation/output/cleanup
  receipts (`wg finalize` remains an alias);
- do not use child worktree paths as synthesis inputs.
