# WorksGood system audit — comprehensive fractal synthesis (draft)

**Audit date:** 2026-08-08

**Status:** review draft; audit findings and proposals, not a product contract

**Primary audited snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`; later bundle artifacts state their own source-equivalence checks

**Evidence checked through:** 2026-08-08

**Change boundary:** this file only; no production source, tests, workflows, or pre-existing documentation were changed

**Normative audit method:** [`README.md`](README.md)

**Detailed contradiction authority:** [`30-contradiction-and-drift-register.md`](30-contradiction-and-drift-register.md)

**Synchronization plan:** [`31-documentation-sync-plan.md`](31-documentation-sync-plan.md)

> This draft is deliberately fractal. A reader may stop after §1, after any section abstract, after the findings and risk tables, or continue through direct evidence. Each major section repeats the same shape: **abstract → findings → risks → recommendations → deeper evidence**. It does not resolve contradictions by choosing the newest prose.

## Table of contents

1. [One-page executive summary](#1-one-page-executive-summary)
2. [How to read this audit](#2-how-to-read-this-audit)
3. [System identity and product boundary](#3-system-identity-and-product-boundary)
4. [Architecture and persistence](#4-architecture-and-persistence)
5. [Task and orchestration lifecycle](#5-task-and-orchestration-lifecycle)
6. [Model and execution plane](#6-model-and-execution-plane)
7. [Agency, evaluation, functions, chat, and evolvability](#7-agency-evaluation-functions-chat-and-evolvability)
8. [Federation, trust, review, remote execution, and Pilot](#8-federation-trust-review-remote-execution-and-pilot)
9. [Testing, CI, and release evidence](#9-testing-ci-and-release-evidence)
10. [Documentation and conceptual coherence](#10-documentation-and-conceptual-coherence)
11. [Operations, configuration, observability, and UX](#11-operations-configuration-observability-and-ux)
12. [Contradiction register summary](#12-contradiction-register-summary)
13. [Prioritized action and synchronization roadmap](#13-prioritized-action-and-synchronization-roadmap)
14. [Detailed evidence and artifact traceability](#14-detailed-evidence-and-artifact-traceability)

---

## 1. One-page executive summary

### Abstract

**`[INFERENCE — high confidence]`** WorksGood is best understood as a **local-first durable work-and-evidence system**. Its stable center is a file-backed task graph. Human-facing TUI/chat surfaces operate that graph; an optional daemon dispatches bounded workers; model handlers execute work; immutable candidate review plus publication derives successful completion; and agency, federation, content review, and remote execution add identity, learning, and cross-instance layers. This formulation is grounded in the durable `Task`, lifecycle, attempt, runtime-agent, and completion types rather than the broader “work OS” slogan (`src/graph.rs:379-529,689-1035`; `src/lifecycle.rs:66-213`; `src/service/registry.rs:37-90`; `src/commands/completion_done.rs:29-294`; synthesis in [`22`](22-product-docs-quality-synthesis.md#1-executive-abstract)).

**`[OBSERVED FACT]`** The strongest current path is safety-conscious:

```text
visible draft -> explicit publication -> derived readiness -> fenced attempt
-> durable ownership/workspace -> launch permit -> model process
-> immutable completion manifest -> exact FLIP then eval receipts
-> reviewed publication -> receipt-backed Done -> dependent readiness
```

Unix graph mutation is serialized and replayable; launch is withheld until ownership is durable; worker capability mode is selected before graph discovery; unattended model admission fails closed; and current Done is re-derived from immutable evidence rather than a worker exit code (`src/parser.rs:275-414`; `src/lifecycle.rs:605-615,1291-1507`; `src/main.rs:734-748,1261-1311`; `src/commands/spawn/execution.rs:1283-1438`; `src/commands/completion_submit.rs:208-482`; `src/commands/completion_done.rs:29-294`).

**`[INFERENCE — high confidence]`** The systemic weakness is not absence of controls. It is **incomplete authority migration and broken joins between controls**. Current and legacy lifecycle surfaces coexist; parser help can disagree with dispatch; review is strong at the exact candidate but weakly observable and disconnected from learning; federation crypto is stronger than its same-user custody and transport operations; remote execution has a secure CLI protocol but no coordinator-owned lifecycle; and documentation/test inventories do not reliably say what is current, selected, or production-validated.

### Findings: prioritized system conclusions

| Priority | Finding | State | Severity / confidence | Why it matters |
|---:|---|---|---|---|
| 1 | Completion review runs synchronously on the daemon coordinator/IPC lane. An inherited live trace saw unrelated requests time out while review was running; pinned source has the same inline shape. | current mechanism; installed-binary provenance bounded | **S1 / medium** | A slow external reviewer may stall scheduling and every worker-control request, not only one task (`11` §3 `ORCH-014`; `src/commands/service/mod.rs:3330-3570`; `src/commands/service/ipc.rs:286-350,835-919`). |
| 2 | Stateful array-valued worker IPC responses can fail serialization after inbox state advances. | current, verified in leaf | **S1 / high** | A worker can lose the only usable reply while the system has already marked messages read (`src/commands/service/ipc.rs:253-274,720-790`; `src/messages.rs:631-696`; `WGDR-049`). |
| 3 | Completion help and smoke policy describe a retired authority: `wg done` advertises legacy flags and an owned-smoke gate that current dispatch rejects/does not call. | open contradiction | **S1 / high** | Operators, agents, tests, and release claims validate incompatible completion protocols (`src/cli.rs:528-554`; `src/main.rs:1261-1274`; `tests/smoke/README.md:3-29`; `WGDR-001/002`). |
| 4 | Generic config editing erases comments, accepts ineffective unknown keys, and lint can call the file clean. | current, verified in leaf | **S1 / high** | The operator control plane can silently destroy intent and preserve typos (`src/commands/config_cmd.rs:3029-3096,3476-3676`; `WGDR-010/011`). |
| 5 | Federation root/recovery custody is same-process/same-user key loading, not the hostile-worker signer boundary claimed by the ADR; recovery is co-located and weakly time-bound. | partial security boundary | **S1 / high** | A same-UID shell-capable worker can plausibly collapse the principal/capability boundary (`src/identity/keys.rs:51-68,226-377`; `src/identity/sigchain.rs:493-515,884-925`; `WGDR-029/030`). |
| 6 | Federation inbox read/delete is unauthenticated and not an owned acknowledgement transaction. | partial transport | **S1 / high** | A reachable node can leak unsealed mail or delete/overwrite/fill an inbox even when message signatures preserve correctness (`src/identity/node.rs:408-443,551-572`; `src/identity/transport.rs:309-354`; `WGDR-032`). |
| 7 | Modern exact candidate review does not feed agency learning, and its attempt/cost lineage is thin. | current disconnect | **S1 learning; S2 observability / high** | Quality gating may be universal while assignment/evolution remain statistically empty (`src/completion_review.rs:83-121`; `src/agency/eval.rs:49-201`; `src/agency/evolver.rs:120-224`; [`23`](23-evaluation-evolvability-cutover.md#3-findings)). |
| 8 | Remote placement reaches planning but normal spawn rejects `RemoteRunner`; result accept and lease operations remain multi-command/manual. Pilot's real path is bootstrap, while dry-run uses a fixed worker. | partial/manual | **S1 / high** | “Dispatcher wired” and “turnkey family team” overstate the reachable lifecycle (`src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:339-348`; `src/commands/pilot_cmd.rs:43-50,1066-1215`; `WGDR-040/042`). |
| 9 | Setup, bare-launch, doctor, model-surface, spend, metrics, package, and platform narratives do not share one product contract. | current drift | **S1–S2 / high** | Operators can take wrong setup, readiness, budget, or support actions (`src/bin/worksgood.rs:6-16,124-144`; `src/commands/setup.rs:1389-1471`; `src/commands/doctor.rs:166-226`; `src/commands/spend.rs:27-67`; `src/metrics.rs:8-26`; `WGDR-008/009/012`). |
| 10 | Test and documentation presence materially overstate activated evidence: normal CI selects a small integration subset, and the authoritative smoke gate is disconnected. | current | **S1 / high** | Green CI or a manifest entry is not evidence that the claimed user/release path passed (`.github/workflows/ci.yml:68-201`; [`17`](17-testing-ci-quality.md#3-findings)). |

### Risks and balanced judgment

**`[OBSERVED FACT]`** Positive controls are substantial and should be preserved: Unix graph serialization/replay; generation/attempt/fence continuity; launch gating; immutable exact-candidate review; capability interception before graph discovery; self-certifying `wgid:` verification and root-locked sigchains; attenuating task-scoped capabilities; exact-byte review and lease-epoch fencing; conservative cleanup; agent-guide byte parity; Pi source-to-embedded regenerate-and-diff; and bounded formal checks.

**`[INFERENCE]`** These controls justify describing WorksGood as a serious, implemented system—not a paper architecture. They do **not** justify describing every plane as complete, production-validated, or one-command automated. The right maturity sentence is: **the local durable core and exact completion valve are shipped; several safety protocols are real but operationally partial; agency learning, remote lifecycle ownership, production custody, broad test activation, and product-contract synchronization remain incomplete.**

### Recommendations

**`[RECOMMENDATION]`** Do not begin with a bulk documentation rewrite. Run a P0 integrity program:

1. keep global lanes responsive: offload completion review and make IPC delivery idempotent;
2. elect one current lifecycle/completion/smoke contract and one admission predicate;
3. repair lossless/schema-aware config and time/scope-correct accounting;
4. isolate custody and authenticate inbox ownership/acknowledgement;
5. keep v3 completion as the sole lifecycle authority while adding review-attempt and exactly-once learning ledgers;
6. either implement coordinator-owned remote execution/Pilot or label them manual/bootstrap;
7. contain false high-impact claims, then build decision, claim, glossary, estate, and evidence registries.

The phased program is detailed in §13 and [`31-documentation-sync-plan.md`](31-documentation-sync-plan.md).

### Deeper audit artifacts

[`20-core-runtime-synthesis.md`](20-core-runtime-synthesis.md), [`21-agency-federation-safety-synthesis.md`](21-agency-federation-safety-synthesis.md), [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md), [`23-evaluation-evolvability-cutover.md`](23-evaluation-evolvability-cutover.md), and [`30-contradiction-and-drift-register.md`](30-contradiction-and-drift-register.md).

---

## 2. How to read this audit

### Abstract

This document separates **what the repository contains**, **what an audit actually executed**, **what follows by inference**, and **what is proposed**. It also distinguishes implementation maturity from normative status. Those distinctions are required because source, tests, help, ADRs, reports, and current guidance often answer different questions.

### Findings and method

Statement labels used throughout:

- **`[OBSERVED FACT]`** — directly present in repository bytes or structured state.
- **`[VERIFIED BEHAVIOR]`** — an exact command/test observed behavior. When inherited, the leaf artifact is named.
- **`[INFERENCE]`** — a reasoned consequence; confidence and material bounds are stated.
- **`[DOC-CLAIM]`** — descriptive or normative prose, not runtime proof.
- **`[RECOMMENDATION]`** — proposed work; never a shipped fact.
- **`[UNCERTAINTY]`** — evidence is absent, environment-bounded, or desired policy is unelected.

Maturity labels:

| Label | Meaning in this draft |
|---|---|
| **current/shipped** | reachable current implementation at the audited snapshot; not necessarily continuously gated or production-qualified |
| **partial/stubbed/manual** | types or CLI exist, but an enforcement seam, independent actor, owned lifecycle, or real-model/host path is absent |
| **deferred** | explicitly left for a later wave or not present |
| **historical/superseded** | correct for an older revision or architecture, not current authority |
| **proposed/decision-required** | desired behavior is not ratified, or competing authorities remain |

Evidence precedence is scoped, not global: exact executed candidate behavior controls that environment; reachable source/schema controls current encoded behavior; accepted applicable decisions control desired policy; generated views control only declared derivative fields; immutable reports control their observed revision. A test file proves nothing about selection, and `AGENTS.md` alone never proves product behavior. See charter `README.md:229-327` and roadmap [`31` §2.2](31-documentation-sync-plan.md#22-target-authority-model-a-two-axis-join-not-newest-file-wins).

### Risks

The audit uses S1 high, S2 medium, S3 low, and S4 informational. Severity describes impact if the condition matters; it is not likelihood. This draft preserves medium/low confidence where binary provenance, platform, provider, or full-flow execution was unavailable.

### Recommendations

Every proposed synchronization must identify whether it is a factual edit (**F**), human decision (**D**), implementation (**I**), structural documentation change (**S**), or verification (**V**). A factual edit may narrow a claim; it may not decide product semantics. A test cannot ratify an ADR.

### Deeper audit artifacts

- Audit contract and inventory: [`README.md`](README.md)
- Deduplicated 49-item register: [`30`](30-contradiction-and-drift-register.md)
- Six-phase synchronization program: [`31`](31-documentation-sync-plan.md)

---

## 3. System identity and product boundary

### Abstract

**`[INFERENCE — high confidence]`** WorksGood's stable public center is durable, answerable work—not a daemon, a model wrapper, or a single “agent.” A WG instance contains a work graph plus lifecycle, completion, registry, configuration, agency, function, and federation sidecars. The product then offers attended operation and optional unattended/federated execution around that center.

### Findings

1. **`[OBSERVED FACT — current/shipped]`** The execution hierarchy is real and typed: **task → generation → attempt → worker process**, while **candidate → reviews → publication → Done** is a separate completion hierarchy (`src/graph.rs:379-529,689-1035`; `src/lifecycle.rs:66-86,181-213`; `src/service/registry.rs:37-90`; `src/commands/completion_done.rs:32-132`).
2. **`[OBSERVED FACT — current]`** The current actor contract names **dispatcher**, **chat agent**, and **worker agent**. “Coordinator” remains in daemon/config/legacy contexts; “agent” also names an agency composition, runtime process, and federated principal (`src/text/agent_guide.md:44-68`; `src/agency/types.rs:500-535`; `src/identity/envelope.rs:41-78`).
3. **`[OBSERVED FACT — current]`** Authentication, trust, permission, content acceptance, candidate acceptance, and completion are separate gates. A signature does not prove safe content; a trust assertion does not grant graph-write; a model route does not identify a principal (`src/trust.rs:29-53,79-125`; `src/identity/custody.rs:294-486`; `src/review/mod.rs:326-461`; `src/evaluation/mod.rs:82-217`).
4. **`[CONTRADICTION — open]`** The root product narrative, bare launcher, setup, and handler implementation do not define one scope for “Pi is the model plane.” Existing-graph bare `worksgood` is setup-neutral, setup exposes Pi, and handler code retains multiple execution kinds (`README.md:87-165`; `src/bin/worksgood.rs:6-16,124-144`; `src/commands/setup.rs:72-150`; `src/dispatch/plan.rs:57-109`).
5. **`[OBSERVED FACT — partial presentation]`** Current public schemas and documents do not provide one versioned conceptual contract covering task, lifecycle, identity namespaces, trust assertions, support level, and maturity (`19` `CONCEPT-010`; [`22` §2.4–2.6](22-product-docs-quality-synthesis.md#24-vocabulary-alignment-across-product-docs-cli-and-operations)).

### Risks

- **S2/high confidence:** “agent,” “provider,” “evaluation,” “review,” “identity,” “trust,” “run,” and “publish” can cause cross-plane authorization or accounting mistakes (`WGDR-T01`–`WGDR-T12`).
- **S2/high confidence:** describing the graph as all instance state can lead backup/repair tooling to omit lifecycle, completion, registry, agency, or federation state.
- **S1 operator impact/high confidence:** unscoped onboarding language can cause wrong credential/plugin/service actions (`WGDR-008/009`).

### Recommendations

1. **`[RECOMMENDATION — D/S]`** Ratify: “WorksGood is a local-first durable work-and-evidence system”; keep “work OS” as positioning.
2. **`[RECOMMENDATION — D/S]`** Approve a namespaced glossary: agency agent, runtime worker/process, federated principal, model provider, compute provider, inbound review, completion review, candidate evaluation, agency performance evaluation.
3. **`[RECOMMENDATION — S/V]`** Generate object/lifecycle/authority tables from types plus reachable dispatch, not from slogans or Clap alone.
4. **`[RECOMMENDATION — D]`** Decide whether Pi is sole attended, sole recommended, or sole overall; publish surface-qualified support.

### Deeper audit artifacts

- [`19-conceptual-model-and-vocabulary.md`](19-conceptual-model-and-vocabulary.md)
- [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md)
- Register terms [`30` §4](30-contradiction-and-drift-register.md#4-terminology-and-authority-collision-subregister)

---

## 4. Architecture and persistence

### Abstract

WorksGood has a strong Unix serialization/recovery spine and clear immutable completion objects, but state authority is distributed across multiple stores and compatibility projections. The main architecture risk is duplicated semantic authority, amplified by large aggregate/read/config modules and uneven persistence guarantees.

### Findings

1. **`[VERIFIED BEHAVIOR — shipped, bounded]`** Focused tests observed no lost Unix graph update and passed lifecycle crash-replay/torn-frame cases. Source appends/fsyncs lifecycle records before graph projection replacement (`src/parser.rs:275-414`; `src/lifecycle.rs:1526-1694`; [`10` §7.4](10-code-architecture.md#74-a3--build-and-focused-persistence-behavior)). This does not prove Windows, NFS, disk-full, or host-power-loss safety.
2. **`[OBSERVED FACT — current]`** `Task.after` drives readiness/reverse derivation while `Task.before` is also persisted. Authority is de facto rather than type-enforced (`src/query.rs:306-517`; `10` `ARCH-003`).
3. **`[OBSERVED FACT — current/compatibility]`** Lifecycle ledger, graph status projection, generation/fence fields, v3 completion objects, legacy `done/finalize` stores, registry entries, streams, and Git publication form one cross-store protocol, not one database transaction (`src/lifecycle.rs:605-1507`; `src/commands/completion_done.rs:29-294`; `src/commands/done.rs:2490-2705`; `src/service/registry.rs:37-90`).
4. **`[OBSERVED FACT — platform gap]`** Graph/registry locking is no-op off Unix, and their bespoke replacement paths do not use the parent-directory sync performed by the generic atomic helper (`src/parser.rs:76-157,297-357`; `src/service/registry.rs:177-324`; `src/atomic_file.rs:20-142`). Exact power-loss semantics are unelected.
5. **`[OBSERVED FACT — current]`** The child-task guardrail counts best-effort provenance before graph locking, treats read failure as zero, and can ignore post-commit record failure (`src/commands/add.rs:430-463,835-865`; `src/provenance.rs:43-117`). It is advisory under race/I/O failure.
6. **`[INFERENCE — medium/high confidence]`** Large core aggregate, read-model, CLI, and config modules increase the chance that one projection changes without its peers. Size is maintainability evidence, not by itself a correctness defect (`src/graph.rs:689-1046`; `src/commands/show.rs:40-222,603-850`; `src/main.rs:702-4739`).

### Risks

- **S2/high:** non-Unix concurrent writers lack the observed Unix serialization guarantee.
- **S2/medium:** cross-store crash points can leave graph, registry, workspace, process, completion, and Git views inconsistent despite current fences.
- **S2/high:** duplicate edges/status/completion authorities invite fixes in the wrong representation.
- **S2/high:** best-effort provenance cannot enforce a hard autopoietic child limit.

### Recommendations

1. **`[RECOMMENDATION — D/I/V]`** Declare the canonical status, edge, completion, and crash model. Make `before` derived/repairable and isolate compatibility writers.
2. **`[RECOMMENDATION — I/V]`** Define a storage durability matrix; implement Windows interprocess locking; add parent sync if host-crash durability is promised; fault-test every cross-store boundary.
3. **`[RECOMMENDATION — I]`** Move authoritative child-limit counting into graph serialization or a create-new transactional primitive.
4. **`[RECOMMENDATION — I/S]`** Extract a versioned `TaskDetails` read model and shared graph-directory resolver.

### Deeper audit artifacts

- [`10-code-architecture.md`](10-code-architecture.md)
- [`20-core-runtime-synthesis.md` §2–3](20-core-runtime-synthesis.md#2-scope-and-unified-map)
- Register `WGDR-007`, `WGDR-U01`, roadmap `DEC-12`

---

## 5. Task and orchestration lifecycle

### Abstract

The current lifecycle kernel is substantially fail-closed: drafts are explicitly published, readiness requires successful dependencies, attempts are fenced, launch is gated, and Done requires exact reviewed publication. The highest operational risk is long work inside the global daemon lane; the highest coherence risk is coexistence of v3 authority with legacy help, manuals, tests, and special paths.

### Findings

1. **`[OBSERVED FACT — current/shipped]`** `wg add` creates a visible paused task; publication unpauses a selected region; readiness checks status, pause, time, and dependency disposition (`src/commands/add.rs:355,614-617,847-975`; `src/commands/resume.rs:164-350`; `src/query.rs:306-517`).
2. **`[VERIFIED BEHAVIOR — current defect]`** Manual `wg claim` admitted an unpublished paused task and a future task that `wg ready` excluded. Source checks dependency disposition but omits pause/time readiness (`src/commands/claim.rs:11-151`; `src/query.rs:306-343`; [`11` Trace B](11-orchestration-lifecycle.md#trace-b--manual-admission-bypass)).
3. **`[OBSERVED FACT — shipped positive control]`** Spawn prepares workspace, registry, fenced attempt, capability, and observer state before publishing the launch token; pre-permit failure rolls back (`src/commands/service/coordinator.rs:2366-2702`; `src/commands/spawn/execution.rs:1283-1438,1780-2160`).
4. **`[OBSERVED FACT — shipped positive control]`** Current completion selects an immutable candidate, runs exact FLIP then eval, rechecks dependency outputs and publication, and commits `AttemptSucceeded` only under current generation/manifest checks (`src/commands/completion_submit.rs:187-487`; `src/commands/completion_done.rs:29-294`).
5. **`[VERIFIED BEHAVIOR — bounded]`** An installed-daemon trace saw one slow completion review block unrelated IPC beyond 30 seconds. The installed binary lacked commit identity, so exact snapshot behavior is not labeled verified; pinned source accepts and executes completion inline on the coordinator thread (`src/commands/service/mod.rs:3330-3570`; `src/commands/service/ipc.rs:286-350,835-919`; `CORE-001`).
6. **`[OBSERVED FACT — current UX gap]`** Rejection findings are stored, but the task owner's capability does not expose the bounded structured findings, encouraging blind resubmission (`src/completion_review.rs:32-56,83-118,351-387`; `src/worker_cli.rs:275-380`).
7. **`[CONTRADICTION — open]`** Help advertises `--converged`, smoke/bypass, and merge flags; main and worker dispatch reject them. Smoke policy claims owned scenarios gate Done, but current completion contains no manifest call (`src/cli.rs:528-554`; `src/main.rs:1261-1274`; `tests/smoke/README.md:3-29`; `WGDR-001/002`).
8. **`[CONTRADICTION/UNCERTAINTY]`** Cycles record a safe `ReopenRequested` hold while outputs say “reactivated”; Abandoned retry is accepted in source but omitted/prohibited elsewhere; exact special legacy path reachability remains incomplete (`src/graph.rs:3044-3567`; `src/commands/reopen.rs:236-328`; `src/commands/retry.rs:215-235`; `WGDR-005/006/U01`).

### Risks

- **S1/medium:** daemon-wide head-of-line blocking plus ambiguous timeout outcome.
- **S1/high:** completion release claims and tests can validate incompatible protocols.
- **S2/high:** unpublished/scheduled tasks execute through silent manual-claim bypass.
- **S2/high:** blind review repair multiplies expensive synchronous calls.
- **S2/medium:** cycle/retry language can cause unsafe operator assumptions even where kernel behavior is safe.

### Recommendations

1. **`[RECOMMENDATION — I/V, P0]`** Persist and enqueue completion work into a bounded idempotent per-task executor; keep unrelated IPC/ticks responsive; replay request IDs exactly once.
2. **`[RECOMMENDATION — D/I/V, P0]`** Elect one execution-admission predicate for ready, claim, direct spawn, and service. Any override must be explicit, reasoned, and auditable.
3. **`[RECOMMENDATION — D/I/F, P0]`** Decide current Done flags, owned-smoke placement, cycle support, Abandoned restoration, and legacy reachability as one lifecycle table.
4. **`[RECOMMENDATION — I]`** Expose exact current candidate findings to the owning capability and deny cross-task/digest access.

### Deeper audit artifacts

- [`11-orchestration-lifecycle.md`](11-orchestration-lifecycle.md)
- [`20-core-runtime-synthesis.md`](20-core-runtime-synthesis.md)
- Register [`30` §3.1](30-contradiction-and-drift-register.md#31-core-lifecycle-completion-and-verification)

---

## 6. Model and execution plane

### Abstract

WorksGood's model plane correctly separates route resolution, surface eligibility, process execution, streams, and accounting—but public/catalog vocabulary often collapses them. Strict unattended admission is a positive control. Pi stream de-duplication is implemented; normal-worker plugin topology and terminal/reviewer accounting remain incomplete or disputed.

### Findings

1. **`[OBSERVED FACT — current]`** Broad resolver/discovery supports more handlers than unattended admission. Service workers accept exact Pi/Claude/Codex routes; attended/discovery/internal execution surfaces include additional kinds (`src/dispatch/handler_for_model.rs:45-137`; `src/dispatch/plan.rs:387-809`; `src/config.rs:2395-2433,3590-3710`). This is a legitimate surface split, but “available” is underspecified.
2. **`[OBSERVED FACT — current]`** Effective worker route and reasoning are explicitly propagated, and unsupported routes fail before launch (`src/config.rs:3590-3710`; [`12` `MODEL-005`](12-model-execution-plane.md#model-005--explicit-routereasoning-propagation-and-fail-closed-worker-admission-are-strong-controls)).
3. **`[CONTRADICTION — partial]`** Ordinary Pi workers run `pi --mode json`; the distinct RPC handler runs `pi --mode rpc -e <embedded> -ne` with the explicit compatibility boundary. Documentation/comments sometimes call the latter the WG-spawned worker path (`src/service/executor.rs:1729-1752`; `src/commands/spawn/execution.rs:1308-1403`; `src/commands/pi_handler.rs:492-537,855-902`; `WGDR-015`).
4. **`[VERIFIED BEHAVIOR — shipped, fixture-bounded]`** Pi usage mapping sums `turn_end` once and avoids repeated snapshot double-counting; watchdog continuation is evidence-based and does not become completion authority (`src/stream_event.rs:410-690`; [`12` §7.2, §7.5](12-model-execution-plane.md#72-executed-focused-tests)).
5. **`[VERIFIED BEHAVIOR — current gap at audited snapshot]`** A generated Pi wrapper reached reviewed Done with non-zero raw/canonical usage while stored `task.token_usage` remained null and spend showed zero. Direct synthesis checking corrected the causal story: normal Done calls `completion_done::run`, whose v3 commit omitted usage; the legacy early return was not the recorded path (`src/main.rs:1261-1274`; `src/commands/service/ipc.rs:885-919`; `src/commands/completion_done.rs:29-294`; `CORE-DRIFT-007`).
6. **`[OBSERVED FACT — current gap]`** Exact reviewer calls return `token_usage`, but the adapter extracts only text and compact receipts have no usage (`src/service/llm.rs:22-29,311-319`; `src/completion_review_model.rs:58-88`; `src/completion_review.rs:83-95`).
7. **`[CONTRADICTION — current policy drift]`** Provider-leading routes are lenient/deprecated at some entry points but strictly rejected for worker admission. Older fallback text promises Claude while current agency source requires an explicit same-system fallback (`src/config.rs:2395-2433,2660-2690,2786-2874`; `src/service/llm.rs:251-407,519-600`; `WGDR-016/017`).
8. **`[OBSERVED FACT — current UX gap]`** Missing cost may appear as zero, and spend groups usage under invocation day rather than terminal date (`src/commands/spend.rs:27-67`; `WGDR-012`).

### Risks

- **S2/high:** discoverable/template routes are mistaken for unattended-worker-ready routes.
- **S2/medium:** normal Pi workers may not receive the promised invocation-scoped extension/compatibility boundary.
- **S2/high at snapshot:** source and reviewer cost can disappear or be misdated, undermining budgeting and model comparison.
- **S2/high:** route migration/fallback behavior depends on entry point.

### Recommendations

1. **`[RECOMMENDATION — D/S/I]`** Publish one generated surface-qualified capability record: handler, provider/model, reasoning, credential owner, plugin topology, worker/attended/RPC/agency eligibility, deprecation state.
2. **`[RECOMMENDATION — I/V]`** Resolve canonical usage before terminal commit and retain attempt-keyed recovery; persist reviewer usage separately from source usage.
3. **`[RECOMMENDATION — D/I/V]`** Decide whether JSON workers require explicit embedded extension loading; prove captured daemon child argv/env and mismatch failure.
4. **`[RECOMMENDATION — I/F]`** Distinguish reported zero, unavailable, and estimated cost; use terminal timestamps.

### Deeper audit artifacts

- [`12-model-execution-plane.md`](12-model-execution-plane.md)
- [`20-core-runtime-synthesis.md`](20-core-runtime-synthesis.md)
- Evaluation/accounting detail: [`23`](23-evaluation-evolvability-cutover.md)

---

## 7. Agency, evaluation, functions, chat, and evolvability

### Abstract

Agency persona composition, current completion review, candidate evaluation, assignment, and legacy performance learning are distinct planes. Exact-candidate completion review is strong, but the cutover removed ordinary review-task hazards without replacing attempt visibility, cost lineage, or the learning join. Chat/human authority has useful explicit checks, while local memory and function history have weaker provenance and schema coherence.

### Findings

1. **`[OBSERVED FACT — current]`** Agency `Agent.id` hashes role/tradeoff IDs, not a cryptographic principal. Several prompt-visible role/tradeoff/component/outcome fields are omitted, and edit semantics can delete/update old identities contrary to immutable-behavior prose (`src/agency/hash.rs:15-67`; `src/commands/role.rs:224-273`; `WGDR-019`).
2. **`[OBSERVED FACT — partial]`** `auto_assign` is surfaced, but no coordinator caller for the retained LLM assigner was found. Manual `wg assign --auto` performs deterministic history ranking; direct dispatch commonly records runtime ownership without an agency composition (`src/commands/assign.rs:205-393`; `src/commands/service/assignment.rs:397-477`; `WGDR-020`).
3. **`[VERIFIED BEHAVIOR — shipped positive control]`** Current v3 review preserves exact FLIP-before-eval ordering, immutable candidate/requirements/output binding, distinct reject/incomplete/unavailable outcomes, and publication-derived Done. Targeted `completion_review_valve` passed 9/9 and agency pipeline 34/34 active tests in audit 23 (`src/completion_review.rs:83-121,182-299`; [`23` §1](23-evaluation-evolvability-cutover.md#1-executive-abstract)).
4. **`[OBSERVED FACT — current gap]`** Compact receipts omit reviewer attempts, timing, reasoning, usage/cost, response digest, retry lineage, source attempt/fence, and source agency composition. `wg show` exposes object references, not a complete review history (`src/completion_review.rs:83-95`; `src/commands/show.rs:937-953,1201-1290`).
5. **`[VERIFIED BEHAVIOR — current disconnect]`** Audit 23 found 12/12 Done tasks with exact review pairs, zero normal `evaluation_records`, zero agency evaluation files, and no live agency task composition. Modern completion/evaluation has no call to `record_evaluation[_with_inference]`; evolver reads the legacy store (`src/agency/eval.rs:49-201`; `src/agency/evolver.rs:120-224`).
6. **`[OBSERVED FACT — current gap]`** Assigner history can record placeholder `0.5`; no later caller replaces it with outcome quality (`src/commands/assign.rs:107-164`; `WGDR-023`).
7. **`[OBSERVED FACT — partial]`** Generative function schema exists, but apply consumes a pre-existing planner task or falls back; normal apply tracking rows do not match the adaptive `RunSummary` loader (`src/commands/func_apply.rs:435-451,612-726`; `src/function.rs:298-346`; `WGDR-024/025`).
8. **`[OBSERVED FACT — current]`** Bound `session-summary.md` enters a future prompt as “own memory” without content CID, model/author/time provenance, review verdict, or spotlight delimiter (`src/service/executor.rs:1342-1386`). Context scope controls quantity, not trust (`src/context_scope.rs:1-70`).
9. **`[OBSERVED FACT — positive/partial]`** Attended and confirmed human reply edges carry explicit authority checks, but onboarding writes agent, board, then binding without a transaction (`src/text/attended_chat_contract.md:1-18`; `src/commands/agency_human.rs:126-214`; `WGDR-026`).

### Risks

- **S1/high:** universal quality review can coexist with statistically empty agency learning.
- **S2/high:** reviewer failures, retries, latency, superseded trajectories, and spend are not queryable as first-class attempts.
- **S2/high:** runtime `agent-N` cannot reconstruct an agency composition for credit.
- **S2/medium:** local summary/function memory can become unlabelled prompt authority and later produce validly signed output.
- **S2/high:** identity hashing/mutability and automatic-assignment claims can mislead users about adaptation.

### Recommendations

1. **`[RECOMMENDATION — architecture default]`** Keep v3 completion as the only source-lifecycle authority. Do not restore schedulable `.flip-*`/`.evaluate-*` tasks.
2. **`[RECOMMENDATION — I/V, P0]`** Add append-only `ReviewRun`/`ReviewAttempt` records plus non-authoritative virtual projections and `wg reviews`/review spend/TUI views.
3. **`[RECOMMENDATION — D/I/V, P0]`** Add an exactly-once learning projector keyed by terminal generation, candidate trajectory, source composition, and policy version. Infrastructure failure must affect reviewer reliability, not source quality.
4. **`[RECOMMENDATION — I]`** Create assignment receipts or mark direct dispatch `uncomposed`; replace placeholder reward with delayed idempotent outcome attribution.
5. **`[RECOMMENDATION — I/S]`** Content-address and provenance-tag local memory; spotlight/review cross-session or externally derived memory; version/unify function history schemas.
6. **`[RECOMMENDATION — D]`** Decide performance meaning, reject-trajectory weight, reviewer identity, legacy eligibility, and actual auto-assignment product before wiring evolution.

### Deeper audit artifacts

- [`13-agency-evaluation-chat.md`](13-agency-evaluation-chat.md)
- [`23-evaluation-evolvability-cutover.md`](23-evaluation-evolvability-cutover.md)
- [`21-agency-federation-safety-synthesis.md`](21-agency-federation-safety-synthesis.md)

---

## 8. Federation, trust, review, remote execution, and Pilot

### Abstract

WorksGood has real cryptographic and enforcement machinery: self-certifying principals, root-locked sigchains, signed/sealed messages, attenuating capabilities, exact-byte review, task-scoped remote grants, disjoint verification hooks, and lease fencing. Its largest risks are at operational joins: same-user custody, recovery, unauthenticated inbox operations, split incident response, best-effort review audit, absent independent quorum/human release, manually choreographed remote execution, and overstated Pilot readiness.

### Findings

1. **`[OBSERVED FACT — shipped positive control]`** A `wgid:` is rooted in a genesis Ed25519 public key; sigchain verification tracks authorized keys and root-locked operations. Envelope signatures, wrap-set encryption, and capability attenuation/expiry are implemented (`src/identity/keys.rs:135-184`; `src/identity/sigchain.rs:680-886`; `src/identity/envelope.rs:385-418,685-761`; `src/identity/custody.rs:294-486,681-805`).
2. **`[OBSERVED FACT — partial/S1]`** `Custodian::sign_digest` loads a seed in-process from the same user's keystore; without a KEK the value may be plaintext and warning is opt-in. The API minimizes returned key material but does not authenticate requester/purpose or isolate worker UID (`src/identity/keys.rs:51-68,223-377`; `WGDR-029`).
3. **`[OBSERVED FACT — partial/S1]`** Recovery key is commonly co-located; time window is optional and verification trusts signer-asserted `recovery_at`; guardian proof lacks a current-head challenge/one-use binding (`src/commands/identity_cmd.rs:253-322`; `src/identity/sigchain.rs:493-515,618-657,884-925`; `WGDR-030`).
4. **`[OBSERVED FACT — partial/S1]`** Node inbox read/delete lacks recipient authentication, insertion can overwrite, and polling lacks an owned automatic ack transaction (`src/identity/node.rs:408-443,551-572`; `src/identity/transport.rs:309-354,480-496`; `WGDR-032`). Signatures protect authenticity, not availability/confidentiality of unsealed bytes.
5. **`[OBSERVED FACT — partial]`** `/version` is unsigned; many signed structures do not enforce version/algorithm floor. Freshness/equivocation protect against some rollback after prior observation, not global currentness (`src/identity/node.rs:343-374`; `src/identity/transport.rs:583-607`; `14` `FED-007/008`).
6. **`[OBSERVED FACT — partial wording]`** The cryptographic ACL is the recipient key-wrap set, not intrinsically the separately supplied `to` field. Current CLI correlates them; the library does not enforce equality (`src/identity/envelope.rs:385-418,685-761`; `WGDR-033`).
7. **`[OBSERVED FACT — current split]`** Author and compute-provider trust are separate local opinions on one enum/order. Provider trust can only lower author trust at the canonical resolver. Capability, content verdict, and candidate verdict remain separate (`src/graph.rs:2530-2541`; `src/trust.rs:85-124`).
8. **`[OBSERVED FACT — partial review]`** Four inbound classes have hooks and named high-level seams can be fail-closed, but entry-point policy differs. Review audit is an unsigned hash-linked JSONL file; live callers may ignore record failures; deterministic reviewer count is ignored; Pass 3/human Pass 4 remain stubs/deferred (`src/review/verdict.rs:53-190`; `src/review/pass2_review.rs:86-99`; `src/review/depth.rs:29-40,97-104`; `WGDR-037/038/039`).
9. **`[OBSERVED FACT — shipped protocol, partial lifecycle]`** WG-Exec grants two task-scoped capabilities, a sealed task slice, and lease epoch; accept authenticates, scopes, reviews, optionally reruns, then fences. But planner-selected `RemoteRunner` is rejected by normal spawn; offer/claim/grant/run/renew/accept/sweep are separate CLI operations (`src/commands/exec_fed_cmd.rs:544-650,700-979`; `src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:339-348`).
10. **`[OBSERVED FACT — cross-store gap]`** Accept commits epoch before provider renewal, graph accounting, and optional finalization; later failure can strand a replay-blocked result (`src/commands/exec_fed_cmd.rs:951-979`; `WGDR-041`).
11. **`[OBSERVED FACT — partial provenance]`** Signed grant names a model, but `--worker-cmd`/environment may replace it and signed result omits actual backend/model route (`src/providers/worker.rs:63-91`; `src/providers/mod.rs:553-590`; `XAUTH-008`).
12. **`[CONTRADICTION — current]`** Pilot dry-run is deterministic and real `up` bootstraps one host, records no live check, and only checks a key path exists. This is not evidence of a credentialed, two-host, coordinator-owned live family team (`src/commands/pilot_cmd.rs:43-50,1066-1215`; `WGDR-042`).
13. **`[GOVERNANCE UNCERTAINTY]`** Federation ADRs remained Proposed while multiple waves and compatibility `0.4.0` shipped. Tests establish behavior, not ratification (`docs/ADR-fed-000-acceptance-brief.md:4-6,48`; `src/identity/mod.rs:141`; `WGDR-028`).

### Risks

- **S1/high:** same-UID shell authority can collapse root/recovery custody.
- **S1/high:** host compromise/backdating/replay weakens recovery as an independent backstop.
- **S1/high:** unauthenticated inbox operations allow read/delete/overwrite/DoS.
- **S1/high:** remote-ready tasks cannot be completed by the normal coordinator; Pilot can appear more complete than it is.
- **S1/high:** review demotion, provider lowering, key revocation, and capability revocation do not form one incident transaction.
- **S2/high:** consumed/rejected bytes may lack a durable tamper-verifiable review record; false quarantine has no shipped human release path.
- **S2/high:** signatures prove provider attribution, not named model/silicon execution.

### Recommendations

1. **`[RECOMMENDATION — D/I/V, P0]`** Put root/recovery operations behind a separately authenticated signer principal unavailable to worker UIDs; bind requester, purpose, digest, scope, rate, and audit; fail closed on plaintext for production profiles.
2. **`[RECOMMENDATION — I/V, P0]`** Make recovery head/challenge/verifier-time/one-use bound. Authenticate recipient inbox list/read/ack; make insert immutable and id-bound.
3. **`[RECOMMENDATION — D/F]`** Ratify federation governance or label it experimental; narrow “complete,” “ACL,” “sigchain,” “quorum,” “all seams,” “dispatcher wired,” and “turnkey” to exact enforcement.
4. **`[RECOMMENDATION — I/V]`** Require digest-bound verdict persistence at enforcing seams; tamper-verify/sign the log; record reviewer source/route; implement quarantine adjudication/release.
5. **`[RECOMMENDATION — D/I/V]`** Choose coordinator-owned remote lifecycle or explicit manual-only admission. If shipped, add restart-safe offer→accept→completion plus renewal/sweep and cross-store recovery.
6. **`[RECOMMENDATION — D/I]`** Decide whether grant model is intent or enforced provenance; if enforced, sign actual handler/model/reasoning/isolation/usage evidence.
7. **`[RECOMMENDATION — D/I]`** Define a correlated incident policy across author trust, provider eligibility, capabilities, keys, dependent consumers, and re-runs.

### Deeper audit artifacts

- [`14-federation-identity-security.md`](14-federation-identity-security.md)
- [`15-review-exec-pilot.md`](15-review-exec-pilot.md)
- [`21-agency-federation-safety-synthesis.md`](21-agency-federation-safety-synthesis.md)
- Register [`30` §3.4](30-contradiction-and-drift-register.md#34-federation-review-execution-federation-and-pilot)

---

## 9. Testing, CI, and release evidence

### Abstract

WorksGood has a large and varied evidence estate, including strong library, formal, installer, Pi embed, integration, smoke, and human-flow work. The audit does not equate inventory with activation. Current CI and completion do not select enough of that estate to support broad “gated” claims.

### Findings

1. **`[VERIFIED BEHAVIOR — current contradiction]`** Audit 17 ran all six `integration_smoke_gate` cases; all failed. After worker-environment sanitation, they still expected a retired completion path. Current Done has no manifest runner and rejects smoke flags (`src/main.rs:1261-1275`; `src/commands/completion_done.rs:32-104`; [`17` §7.2](17-testing-ci-quality.md#72-commands-actually-executed)).
2. **`[OBSERVED FACT — current]`** Normal CI selects the library harness and named formal/service/binary targets, not broad `cargo test --tests`; the audit counted 177 Cargo integration targets and only eight names referenced in CI (`.github/workflows/ci.yml:68-162`; `17` `TEST-002`). “169 omitted” means not explicitly selected, not 169 known failures.
3. **`[OBSERVED FACT — positive]`** The Pi package has a declared build/test/re-embed/diff gate; the universal agent contract has source include and parity tests; formal checks explicitly bound OS/filesystem/network exclusions (`.github/workflows/ci.yml:82-125,174-201`; `src/commands/agent_guide.rs:3-15,132-185`; `formal/README.md:3-5,86-138`).
4. **`[OBSERVED FACT — policy drift]`** Smoke policy categorically says real endpoint/binary, while manifest includes static contracts, compile-only checks, fixture binaries, and fixed Pilot workers (`tests/smoke/README.md:82-87`; `tests/smoke/scenarios/release_workflow_signing_contract.sh:2-10`; `src/commands/pilot_cmd.rs:43-50`; `WGDR-045`).
5. **`[OBSERVED FACT — suspected drift]`** Helper policy and corpus differ on trap/temp/daemon patterns; per-script applicability was not fully adjudicated (`WGDR-046`).
6. **`[OBSERVED FACT — release gap]`** Release construction has checksums/attestation/package steps, but source/release binary membership and platform runtime qualification remain separate authorities (`Cargo.toml:20-41`; `.github/workflows/release.yml:470-523,646-667`; `WGDR-013`).

### Risks

- **S1/high:** an owned failing smoke can coexist with Done despite the published agent/release contract.
- **S1/high:** green CI misses stale/failing integration contracts.
- **S2/high:** “smoke passed” hides deterministic/static/credentialed/multi-host distinctions.
- **S2/medium:** environment-dependent skips and process-global tests can yield false reassurance or flakes.
- **S2/medium:** macOS/Windows release construction is mistaken for runtime qualification.

### Recommendations

1. **`[RECOMMENDATION — D/I/V, P0]`** Decide whether smoke gates Done, publication, or neither. Bind selected manifest/policy/results to immutable completion evidence if required.
2. **`[RECOMMENDATION — V/S]`** Classify every integration target and smoke scenario: required hermetic, human-flow, platform, live advisory, static contract, formal bounded, release artifact. Make skip/not-selected explicit.
3. **`[RECOMMENDATION — I/V]`** Select the current completion integration gate in CI and require every target to be selected or explicitly quarantined.
4. **`[RECOMMENDATION — S/V]`** Generate an evidence dashboard from exact commit/artifact receipts; required lanes cannot pass with zero assertions.
5. **`[RECOMMENDATION — D/S/V]`** Publish one binary/platform support manifest shared by Cargo, installer, archive, docs, CI, and release workflows.

### Deeper audit artifacts

- [`17-testing-ci-quality.md`](17-testing-ci-quality.md)
- [`22-product-docs-quality-synthesis.md` §2.6, §3](22-product-docs-quality-synthesis.md#26-evidence-authority-indexing-and-testing-model)
- Register `WGDR-002`, `WGDR-044`–`WGDR-046`

---

## 10. Documentation and conceptual coherence

### Abstract

The documentation problem is broken authority composition, not lack of prose. The estate is large, status/applicability is usually implicit, curated indexes claim completeness, hand-maintained derivatives have unclear source graphs, and immutable historical reports lack supersession navigation. Where the repository declares one source and tests its derivative, synchronization works.

### Findings

1. **`[OBSERVED FACT — snapshot-bounded]`** Audit 16 counted 555 docs Markdown files and 56 root Markdown files; the later sync-plan checkout counted 619 files below `docs/`, 570 Markdown. Counts changed as audits landed; the structural fact is absence of an estate/claim/decision/glossary registry (`16` §2.1; [`31` §1](31-documentation-sync-plan.md#1-executive-abstract)).
2. **`[OBSERVED FACT — current]`** `docs/KEY_DOCS.md` calls itself canonical and dated, but is a curated router, not a complete estate index. `COMMANDS.md` similarly does not cover the current help surface (`docs/KEY_DOCS.md:1-16`; `16` `DOC-001/005`).
3. **`[CONTRADICTION — source graph]`** Manual README calls unified Typst authoritative while the sync script treats chapter Typst as source, concatenates Markdown, and can copy raw Typst to `.md` when conversion fails (`docs/manual/README.md:30-42`; `scripts/sync-docs.sh:1-8,66-118`).
4. **`[OBSERVED FACT — current]`** Current designs, accepted decisions, historical reports, proposals, deterministic sparks, and shipped later slices often lack section-level status/applicability/supersession. This causes both false “current” and false “still hypothetical” readings (`WGDR-047/048`, `WGDR-R08/R09`).
5. **`[OBSERVED FACT — positive control]`** `AGENTS.md` and `CLAUDE.md` are intentional byte-identical derivatives with parity tests; Pi embedded output is regenerated and diffed. These should not be blindly deduplicated (`src/commands/agent_guide.rs:132-185`; `.github/workflows/ci.yml:174-201`).
6. **`[INFERENCE — high confidence]`** Parser, dispatch, product support, test selection, and docs must be joined. Generating CLI reference from Clap alone would preserve the current rejected Done flags.
7. **`[OBSERVED FACT — current vocabulary debt]`** Typed code distinguishes task/generation/attempt/process; author/provider trust; agency/runtime/federated identities; inbound/completion review. Public prose often compresses them (`19` §3; `WGDR-T01`–`T12`).

### Risks

- **S1/high:** stale setup/security/completion prose causes action before source fails closed.
- **S2/high:** historical reports reverse current conclusions without supersession edges.
- **S2/high:** a new manifest becomes another stale index unless orphan/delta checks are release gates.
- **S2/medium:** bulk moves break inbound links and destroy tool-required discovery.

### Recommendations

1. **`[RECOMMENDATION — S]`** Create distinct registries: `docs/manifest.toml` (estate), `decision-index.toml` (normative applicability), `product-contract.toml` (claims/support/evidence join), `glossary.toml` (namespaced terms), and generated evidence/result index.
2. **`[RECOMMENDATION — F]`** Immediately contain false high-impact claims with scoped `broken/partial/decision-required` notices; do not settle disputed semantics in prose.
3. **`[RECOMMENDATION — S/V]`** Declare generator DAGs; fail closed on converter error; regenerate-and-diff; check links/assets/redirects according to current/historical class.
4. **`[RECOMMENDATION — S]`** Preserve historical bodies and add external observed-revision/applicability/supersession indexes.
5. **`[RECOMMENDATION — D/S]`** Ratify the namespaced glossary and maturity vocabulary before global replacements.

### Deeper audit artifacts

- [`16-documentation-information-architecture.md`](16-documentation-information-architecture.md)
- [`19-conceptual-model-and-vocabulary.md`](19-conceptual-model-and-vocabulary.md)
- [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md)
- [`31-documentation-sync-plan.md`](31-documentation-sync-plan.md)

---

## 11. Operations, configuration, observability, and UX

### Abstract

Installer collision/receipt handling, profile fail-closed behavior, secret redaction, authenticated service status, and conservative cleanup are strong. Day-1/day-2 operations are nevertheless fragmented, and several commands give incorrect or route-inappropriate information. These are control-plane integrity issues, not polish.

### Findings

1. **`[VERIFIED BEHAVIOR — current S1]`** Worker message read may mutate state before flattened array response serialization fails (`WGDR-049`; §1 priority 2).
2. **`[VERIFIED BEHAVIOR — current S1]`** `config set` erased comments, accepted an ineffective unknown path, and lint declared it clean (`src/commands/config_cmd.rs:3027-3102,3476-3676`; [`18` §7.2](18-operations-configuration-ux.md#72-reproducible-command-record)).
3. **`[OBSERVED FACT — current]`** `doctor` makes Claude absence an error before optional/conditional Pi checks, conflicting with Pi-only setup (`src/commands/doctor.rs:166-226,267-412`; `WGDR-009`).
4. **`[OBSERVED FACT — current]`** Spend groups all records under invocation day; metrics are process-local statics presented with broader semantics (`src/commands/spend.rs:27-67`; `src/metrics.rs:8-26`; `src/commands/metrics.rs:1-20`).
5. **`[VERIFIED BEHAVIOR — current]`** A dirty attached root checkout can block unrelated publication, serializing independent work at a human workspace (`18` `OPS-006`).
6. **`[OBSERVED FACT — current risk]`** HTML remote publishing includes broad task metadata by default; transcript exclusion does not make every task public (`18` `OPS-008`).
7. **`[OBSERVED FACT — partial]`** Source install exposes four binaries while archives/receipt/docs expose three. Archive replacement is per-file rather than set-transactional (`Cargo.toml:20-41`; release sources; `WGDR-013`).
8. **`[OBSERVED FACT — bounded secret safety]`** Secret redaction/plaintext opt-in are positive, but “keyring” can resolve to an unencrypted file depending on backend/deployment (`18` `OPS-011`).
9. **`[OBSERVED FACT — positive]`** Service status/identity and cleanup defaults are conservative: dry-run/dirty-safe behavior and preserved logs reduce destructive operator error (`18` `OPS-012`).
10. **`[OBSERVED FACT — UX debt]`** Help is curated but brittle for exploration/pipes; no one route-aware readiness command joins config, credential, plugin, endpoint, daemon, and selected model (`18` `OPS-005/013`).

### Risks

- **S1/high:** consumed worker inbox state without usable response.
- **S1/high:** config edits destroy comments or silently persist ineffective keys.
- **S1/high:** readiness/accounting/metrics cause false operational decisions.
- **S1/user-dependent:** remote publishing leaks non-public task metadata.
- **S2/medium:** package/platform/upgrade mismatch causes partial or unsupported installations.

### Recommendations

1. **`[RECOMMENDATION — I/V, P0]`** Replace flatten with named typed `data`; make stateful read delivery replayable/receipted by stable request ID; test real socket arrays/maps and response loss.
2. **`[RECOMMENDATION — I/V, P0]`** Use lossless TOML editing; reject typo paths or require reviewed extension namespace/`--raw`; split schema/migration/selection/secret/runtime lint.
3. **`[RECOMMENDATION — I/V, P1]`** Build route-aware `doctor --all` and four supported operator journeys with exact mutations, credentials, side effects, rollback, and release-binary human-flow tests.
4. **`[RECOMMENDATION — I/F]`** Correct spend timestamps and metric scope; decouple publication from dirty human root; make remote publishing public-only by default.
5. **`[RECOMMENDATION — D/S/I]`** Unify package manifest and transactional upgrade; publish truthful platform support and a general day-2/recovery runbook.

### Deeper audit artifacts

- [`18-operations-configuration-ux.md`](18-operations-configuration-ux.md)
- [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md)
- Register [`30` §3.2](30-contradiction-and-drift-register.md#32-installation-configuration-model-plane-accounting-and-packaging)

---

## 12. Contradiction register summary

### Abstract

The bundle deduplicates repeated leaf findings into **49 open `WGDR-*` records**, 12 terminology collisions, 12 resolved/narrowed apparent contradictions, and an explicit uncertainty register. The largest clusters are incomplete migrations, not random copy errors. This summary includes every high-severity contradiction and does not treat resolved safeguards as defects.

### Findings: all material S1 contradiction/drift records

| ID | Short form | State / authority | Required disposition |
|---|---|---|---|
| `WGDR-001` | Done help flags are rejected by operator and worker dispatch. | open fact; dispatch controls current behavior | remove or implement one protocol across parser/dispatch/help/tests |
| `WGDR-002` | Owned smoke is claimed as Done gate but absent from current completion; permanent target is stale/unselected. | open fact | restore a v3-bound gate or narrow the contract immediately |
| `WGDR-008` | Bare `worksgood` Pi/plugin prerequisite prose is false for existing graphs. | open scoped fact | split existing/new/unattended journeys |
| `WGDR-009` | Pi-only setup conflicts with old route tests and Claude-first doctor. | open fact/product-scope decision | route-aware doctor and elected model scope |
| `WGDR-010` | Config edit claims preservation but erases comments. | open verified fact | lossless edit or narrow promise |
| `WGDR-011` | Unknown config persists, is ineffective, and lint can say clean. | open verified fact | schema classes, typo rejection, exact remedy |
| `WGDR-012` | Spend dates and metrics scope overstate semantics. | open fact | persist/derive correct time/scope or rename |
| `WGDR-021` | Universal completion review does not feed agency learning. | open architectural fact | v3-only lifecycle + attempt ledger + exactly-once projector |
| `WGDR-029` | Same-user custodian is not claimed hostile-worker signer boundary. | open security fact | separate authenticated signer/HSM boundary |
| `WGDR-030` | Recovery is not reliably offline/windowed/transition-bound. | open security fact | current-head challenge, verifier time, one-use, safe ceremony |
| `WGDR-032` | Inbox list/read/delete is unauthenticated and ack is not owned. | open security/reliability fact | recipient authentication, immutable insert, ack/cursor |
| `WGDR-040` | RemoteRunner plans but normal spawn rejects. | open workflow fact | owned lifecycle or explicit manual-only policy |
| `WGDR-042` | Pilot one-command/live/key-wired claims exceed real path. | open operator fact | complete two-host path or rename to bootstrap |
| `WGDR-049` | Flattened worker response cannot carry arrays after stateful read. | open verified fact | typed envelope and idempotent delivery |

### Other material open clusters

- **Lifecycle:** manuals describe wrong status/dependency/wait semantics; manual claim bypasses pause/time; cycles, Abandoned retry, and special legacy reachability require decisions (`WGDR-003`–`007`).
- **Model/packaging:** binary membership, execution-surface catalog, Pi topology, provider-leading deprecation, fallback, and upgrade support conflict (`WGDR-013`–`018`).
- **Agency/functions/human:** identity hashes, auto-assignment, attempt visibility, placeholder reward, function planner/history, onboarding transaction, and chat storage need synchronization (`WGDR-019`–`027`).
- **Federation/review/exec:** governance, compatibility, ACL wording, state safety, historical-signature semantics, topology, review audit/quorum/seams, and cross-store accept are partial (`WGDR-028`–`041`).
- **Docs/tests:** source DAG, CI compile diagnosis, smoke classes/helpers, design maturity, and version literals drift (`WGDR-043`–`048`).

### Resolved or narrowed safeguards that must not be “fixed away”

1. Graph-only fresh init and a default compatibility executor answer different scopes (`WGDR-R01`).
2. Discovery may include handlers that unattended admission rejects (`R02`).
3. `AGENTS.md`/`CLAUDE.md` parity is intentional and tested (`R03`).
4. Unknown authors resolve fail-closed despite a different enum default (`R04`).
5. `Task.assigned` and `Task.agent` represent runtime ownership and agency composition (`R05`).
6. No offline forward secrecy is accepted design debt, not hidden drift (`R06`).
7. Broad generic leash and narrow WG-Exec task grants coexist (`R07`).
8. Old spark/audit claims can be correct historically and superseded now (`R08/R09`).
9. Completion review and agency performance evaluation are technically distinct (`R10`).
10. Worker-capability environment contamination was a harness issue; do not weaken the boundary (`R11`).
11. Cycle support is partial/unelected, not safely declared absent (`R12`).

### Risks

- **S1:** deleting old flags/prose without deciding smoke/cycle/retry authority can remove intended controls or preserve false assurances.
- **S1:** rewriting security claims without implementation can make partial custody/review/Pilot appear fixed.
- **S2:** treating old reports as wrong destroys provenance; treating them as current reverses conclusions.
- **S2:** terminology replacement without namespacing can collapse legitimate authorities.

### Recommendations

1. Keep `WGDR-*` stable and machine-map each row to owner, factual edit, decision, implementation, verification, accepted debt, or uncertainty test.
2. Prioritize behavioral and security S1 records, but preserve exact evidence confidence and scope.
3. Close contradictions with paired source/help/test/docs changes and exact receipts; never close by prose age.
4. Preserve the resolved/non-issue set as regression guardrails.

### Deeper audit artifacts

- Full register: [`30-contradiction-and-drift-register.md`](30-contradiction-and-drift-register.md)
- Evaluation migration adjudication: [`23`](23-evaluation-evolvability-cutover.md)
- Synchronization decision queue: [`31` §4.1](31-documentation-sync-plan.md#41-decision-queue)

---

## 13. Prioritized action and synchronization roadmap

### Abstract

The roadmap is a dependency-ordered integrity program, not a doc cleanup. First contain harmful claims and assign authorities; then correct known facts; then decide contested behavior; then implement machine-readable contracts and generators; only then migrate information architecture and turn drift checks into release gates.

### Findings: root causes

1. **Migration without retirement:** current completion, Pi onboarding, handler-first routing, review receipts, and federation waves landed while legacy surfaces remained discoverable.
2. **Policy copied instead of shared:** ready/claim, parser/dispatch, route discovery/admission, store durability, package membership, and docs status use independent predicates.
3. **Late projection after terminal boundary:** usage, review attempts, and learning are not committed with the authoritative terminal event.
4. **Long work in global lanes:** synchronous review occupies coordinator IPC.
5. **Inventory mistaken for activation:** test/doc presence is treated as selected evidence/current authority.
6. **Historical evidence without closure:** immutable reports lack applicability/supersession routing.

### Recommended critical path

| Phase | Priority work | Type | Exit criteria |
|---|---|---|---|
| **0. Baseline and containment** | export all WGDR/term/resolved/uncertainty rows; assign named owner/reviewer; add narrow notices for false action-authorizing claims; capture exact help/CI/journey evidence | S/V/F/D | every record has a disposition; no factual notice chooses policy; failures/skips remain visible |
| **1. P0 control-plane integrity** | off-thread idempotent completion; typed replayable worker IPC; one admission predicate; lossless/schema-aware config; correct accounting; isolate custody; authenticate inbox | I/V | real socket/human/security-negative/fault flows pass; no unrelated IPC stall; no silent mutation/loss |
| **2. Authority decisions** | Done/smoke/cycles/retry; Pi/handler/public package scope; evaluation/learning identity and credit; federation threat model; review audit/quorum/bypass; remote lifecycle/Pilot; docs DAG/glossary/crash claim | D→I/F/V | accepted decision records with alternatives, migration, rollback, named approver; source/help/tests/docs agree when implemented |
| **3. Evaluation and remote joins** | review-attempt ledger + virtual views; exactly-once learning projector; assignment receipts; coordinator-owned remote state machine or explicit manual refusal; recoverable remote accept | I/V | lifecycle remains v3-only; replay does not duplicate learning/model calls; remote restart/failure flow or clear manual-only behavior |
| **4. Structural contracts** | estate manifest, decision index, product contract, glossary, CLI/schema generators, docs DAG, links, evidence dashboard | S/V | every public/safety claim, command, enum, target, scenario, binary, doc, and generated output classified or explicitly excluded |
| **5. Current-doc and IA migration** | four journeys; task-first concepts; generated reference; operations runbooks; path mapping/root cleanup | F/S/V | actual release-binary flows match; redirects/link checks pass; tool-required files preserved |
| **6. Evidence/archive/release gate** | bundle indexes, section-level status, retention, unclassified-delta gates, parser-dispatch-support join, evidence selection, human-flow canaries | S/V/D | exact receipts show selected pass/fail/skip; historical body hashes unchanged; unclassified public deltas block release |

### P0 implementation acceptance checks

1. Slow reviewer exceeds client timeout while unrelated Show/Log/Wait/Done and daemon ticks remain responsive; replay commits once.
2. Worker `MessageRead`, `MessagePoll`, `ArtifactList`, `Show`, and `Context` round-trip arrays/maps; injected response loss does not consume unique delivery.
3. Default claim refuses paused/future work; explicit override, if approved, records reason and lifecycle event.
4. Config golden files preserve comments/order; typos fail with suggestions; extension/raw policy is explicit.
5. Source, FLIP, and eval usage are non-null/deduplicated and separately reportable; spend uses terminal time and marks unknown cost.
6. Hostile worker UID cannot read keys or invoke arbitrary signing; recovery replay/backdating fails; unauthenticated inbox read/delete fails.
7. Review persistence failure cannot yield consumed-but-unrecorded bytes at enforcing seams.
8. Remote accepted epoch cannot strand an unrecoverable graph completion; renewal/sweep ownership is explicit.

### Human decisions that cannot be delegated to prose

- `DEC-01/02`: completion flags, smoke, cycles, retry, claim override.
- `DEC-03/04`: Pi scope, handler/public command/binary/platform membership.
- `DEC-05/06`: evaluation performance/learning representation and persona↔principal↔human binding.
- `DEC-07/08`: federation custody/recovery/governance/history and review audit/quorum/bypass.
- `DEC-09`: coordinator-owned remote execution versus manual; Pilot bootstrap versus turnkey.
- `DEC-10/11/12`: docs source DAG, glossary, and public persistence guarantee.

### Risks

- **Program risk:** mass rewrite before decisions creates another unenforced authority.
- **Program risk:** a registry can drift unless new unclassified deltas fail CI/release.
- **Security risk:** documentation narrowing without implementation reduces overclaim but does not reduce exploitability.
- **Delivery risk:** changing current completion and remote lifecycle simultaneously can cross-contaminate authority; keep same-source changes sequential.
- **History risk:** bulk archive/move can destroy inbound links and point-in-time evidence.

### Recommendations

- Small, domain-scoped factual commits first; no path moves.
- Same files and same authority migrate sequentially; independent P0 implementation streams may run in parallel with an explicit integration gate.
- Every public/safety change names claim ID, decision, source/dispatch, behavior test, CI/release lane, docs, owner, and rollback.
- “No docs needed” requires a contract row showing the change is internal.
- Quarterly audits supplement same-change gates; they do not replace them.

### Deeper audit artifacts

- Full phased plan and decision queue: [`31-documentation-sync-plan.md`](31-documentation-sync-plan.md)
- Ranked core actions: [`20` §6](20-core-runtime-synthesis.md#6-recommendations)
- Trust/safety actions: [`21` §6](21-agency-federation-safety-synthesis.md#6-recommendations)
- Product actions: [`22` §6](22-product-docs-quality-synthesis.md#6-ranked-recommendations)
- Learning representation/acceptance: [`23` §6](23-evaluation-evolvability-cutover.md#6-recommendations-and-human-decisions)

---

## 14. Detailed evidence and artifact traceability

### Abstract

This section is the deepest layer: it maps every audit artifact from `10-*.md` through `31-*.md` that exists in the bundle, lists representative primary evidence, and states what this draft did and did not verify. It is navigation, not a replacement for leaf transcripts.

### Findings: artifact coverage matrix

| Artifact | Contribution used in this draft | Main destinations |
|---|---|---|
| [`10-code-architecture.md`](10-code-architecture.md) | package/binary map, persistence, duplicate authorities, guardrails, config/read-model boundaries | §§4–5 |
| [`11-orchestration-lifecycle.md`](11-orchestration-lifecycle.md) | lifecycle sequences, claim bypass, spawn transaction, completion generations, daemon blocking, rejection feedback | §5 and executive priorities |
| [`12-model-execution-plane.md`](12-model-execution-plane.md) | route/surface matrix, Pi topologies, streams/watchdog/accounting, setup drift | §6 |
| [`13-agency-evaluation-chat.md`](13-agency-evaluation-chat.md) | persona hash, assignment reachability, legacy learning, functions, chat/memory, human authority | §7 |
| [`14-federation-identity-security.md`](14-federation-identity-security.md) | cryptography, custody, recovery, transport, compatibility, freshness, delegation, state safety | §8 |
| [`15-review-exec-pilot.md`](15-review-exec-pilot.md) | ingest/review enforcement, trust split, remote accept/lease, coordinator seam, Pilot maturity | §8 |
| [`16-documentation-information-architecture.md`](16-documentation-information-architecture.md) | estate inventory, authority/freshness, source graphs, target IA, link/index gaps | §10 |
| [`17-testing-ci-quality.md`](17-testing-ci-quality.md) | CI selection, disconnected smoke, scenario classes, release qualification, positive Pi/formal controls | §9 |
| [`18-operations-configuration-ux.md`](18-operations-configuration-ux.md) | worker IPC, config, onboarding/doctor, accounting/metrics, publish/install/secrets/UX | §11 |
| [`19-conceptual-model-and-vocabulary.md`](19-conceptual-model-and-vocabulary.md) | task-centered product model, object hierarchy, glossary and authority collisions | §§3, 10 |
| [`20-core-runtime-synthesis.md`](20-core-runtime-synthesis.md) | reconciled architecture/orchestration/model findings; corrected Pi-accounting cause | §§1, 4–6 |
| [`21-agency-federation-safety-synthesis.md`](21-agency-federation-safety-synthesis.md) | typed authority vector and cross-plane seam risks | §§1, 7–8 |
| [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md) | product contract, operator journey, evidence activation, docs/testing/UX priorities | §§1, 3, 9–11 |
| [`23-evaluation-evolvability-cutover.md`](23-evaluation-evolvability-cutover.md) | historical cutover, receipt lineage, live 12-task observation, target ledger/projector design | §7 and roadmap |
| [`30-contradiction-and-drift-register.md`](30-contradiction-and-drift-register.md) | 49-item deduplication, S1 primary checks, terminology/resolved/uncertainty registers | §12 and all risk adjudication |
| [`31-documentation-sync-plan.md`](31-documentation-sync-plan.md) | authority hierarchy, five registries, six phases, decision queue, rollback/release gates | §13 |

**Completeness note:** there are no `24-*.md` through `29-*.md` artifacts in this bundle; the table covers every existing numbered artifact in the requested `10`–`31` range.

### Representative direct repository evidence

| Question | Primary source span | Draft conclusion |
|---|---|---|
| What is a task/status? | `src/graph.rs:379-529,689-1035` | durable task center; eleven statuses; Done-specific dependency semantics |
| What owns lifecycle? | `src/lifecycle.rs:605-1507` | generation/attempt/fence/event-repaired authority with compatibility projection |
| How are mutations serialized? | `src/parser.rs:275-414` | Unix locked ledger-before-projection mutation |
| What does ready/claim do? | `src/query.rs:306-517`; `src/commands/claim.rs:11-151` | readiness is fail-closed; manual claim omits pause/time |
| How does spawn fence execution? | `src/commands/spawn/execution.rs:1283-1438,1780-2160` | durable setup and launch permit before handler starts |
| What authorizes Done? | `src/commands/completion_submit.rs:187-487`; `completion_done.rs:29-294` | exact manifest/review/publication, not exit/status button |
| Why can daemon block? | `src/commands/service/mod.rs:3330-3570`; `service/ipc.rs:286-350,835-919` | completion work runs inline on coordinator lane |
| Why can worker replies fail? | `src/commands/service/ipc.rs:253-274,720-790`; `src/messages.rs:631-696` | flatten expects map while operations return arrays after state mutation |
| Which model surface is allowed? | `src/config.rs:2395-2433,3590-3710`; `src/dispatch/handler_for_model.rs:45-137` | strict worker subset versus broad discovery/resolver |
| Which Pi topology runs? | `src/service/executor.rs:1729-1752`; `src/commands/pi_handler.rs:492-537` | JSON worker differs from hermetic RPC handler |
| What does review bind? | `src/completion_review.rs:83-121,182-299`; `src/completion_task.rs:95-217` | immutable exact candidate, thin operational lineage |
| Does review feed learning? | `src/agency/eval.rs:49-201`; `src/agency/evolver.rs:120-224` | legacy store only; no modern join |
| What does `wgid`/capability prove? | `src/identity/sigchain.rs:680-886`; `src/identity/custody.rs:294-486,681-805` | key lineage and attenuated authority, not trust/safety |
| Is custody isolated? | `src/identity/keys.rs:51-68,226-377` | same-process/same-user key load |
| Is inbox recipient-owned? | `src/identity/node.rs:408-443,551-572`; `src/identity/transport.rs:309-354` | no authenticated list/read/delete/ack transaction |
| Is review audit mandatory/signed? | `src/review/verdict.rs:53-190` | local unsigned hash chain; live recording can be best-effort |
| Does dispatcher run remote? | `src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:339-348` | planning yes, normal runtime no |
| Is Pilot turnkey? | `src/commands/pilot_cmd.rs:43-50,1066-1215` | deterministic rehearsal/one-host bootstrap, not full live team |
| Is config edit lossless? | `src/commands/config_cmd.rs:3029-3096,3476-3676` | no; semantic reserialization and weak unknown-key handling |
| Is accounting time/scope correct? | `src/commands/spend.rs:27-67`; `src/metrics.rs:8-26` | invocation-date spend and process-local metrics |
| What does CI select? | `.github/workflows/ci.yml:68-201` | strong bounded lanes, incomplete integration/smoke activation |

### Exact commands already executed by upstream audits

This draft adopts the bounded results and limitations recorded in the leaves rather than pretending to rerun them. Representative commands include:

```bash
# Candidate-built parser behavior (audit 30 P1)
cargo run --quiet --bin wg -- done --help

