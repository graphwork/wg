# FLIP / Eval evolution: from metric and gate to two overlapping contractual reviewers

**Status:** research survey (no behavioural change). **Task:** `survey-how-flip`.
**Method:** git archaeology (`git log -S`, `git blame`, `git show`) over the
FLIP/Eval prompts, protocol constants, and config history; plus a read-only
parse of the retained completion-review receipts in
`/home/bot/wg/.wg/completion/v3/objects` and each task's
`completion_review_activity` in `.wg/graph.jsonl`.

**Working hypothesis tested** (as supplied): FLIP began as a two-phase
fidelity check (phase I blind reconstruction, phase II revealed comparison)
and Eval was the acceptance gate; over time *both* accreted contractual
responsibilities and now overlap.

**Verdict (short):** the archaeology **confirms the direction of the
hypothesis but corrects its starting point**. FLIP did *not* start as a
pass/fail fidelity gate. It started as a **0–1 fidelity score**
(`flip_score`), separate from Eval; Eval was the threshold gate. FLIP was then
promoted to a *required pre-merge gate* (`11b4bdcb`, 2026-07-28), and the
manifest-bound valve (`6ac127a4` / `4fee4439` / `88e79dc9`) turned *both* into
mandatory pass/fail reviewers. The overlapping contractual clauses
(`REQUIREMENTS CLASSIFICATION`, `TEMPORAL EVIDENCE BOUNDARY`,
`EVIDENCE-GAP CLASSIFICATION`, `Continue strict candidate review`) were added
to the *shared reviewer prompt* and *also* to the FLIP phase-II prompt, so FLIP
spent ~2 weeks (2026-09-06 → 09-20) adjudicating acceptance/evidence questions
that Eval already owned. The fidelity-only narrowing (`9228862f`, 2026-09-20)
is the deliberate de-duplication; this survey confirms and quantifies why it
was needed and what remains.

An honest caveat where the hypothesis is *wrong*: the temporal/evidence clauses
were **not** added because two components drifted redundantly by accident. Each
has a documented incident behind it (`transient-failure-backoff` for
`TEMPORAL EVIDENCE BOUNDARY`; an empty-diff idempotent re-completion for
`EMPTY-DIFF IDEMPOTENCE`; the `completion.missing_authoritative_runtime_evidence`
contract-help path for the evidence-gap code). The overlap is therefore
**defensive compensation for real failures**, not pure duplication — which is
exactly why the narrowing moved the clauses to Eval rather than deleting them.

---

## 1. Timeline (commit-anchored)

Dates are author dates; hashes are full-history short hashes on `main`.

### 1.1 Pre-valve era: FLIP is a score, Eval is a threshold gate

| Date | Commit | What changed |
|---|---|---|
| 2026-03-03 | `f2ab3296` | **FLIP introduced** (`src/agency/prompt.rs`). Two phases: `render_flip_inference_prompt` reconstructs the prompt from output only; `render_flip_comparison_prompt` compares reconstruction to the real prompt and emits a `flip_score` = `0.4*semantic_match + 0.3*requirement_coverage + 0.2*specificity_match + 0.1*(1-hallucination_rate)`. Same commit adds "eval-can-fail": a **threshold Eval gate** that fails tasks below a quality bar. So at birth: **FLIP = measurement, Eval = gate.** |
| 2026-03-09 | `02a350f6` | FLIP extracted from the Eval task script into its own inline task (`.flip-*`-style); FLIP no longer runs *inside* `wg evaluate`. |
| 2026-03-09 | `3e75eaa4` | Eval prompt starts consuming `flip_score` and verify findings — first coupling between the two. |
| 2026-03-10 | `74998019` | `flip_score` mechanically injected into Eval's dimensions as `intent_fidelity`. Fidelity becomes a *dimension of* acceptance. |
| 2026-04-12 | `0810050e` | Decomposition detection added to Eval. |
| 2026-07-18 | `b54affaa` | "Harden evaluator artifact diff evidence" — Eval begins policing evidence. |
| 2026-07-25 | `9ca91c8d` | `fix-low-score-eval-gate` — Eval threshold gate tuned. |
| 2026-07-28 | `0dd48b92` | "Lazily mint candidate evaluation evidence". |
| 2026-07-28 | `11b4bdcb` | **`flip-first-required-gate`: FLIP becomes a required pre-merge gate.** Introduces `flip_policy = required-primary-pre-merge`, `flip_threshold`, `evaluator_threshold`. This is the pivotal accretion: FLIP stops being only a metric and refuses the merge when it fails. |

