# Core runtime synthesis: architecture, orchestration, and execution plane

**Audit date:** 2026-08-08

**Evidence checked through:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Synthesis checkout:** `b72cdb9f26afb60e5e77f211a6c8514a90598fa8`; audited production paths were byte-equivalent to the snapshot (evidence S1)

**Freshness:** snapshot-current for cited source; inherited executions are qualified by their leaf artifacts

**Scope:** synthesis of code architecture/persistence, orchestration/lifecycle, and model/execution-plane evidence

**Change boundary:** this new audit artifact only

**Normative charter:** [`README.md`](README.md), especially its fan-in and evidence contract at `README.md:151-194,196-374`

## 1. Executive abstract

**`[FACT]`** This synthesis read the charter and all three required dependency artifacts in full: [`10-code-architecture.md`](10-code-architecture.md), [`11-orchestration-lifecycle.md`](11-orchestration-lifecycle.md), and [`12-model-execution-plane.md`](12-model-execution-plane.md). It then checked material and disputed claims against primary source. No product, test, or pre-existing documentation file was changed. This synthesis ran source-provenance and artifact validation commands only; it did not rerun the leaf audits' product tests, daemons, smokes, or provider traces.

**`[FACT]`** WorksGood's core is a file-backed, event-repaired task graph surrounded by a daemon, capability-brokered workers, model-specific processes, immutable completion objects, Git publication, and several operational projections. The strongest implemented path is:

```text
paused task -> atomic publish -> derived readiness -> fenced attempt
-> durable workspace/registry -> launch gate -> model process
-> raw/canonical events -> immutable candidate -> exact FLIP+eval
-> publication -> receipt-backed Done -> dependency authorization
```

**`[FACT]`** The graph mutation spine is serialized and replayable on Unix; launch is gated until ownership and workspace state are durable; worker capabilities switch authority before graph discovery; unsupported unattended model routes fail closed; and modern Done is re-derived from immutable review and publication (`src/parser.rs:275-414`; `src/lifecycle.rs:605-615,1291-1507`; `src/main.rs:734-748,1261-1274`; `src/commands/spawn/execution.rs:1283-1438,3430-3707`; `src/commands/completion_done.rs:29-294`).

**`[INFERENCE]`** The dominant core risk is **authority fragmentation during migration**, not the absence of safeguards. Equivalent decisions are made at different layers by different predicates: ready versus manual claim; handler discovery versus strict worker admission; normal Pi worker versus hermetic Pi RPC handler; current completion-v3 versus retained legacy finalization; canonical event translation versus terminal accounting projection. Confidence is high because the same shape appears independently in all three leaves and in direct source checks. The falsifying condition would be a single typed authority used by every listed entry point; the sampled source instead contains the splits cited below.

**`[VERIFIED]`** The highest-impact inherited runtime observation is daemon-wide head-of-line blocking during synchronous completion review: unrelated worker Done/Show requests reached their 30-second client deadline while the installed daemon remained under a reviewer process. The installed binary was not commit-identifiable, so applicability to the pinned build is not itself verified; however, snapshot source accepts IPC and executes `SubmitCompletion -> completion_submit::run` inline on the coordinator thread (`src/commands/service/mod.rs:3330-3570`; `src/commands/service/ipc.rs:286-350,835-919`; full trace in `11-orchestration-lifecycle.md` §7.3 Trace C). This synthesis ranks the cross-layer impact **S1 High, medium confidence**, raising the leaf's S2 because one slow external review can suspend the graph's whole worker-control and scheduling lane rather than only one task (`CORE-001`).

**`[VERIFIED]`** Two bounded, current-path failures are also material. Manual `wg claim` admitted an unpublished paused task and a future-delayed task even though `wg ready` excluded both (`11-orchestration-lifecycle.md` §7.3 Trace B; `src/commands/claim.rs:11-151` versus `src/query.rs:306-343`). Separately, a daemon-launched fake Pi worker reached reviewed Done with correct non-zero raw and canonical usage, but stored `task.token_usage` remained null and `wg spend` showed zero (`12-model-execution-plane.md` §7.5). Direct source checking confirms the accounting symptom but **rejects the leaf's specific legacy-branch explanation**: ordinary and worker-brokered Done call `completion_done::run`, whose commit path writes no usage; the cited `done.rs -> task_owned_done` branch is reachable only through retained special/legacy callers, not normal Done (`src/main.rs:1261-1274`; `src/commands/service/ipc.rs:885-919`; `src/commands/completion_done.rs:29-294`; call-site command S2). This is recorded, not smoothed over, in `CORE-DRIFT-007`.

**`[RECOMMENDATION]`** The first implementation decision is to protect the control plane: move long completion/reviewer work off the daemon coordinator thread into a bounded, idempotent per-task executor (`CORE-REC-001`). In parallel, elect one admission function and one surface-qualified execution capability matrix (`CORE-REC-002`, `CORE-REC-003`). The first synchronization action is to classify completion tests/help/docs by current, compatibility, or retired authority and correct the accounting root-cause record (`CORE-REC-006`, `CORE-REC-007`).

## 2. Scope and unified map

### 2.1 Inputs, exclusions, and fan-in disposition

**`[FACT]`** The three input leaves have different evidence strengths and emphases:

| Input artifact | Major contribution represented here | Inherited executed evidence | Synthesis treatment |
|---|---|---|---|
| [`10-code-architecture.md`](10-code-architecture.md) | package/binary boundaries; broad models; graph/lifecycle persistence; configuration layers; duplicate completion/resolution authorities; provenance and platform gaps | focused build, locking/replay tests, add/show/config trace, stale Done test | Adopted architecture and persistence map; completion severity reconciled with orchestration; no-lost-update result not generalized beyond Unix/sample. |
| [`11-orchestration-lifecycle.md`](11-orchestration-lifecycle.md) | staged publication; readiness; attempt/generation/fence; spawn transaction; completion; retry/cycles/waits/cron; daemon IPC; documentation/test drift | two manual CLI traces, 15 targeted Rust binaries, six smokes, installed-daemon observation | Adopted current lifecycle authority and verified claim bypass; narrowed daemon observation by binary provenance; represented all ORCH finding groups in §3.4. |
| [`12-model-execution-plane.md`](12-model-execution-plane.md) | strict worker route set; config/role/reasoning precedence; process topology; Pi bridge/watchdog/accounting; setup and documentation drift | focused route/stream/watchdog/profile/native tests, generated Pi wrapper trace, two passing and one failing smoke | Adopted capability split, Pi topology, and observed accounting gap; rejected only the legacy `done.rs` causal attribution for MODEL-009 after current call-graph checking. |

