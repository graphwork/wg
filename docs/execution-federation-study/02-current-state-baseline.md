# Execution Federation 2/6 — Current-State Baseline

**Task:** `exec-baseline` (wave 1, task 2 of 6 — gather phase)
**Date:** 2026-06-25
**Baseline ref:** current `main` (this worktree branched from `7a9b5d30`)
**Depends on:** the WG-Fed identity/capability substrate study
(`docs/federation-study/*`, esp. `02-current-state-baseline.md`). Execution
federation **sits on top of** that layer — providers must be *identities*,
workers must be *scoped-capability holders*. Where that substrate is absent,
this document says so and treats it as a hard prerequisite, not a parallel
concern.

> **Purpose.** Honest "where we are *today*" for **federating execution** —
> decoupling *"a task is approved to run"* (a graph/scheduling fact) from
> *"which machine runs the agent"* (a placement fact). Every current capability
> below is cited to a real `file:line` on current `main`. The output is a
> **seed-vs-missing** ledger: what WG already has that a federated execution
> layer can stand on, vs what is entirely unbuilt.

---

## 0. TL;DR — the one-paragraph truth

WG today runs **every agent as a local subprocess on the dispatcher's own host,
inside a git worktree that symlinks the one shared `.wg/` graph**. The
dispatcher (`coordinator.rs`) selects ready tasks and *directly forks them* up
to a fixed `max_agents` slot count (`spawn_agents_for_ready_tasks`,
`coordinator.rs:3924`); there is **no placement axis** — the only "where" the
spawn decision answers is *which subprocess binary* (claude/codex/nex/pi), never
*which machine*. What WG *does* already have, and what makes a federation story
plausible, is a genuine **lease lifecycle**: `claim` flips a task to
`InProgress`+`assigned` (`claim.rs:13`), a background loop renews a **heartbeat**
every 120 s (`execution.rs:1918`), an **agent registry** tracks liveness
(`registry.rs:60`), and on heartbeat-timeout the **sweep/dead-agents** reclaimers
unclaim the task back to `Open` so the dispatcher re-dispatches it
(`dead_agents.rs:93`, `sweep.rs reconcile_orphaned_tasks`). Plus a real
**handler abstraction** (`handler_for_model.rs:87`) that already routes a task to
a *remote LLM API* (Anthropic, OpenAI, OpenRouter) — but the *agent process and
all its tool calls / file edits still execute locally*. So WG federates
**inference** already, but **not execution**. Every cross-machine primitive a
federated-execution layer needs — remote placement, providers as `wgid:`
identities, capability-gated run authorization, trust-gated placement,
result verification across a trust boundary, shipping the task's input/graph-slice
to a remote provider — is **absent**, and all of them depend on the WG-Fed
identity/capability substrate which (per `fed-baseline`) **does not exist yet**.
The claim → heartbeat → reclaim loop is the **seed**; lifting it across a trust
boundary is the **whole job**.

---

## 1. Method & scope

Audited subsystems (with the modules that actually implement them on `main`):

| Subsystem | Primary module(s) | LoC |
|---|---|---|
| Dispatch decision (coordinator) | `src/commands/service/coordinator.rs` | 7241 |
| Spawn path (worktree / agent dir / env / wrapper) | `src/commands/spawn/{execution,worktree,context,mod}.rs` | 8512 |
| Handler/executor abstraction | `src/dispatch/handler_for_model.rs`, `src/dispatch/plan.rs`, `src/commands/pi_handler.rs` | 265 + 1740 + ~700 |
| Claim/spawn surface | `src/commands/{claim,exec,reclaim}.rs`, `src/commands/spawn/mod.rs` | 526 + 699 + 231 |
| Liveness (registry / heartbeat / sweep / reclaim) | `src/service/registry.rs`, `src/commands/{heartbeat,sweep,dead_agents,reclaim}.rs` | 1349 + 309 + 1007 + 731 + 231 |
| Tool/context controls | `src/config.rs` (`ExecMode`), `src/context_scope.rs`, `src/commands/spawn/execution.rs` | — |
| Result verification | `src/commands/done.rs`, `src/agency/*` (auto_evaluate / FLIP) | — |

