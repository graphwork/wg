# ADR-002 (WG-Fed): Transport — Node Store-and-Forward Default, Untrusted, Fallback Ladder

**Status:** Proposed
**Date:** 2026-06-25
**Decision:** Bytes move between WGs over a **pluggable fallback ladder** whose default rung is the **WG node's HTTP store-and-forward inbox**, escalating to an **Iroh QUIC direct path when both peers are online**, with **optional shared relays** as further rungs. **No single relay or node is mandatory** — the same `SignedEvent` traverses any rung, so losing one degrades *reach*, never *correctness*. Bytes are **signed and optionally sealed end-to-end**, so **the transport — including your own node — is untrusted**. The compat/transport handshake is **authenticated** (S-7). Email-speed, both-ends-offline-tolerant.

> **This ADR builds on ADR-001.** It does not define identity, keys, addressing, or
> verification — those are fixed in `docs/ADR-fed-001-identity-key-model.md`
> (`wgid:` self-certifying address, the sigchain, the three-tier key hierarchy, the
> `WG_FED_COMPAT_VERSION` handshake, "verification is never central"). ADR-002 takes
> the `SignedEvent` wire envelope and the `wgid:`-addressed identities as given and
> answers only **how the bytes actually move**. The decision was *made* in the
> federation-study decision memo
> (`docs/federation-study/06-decision-memo-and-roadmap.md` §1, §3 HQ3 (with HQ4/HQ6),
> §5 guardrails, §6 ADR-002 stub, §8 open-question hand-off); this ADR formalizes it
> and resolves the stub's three open questions. It is **not** a re-litigation of the
> architecture choice (`WG-Fed` = Candidate C with a B-shaped default, A's node-less
> mode preserved) — that is settled.

---

## Context

