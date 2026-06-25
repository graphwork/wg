# Execution Federation Study 3/6 — Requirements & Hard-Questions Catalog

> **Execution-federation study, wave 1, task 3 of 6 (gather phase).**
> This is the **"what must be true"** spec for federating the *execution plane*.
> The candidate architectures (task 4, `exec-architectures`) are judged against
> the requirements here; the adversarial pass (task 5, `exec-adversarial`)
> attacks the answers proposed for the hard questions (especially the malicious
> provider / result-forgery cruxes); the decision memo (task 6, `exec-decision`)
> picks and roadmaps. Prior-art (task 1, `exec-prior-art`) and current-state
> baseline (task 2, `exec-baseline`) feed in alongside this document — it is
> written to **stand on its own from the frame**, not to depend on them.

**Status:** draft for evaluation · **Date:** 2026-06-25 · **Owner task:** `exec-requirements`

---

## 0. How to read this document

WG fuses three things on one machine: the **graph** (durable task/agent state),
the **authority** to dispatch (who may turn a task into a running agent), and the
**compute** that actually runs the agent. The headline federation study
(`docs/federation-study/*`, "WG-Fed") federates **state + authority + identity**.
**This study owns the fourth seam: the execution plane** — taking a task that has
*already been authorized* and placing its agent onto **compute that need not
belong to the authorizer**, gated by trust and capabilities, returning results
that are *verifiable*, while keeping the agent's working context *confidential*.

Four artifacts, in order of decreasing stability:

1. **§2 Requirements** — the durable contract. Numbered `FR-*` (functional) and
   `NFR-*` (non-functional), each with **MUST / SHOULD / MAY** force (RFC 2119),
   a **trace** back to the frame (execution pillars `EX*`, or the gap analysis),
   a one-line rationale, and an **acceptance signal** (how task 4/5 can tell it
   was met). These change slowly.
2. **§3 Hard-Questions catalog** — the unresolved design forks. Twelve questions
   (`HQ1`…`HQ12`), the ten from the brief plus two the requirements surfaced as
   genuinely-distinct forks, each with **why it's hard**, the **decision axes**
   (the spectrum of real choices), and **success criteria a good answer must
   satisfy**. These are *open*; task 4 proposes, task 5 stress-tests, task 6
   decides. **HQ1 (context confidentiality) and HQ2 (result integrity) are the
   load-bearing cruxes** and are placed first.
3. **§4 cross-cutting tensions**, **§5 non-goals**, **§6 architecture acceptance
   checklist**, **§7 traceability matrix** — the connective tissue that makes the
   above usable downstream.

**Traceability convention.** Every requirement cites at least one source:
- **Execution pillars** `EX1…EX8` (defined in §1) — the north star distilled
  from the study frame (Erik's execution-federation model; the brief for this
  wave).
- **WG-Fed substrate** — the identity/key/delegation/trust layer this study
  **reuses and must not reinvent** (`docs/federation-study/03`,`04`). Cited as
  `WG-Fed FR-*` / `WG-Fed HQ*` where a specific federation requirement or
  decision is the load-bearing dependency.
- **Gap-analysis reqs** `R*` — from the private `poietic-pbc/poietic-family-team`
  gap analysis. Only **R32** (budgets/ceilings) is cited by number, because it is
  named in this study's frame; others are cited as "(gap-analysis)" because the
  exact `R#` lives in the source repo and must not be invented.
- **Current code** — where a requirement is shaped by what exists today
  (the dispatcher / `wg service start`, `wg spawn` worktree isolation, `wg
  claim` / `wg reclaim`, the handler routing in `src/dispatch/handler_for_model.rs`,
  `WG_AGENCY_COMPAT_VERSION`, token accounting via `graph::parse_token_usage`).
  Detailed baselining is task 2's job; cited here only for grounding.

---

## 1. The north star, restated as named execution pillars

These are the load-bearing claims of the execution-federation frame, named so
requirements can trace to them. (Source: this study's frame; Erik's
execution-federation model.)

| ID | Pillar | One-line statement |
|----|--------|--------------------|
| **EX1** | **Compute is separable from authority** | The principal that holds the graph + dispatch authority (the **authorizer**) and the machine that runs the agent (the **provider**) need not be the same host or even the same owner. The execution plane is the seam between "I decided this task should run" and "this CPU ran it." |
| **EX2** | **Trust- and capability-gated placement** | An authorized task's agent lands on a provider **only if** that provider's trust level and advertised capabilities (model availability, sandbox, cost) satisfy the task's requirements. Placement is a *match*, not a free-for-all. |
| **EX3** | **Verifiable results** | Work that runs on **borrowed compute** returns results that are **attributable** (the worker signs as the agent) and **checkable** (re-run / quorum / WG's existing eval-gate), so a hostile provider cannot silently forge or corrupt output. *(crux — HQ2)* |
| **EX4** | **Confidential context** | Running an agent exposes its working context (task input, graph slice, secrets, prior conversation/state) to whoever owns the compute. The execution plane must let an authorizer **bound what the provider can see** — by trust, by minimization, or by attested isolation. *(crux — HQ1)* |
| **EX5** | **Reuse the WG-Fed substrate — do not reinvent identity/delegation** | Providers, agents, and authorizers are `wgid:` identities under WG-Fed's keys; the worker's authority to act is a **scoped UCAN delegation**, not a copied key; trust is WG-Fed's `trust_level`. This study **composes** that substrate and adds *placement, isolation, and result-verification* on top — it does **not** define a second identity or delegation system. |
| **EX6** | **Trust-default / leash-as-a-dial** | Authority is **broad and long-lived by default** (the trusting case is the common case — a private pool you own). **Tightening** — short-lived, narrowly-scoped capabilities; minimal context; mandatory attestation — is an **environment-driven policy dial**, applied when the provider is less trusted, **not** the birth state of every delegation. The leash exists; it is normally slack. |
| **EX7** | **Liveness across trust boundaries** | A remote provider that claims authorized work and then **dies or stalls** must not orphan the task. Leases, heartbeats, and reclaim must work **across** a trust boundary where the provider may be uncooperative or adversarial — the distributed-orphan problem. |
| **EX8** | **Incremental on today's WG** | The design extends the existing dispatcher / spawn-worktree / claim / heartbeat / handler machinery rather than replacing it. A v0 (private trusted pool) must be reachable without the full confidential-compute stack. |

**The two cruxes, called out up front.** The frame names two problems as the
hardest, and they are duals of the same fact — *running an agent on a machine you
don't own means that machine sees everything the agent sees and produces
everything the agent produces*:

- **EX4 / HQ1 — Context confidentiality on borrowed compute.** The provider can
  read the agent's context. This is **THE crux**.
- **EX3 / HQ2 — Result integrity.** The provider can lie about what the agent
  produced. This is the **co-crux**.

Every requirement and question below is arranged so these two stay visible; they
are the decisions most likely to make or break the architecture. A trusted-pool
design can make both trivial (you own the provider); an open-market design makes
both load-bearing (the provider is a stranger, possibly hostile). The leash-dial
(EX6) is precisely the knob that moves an architecture along that spectrum.

---

## 2. Requirements

Force: **MUST** (architecture is wrong without it) · **SHOULD** (strongly
preferred; deviation needs justification) · **MAY** (permitted, optional).

### 2.A Placement & scheduling

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-P1** | MUST | The execution plane **separates the authorizer from the provider**: a task authorized in WG-A can have its agent run on a provider that is a *different* host and may be a *different* owner. | EX1 | *Accept:* a task whose graph lives on host A runs to completion on host B; A never ran the agent process. |
| **FR-P2** | MUST | **Placement is gated by capability + trust matching**: a task declares what it needs (model/handler, sandbox class, cost ceiling, min provider trust); a provider is eligible only if it satisfies all of them. | EX2 | *Accept:* a task requiring `claude:opus` + `trust≥standard` is never placed on a provider lacking the model or below the trust floor. |
| **FR-P3** | SHOULD | Both **push** (authorizer/dispatcher assigns a chosen provider) **and pull** (an eligible provider *claims* authorized, unassigned work) placement are expressible; the design states which it defaults to and why. | EX1, EX8 (**HQ3**) | *Accept:* the same task can be (a) directly dispatched to provider X, and (b) left in a pool for any eligible provider to claim. |
| **FR-P4** | SHOULD | Placement supports a **pool spectrum**: a **private pool** the authorizer owns, a **cooperative** of mutually-trusting peers, and (eventually) an **open market** of unknown providers. Trust gating (FR-T*) is what distinguishes them; the mechanism is one mechanism, not three. | EX2, EX6 (**HQ3**, **HQ10**) | *Accept:* moving a provider from "my pool" to "a stranger" changes only its trust level + the applied leash, not the placement protocol. |
| **FR-P5** | MAY | The scheduler may rank eligible providers by **cost / latency / model freshness / reputation**, not only filter by eligibility. | EX2 (**HQ3**, **HQ4**) | *Accept:* given two eligible providers, the cheaper / higher-reputation one is preferred per a stated policy. |

### 2.B Capability flow (delegation to the worker)

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-C1** | MUST | A worker on a provider receives a **scoped act-as-agent capability** (a UCAN delegation: `iss`=authorizer/agent root, `aud`=provider/worker, capability="run task T as agent G", + expiry) — **never the agent's root signing key**. | EX5, EX3 (WG-Fed FR-I2/FR-S1) (**HQ5**) | *Accept:* the bytes delivered to the worker contain a delegation token, not a private key; the worker can act for the task and **cannot** mint authority outside the delegated scope. |
| **FR-C2** | MUST | A worker receives a **scoped graph-write capability** authorizing exactly the writes its task needs (log/append to task T, write artifacts under T, mark T done) — **not** blanket write authority over the whole graph. | EX5, EX2 (**HQ5**) | *Accept:* a worker delegated for task T cannot use its credential to mutate task U or another agent's record. |
| **FR-C3** | MUST | Delegations are **expiring** and **revocable**: they carry a TTL, and the authorizer can revoke before expiry so an honest worker stops being accepted. | EX6 (WG-Fed FR-S7) (**HQ5**, **HQ6**) | *Accept:* after expiry or explicit revoke, a write signed under the delegation is rejected by the authorizing graph. |
| **FR-C4** | SHOULD | Delegation **scope and lifetime follow the leash dial (EX6)**: broad/long by default for trusted providers, automatically narrowed/shortened by policy for low-trust providers. The narrowing is *policy*, not hardcoded into every issuance. | EX6 (**HQ5**, **HQ11**) | *Accept:* the *same* task issued to a trusted vs an untrusted provider yields a longer/broader vs shorter/narrower UCAN, driven by a stated policy input. |
| **FR-C5** | SHOULD | The worker's writes are **attributable to the agent identity** (signed by the worker's delegated signer, chained to the agent), so the graph records *who* produced each artifact/log even when the *compute* was someone else's. | EX3, EX5 (WG-Fed FR-M1) (**HQ2**) | *Accept:* `wg show <task>` attributes the result to agent G; the signature verifies against G's delegated signer, recorded under the authorizer's sigchain. |

