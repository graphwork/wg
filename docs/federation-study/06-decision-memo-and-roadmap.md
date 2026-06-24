# Federation Study 6/6 — Decision Memo, Spark Test & Roadmap

> **Headline federation study, wave 1, task 6 of 6 — the *decide* phase.**
>
> This memo synthesizes the whole study (docs 01–05) into **the** federation /
> identity decision: it picks **one** architecture, makes an explicit call on
> **every** hard question from doc 03, defends the choice against the alternatives
> using the adversarial pass (doc 05), defines the **spark test** that validates
> it, and lays out the phased roadmap with ADR stubs and non-goals.
>
> **This memo IS the spark-test definition.** It is written to be acted on by Erik
> (decision + roadmap) and by Vaughn (the agency-lib identity primitives the spark
> exercises).

**Status:** decision · **Date:** 2026-06-24 · **Owner task:** `fed-decision`
**Inputs:** `01-prior-art-landscape.md` · `02-current-state-baseline.md` ·
`03-requirements-and-hard-questions.md` · `04-candidate-architectures.md` ·
`05-adversarial-evaluation.md`

---

## 0. How to read this document

1. **§1 — The decision in one page.** The architecture, the plain
   decentralization call, and the headline defense. Read this if you read nothing
   else.
2. **§2 — The recommended architecture (`WG-Fed`)**, named and bounded, with its
   defense against A/B/C/D citing doc 05's ranking and findings.
3. **§3 — Decision register: every doc-03 hard question (HQ1–HQ12) decided**, each
   as `DECISION / why / what we rejected (cite doc 05) / cost we accept`.
4. **§4 — The spark test** — the concrete, runnable first-milestone PoC that
   validates the choice end-to-end.
5. **§5 — Phased roadmap** — the waves after this study, dependencies, and the
   **don't-build-yet** guardrails.
6. **§6 — ADR stubs** for the three load-bearing decisions (identity/key model,
   transport, custody/recovery) + a fourth for loadable-state safety.
7. **§7 — Non-goals for v1.**
8. **§8 — Open questions handed to the ADR wave.**

---

## 1. The decision, in one page

**We will build `WG-Fed`: a self-certifying, key-rooted federation layer whose
*trust root is decentralized and never central*, which *rests on an optional
always-on WG node* for availability, recovery, discovery, and anti-abuse, uses
*UCAN-style attenuating delegation* for agent custody, and *preserves a node-less
P2P mode* as a first-class option.**

In the study's vocabulary: **the recommended architecture is Candidate C (the
key-rooted hybrid), deployed by default in a Candidate-B shape (a per-household/org
WG node), with Candidate D's UCAN layer grafted in as the delegation/custody
mechanism, and Candidate A's node-less mode preserved as the decentralization
option.** This is exactly the synthesis doc 04 §6.3 floated and doc 05 §7.1
defended adversarially — adopted here as the decision.

**The decentralization-vs-central call, stated plainly (this is HQ6, and it is the
spine of the whole design):**

> **Decentralized at the trust layer — absolutely. Central at the availability
> layer — by default.**
>
> - **Identity verification is *never* central.** "Is this artifact really from
>   Nora?" is always a local signature check against a self-verifying sigchain
>   rooted at Nora's genesis pubkey. No node, directory, relay, DNS name, or CA is
>   ever in the correctness/security path. This is non-negotiable (FR-F4/F5, V4).
> - **Everything else may be central, and by default *is*.** Message relay/inbox,
>   state hosting, discovery, alias resolution, recovery anchoring, and anti-abuse
>   all default to an always-on **WG node** per household/org, because — for the
>   *long term* — an always-on node is the reliable choice for those jobs (doc 05
>   ranks the node model strongest on Recoverability, Maturity, and anti-abuse).
> - **The invariant that makes resting-on-central safe:** *every central component
>   is a hint that can only help, never override a self-verification.* Fail-safe by
>   default — lose the directory, fall back to self-verify; be served a forged
>   directory, ignore it, because it cannot override the local sigchain check. This
>   neutralizes the node's worst case (doc 05 B-1, node compromise) **at the
>   protocol level**: a fully-compromised node still cannot forge what a verifier
>   self-checks.
> - **The node-less, self-host-everything deployment stays real** (NFR-5) — it is
>   the decentralization option, with its own (mandatory) recovery ceremony.

**The headline defense (full version in §2.2, all citations to doc 05):** doc 05
ranks the whole architectures **B > C > A > D** for a security/reliability gate,
and its §7.1 explicitly frames the result not as "pick B" but as "**ship B's node,
keep the self-certifying core as the correctness root (that *is* C), graft D's
UCAN, preserve A as the option** — the only arrangement in which no Fatal finding
remains unbounded." We adopt that arrangement. We reject **A as v1** (its
hydra-vs-recovery bind is the single strongest adversarial argument against it,
doc 05 §4.1; recovery is Fatal for agents and the careless, A-4) and **D as an
identity root** (its DNS/CA/web-host anchor is the easiest impersonation surface of
the four and contradicts the self-certification the study exists to protect — D-1/
D-2, Fatal-as-root) — while **harvesting D's UCAN, the best-scoring single
component in the study** (doc 05 §7.1).

**Three things we commit to engineering regardless of any later re-litigation** —
the unavoidable substrate findings doc 05 §8 hands us: **S-3** revocation liveness
(the freeze attack), **S-4** delegation self-perpetuation (the hydra) and its
recovery-flexibility cost, and **S-5** loadable-state poisoning (the AI-specific
threat with no prior-art precedent).

---

## 2. The recommended architecture — `WG-Fed`

### 2.1 What it is (the substrate, fixed)

`WG-Fed` adopts doc 04 §1's shared substrate wholesale — that substrate is *not* a
candidate, it is the agreed foundation under all four, and nothing in the
adversarial pass broke it (doc 05 §2 verdict: none of S-1…S-7 is Fatal). Concretely:

- **A three-tier key hierarchy** (doc 04 §1.1): a **root/identity key** (ed25519)
  that *is* the identity and signs the sigchain; a **signer/device/agent key**
  (ed25519) that signs day-to-day events and state within delegated scope; an
  **encryption key** (X25519) that realizes per-recipient ACLs. The root never does
  day-to-day work and never leaves its custodian.
- **A sigchain** (doc 04 §1.2): a per-identity append-only, hash-linked, signed log
  (`genesis` / `add_key` / `revoke_key` / `rotate_root` / `delegate` /
  `set_alias_proof` / `set_endpoints`). This single structure discharges rotation,
  recovery, revocation, audit, delegation, and the fork-vs-same-self continuity
  decision. **This indirection layer is mandatory** — doc 01 §4.2's single most
  important lesson is that *every* system achieving identity continuity (Keybase,
  atproto/DID, Farcaster) does it via "identity is a stable name; keys are
  revocable contents of a signed record," and every system without it (SSB, Nostr
  as-shipped, raw Iroh NodeId) *cannot* offer long-term reliability (V6).
- **An audited crypto suite** (doc 04 §1.3, FR-S5 — invent nothing): ed25519,
  X25519, XChaCha20-Poly1305, HKDF-SHA-256, BLAKE3 CIDs, optional MLS/Double-Ratchet
  for forward-secret groups. Every structure carries an explicit `alg` id for
  crypto agility.
- **Three self-describing, versioned wire envelopes** (doc 04 §1.4): `IdentityRecord`
  (the public, portable, key-free identity), `StateSnapshot` (loadable state with
  the evolvable `payload_kind` slot), `SignedEvent` (a signed, optionally-sealed
  message). All content-addressed by BLAKE3.
