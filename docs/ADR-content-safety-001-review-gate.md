# ADR-CS1 (WG-Review): The Inbound-Content Review Gate — Consumption Gate · Cheap→Expensive Pipeline · the `review{}` Leash Face

**Status:** Proposed
**Date:** 2026-06-25
**Decision:** Generalize **ADR-fed-004's D6 load pipeline** from one content class (IC3 loadable state) to a **content-class-generic consumption gate** — `WG-Review` — hooked at **four ingest seams** (IC1 federation placement / graph-import, IC2 `ResultEnvelope` accept, IC3 `StateSnapshot` load *unchanged*, IC4 inbox). Inbound content is received, signed, CAS-addressed, and attributed **freely**, then **held un-consumed until review grants consumption** (*received ≠ consumed*). One **fail-closed, monotonic, cheap→expensive** pipeline — **Pass 0** provenance precondition → **Pass 1** deterministic lint → **Pass 2** quarantined weak-tier LLM-review → **Pass 3** sandboxed detonation (IC2 / opaque-IC3 only) → **Pass 4** the reused S-5 human gate — screens every item. Review *depth* is **not a new threshold**: it is the WG-Exec `leash()` engine's **new `review{depth, default_verdict}` output face**, keyed on the existing `trust_level` × sensitivity inputs, with **fail-closed-on-unlabeled** + **monotonic-escalate-on-flag** + surfaced + linted. Each pass is a `--after` review task; IC1 text is always **spotlighted-as-data**; directive messages route through the IC1 pipeline. **No** new `TrustLevel`, **no** new dial, **no** new human-gate, **no** `WG_REVIEW_COMPAT_VERSION`.

> **This is the inbound-content review-gate WG-Review ADR.** It is one of three
> (with **ADR-CS2**, reviewer hardening, and **ADR-CS3**, verdict / audit / revoke)
> that flesh out the stubs fixed in the content-safety decision memo
> (`docs/content-safety-study/04-decision-memo-and-roadmap.md` §1–§3, §6 ADR-CS1 stub,
> §8 items 1–3). This ADR formalizes the **mechanism** — placement, the pipeline, and
> the trust-proportional depth dial; ADR-CS2 owns the reviewer's anti-injection bound
> and ADR-CS3 owns the verdict semantics and the audit/revoke leg.
>
> `WG-Review` is a **consumer** of two prior decisions and **invents no third trust
> system**. It composes with — and never contradicts — **WG-Fed** (`trust_level`, the
> sigchain, the ADR-fed-004 S-5 load pipeline it generalizes) and **WG-Exec** (the
> HQ11 `leash()` engine, the HQ2 `verify.rs` accept seam, the worktree/UCAN
> blast-radius bound). The decision was *made* in the memo; this ADR records it and
> resolves the stub's open questions. It is **not** a re-litigation of the choice
> (`WG-Review` = the S-5 D6 pipeline generalized to all four content classes, weighted
> toward containment+audit over detection) — that is settled.

---

## Context

WG-Fed and WG-Exec defend four axes of the federated-WG threat surface: **who**
authored content (`trust_level`, provenance, the sigchain — WG-Fed), whether a result
is **correct** (the HQ2 verification leash — WG-Exec), the **authority/damage** an
action may do (UCAN blast-radius — WG-Exec), and **containment** of execution (the
worktree isolation ladder — WG-Exec). **None of them answers the question *is this
inbound content adversarial in intent?*** (memo §1, doc 01 §1). A signature proves
*who wrote* a thing, never that it is *safe to consume* — the load-bearing S-5 finding
of ADR-fed-004, now lifted out of the single state-load path and recognised as a
property of **every** inbound class.

The threat is all four inbound content classes, not only loadable state (doc 01 §2):

- **IC1 — task / prompt text** seeded across a federation boundary (graph-import,
  cross-WG task placement). The classic prompt-injection / goal-hijack surface (A-1.\*).
- **IC2 — code / artifact** arriving on the `ResultEnvelope` accept path. The
  malicious-but-correct diff, the backdoor that passes its own tests, the test-file
  rewrite (A-2.\*, X-6 / TC8).
- **IC3 — loadable state** (`StateSnapshot`). The class ADR-fed-004's D6 pipeline
  *already* covers; WG-Review leaves it unchanged and reuses it as the template.
- **IC4 — message** (the inbox). A directive-bearing or task-seeding message is an IC1
  attack wearing a message envelope (A-4.\*).

