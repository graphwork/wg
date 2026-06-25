# ADR-CS3 (WG-Review): Verdict Semantics, Audit & Revoke — accept/quarantine/reject · Sigchain · Digest-Pinning · Cross-Plane TC8

**Status:** Proposed
**Date:** 2026-06-25
**Decision:** One **uniform `accept / quarantine / reject` verdict** across all four content classes; the pipeline verdict is the **strictest any pass reached** (monotonic), and **`quarantine` is the fail-closed default** for the unknown/unlabeled — a *held, zero-consumed* state, not "maybe-approved." Every verdict is **recorded on the existing WG-Fed sigchain** as `{verdict, reason, content_class, deciding_pass, confidence, provenance(author wgid + sigchain position)}`; **no SKIP/uncertain verdict is ever silently dropped**. Consumption is **digest-pinned** (the accept-verdict binds to a CAS digest; consume **only that exact digest, never a mutable name** — MUST-2). The **revoke leg is automatic and loud**: a later-discovered poison is traced to its author, the author's `trust_level` is **lowered**, and **TC8 downstream `--after` consumers are found and re-run**. The cross-task (RA-4) defense is **cross-plane** — it cooperates with WG-Exec D-iii (tier-by-graph-position + provenance re-run) and is **flagged, not falsely closed**, by the review gate alone. **The gate's safety guarantee is this containment + audit + revoke leg, NOT its detection leg.**

> **This is the verdict / audit / revoke WG-Review ADR — and the one that states the
> residual-risk boundary plainly.** It is one of three (with **ADR-CS1**, the gate /
> pipeline / dial, and **ADR-CS2**, reviewer hardening) formalizing the content-safety
> decision memo (`docs/content-safety-study/04-decision-memo-and-roadmap.md` §3
> DP4/DP5/DP6/DP7, §6 ADR-CS3 stub, §8 items 6–8). ADR-CS1 fixes the mechanism and
> ADR-CS2 hardens the reviewer; this ADR owns **what a verdict means, how it is
> audited, how a miss is revoked, and — the central honesty requirement — exactly what
> the gate screens versus what it accepts as residual.**
>
> It composes with WG-Fed / WG-Exec and invents no parallel trust system: verdicts ride
> the existing sigchain, the revoke leg reuses `trust_level`-lowering + the
> `auto_evaluate` / FLIP re-run machinery, and the cross-task defense is **joint** with
> the WG-Exec plane, never a duplicate. The decision was *made* in the memo (doc 03 §5
> proves detection scores 2–3 and containment/revoke scores 4); this ADR records it and
> resolves the stub's open questions. It does not re-litigate the choice.

---

## Context

Doc 03 §5 is the load-bearing finding behind this ADR: across the scorecard, the gate's
**detection** leg scores **2–3** while its **containment / audit / revoke** leg scores
**4**. The reviewer is a cheap LLM eating the attacker's text — it **will** be fooled by
novel framings (RA-1); a `Verified` defector's first low-blast payload **will** get the
light path and land (RA-3); a check-passing poison that activates only downstream
**will** pass per-task review (RA-4 / TC8). The design's promise (doc 01 §0) is **not**
that these never happen — it is that **when they do, the miss is blast-radius-bounded,
attributed, logged, reversible, and triggers `trust_level`-lowering + downstream re-run.**

Three of doc 03's twelve attack classes are **Fatal-as-prevention** — survivable only as
detect-contain-revoke, never prevented (memo §3 DP7). ADR-CS2 bounds **RA-1**
(reviewer-injection) with the dual-LLM no-scope pattern. The remaining two land here:

- **RA-3 — the trusted-actor-turned-bad.** The affordability dial that makes the gate
  runnable *is* the light path a `Verified` defector buys with reputation; no in-band
  check prevents the first clean-looking, low-blast shot.
- **RA-4 / TC8 — cross-task poison.** Per-task review is **structurally blind to
  cross-task activation** — a payload that passes its own review activates only when a
  downstream task consumes it. Named "most under-defended" by **three independent
  studies**; that consistency is the finding.