- **A new `src/identity/` module** (doc 04 §1.5) as the home for all crypto, plus
  surgical touch-points in `src/secret.rs` (signing custodian), `src/federation.rs`
  (key/URL peers + signed-artifact transfer), `src/agency/types.rs` (`Agent` gains
  `pubkey`/`sigchain_head`/`delegations`; `contact` → routed `endpoints`), and
  `src/messages.rs` (`from`/`to`/`sig`/`refs`, cross-graph send/poll).
- **A compat handshake** `WG_FED_COMPAT_VERSION` in `src/identity/mod.rs`, mirroring
  `WG_AGENCY_COMPAT_VERSION` (`1.2.4`) and `WG_PI_PLUGIN_COMPAT_VERSION`, **fail-loud
  on mismatch** — and (the adversarial addition, S-7) **authenticated**: the
  negotiated parameters are signed, not merely exchanged, to defeat downgrade.

### 2.2 What it configures (the C-with-B-default-and-D's-UCAN choices) and why

`WG-Fed` is Candidate C — the self-certifying core that *can* rest on optional
central nodes — pinned to a specific, opinionated default configuration so it is
**one architecture, not a menu**:

| Dimension | `WG-Fed` choice | Lineage |
|---|---|---|
| **Trust root** | Self-certifying `wgid:<pubkey>` + sigchain; never central | A / Nostr / did:key |
| **Default deployment** | A per-household/org **WG node** (the existing daemon, promoted) holds agent signing keys and serves inbox/state/directory | **B** / atproto PDS |
| **Agent custody / delegation** | **UCAN** short-lived, scoped, attenuating capabilities + Farcaster-style sigchain signers | **D's UCAN** + Farcaster |
| **Node-less mode** | First-class, with a *mandatory* recovery ceremony | **A** preserved |
| **Transport** | Node HTTP store-and-forward by default; Iroh/relay fallback ladder | B default, A/C fallback |
| **Addressing alias** | Verifiable petnames + optional node handle; **did:web rejected as a root** | C; **not** D-as-root |

**Why C and not B outright** (doc 05 ranks B #1 overall): doc 05 §7.1 is explicit
that B's #1 ranking is "the survivable, proven, nearest option a security gate
trusts *today*" and that **"C is where you want to end up; you get there *through*
B, not instead of it"** — because **B is reachable inside C** (B is a configuration
of C, doc 04 §4.9). Picking C *with a B-shaped default* gives us B's entire
score-sheet (Recoverability 5, Maturity 5, WG-fit 5, Security 4) **plus** the
self-certifying correctness root that turns B's one Critical finding (B-1, node
compromise → mass impersonation) from "recoverable-after-the-fact" into
"can't-forge-in-the-first-place." We pay C's price (doc 05 ranks C below B on
**Simplicity 2** and **Maturity 2**, C-1/C-2 — the largest surface to
misconfigure, an unproven composition) and we pay it deliberately, with the
mitigations doc 05 names: **fail-safe-by-default, a strict mode, and a linted
resolution cascade** (WG already ships `wg config lint`). The phased rollout (§5)
is what makes C's "largest build" tractable — each phase is independently
auditable, and C is reached by *convergence*, not a big-bang (doc 05 C-2).

