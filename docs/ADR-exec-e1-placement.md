# ADR-E1 (Exec): Placement & Scheduling — Per-Authorizer, Push-Default, One-Mechanism, No Central Scheduler

**Status:** Proposed
**Date:** 2026-06-26
**Decision:** Placement authority is **per-authorizer** — each WG schedules its own ready tasks onto providers it knows; **there is no central scheduler, no mandatory directory, no global queue in the correctness path**. The default is **push** (the authorizer assigns); **pull** (a provider claims work via a capability-gated `Claim`) is a first-class option, but a `Claim` is a *request* the authorizer independently verifies — the authorizer's signed `RunGrant` is the actual placement decision. Matching is a **hard filter** (capability match + trust-floor) followed by an **advisory, deterministic rank**; rank can reorder eligible providers but can **never** promote a provider past the trust-floor. **One mechanism spans private → cooperative → market** with only `trust_level` + the applied leash changing, never the protocol. Any central directory/reputation is an **optional hint that can only help, never override** the local trust + capability + signature check. `plan_spawn` gains a `placement ∈ {Local, Provider(wgid:)}` field; `wg claim` becomes **capability-gated**.

> **This is the first execution-plane ADR (Exec-Wave A).** ADR-E2 (confidentiality
> tier & the attestation slot), ADR-E3 (result integrity & the verification leash),
> and ADR-E4 (capability & lease lifecycle) cite the placement mechanism, the
> push/pull split, and the `Provider(wgid:)` seam fixed here. The decision was
> *made* in the execution-federation decision memo
> (`docs/execution-federation-study/06-decision-memo-and-roadmap.md` §1, §2.2, §3
> HQ3/HQ10, §6 ADR-E1 stub, §8 hand-off); this ADR formalizes it and resolves the
> stub's three open questions. It is **not** a re-litigation of the architecture
> choice (`WG-Exec` = Candidate D's leash-selector, shipped A-first, with C's
> attested tier as the confidential escape-hatch and B as a vouched-overflow
> cooperative) — that is settled.
>
> **It rests on WG-Fed and invents no second system.** Provider/authorizer/worker
> identities are `wgid:` (**ADR-fed-001**, Accepted); the scoped capabilities a
> placement issues are WG-Fed UCANs and the custodian-held root boundary is
> WG-Fed's (**ADR-fed-003**, Accepted). `WG-Exec` is a *consumer* of those
> substrates (memo §2.3, NFR-4); this ADR defines only the *execution-plane
> placement* that runs on top. **No execution code lands until ADR-E1/E2/E3/E4 are
> Accepted, and WG-Fed ADR-001/003 must be Accepted first** (memo §5, Exec-Wave A).

---

## Context

WG today is **pure push**: one dispatcher (`spawn_agents_for_ready_tasks`) walks the
ready set and spawns workers **locally**, on the **same machine, same filesystem**,
under `git worktree` isolation (`docs/execution-federation-study/02-current-state-baseline.md`
§2.1). "Placement" is implicit — every task runs *here*, because *here* is the only
place there is. There is no notion of a remote provider, no capability negotiation,
no scheduling decision beyond "is this task ready and do I have a free slot."

Federation forks this single implicit choice into a real **scheduling problem**: a
task may now run on a **separately-owned provider on a different host with no shared
filesystem** (memo §4.1). Three forces shape how that problem must be answered, and
they are the reason this ADR exists rather than "just lift the dispatcher across a
socket":

1. **One mechanism must span the whole trust spectrum.** The frame's load-bearing
   requirement (FR-P4, the EX6 spine) is that *moving a provider from my private pool
   to a stranger's cooperative changes only its `trust_level` and the applied leash —
   **not** the protocol* (memo §2.2). Private pool, vouched cooperative, and (future)
   open market are **one placement mechanism at three operating points of the leash
   dial**, not three subsystems. A design that needs a different scheduler per pool
   class fails this on contact.