**`[UNCERTAINTY]`** This synthesis does not independently execute full Cargo tests, the full smoke manifest, Windows writers, real external models, provider credentials, power-loss injection, production-duration watchdogs, or a daemon built with embedded revision identity. The leaf commands remain the E1 evidence for their exact environments; links above preserve their bounded transcripts.

### 2.2 Unified component and authority map

**`[FACT]`** One Cargo package exposes a broad library and four binaries. The core `wg` binary additionally owns a large private CLI/command/TUI layer. Durable state is split across the graph projection, lifecycle ledger, attempt runtime, registry, configuration/profile files, provenance log, completion object store, streams, and Git (`Cargo.toml:1-41`; `src/lib.rs:20-144`; `src/main.rs:17-25`; detailed inventory in `10-code-architecture.md` §2.2–2.4).

```text
Operator / worker command
  |
  +-- worker capability present? --> typed own-task IPC (before graph discovery)
  |                                      |
  |                                      v
  +-- operator CLI -----------------> daemon main IPC lane
                                         |
Configuration/profile ------------------+--> strict role route + reasoning
Graph + lifecycle ledger ---------------+--> ready / claim / fenced attempt
                                         |
Registry + attempt runtime + worktree ---+--> launch permit --> handler process
                                                               |
                                      raw_stream / stream / summary
                                                               |
Completion object store <--- manifest ---+--> FLIP --> eval --> Git/artifact publication
                                                               |
                                         receipt-backed lifecycle Done
                                                               |
                                         graph projection + dependent readiness
```

**`[FACT]`** The authoritative-versus-projection relationships are:

| Semantic dimension | Current authority | Projection or compatibility surface | Boundary quality |
|---|---|---|---|
| Task lifecycle edge | `LifecycleKernel` plus append-only ledger (`src/lifecycle.rs:605-1507`) | `Task.status`, generation/fence and logs in `graph.jsonl` | Strong replay/CAS spine; broad compatibility states remain. |
| Graph structure | `Task.after` is used by readiness/reverse derivation (`src/query.rs:306-517`) | persisted `Task.before` backlink | Authority is de facto, not type-enforced (`ARCH-003`). |
| Mutation serialization | `modify_graph` exclusive `graph.lock`, ledger append, temp replace (`src/parser.rs:275-414`) | lockless/nonblocking composite reads | Strong Unix lost-update control; no-op non-Unix lock and parent-fsync gap. |
| Execution ownership | current generation/attempt/fence plus capability binding | registry PID, observer, worktree and attempt files | Transactionally gated, but cross-store rather than one transaction. |
| Worker eligibility | `validate_execution_model_plane` permits exact Pi/Claude/Codex (`src/config.rs:3590-3710`) | handler resolver, discovery catalog, templates and live handlers | Intentionally surface-dependent but poorly labeled. |
| Completion truth | immutable candidate, exact review receipts, publication and completion receipt (`src/commands/completion_submit.rs:187-487`; `completion_done.rs:29-294`) | legacy finalize/done modules and historical statuses/tests | Current authority is clear at root dispatch; migration inventory is not. |
| Event truth | raw handler stream plus canonical bridge (`src/commands/spawn/execution.rs:3430-3511`; `src/stream_event.rs:410-690`) | stored `Task.token_usage`, show/spend projections | Pi translation is correct in fixtures; terminal persistence has a current-path hole. |
| Effective configuration | merged global/local plus selected project-profile overlay (`src/config.rs:5925-6142`) | active global profile, legacy executor fields, defaults and task overrides | Provenance exists in parts; many callers and authorities remain. |

### 2.3 End-to-end orchestration and execution flow

**`[FACT]`** Creation and admission begin with `wg add`, which always persists an `Open` task with `paused=true`; publication validates and unpauses an explicitly selected task region; ready derives status, pause, time and relationship-aware required-success dependencies (`src/commands/add.rs:355,614-617,847-975`; `src/commands/resume.rs:164-350`; `src/query.rs:306-517`). The manual claim path only shares dependency disposition, not the pause/time portion, which is the concrete break in the otherwise unified flow (`CORE-003`).

**`[FACT]`** The service performs maintenance and bounded dispatch, creates or verifies a workspace, reserves registry capacity, projects a fenced attempt, writes capability/observer state, starts a gated wrapper, and only then publishes the launch permit. Pre-permit failures roll back rather than allowing an unowned process to run (`src/commands/service/coordinator.rs:2366-2702`; `src/commands/spawn/execution.rs:1283-1438,1780-2160`).

**`[FACT]`** Route selection is a two-stage system. Broad planning and live surfaces recognize more handlers, but service/worker admission accepts only exact `pi:<provider>:<model>`, `claude:<model>`, and `codex:<model>` with effective reasoning (`src/dispatch/handler_for_model.rs:45-137`; `src/dispatch/plan.rs:387-809`; `src/config.rs:2395-2433,3590-3710`). The process wrapper then supplies handler-specific argv/environment and captures streams. This is fail-closed for unattended work but conflicts with templates, discovery labels, and lenient deprecation narratives (`CORE-006`).

**`[FACT]`** Current Pi workers run direct one-shot `pi --mode json` with provider/model/thinking and piped task prompt (`src/service/executor.rs:1729-1752`; `src/commands/spawn/execution.rs:1308-1403`). The distinct `wg pi-handler` attended/RPC topology runs `pi --mode rpc -e <embedded> -ne` with a compatibility handshake (`src/commands/pi_handler.rs:492-537,855-902,1012-1040`). Therefore “Pi integration exists” does not prove that the normal worker uses the hermetic plugin boundary (`CORE-007`).

