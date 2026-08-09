# Simple Local WG Recovery Work Document

**Status:** ACTIVE — Phases 1, 2, 4, and 5 validated; WG dispatch remains stopped pending full canary/install
**Started:** 2026-08-09  
**Working branch:** `rescue/simple-local-wg`  
**Owner:** repository operator  
**Source of truth:** this document. Update its checkboxes and checkpoint log as work progresses.

## 1. Mission

Restore the simple, useful local WorksGood contract:

1. A user asks for work.
2. Chat creates a task graph.
3. Trusted local workers fan out and coordinate freely through that graph.
4. Workers finish through one ordinary operation.
5. FLIP/evaluation inspect the exact result and provide useful findings.
6. A real defect causes a bounded repair; infrastructure trouble retries neutrally.
7. Tasks become Done without hidden controller deadlocks or manual object-store surgery.

This is a **semantic rollback**, not a blind historical Git revert. Retain integrity mechanisms that help—worktree isolation, attribution, stale-attempt fencing, immutable evidence, accounting, and recoverability—but remove default permission and completion machinery that prevents trusted local work.

## 2. Why recovery mode is necessary

Observed on the 2026-08-08/09 audit run:

- The trust-first fix passed its substantive validation, then accumulated at least 10 consecutive FLIP rejections.
- Review-driven repairs expanded the trust branch to 15 commits and approximately 3,653 inserted lines, including new “seal” and “fail closed” controls contrary to the requested simplification.
- The build-admission fix initially passed build, format, clippy, targeted tests, install, and owned smokes, then accumulated at least 20 review rejections and expanded to 28 commits and approximately 1,628 inserted lines.
- `audit-sync-roadmap` produced an accepted and landed candidate, but later generation drift prevented Done.
- A malformed/stale worker capability path prevented a dead worker from even recording its own failure.
- Agents died while tasks remained `InProgress`.
- The documented human `wg done --skip-verify --skip-smoke` escape path was rejected with `legacy wg done bypass/merge/cycle flags are not supported by publication-derived completion`.
- Trusted local coordination was refused as `worker_control.cross_task_refused`.
- A Pi worker could fail before its first authorized turn because control IPC timed out, watchdog state was absent, and the capability was classified stale.

These are control-plane failures, not failures of the requested source, audit, or scientific work.

Relevant audits:

- [`../audit/2026-08-08-worksgood-system/11-orchestration-lifecycle.md`](../audit/2026-08-08-worksgood-system/11-orchestration-lifecycle.md)
- [`../audit/2026-08-08-worksgood-system/23-evaluation-evolvability-cutover.md`](../audit/2026-08-08-worksgood-system/23-evaluation-evolvability-cutover.md)
- [`../audit/2026-08-08-worksgood-system/30-contradiction-and-drift-register.md`](../audit/2026-08-08-worksgood-system/30-contradiction-and-drift-register.md)
- [`../audit/2026-08-08-worksgood-system/31-documentation-sync-plan.md`](../audit/2026-08-08-worksgood-system/31-documentation-sync-plan.md)

Historical concentration of complexity:

| Date | Commit | Change |
|---|---|---|
| 2026-07-28 | `0dd48b92` | Lazy candidate evaluation evidence |
| 2026-07-31 | `9beddfcb` | Task-agent-owned finish transactions |
| 2026-08-02 | `5b0e67b4` | Scoped worker-control IPC broker; 1,807-line initial change |
| 2026-08-05 | `6ac127a4` | Manifest-bound FLIP/eval completion valve |
| 2026-08-07 | `76fbe614` | Synthetic agency-task authority removed |

The local authority baseline should resemble the behavior before `5b0e67b4`, while exact review receipts may remain as internal evidence rather than worker-facing ceremony or reviewer lifecycle authority.

## 3. Non-negotiable target contract

### 3.1 Actor policy

| Actor | Default authority |
|---|---|
| Human/operator | Full project graph authority |
| Attended chat | Full project graph coordination |
| Ordinary local worker | Trusted project graph coordination |
| FLIP/evaluator/reviewer | Observation and findings only |
| Remote/federated worker | Explicit scoped capability |
| Hostile inbound content | Quarantined/read-only until accepted |

For trusted local workers, `target_task != source_task` must not itself be a refusal reason. Normal graph operations—inspect, add, edit, link, assign, reprioritize, message, pause/resume, and coordinate—run through the public WG commands with actor attribution.

### 3.2 Integrity retained

