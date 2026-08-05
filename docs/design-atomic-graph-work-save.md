# Atomic GraphSave + WorkSave protocol

Status: **proposed for operator review; not implemented and not dispatched**

Companion plan: [`docs/plans/atomic-graph-work-save-dag.json`](plans/atomic-graph-work-save-dag.json)
Scope: WG task completion, finalization, replay, dependency reads, archive/reset/retry, and legacy reconciliation.

## 0. Executive decision

WG needs one completion commit, not a status write followed by best-effort Git work.
The design introduces:

- **WorkSave**: an immutable, exact-attempt snapshot receipt made only after the
  terminal writer is quiescent. It captures tracked, dirty, deleted, and
  untracked task-owned bytes, including the clean-tree case.
- **SaveTransaction**: a write-ahead, content-addressed transaction that binds
  the completion intent, WorkSave, candidate, validation/FLIP, task-owned
  disposition, promotion/output, cleanup, and graph commit.
- **GraphSave**: the immutable terminal evidence bundle. The lifecycle ledger
  projects `Done` from a valid GraphSave; `status` is never independent
  completion authority.

The safe failure direction is retained work plus a named action. Missing,
stale, contradictory, unsupported, or unverifiable evidence never produces a
dependency-satisfying state.

## 1. Primary evidence and incident reconstruction

The following was inspected read-only. Absolute `.wg` paths below identify the
self-hosting incident record; they are evidence, not proposed schema locations.

### 1.1 False Done and premature dispatch

The decisive sequence in `/home/bot/wg/.wg/lifecycle/events.jsonl` is:

1. `formalize-lifecycle-finish-lean4` generation 2 / `attempt-2-3` produced a
   candidate against base `1a1e112e...`. Its finalization journal entered
   `repair-needed` at `2026-08-02T14:06:18Z`; there was no merge, promotion, or
   cleanup receipt.
2. Reset/retry advanced through generations 3 and 4. Event 622 then recorded
   `AttemptSucceeded`, `open -> done`, as operator `bot`, with no attempt and
   only `completion:formalize-lifecycle-finish-lean4:4:legacy` as evidence.
3. `Task::effective_completion_disposition` in `src/graph.rs` treats a legacy
   `Done` Land task as `Landed`. `query::dependency_disposition` in
   `src/query.rs` accepts raw `Status::Done`. Thus the incomplete record
   satisfied the `formalize-daemon-planner-replay` edge.
4. Events 623--625 reserved and launched planner `attempt-0-1`. Dependency
   checking did exactly what it was coded to do; the input authority was false.

This was not merely a stale UI row. It authorized another process and therefore
violated dependency safety.

### 1.2 Lost broker context

Before `9774eed0`, `execute_worker_operation` handled a brokered
`WorkerOperation::DoneHandoff` by calling `commands::done::run` on the daemon
thread. That thread intentionally lacked `WG_WORKTREE_PATH`, `WG_BRANCH`, and
`WG_PROJECT_ROOT`. `done::detect_worktree` therefore returned `None`, skipped
`task_owned_done`, and reached the graph-root/operator compatibility path. That
path could create the legacy evidence string and terminalize without saving the
retained worktree.

Commit `9774eed0` is **necessary**: it retrieves the worker's worktree from the
registry, passes it through `run_from_worker_control`, and makes
`detect_worktree` see a thread-local override. It prevents this exact missing-env
fall-through.

It is **insufficient as a protocol**:

- the path is reconstructed from mutable registry state rather than included in
  and verified with the authenticated attempt/worktree capability;
- `worktree_id=agent-965` and the later transaction's
  `worktree_path=.../agent-956` demonstrate that identity and location can
  already disagree while downstream functions proceed;
- a thread-local value is not durable across response loss, daemon restart, or
  binary replacement;
- authenticated context is not propagated to every current consumer:
  `done::run_inner` still computes `project_root = dir.parent()` for
  `deliverables::preflight`, so a brokered worker can be told its required file
  is missing even when that file exists and is committed in the authenticated
  worktree;
- `checkpoint_uncommitted_source_work` runs `git add`/`git commit` before a
  WorkSave WAL record, and it can run while the source process is still able to
  write;
- `task_owned_done` returns after promotion/output and before cleanup and graph
  completion; the lifecycle kernel checks only that `acceptance_ref` is
  non-empty, not that the whole evidence bundle agrees;
- the legacy terminal path and many direct `status = Done` paths still exist;
- worker request replay validates the now-current capability before consulting
  the completed request journal, while the worker CLI normally generates a new
  request ID on each invocation. A lost response after terminalization is not a
  reliable retry;
- reset/retry, archive boundaries, dependency reads, and cleanup are not part of
  one evidence transaction.

The commit is therefore a narrow routing repair, not an atomic save protocol.

### 1.3 Reset did not prove continuation

Events 626--630 reset the false Done and launched generation 5 / `attempt-5-5`
with a new process/session authority. Retaining the old branch/worktree was
valuable, but it did not prove continuation of the previous exact session.
`preserve_session=true` is intent, not a cryptographic or append-prefix proof.
A retry may **import retained WorkSave bytes into a new attempt**; it may not call
that exact-session continuation.

Exact-session continuation is reserved for a process-epoch replacement with all
of: unchanged task/generation/attempt/fence, worktree lease and root identity,
session header and append-prefix digest, route snapshot, proven old-process
death, and absence of a terminal intent or SaveTransaction effect.

### 1.4 Dead planner remained InProgress

The prematurely dispatched planner's PID exited and its retained worktree was
clean at `5e5e6b5d...`. Registry state became `dead`, but the graph remained
`in-progress` until later exited-worker convergence events 636--637 fenced it
and created generation 1. A clean tree is still an observable WorkSave (the
receipt says `clean=true`); it is not evidence of success. The authoritative
convergence rule must always leave one of:

- an authenticated live owner;
- a durable SaveTransaction action and deadline;
- a proven exact-session continuation action and deadline; or
- a non-running `Failed`, `AbortedPreserved`, or `NeedsReconciliation` outcome.

