# ADR-E4 (WG-Exec): Result Integrity & the Verification Leash — Attribution + Trust-Proportional Re-run in a Trusted Domain vs a Pinned Spec

**Status:** Proposed
**Date:** 2026-06-26
**Decision:** A result from a borrowed provider is **signature-attributed** to the
agent (unsigned / wrong-signed ⇒ rejected) — but **attribution is not integrity**: the
provider holds the delegated signer and can sign its lie. Integrity therefore rides a
**trust-proportional verification leash** (one output dial of the `leash()` engine):
**trusted ⇒ attribution + the WG eval-gate**; **low-trust ⇒ a deterministic re-run in a
TRUSTED DOMAIN — authorizer-side or a *disjoint* trusted provider, NEVER the producing
provider — against the authorizer's *pinned* acceptance test (tests are spec, not the
provider's deliverable).** The bar is **equivalence, not byte-identity** (agent outputs
are nondeterministic). A forged result's blast radius is bounded by the **task-scoped
graph-write UCAN** (ADR-E3) and is **auditable + revocable** after the fact via
provenance. **Quorum is deferred to v2** (it needs sybil-resistance, unsolved).
**Random spot-checks** cover the fungible normal-sensitivity middle. **Cross-task poison
is a first-class placement constraint** — foundational/root tasks route to high-trust/
attested tiers, leaf tasks may use the verified-overflow tier, every artifact is
provenance-tracked, and a higher-trust task re-verifies inputs it inherits from a
lower-trust tier.

> **This ADR formalizes the co-crux.** It owns **HQ2 — "the provider can lie about what
> the agent did"** — the second of the two load-bearing cruxes the execution-federation
> decision memo names (`docs/execution-federation-study/06-decision-memo-and-roadmap.md`
> §3 HQ2; §6 stub). It is the execution-plane counterpart to WG-Fed's "download ≠
> impersonation": here the question is *"a borrowed provider produced a result — is it
> what the agent actually did, and is it correct?"*
>
> **A numbering note, so cross-references resolve.** The decision memo's §6 ADR *stubs*
> number this content **"ADR-E3" (result integrity)** and the capability/lease content
> **"ADR-E4."** The Exec-Wave A task graph **swapped** those two assignments so the file
> ordering groups placement→confidentiality→capability/lease→verification: this file is
> **`ADR-exec-e4-verification.md`** and the capability/lease ADR is
> **`ADR-exec-e3-capability-lease.md`**. The *content* mapping is unchanged — this ADR
> is the memo's "result integrity & the verification leash" decision. The acceptance
> brief (`ADR-exec-000-acceptance-brief.md`) indexes all four under the task-graph
> numbering.
>
> **It does not re-litigate the architecture.** `WG-Exec` = Candidate D's leash-selector,
> shipped A-first with C's attested tier as the confidential escape-hatch and B as a
> vouched-overflow cooperative — settled in the memo (§1–§2) and not reopened here. This
> ADR resolves the verification *mechanism* and the memo §6 stub's three open questions
> (spot-check rate; what is "checkable"; the test-authoring review gate).
>
> **It rests on the sibling ADRs and on WG-Fed.** Attribution is a signature against the
> agent's sigchain (**WG-Fed ADR-001**, `wgid:` + sigchain). The blast-radius bound is
> the **task-scoped graph-write UCAN + the epoch-fenced lease** (**ADR-E3**, itself
> WG-Fed ADR-003's attenuating UCAN). The verification *depth* is one output of the same
> `leash()` engine whose *context* output **ADR-E2** owns and whose *placement* **ADR-E1**
> owns. No second trust system is invented (NFR-4).

---

## Context

The memo's one-sentence framing of the co-crux: **the provider owns the worker
environment, so it can make the agent emit anything, or fabricate a `ResultEnvelope`
wholesale — what catches it?** (doc 03 §"HQ2"; doc 05 §3.2). This is the integrity twin
of the confidentiality crux (ADR-E2): both arise because federation places an agent on
compute the authorizer does not own.

The adversarial pass returned a sharp, uncomfortable verdict that this ADR is designed
*to*:

1. **Attribution proves *who claims*, not *correctness* (doc 05 §3.2, A-row).** The
   `ResultEnvelope` is signed by the *delegated act-as-agent signer*, which lives in the
   worker's address space **on the provider's box**. "Signed by agent G" proves only
   that the delegated key signed it — which a malicious operator can do for **any**
   content. **Attribution provides zero integrity against a provider that holds the
   signer.** What remains is the **WG eval-gate** (`auto_evaluate` / FLIP scoring output
   against `## Validation`), which is a **quality filter, not an integrity proof**: a
   forged diff that compiles and passes the listed tests but plants a backdoor, or a
   plausible-but-wrong research summary, sails through. So a low-trust result on
   attribution-plus-eval-gate alone is *acceptable iff the trust assumption holds, and
   catastrophic the moment it doesn't — with no in-band way to know it doesn't.*

2. **The re-run lever is correct only under two conditions doc 04 stated loosely
   (doc 05 X-5).** "Verify the **checkable artifact**, not the nondeterministic
   transcript" (doc 01 §4.2) is the right instinct, but a `cargo test` re-run is
   **theatre** unless (a) it runs in a **trusted domain** — authorizer-side or on a
   *disjoint* independently-trusted provider, never the same provider that produced the
   result — and (b) it runs against a **pinned spec**, not the provider's own shipped
   tests.

3. **Test-poisoning is a genuine gap (doc 05 X-6).** A WG code task's deliverable *is a
   diff that may modify the tests themselves.* A hostile provider returns a diff that
   edits both the implementation *and* the tests so `cargo test` passes — and the re-run
   **on the authorizer's host still passes**, because the poisoned oracle shipped in the
   diff. "Checkable" was checked against an oracle the attacker rewrote.

4. **The structural verification claim is narrower than it looks (doc 05 §3.2 D-row,
   P6).** "High-sensitivity or low-trust placement *always* applies the verification
   leash" protects the *sensitive minority* — but a **normal-sensitivity task on a
   reputation-`Verified` provider gets attribution-only accept**, and a patient
   *behave-then-defect* provider (P6) earns `Verified` on cheap work precisely to reach
   that path. The fungible **majority** is under-verified by default.

5. **The deepest threat is cross-task, not single-task (doc 05 §3.2 close, TC8/D-iii).**
   Every candidate's integrity story is *per-task*; the poison threat is *cross-task* —
   a check-passing forgery in task T launders through honest `--after` consumers. This
   is the study's **most under-defended attack**, and the only structural answer (route
   by graph position + provenance + cross-trust re-verification) exists **only in D**,
   which is the single strongest argument for the D convergence target.