**`[FACT]`** Completion selects the immutable candidate before external review, records exact FLIP/eval receipt references, verifies dependency outputs and publication again at Done, and commits `AttemptSucceeded` under generation/manifest checks (`src/commands/completion_submit.rs:187-487`; `src/commands/completion_done.rs:29-294`). This is a strong authorization invariant. The synchronous review call currently shares the daemon's coordinator thread, while terminal accounting is not incorporated into `completion_done`, producing two distinct projection failures around an otherwise strong completion protocol (`CORE-001`, `CORE-005`).

**`[FACT]`** Failure and recovery preserve old-owner safety: fail/lost terminalizes the exact attempt; retry records a `ReopenRequested` hold; only proof that the old owner is released permits `ReopenOwnerReleased`, generation advance, and redispatch. Cycle code uses the same hold but names it “reactivated” before release, creating latency/semantic drift rather than a safety bypass (`src/lifecycle.rs:874-991`; `src/commands/reopen.rs:1-328`; `src/graph.rs:3044-3567`; `ORCH-007`, `ORCH-013`).

### 2.4 Cross-layer invariants

**`[INFERENCE]`** The following invariants are the shortest reliable description of the core. Each is both an existing positive control and a criterion for judging the gaps:

1. **One semantic authority, rebuildable projections.** Status edges come from the lifecycle kernel; completion from immutable evidence/publication; reverse edges, read models, streams and usage should be derived projections.
2. **One default admission predicate per surface.** Ready, claim, direct spawn and service dispatch should agree unless an explicit, audited operator override names which gate it bypasses.
3. **Generation/attempt/fence continuity.** Every worker mutation, retry, review selection and terminal transition must bind the same task generation and current ownership.
4. **Durable ownership before execution.** Graph claim, registry entry, workspace isolation, observer/capability binding and launch token precede handler start.
5. **Execution identity continuity.** Effective profile/role/tier route, handler, inner model/provider, reasoning, credential owner, plugin topology and recorded metadata must describe the same child process.
6. **Immutable review before authorization.** Candidate bytes, requirements and dependency outputs are fixed before review; the reviewed publication, not an exit code or status button, authorizes Done and downstream work.
7. **Accounting before linkage release.** Raw/canonical usage must be resolved and persisted before terminal completion clears the agent linkage used by live fallback reads.
8. **No unbounded external work in global serialization lanes.** Graph locks and coordinator/IPC lanes protect commits and scheduling; model calls, smoke, Git work and long review belong in bounded task-scoped executors.
9. **Capability before path discovery.** A worker's opaque capability is a hard mode switch before arbitrary graph resolution, preventing confused-deputy filesystem fallback.

**`[INFERENCE]`** Most high-value findings are violations of invariants 1, 2, 5, 7, or 8; most positive controls are implementations of 3, 4, 6, and 9. This explains why the system can be locally safety-conscious yet operationally inconsistent.

## 3. Findings

### 3.1 Ranked cross-cutting findings

**Synchronization impact** ranks how many code/help/test/doc/operator surfaces must change together: **critical** (release authority or many planes), **high** (multiple user/runtime surfaces), **medium** (bounded subsystem), **low** (local).

