# Worker-owned completion with a universal FLIP/eval valve

**Status:** implemented normative protocol. The isolated ten-worker recovery exit canary
passed on 2026-08-05; see
`docs/reports/worker-owned-completion-exit-canary-2026-08-05.md`.

**Implementation plan:** `docs/plans/simple-worker-owned-lean-convergence.md`

**Supersedes for production authority:**
`docs/plans/deterministic-convergence-final-cutover.md`. The existing planner,
SaveTransaction, and Lean incident work remains preserved as historical evidence; it is
not the target runtime protocol.

## 1. Purpose

WG exists to run autonomous work and determine whether the resulting work is correctly
completed. The normal path MUST remain understandable from one screen:

```text
one worker owns the work
→ worker submits exact outputs
→ FLIP and eval inspect those outputs
→ both pass
→ worker publishes/lands the reviewed outputs
→ Done
```

A successful model call is not completion. A commit alone is not completion. A review
without resolvable output is not completion. Cleanup is not completion. Completion means
that the exact reviewed outputs are durably published under the task's declared contract.

## 2. Non-negotiable invariants

1. **One source owner.** One task attempt has at most one source worker, session, branch,
   and worktree. Reviewers never become source owners.
2. **Universal review.** Every Land, Report, and Explore task MUST pass FLIP and eval.
3. **Exact output identity.** Both reviewers inspect the same immutable completion
   manifest and the same digest-verified outputs.
4. **Worker-owned repair.** Review rejection, missing evidence, merge conflict, and stale
   main return to the same worker context. WG MUST NOT spawn replacement source work
   automatically.
5. **Worker-owned landing.** A Land worker integrates current main, resolves conflicts,
   validates, obtains review, and lands its own accepted commit.
6. **Done is derived.** `Done` is allowed only when both reviews pass and the reviewed
   outputs are published under the task contract.
7. **Failure is visible.** Failure blocks dependents and names the retained work/output
   and exact next action. There is no hidden retry or global pause.
8. **Terminal success is final.** Wrapper, observer, provider, cleanup, or process errors
   after reviewed publication are diagnostic only and cannot revoke Done.
9. **Cleanup is subordinate.** Worktree/session cleanup is best effort and never gates
   completion or dependency truth.
10. **No control-plane candidate.** `.wg` and equivalent protected control-plane paths
    never enter candidate outputs, review bundles, or publication.

## 3. Task contracts

Every task declares exactly one completion contract before spawn.

### 3.1 Land

Use a Git branch/worktree. The primary output is a Git commit. Publication means the
reviewed commit is reachable from the configured integration branch, normally `main`.

### 3.2 Report

Do not create a Git worktree by default. Outputs are immutable reports/artifacts in the
WG artifact store or explicit task-owned paths. Publication means the reviewed digests
are present at their declared durable locators.

### 3.3 Explore

Use read-only project access plus a task-scoped scratch/output directory. Outputs are the
analysis, evidence, datasets, or report artifacts named in the manifest. Exploration of
another mutable repository requires an explicit repository and publication contract; it
MUST NOT create an implicit WG source worktree.

No contract bypasses FLIP or eval.

## 4. Completion manifest

The worker MUST submit one immutable completion manifest before review:

```text
CompletionManifest {
  manifest_version,
  task_id,
  generation,
  completion_contract,
  requirements_digest,
  source_revision,
  outputs,
  validation_evidence,
  worker_summary_digest
}
```

Output references are typed:

```text
GitOutput {
  commit_oid,
  integrated_main_oid,
  tree_oid,
  diff_bundle_digest
}

ArtifactOutput {
  content_digest,
  immutable_locator,
  media_type,
  size
}

ExternalOutput {
  adapter_kind,
  resource_id,
  before_digest,
  after_digest,
  operation_receipt_digest,
  verification_probe_digest
}
```

An evidence reference contains an immutable locator, digest, and evidence kind. Mutable
paths may be displayed for navigation, but they are never identity.

