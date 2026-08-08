# Evaluation cutover: interpretability and agency evolvability

**Audit date:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Evidence checkout:** `e702437df0e39e911ff628a6da994bb294d0ad5e`; production files are byte-equivalent to the audit snapshot (`git diff --name-only b0892ea7..e702437d -- . ':(exclude)docs/audit/2026-08-08-worksgood-system/**'` returned no paths).

**Evidence checked through:** 2026-08-08T14:32:15Z

**Scope:** the cutover from schedulable `.assign-*`, `.flip-*`, and `.evaluate-*` graph tasks to candidate-bound evaluation records and then the worker-owned completion manifest valve; legacy agency learning, current completion review, direct assignment, and evolver inputs. This is a decision audit only. It changes no production behavior.

## 1. Executive abstract

**`[FACT]`** Three successive representations coexist in source, but only one is the normal completion authority. March 2026 made assignment, FLIP, and evaluation ordinary graph citizens. The ratified July lifecycle design replaced eager review satellites with candidate- and attempt-bound `EvaluationRecord`s. The implemented August 5 worker-owned protocol then made `wg submit` synchronously run manifest-bound FLIP followed by eval and store compact `ReviewReceipt` object references on the parent task. On August 7, production creation and lifecycle authority of synthetic agency tasks were deleted. Current `wg done` derives success from the exact review pair plus publication; neither the lazy-record runners, legacy agency evaluator, assigner, nor evolver participates in this current path (`src/commands/completion_submit.rs:208-482`; `src/completion_review.rs:162-388`; `src/commands/completion_done.rs:33-267`).

**`[VERIFIED]`** The cutover removed the old review-lifecycle hazards in the tested current protocol. `completion_review_valve` passed 9/9: exact FLIP-before-eval ordering, candidate binding, immutable receipts/findings, and distinct semantic reject, incomplete evidence, and infrastructure-unavailable outcomes. `integration_agency_pipeline` passed 34/34 active tests (5 retired model-registry tests ignored), including that publication creates neither synthetic agency tasks nor agency dependency edges. Current source routes ordinary `wg submit`, `wg land`, and `wg done` directly to the v3 completion commands (`src/main.rs:1274-1311`; worker IPC at `src/commands/service/ipc.rs:888-918`).

**`[FACT]`** The current receipts preserve the most safety-critical property: the reviewed manifest, requirements, inspected outputs, reviewer kind, semantic/infrastructure verdict class, findings digest, and exact model route are immutable and rechecked before publication-derived Done (`src/completion_review.rs:83-121,182-299,357-388`; `src/completion_task.rs:150-217`). They do **not** preserve several useful properties of ordinary graph tasks or the richer July `EvaluationRecord`: reviewer attempt identity/history, start/end timing, reasoning, structured usage/cost, response digest, retry lineage, durable failure attempts, source attempt/fence, source agency composition, or a consumed learning key.

**`[VERIFIED]`** Live graph evidence makes the split visible. At 2026-08-08T14:32:15Z the graph had 23 rows and 12 Done tasks. `wg list --all` exposed zero `.assign-*`, `.flip-*`, or `.evaluate-*` rows. Direct graph/object-store verification found zero task `evaluation_records`, but all 12 Done tasks had an exact FLIP+eval pass pair bound to the selected manifest and requirements. All 12 had nonzero raw Pi `turn_end` usage in their source-worker streams; 11 of those 12 had `token_usage=null` in the graph. The one populated row was `audit-charter`. Thus the task-specified observation is exact: **11/12 Done tasks had raw Pi usage and null graph accounting**. `wg agency stats` reported `Evaluations: 0`; `.wg/agency/evaluations` contained zero JSON files; `agency.auto_assign=false`; and no live task had an agency `Task.agent` composition.

**`[INFERENCE]` (high confidence)** Current reviews are candidate-interpretable enough to enforce completion, but not attempt-interpretable or economically attributable enough to explain reviewer operation. They are not agency-evolvable. Modern completion and lazy-record call sites do not invoke `record_evaluation` or `record_evaluation_with_inference`; the agency performance/evolver plane reads only legacy evaluation files and inline performance arrays. Direct dispatch supplies runtime worker identity but no agency composition. Consequently exact review can gate work while assignment ranking, component credit, retrospective inference, and evolution receive no signal.

**Decision recommendation:** keep the current completion valve as the **only** source-lifecycle authority. Do not restore ordinary review tasks. Add an append-only, first-class review-attempt/receipt ledger with dedicated CLI/TUI and non-authoritative virtual projections, then add a separate exactly-once learning projector. The projector may observe a terminal generation and its candidate trajectory; it must have no capability to publish, retry, reopen, fail, or complete the source. This combines representation options 2 and 4 below without reviving the lifecycle coupling of option 1.

## 2. Scope and system/data-flow map

### 2.1 The five planes that must not be conflated

| Plane | Durable representation | Current entry | Lifecycle authority | Learning effect |
|---|---|---|---|---|
| Legacy agency evaluation | `.wg/agency/evaluations/*.json` plus `PerformanceRecord.evaluations` in agent/role/tradeoff/component/outcome YAML | manual/compatibility `wg evaluate` and `wg evaluate record` | legacy gate code remains, but not v3 Done authority | **Yes:** `record_evaluation[_with_inference]` propagates scores |
| Candidate-bound bounded/deep evaluation | `Task.evaluation_records: Vec<EvaluationRecord>` plus CAS evidence | explicit bounded/deep CLI; retained candidate-finalization and coordinator lane | acceptance controller may consume; runners are observation-only | **No join** to agency recorder |
| Universal completion FLIP/eval | parent `completion_candidate` object refs and CAS `ReviewReceipt`s created during `wg submit` | normal current worker completion | **Yes, indirectly:** only the completion valve and `wg done` consume exact pass receipts | **No join** to agency recorder |
| Assignment | `Task.agent`, assignment YAML/provenance, or runtime `Task.assigned` | manual `wg assign`; direct dispatcher reservation | assignment can gate dispatch only when explicitly performed; current publisher creates no hidden assignment task | ranking reads legacy performance; no current completion feedback |
| Evolution | entity performance YAML, evaluation JSON, assignment experiments, `evolver_state.json` | manual/optional auto-evolve | none over source lifecycle | reads legacy files/aggregates only |

