# WG control-plane authority deletion map

**Audit type:** evidence-backed deletion audit; no production change

**March baseline:** `41084d9a3cb2ddfbded358326731a84115a293e8` (2026-03-31)

**Current baseline:** `4e227b1f52ab63e9eeeb3c613f7329448d872410` (2026-08-06)

**Recorded traces:** [`docs/research/traces/wg-control-plane-authority-deletion-map/`](traces/wg-control-plane-authority-deletion-map/)

## 1. Finding

WG does not have one control plane with several views. It has overlapping state machines that happen to share a graph file.

The sharpest example is current completion: `completion_done::commit_done` sets the graph row to `Done`, clears `assigned` and marks the registry agent Done, but does not terminalize `task.lifecycle.current_attempt` or append a lifecycle event (`src/commands/completion_done.rs:228-293`). One semantic completion can therefore be terminal in graph/registry and still nonterminal in the lifecycle projection.

The current source says that `LifecycleKernel::transition` is the only production code that decides task/attempt edges (`src/lifecycle.rs:1-10`). A production-only syntactic scan nevertheless finds **32 direct task/projection-status assignments in 20 files outside that projection application**. The current `wg done` command bypasses the lifecycle ledger entirely and writes `Status::Done` in `completion_done::commit_done` (`src/main.rs:1261-1274`; `src/commands/completion_done.rs:228-281`). The daemon likewise says `PlannerStore` is not an authority and dispatches directly (`src/commands/service/mod.rs:3028-3037`), while `PlannerStore` still describes its trace and effect journal as durable production authority (`src/service/planner.rs:1988-2000`).

The problem is therefore not a missing controller. Adding another one would worsen it. The deletion target is one small, single-attempt semantic reducer plus adapters and read-only projections.

### Hard conclusions

1. **Terminal authority is plural.** `completion_done`, `LifecycleKernel`, `GraphSave`, evaluation reconciliation, human dispatch, remote-exec acceptance, chat/service cleanup and legacy terminal adapters can all produce terminal-compatible graph state.
2. **Attempt identity is copied, not shared.** Lifecycle attempt/fence, capability binding, save key, finalization context, Pi source tuple, observer identity, registry agent/run metadata and remote lease epoch all encode overlapping ownership.
3. **Replay is split by subsystem.** Lifecycle event keys, worker request IDs, save action keys, planner effect IDs, Pi action IDs, observer sequence/digests, provider composite keys and assignment task IDs do not share one semantic key.
4. **Liveness is an authority collision.** PID birth identity, registry heartbeat and stream timestamps can disagree; heartbeat timeout can mark a verified live PID dead and request `AttemptLost` (`src/commands/service/triage.rs:194-470`).
5. **“Evidence-only” components still persist controller-shaped state.** Worktree observer, Pi watchdog, provider telemetry and planner files have phases, action IDs, cursors, leases or pending actions. Some are useful evidence, but none should retain a transition API.
6. **Compatibility is now a product surface.** Fifteen named compatibility paths remain in this audited slice. Several repair old controllers into newer controllers rather than removing an authority.

## 2. Audit method and count rules

This report distinguishes:

* **authority mechanism** — a production-reachable or authority-shaped reducer/store that can gate, reserve, terminalize, retry, schedule an effect, or supply a fact treated as sufficient for one of those actions;
* **projection** — a rebuildable/read-only rendering that cannot issue an effect or transition;
* **durable control-store family** — files with an independently updated control fact, even when several physical files form one family;
* **helper process role** — a non-workload process launched to observe, classify or advance an attempt; and
* **direct status site** — a pre-`#[cfg(test)]` assignment matching `.status = ...Status::...`, excluding message-delivery status.

The LOC scope is the 62 source paths in §11. Inline test modules are excluded by counting only text before the first `#[cfg(test)]`. These are physical production lines, not an estimate from the full repository.

### March versus current scorecard

| Metric | March 31 | Current | Deletion-program target |
|---|---:|---:|---:|
| Production LOC in fixed control-plane manifest | 17,527 | 52,304 | ≤ March baseline before freeze lifts |
| Persisted `Status` variants | 8 | 11 | 6 task projections: Open, Running, Waiting, Done, Failed, Abandoned |
| Direct task/projection-status assignment sites/files | 46 / 16 | 32 / 20 | 0 outside the one projection applier |
| Named authority mechanisms in §4 | 9 | 22 | 1 semantic kernel; adapters have no transition authority |
| Independent reducer families | 0 named pure reducers | 9 | 1 |
| Lifecycle `TransitionKind` variants | 0 | 28 | ≤ 8 semantic requests |
| Durable control-store families | 5 | 18 | 1 authoritative attempt journal + projections/evidence stores |
| Append/replay journal families | 3 | 11 | 1 semantic journal; raw evidence logs remain non-authoritative |
| Per-attempt control helper roles (maximum Pi path) | 1 wrapper | 7 | 1 wrapper; observation runs in-process or on demand |
| Named compatibility paths in §8 | 1 | 15 | 0 after one release-bounded migration |

The March design itself enumerated 11 conceptual lifecycle states (`docs/design/unified-lifecycle-state-machine.md:11-30`), while the March 31 persisted enum had eight. The current enum happens to have 11 again, but state count did not produce one state machine: `PendingEval`, `FailedPendingEval` and `Incomplete` added more terminal/retry authorities.

