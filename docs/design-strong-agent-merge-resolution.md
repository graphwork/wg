# Strong-agent merge resolution

**Status:** implementation-ready design; no production code in this change

**Date:** 2026-07-26

**Owner:** `design-strong-agent`

**Default rollout state:** disabled

**Normative dependencies:**

- [Candidate finalization transaction](design-candidate-finalization-transaction.md)
- [Pi-first evaluation and deep-readonly FLIP plane](design-pi-evaluation-plane.md)
- [Simplified authoritative task lifecycle](design-simplified-task-lifecycle.md)
- [WG inbound-content review gate](ADR-content-safety-001-review-gate.md)

## 1. Decision and scope

WG has two integration lanes and no third lane:

1. The central merge authority may perform a **provably mechanical** integration
   of one immutable `CandidateDescriptor` at one immutable target snapshot. This
   lane makes zero model calls.
2. Every real textual or semantic integration conflict is either handled by
   **one explicitly selected full strong coding-agent route** in a disposable,
   isolated integration repository, or stopped for a content-bound human
   decision. It is never edited by the coordinator, evaluator, reviewer, weak
   tier, or merge authority.

The resolver's successful output is not an edit to the source candidate. The
candidate finalizer seals it as a new immutable `ResolutionCandidateDescriptor`,
then the canonical review, deterministic validation, and policy-selected
candidate evaluation/FLIP gates run again. Only the central merge authority may
accept the exact sealed tree and CAS the canonical target ref.

This design consumes, rather than replaces, the upstream contracts:

- `CandidateDescriptor`, rescue/candidate version ownership, target snapshot,
  merge-attempt receipt, validation bindings, and merge CAS remain owned by the
  candidate finalization transaction.
- The lifecycle kernel remains the only task/attempt status writer. Every phase
  in this document is an append-only resolution-domain projection while the
  canonical task normally remains `AwaitingAcceptance`.
- Evaluators and deep-readonly FLIP remain candidate-bound, read-only evidence
  producers. They cannot resolve, merge, reopen, retry, or downgrade review.
- The source candidate/worktree, its immutable candidate ref, and canonical
  main ref remain immutable throughout resolution.

The input boundary is exactly:

```text
immutable CandidateDescriptor
+ accepted candidate validation/evaluation evidence required by its policy
+ immutable merge base and target-head snapshot (commit + tree)
+ candidate finalizer's deterministic merge-attempt receipt/conflict evidence
+ snapshotted repository integration/review/evaluation policy
```

A path, branch, current profile, mutable worktree, current main checkout, or
coordinator memory is not an input identity.

## 2. Authority boundaries and why each exists

| Actor | Authority | Forbidden authority | Why |
|---|---|---|---|
| Lifecycle kernel | Canonical generation/attempt/task transitions and repair authorization | Inspect conflicts, choose content, run Git/model commands | A single status writer prevents a resolution sidecar from reopening or completing a task independently. |
| Candidate finalizer | Candidate and resolution-candidate version slots; descriptors, manifests, immutable refs | Choose product behavior, update main, evaluate | The same content owner must bind every proposed byte before any gate consumes it. |
| Deterministic classifier | Verify pinned objects and evidence; emit one typed classification | Edit files, infer intent, call a model, mutate lifecycle/main | Classification must be replayable and credential-free; giving it edit authority would make “mechanical” unverifiable. |
| Canonical review gate | Deterministic lint and policy-selected inbound-content verdicts | Resolve code, select a route, lower a prior verdict | Untrusted conflict/diff/model output is data. Safety precedes exposure and cannot be negotiated by the actor being reviewed. |
| Route resolver | Resolve `models.merger` or an allowed strong alias once; attest exact adapter/route availability | Substitute a model/provider/executor, weaken reasoning, interpret conflict content | Route choice is execution policy, not a recovery heuristic. Persisting it prevents ambient config drift. |
| Strong merger | Edit/build/test only in its isolated integration repository; return `resolved`, `reject`, or `needs_human` | Access canonical `.git`/main, source worktree, graph, lifecycle tools, evaluator tools, credentials for push/ref update | It needs coding capability to solve real conflicts but no authority to make its proposal canonical. |
| Validator | Deterministically inspect the sealed resolution descriptor | Edit source/resolution/main, call acceptance | A fresh deterministic check distinguishes valid integrated bytes from persuasive model prose. |
| Evaluator/FLIP | Append policy-selected evidence bound to the resolution descriptor | Resolve, mutate, merge, reopen, or downgrade safety | Semantic judgment is evidence only and must not become integration authority. |
| Human decision author | Supply otherwise missing product/policy intent with bounded rationale and constraints | Directly push/merge or waive content binding and gates | Humans own ambiguous intent and policy choices, but their decision is an input to a new checked proposal, not a bypass. |
| Central merge authority | Verify every binding, import one accepted tree, CAS canonical target, issue one receipt | Invent conflict resolutions, reroute a model, edit candidate bytes | A small mechanical authority makes the only canonical write auditable and exactly-once. |
| Coordinator/daemon | Drive outbox actions and render state | Edit conflict markers, choose “ours/theirs,” write status, silently retry on another route | Scheduling availability must not become content authority. |

The classifier may say that evidence is ambiguous; it may not invent the
missing intent. The strong merger may discover an ambiguity and return
`needs_human`; it may not choose a plausible behavior merely because one lets
tests pass. The evaluator may reject or be inconclusive; it cannot “fix” the
bytes. These separations make every content-changing decision attributable to
one strong route or one named human.

## 3. Immutable identities and evidence snapshots

The finalizer verifies the incoming `CandidateDescriptorV1` as specified by the
candidate-finalization design: descriptor CID, commit/tree OIDs, full and delta
manifest CIDs, base commit/tree, source tuple, completion contract, dependency
revision, and policy snapshots. It also verifies the accepted validation and,
where required, evaluation evidence bind that same tuple. A mismatch is
`MR_BINDING_MISMATCH`, not a conflict.

The target snapshot is:

```rust
struct TargetHeadSnapshotV1 {
    target_ref_id: String,       // canonical identity, not a free-form ref
    commit_oid: GitOid,
    tree_oid: GitOid,
    content_manifest_cid: Cid,
    dependency_revision_cid: Cid,
    captured_by_event: EventId,
    captured_at: Timestamp,      // provenance only
}
```

The merge-attempt receipt records the exact candidate/base/target tuple, merge
tool and version, conflict-index digest, candidate/target diffs, generated and
dependency metadata, commands run, exit/output digests, and prepared clean tree
when one exists. All arrays and maps are canonicalized; arbitrary command,
diff, and model text is stored by CID rather than interpolated into reason
fields.

There are two IDs:

```text
classification_id = BLAKE3(
  "wg-merge-classification-v1\0" || candidate commit/tree/manifest CID ||
  base commit/tree || target commit/tree/manifest CID ||
  merge-attempt/evidence-bundle digest || policy snapshot CID)

resolution_request_id = BLAKE3(
  "wg-merge-resolution-v1\0" || all classification identity fields ||
  classifier outcome/reason digest || ResolutionRouteSnapshot CID)
```