The memo §3 DP6 makes stating the **screened-vs-accepted-residual boundary** the
study's *central honesty requirement* (the "npm reality"): a gate that *implied* a
defense it does not have would be the liability doc 01 §0 reason 4 warns of. This ADR
states that boundary as a first-class decision (D5), formalizes the verdict semantics
that bound the miss to *zero-consumed* (D1), the audit substrate that makes every miss
traceable (D2–D3), and the loud revoke leg that is the **actual safety guarantee** (D4).

The audit/revoke leg reuses landed machinery, no new system: the WG-Fed **sigchain** (the
verdict-record substrate), the **`TrustLevel`** enum (`Verified | Provisional | Unknown`,
`src/graph.rs:1920`) carried on `Agent.trust_level` (`src/agency/types.rs:521`) that the
revoke leg **lowers**, and the existing `auto_evaluate` / FLIP **re-run** machinery the
TC8 downstream re-run rides. The verdict recorder lands in `src/review/verdict.rs`
(ADR-CS1, Review-Wave B).

---

## Decision

### D1 — Uniform accept / quarantine / reject; quarantine is the fail-closed default

Every pass emits one of three values; the pipeline verdict is the **strictest any pass
reached** (monotonic — a single soft hit can only tighten, never loosen):

- **`accept`** — consumption permitted. It means **"no detector fired," nothing more**
  — never certification (ADR-CS2 MUST-1 / D5).
- **`quarantine`** — content is **held, *not consumed***, pending escalation / human;
  **reversible** (re-review can release or reject). This is the **fail-closed default
  for the unknown / unlabeled** — a safe holding state that bounds risk to
  **zero-consumed** even if no human ever looks, while preserving the bytes for audit
  and possible later release (the App-Store / moderation review-queue shape, PA-3/PA-7).
- **`reject`** — refused; the author's `trust_level` may be lowered (D4), and if the
  content already propagated, TC8 downstream `--after` consumers are re-run (D4).

**Why a three-valued verdict with a *held* middle.** A two-valued accept/reject forces a
choice between losing the false positive (reject honest work) and consuming the false
negative (accept the attack) on every uncertain item. The held middle is the **only**
verdict that **bounds the miss to zero-consumed without forcing an immediate human
decision on every uncertain item** (doc 02 §2.5). It also sets the right failure
direction under the two DoS / tuning attacks:

- **RA-5 (human-in-loop DoS):** a flooded queue the human *never clears* leaves those
  items **un-consumed** — the DoS denies *liveness*, not *safety* (the right failure
  direction).
- **RA-6 (the FP/FN dial):** a false positive is **held, not destroyed** — the FP-cost
  is **latency**, so the design rightly biases toward *quarantine-not-reject* under
  uncertainty.

### D2 — Every verdict is recorded on the sigchain; no SKIP is silently dropped

Every verdict — for every item, at every pass — is recorded on the **existing WG-Fed
sigchain** (no new ledger) as:

```jsonc
{ "verdict": "accept | quarantine | reject",
  "reason": "<bounded category code>",        // ADR-CS2 D3 / MUST-3 — never free-form
  "content_class": "IC1 | IC2 | IC3 | IC4",
  "deciding_pass": "0 | 1 | 2 | 3 | 4",
  "confidence": "<low | med | high>",
  "provenance": { "author": "wgid:<…>", "sigchain_pos": "<…>",
                  "content_cid": "<blake3 digest>" } }    // the pin for D3
```

**No SKIP / uncertain verdict is ever silently dropped** (the smoke-gate discipline,
doc 01 law 7): an endpoint-unreachable / credential-missing / timeout outcome is a
**loud, recorded SKIP that fails closed** (escalates stricter — ADR-CS1 D2), never an
implicit accept. The record is what makes every accept/reject a **reversible event**
(D1) and is the substrate the revoke leg (D4) walks to find a poison's author and
descendants. The `reason` is the bounded category code ADR-CS2 D3 produces, rendered
**spotlighted** in `wg show` (MUST-3); a meta-agent reading verdicts routes them back
through the gate as IC1 content (verdicts are not trusted because the gate emitted them).

### D3 — Digest-pinned consumption (MUST-2 / RA-8)