| Rank / ID | Label and state | Severity / likelihood / confidence | Synchronization impact | Synthesis claim and leaf provenance |
|---:|---|---|---|---|
| 1 — `CORE-001` | `[VERIFIED]` installed; `[FACT]` source; current | **S1 High** / observed on installed daemon, possible at snapshot / **medium** | **critical** | Synchronous completion review occupies the daemon coordinator/IPC lane, so one slow external review can stall unrelated worker control and ticks. Severity raised from `ORCH-014` S2 because synthesis exposes graph-wide amplification. Binary provenance keeps confidence medium. See `11-orchestration-lifecycle.md` §3 `ORCH-014`, §7.3; `src/commands/service/mod.rs:3330-3570`; `src/commands/service/ipc.rs:286-350,835-919`. |
| 2 — `CORE-002` | `[CONTRADICTION]`; partial migration | **S1 High** / likely synchronization failure / **high** | **critical** | Completion-v3 is the current root authority, but Clap/help, manuals, legacy modules, integration tests and active smokes span direct Done, v2, and v3. Safety fails closed; the high severity is release/core-workflow ambiguity rather than an authorization bypass. Unifies `ARCH-005`, `ARCH-DRIFT-001..003`, `ORCH-006`, `ORCH-011`, `ORCH-016`, `ORCH-017`, and the `MODEL-009` causation conflict. |
| 3 — `CORE-003` | `[VERIFIED]`; current defect or undocumented override | **S2 Medium** / observed / **high** | **high** | Manual claim bypasses publication and schedule gates without a force flag, although its source calls claim “an execution admission edge, not an operator waiver.” `src/commands/claim.rs:11-78`; `src/query.rs:306-343`; `ORCH-003`; `ORCH-DRIFT-004`. |
| 4 — `CORE-004` | `[FACT]` + `[INFERENCE]`; current/partial | **S2 Medium** / likely drift / **high** | **critical** | Semantic authority is split across lifecycle ledger/graph projection, canonical/denormalized edges, v3/legacy completion, strict/broad route catalogs, and global/local/profile configuration. These are one migration-overlap root cause, not independent trivia. Unifies `ARCH-002`, `ARCH-003`, `ARCH-005`, `ARCH-007`, `ORCH-011`, `MODEL-001`, `MODEL-003`, and `MODEL-010`. |
| 5 — `CORE-005` | `[VERIFIED]`; current | **S2 Medium** / observed on generated Pi wrapper / **high** | **high** | Reviewed Pi completion retains correct raw/canonical usage but terminal task/spend accounting can remain empty. The behavior from `MODEL-009` is adopted; its legacy-branch causal explanation is rejected. Current `completion_done::run` contains no usage resolution before ownership/linkage is cleared (`src/commands/completion_done.rs:29-294`). |
| 6 — `CORE-006` | `[FACT]` + `[CONTRADICTION]`; current policy, stale surfaces | **S2 Medium** / likely for old or template-driven configs / **high** | **high** | Handler discovery/planning, templates and live commands expose a larger catalog than unattended admission, which permits only Pi/Claude/Codex. Strict failure is a positive control; absent surface labels make discoverability look like worker eligibility. `MODEL-001`, `MODEL-003`, `MODEL-010`; `src/config.rs:2395-2433,3590-3710`; `src/dispatch/handler_for_model.rs:45-137`. |
| 7 — `CORE-007` | `[CONTRADICTION]`; partial | **S2 Medium** / possible / **medium** | **high** | Ordinary Pi task workers use direct JSON mode without the explicit embedded-extension/no-discovery handshake implemented by `wg pi-handler`; documentation/comments have conflated these topologies. `MODEL-002`, `MODEL-DRIFT-004`; `src/service/executor.rs:1729-1752`; `src/commands/pi_handler.rs:492-537`. |
| 8 — `CORE-008` | `[FACT]` + `[INFERENCE]`; current | **S2 Medium** / possible / **high** | **medium** | Persistence guarantees differ by store and platform: Unix graph mutation is serialized/replayed, but graph/registry locking is no-op off Unix and their bespoke rename paths omit the parent-directory sync used by the generic atomic helper. `ARCH-001`, `ARCH-006`; `src/parser.rs:76-157,275-414`; `src/service/registry.rs:285-324`; `src/atomic_file.rs:20-142`. |
| 9 — `CORE-009` | `[VERIFIED]` + `[FACT]`; current feedback gap | **S2 Medium** / observed / **high** | **medium** | Completion rejection tells a worker to repair but does not expose its bounded structured findings through the own-task capability. This combines expensive synchronous reviews with blind resubmission, amplifying `CORE-001`. `ORCH-015`; `src/completion_review.rs:32-56,83-118,351-387`; `src/worker_cli.rs:275-380`. |
| 10 — `CORE-010` | `[FACT]` + `[INFERENCE]`; current | **S2 Medium** / possible / **high** | **medium** | The autopoietic child-task limit counts a best-effort provenance log before the graph lock, treats read errors as zero, and ignores post-commit log failure. It is advisory under race/I/O failure despite being presented as a guardrail. `ARCH-008`; `src/commands/add.rs:430-463,835-865`; `src/provenance.rs:43-117`. |
| 11 — `CORE-011` | `[FACT]` + `[VERIFIED]`; current positive control with test seam | **S3 Low** / observed in agent-run tests / **high** | **medium** | Capability interception before graph discovery is a strong authority boundary, but integration subprocesses that do not scrub the complete worker environment exercise the wrong product mode. `ARCH-009`, `ORCH-009`; `src/main.rs:734-748`; affected test helpers cited in both leaves. |
| 12 — `CORE-012` | `[FACT]` + `[INFERENCE]`; current | **S2 Medium** / likely maintenance impact / **high structural, medium outcome** | **medium** | Oversized aggregate/read/config/CLI modules and composite, non-point-in-time show output increase synchronization cost and stale projection risk. This is not a defect by line count alone. `ARCH-002`, `ARCH-004`, `ARCH-RISK-004`; `src/graph.rs:689-1046`; `src/commands/show.rs:40-222,603-850`; `src/main.rs:702-4739`. |

### 3.2 Positive controls that should not be lost

**`[VERIFIED]`** The Unix graph spine prevented lost updates in the focused concurrent test, and lifecycle crash-replay/torn-frame tests passed. Source fsyncs lifecycle records before atomically replacing `graph.jsonl` (`ARCH-001`; `10-code-architecture.md` §7.4; `src/parser.rs:275-414`; `src/lifecycle.rs:1526-1694`). This does not extend to Windows or host-power-loss directory durability.

**`[VERIFIED]`** Staged publication and required-success readiness worked in the manual trace; scheduled and cron suites passed; Failed/Abandoned prerequisites do not authorize ordinary dependents (`ORCH-002`, `ORCH-012`; `11-orchestration-lifecycle.md` §7.3–7.5; `src/query.rs:306-517`). Manual claim is the scoped exception, not evidence that readiness itself is permissive.

**`[FACT]`** Spawn is deliberately transactional: worktree/registry/capability/observer state is prepared before the launch token, and pre-permit failure rolls back (`ORCH-004`, `ORCH-010`; `src/commands/spawn/execution.rs:1283-1438,1780-2160`). No crash-chaos or multi-daemon proof was run.

**`[VERIFIED]`** Modern completion resolvers, review valve, task projection, current v3 smoke canary, and one-lifecycle-path smoke passed in the orchestration leaf. Done re-verifies exact publication and current generation/manifest before applying `AttemptSucceeded` (`ORCH-005`; `src/commands/completion_done.rs:29-294`). The migration drift around this path does not negate the path's positive authorization design.

**`[VERIFIED]`** Strict route/reasoning tests, Pi stream de-duplication, watchdog tests, native local-stub processing, and two credential-free Pi smokes passed (`MODEL-005`, `MODEL-006`, `MODEL-007`; `12-model-execution-plane.md` §7.2, §7.5–7.6). Unsupported unattended routes fail before launch. No live provider/schema compatibility was exercised.

### 3.3 Shared root causes

**`[INFERENCE]`** `ROOT-A — migration without explicit authority retirement` explains completion flags/tests, pending statuses, v2/v3 stores, cycle naming, setup routes, starter profiles and handler-first deprecation drift. Compatibility material remains compiled or active, but most surfaces do not say whether it is current, compatibility-only, historical, or red-first.

**`[INFERENCE]`** `ROOT-B — policy copied instead of shared` explains ready versus claim, private versus public graph-directory resolution, handler catalog versus strict admission, and store-specific durability. Comments often call a function authoritative while another entry point implements only a subset.

**`[INFERENCE]`** `ROOT-C — late projection after terminal boundary` explains missing stored usage and composite show skew. Canonical data exists, but the terminal path clears ownership/linkage without making usage part of the same commit.

**`[INFERENCE]`** `ROOT-D — long work inside a globally serialized lane` explains daemon head-of-line blocking. The architecture correctly keeps graph critical sections bounded, yet external completion review is synchronous on the coordinator's IPC thread.