ADR-fed-004 built a content-semantics gate for **exactly one** of these (IC3) and
explicitly deferred the general scan layer, noting it "requires an AI-input-safety
layer WG does not have today." This ADR builds that layer — **once, generically** —
and hooks it at the three ingest seams S-5 never touched, so the same fail-closed,
trust-proportional pipeline guards code, tasks, and messages exactly as it already
guards state.

The bar this ADR holds itself to is the memo's, inherited from doc 01 §0: *raise the
attacker's cost, raise the catch probability, bound the blast radius, make every miss
auditable and reversible — and do **not**, because it **cannot**, certify inbound
content as safe.* The gate's safety guarantee is its **containment + audit + revoke**
leg, not its detection leg (formalized in **ADR-CS3**); this ADR fixes the mechanism
that carries that guarantee, while ADR-CS3 owns the guarantee itself.

One composition fact this ADR pins (a fix, not a contradiction — memo §2.3): the
seams the upstream studies named as "existing primitives" —
`src/identity/state_safety.rs` (ADR-fed-004 S-5), `src/providers/placement.rs` and
`src/providers/verify.rs` (WG-Exec) — are **proposed, not yet landed** in the tree.
WG-Review therefore treats them as **to-be-built seams** (Review-Wave B), which is
*why* digest-pinned consumption is a MUST (**ADR-CS3** MUST-2): the implementer builds
the seam fresh, and the TOCTOU surface (RA-8) turns on exactly where it lands. The
four primitives that *are* landed are reused as-is:

- **`TrustLevel`** (`Verified | Provisional | Unknown`, `src/graph.rs:1920`), carried
  on `Agent.trust_level` (`src/agency/types.rs:521`) — the single dial, **read** as the
  review-depth input. No new enum.
- **`resolve_agency_dispatch`** (`src/service/llm.rs:193`) + **`Config::weak_tier_spec()`**
  (`src/config.rs:2865`) — the weak-tier agency one-shot mechanism the Pass-2 reviewer
  runs on, exactly as `.evaluate-*` / `.flip-*` do today.

---

## Decision

### D1 — A consumption gate at four ingest seams, not an admission gate

The gate sits on the **consumption edge** of each ingest seam — **not** at admission:

- **IC1** — federation placement / graph-import: gate before the imported task can
  become `ready`.
- **IC2** — the `ResultEnvelope` accept path: gate before merge-back *and* before any
  downstream `--after` consumer reads the artifact (the TC8 seam).
- **IC3** — `StateSnapshot` load: **unchanged**; reuses the ADR-fed-004 D6 pipeline.
- **IC4** — inbox: a plain message is delivered freely; a **directive-bearing or
  task-seeding** message routes through the IC1 pipeline (D4).

Content may be **received, signed, CAS-addressed, and attributed freely** (the existing
WG-Fed *who* layer runs untouched); it is then **held un-consumed until review resolves
`accept`** (the dual-LLM quarantine law made structural — *received ≠ consumed*). A
consuming task cannot become `ready` until its review task(s) resolve; the dependency
is expressed as an ordinary `--after` edge.

**Why admission is rejected.** An admission gate (refuse the bytes at the door) throws
away the provenance + audit substrate WG already paid for and **loses honest-but-unusual
work irreversibly** (doc 02 §1). A consumption gate keeps the bytes for audit,
quarantines by default, and makes every accept/reject a **reversible, sigchained event**
(the verdict record — ADR-CS3). A wrongly-rejected legit task is recoverable; its
false-positive cost is **latency, not lost work** (RA-6).

### D2 — One fail-closed, cheap→expensive pipeline; each pass a `--after` review task

There is **one** pipeline, ordered cheap-deterministic-first, expensive-judgment-last,
**fail-closed** (a pass that errors or times out escalates stricter, never opens) and
**monotonic** (any soft hit moves the verdict one level stricter):

| Pass | What it is | Cost / tier | Scope |
|---|---|---|---|
| **Pass 0** | provenance precondition (the reused WG-Fed *who* layer + sigchain) | ~free | every item |
| **Pass 1** | fast deterministic lint / classifier, **normalize-before-scan** | near-zero (smoke-gate tier) | every item |
| **Pass 2** | quarantined weak-tier LLM-review (`.review-*`, no privileged scope, spotlighted input — **ADR-CS2**) | weak (`fast`) tier | suspicious band |
| **Pass 3** | sandboxed detonation, reusing the WG-Exec isolation ladder | expensive tier | IC2 / opaque-IC3 only |
| **Pass 4** | the reused ADR-fed-004 S-5 human-in-loop gate | human | uncertain / high-impact tail |