**`[FACT]`** Retained compatibility code is not proof of normal reachability. `mint_for_candidate` is still called by historical finalization/Done paths, and coordinator ticks can run pending bounded/deep records (`src/commands/finalize.rs:1206`; `src/commands/done.rs:2818`; `src/commands/service/coordinator.rs:2401-2424`). The v3 worker path instead calls `completion_submit::run`, records compact receipts, lands/publishes, and calls `completion_done::run`; it does not mint an `EvaluationRecord` (`src/main.rs:1274-1311`; `src/commands/completion_submit.rs:208-482`). The live graph's 12 exact completion pairs and zero `EvaluationRecord`s corroborate that distinction.

### 2.2 Historical reconstruction

| Date / commit | Architecture and intent | Useful property | Hazard or reason for cutover |
|---|---|---|---|
| 2026-03-05 `f0c7ec74` | Persist token usage on `.evaluate-*` and `.assign-*` rows. | reviewer/assigner usage appeared in normal task accounting/UI | accounting depended on treating internal calls as tasks |
| 2026-03-08 `6bb0123a`; 03-09 `71ab2e46`, `c6838805` | Publish-time `.assign-X → X → .flip-X → .evaluate-X` graph citizens. | ordinary list/show/history, messages, route, token cost, failures, retries, edges | eager noise; work for tasks that never executed; task/evaluator statuses and dependency edges became coupled |
| 2026-07-14 `9176849c`; 07-19 `823dc578` | Repair `FailedPendingEval` deadlocks and salvage durable pending-eval lifecycle. | retained failed source/evaluator evidence | rescue exceptions, edge surgery, status proliferation, and attempt drift demonstrate structural coupling |
| 2026-07-26 ratified lifecycle design; `8cce460f` Pi evaluation design | Evaluation becomes a separate state domain: lazy `EvaluationRecord`, exact candidate/attempt/fence/route, independent queue and retry budget, virtual satellites only. | strong provenance, attempt history, usage, failures, replay, scheduling isolation | implementation complexity; coexistence with old finalization paths |
| 2026-07-28 `0dd48b92`; `11b4bdcb` | Implement lazy records, then required deep FLIP before candidate merge. | exact immutable source tuple; rich attempts/usage/evidence; evaluator cannot mutate source | not yet the final universal completion protocol |
| 2026-08-05 `6ae37e9e`; `6ac127a4` | Universal worker-owned manifest valve: synchronous FLIP then eval receipts for every Land/Report/Explore submission. | simple exact gate; same worker repairs; no scheduler/finalizer child work | receipt schema is much thinner than `EvaluationRecord` |
| 2026-08-07 `76fbe614`, `922a5856`, `4b789733`, `8f129c55`, `453c3e2b`, `6c333460` | Delete synthetic agency-task authority, assignment rewrites, evaluator-driven retry/reconciliation/repair, and retire legacy FLIP routing tests. | one source owner and one completion authority | retained legacy/manual code and documentation still imply a composed learning loop |

**`[DOC-CLAIM]`** The July lifecycle design explicitly required records rather than ordinary tasks, separate evaluation attempts/retries, append-only evidence, no evaluator rescue authority, and virtual aliases for observability (`docs/design-simplified-task-lifecycle.md:1-42,520-610,742-845`). The Pi evaluation design went further: runner attempts, Pi-reported usage/cost, exact route/reasoning, failure categories, `wg evaluate status`, and a later ledger projection (`docs/design-pi-evaluation-plane.md:1-88,215-313,575-699`).

**`[DOC-CLAIM]`** The August normative protocol deliberately simplified review to a compact `ReviewReceipt` and states that FLIP/eval are first-class stages of the parent, not synthetic graph children (`docs/design-worker-owned-universal-review.md:1-34,126-179`). It is authoritative for current completion. It did not decide how those receipts feed agency learning.

### 2.3 Current data flow and the missing learning join

```text
source task + lifecycle attempt
        |
        | worker builds immutable CompletionManifest
        | {task, generation, contract, requirements digest,
        |  source revision, outputs, validation, summary digest}
        v
wg submit --manifest M --summary S
        |
        +--> select parent CompletionCandidateRefs
        |    (new selection clears disposition/terminal receipt)
        |
        +--> resolve exact immutable review bundle
        |
        +--> FLIP exact-route call
        |      semantic pass/reject OR unavailable
        |      -> findings object + compact ReviewReceipt
        |
        +--> only exact FLIP pass permits eval call
        |      -> findings object + compact ReviewReceipt
        |
        +--> store receipt refs on parent candidate
        v
wg land/report/explore publication
        |
wg done re-resolves bytes + exact pair + publication
        |
        +--> lifecycle AttemptSucceeded / Done
        |
        X  no record_evaluation[_with_inference]
        X  no source agent/role/tradeoff/component/outcome update
        X  no assignment outcome feedback
        X  no evolver evaluation-file increment
```

**`[FACT]`** `run_exact_agency_dispatch_call` returns `LlmCallResult { text, token_usage }`, but `ExactModelReviewer::review` consumes only `result.text`; `ReviewReceipt` has no usage field (`src/service/llm.rs:22-29,311-319`; `src/completion_review_model.rs:58-88`; `src/completion_review.rs:83-95`). This is a proven call-site loss, not an inference from absent UI.

### 2.4 Field-level lineage matrix

Legend: **Durable** = persisted and revalidated; **Indirect** = recoverable only through another store/log; **Lost** = produced or available at a call site but not joined into the current receipt/consumer; **N/A** = intentionally separate.