Today's WG "federation" is a daemon brokering a **same-filesystem, read-only,
path-addressed** transfer: a peer is resolved by a `PeerConfig.path`, and cross-repo
queries go over a **localhost Unix-domain socket** or a direct `graph.jsonl` file read
(`src/federation.rs`; `docs/federation-study/02-current-state-baseline.md` §2.1 — "no
remote host, no auth, no wire protocol beyond a localhost Unix socket"). It cannot
cross a host boundary, authenticate a peer by key, or deliver to an offline recipient.
The WG social-network vision needs the opposite: messages that move **across
independently-owned WGs on different hosts** (FR-M3), authenticated **purely by key**
(FR-M1), at **email speed with both ends possibly offline** (FR-M2, NFR-2).

The transport is squeezed from three sides (doc 03 HQ3):

- **Email-speed + offline tolerance** (V3, NFR-2, FR-M2) means the common case is
  *neither peer online at the same moment* — so a store-and-forward holding point is
  unavoidable, exactly as SMTP uses MX relays.
- **Real P2P is operationally heavy.** Most peers sit behind NATs and are not
  always-on; libp2p/DHT/hole-punching is a large, finicky dependency that does not by
  itself solve always-on availability (doc 04 §2.4).
- **Decentralization-leaning** (V5, FR-F4/F5) forbids letting any single relay or node
  become a mandatory root: removing any one central component must degrade *convenience*
  only, never *correctness or security* (FR-F4), and an identity plus its
  already-published state must survive the loss of its origin host (FR-F5).

The shortest path from today's code is the node: doc 02 §2.1 shows federation is
*already* a daemon brokering messages — promoting that broker from a localhost Unix
socket to an authenticated HTTP store-and-forward inbox is a small, proven step (doc 05
rates the node model **WG-fit 5**, "operationally the simplest"). The remaining freedom
is whether to *also* run a direct P2P leg and shared relays, and how to keep none of
them mandatory. This ADR fixes that.

This is the second Wave-2 deliverable; **no federation code lands until ADR-001/002/003
are Accepted** (memo §5, Wave 2).

---

## Decision

### D1 — A pluggable fallback ladder; the node inbox is the default rung

Transport is a **set of interchangeable adapters arranged as an ordered fallback
ladder**, each carrying the *same* `SignedEvent` (ADR-001 §D, doc 04 §1.4c). The rungs,
in default preference order:

1. **WG node HTTP store-and-forward inbox — the default.** The sender's node `POST`s a
   `SignedEvent` to the recipient's node inbox over HTTP(S); the recipient's node
   **holds it until the (possibly-offline) recipient polls**. This is the
   ActivityPub/atproto-PDS inbox model (doc 01 §2.4/§2.6, doc 04 §3.4). It is the
   default because the node is **always-on and NAT-free** (no hole-punching), it reuses
   the existing daemon-broker (doc 02 §2.1), and it is the only rung that *by itself*
   satisfies the both-ends-offline budget. Routine traffic never needs anything else.
2. **Iroh QUIC direct path — opportunistic, when both peers are online.** When sender
   and recipient are both reachable, a dial-by-pubkey QUIC path (relay-assisted
   hole-punch; doc 01 §2.9, doc 04 §2.4) gives a lower-latency direct hop. This is a
   pure *optimization* over rung 1; if hole-punching fails, delivery falls back to the
   node inbox with no loss of correctness. (Whether Iroh specifically is the P2P library
   is **deferred** — see OQ1.)
3. **Optional shared relays — additional store-and-forward rungs.** A dumb,
   self-hostable store-and-forward box (Nostr-style relay; doc 04 §2.4) an identity may
   advertise *in addition to* its node, for reach when the node is unreachable or for
   the node-less deployment. An identity advertises *several* via the `endpoints` list
   in its `IdentityRecord` (ADR-001 §D5 / doc 04 §1.4a).

The same envelope is acceptable on any rung; the adapter is a routing/availability
choice, never a semantic one. This is the hybrid of doc 03 HQ3's three axes
(store-and-forward ↔ true P2P ↔ hybrid) realized as a ladder, matching doc 04 §4.4 (the
Candidate-C transport) with the **node as the explicit default** (the B-shaped default,
memo §2.2).

### D2 — No single relay or node is mandatory (FR-F4/F5)

No rung is a required root. The reachability contract:

- An identity advertises **≥1** delivery endpoint (its node and/or one or more relays)
  in its signed `IdentityRecord`. Resolving *where to deliver* reuses ADR-001 §D5's
  **fail-safe resolution cascade** (cached signed endpoint record → optional directory
  hint → DHT/Iroh discovery); any one step suffices.
- **Removing any single relay or node degrades reach, not correctness** (FR-F4): a
  message that cannot reach rung 1 is retried on the next advertised rung; an identity
  and its already-published state remain verifiable and fetchable from any surviving
  host (FR-F5), because the bytes are self-verifying (ADR-001 §D2 — content-addressed,
  signed) and the address embeds its own trust root (ADR-001 §D1).
- The **node-less deployment** drops rung 1 and runs on rungs 2–3 (shared relays + the
  opportunistic Iroh leg) — the decentralization option (A) preserved, with no
  correctness dependency on any always-on box.

This is the literal FR-F4/F5 guarantee and HQ6's invariant applied to transport: a
central delivery component is a *convenience* (CV in doc 04 §6.2's table), never
correctness-critical (CC). The one CC capability — verifying *who* authored an event —
lives entirely in ADR-001 and touches no transport rung.

### D3 — The transport, including your own node, is untrusted

Every byte on every rung is a `SignedEvent`: **signed** by an authorized signer key
(verifiable offline against the author's sigchain, ADR-001 §D2/§D3, FR-M1) and
**optionally sealed** per-recipient (X25519 + XChaCha20-Poly1305; the `to` set *is* the
ACL, FR-S3, HQ4). Consequently **no transport rung is ever trusted for
confidentiality or authenticity — and this explicitly includes a deployment's own
node**:

- A relay or peer node **cannot forge** an event: a forged "from Nora" fails the
  signature check at the recipient (FR-M1), and a tampered byte breaks the BLAKE3 CID.
- A relay or peer node **cannot read sealed content**: only a holder of a recipient
  encryption key can open a sealed envelope; a third party holding the ciphertext
  cannot (FR-S3).
- **Your own node is untrusted for the same two properties.** This is a deliberate
  sharpening of Candidate B's "your own node is trusted" framing (doc 04 §3.4): WG-Fed
  treats the node as a convenience that *cannot* override a self-verification (HQ6,
  ADR-001 §D5), so even a fully-compromised node cannot impersonate an identity or
  decrypt sealed traffic. This is precisely what bounds the node-compromise finding
  (doc 05 B-1) at the protocol level: a compromised node degrades availability and sees
  metadata (see the metadata note below), but cannot forge what a verifier self-checks.

**What the transport *does* see (metadata, disclosed not eliminated — FR-S4, HQ4).** A
node/relay necessarily sees routing metadata: the `to` set, the `from` (unless
sealed-sender is used), timing, and size; and an *unsealed* body is readable by any rung
that handles it (sealing is optional in the spark and for low-value traffic). In the
node deployment, **your own node sees your whole social graph** — doc 05 B-2, the worst
metadata posture of the four candidates, accepted and disclosed: self-hosting your node
makes *you* that observer. WG-Fed offers **sealed-sender** to hide `from` from peer
nodes but explicitly **does not** promise recipient-unlinkability or mixnet-grade
anonymity (non-goal, memo §7.5). The encryption-as-ACL mechanism and its
forward-secrecy/metadata trade-offs are owned by HQ4 (realized in Wave 6); ADR-002 only
commits that the transport assumes nothing about its own trustworthiness.

### D4 — The transport/compat handshake is authenticated (S-7)

Before two peers exchange events they negotiate `WG_FED_COMPAT_VERSION` and the crypto
parameters (ADR-001 §D7). Per the adversarial finding **S-7**, that handshake is
**authenticated, not merely exchanged**: the negotiated parameters are **signed** by the
peers' keys, so a man-in-the-middle on a transport rung **cannot strip strong crypto or
force a "lowest-common-`alg`" downgrade**. WG-Fed enforces a **minimum-`alg` floor** with
aggressive retirement of known-weak suites (refuse, don't silently degrade), and **fails
loudly** on an incompatible mismatch (WG's existing convention). Because the transport is
untrusted (D3), the handshake is the one place a downgrade could be injected, so it is
hardened here rather than left to per-adapter discretion.

### D5 — Delivery semantics: email-speed, offline-tolerant, at-least-once

The unit of transport is the `SignedEvent` (doc 04 §1.4c). Across every rung:

- **Email-speed, both-ends-offline.** Delivery targets seconds-to-minutes (NFR-2), not
  real-time; a message **persists until fetched** (FR-M2). Sending to an offline
  recipient succeeds (it is accepted for store-and-forward); the recipient receives it
  on its next poll/connect.
- **At-least-once with idempotent dedup.** The event `id` is its BLAKE3 CID; a
  redelivered event is recognized as a duplicate and **not double-applied** (FR-M6). A
  rung may deliver an event more than once (retries across rungs); the recipient
  deduplicates by `id`.
- **Causal ordering per conversation, not global.** Ordering is established by `refs`
  (reply/task/artifact links; FR-M5/M6), giving per-conversation causal order without a
  global serializer — consistent with the single-writer-per-object spine (HQ7).
- **Freshness rides the async path.** High-value actions re-fetch the signed freshness
  attestation defined in ADR-001 §OQ4 (the S-3 freeze defense) before acting, and
  **fail closed on stale**; the transport simply carries the attestation as another
  fetchable artifact — it provides no liveness guarantee of its own.

---

## Status

**Proposed.** This ADR records the transport decision exactly as fixed in the
federation-study decision memo (§3 HQ3) and resolves the three open questions the
ADR-002 stub left open. **Erik ratifies it to Accepted** — that human gate is
deliberately not set here. No federation code lands until ADR-001/002/003 are Accepted
(memo §5).

---

## Consequences

- **Promote the daemon to an authenticated HTTP node.** The existing broker (doc 02
  §2.1, the localhost Unix-socket IPC) gains an HTTP(S) store-and-forward **inbox**
  endpoint (`POST` an event, `GET`/poll the inbox). This is the default-rung work and
  the nearest extension of today's code.
- **`src/messages.rs` (`Message`) gains the cross-graph path.** Per ADR-001 / doc 04
  §1.5, `Message` adds `from`/`to`/`sig`/`refs` (all `#[serde(default)]`, so today's
  task-keyed JSONL still parses — backward compatible) and a **send/poll** path that
  wraps the local queue at one end and a node-inbox/relay at the other. A small
  **transport-adapter trait** abstracts the rungs (node-HTTP / Iroh / relay) behind one
  `send(event)` / `poll(inbox)` interface so the fallback ladder is a list of adapters,
  not branching call sites.
- **`src/federation.rs` peers gain endpoints + the resolution cascade.** `PeerConfig`/
  `Remote` carry `wgid` + an `endpoints` list (node/relay URIs) beside the legacy
  `path`; `resolve_peer` becomes the ADR-001 §D5 cascade. Path-based `federation.yaml`
  peers keep working alongside key-based ones (FR-F6 — the migration substrate, not the
  redesign target; memo §7.8).
- **New transport dependency is deferred** (OQ1): the default rung needs only an HTTP
  client/server (the tree already carries `reqwest`/`rustls`, doc 02 §2.4); the P2P-leg
  crate (e.g. `iroh`) is **not** added until its deciding wave.
- **Realizes** FR-M1/M2/M3 (signed, async, cross-graph), FR-F4/F5 (no mandatory central
  node), NFR-2 (email-speed), NFR-5 (self-hostable rungs), and NFR-1 (store-and-forward
  survives restarts/outages).
- **Cost we accept:** the node sees routing metadata and unsealed bodies (D3, doc 05
  B-2), bounded and disclosed. Maintaining multiple transport adapters is part of
  Candidate C's coherence tax (doc 05 C-1/C-2), mitigated by the single adapter trait
  and by deferring rungs 2–3 until the wire is proven (memo §5 guardrail; the wire is
  candidate-agnostic through Wave 4, doc 04 §9).

This ADR maps onto the roadmap's Wave 4 (memo §5): the node HTTP store-and-forward inbox
becoming the default transport, `wg msg --to wgid:` between graphs, freshness
attestations on the async path. The spark test (memo §4) exercises the thinnest slice —
a single store-and-forward rung delivering to an offline recipient (memo §4.2 step 4).

---

## Alternatives rejected

- **Pure-P2P-only transport** (Candidate A's purest form — libp2p/DHT/hole-punching with
  no store-and-forward). Heavy NAT traversal, no always-on availability, and it does not
  meet the both-ends-offline budget on its own (a direct path needs both peers present).
  Rejected as the *default*; the opportunistic Iroh leg (D1 rung 2) keeps the direct-P2P
  *option* without making correctness depend on it (doc 04 §2.4, doc 05 §4.1).
- **A single mandatory relay or node** (the SMTP-with-one-MX shape, or making "your"
  node a required root). Violates FR-F4/F5 — losing it would break correctness, not just
  convenience — and recreates the single-point-of-failure WG-Fed exists to remove.
  Rejected: every rung is optional and interchangeable (D2).
- **Trusting your own node** (Candidate B's literal framing, doc 04 §3.4: "your own node
  is trusted"). Rejected as too strong: end-to-end signing + optional sealing cost
  nothing extra and turn node compromise (doc 05 B-1) from "mass impersonation" into
  "availability + metadata loss only" (D3). The node stays a *convenience*, never a
  trust root.
- **Real-time / RTC transport** (sub-second delivery, persistent connections as the
  baseline). A non-goal (memo §7.1, NFR-2): email-speed is a deliberate relaxation that
  *buys* decentralization and offline tolerance. Real-time would force always-on
  connectivity and undercut the store-and-forward model. The optional push/firehose
  (OQ2) is a latency *optimization* within the email-speed envelope, not a real-time
  guarantee.
- **Mandatory forward secrecy on the store-and-forward path.** Does not compose with
  send-to-offline — you cannot ratchet with a party who is not there to ratchet back
  (doc 05 S-6); the default path uses static recipient keys (capped by enc-key
  rotation), and FS is available only on the online/long-lived path (MLS, Wave 6). This
  is an HQ4 decision noted here because it constrains what the transport can promise.

---

## Open questions

The ADR-002 stub (memo §6) and the memo's handed-off checklist (§8 item 4) left three
questions for this ADR to close. All three are resolved with rationale below; where a
residue is genuinely an economic/policy/tuning value judgment it is **explicitly flagged
for Erik** rather than silently fixed.

### OQ1 — Iroh vs a thinner relay for the P2P leg — **RESOLVED: defer past Wave 4 (by design), with the deciding criteria fixed here**

**Resolution.** **Keep the wire library candidate-agnostic through Wave 4 and do not
bind the P2P-leg library now.** This is not an omission — it is the decision, and it
matches the memo's explicit guardrail: *"Don't pick the final wire library (Iroh vs
relays vs node-HTTP-only) before Phase 2 — the wire is candidate-agnostic through Wave 4;
let the spark and cross-graph waves inform it"* (memo §5, doc 04 §9). The default rung
(node HTTP store-and-forward, D1 rung 1) carries the entire correctness load with only an
HTTP client/server, so the P2P leg is a pure latency optimization whose library can be
chosen *late*, on evidence, behind the transport-adapter trait (Consequences) without
reopening anything in D1–D5.

**The decision is bound to a gate, not left open-ended.** Decide the P2P-leg library at
the **end of Wave 4** (entering Wave 5 / Phase 2), informed by the spark (Wave 3) and
cross-graph (Wave 4) waves. The candidates at that gate are **(a) Iroh QUIC**
(dial-by-pubkey, relay-assisted hole-punching), **(b) a thin self-hosted relay only** (no
direct leg — node + shared store-and-forward relays carry everything), or **(c) both**.

**The deciding criteria (fixed now so the later choice is mechanical):**

1. **Does the direct leg earn its weight under the email-speed budget?** If
   store-and-forward (rungs 1+3) already meets NFR-2 in the field, a direct P2P leg is a
   pure optimization and the bar for adding a heavy QUIC/DERP dependency is *high*. If
   measured latency or relay load makes a direct hop materially better, the bar drops.
2. **NAT-traversal success rate in the real deployment topology.** Iroh's
   relay-assisted hole-punch only pays off if it actually connects across the NATs WG
   peers sit behind; if success rates are poor, a guaranteed-but-indirect relay path is
   simpler and just as correct.
3. **Operational weight / self-hostability (NFR-5).** A thin relay is a dumb HTTP box a
   single person can stand up; Iroh pulls a larger QUIC + relay (DERP) stack. The lower
   the operational burden, the better the decentralization option survives.
4. **Library maturity, audit status, and Rust-ecosystem fit at the gate.** Measured at
   decision time, not assumed now (doc 05 weights maturity heavily).
5. **Addressing alignment.** A dial-by-pubkey transport (Iroh `NodeId`) should compose
   cleanly with `wgid:` addressing (ADR-001 §D1) rather than introduce a second,
   conflicting identity namespace.
6. **Dependency / licensing surface.** The marginal dependency cost the leg adds to the
   `wg` binary.

*Why defer.* Binding the wire now would prematurely harden the topology before the key
model and the default rung are proven (the exact mistake doc 03 §5 non-goal 9 and the
memo §5 guardrail warn against). The cost of waiting is *zero* for correctness (rung 1
suffices) and the benefit is a library choice made on real data.

*Not Erik's call (a deferral, not a policy gap):* this is an engineering binding the memo
already scheduled to be made later on evidence. It is **settled as "decide at the Wave-4
gate against criteria 1–6,"** which is itself the commitment. (If Erik wants to *pull the
gate forward* or *force node-HTTP-only forever*, that is a product call he can make — but
the default is the disciplined deferral above.)

### OQ2 — Pull (poll) vs push (subscription) delivery on the inbox — **RESOLVED**

**Resolution — pull is the mandatory baseline; push is an optional latency
optimization.**

- **Pull / poll is the default and the only required mechanism.** The recipient (or its
  node, on its behalf) **polls** its inbox for new events and fetches them. Poll is the
  only model that **composes with both-ends-offline** (FR-M2): an offline recipient
  cannot hold a live subscription, so it must reconcile on its next connect — which is a
  poll. Poll is also the simplest, most NAT-friendly, and matches the
  atproto-PDS/ActivityPub inbox model (doc 04 §3.4).
- **Push / subscription is an optional optimization for *online* clients.** A node MAY
  expose a **firehose / subscription** (Nostr-style `subscribe`, or SSE/long-poll/
  WebSocket; doc 04 §2.4/§3.4) so an already-connected client gets near-real-time fanout
  instead of waiting for its next poll. This is a **convenience layer only**: if push is
  unavailable or a subscription drops, delivery degrades to the poll interval — *latency
  degrades, correctness does not*. Push is never required and never a delivery guarantee.
- **Default poll cadence is email-speed** (NFR-2): on the order of tens of seconds to a
  few minutes, configurable per deployment. A client that wants lower latency opts into
  the push firehose where the node offers it.

*Why.* This is the same fail-safe shape as the rest of the design: the offline-tolerant,
correctness-bearing mechanism (poll) is mandatory and universal; the latency optimization
(push) is optional and degrades gracefully. Requiring push would break FR-M2 (offline
recipients) and re-introduce an always-connected assumption the email-speed budget exists
to avoid.

*Flagged for Erik (tuning only):* the **default poll cadence** and **whether nodes ship
the push firehose in v1 or defer it to a later wave** are tuning/scope knobs, not
design decisions. The *mechanism rule* (poll mandatory + offline-bearing, push optional +
latency-only) is the ADR-002 commitment; the cadence number and firehose-now-vs-later are
Erik's to set.

### OQ3 — State-blob storage duration + who pays (self-host vs shared) — **RESOLVED (mechanism; commercial economics flagged out-of-scope / for Erik)**

**Resolution — self-host by default; durability comes from pinning, never from a relay
you don't control.** Two distinct things are storage-bearing, and they get the same
fail-safe answer:

- **Inbox messages (store-and-forward).** Retained **until fetched-and-acked**, then for
  a **grace window** (default on the order of weeks, e.g. 30 days), then garbage-
  collected. At-least-once + idempotent dedup (D5) makes redelivery safe; an event that
  expires unfetched is the **sender's** responsibility to re-send (the email "bounce/
  expiry" model). No rung promises infinite retention.
- **State blobs (`StateSnapshot`, ADR-001 / doc 04 §1.4b).** **Content-addressed and
  immutable** (BLAKE3), so any number of hosts can serve the identical bytes. A blob is
  retained **as long as the publisher (or any pinner) keeps it pinned**; `prev`-chained
  incremental snapshots mean you keep the head plus as much history as you choose to pin.
  Unpinned blobs MAY be GC'd by a host. FR-F5 holds as long as **≥1** host serves the
  bytes — and because the bytes are self-verifying, that host can be *any* dumb store (a
  file, an S3 bucket, an IPFS gateway, a relay, the node), exactly the "third location"
  the spark test uses (memo §4.1).

- **Who pays — self-host is the default answer.** In the default deployment your **WG
  node hosts your own inbox and your own state blobs**, so "who pays" is "the node
  operator (you)" — which *is* the NFR-5 self-hostability answer (a single person can
  stand up a node on commodity hardware). **Shared/third-party relays are best-effort:**
  retention is the operator's policy and carries **no durability guarantee** — durability
  comes from self-hosting or from pinning to a host you control, never from a shared
  relay you don't. This keeps the FR-F5 guarantee on a foundation you own.

*Why.* Tying durability to *pinning a self-verifying, content-addressed blob* (rather
than to trusting a particular relay's retention) is the only model that keeps FR-F5 true
without a mandatory central store: as long as one host you trust to *exist* serves the
bytes, the identity and its published state survive (memo §1, doc 04 §6.2 "state hosting
= any of relay/IPFS/node/file"). Bounded inbox retention matches the email mental model
and prevents an inbox from being an unbounded liability for a node operator.

*Flagged for Erik / explicitly out of scope for v1:* **multi-tenant billing, quotas, and
paid-pinning economics** for relay/node operators are a **non-goal for v1** (memo §7.12 —
NFR-5 covers self-hostability; commercial relay economics are out of scope). The
**default retention windows** (inbox grace, blob GC policy) are tuning knobs, and
**whether to ship any shared-relay quota mechanism at all** is a product/policy call —
the default is *no quota mechanism; self-host or pin*. The *mechanism* (self-host-by-
default, content-addressed pinnable blobs, bounded inbox retention with sender-resend,
GC of unpinned, FR-F5 satisfied by ≥1 self-verifying host) is the ADR-002 commitment; the
numbers and the commercial-economics question are Erik's.

---

## References

- `docs/ADR-fed-001-identity-key-model.md` — the identity, `wgid:` address, sigchain,
  three-tier keys, resolution cascade (§D5), authenticated `WG_FED_COMPAT_VERSION`
  handshake (§D7), and freshness attestations (§OQ4) that this transport carries and
  verifies against. **ADR-002 depends on and cites ADR-001 throughout.**
- `docs/federation-study/06-decision-memo-and-roadmap.md` — §1 (the decision; "the
  transport — including your own node — is always untrusted"), §3 HQ3 (transport,
  formalized here) with HQ4 (encryption=ACL / metadata) and HQ6 (decentralization vs
  central), §5 (Wave 4 transport + the don't-pick-the-wire-library-yet guardrail), §6
  ADR-002 stub, §8 item 4 (P2P-leg deferral hand-off).
- `docs/federation-study/04-candidate-architectures.md` — §1.4c (`SignedEvent` wire
  envelope), §1.5 (`src/messages.rs`/`src/federation.rs` touch-points), §2.4 (Candidate-A
  relays+gossip+Iroh transport), §3.4 (Candidate-B node HTTP store-and-forward inbox),
  §4.4 (Candidate-C fallback ladder — the shape adopted), §6.2 (per-capability
  central-node table; transport = CV), §9 (candidate-agnostic phasing through Phase 2).
- `docs/federation-study/03-requirements-and-hard-questions.md` — HQ3 (transport, with
  its decision axes and success criteria), FR-M1–M6 (signed/async/cross-graph/threaded/
  idempotent messaging), FR-F4/F5 (no mandatory central node; no single point of
  permanent failure), FR-S3/S4 (encryption=ACL; bounded metadata), NFR-2 (email-speed),
  NFR-5 (self-hostability), NFR-1 (reliability under restart/outage).
- `docs/federation-study/02-current-state-baseline.md` — §2.1 (today's federation is a
  daemon brokering over a localhost Unix socket, same-filesystem, read-only — the
  baseline this transport extends), §2.3 (`Message`), §2.4 (crypto/transport crates
  present: `reqwest`/`rustls`).
- `docs/federation-study/05-adversarial-evaluation.md` (via the memo) — B-1 (node
  compromise, bounded by D3's end-to-end signing/sealing), B-2 (metadata posture: your
  own node sees your social graph — disclosed), S-6 (forward secrecy does not compose
  with send-to-offline), S-7 (downgrade — the authenticated handshake, D4), WG-fit 5 (the
  node inbox is the nearest, simplest path from today's code).
- `docs/federation-study/01-prior-art-landscape.md` — §2.4/§2.6 (ActivityPub/atproto-PDS
  inbox model), §2.9 (Iroh QUIC dial-by-pubkey), §2.1 (Nostr relays) — the prior art the
  rungs draw on.