**Framing convention.** Throughout, "remote provider" in WG's *current* sense
means a remote **LLM API endpoint** (the model runs elsewhere). The federation
study's sense — a remote **execution host** that runs the agent process itself —
is what is *missing*. Keeping those two senses distinct is the single most
important thing this baseline does.

---

## 2. Subsystem audit (cited to current `main`)

### 2.1 Dispatch decision — `src/commands/service/coordinator.rs`

**What it is: a single-host, slot-limited fork loop. It decides *whether* and
*what* to spawn, never *where*.**

- **Capacity gate.** `cleanup_and_count_alive()` (`coordinator.rs:44`) counts
  agents that are `is_alive()` **and** whose PID is still running locally
  (`is_process_alive`, `coordinator.rs:119`); if `alive_count >= max_agents` it
  returns early (`:122`). `max_agents` defaults to **8** (`config.rs:3699`). The
  number of dispatchable "slots" is purely `max_agents - alive_count`
  (`slots_available`, `coordinator.rs:4489`). **Capacity is a local process
  count, not a fleet/placement model.**
- **Ready-task selection.** `check_ready_or_return()` (`:162`) computes
  `ready_tasks_with_peers_cycle_aware()` and filters out daemon-managed loop
  tasks (`DAEMON_MANAGED_TAGS`, `:144`). Selection is **priority-sorted with
  starvation prevention** (`sort_tasks_by_priority_with_features`,
  `coordinator.rs:3943`) — a scheduling decision over *one* graph.
- **The spawn loop.** `spawn_agents_for_ready_tasks()` (`coordinator.rs:3924`)
  walks sorted ready tasks; for each, up to `slots_available`, it skips
  already-`assigned` tasks (`:3950`), applies a respawn throttle (`:3960`) and a
  spawn circuit-breaker (`:3966`), then resolves `{executor, model, endpoint}`
  through **`plan_spawn` — explicitly documented as "the ONLY place that decides
  {executor, model, endpoint} for a task spawn"** (`coordinator.rs:4133–4153`),
  and calls `spawn::spawn_agent(...)` (`:4181`). **`plan_spawn` has no host /
  placement parameter** — it answers handler+model+endpoint only (see §2.3).
- **Inline fast paths.** Shell tasks (`exec_mode == "shell"`) fork
  `spawn_shell_inline` (`:4001`); evaluation/flip/assignment tasks fork inline
  (`spawn_eval_inline:3110`, `spawn_assign_inline:3320`). All are **local
  forks**.

> **Dispatch verdict:** a competent local scheduler — priority, starvation,
> throttle, circuit-breaker, capacity — bound to **one machine and one graph**.
> The decision it makes is "approve-and-run-here"; "approve" and "run-here" are
> fused. Federation's first job is to *split* them.

---

### 2.2 Spawn path — `src/commands/spawn/`

**What it is: per-agent git-worktree isolation + an env-var contract + a bash
wrapper, all on the local filesystem.** This is the richest seed in the codebase.

**(a) Worktree isolation — `worktree.rs:29`.** `create_worktree()` runs
`git worktree add .wg-worktrees/<agent-id> -b wg/<agent-id>/<task-id> HEAD`
(`worktree.rs:52`), then **symlinks the project's `.wg/` into the worktree**
(`create_workgraph_link`, `:70`/`:91`) so the `wg` CLI works from inside. Cargo
target dirs are isolated per-worktree (`CARGO_TARGET_DIR`, `execution.rs:656`).
Worktrees are **"sacred"** — never auto-overwritten (`:42`), preserved even on
spawn failure (`execution.rs:803`).
**The symlink is the load-bearing coupling for federation:** every agent edits a
*different working tree* but reads/writes the *same `.wg/` graph by shared
filesystem*. A remote host cannot symlink your `.wg/`; this is exactly the
boundary federation must replace with a shipped graph-slice + a result channel.

**(b) Agent dirs + registration.** A temp agent id is minted from the registry's
`next_agent_id` (`execution.rs:329`); on successful fork the child PID is
registered via `register_agent_with_model(pid, task_id, executor, output_file,
model)` (`execution.rs:821`, registry at `registry.rs:341`) and the worktree path
recorded (`set_worktree_path`, `:832`). **Identity is a local autoincrement
`agent-N`, not a key** (`registry.rs:349`).