| Source field / event | Current v3 receipt lineage | Accounting | Agency performance | Assignment / evolution | Disposition |
|---|---|---|---|---|---|
| graph ID, task ID, generation | task/generation in manifest and completion receipt; task row selects candidate | task row only | no projection | no projection | **Durable at gate; Lost to learning** |
| source attempt ID and fence | lifecycle audit has them; manifest/`ReviewReceipt` does not | task usage is attempt-aggregated/nullable | no key for idempotent projection | cannot separate retries/generations by receipt alone | **Indirect** |
| runtime worker `agent-N` | lifecycle actor/agent metadata and raw stream | raw Pi stream; graph field often null after completion | not an agency composition ID | not rank/evolver input | **Indirect and identity-incompatible** |
| `Task.agent` agency composition | would persist on task if assigned; live graph has none | N/A | legacy recorder can credit it | ranking can read its performance | **Absent on direct-dispatch live flow** |
| assignment decision/experiment | assignment YAML only when `wg assign` path records it | assignment one-shot may return usage on dormant path | placeholder assigner score 0.5 may be written | retrospective inference requires a later legacy evaluation | **No v3 completion join** |
| requirements digest | manifest and both receipts; rechecked by Done | no cost join | no context partition | no evolver input | **Durable** |
| output and validation digests | manifest plus `inspected_output_digests`; resolver rechecks | no cost join | no score/dimension mapping | no input | **Durable** |
| manifest/candidate digest | parent candidate and both receipts | no cost join | no normalized evaluation ID | no ranking/evolution key | **Durable at gate; Lost to learning** |
| reviewer kind | receipt `flip` or `eval` | no reviewer account | not evaluator-agent ID | cannot calibrate a reviewer entity | **Durable kind, missing identity** |
| exact model route | receipt string; exact-pass validation requires nonempty route | no route-level usage/cost | legacy `Evaluation.model` not written | no route quality/reliability learning | **Durable route, Lost accounting/calibration** |
| handler/provider/reasoning/adapter version | available in resolved dispatch/config; absent from compact receipt | none | none | none | **Lost** |
| reviewer attempt ID, attempt number, started/completed time | no attempt object; pair receives one shared `created_at` even though calls are sequential | none | none | no failure/retry rate | **Lost** |
| model token usage/cost | returned by exact call as `LlmCallResult.token_usage`, then discarded | no reviewer usage or cost row | none | none | **Lost at call site** |
| raw response digest / stop reason | parsed text only; findings object persisted; no response digest/stop reason | none | none | no replay/calibration evidence | **Lost** |
| semantic pass/reject | receipt verdict; reject keeps valve closed | no score/cost | no performance update | no rank/evolver signal | **Durable at gate; Lost to learning** |
| incomplete evidence | FLIP receipt with distinct verdict and resolver findings; eval not invoked | no usage if no call | no negative performance | no evolution signal | **Durable infrastructure/evidence class** |
| reviewer unavailable/malformed/route failure | compact `Unavailable` plus bounded finding; no structured attempt taxonomy | no failed-attempt usage/cost | correctly not source performance | no provider/reviewer reliability input | **Semantically separated, operational history thin** |
| findings | CAS object referenced by digest | N/A | no dimensions/score | no evolver prompt input | **Durable but weakly projected** |
| superseded candidate and old receipts | CAS bytes remain; replacing `completion_candidate` removes the task's refs and there is no parent receipt index | orphaned cost | no learning key | no trajectory | **Physically retained, logically unjoined** |
| exact review consumption / Done | completion receipt references manifest plus flip/eval receipt digests; lifecycle event records Done | task usage nullable | no learning event | no exactly-once projection | **Durable completion, missing learning receipt** |
| legacy `Evaluation.score/dimensions/notes/model` | separate `.wg/agency/evaluations` JSON | not review-attempt accounting | propagates to agent, role, tradeoff, components, outcome | evolver loads/counts it; retrospective inference may update experiments | **Durable old plane only** |

### 2.5 Credit and failure semantics that a bridge must preserve

**`[FACT]`** Legacy `record_evaluation` writes an `Evaluation`, then independently updates agent, role, tradeoff, every role component, and desired outcome (`src/agency/eval.rs:49-160`). `record_evaluation_with_inference` additionally looks up a learning assignment and updates experimental primitive/attractor state (`src/agency/eval.rs:167-201`; `src/agency/run_mode.rs:378-482`). `should_trigger_evolution` counts JSON files and computes recent scores from that store (`src/agency/evolver.rs:120-224`). None reads completion receipts or `EvaluationRecord`s.

**`[FACT]`** Manual automatic assignment ranks agency agents by history-partitioned `PerformanceRecord.avg_score`; the retained LLM assigner also renders that history but has no production coordinator caller (`src/commands/assign.rs:205-393`; `src/commands/service/assignment.rs:397-477`). Explicit assignment writes a placeholder evaluation of 0.5 to the assigner composition. Despite a comment promising retrospective quality, no call site later updates that placeholder from the completed task (`src/commands/assign.rs:107-164`; repository search in appendix).

**`[INFERENCE]` (high confidence)** A safe bridge cannot simply call legacy `record_evaluation` for every review receipt. That would (a) count FLIP and eval twice for one source candidate; (b) count every rejected superseded candidate as a full independent task outcome; (c) treat infrastructure outages as source quality; (d) conflate reviewer confidence with ground truth; (e) let an evaluator improve or damage its own score using its own verdict; and (f) duplicate updates on crash/replay because legacy filenames/timestamps are not candidate idempotency keys.

## 3. Findings

### `EVC-001` — the cutover removed lifecycle authority from evaluators

- **Label/state:** `[FACT]`, `[VERIFIED]`; shipped/current.
- **Severity/confidence:** S4 positive control; high.
- Ordinary publication creates no agency satellites or edges. The current review adapter has no graph tools or source worktree and can only return a semantic result. FLIP must exactly pass before eval runs. `wg done` independently re-resolves the candidate, reviews, and publication before requesting the terminal lifecycle transition (`src/completion_review_model.rs:1-19,58-88`; `src/completion_review.rs:182-299`; `src/commands/completion_done.rs:33-267`).
- The August 7 commits removed evaluator-driven retry, reconciliation, repair, and synthetic task authority rather than merely hiding rows.
- **Executed control:** targeted tests passed as stated in §1.

### `EVC-002` — current receipts strongly bind decisions to immutable output

- **Label/state:** `[FACT]`, `[VERIFIED]`; shipped/current.
- **Severity/confidence:** S4 positive control; high.
- `ReviewReceipt` binds version, manifest, requirements, reviewer kind, verdict, findings, inspected outputs, route, and time. `load_exact_review_pair` requires exact pass, nonempty route, and identical inspected output digests. Changed requirements/output/manifest invalidates the pair (`src/completion_review.rs:83-121`; `src/completion_task.rs:95-217`).
- Live graph verification found 12/12 Done tasks with exact FLIP and eval pass pairs. No Done row depended on a synthetic review task.
- Semantic `Reject`, `IncompleteEvidence`, and `Unavailable` are distinct. An unavailable reviewer cannot be parsed as semantic reject (`src/completion_review.rs:35-68,126-152,182-299`).

### `EVC-003` — attempt interpretability regressed from both predecessor representations