The route-independent classification ID permits clean classification without
resolving credentials or creating model work. A `resolution_request_id` exists
only after a real, model-resolvable conflict and one exact strong route have
both been established. One run-generation slot is created with create-if-absent
CAS for that ID. Clean merges, candidate repair, security blocks, and known
human-only decisions create **no merger run, no model call, and no graph task**.

## 4. Deterministic classifier and normative decision table

### 4.1 Ordered evidence pipeline

The classifier is a total function over pinned evidence. It performs these
steps in order and records the result of every earlier step even when a later
step is not reached:

0. **Binding/integrity:** verify object availability, schema, CIDs/OIDs,
   target snapshot, accepted evidence, toolchain and policy snapshot. Unknown
   or unlabeled input fails closed as `MR_CLASSIFIER_INCONCLUSIVE`.
1. **Pre-exposure safety:** run canonical deterministic lint and the
   credential-free portion of the inbound-content review contract over task
   intent, candidate/target diffs, conflict map, generated metadata, policy
   text, and bounded logs before any merger sees them. Reuse an already accepted
   exact content-bound receipt where policy requires it. Classification and the
   mechanical lane never launch a reviewer model; if policy requires a new live
   reviewer for these unchanged inputs and no accepted receipt exists, the case
   holds rather than being called mechanical. Reject, quarantine, or hard
   findings produce `SecurityReviewBlocked`. Resolution output later traverses
   the policy-selected full canonical review gate as §9 specifies.
2. **Candidate baseline:** verify or rerun the candidate's own deterministic
   pre-integration policy against its detached immutable tree. A source failure
   is `CandidateRepairRequired`, even if Git would also conflict.
3. **Independent target baseline:** run the target-side commands selected by
   the integration policy against the pinned target snapshot. A broken target
   is an operator hold (`MR_TARGET_BASELINE_INVALID`), not a candidate semantic
   conflict and not merger work.
4. **Intent/policy decidability:** deterministic metadata may prove that a
   policy-sensitive choice, contradictory requirement, or unresolved ownership
   decision needs a human. Human-required intent/policy wins before model
   resolution. No natural-language model is called merely to classify it.
5. **Credential-free dry merge:** use the pinned merge engine and exact
   candidate/base/target objects in a private index. Never use a live checkout.
   A formally deterministic engine is pinned by binary/config digest; otherwise
   two fresh private-index runs must produce the same conflict map or complete
   tree. Any divergence is `MR_CLASSIFIER_INCONCLUSIVE`.
6. **Generated ownership analysis:** consult snapshotted source-of-truth rules,
   generator command/toolchain, determinism receipt, and output ownership.
7. **Combined-tree checks:** only for a clean textual tree, run the complete
   required integration command set. Preserve the candidate-alone, target-alone,
   and combined receipts.
8. **Target CAS preflight:** immediately before a mechanical acceptance request,
   require the canonical target commit and tree still equal the snapshot.

Safety and candidate-invalid findings both win before conflict resolution;
where both are present, the safety block is rendered primary because no actor
may be exposed to blocked bytes, with candidate invalid retained as secondary
evidence. Known human-only product/policy ambiguity wins over a model run.
Everything else must prove the fully clean case; absence of evidence never means
clean.

### 4.2 Mutually exclusive outcomes

| Outcome and stable reason | Required positive evidence | Required negative evidence / consequence |
|---|---|---|
| `MechanicalMerge` / `MR_MECHANICAL_CLEAN` | Exact bindings; accepted pre-exposure review; candidate and target baselines pass; pinned credential-free dry run produces exactly one deterministic conflict-free complete tree; candidate projection is exact; generated outputs and policy ownership are unambiguous; every required integration check passes; target ref still equals snapshot | No conflict entry, unresolved marker, generated/policy ambiguity, unaccepted safety finding, source mutation, nondeterministic tool result, or unknown classifier field. **Zero model calls.** Proceed to central mechanical acceptance only. |
| `CandidateRepairRequired` / `MR_CANDIDATE_BASELINE_FAILED` | A named pre-integration candidate command/invariant fails on the immutable candidate under its pinned standalone policy; receipt binds command, toolchain, exit/output and candidate tree | Not semantic integration. Do not create a resolution request. Link a normal candidate repair/new version through lifecycle policy. |
| operator hold / `MR_TARGET_BASELINE_INVALID` | Candidate passes but target fails its own pinned independent baseline | Not candidate breakage and not model-resolvable by altering candidate. Hold target owners/operators; main remains unchanged. |
| `MergeResolutionRequired::TextualConflict` / `MR_TEXTUAL_OVERLAP`, `MR_ADD_ADD`, `MR_RENAME_DELETE`, `MR_MODIFY_DELETE`, `MR_SUBMODULE_INTERACTION`, `MR_DEPENDENCY_LOCK_INTERACTION`, or `MR_OTHER_NONCLEAN_MERGE` | Candidate and target baselines pass; merge index contains the named non-clean conflict class and canonical conflict-map digest | No coordinator conflict-marker edit, `ours`/`theirs` choice, evaluator resolution, or mechanical merge. Route exactly one strong merger unless a human/policy/safety outcome has precedence. |
| `MergeResolutionRequired::SemanticIntegrationConflict` / `MR_COMBINED_CHECK_FAILED` | Candidate-alone receipt passes; target-alone receipt passes; textual merge is clean and deterministic; the combined tree fails a named compile/test/schema/API/invariant command. Store all three command/toolchain receipts and failing output CID | If candidate-alone did not pass, use candidate repair. A policy-only check failure is still a combined integration failure or human policy decision; Git cleanliness cannot disguise it as mechanical. |
| `MergeResolutionRequired::GeneratedArtifactConflict` / `MR_GENERATED_REGEN_REQUIRED` | Source-of-truth ownership is explicit and unique; affected output-to-input mapping is known; pinned generator argv/toolchain exists; prior determinism receipt or two-run reproduction proves deterministic output; regeneration is available inside policy | Merger may edit only source inputs and invoke the pinned generator. Hand-editing generated output is rejected. Missing any positive evidence becomes human ambiguity below. |
| `NeedsHumanMergeDecision::GeneratedIntentAmbiguous` / `MR_GENERATED_INTENT_AMBIGUOUS` | Generated output is involved but ownership, authoritative form, deterministic command/toolchain, or reproducibility is missing/disputed | No model run. Human decision must identify source of truth and constraints; it cannot bless hand-edited generated bytes without a policy change. |
| `NeedsHumanMergeDecision` / `MR_PRODUCT_INTENT_AMBIGUOUS`, `MR_REQUIREMENTS_CONFLICT`, `MR_POLICY_DECISION_REQUIRED`, or `MR_INSUFFICIENT_EVIDENCE` | Evidence proves two or more plausible user/product behaviors, contradictory requirements, a human-owned policy choice, or insufficient authoritative evidence. The strong merger may also return this outcome later | A model may identify and explain ambiguity but cannot choose. Main remains unchanged pending a content-bound human record. |
| `SecurityReviewBlocked` / `MR_REVIEW_REJECTED`, `MR_REVIEW_QUARANTINED`, or `MR_HARD_SAFETY_FINDING` | Canonical pre-exposure lint/review emits reject, quarantine, or hard finding bound to input CIDs | No merger launch and no evaluator/model exposure. Neither route policy nor model output can downgrade it. |
| `ResolutionRejected` / `MR_OUTPUT_REVIEW_REJECTED`, `MR_OUTPUT_INVALID`, `MR_OUTPUT_EVALUATION_REJECTED`, `MR_OUTPUT_BINDING_MISMATCH`, or `MR_WORKSPACE_MUTATED_AFTER_SEAL` | A launched merger's sealed output later fails content review, deterministic validation, required evaluation, descriptor binding, or seal equality | Retain source and resolution descriptors/evidence. No target mutation. Create only an explicitly linked repair/new candidate version. |
| fail-closed hold / `MR_CLASSIFIER_INCONCLUSIVE`, `MR_BINDING_MISMATCH`, `MR_TOOLCHAIN_UNAVAILABLE`, or `MR_POLICY_UNLABELED` | A required field, object, toolchain, policy label, or deterministic fact is absent/unknown | No mutation and no model call unless an operator supplies a new authoritative snapshot. Unknown is never converted to textual conflict or mechanical merge. |