**(c) Env-var contract — `execution.rs:603–656`.** The spawned wrapper receives:
`WG_TASK_ID` (`:603`), `WG_AGENT_ID` (`:604`), `WG_EXECUTOR_TYPE` (`:605`),
`WG_TASK_TIMEOUT_SECS`/`WG_SPAWN_EPOCH` (`:608`/`:610`), `WG_USER` (`:619`),
`WG_MODEL` (`:621`), `WG_TIER` (`:632`), `WG_ENDPOINT`/`WG_ENDPOINT_NAME`
(`:635`), `WG_LLM_PROVIDER` (`:639`), `WG_ENDPOINT_URL` (`:642`), the API key
(`inject_api_key_env`, `:644`), and worktree coordinates `WG_WORKTREE_PATH` /
`WG_BRANCH` / `WG_PROJECT_ROOT` / `WG_WORKTREE_ACTIVE` (`:649–654`). **This is a
self-contained task-execution descriptor** — almost everything a remote runner
would need *except* the graph data itself (which is reached via the `.wg/`
symlink, not passed). The API key is injected by **env only, never argv**
(mirrored in the pi handler, `pi_handler.rs:465`).

**(d) The wrapper script — `execution.rs` `write_wrapper_script`.** A bash
wrapper (built ~`:1776+`) that: starts a **background heartbeat loop** —
`(while kill -0 $$; do sleep 120; wg heartbeat "$WG_AGENT_ID"; done) &`
(`execution.rs:1918–1924`); runs the executor under an optional `timeout`
wrapper (`:563`); on exit stops the heartbeat (`:1931`); then checks whether the
task is still `InProgress` and (for some executors) captures the stream
(`pi-stream-bridge`, see CLAUDE.md). **Detached via `setsid()`** so the agent
survives daemon restart (`execution.rs:666`). The agent reports liveness *to
itself* by writing the shared registry — a trusted-host assumption.

> **Spawn verdict:** the strongest seed. Worktree isolation, a clean env-var
> task descriptor, a heartbeat self-renewal loop, and detached lifetime are
> *exactly* the primitives a remote runner needs — but every one of them assumes
> a **shared local filesystem and a trusted local `wg` binary** that writes the
> one true registry/graph.

---

### 2.3 Handler / executor abstraction — `handler_for_model.rs`, `plan.rs`, `pi_handler.rs`

**What it is: a model-spec → local-subprocess router. It already lets the *model*
live on a remote API, but the *agent process* is always local.**

- **`handler_for_model(model) -> ExecutorKind`** (`handler_for_model.rs:87`) is
  documented as **"the ONE function"** mapping a model spec to the internal
  handler subprocess (`:21`). The **leading token is always a handler**
  (claude / codex / nex / pi / opencode / …); everything after the first `:` is
  that handler's native dialect (`:28`). Claude/codex/pi are **delegate-to-CLI**;
  native/nex is **in-process** (`:9`). The two real axes are *delegate-vs-in-
  process* and *wire protocol* (`:9–12`) — **"which host" is not an axis.**
- **`plan_spawn`** (`dispatch/plan.rs`, called at `coordinator.rs:4148`) is the
  single source of truth for `{executor, model, endpoint}`. It resolves the
  executor floor, agency's preferred executor, and a model-compat override
  (claude→native for non-Anthropic models). **No placement / host field exists in
  the plan.**
- **The pi handler** (`pi_handler.rs`) spawns `pi --mode rpc` as a **local child
  process** through a PTY terminal host (`RpcTransport::spawn`,
  `pi_handler.rs:444`/`:478`), hermetically loading the embedded plugin by
  absolute path. The model it talks to may be remote (OpenRouter), but **the pi
  process, its tool calls, and its file edits are local.**

> **Handler verdict:** WG already **federates inference** — a task can be served
> by a remote model API behind any of several handlers. It does **not federate
> execution** — the handler subprocess, the tool sandbox, and the working tree
> are always on the dispatcher's host. The cleanest place to *add* a placement
> axis is here: a `wgid:`/remote-runner handler would slot in beside claude/pi as
> a new `ExecutorKind` (the module is explicitly designed for new arms, `:64`),
> but its semantics (ship work out, get a signed result back) are categorically
> different from "spawn a local CLI."

---

### 2.4 Claim / spawn / exec surface — `claim.rs`, `spawn/mod.rs`, `exec.rs`

