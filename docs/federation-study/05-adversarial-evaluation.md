# Federation Study 5/6 — Adversarial Evaluation, Threat Model & Ranking

> **Headline federation study, wave 1, task 5 of 6 — the *evaluate* phase.**
>
> This document **attacks** the four candidate architectures from doc 04. It is a
> red-team pass, not a review: the goal is to find where each design *breaks*,
> classify each break **fatal vs mitigable**, and produce a **defended ranking**.
> This is the security/reliability gate before doc 06's decision.
>
> Inputs: `04-candidate-architectures.md` (the candidates under attack),
> `03-requirements-and-hard-questions.md` (the FR/NFR contract + 12 HQs the
> attacks target), `01-prior-art-landscape.md` (§4.1 custody, §4.2 recovery — the
> real-world failure modes), `02-current-state-baseline.md` (§2.4 — WG has *zero*
> signing crypto today, so every attack lands on code that must be *built*).
>
> Downstream: `fed-decision` (6/6) chooses against these findings.

**Status:** draft for evaluation · **Date:** 2026-06-24 · **Owner task:** `fed-adversarial`

---

## 0. How to read this document

The candidates share a large substrate (doc 04 §1: one sigchain, one envelope
suite, one custody pattern). Most attacks therefore land on the **substrate** and
hit all four; a minority exploit a candidate's **divergent** anchor (A's gossip,
B's node, C's optionality, D's domain). So:

- **§1 — Threat model.** The attacker capabilities (A1–A8) and the **nine threat
  classes** (TC1–TC9) every candidate is attacked across. Read first.
- **§2 — Substrate attacks (S-1…S-7).** The breaks that hit *all four* because
  they exploit the shared design (doc 04 §1). Defined once, referenced by ID
  below so the per-candidate sections aren't 7× redundant.
- **§3 — The headline attack: downloaded-identity → impersonation.** The crux the
  brief flags *specifically* (doc 03 HQ1). Does the agent-key-custody split
  actually hold? Cross-candidate deep-dive + the verdict + the
  **decentralization-vs-custody irony**.
- **§4 — Per-candidate adversarial pass.** Each of A/B/C/D attacked across all
  **nine** threat classes (one row each), then its 2–4 *sharp divergent* findings
  in prose. This is the per-candidate ≥8-threat-class coverage.
- **§5 — Consolidated failure-mode register.** Every finding in one table:
  fatal/mitigable, the mitigation, its cost.
- **§6 — Scoring.** The eight-axis rubric, each candidate scored 1–5 with
  justification.
- **§7 — Defended ranking.** The security/reliability-gate verdict, the phased
  reading, and the D-as-component nuance.
- **§8 — Handoff to doc 06.**

