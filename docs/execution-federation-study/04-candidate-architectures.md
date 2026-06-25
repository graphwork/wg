# Execution Federation Study 4/6 — Candidate Architectures

> **Execution-federation study, wave 1, task 4 of 6 (the *generate* phase).**
> Four fully-worked candidate architectures for federating the **execution plane**
> — placing an *already-authorized* task's agent onto compute that need not belong
> to the authorizer, gated by trust + capabilities, returning *verifiable* results
> while keeping the agent's working context *confidential*.
>
> The candidates span the trust/openness spectrum: **A — trusted private pool**,
> **B — capability-gated cooperative / market**, **C — confidential compute (TEE)**,
> **D — hybrid synthesis**. Each answers *every* hard question from doc 03
> (`03-requirements-and-hard-questions.md`), composes with the **WG-Fed**
> identity/UCAN substrate decided in `docs/federation-study/06-decision-memo-and-roadmap.md`,
> and maps to concrete WG code changes + a migration path.
>
> The adversarial pass (`exec-adversarial`, 5/6) attacks these — especially the
> malicious-provider / result-forgery cruxes; the decision memo (`exec-decision`,
> 6/6) picks and roadmaps. This document *proposes*; it does not *decide*.

**Status:** draft for evaluation · **Date:** 2026-06-25 · **Owner task:** `exec-architectures`
**Inputs:** `01-prior-art-landscape.md` · `02-current-state-baseline.md` ·
`03-requirements-and-hard-questions.md` · WG-Fed decision memo
(`docs/federation-study/06-decision-memo-and-roadmap.md`).

---

## 0. How to read this document

WG today runs **every agent as a local subprocess on the dispatcher's own host,
inside a git worktree that symlinks the one shared `.wg/` graph** (doc 02 §0). The
dispatcher fuses *"this task is approved to run"* with *"this CPU runs it"*
(`spawn_agents_for_ready_tasks`, `coordinator.rs:3924`). The whole study is the
project of **splitting** those two — and doc 02 §4's load-bearing finding is that
WG already owns a *complete lease lifecycle* (`claim → heartbeat → reclaim`) that
"happens to run entirely inside one trust domain." Federation is **re-substrating**
that lifecycle so every place it relies on shared-host trust becomes a place where
a **signed message, a scoped capability, or a verified result** stands in.

Read the sections in this order:

1. **§1 — the shared execution-plane substrate.** The wire envelopes, the two
   scoped UCANs, the context bundle, the cross-host lease, the **leash policy
   engine**, and the common code skeleton that **all four candidates configure**.
   This is *not* a candidate — it is the agreed foundation, and it discharges
   **HQ11** (leash calibration) and **HQ12** (wire compat) once for all four.
2. **§2–§5 — Candidates A / B / C / D.** Each has the same eleven subsections so a
   reader (or the doc-05 adversary) can compare them line-for-line. Every candidate
   answers all twelve HQs; where a HQ is discharged by §1 (HQ11/HQ12), the candidate
   states only its *delta*.
3. **§6 — cross-candidate comparison** (spectrum placement, central-node table,
   decision-relevant deltas).
4. **§7 — resolution of doc 03 §4's ten cross-cutting tensions.**
5. **§8 — the 12-HQ × 4-candidate coverage matrix + the doc 03 §6 acceptance
   checklist**, the validator's one-glance check.
6. **§9 — migration phasing** (v0 → … ); **§10 — non-goals honored**; **§11 —
   handoff** to doc 05/06.

**Naming convention (carried from doc 03 §0).** The **authorizer** holds the graph
+ dispatch authority. The **provider** owns the compute. The **worker** is the
agent process that runs on the provider under a scoped delegation. All three are
`wgid:` identities (WG-Fed HQ5). "Remote provider" here means a remote *execution
host* that runs the agent process — **not** today's sense of a remote LLM API
(doc 02 §1, "framing convention").

---

## 1. Shared design primitives (the substrate all four configure)

The four candidates differ in *how much trust the placement assumes* and *how
results are verified* — i.e. **which of doc 02 §3's MISSING rows each one buys**.
They do **not** differ in the wire, the delegation format, or the lease mechanics:
those are **one substrate**, reused from WG-Fed (NFR-4/EX5 — invent no second
identity/delegation/crypto/trust system). This section fixes that substrate.

### 1.1 The actors and the one trust dial (HQ4, reused)

- **Authorizer / provider / worker are `wgid:<pubkey>` identities** (WG-Fed HQ5,
  `IdentityRecord`), self-certifying, verified by a local signature check against
  the sigchain rooted at the genesis pubkey — **never central** (WG-Fed HQ6,
  FR-R1). A provider authenticates with its key; a spoofed provider fails
  signature verification.
- **`Agent.trust_level` is the single dial** the whole execution plane reads
  (FR-R2). It gates placement (HQ3), leash tightness (HQ5/HQ11), context exposure
  (HQ1), verification depth (HQ2), and lease term (HQ6). WG-Fed already defines it
  (Verified / Provisional / Unknown, doc 02 §2.8 / WG-Fed HQ8); this study *reads*
  it, it does not redefine it.
- **Reputation** (FR-R3) accrues per-authorizer from observed behaviour —
  eval-gate pass-rate, lease/liveness record, integrity-check (re-run/quorum)
  outcomes — and can raise/lower *effective* trust. It is **local-by-default, never
  a mandatory central ledger** (HQ10); it may be gossiped or vouched (WG-Fed
  web-of-trust) but no candidate's correctness depends on a central reputation
  authority. Resistance to the **behave-then-defect** attack is structural, not
  reputational: high-sensitivity or low-trust placement *always* applies the
  verification leash (FR-V5) regardless of accrued reputation, so a provider that
  builds trust then defects on a sensitive task is still caught by re-run/quorum or
  was never given confidential context in the clear (it gets a TEE seal or
  nothing).

### 1.2 The versioned execution wire + compat handshake (HQ12, NFR-5)

A small set of **self-describing, versioned envelopes** carries placement, lease,
capability, and result across the authorizer↔provider boundary. They are **this
study's to version**; identity/delegation/crypto formats are **inherited from
WG-Fed** (the boundary is explicit — NFR-4).

| Envelope | Purpose | Signed by | Carries |
|---|---|---|---|
| `PlacementOffer` | "task T is available / assigned, needs ⟨caps⟩, min-trust ⟨t⟩" | authorizer | task id, required model/handler, isolation class, cost ceiling, sensitivity label, leash params |
| `Claim` | "I, provider P, take T" (pull) / ack of a push | provider | provider `wgid:`, signed capability advertisement (FR-R4), accepted leash |
| `RunGrant` | the two scoped UCANs (§1.3) + sealed context-bundle ref | authorizer | act-as-agent UCAN, graph-write UCAN, bundle CID, decryption-key delivery (§1.4) |
| `LeaseRenewal` | the cross-host heartbeat (§1.5) | worker | lease epoch, progress marker, attestation evidence (C only) |
| `ResultEnvelope` | the returned work product + evidence (FR-V3) | worker | artifacts/diff, token/cost accounting, optional transcript / attestation, signature attributing to agent G |

- **`WG_EXEC_COMPAT_VERSION`** — a named constant in the new `src/providers/mod.rs`
  (§1.7), **mirroring `WG_AGENCY_COMPAT_VERSION` (`1.2.4`)** and
  `WG_PI_PLUGIN_COMPAT_VERSION` (CLAUDE.md). Authorizer and provider exchange it on
  connect and **fail loudly on incompatible mismatch** — never silently mis-route
  (the bare-`openrouter:` 14-hour-401 cautionary tale, doc 03 HQ12 / CLAUDE.md is
  the exact bug this prevents). vN/vN+1 negotiate a shared subset or refuse.
- **The handshake is authenticated** (the WG-Fed S-7 lesson): negotiated parameters
  are *signed*, not merely exchanged, so an active attacker cannot strip the
  encryption requirement or force a weak isolation class. A **minimum floor**
  (min isolation class, min `alg`, must-encrypt) is enforced, not "lowest common."

### 1.3 The two scoped UCANs — capability flow to the worker (HQ5, reused)

The worker receives **exactly two attenuated UCAN delegations** (WG-Fed HQ11,
"the best component in the study") — **never the agent's root signing key** (FR-C1,
WG-Fed HQ1):

1. **Act-as-agent UCAN** — `iss` = the agent's authorizing custodian (the WG node
   or owner holding the root, WG-Fed HQ1/ADR-003), `aud` = the worker/provider,
   capability = *"run task T as agent G,"* + expiry. This lets the worker sign
   results *as* agent G (FR-C5 attribution) **without** holding G's root — a stolen
   signer is near-worthless after expiry.
2. **Graph-write UCAN** — capability = *"log/append to task T, write artifacts
   under T, mark T done, [optionally] create subtasks under T,"* scoped to **task T
   only** — **not** blanket graph write (FR-C2). A worker delegated for T cannot
   mutate task U or impersonate another agent (FR-V4 blast-radius bound).

- **Scope & TTL follow the leash dial** (FR-C4, §1.6): broad/long for trusted
  providers, narrow/short for strangers. Delegation is **attenuating-only**
  (sub-delegation can narrow, never widen — WG-Fed HQ11, kills the hydra) and
  **revocable**: by-expiry (default, short TTLs), by issuer-subtree revocation
  (kill the lease → kill the grant), enforced **at write time** by the authorizing
  graph (FR-C3). A write under an expired/revoked delegation is rejected.
- **Privileged-op callback (the EX6/HQ1 lever):** anything beyond the worker's
  task scope (reading a secret, writing outside T) is **not** granted to the
  worker — it **requests** the action from the authorizer (ssh-agent-style
  "sign/do this" callback, WG-Fed ADR-003). On low-trust providers this keeps the
  authority *off the provider entirely* (FR-K2). On a trusted pool the worker may
  hold a broader standing signer (the leash slack default).