The cross-candidate verdict the memo adopts: **C ≥ B(in-scope) > D(default) > A** on
single-task integrity, but the design must beat D's *default* exposure on the fungible
middle and must treat TC8 as a *requirement, not a nicety*. The requirements this ADR
satisfies are doc 03 §2.D **FR-V1…FR-V5** and FR-C5 (attribution).

---

## Decision

### D1 — Attribution is MANDATORY but is explicitly NOT integrity (FR-V1, FR-C5)

Every `ResultEnvelope` written back to the authorizer's graph MUST carry a signature
from the worker's **delegated act-as-agent signer**, chained to agent G's sigchain
(WG-Fed ADR-001). The write path **rejects** an unsigned result and a result whose
signature does not verify against G's delegated signer (FR-V1). `wg show <task>`
attributes the accepted result to agent G; the usage is not bare (FR-V3). Attribution
is what makes the graph record *who claims* an artifact even when the *compute* was
someone else's (FR-C5).

We state the limit of attribution **in the design, not in a footnote**: because the
delegated signer lives on the provider's box, **a valid signature proves origin-of-
claim, never correctness.** Attribution is a *necessary precondition* for every
downstream lever (you cannot revoke, provenance-track, or lower the trust of an
*unattributed* result), and it bounds *who* to blame — but it is never, on any tier,
treated as evidence that the work is *right*. This is the explicit rejection of the
seductive "it's signed by the agent, so trust it" error (doc 05 §3.2).

### D2 — The verification leash: a lever menu selected by trust (FR-V2, FR-V5)

Integrity is a **trust-proportional verification leash** — the `verification{depth}`
output of the one `leash(provider_trust, task_sensitivity, pool_class, env_config)`
engine (memo §2.1 HQ11; ADR-E1/E2 own the sibling outputs). The lever **menu**:

| Provider trust × task sensitivity | Verification lever (the floor) |
|---|---|
| `Verified` provider, `normal` sensitivity | **Attribution + the WG eval-gate** (`auto_evaluate` / FLIP vs `## Validation`) + **random spot-check** (D5) |
| `Provisional` / low-trust, **checkable** deliverable | Attribution + **deterministic re-run in a trusted domain vs a pinned spec** (D3) |
| `Provisional` / low-trust, **non-checkable** deliverable | Attribution + eval-gate + **escalate** (route to A/C, or human/second-reviewer); **never** "re-run says nothing, so accept" |
| `high` sensitivity OR foundational graph position | The verification lever **always** applies, **regardless of accrued reputation** (D5/D6) |
| Confidential | Verified inside the attested tier (ADR-E2); attestation is defence-in-depth **alongside** a check, never a sole oracle |

The leash **tightens monotonically under suspicion and never loosens itself**: trust
buys *cheaper verification on fungible work*, never *unverified acceptance on work that
matters* (FR-V5; the X-7 structural bound). Verification cost is therefore proportional
to trust (FR-V5) — a trusted-pool result is not needlessly re-run; a low-trust result is,
per a stated policy. The applied verification depth is **surfaced** in
`wg show <task>` / `wg providers` (mirroring the leash-surfacing of ADR-E1/E2) so an
under-verified acceptance is visible at a glance, and a **leash lint** flags a config
that would accept a low-trust checkable result on attribution alone.

### D3 — The re-run lever's two non-negotiable invariants: trusted domain + pinned spec

The low-trust integrity lever is a **deterministic re-run of the checkable artifact**,
governed by two invariants that are **load-bearing, not advisory** (doc 05 X-5/X-6):

1. **Trusted domain (X-5).** The re-run executes **authorizer-side, or on a disjoint
   independently-trusted provider Q ≠ P** — **never on the provider P that produced the
   result.** A re-run on the producing provider is theatre: P controls the runtime and
   can make any check pass. `verify.rs` pins the *trust-domain of the re-runner* as an
   explicit field; a re-run scheduled back onto the producer is a **bug the engine
   refuses**, not a configuration.

2. **Pinned spec, not the provider's deliverable (X-6).** The acceptance test is part of
   the **authorizer's trusted spec**, held at the authorizer, **not** taken from the
   returned diff. The re-run runs the authorizer's *pinned* test suite against the
   provider's *implementation*. Any **test-file change inside the returned diff is split
   out and routed to the test-authoring review gate (D7 / OQ3)** before it can become
   part of any future pinned spec — it is **never** auto-trusted as the oracle for its
   own diff.

**Equivalence, not byte-identity.** Agent outputs are nondeterministic (different valid
diffs, different prose), so the re-run does **not** demand the producer's exact bytes.
The accept predicate is *equivalence at the spec level*: the authorizer's pinned tests
pass against the artifact, the eval-gate agrees, or a semantic check holds (the exact
"equivalent-not-identical" comparison for non-test artifacts is OQ2). This is the
difference between "verify the checkable artifact" and the impossible "reproduce the
nondeterministic transcript."

The cost is stated plainly (T2, the integrity-vs-cost tension): the authorizer must
**hold compute to re-run** — it cannot fully offload integrity — and a cross-provider
re-run is **N×**. We pay it *only* on the low-trust / high-stakes rows, never globally.

### D4 — Forged-result blast radius is bounded, audited, and revocable (FR-V3, FR-V4)

Even a fully-believed forged result is **blast-radius-bounded**: the worst a forging
provider can do is corrupt **its own task's** output, because the worker holds only the
**task-scoped graph-write UCAN** (ADR-E3 / WG-Fed ADR-003 — log/append/artifact/done on
task T only, *never* blanket graph write) and the **epoch-fenced lease** prevents a
stale/partitioned worker from committing at all (ADR-E3, the atomic CAS at the canonical
write boundary). A forged result cannot mutate the rest of the graph or impersonate
another agent (FR-V4).

