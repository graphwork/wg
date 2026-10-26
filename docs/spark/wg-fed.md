<!-- provenance: extracted verbatim from AGENTS.md (sections now stubbed there) by
     task `doc-archive-policy`. Text is unchanged apart from heading level; the
     stub in AGENTS.md/CLAUDE.md links here. See docs/archive/POLICY.md. -->

# WG-Fed identity (`wg identity`) — the federation substrate (Waves 3–6)

*Extracted from `AGENTS.md` § "WG-Fed identity (`wg identity`) — federation Wave 3 spark".*

WG-Fed is the cross-WG federation substrate (self-certifying key identity +
signed messages + portable signed state) decided in `docs/federation-study/06`
and ratified in `docs/ADR-fed-001..004-*.md`. The **Wave-3 spark PoC** is the
first real federation code: `src/identity/` (`mod.rs` = `WG_FED_COMPAT_VERSION`
+ loud-fail handshake + canonical-JSON BLAKE3 content-addressing; `keys.rs` =
ed25519/X25519 gen + the `wgid:` / `did:key` multibase + the custody boundary;
`sigchain.rs` = `genesis`/`add_key` + `verify`; `envelope.rs` =
`IdentityRecord`/`StateSnapshot`/`SignedEvent` sign/verify/seal). `Message`
gained `from`/`to`/`sig`/`refs` (all `#[serde(default)]`, backward compatible).

An identity is a **self-certifying `wgid:<multibase-ed25519-pubkey>`** (a pure
prefix-swap with `did:key:`) backed by an append-only signed sigchain. The
**three-tier key hierarchy** (root / signer / encryption) lives in `wg secret`'s
keystore behind an **ssh-agent-style "sign this digest" custody boundary**
(`identity::keys::Custodian` — `sign_digest`/`agree` only; **the root private
key is never returned, exported, or written to any record/file/env**).
Verification is **never central**: a pure local signature check rooted at the
genesis pubkey embedded in the address. The custody split is what makes
"download ≠ impersonation" hold — possessing a published bundle confers no
ability to author as that identity.

```
wg identity new <name>                    # mint (root → wg secret; emits wgid: + signed IdentityRecord)
wg identity publish <name> --store <L>     # publish IdentityRecord + sigchain + a StateSnapshot to a dumb, untrusted location
wg identity fetch <wgid> --store <L> [--save <name>]   # fetch + verify OFFLINE by wgid alone
wg identity send --from <name> --to <wgid> --store <L> --body <text> [--seal]   # signed (optionally sealed) cross-graph event
wg identity poll <name> --store <L>        # receive + authenticate by key (forged "from" / tampered events rejected)
wg identity show <name> | wg identity list | wg identity verify <file> [--store <L>]
```

The third location `L` is a dumb, untrusted bytes store (the spark uses a
directory / `file://` path; the HTTP-inbox and relay transport rungs harden in
Wave 4 per ADR-fed-002). The keystore is `$HOME`-relative, so two WG instances
on one host are isolated by `HOME` alone. Compatibility is gated by
`WG_FED_COMPAT_VERSION` in `src/identity/mod.rs` (loud-fail on incompatible
mismatch, mirroring `WG_AGENCY_COMPAT_VERSION` / `WG_PI_PLUGIN_COMPAT_VERSION`).
The seven-step end-to-end proof is pinned by
`tests/smoke/scenarios/federation_spark_two_graphs.sh` (`owners = [wg-fed-spark]`).

## WG-Fed Wave 4 — cross-graph addressing + transport hardening

Wave 4 (`docs/federation-study/06` §5, `docs/ADR-fed-002-transport.md`) promotes the
spark's single dumb-directory rung to a **real network transport** and adds the S-3
freshness defense. The transport stays **untrusted** end-to-end (every byte is
signed/optionally-sealed and self-verifying); no relay or node is mandatory.

- **Transport abstraction** (`src/identity/transport.rs`) — a `FedStore` trait (put/get
  content-addressed objects, head pointers, store-and-forward inbox events, freshness
  attestations) with two wire-compatible rungs: `FileStore` (the spark's directory,
  unchanged layout) and `HttpStore` (a `reqwest::blocking` client). `open_store(loc)`
  routes by scheme — `http(s)://` → node, anything else → directory/`file://`. **Every
  `wg identity … --store <L>` now accepts an `http://` node URL** transparently.
- **The WG node inbox** (`src/identity/node.rs`, `wg fed-node serve --addr H:P
  [--store DIR]`) — the **default rung** (ADR-fed-002 §D1 rung 1), a dependency-light
  HTTP/1.1 store-and-forward server over `TcpListener` exposing the `FedStore` surface
  under `/wgfed/v1/…`. It holds `wgid:`-addressed `SignedEvent`s for **offline**
  recipients until they poll.
