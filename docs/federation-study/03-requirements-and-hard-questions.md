# Federation Study 3/6 — Requirements & Hard-Questions Catalog

> **Headline federation study, wave 1, task 3 of 6 (gather phase).**
> This is the **"what must be true"** spec. The candidate architectures
> (task 4, `fed-architectures`) are judged against the requirements here; the
> adversarial pass (task 5, `fed-adversarial`) attacks the answers proposed for
> the hard questions; the decision memo (task 6, `fed-decision`) picks and
> roadmaps. Prior-art (task 1, `fed-prior-art`) and current-state baseline
> (task 2, `fed-baseline`) feed in alongside this document — it is written to
> stand on its own from the vision, not to depend on them.

**Status:** draft for evaluation · **Date:** 2026-06-24 · **Owner task:** `fed-requirements`

---

## 0. How to read this document

Three artifacts, in order of decreasing stability:

1. **§2 Requirements** — the durable contract. Numbered `FR-*` (functional) and
   `NFR-*` (non-functional), each with **MUST / SHOULD / MAY** force (RFC 2119),
   a **trace** back to the vision (or the gap analysis), a one-line rationale,
   and an **acceptance signal** (how task 4/5 can tell it was met). These should
   change slowly.
2. **§3 Hard-Questions catalog** — the unresolved design forks. Twelve questions
   (`HQ1`…`HQ12`), each with **why it's hard**, the **decision axes** (the
   spectrum of real choices), and **success criteria a good answer must
   satisfy**. These are *open*; task 4 proposes answers, task 5 stress-tests
   them, task 6 decides.
3. **§4 cross-cutting tensions**, **§5 non-goals**, **§6 architecture
   acceptance checklist**, **§7 traceability matrix** — the connective tissue
   that makes the above usable downstream.

**Traceability convention.** Every requirement cites at least one source:
- **Vision pillars** `V1…V7` (defined in §1) — the north star from the
  social-network vision memo / Erik's 2026-06-24 model.
- **Gap-analysis reqs** `R*` — from the private `poietic-pbc/poietic-family-team`
  gap analysis (38 reqs R1–R38, 2026-04-30). Only the IDs confirmed in the
  vision memo are cited by number (**R2** loadable memory, **R24** per-artifact
  ACLs, **R16/R19** multi-bot messaging); others are cited as "(gap-analysis)"
  because the exact R# lives in the source repo and must not be invented.
- **Current code** — where a requirement is shaped by what exists today
  (`src/federation.rs`, `docs/ADR-actor-vs-agent-identity.md`, `wg msg`, `wg
  secret`). Detailed baselining is task 2's job; cited here only for grounding.

---

## 1. The north star, restated as named pillars

