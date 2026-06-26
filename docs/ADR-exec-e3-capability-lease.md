# ADR-E3 (Exec): Capability & Lease Lifecycle — Two Scoped Attenuating UCANs + CAS Lease-Epoch Fencing

**Status:** Proposed
**Date:** 2026-06-26
**Decision:** A worker on a separately-owned provider receives **two scoped,
attenuating UCANs — never the agent's root key**: an *act-as-agent UCAN* ("run task
T as agent G," + expiry) and a *graph-write UCAN* ("log/append/artifact/done on task
T only" — **not** blanket graph write). Both ride the **leash dial** — **broad/long
by default** for a trusted pool (the trust-default amendment), automatically
narrowed/shortened by policy for strangers. Real authority is kept *off* low-trust
providers by an **intent-bound, rate-limited, budget-metered, logged privileged-op
callback** (an ssh-agent-style "do this," never a signing oracle). Revocation =
**short TTL** (where the dial tightens it) + **issuer-subtree** + a **write-time
check at the authorizing graph** (FR-C3). The cross-host claim is a **lease**:
liveness is a **signed `LeaseRenewal` judged by the *authorizer's* observation** of
accepted renewals (not the provider's self-report); double-execution is fenced by a
**monotonic lease epoch enforced by an atomic compare-and-set at the single
canonical-graph write boundary**; the reclaim stance is **prefer-liveness** (reclaim
fast — the fence dedupes). Delegation, custody, and revocation are **WG-Fed's UCAN
mechanism, reused verbatim** — no second delegation system is invented (NFR-4).

> **This ADR formalizes the memo's load-bearing capability/lease stub.** The
> decision was *made* in the execution-federation decision memo
> (`docs/execution-federation-study/06-decision-memo-and-roadmap.md` §3 HQ5/HQ6,
> §6 stub, §8 hand-off); this ADR formalizes it, **incorporates Erik's
> trust-default / leash-as-a-dial amendment** (broad-by-default), and resolves the
> three open questions the stub left open. It is **not** a re-litigation of the
> architecture choice (`WG-Exec` = Candidate D's leash-selector, shipped A-first) —
> that is settled.
>
> **A numbering note (read before cross-referencing).** The decision memo §6 labels
> this stub **ADR-E4** ("Capability & lease lifecycle") and labels result-integrity
> **ADR-E3**. The Exec-Wave A **task graph renumbers them**: this document is
> **ADR-E3 (capability & lease)** and result-integrity is **ADR-E4** — *same
> decisions, swapped labels*. Wherever the memo says "ADR-E4 (capability/lease)" it
> means this file (`docs/ADR-exec-e3-capability-lease.md`); wherever it says
> "ADR-E3 (result integrity)" it means `docs/ADR-exec-e4-*.md`. The four are
> packaged by `exec-adr-coherence`, which uses the task-graph numbering.
>
> **What this ADR builds on (the explicit compose boundary, NFR-4).** Identity
> (`wgid:`), the sigchain, the custodian-held root behind an ssh-agent-style "sign
> this digest" boundary, and the UCAN format + its integrity invariants are
> **WG-Fed's** (`docs/ADR-fed-001-identity-key-model.md`,
> `docs/ADR-fed-003-custody-delegation-recovery.md`). This ADR *consumes* them
> across a host boundary; it defines **no** identity, **no** new crypto, and **no**
> second delegation system. What is *this study's* to own is the **execution wire**
> (`PlacementOffer`/`Claim`/`RunGrant`/`LeaseRenewal`/`ResultEnvelope`), the
> **lease-epoch fence**, and **how the leash dial wires UCAN scope/TTL + lease
> term/cadence together per task**.

---

## Context

The worker no longer runs on a machine the authorizer owns. Under federation it runs
on a **separately-owned provider** that may **leak, steal, copy, or replay whatever
the worker holds**, and that may **lie about being alive** while squatting a claim
(`docs/execution-federation-study/03-requirements-and-hard-questions.md` HQ5/HQ6).
Two hard problems follow, and this ADR owns both.

**The capability problem (HQ5).** *"Ship too much and a single leaky provider
impersonates the agent everywhere; ship too little and the worker can't do its job"*
(doc 03 HQ5). The wall today's same-filesystem spawn cannot cross is that the local
worker simply shares the authorizer's ambient credentials and `.wg/` symlink (doc 02
§2.1/§2.2a). Across a trust boundary that is catastrophic: the worker's credential is
now exfiltratable, so its **blast-radius-if-stolen must be small and its scope must
be exactly the task's needs** — and the agent's **root key must never leave the
custodian** (the WG-Fed S-1/S-2 custody boundary, now stretched across a host). The
adversarial pass rates attenuating-only + short-TTL UCAN the **best TC4 posture**
(doc 05 §4.2): a leaked credential is bounded to *one task, for a bounded window*,
and **cannot widen its own scope** (the hydra kill, WG-Fed S-4 / ADR-fed-003 §D3).