- **`wg msg --to wgid:` between graphs** — `wg msg send --to <wgid|peer> --from
  <identity> [--seal] [--store <url>]` and `wg msg poll --as <identity> [--store
  <url>] [--require-fresh <class>]` route a signed event over the node inbox. `--to`
  resolves the delivery node via the cascade; the local `wg msg send <task> …` path is
  unchanged when `--to` is absent.
- **Resolution cascade** (`federation::resolve_peer_endpoint`, ADR-fed-001 §D5) —
  `PeerConfig`/`Remote` gain `wgid` + `endpoints`; resolution is **cached signed
  endpoint record → optional directory hint → DHT (deferred past Wave 4)**. Path-based
  `federation.yaml` peers keep resolving (`resolve_peer`) alongside key-based ones;
  `wg peer add <name> [<path>] [--wgid W --endpoint U …]` registers either kind.
- **Freshness attestations** (`src/identity/freshness.rs`, S-3) — `publish`/`attest`
  emit a signed `valid-as-of T, expires T+Δ, seq` over the current head;
  `wg identity check-fresh <wgid> --store <L> --class routine|high-value` and
  `--require-fresh` on poll **re-fetch it and fail closed on stale** (high-value Δ ≤
  15 min, routine ≈ 24 h, ±5 min skew) with a **monotonic `seq`** rollback backstop.
  `--fresh-ttl` (negative = already-expired) exercises the stale path.

The cross-graph end-to-end proof is pinned by
`tests/smoke/scenarios/federation_node_inbox_cross_graph.sh` (`owners = [fed-wave4]`):
two `wg fed-node` daemons exchanging a signed+sealed `wg msg --to wgid:` to an offline
recipient, a forged sender rejected, high-value freshness failing closed on stale, and
verification surviving the sender's origin node going offline (cached sigchain).

## WG-Fed Wave 5 — portable state + recovery (rotate/revoke/recover, fork-vs-same-self, S-5)

Wave 5 (`docs/federation-study/06` §5 Wave 5, `docs/ADR-fed-003` §D4/§D5/§D6 +
`docs/ADR-fed-004` §D6) makes an identity **portable (V2)** and **recoverable (V6)**,
and makes the fork-vs-same-self continuity boundary *cryptographically unskippable*.
The `wgid:` address (the **genesis** root pubkey) never changes; the **active** signing
root rotates underneath it. `WG_FED_COMPAT_VERSION` is bumped `0.1.0 → 0.2.0` (the
`rotate_root` verification semantics changed — a 0.1.x peer must fail loud, not silently
mis-verify a rotated chain).

- **Sigchain ops** (`src/identity/sigchain.rs`) — `rotate_root` (succession: the current
  active root signs in the next), `revoke_key` (durable, self-verifying), and **layered
  recovery**: `rotate_root_via_recovery_key` (offline recovery key, the node-default
  owner backstop) and `rotate_root_via_guardians` (M-of-N guardian quorum, the node-less
  ceremony that defuses Fatal A-4). `verify` replays the chain to track `active_root` and
  keeps **add_key / revoke_key / succession-rotate root-locked** (the hydra kill, S-4);
  recovery installs a *new root the recoverer possesses*, authorized by a control the
  downloader lacks (the recovery key or quorum) — never a surviving-delegate `add_key`.
  `RecoverySlot` gains `recovery_key`; `validate_node_less` mandates paper-key + M-of-N.
- **Fork vs same-self** (ADR-fed-003 §D4) — a genesis may cite a `ParentRef`; `wg identity
  fork --from <downloaded> --as <child>` mints a **NEW** `wgid` (a verifiable child, NOT
  the parent) — the default "download = fork". Continuing as the **same** identity needs a
  root-signed `add_key`: `wg identity enroll-signer <name>` — which a downloaded, key-less
  bundle **cannot** produce (no root in custody), so same-self is unskippable by design.
- **S-5 loadable-state safety** (`src/identity/state_safety.rs`, ADR-fed-004 §D6) — a
  loaded `StateSnapshot` is **UNTRUSTED INPUT**: a signature proves *who wrote* it, never
  that it is *safe to load*. `wg identity load-state <name> --store <L> [--from <wgid>]
  [--author-trust …]` runs the fail-closed pipeline — CAS integrity → signature/provenance
  → `model_binding` → kind dispatch (transparent scan / opaque contain / unknown degrade) →
  the per-kind AI-input-safety scan (structural / embedded-secret / prompt-injection) →
  provenance-gate by `trust_level`. **Auto-load only for `same-self` OR `(cross-self ∧
  Verified ∧ transparent ∧ scan-clean)`**; everything else is human-in-loop, an `Unknown`
  cross-self author is refused, and a hard scan hit blocks unconditionally.
- **CLI** — `wg identity {new --recovery|--node-less|--guardian|--threshold, rotate,
  revoke, recover, fork, enroll-signer, load-state}`; `publish --state-text` seeds a
  custom (e.g. poisoned) cache for the S-5 demo.