### LOC growth by fixed group

| Group | March | Current | Delta |
|---|---:|---:|---:|
| Graph/readiness | 2,617 | 5,437 | +2,820 |
| Attempt kernels/broker | 0 | 3,873 | +3,873 |
| Completion/finalization | 11 | 10,597 | +10,586 |
| Daemon/dispatch | 9,789 | 11,581 | +1,792 |
| Liveness/observers/provider | 1,508 | 9,155 | +7,647 |
| Agency/synthetic control tasks | 2,637 | 7,843 | +5,206 |
| Other terminal/recovery/remote | 965 | 3,818 | +2,853 |

The largest additions are completion/finalization and liveness/observers, exactly where the incident traces show authority disagreement.

## 3. Smallest single-attempt semantic kernel

This is a deletion boundary, not a new scheduler.

### 3.1 State

One task has at most one current attempt:

```text
Task projection:    Open | Running | Waiting | Done | Failed | Abandoned
Attempt phase:      Reserved | Running | Finishing(intent) | Terminal(outcome)
Attempt key:        task_id + attempt_id + fence
Request identity:   idempotency_key + canonical request digest
Effect identity:    attempt key + semantic action + evidence/input digest
```

No generation is required in the kernel if `attempt_id` is monotonic and never reused. Historical generation remains import metadata. Provider health, PID state, heartbeat, stream activity, compaction, worktree hashes, evaluation, assignment and resource pressure are facts or receipts; none is an attempt phase.

### 3.2 One function and one commit

```text
apply(current_attempt, request)
  -> next_attempt + stable response + zero/one idempotent effect
```

The I/O wrapper holds `graph.lock` and commits the request outcome, task projection and any issued effect in one authoritative journal append before acknowledging. The graph row is rebuilt from the journal during migration. A lost response is therefore replayed by request key; it never requires semantic re-execution.

The reducer needs no more than eight request kinds:

1. `ReserveAttempt`
2. `MarkRunning`
3. `RecordTerminalIntent(Succeed|Fail|Park|Cancel)`
4. `RecordEffectReceipt`
5. `RecordExactExit`
6. `RecordObservation`
7. `SatisfyWait`
8. `OpenNextAttempt` (operator/pinned policy only)

### 3.3 Kernel invariants

| ID | Invariant |
|---|---|
| K1 | The attempt key is compared on every mutating request. |
| K2 | Same idempotency key + same digest returns the original response; different digest is a conflict. |
| K3 | Request outcome, response and logical effect issue are one commit. |
| K4 | First terminal intent wins; later contradictory reports are evidence only. |
| K5 | Success requires an exact immutable completion/publication receipt. |
| K6 | PID, heartbeat, EOF, stream, provider, compaction, assignment and evaluator observations cannot directly terminalize. |
| K7 | Exact exit without a terminal intent issues one disposition-reconciliation effect; it never infers success. |
| K8 | An effect ID is stable across crash/retry and can be acknowledged once. |
| K9 | Stale ownership and duplicate observations are breaker-neutral. |
| K10 | A terminal attempt cannot reopen; another run requires `OpenNextAttempt`. |

This kernel preserves the useful guarantees already scattered through `LifecycleKernel`, the worker request journal, `SaveTransactionKernel`, the planner effect journal and the capability broker. It deletes their independent state spaces.

## 4. Exhaustive mechanism inventory

“March” means the mechanism existed in the fixed March revision. “Current role” describes actual reachability, not comments alone.