2. **No central scheduler.** Placement authority must stay **per-authorizer**
   (HQ10, mirroring **WG-Fed HQ6** / ADR-fed-001 §D5): the trust root is never
   central, and any central component is *a hint that can only help, never override*
   a local self-verification. A central matching/scheduling service would be a single
   point of failure, a capture target, and a metadata chokepoint over who-runs-what —
   exactly the centralization the whole study exists to avoid (memo §3 HQ10). The
   private-pool case must work with **zero** central nodes (NFR-6).

3. **Push is the proven nearest lift; pull must be first-class.** Today's
   push-only dispatcher is the highest-WG-fit starting point (memo §2.2, Candidate A
   scores WG-fit 5), so push stays the default. But a cooperative wants providers to
   **pull** spare-capacity work — and `wg claim`/`wg reclaim` is already WG's mature
   claim lifecycle (`docs/execution-federation-study/02-current-state-baseline.md`
   §2.5), so pull is the *same shape*, now **capability-gated** across a trust
   boundary rather than open to any local process.

This ADR fixes the placement & scheduling model the rest of `WG-Exec` builds on. It
is the first Exec-Wave A deliverable; it is gated on **WG-Fed ADR-001/003 Accepted**
(the identity + UCAN substrate it cites) and, like its siblings, must be Accepted
before any execution code lands (memo §5).

---

## Decision

### D1 — Placement authority is per-authorizer; there is no central scheduler

Each authorizer's own dispatcher is the **sole** scheduler for the tasks in its
graph. It assigns a ready task to a provider drawn from **its own known pool** — the
natural lift of today's `spawn_agents_for_ready_tasks`, now able to target a remote
`Provider(wgid:)` as well as `Local`. **No matching/scheduling decision is ever made
by a shared third party**, and **no correctness- or security-critical capability
depends on any single central node** (HQ10).

The per-capability rule is fixed exactly as HQ10's table states it (and verbatim from
**WG-Fed HQ6** / ADR-fed-001 §D5):

| Capability | Centralizable? | Criticality |
|---|---|---|
| Trust / identity verification | **Never** | correctness/security-critical → local self-verify against the `wgid:` sigchain |
| Matching / scheduling | **No (per-authorizer)** | correctness-critical → never central |
| Provider directory / discovery | Optional | convenience — loss only **degrades reach** |
| Reputation | Optional (signed gossip) | convenience — a **hint**, never trust on sensitive work (ADR-E3) |

The binding invariant, inherited verbatim: **a central component is a hint that can
only help, never override a self-verification** (fail-safe, never fail-open). Lose the
directory and you reach fewer providers; lose nothing about correctness or security.
The cost — **no global cross-authorizer optimization or fairness** — is accepted: WG
runs at *work-speed*, not real-time-compute (NFR-2), and the open-market scale case is
a stated non-goal (memo §7).

### D2 — Push-default + pull-optional; a `Claim` is a request, the `RunGrant` is the decision

Placement has **two directions**, and the authorizer is the placement authority in
**both**:

- **Push (default).** The authorizer's dispatcher selects a provider (D3) for a ready
  task and emits a `PlacementOffer`; on acceptance it issues a `RunGrant` (the two
  scoped UCANs + the sealed context bundle, per ADR-E4/ADR-E2). This is the trusted
  private-pool path and the nearest lift of today's behavior.

- **Pull (first-class option).** A provider with spare capacity emits a **`Claim`**
  against the authorizer's **own** ready queue — the same `wg claim` shape WG already
  has, now **capability-gated** across the trust boundary (FR-P3). The provider
  advertises what it can run; it does **not** thereby authorize itself to run.

