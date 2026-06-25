# ADR-001 (WG-Fed): Identity & Key Model — `wgid` + Sigchain + 3-Tier Keys

**Status:** Proposed
**Date:** 2026-06-25
**Decision:** Identity is a self-certifying `wgid:<multibase-ed25519-pubkey>` backed by an append-only, hash-linked, signed **sigchain** that maps `identity → {current key set}`, over a **three-tier key hierarchy** (root / signer / encryption). The address is the **genesis root pubkey, stable under rotation**. **Verification is never central.** `did:web`/DNS is rejected as a root; aliases are optional and verifiable.

> **This is the foundational WG-Fed ADR.** ADR-002 (transport), ADR-003 (custody,
> delegation & recovery — the crux), and ADR-004 (loadable-state format & AI-input
> safety) all cite the identity, addressing, key hierarchy, and sigchain primitives
> fixed here. The decision was *made* in the federation-study decision memo
> (`docs/federation-study/06-decision-memo-and-roadmap.md` §1, §2, §3 HQ1/HQ5/HQ6/HQ9/HQ12,
> §6 ADR-001 stub); this ADR formalizes it and resolves the stub's four open
> questions. It is **not** a re-litigation of the architecture choice (`WG-Fed` =
> Candidate C with a B-shaped default, D's UCAN grafted, A preserved as the
> node-less option) — that is settled.

---

## Context