**MUST-2 (RA-8) — the accept-verdict binds to a content digest, and consumption MUST be
of that exact digest, never of a mutable name.** A verdict is over the bytes the gate
*reviewed* — pinned by their BLAKE3 CID (the `content_cid` in D2). The consuming task
re-fetches and consumes **only that exact digest**; a post-review mutated byte changes
the CID and is **rejected**. There is **no** post-accept `git pull`, URL-fetch, or
floating dependency-version resolve between review and consumption — that TOCTOU gap is
the RA-8 attack, and digest-pinning closes it.

This is a MUST (not a note) **precisely because the ingest seams are to-be-built**
(ADR-CS1 context / memo §2.3): the implementer enumerates **every** consumption seam and
proves each digest-pinned as it is built. The **indirect / referenced-artifact seam
(A-1.5)** — content that names *another* artifact to fetch — is the one most likely to
leak a mutable name and gets explicit attention (OQ3).

### D4 — The revoke leg is automatic and loud; the cross-task (RA-4/TC8) defense is cross-plane

The gate's safety guarantee lives here. When a miss is later discovered — a contained
RA-1 false-accept, an RA-3 defector's first shot that landed, an RA-4 poison that
activated downstream — the revoke leg runs **automatically and loudly**, never slowly or
manually (a slow/manual revoke turns a bounded miss into an unbounded one):

1. **Trace.** The verdict record (D2) names the **author** (`wgid` + sigchain position)
   and the **content digest** (D3); the sigchain trace finds them deterministically.
2. **Lower trust.** The author's `Agent.trust_level` is **lowered** (`src/agency/types.rs:521`)
   so the author's **next** item takes the **deep** path (the RA-3 bound — the first
   low-blast shot is the price of an affordable gate; the *second* gets the deep path).
   This reuses WG's existing trust machinery — no new reputation system.
3. **Re-run descendants (TC8).** The downstream `--after` consumers that read the
   poisoned (digest-pinned) artifact are **found and re-run**, reusing the existing
   `auto_evaluate` / FLIP re-run machinery.

**The RA-4 / TC8 defense is *cross-plane* and is flagged, not falsely closed.** The
review gate is **per-task and structurally blind to the graph** — it cannot see that a
review-passing artifact activates only when a *different* downstream task consumes it.
So RA-4 is a **joint residual of the entire WG-Fed + WG-Exec + content-safety stack**:
the review gate supplies the **verdict record** (author + sigchain position + digest);
the **WG-Exec D-iii** plane supplies **tier-by-graph-position** (foundational/root tasks
get deep review + high-trust tiers; leaves get the light path) and the **provenance
re-run** that consumes the verdict record. This ADR **does not claim the review gate
alone closes TC8** — that would be a promise it cannot keep (RA-4). It commits to
**emitting the record the cross-plane defense needs** and cooperating with WG-Exec
D-iii; the exact interface is OQ2.

### D5 — The residual-risk boundary, stated plainly: safety = containment + audit, not detection

This is the memo's central honesty requirement (§3 DP6), stated operationally. **What
the gate *screens*** (reduces — raises attacker cost, raises catch probability):

- **Known / cheap attacks** — known injection signatures, known-malware packages,
  committed secrets, typosquatted deps, test-file rewrites (X-6), kind/structure
  mismatches — anything deterministically detectable (Pass 1, near-zero cost).
- **Plausible-but-detectable semantic attacks** — goal-hijack, social-engineering,
  malicious-but-correct code an LLM-review can recognise (Pass 2).
- **Behaviour-revealing payloads** — exfil / escalation visible only at runtime, by
  detonating unknown code / opaque state in the bound sandbox (Pass 3).

**What is *accepted as residual*** (NOT eliminated — the irreducible tail, confirmed and
audited by doc 03 §7; each is **real, bounded, disclosed, and owned**):

1. **A novel injection that flips the reviewer to `accept`** (RA-1). Bounded to **one
   wrong, logged, attributed, contained, reversible verdict** — not a privileged action
   (the dual-LLM no-scope bound, ADR-CS2 D1); the consumption that follows is bounded by
   skeptical-on-accept (ADR-CS2 MUST-1).
2. **A `Verified` defector's first clean-looking, low-blast payload** (RA-3). The light
   path the affordability dial buys; no in-band fix prevents the first shot. Bounded by
   **sensitivity-floors-depth** (high-blast actions never get the light path regardless
   of trust — ADR-CS1 D3) and **fast trust-revocation** (D4 — the second shot gets the
   deep path). **The first low-blast shot is the price of an affordable gate.**
