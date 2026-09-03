# Receipt-backed adaptive agency loop

**Status:** implementation design

**Decision:** keep the worker-owned completion controller as the only ordinary source-lifecycle authority; represent assignment, candidate review, scoring, calibration, reward, and evolution as immutable evidence plus derived projections. Never restore `.assign-*`, `.flip-*`, `.evaluate-*`, or `.evolve-*` as schedulable graph tasks.

**Starting point:** [Evaluation cutover: interpretability and agency evolvability](audit/2026-08-08-worksgood-system/23-evaluation-evolvability-cutover.md)

## 1. Problem and scope

WG must recover the useful feedback loop:

```text
assign → execute → FLIP/evaluate → learn → improve
```

without recovering the old coupling:

```text
.assign-T → T → .flip-T → .evaluate-T
```

The old rows made internal calls visible, but also gave them task status, dependency edges, worker admission, retry, and lifecycle implications. That caused eager work, graph inflation, stale-attempt review, and `PendingEval` deadlocks. Visibility is worth retaining; graph-task authority is not.

The current code already contains most of the safe foundation:

- attempt/candidate bindings and immutable completion-review activity (`src/completion_review.rs:111-173`, `src/completion_review.rs:700-748`);
- semantic receipt replay without repeating a valid exact-route judgment (`src/completion_review.rs:938-966`);
- rich, older lazy evaluation attempts (`src/evaluation/mod.rs:112-224`);
- exactly-once terminal observations (`src/terminal_observation.rs:141-174`, `src/terminal_observation.rs:1001-1082`);
- receipt-backed scored outcome evaluation and idempotent performance repair (`src/agency/types.rs:727-776`, `src/agency/eval.rs:405-437`);
- legacy retrospective assignment learning and evolver stores (`src/agency/run_mode.rs:378-482`, `src/agency/evolver.rs:120-164`); and
- initial virtual review rows, detailed task views, TUI history, and separate review spend (`src/commands/list.rs:254-318`, `src/commands/show.rs:1023-1073`, `src/tui/viz_viewer/state.rs:8619-8677`, `src/commands/spend.rs:34-79`).

The missing piece is one coherent authority and identity model joining those stores. This design supplies it.

### 1.1 Goals

1. Bind every source attempt to either an immutable assignment receipt or an explicit `uncomposed` receipt.
2. Preserve every candidate FLIP/eval invocation, including failures, route, timing, findings, usage/cost, and supersession.
3. Project familiar `.flip-*` / `.evaluate-*` names without making tasks.
4. Create exactly one learning episode for a terminal source generation.
5. Keep semantic source quality, reviewer calibration, and route/infrastructure reliability separate.
6. Deliver delayed, idempotent assignment reward and evolver input.
7. Allow bounded automatic assignment and evolution without ordinary graph edges or source-lifecycle authority.
8. Import historical evidence without inventing missing candidate, attempt, or composition identity.

### 1.2 Non-goals

- Reviewers are not workers and do not own source worktrees.
- A completion-review verdict is not automatically a numeric quality score or ground truth.
- An external score is not completion acceptance.
- Candidate rejections do not become separate completed tasks.
- The design does not promise exactly-once external model execution. It promises create-once durable attempts, receipts, rewards, and projections; an external call interrupted before its receipt is durable may have run.
- The design does not add a general graph-node type for reviews.

## 2. Vocabulary: four operations that must remain distinct

| Operation | Question | Canonical evidence | May affect ordinary source lifecycle? | Feeds learning? |
|---|---|---|---|---|
| **Completion review** | “May this exact candidate satisfy the generation's snapshotted completion policy?” | exact FLIP/eval terminal receipts plus a controller-authored consumption receipt | **Only indirectly.** The completion controller, never the reviewer, consumes an exact current receipt and applies policy. | Receipt is trajectory evidence only; it is not ground truth. |
| **Candidate evaluation** | “What did a reviewer observe about this immutable candidate?” | append-only candidate/review-run/attempt events | No. Advisory and superseded observations cannot transition, retry, publish, or block a source. | Diagnostic trajectory; later reviewer calibration when independent truth exists. |
| **Scored outcome evaluation** | “How good was the already-terminal generation?” | receipt-backed `OutcomeAssessment`, normally from `wg evaluate run` | No. It observes a terminal episode. | Supplies one effective score for the one episode; never increments task count again. |
| **External adjudication** | “What does an authorized external actor assert?” | signed/actor-attributed, reasoned `AdjudicationReceipt` with scope and exact binding | Only the separately named **acceptance adjudication** command may satisfy lifecycle policy, under operator authority. Outcome adjudication never does. | Outcome adjudication may supersede an assessment under a versioned policy. |

Two external adjudication scopes are deliberately different:

- `acceptance`: `wg done --operator-accept --reason ...` and the legacy `wg migrate evaluation-cutover --accept ...` escape hatch. It is accepted only by the lifecycle controller. The normal escape is generation/attempt/fence-bound and binds a candidate when one is verifiable; when bookkeeping is unavailable it records that evidence gap rather than inventing a candidate. The legacy cutover escape remains exact-candidate-bound.
- `outcome`: `wg evaluate record ...`. It attaches independent ground truth or a human score to a terminal episode and cannot complete, fail, retry, reopen, or publish the task.

The CLI and JSON schemas must always print `operation_kind` / `adjudication_scope`; the word “evaluation” alone is insufficient.

## 3. Authority map and proof boundary

### 3.1 Capabilities

| Actor/module | Input capability | Write capability | Explicitly absent |
|---|---|---|---|
| Source worker | own attempt, worktree, completion manifest API | candidate artifacts and a submit request | no reviewer ledger mutation; no terminal transition except an own-attempt request |
| Assignment selector | immutable task/admission snapshot and read-only performance view | return one `AssignmentDecision` to dispatcher | graph writer, lifecycle sink, task edge writer, spawn, retry, message, publication |
| Dispatcher admission controller | ready snapshot and selector decision | append assignment receipt; reserve/spawn one attempt | completion, source success/failure, review consumption |
| FLIP/eval adapter | one digest-verified `ResolvedReviewBundle` and exact route | provider progress/result bytes to a `ReviewAdapterSink` | candidate selection/supersession/consumption sinks; graph path, source worktree, WG token, task commands, lifecycle/dispatch/publication APIs |
| Candidate-ledger linker | authenticated immutable event/receipt bytes from typed sinks | create-new ledger objects/index links | task status, edges, readiness, retry, route fallback |
| Completion controller | current fenced source snapshot, exact current candidate, policy, receipts, publication truth | review-consumption receipt and lifecycle request | model execution and source editing |
| Terminal episode projector | read-only terminal/lifecycle and immutable receipt resolvers | create-new `LearningEpisode` | graph mutation, lifecycle, dispatch, review execution, publication, messaging |
| Outcome scorer | verified terminal episode/evidence bundle and exact route | one `OutcomeAssessment` | graph/source/worktree/tools; no self-authored source attribution |
| Learning projector | immutable episodes, assessments, assignment receipts, adjudications | rebuild derived performance/reward/calibration views and projector checkpoint | graph/lifecycle/dispatch/review/publication APIs |
| Evolver | eligible episode/reward view | immutable proposal | direct source/task mutation, graph edges, lifecycle, its own trial score |
| Evolver applier | explicit operator or separately configured agency-store authority | agency primitive successor + apply receipt | source lifecycle and graph topology |
| Read-only projector / TUI | verified ledgers and derived views | display cache only | every authoritative write API |

Implementation must express these as narrow constructor-injected traits. In particular, the reviewer, terminal projector, learning projector, and TUI crates/modules must not receive a mutable `WorkGraph`/`Task`, `LifecycleStore`, `TransitionRequest`, dispatcher handle, `Command`, or publication handle. A read-side resolver returns immutable snapshots/bytes, not a writable graph or filesystem root.

