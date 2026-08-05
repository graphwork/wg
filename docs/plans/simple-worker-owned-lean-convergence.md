# Boring convergence: worker-owned landing with a Lean 4 kernel

**Status:** proposed recovery plan. This replaces the production authority cutover in
`docs/plans/deterministic-convergence-final-cutover.md`; it does not delete the existing
proofs or incident corpus.

## Goal

Restore autonomous long-running work around one universal review valve:

```text
spawn one model under the task's explicit completion contract
→ model produces declared outputs
→ model submits an immutable, resolvable completion manifest
→ FLIP inspects the exact manifest and outputs
→ eval independently inspects the same exact manifest and outputs
→ both pass: the valve opens
→ Land publishes to main; Report/Explore publishes immutable output
→ wg done verifies review plus publication
```

**Every task passes through FLIP and eval.** They are the platform's primary mechanism
for determining whether work is correctly completed, not an optional add-on. The Lean
reducer proves the universal submit/review/accept/publish protocol. It does not schedule
models, supervise PIDs, parse streams, clean worktrees, archive tasks, choose routes, or
create recovery work. Those are ordinary adapters and UI operations.

## Non-goals

- No exactly-once claim for Linux, Git, filesystems, sockets, models, or providers.
- No proof that a model eventually succeeds or that reviewers are available.
- No automatic source retry, replacement worker, or global dispatcher pause.
- No worktree for a report, read-only analysis, or exploration task unless the task
  explicitly requests a separate repository checkout.
- No cleanup requirement for task success.
- No task may bypass FLIP or eval; reviewer infrastructure unavailability blocks the
  valve visibly without becoming semantic rejection.
- No hidden fallback between model, executor, route, graph, session, or candidate.

## One authority per concern

| Concern | Sole authority |
|---|---|
| Task readiness | Graph status plus ordinary successful `after` dependencies |
| Source work | Exact assigned worker and its branch/worktree |
| Submission identity | Content digest of the universal completion manifest |
| Review | Required FLIP and eval receipts bound to that exact manifest digest |
| Landing | Git fast-forward under one short repository landing lock |
| Code-task completion | `main` contains the manifest's reviewed candidate commit |
| Report/Explore completion | Every reviewed output locator resolves and matches its digest |
| Failure | Explicit task failure/block record; never an automatic replacement |
| Cleanup | Best-effort asynchronous maintenance; never completion authority |
| Formal truth | Pure Lean reducer plus Rust trace conformance |

`PlannerStore`, `SaveTransaction`, `FinalizationStore`, `ConvergenceState`, watchdogs,
stream observers, and wrappers must not share any of the authorities above.

## Task contracts

### Land

Use a Git worktree. The manifest points to a Git commit/tree/diff plus validation
evidence. Completion requires FLIP and eval acceptance of that manifest and the accepted
commit reachable from the configured integration branch (normally `main`). The
implementing worker owns conflict repair, validation, review feedback, and landing.

### Report

Do not create a Git worktree. Write declared outputs to the graph artifact store or an
explicit task path, then content-address them in the manifest. FLIP and eval must resolve
and inspect those exact outputs. Completion requires both reviews plus matching published
output digests; there is no fake Git promotion.

### Explore

Default to read-only project access plus a task-scoped scratch/output directory. The
manifest identifies the resulting analysis, evidence, datasets, or report artifacts for
FLIP and eval. No Git worktree or merge expectation exists unless another repository and
its delivery contract are explicit.

## Universal completion manifest

Before any task can request review, it writes one immutable manifest:

```text
CompletionManifest {
  task_id,
  generation,
  contract,                 // Land, Report, Explore
  requirements_digest,      // exact task specification reviewed
  source_revision,
  outputs : List OutputRef,
  validation : List EvidenceRef,
  worker_summary_digest
}

OutputRef :=
  | git(commit_oid, integrated_main_oid, tree_oid, diff_bundle_digest)
  | artifact(content_digest, media_type, size, immutable_locator)
  | external(adapter_kind, resource_id, before_digest, after_digest, receipt_digest)

EvidenceRef := content_digest + immutable_locator + evidence_kind
```

The manifest digest is the universal candidate identity. A locator is valid only when a
reviewer can resolve it read-only and verify its digest. A human-readable path may be
shown in the UI, but identity comes from content, not a mutable pathname.

The review bundle presented to both reviewers contains:

- the exact task requirements and completion contract;
- dependency outputs named by digest;
- the completion manifest;
- every resolved output and validation item;
- for Land, the immutable Git tree/diff without `.wg`;
- for external actions, the typed receipt plus a read-only verification probe.

Missing, mutable, inaccessible, or digest-mismatched output is `incomplete evidence` and
returns to the same worker. It is never silently accepted and is not mislabelled as a
semantic rejection.

## Universal task protocol

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

### 3. Universal FLIP/eval valve

FLIP and eval are required read-only model calls over the exact completion manifest and
resolved review bundle for **every** task contract. They receive no mutable worker
worktree. Their durable receipts are keyed by manifest digest:

```text
ReviewReceipt {
  manifest_digest,
  requirements_digest,
  reviewer_kind,       // flip or eval
  verdict,             // pass, reject, unavailable, incomplete_evidence
  findings_digest,
  inspected_output_digests,
  model_route,
  created_at
}
```

FLIP runs first as the deep adversarial completion check. Eval then independently checks
requirements, tests/evidence, and output quality against the same immutable submission.
Both are mandatory.

Rules:

- `pass`: satisfies that review slot only for that manifest and requirements digest.
- `reject`: returns bounded actionable findings to the same worker/session/work context.
  The valve stays closed.
