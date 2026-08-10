# Completion-review projection recovery

## Incident and exact writer

The scored-evaluation restart canary exposed a schema-skew overwrite, not a
receipt loss. In the incident trace, TUI PID `471767` had started on August 7,
its `/proc/471767/exe` pointed at a deleted/replaced WG executable predating
`completion_candidate.review_binding` and `completion_review_activity`, and it
outlived the protected daemon PID change `2095117 → 1337595`. The service
restart changed `graph.jsonl`; that woke the long-lived TUI. Its old `Task`
decoder omitted the unknown review fields and its compatibility full-save path
replaced the graph with that reduced shape.
Candidate, FLIP, completion, terminal-observation, and scored-evaluation objects
were immutable and survived.

Current TUI mutations use fresh locked graph edits rather than replacing a
cached graph. The graph save boundary additionally preserves append-only review
activity and same-manifest candidate identity. Current writers also reconcile a
stripped current projection from immutable receipts before saving, so daemon
startup/ticks, lifecycle replay followed by a write, `wg log`, Done, and cleanup
converge rather than perpetuate the loss.

A binary upgrade cannot change code already mapped into a running process.
Restart old TUI sessions after upgrading. Receipt reconciliation is the recovery
backstop; it does not make arbitrary old writers safe.

## Bounded repair

Preview one bounded batch:

```sh
wg --json migrate review-identity --limit 256 --dry-run
```

Apply it:

```sh
wg --json migrate review-identity --limit 256
```

The report separates repaired, unchanged, skipped, invalid, and remaining rows.
Re-running is idempotent.

Repair is intentionally narrow:

- only the selected current candidate is considered;
- referenced review objects are rehashed and decoded;
- FLIP/Eval must agree on task, generation, attempt, fence, candidate sequence,
  manifest, and requirements;
- a terminal task must also have a matching immutable completion receipt;
- findings objects are content-verified before activity is projected;
- conflicting mutable rows fail closed;
- missing superseded history is never inferred.

The repair changes no lifecycle event, assignment, publication, scored Agency
row, evaluator route, or provider state. It only restores the mutable read model
that points at already-existing immutable evidence.

Scored-evaluation replay still re-verifies the current lifecycle, candidate,
review, publication, and completion receipts. Its stored terminal observation
remains authoritative only for terminal-time execution accounting and Agency
attribution, because ordinary cleanup may clear those two mutable graph
snapshots later. This allows an existing create-once score to replay without a
provider call while keeping every authority-bearing field fail-closed.
