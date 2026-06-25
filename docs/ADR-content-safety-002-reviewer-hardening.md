# ADR-CS2 (WG-Review): Reviewer Hardening — Dual-LLM No-Scope · Spotlight · Diverse-Reviewer Quorum · Structured Verdict

**Status:** Proposed
**Date:** 2026-06-25
**Decision:** The Pass-2 content reviewer is a **weak-tier `.review-*` agency one-shot with no privileged tools — no graph-write, no network, no exfil** (the **dual-LLM bound**: a successful injection of the reviewer yields a wrong *verdict*, never a wrong *action*). Its input is **spotlighted / delimited** and **normalized-before-scan**; its output is a **structured, enum-only verdict** whose `reason` is a **bounded category code, never free-form prose that echoes attacker-controlled text** (closes the RA-11 second-order channel). The **high band (Unknown / high-sensitivity) escalates to a diverse-reviewer quorum** — N independent prompts/models, **strictest-wins** — **and to a stronger model** (don't run the most-injectable pass on the weakest model). **`accept` is never certification**: downstream consumption stays **skeptical even on `accept`** (MUST-1), consuming content spotlighted-as-data, never as trusted instructions.

> **This is the reviewer-hardening WG-Review ADR.** It is one of three (with
> **ADR-CS1**, the gate / pipeline / dial, and **ADR-CS3**, verdict / audit / revoke)
> formalizing the content-safety decision memo
> (`docs/content-safety-study/04-decision-memo-and-roadmap.md` §3 DP7, §6 ADR-CS2 stub,
> §8 items 4–5). ADR-CS1 fixes *where* and *how deep* the gate runs; this ADR fixes the
> **Pass-2 reviewer** — the single most-injectable component in the system — so that
> when it is fooled (and doc 03 proves it will be), the miss is bounded to a contained,
> logged, reversible *verdict*, not a privileged *action*.
>
> It composes with WG-Fed / WG-Exec and invents no parallel trust system: the reviewer
> runs on the existing weak-tier agency one-shot mechanism (`resolve_agency_dispatch` /
> `Config::weak_tier_spec()`), the same path `.evaluate-*` / `.flip-*` already use. The
> decision was *made* in the memo (RA-1 is **fatal-as-prevention, survivable only as
> containment**); this ADR records it and resolves the stub's open questions. It does
> not re-litigate the choice.

---

## Context

The content-semantics detector is **itself an LLM consuming attacker-controlled text to
decide whether attacker-controlled text is an attack** — the recursion doc 03 §0.1
names the structural vulnerability. The reviewer is the **most-injectable component in
the entire system** (RA-1), and the design deliberately puts it on the **weakest
(weak-tier) model** for cost (RA-1e). A reviewer that could be flipped *and* could act
would convert an injection into arbitrary privileged behaviour; a reviewer that can be
flipped but **cannot act** converts the same injection into a single wrong verdict that
the audit/revoke leg (ADR-CS3) can catch and reverse.

Doc 03 §5 scores the gate's two legs: **detection scores 2–3, containment/revoke scores
4.** RA-1 is one of the three **Fatal-as-prevention** findings — it cannot be
*prevented*, only *contained*. The memo §3 DP7 therefore asks this ADR to (a) bound
RA-1 with controls that separate *prevention* (raise the attacker's cost) from
*containment* (the actual guarantee) and weight toward the latter, and (b) promote two
under-owned design notes to **MUSTs**: MUST-1 (skeptical-on-accept, the RA-1 downstream)
and MUST-3 (the structured/spotlighted verdict `reason`, the RA-11 second-order
channel). Both are decided here. (MUST-2, digest-pinned consumption, lives in ADR-CS3.)

The reviewer runs on landed primitives, not a new system: the weak-tier agency one-shot
mechanism (`resolve_agency_dispatch`, `src/service/llm.rs:193`;
`Config::weak_tier_spec()`, `src/config.rs:2865`), with the reviewer added as a new
`DispatchRole` variant (`src/config.rs:1278`) alongside `Evaluator` / `Assigner` — so a
`.review-*` task is dispatched, accounted, and recorded under its resolved handler
exactly like the other agency one-shots. The reviewer's code lands in
`src/review/pass2_review.rs` (ADR-CS1, Review-Wave B).

---

## Decision

### D1 — The reviewer is a no-privilege weak-tier one-shot (the dual-LLM bound)

The Pass-2 reviewer runs as a **weak-tier `.review-*` agency one-shot with no
privileged scope**: **no graph-write, no network, no tool access, no exfil**. Its only
granted capability is `act-as-reviewer` — read the (spotlighted) candidate content,
emit a structured verdict (D3). This is the **dual-LLM pattern made structural**: the
component that eats the attacker's text is the one that **cannot act on the world**.

**The bound, stated as the guarantee:** a successful injection of the reviewer yields a
**wrong verdict, never a wrong action.** A field-scan of the reviewer's granted scope
must find **only** `act-as-reviewer` — this is checkable, and the content-safety spark
asserts it (memo §4.2 step 4). The wrong verdict it can produce (a false `accept`) is
**logged, attributed, and reversible** (ADR-CS3) and is further bounded by MUST-1 (D5)
so the contained payload cannot drive a privileged tool downstream.

**Why this is the load-bearing control.** RA-1 cannot be prevented — a sufficiently
novel framing will flip a cheap LLM. The design's honesty (doc 01 §0) is to *own* that
and bound its blast radius: the reviewer's lack of scope is what makes "the most
injectable component" survivable. Prevention controls (D2, D4) raise the attacker's
cost; this containment control is the actual guarantee.

### D2 — Spotlighted, delimited, normalized input

Candidate content is presented to the reviewer **spotlighted and delimited** as
data-to-be-judged, never as instructions to the reviewer, and is **normalized before it
is scanned or shown** (NFKC-fold, strip zero-width / tag characters, decode common
encodings — shared with ADR-CS1 Pass 1, RA-2). Delimiters are reviewer-generated and
unforgeable by the content: a forged `---END UNTRUSTED---` marker in the payload (the
RA-1b boundary-escape) **does not** end the untrusted region. This is a prevention
control — it raises the cost of a reviewer-injection; it does not eliminate it (D1 is
why the residual is survivable).

### D3 — Structured, enum-only verdict; the `reason` is a bounded category code (MUST-3)

The reviewer's output is a **structured, enum-only verdict** — one of
`accept / quarantine / reject` (the uniform vocabulary, ADR-CS3) plus a `confidence`
and a `reason` that is a **bounded category code from a fixed schema, never free-form
prose**. The reviewer **cannot** emit attacker-controlled text into the verdict record.

**MUST-3 (RA-11) — the verdict `reason` is structured / spotlighted, never rendered
verbatim.** Free-form reviewer prose that echoes attacker-controlled text is a
**second-order injection channel**: it would re-inject the *next* reader of the verdict
(a human in `wg show`, or a meta-agent consuming verdicts). Therefore:

- the `reason` is a **bounded category code**, not prose;
- `wg show` renders it **spotlighted as data-with-provenance**, never as instructions;
- **any meta-agent that consumes verdicts routes them back through the gate as IC1
  inbound content** — a verdict is **not trusted just because the gate emitted it**.

The content-safety spark asserts that rendering a verdict in `wg show` does not
re-inject a reading agent (memo §4.2 step 4, the RA-11 assertion).

### D4 — Diverse-reviewer quorum + model-strength escalation on the high band

On the **high band** (Unknown author, or high-sensitivity / code / opaque content) the
single weak-tier reviewer is **not** sufficient. Two escalations, both prevention
controls keyed off `review.depth` (ADR-CS1 D3):

1. **Diverse-reviewer quorum.** N **independent** reviewers (different prompts and/or
   different models), **strictest-wins** — the pipeline verdict is the strictest any
   reviewer reached (monotonic, ADR-CS3 D1). One tuned payload that flips one reviewer
   does not flip an independent second; **at least one still flags** the item.
2. **Model-strength escalation.** The high band runs a **stronger model**, not the
   weakest one — *don't run the most-injectable pass on the weakest model* (RA-1e).
   `review.depth` selects both the reviewer **count** and the model **strength**.

Cost is N× weak-tier spend on the **small high-risk slice** only — cheap, because it is
the *fast* tier and the light path (the common case) still runs a single cheap pass.

### D5 — `accept` is never certification; downstream stays skeptical even on accept (MUST-1)

**MUST-1 (RA-1) — downstream consumption stays skeptical even on `accept`.** An
`accept` verdict means **"no detector fired," nothing more.** It is **not** a
proof-of-safety, and it does **not** license the consuming agent to drop its guard:
accepted content is consumed **spotlighted-as-data, never as trusted instructions**
(ADR-CS1 D4). This is the structural reason a *contained* reviewer-injection (D1)
cannot escalate into a privileged action: even if a false `accept` slips through, the
downstream consumer treats the payload as data, so the contained payload **cannot drive
a privileged tool**.

Treating `accept` as certification is the precise liability doc 01 §0 reason 4 warns of:
a gate that *licenses* unskeptical consumption removes the skepticism that is the real
last line of defense. Therefore `accept` is, and is documented as, a **negative
result** ("no detector fired"), never a positive safety guarantee — the same posture
ADR-CS3's residual-risk boundary formalizes.

---

## Status

**Proposed.** This ADR records the decision exactly as fixed in the content-safety
decision memo (§3 DP7, §6 ADR-CS2 stub) and resolves the stub's open questions below.
**Erik ratifies it to Accepted** — the human gate is deliberately not set here. No
review code lands until ADR-CS1/CS2/CS3 are Accepted; the reviewer's spark-level form
(no-scope one-shot, spotlight, a 2-reviewer quorum, structured verdict) is Review-Wave
B, and the full N-reviewer quorum + model-escalation tuning is Review-Wave C (memo §5).

---

## Consequences

- **`src/review/pass2_review.rs`** (Review-Wave B) holds the reviewer prompt, the
  no-scope grant (`act-as-reviewer` only), the spotlight/normalize wrapper, the
  structured-verdict schema, and the quorum policy. `review.depth` (ADR-CS1 D3) selects
  reviewer **count** and model **strength**.
- **A new `DispatchRole` variant** (`src/config.rs:1278`) for the `.review-*` role,
  resolved via `resolve_agency_dispatch` (`src/service/llm.rs:193`) on the weak tier
  (`Config::weak_tier_spec()`, `src/config.rs:2865`) — accounted and recorded under its
  resolved handler like `.evaluate-*` / `.flip-*`.
- **The verdict `reason` becomes a bounded category schema** (not prose), consumed by
  ADR-CS3's recorder and rendered spotlighted in `wg show`. Meta-agents that read
  verdicts must route them back through the gate as IC1 content.
- **N× weak-tier spend on the high-risk slice** (the quorum) — bounded to the small
  Unknown/high-sensitivity band; the light path stays a single cheap pass.
- **Downstream consumers must be written to stay skeptical on `accept`** (MUST-1) — this
  is a contract on *every* consumption seam, not a reviewer-local property; it is what
  makes the dual-LLM bound (D1) effective end-to-end.
- **A residual is accepted and disclosed:** a novel injection that flips the reviewer to
  `accept` (RA-1) is **not prevented** — it is bounded to **one wrong, logged,
  attributed, contained, reversible verdict** by D1 + MUST-1, and caught by the
  audit/revoke leg (ADR-CS3). The reviewer's detection layer is a cost-raiser with a
  real false-negative tail; the safety guarantee is the containment, not the detection.

---

## Alternatives rejected

- **A single reviewer on the high band.** One tuned payload flips it (RA-1). Rejected
  for the diverse-reviewer quorum + strictest-wins (D4).
- **The weakest model on the high band** (uniform weak-tier everywhere, for cost). Runs
  the most-injectable pass on the most-flippable model (RA-1e). Rejected: model strength
  escalates with `review.depth` (D4); the high band's N× *fast*-tier cost is affordable.
- **A privileged reviewer** (let the reviewer act on its own verdict — write the graph,
  fetch, quarantine directly). Turns a wrong verdict into a wrong action — collapses the
  dual-LLM bound. Rejected: the reviewer's only scope is `act-as-reviewer` (D1).
- **A free-form `reason` rendered verbatim.** Re-injects the next reader — the RA-11
  second-order channel. Rejected for a bounded category code + spotlighted rendering +
  route-verdicts-back-through-the-gate (D3, MUST-3).
- **Treating `accept` as proof-of-safety** (let downstream drop its guard on accept).
  The certification liability (doc 01 §0 reason 4). Rejected: `accept` = "no detector
  fired"; downstream stays skeptical even on accept (D5, MUST-1).
- **A non-LLM-only Pass 2** (rely solely on deterministic Pass 1). Misses the plausible
  semantic attacks (goal-hijack, social-engineering, malicious-but-correct code) only an
  LLM-review can recognise. Rejected: Pass 2 is retained but **bounded** by the no-scope
  dual-LLM pattern so its injectability is contained, not eliminated.

---

## Open questions

The ADR-CS2 stub (memo §6) and the hand-off (memo §8 items 4–5) leave three questions.
Each commits to the **durable design** and **flags the tunable value** for Erik / the
security owner.

### OQ1 — The diverse-reviewer quorum size N + the model-escalation threshold — **RESOLVED (mechanism decided; the numbers are tunable policy, flagged)**

**Resolution — the *mechanism* is decided in D4** (N independent reviewers,
strictest-wins, keyed off `review.depth`, with model strength escalating on the high
band). The spark proves the **slot** with a **2-reviewer** quorum (memo §4.3); the
production N and the exact `review.depth` cell at which model-strength escalates are a
**cost vs catch-probability trade-off**, not a design constant. **Flagged for Erik / the
security owner:** the production N, the per-depth model-strength ladder, and the
threshold band at which the quorum engages are tuned on per-class FP/FN telemetry
(Review-Wave C/D, shared with ADR-CS3 OQ1). The ADR commits to strictest-wins, the
independence requirement, and "don't run the most-injectable pass on the weakest model";
the numbers are policy.

### OQ2 — The structured `reason` category schema + its `wg show` spotlight rendering — **RESOLVED (enum-only + spotlight decided; the category set is an evolvable schema, flagged)**

**Resolution — the `reason` is an **enum-only bounded category code** (D3, MUST-3), and
its `wg show` rendering is **spotlighted as data-with-provenance**, never verbatim
prose.** That structural decision is fixed: the reviewer **cannot** emit
attacker-controlled free text into the verdict record, and any meta-agent reading
verdicts routes them back through the gate (D3). **Flagged for Erik:** the **exact
category set** (the enumerated `reason` codes — e.g. `goal-hijack`, `exfil-pattern`,
`injection-signature`, `secret-shaped`, `test-file-rewrite`, `kind-mismatch`,
`opaque-uninspectable`, …) is an **evolvable schema** that grows with the Pass-1/Pass-2
check categories (ADR-CS1 OQ1, Review-Wave D), maintained like the signature set, not
frozen here. The ADR commits to enum-only + spotlight + route-back; the category
membership is living policy.

### OQ3 — The weak-tier cost ceiling for the high band — **RESOLVED (posture decided; the ceiling value is the security owner's signed budget, flagged)**

**Resolution — the cost is *structurally bounded* by construction:** the quorum and
model-escalation run **only on the small high-risk slice** (Unknown / high-sensitivity),
and even there on the **fast (weak) tier** — the light path stays one cheap pass (D4).
So the worst case is N× *fast*-tier spend on a minority of items, not N× premium spend
on everything. **Flagged for Erik / the security owner:** the explicit per-task and
per-period **cost ceiling** for the high band (the budget at which the quorum degrades —
e.g. to fewer reviewers or to quarantine-pending-human rather than unbounded spend) is a
**budget value the security owner signs**, tuned with the FP/FN operating point
(ADR-CS3 OQ1). The ADR commits to the structural bound (high-band-only, fast-tier); the
numeric ceiling is policy. **A degrade-under-budget that silently weakened the gate is
forbidden** — any loosening of review depth is human-actioned and sigchained (ADR-CS3
D4, the anti-fatigue rule).

---

## References

- `docs/content-safety-study/04-decision-memo-and-roadmap.md` — §1 item 6/7 (the RA-1
  bound + the three MUSTs), §3 DP7 (the Fatal-as-prevention findings + the MUSTs,
  weighted toward containment), §4.2 step 4 (the reviewer-injection-contained spark
  assertion), §4.3 (the 2-reviewer spark scope), §6 ADR-CS2 stub, §8 items 4–5.
- `docs/content-safety-study/03-adversarial-evaluation.md` — **RA-1** (reviewer-
  injection, fatal-as-prevention), **RA-1b** (boundary escape), **RA-1e** (the weakest
  model is the most injectable), **RA-11** (the verdict-`reason` second-order channel),
  §5 (detection scores 2–3, containment/revoke scores 4).
- `docs/content-safety-study/01-threat-and-prior-art.md` — §0 reason 4 (the
  certification liability), the dual-LLM / spotlighting prior art (PA-\*).
- `docs/ADR-content-safety-001-review-gate.md` (Proposed, sibling) — Pass 2's place in
  the pipeline, the `review{depth}` face that selects reviewer count + model strength,
  IC1 spotlight-as-data (D4), the compose contract.
- `docs/ADR-content-safety-003-verdict-audit-revoke.md` (Proposed, sibling) — the
  uniform accept/quarantine/reject verdict the reviewer emits, the strictest-wins
  monotonic rule, the sigchain record the structured `reason` feeds, and the
  audit/revoke leg that catches the contained false-accept.
- `docs/ADR-fed-004-loadable-state-safety.md` — OQ1 (the scan categories the reviewer's
  prompt + Pass-1 share), the fail-closed/escalate-on-soft-hit posture.
- `src/service/llm.rs:193` (`resolve_agency_dispatch`), `src/config.rs:2865`
  (`Config::weak_tier_spec()`), `src/config.rs:1278` (`DispatchRole`) — the weak-tier
  agency one-shot mechanism the `.review-*` reviewer runs on.
