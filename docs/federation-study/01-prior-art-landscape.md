# Federation Study 1/6 — Prior-Art Landscape

> **Decentralized / federated identity + messaging systems, mapped to WG's
> federation vision.**
>
> Wave 1, task 1 of 6 (the *gather* phase). Downstream consumers:
> `fed-requirements` (3/6), `fed-architectures` (4/6), `fed-adversarial`
> (5/6), `fed-decision` (6/6). This document is a *survey + comparison*, not a
> design — design decisions are deferred to those tasks.

---

## 0. The WG yardstick (what we are measuring against)

WG's north star (see the project's social-network vision; gap-analysis R1–R38)
is to evolve from a task orchestrator into a **hybrid human + agent social
network**. The federation pillars that every system below is scored against:

| # | WG requirement | One-line test |
|---|----------------|---------------|
| **V1** | **Key *is* identity *and* address** — self-certifying, no central registry | Can I name a peer by their public key alone and have that name *be* verifiable? |
| **V2** | **Long-lived, loadable, portable identity** — published to a registry, fetchable, reconstituted on another host | Can an identity move hosts / be downloaded and reconstituted? |
| **V3** | **Async "email-speed" store-and-forward messaging** — signed events, relays OK, not real-time | Can A send to offline B and have it delivered later, signed? |
| **V4** | **Cross-WG, key-authenticated collaboration** — P2P-leaning, central nodes *allowed* | Can two independently-owned WGs talk without a shared central authority? |
| **V5** | **Agent key custody** — a downloadable identity is *public-identity + signed-state*, **NOT the signing key** (download ≠ impersonation) | Can I publish a portable identity that others can *verify* but cannot *sign as*? |
| **V6** | **Rotation / recovery without losing identity continuity** — lose/rotate a key, keep the identity | If a key is lost or compromised, does the identity survive? |
| **V7** | **Hybrid human + agent** — same primitives serve people and bots; many keys per identity | Can one identity hold many device/agent keys with different powers? |

The two **special-focus cruxes** the task singles out are **V5 (agent key
custody)** and **V6 (rotation/recovery)** — these are scored explicitly in §4.

---

## 1. Comparison matrix (systems × dimensions)

Sixteen systems across the seven WG dimensions plus the eight per-system
capture dimensions. Abbreviations: **WoT** = web-of-trust, **TOFU** =
trust-on-first-use, **CA** = certificate authority, **S&F** =
store-and-forward, **CAS** = content-addressed storage.

### 1a. Identity, trust, transport, portability