# Source reachability/call-site checks (audit 20 / 23)
rg -n 'commands::done::run|done::run\(' src
rg -n 'task_owned_done\(' src
rg -n 'record_evaluation|record_evaluation_with_inference' src

# Focused tests and traces are recorded verbatim in each leaf §7.
# Artifact validation for this draft appears below.
```

Leaf execution summary, with exact environments and exclusions preserved:

- Architecture: focused build/persistence/add-show-config and stale Done tests (`10` §7).
- Orchestration: two CLI traces, 15 targeted Rust binaries, six smokes, installed-daemon observation (`11` §7).
- Model: focused route/stream/watchdog/profile/native tests and generated Pi wrapper traces (`12` §7).
- Agency: selected agency/evaluation/context tests and call-graph checks (`13` §7).
- Federation: 100 identity tests and four isolated operator-mode smokes (`14` §7).
- Review/exec/Pilot: focused review/provider/trust/Pilot/planner tests; live smokes were worker-authority blocked in that leaf (`15` §7).
- Product leaves: inventory/source checks, six failing smoke-gate cases, and clean-room operations/conceptual fixtures (`16`–`19` §7).
- Cutover: 9/9 completion valve, 34/34 active agency pipeline tests, and bounded live graph inspection (`23` §1, §7).

### Risks and evidence limits

**`[UNCERTAINTY]`** Neither this draft nor the whole bundle establishes full production readiness. Missing or bounded evidence includes:

- no full Cargo and full smoke pass for the audited snapshot;
- no real external provider matrix, credential/rate-limit/schema coverage, or production-duration watchdog run;
- no complete Windows/macOS runtime journey or non-Unix concurrent-writer proof;
- no power-loss, disk-full, NFS, PID-reuse, multi-daemon, or comprehensive cross-store chaos campaign;
- no hostile same-UID custodian exploit test or repository-enforced distinct-UID deployment invariant;
- no credentialed independent review quorum, human quarantine release, TEE, DHT/Iroh, or coordinator-owned remote/Pilot flow;
- no end-to-end incident response joining review demotion, provider trust, key/capability revocation, and consumer re-run;
- no one actor-binding test spanning human, chat, agency persona, runtime worker, model route, and `wgid:`;
- no proof that every historical report/design has an applicability successor.

Test absence is not exploit proof. File/test presence is not pass proof. A deterministic smoke is evidence for its protocol shape, not a credentialed model or multi-host boundary.

### Recommendations and draft validation

The task-specific validation for this artifact is:

```bash
test -s docs/audit/2026-08-08-worksgood-system/40-system-synthesis-draft.md
# verify every existing 10-*.md through 31-*.md artifact is linked
# verify headings/anchors provide a navigable table of contents
git diff --check
```

### Deeper audit artifacts

The artifact coverage matrix above links every leaf and synthesis. For line-level transcripts, use each leaf's §7 evidence appendix; for deduplicated adjudication use [`30`](30-contradiction-and-drift-register.md); for implementation/documentation dependency order use [`31`](31-documentation-sync-plan.md).

**Final caution:** This is a review draft. Recommendations, target registries, authority hierarchy, and maturity wording remain proposed until downstream independent review and final synthesis adjudicate them. The draft's factual claims remain scoped to the cited repository revision, source spans, commands, and inherited evidence.