**What it is: the pull/manual/inline entry points. All mutate one local graph.**

- **`wg claim` (pull)** — `claim(dir, id, actor)` (`claim.rs:13`) is a pure
  graph-status transition: gated by current status (only `Open | Blocked |
  Incomplete` may be claimed, `:33`), it sets `status = InProgress`,
  `started_at`, optional `assigned = actor`, and appends a log entry
  (`:112–127`). **No lease token, no lock to a host, no heartbeat coupling** —
  the "claim" is a field in `graph.jsonl`, enforced only by everyone sharing the
  file. `wg unclaim` reverses it (`claim.rs:159`). This optimistic,
  status-as-lease design is the conceptual seed for a *distributed* lease.
- **`wg spawn` (manual)** — `Commands::Spawn` (`main.rs:2308`) → `spawn::run` →
  the same `spawn_agent` local path the coordinator uses (`spawn/mod.rs:156`).
  Manual and automatic dispatch share one code path; both are local-only.
- **`wg exec`** — `exec::run` (`exec.rs:26`) is the "optional exec helper": for a
  task with a `task.exec` shell command, it claims, runs the command locally
  (with optional worktree + scope context), and marks done/fail by exit code. A
  **no-LLM local execution** path.

> **Claim/exec verdict:** "claim" is already an *abstraction over who is allowed
> to run a task* — but it is enforced by shared-filesystem honesty, addresses the
> actor as a free-text `assigned` string, and grants no scoped authority. It is
> the right *shape* for a capability-gated, cross-host claim and the wrong
> *substrate*.

---

### 2.5 Liveness — registry, heartbeat, sweep, dead-agents, reclaim

**What it is: a real lease lifecycle (claim → renew → expire → reclaim),
implemented over one local registry file.** This is the single most important
seed for federation and the section the downstream architectures will build on.

- **The registry — `registry.rs`.** `AgentEntry` (`:60`) holds `id`, `pid`,
  `task_id`, `executor`, `started_at`, `last_heartbeat`, `status`,
  `output_file`, `model`, `completed_at`, `worktree_path`. `AgentStatus`
  (`:37`) is a lifecycle enum (`Starting | Working | Idle | Stopping | Parked |
  Frozen | Done | Failed | Dead`). Stored at `.wg/service/registry.json`,
  written **atomically** (temp+rename, `:220`) under an **exclusive flock**
  (`load_locked`, `:255`). **PID-based liveness ties an agent to a local
  process** — `is_live()` requires status-alive **and** `is_process_alive(pid)`
  **and** a fresh heartbeat (`registry.rs:118`). *That PID check is meaningless
  across a machine boundary* and is the first thing federation must replace.
- **Heartbeat — `heartbeat.rs` + the wrapper loop.** `wg heartbeat <agent-id>`
  updates `last_heartbeat` to now (`heartbeat.rs:10` → `registry.update_heartbeat`,
  `registry.rs:484`); the spawn wrapper renews it every **120 s**
  (`execution.rs:1921`). The expiry threshold (`heartbeat_timeout`) defaults to
  **5 minutes** (`config.rs:3966`). 120 s renewal under a 300 s lease = a
  textbook lease/renewal ratio.
- **Expiry detection.** `find_dead_agents(timeout_secs)` / `mark_dead_agents`
  (`registry.rs:520`/`:535`) select alive agents whose
  `seconds_since_heartbeat > timeout` and flip them to `Dead`.
- **Reclaim — `dead_agents.rs` + `sweep.rs`.** `wg dead-agents --cleanup`
  (`run_cleanup`, `dead_agents.rs:93`) marks dead agents, archives their output,
  and **unclaims their tasks back to `Open`** (`:156`) so the dispatcher
  re-dispatches. `wg sweep` (`find_orphaned_tasks` / `reconcile_orphaned_tasks`,
  `sweep.rs:57`) is the reconciliation safety net for `InProgress`
  (or `Open`-with-stale-claim) tasks whose agent is `Dead`/missing — also called
  inline every dispatcher tick (`coordinator.rs:61`). `wg reclaim`
  (`reclaim.rs:16`) is the *manual* takeover: reassign an `InProgress` task from
  one actor to another with a "(agent takeover)" log entry.