Dead process + no action + `InProgress` is forbidden.

## 2. The one normative invariant

Let `K = (graph_id, task_id, generation, attempt_id, attempt_fence,
worktree_lease_epoch, candidate_id, base_commit_oid)` and let
`Required(policy)` expand the validation and FLIP receipts required by the
policy snapshot captured for that attempt. Here `candidate_id` is the canonical
candidate-binding hash derived from `(base, saved commit/tree/manifests,
policies)`; it is computable during WorkSave capture and is distinct from the
CID of any envelope that contains it, avoiding a content-addressing cycle.

> **GS/WS invariant.** A task generation is dependency-satisfying `Done` **iff**
> one durable `GraphSaveReceipt` exists whose exact-attempt `WorkSaveReceipt`,
> accepted immutable `CandidateDescriptor`, every `Required(policy)` validation
> and FLIP acceptance receipt, task-owned `LandReceipt` / `DeliverReceipt` /
> `ReportReceipt`, exact `PromotionReceipt` or `OutputReceipt`, and
> `CleanupCommit` all exist, verify, and agree on every field of `K` (and on the
> disposition, policy, result tree/manifest, and immutable references).

Consequences are normative:

1. A raw string, log line, exit code, registry status, `Status::Done`, candidate,
   validation result, merge commit, or cleanup marker alone is never sufficient.
2. `Done -> bundle valid` and `bundle valid -> Done` are both reducer rules.
   A valid bundle not yet projected is replayed to Done. A Done projection with
   no valid bundle is projected to `NeedsReconciliation`.
3. `status`, `completion_disposition`, `completion_receipt`, archive boundary
   status, and remote status are cached read models. They cannot confer
   authority.
4. All optional-looking receipt slots have a policy-bound explicit result. For
   example, no-FLIP policy uses a content-bound `FlipNotRequiredReceipt`; it is
   not represented by an absent field or an ambient configuration check.
5. Land requires a target-ref CAS receipt. Deliver/Report require an immutable
   output-ref receipt. A task with no worktree still requires an explicit
   no-worktree WorkSave and no-op cleanup receipt bound to `K`.
6. Failure, abandonment, cancellation, and quarantine are terminal for
   scheduling but never dependency-satisfying.

## 3. Evidence schema

All objects use canonical JSON, a content CID, `schema_version`, `protocol_major`,
and `producer_build_id`. CIDs are immutable; mutable transaction heads point to
them. Every receipt repeats `K` rather than relying on an enclosing filename.

### 3.1 Source key and terminal intent

`AttemptSaveKey` contains `graph_id`, task/generation/attempt/fence,
worktree-lease epoch, process/wrapper epoch, route snapshot, session proof, and
canonical worktree identity (path, Git administrative dir, branch, device/file
identity where available). `save_tx_id` is the hash of the key and the first
terminal reservation.

`CompletionIntentReceipt` is accepted only from the exact worker or its owning
wrapper capability. It commits to:

- Land/Deliver/Report;
- the terminal WorkSave capture algorithm and inclusion policy;
- validation, FLIP, smoke, and deliverable policy CIDs;
- the expected target ref and prepared base for Land;
- a stable client idempotency key.

The worker supplies semantics. The wrapper or daemon may replay authorized
mechanics; it may not change disposition or invent acceptance.

### 3.2 WorkSaveReceipt

A WorkSave is captured after exact writer quiescence (PID/start/boot/nonce,
process-group empty, pipe EOF, and observer reconciliation). It contains:

- the complete `AttemptSaveKey` and terminal/quiescence receipt IDs;
- worktree root identity, branch, worker HEAD, prepared base, clean flag, and
  the canonical candidate-binding ID computed from the saved snapshot;
- rescue commit/tree, full manifest CID, delta manifest CID, and immutable
  `refs/wg/work-saves/<task-hash>/<generation>/<attempt>/<cid>`;
- excluded-path policy and proof that `.wg`, `.git`, cleanup markers, caches,
  and control-plane aliases were excluded;
- observer final manifest/sequence and any late-mutation quarantine evidence.

Capture uses a private index or equivalent snapshot; it never mutates the
worker's index. The immutable Git ref and object files are fsynced before the
receipt phase advances. If the writer is not provably quiescent or the manifest
is unstable, capture holds. A clean worktree produces a real receipt whose
saved tree equals HEAD.

### 3.3 Candidate and acceptance

The candidate is a deterministic projection of one WorkSave. It repeats `K`,
including the same precomputed candidate-binding ID, base,
commit/tree/manifests, WorkSave CID, candidate version, inclusion policy, and
immutable ref. The descriptor envelope has its own CID. Repair creates a new
candidate version/binding and new envelope CID; old validation never retags it.

Validation and FLIP receipts repeat the candidate binding and policy CID.
Infrastructure failure, timeout, malformed output, and insufficient evidence
are `Unavailable`/`Insufficient`, not semantic rejection and never acceptance.
A semantic rejection retains WorkSave and candidate in `NeedsRepair`.

The task-owned disposition receipt repeats `K` and cites the authenticated
completion intent. It may be materialized by the owning wrapper after WorkSave
capture because the intent authorized “the exact terminal WorkSave under policy
P”; this derivation is deterministic and does not give the daemon semantic
authority.

### 3.4 Effect, cleanup, and GraphSave

For Land, `EffectPlan` records target ref, expected base commit/tree, candidate,
and action key before `git update-ref`. `PromotionReceipt` records the exact
integration commit/tree/manifest, result ref, expected and observed old target,
and successful CAS. For Deliver/Report, `OutputReceipt` records the immutable
output ref and exact candidate binding.

`CleanupPlan` is durable before deletion. `CleanupCommit` records the exact
worktree/root identity and branch that were removed, or a schema-defined
`not-applicable` no-worktree result. It cites the durable WorkSave and effect
receipt. “Path does not exist” is accepted on replay only if the prior cleanup
plan, immutable WorkSave, Git administrative tombstone, and expected branch/ref
state all agree. Unknown absence is quarantine, not success.

