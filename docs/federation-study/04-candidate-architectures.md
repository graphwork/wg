# Federation Study 4/6 — Candidate Architectures

> **Headline federation study, wave 1, task 4 of 6 — the *generate* phase.**
>
> This document proposes **four fully-worked candidate architectures** for WG's
> key-based federation / identity / messaging, spanning the decentralization
> spectrum. Each answers **every** hard question from doc 03, is grounded in the
> prior art (doc 01) and the current code state (doc 02), and maps to **concrete
> WG code changes plus a migration path**.
>
> Downstream: `fed-adversarial` (5/6) attacks the custody / rotation / transport
> answers here; `fed-decision` (6/6) picks one (or a phased blend) and roadmaps.

**Status:** draft for evaluation · **Date:** 2026-06-24 · **Owner task:** `fed-architectures`
**Inputs:** `01-prior-art-landscape.md`, `02-current-state-baseline.md`, `03-requirements-and-hard-questions.md`

---

## 0. How to read this document

The four candidates share a large common substrate (the same crypto suite, the
same envelope formats, the same new `src/identity/` module skeleton). Repeating
all of it four times would bury the *actual differences*. So:

- **§1 — Shared design primitives.** The vocabulary every candidate draws from:
  the two-tier key hierarchy, the sigchain indirection layer, the crypto suite,
  the three wire envelopes, the new module skeleton, and the compat handshake.
  Read this first; the candidates are described as *configurations* of it.
- **§2–§5 — The four candidates.** Each is self-contained on its **divergent**
  decisions and carries its **own HQ-answer table** (all 12 hard questions,
  concretely), its **own WG code-mapping + migration path**, and its **own
  tradeoffs**. Where a candidate simply adopts a §1 primitive unchanged, it says
  so rather than re-deriving it.
- **§6 — Cross-candidate comparison** (spectrum placement, per-capability
  central-node table, the decision-relevant deltas).
- **§7 — §4-tension resolution** (each candidate's side on each of the nine
  tensions T1–T9).
- **§8 — HQ coverage matrix** (all 12 HQs × 4 candidates, the validator's
  one-glance checklist) and the **acceptance-checklist** sign-off.
- **§9 — Migration phasing** (v0→v3, common to all; the candidate choice is a
  late binding).

**The four candidates at a glance:**

| | Candidate | Shape | Prior-art lineage | Decentralization |
|---|---|---|---|---|
| **A** | **Fully decentralized P2P** | keys + relays/gossip, no authority | Nostr + SSB + Iroh + Keybase-sigchain | **Maximal** |
| **B** | **Central-node-anchored federation** | one coordinating node per household/org, account-portable | AT-Protocol / PDS + did:plc | **Pragmatic / federated-central** |
| **C** | **Hybrid** (recommended-feeling middle) | key-rooted, *optional* relays + *optional* directory | the doc-01 §5 composition (sigchain + Farcaster/UCAN custody + Nostr/Iroh transport) | **Decentralization-capable, rests on central nodes fine** |
| **D** | **Wildcard — capabilities-first** | did:web domain anchor + UCAN delegation | did:web + UCAN + Sigstore-log | **Domain-anchored (DNS trust root)** |

> **The crux up front (doc 03 HQ1).** Every candidate is arranged so the
> load-bearing decision — *agent key custody* — stays visible. In all four, the
> **portable identity = public identity + signed state + the currently-authorized
> delegate key set, and never the root signing key** (FR-I2, FR-S1). They differ
> only in *where the root key physically lives* and *what "download Nora onto host
> B" means*. Those two answers are stated explicitly per candidate.

---

## 1. Shared design primitives (the substrate all four configure)

Doc 01 §4.2 proved one lesson decisively: **every system that survives key loss
does so via an indirection layer where the identity is a stable name and keys are
revocable contents of a signed, append-only record** (Keybase sigchain ≈ DID-doc
rotation keys ≈ Farcaster on-chain registry). Systems where *key == identity* with
no indirection (SSB, Nostr-as-shipped, raw libp2p/Iroh NodeId) **cannot** offer
V6. Therefore **all four candidates adopt the indirection layer** — they differ
only in *where that record is published and who, if anyone, anchors it.*

### 1.1 The two-tier key hierarchy (answers the V5 custody pattern)

Doc 01 §4.1's synthesis: a **root/custody key** that **issues scoped, revocable
delegations** to agents; the portable identity ships the delegations, never the
root. Concretely, three key roles:

| Tier | Role | Algorithm | Where it lives (default) | What it can do |
|---|---|---|---|---|
| **Root / identity key** | *Is* the identity; signs the sigchain; authorizes/revokes everything below | ed25519 | **Human:** device/OS keychain, ideally hardware-backed (FIDO2/passkey, §doc01 4.1 ★★★). **Agent:** custodian-held in `wg secret` keystore / HSM — *never* on the ephemeral worker host. | Sign sigchain links (add/revoke keys, rotate, delegate). Rarely used online. |
| **Signer / device / agent key** | The working key an agent or device actually signs events & state with | ed25519 | On the acting host (worker, device). Recorded as an authorized method in the sigchain. | Sign messages & state snapshots **within delegated scope**; cannot alter the sigchain. |
| **Encryption key** | Per-recipient confidentiality (the ACL realization, FR-S3) | X25519 (static) + per-message ephemeral | Alongside the signer key | Decrypt envelopes addressed to it; derive per-message keys. |

**Why three tiers, not one.** Collapsing them is exactly the SSB trap (doc 01 §2.5):
one immutable key that is identity + signing + encryption = lose it once and the
identity is dead, leak it once and it is stolen, with no revocation. Splitting
root from signer is what makes rotation (HQ2), agent custody (HQ1), and delegation
(HQ11) all expressible without ever moving the root private key.

**Agent custody, stated precisely (HQ1).** An agent's *signer* key lives on its
host; its *root/identity* key is **custodian-held** — by the human owner or the WG
node operator — in `wg secret` (the credential store doc 02 §2.4 identifies as the
natural home), behind an `ssh-agent`-style boundary: the worker can *request a
signature* (or is issued a short-lived signer/UCAN) but never receives the root
bytes. "Download Nora" copies Nora's `IdentityRecord` + `StateSnapshot`s + her
*public* key set; an honest client can **verify** every Nora artifact but cannot
**author** a new one, because authoring requires a private signer that the
sigchain authorizes — and that signer is not in the bundle (FR-I2, FR-S1).

### 1.2 The sigchain — `identity-name → {current key set}` as signed data

A per-identity **append-only, hash-linked, signed log** (the Keybase model). Each
link is one of: `genesis` (declare root key), `add_key` (authorize a signer/device
with a scope), `revoke_key`, `rotate_root` (old root signs the next — succession),
`delegate` (issue a capability; see HQ11), `set_alias_proof`, `set_endpoints`. A
link is valid iff signed by a key the chain authorized *at that link's position*
(old links stay valid at their historical position — Keybase semantics, doc 01 §2.2).

This single structure discharges: **rotation/recovery** (HQ2 — add a new signer
from a surviving key; rotate root via succession), **revocation** (FR-S7 — a
`revoke_key` link, verifiable), **auditability** (NFR-7 — replay the chain),
**delegation** (HQ11 — `delegate` links), and **continuity semantics** (HQ1/FR-I5
— "same self" = a link that extends *this* chain; "fork" = a new genesis citing
this one as parent).