- Every mutation remains actor- and attempt-attributed.
- Stale/reaped attempts remain fenced from later writes.
- Immutable candidates, findings, review receipts, and completion receipts cannot be rewritten.
- Graph writes use existing atomic/recoverable storage boundaries.
- Remote and observation-only actors remain scoped.
- Token, model, route, timing, and cost evidence remain durable and queryable.

### 3.3 Permission machinery removed from the local default

- No own-task-only graph restriction for ordinary local workers.
- No mandatory live control-plane round trip for every Pi continuation.
- No hidden secondary build concurrency limit.
- No mandatory quality-pass dependency.
- No model reviewer with unbounded source-task blocking authority.
- No infrastructure outage classified as source/scientific failure.
- No requirement for workers to manually orchestrate `submit → land → done`.

## 4. Completion and review policy

### 4.1 One ordinary worker operation

Prefer retaining `wg done` as the single worker-facing completion operation rather than adding another command. Internally it may:

1. snapshot the exact candidate;
2. record deterministic validation evidence;
3. run FLIP and evaluation;
4. publish/land the accepted candidate;
5. derive Done.

The worker should not need to understand or manually invoke completion object, manifest, submit, land, receipt, or CAS internals.

### 4.2 Gate hierarchy

1. **Hard:** deterministic required validation explicitly stated by the task.
2. **Hard when explicitly requested:** an operator-selected strict review policy.
3. **Advisory/bounded by default:** model FLIP and evaluation for ordinary trusted local work.

Model review may detect a real problem and request repair, but it may not invent requirements beyond the task description and `## Validation` contract.

### 4.3 Bounded behavior

- Persist and show every review attempt and its exact findings.
- Permit at most two model-review repair rounds for one task generation.
- A semantic rejection with actionable findings returns the task to an unowned repairable state.
- Continued disagreement becomes visible `Needs review` activity; it does not retain a dead worker or spin indefinitely.
- Reviewer timeout, malformed response, provider outage, route failure, and control-plane failure never count as source quality failure.
- A human/operator can accept or reject with a required audit reason.
- An operator escape must actually work and must append evidence rather than bypassing history silently.

## 5. Task and activity UX

The graph remains task-centric. Internal activity is shown parenthetically rather than masquerading as graph tasks:

```text
audit-charter    Done    (assign ✓ · flip ✓ · flipx ✓ · eval ✓ · 2 attempts)
```

Expanding the task exposes the complete hierarchy without flattening distinct concepts:

```text
Task
└─ lifecycle generation
   └─ execution attempt + fence
      └─ immutable candidate
         ├─ FLIP attempt(s)
         ├─ FLIP comparison attempt(s)
         ├─ evaluation attempt(s)
         └─ publication/completion evidence
```

Structural cycle iteration, lifecycle generation, execution attempt, candidate revision, and review attempt must remain separately named.

## 6. Recovery progress

### Phase 0 — Contain and preserve

- [x] Identify completion/control-plane deadlock as systemic rather than task-specific.
- [x] Stop all active workers.
- [x] Stop the WG service/dispatcher; do not dispatch more recovery work through the broken path.
- [x] Preserve current `main` and the runaway worker branches.
- [x] Create operator-owned branch `rescue/simple-local-wg`.
- [x] Establish this checked work document as the recovery source of truth.
- [x] Capture external graph-state archive and Git bundle.
- [x] Record archive checksums.
- [x] Keep the existing `.wg` graph as forensic evidence; do not rewrite it during bootstrap.

Recovery snapshot:

```text
/home/bot/wg-recovery/2026-08-09-pre-simple-local/
├── wg-state.tar.zst
├── preserved-refs.bundle
├── SHA256SUMS
└── README.txt
```

Snapshot notes: the service and workers were stopped. The attended chat remained active, so chat/usage tail files are crash-consistent; graph state and immutable objects were copied through a staging directory before archive creation.

```text
ca42057e11cf9401ffffb16419b75f599f6818b71792afdd3c0bb42d6761727e  wg-state.tar.zst
ef8719a423d00a0be98eb7c1e4297751e1c0dca32348b9e55f3238b509d01c9b  preserved-refs.bundle
```

### Phase 1 — Establish a minimal operator bootstrap

- [x] Add a real operator completion/reconciliation escape that works even when the model-review valve or task generation bookkeeping is broken.
- [x] Bound review retries and prevent a reviewer from keeping a dead worker `InProgress`.
- [x] Classify pre-execution control failure as neutral startup deferral.
- [x] Ensure dead worker reconciliation releases task ownership deterministically.
- [x] Add a configuration/default that makes local model review advisory while preserving explicit strict mode.
- [x] Prove the bootstrap in an isolated clean graph before touching the preserved graph.