### 2.C Context confidentiality — **THE crux** (HQ1)

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-K1** | MUST | The architecture **names, for each placement class, what the provider can and cannot see** of the agent's context (task input, graph slice, secrets, prior state, tool outputs). Confidentiality is a **stated, bounded property per trust level**, not an unexamined default. | EX4 (**HQ1**) | *Accept:* a threat-model table enumerates the provider's view at each trust level; nothing in the context is "secret by accident." |
| **FR-K2** | MUST | **Secrets are never shipped in the clear to an untrusted provider.** Credentials the worker needs (API keys, graph-write tokens) are delivered by a mechanism whose exposure matches the provider's trust — held by the authorizer and used via remote calls, sealed to attested hardware, or simply trusted on a private pool. | EX4, EX6 (WG-Fed FR-S1) (**HQ1**, **HQ7**) | *Accept:* on an untrusted provider, the worker never holds a long-lived plaintext root credential; an inspection of the provider's disk/RAM does not yield the authorizer's keys. |
| **FR-K3** | SHOULD | The design supports **minimal-context placement**: a task can run with only the slice of context it needs (not the whole conversation/graph), so the *blast radius of a curious provider* is bounded by what it must see to work. | EX4, EX6 (**HQ1**) | *Accept:* a task can be configured to ship only task T's input + the artifacts it depends on, not the entire graph or unrelated history. |
| **FR-K4** | SHOULD | For low-trust providers, the design offers (or designs the slot for) **attested confidential execution** (TEE/enclave attestation) as the lever that lets the provider run the agent **without** the operator being able to read its memory. The interface is specified even if the v1 payload is "trust the pool." | EX4 (**HQ1**, **HQ8**) | *Accept:* there is a defined attestation handshake by which an authorizer verifies "this agent runs in an enclave I trust" before shipping confidential context; absence of attestation downgrades to lower confidentiality, loudly. |
| **FR-K5** | MUST | **Context confidentiality degrades loudly, never silently.** If a provider cannot meet the confidentiality bar a task requires (no attestation, too low trust), the task is **not placed there** — it is refused/held, with a stated reason, rather than silently exposing context. | EX4, EX6 (**HQ1**) | *Accept:* a confidential task offered only low-trust providers is held with a "no eligible confidential provider" reason, not run with its context exposed. |

### 2.D Result integrity — the co-crux (HQ2)

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-V1** | MUST | Results from a provider are **attributed via signature** (FR-C5) so the graph can tell *which* identity/worker produced them; an unsigned or wrong-signed result is rejected. | EX3 (WG-Fed FR-M1) (**HQ2**) | *Accept:* a result lacking a valid delegated-signer signature is not accepted into the graph. |
| **FR-V2** | MUST | The architecture defines **how much to trust a provider's claimed result** as a function of trust level, and the **verification lever(s)** available to raise confidence: **re-run** (idempotent re-execution elsewhere), **quorum** (N providers, compare), and **WG's eval-gate** (the existing `auto_evaluate` / FLIP scoring of output against `## Validation`). | EX3 (**HQ2**) | *Accept:* a low-trust result can be configured to require re-run or quorum agreement before it is accepted as authoritative. |
| **FR-V3** | SHOULD | Results carry enough **evidence** (the work product itself, token/cost accounting, optionally a transcript/attestation) for the eval-gate or a re-runner to judge them — not just a "done" bit. | EX3, EX8 (current token accounting) (**HQ2**) | *Accept:* `wg show <remote task>` is not bare: it carries the artifacts + usage/cost the eval-gate scores, exactly as a local task does. |
| **FR-V4** | SHOULD | **A hostile provider cannot escalate beyond its delegation by forging results**: even a fully-believed forged result is bounded by the scoped graph-write capability (FR-C2) and is revocable/auditable after the fact. | EX3, EX5 (**HQ2**, **HQ5**) | *Accept:* the worst a forging provider can do is corrupt *its own task's* output (caught by eval/re-run), not mutate the rest of the graph or impersonate another agent. |
| **FR-V5** | MAY | Cost/verification is **proportional to trust**: trusted providers' results are accepted on attribution alone; expensive verification (re-run/quorum) is reserved for low-trust placement (the EX6 dial applied to *integrity*, mirroring FR-C4 for *authority*). | EX3, EX6 (**HQ2**, **HQ9**) | *Accept:* a trusted-pool result is not needlessly re-run; an open-market result is, per a stated policy. |