**The liveness problem (HQ6) — the distributed orphan.** WG already has a mature
`claim → heartbeat → reclaim` lifecycle (doc 02 §2.5; the local liveness timeout is
`HEARTBEAT_LIVENESS_TIMEOUT_SECS = 300` s with a 30 s heartbeat cadence). But across
a host boundary `is_process_alive()` is **meaningless** — liveness is not a local
PID, it is "did the authorizer accept a fresh signed renewal?" (FR-L3). And reclaim
opens a **double-execution hazard** (FR-L2): the authorizer reclaims a stalled
worker and re-places the task; the original worker was merely *partitioned*, wakes,
and tries to commit — now two workers race to write one task's result. This is the
**best-defended area in the whole study** (doc 05 X-4) precisely because the fix is
known and small: a **monotonic fencing epoch** checked by an **atomic compare-and-set
at the single canonical-graph write boundary**, where a TOCTOU would reopen the
double-commit (X-4, so the atomicity is *mandatory*, not advisory).

**The amendment that sets the default.** WG-Fed's ADR-003 §D2 carries **Erik's
trust-default / leash-as-a-dial amendment**: it separates **custody** (where the root
lives — fixed, non-negotiable, costs the agent no autonomy) from **authority scope**
(what the agent may do, for whom, for how long — *a dial*). The memo's original
framing set the dial's default to the *tight* end ("short-lived UCAN per session,"
§3 HQ5/HQ11). **That is reversed as the birth default: broad and long-lived
authority** — agents and humans are *first-class peers, not tools*; the short, scoped
"leash" is **environment-driven policy**, never the default; **humans are never
leashed**. This ADR inherits that amendment and applies it to the execution plane's
two UCANs and its lease (§D2, OQ1, OQ3). Crucially — and this is the load-bearing
argument WG-Fed already made — **the amendment does not reopen any Fatal finding**,
because the integrity defenses are the *attenuating-only* invariant and the
*write-time / fencing* checks, which hold **at every dial setting**, never the short
TTL (ADR-fed-003 §D2/§D3).

Delegation here is **WG-Fed's UCAN, period** (NFR-4): the act-as-agent and
graph-write capabilities are issued via ADR-fed-003 §D3's `delegate`/UCAN machinery,
custodian root unchanged, attenuating-only and `add_key`/`rotate_root`-locked
unchanged. This ADR is **how that capability flows to a worker on a borrowed box, and
how the borrowed box's claim is leased and fenced** — nothing about identity or
crypto is re-invented.

---

## Decision

### D1 — Two scoped attenuating UCANs, never the root key (the capability split)

A `RunGrant` delivers the worker **exactly two capabilities, and a private key is
never one of them** (FR-C1, FR-C2):

- **The act-as-agent UCAN** — `iss` = the agent G's authorized signer (chained to
  G's sigchain, custodian-held root never leaving the custodian), `aud` = the
  provider/worker's enrolled signer, capability = **"run task T as agent G,"** with
  an expiry. This authorizes the worker to *act as G for this task* and to attribute
  its result to G (the signature ADR-E4/result-integrity verifies, FR-C5) — it does
  **not** hand over G's key (delegation never shares a private key, ADR-fed-003 §D3).
- **The graph-write UCAN** — capability = **task-T-scoped graph writes only**:
  `log`/`append` to T, write `artifact`s under T, mark T `done`, and *optionally* a
  scoped subtask-create under T. This is the **one non-negotiable floor even on a
  fully trusted pool** (FR-C2): **never blanket graph write.** A worker delegated for
  T **cannot** mutate task U, another agent's record, or the graph's structure
  outside T's subtree.