Lifecycle mutation entry points require sealed, unforgeable capability tokens: `AttemptReservationAuthority` for the dispatcher and `CompletionAuthority` / `OperatorAcceptanceAuthority` for the lifecycle controller. Their constructors are private to the lifecycle composition root. The adaptive package depends only on read-side types and event sinks; the lifecycle writer package must not be in its dependency graph. Compile-fail tests try to construct/import each forbidden token, while runtime tests hash graph/publication bytes before and after every reviewer/projector action. Compile-time dependency direction plus unforgeable tokens is the proof; a naming convention or code-review promise is not.

Compile-fail capability tests are part of acceptance (§16).

### 3.2 Why a review can gate without reviewer authority

A reviewer appends a statement: “for binding B, my semantic result is R.” It cannot apply R. The completion controller independently:

1. reloads and verifies the receipt bytes;
2. requires exact graph/task/generation/attempt/fence/candidate/manifest/requirements/output/policy/route binding;
3. requires that the candidate is still current;
4. applies the generation's snapshotted completion policy; and
5. alone requests the fenced lifecycle transition or records a completion blocker.

Thus a semantic reject can be an input to a configured completion valve, but the reviewer cannot directly block or transition the source. Removing or crashing every projector also cannot open or close that valve.

### 3.3 Structural invariants

1. Adaptive records never implement `Node` or `Task`, have no `Status`, `after`, `before`, `assigned`, worktree, task inbox, cron, or task retry count.
2. No adaptive record ID is accepted by `wg done`, `fail`, `retry`, `requeue`, `wait`, `publish`, `add-dep`, `rm-dep`, `msg send`, or `artifact`.
3. No adaptive writer invokes a generic command runner. Model adapters receive a purpose-built provider transport and a zero-tool/bounded observation capability manifest.
4. Source readiness ignores adaptive backlogs. Only the completion controller's snapshotted completion policy can hold finalization; no ordinary graph edge represents that hold.
5. Projector errors produce a loud backlog/health condition. They never reverse terminal state and never block dependents.
6. Review retry changes only review attempt state. Source retry remains an explicit source-owner/operator lifecycle operation.
7. Assignment selection has a bounded deadline and fail-open-to-**uncomposed**, never fail-open-to-an unrecorded composition.
8. `task_count` is the count of distinct eligible `episode_id`s, not receipt, candidate, assessment, or reward rows.

## 4. Canonical storage and identity

Canonical files live beneath `.wg/agency/adaptive/v1/`:

```text
assignment-selection/<selection-id>.json
assignment-receipts/<receipt-id>.json
candidate-ledger/events/<event-id>.json
candidate-ledger/index/<task-key>.jsonl       # rebuildable append index
trajectory-seals/<seal-id>.json
terminal-episodes/<episode-id>.json
outcome-assessments/<assessment-id>.json
external-adjudications/<adjudication-id>.json
reviewer-calibration/<calibration-id>.json
assignment-rewards/<reward-id>.json
performance-projections/<policy>/<subject>.json
projector-checkpoints/<projector>/<policy>.json
evolution-runs/<run-id>.json
legacy-import/<import-id>.json
```

Every canonical object is canonical JSON, create-new, fsynced, and content/identity verified on load. The object is the commit point; indexes and mutable projections are rebuildable. Conflicting bytes for one deterministic ID are corruption and fail closed. Mutable `Task.completion_review_activity`, `Task.evaluation_records`, `PerformanceRecord`, statistics, and TUI caches are projections, not authority.

IDs are BLAKE3 over a domain separator plus canonical identity fields. Timestamps and mutable display prose never participate unless explicitly shown below.

`graph_identity` is the repository/graph identity digest, not a path. It prevents two graphs with the same task IDs from joining evidence accidentally.

## 5. Assignment: receipt or explicit uncomposed dispatch

### 5.1 Schema

```rust
struct AssignmentSelectionStartedV1 {
    schema: u16,
    selection_id: String,
    graph_identity: String,
    task_id: String,
    generation: u64,
    proposed_attempt_id: String,
    proposed_attempt_fence: u64,
    admission_snapshot_digest: String,
    selector_policy_digest: String,
    started_at: String,
    absolute_deadline: String,
}

struct AssignmentReceiptV1 {
    schema: u16,
    receipt_id: String,
    selection_id: Option<String>,
    graph_identity: String,
    task_id: String,
    generation: u64,
    attempt_id: String,
    attempt_fence: u64,
    admission_snapshot_digest: String,
    decision: AssignmentDecision,
    selector: SelectorSnapshot,
    candidate_set_digest: Option<String>,
    selected_composition: Option<CompositionSnapshot>,
    experiment: Option<ExperimentSnapshot>,
    route: Option<RouteSnapshot>,
    usage: Option<Usage>,
    started_at: String,
    completed_at: String,
    failure: Option<InfrastructureFailure>,
}

enum AssignmentDecision {
    Explicit,
    Automatic,
    Uncomposed { reason: UncomposedReason },
}

struct CompositionSnapshot {
    agent_id: String,
    role_id: String,
    tradeoff_id: String,
    component_ids: Vec<String>,
    outcome_id: String,
    composition_digest: String,
}
```

`receipt_id = b3("wg-assignment-receipt-v1\0" || graph_identity || task_id || generation || attempt_id || attempt_fence || admission_snapshot_digest)`.

The receipt snapshots the exact composition; later edits to role/agent YAML do not rewrite history. An automatic selector also records candidate scores, selection policy/propensity, exploration parameters, and the digest of the candidate set. Model-backed selection records its exact route/reasoning/adapter, response digest, timing, and provider-reported usage/cost.

### 5.2 Admission flow

1. Under the dispatcher admission snapshot, derive the next attempt identity/fence and `admission_snapshot_digest`.
2. If an explicit assignment intent exists, resolve and snapshot it.
3. Else if automatic assignment is `prefer`, create-once `AssignmentSelectionStartedV1` **before** the external call. Its deterministic ID binds the graph/task/generation/proposed attempt/admission snapshot/selector policy, and its wall-clock `absolute_deadline` never moves on restart. A per-selection lease permits at most one live caller.
4. Invoke the selector outside the graph lock. A live lease may be recovered before the persisted absolute deadline; after that deadline replay must not invoke again and instead creates the one `Uncomposed` receipt.
5. On timeout, malformed output, unavailable route, no eligible composition, or crash, create `Uncomposed` with a structured reason. Infrastructure failure is recorded against selector/route reliability, not source quality.
6. Reacquire the lock. If the task snapshot/next-attempt tuple changed, retain the receipt as unused/superseded and restart admission. Otherwise reserve the attempt with the `assignment_receipt_id` in the reservation event and spawn.
7. A crash after receipt creation but before reservation reuses that exact receipt if the snapshot still matches. A crash after reservation recovers the one referenced receipt. Repeated crashes cannot reset the selection deadline or hold dispatch indefinitely.

There is no `.assign-T` task and no `T after .assign-T` edge. Automatic assignment may add bounded admission latency, but it cannot leave a persistent readiness blocker. `off` and every failure path dispatch `uncomposed` explicitly. There is no hidden “required auto assignment” mode in v1.

`wg assign T A`, `wg assign T --auto`, and `wg assign T --clear` continue to set/select/clear the explicit **next-attempt intent**. They do not fabricate an attempt receipt. The dispatcher creates the receipt when a real attempt is reserved. `--auto` records which deterministic or model-backed selector actually ran and must not claim “LLM” when it used historical ranking. `wg match T` is the read-only preview. A historical `auto_assign=true` does not activate daemon selection silently during migration; the operator must explicitly set `agency.assignment.mode="prefer"` after its canary.

## 6. Append-only candidate FLIP/eval ledger