`GraphSaveReceipt` contains the full evidence-CID list, their canonical bundle
digest, `K`, disposition, graph revision before commit, and lifecycle event ID.
It is written after cleanup. The lifecycle ledger appends
`GraphSaveCommitted(graph_save_cid)` under graph lock and projects status from
that receipt.

## 4. Authoritative SaveTransaction

### 4.1 Durable representation

Per-attempt state lives under a source-tuple key, not only a task filename:

```text
.wg/completion/v2/
  transactions/<source-tuple-hash>/head.json
  journal/<source-tuple-hash>.jsonl
  objects/<cid>
  requests/<idempotency-key>.json
  graph-saves/<task>/<generation>.json
  protocol.json
```

Each journal frame is hash-linked, carries `tx_revision`, `prior_phase`,
`next_phase`, action key, evidence CIDs, and checksum. Commit is:
write object -> fsync object -> fsync object dir -> append frame -> fsync journal
-> atomic head replacement -> fsync parent. A torn tail is truncated to the
last valid newline/checksum frame. The journal is authority; `head.json` is a
rebuildable projection.

### 4.2 Phases and legal edges

| Phase | Required durable fact | Only legal forward edge |
|---|---|---|
| `Absent` | none | `Prepared` |
| `Prepared` | exact capability + completion intent + policy/base snapshot | `Quiescing` |
| `Quiescing` | terminal reservation; no later worker mutation authorized | `WorkSaved` after exact quiescence |
| `WorkSaved` | immutable WorkSave object/ref | `CandidateSealed` |
| `CandidateSealed` | accepted immutable candidate derived from that WorkSave | `Validated` or `NeedsRepair` |
| `Validated` | exact passing validation and explicit FLIP policy result | `AwaitingAcceptance`, `Accepted`, or `NeedsRepair` |
| `AwaitingAcceptance` | required evaluator request | `Accepted` or `NeedsRepair`/hold |
| `Accepted` | every required receipt accepted | `DispositionRecorded` |
| `DispositionRecorded` | exact task-owned Land/Deliver/Report receipt | `EffectPrepared` |
| `EffectPrepared` | write-ahead target/output action | `EffectCommitted` or `NeedsRepair` |
| `EffectCommitted` | exact promotion/output receipt | `CleanupPrepared` |
| `CleanupPrepared` | deletion/no-op plan and durable rescue refs | `CleanupCommitted` or hold |
| `CleanupCommitted` | cleanup receipt | `GraphSaved` |
| `GraphSaved` | valid GraphSave and ledger event | terminal/inert; only explicit new generation |
| `NeedsRepair` | retained evidence + reason + safe action | a higher candidate version or explicit abort |
| `AbortedPreserved` | WorkSave/rescue + non-success disposition | explicit new generation only |
| `UpgradeBlocked` | unsupported protocol/build evidence | compatible binary only |
| `NeedsReconciliation` | legacy or contradictory evidence | explicit reconstruction, retry, or abandon |

No edge may skip a row. Replaying an already-completed exact edge is a no-op.
A request for an earlier phase with different bytes is `idempotency-conflict` and
holds the transaction.

### 4.3 Idempotency keys

- transaction: `save:<graph>:<task>:<generation>:<attempt>:<fence>`;
- intent: `intent:<save-tx>:<worker-terminal-seq>`;
- WorkSave: `worksave:<save-tx>:<final-observer-manifest>`;
- candidate: `candidate:<worksave-cid>:<version>:<base>:<policy>`;
- validation: `validate:<candidate>:<policy>`;
- FLIP: `flip:<candidate>:<policy>:<route-snapshot>`;
- disposition: `disposition:<intent>:<candidate>:<land|deliver|report>`;
- effect: `effect:<disposition>:<target-or-output-ref>:<base>`;
- cleanup: `cleanup:<effect-receipt>:<worktree-lease>`;
- GraphSave: `graphsave:<cleanup-cid>:<bundle-digest>`.

The broker's Done request ID is the intent key, not a random UUID. A second
payload under the same key is rejected. Completed request replay authenticates
the original capability/token hash first, then returns the stored transaction
state even if that attempt is no longer current; it does not re-execute.

### 4.4 CAS and revision rules

1. Every tx frame requires expected `tx_revision`, phase, and exact source key.
2. Every graph event requires expected lifecycle revision,
   generation/attempt/fence, and terminal reservation.
3. Attempt fence and worktree lease epoch must match; neither is inferred from
   path names or agent IDs.
4. Land takes one repository finish lease containing source key and base. The
   final target write is `update-ref(target, result, expected_base)`.
5. A CAS that already observes the prepared immutable result ref is replayed;
   any other target movement is `NeedsRepair`. There is no automatic rebase or
   “take main”.
6. Cleanup CASes the exact root identity/lease. Generic GC cannot delete a
   source with a non-GraphSaved transaction.
7. `GraphSaveCommitted` is appended before the compatibility graph projection
   replacement. Ledger replay repairs a crash between them.
8. Reset/retry cannot change generation while a source transaction or
   unsaved/quiescing worktree is unresolved.

### 4.5 Projection rules

`project_completion(evidence)` is the only source of successful status:

- valid GraphSave for current generation -> `Done` plus typed disposition;
- active transaction -> `InProgress`/`PendingEval` compatibility rendering with
  explicit save phase;
- repair/upgrade hold -> non-ready hold;
- historical Done without a valid GraphSave -> `NeedsReconciliation`;
- successful old generation followed by explicit new generation -> current
  generation's non-Done state, while old GraphSave remains immutable history.

Direct assignment of `Done`, fallback `effective_completion_disposition`, and
`Status::is_dep_satisfied` are removed from authority paths. Debug/test
constructors may create projections only through a fixture builder that also
creates evidence.

## 5. Authority matrix