3. **A check-passing semantic poison that activates only downstream** (RA-4 / TC8). The
   **joint cross-plane residual**; bounded by WG-Exec D-iii (tier-by-graph-position +
   provenance re-run), **not** the review gate alone (D4). Named "most under-defended" by
   three independent studies.
4. **An opaque payload no classifier can read** (RA-12). Un-screenable by construction;
   **contained, never screened** (sealed, sandbox-only, mandatory human gate — ADR-fed-004
   D5). The RA-3 × RA-12 product (an opaque blob from a `Verified` author) is the worst
   cell and **never** gets the light path regardless of trust.
5. **The FP/FN operating point** (RA-6). A tuned trade-off forever; the residual is
   whatever the chosen threshold lets through — **made visible as per-class telemetry,
   never silent** (OQ1), and signed off by the security owner.
6. **The transitive supply-chain payload dormant past detonation** (RA-7/RA-10). The npm
   reality; bounded by containment + after-the-fact audit + downstream re-run, not by
   scanning the unbounded transitive closure (memo §7 non-goal 5).
7. **The human socially-engineered or fatigued into an override** (RA-5 / A-4.3). The
   gate routed it to a human (the human-in-loop gate, below); the residual is **human
   judgment, outside the mechanism** — owned, not closed.

**The boundary, in one sentence (the line the whole study turns on):** *the review gate
does precisely what doc 01 §0 promised — raises attacker cost, raises catch probability,
bounds the blast radius, makes the miss auditable and reversible — and it does **not**,
and **cannot**, certify inbound content as safe; its detection layer is a cost-raiser
with a real false-negative tail, and its **safety guarantee is the containment + audit +
revoke layer, not the detection layer**.* **v1 invests in the right-hand column first**
(doc 03 §5): the dual-LLM bound, digest-pinning, the sigchain audit, the trust-lowering,
and the downstream re-run — because that is where the gate's coverage actually lives.

**Human-in-loop escalation (the reused S-5 gate).** The uncertain / high-impact tail
escalates to the **same ADR-fed-004 OQ2 cross-trust human gate** (Pass 4), now also
reachable from the IC1/IC4 inbox and the IC2 accept path. Two anti-fatigue rules are
**MUSTs**: (a) the queue is **batched and prioritized by blast-radius** so the human
spends attention where a miss is costly; (b) the **policy-loosening lever** (relaxing
review depth, disabling a pass) requires a **human action *and* is sigchained** — the
gate cannot be *silently* weakened under flood pressure, and `wg config lint` flags a
too-loose policy. A flooder that trips many flags **de-trusts itself** (its future items
auto-quarantine without reaching the human — RA-5). The residual here is **human**: a
determined operator who overrides under pressure is outside the mechanism (residual 7).

---

## Status

**Proposed.** This ADR records the decision exactly as fixed in the content-safety
decision memo (§3 DP4/DP5/DP6/DP7, §6 ADR-CS3 stub) and resolves the stub's open
questions below. **Erik ratifies it to Accepted** — the human gate is deliberately not
set here. No review code lands until ADR-CS1/CS2/CS3 are Accepted; the verdict recorder,
digest-pinning, and a single hand-wired downstream re-run are Review-Wave B (the spark),
and the full cross-plane D-iii TC8 defense + the `wg review` quarantine-queue UX are
Review-Wave C, joint with WG-Exec Exec-Wave C (memo §5).

---

## Consequences

- **`src/review/verdict.rs`** (Review-Wave B): the sigchain verdict recorder (the D2
  schema) + the digest-pin enforcement (D3) + the revoke trigger (D4). The verdict
  record feeds the WG-Exec D-iii re-run interface (OQ2).
- **The verdict rides the existing sigchain** — **no** `WG_REVIEW_COMPAT_VERSION`, no new
  ledger. The revoke leg reuses `Agent.trust_level`-lowering (`src/agency/types.rs:521`)
  + the `auto_evaluate` / FLIP re-run machinery — no new reputation or re-run system.
- **`wg show` renders verdicts spotlighted** (MUST-3); a `wg review` / `wg list` filter
  surfaces the **blast-radius-prioritized quarantine queue** (Review-Wave C/D) — reusing
  existing surfaces, **no bespoke moderation UI** (memo §7 non-goal 10).
