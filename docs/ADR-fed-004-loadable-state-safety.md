# ADR-004 (WG-Fed): Loadable-State Format & AI-Input Safety — *Loaded State Is Untrusted Input*

**Status:** Proposed
**Date:** 2026-06-25
**Decision:** One stable `StateSnapshot` envelope with a **tagged, evolvable `payload_kind` slot** (`conv-cache-v1` / `summary-v1` / `opaque-blob-v1` / future); **signed + BLAKE3-content-addressed + incremental** (`prev`); a `model_binding` guard against wrong-model loads; unknown `payload_kind`s **degrade gracefully**. We design the **slot, not the opaque payload**. Two security carve-outs are load-bearing: **FR-S1 becomes a runtime-containment guarantee for opaque kinds** (the custody boundary holds the only key copy — S-1), and **loaded state is treated as UNTRUSTED INPUT** — a signature proves *who wrote* state, never that it is *safe to load* — so the load path provenance-gates by `trust_level`, enforces `model_binding`, and puts a human in the loop for cross-trust loads (S-5).

> **This is the loadable-state-safety WG-Fed ADR.** It cites the identity,
> addressing, key-hierarchy, sigchain, and freshness-attestation primitives fixed
> in **ADR-001** (`docs/ADR-fed-001-identity-key-model.md`), the custody boundary
> owned by **ADR-003**, and the transport/seal primitives of **ADR-002**. The
> decision was *made* in the federation-study decision memo
> (`docs/federation-study/06-decision-memo-and-roadmap.md` §3 HQ10, §6 ADR-004
> stub, §8 item 5); this ADR formalizes it and resolves the stub's three open
> questions. It is **not** a re-litigation of the architecture choice (`WG-Fed` =
> Candidate C with a B-shaped default, D's UCAN grafted, A preserved as the
> node-less option) — that is settled.

---

## Context

The WG social-network vision wants an identity to be a **long-lived loadable
self** (V1, R2): an agent resumes a *continuous* identity by loading prior
state rather than starting fresh each task, and that state evolves over time
from today's **readable conversation cache** → intermediate **summaries** →
eventually an **opaque multi-gigabyte hidden/RNN tensor blob** meaningful only to
one model version (doc 03 HQ10, FR-I3). The same artifact must be
**portable/downloadable** (V2): published to the web, fetchable from anywhere,
and verifiable independent of where it was fetched (FR-I4). WG today has **no**
loadable-identity format and **no** AI-input-safety layer at all — both are net-new
(doc 02 §2.4).

Two requirements pull against each other (tension T8, doc 03): a format that
**hard-codes today's conversation-log shape** needs a rewrite the moment opaque
state arrives, while one **abstract enough to be future-proof** is useless for the
concrete conversation cache we need now. The memo resolves T8 with a
**stable-interface / evolvable-payload split**: one `StateSnapshot` envelope whose
**interface is fixed** and whose **`payload_kind` is a tagged, evolvable slot**
(doc 04 §1.4b). We commit to designing the *slot*; serializing/reloading a model's
opaque hidden state is an explicit non-goal (memo §7.7).

The adversarial pass (doc 05) makes two findings on this feature **unavoidable**,
and the memo §8 hands both to this ADR to *budget engineering for*, not discover in
code:

- **S-1 — opaque-state key/secret exfiltration.** FR-S1's acceptance signal is
  *"static analysis / format-spec guarantees no field can carry a private key."*
  That is achievable for the **typed** `IdentityRecord`/`SignedEvent` (every field
  is declared) but **impossible for an opaque `payload_kind`** — an un-introspectable
  blob can smuggle the root signer, a session token, or another identity's key *out
  through the custody boundary under a valid signature* (doc 05 S-1; the
  download-attack step D-1→S-1, §3.1). The portable-state feature becomes a
  key-exfiltration channel for a malicious or buggy agent.

- **S-5 — loadable-state poisoning / stored prompt-injection.** V1's premise is
  loading a `StateSnapshot` to *resume a continuous self*. **A signature proves *who
  authored* the state, never that the state is *safe to load*.** A malicious or
  compromised agent publishes a *validly-signed* "conversation cache" carrying a
  prompt-injection or poisoned summary; the next host that loads it to "resume Nora"
  inherits the poison — a **persistence + lateral-movement vector that rides the
  legitimate portability feature** (doc 05 S-5). This threat is **AI-substrate
  specific and has no analogue in Nostr / Keybase / atproto** — there is no
  prior-art mitigation to copy, which is exactly why the memo §1 names it one of the
  three substrate findings WG must engineer for regardless of any later
  re-litigation, and §8 budgets it.