**Why two, and why split.** Authority-to-act and authority-to-write are *different
scopes with different blast radii*: the act-as-agent capability is the impersonation
surface (it must be intent-bound and short where the provider is a stranger), while
the graph-write capability is the integrity surface (its task-scoping is what
**bounds a forged result to corrupting its own task** — FR-V4, the result-integrity
ADR's blast-radius bound). Splitting them lets the leash tighten each independently
and lets the write UCAN be the structural cap on damage regardless of how the
act-as-agent UCAN is calibrated.

**The bytes carry no root and no blanket write — and this is *testable*.** The
execution spark (memo §4.2 step 1) field-scans the `RunGrant` bytes delivered to the
provider and asserts they contain **no root key** and **no blanket graph-write
capability** — only the two scoped UCANs. This is the execution-plane analog of the
WG-Fed spark's "downloaded identity ≠ impersonation."

*Why.* Attenuating-only + scoped bounds a leaked credential's blast radius to *one
task* (doc 05 §4.2, TC4-best). The split is WG-Fed's UCAN applied at the worker
boundary — *no second delegation system* (NFR-4).

*Cost.* The worker cannot do anything outside T without going back to the authorizer
(the callback, §D4) — accepted: that chatter *is* the leash holding.

### D2 — Scope and TTL ride the leash dial — broad/long by default (the amendment)

The two UCANs' **scope breadth and expiry are policy outputs of the one `leash()`
function**, not hardcoded constants at the issue site (FR-C4):

```
leash(provider_trust, task_sensitivity, pool_class, env_config)
    → { delegation { scope, ttl }, … , lease { term, renew_cadence } }
```

Per **Erik's trust-default amendment** (ADR-fed-003 §D2), the **default is genuinely
slack**:

- **Trusted pool (`Verified` provider, normal sensitivity) → broad scope, long
  expiry.** A trusted pool member is a *first-class peer*: it may hold a **standing,
  sigchain-authorized scoped signer** covering many tasks (the leash-slack default,
  HQ5 "a trusted pool member *may* hold a broader standing scoped signer"), and its
  per-task UCANs are issued **long** — comfortably exceeding the expected task/lease
  lifetime, *not* a tight per-renewal leash. This default path is **offline-friendly
  by construction**: a broad, long-lived capability needs **no chatty re-issuance**,
  which matters *more* in the exec plane than in WG-Fed because every re-issuance is a
  cross-host round-trip to the authorizer (NFR-2, email-speed). The amendment *helps*
  the exec plane's async budget; it does not fight it (OQ1).
- **Stranger (low-trust / `Unknown`) → narrow scope, short expiry.** The dial
  tightens **automatically by policy** (no code change, no per-issuance hardcoding):
  the act-as-agent UCAN is clamped to a single task and a short window, the
  graph-write UCAN to the minimal verb set, and the privileged-op callback (§D4)
  carries the rest off-box. Tightening **opts into** re-issuance chattiness *as a
  chosen cost* for a smaller blast radius and faster revocation — an explicit,
  *local* decision, never a global default everyone pays.

**A too-loose UCAN on a low-trust provider is impossible by construction.** Mirroring
the leash-engine invariant (memo §2.2/HQ11), the `leash()` function **cannot emit
broad-scope + long-TTL for `provider_trust < floor`** — it returns the
narrowed/shortened region or refuses. The dial moves UCAN scope, UCAN TTL, **and**
lease term/cadence (§D5–D7) **coherently in one place**, so the capability lifetime
and the lease lifetime never drift apart (a stranger gets *short UCAN + short lease*
together; a trusted peer gets *long-or-standing UCAN + long lease* together).

**Why the broad default is safe (the amendment's load-bearing argument, applied
here).** The integrity of the capability flow rests on three controls that hold **at
every dial setting**, none of which is the short TTL:

1. **Custody** — the root is custodian-held and never on the worker (§D1, WG-Fed
   §D1): a stolen *broad* signer is still **not the root** and degrades to
   revocation, not permanent takeover.
2. **Attenuating-only** — a broad signer **cannot widen its own scope** and
   **cannot `add_key`** (the hydra kill, ADR-fed-003 §D3); broad ≠ unbounded.
3. **The task-scoped write UCAN + the lease-epoch fence** — a forged result is capped
   to its own task (FR-V4) and a stale/partitioned worker's write is rejected (§D6).

What a long expiry changes is only the **detection-to-revocation window** — a
blast-radius/latency trade, and *exactly* what the dial tightens where that trade is
unacceptable (OQ1).

*Why.* The vision (V7) and the social-network north star treat agents as **peers**,
not short-lived tools on a per-session leash; a birth default of "containment"
silently re-casts agents as tools (ADR-fed-003 §D2). The security never depended on
the leash being the default — it depends on custody + the integrity invariants, both
dial-independent.

*Rejected.* **Short-session-leash as the birth default** (the memo's original HQ5/HQ11
phrasing) — rejected per Erik's amendment; the short leash survives as
environment-driven policy. **Hardcoding scope/TTL at the issue site** — the five
dials drift incoherently; they must come from one `leash()` call (HQ11).

### D3 — Revocation: short TTL (where tightened) + issuer-subtree + write-time check

A delegation is **expiring and revocable** (FR-C3). Because the default is now
long-lived (§D2), **revocation — not expiry — is the primary kill-switch**, exactly
as WG-Fed re-weighted it (ADR-fed-003 §D3). Three composed layers, **all WG-Fed's,
reused verbatim** (NFR-4):

1. **Short TTL** *where the dial tightens it* — on a stranger the UCAN expiry is
   clamped to ≈ one lease term, so a reclaimed/abandoned grant also self-expires
   (defense in depth: even if the fence had a bug, the UCAN dies). On the trusted
   pool the TTL is long and this layer contributes little — by design.
2. **Issuer-subtree revocation** (ADR-fed-003 §D3) — the authorizer is the `iss` of
   both UCANs; revoking the parent **kills the whole delegated subtree**. **Killing
   the lease kills the grant**: reclaim (§D6) revokes the issued UCANs as part of the
   same operation, so a reclaimed worker's capabilities are dead, not merely stale.
3. **A write-time check at the authorizing graph** (FR-C3) — *the exec plane's
   decisive simplification.* Every result/log/artifact write flows back to the
   **canonical graph at the authorizer** (the single-writer spine, HQ7). The
   authorizer **is** the `iss`, **is** the revocation authority, **and is** the party
   performing the write-time accept check — so "is this capability revoked?" is
   answered **locally at the authorizer at accept time**, needing **no external
   revocation lookup on the safety-critical path** (this collapses OQ2's "lookup
   dependency" worry — see OQ2). A write presented under a revoked or expired UCAN is
   **rejected at the boundary**, full stop.

*Why.* Under a broad/long default, leaning on a 15-minute TTL as the primary
revocation path is exactly the leash we moved off the default (ADR-fed-003 §D2/§D6);
the durable, self-verifying subtree-revoke + the authorizer-local write-time check
are what make revocation *effective* without a short TTL or a central CRL.

*Rejected.* **Expiry-as-the-only-revocation** (fights offline tolerance, the leash we
moved off the default). **A mandatory central CRL lookup** before every write (a
censorship/availability single point — OQ2; the write-time check is authorizer-local
instead).

### D4 — The privileged-op callback: real authority off low-trust providers, not a signing oracle

For anything the worker should **not** hold standing authority to do on a low-trust
provider — using a long-lived API key, signing outside the task scope, a privileged
graph op beyond the two UCANs — the worker **calls back to the authorizer** with an
**ssh-agent-style "do this specific thing"** request (HQ5, the secrets-off-untrusted
posture, FR-K2). Authority **stays at the authorizer**; the provider gets a *result*,
never the *capability*.

The callback is **deliberately not general**, so it cannot be turned into a
confused-deputy oracle (doc 05 X-3). It is:

- **Intent-bound** — "sign *this digest* for *this purpose* / run *this* privileged
  op," never "sign anything" / "do anything" (the WG-Fed S-2 boundary, ADR-fed-003
  §D1).
- **Rate-limited and budget-metered** — bounded calls/sec and a spend ceiling
  (reusing `graph::parse_token_usage` / `wg spend` as the budget substrate, HQ9/R32),
  so it cannot be milked as a free-inference proxy or a signing oracle.
- **Logged** — every request is an auditable record (NFR-7), so abuse is detectable
  after the fact and attributable to the provider.

A **trusted** pool member, by the leash-slack default (§D2), may instead hold a
**broader standing scoped signer** and skip most callbacks — still revocable, root
still never leaving the custodian. The callback is the *stranger's* mechanism; the
standing signer is the *peer's*.

*Why.* Attenuating-only handles "the worker can't widen its delegated scope"; the
callback handles "the worker should never *hold* the credential at all" — together
they keep a leaky low-trust provider from ever holding standing authority (best TC4
posture, doc 05 §4.2).

*Rejected.* A **"do-anything" bearer-token callback** — the confused-deputy oracle
(X-3); the callback is intent-bound, rate-limited, metered, logged.

### D5 — The cross-host lease: signed `LeaseRenewal`, authorizer-judged liveness

A remote claim is a **lease**: the provider holds task T for a **bounded, renewable
term**; failure to produce an accepted renewal makes T **reclaimable** (FR-L1). This
is WG's existing `claim → heartbeat → reclaim` lifecycle (doc 02 §2.5) **lifted
across the trust boundary and made cryptographic**:

- **Liveness is a signed `LeaseRenewal` judged by the *authorizer's own observation*
  of accepted renewals** (FR-L3) — **not** the provider's self-report. A provider
  that lies "still alive" but produces no accepted renewal/result is reclaimed when
  the lease lapses. `is_live()` consults the **last-accepted `LeaseRenewal`**, not
  `is_process_alive()` (which is meaningless across a machine boundary).
- The `LeaseRenewal` is signed by the worker's delegated signer (the act-as-agent
  UCAN, §D1), so a renewal is **attributable and unforgeable** by a third party — a
  relay cannot fake "P is alive."