The load-bearing rule that keeps pull from becoming a central-queue-in-disguise or a
self-authorization hole: **a `Claim` is *necessary but not sufficient*.** It is a
*request* the authorizer **independently verifies** (D3, OQ3) and **may decline**;
the authorizer's signed **`RunGrant` is the actual placement decision**. A provider
can never start a run on the strength of its own `Claim` — only on receipt of a
`RunGrant` carrying the scoped UCANs. This preserves the HQ10 invariant on the pull
path: the claimer's self-asserted eligibility is just a hint the authorizer
self-checks before granting.

Claim **contention** (two providers claim the same task) is resolved by the **same
monotonic lease-epoch atomic compare-and-set** that ADR-E4 uses for reclaim fencing
(HQ6/X-4): the first `Claim` the authorizer grants wins the CAS on the task's lease;
a competing grant attempt sees a stale epoch and is rejected. No auction, no global
lock — one well-understood concurrency primitive shared with the lease lifecycle.

### D3 — Matching is a hard filter (capability + trust-floor), then an advisory rank

Matching a task to a provider — on **either** the push or pull path — is **two
phases**, and the order is load-bearing:

1. **Filter (a hard gate, never overridable).** A provider is *eligible* iff it
   passes **both**:
   - **Capability match** — the provider advertises the task's required
     model/handler and an isolation class ≥ the task's minimum (HQ8). For
     low-trust/confidential routing the class must be **attested, not self-advertised**
     (HQ8/TC10, ADR-E2) — an unverifiable advertisement does not satisfy the filter.
   - **Trust-floor** — `provider.trust_level ≥ leash(task).trust_floor`, where the
     floor is whatever the **fail-closed leash** (HQ11, ADR-E2/E3) demands for the
     task's sensitivity. **Trust is the authorizer's to assert from its own local
     record, never the provider's to self-certify.** An unlabeled task fails closed
     (refuse/C, never A — D-i); a confidential task with no attested-confidential
     provider eligible is **refused**, never downgraded (FR-K5).

2. **Rank (advisory, deterministic, optimization-only).** Among the providers that
   survive the filter, an optional rank picks an order (FR-P5). **Rank can only
   reorder the already-eligible set; it can never promote an ineligible provider, and
   reputation in the rank never buys a way past the trust-floor** (HQ4/X-7 — reputation
   is advisory, structurally barred from sensitive work). The default rank and its
   tiebreak are resolved in **OQ1**.

This filter-then-rank shape is what makes "a too-loose placement on a stranger is
impossible by construction" hold at the *scheduler* (it mirrors the leash engine's own
"cannot emit broad-scope for `provider_trust < floor`" guarantee, memo §2.2): the
security decision lives entirely in the **filter**, which a deployment cannot relax
into fail-open by tuning the **rank**.

### D4 — One mechanism spans private → cooperative → market

There is **one** placement mechanism — `PlacementOffer` / `Claim` / `RunGrant` over
the `WG_EXEC_COMPAT_VERSION` wire — and the three pool classes are **three operating
points of it**, distinguished by **only two things**: the providers' `trust_level`
and the **applied leash** (delegation scope/TTL, context tier/seal, isolation minimum,
verification depth, lease term — HQ11). The **protocol does not change** across pool
classes (FR-P4):

| Pool class | What changes | What does **not** change |
|---|---|---|
| **Private pool** (own boxes) | `Verified` trust, slack leash, push-default | the placement protocol |
| **Vouched cooperative** (B overflow) | `Provisional` trust, tighter leash, pull first-class, re-run verification (ADR-E3) | the placement protocol |
| **Open market** (v1 **non-goal**) | `Unknown` trust, tightest leash, fairness/auction layer | the placement protocol |

This is the EX6 spine made concrete: *"moving a provider from my pool to a stranger's
cooperative changes only its trust level + the applied leash."* The open-market point
is **out of scope for v1** (memo §7 non-goal 1 — permissionless sybil-resistance is
unsolved, B-i); it is named here only to show the mechanism *already spans to it*
without a protocol fork, so reaching it later is a leash-and-trust change, not a
redesign.

### D5 — Central directory/reputation are optional hints, never the correctness path