> **Liveness verdict:** WG has, in all but name, a **lease manager**: claim =
> lease acquire, heartbeat = lease renew, `heartbeat_timeout` = lease TTL,
> `mark_dead` + unclaim = lease expiry + requeue, `reclaim` = forced steal. The
> mechanics are sound. The substrate is not: liveness is proven by a **local
> PID** and a **locally-written registry**, both unforgeable only because every
> writer shares one trusted host. Across a trust boundary the PID is invisible
> and the heartbeat is just an unsigned string anyone could write.

---

### 2.6 Tool / context controls — `ExecMode`, `ContextScope`

**What it is: a local capability-restriction knob (which tools the agent may
call) and a context-sizing knob (how much graph the agent sees). Both are
enforced by the *trusted local dispatcher* configuring the *trusted local CLI*.**

- **`ExecMode` — `config.rs:1304`.** Four tiers, lightest→heaviest: `Shell`
  (no LLM, run `task.exec` via bash), `Bare` (LLM with `Bash(wg:*)` only),
  `Light` (read-only file tools: `Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch`),
  `Full` (all tools). Resolution is `task.exec_mode > role.default_exec_mode >
  "full"` (`context.rs:1080`). The tiers are **realized as CLI flags** —
  `--allowedTools` / `--disallowedTools` on the spawned executor
  (`execution.rs:1438`, `:1471`, `:1685`). **This is a genuine capability-gating
  primitive** — but it gates *the local tool sandbox*, decided by a dispatcher
  that trusts its own CLI to obey the flags.
- **`ContextScope` — `context_scope.rs:17`.** Strict-superset tiers `Clean <
  Task < Graph < Full` controlling how much graph context the prompt carries.
  Resolution `task > role > config > default(Task)` (`:54`). This is the dial
  that decides **how much of the graph an agent is shown** — the seed for "how
  large a graph-slice do we ship to a remote provider, and how much may it see."

> **Tool/context verdict:** WG already expresses *"this run gets a restricted set
> of capabilities"* (ExecMode) and *"this run sees a bounded slice of the graph"*
> (ContextScope). These are the conceptual ancestors of **capability-gated run
> authorization** and **graph-slice scoping** — but today they are *advisory
> configuration enforced by a trusted local process*, not *cryptographically
> scoped authority a remote provider is constrained by*.

---

### 2.7 Result verification — `done.rs` + agency auto_evaluate / FLIP

**What it is: a graph-internal quality gate. It scores an agent's output; it does
not authenticate *who* produced it or verify it *across a trust boundary*.**

- `wg done` can soft-complete a task into `PendingEval` when an `.evaluate-X`
  task gates it (`done.rs:1372`, `:1386`); the agency evaluator/FLIP machinery
  then scores the work against the task's `## Validation` section. There is also
  a **smoke-gate** hard check on `wg done` for tasks that own smoke scenarios.
- **But verification is trust-internal.** The evaluator reads the *result that is
  already written into the local graph/worktree*; it judges *quality*, not
  *authenticity*. Nothing checks that the writer was authorized, that the diff
  wasn't tampered with in transit, or that a claimed result came from the
  provider it says it did. There is no signature on the result, no attestation,
  no reproducibility/quorum check.

> **Verification verdict:** WG has *output evaluation* (is this good?) but **zero
> *result verification across a trust boundary*** (did the entity I delegated to
> actually, honestly produce this?). The latter is the central adversarial
> problem `exec-adversarial` (5/6) must solve.

---

### 2.8 Trust / crypto inventory (whole repo)

To confirm there is no hidden placement-trust or result-integrity machinery:

- **Crypto crates (`Cargo.toml`):** only `sha2` (content hashing). **No
  `ed25519`, `secp256k1`, `ring`, `libsodium`/`nacl`, `x25519`, `noise`,
  `libp2p`, `nostr`, `ssh-key`, `jsonwebtoken`, or any UCAN crate.** This is the
  same inventory `fed-baseline` found (`docs/federation-study/02-current-state-baseline.md` §2.4)
  — and it is dispositive: **nothing in WG signs, attests, or verifies anything**,
  so there is no foundation for trust-gated placement or signed results today.
- **Agent identity** is an autoincrement `agent-N` (`registry.rs:349`) or a
  SHA-256 *content* hash for agency `Agent`s — **not a keypair** (per
  `fed-baseline` §2.2). Providers therefore cannot be `wgid:` identities yet.