Conflict types are exclusive at the top level. A deterministic ordering chooses
`GeneratedArtifactConflict` when a generated output conflict satisfies all
ownership/regeneration evidence, otherwise generated ambiguity; then explicit
textual conflict; then semantic integration failure after a clean tree. The
conflict map may contain subordinate facts, but one primary outcome and reason
is persisted.

Unresolved conflict markers are detected from the merge index plus a
policy-defined marker scan of newly introduced bytes. Existing literal marker
fixtures must be explicitly owned by policy; an unknown marker blocks
mechanical classification. Submodule and lockfile interactions are textual
unless the complete merge is clean and only a combined command fails, in which
case they are semantic.

## 5. Exact strong route contract

### 5.1 Persisted snapshot

`models.merger` is a new explicit role. Its value normally names a fully
qualified handler-first route. If policy deliberately allows the literal alias
`strong` or `premium`, WG resolves that alias once through the snapshotted
profile/config/catalog and persists the resulting exact route before enqueue.
No alias is passed to a runner.

```rust
struct ResolutionRouteSnapshotV1 {
    schema: u16,
    route_snapshot_cid: Cid,
    exact_handler_first_spec: String, // e.g. pi:openai-codex:gpt-5.6-sol
    handler: ExecutorKind,
    adapter_id: String,
    provider: String,
    model: String,
    declared_class: Strong | Premium,
    reasoning: High | Xhigh,
    config_revision_cid: Cid,
    active_profile_name: Option<String>,
    profile_revision_cid: Option<Cid>,
    catalog_entry_cid: Cid,
    budget: ResolutionBudget,
    tool_policy_cid: Cid,
    sandbox_policy_cid: Cid,
    route_provenance: ExplicitModelsMerger | AllowedTierAlias,
    resolved_by_event: EventId,
}
```

Strength is attested by the snapshotted model catalog/policy entry, not guessed
from a model-name substring. `Fast`, weak, unclassified, deprecated bare
provider, or catalog-unknown routes are invalid. Reasoning must be supported by
the selected adapter and exactly `high` or `xhigh`; omission and silent
clamping are invalid. Budget includes wall time, model input/output/token/cost,
tool invocations, subprocesses, disk, and optional dependency-download limits.
The capability/tool manifest is hashed into the route snapshot.

### 5.2 No fallback

These outcomes enter visible `StrongRouteUnavailable` / `RouteUnavailable`:

- `MR_ROUTE_MISSING`, `MR_ROUTE_ALIAS_AMBIGUOUS`, `MR_ROUTE_WEAK`,
  `MR_ROUTE_INVALID`, or `MR_ROUTE_ADAPTER_UNSUPPORTED` for absent/ambiguous/
  weak/invalid configuration;
- `MR_ROUTE_EXECUTOR_UNAVAILABLE`, `MR_ROUTE_PREFLIGHT_MISMATCH`,
  `MR_ROUTE_AUTH_FAILED`, `MR_ROUTE_REASONING_UNSUPPORTED`,
  `MR_ROUTE_SANDBOX_UNAVAILABLE`, `MR_ROUTE_TIMEOUT`,
  `MR_ROUTE_BUDGET_EXHAUSTED`, or `MR_ROUTE_SERVICE_STOPPED` for execution
  infrastructure; and
- `MR_ROUTE_RUNTIME_DRIFT` when runtime reports a different provider/model/
  reasoning than the snapshot.

There is no weak-tier downgrade, provider/model substitution, alias
re-resolution, cross-executor fallback, inherited `models.evaluator`, or
conversion to `MechanicalMerge`. Automatic worker retry/escalation settings do
not apply. A retry repeats the exact snapshot only through an explicit new run
generation. `wg merge-resolution change-route` resolves a new snapshot and
creates a new generation/audit event; it never mutates an in-flight or completed
request. Old sessions, charges, and output remain attributable to the old route.

This boundary exists because availability says nothing about the authority or
quality of another route. Silent substitution would make the recorded decision
producer false.

## 6. Isolated integration repository and merger capabilities

### 6.1 Materialization

After route resolution, the workspace manager creates a dedicated **standalone
integration clone/worktree** (a repository with its own private Git directory)
from the exact target commit/tree and imports the
candidate/base objects by verified OID/CID. It is not the source worktree and
not a linked worktree of the canonical repository.

Required containment:

- a private Git directory and object database owned by the run; no writable
  alternates, common-dir, canonical ref namespace, ref-update socket, remote,
  credential helper, signing key, hooks, or push URL;
- canonical repository `.git`, main checkout, source candidate worktree,
  `.wg/graph.jsonl`, lifecycle ledger, service socket, agent registry, user
  sessions, and unrelated home/config paths are absent from the mount
  namespace;
- the original candidate/base/target objects and bundle CAS are read-only;
  writable source and build directories exist only inside the run repository;
- no `wg done/fail/retry/msg/config/merge`, Git push, canonical ref update, or
  evaluator/reviewer-authority tool is present; environment variables and PATH
  are allowlisted and contain no canonical path or push credential;
- network is denied by default. Pinned dependency installation may use only the
  snapshotted repository/package endpoints and cache under bounded policy; its
  lock/provenance/output digests are evidence;
- repository tools, editor, shell, compiler, tests, and pinned generators are
  allowed inside the workspace under subprocess, time, disk, and output
  budgets;
- a workspace lease and filesystem/process containment identity are recorded,
  and all descendants are reaped before sealing.

An ordinary linked Git worktree is insufficient because its `.git` file points
into the canonical repository's common directory. A process with normal Git or
filesystem access can update shared refs, hooks, config, index/worktree
metadata, or objects and may reach credentials/remotes. A linked worktree is
permitted only if an OS/mechanical sandbox makes the shared administration area
read-only, supplies a private writable object/ref layer, denies ref transactions
and remotes, and tests those denials. Since a standalone private clone is
simpler to prove, it is the normative first implementation.

### 6.2 Model-visible bundle

The runner receives one content-addressed `ResolutionEvidenceBundleV1`. It
contains:

- original task/user intent and completion contract with provenance;
- immutable source candidate descriptor and bounded manifest/artifact views;
- merge base and target snapshot descriptors;
- candidate-vs-base, target-vs-base, and candidate-vs-target diffs;
- deterministic conflict map and classifier reason/evidence;
- candidate-alone, target-alone, dry-merge, and combined-check receipts;
- dependency/lock/submodule metadata and revisions;
- generated-file ownership, input/output mapping, generator argv/toolchain and
  prior determinism evidence;