| # | Mechanism and evidence | March | Current authority/overlap | Classification | Exact cutover/deletion owner |
|---:|---|:---:|---|---|---|
| 1 | Graph status/readiness: `src/graph.rs:407-482`, `src/query.rs:290-453` | yes | Compatibility row is still read as scheduling and dependency truth; 32 direct assignment sites remain. | **projection-only** | Single kernel projection applier; delete all §7 assignments. |
| 2 | Lifecycle kernel/ledger: `LifecycleKernel::transition`, `append_new_events`, `replay_ledger` (`src/lifecycle.rs:600-1494,1588-1667`) | no | Claims sole edge authority; also stores full event audit inside every task and in `.wg/lifecycle/events.jsonl`. | **fold** | Replace with the eight-request reducer; retain one journal, not per-task audit copies. |
| 3 | Reference lifecycle reducer: `src/lifecycle_protocol.rs:1-569` | no | Second model with different phases/capability/finish transaction; no production caller found. | **delete** | Delete module and fixtures after trace corpus targets the real kernel. |
| 4 | Direct fail-stop dispatcher: `spawn_agents_for_ready_tasks`, `src/commands/service/coordinator.rs:4356-4590` | yes | Actual dispatch authority; computes readiness, route and spawn directly. | **keep/fold admission only** | Keep deterministic `SpawnPlan`; reserve only through kernel. |
| 5 | PlannerStore: `src/service/planner.rs:1992-2548` | no | Trace/effect journal calls itself production authority, but service explicitly does not open it. | **migrate-once, delete** | `wg service` migration reads historical trace/status once; remove planner scheduler/store. |
| 6 | Convergence state/reducers: `src/service/convergence.rs:1-843,1314-1384` | no | Dispatch reducer marked legacy; finish reducers still read by finalize/status. | **migrate-once, delete** | Import unresolved holds/effects into kernel once; remove `ConvergenceState` and finish reducers. |
| 7 | Worker capability + request journal: `src/worker_control.rs:49-715` | no | Exact attempt authorization is useful; Pending/Completed is a second commit around mutation. | **fold** | Capability validator becomes kernel adapter; request outcomes move into the same kernel commit. |
| 8 | Completion v3: `completion_submit`, `completion_land`, `completion_done` | no | Current CLI path; immutable objects/reviews are strong, but `completion_done` writes Done directly. | **keep evidence, fold terminal** | Keep `completion/v3` CAS and verifiers; send exact receipt to kernel. |
| 9 | SaveTransaction v2: `src/save_transaction.rs:1-360`; `completion/v2/{journal,transactions,objects}` | no | 18 phases and a separate WAL; still used by fail/incomplete/legacy paths. | **migrate-once, delete** | Resolve each nonterminal v2 transaction to one kernel hold/effect/receipt, then remove. |
| 10 | Finalization v1: `FinalizationStore`, 18 phases, finish lease (`src/finalization/mod.rs:21-718`) | no | Another transaction, object store, journal and repository lease; mutation CLI is mostly retired but readers/adapters remain. | **migrate-once, delete** | Convert durable candidate/effect receipts into v3 objects/kernel receipts; delete store and lease. |
| 11 | Legacy `done`/terminal adapters: `src/commands/done.rs`; `src/commands/finalize.rs`; `src/commands/user.rs:137` | yes | Old implementation remains callable internally; terminal adapter can mint attempts for old rows. | **delete** | Route every terminal command to v3 receipt + kernel; remove old implementation. |
| 12 | Registry/PID identity: `src/service/registry.rs:42-399` | yes | Mutable agent status, heartbeat, PID identity, output/worktree paths; read as capacity and liveness. | **projection-only** | Rebuild process view from exact spawn/exit observations; no task transition writes. |
| 13 | Heartbeat watcher: `src/commands/heartbeat.rs`; generated wrapper at `spawn/execution.rs:3657-3671` | yes | Independent periodic liveness writer/helper process. | **delete** | Wrapper/process identity supplies exact alive/exit evidence; heartbeat becomes optional metric only. |
| 14 | Stream liveness: `check_stream_liveness`, `src/commands/service/triage.rs:170-270` | yes | Stream timestamp can override heartbeat, but stale stream warning uses another threshold. | **projection-only** | UI activity projection only; no reaper input. |
| 15 | Reaper/triage: `cleanup_dead_agents`, `src/commands/service/triage.rs:272-730` | yes | Marks registry Dead, requests AttemptLost, mutates failure/accounting and provider health. | **fold** | Exact exit adapter sends one kernel request; split accounting/provider observations into projections. |
| 16 | Worktree observer: `src/worktree_observer.rs:559-1178,1530-1830` | no | Separate helper, hash-linked activity, health, watcher lease, preservation and late-write state. | **projection-only/delete helper** | Keep checkpoint-time manifest verifier; remove always-on watcher and action/lease state. |
| 17 | Pi watchdog: `src/pi_watchdog/mod.rs:440-780,988-1899`; `src/commands/pi_watchdog.rs` | no | Duplicates source/process/fence/session/route, phases, guards, terminal receipt, budgets and actions. Production drops returned continuation actions. | **fold** | Pi adapter emits bounded observations/effect receipts to kernel; delete watchdog transition/action state and helper commands. |
| 18 | Provider health: `src/service/provider_health.rs:329-523`; writer in triage | no | Mutable counters/pause fields without provider event ID. Current direct dispatcher does not consult PlannerStore. | **projection-only** | Unique provider-event observations in kernel journal; derive counters, never task state. |
| 19 | Rolling telemetry: `src/telemetry/mod.rs:321-507` | no | Separate JSONL deduped by task/attempt/executor/bucket; computes another cooldown. | **projection-only** | Rebuild telemetry from unique observations; delete cooldown authority. |
| 20 | Assignment synthetic tasks: `eval_scaffold::scaffold_assign_task`, coordinator assignment, spawn fallback | yes | Assignment is Open blocking work on one path and a born-Done post-spawn receipt on another. | **delete** | Assignment becomes one source-bound admission receipt; delete `.assign-*` tasks and edges. |
| 21 | Evaluation/FLIP satellites and `eval_lifecycle`: `src/eval_lifecycle.rs`, `src/commands/eval_scaffold.rs` | yes | Synthetic task status plus immutable verdicts plus direct source terminal/retry writes. V3 also runs manifest-bound reviewers. | **delete tasks; keep evidence** | V3 review receipts remain evidence; kernel alone accepts/rejects; delete `.flip-*`/`.evaluate-*` state authority. |
| 22 | Remote-exec lease/acceptance: `LeaseLedger::try_commit`, `src/providers/lease.rs:197-340`; direct Done at `exec_fed_cmd.rs:1080-1120` | no | Correct epoch CAS exists, then graph usage bridge writes Done independently. | **fold** | Remote result receipt uses the same attempt key and terminal kernel request; lease becomes adapter evidence. |

**Count:** nine of these mechanisms existed in March; all 22 exist in the current tree. Dormant/retired code is included because it remains compiled, readable, callable internally, or migration-authoritative. Pure immutable artifact stores are not counted as authorities unless another component treats their presence as sufficient to transition.