**`[INFERENCE]`** `ROOT-E — configuration identity is not surfaced as one resolved record` explains route/template/setup/profile confusion. Model, handler, endpoint, reasoning, executor compatibility, plugin topology and surface eligibility are resolved at different boundaries and only partly exposed as structured provenance.

### 3.4 Leaf finding disposition ledger

**`[FACT]`** This ledger proves that every major input finding is represented without copying its evidence appendix.

| Leaf IDs | Synthesis disposition | Destination |
|---|---|---|
| `ARCH-001` | adopt positive control with Unix/power-loss bounds | §3.2, `CORE-008` |
| `ARCH-002`, `ARCH-004` | adopt structural/maintenance risk; do not infer defect from size alone | `CORE-012` |
| `ARCH-003` | adopt de facto `after` authority and denormalized-backlink risk | `CORE-004`, §2.2 |
| `ARCH-005` | adopt migration conflict; reconcile S1/S2 as safety-preserving but release-critical | `CORE-002`, §4 |
| `ARCH-006` | adopt platform/durability fragmentation | `CORE-008` |
| `ARCH-007` | adopt layered-authority complexity and explicit profile fail-closed positive control | `CORE-004`, §2.2 |
| `ARCH-008` | adopt guardrail race/I/O inference | `CORE-010` |
| `ARCH-009` | adopt capability positive control and test isolation coupling | `CORE-011` |
| `ORCH-001`, `ORCH-002`, `ORCH-004`, `ORCH-005`, `ORCH-010`, `ORCH-012`, `ORCH-013` | adopt current positive controls with leaf execution limits | §2.3, §3.2 |
| `ORCH-003` | adopt verified defect/design ambiguity | `CORE-003` |
| `ORCH-006`, `ORCH-011`, `ORCH-016`, `ORCH-017` | adopt as one authority-retirement/synchronization finding | `CORE-002`, `CORE-004`, §4 |
| `ORCH-007` | adopt semantic/tick-order drift; do not claim unsafe reopen | §2.3, `CORE-DRIFT-004` |
| `ORCH-008` | preserve unresolved product decision on Abandoned retry | `CORE-DRIFT-005`, `CORE-REC-009` |
| `ORCH-009` | adopt ambient worker test seam | `CORE-011` |
| `ORCH-014` | adopt observation, narrow binary provenance, raise impact rank | `CORE-001` |
| `ORCH-015` | adopt worker feedback gap and link to queue amplification | `CORE-009` |
| `MODEL-001`, `MODEL-003`, `MODEL-010` | adopt strict-vs-broad capability/setup drift | `CORE-006`, §4 |
| `MODEL-002` | adopt topology divergence; impact remains uncertain | `CORE-007` |
| `MODEL-004` | adopt same-system explicit fallback as current, preserve old-doc conflict | `CORE-DRIFT-010` |
| `MODEL-005`, `MODEL-006`, `MODEL-007` | adopt positive controls, bounded to executed fixtures | §3.2 |
| `MODEL-008` | adopt zero/unavailable cost and current-day spend presentation as lower-severity companion | `CORE-RISK-010`, `CORE-REC-005` |
| `MODEL-009` | adopt observed accounting failure; **reject legacy `done.rs` causal attribution** | `CORE-005`, `CORE-DRIFT-007` |

## 4. Contradictions and drift

### 4.1 Reconciled contradiction table

| ID | Claim/evidence A | Claim/evidence B | Primary-source adjudication | Severity / confidence / sync impact | State |
|---|---|---|---|---|---|
| `CORE-DRIFT-001` | Clap exposes legacy Done flags (`src/cli.rs:527-557`). | Root main rejects every flag and uses publication-derived completion (`src/main.rs:1261-1274`). | Execution authority is `completion_done`; help is stale. Active tests/smokes must be classified rather than used as current success contracts. | S1 / high / critical | open |
| `CORE-DRIFT-002` | Manuals say terminal failure/abandonment unblock, wait resumes InProgress, direct Done and synthetic eval are current (`docs/manual/02-task-graph.md:65-125,230-280`; `docs/manual/04-coordination.md:145-215`). | Query requires successful Landed/Delivered completion; wait satisfaction projects Open; completion is candidate-bound (`src/query.rs:306-517`; `src/lifecycle.rs:821-835`; completion source cited above). | Current implementation wins for behavior; docs contain mixed current and historical sections. | S2 / high / high | open |
| `CORE-DRIFT-003` | Manual claim source says it is “not an operator waiver” and should share dispatcher disposition (`src/commands/claim.rs:18-21`). | It checks dependencies but not `paused` or `is_time_ready`; executed claims succeeded (`src/commands/claim.rs:25-78`; `src/query.rs:306-343`). | Verified implementation defect unless product explicitly elects a manual override. No source evidence supports an intentional silent waiver. | S2 / high / high | open |
| `CORE-DRIFT-004` | Cycle functions/logs say tasks were “reactivated.” | They record `ReopenRequested`; release to Open occurs after exact owner reconciliation, often a later tick (`src/graph.rs:3044-3567`; `src/commands/reopen.rs:236-328`). | Safety kernel is current; naming/output and immediate-Open tests are stale or premature. | S2 / high text, medium runtime / medium | open |
| `CORE-DRIFT-005` | Retry implementation accepts Abandoned (`src/commands/retry.rs:215-235`). | Help omits it and an integration test prohibits it (`11-orchestration-lifecycle.md` `ORCH-008`). | Product authority remains unknown; neither text age nor implementation alone resolves desired supersession semantics. | S3 / high conflict, unknown decision / medium | open |
| `CORE-DRIFT-006` | Handler resolver/discovery/templates recognize nex/native/OpenCode/external CLIs. | Strict unattended admission accepts only exact Pi/Claude/Codex (`src/config.rs:2395-2433,3590-3710`). | Apparent implementation contradiction resolves when capabilities are surface-scoped; terminology, templates and UI remain open debt. | S2 / high / high | apparent implementation non-issue; synchronization open |
| `CORE-DRIFT-007` | `MODEL-009` says normal reviewed Pi completion bypasses accounting through `done.rs:2509-2527 -> finalize::task_owned_done`, returning before token parsing. | Root Done and worker IPC call `completion_done::run`; repository call sites show legacy `done::run` only from user/finalize special paths and tests (`src/main.rs:1261-1274`; `src/commands/service/ipc.rs:885-919`; command S2). `completion_done` itself writes no usage (`src/commands/completion_done.rs:29-294`). | **Observed missing usage is retained; causal explanation is corrected.** The current v3 terminal projection omits usage. Legacy early return may be a separate compatibility defect but did not explain the recorded wrapper trace. | S2 / high / high | behavior open; causal contradiction resolved |
| `CORE-DRIFT-008` | Quickstart/comments describe `wg pi-handler`/embedded plugin as the WG-spawned worker path. | Normal task workers run direct `pi --mode json`; RPC handler alone adds `-e <embedded> -ne` (`src/service/executor.rs:1729-1752`; `src/commands/pi_handler.rs:492-537`). | These are distinct topologies. Either make JSON workers explicit/hermetic or narrow the claim. | S2 / high topology, medium impact / high | open |
| `CORE-DRIFT-009` | Handler-first lenient policy says leading providers warn/canonicalize while hard-error flag is false (`src/config.rs:2660-2690,2786-2874`). | Strict worker admission already rejects them (`src/config.rs:2395-2433`). | Different entry points currently implement different release phases. Strict worker safety wins; migration promise is not universal. | S2 / high / high | open |
| `CORE-DRIFT-010` | Agent/design text promises automatic `claude:haiku` fallback. | Current source/tests require explicit same-system fallback (`src/service/llm.rs:251-407,519-600`; `12-model-execution-plane.md` §7.2). | Source and executed focused tests are current; older cross-system fallback text is stale. | S2 / high / high | open docs |
| `CORE-DRIFT-011` | Generic atomic helper syncs the parent directory after rename (`src/atomic_file.rs:20-142`). | Graph/registry bespoke replacement does not; comments call graph replacement crash-safe (`src/parser.rs:297-357`; `src/service/registry.rs:177-247`). | Exact crash model is unspecified. Process-crash atomic visibility is supported; host/power-loss durability remains uncertain. | S2 / medium / medium | open uncertainty |
| `CORE-DRIFT-012` | Fresh initialization is graph-only. | Default compatibility executor is Pi. | Resolved apparent conflict: default model route is empty and no-flag init deliberately avoids route activation (`src/config.rs:5289-5340`; `src/commands/init.rs:87-123,247-266`; `MODEL-DRIFT-007`). | S4 / high / low | resolved/apparent non-issue |