### 6.1 Source and candidate binding

```rust
struct SourceBindingV1 {
    graph_identity: String,
    task_id: String,
    generation: u64,
    source_attempt_id: String,
    source_fence: u64,
    assignment_receipt_id: String,
}

struct CandidateBindingV1 {
    source: SourceBindingV1,
    candidate_sequence: u64,
    manifest_digest: String,
    requirements_digest: String,
    source_revision: String,
    dependency_revision_digest: String,
    output_digests: Vec<String>,
    validation_evidence_digest: String,
}
```

Unordered arrays are canonicalized before hashing. A changed output, requirement, dependency revision, manifest, source attempt/fence, or assignment receipt is a different binding.

### 6.2 Event union

```rust
enum CandidateLedgerEventV1 {
    CandidateSelected {
        event_id: String,
        binding: CandidateBindingV1,
        selected_at: String,
    },
    ReviewAttemptStarted {
        event_id: String,
        review_run_id: String,
        review_attempt_id: String,
        ordinal: u32,
        reviewer_kind: Flip | Eval,
        product: Completion | Bounded | DeepReadonly,
        binding: CandidateBindingV1,
        policy: PolicySnapshot,
        route: RouteSnapshot,
        capability_manifest_digest: String,
        started_at: TimeEvidence,
        lease_expires_at: Option<String>,
        supersedes_attempt: Option<String>,
    },
    ReviewAttemptFinished {
        event_id: String,
        started_event_id: String,
        review_run_id: String,
        review_attempt_id: String,
        binding: CandidateBindingV1,
        policy_digest: String,
        route_digest: String,
        capability_manifest_digest: String,
        outcome: ReviewOutcome,
        completed_at: String,
        duration_ms: u64,
        response_digest: Option<String>,
        findings_digest: Option<String>,
        inspected_output_digests: Vec<String>,
        usage: Option<Usage>,
        stop_reason: Option<String>,
        provider_reported_route: Option<String>,
        receipt_digest: String,
    },
    CandidateSuperseded {
        event_id: String,
        binding_digest: String,
        superseded_by_binding_digest: String,
        superseded_at: String,
        reason: String,
    },
    ReviewConsumed {
        event_id: String,
        review_attempt_id: String,
        binding_digest: String,
        receipt_digest: String,
        controller_policy_digest: String,
        source_fence: u64,
        consumed_at: String,
        effect: AcceptedEvidence | RejectedEvidence | AdvisoryOnly,
    },
}
```

`RouteSnapshot` contains handler, provider, model, exact route, reasoning, adapter and adapter version, route generation, and route digest. `Usage` contains input/output/cache-read/cache-write/total tokens and provider-reported cost with currency/source. Cost is never silently estimated in canonical attempt evidence; an optional derived estimate is labeled `estimated`.

`ReviewOutcome` is a tagged union:

```text
semantic.pass
semantic.reject
semantic.inconclusive
infrastructure.timeout
infrastructure.adapter_unavailable
infrastructure.process_failed
infrastructure.malformed_output
infrastructure.route_drift
infrastructure.evidence_unavailable
infrastructure.insufficient_evidence
infrastructure.budget_exceeded
infrastructure.interrupted_unknown
```

The event enum is a serialization union, not one writer interface. Four sealed sinks authorize disjoint variants: `CandidateSelectionSink` writes `CandidateSelected/Superseded`; `ReviewAttemptSink` reserves starts and accepts only parsed adapter progress/results; `ReviewConsumptionSink` writes `ReviewConsumed`; `LegacyImportSink` writes confidence-labeled inert imports. Each event carries the author/capability kind in its signed or local-authenticated envelope, and the linker rejects a variant authored by the wrong sink. A model adapter never receives the selection or consumption sinks and cannot manufacture those controller events.

`TimeEvidence` is `Observed(rfc3339)` or `UnknownLegacy`. A new live attempt requires `Observed` plus a lease expiry; an import may use `UnknownLegacy` and has no runnable lease. Migration never fabricates an exact start time.

The type system prevents an infrastructure outcome from carrying a semantic pass/reject. Findings are bounded, immutable, and referenced by digest. Raw provider output is digest-only by default; protected bounded retention is a separate policy.

### 6.3 Attempt and replay identity

- `review_run_id` is deterministic over `(candidate binding, reviewer kind, product, policy digest, route generation)`.
- `review_attempt_id` is deterministic over `(review_run_id, ordinal)`.
- Ordinal reservation and `ReviewAttemptStarted` creation happen under a per-run ledger lock.
- `ReviewAttemptFinished` is create-once for that attempt. Identical replay is a no-op; conflicting content is quarantined.
- A valid finished receipt found after a crash is linked and never invokes the model again.
- An expired start with no durable finished receipt becomes `infrastructure.interrupted_unknown`; a retry receives the next ordinal. The provider may have executed, so unknown usage/cost stays unknown rather than being invented.
- Retry repeats the persisted route. Explicit reroute creates a new route generation and review run; it never relabels earlier output.
- Semantic outcomes are never auto-retried to obtain a preferred answer. Only policy-declared infrastructure classes have bounded retries.

### 6.4 Superseded candidates

Every selected candidate receives an explicit selection event. Selecting candidate B appends `CandidateSuperseded(A,B)` before B can be consumed. Consequences:

1. A's attempts/findings/cost remain queryable forever.
2. A's pass cannot satisfy B; a late A result is retained but never consumed.
3. The current completion controller may consume only a receipt whose complete binding equals current B and current source fence.
4. Restoring a previously used route may reuse a semantic receipt only when candidate binding, reviewer kind, product, policy, capability manifest, and route generation are exact.
5. Candidate rejects are trajectory features inside one eventual generation episode. Ten rejected candidates followed by one accepted candidate still produce one source `task_count` observation.
6. Infrastructure failures never become candidate-quality features. They feed route/reviewer reliability only.

The current `ReviewReceipt` remains the first compatible `ReviewAttemptFinished.receipt_digest`. `Task.completion_review_activity` becomes a verified cache of ledger terminal events during rollout.

## 7. One terminal generation learning episode

### 7.1 Eligibility and schema

A terminal generation is one lifecycle generation with exactly one accepted terminal event (`Done`, terminal `Failed`, or `Abandoned/Cancelled`) under lifecycle first-terminal-wins rules. A retry/reopen creates a greater generation and may later create its own episode. A rejected candidate inside the same generation does not.

```rust
struct LearningEpisodeV1 {
    schema: u16,
    episode_id: String,
    policy_version: String,
    graph_identity: String,
    task_id: String,
    generation: u64,
    terminal_event_id: String,
    terminal_disposition: Done | Failed | Abandoned | Cancelled,
    source_attempt_id: Option<String>,
    source_fence: Option<u64>,
    assignment_provenance: AssignmentProvenance,
    terminal_provenance: TerminalProvenance,
    terminal_candidate_binding: Option<CandidateBindingV1>,
    trajectory_seal_id: String,
    trajectory_event_ids: Vec<String>,
    trajectory_digest: String,
    semantic_trajectory: SemanticTrajectory,
    infrastructure_summary: InfrastructureSummary,
    source_quality_eligibility: Eligible | Ineligible { reason: String },
    created_at: String,
}

enum AssignmentProvenance {
    BoundReceipt(String),
    NoAttempt,
    ImportedUncomposed(String),
    UnknownLegacy { raw_digest: String },
}

enum TerminalProvenance {
    CompletionReceipt(String),
    FailureEvent(String),
    CancellationEvent(String),
    OperatorAcceptance(String),
    UnknownLegacy { raw_digest: String },
}
```