- `incomplete_evidence`: names unresolved/mismatched outputs and returns them for repair;
  it is not a semantic verdict.
- `unavailable`: reports reviewer infrastructure failure. The valve stays closed, but
  the submission is preserved and source work is not respawned.
- Any changed output, evidence item, task requirement, commit, or manifest creates a new
  digest and invalidates both old receipts automatically.
- Review stages and findings are first-class in the parent task/TUI, but are not
  schedulable child graph tasks with independent worktrees, retries, dependencies, or
  finalizers.
- The valve opens only for `(flip = pass) ∧ (eval = pass)` on the same manifest.

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

`wg done <task>` is deliberately boring. For every task it first checks:

```text
FLIP passed exact manifest
AND eval passed exact manifest
AND every output locator still resolves to its reviewed digest
```

It then checks the contract-specific publication:

```text
Land: accepted commit is an ancestor of the integration branch
Report/Explore: accepted artifact/output digests are durably published
```

If true, the graph transitions to Done. If false, `wg done` refuses with the exact
missing condition: absent/incomplete manifest, unresolved output, stale review, FLIP or
eval rejection/unavailability, uncommitted work, main not integrated, main moved,
candidate not landed, or output not published.

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
ReviewVerdict := absent | pass | reject | unavailable | incompleteEvidence

OutputRef := git | artifact | external
Manifest := {
  id : Digest,
  requirements : Digest,
  contract : Contract,
  outputs : List OutputRef,
  evidence : List Digest,
  allResolvable : Bool,
  protectedFree : Bool
}

State := {
  phase : Phase,
  manifest : Option Manifest,
  flipManifest : Option Digest,
  flip : ReviewVerdict,
  evalManifest : Option Digest,
  eval : ReviewVerdict,
  published : Option Digest,
  failure : Option FailureCode
}
```

The pure model uses finite symbolic OIDs/digests (`Nat`) exactly as the current model
does. Git truth is supplied as a verified adapter observation, not proved by Lean.

### Events

```text
submitManifest(manifest)
recordFlip(manifestDigest, requirementsDigest, verdict)
recordEval(manifestDigest, requirementsDigest, verdict)
mainMoved(observedMain)
publishObserved(manifestDigest, publicationReceipt)
complete(manifestDigest, outputsStillResolve)
fail(code)
retry
```

A changed manifest or requirements digest resets both review slots. Review pass is
recordable only when every declared output resolves, its digest agrees, and protected
control-plane content is absent. `publishObserved` succeeds only when:

- FLIP and eval both pass the same manifest and requirements digest;
- every output and validation reference resolved during review;
- for Land, observed main equals the Git output's integrated main and the adapter reports
  successful Git CAS/fast-forward;
- for Report/Explore, the adapter reports durable publication of the reviewed digests.

`complete` succeeds only when the adapter verifies that the contract-specific published
outputs still match the accepted manifest.

### Theorems

Prove without `sorry`, `admit`, unsafe declarations, or hidden axioms:

1. **Universal review gate:** every reachable Done task has FLIP and eval pass receipts
   for the same exact manifest and requirements digest.
2. **Done implies resolved reviewed output:** every published output belongs to that
   manifest and was digest-verified.
3. **Done implies contract publication:** Land is in main; Report/Explore artifacts are
   durably published.
4. **Rejected review cannot publish or complete.**
5. **Unavailable/incomplete review cannot become semantic rejection or Done.**
6. **Manifest/output/requirements change invalidates both reviews.**
7. **Stale main cannot land:** observed main unequal to integrated main leaves the task
   nonterminal and requests no automatic spawn.
8. **At-most-one accepted publication per task generation.**
9. **Terminal is inert:** later process/wrapper/cleanup observations cannot revoke Done.
10. **Failure never satisfies dependencies.**
11. **Report/Explore completion does not require a Git worktree, but does require both
    reviews over resolvable immutable output.**
12. **No automatic source retry exists:** the event/action type contains no spawn or
    replacement-worker constructor.
13. **Conditional collision progress:** under explicit assumptions of fair lock access,
    truthful Git/output observations, successful model repair/review, and eventually
    stable publication targets, the worker can reach Done.

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

### Change C — universal manifest review valve

- Require every Land/Report/Explore attempt to submit a resolvable completion manifest.
- Replace `.flip-*`/`.evaluate-*` scheduling prerequisites with required read-only
  manifest-bound FLIP then eval calls and receipts.
- Make both passes mandatory for every contract; no bypass or optional policy path.
- Return reject/unavailable/incomplete-evidence results to the same worker context.
- Add first-class TUI manifest, output-locator, FLIP, eval, and findings status.

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

- every task submits an immutable manifest whose output locators resolve read-only;
- every Done task has FLIP and eval passes for the same manifest and requirements digest;
- every successful code task is in main;
- every successful Report/Explore task has reviewed, digest-matching published output;
- conflicts return to the same worker;
- stale candidates cannot land;
- FLIP/eval failures visibly block the universal valve;
- reviewer outage or incomplete evidence blocks without semantic rejection or source
  respawn;
- changing any output invalidates both review receipts;
- no successful work is stranded only in a worktree;
- no task is respawned after its reviewed output is published;
- Report/Explore tasks complete without worktrees but never without review;
- daemon restart after Git land/before graph Done converges by ancestry;
- service has no `wait until None`, planner effect journal, or hidden retry timer.

Only after this canary should autonomous service operation resume on the development
graph.

## Rollback

The preserved tag `recovery/overengineered-cutover-20260805` retains the current planner/
SaveTransaction implementation and evidence. Each production cutover change must be
revertible independently. The service remains stopped until Change F passes.