### 4.2 What was not reconciled by fiat

**`[UNCERTAINTY]`** Whether native/OpenCode should become unattended handlers, whether Abandoned is reversible, whether ordinary Pi workers require `wg_*` extension tools, and whether graph crash safety includes power loss are product or architecture decisions. Current source establishes behavior, not desired policy.

**`[UNCERTAINTY]`** The installed-daemon head-of-line trace and pinned source mechanism align, but lack of binary commit identity prevents labeling the exact pinned binary behavior `[VERIFIED]`. The risk remains open with medium confidence rather than being downgraded to a documentation issue.

## 5. Risks and gaps

### 5.1 Cross-cutting risk ranking

| Rank / ID | Severity | Likelihood | Confidence | Synchronization impact | Impact and missing control |
|---:|---|---|---|---|---|
| 1 — `CORE-RISK-001` | **S1** | observed installed; possible snapshot | medium | critical | One slow reviewer can block all worker-control IPC and ticks; timed-out callers cannot know whether work later committed. Missing: bounded off-thread executor, durable request journal, idempotent status. |
| 2 — `CORE-RISK-002` | **S1** | likely | high | critical | Mixed completion generations can make help, active smokes, integration tests, legacy callers and modern workers validate incompatible protocols. Missing: authority classification and one generated lifecycle contract. |
| 3 — `CORE-RISK-003` | **S2** | observed | high | high | Manual claim can execute unpublished or future work without explicit override/audit. Missing: shared admission predicate. |
| 4 — `CORE-RISK-004` | **S2** | observed for Pi trace | high | high | Correct retained usage can disappear from terminal reporting and spend. Missing: accounting in the v3 Done transaction before linkage release. |
| 5 — `CORE-RISK-005` | **S2** | likely for legacy/template users | high | high | Discoverable/template routes fail only at strict worker admission; setup smoke still asserts retired routes. Missing: surface-qualified matrix generated from code. |
| 6 — `CORE-RISK-006` | **S2** | possible | medium | high | Normal Pi workers may not receive the version-locked extension/compatibility handshake promised for WG-spawned workers. Built-in bash limits blast radius. Missing: actual daemon-child argv/env/plugin assertion. |
| 7 — `CORE-RISK-007` | **S2** | possible | high code, medium operations | medium | Store/platform durability differs; non-Unix concurrent writers have no real graph/registry lock. Missing: durability matrix, Windows serialization, fault injection. |
| 8 — `CORE-RISK-008` | **S2** | observed | high | medium | Rejected workers cannot read actionable findings, causing blind resubmission and extra queue pressure. Missing: capability-scoped finding read. |
| 9 — `CORE-RISK-009` | **S2** | possible | high | medium | Best-effort provenance cannot enforce a hard child-task limit under race/I/O failure. Missing: graph-locked/create-new authoritative counter. |
| 10 — `CORE-RISK-010` | **S3** | likely | high | medium | Missing Pi cost is displayed as zero and spend groups historical tasks under today; this compounds missing terminal usage (`MODEL-008`). Missing: unavailable-vs-zero state and terminal timestamp grouping. |
| 11 — `CORE-RISK-011` | **S2** | possible | medium | medium | Cross-store operations span graph, registry, filesystem, process and Git without one transaction. Existing fences/gates reduce risk, but no crash-chaos covers every boundary. |
| 12 — `CORE-RISK-012` | **S2** | likely maintenance impact | medium | medium | Large duplicated models and composite reads make future changes easy to apply to one projection only. Missing: typed read model, module dependency policy, consistency contract. |

### 5.2 Verification and uncertainty gaps