- **The registry / graph** are plain JSON/JSONL on a shared filesystem, guarded
  by `flock`, not by authorization. Any process that can open the file is fully
  trusted.

---

## 3. Seed-vs-Missing ledger for **federated execution**

The federation goal — split *"approved to run"* from *"which machine runs the
agent,"* safely, across independently-owned hosts — decomposes into the rows
below. Each is checked against current `main`.

| Capability | Status | Evidence on `main` / what's needed |
|---|---|---|
| **Lease lifecycle** (acquire / renew / expire / reclaim a unit of work) | **SEED — present, local** | claim→`InProgress` (`claim.rs:13`); heartbeat renew 120 s (`execution.rs:1921`); `heartbeat_timeout` TTL 5 min (`config.rs:3966`); `mark_dead`+unclaim (`registry.rs:535`, `dead_agents.rs:156`); `reclaim` steal (`reclaim.rs:16`). **Sound mechanics, single-host substrate.** |
| **Execution isolation** (a contained, reproducible workspace per run) | **SEED — present, local** | per-agent git worktree off HEAD + `.wg` symlink + isolated cargo target (`worktree.rs:29`, `execution.rs:656`). Assumes shared FS. |
| **Self-contained task descriptor** (everything needed to run a task, portably) | **PARTIAL SEED** | env-var contract `WG_TASK_ID/AGENT_ID/MODEL/TIER/ENDPOINT/…` (`execution.rs:603–654`) carries *almost* all of it — **except the graph data**, which is reached via the `.wg/` symlink, not shipped. |
| **Handler/executor abstraction** (pluggable run backends) | **SEED — present** | `handler_for_model` single router, designed for new arms (`handler_for_model.rs:87`,`:64`); `plan_spawn` single source of truth for executor/model/endpoint. A remote-runner handler would slot in here. |
| **Inference federation** (the *model* runs on a remote API) | **PRESENT** | claude/codex/nex/pi handlers already call remote LLM endpoints; key by env only (`execution.rs:644`, `pi_handler.rs:465`). |
| **Capability-restricted run** (limit what a run may do) | **PARTIAL SEED** | `ExecMode` → `--allowedTools/--disallowedTools` (`config.rs:1304`, `execution.rs:1438`); `ContextScope` bounds graph exposure (`context_scope.rs:17`). **Advisory, enforced by a trusted local CLI — not cryptographic scope.** |
| **Cross-machine placement** (decide *which host* runs an approved task) | **MISSING** | `plan_spawn` has no host axis (`coordinator.rs:4148`); spawn is always a local `setsid` fork (`execution.rs:666`); capacity is a local PID count vs `max_agents` (`coordinator.rs:122`). No fleet/host registry. |
| **Providers as first-class `wgid:` identities** | **MISSING** | agent id is `agent-N` (`registry.rs:349`); no keypair anywhere (`Cargo.toml` = `sha2` only). Depends on WG-Fed identity layer, which is absent. |
| **Capability-gated run authorization** (a remote may run *only* what a scoped grant permits) | **MISSING** | claim is a free-text `assigned` field (`claim.rs:114`); no UCAN/token, no scope enforcement at a boundary. ExecMode is the *shape* but is host-trusted. |
| **Trust-gated placement** (only place work on hosts you trust, at a trust-appropriate level) | **MISSING** | `Agent.trust_level` exists as data (`fed-baseline` §2.2) but is **not consulted by any placement code** — there is no placement code. |
| **Result verification across a trust boundary** (prove a remote actually & honestly did the work) | **MISSING** | `done`/auto_evaluate judge *quality* of locally-written output (`done.rs:1372`); no signature/attestation/quorum/reproducibility on results. No crypto to build it on. |
| **Ship the input / graph-slice to a remote provider** (and merge the result back) | **MISSING** | graph reached via local `.wg/` symlink (`worktree.rs:70`); merge-back is a local squash by `wg done`. No serialization-of-a-slice, no remote-write protocol, no signed result channel. |
| **Cross-host liveness proof** (heartbeat that means something off-box) | **MISSING** | liveness = local `is_process_alive(pid)` + locally-written heartbeat string (`registry.rs:118/484`). PID is invisible off-host; heartbeat is unsigned and forgeable across a boundary. |

