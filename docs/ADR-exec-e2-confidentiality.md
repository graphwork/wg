# ADR-E2 (Exec): Confidentiality Tier & the Attestation Slot — Trust / Minimize / Attest; Confidential ⇒ C-or-Refuse

**Status:** Proposed
**Date:** 2026-06-26
**Decision:** Context confidentiality on borrowed compute has exactly **three levers along the one leash dial**: **trust (A)** — the provider sees plaintext, acceptable *only* on a `Verified` box you own; **minimize (B)** — ship the smallest `ContextScope` slice, which bounds blast radius but is **NOT confidentiality** (the provider reads its slice and every token of it); and **attest (C)** — a TEE the operator runs but provably cannot read, the *only* lever that defends context against a provider you do not trust. **A task that requires confidentiality routes to an *attested* provider (C) or is *refused* — never to A, never to B** (FR-K5, loud). We ship C's **attestation slot** (the handshake interface + the seal-context-to-quote hook) **early** — before any enclave exists — so confidential work is a provider-capability away rather than a redesign; v1's payload behind the slot is "trust the pool, or refuse, loudly," and the real enclave lands in Exec-Wave D. **Secrets** are never long-lived plaintext on an untrusted provider (authorizer-held + privileged-op callback, or sealed-to-attestation, FR-K2). **Minimal context** defaults to the smallest slice and widens on demonstrated need, with sensitivity **taint-tracked through `--after` edges** (the slice-builder is the confidentiality TCB, X-2). Degradation is **loud, never silent** (FR-K1/K5, D-i fail-closed).

> **This is one of the two load-bearing execution ADRs** (with ADR-E3, result
> integrity). It owns **HQ1 — context confidentiality on borrowed compute, *the*
> execution-plane crux** — and the confidentiality-tier call the decision memo
> demands be made *plainly*. The decision was *made* in the execution-federation
> decision memo (`docs/execution-federation-study/06-decision-memo-and-roadmap.md`
> §1 "the confidentiality-tier call", §2.2, §3 HQ1, §3 HQ8, §6 ADR-E2 stub, §8
> hand-off items 2–3); this ADR formalizes it and resolves the stub's three open
> questions. It is **not** a re-litigation of the architecture choice (`WG-Exec`
> = Candidate D's leash-selector, shipped **A-first**, with **C's attested tier as
> the confidential escape-hatch — its slot shipped before any enclave** — and B as
> the vouched-overflow tier) — that is settled (memo §1/§2, defended adversarially
> in `docs/execution-federation-study/05-adversarial-evaluation.md` §3.1).
>
> **It invents no identity, capability, or crypto.** Confidential transit reuses
> WG-Fed's per-recipient sealed envelopes (`docs/ADR-fed-002` transport,
> `docs/ADR-fed-001` keys); the secrets-off-the-provider callback rides WG-Fed's
> custodian-held-root + ssh-agent-style "sign this digest" boundary
> (`docs/ADR-fed-003` §D1) and the two scoped attenuating UCANs (ADR-fed-003 §D3,
> formalized for execution in **ADR-E4**); the sealed-state-at-rest story composes
> with the loadable-state safety pipeline (`docs/ADR-fed-004` §D6). The boundary is
> explicit: **identity/delegation/crypto formats are inherited from WG-Fed; the
> *confidentiality policy* — the three tiers, the C-or-refuse rule, the attestation
> slot, the sensitivity classifier — is this ADR's** (memo §2.3, NFR-4).

---

## Context

The execution plane's entire reason for tension is one sentence: **running an agent
exposes its working context to whoever owns the compute.** When the box is yours,
that is fine; when it is a stranger's GPU, "borrow a friend's compute" must not
silently become "hand a stranger your secrets." This is **HQ1, *the* crux** (doc 03
§"HQ1", EX4/FR-K\*): the more compute you reach for (the openness the whole study
exists to enable), the more a provider you do not control can read.

The context a worker holds is not one thing. **FR-K1** (the MUST that grounds this
ADR) demands the architecture *name, for each placement class, what the provider can
and cannot see* of: **task input**, the **graph slice**, **secrets** (API keys,
graph-write tokens), **prior state**, and **tool outputs**. Confidentiality must be
a *stated, bounded property per trust level* — never "secret by accident."

The adversarial pass (`docs/execution-federation-study/05` §3.1) attacked exactly
this surface — **TC1, malicious-provider-reads-context** — across all four candidates
and returned a verdict that this ADR is built to honor:

- **A (trust):** *no defence vs a malicious provider — by design.* The provider sees
  all plaintext; A's confidentiality reduces to "you own/trust the box." Attacking A
  on TC1 is attacking its *scope boundary* (never enroll an untrusted provider in A),
  which holds only because A is **trusted-compute-only** (A-i, Fatal-if-misused).
- **B (minimize):** *minimization only.* The provider reads the minimal slice and
  **every token of it**; transitive over-inclusion (X-2) and the content-vs-credential
  problem remain. **"You cannot run confidential work on B" is inherent, not a bug.**
  The dangerous deployment error is mistaking "minimized" for "confidential" and
  routing mildly-sensitive work to B.
- **C (attest):** **the only candidate that defends TC1 against a provider you do not
  trust** — context is plaintext *only inside the enclave*; the operator runs it but
  provably cannot read it. But C is not free: its residuals are real and named
  (doc 05 §3.1, C-i…C-iv) —
  - **C-i (chief residual):** C's confidentiality+integrity TCB is the **measurement
    allow-list, not the silicon.** Attestation proves "*a* runtime with measurement M
    ran in a genuine enclave," **never that M is *safe*.** A permissive allow-list (a
    version range, a vendor-default image, a debug-hooked runtime) attests a runtime
    that exfiltrates from the inside. **Curating that allow-list is an ongoing,
    security-critical operational job** — the real weak link.
  - **C-ii:** a *broken* attestation is **worse than none** — it forges
    confidentiality *and* integrity at once, so attestation must be
    **defence-in-depth alongside** the eval-gate + (where checkable) a cross-domain
    re-run, **never a sole oracle**.
  - **C-iii (inherent-bounded):** a **non-WG, centralized, compellable vendor trust
    root** (Intel/AMD/AWS sign the quotes; a leaked or compelled vendor key forges
    any quote). Cannot be removed from a TEE — only disclosed, **diversified across
    vendors**, and monitored for key revocation. This is what makes C the *least*
    decentralized tier.
  - **C-iv (inherent-bounded):** **side-channels** (Foreshadow/SGAxe/ÆPIC-class) are
    an active arms race; C's confidentiality is *probabilistic, not absolute*.

  **Cross-candidate verdict (TC1): C > D-routing-to-C > B > A.** Only C (and D when
  its selector routes confidential work *to* C) actually defends context against P1.

There is a second, structural risk the selector itself carries — **TC10**, the
leash-engine mis-route: a config error or an attacker-influenced sensitivity label
that silently routes a confidential task to A/B instead of C. doc 05 D-i names the
Fatal-as-written form: **an *unlabeled* task that fails *open* to A is silently
exposed.** The confidentiality guarantee therefore lives as much in the **fail-closed
selector** and the **honest sensitivity label** as in the crypto.

This ADR fixes the confidentiality policy the rest of the execution plane reads:
the three tiers, the C-or-refuse rule, the attestation *slot* (interface before
silicon), the secrets-off-the-provider mechanism, minimal-context with taint-tracked
sensitivity, and the per-trust-class threat-model table FR-K1 demands. It composes
with sandboxing/isolation (HQ8/FR-D4–D5 — the isolation-class ladder whose top rung
*is* the TEE) and with placement (ADR-E1 — confidential routing is a placement
constraint). **No execution code lands until ADR-E1/E2/E3/E4 are Accepted** (memo §5,
Exec-Wave A).

---

## Decision

### D1 — Three confidentiality tiers along the one leash dial, named *honestly*

Confidentiality on compute you do not own has **exactly three levers**, and they are
the three regions of the one `leash()` policy's `context{scope_tier, seal}` output
(memo §3 HQ11) — *not* a separate confidentiality subsystem:

| Lever | What it is | What the provider can read | Honest name |
|---|---|---|---|
| **A — trust** | Plaintext on a box you own/trust | **Everything** in the shipped slice | *Trust* — bounded only by "you trust the operator" |
| **B — minimize** | The smallest `ContextScope` slice | **Every token of the slice** it is given | *Minimize* — bounds blast radius, **NOT confidentiality** |
| **C — attest** | An enclave the operator runs but cannot read | **Nothing** (plaintext only inside the TEE) | *Attest* — the only defence vs an untrusted operator |