### 1.2 Manifest-bound valve era: both become mandatory reviewers

| Date | Commit | What changed |
|---|---|---|
| 2026-08-05 | `6e3186d9` | Immutable completion **manifest resolver**; `COMPLETION_MANIFEST_VERSION = 1` (never bumped since). |
| 2026-08-05 | `6ac127a4` | **Manifest-bound FLIP-then-Eval valve** (`src/completion_review.rs`), `COMPLETION_REVIEW_RECEIPT_VERSION = 1`, `ReviewerKind::{Flip,Eval}`. Eval is never invoked until FLIP passes the exact manifest/requirements binding. FLIP is now a gate *inside* the valve. |
| 2026-08-05 | `5711b9a8` | Wire immutable submit through the universal review valve (`ReviewValveStatus`). |
| 2026-08-05 | `5f9161d8` | Exact-route model adapter; introduces the `SECURITY BOUNDARY` block ("untrusted task/output data", "You have no tools…", "Missing evidence must not be guessed", "Infrastructure availability is not a semantic verdict"). |
| 2026-08-08 | `6e46d8c0` | Persist Pi accounting and expose `completion_review_activity` — the task-level ledger this survey parses. |
| 2026-08-09 | `275887dd` | `completion_review_strict` policy flag: strict = fail-closed; advisory = record + warn. |
| 2026-08-10 | `db7edd28` | Bind deterministic validation evidence: `deterministic-validation/configured/*` and `baseline/*` become **mandatory authority** in the reviewer prompt. |
| 2026-09-05 | `4fee4439` | Completion rejections and landing leases become authoritative. Hard-codes the two-phase FLIP protocol string `prompt-reconstruction-two-phase-v1` in `completion_review.rs`. |
| 2026-09-05 | `88e79dc9` | **Bind FLIP review to immutable phase executions**: `FLIP_PROTOCOL = "…-v2"`, `COMPLETION_REVIEW_RECEIPT_VERSION = 2`, `FlipProof` + create-once `flip-execution-authority`. Prompts move into `src/completion_review.rs` (where the task names them). |
| 2026-09-06 | `6fbd47c1` | **`TEMPORAL EVIDENCE BOUNDARY`** + **`Continue strict candidate review`** added to *both* the FLIP phase-II prompt and the shared reviewer prompt. Incident: `review-causal-evidence-boundary`. |
| 2026-09-14 | `bd1c4e1f` | **`REQUIREMENTS CLASSIFICATION` / `coordination_guidance`** added to both prompts; splits `deterministic-validation/optional/*` (worker observations) from `configured/baseline` mandatory authority. |
| 2026-09-15 | `3959e9a9` | **`EVIDENCE-GAP CLASSIFICATION` + reserved code `completion.missing_authoritative_runtime_evidence`** added to both prompts, plus `is_authoritative_runtime_evidence_gap` and the bounded contract-correction help path. Incident: `fix-semantic-completion-help`. |
| 2026-09-16 | `2901fc13` | **`EMPTY-DIFF IDEMPOTENCE RULE`** added to FLIP phase II. Incident: a probe rejected a legitimate idempotent re-completion 5/6 times. |
| 2026-09-16 | `c14b5272` | Same rule copied into the shared Eval prompt. |
| 2026-09-16 | `17de47fa` | Controller-computed `deliverable_presence` facts added to review material (the empty-diff rule's factual basis). |
| 2026-09-19 | `d2512ac1` | Bounded transient-failure retry on the agency review path. |
| 2026-09-20 | `9228862f` | **Fidelity-only narrowing** (`implement-flip-fidelity`): `FLIP_PROTOCOL` → `-v3`. FLIP phase II loses `REQUIREMENTS CLASSIFICATION`, `TEMPORAL EVIDENCE BOUNDARY`, `Continue strict candidate review`, `EVIDENCE-GAP CLASSIFICATION`; gains **"You are NOT the acceptance gate"** and a `flip.<fidelity-category>` code namespace. Eval's role line is recalibrated to acceptance ("reject only for a substantive, actionable gap…"). |
| 2026-09-20 | `7f7e600c` | **Recoverable review rejection resumes the same node in place** (`implement-milestone-2-of`): `SemanticRejectionClass::{EvidenceGap,Irrecoverable,Recoverable}`, `IrrecoverableSemanticClass::{GateWeakening,ScopeAmbiguity,MissingAuthority,DefectOrIncompleteImplementation}`, `Repairing` disposition + auto-resumable `Timer` wait, **no acceptance path**. |

### 1.3 Clause timeline (each clause → introducing commit)

| Clause (as it lives today) | Location today | Introduced / moved by |
|---|---|---|
| Blind reconstruction ("BLIND PROMPT RECONSTRUCTION", requirements/prompt/conversation/worker-summary denied) | `completion_review.rs:465` (`render_flip_inference_prompt`) | Two-phase blind idea: `f2ab3296`; exact valve text `4fee4439`; moved into `completion_review.rs` at `88e79dc9` |
| Revealed comparison ("FRESH INTENT REVEAL AND FIDELITY COMPARISON") | `completion_review.rs:472` (`render_flip_comparison_prompt`) | `f2ab3296`; valve text `4fee4439`; `88e79dc9` |
| `inert untrusted data` | both FLIP prompts | `4fee4439`; `88e79dc9` |
| `SECURITY BOUNDARY` (untrusted material, no tools, "missing evidence must not be guessed") | `completion_review_model.rs:597` | `5f9161d8` |
| `deterministic-validation/configured` + `baseline` = mandatory authority | `completion_review_model.rs:597` | `db7edd28` |
| `deterministic-validation/optional` ≠ gate; worker prose ≠ evidence | `completion_review_model.rs:597` | `bd1c4e1f` |
| `REQUIREMENTS CLASSIFICATION` / `coordination_guidance` / `classification_ambiguities` | shared prompt `:597`; **removed from FLIP** | `bd1c4e1f`; label `REQUIREMENTS CLASSIFICATION:` `9228862f`; removed from FLIP phase II `9228862f` |
| `TEMPORAL EVIDENCE BOUNDARY` | shared prompt `:597`; **removed from FLIP** | `6fbd47c1`; removed from FLIP phase II `9228862f` |
| `Continue strict candidate review` | shared prompt `:597`; **removed from FLIP** | `6fbd47c1`; removed from FLIP phase II `9228862f` |
| `EVIDENCE-GAP CLASSIFICATION` + `completion.missing_authoritative_runtime_evidence` | shared prompt `:597`; **removed from FLIP** | `3959e9a9`; removed from FLIP phase II `9228862f` |
| `EMPTY-DIFF IDEMPOTENCE` | both FLIP phase II and shared prompt | FLIP `2901fc13`; shared `c14b5272`; `deliverable_presence` basis `17de47fa` |
| `You are NOT the acceptance gate` | FLIP phase II `:472` | `9228862f` |
| Eval role "Perform the acceptance evaluation" (impressionistic calibration) | `completion_review_model.rs:574` | `9228862f` (replaces "independent correctness evaluation") |

### 1.4 Protocol / receipt / policy version history

| Artifact | Values | Bumps and reasons |
|---|---|---|
| `FLIP_PROTOCOL` | `prompt-reconstruction-two-phase-v1` → `-v2` → `-v3` | v1 `4fee4439` (two-phase FLIP in the valve); v2 `88e79dc9` (bind to immutable phase executions / `FlipProof` + execution authority — a verification-semantics change); v3 `9228862f` (phase II narrowed to fidelity-only — a meaning change to the same call) |
| `COMPLETION_REVIEW_RECEIPT_VERSION` | `1` → `2` | 1 `6ac127a4` (valve receipts); 2 `88e79dc9` (adds the attempt/fence/candidate `binding` + `flip_proof`) |
| `COMPLETION_MANIFEST_VERSION` | `1` (never bumped) | `6e3186d9`; the manifest stayed stable while review semantics evolved around it |
| `ReviewValveStatus` | `Accepted / FlipRejected / EvalRejected / ReviewUnavailable / IncompleteEvidence` | `6ac127a4`; wired `5711b9a8`; semantic/infra split is load-bearing (unavailable/incomplete never become a semantic rejection) |
| Review policy | `completion_review_strict` bool | `275887dd`. Matrix in `completion_submit.rs:251-320`: strict → FlipRejected/EvalRejected/ReviewUnavailable fail closed (park/NeedsAttention); advisory → recorded + warn, deterministic publication proceeds. |
| FLIP gate config | `flip_policy`, `flip_threshold`, `evaluator_threshold`, `global_flip_enabled` | `11b4bdcb` (`required-primary-pre-merge`), tuned `9ca91c8d`; the active FLIP-gate path was superseded by the manifest valve, and `fd32f89f` pruned `flip_score`/`evaluator_threshold` from the active eval path; the threshold machinery survives only in the dormant `eval_lifecycle`/`evaluation` compatibility code |
| Recoverability policy | `SemanticRejectionClass` + `IrrecoverableSemanticClass` + `deterministic_repair_budget` | `7f7e600c`; reuses the existing per-episode repair budget, **no second budget**, **no acceptance path** |

---

## 2. Overlap map

Ownership today, per responsibility. "Controller" = the deterministic
completion controller / manifest / validation subsystem (not a model
reviewer). **Duplicate owners are flagged with ⚠.**

| Responsibility | FLIP owns? | Eval owns? | Controller / deterministic owns? | Repair / recovery owns? | Duplicate? |
|---|---|---|---|---|---|
| Fidelity judgment (does the candidate match revealed intent) | ✅ **sole owner** (`render_flip_comparison_prompt`) | historically used `flip_score` as `intent_fidelity`; no longer primary | — | — | no (post-`9228862f`) |
| Blind reconstruction (phase I) | ✅ sole | — | — | — | no |
| Requirement coverage | historically yes (FLIP phase II "validation coverage, omissions"; Eval role line "check every requirement") | ✅ **primary** | projection built by controller (`review_requirements_projection`) | — | ⚠ FLIP phase II *still says* "cross-component assumptions / counterfactual" which can shade into coverage |
| Acceptance vs coordination classification | ❌ removed `9228862f` | ✅ owner (`REQUIREMENTS CLASSIFICATION`) | projection + `classification_ambiguities` computed deterministically (`completion_task.rs:104`) | contract-correction help | no (post-narrowing) — but the deterministic projection and Eval both act on it |
| Evidence completeness / authority | ❌ removed `9228862f` | ✅ owner (Eval prompt; `configured/baseline` mandatory) | ✅ **authority**: `deterministic-validation/*` capture+registration (`db7edd28`, `bd1c4e1f`) | — | ⚠ Eval reasons about authority the controller already decides; FLIP used to also |
| Temporal / future-fact boundary | ❌ removed `9228862f` | ✅ owner (`TEMPORAL EVIDENCE BOUNDARY`) | ✅ controller verifies postconditions after the response | — | ⚠ clause asserted in two prompts 2026-09-06→09-20 |
| Evidence-gap classification + reserved code | ❌ removed `9228862f` | ✅ owner (`EVIDENCE-GAP CLASSIFICATION`) | ✅ `is_authoritative_runtime_evidence_gap` (typed, exact-command, unmixed) | ✅ bounded contract-correction / help path | ⚠ clause asserted in two prompts 2026-09-15→09-20 |
| Empty-diff / idempotence | ✅ fidelity framing retained | ✅ acceptance framing retained | ✅ `deliverable_presence` facts | — | ⚠ deliberately duplicated (`2901fc13` + `c14b5272`) — see §4 |
| Contract-correction requests | ❌ explicitly disowned | via Eval receipt findings; the *request* action is worker/operator | typed `wg fail --intent request-contract-correction` | ✅ owner | no |
| Repair prompting / resumption | ❌ | ❌ | — | ✅ `CompletionRepairState` + `Timer` resume (`7f7e600c`) | no |
| Fail-closed classification of rejection | ❌ | ❌ | ✅ `classify_semantic_rejection` (`completion_validation.rs:689`) | ✅ owner | no |
| Gate/requirements integrity, provenance, fences | ❌ | ❌ | ✅ manifest + binding + `flip-execution-authority` | ✅ never repaired as "repair" | no |

**Duplicates that remain after the narrowing:** (a) the *empty-diff* rule exists
in both prompts by design (different framing, same facts — medium risk of
divergence if the underlying facts change); (b) evidence *authority* is
deterministically owned by the controller but *reasoned about* by Eval and
historically by FLIP; (c) "requirement coverage" is Eval-primary but FLIP's
"counterfactual / cross-component" wording still permits a coverage-flavoured
finding under a `flip.*` code.

---

## 3. Quantified rejection history

### 3.1 Commands used (read-only; no prompt or production change)

```bash
# clause introduction commits
for s in "TEMPORAL EVIDENCE BOUNDARY" "REQUIREMENTS CLASSIFICATION" \
         "EVIDENCE-GAP CLASSIFICATION" "EMPTY-DIFF IDEMPOTENCE" \
         "Continue strict candidate review" "You are NOT the acceptance gate" \
         "coordination_guidance" "missing_authoritative_runtime_evidence" \
         "inert untrusted data" "BLIND PROMPT RECONSTRUCTION"; do
  echo "=== $s ==="; git log --oneline -S "$s" -- src/ | tail -5
done

# protocol / receipt / policy history
git log --oneline --reverse -S "prompt-reconstruction-two-phase" -- src/
git log --oneline --reverse -S "COMPLETION_REVIEW_RECEIPT_VERSION" -- src/
git log --oneline --reverse -S "COMPLETION_MANIFEST_VERSION: u32" -- src/completion_manifest.rs
git log --oneline --reverse -S "completion_review_strict" -- src/
git log --oneline --reverse -S "flip_threshold" -- src/

# receipt corpus (415 real receipts, 214 v1 + 201 v2):
#   object is a receipt iff it has reviewer_kind + manifest_digest + findings_digest
#   findings are resolved through findings_digest -> CAS object
# task-level ledger: latest task record per id in .wg/graph.jsonl,
#   completion_review_activity[] (102 tasks, 241 activities)
```

The full parse script is the Python heredoc in this task's session log; it
resolves every `findings_digest`/`flip_proof` reference through the CAS and
counts distinct review receipts (not the `flip_proof` phase-execution records
or `latent_hypothesis`/prompt objects, which are also in the same CAS).

### 3.2 Receipt counts by version × reviewer × verdict (CAS: 415 receipts)

| Version | Reviewer | pass | reject | unavailable | incomplete_evidence | total |
|---|---|---|---|---|---|---|
| v1 (`6ac127a4`…`4fee4439`) | flip | 31 | **141** | 11 | 0 | 183 |
| v1 | eval | 27 | 4 | 0 | 0 | 31 |
| v2 (`88e79dc9`…today) | flip | 55 | **86** | 4 | 1 | 146 |
| v2 | eval | 37 | **18** | 0 | 0 | 55 |

Observations:
- v1 was **FLIP-dominated and FLIP-rejection-heavy** (141/183 = 77% of FLIP
  receipts rejected) while Eval almost never rejected (4/31). FLIP was doing
  the rejecting during the gate era.
- v2 shifts rejections toward Eval (18/55 = 33% of Eval receipts reject) and
  FLIP's rejection rate falls (86/146 = 59%). This is consistent with the
  narrowing moving acceptance/evidence rejections out of FLIP and into Eval.
- `failure_class` is cleanly separated the whole time: `semantic_rejection`
  (133), `reviewer_unavailable` (13), `incomplete_evidence` (1) — infrastructure
  and evidence-resolver failures are *never* mislabelled as semantic
  rejection. That separation is a load-bearing invariant.

### 3.3 Rejections by class, and recovery

Task-level ledger (`.wg/graph.jsonl`, latest record per task; 102 tasks,
241 activities, 2026-08-09 → 2026-09-20):

- **133 semantic rejections** (all `semantic_rejection`); **67** were later
  followed by a `pass` on the same task, **66** were not (within the retained
  ledger).
- **71 tasks** saw at least one rejection; **27** of those eventually reached a
  pass, **44** did not (in the retained ledger; some end in `NeedsAttention`).
- By reviewer kind over time (week buckets, rejections only):

  | Week | FLIP rejects | Eval rejects |
  |---|---|---|
  | 2026-W31 | 6 | 0 |
  | 2026-W32 | 4 | 0 |
  | 2026-W33 | 7 | 0 |
  | 2026-W34 | 1 | 0 |
  | 2026-W35 | 21 | 1 |
  | 2026-W36 | 42 | 6 |
  | 2026-W37 | 34 | 11 |

- Finding-code families across all 415 rejection receipts: `validation.*`
  (115), `flip.*` (113), `safety.*` (60), `coverage.*` (42), `completion.*`
  (36), `bounded.*` (34), `evidence.*` (27), `requirement(s).*` (17).

### 3.4 The reserved evidence-gap class specifically

`completion.missing_authoritative_runtime_evidence`:

- **36 finding occurrences** across all CAS rejection receipts; the containing
  receipts are **10 FLIP** and **7 Eval** (v2 only — the code did not exist
  before `3959e9a9`, 2026-09-15).
- In the task ledger, **17 rejections** whose findings include the reserved
  code; only **1** was later followed by a pass. The rest did not recover
  within the retained ledger.
- This is the strongest single cost signal in the corpus: the clause that the
  fidelity-only narrowing removed from FLIP is also the clause with the worst
  recovery rate, and it was being emitted by *both* reviewers.

### 3.5 Irrecoverable classes

The `IrrecoverableSemanticClass` codes (`gate-weakening`, `scope ambiguity`,
`missing authority`, `defect/incomplete implementation`) do **not** appear as
`eval.*`/`flip.*` finding codes in the 415-receipt corpus. In practice reviewer
findings are phrased in the `validation.*`/`flip.*`/`safety.*` families and the
recoverability split matches those bounded findings by substring
(`completion_validation.rs:611-698`). That is worth stating plainly: the
recoverability taxonomy is real, but its "missing authority" and
"gate-weakening" arms are not currently exercised by a stable finding-code
vocabulary.

---

## 4. Load-bearing vs compensation

### Load-bearing safety invariants (must stay)

1. **Semantic vs infrastructure separation.** `SemanticReview` may only be
   `pass|reject`; resolver failures are `IncompleteEvidence`, provider
   failures are `ReviewUnavailable`, and neither is convertible to a semantic
   rejection (`completion_review.rs` module doc; `ReviewValveStatus`). Quantified
   above (14 non-semantic outcomes, all correctly labelled).
2. **Immutable binding / provenance / fences.** v2 receipts carry
   `binding{task_id, generation, attempt_id, attempt_fence, candidate_sequence}`;
   `flip-execution-authority` is create-once and re-derived from the full record;
   `is_exact_pass` checks manifest + requirements + reviewer kind. `wg done`
   refuses stale/forged/wrong-binding receipts. This is the anti-replay core.
3. **Manifest + requirements digest pinning.** The reviewer sees the exact
   digest-bound bytes; FLIP phase I cannot see intent at all (type-enforced by
   `FlipBlindInput`'s lack of requirement fields + `deny_unknown_fields`).
4. **`completion.missing_authoritative_runtime_evidence` is a typed, narrow
   seam.** `is_authoritative_runtime_evidence_gap` demands *every* finding carry
   the reserved code, one identical exact command in `evidence`, and no
   mixed defect. This is the invariant that prevents a contract-correction
   request from hiding a code defect (`3959e9a9` test
   `authoritative_runtime_gap_requires_an_exact_unmixed_marker`).
5. **Fail-closed recoverability.** Irrecoverable classes and exhausted budget go
   to `NeedsAttention`; a recoverable round has **no acceptance path** and must
   pass the unchanged reviewer (`7f7e600c`; `docs/completion-repair.md`).
6. **Temporal honesty is load-bearing for correctness of the *call*** — a
   reviewer must not reject for facts that cannot exist before the call.
   But it does not follow that FLIP must carry the clause (§4.5).

### Compensation for earlier design gaps (now redundant or relocatable)

1. **FLIP carrying acceptance/evidence clauses** (`REQUIREMENTS
   CLASSIFICATION`, `TEMPORAL EVIDENCE BOUNDARY`, `Continue strict candidate
   review`, `EVIDENCE-GAP CLASSIFICATION`) — these were copied into FLIP
   (`6fbd47c1`, `bd1c4e1f`, `3959e9a9`) while FLIP was a general reviewer. The
   `transient-failure-backoff` incident (documented in
   `docs/design-review-recovery.md` §0) is the proof: an *evidence-policing*
   verdict was produced by the FLIP phase-II prompt, parked the task in
   `LandingPending`, and needed a human ~22h later. The narrowing (`9228862f`)
   removed all four from FLIP. **They are redundant in FLIP, not in the system.**
2. **`flip.threshold` / `flip_policy` pre-merge gate** — superseded by the
   manifest-valve `is_exact_pass` gate; `fd32f89f` pruned `flip_score` /
   `evaluator_threshold` from the active eval path and the threshold machinery
   now survives only in dormant `eval_lifecycle` / `evaluation` compatibility
   code. This is legacy, not a live responsibility.
3. **Empty-diff duplication in both prompts** — added twice (`2901fc13` then
   `c14b5272`) because the same blind-to-base failure mode appeared in two
   independent reviewers. It is *compensation for a missing shared
   controller-side rule*: the controller already computes
   `deliverable_presence`, so the clean fix is for the controller to settle
   idempotence deterministically and have the prompts stop adjudicating it.
   However, until that lands, the duplication is mildly load-bearing (removing
   either copy re-introduces a known false-reject class).
4. **Evidence *authority* living in reviewer prose** — the controller already
   owns capture + registration + `configured/baseline` authority. Eval
   reasoning about it is the fallback for the fact that `wg done` does not
   automatically treat a missing runnable check as a deterministic failure.
   The narrowing keeps it in Eval; the *clean* end state is §5.

### Where the hypothesis is contradicted / nuanced

- FLIP did not start as a gate; it started as a score. The overlap is partly
  *promotion* (FLIP promoted into the gate role at `11b4bdcb`), not only drift.
- The duplicated clauses were defensive responses to real incidents, each with
  a documented failure. The survey's value is the map: which copy is the
  safety copy (Eval's) and which is the redundant copy (FLIP's).

---

## 5. Recommended minimal contract

This recommendation is consistent with the landed narrowing (`9228862f`) and
adds nothing that changes behaviour today. It is a target contract; adopting it
is a separate implementation task.

### 5.1 What FLIP must assert — **fidelity only**

- Phase I: blind reconstruction (unchanged; type-enforced).
- Phase II: "is the exact candidate faithful to the revealed intent?" with the
  `flip.<fidelity-category>` namespace.
- The empty-diff rule **as a fidelity judgment only** (an empty diff is not
  *unfaithful* when the deliverable is already present).
- Inert-untrusted-data boundary and the output schema.
- Nothing about acceptance, evidence completeness/authority, temporal
  postconditions, or contract corrections.

### 5.2 What Eval must assert — **acceptance with impressionistic calibration**

- Acceptance against `review_requirements.acceptance` (coordination guidance
  visible, not gating).
- Reject only for a substantive, actionable gap a competent operator would
  agree blocks acceptance; not for evidence ceremony or pre-call-impossible
  facts.
- The reserved evidence-gap code only for the one exact, already-existing
  runnable check lacking host-captured authority; never for a defect/omission.
- All four shared clauses (`REQUIREMENTS CLASSIFICATION`,
  `TEMPORAL EVIDENCE BOUNDARY`, `EMPTY-DIFF IDEMPOTENCE`, `Continue strict
  candidate review`, `EVIDENCE-GAP CLASSIFICATION`) live here.

### 5.3 What belongs to neither reviewer

- **Evidence authority → deterministic validation.** The controller already
  captures and registers `deterministic-validation/configured/*` and
  `baseline/*` and computes `deliverable_presence`. A missing required check
  should be resolved by `wg done`'s deterministic contract (or its structured
  `Repairing`/contract-correction path), not by asking a model to notice it.
  Eval keeps the reserved code as a *typed relay* for the legacy path; the
  long-term owner is `completion_validation.rs`.
- **Repair / resumption → the repair path** (`CompletionRepairState` +
  `Timer` + `classify_semantic_rejection`). Reviewers must never prompt for
  resumption or write lifecycle state; `7f7e600c` already enforces this and
  the `Repairing` row has no acceptance path.
- **Requirement projection / coordination classification → the controller**
  (`review_requirements_projection`). Reviewers consume the projection; they do
  not build it.

### 5.4 Specific clauses that can be deleted (and their justification)

Post-`9228862f` these are already gone from FLIP. The remaining deletable
targets are:

1. **Delete from FLIP phase II: the `EMPTY-DIFF IDEMPOTENCE RULE`** once the
   controller settles idempotence deterministically from `deliverable_presence`
   and passes a boolean "deliverable already present" assertion to the
   reviewer. Until then, keep it in **both** prompts (it is covering a real
   false-reject class).
2. **Delete from the shared prompt: `TEMPORAL EVIDENCE BOUNDARY`** once the
   controller filters causally-future facts out of the material before
   rendering (the reviewer cannot demand what it cannot see). Until then it is
   the last line of defence; keep it in Eval only (already done).
3. **Delete from the shared prompt: `Continue strict candidate review`** — it is
   the positive half of the temporal clause (the controller can state
   "actionable pre-existing facts are:" explicitly rather than asking the model
   to reason about causality).
4. **Delete from FLIP phase II: `counterfactual behavior and cross-component
   assumptions`** wording — it is the last coverage-flavoured authority in
   FLIP and can yield `flip.*` findings that are really acceptance findings.
   Replace with explicit fidelity categories (intent mismatch / missing
   required deliverable / contradicts revealed constraint).
5. **Do not delete** (load-bearing): the semantic/infra separation, immutable
   binding/fences, digest pinning, the typed evidence-gap marker and its
   unmixed/exact-command checks, fail-closed recoverability, and the
   no-acceptance-path rule. These are invariants, not ceremony.

### 5.5 Net contract in one line

> FLIP answers "did the candidate do what was asked?"; Eval answers "is that
> good enough to accept?"; the controller answers "is the evidence
> authoritative?"; the repair path answers "what happens after a rejection?".
> No question is answered by two components.

---

## Appendix A — data provenance caveats

- The CAS `.wg/completion/v3/objects` is content-addressed and shared across
  attempts/tasks; it contains 415 review receipts plus `flip_proof` phase
  executions, prompts, latent hypotheses, and deterministic evidence objects.
  Counts here filter to objects with `reviewer_kind` + `manifest_digest` +
  `findings_digest`.
- The task ledger parses the latest `kind:"task"` record per id in
  `.wg/graph.jsonl`; `completion_review_activity` only exists for tasks created
  after `6e46d8c0`, so the August ledger is thinner than the CAS.
- "Later followed by a pass" means a `pass` activity with a strictly later
  `created_at` on the same task within the retained ledger; it under-counts
  recoveries whose continuation happened outside the retained window.
- No prompt, config, or production file was modified by this survey.
