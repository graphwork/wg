# Execution Federation Study 1/6 — Prior-Art Landscape

> **Distributed / federated / shared *execution & compute* systems, mapped to
> WG's need: place an authorized task's agent onto compute that need not be the
> authorizer's own machine — gated by trust + capabilities, with verifiable
> results and confidential context.**
>
> Wave 1, task 1 of 6 (the *gather* phase). Downstream consumers:
> `exec-architectures` (4/6 — candidate architectures: trusted-pool ↔ market ↔
> confidential ↔ hybrid) and the FLIP. This document is a *survey + comparison*,
> not a design — design decisions are deferred to those tasks.
>
> **Substrate.** This study builds on the WG-Fed identity/capability decision
> (`docs/federation-study/06-decision-memo-and-roadmap.md`). The relevant fixed
> points: every actor is a self-certifying `wgid:<pubkey>` (HQ5); agents act
> under **short-lived, scoped, attenuating UCANs**, never the principal's root
> key (HQ1/HQ11); the trust root is **never central** while availability rests on
> an optional WG node (HQ6); and **loaded state is untrusted input** (S-5). In
> execution-federation terms: *providers* will be `wgid:` identities at some
> `trust_level`, and *workers* will carry scoped UCANs. The execution layer rides
> on that substrate; this survey asks how the outside world has solved running
> someone else's job on someone else's machine.

---

## 0. The WG execution yardstick (what we are measuring against)

WG's task graph already dispatches a task's agent onto a *local* worker (the
daemon spawns a handler — claude/codex/nex/pi — in a worktree). The
execution-federation question is the next step: **dispatch that agent onto
compute that is not the authorizer's machine**, safely. Every system below is
scored against these dimensions:

| # | WG execution requirement | One-line test |
|---|----------------|---------------|
| **X1** | **Off-machine placement** — run an authorized task's agent on compute that need not be the authorizer's own machine | Can I dispatch a task's agent to a runner I do not own? |
| **X2** | **Trust + capability gating** — placement is gated by the provider's trust level; the worker acts under a scoped, expiring capability, not a standing root key | Is the runner a `wgid:` at sufficient `trust_level`, and does the worker hold only a scoped UCAN? |
| **X3** | **Context confidentiality on untrusted compute** *(CRUX a)* | Can a runner *run* the agent without being able to *read* the principal's private context/data? |
| **X4** | **Result integrity against a hostile runner** *(CRUX b)* | Can I trust the output without trusting the runner — by attestation, re-run, quorum, or proof? |
| **X5** | **Liveness / lease / reclaim** — a runner that claims work then dies is detected and the work reclaimed | If the runner vanishes mid-task, does the task get rescheduled (at-least / exactly once)? |
| **X6** | **Decentralization posture** — P2P-leaning, a central coordinator *allowed* but never the trust root (inherits WG-Fed HQ6) | Can placement work without a single mandatory central scheduler *in the trust path*? |
| **X7** | **Identity-rooted, attributable** — providers and workers are keys; every placement and result is signed and attributes to agent + principal | Is the dispatch and the result attributable back to a `wgid:`/UCAN chain? |

The two **special-focus cruxes** the task singles out are **X3 (context
confidentiality on untrusted compute)** and **X4 (result integrity against a
hostile runner)** — the two things that are *hard* precisely because the runner
is not you. They are scored explicitly in §4. The other dimensions
(placement, lease/reclaim, decentralization) are largely *solved* by mature
systems and WG can borrow directly; the cruxes are where prior art thins out.

**One framing that governs the whole survey:** confidentiality (X3) is about
hiding the job from the *runner*; integrity (X4) is about the runner not being
able to *lie* about the result. They are independent axes, and almost every
mainstream system **solves neither**, because it assumes the runner is *your own
infrastructure in your own trust domain*. Only two families break that
assumption — **TEEs/confidential computing** (the only practical answer to X3)
and **verifiable computation** (re-run/quorum, optimistic challenge, zkVM — the
answers to X4). See §4.

---

## 1. Comparison matrix (systems × dimensions)

Twenty-two systems across the seven WG dimensions plus the per-system capture
dimensions. Some closely-related pairs share a row (Sidekiq/Faktory, SGX/TDX,
Modal/RunPod, E2B/Daytona). Abbreviations: **S&F** = store-and-forward, **TEE** =
trusted execution environment, **RA** = remote attestation, **CAS** =
content-addressed storage, **DePIN** = decentralized physical-infrastructure
network, **TCB** = trusted computing base.

### 1a. Placement, trust, liveness/lease