### 2.E Liveness, leases & reclaim across trust boundaries

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-L1** | MUST | Remote claims are **leased**: a provider holds a task for a bounded, renewable term; failure to renew (heartbeat) makes the task **reclaimable** by the authorizer. | EX7, EX8 (current `wg reclaim`) (**HQ6**) | *Accept:* a provider that stops heartbeating loses the lease; the task returns to ready/reclaimable after the lease term, without manual intervention. |
| **FR-L2** | MUST | Reclaim is **safe against double-execution**: when a stalled remote worker is reclaimed and the task re-placed, a *late-returning* original worker's writes are rejected (its delegation/lease is no longer valid), so two workers cannot both commit results. | EX7 (**HQ6**) | *Accept:* reclaim → re-run → the original worker wakes and tries to write → its write is refused (stale lease/expired delegation); no double-commit. |
| **FR-L3** | SHOULD | Heartbeat/liveness must work **across a trust boundary** where the provider may be uncooperative: liveness is judged by the *authorizer's* observations (missed renewals), not solely by the provider's self-report. | EX7 (**HQ6**) | *Accept:* a provider that lies "still alive" but produces nothing is still reclaimed once its lease term elapses without an accepted result. |
| **FR-L4** | SHOULD | Lease term and heartbeat cadence follow the **leash dial (EX6)**: long/relaxed for trusted providers, short/aggressive for low-trust ones. | EX6, EX7 (**HQ6**, **HQ11**) | *Accept:* the same task gets a longer lease on a trusted provider, a shorter one on a stranger, per stated policy. |

### 2.F Provider identity, trust & reputation

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-R1** | MUST | A provider is a **`wgid:` identity** under the WG-Fed key model; placement decisions are made against a verifiable provider identity, not an IP/hostname. | EX5 (WG-Fed FR-I1) (**HQ4**) | *Accept:* a provider authenticates with its key; a spoofed provider identity fails signature verification. |
| **FR-R2** | MUST | Provider **trust level** (reusing WG-Fed/`Agent.trust_level`) gates placement, leash tightness (FR-C4/FR-L4), context exposure (FR-K*), and verification depth (FR-V*). Trust is the **single dial** the whole execution plane reads. | EX2, EX6 (WG-Fed FR-T3) (**HQ4**) | *Accept:* raising/lowering a provider's trust level visibly changes what it may run, see, and how its results are verified. |
| **FR-R3** | SHOULD | Provider **reputation** accrues from observed behavior (completed tasks, eval-gate pass rate, liveness, integrity-check outcomes) and can raise/lower effective trust over time. | EX2 (**HQ4**) | *Accept:* a provider with a history of eval-gate failures or reclaimed leases is down-weighted in ranking (FR-P5) or trust. |
| **FR-R4** | SHOULD | Provider **capability advertisement** (models/handlers available, sandbox class, cost, attestation support) is itself **signed** by the provider, so capability claims are attributable and a false advertisement is detectable after the fact. | EX2, EX3 (**HQ4**) | *Accept:* a provider that advertised `claude:opus` but ran something else is caught (signed advertisement vs signed result mismatch). |