- **Lease term and renew cadence ride the dial** (FR-L4, §D7): long/relaxed for
  trusted, short/aggressive for strangers — the *same* `leash()` output that sets the
  UCAN scope/TTL (§D2), so capability and lease lifetimes stay coherent.

*Why.* This is the study's best-defended area (doc 05 X-4): it inherits WG's mature
lease lifecycle, made cross-host and cryptographic. Authorizer-judged liveness is the
only liveness that survives a *lying* provider (FR-L3).

*Rejected.* **Provider self-report as the liveness source** (a hostile provider
squats forever — FR-L3).

### D6 — Lease-epoch fencing: monotonic epoch via atomic compare-and-set at the canonical-graph write boundary

Reclaim is **safe against double-execution** by a **monotonic lease epoch** (FR-L2,
the fencing-token pattern):

- Each placement of T carries a **lease epoch** `e`. **Reclaim increments the epoch**
  (`e → e+1`) and re-places T to a new worker with the higher epoch (and a fresh
  `RunGrant`; the old worker's UCANs are revoked as part of reclaim, §D3).
- A `ResultEnvelope` / `LeaseRenewal` write carries the epoch the worker holds. The
  canonical graph **accepts a write only if its epoch equals the current epoch**, via
  an **atomic compare-and-set (CAS)** at the **single canonical-graph write boundary**
  (the authorizer). A **late-returning, merely-partitioned** worker presents a
  **stale epoch** (and an expired/revoked delegation) and its write is **rejected at
  the boundary** — no double-commit.
- **The CAS is mandatory, not advisory.** A read-then-write (check the epoch, then
  write) has a TOCTOU window where two workers both pass the check and both write
  (doc 05 X-4); the compare-and-set must be **atomic** at the one write boundary.
  This is a small, well-understood concurrency primitive on the `graph.jsonl`
  write-path — the *exact* primitive is OQ flagged in the memo (memo §8 item 6) and
  is an implementation detail for Exec-Wave B's `lease.rs`, not a design fork.

**There is one canonical-write boundary, and it is the authorizer.** The provider
holds a *slice* and writes back *deltas*; the canonical graph stays at the authorizer
(HQ7), which is the *only* place the epoch CAS runs — so there is no distributed
consensus to get wrong, just one atomic check at one writer.

*Why.* Fencing tokens are the textbook defense against a partitioned-worker
double-commit (doc 05 X-4). One canonical writer + an atomic epoch CAS makes
**prefer-liveness** (§D7) safe: you can reclaim aggressively because the fence
guarantees only one writer's result ever lands.

*Rejected.* **Read-then-write epoch checking** (TOCTOU reopens the double-commit —
X-4). **First-writer-wins without a fence** (a stale partitioned worker that returns
*first* would win over the legitimate reclaim-placed worker).

### D7 — Lease term + renew cadence ride the dial; prefer-liveness

**Lease term** (how long a claim is held before lapsing without renewal) and **renew
cadence** (the heartbeat interval the worker must beat) **both ride the leash dial**
(FR-L4), from the same `leash()` call that sets UCAN scope/TTL (§D2). Under the
broad-by-default amendment:

- **Trusted pool → a long, relaxed lease** (generous term, infrequent renewal); a
  trusted peer that goes briefly quiet is **not** aggressively reclaimed (peerhood +
  offline tolerance). Safe because the epoch fence (§D6) dedupes any late return.
- **Stranger → a short, aggressive lease** (short term, frequent renewal); a
  squatting or dead stranger is reclaimed fast.

The **reclaim stance is prefer-liveness** (reclaim fast; the fence dedupes) — a
*decision*, not a tuning knob: don't-reclaim-until-certainly-dead (prefer-safety)
stalls the graph, and the CAS fence is *exactly* what makes prefer-liveness safe (you
cannot double-commit, so reclaiming a live-but-partitioned worker costs at most one
wasted re-run, never a corrupt graph). **Squatting** is bounded by lease caps +
no-progress reclaim + a reputation penalty (the result-integrity ADR's reputation is
advisory; the *structural* bound here is the lease cap + fence). Concrete per-trust
defaults are **OQ3**.

*Why.* Trust-scaled lease term is FR-L4 / EX6 applied to liveness; prefer-liveness is
safe **only because** of the fence (§D6) — the two are a pair.

*Rejected.* **Prefer-safety** (stalls the graph; unnecessary given the fence).
**Fixed (non-trust-scaled) lease term** (a stranger gets the same generous term as a
peer — wrong blast radius for squatting).

---

## Status

**Proposed.** This ADR records the decision exactly as fixed in the
execution-federation decision memo (§3 HQ5/HQ6, §6 capability/lease stub),
**incorporates Erik's trust-default / leash-as-a-dial amendment** (broad-by-default,
§D2), and resolves the three open questions the stub left open (below). **Erik
ratifies it to Accepted** — that human gate is deliberately not set here. **No
execution code lands until ADR-E1/E2/E3/E4 are Accepted** (memo §5, Exec-Wave A), and
this ADR additionally depends on **WG-Fed ADR-001 (identity) and ADR-003
(custody/UCAN) being Accepted** — the substrate it consumes. Downstream:
`exec-adr-coherence` packages this with ADR-E1/E2/E4 for Erik's ratification, and the
**hard sequencing dependency** stands — the two scoped UCANs *are* WG-Fed Wave 6, so
the **Exec Spark (Exec-Wave B) cannot complete before WG-Fed Wave 6** (an interim
A-tier preview using a Wave-5 standing signer is allowed *only* behind the
fail-closed leash refuse-row: trusted-pool, non-confidential, normal-sensitivity).