The naming is load-bearing. **Minimization is not confidentiality** — a B provider
reads its slice and everything in it; shipping *less* bounds *how much* leaks, never
*whether* it leaks. We state this plainly so no deployment mistakes "minimized" for
"hidden" (the §3.1 deployment error). The default tier is **A** (the slack leash
default for a `Verified` provider at `normal` sensitivity, memo §2.2 — EX6 honored).
**C is an escape-hatch reached on demand, per confidential task — never globally**;
we pay its TEE tax (C-i/iii/iv) *only* for the tasks that require it.

### D2 — Confidential ⇒ C (attested) or refuse — **never A, never B** (FR-K5, loud)

**A task that requires confidentiality is offered only to providers that present a
*verified* attestation (C), or it is held/refused** — with a stated
"no eligible confidential provider" reason. It is **never** placed on A (a provider
that sees plaintext) or B (a provider that sees the minimized plaintext). This is the
**MUST** the whole tier exists to enforce (FR-K5): *context confidentiality degrades
loudly, never silently.*

The **fail-closed selector** is the enforcement point, not a downstream check:

- The `leash()` engine **cannot emit** `context = plaintext` (A) or `context =
  minimized-plaintext` (B) for a task whose resolved sensitivity is `confidential`;
  it returns the **sealed-to-attestation (C)** region or **refuses** (memo §3 HQ11,
  the policy *is* the guardrail).
- **Unlabeled fails CLOSED** — an *unlabeled* task does **not** fall through to A on a
  stranger; it **refuses or routes to C** (doc 05 D-i). A is for *explicitly
  normal*-sensitivity work only.
- **Attested, never self-advertised.** For confidential routing the isolation/TEE
  class must be **proven by a verified quote**, not taken from the provider's
  self-advertised capability (doc 05 TC10/C-iii — accepting a self-advertised class
  *as if* attested is the selector bug that re-opens TC1).

The cost is honest: a confidential task with no attested provider in reach is
**blocked**, not run-degraded. That is the point.

### D3 — Secrets are never long-lived plaintext on an untrusted provider (FR-K2)

Credentials the worker needs are delivered by a mechanism whose exposure **matches
the provider's trust** — and on an untrusted provider an inspection of its disk/RAM
must not yield the authorizer's keys:

- **Trusted pool (A):** plaintext-for-the-task-only is acceptable (you own the box);
  even here the worker never holds a *standing root* credential — it holds the two
  task-scoped attenuating UCANs (ADR-E4 / ADR-fed-003 §D3), never the agent's root
  key (FR-C1).
- **Low-trust (B / cooperative):** **authority stays *off* the provider.** The
  authorizer holds the secret and the worker reaches back via the **privileged-op
  callback** (ssh-agent-style "sign/do this") — which is **intent-bound,
  rate-limited, budget-metered, and logged** so it cannot be turned into a signing or
  free-inference oracle (doc 05 X-3, formalized in ADR-E4). The provider gets the
  *use* of a capability for the task, never the *bytes* of a long-lived credential.
- **Confidential (C):** secrets are **sealed to the attestation** — released into the
  enclave only against a *verified* quote (sealed-to-attested-key, the key-release
  bound to the measured runtime), so they are plaintext only where the operator
  cannot read them.

This composes with WG-Fed's custody boundary verbatim (the root never leaves the
custodian; download confers no oracle, ADR-fed-003 §D1) — now extended across a host.

### D4 — Minimal context: default smallest, widen on need, **taint-tracked through `--after`** (FR-K3, X-2)

The context bundle is a **signed, BLAKE3-content-addressed, optionally-sealed
`ContextScope` slice** (`Clean < Task < Graph < Full`, the successor to today's
`.wg/` symlink; HQ7/FR-D1). Its policy:

- **Default to the smallest slice** that lets the task work (task T's input + the
  artifacts of its `--after` dependencies), **not** the whole graph or unrelated
  history. The blast radius of a curious provider is bounded by what it *must* see.
- **Widen only on demonstrated need** — a larger tier is a deliberate, surfaced step,
  not a default.
- **Sensitivity is taint-tracked through `--after` edges**, so an adversarial task
  author cannot mislabel an exfiltration task "normal": a task that depends on a
  `confidential` artifact **inherits** a sensitivity floor from it (doc 05 D-ii, X-2 —
  sensitivity is *inferred + labelled*, not solely self-asserted).