- **Label/state:** `[FACT]` + `[INFERENCE]`; current gap.
- **Severity/likelihood/confidence:** S2; observed; high.
- Ordinary graph rows had task attempt/log/status/message/route/token fields. July `EvaluationRecord` has a source attempt/fence/round, route snapshot, `EvaluationAttempt`s, structured usage, response digest, failures, evidence IDs, prior deep reports, and consumed verdict (`src/evaluation/mod.rs:112-235`). Current compact receipts omit these.
- `wg show` prints only completion manifest and receipt object digests for the v3 path. Its rich attempt/usage/failure renderer applies only when `evaluation_records` is nonempty (`src/commands/show.rs:937-953,1201-1290`). Live current tasks have zero such records.
- A rejection's findings are durable CAS bytes, but `wg submit` returns a generic repair message and no dedicated `wg reviews` projection enumerates prior/superseded attempts. Replacing the parent candidate clears the only graph refs to prior review receipts (`src/commands/completion_submit.rs:403-482`). CAS retention alone is not an inspectable history join.

### `EVC-004` — review and worker accounting are not closed

- **Label/state:** `[FACT]`, `[VERIFIED]`; current gap.
- **Severity/likelihood/confidence:** S2; observed; high.
- The exact reviewer call returns structured token usage, but the adapter discards it when it extracts text. The receipt schema has no token/cost field. Therefore no current receipt, task field, agency evaluation, or spending projection attributes review cost (`src/service/llm.rs:22-29,311-319`; `src/completion_review_model.rs:58-88`; `src/completion_review.rs:83-95`).
- Live source-worker accounting is also incomplete: all 12 Done tasks had raw Pi usage; 11 had null graph `token_usage`. This is not evidence that review caused worker-accounting loss, but it proves the present graph/spend projection is not a reliable economic ledger for either the reviewed source population or its synchronous reviewers.
- Historical commit `f0c7ec74` had already solved internal-call usage visibility by attaching usage to ordinary `.assign-*`/`.evaluate-*` tasks. The cutover removed the representation without replacing that accounting join.

### `EVC-005` — automatic completion review does not evolve agency

- **Label/state:** `[FACT]` + `[INFERENCE]`; disconnected composition.
- **Severity/likelihood/confidence:** S1; observed in live graph and likely generally; high.
- Repository-wide call-site search finds agency performance writes only in legacy/manual assignment/evaluation/evolve paths; none in `src/completion_*`, `src/commands/completion_*`, or `src/evaluation/**`. Evolver input is only legacy evaluation JSON and entity performance (`src/agency/eval.rs:49-201`; `src/agency/evolver.rs:120-224`).
- Live evidence: exact review pairs for 12/12 Done; agency stats `Evaluations: 0`; zero agency evaluation JSON; every task had zero `evaluation_records`; no task had an agency composition.
- **Inference:** review can be universally mandatory while agency assignment/evolution remains completely starved. The product property “evaluation causes adaptive agency learning” is not preserved by the cutover.

### `EVC-006` — direct dispatch and assignment feedback have no durable outcome join

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial/dormant.
- **Severity/likelihood/confidence:** S2; likely; high.
- Current publication creates no hidden assignment task. The dispatcher reserves a runtime `assigned=agent-N`, while agency composition lives in separate `Task.agent`. The live graph had no non-null `Task.agent`; completed rows clear runtime `assigned`, leaving lifecycle actor metadata rather than a role/tradeoff composition.
- `agency.auto_assign=false` in the live config. Repository-wide search found no production caller of `run_lightweight_assignment`; the targeted build also emitted it as dead code. Manual `wg assign --auto` is deterministic performance ranking, despite help/comment wording that says LLM.
- The assigner placeholder evaluation is always 0.5 and is not joined to later review/outcome. Retrospective inference only runs when a legacy evaluation is recorded for a task with an assignment experiment (`src/commands/assign.rs:107-164,205-393`; `src/agency/run_mode.rs:378-482`).

### `EVC-007` — retained compatibility planes are individually useful but compositionally ambiguous

- **Label/state:** `[FACT]` + `[CONTRADICTION]`; current.
- **Severity/likelihood/confidence:** S2; likely; high.
- `EvaluationRecord` remains a rich active/manual/compatibility facility and coordinator lane. Legacy `wg evaluate` remains a score-propagating agency facility. The normal current completion valve uses neither. Similar labels—evaluation, FLIP, evaluator—therefore refer to different schemas, containment, retries, visibility, accounting, and learning effects.
- Current source comments are clear locally, but manuals/quickstart and retained config (`auto_evaluate`, `auto_rescue_on_eval_fail`, evaluator agents) do not provide one authority/reachability map. The live config has `auto_evaluate=true`, yet normal Done tasks have only v3 receipts, zero `EvaluationRecord`s, and zero agency evaluations.

### `EVC-008` — legacy history is preserved but not migrated into one queryable lineage

- **Label/state:** `[FACT]` + `[UNCERTAINTY]`; compatibility gap.
- **Severity/likelihood/confidence:** S2; possible for upgraded graphs; high for implementation shape.
- `retire_stale_legacy_satellites` abandons only unclaimed, evidence-free rows and retains claimed/terminal/verdict-bearing rows for explicit handling (`src/commands/eval_scaffold.rs:1-86`). The Pi design says unbound legacy verdicts remain historical/advisory and files are not rewritten (`docs/design-pi-evaluation-plane.md:681-699`).
- No current importer joins terminal synthetic task attempts, legacy agency evaluation JSON, lazy records, and v3 receipts under one provenance/version model. This correctly avoids inventing candidate bindings, but it leaves cross-era trend, cost, and assignment analysis fragmented.
- **Uncertainty:** the live audit graph is new and has no legacy rows/files, so upgraded-graph behavior was inspected, not executed.

## 4. Contradictions and drift

| ID | Evidence in tension | Current authority and resolution |
|---|---|---|
| `EVC-DRIFT-001` | July designs promise rich runner attempts, usage/cost, failures, and `wg evaluate status`; August current receipt contains only compact decision fields. | August worker-owned completion is current lifecycle authority. The rich July record still exists, but is not the normal v3 receipt. Product must decide whether the simplification intentionally dropped observability. **Open.** |
| `EVC-DRIFT-002` | “Every task passes through FLIP and eval” is true for v3 completion, while `auto_evaluate=true` historically meant a scored agency evaluation and still names a bounded-record selector. | Scope-qualify: universal **completion review** is not agency performance evaluation. **Apparent contradiction resolved technically; terminology drift remains.** |
| `EVC-DRIFT-003` | Agency docs/config imply evaluation feeds agent/primitive evolution; modern universal receipts never call performance recording. | Current call graph and live stats govern behavior. **Open/material.** |
| `EVC-DRIFT-004` | `wg assign --auto` comments/help say LLM; implementation performs deterministic historical max. A retained LLM assigner exists but has no coordinator caller. | Manual implementation governs; rename/document or restore a receipt-bound path. **Open.** |
| `EVC-DRIFT-005` | `record_assigner_evaluation` says downstream evaluation later supplies actual quality; only a placeholder 0.5 is recorded and no update call exists. | Comment overclaims. Retrospective assignment experiments are a separate mechanism dependent on legacy evaluation. **Open.** |
| `EVC-DRIFT-006` | Old graph tasks made review activity listable/accounted; current design says stages are first-class on parent/TUI, but current `wg list --all` shows none and `wg show` prints only object refs for v3. | Hiding schedulable rows is intentional; dedicated projection is incomplete. **Open UX/observability debt.** |
| `EVC-DRIFT-007` | Live raw Pi usage is present for every Done source, while graph accounting is null for 11/12 and current reviewer usage is discarded at its call site. | Raw and receipt evidence establish spend exists; graph/spend projections are incomplete. **Open; overlaps `fix-pi-accounting-review-visibility`.** |