**`[UNCERTAINTY]`** No genuine daemon-launched native worker trace exists because strict admission rejects native. The attended-native/local-SSE trace validates process and terminal parsing only through an explicit harness adapter (`12-model-execution-plane.md` §7.6).

**`[UNCERTAINTY]`** No external Pi/Claude/Codex credential, live provider schema, malformed real stream, rate limit, or plugin clean-install was tested. Fixture success establishes local mapping, not provider compatibility.

**`[UNCERTAINTY]`** No power-loss, disk-full, NFS, PID-reuse, simultaneous multi-daemon, or Windows writer test spans graph, registry, worktree, completion object, Git publication and Done commit.

**`[UNCERTAINTY]`** Full Cargo and full smoke were not run by any of these three leaves. The orchestration sample intentionally reported 54 failures and 3 ignored tests; failures cluster around migration drift but were not individually adjudicated. Scenario presence is not pass evidence.

**`[UNCERTAINTY]`** No production-duration watchdog silence or live two-tick cycle/owner release was executed. Short tests support the reducer and guard design only.

## 6. Recommendations

### 6.1 Implementation work

1. **`CORE-REC-001` — `[RECOMMENDATION]` (P0, service/completion; links `CORE-001`, `CORE-RISK-001`):** move review, smoke, Git and other long work off the accept/coordinator thread into a bounded task-scoped executor. Persist request ID and state before enqueue; serialize per task/attempt, not per graph; make replay return pending/completed without duplicate review. **Acceptance:** a reviewer sleeps past the client deadline while unrelated own-task Show/Log/Wait/Done and coordinator ticks remain within budget; replay commits exactly once; overload is visible and bounded.
2. **`CORE-REC-002` — `[RECOMMENDATION]` (P0, orchestration; links `CORE-003`):** implement one execution-admission function used by ready, claim, direct spawn and service spawn. Default claim rejects paused/future tasks. If override is required, expose an explicit reason-bearing flag and lifecycle event; do not allow dependency/publication evidence bypass by accident. **Acceptance:** real CLI flow proves default refusal and audited override.
3. **`CORE-REC-003` — `[RECOMMENDATION]` (P0, model/config; links `CORE-006`, `CORE-007`):** create a generated, surface-qualified capability record covering worker, attended chat/TUI, RPC, agency, discovery-only and deprecated status. Include route provenance, handler, inner provider/model, reasoning, credential owner and plugin topology. **Acceptance:** service validation, handler resolver, templates/profile listing and dry-run structured output agree.
4. **`CORE-REC-004` — `[RECOMMENDATION]` (P0, completion/accounting; links `CORE-005`, `CORE-DRIFT-007`):** resolve canonical usage before `completion_done::commit_done`, persist it in the same graph mutation, and retain a recovery path from immutable streams by attempt tuple after ownership clears. Do not patch only legacy `done.rs`. **Acceptance:** generated-wrapper Pi Land/Report/Explore tasks reach v3 reviewed Done with identical raw, canonical and stored usage; Failed and unavailable-cost cases are covered.
5. **`CORE-REC-005` — `[RECOMMENDATION]` (P1, accounting/UX):** represent cost as reported-zero versus unavailable/estimated and group spend by terminal timestamp. **Acceptance:** structured and human outputs distinguish all states and preserve cache fields/historical dates.
6. **`CORE-REC-008` — `[RECOMMENDATION]` (P1, persistence/platform; links `CORE-008`, `CORE-010`):** define one durability/serialization matrix and route graph/registry/config/provenance through reviewed primitives or explicit exceptions. Implement Windows interprocess locking; parent-sync graph/registry replacements if host-crash durability is promised; move guardrail counting inside authoritative serialization. **Acceptance:** Unix/Windows concurrent writers, injected rename/fsync failures, and concurrent child-limit tests pass.
7. **`CORE-REC-010` — `[RECOMMENDATION]` (P1, completion/worker control; links `CORE-009`):** expose only the current task/generation/candidate's bounded structured findings through the worker capability. **Acceptance:** exact rejection evidence is readable by the owner, cross-task/digest access is refused, and later candidate findings remain immutable.
8. **`CORE-REC-011` — `[RECOMMENDATION]` (P2, architecture; links `CORE-004`, `CORE-012`):** elect canonical `after` edges, derive/repair `before`, consolidate graph-directory resolution, and extract a versioned `TaskDetails` builder. **Acceptance:** contradictory backlinks are rejected/repaired, all binaries share one resolver, and field mapping has compile/test coverage.

### 6.2 Factual synchronization work

9. **`CORE-REC-006` — `[RECOMMENDATION]` (P0, completion/docs/tests/release; links `CORE-002`):** publish a current-authority matrix for operator Done, worker Done, user-board archive, finalization settlement, cycles and compatibility readers. Remove rejected flags from help and classify every lifecycle smoke as current, compatibility, historical-retired or red-first. **Acceptance:** current release scenarios all use candidate -> exact review -> publication -> Done and have no unexplained legacy failures.
10. **`CORE-REC-007` — `[RECOMMENDATION]` (P0, audit/doc owners; links `CORE-DRIFT-007`):** correct references that attribute current missing usage to the legacy `task_owned_done` early return. Preserve the executed symptom and point the fix/test at `completion_done`. **Acceptance:** root-cause record includes the normal operator and worker-IPC call graph and no longer implies `done.rs` is root Done authority.
11. **`CORE-REC-012` — `[RECOMMENDATION]` (P1, docs/model plane):** mark implemented designs shipped/superseded at paragraph granularity; replace automatic Claude fallback claims; distinguish Pi JSON worker from Pi RPC/attended plugin topology; align Pi-only setup smoke or restore routes. **Acceptance:** root README, docs README, quickstart, agent contract, profile templates and current smokes agree with source.
12. **`CORE-REC-013` — `[RECOMMENDATION]` (P1, test infrastructure):** centralize creation of clean operator subprocess environments and explicit worker environments. **Acceptance:** affected suites produce identical results inside/outside a real WG worker unless they intentionally opt into capability mode.

### 6.3 Human product and architecture decisions