- **Every consumption seam must be digest-pinned** (D3) as it is built — a contract
  checked seam-by-seam, with the indirect/A-1.5 seam called out (OQ3).
- **The cross-plane TC8 interface is a shared deliverable with WG-Exec** (D4, OQ2): the
  review gate emits the verdict record; WG-Exec D-iii owns tier-by-graph-position + the
  re-run trigger. Neither plane closes RA-4 alone.
- **The residual is disclosed, not hidden** (D5): seven owned residuals, each bounded by
  a named control; the gate is documented as a **cost-raiser + container + auditor**,
  never a certifier. v1 investment is weighted toward the containment/revoke column.

---

## Alternatives rejected

- **A two-valued accept/reject verdict** (no held middle). Forces losing the false
  positive (reject honest work) or consuming the false negative (accept the attack) on
  every uncertain item (doc 02 §2.5). Rejected for accept/**quarantine**/reject, where
  the held middle bounds the miss to zero-consumed (D1).
- **A silent dropped / implicit-accept verdict** on error/timeout/SKIP. Violates the
  smoke-gate discipline (doc 01 law 7) and fails *open*. Rejected: every SKIP is loud,
  recorded, and fails closed (D2).
- **Consume-by-mutable-name** (review a name/URL, fetch at consumption time). Re-opens
  the TOCTOU gap (RA-8) — the reviewed bytes and the consumed bytes diverge. Rejected
  for digest-pinned consumption (D3, MUST-2).
- **Claiming the review gate alone closes cross-task poison** (RA-4 / TC8). A promise the
  per-task gate cannot keep — it is blind to the graph. Rejected: RA-4 is a **joint
  cross-plane residual**, flagged and defended with WG-Exec D-iii, never falsely closed
  (D4, D5 residual 3).
- **A slow / manual revoke** (a human-driven trust-lowering + re-run after a report).
  Turns a bounded miss into an unbounded one — the window between landing and revoke is
  where the damage compounds. Rejected: the revoke leg is automatic and loud (D4).
- **Treating `accept` as a safety certification** (the certification gate). The headline
  liability (doc 01 §0 reason 4); `accept` = "no detector fired" (ADR-CS2 D5). Rejected:
  the safety guarantee is containment + audit + revoke, not detection (D5).
- **A new reputation / quarantine-queue product** (a bespoke moderation system). Invents
  a parallel surface. Rejected: reuse `trust_level` + the sigchain + `wg show` /
  `wg review` (memo §7 non-goals 3, 10).

---

## Open questions

The ADR-CS3 stub (memo §6) and the hand-off (memo §8 items 6–8) leave three questions.
Each commits to the **durable design** and **flags the tunable value or the joint
interface** for Erik / the security owner / the WG-Exec plane.

### OQ1 — The per-class FP/FN telemetry schema + the policy-loosening control — **RESOLVED (posture decided; the schema fields + operating point are signed policy, flagged)**

**Resolution — the posture is decided: the FP/FN operating point is a *surfaced,
linted, per-class-telemetried policy signed by the security owner*, never a silent
constant** (doc 01 law 7, RA-6), and the **policy-loosening lever is human-actioned and
sigchained** (D4 anti-fatigue rule) with `wg config lint` flagging a too-loose policy.
That governance is fixed. **Flagged for Erik / the security owner:** the **exact
telemetry schema fields** (per-class true/false-positive/negative counters, confidence
distributions, queue-depth/latency) and the **signed operating point** itself are tuned
on evidence in Review-Wave D, not frozen here. The ADR commits to per-class telemetry +
the signed operating point + the human+sigchained loosening lever; the field list and
the numbers are living policy.

### OQ2 — The cross-plane TC8 interface (which plane owns the re-run trigger) — **RESOLVED in principle; flagged for joint close with WG-Exec (the one item no single study can close)**