**The number of review tasks *is* the depth (D3), not a constant.** Each pass is a WG
task on a `--after` edge in front of the consuming task, so a concrete pipeline is
literally:

```
ingest → .review-lint(T) → [.review-llm(T)] → [.review-sandbox(T)] → [human gate] → T
```

A `Verified` low-sensitivity source collapses to just `.review-lint`; an `Unknown`
author's opaque code expands to the full chain. Building each pass from the same WG
task primitive means the gate is **a node in the live graph** — dispatched, surfaced,
and audited like everything else (memo §3 DP2). **Pass 1 normalizes before it scans**
(NFKC-fold, strip zero-width / tag characters, decode common encodings) so a known
attack hidden behind an encoding (RA-2) is caught, not laundered.

**Why fail-closed and cost-ordered.** Every mature prior-art system is layered and
cost-ordered (doc 01 laws 1–2; PA-5/PA-7/PA-8): Pass 1 catches the cheap/known 80% at
near-zero cost, Pass 2 catches plausible semantic attacks, Pass 3 catches
behaviour-revealing payloads **and doubles as containment** (the evaded payload still
runs inside the bound — RA-10). The expensive tier is reserved for the uncertain
code/opaque tail.

### D3 — Trust-proportional depth = the existing `leash()` engine + a new `review{}` face

Review depth is **an additional output of the existing WG-Exec `leash()` engine
(HQ11)**, not a new threshold and not a new dial:

```
leash(author_trust, task_sensitivity, …) → { …, review{ depth, default_verdict } }
```