- accepted original validation/evaluation evidence (as evidence, not authority
  over new bytes);
- applicable repository, safety, merge, tool, and human-authority policies; and
- exact available repository tool/capability manifest.

Every object is pinned by CID/OID and length. Branch/path names are diagnostics
only. Each model-visible item is framed as spotlighted untrusted data:

```text
BEGIN UNTRUSTED MERGE EVIDENCE
kind=<enum> cid=<cid> bytes=<n> trust=<label>
<exact normalized bytes>
END UNTRUSTED MERGE EVIDENCE cid=<same cid>
```

Delimiter recognition also checks length and CID. Candidate, target, conflict,
log, generated, and human-provided bytes cannot request a route change, tool
addition, policy override, credential, or output-schema change. A fixed system
contract and capability manifest are outside the untrusted frame. Accepted but
untrusted material is least-privilege framed; rejected/quarantined material is
never exposed.

## 7. Append-only resolution transaction

### 7.1 Projection and lifecycle mapping

```text
Classifying
  -> MechanicalPending -> AcceptancePending -> Merged
  -> CandidateRepairRequired
  -> HumanDecisionRequired
  -> SecurityBlocked
  -> ResolutionRequired -> StrongRouteResolved -> IntegrationWorkspaceReady
       -> Resolving -> ResolutionCandidateSealed -> SafetyReview
       -> Revalidating -> Reevaluating -> AcceptancePending -> Merged

Any applicable phase -> RouteUnavailable | ResolutionRejected |
                        HumanDecisionRequired
```

This is a resolution projection, never `Task.status`.

| Resolution projection | Canonical task/attempt | Finalizer/worktree projection | Lifecycle effect |
|---|---|---|---|
| `Classifying`, `MechanicalPending`, `ResolutionRequired`, `StrongRouteResolved`, `IntegrationWorkspaceReady`, `Resolving`, `ResolutionCandidateSealed`, `SafetyReview`, `Revalidating`, `Reevaluating`, `AcceptancePending` | `AwaitingAcceptance`; source attempt remains immutable `Succeeded` | source candidate `Sealed`; resolution workspace has its independent lease | None; append resolution evidence/actions only |
| `CandidateRepairRequired` | normally `AwaitingAcceptance` until pinned rejection/repair policy asks kernel; source attempt stays `Succeeded` | source/rescue retained | Acceptance controller may request the upstream design's `AcceptanceRejected`/authorized repair; resolver never does |
| `HumanDecisionRequired`, `SecurityBlocked`, `RouteUnavailable` | `AwaitingAcceptance` unless explicit policy/operator later rejects | source and all prepared evidence retained; workspace held or cleaned only after seal | No canonical transition |
| `ResolutionRejected` | acceptance controller applies pinned rejection policy; never reopens source attempt | both candidates and evidence retained; linked repair may be authorized | Only lifecycle kernel can commit rejection/new generation |
| `Merged` before acceptance link | `AwaitingAcceptance` | `Integrated(receipt)` | Merge receipt only |
| accepted `Merged` | `Done`, source attempt still `Succeeded` | cleanup pending after receipt/acceptance | Kernel commits `AcceptanceSatisfied`; cleanup remains ancillary |

A security block does not silently become task failure because the review gate
is evidence and policy may require human disposition. It can never permit
consumption. A route outage likewise says nothing about source correctness.

### 7.2 Persisted records

```rust
struct MergeResolutionRecordV1 {
    schema: u16,
    classification_id: String,
    source: SourceCandidateBinding,
    target: TargetHeadSnapshotV1,
    merge_attempt_receipt_cid: Cid,
    evidence_bundle_cid: Cid,
    policy_snapshot_cid: Cid,
    classification: MergeClassification,
    state: ResolutionState,
    generations: Vec<ResolutionRunGenerationV1>,
    human_decisions: Vec<HumanMergeDecisionV1>,
    resolution_candidates: Vec<Cid>,
    merge_receipt_cid: Option<Cid>,
    created_by_event: EventId,
}

struct ResolutionRunGenerationV1 {
    generation: u32,
    resolution_request_id: String,
    route_snapshot_cid: Cid,
    session_id: Option<String>,
    process_identity: Option<ProcessIdentity>,
    workspace_receipt_cid: Option<Cid>,
    runner_receipt_cid: Option<Cid>,
    outcome_descriptor_cid: Option<Cid>,
    state: RunState,
    supersedes_generation: Option<u32>,
}
```

The smallest safe persistence shape is a serde-defaulted resolution-reference
map on the upstream finalization record, with large bundles, descriptors,
transcripts, and receipts in content-addressed storage. It is not a graph task:
no task ID, `after` edge, assignment, worker status, source retry count, or weak
agency queue row is created.

One run generation is lazily created only after classification is a genuine
strong-resolvable textual, semantic, or deterministic-generated conflict and
an exact route snapshot exists. CAS key is
`(resolution_request_id, generation, slot_absent)`. Duplicate enqueue returns
the same run. A second model invocation after malformed output, timeout, human
resume, or operator retry requires an explicit new generation and audit event;
there is never an unrecorded “try again for a better answer.”

### 7.3 Outbox keys

```text
classify:<classification-id>
review-input:<classification-id>:<review-policy-cid>
resolve-route:<classification-id>:<config-revision-cid>
prepare-workspace:<resolution-request-id>:g<generation>
start-run:<resolution-request-id>:g<generation>:<route-cid>:<workspace-cid>
seal-resolution:<resolution-request-id>:g<generation>:<runner-receipt-cid>
review-resolution:<resolution-descriptor-cid>:<review-policy-cid>
validate-resolution:<resolution-descriptor-cid>:<validation-policy-cid>
evaluate-resolution:<resolution-descriptor-cid>:<evaluation-policy-cid>:<route-cid>:<slot>
accept-resolution:<resolution-request-id>:<resolution-descriptor-cid>:<target-commit>:<evidence-set-cid>
merge-receipt:<resolution-request-id>:<resolution-descriptor-cid>:<target-commit>
cleanup-resolution:<workspace-id>:<lease-epoch>:<terminal-record-cid>
```

Outbox states use the upstream
`Pending | Claimed | ReceiptAvailable | Succeeded | Cancelled | OperatorHold`
semantics. A claim is an expiring execution lease; it grants no content or
lifecycle authority.

## 8. Strong execution and immutable output

### 8.1 Runner protocol

The adapter preflights exact handler/provider/model/reasoning and containment,
then starts one full coding session. It records argv, executable digest,
reported route, session ID, process identity, tool calls, usage/cost, command
and output CIDs, budget counters, exit/settlement, and attempted policy denials.
The merger must terminate through a strict tool/schema with exactly one outcome:

- `resolved`: proposed integrated tree is complete and conflict-free;
- `reject`: evidence shows the candidate should not be integrated; or
- `needs_human`: identifies a bounded ambiguity reason and evidence refs,
  without choosing the missing intent.

Free-form “done” prose, zero exit, a commit, or a clean `git status` is not an
outcome. Missing/malformed/over-budget output is infrastructure/invalid-output
evidence and cannot be accepted.

