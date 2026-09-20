# Review-gate calibration batch — verdict profile

**Task (superseded by this document):** `calibration-batch-run`
**Author:** operator (written directly from the receipt CAS, 2026-09-21)
**Gate under test:** FLIP `prompt-reconstruction-two-phase-v3` (fidelity-only narrowing, `9228862f`)
+ Eval `render_review_prompt` (impressionistic acceptance, same commit).
**Routes:** both reviewers on `pi:lunaroute/deepseek-4.1-flash-background-background`.

> This document is the deliverable the task `calibration-batch-run` was written to produce.
> Its raw inputs are retained immutable receipts in `.wg/completion/v3/objects` plus each
> task's `completion_review_activity`; every number below is recomputable with the commands
> in §5. The task itself is retired (§6) — the knowledge is kept, the obligation is not.

---

## 0. Verdict

**The narrowed gate is calibrated in both directions, and the recoverable-rejection loop is
proven live.**

| Question | Answer | Evidence |
|---|---|---|
| Does it over-reject legitimate work? | **No.** 19/19 legitimate candidates accepted | §1 (10 receipts) + §2 (9 strict-instance probes) |
| Does it reject genuine defects? | **Yes.** A deliberately defective candidate was rejected with substantive findings | §3 |
| Does a rejection resume the node in place? | **Yes.** Repairing → bounded checkpoint → Timer auto-resume → corrected → landed, **no operator action** | §3 |
| Is the veto enforced? | **No, deliberately.** The one real rejection on the shared graph was published over (advisory policy) | §4 |

The last row is not a defect in the gate; it is the *policy* setting. It matches the shape the
external study recommends: **keep the knowledge, demobilize the veto** — enforcement is a
separate, explicit decision (`wg config set agency.completion_review_strict true` enables it).

---

## 1. Calibration probes (shared graph, advisory policy)

Four small, genuinely useful chores authored by `calibration-batch-run`
(`calib-doc-list-audit-rows`, `calib-doc-recovery-status`,
`calib-doc-repair-outcome`, `calib-test-deliverable-tokens`).

| task | FLIP | Eval |
|---|---|---|
| `calib-doc-list-audit-rows` (2 attempts — attempt-loss retry) | 2 pass | 2 pass |
| `calib-doc-recovery-status` | 1 pass | 1 pass |
| `calib-doc-repair-outcome` | 1 pass | 1 pass |
| `calib-test-deliverable-tokens` | 1 pass | 1 pass |
| **total** | **5 pass / 0 reject** | **5 pass / 0 reject** |

**10 receipts, 10 passes, zero findings, zero recovery rounds.** All four tasks landed.
Both reviewers ran the `v3` FLIP prompt / calibrated Eval on the background-queue route.

## 2. Corroborating strict-instance probe series

Independently of this batch, `prove-the-recoverable` published **nine** escalating-strictness
probes under an isolated strict-policy instance (`docs/reports/recovery-loop-live-proof.md`
§3.2–3.3): controlled-vocabulary bans, tight per-section word windows, a low-reasoning worker,
a required-but-uncaptured runnable check, and a real Rust change naming `cargo test --lib` in
its `## Validation`. **9/9 passed with zero findings.**

Combined: **19/19 legitimate candidates accepted** by the narrowed gate.

## 3. Negative control (isolated strict instance) — the gate bites, and the loop fires

`negative-control-for` (`docs/reports/negative-control-gate.md`): a Land task requiring four
exact `##` sections whose candidate omitted `## Failure Modes` entirely while passing its
deterministic check (`test -s report.md`).

- **FLIP rejected** with two substantive findings — receipt
  `b3:e52cc02d4cc53d8f67cdaf284b9286885a61abd8a117403fb689f7d46d8e6329`.
- `completion_repair.disposition = repairing`, **`recovery_round = 1`**, `opportunities_used = 1/2`,
  on the **same episode** (`generation=0`, `attempt-0-1`, `fence=1`).
- A **bounded corrective checkpoint** was written for the worker.
- **Auto-resumed by the Timer wait with no operator action**; the dispatcher reserved
  `attempt-0-2` / `fence=2` on the same generation.