Before any terminal lifecycle commit, the lifecycle controller writes a create-once `TrajectorySealV1 { task, generation, terminal_event_id, candidate_ledger_head, ordered_event_ids, trajectory_digest }` and includes its ID in the terminal request/receipt. The seal and terminal event share the controller's idempotency key. `trajectory_event_ids` in the episode comes **only** from this verified seal, never from a time comparison or a fresh cross-store scan. A candidate event linked after the seal is late evidence: it remains usable for reliability/calibration but cannot change the episode. A crash after seal/before terminal reuses the seal; a conflicting seal for the same terminal event is corruption.

`episode_id = b3("wg-learning-episode-v1\0" || policy_version || graph_identity || task_id || generation || terminal_event_id)`.

The create-new episode is the commit point. Reconciliation returns the same episode. A different terminal event for the same generation is a lifecycle corruption, not a second episode. A generation cancelled before any attempt has neither assignment nor terminal completion receipt and is explicitly ineligible; a post-attempt terminal generation must resolve the attempt's assignment/uncomposed receipt. Migration may create a confidence-labeled legacy-uncomposed receipt, but never invents a composition.

### 7.2 Source-quality eligibility

- `Done` with ordinary receipt-backed completion is eligible.
- A terminal semantic implementation failure may be eligible for negative outcome attribution if it proves that source work ran and the failure is source-caused.
- launch failure, provider outage, disk full, admission deferral, reviewer outage, route drift, malformed reviewer output, interrupted-unknown, operator cancellation, and missing attribution are ineligible for source-quality scoring by default.
- operator-accepted completion creates an episode but is marked `operator_accepted`; it is not silently equivalent to ordinarily verified publication.
- `uncomposed` episodes remain valid operational observations but update no invented agent/role/component.

The projector records why an episode is ineligible. It never drops it.

### 7.3 Exactly-once derived performance

Canonical performance is a fold, not a sequence of blind increments:

```text
task_count(subject, partition) =
  count(distinct eligible episode_id attributed to subject in partition)

avg_score(subject, partition) =
  average(one effective outcome score per eligible episode_id)
```

The learning projector writes a `PerformanceProjectionReceipt` keyed by `(projector policy, subject, partition, ledger head/input digest)`. On crash it rebuilds from immutable episodes and effective assessments. It does **not** call legacy `update_performance` once for FLIP, once for eval, or once per candidate. When a later adjudication changes the effective score, the fold replaces the score for that episode; `task_count` remains one.

Existing `PerformanceRecord` YAML remains a compatibility cache until consumers read the canonical projection directly.

## 8. Scored outcomes, external truth, calibration, and reliability

### 8.1 Outcome assessment

`wg evaluate run T` first re-verifies the terminal episode, completion/publication evidence, and anti-self-scoring rules. It then emits:

```rust
struct OutcomeAssessmentV1 {
    assessment_id: String,
    episode_id: String,
    scorer_policy_id: String,
    scorer_principal: String,
    route: RouteSnapshot,
    evidence_digest: String,
    score: f64,
    dimensions: BTreeMap<String, f64>,
    notes_digest: String,
    usage: Option<Usage>,
    usage_state: Reported | Unavailable { reason: String },
    independence: Independent | NonIndependent { reasons: Vec<String> },
    created_at: String,
}
```

The deterministic assessment ID includes episode, scorer policy, evidence digest, and route generation. Re-running exact input reuses it. Rerouting creates a separate assessment; the outcome-resolution policy selects one and retains the others. Missing provider usage is explicit `Unavailable` and produces unknown cost; it never becomes a zero-cost receipt.

The versioned effective-outcome resolver uses, in order: a current trusted external outcome adjudication; an independent scored assessment; then a policy-defined categorical terminal outcome only for an eligible, proven source-caused terminal failure. A successful `Done` without an independent score remains `unscored`, not an invented `1.0`. Infrastructure and cancellation dispositions never synthesize `0.0`. The resolver emits a create-new selection/supersession receipt so a changed effective outcome is auditable.

### 8.2 Anti-self-scoring

An assessment is `Independent` only if all are true:

1. scorer principal is not the source worker, assigned composition, assigner, or evolver that produced the evaluated composition;
2. scorer policy/route cohort is not one of the completion reviewers being calibrated from this assessment;
3. scorer context/session is fresh and contains only the immutable terminal bundle;
4. no scorer tool can mutate graph/source/agency performance; and
5. provenance is complete and exact.

Same-provider is allowed only when principal, policy, context, and route cohort are independently configured; same exact route/model cohort as source is labeled non-independent by default. Non-independent scores remain visible but are excluded from automatic assignment reward, reviewer calibration, and evolver input unless an operator chooses a new explicit policy version. An evaluator can never improve its own calibration by emitting a high outcome score.

Route roles are distinct: `completion_flip`, `completion_eval`, and `outcome_scorer`. `outcome_scorer` has no fallback to either completion reviewer; when it is absent, automatic scoring stays disabled and manual scoring fails with a remediation command. This prevents the current shared `Evaluator` role from making independent learning unreachable by default.

### 8.3 External outcome adjudication

`wg evaluate record` targets an `episode_id` (task shorthand resolves exactly one terminal generation) and writes:

```rust
struct AdjudicationReceiptV1 {
    adjudication_id: String,
    scope: Outcome,
    episode_id: String,
    candidate_binding: Option<CandidateBindingV1>,
    authentication: AdjudicationAuthentication,
    verified_issuer: String,
    trust_policy_digest: String,
    authority: Human | SignedSystem | ImportedLegacy,
    decision: Score | Defect | Exoneration | Unknown,
    score: Option<f64>,
    dimensions: BTreeMap<String, f64>,
    reason: String,
    evidence_refs: Vec<String>,
    supersedes: Option<String>,
    source_event_id: String,
    created_at: String,
}

enum AdjudicationAuthentication {
    LocalOperator { authorization_receipt: String },
    FedSigned {
        issuer: String,
        issuer_key_id: String,
        signature: String,
        envelope_digest: String,
    },
    LegacyImport { import_receipt: String },
}
```

`adjudication_id` is deterministic over `(scope, episode, verified issuer, authority, source_event_id)`. Signed systems must supply their stable event ID. For a human CLI record, `source_event_id` defaults to the digest of the canonical score/dimensions/reason/evidence input, so exact command replay is idempotent; `--idempotency-key` names an intentionally distinct assertion.

The adjudication verifier derives the actor from the locally authenticated operator receipt or a WG-Fed signed envelope and verifies the typed authentication against the snapshotted trust policy. `verified_issuer` and `authority` must equal the derived result; it never trusts caller-supplied identity/authority labels. Imported legacy evidence uses an import attestation and remains ineligible by default. `candidate_binding` is required for claims about a superseded candidate and optional for a generation-level outcome.

A trusted human/signed-system adjudication can supersede a model assessment under the configured resolver. It cannot rewrite the assessment or episode. Every supersession is explicit. Imported legacy assertions are excluded by default unless their binding confidence satisfies policy.

### 8.4 Reviewer calibration

Calibration is a delayed comparison, never a reviewer's own verdict treated as truth:

```rust
struct ReviewerCalibrationV1 {
    calibration_id: String,
    review_attempt_id: String,
    independent_outcome_id: String,
    reviewer_policy_id: String,
    route_cohort_digest: String,
    prediction: Pass | Reject | Inconclusive,
    observed_outcome: f64,
    brier_component: Option<f64>,
    agreement: Agreement | Disagreement | Unscorable,
    created_at: String,
}
```

Only independent scored outcomes, trusted external outcome adjudication, later defect/revoke evidence, or a disjoint adjudicator can calibrate. Completion pass/reject alone is not ground truth. Candidate attempts from superseded candidates may be calibrated when the external truth names that exact candidate; otherwise they stay unscored diagnostics.

### 8.5 Infrastructure reliability

A separate fold groups attempts by adapter/executor/provider/model/reasoning/route generation and reports:

- starts, settled calls, timeout/malformed/route-drift/unavailable/interrupted counts;
- p50/p95 duration;
- provider-reported tokens/cost and unknown-cost attempts;
- retry and recovery rate; and
- evidence/bundle failure rate separately from provider failure.

These metrics never alter source quality. They may inform route selection only through a separately versioned route-health policy; they cannot cause fallback inside an already pinned attempt.

## 9. Delayed assignment reward and evolver input

### 9.1 Assignment reward

```rust
struct AssignmentRewardV1 {
    reward_id: String,
    assignment_receipt_id: String,
    episode_id: String,
    effective_outcome_id: String,
    reward_policy_version: String,
    context_partition: String,
    propensity: Option<f64>,
    reward: f64,
    eligible: bool,
    exclusion_reason: Option<String>,
    supersedes: Option<String>,
    created_at: String,
}
```

No placeholder `0.5` is recorded. Reward appears only after one eligible effective outcome exists. The active reward is deterministic for the assignment/episode/policy/effective-outcome tuple. If trusted external truth supersedes the outcome, a new reward supersedes the old one; ranking folds only the active reward and still counts the episode once.

Automatic ranking/UCB reads assignment rewards in the same context partition used by selection. It never reads reviewer pass rate as source reward. Uncomposed attempts have no composition reward.

### 9.2 Evolver input and trials

The evolver consumes an immutable input manifest containing distinct eligible episode IDs, effective outcome IDs, active assignment reward IDs, context partitions, and prior evolution-trial IDs. Trigger thresholds count new eligible episodes, not files or reviewer attempts.

An evolution run writes proposals first:

```text
EvolutionRunReceipt {
  run_id, policy, input_manifest_digest, input_episode_ids,
  proposed_operations, route, timing, usage, status
}
```

Applying a proposal requires `wg evolve apply` by an operator or an explicitly configured agency-store applier. Apply writes an agency primitive successor and `EvolutionApplyReceipt`; it never changes source task lifecycle or graph topology. The successor's quality is measured only on later, disjoint task episodes. The evolver, its proposal task, and review of the proposal cannot score the successor.

`auto_evolve` runs this bounded observer/proposal lane directly. It creates no `.evolve-*` graph task. Failure creates backlog/reliability evidence and does not affect source readiness.

## 10. End-to-end data flow

```text
ready source generation
  |
  +-- dispatcher snapshots next attempt
  |     +-- explicit/automatic selector decision
  |     `-- AssignmentReceipt(composition | uncomposed)
  |
  +-- AttemptReserved references assignment receipt
  +-- source executes and submits immutable candidate A
  |
  +-- candidate ledger: Selected(A)
  |     +-- FLIP attempts (semantic or infrastructure)
  |     +-- eval attempts only per policy/order
  |     `-- completion controller alone consumes exact current receipts
  |
  +-- rejected A -> same source repairs -> Superseded(A,B) -> Selected(B)
  |     (no task and no source task_count increment)
  |
  +-- exact accepted candidate + publication -> lifecycle terminal event
  |
  `-- terminal projector, observation-only
        `-- exactly one LearningEpisode(generation, trajectory A..B)
              |
              +-- optional independent OutcomeAssessment
              +-- optional External Outcome Adjudication
              +-- effective-outcome resolver
              |     +-- source performance fold (one episode)
              |     +-- delayed AssignmentReward
              |     `-- ReviewerCalibration
              `-- Evolver input manifest -> proposal -> explicit agency apply
```

Every arrow after the lifecycle terminal event points away from source authority.

## 11. Crash/replay protocol

| Crash boundary | Required replay result |
|---|---|
| assignment selection-start written, no receipt | reuse the persisted absolute deadline and one live lease; after deadline create one `uncomposed` receipt without another call |
| assignment receipt written, no reservation | reuse if next-attempt and admission snapshot are exact; otherwise mark unused/superseded |
| reservation committed, spawn not started | reservation references one receipt; ordinary spawn recovery continues |
| candidate selected, review start absent | reserve one attempt ordinal and run |
| review start written, provider not called | expired lease becomes `interrupted_unknown`; retry is next ordinal |
| provider settled, terminal receipt written, index/link absent | verify and link receipt; do not call model again |
| provider may have settled, no durable receipt | mark unknown; a retry may duplicate external spend and must use a new ordinal |
| receipt linked, not consumed | completion controller rechecks current binding/fence and consumes once |
| candidate A result arrives after B selected | retain result as superseded/late; never consume for B |
| trajectory seal written, terminal event absent | controller reuses the exact seal/idempotency key; later candidate events cannot enter that terminal episode |
| publication committed, terminal event absent | existing completion controller derives and commits terminal state once using the sealed trajectory |
| terminal event committed, episode absent | projector creates deterministic episode only from the referenced seal |
| episode written, performance cache absent/partial | rebuild projection from immutable episode set; task count remains one |
| assessment written, reward absent | create deterministic delayed reward and rebuild projections |
| reward written, evolver checkpoint absent | next fold sees existing reward once; trigger counts distinct new episodes |
| projection written, checkpoint absent | recompute identical bytes or replace cache atomically; canonical records are unchanged |

No recovery path creates an ordinary review/assignment/evolution task. No replay infers a semantic result from process exit.

## 12. CLI, terminal, and TUI flows

### 12.1 Canonical commands

```text
wg reviews list [TASK] [--candidate current|all] [--kind flip|eval] [--json]
wg reviews show <REVIEW-ATTEMPT|VIRTUAL-ID> [--json]
wg reviews retry <REVIEW-RUN>                 # exact route; review lane only
wg reviews reroute <REVIEW-RUN> --route ...  # new audited route generation
wg assignment show <TASK> [--generation N] [--json]
wg learning show <TASK|EPISODE> [--json]
wg learning backlog [--json]
wg evaluate run <TASK|EPISODE> [--dry-run]
wg evaluate record --episode <ID> --score ... --source ... --reason ...
wg evaluate show [TASK]
wg evolve run|apply|review ...
```

`wg reviews retry` is an operator/review-lane request; it cannot retry the source. `wg retry T` remains the only source-generation retry surface.

### 12.2 Virtual aliases

The ledger projects stable read-only aliases such as:

```text
.flip-build-index@g2/aattempt-2-1/c3/r1
.evaluate-build-index@g2/aattempt-2-1/c3/r1
```

Aliases resolve only in `wg reviews show`, `wg show`, list/viz/TUI navigation, and URL/deep-link routing. Output begins:

```text
VIRTUAL REVIEW — not a graph task; no status, edge, worker slot, or lifecycle authority
```

Any mutating task command given a virtual alias exits nonzero with `WG-VIRTUAL-REVIEW-NON-AUTHORITATIVE` and suggests the typed review or source command. Projectors cannot make aliases addressable as tasks.

### 12.3 Terminal flow

```text
$ wg list --all
[R] .flip-build-index@g2/...  reject  superseded  $0.0041
[R] .flip-build-index@g2/...  pass    current     $0.0038
[R] .evaluate-build-index@g2/... pass current    $0.0022
    VIRTUAL REVIEW — not graph work

$ wg show build-index
Assignment: automatic receipt=asg_... composition=... attempt=attempt-2-1
Completion review: candidate #3 current; FLIP pass; eval pass
Candidate history: 2 candidates, 1 semantic reject, 0 infrastructure failures
Learning: episode=lep_... score=pending reward=pending (never blocks Done)

$ wg learning show build-index
Episode: one terminal generation observation
Trajectory: candidate #2 rejected; candidate #3 accepted
Source quality: eligible; outcome score pending
Reviewer reliability/calibration: shown separately
```

`wg spend` prints separate source worker, assignment selector, completion FLIP, completion eval, outcome scorer, and evolver totals plus an all-agency total. A row with missing provider usage is counted as an attempt with `cost=unknown`, never `$0`.