**Severity** = Critical / High / Medium / Low (worst-case impact × attacker
reach). **Disposition** = **Fatal** (breaks a MUST with no fix that preserves the
candidate's premise) · **Mitigable** (a named control reduces it to acceptable
residual at a stated cost) · **Inherent-bounded** (cannot be eliminated, only
disclosed and capped — doc 03's stance for the metadata/FS non-goals).

---

## 1. Threat model

### 1.1 Attacker capabilities

We assume the crypto primitives (doc 04 §1.3: ed25519 / X25519 / XChaCha20 /
BLAKE3) are sound — no candidate is attacked by "break ed25519." Every attack
below is an attack on the **architecture around** the primitives. Eight capable
adversaries, composable:

| ID | Adversary | Can do |
|----|-----------|--------|
| **A1** | **Network adversary** (Dolev–Yao on the wire) | observe, drop, delay, reorder, replay, inject bytes on any link/relay. Cannot forge a signature. |
| **A2** | **Malicious relay / central-node operator** | a relay or node *the victim depends on*: censor, withhold, equivocate (show different views to different viewers), log metadata, go offline. |
| **A3** | **Malicious host operator** (worker-host root) | full control of a box where the *victim's agent* executes: read process memory, env, files, the worker's signer key, and any socket the worker holds. |
| **A4** | **Malicious keyed peer / counterparty** | a legitimately-keyed participant who lies, floods, abuses delegation, or poisons shared state. |
| **A5** | **Sybil farmer** | mint unlimited keypairs cheaply (keys are free in every candidate). |
| **A6** | **Thief / opportunist** | obtains a *published identity bundle* (the "download Nora" artifact) and tries to impersonate. |
| **A7** | **Hostile insider** | a social-recovery guardian, a node-custodian, or a delegated agent turning hostile or being coerced. |
| **A8** | **State / legal adversary** | seize a domain, compel a CA, hijack DNS, court-order or subpoena a node/registrar operator. |

### 1.2 The nine threat classes (every candidate is attacked across all nine)

| ID | Threat class | The core question |
|----|--------------|-------------------|
| **TC1** | **Identity forgery / impersonation** (incl. **downloaded-identity → impersonation**) | Can an attacker produce artifacts an honest client accepts as the victim — including by *downloading* the published identity? Does the custody split hold? (HQ1, FR-I2/S1) |
| **TC2** | **Key compromise / theft** | A3 steals the host-held signer. What is the blast radius, and how fast can it be revoked? (HQ1/HQ2, FR-S7) |
| **TC3** | **Key loss / unrecoverable identity** | The recovery model itself fails (lost keys, lost guardians, lost domain). Is the identity dead? (HQ2, FR-S2, V6) |
| **TC4** | **Sybil / spam / fake-identity flooding** | A5 floods inboxes / poisons trust with free keys. (HQ8, FR-T2) |
| **TC5** | **Relay / central-node compromise, censorship, outage** | A2 censors, equivocates, or disappears. What degrades — convenience or correctness? (HQ6, FR-F4/F5) |
| **TC6** | **Network partition → consistency / split-brain** | Two hosts diverge under partition (incl. the *sigchain itself*). Does shared state — or identity state — fork irrecoverably? (HQ7, FR-M6) |
| **TC7** | **Privacy: ACL/encryption bypass + metadata leakage** | C reads what it shouldn't; who-talks-to-whom leaks. (HQ4, FR-S3/S4) |
| **TC8** | **Replay / downgrade / ordering** | A1 replays old events, rolls back the sigchain past a revocation, or strips strong crypto. (HQ12, FR-M6) |
| **TC9** | **Malicious / buggy agent abusing its key / persistent personas** | A4/A7 inside the trust boundary: scope escape, self-perpetuating delegation, poisoned loadable state. (HQ9/HQ11, FR-T4) |

---

## 2. Substrate attacks (S-1…S-7) — these hit all four candidates

Doc 04 §1 is a single shared design. Seven attacks exploit it directly, so they
apply to **A, B, C, and D alike** (the per-candidate sections in §4 reference
these by ID and only add what *diverges*). These are the most important findings
in the document precisely because no candidate choice escapes them.

### S-1 — Opaque-state key/secret exfiltration (TC1, TC7) · **Mitigable**

FR-S1's acceptance signal is *"static analysis / format spec guarantees no field
can carry a private key."* That guarantee is **achievable for the typed
`IdentityRecord`/`SignedEvent`** (every field is declared) but **impossible for an
opaque `payload_kind`** (doc 04 §1.4b — the deliberately-evolvable slot for V1's
future hidden/RNN state, doc 03 HQ10). An opaque blob is by definition
un-introspectable, so it can smuggle the root signer, a session token, or another
identity's key *out through the custody boundary* under a valid signature. A
malicious or buggy agent that bakes its key into its "conversation cache" turns
the portable-state feature into a key-exfiltration channel.

- **Mitigation:** the custody boundary must hold the *only* copy of the root/signer
  (so there is no key in the worker's address space to bake in — §S-2); seal
  opaque payloads to recipients (encrypt-at-rest so even a leaked blob is opaque
  to a thief); content-scan transparent kinds; and downgrade FR-S1 from a *static*
  guarantee to a *runtime-containment* guarantee for opaque kinds, disclosed.
- **Cost:** the headline "just publish the blob and anyone can load it" loses its
  simplicity — opaque state must be treated as untrusted and sandboxed (ties S-5).

### S-2 — Custody-oracle access leakage / confused deputy (TC1, TC2) · **Mitigable**

Every candidate keeps the root/signer off the worker and signs through an
**ssh-agent-style request-signature boundary** (doc 04 §1.1, §1.5 `custody.rs`).
But the boundary protects the *key bytes*, not necessarily *access to the signing
oracle*. If "download Nora onto host B" also copies the ambient credential or
endpoint config that lets a worker *reach Nora's custodian and request signatures*,
then host B can sign as Nora **without ever holding a key** — download ≈
impersonation by confused deputy. The custody split can hold at the byte level and
still fail at the access level.

- **Mitigation:** the custodian authenticates the *requesting host/agent identity*
  (a per-host enrolled signer key), not a bearer token that travels in the bundle;
  signing requests are **intent-bound** (sign *this digest for this purpose*), not
  "sign anything"; rate-limited and logged.
- **Cost:** per-host enrollment friction — "download and it just works" needs an
  explicit re-authorization step. (That step is *correct*: it is exactly the
  fork-vs-same-self boundary, doc 03 FR-I5, made unskippable.)

### S-3 — Revocation liveness / freeze-eclipse (TC5, TC8) · **Mitigable**

Revocation (FR-S7) is the load-bearing recovery-from-compromise primitive, and it
**requires freshness**: a `revoke_key` link only protects a verifier who *sees it*.
On an async, offline-tolerant, store-and-forward network (the whole point — doc 03
NFR-2, FR-M2), an attacker (A1/A2) who eclipses a victim's relays or serves a
**stale, validly-signed sigchain head** (the *freeze attack*) keeps a revoked or
stolen key alive indefinitely. The signature check *passes* — the old head was
genuinely signed — only freshness detects the rollback. **The architecture's core
strength (offline tolerance) directly weakens its core safety primitive
(revocation).**

- **Mitigation:** monotonic sigchain-head counter + short-lived signed
  *freshness attestations* (a "valid-as-of T, expires T+Δ" the verifier re-fetches);
  relay/endpoint diversity (≥N independent); high-value actions **fail closed on
  stale**.
- **Cost:** a liveness requirement intrudes on offline tolerance — you must reach
  *some* fresh source within Δ; introduces clock dependence (and clock-skew
  attacks). Worst in A (no authoritative head); bounded in B/D (node/doc-host
  serves the canonical head — but a *compromised* host can still freeze).

### S-4 — Delegation self-perpetuation ("hydra") (TC9) · **Mitigable, but costly for A**

The most dangerous insider attack. Doc 04 §2.1 says same-self enrollment is *"B's
signer key added to Nora's sigchain **by a surviving authorized key** (`add_key`
link)."* If **any authorized signer** can issue `add_key`/`delegate`, then a
compromised *delegated* agent (A3 steals a worker's signer) can authorize a *new*
key it controls, and that key can authorize another — revoke one head and the next
is already live. The malicious `add_key` is *auditable* (append-only, doc 04 §1.2)
but not *prevented*. A leaked agent key becomes a self-renewing persistent persona.

- **Mitigation:** **delegated signers MUST NOT grow the authorized set** —
  `add_key`/root-rotate is restricted to the root/custodian (or M-of-N); UCAN-style
  sub-delegation must be *attenuating only* (can narrow, never widen, and inherits
  the parent's expiry); revocation operates at **issuer-subtree** granularity (kill
  the parent, kill the whole subtree).
- **Cost:** this **directly conflicts with A's recovery model.** A's *only*
  no-node recovery path is "a surviving authorized (non-root) key adds a new
  signer" (doc 04 §2.2). Restricting `add_key` to root removes the surviving-key
  recovery primitive A depends on. **A cannot have both cheap surviving-key
  recovery and hydra-resistance** — a genuine bind (see §4.1). B (node mediates
  enrollment), C (can restrict), and especially D (expiry caps every subtree
  structurally) resolve it far more cheaply.

### S-5 — Loadable-state poisoning / stored prompt-injection (TC9) · **Mitigable**

V1's premise is loading a `StateSnapshot` to *resume a continuous self* (doc 03
V1, R2). A signature proves *who authored* the state, **never that the state is
safe to load.** A malicious or compromised agent (A4) publishes a validly-signed
"conversation cache" containing a prompt-injection or poisoned summary; the next
host that loads it to "resume Nora" inherits the poison — a persistence +
lateral-movement vector that rides the legitimate portability feature. This is
AI-substrate-specific and has no analogue in Nostr/Keybase/atproto.

- **Mitigation:** treat loaded state as **untrusted input** — sandbox/scan,
  enforce `model_binding` (doc 04 §1.4b), provenance-gate (load only from authors
  at sufficient `trust_level`, FR-T3), human-in-loop for cross-trust loads.
- **Cost:** erodes the seamless-resume UX; requires an AI-input-safety layer WG
  does not have today. Residual is **inherent**: signature ≠ safety.

### S-6 — Forward-secrecy vs async tension (TC7) · **Inherent-bounded**

Encryption=ACL (doc 04 §1.3/§2.7) uses **static** X25519 recipient keys. Stealing a
static enc key (TC2) **retroactively decrypts every logged ciphertext** ever
addressed to it. Forward secrecy (Double-Ratchet/MLS, offered as opt-in) fixes
this — **but forward secrecy and send-to-an-offline-peer do not compose**: you
cannot ratchet with a party who is not there to ratchet back. The email-speed,
both-ends-offline premise (FR-M2) *fights* forward secrecy.

- **Mitigation:** MLS/ratchet for online or long-lived groups; accept static-key
  (no FS) for offline store-and-forward; choose per-conversation.
- **Cost / residual:** you cannot have both, by construction — disclosed and
  capped by enc-key rotation. **Inherent** to the chosen latency budget.

### S-7 — Plaintext / crypto downgrade (TC7, TC8) · **Mitigable**

Two downgrade surfaces in the shared envelope: (a) `SignedEvent` carries `body`
(plaintext) **or** `ciphertext` (doc 04 §1.4c) — a buggy/hostile sender silently
emits plaintext; (b) crypto-agility (doc 04 §1.3: per-structure `alg` id,
dual-sign during a suite migration) lets A1 **strip the strong signature** or force
both peers to the lowest common `alg`.

- **Mitigation:** the compat handshake (`WG_FED_COMPAT_VERSION`, doc 04 §1.5) must
  be **authenticated** (sign the negotiated parameters, not just exchange them);
  per-conversation **"MUST encrypt"** policy enforced at send; a **minimum-alg
  floor** with aggressive retirement (refuse known-weak, never merely "lowest
  common"); WG's loud-fail-on-mismatch convention.
- **Cost:** policy must be maintained and enforced; retiring an alg loudly breaks
  old artifacts (acceptable per doc 03 HQ12, but real).

> **Substrate verdict.** None of S-1…S-7 is *fatal* — each has a named mitigation —
> but **S-3 (revocation liveness), S-4 (hydra), and S-5 (state poisoning) are the
> three the decision memo must budget engineering for regardless of which candidate
> wins**, and **S-4 is the only substrate attack whose mitigation cost differs
> sharply by candidate** (it is nearly free for D, expensive for A). They are the
> shared price of admission.

---

## 3. The headline attack — downloaded-identity → impersonation (TC1)

The brief flags this *specifically*, and doc 03 names it the crux: *"download
Nora's identity" must never become "impersonate Nora"* (HQ1, FR-I2, FR-S1). Here is
the attack, run against all four, and the verdict on whether the custody split
holds.

### 3.1 The attack tree

A6 obtains Nora's published bundle (`IdentityRecord` + `StateSnapshot`s + public
keys — doc 04 §1.4) and tries to author a new event honest clients accept as Nora.

1. **D-paper (the bundle excludes the key).** By spec the bundle carries no
   private key (FR-S1). A6 holding it can *verify and render* Nora, not *sign* as
   Nora. **The split holds on paper in all four.** This is the easy, intended case
   — and it is *not* where the attack lives.
2. **D-1 → opaque leak (S-1).** If Nora's state includes an opaque `payload_kind`,
   the key may have been smuggled in. A6 extracts it. **Break, all candidates,
   mitigated by S-1.**
3. **D-2 → oracle access (S-2).** If the bundle confers reach to Nora's signing
   custodian, A6 requests signatures. **Break, all candidates, mitigated by S-2.**
4. **D-3 → hostile add_key (S-4).** If A6 first compromises *any* authorized signer
   (TC2), it issues an `add_key` enrolling its own key — now a *legitimately
   authorized* persistent Nora. **Break; worst in A by design, mitigated by S-4.**
5. **D-resolver → equivocation (the candidate-divergent break).** A6 doesn't need
   Nora's key at all if it controls **how the victim resolves `Nora → key set`**.
   Serve a forged sigchain/DID-doc authorizing A6's key:
   - **A:** must forge a sigchain whose links chain to the *genesis pubkey the
     victim already pinned* — **fails** under TOFU. A6's only opening is
     **first-contact substitution** (the victim pins A6's key as "Nora" before any
     real Nora exists — Zooko, no human-meaningful name; §4.1 A-1).
   - **B:** the **node holds Nora's signing key** — a hostile node (A2/A7) signs as
     Nora *directly*, no forgery needed, until the offline recovery key overrides it
     (§4.2 B-1). Largest impersonation surface; uniquely *recoverable*.
   - **C:** as strong as A *if* the verifier always self-verifies; as weak as B *if*
     it trusts the optional directory hint without fallback — **a downgrade
     surface** (§4.3 C-1).
   - **D:** resolution is `did:web` over HTTPS → **DNS + CA + web-host** are all in
     the trust path. A1-with-a-CA or A8 serves a forged `did.json` authorizing
     A6's key → **full impersonation, no key theft required** (§4.4 D-1). Easiest
     of the four.

### 3.2 Does the custody split actually hold? — the verdict

**On paper, yes, in all four** (step 1). **In practice, its strength is gated by
three implementation controls, each an attack surface**: the bundle must
*provably* exclude the key (S-1 — impossible to guarantee statically for opaque
state); download must not confer *oracle access* (S-2); same-self enrollment must
require a control the downloader lacks (S-4). **Ranked by how structurally each
candidate enforces all three:**

> ### **D ≈ B  >  C  >  A**

- **D** — authority *is the UCAN token, not the key* (doc 04 §5.1); a downloaded
  agent holds an expiring capability, never the root; a stolen signer is
  near-worthless after expiry. Strongest structural split — *if* you ignore that
  D's resolver (§3.1, D-resolver) is the weakest (the split holds; the *anchor*
  doesn't — see §4.4).
- **B** — the signing key **lives in the node, never on the worker** (doc 04
  §3.1); the node mediates enrollment, so a downloaded repo without a node-signed
  rotation op is inert. Strong structural split; the residual is that the *node*
  is the custodian-of-record and a hostile node breaks it (recoverably).
- **C** — *can* match D or B per its config (Farcaster-signer or UCAN mode), but
  inherits the weaker mode's surface unless configured strictly.
- **A** — weakest: a **standing signer on the worker host** (A3's prize) and
  **any-authorized-key can `add_key`** (S-4 by design). With **no
  custodian-of-record**, A's "custody boundary" is the user's own discipline.

### 3.3 The irony the adversarial pass exists to surface

**Decentralization and custody-strength pull against each other.** The candidate
that best honors V5's decentralization lean (**A**, no authority) is the one where
"download ≠ impersonation" rests on the *thinnest* enforcement — because there is
no custodian-of-record, only the user's discipline. The candidates with a
custodian (**B**'s node, **D**'s UCAN-issuer) enforce the split *structurally*,
precisely *because* they accepted a more central trust anchor. **The crux feature
(non-impersonable portable identity) is best served by the *less* decentralized
designs.** Doc 06 must price this directly: V4 (self-certifying non-impersonation)
and V5 (maximal decentralization) are in tension *at the custody layer*, not just
at the convenience layer.

---

## 4. Per-candidate adversarial pass

Each candidate is attacked across all nine threat classes (the table), then its
sharp divergent findings are developed in prose. Substrate findings S-1…S-7 apply
to every candidate and are cited, not repeated.

### 4.1 Candidate A — Fully decentralized P2P

| TC | Attack on A | Severity | Disposition |
|----|-------------|----------|-------------|
| **TC1** forgery / **downloaded-identity** | Forgery fails once pinned (self-certifying, no node/DNS to subvert — A's integrity strength). Real vector: **first-contact key substitution** (A-1) — no human name, so the victim pins whatever a relay served. Downloaded-identity: split weakest of the four (§3.2). | High | Mitigable (S-1/S-2/S-4 + A-1) |
| **TC2** signer theft | A3 steals the **standing signer on the worker**; signs as A until a `revoke_key` propagates — and propagation is **gossip-slow + freeze-attackable** (S-3). Largest *temporal* blast radius. | High | Mitigable (S-3 + short signer lifetimes) |
| **TC3** key loss | **Social M-of-N only.** Pre-arranged-or-death; agents have no natural guardians (A-4). | High | **Fatal** for the careless / agents; mitigable for the disciplined |
| **TC4** sybil / spam | **Weakest of the four** — PoW + consent + WoT only; no anchor (A-2). doc 04 §8.2 itself defers FR-T2 for A. | High | Mitigable, **never solved** without an anchor |
| **TC5** relay compromise | Relays dumb + plural → compromise degrades **reach, not correctness**. Sharp case: **eclipse → freeze revocation** (A-3 + S-3). | Medium | Mitigable (relay diversity) |
| **TC6** partition / split-brain | **No serialization point** → the *sigchain itself* can fork under partition (A-3); fork-choice is policy, attackable to suppress a `revoke`. | High | Mitigable at a decentralization cost |
| **TC7** privacy / metadata | E2E sealed envelopes; `to` leaks per-relay but **distributed** across self-chosen relays → best metadata posture *if* relays are diverse. FS tension S-6. | Medium | Inherent-bounded |
| **TC8** replay / downgrade | `id` dedup defeats naive replay; **sigchain rollback/freeze** is the live one (S-3); downgrade S-7. | Medium | Mitigable (S-3/S-7) |
| **TC9** malicious agent | **Hydra by design** (S-4): any authorized key can `add_key` → self-perpetuating persona. State poisoning S-5. | Critical | Mitigable, **but the fix costs A its recovery model** |

**A's defining bind — hydra-vs-recovery (S-4 × TC3/TC9).** A's only no-node
recovery is "a surviving authorized key adds a new signer." That is *the exact
capability* a hydra needs. Lock `add_key` to root-only and you kill the hydra *and*
A's recovery primitive together; leave it open and a single stolen signer becomes
an immortal persona. There is no clean A-shaped resolution — both fixes
reintroduce a custodian or a witness, which is no longer A. **This is the single
strongest adversarial argument against A as a v1.**

**A's quiet strength.** On pure *integrity* (TC1 forgery, TC5 correctness) A is
excellent: nothing to subvert but the math. Its weaknesses are entirely on the
**abuse / availability / recovery** axes (TC3, TC4, TC9) — exactly the axes a
security/reliability gate weights heaviest. A is *cryptographically purest and
operationally most fragile.*

### 4.2 Candidate B — Central-node-anchored federation

| TC | Attack on B | Severity | Disposition |
|----|-------------|----------|-------------|
| **TC1** forgery / **downloaded-identity** | **Hostile node signs as every hosted identity directly** (B-1) — largest impersonation surface, *but* the offline recovery key overrides within a window (atproto-proven). Downloaded-identity split is structurally strong (§3.2): key never on the worker. | Critical (in-window) | Mitigable (recovery key) |
| **TC2** signer theft | A3 (worker root) **never had the key** — it lives in the node. Quiet strength. Target moves to the **node** → compromise steals *all* hosted keys at once. | Critical (node) / Low (worker) | Mitigable (harden + recovery key) |
| **TC3** key loss | **Strongest recovery of the four**: rotation keys + offline recovery key + migration + app-passwords. Residual: the recovery key is a **standing takeover capability** (B-4). | Low | Mitigable |
| **TC4** sybil / spam | **Node is a natural choke point** → per-node rate-limit + handle/DNS cost. Strong. Residual: a malicious node vouches for its own sybils. | Low | Mitigable (well) |
| **TC5** node compromise | **Most consequential single event of any candidate** (B-1): mass impersonation + mass key theft + censorship of all hosted users. Directory equivocation **well-mitigated** (mirrorable signed log + `.well-known`, B-3). | Critical | Mitigable (recovery key bounds it) |
| **TC6** partition / split-brain | **Least exposed** — the node is a serialization point; identity state has one authoritative writer, no sigchain fork by construction. | Low | (no finding) |
| **TC7** privacy / metadata | **Worst metadata posture** (B-2): your own node sees your *entire* social graph. Sealed-sender hides `from` from *peer* nodes only. | High | Mitigable only partially |
| **TC8** replay / downgrade | Node serves the canonical latest head → rollback bounded (unless node compromised). Downgrade S-7. | Low | Mitigable |
| **TC9** malicious agent | Node mediates key ops → can enforce owner-only `add_key`, app-passwords scoped+revocable → **hydra resisted if node honest** (a compromised node *is* the hydra). State poisoning S-5. | Medium | Mitigable |

**B's defining risk — the node is a key-holding single target (B-1).** B trades
A's *many small* exposures (a signer per worker) for *one big* one (all signing
keys in the node). Compromise the node and you impersonate and loot every hosted
identity at once — "a juicier target than a dumb relay" (doc 04 §3.10, conceded).
**But B is the *only* candidate whose mass-compromise is recoverable**: the
offline, higher-priority recovery key overrides a hostile node within a bounded
window (atproto's 72h model, doc 01 §4.2 ★★★). The in-window exposure is real and
must be disclosed; the recovery property is what keeps B's security score above
A's despite the larger blast radius. **B concentrates risk into one box you can
harden, monitor, and recover from — the opposite of A, which diffuses risk into N
boxes you can do none of those to.**

**B's privacy admission (B-2).** "Your node knows everyone you talk to" is
unavoidable in the node model — the node must see `to` to route. Self-hosting your
node makes *you* the observer (acceptable for a household) but a hosted user's node
operator (A7) sees the full graph. This is a metadata loss, not an integrity loss,
and it is honestly disclosed (FR-S4) — but it is the worst of the four.

### 4.3 Candidate C — Hybrid (key core + optional relays/directory)

| TC | Attack on C | Severity | Disposition |
|----|-------------|----------|-------------|
| **TC1** forgery / **downloaded-identity** | As strong as A *if* the verifier always self-verifies; as weak as B *if* it trusts the optional directory without fallback → **resolution-downgrade** (C-1). Downloaded-identity split matches whichever mode is configured (§3.2). | High (if misconfigured) | Mitigable (fail-safe defaults) |
| **TC2** signer theft | Tunable: Farcaster-signer (= A's exposure) **or** UCAN short-lived (= D's small blast radius). C can *choose* the safer mode. | Medium | Mitigable (choose UCAN mode) |
| **TC3** key loss | A's *and* B's recovery, composable → best-of-both *if* a node is opted into; pure-P2P C = A's weakness. | Low–High (config-dependent) | Mitigable |
| **TC4** sybil / spam | Layered: B's strength where a node is in the path, A's weakness on the pure-P2P path. Tunable. | Medium | Mitigable |
| **TC5** central compromise | Nothing correctness-critical is central (the right invariant) — *if* the optional path **fails safe** (lose the directory → fall back to self-verify), never **fails open** (trust a forged directory). The invariant is an *engineering claim that must be proven* (C-1). | Medium | Mitigable (must prove fail-safe) |
| **TC6** partition / split-brain | Can borrow B's serialization point when a node is present; pure-P2P path inherits A's sigchain-fork risk. | Medium | Mitigable |
| **TC7** privacy / metadata | Leak surface **scales with the central components enabled** — disclosed per-deployment (doc 04 §4.7). | Medium | Inherent-bounded |
| **TC8** replay / downgrade | Inherits the configured mode's posture; the optionality adds a **path-downgrade** surface (force the victim onto the weaker resolver, then attack it). | Medium | Mitigable |
| **TC9** malicious agent | Can restrict `add_key` / use UCAN-attenuating delegation (resolves S-4) — *if* configured to. Farcaster-signer mode inherits A's hydra unless restricted. | High (if misconfigured) | Mitigable |

**C's defining risk — optionality *is* the attack surface (C-1, C-2).** Every
"optional" central piece is a path an attacker can try to *force the victim onto*
(downgrade) or that an operator can *misconfigure* into the trusting position.
C's security is **bimodal**: a strictly-configured C is the strongest of the four
(it can adopt each candidate's best-defended mode per threat); a sloppily-configured
C can be **weaker than a careful A or B**, because it has more switches to set
wrong. C also has the **largest attack surface to test and keep coherent** — doc
04 §4.10 names this its chief risk. "Secure by composition" must be *proven*, not
assumed; the §9 phasing helps (each phase is independently auditable) but until the
late phases C *is* A or B and inherits their open findings.

The mitigation is **fail-safe-by-default**: the self-certifying core is the
correctness root, central pieces are *hints that can only help, never override a
self-verification*; ship a strict mode; lint the resolution cascade (WG already has
`wg config lint`, CLAUDE.md). The cost is the discipline and the test matrix — the
verification burden is the price of C's flexibility.

### 4.4 Candidate D — Wildcard: capabilities-first (did:web + UCAN)

| TC | Attack on D | Severity | Disposition |
|----|-------------|----------|-------------|
| **TC1** forgery / **downloaded-identity** | **DNS/CA/web-host in the trust path** → A1-with-a-CA or A8 serves a forged `did.json` authorizing their key → **full impersonation, no key theft needed** (D-1). *Easiest impersonation of the four.* The UCAN *delegation* split is strongest (§3.2); the *anchor* is weakest. | Critical | **Fatal as identity root**; mitigable only via did:key fallback (which drops D's premise) |
| **TC2** signer theft | **Smallest blast radius of the four** — the agent's signer is disposable; authority rides an expiring UCAN; a stolen signer is near-worthless after expiry. D's standout strength. | Low | Mitigable (short expiry) |
| **TC3** key loss | **Recovery = domain control** → hostage to registrar/DNS (D-2). DID-doc edit rotates keys cleanly, but lose the domain (lapse, seizure, registrar-account theft) → lose the identity unless did:key fallback is pre-configured. | High | **Fatal-as-primary** / mitigable via fallback |
| **TC4** sybil / spam | **Domains cost money + are accountable** → strongest anti-sybil. Residual: free-subdomain hosts (`*.somehost`) collapse the cost. | Low | Mitigable (well) |
| **TC5** central compromise | **Domain web-host + DNS + CA are the central nodes** — compromise/seizure = impersonation (D-1) + censorship + identity loss (D-2). **Strongest central dependency, hardest to mitigate** (DNS/CA are outside WG's control). A8-exposed by design. | Critical | **Fatal-as-primary** |
| **TC6** partition / split-brain | DID-doc host serializes the identity doc (good); but **UCANs issued during a partition can't be un-issued** → split-brain risk is in **revocation lag** (D-3). | Medium | Mitigable (short expiry) |
| **TC7** privacy / metadata | Domain host sees the graph; **transparency log = permanent, world-readable metadata** (D-4) — audit-by-design is privacy-leak-by-design. | High | Mitigable (omit the log) / by-design-leak |
| **TC8** replay / downgrade | UCAN replay bounded by expiry + nonce; **fast revocation before expiry is UCAN's open problem** (D-3). Downgrade S-7. | Medium | Mitigable (short expiry) |
| **TC9** malicious agent | **Best structural defense** — UCAN sub-delegation is *attenuating + expiring* by construction → hydra (S-4) self-limits; scope is enforced on the capability chain. Strongest HQ11. State poisoning S-5. | Low | Mitigable (best of the four) |

**D's defining contradiction — the anchor fights the vision (D-1, D-2).**
D has the **best delegation/custody layer of all four** (TC2, TC9: UCAN expiry +
attenuation give the smallest signer blast radius and the strongest hydra
resistance) bolted onto the **weakest identity root** (TC1, TC5: DNS/CA/web-host).
The root is load-bearing: a forged `did.json` is a *complete* takeover, achievable
by any adversary who can compel a CA, hijack DNS, or seize a domain (A8) — the
*exact* impersonation the study exists to prevent. This **contradicts V4 /
FR-I1's self-certifying non-impersonation.** The only escape is the `did:key`
fallback + a pre-published recovery key — but that is an admission that the domain
anchor is untrustworthy as a root, and it *abandons the human-meaningful-name
premise that is D's entire reason to exist*. At that point did:key is the real root
and did:web is a convenience alias — which is **C with a directory**, not a
distinct candidate.

**The decision-relevant nuance: D is last-as-a-whole, first-as-a-component.** Its
UCAN layer is the best-scoring delegation mechanism in the study and should be
**harvested into the winner** (grafted onto B/C as the in-architecture
custody/authority layer — exactly doc 04 §6.3's hint), *without* adopting did:web
as the identity root.

---

## 5. Consolidated failure-mode register

Every finding above, classified, with mitigation and its cost. **Fatal** =
breaks a MUST with no fix preserving the candidate's premise. **Mitigable** =
named control, acceptable residual. **Inherent-bounded** = disclosed + capped, not
eliminable.

### 5.1 Substrate (all four candidates)

| ID | Threat | Disposition | Mitigation | Cost of mitigation |
|----|--------|-------------|------------|--------------------|
| **S-1** | Opaque-state key/secret exfiltration (TC1/7) | Mitigable | Key only ever in custodian (never in worker memory to bake in); seal+scan payloads; runtime containment for opaque kinds | FR-S1 static guarantee → runtime-only for opaque kinds; lose "just publish the blob" |
| **S-2** | Custody-oracle access leakage / confused deputy (TC1/2) | Mitigable | Custodian authenticates requesting host-identity (not bearer token); intent-bound, rate-limited, logged signing | Per-host enrollment friction (= the fork/same-self boundary, correctly) |
| **S-3** | Revocation liveness / freeze-eclipse (TC5/8) | Mitigable | Monotonic head counter + short-lived freshness attestations; endpoint diversity; fail-closed on stale for high-value acts | Liveness req intrudes on offline tolerance; clock dependence |
| **S-4** | Delegation self-perpetuation / hydra (TC9) | Mitigable (cheap for D, **costly for A**) | Delegated keys cannot grow the set; only root/M-of-N adds keys; attenuating-only sub-delegation; issuer-subtree revoke | **Kills A's surviving-key recovery**; less flexible delegation |
| **S-5** | Loadable-state poisoning / stored prompt-injection (TC9) | Mitigable | Treat loaded state as untrusted (sandbox/scan); `model_binding` + provenance gate; human-in-loop cross-trust | Erodes seamless-resume UX; needs an AI-input-safety layer; signature ≠ safety (residual) |
| **S-6** | Forward-secrecy vs async (TC7) | Inherent-bounded | MLS/ratchet online; static-key for offline; per-conversation choice | Cannot have both FS + offline-send; static-key compromise retro-decrypts (capped by rotation) |
| **S-7** | Plaintext / crypto downgrade (TC7/8) | Mitigable | Authenticated handshake (sign negotiated params); per-convo MUST-encrypt; min-alg floor + retire; loud-fail | Maintain+enforce policy; alg retirement breaks old artifacts (loudly) |

### 5.2 Candidate-divergent

| ID | Threat | Candidate | Disposition | Mitigation | Cost |
|----|--------|-----------|-------------|------------|------|
| **A-1** | First-contact key substitution (TC1) | A | Mitigable | OOB fingerprint / WoT intro / self-hosted proof | UX friction; no human name → typical user pins what the relay served |
| **A-2** | Sybil/spam, no anchor (TC4) | A | Mitigable, **never solved** | PoW + consent gate + WoT-distance gating | PoW penalizes legit/mobile > botnets; caps A's open-network reach |
| **A-3** | Sigchain fork, no serialization point (TC6) | A | Mitigable at a decentralization cost | Witness/checkpoint **or** fork=policy-identity-split | A witness reintroduces a semi-central node (→ not A) |
| **A-4** | Recovery pre-arranged-or-death (TC3) | A | **Fatal** (careless/agents) / mitigable (disciplined) | Mandatory paper-key + guardians at genesis; agents → custodian is recovery anchor | Ceremony users skip → identity death; agent recovery ⇒ a custodian (not pure-P2P) |
| **B-1** | Node compromise = mass impersonation + key theft (TC1/2/5) | B | Mitigable (**uniquely recoverable**) | Hardened/HSM node + monitoring + **offline recovery-key override window** | In-window exposure real; operating a hardened node |
| **B-2** | Node sees whole social graph (TC7) | B | Mitigable only partially | Sealed-sender (peer nodes); self-host your node | Your *own* node still sees all `to` — inherent to the node model |
| **B-3** | Directory equivocation/outage (TC5) | B | Mitigable (well) | Mirrorable signed append-only log + `.well-known` self-resolve fallback | Low — genuinely well-handled |
| **B-4** | Recovery key = standing takeover capability (TC3) | B | Mitigable | Offline/hardware; M-of-N split; time-locked visible override | Moves the crux to one well-protected key (acceptable) |
| **C-1** | Optionality = downgrade/misconfig surface (TC1/5/8/9) | C | Mitigable, **chief risk** | Fail-safe defaults (self-cert core is correctness root; central = hints only); strict mode; lint the cascade | Engineering discipline + largest test matrix of the four |
| **C-2** | Largest build / coherence risk (maturity) | C | Mitigable via phasing | §9 phased rollout, each phase auditable; C is convergence not big-bang | Long road; until late phases C *is* A or B + their open risks |
| **D-1** | DNS/CA/web-host anchor = easiest impersonation + seizure (TC1/5) | D | **Fatal as identity root** | did:key fallback + recovery key + DNSSEC/CAA + CT monitoring | The fallback abandons D's human-meaningful premise (→ becomes C) |
| **D-2** | Domain loss = identity loss (TC3) | D | **Fatal-as-primary** / mitigable via fallback | did:key fallback + pre-published recovery key | Same — fallback undercuts the premise; violates FR-F5 without it |
| **D-3** | UCAN fast-revocation-at-scale unsolved (TC8/9) | D | Mitigable | Short expiries (revoke-by-expiry); issuer revocation lists; transparency cross-check | Short expiry = chatty re-issuance (fights offline); revoke-lists add a lookup dep |
| **D-4** | Transparency log = permanent public metadata (TC7) | D | Mitigable (omit) / by-design-leak | Opt-in only; log hashes not identities; or omit | Lose the tamper-evident audit feature that distinguishes D |

### 5.3 Fatal-finding summary

Only **three findings are Fatal**, and each is candidate-specific and *bounded*:

- **A-4 (recovery) is fatal for the careless user and for agents-without-guardians** —
  A's "no node" premise means there is no backstop if the genesis ceremony was
  skipped. Mitigable only by mandating ceremony (which users skip) or by
  introducing a custodian (which makes it C/B). For agents specifically, recovery
  *always* collapses to "the custodian's key is safe" — i.e., agents are never
  purely-P2P-recoverable in any candidate.
- **D-1 / D-2 (the DNS/CA/domain anchor) is fatal for D *as a primary identity
  root*** — it contradicts the self-certifying non-impersonation that is the
  study's whole point (V4/FR-I1), and the only mitigation (`did:key` fallback)
  dissolves D into "C with a directory." **D's anchor is disqualifying; D's UCAN
  layer is not** — it is the best component in the study.

Every other finding (the substrate S-1…S-7, and A/B/C's divergent ones) is
**Mitigable or Inherent-bounded**. Critically: **no candidate is fatally broken on
TC1 forgery, TC2 theft, or TC9 malicious-agent** at the architecture level — those
are all reducible to acceptable residual with the named mitigations. The fatal
breaks are concentrated in **recovery (A) and the identity anchor (D)**, which is
exactly where the candidates most diverge.

---

## 6. Scoring

Eight axes (doc 04's rubric, the brief's rubric), 1–5 (★ = filled). Scores are
*adversarial* — they weight worst-case and abuse-resistance, per this document's
purpose as a security/reliability gate, not best-case elegance.

| Axis | A | B | C | D | What the axis measures |
|------|:-:|:-:|:-:|:-:|------------------------|
| **Security** | 3 | 4 | 4 | 2 | Resistance across TC1–TC9, weighting worst-case + recoverability |
| **Decentralization** | 5 | 2 | 4 | 2 | No mandatory central root of trust (FR-F4/F5) |
| **Recoverability** | 2 | 5 | 5 | 3 | Survive key/host/domain loss (HQ2, FR-S2) |
| **Portability** | 4 | 5 | 5 | 3 | Download/move identity + guaranteed state availability (V2, NFR-3) |
| **Simplicity** | 3 | 4 | 2 | 3 | Few moving parts; small surface to get wrong |
| **WG-fit** | 2 | 5 | 3 | 2 | Distance from today's code (doc 02); reuse of the daemon/`secret.rs` |
| **Operational cost** | 3 | 4 | 3 | 3 | Total user+operator burden (NFR-5) |
| **Maturity** | 3 | 5 | 2 | 3 | Proven-ness of the blueprint end-to-end |
| **Unweighted total** | **25** | **34** | **28** | **21** | |

### 6.1 Per-axis justification

**Security — A 3 · B 4 · C 4 · D 2.** A has excellent *integrity* (TC1 forgery,
TC5 correctness — nothing to subvert but the math) but the worst *abuse/recovery*
profile (TC3 fatal-careless, TC4 weakest, TC9 hydra-by-design) → net 3. B's
node-holds-keys is the largest blast radius (B-1) but the *only* mass-compromise
that is **recoverable** (offline override) and it has the best anti-abuse and
consistency → 4, with the metadata caveat (B-2). C *can* adopt each candidate's
best-defended mode and holds the right invariant (nothing correctness-critical
central) → 4, but only *if configured strictly* (C-1) — the score assumes
discipline. D has the best delegation layer (TC2/TC9 strongest) on the weakest
anchor (TC1/TC5 Critical, **Fatal-as-root**) → 2: the anchor is load-bearing and
the study's whole point is non-impersonation, which D's anchor fails against A8.

**Decentralization — A 5 · B 2 · C 4 · D 2.** A: maximal, no authority. B:
federated-central, a directory dependency (mitigated, not removed). C: tunable,
self-certifying core, defaults decentralized but rests on nodes in practice → 4.
D: DNS trust root; the did:key fallback restores it but isn't the default → 2.

**Recoverability — A 2 · B 5 · C 5 · D 3.** A: social-M-of-N only,
pre-arranged-or-death (A-4) → 2. B: rotation + offline recovery key + migration +
app-passwords, the strongest (doc 01 §4.2) → 5. C: A's *and* B's, composable → 5.
D: clean DID-doc rotation but recovery=domain-control, hostage to the registrar
(D-2) → 3.

**Portability — A 4 · B 5 · C 5 · D 3.** A: self-certifying bundles publish
anywhere, but "is anyone *storing* my state?" is unsolved (no node) → 4. B:
account migration is atproto's proven headline + the node guarantees availability →
5. C: superset, same bundle through ≥2 transports → 5. D: the did:web URL is
*bound to the domain* — move domains = move identity (or fall back to did:key) → 3.

**Simplicity — A 3 · B 4 · C 2 · D 3.** A: one conceptual model but the user
carries keys/relays/guardians; DHT/eclipse/PoW are subtle → 3. B: the node does
the work, one authoritative writer, reuses the daemon → 4. C: **"most moving
parts"** (doc 04 §4.10), the optionality is a configuration-complexity tax → 2. D:
did:web is dead-simple (host a JSON file), UCAN adds verifier complexity → 3.

**WG-fit — A 2 · B 5 · C 3 · D 2.** Doc 02 is decisive: WG's federation is
*already* a daemon brokering cross-repo queries over a socket (doc 02 §2.1), and
`secret.rs` is the natural signing-key custodian (doc 02 §2.4). **B promotes the
existing daemon to a network node — the shortest path** (doc 04 §3.9) → 5. C is the
end-state of the phasing but the largest build → 3. A abandons the daemon-as-broker
for pure P2P → 2. D adds a DNS dependency WG has no analogue for today → 2.

**Operational cost — A 3 · B 4 · C 3 · D 3.** A: lowest *server* cost, highest
*user* burden (it moves cost, not removes it) → 3. B: lowest *user* burden,
moderate *operator* burden (one always-on node per household, NFR-5-feasible) → 4.
C: tunable but pays a coherence tax → 3. D: low *if you already run a domain*,
otherwise you must acquire/operate one → 3.

**Maturity — A 3 · B 5 · C 2 · D 3.** B: AT Proto runs at millions of users, the
pattern proven end-to-end (identity+migration+recovery, doc 01 §2.4) → 5. A:
transport pieces real (Iroh/Nostr) but the sigchain layer + recovery UX are new WG
code → 3. C: components proven individually, the *composition* is novel + the
largest unproven surface → 2. D: did:web is a W3C Rec, UCAN is emerging (v1.0
line), the *combination as a messaging substrate* is novel + UCAN-revocation-at-
scale is open → 3.

> **Scoring honesty.** The unweighted total (B 34 > C 28 > A 25 > D 21) is *one*
> input, not the verdict — it implicitly weights all axes equally, which a
> security gate must not. §7 re-weights for the gate's actual purpose and defends
> the result.

---

## 7. Defended ranking

This is a **security/reliability gate**, so the ranking weights the axes a
red-team weights: **Security, Recoverability, Maturity** (survivability and
proven-ness) above **Decentralization, Portability** (which the gate treats as
desirable but not safety-critical), with **WG-fit** as the tie-breaker the
downstream decision actually cares about. Under that lens — and confirmed by the
unweighted total pointing the same way at the top — the ranking is:

> ## 1. B   ·   2. C   ·   3. A   ·   4. D

### Rank 1 — **B (central-node-anchored)**

**Not the most decentralized — the most *survivable* and *proven*.** B leads the
three gate-critical axes (Recoverability 5, Maturity 5, Security 4) and the
tie-breaker (WG-fit 5). Its one severe finding (B-1, node compromise) is the **only
mass-compromise in the study that is recoverable** — the offline recovery key
overrides a hostile node within a bounded window (atproto-proven, doc 01 §4.2).
Every other candidate's worst case is either *unrecoverable* (A's lost-key death,
D's domain seizure) or *unproven* (C's composition). A security gate ranks the
*recoverable, proven, nearest* option first, even though it is the *least
decentralized* of the serious contenders. Its honest debits — worst metadata
(B-2), the node as a juicy target (B-1) — are disclosed and bounded, not fatal.

### Rank 2 — **C (hybrid)**

**The best *destination*, penalized for being the riskiest *build*.** C ties B/A's
ceilings on Security (4), Recoverability (5), and Portability (5), holds the right
invariant ("nothing correctness-critical is central"), and is the only candidate
that can adopt *each threat's best-defended mode* (UCAN custody for TC2/TC9,
self-certifying core for TC1). It ranks **below** B for two adversarial reasons:
**Simplicity 2 and Maturity 2** — the largest surface to misconfigure (C-1) and an
unproven composition (C-2). "Secure only if configured strictly" is a real
liability in an adversarial setting. Crucially, **B is *reachable inside* C** (B is
a configuration of C — doc 04 §4.9), so ranking C second is not a rejection: it is
"C is where you want to end up; you get there *through* B, not instead of it."

### Rank 3 — **A (fully P2P)**

**Cryptographically purest, operationally most fragile.** A wins Decentralization
outright (5) and has the strongest *integrity* story (nothing to subvert but the
math). A security/reliability gate must rank it third because it is **weakest on
exactly the axes the gate weights**: Recoverability 2 (A-4, fatal-for-the-careless),
anti-sybil weakest (A-2, FR-T2 deferred), and the **hydra-vs-recovery bind** (S-4 ×
A — the only finding where the *fix for one threat reintroduces another*). A is the
right *aspiration* for V5's decentralization lean but the wrong *v1* for a system
that must be reliable for the long term (V6). It survives as **the decentralization
*option* that C preserves**, not as the target.

### Rank 4 — **D (did:web + UCAN)**

**Last as a whole architecture; first as a *component*.** D is ranked fourth
because its identity anchor (DNS/CA/web-host) is the weakest root against the very
adversary the study exists to resist (A8 impersonation/seizure — D-1/D-2,
**Fatal-as-root**), and it *contradicts* V4's self-certification unless you fall
back to did:key (which dissolves it into C). **But this ranking is about D's
*anchor*, not D's *delegation layer*** — UCAN scores the best of all four on TC2
(smallest signer blast radius) and TC9 (attenuating+expiring sub-delegation kills
the hydra structurally). The defended recommendation is therefore **not "discard
D"**: it is **harvest D's UCAN as the custody/authority layer grafted into the
winner (B/C), and reject did:web as the identity root.**

### 7.1 The synthesis the ranking points doc 06 toward

The ranking is **not** "pick B, discard the rest." Read as a *phased*, defended
plan it says:

1. **Ship B's node as phase-2** (doc 04 §9) — the survivable, proven, nearest
   option a security gate trusts *today*; it reuses the existing daemon (WG-fit 5).
2. **Keep the self-certifying core as the correctness root** so you are never
   *only* trusting the node — this is what makes you **C**, and it is why B-as-a-
   config-of-C is the right framing (the node is a *convenience* over a key-rooted
   identity, never the root of trust). This neutralizes B-1's worst case at the
   protocol level: even a fully-compromised node cannot forge what a verifier
   self-checks.
3. **Graft D's UCAN** as the in-architecture delegation/custody layer — it is the
   best answer to S-4 (hydra) and TC2 (blast radius), and it is the only candidate
   component that makes the downloaded-identity split (§3) *structural* rather than
   disciplinary.
4. **Treat A's pure-P2P as the decentralization option C preserves**, mandating the
   recovery ceremony (paper-key + guardians) wherever a deployment runs node-less,
   and locking `add_key` to root to defuse the hydra (accepting the recovery-
   flexibility cost S-4 spells out).

This is doc 04 §6.3's hint reached **adversarially**: it survives the threat model
because each piece is chosen for the axis it is strongest on and bounded on the
axis it is weakest on. Doc 06 owns the final call against the doc-03 requirements;
this document's contribution is the defended claim that **B-reached-toward-C, with
D's UCAN grafted in and A as the preserved decentralization option, is the only
arrangement in which no Fatal finding remains unbounded.**

---

## 8. Handoff to doc 06 (decision)

- **The three things to budget engineering for regardless of the choice** (the
  unavoidable substrate findings): **S-3 revocation liveness**, **S-4 hydra
  (and its recovery-flexibility cost)**, **S-5 loadable-state poisoning** (the
  AI-specific one with no prior-art precedent).
- **The two Fatal findings to design around**: **A-4** (mandate the recovery
  ceremony / accept that agent recovery ⇒ a custodian) and **D-1/D-2** (never make
  DNS/CA the identity root; did:key/self-cert is the root, did:web at most an alias).
- **The custody-split verdict** (§3.2): the split holds *on paper* everywhere but is
  *structural* only in **D (UCAN) and B (node-held key)**; make the winner enforce
  it structurally (custodian holds the only key copy; download confers an expiring
  capability, never oracle access; same-self enrollment requires root/M-of-N).
- **The irony to price** (§3.3): V4 (non-impersonation) and V5 (decentralization)
  pull against each other *at the custody layer* — the more decentralized the
  design, the more "download ≠ impersonation" rests on user discipline rather than
  structure. The recommended B→C-with-UCAN path resolves this by keeping a
  custodian-of-record *under* a self-certifying root.
- **The defended ranking** (§7): **B > C > A > D** as whole architectures, with
  **D's UCAN first among components** — feeding directly into doc 06's "C as
  north-star wire, B's node as the pragmatic phase-2, D's UCAN as the in-C
  delegation mechanism, A as the preserved decentralization option."

---

## 9. Validation checklist (this document)

- [x] **Each candidate attacked across ≥8 threat classes.** All four (A §4.1, B
      §4.2, C §4.3, D §4.4) attacked across **all nine** TC1–TC9, one row each, plus
      divergent-finding prose.
- [x] **The downloaded-identity → impersonation attack covered specifically.**
      Dedicated cross-candidate deep-dive (§3, the attack tree D-paper…D-resolver),
      the "does the split hold?" verdict (§3.2: **D ≈ B > C > A**), the
      decentralization-vs-custody irony (§3.3), *and* a TC1 row per candidate (§4).
- [x] **Each scored on the full eight-axis rubric with justification.** §6 table +
      §6.1 per-axis prose (security, decentralization, recoverability, portability,
      simplicity, WG-fit, operational cost, maturity).
- [x] **Failure modes classified fatal/mitigable with mitigations + cost.**
      Consolidated register §5 (S-1…S-7 substrate + A/B/C/D divergent), each with
      disposition, mitigation, and cost; Fatal-finding summary §5.3.
- [x] **A defended overall ranking.** §7: **B > C > A > D** as architectures (D's
      UCAN first as a component), weighted for the security/reliability gate and
      defended axis-by-axis, with the phased synthesis (§7.1) for doc 06.
- [x] **`docs/federation-study/05-adversarial-evaluation.md` written.**

---

*Wave-1 evaluate phase complete. Four candidates adversarially threat-modeled
across nine threat classes by eight attacker capabilities, with the
downloaded-identity → impersonation crux dissected specifically; seven
substrate-level and fourteen candidate-divergent failure modes classified
fatal/mitigable/inherent with mitigations and costs; each candidate scored on the
full eight-axis rubric; and a defended ranking (B > C > A > D as whole
architectures, D's UCAN first as a component) produced for the doc-06 decision.*