The nine current independent reducer families counted in §2 are: `LifecycleKernel`, `lifecycle_protocol::reduce`, `SaveTransactionKernel`, `planner::plan`, the convergence reducers, `PiWatchdog::{observe,tick}`, `WorktreeObserver::reconcile_at`, `ProviderHealth` mutation/check logic and remote `LeaseLedger` CAS/reclaim. Direct command writers are additional issuers, not counted again as reducer families.

### 4.1 Durable state-enum inventory

| Domain | Current persisted/control states | Writers | Readers that act on it | Deletion result |
|---|---|---|---|---|
| Task graph | `Open`, `InProgress`, `Waiting`, `Done`, `Blocked`, `Failed`, `Abandoned`, `PendingValidation`, `PendingEval`, `FailedPendingEval`, `Incomplete` (`src/graph.rs:382-482`) | lifecycle projection + §7 | query, dispatcher, dependency and UI paths | Six projection values; no direct writer. |
| Lifecycle attempt | `AttemptDisposition::{Succeeded,Failed,Parked,Cancelled,Lost}` and `PiAuthorizationState::{Active,HeldOperatorRequired,Consumed,Revoked}` (`src/lifecycle.rs:66-96`) | lifecycle reducer | broker, watchdog, retry/finalization | Fold terminal outcome into the one attempt phase; delete Pi sub-state. |
| Reference protocol | `TaskPhase::{Running,Done,Failed}`; `PendingAction::{ResumeSame,BeginFinish,Promote,Cleanup}` (`src/lifecycle_protocol.rs:14-69`) | `reduce` | trace replay only | Delete whole schema. |
| SaveTransaction | 18 `SavePhase` values from `Absent` through `NeedsReconciliation` (`src/save_transaction.rs:19-53`) | save reducer/adapters | worker broker, convergence, finalize/status | Import receipt/hold/effect, delete phases/store. |
| Finalization | 18 `FinalizationPhase` values from `NeedsFinalization` through `OperatorHold` (`src/finalization/mod.rs:21-42`) | FinalizationStore functions | finalize, evaluation, convergence, why-blocked | Convert immutable receipts to v3; delete phase machine. |
| Planner | four `OwnerEvidence`; two readiness; two admission; 15 `ActionKind`; two `EffectStatus`; four `EffectExecutionPhase` (`src/service/planner.rs:121-260,786-812,1863-1880`) | PlannerStore/reducer | replay/status only in current daemon configuration | Archive trace once, delete schema/store. |
| Convergence | nine `ConvergenceStage`; three `RouteBreakerState` (`src/service/convergence.rs:291-414`) | convergence reconciliation | why-blocked, Pi status, finish compatibility | Import unresolved hold once, delete. |
| Registry | nine `AgentStatus` values (`Starting`…`Dead`) (`src/service/registry.rs:37-59`) | spawn, heartbeat, freeze, reaper, completion | capacity, reaper, status, worktree reuse | Process projection only; no lifecycle edge. |
| Pi watchdog | nine `Classification`, six `Phase`, four `ActionState` values (`src/pi_watchdog/mod.rs:20-42,552-575`) | native parser/watchdog commands | bridge, wrapper, status, lifecycle sync | Bounded observation adapter; no persisted action state. |
| Worktree observer | four `ObserverHealth` plus preservation/reap/quarantine booleans (`src/worktree_observer.rs:425-470`) | observer helper/reconcile CLI | show/status, spawn reuse, Pi sync | Checkpoint-time evidence projection only. |
| Provider health/telemetry | per-route count, pause, cooldown and three time windows (`src/service/provider_health.rs:309-523`; `src/telemetry/mod.rs:420-507`) | triage + telemetry CLI | status/historical scheduler | Derived unique-event projection. |
| Remote lease | epoch, provider, `committed`, renewal and verification fields (`src/providers/lease.rs:124-340`) | provider offer/grant/accept/reclaim | remote accept/reclaim/verify | Attempt receipt adapter; no separate terminal state. |

### 4.2 Complete `LifecycleKernel` transition inventory

All 28 current variants are accounted for below; none may survive as a second issuer during cutover.

| Current variants | Current edge/effect | Target classification |
|---|---|---|
| `AttemptReserved`, `AttemptRunning`, `ReservationCancelled` | Open → InProgress, running evidence, rollback | Fold into `ReserveAttempt`, `MarkRunning`, terminal cancel. |
| `AttemptSucceeded`, `DurableSuccessProjected`, `GraphSaveCommitted` | Three success authorities | Keep only one success intent + exact v3 receipt/effect acknowledgement. |
| `AttemptFailed`, `AttemptLost` | terminal failure/lost | Fold into terminal intent or exact-exit reconciliation. |
| `AttemptParked`, `WaitSatisfied` | InProgress → Waiting → Open | Fold into terminal park/checkpoint and exact wait receipt. |
| `AcceptanceSatisfied`, `AcceptanceRejected` | Pending* → Done or retained PendingEval | Delete Pending* states; evidence is consumed by one terminal decision. |
| `GenerationCreated`, `ReopenRequested`, `ReopenOwnerReleased` | cancel/fence and later reopen | Fold into `OpenNextAttempt`; attempt IDs replace generation sub-protocol. |
| `Abandoned` | nonterminal → Abandoned | Terminal cancel/abandon request. |
| `AdmissionDeferred`, `EvaluationEvidence`, `CandidateCheckpointed`, `ReconciliationIssue`, `MessageObserved` | evidence-only events that still increment lifecycle revision | Projection/evidence only; append only when needed by a kernel decision. |
| `LegacyCheckpointImported` | compatibility evidence | Migrate once, delete. |
| `PiContinuationAuthorized`, `PiContinuationHeld`, `PiContinuationEpochReserved`, `PiProcessEpochReplaced`, `PiTerminalIntent`, `PiProcessEpochExited` | Pi-specific attempt/process/continuation controller inside lifecycle | Replace with generic observation, effect and terminal requests; delete all six. |

