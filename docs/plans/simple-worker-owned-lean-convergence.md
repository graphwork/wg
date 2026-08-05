# Boring convergence: worker-owned landing with a Lean 4 kernel

**Status:** proposed recovery plan. This replaces the production authority cutover in
`docs/plans/deterministic-convergence-final-cutover.md`; it does not delete the existing
proofs or incident corpus.

## Goal

Restore the workflow that supported autonomous long-running work:

```text
spawn one model in one worktree
→ model works and commits
→ model integrates current main and validates
→ read-only FLIP/eval reviews the immutable commit
→ model lands its own accepted commit
→ wg done verifies that main contains it
```

The Lean reducer proves the small acceptance/landing protocol. It does not schedule
models, supervise PIDs, parse streams, clean worktrees, archive tasks, choose routes, or
create recovery work. Those are ordinary adapters and UI operations.

## Non-goals

- No exactly-once claim for Linux, Git, filesystems, sockets, models, or providers.
- No proof that a model eventually succeeds or that reviewers are available.
- No automatic source retry, replacement worker, or global dispatcher pause.
- No worktree for a report, read-only analysis, or exploration task unless the task
  explicitly requests a separate repository checkout.
- No cleanup requirement for task success.
- No hidden fallback between model, executor, route, graph, session, or candidate.

## One authority per concern

| Concern | Sole authority |
|---|---|
| Task readiness | Graph status plus ordinary successful `after` dependencies |
| Source work | Exact assigned worker and its branch/worktree |
| Candidate identity | Git commit OID |
| Review | FLIP/eval receipts bound to that exact commit OID |
| Landing | Git fast-forward under one short repository landing lock |
| Code-task completion | `main` contains the reviewed candidate commit |
| Report completion | Declared immutable output exists and matches its digest |
| Failure | Explicit task failure/block record; never an automatic replacement |
| Cleanup | Best-effort asynchronous maintenance; never completion authority |
| Formal truth | Pure Lean reducer plus Rust trace conformance |

`PlannerStore`, `SaveTransaction`, `FinalizationStore`, `ConvergenceState`, watchdogs,
stream observers, and wrappers must not share any of the authorities above.

## Task contracts

### Land

Use a Git worktree. Completion requires an accepted candidate commit reachable from the
configured integration branch (normally `main`). The implementing worker owns conflict
repair, validation, review feedback, and landing.

### Report

Do not create a Git worktree. Write the declared output to the graph output store or an
explicit task path. Completion requires the output digest. A report may have FLIP/eval,
but never a fake Git promotion.

### Explore

Default to read-only access to the project plus a task-scoped scratch/output directory.
No Git worktree and no merge expectation. If an exploration genuinely modifies another
repository, that repository and its delivery contract must be explicit.

## Code-task protocol

### 1. Spawn

The daemon performs a direct capacity check and starts the selected exact route. It
records task ID, worker ID, branch, worktree, model, session, and PID for visibility.
There is no planner effect, spawn journal, route fallback, or automatic retry.

A spawn failure marks the task failed with the command, route, and error. The operator
may explicitly retry.

### 2. Work and prepare candidate

The worker edits and commits in its worktree. Before requesting review it runs:

```text
git fetch/update local refs as configured
git merge <integration-branch> into worker branch
resolve conflicts
run task validation
commit the resulting tree
```

The immutable candidate is:

```text
Candidate {
  task_id,
  generation,
  commit_oid,
  integrated_main_oid,
  tree_oid,
  validation_digest,
  protected_control_plane_free
}
```

Candidate identity is the Git commit OID; no parallel candidate ID is necessary.

### 3. FLIP/eval gate

FLIP and eval are read-only model calls over the exact candidate commit/tree. They are
not graph tasks and receive no worktree. Their durable receipts live under one review
record keyed by candidate OID:

```text
ReviewReceipt {
  candidate_oid,
  reviewer_kind,       // flip or eval
  verdict,             // pass, reject, unavailable
  findings_digest,
  model_route,
  created_at
}
```

Rules:

- `pass`: satisfies that review slot for only that candidate OID.
- `reject`: returns findings to the same worker/session/worktree. Landing is blocked.
- `unavailable`: reports infrastructure failure. Landing is blocked, but the candidate
  is not semantically rejected and source work is not respawned.