| Actor | May | Must not |
|---|---|---|
| Worker/native child | run gates; request exact terminal intent for its own tuple; choose Land/Deliver/Report; query same request | write Done, assert quiescence, promote, clean, change base, finish another task |
| Owning wrapper | authenticate child topology; reserve terminal; observe child exit; derive task-owned disposition from intent + exact WorkSave; replay mechanics | invent success without child intent; reuse a different worktree/session; declare OS facts it did not observe |
| Broker | validate graph/capability/source/worktree binding; WAL the intent; return tx ID/state; replay response | depend on daemon-thread env; look up an unverified mutable path; call legacy Done; infer semantics |
| Process/worktree observer | provide exact death/quiescence/root/manifest observations | mark success or resurrect an attempt |
| Daemon convergence | reduce persisted facts; schedule/retry idempotent capture, effect, cleanup, GraphSave; enforce deadlines | create FLIP acceptance, switch disposition, auto-resolve conflict, call status Done directly |
| Validator/evaluator | emit candidate/policy-bound receipt; distinguish semantic and infrastructure outcomes | promote, clean, mutate graph status, accept a newer/different candidate |
| Promoter/output publisher | execute one prepared effect with target/base CAS; reconstruct exact lost receipt | choose candidate, evaluate, rebase, merge unreceipted work, mark Done |
| Cleanup adapter | remove only exact planned lease after durable WorkSave/effect; commit cleanup | delete unresolved/legacy WIP, treat unknown absence as success, mark Done independently |
| Operator reset/retry | request cancellation/new generation; choose retained WorkSave import, explicit fresh discard after save, or reconciliation | mutate status; silently bless legacy Done; claim retry is same-session continuation |
| Dependency/archive/remote reader | verify GraphSave or verified boundary proof | trust raw Done, legacy Land fallback, registry state, or task filename |

Operator override is authority to choose a documented repair path, not authority
to falsify the invariant. There is no `--force-done`.

## 6. Complete mutation and read inventory

This is an implementation inventory, not a claim that every listed path has the
same semantics. Test-only constructors were separated from runtime paths during
review; future static checks must enforce the distinction.

### 6.1 Terminal and generation writers

| Area | Concrete runtime functions/files | Required conversion |
|---|---|---|
| Central lifecycle | `LifecycleKernel::transition`, `apply_transition`, `append_new_events`, `replay_ledger` in `src/lifecycle.rs` | `GraphSaveCommitted` becomes sole successful edge; remove nonempty-string acceptance and operator legacy success |
| Worker completion | `done::run`, `run_inner`, `run_from_worker_control`, `detect_worktree` and its `deliverables::preflight` caller in `src/commands/done.rs`; parser/checker in `src/commands/deliverables.rs` | produce/query completion intent only; no compatibility fall-through; resolve every file gate against the authenticated source root, never `dir.parent()` |
| Finish | `task_owned_done`, `submit_finish`, `prepare_candidate_evaluation`, `cleanup_finish`, `settle`, `converge_exited_worker_finishes` in `src/commands/finalize.rs`; promotion functions in `src/finalization/mod.rs` | one SaveTransaction reducer and effect adapters; cleanup cannot directly request AttemptSucceeded |
| Evaluation | `eval_lifecycle::reconcile_durable_verdicts`; `evaluation::deep::consume_required_pass`; `evaluation::bounded::finalize_success`; `commands::approve::run`, `reject::run` | write exact acceptance/rejection receipts only |
| Failure/terminal commands | `fail::run_inner`, `incomplete::run`, `abandon::run`, `recover::apply_plan` | non-success events; capture WorkSave before destructive/fence operations |
| Direct task APIs | `matrix_commands::{execute_claim,execute_done,execute_fail,execute_unclaim}`, `commands::exec::{run,run_interactive}`, `commands::agent::{claim_task,complete_task,fail_task}` | route through lifecycle/SaveTransaction adapters |
| Human/chat | `human_dispatch::try_complete_human_task_on_reply`; `chat_cmd::{archive_chat_direct,delete_chat_direct}` | human reply creates Report/Deliver evidence; chat archive gets a non-dependency archive receipt or is forbidden as an `after` prerequisite |
| Daemon triage | `service::triage::{cleanup_dead_agents,apply_triage_verdict}`, `sweep::{run,reconcile_orphaned_tasks}`, coordinator wait/unblock and inline spawn paths | observations and non-success/retry requests only; never infer success from exit |
| Provider bridge (inventoried non-goal) | `exec_fed_cmd::bridge_usage_into_graph` | raw Done is blocked by central kernel immediately; provider-specific receipt adaptation is a separately reviewed follow-up, not this project's provider redesign |
| Imported/synthetic Done | `migrate_pending_validation_tasks`; `function_memory`; `func_apply`; `evolve::fanout`; `trace_import`; `service::assignment` synthetic completed rows | create explicit synthetic/import GraphSave or `NeedsReconciliation`; never construct authoritative raw Done |
| Reopen/ownership | `reopen::{request,reconcile_pending,discard_old_worktree}`, `retry::{run_with_selection,retry_in_progress}`, `reset::run`, `claim::{claim,unclaim}`, `claim_lifecycle::clear_stale_downstream_claims`, `graph::{reactivate_cycle,reactivate_cycle_on_failure}` | save/fence exact prior tuple; new generation is explicit; session reuse is lineage, not continuation |

### 6.2 Worktree, candidate, effect, and deletion mutations

