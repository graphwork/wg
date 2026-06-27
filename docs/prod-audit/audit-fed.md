# Production-Readiness Audit — WG-Fed (identity / crypto / transport / custody / state)

**Task:** `audit-fed` · **Scope:** `src/identity/{mod,keys,sigchain,envelope,custody,freshness,state_safety,transport,node}.rs`,
`src/commands/identity_cmd.rs`, `src/federation.rs`, `src/trust.rs`, `src/messages.rs` (WG-Fed deltas).
**Decisions checked against:** `docs/ADR-fed-001..004`, `docs/federation-study/05` (threats S-1..S-7, Fatals A-4/D-1), `docs/federation-study/06`.
**Stance:** skeptic — assume *not* production-ready; prove what's missing. **Analysis only; no production code changed.**

---

## TL;DR verdict

WG-Fed is an **unusually solid cryptographic core wrapped in a deliberately-thin, non-production transport/ops shell.**
The *offline-verifiable identity algebra* — self-certifying `wgid:`, hash-linked signed sigchain, the hydra-locked
key-set mutation rule, attenuating-only UCAN delegation, the fail-closed S-5 state gate, tiered freshness — is
**genuinely production-grade logic, adversarially unit-tested (66 identity tests).** What is **not** production-ready is
everything that turns that algebra into a running federated system: **the node (no auth, trivial DoS), the custody
boundary (plaintext-at-rest, in-process), the compat handshake (defined but never invoked on the wire), peer discovery
(DHT deferred), and several ADR properties that exist as *slots* but whose payloads are stubs (`model_binding`
enforcement, the content scanner, actual state consumption).**

A blunt way to say it: **the math is ready; the daemon is a demo.** Shipping WG-Fed as-is would expose a network
service (`wg fed-node serve`) that any unauthenticated peer can OOM, flood, or head-squat, and a "loud-fail compatibility
handshake" that never actually runs.

Severity legend: **BLOCKER** = must fix before any networked deployment · **MAJOR** = must fix before relying on the
property in production · **MINOR** = hardening / known-deferral / documentation.

---

## Findings table

### A. Identity, addressing & custody (`keys.rs`, `mod.rs`)

| # | Capability | Status | Decision it should meet | Severity | Effort |
|---|------------|--------|--------------------------|----------|--------|
| A1 | `wgid:`/`did:key:` self-certifying address (multibase, `verify_strict`, prefix-swap) | **prod** | ADR-fed-001 §D1/§OQ1/§OQ2 | — | — |
| A2 | Custody boundary: `sign_digest`/`agree` only, **private key never returned** by any API (`keys.rs:295,305`) | **prod (API surface)** | ADR-fed-003 §D1 | — | — |
| A3 | **At-rest key protection.** Root/signer/enc seeds stored as **plaintext hex** in a `0600` file (`secret.rs:152` `write_secret_file`; no encryption/passphrase). "ssh-agent-style" custody is **in-process**: the seed is read into `wg`'s memory on every sign (`keys.rs:296-299`). Anything that reads the file or the process gets the root key. | **demo** | ADR-fed-003 §D1 ("download ≠ impersonation" holds for *published bytes*, but local key theft is wide open) | **MAJOR** | M (OS-keyring/HSM/agent split, or at-rest AEAD with a KEK) |
| A4 | Compat handshake `check_compat()` (loud-fail, major+minor pin pre-1.0) | **prod logic, but UNWIRED** | ADR-fed-001 §D7, S-7 | — | see A5 |
| A5 | **S-7 handshake is never invoked on the wire.** `check_compat` has **zero callers** outside its own tests; `WG_FED_COMPAT_VERSION` is only *emitted* as a field in `wg identity show` (`identity_cmd.rs:514`), never *checked* against a peer on fetch/poll/send/serve. The node performs no version negotiation at all. | **missing (wiring)** | ADR-fed-001 §D7 ("fail loud on incompatible mismatch") | **MAJOR** | S (call at fetch/poll + node `/health`) |
| A6 | `canonical_json` number handling: serializes non-string scalars via `serde_json::to_string` — fine for the integer/bool/string fields used today, but **no float/`NaN`/large-int normalization** if a future payload carries floats. | **prod (today) / latent** | doc 04 §1.4 canonical encoding | MINOR | S (forbid floats or pin a canonical number form) |