The depth matrix (the ADR-fed-004 OQ2 trust matrix **generalized** from IC3 to all four
classes; rows are the author's trust *relative to the consumer*):

| Author `trust_level` × content | Review depth | Default verdict on clean |
|---|---|---|
| **Verified**, low-sensitivity, transparent | Pass 1 only | **accept** |
| **Verified**, high-sensitivity **or** code/opaque | Pass 1+2 (+3 for unknown code/opaque) | accept on clean; **human on soft hit** |
| **Provisional** (TOFU default for federated peers) | Pass 1+2, human-in-loop on any flag | accept on clean; **human** on flag |
| **Unknown** | Pass 1+2+3, **quarantine-by-default** | **quarantine**; human to release |
| **Unlabeled sensitivity** (fail-closed cell) | **deep** (treated Unknown / high) | **quarantine** — never light (WG-Exec D-i) |
| **same-self** (IC3 resume of *my own* continuous self) | Pass 1 scan only, no human gate | accept on clean (the S-5 happy path) |

Two coherence rules are inherited **verbatim** from the dial's two existing faces:

1. **Fail-closed on unlabeled.** The gate **cannot** emit "light review" for
   `author_trust < floor` or an unlabeled sensitivity — it routes deep (WG-Exec D-i).
   The failure direction is **over-review (a false positive)**, never under-review.
2. **Monotonic escalate-on-flag.** The dial only ever tightens under suspicion (auto →
   human → refuse); it never loosens itself (ADR-fed-004 S-5 OQ2).

**One dial, three faces — and it survives the adversarial pass (DP8).** The `leash()`
engine now emits three output faces from the same input: **verification** (is it
correct? — WG-Exec HQ2), **S-5-load** (is it safe to load? — ADR-fed-004 D6), and
**review** (is it safe to consume? — this ADR). RA-9 (the dial is itself an attack
surface, the WG-Exec TC10) is bounded: a self-asserted `sensitivity = low` on a
secret-touching task is **overridden upward by taint-inference, never solely
self-asserted**; the applied `review.depth` is **surfaced** in `wg show` and **linted**
by `wg config lint` so a too-loose route is visible at a glance. The RA-9 residual
(taint-inference is never provably complete) is bounded by the fail-closed default —
the failure is over-review, not under-review. **One dial, three faces, holds under
attack.** Sensitivity *floors* depth (a high-blast action never gets the light path
regardless of author trust — the RA-3 bound, ADR-CS3 D4).

### D4 — IC1 text is spotlighted-as-data; directive messages route through the IC1 pipeline

Even an **accepted** IC1 task is presented to the consuming agent as **data-with-
provenance, never as instructions** — spotlighted/delimited on the way in (the standard
prompt-injection structural defense). This is the structural counterpart to **ADR-CS2
MUST-1** (downstream stays skeptical even on accept): `accept` removes the
dependency-edge block, it does **not** promote attacker text to trusted instructions.

A message (IC4) is delivered freely as data; a message that **bears a directive or
seeds a task** is routed through the **IC1 review pipeline** before any agent can act
on it — one pipeline, two entry points (closing the A-4.2 / A-4.4 inject→IC1 path).
There is no second message-specific gate.

### D5 — Composes with WG-Fed and WG-Exec; invents no parallel trust system

Everything genuinely *new* is small, named, and bounded so a reviewer can confirm no
second trust vocabulary was invented (the memo §2.3 compose contract):

| `WG-Review` needs | Provided by | Contract |
|---|---|---|
| *who authored this, unmodified?* | WG-Fed provenance + sigchain (Pass 0) | reused verbatim; **necessary, never sufficient** (S-5) |
| the depth dial | WG-Exec `leash()` (HQ11) | **+1 output face**, same fail-closed + monotonic + surfaced + linted rules |
| the IC3 template | ADR-fed-004 D6 / OQ1 / OQ2 | **generalized; IC3 unchanged** |
| the IC2 accept seam | WG-Exec `verify.rs` (HQ2) | **sibling check, same seam** — correctness ∥ content-semantics; both pass before merge-back |
| Pass-3 sandbox + the bound on every miss | WG-Exec isolation ladder (HQ8) + FR-V4 | reused as the detonator **and** the residual's containment |
| the human gate | ADR-fed-004 S-5 OQ2 | **reused verbatim** as Pass 4, now reachable from IC1/IC4/IC2 |
| the cross-task (TC8) defense | WG-Exec D-iii | **cross-plane** — a *joint* residual, not a review-gate claim (ADR-CS3) |

The genuinely new surface is exactly five items: the **three new ingest hooks**
(IC1/IC2/IC4), the **per-class Pass-1/Pass-2 check sets**, the **uniform
accept/quarantine/reject verdict** (ADR-CS3), the **`.review-*` agency role + the
`review{}` leash face**, and the **three MUSTs** (ADR-CS2/CS3). There is **no** new
`TrustLevel`, **no** new reputation system, **no** new human-gate, **no** new
wire/crypto, and **no** `WG_REVIEW_COMPAT_VERSION` — the verdict rides the existing
WG-Fed/WG-Exec envelopes (ADR-CS3). This is the same discipline by which WG-Exec
composed onto WG-Fed: a consumer, not a peer.

---

## Status

**Proposed.** This ADR records the decision exactly as fixed in the content-safety
decision memo (§1–§3, §6 ADR-CS1 stub) and resolves the stub's open questions below.
**Erik ratifies it to Accepted** — that human gate is deliberately not set here. Per
the roadmap (memo §5), **no review code lands until ADR-CS1/CS2/CS3 are Accepted**, and
Review-Wave B (the content-safety spark) additionally depends on WG-Fed Wave 5
(ADR-fed-004 Accepted + the D6 pipeline landed) and WG-Exec Exec-Wave B (the `leash()`
engine + the `ResultEnvelope` accept seam landed).

---

## Consequences

- **A new `src/review/` module** (Review-Wave B): `mod.rs` (the verdict enum + the
  per-class registry — **no** `WG_REVIEW_COMPAT_VERSION`), `pass1_lint.rs` (the
  per-class deterministic check sets + normalize-before-scan), `pass2_review.rs` (the
  `.review-*` weak-tier one-shot — **ADR-CS2**), and `verdict.rs` (the sigchain verdict
  recorder — **ADR-CS3**).
- **The `leash()` engine gains the `review{depth, default_verdict}` output face**
  (alongside its verification face), built on WG-Exec's `placement.rs`, with the D3
  matrix and the fail-closed-on-unlabeled + monotonic rules. The applied depth is
  surfaced in `wg show` and linted by `wg config lint`.
- **A new `DispatchRole` variant** (the reviewer role; `DispatchRole` is at
  `src/config.rs:1278` alongside `Evaluator` / `Assigner`), routed via
  `resolve_agency_dispatch` (`src/service/llm.rs:193`) on the weak tier
  (`Config::weak_tier_spec()`, `src/config.rs:2865`) — `.review-*` tasks are recorded
  under their resolved handler exactly like `.evaluate-*` / `.flip-*`.
- **Three new ingest hooks** (IC1 at placement/graph-import, IC2 at the
  `ResultEnvelope` accept path as a sibling to `verify.rs`, IC4 at the inbox); **IC3
  reuses the D6 pipeline unchanged.** Because the three seams are *to-be-built*,
  digest-pinned consumption (ADR-CS3 MUST-2) is enforced as each is built.
- **A latency tax on every cross-trust ingest**, paid as a `--after` dependency edge: a
  consuming task waits for its review chain. The light path (`Verified`,
  low-sensitivity) is one cheap deterministic pass; the cost is concentrated on the
  suspicious band, where it belongs.
- **The Pass-1/Pass-2 check sets are a living, maintained policy surface**
  (Review-Wave D), like an antivirus signature set — not a write-once check.
- **A residual is accepted and disclosed:** the gate **screens** (raises cost, raises
  catch probability) but **does not certify** content as safe. The residual-risk
  boundary and the containment+audit+revoke guarantee that bounds it are formalized in
  **ADR-CS3**; this ADR's pipeline is the mechanism that carries it.

---

## Alternatives rejected

- **An admission gate** (refuse the bytes at the door). Discards the provenance + audit
  substrate WG paid for and loses honest-but-unusual work irreversibly (doc 02 §1).
  Rejected for the consumption gate (D1), which keeps the bytes, quarantines by default,
  and makes every decision reversible and sigchained.
- **A parallel trust / scan system** (a second trust enum, a bespoke reputation store,
  a separate review-only dial). The whole point is **generalize, don't parallel** (memo
  §0, §2.2). Rejected: WG-Review reads `trust_level`, adds **one** `leash()` face, and
  reuses the sigchain + human gate (D5).
