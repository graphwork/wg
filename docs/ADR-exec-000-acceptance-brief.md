# ADR-exec-000 (WG-Exec): Exec-Wave A Acceptance Brief — the four execution ADRs, packaged for ratification

**Date:** 2026-06-26 · **Owner task:** `exec-adr-coherence` · **For:** Erik (ratification)
**Package status:** all four ADRs are **Proposed**. This brief is the one-page index Erik
reads to ratify **Proposed → Accepted** in one pass. **This brief is not itself an ADR and
sets no Status** — ratification is flipping each ADR's `**Status:** Proposed` line to
`Accepted`.

> **What this is.** A coherence pass + acceptance package over the four Exec-Wave A ADRs
> (`docs/ADR-exec-e1..e4-*.md`), which together formalize the execution-federation decision
> memo (`docs/execution-federation-study/06-decision-memo-and-roadmap.md`) into the
> commitments `WG-Exec` implementation will cite. It is **not** a fifth design — every
> decision below was made in the memo (and, for authority scope, in Erik's leash amendment).
> Per memo §5, **no execution code lands until ADR-E1/E2/E3/E4 are Accepted** — and those in
> turn are gated on **WG-Fed ADR-001 (identity) and ADR-003 (custody/UCAN) being Accepted
> first** (the substrate the execution plane consumes; `WG-Exec` invents no second system,
> NFR-4).

> **Numbering (read once).** The decision memo §6 stubs label *result-integrity* **ADR-E3**
> and *capability/lease* **ADR-E4**; the Exec-Wave A task graph **swapped** them so the files
> read placement → confidentiality → capability/lease → verification. **This brief and all
> four ADRs use the task-graph numbering:** **E1** = placement, **E2** = confidentiality,
> **E3** = capability & lease (`ADR-exec-e3-capability-lease.md`), **E4** = result integrity
> & verification (`ADR-exec-e4-verification.md`). *Same decisions, swapped labels.*

---

## Coherence verdict (the consistency + faithfulness pass)

- **Internally consistent.** The four ADRs compose without contradiction. They are four
  output faces of **one `leash(provider_trust, task_sensitivity, pool_class, env_config)`
  engine**: E1 reads its `trust_floor` for placement, E2 owns the `context{scope_tier,
  seal}` output, E3 owns the `delegation{scope,ttl}` + `lease{term,cadence}` outputs, E4
  owns the `verification{depth}` output. The shared spine — the five wire envelopes, the two
  scoped UCANs, the lease-epoch CAS fence, the fail-closed selector — is referenced
  identically across all four.
- **Shared terms verified identical** across all four: `wgid:` identity; the `TrustLevel`
  values `Verified` / `Provisional` / `Unknown` and the `trust_level` dial; the **two scoped
  attenuating UCANs** (*act-as-agent UCAN* + *graph-write UCAN*, never the root key); the
  **five execution wire envelopes** (`PlacementOffer` / `Claim` / `RunGrant` / `LeaseRenewal`
  / `ResultEnvelope`); `WG_EXEC_COMPAT_VERSION` (defined once in E1's `src/providers/mod.rs`,
  the wire's home — like WG-Fed's `WG_FED_COMPAT_VERSION` in ADR-001 — with no literal
  version pinned in any ADR, so no version drift); the **lease-epoch atomic CAS** at the
  single canonical-graph write boundary (defined in E3 D6, reused by E1 for claim contention
  and by E4 as the double-commit bound).
- **Consistent with the WG-Fed ADRs they build on.** All four take WG-Fed's `wgid:` +
  sigchain (ADR-fed-001), the custodian-held root + ssh-agent-style signing boundary and the
  **attenuating-only UCAN** with `add_key`/`rotate_root` root-locked (ADR-fed-003), the
  per-recipient sealed transport (ADR-fed-002), and the loadable-state-is-untrusted posture
  (ADR-fed-004) **as given** and never redefine them. The "central is a hint that can only
  help, never override a self-verification" invariant (WG-Fed HQ6 / ADR-fed-001 §D5) is
  inherited verbatim by E1's placement model.
- **Faithful to the decision memo**, with **one authorized departure**: **E3 §D2 carries
  Erik's trust-default / leash-as-a-dial amendment** (broad/long-lived authority by birth;
  the short/scoped "leash" is environment-driven policy, not the default; humans are never
  leashed), which *reverses* the memo's HQ5/HQ11 "short-lived UCAN per session" default. This
  is a deliberate, flagged amendment — not drift — ratified upstream in **WG-Fed ADR-003
  §D2**, and E3 argues at length why it reopens **no** Fatal finding (the integrity defenses
  — attenuating-only + the write-time/epoch-fence checks — hold at *every* dial setting;
  custody is unchanged). The memo's **three §1 commitments are all present and load-bearing:**
  **(1) fail-closed leash** (E2 D2/D7 unlabeled⇒refuse/C; E1 D3 trust-floor; E4 D2
  monotonic-tighten-never-loosen), **(2) verification re-runs in a trusted domain vs a pinned
  spec** (E4 D3, never same-provider, tests-are-spec), **(3) cross-task poison as a
  first-class placement constraint** (E4 D6 tier-by-graph-position + provenance + cross-trust
  re-verification; consumed by E1 placement).