### B. Sigchain — key-set algebra & recovery (`sigchain.rs`)

| # | Capability | Status | Decision it should meet | Severity | Effort |
|---|------------|--------|--------------------------|----------|--------|
| B1 | genesis + `add_key` + hash-link + self-cert `verify()` rooted at the address | **prod** | ADR-fed-001 §D2/§D4/§D5 | — | — |
| B2 | **Hydra lock (S-4):** `add_key`/`revoke_key`/succession-`rotate_root` require the **active root** (`sigchain.rs:669-683, 732-738`) | **prod** | ADR-fed-003 §D2/§D3, S-4 | — | — |
| B3 | `rotate_root` succession (address stable, active root moves) | **prod** | ADR-fed-003 §D4/§D5 | — | — |
| B4 | `revoke_key` (durable, root-locked) | **prod** | ADR-fed-003 §D6 | — | — |
| B5 | Guardian M-of-N quorum recovery (distinct-count, threshold ≥ 2, in-set check) | **prod** | ADR-fed-003 §D5, A-7 | — | — |
| B6 | node-less mint validation (recovery key **and** M≥2 guardians) defuses Fatal A-4 | **prod** | ADR-fed-003 §D5, Fatal A-4 | — | — |
| B7 | Fork genesis (`ParentRef`) = new `wgid`, verifiable child, not parent | **prod** | ADR-fed-003 §D4 | — | — |
| B8 | **Offline recovery key is a permanent, unrevocable override.** It can `rotate_root` at **any time, forever** (`sigchain.rs:741-759`); the genesis recovery slot is immutable (no `SetRecovery` link type), so a compromised recovery key can never be rotated out. Doc comment claims atproto-style recovery "**within the window**" (`sigchain.rs:434-435`) but **no time window is implemented** — recovery is unbounded in time. | **demo (claim) / missing (window)** | ADR-fed-003 §D5 (atproto-style higher-priority override is windowed) | **MAJOR** | M (add a signed recovery-window + a root-signed `SetRecovery` link) |
| B9 | **No equivocation / forked-history defense.** `verify()` validates *one* linear chain handed to it; a malicious signer can produce **two divergent, each-validly-signed** chains from the same genesis (different `seq=2` links) and show different histories to different peers. There is no transparency log / gossip / fork detection. | **missing** | doc 05 (eclipse/equivocation), ADR-fed-001 §D2 ("never central" ⇒ needs a fork-detection substitute) | **MAJOR** | L (transparency log or head gossip; partly mitigated by freshness `seq` for the *same* verifier, not across peers) |
| B10 | Guardian endorsement digest binds only `(wgid, new_root)` — **not** chain position or an expiry (`sigchain.rs:189-196`). Deliberate (async collection) but an endorsement is replayable to any `rotate_root` installing that root and never expires. Bounded by new-root proof-of-possession. | **prod-ish / latent** | ADR-fed-003 §OQ4 | MINOR | S (add `prev`/expiry to the assertion) |
| B11 | `SetEndpoints` / `SetAliasProof` link types are declared in `LinkType` but `verify()` treats them as **no-ops** (`sigchain.rs:786-787`) — endpoints/aliases are not actually extracted from the chain. | **scaffold** | ADR-fed-001 §D6 (endpoints in record/chain) | MINOR | S |
| B12 | DoS: `verify()` replays an unbounded chain; no length cap | **latent** | resilience | MINOR | S |

### C. Envelopes & sealing — encryption = ACL (`envelope.rs`)