---

## Consequences

- **New `src/providers/lease.rs`** — the signed `LeaseRenewal`, the monotonic lease
  epoch, and the **atomic-CAS fence** at the canonical-graph write boundary. The
  agent registry gains a `ProviderEntry` whose `is_live()` consults the
  **last-accepted `LeaseRenewal`**, not `is_process_alive()` (the cross-host liveness
  fix, FR-L3). Reuses WG's `claim → heartbeat → reclaim` lifecycle and lifts today's
  local `HEARTBEAT_LIVENESS_TIMEOUT_SECS`/heartbeat-cadence constants onto the dial.
- **The graph write-path gains a mandatory epoch CAS** on the `graph.jsonl` writer
  (the one canonical-write boundary). A write whose lease epoch ≠ the current epoch is
  rejected; the exact CAS primitive is an Exec-Wave B implementation detail (memo §8
  item 6), the *requirement* (atomic, single-boundary) is fixed here.
- **Capability issuance flows through the `leash()` engine** (`src/providers/
  placement.rs`): the `RunGrant` carries the **two scoped UCANs** whose `{scope, ttl}`
  are `leash()` outputs (§D2), issued via **WG-Fed's `src/identity/custody.rs`**
  (`delegate`/`issue`/`verify`/`revoke`) — *no new delegation code* (NFR-4). The
  write UCAN's verb set is the task-T floor (FR-C2).