| Mutation | Concrete paths | Protocol rule |
|---|---|---|
| Allocate/reuse worktree | `spawn::worktree::{create_worktree,verify_worktree_info,find_verified_worktree_for_task,rollback_created_worktree}` and `spawn::execution::{prepare_spawn_workspace,claim_task_for_spawn}` | persist root identity and lease in source key before launch |
| Observe bytes | `WorktreeObserver` and `run_watch_loop` in `src/worktree_observer.rs`; command adapter in `src/commands/worktree_observer.rs` | final manifest is evidence only; WorkSave capture consumes it after quiescence |
| Mutable checkpoint | `checkpoint_uncommitted_source_work` in `src/commands/finalize.rs`; `commands::checkpoint::run` | replace terminal use with private-index WorkSave WAL; ordinary checkpoints remain nonterminal hints |
| Rescue/candidate | `checkpoint_rescue`, `checkpoint_candidate`, `snapshot_tree`, `publish_ref`, `validate_candidate` in `src/finalization/mod.rs`; `fail::run_inner` | immutable objects/refs attached to exact source transaction |
| Evaluation receipt | `record_evaluation_receipt`, deep/bounded finalizers | candidate/policy/source-key agreement checked by kernel |
| Promotion/output | `promote_task_owned_candidate`, `merge_candidate`, `accept_resolution_tree`, `publish_output`, merge-resolution callers | only prepared action; exact base CAS/result ref; receipt before cleanup |
| Task-owned cleanup | `finalize::cleanup_finish`, `finalization::record_cleanup` | cleanup plan first, exact identity removal, cleanup commit, then GraphSave |
| Generic deletion | `spawn::worktree::remove_worktree`; `service::worktree::{remove_worktree,remove_worktree_verified,cleanup_dead_agent_worktree,sweep_cleanup_pending_worktrees,prune_stale_worktrees,prune_recovery_branches}`; `worktree_gc::run`; `cleanup::{run_orphaned_cleanup,attempt_manual_worktree_cleanup,cleanup_filesystem,cleanup_git}`; `worktree_cmd::{archive,gc}` | all consult SaveTransaction retention barrier; unresolved/legacy work is quarantined, not deleted |
| Reset fresh deletion | `reopen::discard_old_worktree` | exact quiescence + WorkSave + explicit discard receipt before delete |

### 6.3 Dependency, dispatch, archive, and migration reads

| Read | Concrete paths | Change |
|---|---|---|
| Completion gates | deliverable preflight inside `done::run_inner`; verify and smoke gates called there; `commands::deliverables::{parse_deliverables,preflight}` | bind policy and results to the transaction; read files from the authenticated source root/WorkSave, not the graph root or ambient cwd |
| Dependency truth | `query::{dependency_disposition,is_blocker_satisfied,is_blocker_satisfied_with_eval_gate,ready_tasks,ready_tasks_with_peers_cycle_aware}` | one `verify_dependency_graph_save` call; no raw status fallback |
| Dispatch consumers | `commands::{ready,claim,done,show,status,why_blocked}`; `spawn::execution::spawn_dependency_blocker`; `service::coordinator::{check_ready_or_return,spawn_agents_for_ready_tasks}`; `check::check_dependencies` | consume the central disposition only; expose missing receipt reason |
| Archive | `archive::{should_archive,archived_boundary_for,append_to_archive,run,archive_automatic_batch,restore,undo}`; `graph::ArchivedBoundary` | archive only valid GraphSave Done or Abandoned; boundary carries GraphSave CID/bundle digest; restore preserves evidence |
| Legacy disposition | `Task::effective_completion_disposition`, `Status::is_dep_satisfied`, `Status::parse_label("pending-review")` in `src/graph.rs` | remove success authority; pending-review/Done without evidence quarantines |
| Remote dependency | `federation::resolve_remote_task_status` callers in `query::dependency_disposition` | require signed/verified GraphSave summary or block; transport/provider redesign remains out of scope |
| Startup/migration | parser load/replay; `commands::migrate`; `lifecycle::migrate_pending_validation_tasks`; trace/function imports | run versioned evidence classification and append quarantine/import events; never rewrite history silently |
| Agent liveness | `AgentRegistry`, `triage::cleanup_dead_agents`, `reopen::owner_is_live`, `finalize::owner_is_live` | liveness chooses convergence action only; it never establishes success |

### 6.4 Audited line anchors and repository-wide searches

Line anchors at audited main `347a1696` (they may drift after implementation):
`src/graph.rs:1144` (`effective_completion_disposition`),
`src/query.rs:364` (`dependency_disposition`),
`src/lifecycle.rs:635,700,1362` (`AttemptSucceeded`, `AcceptanceSatisfied`,
`apply_transition`), `src/commands/done.rs:327,401,2555,2883`
(broker override, ambient detection, task-owned branch, terminal request),
`src/commands/finalize.rs:659,986,1158,1213` (dead-worker convergence,
cleanup, mutable checkpoint, task-owned entry),
`src/commands/service/ipc.rs:551,735,871` (capability check, DoneHandoff,
request journal), `src/worker_cli.rs:24,91` (request ID/send),
`src/finalization/mod.rs:643,797,844,896,1203` (promotion, output, cleanup,
capture, merge), `src/commands/reopen.rs:26,227,247` (intent, deletion,
release), and `src/commands/archive.rs:236,264,571` (eligibility, boundary,
archive mutation).

These commands were run from the repository root and are the reproducible audit
basis. Implementation should save refreshed output as a review artifact and
fail a static “unapproved Done writer” allow-list test.

```sh
rg -n 'Status::Done|status\s*=\s*Status::|status:\s*Status::' src --glob '*.rs'
rg -n 'TransitionKind::(AttemptSucceeded|AcceptanceSatisfied|GenerationCreated|ReopenRequested|ReopenOwnerReleased|Abandoned)' src --glob '*.rs'
rg -n 'dependency_disposition\(|is_dep_satisfied\(|get_archived_boundary\(' src --glob '*.rs'
rg -n 'DoneHandoff|run_from_worker_control|detect_worktree|task_owned_done' src --glob '*.rs'
rg -n 'deliverables::preflight|project_root\s*=\s*dir\.parent|run_smoke_gate|run_verify' src/commands --glob '*.rs'
rg -n 'checkpoint_candidate|checkpoint_rescue|merge_candidate|promote_task_owned_candidate|publish_output|record_cleanup' src --glob '*.rs'
rg -n 'remove_worktree|remove_dir_all|CLEANUP_PENDING|prune_recovery|delete_recovery' src --glob '*.rs'
rg -n 'modify_graph|save_graph|append_new_events|replay_ledger' src --glob '*.rs'
rg -n 'migrate|LegacyCheckpointImported|pending-review|effective_completion_disposition' src --glob '*.rs'
rg -n 'request_id|begin_request|complete_request|worker_control' src/worker_cli.rs src/worker_control.rs src/commands/service/ipc.rs
git show --stat 9774eed0
git show 9774eed0 -- src/commands/done.rs src/commands/finalize.rs src/commands/service/ipc.rs src/commands/service/mod.rs
git show 9774eed0:formal/README.md
python3 - <<'PY'
# Parse /home/bot/wg/.wg/lifecycle/events.jsonl and print events whose task_id
# is formalize-lifecycle-finish-lean4 or formalize-daemon-planner-replay.
PY
```