To make a forgery **auditable and revocable after the fact** (NFR-7), the
`ResultEnvelope` carries (FR-V3): the **work product** (diff/artifacts), **token/cost
usage** (the eval-gate / re-runner's evidence), optionally a transcript/attestation, and
**provenance** — *which provider produced this artifact*. When a provider is later
discovered to be bad, provenance lets the authorizer **find every descendant that
consumed the poison and re-run it** (D6). This is the integrity guarantee's real shape:
**bound + audit + revoke**, not "detect every forgery up front" — perfect single-shot
detection of a semantic poison is not claimed.

### D5 — Random spot-checks cover the fungible middle (P6 behave-then-defect)

The structural leash protects the sensitive minority; the **fungible normal-sensitivity
majority on a reputation-`Verified` provider** is the residual (doc 05 §3.2 D-row /
X-7 — the behave-then-defect path P6 is built to reach). Defence-in-depth: the
authorizer runs **random spot-check re-runs even on trusted providers**, at a sampled
rate (the rate is OQ1). A spot-check is a full trusted-domain-vs-pinned-spec re-run (D3)
on a randomly-selected fraction of otherwise-attribution-accepted results. A provider
that has been behaving to earn cheap acceptance **cannot predict which result is
sampled**, so a single defection carries an expected-detection cost; a caught defection
**lowers the provider's trust** (so its next item takes the deep path) and triggers the
revoke leg (D4). Spot-checking is what keeps "reputation buys cheaper verification" from
degrading into "reputation buys *unchecked* acceptance."

### D6 — Cross-task poison is a FIRST-CLASS placement constraint (TC8 / D-iii)

The study's most under-defended attack — **a check-passing poison in task T that
launders through honest `--after` consumers** — is defended **structurally**, because
only D's whole-graph selector can (doc 05 D-iii; memo §1 commitment 3). Three rules,
enforced by placement and verification together:

1. **Tier-by-graph-position.** Route **foundational/root** tasks — whose poison would
   propagate widest — to **high-trust (A) or attested (C)** tiers; reserve the
   **verified-overflow (B)** tier for **leaf** tasks whose output nothing depends on.
   The selector reasons about graph *topology*, not just per-task sensitivity. (This is
   a placement input ADR-E1 consumes; this ADR owns the *requirement* and the
   verification half.)

2. **Provenance on every artifact (D4).** Every `ResultEnvelope` records which provider
   produced it, so a later-discovered bad provider yields its full poisoned-descendant
   set for re-run (NFR-7 auditability).

3. **Re-verify inputs across trust boundaries.** A downstream task running on a *higher*
   trust tier **re-checks the inputs it inherits from a *lower* tier** rather than
   assuming them — N×-on-the-critical-path re-verification for high-stakes graphs.
   Per-task verification is *blind* to cross-task poison; this rule is what makes the
   graph, not just the task, the unit of integrity.

This is treated as a **requirement, not a nicety** (doc 05 §6 item 5): the cost (the
selector must reason about topology; critical-path re-verification is N×) is accepted on
the foundational/high-stakes rows.

### D7 — Quorum is deferred to v2; the v1 low-trust lever is a single disjoint re-run

`WG-Exec` v1's low-trust integrity lever is a **single disjoint trusted-domain re-run**
(D3), **not** N-of-M quorum. Quorum assumes an honest majority, which a **sybil cartel**
(P4 ∧ P5 — M cheap `wgid:` providers, one operator, all returning the *same* forged
artifact) defeats: quorum then agrees on the lie (doc 05 §3.2 Hole 3; B-i,
Fatal-for-the-open-market). Permissionless sybil-resistance is the unsolved problem, so
**quorum waits on it (v2)**. The path to a *safe* quorum is already named: gate it to
**vouched/attested providers** (provider diversity — distinct operators/networks/
hardware — plus enrollment cost), and note the elegant corollary that **C's attestation
doubles as sybil-resistance** (a distinct enclave proves distinct hardware, far costlier
to mint than a keypair). Until then, v1 ships the single disjoint re-run, which needs no
honest-majority assumption.

Tasks that legitimately **author tests** are not auto-trusted (the X-6 corollary): their
new tests are reviewed **as spec, not as deliverable**, through the gate in OQ3 before
they can become part of any pinned acceptance suite.

---

## Status

**Proposed.** Decided in the execution-federation decision memo (§3 HQ2, §6 "result
integrity" stub) and formalized here. **No execution code lands until ADR-E1…E4 are
Accepted** (memo §5, Exec-Wave A). This ADR's mechanism is exercised end-to-end by the
execution spark (`tests/smoke/scenarios/exec_spark_borrowed_box.sh`, memo §4) — step 3
(signed result accepted; unsigned/wrong-signed rejected) and step 5 (the hostile-
provider integrity check: attribution alone does **not** accept; a disjoint re-run vs the
*pinned* spec catches a corrupted + test-poisoned result; provenance records the bad
producer). It depends on **WG-Fed ADR-001 (attribution root)** and **ADR-E3 /
WG-Fed ADR-003 (the blast-radius-bounding write UCAN + epoch fence)** being Accepted.

---

## Consequences

- **New `src/providers/verify.rs`** — the lever menu (attribution check → eval-gate →
  trusted-domain re-run → spot-check), selected by `leash().verification`. Pins the
  **trust-domain of each re-runner** as an explicit field (X-5) and **splits test-file
  changes out of the deliverable** for the review gate (X-6).
- **`ResultEnvelope` gains evidence + provenance fields** (FR-V3): work product,
  token/cost usage, optional transcript/attestation, and the producing-provider
  provenance that powers the audit/revoke leg (D4) and cross-task re-run (D6).
- **The graph write-path enforces attribution** (FR-V1): unsigned / wrong-signed
  `ResultEnvelope` is rejected at the boundary, alongside ADR-E3's epoch CAS.
- **The eval-gate (`auto_evaluate` / FLIP) is reused, not rebuilt** — it is the trusted
  tier's quality lever and one input to the low-trust re-run's accept predicate.
- **A test-authoring review gate** is added (OQ3) — the one real workflow constraint
  this ADR imposes ("tests are spec").
- **The applied verification depth is surfaced** (`wg show` / `wg providers`) and a
  **leash lint** rides `wg config lint`, so an under-verified acceptance is legible.
- **The selector consumes graph topology** (D6): foundational ⇒ A/C, leaf ⇒ B — a new
  placement input ADR-E1 reads.
- **Cost accepted:** N× compute for re-run (the authorizer holds it, cannot fully
  offload — T2); spot-checks at N×-on-a-sample; some friction on legitimate test
  authoring; the selector reasons about topology, not just per-task sensitivity.

---

## Alternatives rejected

- **Attribution-only acceptance on low-trust.** A signer-holding provider forges freely;
  A's integrity reduces to the eval-gate, "a quality filter, not an integrity proof"
  (doc 05 §3.2). Rejected — attribution is necessary but never sufficient (D1).
- **Same-provider re-run.** "Verify by re-running on the box that produced it" is
  **theatre** — the producer controls the runtime (X-5). Rejected; the re-run is
  authorizer-side or on a disjoint trusted provider (D3).
- **Auto-trusting the provider's shipped tests as the oracle.** Test-poisoning: the diff
  rewrites its own acceptance test and the re-run passes against the poisoned oracle
  (X-6). Rejected; tests are spec, pinned at the authorizer, and test-file changes are
  reviewed (D3/D7/OQ3).
- **Quorum (N-of-M) on an open pool in v1.** A sybil cartel returns the same forged
  artifact and quorum agrees on the lie (B-i, Fatal-for-the-open-market). Deferred to
  v2, gated to vouched/attested providers (D7).
- **Byte-identity reproduction as the integrity test.** Agent outputs are
  nondeterministic; demanding identical bytes is impossible and not what integrity
  needs. Rejected for equivalence-at-the-spec-level (D3).
- **Per-task verification as the whole story.** Blind to cross-task poison (TC8). Rejected
  for the first-class placement constraint + provenance + cross-trust re-verification
  (D6).
- **"Trust the attestation" as a sole oracle** (forward-looking, C-tier). A broken
  attestation forges integrity *and* confidentiality at once (doc 05 §3.2 C-row / C-ii);
  attestation is defence-in-depth **alongside** the eval-gate and a check, never instead
  of them.

---

## Open questions

The three implementation forks the memo §6 stub hands this ADR, **resolved** with a
proposed default; the residual *value calls* are explicitly **flagged for Erik**.

### OQ1 — Spot-check sample rate on trusted providers — **RESOLVED (mechanism + default; the exact rate is Erik's dial)**

**Resolution.** Spot-checking (D5) is **rate-driven and config-tunable per deployment**,
expressed as a probability `p` that an otherwise-attribution-accepted
`Verified`-provider result is sampled for a full trusted-domain-vs-pinned-spec re-run.
The mechanism is fixed: **unpredictable selection** (the provider cannot know which
result is sampled), a caught defection **lowers trust + triggers the revoke leg** (D4),
and `p` **floors upward** for a provider with any prior caught defection or a thinner
trust history (it is never a flat global constant that a patient P6 can amortize). The
rate is **monotonic with stakes**: higher for results that feed many `--after` consumers
(it composes with D6's topology view).

**Proposed default:** `p ≈ 0.05` (one in twenty) for a long-history `Verified` provider
on leaf/normal work, rising toward `p ≈ 0.15–0.25` for a short-history provider or a
result with many descendants; `p = 1.0` (always) is the floor for high-sensitivity or
foundational tasks (those never ride attribution-only, D2/D6). The deployment dial
ranges from `0` (a solo hobbyist's fully-trusted home pool, accepting the P6 residual
knowingly) to aggressive sampling (a paranoid org).

**Flag for Erik (a value call, not a mechanism call):** the *default* `p` is a
cost/assurance trade — every spot-check is an N× re-run on work that was going to be
accepted. `0.05` is a guess at "cheap enough to always leave on, dense enough to make
defection expected-costly." Erik should set the **shipped default** and whether it is
**on by default** for the trusted-pool case (my recommendation: **on, at the low
default**, because an off-by-default spot-check is exactly the silent-A-weakness the
memo's commitment 3 exists to prevent).

### OQ2 — What counts as "checkable" for the re-run lever (vs eval-gate-only) — **RESOLVED (a typed checkability classifier; the boundary cases are flagged)**

**Resolution.** "Checkable" means the deliverable has a **deterministic, authorizer-held
oracle** that a trusted-domain re-run can evaluate to a pass/fail **independent of the
provider's bytes** (D3). The leash routes by a **checkability class** carried on the
task, inferred + labelled (not solely self-asserted, mirroring the D-ii sensitivity
rule):

- **Checkable (deterministic oracle ⇒ re-run is the lever):** code changes backed by a
  **pinned authorizer test suite**; build/lint/typecheck gates; schema/format
  conformance; anything with a reproducible pass/fail the authorizer can run against its
  own pinned spec. These get the full D3 re-run on the low-trust row.
- **Semi-checkable (a weaker oracle ⇒ re-run *plus* a semantic/second-reviewer check):**
  a refactor with partial test coverage, a doc with checkable claims (links resolve,
  code blocks compile), a data transform with invariants. The re-run covers what it can;
  the eval-gate + a semantic equivalence check (OQ2's deferred sub-question, below)
  covers the rest.
- **Non-checkable (no deterministic oracle ⇒ re-run does NOT apply):** a design doc, a
  research summary, a judgment call, a refactor with no test. Per D2, a low-trust
  non-checkable result is **never** accepted on "the re-run found nothing" — it
  **escalates**: route the task to A/C (a higher-trust tier) up front, or require a
  human / disjoint second reviewer. This closes doc 05 §3.2 Hole 1 ("B is strong for
  test-backed code and weak for everything else") by **never letting the weak case
  masquerade as verified.**

**Flag for Erik (the genuinely hard sub-question):** the **semantic "equivalent-not-
identical" comparator** for the *semi-checkable* middle (does this non-test artifact
satisfy the spec without byte-matching the producer?) is the deep open problem — it is
where an LLM-judge equivalence check would live, and an LLM judge is itself attackable
(it is the WG-Review surface). My recommendation: **v1 treats the semi-checkable class
conservatively as non-checkable** (escalate rather than trust a fuzzy semantic judge for
*integrity*), and a real semantic comparator is a v2 item co-designed with WG-Review.
Erik should confirm the conservative v1 default vs. accepting an LLM-judge equivalence
check earlier.

### OQ3 — The review gate for tasks that legitimately author tests (test-poisoning) — **RESOLVED (tests-are-spec gate; the human-vs-agent reviewer is Erik's call)**

**Resolution.** A task may legitimately add or change tests, but a provider-authored
test **must never become its own acceptance oracle** (X-6). The gate (D3/D7):

1. **Split.** On every returned diff, the **test-file changes are separated** from the
   implementation changes before any re-run (a mechanical diff partition — `verify.rs`).
2. **Re-run against the *old* pinned spec.** The implementation is re-run against the
   authorizer's **pre-existing** pinned suite (the test changes are **excluded** from
   that oracle), so a poisoned test cannot launder the implementation.
3. **Review the new tests *as spec*.** The proposed test changes are routed to a
   **review gate** and treated as a **specification change**, not a deliverable. Only
   *after* the new tests are reviewed-and-accepted do they become part of the pinned
   suite for **future** tasks — never auto-trusted for their own diff.
4. **Trust-proportional reviewer.** The reviewer is selected by the same leash:
   higher-trust author + lower-stakes test change ⇒ a lighter review (eval-gate /
   second-agent reviewer under WG-Review's no-privileged-scope bound); lower-trust
   author or a test guarding a security-sensitive path ⇒ **human-in-the-loop**.

This composes with **WG-Review** (the inbound-content review gate, `wg review`): a
test-file change is an **IC2 artifact** on the accept path, and the test-authoring gate
is a depth-tuned instance of WG-Review's trust-proportional review — no parallel review
system is invented.

**Flag for Erik (the policy call):** **which test changes mandate a *human* reviewer vs.
an agent/eval-gate reviewer.** "Tests are spec" imposes real friction on legitimate
test authoring (doc 05 X-6 cost), and the friction/assurance balance is a deployment
value: a strict shop human-reviews every test change; a fast-moving solo accepts an
agent-reviewer for `Verified`-author low-stakes test edits and reserves humans for
security-path tests. My recommendation: **default to agent/eval-gate review for
`Verified`-author non-security test changes, human review for low-trust authors and
security-sensitive test paths**, with the threshold as a `wg config` dial. Erik should
set the shipped default.

---

## References

- `docs/execution-federation-study/06-decision-memo-and-roadmap.md` — §1 (commitment 2:
  re-run in a trusted domain vs a pinned spec; commitment 3: cross-task poison as a
  first-class placement constraint), §2.2 (the verification row of the `WG-Exec` config
  table), **§3 HQ2** (the result-integrity decision this ADR formalizes), §3 HQ4 (the
  behave-then-defect / reputation-structural handling), §6 ("result integrity" ADR stub
  — task-graph numbered E4), §7 non-goals 1–2 (open market + quorum deferred), §8 item 4
  (the open-question hand-off: equivalent-not-identical + spot-check rate).
- `docs/execution-federation-study/05-adversarial-evaluation.md` — **§3.2** (the
  integrity crux deep-dive, the cross-candidate verdict C ≥ B(in-scope) > D(default) > A),
  **X-5** (re-run must run in a trusted domain — never same-provider), **X-6**
  (test-poisoning — tests are spec, the genuine gap), **X-7** (reputation poisoning
  bounded by always-verify-sensitive; the fungible-middle residual ⇒ spot-checks),
  **D-iii** (cross-task poison / tier-by-graph-position + provenance — the most valuable
  new recommendation), P6 (behave-then-defect), TC2/TC8 (the threat classes).
- `docs/execution-federation-study/03-requirements-and-hard-questions.md` — **HQ2**
  (result integrity, the co-crux), EX3 (verifiable results), **FR-V1** (attribution;
  unsigned/wrong-signed rejected), **FR-V2** (the verification-lever menu by trust),
  **FR-V3** (results carry evidence — not a bare "done" bit), **FR-V4** (forged-result
  blast radius bounded by the scoped write capability; revocable/auditable), **FR-V5**
  (verification cost proportional to trust), FR-C5 (writes attributable to the agent),
  T2 (the integrity-vs-cost tension), NFR-7 (auditability).
- `docs/execution-federation-study/04-candidate-architectures.md` — §1.2 (the
  `ResultEnvelope` wire), §1.6 (the `leash()` engine whose `verification{depth}` output
  this ADR owns), §1.7 (`src/providers/verify.rs`).
- `docs/ADR-fed-001-identity-key-model.md` — `wgid:` + sigchain (the attribution root
  D1 verifies against).
- `docs/ADR-fed-003-custody-delegation-recovery.md` — the attenuating UCAN that ADR-E3
  scopes to task-T graph-write, bounding forged-result blast radius (D4).
- `docs/ADR-exec-e1-placement.md` — placement consumes the tier-by-graph-position input
  (D6). `docs/ADR-exec-e2-confidentiality.md` — the `context` output of the same leash;
  attestation as defence-in-depth alongside a check. `docs/ADR-exec-e3-capability-lease.md`
  — the task-scoped write UCAN + epoch-fenced lease that bound blast radius and prevent
  double-commit (D4).
- `docs/ADR-content-safety-001-review-gate.md` — WG-Review, the IC2 artifact review path
  the test-authoring gate (OQ3) composes with (no parallel review system, NFR-4).