The canonical submission identity is the content digest of the manifest. Any change to
requirements, output, evidence, commit, or summary creates a new manifest identity and
invalidates all earlier review receipts.

## 5. Review bundle and resolver

WG materializes a read-only review bundle for the manifest. It contains:

- exact task requirements and completion contract;
- dependency outputs by digest;
- completion manifest;
- all resolved output bytes or bounded typed projections;
- all validation evidence;
- Git tree and diff for Land, excluding protected control-plane paths;
- operation receipt and read-only verification probe for ExternalOutput.

The resolver MUST verify each digest before invoking a reviewer. A missing, mutable,
inaccessible, oversized-without-projection, or digest-mismatched reference produces
`IncompleteEvidence`. It does not invoke semantic judgment and does not discard the
submission.

Reviewers MUST NOT read the worker's mutable worktree. They inspect only the materialized
bundle.

## 6. Universal FLIP/eval valve

FLIP and eval are first-class stages of the parent task. They are not synthetic graph
children and receive no source worktree, independent retry lifecycle, or finalizer.

### 6.1 FLIP

FLIP runs first. It performs the deep adversarial completion review:

- challenge whether the task requirements were actually met;
- inspect all declared output locations;
- look for missing, misleading, unsafe, or unrelated work;
- verify claimed tests/evidence against output;
- produce bounded actionable findings.

### 6.2 Eval

Eval runs after FLIP passes. It independently checks correctness, requirement coverage,
quality, regressions, and the declared validation evidence against the same manifest.

### 6.3 Receipt

```text
ReviewReceipt {
  manifest_digest,
  requirements_digest,
  reviewer_kind,       // flip | eval
  verdict,             // pass | reject | unavailable | incomplete_evidence
  findings_digest,
  inspected_output_digests,
  model_route,
  created_at
}
```

The valve opens only when:

```text
FLIP(exact manifest, exact requirements) = pass
AND
Eval(exact manifest, exact requirements) = pass
```

There is no bypass, timeout-as-pass, weak fallback, or old-receipt reuse.

- `Reject`: return findings to the same worker context.
- `IncompleteEvidence`: return unresolved evidence to the same worker context.
- `Unavailable`: preserve submission and visibly block review; do not classify semantic
  failure and do not respawn source work.

## 7. Worker-owned Land protocol

A Land worker performs this loop:

```text
work and commit
→ merge current main into worker branch
→ resolve conflicts
→ validate
→ submit manifest
→ FLIP pass
→ eval pass
→ wg land
```

`wg land`:

1. verifies task/worker/branch/worktree identity;
2. requires a clean worktree at the reviewed commit;
3. resolves and rechecks both review receipts;
4. acquires one short repository landing lock;
5. reads current main;
6. requires current main to equal `integrated_main_oid`;
7. fast-forwards main/root checkout to the reviewed commit;
8. releases the lock;
9. records a compact landing receipt.

If main moved, no success is recorded. The same worker merges the new main, validates,
submits a new manifest, reruns both reviews, and retries. Review invalidation after a
merge is intentional: the candidate tree changed.

Git owns atomic ref updates. The lock only serializes compare/fast-forward and root
checkout synchronization. Eight to ten workers may create bounded merge/review retries;
they MUST NOT create replacement source workers or permanent waits.

## 8. Completion

`wg done` verifies the universal gate and contract publication.

Universal checks:

```text
manifest resolves
FLIP passed exact manifest and requirements
eval passed exact manifest and requirements
all reviewed output digests still agree
```

Contract checks:

```text
Land: reviewed commit is reachable from integration branch
Report: reviewed artifact digests are durably published
Explore: reviewed output/evidence digests are durably published
```

On success, graph status becomes Done and successful dependencies unblock. On failure,
`wg done` returns the exact unsatisfied predicate. It does not create a recovery task.

A crash after Git publication but before graph status is recoverable by the same derived
check: if the exact accepted commit is in main and both exact review receipts pass,
repeating `wg done` is idempotent.

## 9. Failure, blocking, and retry