## 5. Risks, gaps, and representation alternatives

### 5.1 Risks and gaps

| ID | Severity | Likelihood | Risk/gap | Needed evidence/control |
|---|---:|---|---|---|
| `EVC-RISK-001` | S1 | observed | Universal review produces no agency learning, so assignment/evolution can remain statistically empty while operators believe the loop is adaptive. | exactly-once learning projection and daemon E2E |
| `EVC-RISK-002` | S2 | observed | Reviewer outages, malformed attempts, retries, latency, and cost cannot be audited or budgeted from current receipts. | first-class review attempts with usage/failure taxonomy |
| `EVC-RISK-003` | S2 | likely | Superseded receipt objects become logically orphaned; repeated candidate submissions can hide failure trajectory and later distort naive learning. | parent review index plus terminal episode projection |
| `EVC-RISK-004` | S2 | possible | A naive bridge double-credits FLIP+eval, penalizes infrastructure as quality, or gives self-evaluators a gaming channel. | disjoint source/reviewer/route ledgers and calibration policy |
| `EVC-RISK-005` | S2 | likely | No agency composition snapshot at direct dispatch means later credit cannot be reconstructed from `agent-N`. | assignment receipt at reservation or explicit “uncomposed” class |
| `EVC-RISK-006` | S2 | possible | Legacy/manual and v3 histories are compared as if equivalent despite different candidate binding, score semantics, and evidence quality. | versioned import with confidence/eligibility partitions |
| `EVC-GAP-001` | S3 | observed | Current CLI lacks an ordinary query for review attempts/findings/accounting comparable to old task list/show. | `wg reviews`/`wg spend --reviews` and virtual projections |
| `EVC-GAP-002` | S3 | unknown | No live semantic reject or infrastructure-failure was injected into this graph; failure interpretability is source/test-based. | real worker submit-repair flow with persisted prior history |

### 5.2 Representation options

| Dimension | 1. Restore ordinary graph tasks | 2. Non-authoritative virtual task projections | 3. First-class review/evaluation graph node type | 4. Receipt/event ledger + dedicated projections |
|---|---|---|---|---|
| Lifecycle authority | **Bad by default:** task status/edges invite source coupling; could be neutered only with many exceptions | **Good:** display has no authority | **Good if type system forbids task transitions**; risk of generic graph APIs treating nodes alike | **Best:** append evidence; completion controller alone consumes |
| Retry/fencing | ordinary retry/generation semantics are wrong for reviewer attempts | projects ledger attempts and exact fences | can model separate review attempts/fences explicitly | explicit attempt sequence, route generation, idempotency keys, consume CAS |
| Candidate binding | possible, but ordinary dependencies encourage “latest parent” mistakes | as strong as backing record | strong if node identity includes manifest/source tuple | strong deterministic IDs and exact receipt bindings |
| Graph noise | highest; 2–3 rows per source plus failures/retries | user-selectable; zero storage noise | medium; nodes enlarge graph even if hidden | lowest canonical noise; projections on demand |
| Scheduling isolation | poor unless ordinary scheduler special-cases every review node | no scheduling semantics | good only with separate node scheduler | best: dedicated synchronous/agency execution lane |
| Auditability | familiar task logs/messages/accounting, but semantics are misleading | excellent navigation if it exposes all backing attempts | excellent queryability; schema migration more invasive | excellent append/replay history with purpose-built fields |
| Accounting | inherited task accounting, but mixes source/reviewer budgets | display only; depends on backing store | natural per-node usage/cost | natural per-attempt usage/cost and aggregate projections |
| Messaging | familiar but dangerous: messages may imply retry/work | aliases can route read-only discussion to source/review thread | could support typed review discussion without task wake authority | typed annotations/events; messages never schedule or mutate source |
| Replay/crash | ordinary task replay can duplicate calls or cross generations | depends on backing record | good with immutable node events | best with deterministic attempt/receipt IDs and event replay |
| Evolvability | easy but naive—each reviewer task looks like a source outcome | can show learning projection, no authority | explicit joins possible | best separation of observation ledger and exactly-once learning ledger |
| Legacy migration | graph rows fit, but revives obsolete semantics | can project old rows/files with badges | requires importing heterogeneous histories into new node schema | append provenance-preserving imports without inventing bindings |
| Operational complexity | superficially simple, historically high | low after backing ledger exists | highest schema/query/tooling change | medium; builds on lifecycle/CAS patterns already present |

**`[INFERENCE]` (high confidence)** Option 1 recovers familiar visibility but also recovers the exact wrong authority domain. It should be rejected. Option 3 is coherent, but a new graph node kind is justified only if cross-review graph queries or typed discussion must be persisted as graph topology; it is unnecessary for scheduling or lifecycle. The strongest near-term architecture is option 4 as canonical storage plus option 2 as the compatibility/human projection. It preserves the current valve while recovering the useful experience of old rows.

### 5.3 Edge cases the chosen representation must decide