- **The privileged-op callback** is a new authorizer endpoint: intent-bound,
  rate-limited, budget-metered (via `wg spend`/`parse_token_usage`), logged (§D4).
  Low-trust providers route privileged ops through it; trusted-pool members may hold a
  standing scoped signer and skip it.
- **Revocation reuses WG-Fed's path verbatim** (sigchain `revoke_key` +
  issuer-subtree UCAN revoke + freshness, ADR-fed-003 §D3/§D6) **plus** the exec
  plane's authorizer-local **write-time check** (FR-C3) — the safety-critical
  revocation check needs **no external lookup** (OQ2).
- **The applied capability + lease are surfaced** in `wg show <task>` / `wg providers`
  (mirroring the handler-first `wg status` rendering, CLAUDE.md): the UCAN
  scope/expiry, the lease term/epoch, and which trust class produced them — so a
  mis-set dial is visible, and a **leash lint** rides `wg config lint` (memo HQ11).
- **Enables FR-C1–C5 and FR-L1–L4.** Composes with the result-integrity ADR (E4): the
  graph-write UCAN's task-scoping is the **blast-radius bound** on a forged result
  (FR-V4); the act-as-agent signer is what **attributes** the result to G (FR-C5/V1).
- **Cost we accept:** on a low-trust provider the worker **chatters back** to the
  authorizer for privileged ops (the callback) and re-issues short-lived
  UCANs/renewals (cross-host round-trips) — accepted, and *exactly* what the dial
  tightens only where the trade is wanted; under the broad default the trusted-pool
  path pays none of this. A **mandatory CAS** on the graph write-path (small, paid per
  write). Under the broad default, a stolen *broad* signer's detection-to-revocation
  window is longer than a short-TTL design — accepted as the price of first-class
  peerhood, **tightenable by the dial** (§D2, OQ1).

---

## Alternatives rejected

- **A standing root or blanket-write credential on the worker** (today's
  same-FS posture, doc 02 §2.1). A leaky provider impersonates the agent everywhere
  and can mutate the whole graph. Rejected: two scoped attenuating UCANs, root
  custodian-held, write UCAN task-scoped (§D1, FR-C1/C2).
- **A single combined "act + write" capability.** Conflates the impersonation surface
  with the integrity surface, so the dial cannot tighten them independently and the
  write-scope cannot be the structural blast-radius cap. Rejected: two UCANs (§D1).
- **A "do-anything" bearer-token callback.** The confused-deputy signing/inference
  oracle (doc 05 X-3). Rejected: intent-bound, rate-limited, budget-metered, logged
  (§D4).
- **Short-session-leash as the *birth default*** (the memo's original HQ5/HQ11
  phrasing). Conflates custody (costs no autonomy) with authority (the dial) and
  defaults the network to treating agents as tools. Rejected per Erik's amendment
  (§D2); the short leash survives as environment-driven policy.
- **Inventing a second delegation/capability system for the exec plane.** Rejected
  outright (NFR-4): the two UCANs *are* WG-Fed's UCAN, issued/verified/revoked by
  `src/identity/custody.rs`; the exec plane adds the *wire* and the *lease*, not new
  crypto.
- **Provider self-report as the liveness source** (FR-L3). A hostile provider squats
  forever claiming "alive." Rejected: liveness is the authorizer's observation of
  accepted signed renewals (§D5).
- **Read-then-write epoch checking** (a TOCTOU window reopens the double-commit, doc
  05 X-4). Rejected: an **atomic compare-and-set** at the single canonical-write
  boundary (§D6).