For `GeneratedArtifactConflict`, the capability broker checks that generated
paths changed only as output of the pinned command after source-input edits. It
records pre/post source digests, generator argv/toolchain, two-run determinism
where policy requires it, and output digests. A direct edit to a generated path
is `MR_GENERATED_OUTPUT_HAND_EDITED` and rejects the proposal.

### 8.2 Sealing

After the exact process group is fenced and reaped, the candidate finalizer—not
the model—rescans the workspace, verifies no post-exit mutation, writes Git
objects with a private index, computes canonical manifests, and seals:

```rust
struct ResolutionCandidateDescriptorV1 {
    schema: u16,
    resolution_candidate_id: Cid,
    resolution_version: u32,
    outcome: Resolved | Reject | NeedsHuman,

    classification_id: String,
    resolution_request_id: String,
    run_generation: u32,
    session_id: String,
    route_snapshot_cid: Cid,
    adapter_receipt_cid: Cid,
    workspace_receipt_cid: Cid,

    parent_candidate_id: Cid,
    parent_candidate_commit_oid: GitOid,
    parent_candidate_tree_oid: GitOid,
    merge_base_commit_oid: GitOid,
    merge_base_tree_oid: GitOid,
    target_snapshot_commit_oid: GitOid,
    target_snapshot_tree_oid: GitOid,

    resolution_commit_oid: GitOid,
    resolution_tree_oid: GitOid,
    content_manifest_cid: Cid,
    delta_manifest_cid: Cid,
    changed_files_cid: Cid,
    conflict_disposition_cid: Cid,

    generator_receipts: Vec<GeneratorReceipt>, // argv/toolchain/input/output digests
    test_tool_evidence_cids: Vec<Cid>,
    pre_exposure_review_cid: Cid,
    runner_safety_evidence_cids: Vec<Cid>, // denials, secret scan, capability audit
    explanation_cid: Cid,
    unresolved_or_human_reason: Option<StableReasonCode>,
    finalization_event_id: EventId,
}
```

As with upstream descriptors, the ID is a derived envelope field excluded from
its own canonical hash preimage. Each `GeneratorReceipt` names source inputs,
generated outputs, exact argv, toolchain/environment CID, input/output digests,
and determinism result. The descriptor's safety fields cover pre-exposure and
runner/containment evidence; the necessarily later canonical output-review
verdict is linked to the immutable descriptor as acceptance evidence rather
than creating a self-referential descriptor. The descriptor records a complete
integrated tree rooted at the target snapshot, not merely a patch. Even
`reject` and `needs_human` outcomes preserve the final workspace tree and
evidence when present, but those trees are never eligible for acceptance.

The source `CandidateDescriptor`, its immutable ref/version, source worktree,
and accepted original evidence are never rewritten, retagged, force-updated, or
made children of the resolution. Resolution version slots are finalizer-owned
and create-only. Mutation after the seal checkpoint causes
`MR_WORKSPACE_MUTATED_AFTER_SEAL`; the descriptor remains retained evidence and
cannot advance.

## 9. Fresh safety, validation, and evaluation

Every byte modified or resolved in the workspace is new untrusted candidate
content. Original validation/evaluation proves only the parent candidate and
serves as context.

For a `resolved` descriptor the mandatory order is:

1. Verify descriptor CID, tree/manifest equality, route/runner/workspace
   receipts, target parent, and conflict disposition.
2. Run the canonical content-safety review over the complete new delta,
   generated outputs, model explanation, tool output selected by policy, and
   provenance. Review is bound to the resolution descriptor. Reject/quarantine
   cannot be downgraded by merger, evaluator, coordinator, or human approval.
3. Run deterministic validation against a detached, no-shared-ref,
   read-only source materialization of the exact resolution tree, with build
   overlay as required. This includes candidate tests and every integration
   command used by classification. Record source-before/source-after equality.
4. Create policy-selected evaluation lazily against the
   `ResolutionCandidateDescriptor`, using the Pi evaluation-plane contracts:
   exact candidate/tree/manifest, policy and route snapshots; bounded or
   deep-readonly product; dedicated agency lane; read-only capabilities; no
   route fallback; write-once/link-once/consume-once.
5. The acceptance controller consumes the fresh exact evidence set. Original
   candidate verdicts cannot fill a resolution-candidate slot, and a stale
   resolution verdict is unlinked evidence.

A deterministic, review, or required-evaluation reject becomes
`ResolutionRejected`. Evaluator infrastructure failure remains a visible
`Reevaluating`/operator hold under required policy, not source failure.
Advisory evaluation may follow the upstream policy only if that policy permits
acceptance without it; it still binds the resolution descriptor and cannot
retroactively rewrite acceptance.

## 10. Central acceptance, equality, and exactly-once merge

Only the upstream central merge authority receives
`ResolutionAcceptanceRequestV1`. It verifies:

- classification/request/run/route/workspace/descriptor CIDs and ancestry;
- `outcome == Resolved`;
- fresh accepted review, deterministic validation, and required evaluation or
  manual-policy evidence all bind the exact descriptor;
- the complete resolution tree is based on the exact target snapshot and its
  candidate projection/conflict dispositions match policy;
- no generated output violates the generator receipt; and
- the canonical target ref's commit **and tree** still equal the snapshot.

The authority imports the descriptor's verified objects into a private
canonical preparation namespace and creates an integration commit whose tree is
the accepted `resolution_tree_oid`. Commit metadata/parents may make its commit
OID different from the private workspace commit, but content equality means:

1. same Git object format, bytewise path names, entry kinds and modes;
2. every blob/symlink/gitlink OID equal;
3. complete recursive tree OID equal when object format is the same; and
4. canonical full-tree manifest CID equal in all cases.

No checkout conversion, generated rerun, formatter, conflict edit, or
candidate-projection-only comparison is allowed during acceptance. The **whole
merged tree**, not merely candidate-controlled paths, must equal the accepted
resolution tree.

The receipt key is:

```text
BLAKE3("wg-resolution-merge-receipt-v1\0" ||
       resolution_request_id || resolution_candidate_id ||
       target_ref_id || expected_target_commit/tree)
```

`MergeReceiptV1` adds parent candidate/base/target IDs, resolution and resulting
commit/tree/manifest digests, input evidence-set CID, private preparation ref,
ref-transaction/CAS proof, and lifecycle link event. The authority fsyncs the
prepared objects/ref, rechecks equality, atomically CASes target from expected
head to the prepared commit, and writes/links one receipt. Duplicate delivery
returns the same receipt. A crash after CAS but before receipt reconstructs only
that receipt from the immutable preparation ref and reflog/ref-transaction
proof; it never applies the tree again.

If target commit or tree moved at any point, append `MR_TARGET_MOVED`, mark this
resolution stale, retain all evidence, and create a **new classification** only
from an operator/policy-authorized fresh target snapshot. Never automatically
rebase, replay the old patch, reuse the old verdict, or merge a stale result.
The new classification may be clean, conflict differently, or require a new
strong run.

## 11. Crash barriers and replay