1. **Exactly-once learning:** model invocations are at-least-once effects; learning projection must be exactly-once by a deterministic observation key and append/consume CAS. Legacy multi-file `record_evaluation` is not sufficient.
2. **Superseded candidates:** retain every candidate/reviewer attempt. Only receipts for the current exact candidate can gate. For learning, candidate rejects are diagnostic trajectory events; they must not each count as a separate completed task. Project one terminal generation episode, with the trajectory digest as context.
3. **Semantic reject vs infrastructure failure:** semantic reject may inform source-composition quality; unavailable, timeout, malformed, route drift, missing evidence, and budget failure inform reviewer/route reliability only. Neither may directly create source retry.
4. **Credit assignment:** terminal outcome/trajectory credits the source composition snapshotted at reservation. Reviewer quality is calibrated later against independent outcome/adjudication, not its own verdict. Assigner quality is a delayed reward linked to its assignment receipt. Evolver quality is measured on subsequent disjoint tasks using the evolved composition.
5. **Evaluator gaming/self-evolution:** system review tasks are excluded from source performance; no evaluator can author its own calibration outcome; model route/identity changes create separate cohorts; a reviewer cannot write learning or lifecycle events.
6. **Migration:** import legacy objects with `schema_origin`, raw digest, candidate-binding confidence, and eligibility class. Never infer a manifest/attempt that old evidence did not record. Legacy unbound evidence is historical/advisory and excluded from exact-candidate ranking by default.

## 6. Recommendations and human decisions

### 6.1 Recommended target: review event ledger plus projections

1. **`EVC-REC-001` — `[RECOMMENDATION]` (P0, completion/evaluation owners):** keep the v3 completion valve as the sole lifecycle consumer. Add a versioned append-only `ReviewRun`/`ReviewAttempt` ledger, keyed to `(graph, task, generation, source attempt/fence, manifest, requirements, reviewer kind, route generation)`. Minimum attempt fields: stable attempt ID/ordinal; selected handler/provider/model/reasoning/adapter version; started/completed times; model-reported route/stop reason; exact usage/cost; response and findings digests; semantic outcome or infrastructure failure taxonomy; inspected digests; retry/supersedes links; and valve-consumption event. **No ledger API may accept a source lifecycle transition.**

2. **`EVC-REC-002` — `[RECOMMENDATION]` (P0, UI/operations):** add `wg reviews [TASK]`, `wg review show ATTEMPT`, and review-aware spend/status/TUI panes. Optionally render stable `.flip-T@g/a#` and `.eval-T@g/a#` aliases, visibly marked **virtual / non-schedulable / no lifecycle authority**. `wg list --all` may remain source-task-only, but `--reviews` must make review activity discoverable. Findings and superseded attempts must be reachable without object-store spelunking.

3. **`EVC-REC-003` — `[RECOMMENDATION]` (P0, agency owners):** implement a separate exactly-once learning projector. Its input is immutable review/terminal/assignment evidence; its output is a versioned learning ledger and derived performance projections. Its deterministic key should include graph ID, task/generation, source-composition snapshot, terminal completion/failure receipt, candidate-trajectory digest, and projection-policy version. Replaying the projector returns the existing event. It must not call lifecycle, dispatch, retry, publication, messaging, or reviewer APIs.

4. **`EVC-REC-004` — `[RECOMMENDATION]` (P0, assignment owners):** create an assignment receipt whenever an agency composition is selected, or explicitly mark the attempt `uncomposed/direct-dispatch`. Snapshot agent, role, tradeoff, component/outcome IDs, selector, candidate set, score evidence, route/usage when model-based, and task/generation. Replace the 0.5 assigner placeholder with delayed, idempotent outcome attribution or label it event-count metadata that ranking never treats as quality.

5. **`EVC-REC-005` — `[RECOMMENDATION]` (P0, model/accounting owners):** persist exact reviewer usage returned by `LlmCallResult`; do not add it to source-worker tokens. Provide source, FLIP, eval, and total cost views. Repair source Pi projection separately so raw stream and graph accounting converge; do not use one defect to estimate the other.

6. **`EVC-REC-006` — `[RECOMMENDATION]` (P1, migration owners):** build a read-only legacy importer for synthetic task rows, legacy agency evaluation JSON, and lazy records. Append normalized envelopes with raw content digests and confidence; never rewrite original histories. Publish counts of exact-candidate, attempt-bound-but-no-manifest, and unbound legacy evidence. Default modern ranking/evolution to exact/declared-compatible partitions only.

7. **`EVC-REC-007` — `[RECOMMENDATION]` (P1, documentation owners):** publish one authority map that names “completion review receipt,” “candidate evaluation record,” and “agency performance evaluation” separately. State which commands create each, whether it can gate, whether it records cost, and whether it feeds agency learning. Deprecate `auto_assign` or restore a production receipt-bound effect; do not leave a no-op-looking flag beside dead assigner code.

### 6.2 Proposed exactly-once learning policy

**`[RECOMMENDATION]`** Preserve two levels rather than flattening all receipts into scores:

- **Candidate observation ledger:** one immutable event per `(manifest, reviewer kind, review attempt)`. It retains pass/reject/infrastructure outcome and cost. Superseded candidates remain queryable.
- **Generation learning episode:** one event after a terminal source disposition or explicit operator adjudication. It includes the ordered candidate-observation IDs and assigns one policy-versioned outcome to the snapshotted source composition. Multiple rejects before a final pass affect trajectory features but do not multiply `task_count`.
- **Reviewer calibration ledger:** a separate delayed event comparing a review prediction/verdict with independent ground truth, later defect/revoke, disjoint review, or human adjudication. A reviewer never scores itself.
- **Assignment reward ledger:** links assignment receipt to the terminal generation episode, with context partition and exploration propensity. This is the input to ranking/UCB, not the assigner's placeholder event.
- **Evolution trial ledger:** links an evolver-produced successor to future disjoint generation episodes. Evolver meta-evaluation cannot itself make that successor successful.

**`[CHARTER-RULE]` / authority boundary:** all five are observations/projections. The only actor that may request source completion remains the manifest completion controller after exact FLIP+eval and publication. A learning failure leaves a loud projection backlog; it never blocks or reverses Done unless a future human-approved policy explicitly makes learning durability a non-lifecycle operational gate.

### 6.3 Explicit human decision points

| Decision | Options | Recommended default |
|---|---|---|
| `EVC-DEC-001` Performance meaning | binary terminal success; reviewer score; human outcome; context-specific composite | versioned context-specific terminal episode; never equate reviewer confidence with ground truth |
| `EVC-DEC-002` Reject trajectory weight | every candidate counts; only final counts; one episode with trajectory | one episode with candidate trajectory features |
| `EVC-DEC-003` Reviewer identity/calibration | agency evaluator agent; exact route cohort; cryptographic actor; combination | stable reviewer policy ID + exact route cohort; agency persona only when explicitly bound |
| `EVC-DEC-004` Raw response retention | none; bounded digest only; encrypted bounded raw retention | digest + bounded normalized response by default; configurable protected raw retention |
| `EVC-DEC-005` Graph representation | virtual projections; first-class non-task node; ordinary task | ledger canonical + virtual projection; revisit non-task node only for topology/query needs |
| `EVC-DEC-006` Legacy eligibility | mix all history; exclude all; confidence-partitioned | preserve all, rank only declared-compatible confidence partitions by default |
| `EVC-DEC-007` Auto-assignment product | restore direct deterministic selection; restore LLM assignment; deprecate flag | either produce an attempt-bound assignment receipt before dispatch or fail loudly/deprecate |