### 1.4 The context bundle — locality, transit, at-rest (HQ7, reused crypto)

The agent's working context must reach the compute. Today it is reached via the
`.wg/` **symlink** (`worktree.rs:70`) — "a remote host cannot symlink your `.wg/`;
this is exactly the boundary federation must replace" (doc 02 §2.2a). The
replacement is a **context bundle**: a signed, content-addressed (BLAKE3),
optionally-sealed `StateSnapshot`-family blob (WG-Fed §2.1 envelopes), built by
promoting today's env-var task descriptor (`execution.rs:603–654`) + the shipped
graph-slice it lacks (doc 02 §3, "self-contained task descriptor: PARTIAL SEED").

- **Contents = a `ContextScope` slice** (`context_scope.rs:17`, the existing
  `Clean < Task < Graph < Full` dial): task T's input + the artifacts it depends
  on, *not* the whole graph (FR-K3 minimization). The slice size **is** the leash
  knob for confidentiality.
- **Movement:** **provider pulls** a signed bundle by CID (decentralization-
  friendly, least-coupling) on the market/confidential paths; **authorizer pushes**
  on the trusted-pool path (operationally simplest). Either way the bundle is a
  defined, addressable artifact, not a symlink (FR-D1).
- **In transit:** WG-Fed **per-recipient sealed envelopes** (X25519 +
  XChaCha20-Poly1305, WG-Fed HQ4) — a relay/MITM sees ciphertext; tampering is
  detected (FR-D2). **No new crypto** (NFR-4).