A provider **directory/discovery** service and **reputation gossip** **MAY** exist as
central *convenience* — they help an authorizer *find* providers and *prefer* among
eligible ones. They are bound by the D1 invariant: **a hint that can only help, never
override**. Concretely:

- A forged or compromised directory can only *withhold* or *mislabel* providers; it
  can never make the authorizer place a task on a provider that **fails the local
  filter** (D3) — capability is verified against the provider's **signed**
  advertisement and trust against the authorizer's **local** record, both checked
  locally (HQ10).
- Reputation gossip is **signed** and **advisory**: it can only feed the *rank*
  (D3 phase 2), never the *filter*. The behave-then-defect attack (P6) is handled
  **structurally** by ADR-E3's verification leash (applied regardless of accrued
  reputation), not by trusting a score (HQ4/X-7).

The **private-pool/per-authorizer case needs none of this** — it works with **zero**
central nodes (NFR-6). Lose every directory and reputation feed and the system still
schedules correctly within each authorizer's hand-enrolled pool; only *reach* degrades.

### D6 — Code seams

The decision lands on a small, surgical set of seams (memo §2.1, §6 ADR-E1
consequences):

- **New `src/providers/mod.rs`** — `WG_EXEC_COMPAT_VERSION` + the five wire envelopes
  (`PlacementOffer` / `Claim` / `RunGrant` / `LeaseRenewal` / `ResultEnvelope`) + the
  `ProviderRegistry` (the authorizer's known-pool record, keyed by `wgid:` with
  `trust_level` and last-seen capability advertisement).