**Resolution — the *division of labor* is decided (D4): the review gate emits the
verdict record (author `wgid` + sigchain position + content digest); the WG-Exec D-iii
plane owns tier-by-graph-position and consumes that record to drive the re-run.** The
review gate does **not** own the graph-position selector — it cannot see the graph. What
remains is the **exact wire interface** and **which plane fires the re-run trigger**.
**Flagged for joint close with WG-Exec Exec-Wave C (memo §8 item 8 — explicitly "the one
item no single study can close"):** the verdict-record → D-iii re-run interface shape,
and whether the review plane *signals* and the exec plane *acts* or the exec plane
*polls* verdict records, are a **joint** decision to settle with the WG-Exec ADRs so the
two planes share one re-run path, not two. The ADR commits to emitting the record D-iii
needs and to **not claiming the review gate closes RA-4 alone**.

### OQ3 — Digest-pinning enforcement across every consumption seam (esp. the indirect/A-1.5 seam) — **RESOLVED (the rule is decided; the per-seam enumeration is a build-time checklist, flagged)**

**Resolution — the *rule* is decided (D3 / MUST-2): consumption is of the exact reviewed
CID, never a mutable name; a post-review byte change is rejected.** Because the ingest
seams are **to-be-built** (ADR-CS1 context), enforcement is a **build-time checklist** —
**every** consumption seam is enumerated and proven digest-pinned as it lands
(Review-Wave B). **Flagged for explicit attention (not a value judgment — a known
hazard):** the **indirect / referenced-artifact seam (A-1.5)** — accepted content that
*names another artifact to fetch* — is the seam most likely to smuggle a mutable name
past the pin, so it gets a dedicated test (the spark's step 6 digest-pin assertion is the
seed). The ADR commits to the digest-pin rule and the per-seam enumeration; the seam
inventory grows as the seams are built, with A-1.5 called out as the priority.

---

## References

- `docs/content-safety-study/04-decision-memo-and-roadmap.md` — §1 item 5 (safety =
  containment+audit, not detection), §3 DP4 (verdict semantics), DP5 (human-in-loop),
  **DP6 (the screened-vs-residual boundary — the central honesty requirement)**, DP7
  (the RA-3/RA-4 containment columns + MUST-2), §4.2 steps 3/5/6 (the IC2-accept,
  detect-contain-revoke, and digest-pin spark assertions), §6 ADR-CS3 stub, §8
  items 6–8.
- `docs/content-safety-study/03-adversarial-evaluation.md` — **RA-3** (trusted defector),
  **RA-4 / TC8** (cross-task poison — most under-defended), **RA-5** (human-in-loop DoS),
  **RA-6** (the FP/FN dial), **RA-7/RA-10** (supply-chain / detonation-evasion), **RA-8**
  (TOCTOU / mutable-name), **RA-12** (opaque blob), §5 (detection 2–3, containment/revoke
  4), §7 (the residual tail).
- `docs/content-safety-study/01-threat-and-prior-art.md` — §0 (the mitigate-don't-
  eliminate bar + reason 4, the certification liability), law 7 (no silent SKIP), the
  X-6 test-file-rewrite and TC8 cross-task findings.
- `docs/ADR-content-safety-001-review-gate.md` (Proposed, sibling) — the pipeline whose
  verdict this ADR defines, the `review{depth, default_verdict}` face, the fail-closed
  defaults, sensitivity-floors-depth (the RA-3 bound), the compose contract.
- `docs/ADR-content-safety-002-reviewer-hardening.md` (Proposed, sibling) — the dual-LLM
  no-scope bound (the RA-1 containment), the structured/spotlighted `reason` (MUST-3),
  skeptical-on-accept (MUST-1) — the controls the residual boundary (D5) leans on.
- `docs/ADR-fed-004-loadable-state-safety.md` — D6 (the IC3 pipeline), **OQ2** (the
  cross-trust human gate reused as Pass 4), D5 (the opaque-contain posture, RA-12).
- `docs/execution-federation-study/06-decision-memo-and-roadmap.md` — WG-Exec **D-iii**
  (tier-by-graph-position + provenance re-run — the cross-plane RA-4/TC8 partner), the
  FR-V4 blast-radius bound, the HQ2 `verify.rs` accept seam.
- `src/graph.rs:1920` (`TrustLevel`), `src/agency/types.rs:521` (`Agent.trust_level` —
  lowered by the revoke leg) — the landed trust machinery the audit/revoke leg reuses;
  the `auto_evaluate` / FLIP re-run machinery the TC8 re-run rides.