### 12.4 TUI

The task inspector gains three read-only sections:

1. **Assignment** — receipt/uncomposed state, composition snapshot, selector route/cost.
2. **Candidate review history** — current/superseded badges, FLIP/eval attempts, semantic vs infrastructure outcome, route, duration, usage/cost, bounded findings, and “virtual/non-schedulable.”
3. **Learning** — terminal episode, effective outcome, reward state, calibration/reliability links, and projector backlog.

The graph canvas may render virtual review chips attached visually to the source, but never edges and never ready/running task colors. Keyboard actions route to typed read-only inspection; source retry and review retry are different labeled commands. A PTY-driven smoke test must prove the rendered distinction.

## 13. Current command and configuration migration map

### 13.1 Commands and surfaces

| Current surface | Target behavior |
|---|---|
| `wg submit`, `land`, `report`, `explore`, ordinary `done` | **Retain.** Completion controller remains the sole ordinary consumer of exact review/publication evidence. It additionally links candidate-ledger events. |
| `wg done --operator-accept --reason` | **Retain as acceptance adjudication.** Operator-only, reasoned, immutable, generation/attempt/fence-bound and candidate-bound when verifiable; an absent candidate is an explicit evidence gap, never an invented binding. Never confused with outcome scoring. |
| `wg assign T A` / `--clear` | **Retain.** Set/clear next-attempt assignment intent; dispatcher emits the attempt-bound receipt. |
| `wg assign T --auto` | **Retain as automatic next-attempt intent.** Help/output names the actual selector (deterministic or model-backed) and stops claiming an LLM when historical ranking ran. No receipt until real reservation; no graph task/edge. |
| `wg match T` | **Retain.** Read-only candidate preview using the same eligibility/partition policy; no intent or receipt. |
| `wg evaluate run T` | **Retain and narrow.** Post-terminal independent scored outcome evaluation of an exact episode; no lifecycle effect. |
| `wg evaluate record` | **Retain as external outcome adjudication.** Require exact episode resolution and reason; no lifecycle effect. |
| `wg evaluate show` | **Retain.** Show assessments/adjudications/effective score and source binding; label legacy/unbound records. |
| `wg evaluate rollout start|advance|record-observation|rollback` | **Deprecate mutations.** Preserve `status` as historical audit until old rollout records age out. New adaptive rollout uses §15 and never restores global evaluator lifecycle authority. |
| future/current lazy bounded/deep evaluation runners | **Fold into `wg reviews` candidate products.** Import `EvaluationRecord` attempts; keep candidate binding and separate agency lane. They remain observation-only unless the completion controller's snapshotted policy explicitly consumes them. |
| `wg evolve run` | **Retain.** Reads episode manifests and writes proposals; `--autopoietic` iterates agency proposals/results, never graph source cycles. |
| `wg evolve apply` | **Retain.** Explicit agency-store apply with receipt; no source/task mutation. |
| `wg evolve review list|approve|reject`; `wg agency deferred|approve|reject` | **Retain aliases for one release, then canonicalize under `wg evolve review`.** Human review concerns agency proposals only. |
| current coordinator-created `.evolve-*` | **Retire.** `auto_evolve` invokes the non-graph observer/proposal lane. |
| `.assign-*`, `.flip-*`, `.evaluate-*` rows | **Retired historical evidence.** Never schedulable. Virtual aliases are computed from ledgers and cannot satisfy/block dependencies. |
| `wg migrate evaluation-cutover` | **Retain permanently for v1 graphs.** Its operator `--accept` stays a narrowly scoped legacy acceptance adjudication. |
| `wg migrate review-identity` | **Retain through dual-write.** Then make it a read-only verification/import command; never infer missing superseded history. |
| `wg agency migrate` | **Extend.** Idempotently import terminal observations, scored envelopes, assignments, and legacy performance under §14 confidence classes. |
| `wg agency stats` | **Retain.** Default to distinct episodes/effective scores; add reviewer calibration and route reliability sections. Legacy file counts are labeled legacy. |
| `wg list --all` | **Retain current virtual visibility.** Add stable `.flip/.evaluate` aliases and explicit non-task banner. Add `--reviews` as discoverable spelling. |
| `wg show T`, `wg trace T`, `wg spend`, `wg status` | **Retain/extend.** Read verified ledgers; separate source, review, scorer, and evolution accounting/backlogs. |
| `wg config --set-model ...`, `wg profile set-model ...`, `wg profile pi --weak ...` | **Retain.** Resolve agency roles to an exact route/reasoning; every receipt snapshots the result. Profile changes affect only later runs. |
| `wg config --auto-evaluate/--auto-assign/--flip-*` | **Deprecate writes.** Compatibility reads remain; a write refuses with the canonical replacement. Ambiguous historical booleans never enable new paid calls silently. |
| `wg show .flip-*` / `.evaluate-*` | **Read-only compatibility routing** to `wg reviews show`; task mutations reject the alias. |
| `wg reset --also-strip-meta` | **Deprecate agency meaning.** It may clean truly retired rows only; it never deletes canonical adaptive evidence or virtual aliases. |
| `wg rescue --from-eval` and evaluator-authored rescue | **Deprecate/remove.** A reviewer finding may suggest a command, but cannot create/rewire/retry source work. `wg rescue` remains an explicit operator graph-surgery command without evaluator privilege. |
| old `PendingEval` / `FailedPendingEval` status behavior | **Migration-only.** New adaptive events never produce either status. |

Option-level behavior is also explicit: `wg evaluate run --dry-run` remains read-only and shows episode/evidence/independence/route without a call; `evaluate record --source/--dim/--notes` becomes authenticated source/dimension/reason material and task shorthand must add `--generation` when ambiguous; `evaluate show --task/--agent/--source/--limit` continues as filters over assessments plus legacy partitions. `wg evolve run --strategy/--budget/--model/--force-fanout/--single-shot` controls proposal computation only. `--autopoietic`, `--max-iterations`, and `--cycle-delay` bound an agency proposal/assessment loop, never task-graph cycle edges. `wg evolve apply --output` and review notes remain agency-store receipts. No retained option gains lifecycle authority.

### 13.2 Canonical new configuration

```toml
[agency.adaptive]
enabled = true
projection_policy = "adaptive-v1"

[agency.assignment]
mode = "off"                 # off | prefer; no required mode in v1
selector = "native"          # native | agency
deadline_secs = 30

[agency.candidate_review]
max_infrastructure_attempts = 2

[agency.outcome]
auto_score = false
independence = "require"

[agency.learning]
performance_policy = "terminal-episode-v1"

[agency.evolution]
auto = false
interval_secs = 7200
min_new_episodes = 10
budget = 5
reactive_score = 0.4

[models.completion_flip]
model = "pi:<provider>:<model>"
reasoning = "high"

[models.completion_eval]
model = "pi:<provider>:<model>"
reasoning = "high"

[models.outcome_scorer]
model = "pi:<provider>:<different-model-or-cohort>"
reasoning = "high"
```

Exact routes/reasoning remain under `[models.<role>]`. Per-attempt receipts snapshot resolved values; later config changes never relabel history.

### 13.3 Every current `AgencyConfig` / evaluation key