### 2.G Data/context locality, isolation & sandboxing

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-D1** | MUST | The design states **where the task input + graph-slice live** during remote execution and **how the provider obtains them** (provider pulls a signed, possibly-encrypted bundle vs authorizer pushes). | EX1, EX4 (**HQ7**) | *Accept:* there is a defined "context bundle" with a stated locality story; the provider's copy is accounted for in the FR-K1 threat model. |
| **FR-D2** | MUST | Context **in transit is encrypted and authenticated** (end-to-end, transport untrusted), reusing WG-Fed's per-recipient encryption rather than a new scheme. | EX4, EX5 (WG-Fed FR-S3/FR-S5) (**HQ7**) | *Accept:* a relay/MITM between authorizer and provider sees ciphertext; tampering is detected. |
| **FR-D3** | SHOULD | Context **at rest on the provider** has a stated protection level per trust class (plaintext on a private pool you own ↔ encrypted-to-enclave on an untrusted one) and a **disposal** expectation (context is ephemeral, removed after the task). | EX4 (**HQ1**, **HQ7**, **HQ8**) | *Accept:* the design says what persists on the provider after the task and for how long; an untrusted provider is not expected to be trusted to delete, so confidentiality does not rely on its goodwill. |
| **FR-D4** | MUST | A provider must meet a **declared isolation guarantee** for the worker (the federated successor to today's per-task git worktree): the agent runs in a bounded environment that contains its filesystem/network blast radius. The required class is stated per trust level. | EX2, EX8 (current spawn worktree isolation) (**HQ8**) | *Accept:* a provider advertises and is held to an isolation class (e.g. worktree ↔ container ↔ microVM ↔ TEE); placement requires the task's minimum class. |
| **FR-D5** | SHOULD | Isolation protects **both directions**: the worker from a hostile co-tenant/provider (confidentiality, FR-K*) **and** the provider from a hostile workload (the agent cannot escape to harm the host). | EX4, EX8 (**HQ8**) | *Accept:* the isolation class is justified against both threat directions, not only "sandbox the agent." |

### 2.H Economics & metering (largely v1-deferred — flagged, not dropped)

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **FR-E1** | SHOULD | The design **names who pays** for compute + tokens when authorizer ≠ provider (authorizer-funded via its own provider credentials, provider-funded billing back, or pre-paid pool), even if v1 implements only the private-pool case. | EX1, R32 (**HQ9**) | *Accept:* the payment model is stated per pool class; v1's "you own the pool, you pay" is explicit, not assumed. |
| **FR-E2** | SHOULD | **Budgets / ceilings** are enforceable per task/provider (R32): a remote task cannot exceed a token/cost ceiling, and a runaway provider is capped — reusing the existing token/cost accounting (`graph::parse_token_usage`, `wg spend`). | R32, EX8 (current accounting) (**HQ9**) | *Accept:* a task with a $-ceiling is halted/flagged when the metered remote spend crosses it. |
| **FR-E3** | MAY | **Metering is attributable and signed** so a cost claim from a provider can be checked against the authorizer's own accounting; disputes are detectable. | EX3, R32 (**HQ9**) | *Accept:* provider-reported usage and authorizer-side accounting can be reconciled; a padded bill is detectable. |

> **v1-deferral note (do not silently drop):** full multi-tenant billing /
> settlement / a compute *market* economy is a **non-goal for v1** (§5). What is
> *not* deferred is **budgets/ceilings (R32)** and **naming the payment model** —
> those are needed even for a private pool and are SHOULD above.

### 2.I Non-functional requirements

| ID | Force | Requirement | Traces to | Rationale / acceptance signal |
|----|-------|-------------|-----------|-------------------------------|
| **NFR-1 Reliability** | MUST | Remote execution survives provider crashes, network partitions, and reclaim (FR-L*) without losing or double-committing work. | EX7 | *Accept:* chaos test — kill/partition providers mid-task; every task either completes once or is cleanly reclaimed; no double-commit, no orphan. |
| **NFR-2 Latency budget** | SHOULD | Target **work-speed, not real-time**: placement + remote round-trips in seconds-to-minutes are acceptable. This relaxation buys decentralization and offline-tolerant providers (mirrors WG-Fed's email-speed stance). | EX1 | *Accept:* the design is not penalized for non-interactive placement latency; nothing assumes sub-second scheduling. |
| **NFR-3 Incrementality** | MUST | A **phased rollout** exists on top of today's WG: v0 = private trusted pool (trust=high, leash slack, no attestation) reachable by extending dispatcher/spawn/claim/reclaim; later phases add cooperative + open-market + confidential compute. | EX8 | *Accept:* a v0 milestone federates execution to a second host *you own* without the confidential-compute or market machinery. |
| **NFR-4 Substrate reuse (no reinvention)** | MUST | Identity, keys, UCAN delegation, per-recipient encryption, trust, and revocation come from **WG-Fed**. This study adds placement, isolation, confidentiality-policy, and result-verification — it **introduces no second identity/delegation/crypto system**. | EX5 | *Accept:* every credential/encryption/trust primitive used here maps to a named WG-Fed requirement; no new key or delegation format is invented. |
| **NFR-5 Evolvability / compat** | MUST | The execution wire (placement, lease, capability, result envelopes) is **versioned with an explicit compat handshake**, mirroring `WG_AGENCY_COMPAT_VERSION` — mismatched authorizer/provider versions fail **loudly**, never silently mis-route. | EX5, EX8 (current `WG_AGENCY_COMPAT_VERSION`) (**HQ12**) | *Accept:* a vN authorizer and vN+1 provider negotiate a shared subset or refuse loudly; no silent capability/route mismatch (the bare-`openrouter:` class of bug). |
| **NFR-6 Operability** | SHOULD | A provider is **self-hostable with modest setup**: running your own provider node (to join a private pool or cooperative) is feasible for an individual, preserving the decentralization option. | EX1, EX8 | *Accept:* a single person can stand up a provider that accepts and runs tasks on commodity hardware. |
| **NFR-7 Auditability** | SHOULD | Placement, delegation issuance/revocation, lease/reclaim, and result-acceptance are **append-only and inspectable**, so "who ran what, under whose authority, verified how" is reconstructable after the fact. | EX3, EX5 (WG-Fed NFR-7) | *Accept:* an auditor can replay a task's execution-federation history (placed→delegated→leased→returned→verified→accepted). |

---

## 3. Hard-questions catalog

Twelve questions: the ten from the brief, plus **HQ11 (leash calibration —
the policy engine behind EX6)** and **HQ12 (execution-wire evolution/compat)**,
which the requirements surfaced as genuinely-distinct forks. Each is *open* —
task 4 proposes, task 5 attacks, task 6 decides. Format: **why hard · decision
axes · success criteria**. **HQ1 and HQ2 are the load-bearing cruxes** and lead.

### HQ1 — Context confidentiality on borrowed compute **(THE crux — load-bearing)**

> *Running an agent exposes its working context to whoever owns the compute. How
> do you let an authorizer bound what a provider can see?*

- **Why hard.** The whole value of federating execution is running on compute you
  don't own (EX1) — but the agent's context (task input, the graph slice, secrets,
  prior conversation/state, every tool output) flows through that machine's RAM.
  The provider's operator can, in principle, read all of it. Encryption-in-transit
  doesn't help: the data must be *plaintext to compute on it*. This is the
  classic confidential-computing problem, and there is no free solution — your
  only levers are **trust** (run only on providers you believe won't look),
  **minimization** (ship less), or **attested isolation** (TEE/enclave: the
  operator runs the agent but provably can't read its memory), each with steep
  costs. Getting this wrong turns "borrow a friend's GPU" into "hand a stranger
  your secrets and conversation history."
- **Decision axes.**
  - *Primary lever:* trust-gating (only-trusted-providers see context) ↔
    minimal-context exposure (ship only the needed slice) ↔ TEE/attestation
    (operator can't read enclave memory) ↔ combinations along the EX6 dial.
  - *Secrets specifically:* ship plaintext to trusted pool ↔ remote-signing /
    keep-secret-at-authorizer (worker calls back for privileged ops) ↔ seal to
    attested hardware ↔ short-lived scoped tokens only (FR-C1/FR-C2).
  - *Granularity of minimization:* whole graph ↔ task subtree ↔ task + explicit
    dependency artifacts ↔ redacted/transformed context.
  - *Failure stance:* refuse-if-not-confidential (FR-K5, loud) ↔ run-degraded.
  - *Where attestation roots:* hardware vendor root ↔ a WG-trusted attestation
    service ↔ none (trust-only).
- **Success criteria.** (1) For each trust class, a **stated, bounded** answer to
  "what can the provider see" (FR-K1). (2) Secrets never sit in long-lived
  plaintext on an untrusted provider (FR-K2). (3) Minimal-context placement is
  possible (FR-K3). (4) An attestation **slot** is specified for the untrusted
  case, even if v1's payload is "trust the pool" (FR-K4) — design the slot, not
  necessarily the enclave. (5) Confidentiality **degrades loudly** — a task that
  needs confidentiality it can't get is held, not exposed (FR-K5). (6) The whole
  thing rides the EX6 dial: trivial on a private pool, machinery only when needed.

### HQ2 — Result integrity from a hostile provider **(the co-crux — load-bearing)**

> *A hostile provider owns the worker's entire environment. How do you trust what
> it says the agent produced?*

- **Why hard.** The dual of HQ1: a provider that can *read* everything can also
  *write* anything. It can return a plausible-but-wrong artifact, a subtly
  corrupted patch, a fabricated "tests pass," or claim work it never did — and the
  authorizer wasn't there to watch. Attribution (the worker signs as the agent)
  proves *who claims* the result but **not that the result is correct** — a
  hostile provider holds the delegated signer and signs its lie. So integrity
  needs an *independent* check, and every independent check (re-run, quorum) costs
  real compute, reopening HQ9's economics. WG already has one cheap-ish check —
  the **eval-gate** (`auto_evaluate`/FLIP scores output against `## Validation`) —
  but an eval can be gamed by an adversary who knows it's being graded.
- **Decision axes.**
  - *Verification lever:* attribution-only (trust the signature) ↔ WG eval-gate
    scoring ↔ deterministic **re-run** elsewhere + compare ↔ **N-of-M quorum**
    (multiple providers, majority) ↔ attested-execution transcript (the TEE
    vouches for what ran).
  - *Who verifies:* the authorizer ↔ a second trusted provider ↔ the eval-gate
    agency role ↔ a quorum of independents.
  - *When:* always ↔ proportional to trust (FR-V5: trusted accepted on
    attribution; untrusted re-run/quorum) ↔ random spot-check / audit sampling.
  - *Determinism problem:* agent outputs are non-deterministic, so "re-run and
    diff" needs a notion of *equivalent*-not-*identical* (eval-gate agreement,
    test-pass, semantic check) rather than byte-equality.
  - *Blast-radius bound:* even a believed-forged result is capped by the scoped
    write capability (FR-C2/FR-V4) and is auditable/revocable after the fact.
- **Success criteria.** (1) Results are signature-**attributed** and unsigned ones
  rejected (FR-V1). (2) A **menu of verification levers** (re-run / quorum /
  eval-gate) is defined, selectable by trust (FR-V2, FR-V5). (3) Results carry
  enough **evidence** to be judged, not just a done-bit (FR-V3). (4) The
  worst-case forging provider is **bounded** by its delegation and caught/revoked,
  never able to escalate beyond its own task (FR-V4). (5) The non-determinism of
  "compare two agent runs" is addressed with an equivalence notion, not naive
  byte-diff. (6) Verification cost rides the EX6 dial (cheap when trusted).

### HQ3 — Placement & scheduling

> *Who decides where a task runs — the dispatcher (push) or the provider (pull) —
> and across what kind of pool?*

- **Why hard.** WG today is pure push: one dispatcher on one machine spawns
  workers locally. Federating execution forks this into a real scheduling problem
  with no single right answer. **Push** keeps the authorizer in control but needs
  it to know every provider's live state; **pull** lets idle providers self-serve
  (decentralized, scales) but cedes ordering/fairness control and invites
  cherry-picking (providers grab cheap tasks, starve hard ones). The pool shape —
  **private** (you own it), **cooperative** (mutual-trust peers), **open market**
  (strangers) — changes everything about trust, confidentiality, and integrity,
  yet the frame wants *one* mechanism that spans them via the trust dial (FR-P4).
- **Decision axes.**
  - *Control:* push (dispatcher assigns) ↔ pull (provider claims authorized work)
    ↔ hybrid (push within pool, pull from a shared queue).
  - *Pool:* private ↔ cooperative ↔ open market — distinguished by trust (FR-P4),
    not by protocol.
  - *Matching:* filter-only (eligibility) ↔ ranked (cost/latency/model/reputation,
    FR-P5).
  - *Fairness / anti-cherry-pick:* free claim ↔ priority/lease auctions ↔
    dispatcher-mediated assignment for hard/sensitive tasks.
  - *Authority of the scheduler:* central scheduler ↔ per-authorizer ↔ fully
    decentralized claiming (→ HQ10).
- **Success criteria.** Both push and pull are expressible with a stated default
  (FR-P3); the **same mechanism** spans private→cooperative→market with only
  trust changing (FR-P4); matching respects capability + trust floors (FR-P2);
  a stated stance on fairness/cherry-picking; consistency with the
  decentralization choice (HQ10).

### HQ4 — Provider identity & trust/reputation

> *Providers are `wgid:` identities — how do trust level and reputation gate what
> they may run, see, and be believed about?*

- **Why hard.** Provider identity is *reused* from WG-Fed (EX5/FR-R1 — don't
  reinvent it), so the hard part isn't the keys; it's the **policy**: trust must
  simultaneously gate *placement* (HQ3), *leash tightness* (HQ5/HQ11),
  *confidentiality* (HQ1), and *verification depth* (HQ2) — it is the one dial the
  whole plane reads (FR-R2). Reputation makes it harder: it must accrue from
  *observed* behavior (eval pass-rate, liveness, integrity outcomes) without
  being **gameable** (a provider that behaves until trusted, then defects — the
  classic reputation attack) and without a central reputation authority
  (decentralization, HQ10). Capability advertisements are self-reported, so a
  provider can *lie* about having `claude:opus` or a TEE.
- **Decision axes.**
  - *Trust source:* manual operator assignment (private pool) ↔ WG-Fed
    web-of-trust / vouching ↔ earned reputation ↔ stake/bond.
  - *Reputation accrual:* eval-gate pass-rate ↔ liveness/lease record ↔
    integrity-check (re-run/quorum) outcomes ↔ peer attestations; centralized
    ledger ↔ per-authorizer local ↔ gossiped.
  - *Advertisement trust:* signed claims (FR-R4) + after-the-fact catch ↔
    attested capability (the TEE proves the model) ↔ probe/challenge before trust.
  - *Defection handling:* slow-to-earn/fast-to-lose trust ↔ bonded stake slashed
    on misbehavior.
- **Success criteria.** Provider trust is a verifiable property of a `wgid:`
  identity (FR-R1) and is the single dial gating placement/leash/context/verify
  (FR-R2); reputation accrues from observed behavior and is resistant to the
  behave-then-defect attack (FR-R3); capability claims are signed and false ones
  detectable (FR-R4); no mandatory central reputation authority (HQ10).

### HQ5 — Capability flow to the worker

> *The worker gets a scoped act-as-agent UCAN + a scoped graph-write UCAN, never
> the root key. How are these issued, scoped, expired, and revoked?*

- **Why hard.** This is the EX5 reuse made concrete, and the place a mistake is
  catastrophic: ship too much (the root key, or blanket graph-write) and a single
  hostile/leaky provider can impersonate the agent everywhere and rewrite the
  whole graph; ship too little and the worker can't do its job (log, write
  artifacts, mark done, perhaps spawn subtasks). The worker also runs on a machine
  that may *leak or steal* whatever it holds (HQ1), so the credential's **blast
  radius if exfiltrated** must be small — which means short TTLs and tight scope,
  exactly the EX6 leash. And revocation must actually *reach* — revoking a
  delegation only helps if the authorizing graph checks it at write time.
- **Decision axes.**
  - *Form:* UCAN capability token (`iss`/`aud`/capability/expiry — the WG-Fed
    delegation, reused) ↔ (rejected) shared key.
  - *Scope:* per-task ↔ per-task-subtree ↔ per-capability (log vs artifact vs
    done vs subtask-create); whole-graph write is rejected (FR-C2).
  - *Lifetime:* fixed TTL ↔ lease-coupled (delegation valid only while lease held,
    ties HQ6) ↔ renewable.
  - *Privileged ops:* worker holds the credential ↔ worker *requests* privileged
    actions from the authorizer (remote-sign / callback), keeping authority off
    the provider entirely (ties HQ1/FR-K2).
  - *Revocation:* expiry-only ↔ explicit revoke list checked at write ↔ short-TTL
    + re-issue (revoke by not-renewing).
- **Success criteria.** The worker receives a **scoped UCAN, never the root key**
  (FR-C1); graph-write is **task-scoped**, not blanket (FR-C2); delegations
  **expire and are revocable** with revocation enforced at write-time (FR-C3); the
  scope/TTL **follow the EX6 dial** by provider trust (FR-C4); a leaked worker
  credential's blast radius is bounded to its task; consistency with WG-Fed's
  delegation format (no new delegation system — NFR-4).

### HQ6 — Liveness across trust boundaries (the distributed orphan problem)

> *A remote provider claims authorized work, then dies or stalls. How is the task
> reclaimed without double-execution — when the provider may be uncooperative?*

- **Why hard.** Today reclaim is a *manual, same-trust* operation (`wg reclaim`
  forcibly takes an InProgress task from one local actor to another). Federation
  makes it **automatic** and **cross-trust**: the authorizer must detect a remote
  death it can't directly observe, decide the provider is gone, and re-place the
  task — all while the "dead" provider might merely be **partitioned** and still
  working. Reclaim-then-it-comes-back is the **double-execution / split-brain**
  hazard: two workers complete the same task and both try to commit (FR-L2). And
  the provider is the one *reporting* its own liveness, so a hostile provider can
  hold a lease forever (DoS by squatting) while producing nothing — liveness must
  be judged by the *authorizer's* observations, not the provider's word (FR-L3).
- **Decision axes.**
  - *Liveness signal:* provider heartbeat (self-report) ↔ authorizer-observed
    progress (accepted partial results) ↔ both; lease renewal as the unit.
  - *Lease term:* fixed ↔ trust-scaled (FR-L4, EX6) ↔ task-size-scaled.
  - *Double-execution defense:* fencing token / lease epoch (late worker's write
    rejected, FR-L2) ↔ idempotent commit ↔ first-writer-wins + reject rest.
  - *Partition stance:* prefer-safety (don't reclaim until certainly dead, risk
    stalls) ↔ prefer-liveness (reclaim fast, rely on fencing to dedupe).
  - *Squatting defense:* lease caps, no-progress reclaim, reputation penalty
    (HQ4).
- **Success criteria.** Leased claims with automatic reclaim on missed renewal
  (FR-L1); **reclaim is safe against double-commit** via fencing/epoch (FR-L2);
  liveness judged by the authorizer, robust to a lying provider (FR-L3); lease
  term rides the EX6 dial (FR-L4); a partitioned-but-alive worker cannot corrupt
  a re-placed task; squatting is bounded.

### HQ7 — Data / context locality

> *Where does the task input + graph-slice live during remote execution, how does
> the provider get them, and how are they protected in transit and at rest?*

- **Why hard.** Federating execution means the *data* has to reach the *compute*,
  and that movement is exactly the confidentiality attack surface (HQ1). Decisions
  here — push vs pull, how much to ship, what format — directly set what the
  provider can see and keep. At-rest is the nasty part: once context lands on the
  provider's disk, **you are trusting the provider to delete it**, and an
  untrusted provider's goodwill is worth nothing (FR-D3). Pull-based (provider
  fetches a signed bundle) is operationally clean and decentralization-friendly
  but means the bundle exists as an addressable blob that could be over-fetched.
- **Decision axes.**
  - *Movement:* authorizer pushes context ↔ provider pulls a signed/encrypted
    bundle ↔ provider streams slices on demand (least-at-rest).
  - *Bundle contents:* full graph ↔ task subtree ↔ task + explicit deps (ties
    HQ1 minimization).
  - *In transit:* WG-Fed per-recipient encryption + auth (FR-D2, reused) — not a
    new scheme.
  - *At rest:* plaintext-on-trusted ↔ encrypted-to-enclave ↔ never-at-rest
    (in-memory only); disposal expectation + whether confidentiality may rely on
    the provider deleting (it may not, for untrusted).
  - *Locality of the canonical graph:* stays at authorizer (provider holds only a
    slice + writes back deltas) ↔ replicated.
- **Success criteria.** A defined **context bundle** with a stated locality and
  acquisition story (FR-D1); transit encrypted/authenticated end-to-end via the
  reused substrate (FR-D2); at-rest protection + disposal stated per trust class,
  not relying on an untrusted provider's goodwill (FR-D3); minimization is
  possible (ties HQ1); the canonical graph's authority stays well-defined.

### HQ8 — Sandboxing / isolation guarantees

> *What isolation must a provider meet — the federated successor to today's
> per-task git worktree?*

- **Why hard.** Today isolation is a **git worktree**: enough to keep concurrent
  *cooperating* local agents from clobbering each other's files, assuming a shared
  trusted machine. That assumption dies under federation. Now isolation must hold
  **both directions across a trust boundary**: protect the *worker's* context from
  a snooping provider/co-tenant (confidentiality, HQ1) **and** protect the
  *provider's* host from a malicious workload (the agent — or a poisoned task —
  trying to escape, exfiltrate, or attack the host, FR-D5). A worktree does
  *neither* across hosts. The stronger the isolation (container → microVM →
  TEE), the higher the operational cost and the fewer providers can offer it — the
  EX6 dial again. And isolation class is *self-advertised* (HQ4), so it can be
  lied about absent attestation.
- **Decision axes.**
  - *Class:* git worktree (local, today) ↔ OS user/cgroup ↔ container ↔ microVM
    (Firecracker-class) ↔ TEE/enclave (also gives HQ1 confidentiality).
  - *Required-vs-advertised:* task declares a *minimum* class; provider advertises
    (FR-R4); attestation proves it (ties HQ1/HQ4) vs trust-the-claim.
  - *Direction:* sandbox-the-agent (protect host) ↔ sandbox-the-host-from-seeing
    (protect context) ↔ both (FR-D5).
  - *Network policy:* egress-blocked ↔ allow-listed ↔ open (affects exfiltration
    and the agent's ability to use tools/APIs).
- **Success criteria.** A declared **isolation-class ladder** with a per-trust
  minimum (FR-D4); isolation justified in **both** threat directions (FR-D5);
  the class is advertised and (for low trust) attestable, not merely claimed
  (ties HQ4); a stated network-egress policy; v0 maps cleanly onto today's
  worktree for the private-pool case (EX8).

### HQ9 — Economics & metering (likely v1-deferred — flagged, not dropped)

> *Who pays for compute + tokens when the runner isn't the authorizer, and how
> are budgets/ceilings enforced (R32)?*

- **Why hard.** The moment compute is someone else's, "who pays" is unavoidable —
  the authorizer's API keys? the provider's, billed back? a pre-paid pool? Each
  reopens confidentiality (whose credentials sit on the provider, HQ1) and
  integrity (a provider can **pad its bill** for tokens it didn't spend, needing
  signed/reconcilable metering, FR-E3/HQ2). A real *market* (price discovery,
  settlement, disputes) is a large subsystem and a clear v1 non-goal — **but**
  budgets/ceilings (R32) and naming the payment model are *not* deferrable even
  for a private pool, because a runaway remote task or a compromised provider can
  burn unbounded money. The risk is silently dropping economics entirely because
  "the market is out of scope."
- **Decision axes.**
  - *Who pays:* authorizer-funded (its provider creds, ties HQ1) ↔ provider-funded
    + billed-back ↔ pre-paid/escrowed pool ↔ free (private pool you own).
  - *Budget enforcement:* per-task ceiling ↔ per-provider ↔ per-pool; halt-on-cap
    ↔ flag-and-continue; reuse existing accounting (`parse_token_usage`,
    `wg spend`).
  - *Metering trust:* trust provider's report ↔ signed/reconcilable metering
    (FR-E3) ↔ authorizer-side independent accounting.
  - *Market depth (mostly out of scope v1):* none ↔ fixed price ↔ auction ↔
    on-chain settlement (non-goal, §5).
- **Success criteria.** The **payment model is named** per pool class, with v1 =
  "you own the pool, you pay" explicit (FR-E1); **budgets/ceilings enforceable**
  via existing accounting (FR-E2, R32); metering attributable enough to detect a
  padded bill (FR-E3); the v1-deferral of a *market* is **stated, not silent**
  (§5). A good answer flags exactly what's deferred and why.

### HQ10 — Decentralization vs central scheduler

> *Where on the placement-authority spectrum does the execution plane sit — one
> scheduler, per-authorizer, or fully decentralized claiming?*

- **Why hard.** Mirrors WG-Fed's V5/HQ6 tension, now for *compute placement*. A
  **central scheduler** gives the best matching, fairness, and global view — and
  is a single point of failure, censorship, and capture, plus it must be *trusted
  with* every task's metadata (who runs what). **Fully decentralized claiming**
  (providers pull from a shared queue) has no such bottleneck but loses global
  optimization and fairness control and complicates anti-cherry-pick (HQ3). The
  frame leans decentralized-but-central-allowed (the WG-Fed V5 stance), so the
  risk is making the *wrong* thing central by accident — e.g. a scheduler that
  becomes a mandatory trust root or a metadata-leak chokepoint.
- **Decision axes.**
  - *Placement authority:* central scheduler ↔ per-authorizer (each WG schedules
    its own tasks onto its known providers) ↔ decentralized claiming (shared
    queue, providers pull) ↔ hybrid (central *hint*/directory, local decision).
  - *What may be central:* provider directory/discovery ↔ reputation ledger ↔
    a matching/scheduling service ↔ none — each marked *correctness-critical* vs
    *convenience-only*.
  - *Trust root:* none (self-certifying providers) ↔ optional anchors ↔ required
    scheduler.
- **Success criteria.** A stated placement-authority position consistent with the
  decentralization lean; a **per-capability table** (directory, reputation,
  scheduling) of what is centralized and why, with **no correctness- or
  security-critical capability depending on a single central node** (mirrors
  WG-Fed HQ6); the per-authorizer/private-pool case works with **no** central
  node at all (NFR-6); central nodes are convenience aids whose loss degrades UX,
  not correctness.

### HQ11 — Leash calibration (the policy engine behind EX6)

> *Authority is broad/long by default and tightened by policy. What computes the
> leash — and who sets it?*

- **Why hard.** EX6 is the spine of this whole study, but "broad by default,
  tighten by policy" only works if there is an actual, legible **policy function**
  mapping `(provider trust, task sensitivity, pool class, environment) →
  (delegation scope/TTL, context exposure, isolation class, verification depth,
  lease term)`. Get the *default* wrong toward tight and you've rebuilt a
  zero-trust system that's unusable for the common private-pool case (violating
  the trust-default principle); get it wrong toward loose and a low-trust provider
  silently gets broad authority + full context (violating HQ1/HQ2/HQ5). The leash
  also touches *five* other dials at once (FR-C4, FR-L4, FR-K*, FR-V5, FR-D*), so
  it must be **one coherent policy**, not five inconsistent ad-hoc thresholds. And
  "environment-driven" means it should be *configurable per deployment* (a paranoid
  org vs a solo hobbyist) without code changes.
- **Decision axes.**
  - *Default position:* broad/long for everything not marked sensitive (the EX6
    default) ↔ broad only within a trust floor.
  - *Policy inputs:* provider trust level ↔ task sensitivity label ↔ pool class ↔
    explicit per-task override ↔ org-wide config.
  - *Policy locus:* hardcoded thresholds ↔ declarative config (per deployment) ↔
    pluggable policy module.
  - *Coupling:* one policy drives all five dials coherently ↔ independent
    per-dimension settings.
  - *Surfacing:* the applied leash is **visible** (like the handler-first
    `wg status` rendering) so a too-tight/too-loose leash is caught.
- **Success criteria.** A single, legible policy maps environment → all five
  execution dials coherently (FR-C4, FR-L4, FR-K*, FR-V5, FR-D*); the **default
  is genuinely slack** for the trusted/private-pool case (EX6 honored — not a
  zero-trust system in disguise); tightening is **policy/config-driven**, not
  hardcoded, and configurable per deployment without code changes; the applied
  leash is **surfaced/inspectable**; a too-loose leash on a low-trust provider is
  impossible-by-construction (the policy can't grant broad authority to a stranger).

### HQ12 — Execution-wire evolution & compat

> *How do the placement / lease / capability / result wire formats evolve across
> independently-updated authorizers and providers without silent mis-routing?*

- **Why hard.** Authorizer and provider are now **separately-owned, separately-
  updated** processes that must agree on placement offers, lease/heartbeat
  messages, capability tokens, and result envelopes. WG already has the scar
  tissue for this: `WG_AGENCY_COMPAT_VERSION` / `WG_PI_PLUGIN_COMPAT_VERSION`
  handshakes that fail **loudly** on mismatch — and the cautionary tale of a bare
  `openrouter:` spec that **silently** routed to a keyless handler and 401'd every
  task for ~14h. A federated execution wire has far more surfaces to drift on, and
  a silent mismatch here means a task placed on a provider that *almost*
  understands it — the worst failure mode. Crypto/format agility matters too
  (delegation/encryption come from WG-Fed, whose own HQ12 owns crypto agility, but
  the execution *envelopes* are this study's to version).
- **Decision axes.**
  - *Versioning:* explicit envelope version + capability negotiation ↔ sniffed.
  - *Handshake:* mirror `WG_AGENCY_COMPAT_VERSION` (named constant, asserted at
    connect) ↔ ad-hoc.
  - *Mismatch stance:* **loud-fail** (WG's convention) ↔ best-effort-degrade ↔
    negotiate-shared-subset.
  - *What's versioned here vs inherited:* execution envelopes (this study) vs
    identity/delegation/crypto formats (inherited from WG-Fed, NFR-4).
- **Success criteria.** A versioned execution wire with an explicit compat
  handshake mirroring `WG_AGENCY_COMPAT_VERSION` (NFR-5); mismatches **fail
  loudly, never silently mis-route** (the bare-`openrouter:` class of bug is
  impossible); vN/vN+1 authorizer↔provider negotiate a shared subset or refuse;
  the boundary between this study's envelopes and WG-Fed's inherited formats is
  explicit (no duplicated/forked crypto — NFR-4).

---

## 4. Cross-cutting tensions (where requirements pull against each other)

These are the conflicts task 4 must *resolve*, not wish away. Each is a real
tradeoff, not a bug.

| # | Tension | Pulls | Where decided |
|---|---------|-------|---------------|
| T1 | **Confidentiality vs reach/cost** | Protecting context (FR-K*) shrinks the eligible provider set and/or demands costly TEEs vs borrowing any idle compute (EX1) | HQ1, HQ8 |
| T2 | **Integrity vs cost** | Re-run/quorum verification (FR-V2) multiplies compute spend vs cheap accept-on-attribution | HQ2, HQ9 |
| T3 | **Open-market reach vs trust gating** | More providers = more capacity (EX1) vs lower trust → tighter leash, more verification (FR-P4, FR-R2) | HQ3, HQ4 |
| T4 | **Trust-default broad authority vs blast radius** | EX6 slack-by-default vs a leaked/ hostile provider's damage if the leash is loose (FR-C1/FR-V4) | HQ5, HQ11 |
| T5 | **Push control vs pull autonomy/decentralization** | Dispatcher control + fairness (HQ3) vs decentralized self-serve providers (HQ10) | HQ3, HQ10 |
| T6 | **Minimal context vs agent effectiveness** | Ship less to protect it (FR-K3) vs the agent needs context to do good work | HQ1, HQ7 |
| T7 | **Liveness/reclaim vs double-execution** | Reclaim fast on stall (FR-L1) vs a partitioned worker returning and double-committing (FR-L2) | HQ6 |
| T8 | **Central scheduler efficiency vs decentralization** | Global matching/fairness vs no single point of failure/capture (FR-F-style) | HQ3, HQ10 |
| T9 | **Verification needs evidence vs confidentiality hides it** | A re-runner/eval-gate needs to *see* the work (FR-V3) vs context/results are confidential (FR-K1) | HQ1, HQ2 |
| T10 | **One-mechanism-spans-all-pools vs per-pool optimization** | FR-P4's single mechanism (private→market via trust) vs specializing each pool | HQ3, HQ11 |

---

## 5. Non-goals / out-of-scope

Explicitly **not** in scope for this execution-federation architecture (stating
these keeps task 4 from over-building and task 5 from attacking absent promises):

1. **Reinventing identity, delegation, keys, encryption, or trust.** These come
   from **WG-Fed** (NFR-4/EX5). This study *composes* `wgid:` identities, UCAN
   delegation, per-recipient encryption, and `trust_level`; it defines **no
   second** such system. Their internal design is WG-Fed's, not ours.
2. **Building a TEE / confidential-compute stack ourselves.** We design the
   **attestation slot** (FR-K4/HQ1) and *use* enclave/attestation primitives; we
   do not implement an enclave, an attestation service, or trusted hardware.
3. **Homomorphic / compute-on-ciphertext execution.** Out of practical reach;
   the confidentiality levers are trust, minimization, and attested isolation —
   not running the model on encrypted data.
4. **A token / blockchain compute marketplace with on-chain settlement.** Pricing,
   auctions, escrow, and dispute resolution as a full economy are out of scope;
   only **budgets/ceilings (R32)** and **naming the payment model** are in (FR-E*,
   HQ9). The deferral is *stated, not silent*.
5. **General-purpose / arbitrary compute.** This federates **WG agent-task
   execution** (run an authorized task's agent), not a generic batch-job or
   container-orchestration platform.
6. **Provider-side model hosting / inference-server design.** A provider brings
   its own handler (claude / codex / nex / pi / opencode) and its own model
   access; how a provider hosts inference is its business, surfaced only as a
   signed *capability advertisement* (FR-R4).
7. **Real-time / low-latency scheduling guarantees.** Work-speed, not RTC
   (NFR-2); nothing assumes sub-second placement.
8. **Re-implementing the existing local dispatcher / spawn / claim path.** Today's
   `wg service start` dispatcher, `wg spawn` worktree isolation, and `wg claim` /
   `wg reclaim` are the **migration substrate** (EX8/NFR-3), re-baselined by task
   2 — not the thing being redesigned.
9. **Picking the final library/wire stack.** Choosing the transport, the TEE
   vendor, the scheduler implementation, or the exact lease protocol is task 4's
   (architectures) call; this doc *constrains*, it does not *select*.
10. **Solving multi-tenant fairness/QoS at scale.** v1 targets private pool →
    cooperative; large-scale open-market fairness, anti-cherry-pick auctions, and
    QoS guarantees are flagged (HQ3) but not required for v1.

---

## 6. Architecture acceptance checklist (definition-of-done for task 4)

A candidate architecture (`exec-architectures`) is **complete** only if it:

- [ ] Answers **HQ1 (context confidentiality — THE crux)** concretely: per
      trust-class, what the provider can/can't see; secret handling; minimal-
      context support; the attestation slot; loud degradation. *(FR-K1–FR-K5)*
- [ ] Answers **HQ2 (result integrity — the co-crux)**: attribution + a menu of
      verification levers (re-run / quorum / eval-gate) selectable by trust, with
      a bounded worst-case forging provider. *(FR-V1–FR-V5)*
- [ ] Specifies **placement & scheduling** (HQ3): push/pull default, the
      private→cooperative→market pool spectrum via one trust-gated mechanism.
      *(FR-P1–FR-P5)*
- [ ] Specifies **provider identity, trust & reputation** (HQ4) reusing WG-Fed
      keys + `trust_level` as the single dial. *(FR-R1–FR-R4)*
- [ ] Specifies **capability flow** (HQ5): scoped act-as-agent + graph-write
      UCANs, never the root key; scope/TTL/revocation; leash-scaled. *(FR-C1–FR-C5)*
- [ ] Specifies **liveness/reclaim across trust boundaries** (HQ6): leases,
      heartbeats, fencing against double-execution. *(FR-L1–FR-L4, NFR-1)*
- [ ] Specifies **data/context locality** (HQ7): the context bundle, transit +
      at-rest protection, disposal not relying on untrusted goodwill. *(FR-D1–FR-D3)*
- [ ] Specifies the **isolation/sandbox ladder** (HQ8) with a per-trust minimum,
      justified in both threat directions. *(FR-D4–FR-D5)*
- [ ] States the **economics/budget** stance (HQ9): payment model named,
      budgets/ceilings (R32) enforceable; the market-deferral is explicit.
      *(FR-E1–FR-E3)*
- [ ] States the **decentralization vs central-scheduler** position (HQ10) with a
      per-capability central/decentralized table; no correctness-critical central
      dependency. *(NFR-6)*
- [ ] Defines the **leash policy engine** (HQ11): one coherent
      environment→five-dials function, slack by default, tightened by config.
      *(FR-C4, FR-L4, FR-K*, FR-V5, FR-D*, EX6)*
- [ ] Defines the **versioned execution wire + compat handshake** (HQ12)
      mirroring `WG_AGENCY_COMPAT_VERSION`, failing loud on mismatch. *(NFR-5)*
- [ ] Confirms **substrate reuse** — no second identity/delegation/crypto/trust
      system is introduced. *(NFR-4/EX5)*
- [ ] Names a **phased rollout** (v0 private trusted pool on today's WG → …).
      *(NFR-3/EX8)*
- [ ] Resolves (does not ignore) each **§4 tension**, stating which side it takes
      and why.
- [ ] Stays inside the **§5 non-goals**.

If any **MUST** requirement (§2) is unmet, the architecture must say so
explicitly and justify the deferral — silence is failure.

---

## 7. Traceability matrix (execution pillar → requirements → hard questions)

| Execution pillar | Requirements | Hard questions |
|------------------|--------------|----------------|
| **EX1** Compute separable from authority | FR-P1, FR-P3, FR-D1, FR-E1, NFR-2, NFR-6 | HQ3, HQ7, HQ10 |
| **EX2** Trust/capability-gated placement | FR-P2, FR-P4, FR-P5, FR-R2, FR-R3, FR-R4, FR-D4 | HQ3, HQ4, HQ8 |
| **EX3** Verifiable results **(co-crux)** | FR-C5, FR-V1–FR-V5, FR-R4, NFR-7 | **HQ2**, HQ4 |
| **EX4** Confidential context **(crux)** | FR-K1–FR-K5, FR-D2, FR-D3, FR-D5 | **HQ1**, HQ7, HQ8 |
| **EX5** Reuse WG-Fed substrate (no reinvention) | FR-C1, FR-C2, FR-D2, FR-R1, NFR-4 | HQ5, HQ4 |
| **EX6** Trust-default / leash-as-a-dial | FR-C3, FR-C4, FR-K2, FR-L4, FR-V5 | HQ5, HQ11 |
| **EX7** Liveness across trust boundaries | FR-L1–FR-L4, NFR-1 | HQ6 |
| **EX8** Incremental on today's WG | FR-P3, FR-D4, FR-E2, NFR-3, NFR-5, NFR-6 | HQ12 |
| **R32** budgets/ceilings (gap) | FR-E1, FR-E2, FR-E3 | HQ9 |

---

## Appendix — sources & provenance

- **North star / frame:** this study's brief (execution-federation, wave 1) and
  Erik's execution-federation model. The execution plane = the fourth seam after
  WG-Fed's state + authority + identity: *placing an authorized task's agent onto
  compute that need not be the authorizer's, gated by trust + capabilities, with
  verifiable results and confidential context*, under the **trust-default /
  leash-as-a-dial** principle (authority broad/long by default; tightening is
  environment-driven policy, not the birth state).
- **WG-Fed substrate (reused, not reinvented):**
  `docs/federation-study/03-requirements-and-hard-questions.md` (identity,
  messaging, keys, trust requirements) and
  `docs/federation-study/04-candidate-architectures.md` — specifically the
  **three-key model** (root/identity key *never on the ephemeral worker*; a
  signer/device/agent key on the acting host; **UCAN-style short-lived scoped
  delegations** — `iss`=authorizer, `aud`=agent/provider, capability + expiry),
  the **"delegate, don't share keys"** custody rule, `wg secret` as agent-key
  custodian, the append-only **sigchain**, per-recipient encryption as the ACL
  layer, and `Agent.trust_level`. This study **composes** all of the above and
  must introduce no second identity/delegation/crypto/trust system (NFR-4/EX5).
- **Gap analysis:** private repo `poietic-pbc/poietic-family-team`. Only **R32**
  (budgets/ceilings) is cited by number, because it is named in this study's
  frame for economics/metering (HQ9). Other gap-analysis IDs live in the source
  repo and were deliberately *not* invented.
- **Current code (grounding only; full baseline is task 2, `exec-baseline`):**
  the **dispatcher** (`wg service start` / daemon — push-only, single-host
  spawning today); **`wg spawn` worktree isolation** (`src/commands/spawn/worktree.rs`
  — the local isolation today's "sandbox" amounts to, the seed of HQ8/FR-D4);
  **`wg claim` / `wg reclaim`** (`src/commands/reclaim.rs` — manual, same-trust
  takeover of an `InProgress` task; the seed of HQ6/FR-L*); **handler routing**
  (`src/dispatch/handler_for_model.rs` — claude/codex/nex/pi/opencode; a provider
  "brings its handler", §5 non-goal 6); **`WG_AGENCY_COMPAT_VERSION`** (the
  loud-fail compat-handshake convention HQ12/NFR-5 mirrors; the bare-`openrouter:`
  silent-misroute is the cautionary tale); **token/cost accounting**
  (`graph::parse_token_usage`, `wg show` / `wg spend` / `wg stats` — the metering
  substrate for FR-E2/FR-V3, and the eval-gate (`auto_evaluate`/FLIP scoring of
  output vs `## Validation`) the integrity lever FR-V2 reuses).
- **Prior art to mine (task 1, `exec-prior-art`, will deepen):** CI runners
  (GitHub Actions self-hosted runners — claim/lease/labels = capability match),
  job schedulers (Nomad/k8s — push/pull placement, bin-packing), compute markets
  (Golem/Akash/Bacalhau — open-market placement + verification), TEEs
  (SGX/SEV-SNP/TDX/Nitro Enclaves — HQ1 attested confidentiality), and verifiable
  compute (zk/optimistic re-execution, quorum — HQ2 integrity).