- **The slice-builder is the confidentiality TCB.** "Minimal" that silently pulls in
  a transitive secret is the X-2 residual; the bundle is **linted** and the applied
  scope is **surfaced** (`wg show <task>` / `wg providers`). Taint analysis is never
  *provably* complete — so a task whose *intent* is sensitive routes to **C/A, never
  B** regardless of how small its slice looks (X-2 mitigation).

Honest residual: a too-small slice makes the agent worse (the effectiveness tax, T6).
We pay it knowingly and per-task — minimization is a *blast-radius* control, and (D1)
**not** a confidentiality one.

### D5 — Ship the attestation **slot** early; the enclave later (FR-K4 — interface before silicon)

We **specify and ship the attestation interface now**, even though v1's payload
behind it is "trust the pool or refuse":

- **`src/providers/attest.rs`** carries the *interface*: an `Attestation` /
  `AttestationQuote` type, a `verify_quote(quote, expected_measurements, nonce, wgid)
  → AttestationVerdict` entry point, and the **seal-context-to-quote hook** the bundle
  builder calls when a task routes to C (`seal_to_attestation(bundle, verified_quote)`).
- **The attestation handshake is specified** — the authorizer verifies "this agent
  runs in an enclave I trust" **before** shipping confidential context (FR-K4): a
  fresh **nonce + the provider's `wgid:` are bound into the quoted user-data**
  (anti-relay, C-i mitigation), checked against an **expected-measurement allow-list**,
  *before any context ships* (the floor-before-context rule, X-1).
- **v1 payload = "trust or refuse."** Until Exec-Wave D ships a real enclave behind
  this slot, the verifier has an **empty (or single-test) allow-list**: every real
  confidential routing therefore **refuses, loudly** (the spark's step-6 assertion,
  memo §4.2). The slot proves *the loud degradation and the seam*, not the silicon.
- **Absence of attestation downgrades loudly** — a provider that advertises no
  attestation is simply **not eligible** for confidential work (D2); there is no
  silent fall-through.
- **Exec-Wave D fills the slot:** real `verify_quote`, the **curated measurement
  allow-list** (pinned + audited measurements, reproducible runtime builds), and
  attestation-bound lease renewals — treated as **defence-in-depth alongside** the
  eval-gate + cross-domain re-run, **never a sole oracle** (C-ii: a broken attestation
  is worse than none). We **design the slot and *use* enclave primitives; we build no
  enclave ourselves** (memo §7 non-goal 3).

### D6 — The per-trust-class confidentiality threat-model table (FR-K1) — *what the provider can/cannot see*

FR-K1 (MUST) is satisfied by this table — confidentiality is a **stated, bounded
property per trust level**, nothing "secret by accident." Each cell is the
provider's view of that context element at that tier:

| Context element | **A — trust** (own pool) | **B — minimize** (cooperative) | **C — attest** (TEE) |
|---|---|---|---|
| Task input | plaintext | **minimized slice, plaintext** | sealed; plaintext only in enclave |
| Graph slice | task + deps (or wider on a pool you own) | **smallest slice only** | smallest slice, sealed |
| Secrets / credentials | task-scoped UCANs; no standing root | **none on the box** — authorizer-held + callback (D3) | sealed-to-attestation; released into enclave only |
| Prior state | per the scope tier | minimized + ephemeral | sealed |
| Tool outputs | visible to the operator | visible to the operator | inside the enclave only |
| **In transit** | sealed (WG-Fed per-recipient) | sealed (WG-Fed per-recipient) | sealed (WG-Fed per-recipient) |
| **At rest** (FR-D3) | plaintext-for-task on a box you own | **minimized + ephemeral; do NOT rely on its deletion** | **never plaintext at rest** (encrypted-to-enclave) |

Two invariants the table encodes: **(1) in transit is *always* sealed** (WG-Fed
per-recipient X25519 + XChaCha20-Poly1305 — a relay/MITM sees ciphertext, tampering
is detected, FR-D2, no new crypto); **(2) confidentiality never relies on an
untrusted provider's goodwill to delete** (FR-D3) — C never sees plaintext, and B
sees only a minimized, ephemeral slice whose leakage is *bounded* (D1), not
*prevented*. The canonical graph stays at the authorizer (single-writer spine,
WG-Fed HQ7); the provider holds a slice and writes back deltas.

### D7 — Loud degradation is the governing invariant; attestation is never a sole oracle