### 6.4 Acceptance tests for the decision

1. **Lifecycle non-authority:** inject pass/reject/unavailable review and learning events after Done/Failed; source generation, attempt, fence, publication, and readiness do not change. Compile-time capabilities prevent review/evolver/projector code from submitting lifecycle transitions.
2. **Exact current candidate:** submit A, reject, submit B, pass. A remains inspectable and superseded; only B can open the valve. Changing requirements/output invalidates both B slots.
3. **Attempt history and accounting:** fail a reviewer by timeout, retry exact route, then pass. CLI shows both attempts, precise failure, start/end, usage/cost, and total; source accounting remains separate.
4. **Crash/replay exactly once:** crash after attempt start, receipt write, receipt link, Done, and learning-event write. Reconciliation performs no duplicate model call when a valid receipt exists and increments source composition `task_count` exactly once.
5. **Semantic/infrastructure separation:** semantic reject enters candidate trajectory; unavailable/malformed/route drift enters route reliability only. Neither directly retries/reopens source.
6. **Source credit:** an agency-assigned composition's terminal episode updates agent/role/tradeoff/components/outcome once. A direct-dispatch uncomposed attempt remains explicit and does not invent a composition.
7. **Assigner/evolver feedback:** assignment ranking and evolver both consume the same normalized episode partition. Assigner reward is delayed; evolver successor is evaluated only on subsequent disjoint work.
8. **Anti-gaming:** reviewer cannot update its own calibration, review its own calibration task, or earn source-composition credit. Repeated superseded submissions cannot multiply terminal `task_count`.
9. **Migration:** fixture imports terminal synthetic rows, bound and unbound legacy evaluations, and lazy records without mutation. Exact-candidate rows are eligible as configured; unbound rows remain visible and excluded by default.
10. **Human flow:** installed CLI/TUI shows a live FLIP attempt, failure findings, exact retry, eval, cost, superseded history, and the “virtual/non-authoritative” label. No ordinary task edge or worker slot is created.
11. **Live accounting canary:** every completed Pi source and both reviewer calls have non-null, deduplicated usage receipts; `source + FLIP + eval = total` in JSON and spend output.
12. **Documentation conformance:** help/manual/quickstart identify all three evaluation planes and `auto_assign`'s exact effect; a source search/daemon test proves the documented call path.

## 7. Evidence appendix

### 7.1 Snapshot, method, and limitations

**`[FACT]`** Static evidence was read at current checkout `e702437d`; production source outside the audit directory is byte-equivalent to pinned snapshot `b0892ea7`. Git history was queried across the repository for the dated cutover commits. Current source was traced from CLI/worker IPC through submit/review/Done, then separately through lazy records, legacy agency recording, assignment, retrospective inference, and evolution.

**`[VERIFIED]`** Live observations used the active graph at `/home/bot/wg/.wg` read-only. Initial worker-authorized `wg list/config/agency stats` calls were correctly refused by worker control. To avoid mutating worker authority while executing read-only operator diagnostics, the audit reran them under a scrubbed environment (`env -i` with only HOME/PATH/user/toolchain variables) and explicit `--dir`. Installed binary: `/home/bot/.cargo/bin/wg`, `wg 0.1.0`, SHA-256 `22a7b4f0b56a1091a66ba11073c6ba010e1f687720c1cdbcf29ab18f802958ba`. Live data is E6 evidence about this graph, not proof that the installed binary equals the pinned source build.

**`[UNCERTAINTY]`** No external reviewer was intentionally failed or induced to reject in the live audit graph. No legacy upgraded graph was migrated. No TUI was driven. The focused tests use deterministic reviewer fixtures, not a credentialed live model. Full Cargo, smoke, formal, and release suites were not run.

### 7.2 Executed source tests

Command, cwd `/home/bot/wg/.wg-worktrees/agent-17`, checkout `e702437d`, completed 2026-08-08, exit 0:

```sh
env -i HOME="$HOME" PATH="$PATH" USER="$USER" LOGNAME="$LOGNAME" \
  SHELL="$SHELL" TERM="$TERM" CARGO_HOME="$CARGO_HOME" \
  RUSTUP_HOME="$RUSTUP_HOME" \
  CARGO_TARGET_DIR=/tmp/wg-cargo-tmp-agent-17/audit-target \
  cargo test --test completion_review_valve \
             --test integration_agency_pipeline -- --test-threads=1
```

Results:

| Target | Result | Establishes / does not establish |
|---|---:|---|
| `completion_review_valve` | 9 passed | exact ordering/binding, receipt immutability, reject/incomplete/unavailable separation, no route fallback; fixture reviewers, not live silicon |
| `integration_agency_pipeline` | 34 passed, 5 ignored | publication creates no synthetic agency tasks/edges and config surfaces round-trip; ignored retired registry behavior is not verified |

The build emitted current dead-code warnings for `run_lightweight_assignment` and several legacy evaluator functions. Warnings corroborate, but do not alone prove, the repository-wide caller search.

### 7.3 Live graph commands and bounded results

Read-only list command, exit 0:

```sh
env -i HOME="$HOME" PATH="$PATH" USER="$USER" LOGNAME="$LOGNAME" \
  SHELL="$SHELL" TERM="$TERM" CARGO_HOME="$CARGO_HOME" \
  RUSTUP_HOME="$RUSTUP_HOME" \
  wg --dir /home/bot/wg/.wg list --all
```

Bounded result: 23 rows, 12 marked Done at the recheck; no ID matched `.(assign|flip|evaluate)-`. Four rows were then in progress (`.chat-0`, `audit-model-plane`, `setup-route-activation-ux`, and this audit).

Agency command, exit 0:

```sh
env -i HOME="$HOME" PATH="$PATH" USER="$USER" LOGNAME="$LOGNAME" \
  SHELL="$SHELL" TERM="$TERM" CARGO_HOME="$CARGO_HOME" \
  RUSTUP_HOME="$RUSTUP_HOME" \
  wg --dir /home/bot/wg/.wg agency stats
```

Bounded output:

```text
Components: 363
Outcomes: 103
Roles: 8
TradeoffConfigs: 206
Evaluations: 0
Avg score: -
No evaluations recorded yet.
```