## 5. Fact/writer/reader/persistence map

This table is the exhaustive semantic inventory for the audited control-plane slice. “Replay key” states the current key, not the desired one.

| Semantic fact | Current writers | Readers / decisions | Current replay key | Terminal authority | Helper process | Durable copies | Disposition |
|---|---|---|---|---|---|---|---|
| Task lifecycle/status | Lifecycle projection plus §7 direct writers | readiness, dependency satisfaction, dispatcher, TUI, completion, reaper | lifecycle idempotency key where used; none for direct writes | many | none | graph row; lifecycle event projection/audit; lifecycle ledger | Graph becomes projection-only. |
| Current attempt/owner/fence | lifecycle reserve/reopen; spawn claim; terminal compatibility mint | worker broker, spawn CAS, watchdog, observer, finalizer | event key; attempt ID/fence | lifecycle and direct adapters | wrapper | graph lifecycle, capability registry, metadata, observer/Pi/finalization/save records | One attempt key in kernel. |
| Worker request result | worker broker `begin_request`/`complete_request` | IPC replay | request ID + token digest + operation CID | special Done replay can rerun terminal verifier | worker CLI/daemon IPC | `worker-capabilities.json`; audit JSONL | Fold into kernel commit. |
| Dispatch eligibility | query + direct coordinator; dormant planner/convergence | coordinator spawn | none/direct; planner observation sequence/effect ID | no terminal, but creates attempt | daemon | graph/config; planner trace/state/effect; convergence state | Keep pure readiness/SpawnPlan; delete schedulers. |
| Assignment satisfied | coordinator LLM, agency server, spawn fallback | query dependency gate, dispatcher, UI/history | `.assign-<task>` ID; assignment record | `.assign-*` Done writers | inline assign worker or one-shot LLM | graph synthetic task/edge; agency assignment file; source.agent | One admission receipt, no task. |
| Evaluation accepted | eval lifecycle, v3 review valve, finalization receipt/waiver | source completion/retry, dependency readiness | pipeline/source-attempt/verdict IDs; v3 receipt digest | eval lifecycle and completion/finalizers | eval/flip one-shots | graph satellites/source fields; verdict files; v3/v2/finalization objects | Keep v3 receipt only; kernel reads it once. |
| Completion candidate/publication | v3 submit/land; v2 WorkSave; FinalizationStore | v3 done, finalizers, convergence, status | manifest/CID; save transaction/action keys; candidate ID | v3 done, GraphSave, legacy done/finalize | Git and review subprocesses | `completion/v3`; `completion/v2`; `finalization`; Git refs; graph refs | Keep v3 immutable bytes; delete v1/v2 controllers. |
| Terminal disposition | v3 done; lifecycle kinds; eval lifecycle; human dispatch; remote exec; legacy commands | readiness/dependencies/reaper/accounting | subsystem-specific | plural | wrapper/IPC/finalizer | graph, lifecycle ledger, request journal, completion stores, registry | Kernel first-terminal only. |
| Process alive/dead | OS PID/start identity; registry status; heartbeat; stream timestamp; observer parent check | capacity, reaper, watchdog, cleanup | PID/start, heartbeat time, stream offset, observer epoch | reaper can request Lost | heartbeat and observer | registry, stream files, observer/Pi state | Exact exit is kernel input; all else projection. |
| Worktree ownership/activity | spawn, registry, observer, watchdog, finalization lease, save key | spawn reuse, completion, cleanup, TUI | fence/lease epoch, root digest, observer sequence | can hold/fail finalization | worktree observer | graph/registry/metadata, observer state+journal, Pi state, v1/v2 records | One attempt fence; checkpoint verifier on demand. |
| Pi continuation/terminal | watchdog + lifecycle Pi variants | wrapper, bridge, status, convergence | action ID, stream offset, process/continuation epoch | Pi terminal + wrapper fail + lifecycle | stream observer/watchdog CLI | Pi state/progress, lifecycle graph/ledger, session marker | Adapter observation/effect only; delete duplicate state. |
| Provider failure/health | triage health tracker; telemetry recorder | status; historical planner; operators | no provider event ID; telemetry composite tuple | no legitimate terminal authority, but reaper already failed task | telemetry CLI | provider health JSON, telemetry JSONL, task failure, outcome sidecar | Unique observation projection. |
| Message/wait | messages append/status/cursor; wait matcher/human dispatch | worker poll, coordinator wait paths | message ID/cursor/wait receipt | human dispatch directly writes Done in one path | none | message JSONL, cursor, graph wait fields | Message inert except exact wait receipt through kernel. |
| Retry/cycle/cron | retry/reset/replay/recover, eval rescue, cron, cycle helpers | readiness/dispatcher | mixed generation/idempotency keys or none | creates later execution | daemon | graph counters/status/log, lifecycle events | `OpenNextAttempt` only; delete direct reopen writes. |
| Remote provider lease | provider CLI ledger | accept/reclaim/verify, graph usage bridge | task + lease epoch + committed flag | direct remote Done bridge | remote provider worker | `exec/leases.json`, result envelope, graph | Fold exact receipt into attempt kernel. |