**Phase 1 exit:** a human can recover and complete a validated local task without editing graph JSON, object-store files, service registries, or lifecycle ledgers manually.

### Phase 2 — Restore trust-first local coordination

- [x] Review the earliest validated trust-first checkpoint; do not merge the latest scope-expanded branch wholesale.
- [x] Extract only behavior directly required by the target contract.
- [x] Make trusted local workers the default when no explicit strict scope is configured.
- [x] Permit normal cross-task graph coordination through public WG commands.
- [x] Keep evaluator/reviewer, remote, and explicit scoped actors constrained.
- [x] Remove live coordinator availability as a prerequisite for every continuation of a valid local lease.
- [x] Provide a worker-readable capability/status view before execution begins.
- [x] Run the historical trust-first regression and a current local coordination smoke.

**Phase 2 exit:** a local worker can create/edit/link/message downstream work and survive temporary coordinator IPC loss without weakening stale-owner fences or immutable evidence.

### Phase 3 — Simplify completion

- [x] Make one worker-facing completion operation own candidate snapshot, review, publication, and Done.
- [x] Return exact findings immediately on repairable rejection.
- [x] Preserve rejected and superseded candidates without forcing repeated manual manifests.
- [x] Separate source-worker, assignment, FLIP, and evaluation accounting.
- [x] Add the bounded repair/`Needs review` behavior from §4.3.
- [x] Remove or hide worker-facing completion ceremony from prompts and normal help.
- [x] Prove infrastructure-unavailable review does not fail or indefinitely block the source task.

**Phase 3 exit:** one command completes normal work; one real semantic defect can be repaired; model or control-plane failure cannot create an infinite loop.

### Phase 4 — Remove surprise scheduler policy

- [x] Make unset build-heavy capacity inherit `max_agents`.
- [x] Preserve only explicit lower operator overrides.
- [x] Keep predictive disk admission opt-in.
- [x] Show inherited/explicit capacity and waiting reasons in CLI/JSON/TUI.
- [x] When disk prediction is disabled, render headroom as unavailable rather than `Healthy — 0.0 GiB`.
- [x] Review the earliest validated build-admission checkpoint; do not merge the 28-commit review-expanded branch wholesale.

**Phase 4 exit:** absent explicit configuration, worker concurrency is governed by `max_agents` alone.

### Phase 5 — Remove mandatory quality-pass behavior

- [x] Remove “always create a quality pass for 2+ tasks” from the default chat/agent contract.
- [x] Make quality pass explicitly requested or advisory only.
- [x] If used, run it as a trusted local coordinator rather than a brokered cross-task exception.
- [x] On quality-pass infrastructure failure, release original tasks unchanged with a warning.
- [x] Do not use hard dependency edges that permanently block a batch on advisory metadata optimization.

**Phase 5 exit:** ordinary fan-out needs no preparatory meta-task; an optional quality pass can help but cannot gum up execution.

### Phase 6 — Task-centric activity and history

- [x] Show compact assignment/FLIP/eval activity on the parent task row.
- [x] Expose full generation/attempt/candidate/review history in CLI, JSON, and TUI.
- [x] Make findings, model route, usage, cost, timing, semantic reject, and infrastructure failure queryable.
- [x] Ensure activities have no graph dependency or source lifecycle authority.
- [ ] Reconnect accepted terminal outcomes to agency learning through a separate exactly-once observation projection only after the local path is stable.

**Phase 6 exit:** review is visible and useful without synthetic task noise or reviewer lifecycle authority.

### Phase 7 — Canary, install, and reconcile

- [x] Build candidate binary without changing the global installation.
- [x] Run the clean-graph golden-path canary in §8.
- [x] Run focused stale-owner, immutable-receipt, and remote-scope safety tests.
- [x] Run `cargo fmt --check`, targeted tests, and `cargo clippy`.
- [x] Install with `cargo install --path . --locked` only after the canary passes.
- [x] Start the service against a disposable copy of the preserved graph and run reconciliation dry-run.
- [x] Reconcile already-landed audit artifacts without re-reviewing identical bytes.
- [ ] Mark runaway trust/build task histories superseded by the operator rescue rather than pretending their latest review-expanded branches were accepted.
- [ ] Resume the real graph only after status, dispatch, completion, review findings, and operator recovery are proven.
- [ ] Reclaim stale worktree targets only after refs and external snapshot checksums are reverified.