This ADR fixes the loadable-state format **and** the AI-input-safety layer that
guards loading it. It is a Wave-2 deliverable; **no federation code lands until
ADR-001/002/003 are Accepted** (memo §5), and the state-poisoning safety layer is
scheduled for Wave 5 (memo §5, Phase 2) — but it must be *designed in here*.

---

## Decision

### D1 — One stable `StateSnapshot` envelope with a tagged, evolvable `payload_kind` slot

State is carried by a single, stable wire envelope (doc 04 §1.4b). The **interface
is fixed; the `payload_kind` is the evolvable slot** (FR-I3, HQ10):

```jsonc
{ "v": 1, "alg": "ed25519",
  "identity": "wgid:<multibase-ed25519-pubkey>",   // whose state this is (ADR-001 D1)
  "payload_kind": "conv-cache-v1 | summary-v1 | opaque-blob-v1 | <future>",  // TAGGED & evolvable
  "model_binding": { "model": "claude-opus-4-8", "min_reader": "conv-cache-v1" },  // wrong-model guard (D3)
  "content_cid": "<blake3 of the (possibly-encrypted) payload>",   // CAS (D2)
  "prev": "<cid of prior snapshot | null>",   // incremental publish (D2)
  "enc": { "scheme": "per-recipient", "recipients": [ {"kid": "...", "wrapped_key": "..."} ] },  // optional/forced — D5/OQ3
  "sig": "<authorized-signer signature over the canonical envelope>" }   // who wrote it (D2)
```

A `conv-cache-v1` conversation log today and a hypothetical `opaque-blob-v1` tensor
state tomorrow load through the **same loader and the same verifier**; only the
per-kind decode at the end differs. This is the only structure that serves both
without a rewrite (resolves T8), and it is deliberately a **slot, not a payload
spec**: WG-Fed standardizes the envelope, the integrity/provenance/safety pipeline,
and the *registry of kind tags* — it does **not** standardize the internal byte
layout of an opaque payload (memo §7.7 non-goal). The `payload_kind` is the single
extension point; new kinds are added by registering a tag, never by changing the
envelope.

### D2 — Signed + BLAKE3-content-addressed + incremental

- **Content-addressed (FR-I4).** `content_cid` is the BLAKE3 CID of the
  (possibly-encrypted) payload bytes; the envelope itself is addressed by BLAKE3 of
  its canonical (sorted-key) serialization (doc 04 §1.4). Fetching by CID makes
  tampering self-evident: a flipped byte changes the hash, so a hostile or buggy
  third location `L` (a dumb object store, an IPFS gateway — anything that returns
  bytes; spark-test §4.1) **cannot corrupt state undetected**.
- **Signed (FR-I4).** `sig` is over the canonical envelope by a signer key the
  author's sigchain authorized *at the snapshot's position* (ADR-001 D2). Integrity
  **and** provenance are therefore verifiable **independent of where the bytes were
  fetched** — the property V2 (portable/downloadable) requires.
- **Incremental (`prev`).** `prev` chains a snapshot to its predecessor's CID so a
  publisher **appends a turn without re-uploading history** (HQ10 success criterion):
  publish a small delta whose `prev` points at the last full or delta snapshot. A
  loader walks the `prev` chain to reconstruct the state at any point. The `prev`
  chain is itself verified (each link signed + CAS) and is checked for consistency
  during load (no silent history-rewrite — see the load pipeline, D6).
- **Identity ≠ any single snapshot (FR-I7).** The stable `wgid:` address (ADR-001
  D4) is independent of its many `StateSnapshot`s over time; an identity survives
  unbounded state updates without its address changing. A snapshot names its
  `identity`; it never *is* the identity.

### D3 — `model_binding` guards wrong-model loads