| Current key | Target / deprecation |
|---|---|
| `agency.auto_evaluate` | Deprecate as ambiguous. Migration records the old value but writes `agency.outcome.auto_score=false`; an operator explicitly opts in after canary. It never means completion review and never creates graph tasks. |
| `agency.auto_assign` | Deprecate as a historical no-authority flag. Migration defaults `assignment.mode=off` and prints the explicit `prefer` command when the old value was true; it must not activate paid selection silently. |
| `agency.assigner_agent` | Migrate to selector policy/principal metadata; not an implementation worker and not a score target until delayed reward. |
| `agency.evaluator_agent` | Deprecate. Scorer/reviewer identity is stable policy + principal + route cohort, not a schedulable agent composition. |
| `agency.evolver_agent` | Migrate to evolver policy/principal metadata; proposal-only capability. |
| `agency.creator_agent`, `agency.auto_create`, `agency.auto_create_threshold` | Unchanged creator subsystem; creator output cannot write adaptive truth or source lifecycle. |
| `agency.placer_agent`, `agency.auto_place` | `placer_agent` remains placement metadata. Deprecate assignment-coupled `auto_place`; dependency edges require the existing explicit placement/graph authority, never an assignment selector. |
| `agency.retention_heuristics` | Retain under evolution policy; snapshot digest in every evolution run. |
| `agency.auto_triage`, `agency.triage_timeout`, `agency.triage_max_log_bytes` | Unchanged triage subsystem; no adaptive or completion authority. |
| `agency.inference_timeout` | Retain as compatibility default only. Canonical lane deadlines are snapshotted per assignment/review/outcome/evolution policy. |
| `agency.exploration_interval` | Retain assignment policy input; count prior eligible assignment rewards/episodes, not task or receipt files. |
| `agency.cache_population_threshold` | Retain; apply to effective independent episode outcomes only. |
| `agency.ucb_exploration_constant` | Retain and snapshot in assignment receipt. |
| `agency.novelty_bonus_multiplier` | Retain and snapshot in assignment receipt. |
| `agency.bizarre_ideation_interval` | Retain as explicit exploration policy; receipt labels forced exploration. |
| `agency.eval_gate_threshold`, `agency.eval_gate_all` | Deprecate as global lifecycle gates. Scores are post-terminal; only completion policy consumes candidate receipts. |
| `agency.auto_rescue_on_eval_fail` | Remove. Reviewer/scorer/evolver cannot rescue, retry, or rewire a source. |
| `agency.flip_enabled` | Deprecate ambiguous switch. Completion FLIP follows completion policy; optional deep candidate FLIP uses an explicit candidate-review product policy. |
| `agency.flip_inference_model`, `agency.flip_comparison_model` | Deprecated aliases to `models.flip_inference` / `models.flip_comparison`; migrate exact routes/reasoning. |
| `agency.flip_verification_threshold` | Remove. No `.verify-*` autospawn or threshold-driven source mutation. |
| `agency.auto_evolve` | Deprecated alias to `agency.evolution.auto`; target is non-graph proposal lane. |
| `agency.evolution_interval` | Migrate to `agency.evolution.interval_secs`. |
| `agency.evolution_threshold` | Migrate to `min_new_episodes`; counts distinct eligible episodes. |
| `agency.evolution_budget` | Migrate to proposal operation budget. |
| `agency.evolution_reactive_threshold` | Migrate to `reactive_score` over effective eligible episode scores. |
| `agency.agency_server_url`, `agency.agency_token_path` | Retain transport/auth. Requests/responses are digest-bound; outage yields uncomposed assignment or outbox backlog, never source hold. Secrets never enter receipts. |
| `agency.assignment_source` | Migrate to `agency.assignment.selector`; reject unknown values. |
| `agency.agency_project_id` | Retain external selector namespace, recorded by digest in assignment receipt. |
| `agency.upstream_url` | Unchanged Agency import setting; outside the loop. |
| `agency.evaluator_model` | Deprecate as ambiguous gate/post-hoc routing. Do not copy it automatically; operators choose exact `models.completion_eval` and a disjoint `models.outcome_scorer`. |
| `agency.default_validation_mode` | Retain deterministic/completion validation meaning. It is not scored outcome evaluation. |
| `agency.gate_uncertain_policy`, `agency.gate_max_attempts`, `agency.gate_confidence_threshold` | Deprecate legacy LLM-gate controls. Candidate infrastructure retry belongs to candidate-review policy; semantic uncertainty never becomes source retry. |
| `agency.completion_review_strict` | Retain. Snapshot into generation completion policy; only the completion controller applies it. |
| `evaluation.managed_rollout`, `evaluation.rollout_stage` | Historical/read-only after adaptive rollout. Migration records old stage, then removes active authority. |
| `models.assigner`, `models.flip_inference`, `models.flip_comparison`, `models.evolver`, associated `.reasoning` | Retain exact role routing for selector, optional candidate products, and evolution. Every call snapshots resolved route/reasoning/adapter. No fallback. |
| `models.reviewer` | Current completion-FLIP route. Migrate explicitly to `models.completion_flip`; after migration `models.reviewer` remains only WG-Review/content-safety. |
| `models.evaluator` | Current completion-Eval **and** scored-outcome route. Split it: migrate to `models.completion_eval`; require an explicitly configured, disjoint `models.outcome_scorer` before scores are automatic/learning-eligible. Never copy it silently into both. |
| new `models.completion_flip`, `models.completion_eval`, `models.outcome_scorer` | Distinct exact routes/policy cohorts. A shared explicit route is allowed only as visible non-independent evidence and is excluded from reward/calibration/evolution by default. |
| `models.verification` | Retain for explicit verification work only; a FLIP threshold cannot autospawn it or grant it source authority. |
| `models.creator`, `models.placer`, `models.triage` | Unchanged non-loop roles. Their routes never substitute for an adaptive role silently. |
| `tiers.fast`, `tiers.fast_reasoning` | Retain only as explicit role-resolution fallback for selector/candidate one-shots; completion and outcome-scoring roles require an exact resolved receipt route. |

`wg config lint` warns on every deprecated key. `wg migrate config --dry-run` prints its exact mapping/removal. No retired key silently retains source-lifecycle authority.

## 14. Historical migration and confidence

Migration is additive and replayable. It stores `raw_digest`, `schema_origin`, original locator, import version, and one binding class:

| Source | Import target | Default eligibility |
|---|---|---|
| current exact `ReviewReceipt` / `completion_review_activity` | candidate selected/start-finished projection using exact binding; missing start time is declared unknown | candidate trajectory/reliability where fields exist |
| rich `Task.evaluation_records` | candidate review runs/attempts, preserving route, failure, usage, verdict, and consumed ID | exact when full source/candidate binding verifies |
| inert `.flip-*` / `.evaluate-*` task row | legacy candidate observation with raw row/log/usage digest | historical only unless exact manifest + attempt binding exists |
| inert `.assign-*` row | legacy selector observation | no delayed reward without exact attempt-bound composition |
| legacy `TaskAssignmentRecord` | assignment import | `task-bound` only; reward excluded unless generation/attempt can be proven |
| `TerminalOutcomeObservation` | learning episode seed | exact for verified receipt-bound terminal generation |
| `ScoredEvaluationEnvelope` | outcome assessment | exact when terminal observation binding verifies; otherwise legacy/unbound |
| legacy `.wg/agency/evaluations/*.json` | external legacy assessment/adjudication | visible, excluded from modern automatic ranking by default |
| evaluation-cutover/operator acceptance receipt | acceptance adjudication provenance in terminal episode | episode visible; ordinary-publication quality remains distinct |
| evolver state/history | legacy evolution run manifests | historical; old file counts do not trigger new evolution |

Binding classes are `exact-candidate`, `attempt-bound-no-manifest`, `task-bound`, `unbound`, and `invalid/quarantined`. Migration never upgrades confidence by inference, never rewrites original files, and never converts historical rows back into tasks. Deterministic import IDs make reruns no-ops.

During dual-read, stable receipt/attempt IDs deduplicate current projections against imports. When the adaptive ledger becomes canonical, old mutable arrays/files remain readable compatibility caches for one release and are then read-only.

## 15. Rollout phases