The single rule that ties the tier together: **confidentiality degrades loudly,
never silently** (FR-K5/D-i). Concretely — unlabeled fails closed (D2); a confidential
task with no attested provider is *held with a reason*, not run-degraded (D2);
absence of attestation makes a provider ineligible, not silently-trusted (D5);
minimization is *named* as blast-radius-only, not sold as confidentiality (D1/D4); and
the applied `context{scope_tier, seal}` is **surfaced** (`wg show` / `wg providers`)
and **linted** (`wg config lint`, which already exists). And because a *broken*
attestation forges both confidentiality and integrity (C-ii), **C is layered with the
ADR-E3 verification levers** (eval-gate + cross-domain re-run where checkable), never
trusted as the only evidence. The failure direction is **always over-protection
(refuse), never under-protection (silent exposure).**

---

## Status

**Proposed.** Decided in the execution-federation decision memo
(`docs/execution-federation-study/06` §1/§2.2/§3 HQ1+HQ8/§6 ADR-E2 stub); this ADR
formalizes the decision and resolves the stub's three open questions (below). It is
one of four Exec-Wave A deliverables (ADR-E1 placement, ADR-E2 confidentiality,
ADR-E3 integrity, ADR-E4 capability/lease). **No execution code lands until all four
are Accepted** (memo §5). Depends on WG-Fed **ADR-001 (identity)**, **ADR-002
(transport/encryption)**, **ADR-003 (custody/UCAN)**, and **ADR-004 (loadable-state
safety)** Accepted — the substrate this ADR composes with and does not redefine.

---

## Consequences

- **New module surface** (memo §2.1, the Exec-Wave B skeleton): `src/providers/`
  gains **`bundle.rs`** (build/seal/verify the `ContextScope` slice over WG-Fed
  crypto, the D4 slice-builder = the confidentiality TCB) and **`attest.rs`** (the D5
  slot — `verify_quote`, `seal_to_attestation`, the handshake). The `leash()` engine
  in `placement.rs` emits `context{scope_tier, seal}` per D1/D2 and **cannot** emit a
  plaintext/minimized region for a `confidential` task (the fail-closed guarantee).
- **A sensitivity classifier feeds the leash** (D4) — sensitivity is inferred +
  taint-propagated through `--after`, with a labelling step and a human-in-loop
  boundary for ambiguous tasks (OQ2). This is new policy code, not just config.
- **Enables FR-K1–K5** (the confidential-context EX4 cluster) and the FR-D1–D3
  locality story. The **attestation slot exists from Exec-Wave B** even though the
  enclave behind it is Exec-Wave D — so confidential work is *a provider-capability
  away, not a redesign*.
- **The measurement allow-list becomes the confidentiality TCB** (C-i) once Exec-Wave
  D fills the slot — a continuous, security-critical curation job (OQ3), the largest
  verification op-surface in `WG-Exec`.
- **Spark coupling:** step 6 of the execution spark
  (`tests/smoke/scenarios/exec_spark_borrowed_box.sh`, memo §4.2/§4.4) is this ADR's
  executable assertion — a `confidential` task offered only to a non-attesting
  provider is **held/refused** (context never shipped), and an **unlabeled** task does
  **not** route to A-on-a-stranger. The spark proves the *slot + loud degradation*,
  not an enclave (memo §4.3).