Every snapshot carries `model_binding` — the model/version that authored it and the
minimum reader capability required to interpret the payload. A load whose model does
not satisfy `model_binding` is **detected and refused (or explicitly degraded), never
silently loaded** (HQ10 success criterion: "a wrong-model load is detected, not
silently corrupt"). This matters most for opaque kinds — an `opaque-blob-v1` tensor
produced by one model version is meaningless or actively corrupting to another — but
it applies uniformly: a `conv-cache-v1` whose `min_reader` exceeds the loader's
capability degrades gracefully (D4) rather than half-loading. `model_binding` is one
of the explicit AI-input-safety gates in the load pipeline (D6), not advisory
metadata.

### D4 — Unknown `payload_kind` degrades gracefully

A client that encounters a `payload_kind` it does not understand **degrades
gracefully** (NFR-4, the forward/backward-compat requirement) — it **verifies the
signature and provenance, surfaces "state present, payload unreadable by this
client," and stops.** It **never** silently corrupts, never half-decodes, and never
treats an unreadable payload as empty. This is the same loud-but-safe posture as the
`WG_FED_COMPAT_VERSION` handshake (ADR-001 D7): unknown ≠ invalid. An old client and
a new publisher still exchange everything the old client *can* read (the envelope,
the provenance, the `prev` lineage); only the unreadable payload is skipped, with the
gap made visible. Graceful degradation is what lets the `payload_kind` slot evolve
(D1) without orphaning deployed clients.

### D5 — FR-S1 is a runtime-containment guarantee for opaque kinds (S-1)

FR-S1 — *"no field can carry a private key"* — is enforced **statically** for the
typed kinds (every field of a `conv-cache-v1`/`summary-v1` is declared and scannable,
so a key-shaped field is a format violation; see the scan, OQ1). For an **opaque**
`payload_kind` it **cannot** be a static guarantee — the blob is un-introspectable by
design (S-1). FR-S1 is therefore **downgraded, explicitly and disclosed, from a static
guarantee to a runtime-containment guarantee for opaque kinds**, resting on three
properties this ADR (with ADR-003) commits to:

1. **The custody boundary holds the *only* copy of the root/signer** (ADR-003,
   S-2). The worker that produces a snapshot **never has the root bytes in its
   address space**, so there is *nothing to bake into a blob* — the exfiltration
   channel has no source for the highest-value secret.
2. **Opaque payloads are always sealed** (OQ3) — encrypted-at-rest to recipients
   (HQ4) — so even if a malicious author *did* smuggle a lower-value secret into a
   blob and publish it to a dumb `L`, a thief who fetches the ciphertext learns
   nothing. The leaked blob is opaque to a non-recipient; the recipient set *is* the
   ACL (FR-S3).
3. **Opaque payloads are loaded into a sandbox and treated as untrusted** (D6) —
   they get the *strongest structural* protections precisely because they cannot be
   inspected.

The residual (an opaque blob shared *with a legitimate recipient* could still carry a
secret to *that* recipient) is **inherent and disclosed** — it is the S-1 cost: the
headline "just publish the blob and anyone can load it" simplicity is **lost for
opaque kinds**, which become per-recipient sealed artifacts, never public ones.

### D6 — Loaded state is UNTRUSTED INPUT (S-5): the load pipeline is the load-bearing decision

**This is the heart of the ADR.** Verifying a `StateSnapshot`'s signature proves
**who authored it and that it is unmodified — it does *not* prove it is safe to
load** (doc 05 S-5). A validly-signed snapshot can carry a prompt-injection, a
poisoned summary, or a tampered tool-history that hijacks the resuming agent. WG-Fed
therefore treats **every loaded `StateSnapshot` as untrusted input** and gates every
load through a fixed, **fail-closed** pipeline. Integrity/provenance (steps 1–3)
establish *who*; the AI-input-safety gates (steps 4–7) decide *whether to load at
all*. Passing 1–3 is **necessary but never sufficient** — that is the whole point of
S-5.

The load pipeline, in order, fail-closed at each step:

1. **CAS integrity.** Recompute the BLAKE3 CID of the fetched payload and compare to
   `content_cid` (and the envelope's own CID). Mismatch → **reject** (tampered or
   wrong bytes). (FR-I4)
2. **Signature + provenance.** Verify `sig` against a signer key the author's
   sigchain authorized at the snapshot's position; resolve `identity → current
   authorized key set` via the ADR-001 sigchain. Establishes the *author*. A forged
   "from Nora" snapshot fails here. (FR-I4, ADR-001 D2)
3. **Freshness (cross-trust / high-value loads).** A cross-trust load is a
   **high-value action** in ADR-001 OQ4's tiering, so re-fetch the author's signed
   freshness attestation and **fail closed on stale** (Δ ≤ 15 min, monotonic `seq`) —
   so a frozen view cannot feed state signed by a since-revoked key. Same-self resume
   uses the routine tier. (ADR-001 OQ4, S-3)
4. **`model_binding` enforcement.** Refuse or explicitly degrade on a model/reader
   mismatch (D3); never silently load wrong-model state.
5. **`payload_kind` dispatch.** *Known transparent* kind → run the AI-input-safety
   scan (step 6). *Known opaque* kind → unwrap the seal into a sandbox under the
   containment posture (D5); the scan degrades to containment, not inspection.
   *Unknown* kind → **degrade gracefully and stop** (D4) — never load.
6. **AI-input-safety scan.** Per-kind content/structure scan (resolved in **OQ1**);
   any flag **escalates** the trust gate one level (step 7).
7. **Provenance-gate by `trust_level` + human-in-the-loop.** Decide **auto-load vs
   human-in-loop vs refuse** from the author's `trust_level`, whether the load is
   same-self or cross-self, and the kind's opacity (the matrix resolved in **OQ2**).

Only after step 7 grants it does the payload decode into the agent's working state,
and opaque kinds decode **inside the sandbox** (D5). This pipeline is the
"AI-input-safety layer WG does not have today" the memo §3/§8 budgets; it lives in
`src/identity/` (new `state_safety.rs` alongside `envelope.rs`/`acl.rs`).

---

## Status

**Proposed.** This ADR records the decision exactly as fixed in the federation-study
decision memo (§3 HQ10, §6 ADR-004 stub) and resolves the three open questions the
stub left open. **Erik ratifies it to Accepted** — that human gate is deliberately
not set here. No federation code lands until ADR-001/002/003 are Accepted, and the
S-5 safety layer is implemented in Wave 5 (memo §5, Phase 2).

---

## Consequences

- **A new AI-input-safety layer WG lacks entirely today** (`src/identity/state_safety.rs`):
  the D6 load pipeline, the OQ1 scan, and the OQ2 trust gate. This is genuinely
  novel — there is no Nostr/Keybase/atproto precedent to copy (S-5), so it is a
  living, maintained policy surface (like an antivirus signature set), not a
  write-once check.
- **`StateSnapshot` lands in `envelope.rs`** (sign/verify/canonical-encode) with the
  `payload_kind` registry; `acl.rs` gains the forced-seal path for opaque kinds (D5,
  OQ3). The `prev` chain walk + consistency check land alongside.
- **The seamless-resume UX is eroded** (the disclosed S-5 cost): a cross-trust load is
  no longer "fetch and go" — it may pause for a human decision. Same-self resume
  stays fast (OQ2), so the common case (an agent resuming its *own* continuous self)
  is unaffected beyond the scan.
- **The "just publish the blob, anyone loads it" simplicity is lost for opaque kinds**
  (the disclosed S-1 cost): opaque state is always sealed and per-recipient, never a
  public CAS artifact (D5, OQ3). Transparent kinds keep optional sealing.
- **FR-S1 weakens to runtime containment for opaque kinds** (D5) — disclosed, bounded
  by custody (ADR-003) + forced sealing + sandboxing.
- **Enables FR-I3** (versioned, model-agnostic loadable state), **FR-I4**
  (content-addressed + signed), and **FR-I7** (stable identity ≠ mutable snapshots);
  satisfies **NFR-4** (graceful unknown-payload degradation) for the state format.
- **A residual is accepted and inherent:** *signature ≠ safety* (S-5). No scan,
  trust-gate, or human glance makes loading a stranger's state provably safe; the
  pipeline raises the cost and catches the known/cheap attacks while the trust gate
  and human-in-loop carry the rest. We **disclose** the residual rather than pretend
  the signature closes it.

---

## Alternatives rejected

- **Hard-coding today's conversation-log shape as *the* state format.** Serves the
  conversation cache now but needs a full rewrite the moment summaries or opaque
  state arrive (tension T8). Rejected for the stable-envelope/evolvable-`payload_kind`
  split (D1), which serves both through one interface.
- **A format so abstract it is useless today** (e.g. "an opaque bag of bytes" with no
  typed kinds). Future-proof but unusable for the concrete conversation cache Wave 3+
  needs. Rejected: D1 ships `conv-cache-v1` concretely *and* keeps the slot open.
- **Solving opaque hidden-state *serialization* now.** Designing the internal byte
  layout of a model's hidden/RNN state is an explicit non-goal (memo §7.7) — it is a
  research problem orthogonal to federation. Rejected: we design the **slot**
  (`opaque-blob-v1` tag + envelope + safety pipeline), not the payload.
- **Treating a valid signature as a safety guarantee** (auto-load anything that
  verifies). This is exactly the S-5 trap: a signature proves authorship, not safety,
  and a malicious author signs poison just as validly as honest state. Rejected — D6
  treats loaded state as untrusted *after* the signature checks, and the load
  pipeline's whole reason to exist is that signature-valid ≠ safe-to-load.
- **A static FR-S1 "no key in any field" guarantee for *all* kinds.** Unachievable for
  an un-introspectable opaque blob (S-1) — claiming it for opaque kinds would be a
  guarantee we cannot keep. Rejected for the honest runtime-containment downgrade (D5).
- **Silent degradation on an unknown `payload_kind`** (treat-as-empty, or best-effort
  half-decode). Violates NFR-4 and risks silent corruption. Rejected: unknown kinds
  degrade *loudly and safely* (D4) — verify, surface "unreadable," stop.
- **Auto-loading cross-trust state to preserve the seamless-resume UX.** The fastest
  UX, and the precise S-5 attack surface. Rejected: cross-trust loads are
  provenance-gated and human-in-loop (OQ2); we pay UX friction exactly where the
  threat lives and nowhere else.

---

## Open questions

The ADR-004 stub (memo §6) and the handed-off checklist (memo §8 item 5) left three
questions for this ADR to close. All three are resolved with rationale below; where a
residue is genuinely a tuning/UX value judgment it is **explicitly flagged for Erik**
rather than silently fixed (the ADR-001 convention).

### OQ1 — What the AI-input-safety scan actually checks — **RESOLVED (categories + posture; heuristic ruleset is a living policy, flagged)**

**Resolution — the scan is a per-kind, defense-in-depth filter, not a safety proof.**
You can only scan what you can introspect, so the scan is **defined per
`payload_kind`** and is **fail-closed**: a hard hit blocks the load; a soft hit
**escalates** the trust gate (OQ2) one level.

**For transparent kinds (`conv-cache-v1`, `summary-v1`) — four check categories:**

1. **Structural / type-confusion validation.** The payload parses as exactly the
   schema its `payload_kind` declares; no oversized, unexpected, or extra fields; the
   declared kind matches the actual structure. A kind tag that disagrees with the
   bytes is hostile (a `conv-cache-v1` shaped like something else) → **block**.
2. **Embedded-secret / key scan (ties S-1).** Entropy + pattern scan for private-key
   material, session tokens, API keys, or credential-shaped blobs in any field. A
   transparent kind that carries key-shaped bytes is malformed-or-hostile by FR-S1
   (D5) → **block**. (This is what makes FR-S1 *static* for transparent kinds.)
3. **Prompt-injection heuristics (the S-5 core).** Scan textual content
   (conversation turns, summaries) for known injection shapes: system-prompt-override
   directives ("ignore previous instructions," "you are now…"), role-confusion,
   tool/command-invocation strings, data-exfiltration patterns, and unexpected
   instruction-like content in *data* positions. **High-confidence hits block;
   lower-confidence hits escalate** to human-in-loop. This is **heuristic and
   best-effort by construction** — it is the AI-specific check with no prior art, and
   it cannot be complete (signature ≠ safety; the inherent residual).
4. **Provenance / lineage consistency.** `identity` matches the signing key's
   authorization; `model_binding` is present and well-formed; if the snapshot claims
   to be incremental, its `prev` chain resolves and is internally consistent (no
   history-rewrite, no `prev` pointing outside the author's lineage) → mismatch
   **blocks**.

**For opaque kinds (`opaque-blob-v1`) — the scan degrades to *containment*, not
inspection.** An un-introspectable blob cannot be content-scanned (S-1), so the
"scan" is: validate envelope/size/shape bounds, require `model_binding`, confirm the
seal (OQ3), and **route to the containment posture** — sandbox-only load, strict
`model_binding`, and a *mandatory* trust gate (no opaque kind auto-loads across a
trust boundary, OQ2). "We cannot read it, so we contain it" — which is *why* opaque
kinds trigger human-in-loop more readily.

*Why.* The scan's job is to **raise the attacker's cost and catch the known/cheap
attacks**, while provenance-gating (OQ2) and human-in-loop carry what heuristics
cannot. Committing to the *categories* and the *fail-closed, escalate-on-soft-hit
posture* is the durable design; pretending a scan could *prove* safety would
contradict S-5's inherent residual.

*Flagged for Erik (living policy, not format):* the **exact heuristic ruleset** for
category 3 (the specific injection signatures, the confidence thresholds for
block-vs-escalate) is a **maintained policy surface** — it evolves like an antivirus
signature set as new injection patterns appear, and it is the kind of
false-positive/false-negative trade-off Erik (or a security owner) should set and
revisit. This ADR commits to the **four categories, the per-kind split, and the
fail-closed/escalate posture**; the signature contents and confidence cut-offs are
tunable without reopening the design.

### OQ2 — The `trust_level` threshold for auto-load vs human-in-loop — **RESOLVED (matrix grounded in WG's `TrustLevel`)**

**Resolution — a small matrix over WG's existing `TrustLevel`
(`Verified` / `Provisional` / `Unknown`, `src/agency/types.rs`), the
same-self/cross-self axis, and kind opacity.** The gate is on the **author's**
trust relative to the loader, because S-5 is about *whose* state you inherit.

- **Same-self load** (author `wgid` == loader `wgid`; signed by an authorized key of
  *this* identity — the V1 "resume my own continuous self" happy path):
  **auto-load after the scan passes**, regardless of trust_level. The scan still runs
  (a previously-compromised self could have poisoned its own cache), but a human gate
  on *every self-resume* would destroy the UX the feature exists to provide — so
  same-self is **scan-gated, not human-gated**.
- **Cross-self load** (author ≠ loader — "download Nora onto host B," loading
  *someone else's* state):
  - Author **`Verified`** → **auto-load permitted iff** the kind is **transparent**
    **and** the scan is **clean**. A `Verified` author's **opaque** kind still
    requires **human-in-loop** (it cannot be scanned, D5/OQ1).
  - Author **`Provisional`** → **human-in-loop always** (the TOFU default for
    federated peers, HQ8).
  - Author **`Unknown`** → **refuse by default** — do not offer auto-load; a human may
    explicitly override with an OOB-verified decision.
- **Escalate on any scan flag:** a soft hit moves the verdict one level *stricter*
  (auto-load → human-in-loop, or human-in-loop → refuse). The gate is monotonic in
  suspicion.

Stated crisply: **auto-load is permitted only for `(same-self)` OR
`(cross-self ∧ author = Verified ∧ transparent-kind ∧ scan-clean)`. Everything else
is human-in-loop; `Unknown`-authored cross-self is refused absent an explicit human
override.** A cross-trust load is also a **high-value action**, so it is
freshness-gated and **fails closed on stale** (ADR-001 OQ4, pipeline step 3).

*Why.* This maps directly onto **FR-T3** ("gate dispatch by `trust_level`") and WG's
already-shipped three-level enum — no new trust vocabulary. It keeps the
**seamless-resume happy path fast** (your own state) while making the genuinely
dangerous case (loading a stranger's, or any opaque, state) **maximally gated** — the
friction lands exactly where the S-5 threat lives. `Provisional`-default-to-human
matches the TOFU posture (HQ8): a newly-met peer is not yet trusted to feed you state
unsupervised.

*Flagged for Erik (UX tuning only):* whether the `Verified ∧ cross-self ∧ transparent
∧ clean` case should **truly auto-load with no human glance** or always surface a
**one-line confirmation** ("loaded N turns from Nora \[Verified]") is a UX
value-judgment, not a mechanism change. The **matrix and the escalate-on-flag rule**
are the ADR commitment; the exact friction on that one happy-path cell is Erik's to
bless.

### OQ3 — Are opaque payloads always sealed? — **RESOLVED: yes (settled by S-1 + S-5)**

**Resolution — YES. Opaque payloads are *always* sealed** (encrypted-at-rest to
recipients per HQ4); sealing is **mandatory** for `opaque-blob-v1` and any future
opaque kind, and **optional** for transparent kinds (a public conversation cache or
summary may be published unsealed, encryption-as-ACL being opt-in per HQ4). Genesis/
publish tooling **refuses to emit an unsealed opaque snapshot.**

*Why* — it falls out of S-1 and S-5 directly:

1. **S-1 containment.** An opaque blob can smuggle a key/secret (un-introspectable,
   D5). Sealing means a thief who fetches the ciphertext from a dumb third location
   `L` learns **nothing** — the exfiltration channel is closed at the storage/transport
   layer, leaving only the inherent residual (a secret reaching a *legitimate*
   recipient), which custody (ADR-003) already minimizes by keeping the only root copy
   off the worker.
2. **S-5 blast-radius bound.** A sealed opaque blob can be unwrapped **only by an
   intended recipient**, so a poisoned opaque blob **cannot be broadcast to arbitrary
   loaders** — the recipient set *is* the ACL (FR-S3), bounding who can even be exposed
   to the poison. An unsealed public opaque blob would be loadable by anyone, the
   worst S-5 surface.
3. **Consistency with the "opaque = maximally untrusted, maximally contained"
   posture** (D5/OQ1): opaque kinds cannot be inspected, so they get the strongest
   *structural* protections instead — always sealed, sandbox-only, strict
   `model_binding`, mandatory trust gate.

*Cost (disclosed).* This is the S-1 cost made concrete: the "just publish the blob and
anyone loads it" public-CAS simplicity is **gone for opaque kinds** — opaque state is
inherently a **private, per-recipient** artifact, never a public one, and cannot be
deduplicated/shared across recipients without re-wrapping. Acceptable: opaque hidden
state is intimate to one identity and one recipient set by nature.

*Not a fresh Erik call* — it is forced by S-1 + S-5 and is **settled here**. The one
adjacent knob is deliberately decided the *other* way: a **transparent** kind found
to carry secret-shaped content is **not** force-sealed — it is **refused by the scan**
(OQ1 category 2, FR-S1), because a transparent kind has no legitimate reason to carry
a key. Sealing is the containment answer for the *un-inspectable* case only.

---

## References

- `docs/federation-study/06-decision-memo-and-roadmap.md` — §1 (S-5 named as one of
  the three budgeted substrate findings), §3 HQ10 (the loadable-state decision this
  ADR formalizes), §5 Wave 5 (the S-5 safety layer's implementation wave), §6 ADR-004
  stub, §7.7 (design-the-slot-not-the-payload non-goal), §8 item 5 (the open-question
  hand-off: scan contents + auto-load threshold).
- `docs/federation-study/05-adversarial-evaluation.md` — **S-1** (opaque-state
  key/secret exfiltration), **S-5** (loadable-state poisoning / stored
  prompt-injection — the AI-specific threat, no prior-art precedent), §3.1
  (the download attack steps D-1→S-1), §3.2 (the custody-split verdict), §8 (the
  handoff budgeting S-1/S-5).
- `docs/federation-study/04-candidate-architectures.md` — §1.4b (the `StateSnapshot`
  envelope: `payload_kind`, `model_binding`, `content_cid`, `prev`, `enc`), §1.4
  (BLAKE3 content-addressing + canonical encoding), §1.5 (`src/identity/` skeleton:
  `envelope.rs`/`acl.rs`).
- `docs/federation-study/03-requirements-and-hard-questions.md` — HQ10 (loadable-state
  format), FR-I3 (versioned model-agnostic state), FR-I4 (content-addressed + signed),
  FR-I7 (stable identity ≠ mutable snapshots), FR-S1 (no field carries a key), FR-S3
  (encryption = ACL), FR-T3 (trust-level dispatch gating), NFR-4 (graceful
  forward/backward-compat degradation), T8 (abstract-vs-usable tension).
- `docs/ADR-fed-001-identity-key-model.md` — the `wgid:` self-certifying address (D1),
  the sigchain provenance check (D2), the stable-address-vs-state separation (D4), the
  compat handshake (D7), and the freshness-attestation mechanism + Δ tiering (OQ4)
  this ADR's cross-trust load reuses as a high-value action.
- `docs/ADR-fed-003-custody-delegation-recovery.md` (Proposed, sibling) — the custody
  boundary that holds the only key copy, on which D5's runtime-containment guarantee
  rests.
- `src/agency/types.rs` — the existing `TrustLevel` enum
  (`Verified` / `Provisional` / `Unknown`) OQ2's gate is grounded in.