**Phase 7 exit:** the installed binary completes a real bounded fan-out, and the preserved project graph resumes without manual internal-state edits.

### Phase 8 — Delete dormant complexity

Do this only after the simple path is stable.

- [ ] Remove obsolete `PendingEval`/`FailedPendingEval` and synthetic-satellite compatibility paths where migration evidence permits.
- [ ] Remove local own-task-only capability branches no longer used by explicit scoped actors.
- [x] Remove worker prompt instructions for retired completion commands.
- [ ] Remove dead assignment/evaluator/evolver paths or label and isolate intentionally manual compatibility commands.
- [ ] Publish one authority map for task, attempt, candidate, activity, review, publication, and learning.
- [ ] Add a complexity budget: any new hard gate needs an explicit user-selected policy, bounded failure behavior, visible state, and a tested operator escape.

## 7. Preserved Git evidence

Snapshot at recovery start:

| Ref | Commit | Use |
|---|---|---|
| `main` | `e12ee37c7bd606900af10c2baeed3cbd08dd225d` | Preserved integration baseline |
| `rescue/simple-local-wg` | `e12ee37c7bd606900af10c2baeed3cbd08dd225d` | Operator recovery branch |
| `wg/agent-23/restore-trust-first-local-worker-control` | `259f03f21f0b53ddaac081024f3536e225366e9c` | Trust implementation and review-induced scope-growth evidence |
| `wg/agent-24/fix-build-admission-default-ui` | `da9ab02c23129417c24d457f9663e2ad2293a9f5` | Admission implementation and review-induced scope-growth evidence |
| `wg/agent-21/audit-sync-roadmap` | `3c2f0f9f639d724e888eb7cae79cddf22ba99e32` | Audit-roadmap task history; reviewed artifact already integrated |

Candidate checkpoints for inspection—not automatic merge instructions:

- Trust-first initial implementation: `5a852b21`; inspect together with its accounting-main merge context before extracting.
- Build-admission initial implementation: `20dbb11a`.
- Build-admission early smoke repair: `753ed6c9`.

Rule: no branch is merged wholesale merely because later commits claim to close reviewer findings.

## 8. Golden-path canary

Run in a clean temporary HOME and graph with fake providers and isolated worktrees:

1. Configure one local route and start WG.
2. Create a three-task fan-out from one user request.
3. Let worker A inspect and improve worker B’s task metadata.
4. Let worker B create/link a legitimate child task.
5. Restart the daemon before worker C’s first authorized model turn.
6. Confirm worker C starts automatically after recovery with no source failure/retry charge.
7. Complete one task normally.
8. Inject one deterministic real defect; confirm FLIP/eval produces visible findings and one repair succeeds.
9. Inject reviewer/provider unavailability; confirm the source does not fail and no worker spins.
10. Confirm every real task reaches Done or a visible unowned repair state.
11. Confirm default list/TUI contains one row per real task with parenthetical activity.
12. Confirm expanded history exposes all attempts/candidates/reviews and separate costs.
13. Confirm an explicit scoped/remote worker still cannot mutate an unrelated task.
14. Confirm the operator recovery command works and appends a reasoned audit event.

Required final assertion:

```text
request → fan-out → trusted coordination → restart recovery
→ bounded review → repair → completion
```

No hidden satellite dependencies, indefinite review loops, control-plane source failures, unexplained admission deferrals, or manual CAS/service-registry edits.

## 9. Complexity and stop rules

Stop and reassess before merging any change that introduces one of the following into the ordinary local path:

- a new protocol version;
- a new capability broker or per-operation allowlist;
- a new terminal task status;
- another background authority required for a local worker to proceed;
- an unbounded retry or model-review loop;
- a hard gate with no functioning operator escape;
- a hidden concurrency/resource policy;
- requirements not present in the user task or its validation contract;
- more code dedicated to proving a controller’s authority than to completing the user’s work.

Prefer deletion, bypass, and reuse of ordinary WG commands over new abstractions.

## 10. Decision log