- **Cost we take on knowingly:** the TEE tax *per confidential task* (vendor root
  C-iii + measurement curation C-i + side-channel residual C-iv); the effectiveness
  tax of minimization (T6); the largest config/test matrix among the four tiers
  (the leash surface, mirroring WG-Fed's C-1) and the discipline to keep defaults
  fail-closed.

---

## Alternatives rejected

- **B-as-confidentiality.** Routing mildly-sensitive work to B believing "minimized =
  confidential." Rejected: the provider reads the slice and **every token of it**; the
  residual "you cannot run confidential work on B" is **inherent, not a bug**
  (doc 05 §3.1). The honest naming (D1) and the C-or-refuse rule (D2) exist precisely
  to prevent this deployment error.
- **Silent run-degraded** on a provider that cannot meet the confidentiality bar.
  Rejected by FR-K5 — the failure direction must be *refuse loudly*, never *expose
  silently*. This is the whole point of the tier.
- **Fail-safe-to-A for unlabeled tasks.** Rejected: doc 05 D-i, Fatal-as-written — an
  unlabeled-but-confidential task silently exposed on a stranger. Unlabeled **fails
  closed** (D2).
- **Accepting a self-advertised isolation/attestation class as if attested** for
  confidential routing. Rejected: doc 05 TC10/C-iii — a stranger advertises "I'm a
  TEE" and reads your context. Confidential routing requires a **verified quote**,
  not a self-claim (D2/D5).
- **Attestation as a sole oracle.** Rejected: doc 05 C-ii — a *broken* attestation
  forges confidentiality *and* integrity at once, so it is **worse than none** if
  trusted alone. C is defence-in-depth alongside the ADR-E3 levers (D7).
- **Building our own TEE / enclave / attestation service.** Rejected: memo §7
  non-goal 3 / doc 03 §5 — we **design the slot** and *use* enclave primitives;
  implementing an enclave is out of scope and out of our competence to get right.
- **Homomorphic / compute-on-ciphertext execution** as the confidentiality answer.
  Rejected: out of reach (memo §7 non-goal 4); the only levers are trust,
  minimization, attested isolation — not running the model on encrypted data.
- **A second confidentiality crypto / identity scheme.** Rejected: NFR-4 — transit,
  sealing, keys, and delegation are all WG-Fed's; this ADR defines *policy*, not
  crypto.

---

## Open questions

The ADR-E2 stub (memo §6) and the hand-off checklist (§8 items 2–3) left **three**
questions for this ADR to close. All three are resolved as to **mechanism**; where a
residue is a genuine vendor / tuning / governance value judgment it is **explicitly
flagged for Erik** rather than silently fixed (the ADR-fed-003 convention).

### OQ1 — TEE vendor(s) + the diversity policy (the C-iii residual) — **RESOLVED (mechanism + recommended posture; exact vendor set flagged for Erik)**

**The question (memo §8 item 3).** Which TEE vendor(s) does the C tier trust, and
what is the diversity policy that bounds the **C-iii** vendor-root risk (a non-WG,
centralized, *compellable* trust root — Intel/AMD/AWS sign the quotes; a leaked or
compelled vendor key forges any quote)?

**Resolution — the mechanism is vendor-agnostic; the trust root is disclosed,
diversified, and monitored, never removed.**

- **`attest.rs` is multi-vendor by construction.** The `verify_quote` entry point
  resolves a **per-vendor verifier** (SGX/TDX, SEV-SNP, AWS Nitro, etc.) keyed off the
  quote's own vendor tag — the interface (D5) commits to *no single vendor*. Adding or
  retiring a vendor is an allow-list + verifier change, not a redesign.
- **The vendor root is `WG-Exec`'s most centralized dependency and is *disclosed* as
  such** (C-iii is *inherent-bounded*, not removable from a TEE). The mitigations are
  the only ones available: **(a)** support **≥ 2 independent vendors** so no single
  vendor key is a universal forgery root; **(b)** **monitor vendor key revocation**
  and fail closed on a revoked signing key; **(c)** bind a fresh **nonce + the
  provider `wgid:`** into the quoted user-data (anti-relay, C-i); **(d)** treat
  attestation as **defence-in-depth, never a sole oracle** (C-ii / D7), so even a
  forged quote is caught by the layered eval-gate / re-run on checkable work.
- **Diversity is also a placement *strength*, not only a hedge** — a distinct enclave
  on distinct hardware is far costlier to mint than a keypair, so C's attestation
  **doubles as sybil-resistance** for the B-overflow tier (memo §2.2). Vendor
  diversity therefore directly hardens both C and B.

*Flagged for Erik (vendor selection — a procurement / trust-surface value call):* the
**exact initial vendor set** (e.g. Intel TDX + AMD SEV-SNP, or lead with **AWS Nitro
Enclaves** for the cleanest attestation-API on-ramp), **how many independent vendors
are required before the C tier is declared production** (the recommendation is ≥ 2 to
avoid a single universal forgery root), and **whether to accept cloud-CSP-rooted
attestation** (Nitro/Azure) as well as silicon-vendor-rooted. The *mechanism*
(multi-vendor verifier, disclosed+diversified+monitored root, nonce+wgid binding,
attestation-as-defence-in-depth) is the ADR commitment; the vendor *choices* are
Erik's, and they can be set without reopening the design. This dependency is
correctly **deferred past Exec-Wave C** by the memo's guardrail (§5 — let the spark
and the A/B tiers inform the pick; the wire/slot is vendor-agnostic until then).