The candidates differ only in **where the sigchain is published & anchored**:
gossiped over relays (A), hosted in the node's repo (B), published anywhere with
an optional directory mirror (C), or pinned under a domain's `.well-known` (D).

### 1.3 Crypto suite (FR-S5 — compose audited primitives, invent nothing)

| Purpose | Primitive | Prior-art precedent |
|---|---|---|
| Signatures | **ed25519** | Nostr/SSB/Iroh/DID `did:key` all use ed25519 or secp256k1 |
| Key agreement | **X25519** (ECDH) | age, Signal, NIP-44 |
| AEAD (envelope) | **XChaCha20-Poly1305** | age, NIP-44, libsodium sealed boxes |
| KDF | **HKDF-SHA-256** | Signal, MLS |
| Content addressing | **BLAKE3** (→ a multihash CID) | IPFS CID family (doc 01 §2.8) |
| Forward-secret sessions (opt) | **Double Ratchet** / **MLS** group | Signal (doc 01 §2.13), MLS for groups |

Every primitive maps to a named, audited construction (FR-S5). **Crypto agility
(HQ12):** every signed structure carries an explicit `alg` id, so a future
suite (e.g. ed25519→ML-DSA post-quantum) migrates by adding a method to the
sigchain and dual-signing during the window — without abandoning existing
identities.

### 1.4 The three wire envelopes (self-describing, versioned — NFR-3/NFR-4)

All three begin with `{ "v": <u16>, "alg": "<suite-id>" }` and are
content-addressed by BLAKE3 of their canonical (sorted-key) serialization.

