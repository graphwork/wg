# Execution Federation Study 6/6 — Decision Memo, Execution Spark Test & Roadmap

> **Execution-federation study, wave 1, task 6 of 6 (the *decide* phase).**
> This memo synthesizes docs 01–05 (`01-prior-art-landscape.md` …
> `05-adversarial-evaluation.md`) into **the** execution-federation decision: one
> recommended architecture, an explicit call on every doc-03 hard question, the
> minimal end-to-end **execution spark test**, a phased roadmap, ADR stubs, and
> v1 non-goals. It is written to be a decision doc Erik can act on.
>
> **It composes with — and does not contradict — the WG-Fed decision memo
> (`docs/federation-study/06-decision-memo-and-roadmap.md`).** WG-Fed decides the
> *identity + capability substrate* (keys, sigchain, UCAN delegation, transport,
> encryption-as-ACL); this memo decides the *execution plane* that runs **on top of
> it**. Where they touch — custody, UCAN, the "central is a hint, never a root"
> invariant, the compat-handshake convention — this memo *inherits* WG-Fed's
> decision verbatim and invents no second system (doc 03 §5 non-goal 1; NFR-4).

**Status:** decision · **Date:** 2026-06-25 · **Owner task:** `exec-decision`
**Inputs:** `04-candidate-architectures.md` (the candidates A/B/C/D + the shared
substrate) · `05-adversarial-evaluation.md` (the threat model, failure-mode
register, and defended ranking) · `03-requirements-and-hard-questions.md` (the 12
HQs + acceptance checklist) · `02-current-state-baseline.md` (the code seams) ·
`01-prior-art-landscape.md` (the prior-art breaks) · **`docs/federation-study/06`**
(the WG-Fed decision this memo rests on). **This is the terminal doc of wave 1.**

> **A naming caution up front.** Both studies label their candidates **A/B/C/D**,
> but they are *different* candidates. WG-Fed's A/B/C/D are *identity models*
> (node-less P2P / node / key-rooted hybrid / DID-anchored); **this study's A/B/C/D
> are *execution-plane models*** (trusted private pool / capability-gated market /
> confidential-compute TEE / hybrid leash-selector). WG-Fed ranks its candidates
> **B > C > A > D**; this study ranks its candidates **A > D > B > C** (doc 05 §7).
> The letters collide; the meanings do not. Everywhere below, A/B/C/D mean the
> *execution* candidates unless prefixed "WG-Fed."

---

## 0. How to read this document

The decision is **not** "pick one of A/B/C/D and discard the rest." Doc 05's
defended finding is that the four candidates are *four operating points of one
leash policy* (doc 04 §1.6), and that the only arrangement with **no unbounded
Fatal finding** is a *phased convergence*: ship the proven trusted tier first, keep
the leash engine fail-closed from day one so confidential work is never silently
exposed, graft the confidential escape-hatch's *interface* early, and add the
verified-overflow tier gated to vouched providers. This memo adopts that
arrangement and names it **`WG-Exec`**.

- **§1 — The decision, in one page.** The architecture, the two plain calls
  (confidentiality-tier, placement-authority), and the headline defense vs the
  alternatives. Read first.
- **§2 — The recommended architecture (`WG-Exec`).** The fixed substrate (§2.1),
  the configured choices and why D-not-the-rest (§2.2), and the two load-bearing
  calls stated plainly (§2.3).
- **§3 — Decision register.** Every doc-03 hard question HQ1–HQ12, decided, with
  *why · rejected (cite doc 05) · cost accepted*. Nothing deferred silently.
- **§4 — The execution spark test** — "One task, a borrowed box, a scoped leash."
  The minimal end-to-end proof, six falsifiable steps, composing with the WG-Fed
  spark; the Exec-Spark-PoC milestone gate.
- **§5 — Phased roadmap**, sequenced *after* the WG-Fed waves, with don't-build-yet
  guardrails.
- **§6 — ADR stubs** for the four load-bearing decisions. · **§7 — Non-goals for
  v1.** · **§8 — Open questions handed to the ADR wave.** · **§9 — Validation
  checklist (this document).**

---

## 1. The decision, in one page

**We will build `WG-Exec`: a per-authorizer execution plane that places WG
agent-tasks onto separately-owned providers under *one leash policy* whose default
is a trusted private pool, whose confidential escape-hatch is an attested TEE, and
whose verified-overflow tier is a vouched cooperative — with the leash engine
*fail-closed* so confidential context is never silently exposed, and every
cross-host capability carried by a *scoped, attenuating UCAN* (never the agent's
root key) on the WG-Fed identity substrate.**

In the study's vocabulary: **the recommended architecture is Candidate D (the
hybrid leash-selector), shipped *A-first* (A is D with only the trusted tier
enabled), with Candidate C's *attested tier* grafted as the confidential
escape-hatch — its *slot* shipped before any enclave — and Candidate B's *verified
tier* added as a vouched-overflow cooperative, never an open market.** This is
exactly the phased synthesis doc 04 §6.3 floated and doc 05 §7.1 defended
adversarially — adopted here as the decision. It is the structural mirror of
WG-Fed's own move (*"ship B's node, keep the self-certifying core as the root,
graft D's UCAN, preserve A as the option"*): **ship A's trusted pool, keep the
leash engine as the guardrail, graft C's attested tier early, add B as overflow —
the only arrangement in which no Fatal finding remains unbounded.**

**The two calls the brief demands be made *plainly*:**

> **The confidentiality-tier call.** Confidentiality on compute you do not own is
> not free, and we will not pretend it is. There are exactly three levers, along
> the leash dial: **trust** (A — the provider sees plaintext; acceptable *only*
> because you own/trust the box), **minimize** (B — ship the smallest slice; this
> bounds blast radius but is **NOT** confidentiality: a B provider reads its slice
> and every token), and **attest** (C — a TEE the operator runs but provably cannot
> read). **Only C defends context against a provider you do not trust** (doc 05
> §3.1). Therefore: **a task that requires confidentiality routes to an *attested*
> provider (C) or is *refused* — never to A, never to B.** We ship C's *slot* (the
> attestation handshake + seal-to-quote hook) early so confidential work is a
> provider-capability away rather than a redesign; v1's payload behind the slot is
> "trust the pool, or refuse, loudly." We pay C's price — a vendor trust root,
> ongoing measurement-allow-list curation, an irreducible side-channel residual
> (doc 05 C-i/C-iii/C-iv) — *only per confidential task*, never globally.