### OQ2 — The sensitivity classifier's auto-label threshold + the human-in-loop boundary (the D-ii requirement) — **RESOLVED (fail-closed mechanism; exact threshold a tunable, flagged)**

**The question (memo §8 item 2).** The leash is only as safe as the sensitivity label
that drives it. Sensitivity must be **inferred + labelled, not solely self-asserted**
(doc 05 D-ii) — so what is the classifier's **auto-label threshold**, and where is the
**human-in-loop boundary** for ambiguous tasks?

**Resolution — fail-closed by construction; ambiguity escalates, never relaxes.**

- **Inputs (taint-tracked, D4).** The classifier reads: explicit author label (a
  *floor*, never a ceiling — an author may raise but not lower below the inferred
  level), **taint inherited through `--after`** from the sensitivity of dependency
  artifacts, secret-touching signals (does the task's scope reference credentials /
  `wg secret` / a sealed artifact?), and the target's blast radius (foundational/root
  graph position ⇒ higher floor, mirroring the cross-task-poison constraint, ADR-E3
  D-iii / TC8).
- **Three-way verdict, fail-closed:** **`normal`** (eligible for A) only when the
  task is *explicitly and inferably* normal and clears every taint signal;
  **`confidential`** (⇒ C-or-refuse, D2) when any signal — label, taint, or
  secret-touch — trips; **`ambiguous` ⇒ treat as `confidential` AND route to the
  human-in-loop** (the fail-closed default — an unresolved label is **never**
  auto-routed to A, D-i). Taint-inference can only **escalate** a self-asserted label,
  never lower it (the D-ii / RA-9 fail-closed-routing rule WG-Review already applies to
  taint — `docs/ADR-content-safety-001` §RA-9).
- **The human-in-loop boundary is the *ambiguous* band**, not a confidence number the
  author controls: a task the classifier cannot confidently place as `normal` is
  **held for a human to confirm or relabel**, exactly as a non-accept review verdict
  blocks consumption (WG-Review). A human may *confirm normal* (clearing the hold) or
  *confirm confidential*; a human is **never** asked to *lower* a `confidential`
  inference to run on A without an explicit, logged override.

*Flagged for Erik (tuning — a precision/recall value call):* the **exact auto-label
threshold** (how aggressive the `normal` band is — a tighter band sends more tasks to
the human-in-loop and to C, a looser band risks the §3.1 deployment error), the
**precise signal weights** (which secret-touch / taint signals are hard-confidential
vs advisory), and **whether a solo-hobbyist profile may widen the `normal` band** (the
EX6 dial — a paranoid org tightens it, a hobbyist on a private pool may loosen it,
*but the fail-closed-on-unlabeled floor is never removable*). The *mechanism*
(infer-don't-self-assert, taint-only-escalates, ambiguous⇒confidential+human,
fail-closed-on-unlabeled) is the ADR commitment; the threshold is a per-deployment
default Erik sets. This is the memo §8-item-2 fork, closed here as to mechanism.

### OQ3 — The measurement-allow-list curation cadence + owner (the C-i residual) — **RESOLVED (mechanism + ownership model; cadence + named owner flagged for Erik)**

**The question (memo §8 item 3).** C's confidentiality+integrity TCB is the
**measurement allow-list, not the silicon** (C-i): attestation proves "*a* runtime
with measurement M ran," never "M is *safe*." A permissive list (version ranges,
vendor-default images, a debug-hooked runtime) attests a runtime that exfiltrates from
the inside. So **who curates the allow-list, and on what cadence?**

**Resolution — the allow-list is a *security-critical, pinned, audited, reproducible*
artifact with a single accountable owner; curation is a release-gated process, not an
ambient config edit.**

- **Pinned, never ranged.** The allow-list holds **exact, pinned measurements** of
  **reproducibly-built** runtime images — *never* version ranges, vendor-default
  images, or debug-hooked runtimes (the C-i failure modes). A measurement enters the
  list only with a **reproducible build recipe** so the pinned value is independently
  re-derivable.
- **Curation is a reviewed, audited, append-with-justification process** — adding or
  retiring a measurement is a **security-gated change** (a reviewed PR against a
  version-controlled allow-list, every entry carrying the build provenance + a
  justification), not a live mutable config knob. This mirrors the WG-Review verdict
  sigchain posture: the allow-list is itself **content-addressed and auditable**, so a
  silent permissive widening is visible.