- Any new commit invalidates all old review receipts by identity.
- Review records are visible in the TUI, but do not participate in graph readiness as
  synthetic `.flip-*`/`.evaluate-*` dependencies.

### 4. Worker-owned landing

After all required reviews pass, the same worker runs `wg land <task>`.

The command:

1. checks the caller/task/worktree binding;
2. checks that the worktree is clean and `HEAD` is the reviewed candidate;
3. checks that required review receipts pass for that exact OID;
4. acquires one short repository landing lock;
5. reads the current integration-branch OID;
6. requires it to equal `candidate.integrated_main_oid`;
7. fast-forwards the integration branch/root checkout to the candidate;
8. releases the lock;
9. records a compact Git-derived landing receipt.

If step 6 fails, no state becomes successful. The worker merges the new main, resolves,
revalidates, obtains reviews for the new commit, and retries. With 8–10 workers this may
cause bounded collision retries, but it cannot create a deadlock or hidden replacement.

Git owns ref atomicity. The landing lock only serializes root-checkout synchronization
and the compare/fast-forward critical section.

### 5. Done

`wg done <task>` is deliberately boring. For a Land task it checks:

```text
review receipts pass for candidate OID
AND candidate OID is an ancestor of integration branch
```

If true, the graph transitions to Done. If false, `wg done` refuses with the exact
missing condition: uncommitted work, main not integrated, review rejected/unavailable,
main moved, or candidate not landed.

A later wrapper error, observer error, provider report, cleanup error, or process exit
cannot undo Done. Cleanup is queued as best effort and is not a dependency truth.

## Failure and retry semantics

- Model/process exits before landed completion: mark Failed and retain branch/worktree.
- Review rejects: keep the same live worker for repair when possible; otherwise record
  `BlockedReview` with findings and retain the branch/worktree.
- Review unavailable: record `ReviewUnavailable`; no semantic failure and no source
  replacement.
- Main moved: return `NeedsRebase` to the same worker; no daemon-created repair task.
- Merge conflict: worker resolves it in place or explicitly fails.
- Explicit retry resumes the retained branch/worktree; it never silently creates a new
  source generation while the old owner is live.
- Failed dependencies visibly block descendants. No automatic prerequisite repair.

The TUI must show a first-class **Unlanded work** section containing task, worker,
branch, worktree, HEAD, main integration status, review status, and last failure.

## Lean 4 fit

### Preserve existing work

Keep the current modules and fixtures as historical specifications:

- `WGLifecycle.Model` / `Safety` v1 contain useful candidate/base-CAS, fencing,
  terminal-inertness, and at-most-once ideas.
- `WGLifecycle.V2` and `DaemonPlanner` document the over-complex design and permanent
  incidents. They remain buildable but are not the production wire contract.
- Existing incident fixtures remain regression inputs for adapters and migration.

Do not reinterpret old schemas. Add a new, small module:

```text
formal/WGLifecycle/SimpleLand.lean
```

### Pure state

```text
Contract := land | report | explore
Phase := working | reviewBlocked | reviewUnavailable | accepted | done | failed
ReviewVerdict := absent | pass | reject | unavailable

Candidate := {
  commit : Oid,
  integratedMain : Oid,
  validation : Digest,
  protectedFree : Bool
}

State := {
  phase : Phase,
  candidate : Option Candidate,
  flip : ReviewVerdict,
  eval : ReviewVerdict,
  landed : Option Oid,
  failure : Option FailureCode
}
```

The pure model uses finite symbolic OIDs/digests (`Nat`) exactly as the current model
does. Git truth is supplied as a verified adapter observation, not proved by Lean.

### Events

```text
prepareCandidate(candidate)
recordFlip(candidateOid, verdict)
recordEval(candidateOid, verdict)
mainMoved(observedMain)
landObserved(candidateOid, expectedMain, observedMain, casSucceeded)
complete(mainContainsCandidate)
fail(code)
retry
```

A changed candidate resets both review slots. `landObserved` succeeds only when:

- candidate is protected-control-plane-free;
- validation is present;
- both required reviews pass for that exact candidate;
- `observedMain = candidate.integratedMain`;
- the adapter reports successful Git CAS/fast-forward.

`complete` succeeds only when the verified adapter observation says the integration
branch contains the accepted/landed candidate.

### Theorems

Prove without `sorry`, `admit`, unsafe declarations, or hidden axioms:

1. **Done implies landed:** every reachable Done Land task has a landed candidate.
2. **Done implies exact review:** FLIP/eval pass receipts name that candidate OID.
3. **Rejected review cannot land.**
4. **Unavailable review cannot become semantic rejection or Done.**
5. **Candidate change invalidates review.**
6. **Stale main cannot land:** observed main unequal to integrated main leaves the task
   nonterminal and requests no automatic spawn.
7. **At-most-one accepted landing per task generation.**
8. **Terminal is inert:** later process/wrapper/cleanup observations cannot revoke Done.
9. **Failure never satisfies dependencies.**
10. **Report/Explore completion does not require a Git worktree.**
11. **No automatic source retry exists:** the event/action type contains no spawn or
    replacement-worker constructor.
12. **Conditional collision progress:** under explicit assumptions of fair lock access,
    truthful Git observations, successful model repair/review, and eventually stable
    main, the worker can reach Done.

The last theorem is conditional. Lean must not claim that models, reviewers, Git, or the
OS eventually cooperate.

### Rust conformance

Add one versioned trace schema solely for this reducer. It contains task/candidate/review
identities and bounded result codes, not logs or paths. Both Lean fixtures and Rust replay:

- happy Land;
- FLIP rejection then same-worker repair;
- evaluation unavailable;
- candidate change invalidates prior pass;
- two workers race on the same expected main;
- main moves before landing;
- crash after Git fast-forward but before `wg done`;
- model exits after landing;
- wrapper fails after Done;
- Report completion without worktree;
- Explore completion without worktree;
- `.wg` candidate rejection.

The production daemon does not need to persist planner effects to use this reducer.
Commands append compact transition records for audit/replay; Git and graph files remain
the operational sources of truth.

## Crash recovery

Only one crash boundary needs special handling: Git landed the candidate but graph Done
was not written. Recovery is derived, not scheduled:

```text
if task has accepted candidate C
and integration branch contains C
then `wg done` (or startup projection) may idempotently mark Done
```

If main does not contain C, no success is inferred. There is no 13-phase save replay.
The worker branch/worktree remains the recovery artifact for every pre-land crash.

## Production cutover

The service is currently stopped. Cut over in changes that never permit dual authority.

### Change A — formal/simple kernel only

- Add `SimpleLand.lean`, Rust reducer, fixtures, and conformance tests.
- Keep it disconnected from production.
- Run `lake build`, Rust conformance, and proof-escape scans.

### Change B — direct spawn and task contracts

- Restore direct capacity/readiness dispatch.
- Remove PlannerStore authorization from spawn/route paths.
- Disable automatic retries and global provider pause.
- Add explicit Land/Report/Explore execution behavior.
- Keep exact route selection and visible failures.

### Change C — synchronous candidate review

- Replace `.flip-*`/`.evaluate-*` scheduling prerequisites with read-only candidate-bound
  review calls and receipts.
- Return reject/unavailable results to the same worker.
- Add TUI review status and findings.

### Change D — worker-owned land/done

- Add `wg land` and the short repository lock/CAS adapter.
- Make `wg done` verify review plus Git ancestry.
- Remove SaveTransaction/FinalizationStore/GraphSave from the Land success path.
- Make cleanup asynchronous and non-authoritative.

### Change E — delete obsolete production authority

- Remove PlannerStore dispatch/finish authority, ConvergenceState scheduling,
  SaveTransaction progression, exited-worker source respawn, and cleanup-gated Done.
- Keep historical readers/migration only where necessary; no dormant alternative runtime
  mode or feature flag.

### Change F — clean-room canary

In an isolated HOME and fresh graph, run 8–10 concurrent agents that edit overlapping and
non-overlapping files. Assert:

- every successful code task is in main;
- conflicts return to the same worker;
- stale candidates cannot land;
- FLIP/eval failures visibly block landing;
- reviewer outage creates ReviewUnavailable without source respawn;
- no successful work is stranded only in a worktree;
- no task is respawned after its candidate reaches main;
- Report/Explore tasks complete without worktrees;
- daemon restart after Git land/before graph Done converges by ancestry;
- service has no `wait until None`, planner effect journal, or hidden retry timer.

Only after this canary should autonomous service operation resume on the development
graph.

## Rollback

The preserved tag `recovery/overengineered-cutover-20260805` retains the current planner/
SaveTransaction implementation and evidence. Each production cutover change must be
revertible independently. The service remains stopped until Change F passes.