## 6. Persisted copies and subprocess inventory

### 6.1 Current durable control-store families (18)

1. `graph.jsonl` task/lifecycle projection;
2. `lifecycle/events.jsonl`;
3. `service/registry.json`;
4. `service/worker-capabilities.json` plus capability audit;
5. `completion/v3/objects`;
6. `completion/v2/{journal,transactions,objects,graph-saves}`;
7. `finalization/{transactions,journal,objects,leases}`;
8. `service/decision-trace-v*.json`, planner state and effect journal;
9. `service/convergence-state.json`;
10. `attempts/.../worktree-observer/{state,baseline,policy,runtime,activity}`;
11. `attempts/.../pi/{state,progress}` plus legacy worktree Pi state;
12. raw/canonical stream, output and metadata files;
13. `service/provider_health.json` and per-agent `outcome.json`;
14. `service/provider-telemetry.jsonl`;
15. agency assignment/evaluation/verdict records;
16. messages JSONL and cursor/status rewrites;
17. `exec/leases.json` remote lease ledger; and
18. coordinator/daemon state and breaker/accounting projections.

The March slice had five corresponding families: graph, registry, daemon/coordinator state, output/streams and agency/message records.

### 6.2 Current maximum per-attempt helper roles (seven)

| Role | Launch evidence | Decision authority today | Target |
|---|---|---|---|
| shell wrapper | `spawn/execution.rs:3610-3780` | orchestrates terminal fallbacks | **keep one** |
| heartbeat watcher | `spawn/execution.rs:3657-3671` | missing heartbeat feeds reaper | **delete** |
| worktree observer | `spawn/execution.rs:3644-3654` | persisted health/preservation/action state | **delete helper** |
| Pi stream observer | `spawn/execution.rs:3513` | invokes watchdog parser | **fold into wrapper capture** |
| Pi watchdog bootstrap/exit CLI | `spawn/execution.rs:3513,3599` | updates watchdog + lifecycle | **delete; wrapper sends observation** |
| Pi stream bridge | `spawn/execution.rs:3520-3521` | writes canonical accounting/summary | **fold post-run projection** |
| failure classifier/telemetry CLI | `spawn/execution.rs:3675-3692,3708-3776` | writes task/provider evidence through separate paths | **fold one post-run observation** |

The workload model process is excluded. In March, the only counted control role was the wrapper; stream `tee`/capture was transport, not a WG controller.

## 7. Direct task-status writer burn-down

The current pre-test scan finds 32 task/projection assignments in these 20 files. They are the concrete deletion checklist.

| File | Current sites | Authority represented | Cutover owner |
|---|---|---|---|
| `src/eval_lifecycle.rs` | 1151, 1176, 1302, 1377, 1912 | evaluator satellite/source repair/terminal | v3 evidence + kernel |
| `src/matrix_commands.rs` | 367, 503 | claim normalization/unclaim | kernel request adapter |
| `src/cron.rs` | 220 | terminal reopen | `OpenNextAttempt` |
| `src/commands/incomplete.rs` | 84, 106 | retry/fail | terminal intent + retry policy |
| `src/commands/agent.rs` | 499 | agent failure | terminal intent |
| `src/commands/completion_done.rs` | 262 | current Done | completion receipt to kernel |
| `src/commands/kill.rs` | 345, 425 | abandon/reopen | cancel + next attempt |
| `src/commands/dead_agents.rs` | 167 | dead-owner reopen | exact exit + next attempt |
| `src/commands/add.rs` | 891 | parent waiting | park/wait request |
| `src/commands/evaluate.rs` | 2439 | evaluation retry | evidence only |
| `src/commands/chat_cmd.rs` | 1005, 1077 | chat terminal state | kernel adapter or non-task chat state |
| `src/commands/finalize.rs` | 247 | compatibility attempt mint | migration only, then delete |
| `src/commands/exec_fed_cmd.rs` | 1111 | remote Done | remote receipt to kernel |
| `src/commands/replay.rs` | 272 | reopen | `OpenNextAttempt` |
| `src/commands/migrate.rs` | 341 | legacy abandon | migration event only |
| `src/commands/edit.rs` | 1032 | assignment-task abandon | delete synthetic task |
| `src/commands/completion_repair.rs` | 239, 244, 288 | legacy quarantine projection | one-time migration |
| `src/commands/service/human_dispatch.rs` | 103 | wait state | kernel wait request |
| `src/commands/service/mod.rs` | 2293 | legacy daemon-task abandon | one-time migration |
| `src/commands/service/ipc.rs` | 2733, 2881, 3040 | chat cleanup/done/reopen | kernel/chat adapter |

`src/lifecycle.rs`'s own `apply_projection` assignment is intentionally not in this burn-down; it is the one site that survives, moved to the reduced kernel. Message `DeliveryStatus` is a different enum and is excluded.

## 8. Compatibility deletion list (15)