The end-to-end proof is pinned by
`tests/smoke/scenarios/federation_recovery_portable_state.sh` (`owners = [fed-wave5]`):
rotate (address unchanged) → same-self enroll → revoke → recover via the offline recovery
key → download=fork enforced (same-self on a downloaded bundle refused) → S-5 gate
(same-self auto-loads, unknown cross-self refused, injection-bearing cache hard-blocked).
Guardian (node-less) recovery is exercised by the `recover_via_guardian_quorum` library
test.

## WG-Fed Wave 6 — encryption=ACL + UCAN delegation (COMPLETES WG-Fed; unblocks exec spark)

Wave 6 (`docs/federation-study/06` §5 Wave 6, `docs/ADR-fed-003` §D2/§D3 + the HQ4
encryption-as-ACL decision) realizes **confidentiality (R24)** and **structural,
attenuating-only delegation**, completing WG-Fed. `WG_FED_COMPAT_VERSION` is bumped
`0.2.0 → 0.3.0` (the wire gained per-recipient sealed envelopes + sealed-sender + the
off-chain UCAN; a 0.2.x peer cannot verify the new sealed events, so it must fail the
handshake loudly rather than mis-handle them, S-7).

- **Encryption = ACL** (`src/identity/envelope.rs`) — a **per-recipient sealed
  envelope** (`SignedEvent.enc_multi`, `SealedEnvelope`): the body is encrypted **once**
  under a fresh content-encryption key (CEK), and that CEK is X25519-wrapped to **each**
  recipient. **The `to` set IS the access-control list** — every member unwraps the CEK
  and decrypts; a third party holding the ciphertext but no listed encryption key is
  locked out. `wg identity send --to A --to B --seal` (repeatable `--to`) seals to the
  set. A **sealed-sender** option (`--sealed-sender`) anonymizes the outer `from` to
  `wgid:anon` so a relay learns nothing — the real author + its signature ride *inside*
  the seal and are recovered + verified only by a recipient (FR-S4). Static recipient
  keys (no forward secrecy) on the offline path — FS does not compose with send-to-offline
  (**S-6**); MLS/Double-Ratchet is online/long-lived-groups only and stays deferred.
- **UCAN delegation** (`src/identity/custody.rs`) — a `Capability` is a signed, scoped,
  **expiring** "agent X (`aud`) may act for principal Y (`iss`), scope S, until T" token,
  **off-chain** (not appended to the sigchain — the chain authorizes *keys*, the UCAN
  authorizes *actions*). `issue_root` / `delegate` / `verify` / `Revocation`. The
  integrity invariants are **dial-independent** (§D3): delegation **never shares a private
  key**; **sub-delegation is attenuating-only** (child scope ⊆ parent, expiry ≤ parent —
  the structural **hydra** kill, verified at issue *and* at verify); `add_key`/`rotate_root`
  stays **root-locked** (Wave 5); **revocation is issuer-subtree** (revoking a parent kills
  the whole delegated subtree); every action is attributable to `iss`+`aud` (NFR-7).
  Verification is offline — `verify` resolves each issuer's sigchain via a resolver
  closure, no central authority. **A short TTL + revocation makes a stolen signer
  near-worthless after expiry.** CLI: `wg identity delegate` (root grant or
  `--parent`-attenuating sub-delegation), `verify-cap`, `revoke-cap`.
- **The leash dial** (`custody::LeashPolicy`, ADR-fed-003 §D2 — Erik's trust-default
  amendment) — authority is **broad/long by birth default** (agents and humans are
  first-class peers, not tools), **slack unless tightened**. Tightening is
  **environment-driven policy** (`WG_FED_LEASH_MAX_TTL_SECS` clamps TTL,
  `WG_FED_LEASH_SCOPE` is a `can@with` ceiling), **never the birth default**, and
  **humans are never leashed**. Custody (root stays with the custodian) ≠ authority scope
  (the dial); the integrity invariants hold at *every* dial setting.
- **Directory/node convenience (C-tier hint)** — revocations are published to the store's
  reserved `wgfed:revocations` list and discovered by `verify-cap`, but each is
  re-verified (signature + issuer-authorization) and **never overrides self-verification**.

The end-to-end proof is pinned by
`tests/smoke/scenarios/federation_acl_ucan_delegation.sh` (`owners = [fed-wave6]`):
multi-recipient ACL (both recipients decrypt, a third party with the ciphertext cannot)
→ sealed-sender (`from` anonymized, recipient still authenticates the real author) → UCAN
issue/verify/attenuate (widening refused) → expiry fail-closed → issuer-subtree revocation
→ leash slack-by-default vs env-tightened → download ≠ capability-issuance. **WG-Fed is
complete; the execution-federation spark (gated on this) can now proceed.**