- **Two small drifts fixed in-place** (noted, not escalated — no real conflict):
  1. **E1 and E2 cross-references used the memo's §6 stub numbering** (E3=verification,
     E4=capability/lease) while E3/E4 had already adopted the task-graph numbering — so E1/E2
     pointed at the wrong sibling files (e.g. E1 cited "ADR-E4" for the lease-epoch CAS, which
     lives in the E3 file). Reconciled: every E1/E2 cross-reference was repointed to the
     task-graph numbering, and a numbering note (matching E3/E4's) was added to both.
  2. **E1's References listed WG-Fed ADR-001/003 as "(Accepted)"** — but all four WG-Fed ADRs
     are still **Proposed** (Erik's gate is open). Corrected to "(Proposed; a gating
     dependency — must be Accepted first, memo §5)," consistent with E1's own Status section
     and with E2/E3/E4, which phrase WG-Fed as a dependency-to-be-Accepted, not a fact.
- **No ADR marked Accepted.** All four remain **Proposed** — that gate is Erik's.

---

## The four ADRs — decision + what still needs Erik's sign-off

### ADR-E1 — Placement & scheduling (per-authorizer, push-default, one-mechanism, no central scheduler)
**Decision (one line):** Placement authority is **per-authorizer** — no central scheduler, no
mandatory directory, no global queue in the correctness path; **push is the default**, **pull
is first-class but a `Claim` is only a request the authorizer's signed `RunGrant` decides**;
matching is a **hard filter (capability + trust-floor) then an advisory, deterministic rank**
that can never promote past the floor; **one mechanism spans private → cooperative → market**
with only `trust_level` + the applied leash changing; any directory/reputation is an optional
hint that can only help, never override a local check.

**Settled without Erik** (mechanical, the ADR closed them): **OQ3 — the `Claim` eligibility
proof** (a four-part identity/capability/trust-floor/freshness proof, all checked
authorizer-locally; "the `RunGrant`, not the `Claim`, authorizes" — a security decision
following directly from HQ10/HQ8). The one efficiency knob it flags (a cached standing
pre-enrollment capability record) is an Exec-Wave B/C optimization, not a sign-off item.

**Needs Erik's sign-off (tuning/policy — mechanism is committed):**
- **Default rank ordering (OQ1 — tuning):** ship **reliability-first** (proposed:
  live+free-capacity → liveness/eval-pass-rate tiebreak → lower cost → hash tiebreak) or
  **cost-first**, and the per-tier reweight for the B verified-overflow tier. The *structure*
  (advisory-only, deterministic herd-safe tiebreak, locally-held/signed inputs,
  reputation-never-past-the-floor) is committed; only the ordering is a value call.
- **Pull-queue anti-starvation (OQ2 — policy):** whether v1 ships **any** anti-starvation
  nudge (the proposed **age-based push fallback** for an over-age unclaimed task) or truly
  nothing for the cooperative, and the **unclaimed-age threshold** if it does. The hard
  commitment — **full auction/proportional-share fairness is a v1 non-goal** — is fixed.

### ADR-E2 — Confidentiality tier & the attestation slot (trust / minimize / attest; confidential ⇒ C-or-refuse)
**Decision (one line):** Three confidentiality levers along the leash dial — **trust (A)**
(provider sees plaintext; trusted box only), **minimize (B)** (smallest slice — bounds blast
radius, **NOT** confidentiality), **attest (C)** (a TEE the operator runs but provably cannot
read); a **confidential task routes to an *attested* provider (C) or is *refused* — never
A, never B** (FR-K5, loud); the **attestation *slot* ships early** (interface +
seal-to-quote hook, v1 payload "trust the pool or refuse," real enclave Exec-Wave D); secrets
are never long-lived plaintext on an untrusted provider; minimal-context is taint-tracked
through `--after`; **degradation is loud, never silent**, and **unlabeled fails closed**.

**Needs Erik's sign-off (the mechanism is committed in every case):**
- **TEE vendor set + diversity (OQ1 — procurement/trust-surface):** the **exact initial
  vendor set** (e.g. Intel TDX + AMD SEV-SNP, or lead with AWS Nitro), **how many independent
  vendors before the C tier is declared production** (recommendation **≥ 2**, no single
  universal forgery root), and **whether to accept cloud-CSP-rooted attestation**. The
  verifier is multi-vendor by construction; the pick is **deferred past Exec-Wave C** by the
  memo §5 guardrail.
- **Sensitivity-classifier threshold + human-in-loop (OQ2 — precision/recall):** the **exact
  auto-label `normal` band**, the **signal weights** (hard-confidential vs advisory), and
  **whether a solo-hobbyist profile may widen the `normal` band**. The mechanism
  (infer-don't-self-assert, taint-only-escalates, ambiguous ⇒ confidential + human,
  **fail-closed-on-unlabeled is non-removable**) is committed.
- **Measurement-allow-list owner + cadence (OQ3 — security governance):** the **named
  accountable owner** (recommendation: a single security owner / small rotating group — this
  is the C-i TCB, the real weak link), the **re-audit cadence** (e.g. weekly CVE/revocation
  sweep + quarterly full re-audit), and **per-deployment vs shared default**. Mechanism
  (pinned + reproducible + audited + content-addressed + fail-closed-on-revocation) is
  committed; the work is scheduled for **Exec-Wave D** (no consumers until the enclave lands).

### ADR-E3 — Capability & lease lifecycle (two scoped UCANs + CAS lease-epoch fencing)
**Decision (one line):** A worker on a borrowed box holds **two scoped attenuating UCANs and
never the root key** (act-as-agent-for-T + graph-write-to-T-only), **broad/long by default**
(Erik's amendment) and narrow/short for strangers by one coherent leash dial that moves UCAN
scope/TTL **and** lease term/cadence together; real authority is kept off low-trust providers
by an **intent-bound, rate-limited, budget-metered, logged privileged-op callback** (never a
signing oracle); revocation = **short-TTL-where-tightened + issuer-subtree + an
authorizer-local write-time check**; the cross-host lease's double-execution hazard is closed
by a **monotonic lease epoch enforced by an atomic compare-and-set at the single
canonical-graph write boundary**, with a **prefer-liveness** reclaim stance the fence makes
safe. WG-Fed's UCAN, custody, and revocation are reused verbatim (NFR-4).

**The headline ratification item:** accepting E3 **applies Erik's trust-default / leash
amendment to the execution plane** (§D2). It composes with — and is gated on — **WG-Fed
ADR-003 §D2**: confirm broad/long-lived by birth · the leash is a deployment-set dial · humans
never leashed · the integrity invariants (custody + attenuation + the epoch fence) hold at
every dial setting, so the amendment reopens no Fatal finding. **Hard sequencing:** the two
UCANs *are* WG-Fed Wave 6, so the **Exec Spark (Exec-Wave B) cannot complete before WG-Fed
Wave 6**; an interim A-tier preview (Wave-5 standing signer) is allowed only behind the
fail-closed refuse-row — trusted-pool, non-confidential, normal-sensitivity.

**Needs Erik's sign-off (tuning/policy — mechanism is committed):**
- **UCAN expiry defaults (OQ1 — tuning, shared with WG-Fed ADR-003 OQ1):** the **standing-
  signer sanity ceiling** on the trusted pool (30 / 90 days vs "until revoked"), the
  **per-task grace multiplier** `k` (expiry ≈ lease-term × k), the **high-value short-Δ**
  (≤ 15 min, shared with the freshness Δ), and **which exec scopes count as "high-value"** by
  default (proposed: a `done`-bearing graph-write UCAN and any standing multi-task signer).
- **Revocation hosting (OQ2 — policy, shared with WG-Fed ADR-003 OQ2):** whether to **also**
  expose an optional non-authoritative CRL aggregate, the **default staleness Δ** for the
  *other-verifier* check, and the **exec courtesy "your lease was reclaimed" notice**
  (recommended as a convenience hint only — the safety guarantee is the authorizer-local
  write-time reject regardless). The safety-critical revocation check is already
  authorizer-local, needing **no external lookup**.
- **Lease term + renew cadence per trust class (OQ3 — tuning):** the **exact term/cadence
  numbers** (proposed table anchored to today's local `300`s/`30`s: Verified long/relaxed,
  Provisional moderate, Unknown short/aggressive), whether the term is **trust-scaled only or
  also task-size-scaled**, and confirmation that **prefer-liveness** is the right default
  reclaim stance (a decision, made safe by the fence — flagged so Erik can veto for a paranoid
  profile).

### ADR-E4 — Result integrity & the verification leash (attribution + trusted-domain re-run vs a pinned spec)
**Decision (one line):** A borrowed result is **signature-attributed** to the agent
(unsigned/wrong-signed ⇒ rejected) — but **attribution is not integrity**; integrity rides a
**trust-proportional verification leash**: trusted ⇒ attribution + the WG eval-gate +
**random spot-checks**; low-trust + checkable ⇒ a **deterministic re-run in a TRUSTED DOMAIN
(authorizer-side or a *disjoint* trusted provider, never the producer) against the
authorizer's *pinned* spec** (tests are spec, not the provider's deliverable);
**equivalence, not byte-identity**; a forged result's blast radius is bounded by the
task-scoped graph-write UCAN and is **auditable + revocable** via provenance; **quorum is
deferred to v2** (needs unsolved sybil-resistance); **cross-task poison is a first-class
placement constraint**.

**Needs Erik's sign-off (the mechanism is committed in every case):**
- **Spot-check sample rate (OQ1 — cost/assurance):** the **shipped default `p`** (proposed
  `≈ 0.05`, rising for short-history providers / many-descendant results; `p = 1.0` floor for
  high-sensitivity/foundational tasks) and **whether spot-checks are on by default** for the
  trusted pool (recommendation: **on, at the low default** — an off-by-default spot-check is
  exactly the silent-A weakness commitment 3 exists to prevent).
- **The "checkable" boundary (OQ2 — the hard one):** confirm the **conservative v1 default**
  — treat the **semi-checkable middle as non-checkable (escalate)** rather than trust a fuzzy
  semantic LLM-judge for *integrity* — vs accepting an **LLM-judge equivalence comparator**
  earlier. The semantic comparator is the deep v2 problem, co-designed with WG-Review. The
  typed classifier (checkable / semi-checkable / non-checkable, **never let the weak case
  masquerade as verified**) is committed.
- **Test-authoring review gate (OQ3 — friction/assurance):** **which test-file changes mandate
  a *human* reviewer vs an agent/eval-gate reviewer** (recommendation: agent-review for
  `Verified`-author non-security test changes, human for low-trust authors and
  security-sensitive test paths, as a `wg config` dial). "Tests are spec" — split out, re-run
  against the *old* pinned suite, review new tests as a spec change before they ever become an
  oracle — is committed and composes with WG-Review (no parallel review system).

---

## Shared knobs (set once, applied across the WG-Fed + Exec packages)

Several "needs sign-off" items are the **same** tunable surfacing in more than one ADR — set
each once and it propagates:

- **The trust-default / leash amendment (broad/long-lived by birth).** Ratified by accepting
  **WG-Fed ADR-003 §D2**; **Exec ADR-E3 §D2 inherits and applies it** to the two UCANs + the
  lease. Confirming it for WG-Fed confirms it for `WG-Exec`.
- **High-value freshness Δ (≤ 15 min) + skew (±5 min).** Defined in **WG-Fed ADR-001 OQ4**;
  reused by **Exec ADR-E3 OQ1** (the high-value UCAN short-Δ) and **OQ2** (the other-verifier
  revocation/freshness staleness Δ). Set once upstream.
- **UCAN expiry "high-value scope" set + sanity ceiling.** **Exec ADR-E3 OQ1** is **WG-Fed
  ADR-003 OQ1 one layer down** — the same dial; set it once and the exec per-task grace +
  standing-signer ceiling follow.

---

## How to ratify

1. Confirm the **trust-default / leash amendment** for the execution plane (E3 §D2) — it is
   the one substantive departure from the exec memo and rides on WG-Fed ADR-003 §D2 being
   accepted with the same amendment.
2. Set (or bless the proposed defaults for) the **tuning/policy knobs** above; none require
   reopening any design — each ADR commits the *mechanism* and flags only the *value*.
3. Flip each ADR's `**Status:** Proposed` → `Accepted`. **Memo §5 gates:** WG-Fed ADR-001 +
   ADR-003 must be Accepted **before** these; and no execution code lands until **all four**
   of E1/E2/E3/E4 are Accepted. (The C-tier enclave / measurement allow-list — E2 OQ1/OQ3 —
   and the real semantic comparator — E4 OQ2 — are designed-in here but built in later Exec
   waves.)

---

## References

- `docs/ADR-exec-e1-placement.md` · `docs/ADR-exec-e2-confidentiality.md` ·
  `docs/ADR-exec-e3-capability-lease.md` · `docs/ADR-exec-e4-verification.md` — the four ADRs
  this brief packages.
- `docs/execution-federation-study/06-decision-memo-and-roadmap.md` — the execution decision
  all four formalize (§1 the decision + the three commitments, §3 HQ1–HQ12, §5 waves +
  guardrails, §6 ADR stubs, §7 non-goals, §8 open-question hand-off). WG-Fed ADR-003 §D2 amends
  its HQ5/HQ11 default.
- `docs/execution-federation-study/01–05` — prior-art, baseline, requirements/hard-questions,
  candidate architectures, and adversarial evaluation underlying the memo.
- `docs/ADR-fed-001-identity-key-model.md` · `docs/ADR-fed-002-transport.md` ·
  `docs/ADR-fed-003-custody-delegation-recovery.md` · `docs/ADR-fed-004-loadable-state-safety.md`
  · `docs/ADR-fed-000-acceptance-brief.md` — the WG-Fed substrate the execution plane consumes
  (gating dependency; all currently **Proposed**), and the brief this one mirrors.
</content>
</invoke>