| Boundary | Durable before effect | Replay/convergence rule |
|---|---|---|
| classification | classification action + exact input tuple | recompute and require identical typed outcome/evidence digest; disagreement holds |
| input safety | review request | identical verdict links once; conflicting verdict quarantines; blocked input never reaches route resolution |
| route resolution | config/profile/catalog snapshot request | same config revision yields same route CID; drift requires new generation |
| workspace creation | workspace action + lease epoch | verify private Git identity, objects and mount policy; absent workspace recreate, ambiguous/source-bearing workspace retain |
| process start | run record with route/workspace/budget and launch nonce | process identity/session receipt before prompt; unknown launch outcome reconcile, never launch a second session blindly |
| process exit | exact wait/group-empty/tool transcript receipt | fence/reap; ambiguous descendants hold and workspace remains source-bearing |
| candidate seal | seal action + final manifest observation | create-only objects/ref; same bytes return same descriptor; drift rejects |
| safety request/result | descriptor-bound action | link exact review once; no downgrade or alternate content |
| validation request/result | descriptor/policy/toolchain action | duplicate computation may occur but one exact result slot links; source mutation invalidates result |
| evaluation request/result | upstream evaluation ID/action | Pi-plane write/link/consume-once rules; never reinvoke after valid verdict exists |
| acceptance request | exact evidence-set and target expectation | stale/missing evidence cancels; no target write |
| target CAS | prepared ref + equality receipt journal | one ref transaction; mismatch means target moved |
| merge receipt | deterministic receipt key and CAS proof | reconstruct/link same receipt only |
| cleanup | accepted/rejected terminal evidence + workspace lease | ancillary retry; semantic state unchanged; unknown/source-bearing objects retained |

Daemon startup replays lifecycle first, verifies upstream candidate and target
objects, ingests resolution receipts, reconciles exact processes/workspaces,
cancels stale actions, and resumes the first incomplete current barrier.
Cleanup is last. No current-main inspection is allowed to infer that a run
succeeded. Post-checkpoint workspace mutation, missing ownership, or an unknown
side effect fails closed and retains the repository.

## 12. Human decision, rejection, retry, and rollback

### 12.1 Human record

```rust
struct HumanMergeDecisionV1 {
    schema: u16,
    decision_id: Cid,
    classification_id: String,
    candidate_commit_tree_manifest: ContentBinding,
    target_commit_tree_manifest: ContentBinding,
    evidence_bundle_cid: Cid,
    policy_snapshot_cid: Cid,
    prior_resolution_candidate_cid: Option<Cid>,
    author_identity: String,
    decision: ChooseIntent | ChangePolicy | Reject | RequestMoreEvidence,
    rationale_cid: Cid,
    constraints_cid: Cid,
    created_at: Timestamp,
    idempotency_key: String,
}
```

The CLI presents both plausible behaviors and evidence refs without executing
either. `resume` never continues a mutable old session. It creates a new
resolution generation, new spotlighted bundle containing the decision, and
ultimately a new descriptor. If the target moved, the decision is stale unless
the author explicitly rebinds it after reviewing the new evidence.

Human approval does not bypass pre-exposure or output safety, deterministic
validation, fresh evaluation/FLIP, complete-tree equality, or target CAS. A
human cannot downgrade a hard safety verdict through this record; policy change
must happen through its separate authorized mechanism and new snapshot.

### 12.2 Failure actions

Budget exhaustion, repeated invalid output, generated ownership ambiguity,
route unavailability, target movement, and ambiguous intent expose only typed
actions:

- `inspect` pinned bundle, route, transcript, denial, descriptor, and gate;
- `retry` exact route as a new audited generation;
- `change-route` to one newly snapshotted strong route as a new generation;
- `reject` through acceptance/lifecycle policy;
- `repair-source` through a linked new source candidate version;
- `decide`/`escalate-human` with a bound human record; or
- `refresh-target` and reclassify from a new snapshot.

Before acceptance, abort/reject first retains immutable transcripts, manifests,
descriptors, and refs, then releases the integration workspace if retention
proof permits. Main and source remain untouched.

After acceptance, rollback is **never** `git reset`, receipt deletion, ref force,
or descriptor erasure. `wg merge-resolution rollback <receipt>` creates a new
auditable compensating task/candidate whose intent is to reverse the accepted
change relative to the then-current target. It traverses ordinary candidate
finalization, safety, deterministic validation, selected evaluation, classifier,
and central target CAS. The original receipt and commit remain permanent. A
conflicting or ambiguous compensation can itself require a strong/human
resolution.

## 13. Operator and TUI contract

Proposed surfaces:

```text
wg merge-resolution status <TASK|CLASSIFICATION|REQUEST> [--json]
wg merge-resolution inspect <ID> [--materialize DIR]
wg merge-resolution retry <REQUEST>          # exact route, new generation
wg merge-resolution change-route <REQUEST> --route <handler:model> --reasoning high|xhigh
wg merge-resolution decide <ID> --decision ... --rationale ... --constraints ...
wg merge-resolution resume <ID>
wg merge-resolution reject <ID> --reason ...
wg merge-resolution refresh-target <ID>
wg merge-resolution abort <ID>
wg merge-resolution rollback <MERGE-RECEIPT>
```

`wg show`, `wg finalize status`, `wg merge status`, service JSON, and TUI/Viz
inspector display:

- primary classifier outcome/reason and evidence CIDs, including independent
  candidate/target/combined command receipts;
- source candidate ID/version/commit/tree/manifest and target/base
  commit/tree/manifest;
- review verdict and whether untrusted evidence was blocked or spotlighted;
- exact route, handler/provider/model, declared strength, reasoning,
  config/profile/catalog provenance, tool policy and budget; explicit
  `no fallback`;
- request/run generation/session/process and workspace isolation/lease/denial
  proofs;
- resolution descriptor CID/outcome/tree/manifest, changed-file and generator
  receipts;
- safety, validation, evaluation/FLIP states and exact bindings;
- target movement/staleness, acceptance action, merge receipt/CAS and complete
  result tree;
- retained source/resolution refs and cleanup state; and
- one safe next action from the typed list above.

Example hold:

```text
Merge resolution: route unavailable (MR_ROUTE_AUTH_FAILED)
  candidate: wgcid:... tree 9ab...  target: 41c... tree 73d...
  conflict: MR_TEXTUAL_OVERLAP  evidence: wgcid:...
  route: pi:openai-codex:gpt-5.6-sol / premium / xhigh (pinned, no fallback)
  main changed: no  merger calls: 0
  next: wg merge-resolution change-route <id> --route <exact> --reasoning xhigh
```

Arbitrary untrusted text is never rendered as the reason or next command. The
TUI invokes the same typed request APIs; it has no direct Git/model/status path.

## 14. RED-first credential-free validation

### 14.1 Fake strong-merger adapter

`tests/fixtures/fake-strong-merger/` is a deterministic adapter executable. It
requires provider credentials to be unset and records exact argv, route,
provider/model, reasoning, config/profile revision, budget, capability/tool and
sandbox policy CIDs, bundle CID, workspace identity, prompt count, and tool
calls. It fails unless the route is fully handler-first, class is
strong/premium, reasoning is high/xhigh, forbidden paths/credentials are
absent, and allowed tools match exactly. A shared counter proves invocation
count.

Fixtures:

| Fixture | Expected result/calls |
|---|---|
| clean merge | `MechanicalMerge`; **0** strong-merger calls and **zero model calls of any kind** |
| candidate-invalid | `CandidateRepairRequired`; **0** calls |
| textual overlap | one exact route and one valid resolution; **1** call |
| semantic integration | each side passes alone, clean merge fails combined check; **1** call |
| deterministic generated | edits source input, runs pinned generator, output digest matches; **1** call |
| generated ambiguous | `HumanDecisionRequired`; **0** calls |
| product ambiguity known from policy | `HumanDecisionRequired`; **0** calls |
| merger discovers ambiguity | one `needs_human` output; **1** call and no acceptance |
| malicious conflict text | pre-review block gives **0** calls, or accepted test variant is framed data and gets exactly one call; route/tool/policy remain unchanged |

Unit/model/property tests also assert:

- all decision-table rows are mutually exclusive and precedence is stable;
- candidate failure cannot be relabeled semantic, and target failure cannot be
  repaired by the candidate;
- policy-only combined failures are not mechanical;
- marker, submodule, lockfile, mode, rename, delete and generated ownership
  classifications are deterministic;
- complete resolution tree—not a projection—equals accepted tree;
- no actor other than central authority can update canonical refs.

### 14.2 Failure and replay matrix

Credential-free fault tests cover:

1. absent, weak, malformed, unsupported-reasoning, unavailable, auth-failed,
   route-drift and stale-config routes; fake fallback executables are sentinels
   and must never run;
2. malformed schema, free-form prose, timeout, cost/token/tool/disk exhaustion,
   abrupt exit and repeated invalid merger output;
3. explicit `reject`, linked repair, and later valid new descriptor;
4. duplicate classification, enqueue, process receipts, descriptor, review,
   validation, verdict, acceptance and merge receipts;
5. daemon kill/restart before and after every §11 barrier;
6. concurrent target movement before run, during run, before acceptance and
   immediately around CAS; old resolution never rebases or merges;
7. operator route change while old process runs; old generation cannot satisfy
   the new slot;
8. attempted source worktree, canonical main, shared ref, remote/push, graph,
   lifecycle and evaluator mutation; each is mechanically denied and audited;
9. post-checkpoint workspace mutation and descendant writer; seal/acceptance
   blocked and source retained;
10. generated hand-edit, unavailable/nondeterministic generator, disagreement
    over authoritative output, malicious dependency install hook, and unknown
    policy label;
11. safety verdict downgrade attempts by merger/evaluator/human; and
12. abort/reject retention, cleanup failure, stale human decision, compensating
    rollback conflict, and duplicate rollback request.

The call assertions above count **strong-merger invocations**; fresh review or
evaluation adapters have their own independent counters and authority and can
never perform the resolution. Credential-free fixtures use deterministic fake
gates. The reference model randomizes candidate/base/target and route generations,
review/validation/evaluation delivery, target CAS, process/workspace epochs and
crashes. Properties include zero merger calls for
clean/blocked rows, at most one merger call per run generation, immutable
source/resolution versions, no stale action
mutating current state, one target CAS/receipt, accepted-tree equality, and no
source-bearing object becoming GC-eligible without a terminal retention record.

### 14.3 Installed-binary live flow

Before implementation, add
`tests/smoke/scenarios/strong_agent_merge_resolution.sh`, register it in the
grow-only manifest with:

```toml
owners = ["implement-strong-agent"]
```

The scenario must first be demonstrated RED on pre-change main. It builds and
installs `wg` with `cargo install --path . --locked` into an isolated prefix and
uses that installed binary for the entire flow. It starts a real daemon/service
with isolated `HOME`, graph, registry and real source worktree; creates a real
standalone integration repository; and uses the fake merger only to avoid model
credentials. Direct Rust helpers or a main-worktree-only script do not satisfy
it.

One terminal script demonstrates:

1. clean candidate -> credential-free classifier -> zero fake calls -> exact
   mechanical CAS/receipt;
2. textual conflict -> visible exact strong route -> isolated workspace -> one
   merger call -> sealed resolution -> canonical safety -> deterministic
   revalidation -> fresh evaluation -> exactly-once merge whose complete tree
   and manifest equal the descriptor;
3. duplicate delivery and daemon restart at rotating durable barriers return
   the same IDs/receipt and charge no duplicate call after valid output;
4. ambiguous intent and generated ownership stop for a human record; resume
   creates a new generation and still traverses every gate;
5. merger rejection retains both candidates, then a linked repair/new version
   succeeds;
6. target movement makes the prepared resolution stale and reclassifies against
   a fresh target with no automatic rebase;
7. sandbox probes fail to mutate source/main/graph/shared refs and status renders
   the denial; and
8. an accepted change is reversed only by an explicit compensating rollback
   candidate through the full pipeline, leaving the original receipt visible.

A tmux/PTY leg drives `wg tui` to the inspector and performs one human stop and
resume through actual key handling. It asserts classifier reason, exact route
and strength, no-fallback hold, run/session/workspace, gate progression,
resolution CID, receipt, retained artifacts, and safe next action. This is the
permanent live human-flow proof.

## 15. File-level implementation seams and rollout

### 15.1 Exact seams

The implementation builds on the modules required by the candidate and Pi
designs; it must not resurrect `attempt_worktree_merge` as a second authority.

