# Execution Federation Study 5/6 — Adversarial Evaluation, Threat Model & Ranking

> **Execution-federation study, wave 1, task 5 of 6 (the *evaluate* phase).**
> A red-team pass over the four candidate execution-plane architectures from doc 04
> (`04-candidate-architectures.md`): **A — trusted private pool**, **B —
> capability-gated cooperative/market**, **C — confidential compute (TEE)**, **D —
> hybrid synthesis**. We *attack* each — especially the two cruxes the brief and
> doc 03 §0 flag as load-bearing: **does the confidentiality approach actually hold
> against a provider that reads the agent's context, and does the integrity approach
> actually catch a provider that owns the worker env and forges results?** — then
> score, classify every failure mode fatal-vs-mitigable, and produce a defended
> ranking.
>
> The central tension the brief names is **confidentiality vs integrity vs
> openness**. This document's job is to find where each candidate breaks under that
> tension and to say, honestly, which arrangement survives the threat model with no
> *unbounded* fatal finding.

**Status:** draft for evaluation · **Date:** 2026-06-25 · **Owner task:** `exec-adversarial`
**Inputs:** `04-candidate-architectures.md` (the candidates) · `03-requirements-and-hard-questions.md`
(the 12 HQs + the rubric) · `02-current-state-baseline.md` (the seams attacked) ·
`01-prior-art-landscape.md` (the prior-art breaks) · the WG-Fed adversarial pass
(`docs/federation-study/05-adversarial-evaluation.md`, the sibling whose substrate
this study reuses). **Outputs to:** `exec-decision` (6/6).

---

## 0. How to read this document

The four candidates share a large **substrate** (doc 04 §1: one versioned wire, two
scoped UCANs, one context bundle, one cross-host lease, one leash policy engine).
Most attacks therefore land on the *substrate* and hit all four; a minority exploit
a candidate's *divergent* posture (A's trust, B's market, C's attestation, D's
selector). And **D is not a fifth mechanism — it is the leash engine selecting A's,
B's, or C's answer per task** (doc 04 §5), so D's *own* attack surface is the
selector, while everything else it inherits per tier. The document is built around
that fact:

- **§1 — Threat model.** Nine composable adversaries (P1–P9) and the **ten threat
  classes** (TC1–TC10) every candidate is attacked across — TC1 (malicious-provider
  reads context) and TC2 (result forgery) being the two the brief mandates
  *specifically*. Read first.
- **§2 — Substrate attacks (X-1…X-7).** The breaks that hit *all four* because they
  exploit the shared §1 design. Defined once, referenced by ID below so the
  per-candidate sections aren't 4× redundant.
- **§3 — The two headline cruxes, deep.** §3.1 the *malicious-provider-reads-context*
  crux (HQ1) across A/B/C/D with a verdict; §3.2 the *result-forgery* crux (HQ2)
  across A/B/C/D with a verdict. These are the load-bearing attacks; everything else
  is supporting fire.
- **§4 — Per-candidate adversarial pass.** Each of A/B/C/D attacked across **all ten**
  TCs (one row each) + its 2–4 *sharp divergent* findings in prose. This is the
  per-candidate ≥8-threat-class coverage.
- **§5 — Consolidated failure-mode register.** Every finding in one table:
  fatal/mitigable/inherent-bounded, the mitigation, its cost; plus the fatal-finding
  summary.
- **§6 — Scoring.** The brief's **eight-axis rubric** (confidentiality,
  result-integrity, decentralization, liveness, simplicity, WG-fit, operational
  cost, maturity), each candidate 1–5, justified — scored *adversarially*
  (worst-case-weighted), not for best-case elegance.
- **§7 — Defended ranking.** The security/reliability-gate verdict, the
  whole-architecture-vs-component nuance, and the phased synthesis it points doc 06
  toward.
- **§8 — Handoff to doc 06.** · **§9 — Validation checklist (this document).**

**Severity** = Critical / High / Medium / Low (worst-case impact × attacker reach).
**Disposition** = **Fatal** (breaks a MUST with no fix that preserves the
candidate's premise) · **Mitigable** (a named control reduces it to acceptable
residual at a stated cost) · **Inherent-bounded** (cannot be eliminated, only
disclosed and capped — the stance doc 03 takes for non-goals like side-channels and
metadata). We assume the **crypto primitives are sound** (ed25519 / X25519 /
XChaCha20-Poly1305 / BLAKE3, inherited from WG-Fed, doc 04 §1.4) — no candidate is
attacked by "break the cipher." Every attack below is on the **architecture around**
the primitives.

---

## 1. Threat model

### 1.1 Attacker capabilities

The headline adversary doc 02 §6 names is the **malicious provider host** — and it
is *categorically* stronger than the federation-study's network/relay adversary,
because in execution federation **the victim's agent literally executes on the
attacker's box**. The attacker is not on the wire; it is *under* the workload. Nine
composable adversaries:

| ID | Adversary | Can do |
|----|-----------|--------|
| **P1** | **Malicious provider / worker-host root** (the headline) | full control of the box where the *authorizer's agent* runs: read the worker's process memory, env vars, files, the worker's signer/UCAN, every socket it holds, the context bundle on disk, swap, core dumps; pause/inspect/mutate the running agent; forge its outputs. Cannot forge a signature it has no key for. |
| **P2** | **Network adversary** (Dolev–Yao on the wire) | observe, drop, delay, reorder, replay, inject bytes on any authorizer↔provider link, relay, or queue poll. Cannot forge a signature. |
| **P3** | **Malicious queue / directory / reputation-store operator** (B/D convenience node) | censor offers, equivocate (show different queues to different providers), bias ranking, withhold, log metadata, go offline. |
| **P4** | **Sybil provider farmer** | mint unlimited `wgid:` provider keypairs cheaply (keys are free); advertise fake capabilities; register many "independent" providers that are one entity. |
| **P5** | **Colluding provider cartel** | N legitimately-keyed providers cooperating: agree on a forged artifact to defeat quorum, pool a split confidential context to recombine it, cross-vouch to inflate reputation. |
| **P6** | **Behave-then-defect provider** | a provider that runs honest work to earn `Verified`/high reputation, then defects on a chosen high-value task. The classic reputation attack. |
| **P7** | **TEE / hardware-vendor adversary** (C-specific) | mount a side-channel (Foreshadow/SGAxe/ÆPIC-class) against an enclave; relay a genuine quote from a different enclave; present a quote for a genuine-but-backdoored runtime; or — at the limit (A8-class) — wield a leaked/compelled vendor attestation key. |
| **P8** | **Malicious authorizer** (the *inverse* threat — the provider's risk) | ships a poisoned task to escape the sandbox and harm the provider's host; refuses to pay for completed work; uses the provider to launder abuse. (FR-D5 demands isolation protect *both* directions.) |
| **P9** | **Partitioned-but-alive worker** (not malicious, but the split-brain hazard) | a worker that is merely network-partitioned, not dead, and returns *after* its lease was reclaimed and re-placed — the distributed-orphan double-committer. |

These compose: the worst real adversary is **P1 ∧ P4 ∧ P5 ∧ P6** — a patient
operator who mints a fleet of providers, earns reputation on cheap work, then
defects on a sensitive task across colluding sybils it controls. The threat classes
below are attacked with the *relevant* composition each time.

### 1.2 The ten threat classes (every candidate is attacked across all ten)

| ID | Threat class | The core question (and the brief item it covers) |
|----|--------------|---------------------------------------------------|
| **TC1** | **Malicious-provider context read / exfiltration** (THE crux) | Does the confidentiality approach actually hold? Can P1 read/exfiltrate the agent's working context + secrets off the box it runs on? (HQ1, FR-K*) |
| **TC2** | **Result forgery / corruption** (the co-crux) | P1 owns the worker env — does the integrity approach catch a plausible-but-forged result? (HQ2, FR-V*) |
| **TC3** | **Liveness denial — squat / stall / distributed orphan** | P1 takes work and produces nothing; P9 double-commits after reclaim. Does the lease+fencing hold? (HQ6, FR-L*) |
| **TC4** | **Capability theft / replay / over-scope / escalation** | Can P1 steal/replay the worker's UCANs, exceed task-scope, or drive the privileged-op callback as a confused deputy? (HQ5, FR-C*) |
| **TC5** | **Impersonation / sybil / collusion** | Spoof a provider; P4 mints a fleet; P5 colludes to defeat quorum/pool context. (HQ4, FR-R*) |
| **TC6** | **TEE side-channel / attestation forgery, relay, measurement-spoof** | C-specific: read enclave memory by side-channel; relay/spoof an attestation; pass a backdoored-but-attested runtime; vendor-root compromise. (HQ8, doc 04 §4) |
| **TC7** | **Data exfiltration in transit / at rest** | P2 on the wire; residue on the provider's disk/swap/core-dump/build-cache beyond the task. (HQ7, FR-D*) |
| **TC8** | **Result poisoning that propagates to downstream `--after` tasks** | A check-passing semantic poison in task T's artifact launders through honest downstream consumers. (graph integrity — the WG-specific one) |
| **TC9** | **Economic / billing abuse** | Metering inflation, budget-exhaustion DoS, callback free-riding, refusal-to-pay. (HQ9, FR-E*) |
| **TC10** | **Leash-engine misconfiguration / silent mis-routing** (D-specific meta-attack) | Can a config error or an attacker-influenced label silently route a confidential task to A/B instead of C? (HQ11, doc 04 §5.4) |

### 1.3 The central tension, restated as the gate

The brief's tension — **confidentiality vs integrity vs openness** — is not three
independent dials; the attacks below show they *trade against each other under the
adversary*:

- **Openness ↑ ⟹ confidentiality ↓:** the more you place on borrowed/untrusted
  compute (reach), the more a provider can read (TC1) — only a TEE (C) breaks the
  link, at a maturity/cost/centralization price.
- **Confidentiality (C) ↑ ⟹ integrity-evidence ↓:** a sealed enclave hides the very
  transcript a re-runner would inspect (doc 03 T9) — C resolves this by making the
  *attestation* the evidence, but that concentrates integrity risk in the same
  attestation chain that protects confidentiality (§3, TC6).
- **Integrity-by-verification (B) ↑ ⟹ openness-of-the-market ↓:** quorum's
  honest-majority and reputation's defect-penalty *both* assume non-sybil providers
  (TC5), so making B's integrity real means *closing* the open market toward a
  vouched cooperative — reach you thought you bought back.

A defensible ranking has to say which candidate navigates this trilemma with the
fewest *unbounded* breaks — not which scores highest on a naive sum (which, as §6
shows, rewards the candidate that *opts out of the threat model*).

---

## 2. Substrate attacks (X-1…X-7) — these hit all four candidates

Doc 04 §1 is a single shared design. Seven attacks exploit it directly and so apply
to **A, B, C, and D alike** (the §4 per-candidate sections reference these by ID and
add only what *diverges*). These are among the most important findings precisely
because no candidate choice escapes them.

### X-1 — Compat-handshake downgrade / floor-strip (TC1, TC7) · **Mitigable**

The `WG_EXEC_COMPAT_VERSION` handshake (doc 04 §1.2) negotiates encryption,
isolation class, and `alg`. P2 (active) tries to strip `must-encrypt`, force a weak
isolation class, or downgrade to a vN with a known weakness — the exact bare-
`openrouter:` silent-misroute class the doc cites as its cautionary tale, weaponized.

- **Mitigation:** doc 04 §1.2 already specifies the defence — the negotiated
  parameters are **signed, not merely exchanged** (the WG-Fed S-7 lesson), with a
  **minimum floor** (min isolation, min `alg`, must-encrypt) enforced rather than
  "lowest common," and **loud-fail** on incompatible mismatch. The residual is
  implementation-correctness: the floor must be checked *before* any context ships.
- **Cost:** maintain + enforce the floor policy; retiring a weak `alg`/version breaks
  old providers — loudly, which is correct.

### X-2 — Context-bundle over-inclusion: "minimal" isn't (TC1, TC7) · **Mitigable, residual real**

The bundle is a `ContextScope` slice (`context_scope.rs:17`, `Clean < Task < Graph <
Full`), and the slice size *is* the confidentiality knob (doc 04 §1.4). But the
slice-builder is the **confidentiality TCB**, and "minimal" is harder than it looks:

- `ContextScope = Task` ships task T's input *plus the artifacts it depends on* (its
  `--after` predecessors' outputs). A task whose own input is innocuous can
  transitively pull in a predecessor artifact that embeds a secret, an internal API
  surface, or a customer identifier. Minimization must reason about **transitive
  sensitivity**, which the dial does not do automatically.
- The **task descriptor itself** (the env-var promotion of `execution.rs:603–654`)
  carries the sensitive *intent* — "fix the auth-bypass in the payment flow" tells
  P1 exactly where your vulnerability lives even if no code ships. You cannot strip
  the task's own text and still have a runnable task.
- **Mitigation:** treat the slice-builder as security-critical — taint-track
  sensitivity through `--after` edges, default to the *smallest* tier and widen only
  on demonstrated need (FR-K3), and lint bundles for high-entropy/secret-shaped
  fields before sealing. For genuinely sensitive *intent*, the task must route to C
  (sealed) or A (trusted), never B.
- **Cost:** effectiveness tax (a too-small slice makes the agent worse — doc 03 T6);
  the taint analysis is non-trivial and never provably complete. **This is the
  honest floor: minimization reduces blast radius, it does not deliver
  confidentiality** (which is exactly why B refuses confidential work and C exists).

### X-3 — Privileged-op callback as a signing/inference oracle (TC4, TC1, TC9) · **Mitigable**

The "ask the authorizer to sign/do X" callback (doc 04 §1.3) keeps authority off the
provider — but P1 *owns the worker process that drives the callback*. If the callback
is "sign anything" / "run any inference," P1 turns it into:

- a **signing oracle** (confused deputy): drive the worker to request signatures over
  attacker-chosen digests → forge attributable artifacts without holding the key (the
  WG-Fed S-2 attack, transposed);
- a **free inference proxy** (TC9 free-riding): pump the authorizer-funded inference
  callback with the provider's *own* unrelated queries — the authorizer's API key
  used as a free LLM.
- **Mitigation:** the callback must be **intent-bound** (sign *this digest for this
  purpose* / infer *this prompt for task T with this model*), **rate-limited**,
  **metered against task T's budget ceiling** (R32, `parse_token_usage`), and
  **logged**. A prompt-relevance check on inference callbacks bounds free-riding. The
  authenticated requester is the *enrolled worker identity*, not a bearer token that
  travels in the bundle.
- **Cost:** the callback loses generality (it can no longer be a convenient
  "do-anything" RPC); per-deployment policy to maintain.

### X-4 — UCAN replay / lease-epoch fencing correctness (TC4, TC3) · **Mitigable, sound if enforced atomically**

Two replays to defeat: (a) P2 replays a still-valid `RunGrant` to start a *second*
unauthorized run; (b) P9 replays a stale `LeaseRenewal`/`ResultEnvelope` after
reclaim (the double-commit). Doc 04 §1.5's defence is the **monotonic lease epoch**:
reclaim increments it; a late worker's stale-epoch write is rejected at the boundary.

- **Mitigation (largely already specified):** envelopes carry nonces + expiry +
  epoch; the graph-write UCAN check rejects stale-epoch writes (`claim.rs:13`,
  `registry.rs`). The **load-bearing requirement the doc under-states: the epoch
  check must be enforced *atomically at the single canonical-graph write boundary*
  (the authorizer)** — doc 04 §1.4 keeps the canonical graph at the authorizer, so
  there *is* one such point; the fencing genuinely holds *provided that write-path
  compares-and-commits epoch in one step*. A TOCTOU there (check epoch, then commit,
  with a window) reopens the double-commit.
- **Cost:** the authorizer's graph write-path gains a mandatory epoch compare-and-set
  — a small, well-understood concurrency primitive. This is the *best-defended* area
  in the study (it inherits WG's mature lease lifecycle, doc 02 §2.5).

### X-5 — The ResultEnvelope is the verification TCB, and the provider authors all of it (TC2, TC8) · **Mitigable, with a sharp caveat**

The eval-gate and the re-runner judge the `ResultEnvelope`'s diff/artifacts/usage
(doc 04 §1.2, §1.7 `verify.rs`) — *all of which P1 authored*. The doc's load-bearing
integrity insight ("verify the **checkable artifact**, not the nondeterministic
transcript," doc 01 §4.2) is correct **only if two conditions hold that doc 04 states
loosely**:

1. **The check runs in a trusted domain.** A `cargo test` re-run that executes on the
   *same untrusted provider* is theatre. The re-run must run **authorizer-side, or on
   a disjoint independently-trusted provider** — doc 04 §3.5 implies this but never
   nails the *trust-domain of the re-runner*. **Stated requirement:** re-run is
   authorizer-local or cross-provider-disjoint, never same-provider.
2. **The check is against a pinned spec, not the provider's deliverable.** See X-6.
- **Mitigation:** pin the trust-domain of every verification lever; carry enough
  evidence (FR-V3) that the authorizer can re-derive the check independently.
- **Cost:** the authorizer must hold compute to re-run (it cannot fully offload), and
  cross-provider re-run is N× — the integrity-vs-cost tension (doc 03 T2) made real.

### X-6 — Test-poisoning: the deliverable redefines its own acceptance test (TC2, TC8) · **Mitigable** *(genuine gap in doc 04)*

The re-run lever assumes `cargo test` is an *independent* oracle. But a WG code task's
deliverable is a **diff that may modify the tests themselves**. P1 returns a diff that
edits both the implementation *and* the tests so `cargo test` passes — the re-run on
the authorizer's host *still passes*, because the poisoned tests shipped in the diff.
The "checkable artifact" was checked against an oracle the attacker rewrote.

- **Mitigation (not in doc 04 — flagged here):** the acceptance test is part of the
  **trusted spec, not the deliverable**. Re-run against the authorizer's *pinned*
  test suite; diff the test files separately and **flag any test-file change for
  human/authorizer review** before accepting; for tasks that legitimately add tests,
  the new tests are reviewed as spec, not auto-trusted.
- **Cost:** "tests are spec" is a real workflow constraint (tasks that write tests
  need a review gate); some friction on legitimate test authoring.

### X-7 — Reputation-store poisoning, gossip equivocation, slander (TC5) · **Mitigable** *(structurally bounded by design)*

The reputation store (FR-R3) is local-by-default, optionally gossiped. P4/P5
cross-vouch a sybil fleet to inflate reputation; P3 (gossip relay) equivocates; a
rival slanders an honest provider.

- **Mitigation — and why this is bounded:** doc 04 §1.1/§3.2's structural claim is the
  defence — **high-sensitivity or low-trust placement *always* applies the
  verification leash regardless of accrued reputation.** So reputation buys *cheaper
  verification on fungible work*, never *unverified trust on sensitive work*. Poisoned
  reputation therefore cannot promote a defector past the verification gate on the
  work that matters. Gossip is signed + per-authorizer-local (no mandatory central
  ledger, HQ10), so equivocation degrades a *hint*, not correctness.
- **Cost + residual:** the structural defence has a **hole §3.2 / §4.4 develops** — it
  protects *high-sensitivity* placement but a *normal*-sensitivity task on a
  reputation-`Verified`-but-defecting provider (P6) still gets attribution-only accept.
  Reputation poisoning's residual value is exactly that fungible middle. Fix = random
  spot-check re-runs even on trusted providers (cost: N× on a sample).

---

## 3. The two headline cruxes, deep

The brief mandates that **TC1 (malicious-provider-reads-context)** and **TC2
(result-forgery)** be attacked *specifically*. They are the load-bearing attacks, so
each gets a cross-candidate deep-dive and a verdict.

### 3.1 The confidentiality crux — does the provider-reads-context defence hold? (TC1, HQ1)

The question: **P1 owns the box. Can it read the agent's working context + secrets?**

- **A (trust):** **No defence — by design.** The provider sees the plaintext context,
  the graph slice, tool outputs, and any shipped secret for the task's duration (doc
  04 §2.4). Attribution and the task-scoped write UCAN bound *integrity blast radius*
  but do **nothing** for confidentiality: a curious/compromised pool member
  exfiltrates everything it sees *regardless of write scope*. The only things A
  actually defends are (i) the **transit** path (sealed envelopes, X25519 — a MITM
  sees ciphertext) and (ii) **secret-as-argv leakage** (env-not-argv,
  `execution.rs:644`). Against P1 itself, A's confidentiality is **zero, honestly
  stated.** The one real attack *within A's scope* is **secret residue beyond the
  task** (TC7): a shipped API key survives in swap, a core dump, the container layer
  cache, or a host compromised *later* — so even "trusted, ephemeral" leaks. Verdict:
  **A offers no confidentiality vs the provider and correctly refuses confidential
  work (FR-K5); attacking A on TC1 is attacking its *scope boundary*, which holds
  only as long as the trust assumption does and the leash engine enforces the
  refuse-row (→ TC10).**

- **B (minimize):** **Reduces blast radius; does not prevent reading.** Doc 04 §3.4 is
  refreshingly honest: "the provider CAN read the slice it is given." The sharp
  attacks are on *what minimization can't shrink*:
  - **Transitive over-inclusion (X-2):** the minimal slice still drags in
    `--after`-predecessor artifacts that may carry secrets.
  - **Intent leakage:** the task description itself reveals the sensitive *what/where*
    (X-2) — unshrinkmable without breaking the task.
  - **Content vs credential:** B keeps the *API key* off the provider (callback /
    provider-funded inference, doc 04 §3.8), but the provider runs the agent loop, so
    **every prompt and every completion token flows through P1's process.** B's
    "no secrets on the provider" protects the *credential*, not the *content*. An
    operator that logs the worker's stdio reconstructs the entire reasoning trace.
  - Verdict: **B's confidentiality = bounded minimization, scoped to non-confidential
    work, with a loud refuse for confidential (FR-K5).** The dangerous failure is not
    a B *bug* — it's a *deployment* that mistakes "minimized" for "confidential" and
    routes mildly-sensitive work to B. Mitigable by the refuse-row + a sensitivity
    classifier; the residual ("you cannot run confidential work on B, period") is
    *inherent*, not fixable.

- **C (attest):** **The only candidate that defends TC1 against P1 — but not
  unconditionally.** The operator runs the agent yet provably cannot read enclave
  memory (SEV-SNP/TDX/SGX/Nitro, doc 04 §4.4). This genuinely converts "trust the
  operator" into "break the hardware or the measurement-curation." It is a large,
  real improvement. But it is **strong, not unconditional**, and the residuals are
  exactly P7's repertoire (developed fully in §4.3 / TC6):
  - **Side-channels** (Foreshadow/SGAxe/ÆPIC, cache-timing, SEV ciphertext side
    channels): the TEE encrypts memory but leaks via timing/access-patterns. *Inherent-
    bounded* — mitigable (constant-time runtimes, microcode patches) but never zero;
    a patient operator extracts *some* information.
  - **Measurement substitution (the deepest one):** attestation proves "*a* runtime
    with measurement M ran in a genuine enclave" — **not that M is *safe*.** If the
    authorizer's expected-measurement allow-list is permissive (a version range, a
    vendor-default image, a runtime with a debug hook or a logging shim), a
    *genuine-but-backdoored* runtime attests successfully and reads the context from
    the inside. **C's confidentiality TCB is the measurement allow-list, and curating
    it is an ongoing, security-critical, operationally heavy job** — the real weak
    link, ahead of the silicon.
  - **Vendor root** (Intel/AMD/AWS): a leaked or compelled vendor attestation key (A8)
    forges any quote. *Inherent-bounded* — non-WG, non-decentralized; diversify
    vendors, monitor key revocation, but you cannot remove it.
  - Verdict: **C clears the crux — uniquely — but trades operator-trust for
    vendor-trust + measurement-curation-trust + an irreducible side-channel residual.
    It is the right *escape hatch* for confidential work, not a cost-free guarantee.**

- **D (route):** D's confidentiality = the tier the selector picks, so its *ceiling
  equals C* (it routes confidential work to C-or-refuse). The **new** attack is the
  selector (TC10): silently route confidential work to A/B. The sharp finding —
  developed in §4.4 — is that **doc 04 §5.4's fail-safe default ("unset sensitivity ⇒
  needs-trust ⇒ A or refuse") is ambiguous and, if it resolves to A, *unsafe for
  confidentiality*:** unlabeled-but-actually-confidential work would land plaintext on
  a pool member. The truly fail-*closed* default for an unlabeled task is **refuse or
  C, never A**. With that fix, D's confidentiality ceiling = C; without it, D is a
  silent-exposure machine. Verdict: **D = C's confidentiality, conditional on a
  fail-closed selector.**

**Cross-candidate verdict (TC1): C > D ≈ C-ceiling > B > A.** Only C (and D-routing-
to-C) defends context against the provider at all; B reduces blast radius without
preventing reading; A defends nothing vs the provider and says so. The crux is *not*
solved cheaply by anyone — and the honest, load-bearing conclusion is that
**confidentiality on genuinely-untrusted compute costs a TEE, with all of the TEE's
residual risk; everything else is "trust" or "ship less."**

### 3.2 The integrity crux — does forgery get caught when the provider owns the worker? (TC2, HQ2)

The question: **P1 can make the agent emit anything, or fabricate a `ResultEnvelope`
wholesale. What catches it?**

- **A (attribution + eval-gate):** **The weakest, because the provider holds the
  signer.** The `ResultEnvelope` is signed by the *delegated act-as-agent signer* —
  which lives in the worker's address space on P1's box. So "signed by agent G" proves
  *the delegated key signed it*, which a malicious operator can do for **any** content.
  **Attribution provides zero integrity against a provider that holds the signer.**
  All that remains is the **eval-gate**, which scores *quality against `## Validation`*
  — a *quality filter, not an integrity proof*. A forged diff that compiles and passes
  the listed tests but plants a backdoor, or a plausible-but-wrong research summary,
  sails through. The task-scoped write UCAN caps *graph damage* but not *acceptance of
  this task's forged result*. Verdict: **against a compromised pool member, A's
  integrity = the eval-gate alone. Acceptable *iff* the trust assumption holds;
  catastrophic the moment it doesn't — and A has no in-band way to know it doesn't.**

- **B (+ re-run / quorum on checkable artifacts):** **Genuinely strong in its sweet
  spot, with three sharp holes:**
  - **Hole 1 — non-checkable deliverables.** "Verify artifacts not transcript" needs an
    *independently checkable* artifact. A design doc, a research summary, a refactor
    with no test — these have **no deterministic oracle**, so B's strongest lever
    *doesn't apply* and B falls back to the eval-gate (= A's weak integrity) on exactly
    the tasks where re-run can't help. **B is strong for test-backed code and weak for
    everything else.**
  - **Hole 2 — test-poisoning (X-6):** the diff rewrites its own acceptance test;
    re-run passes against the poisoned oracle. Needs "tests are spec," which doc 04
    omits.
  - **Hole 3 — quorum under collusion (P4 ∧ P5):** N-of-M quorum assumes honest
    majority. A sybil cartel registers M cheap `wgid:` providers, all controlled by one
    operator, all return the *same* forged artifact → quorum agrees on the lie (doc 01
    §2.4 BOINC's honest-majority assumption, broken by free keys). Doc 04 §11 flags
    this for us; the defence is **provider diversity (distinct operators/networks/
    hardware) + sybil-resistance on enrollment + reputation weighting** — but
    permissionless sybil-resistance is *the* unsolved problem (→ TC5), so quorum is
    only as strong as enrollment cost. **Gating quorum to vouched/attested providers
    fixes it — by shrinking the market back toward a cooperative** (the openness cost,
    §1.3).
  - And the substrate caveat **X-5**: the re-run must run in a *trusted domain* against
    a *pinned spec*, or it's theatre.
  - Verdict: **B's integrity is strong for deterministically-checkable, test-backed,
    trusted-domain-re-run-against-a-pinned-spec work performed by non-colluding
    providers — and weak outside every one of those qualifiers.** Its honest scope:
    fungible, checkable, code-like work on a *vouched* (not open) pool.

- **C (+ attestation of the process):** **Strongest fit for nondeterministic agents —
  but risk is concentrated, and the I/O boundary leaks.** The quote proves "the
  expected harness ran the pinned model in a genuine enclave and produced this output"
  (doc 04 §4.5) — sidestepping the determinism wall that breaks re-run/quorum/zkVM for
  agent transcripts (doc 01 §4.2). Two sharp attacks:
  - **The enclave's I/O boundary is the soft underbelly.** Attestation secures the
    *compute*, not the *network the enclave talks to*. If P1 can influence the
    enclave's *inputs* — MITM a tool fetch the agent makes to the operator-controlled
    network, inject a prompt-injection into an operator-supplied tool result — the
    attested-honest harness *faithfully* produces a manipulated output. Garbage-in,
    attested-garbage-out. **Mitigation:** route *all* enclave I/O through sealed
    channels (the bundle + the inference callback) and treat any operator-supplied tool
    result as untrusted input to be validated — but agents *need* to fetch things, and
    a poisoned egress corrupts reasoning under a valid attestation.
  - **A broken attestation forges integrity *and* confidentiality at once.** Every TC6
    attestation weakness (relay, measurement-substitution, vendor-root) doesn't just
    leak context — it lets P1 emit a forged result *carrying a valid-looking
    attestation*, which the authorizer trusts *more* than a B result. **C concentrates
    both cruxes in one chain;** when it breaks, it breaks worse.
  - Verdict: **C's integrity is the strongest *if the attestation holds*, but it
    concentrates catastrophic risk in the vendor-rooted attestation/measurement chain
    and leaves the enclave-I/O boundary as a live poison vector.**

- **D (route + the structural verification claim):** integrity per tier; the new attack
  is **P6 behave-then-defect against the verification leash**. Doc 04 §1.1 claims a
  structural defence: "high-sensitivity or low-trust placement *always* applies the
  verification leash regardless of accrued reputation." **Test it:** the claim holds
  for *high-sensitivity* tasks (always verified) — but a **normal-sensitivity task on a
  reputation-`Verified` provider gets A's attribution-only accept**, and a patient P6
  earns `Verified` on cheap work precisely to reach that path. **The structural defence
  protects the sensitive minority and leaves the fungible *majority* — the volume —
  under-verified.** Mitigation: **random spot-check re-runs even on trusted providers**
  (defence-in-depth), at N×-on-a-sample cost. Verdict: **D's integrity ceiling = C, but
  its default exposure is A-weak on the volume middle unless spot-checks are wired —
  the structural claim is narrower than doc 04 implies.**

**Cross-candidate verdict (TC2): C ≥ B(in-scope) > D(default) > A.** C is strongest
where its attestation holds (and weakest-but-loud where it doesn't); B is strong for
checkable code on a vouched pool and weak elsewhere; D matches the best per tier but
under-verifies the fungible middle by default; A reduces to the eval-gate against any
compromised member. **The deepest, most under-defended integrity threat is not any of
these — it is TC8 (cross-task poison propagation), because every candidate's integrity
story is *per-task* and the threat is *cross-task*** (§4 develops it; it is the one
genuinely new fatal-class the study under-treats).

---

## 4. Per-candidate adversarial pass

Each candidate is attacked across all ten threat classes (one row each — this is the
≥8-threat-class coverage), then its sharpest divergent findings in prose. Rows cite
the substrate findings (X-n) and crux verdicts (§3) rather than repeating them.

### 4.1 Candidate A — Trusted private pool

| TC | Attack & outcome on A | Sev | Disposition |
|----|----------------------|-----|-------------|
| **TC1** read-context | **No defence vs P1 (by design)** — provider sees all plaintext; only transit sealed + secret-residue (X-2/TC7) remain in-scope (§3.1). | Crit | **Fatal-if-misused** / by-design-in-scope |
| **TC2** forgery | Provider holds the signer ⇒ attribution worthless; eval-gate (quality) is the only check (§3.2). | Crit | **Fatal-if-trust-violated** |
| **TC3** liveness | Enrolled pool, partition unlikely; §1.5 lease + fencing (X-4) hold; squatting a non-issue (no strangers). | Low | Mitigable (already strong) |
| **TC4** capability | Worker may hold a **standing signer** (A's convenience) ⇒ larger theft blast radius than B/C; bounded by task-scoped write UCAN + revoke list. | Med | Mitigable |
| **TC5** sybil/collude | Out of scope — you enrolled every provider by hand (`wg provider add`); sybil/collusion presuppose strangers (that is B). | Low | N/A in scope |
| **TC6** TEE/attest | N/A — A has no TEE (its confidentiality is trust, not attestation). | — | N/A |
| **TC7** transit/at-rest | Transit sealed; **at-rest = plaintext on the trusted box + residue (swap/core/cache) beyond the task** — the one real in-scope leak. | Med | Mitigable |
| **TC8** downstream poison | A compromised member's check-passing poison in T propagates to honest `--after` consumers (§4-common). | High | Mitigable, residual real |
| **TC9** economics | "You own it, you pay"; metering believed (trusted). Budget ceilings via `parse_token_usage` bound a runaway. | Low | Mitigable |
| **TC10** mis-route | A *is* the slack default; the risk is the **leash engine routing a confidential task *into* A** (the refuse-row must fire) — really a D/TC10 finding. | High | Mitigable (→ TC10) |

**Divergent findings.**
- **A-i (Critical) — A is not an answer to the malicious-provider adversary; it
  *assumes the adversary away*.** This is not a bug — it is A's defining property. A's
  confidentiality (TC1) and integrity (TC2) are both "the provider is honest." A is
  *correct exactly when that holds* and **loudly refuses** what it can't do (FR-K5). The
  red-team consequence: A's strong scores on every *other* axis (§6) are *cheap because
  it does not play the game* — they must not be read as "A is the safest." A is the
  safest **for trusted compute**, which is a real and common case, and the wrong tool
  the instant the box is not trusted.
- **A-ii (Medium) — the standing signer widens TC4.** A's convenience (a long-lived
  delegated signer instead of a per-op callback, doc 04 §2.3) means a compromised pool
  member can sign as agent G for *hours*, not minutes. Mitigation: short-ish re-issue +
  a revoke list checked at write time; or drop to the callback model (which is B). Cost:
  losing A's main convenience.
- **A-iii (High) — "trusted" is a *point-in-time* claim.** A box you trust today can be
  rooted tomorrow; the shipped secret/context residue (TC7) then leaks retroactively.
  Mitigation: ephemeral secrets, env-not-argv (`:644`), encrypted swap, no core dumps,
  short secret TTLs — and prefer the callback over shipping a standing key even on A.

### 4.2 Candidate B — Capability-gated cooperative / market

| TC | Attack & outcome on B | Sev | Disposition |
|----|----------------------|-----|-------------|
| **TC1** read-context | Minimization only — provider reads the slice; transitive over-inclusion (X-2) + content-vs-credential (§3.1) remain; **refuses confidential**. | High | Mitigable / inherent-scoped |
| **TC2** forgery | Strong for checkable code (re-run/quorum, X-5) — **weak for non-checkable, test-poisonable (X-6), collusion-breakable (TC5)** (§3.2). | High | Mitigable, scoped |
| **TC3** liveness | Tight lease + fencing load-bearing (partitioned market workers *do* return — X-4 holds); **sybil-squatting dodges the reputation penalty** (fresh `wgid:` each time). | Med | Mitigable (needs enrollment cost) |
| **TC4** capability | No standing signer; short-TTL task-scoped UCANs + callback ⇒ leaked-credential blast radius = one task, minutes (the *best* TC4 posture). Callback-oracle risk (X-3). | Low | Mitigable (well) |
| **TC5** sybil/collude | **The chief unsolved problem.** P4 mints a fleet; P5 colludes to defeat quorum + cross-vouch reputation (X-7). Permissionless sybil-resistance unsolved. | Crit | **Fatal for the *open* market** / mitigable for a vouched cooperative |
| **TC6** TEE/attest | N/A (B has no TEE) — which is *why* B must refuse confidential work; its isolation is **self-advertised, unverifiable** (doc 04 §3.7). | — | N/A (the gap that motivates C) |
| **TC7** transit/at-rest | Transit sealed + minimal; **at-rest disposal NOT trusted** (doc 04 §3.7 — "ship little, assume it persists"); honest. | Med | Mitigable (by shipping little) |
| **TC8** downstream poison | **Worst here:** an open-market forgery (TC2/TC5) that passes per-task checks launders into honest downstream consumers; per-task verification is blind to it. | Crit | Mitigable, **most under-defended** |
| **TC9** economics | Metering inflation (padded bill) — reconciliation works for *authorizer-funded* (real API count), **circular for provider-funded**; budget-exhaustion DoS; callback free-riding (X-3). | Med | Mitigable |
| **TC10** mis-route | Routing mildly-sensitive work to B believing "minimized = confidential" — a deployment error the refuse-row must catch (→ TC10). | High | Mitigable |

**Divergent findings.**
- **B-i (Critical) — sybil/collusion is the load-bearing unsolved problem, and it
  breaks *three* defences at once.** Free keys (P4) + collusion (P5) defeat (a) quorum's
  honest-majority (TC2), (b) the squat-penalty (TC3 — a fresh identity each time), and
  (c) reputation (TC5/X-7). All three of B's market-side defences *assume non-sybil
  providers*. The only real fixes raise enrollment cost — **vouching (web-of-trust,
  cooperative), stake/deposit, or hardware-attestation-as-identity** — and every one of
  them **moves B from an *open* market toward a *closed* cooperative**, surrendering the
  reach that was B's point (the openness↔integrity trade, §1.3). *Elegant corollary:*
  **C's attestation doubles as sybil-resistance** (a distinct TEE proves distinct
  hardware, far costlier to mint than a keypair) — a reason the C tier strengthens the B
  tier inside D.
- **B-ii (High) — integrity is conditional on the artifact being checkable, and most
  agent work isn't fully checkable.** B's strongest lever evaporates on design/research/
  refactor tasks. The mitigation is to *route non-checkable work to a higher-trust tier*
  (A/C) rather than B — i.e., B is honestly a tier for *checkable code on a vouched
  pool*, not a general execution market.
- **B-iii (Medium) — provider-funded metering is unverifiable.** Reconciliation
  (FR-E3) compares the provider's signed usage against the authorizer's own
  `parse_token_usage` — but if the *provider* ran inference, the authorizer has no
  independent count to compare against. Mitigation: **prefer authorizer-funded inference**
  (doc 04 §3.8's v1 choice — correct) and cap+sample-audit provider-funded; never trust
  a provider-funded bill at face value.

### 4.3 Candidate C — Confidential compute (TEE)

| TC | Attack & outcome on C | Sev | Disposition |
|----|----------------------|-----|-------------|
| **TC1** read-context | **The only real defence vs P1** — context plaintext only inside the enclave; residuals = side-channel + measurement-curation + vendor-root (§3.1, TC6). | Med | Mitigable / inherent-bounded |
| **TC2** forgery | Strongest fit (attestation-of-process) **if attestation holds**; a broken quote forges integrity *and* confidentiality together; enclave-I/O poison (§3.2). | Med–High | Mitigable / concentrated |
| **TC3** liveness | Attestation-bound renewals prove *the genuine enclave* is alive (stronger than B); enclave ephemerality aids reclaim; TEE infra adds *operational* failure modes. | Low–Med | Mitigable |
| **TC4** capability | UCANs + context key **sealed to attestation** (Nitro→KMS) ⇒ operator-outside-TEE cannot extract them even with host root — the *best* TC4 confidentiality. | Low | Mitigable (well) |
| **TC5** sybil/collude | Attestation **is** sybil-resistance (distinct hardware); collusion across enclaves can still pool *outputs* but not *enclave memory*. | Low | Mitigable (well) |
| **TC6** TEE/attest | **C's whole risk surface:** side-channel (inherent), relay (mitig. nonce+wgid bind), **measurement-substitution (chief residual)**, vendor-root (inherent), key-release TOCTOU. | High | Mixed: mitigable + inherent-bounded |
| **TC7** transit/at-rest | Sealed-to-enclave; at-rest encrypted-to-enclave; **disposal by hardware teardown** (no persistent storage) — the *only* candidate not relying on provider goodwill to delete. | Low | Mitigable (best) |
| **TC8** downstream poison | An *attested* poison (passed garbage-in via enclave I/O, §3.2) propagates carrying a *valid attestation* ⇒ trusted *more* downstream — attestation can amplify TC8. | High | Mitigable, residual real |
| **TC9** economics | TEE premium; metering can be **attested** (strongest reconciliation); settlement still a non-goal. | Low | Mitigable |
| **TC10** mis-route | C is the escape-hatch tier; the risk is the selector accepting a **self-advertised** (un-attested) isolation claim as if attested (→ TC10). | High | Mitigable (require attested, not advertised) |

**Divergent findings.**
- **C-i (High) — the confidentiality+integrity TCB is the *measurement allow-list*, not
  the silicon.** Attestation proves "*a* runtime with measurement M ran," never "M is
  safe." A permissive allow-list (version ranges, vendor-default images, a debug-hooked
  or logging-shimmed runtime) admits a *genuine-but-backdoored* enclave that reads
  context and forges results *with a valid quote*. **Curating a pinned, audited
  measurement allow-list is an ongoing, security-critical operational job** — the real
  weak link. Mitigation: pinned + audited measurements, reproducible runtime builds,
  nonce+wgid binding inside the quoted user-data (anti-relay), vendor diversity. Cost:
  heavy, continuous curation + the largest verification operational surface.
- **C-ii (High) — a broken attestation is *worse* than no attestation.** Because a
  forged-but-attested result carries more weight than a B result, the failure mode is
  not "C degrades to B" — it's "C confidently certifies a lie." The mitigation is to
  treat attestation as *defence-in-depth alongside* the eval-gate and (where the
  artifact is checkable) a cross-domain re-run, never as a sole oracle. Cost: you don't
  get to skip the other levers just because you have a quote.
- **C-iii (Inherent-bounded) — a non-WG, non-decentralized vendor trust root.** Intel/
  AMD/AWS sign the attestations; a leaked/compelled vendor key (A8/P7) forges any quote,
  and the root is centralized and outside WG's control (doc 04 §4.11). Cannot be removed
  from a TEE — only disclosed, diversified across vendors, and monitored for revocation.
  This is the price of confidential compute, and it is why C is *least* decentralized.
- **C-iv (Inherent-bounded) — side-channels are an active, losing-looking arms race.**
  The TEE security record (Foreshadow → SGAxe → ÆPIC → SEV ciphertext channels) is a
  parade of breaks-then-patches. C's confidentiality is *probabilistic*, not absolute;
  for the highest-sensitivity work the residual access-pattern/timing leakage must be
  disclosed and accepted, not assumed away.

### 4.4 Candidate D — Hybrid (the leash engine as selector)

| TC | Attack & outcome on D | Sev | Disposition |
|----|----------------------|-----|-------------|
| **TC1** read-context | Ceiling = C (routes confidential → C-or-refuse) **iff the selector is fail-closed**; the §3.1 unlabeled→A hole exposes context. | High | Mitigable (fail-closed default) |
| **TC2** forgery | Ceiling = C per tier; **default A-weak on the normal-sensitivity volume** (P6 behave-then-defect, §3.2). | High | Mitigable (spot-checks) |
| **TC3** liveness | One §1.5 lease across tiers, fencing always on; sybil-squat exposure only when the B tier is enabled (= B's TC3). | Med | Mitigable |
| **TC4** capability | scope/TTL by `leash()` per tier — broad/long (A) → short/narrow (B/C); inherits each tier's TC4. | Low–Med | Mitigable |
| **TC5** sybil/collude | Inherits B's TC5 when the market tier is on; A/C tiers are sybil-resistant. The selector should gate quorum to vouched/attested providers. | High | Mitigable |
| **TC6** TEE/attest | Inherits C's TC6 for the confidential tier; **plus accepting a self-advertised isolation class as attested** is a selector bug (→ TC10). | High | Mitigable |
| **TC7** transit/at-rest | Per tier (A plaintext / B minimal-no-trust-delete / C sealed-HW-dispose). | Low–Med | Mitigable |
| **TC8** downstream poison | Inherits per tier — **but D *enables* the real defence:** route foundational/root tasks to high-trust tiers, leaf tasks to market (new, below). | High | Mitigable (D-enabled) |
| **TC9** economics | Per tier; one budget/ceiling substrate (R32) across tiers. | Low | Mitigable |
| **TC10** mis-route | **D's signature risk** — the selector/leash policy is the largest, newest, security-critical config surface; a misconfig silently mis-routes (below). | High | **Mitigable, chief D risk** |

**Divergent findings.**
- **D-i (High) — the fail-safe default is fail-safe for *integrity* but not for
  *confidentiality*.** Doc 04 §5.4's "unset sensitivity ⇒ needs-trust ⇒ **A** or refuse"
  is safe for integrity (A re-runs nothing but is attribution-signed and eval-gated) yet
  *unsafe* for confidentiality (A exposes context to a pool member). **An unlabeled-but-
  actually-confidential task routed to A is silently exposed — exactly the failure FR-K5
  exists to prevent.** *Fix:* the fail-closed default for an unlabeled task is **refuse,
  or route to C** — A only for *explicitly-normal* sensitivity. This single change is the
  difference between D being the safest candidate and being a silent-exposure machine.
- **D-ii (High) — the selector is itself an attack surface (TC10), and the
  sensitivity label may be attacker-controlled.** If the *task author* is the adversary
  (an insider, or an agent that creates subtasks), they label an exfiltration task
  "normal" and it routes to the cheap tier. Mitigation: sensitivity is **inferred +
  labelled, not solely self-asserted** (taint-track from the bundle, X-2); strict mode;
  `wg config lint` over the leash policy (the lint already exists, CLAUDE.md); the
  applied leash **surfaced** in `wg show`/`wg providers` so a too-loose route is visible.
  Cost: the largest test/config matrix of the four (mirrors the WG-Fed C-1 finding) +
  the discipline to keep defaults fail-closed.
- **D-iii (High, and the study's most valuable *new* recommendation) — D is the only
  candidate that can defend TC8 (cross-task poison propagation) structurally.** Every
  candidate's integrity is *per-task*; the poison threat is *cross-task* (a check-passing
  forgery in T launders through honest `--after` consumers). The defence is a
  **placement policy keyed to graph position**: route *foundational/root* tasks — whose
  poison would propagate widest — to **high-trust (A) or attested (C)** tiers, and
  reserve the **market (B)** tier for *leaf* tasks whose output nothing depends on. Add
  **provenance tracking** (the `ResultEnvelope` records which provider produced each
  upstream artifact, so a later-discovered bad provider lets you find and re-run all
  poisoned descendants — NFR-7 auditability) and a **re-verify-inputs-across-trust-
  boundaries** rule (a downstream task on a *higher* trust tier re-checks inputs from a
  *lower* tier rather than assuming them). Only D — with a selector that sees the whole
  graph — can implement this. Cost: the selector must reason about graph topology, not
  just per-task sensitivity; N×-on-the-critical-path re-verification for high-stakes
  graphs.

---

## 5. Consolidated failure-mode register

Every finding above, classified, with mitigation and cost. **Fatal** = breaks a MUST
with no fix preserving the candidate's premise. **Mitigable** = named control,
acceptable residual. **Inherent-bounded** = disclosed + capped, not eliminable.

### 5.1 Substrate (all four candidates)

| ID | Threat (TC) | Disposition | Mitigation | Cost of mitigation |
|----|-------------|-------------|------------|--------------------|
| **X-1** | Handshake downgrade / floor-strip (TC1/7) | Mitigable | Sign negotiated params; min isolation/`alg`/must-encrypt floor; loud-fail; check floor before any context ships | Maintain+enforce floor; alg/version retirement breaks old providers (loudly) |
| **X-2** | Bundle over-inclusion; "minimal"≠confidential (TC1/7) | Mitigable, residual real | Taint-track sensitivity through `--after`; default smallest tier, widen on need; lint bundles; sensitive *intent* ⇒ C/A not B | Effectiveness tax (T6); taint analysis never provably complete |
| **X-3** | Callback as signing/inference oracle (TC4/1/9) | Mitigable | Intent-bound, rate-limited, budget-metered, logged callback; authenticate enrolled worker identity not bearer token; prompt-relevance check | Callback loses generality; per-deployment policy |
| **X-4** | UCAN replay / fencing TOCTOU (TC4/3) | Mitigable (sound if enforced) | Nonce+expiry+epoch envelopes; **atomic compare-and-set of epoch at the single canonical-graph write boundary** | A mandatory CAS on the graph write-path (small, well-understood) |
| **X-5** | Provider authors the verification evidence (TC2/8) | Mitigable, sharp caveat | Re-run **authorizer-side or cross-provider-disjoint**, never same-provider; carry enough evidence to re-derive the check | Authorizer holds re-run compute (can't fully offload); cross-provider re-run is N× |
| **X-6** | Test-poisoning — deliverable rewrites its oracle (TC2/8) | Mitigable *(doc-04 gap)* | "Tests are spec": re-run vs the authorizer's *pinned* suite; flag test-file changes for review | Friction on legitimate test-authoring tasks |
| **X-7** | Reputation poisoning / gossip equivocation (TC5) | Mitigable *(structurally bounded)* | Verification leash applies regardless of reputation; signed, per-authorizer-local gossip; spot-checks on the fungible middle | Spot-check N×-on-a-sample; gossip is a hint, not correctness |

### 5.2 Candidate-divergent

| ID | Threat (TC) | Cand. | Disposition | Mitigation | Cost |
|----|-------------|:----:|-------------|------------|------|
| **A-i** | No defence vs malicious provider — assumes it away (TC1/2) | A | **Fatal-if-misused** / by-design-in-scope | Keep A's scope honest (never enroll untrusted); leash refuse-row enforced; spot-checks | A is only correct for *trusted* compute; the moment it isn't, it's the wrong tier |
| **A-ii** | Standing signer widens theft window (TC4) | A | Mitigable | Short re-issue + write-time revoke list; or drop to callback (=B) | Loses A's main convenience |
| **A-iii** | "Trusted" is point-in-time; secret residue leaks retroactively (TC7) | A | Mitigable | Ephemeral secrets, env-not-argv, encrypted swap, no core dumps, callback-not-ship | Operational hardening on every pool host |
| **B-i** | Sybil/collusion breaks quorum + squat-penalty + reputation (TC2/3/5) | B | **Fatal for the *open* market** / mitigable for a cooperative | Vouch/stake/attest enrollment; diversity-required quorum; **C's attestation as sybil-resistance** | Closes the market toward a cooperative — surrenders open reach |
| **B-ii** | Integrity conditional on checkable artifacts (TC2) | B | Mitigable (scope) | Route non-checkable work to A/C; B = checkable-code-on-vouched-pool tier | B is not a general market — a narrower scope than "any work" |
| **B-iii** | Provider-funded metering unverifiable (TC9) | B | Mitigable | Prefer authorizer-funded; cap+sample-audit provider-funded; never trust a provider bill at face | Confidentiality-cleanest funding model constrains who-pays choices |
| **C-i** | Measurement allow-list is the TCB, not the silicon (TC1/2/6) | C | **High, Mitigable** (chief residual) | Pinned+audited measurements; reproducible runtime builds; nonce+wgid in quoted user-data | Continuous, security-critical curation; largest verification op-surface |
| **C-ii** | Broken attestation forges integrity *and* confidentiality (TC2/6) | C | Mitigable | Attestation as defence-in-depth alongside eval-gate + cross-domain re-run, never sole oracle | Can't skip the other levers despite a quote |
| **C-iii** | Vendor trust root — non-WG, centralized, compellable (TC6) | C | **Inherent-bounded** | Disclose; diversify vendors; monitor key revocation | Cannot remove; makes C the least decentralized |
| **C-iv** | Side-channels — active arms race (TC1/6) | C | **Inherent-bounded** | Constant-time runtimes; microcode patches; disclose residual access-pattern/timing leak | C's confidentiality is probabilistic, not absolute |
| **D-i** | Fail-safe default exposes unlabeled confidential work (TC1/10) | D | **High, Mitigable** | **Unlabeled ⇒ refuse or C, never A**; A only for explicitly-normal | A behind a stricter gate; some "just run it" friction |
| **D-ii** | Selector is an attack surface; label may be attacker-set (TC10) | D | **High, Mitigable** (chief D risk) | Infer+label sensitivity (taint), don't self-assert; strict mode; leash lint; surface applied leash | Largest test/config matrix; ongoing config discipline |
| **D-iii** | Cross-task poison propagation — per-task verify is blind (TC8) | A/B/C/D | **Mitigable, most under-defended** | **Tier-by-graph-position** (foundational⇒A/C, leaf⇒B); provenance tracking; re-verify inputs across trust boundaries | Selector reasons about topology; N×-on-critical-path re-verify |

### 5.3 Fatal-finding summary

Only **three findings are Fatal**, and — exactly as in the WG-Fed sibling — each is
candidate-specific and *bounded*, concentrated where the candidates most diverge:

- **A-i (no defence vs the malicious provider) is Fatal *for A used outside its trust
  scope*.** A's confidentiality and integrity are both "the provider is honest"; there
  is no in-band fix that preserves A's premise (re-run/attest *is* B/C). Bounded by:
  keeping A's scope honest (trusted compute only) and the leash refuse-row firing on
  confidential work. **A is not broken — it is *scoped*; the fatality is in *misuse*,
  which is a TC10/selector problem.**
- **B-i (sybil/collusion) is Fatal *for the open market*** — free keys defeat quorum,
  the squat-penalty, and reputation simultaneously, and permissionless sybil-resistance
  is unsolved (the same wall the WG-Fed sibling's A-2 hit). The only fixes (vouch/stake/
  attest) **close the market into a cooperative**, surrendering the openness that was
  B's reason to exist. **Mitigable for a vouched cooperative; Fatal for a truly open
  market in v1.**
- **D-i (fail-safe-to-A) is Fatal-as-written** (a confidential task silently exposed)
  but **trivially mitigable** by making the unlabeled default fail-*closed* (refuse/C).
  It is listed Fatal to force the fix into doc 06, not because it is unfixable.

Every other finding (the substrate X-1…X-7, A-ii/iii, B-ii/iii, C-i…iv, D-ii/iii) is
**Mitigable or Inherent-bounded**. Two structural truths the register makes plain:

1. **No candidate solves the confidentiality crux (TC1) cheaply.** A doesn't try; B
   minimizes (not the same thing); C pays the TEE/vendor/curation tax; D routes to C.
   *Confidentiality on untrusted compute costs a TEE, and the TEE costs maturity,
   money, decentralization, and a residual side-channel.*
2. **The most under-defended attack is TC8 (cross-task poison propagation)** — *not*
   any single-task crux — because every candidate's integrity story is per-task and the
   threat is cross-task. The only structural answer (tier-by-graph-position +
   provenance) is **available only in D**, which is the strongest single argument for D.

---

## 6. Scoring

The brief's **eight axes**, 1–5 (5 = best), scored **adversarially** — weighting
worst-case and abuse-resistance, per this document's purpose as a security/reliability
gate, not best-case elegance. The two cruxes (confidentiality, result-integrity) are
the load-bearing axes (doc 03 §0).

| Axis | A | B | C | D | What the axis measures |
|------|:-:|:-:|:-:|:-:|------------------------|
| **Confidentiality** (TC1) | 2 | 3 | 4 | 4 | Does context survive a malicious provider? (HQ1) |
| **Result-integrity** (TC2) | 2 | 3 | 4 | 3 | Is a forged result caught when the provider owns the env? (HQ2) |
| **Decentralization** | 5 | 3 | 2 | 4 | No mandatory central/non-WG root of trust (HQ10) |
| **Liveness** | 5 | 4 | 4 | 4 | Squat/orphan/fencing resistance (HQ6) |
| **Simplicity** | 4 | 3 | 2 | 2 | Few moving parts; small surface to get wrong |
| **WG-fit** | 5 | 3 | 2 | 3 | Distance from today's code; reuse of spawn/lease/eval (doc 02) |
| **Operational cost** | 4 | 3 | 2 | 3 | Total user+operator burden, incl. running the *defences* |
| **Maturity** | 4 | 3 | 2 | 3 | Proven-ness of the blueprint end-to-end under attack |
| **Unweighted total** | **31** | **25** | **22** | **26** | |

### 6.1 Per-axis justification

**Confidentiality — A 2 · B 3 · C 4 · D 4** (§3.1). A defends nothing against the
provider (pure trust) — 2, not 1, because transit is sealed and it *refuses*
confidential work rather than silently exposing it. B's minimization shrinks blast
radius and keeps credentials off the box but the provider reads the slice and every
token — 3. C is the *only* candidate that defends context against a root operator —
4, docked one for the side-channel/vendor-root/measurement-curation residual (not
unconditional). D's ceiling = C (it routes confidential ⇒ C-or-refuse) — 4, with the
selector-misconfig discount already baked vs a naive 5.

**Result-integrity — A 2 · B 3 · C 4 · D 3** (§3.2). A's attribution is worthless
against a signer-holding provider; only the eval-gate (a quality filter) stands — 2.
B is strong for checkable code re-run in a trusted domain against a pinned spec, weak
for non-checkable / test-poisonable / collusion — 3. C's attestation-of-process is the
best fit for nondeterministic agents *if it holds*, but concentrates risk in the
attestation chain and leaks at the enclave-I/O boundary — 4. D matches the best per
tier but **under-verifies the fungible normal-sensitivity middle by default** (P6
behave-then-defect) — 3 until spot-checks lift it.

**Decentralization — A 5 · B 3 · C 2 · D 4.** A needs *zero* central nodes
(per-authorizer scheduling onto your own pool) — 5. B's directory/queue/reputation may
be central convenience (sig-checked, degrade-not-break) but the open market *leans* on
them for reach — 3. C roots in a **hardware vendor** (Intel/AMD/AWS) — a real non-WG,
non-decentralized, compellable dependency (C-iii) — 2. D *defaults* to A's
zero-central posture, with B/C's hints only when enabled — 4.

**Liveness — A 5 · B 4 · C 4 · D 4.** This is the *most mature* area — lifted from
WG's existing `claim→heartbeat→reclaim` lease lifecycle (doc 02 §2.5) — so scores
cluster high, honestly. A: enrolled pool, fencing kept-though-cheap — 5. B: fencing
load-bearing (it holds, X-4) but **sybil-squatting dodges the reputation penalty** — 4.
C: attestation-bound renewals are a *stronger* liveness proof, but TEE infra adds
operational failure modes (enclave crashes/teardown) — 4. D: one lease across tiers,
sybil-squat exposure only when B is enabled — 4.

**Simplicity — A 4 · B 3 · C 2 · D 2.** A is the smallest delta (doc 04 §2.10: "two
`wg` daemons, one graph, a signed bundle between them") — 4. B adds queue + reputation +
re-run/quorum + sybil concerns — 3. C is the **most moving parts** — TEE runtime,
attestation verify, seal-to-quote, *measurement-allow-list curation* — a large, subtle,
security-critical surface — 2. D adds the selector + leash policy + fail-safe defaults
on top of all three tiers — the largest config surface to misconfigure (TC10) — 2.

**WG-fit — A 5 · B 3 · C 2 · D 3.** Doc 02 is decisive: A reuses almost the entire
spawn machinery and the existing lease lifecycle (`claim.rs`, `dead_agents.rs`,
`reclaim.rs`) — the natural lift of today — 5. B adds new surfaces but `wg claim`
already abstracts "who may run a task" (doc 02 §2.4) and the eval-gate is reused — 3. C
needs a TEE runner + attestation WG has no analogue for — 2. D's selector wires onto
`plan_spawn`'s new placement field (doc 02 §2.1/§2.3 names this the right seam) — clean
seam, large surface — 3.

**Operational cost — A 4 · B 3 · C 2 · D 3.** A: lowest — no TEE, no N× re-run, no
market infra; you run a second daemon — 4. B: N× re-run/quorum + directory/queue/
reputation + sybil-mitigation overhead — 3. C: TEE premium + attestation infra +
**ongoing measurement curation** (not a one-time setup) + side-channel patching — 2. D:
pay-for-what-you-enable + the selector/lint/config-audit tax — 3.

**Maturity — A 4 · B 3 · C 2 · D 3.** A: Temporal/CI/Ray posture on WG's
battle-tested lease runtime; only the cross-host wire/UCAN layer is new — 4. B: strong
prior art for placement (Akash, CI) and quorum (BOINC) but the determinism-reframing is
WG-specific and sybil-resistant open markets are unsolved — 3. C: confidential compute
is emerging and the TEE security record is a parade of broken side-channels — 2. D:
A's maturity on the default path, but the full hybrid + selector is unproven as a
whole — 3.

> **Scoring honesty (the load-bearing caveat).** The unweighted total puts **A first
> (31 > D 26 > B 25 > C 22)** — and the danger is *how that lead is read*. A tops the
> scoreboard largely on the **convenience axes** (decentralization 5, liveness 5,
> WG-fit 5, simplicity/cost/maturity 4) while scoring the *worst of the four* on the
> two cruxes (confidentiality 2, integrity 2) — because **it does not attempt to
> defend them; it assumes the provider honest.** Read naively, A's lead looks like "A
> is the most *secure mechanism*," which is false. A security gate must **weight the
> two cruxes** (the study's whole point, doc 03 §0) above the convenience axes — and
> when it does (cruxes ×2: A 35 > D 33 > B 31 > C 30), the ordering is **unchanged**.
> That robustness is the real finding: A's top spot survives crux-weighting **not
> because it answers the adversary but because its crux-2s are *scope, not failure*
> (it refuses what it can't do) on top of a proven, simple, decentralized base.** §7
> defends *why* A leads (honest scoping), and why D > B > C beneath it (navigation >
> conditional-tier > concentrated-risk), rather than treating the total as the verdict.

---

## 7. Defended ranking

This is a **security/reliability gate over the confidentiality-vs-integrity-vs-openness
trilemma**, so the ranking weights the two **cruxes (confidentiality, result-integrity)
highest**, then **liveness** (the must-not-orphan property), then **maturity +
simplicity** (a gate distrusts large unproven surfaces), with **WG-fit** as the
tie-breaker doc 06 cares about — and treats **decentralization/operational-cost** as
desirable but not safety-critical. Under that lens — which *confirms* the unweighted
ordering (A > D > B > C) while correcting the *reasoning* the naive total invites (the
scoring-honesty note) — the ranking is:

> ## 1. A   ·   2. D   ·   3. B   ·   4. C   *(as whole architectures)*
> ### with **C first among *components*** (the only answer to the confidentiality crux)

The ordering deliberately mirrors the WG-Fed sibling's logic — **rank the proven,
smallest-surface, never-silently-wrong option first; rank the best *destination* second
but penalize it for being the riskiest *build*; rank the last-place architecture last
*as a whole* while naming it first *as a component*.**

### Rank 1 — **A (trusted private pool)** — *what a gate trusts and ships now*

**Not because it's the strongest mechanism — it is the *smallest proven surface that
is never silently wrong*.** A leads liveness (5), simplicity (4), WG-fit (5), maturity
(4), decentralization (5) and reuses WG's battle-tested lease lifecycle almost whole
(doc 02 §2.5). Its confidentiality-2 / integrity-2 are **not failures — they are
*scope*:** A is correct *iff* its trust assumption holds, and it **loudly refuses**
(FR-K5) what it cannot do, so it never *silently* exposes context or accepts an
unverifiable result. A security gate ranks the *recoverable, proven, nearest, honestly-
scoped* option first, even though it is the *least capable against the malicious
provider* — because A is also **`D`-with-only-the-trusted-tier-enabled** and the only
thing reachable today (NFR-3 names it the v0). Its one Fatal finding (A-i) is bounded
to *misuse* (running it outside its trust scope), which is a selector/TC10 problem the
leash refuse-row + spot-checks contain. **Ship A; do not pretend it answers the
adversary it is scoped to exclude.**

### Rank 2 — **D (hybrid / leash selector)** — *the best destination, penalized for the riskiest build*

**The only candidate that *navigates* the trilemma instead of picking one corner** —
it routes each task to the tier that can defend it (confidential ⇒ C-or-refuse,
low-trust ⇒ verified-B, trusted ⇒ A), so its confidentiality ceiling = C (4) and its
integrity ceiling = C, and it is the **only** candidate that can defend the study's
most under-defended attack — TC8 cross-task poison propagation — *structurally*
(tier-by-graph-position + provenance, D-iii). It ranks **below A** for two adversarial
reasons that exactly parallel the sibling's treatment of *its* destination-candidate:
**Simplicity 2 and the selector-as-attack-surface (TC10/D-ii)** — D is the largest,
newest, security-critical config surface, and "secure only if configured fail-closed"
is a real liability under attack. Its two High findings are *mitigable but mandatory*:
**D-i (make the unlabeled default fail-*closed* — refuse/C, never A)** and **D-ii
(infer-don't-self-assert sensitivity + strict mode + leash lint + surface the applied
leash)**. Crucially, **A is *reachable inside* D** (A is D with one tier enabled, doc
04 §5.3), so ranking D second is not a rejection — it is "**D is where you converge;
you get there *through* A, not instead of it.**"

### Rank 3 — **B (capability-gated market)** — *a valuable tier, the most-exposed standalone*

B earns genuine integrity (re-run/quorum on checkable artifacts, X-5/§3.2) and extends
reach to semi-trusted compute — but as a *standalone architecture* it carries the
study's hardest unsolved problem: **B-i (sybil/collusion), Fatal for the *open*
market**, which breaks quorum + squat-penalty + reputation at once and is only fixable
by *closing* the market into a vouched cooperative (the openness it was meant to buy).
Its integrity is *conditional* (checkable artifacts only, B-ii) and its confidentiality
is minimization-only (refuses confidential, §3.1). It ranks below A and D because A's
trusted scope is safer-and-simpler and D *contains* B as its verified-overflow tier
under a selector that can gate quorum to vouched/attested providers. **B is best
understood as a *tier within D* (checkable code on a vouched pool), not a general
execution market.**

### Rank 4 — **C (confidential compute / TEE)** — *last as a whole architecture; first as a component*

**Last as a standalone** because it is the least mature (2), costliest (2), least
decentralized (2 — the vendor root, C-iii), most complex (2 — measurement curation,
C-i), and **concentrates catastrophic risk in one chain**: a single attestation break
forges *both* confidentiality *and* integrity (C-ii), and the enclave-I/O boundary
leaks poison under a valid quote (§3.2). As the everyday architecture it is
indefensible for a gate. **But this ranking is about C-as-default, not C-as-capability**
— exactly the sibling's D-UCAN move: **C is the *only* answer to the confidentiality
crux (TC1), the only candidate where a provider you do *not* trust can hold confidential
context, and its attestation *doubles as sybil-resistance* that strengthens B's quorum
(B-i corollary).** The defended recommendation is therefore **not "discard C"** — it is
**harvest C's attested tier as D's confidential escape-hatch, ship its *slot* early
(doc 04 §4.10 — interface before enclave), and require *attested* (never
self-advertised) isolation for any confidential routing.**

### 7.1 The synthesis the ranking points doc 06 toward

The ranking is **not** "pick A, discard the rest." Read as a *phased, defended* plan it
says — reaching doc 04 §6.3's hint *adversarially*:

1. **Ship A as phase-2** (doc 04 §9) — the proven, simplest, nearest, never-silently-
   wrong option a security gate trusts *today*; it reuses the existing spawn + lease
   machinery (WG-fit 5) and is the v0 milestone (NFR-3).
2. **Keep the leash engine + refuse-row as the correctness guardrail from day one**, so
   confidential work is *never* silently exposed even while only A is enabled — this is
   what makes you **D-shaped** before D is fully built, and it neutralizes A-i's
   fatality at the policy level. **Make the unlabeled default fail-closed (refuse/C, not
   A) — D-i.**
3. **Graft C's attested tier as the confidential escape-hatch**, shipping the *slot*
   (the `attest.rs` interface + seal-to-attestation hook) **before** any enclave exists
   (doc 04 §4.10), so confidential work is a provider-capability away, not a redesign —
   and so C's attestation is available as sybil-resistance for the B tier.
4. **Add B's verified tier for overflow** — re-run/quorum on checkable artifacts,
   **re-run in a trusted domain against a pinned spec (X-5/X-6)**, **gated to
   vouched/attested providers** (not open-market) until sybil-resistance is solved.
   Route only checkable code work here (B-ii).
5. **Treat cross-task poison (TC8) as a first-class placement constraint** — foundational
   tasks ⇒ A/C tiers, leaf tasks ⇒ B; provenance-track every artifact; re-verify inputs
   across trust boundaries. This is the study's most under-defended attack and **only D's
   whole-graph selector can defend it (D-iii)** — the strongest single argument for the
   D convergence target.

This survives the threat model because each piece is chosen for the axis it is
strongest on and bounded on the axis it is weakest on. **Doc 06 owns the final call
against the doc-03 requirements; this document's defended contribution is that
`A-shipped-first → D-as-convergence-target, with C's attested tier as the confidential
escape-hatch grafted early and B as the vouched-overflow tier`, with the unlabeled
default fail-*closed*, is the only arrangement in which no Fatal finding remains
unbounded.**

---

## 8. Handoff to doc 06 (decision)

- **The two cruxes' verdicts (the load-bearing inputs):** *confidentiality* — only **C
  (and D-routing-to-C)** defends context against the provider; B minimizes (≠ hides),
  A trusts (≠ defends); confidentiality on untrusted compute **costs a TEE** with its
  full residual (§3.1). *Integrity* — **C ≥ B-in-scope > D-default > A**, but the
  deepest threat is **cross-task, not single-task** (§3.2).
- **The three Fatal findings to design around** (§5.3): **A-i** (keep A's scope honest;
  enforce the refuse-row; spot-check trusted providers), **B-i** (gate the market to
  vouched/attested providers — accept it is a cooperative, not an open market, in v1),
  **D-i** (fail-*closed* default: unlabeled ⇒ refuse/C, never A).
- **The most under-defended attack to budget engineering for regardless of choice:**
  **TC8 cross-task poison propagation** — per-task verification is structurally blind to
  it; the only answer is D's tier-by-graph-position + provenance + cross-trust-boundary
  re-verification (D-iii). Treat this as a *requirement*, not a nicety.
- **The three substrate findings every tier inherits:** **X-2** (minimization ≠
  confidentiality — taint-track the slice), **X-5/X-6** (the provider authors the
  evidence — re-run in a trusted domain against a *pinned* spec; tests are spec),
  **X-3** (intent-bind + meter the privileged-op/inference callback).
- **The defended ranking** (§7): **A > D > B > C as whole architectures**, with **C
  first among components** (the attested tier) — feeding directly into doc 06's likely
  shape: *A as the shipped v0 default, D as the convergence target, C's attested tier as
  the confidential escape-hatch (slot early), B as the vouched-overflow tier, the leash
  engine fail-closed from day one.*
- **The irony to price** (the trilemma made concrete, §1.3): *openness* and
  *confidentiality* pull against each other at the placement layer, and *confidentiality
  (C)* and *integrity-evidence* pull against each other at the verification layer — the
  recommended path resolves both by **defaulting to a trusted pool (no tension), reaching
  for the TEE only when confidentiality demands it (pay the tension's price per-task),
  and verifying checkable artifacts in a trusted domain (evidence without exposing the
  transcript)** — i.e., the leash engine pricing the trilemma per task rather than
  globally.

---

## 9. Validation checklist (this document)

- [x] **Each candidate attacked across ≥8 threat classes.** All four (A §4.1, B §4.2, C
      §4.3, D §4.4) attacked across **all ten** TC1–TC10, one row each, plus 2–4
      divergent-finding prose items each. (≥8 satisfied with margin.)
- [x] **The malicious-provider-reads-context attack covered specifically.** TC1 is the
      first threat class; §3.1 is a dedicated cross-candidate deep-dive with a verdict
      (C > D-ceiling > B > A); a TC1 row per candidate in §4; substrate findings X-1/X-2
      develop the wire + minimization attacks.
- [x] **The result-forgery attack covered specifically.** TC2 is the second threat
      class; §3.2 is a dedicated cross-candidate deep-dive with a verdict (C ≥ B-in-scope
      > D-default > A); a TC2 row per candidate; substrate findings X-5 (provider authors
      the evidence) and X-6 (test-poisoning) develop it.
- [x] **All nine brief-named attack categories covered:** malicious-read (TC1/§3.1),
      forgery (TC2/§3.2), stall/deny + distributed-orphan (TC3/X-4), capability
      theft/replay/over-scope/escalation (TC4/X-3/X-4), impersonation/sybil/collusion
      (TC5/X-7), TEE side-channel + attestation forgery/compromise (TC6/§4.3), data
      exfil in-transit/at-rest (TC7/X-1/X-2), downstream-`--after` poison (TC8/D-iii),
      economic/billing abuse (TC9/B-iii/X-3).
- [x] **Rubric scores with justification.** §6 table on the brief's eight axes
      (confidentiality, result-integrity, decentralization, liveness, simplicity,
      WG-fit, operational cost, maturity) + §6.1 per-axis prose + the scoring-honesty
      caveat (the naive total rewards opting out of the threat model).
- [x] **Failure modes classified fatal/mitigable (+ inherent-bounded) with mitigation +
      cost.** Consolidated register §5 (substrate X-1…X-7 + per-candidate divergent),
      each with disposition, mitigation, and cost; Fatal-finding summary §5.3 (three
      bounded Fatals).
- [x] **A defended overall ranking.** §7: **A > D > B > C** as whole architectures (with
      **C first among components**), weighted for a security gate, defended against the
      naive total, with the phased synthesis (§7.1) handed to doc 06.
- [x] **File written:** `docs/execution-federation-study/05-adversarial-evaluation.md`.

---

*Wave-1 evaluate phase (execution federation) complete. The four candidates were
attacked across ten threat classes — the two cruxes (malicious-provider-reads-context,
result-forgery) deepest — and the honest finding is that **confidentiality on untrusted
compute is not solved cheaply by anyone** (A trusts, B minimizes, only C's TEE defends
it, at a maturity/cost/centralization/side-channel price), while **the most
under-defended attack is cross-task poison propagation**, which only D's whole-graph
selector can defend structurally. The defended ranking — **A (ship now, proven,
honestly-scoped) > D (the convergence target, penalized for the selector's config risk)
> B (a vouched-overflow tier, not an open market) > C (last as an architecture, first as
the confidential-escape-hatch component)** — with three bounded Fatal findings (A
misuse, B's open-market sybil, D's fail-safe-to-A) and a fail-closed leash engine from
day one, is handed to the decision memo.*