A Python read-only join of `graph.jsonl`, agent `metadata.json`, each `raw_stream.jsonl`, and content-addressed completion objects revalidated the stronger field-level observation at `2026-08-08T14:32:15.780356+00:00`:

```text
done=12
done_token_usage_null=11
done_with_raw_pi_turn_end_usage=12
exact_manifest+requirements-bound_flip_and_eval_pass_pairs=12
evaluation_records_total=0
synthetic_rows=0
agency_evaluation_json=0
auto_assign=false
auto_evaluate=true
task_agent_nonnull=0
```

“Exact pair” required, for each Done task: both object refs existed; each object parsed as `ReviewReceipt`; receipt manifest equaled the selected manifest; requirements equaled the selected requirements object; kind was respectively `flip`/`eval`; verdict was `pass`; and route was nonempty. This check did not infer reviewer attempt or usage fields that the schema does not contain.

### 7.4 Current source evidence index

| Topic | Primary evidence |
|---|---|
| v3 receipt schema and valve | `src/completion_review.rs:1-388` |
| exact model adapter and discarded usage | `src/completion_review_model.rs:1-88`; `src/service/llm.rs:22-29,311-319` |
| source candidate selection and outcome linking | `src/commands/completion_submit.rs:208-482` |
| exact pair validation | `src/completion_task.rs:24-217` |
| publication-derived Done | `src/commands/completion_done.rs:33-267` |
| current CLI/worker dispatch | `src/main.rs:1274-1311`; `src/worker_cli.rs:325-360`; `src/commands/service/ipc.rs:888-918` |
| rich lazy record/attempt/source schema | `src/evaluation/mod.rs:30-235,491-706,792-906` |
| bounded/deep lane runners | `src/evaluation/bounded.rs:417-1014`; `src/evaluation/deep.rs:291-711,1810-1976` |
| rich record UI vs compact v3 refs | `src/commands/show.rs:937-953,1201-1290` |
| synthetic migration only | `src/commands/eval_scaffold.rs:1-86` |
| legacy performance cascade | `src/agency/types.rs:107-124,691-720`; `src/agency/eval.rs:49-201` |
| retrospective assignment learning | `src/agency/run_mode.rs:378-482` |
| manual deterministic assignment and placeholder | `src/commands/assign.rs:107-164,205-393` |
| dormant LLM assigner | `src/commands/service/assignment.rs:118-565` |
| evolver inputs | `src/agency/evolver.rs:120-224`; `src/commands/evolve/mod.rs:146-174` |
| current config defaults | `src/config.rs:4114-4213,4363-4382`; live `.wg/config.toml` rechecked separately |

### 7.5 Historical and normative evidence

History commands:

```sh
git log --all --date=short --pretty=format:'%h %ad %s' \
  --grep='evaluat\|FLIP\|flip\|assign\|completion valve\|satellite\|manifest' -i
git show --stat <commit>
git show <commit> -- <relevant paths>
```

Material commits:

- `f0c7ec74eeeaa352445db5ab15ae6ba95194e0ed` (2026-03-05), internal task usage accounting.
- `6bb0123a8ec337361989bd15af5f4ac5dcc38cb8` (2026-03-08), eager evaluator scaffolding.
- `71ab2e46e4853c78f50095906c95a243eede74d8` and `c683880580427b03f62baa707bf5fc57525abbc6` (2026-03-09), FLIP and lifecycle graph citizens.
- `9176849cac11a93134aa21f94db879923ffa002e` (2026-07-14), `FailedPendingEval` deadlock repair.
- `823dc578dcd204e2c4afe26a03e87a93ab5c6b71` (2026-07-19), pending-eval salvage.
- `8cce460f1bc3196140eba74152c4226a7e2389cd` (2026-07-26), Pi evaluation-plane design.
- `0dd48b929660401abd6755b2cc6856d6a9d56a25` and `11b4bdcb8aa2dc391ce573238d018dc1d4b09599` (2026-07-28), lazy records and required deep FLIP.
- `6ae37e9e0264c84064a7fe6c9e137c4c5749c419` and `6ac127a449fbbf3aeec15cd3e175063d2fb7694f` (2026-08-05), universal manifest valve design and implementation.
- `76fbe6142bc5423b55846049c41db619772532a7`, `922a5856a5afca58555b56662a53b54bcf39f734`, `4b789733c93af89d18ea4402c029c67f762425dd`, `8f129c55de3bfbce00ca5279ec0d34b90c601062`, `453c3e2bafda88095031e4aef52168e720e30f65`, and `6c3334604332c9b73bd296717d964914a64cf31a` (2026-08-07), authority deletion/retirement series.

Normative/current design evidence:

- `docs/design-simplified-task-lifecycle.md:1-42,520-610,742-901` — ratified separate evaluation domain, migration, visibility, tests, and no rescue authority.
- `docs/design-pi-evaluation-plane.md:1-88,215-313,575-699` — rich record/attempt/accounting design and legacy handling.
- `docs/design-worker-owned-universal-review.md:1-179` — implemented current completion authority.
- `docs/plans/simple-worker-owned-lean-convergence.md:1-244,429-467` — implementation cutover and canary contract.
- `docs/audit/2026-08-08-worksgood-system/13-agency-evaluation-chat.md:1-187,314-316,349-395` — predecessor leaf audit; findings were rechecked against primary source/live evidence rather than copied.

### 7.6 Caller-search evidence

```text
rg "record_evaluation_with_inference|record_evaluation\\(" src --glob '*.rs'
  -> agency/eval.rs definitions/tests
  -> commands/assign.rs placeholder
  -> commands/evaluate.rs legacy/manual
  -> commands/evolve/mod.rs evolver meta-evaluation
  -> no completion_* or evaluation/** caller

rg "run_lightweight_assignment" src --glob '*.rs'
  -> definition/tests in commands/service/assignment.rs
  -> no coordinator/publish/main/worker caller

rg "mint_for_candidate|run_one_pending|completion_submit::run|completion_done::run" src
  -> lazy records remain in historical finalization/Done and agency-lane paths
  -> current CLI/worker completion dispatches directly to v3 submit/Done
```

**`[INFERENCE]`** Static caller absence plus dead-code diagnostics and live zero-count stores is strong reachability evidence. It remains falsifiable: a daemon E2E can publish an unassigned task with `auto_assign=true`, invoke no manual assignment, and prove either a durable composition/assignment receipt or no effect; a second E2E can complete a v3 task and assert exactly one agency learning event.