**Summary:** the **execution-side lease/isolation/handler machinery is a strong
seed** (5 rows present-or-partial), but **every row that crosses a machine or a
trust boundary is missing** (7 rows), and all of those depend on a key/identity
substrate that does not exist.

---

## 4. The claim/worktree/heartbeat/reclaim model **is** the seed to lift across trust boundaries

This is the load-bearing claim of the baseline, stated explicitly:

WG's current single-host execution loop is a **complete lease protocol that
happens to run entirely inside one trust domain.** Map it term-for-term onto what
federated execution needs, and the gap is precisely "replace shared-host trust
with cryptographic trust":

| Local primitive (today) | Enforced today by | Federated analog (needed) | Must be replaced by |
|---|---|---|---|
| `claim` → `assigned=actor`, `InProgress` (`claim.rs:13`) | shared-file honesty | a **scoped run grant** to a chosen provider | a capability/UCAN bound to a provider `wgid:` |
| per-agent **worktree** off HEAD (`worktree.rs:29`) | local filesystem | a **shipped graph-slice + isolated remote workspace** | serialized task input + a remote sandbox |
| `wg heartbeat` every 120 s (`execution.rs:1921`) | local registry write | a **liveness proof from the remote** | a signed, periodic lease-renewal message |
| `heartbeat_timeout` TTL (5 min, `config.rs:3966`) | local clock | the **lease TTL** in a placement contract | unchanged in spirit; clock-skew-aware |
| `mark_dead` + unclaim → `Open` (`registry.rs:535`, `dead_agents.rs:156`) | local PID check + file write | **reclaim a stalled remote run, re-place it** | expiry on missing signed renewal, then re-place |
| `wg reclaim` forced takeover (`reclaim.rs:16`) | trusted operator | **revoke a placement, re-dispatch** | capability revocation + re-grant |
| `ExecMode` tool gating (`config.rs:1304`) | trusted local CLI obeys flags | **capability-gated authorization** | scope carried *in* the grant, checked at the boundary |
| `done`/auto_evaluate quality gate (`done.rs:1372`) | trust the local writer | **result verification across trust** | signature/attestation/quorum on the returned result |

The encouraging conclusion: **WG does not need to invent the lease lifecycle —
it has one, tested and in production.** Federated execution is the project of
**re-substrating** that exact lifecycle: every place the current model relies on
"we all share one trusted host" (PID liveness, file-honesty claims, CLI-obeyed
tool flags, locally-judged results) becomes a place where a **signed message, a
scoped capability, or a verified result** must stand in. The shape is right; the
trust model is the work.

---

## 5. Dependency on the WG-Fed identity/capability substrate (explicit)

Federated **execution sits on top of** federated **identity** — it cannot be
built first, and it cannot be built independently. Concretely:

1. **Providers must be identities, not strings.** A remote runner is addressed
   today by nothing at all (there is no remote-runner concept); the federation
   vision requires it to be a `wgid:` keyed identity so placement can be
   *addressed*, *authenticated*, and *trust-rated*. WG agent identity is an
   autoincrement `agent-N` or a SHA-256 content hash — **not a keypair**
   (`registry.rs:349`; `fed-baseline` §2.2). **Blocked on the identity layer.**
2. **Run authorization must be a scoped capability, not a claim field.** The
   "workers = scoped-UCAN holders" model needs signed, attenuable grants. WG has
   `ExecMode`/`ContextScope` as the *shape* of scope (§2.6) but **no capability
   token, no signing, no verification** (`Cargo.toml` = `sha2` only). **Blocked
   on the capability layer.**
3. **Placement must be trust-gated.** `Agent.trust_level` exists as *data* but is
   consulted by **no placement code** (there is none). Trust-gating presupposes
   authenticated identities to rate. **Blocked on identity + trust layer.**
4. **Results must be verifiable across the boundary.** Requires signed/attested
   results — i.e. the keypair/signing infrastructure the substrate study found
   **entirely absent** (`fed-baseline` §2.4, §3).

Per `docs/federation-study/02-current-state-baseline.md`, **all four pillars of
the key-based substrate — identity keys, signed messages, cross-WG addressing,
portable signed state — are confirmed ABSENT on `main`.** Therefore **every
cross-trust row in §3 is downstream of building that substrate.** Execution
federation is a *consumer* of WG-Fed, and this study must treat the identity/
capability layer as a hard prerequisite, sequencing its own roadmap (`exec-decision`,
6/6) *after* (or co-designed with) the WG-Fed roadmap (`fed-decision`).