## 7. Crash cuts and replay

Every row is a mandatory fault point. “Replay” always means re-read journal and
immutable objects, validate schema/build/source key, run the pure reducer, and
execute at most one idempotent action.

| Crash/loss cut | Restart action | Forbidden result |
|---|---|---|
| before `Prepared` | no transaction/effect; old attempt remains owned | Done or deletion |
| after intent object, before Prepared frame | find deterministic intent CID/key; attach exact object or hold on conflict | second intent with different disposition |
| after `Prepared`, before response | retry same intent key returns tx state | rerun under random request ID |
| while writer live / Quiescing | wait/probe exact identity and deadline | snapshot mutable bytes or call success |
| after quiescence receipt | capture exact root once | infer success from exit |
| after WorkSave objects/ref, before frame | discover deterministic ref/CID, verify, attach | create a different snapshot |
| after WorkSaved frame | derive same candidate | mutate worker index/worktree |
| after candidate object/ref | attach/validate same binding | validate source worktree or stale candidate |
| after validation | request/replay exact FLIP policy | use ambient current config |
| after FLIP response, before receipt frame | recover content-addressed evaluator receipt | run/charge a second semantic call without dedup |
| after task-owned disposition | prepare exact effect | daemon changes Land/Deliver/Report |
| after EffectPrepared, before physical effect | execute same action | unplanned target mutation |
| after target CAS/output publish, before receipt | verify prepared result ref, old/new target, tree/manifest; reconstruct identical receipt | second merge/promotion |
| target moved before CAS | retain WorkSave/candidate, release lease, `NeedsRepair` | auto-rebase, force update, Done |
| after effect receipt | persist CleanupPrepared | graph Done |
| after CleanupPrepared, before deletion | remove only exact root/branch identity | generic age-based GC |
| after deletion, before CleanupCommit | prove planned root absent + Git tombstone/ref state + WorkSave durable; commit same receipt | bless unknown missing path |
| after CleanupCommit, before GraphSave | create GraphSave bundle | dependency satisfaction |
| after GraphSave object, before ledger event | append same `GraphSaveCommitted` by expected revision | new candidate/effect |
| after ledger fsync, before graph replacement | `replay_ledger` rebuilds projection | raw graph status wins |
| after graph replacement, before IPC response | replay completed intent request by original token/key and return GraphSave | stale-capability error causing re-execution |
| daemon/client major-version skew | enter `UpgradeBlocked`; no mutation; print required version/build | downgrade interpretation |
| crash at any `NeedsRepair`/`NeedsReconciliation` cut | retain evidence and safe command indefinitely | automatic erase/archive |

A transaction recovery rank is the number of missing monotone commits from
GraphSave. Every useful replay action must decrease it; waits have a durable
reason/deadline. Expected lock/CAS contention is breaker-neutral.

## 8. Reset, retry, archive, and dead-owner semantics

### 8.1 Reset/retry

1. Persist an operator intent bound to the current source tuple.
2. Fence new worker control requests but keep status non-runnable.
3. Prove exact owner quiescent.
4. Capture WorkSave even for failure/cancel/clean tree.
5. If `--fresh`, write an explicit discard receipt and only then remove the
   exact worktree. Otherwise preserve/import the WorkSave.
6. Commit `AbortedPreserved` for the old attempt.
7. Increment generation once by CAS. A subsequent dispatch creates a new
   attempt. Its lineage may cite the retained WorkSave and session transcript,
   but output must say **new attempt using retained work**, not “same session”.

A true same-attempt continuation does not use reset/retry and does not increment
generation/attempt. It requires the exact continuation proof listed in section
1.3. Failure of any proof creates a new attempt or hold.

If reset races completion, first durable terminal reservation wins. Before an
effect, an operator may request abort-preserved; after effect commit, cleanup and
GraphSave must converge before a new generation can be created. Already-running
dependents carry a dependency revision digest and are fenced when their input
generation changes.

### 8.2 Dead owner convergence

- terminal intent present: capture/replay its SaveTransaction;
- no intent, exact continuation proof present: schedule one same-attempt process
  epoch replacement with deadline;
- no intent or proof: capture failure WorkSave and transition to
  `AbortedPreserved`/`Failed` or `NeedsReconciliation`;
- transaction already effect-committed: cleanup then GraphSave;
- unsupported/corrupt evidence: `UpgradeBlocked`/`NeedsReconciliation`.

Registry state is updated as a projection after the lifecycle event. A registry
save failure cannot leave the graph success state false; reconciliation repairs
registry. Conversely, registry `Done`/`Dead` cannot terminalize the task.

### 8.3 Archive

Only a valid GraphSave Done or explicit Abandoned record is archivable. The
active `ArchivedBoundary` stores source generation, GraphSave CID, bundle
digest, disposition, and verification version. Boundary reads re-verify the
object. A legacy Done boundary without that proof becomes
`NeedsReconciliation` and blocks. Restore/undo is idempotent and never removes
the evidence object.

## 9. Legacy migration and quarantine

On first v2-capable startup, with dispatch paused:

1. Snapshot graph, archive, lifecycle ledger, finalization store, and build ID.
2. Classify every active or archived Done:
   - complete, internally consistent existing evidence -> deterministic
     `LegacyEvidenceImported` GraphSave, with all original CIDs and a migration
     receipt;
   - any missing or contradictory required evidence -> append
     `LegacyDoneQuarantined` and project `NeedsReconciliation`.