**(a) `IdentityRecord`** — the public, portable identity (the V2 "downloadable
identity"; contains **no** private key):
```jsonc
{ "v": 1, "alg": "ed25519",
  "id": "wgid:<multibase-ed25519-pubkey>",   // the root pubkey == the address
  "sigchain_head": "<blake3-cid>",            // latest sigchain link
  "keys": [ {"kid":"...","pub":"...","role":"signer|enc","scope":[...],"status":"active|revoked"} ],
  "endpoints": [ {"kind":"relay|node|inbox|state","uri":"..."} ],  // how to fetch state+inbox
  "alias_proofs": [ {"alias":"@nora","proof":"dns|https|social","url":"..."} ],
  "agent_fields": { "role_id":"...","trust_level":"...","executor":"...","capabilities":[...] },
  "sig": "<root-or-authorized-key signature over the above>" }
```
`agent_fields` carries the operational fields WG already unifies onto `Agent`
(FR-I6, doc 02 §2.2a) so a pulled federated identity is dispatchable without a
schema mismatch.

**(b) `StateSnapshot`** — loadable/portable state (V1/V2; the HQ10 format):
```jsonc
{ "v": 1, "alg": "ed25519",
  "identity": "wgid:...",
  "payload_kind": "conv-cache-v1 | summary-v1 | opaque-blob-v1 | <future>",  // TAGGED & evolvable
  "model_binding": { "model":"claude-opus-4-8", "min_reader":"conv-cache-v1" }, // HQ10 wrong-model guard
  "content_cid": "<blake3 of the (possibly-encrypted) payload>",
  "prev": "<cid of prior snapshot | null>",   // incremental: publish a delta, not the whole history
  "enc": { "scheme":"per-recipient", "recipients":[ {"kid":"...","wrapped_key":"..."} ] }, // optional
  "sig": "<authorized-signer signature>" }
```
The **interface is stable; `payload_kind` is the evolvable slot** (HQ10/FR-I3): a
v1 conversation cache and a hypothetical v2 opaque tensor blob load through the
*same* loader; an old client hitting an unknown `payload_kind` degrades gracefully
(verifies signature + provenance, surfaces "state present, payload unreadable by
this client" — never silently corrupts). `prev` gives incremental publish
(append a turn without re-uploading history). We design the *slot*, not the opaque
payload (doc 03 §5 non-goal 7).

**(c) `SignedEvent`** — a message (V3; the HQ3 unit of transport):
```jsonc
{ "v": 1, "alg": "ed25519",
  "id": "<blake3 cid — idempotent dedup key (FR-M6)>",
  "from": "wgid:...", "to": ["wgid:...", ...],     // addressed by PUBKEY, not path
  "created_at": "<rfc3339>",
  "kind": "msg | ack | task-ref | state-head | sigchain-link | delegation",
  "refs": [ {"rel":"reply|task|artifact","cid":"..."} ],  // threading/causality (FR-M5)
  "body": "<plaintext OR>", "ciphertext": "<sealed envelope>",  // encrypted = ACL (FR-S3)
  "sig": "<authorized-signer signature over all of the above>" }
```
Any recipient verifies sender authenticity & integrity **offline** (FR-M1); a
forged "from Nora" fails the signature check; the `to` set is the encryption
recipient set, so **encrypting-to-recipients *is* the ACL** (FR-S3, HQ4).

### 1.5 New & changed code surface (common skeleton)

A **new `src/identity/` module** is the home for everything cryptographic (doc 02
§2.4 confirms the tree has *zero* signing crypto today; the only `private_key`
symbol is an unrelated VAPID push key):

```
src/identity/
  mod.rs          // WG_FED_COMPAT_VERSION, public API: gen/load/verify/sign
  keys.rs         // ed25519/X25519 keypair gen; custody boundary over wg secret (host-held signer)
  sigchain.rs     // append-only signed log: genesis/add_key/revoke/rotate/delegate; verify()
  did.rs          // wgid:/did:key/did:web resolution → IdentityRecord (+ endpoints)
  envelope.rs     // IdentityRecord/StateSnapshot/SignedEvent sign + verify + canonical encode
  custody.rs      // remote-sign / ssh-agent-style request-signature; UCAN-style delegations (HQ11)
  acl.rs          // per-recipient seal/unseal (X25519 + XChaCha20-Poly1305); group rekey (HQ4)
```

Touch-points in existing code (cited to doc 02's `file:line`):

- **`src/secret.rs` (`Backend` enum `:31`)** — becomes the **signing-key
  custodian**. Add a typed signing-key store (not just opaque API strings) and an
  `ssh-agent`-style "sign this digest" call so a worker never holds root bytes
  (FR-S1, HQ1). New crates: `ed25519-dalek`, `x25519-dalek`, `blake3`,
  `chacha20poly1305`, `hkdf` (today: only `sha2`/`rustls`/`keyring`, doc 02 §2.4).
- **`src/federation.rs` (`Remote`/`PeerConfig` `:21`/`:31`, `transfer()` `:637`,
  `AccessPolicy` `:627`)** — peers gain a **key/URL identity** beside `path`;
  `transfer()` learns to push/pull *signed* `IdentityRecord`/`StateSnapshot`
  artifacts (not just plaintext YAML); `AccessPolicy` gains the per-recipient /
  encrypted realization the doc-comment already promises as a "future extension".
- **`Agent` (`src/agency/types.rs:505`)** — add `pubkey: Option<String>`
  (the `wgid:`), `sigchain_head: Option<String>`, and promote `contact` from a
  display-only string (doc 02 R11/R22) to a **routed binding table**
  (`endpoints`). `trust_level` (`:521`) is reused for FR-T3 gating. Content-hash
  IDs (`hash.rs:70`) keep working for local/legacy agents and coexist with
  pubkey IDs.
- **`wg msg` / `src/messages.rs` (`Message` `:44`)** — add `from`/`to`/`sig`/
  `refs` (all `#[serde(default)]` so today's task-keyed JSONL still parses —
  backward compatible); add a **cross-graph send/poll** path that wraps the local
  queue at one end and a relay/node inbox at the other.

**Compat handshake (HQ12, NFR-4).** A new `WG_FED_COMPAT_VERSION` in
`src/identity/mod.rs` mirrors `WG_AGENCY_COMPAT_VERSION` (currently `"1.2.4"`,
`src/agency/mod.rs:16`) — peers exchange it on first contact and **fail loudly**
on an incompatible mismatch (WG's existing convention), negotiate the shared
subset otherwise.

With the substrate fixed, the four candidates are *where you publish & anchor the
sigchain, how bytes move, and what (if anything) is central.*

---

## 2. Candidate A — Fully decentralized P2P

> **Shape:** keys + relays/gossip, **no central authority of any kind**.
> Lineage: Nostr (key=identity=address, relays, signed events) + SSB (gossip of
> append-only signed logs) + Iroh (Rust-native dial-by-pubkey QUIC) + a
> Keybase-style sigchain bolted on so it does *not* repeat SSB's no-recovery trap.
> **Maximal decentralization** — this is the left edge of doc 01 §3's spectrum.

### 2.1 Identity & key custody (HQ1, HQ9)

- **Identity** = the root ed25519 pubkey, encoded `wgid:<multibase>` (§1.4a). The
  pubkey *is* the identity *and* the address (V1/FR-I1), self-certifying, no
  registry.
- **Custody.** Human root keys live on the human's device (passkey/OS keychain,
  ideally hardware-backed — the strongest "download ≠ impersonation" guarantee,
  doc 01 §2.17). **Agent root keys are custodian-held in `wg secret`**; the agent
  worker gets a **signer key** (added to the sigchain) and signs through the
  custody boundary (§1.1). Because A has no node and no server, the custodian is
  the **human owner** running the relay-facing client.
- **What "download Nora onto host B" means (FR-I5).** Default = **fork**: pulling
  Nora's `IdentityRecord`+state onto an un-authorized host produces a *verifiable
  read-only copy* (you can render Nora's history, you cannot sign as Nora). To make
  host B *the same Nora*, B's signer key must be **added to Nora's sigchain by a
  surviving authorized key** (`add_key` link) — an explicit, signed act. Same-self
  vs fork is therefore a one-bit, cryptographically-enforced decision, not an
  accident.

### 2.2 Rotation & recovery (HQ2, FR-S2/S7)

- **Rotation:** `add_key`/`revoke_key` sigchain links — any surviving authorized
  key adds a new signer or revokes a compromised one; revocation is a verifiable
  link (FR-S7).
- **Recovery (the SSB trap, avoided):** because identity = sigchain (not the raw
  key), losing one signer is survivable as long as **one authorized key remains**
  (another device, or a **paper key** generated at genesis — Keybase's model). For
  total loss, A offers **M-of-N social recovery**: guardians' keys, named in the
  sigchain at genesis, can jointly sign a `rotate_root` succession link. This is
  the only recovery primitive that needs no central node, so it is A's *only*
  recovery path — the cost of maximal decentralization is that recovery is
  entirely self-organized (no node-held recovery key like B).
- **Stable-across-rotation address.** The address is the **genesis root pubkey**;
  rotations change the *active signer set* underneath, not the `wgid:`. (If the
  root itself rotates via succession, the chain links old→new so resolvers follow
  the pointer — the *identity* is the chain, the *address* is its genesis id.)

### 2.3 Addressing (HQ5, FR-F1/F2)

- **Locator:** `wgid:<pubkey>` resolves to `IdentityRecord` → `endpoints` (relay
  list) → state + inbox. Resolution is **self-contained**: a signed, gossiped
  "relay list" event (Nostr NIP-65 "outbox model", doc 01 §2.1) tells you which
  relays carry this key's events; a **Kademlia/Iroh DHT** provides
  pubkey→relay-hint discovery with no registry.
- **Aliases:** **petnames only** (local, per-user nicknames) plus optional
  self-hosted `name@domain` proofs (NIP-05 style: a `.well-known/wg.json` you host
  yourself, verifiable back to the key). **No global alias registry** — that would
  be a central node A forbids. Zooko's triangle (T2): A picks
  **secure + decentralized**, sacrifices global human-meaningful names.

### 2.4 Messaging / transport (HQ3, FR-M*)

- **Transport:** **relays + gossip**, exactly Nostr's model with an Iroh P2P fast
  path. A client publishes `SignedEvent`s to the recipient's advertised relays;
  the recipient polls (or subscribes to) those relays. When both ends are online,
  **Iroh QUIC** (dial-by-pubkey, relay-assisted hole-punch, doc 01 §2.9) gives a
  direct path; otherwise the **relay store-and-forwards** until the recipient
  polls — **email-speed, both-ends-offline-tolerant** (FR-M2, NFR-2).
- **No mandatory relay (FR-F4/F5):** an identity advertises *several* relays;
  losing any one degrades reach, not correctness. Anyone self-hosts a relay
  (NFR-5) — a relay is a dumb, untrusted store-and-forward box (bytes are signed +
  encrypted end-to-end, so the relay is never trusted, HQ3 success criterion).
- **Delivery/read semantics:** at-least-once with idempotent `id` dedup (FR-M6);
  causal ordering per conversation via `refs` (not global order); read receipts are
  optional `kind:ack` events.

### 2.5 Loadable / portable state (HQ10, FR-I3/I4)

- `StateSnapshot` (§1.4b), **content-addressed (BLAKE3) and signed**; published to
  the same relays (or Iroh-blobs / IPFS) as immutable blobs, with a mutable
  **signed "state-head" event** acting as the IPNS-like pointer to the latest CID
  (doc 01 §2.8). Tampering → hash/sig mismatch (FR-I4). Incremental via `prev`.

### 2.6 Trust & anti-abuse (HQ8, FR-T1/T2)

- **Trust establishment:** TOFU (pin the key on first contact) + **web-of-trust**
  from the follow graph (SSB's friend-of-friend, doc 01 §2.5) + optional
  Keybase-style **social proofs** (self-hosted). No CA.
- **Sybil/spam:** keys are free, so A leans on **consent gates** — an unknown
  `from` lands in a "requests" tray, not the inbox, until accepted; relays apply
  **proof-of-work** (a small hashcash stamp on events from unknown keys, Nostr
  NIP-13 style) and **rate limits**. WoT distance gates trust_level (FR-T3). This
  is A's weak point: with no anchoring node, sybil resistance is purely
  social/PoW — adequate for email-speed, attackable at scale (flagged for doc 05).

### 2.7 Encryption / ACL & metadata (HQ4, FR-S3/S4)

- **Per-recipient sealed envelopes** (§1.3): encrypt the payload key to each
  recipient's X25519 key; the `to` set *is* the ACL (FR-S3). Groups use
  **sender-keys with rekey-on-membership-change**; large/long-lived groups can opt
  into **MLS** for forward secrecy (HQ4).
- **Metadata stance (FR-S4):** relays see `from`, `to`, timing, size. A offers
  **sealed-sender** (Signal-style, doc 01 §2.13) to hide `from` from the relay, but
  **does not** promise recipient-unlinkability or mixnet-grade anonymity (doc 03
  §5 non-goal 5). The leak surface is disclosed, not eliminated.

### 2.8 Consistency (HQ7, FR-M6)

- **Single-writer-per-object** is the default and the cleanest fit: *your key is
  authoritative for your own identity state*, so identity-state has no conflicts.
  Shared mutable objects (a co-edited thread, a shared task) use **CRDTs**
  (automatic merge) or, where a CRDT is overkill, **last-writer-wins with version
  vectors** that *surface* (never silently drop) conflicts. Ordering is **causal**
  per conversation via `refs`.

### 2.9 Concrete WG mapping + migration

| Surface | Change |
|---|---|
| **NEW `src/identity/`** | Full module (§1.5); plus an **Iroh** transport adapter and a **relay client** (publish/subscribe `SignedEvent`s). |
| **`src/federation.rs`** | `PeerConfig`/`Remote` gain `wgid` + `relays: Vec<String>` beside `path`; `resolve_peer` (`:301`) learns key→relay resolution; `transfer()` (`:637`) gains a signed-artifact path. |
| **`Agent` (`types.rs:505`)** | `+pubkey`, `+sigchain_head`; `contact`→`endpoints` binding table (relay URIs). |
| **`src/messages.rs`** | `Message` `+from/+to/+sig/+refs` (`#[serde(default)]`); new `relay_send`/`relay_poll`; a `RelayMessageAdapter` beside the existing executor adapters (`:507`). |
| **`src/secret.rs`** | Signing-key custody + remote-sign (§1.5). |

**Migration from today (doc 02):** ① add `src/identity/` + crates, keep everything
local — `wg identity new` mints a keypair into `wg secret`, writes a genesis
sigchain. ② Dual-write: `Message` gets optional `sig` (old readers ignore it). ③
Stand up a relay (`wg relay serve`) + Iroh adapter; `wg msg --to wgid:...` routes
cross-graph. ④ Path-based peers in `federation.yaml` keep working; key-based peers
are added alongside. No big-bang; content-hash agent IDs and pubkey IDs coexist.

### 2.10 Maturity / risk / op-cost

- **Maturity:** transport pieces are real (Iroh 1.0 shipped 2026, doc 01 §2.9;
  Nostr relays mature). The sigchain layer is new code WG must own.
- **Risk:** discovery & recovery UX are the hardest (no anchor); sybil resistance
  is purely social/PoW; "is anyone storing my state?" needs pinning incentives.
- **Op-cost:** lowest server cost (relays optional & cheap), **highest user-side
  burden** (you manage your own keys, relays, guardians).

---

## 3. Candidate B — Central-node-anchored federation

> **Shape:** a **coordinating node per household/org** (the AT-Proto **PDS**
> analogue) with **account portability across nodes**. Pragmatic, operationally
> simple, still key-rooted. Lineage: AT Protocol / Bluesky (doc 01 §2.4) +
> did:plc rotation/recovery keys. Right-of-center on the spectrum.

### 3.1 Identity & key custody (HQ1, HQ9)

- **Identity** = a **DID** anchored by the node: `did:wg:<node-host>/<id>` (or
  reuse `did:plc`-style content-addressed DIDs). The DID resolves to a DID
  document (= our `IdentityRecord`) the node hosts and serves. Handle alias
  `@nora.garrison.family` is a mutable, DNS/`.well-known`-verified human name
  resolving to the DID (FR-F2, doc 01 §2.4).
- **Custody (the PDS model).** The **node holds the day-to-day signing keys** for
  the identities it hosts (exactly as a PDS holds repo signing keys) — for agents
  this is *ideal*: the agent worker calls the node to sign, never holds key bytes
  (FR-S1). The **human owner holds higher-priority rotation/recovery keys offline**
  (atproto's rotation+recovery key model, doc 01 §4.2 ★★★) so a *hostile or failed
  node* can be overridden. This is the cleanest **agent custody** answer of the
  four: agents are first-class node-hosted identities; their signing key lives in
  the node, not on the ephemeral worker (HQ9 — agent custody is *delegated to the
  node-custodian* by design, distinct from human self-held recovery keys, FR-S6).
- **"Download Nora onto host B" (FR-I5).** This is **account migration**, a
  first-class flow (atproto's headline, doc 01 §2.4): copy Nora's signed repo to
  node B, then sign a **rotation-key operation** repointing the DID's endpoints at
  B. The DID — the identity — is unchanged; it is **still Nora, on a new node**.
  Forking (a *copy* that is *not* Nora) is the explicit alternative: import the
  repo *without* the rotation op → a new DID citing Nora's as parent.

### 3.2 Rotation & recovery (HQ2)

- **Strongest of the four.** DID doc carries rotation keys; a higher-priority
  **recovery key can override the node within a time window** if the node is
  malicious (atproto's 72h model). Day-to-day signer keys rotate via node-signed
  ops; the human's offline recovery key is the backstop. **App-password-style
  scoped credentials** give revocable secondary access. Identity survives node loss
  *and* key loss as long as the offline recovery key survives.

### 3.3 Addressing (HQ5)

- **Locator:** the DID (resolves via the node + a **directory**). Human handle via
  DNS/`.well-known` (human-meaningful by construction). Zooko (T2): B picks
  **human-meaningful + secure**, accepting a **directory** as a (mitigated)
  central dependency — see §3.9 mitigation.

### 3.4 Messaging / transport (HQ3)

- **Node-to-node store-and-forward over HTTP** (ActivityPub/PDS inbox model, doc
  01 §2.6/§2.4): sender's node POSTs a `SignedEvent` to the recipient's node
  inbox; the node **holds it until the (possibly-offline) agent polls** — email-
  speed, offline-tolerant, and *operationally the simplest* because the node is
  always-on and NAT-free (no hole-punching). A **firehose** gives near-real-time
  fanout for online clients. Bytes are still signed (and optionally encrypted)
  end-to-end so a *peer* node is untrusted, even though *your own* node is trusted.

### 3.5 Loadable / portable state (HQ10)

- State lives in a **signed repo** on the node (atproto's Merkle-tree repo;
  records are content-addressed). `StateSnapshot`s are repo records; fetch over
  HTTP; portability = the migration flow (§3.1). The node makes "is my state
  stored & available?" trivial (the node stores it) — B's big UX win over A.

### 3.6 Trust & anti-abuse (HQ8)

- DID-doc + handle verification establishes identity; **node reputation** and
  per-node **allow/block + rate limits** give strong, easy spam control (the node
  is a natural choke point — B's anti-abuse is the easiest of the four). trust_level
  (FR-T3) gates dispatch. Sybil resistance benefits from handle/DNS cost.

### 3.7 Encryption / ACL & metadata (HQ4)

- Per-recipient encryption = ACL for **cross-node** confidentiality (FR-S3). As a
  *convenience*, the node can additionally enforce **server-side ACLs** for
  same-node access — but encryption remains the real boundary (never trust a peer
  node). Metadata: your node sees your social graph (a real leak, disclosed,
  FR-S4); sealed-sender hides `from` from *peer* nodes.

### 3.8 Consistency (HQ7)

- **Single-writer = the owning node is authoritative for its repo** — the simplest
  model of the four (atproto's design). Strong-ish consistency within a node;
  eventual across nodes via the firehose. Conflicts are rare because each object
  has one authoritative node.

### 3.9 Concrete WG mapping + migration

| Surface | Change |
|---|---|
| **The WG node = the existing daemon** | The `wg service`/daemon + its Unix-socket IPC (doc 02 §2.1d) **becomes the PDS**: add an HTTP inbox + repo-serve + DID-resolve endpoint. The biggest reuse — federation today is *already* node-mediated (just same-filesystem); B promotes it to network-mediated. |
| **`src/federation.rs`** | `PeerConfig.path` → `PeerConfig.node_url` + `did`; `resolve_remote_task_status` (`:458`) gains an HTTP transport beside the Unix-socket one (`:545`); `transfer()` pushes signed repo records. |
| **NEW `src/identity/`** | did.rs does `did:wg`/`did:plc` resolution; sigchain is the repo's key-ops log; custody.rs is the **node-side signing service**. |
| **`Agent` (`types.rs:505`)** | `+did`, `+node_url`; `contact`→`endpoints`. |
| **`src/messages.rs`** | `Message` `+from/+to/+sig`; node inbox `POST`/poll; a `NodeInboxAdapter`. |

**Migration from today (doc 02):** B is the *shortest path* from the current code,
because doc 02 §2.1 shows federation is already a node (daemon) brokering
cross-repo queries over a socket. ① Add HTTPS to the daemon's existing IPC. ②
`wg identity new` mints a node-hosted DID + node-held signing key in `wg secret`.
③ `federation.yaml` peers gain `node_url`; the socket path stays as the
same-machine fast path. ④ Account migration ships last.

**Central dependency & its mitigation (HQ6).** B's directory (DID→node
resolution) is a *de facto* central node — atproto's honestly-acknowledged weak
spot (doc 01 §2.4 "credibly decentralized, not P2P"). Mitigation: the directory
is a **mirrorable, signed, append-only log** (anyone can run a mirror; entries are
self-verifying), and a DID can fall back to a self-resolving `did:web`-style
`.well-known` so **directory loss degrades discovery, not correctness** (FR-F5).
B *accepts* more centralization than C/A and says so.

### 3.10 Maturity / risk / op-cost

- **Maturity:** highest — AT Proto runs at millions of users; the pattern is
  proven end-to-end (identity + migration + recovery, doc 01 §2.4).
- **Risk:** the directory is a centralization WG must consciously bound; a node is
  a juicier target than a dumb relay (it holds signing keys).
- **Op-cost:** **lowest user burden** (the node does the work), **moderate
  operator burden** (someone runs the always-on node per household/org; NFR-5 is
  met — a single person can run one).

---

## 4. Candidate C — Hybrid (key-based identity + optional relays + optional central directory)

> **Shape:** **decentralization-*capable*, but works fine resting on central
> nodes.** Key-rooted self-certifying identity (like A) that can *optionally* lean
> on relays and a directory (like B) for availability & discovery, with **nothing
> correctness-critical depending on any central node** (FR-F4/F5). This is the
> doc-01 §5 "layered composition" recommendation and the doc-01 §3 target band
> (Iroh ↔ Nostr ↔ Farcaster). It is the most direct realization of vision pillar
> **V5** ("lean decentralized, central nodes *allowed*").

### 4.1 Identity & key custody (HQ1, HQ9) — the Farcaster/UCAN core

- **Identity** = self-certifying `wgid:<root-pubkey>` (A's model), with the
  sigchain published wherever the user likes (relay, node, IPFS, file) and
  *optionally* mirrored in a directory for fast lookup.
- **Custody = the doc-01 §4.1 winning pattern, made primary.** A **two-tier
  hierarchy**: a host-held/human-held **root key** that issues **scoped,
  revocable, expiring delegations** to agents — **Farcaster *signers*** + **UCAN
  *capabilities*** (doc 01 §2.10/§2.14, the two ★★★ V5 exemplars). An agent gets a
  **signer key authorized by a `delegate` sigchain link** *and/or* a short-lived
  UCAN token; it can *act within scope* but **downloading/copying a signer ≠
  owning the identity** (the sigchain, not key-possession, is authoritative —
  Farcaster's exact property). This is the strongest, most flexible custody answer:
  it supports both "agent has a standing signer" (Farcaster) and "agent gets a
  short-lived capability per session" (UCAN, ssh-agent-forwarding-style).
- **Agent vs human (HQ9):** **one identity type with capability flags** (WG's
  current `Agent` direction, doc 02 §2.2). Humans self-hold the root (passkey);
  agents have their root **custodian-held** and operate via delegated signers.
  Differences are by-design (FR-S6), not emergent.
- **"Download Nora onto host B" (FR-I5):** *fork by default* (verifiable copy);
  *same-self* requires either (a) an `add_key`/`delegate` sigchain link (A's way)
  **or** (b) if Nora rests on a node, a node-mediated migration (B's way). C
  supports **both** continuity mechanisms — the user picks per their topology.

### 4.2 Rotation & recovery (HQ2)

- **All of A's *and* B's options, composable.** Sigchain succession + device/paper
  keys + M-of-N social recovery (A); *plus*, if the user opts into a node, a
  node-held recovery key (B). The user trades decentralization for recovery-ease by
  choosing which to enable — **the recovery story scales with how central you
  choose to be.** Revocation is a sigchain link (FR-S7).

### 4.3 Addressing (HQ5)

- **Locator:** `wgid:<pubkey>` (self-certifying core). **Resolution is a fallback
  cascade:** (1) a locally-cached signed relay-list/endpoint record; (2) an
  *optional* directory hint (fast, convenient); (3) a DHT lookup (no infra). Any
  one suffices — **no single resolver is mandatory** (FR-F1/F4).
- **Aliases:** **petnames** (local) + *optional* **verified directory aliases**
  (`@nora`, with a proof checkable back to the key) — never mandatory, never a
  central naming authority (FR-F2). Zooko (T2): C **lets the user choose their
  point on the triangle** — raw key (secure+decentralized) by default, opt into a
  directory alias (adds human-meaningful) when convenient.

### 4.4 Messaging / transport (HQ3)

- **Pluggable, with a fallback ladder** (the hybrid of doc 03 HQ3's three axes):
  **Iroh P2P** direct when both online → **shared relays** for store-and-forward
  when one is offline → an *optional* always-on **node/super-relay** per org for
  guaranteed availability. **No single mandatory relay** (FR-F4): the same
  `SignedEvent` can traverse any of them; removing any one degrades convenience
  only (FR-F5). Email-speed, offline-tolerant, untrusted transport (signed +
  encrypted end-to-end).

### 4.5 Loadable / portable state (HQ10)

- `StateSnapshot` (§1.4b), content-addressed + signed, **published to any/all of:
  a relay, the optional directory, IPFS, or a plain file** (NFR-3 — same bundle
  round-trips through ≥2 transports). Mutable signed head pointer (IPNS-style).
  Incremental via `prev`. Tagged `payload_kind` future-proofs the opaque-state slot.

### 4.6 Trust & anti-abuse (HQ8)

- **Layered:** TOFU + WoT (A) + *optional* directory-anchored proofs / Keybase
  social proofs (richer when a directory exists). Anti-abuse: consent gates + PoW
  (A) for the pure-P2P path, *plus* node/relay rate-limits + reputation (B) when a
  central node is in the path. **You get stronger anti-abuse exactly where you've
  accepted more centralization** — the tradeoff is explicit and per-deployment.

### 4.7 Encryption / ACL & metadata (HQ4)

- Per-recipient sealed envelopes = ACL (FR-S3); MLS groups for forward secrecy.
  Metadata: sealed-sender option; the *optional* directory/node sees more than a
  dumb relay, so the leak surface **scales with the central components you enable**
  — disclosed per-deployment (FR-S4).

### 4.8 Consistency (HQ7)

- Single-writer-per-identity-state (no conflicts on your own state) + CRDTs for
  shared objects + causal ordering — A's model. If a node is in the path it can
  *additionally* provide a serialization point for its hosted objects (B's model),
  but correctness never *requires* it.

### 4.9 Concrete WG mapping + migration

C is a **superset** of A and B — the same `src/identity/` module (§1.5) with
*both* transport adapters (relay/Iroh **and** node-HTTP) and *both* resolution
paths (self-resolving/DHT **and** directory), selected by config. Critically,
**this is what the phased rollout in §9 naturally converges to**: ship the
self-certifying core first (A-like), add an optional node/directory later (B-like),
and you *are* at C. Concretely:

| Surface | Change (superset of §2.9 + §3.9) |
|---|---|
| **`src/federation.rs`** | `PeerConfig` gains `wgid` + a `transport: {relay\|node\|iroh\|path}` enum; `resolve_peer` (`:301`) becomes a resolution cascade; `AccessPolicy` (`:627`) gets the encrypted per-recipient realization. |
| **NEW `src/identity/`** | Full §1.5 module + `custody.rs` carrying **both** Farcaster-style standing signers **and** UCAN-style short-lived delegations (the C-specific richness). |
| **`Agent` (`types.rs:505`)** | `+pubkey`, `+sigchain_head`, `+delegations`; `contact`→`endpoints` (multi-transport binding table — finally satisfies R22, doc 02 §4). |
| **`src/messages.rs`** | `Message` `+from/+to/+sig/+refs`; adapters for relay, Iroh, and node inbox, chosen by the recipient's advertised endpoints. |
| **`src/secret.rs`** | Signing-key custody + remote-sign + UCAN issuance. |

**Migration from today:** identical to §9's phasing — C *is* the end-state of the
incremental path (FR-F6, NFR-6). Each phase is independently useful; you stop
wherever your decentralization/ops appetite lands.

### 4.10 Maturity / risk / op-cost

- **Maturity:** the *components* are individually proven (Farcaster signers, UCAN,
  Nostr relays, Iroh, sigchain); the *composition* is the new work and the largest
  surface area of the four. Doc 01 §5 explicitly recommends this composition.
- **Risk:** **most moving parts** → most to test and keep coherent (the chief
  risk; flagged for doc 05). The optionality that is its strength is also a
  configuration-complexity cost.
- **Op-cost:** **tunable** — run it fully P2P (A's cost) or rest it on a node (B's
  cost). Best fit to V5 precisely because the operator chooses.

---

## 5. Candidate D — Wildcard: capabilities-first (did:web + UCAN)

> **Shape:** identity anchored to a **domain** (`did:web`), authority expressed
> purely as **UCAN capability chains**. The "capabilities-first, human-meaningful-
> first" alternative. Lineage: did:web (doc 01 §2.3) + UCAN (§2.10) + a
> Sigstore-style transparency log (§2.11). Domain-anchored — trades pure
> decentralization for human-meaningful names and the strongest delegation story.

### 5.1 Identity & key custody (HQ1, HQ9)

- **Identity** = `did:web:garrison.family:nora` → a DID document hosted at
  `https://garrison.family/.well-known/did.json` (human-meaningful **by
  construction**), with a `did:key` self-certifying fallback for portability.
- **Custody = "delegate, don't share keys," taken to its logical end** (UCAN's
  founding slogan, doc 01 §4.1 ★★★). The **root key never signs day-to-day
  anything**; it only issues **UCAN tokens**. An agent holds a **short-lived,
  scoped UCAN** (`iss`=human DID, `aud`=agent DID, capability + expiry) and acts
  under it. The agent's *own* signer key is trivial/disposable because authority
  comes from the *token*, not the key — so a leaked agent key is near-worthless
  once the UCAN expires. This is the **cleanest HQ11 (authority/delegation)** answer
  and a very strong HQ1 answer: "download the agent" gets you an expired-or-
  expiring capability, never the root.
- **"Download onto host B":** D is *capability-centric*, so "same self" = "holds a
  valid UCAN from the root"; a copy without a fresh UCAN simply can't act after
  expiry — continuity is **time-boxed by construction**.

### 5.2 Rotation & recovery (HQ2)

- **Rotation = edit the DID document** (did:web: change the hosted JSON; the DID is
  stable, doc 01 §2.3/§4.2) + re-issue UCAN chains from the new key. **Recovery =
  domain control**: whoever controls the domain controls the DID — which is also
  D's central weakness (lose the domain → lose the identity, unless the `did:key`
  fallback + a pre-published recovery key is configured). Revocation = UCAN
  revocation list + DID-doc key removal (FR-S7).

### 5.3 Addressing (HQ5)

- **Locator:** the `did:web` URL (resolves via HTTPS — human-meaningful, but
  **DNS/CA-dependent**). Zooko (T2): D openly picks **human-meaningful + secure**,
  **sacrifices decentralization** (DNS is the naming authority). The `did:key`
  fallback restores decentralization at the cost of memorability.

### 5.4 Messaging / transport (HQ3)

- **Transport-agnostic** — UCAN/DIDComm tokens travel in any envelope (doc 01
  §2.10 "tokens travel however you like"). In WG, reuse C's relay/node transport;
  D's contribution is the **capability-gated `SignedEvent`**: every message carries
  (or references) the UCAN proving the sender's authority for that action. Email-
  speed, offline-tolerant (the transport is borrowed; the auth model is the novelty).

### 5.5 Loadable / portable state (HQ10)

- Same `StateSnapshot` as §1.4b, but **fetch is UCAN-gated** (you present a
  capability to read encrypted state) and provenance can be logged to a
  **Sigstore/Rekor-style transparency log** (doc 01 §2.11) for tamper-evident
  audit (NFR-7) — D's distinctive auditability feature.

### 5.6 Trust & anti-abuse (HQ8)

- **Domain = trust anchor** (DNS/CA, like did:web): strong binding to a real
  org/household, strong anti-sybil (domains cost money & are accountable), at the
  price of a centralizing root. Capability chains are offline-verifiable. A
  transparency log makes misbehavior auditable after the fact.

### 5.7 Encryption / ACL & metadata (HQ4)

- Per-recipient encryption = ACL (FR-S3), but D can *also* express access as a
  **capability** ("holder of UCAN X may decrypt") — a second, composable ACL
  mechanism. Metadata leak surface includes the domain host and (if used) the
  transparency log (disclosed, FR-S4).

### 5.8 Consistency (HQ7)

- DID-doc (single hosted source) is single-writer; UCAN tokens are immutable
  once issued; shared state uses CRDTs as in C. The hosted DID doc gives a clean
  serialization point at the price of centralization.

### 5.9 Concrete WG mapping + migration

| Surface | Change |
|---|---|
| **NEW `src/identity/did.rs`** | `did:web` resolver (HTTPS `.well-known/did.json`) + `did:key` fallback. |
| **NEW `src/identity/custody.rs`** | **UCAN issue/verify/revoke** as the primary custody mechanism (vs C's signer+UCAN blend). |
| **`src/federation.rs`** | peers addressed by `did:web` URL; `transfer()` is UCAN-gated. |
| **`Agent` (`types.rs:505`)** | `+did` (did:web), `+held_ucans`; `contact`→domain endpoints. |
| **`src/messages.rs`** | `SignedEvent` carries/references a UCAN; verify the capability chain on receipt. |

**Migration:** D layers cleanly onto C's transport (so it shares §9's transport
phases) but swaps the *identity anchor* to a domain — attractive for
**org/household deployments that already own a domain** and want human-meaningful
names + airtight delegation, less so for individuals who want no DNS dependency.

### 5.10 Maturity / risk / op-cost

- **Maturity:** did:web is a W3C Rec with broad tooling (doc 01 §2.3); UCAN is
  emerging (v1.0 line). The combination is novel for a messaging substrate.
- **Risk:** **DNS/domain is a hard central dependency** (and a censorship/seizure
  surface) — the opposite of A's stance; UCAN revocation-at-scale is an open
  problem (flagged for doc 05).
- **Op-cost:** low if you already run a domain; the domain *is* the infra.

---

## 6. Cross-candidate comparison

### 6.1 Spectrum placement (extends doc 01 §3)

```
 FULLY P2P <--------------------------------------------------> CENTRAL NODE
   A (relays+gossip+Iroh)        C (key core, optional node)        B (node/PDS)
        |                              |                              |
   no authority;             self-certifying core that          per-household node
   sigchain gossiped;        rests on optional relays/          holds signing keys;
   social recovery only      directory; nothing critical        directory + migration
                             is central                          (atproto-shaped)

   D (did:web + UCAN): off-axis — domain-anchored identity (DNS trust root) with
   offline-verifiable capability delegation; transport borrowed from C.
```

### 6.2 Per-capability central-node table (HQ6 — what may be central, what must not)

For each capability: **CC** = correctness/security-critical (must NOT depend on a
single central node), **CV** = convenience-only (central node allowed; loss
degrades UX only).

| Capability | Criticality | A | B | C | D |
|---|---|---|---|---|---|
| Identity verification (sig check) | **CC** | self-certifying (none) | DID-doc, node-served but self-verifying | self-certifying | DID-doc, domain-hosted but cached/verifiable |
| Message relay / inbox | CV | self-host relays, ≥2 | node inbox (always-on) | relay **or** node, fallback ladder | borrowed from C |
| State hosting | CV | relays/IPFS, pin-incentivized | node repo | any of relay/IPFS/node/file | UCAN-gated, any host |
| Alias / discovery | CV | petnames + self-hosted proofs | DNS handle + directory | petnames + **optional** directory | DNS (domain) |
| Key directory / resolution | CV | DHT (none central) | directory (**mitigated**: mirrorable+`.well-known` fallback) | cascade: cache→directory(opt)→DHT | DID-doc over HTTPS |
| Recovery anchor | CV | social M-of-N only | node recovery key + offline | social **and/or** node | domain control |
| Anti-abuse choke point | CV | PoW + consent (no central) | node rate-limit (easy) | both, per-deployment | domain accountability |

**No candidate makes identity verification depend on a central node** (FR-F4/F5
satisfied across the board). B and D accept central *convenience* dependencies
(directory, domain) and **mitigate** them (mirrorable signed log / `did:key`
fallback); A accepts none; C makes every central component **optional**.

### 6.3 Decision-relevant deltas

| Axis | A (P2P) | B (node) | C (hybrid) | D (caps) |
|---|---|---|---|---|
| Decentralization | ★★★ | ★ | ★★ (tunable) | ★ (DNS) |
| Agent-custody cleanliness (HQ1) | ★★ | ★★★ | ★★★ | ★★★ |
| Rotation/recovery (HQ2) | ★★ (social only) | ★★★ | ★★★ | ★★ (domain) |
| Human-meaningful names (HQ5) | ★ | ★★★ | ★★ | ★★★ |
| Anti-sybil/spam (HQ8) | ★ | ★★★ | ★★ | ★★★ |
| Authority/delegation (HQ11) | ★★ | ★★ | ★★★ | ★★★ |
| Operational simplicity | ★★ (low server, high user) | ★★★ (low user) | ★ (most parts) | ★★ |
| Distance from today's code | far | **nearest** | far (superset) | far |
| Maturity of the blueprint | ★★ | ★★★ | ★★ | ★★ |

**Reading.** **B is the shortest path from today** (doc 02 shows federation is
already a node brokering over a socket) and the most operationally proven, at the
cost of accepting a directory. **A is the purest** on the vision's decentralization
lean but weakest on recovery & anti-sybil. **C is the best fit to the literal
vision** ("lean decentralized, central nodes allowed") and is what the phased
rollout converges to — at the cost of being the largest build. **D wins on
delegation + human names** but bets the identity on DNS. The decision memo (doc 06)
chooses; a likely outcome is **"ship B's node as phase-2, design the wire so C is
reachable, keep D's UCAN layer as the delegation mechanism inside C."**

---

## 7. Resolution of the §4 cross-cutting tensions (doc 03)

Each candidate must *take a side*, not wish the tension away.

| Tension (doc 03 §4) | A | B | C | D |
|---|---|---|---|---|
| **T1** Portability vs non-impersonation | publish `IdentityRecord`+state, **never root** (§1.1); fork-by-default | node migration moves repo, **not** signing power | both mechanisms; fork-by-default | UCAN expiry bounds "portability" of authority |
| **T2** Secure+decentralized vs human names (Zooko) | **secure+decentralized** (petnames only) | **human+secure** (DNS handle + directory) | **user chooses** (raw key default, opt-in alias) | **human+secure** (did:web) |
| **T3** Decentralization vs reliability/UX | accept lower UX for max decentralization | accept central directory for reliability | **tunable per deployment** | accept DNS for UX |
| **T4** Metadata privacy vs spam resistance | PoW/consent; sealed-sender; leak disclosed | node sees graph (leak) but easy anti-spam | scales with centralization chosen | domain sees graph; strong anti-sybil |
| **T5** Easy recovery vs no backdoor | social M-of-N (no backdoor, harder) | node+offline recovery key (override window, not silent) | both; user picks | domain control (= the backdoor risk) |
| **T6** Offline/async vs consistency | single-writer + CRDT; causal order | node = serialization point | single-writer + optional node serialization | DID-doc single source + CRDT |
| **T7** Stable address vs rotatable key | address = genesis pubkey; sigchain rotates underneath | DID stable; keys rotate in doc | both | DID stable; keys rotate in doc |
| **T8** Abstract future state vs usable-today | tagged `payload_kind`, v1 conv-cache today (§1.4b) | same | same | same |
| **T9** Delegated authority vs leaked-agent-key blast radius | scoped signer + revoke link | node-held key + app-passwords (scoped) | **Farcaster signer + UCAN expiry** (smallest blast radius) | **UCAN expiry** (smallest blast radius) |

Every tension is resolved explicitly per candidate (no silent passes).

---

## 8. Hard-question coverage matrix + acceptance checklist

### 8.1 All 12 HQs × 4 candidates (the validator's one-glance check)

| HQ | A | B | C | D |
|---|---|---|---|---|
| **HQ1** custody **(crux)** | §2.1 — host/custodian root, agent signer, fork-default | §3.1 — **node-held signing**, owner recovery key | §4.1 — **Farcaster signer + UCAN**, two-tier | §5.1 — **UCAN-only**, root never signs daily |
| **HQ2** rotation/recovery | §2.2 — sigchain + social M-of-N | §3.2 — rotation+recovery keys (**strongest**) | §4.2 — A's + B's, composable | §5.2 — DID-doc edit + domain |
| **HQ3** transport | §2.4 — relays+gossip+Iroh | §3.4 — node HTTP inbox | §4.4 — fallback ladder | §5.4 — borrowed, capability-gated |
| **HQ4** encryption=ACL + metadata | §2.7 | §3.7 | §4.7 | §5.7 |
| **HQ5** addressing | §2.3 — `wgid:` + petnames | §3.3 — DID + DNS handle | §4.3 — `wgid:` + opt alias | §5.3 — `did:web` |
| **HQ6** decentralization vs central | §6.2 row | §6.2 + §3.9 mitigation | §6.2 (all optional) | §6.2 (DNS root) |
| **HQ7** consistency | §2.8 | §3.8 | §4.8 | §5.8 |
| **HQ8** trust + anti-abuse | §2.6 | §3.6 | §4.6 | §5.6 |
| **HQ9** human vs agent | §2.1 | §3.1 | §4.1 (one type, flags) | §5.1 |
| **HQ10** loadable-state format | §2.5 + §1.4b | §3.5 | §4.5 | §5.5 |
| **HQ11** authority/delegation | scoped signer | app-passwords | **signer+UCAN** | **UCAN (best)** |
| **HQ12** protocol evolution | §1.3 alg-id + §1.5 `WG_FED_COMPAT_VERSION` (all candidates) | ″ | ″ | ″ |

### 8.2 Doc-03 §6 acceptance checklist (does each candidate clear it?)

All four candidates clear every item; the *answer* differs, never its presence:

- [x] **HQ1 custody concretely** — what's published (IdentityRecord+state, never
      root), where the agent signing key lives, load-on-new-host semantics: §2.1 /
      §3.1 / §4.1 / §5.1.
- [x] **Addressing + verifiable aliases** (HQ5): §2.3 / §3.3 / §4.3 / §5.3.
- [x] **Transport, email-speed, no mandatory single relay** (HQ3): §2.4 / §3.4 /
      §4.4 / §5.4; FR-F4 confirmed in §6.2.
- [x] **Encryption=ACL + metadata stance** (HQ4): §2.7 / §3.7 / §4.7 / §5.7.
- [x] **Rotation/recovery + revocation** (HQ2): §2.2 / §3.2 / §4.2 / §5.2.
- [x] **Per-capability central table, no critical central dep** (HQ6): §6.2.
- [x] **Consistency model** (HQ7): §2.8 / §3.8 / §4.8 / §5.8.
- [x] **Trust + anti-abuse** (HQ8): §2.6 / §3.6 / §4.6 / §5.6.
- [x] **Human vs agent custody/recovery/authority** (HQ9/HQ11): §2.1/§4.1 + the
      HQ11 column in §8.1.
- [x] **Versioned loadable-state interface + compat handshake** (HQ10/HQ12):
      §1.4b + §1.3/§1.5.
- [x] **Phased rollout on today's WG** (NFR-6): §9.
- [x] **Each §4 tension resolved** (T1–T9): §7.
- [x] **Inside §5 non-goals** — §10.

**MUST-requirement honesty (doc 03 §6 closing rule).** One MUST is *partially*
deferred and stated openly: **FR-T2 (sybil/spam resistance)** is strong in B/D
(node/domain choke points), **weaker in A** (PoW + consent only, no anchor) and
**tunable in C**. This is inherent to maximal decentralization and is flagged for
the adversarial pass (doc 05), not silently met. No other MUST is unmet.

---

## 9. Migration phasing (common path; candidate choice is a late binding)

Doc 02 §6 names the two real extension hooks — `federation.yaml` peer addressing
(swap path→key) and `AccessPolicy`+`shared_peers` (the encrypted-ACL hook). The
phasing exploits both and keeps every phase independently valuable (NFR-6, FR-F6):

- **Phase 0 — Keys, no network (local, ships first).** Add `src/identity/` +
  crypto crates. `wg identity new` mints an ed25519 root into `wg secret`
  (`src/secret.rs`), writes a genesis sigchain. `Message` gains optional `sig`
  (`#[serde(default)]`, old readers ignore it — doc 02 §2.3). *Value:* signed,
  tamper-evident local messages and a real identity primitive where today there is
  none. **No candidate chosen yet.**
- **Phase 1 — Cross-graph addressing.** `federation.yaml` `PeerConfig`/`Remote`
  (`federation.rs:21/:31`) gain `wgid`; `resolve_peer` (`:301`) resolves
  key→endpoint; `wg msg --to wgid:...` works between two graphs over **one
  transport** (pick the cheapest: a relay for A/C, the daemon-over-HTTP for B).
  *Value:* first real cross-WG signed message. **A vs B vs C diverges here only in
  which transport adapter ships first.**
- **Phase 2 — Loadable signed state + recovery.** `StateSnapshot` publish/fetch;
  sigchain `add_key`/`revoke`/recovery. `transfer()` (`:637`) handles signed
  artifacts. *Value:* portable identity (V2) + key recovery (V6).
- **Phase 3 — Encryption=ACL + the optional central pieces.** Per-recipient
  envelopes realize `AccessPolicy` (the `federation.rs:627` hook); add the
  *optional* directory/node (→ B or C) or the UCAN layer (→ D). *Value:*
  confidentiality + discovery/availability; **this is where the candidate
  identity finally crystallizes** — and a system that stopped at Phase 2 is
  A-shaped, one that adds the node is B/C-shaped.

The order is deliberately **candidate-agnostic through Phase 2**: WG can defer the
A/B/C/D choice until the wire and key model are proven, which is exactly the
"don't pick the final stack prematurely" stance of doc 03 §5 non-goal 9.

---

## 10. Non-goals honored (doc 03 §5)

All four candidates stay inside the stated boundaries: **email-speed only** (no
RTC); **no blockchain/token/global ledger** (sigchains are per-identity, not a
global chain — even D's transparency log is append-only audit, not consensus);
**audited primitives only** (§1.3, no homemade crypto); **aliases are optional,
not a new root namespace**; **no mixnet-grade anonymity** (metadata leak disclosed,
not eliminated); **substrate not product** (no feeds/ranking UX); **opaque
hidden-state portability is a *slot* (`payload_kind`), not a solved payload**; the
existing agency-primitive `transfer()` is the migration substrate, not the
redesign target; and **the final library/wire stack is proposed, not mandated** —
that binding is doc 06's call.

---

## 11. Handoff to doc 05 (adversarial) and doc 06 (decision)

- **For `fed-adversarial` (5/6):** attack, in priority order — (1) **HQ1 custody**
  in each candidate: can a malicious host that runs an agent exfiltrate or abuse
  the signer beyond its scope? Does fork-vs-same-self actually hold under a
  hostile `add_key`? (2) **A's anti-sybil** (FR-T2 partial deferral above). (3)
  **B's directory and D's domain** as central dependencies — censorship, seizure,
  equivocation. (4) **Recovery as a backdoor** (T5) — does M-of-N social recovery
  (A) or the node recovery key (B) enable theft of a *live* identity? (5)
  **Metadata** leak surface per candidate (§§2.7/3.7/4.7/5.7).
- **For `fed-decision` (6/6):** the likely synthesis is **C as the north-star
  wire, reached by B's pragmatic phase-2 node, with D's UCAN as the in-C
  delegation mechanism** — but that is doc 06's decision to make against the
  doc-03 requirements and the doc-05 threat findings, not a foregone conclusion
  here. The phasing in §9 is built so any of the four remains reachable.

---

*Wave-1 generate phase complete. Four candidates spanning decentralized ↔
central-node, each answering all twelve hard questions concretely with explicit
agent-key-custody, rotation/recovery, and transport, each mapped to concrete WG
code changes (`src/identity/` + `federation.rs`/`messages.rs`/`Agent`/`secret.rs`)
and a shared migration path from doc 02's current state, with tradeoffs explicit
per candidate.*