| # | Capability | Status | Decision it should meet | Severity | Effort |
|---|------------|--------|--------------------------|----------|--------|
| C1 | `IdentityRecord`/`StateSnapshot`/`SignedEvent` sign + **offline** verify (forged `from` fails, `envelope.rs:854`) | **prod** | ADR-fed-001 §D2/§D5, ADR-fed-002 | — | — |
| C2 | Per-recipient sealed envelope (CEK once + X25519-wrap per recipient = the ACL; third party locked out) | **prod** | ADR-fed-003 §HQ4 (encryption = ACL), R24 | — | — |
| C3 | **Sealed-sender outer envelope is unauthenticated.** A sealed-sender event carries **no outer signature** (`envelope.rs:409-422`); only the *inner* `SenderSealed` is signed. The outer `to`/`created_at`/`kind`/`id`/`refs` are therefore malleable by any relay/MITM, and `open()` does **not** cross-check inner-vs-outer metadata. AEAD uses **no AAD**, so ciphertext is not bound to envelope metadata. | **demo** | ADR-fed-003 §HQ4 / FR-S4 (sealed-sender hides author *without* sacrificing integrity) | **MAJOR** | M (sign an outer commitment over routing metadata, or fold metadata into the sealed inner + enforce equality on open) |
| C4 | Forward secrecy on the offline path | **missing (acknowledged)** | S-6 (FS doesn't compose with send-to-offline; static keys + rotation) | MINOR | — (deferred by decision) |
| C5 | Replay protection at the envelope layer — `verify()` does **not** dedup or freshness-check; `id` is a dedup key the *consumer* must track. No consumer-side dedup store is wired. | **missing** | FR-M6 (idempotent dedup) | MAJOR | M (per-recipient seen-id store at the consume edge) |
| C6 | `open_multi` returns on the first wrap whose `recipient_kid` we hold; if that wrap's AEAD fails it errors instead of trying other wraps addressed to us | **latent** | resilience | MINOR | S |

### D. UCAN delegation & the leash (`custody.rs`)

| # | Capability | Status | Decision it should meet | Severity | Effort |
|---|------------|--------|--------------------------|----------|--------|
| D1 | Capability issue / verify / **attenuating-only** sub-delegation (widening refused at *both* issue and verify) | **prod** | ADR-fed-003 §D3, S-4 hydra | — | — |
| D2 | Expiry fail-closed (`verify_inner` step 5) — stolen signer worthless after expiry | **prod** | ADR-fed-003 §D3 | — | — |
| D3 | Issuer-subtree revocation; CLI re-verifies each revocation's issuer+signature (`identity_cmd.rs:1999-2010`) before honoring it | **prod** | ADR-fed-003 §D3 | — | — |
| D4 | Leash dial: broad/long birth default, env-tightenable (`WG_FED_LEASH_*`), humans-never-leashed; `from_env()` wired into CLI delegate (`identity_cmd.rs:1860`) | **prod** | ADR-fed-003 §D2 | — | — |
| D5 | **Revocation propagation is withholdable.** `verify()` trusts a `revoked: &[String]` the caller assembles; discovery is via a store list the (untrusted) node can simply **omit** — a revoked-but-unexpired cap is then honored. No freshness gate on revocation discovery. | **demo** | ADR-fed-003 §D3 (revocation composed with expiry — but expiry is the only real backstop) | **MAJOR** | M (publish+freshness-gate a revocation head; or short TTLs as policy) |
| D6 | DoS: `Capability.proof` is `Box<Capability>` with **no depth cap**; `verify_inner`/`chain_len` recurse — a pathologically deep cap can stack-overflow during deserialize/verify | **latent** | resilience | MAJOR | S (cap chain depth on parse + verify) |
| D7 | No clock-skew tolerance on the UCAN temporal check (`now < not_before` is strict), unlike freshness's ±5 min — a just-issued cap can be rejected across skewed clocks | **latent** | resilience | MINOR | S |

### E. Freshness / S-3 (`freshness.rs`)

| # | Capability | Status | Decision it should meet | Severity | Effort |
|---|------------|--------|--------------------------|----------|--------|
| E1 | Signed attestation, tiered Δ (routine 24 h / high-value 15 min), ±5 min skew, monotonic `seq` rollback backstop, fail-closed-on-stale | **prod** | ADR-fed-001 §OQ4, ADR-fed-002 §D5, S-3 | — | — |
| E2 | `seen_seq` persistence is a **plain unsigned integer file** (`freshness.rs:290-305`) — locally tamperable (reset → rollback re-enabled); also a single global counter ⇒ **multi-device** issuers collide / rollback each other | **demo** | S-3 rollback resistance | MINOR | S–M (sign/seal the tracker; per-device seq namespacing) |
| E3 | No trusted time source — `now` is the caller's wall clock; an NTP/clock attacker shifts the freshness window (skew is a fixed ±5 min) | **latent** | S-3 residual | MINOR | M (roughtime/quorum time — broad problem) |
| E4 | Caller must cross-check `att.head` against the chain it verified (`check_fresh` only *returns* head) — relies on every call site doing it | **latent** | S-3 | MINOR | S (fold into a single gated API) |

### F. Loadable-state safety / S-5 (`state_safety.rs`, `identity_cmd.rs` load path)

| # | Capability | Status | Decision it should meet | Severity | Effort |
|---|------------|--------|--------------------------|----------|--------|
| F1 | Trust × same-self × kind-opacity gate, fail-closed; **auto-load only same-self OR (Verified ∧ transparent ∧ scan-clean)**; soft-hit suspicion-monotonic escalation | **prod (logic)** | ADR-fed-004 §D6/§OQ2 | — | — |
| F2 | CAS integrity + signature provenance re-checked in the load path (`identity_cmd.rs:1696-1702`) | **prod** | ADR-fed-004 §D6 steps 1–2 | — | — |
| F3 | `same_self` is cryptographically sound (snapshot signature checked against the loader's own chain; an attacker cannot forge a same-self snapshot) (`identity_cmd.rs:1681,1701`) | **prod** | ADR-fed-004 §D4 | — | — |
| F4 | **Content scanner is a heuristic demo.** `scan_transparent` matches a ~10-phrase static list (`INJECTION_BLOCK`/`INJECTION_ESCALATE`) + key-marker substrings (`state_safety.rs:78-112`). Trivially bypassed by paraphrase, encoding (base64), homoglyphs, or non-English. Honestly self-documented as best-effort. | **demo** | ADR-fed-004 §OQ1 (the *slot*; real Pass-2 reviewer is Review-Wave C) | **MAJOR** | L (real weak-tier reviewer + sandbox) |
| F5 | **`model_binding` enforcement is a stub.** Only *presence* is checked, and only for **opaque** kinds (`identity_cmd.rs:1707-1709`); transparent kinds skip it; **nothing validates the binding against the consuming agent's actual model.** `mod.rs:36` / ADR claim "`model_binding`-enforced" overstates this. | **scaffold** | ADR-fed-004 §OQ1 (model_binding enforced) | **MAJOR** | M (compare binding to runtime model; fail-closed on mismatch) |
| F6 | **No actual state consumption.** `AutoLoad` only prints "LOADED" (`identity_cmd.rs:1786`); no loadable-state consumer decodes the payload into a running agent. The gate decides; the *load* is unimplemented. | **missing** | ADR-fed-004 (the V6 resume) | **MAJOR** | M–L (wire a real conv-cache consumer) |

### G. Transport & node (`transport.rs`, `node.rs`)

| # | Capability | Status | Decision it should meet | Severity | Effort |
|---|------------|--------|--------------------------|----------|--------|
| G1 | `FedStore` trait + `FileStore` + `HttpStore`, scheme routing (`http(s)://` vs dir/`file://`) | **prod (structure)** | ADR-fed-002 §D1 | — | — |
| G2 | **`get_object` does not re-verify `cid == hash(bytes)`.** The trait comment claims "the CID is the integrity check — the store cannot tamper" (`transport.rs:67-68`) but neither store re-hashes on fetch; integrity is silently the caller's duty. Some callers verify (state-load `identity_cmd.rs:1697`); the generic object path does not. | **demo (misleading invariant)** | ADR-fed-002 §D3 (untrusted transport ⇒ content-address *must* be checked) | **MAJOR** | S (verify CID inside `get_object`) |
| G3 | No response size limits — `resp.bytes()`/`read_exact` read unbounded bodies → memory DoS from a hostile node/client | **missing** | resilience | MAJOR | S (cap body size) |
| G4 | No retry / partition / offline-send queue — a single failed request bails (offline *recipients* are covered by store-and-forward; offline *sender's node* is not) | **missing** | NFR-2 (email-speed, both-ends-offline) | MINOR | M |
| G5 | **Node: zero authentication/authorization on every endpoint.** Anyone can `PUT /heads/<wgid>`, `/inbox/<wgid>/<id>`, `/attestations/<wgid>`, `/objects/<cid>` for **any** identity → inbox flooding, head-squatting/rollback, attestation overwrite, storage exhaustion (`node.rs:132-200`). Forged *identity* is still impossible (self-verify), but DoS/grief is trivial. | **demo** | ADR-fed-002 §D2/§D3 (untrusted node, but must survive abuse) | **BLOCKER** | M (per-wgid write-auth via signed PUT, quotas, rate limits) |
| G6 | **Node: unbounded `Content-Length` allocation** — `body = vec![0u8; content_length]` (`node.rs:105`) pre-allocates the client-claimed length before reading → trivial memory-exhaustion DoS (`Content-Length: 4000000000`) | **demo** | resilience | **BLOCKER** | S (cap + stream) |
| G7 | **Node: `PUT /objects/<cid>` does not validate bytes hash to `<cid>`** (`node.rs:135-138`) → object store is freely poisonable/overwritable | **demo** | ADR-fed-002 §D3 | MAJOR | S |
| G8 | **Node: thread-per-connection, unbounded, no socket read/write timeout** (`node.rs:58`, no `set_read_timeout`) → connection-flood + slow-loris DoS | **demo** | resilience | MAJOR | M (thread pool + timeouts) |
| G9 | **Node: no inbox GC / retention / delete-after-poll / cursor.** Consumed events are never removed; `list_events` returns *all* every poll → unbounded growth + O(n) re-fetch. The `FedStore` trait has no delete op. | **demo** | NFR (scale) | MAJOR | M (ack/delete + cursor + TTL) |
| G10 | Node: no TLS (plaintext `http://`) — sealed bodies stay encrypted, but routing metadata + unsealed bodies + head pointers are cleartext on the wire | **demo (by design)** | ADR-fed-002 (untrusted transport) | MINOR | S (TLS or document proxy requirement) |
| G11 | Node test depth: **one** happy-path roundtrip test (`node.rs:283`); no adversarial/DoS/auth tests | **gap** | test depth | MAJOR | M |

### H. Federation resolution & trust (`federation.rs`, `trust.rs`)

| # | Capability | Status | Decision it should meet | Severity | Effort |
|---|------------|--------|--------------------------|----------|--------|
| H1 | Key-based `PeerConfig`/`Remote` (`wgid` + `endpoints` + `trust`); resolution cascade cached-record → directory hint → DHT | **prod (first two rungs)** | ADR-fed-001 §D5, ADR-fed-002 §D1 | — | — |
| H2 | **DHT / Iroh discovery deferred** (`federation.rs:411,535`). The "cascade" in practice is **manual config + pre-cached records** — there is no decentralized peer discovery. A peer you haven't been told about cannot be found. | **missing (deferred)** | ADR-fed-002 §D1 rung ≥ 2 | MAJOR (for "decentralized" claim) | L (bind Iroh/DHT) |
| H3 | `resolve_author_trust` unifies peer registry + exec provider pool, **fail-closed to `Unknown`** (`trust.rs:100-106`) | **prod (structure)** | one trust dial (ADR-CS1 D5) | — | — |
| H4 | Merge is **`most_trusting` (max)** across two heterogeneous sources — a wgid marked `Verified` as a *compute provider* is then `Verified` as a *message author*. Conflated semantics widen trust. Also "Verified" is a **purely operator-assigned local label** (`wg peer add --trust`), backed by no cryptographic verification ceremony. | **prod / debatable default** | ADR-CS1 D5, ADR-fed-001 §D5 | MINOR–MAJOR | S–M (min-merge across planes, or per-plane dials) |

---

## ADR security-property checklist — delivered *in code*?

| Property | ADR | In code? | Evidence / gap |
|----------|-----|----------|----------------|
| Root never leaves custody (no API returns it) | fed-003 §D1 | **YES** | `keys.rs:295,305` return only sig/shared-secret |
| …but root is safe **at rest** | fed-003 §D1 | **NO** | plaintext-hex `0600` file, in-process load (A3) |
| Self-verify, never central | fed-001 §D5 | **YES** | `sigchain::verify` + envelope verify are pure-local closures |
| Address stable under rotation | fed-001 §D4 | **YES** | B3, tested `rotate_root_succession_keeps_address…` |
| Hydra lock — delegate can't grow key set (S-4) | fed-003 §D2/D3 | **YES** | B2, tested `add_key_not_signed_by_root_is_rejected` |
| Rotation / revoke / recover | fed-003 §D5/D6 | **YES (logic)** | B3–B6; **but** recovery key unrevocable + unwindowed (B8) |
| M-of-N guardian ceremony (Fatal A-4) | fed-003 §D5 | **YES** | B5/B6, tested incl. below-threshold & outsider rejection |
| Encryption = ACL (R24) | fed-003 §HQ4 | **YES** | C2, tested `multi_recipient_only_acl_set_opens` |
| Sealed-sender hides author *with* integrity (FR-S4) | fed-003 §HQ4 | **PARTIAL** | author hidden + inner-authenticated, **outer metadata unauthenticated** (C3) |
| Attenuating-only delegation (hydra) | fed-003 §D3 | **YES** | D1, tested at issue *and* verify |
| Stolen signer worthless after expiry | fed-003 §D3 | **YES** | D2, tested `expired_capability_is_worthless` |
| Freshness / S-3 freeze defense | fed-001 §OQ4 | **YES** | E1, fully tested; residuals E2–E4 |
| S-5 loaded state is untrusted input | fed-004 §D6 | **YES (gate)** | F1–F3; **but** scanner heuristic (F4), `model_binding` stub (F5), no real load (F6) |
| Loud-fail compat handshake (S-7) | fed-001 §D7 | **NO (unwired)** | `check_compat` has zero non-test callers (A5) |
| Untrusted transport — integrity from content-address | fed-002 §D3 | **PARTIAL** | content-addressing exists, but `get_object`/node don't enforce it (G2/G7) |
| Equivocation / fork detection | doc 05 | **NO** | B9 — no transparency log |

---

## Known deferrals — built vs scaffold (no rubber-stamping)

- **Real transport library (Iroh / relay / DHT)** — **SCAFFOLD/DEFERRED.** Only two rungs exist: a dumb directory
  (`FileStore`) and a bespoke HTTP node (`HttpStore`+`node.rs`). DHT/Iroh discovery is explicitly deferred
  (`federation.rs:411,535`). The node is dependency-light and **not** abuse-hardened (G5–G11). *Verdict: the
  store-and-forward **shape** is built and wire-tested; the **production transport** is not.*
- **MLS / forward-secret groups** — **DEFERRED BY DECISION (S-6).** Offline path uses static recipient keys; the code
  is honest about it (C4). Not a stub pretending to be FS — it's an explicit non-goal. *Verdict: correctly scoped out.*
- **Rotation / revoke / recover + M-of-N guardian ceremony** — **REAL, not scaffold.** All three rotation paths, both
  recovery paths, node-less validation, and fork are implemented and adversarially tested (B3–B7). The *gaps* are
  refinements (unrevocable/unwindowed recovery key B8; immutable recovery slot), not missing primitives. *Verdict:
  built.*
- **S-5 state gate** — **gate REAL, payload STUB.** The decision matrix is production logic (F1–F3); the content
  scanner (F4), `model_binding` enforcement (F5), and the actual state load (F6) are demo/stub/missing. *Verdict:
  half-built — the hard *policy* is done, the *mechanism* it gates on is not.*
- **Compat handshake (S-7)** — **DEFINED, NOT WIRED.** The function and its tests exist; no wire path calls it (A5).
  *Verdict: scaffold at the seam.*

---

## What's genuinely production-ready today vs what is scaffold

**Production-ready (the cryptographic algebra):**
- `wgid:` addressing + offline signature verification (`keys.rs`, envelope verify).
- The sigchain key-set algebra: genesis/add_key/revoke/rotate, hash-linking, the **hydra lock**, fork semantics, and
  both recovery paths' *verification* (`sigchain.rs`) — 15 tests including the adversarial cases.
- UCAN delegation: attenuating-only, expiry-fail-closed, issuer-subtree revocation, the leash dial (`custody.rs`).
- The freshness algebra (tiered Δ, skew, monotonic seq, fail-closed) (`freshness.rs`).
- The S-5 trust×opacity **decision logic** (`state_safety.rs`).
- Encryption-as-ACL multi-recipient sealing (`envelope.rs`).

These are well-factored, pure, and adversarially unit-tested. I would trust the *verification* code to reject forgery,
tampering, expiry, attenuation-widening, and below-threshold recovery.

**Scaffold / demo (everything that makes it a running networked system):**
- **The node** (`node.rs`) — no auth, unbounded allocation, no CID validation on PUT, no timeouts, no GC, one test.
  This is the single biggest gap and the one with **BLOCKER** items.
- **At-rest custody** (`keys.rs`/`secret.rs`) — plaintext root keys, in-process signing.
- **The compat handshake** — unwired (A5).
- **Peer discovery** — manual/cached only; DHT deferred (H2).
- **The S-5 mechanism** — heuristic scanner (F4), stubbed `model_binding` (F5), no real load (F6).
- **Sealed-sender integrity** (C3), **envelope-layer replay/dedup** (C5), **revocation propagation** (D5),
  **equivocation defense** (B9), and several **DoS-hardening** items (D6, G3, G6, G8).

---

## Ranked v1 punch-list (input to `audit-synth`)

**Blockers (before any networked deployment of `wg fed-node`):**
1. G6 — bound `Content-Length` / stream request bodies (node OOM). *(S)*
2. G5 — authenticate node writes (signed PUT per wgid) + quotas/rate limits. *(M)*

**Majors (before relying on the property in prod):**
3. A3 — protect root keys at rest (keyring/HSM/at-rest AEAD) + ideally split signing out-of-process.
4. A5 — wire `check_compat` into fetch/poll/serve (the S-7 handshake).
5. G2/G7 — enforce `cid == hash(bytes)` in `get_object` and on node PUT.
6. G8/G9 — node socket timeouts + thread bound; inbox ack/GC/cursor + retention.
7. F5 — enforce `model_binding` against the consuming runtime; F6 — wire a real state consumer.
8. C3 — authenticate the sealed-sender outer envelope (metadata integrity).
9. C5 — envelope-layer dedup/replay store at the consume edge.
10. D5 — freshness-gate revocation propagation (or mandate short cap TTLs by policy).
11. B8 — windowed + revocable recovery key (and a `SetRecovery` link).
12. B9 — equivocation/fork detection (transparency log or head gossip).
13. D6/G3 — depth-cap UCAN chains; size-cap transport responses.
14. F4 — replace the heuristic scanner with the real Pass-2 reviewer (Review-Wave C dependency).
15. G3/H2 — bind a real discovery/transport rung (Iroh/DHT) for the "decentralized" claim.

**Minors (hardening / docs):** A6, B10, B11, B12, C4(doc), C6, D7, E2, E3, E4, G4, G10, H4.

---

## Methodology & caveats

Read every `src/identity/*.rs` module in full (incl. tests), the `wg identity` CLI load/fetch/delegate/revoke/load-state
paths, the WG-Fed deltas in `federation.rs`/`trust.rs`/`messages.rs`, and the four ADRs. Findings are cited to
`file:line`. Severity reflects production risk, not spark-acceptance — by the spark's own boundary statements (memo §4.3,
the per-wave "spark boundary" notes in `CLAUDE.md`) most G/F-row gaps are **deliberately** out of the spark and listed
here as the production delta, not as spark defects. The crypto primitives themselves (ed25519-dalek `verify_strict`,
X25519, XChaCha20-Poly1305, HKDF-SHA256, BLAKE3) were taken as correct; this audit assesses their *composition and
wiring*, not the primitives.