3. Preserve the original row/event/archive bytes. Never rewrite them away.
4. Rebuild dependency and archive projections. Quarantined predecessors block.
5. Produce a machine-readable report listing downstream tasks that were
   previously (or currently) authorized by each false Done.

There is no automatic “probably merged” blessing. An operator may:

```text
wg completion audit [TASK|--all] [--json]
wg completion show TASK [--json]
wg completion reconcile TASK                 # deterministic replay only
wg completion reconstruct TASK --work-save REF --candidate CID --evidence FILE
wg completion retry TASK [--fresh]            # new generation; prior bytes saved
wg completion abandon TASK --reason TEXT
wg completion rollout pause|status|resume
```

`reconstruct` succeeds only when exact attempt/worktree/base/candidate evidence
can be proven and produces an explicit operator-reviewed reconstruction receipt.
If exact WorkSave cannot be established, the task cannot return to Done; retry
or abandon is required. No repair command deletes retained evidence.

## 10. Versioning, rollout, rollback, and observability

### 10.1 Compatibility

- bump worker control to `worksgood-worker-control-v2`;
- add explicit SaveTransaction, WorkSave, GraphSave, archive-boundary, and
  reducer wire versions;
- write `.wg/completion/v2/protocol.json` with minimum reader/writer versions;
- daemon, CLI, wrapper, and candidate binaries handshake before mutation;
- preserve v1 formal fixtures unchanged; v2 gets new fixture paths and a new
  wire meaning rather than reinterpreting v1;
- old binaries seeing the v2 sentinel refuse startup/mutation loudly.

### 10.2 Rollout

1. Land shared schema/kernel and audit-only diagnostics in a disposable graph.
2. Land all adapters behind a graph-local disabled flag; dual-read, v2 shadow
   write, but do not call shadow state authoritative.
3. Run full migration dry-run and export the quarantine/downstream impact report.
4. Stop dispatch, install a v2 reader/writer everywhere, write the protocol
   sentinel, perform migration, and re-open only valid GraphSave dependencies.
5. Canary one graph and inject every crash boundary and lost response.
6. Enable writes, then remove raw Done compatibility writers.
7. After one release, remove permissive read fallback. Keep v1 evidence readers
   and quarantine forever.

Safety takes precedence over availability at cutover: legacy rows without proof
block rather than dispatch.

### 10.3 Rollback

Before the sentinel/cutover, rollback code normally. After cutover, do not run a
v1 writer. `rollout pause` stops dispatch/effects while retaining v2 records; a
compatible rollback binary may read/replay them. Never roll back by deleting the
protocol sentinel, GraphSave objects, quarantine events, or by mapping
`NeedsReconciliation` to Done.

### 10.4 Metrics and diagnostics

Expose at least:

- `completion_transactions{phase,contract,protocol}` and oldest age;
- `completion_graphsave_total`, `completion_bundle_invalid_total`;
- `completion_legacy_quarantined_total` and blocked-dependent count;
- `worksave_capture_total{clean,result}`, unstable/late-mutation counts;
- `completion_effect_total{kind,result}`, target-moved and replay counts;
- `completion_cleanup_total{result}`, retained bytes/worktrees;
- `completion_request_replay_total{result}`, lost-response recoveries;
- `completion_dead_owner_total{action}`, max convergence deadline lateness;
- `completion_binary_skew_total{client,daemon,protocol}`.

`wg show`/`wg completion show` display source tuple, phase, every evidence CID,
missing/mismatched field, last action/deadline, recovery rank, build versions,
and one safe next command. Structured logs include tx/action/idempotency key but
not task content.

## 11. Executable acceptance traces

The implementation must expose `WG_TEST_SAVE_CRASH_AFTER=<phase>` only in test
builds and replay with the candidate binary. Proposed permanent commands are
listed in the companion DAG.

| Trace | Script/test | Required assertions |
|---|---|---|
| false Done dependency | `atomic_save_false_done_dependency.sh`; Rust `false_done_dependency_dispatch` | plant Done with no bundle; load -> NeedsReconciliation; dependent never reserves/spawns |
| broker lost worktree | `atomic_save_broker_handoff.sh`; `broker_handoff_requires_bound_worktree` | broker thread has no WG env; authenticated request includes exact root identity; WIP appears in WorkSave; mismatched registry path is refused |
| crash every phase | `atomic_save_crash_replay.sh`; table-driven `crash_after_each_durable_boundary` | kill after every phase in section 7; restart; exactly one logical effect; no lost WIP; only final cut satisfies dependency |
| dead worker | `atomic_save_dead_worker.sh`; `dead_worker_without_intent_converges_nonrunning` | kill wrapper/child with clean and dirty trees; by deadline get resume-exact or AbortedPreserved/NeedsReconciliation, never stranded InProgress |
| target movement | crash script; `target_movement_holds_candidate` | move main after EffectPrepared; no force/rebase/Done; candidate retained; explicit repaired version required |
| reset/retry | `atomic_save_reset_migration.sh`; `reset_retry_saves_before_generation` | reset live/dead owner; old WorkSave exists before generation+1; new session labeled new attempt; stale actor fenced |
| lost IPC response | broker script; `lost_done_response_replays_graphsave` | drop response before and after commit; same intent key returns same tx/GraphSave; random/different payload conflicts; one promotion |
| legacy migration | reset/migration script; `legacy_done_without_evidence_is_quarantined` | active and archived legacy Done become NeedsReconciliation; original records remain; dependencies block; second migration is no-op |
| daemon/binary skew | crash script; `old_daemon_new_client_holds` | incompatible major performs no graph/Git/worktree mutation and emits UpgradeBlocked |
| cleanup loss | crash script; `delete_before_cleanup_receipt_reconstructs_exactly` | deletion after CleanupPrepared reconstructs only with matching root/tombstone; unknown missing path quarantines |

## 12. Formal boundary and traceability