- **Ending: corrected and passed legitimately** — the missing section was added, resubmitted,
  fresh FLIP + Eval pass, landed.

No gate weakened; no defective candidate accepted.

## 4. The shared graph since the narrowed gate deployed (22:12Z)

Cut by receipt timestamp (the FLIP protocol string alone cannot separate Eval generations):

```
flip  pass   4
flip  reject 0
eval  pass   3
eval  reject 1
```

The single rejection is a **true positive**, and it is the same class that has recurred all
week:

> `completion.missing_authoritative_runtime_evidence` — *"The acceptance projection requires
> `cargo test --lib` green and fmt/clippy clean, but the only host-captured validation envelope
> is `deterministic-validation/baseline/v1` running `git diff --check`. Worker-summary prose
> asserting the tests/lints passed is not validation evidence."*
> (task `investigate-the-attempt`, digest `b3:31f33763edf982035a6ff41d7e0ded9a7a40dcf431b3b40483251f513047b3b5`)

The reviewer was right: the task's own stated validation was never captured. Under **advisory**
policy the candidate published anyway (and landed). Under **strict** policy the Milestone-2
recovery loop would have written a corrective checkpoint and required the evidence — the
mechanism proven live in §3.

## 5. Historical contrast (why the narrowing was needed)

From `docs/research/flip-eval-evolution.md` §3.2 (415 receipts): in the v1 era FLIP rejected
**141/183 (77%)** while Eval rejected **4/31 (13%)** — the fidelity *metric*, promoted to a
gate, was doing the rejecting while the acceptance gate barely did. v2 shifted rejections
toward Eval (18/55 = 33%). The narrowed v3 prompt has produced **zero FLIP rejections** in
the observed window.

Reproduction commands (read-only):

```bash
# receipts by reviewer/verdict (all time)
python3 - <<'PY'
import json,glob,collections
c=collections.Counter()
for f in glob.glob('.wg/completion/v3/objects/*'):
    try: d=json.load(open(f))
    except Exception: continue
    if isinstance(d,dict) and d.get('reviewer_kind') in ('flip','eval'):
        c[(d['reviewer_kind'], d.get('verdict'))]+=1
print(c)
PY
# the same, cut at the narrowed-gate deploy
# filter: str(d.get('created_at',''))[:16] >= '2026-09-20T22:12'
```

## 6. Honest caveats

- **Small n, easy work.** The probes are documentation/test chores; 19/19 says the gate does
  not nitpick cheap work — it does not bound behaviour on large, ambiguous deliverables.
- **One side of the loop is unproven on the shared graph.** The recovery loop was demonstrated
  in an isolated strict instance, never on the shared graph, because the shared graph is
  advisory. Enabling strict is what would make §3's arc reproducible on real work.
- **The isolated instance shares auth but not graph/keystore**; it is a real dispatch, not the
  production graph.
- **Calibration receipts predate the Milestone-2 binary** (they ran under the narrowed prompt
  with the pre-recovery daemon), so they measure acceptance behaviour only, not recovery.
- **The observed window is short** (≈1 hour of shared-graph receipts) and partly overlapped an
  attempt-loss episode; re-dispatch burden from that bug is not a governance signal.

## 7. Recommendation

1. **Add no gate.** The gate is calibrated; the remaining problems this week were bugs
   (attempt-loss, plugin-cache staleness, the npm publish step), not missing policies.
2. **Choose enforcement explicitly.** Either leave advisory (knowledge recorded, delivery
   unblocked — the state matching the external study's recovery prescription) or set
   `agency.completion_review_strict = true` to make §3's corrective arc the default on real
   work. Either is defensible; the point is that it is a human decision, not an automatic one.
3. **Retire, don't tune, what this document replaces.** `calibration-batch-run` is retired by
   this report; the synthesis it fed is satisfied by the report set
   (`first-npm-release.md`, `recovery-loop-live-proof.md`, `negative-control-gate.md`,
   `attempt-loss-investigation.md`, `flip-eval-evolution.md`, and this file).