| # | Path | Evidence | Action |
|---:|---|---|---|
| 1 | `pending-review` spelling → Done | `src/graph.rs:444-482` | migrate once, remove parser alias |
| 2 | lifecycle rows without `AttemptRef` accepted once | `src/lifecycle.rs:1438-1478` | import as historical attempt or hold; remove branch |
| 3 | `PendingValidation` auto migration | `src/lifecycle.rs:1670-1712` | migrate once, remove status/loop |
| 4 | terminal adapter mints/repairs attempts | `src/commands/finalize.rs:200-267` | migrate once; no runtime mint |
| 5 | completion v2 legacy quarantine/store | `src/commands/completion_repair.rs:1-20,142-350` | run report/apply once; archive read-only |
| 6 | completion head without v2 journal | `src/worker_control.rs:770-800` | import verified head once |
| 7 | legacy finalization receipts bridged to v2 | `src/commands/finalize.rs:271-540,1900-2100` | convert to v3 receipt once |
| 8 | Planner schema/state upgrades | `src/service/planner.rs:2000-2057` | report/export only, then delete store |
| 9 | convergence imported into planner | `src/service/planner.rs:2057-2068` | import unresolved actions directly into kernel once |
| 10 | legacy convergence file remains readable | `src/service/convergence.rs:1-6,450-478` | archive after import |
| 11 | capability worktree-path rebind | `src/worker_control.rs:589-638` | migrate binding once before dispatch |
| 12 | flat attempt runtime slots | `src/attempt_runtime.rs:76-261` | index/copy once, never probe at runtime |
| 13 | old worktree Pi watchdog path | `src/commands/pi_watchdog.rs:59-88` | import once, then remove fallback |
| 14 | Pi watchdog schema-v1 epoch repair | `src/pi_watchdog/mod.rs:900-930` | migrate state once, then delete watchdog state |
| 15 | provider `legacy:<executor>` bucket | `src/commands/service/triage.rs:600-640` | retain as historical telemetry only; never route/gate |

March had only item 1 in this exact list. A compatibility path is complete only when its production branch is deleted, not when it is labelled legacy.

## 9. Incident traces and expected single-kernel outcomes

All eight fixtures are bounded JSON, contain no prompts, paths requiring local credentials, provider responses or executable commands, and name their source evidence.

| Trace | Current contradiction | Expected single-kernel result |
|---|---|---|
| `01-lost-ipc-response-after-durable-mutation.json` | graph/message mutation durable while request remains Pending | mutation + response share one commit; replay returns original response |
| `02-live-pid-missing-heartbeat.json` | exact live PID can be marked Dead/AttemptLost on missing heartbeat | heartbeat absence is evidence only; current attempt remains Running |
| `03-duplicate-provider-event.json` | no common provider event ID across health and telemetry | one unique event increments once; duplicate is inert |
| `04-repeated-threshold-compactions.json` | epochs/markers can advance while delivery actions are discarded | one compaction ID issues at most one acknowledged continuation effect |
| `05-child-exit-observer-eof.json` | EOF, child exit, continuation and wrapper failure nominate different actions | EOF inert; exact exit issues one disposition-reconciliation effect |
| `06-stale-capability.json` | safe check is duplicated across three reducers/state shapes | one attempt-key rejection, no effect or breaker charge |
| `07-assignment-plumbing-completion.json` | assignment is either blocking Open work or born-Done post-spawn receipt | one admission receipt; no synthetic task/edge |
| `08-completion-response-replay.json` | Done and response are separate, repaired by re-running completion verifier | terminal receipt + response atomic; replay performs no verifier/effect |

### Credential-free trace validation

This replays the recorded state chain and validates schema, count, ordering and exact final projection; it performs no subprocess or network call:

```bash
python3 - <<'PY'
import json, pathlib
root = pathlib.Path('docs/research/traces/wg-control-plane-authority-deletion-map')
index = json.loads((root / 'index.json').read_text())
assert index['incident_count'] == 8 and index['credential_free'] is True
for name in index['traces']:
    trace = json.loads((root / name).read_text())
    assert trace['trace_schema'] == index['trace_schema']
    assert trace['credential_free'] is True
    state = trace['single_kernel_replay']['initial']
    for sequence, step in enumerate(trace['single_kernel_replay']['steps'], 1):
        assert step['seq'] == sequence
        assert step['before'] == state
        state = step['after']
    assert state == trace['single_kernel_replay']['expected_final']
print('8 credential-free traces: OK')
PY
```

These are audit traces, not claims that the current production reducers already yield the expected projection. `current_trace` records current behavior; `single_kernel_replay` records the deletion target.

## 10. Deletion sequence and no-accretion freeze

### 10.1 Ordered cutover

Each step must remove the old issuer in the same change that activates the replacement.