The existing Lean v1 program is completed work and remains intact. A v2 model
may extend the abstract state with receipt identities/agreement predicates and
SaveTransaction phases. Boolean receipt fields in Lean mean **verified durable
facts supplied by adapters**. Lean does not verify `fsync`, rename atomicity,
Git object/ref durability, PID identity, worktree snapshot stability, sockets,
signals, evaluators, NFS, or the absence of malicious out-of-band repository
mutation.

Environmental assumptions for conditional liveness are explicit:

- committed object/journal/ledger writes survive restart or fail detectably;
- Git object IDs are collision-resistant and `update-ref old->new` is atomic;
- exact quiescence/root/process observations are truthful;
- candidate inclusion policy completely excludes control-plane paths;
- eventually a compatible daemon restarts and fairly schedules a useful
  rank-decreasing action;
- unsupported filesystems/adapters fail closed rather than emulate durability.

Rust adapter tests and candidate-binary fault smokes discharge those assumptions
operationally.

### 12.1 Traceability matrix

| Invariant | Runtime guard | Pure reducer rule | Planned Lean theorem | Rust conformance trace | Fault test | Smoke |
|---|---|---|---|---|---|---|
| GS/WS Done iff full bundle | `verify_graph_save_bundle` at commit/read | `GraphSaveCommitted` iff all receipt predicates; projection converse | `done_iff_complete_agreeing_graphsave` | `happy_land/deliver/report_v2` | `missing_each_receipt_blocks_done` | false-Done |
| Exact tuple agreement | source-key equality at every store/effect API | stale generation/attempt/fence/candidate/base rejects inertly | `stale_capability_cannot_advance_save` | `stale_actor_v2` | concurrent stale/current CAS | broker handoff |
| WorkSave before destructive action | retention barrier in reset/cleanup/GC | no cleanup/reset-release before `workSaved` | `destructive_step_implies_work_saved` | `reset_dirty_v2` | kill before/after capture/ref | reset/migration |
| Candidate immutable and protected | CID/ref/tree/manifest/control-plane checks | candidate once per version; exact WorkSave derivation | `accepted_candidate_exact_and_protected` | `candidate_mismatch_v2` | mutate source after seal | broker/crash |
| Required validation/FLIP exact | candidate/policy/route binding | Accepted requires all policy slots | `acceptance_requires_exact_policy_receipts` | `flip_stale_candidate_v2` | lost/malformed evaluator response | crash replay |
| Task-owned disposition | authenticated intent/topology chain | daemon event cannot create disposition | `only_owner_intent_authorizes_disposition` | `wrapper_handoff_v2` | missing child intent after exit | broker handoff |
| At-most-once exact effect | EffectPrepared + result ref + target CAS | effect count <= 1; base mismatch holds | `promotion_at_most_once_and_base_exact` | `lost_effect_response_v2`, `cas_move_v2` | crash around update-ref | crash replay |
| Cleanup before GraphSave | cleanup plan/root identity/tombstone | GraphSaved requires cleanupCommitted | `done_implies_cleanup_commit` | `cleanup_cut_v2` | delete/receipt cuts | crash replay |
| Dependency reads evidence | central verified GraphSave index | `dependencySatisfied = graphSaveValid` | `dependency_iff_valid_graphsave` | `false_done_v2` | corrupt/remove evidence object | false-Done |
| Reset is new generation; continuation exact | session/worktree/route/process proof | retry increments generation; ResumeSame preserves tuple only with proof | `resume_same_requires_exact_continuation_proof` | `reset_vs_resume_v2` | reset races terminal intent | reset/migration |
| Dead owner always converges/holds | action/deadline exhaustiveness monitor | dead state has rank-decrease, exact resume, or nonrunning hold | `dead_owner_not_parked_conditionally` | `dead_owner_v2` | kill each owner topology | dead-worker |
| Legacy Done quarantines | migration classifier + archive reader | legacy missing proof -> NeedsReconciliation, not satisfied | `legacy_unproven_never_satisfies` | `legacy_done_v2` | crash/re-run migration | reset/migration |
| Lost response/skew safe | stable intent ID; replay-before-current check; version sentinel | duplicate exact no-op; version mismatch inert | `duplicate_and_version_mismatch_inert` | `lost_response_v2`, `wire_skew_v2` | response drop before/after graph save | broker/crash |

## 13. Implementation DAG and file ownership

The machine-readable proposal is
`docs/plans/atomic-graph-work-save-dag.json`. Its exact task IDs are:

1. shared gate: `atomic-save-kernel-schema`;
2. parallel after the gate:
   - `atomic-save-worksave-capture`,
   - `atomic-save-terminal-adapters`,
   - `atomic-save-daemon-convergence`,
   - `atomic-save-dependency-archive-reset`,
   - `atomic-save-legacy-migration`,
   - `atomic-save-formal-rust-traces`,
   - `atomic-save-adversarial-smokes`;
3. join/final gate: `atomic-save-synthesis-canary`, after every item in step 2.

The JSON names bounded owned files/modules, non-goals, and validation commands.
No two tasks in the parallel layer own the same correctness-critical file. The
kernel is deliberately shared and lands first; CLI wiring and canary synthesis
land last. These are proposed tasks only. This design task does not create,
publish, or dispatch them.

## 14. Non-goals

- no filesystem/XDG sandbox project;
- no provider-plane redesign (the raw provider Done writer is inventoried and
  centrally blocked, but provider receipts need separate review);
- no broad TUI redesign;
- no historical worktree cleanup;
- no rewrite or reinterpretation of the completed Lean v1 program;
- no claim that Lean verifies OS, Git, filesystem, storage, socket, process, or
  evaluator adapters.

## 15. Ratification checklist

Before implementation is approved:

- accept the single GS/WS invariant and no-force-Done policy;
- accept fail-closed legacy quarantine and its potential temporary scheduling
  impact;
- accept exact-session continuation as distinct from retry with retained work;
- review the complete mutation/read inventory and refresh the listed searches;
- approve the nine-task DAG and disjoint parallel ownership;
- require every trace in section 11 plus the traceability matrix before canary;
- require a stop-the-world per-graph protocol cutover and loud old-binary refusal.
