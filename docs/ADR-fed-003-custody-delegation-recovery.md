# ADR-003 (WG-Fed): Custody, Delegation & Recovery — Custodian-of-Record + Attenuating UCAN + Layered Recovery

**Status:** Proposed
**Date:** 2026-06-25
**Decision:** The portable identity **excludes the root private key, always**. An agent's root is **custodian-held** (the WG node operator when a node is present, the human owner node-less) behind an ssh-agent-style "sign this digest" boundary; the worker never holds root bytes and **download confers no oracle access**. Agents act under **UCAN** scoped, **attenuating-only** capabilities; `add_key`/`rotate_root` is **root/M-of-N only** (kills the hydra). **Download onto host B = fork by default**; same-self needs an explicit signed `add_key`/`delegate` or a node migration op. **Recovery is layered:** offline recovery key + override window (node) / mandatory paper-key + M-of-N guardian ceremony (node-less) / agents anchor to the custodian. **Custody (root-key safety) is distinct from authority scope (the dial): default authority is broad and long-lived; the short/scoped "leash" is environment-driven policy, not the birth default; humans are never leashed.**

> **This is the load-bearing WG-Fed ADR.** It owns the crux (HQ1 — "download
> Nora ≠ impersonate Nora") and the two Fatal findings of the adversarial pass
> (A-4 node-less recovery, and the hydra S-4), plus the freeze attack (S-3). It
> builds on **ADR-001** (`wgid` + sigchain + three-tier keys), whose `genesis` /
> `add_key` / `revoke_key` / `rotate_root` / `delegate` links and ssh-agent-style
> custody boundary this ADR fleshes into a full custody/delegation/recovery
> mechanism. The decision was *made* in the federation-study decision memo
> (`docs/federation-study/06-decision-memo-and-roadmap.md` §3 HQ1/HQ2/HQ9/HQ11,
> §6 ADR-003 stub, §8 hand-off); this ADR formalizes it and resolves the stub's
> four open questions. It is **not** a re-litigation of the architecture choice
> (`WG-Fed` = Candidate C with a B-shaped default, D's UCAN grafted, A preserved
> node-less) — that is settled.
>
> **One thing here *amends* the memo.** The memo's HQ1/HQ11 stated the
> session-scoped short-lived UCAN as the *default* ("Default to short-lived UCAN
> per session," §3 HQ1). Erik has amended that default (§D2 below):
> **broad/long-lived authority is the birth default** (agents and humans are
> first-class peers, not tools); the short, scoped "leash" is **environment-driven
> policy**, not the default. The custody mechanism — root never on the worker,
> attenuating-only delegation, `add_key` locked to root/M-of-N — is **unchanged**;
> what changes is the *default value of the authority-scope dial*, and §D2/§D3
> show why that does not reopen any Fatal finding.

---

## Context

The study's entire reason to exist is one sentence: **"download Nora's identity"
must never become "impersonate Nora"** (HQ1, FR-I2/FR-S1 — doc 03 §"HQ1, THE
CRUX"). The vision pulls two ways at once: identities must be **portable /
downloadable** (V2) *and* **cryptographically un-impersonatable** (V4). The more
freely an identity can move, the closer "moving the identity" gets to "moving its
signing power." Get this wrong and a feature (publish your agent) becomes a
catastrophe (anyone runs *as* your agent).

The adversarial pass (`docs/federation-study/05-adversarial-evaluation.md` §3)
attacked exactly this and returned a sharp verdict: the "download ≠ impersonation"
split **holds on paper in all four candidates** (the published bundle carries no
private key), but its **strength is gated by three implementation controls, each
an attack surface** (§3.2):

1. **S-1** — the bundle must *provably* exclude the key (impossible to guarantee
   statically for an opaque `payload_kind`; the runtime-containment answer is
   ADR-004's).
2. **S-2** — download must not confer **oracle access** to the signing custodian
   (a confused-deputy break: copy the ambient credential that lets a worker
   *reach* Nora's custodian and you sign as Nora *without ever holding a key*).
3. **S-4** — same-self enrollment must require a control the downloader **lacks**
   (else a single compromised signer issues an `add_key` and becomes a
   self-renewing persistent persona — *the hydra*).

Ranked by how *structurally* each candidate enforces all three, the verdict is
**D ≈ B > C > A** (doc 05 §3.2): the split is structural only where there is a
**custodian-of-record** (B's node holds the key; D's authority *is* an expiring
UCAN, not the key). A — the most decentralized — enforces it the *weakest*,
because with no custodian-of-record "the custody boundary is the user's own
discipline." This is the irony doc 05 §3.3 exists to surface: **V4
(non-impersonation) and V5 (decentralization) pull against each other at the
custody layer**, and the crux feature is best served by the *less* decentralized
designs.

`WG-Fed` resolves the irony by keeping a **custodian-of-record *underneath* a
self-certifying root** (memo §2.2): the custodian holds the only copy of the
root, download confers an *expiring capability* and never *oracle access*, and
same-self enrollment requires `root/M-of-N` — so it gets the *structural* split
**without** making the root central (identity verification is still never central,
ADR-001 D5). Two findings here are *Fatal* and must be designed in, not
discovered in code (doc 05 §5.3): **A-4** (a node-less identity with no recovery
ceremony is unrecoverable — fatal for the careless and for agents) and the
**hydra (S-4)** (and its recovery-flexibility cost). The freeze attack **S-3**
rides along as the revocation-liveness problem on the async path.

Agents sharpen all of this: they have **no fingers, phones, or hardware tokens**
(doc 03 HQ1), so the human custody patterns (device key + biometric, FIDO2/
passkey) do not map; and they are spun up, cloned, and resumed on arbitrary
hosts, so "the key lives on the user's device" has no agent analogue. Agent
recovery therefore *always* collapses to "the custodian's key is safe" (doc 05
§5.3) — agents are never purely-P2P-recoverable in any candidate. This is the
honest consequence of agents having no fingers, and it is **by design** (FR-S6).

This ADR fixes the custody, delegation, and recovery mechanism that the rest of
WG-Fed builds on. It is a Wave-2 deliverable; **no federation code lands until
ADR-001/002/003 are Accepted** (memo §5, Wave 2).

---

## Decision

### D1 — The portable identity excludes the root private key, *always*

The portable identity (ADR-001 D2) is **`IdentityRecord` + `StateSnapshot`s + the
public key set + the currently-authorized delegations — and never the root
private key** (FR-I2, FR-S1). Fed to an honest client, the published artifact lets
you **read and verify** Nora's history; it does **not** let you **sign as** Nora.
This is non-negotiable and is the directly-tested headline assertion of the spark
milestone (memo §4.2 step 6).

**Where the root lives** (ADR-001 D3, made operational here):

- **Humans** self-hold the root on a device/OS keychain, hardware-backed
  (FIDO2/passkey) where available. The human is **sovereign** — the root is theirs
  and no custodian sits above it.
- **Agents'** root is **custodian-held** — by the **WG node operator** when a node
  is present, by the **human owner** node-less — in `wg secret` (or an HSM), behind
  an **ssh-agent-style "sign this digest" boundary** (doc 01 §2.16, the canonical
  "use a key without holding it" primitive; doc 04 §1.1/§1.5 `custody.rs`). The
  worker **requests signatures** (or is issued a short-lived signer/UCAN); it
  **never receives the root bytes** (FR-S1, FR-S6).

**Download confers no oracle access (S-2).** The custody split must hold at the
*access* level, not only the *byte* level. Therefore:

- The custodian authenticates the **requesting host/agent identity** (a per-host
  *enrolled signer key*), **not a bearer token that travels in the bundle**.
  Copying Nora's published bundle onto host B does **not** copy anything host B can
  present to Nora's custodian to obtain a signature.
- Signing requests are **intent-bound** — "sign *this digest* for *this
  purpose*," never "sign anything" — and are **rate-limited and logged** (NFR-7
  audit).
- The per-host enrollment step is **the fork-vs-same-self boundary made
  unskippable** (§D4): "download and it just works" is *deliberately* not how it
  works, because that would be impersonation.

*Why.* Adopts the doc 01 §4.1 winning pattern (Farcaster custody-key → signer /
ssh-agent custody) and the *structural* custody end of doc 05 §3.2 (D ≈ B > C > A),
enforcing all three controls the verdict names: the bundle provably excludes the
key (S-1, with ADR-004 owning the opaque-payload runtime-containment residue),
download confers no oracle access (S-2), enrollment needs a control the downloader
lacks (S-4 → §D3).

*Cost.* Per-host enrollment friction — "download and it just works" requires an
explicit re-authorization step. That friction **is** the fork/same-self boundary
made correct (S-2 cost, accepted).

### D2 — Custody ≠ authority: the trust-default / leash-as-a-dial amendment

This is the structural distinction the whole ADR turns on, and it carries **Erik's
amendment** to the memo's HQ1/HQ11 default. Two things that the memo's
"short-lived UCAN per session by default" framing conflated are **separated** here:

- **Custody** — *where the root private key lives and who can be compelled to sign
  with it.* The answer is fixed and non-negotiable (§D1): the root is **always**
  safe with a custodian; the worker **never** holds root bytes. **Custody costs the
  agent no autonomy.** An agent whose root is custodian-held can still do anything
  its authority permits — it merely *requests a signature* instead of *holding the
  root*. Custody is a **safety/integrity** property, not a restriction on what the
  agent may do.
- **Authority scope** — *what an agent (or human) is permitted to do, for whom, and
  for how long.* This is **a dial**, and **its default value is the amendment's
  subject.**

**The amendment (Erik's call).** The memo set the dial's default to the *tight*
end ("short-lived UCAN per session," §3 HQ1) — a leash. **That is reversed as the
*birth default*:**

> **Default authority is broad and long-lived.** Agents and humans are
> **first-class peers, not tools.** A newly-created agent receives a broad,
> long-lived delegation (or a standing sigchain-authorized signer), because the
> default posture of the system is *peerhood*, not *containment*.
>
> **The short, scoped "leash" is environment-driven policy, not the default.**
> Corporate, regulated, multi-tenant, or otherwise high-stakes deployments
> **tighten the dial** (short expiries, narrow per-task scopes, mandatory
> re-issuance) as a matter of *local policy*. The leash is a setting the
> *environment* turns on, not the value an agent is *born with*.
>
> **Humans are never leashed.** A human self-holds their root (§D1) and is
> **sovereign**: their standalone authority is never scoped down by any custodian,
> node, or policy. The dial governs *delegated* authority (an agent acting *for* a
> principal, or an agent's authority *within* an org's policy) — never a human's
> own.

**Why the amendment does not reopen any Fatal finding** — this is the load-bearing
argument, and it works precisely *because* custody ≠ authority:

- The hydra (S-4) is killed by **integrity invariants** — attenuating-only
  sub-delegation and `add_key`/`rotate_root` locked to root/M-of-N (§D3) — **not**
  by short expiries. Those invariants hold **at every dial setting**. A broad,
  long-lived signer still **cannot widen its own scope** and still **cannot grow
  the authorized key set**. Broad-by-default authority is therefore *not* the
  hydra; the hydra was always an *integrity* failure, not an *expiry* failure.
- Custody is unchanged, so a stolen *broad* signer is **still not the root** and
  **still degrades to recovery + revocation, not permanent takeover** (HQ2, §D5/D6).
  What a long expiry changes is the *detection-to-revocation window* for a stolen
  broad signer — a **blast-radius/latency** trade, addressed in §D3 and OQ1, and
  *exactly* what the dial exists to tighten where that trade is unacceptable.

*Why (the principle).* The vision (V7) insists agents and humans are both
**first-class**, and the social-network north star treats long-lived, loadable,
portable identities as **peers** in an email-speed network — not short-lived tools
on a per-session leash. A birth default of "containment" silently re-casts agents
as tools and contradicts that pillar. Making the leash *environment-driven* keeps
the safety knob available to every deployment that needs it **without** baking a
tool-not-peer assumption into the substrate. Critically, the *security* of the
design never depended on the leash being the default — it depends on custody (§D1)
and the integrity invariants (§D3), both of which are dial-independent.

*Rejected.* Short-session-leash-as-the-birth-default (the memo's original HQ1
phrasing) — it conflates custody (a safety property that costs no autonomy) with
authority (the dial), and it defaults the whole network to treating agents as
tools. We keep its *mechanism* (UCAN expiry is real and useful) but move it from
*default* to *policy* (§D3, OQ1).

### D3 — Authority is a UCAN: signed, scoped, attenuating-only, revocable

Delegation is **UCAN-style capability certificates** (grafted from Candidate D —
"the best component in the study," "first among components," doc 05 §5.3/§7.1;
doc 04 §5.1). "Agent X may act for principal Y, scope S, until T" is a **signed,
checkable, revocable, expiring capability** (FR-T4), issued via ADR-001's
`delegate` sigchain link and/or a standing UCAN token. The standing signer is a
sigchain-`add_key`-authorized device key; the UCAN is the per-action capability.

**The integrity invariants (dial-independent — these are *not* the leash):**

- **Delegation never shares a private key** (FR-S1). The `aud` agent holds *its
  own* signer; the capability authorizes it, it does not hand over `iss`'s key.
- **Sub-delegation is attenuating-only** — it can **narrow, never widen** scope,
  and **inherits the parent's expiry**. A child capability is always ⊆ its parent.
  This is what structurally kills the **hydra** (S-4): no chain of delegations can
  manufacture authority its root did not grant.
- **`add_key` and `rotate_root` are root/M-of-N only** (S-4). A delegated signer —
  **however broad** — **cannot grow the authorized key set**. Enrolling a new
  signing key onto an identity requires the root (or the M-of-N quorum, §D5), never
  a mere delegate. This is the single control that turns "any authorized key can
  `add_key`" (Candidate A's hydra-by-design, doc 05 §4.1) into a closed set.
- **Revocation** operates at **issuer-subtree granularity** (kill the parent → kill
  the whole subtree), composed with sigchain `revoke_key` and the freshness
  mechanism of §D6.
- **Accountability** — every action is attributable to **both** the agent signer
  **and** the principal: the UCAN chain records `iss`/`aud`, and the append-only
  sigchain is the audit trail (NFR-7).

**The dial (the part §D2's amendment governs):** the UCAN's **scope breadth** and
**expiry** are policy, not fixed constants. The **default** is **broad scope and
long expiry** (first-class peer); **environment policy** may set narrow scope and
short expiry (the leash). The integrity invariants above hold at *every* setting —
so tightening the dial reduces blast radius and revocation latency, while loosening
it never reintroduces the hydra and never moves the root onto the worker.

**Revocation, not expiry, is the primary kill-switch under the broad default.**
The memo's original design leaned on *short expiries* doing double duty: small
theft blast-radius **and** revoke-by-expiry as the primary revocation path. Under
the broad/long default, that second duty shifts onto **explicit** mechanisms —
issuer-subtree revocation + sigchain `revoke_key` + freshness attestations (§D6) —
because we deliberately are *not* relying on a 15-minute TTL to retire a
capability. This is consistent with rejecting "fast-revocation-at-scale as the
*primary* mechanism" (UCAN's open problem D-3, non-goal §11) while still making
revocation *effective* via the subtree + freshness path. Exact default TTLs are
**OQ1**.

*Why.* UCAN scores best of all four candidates on TC2 (smallest signer blast
radius) and TC9 (attenuating + expiring sub-delegation) and is the only component
that makes the downloaded-identity split *structural* rather than disciplinary
(doc 05 §3.2/§7.1). Separating the integrity invariants from the dial is what lets
us adopt Erik's broad-by-default amendment *without* surrendering the structural
split.

*Rejected.* Shared-key delegation (collapses HQ1 — doc 03 §HQ11). Blanket
*non-attenuating* delegation (a child could widen scope → the hydra returns).
`add_key` available to any authorized signer (Candidate A's hydra-by-design, doc 05
§4.1/§5.3). Relying on fast global revocation-at-scale instead of subtree +
freshness (D-3, an open problem).

### D4 — "Download onto host B" = fork by default; same-self is explicit

Loading Nora's published identity onto a new host has a **defined, cryptographically
unskippable continuity semantics** (FR-I5):

- **Fork by default.** A download yields a **verifiable, read-only copy** — host B
  can render Nora's history and verify her artifacts, but **cannot sign as Nora**,
  because the bundle carries no root and host B holds no signer Nora's sigchain
  authorizes. A fork that wishes to be its own identity starts a **new genesis that
  cites Nora's chain as parent** (ADR-001 D2) — a verifiable *child*, not Nora.
- **Same-self is explicit and authorized.** Continuing as *the same* Nora on host B
  requires **either** (a) an explicit, signed **`add_key`/`delegate`** link by a
  surviving authorized key (enrolling host B's signer — and recall `add_key` is
  root/M-of-N only, §D3), **or** (b) a **node-mediated migration op** (the node
  custodian re-homes the agent and issues host B a fresh signer). **Never
  automatic.**

This makes FR-I5's fork/same-self boundary cryptographically unskippable, and it is
*the same step* as the per-host enrollment of §D1 (S-2): the friction is not an
accident, it **is** the boundary between "a copy of Nora" and "Nora on a new host."

*Why.* Adopts doc 04 §4.1's continuity rule (same-self ⇒ an `add_key`/`delegate`
link or a node migration; copy ⇒ fork) and binds it to the custody boundary so the
default-safe answer ("it's a fork") requires no discipline, while the powerful
answer ("it's still me") requires a control the mere downloader lacks.

*Rejected.* Automatic same-self on load (collapses HQ1 — download *becomes*
impersonation). Treating every load as a hard fork with no same-self path (breaks
V2 portability — you could never legitimately move your own agent to a new host).

### D5 — Recovery is layered by deployment mode

Recovery composes B's and A's models (doc 05 Recoverability: B 5, A 2; C gets
both), **layered by deployment** so each mode has a backstop and the node-default
majority is not forced into guardian-ceremony friction:

- **Default — node present.** The node holds rotation keys; the **human owner holds
  an *offline* recovery key with a time-boxed override window** (atproto's model, a
  72h-style window — doc 01 §4.2 ★★★, §2.4). This is recoverable **even against a
  hostile node**: the offline, higher-priority recovery key overrides a compromised
  custodian within the window (this is what bounds B-1, doc 05 §4.2). App-password-
  style scoped, revocable secondary credentials cover routine re-auth without
  touching the root (doc 04 §3, doc 01 §2.4).
- **Node-less mode — MANDATORY ceremony.** Genesis MUST embed **both a paper key
  AND M-of-N social-recovery guardians** named at genesis (the `genesis` `recovery`
  slot of ADR-001 D3/OQ3, here *populated and required*). This is **non-negotiable**
  and is what defuses the Fatal finding **A-4** (a node-less identity with no
  recovery path is unrecoverable). Genesis tooling **refuses** to mint a node-less
  human identity without it (memo §5 guardrail: *"Never ship the node-less mode
  without the mandatory recovery ceremony"*). Guardian UX is **OQ4**.
- **Agents — anchor to the custodian.** Agent recovery *always* collapses to "the
  custodian's key is safe" (doc 05 §5.3): the node (or owner) custodian **is** the
  recovery anchor by design (FR-S6, HQ9). Agents have no social graph, so guardians
  are not an agent concept; an agent is recovered by its custodian re-issuing a
  signer, not by an independent P2P ceremony. This is the honest consequence of
  agents having no fingers, phones, or hardware tokens — **accepted**.

**The hydra cost A-4 spells out, accepted.** Locking `add_key` to root/M-of-N (§D3)
**removes A's only node-less recovery primitive** ("a surviving authorized key adds
a new signer," doc 05 §4.1/§5.3) — that primitive *is* the hydra. We pay for
hydra-resistance with the mandatory genesis ceremony (paper key + guardians)
*instead of* surviving-key recovery. You cannot have both cheap surviving-key
recovery and hydra-resistance node-less; we choose hydra-resistance and mandate the
ceremony.

*Why.* B has the strongest recovery of the four (doc 05 Recoverability 5); the
offline recovery key is the one mechanism that makes even mass node-compromise
recoverable (doc 05 §4.2, B-1). The mandatory node-less ceremony is the only thing
that turns A's Fatal-careless recovery into a bounded one (A-4).

*Rejected.* Social-M-of-N as the *only* recovery (Fatal A-4 — pre-arranged-or-death;
doc 05 §5.3). Domain-control recovery (Candidate D's model — Fatal-as-primary,
hostage to a registrar, D-2). Accept-loss / immutable identity (violates V6).
Surviving-key `add_key` recovery node-less (it *is* the hydra, S-4).

*Cost.* The offline recovery key is itself a standing takeover capability (B-4) —
mitigated by holding it offline/hardware, optionally M-of-N split, with a *visible*
time-locked override. The mandatory node-less ceremony is friction users may resent
(but skipping it is the Fatal path, so it is enforced, not optional).

### D6 — Revocation is live on the async path (the S-3 freeze defense)

Revocation is the load-bearing recovery-from-compromise primitive, and on an async,
offline-tolerant, store-and-forward network it **requires freshness** (S-3): a
`revoke_key`/subtree-revoke only protects a verifier who *sees* it. An attacker who
eclipses a victim's relays or serves a **stale, validly-signed sigchain head** (the
*freeze attack*) keeps a revoked or stolen key looking alive — the signature still
checks; only freshness detects the rollback.

WG-Fed's revocation is **three composed layers**, all self-verifying:

1. **Sigchain `revoke_key`** (ADR-001 D2) — the durable, content-addressed,
   self-verifying revocation of a key.
2. **Issuer-subtree UCAN revocation** (§D3) — kill a parent capability, kill its
   whole subtree.
3. **Freshness attestations** (ADR-001 OQ4 — the *shared* mechanism; the freshness
   attestation format lives in ADR-001 OQ4, distinct from the D7 compat handshake): the
   custodian/node periodically emits a signed `{ head, as_of, expires = as_of + Δ,
   seq, alg, sig }` over the current `sigchain_head`; a verifier **re-fetches** it
   before a freshness-gated action and **fails closed on stale** for high-value
   operations, with a monotonic `seq` closing the clock-skew replay gap.

Because the broad/long authority default (§D2/D3) deliberately does *not* rely on
short expiries to retire capabilities, this explicit revocation + freshness path is
**the** kill-switch and must be live — high-value actions (accepting a
`rotate_root`, a large-scope delegation, a cross-trust `StateSnapshot` load) fail
closed when the freshest obtainable attestation is older than the action's Δ.
*Where* the revocation/attestation feed is hosted is **OQ2**.

*Why.* This is the atproto/Keybase posture (a re-fetchable "valid-as-of" + a strict
window for sensitive ops) adapted to WG's async budget. The freeze attack is the
one way a stolen-but-revoked key survives, and the broad-default amendment raises
the stakes on getting revocation *live* rather than expiry-driven.

*Rejected.* Expiry-as-the-only-revocation (fights offline tolerance and is exactly
the leash we moved off the default — §D2). Trusting a single relay's head (freezable
— S-3). Trusting local clocks alone (clock-skew attack — the `seq` is the
clock-independent backstop).

---

## Status

**Proposed.** This ADR records the decision exactly as fixed in the federation-study
decision memo, **incorporates Erik's trust-default / leash-as-a-dial amendment**
(§D2), and resolves the four open questions the ADR-003 stub left open. **Erik
ratifies it to Accepted** — that human gate is deliberately not set here. No
federation code lands until ADR-001/002/003 are Accepted (memo §5). Downstream:
`adr-fed-coherence` packages this with ADR-001/002/004 for Erik's ratification.

---

## Consequences

- **`src/secret.rs` becomes a typed signing custodian** with an ssh-agent-style
  "sign this digest" call (the §D1 boundary): a worker requests an *intent-bound*
  signature over a digest and **never receives root bytes**. The custodian
  authenticates the requesting host/agent by an *enrolled per-host signer key*, not
  a bearer token, and **rate-limits + logs** every request (S-2, NFR-7).
- **New `src/identity/custody.rs`** issues / verifies / revokes UCANs (harvested
  from Candidate D, doc 04 §5.1/§1.5): scoped, **attenuating-only**, revocable,
  expiring capabilities; a UCAN verifier walks the `iss`/`aud` chain and enforces
  scope ⊆ parent and expiry ≤ parent at every hop. The **dial** (default-broad
  scope + expiry, environment-tightenable) lives here as policy, distinct from the
  integrity invariants which are enforced unconditionally.
- **The node becomes the agent custodian-of-record in the default deployment** — it
  holds agent root keys and is the recovery anchor (FR-S6) — *under* a human-owner
  offline recovery key with a time-boxed override (so a hostile node is recoverable,
  bounding B-1). Node-less, the human owner is custodian and the mandatory paper-key
  + M-of-N ceremony is the anchor.
- **`add_key` / `rotate_root` are gated to root/M-of-N** at the sigchain-validation
  layer (ADR-001 `sigchain.rs::verify()`): a `delegate`-authorized signer that
  emits an `add_key` produces an **invalid** link. This is the hydra kill enforced
  in code, not policy.
- **`Agent` gains held-delegation / custodian fields** (`src/agency/types.rs`,
  building on ADR-001's `pubkey`/`sigchain_head`): the currently-held UCANs and the
  custodian reference, all `#[serde(default)]` so existing graphs parse unchanged.
- **An authority-scope policy surface** is added (the dial): a deployment sets the
  default delegation scope/expiry and may declare a "strict/corporate" profile that
  tightens it. The **default profile is broad/long** (§D2). `wg config lint`-style
  validation should surface a misconfigured dial the way it surfaces a mis-routed
  model (CLAUDE.md), so a too-loose *corporate* deployment is visible.
- **Resolves HQ1, HQ2, HQ11**; **bounds B-1** (a compromised node still cannot forge
  what a verifier self-checks — ADR-001 D5 — and the offline recovery key overrides
  it within the window); **defuses Fatal A-4** (mandatory node-less ceremony) and the
  **hydra S-4** (attenuating-only + `add_key`-locked).
- **Cost we accept:** per-host enrollment friction (= the fork/same-self boundary,
  S-2); the offline recovery key is a standing takeover capability (B-4, mitigated
  by offline/hardware/M-of-N storage + visible time-locked override); agents are
  *never* purely-P2P-recoverable (custodian is the anchor, FR-S6); and under the
  broad-default the detection-to-revocation window for a stolen *broad* signer is
  longer than a short-TTL design — accepted as the price of first-class peerhood and
  **tightenable by the dial** wherever it is not.

---

## Alternatives rejected

- **Standing signer on the worker host + any-authorized-key-can-`add_key`**
  (Candidate A's custody, doc 05 §3.2/§4.1). Rated *weakest* of the four ("the
  custody boundary is the user's own discipline") and **the hydra by design** —
  a single stolen worker signer becomes an immortal persona. Rejected: the root is
  custodian-held (§D1) and `add_key` is root/M-of-N only (§D3).
- **Sharing a private key as the portability mechanism.** Collapses HQ1 — download
  *is* impersonation. Rejected in doc 03 §HQ11 itself; the portable identity carries
  no private key (§D1).
- **Blanket / non-expiring, *non-attenuating* delegation.** A leaked agent key acts
  as the human indefinitely (doc 03 T9), and a non-attenuating child can *widen*
  scope → the hydra returns. Rejected: delegation is attenuating-only with
  issuer-subtree revocation (§D3). (Note: long *expiry* by default is **kept** — §D2
  — because the *integrity* defense is attenuation + `add_key`-lock, not short TTL.)
- **Short-session-leash as the *birth default*** (the memo's original HQ1 phrasing).
  Conflates custody (costs no autonomy) with authority (the dial) and defaults the
  network to treating agents as tools. Rejected per Erik's amendment (§D2); the
  short leash survives as **environment-driven policy**, not the default.
- **Leashing humans.** Humans self-hold their root and are sovereign; scoping a
  human's *own* authority down is rejected outright (§D2). The dial governs
  *delegated* authority only.
- **Social-M-of-N as the *only* recovery** (Fatal A-4, pre-arranged-or-death, doc 05
  §5.3). Rejected as the sole path; it is *mandatory* node-less (alongside a paper
  key) and *optional defense-in-depth* node-present.
- **Domain-control recovery** (Candidate D's model, doc 05 D-2, Fatal-as-primary —
  hostage to a registrar/DNS). Rejected: recovery anchors to the offline recovery
  key / guardian quorum / custodian, never to a domain.
- **Expiry-as-the-only-revocation** (fast-revocation-at-scale, UCAN's open problem
  D-3). Rejected as the *primary* mechanism; revocation is the composed
  subtree + `revoke_key` + freshness path (§D6), with expiry as one contributor the
  dial can tighten.

---

## Open questions

The ADR-003 stub (memo §6) and the memo's hand-off checklist (§8 items 1, 3, 6)
left four questions for this ADR to close. All four are resolved with rationale
below; where a residue is genuinely a tuning / UX / governance value judgment it is
**explicitly flagged for Erik** rather than silently fixed.

### OQ1 — UCAN expiry defaults vs offline-chattiness (under the *broad-by-default* amendment) — **RESOLVED (default + mechanism; exact TTLs flagged)**

**Reframed by §D2.** The memo posed this as "short expiries mean chatty
re-issuance, which fights offline tolerance — accepted as the price of a small
blast radius" (HQ11 cost). Under Erik's amendment the *default* is no longer short,
so the tension **largely dissolves at the default** and re-appears only when the
environment dial tightens it.

**Resolution.**

- **Default expiry is long** — a created agent's delegation/standing signer is
  **long-lived** (bounded by issuer-subtree revocation + `revoke_key` + freshness,
  §D6, **not** by a short TTL). Concretely the default is "**valid until revoked**,
  with a long sanity ceiling" rather than per-session. This makes the **default
  path offline-friendly**: a broad, long-lived capability needs **no chatty
  re-issuance**, so the email-speed / both-ends-offline budget (NFR-2) is *helped*,
  not fought, by the amendment.
- **Tightening the dial re-introduces chattiness *as a chosen cost*.** A corporate /
  high-stakes profile that sets short expiries (e.g. per-session, ≤15 min for
  high-value scopes) **opts into** the re-issuance chattiness in exchange for a
  smaller blast radius and faster revocation. That trade is now an *explicit,
  local* decision, not a global default everyone pays.
- **The blast-radius the short default used to buy is recovered elsewhere** at the
  broad default: custody keeps a stolen signer from being the root (§D1),
  attenuation keeps it from widening (§D3), and issuer-subtree revocation +
  freshness retire it once detected (§D6). The residual is a longer
  *detection-to-revocation* window — which is *precisely* what a tightened dial
  buys back where it matters.

*Flagged for Erik (tuning):* the **exact default sanity-ceiling** for a "broad,
long-lived" capability (e.g. 30 / 90 days vs "until revoked"), the **high-value
short-Δ value** the corporate profile uses (the memo floated ≤15 min, shared with
ADR-001 OQ4's freshness Δ), and **which scopes are deemed "high-value"** by default
(rotate_root, large-scope delegation, cross-trust state load are the obvious ones).
The *mechanism* (long-by-default, revocation-not-expiry as primary, dial-tightenable)
is the ADR-003 commitment; the numbers are sensible defaults Erik can set without
reopening the design.

### OQ2 — Revocation-list hosting (itself a lookup dependency) — **RESOLVED (rides ADR-001's fail-safe cascade; no new mandatory central lookup)**

**The concern.** A revocation list is itself a *lookup dependency* — if checking
"is this capability revoked?" requires reaching a single mandatory endpoint, that
endpoint becomes a censorship/availability single point (and a freeze target, S-3),
re-introducing exactly the central dependency WG-Fed exists to avoid (FR-F4/F5).

**Resolution — host revocation the same way WG-Fed hosts everything else:
self-verifying bytes over a fail-safe cascade, never a mandatory central CRL.**

- **Sigchain `revoke_key` links** are content-addressed, self-verifying sigchain
  links (ADR-001 D2) — they publish *anywhere* the sigchain publishes (cache → node
  → relay → DHT/Iroh / a dumb third location, ADR-001 D5 cascade). *Where* they are
  hosted is a convenience, not a trust dependency; a forged or withheld feed cannot
  *manufacture* a valid un-revoke (a revoke, once any verifier sees it, is durable).
- **Issuer-subtree UCAN revocations** are signed revocation records published in the
  **same cascade** and, crucially, **piggy-backed on the freshness attestation**
  (§D6 / ADR-001 OQ4): the `{ head, as_of, expires, seq }` a verifier *already*
  re-fetches before a high-value action carries the current head, so learning "is
  this revoked?" is the *same fetch* as "is this fresh?" — **no extra lookup
  dependency** is added.
- **Default host = the custodian/node** (it is already the always-on availability
  anchor), **but the feed is self-verifying**, so any mirror / relay / the
  recipient's own cache is an equally-valid source. **Fail-closed on stale** for
  high-value actions (§D6) means a *withheld* revocation feed degrades the verifier
  to read-only, never to fail-open.

So revocation adds **no new mandatory central lookup** — it reuses ADR-001's
freshness cascade and inherits its fail-safe property (a hint that can only help,
never override a self-verification; HQ6).

*Flagged for Erik (policy):* whether to *also* expose a **standalone CRL-style
endpoint** (a convenience aggregate of subtree revocations for clients that want
one fetch) — it must remain *optional and non-authoritative* if offered — and the
**default staleness Δ for revocation checks** (shared with ADR-001 OQ4's freshness
Δ). The *mechanism* (self-verifying, cascade-hosted, freshness-piggy-backed,
fail-closed) is the commitment.

### OQ3 — Default agent custodian: node-operator vs human-owner when both exist — **RESOLVED (node = operational custodian; human = sovereign backstop; multi-tenant governance flagged)**

**The question (memo §8 item 1).** When *both* a WG node and a human owner exist,
which is the agent's *default* custodian-of-record?

**Resolution — a layered rule, mirroring atproto's PDS-holds-key / human-holds-
recovery-key split (doc 01 §4.2 ★★★):**

- **The node is the operational custodian-of-record by default when a node is
  present.** It is always-on, so it is the right home for the availability,
  recovery-anchor, and structural-split-enforcement jobs (doc 05 ranks B's
  recovery strongest); the agent's root lives in the node's `wg secret` and the node
  answers "sign this digest" requests (§D1).
- **The human owner is *always* the sovereign backstop.** The owner holds a
  **higher-priority offline recovery key** with a time-boxed override window (§D5),
  so the node is **never an unaccountable custodian** — a hostile or failed node is
  overridden by the owner within the window (this is what bounds B-1). The human is
  never *subordinate* to the node; the node is a *convenience custodian under* the
  human's recovery authority.
- **Node-less → the human owner is the custodian** (there is no node), with the
  mandatory paper-key + M-of-N ceremony as the anchor (§D5).

So the default is "**node operates, human owns**": the node does the always-on
signing/recovery work, the human retains sovereign override. This keeps agent
custody concrete (a real always-on holder) without making the node a trust root the
owner cannot escape.

*Flagged for Erik (governance — ties to the §D2 amendment):* in a **multi-tenant /
org node** (a node operated by *someone other than the agent's human owner* — a
company, a shared household-of-strangers), should the default custodian be the
**org node-operator** or the **human owner**? This is a trust/governance call, and
it is exactly where the trust-default amendment bites: for a *personal* agent the
human owner is the natural sovereign; in a *corporate* deployment the org node is
the custodian *and* the dial is tightened (§D2). The recommendation is **owner-
sovereign by default, org-custodian as an explicit policy of the org profile** (so
no one is silently subordinated to a node operator they did not choose). The exact
default for the org profile, and whether an org may *deny* an owner the override key,
is Erik's governance call.

### OQ4 — M-of-N guardian UX for node-less recovery — **RESOLVED (mechanism + proposed default UX; exact M/N + polish flagged)**

**Resolution — mechanism (the ADR commitment):**

- **Enrollment at genesis.** The node-less `genesis` ceremony embeds the guardian
  set — **guardian pubkeys + a threshold M-of-N** — in ADR-001's `genesis`
  `recovery` slot (D3/OQ3), cryptographically bound from the identity's first link
  (you cannot safely add a recovery quorum *after* a key is at risk). A guardian is
  enrolled by exchanging a signed `IdentityRecord` (the guardian is just another
  `wgid:` identity) — guardianship is a signed contact relationship, reusing the
  same primitives.
- **Recovery is a guardian-signed quorum, async by construction.** To recover, the
  owner (from a new key) requests guardian endorsements; **M of the N guardians each
  sign** a recovery assertion over the new root pubkey; the collected quorum
  authorizes a `rotate_root` (or a dedicated `recover` link) on the sigchain.
  Crucially, **guardians need not be online simultaneously** — endorsements are
  collected store-and-forward at email-speed (NFR-2), which fits the async network
  and real human availability.
- **Abuse resistance (A-7, hostile guardian).** No single guardian can recover
  (threshold M ≥ 2); the recovery is a *visible* sigchain event (append-only, so a
  surprise recovery is auditable); and a recovery `rotate_root` is itself a
  high-value, freshness-gated action (§D6). A coerced minority of guardians cannot
  reach the threshold.

**Proposed default UX (flagged for Erik):**

- **Default threshold 2-of-3** for an individual (low ceremony, survives one lost or
  uncooperative guardian) — explicitly a *proposed default*, not a fixed constant.
- **Guardian invitations** ride the existing contact/message exchange (a guardian
  accepts by counter-signing), so no new UX surface is invented.
- **Recovery flow** is an async "collect M endorsements" wizard, not a synchronous
  ceremony, matching the email-speed budget.

*Flagged for Erik / shared with ADR-001 OQ3:* the **exact default M and N**
(2-of-3 vs 3-of-5), whether the **guardian set is mutable post-genesis** and under
what authority (mutating it is itself a root/M-of-N-class operation), and the
**guardian-enrollment and recovery UX polish**, are policy / UX value judgments,
not mechanism. ADR-001 OQ3 flagged the same M/N + guardian-mutability questions and
deferred them to ADR-003; this ADR commits to the *mechanism* (genesis-bound
guardian set, async threshold endorsement, visible/auditable recovery) and proposes
the defaults, leaving the exact numbers and UX for Erik to bless.

---

## References

- `docs/federation-study/06-decision-memo-and-roadmap.md` — §1 (the decision), §2.2
  (custody-split verdict we design to), §3 HQ1 (the crux), HQ2 (rotation/recovery),
  HQ9 (human vs agent custody), HQ11 (authority/delegation), §5 Wave 5 (recovery
  build) + guardrails, §6 ADR-003 stub, §8 hand-off (items 1/3/6). **Erik's
  trust-default / leash-as-a-dial amendment** revises §3 HQ1/HQ11's
  "short-lived-UCAN-per-session default" (§D2 here).
- `docs/federation-study/05-adversarial-evaluation.md` — §3 (the headline
  downloaded-identity → impersonation attack), §3.2 (the verdict **D ≈ B > C > A**;
  the three controls S-1/S-2/S-4), §3.3 (the decentralization-vs-custody irony),
  §4.1 (Candidate A's hydra-vs-recovery bind), §4.2 (Candidate B's node-as-custodian
  + recovery-key override of B-1), §5.1/§5.2 (failure register: S-2 oracle access,
  S-3 freeze, S-4 hydra, A-4 recovery, B-1/B-4, D-2/D-3), §5.3 (Fatal-finding
  summary), §7.1 (harvest D's UCAN, A as preserved node-less option).
- `docs/federation-study/04-candidate-architectures.md` — §1.1 (three-tier keys +
  the ssh-agent custody boundary), §1.5 (`custody.rs` / `secret.rs` touch-points),
  §4.1 (the Farcaster/UCAN custody core; same-self vs fork), §5.1 (Candidate D's
  UCAN issue/verify/revoke, the harvested layer).
- `docs/federation-study/03-requirements-and-hard-questions.md` — FR-I2 (portable =
  public + state, excludes the key), FR-I5 (fork/same-self continuity), FR-S1 (keys
  never leave the custodian), FR-S2 (rotation & recovery), FR-S6 (agent vs human
  custody differs by design), FR-T4 (delegation is checkable/revocable/expiring);
  HQ1 (the crux), HQ2, HQ11; tensions T1/T5/T9.
- `docs/federation-study/01-prior-art-landscape.md` — §2.16 (ssh-agent, the canonical
  "use a key without holding it" primitive), §2.4/§4.1 (Farcaster custody-key →
  signer; "copying a signer ≠ owning the identity"), §4.2 (atproto rotation key +
  72h recovery-key override; Keybase paper keys + sigchain).
- `docs/ADR-fed-001-identity-key-model.md` — the `wgid` + sigchain + three-tier key
  model this ADR's custody/delegation/recovery mechanism operates over; D2 (sigchain
  links incl. `delegate`), D3 (key hierarchy + custody boundary), D5 (verification
  never central; resolution cascade), D7 (crypto agility) + OQ3 (genesis `recovery`
  slot) + OQ4 (freshness-attestation format, *shared* with §D6 here).
- `docs/ADR-actor-vs-agent-identity.md` — the unified `Agent` identity whose custody
  / recovery / authority split (not type split) this ADR specifies.