- **A single classifier** ("just add a good prompt-injection detector"). No mature
  system is one layer (doc 01 law 1); a single LLM detector is the **most-injectable
  component** in the system (ADR-CS2). Rejected for the layered, cost-ordered,
  fail-closed pipeline (D2).
- **A fixed depth for all content** (review everything deeply, or review everything
  lightly). Deep-everything is unaffordable and trains operators to disable the gate;
  light-everything is no gate at all. Rejected for trust-proportional depth on the
  existing dial (D3).
- **A separate message-safety gate** (a fourth, IC4-specific mechanism). A directive
  message *is* an IC1 attack; a separate gate is a second surface to keep in sync.
  Rejected: directive messages route through the IC1 pipeline (D4).
- **A real-time / synchronous inline check.** Review is a `--after` dependency at
  work-speed, not a low-latency inline interceptor (memo §7 non-goal 11). Rejected: the
  gate is a graph node, dispatched like any task.

---

## Open questions

The ADR-CS1 stub (memo §6) and the handed-off checklist (memo §8 items 1–3) leave three
questions for this ADR. Following the ADR-fed-004 convention, each commits to the
**durable design** (categories, posture, structure) and **flags the tunable value**
(operating point, ruleset contents, exact cells) for Erik / the security owner, rather
than freezing a policy value the decision does not own.

### OQ1 — The Pass-1 deterministic ruleset seed + the per-class cadence/owner — **RESOLVED (categories + posture; the ruleset is a living policy, flagged)**

**Resolution — Pass 1 is a per-class, defense-in-depth, fail-closed deterministic
filter, defined by *category*, not by a frozen signature list** (the OQ1 categories of
ADR-fed-004, generalized from state-kinds to all four classes). The categories:

- **IC1 / IC4 (text):** normalize-before-scan, then known-injection-signature and
  instruction-in-data-position heuristics (system-prompt-override directives,
  role-confusion, tool/command-invocation strings, exfiltration patterns).
- **IC2 (code/artifact):** known-malware / typosquatted-dep signatures, committed-secret
  scan, **test-file-rewrite detection (X-6)**, kind/structure mismatch, first-order
  dep-diff (not the unbounded transitive closure — memo §7 non-goal 5).
- **IC3 (state):** unchanged — the ADR-fed-004 OQ1 scan.

A hard hit blocks; a soft hit **escalates the verdict one level stricter** (monotonic,
D3). *Why categories not a list:* committing to the categories + the fail-closed,
escalate-on-soft-hit posture is the durable design; the **exact signature contents and
confidence cut-offs are a maintained policy surface** that evolves like an AV signature
set (Review-Wave D). **Flagged for Erik / the security owner:** the seed ruleset, the
block-vs-escalate confidence thresholds, and the **per-class curation cadence + owner**
are a false-positive/false-negative trade-off to set and revisit, not a one-time
constant. This ADR commits to the categories, the per-class split, and the posture.