---

## 6. Handoff notes for the rest of the execution-federation study

- **For `exec-prior-art` (1/6):** the closest prior art maps onto the *missing*
  rows, not the present ones. CI runners / schedulers (Buildkite agents, GitHub
  Actions runners, Nomad/k8s) solve *placement + pull-claim across hosts* — WG
  already has the pull-claim half (`claim.rs`), needs the cross-host half.
  TEEs / verifiable compute / compute markets solve *result verification across
  trust* (§2.7 row) — WG has *zero* of that. Don't map prior art onto
  `coordinator.rs` (a single-host scheduler); map it onto §3's MISSING rows.
- **For `exec-requirements` (3/6):** the two cruxes are **confidentiality**
  (what graph-slice / secrets may a remote provider see — extends `ContextScope`
  and the env-var descriptor §2.2c) and **integrity** (how is a returned result
  proven authentic — the §2.7 gap). The `heartbeat_timeout`/renewal ratio
  (§2.5) is the model for the cross-host lease contract.
- **For `exec-architectures` (4/6):** the natural extension hooks are
  (a) a new `ExecutorKind` remote-runner arm in `handler_for_model.rs`
  (`:64` invites it) + a placement field added to `plan_spawn`; (b) the
  lease lifecycle in §2.5 lifted to a signed protocol; (c) the env-var task
  descriptor (§2.2c) promoted to a serialized, shippable graph-slice. The four
  candidate shapes (trusted-pool ↔ market ↔ confidential ↔ hybrid) differ mainly
  in *how much trust the placement assumes* and *how results are verified* — i.e.
  which of §3's MISSING rows each one buys.
- **For `exec-adversarial` (5/6):** today's execution trust model is *"any
  process that can write `.wg/service/registry.json` and `graph.jsonl` is fully
  trusted: it can forge a heartbeat, claim any task, mark any agent dead, and
  write any result, all unsigned."* That is the threat baseline. The headline
  adversary is the **malicious provider** that returns a plausible-but-forged
  result (§2.7 has no defense) or claims liveness without doing work (§2.5
  heartbeat is forgeable off-host).

---

## 7. Validation checklist (this document)

- [x] **Every current capability cited to `file:line` on current `main`** (§2;
      e.g. `coordinator.rs:3924`/`:44`/`:4148`, `worktree.rs:29`,
      `execution.rs:603`/`:821`/`:1918`, `handler_for_model.rs:87`,
      `pi_handler.rs:444`, `claim.rs:13`, `registry.rs:60`/`:118`/`:535`,
      `heartbeat.rs:10`, `dead_agents.rs:93`, `sweep.rs:57`, `reclaim.rs:16`,
      `exec.rs:26`, `config.rs:1304`/`:3966`, `context_scope.rs:17`,
      `done.rs:1372`).
- [x] **Seed-vs-missing list for federated execution** (§3): 5 seed/partial rows
      present (lease, isolation, descriptor, handler, inference + capability
      gating), 7 cross-trust rows missing (placement, provider identities,
      capability-gated auth, trust-gated placement, result verification,
      ship-the-slice, cross-host liveness).
- [x] **Explicit statement that today's claim/worktree/heartbeat/reclaim model is
      the seed to lift across trust boundaries** (§4, term-for-term mapping
      table).
- [x] **Dependency on the WG-Fed identity/capability substrate made explicit**
      (§5): providers=identities, workers=scoped-capability holders; all four
      substrate pillars confirmed ABSENT per `fed-baseline`; every cross-trust
      row is downstream of that layer.
- [x] `docs/execution-federation-study/02-current-state-baseline.md` written.

---

*Cross-refs:* WG-Fed substrate baseline
`docs/federation-study/02-current-state-baseline.md` (identity/keys/messaging —
the layer this one depends on); the universal worker contract (`wg agent-guide`)
for the claim/heartbeat/done lifecycle a worker actually runs; `CLAUDE.md`
"Service Configuration" + `handler_for_model.rs` for the handler-first model
spec; downstream consumers `exec-architectures` (4/6) and `exec-adversarial`
(5/6).