> **The placement-authority call.** Placement authority is **per-authorizer; there
> is no central scheduler, no mandatory directory, no global queue in the
> correctness path** (HQ10, mirroring WG-Fed HQ6). Each WG schedules its own tasks
> onto providers it knows, exactly as today's dispatcher does. The default is
> **push** (the authorizer assigns); **pull** (a provider claims capability-gated,
> authorized work from the authorizer's own ready queue) is a first-class option.
> A provider directory or reputation gossip **MAY** be central *convenience* — but
> every central component is a *hint that can only help, never override* the local
> trust + capability + signature check. Lose it and reach degrades; correctness
> does not. The private-pool case works with **zero** central nodes (NFR-6).

**Why D, defended against the alternatives (all citations doc 05):** doc 05 ranks
the whole architectures **A > D > B > C** for a security/reliability gate, and its
§7.1 frames the result not as "pick A" but as "**ship A, keep the leash engine
fail-closed, graft C's attested tier early, add B as the vouched-overflow tier**."
We adopt that. We reject **A *as the terminal architecture*** (it *assumes the
malicious provider away* — A-i, Fatal-if-misused — and so can never run confidential
or low-trust work; but A is *D with one tier enabled*, so we ship it first and
converge through it, not instead of it). We reject **B as an *open market*** (B-i:
free `wgid:` keys defeat quorum + the squat-penalty + reputation simultaneously,
Fatal-for-the-open-market, fixable only by *closing* the market into a cooperative —
so B survives as a *vouched* tier, not a market). We reject **C *as the everyday
architecture*** (last as a whole: least mature/decentralized/simple, and a single
attestation break forges confidentiality *and* integrity at once — C-ii) **while
harvesting C's attested tier** — the only answer to the confidentiality crux and
"first among components" (doc 05 §7) — as D's escape-hatch.

**Three things we commit to engineering regardless of any later re-litigation** —
the load-bearing findings doc 05 §8 hands us:

1. **The leash default is *fail-closed*** (doc 05 D-i, Fatal-as-written): an
   *unlabeled* task routes to **refuse-or-C**, never to A. A is for *explicitly
   normal*-sensitivity work only. This single rule is the difference between
   `WG-Exec` being the safest arrangement and a silent-exposure machine.
2. **Verification re-runs in a *trusted domain* against a *pinned* spec** (doc 05
   X-5/X-6): never on the same provider that produced the result, never against the
   provider's own shipped tests. Tests are *spec*, not deliverable.
3. **Cross-task poison is a first-class placement constraint** (doc 05 D-iii, TC8 —
   the study's most under-defended attack): foundational/root tasks route to
   high-trust (A) or attested (C) tiers, leaf tasks may use B; every artifact is
   provenance-tracked; a higher-trust task re-verifies inputs from a lower-trust
   tier. Only D's whole-graph selector can do this — the single strongest argument
   for the D convergence target.

---

## 2. The recommended architecture — `WG-Exec`

### 2.1 What it is (the substrate, fixed)

`WG-Exec` adopts doc 04 §1's shared substrate wholesale — that substrate is *not* a
candidate, it is the agreed foundation under all four, and the adversarial pass
broke none of it unboundedly (doc 05 §2: every substrate finding X-1…X-7 is
Mitigable). Concretely, and reused from WG-Fed where the row says so:

- **One trust dial** (doc 04 §1.1): authorizer / provider / worker are all
  `wgid:<pubkey>` identities (WG-Fed identity, FR-R1); `Agent.trust_level`
  (Verified / Provisional / Unknown) is **the single dial** the whole plane reads —
  it gates placement, leash tightness, context exposure, verification depth, and
  lease term (FR-R2). This study *reads* trust; WG-Fed *defines* it.
- **A versioned execution wire** (doc 04 §1.2): five self-describing envelopes —
  `PlacementOffer`, `Claim`, `RunGrant`, `LeaseRenewal`, `ResultEnvelope` — carry
  placement/lease/capability/result across the authorizer↔provider boundary, behind
  a `WG_EXEC_COMPAT_VERSION` handshake that is **authenticated** (negotiated params
  signed, not merely exchanged — the WG-Fed S-7 lesson) with an enforced **minimum
  floor** (min isolation / `alg` / must-encrypt) and **loud-fail** on mismatch.
  These envelopes are *this study's* to version; identity/delegation/crypto formats
  are **inherited from WG-Fed** (the boundary is explicit, NFR-4).
- **Two scoped UCANs** (doc 04 §1.3, WG-Fed HQ11): the worker receives an
  *act-as-agent UCAN* ("run task T as agent G," + expiry) and a *graph-write UCAN*
  ("log/append/artifact/done on task T only"), **never the root key** (FR-C1/C2).
  Attenuating-only, revocable, scope/TTL set by the leash. This is WG-Fed's UCAN —
  no second delegation system is invented.
- **A context bundle** (doc 04 §1.4): a signed, BLAKE3-content-addressed,
  optionally-sealed `ContextScope` slice (`Clean < Task < Graph < Full`) — the
  successor to today's `.wg/` symlink. Transit is sealed with WG-Fed per-recipient
  encryption (no new crypto, NFR-4); the canonical graph stays at the authorizer.
- **A cross-host lease** (doc 04 §1.5): WG's existing
  `claim → heartbeat → reclaim` loop lifted across the trust boundary — liveness is
  a signed `LeaseRenewal` judged by the *authorizer's* observation (FR-L3), and a
  monotonic **lease epoch** fences double-execution (FR-L2).
- **The leash policy engine** (doc 04 §1.6): one legible function
  `leash(provider_trust, task_sensitivity, pool_class, env_config) →
  {delegation, context, isolation, verification, lease}`. The four candidates are
  **four named regions of its output space** — this is the spine of the whole
  design (§3 HQ11).
- **A new `src/providers/` module** (doc 04 §1.7) — the execution-plane analog of
  WG-Fed's `src/identity/`: `mod.rs` (`WG_EXEC_COMPAT_VERSION` + wire envelopes +
  the `ProviderRegistry`), `placement.rs` (matcher + the `leash()` engine),
  `bundle.rs` (build/seal/verify the context bundle), `lease.rs` (signed lease +
  fencing epoch), `verify.rs` (the verification levers). Plus surgical touch-points:
  `handler_for_model.rs` gains a `RemoteRunner` `ExecutorKind` arm; `plan_spawn`
  gains a `placement ∈ {Local, Provider(wgid:)}` field; `wg claim` becomes
  capability-gated; the agent registry gains a `ProviderEntry` with cross-host
  liveness; token/cost accounting (`parse_token_usage`, `wg spend`) is reused as the
  budget substrate.

### 2.2 What it configures (D, shipped A-first, C-slot-early, B-as-overflow) and why

`WG-Exec` is **Candidate D** — the leash engine selecting per task among A's, B's,
and C's tiers — pinned to a specific, opinionated, *fail-closed* default
configuration so it is **one architecture, not a menu**:

| Dimension | `WG-Exec` choice | Lineage (doc 04 candidate) |
|---|---|---|
| **Placement authority** | Per-authorizer; push-default + pull-optional; no central scheduler | A / CI pull-claim |
| **Default tier** | **A — trusted private pool** (the slack leash default: `Verified` provider, `normal` sensitivity) | **A** |
| **Confidential tier** | **C — attested TEE**, the escape-hatch; *slot shipped early*, enclave later | **C (as component)** |
| **Overflow tier** | **B — verified cooperative**, re-run/eval-gate on checkable code, **vouched/attested providers only** | **B (closed, not open)** |
| **Selector** | The `leash()` engine, **fail-closed** (unlabeled ⇒ refuse/C, never A); sensitivity inferred+labelled, leash linted + surfaced | **D** |
| **Capability flow** | Two scoped attenuating UCANs; privileged-op callback off low-trust providers | reused (WG-Fed UCAN) |
| **Verification** | Trust-proportional: attribution+eval-gate (trusted) → trusted-domain re-run vs pinned spec (low-trust); **quorum deferred** | A/B levers, B's re-run gated |

**Why D and not A outright** (doc 05 ranks A #1 overall): doc 05 §7.1 is explicit
that A's #1 ranking is *"the smallest proven surface that is never silently wrong …
the v0,"* and that **"D is where you converge; you get there *through* A, not
instead of it"** — because **A is reachable inside D** (A is D with one tier
enabled, doc 04 §5.3). Choosing D *with an A-shaped default* gives us A's entire
score-sheet (Liveness 5, Simplicity 4, WG-fit 5, Decentralization 5, Maturity 4)
**plus** the only thing A structurally cannot do: run confidential work (route to
C) and low-trust work (route to verified-B) *without silently exposing context or
accepting an unverifiable result*. A *as the terminal architecture* is A-i Fatal —
"it assumes the adversary away" and has *no in-band way to know* its trust
assumption has been violated (doc 05 §4.1). We do not discard A; we **ship it first
and keep the leash engine wrapped around it from day one**, which makes us *D-shaped
before D is fully built* and neutralizes A-i at the policy level.

**Why we reject B as an open market** (doc 05 ranks B #3): B earns *genuine*
integrity (re-run/quorum on checkable artifacts, X-5) and extends reach to
semi-trusted compute — but as a standalone *open* market it carries the study's
hardest unsolved problem, **B-i (sybil/collusion), Fatal-for-the-open-market**: free
`wgid:` keys (P4) + collusion (P5) defeat quorum's honest-majority, the
squat-penalty, *and* reputation **at once**, and permissionless sybil-resistance is
unsolved (the same wall WG-Fed's identity study hit). The only fixes —
vouch/stake/attest enrollment — **close the market into a cooperative**, surrendering
the open reach that was B's reason to exist. So we **harvest B as a *tier*** (the
verified-overflow path for checkable code on a *vouched/attested* pool) and **reject
the open market for v1** (§7 non-goal). Elegant corollary doc 05 B-i names: **C's
attestation doubles as sybil-resistance** (a distinct enclave proves distinct
hardware, far costlier to mint than a keypair), so the C tier *strengthens* the B
tier inside D.

**Why we reject C as the everyday architecture** (doc 05 ranks C #4 as a whole, #1
as a component): C is *last as a standalone* — least mature (2), costliest (2),
least decentralized (2 — the non-WG vendor root, C-iii), most complex (2 — the
measurement-allow-list curation that is its real TCB, C-i) — and it **concentrates
catastrophic risk in one chain**: a single attestation break forges *both*
confidentiality *and* integrity (C-ii), and the enclave's I/O boundary leaks poison
under a valid quote (doc 05 §3.2). As the daily architecture it is indefensible for
a gate. **But this is about C-as-default, not C-as-capability.** C is the *only*
answer to the confidentiality crux (TC1), the only tier where a provider you do
*not* trust can hold confidential context. So we **harvest C's attested tier as D's
escape-hatch, ship its *slot* before any enclave exists** (doc 04 §4.10 — interface
before silicon), and require **attested, never self-advertised**, isolation for any
confidential routing.

**The selector verdict we are designing to** (doc 05 §4.4): D's own attack surface
is the *selector* (TC10) and its fail-safe default (D-i/D-ii). `WG-Exec` therefore
adopts the *defended* end of that surface: the leash policy **cannot emit
broad-scope + full-plaintext-context for `provider_trust < floor`** (it returns the
minimized/sealed region or refuses — the policy *is* the guardrail, doc 04 §1.6);
the **unlabeled default is fail-closed** (refuse/C, never A — D-i); sensitivity is
**inferred + labelled, not solely self-asserted** (D-ii, taint-tracked through
`--after`, X-2); the applied leash is **surfaced** in `wg show`/`wg providers`; and
a **leash lint** rides `wg config lint` (which already exists — CLAUDE.md). This is
the resolution to the irony doc 05 §1.3 surfaces — *openness ↔ confidentiality* pull
against each other at placement, *confidentiality ↔ integrity-evidence* pull at
verification — `WG-Exec` keeps **one leash engine pricing the trilemma per task**
rather than globally: default to the trusted pool (no tension), reach for the TEE
only when confidentiality demands it (pay the price per task), verify checkable
artifacts in a trusted domain (evidence without exposing the transcript).

### 2.3 Where this rests on WG-Fed (the compose contract)

`WG-Exec` is a *consumer* of WG-Fed, not a peer. The dependencies are explicit so
the two memos cannot drift:

- **Identity.** Authorizer/provider/worker `wgid:` identities, sigchain
  verification, and `trust_level` are WG-Fed's (its HQ1/HQ5/HQ8). `WG-Exec` reads
  them; it defines no identity.
- **Capability.** The two scoped UCANs are WG-Fed's UCAN (its HQ11, "attenuating-only,
  kills the hydra"). The custodian-held agent root + ssh-agent-style "sign this
  digest" boundary is WG-Fed's HQ1/ADR-003. The worker never receives root bytes —
  same custody boundary, now across a host.
- **The central-vs-decentralized invariant** is WG-Fed HQ6 verbatim: *trust root
  never central; everything else may be central but only as a hint that cannot
  override a self-verification.* `WG-Exec`'s placement authority is per-authorizer
  (it does not even need WG-Fed's optional node for the private-pool case), and any
  central directory/reputation it uses obeys the same "hint, never override" rule.
- **Transport + encryption.** Cross-host bundle/result movement rides WG-Fed's
  store-and-forward transport and per-recipient sealed envelopes (its HQ3/HQ4). No
  new wire crypto.
- **The compat convention** mirrors WG-Fed's `WG_FED_COMPAT_VERSION`
  (itself mirroring `WG_AGENCY_COMPAT_VERSION`): `WG_EXEC_COMPAT_VERSION`,
  authenticated, loud-fail.

There is **no contradiction** with WG-Fed: where WG-Fed ranks B>C>A>D and chooses
"C, B-shaped, D's-UCAN-grafted, A-preserved," that is the *identity* layer; this
memo ranks A>D>B>C and chooses "D, A-shaped, C's-tier-grafted, B-as-overflow" for
the *execution* layer. Both reach their convergence target by *phased convergence
from the proven nearest option*, both keep a fail-safe guardrail (WG-Fed's strict
mode + cascade lint; `WG-Exec`'s fail-closed leash + leash lint), both keep the
trust root decentralized, both harvest the best component of the rejected candidate.
The shapes rhyme by design.

---

## 3. Decision register — every doc-03 hard question, decided

Format per question: **DECISION** · *why* · *rejected (cite doc 05 where adversarial)*
· *cost we accept*. Every one of the twelve is decided; nothing is deferred
silently. **HQ1 and HQ2 are the load-bearing cruxes** and lead.

### HQ1 — Context confidentiality on borrowed compute **(THE CRUX)** — *DECIDED*

**DECISION.** Three confidentiality tiers along the leash dial, made *plainly*
(§1, §2.3): **trust (A, default)** — the provider sees plaintext, a *stated, bounded*
property for `Verified` providers you own; **minimize (B)** — ship the smallest
`ContextScope` slice, which bounds blast radius **but is not confidentiality**; and
**attest (C)** — the only tier that defends context against a provider you do not
trust. **A confidential task routes to C (attested) or is refused — never to A or
B** (FR-K5, loud). **Secrets** are never long-lived plaintext on an untrusted
provider (FR-K2): authorizer-held with a privileged-op callback (authority *off* the
provider) for low-trust, sealed-to-attestation for C, plaintext-for-the-task-only on
a pool you own. **Minimal-context** defaults to the *smallest* slice and widens on
demonstrated need (FR-K3), with sensitivity **taint-tracked through `--after`
edges** (the slice-builder is the confidentiality TCB — X-2). The **attestation
slot** ships early — the `attest.rs` interface + seal-to-quote hook — even though
v1's payload behind it is "trust the pool or refuse" (FR-K4); absence of attestation
**downgrades loudly**, never silently.

*Why.* This is doc 05 §3.1's verdict made operational: *confidentiality on untrusted
compute costs a TEE, with all of the TEE's residual; everything else is "trust" or
"ship less."* Naming the tier honestly — and *refusing* rather than pretending — is
what keeps "borrow a friend's GPU" from becoming "hand a stranger your secrets."

*Rejected.* **B-as-confidentiality** — the dangerous deployment error that mistakes
"minimized" for "confidential" and routes mildly-sensitive work to B (doc 05 §3.1:
the provider reads the slice and *every token*; the residual "you cannot run
confidential work on B" is *inherent*, not a bug). **Silent run-degraded** on a
provider that can't meet the bar (FR-K5 exists to prevent exactly this).

*Cost.* The TEE tax (vendor root + measurement curation + side-channel residual,
C-i/iii/iv) *per confidential task*; the effectiveness tax of minimization (a
too-small slice makes the agent worse, T6). We pay both knowingly and per-task,
never globally.

### HQ2 — Result integrity from a hostile provider **(the co-crux)** — *DECIDED*

**DECISION.** **Attribution + a trust-proportional verification leash.** Every
result is signature-**attributed** to agent G (via the delegated signer chained to
G's sigchain); an unsigned/wrong-signed result is rejected (FR-V1). But **attribution
is not integrity** — the provider holds the delegated signer and can sign its lie
(doc 05 §3.2) — so the *menu of levers* is selected by trust (FR-V2/V5): **trusted ⇒
attribution + the WG eval-gate** (`auto_evaluate`/FLIP scoring against
`## Validation`); **low-trust ⇒ deterministic re-run**, run **in a trusted domain
(authorizer-side or a *disjoint* trusted provider — never the same provider, X-5)**
against the **authorizer's *pinned* acceptance test (tests are spec, not the
provider's deliverable — X-6)**. **Equivalence, not byte-identity** (agent outputs
are nondeterministic: eval-gate agreement / test-pass / semantic check). The
worst-case forged result is **blast-radius-bounded** by the task-scoped graph-write
UCAN (FR-V4) and is auditable/revocable after the fact. **Quorum is deferred to v2**
(it needs sybil-resistance, B-i — see HQ4/§7). **Random spot-check re-runs even on
trusted providers** cover the fungible normal-sensitivity middle (the P6
behave-then-defect path, doc 05 §3.2). **Cross-task poison (TC8)** is defended
structurally — see HQ11/§1's commitment 3.

*Why.* Doc 05 §3.2's verdict: **C ≥ B-in-scope > D-default > A** on integrity, but
*the deepest threat is cross-task, not single-task*. The re-run lever is "verify the
checkable artifact, not the nondeterministic transcript" (doc 01 §4.2) — correct
**only** in a trusted domain against a pinned spec, which doc 04 stated loosely and
this decision nails.

*Rejected.* **Attribution-only on low-trust** (a signer-holding provider forges
freely, doc 05 §3.2 — A's integrity reduces to the eval-gate, "a quality filter, not
an integrity proof"). **Same-provider re-run** ("theatre," X-5). **Auto-trusting a
provider-shipped test** (test-poisoning — the diff rewrites its own oracle, X-6).
**Quorum on an open pool** (a sybil cartel returns the same forged artifact and
quorum agrees on the lie, B-i).

*Cost.* N× compute for re-run (the integrity-vs-cost tension, T2 — the authorizer
holds re-run compute and cannot fully offload); a review gate on tasks that
legitimately author tests (X-6 friction); spot-checks at N×-on-a-sample.

### HQ3 — Placement & scheduling — *DECIDED*

**DECISION.** **Per-authorizer, push-default + pull-optional.** The authorizer's
dispatcher assigns a ready task to a chosen provider in its known pool (the natural
lift of today's push-only `spawn_agents_for_ready_tasks`, now with a
`Provider(wgid:)` placement); a provider **may also pull** (claim) from the
authorizer's own ready queue via a capability-gated `Claim` (FR-P3). **One mechanism
spans private → cooperative → market** with only `trust_level` + the applied leash
changing, **not** the protocol (FR-P4). Matching is **filter-by-capability +
trust-floor**, with optional rank by cost/latency/reputation (FR-P5). Fairness /
anti-cherry-pick is **scoped out for v1** (private→cooperative pools don't need
auction fairness; flagged for the open-market non-goal, §7).

*Why.* Push is the nearest lift of today's dispatcher (WG-fit 5, doc 05 §6.1); pull
is the same `wg claim` shape, capability-gated. The single-mechanism-spans-pools
property is the EX6 spine: "moving a provider from my pool to a stranger changes only
its trust level + the applied leash" (doc 04 §1.6).

*Rejected.* A **central scheduler** (→ HQ10: a single point of failure/capture and a
metadata chokepoint). **Open-market cherry-pick auctions** in v1 (non-goal §7).

*Cost.* The per-authorizer scheduler sees only *its* providers' state (no global
optimization) — acceptable at work-speed (NFR-2), not RTC.

### HQ4 — Provider identity & trust/reputation — *DECIDED*

**DECISION.** A provider is a **`wgid:` identity** (reused from WG-Fed, FR-R1) —
placement is against a verifiable key, never an IP/hostname. **`trust_level` is the
single dial** gating placement, leash, context, and verification (FR-R2).
**Reputation** accrues per-authorizer from observed behaviour (eval pass-rate,
liveness, integrity-check outcomes), is **local-by-default and advisory**, may be
signed-gossiped or vouched (WG-Fed web-of-trust), and **never depends on a central
ledger** (HQ10). The **behave-then-defect** attack (P6) is handled **structurally,
not reputationally**: high-sensitivity or low-trust placement *always* applies the
verification leash regardless of accrued reputation (FR-V5), and reputation only
buys *cheaper verification on fungible work* — plus random spot-checks on that
fungible middle. **Capability advertisements are signed** (FR-R4) and caught
after-the-fact on mismatch; **attestation is the strong proof** of a capability (the
TEE proves the model/sandbox), tying HQ1/HQ8.

*Why.* Doc 05 X-7's structural bound: poisoned reputation "cannot promote a defector
past the verification gate on the work that matters." Sybil-resistance is the
unsolved problem (B-i), so trust for *sensitive* work is gated by verification, not
by an accruable score.

*Rejected.* A **central reputation authority** (HQ10 violation). **Reputation-as-trust
on sensitive work** (the behave-then-defect hole, doc 05 §3.2/X-7).

*Cost.* Spot-checks at N×-on-a-sample; gossip is a hint, not correctness.

### HQ5 — Capability flow to the worker — *DECIDED*

**DECISION.** **Two scoped attenuating UCANs, never the root key** (FR-C1, WG-Fed
HQ11): an *act-as-agent UCAN* ("run task T as agent G," + expiry) and a *graph-write
UCAN* (task-T-scoped: log/append/artifact/done/[optional subtask] — **not** blanket
graph write, FR-C2 — the one non-negotiable floor even on a trusted pool). **Scope
and TTL ride the leash dial** (FR-C4): broad/long for trusted, narrow/short for
strangers. **Revocation** = short TTLs + issuer-subtree revocation (kill the lease →
kill the grant) + **a write-time check at the authorizing graph** (FR-C3). The
**privileged-op callback** (ssh-agent-style "sign/do this") keeps authority *off*
low-trust providers entirely — and it is **intent-bound, rate-limited,
budget-metered, and logged** (X-3) so it cannot be turned into a signing oracle or a
free-inference proxy. A *trusted* pool member **may** hold a broader standing scoped
signer (the leash slack default), still revocable, root still never leaving the
custodian. This is WG-Fed's UCAN — **no second delegation system** (NFR-4).

*Why.* "Ship too much and a single leaky provider impersonates the agent everywhere;
ship too little and the worker can't do its job" (doc 03 HQ5). Attenuating-only +
short-TTL bounds a leaked credential's blast radius to *one task, minutes* — the
best TC4 posture (doc 05 §4.2).

*Rejected.* A **standing root or blanket-write** credential on the worker (FR-C1/C2).
A **"do-anything" bearer-token callback** (the confused-deputy oracle, X-3).

*Cost.* The callback loses generality; per-deployment callback policy; on low-trust
providers the worker chatters back to the authorizer for privileged ops.

### HQ6 — Liveness across trust boundaries (the distributed orphan) — *DECIDED*

**DECISION.** Remote claims are **leased**; liveness is a **signed `LeaseRenewal`**
judged by the **authorizer's own observation of accepted renewals** (FR-L3), not the
provider's self-report — a provider that lies "alive" but produces no accepted
renewal/result is reclaimed when the lease lapses. Double-execution is fenced by a
**monotonic lease epoch**: reclaim increments it and re-places; a late-returning
(merely partitioned) worker presents a stale epoch / expired delegation and its
`ResultEnvelope` write is **rejected at the boundary** (FR-L2). **The epoch check is
an atomic compare-and-set at the single canonical-graph write boundary** (the
authorizer — X-4; a TOCTOU there reopens the double-commit, so this is mandatory).
**Lease term rides the dial** (FR-L4): long/relaxed for trusted, short/aggressive for
strangers. **Squatting** is bounded by lease caps + no-progress reclaim + a
reputation penalty. Stance = **prefer-liveness** (reclaim fast; the fence dedupes).

*Why.* This is the **best-defended area in the study** (doc 05 X-4) — it inherits
WG's mature `claim → heartbeat → reclaim` lifecycle (doc 02 §2.5), made cryptographic
and cross-host. Liveness ≠ local PID (`is_process_alive` is "meaningless across a
machine boundary").

*Rejected.* **Provider self-report as the liveness source** (FR-L3 — a hostile
provider squats forever). **Prefer-safety** (don't-reclaim-until-certainly-dead
stalls the graph; the fence makes prefer-liveness safe).

*Cost.* A mandatory compare-and-set on the graph write-path (a small, well-understood
concurrency primitive).

### HQ7 — Data / context locality — *DECIDED*

**DECISION.** A defined **context bundle** = a signed, BLAKE3-content-addressed,
optionally-sealed `ContextScope` slice (FR-D1). **Movement:** the **provider pulls**
a signed bundle by CID on the market/confidential paths (decentralization-friendly,
least-coupling); the **authorizer pushes** on the trusted-pool path (operationally
simplest). **In transit:** WG-Fed per-recipient sealed envelopes (X25519 +
XChaCha20-Poly1305) — a relay/MITM sees ciphertext, tampering is detected (FR-D2, no
new crypto). **At rest:** stated *per trust class* (FR-D3) — plaintext-for-task on a
trusted pool you own / encrypted-to-enclave on C / minimized-and-ephemeral on a B
provider whose deletion you cannot trust — so **confidentiality never relies on an
untrusted provider's goodwill to delete** (C never sees plaintext; B sees only the
minimal slice). **The canonical graph stays at the authorizer** (single-writer
spine, WG-Fed HQ7); the provider holds a slice and writes back deltas.

*Why.* "Once context lands on the provider's disk you are trusting it to delete, and
an untrusted provider's goodwill is worth nothing" (doc 03 HQ7) — answered by *not
relying on deletion* for the untrusted classes. The bundle replaces the `.wg/`
symlink that "a remote host cannot symlink" (doc 02 §2.2a).

*Rejected.* **Relying on provider deletion** for confidentiality on untrusted classes
(FR-D3). **A new at-rest crypto scheme** (reuse WG-Fed, NFR-4).

*Cost.* The pull path means the bundle exists as an addressable blob (mitigated by
sealing + CID + minimization, X-2).

### HQ8 — Sandboxing / isolation guarantees — *DECIDED*

**DECISION.** An **isolation-class ladder** with a **per-trust minimum**
(FR-D4): `git worktree` (today, private pool) ↔ container ↔ microVM
(Firecracker-class) ↔ TEE/enclave (also gives HQ1 confidentiality). Placement
requires the task's minimum class. Isolation is justified in **both directions**
(FR-D5): protect the *worker's* context from a snooping provider/co-tenant (HQ1)
**and** protect the *provider's* host from a hostile workload (the agent or a
poisoned task escaping — P8). For low-trust/confidential routing the class is
**attested, not merely advertised** (a self-advertised class is unverifiable, doc 04
§3.7 — the gap that motivates C). **Egress policy** is stated: default
egress-restricted/allow-listed for low-trust (bounds exfiltration), open on the
trusted pool. v0 maps cleanly onto today's worktree for the private-pool case (EX8).

*Why.* A worktree assumes a shared trusted machine — "that assumption dies under
federation" (doc 03 HQ8). The class must hold across a trust boundary, both ways.

*Rejected.* **Trusting a self-advertised isolation class on confidential routing**
(doc 05 TC10/C-iii — require attested). **Open egress for low-trust** (exfiltration
vector).

*Cost.* Stronger isolation (microVM/TEE) costs operationally and shrinks the eligible
provider set (the EX6 dial again).

### HQ9 — Economics & metering (v1-deferred, *named not dropped*) — *DECIDED*

**DECISION.** The **payment model is named per pool class**, with v1 = **"you own
the pool, you pay" (authorizer-funded)** explicit (FR-E1). **Budgets/ceilings are
enforced** via the existing `graph::parse_token_usage` / `wg spend` accounting
(FR-E2, R32): a remote task is halted/flagged when metered spend crosses its ceiling;
a runaway provider is capped. **Metering is attributable/signed** enough to detect a
padded bill (FR-E3) — and we **prefer authorizer-funded inference** because
provider-funded metering is **unverifiable** (the authorizer has no independent count
to reconcile against, doc 05 B-iii); provider-funded is capped + sample-audited,
never trusted at face value. **A market economy — pricing, auctions, escrow,
settlement, disputes — is a stated non-goal** (§7), not silently dropped.

*Why.* "The moment compute is someone else's, who-pays is unavoidable" (doc 03 HQ9),
but a full market is a large subsystem and a clear v1 non-goal — while budgets and
naming the model are *not* deferrable (a runaway remote task burns unbounded money).

*Rejected.* **Trusting a provider-funded bill at face value** (B-iii — unverifiable).
**Building market settlement** in v1 (non-goal §7).

*Cost.* The authorizer-funded model constrains who-pays choices (the
confidentiality-cleanest funding model, doc 05 B-iii); provider-funded overflow is
capped, not free-form.

### HQ10 — Decentralization vs central scheduler — *DECIDED (the plain call)*

**DECISION (plain).** **Placement authority is per-authorizer; there is no central
scheduler, and no correctness- or security-critical capability depends on any single
central node** (mirrors WG-Fed HQ6). Per the per-capability table below, a provider
**directory/discovery** and **reputation gossip** **MAY** be central *convenience*;
**matching/scheduling** is **always** per-authorizer; **trust verification** is
**never** central (a local signature check against the `wgid:` sigchain). Every
central component is **a hint that can only help, never override** the local trust +
capability + signature check — lose it and reach degrades, correctness does not. The
private-pool/per-authorizer case works with **zero** central nodes (NFR-6).

| Capability | Centralizable? | Criticality |
|---|---|---|
| Trust / identity verification | **Never** | correctness/security-critical → local self-verify only |
| Matching / scheduling | **No (per-authorizer)** | correctness-critical → never central |
| Provider directory / discovery | Optional | convenience — loss degrades reach |
| Reputation | Optional (signed gossip) | convenience — a hint, never trust on sensitive work |

*Why.* This is WG-Fed HQ6 applied to compute placement: "decentralized at the trust
layer; central is a hint that can't override self-verify." It neutralizes a
compromised directory/reputation node *at the protocol level* (it cannot forge what a
verifier self-checks).

*Rejected.* A **central scheduler** as a mandatory trust root or metadata chokepoint
(doc 03 HQ10 — single point of failure/capture). A **mandatory central reputation
ledger** (HQ4).

*Cost.* No global cross-authorizer optimization or fairness — acceptable at
work-speed (NFR-2); the open-market scale case is a non-goal (§7).

### HQ11 — Leash calibration (the spine) — *DECIDED*

**DECISION.** **One legible policy function**
`leash(provider_trust, task_sensitivity, pool_class, env_config) →
{delegation{scope,ttl}, context{scope_tier,seal}, isolation{min_class},
verification{depth}, lease{term,renew_cadence}}` (doc 04 §1.6) — five dials moved
**coherently in one place**, not five ad-hoc thresholds. The **default is genuinely
slack** for the trusted/private-pool case (`Verified` + `normal` ⇒ broad/long
delegation, full context, worktree/container isolation, attribution-only
verification, long lease = **Candidate A** — EX6 honored, *not* a zero-trust system
in disguise). **Tightening is config-driven, per-deployment, no code change** (a
paranoid org and a solo hobbyist set different thresholds). **A too-loose leash on a
low-trust provider is impossible by construction** — the function *cannot* emit
broad-scope + full-plaintext-context for `provider_trust < floor`; it returns the
minimized/sealed region or **refuses** (FR-K5). **The unlabeled default is
fail-CLOSED: an unlabeled task ⇒ refuse or C, never A** (doc 05 D-i — A only for
*explicitly normal* sensitivity). Sensitivity is **inferred + labelled, not solely
self-asserted** (D-ii — taint-tracked through `--after`, X-2, so an adversarial task
author cannot mislabel an exfiltration task "normal"). The **applied leash is
surfaced** (`wg show <task>` / `wg providers`, mirroring the handler-first
`wg status` rendering) and a **leash lint** rides `wg config lint`. This is the
single most important decision: the four candidates are **four operating points of
this one policy**.

*Why.* EX6 is the spine of the study; doc 05 D-i/D-ii make the selector the chief D
risk, so the fail-closed default + infer-don't-self-assert + lint + surface are
*mandatory*, not optional polish.

*Rejected.* **Fail-safe-to-A** (D-i Fatal-as-written: an unlabeled-but-confidential
task silently exposed). **Self-asserted sensitivity** (D-ii: attacker-set label).
**Hardcoded per-dimension thresholds** (the five dials drift incoherently).

*Cost.* The largest test/config matrix of the four (mirrors WG-Fed's C-1 finding) +
ongoing config discipline to keep defaults fail-closed.

### HQ12 — Execution-wire evolution & compat — *DECIDED*

**DECISION.** A **`WG_EXEC_COMPAT_VERSION`** named constant in `src/providers/mod.rs`,
**mirroring `WG_AGENCY_COMPAT_VERSION` (`1.2.4`)** and `WG_FED_COMPAT_VERSION`,
exchanged on connect and **failing loudly on incompatible mismatch** — vN/vN+1
negotiate a shared subset or refuse; the bare-`openrouter:` silent-misroute class of
bug (the 14-hour-401 cautionary tale) is **impossible**. The handshake is
**authenticated** (negotiated parameters *signed*, not merely exchanged — the WG-Fed
S-7 lesson) with an enforced **minimum floor** (min isolation class, min `alg`,
must-encrypt — not "lowest common," and **checked before any context ships** — X-1).
**The execution envelopes** (`PlacementOffer`/`Claim`/`RunGrant`/`LeaseRenewal`/
`ResultEnvelope`) are *this study's* to version; **identity/delegation/crypto formats
are inherited from WG-Fed** — the boundary is explicit, no duplicated/forked crypto
(NFR-4).

*Why.* Authorizer and provider are separately-owned, separately-updated processes; a
silent mismatch places a task on a provider that *almost* understands it — the worst
failure mode (doc 03 HQ12). WG already has the scar tissue and the convention.

*Rejected.* **Sniffed/ad-hoc versioning** and **best-effort-degrade** (silent
mis-route). **Re-versioning WG-Fed's inherited formats** (NFR-4 boundary).

*Cost.* Maintain + enforce the floor; retiring a weak `alg`/version breaks old
providers — loudly, which is correct.

### Decision-register summary

| HQ | Topic | `WG-Exec` decision (one line) |
|---|---|---|
| HQ1 | **Context confidentiality** | 3 tiers: trust(A)/minimize(B)/attest(C); **confidential ⇒ C-or-refuse, never A/B**; secrets off untrusted providers; slot early, enclave later |
| HQ2 | **Result integrity** | Attribution + trust-proportional leash; **re-run in a trusted domain vs a pinned spec**; quorum deferred; cross-task poison defended structurally |
| HQ3 | Placement | Per-authorizer, push-default + pull-optional; one mechanism spans pools via trust; no central scheduler |
| HQ4 | Provider trust/reputation | `wgid:` + `trust_level` single dial; reputation local/advisory; behave-then-defect handled **structurally** (verify regardless of reputation) |
| HQ5 | Capability flow | **Two scoped attenuating UCANs, never the root key**; leash-driven scope/TTL; intent-bound privileged-op callback; WG-Fed's UCAN |
| HQ6 | Liveness/reclaim | Signed lease, authorizer-judged liveness; **lease-epoch fencing via atomic CAS** at the canonical-write boundary; prefer-liveness |
| HQ7 | Data locality | Signed/CAS/sealed context bundle; provider-pull (market/C) / authorizer-push (trusted); **never rely on provider deletion**; canonical graph at authorizer |
| HQ8 | Sandboxing | Isolation-class ladder, per-trust minimum; **both-directions**; **attested not advertised** for low-trust; egress-restricted low-trust |
| HQ9 | Economics | v1 = authorizer-funded "you-own-it-you-pay"; budgets/ceilings enforced (R32); signed metering; **market is a stated non-goal** |
| HQ10 | **Decentralization vs central** | **Placement per-authorizer; no central scheduler; verification never central; central = a hint that can't override self-verify** |
| HQ11 | Leash calibration | **One policy, five dials coherent; slack default = A; fail-CLOSED unlabeled ⇒ refuse/C; infer-don't-self-assert; surfaced + linted** |
| HQ12 | Wire evolution | `WG_EXEC_COMPAT_VERSION` loud-fail + **authenticated** handshake + min floor; envelopes ours, crypto inherited from WG-Fed |

---

## 4. The execution spark test — "One task, a borrowed box, a scoped leash"

**Purpose.** The execution spark is the **minimal end-to-end proof** that validates
the whole `WG-Exec` choice and is the **first implementation milestone** the rest of
the execution plane runs across. It proves that a task can be **placed on a
separately-owned remote provider**, that the worker runs under a **scoped UCAN (not
the agent's root key)**, reads **only its task-slice**, and writes a **signed result
back to the shared graph** — with **(a)** the provider demonstrably *unable to act as
the agent beyond its granted scope*, and **(b)** a *hostile-provider integrity check*
catching a corrupted result. It is deliberately scoped to the smallest thing that
exercises every load-bearing §3 decision **and** the two headline cruxes of doc 05
§3 — and nothing more.

**It composes with the WG-Fed spark.** The execution spark **depends on** WG-Fed's
spark (`federation_spark_two_graphs.sh`) passing first — it assumes the WG-Fed
substrate (identity + signed cross-graph messages + UCAN delegation) already exists.
Where the WG-Fed spark proves *"two graphs, one key, a third location"* (a downloaded
identity cannot impersonate), the execution spark proves *"one graph, a borrowed box,
a scoped leash"* (a borrowed provider cannot exceed its lease) — the execution-plane
analog of WG-Fed spark step 6.

### 4.1 Setup

- **WG-A** — the **authorizer** (Alice's WG). Holds agent G's root in `wg secret`
  (the WG-Fed custody boundary). Its graph contains a task **T** with a
  `## Validation` section and a **pinned acceptance test** (the trusted spec).
- **Provider-P** — a **separately-owned** WG instance on a **different host with no
  shared filesystem**, enrolled as a `wgid:` provider at a chosen `trust_level`.
  **P does *not* hold G's root key.** This is the wall today's same-FS spawn cannot
  cross (doc 02 §2.1).
- A **disjoint verifier** — WG-A itself, or a second *independently-trusted* provider
  **Q** ≠ P — available to re-run for the integrity check (X-5: never same-provider).
- The **canonical graph stays at WG-A**; P writes back deltas via a scoped UCAN.

### 4.2 The six steps (each a falsifiable assertion)

1. **Place a task on a separately-owned provider.** WG-A emits a `PlacementOffer`
   for T (required model/handler, isolation class, sensitivity label, leash params);
   P claims it (`Claim`, with P's signed capability advertisement); WG-A issues a
   `RunGrant` = the **two scoped UCANs** (act-as-agent-G-for-T + graph-write-to-T-only)
   + a **sealed context bundle** (the `ContextScope` slice for T), sealed to P's
   enrollment key.
   **Assert:** the bytes delivered to P contain **no root key** and **no blanket
   graph-write capability** — only the two scoped UCANs (field-scan + spec-check).
   The `WG_EXEC_COMPAT_VERSION` handshake succeeded and is signed.
   *(Validates HQ5/FR-C1/C2, HQ12, HQ1/FR-K2.)*

2. **Worker runs under the scoped UCAN, reads only its task-slice.** The worker on P
   decrypts the bundle, runs agent G **under the act-as-agent UCAN**, and reads task
   T's slice.
   **Assert:** the bundle is **exactly** the configured `ContextScope` slice (T's
   input + its `--after` artifacts), **not** the whole graph; a field-scan of the
   delivered bytes finds **no out-of-slice secret** and no credential beyond the
   scoped UCANs. *(Validates HQ1/FR-K3 minimization, HQ7, the applied leash.)*

3. **Write a SIGNED result back to the shared graph.** The worker emits a
   `ResultEnvelope` (diff/artifacts + token/cost usage + a signature attributing to
   agent G via the delegated signer) and writes it to T via the graph-write UCAN.
   WG-A accepts it; `wg show T` attributes the result to agent G, and the usage is
   not bare (FR-V3).
   **Assert:** the result **verifies** against G's delegated signer chained to G's
   sigchain; an **unsigned or wrong-signed** result is **rejected**. *(Validates
   HQ2/FR-V1/C5; composes with WG-Fed spark step 5 "authenticate by key.")*

4. **(a) The provider cannot act as the agent beyond its granted scope** — the
   over-scope / confused-deputy / replay assertion (the *leash holds* proof). Using
   only what it holds, P attempts:
   - **(i)** to write to a **different task U** → **rejected** (graph-write UCAN is
     task-T-scoped — FR-C2/V4, blast-radius bound);
   - **(ii)** to sign a **new artifact as agent G for an unrelated purpose** after
     the lease/TTL elapses → **rejected** (act-as-agent UCAN is intent-bound +
     expired; the privileged-op callback is intent-bound, not a signing oracle — X-3);
   - **(iii)** to **replay its `RunGrant`** to start a second run, or replay a
     **stale `LeaseRenewal`/`ResultEnvelope` after reclaim** → **rejected** by the
     lease-epoch fence (atomic CAS at the canonical-write boundary — FR-L2/X-4).
   **Assert:** all three fail; the provider's authority is bounded to "run task T as
   G," nothing more. *(Validates HQ5, HQ6, FR-V4 — the execution analog of WG-Fed
   spark step 6 "downloaded-identity ≠ impersonation.")*

5. **(b) Hostile-provider integrity check** — result attribution **plus** a
   re-run/eval-gate catching a corrupted result. P, behaving hostilely, returns a
   **plausible-but-corrupted** `ResultEnvelope` (e.g. a diff that claims "tests pass"
   but is wrong, *and* a diff that edits the tests themselves). The authorizer
   applies the verification leash for P's trust level:
   - **(i)** **attribution** confirms *who* produced it (P-as-G) but **not**
     correctness, so on its own it does **not** accept;
   - **(ii)** the **re-run / eval-gate** runs **on the disjoint verifier (WG-A or Q,
     never P — X-5)** against WG-A's **pinned** acceptance test (**not** the
     provider's shipped tests — X-6) and **catches the corruption** → the result is
     **rejected** and T is re-placed;
   - **(iii)** any **test-file change** in the diff is **flagged for review** (X-6),
     and **provenance** records that P produced the bad artifact so descendants can be
     found and re-run (TC8/D-iii).
   **Assert:** the corrupted result is **caught and rejected**; attribution alone
   **never** launders a forgery into acceptance; the test-poisoning attempt is
   flagged. *(Validates HQ2/FR-V2/V4, the doc 05 §3.2 crux directly — the single most
   important integrity assertion in the milestone.)*

6. **Fail-closed confidentiality routing** — the confidentiality-tier call made
   executable. A task labelled `sensitivity = confidential` is offered only to P,
   which advertises **no** attestation.
   **Assert:** it is **held/refused** with a "no eligible confidential provider"
   reason — its context is **never** shipped to P in plaintext (FR-K5). And an
   **unlabeled** task does **not** route to A-on-a-stranger: it **refuses or routes
   to C** (D-i fail-closed). *(Validates HQ1/FR-K4/K5, HQ11/D-i — the confidentiality
   crux as a policy assertion.)*

### 4.3 What the spark deliberately leaves out (so it stays minimal)

- **No TEE enclave** (that is the C-tier build wave) — the attestation *slot* is
  exercised by the **fail-closed refuse** in step 6, not by a real enclave. The spark
  proves the *slot and the loud degradation*, not the silicon.
- **No quorum** (deferred to v2, needs sybil-resistance, B-i) — the integrity lever
  in step 5 is a **single disjoint re-run**, not N-of-M.
- **No open market / directory** — P is **hand-enrolled** and placement is **push**;
  the spark proves "central is convenience, not correctness" by needing none.
- **No economic settlement** — only a **budget ceiling** (R32) is exercised; pricing
  and provider-funded billing are out (HQ9/§7).
- **No human-friendly provider alias** — addressing is raw `wgid:` only.

### 4.4 Done-criteria (the Exec-Spark-PoC milestone gate)

The execution spark is **passed** when all six assertions hold in an automated
scenario, and that scenario is **landed as a permanent smoke gate**:
`tests/smoke/scenarios/exec_spark_borrowed_box.sh`, listed in
`tests/smoke/manifest.toml` `owners` for the Exec-Spark-PoC task (the manifest is
**grow-only** — CLAUDE.md). Its **prerequisite** is that the WG-Fed spark
(`federation_spark_two_graphs.sh`) already passes — the execution spark builds
*across* the identity+capability substrate, it does not re-prove it. Passing this
scenario is the empirical proof that the `WG-Exec` choice is buildable and correct;
everything in §5 builds across it.

---

## 5. Phased roadmap

The execution waves are a **successor program to WG-Fed** — execution depends on the
identity/capability substrate *existing*. They are numbered **Exec-Wave A…D** and
sequenced **after** the relevant WG-Fed waves. Each wave is independently valuable
(NFR-6).

```
                    WG-Fed waves (docs/federation-study/06 §5)
  W2 ADRs ─► W3 Spark ─► W4 transport ─► W5 state+recovery ─► W6 UCAN delegation
                                                                     │
                          (UCAN exists ⇒ scoped capability flow possible)
                                                                     ▼
  Exec-Wave A (ADRs) ─► Exec-Wave B (Exec Spark PoC) ─► Exec-Wave C (A hardened + B overflow)
                                                                     │
                                                                     ▼
                                          Exec-Wave D (C attested tier — enclave behind the slot)
```

**The hard sequencing dependency:** the load-bearing safety property of the
execution plane is the **two scoped UCANs (never the root key)**, which *is* WG-Fed
Wave 6 (UCAN delegation in `custody.rs`). So **the execution spark (Exec-Wave B)
cannot complete before WG-Fed Wave 6.** An *interim* A-tier preview using a
sigchain-authorized **standing signer** (WG-Fed Wave 5 deliverable — a signer, still
not the root) is possible earlier **only** behind the fail-closed leash refuse-row
(trusted-pool, non-confidential, normal-sensitivity work only); it is a preview, not
the spark, and is explicitly *not* allowed to run confidential or low-trust work.

### Exec-Wave A — ADRs (draft + accept *before* any execution code)

Draft and accept the four load-bearing execution ADRs (stubs in §6). **No execution
code lands until ADR-E1/E2/E3/E4 are Accepted.** Dependencies: this memo + **WG-Fed
ADR-001 (identity) and ADR-003 (custody/UCAN) Accepted** (the substrate the execution
ADRs cite). *Why first:* the three commitments of §1 (fail-closed leash, trusted-domain
re-run, cross-task poison) and the three bounded Fatals (A-i, B-i, D-i) must be
designed *in*, not discovered in code.

### Exec-Wave B — The Exec Spark PoC (the thinnest end-to-end slice)

Implement the minimum to pass §4's execution spark:
- `src/providers/` skeleton: `mod.rs` (`WG_EXEC_COMPAT_VERSION` + the five wire
  envelopes + `ProviderRegistry`), `placement.rs` (matcher + the **fail-closed**
  `leash()` engine), `bundle.rs` (build/seal/verify the `ContextScope` slice over
  WG-Fed crypto), `lease.rs` (signed lease + **atomic-CAS fencing epoch**),
  `verify.rs` (attribution + eval-gate + **single disjoint re-run vs pinned spec**).
- `handler_for_model.rs` gains the `RemoteRunner` `ExecutorKind` arm; `plan_spawn`
  gains the `placement` field; `wg claim` becomes capability-gated; the agent
  registry gains a `ProviderEntry` with signed-renewal liveness.
- **Deliverable:** the `exec_spark_borrowed_box.sh` smoke scenario passes (§4.4).
- **Dependencies:** Exec-Wave A Accepted **+ WG-Fed Wave 6 (UCAN)** (or the documented
  interim standing-signer preview, A-tier only).

### Exec-Wave C — A-tier hardened + B verified-overflow tier

- **A tier to production:** trusted private pool, push+pull, the leash slack default,
  budget ceilings (R32), the applied leash **surfaced** (`wg show`/`wg providers`)
  and **linted** (`wg config lint`).
- **B verified-overflow tier:** re-run/eval-gate on **checkable code** only (B-ii),
  re-run **in a trusted domain against a pinned spec** (X-5/X-6), **gated to
  vouched/attested providers** — never an open market (B-i). Route *non-checkable*
  work to A/C, not B.
- **Cross-task poison defense (TC8/D-iii) — first-class:** tier-by-graph-position
  (foundational/root ⇒ A/C, leaf ⇒ B), provenance tracking on every `ResultEnvelope`,
  re-verify-inputs-across-trust-boundaries.
- **Deliverable:** federated trusted-pool + verified-overflow execution; the
  cross-task poison placement constraint is enforced and tested.

### Exec-Wave D — C attested tier (enclave behind the slot)

- Ship the **real enclave** behind the attestation slot that has existed since
  Exec-Wave B: `attest.rs` verify, seal-context-to-attestation, attestation-bound
  lease renewals.
- **Curate the measurement allow-list** (pinned + audited measurements, reproducible
  runtime builds, nonce+`wgid:` binding in the quoted user-data — C-i); require
  **attested, never self-advertised** isolation for confidential routing.
- Treat attestation as **defence-in-depth alongside** the eval-gate + (where
  checkable) a cross-domain re-run — **never a sole oracle** (C-ii: a broken
  attestation is *worse* than none).
- **Deliverable:** confidential work runs on a provider you do *not* trust;
  `WG-Exec` is complete — Candidate D, reached by convergence, never big-bang.

### Don't-build-yet guardrails (explicit)

Out of scope until their gating wave; three are *never* to be built in the rejected
form:

- **Never** enroll an *untrusted* provider in the **A tier** — A is trusted-compute-only
  (A-i); the moment a provider is a stranger you are in B/C, under the leash.
- **Never** ship the leash engine without **fail-closed defaults** (unlabeled ⇒
  refuse/C, never A — D-i) + a **strict mode** + a **leash lint** (D-ii; mirrors
  WG-Fed C-1).
- **Never** route confidential work to A or B — confidential ⇒ **C (attested) or
  refuse** (FR-K5); and **never** accept a *self-advertised* isolation/attestation
  class as if attested for confidential routing (TC10/C-iii).
- **Never** open the **B tier** to permissionless providers until sybil-resistance is
  solved — B is a **vouched/attested cooperative** in v1, not an open market (B-i,
  Fatal-for-the-open-market).
- **Never** re-run verification on the **same provider** that produced the result, or
  against the **provider's own tests** — re-run is authorizer-side/disjoint against a
  **pinned** spec (X-5/X-6).
- **Don't** build **quorum**, an **open market**, **economic settlement**, or a
  **central scheduler** in v1 (§7 non-goals).
- **Don't** build the **TEE/enclave/attestation stack ourselves** — design the slot,
  *use* enclave primitives (doc 03 §5 non-goal 2).
- **Don't** pick the final wire/transport/TEE-vendor library before the spark and the
  A/B tiers inform it (the wire is candidate-agnostic through Exec-Wave C — doc 04
  §9).

---

## 6. ADR stubs (Exec-Wave A deliverables)

Four stubs, each following the project's lightweight ADR shape (Context · Decision ·
Status · Consequences · Alternatives rejected · Open questions). They are *stubs* —
Exec-Wave A fleshes each into an accepted ADR under `docs/` (matching the existing
`docs/ADR-*.md` convention).

### ADR-E1 — Placement & scheduling model (per-authorizer, push-default, one-mechanism)

- **Status:** Proposed (decided in this memo; to be ratified Exec-Wave A).
- **Context.** WG today is pure push: one dispatcher spawns workers locally (doc 02
  §2.1). Federation forks this into a scheduling problem; the frame wants *one*
  mechanism spanning private→cooperative→market via the trust dial (FR-P4) with **no
  central scheduler** (HQ10, mirroring WG-Fed HQ6).
- **Decision.** Placement is **per-authorizer**; **push-default + pull-optional**
  (capability-gated `Claim`); matching is filter-by-capability+trust-floor with
  optional rank; one mechanism spans pools with only `trust_level` + the applied
  leash changing. No correctness/security-critical capability depends on a central
  node; directory/reputation are optional *hints*. `plan_spawn` gains a
  `placement ∈ {Local, Provider(wgid:)}` field; `wg claim` becomes capability-gated.
- **Consequences.** New `src/providers/{mod,placement}.rs`; `handler_for_model.rs`
  gains a `RemoteRunner` arm. Enables FR-P1–P5. No central scheduler means no global
  optimization (acceptable at work-speed, NFR-2).
- **Alternatives rejected.** Central scheduler (HQ10 — SPOF/capture/metadata
  chokepoint). Open-market cherry-pick auctions in v1 (non-goal §7).
- **Open questions.** Default rank policy (cost vs latency vs reputation);
  pull-queue fairness for the cooperative case; exact `Claim` eligibility proof.

### ADR-E2 — Confidentiality tier & the attestation slot (trust/minimize/attest; C-or-refuse)

- **Status:** Proposed.
- **Context.** Running an agent exposes its context to whoever owns the compute (HQ1,
  THE crux). The only levers are trust, minimization, attested isolation — and doc 05
  §3.1 proves **only a TEE defends context against a provider you don't trust**;
  minimization ≠ confidentiality.
- **Decision.** Three tiers along the leash dial: trust(A)/minimize(B)/attest(C).
  **Confidential ⇒ C (attested) or refuse — never A/B** (FR-K5). Ship the attestation
  **slot** (interface + seal-to-quote hook) early; the enclave later (Exec-Wave D).
  Secrets never long-lived plaintext on untrusted providers (authorizer-held +
  callback, or sealed-to-attestation). Minimal-context defaults smallest, widens on
  need, **taint-tracked through `--after`** (X-2). Loud degradation, never silent.
- **Consequences.** `src/providers/{bundle,attest}.rs`; the leash engine's
  `context{scope_tier, seal}` output; a sensitivity classifier feeding the leash.
  Enables FR-K1–K5. The measurement-allow-list (curated Exec-Wave D) becomes the
  confidentiality TCB (C-i).
- **Alternatives rejected.** B-as-confidentiality (doc 05 §3.1 — minimize ≠ hide,
  inherent). Silent run-degraded (FR-K5). Building our own TEE (non-goal §7 / doc 03
  §5).
- **Open questions.** TEE vendor(s) and the diversity policy (C-iii); the
  sensitivity-classifier's auto-label threshold and human-in-loop boundary; the
  measurement-allow-list curation cadence and owner.

### ADR-E3 — Result integrity & the verification leash (attribution + trusted-domain re-run)

- **Status:** Proposed.
- **Context.** A hostile provider owns the worker env and holds the delegated signer,
  so **attribution proves *who claims*, not *correctness*** (HQ2, the co-crux). Every
  independent check costs compute (T2). The deepest threat is **cross-task**, not
  single-task (doc 05 §3.2/TC8).
- **Decision.** Attribution (FR-V1) + a **trust-proportional verification leash**:
  trusted ⇒ eval-gate; low-trust ⇒ **deterministic re-run in a trusted domain
  (authorizer-side or a disjoint provider, never same-provider — X-5) against a
  *pinned* acceptance test (tests are spec — X-6)**; **equivalence not byte-identity**.
  Blast-radius bounded by the task-scoped write UCAN (FR-V4). **Quorum deferred** to
  v2 (needs sybil-resistance, B-i). **Cross-task poison** defended via
  tier-by-graph-position + provenance + cross-trust-boundary input re-verification
  (D-iii). Random spot-checks on the fungible middle (P6).
- **Consequences.** `src/providers/verify.rs` (the lever menu, selected by
  `leash().verification`); `ResultEnvelope` carries evidence (FR-V3) + provenance;
  the eval-gate (`auto_evaluate`/FLIP) is reused; a test-file-change review gate.
  Enables FR-V1–V5.
- **Alternatives rejected.** Attribution-only on low-trust (signer-holding provider
  forges, §3.2). Same-provider re-run (theatre, X-5). Auto-trusting provider tests
  (X-6). Quorum on an open pool (B-i).
- **Open questions.** The "equivalent-not-identical" comparison for non-test artifacts
  (semantic check); spot-check sampling rate; provenance retention/audit window.

### ADR-E4 — Capability & lease lifecycle (two scoped UCANs + epoch-fenced lease)

- **Status:** Proposed.
- **Context.** The worker runs on a machine that may leak/steal whatever it holds, so
  the credential's blast-radius-if-exfiltrated must be small (HQ5), and reclaim must
  be safe against a partitioned worker double-committing (HQ6, the distributed orphan).
  Delegation is WG-Fed's UCAN — invent no second system (NFR-4).
- **Decision.** **Two scoped attenuating UCANs, never the root key** (FR-C1/C2):
  act-as-agent-for-T + graph-write-to-T-only; scope/TTL ride the leash (FR-C4);
  revocation = short TTL + issuer-subtree + **write-time check** (FR-C3); the
  **privileged-op callback is intent-bound, rate-limited, budget-metered, logged**
  (X-3). The cross-host lease carries a **monotonic epoch**; reclaim increments it;
  a late worker's stale-epoch write is **rejected via an atomic compare-and-set at
  the single canonical-graph write boundary** (FR-L2/X-4). Liveness is the
  authorizer's observation of accepted signed renewals (FR-L3).
- **Consequences.** `src/providers/lease.rs`; the graph write-path gains a mandatory
  epoch CAS; the agent registry's `is_live()` consults last-accepted `LeaseRenewal`,
  not `is_process_alive`. Reuses WG's `claim → heartbeat → reclaim` lifecycle.
  Enables FR-C1–C5, FR-L1–L4.
- **Alternatives rejected.** Standing root/blanket-write on the worker (FR-C1/C2).
  "Do-anything" bearer-token callback (X-3 oracle). Provider self-report as liveness
  (FR-L3). Prefer-safety reclaim (stalls; the fence makes prefer-liveness safe).
- **Open questions.** UCAN expiry defaults (blast-radius vs callback chattiness —
  inherits WG-Fed's open D-3); standing-signer TTL on the trusted pool; the exact
  CAS primitive on the `graph.jsonl` write-path.

---

## 7. Non-goals for v1 (explicit)

`WG-Exec` v1 carries forward doc 03 §5's non-goals and adds the decision-specific
exclusions. **Out of scope:**

1. **An open, permissionless compute market.** B is a **vouched/attested cooperative**
   in v1, never an open market (B-i, Fatal-for-the-open-market — permissionless
   sybil-resistance is unsolved).
2. **Quorum (N-of-M) verification.** v1's low-trust integrity lever is a **single
   disjoint trusted-domain re-run**; quorum waits on sybil-resistance (v2).
3. **Building a TEE / enclave / attestation service ourselves.** We design the
   **attestation slot** and *use* enclave/attestation primitives; we implement no
   enclave (doc 03 §5 non-goal 2).
4. **Homomorphic / compute-on-ciphertext execution.** Out of reach; the levers are
   trust, minimization, attested isolation — not running the model on encrypted data.
5. **A token/blockchain compute marketplace with on-chain settlement** — pricing,
   auctions, escrow, disputes. Only **budgets/ceilings (R32)** and **naming the
   payment model** are in (HQ9); the deferral is *stated, not silent*.
6. **A central scheduler / global matching / multi-tenant fairness-at-scale.**
   Placement is per-authorizer; large-scale open-market fairness and anti-cherry-pick
   auctions are flagged (HQ3) but not required.
7. **General-purpose / arbitrary compute.** This federates **WG agent-task
   execution**, not a generic batch-job or container-orchestration platform.
8. **Provider-side model hosting / inference-server design.** A provider brings its
   own handler + model access, surfaced only as a signed capability advertisement
   (FR-R4).
9. **Real-time / low-latency placement guarantees.** Work-speed, not RTC (NFR-2).
10. **Re-implementing the local dispatcher / spawn / claim path.** Today's
    `wg service start` / `wg spawn` worktree isolation / `wg claim` / `wg reclaim`
    are the **migration substrate** (EX8/NFR-3), not the redesign target.
11. **Inventing any identity / delegation / crypto / trust system.** All inherited
    from **WG-Fed** (NFR-4); `WG-Exec` composes `wgid:`, UCAN, per-recipient
    encryption, and `trust_level`, and defines no second such system.
12. **Provider-funded inference billing as the primary model.** v1 prefers
    authorizer-funded (provider-funded metering is unverifiable, B-iii);
    provider-funded is capped + sample-audited, not free-form.

---

## 8. Open questions handed to the ADR wave

These are *not* re-openings of the decision — they are the implementation forks the
Exec-Wave A ADRs (§6) must close, surfaced here so the ADR wave has a checklist:

1. **Default rank policy** for matching — cost vs latency vs model-freshness vs
   reputation, and the tiebreak (ADR-E1).
2. **The sensitivity classifier** — its inputs (taint-track depth through `--after`),
   its auto-label threshold, and the human-in-loop boundary for ambiguous tasks
   (ADR-E2, the D-ii infer-don't-self-assert requirement).
3. **TEE vendor(s) + diversity policy** and the measurement-allow-list curation
   cadence/owner (ADR-E2, the C-i/C-iii residuals).
4. **The "equivalent-not-identical" comparison** for non-test artifacts (semantic
   check) and the spot-check sampling rate (ADR-E3).
5. **UCAN expiry defaults** balancing blast-radius vs callback chattiness (inherits
   WG-Fed's open D-3) and the trusted-pool standing-signer TTL (ADR-E4).
6. **The atomic-CAS primitive** on the `graph.jsonl` write-path for the lease-epoch
   fence (ADR-E4, X-4).
7. **The wire/transport/TEE-vendor library** — deferred past Exec-Wave C by guardrail;
   let the spark + A/B tiers inform it.
8. **The interim standing-signer preview boundary** — exactly which tasks the A-tier
   preview may run before WG-Fed Wave 6 lands (trusted-pool, non-confidential,
   normal-sensitivity only).

---

## 9. Validation checklist (this document)

- [x] **One architecture chosen and defended vs the alternatives, citing doc 05.**
      `WG-Exec` = Candidate D (the leash-selector), shipped A-first, with C's attested
      tier as the confidential escape-hatch (slot early) and B as the vouched-overflow
      tier (§1, §2). Defended against A/B/C/D via doc 05's ranking (**A > D > B > C**
      whole-arch, **C first among components**, §7), the three bounded Fatals (A-i,
      B-i, D-i), and the §7.1 phased synthesis (§2.2).
- [x] **Every doc-03 hard question has an explicit decision** (§3, HQ1–HQ12), each
      with *why · rejected (cite doc 05) · cost*; the **decision-register summary**
      table is the one-glance check.
- [x] **The confidentiality-tier call made plainly** (§1, §2.3, HQ1): three tiers;
      **confidential ⇒ C (attested) or refuse — never A/B**; slot early, enclave
      later; secrets off untrusted providers.
- [x] **The placement-authority call made plainly** (§1, §2.3, HQ10):
      **per-authorizer; no central scheduler; verification never central; central = a
      hint that can't override self-verify** + the per-capability table.
- [x] **Trust-default / leash-as-a-dial honored** (§3 HQ11): one policy, five dials
      coherent, **slack default = A**, tightening config-driven, too-loose-on-a-stranger
      impossible by construction, **fail-CLOSED unlabeled default**.
- [x] **A concrete, runnable execution spark test** (§4): a task placed on a
      **separately-owned remote provider**, the worker under a **scoped UCAN (not the
      root key)** reading its **task-slice** and writing a **signed result** back, with
      **(a)** the provider unable to act beyond its scope (step 4) and **(b)** a
      **hostile-provider integrity check** (step 5: attribution + disjoint re-run vs
      pinned spec catching a corrupted+test-poisoned result). Composes with the WG-Fed
      spark (depends on `federation_spark_two_graphs.sh`); landed as a permanent smoke
      gate `exec_spark_borrowed_box.sh` (§4.4).
- [x] **Phased roadmap sequenced vs the WG-Fed waves** (§5): Exec-Wave A (ADRs) → B
      (Exec Spark PoC) → C (A hardened + B overflow) → D (C attested tier), with the
      hard dependency on **WG-Fed Wave 6 (UCAN)** stated + don't-build-yet guardrails.
- [x] **ADR stubs** for the four load-bearing decisions (§6): ADR-E1 (placement
      model), ADR-E2 (confidentiality tier), ADR-E3 (result integrity), ADR-E4
      (capability/lease lifecycle).
- [x] **Non-goals for v1** (§7), explicit and decision-specific.
- [x] **Composes with `docs/federation-study/06` (no contradiction)** (§2.3): inherits
      WG-Fed identity/UCAN/crypto/transport + the "central = hint, never root"
      invariant verbatim; the differing A/B/C/D rankings are *different candidate sets*
      (the naming caution, §0); both memos converge from the proven nearest option
      behind a fail-safe guardrail. **File written.**

---

*Wave-1 (execution federation) complete. The decision: **`WG-Exec`** — Candidate D's
leash-selector, **shipped A-first** (the proven trusted pool, the slack default),
with **C's attested tier as the confidential escape-hatch** (its slot shipped before
any enclave) and **B's verified tier as a vouched-overflow cooperative** (never an
open market), the whole reached by **convergence, not big-bang**, behind a
**fail-closed leash engine** so confidential context is never silently exposed and a
borrowed provider can never exceed "run task T as G." It rests on the WG-Fed
substrate — `wgid:` identity, scoped attenuating UCANs, per-recipient encryption, the
single trust dial — inventing no second system, and it composes with the WG-Fed
decision rather than contradicting it. The execution spark — a task on a borrowed
box, a scoped leash, a signed result, a hostile-provider integrity check — is the
first milestone and the permanent gate. The three bounded Fatals (A-misuse, B's
open-market sybil, D's fail-safe-to-A) are designed out from day one.*