- **New `src/providers/placement.rs`** — the **matcher** (D3's filter-then-rank) and
  the `leash()` engine it consults for the trust-floor (the leash engine itself is
  ADR-E2/E3/E4's; placement *reads* its `trust_floor`).
- **`plan_spawn` gains `placement ∈ {Local, Provider(wgid:)}`** — the single field
  that turns today's implicit "run here" into an explicit placement decision; `Local`
  reproduces today's behavior byte-for-byte (NFR-3, the migration substrate).
- **`handler_for_model.rs` gains a `RemoteRunner` `ExecutorKind` arm** — the executor
  that drives a `Provider(wgid:)` placement (its mechanics — bundle, lease, result —
  are ADR-E2/E3/E4's; this ADR fixes that the *arm exists* and is selected by
  `placement`).
- **`wg claim` becomes capability-gated** — the local-only claim grows the D2/OQ3
  eligibility proof and is admitted only via a `RunGrant`.

These seams enable **FR-P1–P5** and preserve today's local path unchanged
(`placement = Local`), so the local dispatcher is **not** re-implemented (memo §7
non-goal 10).

---

## Status

**Proposed.** This ADR records the placement & scheduling decision exactly as fixed in
the execution-federation decision memo (§1, §2.2, §3 HQ3/HQ10) and resolves the three
open questions the ADR-E1 stub left open. **Erik ratifies it to Accepted** — that
human gate is deliberately not set here. It is additionally gated on **WG-Fed ADR-001
and ADR-003 being Accepted** (the identity + UCAN substrate it cites), and no execution
code lands until **ADR-E1/E2/E3/E4** are Accepted (memo §5, Exec-Wave A).

---

## Consequences

- **New `src/providers/{mod,placement}.rs`** is the home of the placement mechanism —
  the execution-plane analog of WG-Fed's `src/identity/`. `bundle.rs`, `lease.rs`,
  `verify.rs`, and `attest.rs` arrive with ADR-E2/E3/E4's waves; this ADR creates
  `mod.rs` (wire + registry) and `placement.rs` (matcher) and stubs the seams the
  others fill.
- **`plan_spawn` gains a `placement` field** and **`handler_for_model.rs` gains a
  `RemoteRunner` arm** — the two touch-points that make remote placement expressible;
  `placement = Local` is the default and reproduces today's local spawn exactly
  (NFR-3).
- **`wg claim` becomes capability-gated** — the local-only claim path gains the
  eligibility proof (OQ3) and the `RunGrant` admission gate; existing local claims
  continue to work because a local worker trivially satisfies the trusted-pool filter.
- **Enables FR-P1–P5** (per-authorizer placement, push, capability-gated pull,
  one-mechanism-spans-pools, filter-plus-rank matching). It composes with ADR-E2 (the
  leash supplies the `trust_floor` the filter reads), ADR-E3 (rank reads reputation as
  an advisory hint only; verification is applied independent of rank), and ADR-E4 (the
  `RunGrant` carries the scoped UCANs; claim contention reuses the lease-epoch CAS).
- **No central scheduler ⇒ no global optimization or cross-authorizer fairness.** Each
  authorizer sees only *its* providers' state. Accepted at work-speed (NFR-2); the
  open-market scale case is a non-goal (memo §7 non-goals 1/6).
- **Cost we accept:** the per-authorizer scheduler cannot make a globally-optimal
  placement, and a cooperative gets no auction-grade fairness in v1 (OQ2). Both are
  acceptable for the private→cooperative pools v1 targets and are *named, not silently
  dropped*.

---

## Alternatives rejected

- **A central scheduler / global matching service** (the obvious "lift the dispatcher
  to a shared service" design). Rejected: it is a single point of failure, a capture
  target, and a metadata chokepoint over who-runs-what (HQ10, memo §3) — and it
  violates the WG-Fed HQ6 invariant the whole study inherits. Placement stays
  per-authorizer; central components survive only as **hints that cannot override a
  local check** (D1/D5).
- **A mandatory provider directory in the correctness path** (placement *requires* a
  lookup against a central registry). Rejected: it re-centralizes the trust root and
  makes a forged/seized directory able to mis-place work. The directory is an optional
  *convenience*; the private-pool case needs **zero** central nodes (NFR-6, D5).
- **A separate scheduler per pool class** (one mechanism for the private pool, another
  for a cooperative, a third for a market). Rejected: it breaks the EX6 spine (FR-P4) —
  *one* mechanism must span the spectrum with only `trust_level` + the leash changing
  (D4). Three protocols means three attack surfaces and a re-design every time a
  provider's trust changes.
- **Pull-only / a shared global ready queue providers drain** (the "compute mesh" /
  open-market cherry-pick model). Rejected for v1: it makes the queue a central
  correctness dependency, invites cherry-picking and starvation of hard tasks, and
  needs auction-grade fairness + sybil-resistance that are unsolved (B-i, memo §7
  non-goals 1/6). Pull stays a *first-class option against each authorizer's own
  queue*, gated by the authorizer's `RunGrant` (D2), not a global mesh.
- **Self-authorizing claims** (a provider's `Claim` directly starts a run). Rejected:
  it would let a provider self-certify its own eligibility and bypass the local filter
  (the HQ10 hole). A `Claim` is necessary-but-not-sufficient; the authorizer's signed
  `RunGrant` is the only thing that authorizes execution (D2, OQ3).
- **Rank as a gate** (letting cost/latency/reputation *admit* a provider, e.g. a
  cheap-enough or reputable-enough stranger clears the bar). Rejected: it collapses the
  filter/rank separation and reopens the behave-then-defect hole (P6/X-7) — a high
  reputation could promote a defector past the trust-floor on sensitive work. Rank is
  **advisory and reorders the eligible set only** (D3).

---

## Open questions

The ADR-E1 stub (memo §6) and the memo's handed-off checklist (§8 item 1) left three
questions for this ADR to close. All three are resolved with rationale below; where a
residue is genuinely a tuning/policy value judgment it is **explicitly flagged for
Erik** rather than silently fixed.

### OQ1 — Default rank policy (cost vs latency vs reputation) — **RESOLVED (structure fixed; default ordering flagged for Erik)**

**Resolution — the structure is the commitment, the weights are the knob.** Rank
operates *only* on the filter-surviving eligible set (D3) and is **advisory,
deterministic, and optimization-only**. Three invariants are fixed here and are **not**
Erik's to tune:

1. **Rank never overrides the filter.** It reorders eligible providers; it can never
   admit an ineligible one, and **reputation in the rank never promotes a provider past
   the trust-floor** (HQ4/X-7). The security decision is entirely in the filter.
2. **Rank is deterministic with a herd-safe tiebreak.** Equal-scoring providers are
   ordered by a stable hash of `(task_id, provider_wgid)` — not by raw reputation or
   arrival order — so independent authorizers do not stampede the same "best" provider
   and a tie cannot be gamed by a provider that merely advertises faster.
3. **Rank inputs are locally-held or signed.** Cost is the authorizer's own metered
   rate (`graph::parse_token_usage` / `wg spend`, the budget substrate); latency is the
   authorizer's *observed* liveness/responsiveness, not the provider's self-report;
   reputation is the local/advisory signed score (ADR-E3) — never a central ledger.

**Default ordering (proposed; flagged).** For v1 — whose default tier is the
**authorizer-funded trusted private pool** (memo §2.2, HQ9) — cost is largely internal
(you pay either way within your own pool), so the expensive failure is *stalling or
redoing work*, not overspend. The proposed default is therefore a **lexicographic
"reliability-first"** order over the eligible set:

> **(1) live & has free capacity** → **(2) higher local eval-pass-rate / liveness**
> (a tiebreaker among the trustworthy, never a gate) → **(3) lower metered cost** →
> **(4) deterministic hash tiebreak**.

When the **B verified-overflow tier** is in play (someone else's metered compute,
ADR-E3), a deployment will reasonably want **cost-first**; that is a one-line reweight,
not a redesign, because the *inputs* and *invariants* above are fixed.

*Why.* Putting liveness/free-capacity first directly serves the work-speed goal (NFR-2)
and avoids placing on a saturated or dead provider; keeping reputation a *tiebreak among
the already-trust-floor-eligible* uses reputation exactly the way HQ4 blesses (cheaper
ordering on fungible work) while X-7 keeps it off sensitive work; cost last reflects the
authorizer-funded v1 default where in-pool cost is a wash.

*Flagged for Erik (tuning only):* the **default ordering/weights** — whether v1 ships
reliability-first (proposed) or cost-first, and the per-tier reweight for the B overflow
case — is a deployment-policy value judgment, config-driven and adjustable without
reopening the design. A paranoid org and a cost-sensitive overflow-heavy shop will want
different defaults. The *mechanism* (filter-then-rank, advisory-only, deterministic
herd-safe tiebreak, locally-held/signed inputs, reputation-never-past-the-floor) is the
ADR-E1 commitment; the numbers are sensible defaults Erik can adjust.

### OQ2 — Pull-queue fairness for the cooperative case — **RESOLVED (no auction in v1; minimal anti-starvation flagged)**

**Resolution — v1 ships no auction-grade fairness, by design, and does not need it.**
The memo already scopes auction fairness / anti-cherry-pick *out* of v1 (memo §3 HQ3,
§7 non-goal 6 — "multi-tenant fairness-at-scale"). The reason it is *safe* to defer is
that a v1 cooperative is **vouched/trusted and authorizer-funded**, not an adversarial
market competing for revenue (memo §2.2, §7 non-goal 1). So the failure mode a fairness
auction exists to prevent — providers gaming a payout split — **does not exist in v1**.
What *does* need an answer in a cooperative is two **operational** hazards, and both are
handled **without** an auction:

- **Cherry-picking** (providers `Claim` only the cheap/easy tasks, starving hard ones).
  Bounded structurally because **the authorizer, not the claimer, decides placement**
  (D2): a `Claim` is a request the authorizer may decline. Hard/foundational/root tasks
  are **pushed** to high-trust providers under the cross-task-poison placement
  constraint (foundational ⇒ A/C tier, leaf ⇒ B — memo §1 commitment 3, ADR-E3), so
  they are never left in the queue to be cherry-picked around in the first place.
- **Claim contention / thundering herd** (many providers race the same task). Resolved
  by the **lease-epoch atomic CAS** (D2, shared with ADR-E4): the first granted `Claim`
  wins; losers get a stale-epoch rejection. No global lock, no auction.

**Minimal anti-starvation (proposed; flagged).** As the one positive fairness nudge in
v1, the authorizer applies an **age-based push fallback**: a ready task that stays
**unclaimed past a threshold** is **pushed** to an eligible idle provider rather than
waiting indefinitely for a pull. This is a tiny, local, deterministic rule — not an
auction — and it composes with the cross-task-poison push (hard tasks are pushed
immediately regardless of age).

*Why.* A vouched cooperative's real risk is *unclaimed hard work*, not *unfair revenue
split*; the age-based push fallback + authorizer-decides-placement together bound
starvation and cherry-picking with **zero** auction machinery, keeping v1 within the
"no central scheduler, no market" guardrails (memo §7).

*Flagged for Erik (policy):* whether v1 ships **any** anti-starvation nudge (the
proposed age-based push fallback) or **truly nothing** for the cooperative, and the
**unclaimed-age threshold** if it does, are policy value judgments. The hard commitment
is that **full auction/proportional-share/anti-cherry-pick fairness is a v1 non-goal**,
deferred to the open-market wave where it is actually load-bearing; the open question is
only *how much* of the cheap, local, deterministic anti-starvation nudge to ship now.

### OQ3 — Exact `Claim` eligibility proof — **RESOLVED**

**Resolution.** A `Claim` carries a **four-part eligibility proof**, and the
authorizer's verdict — encoded as issuing-or-withholding the `RunGrant` — is reached
by checking each part **locally**, never by trusting the claimer's self-assertion:

1. **Identity proof.** The `Claim` is **signed by the provider's `wgid:` signer**,
   chained to the provider's sigchain (ADR-fed-001). The authorizer verifies the
   signature locally (never central, HQ10); a forged `from` or a signature not chained
   to an authorized signer is **rejected** (composes with the WG-Fed spark's
   "authenticate by key," memo §4.2 step 3).
2. **Capability proof.** The `Claim` carries the provider's **signed capability
   advertisement** (FR-R4): the model/handler it offers and the isolation class it can
   provide. For **low-trust or confidential** routing, a self-advertised class is
   **insufficient** — the proof must reference an **attestation** binding the
   isolation/TEE class to the provider's `wgid:` (HQ8/TC10, ADR-E2); absent that, the
   task is simply not claim-eligible by that provider. An advertised-but-undelivered
   capability is caught after-the-fact and penalizes reputation (FR-R4, ADR-E3).
3. **Trust-floor proof.** Eligibility requires
   `provider.trust_level ≥ leash(task).trust_floor`, evaluated **by the authorizer
   against its own local `ProviderRegistry` trust record** — **never** asserted by the
   claimer. **Trust is the authorizer's to assert; a provider cannot self-certify its
   own trust level.** A claim for a task whose floor the provider does not meet is
   declined (the `RunGrant` is withheld) — this is the D3 filter, initiated by the
   provider instead of the dispatcher.
4. **Freshness proof.** For a freshness-gated (high-value) task, the authorizer
   **re-fetches and checks the provider's freshness attestation** (signed `as_of` +
   `expires` + monotonic `seq`, ADR-fed-001 OQ4 / the S-3 defense) **before** granting,
   and **fails closed on stale**.

The **invariant that ties it together:** a `Claim` is **necessary but not sufficient**
(D2). Even a perfectly-formed, fully-attested claim does **not** authorize execution;
only the authorizer's signed **`RunGrant`** (the two scoped UCANs + sealed bundle,
ADR-E4/ADR-E2) does. This keeps the HQ10 invariant intact on the pull path: the
provider's self-asserted eligibility is a **request the authorizer independently
verifies and may decline** — a compromised directory or a lying provider can shape what
is *requested*, never what is *granted*.

*Why.* The proof is just the D3 filter expressed as a verifiable claim payload, with
every check rooted in something the authorizer can verify **locally** (a signature, its
own trust record, a re-fetched freshness attestation) — so capability-gating `wg claim`
adds *no* new trust root and no central dependency. Pinning "trust is the authorizer's
to assert, never the claimer's" and "the `RunGrant`, not the `Claim`, authorizes"
closes the self-authorization hole that an open pull queue would otherwise open.

*Not Erik's call* — this is a mechanical security decision following directly from HQ10
+ HQ8 + ADR-fed-001/003; it is settled here. *One efficiency knob is flagged as a future
optimization, not a security question:* whether a **standing pre-enrollment capability
record** (cached in the `ProviderRegistry`, periodically refreshed) may substitute for
an inline capability advertisement on every `Claim` to save bytes. It changes only
*where the same signed advertisement is read from*, not *what is verified*, so it is a
performance choice for the Exec-Wave B/C build, not a placement-model decision.

---

## References

- `docs/execution-federation-study/06-decision-memo-and-roadmap.md` — §1 (the
  decision + the placement-authority call made plainly), §2.1 (the substrate, the
  `src/providers/` skeleton + touch-points), §2.2 (D shipped A-first; the EX6
  one-mechanism spine; the placement-authority row), §2.3 (the WG-Fed compose
  contract), §3 **HQ3** (placement & scheduling — the decision this ADR formalizes),
  §3 **HQ10** (decentralization vs central scheduler + the per-capability table), §3
  HQ4 (provider trust/reputation), §6 ADR-E1 stub, §7 (non-goals 1/6/10), §8 item 1
  (the open-question hand-off).
- `docs/execution-federation-study/02-current-state-baseline.md` — §2.1 (today's
  push-only same-FS dispatcher), §2.5 (the `claim → heartbeat → reclaim` lifecycle
  this lifts cross-host).
- `docs/execution-federation-study/04-candidate-architectures.md` — §1.2 (the five
  execution wire envelopes), §1.6 (the leash policy engine the filter reads
  `trust_floor` from), §1.7 (`src/providers/` skeleton).
- `docs/execution-federation-study/05-adversarial-evaluation.md` (via the memo) —
  §6.1 (push as the nearest WG-fit lift), X-4 (the lease-epoch CAS reused for claim
  contention), X-7 (poisoned reputation cannot promote past the verification gate),
  B-i (open-market sybil — why the market is a non-goal), D-i (fail-closed unlabeled
  default).
- **`docs/ADR-fed-001-identity-key-model.md`** (Accepted) — `wgid:` identity, the
  sigchain, and §D5 "verification is never central; central = a hint that can't
  override self-verify" (the HQ6 invariant this ADR inherits for placement); OQ4 (the
  freshness attestation OQ3 re-checks).
- **`docs/ADR-fed-003-custody-delegation-recovery.md`** (Accepted) — the
  custodian-held root + ssh-agent-style signing boundary and the **attenuating-only
  UCAN** the `RunGrant` carries (the scoped capabilities placement issues; the worker
  never holds root — invent no second delegation system, NFR-4).
- Sibling execution ADRs: **ADR-E2** (confidentiality tier & the attestation slot —
  supplies the leash `trust_floor` and the attested-isolation requirement the D3
  filter reads), **ADR-E3** (result integrity & the verification leash — the advisory
  reputation rank input and the cross-task-poison push constraint), **ADR-E4**
  (capability & lease lifecycle — the `RunGrant` UCANs and the lease-epoch CAS reused
  for claim contention).