- **Prefer-safety reclaim** (don't-reclaim-until-certainly-dead). Stalls the graph,
  and is unnecessary because the fence makes prefer-liveness safe. Rejected:
  prefer-liveness + epoch fence (§D6/§D7).
- **A mandatory central CRL** consulted before each write. A censorship/availability
  single point and a freeze target (S-3). Rejected: the safety-critical revocation
  check is authorizer-local at write time; cross-verifier discovery rides WG-Fed's
  fail-safe cascade (§D3, OQ2).

---

## Open questions

The three open questions the task carries. Two **inherit a WG-Fed resolution
verbatim** (they are the same questions, one trust layer down) and are resolved by
reference + an exec-specific sharpening; the third is exec-specific and is resolved
with a proposed per-trust-class default. Where a residue is a genuine tuning /
governance value judgment it is **flagged for Erik**, not silently fixed.

### OQ1 — UCAN expiry defaults vs offline-chattiness (under broad-by-default) — **RESOLVED (inherits WG-Fed ADR-003 OQ1; exec-sharpened; exact TTLs flagged)**

**This is WG-Fed ADR-003 OQ1, one layer down**, and inherits its resolution: under
the amendment the **default expiry is long** ("**valid until revoked, with a long
sanity ceiling**," bounded by issuer-subtree revocation + freshness, **not** by a
short TTL), so the tension **largely dissolves at the default** and reappears only
when the environment dial tightens it.

**Exec-specific sharpening (why the amendment helps the exec plane *more*).** In the
execution plane the worker is on a **different host**, so every re-issuance is a
**cross-host round-trip to the authorizer** — chatty re-issuance is *more* expensive
here than in WG-Fed's same-host case, which is an **additional** argument for
long-by-default on the trusted pool and for reserving short-TTL chattiness for the
*low-trust* dial. Two further exec nuances:

- **The two UCANs have a natural lifetime bound: the task/lease.** Even under
  broad-by-default, a per-task act-as-agent / graph-write UCAN is *naturally* scoped
  to T's lifetime — its sensible default expiry is **lease-lifetime + a generous
  grace** (e.g. `expiry ≈ lease_term × k`, or "valid until T is `done` or revoked,
  with a sanity ceiling"), **not** "until revoked forever." The "long-lived / until
  revoked" end of WG-Fed OQ1 applies to a **standing trusted-pool signer** (the
  many-task leash-slack option), not to a single task's grant.
- **UCAN expiry and lease term move together on the dial** (§D2/§D7) — a stranger's
  short UCAN ≈ its short lease term, so a reclaimed lease's UCAN is also near-expiry
  (defense in depth). This coupling is the mechanism; the numbers are tuning.

*Flagged for Erik (tuning, shared with WG-Fed ADR-003 OQ1 / ADR-001 OQ4):* the
**standing-signer sanity ceiling** on the trusted pool (30 / 90 days vs "until
revoked"), the **per-task grace multiplier** `k` (e.g. expiry = 2–4× expected lease
term), the **high-value short-Δ** the corporate profile uses (the memo floated
≤15 min, shared with the freshness Δ), and **which exec scopes are "high-value" by
default** — the obvious candidates are a graph-write UCAN bearing **`done`** (a forged
"done" closes a task) and any **standing** (multi-task) signer. The *mechanism*
(long/standing-by-default, per-task ≈ lease-lifetime, revocation-not-expiry as
primary, dial-tightenable, expiry-couples-to-lease) is the ADR commitment; the
numbers are Erik's to set without reopening the design.

### OQ2 — Revocation-list hosting (itself a lookup dependency) — **RESOLVED (inherits WG-Fed ADR-003 OQ2; exec plane collapses the safety-critical lookup to authorizer-local)**

**This is WG-Fed ADR-003 OQ2, one layer down**, and inherits its resolution: host
revocation the way WG-Fed hosts everything — **self-verifying bytes over a fail-safe
cascade, never a mandatory central CRL**, with the issuer-subtree revoke
**piggy-backed on the freshness attestation** a verifier *already* re-fetches, so
learning "is this revoked?" is the *same fetch* as "is this fresh?" — **no new lookup
dependency** (ADR-fed-003 §D6 / ADR-001 OQ4 cascade). Reused verbatim (NFR-4).

**Exec-specific sharpening (the lookup-dependency worry largely *evaporates* on the
critical path).** The exec plane has a structural advantage WG-Fed's general async
path lacks: a **single canonical-write boundary that is the issuer itself**. Every
result/log/artifact write returns to the **authorizer's** canonical graph (HQ7), and
the authorizer **is** the `iss` of both UCANs, **is** the revocation authority, **and
is** the party doing the write-time accept check (§D3). So the revocation check that
**matters for safety** — *rejecting a write under a revoked capability* — is performed
**authorizer-locally at accept time**, against the authorizer's **own** revocation
state, needing **no external CRL fetch at all**. A revoked or partitioned worker is
simply *rejected at the boundary*; it does not need to learn it was revoked to be
*prevented*. External revocation **discovery** matters only for *other* verifiers (a
disjoint provider Q re-verifying provenance for the result-integrity ADR, or a
downstream consumer), and those ride WG-Fed's freshness-piggy-backed cascade exactly
as inherited — fail-closed on stale for high-value actions.

*Flagged for Erik (policy, shared with WG-Fed ADR-003 OQ2):* whether to *also* expose
an **optional, non-authoritative** CRL-style aggregate (a convenience one-fetch for
clients that want it — it must never override a self-verification), the **default
staleness Δ** for the *other-verifier* revocation/freshness check (shared with the
freshness Δ), and one **exec-specific courtesy call**: whether a reclaimed/revoked
provider should get a **best-effort "your lease was reclaimed" notice** so it stops
burning compute — recommended as a *convenience hint only* (the safety guarantee is
the authorizer-local write-time reject regardless of whether the notice arrives). The
*mechanism* (self-verifying, cascade-hosted, freshness-piggy-backed, **safety-check
authorizer-local**) is the commitment.

### OQ3 — Lease term + renew-cadence defaults per trust class — **RESOLVED (mechanism + proposed per-class defaults; exact numbers flagged)**

**Mechanism (the ADR commitment).** Lease **term** and **renew cadence** are
**`leash()` outputs that ride the dial** (FR-L4, §D7), from the *same* call that sets
UCAN scope/TTL (§D2) — long/relaxed for trusted, short/aggressive for strangers — and
the reclaim stance is **prefer-liveness** made safe by the epoch fence (§D6). The
lease term is **primarily trust-scaled** and *secondarily* **task-size-scaled** (doc
03 HQ6 floated task-size scaling): a long-running task earns a longer term, but the
trust class is the dominant multiplier.

**Proposed per-class defaults (flagged for Erik), anchored to today's local values**
(`HEARTBEAT_LIVENESS_TIMEOUT_SECS = 300` s, heartbeat cadence 30 s):

| Trust class | Lease term (no accepted renewal ⇒ reclaim) | Renew cadence | Reclaim posture |
|---|---|---|---|
| **`Verified` (trusted pool)** | **Long / relaxed** — generous, e.g. ≈ 30–60 min or `expected_task_duration × slack`; brief quiet tolerated | **Relaxed**, e.g. every 5–10 min (a multiple of today's 30 s local cadence) | Patient — a peer that goes briefly quiet is not aggressively reclaimed (fence dedupes) |
| **`Provisional` (cooperative)** | **Moderate** — e.g. ≈ today's local 5 min (`300` s) | **Moderate**, e.g. every 1–2 min | Standard reclaim on lapse |
| **`Unknown` (stranger / low-trust)** | **Short / aggressive** — e.g. ≤ a few min | **Aggressive**, e.g. every 30–60 s | Fast reclaim on first missed renewal; squat caps apply |

Two couplings are part of the mechanism, not tuning: **(a)** the renew cadence is
bounded *below* by the cross-host RTT to the authorizer (a renewal costs a
round-trip), so the low-trust aggressive cadence is *also* where the OQ1 chattiness
bites — **same trade, same dial**; **(b)** the low-trust UCAN expiry (OQ1) should be
≈ the lease term, so a reclaimed lease's grant is also near-expiry.

*Flagged for Erik (tuning):* the **exact term/cadence numbers** per trust class (the
table is a *proposal* anchored to today's `300`s/`30`s, not a fixed constant),
whether the lease term is **purely trust-scaled or also task-size-scaled** (the ADR
recommends *trust-scaled primary × task-size factor*), and confirmation that
**prefer-liveness** is the right default reclaim stance (it is a *decision* here, made
safe by the fence — flagged so Erik can veto in favor of prefer-safety for a paranoid
profile). The *mechanism* (dial-driven, trust×task-size, prefer-liveness-via-fence,
cadence-couples-to-RTT-and-OQ1) is the commitment; the numbers are Erik's to set.

---

*Exec-Wave A deliverable. The decision: a worker on a borrowed box holds **two scoped
attenuating UCANs and never the root key**, calibrated **broad/long by default**
(Erik's trust-default amendment) and **narrow/short for strangers** by one coherent
leash dial that moves UCAN scope/TTL and lease term/cadence together; real authority
is kept off low-trust providers by an **intent-bound, rate-limited, logged
privileged-op callback**, not a signing oracle; revocation is **short-TTL-where-
tightened + issuer-subtree + an authorizer-local write-time check**; and the
cross-host lease's double-execution hazard is closed by a **monotonic lease epoch
enforced by an atomic compare-and-set at the single canonical-graph write boundary**,
with a **prefer-liveness** reclaim stance the fence makes safe. It invents no
identity, no crypto, and no second delegation system — the UCAN, the custody
boundary, and the revocation cascade are **WG-Fed's, reused verbatim** (NFR-4). The
three open questions are resolved: UCAN expiry (long-by-default, exec-sharpened) and
revocation hosting (authorizer-local safety check, no new central lookup) inherit
WG-Fed's resolutions one layer down; lease term/cadence per trust class is resolved
with a proposed table anchored to today's local `300`s/`30`s defaults — exact numbers
flagged for Erik, mechanism committed.*