- **Cadence is event-driven first, periodic second:** **immediate** on a runtime CVE /
  microcode patch / vendor key revocation (fail closed on anything dropped from the
  list — an enclave running a now-revoked measurement is rejected at the next
  attestation-bound lease renewal, D5); and a **scheduled re-audit** of the full list
  on a regular interval to retire stale measurements. Defence-in-depth (D7) means a
  curation lag degrades *gracefully* — a stale-but-not-malicious measurement is still
  backstopped by the eval-gate / cross-domain re-run on checkable work (C-ii).

*Flagged for Erik (governance — a security-ownership value call):* the **named owner**
(the recommendation is a **single accountable security owner / small rotating
security-review group**, not "whoever touches it" — this is the largest verification
op-surface in `WG-Exec` and the real C-i weak link), the **exact re-audit cadence**
(e.g. weekly automated CVE/revocation sweep + quarterly full re-audit), and **whether
the allow-list is per-deployment or a shared `WG-Exec` default** an org can pin or
override. The *mechanism* (pinned + reproducible + audited + content-addressed +
fail-closed-on-revocation, with event-driven cadence) is the ADR commitment;
**this work is correctly scheduled for Exec-Wave D** (the enclave wave, memo §5) — the
allow-list has no consumers until a real enclave fills the slot (D5) — but the
*ownership + cadence model* is fixed here so it is designed in, not discovered late.

---

## References

- `docs/execution-federation-study/06-decision-memo-and-roadmap.md` — §1 (the
  confidentiality-tier call, made plainly), §2.2 (D shipped A-first, C-slot-early),
  §2.3 (the WG-Fed compose contract), §3 **HQ1** (the crux — the decision this ADR
  formalizes), §3 HQ7/HQ8 (locality + isolation ladder), §3 HQ11 (the leash spine),
  §4.2/§4.3/§4.4 (the execution spark — step 6 is this ADR's executable assertion),
  §5 (Exec-Wave A/D sequencing + don't-build-yet guardrails), §6 (ADR-E2 stub), §7
  (non-goals), §8 (hand-off items 2–3, closed here).
- `docs/execution-federation-study/05-adversarial-evaluation.md` — §3.1 (the TC1
  confidentiality crux + the cross-candidate verdict), TC1/TC6/TC10 (the attack
  classes), **C-i…C-iv** (the C-tier residuals this ADR's open questions resolve),
  D-i/D-ii (the fail-closed selector + infer-don't-self-assert), X-1 (floor before
  context), X-2 (bundle over-inclusion), X-3 (callback-as-oracle).
- `docs/execution-federation-study/03-requirements-and-hard-questions.md` —
  **FR-K1–FR-K5** (the confidential-context MUSTs/SHOULDs), FR-D1–FR-D5 (locality +
  isolation), EX4 (the confidential-context crux), EX6 (trust-default / leash-as-dial),
  T6 (minimal-context vs effectiveness), T9 (verification-needs-evidence vs
  confidentiality-hides-it).
- `docs/ADR-fed-001-identity-key-model.md` — `wgid:` identity + keys the sealing uses.
- `docs/ADR-fed-002-transport.md` — the untrusted store-and-forward transport the
  sealed bundle rides.
- `docs/ADR-fed-003-custody-delegation-recovery.md` — §D1 (custodian-held root +
  ssh-agent boundary the secrets-off-the-provider callback inherits), §D2 (the
  leash-as-a-dial amendment this tier's `context` output is one face of), §D3 (the
  attenuating UCANs ADR-E4 formalizes for execution).
- `docs/ADR-fed-004-loadable-state-safety.md` — §D6 (loaded state is untrusted input;
  the sealed-state-at-rest + provenance posture this ADR's at-rest row composes with).
- `docs/ADR-content-safety-001-review-gate.md` — §RA-9 (taint-inference fail-closed
  routing the sensitivity classifier OQ2 reuses).
- **Sibling Exec-Wave A ADRs:** `docs/ADR-exec-e1-*` (placement — confidential routing
  is a placement constraint), `docs/ADR-exec-e3-*` (result integrity — the layered
  verification levers C is defence-in-depth *alongside*), `docs/ADR-exec-e4-*`
  (capability/lease — the two scoped UCANs + the privileged-op callback secrets ride).