**Why we reject A as v1** (doc 05 ranks A #3): A wins Decentralization outright (5)
and has the purest integrity story, but it is "**operationally most fragile**"
(doc 05 §4.1) and **weakest on exactly the axes a reliability gate weights** —
Recoverability 2, anti-sybil weakest (A-2, FR-T2 deferred), and the
**hydra-vs-recovery bind** (S-4 × A): A's *only* node-less recovery primitive ("a
surviving authorized key adds a new signer") *is* the exact capability the hydra
needs, so "you cannot have both cheap surviving-key recovery and hydra-resistance"
(doc 05 §4.1, §5.3). A-4 is Fatal for agents and the careless. We do not discard A
— we **preserve it as the node-less option C carries** (doc 05 §7.1 point 4), with
the recovery ceremony *mandatory* and `add_key` *locked to root* wherever a
deployment runs node-less.

**Why we reject D as an identity root** (doc 05 ranks D #4): D has "**the best
delegation/custody layer of all four bolted onto the weakest identity root**" (doc
05 §4.4). Its did:web/DNS/CA anchor is the *easiest impersonation of the four* — a
forged `did.json` is a complete takeover by anyone who can compel a CA, hijack DNS,
or seize a domain (A8) — which is **the exact attack the study exists to prevent**
and **contradicts V4/FR-I1** (D-1/D-2, Fatal-as-root). The only escape (the did:key
fallback) "dissolves D into C with a directory." So we **harvest D's UCAN** — "the
best component in the study" and "first among components" (doc 05 §5.3, §7.1) — and
**reject did:web as the root** (did:web survives at most as one *verifiable alias*
among others, never the anchor).

**The custody-split verdict we are designing to** (doc 05 §3.2): the
"download ≠ impersonation" split holds *on paper* in all four, but is *structural*
only where there is a custodian-of-record — **D ≈ B > C > A**. `WG-Fed` therefore
adopts the *structural* end of that spectrum: a custodian holds the only copy of
the root, download confers an *expiring capability* and never *oracle access* (S-2),
and same-self enrollment requires a control the downloader lacks (root/M-of-N, S-4).
This is the resolution to **the irony doc 05 §3.3 exists to surface** — V4
(non-impersonation) and V5 (decentralization) pull against each other *at the
custody layer* — `WG-Fed` keeps a custodian-of-record *underneath* a self-certifying
root, so it gets the structural split without making the root central.

---

## 3. Decision register — every doc-03 hard question, decided

Format per question: **DECISION** · *why* · *rejected (cite doc 05 where adversarial)*
· *cost we accept*. Every one is decided; nothing is deferred silently.

### HQ1 — Agent key custody **(THE CRUX)** — *DECIDED*

**DECISION.** Three-tier key hierarchy (§2.1). The **portable identity =
`IdentityRecord` + `StateSnapshot`s + the public key set + currently-authorized
delegations — and never the root private key** (FR-I2, FR-S1).
- **Where the root lives:** *Humans* self-hold it on a device/OS keychain,
  hardware-backed (FIDO2/passkey) where available. *Agents'* root is
  **custodian-held** — by the WG node operator when a node is present, by the human
  owner in node-less mode — in `wg secret` (or an HSM), behind an **ssh-agent-style
  "sign this digest" boundary**. The worker requests signatures (or is issued a
  short-lived signer/UCAN); it **never receives the root bytes**.
- **How an agent acts:** under a **short-lived, scoped, attenuating UCAN** (default)
  and/or a sigchain-authorized standing signer. Default to short-lived UCAN per
  session so a stolen agent signer is near-worthless after expiry.
- **"Download Nora onto host B":** **fork by default** — a verifiable, read-only
  copy (render Nora's history; cannot sign as Nora). *Same-self* requires an
  explicit, signed `add_key`/`delegate` link by a surviving authorized key, **or** a
  node-mediated migration op. Never automatic. This makes FR-I5's fork/same-self
  boundary cryptographically unskippable.

*Why.* Adopts the doc 01 §4.1 winning pattern (Farcaster signers / UCAN / ssh-agent
custody) and the *structural* custody end of doc 05 §3.2 (D ≈ B > C > A). Enforces
the three controls doc 05 §3.2 names: bundle provably excludes the key (S-1),
download confers no oracle access (S-2), enrollment needs a control the downloader
lacks (S-4).

*Rejected.* A's "standing signer on the worker + any-authorized-key-can-`add_key`"
custody, which doc 05 §3.2 rates *weakest* ("the custody boundary is the user's own
discipline"). Sharing a private key as the portability mechanism (collapses HQ1 —
rejected in doc 03 itself).

*Cost.* Per-host enrollment friction — "download and it just works" requires an
explicit re-authorization step (S-2). That friction *is* the fork/same-self
boundary made correct and unskippable.

### HQ2 — Key rotation & recovery — *DECIDED*

**DECISION.** Identity continuity via the sigchain (address = stable genesis root
pubkey; keys are rotatable/revocable contents). Recovery is **layered by
deployment**:
- **Default (node present):** node-held rotation keys **+ a human-held *offline*
  recovery key with a time-boxed override window** (atproto's model, doc 01 §4.2
  ★★★) — recoverable even against a *hostile* node.
- **Node-less mode:** a **mandatory genesis ceremony** — a paper key **and** M-of-N
  social-recovery guardians named at genesis (non-optional; this is what defuses
  Fatal A-4).
- **Agents:** recovery *always* collapses to "the custodian's key is safe" (doc 05
  §5.3) — agents are never purely-P2P-recoverable; the custodian (node/owner) is the
  recovery anchor by design (FR-S6).
- **Revocation:** a verifiable `revoke_key` sigchain link, **plus freshness
  attestations** (a "valid-as-of T, expires T+Δ" the verifier re-fetches) so an
  eclipse/freeze cannot keep a revoked key alive (S-3); high-value actions
  **fail-closed on stale**.

*Why.* B has the strongest recovery of the four (doc 05 Recoverability 5); C
composes A's *and* B's options. The offline recovery key is the one mechanism that
makes even mass node-compromise recoverable (doc 05 §4.2, B-1).

*Rejected.* A-as-only-recovery (social-M-of-N alone), which doc 05 rates
*Recoverability 2* and *Fatal-careless* (A-4). Domain-control recovery (D's model),
*Fatal-as-primary* (D-2). Accept-loss / immutable identity (violates V6).

*Cost.* The offline recovery key is itself a standing takeover capability (doc 05
B-4) — mitigated by holding it offline/hardware, optionally M-of-N split, with a
*visible* time-locked override. The freshness requirement intrudes slightly on
offline tolerance and adds clock dependence (S-3).

### HQ3 — Transport — *DECIDED*

**DECISION.** **Pluggable fallback ladder with the WG node's HTTP store-and-forward
inbox as the default.** Order: node inbox (default — always-on, NAT-free,
operationally simplest, reuses the existing daemon) → Iroh QUIC direct path when
both peers are online → optional shared relays. **No single relay/node is mandatory**
(FR-F4): the same `SignedEvent` traverses any of them; removing one degrades reach,
not correctness (FR-F5). Bytes are **signed and (optionally) sealed end-to-end**, so
the transport — *including your own node* — is always untrusted. Email-speed,
both-ends-offline-tolerant (NFR-2, FR-M2).

*Why.* B's node inbox is "operationally the simplest" and the shortest path from
today's code (doc 05 WG-fit 5; doc 02 §2.1 — federation is *already* a daemon
brokering over a socket). The fallback ladder preserves the decentralization option
without making correctness depend on any node.

*Rejected.* Pure-P2P-only transport (A) — heavy NAT traversal, no always-on
availability. A single mandatory relay/node (violates FR-F4/F5).

*Cost.* The node sees routing metadata (HQ4/B-2). Maintaining multiple transport
adapters is part of C's coherence tax (C-1/C-2).

### HQ4 — Encryption = ACL + metadata — *DECIDED*

**DECISION.** Per-recipient **sealed envelopes** (X25519 + XChaCha20-Poly1305); the
`to` set *is* the ACL (FR-S3). Groups use sender-keys with rekey-on-membership-change;
**MLS/Double-Ratchet is opt-in for online or long-lived groups only**.
- **Forward secrecy (S-6, inherent-bounded):** the default offline store-and-forward
  path uses **static recipient keys (no forward secrecy)** — you cannot ratchet with
  a party who is not there to ratchet back, so FS and send-to-offline *do not
  compose*. FS is available only on the online/long-lived path. Static-key
  compromise retro-decrypts logged ciphertext; we **cap this with enc-key rotation**
  and disclose it.
- **Metadata (FR-S4):** disclosed, not eliminated. The node/relay sees `to` (and
  `from` unless sealed-sender is used). We offer **sealed-sender** to hide `from`
  from *peer* nodes. We explicitly **do not** promise recipient-unlinkability or
  mixnet-grade anonymity (non-goal §7). In the node deployment, *your own node sees
  your whole social graph* (doc 05 B-2, the worst metadata posture of the four) —
  self-hosting your node makes *you* the observer; that is the accepted trade.

*Why.* Encryption-as-ACL is R24's elegant collapse and the only model that survives
on untrusted relays. Static-key-for-offline is the honest consequence of the
email-speed budget (doc 05 §S-6).

*Rejected.* Mandatory forward secrecy on all paths (doesn't compose with offline —
S-6). Server-side-permission-check ACLs as the *boundary* (never trust a peer node —
encryption is the real boundary; same-node server ACLs are a *convenience* only).

*Cost.* No FS on the default path (capped by rotation, S-6). Metadata leak to the
node (B-2), bounded and disclosed. Plaintext/crypto-downgrade surface (S-7) →
mitigated by per-conversation **MUST-encrypt** policy + a minimum-`alg` floor +
authenticated handshake.

### HQ5 — Addressing — *DECIDED*

**DECISION.** The address is **`wgid:<multibase-ed25519-pubkey>`** (self-certifying;
the Nostr-npub / did:key family). Resolution is a **fallback cascade**: cached signed
endpoint record → optional directory hint → DHT/Iroh discovery. Any one suffices; no
resolver is mandatory (FR-F1/F4). The address is **stable under rotation** = the
genesis root pubkey; the sigchain rotates the active key set underneath (FR-I7).
- **Aliases:** local **petnames** + an optional **verified handle** (e.g.
  `@nora.garrison.family` via DNS/`.well-known` or a Keybase-style social proof,
  *checkable back to the key*). Never a mandatory central naming authority; alias
  loss never compromises the key identity (FR-F2).
- **did:web is explicitly *not* the identity root** — at most one verifiable alias
  among others.

*Why.* Secure + decentralized by default (Zooko: we pick those two for the root),
opt into human-meaningful via a verifiable alias. Consistent with prior-art formats
(Nostr/did:key) for interop (doc 01 §5).

*Rejected.* did:web/DNS as the root (doc 05 D-1, Fatal-as-root — easiest
impersonation of the four). A mandatory global registry/naming authority (non-goal,
re-centralizes).

*Cost.* Raw `wgid:` keys are not human-memorable (the Zooko price); the alias layer
is opt-in convenience, and first-contact key→human binding remains a social problem
(HQ8, A-1).

### HQ6 — Decentralization vs central nodes — *DECIDED (the plain call)*

**DECISION.** Stated plainly in §1 and restated as the per-capability table below.
**Identity verification is never central** (CC = correctness-critical). **Every
other capability may be central and defaults to the WG node** (CV = convenience). The
binding invariant: **a central component is a hint that can only help, never override
a self-verification** — fail-safe, never fail-open.

| Capability | Criticality | `WG-Fed` decision |
|---|---|---|
| Identity verification (sig check) | **CC** | **Self-certifying. Never central.** Local check vs the sigchain rooted at the `wgid:` genesis pubkey. |
| Message relay / inbox | CV | **WG node by default**; Iroh/relay fallback; ≥1 alternative always possible. |
| State hosting | CV | WG node by default; also any of relay / IPFS / plain file. |
| Alias / discovery | CV | Petnames (no infra) + optional node directory + DHT. |
| Key directory / resolution | CV | Cascade: cache → optional directory → DHT. **Fail-safe**: a forged directory cannot override the local sigchain check. |
| Recovery anchor | CV | Offline recovery key + node (default); social M-of-N (node-less). |
| Anti-abuse choke point | CV | Node rate-limit/consent (default); PoW + consent (node-less). |

*Why.* This is the literal reading of vision pillar **V5** ("lean decentralized,
central nodes *allowed*"), made an engineering invariant. It resolves doc 05's
central-node finding (B-1) at the protocol level (a compromised node can't forge a
self-checked artifact) and keeps the FR-F4/F5 guarantee that *no correctness- or
security-critical capability depends on a single central node*.

*Rejected.* Making identity verification depend on a directory/node/DNS (A's only
real opening per doc 05 §3.1 D-resolver; D's whole anchor). Making *no* capability
central (pure A) — sacrifices the reliable availability/recovery the long term needs
(doc 05 Recoverability/Maturity).

*Cost.* The optionality is itself an attack/misconfig surface (doc 05 C-1):
mitigated by **fail-safe defaults + a strict mode + linting the resolution cascade**,
and paid for with the largest test matrix of the four.

### HQ7 — Consistency model — *DECIDED*

**DECISION.** **Single-writer-per-object is the spine.** Your key is authoritative
for your own identity state → identity state has **no conflicts** by construction.
The owning node (when present) is a serialization point for objects it hosts (B's
model), but **correctness never requires it**. Shared mutable objects (co-edited
threads, shared tasks) use **CRDTs** (auto-merge) or LWW with version vectors that
**surface conflicts, never silently drop** them. Ordering is **causal per
conversation** via `refs`, not global (FR-M6).

*Why.* Single-writer sidesteps most conflicts and fits the self-certifying model
(your sigchain is yours alone). It is consistent with the fork/continuity decision
(HQ1).

*Rejected.* A global authoritative writer / total ordering (violates async +
decentralized, V3/V5). Silent last-writer-wins that loses data.

*Cost.* In node-less mode (A) the **sigchain itself can fork under partition** (doc
05 A-3, TC6) — bounded by a witness/checkpoint *or* by treating a fork as a
policy-level identity split. A witness re-introduces a semi-central node (the
node-default deployment avoids this entirely, doc 05 TC6 → B Low).

### HQ8 — Trust establishment & anti-abuse — *DECIDED*

**DECISION.**
- **First contact:** TOFU (pin the key, re-checkable thereafter) + verifiable proofs
  (Keybase-style social proofs / NIP-05 handles) + web-of-trust from the peer graph.
  No mandatory CA (FR-T1). High-value contacts use OOB fingerprint compare (mitigates
  A-1, first-contact substitution).
- **Anti-abuse:** the WG node is the natural choke point → per-node rate-limits,
  allow/block lists, **consent gates** (an unknown `from` lands in a *requests* tray,
  not the inbox), reputation tied to `trust_level`. Node-less path adds **proof-of-work**
  (hashcash) + consent gates. Integrates with WG's existing `trust_level`
  (Verified/Provisional/Unknown) to gate dispatch (FR-T3).

*Why.* Anti-abuse is strongest exactly where we accepted the node (doc 05 B: sybil/
spam Low, "the easiest of the four"); the node-less path gets the best *available*
no-anchor controls.

*Rejected.* A central CA / global gatekeeper (re-centralizes, excludes newcomers).
Relying on PoW alone at scale (doc 05 A-2: "never solved" without an anchor).

*Cost — disclosed deferral.* **FR-T2 (sybil/spam) is strong with a node, weaker
node-less** (PoW + consent only, no anchor) — doc 04 §8.2 flagged it and doc 05 A-2
confirms it is "mitigable, never solved" without an anchor. This is inherent to the
decentralization option and is disclosed, not silently met.

### HQ9 — Human vs agent identity — *DECIDED*

**DECISION.** **One identity type with capability flags** — keep WG's current unified
`Agent` (doc 02 §2.2, the actor/agent ADR). The differences are **by-design**
(FR-S6), not emergent:

| | Human | Agent |
|---|---|---|
| **Root custody** | Self-held (device/passkey, hardware-backed) | Custodian-held (node/owner), via ssh-agent-style signing boundary |
| **Day-to-day signing** | Device signer | Delegated UCAN signer (short-lived) |
| **Recovery** | Device set + offline recovery key + social M-of-N | Falls back to the custodian (never independently recoverable) |
| **Authority** | Standalone | Standalone for agent-native work; **delegated** when acting *for* a principal (→ HQ11) |
| **Lifecycle** | Rare creation | Created/cloned/retired often → cheap signer issuance, fork-by-default on copy |

*Why.* The vision insists both are first-class (V7) and WG already unified them; the
differences live in custody/recovery/authority, not in the type. No human-only
assumption (biometric, phone) sits on a path agents must traverse.

*Rejected.* Two separate identity types (re-opens the actor/agent split the shipped
ADR closed). Treating agents and humans identically (yields insecure humans or
unusable agents — doc 03 HQ9).

*Cost.* Agents are *never* purely-P2P-recoverable — agent recovery always implies a
custodian (doc 05 §5.3). Accepted: it is the honest consequence of agents having no
fingers, phones, or hardware tokens.

### HQ10 — Loadable-state format — *DECIDED*

**DECISION.** One stable `StateSnapshot` envelope (doc 04 §1.4b) with a **tagged,
evolvable `payload_kind` slot** (`conv-cache-v1` / `summary-v1` / `opaque-blob-v1` /
future). Content-addressed (BLAKE3) + signed (FR-I4). `model_binding` guards
wrong-model loads. `prev` enables **incremental publish** (append a turn, never
re-upload history). Unknown `payload_kind` **degrades gracefully** — verify
signature + provenance, surface "state present, payload unreadable by this client,"
never silently corrupt (NFR-4). We design the **slot, not the opaque payload**
(non-goal §7).
- **Security carve-outs the adversarial pass forces (load-bearing):**
  - **S-1:** FR-S1's "no field carries a private key" is **downgraded from a static
    guarantee to a runtime-containment guarantee for opaque kinds** — an opaque blob
    is un-introspectable and could smuggle a key. The defense is that **the custody
    boundary holds the *only* copy of the root/signer**, so there is nothing in the
    worker's address space to bake into a blob; opaque payloads are additionally
    sealed and treated as untrusted.
  - **S-5:** **loaded state is untrusted input.** A signature proves *who authored*
    state, never that it is *safe to load*. Mitigation: sandbox/scan, enforce
    `model_binding`, **provenance-gate** (load only from authors at sufficient
    `trust_level`, FR-T3), human-in-the-loop for cross-trust loads. This AI-specific
    threat has no Nostr/Keybase/atproto precedent and **must be budgeted** (doc 05 §8).

*Why.* The stable-interface/evolvable-payload split is the only way to serve today's
conversation cache *and* tomorrow's opaque RNN state (V1, R2, doc 03 HQ10) without a
rewrite. The carve-outs are non-negotiable security consequences from doc 05.

*Rejected.* Hard-coding today's conversation-log shape (needs a rewrite later). A
format so abstract it's useless now. Treating a valid signature as a safety
guarantee (doc 05 S-5: signature ≠ safety).

*Cost.* Erodes the seamless-resume UX and requires an **AI-input-safety layer WG does
not have today** (S-5); the static-no-key guarantee weakens to runtime containment
for opaque kinds (S-1).

### HQ11 — Authority, delegation & accountability — *DECIDED*

**DECISION.** **UCAN-style capability certificates** (grafted from Candidate D — "the
best component in the study," doc 05 §7.1) are the delegation mechanism.
"Agent X may act for human Y, scope S, until T" = a signed, checkable, revocable,
**expiring** capability (FR-T4).
- Delegation **never shares a private key** (FR-S1).
- **Sub-delegation is attenuating-only** — can narrow, never widen, inherits the
  parent's expiry. This is what structurally kills the **hydra** (S-4).
- **Revocation:** by-expiry (default; short expiries) + issuer-subtree revocation
  (kill the parent → kill the whole subtree) + sigchain `revoke_key`.
- **Accountability:** actions are attributable to **both** the agent signer and the
  principal (the UCAN chain records `iss`/`aud`); the append-only sigchain is the
  audit trail (NFR-7).

*Why.* UCAN scores best of all four on TC2 (smallest signer blast radius) and TC9
(attenuating + expiring sub-delegation), and is "the only candidate component that
makes the downloaded-identity split *structural* rather than disciplinary" (doc 05
§7.1).

*Rejected.* Shared-key delegation (collapses HQ1). Blanket/non-expiring delegation
(leaked agent key acts as the human indefinitely — doc 03 T9). Relying on
fast-revocation-at-scale (UCAN's open problem, doc 05 D-3) instead of short expiries.

*Cost.* Short expiries mean **chatty re-issuance**, which fights offline tolerance
(doc 05 D-3); accepted as the price of a small blast radius. `add_key`/root-rotate is
restricted to root/M-of-N (S-4) — which in node-less mode removes A's surviving-key
recovery primitive (the recovery-flexibility cost S-4 spells out, accepted).

### HQ12 — Protocol evolution & long-term compatibility — *DECIDED*

**DECISION.** `WG_FED_COMPAT_VERSION` in `src/identity/mod.rs`, mirroring
`WG_AGENCY_COMPAT_VERSION` (`1.2.4`) and `WG_PI_PLUGIN_COMPAT_VERSION`. Every signed
structure carries an explicit `alg` id (crypto agility). On first contact peers
exchange the version and **fail loudly on incompatible mismatch** (WG's convention),
negotiating the shared subset otherwise.
- **The adversarial requirement (S-7):** the handshake **must be authenticated** —
  the negotiated parameters are *signed*, not merely exchanged, so A1 cannot strip
  strong crypto or force "lowest common `alg`." Enforce a **minimum-`alg` floor**
  with aggressive retirement (refuse known-weak), not "lowest common."
- **Crypto migration:** dual-sign during a window; an `alg` change (e.g. ed25519 →
  ML-DSA post-quantum) is a sigchain method addition — **no identity is abandoned**.

*Why.* Mirrors WG's existing loud-fail compat convention; crypto agility lets a
primitive be retired without orphaning years-old identities (V6, doc 03 HQ12).

*Rejected.* Implicit/sniffed versioning. A fixed crypto suite with no migration path.
Best-effort silent degrade (WG's convention is loud-fail).

*Cost.* Policy must be maintained and enforced; retiring an `alg` loudly breaks old
artifacts (acceptable per doc 03 HQ12, but real — S-7).

### Decision-register summary

| HQ | Topic | `WG-Fed` decision (one line) |
|---|---|---|
| HQ1 | **Agent key custody** | 3-tier keys; portable = public+state+delegations, never root; custodian-held agent root; fork-by-default; UCAN signers |
| HQ2 | Rotation/recovery | Sigchain continuity; offline recovery key + window (node) / mandatory paper+M-of-N ceremony (node-less); freshness attestations |
| HQ3 | Transport | Node HTTP store-and-forward default; Iroh/relay fallback ladder; untrusted transport; no mandatory relay |
| HQ4 | Encryption = ACL | Per-recipient sealed envelopes = ACL; static-key offline (no FS), MLS online; metadata disclosed; sealed-sender |
| HQ5 | Addressing | `wgid:<pubkey>` self-certifying; cascade resolution; verifiable optional aliases; **did:web ≠ root** |
| HQ6 | **Decentralization vs central** | **Trust root never central; everything else central-by-default; central = a hint that can't override self-verify** |
| HQ7 | Consistency | Single-writer spine; CRDT/LWW-with-vectors for shared; causal order; node as optional serializer |
| HQ8 | Trust/anti-abuse | TOFU + proofs + WoT; node choke-point anti-abuse (default), PoW+consent (node-less); FR-T2 weaker node-less (disclosed) |
| HQ9 | Human vs agent | One type, capability flags; agents custodian-held + delegated, never independently recoverable |
| HQ10 | Loadable-state | Stable envelope + evolvable `payload_kind`; signed+CAS+incremental; **state = untrusted input** (S-5); runtime-containment for opaque (S-1) |
| HQ11 | Authority/delegation | UCAN: signed, scoped, expiring, **attenuating-only** (kills hydra); attributable to agent+principal |
| HQ12 | Protocol evolution | `WG_FED_COMPAT_VERSION` loud-fail + **authenticated** handshake; `alg` agility; dual-sign migration; min-alg floor |

---

## 4. The spark test — "Two graphs, one key, a third location"

**Purpose.** The spark test is the **minimal end-to-end proof** that validates the
whole `WG-Fed` choice, and the **first implementation milestone** the rest of the
system runs across. It proves that the four pillars doc 02 §3 confirmed *absent* —
**identity keys, signed messages, cross-WG addressing, portable signed state** —
exist and compose, authenticated *purely by keys*, with **no shared filesystem**
(the limit of today's `federation.rs`) and **no central trust anchor**.

It is deliberately scoped to the smallest thing that exercises every load-bearing
decision in §3 *and* the headline attack of doc 05 §3 — and nothing more.

### 4.1 Setup

- **WG-A** — Alice's WG instance.
- **WG-B** — Bob's WG instance, on a **different host with no shared filesystem**
  with WG-A (this is the wall today's same-FS federation cannot cross — doc 02 §2.1).
- **A third location `L`** — a *dumb, untrusted* HTTP/object store that neither WG-A
  nor WG-B controls as a *trust anchor* (a static file host, an S3 bucket, an IPFS
  gateway — anything that returns bytes). `L` is explicitly **not** trusted: every
  artifact it serves is self-verifying.

### 4.2 The seven steps (each a falsifiable assertion)

1. **Mint a self-certifying identity.** `wg identity new alice` on WG-A generates an
   ed25519 root keypair **into `wg secret`** (the custody boundary), writes a
   `genesis` sigchain link, and emits `wgid:<alice-pubkey>` + a signed
   `IdentityRecord`.
   **Assert:** the root private key never appears outside the keystore — neither in
   the `IdentityRecord` nor in any worker-reachable file/env (scan + spec-check).
   *(Validates HQ1, FR-S1.)*

2. **Publish to the third location.** WG-A publishes Alice's `IdentityRecord` (+ one
   `StateSnapshot`, `payload_kind: conv-cache-v1`) to `L`. The bundle is signed and
   BLAKE3-content-addressed and **carries no private key**.
   **Assert:** a field-scan + format spec-check of the published bytes finds no
   private-key material. *(Validates FR-I2/I4, NFR-3.)*

3. **Fetch + verify offline, with the origin down.** WG-B fetches Alice's
   `IdentityRecord` from `L` by `wgid:` alone and **verifies it offline** — the
   signature checks against the genesis pubkey *embedded in the address* (no call to
   WG-A, no central authority). Then **WG-A is taken offline** and WG-B re-verifies
   from its cache + `L`.
   **Assert:** verification passes with WG-A offline; flipping any byte of the fetched
   record makes verification fail. *(Validates FR-I1/F1/F5, self-certifying root,
   HQ6's "verification never central.")*

4. **Send a signed (optionally sealed) cross-graph message.** WG-B sends Alice a
   `SignedEvent` addressed to `wgid:<alice-pubkey>`, signed by Bob's signer key and
   **optionally sealed** to Alice's X25519 key, delivered **store-and-forward while
   WG-A is offline** (spark transport = the simplest available: an HTTP inbox or a
   shared relay).
   **Assert:** the event is accepted for delivery with the recipient offline.
   *(Validates FR-M1/M2/M3, HQ3.)*

5. **Receive + authenticate by key.** WG-A comes online, polls, receives Bob's
   message, and verifies Bob's signature against Bob's `wgid:` (resolved/pinned via
   the same self-certifying mechanism).
   **Assert:** the genuine message verifies; a **forged "from Bob"** event (wrong
   signature) **fails**; if sealed, only Alice's key decrypts it and a third party
   holding the ciphertext **cannot**. *(Validates FR-M1, FR-S3, HQ4.)*

6. **The headline attack — downloaded-identity ≠ impersonation.** A thief (`A6`) who
   has Alice's *published bundle* from `L` attempts to author a **new** event that
   WG-B accepts as Alice.
   **Assert:** it **fails** — the bundle contains no private signer, and the sigchain
   authorizes none that the thief holds. The custody split holds. *(Validates the
   doc 05 §3 crux directly — this is the single most important assertion in the
   milestone.)*

7. **Re-fetch from a third location, by a third party.** A fourth party (or WG-B from
   a fresh host with no prior state) fetches Alice's identity from `L` and
   re-verifies — proving portability and self-certifying verification *independent of
   origin*.
   **Assert:** re-verification succeeds against `L` alone, with WG-A still offline.
   *(Validates V2, NFR-3, FR-F5.)*

### 4.3 What the spark deliberately leaves out (so it stays minimal)

- **No rotation/recovery** (that is Wave 5 / ADR-003) — the spark uses one signer.
- **No UCAN delegation** (Wave 6 / harvested from D) — the spark uses a single
  sigchain-authorized signer; delegation is proven later.
- **No required encryption** — sealing is *optional* in the spark; the must-prove is
  *signature authentication*. (Encryption-as-ACL is exercised by the optional seal in
  steps 4–5 but not gated.)
- **No node/directory required** — the spark runs with `L` (a dumb host) as the
  third location and a minimal inbox/relay; it does **not** require the full WG node.
  (It is *compatible* with the node being the transport, but does not depend on it —
  proving HQ6's "central is convenience, not correctness.")
- **No human-friendly alias** — addressing is raw `wgid:` only; the alias layer is
  later.

### 4.4 Done-criteria (this is the Wave-3 milestone gate)

The spark is **passed** when all seven assertions hold in an automated scenario, and
that scenario is **landed as a permanent smoke gate**:
`tests/smoke/scenarios/federation_spark_two_graphs.sh`, listed in
`tests/smoke/manifest.toml` `owners` for the spark task (the manifest is grow-only —
CLAUDE.md). Passing this scenario is the empirical proof that the `WG-Fed` choice is
buildable and correct; everything in §5 builds *across* it.

---

## 5. Phased roadmap

The roadmap continues doc 04 §9's phasing (deliberately **candidate-agnostic through
Phase 2** so the wire is proven before the topology hardens) and binds it to waves,
dependencies, and guardrails. Each wave is independently valuable (NFR-6).

```
Wave 2 (ADRs) ──► Wave 3 (Spark PoC) ──► Wave 4 (cross-graph + transport)
                                              │
                                              ▼
                            Wave 5 (portable state + recovery)
                                              │
                                              ▼
                  Wave 6 (encryption=ACL + UCAN delegation + optional central pieces)
```

### Wave 2 — ADRs (draft + accept *before* any federation code)

Draft and accept the load-bearing ADRs (stubs in §6). **No Phase-0 code lands until
ADR-001/002/003 are Accepted.** Dependencies: this memo. Deliverables:
ADR entries (following the repo's existing `docs/ADR-*.md` convention, e.g.
`docs/ADR-actor-vs-agent-identity.md`) for the identity/key model, transport,
custody/recovery, and
loadable-state safety. *Why first:* the study is the analysis; the ADRs are the
commitments the implementation cites — and three substrate findings (S-3/S-4/S-5)
plus two Fatal findings (A-4, D-1/D-2) must be designed *in*, not discovered in code.

### Wave 3 — The Spark PoC (Phase 0 + the thinnest slice of Phase 1)

Implement the minimum to pass §4's spark test:
- `src/identity/` skeleton: `keys.rs` (ed25519/X25519 gen + custody boundary over
  `wg secret`), `sigchain.rs` (`genesis` + `verify`), `envelope.rs`
  (`IdentityRecord`/`StateSnapshot`/`SignedEvent` sign/verify/canonical-encode),
  `mod.rs` (`WG_FED_COMPAT_VERSION`).
- `wg identity new`; publish/fetch to a dumb third location; the minimal cross-graph
  signed `SignedEvent` send/poll (one transport).
- `Message` gains `from`/`to`/`sig`/`refs` (`#[serde(default)]` — old readers ignore
  them, doc 02 §2.3, backward compatible).
- **Deliverable:** the `federation_spark_two_graphs.sh` smoke scenario passes (§4.4).
- **Dependencies:** Wave 2 ADRs Accepted. New crates: `ed25519-dalek`,
  `x25519-dalek`, `blake3`, `chacha20poly1305`, `hkdf` (doc 04 §1.5).

### Wave 4 — Cross-graph addressing + transport hardening (Phase 1)

- `federation.yaml` `PeerConfig`/`Remote` gain `wgid` + endpoints; `resolve_peer`
  becomes the resolution **cascade** (cache → optional directory → DHT).
- The **WG node HTTP store-and-forward inbox** becomes the default transport
  (promote the existing daemon — doc 02 §2.1, doc 05 WG-fit 5); `wg msg --to wgid:`
  works between graphs.
- **Freshness attestations** (S-3) so revocation/rollback is detectable on the async
  path; high-value actions fail-closed on stale.
- **Deliverable:** signed cross-WG messaging at email-speed over a real network, with
  the path-based `federation.yaml` peers still working alongside key-based ones.

### Wave 5 — Portable state + recovery (Phase 2)

- Sigchain `add_key` / `revoke_key` / `rotate_root`; `transfer()` learns the
  signed-artifact path (doc 04 §2.9/§3.9).
- **Recovery:** offline recovery key + time-boxed override window (node default);
  the **mandatory paper-key + M-of-N guardian ceremony** for node-less mode (defuses
  Fatal A-4); agent recovery anchored to the custodian.
- **Hydra mitigation (S-4):** `add_key`/root-rotate restricted to root/M-of-N;
  delegated signers cannot grow the authorized set.
- **State-poisoning safety layer (S-5):** treat loaded `StateSnapshot`s as untrusted
  — provenance-gate by `trust_level`, enforce `model_binding`, human-in-loop for
  cross-trust loads.
- **Deliverable:** portable identity (V2) + demonstrable key recovery (V6) +
  fork-vs-same-self enforced.

### Wave 6 — Encryption=ACL + UCAN delegation + optional central pieces (Phase 3)

- Per-recipient sealed envelopes realize `AccessPolicy` (the `federation.rs:627`
  hook); MLS for online/long-lived groups; sealed-sender option.
- **UCAN** issue/verify/revoke in `custody.rs` (harvested from D) — short-lived,
  scoped, **attenuating-only** delegations; this is where agent custody becomes
  *structural*.
- The *optional* directory and node-side conveniences crystallize C fully; an
  optional **verifiable did:web/handle alias** may land here (**as an alias, never a
  root**).
- **Deliverable:** confidentiality (R24), structural delegation, discovery — `WG-Fed`
  is complete; the system is C, reached by convergence.

### Don't-build-yet guardrails (explicit)

These are **out of scope until their gating wave**, and three are *never* to be built
in the rejected form:

- **Never** build did:web/DNS as an *identity root* (doc 05 D-1/D-2, Fatal-as-root) —
  did:web is at most a Wave-6 *alias* over a self-certifying root.
- **Never** ship the node-less mode *without* the mandatory recovery ceremony (doc 05
  A-4, Fatal) or *without* `add_key` locked to root (doc 05 S-4, the hydra).
- **Never** ship C's optionality without **fail-safe defaults + a strict mode +
  resolution-cascade lint** (doc 05 C-1).
- **Don't** pick the final wire library (Iroh vs relays vs node-HTTP-only) before
  Phase 2 — the wire is candidate-agnostic through Wave 4 (doc 04 §9); let the spark
  and cross-graph waves inform it.
- **Don't** build forward-secret ratcheting on the offline path (doc 05 S-6 — it does
  not compose with send-to-offline; static-key + rotation only). MLS is online/groups
  only, Wave 6.
- **Don't** build a global registry / naming authority, mixnet/Tor-grade anonymity,
  opaque hidden-state *serialization*, real-time transport, or a social-media product
  surface — all non-goals (§7).

---

## 6. ADR stubs (Wave-2 deliverables)

Four stubs. Each follows the project's lightweight ADR shape (Context · Decision ·
Status · Consequences · Alternatives rejected · Open questions). They are *stubs* —
the Wave-2 task fleshes each into an accepted ADR under `docs/` (matching the
existing `docs/ADR-*.md` convention).

### ADR-001 — Identity & key model (self-certifying `wgid` + sigchain + 3-tier keys)

- **Status:** Proposed (decided in this memo; to be ratified Wave 2).
- **Context.** WG has *zero* signing crypto today; identity is a content hash, not a
  keypair (doc 02 §2.2/§2.4). The vision needs `pubkey == identity == address`
  (V1/V4) with long-term continuity through key loss (V6). Doc 01 §4.2's decisive
  lesson: continuity requires an indirection layer (identity = stable name; keys =
  revocable contents).
- **Decision.** Identity = `wgid:<multibase-ed25519-pubkey>`, self-certifying, with a
  **sigchain** (append-only, hash-linked, signed) mapping `identity → {current key
  set}`. A **three-tier key hierarchy** (root/signer/encryption). The address is the
  **genesis root pubkey**, stable under rotation. **Verification is never central**
  (HQ6). did:web/DNS is rejected as a root; aliases are optional and verifiable
  (HQ5).
- **Consequences.** New `src/identity/{mod,keys,sigchain,did,envelope}.rs` + crypto
  crates; `Agent` gains `pubkey`/`sigchain_head`; `WG_FED_COMPAT_VERSION` handshake.
  Enables FR-I1–I7. Requires the freshness-attestation mechanism (S-3) so revocation
  is live on the async path.
- **Alternatives rejected.** Key-as-identity with no indirection (SSB/Nostr-as-shipped
  — *cannot* offer V6, doc 01 §4.2). did:web as root (doc 05 D-1/D-2, Fatal). A
  content-hash "identity" (today's model — unsigned, anyone can recompute, doc 02
  §2.2).
- **Open questions.** Exact multibase/multicodec encoding; `did:key` interop surface;
  whether genesis embeds guardians by default; freshness-attestation Δ and clock-skew
  handling.

### ADR-002 — Transport (node-default store-and-forward, untrusted, fallback ladder)

- **Status:** Proposed.
- **Context.** Email-speed (NFR-2) + offline tolerance + decentralization-leaning
  (V5) constrain transport; pure P2P is operationally heavy (NAT, always-on), pure
  relays re-introduce semi-central nodes (doc 03 HQ3). WG's federation is *already* a
  daemon brokering over a socket (doc 02 §2.1) — the shortest path to a node inbox.
- **Decision.** A **fallback ladder**: WG node HTTP store-and-forward inbox by
  **default** → Iroh QUIC direct when both online → optional shared relays. **No
  single mandatory relay** (FR-F4/F5). Bytes are signed and optionally sealed
  end-to-end; **the transport, including your own node, is untrusted**. The
  compat/transport handshake is **authenticated** (S-7).
- **Consequences.** Promote the daemon to an HTTP node (reuse doc 02 §2.1d IPC);
  `messages.rs` gains cross-graph send/poll + adapters (node/Iroh/relay);
  `federation.rs` peers gain endpoints + a resolution cascade. Email-speed,
  offline-tolerant, NAT-free in the default path.
- **Alternatives rejected.** Pure-P2P-only (A — heavy, no always-on availability). A
  single mandatory relay/node (violates FR-F4/F5). Real-time/RTC transport (non-goal).
- **Open questions.** Iroh vs a thinner relay for the P2P leg (defer past Wave 4 —
  guardrail); pull-vs-push (poll vs subscription); state-blob storage duration and
  who pays (NFR-5 self-host vs shared).

### ADR-003 — Custody, delegation & recovery (custodian-of-record + UCAN + layered recovery)

- **Status:** Proposed. **This is the load-bearing ADR — it owns the crux (HQ1) and
  the two Fatal findings.**
- **Context.** "Download Nora ≠ impersonate Nora" (HQ1, FR-I2/S1) is the study's
  whole point. Doc 05 §3.2: the split is *structural* only with a custodian-of-record
  (D ≈ B > C > A). Agents have no fingers/phones; their keys must live with a
  custodian (HQ9). Two Fatal findings live here: A-4 (recovery) and the hydra (S-4);
  plus the freeze attack (S-3).
- **Decision.** The **portable identity excludes the root private key**, always.
  Agent root is **custodian-held** (node/owner) behind an ssh-agent-style signing
  boundary; the worker never holds root bytes and download confers **no oracle
  access** (S-2). Agents act under **UCAN** short-lived, scoped, **attenuating-only**
  capabilities (harvested from D); `add_key`/root-rotate is **root/M-of-N only**
  (kills the hydra, S-4). **"Download onto host B" = fork by default**; same-self
  needs an explicit signed `add_key`/`delegate` or a node migration op (FR-I5).
  **Recovery is layered:** offline recovery key + override window (node) / mandatory
  paper-key + M-of-N ceremony (node-less); agent recovery anchors to the custodian.
- **Consequences.** `src/secret.rs` becomes a typed **signing custodian** with a
  "sign this digest" call; `custody.rs` issues/verifies/revokes UCANs; the node
  becomes the agent custodian-of-record in the default deployment. Resolves HQ1, HQ2,
  HQ11; bounds B-1 (node compromise can't forge what a verifier self-checks).
- **Alternatives rejected.** Standing signer on the worker + any-key-`add_key` (A —
  weakest split, the hydra by design, doc 05 §3.2/§4.1). Shared-key delegation
  (collapses HQ1). Social-M-of-N as the *only* recovery (Fatal A-4). Domain-control
  recovery (Fatal D-2). Blanket/non-expiring delegation (T9).
- **Open questions.** UCAN expiry defaults vs offline chattiness (D-3); revocation-list
  hosting (which is itself a lookup dependency); whether the node-custodian or the
  human owner is the *default* agent custodian; M-of-N guardian UX.

### ADR-004 — Loadable-state format & AI-input safety

- **Status:** Proposed.
- **Context.** State must serve today's conversation cache and tomorrow's opaque RNN
  blob through one interface (V1, R2, HQ10). Two adversarial findings are
  unavoidable: opaque blobs can smuggle keys (S-1) and *any* loaded state is a
  prompt-injection/persistence vector even when validly signed (S-5, the AI-specific
  threat with no prior-art precedent).
- **Decision.** One stable `StateSnapshot` envelope with a tagged, evolvable
  `payload_kind`; signed + BLAKE3-CAS + incremental (`prev`); `model_binding`
  wrong-model guard; unknown kinds degrade gracefully. **FR-S1 becomes a
  runtime-containment guarantee for opaque kinds** (the custody boundary holds the
  only key copy, so nothing is in-memory to bake in — S-1). **Loaded state is
  untrusted input:** sandbox/scan, provenance-gate by `trust_level`, human-in-loop
  for cross-trust loads (S-5). We design the **slot, not the opaque payload**.
- **Consequences.** A new AI-input-safety layer WG lacks today (S-5); the
  "just publish the blob and anyone loads it" simplicity is lost for opaque kinds.
  Enables FR-I3/I4/I7.
- **Alternatives rejected.** Hard-coding the conversation-log shape (rewrite later).
  Treating a valid signature as a safety guarantee (S-5: signature ≠ safety). Solving
  opaque hidden-state *serialization* now (non-goal §7 — design the slot only).
- **Open questions.** What the AI-input-safety scan actually checks; the
  `trust_level` threshold for auto-load vs human-in-loop; whether opaque payloads are
  *always* sealed.

---

## 7. Non-goals for v1 (explicit)

`WG-Fed` v1 carries forward doc 03 §5's non-goals and adds the decision-specific
exclusions. **Out of scope:**

1. **Real-time / low-latency transport.** Email-speed is a deliberate relaxation
   (NFR-2); chat/RTC latency is a non-goal.
2. **Blockchain / token / global consensus ledger.** Sigchains are per-identity, not
   a global chain.
3. **Rolling our own cryptography** (FR-S5) — compose audited primitives only.
4. **A global naming authority / DNS replacement / registry.** Aliases are optional,
   verifiable convenience (FR-F2).
5. **Strong anonymity / mixnet-grade metadata hiding.** We *bound and disclose* the
   metadata leak (FR-S4, HQ4); no sender-recipient unlinkability in v1.
6. **A social-media product surface** (feeds, ranking, notification UX) — this is the
   substrate, not the product.
7. **Solving opaque hidden-state portability now.** We design the `payload_kind`
   *slot*; serializing/reloading a model's hidden state is out of scope.
8. **Re-implementing the agency-primitive transfer.** Today's `src/federation.rs`
   path-based transfer is the *migration substrate* (FR-F6), not the redesign target.
9. **did:web / DNS as an identity root** — *rejected* (doc 05 D-1/D-2); did:web is at
   most a later, optional, verifiable *alias*.
10. **Forward secrecy on the offline store-and-forward path** — does not compose with
    send-to-offline (S-6); static-key + rotation there, MLS for online/long-lived
    groups only.
11. **Fast UCAN revocation-at-scale as a primary mechanism** — an open problem (doc 05
    D-3); v1 relies on short expiries + issuer-subtree revocation, not instant global
    revocation.
12. **Multi-tenant billing / quota / payments** for relay/node operators — NFR-5
    covers self-hostability; commercial economics are out of scope.

---

## 8. Open questions handed to the ADR wave

These are *not* re-openings of the decision — they are the implementation forks the
ADRs (§6) must close, surfaced here so Wave 2 has a checklist:

1. **Default agent custodian** — node-operator vs human-owner when both exist
   (ADR-003).
2. **Freshness-attestation Δ** and clock-skew handling for the freeze defense (S-3,
   ADR-001).
3. **UCAN expiry defaults** balancing blast-radius vs offline re-issuance chattiness
   (D-3, ADR-003).
4. **P2P leg library** — Iroh vs a thinner relay — deferred past Wave 4 by guardrail
   (ADR-002).
5. **AI-input-safety scan** contents and the `trust_level` auto-load threshold (S-5,
   ADR-004).
6. **Guardian/M-of-N UX** for node-less recovery (ADR-003).
7. **`wgid` encoding** (multibase/multicodec) and `did:key` interop (ADR-001).

---

## 9. Validation checklist (this document)

- [x] **One recommended architecture chosen and defended vs the alternatives, citing
      doc 05.** `WG-Fed` = Candidate C (key-rooted hybrid), B-shaped default node,
      D's UCAN grafted, A preserved as the node-less option (§1, §2); defended against
      A/B/C/D via doc 05's ranking (B > C > A > D), the custody verdict (D ≈ B > C >
      A), the Fatal findings (A-4, D-1/D-2), and the §7.1 phased synthesis (§2.2).
- [x] **Every doc-03 hard question has an explicit decision; the
      decentralization-vs-central-node call is made plainly.** HQ1–HQ12 each decided
      in §3 (with the summary table); HQ6 stated plainly in §1 and §3 ("decentralized
      at the trust layer, central by default at the availability layer; central = a
      hint that can't override a self-verification").
- [x] **A concrete, runnable spark-test PoC milestone is defined.** §4: seven
      falsifiable steps across two filesystem-independent graphs + a third location,
      including the downloaded-identity ≠ impersonation assertion (doc 05 §3), landing
      as the `federation_spark_two_graphs.sh` smoke gate.
- [x] **Phased roadmap + ADR stubs + explicit non-goals.** Waves 2–6 with
      dependencies and don't-build-yet guardrails (§5); four ADR stubs (§6); twelve
      non-goals (§7).
- [x] **`docs/federation-study/06-decision-memo-and-roadmap.md` written.**

---

*Wave-1 decide phase complete. The federation study (docs 01–05) is synthesized into
a single decision: build `WG-Fed` — a self-certifying, key-rooted hybrid whose trust
root is never central and whose availability/recovery/discovery rest by default on an
optional WG node, with UCAN-style attenuating delegation for agent custody and a
node-less P2P mode preserved as the decentralization option. Every doc-03 hard
question is decided, the decentralization-vs-central call is made plainly, the choice
is defended against the alternatives using the adversarial pass, the spark test that
validates it is defined concretely as the first implementation milestone, and the
phased roadmap, ADR stubs, and non-goals lay out what to build next — and what not to
build yet.*