1. **Freeze and measurement.** Check in the writer/LOC/store/process counters as CI-readable audit data. No behavior change.
2. **Kernel and request atomicity.** Reduce `LifecycleKernel`; fold worker request outcomes into its commit. Cut over stale capability, terminal and lost-response traces first.
3. **Dispatch only.** Keep direct readiness and `SpawnPlan`; reserve through the kernel. Migrate then delete PlannerStore and ConvergenceState rather than activating either.
4. **One observation lane.** Wrapper submits exact start/exit plus bounded provider/stream facts. Delete heartbeat, worktree-observer and Pi-observer controller roles; retained raw bytes are evidence.
5. **Assignment/evaluation projection.** Replace `.assign-*`, `.flip-*`, `.evaluate-*` lifecycle tasks with source-bound receipts; delete their status/edge writers.
6. **One completion protocol.** Keep v3 immutable objects/review/publication verification. Send one receipt to the kernel. Migrate v1/v2/finalization state, then delete `done.rs`, SaveTransaction and FinalizationStore mutation paths.
7. **Remote/cron/retry.** Route remote result, wait, retry, cycle and cron through the same attempt requests. Delete remaining direct writers.
8. **Compatibility removal.** Run each §8 migration once, archive evidence, remove branches/status variants, then enforce zero direct status writes.

### 10.2 Freeze policy (effective immediately for this program)

Until the target scorecard is met:

* No new task status, attempt phase, lifecycle event kind, control journal, sidecar state, background/helper process, controller thread, retry counter, breaker, lease, synthetic control task or compatibility reader may land.
* A control-plane change is admissible only when it **deletes or replaces an existing path in the same change** and is net-negative in at least one scorecard metric without increasing another.
* “Projection-only” code may be added only by removing its mutation/effect API in the same change.
* A migration must be one-shot, idempotent, version-bounded, observable and accompanied by deletion of the compatibility reader by the next release. Dual-write is prohibited.
* A release-blocking safety defect is the sole exception. Its change must name the incident, contain no feature expansion, add an expiry/deletion issue, and receive explicit release-owner approval.
* New replay fixtures are allowed; new replay engines/controllers are not.
* Review must reject claims such as “sole authority” unless a production scan proves zero bypass writers and the old store/process is deleted.

### 10.3 Merge gate during freeze

Every control-plane PR must report:

```text
authority mechanisms: before -> after
production LOC manifest: before -> after
direct status sites: before -> after
reducers / typed transition variants: before -> after
durable stores / journals: before -> after
helper process roles: before -> after
compatibility paths: before -> after
old issuer deleted in this change: path::symbol
```

A neutral or positive total is rejected unless it is the documented release-blocking exception.

## 11. Reproducibility and evidence limits

### Fixed LOC manifest

The seven groups in §2 contain:

* graph/readiness: `graph.rs`, `parser.rs`, `query.rs`, `cron.rs`;
* attempt kernel/broker: `lifecycle.rs`, `lifecycle_protocol.rs`, `attempt_runtime.rs`, `worker_control.rs`;
* completion/finalization: `save_transaction.rs`, `completion_evidence.rs`, `completion_manifest.rs`, `completion_review.rs`, `completion_review_model.rs`, `completion_task.rs`, `finalization/mod.rs`, `work_save.rs`, and command files `done`, `finalize`, `completion_{submit,land,done,repair}`, `work_save`;
* daemon/dispatch: service `planner`, `convergence`, command service `coordinator`, `ipc`, `mod`, and spawn `execution`, `worktree`;
* liveness/observers/provider: registry, provider health, telemetry, worktree observer, Pi watchdog, stream event, and command heartbeat/reap/Pi bridge/watchdog/observer/triage/zero-output files;
* agency/synthetic: assignment eligibility, eval lifecycle, service assignment, evaluate, eval scaffold and assign; and
* terminal/recovery/remote: claim/lifecycle, fail, incomplete, retry, reset, recover, requeue, sweep, abandon, kill, dead-agents and exec-federation command files.

Reproduction algorithm: `git show REV:path`, truncate at the first line matching `^[[:space:]]*#[cfg(test)]`, then count physical lines. A missing path contributes zero. This produced exactly 17,527 and 52,304.

### Source searches used

```bash
# Direct task-status candidates; inspect types and exclude DeliveryStatus.
rg -n '\.status\s*=\s*.*Status::' src --glob '*.rs'

# Durable control files and independent writes.
rg -n 'write_atomic|save_ref|sync_all|jsonl|state\.json|registry\.json|head\.json' src

# Control helper launches.
rg -n 'heartbeat-watch|worktree-observer-run|pi-stream-observe|pi-watchdog|pi-stream-bridge' \
  src/commands/spawn/execution.rs src/commands/service/coordinator.rs

# Planner/convergence production reachability.
rg -n 'PlannerStore|ConvergenceState|reconcile_dir|reduce_exited_worker_finish' src --glob '*.rs'
```

Line numbers are tied to the current audited revision. Negative reachability claims are bounded to repository Rust production source: no caller of `lifecycle_protocol::reduce` was found; the daemon explicitly disables PlannerStore; production Pi action-return sites discard the vector as detailed in `docs/research/wg-pi-compaction-continuation-seams.md`.

## 12. Exit criterion

The deletion program is complete only when:

1. every task/attempt transition enters one reducer;
2. every exact replay returns one recorded response without semantic re-execution;
3. the eight traces reduce to their expected final projections through production kernel code;
4. graph, registry, telemetry and observer surfaces are provably projection-only;
5. `.assign-*`, `.flip-*` and `.evaluate-*` have no lifecycle authority;
6. v1/v2 completion/finalization and planner/convergence stores have been migrated and deleted;
7. no per-attempt control helper remains except the wrapper; and
8. the freeze scorecard reaches its target with zero compatibility branches.

Until then, adding control-plane behavior is authority accretion, not progress.