1. **Phase 0 — schemas and static boundary.** Add serde schemas, deterministic IDs, create-new stores, capability traits, and compile-fail authority tests. No behavior change.
2. **Phase 1 — candidate dual-write.** Current completion review writes/verifies candidate ledger events and current task activity. Compare exact counts/digests; source controller still reads current receipts.
3. **Phase 2 — visibility/accounting.** Ship `wg reviews`, virtual aliases, list/show/trace/spend/status/TUI views. Invalid ledger data fails projection closed. No automatic selector/scorer/evolver.
4. **Phase 3 — attempt-bound assignment.** Emit explicit/uncomposed receipts in shadow, then enable `prefer` canaries. Timeout/unavailable canaries must dispatch uncomposed and create no edge.
5. **Phase 4 — terminal episodes.** Dual-project `TerminalOutcomeObservation` and `LearningEpisode`; fault-inject every terminal boundary. Compare distinct episode count to terminal generation count. Performance remains shadow.
6. **Phase 5 — scored outcome and delayed reward.** Route `wg evaluate run/record/show` through episode-bound assessments/adjudications. Shadow-fold performance, rewards, and calibration; compare legacy views without writing placeholder scores.
7. **Phase 6 — adaptive consumers.** Switch assignment ranking and evolver input to canonical projections. Replace coordinator `.evolve-*` creation with non-graph proposals. Keep explicit apply.
8. **Phase 7 — migration and retirement.** Import all legacy planes, run confidence reports, make old writers read-only, migrate/deprecate config, and remove active rollout/gate/rescue paths.

Rollback at every phase disables new calls/consumers but preserves evidence. It cannot change already-terminal source state. No phase re-enables synthetic row scheduling.

## 16. Executable acceptance matrix

The downstream implementation is complete only when these commands exist and pass. Scenario scripts must be registered in the grow-only smoke manifest with `owners = ["implement-adaptive-agency-ledger"]` (and any more specific implementation task owners).

| ID / property | Executable command | Required assertion |
|---|---|---|
| A1 capability non-authority | `cargo test --test adaptive_agency_capabilities -- --nocapture` | compile-fail fixtures cannot construct lifecycle/dispatch/publication/graph-edge calls from reviewer, projector, scorer, or evolver capabilities; runtime fixtures leave graph bytes unchanged |
| A2 no synthetic authority | `bash tests/smoke/scenarios/adaptive_agency_no_synthetic_tasks.sh` | publish/submit/reject/retry/score/evolve creates zero schedulable `.assign/.flip/.evaluate/.evolve` rows and zero agency edges |
| A3 assignment receipt/uncomposed | `bash tests/smoke/scenarios/adaptive_assignment_receipt_flow.sh` | explicit, automatic, timeout, and unavailable paths each reserve an attempt referencing exactly one receipt; failure dispatches uncomposed within deadline |
| A4 exact binding/supersession | `cargo test --test adaptive_candidate_ledger exact_binding_supersession -- --exact` | A reject then B pass retains both; only B consumes; changed manifest/requirements/output/fence refuses stale receipt |
| A5 semantic vs infrastructure | `cargo test --test adaptive_candidate_ledger semantic_and_infrastructure_partitions -- --exact` | reject enters semantic trajectory; timeout/malformed/route drift enter reliability only; neither directly retries/reopens source |
| A6 retry/reroute provenance | `cargo test --test adaptive_candidate_ledger retry_and_reroute_are_distinct -- --exact` | retry repeats exact route with next ordinal; reroute creates route generation; old bytes unchanged |
| A7 crash replay matrix | `cargo test --test adaptive_agency_replay crash_matrix -- --exact` | faults at every §11 boundary converge; assignment deadline never resets; trajectory seal is stable; valid receipt avoids model reinvocation; unknown call is labeled and new ordinal used |
| A8 exactly one episode | `cargo test --test adaptive_learning_episode repeated_candidates_count_once -- --exact` | N rejected candidates + one pass yields one episode and `task_count=1`; projector replay/crash remains one |
| A9 terminal classes | `cargo test --test adaptive_learning_episode terminal_eligibility_matrix -- --exact` | Done/source-caused failure/infra failure/cancel/operator acceptance have the specified distinct eligibility |
| A10 anti-self-scoring | `cargo test --test adaptive_outcome anti_self_scoring -- --exact` | distinct scorer role is required; source/assigner/evolver/same calibrated reviewer cannot author an independent score; non-independent evidence stays visible and excluded |
| A11 delayed reward/evolver | `cargo test --test adaptive_reward delayed_reward_and_evolver_input -- --exact` | no placeholder reward; one effective outcome yields one active reward; superseding adjudication replaces score without task-count growth; evolver counts episode IDs |
| A12 external adjudication separation | `bash tests/smoke/scenarios/adaptive_external_adjudication.sh` | forged actor/authority/signature is rejected; exact replay dedupes; candidate claim binds exactly; outcome record cannot alter task state/publication; operator acceptance requires operator capability/reason and is labeled acceptance, not score |
| A13 accounting | `cargo test --test adaptive_accounting deduplicated_lane_totals -- --exact` | source/assign/FLIP/eval/scorer/evolver totals are separate; duplicate links do not double charge; missing usage is unknown, not zero |
| A14 legacy migration | `bash tests/smoke/scenarios/adaptive_agency_migration.sh` | synthetic rows, lazy records, current receipts, scored envelopes, and unbound legacy files import idempotently with correct confidence; originals unchanged |
| A15 terminal human flow | `bash tests/smoke/scenarios/adaptive_agency_terminal_flow.sh` | installed CLI shows live start, infra failure, exact retry, semantic reject, supersession, pass/eval, cost, one episode, delayed reward, and non-task banner |
| A16 TUI human flow | `bash tests/smoke/scenarios/adaptive_agency_tui_flow.sh` | tmux/PTY keystrokes open task/review/learning panes; rendered text distinguishes source retry vs review retry and says virtual/non-schedulable |
| A17 mutation rejection | `bash tests/smoke/scenarios/adaptive_virtual_alias_refuses_mutation.sh` | `wg retry/fail/done/publish/add-dep/msg` on every virtual alias exits nonzero; source graph hash unchanged |
| A18 performance rebuild | `cargo test --test adaptive_performance_projection rebuild_after_partial_write -- --exact` | delete/corrupt mutable cache after canonical commit; rebuild matches expected bytes and distinct episode counts |
| A19 route and reviewer calibration | `cargo test --test adaptive_calibration independent_truth_only -- --exact` | completion/scorer roles are distinct; semantic outcomes never affect route reliability; infra never affects source score; calibration requires disjoint truth |
| A20 repository policy | `cargo fmt --check && cargo clippy && cargo test --locked` | checked-in Rust policy passes; failures are reported against base/environment as required |

The terminal/TUI scenarios are mandatory human-flow tests, not render-function substitutes. Provider behavior may use a credential-free deterministic adapter in Rust tests; a real installed-binary smoke scenario must still exercise the command/service/PTY surfaces. Live provider canaries are additional and may skip only for missing credentials.

## 17. Ratification criteria

The adaptive loop may be enabled by default only when:

- every canonical source attempt has one assignment or `uncomposed` receipt;
- every current completion receipt is represented in the append-only candidate ledger with exact binding and accounting;
- every terminal generation eligible for projection has exactly one episode;
- no source performance cache can count more distinct tasks than eligible episode IDs;
- assignment reward and evolver triggers use effective episode outcomes, never reviewer/file counts;
- reviewer calibration and route reliability are independently queryable and cannot affect source quality accidentally;
- all mutating commands reject virtual aliases;
- all capability and crash-replay tests pass; and
- config/help/manual/TUI use the four terms in §2 consistently.

The central rule is:

> Internal agency actors append attributable evidence and derived proposals. Only the dispatcher reserves an attempt, only the completion controller consumes exact current review evidence, and only the lifecycle kernel changes source state. Learning observes terminal truth; it never becomes a hidden prerequisite for it.