These are the load-bearing claims of the vision, named so requirements can trace
to them. (Source: WG social-network vision; Erik's federation model, 2026-06-24.)

| ID | Pillar | One-line statement |
|----|--------|--------------------|
| **V1** | **Long-lived loadable identity** | An identity persists across sessions and is *restored from loadable state* — cached conversations → summaries → eventually opaque hidden/RNN state — so an agent resumes a continuous self rather than starting fresh each task. (R2) |
| **V2** | **Portable / downloadable identity** | Identities are cached, published on the web, and fetchable from anywhere; an identity can move hosts or be reconstituted elsewhere. |
| **V3** | **Async email-speed messaging** | Store-and-forward message queue between humans and agents at "speed of email / speed of work" — explicitly *not* real-time. (builds on `wg msg`; R16/R19) |
| **V4** | **Cryptographic key-based P2P federation** | Cross-WG collaboration is cryptographically secure and Keybase/Nostr-like: each human/agent holds a keypair; the **public key IS the identity AND a URL-like address** (self-certifying, no mandatory central registry); messages are signed events; loadable state is a signed, content-addressed artifact bound to the key. |
| **V5** | **Decentralization-leaning, central nodes allowed** | Lean decentralized/P2P, but permit central relays/registries/super-nodes where they buy reliability or UX. An explicit, deliberate tradeoff — not an ideological purity test. |
| **V6** | **Long-term reliability** | The system must be dependable "for the long term": durable identities, recoverable from key loss, forward/backward-compatible wire and state formats, no single point of permanent failure. |
| **V7** | **Hybrid: humans and agents are both first-class** | The network mixes humans and AI agents as peers. Identity, messaging, and federation must serve both — while respecting that they differ in custody, recovery, and authority. |

**The crux, called out up front.** The vision names one problem as the hardest:
**agent key custody** (§3, **HQ1**). A portable/downloadable identity must equal
*public identity + signed state*, **never the signing key** — otherwise
"download Nora's identity" becomes "impersonate Nora." Every requirement and
question below is arranged so this crux stays visible; it is the single decision
most likely to make or break the architecture.

---

## 2. Requirements

Force: **MUST** (architecture is wrong without it) · **SHOULD** (strongly
preferred; deviation needs justification) · **MAY** (permitted, optional).

### 2.A Identity & loadable state

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-I1** | MUST | An identity is named by, and verifiable against, a **public key** (self-certifying). Anyone holding the public address can verify authorship of that identity's artifacts without consulting a central authority. | V4 | *Accept:* given an artifact + address, verification is a local signature check; no network call to a trusted third party is required. |
| **FR-I2** | MUST | A **portable identity record = public identity + signed state**, and **excludes the private signing key**. Downloading/publishing an identity MUST NOT transfer the ability to author new signed events as that identity. | V2, V4 (**HQ1**) | *Accept:* the published artifact, fed to an honest client, lets you *read & verify* Nora's state but not *sign as* Nora. |
| **FR-I3** | MUST | **Loadable state** has a **versioned, model-agnostic format** that abstracts over today's conversation cache, intermediate summaries, and tomorrow's opaque hidden/RNN state. The interface is stable; the payload kind is a tagged, evolvable field. | V1, R2 (**HQ10**) | *Accept:* a v1 (conversation-cache) and a hypothetical v2 (opaque-blob) state load through the *same* interface; an old client degrades gracefully on an unknown payload kind. |
| **FR-I4** | MUST | Loadable state is **content-addressed and signed by the identity key**, so its integrity and provenance are verifiable independent of where it was fetched. | V2, V4 | *Accept:* tampering with a fetched state blob is detected by hash mismatch and/or signature failure. |
| **FR-I5** | SHOULD | Loading state on a new host yields a **defined continuity semantics**: it is explicit whether the result continues the *same* sigchain (one self, new device) or *forks* a new lineage (a copy/child). The default and the mechanism to choose are specified. | V1, V2 (**HQ1**) | *Accept:* "load Nora on host B" has a documented, predictable answer to "is this still Nora?". |
| **FR-I6** | SHOULD | Identity records carry the **operational fields WG already unifies onto `Agent`** (role, motivation, capabilities, trust level, contact, executor kind) so federated identities remain usable for task matching and dispatch. | V7; `ADR-actor-vs-agent-identity` | *Accept:* a pulled federated identity can be assigned work without a schema mismatch. |
| **FR-I7** | MUST | The format distinguishes the **stable public identity** (long-lived address) from its **mutable state snapshots** (many over time), so identity ≠ any single state version. | V1, V2 | *Accept:* an identity survives unbounded state updates without its address changing. |

### 2.B Messaging & transport

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-M1** | MUST | Messages are **signed events** authored by an identity key; any recipient can verify sender authenticity and message integrity offline. | V3, V4 | *Accept:* a forged "from Nora" message fails signature verification. |
| **FR-M2** | MUST | Delivery is **asynchronous store-and-forward** ("email speed"): sender and recipient need not be online simultaneously; a message persists until fetched. | V3 | *Accept:* send while recipient offline → recipient gets it on next poll/connect. |
| **FR-M3** | MUST | Messaging works **across independently-owned WGs** (cross-graph, cross-host), not only within a single local graph as `wg msg` does today. | V3, V4; `wg msg` baseline | *Accept:* an agent in WG-A messages an agent in WG-B addressed only by public key. |
| **FR-M4** | SHOULD | Messaging supports **both humans and agents** as senders/recipients, bridging existing human channels (e.g. the multi-bot Telegram path) without forcing a single transport. | V3, V7; R16/R19 | *Accept:* a human via a bridge and an agent via native transport can exchange the same conversation. |
| **FR-M5** | SHOULD | Messages carry enough structure to **track ongoing collaboration** (threading/causality, references to tasks/artifacts) — "speed of work," not just free text. | V3 | *Accept:* a reply can be unambiguously attributed to its parent and to a task/artifact. |
| **FR-M6** | MAY | At-least-once delivery with **idempotent, deduplicable** message IDs; ordering is per-conversation causal, not global. | V3, V6 (**HQ7**) | *Accept:* a redelivered message is recognized as a duplicate, not double-applied. |

### 2.C Federation, addressing & decentralization

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-F1** | MUST | The **address scheme is the public key** (or a stable derivation of it), usable as a **URL-like locator** to fetch an identity, its state, and its inbox. | V4 (**HQ5**) | *Accept:* given only `wg://<pubkey>` (or did:key/npub equivalent), a client can resolve & fetch. |
| **FR-F2** | SHOULD | **Human-friendly aliases** (`@nora`) MAY layer on top, but MUST be **verifiable back to the key** and MUST NOT become a mandatory central naming authority. Loss/abuse of an alias never compromises the underlying key identity. | V4, V5 (**HQ5**, **HQ8**) | *Accept:* `@nora` resolves to a pubkey via a proof the client can check; the system still works with raw keys if the alias layer is gone. |
| **FR-F3** | MUST | Federation is **opt-in and peer-scoped**: a WG node chooses which peers/relays it federates with; there is no requirement to join a global namespace. | V4, V5 | *Accept:* two nodes can federate privately without registering anywhere global. |
| **FR-F4** | SHOULD | The design **leans decentralized/P2P but explicitly permits central nodes** (relays, registries, super-peers) as performance/reliability/UX aids — never as mandatory roots of trust. | V5 (**HQ6**) | *Accept:* removing any single central node degrades convenience but not correctness or security. |
| **FR-F5** | SHOULD | **No single point of permanent failure** for identity or message recovery: an identity and its already-published state remain recoverable even if the node that created it is gone. | V6 | *Accept:* kill the origin host → the identity is still verifiable and its published state still fetchable from a relay/cache/peer. |
| **FR-F6** | MAY | Reuse existing WG federation plumbing (`src/federation.rs` named remotes/peers, `WG_AGENCY_COMPAT_VERSION` handshake) as a migration substrate, extending it from agency-primitive transfer to identity/message transfer. | current code; V6 | *Accept:* the new layer interops with, or cleanly supersedes, today's `federation.yaml` remotes/peers. |

### 2.D Security, keys & privacy

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-S1** | MUST | **Private signing keys never leave their custodian** in normal operation: not embedded in portable identity records, not in published state, not in relayed messages. | V2, V4 (**HQ1**) | *Accept:* static analysis / format spec guarantees no field can carry a private key; a published bundle contains none. |
| **FR-S2** | MUST | The architecture defines a **key rotation & recovery** story so that key loss/compromise does not necessarily mean permanent identity loss (e.g. sigchain succession, device keys, or social/threshold recovery). | V6 (**HQ2**) | *Accept:* there is a documented procedure by which a user who lost device A continues as the same identity from device B. |
| **FR-S3** | MUST | **Confidentiality is per-recipient encryption**, and this *is* the access-control layer: an artifact/message is readable only by keys it was encrypted to. Per-recipient encryption realizes per-artifact ACLs. | R24 (**HQ4**) | *Accept:* an artifact encrypted to {A,B} is unreadable by C even if C obtains the ciphertext from a relay. |
| **FR-S4** | SHOULD | **Metadata privacy** is a stated, bounded goal: the design names what relays/peers can and cannot learn (sender, recipient, timing, size) and what protections (if any) apply. | R24 (**HQ4**) | *Accept:* a threat-model paragraph enumerates the metadata leak surface and the chosen stance. |
| **FR-S5** | MUST | The system **composes standard, audited crypto primitives** (signatures, AEAD, KDF, hashes) — it does **not** invent its own cryptography. | V6 | *Accept:* every primitive maps to a named, published, widely-reviewed construction. |
| **FR-S6** | SHOULD | **Agent vs human key custody differs by design**: agents' keys live host-held / HSM-style under a custodian, humans' keys may use device + recovery models. The differences are explicit, not accidental. | V7 (**HQ1**, **HQ9**) | *Accept:* the doc states where an *agent's* signing key lives and who controls it, distinctly from a human's. |
| **FR-S7** | SHOULD | **Revocation** is possible and verifiable: a compromised/retired key can be marked dead so honest clients stop trusting new signatures from it. | V6 (**HQ2**, **HQ12**) | *Accept:* after revocation, a new signature from the dead key is rejected by an updated client. |

### 2.E Trust, anti-abuse & governance

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-T1** | MUST | A **trust-establishment mechanism** exists for "is this key who I think it is" — web-of-trust, cryptographic proofs (Keybase-style social proofs), and/or TOFU — without a mandatory global CA. | V4 (**HQ8**) | *Accept:* a first contact has a defined verification path; trust state is recorded and re-checkable. |
| **FR-T2** | SHOULD | **Sybil / spam resistance** for messaging and federation: cheap-to-create keys do not translate to cheap-to-spam inboxes or cheap-to-poison trust. | V3, V6 (**HQ8**) | *Accept:* an unknown key cannot flood a recipient's inbox without a cost/consent gate. |
| **FR-T3** | SHOULD | The architecture interoperates with WG's existing **`trust_level`** field on the unified `Agent` so federated trust feeds local permission/dispatch gating. | `ADR-actor-vs-agent-identity`; V7 | *Accept:* a federated identity's trust state can gate what tasks it may be assigned. |
| **FR-T4** | MAY | **Authority & delegation** are representable: an agent acting *on behalf of* a human (or another agent) can be expressed and verified, and revoked. | V7 (**HQ11**) | *Accept:* "agent X may act for human Y until T" is a checkable, revocable credential. |

### 2.F Non-functional requirements

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **NFR-1 Reliability** | MUST | Durable identities and store-and-forward delivery survive node restarts, transient outages, and origin-host loss (see FR-F5). | V6 | *Accept:* chaos test — restart/kill nodes mid-flight; no identity or accepted message is lost. |
| **NFR-2 Latency budget** | SHOULD | Target **"email speed"** (seconds-to-minutes acceptable), *not* real-time. This is a deliberate *relaxation* that buys decentralization and offline tolerance. | V3, V5 | *Accept:* design choices are not penalized for non-real-time latency; nothing assumes sub-second delivery. |
| **NFR-3 Portability** | MUST | Identity & state formats are **self-describing and transport-independent** — publishable to "the web," a relay, IPFS-like storage, or a file, and fetchable from anywhere. | V2 | *Accept:* the same bundle round-trips through ≥2 unrelated transports. |
| **NFR-4 Evolvability** | MUST | **Versioned, forward/backward-compatible** wire and state formats with an explicit compat-handshake (mirroring `WG_AGENCY_COMPAT_VERSION`). Old and new peers degrade gracefully. | V6 (**HQ10**, **HQ12**) | *Accept:* a vN+1 peer and a vN peer still exchange the subset they share. |
| **NFR-5 Operability** | SHOULD | A node is **self-hostable with modest resources**; running your own relay/registry is feasible for an individual, preserving the decentralization option. | V5 | *Accept:* a single person can stand up a working node on commodity hardware. |
| **NFR-6 Incrementality** | SHOULD | The design admits a **phased rollout** on top of today's WG (single-graph `wg msg`, `federation.rs` primitive transfer, `wg secret` keystore) rather than a big-bang rewrite. | V6; current code | *Accept:* a v0 milestone delivers value without the full P2P stack. |
| **NFR-7 Auditability** | SHOULD | Security-relevant events (key rotation, revocation, trust changes, delegation) are **append-only and inspectable** (sigchain-like), so history is verifiable after the fact. | V4, V6 | *Accept:* an auditor can replay an identity's key history and detect tampering. |

---

## 3. Hard-questions catalog

Twelve questions (the ten from the brief, plus **HQ11 authority/delegation** and
**HQ12 protocol evolution**, which the requirements above surfaced as
genuinely-distinct forks). Each is *open* — task 4 proposes, task 5 attacks, task
6 decides. Format: **why hard · decision axes · success criteria**.

### HQ1 — Agent key custody **(THE CRUX — load-bearing)**

> *Where does an agent's signing key live, and what exactly moves when you
> "download Nora's identity"?*

- **Why hard.** The vision wants identities to be **portable/downloadable**
  (V2) *and* **cryptographically un-impersonatable** (V4). Those pull opposite
  directions: the more you can move an identity, the closer you get to moving its
  signing power. Agents have no fingers, phones, or hardware tokens — the human
  custody patterns (device key + biometric) don't map. And agents are spun up,
  cloned, and resumed on arbitrary hosts, so "the key lives on the user's
  device" has no agent analogue. Getting this wrong silently turns a feature
  (publish your agent) into a catastrophe (anyone runs *as* your agent).
- **Decision axes.**
  - *What is published:* public identity + signed state only (FR-I2/FR-S1) ↔
    (rejected) the keypair itself.
  - *Where the signing key lives:* host-held file ↔ OS keychain / `wg secret`
    keystore ↔ HSM/TPM/enclave ↔ remote signing service (key never on the
    worker host; worker requests signatures) ↔ threshold/MPC split.
  - *Load-on-new-host semantics:* same sigchain (new authorized device key) ↔
    fork into a new lineage (a verifiable *copy/child* of Nora, not Nora).
  - *Who is the custodian:* the human owner ↔ the WG node operator ↔ a
    delegated signing service.
- **Success criteria.** (1) Publishing/downloading an identity provably cannot
  let the downloader author new events as it (FR-I2, FR-S1). (2) There is a
  precise, defensible answer to "load Nora on host B → same self or fork?"
  (FR-I5). (3) The agent-specific custody model is stated explicitly and differs
  from the human one where it should (FR-S6). (4) A signing-service / remote-sign
  option is evaluated, since it may be the only way to have both portability and
  non-extractable agent keys. (5) The failure mode "key leaked" degrades to
  HQ2's recovery, not to permanent takeover.

### HQ2 — Key rotation & recovery

> *Does losing your key mean losing your identity?*

- **Why hard.** Self-certifying identity (address = key) means the key *is* the
  identity — so naïvely, key loss = identity death, violating long-term
  reliability (V6). Recovery mechanisms that are too easy become impersonation
  backdoors (re-opening HQ1); too hard and real users are locked out forever.
  For agents, "lost key" can happen on any ephemeral host crash.
- **Decision axes.** Sigchain succession (Keybase-style: old key signs the next)
  ↔ device-key sets (multiple keys, any can act) ↔ social recovery / threshold
  (M-of-N guardians) ↔ deterministic re-derivation from a seed ↔ accept-loss
  (immutable identity, lost = new identity + migration of reputation). Stable
  address vs rotating key: is the address the *first* key, a *seed-derived*
  constant, or a *human alias* decoupled from the rotating key?
- **Success criteria.** A documented, demonstrable procedure to continue as the
  same identity from a new key after loss (FR-S2); recovery cannot be abused to
  steal a live identity (ties to HQ1/HQ8); revocation of the old key is verifiable
  (FR-S7, HQ12); the chosen "what stays stable across rotation" is explicit and
  consistent with the addressing scheme (HQ5).

### HQ3 — Transport: relays/store-and-forward vs true P2P

> *How do bytes actually move between two WGs that may both be offline?*

- **Why hard.** "Email speed" (V3) + offline tolerance + decentralization-leaning
  (V5) constrain the transport, but real P2P (NAT traversal, DHT, libp2p) is
  operationally heavy, while pure relays reintroduce semi-central nodes (V5
  tension). Most peers are behind NATs and not always-on, which is exactly why
  email uses relays.
- **Decision axes.** Relay/store-and-forward (Nostr-style relays, SMTP-like
  MX) ↔ true P2P (libp2p + DHT + hole-punching) ↔ hybrid (P2P when both online,
  relay fallback otherwise). Number/operator of relays (self-host vs shared).
  Pull (poll inbox) vs push (subscriptions). Storage duration & who pays for it.
- **Success criteria.** Meets the email-speed budget (NFR-2) with both ends
  possibly offline (FR-M2); no mandatory single relay/root (FR-F4, FR-F5);
  operability for self-hosters (NFR-5); a clear story for NAT/availability;
  bytes are signed/encrypted end-to-end so the transport is untrusted (FR-M1,
  FR-S3).

### HQ4 — Encryption as the privacy/ACL layer

> *How do per-recipient encryption and per-artifact access control become one
> mechanism — and what leaks anyway?*

- **Why hard.** Collapsing ACLs into encryption (R24) is elegant but unforgiving:
  there's no "server-side permission check" fallback once data is on untrusted
  relays — if you encrypt to the wrong set, or can't revoke a recipient's past
  access, that's permanent. Group membership changes, forward secrecy, and
  multi-device all complicate per-recipient encryption. And encryption hides
  *content* but not *metadata* (who talks to whom, when) — which on a relay
  network can be the more sensitive leak.
- **Decision axes.** Per-recipient envelope encryption ↔ group keys (with
  rekey-on-membership-change) ↔ ratcheting (Signal/MLS-style forward secrecy).
  Revocation = cannot-read-future vs cannot-read-past (the latter is generally
  impossible once ciphertext is out). Metadata stance: none ↔ relay-blind
  addressing ↔ sealed-sender ↔ mixnet (almost certainly out of scope, §5).
- **Success criteria.** Encrypting-to-recipients *is* the ACL (FR-S3) with a
  defined group/membership-change model; the metadata leak surface is explicitly
  enumerated and a stance chosen (FR-S4); forward-secrecy and multi-device
  positions are stated; nothing assumes a trusted server for access decisions.

### HQ5 — Addressing scheme

> *What is an address — `did:key`, `npub`, `wg://<pubkey>` — and how do
> human-friendly names attach without a central authority?*

- **Why hard.** **Zooko's triangle**: names want to be *secure*,
  *decentralized*, and *human-meaningful* — pick ~two. The key-as-address gives
  secure + decentralized but not memorable; `@nora` gives human-meaningful but
  reintroduces a naming authority and name disputes/squatting. The address also
  has to *resolve* to fetchable endpoints (state, inbox), so it's a locator, not
  just an identifier.
- **Decision axes.** Encoding: `did:key` ↔ Nostr `npub` ↔ custom `wg://<pubkey>`
  ↔ multibase/multicodec. Resolution: pure self-resolving (key → endpoints via a
  signed record) ↔ DHT lookup ↔ relay hint ↔ DNS/`.well-known`. Alias layer:
  none ↔ petnames (local, per-user) ↔ verified global aliases (proofs) ↔ a
  registry (central node, V5). Whether the address is the raw key or a
  rotation-stable derivation (ties to HQ2).
- **Success criteria.** A pubkey alone resolves to identity + state + inbox
  (FR-F1); aliases are verifiable back to the key and never mandatory/central
  (FR-F2); the scheme is stable under key rotation (HQ2) or explicitly is not;
  consistency with at least one prior-art format (task 1) for interop.

### HQ6 — Decentralization vs central nodes

> *Where, deliberately, do we allow a central node — and what must never depend
> on one?*

- **Why hard.** The vision says lean decentralized **but central nodes allowed**
  (V5) — that's an invitation to make the *wrong* things central by accident
  (the way a bare `openrouter:` once silently routed to a keyless handler). Every
  convenience (discovery, aliases, relays, search) is easier centralized; every
  centralization is a future single point of failure, censorship, or capture.
- **Decision axes.** For each capability — message relay, identity/state
  hosting, alias registry, discovery/search, key directory — choose
  decentralized ↔ federated-central ↔ fully-central, and mark it *correctness-
  critical* vs *convenience-only*. Trust root: none (self-certifying) ↔
  optional anchors ↔ required anchors.
- **Success criteria.** An explicit per-capability table of what is centralized
  and why; **no correctness- or security-critical capability depends on a single
  central node** (FR-F4, FR-F5); central nodes are *aids* whose loss degrades UX
  only; the decentralization option (self-host everything) remains real (NFR-5).

### HQ7 — Consistency model

> *Two hosts edit shared state (a profile, a thread, a task) concurrently — what
> is correct?*

- **Why hard.** Decentralized + offline + async (V3, V5) rules out a single
  authoritative writer, so concurrent divergent updates are the *normal* case,
  not an error. Identity state that can fork (HQ1) makes "which state is current"
  genuinely ambiguous. Yet users expect *some* convergence and a sane merge.
- **Decision axes.** Eventual consistency with CRDTs (auto-merge) ↔ last-writer-
  wins (lossy, simple) ↔ explicit version vectors + manual conflict surfacing ↔
  single-writer-per-object (the author's key is authoritative for *its own*
  state, sidestepping most conflicts). Causal vs total ordering (FR-M6). What is
  even shared-writable vs single-author.
- **Success criteria.** Concurrent updates converge to a defined, deterministic
  result (or conflicts are surfaced, never silently lost); ordering semantics are
  specified per object type; the model is consistent with the fork/continuity
  decision from HQ1; "my own identity state" has a clear single-writer story.

### HQ8 — Trust establishment & anti-abuse

> *On first contact with a key, how do I know it's who I think — and how do I
> keep cheap keys from flooding/poisoning the network?*

- **Why hard.** Self-certifying keys prove *consistency* (same author over time)
  but not *binding to a real person/org* on first contact — that's a separate,
  social problem. Keys are free, so sybil attacks (fake identities) and spam
  (unsolicited inbox flooding) are cheap; but any anti-sybil cost or gatekeeper
  risks re-centralizing (V5) or excluding legitimate newcomers. Web-of-trust has
  well-known scaling/bootstrapping problems.
- **Decision axes.** Verification: TOFU (trust first sight, pin thereafter) ↔
  Keybase-style cryptographic social proofs (control of a Twitter/GitHub/DNS
  handle) ↔ web-of-trust signatures ↔ out-of-band fingerprint compare. Anti-
  abuse: contact-request/consent gates ↔ proof-of-work / stake ↔
  allow/block-lists ↔ reputation/trust-level (FR-T3) ↔ rate limits at relays.
- **Success criteria.** A defined first-contact verification path with
  recorded, re-checkable trust state (FR-T1); an inbox cannot be cheaply flooded
  by unknown keys (FR-T2); anti-abuse measures don't require a central authority
  or exclude honest newcomers; integration with WG `trust_level` gating (FR-T3).

### HQ9 — Human vs agent identity

> *How do custody, recovery, and authority differ between a person and an AI
> agent?*

- **Why hard.** The vision insists both are first-class (V7), and WG already
  *unified* them onto one `Agent` struct (the actor/agent ADR). But they differ
  where it matters most: humans can hold a device key and do social recovery;
  agents can't, are cloned/resumed/ephemeral, and often act *for* a human. Treat
  them identically and you get either insecure humans or unusable agents.
- **Decision axes.** One identity type with capability flags (current WG
  direction) ↔ two related types. Agent key custody: host-held ↔ custodian-held
  ↔ remote-signing (HQ1). Recovery: agents fall back to their custodian/owner
  vs independent recovery. Authority: does an agent have standalone authority or
  always derive it from a human principal (→ HQ11)? Lifecycle: agents are
  created/retired far more often than humans.
- **Success criteria.** The model serves both within one coherent scheme
  (FR-I6); agent custody/recovery is explicitly *delegated to a custodian* where
  human custody is *self-held* (FR-S6); the differences are by-design and
  documented, not emergent; no human-only assumption (biometric, phone) is baked
  into a place agents must pass through.

### HQ10 — Loadable-state format

> *How do we define a state format that works for today's conversation cache and
> tomorrow's opaque hidden/RNN state?*

- **Why hard.** Today "state" = readable cached conversations / summaries;
  tomorrow it may be an opaque multi-gigabyte tensor blob meaningful only to one
  model version (V1, R2). A format that hard-codes today's shape will need a
  rewrite; one too abstract is useless now. State also has to be signable,
  content-addressed (FR-I4), diffable/incremental (you don't re-publish
  everything per turn), and possibly encrypted (HQ4).
- **Decision axes.** Payload model: structured/transparent (JSON conversation
  log) ↔ opaque tagged blob ↔ layered (transparent summary + opaque detail).
  Versioning: single envelope + tagged payload-kind ↔ per-kind schemas.
  Granularity: full snapshots ↔ incremental deltas/event-log ↔ checkpoints +
  deltas. Model-binding: state tagged with the model/version that can interpret
  it; behavior on a model mismatch.
- **Success criteria.** One stable interface loads v1 (conversation) and a
  hypothetical v2 (opaque) payload (FR-I3); unknown payload-kinds degrade
  gracefully (NFR-4); state is signed + content-addressed (FR-I4); incremental
  update is possible (you can publish a new turn without re-uploading history);
  model-binding metadata is present so a wrong-model load is detected, not
  silently corrupt.

### HQ11 — Authority, delegation & accountability

> *When an agent acts "for" a human, whose authority is it exercising — and who
> is accountable?*

- **Why hard.** In a hybrid network (V7) agents routinely act on behalf of
  humans (and other agents). If an agent's signature carries the *human's*
  authority, key custody (HQ1) becomes a delegation problem; if it carries only
  the *agent's*, you need a verifiable, revocable link from agent to principal.
  Over-broad delegation is dangerous (a leaked agent key acts as the human);
  too-narrow is unusable.
- **Decision axes.** Delegation form: capability certificate (human key signs
  "agent X may do Y until T") ↔ shared key (rejected — collapses HQ1) ↔
  attestation records. Scope: blanket ↔ per-capability ↔ per-task. Revocation:
  expiry ↔ explicit revoke (FR-S7). Accountability: signatures attributable to
  both agent and principal vs agent-only.
- **Success criteria.** "Agent X may act for human Y, scope S, until T" is a
  checkable, revocable, expiring credential (FR-T4); delegation never requires
  sharing a private key (FR-S1); actions remain attributable for audit (NFR-7);
  revoking delegation immediately stops accepted future actions.

### HQ12 — Protocol evolution & long-term compatibility

> *How do identity, message, and state formats evolve over years without
> orphaning old identities or splitting the network?*

- **Why hard.** "Reliable for the long term" (V6) means formats outlive any one
  model, client version, or crypto choice. Crypto primitives get deprecated
  (today's signature scheme may be broken later); old identities published years
  ago must still verify; a vN+1 client must coexist with vN peers. WG already
  feels this with `WG_AGENCY_COMPAT_VERSION` / `WG_PI_PLUGIN_COMPAT_VERSION`
  handshakes and loud-failure-on-mismatch.
- **Decision axes.** Versioning: explicit envelope version + capability
  negotiation ↔ implicit/sniffed. Crypto agility: algorithm IDs + multi-sig
  migration ↔ fixed suite. Failure stance: loud-fail-on-mismatch (WG's current
  convention) ↔ best-effort-degrade. Governance of the spec: BDFL ↔ informal ↔
  RFC-like process.
- **Success criteria.** vN and vN+1 peers exchange their shared subset (NFR-4);
  a crypto primitive can be migrated without abandoning existing identities
  (crypto agility); old signed artifacts remain verifiable or are explicitly,
  loudly deprecated; the compat handshake mirrors WG's existing
  `*_COMPAT_VERSION` convention so mismatches fail loudly, not silently.

---

## 4. Cross-cutting tensions (where requirements pull against each other)

These are the conflicts task 4 must *resolve*, not wish away. Each is a real
tradeoff, not a bug.

| # | Tension | Pulls | Where decided |
|---|---------|-------|---------------|
| T1 | **Portability vs non-impersonation** | V2 (download identity) vs FR-I2/FR-S1 (can't ship the key) | HQ1 |
| T2 | **Secure + decentralized vs human-friendly names** (Zooko) | FR-F1 (key address) vs FR-F2 (`@nora`) | HQ5, HQ8 |
| T3 | **Decentralization vs reliability/UX** | V5/FR-F4 vs convenience of central relays/registries | HQ3, HQ6 |
| T4 | **Metadata privacy vs spam resistance / discovery** | FR-S4 (hide who-talks-to-whom) vs FR-T2 (gate unknown senders) & discovery | HQ4, HQ8 |
| T5 | **Easy recovery vs no impersonation backdoor** | FR-S2 (recover after loss) vs HQ1 (recovery ≠ takeover) | HQ2 |
| T6 | **Offline/async + decentralized vs consistency** | V3/V5 vs FR-M6/strong consistency | HQ7 |
| T7 | **Stable address vs rotatable key** | FR-F1/FR-I7 (durable address) vs FR-S2 (rotate keys) | HQ2, HQ5 |
| T8 | **Abstract future-proof state vs concrete usable-today format** | NFR-4/HQ10 vs ship something now (NFR-6) | HQ10 |
| T9 | **Delegated agent authority vs blast radius of a leaked agent key** | FR-T4 (act for human) vs FR-S1/HQ1 | HQ11, HQ1 |

---

## 5. Non-goals / out-of-scope

Explicitly **not** in scope for this federation architecture (stating these
keeps task 4 from over-building and task 5 from attacking absent promises):

1. **Real-time / low-latency transport.** Email-speed (NFR-2) is a *deliberate*
   relaxation; chat-grade or RTC latency is a non-goal.
2. **Blockchain / token / global consensus ledger.** Self-certifying keys + signed
   events give us what we need without a coin, a chain, or global ordering.
3. **Rolling our own cryptography** (FR-S5). We compose audited primitives only.
4. **A global naming authority / DNS replacement.** Aliases are an optional,
   verifiable convenience layer (FR-F2), not a new root namespace.
5. **Strong anonymity / metadata-hiding network (Tor/mixnet grade).** We *bound
   and disclose* the metadata leak (FR-S4); we do not promise sender-recipient
   unlinkability in v1.
6. **A social-media product surface** (feeds, ranking, notifications UX). This is
   the identity/messaging/federation *substrate*; product UX is downstream.
7. **Solving opaque hidden-state portability now.** HQ10 requires the *interface*
   to accommodate future opaque/RNN state; actually serializing and reloading a
   model's hidden state is out of scope — we design the slot, not the payload.
8. **Re-implementing the existing agency-primitive transfer.** Today's
   `src/federation.rs` (Roles/Motivations/Agents transfer over filesystem paths)
   is the *migration substrate* (FR-F6), not the thing being redesigned; its
   re-baselining is task 2's job.
9. **Picking the final library/wire stack.** Choosing libp2p vs relays vs a
   specific DID method is task 4's (architectures) call; this doc constrains, it
   does not select.
10. **Multi-tenant billing / quota / payments** for relay/storage operators.
    Operability (NFR-5) covers self-hostability; commercial relay economics are
    out of scope.

---

## 6. Architecture acceptance checklist (definition-of-done for task 4)

A candidate architecture (`fed-architectures`) is **complete** only if it:

- [ ] Answers **HQ1 (key custody — the crux)** concretely: what is published,
      where the agent signing key lives, and the load-on-new-host continuity
      semantics. *(FR-I2, FR-I5, FR-S1, FR-S6)*
- [ ] Specifies the **addressing scheme** (HQ5) and how/whether aliases attach
      verifiably. *(FR-F1, FR-F2)*
- [ ] Specifies the **transport** (HQ3) meeting the email-speed/offline budget
      with no mandatory single relay. *(FR-M2, FR-F4, NFR-2)*
- [ ] States the **encryption = ACL** model and the **metadata** stance (HQ4).
      *(FR-S3, FR-S4)*
- [ ] States the **key rotation/recovery** and **revocation** story (HQ2).
      *(FR-S2, FR-S7)*
- [ ] Gives the **decentralization vs central** per-capability table, with no
      correctness-critical central dependency (HQ6). *(FR-F4, FR-F5)*
- [ ] States the **consistency model** for shared/forked state (HQ7). *(FR-M6)*
- [ ] States the **trust-establishment** + **anti-abuse** mechanisms (HQ8).
      *(FR-T1, FR-T2)*
- [ ] States the **human vs agent** custody/recovery/authority differences (HQ9,
      HQ11). *(FR-I6, FR-S6, FR-T4)*
- [ ] Defines the **versioned loadable-state interface** with graceful unknown-
      payload handling (HQ10) and a **compat handshake** (HQ12). *(FR-I3, FR-I4,
      NFR-4)*
- [ ] Names a **phased rollout** on top of today's WG (HQ-none; NFR-6). *(FR-F6,
      NFR-6)*
- [ ] Resolves (does not ignore) each **§4 tension**, stating which side it takes
      and why.
- [ ] Stays inside the **§5 non-goals**.

If any **MUST** requirement (§2) is unmet, the architecture must say so
explicitly and justify the deferral — silence is failure.

---

## 7. Traceability matrix (vision → requirements → hard questions)

| Vision pillar | Requirements | Hard questions |
|---------------|--------------|----------------|
| **V1** Loadable identity | FR-I1, FR-I3, FR-I5, FR-I7 | HQ1, HQ10 |
| **V2** Portable/downloadable | FR-I2, FR-I4, FR-I7, NFR-3 | HQ1, HQ5 |
| **V3** Email-speed messaging | FR-M1–FR-M6, NFR-2 | HQ3, HQ7 |
| **V4** Key-based P2P federation | FR-I1, FR-I2, FR-F1–FR-F3, FR-S1, FR-T1, NFR-7 | HQ1, HQ5, HQ8 |
| **V5** Decentralized, central allowed | FR-F3, FR-F4, FR-F5, NFR-5 | HQ3, HQ6 |
| **V6** Long-term reliability | FR-S2, FR-S5, FR-S7, FR-F5, NFR-1, NFR-4, NFR-6, NFR-7 | HQ2, HQ12 |
| **V7** Hybrid human+agent | FR-I6, FR-M4, FR-S6, FR-T3, FR-T4 | HQ9, HQ11 |
| **R2** loadable memory (gap) | FR-I3, FR-I7 | HQ10 |
| **R24** per-artifact ACLs (gap) | FR-S3, FR-S4 | HQ4 |
| **R16/R19** multi-bot messaging (gap) | FR-M3, FR-M4 | HQ3 |

---

## Appendix — sources & provenance

- **Vision / north star:** WG social-network vision memo; Erik's federation
  model dated **2026-06-24** (Keybase-like key-based P2P; "hardest crux: agent
  key custody"). Co-conceived with Luca Pinello.
- **Gap analysis:** private repo `poietic-pbc/poietic-family-team`,
  `docs/01-vision-and-requirements.md` + `docs/02-workgraph-gap-analysis.md`
  (38 reqs R1–R38, 2026-04-30; scorecard 8 supported / 15 partial / 15 missing).
  Only R2, R16/R19, R24 are cited by number here (confirmed in the vision memo);
  the rest live in the source repo and were deliberately *not* invented.
- **Current code (grounding only; full baseline is task 2):**
  `src/federation.rs` (named remotes/peers in `federation.yaml`; transfers agency
  *primitives* over filesystem paths — no crypto/identity/messaging today),
  `docs/ADR-actor-vs-agent-identity.md` (unified `Agent` = Role + Motivation +
  operational fields incl. `trust_level`, `contact`, `executor`; content-
  addressed IDs; human vs AI executor split), `wg msg` (task-keyed, single-graph
  messaging today), `wg secret` (keyring/keystore/plaintext backends — candidate
  agent-key custodian for HQ1).
- **Prior art to mine (task 1 will deepen):** Nostr (key=identity, relays, signed
  events — closest fit), Keybase (sigchain rotation + social proofs), DIDs
  (`did:key` URL addressing + key history), libp2p/IPFS (P2P transport +
  content-addressed state), Matrix (heavier E2EE-federation contrast),
  Signal/MLS (group key ratcheting for HQ4), AT Protocol (portable identity +
  PDS hosting).