### OQ2 — The `review.depth` matrix operating point — **RESOLVED (matrix shape decided; the exact cell values are the security owner's signed operating point, flagged)**

**Resolution — the matrix *shape* is decided in D3** (the six rows over
`trust_level` × sensitivity × opacity, with fail-closed-on-unlabeled and
monotonic-escalate). What remains is the **operating point**: exactly which
trust × sensitivity cells get light / Pass 2 / Pass 3 / human. This is the RA-6 FP/FN
trade-off, and per the memo it is **published per-class telemetry signed by the security
owner** (Review-Wave D), **never a silent constant** (doc 01 law 7). **Flagged for
Erik:** the precise cell assignments and any future re-tuning are the security owner's
call, made on per-class telemetry, with the policy-loosening lever human-actioned and
sigchained (ADR-CS3 OQ1). The ADR commits to the matrix shape and the two coherence
rules; the numbers are a surfaced, linted policy, not a design constant.

### OQ3 — The sensitivity classifier (taint-inference inputs + auto-label-vs-self-assert boundary) — **RESOLVED (posture decided; one shared classifier, the inputs flagged)**

**Resolution — sensitivity is *inferred*, not solely self-asserted, and the failure
direction is fail-closed.** A self-asserted `sensitivity = low` on a task that touches
a secret, disables the smoke gate, edits another task, or approves a merge (the A-1.4
high-blast set) is **overridden upward** by taint-inference (the RA-9 / WG-Exec TC10
defense, D3). The decided posture: **the auto-inferred label wins when it is stricter
than the self-asserted one; an unlabeled sensitivity is treated as high (fail-closed).**

Critically, **this is the *same* classifier as WG-Exec ADR-E2's, not a second one** —
one taint-inference surface feeds both the verification face and the review face of the
one dial (D3, D5). **Flagged for Erik / shared with WG-Exec ADR-E2:** the exact
taint-inference inputs (how deep through `--after` edges the taint propagates) and the
precise auto-label-vs-self-assert boundary are a **joint** design value to close with
ADR-E2 so the two planes share one classifier; the RA-9 residual (taint-inference is
never provably complete — a laundered payload can read low-sensitivity) is **disclosed
and bounded by the fail-closed default**, not claimed closed.

---

## References

- `docs/content-safety-study/04-decision-memo-and-roadmap.md` — §1 (the one-page
  decision), §2 (the recommended mechanism + the compose contract), §3 DP1/DP2/DP3/DP8
  (placement, the pipeline, trust-proportional depth, one-dial coherence under RA-9),
  §5 Review-Wave A/B (this ADR's wave + the spark it gates), §6 ADR-CS1 stub, §8
  items 1–3 (the open-question hand-off).
- `docs/content-safety-study/01-threat-and-prior-art.md` — the IC1–IC4 content classes,
  the A-\* attack families, the eight design laws, the mitigate-don't-eliminate stance.
- `docs/content-safety-study/02-review-mechanism-design.md` — the Pass 0→4 gate, the
  three-faces table (verification / S-5-load / review), the received-≠-consumed law.
- `docs/content-safety-study/03-adversarial-evaluation.md` — RA-2 (encoding evasion →
  normalize-before-scan), RA-6 (the FP/FN operating point), RA-9 (the dial as attack
  surface), RA-10 (detonation-as-containment).
- `docs/ADR-fed-004-loadable-state-safety.md` — **D6** (the load pipeline this ADR
  generalizes), **OQ1** (the scan categories), **OQ2** (the trust matrix), the
  IC3-unchanged contract; the proposed `src/identity/state_safety.rs` seam.
- `docs/ADR-content-safety-002-reviewer-hardening.md` (Proposed, sibling) — the Pass-2
  reviewer's dual-LLM no-scope bound, spotlighting, the diverse-reviewer quorum, the
  structured verdict, and MUST-1 (skeptical-on-accept).
- `docs/ADR-content-safety-003-verdict-audit-revoke.md` (Proposed, sibling) — the
  uniform accept/quarantine/reject verdict, the sigchain audit, digest-pinning
  (MUST-2), the loud revoke leg, and the residual-risk boundary the gate's safety
  guarantee rests on.
- `src/graph.rs:1920` (`TrustLevel`), `src/agency/types.rs:521` (`Agent.trust_level`),
  `src/service/llm.rs:193` (`resolve_agency_dispatch`), `src/config.rs:2865`
  (`Config::weak_tier_spec()`), `src/config.rs:1278` (`DispatchRole`) — the landed
  primitives this ADR reuses.