| Date | Decision | Reason |
|---|---|---|
| 2026-08-09 | Freeze WG dispatch and stop all workers. | The system was spending indefinitely inside completion/control loops. |
| 2026-08-09 | Use an operator-owned Git branch, not another WG task. | The broken task completion path cannot be the authority for its own repair. |
| 2026-08-09 | Perform semantic rollback rather than raw repository reset. | Preserve useful unrelated work and integrity evidence while restoring the old behavioral contract. |
| 2026-08-09 | Trusted local workers regain broad graph coordination by default. | Local workers are first-class collaborators; remote/reviewer threat models must not govern them. |
| 2026-08-09 | Model review is bounded/advisory by default; deterministic task validation remains hard. | Strong models can detect issues, but cannot own infinite lifecycle authority or invent scope. |
| 2026-08-09 | Keep review embedded in parent-task UX, not as authoritative graph tasks. | Restores visibility without reviving satellite deadlocks and graph inflation. |
| 2026-08-09 | Do not merge the latest trust/build branches wholesale. | Their size and policy direction were materially driven by repeated reviewer demands. |

## 11. Checkpoint log

### 2026-08-09 — Recovery initialized

- Service stopped; zero workers alive.
- Working branch created at `e12ee37c`.
- Existing graph preserved in place and externally archived.
- Git refs bundled and checksummed.
- Runaway task branches retained as evidence.
- Audit draft and roadmap files already present on the integration baseline; task statuses remain forensic evidence and are not treated as product truth.
- Next action: implement Phase 1 operator bootstrap directly on `rescue/simple-local-wg`.

### 2026-08-09 — Trusted-local bootstrap validated

- Selectively applied trust checkpoint `5a852b21` and build-admission checkpoint `20dbb11a`; did not merge either review-expanded branch wholesale.
- Added reason-required, worker-refused `wg done --operator-accept` with an immutable receipt; `simple_local_recovery` proves it in a clean graph.
- Made completion review advisory by default with explicit strict mode retained; unavailable review is visible and does not block deterministic publication.
- Collapsed ordinary Land completion to one `wg done`: baseline/configured validation evidence, exact candidate, review activity, landing, and Done are internal.
- Restored direct trusted graph coordination during daemon outages while retaining exact generation/attempt/fence validation, audit attribution, graph identity checks, and explicit service/configuration administration refusals.
- Removed the Pi continuation-token exception from dead-owner sweeping; a dead process can no longer hide an `InProgress` task. The focused unit regression passed.
- Validated `trust_first_local_worker_coordination`, `worker_control_capability_broker`, `simple_local_recovery`, and `build_admission_inherits_worker_slots` against the candidate binary.
- Validated completion review tests, neutral spawn-preparation deferral, optional quality-pass release behavior, trusted/scoped actor defaults, `cargo check --all-targets`, and pinned `cargo fmt --check`.
- Remaining before install: clippy/full focused regression and the complete three-worker restart golden canary.

### 2026-08-09 — Bounded strict review and compact activity validated

- Explicit strict review now permits a repaired candidate to replace a rejected immutable candidate without manual manifest plumbing.
- After the configured two non-passing strict attempts, `wg done` parks the task as visible `Waiting`/Needs-review, releases the source worker, preserves both activities, and requires operator accept/reject rather than spinning.
- Hidden diagnostic completion subcommands remain callable but no longer appear in normal top-level help.
- Default `wg list --all` task rows now show compact assignment/FLIP/eval state and attempt count parenthetically; review activities remain non-task audit records.
- The clean `simple_local_recovery` smoke now proves advisory completion, operator recovery, compact parent-row activity, exact strict findings, and bounded strict exhaustion.
- CLI text/JSON already expose lifecycle audit, attempts, candidates, and immutable review activity; the TUI task inspector now renders the same concepts in a dedicated Completion history section with generation, attempt/fence, candidate digest, review route/verdict, usage, and cost. A focused TUI state test passed.

### 2026-08-09 — Golden canary, install, and disposable reconciliation

- `simple_local_fanout_restart` passed: two trusted fake-Pi workers coordinated across tasks; A completed while the dispatcher was stopped; after restart B completed and dependent C dispatched/completed automatically with zero source retry charge.
- The legacy ten-worker `worker_owned_completion_canary` also passed, preserving diagnostic submit/land compatibility while the ordinary prompt uses one `wg done`.
- `cargo fmt --check`, `cargo check --all-targets`, targeted completion/control/reconciliation/TUI tests, and `cargo clippy` passed (existing warnings only).
- Installed all three binaries with `cargo install --path . --locked` only after the canaries passed; `wg`, `worksgood`, and `nex` report `0.1.0`.
- Started the installed candidate against an extracted disposable preserved graph with `max_agents=0`; service status stayed healthy and `wg sweep --dry-run` reported no orphan mutation.
- On that disposable graph, operator reconciliation marked `audit-sync-roadmap` Done with a reason-bound immutable receipt and no model review call, proving the already-integrated-artifact recovery path without touching the real forensic graph.