WG today has **zero signing cryptography**. "Identity" is a content hash:
AI agents are `SHA-256(role_id + motivation_id)` and human agents are
`SHA-256(name + executor)` (see `docs/ADR-actor-vs-agent-identity.md`, §"ID
generation"). A content hash is unsigned — anyone who knows the inputs can
recompute it, and nothing about it proves *authorship* of an artifact. The only
`private_key` symbol in the tree is an unrelated VAPID push key
(`docs/federation-study/02-current-state-baseline.md` §2.4). Cross-graph
"federation" today is a daemon brokering a **shared-filesystem** path-based
transfer (`src/federation.rs`); it cannot cross a host boundary or authenticate a
peer by key.

The WG social-network vision (gap-analysis pillars V1–V7) needs the opposite:
**`pubkey == identity == address`** (V1/V4), self-certifying with no central
registry, and **long-term continuity through key loss** (V6) — a key can be lost,
rotated, or compromised without the identity dying.

The single most important lesson of the prior-art survey
(`docs/federation-study/01-prior-art-landscape.md` §4.2) is decisive and
non-negotiable: **every system that survives key loss does so via an indirection
layer where the identity is a stable name and keys are revocable contents of a
signed, append-only record** — Keybase's *sigchain*, atproto/DID's *rotation
keys*, Farcaster's *on-chain key registry*. Every system without that indirection
— SSB, Nostr-as-shipped, raw libp2p/Iroh `NodeId` — **cannot** offer continuity:
the key *is* the identity, so losing the key kills the identity. WG must not
repeat the SSB trap.

This ADR fixes the identity, addressing, key hierarchy, and sigchain that the rest
of WG-Fed builds on. It is the first Wave-2 deliverable; **no federation code lands
until ADR-001/002/003 are Accepted** (memo §5, Wave 2).

---

## Decision

### D1 — Identity is a self-certifying `wgid:`

An identity is named by, and verifiable against, a **public key** (FR-I1). The
canonical address is:

```
wgid:<multibase-ed25519-pubkey>
```

where the encoded body is the **genesis root ed25519 public key**. Given only the
address and an artifact, verification is a **local signature check** rooted at the
genesis pubkey embedded in the address — **no network call to a trusted third
party is ever required** (FR-I1, HQ5). This is the Nostr-`npub` / `did:key` family
of self-certifying identifiers, chosen so that the secure + decentralized corner of
Zooko's triangle is the *root* and human-meaningfulness is layered on as opt-in
verifiable convenience (HQ5).

The exact encoding and `did:key` interop are resolved in **Open Questions 1 and 2**
below.

### D2 — A sigchain maps `identity → {current key set}`

Behind the stable `wgid:` address sits a per-identity **append-only, hash-linked,
signed log** — the sigchain (the Keybase model; doc 04 §1.2). Each link is one of:

- `genesis` — declare the root key (this link's root pubkey *is* the `wgid:` body)
- `add_key` — authorize a signer/device/encryption key with a scope
- `revoke_key` — revoke a previously-authorized key
- `rotate_root` — succession: the old root signs the next root
- `delegate` — issue a capability (the UCAN-style delegation owned by ADR-003)
- `set_alias_proof` — bind a verifiable alias (see D5)
- `set_endpoints` — publish fetchable inbox/state/relay endpoints

A link is valid **iff it is signed by a key the chain authorized at that link's
position**; old links stay valid at their historical position even after the key
that signed them is later revoked (Keybase semantics, doc 01 §2.2). This single
structure discharges **rotation/recovery** (HQ2), **revocation** (`revoke_key`),
**auditability** (replay the chain; NFR-7), **delegation** (HQ11, ADR-003), and the
**fork-vs-same-self continuity** decision (HQ1/FR-I5): "same self" is a link that
extends *this* chain from a surviving authorized key; "fork" is a *new* genesis that
cites this chain as its parent. The sigchain is the mandatory indirection layer doc
01 §4.2 proved is the only path to V6.

The sigchain is **content-addressed** (each link a BLAKE3 CID; `sigchain_head` is the
latest link's CID) and **published anywhere** — the bytes are self-verifying, so
*where* they are hosted is a convenience, not a trust dependency (HQ6).

### D3 — Three-tier key hierarchy

| Tier | Role | Algorithm | Where it lives (default) | Powers |
|---|---|---|---|---|
| **Root / identity** | *Is* the identity; signs the sigchain; authorizes/revokes everything below | ed25519 | **Human:** device/OS keychain, hardware-backed (FIDO2/passkey) where available. **Agent:** custodian-held in `wg secret` (or HSM) behind an ssh-agent-style "sign this digest" boundary — never on the ephemeral worker host. | Sign sigchain links only. Rarely used online. |
| **Signer / device / agent** | The working key that signs day-to-day events and state | ed25519 | On the acting host (worker, device). Recorded as an authorized method in the sigchain. | Sign `SignedEvent`/`StateSnapshot` **within delegated scope**; **cannot** alter the sigchain. |
| **Encryption** | Per-recipient confidentiality (the ACL realization, FR-S3) | X25519 (static) + per-message ephemeral | Alongside the signer key | Decrypt envelopes sealed to it; derive per-message keys. |

Collapsing these into one key *is* the SSB trap (doc 01 §2.5): one immutable key as
identity + signer + encryption means losing it once kills the identity and leaking
it once steals it, with no revocation. Splitting root from signer is precisely what
makes rotation (HQ2), agent custody (HQ1), and delegation (HQ11) expressible
**without ever moving the root private key**. The full custody mechanics — how the
agent root is held, how a worker requests a signature, how UCAN signers are issued —
are owned by **ADR-003**; this ADR fixes only that the hierarchy exists and what each
tier may do.

### D4 — The address is the genesis root pubkey, stable under rotation

The `wgid:` address is the **genesis** root pubkey and **never changes**, even when
the active key set rotates completely underneath it (FR-I7). `rotate_root`
succession (old root signs new root) lets the *signing* root change while the
*address* stays the genesis pubkey — the identity survives unbounded key churn
without its name changing. This resolves tension T7 (stable address vs rotatable
key): the address is the durable anchor; the sigchain rotates the contents.

A consequence worth stating plainly: the genesis pubkey is a permanent label, so
losing the genesis *private* key does not lose the *address* (others can still name
and verify the historical identity), but it does end the ability to extend the chain
unless recovery material was provisioned at genesis — which is exactly why recovery
(ADR-003) and the guardian/freshness questions below are load-bearing.

### D5 — Verification is never central; aliases are optional and verifiable

Per HQ6, **identity verification is correctness-critical and is never central**: "is
this artifact really from Nora?" is always a local signature check against the
sigchain rooted at Nora's `wgid:` genesis pubkey. No node, directory, relay, DNS
name, or CA is ever in the correctness/security path. Every other capability
(resolution, discovery, alias lookup, state hosting, recovery anchoring) **may** be
central and by default *is* (the WG node), under the binding invariant that **a
central component is a hint that can only help, never override a self-verification**
(fail-safe, never fail-open).

Resolution of an address to fetchable endpoints is a **cascade**, any one step of
which suffices: cached signed endpoint record → optional directory hint → DHT/Iroh
discovery (FR-F1/F4). A forged directory cannot override the local sigchain check.

Human-friendly names layer on as **opt-in, verifiable** aliases (FR-F2):

- **Petnames** — local, per-user, no infrastructure.
- **Verified handles** — e.g. `@nora.garrison.family` via DNS/`.well-known` or a
  Keybase-style social proof, bound by a `set_alias_proof` sigchain link and
  **checkable back to the key**. Never a mandatory central naming authority; loss or
  abuse of an alias **never** compromises the underlying key identity.

**`did:web`/DNS is explicitly rejected as a root** (see Alternatives Rejected) — it
survives at most as *one verifiable alias among others*, never the anchor.

### D6 — One identity type for humans and agents

WG-Fed keeps WG's already-unified `Agent` (see `docs/ADR-actor-vs-agent-identity.md`):
**one identity type with capability flags**, not two types (HQ9, FR-I6). Humans and
agents share the `wgid:` + sigchain + 3-tier model identically; the differences are
**by design** and live in custody/recovery/authority, not in the identity type:

| | Human | Agent |
|---|---|---|
| Root custody | Self-held (device/passkey, hardware-backed) | Custodian-held (node/owner) via ssh-agent-style signing boundary |
| Day-to-day signing | Device signer | Delegated UCAN signer (scope/expiry per ADR-003's authority dial) |
| Recovery | Device set + offline recovery key + social M-of-N | Falls back to the custodian (never independently recoverable) |

No human-only assumption (biometric, phone) sits on a path an agent must traverse.
The `IdentityRecord` carries the operational `Agent` fields (role, motivation,
capabilities, trust level, executor) so a pulled federated identity is dispatchable
without a schema mismatch (FR-I6). The custody/recovery split itself is ADR-003 —
including the **authority-scope default**, which ADR-003 §D2's trust-default amendment
sets to **broad / long-lived by birth** (the short-lived "leash" is environment-driven
policy, not the agent's default; humans self-hold their root and are never leashed).

### D7 — Crypto agility and a fail-loud compat handshake

Every signed structure carries an explicit `alg` id (doc 04 §1.3), so a primitive
(e.g. ed25519 → ML-DSA post-quantum) can be retired by adding a method to the
sigchain and dual-signing during a migration window — **no identity is ever
abandoned** (HQ12, V6). A `WG_FED_COMPAT_VERSION` constant in `src/identity/mod.rs`
mirrors `WG_AGENCY_COMPAT_VERSION` and `WG_PI_PLUGIN_COMPAT_VERSION` and **fails loud
on incompatible mismatch**. Per the adversarial finding S-7, the version/parameter
handshake is **authenticated** (the negotiated parameters are *signed*, not merely
exchanged) with a **minimum-`alg` floor** and aggressive retirement, so a downgrade
attack cannot strip strong crypto or force a "lowest-common-`alg`."

---

## Status

**Proposed.** This ADR records the decision exactly as fixed in the federation-study
decision memo and resolves the four open questions the ADR-001 stub left open. **Erik
ratifies it to Accepted** — that human gate is deliberately not set here. No
federation code lands until ADR-001/002/003 are Accepted (memo §5).

---

## Consequences

- **New `src/identity/` module** is the home for all cryptography (the tree has none
  today): `mod.rs` (`WG_FED_COMPAT_VERSION`, public gen/load/verify/sign API),
  `keys.rs` (ed25519/X25519 generation + the custody boundary over `wg secret`),
  `sigchain.rs` (`genesis`/`add_key`/`revoke_key`/`rotate_root`/`delegate` +
  `verify()`), `did.rs` (`wgid:`/`did:key` resolution → `IdentityRecord`),
  `envelope.rs` (`IdentityRecord`/`StateSnapshot`/`SignedEvent` sign/verify/canonical
  encode). `custody.rs` and `acl.rs` arrive with ADR-003/ADR-004's waves.
- **New crates:** `ed25519-dalek`, `x25519-dalek`, `blake3`, `chacha20poly1305`,
  `hkdf` (today the tree carries only `sha2`/`rustls`/`keyring`).
- **`Agent` gains `pubkey: Option<String>` and `sigchain_head: Option<String>`**
  (`src/agency/types.rs`); `contact` generalizes toward routed `endpoints`. Fields are
  `#[serde(default)]` so existing graphs parse unchanged.
- **`src/secret.rs` becomes a typed signing custodian** with an "sign this digest"
  call (the ssh-agent-style boundary) — the detailed design is ADR-003's, but this
  ADR fixes that the root private key lives there and never reaches a worker.
- **The address is permanent.** Tooling, logs, and stored references may treat a
  `wgid:` as a stable primary key; rotation never invalidates a stored address.
- **Enables FR-I1–I7** (self-certifying, portable, model-agnostic loadable state,
  content-addressed, continuity semantics, operational fields, stable-address-vs-state
  separation) and FR-F1/F2 (key-as-locator, verifiable optional aliases).
- **A freshness-attestation mechanism is required** (Open Question 4) so that
  revocation is *live* on the async transport path; without it an eclipse/freeze can
  keep a revoked key looking alive (finding S-3). High-value actions fail closed on
  stale freshness.
- **Cost we accept:** raw `wgid:` keys are not human-memorable (the Zooko price); the
  alias layer is opt-in convenience, and first-contact key→human binding remains a
  social problem (TOFU + proofs + OOB fingerprint compare, HQ8). The sigchain is a new,
  mandatory data structure that every WG-Fed verifier must implement and replay.

---

## Alternatives rejected

- **Key-as-identity with no indirection layer** (SSB; Nostr-as-shipped; raw
  libp2p/Iroh `NodeId`). The key *is* the identity, so key loss = identity death and a
  compromised key cannot be revoked. This **cannot** offer V6 continuity — doc 01 §4.2
  proves every continuity-capable system has the indirection layer and every system
  without it fails. Rejected: the sigchain (D2) exists precisely to avoid this trap.
- **A content-hash "identity"** (WG's model today — `SHA-256(role+motivation)` /
  `SHA-256(name+executor)`). Unsigned: anyone who knows the inputs recomputes it, and
  it proves no authorship. It cannot self-certify an artifact and has no notion of key
  rotation. Rejected as a federation root; it remains a *local* convenience id, not a
  federated identity.
- **`did:web` / DNS as the identity root** (Candidate D's anchor; doc 05 D-1/D-2,
  **Fatal-as-root**). A forged `did.json` is a complete takeover by anyone who can
  compel a CA, hijack DNS, or seize a domain — the *easiest impersonation surface of
  the four candidates* and the **exact attack this whole study exists to prevent**
  (contradicts V4/FR-I1). Its only escape (a `did:key` fallback) "dissolves D into C
  with a directory." Rejected as a root; `did:web` survives at most as one verifiable
  *alias* (D5), never the anchor (memo §2.2, non-goal §7.9).
- **A mandatory central registry / global naming authority** for addresses. Re-centralizes
  the trust root, excludes newcomers, and reintroduces a single point WG-Fed exists to
  remove (HQ5, non-goal §7.4). Rejected: resolution is a fail-safe cascade and aliases
  are optional + verifiable.
- **A single-key (one-tier) model.** Collapsing root + signer + encryption is the SSB
  trap restated at the key layer (D3): no rotation, no custody split, no delegation,
  catastrophic blast radius on leak. Rejected for the three-tier hierarchy.
- **Making identity verification depend on a directory/node/DNS** (A's only real
  opening per doc 05 §3.1; D's whole anchor). Rejected: verification is
  correctness-critical and never central (D5, HQ6) — a compromised node still cannot
  forge what a verifier self-checks.

---

## Open questions

The ADR-001 stub (memo §6) and the memo's handed-off checklist (§8 items 2 and 7)
left four questions for this ADR to close. All four are resolved with rationale below;
where a residue is genuinely a tuning/UX value judgment it is **explicitly flagged for
Erik** rather than silently fixed.

### OQ1 — Exact multibase/multicodec encoding for `wgid:` — **RESOLVED**

**Resolution.** Encode the body **identically to `did:key`'s ed25519 form**, so the
two are a pure prefix swap:

1. Take the 32-byte raw ed25519 public key.
2. Prepend the **multicodec `ed25519-pub` prefix** `0xed`, varint-encoded as the two
   bytes `0xed 0x01`.
3. **Multibase-encode** the result with **base58btc** (multibase prefix `z`) — the
   same base `did:key` uses.

Result: `wgid:z6Mk…` where everything after `wgid:` is byte-for-byte what
`did:key:` carries. The genesis link records the raw 32-byte key plus its `alg`; the
`wgid:` string is the canonical render.

*Why.* (a) **Maximal interop** — `wgid:<body>` ⇄ `did:key:<body>` is a trivial prefix
swap, which makes OQ2 nearly free and lets W3C DID tooling consume our root pubkey
unchanged. (b) **Crypto agility for free** — the multicodec prefix self-describes the
key type, so an `alg` migration (ed25519 → ML-DSA, HQ12) is a new multicodec prefix,
not a new address scheme. (c) **Consistency with prior art** the study mandates for
interop (HQ5 success criterion; doc 01 §5: the Nostr/`did:key` family).

*Liberal acceptance (Postel).* The **canonical emitted form is base58btc (`z`)**, but
parsers MUST also accept a **base32 (`b`, lowercase, case-insensitive)** rendering of
the same multicodec+key bytes for DNS-safe / case-insensitive contexts (e.g.
embedding a `wgid` in a hostname-shaped alias). Both decode to the identical
32-byte key, so they name the same identity. We do **not** accept Nostr `npub`
(bech32) as a `wgid` body — it is secp256k1-flavored and a different multibase; an
`npub` is at most an *alias proof*, never a `wgid` spelling.

*Not Erik's call* — this is a mechanical interop/encoding decision the memo already
pointed at ("the Nostr-npub / `did:key` family," HQ5). It is settled here.

### OQ2 — `did:key` interop surface (do we emit/accept `did:key`?) — **RESOLVED**

**Resolution.** **Accept liberally, emit on request, never treat `did:key` as a
substitute for the full identity.** Concretely:

- **Accept:** a `did:key:z6Mk…` on input is parsed to the same root pubkey as the
  equivalent `wgid:` (OQ1 makes the bodies identical). `did:key` is recognized as a
  valid *spelling of the genesis anchor*.
- **Emit:** the canonical form WG-Fed publishes and stores is `wgid:`. `did.rs`
  additionally offers a `did:key` rendering (and a minimal DID-document projection)
  on request, for interop with external DID verifiers.
- **The hard boundary:** `did:key` is **stateless** — it carries *no sigchain*, hence
  no rotation, revocation, delegation, alias, or endpoint information. It can
  therefore represent **only the genesis root pubkey**, i.e. the *anchor*, never the
  evolving identity. Accepting a `did:key` gives a verifier the root pubkey to *start*
  verification; the verifier must still fetch and replay the `wgid:` sigchain to learn
  the **current** authorized key set. Formally: `did:key` ≡ the WG-Fed genesis anchor,
  but `wgid:` ⊋ `did:key` because only `wgid:` carries continuity. A `did:key` MUST NOT
  be used to authorize an action against the *current* key set without resolving the
  sigchain first.

*Why.* Interop is almost free given OQ1, and it lets WG-Fed identities be consumed by
the broad `did:key` tooling ecosystem (doc 01 §2.3). Refusing to *emit* would forfeit
that for no benefit; treating `did:key` as the *whole* identity would silently discard
the rotation/revocation continuity that is the entire point of D2 — which is why the
boundary above is explicit.

*Not Erik's call* — follows directly from OQ1 and the memo's interop intent. Settled.

### OQ3 — Does genesis embed M-of-N guardians by default? — **RESOLVED (slot here; policy/tuning flagged)**

**Resolution — split by layer.** This ADR owns the *genesis link format*; the recovery
*policy* is ADR-003's. So:

- **ADR-001 (here):** the `genesis` link carries an **optional `recovery` field** — a
  slot for guardian commitments and an M-of-N policy (hashes/pubkeys of guardians +
  the threshold). The slot always exists; whether it is *populated* is mode-dependent.
- **Whether it is populated by default is decided by deployment mode** (the memo
  already decided this in HQ2 and the §5 guardrails):
  - **Node-less human mode → MANDATORY.** Genesis MUST embed both a paper key **and**
    M-of-N social-recovery guardians. This is non-negotiable: it is the ceremony that
    defuses the Fatal finding A-4 (a node-less identity with no recovery path is
    unrecoverable), and the §5 guardrail is explicit — *"Never ship the node-less mode
    without the mandatory recovery ceremony."* Genesis tooling refuses to mint a
    node-less human identity without it.
  - **Node-present (default) human mode → OPTIONAL, off by default.** The primary
    recovery anchor is the node-held rotation key + a human-held *offline* recovery key
    with a time-boxed override window (atproto's model). Guardians MAY be added as
    defense-in-depth but are not required at genesis, because the offline recovery key
    + custodian already provide recovery (doc 05 Recoverability 5 for the node model).
  - **Agents → N/A.** Agent recovery always collapses to "the custodian's key is safe"
    (the node/owner is the recovery anchor by design; HQ9). Agents have no social graph,
    so guardians are not an agent concept; the agent `genesis` `recovery` slot stays
    empty and recovery is anchored to the custodian.

*Why.* Embedding the *slot* at genesis is necessary because guardian commitments must
be cryptographically bound to the identity from its first link to be trustworthy — you
cannot safely add a recovery quorum *after* a key is already at risk. Making the slot
*mandatory only node-less* matches exactly where the Fatal A-4 bind lives (no
custodian-of-record to fall back on) and avoids forcing guardian-ceremony friction on
the node-default majority who already have a stronger recovery anchor.

*Flagged for Erik / delegated to ADR-003:* the **default M and N values** (e.g. 2-of-3
vs 3-of-5), whether the guardian set is **mutable post-genesis** and under what
authority, and the **guardian-enrollment UX** are policy/UX value judgments, not
format decisions. They belong to ADR-003 (which owns recovery) and are the kind of
default Erik should bless. This ADR commits only to the *slot* and the *mode-dependent
mandatory/optional/absent rule*.

### OQ4 — Freshness-attestation Δ + clock-skew handling (the S-3 freeze defense) — **RESOLVED (mechanism + default Δ; exact calibration tunable)**

**The threat (S-3).** On the async store-and-forward path, an attacker who controls a
verifier's view of the directory/relay can **withhold** a `revoke_key` link — an
eclipse/freeze — so a revoked key keeps looking alive. A signature check alone cannot
detect this, because the *signature* is still valid; what is stale is the verifier's
knowledge of *current authorization status*.

**Resolution — mechanism.**

1. **Freshness only gates "is this key authorized *now*" decisions, not historical
   verification.** Verifying an already-signed, immutable, BLAKE3-addressed artifact
   against the chain state *at its link's position* is valid forever and needs no
   freshness (a revoke that happened later does not retroactively invalidate a link
   that was valid when signed — Keybase semantics, D2). Freshness is required only
   when accepting a **new** action/delegation or a **high-value** operation.
2. **Signed freshness attestation.** The custodian/node periodically emits a signed
   `valid-as-of` attestation over the current `sigchain_head`:
   `{ head, as_of, expires = as_of + Δ, seq, alg, sig }`. The verifier **re-fetches**
   it before a freshness-gated action.
3. **Fail closed on stale.** If the freshest attestation a verifier can obtain is older
   than its policy Δ for the action class, the verifier **refuses the high-value
   action** and degrades to read-only — it does **not** fail open.
4. **Rollback resistance independent of clocks.** Each attestation carries a
   **monotonic `seq`**; a verifier remembers the highest `seq` seen for an identity and
   rejects any attestation with a lower `seq`. This means even a perfect clock-skew
   attack cannot *replay* an old "still valid" attestation to resurrect a revoked key —
   freshness is enforced by **both** a signed `as_of` (with bounded skew tolerance)
   **and** a monotonic counter.

**Resolution — Δ and clock-skew defaults (proposed; tunable).**

- **Δ is tiered by action sensitivity, not one global constant:**
  - *Routine "key still valid" checks* (e.g. accepting an ordinary message from a known
    peer): **Δ ≈ 24 h**. Long enough to tolerate the email-speed, both-ends-offline
    budget (NFR-2); a 24 h window on an already-low-value action is an acceptable
    revocation-latency.
  - *High-value actions* (accepting a `rotate_root`, a large-scope delegation, or a
    cross-trust `StateSnapshot` load): **Δ ≤ 15 min**, with a forced live re-fetch of
    head + attestation. The blast radius of acting on a stale-but-revoked key here is
    large, so the freshness bar is tight.
- **Clock skew:** the verifier compares `as_of`/`expires` against its own clock with a
  **bounded tolerance of ±5 min** (configurable). Skew tolerance only *widens* the
  acceptance window slightly; it can never extend a revoked key's life beyond `Δ +
  skew`, and the monotonic `seq` (point 4) closes the residual replay gap. WG-Fed does
  **not** trust local clocks as the sole freshness signal — the `seq` is the
  clock-independent backstop.

*Why.* This is the atproto/Keybase posture (a re-fetchable "valid-as-of" plus a strict
window for sensitive ops) adapted to WG's async budget. Tiering Δ keeps email-speed
tolerance for routine traffic while making the freeze attack ineffective against the
operations that matter; the monotonic `seq` removes the dependence on synchronized
clocks that a pure-timestamp scheme would have.

*Flagged for Erik (tuning only):* the **exact Δ values** (24 h / 15 min) and the **±5
min skew tolerance** are calibration knobs, and whether high-value Δ should be
**operator-configurable per deployment** is a policy choice. The *mechanism* (signed
`as_of` + `expires` + monotonic `seq` + fail-closed-on-stale + bounded skew + the
historical-verification carve-out) is the ADR-001 commitment; the numbers are sensible
defaults Erik can adjust without reopening the design. Note the freshness mechanism is
**shared with ADR-003** (which owns revocation hosting) — this ADR fixes the attestation
*format and verifier rule*; ADR-003 fixes *who hosts* the revocation/attestation feed.

---

## References

- `docs/federation-study/06-decision-memo-and-roadmap.md` — §1 (the decision), §2.1/§2.2
  (substrate + C-with-B-default), §3 HQ1/HQ2/HQ5/HQ6/HQ9/HQ11/HQ12 (the decisions this
  ADR formalizes), §6 ADR-001 stub, §8 (open-question hand-off).
- `docs/federation-study/04-candidate-architectures.md` — §1.1 (three-tier key
  hierarchy), §1.2 (the sigchain), §1.3 (crypto suite + agility), §1.4 (the three wire
  envelopes), §1.5 (`src/identity/` skeleton + touch-points).
- `docs/federation-study/01-prior-art-landscape.md` — §4.2 (the indirection-layer
  lesson: continuity requires identity = stable name, keys = revocable contents), §4.1
  (the V5 custody pattern), §2.1–2.3/§2.14 (Nostr / Keybase / DID / Farcaster), §5
  (the layered composition + interop formats).
- `docs/federation-study/03-requirements-and-hard-questions.md` — FR-I1–I7, FR-S1,
  FR-F1/F2; HQ1 (custody, the crux), HQ5 (addressing), HQ6 (decentralization vs
  central), HQ9 (human vs agent), HQ12 (protocol evolution).
- `docs/federation-study/05-adversarial-evaluation.md` (via the memo) — findings S-3
  (freeze attack), S-4 (the hydra), S-7 (downgrade), D-1/D-2 (did:web Fatal-as-root),
  A-4 (node-less recovery Fatal).
- `docs/ADR-actor-vs-agent-identity.md` — the unified `Agent` identity this model extends.