| File/module | Required change |
|---|---|
| `src/finalization/descriptor.rs` | Add `TargetHeadSnapshotV1`, `ResolutionCandidateDescriptorV1`, generator/workspace/runner bindings and CID verification; candidate finalizer owns create-only version slots. |
| `src/finalization/merge.rs` | Replace current inline merge with pinned dry-run evidence, mechanical classification input, private preparation ref, complete-tree equality, target CAS and exactly-once receipts. Remains the only canonical merge authority. |
| `src/finalization/outbox.rs` | Add classification/resolution/gate/acceptance keys, barrier receipts, stale cancellation and startup replay order. |
| `src/finalization/validation.rs` | Independent candidate/target/combined receipts and fresh descriptor-bound resolution validation. |
| `src/finalization/retention.rs` | Retain source and resolution objects; cleanup only after terminal evidence; rollback receipt ancestry. |
| `src/merge_resolution/mod.rs` (new) | Public states, classifications/reason codes, IDs, records, projection only; no lifecycle writes. |
| `src/merge_resolution/classifier.rs` (new) | Total credential-free precedence/decision table, marker/generated/policy rules and evidence validation. |
| `src/merge_resolution/route.rs` (new) | `ResolutionRouteSnapshotV1`, `models.merger`/allowed-alias one-time resolution, strong/reasoning/availability checks, no-fallback errors. |
| `src/merge_resolution/bundle.rs` (new) | Canonical spotlighted bundle, redaction, CIDs/lengths, conflict/generated/dependency evidence. |
| `src/merge_resolution/workspace.rs` (new) | Standalone private clone/object import, containment/lease receipts, mutation probes and cleanup. |
| `src/merge_resolution/adapter.rs` and `runner.rs` (new) | Executor-neutral full coding adapter, exact route preflight, strict terminal outcome, budget/process/tool receipts. No worker retry cascade. |
| `src/merge_resolution/store.rs` (new) | Create-once CAS objects for bundles, transcripts, human decisions and descriptors; conflict quarantine. |
| `src/merge_resolution/human.rs` (new) | Bound decision/resume generations and stale-target checks. |
| `src/lifecycle.rs` (or authoritative `src/lifecycle/{kernel,event,projector}.rs`) | Typed evidence link, rejection/repair, acceptance and rollback-candidate requests. No resolution actor gains status mutation. |
| `src/evaluation/{mod,policy,queue}.rs` from the Pi design | Accept `ResolutionCandidateDescriptor` as a new immutable source binding and force fresh policy-selected records; evaluator remains read-only. |
| `src/review/{mod,pass1_lint,verdict}.rs` | Canonical pre-exposure and post-output review entry points with no verdict downgrade; persist resolution CIDs. |
| `src/config.rs` | Add `DispatchRole::Merger`/`models.merger`, reasoning, rollout and budgets; serde defaults. Validate strong/premium and high/xhigh. |
| `src/dispatch/handler_for_model.rs` | Parse the already exact handler-first merger route only; never perform provider/model fallback. |
| `src/dispatch/plan.rs` | Separate `MergeResolution` execution plan/capability manifest, not a normal task-agent/agency plan. |
| `src/commands/done.rs` | Remove `attempt_worktree_merge` and `--ignore-unmerged-worktree` authority; submit finalization intent only. Legacy flag becomes a loud migration error/advice. |
| `src/commands/service/coordinator.rs` | Tick resolution outbox after lifecycle reconciliation; no conflict editing, ordinary task scaffolding, weak queue, or worker-slot fallback. |
| `src/commands/service/worktree.rs`, `src/commands/worktree_cmd.rs`, `src/commands/worktree_gc.rs` | Recognize integration workspace leases/retention; never force-remove unresolved/source-bearing workspaces. |
| `src/commands/merge_resolution.rs` (new), `src/cli.rs`, `src/main.rs` | §13 typed CLI and JSON surfaces. |
| `src/commands/show.rs`, `src/commands/status.rs`, service IPC | Classifier/route/run/gate/receipt/retention diagnostics and safe next action. |
| `src/tui/viz_viewer/{state,render,event}.rs`, `src/commands/viz/*` | Inspector and typed human decision/resume action; no direct Git/status path. |
| `tests/fixtures/fake-strong-merger/*` | Exact route/reasoning/tool/sandbox assertions, deterministic outcomes and call counter. |
| unit/property tests under `src/merge_resolution/*` and `src/finalization/*` | Decision table, authority, replay, equality, route and containment matrices. |
| `tests/smoke/scenarios/strong_agent_merge_resolution.sh` | Real installed binary/daemon/source + standalone integration repository terminal/TUI flow. |
| `tests/smoke/manifest.toml` | Grow-only entry owned by `implement-strong-agent`. |

### 15.2 Persistence migration and compatibility

Add versioned, serde-defaulted resolution references to the finalization/attempt
projection and append-only ledger events. Store large objects under:

```text
.wg/finalization/merge-resolution/cas/b3/<digest>
.wg/finalization/merge-resolution/records/<classification-id>.json
.wg/finalization/merge-resolution/runs/<request-id>/g<generation>.json
.wg/finalization/merge-resolution/receipts/<receipt-id>.json
```

The authoritative ledger/index, not filenames, decides linkage. Writes use
create-new, fsync file and parent, canonical encoding, and CID verification.
Schema readers accept old graphs because fields default empty; writers emit v1.
Unknown newer required fields fail closed.

Legacy conflicts migrate as follows:

- a conflict with an exact immutable candidate/base/target and preserved merge
  evidence imports as `LegacyConflictObserved`, then is reclassified under a
  new policy snapshot; no old prose classification is trusted;
- a conflict naming only branch/path/current main becomes
  `MR_BINDING_MISMATCH`/operator hold. Preserve all refs/worktrees; never invent
  candidate or target CIDs;
- partially recorded model edits are untrusted retained bytes, not a resolution
  candidate. They require a fresh explicit run or source repair;
- historical manual merges remain legacy acceptance evidence and are not
  rewritten. No modern complete-tree/CAS receipt is fabricated;
- old `.merge-*` graph tasks are display-only legacy records, cannot dispatch,
  and are archived after exact source linkage. They are not converted to weak
  or strong runs; and
- old cleanup markers do not authorize deletion until modern reachability and
  retention checks pass.

### 15.3 Rollout

1. **Disabled (default):** schemas, migration diagnostics, CLI/status only.
2. **Classifier shadow:** recompute outcomes and metrics without merge/model
   effects. Record counts for clean, source-invalid, textual, semantic,
   generated, human, safety and inconclusive; disagreement with legacy logic
   holds, never delegates to legacy.
3. **Mechanical canary:** enable only `MR_MECHANICAL_CLEAN` after zero-LLM and
   complete-tree/CAS fault tests. This path does not load, probe, authenticate,
   or otherwise depend on merger models.
4. **Fake strong canary:** named repositories/tasks run the installed-binary
   smoke with exact fake route and containment.
5. **Live advisory strong canary:** named conflicts may produce descriptors but
   require manual observation before acceptance; collect route, budget,
   invalid-output, human-stop, review and equality metrics.
6. **Selective enabled:** explicit per-project policy enables strong runs after
   a recorded compatible canary certificate. Global implicit conflict
   resolution remains unsupported in the first release.
7. Remove legacy inline merge/`.merge-*` dispatch only after recovery reports no
   partially bound conflicts. Never remove fail-closed legacy retention.

Classifier metrics contain reason codes and CIDs, not conflict text. Model
availability cannot alter the mechanical classifier or clean-merge bytes. A
fresh install remains disabled, and stage skipping is rejected.

## 16. Acceptance checklist

Implementation conforms only if:

- the deterministic table distinguishes clean, candidate-invalid, target-invalid,
  textual, semantic, generated resolvable/ambiguous, product/policy ambiguity,
  and malicious/untrusted content with the stated precedence;
- clean integration makes zero model calls and no resolution graph clutter;
- one fully qualified snapshotted strong route runs per generation, with
  strong/premium class and high/xhigh reasoning, and every unavailable/weak
  route holds without fallback;
- the merger has coding/test capability only in a standalone integration
  repository and cannot mutate canonical/source/shared refs, graph, lifecycle,
  review, or evaluation;
- modified bytes become a new immutable resolution descriptor and receive fresh
  safety, deterministic validation, and policy-selected evaluation/FLIP;
- central acceptance compares the complete descriptor tree, performs one target
  CAS, and produces/replays one receipt; target movement always reclassifies;
- human decisions, rejection/repair, retention, crash replay, route generations,
  abort and compensating rollback are content-bound and visible; and
- RED fixtures and the permanent installed-binary daemon/worktree terminal/TUI
  smoke are green and owned by `implement-strong-agent`.

## 17. Final rule

> The classifier proves the zero-model case. One snapshotted strong merger may
> propose changed integrated bytes in isolation. A human alone supplies missing
> product or policy intent. The finalizer binds every proposed byte, independent
> gates judge that exact descriptor, and the central merge authority alone CASes
> the exact accepted tree into main.

No coordinator edit, evaluator guess, weak-tier downgrade, route fallback,
shared-ref worktree, stale target, human shortcut, or cleanup/restart path may
bypass that chain.
