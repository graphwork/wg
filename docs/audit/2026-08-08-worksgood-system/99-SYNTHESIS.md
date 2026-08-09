# WorksGood system audit — reviewed final synthesis

**Audit date:** 2026-08-08

**Final synthesis date:** 2026-08-09

**Audited product snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Final-synthesis checkout:** `7219f71540557bc79fe313a6dd546ca9463292d5`

**Evidence checked through:** 2026-08-09

**Status:** final audit synthesis; descriptive and advisory, not a product contract, security certification, release approval, or permission to change production behavior

**Normative method:** [`README.md`](README.md)

> **Applicability warning — historical pinned snapshot, not current HEAD.** Every unqualified source citation below is interpreted at `b0892ea7496fd2cc8f641417a3d8e33ca9add369`. The final-synthesis checkout is a descendant but differs from that snapshot in **89 non-audit files, 5,995 insertions, and 413 deletions**. Relevant later work includes setup activation/readiness, Pi accounting/review visibility, and completion/recovery changes. This report therefore says **snapshot-current**, not “current,” and does not selectively declare old findings fixed. Recheck the named source and executable flow before turning any item into a present-day backlog.

> **Audit-only boundary.** This artifact changes no product source, test, workflow, schema, generated output, or pre-existing documentation. Recommendations and decision defaults below are proposals only.

## Reading key

The exact evidence labels required by the charter are used throughout:

- **`[FACT]`** — repository fact at the pinned revision; source shape is not runtime reachability.
- **`[VERIFIED]`** — behavior or repository/provenance command actually executed with the command, environment, date, and result recorded.
- **`[DOC-CLAIM]`** — statement made by documentation, help, ADR, or report.
- **`[INFERENCE]`** — bounded conclusion from cited evidence; confidence and falsifier are stated where material.
- **`[RECOMMENDATION]`** — proposed action or decision, not shipped behavior.
- **`[CONTRADICTION]`** — two authorities cannot be read literally in the same scope.
- **`[UNCERTAINTY]`** — evidence, applicability, or desired authority remains unresolved.

Evidence strength used in compact tables: **E1** exact executed candidate behavior; **E1-I** installed-binary observation with incomplete build identity; **E2** pinned implementation; **E3** inspected but unexecuted executable specification; **E4** document/decision claim; **E5** historical context. Risk rows separate **severity**, **likelihood**, and **confidence**. “Observed” likelihood means the harmful condition, not merely a textual disagreement, was observed in the cited bounded environment.

## Contents