13. **`CORE-REC-009` — `[RECOMMENDATION]` (P0 decision, product/lifecycle):** decide whether Abandoned is reversible and whether claim has an override role. Recommended: ordinary retry/claim preserve staged/superseded intent; restoration/override is explicit, reasoned, and starts a new fenced generation where appropriate. **Acceptance:** source, help, manual and tests encode one table.
14. **`CORE-REC-014` — `[RECOMMENDATION]` (P1 decision, model product):** decide whether nex/native and OpenCode are attended-only or future workers. **Acceptance:** either remove/hide worker cues and unusable starters, or add strict admission, credential, wrapper, stream and terminal-accounting tests.
15. **`CORE-REC-015` — `[RECOMMENDATION]` (P1 decision, Pi runtime):** decide whether ordinary Pi task workers require explicit embedded plugin loading. Prefer explicit `-e <embedded>` with compatibility handshake if JSON mode supports it; otherwise narrow “hermetic WG worker” documentation to RPC/attended topology. **Acceptance:** captured daemon child argv/env and a mismatch-negative test settle the claim.
16. **`CORE-REC-016` — `[RECOMMENDATION]` (P1 decision, persistence):** declare whether `graph.jsonl` is public authority or a compatibility projection of lifecycle/content-addressed authorities, and define the promised crash model. **Acceptance:** one ADR names authority for status, edges, completion and recovery plus process-crash versus host/power-loss guarantees.

## 7. Evidence appendix

### 7.1 Synthesis commands

**`[VERIFIED]`** On 2026-08-08 in `/home/bot/wg/.wg-worktrees/agent-19`, the source-equivalence command exited 0:

```bash
git diff --quiet \
  b0892ea7496fd2cc8f641417a3d8e33ca9add369..HEAD -- \
  src tests README.md docs/README.md AGENTS.md \
  Cargo.toml Cargo.lock rust-toolchain.toml
printf 'head=%s\n' "$(git rev-parse HEAD)"
```

```text
production_evidence_diff=none
head=b72cdb9f26afb60e5e77f211a6c8514a90598fa8
```

**`[VERIFIED]`** The normal-Done call-site check was executed against the source-equivalent checkout and is the basis for `CORE-DRIFT-007`:

```bash
rg -n 'commands::done::run|done::run\(' src
rg -n 'task_owned_done\(' src
rg -n 'DoneHandoff|Commands::Done' src/worker_cli.rs src/main.rs
```

**`[FACT]`** Bounded result: root `Commands::Done` and worker `DoneHandoff` route to `completion_done::run`; `super::done::run` occurs in `commands/user.rs`, a finalization settlement path, and tests; `task_owned_done` is called only from legacy `commands/done.rs`. This establishes reachability shape, not execution.

### 7.2 Primary source spot checks

| Topic checked directly | Primary source | Result/class |
|---|---|---|
| locked graph mutation and replay | `src/parser.rs:275-414`; `src/lifecycle.rs:1526-1694` | `[FACT]` ledger-before-projection, exclusive Unix mutation, nonblocking read snapshot |
| platform and atomic durability differences | `src/parser.rs:76-157,297-357`; `src/service/registry.rs:177-324`; `src/atomic_file.rs:20-142` | `[FACT]` non-Unix no-op locks; generic helper parent-sync difference |
| ready versus claim | `src/query.rs:306-517`; `src/commands/claim.rs:11-151` | `[FACT]` pause/time gates absent from claim |
| daemon IPC serialization | `src/commands/service/mod.rs:3330-3570`; `src/commands/service/ipc.rs:286-350,835-919` | `[FACT]` accepted connection and completion run inline |
| worker authority | `src/main.rs:734-748`; `src/worker_cli.rs:120-126,345-474` | `[FACT]` capability intercept precedes graph resolution |
| modern completion | `src/main.rs:1261-1342`; `src/commands/completion_submit.rs:187-487`; `src/commands/completion_done.rs:29-294` | `[FACT]` immutable candidate/review/publication authority; no usage projection in Done |
| retained legacy completion | `src/commands/done.rs:2490-2705`; `src/commands/finalize.rs:2385-2427,2541-2600`; repository call-site check S2 | `[FACT]` compatibility/special path, not normal root Done |
| strict versus broad routes | `src/config.rs:2395-2433,3590-3710`; `src/dispatch/handler_for_model.rs:45-137`; `src/dispatch/plan.rs:387-809` | `[FACT]` worker admission subset of catalog/planning |
| Pi process topologies | `src/service/executor.rs:1729-1752`; `src/commands/spawn/execution.rs:1308-1403,3430-3707`; `src/commands/pi_handler.rs:492-537,855-902` | `[FACT]` JSON worker differs from hermetic RPC handler |
| config layering | `src/config.rs:5925-6142`; `src/profile/named.rs:1-145`; `src/profile/project.rs:1-220` | `[FACT]` global/local merge plus selected project overlay and separate active profile |
| provenance guardrail | `src/commands/add.rs:430-463,835-865`; `src/provenance.rs:43-117` | `[FACT]` pre-lock count and ignored post-commit record error |

### 7.3 Dependency evidence links

**`[FACT]`** Detailed commands, output excerpts, test counts and limitations remain in the leaves and are not duplicated here:

- Architecture package/persistence/add-show-config evidence: [`10-code-architecture.md` §7](10-code-architecture.md#7-evidence-appendix).
- Orchestration lifecycle/manual claim/daemon/rejection/test/smoke evidence: [`11-orchestration-lifecycle.md` §7](11-orchestration-lifecycle.md#7-evidence-appendix).
- Model route/Pi/native/generated-wrapper/accounting evidence: [`12-model-execution-plane.md` §7](12-model-execution-plane.md#7-evidence-appendix).

### 7.4 Validation and limitations

**`[VERIFIED]`** Final artifact validation commands and results are recorded in the task log. Required checks are:

```bash
test -s docs/audit/2026-08-08-worksgood-system/20-core-runtime-synthesis.md
git diff --check
```

**`[UNCERTAINTY]`** This synthesis did not rerun inherited E1 product behavior. It statically spot-checked the highest-severity, disputed, and cross-boundary claims against source-equivalent primary evidence. It does not certify security, crash safety, provider correctness, cross-platform behavior, accounting completeness, or production readiness.