- Process/model failure before Done: task becomes Failed; branch/worktree/output remains.
- Review rejection: task remains owned for repair or becomes visibly `ReviewBlocked` if
  the worker exits.
- Review unavailable: visibly `ReviewUnavailable`; no source replacement.
- Main moved: visibly `NeedsRebase`; same worker handles it.
- Failed dependency: descendants remain visibly blocked.
- Retry is explicit. It resumes retained source state when safe.
- No automatic source retry, route fallback, prerequisite-repair task, or global service
  pause exists in the normal protocol.

The TUI MUST show an Unlanded/Unpublished section with task, contract, worker, branch or
artifact locator, manifest, FLIP verdict, eval verdict, publication status, and last
failure.

## 10. Lean 4 convergence kernel

Lean proves the protocol above, not daemon scheduling.

The target module is `formal/WGLifecycle/SimpleLand.lean`. Its pure state contains:

```text
Contract := land | report | explore
Phase := working | reviewBlocked | reviewUnavailable | accepted | done | failed
ReviewVerdict := absent | pass | reject | unavailable | incompleteEvidence
State := manifest + flip receipt + eval receipt + publication receipt + phase
```

Events are limited to submission, review recording, publication observation, completion,
failure, and explicit retry. There is no spawn, timer, replacement-worker, cleanup,
archive, route-probe, or stream event in the formal action type.

Required theorems:

1. Done implies both reviews passed the exact manifest and requirements.
2. Done implies every published output belongs to and matches that manifest.
3. Done implies contract-specific publication.
4. Reject, unavailable, incomplete evidence, stale review, and stale main cannot Done.
5. Manifest or requirement change invalidates both review receipts.
6. At most one publication is accepted per task generation.
7. Failure does not satisfy dependencies.
8. Terminal success is inert under later diagnostic observations.
9. Report/Explore require review but not Git worktrees.
10. No automatic source replacement is expressible.
11. Conditional progress is proved only under explicit truthful-adapter, fair-lock,
    reviewer-availability, model-repair, and stable-publication assumptions.

Git, filesystem, output resolution, model behavior, and process truth remain adapter
assumptions tested in Rust. The existing V1/V2/DaemonPlanner proofs remain buildable as
historical models but MUST NOT retain production authority.

## 11. Implementation recovery mode

Recovery mode exited after the clean-room canary passed on 2026-08-05. The bootstrap
rules below are retained as the historical implementation protocol:

- WG service remains stopped on this repository.
- Implementation proceeds from one attended agent session directly in the repository.
- Do not use the WG graph to dispatch implementation or validation workers.
- Do not install a worktree candidate globally or restart the repository daemon.
- Run candidate binaries directly against isolated temporary graphs and HOME directories.
- Use one build target; do not create per-agent multi-gigabyte target directories.
- Preserve the recovery tag and historical worktrees read-only.
- Make small reviewable commits; each commit removes any legacy authority it replaces.
- Do not add a compatibility mode that leaves old and new production authorities live.

This attended recovery implementation is a bootstrap exception because the universal
valve does not yet exist and the current graph review path is unavailable. Before the
service can resume, the new candidate MUST dogfood its own manifest resolver, FLIP, eval,
publication, and Done checks in the exit canary. No later production task is exempt.

## 12. Exit from recovery mode

The isolated ten-worker canary passed on 2026-08-05; the evidence and exit decision are
recorded in `docs/reports/worker-owned-completion-exit-canary-2026-08-05.md`. The service
was permitted to resume after the canary proved:

- every task submits a resolvable immutable manifest;
- every Done task has exact FLIP and eval pass receipts;
- every Done Land output is in main;
- every Done Report/Explore output is durably published;
- review failure/unavailability blocks visibly without source respawn;
- merge collisions return to the same worker;
- changed output invalidates review;
- crash after publication/before Done recovers by derived checks;
- no successful work is stranded only in a worktree;
- no `wait until None`, hidden retry, planner effect journal, or cleanup-gated Done exists.

Only then may the temporary single-session restriction be removed from the project agent
guide.