1. [Executive abstract](#1-executive-abstract)
2. [Scope, method, and compact system map](#2-scope-method-and-compact-system-map)
3. [Top-level state assessment](#3-top-level-state-assessment)
4. [Product identity, architecture, and persistence](#4-product-identity-architecture-and-persistence)
5. [Task, orchestration, and completion lifecycle](#5-task-orchestration-and-completion-lifecycle)
6. [Model execution, configuration, accounting, and operations](#6-model-execution-configuration-accounting-and-operations)
7. [Agency, evaluation, functions, chat, and evolvability](#7-agency-evaluation-functions-chat-and-evolvability)
8. [Federation, trust, review, remote execution, and Pilot](#8-federation-trust-review-remote-execution-and-pilot)
9. [Testing, CI, release, and human-facing evidence](#9-testing-ci-release-and-human-facing-evidence)
10. [Documentation and conceptual coherence](#10-documentation-and-conceptual-coherence)
11. [Cross-cutting findings](#11-cross-cutting-findings)
12. [Contradiction, drift, and uncertainty summary](#12-contradiction-drift-and-uncertainty-summary)
13. [Prioritized human decision queue](#13-prioritized-human-decision-queue)
14. [Synchronized documentation roadmap and handoff](#14-synchronized-documentation-roadmap-and-handoff)
15. [Independent-review dispositions](#15-independent-review-dispositions)
16. [Evidence and provenance appendix](#16-evidence-and-provenance-appendix)

---

## 1. Executive abstract

### 1.1 What WorksGood is at the audited snapshot

**`[INFERENCE]` (high confidence)** WorksGood is best described as a **local-first durable work-and-evidence system**. Its stable center is a file-backed task graph. An attended TUI/chat layer and an optional service daemon operate that graph; bounded worker attempts invoke model handlers; immutable candidate/review/publication evidence derives ordinary successful completion; agency, federation, content review, and remote-compute protocols add distinct identity, learning, and cross-instance planes. This description follows the durable types and dispatch paths rather than the broader “work OS” slogan (`src/graph.rs:379-529,689-1035`; `src/lifecycle.rs:66-213`; `src/service/registry.rs:37-90`; `src/commands/completion_done.rs:29-294`; [`19`](19-conceptual-model-and-vocabulary.md), `CONCEPT-001..010`). A falsifier would show that another persisted object, rather than task/evidence state, is the common authority for the ordinary user journey.

**`[FACT]`** The pinned implementation has a substantial safety-oriented local spine:

```text
visible draft -> explicit publication -> readiness query -> fenced attempt
-> durable ownership/workspace -> launch permit -> model process
-> immutable completion candidate -> FLIP/evaluation receipts
-> reviewed publication -> receipt-derived Done -> dependent readiness
```

Unix graph mutation uses advisory locking; lifecycle append/projection separates durable edges from graph views; spawn delays launch until ownership is recorded; model admission is explicit; and ordinary `Done` dispatch consumes publication-derived completion (`src/parser.rs:83-157,275-357`; `src/lifecycle.rs:605-615,1291-1507`; `src/commands/spawn/execution.rs:1283-1438`; `src/main.rs:1261-1274`; `src/commands/completion_done.rs:29-294`). These are E2 facts, not universal runtime or crash-safety proofs.

**`[INFERENCE]` (high confidence)** The dominant systemic defect class is **incomplete authority migration**. New safety mechanisms exist, but old help, manuals, tests, stores, names, and automation claims remain visible. The resulting broken joins—parser versus dispatch, review versus learning, protocol versus lifecycle owner, source versus release selection, historical evidence versus snapshot applicability—explain more of the risk than “missing controls” alone.

### 1.2 Compact state assessment

| Plane | Snapshot-current assessment | Evidence | Confidence |
|---|---|---|---|
| Durable local graph | **Shipped, strongest core; Unix-bounded.** Serialized mutation and replay/fencing are substantial; non-Unix locking, parent-directory durability, and multi-store crash closure are not established. | E2 + bounded inherited E1; `src/parser.rs:83-157,275-357`; [`10`](10-code-architecture.md) | High for source shape; medium for destructive bounds |
| Lifecycle/completion | **Shipped ordinary v3 valve; compatibility and public contracts conflict.** | E2 + candidate-built help in [`30`](30-contradiction-and-drift-register.md); `src/main.rs:1261-1274` | High |
| Worker/model plane | **Multiple real handlers; public capability and ordinary worker topology differ.** Pi accounting/review gaps are snapshot findings, not present-HEAD assertions. | E2 + leaf executions; [`12`](12-model-execution-plane.md) | High static; medium external-provider applicability |
| Agency/evaluation | **Substantial representations; adaptive join incomplete.** Exact candidate review protects completion but does not feed agency evolution at the snapshot. | E2 absence/call-site sample; `src/completion_review.rs:83-121`; `src/agency/evolver.rs:120-224` | High within searched modules |
| Federation/identity | **Real crypto and capability controls; production custody, recovery, and transport ownership partial.** | E2; [`14`](14-federation-identity-security.md) | High static; no security certification |
| Content review | **Enforcing paths exist; audit durability/quorum/default-on claims narrower than prose.** | E2; `src/review/verdict.rs:53-80,117-190`; `src/review/pass2_review.rs:80-98` | High |
| Remote execution/Pilot | **Protocol/CLI and dry-run composition exist; coordinator-owned lifecycle and turnkey real-host operation do not.** | E2; `src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:330-347`; `src/commands/pilot_cmd.rs:1066-1215` | High |
| Testing/release evidence | **Large inventory, selectively activated.** Presence must not be read as pass/selection. | E2/E3; `.github/workflows/ci.yml:68-201`; [`17`](17-testing-ci-quality.md) | High |
| Product/docs/operations | **Fragmented authority.** Strong bounded parity gates exist, but no repository-wide claim/decision/estate/evidence join was found. | E2/E4; [`16`](16-documentation-information-architecture.md); [`31`](31-documentation-sync-plan.md) | High for sampled estate |

### 1.3 Highest-priority snapshot risks, recalibrated after independent review

| Priority | Risk/state | Severity | Likelihood | Confidence | Primary evidence and countercontrol |
|---:|---|---:|---|---|---|
| 1 | **Worker IPC may mutate inbox read state before an array-valued response fails serialization** (`WGDR-049`). | S1 | Observed in the leaf environment; present-day applicability unknown | High for source mechanism; inherited E1 bounded to leaf environment | Flattened response and array results: `src/commands/service/ipc.rs:253-274,720-790`; stateful read: `src/messages.rs:631-696`; exact leaf command/output in [`18`](18-operations-configuration-ux.md), `OPS-001`. |
| 2 | **Completion help, dispatch, and owned-smoke policy describe incompatible authorities** (`WGDR-001/002`). This is a misleading core-workflow/release guarantee, not proof that selected tests fail. | S1 | Possible operator/release reliance | High | Flags declared at `src/cli.rs:528-554`, rejected at `src/main.rs:1261-1274`; smoke claim at `tests/smoke/README.md:3-29`; pinned Done source contains no manifest gate. Positive control: v3 publication-derived completion is explicit. |
| 3 | **Federation custody is same-user/in-process, and recovery is not the promised hostile-worker/offline/time-secure boundary** (`WGDR-029/030`). | S1 | Possible under a same-UID shell-capable adversary | High static; exploit not run | `src/identity/keys.rs:51-68,226-300,340-377`; `src/identity/sigchain.rs:493-515,884-925`. Countercontrol: an optional KEK can encrypt seed material at rest; it does not isolate invocation from the same user/process. |
| 4 | **Inbox list/read/delete and acknowledgement are not recipient-authenticated/owned** (`WGDR-032`). | S1 | Possible for a party that can reach the node | High static; adversarial runtime not rerun | `src/identity/node.rs:408-443,551-572`; `src/identity/transport.rs:318-354,480-496`. Countercontrol: per-inbox count/byte limits and retention bound storage; the residual is unauthorized read/delete/overwrite and **bounded quota consumption**, not unbounded fill. |
| 5 | **Remote placement can be planned but ordinary spawn rejects it; Pilot real-host mode is bootstrap, not the claimed turnkey checked team** (`WGDR-040/042`). | S1 for misleading automation/support guarantee | Possible operator reliance | High | `src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:330-347`; `src/commands/pilot_cmd.rs:43-50,1066-1125,1184-1215`. Countercontrol: Pilot's own CLI honestly says the full check awaits both hosts and records `check_passed: None`; dry-run is explicitly a rehearsal. |
| 6 | **Bare launch, Pi-only setup, and doctor encode different onboarding scopes** (`WGDR-008/009`). | S1 for broad first-run/support misdirection | Possible | High | `src/bin/worksgood.rs:6-16,124-151`; `src/config_defaults.rs:20-107`; `src/commands/setup.rs:1389-1471`; `src/commands/doctor.rs:166-226,241-416`. Positive control: setup-neutral existing-graph launch is deliberate and must not be removed as a “fix.” |

**`[UNCERTAINTY]`** Synchronous completion-review head-of-line blocking is **S2 open uncertainty (`WGDR-U04`)**, not an established S1 snapshot mechanism. An installed daemon exhibited unrelated request timeout while the pinned source has a compatible inline shape, but the binary lacked audited-build identity (`11-orchestration-lifecycle.md:599-656,1103-1145`; `30-contradiction-and-drift-register.md:190-193`). Required check: exact candidate build ID plus slow-review concurrent request scenario.

**`[INFERENCE]` (high confidence)** The following remain important but are better treated as S2 without additional impact evidence: lossy/extension-tolerant config editing, spend/metrics labeling, disconnected agency learning, review-log durability/quorum, selectively activated integration tests, documentation completeness, and terminology drift. High confidence in a static discrepancy is not high likelihood or S1 impact.

### 1.4 Next action and decision

**`[RECOMMENDATION]` (P0, product + lifecycle + security owners)** First contain claims that can authorize unsafe or impossible action; assign named owners; then decide the lifecycle/smoke, custody/recovery, remote/Pilot, and review-audit contracts. Do **not** bulk-rewrite documentation or restore legacy task satellites to gain visibility. The proposed safe architectural direction is to retain v3 publication-derived completion as the sole ordinary source-lifecycle authority, add non-authoritative append-only review-attempt visibility and a separate exactly-once learning projector, and either implement or explicitly reject coordinator-owned remote execution. Acceptance and traceability appear in §§13–14.

---

## 2. Scope, method, and compact system map

### 2.1 Scope and map

**`[FACT]`** The audit covers product binaries, library boundaries, graph/persistence, service/orchestration, model handlers, agency/chat, federation/review/remote execution, TUI/server/channels, Rust/smoke/formal/Pi tests, documentation, packaging, installers, release workflows, and operations as enumerated in [`README.md` §2](README.md#2-scope-and-system-map). The fan-out/fan-in chain is:

```text
README charter
  -> leaves 10..19
  -> syntheses 20..22 + focused cutover 23
  -> contradiction register 30
  -> synchronization roadmap 31
  -> synthesis draft 40
  -> independent review 90
  -> this reviewed final 99
```

**`[FACT]`** Primary product evidence is pinned to `b0892ea7`. Register and roadmap authors demonstrated non-audit byte equivalence to the pinned snapshot at their stated revisions. The independent reviewer sampled thirteen thematic checks directly with `git show` and found eleven supported as written and two supported only with narrowing/counterevidence ([`90` §3.2](90-independent-review.md#32-primary-evidence-spot-checks)). This final independently reran fourteen static byte assertions against the pinned tree; commands are in §16.3.

### 2.2 What was executed versus inspected

**`[FACT]`** Leaf artifacts contain bounded commands and tests; their exact environments/results remain authoritative for those executions. This final did not rerun the product suite. It ran repository/provenance checks only. Therefore domain behavior is phrased as pinned source encoding unless an exact inherited E1 record is named.

**`[UNCERTAINTY]`** No full exact-snapshot Cargo/smoke campaign, real external provider campaign, real two-host Pilot, distinct-UID custody test, destructive power-loss/disk-full/NFS campaign, sustained concurrency/scale test, or exact-archive Windows/macOS runtime campaign was performed by this finalization step.

### 2.3 Coverage map and explicit omissions

These are evidence gaps, not proof of defects.

| Surface | Audit depth | Explicit limit |
|---|---|---|
| TUI, HTML/server, Telegram, Matrix, notifications, accessibility, terminal discoverability | Sampled in operations/concepts/leaves; not broadly synthesized by live human flow | Do not read “human-facing layer” as broadly verified. See [`18`](18-operations-configuration-ux.md) and [`19`](19-conceptual-model-and-vocabulary.md). |
| Formal verification | CI and selected reducer/model boundaries inspected | Lean evidence does not model filesystem, process, Git, network, provider, or operator effects; see [`17`](17-testing-ci-quality.md), `TEST-007`. |
| Casa adapter, `website/`, schemas/examples/templates, terminal benchmark, ancillary scripts | Inventoried and selectively sampled | Product/support status remains decision-required; inventory is not qualification. |
| Performance and scale | No sustained campaign | Locking and smoke inventory do not establish large-graph throughput, latency, disk-pressure, or long-duration stability. |
| Supply chain/dependency security | Release signing and Pi embed staleness sampled | Cargo/npm vulnerability, provenance, compromise response, and end-to-end supply-chain security were not audited. |
| Destructive/cross-platform/external behavior | Mostly source/test inspection | Power loss, disk full, NFS, real credentials/providers, distinct UID, and exact release archives remain unverified. |
| Human synchronization ownership | Roles proposed | No CODEOWNERS-like assignment was found by audit 31; named people/teams are a Phase-0 precondition. |

### 2.4 Method contradictions and drift

**`[CONTRADICTION]`** The draft used alternate labels, omitted per-domain local scope/drift sections, mixed confidence with likelihood, and used “current/shipped” for a historical snapshot. The charter requires exact evidence prefixes, seven-part fractal ordering, separate risk fields, and `snapshot-current` applicability (`README.md:196-327`; [`90`](90-independent-review.md), `IR-003..007`). This final adopts the charter contract and records all review dispositions in §15.

### 2.5 Risks and gaps

**`[INFERENCE]` (high confidence)** The principal audit-use risk is treating this pinned report as a current backlog. Falsifier: a consumer can prove every cited behavior still applies to its candidate revision and environment. Until then, each work item begins with revalidation.

### 2.6 Recommendations

**`[RECOMMENDATION]` (P0, every downstream owner)** Preserve the snapshot hash in issue/decision records; classify evidence E1–E5; retain pass/fail/skip/not-selected separately; and attach a candidate revision before implementing any recommendation.

### 2.7 Evidence appendix

- Normative scope and inventory: [`README.md`](README.md).
- Draft method and source links: [`40-system-synthesis-draft.md`](40-system-synthesis-draft.md).
- Independent check sample and limitations: [`90-independent-review.md`](90-independent-review.md).
- Final command provenance: §16.3.

---

## 3. Top-level state assessment

### 3.1 State filters

| Filter | Snapshot examples | Interpretation |
|---|---|---|
| **Shipped/snapshot-current control** | Unix graph locking; lifecycle generation/fence; v3 completion evidence; fail-closed trust resolution; task-scoped Exec grants | Preserve unless a scoped decision/evidence shows otherwise. |
| **Snapshot-current defect/drift** | Done parser/dispatch mismatch; stateful worker IPC envelope; lossy config; unsigned best-effort review record | Revalidate, then factual containment or implementation. |
| **Partial/manual capability** | review ingest matrix; remote provider CLI; Pilot real-host bootstrap; state loading | Do not call complete, default-on, or turnkey. |
| **Open decision** | cycles/retry, Pi product scope, agency learning semantics, Fed governance, review quorum, remote ownership | Prose cannot settle it; route to §13. |
| **Open uncertainty** | exact candidate head-of-line behavior, crash model, external Pi/provider behavior, cross-platform runtime | Run the bounded falsifying check; do not promote source plausibility to execution. |
| **Accepted debt/non-issue** | route-free init; attended discovery wider than worker admission; offline static keys/no mandatory forward secrecy; separate operational and agency assignment fields | Preserve the deliberate distinction. |
| **Post-snapshot applicability unknown** | every finding affected by the 89-file delta | Never silently close or reopen from this report alone. |

### 3.2 System-level finding

**`[INFERENCE]` (high confidence)** WorksGood is neither “only a prototype” nor “complete production automation.” Its local persistence and exact-candidate completion controls are implemented; its broader planes contain real protocol code and negative controls; but authority, operations, evidence selection, and documentation have not converged on one product contract. That mixed maturity is the central state, not an inconsistency to smooth away.

### 3.3 Contradictions and drift

**`[CONTRADICTION]`** “Complete,” “one trust dial,” “dispatcher wired,” “every verdict,” “turnkey,” and “live smoke” are repeatedly used at a coarser scope than their enforcement. Conversely, some historical spark documents understate later snapshot capabilities. Both directions require applicability metadata rather than age-based rewriting (`WGDR-028..048`, `WGDR-R08/R09`).

### 3.4 Risks and gaps

**`[INFERENCE]` (high confidence)** The authority-migration pattern can create false assurance even where individual components are strong: a signature can coexist with unsafe content; a reviewed candidate can coexist with absent learning; a planned remote route can coexist with rejected spawn; a test can coexist with no CI selection.

### 3.5 Recommendations

**`[RECOMMENDATION]`** Treat “control joins” as the unit of work. Each public/safety claim must join policy, reachable source, executable evidence, support level, owner, and applicable documentation. A missing join is recorded as partial/broken/decision-required, not filled by the newest prose.

### 3.6 Evidence appendix

- Cross-plane synthesis: [`20`](20-core-runtime-synthesis.md), [`21`](21-agency-federation-safety-synthesis.md), [`22`](22-product-docs-quality-synthesis.md), [`23`](23-evaluation-evolvability-cutover.md).
- Deduplicated states: [`30`](30-contradiction-and-drift-register.md), especially §§3–6.

---

## 4. Product identity, architecture, and persistence

### Executive abstract

**`[INFERENCE]` (high confidence)** The durable task/evidence object is WorksGood's coherent center. The architecture is strongest where mutation is serialized and state transitions are append/replay/projection based; it is weakest where one semantic fact is duplicated across graph, lifecycle, registry, completion, Git, and compatibility stores.

### Scope and map

```text
repository/project
  -> WG instance directory
       -> graph.jsonl (tasks/dependencies/projections)
       -> lifecycle/attempt/fence records
       -> service/runtime registry and streams
       -> completion objects/receipts/publication
       -> config/profile + agency/functions + federation/review/provider sidecars
```

Scope: binaries/module boundaries, graph directory resolution, task/dependency representation, locking, atomic replacement, recovery, and cross-store authority. Deep evidence: [`10-code-architecture.md`](10-code-architecture.md), [`20-core-runtime-synthesis.md`](20-core-runtime-synthesis.md), and conceptual audit [`19`](19-conceptual-model-and-vocabulary.md).

### Findings

1. **`[FACT]` — `ARCH-001` positive control.** Unix graph modification takes an exclusive advisory lock; replacement flushes/fsyncs the temporary file before rename (`src/parser.rs:83-157,275-357`). Lifecycle append/projection and torn-frame recovery provide a coherent serialized kernel (`src/lifecycle.rs:1526-1694`; [`10` §3](10-code-architecture.md#3-findings)).
2. **`[FACT]` — `ARCH-003/005/007`.** Dependency edges, completion state, graph status, config layers, and directory resolution have duplicated or distributed authority. `Task.after` drives readiness while `Task.before` is persisted; v3 and legacy completion writers coexist; config/profile fallback has several sources (`src/query.rs:306-517`; `src/commands/completion_done.rs:29-294`; `src/commands/done.rs`; `src/config.rs`).
3. **`[FACT]` — `ARCH-006`.** Non-Unix graph/registry locking methods are no-ops in the sampled implementation. The bespoke replace path does not visibly establish the generic helper's parent-directory fsync behavior (`src/parser.rs:83-157,275-357`; `src/atomic_file.rs:20-142`).
4. **`[FACT]` — `ARCH-008`.** Autopoietic child limits depend on best-effort provenance read/write around graph mutation rather than one transaction (`src/commands/add.rs:438-463,843-865`; `src/provenance.rs:43-117`).
5. **`[FACT]` — `ARCH-009` positive control.** Worker capability interception occurs before graph discovery, a meaningful authority boundary; inherited testing also found that this hard switch can contaminate child-process tests if their environment is not scrubbed (`src/main.rs:734-748`; [`11`](11-orchestration-lifecycle.md), `ORCH-009`).

### Contradictions and drift

- **`[CONTRADICTION]` `WGDR-007/U01`.** The lifecycle ledger calls itself the sole status-edge authority while special v2/legacy writers remain reachable or insufficiently inventoried.
- **`[CONTRADICTION]` `WGDR-T11`.** Manuals can equate graph and project, while critical instance state lies outside graph nodes.
- **`[UNCERTAINTY]` `WGDR-U03/U11`.** Exact power-loss durability, non-Unix concurrency, provenance races, and chat registry lost-update frequency require destructive/concurrent tests.

### Risks and gaps

| Risk | Severity | Likelihood | Confidence | Boundary |
|---|---:|---|---|---|
| Non-Unix or cross-store concurrency/crash loses or strands a projection | S2 | Possible | Medium | Windows/NFS/power-loss unexecuted |
| A fix changes a non-authoritative edge/status/completion copy | S2 | Likely during maintenance | High | Multiple reachable representations |
| Best-effort provenance is treated as a hard child-creation security limit | S2 | Possible | High static | Race/I/O failure not fault-tested |

### Recommendations

1. **`[RECOMMENDATION]` `DEC-12` / P1:** declare the public crash/platform guarantee; promise only tested Unix/process-crash bounds until parent-fsync and cross-platform fault evidence exists.
2. **`[RECOMMENDATION]` P1:** publish canonical status/edge/completion/store authority and compatibility reachability; make reverse edges derived/repairable.
3. **`[RECOMMENDATION]` P1:** move hard child-limit accounting inside serialized graph mutation or rename it advisory.

### Evidence appendix

- Leaf: [`10-code-architecture.md`](10-code-architecture.md), `ARCH-001..009` and its focused commands.
- Synthesis: [`20-core-runtime-synthesis.md`](20-core-runtime-synthesis.md).
- Register: `WGDR-003/007`, `WGDR-T11`, `WGDR-U01/U03/U11` in [`30`](30-contradiction-and-drift-register.md).

---

## 5. Task, orchestration, and completion lifecycle

### Executive abstract

**`[INFERENCE]` (high confidence)** Ordinary v3 completion is a strong immutable valve, but it has not become the single public contract. Manual admission, cycle/retry language, legacy flags, smoke policy, special completion paths, and service review execution still encode different generations.

### Scope and map

```text
draft/pause/time/dependencies
  -> readiness/admission -> claim/generation/attempt/fence
  -> durable ownership/worktree -> process
  -> candidate manifest -> FLIP -> evaluation -> publication -> Done
  -> retry/reopen/recovery or dependent readiness
```

Scope: graph readiness, manual claim, spawn transaction, service concurrency, completion/review/publication, retries, cycles, waits/cron, recovery, and worker messaging. Deep evidence: [`11-orchestration-lifecycle.md`](11-orchestration-lifecycle.md) and [`20`](20-core-runtime-synthesis.md).

### Findings

1. **`[FACT]` — `ORCH-001/002/004/005` positive controls.** Publication and dependency readiness are fail-closed; spawn is rollback-capable and fenced; ordinary completion is immutable-manifest/review/publication derived rather than an exit-code/status button (`src/query.rs:306-517`; `src/commands/spawn/execution.rs:1283-1438`; `src/commands/completion_submit.rs:208-482`; `src/commands/completion_done.rs:29-294`).
2. **`[FACT]` — `ORCH-003`.** Manual claim checks dependency disposition/status but omits pause and scheduled-time admission in its sampled body (`src/commands/claim.rs:18-90`).
3. **`[CONTRADICTION]` — `ORCH-006`, `WGDR-001/002`.** Clap advertises five legacy Done flags; dispatch rejects them. Smoke documentation says owned scenarios gate Done; the ordinary dispatch path does not invoke that manifest gate (`src/cli.rs:528-554`; `src/main.rs:1261-1274`; `tests/smoke/README.md:3-29`).
4. **`[FACT]` — `ORCH-007/008/011`.** Cycle reactivation, Abandoned retry, and legacy/snapshot-current lifecycle representations remain semantically split. Source can record a reopen request without immediately projecting Open, and retry accepts a status help/tests call permanent (`src/graph.rs:3044-3567`; `src/commands/reopen.rs:236-328`; `src/commands/retry.rs:215-235`).
5. **`[FACT]` — `ORCH-010`.** Service concurrency is bounded and mostly fail-stop. **`[UNCERTAINTY]` — `ORCH-014/WGDR-U04`.** Exact-snapshot daemon-wide review blocking was not verified; installed-binary behavior and source plausibility must remain separate.
6. **`[FACT]` — `WGDR-049`.** The IPC envelope flattens `data`, message/artifact operations return arrays, and unread-message state is written before response delivery (`src/commands/service/ipc.rs:253-274,720-790`; `src/messages.rs:631-696`). The operations leaf records the bounded live failure.

### Contradictions and drift

- `WGDR-001`: help versus publication-derived dispatch — open S1.
- `WGDR-002`: claimed owned-smoke completion invariant versus disconnected source/test selection — open S1.
- `WGDR-003`: manuals teach an obsolete status/dependency/completion model — open S2.
- `WGDR-004`: manual claim says it shares admission disposition but omits gates — open S2.
- `WGDR-005/006`: cycle and Abandoned semantics are product decisions, not copy edits.
- `WGDR-R11/R12`: preserve worker interception; do not claim cycles absent or fully supported.

### Risks and gaps

| Risk | Severity | Likelihood | Confidence | Evidence boundary |
|---|---:|---|---|---|
| Worker loses usable unread-message response after state mutation | S1 | Observed in leaf environment | High mechanism | Present-day candidate untested |
| Operators/releases rely on a nonexistent legacy Done/smoke contract | S1 | Possible | High conflict | No inference that all smoke tests fail |
| Slow review stalls unrelated work at the exact snapshot | S2 | Unknown | Medium | Installed binary lacked build identity |
| Draft/future work can be manually claimed without explicit override semantics | S2 | Possible | High static | Leaf executed bounded cases |
| Cycle/retry recovery behaves contrary to operator expectation | S2 | Possible | Medium-high | Desired policy unelected |

### Recommendations

1. **`[RECOMMENDATION]` `DEC-01` / P0:** keep v3 publication-derived completion sole ordinary authority; decide whether smoke binds publication, Done, or neither; remove/restore parser, dispatch, worker, tests, and agent contract atomically.
2. **`[RECOMMENDATION]` `DEC-02` / P0:** elect cycles, Abandoned retry, and manual-claim override semantics with real worker/operator flows.
3. **`[RECOMMENDATION]` P0 implementation:** make IPC response `{ok,data}` non-flattened and stateful read/response replay-safe before acknowledging mutation.
4. **`[RECOMMENDATION]` P1 verification:** run candidate-built slow-review concurrency with build ID before changing service architecture on `WGDR-U04`.

### Evidence appendix

- Leaf: [`11-orchestration-lifecycle.md`](11-orchestration-lifecycle.md), especially `ORCH-001..017` and §7 command records.
- Register: [`30` §3.1 and `WGDR-049/U04`](30-contradiction-and-drift-register.md).
- Draft source crosswalk: [`40` §5](40-system-synthesis-draft.md#5-task-and-orchestration-lifecycle).

---

## 6. Model execution, configuration, accounting, and operations

### Executive abstract

**`[INFERENCE]` (high confidence)** Model routing and worker admission contain strong explicit controls, but capability is surface-dependent. Onboarding, Pi worker/plugin topology, config mutation, accounting time/scope, packaging, and platform support do not form one reliable operator contract at the snapshot.

### Scope and map

```text
setup/profile/config -> handler-first route + reasoning
  -> strict unattended admission -> executor/worker argv
  -> raw stream -> canonical events -> usage/cost -> spend/show/stats
operator plane: worksgood launcher + doctor + service/status + config + upgrade/package
```

Scope: route grammar, handlers, Pi/plugin/watchdog/streams, config/profile/setup/doctor, accounting, service UX, HTML publication, install/upgrade/package/platform. Deep evidence: [`12-model-execution-plane.md`](12-model-execution-plane.md), [`18-operations-configuration-ux.md`](18-operations-configuration-ux.md), and [`20`](20-core-runtime-synthesis.md).

### Findings

1. **`[FACT]` — `MODEL-005/006/007` positive controls.** Explicit route/reasoning propagation, fail-closed worker admission, Pi turn-end usage deduplication, and evidence-based watchdog continuation are encoded controls (`src/dispatch/handler_for_model.rs`; `src/stream_event.rs`; `src/pi_watchdog/mod.rs`). The watchdog is not completion authority.
2. **`[FACT]` — `MODEL-001`.** Discovery/templates expose more handler kinds than strict unattended worker admission supports. That is a surface distinction, not inherently a defect (`src/executor_discovery.rs:40-188`; `src/config.rs:2395-2433`; `WGDR-R02`).
3. **`[FACT]` — `MODEL-002`.** Ordinary Pi JSON workers and the hermetic RPC handler construct different argv; ordinary worker source lacks the documented invocation-scoped `-e/-ne` plugin boundary (`src/service/executor.rs:1729-1752`; `src/commands/pi_handler.rs:492-537`). External-provider consequence remains `WGDR-U05`.
4. **`[FACT]` — `MODEL-008/009`, snapshot-current.** Spend groups records under invocation day; cleanup metrics are process-local; the sampled v3 review/commit path omitted usage propagation (`src/commands/spend.rs:27-57`; `src/metrics.rs:1-29`; completion-module search in [`90` §3.2](90-independent-review.md#32-primary-evidence-spot-checks)). Post-snapshot Pi accounting/review work makes present-day applicability unknown.
5. **`[FACT]` — `OPS-002/003`.** Generic config set pretty-serializes parsed TOML, erasing comments. Arbitrary dotted paths are deliberately accepted to expose extension knobs, but ineffective unknown keys can remain lint-clean (`src/commands/config_cmd.rs:3029-3098`). The defect is not simply “unknown keys allowed”; policy must preserve intentional extension space.
6. **`[CONTRADICTION]` — `WGDR-008/009`.** Existing-graph bare launch is setup-neutral, setup presents Pi, and doctor diagnoses Claude as required in sampled branches (`src/bin/worksgood.rs:6-16,124-151`; `src/config_defaults.rs:20-107`; `src/commands/setup.rs:1389-1471`; `src/commands/doctor.rs:166-226,241-416`).
7. **`[FACT]` — `OPS-012` positive control.** Service status/cleanup defaults and conservative destructive behavior are comparatively strong; install safety also has bounded positive controls ([`18`](18-operations-configuration-ux.md), `OPS-010/012`).

### Contradictions and drift

- `WGDR-008/009`: launcher/setup/doctor route scope.
- `WGDR-010/011`: “preserves everything,” extension intent, unknown keys, and lint remedy.
- `WGDR-012`: “daily” spend and system-looking metrics versus invocation/process scope.
- `WGDR-013..018`: Casa packaging, discoverable versus worker-ready handlers, Pi hermeticity, provider-leading transition, fallback, and upgrade scope.
- `WGDR-U05/U07/U10`: real provider schema/auth, CI toolchain resolution, and exact-archive cross-platform runtime remain unverified.

### Risks and gaps

| Risk | Severity | Likelihood | Confidence | Counterevidence/boundary |
|---|---:|---|---|---|
| Existing-graph/Pi/doctor prose causes wrong setup/readiness action | S1 | Possible | High | Route-free existing-graph launch is deliberate. |
| Config edit destroys comments or preserves ineffective typo | S2 | Likely for users of generic setter | High | Unknown-path acceptance is intentional extensibility; namespace policy unresolved. |
| Spend/metrics drive wrong budget/capacity interpretation | S2 | Possible | High | Values are not necessarily numerically wrong; date/scope label is. |
| Ambient Pi plugin/provider behavior differs from claimed hermetic path | S2 | Unknown | Medium | No external credential/schema run. |
| Package/platform claims exceed tested artifacts | S2 | Possible | High gap confidence | Builds do not equal exact-archive runtime support. |

### Recommendations

1. **`[RECOMMENDATION]` `DEC-03/04` / P0–P1:** decide Pi scope, unattended handlers, binaries, install modes, and platforms; generate a surface capability matrix.
2. **`[RECOMMENDATION]` P0:** make config editing lossless and schema-aware after deciding an explicit extension namespace; do not “fix” by banning intended extension knobs.
3. **`[RECOMMENDATION]` P1:** persist event timestamps/metrics or rename outputs to invocation-day/process-local; recheck post-snapshot accounting before implementation.
4. **`[RECOMMENDATION]` P1 verification:** capture actual child argv/env with isolated credentials and run exact release archives on declared platforms.

### Evidence appendix

- Model leaf: [`12-model-execution-plane.md`](12-model-execution-plane.md), `MODEL-001..010`.
- Operations leaf: [`18-operations-configuration-ux.md`](18-operations-configuration-ux.md), `OPS-001..014`.
- Register: [`30` §3.2](30-contradiction-and-drift-register.md#32-installation-configuration-model-plane-accounting-and-packaging).

---

## 7. Agency, evaluation, functions, chat, and evolvability

### Executive abstract

**`[INFERENCE]` (high confidence)** WorksGood has concrete agency compositions, exact candidate review, function/trace machinery, and attended/runtime authority boundaries. The snapshot's adaptive loop is incomplete: completion review is not the agency performance store, review attempts/cost are thin, assignment outcome has no durable join, and function apply history does not cleanly feed adaptive summaries.

### Scope and map

```text
agency composition (role/motivation/tradeoffs)
  -> task assignment metadata -> runtime attempt/worker
  -> immutable candidate -> completion review/evaluation -> lifecycle result
  -X-> agency performance/evolver at snapshot
functions: template/schema -> apply/planner -> tasks -> run history -> adaptation
chat/human: attended binding -> durable histories/summary -> worker context
```

Scope: agency identity/hashing, assignment, completion/candidate/performance evaluation, evolver, trace functions, chat histories/summary, concierge/human onboarding. Deep evidence: [`13-agency-evaluation-chat.md`](13-agency-evaluation-chat.md), [`21`](21-agency-federation-safety-synthesis.md), and focused [`23`](23-evaluation-evolvability-cutover.md).

### Findings

1. **`[FACT]` — `EVAL-001`, `EVC-001/002` positive controls.** Candidate-bound completion review has strong immutable provenance/replay properties and no longer gives evaluator satellites source-lifecycle authority (`src/completion_review.rs:83-121`; `src/evaluation/mod.rs:112-235`).
2. **`[FACT]` — `AGENCY-004/EVC-005`.** In the searched completion modules, no call fed accepted modern review into `agency/evaluations/*.json`; the evolver reads that legacy performance store (`src/agency/eval.rs:49-201`; `src/agency/evolver.rs:120-224`; scoped call-site command in [`90` §7.2](90-independent-review.md#72-exact-command-log)). This is a bounded absence claim, not proof no generated/indirect path exists elsewhere.
3. **`[FACT]` — `EVC-003/004`.** Compact receipts bind verdict/model/time/object identity but omit attempt lineage, latency, source composition, and usage; reviewer call results discard usage in the sampled path (`src/completion_review.rs:83-95`; `src/completion_review_model.rs:58-88`).
4. **`[FACT]` — `AGENCY-001/002/003`.** Persona composition is concrete, but access metadata is descriptive; documented hash/immutability equations differ from source; `auto_assign` is surfaced without the promised production LLM reachability (`src/agency/hash.rs:15-67`; `src/commands/role.rs:224-273`; `src/commands/assign.rs:205-393`).
5. **`[FACT]` — `FUNC-001/002`.** Generative function schema exists, but planner execution is externally staged; apply rows and adaptive `RunSummary` readers use incompatible shapes (`src/commands/func_apply.rs:435-451,612-726`; `src/function.rs:298-346`).
6. **`[FACT]` — `CHAT-001/HUMAN-001` positive controls.** Attended/runtime authority and confirmed binding are explicit. **`[FACT]` — `CHAT-002/003/HUMAN-002`.** Multiple histories/concurrency signals remain, a bound summary can enter worker prompt without federated-style provenance review, and onboarding writes several stores without one transaction (`src/chat_sessions.rs`; `src/commands/agency_human.rs:126-214`).

### Contradictions and drift

- `WGDR-019`: documented agency hash/immutability versus code.
- `WGDR-020`: automatic LLM assignment claim versus manual/dormant paths.
- `WGDR-021/022/023`: completion review, attempt observability, performance learning, and assignment reward are distinct but often described as one evaluation loop.
- `WGDR-024/025`: function planner/apply/adaptive history contract.
- `WGDR-027`: chat canonical history versus compatibility/runtime representations.
- `WGDR-R10`: completion review and agency performance evaluation are correctly separate authorities; do not merge them into lifecycle-coupled synthetic tasks.

### Risks and gaps

| Risk | Severity | Likelihood | Confidence | Boundary |
|---|---:|---|---|---|
| Universal quality gate is mistaken for agency learning | S2 | Likely documentation/analytics misreading | High | Learning quality/production frequency not measured |
| Review cost/failure trajectory cannot be reconstructed | S2 | Likely when auditing attempts | High | Raw provider logs may contain partial evidence |
| Persona/hash/assignment identity credits wrong actor/composition | S2 | Possible | High static | Binding policy unelected |
| Local summaries/adaptive memory bypass inbound-content trust discipline | S2 | Possible | Medium-high | Exploitability not run |
| Restoring evaluator tasks reintroduces lifecycle coupling | S1 architecture regression | Possible if chosen as fix | High reasoning | Positive v3 valve must be preserved |

### Recommendations

1. **`[RECOMMENDATION]` `DEC-05` / P0:** retain v3 completion as sole lifecycle consumer; add append-only `ReviewRun/Attempt` events and non-schedulable projections; separately project exactly-once learning with no complete/retry/reopen/publish authority.
2. **`[RECOMMENDATION]` `DEC-06` / P0:** define signed/authorized bindings among agency composition, runtime worker, federated principal, and human classification; define credit and anti-gaming semantics.
3. **`[RECOMMENDATION]` P1:** version/unify function apply/run summary schemas and make fallback/staged planning explicit.
4. **`[RECOMMENDATION]` P1:** make human onboarding restartable/transactional and route prompt-bound local summaries through appropriate provenance/safety policy.

### Evidence appendix

- Leaf: [`13-agency-evaluation-chat.md`](13-agency-evaluation-chat.md), `AGENCY/EVAL/FUNC/CHAT/CONTEXT/CONCIERGE/HUMAN` findings.
- Cross-plane: [`21-agency-federation-safety-synthesis.md`](21-agency-federation-safety-synthesis.md), `XAUTH-001..010`.
- Cutover: [`23-evaluation-evolvability-cutover.md`](23-evaluation-evolvability-cutover.md), `EVC-001..008` and proposed acceptance tests.

---

## 8. Federation, trust, review, remote execution, and Pilot

### Executive abstract

**`[INFERENCE]` (high confidence)** These planes contain real cryptography, scoped capabilities, content gates, leases, and signed protocol objects. Their major risks are at composition and ownership boundaries: same-user custody, recovery freshness, unauthenticated inbox operations, best-effort review audit, partial trust/bypass matrices, manual remote lifecycle, and product prose that calls a protocol or rehearsal turnkey.

### Scope and map

```text
wgid + sigchain + custodian
  -> signed/sealed message via file/HTTP store
  -> authentication -> trust resolution -> content review -> consumption
  -> provider placement/claim/grant (scoped UCANs) -> lease epoch
  -> result signature/accept/re-run -> graph finalization
Pilot = orchestration wrapper around selected identity/message/review/provider CLI flows
```

Scope: identity, key custody/recovery, envelope/transport/freshness, trust composition, inbound review, provider bundles/capabilities/leases/results, coordinator seam, and Pilot. Deep evidence: [`14-federation-identity-security.md`](14-federation-identity-security.md), [`15-review-exec-pilot.md`](15-review-exec-pilot.md), and [`21`](21-agency-federation-safety-synthesis.md).

### Findings

1. **`[FACT]` — `FED-001/011` positive controls.** Self-certifying identity, root-locked sigchain operations, attenuating capabilities, task-scoped Exec grants, sealed-wrap access, and lease-epoch fencing are real encoded controls (`src/identity/sigchain.rs`; `src/identity/custody.rs`; `src/identity/envelope.rs`; `src/providers/lease.rs`).
2. **`[FACT]` — `FED-003/004`.** `Custodian::sign_digest` loads same-user secret material in process; an optional KEK protects at rest but is not process/UID isolation. Recovery checks signer-carried timing and co-locates the backstop in sampled CLI flows (`src/identity/keys.rs:51-68,226-300,340-377`; `src/identity/sigchain.rs:493-515,884-925`; `src/commands/identity_cmd.rs:253-322`).
3. **`[FACT]` — `FED-006`.** Inbox operations lack recipient-auth input. Count/byte quotas and retention bound storage; they do not prevent unauthorized read/delete/overwrite or bounded quota consumption (`src/identity/node.rs:408-443,551-572`; `src/identity/transport.rs:318-354,480-496`).
4. **`[FACT]` — `RXP-001/002`.** Authentication, author trust, provider trust, review depth, capability, and candidate acceptance are separate inputs/gates. Author trust fails closed and provider opinion can lower it; “one trust dial” is an oversimplification (`src/trust.rs:79-125`; `src/review/depth.rs`; `src/providers/placement.rs`; `WGDR-R04`).
5. **`[FACT]` — `RXP-003/004`.** Review enforcement exists, and its record read-modify-write is lock-protected and atomically replaced. The `VerdictRecord` is not signed, load does not revalidate the whole claimed chain in the sampled span, live callers can ignore recording failure, and deterministic reviewer `n` is ignored (`src/review/verdict.rs:53-80,117-190`; `src/review/pass2_review.rs:80-98`; ignored caller results listed in [`90` §3.2](90-independent-review.md#32-primary-evidence-spot-checks)).
6. **`[FACT]` — `RXP-005/007` positive controls.** Provider accept has coherent signature/lease/canonical-write checks and durable epoch fencing. **`[FACT]` — `RXP-006/008/009`.** Planner selection does not produce coordinator-owned spawn; lease renewal/sweep and multi-step accept/finalize remain manual or can strand post-fence work (`src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:330-347`; `src/commands/exec_fed_cmd.rs:951-979,1237-1384`).
7. **`[FACT]` — `RXP-010/011`.** Pilot validates safe defaults before startup, but real-host mode records `check_passed: None`, starts a bootstrap rather than a complete agent/lease lifecycle, and dry-run uses a fixed worker (`src/commands/pilot_cmd.rs:43-50,1066-1125,1184-1215`). Countercontrol: the real CLI tells operators the full check requires both hosts.

### Contradictions and drift

- `WGDR-028`: Proposed Fed governance versus implemented waves—tests do not ratify ADRs.
- `WGDR-029..036`: custody, recovery, compatibility, transport, ACL wording, state load, historical signatures, and topology independence.
- `WGDR-037..039`: “signed sigchain/every verdict,” quorum, and four default-on ingest seams versus narrower source.
- `WGDR-040..042`: “dispatcher wired,” automatic lease/result lifecycle, and turnkey Pilot versus manual/bootstrap source.
- `WGDR-R06..R09`: preserve disclosed no-offline-forward-secrecy debt, task-scoped grants, historical spark applicability, and immutable old audit evidence.

### Risks and gaps

| Risk | Severity | Likelihood | Confidence | Countercontrol/unverified boundary |
|---|---:|---|---|---|
| Same-UID worker collapses claimed custody/recovery boundary | S1 | Possible | High static | Optional at-rest KEK; distinct-UID/exploit not run |
| Reachable node permits unauthorized inbox operations | S1 | Possible | High static | Count/byte/retention bounds; adversarial runtime not rerun |
| Operator believes remote/Pilot lifecycle is owned and live-checked | S1 | Possible | High | CLI caveat is honest; prose/product naming is the overclaim |
| Consumed/rejected content lacks durable trustworthy review audit | S2 | Possible | High | Lock + atomic write reduce torn/lost updates |
| Trust/review bypass or hand-passed trust is mistaken for default-on safety | S2 | Possible | High | Entry-point matrix incomplete |
| Signature is mistaken for content safety or remote silicon provenance | S2 | Possible | High | Separate review and disjoint rerun exist in bounded paths |

### Recommendations

1. **`[RECOMMENDATION]` `DEC-07` / P0:** either mark federation custody experimental/same-user or deploy a separate authenticated, purpose-bound signer; bind recovery to current head/challenge/verifier time/one-use.
2. **`[RECOMMENDATION]` P0:** authenticate inbox ownership, make insertion immutable/id-bound, and add owned ack/cursor; preserve quotas/retention.
3. **`[RECOMMENDATION]` `DEC-08` / P0:** decide required versus best-effort review audit, escalation versus independent quorum, provenance fields, and allowed bypasses; make claim wording match enforcement.
4. **`[RECOMMENDATION]` `DEC-09` / P0:** either build restart-safe coordinator-owned remote lifecycle and real two-host Pilot check, or reject automatic/turnkey interpretation in admission/help/runbook.
5. **`[RECOMMENDATION]` P1:** keep authentication, trust opinion, capability, content acceptance, candidate acceptance, and completion distinct in schemas/docs.

### Evidence appendix

- Federation leaf: [`14-federation-identity-security.md`](14-federation-identity-security.md), `FED-001..014`.
- Review/Exec/Pilot leaf: [`15-review-exec-pilot.md`](15-review-exec-pilot.md), `RXP-001..011` and drift records.
- Typed authority synthesis: [`21-agency-federation-safety-synthesis.md`](21-agency-federation-safety-synthesis.md), `XAUTH-001..010`.

---

## 9. Testing, CI, release, and human-facing evidence

### Executive abstract

**`[INFERENCE]` (high confidence)** WorksGood has a large and diverse executable-evidence estate, plus meaningful formal and source/embed controls. Its central quality problem is selection and classification: file/manifest presence, compilation, fixture/static smoke, exact runtime execution, CI selection, and release qualification are not consistently distinguished.

### Scope and map

```text
inline/unit + 176 top-level integration targets
  + 324 smoke manifest entries/scenarios
  + install/upgrade + formal Lean + Pi package
  -> selected CI lanes -> release build/sign/package
  -> actual human/provider/platform journey (only when explicitly run)
```

Scope: Rust tests, smoke owner/gate policy, skips/fixtures/static contracts, CI selection, formal scope, Pi embed gate, release target/signing/package, and human-facing TUI/browser/channel/install flows. Deep evidence: [`17-testing-ci-quality.md`](17-testing-ci-quality.md), [`18`](18-operations-configuration-ux.md), and [`22`](22-product-docs-quality-synthesis.md).

### Findings

1. **`[FACT]` — `TEST-006/007` positive controls.** Pi source-to-embedded regenerate/diff is a clear declared anti-drift gate, and formal checks are explicitly bounded to selected reducers/models (`.github/workflows/ci.yml:82-125,174-201`; `worksgood-pi/`; `formal/`).
2. **`[FACT]` — `TEST-001/002`.** At the snapshot, CI selected library, formal, selected binary/canary, Pi, and `integration_service` paths but not the full top-level integration estate. The pinned inventory counted 176 top-level Rust targets; independent review confirmed the count. “Not selected” does not mean failing (`.github/workflows/ci.yml:68-201`; [`90` §3.2](90-independent-review.md#32-primary-evidence-spot-checks)).
3. **`[CONTRADICTION]` — `TEST-DRIFT-001/002`.** Smoke policy says Done is owner-gated while publication-derived dispatch does not call the gate and the permanent integration target asserts a retired completion generation (`tests/smoke/README.md:3-29`; `tests/integration_smoke_gate.rs:1-11,131-412`; `src/main.rs:1261-1274`).
4. **`[FACT]` — `TEST-DRIFT-004`.** The smoke corpus includes protocol-live, deterministic fixture, compile-only, and static release-contract scenarios despite categorical “live, not stubs” policy. Each can be useful; the class must travel with the result (`tests/smoke/README.md:82-87`; `tests/smoke/scenarios/release_workflow_signing_contract.sh:1-18`; Pilot source).
5. **`[UNCERTAINTY]`** Human interaction surfaces were not broadly run by this synthesis. TUI, HTML/browser, Telegram/Matrix, notifications, accessibility, terminal discovery, installers, and real provider flows cannot inherit confidence from CLI/library tests.
6. **`[FACT]` — `TEST-005/008`.** Release construction/signing is stronger than release qualification; exact toolchain precedence, feature/MSRV matrix, and Windows/macOS runtime remain incompletely established (`.github/workflows/release.yml`; `rust-toolchain.toml`; `WGDR-U07/U10`).

### Contradictions and drift

- `WGDR-002`: smoke as completion invariant versus pinned dispatch.
- `WGDR-044`: binary test blind-spot scenario partly superseded by filtered binary-test compilation.
- `WGDR-045`: categorical live-smoke claim versus mixed evidence classes.
- `WGDR-046`: helper/cleanup policy versus corpus exceptions, still requiring per-file adjudication.
- `WGDR-U07/U10`: toolchain and exact-archive platform runtime.

### Risks and gaps

| Risk | Severity | Likelihood | Confidence | Boundary |
|---|---:|---|---|---|
| “Green CI/smoke present” is treated as broad user/release pass | S1 if used as release safety guarantee | Possible | High inventory/selection evidence | No assertion that unselected targets fail |
| Fixture/static checks are mistaken for live protocol/model/host evidence | S2 | Likely without classification | High | These tests remain valuable within class |
| Unsupported human/platform flow escapes selected lanes | S2 | Possible | High gap confidence | No broad exact-archive/human campaign |
| Formal theorem is projected onto unmodeled filesystem/network/operator effects | S2 | Possible | High | Formal scope is itself honestly bounded |

### Recommendations

1. **`[RECOMMENDATION]` P0:** decide the smoke/completion authority under `DEC-01`; until then, narrow the guarantee.
2. **`[RECOMMENDATION]` P1:** classify every target/scenario as selected/not-selected and protocol-live/fixture/static/credentialed/multi-host; preserve skips and zero-assertion failures.
3. **`[RECOMMENDATION]` P1:** add actual release-binary human flows for supported TUI/terminal/browser/install/platform journeys; do not substitute library calls.
4. **`[RECOMMENDATION]` P1:** state theorem-to-Rust boundaries beside formal claims and print/assert exact CI compiler identity.

### Evidence appendix

- Leaf: [`17-testing-ci-quality.md`](17-testing-ci-quality.md), `TEST-001..011` and `TEST-DRIFT-001..007`.
- Product synthesis: [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md), quality/evidence map.
- Charter inventory: [`README.md` §7](README.md#7-evidence-appendix).

---

## 10. Documentation and conceptual coherence

### Executive abstract

**`[INFERENCE]` (high confidence)** Documentation drift is the visible consequence of authority multiplication. Curated indexes, manual source claims, command reference, designs/ADRs, reports, agent guides, website copies, and source behavior each answer different questions without a machine-readable join. Bounded parity controls show the problem is tractable when source and derivative are explicit.

### Scope and map

```text
policy/ADR + reachable source + executable evidence + support decision
  -> product contract/claim state
  -> generated reference + authored journeys/runbooks
historical reports/designs --immutable body--> applicability/supersession index
estate manifest -> curated routers (not vice versa)
```

Scope: docs inventory/indexes, manual/website source graph, command/config/reference, design/ADR/report status, vocabulary, root organization, link integrity, generated/embedded guides, and target synchronization architecture. Deep evidence: [`16-documentation-information-architecture.md`](16-documentation-information-architecture.md), [`19`](19-conceptual-model-and-vocabulary.md), [`22`](22-product-docs-quality-synthesis.md), and [`31`](31-documentation-sync-plan.md).

### Findings

1. **`[FACT]` — `DOC-001/003/009`.** `KEY_DOCS` is curated and aging, while document status/applicability and report supersession are often implicit. It cannot safely serve as both complete estate inventory and reader router (`docs/KEY_DOCS.md:1-16`; [`16`](16-documentation-information-architecture.md)).
2. **`[FACT]` — `DOC-004`.** Manual README and sync script describe an ambiguous source graph; converter failure can copy raw Typst into `.md` (`docs/manual/README.md:30-42`; `scripts/sync-docs.sh:1-8,66-118`).
3. **`[CONTRADICTION]` — `DOC-002/005`.** Setup journeys and “complete” command reference disagree with source/dispatch/support; parser help itself can be unreachable. A generated parser inventory alone is insufficient (`docs/COMMANDS.md`; `src/cli.rs:528-554`; `src/main.rs:1261-1274`).
4. **`[FACT]` — `DOC-008` positive control.** Agent guide embedding and root guide parity are explicit/tested source relationships (`src/commands/agent_guide.rs:3-15,132-185`). They should be preserved, not deduplicated blindly.
5. **`[FACT]` — `CONCEPT-001/002/007/008` positive controls.** Task/execution object hierarchy and distinctions among authentication, trust, authority, review, execution, and Pilot ownership are recoverable from types. **`[FACT]` — `CONCEPT-003..010`.** Public vocabulary and schemas do not consistently preserve those namespaces.
6. **`[FACT]`** Audit 31 found no `docs/manifest.toml`, `docs/product-contract.toml`, machine-readable decision index, glossary, or CODEOWNERS-like assignment at its byte-equivalent planning checkout. These are recommended records, not hidden existing systems ([`31` §2.3](31-documentation-sync-plan.md#23-canonical-registries-and-generated-views)).

### Contradictions and drift

- `WGDR-043`: estate/source graph/command/assets ambiguity.
- `WGDR-047/048`: “Proposed/design only” and historical compatibility values versus later source; historical bodies remain valid for their revision.
- `WGDR-T01..T12`: agent, role, provider, handler/executor, coordinator, evaluation/review, identity/trust, assigned/claimed, terminal/satisfied, publish/run/function/trace/replay, graph/project/instance, spark/wave/shipped.
- `WGDR-R03/R08/R09`: preserve root-guide parity and immutable historical reports; add external applicability edges.

### Risks and gaps

| Risk | Severity | Likelihood | Confidence | Boundary |
|---|---:|---|---|---|
| New prose becomes another authority selected by recency | S2 | Likely without registries | High | No repository-wide join located |
| Cross-plane vocabulary causes wrong authorization/accounting/support inference | S2 | Possible | High | Typed distinctions exist as countercontrol |
| Generator publishes malformed or divergent derivative | S2 | Possible | High source evidence | Generator not run by final |
| Bulk IA move breaks links/tool-required discovery/history | S2 | Likely if attempted without manifest | High | Git history alone is not user rollback |

### Recommendations

1. **`[RECOMMENDATION]` P0:** contain false high-impact claims before mass rewrite; label partial/broken/decision-required.
2. **`[RECOMMENDATION]` `DEC-10/11` / P1:** elect one source DAG and namespaced glossary; fail closed on conversion.
3. **`[RECOMMENDATION]` P1:** keep separate estate manifest, decision index, product-contract claim registry, glossary, and generated evidence index. A curated router is not the estate manifest.
4. **`[RECOMMENDATION]` P1–P2:** preserve historical bytes and add applicability/supersession/errata sidecars before moves/archive.

### Evidence appendix

- Documentation leaf: [`16-documentation-information-architecture.md`](16-documentation-information-architecture.md), `DOC-001..010`.
- Concept leaf: [`19-conceptual-model-and-vocabulary.md`](19-conceptual-model-and-vocabulary.md), `CONCEPT-001..010`.
- Product synthesis: [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md), `PRODUCT-001..008`.
- Roadmap: [`31-documentation-sync-plan.md`](31-documentation-sync-plan.md).

---

## 11. Cross-cutting findings

### 11.1 `X-01` — authority migration, not component absence

**`[INFERENCE]` (high confidence)** The same pattern appears in lifecycle, routing, evaluation, federation governance, remote execution, tests, and docs: a newer safer representation lands while old discovery, prose, test, or compatibility authority remains reachable. Falsifier: an end-to-end authority matrix shows one elected source for each scoped question and classifies all other representations.

### 11.2 `X-02` — exact-object binding is WorksGood's strongest reusable pattern

**`[FACT]`** Generation/attempt/fence, content CIDs, candidate manifests, review receipts, scoped capabilities, lease epochs, and signed envelopes all bind decisions to named immutable objects. This is stronger than mutable status or actor assertion (`src/lifecycle.rs`; `src/completion_review.rs`; `src/identity/custody.rs`; `src/providers/lease.rs`).

**`[RECOMMENDATION]`** Extend this pattern to review attempts, learning projection, documentation claims, test receipts, and remote accept/finalize recovery—without granting those projections lifecycle authority.

### 11.3 `X-03` — typed authority vector, not one identity/trust scalar

**`[FACT]`** Agency composition, runtime worker, federated principal, model route, compute provider, human classification, author/provider trust opinions, capability, content verdict, candidate verdict, and completion disposition are separate typed questions ([`21`](21-agency-federation-safety-synthesis.md), `XAUTH-001..010`).

**`[INFERENCE]` (high confidence)** Collapsing them into “agent identity” or “one trust dial” can authorize or credit the wrong subject. The remedy is explicit bindings and qualified terms, not one universal ID/score.

### 11.4 `X-04` — enforcement is stronger than audit/learning/operations

**`[FACT]`** Candidate review, content blocking, capability attenuation, and lease fencing have enforcement sites. Review recording can be best-effort; candidate review usage/attempt detail is thin; learning is disconnected; remote renew/accept/finalize is not owned as one transaction.

**`[INFERENCE]` (high confidence)** WorksGood often prevents an immediate unsafe transition better than it explains, attributes, learns from, or recovers that transition. This is a systems observability/reconciliation problem, not evidence that enforcement is absent.

### 11.5 `X-05` — evidence activation matters more than inventory size

**`[FACT]`** The snapshot inventory counted 176 top-level Rust integration targets and 324 smoke entries, while ordinary CI selected a bounded subset. Formal and Pi embed controls are strong within their scope. **`[INFERENCE]`** Assurance must report exact selection/class/result rather than file counts.

### 11.6 Cross-cutting risks and recommendations

| Finding | Risk | Severity | Likelihood | Recommendation |
|---|---|---:|---|---|
| `X-01` | Fix lands in wrong authority or docs ratify accidental behavior | S2 | Likely | Authority/reachability matrices + decisions |
| `X-02` | New mutable projection becomes lifecycle authority | S1 regression | Possible | Immutable event + read-only/exactly-once projector boundaries |
| `X-03` | Wrong identity/trust subject is authorized or credited | S1 at security boundary | Possible | Signed binding records + namespaced glossary |
| `X-04` | Enforcement outcome cannot be audited/recovered/learned | S2 | Possible | Durable attempt/audit/reconciliation records |
| `X-05` | Release claim exceeds selected evidence | S1 if safety guarantee | Possible | Machine-visible selection/evidence classes |

---

## 12. Contradiction, drift, and uncertainty summary

### 12.1 Register shape

**`[FACT]`** [`30-contradiction-and-drift-register.md`](30-contradiction-and-drift-register.md) deduplicates upstream findings into **49 `WGDR-*` open records**, **12 terminology collisions**, **12 resolved/narrowed safeguards**, and **12 explicit uncertainties**. Its stable IDs remain the detailed authority; this section does not replace row metadata.

### 12.2 Material open clusters

| Cluster | IDs | Final disposition |
|---|---|---|
| Completion/lifecycle/smoke | `WGDR-001..007`, `U01/U04` | Keep v3 ordinary authority; `001/002` S1; HOL restored to S2 uncertainty; human decisions required. |
| Launcher/model/config/accounting/package | `WGDR-008..018`, `U05/U07/U10` | Contain setup claims; config/accounting generally S2 after recalibration; preserve intentional discovery/extensions. |
| Agency/evaluation/functions/chat | `WGDR-019..027`, `U08/U11` | Learning disconnect S2; do not restore lifecycle-coupled evaluator tasks; decide representation/credit. |
| Federation/review/Exec/Pilot | `WGDR-028..042`, `U09/U12` | Custody/inbox and misleading turnkey automation remain S1; carry countercontrols; distinguish protocol from owned lifecycle. |
| Docs/tests/freshness | `WGDR-043..049` | IPC `049` S1; documentation/test activation mostly S2 except misleading core release guarantee; apply snapshot applicability. |
| Vocabulary | `WGDR-T01..T12` | Open qualified-term decisions, not global search/replace. |

### 12.3 Resolved or narrowed safeguards that must survive

**`[FACT]`** The following are not defects to “fix away”:

- route-free graph initialization and setup-neutral existing-graph launch (`WGDR-R01`);
- attended discovery broader than unattended admission (`R02`);
- generated root guide parity (`R03`);
- fail-closed unknown author/provider trust resolution (`R04`);
- distinct agency composition and operational assignment fields (`R05`);
- disclosed offline static-key/no-mandatory-forward-secrecy choice (`R06`);
- task-scoped Exec grants despite broader generic leash (`R07`);
- immutable historical spark/audit reports with later applicability (`R08/R09`);
- distinct completion review and agency performance evaluation (`R10`);
- worker capability interception despite test-isolation friction (`R11`);
- cycles as narrowed open ambiguity rather than absent or fully supported (`R12`).

### 12.4 Explicit uncertainty queue

**`[UNCERTAINTY]`** The following remain unresolved and must not be converted to facts by this synthesis: special completion writer reachability (`U01`), desired legacy policies (`U02`), destructive crash model (`U03`), exact candidate review concurrency (`U04`), real Pi/provider behavior (`U05`), website/source provenance (`U06`), CI toolchain precedence (`U07`), legacy evaluation migration (`U08`), credentialed independent review quality (`U09`), exact-archive platforms (`U10`), concurrency races (`U11`), and historical-signature/fresh-first-contact policy (`U12`). Proposed checks are in [`30` §6](30-contradiction-and-drift-register.md#6-explicit-uncertainty-register).

### 12.5 Recommendation

**`[RECOMMENDATION]`** Import the register into a machine-readable temporary disposition ledger before synchronization. Preserve `open`, `decision-required`, `accepted debt`, `resolved guard`, and `unknown`; do not flatten all rows into bugs.

---

## 13. Prioritized human decision queue

These are decisions, not factual documentation edits. Defaults are recommendations only.

| Order | Decision | Human question | Recommended default | Acceptance evidence |
|---:|---|---|---|---|
| 1 | `DEC-01` completion/smoke | Remove or restore legacy Done flags? Where, if anywhere, does owned smoke bind completion? | Keep v3 publication-derived completion sole ordinary authority; if required, bind smoke to immutable publication evidence. | One parser/dispatch/worker/help/test/agent-contract matrix plus actual operator and worker flow. |
| 2 | `DEC-07` federation governance | Is Fed production-supported or experimental? What custody/recovery/history policy is promised? | No hostile-worker custody claim until a separate authenticated signer exists; preserve explicit offline-FS debt. | Accepted threat model; distinct-boundary and adversarial recovery tests. |
| 3 | `DEC-09` remote/Pilot | Coordinator-owned remote lifecycle or manual protocol? Turnkey or bootstrap? | Reject automatic/turnkey interpretation until restart-safe owned lifecycle and real two-host check exist. | Two-home restart/failure flow, or explicit admission/help refusal. |
| 4 | `DEC-08` review | Required audit or best effort? Escalation or independent quorum? Which bypasses? | Required digest-bound durable record at enforcing high-value edges; call snapshot path escalation, not quorum. | Entry-point matrix, tamper/record-failure tests, persisted reviewer provenance. |
| 5 | `DEC-02` lifecycle | Cycles, Abandoned retry, and manual claim override semantics? | Shared admission by default; explicit reasoned/fenced override/restore. | Pause/time/cycle/retry human and worker flows. |
| 6 | `DEC-03` product/model | Pi sole attended, recommended, or overall? Which handlers unattended? | Scope “sole” narrowly until strict admission and selected tests support more. | Product sentence + route matrix + setup/doctor tests. |
| 7 | `DEC-05` evaluation/learning | Representation, performance definition, attempt visibility, credit, and learning join? | Append-only attempt ledger + non-authoritative views + separate exactly-once projector. | Focused audit 23 acceptance set; no projector lifecycle powers. |
| 8 | `DEC-06` identity/human | What binds persona, worker, `wgid`, compute provider, and human? | Signed/authorized binding; never infer trust from self-asserted metadata. | Rotation/evolution/mistaken-human negative tests. |
| 9 | `DEC-04` support surface | Commands, Casa, install modes, platforms? | Explicit public/advanced/internal/source-only matrix. | Release membership and exact-archive platform evidence. |
| 10 | `DEC-10` docs source graph | Unified or chapter Typst? Website generated or external? | One declared DAG; converter failure fatal. | Clean pinned regeneration and digest/link tests. |
| 11 | `DEC-11` vocabulary | Qualified names/aliases for `T01..T12`? | Namespaced cross-plane terms; compatibility aliases explicit. | Approved glossary linked to types/commands. |
| 12 | `DEC-12` persistence | What crash/platform guarantee is public? | Promise only tested Unix/process-crash bounds. | Fault injection, parent-fsync decision, Windows concurrency. |

**`[RECOMMENDATION]`** Named people/teams and independent reviewers must be assigned before dispatch. Security decisions require a security reviewer distinct from the prose author. No decision is “accepted” because source or a test exists.

---

## 14. Synchronized documentation roadmap and handoff

### 14.1 Governing rule

**`[INFERENCE]` (high confidence)** The immediate work is claim containment, not mass rewriting. A safe authority model joins scoped accepted policy, reachable implementation, exact evidence class, public support decision, owner, and derivative documentation. When the join disagrees, record `drift`, `partial`, `broken`, or `decision-required`.

### 14.2 Compact handoff matrix

This implements independent-review improvement `NBR-3` by tying executive issues to containment, decision, implementation, verification, and ownership.

| Executive issue / trace | Immediate factual containment path | Decision | Implementation domain | Verification class/command target | Owner/status |
|---|---|---|---|---|---|
| Done/smoke `WGDR-001/002` | `F-LIFE`: mark flags unsupported and smoke gate disputed in lifecycle/help/test docs | `DEC-01` | completion + CLI + test infrastructure | checkout-built release binary operator/worker Done flows; selected owned-smoke negative/positive | lifecycle owner; **unassigned/open** |
| Worker IPC `WGDR-049` | `F-LIFE/F-MODEL`: state array/read mutation limitation | Existing contract largely unambiguous | service IPC + messages | real socket unread/read/replay flow | service owner; **unassigned/open** |
| Onboarding `WGDR-008/009` | `F-ENTRY`: split existing/new/attended/unattended journeys | `DEC-03` | launcher/setup/doctor/profile | release-binary journey matrix | product/model owner; **unassigned/open** |
| Config/accounting `WGDR-010..012` | `F-MODEL`: state comment, extension, date, process scope | extension namespace/support wording | config + accounting | lossless config fixture; dated multi-day spend; cross-process metrics | config/ops owner; **unassigned/open** |
| Evaluation `WGDR-021..023` | `F-AGENCY`: separate completion, candidate, content, performance planes | `DEC-05/06` | completion/evaluation/agency | append-only/replay/exactly-once/anti-authority tests from audit 23 | evaluation/agency owner; **unassigned/open** |
| Custody/recovery `WGDR-029/030` | `F-SEC`: say same-user/in-process; carry optional KEK | `DEC-07` | identity custody/recovery | distinct-UID/hostile-worker signer and replay/backdating ceremony | security + Fed; **unassigned/open** |
| Inbox `WGDR-032` | `F-SEC`: say unauthenticated and quota-bounded | `DEC-07` | node/transport | authenticated read/delete/ack; overwrite/quota adversarial test | security + transport; **unassigned/open** |
| Review audit/quorum `WGDR-037..039` | `F-SEC`: best-effort hash chain; escalation; bypass matrix | `DEC-08` | review/trust/ingest | record-failure fail-closed, tamper, provenance, credentialed reviewer class | review/security; **unassigned/open** |
| Remote/Pilot `WGDR-040..042` | `F-SEC`: manual provider plane and real-host bootstrap | `DEC-09` | coordinator/Exec/Pilot | restart/failure two-home flow or explicit refusal; real provider identity | Exec/Pilot; **unassigned/open** |
| Evidence/docs `WGDR-043..048` | `F-EVIDENCE`: remove complete/current/live absolutes; add applicability | `DEC-04/10/11` | docs/tooling/CI/release | manifest coverage, clean regen, links/assets, evidence selection | docs/test/release; **unassigned/open** |

### 14.3 Phased synchronization plan

1. **Phase 0 — baseline, containment, owners.** Export every `WGDR`, assign named owner/reviewer, capture exact candidate evidence, and place narrow warnings on false high-impact claims.
2. **Phase 1 — bounded factual corrections.** Run `F-ENTRY`, `F-LIFE`, `F-MODEL`, `F-AGENCY`, `F-SEC`, and `F-EVIDENCE` without path moves or product decisions.
3. **Phase 2 — adjudicate and implement.** Decide `DEC-01..12`; land code/tests/contract/docs atomically where feasible. Fix unambiguous control-integrity defects such as IPC response semantics separately.
4. **Phase 3 — structural contracts.** Create separate estate manifest, decision index, product contract, glossary, and evidence index; generate CLI/schema/source-DAG/link views.
5. **Phase 4 — applicable journeys and information architecture.** Rewrite supported journeys/reference/operations from the adjudicated contract; add destinations and compatibility paths before moves.
6. **Phase 5 — evidence/archive/supersession.** Keep historical bodies immutable; add section-scoped status, applicability, successors, retention, and errata.
7. **Phase 6 — drift gates.** Fail on unclassified public surfaces, stale derivatives, missing evidence selection, broken links/assets, unauthorized decision-state changes, and unsupported human journeys—not on file age.

The detailed dependencies, path lists, rollback policy, negative fixtures, and program acceptance criteria are in [`31` §§3–6](31-documentation-sync-plan.md#3-findings-and-phased-synchronization-backlog).

### 14.4 Synchronization acceptance

**`[RECOMMENDATION]`** The program is not complete until every contradiction/term/guard/uncertainty has a machine-readable disposition and owner; every public/safety claim joins decision, source, selected evidence, support, and docs; every applicable public/safety document is inventoried; generated outputs are reproducible; historical bytes remain immutable; human journeys run against release artifacts; and deliberate unclassified changes fail the relevant gate.

### 14.5 Risks and rollback

**`[INFERENCE]` (high confidence)** The largest roadmap risks are factual edits choosing policy, consistency with unsafe overclaims, stale registries, link-breaking bulk moves, malformed generation, and compatibility stubs becoming authorities. Use domain-sized changes, compatibility windows, immutable historical bodies, pinned generators, fail-closed conversion, and contract status rollback alongside code rollback ([`31` §5](31-documentation-sync-plan.md#5-risks-safeguards-and-rollbackarchive-policy)).

---

## 15. Independent-review dispositions

### 15.1 Release decision

**`[FACT]`** [`90-independent-review.md`](90-independent-review.md) scored the draft **75/100** and recommended **HOLD** until seven blocking corrections were visibly resolved. This final does not claim a re-audit or a product refresh; it resolves presentation/calibration blockers against the pinned evidence.

### 15.2 Blocking corrections

| Review item | Disposition in this final |
|---|---|
| `BR-1` charter conformance | **Adopted.** Exact charter labels are used. Every major domain (§§4–10) has Executive abstract → Scope and map → Findings → Contradictions and drift → Risks and gaps → Recommendations → Evidence appendix. Method deviation in the draft is recorded, not hidden. |
| `BR-2` risk rescoring | **Adopted.** Executive/material tables separate severity, likelihood, and confidence. Config, accounting, learning, CI inventory, docs, and setup/doctor findings were individually recalibrated; no `S1/high` shorthand remains. |
| `BR-3` daemon blocking | **Adopted.** Restored to `WGDR-U04` **S2 open uncertainty**. Installed-binary observation, pinned source shape, and required candidate-built test are separate. |
| `BR-4` mixed evidence classes | **Adopted.** This final does not label source plausibility as verified behavior. Inherited executions are attributed to exact leaves; final-local `[VERIFIED]` commands are repository/provenance checks with environment/result in §16.3. Pi/accounting and HOL clauses are split into fact/uncertainty/post-snapshot applicability. |
| `BR-5` applicability | **Adopted.** Top banner says historical pinned snapshot; local state uses `snapshot-current`; exact 89-file post-snapshot delta is recorded; no selective remediation is silently closed. |
| `BR-6` security balance | **Adopted.** Custody carries optional at-rest KEK but no hostile-worker isolation; inbox says bounded quota consumption and retention; review log carries lock/atomic-write countercontrols; Pilot overclaim is attributed to product/runbook scope while its CLI caveat is preserved. Every executive S1 names enforcement, gap, likelihood, confidence, and unverified boundary. |
| `BR-7` coverage omissions | **Adopted.** §2.3 names human surfaces, formal-model boundary, ancillary/product trees, scale, supply chain, destructive/cross-platform/external behavior, and owner assignment. Relevant domain sections repeat local limits. |

### 15.3 Non-blocking review points

| Review point | Response |
|---|---|
| `IR-009` config intent | Adopted: arbitrary-path support is described as intentional extensibility; comment loss/ineffective keys remain, while namespace policy is decision-gated. |
| `IR-010` roadmap density / `NBR-3` | Adopted: §14.2 provides a one-page handoff matrix. |
| `NBR-1` state filters | Adopted in §3.1. |
| `NBR-2` evidence-strength key | Adopted in Reading key and executive tables. |
| `NBR-4` reduce absolutes | Adopted: bounded phrases such as “sampled path,” “pinned source encodes,” and scoped absence searches are used. |
| `NBR-5` commit permalinks | Not implemented by this audit-only artifact. Snapshot hash and line spans are explicit; a generated permalink/evidence index remains a roadmap recommendation. |
| `NBR-6` nearby counterexamples | Adopted in executive/domain findings and §12.3. |

### 15.4 Remaining review uncertainty

**`[UNCERTAINTY]`** The independent review's advisory completion route timed out/unavailable according to its task provenance; its artifact itself records deterministic structural/primary checks and limitations. This final treats the written review as the required independent artifact, not as a product behavior pass.

---

## 16. Evidence and provenance appendix

### 16.1 Complete artifact crosswalk

Every artifact in the audit bundle is linked here; the final does not supersede leaf evidence.

| Artifact | Contribution used by this final |
|---|---|
| [`README.md`](README.md) | Normative charter, scope, evidence labels, risk vocabulary, snapshot, fan-in contract. |
| [`10-code-architecture.md`](10-code-architecture.md) | Module/persistence/authority findings `ARCH-001..009`. |
| [`11-orchestration-lifecycle.md`](11-orchestration-lifecycle.md) | Lifecycle, completion, service, cycle/retry, and bounded live traces `ORCH-001..017`. |
| [`12-model-execution-plane.md`](12-model-execution-plane.md) | Handler/Pi/stream/accounting/setup findings `MODEL-001..010`. |
| [`13-agency-evaluation-chat.md`](13-agency-evaluation-chat.md) | Agency/evaluation/function/chat/context/concierge/human findings. |
| [`14-federation-identity-security.md`](14-federation-identity-security.md) | Identity/custody/recovery/transport/capability/state/governance findings `FED-001..014`. |
| [`15-review-exec-pilot.md`](15-review-exec-pilot.md) | Ingest/trust/review/Exec/Pilot findings `RXP-001..011` and drift. |
| [`16-documentation-information-architecture.md`](16-documentation-information-architecture.md) | Documentation estate/authority/source-DAG findings `DOC-001..010`. |
| [`17-testing-ci-quality.md`](17-testing-ci-quality.md) | Test selection/smoke/formal/release findings `TEST-001..011`. |
| [`18-operations-configuration-ux.md`](18-operations-configuration-ux.md) | IPC/config/onboarding/accounting/package/platform/operator findings `OPS-001..014`. |
| [`19-conceptual-model-and-vocabulary.md`](19-conceptual-model-and-vocabulary.md) | Product center, object hierarchy, authority namespaces, terminology `CONCEPT-001..010`. |
| [`20-core-runtime-synthesis.md`](20-core-runtime-synthesis.md) | Architecture/orchestration/model cross-plane synthesis. |
| [`21-agency-federation-safety-synthesis.md`](21-agency-federation-safety-synthesis.md) | Typed authority vector and safety composition `XAUTH-001..010`. |
| [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md) | Product contract, documentation, verification, and operations synthesis `PRODUCT-001..008`. |
| [`23-evaluation-evolvability-cutover.md`](23-evaluation-evolvability-cutover.md) | Focused representation/learning audit `EVC-001..008` and ledger/projector decision default. |
| [`30-contradiction-and-drift-register.md`](30-contradiction-and-drift-register.md) | Deduplicated `WGDR-001..049`, terms, guards, uncertainties, primary checks. |
| [`31-documentation-sync-plan.md`](31-documentation-sync-plan.md) | Authority model, F/D/I/S/V work types, phases, decision queue, rollback, drift gates. |
| [`40-system-synthesis-draft.md`](40-system-synthesis-draft.md) | Comprehensive draft architecture and source crosswalk; used as base but not copied unchanged. |
| [`90-independent-review.md`](90-independent-review.md) | Skeptical score, 13 primary samples, blockers, countercontrols, coverage gaps, release recommendation. |
| [`99-SYNTHESIS.md`](99-SYNTHESIS.md) | This reviewed final, including dispositions and applicability warning. |

### 16.2 High-severity primary-evidence index

| Assertion | Direct pinned evidence | Counterevidence/uncertainty |
|---|---|---|
| Done/smoke public authority mismatch | `src/cli.rs:528-554`; `src/main.rs:1261-1274`; `tests/smoke/README.md:3-29`; pinned completion modules searched by audit 30 | v3 completion is a strong positive control; tests not selected are not failures. |
| Stateful worker IPC response | `src/commands/service/ipc.rs:253-274,720-790`; `src/messages.rs:631-696` | Live result inherited from audit 18; present-day candidate unknown. |
| Same-user custody/recovery | `src/identity/keys.rs:51-68,226-300,340-377`; `src/identity/sigchain.rs:493-515,884-925` | Optional at-rest KEK; exploit/distinct-UID not run. |
| Inbox ownership | `src/identity/node.rs:408-443,551-572`; `src/identity/transport.rs:318-354,480-496` | Count/byte limits and retention bound storage. |
| Remote/Pilot overclaim | `src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:330-347`; `src/commands/pilot_cmd.rs:43-50,1066-1125,1184-1215` | Pilot CLI states two-host prerequisite; product/runbook scope is at issue. |
| Onboarding scope | `src/bin/worksgood.rs:6-16,124-151`; `src/config_defaults.rs:20-107`; `src/commands/setup.rs:1389-1471`; `src/commands/doctor.rs:166-226,241-416` | Existing-graph setup-neutral launch is deliberate. |
| Release/evidence overclaim | `.github/workflows/ci.yml:68-201`; 176-target pinned inventory; `tests/integration_smoke_gate.rs:1-11,131-412` | Formal/Pi/selected lanes are real positive controls; unselected does not mean fail. |

### 16.3 Final-local commands

**`[VERIFIED]`** On 2026-08-09 UTC, from `/home/bot/wg/.wg-worktrees/agent-28`, Linux `6.8.0-90-generic x86_64`, final pre-write checkout `7219f71540557bc79fe313a6dd546ca9463292d5`, repository/provenance commands below completed with exit 0 unless explicitly shown. They verify repository bytes and ancestry/delta, not product behavior.

```bash
rev=b0892ea7496fd2cc8f641417a3d8e33ca9add369
git rev-parse HEAD
git diff --shortstat "$rev"..HEAD -- . \
  ':(exclude)docs/audit/2026-08-08-worksgood-system/**'

# Fourteen pinned-byte assertions; each uses:
#   text=$(git show "${rev}:${path}"); printf '%s' "$text" | rg -q "$pattern"
# Assertions:
#   src/cli.rs                           full_smoke
#   src/main.rs                          legacy wg done bypass/merge/cycle flags
#   src/commands/service/ipc.rs          serde(flatten)
#   src/commands/service/ipc.rs          to_value(messages)
#   src/identity/keys.rs                 load_secret
#   src/identity/node.rs                 inbox_max_events
#   src/commands/spawn_task.rs           remote-runner executor is driven by the WG-Exec providers plane
#   src/commands/pilot_cmd.rs            check_passed: None
#   src/commands/config_cmd.rs            toml::to_string_pretty
#   src/commands/spend.rs                Utc::now().format
#   src/metrics.rs                       AtomicU64
#   .github/workflows/ci.yml             cargo test --test integration_service
#   src/review/verdict.rs                pub struct VerdictRecord
#   src/review/verdict.rs                atomic_file::write_atomic
```

Bounded output:

```text
HEAD=7219f71540557bc79fe313a6dd546ca9463292d5
post-snapshot non-audit delta=89 files changed, 5995 insertions(+), 413 deletions(-)
14/14 corrected pinned-byte assertions: PASS
```

**`[FACT]`** An earlier assertion script attempt used incorrect patterns and exited nonzero; it produced no evidence used here. The corrected command above passed and is the only final-local assertion result claimed.

### 16.4 Evidence inherited, not rerun

**`[FACT]`** Candidate-built help behavior and leaf product executions are inherited from their exact appendices, especially audits [`10`](10-code-architecture.md), [`11`](11-orchestration-lifecycle.md), [`12`](12-model-execution-plane.md), [`18`](18-operations-configuration-ux.md), [`23`](23-evaluation-evolvability-cutover.md), and register [`30`](30-contradiction-and-drift-register.md). This final checked source plausibility and review disputes; it did not relabel those commands as final-local executions.

### 16.5 Final limitations

- **`[UNCERTAINTY]`** Static absence searches are bounded to named modules and cannot exclude indirect/generated callers elsewhere.
- **`[UNCERTAINTY]`** Severity and likelihood require candidate-specific threat/usage data; this final calibrates audit reporting but cannot measure production incidence.
- **`[UNCERTAINTY]`** Cryptographic source inspection and smoke evidence are not a security audit or proof of deployment custody.
- **`[UNCERTAINTY]`** The documentation roadmap has roles, not named accountable people; it is traceable but not dispatch-ready.
- **`[FACT]`** Recommendations do not alter production behavior or authorize documentation changes. Each begins with candidate revalidation and the named human decision where required.