- **At rest on the provider:** stated **per trust class** (FR-D3) — plaintext on a
  trusted pool you own (A), encrypted-to-enclave on an untrusted provider (C),
  minimized-and-ephemeral on a market provider whose deletion you cannot trust (B).
  **Confidentiality never relies on an untrusted provider's goodwill to delete**
  (the C/B at-rest story is "it never sees plaintext" / "it sees only the minimal
  slice"), which is the honest answer to doc 03 HQ7's "you are trusting the
  provider to delete it."
- **The canonical graph stays at the authorizer.** The provider holds only a slice
  + writes back deltas via the graph-write UCAN; there is one source of truth
  (HQ7, single-writer spine, WG-Fed HQ7).

### 1.5 The cross-host lease — liveness & reclaim (HQ6, lifted from doc 02 §2.5)

WG's `claim → heartbeat(120 s) → heartbeat_timeout(5 min) → mark_dead + unclaim →
reclaim` loop (`claim.rs:13`, `execution.rs:1921`, `config.rs:3966`,
`registry.rs:535`, `dead_agents.rs:156`, `reclaim.rs:16`) is "in all but name a
lease manager" (doc 02 §2.5 verdict). The substrate **lifts it across the trust
boundary** by replacing two shared-host assumptions:

- **Liveness ≠ local PID.** `is_live()` today requires `is_process_alive(pid)`
  (`registry.rs:118`) — "meaningless across a machine boundary" (doc 02 §2.5).
  Federated liveness is a **signed `LeaseRenewal`** the worker sends each period;
  the authorizer judges liveness by **its own observation of accepted renewals**,
  not the provider's self-report (FR-L3) — a provider that lies "still alive" but
  produces no accepted renewal/result is still reclaimed when the lease lapses.
- **Fencing against double-execution (FR-L2, the split-brain hazard).** Each lease
  carries a monotonic **epoch**. Reclaim increments the epoch and re-places. A
  **late-returning original worker** (merely partitioned, not dead) presents a
  stale epoch / expired delegation; its `ResultEnvelope` write is **rejected at the
  boundary** by the graph-write UCAN check (§1.3). Two workers can never both
  commit. This is the Temporal/k8s-lease pattern (doc 01 §2.2/§2.3) made
  cryptographic.
- **Lease term rides the dial** (FR-L4): long/relaxed for trusted providers, short/
  aggressive for strangers. **Squatting** (hold-lease-produce-nothing DoS, HQ6) is
  bounded by lease caps + no-progress reclaim + a reputation penalty (FR-R3).

### 1.6 The leash policy engine (HQ11 — the spine, decided once)

EX6 ("authority broad/long by default, tightening is policy not birth-state") only
works if there is **one legible policy function** mapping environment → all five
execution dials *coherently* (doc 03 HQ11, the five-dials-at-once requirement). The
substrate defines exactly that, and every candidate is a **named region of its
output space**:

```
leash(provider_trust, task_sensitivity, pool_class, env_config) → {
    delegation:   { scope, ttl },          // FR-C4  (HQ5)
    context:      { scope_tier, seal },     // FR-K*  (HQ1/HQ7)
    isolation:    min_class,                // FR-D4  (HQ8)
    verification: depth,                    // FR-V5  (HQ2)
    lease:        { term, renew_cadence },  // FR-L4  (HQ6)
}
```

- **Default is genuinely slack** (EX6 honoured — *not* a zero-trust system in
  disguise): for the trusted/private-pool case (`provider_trust = Verified`,
  `task_sensitivity = normal`) the function returns broad/long delegation, full
  context, worktree/container isolation, attribution-only verification, a long
  lease. That is **Candidate A**.
- **Tightening is config-driven, per-deployment, no code change** (a paranoid org
  vs a solo hobbyist set different thresholds): lowering `provider_trust` or
  raising `task_sensitivity` narrows scope, shortens TTL, shrinks the context
  slice, raises the isolation floor, and deepens verification — **coherently, in
  one place**. Tightened-for-strangers is **Candidate B**; sensitivity-forces-
  attestation is **Candidate C**.
- **A too-loose leash on a low-trust provider is impossible by construction**: the
  function *cannot* emit broad-scope + full-plaintext-context for
  `provider_trust < floor` — it either returns the minimized/sealed region or
  **refuses placement** (FR-K5 loud degradation). The policy *is* the guardrail.
- **The applied leash is surfaced** (mirrors the handler-first `wg status`
  rendering, CLAUDE.md): `wg show <task>` / a new `wg providers` view renders the
  resolved leash so a too-tight/too-loose setting is caught at a glance.

This is the single most important shared decision: the four candidates are not four
mechanisms, they are **four operating points of one policy** (FR-P4 — "moving a
provider from my pool to a stranger changes only its trust level + the applied
leash, not the placement protocol").

### 1.7 New & changed WG code surface (common skeleton)

All four candidates share this skeleton; each adds candidate-specific pieces in its
own §x.10. Grounded in doc 02's cited seams.

- **NEW `src/providers/` module** — the home for everything cross-host, the
  execution-plane analog of WG-Fed's new `src/identity/`:
  - `mod.rs` — `WG_EXEC_COMPAT_VERSION`, the wire envelopes (§1.2), the
    `ProviderRegistry` (a directory of `wgid:` providers + capability ads + trust +
    reputation).
  - `placement.rs` — the matcher (capability + trust-floor filter; optional rank)
    and the `leash()` policy engine (§1.6).
  - `bundle.rs` — build/seal/verify the context bundle (§1.4), reusing
    `src/identity/` crypto.
  - `lease.rs` — the signed cross-host lease + fencing epoch (§1.5).
  - `verify.rs` — the result-verification levers (attribution / eval-gate / re-run
    / quorum / attestation, selected by `leash().verification`).
- **`handler_for_model.rs` (`:64`, `:87`) gains a `RemoteRunner` `ExecutorKind`
  arm.** Doc 02 §2.3 says this module "is explicitly designed for new arms" and
  that "the cleanest place to add a placement axis is here." A `RemoteRunner`
  handler ships the bundle out and gets a signed `ResultEnvelope` back — "slots in
  beside claude/pi" but with categorically different semantics.
- **`plan_spawn` (`dispatch/plan.rs`, `coordinator.rs:4148`) gains a placement
  field.** Today it is "the ONLY place that decides `{executor, model, endpoint}`"
  and "has no host / placement parameter" (doc 02 §2.1/§2.3). It becomes
  `{executor, model, endpoint, placement}` where `placement ∈ {Local, Provider(wgid:)}`,
  resolved through `placement.rs`.
- **`wg claim` (`claim.rs:13`) becomes capability-gated.** Today claim is "a field
  in `graph.jsonl`, enforced only by everyone sharing the file" with "no lease
  token, no lock to a host" (doc 02 §2.4). The federated claim carries a `Claim`
  envelope proving provider eligibility (capability ad + trust), and the
  authorizer's grant is the `RunGrant` (the two UCANs). The optimistic
  status-as-lease becomes a **signed, fenced lease**.
- **The agent registry (`registry.rs`) gains a `ProviderEntry` + cross-host
  liveness.** `AgentEntry` (`:60`) is keyed by local PID; a federated worker is
  keyed by `(provider wgid:, lease epoch)` and its `is_live()` (`:118`) consults
  **last accepted `LeaseRenewal`**, not `is_process_alive`.
- **Heartbeat / sweep / dead-agents (`heartbeat.rs`, `sweep.rs`,
  `dead_agents.rs`) already implement the reclaim half** — they need only learn the
  signed-renewal liveness source and the fencing-epoch check (§1.5). The
  reconcile-orphaned-tasks safety net (`sweep.rs`, `coordinator.rs:61`) is reused
  verbatim.
- **Token/cost accounting (`graph::parse_token_usage`, `wg spend`/`wg show`)** is
  the budget/metering substrate (FR-E2, doc 02 §2.7) — reused; a remote
  `ResultEnvelope` carries the same usage fields a local task does (FR-V3, so
  `wg show <remote task>` "is not bare").
- **Migration substrate:** today's `src/federation.rs` path-based transfer is the
  migration substrate, not the redesign target (doc 03 §5 non-goal 8 / WG-Fed
  FR-F6).

---

## 2. Candidate A — Trusted private pool

> **Run only on providers you trust** — your own machines, home server, a handful
> of trusted peers. Minimal new trust machinery: **confidentiality by trust,
> integrity by attribution + the WG eval-gate.** The nearest thing to today, and
> the v0 of every other candidate. This is the **slack default** of the §1.6 leash
> engine (`provider_trust = Verified`, `task_sensitivity = normal`).

### 2.1 Placement & scheduling (HQ3)

- **Push by default**, pull optional. The authorizer's dispatcher assigns a ready
  task to a chosen provider in its known pool — the natural lift of today's
  push-only `spawn_agents_for_ready_tasks` (doc 02 §2.1), now with a `Provider(wgid:)`
  placement (§1.7) instead of an implicit local fork. A provider **may** also pull
  (claim) from the authorizer's own ready queue — the same `wg claim` shape (doc 02
  §2.4), now capability-gated (§1.7) — for self-serve idle capacity (FR-P3).
- **Matching** (FR-P2) is **filter-only**: capability (model/handler available,
  sandbox class) + trust floor (`Verified`). The pool is small enough that ranking
  (FR-P5) is unnecessary; a simple least-loaded tiebreak suffices.
- **Pool = private** (FR-P4): every provider is an identity *you* enrolled and rate
  `Verified`. Decentralization (HQ10): **no central node at all** — per-authorizer
  scheduling onto your own known providers, the NFR-6 self-host baseline. This is
  prior-art **Ray** (fan work across a trusted pool, doc 01 §2.5) + **CI pull-claim**
  (doc 01 §2.1) with the runner being *yours*.

### 2.2 Provider identity, trust & reputation (HQ4)

- Providers are `wgid:` identities enrolled by hand (`wg provider add wgid:<pubkey>`),
  pinned at `trust_level = Verified` (FR-R1/R2). Trust source = **manual operator
  assignment** (the private-pool branch of doc 03 HQ4's axis).
- **Reputation is optional and advisory** here — you already trust the pool; a
  liveness/eval record is kept for ranking only (FR-R3). Capability advertisements
  are signed (FR-R4) but believed (a false ad from a machine you own is a
  you-problem). The behave-then-defect attack is **out of scope by assumption** —
  you do not put strangers in pool A; the moment you would, you are in Candidate B.

### 2.3 Capability flow to the worker (HQ5)

- **Leash at its slackest** (EX6 default). The worker gets a **broad, long-lived**
  act-as-agent UCAN (per-task-subtree, TTL measured in hours) + a task-scoped
  graph-write UCAN (still task-scoped, never blanket — FR-C2 holds *even here*, so
  a single compromised pool member cannot rewrite the whole graph; that is the one
  non-negotiable floor). **Never the root key** (FR-C1).
- The worker may hold a **standing signer** issued by the authorizer's custodian
  (the trusted-pool convenience) rather than calling back for every privileged op —
  but the signer is still scoped + revocable (FR-C3) and the root never leaves the
  custodian (WG-Fed HQ1). Revocation by short-ish re-issue + an explicit revoke
  list checked at write time.

### 2.4 Context confidentiality — THE crux (HQ1)

- **Confidentiality is by trust.** The provider **sees the plaintext context**
  (task input, graph slice, tool outputs, any shipped secret) — and that is the
  *stated, bounded* property for `trust = Verified` (FR-K1): you accept it because
  you own/trust the machine. This is the honest mirror of doc 01's finding that
  "everything that isn't a TEE trusts the runner" (doc 01 §4.1) — A simply makes
  the trust **explicit and `trust_level`-gated** rather than implicit in "it's my
  box."
- **Secrets** (FR-K2): shipped sealed-in-transit (§1.4), held in plaintext **only
  for the task's duration** on the trusted provider — acceptable on a pool you own
  (the doc 03 HQ1 "ship plaintext to trusted pool" branch). Even here the API key
  is delivered **by env, never argv** (today's invariant, `execution.rs:644`).
- **Minimal-context is available but not required** (FR-K3): you *may* set
  `ContextScope = Task` to shrink the slice, but the default is generous because the
  provider is trusted.
- **Attestation slot:** unused in A (that is C) — but the **interface is the same
  bundle** (§1.4), so escalating a task to a confidential provider is a config
  change, not a redesign (FR-K4 "design the slot").
- **Loud degradation (FR-K5):** a task labelled `sensitivity = confidential` that
  *requires* attestation is **not placed in pool A** — it is held with a
  "no eligible confidential provider" reason (escalate to C), never run with its
  context exposed.

### 2.5 Result integrity — the co-crux (HQ2)

- **Attribution + eval-gate.** The `ResultEnvelope` is **signed by the worker's
  delegated act-as-agent signer**, attributable to agent G (FR-V1/C5); an unsigned
  or wrong-signed result is rejected. Then WG's **existing eval-gate**
  (`auto_evaluate` / FLIP scoring output against `## Validation`, doc 02 §2.7)
  judges *quality* exactly as for a local task (FR-V2). No re-run, no quorum — the
  provider is trusted (FR-V5: trusted results accepted on attribution; this is the
  cheap end of the dial).
- This is prior-art **CI/SLSA provenance** + **trusted-pool acceptance** (doc 01
  §4.2 path 3): integrity = "the result is signed, attributable, and quality-scored;
  trust in the pool covers the rest."
- **Blast-radius bound (FR-V4):** even if a pool member is *quietly* compromised,
  its forged result is capped by the **task-scoped graph-write UCAN** (§1.3) — it
  can corrupt *its own task's* output (caught by the eval-gate / a human), never
  mutate the rest of the graph or impersonate another agent. The forging provider
  is auditable/revocable after the fact (NFR-7).

### 2.6 Liveness, lease & reclaim (HQ6)

- The §1.5 lease, with the **dial relaxed**: a long lease term, relaxed renewal
  cadence (the 120 s / 5 min ratio of today, or longer). Renewals are signed
  `LeaseRenewal`s; fencing epochs still defeat double-commit (FR-L2) even though a
  trusted partition is unlikely — the floor is cheap and worth keeping.
- Reclaim re-uses today's `mark_dead + unclaim → Open` path (`dead_agents.rs:156`)
  unchanged in spirit, now triggered by missed *signed* renewals rather than a dead
  PID.

### 2.7 Data/context locality + isolation (HQ7, HQ8)

- **Locality:** authorizer **pushes** the bundle (simplest); at rest **plaintext**
  on the trusted provider, ephemeral, disposed after the task — and here
  confidentiality *may* rely on the trusted provider to delete, because it is
  trusted (the one case doc 03 HQ7 allows it).
- **Isolation (FR-D4):** the **federated successor of today's git worktree**
  (`worktree.rs:29`) — a **container** on the provider host (worktree-equivalent
  filesystem isolation, plus process/cgroup limits since it is now a separate host).
  This is the bottom rung of the §6.1 isolation ladder; a microVM (Firecracker,
  doc 01 §2.5 E2B) is optional for stronger host-protection. Network egress
  defaults to allow-listed (the agent's tools/APIs), tightenable per deployment.
- **Both directions (FR-D5):** on a *trusted* pool the dominant concern is the
  host-from-agent direction (a poisoned task shouldn't wreck your home server) —
  the container handles it. The agent-from-host (confidentiality) direction is
  covered by trust, not isolation (that's C).

### 2.8 Economics / budget (HQ9)

- **Payment model: "you own the pool, you pay"** (FR-E1) — authorizer-funded via
  its own provider credentials, or the provider uses its own model access since it
  is your machine. Explicit, not assumed.
- **Budgets/ceilings (FR-E2, R32):** enforced via the existing token/cost
  accounting (`parse_token_usage`, `wg spend`) — a remote task carries the same
  usage in its `ResultEnvelope`, so a per-task $-ceiling halts/flags a runaway
  exactly as locally. Metering is believed (trusted pool); signed-reconcilable
  metering (FR-E3) is a B/C concern.

### 2.9 Leash setting (HQ11)

The **slack default** itself (§1.6): broad+long delegation, full context, container
isolation, attribution-only verification, long lease. A is *defined* as
`leash(Verified, normal, private, default)`.

### 2.10 Concrete WG mapping + migration

- **Smallest delta of all four.** `src/providers/` ships with a hand-curated
  `ProviderRegistry`; `placement.rs` does filter-only matching; `bundle.rs` pushes a
  sealed bundle; `lease.rs`/`verify.rs` are thin (signed renewal + attribution +
  the *existing* eval-gate).
- `handler_for_model.rs` gains the `RemoteRunner` arm; `plan_spawn` gains
  `placement = Provider(wgid:)`; the registry tracks the remote worker by
  `(wgid:, epoch)`; heartbeat/sweep learn the signed-renewal source.
- The **remote side** is the *existing `wg` daemon on the provider host* receiving a
  `RunGrant` and running the *existing* spawn path (`spawn_agent`, worktree,
  wrapper, heartbeat) — federation here is "two `wg` daemons, one graph of record,
  a signed bundle between them." This reuses almost the entire spawn machinery
  (doc 02 §2.2 "the richest seed").
- **Migration (v0, NFR-3/EX8):** federate execution to **a second host you own**.
  Enroll it as a `Verified` provider; place one task on it; it runs to completion;
  the result comes back signed and is eval-gated. **No confidential-compute, no
  market, no reputation machinery.** This is the milestone doc 03 NFR-3 names.

### 2.11 Maturity / risk / op-cost

- **Maturity: highest.** It is Temporal/CI/Ray's proven posture (doc 01 §2.1–§2.5)
  with `wgid:`/UCAN gating bolted on. **Op-cost: lowest** (no TEE, no N× re-run, no
  market).
- **Risk:** confidentiality is **pure trust** — a compromised or curious pool member
  reads everything (doc 01 §4.1: the universal non-TEE weakness). Bounded by:
  enrolling only machines you control; the task-scoped write UCAN (blast radius);
  and the escape hatch to C for genuinely-confidential work. **A is correct exactly
  when the trust assumption is true, and loudly refuses confidential work when it
  isn't (FR-K5).**

---

## 3. Candidate B — Capability-gated cooperative / market

> **The labor market.** Providers (`wgid:` identities) **claim authorized work from
> queues they are eligible to see**; placement is gated by **policy + reputation**;
> integrity by **re-run / quorum / eval-gate**. The pool widens from "peers you
> trust" (a cooperative) to "strangers" (an open market) by **lowering the trust
> floor** — *the same mechanism, a tighter leash* (FR-P4). This is the §1.6 engine
> at `provider_trust ∈ {Provisional, Unknown}`.

### 3.1 Placement & scheduling (HQ3)

- **Pull-claim by default** (the inversion of A's push): eligible providers **poll
  a shared queue** of authorized, unassigned work and claim what they can run — the
  CI/Buildkite/GitLab/Temporal/Akash model (doc 01 §2.1/§2.3/§2.5). This is
  decentralization-friendly (idle providers self-serve, FR-P3) and is the natural
  lift of WG's existing `wg claim` (doc 02 §2.4, "already an abstraction over who is
  allowed to run a task"), now capability-gated (§1.7).
- **Matching is filter + rank** (FR-P2/P5): filter by capability + trust floor +
  required isolation class; **rank** eligible providers by cost / latency / model
  freshness / **reputation** (FR-P5) — the Akash reverse-auction / bid→lease shape
  (doc 01 §2.5).
- **Anti-cherry-picking** (doc 03 HQ3, the "grab cheap tasks, starve hard ones"
  problem): **dispatcher-mediated assignment** for hard/sensitive tasks (push them,
  don't pool them); free-claim only for the fungible middle; lease/priority caps so
  a provider can't hoard. Stated stance: **hybrid — pull from the shared queue for
  the common case, push for the tasks where fairness/sensitivity matters.**
- **Decentralization (HQ10):** a shared **queue + provider directory may be central
  (convenience)** — but it is a *hint*: verification (sig check), the trust dial,
  and result acceptance are local and **never depend on the central queue's
  honesty** (a forged queue entry is just a `PlacementOffer` whose signature is
  checked). No correctness-critical central node (NFR-6, WG-Fed HQ6).

### 3.2 Provider identity, trust & reputation (HQ4)

- Providers are `wgid:` **cooperative peers or strangers** at `Provisional` /
  `Unknown` trust (FR-R1/R2). Trust source spans the doc 03 HQ4 axis: **WG-Fed
  web-of-trust / vouching** (cooperative) → **earned reputation** (market).
- **Reputation accrues from observed behaviour** (FR-R3): eval-gate pass-rate,
  lease/liveness record, **integrity-check (re-run/quorum) outcomes**. It is
  **per-authorizer-local by default**, optionally gossiped — **no mandatory central
  reputation ledger** (HQ10).
- **Behave-then-defect resistance** (the classic reputation attack, doc 03 HQ4): is
  **structural, not reputational** — the verification leash (§3.5) is applied to
  low-trust *and* high-sensitivity placement **regardless of accrued reputation**,
  so reputation buys *cheaper verification on fungible work*, never *unverified
  trust on sensitive work*. Trust is **slow-to-earn, fast-to-lose** (one
  integrity-check failure or reclaimed lease sharply down-weights).
- **Capability-advertisement integrity (FR-R4):** ads are signed; a provider that
  advertised `claude:opus` but ran something else is caught by **signed-ad vs
  signed-result mismatch** (the model/handler in the `ResultEnvelope` must match the
  ad) — after-the-fact detection, the honest limit absent attestation (which is C).

### 3.3 Capability flow to the worker (HQ5)

- **Leash tightened by policy** (FR-C4, §1.6). The worker gets a **short-TTL,
  narrowly-scoped** act-as-agent UCAN (per-task, minutes-to-hours) + a graph-write
  UCAN scoped to **just** `log + artifact + done` for T (no subtask-create unless
  the task needs it). **Lease-coupled:** the delegation is valid *only while the
  lease is held* (ties HQ6) — reclaim revokes it.
- **Privileged ops go through the callback** (§1.3): the worker holds **no standing
  signer** and **no long-lived secret** — anything privileged (reading a secret,
  writing outside T) is a **remote request to the authorizer**, keeping authority
  off the (untrusted) provider entirely (FR-K2). A leaked worker credential's blast
  radius is **one task, for minutes** (FR-V4).

### 3.4 Context confidentiality — THE crux (HQ1)

- **Minimal-context, no plaintext secrets — and honest about what remains.** B's
  confidentiality lever is **minimization** (FR-K3), *not* hiding: the provider is
  shipped the **smallest slice that lets it work** (`ContextScope = Task` or
  `Clean`), so a curious/hostile provider's blast radius is bounded by *what it must
  see to do the job* (doc 03 HQ1 minimization axis). **The provider CAN read the
  slice it is given** — B does not pretend otherwise (that pretence is exactly the
  trap doc 01 §4.1 warns against). For data that *must not* be readable by the
  runner, **B refuses placement** and escalates to C (next).
- **Secrets are never shipped to a market provider** (FR-K2): the worker uses the
  authorizer's model access via **remote inference / callback**, or the provider
  funds its **own** inference (it never receives the authorizer's key). An
  inspection of the provider's disk/RAM yields **no authorizer key** and **no
  long-lived credential** — only the short-TTL, task-scoped UCAN.
- **Loud degradation (FR-K5):** a `confidential` task offered only B-class
  providers is **held** ("no eligible confidential provider"), never run with its
  context exposed. The threat-model table (FR-K1) for B says plainly: *provider
  sees the minimized slice; secrets never; suitable for non-confidential work
  only.*

### 3.5 Result integrity — the co-crux (HQ2)

- **The full verification menu, selected by trust (FR-V2/V5)** — this is where B
  earns its keep, because the runner may be hostile (doc 02 §6, the headline
  adversary). Levers, cheapest → strongest:
  1. **Attribution + eval-gate** (as A) — the floor, always applied.
  2. **Deterministic re-run of the *checkable artifacts*** — *not* the agent
     transcript. This is **the single most important integrity insight from doc 01
     §4.2**: LLM output is **not** bitwise-reproducible (sampling, tool/model
     drift), so the volunteer-grid/blockchain re-run playbook **does not transfer to
     the transcript**. It *does* transfer to the task's **deterministic sub-units**
     — `cargo build`, `cargo test`, the file diff applies, a tool's output hash
     (doc 01 §2.4 BOINC, §2.5 Bacalhau content-addressed re-run). WG's task graph
     *already culminates in checkable artifacts* (doc 01 §4.2 path 2), so integrity
     **attaches to the verifiable artifact, not the chat**. Equivalence =
     "tests pass / diff applies / hash matches," not byte-equality of the agent's
     words.
  3. **N-of-M quorum** — for high-value low-trust placement, dispatch to **N
     independent providers** and accept on **agreement of the checkable artifacts**
     (BOINC redundant computing, doc 01 §2.4). Honest-majority assumption, **N×**
     cost — reserved for when it is worth it (FR-V5, the EX6 dial applied to
     integrity).
- **Evidence (FR-V3):** the `ResultEnvelope` carries the diff/artifacts + token/cost
  accounting (+ optional transcript) so the eval-gate / a re-runner can judge —
  "not just a done-bit." `wg show <remote task>` is not bare.
- **Blast-radius bound (FR-V4):** even a fully-believed forged result is capped by
  the task-scoped write UCAN and is auditable/revocable; the worst a forger does is
  corrupt *its own task* (caught by re-run/quorum/eval), never escalate.

### 3.6 Liveness, lease & reclaim (HQ6)

- The §1.5 lease with the **dial tight**: short lease term, aggressive renewal,
  no-progress reclaim. Liveness is **authorizer-observed** (accepted renewals +
  accepted partial results), robust to a provider that lies "alive" (FR-L3).
- **Fencing is load-bearing here** (FR-L2): a partitioned-but-alive market worker
  *will* sometimes return after reclaim — its stale-epoch write is **rejected**, so
  the re-placed run is authoritative and no double-commit occurs. **Squatting**
  (DoS-by-lease-hoarding) is bounded by lease caps + no-progress reclaim + a
  **reputation penalty** (FR-R3) that down-weights the squatter.

### 3.7 Data/context locality + isolation (HQ7, HQ8)

- **Locality:** the provider **pulls** a signed, **sealed**, content-addressed
  **minimal** bundle (least-at-rest, decentralization-friendly — doc 03 HQ7 pull
  axis). At rest the slice is **encrypted** and **minimized**; disposal is **not
  trusted** — confidentiality does **not** rely on the untrusted provider deleting
  it (doc 03 HQ7: "an untrusted provider's goodwill is worth nothing"). The defense
  is *ship little*, accept it may persist, and ship *nothing confidential* (that's
  C).
- **Isolation (FR-D4):** floor is **container**, **microVM** (Firecracker, doc 01
  §2.5 E2B/Daytona) preferred for a stranger — strong host-from-agent protection
  *and* co-tenant isolation. **Self-advertised** (FR-R4) and, absent attestation,
  **a claim you cannot verify** (doc 03 HQ8) — which is precisely why
  *confidential* work cannot trust B's isolation and must go to C. **Network egress
  allow-listed** (limit exfiltration of the minimal slice).
- **Both directions (FR-D5):** host-from-agent (the provider's concern, microVM) +
  agent-from-co-tenant (the authorizer's concern, microVM isolation) — but
  **agent-from-provider-operator is *not* solved** by B's isolation (the operator
  owns the host); that gap is the explicit reason C exists.

### 3.8 Economics / budget (HQ9)

- **"Who pays" becomes real** (FR-E1): three named models — **authorizer-funded**
  (worker uses authorizer's inference via callback; authorizer's creds never leave
  home), **provider-funded + billed-back** (provider uses its own model access,
  invoices), or a **pre-paid/escrowed pool** (cooperative). v1 implements
  authorizer-funded (cleanest confidentiality posture); the others are specified.
- **Budgets/ceilings (FR-E2, R32):** enforced as in A, *and* a per-provider cap so a
  runaway/compromised provider is bounded. **Signed metering (FR-E3):** the
  provider's reported usage is signed and **reconciled against the authorizer's own
  accounting** (`parse_token_usage`), so a **padded bill is detectable** — necessary
  because the provider could inflate token claims.
- **A full market economy** (price discovery, on-chain settlement, dispute
  resolution) is a **stated v1 non-goal** (doc 03 §5 non-goal 4) — B delivers the
  *placement + verification* of a market, not its *economy*.

### 3.9 Leash setting (HQ11)

`leash(Provisional|Unknown, normal, cooperative|market, default)` → short/narrow
delegation, minimal context, container/microVM isolation, **re-run or quorum**
verification, short lease. **Cooperative** sits between A and the open-market end;
**market** is the tightest non-confidential point. Tightening from A→B is *one
config change* (lower the trust floor / widen the pool), not a code change (§1.6).

### 3.10 Concrete WG mapping + migration

- `src/providers/`: `ProviderRegistry` becomes a **directory + reputation store**;
  `placement.rs` adds the **shared claim queue** + ranked matching;
  **`wg claim` becomes the capability-gated pull** (the `Claim` envelope carries the
  provider's signed ad + a proof of eligibility, checked at the boundary before a
  `RunGrant` is issued); `verify.rs` adds the **re-run/quorum orchestrator** (it
  re-dispatches a task's deterministic sub-units, or fans N copies and compares
  artifacts).
- The shared queue is a new graph surface (authorized-but-unplaced tasks
  advertised as signed `PlacementOffer`s); reputation is a new per-provider record
  in the registry.
- **Migration (phase 2, after A):** **open pool A to cooperative peers**, then to
  strangers, by **lowering the trust floor** — the same placement protocol, a
  tighter leash, plus the re-run/quorum verifier turned on. FR-P4 in action:
  "moving a provider from my pool to a stranger changes only its trust level + the
  applied leash."

### 3.11 Maturity / risk / op-cost

- **Maturity: medium.** Strong prior art for *placement* (Akash bid→lease, CI
  pull-claim, doc 01 §2.1/§2.5) and *integrity-by-quorum* (BOINC, doc 01 §2.4) —
  but the **determinism reframing** (verify artifacts, not transcript) is WG-specific
  and the load-bearing novelty. **Op-cost: higher** (re-run/quorum is N×; running a
  directory + reputation).
- **Risk:** (1) confidentiality is **minimization-only** — B is explicitly *not for
  confidential context* and must loudly refuse it (FR-K5); (2) integrity rests on
  the artifacts being genuinely checkable and on the honest-majority assumption for
  quorum (doc 05 will attack collusion); (3) self-advertised isolation is a claim,
  not a proof (→ C). **B's honest scope: non-confidential work on semi-/un-trusted
  compute, where the result is checkable.**

---

## 4. Candidate C — Confidential compute (TEE)

> **Providers run workers in TEEs / enclaves with remote attestation, so even an
> untrusted provider cannot read the agent's context.** Integrity is **rooted in
> attestation**. C is **not a placement model of its own** — it is the
> confidentiality+integrity **unlock that rides on A's or B's placement** (doc 01
> §3: "TEEs ride on a scheduler"). It is the *only* candidate where a provider you
> do **not** trust can be given confidential context, because the hardware bars the
> operator from reading enclave memory (doc 01 §4.1: "only TEEs solve X3").

### 4.1 Placement & scheduling (HQ3)

- **Push or pull (inherits A or B), but eligibility is gated by *attestation*.** A
  provider is eligible for a confidential task **only if** it presents a valid
  **remote-attestation** for the expected agent-runtime measurement (FR-P2 extended:
  the "capability" being matched includes *attested isolation class*). Otherwise
  placement is identical to A (push to a known attesting provider) or B (pull from a
  queue, filtered to attesting providers).
- **Decentralization (HQ10):** the **attestation root is a hardware vendor**
  (Intel/AMD/AWS — doc 01 §2.6), a **non-WG trust dependency that is explicitly
  flagged** (doc 03 HQ1 "where attestation roots" axis). A WG-trusted
  **attestation-verification service** *may* be central (convenience), but
  verification is a **local check of a signed quote** — like a signature check, it
  is **never correctness-central** (WG-Fed HQ6). No mandatory WG central node.

### 4.2 Provider identity, trust & reputation (HQ4)

- **Attestation substitutes for reputation.** A provider at `Unknown`
  reputation-trust becomes eligible for *confidential* work the moment it attests —
  because the trust is rooted in the **hardware**, not the operator's track record.
  The attestation quote is **bound to the provider's `wgid:`** (the quote includes a
  nonce + the provider's key, so the attested enclave is provably *this* provider's,
  defeating attestation-relay — a doc 05 attack to pre-empt).
- This is the iExec model (doc 01 §2.5/§4.1): **"untrusted market + TEE
  confidentiality + attested integrity"** — the single closest exemplar of WG's
  confidential target.

### 4.3 Capability flow to the worker (HQ5)

- **The UCANs and the context-decryption key are *sealed to the attestation*** — the
  **Nitro Enclaves → KMS pattern** (doc 01 §2.6/§4.1): the authorizer releases the
  context-decryption key (and the scoped UCANs) **only if the attestation PCRs match
  the expected measured runtime**, encrypted to the enclave's attestation-reported
  public key. So the worker gets its scoped authority **only inside the enclave**;
  the **operator outside the TEE cannot extract it** even with root on the host.
- The two UCANs are otherwise identical to §1.3 (scoped, expiring, attenuating) —
  C changes *where they can be decrypted*, not their shape (NFR-4 reuse).

### 4.4 Context confidentiality — THE crux (HQ1) — *the decisive answer*

- **Attested isolation: the operator runs the agent but provably cannot read its
  memory.** This is THE crux's only practical solution on genuinely untrusted
  compute (doc 01 §4.1, ★★★). Threat-model table (FR-K1) for C: the **provider sees
  ciphertext + attestation metadata only**; the agent's context is plaintext
  **only inside the TEE**, whose memory is barred to the host OS / hypervisor /
  operator (SEV-SNP memory encryption, TDX trust-domain, SGX enclave, Nitro vsock —
  doc 01 §2.6).
- **Secrets sealed to attestation** (FR-K2): never in long-lived plaintext on the
  provider; an inspection of the provider's disk/RAM yields **ciphertext**. This is
  the strongest possible answer to "an inspection of the provider's disk/RAM does
  not yield the authorizer's keys."
- **The attestation slot is fully realized** (FR-K4): the handshake is
  `authorizer → nonce → provider → signed quote(measurement, wgid:, nonce) →
  authorizer verifies quote against the expected measurement + a trusted vendor root
  → releases sealed context`. C *uses* enclave/attestation primitives; it **does not
  build a TEE / attestation service** (doc 03 §5 non-goal 2 — we design the slot and
  the handshake, not the silicon).
- **Loud degradation (FR-K5):** no valid attestation → the context-decryption key is
  **never released** → the task **cannot run** there. Confidentiality fails
  *closed*, by cryptography, not by policy goodwill.

### 4.5 Result integrity — the co-crux (HQ2) — *attestation of the process*

- **The attestation certifies the *process*, which is exactly right for a
  nondeterministic agent** (doc 01 §4.2, the survey's single most important fit
  insight). The quote proves *the expected harness ran the expected (pinned) model
  in a genuine enclave and produced this output* — it does **not** require the
  output to be reproducible, sidestepping the determinism wall that breaks
  re-run/quorum/zkVM for agent transcripts (doc 01 §4.2). This is the **only ★★★ X4
  that also delivers X3** and the only one needing no determinism.
- **It composes with the eval-gate** (FR-V2): attestation proves *honest execution*;
  the eval-gate still scores *quality*. The two are orthogonal and both apply.
- **Blast radius (FR-V4):** unchanged — the scoped write UCAN bounds even an
  enclave-escaping forger to its own task; attestation makes such an escape require
  breaking the hardware root, not merely lying.

### 4.6 Liveness, lease & reclaim (HQ6)

- The §1.5 lease; renewals are signed by the **attestation-bound key**, so a renewal
  proves *the genuine enclave is still alive*, not merely that some process on the
  host is. Fencing/reclaim identical.
- **Enclave ephemerality helps:** Nitro enclaves have **no persistent storage**
  (doc 01 §2.6) — a crashed enclave loses its (encrypted) state and the task is
  re-placed; nothing confidential survives the crash on the host.

### 4.7 Data/context locality + isolation (HQ7, HQ8)

- **Locality:** context **sealed-to-enclave**, decryptable **only inside** the TEE
  (§4.4); at rest **encrypted-to-enclave, never plaintext on the provider's disk**;
  disposal **guaranteed by enclave teardown** (no persistent storage) — so
  confidentiality **does not rely on the provider's goodwill to delete** (doc 03
  HQ7's hardest requirement, met by hardware).
- **Isolation (FR-D4): the top of the ladder — TEE.** It is the only class that
  delivers **both** threat directions at once (doc 01 §2.6): host-from-agent (the
  enclave/microVM contains the workload) *and* agent-from-host (the hardware bars the
  operator) — uniquely closing the agent-from-provider-operator gap B leaves open
  (FR-D5).

### 4.8 Economics / budget (HQ9)

- TEE compute is **pricier** (confidential-VM premium + attestation-verification
  overhead) — same budget/ceiling machinery (FR-E2), applied to a costlier tier.
  "Who pays" is as B (authorizer- or provider-funded). Market settlement remains a
  non-goal (doc 03 §5 non-goal 4). Signed metering (FR-E3) as B; the attestation
  additionally lets metering be *attested*, strengthening reconciliation.

### 4.9 Leash setting (HQ11) — *the dial's interesting twist*

- C **reshapes** the leash rather than only tightening it: because attestation
  substitutes for trust, the engine may grant **more context to a
  reputation-untrusted provider** than B would — `leash(Unknown + attested,
  confidential, …)` returns *full sealed context* where `leash(Unknown,
  confidential, …)` (no attestation) returns **refuse**. Attestation is a distinct
  policy input that **raises effective trust for confidentiality + integrity
  purposes** (FR-K4/FR-V2). This is the precise mechanism by which "confidential
  context on untrusted compute" becomes possible at all.

### 4.10 Concrete WG mapping + migration

- `src/providers/`: a new **`attest.rs`** submodule — verify a remote-attestation
  quote against an expected measurement + a configured vendor root; the
  **seal-to-attestation** step in `bundle.rs` (release the context key only on a
  matching quote — the Nitro→KMS flow). `placement.rs` filters by *attested*
  isolation class. The wire (§1.2) versions the **attestation-evidence format**
  (RATS/RFC 9334 shape, doc 01 §2.6) so vendor/format evolution is a loud-compat
  matter (HQ12).
- The `RemoteRunner` handler launches the agent **inside a TEE VM**
  (SEV-SNP/TDX/Nitro/Confidential-Containers — doc 01 §2.6) on the provider; WG
  **does not implement the enclave** — it ships the runtime + consumes the
  attestation (non-goal 2).
- **Migration (latest phase):** needs the attestation handshake + a TEE-capable
  provider. **v1 payload may be "trust the pool" (A) with the slot specified**
  (doc 03 FR-K4: "design the slot even if v1's payload is trust") — i.e. ship the
  `attest.rs` interface and the seal-to-attestation hook *before* any enclave
  exists, so the confidential path is a provider-capability away, not a redesign.

### 4.11 Maturity / risk / op-cost

- **Maturity: lowest** of the four — confidential computing is **emerging**
  (Confidential Containers, iExec — doc 01 §1b/§2.6); deployable but operationally
  heavy. **Op-cost: highest** — TEE hardware, attestation infra, the confidential-VM
  premium.
- **Risk:** (1) a **hardware vendor trust root** (Intel/AMD/AWS) — a non-WG, non-
  decentralized dependency, flagged (doc 03 HQ1); (2) **side-channel attack surface**
  (Foreshadow/SGAxe/ÆPIC on SGX — doc 01 §2.6); (3) **attestation-relay / TEE-spoofing**
  attacks (mitigated by binding the quote to a nonce + the provider `wgid:`, §4.2) —
  a prime doc 05 target; (4) complexity. **C is the only answer for confidential
  context on untrusted compute, and its cost is exactly that confidentiality** — so
  it is reserved (by the leash engine) for tasks whose sensitivity demands it.

---

## 5. Candidate D — Hybrid (the synthesis)

> **Trusted-private by DEFAULT (A), with an optional market tier (B) and an
> optional confidential tier (C); a placement policy picks the tier by task
> sensitivity + provider trust under the trust-default / leash-as-a-dial
> principle.** D is not a fourth mechanism — it is **the §1.6 leash engine wired to
> select A's, B's, or C's answer per task** (FR-P4: one mechanism spans
> private→cooperative→market, distinguished only by trust + the applied leash).
> This mirrors WG-Fed's own decision shape — "Candidate C deployed in a B-shaped
> default with D's UCAN grafted and A preserved" (federation/06 §1) — transposed to
> the execution plane.

### 5.1 The synthesis rule (one policy, three tiers)

The placement policy (`placement.rs::leash` + a tier selector) maps each task:

| Task sensitivity / provider trust available | Tier chosen | Why |
|---|---|---|
| normal work, **trusted** provider available | **A** (trusted pool) | the slack default — cheapest, no machinery (EX6) |
| normal work, only **semi-/un-trusted** providers (overflow / scale) | **B** (market, verified) | reach for idle compute; tighten leash + verify artifacts |
| **confidential** work, only untrusted providers | **C** (attested) | the only way to keep context secret on borrowed compute |
| **confidential** work, **no** attesting provider available | **refuse / hold** (FR-K5) | confidentiality degrades loudly, never silently |

- **Default is A** (trust-default, EX6): a task runs on your trusted pool unless a
  reason (no trusted capacity, or a sensitivity/trust mismatch) moves it. Tightening
  to B or escalating to C is **policy-driven, per-deployment config, no code change**
  (§1.6) — a solo hobbyist may only ever use A; a scale-out org enables B; a
  privacy-sensitive org enables C.
- **The leash is coherent across all five dials** (§1.6) because it is **one
  function**: choosing tier B doesn't just change placement, it *simultaneously*
  narrows the UCAN, shrinks the context slice, raises the isolation floor, deepens
  verification, and shortens the lease — no five-inconsistent-thresholds bug
  (doc 03 HQ11).

### 5.2 How D answers each HQ (by selection, with the synthesis delta)

D inherits A/B/C's per-HQ answers *by tier* and adds the **selection** logic:

- **HQ1 confidentiality:** trust (A) ↔ minimize (B) ↔ attest (C), chosen by
  `task_sensitivity`. The **whole confidentiality spectrum** is available; the
  policy guarantees a confidential task never lands plaintext on an untrusted
  provider (it gets C or is held).
- **HQ2 integrity:** attribution+eval (A) ↔ +re-run/quorum (B) ↔ +attestation (C),
  chosen by `provider_trust`. Verification cost is **proportional to trust** (FR-V5)
  — the EX6 dial applied to integrity, automatically.
- **HQ3 placement:** push (A) ↔ pull-claim (B) ↔ either-gated-by-attestation (C);
  the selector decides per task. Hybrid push/pull (FR-P3) is the *defined* default.
- **HQ4 trust/reputation:** the single dial (§1.1) feeds the selector; attestation
  (C) is a distinct input that raises effective trust for confidential purposes
  (§4.9).
- **HQ5 capability flow:** the two UCANs (§1.3), scope/TTL set by `leash()` per tier
  — broad/long (A) → short/narrow (B/C). One delegation format, five-dial-coherent.
- **HQ6 liveness:** the one §1.5 lease, term set by `leash()` — long (A) → short
  (B/C). Fencing always on.
- **HQ7 locality / HQ8 isolation:** push-plaintext-container (A) ↔ pull-sealed-
  minimal-microVM (B) ↔ sealed-to-enclave-TEE (C), by tier.
- **HQ9 economics:** "you pay" (A) ↔ funded/billed/escrow + signed metering (B/C);
  budgets/ceilings always enforced (R32).
- **HQ10 decentralization:** A needs no central node; B's directory/queue and C's
  attestation-verifier are *convenience* hints, never correctness-central — the
  per-capability table (§6.2) holds across all tiers.
- **HQ11 leash:** D **is** the leash engine made the top-level dispatcher decision
  (§1.6) — the one place the five dials are set coherently.
- **HQ12 wire/compat:** the one versioned wire (§1.2) + `WG_EXEC_COMPAT_VERSION`
  spans all tiers; a tier is a set of envelope fields, not a new protocol.

### 5.3 Concrete WG mapping + migration

- `src/providers/` holds **all three tiers**; `placement.rs` gains the **tier
  selector** in front of the matcher; `plan_spawn`'s new `placement` field is the
  selector's output. **One wire, one registry, one lease, one delegation format** —
  the tiers are configuration regions, not forks (the entire point of FR-P4).
- **Migration = the phased roadmap itself** (§9): build A (v0), add B (verify +
  market), add C (attestation slot → enclave) — D is the **convergence target**, the
  shape you have once all three tiers exist and the selector is live. You are
  *always* running "D with some tiers disabled."

### 5.4 Maturity / risk / op-cost

- **Maturity: inherits each tier's** — A high, B medium, C low — but D's *own* risk
  is **the selector + leash policy** (the largest config surface to misconfigure;
  WG-Fed's C-1 finding transposed). Mitigated exactly as WG-Fed mitigates it
  (federation/06 §2.2): **fail-safe defaults** (unset sensitivity ⇒ treat as
  needing-trust ⇒ A or refuse, never silently B/market), **a strict mode**, and
  **linting the leash policy** (`wg config lint` already exists, CLAUDE.md). The
  applied leash is **surfaced** (§1.6) so a mis-set tier is visible.
- **Op-cost:** pay-for-what-you-enable — A-only is A's cost; enabling B/C adds their
  costs only for tasks that use them. **This is the synthesis the frame anticipated
  ("likely the synthesis") and the most probable input to `exec-decision`'s pick** —
  but the *decision* is doc 06's, not this document's.

---

## 6. Cross-candidate comparison

### 6.1 Spectrum placement (extends doc 01 §3)

```
 UNTRUSTED / OPEN                                                  TRUSTED / PRIVATE
 (provider = potential adversary)                                  (you own the box)

  C (TEE/attested) ─────── B (market) ──── B (cooperative) ─────────── A (trusted pool)
       │                       │                  │                          │
  attest substitutes      pull-claim,        vouched peers,            push, full trust,
  for trust; context      minimize+verify    tighter-than-A leash      broad UCAN, plaintext,
  sealed to enclave;      (re-run/quorum on   + verify                 attribution+eval-gate
  integrity by            checkable artifacts)
  attestation

  D (hybrid) = the leash engine sliding a task to the right tier by sensitivity × trust.
               Default sits at A; slides left only when a reason (overflow / confidentiality) demands it.

  Isolation ladder (FR-D4):  worktree(today) < container(A) < microVM(B) < TEE(C)
  Verification ladder (FR-V2): attribution+eval(A) < +artifact re-run < +N-of-M quorum(B) < +attestation(C)
```

### 6.2 Per-capability central/decentralized table (HQ10 — what may be central, what must not)

| Capability | Criticality | Decision (holds across A/B/C/D) |
|---|---|---|
| **Identity / signature verification** (provider, worker, result) | **CC** | **Self-certifying, never central** — local check vs the `wgid:` sigchain (WG-Fed HQ6). A forged directory/queue/quote cannot override it. |
| **Result acceptance** (attribution + verification) | **CC** | Local to the authorizer; never delegated to a central node. |
| **Attestation verification** (C) | **CC (local check)** | A local check of a hardware-signed quote against a configured vendor root — like a sig check. The vendor root is a flagged non-WG dependency; a WG attestation-*service* is convenience only. |
| **Provider directory / discovery** (B/D) | CV | A shared directory may be central; a forged entry is a signature-checked offer, harmless. Per-authorizer or gossiped also works. |
| **Shared claim queue** (B/D) | CV | Convenience; losing it degrades reach (fall back to push/A), not correctness. |
| **Reputation store** (B/D) | CV | Per-authorizer-local by default; optional gossip; **no mandatory central ledger**. |
| **Budget / metering reconciliation** | CV (authorizer-local) | The authorizer's own `parse_token_usage` is the source of truth; the provider's signed metering is *reconciled against* it, not trusted over it. |

**No correctness- or security-critical capability depends on a single central
node** (NFR-6, WG-Fed HQ6). The private-pool case (A/D-default) works with **zero**
central nodes.

### 6.3 Decision-relevant deltas (for doc 06)

| Dimension | A trusted pool | B market | C confidential | D hybrid |
|---|---|---|---|---|
| **Confidentiality (HQ1)** | trust (provider sees all) | minimize (sees the slice) | **attest (sees ciphertext)** | per-tier by sensitivity |
| **Integrity (HQ2)** | attribution + eval | + re-run/quorum on artifacts | **+ attestation of process** | per-tier by trust |
| **Placement (HQ3)** | push (pull optional) | pull-claim (push for sensitive) | A/B gated by attestation | selector chooses |
| **Untrusted provider OK?** | no (trust required) | non-confidential only | **yes, even for confidential** | yes, routed to the right tier |
| **New trust machinery** | minimal | reputation + verify | attestation + seal-to-quote | all, gated by policy |
| **Maturity** | ★★★ highest | ★★ medium | ★ emerging | inherits per tier |
| **Op-cost** | lowest | N× verify | TEE premium | pay-per-tier-enabled |
| **v0 reachable today?** | **yes** (NFR-3) | after A | needs TEE (slot now) | = the phased path |

---

## 7. Resolution of the doc 03 §4 cross-cutting tensions

Each tension is *resolved* (a side taken), not wished away.

| # | Tension | Resolution |
|---|---|---|
| **T1** | Confidentiality vs reach/cost | **Tiered (D).** Don't pay TEE cost for non-confidential work (use A/B); pay it *only* when sensitivity demands confidentiality on untrusted compute. The leash engine (§1.6) makes the trade per-task, not globally. |
| **T2** | Integrity vs cost | **Proportional to trust (FR-V5).** Trusted-pool results accepted on attribution (cheap, A); re-run/quorum reserved for low-trust (B); attestation when confidential (C). Never re-run a trusted result needlessly. |
| **T3** | Open-market reach vs trust gating | **One mechanism, trust as the only difference (FR-P4).** Widening the pool = lowering the trust floor = a tighter leash + deeper verification, automatically. Reach is *available*, its cost is *priced into the leash*. |
| **T4** | Trust-default broad authority vs blast radius | **Slack default + a hard floor.** Broad/long UCANs by default (EX6, A) — but graph-write is **always** task-scoped (FR-C2, even in A), so the worst a leaked/hostile worker does is corrupt its own task (FR-V4). The leash *cannot* emit blanket write (§1.6). |
| **T5** | Push control vs pull autonomy | **Hybrid, stated default (FR-P3).** Push within the trusted pool (A) + pull from the shared queue (B); push the hard/sensitive tasks, pool the fungible ones (§3.1 anti-cherry-pick). |
| **T6** | Minimal context vs agent effectiveness | **Sensitivity-driven.** Generous context on trusted providers (A — effectiveness wins); minimized slice only when the provider is less trusted (B) — where the effectiveness cost buys a smaller blast radius. The `ContextScope` dial already exists (`context_scope.rs:17`). |
| **T7** | Liveness/reclaim vs double-execution | **Prefer-liveness + fencing (FR-L2).** Reclaim aggressively on a missed signed renewal; the **lease epoch** makes a late partitioned worker's write rejectable, so fast reclaim is safe (§1.5). |
| **T8** | Central scheduler efficiency vs decentralization | **Decentralized-leaning, central allowed as a hint (§6.2).** A directory/queue may be central (convenience); verification + acceptance are never (WG-Fed HQ6). The private-pool case needs no central node. |
| **T9** | Verification needs evidence vs confidentiality hides it | **Verify the *checkable artifact*, not the hidden transcript (doc 01 §4.2); or let the *attestation* be the evidence (C).** The eval-gate/re-runner sees the diff/tests (which must be visible to be useful), not the confidential reasoning; on C, the attestation vouches for the process without exposing context. |
| **T10** | One-mechanism-spans-all-pools vs per-pool optimization | **One mechanism (FR-P4), tiers are config regions (D).** The wire, delegation, lease, and bundle are shared (§1); a "pool" is a trust-floor + leash setting, not a bespoke protocol. Per-pool optimization is a ranking/policy knob, not a fork. |

---

## 8. Hard-question coverage matrix + acceptance checklist

### 8.1 All 12 HQs × 4 candidates (the validator's one-glance check)

| HQ | A — trusted pool | B — market | C — confidential | D — hybrid |
|---|---|---|---|---|
| **HQ1** confidentiality (CRUX) | trust; provider sees all; loud-refuse confidential | minimize slice; no secrets; refuse confidential | **attest; ciphertext-only to host; sealed-to-quote** | per-tier by sensitivity; never silent-expose |
| **HQ2** integrity (co-crux) | attribution + eval-gate | + artifact re-run / N-of-M quorum | **+ attestation of the process** | per-tier by trust (FR-V5) |
| **HQ3** placement | push (pull opt.) | pull-claim + push-for-sensitive | A/B gated by attestation | selector (hybrid default) |
| **HQ4** identity/trust/rep | `wgid:`, manual Verified | `wgid:`, earned rep, signed ads | attestation substitutes for rep | single dial + attest input |
| **HQ5** capability flow | broad/long UCANs, never root | short/narrow, callback for priv-ops | UCANs sealed to attestation | scope/TTL by `leash()` |
| **HQ6** liveness/reclaim | §1.5 lease, relaxed | §1.5 lease, tight, fencing-critical | attestation-bound renewals | one lease, term by tier |
| **HQ7** context locality | push, plaintext-at-rest (trusted) | pull, sealed+minimal, no-trust-delete | sealed-to-enclave, HW disposal | per-tier |
| **HQ8** isolation | container | microVM (self-advertised) | **TEE (attested, both directions)** | floor by tier |
| **HQ9** economics | "you own it, you pay" | funded/billed/escrow + signed meter | + attested metering | budgets always (R32) |
| **HQ10** decentralization | no central node | directory/queue = convenience | attest-verify local; vendor root flagged | §6.2 table holds |
| **HQ11** leash | the slack default itself | tightened by policy | reshapes (attest raises trust) | **is** the leash engine (§1.6) |
| **HQ12** wire/compat | §1.2 wire + `WG_EXEC_COMPAT_VERSION` | same wire | + versioned attestation evidence | one wire spans tiers |

### 8.2 Doc 03 §6 acceptance checklist (does the candidate set clear it?)

- [x] **HQ1 (confidentiality — THE crux) answered concretely per trust class** —
      per-class threat-model statement (A trust / B minimize / C attest); secret
      handling (§2.4/§3.4/§4.4); minimal-context (`ContextScope`, FR-K3); the
      attestation slot (§4.4, designed even if v1 payload = trust, FR-K4); loud
      degradation (FR-K5, §5.1 refuse-row). *No hand-waving: B states plainly the
      provider sees the slice; C states the operator sees only ciphertext.*
- [x] **HQ2 (integrity — co-crux): attribution + a trust-selected menu** (eval /
      artifact re-run / quorum / attestation), bounded forging provider (FR-V4),
      **with the non-determinism reframing** (verify checkable artifacts, not the
      transcript — doc 01 §4.2). §2.5/§3.5/§4.5.
- [x] **Placement & scheduling (HQ3):** push/pull default stated per tier; the
      private→cooperative→market spectrum via **one trust-gated mechanism** (FR-P4,
      §1.6/§5.1). FR-P1–P5.
- [x] **Provider identity/trust/reputation (HQ4):** `wgid:` + `trust_level` as the
      single dial (§1.1), reputation observed + behave-then-defect-resistant
      (structural, §3.2). FR-R1–R4.
- [x] **Capability flow (HQ5):** two scoped UCANs, never root (§1.3); scope/TTL/
      revocation; leash-scaled (FR-C4). FR-C1–C5.
- [x] **Liveness/reclaim across trust (HQ6):** signed lease, fencing epoch vs
      double-execution (§1.5). FR-L1–L4, NFR-1.
- [x] **Data/context locality (HQ7):** the context bundle (§1.4); transit sealed
      (FR-D2); at-rest per trust class, no reliance on untrusted goodwill (FR-D3).
- [x] **Isolation ladder (HQ8):** worktree < container < microVM < TEE, per-trust
      minimum, both threat directions (§6.1, FR-D4/D5).
- [x] **Economics/budget (HQ9):** payment model named per tier (FR-E1); budgets/
      ceilings via existing accounting (FR-E2, R32); **market economy deferral
      explicit** (§3.8, non-goal 4). FR-E1–E3.
- [x] **Decentralization vs central (HQ10):** per-capability table (§6.2); no
      correctness-critical central node; private-pool needs none. NFR-6.
- [x] **Leash policy engine (HQ11):** one coherent environment→five-dials function,
      slack by default, config-tightened (§1.6). EX6.
- [x] **Versioned wire + compat (HQ12):** `WG_EXEC_COMPAT_VERSION`, loud-fail,
      authenticated handshake (§1.2). NFR-5.
- [x] **Substrate reuse (NFR-4/EX5):** no second identity/delegation/crypto/trust
      system — every credential/encryption/trust primitive maps to a named WG-Fed
      requirement (§1.1/§1.3/§1.4 all cite WG-Fed). *Confirmed.*
- [x] **Phased rollout (NFR-3/EX8):** v0 = A on a second host you own → B → C/D (§9).
- [x] **Each §4 tension resolved** (§7).
- [x] **Inside the §5 non-goals** (§10).

*No MUST requirement is left unmet. The one MUST that is **scoped rather than
universally met** is FR-K (confidentiality) on Candidate B: B **cannot** keep
context secret from the provider (it has no TEE) — and it does **not pretend to**,
it **loudly refuses** confidential placement (FR-K5) and defers it to C. That is a
correct, stated answer, not a silent gap.*

---

## 9. Migration phasing (v0 → … ; candidate choice is a late binding)

Deliberately **candidate-agnostic through Phase 1** (the wire, the lease, the two
UCANs, the bundle are shared §1 substrate proven before the topology hardens), then
each tier is an independently-valuable add (NFR-3/NFR-6). This **co-sequences with
the WG-Fed roadmap** (doc 02 §5: execution federation is a *consumer* of WG-Fed and
cannot precede its identity/UCAN substrate — federation/06 §5 Waves 2–6).

```
WG-Fed Waves 2–6 (identity, UCAN, transport)  ──┐  (hard prerequisite, doc 02 §5)
                                                 ▼
 Phase 0 ── Phase 1 ──► Phase 2 (Candidate A) ──► Phase 3 (Candidate B) ──► Phase 4 (Candidate C) ──► Phase 5 (Candidate D)
 wire+UCAN  bundle+lease  trusted pool, 2nd host    market: directory,        attestation slot →       selector live;
 skeleton   over network  you own; attribution      reputation, re-run/       seal-to-quote;           "D with tiers
 (no topo)               + eval-gate (v0)           quorum on artifacts       TEE runner               enabled"
```

- **Phase 0 — substrate skeleton.** `src/providers/mod.rs` with
  `WG_EXEC_COMPAT_VERSION` + the wire envelopes (§1.2); the two-UCAN issuance
  (§1.3) on top of WG-Fed's `custody.rs`. No placement yet. **Dep:** WG-Fed UCAN
  (Wave 6).
- **Phase 1 — bundle + lease over a real network.** `bundle.rs` (build/seal/verify,
  reusing `src/identity/` crypto) + `lease.rs` (signed renewal + fencing epoch).
  Proven on loopback / two hosts, **no trust topology committed**.
- **Phase 2 — Candidate A (v0 milestone, NFR-3).** `RemoteRunner` handler arm
  (`handler_for_model.rs:64`), `plan_spawn` placement field, a hand-curated
  `Verified` `ProviderRegistry`; **federate one task to a second host you own**;
  result signed + eval-gated. *This is the smallest end-to-end win and the
  equivalent of WG-Fed's spark test.* A smoke scenario
  (`exec_federation_trusted_pool.sh`, manifest `owners`) is the gate.
- **Phase 3 — Candidate B.** Shared claim queue + capability-gated `wg claim` +
  reputation store + `verify.rs` re-run/quorum orchestrator. Open the pool by
  lowering the trust floor.
- **Phase 4 — Candidate C.** `attest.rs` + seal-to-attestation in `bundle.rs`; the
  TEE runner. **The slot ships in Phase 2** (interface only); the enclave lands
  here.
- **Phase 5 — Candidate D.** The tier selector in `placement.rs` + fail-safe
  defaults + leash lint + `wg providers` leash surfacing. The convergence target.

**Don't-build-yet guardrails** (mirroring federation/06 §5): never ship B/market
placement without the verification leash wired (a low-trust result must be
verifiable before accept); never ship C's confidential path without the
**attestation bound to the provider `wgid:` + nonce** (anti-relay); never ship D's
selector without **fail-safe defaults** (unknown sensitivity ⇒ needs-trust ⇒ A or
refuse, never silent-market) + the leash lint; don't pick the TEE vendor or the
queue/transport library before Phase 3 informs it.

---

## 10. Non-goals honored (doc 03 §5)

1. **No second identity/delegation/keys/encryption/trust system** — all from WG-Fed
   (§1.1/§1.3/§1.4 cite it; NFR-4). ✓
2. **We do not build a TEE / attestation service** — C designs the *slot* + the
   handshake and *consumes* attestation (§4.4/§4.10). ✓
3. **No homomorphic / compute-on-ciphertext** — confidentiality levers are trust /
   minimization / attested isolation only (doc 01 §4.1 rates FHE/MPC impractical). ✓
4. **No token/blockchain compute marketplace with on-chain settlement** — B delivers
   placement + verification, **not** the economy; budgets/ceilings + payment-model-
   naming only (§3.8, FR-E*). Deferral **stated, not silent**. ✓
5. **No general-purpose / arbitrary compute** — this federates *WG agent-task
   execution*, not batch jobs. ✓
6. **Provider brings its own handler/model** — surfaced only as a signed capability
   ad (FR-R4); we don't design inference hosting. ✓
7. **No real-time scheduling** — work-speed, seconds-to-minutes (NFR-2). ✓
8. **We don't re-implement the local dispatcher/spawn/claim** — they are the
   migration substrate, extended not replaced (§1.7, doc 03 §5 non-goal 8). ✓
9. **We don't pick the final library/wire/TEE-vendor stack** — §9 keeps it a late
   binding. ✓
10. **No multi-tenant fairness/QoS at scale** — v1 = private → cooperative; large-
    scale anti-cherry-pick auctions flagged (§3.1), not built. ✓

---

## 11. Handoff to doc 05 (adversarial) and doc 06 (decision)

- **For `exec-adversarial` (5/6) — the headline targets to attack:**
  - **The malicious provider returning a plausible-but-forged result (the co-crux).**
    Attack each integrity tier: A's attribution-only accept (a trusted-but-compromised
    pool member); B's quorum under **collusion** (N colluding providers agree on a
    lie) and the determinism reframing (can the "checkable artifact" itself be
    forged?); C's **attestation-relay / TEE side-channel / measurement-spoof**.
  - **The curious/hostile provider reading context (the crux).** Attack B's
    minimization (is the minimal slice still too revealing?); confirm A's trust
    assumption is the whole ballgame; probe C's seal-to-attestation for a key-release
    bypass.
  - **Liveness adversaries:** the squatter (hold lease, no work), the partition
    double-committer (does fencing truly hold?), the lying-heartbeat provider.
  - **Capability escalation:** can a worker exceed its task-scoped graph-write UCAN?
    Is the callback-for-privileged-ops boundary tight?
  - **The leash engine itself (D):** can a misconfiguration silently route a
    confidential task to B/market? (The fail-safe-default + lint is the claimed
    defense — break it.)
- **For `exec-decision` (6/6):** §6.3's delta table + §8.1's matrix are the
  decision inputs. The frame's "**likely the synthesis**" hypothesis (D) and the
  WG-Fed precedent ("C deployed B-shaped with D's UCAN, A preserved", federation/06
  §1) both point at **D with A as the v0 default and C's slot specified early** —
  but the pick, the per-HQ decision register, and the spark/milestone definition are
  **doc 06's to make**, defended against this set using doc 05's findings. This
  document deliberately stops at *proposing the spanning set*.

---

*Wave-1 generate phase (execution federation) complete. Four candidate
architectures — A trusted-private pool, B capability-gated market, C confidential
compute, D hybrid synthesis — span the trust/openness spectrum, each answering all
twelve doc-03 hard questions concretely (no hand-waving on the confidentiality or
integrity cruxes — B loudly refuses what it cannot hide; C seals context to
attestation; integrity attaches to checkable artifacts, not the nondeterministic
transcript), each composing with the WG-Fed `wgid:`/UCAN/trust substrate
(federation/06) without inventing a second one, and each mapped to concrete WG code
changes (a new `src/providers/` module, a `RemoteRunner` handler arm, a placement
field on `plan_spawn`, a capability-gated `wg claim`, a cross-host signed lease, the
leash policy engine) on a phased migration from today's single-machine model. The
cross-cutting tensions are resolved, the central/decentralized boundary is drawn so
no correctness-critical capability is central, and the set is handed to the
adversarial pass and the decision memo.*