| System | Placement model | Trust model (is the runner trusted with the payload?) | Liveness / lease / reclaim |
|---|---|---|---|
| **GitHub Actions self-hosted runners** | **Pull-claim** — runner long-polls GitHub for a job and claims it | **Fully trusted** — runner sees repo + injected secrets; GitHub warns *never* run on public-repo PRs (arbitrary code on your host) | Job timeout → re-run; **ephemeral / just-in-time runners** (one job, then deregister) bound blast radius |
| **Buildkite agents** | **Pull-claim** — agent polls the Buildkite control plane for work | **Agent trusted with payload; SaaS control plane is NOT** — "your code and secrets never touch our servers"; builds run on *your* agents | Agent heartbeat; lost agent → job rescheduled; configurable disconnect timeout |
| **GitLab runners** | **Pull-claim** — runner long-polls (`request_job`); executors shell/docker/k8s | **Fully trusted** — runner sees source + CI/CD variables; shared-runner isolation is the user's job | Runner heartbeat; job timeout → retry; ephemeral docker/k8s executors per job |
| **Kubernetes (kube-scheduler)** | **Push / bin-pack** — scheduler filters+scores nodes, binds Pod; kubelet pulls & runs | **Single trust domain** — node/kubelet sees all workload memory & secrets; no intra-cluster confidentiality (unless Confidential Containers) | **Node lease** + heartbeat; node-not-ready → pod eviction → controller reschedules |
| **HashiCorp Nomad** | **Push** — server evaluates → plan → allocation; client pulls its allocs | **Single trust domain** — client runs whatever the server placed | **Client heartbeat + grace**; missed heartbeat → node down → allocs rescheduled |
| **Apache Mesos** | **Resource-offer** (two-level) — master *offers* resources to frameworks; framework accepts & launches via executor | **Single trust domain** (datacenter) | Agent heartbeat; failed agent → tasks lost → framework relaunches |
| **Temporal** | **Pull-claim** — workers poll task queues; service persists durable history | **Workers trusted (your code/infra)**; service is a durable store — *can* be blinded (see X3) | **Task lock = lease**; schedule-to-start / start-to-close / **heartbeat** timeouts → activity retried on another worker |
| **Celery** | **Pull** from broker (RabbitMQ/Redis); prefetch | **Workers trusted**; broker holds task args (often plaintext; pickle = RCE risk) | **`acks_late` + visibility timeout** → redeliver on worker death (at-least-once) |
| **Sidekiq / Faktory** | **Pull** — workers FETCH jobs from Redis/Faktory server | **Workers trusted** (same trust domain) | **Reservation/visibility timeout** → requeue if not ACKed (Faktory `RESERVE`; Sidekiq super-fetch) |
| **BOINC** | **Pull** — volunteer client requests work units from the project scheduler | **Runner UNTRUSTED** (anyone may attach); app code **signed offline** so a hijacked server can't push malware | **Report deadline** per result; missed deadline → work unit **reissued** to another host |
| **Folding@home** | **Pull** — client requests work units from assignment/work servers | **Runner UNTRUSTED** (public volunteers) | Deadline + reissue; less adversarially hardened than BOINC |
| **Bacalhau** | **Bid/match** — requester posts job; compute nodes near the data bid; **move compute to data** | **Compute node semi-trusted**; data-locality means the node already holds the data | Lease/ack on the orchestrator; failed exec → reschedule to another node |
| **Golem** | **Market match** — requestor posts, providers offer; micropayment per task | **Provider UNTRUSTED** (open market) | Agreement with timeout; payment on verified completion |
| **Akash** | **Market reverse-auction** — providers bid on a deployment; tenant picks a bid → **on-chain lease** | **Provider trusted with the workload** (no confidentiality by default; runs k8s under the hood) | **On-chain lease**; provider liveness via bids/escrow; tenant closes lease on misbehavior |
| **Fluence** | **Market** (DePIN) — rent VMs from independent providers; **Proof-of-Capacity** to the chain | **Provider trusted with the VM** | Proof-of-Capacity proves *availability*; deal/escrow governs reclaim |
| **iExec** | **Market** — requester orders a task; worker pool executes; on-chain settlement | **Provider UNTRUSTED → run inside SGX** (TEE is the trust mechanism) | On-chain deal; PoCo consensus governs result acceptance & reclaim |
| **Ray** | **Distributed scheduler** — head + workers; tasks/actors placed across the cluster | **Single trust domain** (your cluster) | **Lineage re-execution** — lost task re-run from its lineage on another worker (fault-tolerance, not anti-cheat) |
| **Modal / RunPod** | **Push to operator fleet** — submit code/container; platform schedules on its GPUs | **Operator trusted** with code+data; gVisor/Firecracker isolates *the operator's host from your code*, not vice-versa | Platform-managed; container timeout/retry; serverless cold-start |
| **E2B / Daytona** | **Push** — spawn a sandbox (Firecracker microVM / dev container) for agent-generated code | **Operator trusted**; isolation **protects the host from the agent** (inverted from WG's need) | Sandbox TTL/lease; killed on idle/timeout |
| **Intel SGX / TDX** | n/a (a *primitive*, not a scheduler) | **Runner host UNTRUSTED of the enclave** — CPU bars OS/hypervisor/host-root from enclave memory | n/a (liveness handled by the surrounding scheduler) |
| **AMD SEV-SNP** | n/a (primitive) | **Hypervisor UNTRUSTED** — VM memory encrypted + integrity-protected against the host | n/a |
| **AWS Nitro Enclaves** | n/a (primitive) — carve an isolated VM from a parent EC2 instance | **Parent-instance operator (and AWS ops) UNTRUSTED of the enclave**; no persistent storage, no interactive access, only a vsock | n/a (parent process supervises) |
| **Confidential Containers (CoCo)** | Pod scheduled by k8s **into a TEE VM** (SEV-SNP/TDX) | **Host/cluster-operator UNTRUSTED**; secrets/images released only post-attestation (Key Broker + Trustee) | k8s lease/reschedule as normal |
| **Re-run / quorum (replication)** | n/a (an integrity *technique*) | Runners untrusted; **honest-majority** assumed | re-dispatch on disagreement |
| **Optimistic challenge (Truebit / OP-rollups)** | n/a (technique) | Runners untrusted; **≥1 honest verifier** assumed + bonds | **Challenge window** is the liveness/finality cost |
| **zkVM / proof-of-execution (RISC Zero / SP1)** | n/a (technique) | **Runner fully untrusted** — no honesty assumption at all | proof attached to result; re-dispatch if no proof |

### 1b. Confidentiality (X3), result integrity (X4), decentralization, maturity, WG fit

| System | **X3 — runner sees the data?** | **X4 — how is the output trusted?** | Decentralization | Maturity / op-cost | WG fit (the cruxes) |
|---|---|---|---|---|---|
| **GitHub self-hosted runners** | **Yes — sees all** (repo+secrets) | **Trusted runner** (your infra); SLSA/signed provenance = *attribution*, not integrity | Central (GitHub) coordinator | Very mature; low cost | Placement/pull model ★; **neither crux** |
| **Buildkite agents** | **Hidden from SaaS, seen by agent** | Trusted agent | Hybrid (SaaS plane, your runners) | Mature | **Best "control-plane sees nothing" split** — but agent still trusted |
| **GitLab runners** | **Yes — sees all** | Trusted runner | Central-ish | Mature | Pull model ★; neither crux |
| **Kubernetes** | **Yes** (node sees memory) unless CoCo | Trusted cluster | Central control plane | Very mature; moderate | **Lease/reclaim ★★★**; neither crux natively |
| **Nomad** | **Yes** | Trusted cluster | Central control plane | Mature; light | **Heartbeat/grace reclaim ★★★** |
| **Mesos** | **Yes** | Trusted datacenter | Central master | Legacy (superseded by k8s) | Resource-offer model of historical interest |
| **Temporal** | **Service can be blinded** — payload codec encrypts; server stores ciphertext; plaintext only on client+worker | Deterministic **replay** detects nondeterminism; workers trusted | Central service (self-hostable) | Mature; moderate | **Lease/heartbeat/retry ★★★ + payload-codec confidentiality-from-the-coordinator ★★** |
| **Celery** | **No** (broker sees args) | Trusted workers | Central broker | Mature; light | Late-ack reclaim ★; neither crux |
| **Sidekiq / Faktory** | **No** | Trusted workers | Central server | Mature; light | **Reservation-timeout reclaim ★★** |
| **BOINC** | **No** (public science data — non-goal) | **Redundant computing + validator** — N copies, compare, credit on agreement; adaptive replication | Central project servers + untrusted edge | Mature (decades) | **Canonical X4-by-quorum ★★★**; X3 n/a |
| **Folding@home** | **No** | Credit + partial re-run; weaker than BOINC | Central servers + edge | Mature | X4-by-redundancy ★★ |
| **Bacalhau** | **Limited** (node holds the data) | **Determinism + content-addressed result hashes** (same code+data → same output hash); re-run to verify; **Lilypad** adds mediation | Decentralized network | Growing | **Deterministic-replay integrity ★★** (breaks on nondeterministic agents — see §4.2) |
| **Golem** | **Limited** (SGX explored for confidential tasks) | **Verification-by-redundancy** for some task classes (e.g. rendering); general case open | Decentralized market | Niche/early | Market model ★; both cruxes partial |
| **Akash** | **No** (provider sees workload) | **Reputation / audited providers**; none cryptographic | Decentralized infra market | Growing | **Reverse-auction lease ★★** maps to wgid: providers; neither crux |
| **Fluence** | **No** | Proof-of-Capacity proves *availability*, **not** correct execution | Decentralized DePIN | Growing | Market/availability-proof ★; neither crux |
| **iExec** | **Yes-but-sealed — SGX enclave** | **TEE attestation + PoCo** on-chain consensus | Decentralized market + TEE | Niche | **Closest "untrusted market + TEE confidentiality + attested integrity" exemplar ★★★** |
| **Ray** | **Yes** (your cluster) | Lineage re-exec = fault-tolerance, not anti-cheat | Single cluster | Mature | How to fan an agent across a *trusted* pool ★; neither crux |
| **Modal / RunPod** | **Yes** (operator sees all) | Trusted operator | Central operator | Mature; pay-per-use | "Rent GPU, trust operator" baseline ★; neither crux |
| **E2B / Daytona** | **Yes** (operator sees all) | Trusted operator | Central operator | New, agent-focused | **Closest in purpose** (run agent code in a sandbox) but **inverted threat model** — protects host *from* agent |
| **Intel SGX / TDX** | **No — CPU bars host** | **RA quote** proves measured code ran in a genuine enclave; sealed output | Primitive (cloud-available) | Mature (SGX); TDX newer | **Solves BOTH cruxes ★★★** at process (SGX) / VM (TDX) granularity |
| **AMD SEV-SNP** | **No — memory encrypted vs hypervisor** | **Signed attestation report** (AMD root) + integrity protection | Primitive (Azure/GCP CVMs) | Mature, widely deployed | **Solves BOTH cruxes ★★★** at whole-VM granularity (agent runtime fits unmodified) |
| **AWS Nitro Enclaves** | **No — parent & AWS ops barred** | **Attestation doc (PCRs)**; **KMS releases the data key only if PCRs match** | Primitive (AWS-only) | Mature, very deployable | **Solves BOTH cruxes ★★★**; cleanest "release secret only to attested code" pattern |
| **Confidential Containers** | **No — host barred** | **RA-gated** secret/image release (Trustee/Veraison) | k8s-native + TEE | Emerging | **K8s-native both-cruxes path ★★★** — closest to a deployable WG confidential pod |
| **Re-run / quorum** | **No** (every replica sees data) | **Agreement of N independent runs** (honest-majority) | technique | proven (BOINC, every blockchain) | X4 ★★ for *deterministic* work; **fails on nondeterministic agents** |
| **Optimistic challenge** | **No** | **Fraud proof in a challenge window**; cheap happy-path, ≥1 honest verifier + bond | technique | mature in rollups | X4 ★★ but **assumes deterministic re-execution** + adds finality latency |
| **zkVM / proof-of-execution** | **No (prover sees witness)**; zk hides inputs only from the *verifier* | **Succinct cryptographic proof** — integrity with *no* trust/bond/quorum | technique | emerging, heavy prover | **Strongest X4 (unconditional) ★★★**; **does not give X3** vs the runner; deterministic-only; expensive |

---

## 2. Per-system narratives

Grouped by family. Each maps to the capture dimensions, with the two cruxes
(X3 confidentiality, X4 integrity) called out. Citations point at canonical
sources (full URLs in §6).

### 2.1 CI runner pools (GitHub / Buildkite / GitLab) — *the pull-claim placement model, fully-trusted runner*

- **Placement.** All three use **pull-claim**: the runner is a long-lived
  process that *polls* the control plane and claims the next job — GitHub
  Actions runners long-poll for jobs [GH runners]; GitLab runners call
  `request_job` [GitLab]; Buildkite agents poll the agent API [Buildkite]. This
  is the opposite of a scheduler *pushing* — the runner advertises capacity and
  takes work, which maps cleanly onto WG providers that *offer* themselves.
- **Trust / X3.** The runner is **fully trusted with the payload**: it checks
  out the source and receives injected secrets. GitHub's own docs warn that
  self-hosted runners should *not* be used on public repositories, because a
  malicious pull request can run arbitrary code on the runner host [GH runner
  security]. So confidentiality from the runner is **zero**. The one bright spot
  is **Buildkite's split**: the SaaS *control plane* orchestrates but never sees
  your code or secrets — those live only on *your* agents [Buildkite security].
  That "coordinator schedules but is blind to payload" property is exactly what
  WG wants for its node-as-coordinator (and is independently achievable via
  Temporal's payload codec, §2.3).
- **Integrity / X4.** None cryptographic — you trust the runner because *it is
  your own infrastructure*. Supply-chain provenance (SLSA, signed build
  attestations) gives **attribution** ("this artifact came from this pipeline"),
  not **integrity against a hostile runner**.
- **Liveness / X5.** Job timeouts plus re-run; **ephemeral / just-in-time
  runners** (one job then deregister) are the modern pattern and bound
  cross-job contamination [GH ephemeral runners].
- **WG fit.** The **pull-claim placement** and **ephemeral-runner** patterns are
  directly reusable. The trust model is the *baseline WG must improve on* —
  these systems only work because the runner is yours.

### 2.2 Cluster schedulers (Kubernetes / Nomad / Mesos) — *bin-packing + the gold-standard lease/reclaim*

- **Placement.** **Kubernetes** pushes: `kube-scheduler` runs filter (predicate)
  then score (priority) over nodes and binds the Pod; the kubelet then pulls and
  runs it [k8s scheduler]. **Nomad** evaluates → plans → places allocations, which
  clients pull [Nomad scheduling]. **Mesos** is the canonical **two-level
  resource-offer** model: the master *offers* resources to framework schedulers,
  which accept and launch tasks via executors [Mesos architecture].
- **Trust / X3.** All three are **single-trust-domain**: every node/kubelet sees
  the workload's memory and secrets. No confidentiality from the node — *unless*
  you add Confidential Containers (§2.6).
- **Integrity / X4.** Trusted — it's your cluster; no anti-cheat.
- **Liveness / X5 — the reference design.** This is what cluster schedulers do
  *best* and what WG should copy wholesale. Kubernetes uses **Node lease**
  objects + heartbeats; a node that stops renewing is marked NotReady and its
  pods are evicted and rescheduled [k8s node lease]. **Nomad** clients
  **heartbeat with a grace window**; a missed heartbeat marks the node down and
  reschedules its allocations [Nomad]. Mesos detects a failed agent and the
  framework relaunches. The "**lease that must be renewed or the work is
  reclaimed**" pattern is the right answer to "a runner takes work then dies."
- **WG fit.** **Lease/heartbeat/reclaim (X5) ★★★** — adopt directly. Bin-packing
  is overkill for WG's coarse task granularity. Neither crux is addressed
  natively; Kubernetes is, however, the host for the *Confidential Containers*
  path that does (§2.6).

### 2.3 Durable workflow / job engines (Temporal / Celery / Sidekiq / Faktory) — *pull-claim queues with leases, and one real confidentiality trick*

- **Placement.** All are **pull**: workers poll a queue/broker and claim tasks
  (Temporal task queues [Temporal]; Celery from RabbitMQ/Redis; Sidekiq from
  Redis; Faktory's `FETCH`). The queue is the matchmaker.
- **Trust / X3.** Workers are trusted (your code, your infra). The brokers
  usually hold task arguments in **plaintext** (Celery's pickle serializer is a
  notorious RCE vector). **Temporal is the exception worth copying**: a custom
  **Data Converter / Payload Codec** encrypts payloads *client-side*, so the
  Temporal **server stores only ciphertext** and data is plaintext **only on the
  client and the worker** you control [Temporal payload codec; Temporal data
  encryption]. That is **confidentiality from the coordinator** — a partial X3
  (it blinds the *durable store/scheduler*, but the executing worker still sees
  plaintext). For WG this is the model for "the node coordinates but cannot read
  the agent's context."
- **Integrity / X4.** None against a hostile worker. Temporal adds **determinism
  enforcement**: workflow code must be deterministic and history **replay**
  detects nondeterminism — but this is a *correctness* guard for your own code,
  not a defense against a lying runner.
- **Liveness / X5 — the second reference design.** Temporal's lease model is the
  most refined here: a task is **locked (leased)** when picked up; **schedule-to-
  start / start-to-close / heartbeat timeouts** govern it; a long activity must
  **heartbeat** or it is declared lost and **retried on another worker** with a
  configurable retry policy [Temporal timeouts]. Celery's **`acks_late` +
  visibility timeout** redelivers on worker death (at-least-once); Faktory
  **reserves** a job with a timeout and **requeues** if not ACKed. These are the
  exact semantics WG needs for "claimed then died."
- **WG fit.** **Temporal is the single best end-to-end reference** for the
  *non-crux* dimensions: pull-claim placement, lease+heartbeat+retry reclaim, and
  **payload-codec confidentiality-from-the-coordinator**. It does not solve X4
  against a hostile *worker* — but in WG's default trusted-pool deployment the
  worker is your own node, so Temporal's posture is close to WG's baseline.

### 2.4 Volunteer / grid compute (BOINC / Folding@home) — *the only mainstream systems whose runner is genuinely untrusted*

- **Placement.** **Pull**: a volunteer client requests work units from the
  project's scheduler [BOINC]. Edge hosts are anonymous and ephemeral.
- **Trust / X3.** The runner is **untrusted** — anyone may attach a machine. But
  **confidentiality is a non-goal**: the science data and application are public,
  so there is nothing to hide *from* the volunteer. (This is why volunteer grids
  do not help with WG's X3 — they sidestep it.) What BOINC *does* protect is the
  reverse direction: the **application binary is code-signed with an offline key**
  so that even a compromised project server cannot push malware to volunteers
  [BOINC code signing] — a useful pattern for WG distributing a trusted agent
  runtime to providers.
- **Integrity / X4 — the canonical answer.** BOINC is the textbook solution to
  "trust a result from a host you don't control": **redundant computing**. Each
  work unit is sent to **multiple hosts (a quorum)**; a **validator** compares the
  returned results (bitwise, or *homogeneous redundancy* / fuzzy comparison to
  tolerate floating-point platform differences); credit and the canonical result
  are granted only on **agreement**; **adaptive replication** sends fewer copies
  to hosts with a track record [BOINC validation]. **Report deadlines** reissue
  unreturned work (X5). Folding@home uses a lighter credit-plus-redundancy scheme.
- **WG fit.** BOINC is the **clearest real-world X4-by-quorum** and **X5-by-
  deadline** in the survey. **The catch (developed in §4.2): quorum integrity
  requires the computation to be *deterministically reproducible*** so that two
  honest hosts produce comparable outputs. An LLM agent's output is **not**
  bitwise-reproducible (sampling, tool nondeterminism, model/version drift), so
  BOINC's exact mechanism does **not** transfer to agent workloads without
  redefining "agreement."

### 2.5 Decentralized compute markets (Bacalhau / Golem / Akash / Fluence / iExec / Ray / Modal / RunPod / E2B / Daytona) — *the closest analogues to "rent someone else's compute"*

These are WG's nearest neighbors: they all **place a job on a machine the
authorizer does not own**. They differ sharply on the two cruxes.

- **Bacalhau** — *compute over data.* A requester posts a (Docker/WASM) job;
  **compute nodes that already hold the data bid** and the job runs *where the
  data lives* [Bacalhau]. **X3:** limited — the node holds the data, so it sees
  it. **X4:** jobs are **content-addressed** (a job ID hashes all code + input),
  and *if execution is deterministic, the same job over the same data yields the
  same output hash on machines that never communicated* [Bacalhau CoD] — so
  verification is **re-run + hash comparison**. The **Lilypad** network builds a
  verifiable compute market on top, adding mediation/re-run on dispute [Lilypad].
  Same determinism caveat as BOINC.
- **Golem** — *open compute market.* Requestors post tasks, providers offer,
  micropayments settle per task. **X3:** limited; Golem has explored **SGX**
  (Graphene/Gramine) for confidential tasks. **X4:** **verification-by-redundancy**
  for amenable task classes (e.g. rendering — compare redundant frames); the
  general case is acknowledged-open [Golem]. Provider untrusted.
- **Akash** — *decentralized cloud.* Providers **bid in a reverse auction** on a
  deployment; the tenant accepts a bid and an **on-chain lease** is created; the
  provider runs the container on its (Kubernetes) infra [Akash]. **X3:** none —
  the provider sees the workload. **X4:** none cryptographic — you trust the
  provider you leased from (reputation / audited providers). The **bid → lease →
  escrow** flow is a strong template for "WG providers are `wgid:` identities you
  lease compute from," and the on-chain lease is an attributable, signed
  placement record (X7).
- **Fluence** — *cloudless DePIN.* Rent VMs from independent providers; providers
  submit **Proof-of-Capacity** to a chain to prove **availability** and earn
  tokens [Fluence]. **X3:** none. **X4:** Proof-of-Capacity proves a provider is
  *online with capacity*, **not** that it ran your job correctly — an availability
  proof, not an execution proof. A cautionary example of a "proof" that does not
  address the integrity crux.
- **iExec** — *confidential compute market.* The standout: tasks run **inside
  Intel SGX enclaves**, and an on-chain **Proof-of-Contribution (PoCo)** consensus
  governs result acceptance [iExec]. **X3 ★★★** (enclave hides data from the
  worker) **and X4 ★★★** (TEE attestation + on-chain consensus). iExec is the
  **single closest exemplar of WG's target**: an *untrusted* market where
  confidentiality and integrity come from **TEE attestation**, not from trusting
  the provider.
- **Ray** — *distributed compute framework* (not a market). A head node + workers;
  the distributed scheduler places tasks/actors across the cluster; lost tasks are
  **re-executed from lineage** [Ray]. **X3/X4:** none — it is one trust domain.
  Relevant as the model for **fanning a single agent's sub-work across a *trusted*
  pool**, and lineage re-execution is a fault-tolerance (not anti-cheat) pattern.
- **Modal / RunPod** — *managed serverless GPU/sandbox fleets.* You push code/a
  container; the operator schedules it on its GPUs with gVisor/Firecracker
  isolation [Modal; RunPod]. **X3/X4:** the **operator is fully trusted** — the
  isolation protects the *operator's host from your code*, not your code from the
  operator. This is the "rent GPU, trust the operator" baseline.
- **E2B / Daytona** — *agent-code sandboxes.* Purpose-built to run **AI-agent-
  generated code** in isolated **Firecracker microVMs** (E2B) or dev containers
  (Daytona) [E2B; Daytona]. **Closest in *purpose* to WG** — but the **threat
  model is inverted**: the sandbox exists to protect the *host* from the agent's
  (possibly malicious) code; it does **not** protect the agent's *context* from
  the host operator. WG needs both directions. The **Firecracker microVM** is,
  however, exactly the isolation primitive WG would use to sandbox a downloaded
  agent locally, and pairs naturally with a TEE for the confidentiality direction.

### 2.6 Confidential computing (SGX/TDX / SEV-SNP / Nitro Enclaves / Confidential Containers) — *the only practical answer to X3, and a strong X4*

This family is the heart of the confidentiality crux. A **TEE** runs code in a
hardware-isolated region whose memory the host OS/hypervisor/operator **cannot
read**, and **remote attestation (RA)** produces a hardware-signed statement of
*exactly which code* is running in a *genuine* TEE.

- **Intel SGX / TDX.** **SGX** isolates an *enclave* (a region of a process);
  enclave memory is encrypted and inaccessible to ring-0/hypervisor; a remote
  party verifies an **attestation quote** (DCAP/EPID) before provisioning secrets
  [Intel SGX; SGX attestation]. **TDX** raises the boundary to a whole **VM
  (Trust Domain)**, so an *unmodified* guest (an agent runtime) runs confidentially
  [Intel TDX]. Caveats: SGX's history of **side-channel attacks** (Foreshadow,
  SGAxe, ÆPIC), a small enclave page cache in older parts, and **trust in Intel's
  attestation root**.
- **AMD SEV-SNP.** Encrypts VM memory and adds **integrity protection** against a
  malicious hypervisor (Secure Nested Paging), with a **signed attestation report**
  rooted in AMD [AMD SEV-SNP]. Whole-VM granularity; widely deployed as Azure/GCP
  **Confidential VMs**. Easiest "lift an existing workload into a TEE."
- **AWS Nitro Enclaves.** Carve an isolated VM from a parent EC2 instance — **no
  persistent storage, no interactive access, no external network, only a vsock**
  to the parent [Nitro Enclaves]. The enclave produces an **attestation document
  with PCR measurements**; the killer integration is **KMS**: a key policy can
  require `kms:RecipientAttestation:PCR`/`ImageSha384`, so **KMS releases the
  data-decryption key *only* if the attestation PCRs match the expected code**, and
  encrypts the data key to the enclave's public key from the attestation document
  [Nitro KMS attestation]. Even the parent-instance admin and AWS operators cannot
  see in. This **"release the secret only to attested code"** flow is the cleanest
  deployable template for WG's X3.
- **Confidential Containers (CoCo).** The CNCF/Kata path: run a **pod inside a TEE
  VM** (SEV-SNP/TDX) on Kubernetes, with a **Key Broker Service + attestation
  service (Trustee / Veraison)** that releases the container image and secrets
  **only after verifying the TEE evidence**; **RA-TLS** binds a TLS channel to an
  attestation [Confidential Containers; RATS]. This is the **most Kubernetes-native,
  most directly deployable** way to get an *attested confidential agent pod* — the
  nearest off-the-shelf shape for a WG confidential worker.
- **X3 / X4 together.** TEEs are the **only family that solves BOTH cruxes at
  once**: the host **cannot read** the context (X3), and the **attestation proves
  the right code ran in a genuine TEE and produced this output** (a strong X4 —
  the runner cannot substitute a different program or forge the result without
  breaking the hardware root). The cost is a **hardware trust root** (Intel/AMD/
  AWS), a **side-channel attack surface**, and operational complexity.
- **WG fit.** **The decisive prior art for X3, ★★★.** WG's confidential-worker
  path is almost certainly *agent-runtime-inside-a-TEE-VM (SEV-SNP/TDX/Nitro) with
  attestation gating context release* — i.e. the principal's UCAN-scoped context
  is sealed to the worker's attestation, exactly like the Nitro→KMS pattern, with
  the `wgid:`/sigchain substrate supplying the identities the attestation binds to.

### 2.7 Verifiable computation (re-run/quorum · optimistic challenge · zkVM) — *the spectrum of answers to X4*

Three techniques, increasing in assurance and cost, for trusting a result from a
hostile runner *without* a TEE.

- **Re-run / quorum (replication).** Dispatch the job to **N independent runners**
  and accept the **majority/unanimous** result. Trust assumption: **honest
  majority** (no collusion). This is BOINC's mechanism and, in the extreme, *every
  blockchain node re-executing every transaction*. Cost: **N×** compute. Simple,
  proven, no special hardware — but **requires deterministic, reproducible
  computation** so honest replicas agree.
- **Optimistic challenge (fraud proofs).** Run **once**, post the result + an
  economic **bond**, and open a **challenge window** in which any verifier may
  dispute. **Truebit** pioneered the off-chain **"verification game"**: solver vs.
  verifiers **bisect** the disputed execution down to a single instruction, which
  is adjudicated cheaply on-chain [Truebit]. **Optimistic rollups** (Arbitrum,
  Optimism) use the same shape with a multi-day challenge window [optimistic
  rollups]. Trust assumption: **≥1 honest verifier** + bonds. Cost: **cheap happy
  path** (no re-run unless challenged) but **adds finality latency** (the window)
  and still **assumes deterministic re-execution** to prove fraud.
- **zkVM / proof-of-execution.** The prover runs a program in a **zero-knowledge
  virtual machine** (**RISC Zero** RISC-V zkVM, **Succinct SP1**, Cairo, Jolt) and
  emits a **succinct cryptographic proof (STARK/SNARK)** that the program executed
  correctly on given inputs producing the claimed output; the verifier checks the
  proof in **milliseconds without re-running** [RISC Zero; SP1]. Trust assumption:
  **none** — integrity is unconditional (no honest-majority, no bond, no window).
  **Two crucial limits for WG:** (1) **it does not give X3 against the runner** —
  zero-knowledge can hide the witness from the *verifier*, but the **prover (the
  runner) still sees the inputs**, so a zkVM alone does not keep context secret
  *from the machine running the agent*; (2) it needs a **deterministic** program
  and imposes **large prover overhead** (historically 10⁴–10⁶×, improving fast).
- **WG fit.** All three are **strong X4 in principle** but share a **fatal
  assumption for agent workloads: deterministic re-execution** (re-run/quorum and
  optimistic compare two runs; zkVM proves a fixed transcript). **LLM-agent output
  is nondeterministic**, so these techniques apply to WG only if the *verifiable
  unit* is redefined (see §4.2) — e.g. proving the *harness* ran a *pinned model*
  with *pinned sampling params over a fixed seed and context*, rather than proving
  "the agent would say the same thing twice." zkVM is the **strongest long-term
  X4** if that reframing is solved; re-run/quorum is the **cheapest near-term**
  option for the *deterministic* parts of a task (tool calls, builds, tests).

---

## 3. Decentralization spectrum placement

```
 FULLY UNTRUSTED / DECENTRALIZED  <--------------------------------------->  TRUSTED / CENTRAL
 (open market, runner = adversary)      (federated pools)        (your own cluster / operator)

 zkVM ── BOINC/F@h ── Golem ── iExec ── Bacalhau/Lilypad ── Fluence ── Akash ── Buildkite ── GH/GitLab runners ── Temporal ── K8s/Nomad/Mesos ── Ray ── Modal/RunPod/E2B
   │        │           │        │           │                │          │         │              │                  │            │                  │
  proof   quorum     redundant  TEE+      determ.+CAS       avail.    on-chain   SaaS plane    pull-claim,        lease/        bin-pack,         operator-
  only    +deadline  +SGX       PoCo      re-run            proof     lease      blind to      ephemeral         heartbeat,    single trust      trusted
                                                                                 payload       runners           durable       domain            fleet

 Cross-cutting (techniques/primitives — no inherent placement; they ride on a scheduler):
   TEEs: SGX/TDX · SEV-SNP · Nitro Enclaves · Confidential Containers  → solve X3 (+strong X4) at ANY point on the line
   Verifiable compute: re-run/quorum · optimistic challenge · zkVM     → solve X4 at ANY point (deterministic work only)
```

**Reading the spectrum for WG.** WG-Fed already decided the federation posture
(HQ6): **trust root never central, availability rests on an optional node**.
Transposed to execution, WG's **default** target zone is the **center-right** —
a **federated *trusted pool*** of `wgid:`-authenticated nodes (your own
household/org machines and those of peers you trust), placed via a **pull-claim /
lease** model borrowed from Temporal + cluster schedulers. The **left half** (open
markets with adversarial runners) is where the two cruxes bite hardest, and is
reachable only by **adding a TEE (for X3) and/or verifiable compute (for X4)** —
the cross-cutting primitives that can be dropped in at *any* point on the line.
That is exactly the **trusted-pool ↔ market ↔ confidential ↔ hybrid** axis
`exec-architectures` (4/6) must develop: the trusted pool is the cheap default;
the confidential/market end is unlocked by TEE + verifiable-compute when you want
to place on compute you do *not* trust.

---

## 4. Special focus — the two cruxes

### 4.1 Context confidentiality on untrusted compute (X3)

> *Can a runner **run** the agent without being able to **read** the principal's
> private context (its loaded state, the principal's data, the conversation)?*

Ranked by how directly each system solves it:

| Rank | System / technique | Mechanism | Why it fits X3 |
|---|---|---|---|
| ★★★ | **Nitro Enclaves** | Isolated VM; **KMS releases the data key only to attested PCRs** | Cleanest deployable "secret released only to attested code"; parent + cloud operator barred. |
| ★★★ | **AMD SEV-SNP / Intel TDX** | Whole-VM memory encryption + integrity vs. hypervisor; signed attestation | An **unmodified agent runtime** runs confidentially; the host operator cannot read memory. |
| ★★★ | **Confidential Containers** | Pod in a TEE VM; **RA-gated** image/secret release (Trustee/Veraison) | The k8s-native, most directly deployable *attested confidential agent pod*. |
| ★★★ | **Intel SGX (+ iExec)** | Process-level enclave; attestation before secret provisioning | iExec proves the **"untrusted market + TEE confidentiality"** model works end-to-end. |
| ★★☆ | **Temporal payload codec** / **Buildkite split** | Client-side encryption; **coordinator stores only ciphertext** | Solves confidentiality **from the scheduler/coordinator** — but the executing *worker* still sees plaintext. Partial: blinds the broker, not the runner. |
| ☆☆☆ | **FHE / MPC** *(frontier, not surveyed in depth)* | Compute on ciphertext (FHE) / split across non-colluding parties (MPC) | The only way to hide data from the runner **without trusting hardware** — but **orders-of-magnitude overhead**; impractical for general agent workloads today. |
| ☆☆☆ | **Everything else** (CI runners, k8s/Nomad/Mesos, Celery/Sidekiq, Akash/Fluence/Golem, Modal/RunPod, E2B/Daytona, Ray, BOINC) | — | **Fail X3** — the runner sees the plaintext. Volunteer grids *sidestep* it (public data); markets and sandboxes *trust the operator*. |

**Synthesis for X3.** **Only TEEs solve context confidentiality on genuinely
untrusted compute** in a practical way today. Everything else either (a) trusts
the runner (CI, schedulers, markets, sandboxes), (b) has no secret to hide
(volunteer grids), or (c) only blinds the *coordinator* while the *worker* still
sees plaintext (Temporal/Buildkite). FHE/MPC are the only non-hardware answers
and remain too slow for agent workloads. **WG's confidential-worker path is
therefore a TEE-attested agent runtime** (SEV-SNP/TDX/Nitro/CoCo), with the
principal's UCAN-scoped context **sealed to the worker's attestation** (the
Nitro→KMS pattern) — and a **fallback to "trusted-pool only"** when no TEE is
available. Crucially, this dovetails with WG-Fed **S-5 (loaded state is untrusted
input)**: the TEE protects the *context from the host*, while S-5's
provenance-gating protects the *host/agent from poisoned context* — the two
directions are independent and **both** are needed.

### 4.2 Result integrity against a hostile runner (X4)

> *Can I trust the output without trusting the runner?*

| Rank | System / technique | Mechanism | Trust assumption / cost |
|---|---|---|---|
| ★★★ | **zkVM / proof-of-execution** | Succinct cryptographic proof of correct execution; verify in ms | **None** (unconditional). Cost: heavy prover; **deterministic programs only**; no X3 vs runner. |
| ★★★ | **TEE attestation** (SGX/TDX/SEV-SNP/Nitro/CoCo) | Hardware-signed quote: *this measured code ran in a genuine TEE → this output* | Trust the **hardware root** (Intel/AMD/AWS) + side-channel surface. **Also gives X3.** |
| ★★☆ | **Re-run / quorum** (BOINC, Lilypad, replication) | N independent runs; accept agreement | **Honest majority**, no collusion. Cost **N×**. **Deterministic work only.** |
| ★★☆ | **Optimistic challenge** (Truebit, OP-rollups) | Run once + bond; fraud proof in a challenge window | **≥1 honest verifier** + bonds. Cheap happy path; **adds finality latency**; deterministic re-exec. |
| ★☆☆ | **Bacalhau content-addressed re-run** | Same code+data → same output hash; re-run to verify | Determinism. Good for pipelines, **breaks on nondeterministic agents**. |
| ★☆☆ | **Reputation / audited providers** (Akash) | Trust providers with a track record | Social/economic, not cryptographic. |
| ☆☆☆ | **Proof-of-Capacity** (Fluence) / **SLSA provenance** (CI) | Prove *availability* / *attribution* | **Not** execution integrity — a common trap: an availability proof or a provenance signature is *not* a correctness proof. |
| ☆☆☆ | **Everything trusted-domain** (k8s/Nomad/Mesos, Temporal/Celery/Sidekiq, Modal/RunPod, E2B, Ray) | — | **No X4 vs a hostile runner** — they assume the runner is yours. |

**Synthesis for X4 — and the single most important fit insight in this survey.**
Every mature integrity technique (re-run/quorum, optimistic challenge, zkVM)
**assumes the computation is deterministically reproducible** — two honest runs
must be comparable, or a transcript must be fixed. **An LLM agent's output is not
bitwise-reproducible**: sampling temperature, tool-call nondeterminism, model and
prompt drift, and wall-clock/context differences all break "run it twice and
compare." This means **the volunteer-grid / blockchain integrity playbook does
not transfer to agent workloads unchanged.** WG has three viable paths:

1. **TEE attestation (recommended primary).** The attestation proves *the
   expected harness ran the expected model in a genuine enclave and produced this
   output* — it certifies the **process**, not a reproducible **result**, which is
   exactly right for nondeterministic agents. This is the **only ★★★ X4 that also
   delivers X3**, and the only one that does not require determinism.
2. **Decompose into deterministic sub-units.** A task's *deterministic* parts
   (builds, tests, tool invocations, file diffs) **can** use re-run/quorum or even
   zkVM; the *nondeterministic* LLM step is the part that needs the TEE or human/
   trusted-pool acceptance. WG's task graph already has this structure (an agent's
   work culminates in checkable artifacts — `cargo test`, a diff), so **integrity
   can attach to the verifiable artifact rather than the chat transcript.**
3. **Attribution + trusted-pool default.** In the default deployment the runner is
   a `wgid:` node at sufficient `trust_level`; integrity is **attribution** (the
   result is signed by the worker's UCAN, attributable to agent + principal) plus
   **trust in the pool** — the same posture as CI/schedulers, but now *explicitly
   gated by `trust_level`* rather than implicitly by "it's my box."

The clean separation: **TEE attestation is WG's answer for the *untrusted*
runner; signed-attribution-over-a-trusted-pool is the default; re-run/quorum and
zkVM are reserved for the *deterministic* sub-work** where they actually apply.

---

## 5. Synthesis — what WG should borrow from whom

No single system satisfies WG's execution needs; the answer is a **layered
composition**, which `exec-architectures` (4/6) will develop into the
**trusted-pool ↔ market ↔ confidential ↔ hybrid** candidate set.

| WG execution layer | Best prior art to borrow | Why |
|---|---|---|
| **Placement (X1)** | **CI pull-claim** (GitHub/Buildkite/GitLab) + **Akash bid→lease** | Providers *advertise/offer* and claim work; Akash's reverse-auction lease maps onto `wgid:` providers and yields a signed placement record (X7). |
| **Trust + capability gating (X2)** | **WG-Fed UCAN** (already decided) + **BOINC offline-signed app** | Worker carries a scoped, expiring UCAN; the agent *runtime* WG ships to a provider is code-signed so a compromised coordinator can't push a malicious harness. |
| **Lease / heartbeat / reclaim (X5)** | **Temporal** (lease + heartbeat + retry) + **k8s/Nomad** (node lease, grace) | The most refined "claimed-then-died" semantics in the survey; adopt directly. |
| **Context confidentiality (X3 — CRUX)** | **Nitro Enclaves / SEV-SNP / TDX / Confidential Containers** (+ **iExec** as the market exemplar) | The only practical way to run an agent on untrusted compute without leaking the principal's context; seal context to the worker's attestation (Nitro→KMS pattern). |
| **Confidentiality from the *coordinator*** | **Temporal payload codec** / **Buildkite split** | The WG node can schedule and store-and-forward an agent's context as **ciphertext** it cannot read — confidentiality from the node even before a TEE is involved. |
| **Result integrity (X4 — CRUX)** | **TEE attestation** (primary) · **BOINC quorum** + **zkVM/Truebit** (for the deterministic sub-units) | Attestation certifies the *process* for nondeterministic agents; quorum/proofs attach to the *checkable artifacts* (builds/tests/diffs), not the transcript. |
| **Isolation primitive** | **E2B/Daytona Firecracker microVMs** | The sandbox WG uses to run a downloaded agent locally; pairs with a TEE for the confidentiality direction (note the inverted threat model — WG needs both directions). |
| **Fan-out across a trusted pool** | **Ray** (distributed scheduler, lineage re-exec) | How to spread one agent's parallel sub-work across `wgid:` nodes in the trusted pool. |
| **Decentralization posture** | **WG-Fed HQ6** (trust root never central) | Trusted-pool default; the market/confidential end is opt-in via TEE + verifiable compute. |

**Headline takeaway.** The two cruxes have a clean, asymmetric answer in the
prior art:

- **X3 (context confidentiality on untrusted compute) is solved *only* by TEEs.**
  Every non-TEE system either trusts the runner, has nothing to hide, or only
  blinds the coordinator. **iExec** proves a TEE-backed *untrusted market* is real;
  **Nitro/SEV-SNP/TDX/Confidential Containers** are the deployable substrates.
- **X4 (result integrity vs a hostile runner) has a spectrum** — re-run/quorum
  (cheap, honest-majority), optimistic challenge (cheaper, ≥1-honest), zkVM
  (unconditional, expensive) — **but every one assumes deterministic
  re-execution, which LLM agents violate.** So WG's primary X4 is **TEE
  attestation of the process** (which also gives X3), with quorum/proofs reserved
  for a task's **deterministic, checkable artifacts**, and **signed attribution
  over a `trust_level`-gated pool** as the default.

The two cruxes therefore **converge on the same primitive**: a **TEE-attested
agent runtime** is the one mechanism that delivers *both* confidentiality and
integrity against a runner you do not trust — and it composes directly with the
WG-Fed substrate (the attestation binds to `wgid:` identities; the context is
sealed under the worker's UCAN). The **trusted pool** (Temporal/k8s placement +
UCAN gating + signed attribution) is the cheap default; the **TEE path** is the
unlock for placing an agent on compute you do not own. That is the design space
`exec-architectures` must lay out as **trusted-pool ↔ market ↔ confidential ↔
hybrid**.

---

## 6. Sources (specs & primary docs)

- **GitHub Actions self-hosted runners** — about/architecture (long-poll job
  assignment): <https://docs.github.com/en/actions/hosting-your-own-runners/managing-self-hosted-runners/about-self-hosted-runners>
  · security hardening (don't use on public repos; ephemeral/JIT runners):
  <https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions>
- **Buildkite** — architecture & security ("your code never touches our
  servers"; agent polls control plane): <https://buildkite.com/docs/agent/v3>
  · <https://buildkite.com/docs/pipelines/security>
- **GitLab Runner** — executors & job-request (`request_job`) flow:
  <https://docs.gitlab.com/runner/> · <https://docs.gitlab.com/runner/security/>
- **Kubernetes** — scheduler (filter/score): <https://kubernetes.io/docs/concepts/scheduling-eviction/kube-scheduler/>
  · node heartbeats / **Node lease**: <https://kubernetes.io/docs/concepts/architecture/nodes/#heartbeats>
- **HashiCorp Nomad** — scheduling (eval→plan→alloc) & client heartbeats:
  <https://developer.hashicorp.com/nomad/docs/concepts/scheduling/scheduling>
- **Apache Mesos** — architecture (two-level resource offers):
  <https://mesos.apache.org/documentation/latest/architecture/>
- **Temporal** — Payload Codec / Data Converter (client-side E2E encryption,
  server sees only ciphertext): <https://docs.temporal.io/payload-codec> ·
  <https://docs.temporal.io/production-deployment/data-encryption> · timeouts &
  heartbeats (lease/retry): <https://docs.temporal.io/encyclopedia/detecting-activity-failures>
- **Celery** — `acks_late` / visibility timeout (at-least-once redelivery):
  <https://docs.celeryq.dev/en/stable/userguide/configuration.html>
- **Sidekiq / Faktory** — Faktory job reservation/requeue protocol:
  <https://github.com/contribsys/faktory/wiki> · Sidekiq reliability:
  <https://github.com/sidekiq/sidekiq/wiki>
- **BOINC** — redundant computing / validators / homogeneous redundancy / adaptive
  replication: <https://boinc.berkeley.edu/trac/wiki/ValidationSimple> ·
  code signing: <https://boinc.berkeley.edu/trac/wiki/CodeSigning>
- **Folding@home** — <https://foldingathome.org/support/faq/>
- **Bacalhau** — compute over data; content-addressed deterministic jobs:
  <https://docs.bacalhau.org/> · vision (deterministic job hashes):
  <https://news.ycombinator.com/item?id=35302608> · **Lilypad** verifiable market:
  <https://docs.lilypad.tech/lilypad>
- **Golem** — task computation & SGX/Graphene confidential tasks:
  <https://docs.golem.network/> · <https://blog.golemproject.net/>
- **Akash** — reverse-auction marketplace, bids & on-chain leases:
  <https://docs.akash.network/> (marketplace/lease lifecycle)
- **Fluence** — Cloudless/DePIN marketplace & Proof-of-Capacity:
  <https://www.fluence.network/> · <https://fluence.dev/docs/>
- **iExec** — confidential compute (Intel SGX) + Proof-of-Contribution (PoCo):
  <https://docs.iex.ec/> · <https://protocol.docs.iex.ec/>
- **Ray** — architecture, distributed scheduler & lineage re-execution:
  <https://docs.ray.io/en/latest/ray-core/scheduling/index.html>
- **Modal / RunPod** — serverless GPU/sandbox execution:
  <https://modal.com/docs> · <https://docs.runpod.io/>
- **E2B / Daytona** — agent-code sandboxes (Firecracker microVMs / dev envs):
  <https://e2b.dev/docs> · <https://www.daytona.io/docs>
- **Intel SGX / TDX** — SGX & attestation (DCAP): <https://www.intel.com/content/www/us/en/developer/tools/software-guard-extensions/overview.html>
  · TDX: <https://www.intel.com/content/www/us/en/developer/tools/trust-domain-extensions/overview.html>
- **AMD SEV-SNP** — Secure Nested Paging whitepaper:
  <https://www.amd.com/system/files/TechDocs/SEV-SNP-strengthening-vm-isolation-with-integrity-protection-and-more.pdf>
- **AWS Nitro Enclaves** — cryptographic attestation & **KMS attestation-gated key
  release** (PCR condition keys): <https://docs.aws.amazon.com/enclaves/latest/user/set-up-attestation.html>
  · <https://docs.aws.amazon.com/enclaves/latest/user/kms.html>
- **Confidential Containers** — CoCo + Trustee/Key Broker; RA-TLS:
  <https://confidentialcontainers.org/> · IETF **RATS** architecture (RFC 9334):
  <https://www.rfc-editor.org/rfc/rfc9334> · Veraison: <https://github.com/veraison>
- **Truebit** — verification game (interactive fraud proof) whitepaper:
  <https://people.cs.uchicago.edu/~teutsch/papers/truebit.pdf>
- **Optimistic rollups** — fraud proofs & challenge windows:
  <https://docs.arbitrum.io/how-arbitrum-works/fraud-proofs/challenge-manager> ·
  <https://community.optimism.io/docs/protocol/2-rollup-protocol/>
- **zkVM / proof-of-execution** — RISC Zero: <https://dev.risczero.com/api> ·
  Succinct SP1: <https://docs.succinct.xyz/>
- **FHE / MPC** (frontier, for X3 without hardware) — overview:
  <https://en.wikipedia.org/wiki/Homomorphic_encryption> ·
  <https://en.wikipedia.org/wiki/Secure_multi-party_computation>

---

*Wave-1 gather phase complete. `exec-architectures` (4/6) should develop §5's
layered composition into the **trusted-pool ↔ market ↔ confidential ↔ hybrid**
candidate set, with the TEE-attested agent runtime as the load-bearing primitive
that solves both cruxes, the Temporal/k8s lease model for X5, and the WG-Fed
`wgid:`/UCAN substrate underneath. The key constraint to carry forward: **agent
output is nondeterministic, so the deterministic-replay integrity playbook
(quorum/optimistic/zkVM) attaches to a task's checkable artifacts, not to the
agent's transcript — and TEE attestation certifies the process, which is the
right shape for X4 against a hostile runner.***