| System | Key = identity & address? | Trust establishment | Messaging / transport (latency) | State portability |
|---|---|---|---|---|
| **Nostr** | **Yes** — `npub` (bech32 secp256k1 pubkey) *is* the identity; human name via NIP-05 `name@domain` | TOFU + NIP-05 DNS hint + follow graph (no WoT enforced) | Relays (websocket), client publishes signed events to N relays; **S&F**, near-real-time→email | High — events are signed, relay-agnostic; re-publish anywhere (NIP-65 relay list) |
| **Keybase** | Username-rooted, but **sigchain is key-anchored**; pubkeys announced in chain | **Social proofs** (Twitter/GitHub/DNS) + sigchain audit + following | Server-mediated (Keybase API/KBFS); chat is E2EE S&F | Medium — sigchain is portable data but bound to keybase.io server/Merkle root |
| **DIDs (did:key/web/plc)** | **Yes (did:key)** — DID *is* a pubkey-derived URI; did:web binds to a domain; did:plc to a ledger | Method-specific: did:key=self/TOFU, did:web=CA/DNS, did:plc=rotation-key ledger | **None** (DID is an identity layer, not transport) | did:key=trivially portable; did:web=domain-bound; did:plc=portable via signed ops |
| **AT Protocol (Bluesky)** | DID is identity; handle (`@name.bsky.social`) is a mutable alias | DID document + handle verification (DNS/HTTPS) | PDS (your repo host) → Relay (firehose) → AppView; **S&F** via repo, near-real-time fanout | **High** — account migration moves repo + updates DID to point at new PDS |
| **Secure Scuttlebutt (SSB)** | **Yes** — feed ID = `@<base64 ed25519 pubkey>.ed25519` | TOFU + follow graph (friend-of-friend replication = implicit WoT) | **Gossip** replication of append-only logs; pubs/rooms relay; **S&F**, eventually-consistent (high latency) | Low/medium — feed *is* portable but **single-key, single-device**; no multi-device |
| **ActivityPub** | **No** — `@user@server` (server-issued); HTTP actor URI is the identity | Server-trust; HTTP Signatures between servers; no end-user keys | Server-to-server HTTP `POST` to inbox; **S&F** at server, email-speed | Low — `Move` activity exists but follower migration is lossy; content stays on origin server |
| **Matrix** | **No** — `@user:homeserver` (server-issued MXID) | Server-trust + per-device keys; **cross-signing** for device trust | Homeserver federation (S2S API); rooms replicated; **S&F**, near-real-time | Low — MXID is server-bound; account portability (MSC) still unsolved in practice |
| **libp2p + IPFS / IPNS** | **PeerId = hash of pubkey** (transport identity); IPNS name = pubkey-signed mutable pointer | TOFU (PeerId in multiaddr); content trust = CAS (CID = hash) | P2P (DHT routing, multiple transports); pubsub gossip; CAS pull; **S&F** weak (needs pinning) | **High (content)** — CID/IPNS are host-agnostic; data is where it's pinned |
| **Iroh** | **Yes** — `NodeId` = ed25519 pubkey; "dial keys, not IPs" | TOFU + mutual TLS (each end's pubkey = its TLS identity) | **Direct P2P QUIC** (relay-assisted hole-punch); iroh-docs = eventually-consistent KV; near-real-time | High (content) — iroh-blobs/docs are content-addressed, relay-agnostic |
| **UCAN** | Principals **are DIDs**; not transport | **Capability chains** — cryptographic delegation, not identity-trust | **None** (authz token format) | High — tokens are self-contained, portable bearer/holder proofs |
| **Sigstore** | **No persistent key** — ephemeral key + OIDC identity (email/CI) | **CA (Fulcio)** + OIDC + **transparency log (Rekor)** | **None** (signing/transparency infra) | N/A — it's a notary, not an identity you carry |
| **PGP / age / minisign** | Key *is* identity (PGP fingerprint); age/minisign = raw keypair | PGP=**WoT**; age/minisign=TOFU/out-of-band | **None** (file/message signing+encryption primitives) | High — keys are files you carry; no built-in publish/resolve |
| **Signal Protocol** | Phone-number-rooted (Signal app); protocol itself is key-based (identity key + prekeys) | TOFU + safety-number verification | **Async E2EE S&F** (X3DH offline setup + Double Ratchet); server relays sealed envelopes | Low — account is phone-number + server-bound; protocol is portable, the *service* is not |
| **Farcaster** | **fid** (number) on-chain; **custody key** owns it; **signers (app keys)** post | On-chain key registry (Optimism); custody Ethereum key = root of trust | **Hubs** gossip-replicate signed messages (libp2p); **S&F**, eventually-consistent | High — messages are signed & hub-replicated; fid is portable across hubs/clients |
| **WireGuard / Tailscale** | **Yes (network layer)** — peer = its Curve25519 pubkey; Tailscale maps key↔node | WG=manual key exchange (TOFU); Tailscale=**OIDC SSO** + coordination server | Encrypted UDP tunnel (Noise IK); **real-time**, not S&F messaging | Low — keys are node-bound; Tailscale node identity is coordination-server-bound |
| **SSH keys / ssh-agent** | Key = identity (authorized_keys, allowed_signers); ssh-agent = **delegated holder** | TOFU (known_hosts) + allowed_signers files; CA mode (SSH certs) | **None** (auth + `ssh -Y` signing primitive) | High — keys are files; **ssh-agent forwarding = canonical "use key without holding it"** pattern |
| **FIDO2 / WebAuthn (passkeys)** | Per-RP credential pubkey; **private key is non-exportable** (hardware) | CA (attestation) + RP-bound TOFU registration | **None** (auth ceremony) | Low (hardware-bound) / Medium (synced passkeys via platform cloud) |

### 1b. Rotation/recovery, decentralization, maturity, agent-key-custody fit

| System | Key rotation & recovery | Decentralization level | Maturity / adoption / op-cost | Agent-key-custody fit (V5) |
|---|---|---|---|---|
| **Nostr** | **Weak** — `npub` *is* identity; lose key = lose identity. NIP-46 keeps key in a "bunker"; NIP-41 (key migration) is draft/contested | Relay-mediated, low barrier (run a relay) | Mature-ish, growing; relays cheap | **Good via NIP-46** — client signs through a remote bunker; client never holds nsec |
| **Keybase** | **Strong** — add/revoke device keys & paper keys in sigchain; PUK rotates on revocation; **identity (username) survives** | Central (keybase.io Merkle server) + verifiable client audit | Mature but **stagnant** (Zoom-owned); free | **Excellent model** — per-device sibkeys; one identity, many keys, revocable |
| **DIDs** | did:key=none; **did:web=rotate via DID doc**; **did:plc=rotation keys + signed history** | Spectrum: did:key (self) → did:web (DNS) → did:plc (ledger) | W3C Rec (2022); broad tooling | **Excellent** — DID doc lists *many* verification methods; agent key = one entry, revocable |
| **AT Protocol** | **Strong** — DID doc has rotation keys; recovery key can override PDS within 72h window | PDS=self-hostable, but PLC directory + relays are *de facto* central | Mature, millions of users; PDS op-cost low | **Good** — signing keys live in PDS/DID doc; rotation keys recover; app passwords (scoped) exist |
| **SSB** | **Weak** — feed = one key forever; lose key = dead feed; no multi-device; "fusion identity" experimental | **Fully P2P** (gossip), no servers required | Niche, declining; pubs/rooms cheap | **Poor** — single key *is* the feed; no delegation |
| **ActivityPub** | Account = server-issued; "recovery" = your server admin; key rotation = server's job | **Server federation** (central per-instance) | Very mature (Mastodon); server op-cost moderate | **Poor** — end users don't hold keys; server signs |
| **Matrix** | **Strong (device keys)** — cross-signing, key backup, SSSS recovery key; **but MXID is server-bound** | **Server federation** + E2EE | Mature; homeserver op-cost **high** (Synapse heavy) | **Good (device model)** — per-device keys, cross-signed; identity = account, not key |
| **libp2p / IPFS / IPNS** | PeerId rotation = new identity; IPNS republish needs the key; no recovery | **Fully P2P** (DHT) | Mature transport; IPFS heavy, IPNS slow | **Partial** — IPNS key signs a pointer; could host-hold the key, publish signed records |
| **Iroh** | NodeId rotation = new node; tickets re-issuable; no identity-recovery layer | **P2P** (relay-assisted) | **New, 1.0 (2026)**; very low op-cost (relays optional) | **Partial** — transport identity; would need an identity layer atop |
| **UCAN** | **Excellent for delegation** — rotate by re-issuing capability chains; root DID can revoke | Fully decentralized (offline-verifiable) | Emerging (v1.0 line); zero infra | **Excellent (the mechanism)** — *delegate, don't share keys* is literally the design goal |
| **Sigstore** | **Keyless by design** — ephemeral keys, identity = OIDC; recovery = re-auth | CA + log (federated trust roots) | Mature (OSS supply-chain standard) | **Conceptually strong** — proves "X signed this" without X holding a long-lived key |
| **PGP / age / minisign** | PGP=subkeys + revocation certs; age/minisign=manual re-key, no recovery | **Fully decentralized** (no infra) | PGP mature/clunky; age/minisign modern/simple | **PGP partial** (subkeys delegate); age/minisign = raw, no delegation |
| **Signal Protocol** | Strong forward-secrecy (ratchet); identity-key change = re-verify safety number | Central (Signal servers) but E2EE | Gold-standard crypto, huge adoption | **Poor** (identity-bound) but **its async S&F + sealed-sender are the messaging model to copy** |
| **Farcaster** | **Excellent** — custody key adds/removes **signers (app keys)**; lose a signer ≠ lose fid; recover via custody key | On-chain registry (central-ish) + P2P hubs | Growing; hub op-cost moderate, registry = gas | **Excellent exemplar** — *exactly* V5: custody key root, app keys act, download a signer ≠ own the identity |
| **WireGuard / Tailscale** | WG=manual re-key; Tailscale=rotate node key, SSO identity persists | WG=fully P2P; Tailscale=coordination-central | WG in Linux kernel; Tailscale mature SaaS | **Partial** — Tailscale's "identity (SSO) ≠ node key" split mirrors V5 |
| **SSH / ssh-agent** | Rotate authorized_keys; SSH CA = short-lived certs; agent holds key, forwards *use* | Decentralized (per-host trust) | Ubiquitous, mature, free | **Excellent pattern** — **ssh-agent = use-the-key-without-holding-it**; the canonical custody primitive |
| **FIDO2 / WebAuthn** | **Non-exportable** keys; recovery = register a 2nd authenticator; synced passkeys = cloud recovery | RP-central (per-site) | Mature, OS/browser-native | **Strong (custody)** — private key *cannot* be exfiltrated; but per-RP, no global identity |

---

## 2. Per-system narratives

Each entry follows the task's eight capture dimensions. Citations point at the
canonical spec/source (full URLs in §6).

### 2.1 Nostr — *the closest single fit*
- **Identity & addressing.** A Nostr identity **is** a secp256k1 keypair; the
  public key, bech32-encoded as `npub1…`, is the identity *and* the address.
  Human-friendly names are an optional layer: **NIP-05** maps `name@domain` →
  pubkey via `/.well-known/nostr.json`. [NIP-01, NIP-05]
- **Trust.** TOFU on the pubkey; NIP-05 gives a DNS-anchored hint; the follow
  graph (kind-3 contact lists) is social signal but no WoT is enforced.
- **Messaging/transport.** Clients sign events and publish to multiple
  **relays** (websocket servers); readers subscribe with filters. **NIP-65**
  ("outbox model") advertises which relays a key writes to. This is
  **store-and-forward at email speed** — exactly WG's V3. [NIP-65]
- **Portability.** Very high: events are self-signed and relay-agnostic; move
  by republishing to new relays. The identity has *no* server binding.
- **Rotation/recovery.** The weak point: because `npub` *is* identity, **losing
  the key loses the identity**. **NIP-46** ("Nostr Connect" / remote signing)
  keeps the key in a **bunker** and lets clients sign remotely — the client
  never sees the nsec. **NIP-41** (key migration / "stateless key invalidation")
  is a long-debated draft and not settled. [NIP-46, NIP-41]
- **Decentralization.** Relay-mediated; anyone can run a relay; no central
  registry. Sits left-of-center on the spectrum.
- **Maturity/cost.** Growing ecosystem, many clients; relays are cheap to run.
- **WG fit/misfit.** *Fit:* key=identity=address, signed events, relay S&F all
  match V1/V3/V4 directly, and **NIP-46 is a ready-made agent-key-custody
  pattern (V5)** — the agent signs through a bunker the human controls.
  *Misfit:* native rotation/recovery (V6) is weak; no first-class
  loadable-state object (V2) beyond arbitrary event kinds.

### 2.2 Keybase — *the rotation/recovery exemplar*
- **Identity & addressing.** Username-rooted, but the **sigchain** (a per-user
  append-only signed log) anchors everything to keys. The chain records device
  additions/revocations, proofs, and follow statements. [Keybase sigchain docs]
- **Trust.** **Social proofs** (post a signed statement on Twitter/GitHub/your
  DNS) bind external identities; **following** = a signed attestation of another
  user's sigchain head, so tampering is detectable; a server-side Merkle tree
  publishes consistency. [Keybase docs/account]
- **Messaging/transport.** Server-mediated (keybase.io); chat and KBFS are E2EE
  store-and-forward.
- **Portability.** The sigchain is portable *data*, but trust is rooted in
  keybase.io's Merkle root — moving hosts wholesale is not a designed flow.
- **Rotation/recovery — the headline.** Users hold **per-device keys** (sibkeys)
  plus **paper keys**. Adding/revoking a device is a signed sigchain link; old
  links stay valid (verified against chain state at their time), and revocation
  **rotates the per-user key (PUK)** so stolen future ciphertext is useless.
  **The username/identity survives any single key loss** as long as one device
  or paper key remains. [Keybase "new key model" blog; sigchain docs]
- **Decentralization.** Central server for the Merkle root, but with verifiable
  client-side auditing — a *trust-minimized central node*.
- **Maturity/cost.** Mature but stagnant (Zoom-owned, little active dev); free.
- **WG fit/misfit.** *Fit:* the **sigchain + multi-device-key model is the best
  answer to V6/V7** in the whole survey — one identity, many revocable keys,
  identity continuity across key loss. *Misfit:* central-server trust root; not
  a P2P design.

### 2.3 DIDs (did:key / did:web / did:plc) — *the identity-document abstraction*
- **Identity & addressing.** A **Decentralized Identifier** is a URI
  (`did:method:id`) that resolves to a **DID document** listing public keys
  ("verification methods"), authentication relationships, and service
  endpoints. **did:key** encodes the pubkey directly in the DID (self-certifying,
  no resolution infra). **did:web** binds to `did:web:example.com` → a hosted
  JSON doc. **did:plc** (used by Bluesky) is a ledger of signed operations.
  [W3C DID Core 1.0; did:key; did:web; did:plc]
- **Trust.** Method-dependent: did:key=self/TOFU; did:web=DNS+CA (TLS);
  did:plc=rotation-key-authenticated operation log.
- **Messaging/transport.** None — DIDs are an identity layer; transport is left
  to other protocols (DIDComm, AT Proto, UCAN, etc.).
- **Portability.** did:key trivially portable; did:web domain-bound; did:plc
  portable via signed ops (this is what powers Bluesky account migration).
- **Rotation/recovery.** A DID document can list **many keys** and be **updated**
  (did:web by editing the hosted doc; did:plc by a rotation-key-signed
  operation that the directory validates). **The DID stays constant while keys
  rotate underneath it** — the cleanest V6 model. [did:plc spec]
- **Decentralization.** A spectrum *within the standard*: self → DNS → ledger.
- **Maturity/cost.** W3C Recommendation (2022); broad tooling and libraries.
- **WG fit/misfit.** *Fit:* the **DID-document indirection is precisely the
  "identity ≠ key" layer WG needs for V5/V6** — an agent key is one verification
  method in the doc, addable/revocable without changing the identity. A
  WG-native method (`did:wg:` over a relay/registry) is a natural design option
  for `fed-architectures`. *Misfit:* no transport/messaging; pick a method and
  you inherit its centralization (did:web=DNS, did:plc=a directory).

### 2.4 AT Protocol / Bluesky — *portability done end-to-end*
- **Identity & addressing.** Identity is a **DID** (usually did:plc); a **handle**
  (`@alice.example.com`, DNS/HTTPS-verified) is a mutable human alias that
  resolves to the DID. [atproto identity docs]
- **Trust.** DID document + handle verification; the PLC directory validates
  rotation-key-signed updates.
- **Messaging/transport.** Your data lives in a signed **repo** on a **PDS**
  (Personal Data Server); a **Relay** aggregates repos into a firehose;
  **AppViews** index it. Store-and-forward via the repo, with near-real-time
  firehose fanout. [atproto architecture]
- **Portability — the headline.** **Account migration** is a first-class flow:
  prove control via a service-auth JWT signed by the current signing key, copy
  the repo to a new PDS, and submit a **PLC operation** (gated by a rotation key
  + an email second factor) repointing the DID at the new PDS. The DID — the
  identity — is unchanged. [atproto account-migration guide]
- **Rotation/recovery.** DID doc carries **rotation keys**; a higher-priority
  **recovery key** can override the PDS within a 72-hour window if the PDS is
  malicious. **App passwords** give scoped, revocable credentials. [atproto docs]
- **Decentralization.** PDS is self-hostable (good), but the **PLC directory and
  large relays are de facto central** — "credibly decentralized," not P2P.
- **Maturity/cost.** Mature, millions of users; PDS op-cost is low.
- **WG fit/misfit.** *Fit:* AT Proto is the **strongest existing realization of
  V1+V2+V6 together** — DID identity, full host migration, rotation/recovery
  keys. *Misfit:* heavyweight (repo/relay/AppView), and the PLC directory is a
  central dependency WG would have to accept or replace.

### 2.5 Secure Scuttlebutt (SSB) — *the purest key-as-feed, and its trap*
- **Identity & addressing.** A feed **is** an ed25519 keypair; the feed ID is
  `@<base64-pubkey>.ed25519`. [SSB protocol guide]
- **Trust.** TOFU + the **follow graph drives replication** (you replicate
  friends and friends-of-friends), making trust and distribution the same
  mechanism — an implicit web-of-trust.
- **Messaging/transport.** **Gossip** replication of **append-only signed logs**;
  pubs/rooms relay for NAT traversal. Eventually-consistent — genuinely
  store-and-forward but high-latency. [SSB protocol guide]
- **Portability.** The feed is portable *data*, but it is **single-key,
  single-device by construction** — you cannot use two machines for one feed
  without forking it.
- **Rotation/recovery — the cautionary tale.** **There is none.** The key *is*
  the feed; lose it and the feed is dead; compromise it and you cannot revoke.
  "Fusion identity" was an experimental attempt to fix this and never landed.
- **Decentralization.** **Fully P2P** — the most decentralized system here, no
  servers required.
- **Maturity/cost.** Niche and declining; near-zero infra cost.
- **WG fit/misfit.** *Fit:* proves the **append-only signed-log + gossip** model
  works fully P2P. *Misfit:* SSB is the **direct demonstration of why
  key=identity *without* an indirection layer fails V6/V7** — WG must *not* copy
  the single-immutable-key design.

### 2.6 ActivityPub — *server federation, weak on keys*
- **Identity & addressing.** `@user@server`; the canonical identity is an HTTP
  **actor URI** issued by the home server. Keys exist only server-side for
  signing federation traffic. [W3C ActivityPub]
- **Trust.** Inter-server **HTTP Signatures**; you trust servers, not users.
- **Messaging/transport.** Server-to-server: deliver an activity by HTTP `POST`
  to the recipient's **inbox**; the server stores and forwards. Email-speed.
- **Portability.** Weak: a `Move` activity migrates followers but not content;
  your posts stay on the origin server; a dead server = a dead identity.
- **Rotation/recovery.** End users hold no keys; "recovery" means your instance
  admin. Identity is server-bound.
- **Decentralization.** **Server federation** — many central nodes, each
  authoritative for its users.
- **Maturity/cost.** Very mature (Mastodon, the Fediverse); moderate server cost.
- **WG fit/misfit.** *Fit:* a proven **server-federation social graph** if WG
  wanted instance-mediated identity. *Misfit:* fails V1 (no key=identity), V2
  (no portability), V5/V6 (no user-held keys) — the *opposite* end from WG's
  self-certifying vision. Useful mainly as a contrast case.

### 2.7 Matrix — *E2EE + federation, server-bound identity*
- **Identity & addressing.** `@user:homeserver` (MXID), issued by and bound to
  the homeserver. Per-device keys exist underneath. [Matrix spec]
- **Trust.** Server-to-server trust for federation; **cross-signing** lets a
  user vouch for their own devices; per-device Olm/Megolm keys for E2EE.
- **Messaging/transport.** Homeserver federation (Server-Server API); rooms are
  replicated DAGs of events; near-real-time but works S&F when servers are
  offline. [Matrix spec — federation, E2EE]
- **Portability.** Weak in practice — the MXID is server-bound; portable-account
  proposals (MSCs) remain unsolved.
- **Rotation/recovery.** **Strong at the device layer:** cross-signing, encrypted
  key backup, and **SSSS** with a recovery key/passphrase let a user recover
  E2EE history and re-establish device trust. But the *identity* (MXID) does not
  survive losing your homeserver.
- **Decentralization.** Server federation + E2EE; homeservers (Synapse) are
  **operationally heavy**.
- **Maturity/cost.** Mature; high op-cost relative to Nostr/Iroh.
- **WG fit/misfit.** *Fit:* the **cross-signing + SSSS recovery** design is a
  strong reference for V6 at the device-key layer. *Misfit:* server-bound
  identity (fails V1/V2), heavy to operate — a contrast for "what E2EE
  federation costs."

### 2.8 libp2p + IPFS / IPNS — *transport + content addressing*
- **Identity & addressing.** A **PeerId** is the multihash of a node's public
  key, embedded in its **multiaddr**. **IPNS** is a mutable pointer **named by a
  pubkey** and updated by signed records pointing at immutable CIDs. [libp2p
  peer-id spec; IPNS spec]
- **Trust.** TOFU on PeerId (it's in the address you dial); **content trust is
  intrinsic** — a CID is the hash of its content, so data is self-verifying.
- **Messaging/transport.** P2P with DHT-based routing over many transports;
  **gossipsub** for pub/sub; content fetched from whoever has it. S&F is weak
  unless data is **pinned**. [libp2p; gossipsub]
- **Portability.** **High for content** — CIDs and IPNS names are host-agnostic;
  data lives wherever it's pinned, not on a home server.
- **Rotation/recovery.** Rotating a PeerId/IPNS key yields a *new* name; no
  identity-recovery layer.
- **Decentralization.** **Fully P2P** (Kademlia DHT).
- **Maturity/cost.** Mature transport stack; full IPFS is heavy, IPNS resolution
  is slow.
- **WG fit/misfit.** *Fit:* **content-addressed, signed loadable-state (V2) is
  exactly the IPNS/CAS model** — publish a signed pointer to an immutable state
  blob; IPNS's "pubkey names a mutable record" is a clean V2 mechanism. *Misfit:*
  no human-identity or messaging layer; operational weight of full IPFS.

### 2.9 Iroh — *modern P2P, dial-by-key*
- **Identity & addressing.** A **NodeId** is a 32-byte ed25519 public key; the
  pitch is literally **"dial keys, not IP addresses."** [iroh docs]
- **Trust.** TOFU + **mutual TLS where each endpoint's pubkey is its TLS
  identity** — connections are end-to-end encrypted and mutually authenticated
  by construction. [iroh endpoints concept]
- **Messaging/transport.** **Direct P2P QUIC** with relay-assisted
  hole-punching; **iroh-docs** is an eventually-consistent signed key-value
  store and **iroh-blobs** is content-addressed transfer. Near-real-time, but
  docs give an async/S&F-like sync. [iroh; iroh-docs]
- **Portability.** High for content (blobs/docs are content-addressed,
  relay-agnostic).
- **Rotation/recovery.** NodeId rotation = a new node; no identity-recovery
  layer above transport.
- **Decentralization.** P2P with optional relays for connectivity — central
  nodes *allowed but not required*, which matches WG's stated stance exactly.
- **Maturity/cost.** **New but real — Iroh 1.0 shipped 2026** with a stable wire
  protocol and multi-language bindings; very low op-cost (relays optional).
- **WG fit/misfit.** *Fit:* if WG is Rust-native (it is), **Iroh is the most
  natural transport substrate for V3/V4** — dial-by-pubkey, encrypted QUIC,
  signed eventually-consistent docs, optional relays. *Misfit:* it's transport,
  not identity/recovery — WG must layer V5/V6 (a Keybase/DID-style sigchain) on
  top.

### 2.10 UCAN — *delegate authority, don't share keys*
- **Identity & addressing.** Principals **are DIDs**; UCAN is not an identity or
  transport but an **authorization** format. [UCAN spec]
- **Trust.** **Capability chains** — an issuer (`iss`, a DID) delegates a scoped
  capability to an audience (`aud`, a DID); validity is proven by the **proof
  chain (`prf`)** back to a root, fully **offline-verifiable**. [UCAN spec]
- **Messaging/transport.** None — tokens travel however you like (header, event
  field, embedded in a message).
- **Portability.** High — a UCAN is a self-contained, portable proof.
- **Rotation/recovery.** Rotate by re-issuing the chain; a root DID can revoke a
  branch. Excellent for *authority* rotation.
- **Decentralization.** Fully decentralized; zero infrastructure to verify.
- **Maturity/cost.** Emerging (v1.0 line); used by Fission/web3.storage lineage.
- **WG fit/misfit.** *Fit:* **UCAN is arguably the single best mechanism for V5.**
  Its founding slogan is *"sharing authority without sharing keys."* A human's
  root key issues a **scoped, expiring UCAN to an agent**, so the agent can *act*
  (sign/send within limits) **without ever holding the root signing key** —
  download-the-agent ≠ impersonate-the-human. *Misfit:* it's only the authz
  layer; needs an identity layer (DIDs) and a transport (Nostr/Iroh) around it.

### 2.11 Sigstore — *keyless signing via identity + transparency log*
- **Identity & addressing.** No long-lived signing key: you authenticate via
  **OIDC** (email / CI workload identity) and **Fulcio** issues a short-lived
  cert binding that identity to an **ephemeral** key. [Sigstore; Fulcio]
- **Trust.** A **CA (Fulcio)** plus a tamper-evident **transparency log
  (Rekor)** — anyone can audit "this identity signed this artifact at this
  time." [Rekor]
- **Messaging/transport.** None — it's signing + transparency infrastructure.
- **Portability.** N/A as an identity you carry.
- **Rotation/recovery.** **Keyless by design** — nothing to rotate or lose;
  "recovery" is just re-authenticating to your OIDC identity.
- **Decentralization.** Federated trust roots + public logs; not P2P but not a
  single vendor either.
- **Maturity/cost.** Mature; the de-facto OSS supply-chain signing standard.
- **WG fit/misfit.** *Fit:* the **"prove who signed without a long-lived
  exfiltratable key" pattern speaks directly to V5**, and a **Rekor-style
  transparency log is a strong design idea for auditing agent actions** and
  sigchain consistency. *Misfit:* depends on an OIDC IdP and a CA — a
  centralizing assumption WG's self-certifying vision wants to avoid as the
  *root*, though it's attractive as an *optional* audit layer.

### 2.12 PGP / age / minisign — *the bare signing primitives*
- **Identity & addressing.** The key (PGP fingerprint, or a raw age/minisign
  keypair) *is* the identity, but there is **no addressing/resolution layer** —
  you exchange keys out of band. [RFC 9580 OpenPGP; age; minisign]
- **Trust.** PGP = the original **web-of-trust** (key signing parties,
  signatures on signatures); age/minisign deliberately drop WoT for TOFU /
  out-of-band simplicity.
- **Messaging/transport.** None — these sign and/or encrypt files and messages.
- **Portability.** High — keys are files you carry; but no publish/resolve.
- **Rotation/recovery.** **PGP supports subkeys + revocation certificates** (a
  master key certifies rotatable subkeys — a primitive form of the Keybase
  model); age/minisign have no rotation/recovery story.
- **Decentralization.** **Fully decentralized** (no infrastructure at all).
- **Maturity/cost.** PGP very mature but notoriously clunky; age/minisign modern,
  minimal, widely liked.
- **WG fit/misfit.** *Fit:* **age/minisign are excellent *primitives*** for
  signing WG's events and loadable-state blobs (V2/V3) with minimal complexity;
  **PGP's master-key→subkey delegation is a conceptual ancestor of V5/V6**.
  *Misfit:* none provide identity resolution, messaging, or recovery as a system
  — they are ingredients, not architectures.

### 2.13 Signal Protocol — *the async-messaging gold standard*
- **Identity & addressing.** In the Signal app, identity is phone-number +
  server-bound; the *protocol* itself is key-based (a long-term identity key plus
  published one-time **prekeys**). [Signal X3DH; Double Ratchet]
- **Trust.** TOFU + out-of-band **safety-number** comparison.
- **Messaging/transport — the headline.** **X3DH** establishes a shared secret
  with an **offline** recipient using their prekey bundle; the **Double Ratchet**
  then gives forward secrecy and post-compromise security per message. The
  server relays **sealed-sender** envelopes — true **async, E2EE,
  store-and-forward**. [X3DH; Double Ratchet; Sealed Sender]
- **Portability.** Low — the *service* is phone-number/server-bound even though
  the *protocol* is portable.
- **Rotation/recovery.** Strong forward secrecy via ratcheting; an identity-key
  change triggers a safety-number re-verification.
- **Decentralization.** Central (Signal servers), but content is E2EE.
- **Maturity/cost.** Gold-standard cryptography, massive adoption, audited.
- **WG fit/misfit.** *Fit:* **the X3DH + Double Ratchet + sealed-sender design is
  the reference to copy for V3** when WG wants confidential agent↔human messaging
  (the per-recipient-encryption / ACL layer, gap-analysis R24). *Misfit:* the
  *service* model (phone numbers, central server) is not WG's identity model —
  borrow the crypto, not the account system.

### 2.14 Farcaster — *the agent-key-custody exemplar*
- **Identity & addressing.** An identity is an **fid** (a number) registered in
  an **on-chain Key Registry** (on Optimism). A user's **custody key** (an
  Ethereum keypair) owns the fid; day-to-day posting is done by **signers (app
  keys)** that the custody key authorizes. [Farcaster docs — accounts, signers]
- **Trust.** Root of trust is the on-chain registry: the custody key's
  authorization of a signer is a verifiable on-chain fact.
- **Messaging/transport.** **Hubs** gossip-replicate signed messages (over
  libp2p) — eventually-consistent store-and-forward. [Farcaster hubs]
- **Portability.** High — messages are signed and hub-replicated; the fid and
  its signer set are portable across hubs and clients.
- **Rotation/recovery — directly relevant.** The custody key can **add and remove
  signers** at will; **losing or revoking a signer does not lose the fid**, and a
  recovery address can be configured. This is a clean separation of *root
  identity* from *operational keys*. [Farcaster — signers, recovery]
- **Decentralization.** On-chain registry (a shared central-ish ledger) + P2P
  hubs — a hybrid that, notably, matches WG's "P2P-leaning, central nodes
  allowed."
- **Maturity/cost.** Growing; hub op-cost moderate; registry writes cost gas.
- **WG fit/misfit.** *Fit:* **Farcaster is the closest existing answer to V5 +
  V7.** Its *custody-key → signer (app-key)* split is **exactly** WG's
  agent-key-custody requirement: a human (or host) holds the custody/root key; an
  agent is issued a **signer** that lets it *act* but is **not** the root key, and
  **downloading/copying a signer ≠ owning the identity** (the registry, not key
  possession alone, is authoritative). *Misfit:* the on-chain registry implies a
  blockchain dependency WG likely wants to avoid — but the *pattern* transposes
  cleanly onto a sigchain/DID-doc registry.

### 2.15 WireGuard / Tailscale — *key as network identity*
- **Identity & addressing.** In WireGuard a peer **is** its Curve25519 public
  key; routing/allowed-IPs are keyed by it. Tailscale layers a coordination
  plane that maps a node key to a user/device identity. [WireGuard whitepaper;
  Tailscale docs]
- **Trust.** WG = manual public-key exchange (TOFU); Tailscale = **OIDC SSO** +
  a coordination server that distributes the key↔node mapping.
- **Messaging/transport.** Encrypted UDP tunnel via the **Noise IK** handshake —
  **real-time** networking, not async messaging. [WireGuard/Noise]
- **Portability.** Low — keys are node-bound; Tailscale's node identity is
  coordination-server-bound.
- **Rotation/recovery.** WG = manual re-key (and update every peer); Tailscale =
  rotate the node key while the **SSO identity persists**.
- **Decentralization.** WG = fully P2P (no central anything); Tailscale =
  coordination-server-central (with P2P data plane).
- **Maturity/cost.** WireGuard is in the Linux kernel and ubiquitous; Tailscale
  is a mature SaaS.
- **WG fit/misfit.** *Fit:* proves **key-as-identity at the network layer** and,
  crucially, **Tailscale's split of "human SSO identity" from "rotatable node
  key" is a small-scale mirror of V5/V6** — the identity outlives the key.
  *Misfit:* real-time tunneling is the wrong latency/abstraction for WG's
  email-speed messaging; Tailscale's coordinator is a central dependency.

### 2.16 SSH keys / ssh-agent / SSH-CA — *the custody primitive*
- **Identity & addressing.** A key is an identity (`authorized_keys`,
  `allowed_signers`); SSH certificates (SSH-CA) bind a key to a principal name
  with a validity window. [OpenSSH; ssh-keygen `-Y sign`]
- **Trust.** TOFU via `known_hosts`; `allowed_signers` files for SSH signatures;
  or a CA model with short-lived certs.
- **Messaging/transport.** None directly — SSH is auth + a transport for
  commands; `ssh-keygen -Y sign/verify` is a general signature primitive.
- **Portability.** High — keys are files; SSH signatures are portable artifacts.
- **Rotation/recovery.** Rotate `authorized_keys`/certs; SSH-CA issues
  short-lived certs so rotation is automatic on expiry.
- **Decentralization.** Decentralized per-host trust; no central registry needed.
- **Maturity/cost.** Ubiquitous, battle-tested, free.
- **WG fit/misfit.** *Fit:* **`ssh-agent` is the canonical "use a key without
  holding it" mechanism** — the agent process holds the private key and
  *forwards the ability to sign* over a socket; **agent forwarding** lets a
  remote process sign *as you* without ever receiving the key. This is a concrete,
  deployable template for **V5** (host holds the signing key; the agent process
  gets signing *capability*, not the key) and pairs naturally with UCAN-style
  scoping. *Misfit:* no identity-resolution or messaging layer of its own.

### 2.17 FIDO2 / WebAuthn (passkeys) — *non-exportable key custody* (bonus)
- **Identity & addressing.** Per-relying-party credential public keys; the
  **private key is generated in and never leaves** a hardware authenticator (or a
  platform secure enclave). [W3C WebAuthn]
- **Trust.** Optional **attestation** (a CA chain proving authenticator make) +
  RP-bound registration (TOFU per site).
- **Messaging/transport.** None — it's an authentication ceremony.
- **Portability.** Low for hardware-bound keys; **synced passkeys** add
  cloud-mediated portability with provider trust.
- **Rotation/recovery.** Register a second authenticator as backup; synced
  passkeys recover via the platform account.
- **Decentralization.** Per-RP central (each site is its own trust root).
- **Maturity/cost.** Mature, OS/browser-native, free.
- **WG fit/misfit.** *Fit:* the **hardware-backed, non-exportable private key is
  the strongest possible V5 custody guarantee** for the *human's* root key — the
  signing key physically cannot be downloaded, so the "download ≠ impersonation"
  property is enforced by hardware. *Misfit:* per-RP scoping and no global
  identity make it a *protector of the root key*, not a federation protocol —
  useful as the human's key-custody backstop, not the network layer.

---

## 3. Decentralization spectrum placement

```
 FULLY P2P  <------------------------------------------------------------>  CENTRAL NODE
 (no servers)        (relay/gossip-mediated)        (federated servers)     (single authority)

 SSB ── WireGuard(raw) ── libp2p/IPFS ── Iroh ── Nostr ── Farcaster ── AT Proto ── Keybase ── Matrix ── ActivityPub ── Tailscale ── Sigstore/Signal
  │         │                │            │        │         │            │           │          │           │            │             │
 gossip   manual          DHT/CAS      P2P-QUIC  relays   on-chain      PDS self-   Merkle    homeserver  instance    coordination  central
 only     key xchg                    +relays   (open)   reg+P2P hubs   host +PLC   server    federation  federation  server+SSO    server+CA

 Cross-cutting (identity/authz layers, no inherent placement — they ride on a transport):
   DIDs (did:key=left ▸ did:web=mid ▸ did:plc=right) · UCAN (offline-verifiable, placement = wherever tokens travel)
   PGP/age/minisign (no infra = far left as primitives) · SSH/ssh-agent (per-host, decentralized) · FIDO2 (per-RP, right)
```

**Reading the spectrum for WG.** WG's stated stance — *"leaning toward
decentralization but allowing central nodes"* — places its target zone in the
**Iroh ↔ Nostr ↔ Farcaster** band: relay/gossip-mediated transport with
self-certifying keys, where central nodes are an *optimization* (connectivity,
discovery, archival) rather than the *root of trust*. The fully-P2P left
(SSB/IPFS) is attractive for portability but weak on identity continuity; the
server-federation right (ActivityPub/Matrix) gives up V1 (key=identity)
entirely.

---

## 4. Special focus — the two cruxes

### 4.1 Agent key custody (V5)

> *A portable/downloadable identity must be **public-identity + signed-state**,
> NOT the signing key — so download ≠ impersonation. The signing key stays
> host-held.*

Ranked by how directly each system solves it:

| Rank | System | Mechanism | Why it fits V5 |
|---|---|---|---|
| ★★★ | **Farcaster** | Custody (root) key authorizes **signers / app keys** via an on-chain registry | The portable artifact (an fid + its message history + a signer) lets an agent *act* without the custody key; copying a signer ≠ owning the identity (the registry is authoritative). The single cleanest real-world V5. |
| ★★★ | **UCAN** | Root DID **delegates a scoped, expiring capability** to an agent DID | "Sharing authority without sharing keys" *is* the design goal. Host keeps the root key; the agent gets a capability it can wield within bounds and that can be revoked/expired. |
| ★★★ | **SSH ssh-agent** | Agent process holds the key; **forwards signing capability** over a socket | The deployable template: the host holds the signing key; the agent gets *use*, never the bytes. Agent forwarding = "sign as me, remotely, without the key." |
| ★★☆ | **DIDs** | DID doc lists **many verification methods**; agent key = one entry, revocable | Identity (the DID) is decoupled from any single key; an agent key is a published, revocable method — download the DID doc and you get *verifiable identity*, not signing power. |
| ★★☆ | **Keybase** | Per-device **sibkeys**; identity = sigchain, not any one key | An agent could be "a device"; its key is added/revoked in the sigchain without touching the identity. |
| ★★☆ | **Nostr (NIP-46)** | **Bunker** holds the nsec; client/agent signs *remotely* | The agent never holds the key; the bunker (host) signs on request. Custody by remote-signer. |
| ★★☆ | **FIDO2/WebAuthn** | Private key **non-exportable** from hardware | Hardware *enforces* "download ≠ impersonation" for the **human's root** key — the custody backstop, not the agent's working key. |
| ★☆☆ | **AT Proto** | **App passwords** / scoped credentials + rotation/recovery keys | Scoped, revocable secondary credentials approximate delegated agent access; signing keys live in the PDS. |
| ☆☆☆ | **SSB, ActivityPub, raw age/minisign** | — | **Fail V5**: SSB's single key *is* the feed; ActivityPub users hold no keys; age/minisign have no delegation. |

**Synthesis for V5.** The winning pattern is a **two-tier key hierarchy**:
a **root/custody key** (host-held, ideally hardware-backed à la FIDO2) that
**issues scoped, revocable delegations** (Farcaster *signers* / UCAN
*capabilities* / SSH-agent *forwarded signing* / Keybase *sibkeys*) to agents.
The **portable/downloadable identity** is then *(public identity record +
signed state + the set of currently-authorized delegate keys)* — never the root
private key. WG should adopt **Farcaster's signer model or UCAN delegation as
the conceptual core of V5**, with the root key protected by an SSH-agent- or
FIDO2-style custody boundary.

### 4.2 Rotation / recovery without losing identity continuity (V6)

> *If a key is lost or compromised, the identity must survive.*

| Rank | System | Mechanism | Identity continuity? |
|---|---|---|---|
| ★★★ | **Keybase** | Sigchain: add/revoke device & **paper keys**; PUK rotates on revoke | **Yes** — username/identity survives as long as one key remains; old links stay valid at their chain position. |
| ★★★ | **AT Proto** | DID doc **rotation keys** + a **recovery key** (72h override) + app passwords | **Yes** — DID (the identity) is constant; keys rotate underneath; recovery key defends against a hostile PDS. |
| ★★★ | **DIDs (did:plc/did:web)** | Update the DID document (signed op / hosted-doc edit); DID is stable | **Yes** — the indirection layer: identity = the DID, keys = swappable contents. |
| ★★☆ | **Matrix** | Cross-signing + encrypted **key backup** + **SSSS** recovery key | **Partial** — device keys recover well; but the *MXID* dies with the homeserver. |
| ★★☆ | **Farcaster** | Custody key swaps signers; **recovery address** can move the fid | **Yes for the fid** — operational keys rotate freely; recovery address adds custody recovery. |
| ★★☆ | **PGP** | Master key certifies **rotatable subkeys** + revocation certs | **Partial** — subkeys rotate, but losing the master is fatal unless a revocation cert was pre-generated. |
| ★☆☆ | **Tailscale** | Rotate node key, **SSO identity persists** | **Yes (at its scale)** — node key ≠ identity; but identity lives in the coordinator/IdP. |
| ★☆☆ | **Nostr** | NIP-46 isolates the key; **NIP-41** key-migration is unsettled draft | **Weak** — no accepted continuity mechanism yet; `npub` *is* identity. |
| ☆☆☆ | **SSB, age/minisign, raw libp2p/Iroh NodeId, WireGuard** | — | **No** — key loss = identity loss; rotating the key creates a *new* identity. |

**Synthesis for V6.** Every system that achieves continuity does so via the
**same trick: an indirection layer where the identity is a *stable name* and
keys are *revocable contents* of a signed, append-only record.** Keybase calls
it a **sigchain**; AT Proto/DID call it a **DID document with rotation keys**;
Farcaster calls it an **on-chain key registry**. Systems where the key *is* the
identity with no indirection (SSB, Nostr-as-shipped, raw libp2p/Iroh/WireGuard)
**cannot** offer V6 — this is the single most important lesson for WG's design.
**WG must introduce a sigchain/DID-document-style indirection** so that
`identity-name → {current key set}` is a signed, rotatable mapping, not an
equation.

---

## 5. Synthesis — what WG should borrow from whom

The survey points at a **layered composition**, not a single off-the-shelf
system (developed further in `fed-architectures`):

| WG layer | Best prior art to borrow | Why |
|---|---|---|
| **Self-certifying identity + addressing (V1)** | **Nostr `npub`** / **Iroh NodeId** / **did:key** | Pubkey-as-identity-and-address, no central registry — directly the vision. |
| **Identity *continuity* / rotation (V6)** | **Keybase sigchain** + **DID-document/did:plc rotation keys** | The indirection layer that lets keys rotate while the identity persists. |
| **Agent key custody (V5)** | **Farcaster signers** / **UCAN delegation** / **ssh-agent** | Two-tier: host-held root key delegates scoped, revocable powers; download ≠ impersonation. |
| **Async store-and-forward messaging (V3)** | **Nostr relays** (transport) + **Signal X3DH/Double Ratchet** (confidentiality, R24) | Email-speed signed events; sealed E2EE when privacy is needed. |
| **Loadable / portable state (V2)** | **IPNS/CAS** + **AT Proto signed repos** | Content-addressed, signed state blobs named by a pubkey-controlled mutable pointer; full host migration as proof-of-concept. |
| **Transport substrate (V3/V4)** | **Iroh** (Rust-native, dial-by-key QUIC, optional relays) | Matches WG's Rust stack and "P2P-leaning, central nodes allowed." |
| **Optional audit / transparency** | **Sigstore Rekor** | Tamper-evident log of agent actions / sigchain heads. |

**Headline takeaway.** No existing system satisfies all seven WG requirements
alone. The **two systems that get closest to WG's hybrid human+agent + custody
+ continuity needs are Keybase (continuity/multi-key) and Farcaster
(custody/signers)** — neither is fully P2P, both prove the *indirection +
delegation* pattern WG needs. The **transport/identity-addressing pieces are
best served by the Nostr/Iroh/DID family**. WG's design space is therefore a
**composition**: a sigchain/DID-style continuity layer + a Farcaster/UCAN-style
custody layer, riding on a Nostr/Iroh-style self-certifying, relay-tolerant
transport.

---

## 6. Sources (specs & primary docs)

- **Nostr** — NIPs repo: <https://github.com/nostr-protocol/nips> · NIP-01 (events/keys), NIP-05 (`name@domain`), NIP-65 (relay list / outbox), NIP-46 (remote signing / "bunker"): <https://nips.nostr.com/46>, NIP-41 (key migration, draft).
- **Keybase** — sigchain: <https://keybase.io/docs/sigchain> · account/key model: <https://book.keybase.io/account> · "Keybase's New Key Model": <https://keybase.io/blog/keybase-new-key-model>.
- **DIDs** — W3C DID Core 1.0 (Rec, 2022): <https://www.w3.org/TR/did-core/> · did:key: <https://w3c-ccg.github.io/did-method-key/> · did:web: <https://w3c-ccg.github.io/did-method-web/> · did:plc: <https://github.com/did-method-plc/did-method-plc>.
- **AT Protocol / Bluesky** — <https://atproto.com/> · identity: <https://atproto.com/specs/did> · account migration: <https://atproto.com/guides/account-migration>.
- **Secure Scuttlebutt** — protocol guide: <https://ssbc.github.io/scuttlebutt-protocol-guide/>.
- **ActivityPub** — W3C Rec (2018): <https://www.w3.org/TR/activitypub/>.
- **Matrix** — spec: <https://spec.matrix.org/> (Server-Server API, end-to-end encryption / cross-signing / key backup / SSSS).
- **libp2p / IPFS / IPNS** — libp2p specs: <https://github.com/libp2p/specs> (peer-id, gossipsub) · IPNS: <https://specs.ipfs.tech/ipns/ipns-record/> · CID/CAS: <https://docs.ipfs.tech/concepts/content-addressing/>.
- **Iroh** — <https://www.iroh.computer/> · endpoints/dial-by-key: <https://docs.iroh.computer/concepts/endpoints> · "Dial by NodeID": <https://www.iroh.computer/blog/iroh-dns> · repo: <https://github.com/n0-computer/iroh> (Iroh 1.0, 2026).
- **UCAN** — spec: <https://github.com/ucan-wg/spec> · site: <https://ucan.xyz/specification/>.
- **Sigstore** — <https://www.sigstore.dev/> · Fulcio & Rekor: <https://docs.sigstore.dev/>.
- **OpenPGP / age / minisign** — OpenPGP RFC 9580: <https://www.rfc-editor.org/rfc/rfc9580> · age: <https://age-encryption.org/> · minisign: <https://jedisct1.github.io/minisign/>.
- **Signal Protocol** — X3DH: <https://signal.org/docs/specifications/x3dh/> · Double Ratchet: <https://signal.org/docs/specifications/doubleratchet/> · Sealed Sender: <https://signal.org/blog/sealed-sender/>.
- **Farcaster** — protocol docs: <https://docs.farcaster.xyz/> (accounts/fid, signers/app-keys, Key Registry, Hubs, recovery).
- **WireGuard / Tailscale** — WireGuard whitepaper: <https://www.wireguard.com/papers/wireguard.pdf> · Tailscale: <https://tailscale.com/blog/how-tailscale-works>.
- **SSH / ssh-agent** — OpenSSH: <https://www.openssh.com/manual.html> · `ssh-keygen -Y sign` (SSH signatures) and agent forwarding (`ssh-agent(1)`).
- **FIDO2 / WebAuthn** — W3C WebAuthn Level 2/3: <https://www.w3.org/TR/webauthn/> · FIDO Alliance: <https://fidoalliance.org/specifications/>.

---

*Wave-1 gather phase complete. `fed-requirements` (3/6) should turn the V1–V7
dimensions here into a hard-questions catalog; `fed-architectures` (4/6) should
develop the layered composition in §5 into candidate decentralized ↔ central ↔
hybrid designs; `fed-adversarial` (5/6) should threat-model the §4 custody &
rotation mechanisms.*
