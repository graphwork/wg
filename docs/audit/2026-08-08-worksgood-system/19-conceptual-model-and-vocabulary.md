# WorksGood conceptual model and vocabulary audit

**Audit date:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369` (commit time 2026-08-07T12:38:38+02:00)

**Evidence checked through:** 2026-08-08

**Artifact status:** leaf audit; canonical glossary **candidate**, not an adopted product decision

**Scope:** product model, graph/task/execution objects, actor roles, model routing, agency, federation trust and authority, WG-Review/WG-Exec/WG-Pilot boundaries, and lifecycle/completion terminology

**Change boundary:** this artifact only; production source, tests, schemas, and pre-existing documentation were not modified

## 1. Executive abstract

**`[INFERENCE]`** WorksGood is most coherently described as a **durable work-and-evidence system**: a file-backed task graph is the durable center; attended chat and the TUI are human operating surfaces; an optional service dispatches bounded worker processes; agency supplies reusable work identities; immutable completion review controls when successful work becomes `Done`; and federation, inbound review, remote execution, and Pilot are overlays around that local core. This formulation preserves the root README's work-centered thesis without treating every file under `.wg/`, every process, or every identity as a graph node (`README.md:1-31`, `93-119`, `188-199`; `src/graph.rs:689-1035`, `2577-2585`). Confidence: **high**.

**`[FACT]`** The strongest current vocabulary is encoded in the bundled universal role contract: **dispatcher**, **chat agent**, and **worker agent**, with coordinator/orchestrator deprecated as role nouns (`src/text/agent_guide.md:44-68`). The strongest current completion model is also explicit: `wg done` derives `Done` from an exact reviewed manifest plus publication truth rather than inferring success from process exit (`src/text/agent_guide.md:221-292`; `src/commands/completion_done.rs:32-132`). These two sources provide a usable conceptual spine.

**`[FACT]`** The repository does not yet present that spine consistently. The manual says there are eight task statuses and that `Done`, `Failed`, and `Abandoned` all unblock ordinary dependents; the source has eleven statuses and `Status::is_dep_satisfied()` accepts only exact `Done` (`docs/manual/02-task-graph.md:35-142`; `src/graph.rs:379-529`). A pinned binary exercise confirmed that a failed predecessor did **not** make its successor ready. The manual and generated `wg done --help` advertise `--converged`; current dispatch rejects it as a legacy flag, also confirmed against the pinned build (`docs/manual/02-task-graph.md:228-386`; `src/cli.rs:527-548`; `src/main.rs:1263-1275`).

**`[FACT]`** Several words name multiple different objects: `agent` names an agency identity, a runtime registry process, and a role-class such as chat/worker; `provider` names both a model API namespace and a remote compute principal; `role` names both an agency composition and a model-routing slot; `run` names a replay snapshot, a function-application summary, and a spawn UUID; `review` names both inbound-content review and completion review. The code itself acknowledges six overlapping routing words—executor, provider, endpoint, route, handler, and model (`src/dispatch/handler_for_model.rs:1-18`).

**`[INFERENCE]`** The highest conceptual risk is not that the system lacks distinctions; most distinctions exist in types. The risk is that public language flattens them. A reader can reasonably infer that terminal failure authorizes downstream work, that `--converged` is supported, that a `Verified` provider is a verified author, that a federated `wgid:` is the same identity as an agency agent hash, or that a “run” is one worker execution. Those inferences are false, incomplete, or unresolved in the sampled implementation. Severity: **S2 Medium**, likelihood **likely**, confidence **high**.

**`[RECOMMENDATION]`** Adopt the glossary candidate in section 3 only after owner review. The immediate P0 synchronization targets are: ordinary dependency success, completion/convergence help, the agent/identity namespaces, model-provider versus compute-provider, and “one trust dial.” Acceptance should be executable: terminology tests should compare generated help and user manuals against the corresponding Rust enums and dispatch behavior.

**`[VERIFIED]`** This audit statically sampled source types, the only committed JSON Schema, README/manuals, accepted/proposed ADR/design language, and CLI help built from the pinned snapshot. It ran a pinned `cargo build --bin wg` and three isolated CLI fixtures (dependency failure, `--converged`, and bare `wg done`). It did **not** run the full unit/integration/smoke suites, the service daemon, TUI, live models, federation network flows, or destructive identity/provider operations.

## 2. Scope and conceptual map

### 2.1 Concise product model

**`[FACT]`** The root README names three normal installed commands: `worksgood` as attended lifecycle concierge, `wg` as the complete expert CLI, and `nex` as a standalone native model client (`README.md:93-115`). It describes WG as “The work OS for human/AI organizations,” says WG centers work rather than the agent, and explicitly rejects chatbot, benchmark, project-management, and agent-framework primacy (`README.md:1-3`, `21-31`, `52-62`).

**`[INFERENCE]`** A concise product model consistent with both that positioning and the types is:

> **WorksGood is a local-first, file-backed operating system for answerable work.** It persists tasks, dependencies, provenance, evidence, and completion judgments; optionally dispatches human or AI workers; and can extend the same work model across identity, content-review, and remote-execution boundaries.

**`[FACT]`** “Local-first” does not mean “local-only.” The base graph is plain files, while WG-Fed introduces self-certifying principals and transport, WG-Review gates inbound consumption, WG-Exec delegates bounded work to compute providers, and WG-Pilot sequences those surfaces into a deployment (`src/identity/keys.rs:1-25`, `122-161`; `src/review/mod.rs:1-40`; `src/providers/mod.rs:436-591`; `src/cli.rs:2698-2718`).

### 2.2 Diagram 1 — product and plane boundaries

**`[INFERENCE]`** The following is the smallest model that accounts for the sampled implementation without collapsing control, identity, and work into one object:

```text
                           ATTENDED HUMAN PLANE
                 worksgood concierge / wg tui / chat agent
                                   │
                                   ▼
┌──────────────────────────────── LOCAL WG INSTANCE ────────────────────────────────┐
│                                                                                   │
│  DURABLE WORK PLANE                  OPTIONAL CONTROL / EXECUTION PLANE            │
│  graph.jsonl                         service daemon                               │
│  Task ─after→ Task                   ├─ dispatcher loop                            │
│  status + lifecycle projection       ├─ chat-handler processes                     │
│  logs + artifacts                    └─ worker registry/processes                  │
│        │                                         │                                 │
│        └──────── claim / attempt / evidence ─────┘                                 │
│                                                                                   │
│  IDENTITY & POLICY SIDECARS             COMPLETION VALVE                           │
│  agency: role + tradeoff → agent        candidate manifest                         │
│  model route: handler + native model    FLIP review → eval review                  │
│  config / functions / service state     publish/land → derived Done                │
└───────────────────────────────────┬───────────────────────────────────────────────┘
                                    │ signed/sealed boundary
                                    ▼
┌──────────────────────────── FEDERATED OVERLAYS ────────────────────────────────────┐
│ WG-Fed: wgid identity + messages + capabilities                                   │
│ WG-Review: inbound bytes → accept | quarantine | reject                           │
│ WG-Exec: compute-provider placement + grant + lease + result                       │
│ WG-Pilot: deployment/orchestration wrapper over Fed + Review + Exec                │
└────────────────────────────────────────────────────────────────────────────────────┘
```

**`[FACT]`** The graph itself is narrower than “everything in `.wg/`.” `Node` is `Task`, `Resource`, or `ArchivedBoundary`; agency, service state, functions, completion objects, and federation state have separate storage/types (`src/graph.rs:2577-2591`; `README.md:188-199`; `src/function.rs:31-60`; `src/service/registry.rs:1-13`).

### 2.3 Diagram 2 — task, generation, attempt, process, and completion

**`[FACT]`** The lifecycle types distinguish a durable task from its execution ownership. `LifecycleProjection` carries generation, revision, fence, attempt sequence, and a current `AttemptRef`; `AttemptRef` carries an attempt ID, generation, fence, actor ID, and disposition (`src/lifecycle.rs:66-86`, `181-213`). Runtime evidence is further keyed by task, generation, attempt, fence, and lease epoch (`src/attempt_runtime.rs:1-65`).

```text
Task (durable graph node; stable task id)
│
├─ generation 0
│   ├─ attempt-0-1  ── claimed by lifecycle actor
│   │   ├─ worker process / registry entry agent-N
│   │   └─ for Pi: process epoch 0..N and continuation epoch 0..N
│   └─ attempt disposition: succeeded | failed | parked | cancelled | lost
│
├─ generation 1 (explicit retry/reopen lineage, not “the same attempt again”)
│   └─ attempt-1-1 ...
│
└─ successful completion path
    candidate + immutable evidence
          → exact FLIP review
          → exact eval review
          → contract publication (land/deliver/report/explore)
          → task status Done + matching completion disposition
```

**`[INFERENCE]`** A **worker process is not an attempt**: a process is an operating-system execution recorded in the service registry, while an attempt is lifecycle authority and may contain multiple fenced Pi process epochs. An **attempt is not a task**: retries/reopens can create new generations/attempts while the task ID and dependency position remain stable. Confidence: **high**.

### 2.4 Diagram 3 — identity, trust, authority, review, and execution

```text
                 proves WHO                         decides LOCAL BELIEF
wgid + sigchain ───────────────► authenticated principal ───────────────► trust opinion
      │                                                                  peer-author trust
      │                                                                  compute-provider trust
      │                                                                         │
      └─ signs capability (UCAN) ──► grants WHAT authority, to WHOM, until WHEN │
                                              │                                 │
                                              ▼                                 ▼
                                    run grant + task-scoped lease      depth / leash policy
                                              │                                 │
                                              ▼                                 ▼
                                    remote result attribution       inbound review verdict
                                              │                      accept/quarantine/reject
                                              └──────────────┬──────────────────┘
                                                             ▼
                                                     consumption / write gate

Agency agent hash = work identity (role + tradeoff); it is not the wgid key address.
Runtime agent-N   = process registry identity; it is neither of the above.
```

**`[FACT]`** A `wgid:` is derived from an Ed25519 public key; an `IdentityRecord` can optionally carry a small `AgentFields` projection (`role_id`, trust string, executor, capabilities), but the full agency `Agent` is a different content-addressed type with `role_id` and `tradeoff_id` (`src/identity/keys.rs:110-161`; `src/identity/envelope.rs:41-78`; `src/agency/types.rs:500-535`).

**`[FACT]`** Authentication, trust, and authority are separate in enforcement code. Provider trust is the authorizer's local assertion, a capability has issuer/audience/scope/time/proof, and an inbound verdict alone decides whether reviewed bytes may be consumed (`src/providers/mod.rs:310-381`; `src/identity/custody.rs:294-332`, `407-485`; `src/review/mod.rs:144-172`).

### 2.5 Sample and exclusions

**`[FACT]`** Sampled source/schema surfaces: `src/graph.rs`, `src/lifecycle.rs`, `src/attempt_runtime.rs`, `src/service/registry.rs`, `src/runs.rs`, `src/function.rs`, `src/agency/types.rs`, `src/config.rs`, `src/dispatch/{plan,handler_for_model}.rs`, `src/identity/`, `src/trust.rs`, `src/federation.rs`, `src/review/`, `src/providers/`, `src/commands/completion_done.rs`, `src/commands/msg.rs`, and `schemas/tool-manifest.schema.json`.

**`[FACT]`** Sampled narrative/decision surfaces: root `README.md`; `docs/manual/01-overview.md` and `02-task-graph.md`; `docs/AGENT-SERVICE.md`; `docs/AGENCY.md`; accepted `docs/ADR-actor-vs-agent-identity.md`; proposed `docs/design-handler-first-model-spec.md`; trace-function design/protocol documents; and generated help for root, `add`, `claim`, `done`, `runs`, `agent`, `agency`, `func`, `service`, `config`, `identity`, `provider`, `review`, and `pilot`.

**`[UNCERTAINTY]`** This is a conceptual audit, not an exhaustive symbol census. It did not adjudicate every archived design or every UI label. It treats the charter's pinned revision as authority even though this worktree contained later integrated audit commits; source and CLI evidence came from an exported and built copy of the pinned revision.

## 3. Findings and canonical glossary candidate

### 3.1 Findings

#### `CONCEPT-001` — the durable work object is the stable center

**`[FACT]`** **State: shipped/current. Severity: S4 Informational. Confidence: high.** `Task` is the durable node containing status, lifecycle projection, dependencies, requirements, artifacts, timing, assignment, agency identity, cycles, context, execution settings, usage, and completion metadata (`src/graph.rs:689-1035`). The README consistently emphasizes that agents can come and go while the graph remains (`README.md:1-31`).

**`[INFERENCE]`** This is the right top-level product distinction: **WG centers answerable work, not a particular LLM session or worker process**. A product explanation should introduce Task/Dependency/Evidence before agency evolution or model routing.

#### `CONCEPT-002` — implementation has a real execution-object hierarchy

**`[FACT]`** **State: shipped/current. Severity: S4. Confidence: high.** The source independently models task status, lifecycle generation/attempt/fence, attempt-scoped runtime storage, and process-registry entries (`src/graph.rs:379-529`; `src/lifecycle.rs:66-213`; `src/attempt_runtime.rs:22-74`; `src/service/registry.rs:37-90`). `AgentStatus::Done` is a registry/process state, while `Status::Done` is a graph-task state.

**`[INFERENCE]`** Most “did it run?” ambiguity disappears if UI/docs always name the layer: **task status**, **attempt disposition**, **process status**, or **completion disposition**. Unqualified “completed” should be avoided in diagnostic output.

#### `CONCEPT-003` — role vocabulary is explicitly canonical but incompletely migrated

**`[FACT]`** **State: partial. Severity: S2. Likelihood: likely. Confidence: high.** The bundled contract defines dispatcher, chat agent, and worker agent, deprecating coordinator/orchestrator as role nouns (`src/text/agent_guide.md:44-68`). Generated `wg service --help` uses “agent service daemon” but still describes pause/resume as coordinator operations and exposes legacy coordinator aliases. `docs/AGENT-SERVICE.md:1-48`, `121-222`, and `304-347` alternates dispatcher, daemon, coordinator, session, and agent.

**`[INFERENCE]`** “Dispatcher” should name the scheduling role/loop; “service daemon” should name the host process; “chat agent” should name the attended persistent LLM session. Saying “the dispatcher is the daemon” is usable shorthand but not a complete process model because the daemon also supervises chat and runtime state.

#### `CONCEPT-004` — `agent` remains three namespaces despite the unified-identity ADR

**`[FACT]`** **State: partial. Severity: S2. Likelihood: likely. Confidence: high.** The accepted ADR removed the old graph `Actor` and declares one unified Agent identity (`docs/ADR-actor-vs-agent-identity.md:1-81`). Current code nevertheless necessarily has: (1) agency `Agent`, a role/tradeoff composition (`src/agency/types.rs:500-535`); (2) runtime `AgentEntry`, a PID/task/executor/heartbeat record (`src/service/registry.rs:60-90`); and (3) federated `IdentityRecord`, a `wgid:` principal with optional agent fields (`src/identity/envelope.rs:41-78`). A task also retains both `assigned` and `agent` fields (`src/graph.rs:714`, `853`).

**`[INFERENCE]`** The ADR successfully removed one obsolete entity type but could not make “agent” globally singular. The remaining namespaces are legitimate; claiming a single identity without qualifiers obscures rather than simplifies them.

#### `CONCEPT-005` — model routing has an implementable grammar, but public vocabulary still leaks internals

**`[FACT]`** **State: shipped with migration residue. Severity: S2. Confidence: high.** `handler_for_model` says six overlapping terms reduce to handler choice and wire protocol, and handler-first specs interpret the leading token as a handler (`src/dispatch/handler_for_model.rs:1-60`). Internally the canonical enum is still called `ExecutorKind` and includes local CLIs, shell, Pi, and `RemoteRunner` (`src/dispatch/plan.rs:57-109`). The agent schema still stores `executor` and `preferred_provider` (`src/agency/types.rs:505-535`).

**`[INFERENCE]`** “Handler” is the best public word for **what implementation runs the model route**. “Executor” remains a compatibility/internal umbrella that also includes shell and remote-runner shapes. They are not perfect synonyms; documentation should not alternate them without stating that boundary.

#### `CONCEPT-006` — one trust scale is not one trust assertion

**`[FACT]`** **State: shipped/current with stale comment. Severity: S2. Confidence: high.** `TrustLevel` is one three-value enum (`Verified`, default `Provisional`, `Unknown`) (`src/graph.rs:2530-2540`). `src/trust.rs` first calls this “one trust dial,” then explicitly separates author trust from provider trust and folds the latter only in the stricter direction (`src/trust.rs:1-53`, `111-125`). Provider enrollment trust is local to the authorizer (`src/providers/mod.rs:310-381`).

**`[INFERENCE]`** The coherent model is **one shared ordinal trust scale, multiple subject-and-purpose-specific local assertions**. A peer-author assertion answers whether to reduce content-review depth; a compute-provider assertion answers where work may execute. Neither is self-certified by a `wgid:`.

#### `CONCEPT-007` — authentication, trust, authority, and acceptance are separate gates

**`[FACT]`** **State: shipped/current. Severity: S4. Confidence: high.** `wgid:` establishes a self-certifying key address; capabilities grant scoped, expiring authority; leases fence a remote task execution; review verdicts gate byte consumption; completion reviews and publication gate task `Done` (`src/identity/keys.rs:122-161`; `src/identity/custody.rs:294-485`; `src/providers/mod.rs:436-591`; `src/review/mod.rs:144-172`; `src/commands/completion_done.rs:32-132`).

**`[INFERENCE]`** This is a strong conceptual property. The documentation should express it as a fixed sentence: **a signature proves attribution, trust is a local judgment, a capability grants authority, a lease bounds execution, a review permits consumption, and completion evidence authorizes `Done`.**

#### `CONCEPT-008` — WG-Review, WG-Exec, and WG-Pilot have distinct ownership boundaries

**`[FACT]`** **State: partial-to-shipped, terminology uncertain. Severity: S3. Confidence: high on code boundary, medium on maturity label.** WG-Review owns classification and the consumption verdict; WG-Exec owns placement/claim/grant/lease/result and remote canonical-write controls; WG-Pilot describes itself as a deploy/UX wrapper that ships no new substrate (pinned generated `wg review --help`, `wg provider --help`, `wg pilot --help`; `src/review/mod.rs:63-172`; `src/providers/mod.rs:436-591`; `src/cli.rs:2698-2718`).

**`[FACT]`** “Review” is overloaded: inbound WG-Review is distinct from exact completion FLIP/eval review. “Provider” is overloaded: a WG-Exec provider is a separately owned compute principal addressed by `wgid:`, not OpenRouter/Anthropic as a model provider.

#### `CONCEPT-009` — function, trace, replay, and run are distinct, but `run` is overloaded

**`[FACT]`** **State: shipped/current. Severity: S3. Confidence: high.** A `TraceFunction` is a parameterized workflow template (`src/function.rs:31-60`). `wg func apply` creates tasks, while replay resets existing tasks; the design states this distinction (`docs/design/trace-functions.md:1-34`). `.wg/runs/run-NNN` is specifically a graph/config snapshot taken before replay (`src/runs.rs:1-29`, `63-128`). Function memory also uses `RunSummary`, and each worker spawn receives a UUID named `WG_SPAWN_RUN_ID`/`WG_LAUNCH_TOKEN` (`src/function.rs:300-335`; `src/commands/spawn/execution.rs:1057`, `1696-1698`).

**`[INFERENCE]`** “Run” cannot safely be a canonical standalone noun. Use **replay snapshot**, **function application**, and **spawn/launch ID**.

#### `CONCEPT-010` — externally published schemas do not cover the core conceptual model

**`[FACT]`** **State: gap. Severity: S3. Confidence: high.** A repository search at the pinned snapshot found one JSON Schema: `schemas/tool-manifest.schema.json`. It defines tool sources/bundles/task scopes and only four executor defaults (`claude`, `native`, `shell`, `amplifier`) (`schemas/tool-manifest.schema.json:1-57`, `154-181`). Core graph, lifecycle, agency, identity, provider, review, and completion schemas are embodied in Serde Rust types and examples, not published JSON Schemas.

**`[INFERENCE]`** Documentation drift is easier because prose tables are acting as schemas. This does not make storage invalid, but it removes a machine-checkable vocabulary authority for external readers and adapters.

### 3.2 Canonical glossary candidate

**`[RECOMMENDATION]`** The table below is a candidate, not a claim that the repository already uses these words consistently.

| Candidate term | Canonical candidate meaning | Do not use it to mean | Primary evidence |
|---|---|---|---|
| **WorksGood / WG** | The product/system: durable work graph plus optional human, dispatch, identity, review, and federation planes | One daemon, one model client, or the agency alone | `README.md:1-62` |
| **`wg`** | Expert CLI for the full task/tool surface | Product as a whole when command precision matters | `README.md:93-115` |
| **`worksgood`** | Attended lifecycle concierge entry point | Dispatcher or model handler | `README.md:93-115` |
| **`nex`** | Standalone native model client | The WG dispatcher or all model execution | `README.md:93-99` |
| **WG instance** | One project-scoped `.wg` state root plus associated worktree/project | Only `graph.jsonl` | `README.md:188-199` |
| **work graph** | Directed relation of graph nodes; operationally task dependencies dominate | Every sidecar file or every process | `src/graph.rs:2577-2591` |
| **task** | Durable, addressable unit of answerable work and its lifecycle/evidence metadata | A worker process, chat turn, or provider grant | `src/graph.rs:689-1035` |
| **graph node** | `Task`, `Resource`, or `ArchivedBoundary` | Synonym for task in schema-level descriptions | `src/graph.rs:2577-2591` |
| **dependency** | Typed/ordinary predecessor relation; ordinary success requires predecessor `Done` | “Any terminal predecessor” | `src/graph.rs:514-529` |
| **draft** | A task held from dispatch (currently represented by pause state), awaiting publication | A separate task type or unpersisted object | pinned `wg add` output; `src/graph.rs:880-884` |
| **publish (task graph)** | Release a draft task/subgraph for readiness/dispatch | Git push, completion publication, or identity publish | pinned `wg publish` help/source; qualify the object |
| **ready** | Derived eligibility for dispatch after status, pause, timing, and dependency checks | A stored task status | `src/query.rs:307-522`; `docs/manual/02-task-graph.md:187-213` (intent, not authority) |
| **assignment** | Selection of an agency identity (`task.agent`) for how work should be approached | Execution ownership | `src/graph.rs:853`; `src/commands/assign.rs:170-215` |
| **claim** | Lifecycle reservation/ownership of one task execution by an actor; begins/associates an attempt | Agency identity selection | `src/lifecycle.rs:247-270`; pinned `wg claim --help` |
| **generation** | Retry/reopen lineage epoch of a task lifecycle | Function generation or model generation | `src/lifecycle.rs:181-213` |
| **attempt** | One fenced lifecycle execution ownership within a task generation | OS process, retry count, or replay run | `src/lifecycle.rs:66-86` |
| **worker process** | Spawned OS process executing a task, recorded in service registry | Durable agent identity or attempt | `src/service/registry.rs:60-90` |
| **process epoch** | Fenced replacement process within the same Pi attempt | New task attempt | `src/lifecycle.rs:193-206` |
| **attempt disposition** | `Succeeded`, `Failed`, `Parked`, `Cancelled`, or `Lost` | Task status or completion disposition | `src/lifecycle.rs:66-73` |
| **task status** | Graph projection (`Open`, `InProgress`, etc.); `Done` is successful terminal | Process status or verdict | `src/graph.rs:379-529` |
| **completion contract** | Promised publication mode: `land`, `deliver`, `report`, or `explore` | Verify command or task status | `src/graph.rs:322-366` |
| **completion candidate** | Immutable manifest/evidence proposal bound to an exact task/attempt | Editable worktree or prose “done” | `src/commands/completion_done.rs:32-103` |
| **completion review** | Exact FLIP then eval review of the candidate manifest | WG-Review inbound content gate | `src/text/agent_guide.md:221-292` |
| **completion disposition** | Receipt-backed outcome: Landed/Delivered/Reported/Explored | Attempt succeeded or task Done alone | `src/graph.rs:346-366` |
| **dispatcher** | Scheduling/control role that polls ready work and spawns workers | Attended chat agent | `src/text/agent_guide.md:44-51` |
| **service daemon** | Background host process for dispatcher, registries, IPC, and chat supervision | The dispatcher role alone | `docs/AGENT-SERVICE.md:1-48` |
| **chat agent** | Persistent attended LLM session represented by `.chat-N` graph entity | Dispatcher/coordinator | `src/text/agent_guide.md:52-62` |
| **worker agent** | Bounded LLM worker role for one task | Agency identity record or every `agent-N` row | `src/text/agent_guide.md:63-65` |
| **agency** | Store/system for composable work identities, evaluation, lineage, and evolution | Federation identity or service registry | `src/agency/types.rs:328-535` |
| **agency role** | Composition of capability-component IDs plus desired outcome | Model-routing role or lifecycle role | `src/agency/types.rs:475-493` |
| **tradeoff** | Primitive describing acceptable/unacceptable compromises; current replacement for Motivation | Generic motivation prose without mapping | `src/agency/types.rs:419-468` |
| **agency agent** | Content-addressed role + tradeoff work identity with operational preferences | Runtime PID entry or `wgid:` | `src/agency/types.rs:500-535` |
| **persona** | Avoid as a schema term; if used in UX, explicitly map it to an agency agent | Role alone or chat session | no current core type found in sampled source |
| **dispatch role** | Model-routing slot such as task_agent/evaluator/assigner | Agency role | `src/config.rs:1623-1716` |
| **model route/spec** | Handler-first string: leading handler plus handler-native model dialect | Provider alone | `src/dispatch/handler_for_model.rs:20-60` |
| **handler** | Implementation selected to execute a model route (Claude CLI, Pi CLI, Nex/native, etc.) | Model provider or endpoint | `src/dispatch/handler_for_model.rs:1-60` |
| **executor** | Legacy/internal execution-kind umbrella; qualify when including shell/remote-runner | Preferred public synonym for handler | `src/dispatch/plan.rs:57-109` |
| **model provider** | API/wire namespace inside a handler-native route, e.g. OpenRouter | Remote compute provider | `src/dispatch/handler_for_model.rs:28-58` |
| **endpoint** | Network URL/config used by a handler/provider | Model, handler, or provider identity | `src/dispatch/plan.rs:34-48` |
| **compute provider** | Separately owned `wgid:` principal offering remote execution capability | OpenRouter, Anthropic, or a local handler | `src/providers/mod.rs:292-381` |
| **replay snapshot** | `.wg/runs/run-NNN` graph/config snapshot used by replay/restore/diff | One worker execution | `src/runs.rs:1-29` |
| **trace function** | Parameterized workflow template that creates new tasks | Replay or recorded tool-call macro | `src/function.rs:31-60`; `docs/design/trace-functions.md:15-34` |
| **function application** | One instantiation of a trace function and its resulting task set/outcome summary | Replay run | `src/function.rs:300-335` |
| **federated identity / `wgid:`** | Self-certifying cryptographic principal/address rooted in an Ed25519 key | Agency agent hash or trust level | `src/identity/keys.rs:110-161` |
| **trust assertion** | Local opinion on shared `Verified/Provisional/Unknown` scale, qualified by subject/purpose | Authentication or delegated authority | `src/trust.rs:29-53`; `src/providers/mod.rs:310-381` |
| **capability** | Signed, scoped, expiring authorization from issuer to audience, with attenuation proof | Trust or identity | `src/identity/custody.rs:294-332` |
| **lease** | Time/epoch fence for one remote execution/write authority | Capability itself or process heartbeat alone | `src/providers/mod.rs:487-591` |
| **inbound review** | WG-Review decision on exact inbound bytes: accept/quarantine/reject | Completion FLIP/eval review | `src/review/mod.rs:63-172` |
| **WG-Fed** | Cross-instance identity, messaging, state, sealing, and capability substrate | Remote execution itself | `src/identity/` |
| **WG-Review** | Inbound-content consumption gate | Universal completion valve | `src/review/mod.rs:1-40` |
| **WG-Exec** | Remote compute placement/delegation/lease/result plane | Local model handler selection | `src/providers/mod.rs:436-591` |
| **WG-Pilot** | Deployment/UX wrapper over Fed + Review + Exec | New security or execution substrate | `src/cli.rs:2698-2718` |

### 3.3 Lifecycle and completion terminology candidate

**`[RECOMMENDATION]`** Use this sequence in manuals and UI copy:

```text
create task → draft/hold → publish task → ready (derived)
→ claim/reserve attempt → worker process running
→ attempt outcome + immutable candidate/evidence
→ completion review accepted
→ contract publication (land/deliver/report/explore)
→ Done derived
```

**`[RECOMMENDATION]`** Reserve verbs as follows:

- **succeed/fail/park/cancel/lose** an **attempt**;
- **start/exit/die** a **process**;
- **accept/quarantine/reject** **inbound content**;
- **pass/reject** a **completion review**;
- **land/deliver/report/explore** a **completion contract**;
- **mark/project/derive `Done`** only for the **task status**;
- **retry/reopen** the **task**, producing an explicitly described generation/attempt transition.

**`[RECOMMENDATION]`** Never say “terminal means dependencies are satisfied.” Say: **terminality controls lifecycle finality; dependency satisfaction is relation-specific, and ordinary successful dependencies require exact `Done`.**

## 4. Contradictions, ambiguity, and collision register

### 4.1 Direct implementation/documentation disagreements

| ID | Record | Severity / confidence | State |
|---|---|---|---|
| `CONCEPT-DRIFT-001` | **`[CONTRADICTION]`** Manual: tasks have eight statuses and Done/Failed/Abandoned all unblock (`docs/manual/02-task-graph.md:35-142`). Source: eleven status variants; only `Done` satisfies ordinary dependencies (`src/graph.rs:379-529`). Pinned fixture: failed `a`; `wg ready --json` returned `[]` for `b after a`. | S2 / high | open; implementation + E1 behavior control |
| `CONCEPT-DRIFT-002` | **`[CONTRADICTION]`** Manual and generated help present `wg done --converged` as supported (`docs/manual/02-task-graph.md:344-386`; pinned `wg done --help`). Bundled role contract says completion has no `--converged`, and dispatch rejects it (`src/text/agent_guide.md:215-219`; `src/main.rs:1263-1275`). Pinned fixture observed the rejection. | S2 / high | open; help/manual stale |
| `CONCEPT-DRIFT-003` | **`[CONTRADICTION]`** `wg done --help` says “Mark a task as done,” but current implementation requires a completion candidate and verifies exact reviews/publication (`src/cli.rs:527-548`; `src/commands/completion_done.rs:32-132`). A pinned claimed task without a candidate failed `missing completion candidate`. | S2 / high | open; help underspecifies hard gate |
| `CONCEPT-DRIFT-004` | **`[CONTRADICTION]`** Manual says graph `status` has eight values and old verify/approve/reject transitions (`docs/manual/02-task-graph.md:35-142`); source includes PendingEval, FailedPendingEval, and Incomplete, while modern completion uses manifest reviews (`src/graph.rs:382-424`; `src/commands/completion_done.rs:32-132`). | S2 / high | open |
| `CONCEPT-DRIFT-005` | **`[CONTRADICTION]`** Manual overview calls motivation a prose synonym for tradeoff and describes Role as inline skills/outcome (`docs/manual/01-overview.md:35-58`); current code stores role as component IDs + outcome ID and declares `TradeoffConfig` replaces old Motivation (`src/agency/types.rs:419-493`). `docs/AGENCY.md:11-71` also presents legacy field names. | S2 / high | open; docs lag schema migration |
| `CONCEPT-DRIFT-006` | **`[CONTRADICTION]`** Accepted actor/agent ADR says “one unified identity model” (`docs/ADR-actor-vs-agent-identity.md:1-81`); runtime and federation necessarily retain separate agent/process/key identities, and tasks retain both `assigned` and `agent` (`src/service/registry.rs:60-90`; `src/identity/envelope.rs:41-78`; `src/graph.rs:714`, `853`). | S3 / high | apparent design overstatement; qualify rather than reverse ADR |
| `CONCEPT-DRIFT-007` | **`[CONTRADICTION]`** `PeerConfig.trust` comment says `None` resolves to Provisional TOFU (`src/federation.rs:63-72`); `peer_trust_opinion` explicitly resolves a present peer with no trust to Unknown and tests that behavior (`src/trust.rs:79-101`, `181-190`). | S2 / high | open; implementation/test control |
| `CONCEPT-DRIFT-008` | **`[CONTRADICTION]`** `src/review/mod.rs:31-40` says Pass 2 is deliberately deterministic and the live weak-tier reviewer is future Wave C; the same module now has a config-aware model-driven reviewer path with weak→strong escalation (`src/review/mod.rs:320-432`). | S2 / high | open; module overview stale relative to implementation |
| `CONCEPT-DRIFT-009` | **`[CONTRADICTION]`** Handler-first design is labeled “Proposed (design only—does NOT implement)” (`docs/design-handler-first-model-spec.md:1-15`), while current config/dispatch source implements and tests handler-first leading-token validation (`src/dispatch/handler_for_model.rs:20-60`; `src/config.rs:2513-2588`, `10158-10182`). | S3 / high | open; update design status/provenance |
| `CONCEPT-DRIFT-010` | **`[CONTRADICTION]`** README calls Pi WorksGood's “sole model plane” (`README.md:117-119`), while `ExecutorKind` and current handler routing support Claude, native/Nex, Codex, Pi, several external CLIs, shell, and remote runner (`src/dispatch/plan.rs:61-109`). | S2 / medium | scope unresolved: recommendation vs implementation capability |
| `CONCEPT-DRIFT-011` | **`[CONTRADICTION]`** Manual says the graph file is the project/canonical state and the daemon holds no state beyond it (`docs/manual/02-task-graph.md:8-10`, `441-447`); the implementation persists lifecycle ledger, service registry, attempt evidence, completion objects, agency, functions, and federation sidecars. | S2 / high | open; “graph is work” slogan overextended into persistence claim |
| `CONCEPT-DRIFT-012` | **`[CONTRADICTION]`** The only JSON Schema's executor defaults enumerate four values (`schemas/tool-manifest.schema.json:49-57`), while `ExecutorKind` has many more (`src/dispatch/plan.rs:61-109`). It is unclear whether the schema intends a closed executor list; JSON Schema `properties` without `additionalProperties: false` permits others, but discoverability still drifts. | S3 / medium | open; schema semantic scope uncertain |

### 4.2 Ambiguity and collision register

| ID | Collision | Evidence and consequence | Recommended qualification |
|---|---|---|---|
| `CONCEPT-AMB-001` | **agent** | Agency identity, runtime PID entry, chat/worker role, federated optional agent fields (`src/agency/types.rs:500-535`; `src/service/registry.rs:60-90`; `src/text/agent_guide.md:44-65`; `src/identity/envelope.rs:41-78`) | agency agent; runtime worker/process; chat agent; federated principal |
| `CONCEPT-AMB-002` | **role** | Agency `Role`, `DispatchRole`, and human/LLM role contract (`src/agency/types.rs:475-493`; `src/config.rs:1623-1716`) | agency role; dispatch role; actor role |
| `CONCEPT-AMB-003` | **provider** | Model provider/wire namespace versus WG-Exec compute provider (`src/dispatch/handler_for_model.rs:28-58`; `src/providers/mod.rs:292-381`) | model provider; compute provider |
| `CONCEPT-AMB-004` | **executor / handler** | Public handler-first grammar; internal enum still `ExecutorKind`; shell and remote runner do not simply “handle a model” (`src/dispatch/handler_for_model.rs:1-60`; `src/dispatch/plan.rs:61-109`) | handler for model route; execution kind for internal umbrella |
| `CONCEPT-AMB-005` | **coordinator** | Deprecated role noun, config keys/internal state, chat legacy aliases, and ordinary activity word (`src/text/agent_guide.md:44-68`; `docs/AGENT-SERVICE.md:121-222`) | dispatcher; service daemon; chat agent; coordination (activity) |
| `CONCEPT-AMB-006` | **run** | Replay snapshot, function memory application, spawn launch token (`src/runs.rs:1-29`; `src/function.rs:300-335`; `src/commands/spawn/execution.rs:1057`, `1696-1698`) | replay snapshot; function application; spawn ID |
| `CONCEPT-AMB-007` | **review** | Inbound content gate and completion FLIP/eval gate | inbound review; completion review |
| `CONCEPT-AMB-008` | **identity** | Agency work identity, runtime process identity, chat/session identity, cryptographic `wgid:` | qualify namespace every time |
| `CONCEPT-AMB-009` | **trust** | One enum, separate author/provider assertions; default Provisional in type but Unknown on absent peer/provider paths (`src/graph.rs:2530-2540`; `src/trust.rs:29-53`, `79-125`) | author trust; compute-provider trust; shared scale |
| `CONCEPT-AMB-010` | **assigned / claimed** | `task.agent` selects agency identity; `task.assigned` and lifecycle actor/fence track operational ownership (`src/graph.rs:714`, `853`; `src/lifecycle.rs:75-86`) | assign identity; claim execution |
| `CONCEPT-AMB-011` | **terminal / successful / satisfied** | `Failed` and `Abandoned` are terminal but ordinary dependency satisfaction is only `Done` (`src/graph.rs:514-529`) | always name the predicate |
| `CONCEPT-AMB-012` | **done / succeeded / landed** | attempt disposition, task status, and completion disposition are separate enums (`src/lifecycle.rs:66-73`; `src/graph.rs:382-424`, `346-366`) | attempt succeeded; task Done; contract Landed |
| `CONCEPT-AMB-013` | **publish** | Task-draft release, Git push/publication, identity publish, and completion publication | publish task; push Git; publish identity; publish completion |
| `CONCEPT-AMB-014` | **function / trace / replay** | Function creates fresh tasks; replay resets; trace records history (`docs/design/trace-functions.md:15-34`) | retain all three distinct nouns |
| `CONCEPT-AMB-015` | **cycle / loop / convergence** | Cycle config remains in schema/CLI, but universal completion rejects `--converged`; service cycle code still exists | call compatibility cycle metadata until supported lifecycle is adjudicated |
| `CONCEPT-AMB-016` | **agency / Agency** | Local identity/evolution subsystem versus bridge/import references to an external Agency source (`src/agency/agency_bridge.rs`; `src/commands/agency_import.rs`) | WG agency; external Agency service/data source |
| `CONCEPT-AMB-017` | **graph / project / instance** | Manual equates graph with project, but sidecars and processes lie outside `Node` | graph for node relation; instance for full `.wg` system; project for repo/work |
| `CONCEPT-AMB-018` | **spark / wave / shipped** | WG-Review/Exec/Pilot help exposes real commands while module/design prose retains spark/wave language | use implementation state labels: CLI-reachable, auto-wired seam, stubbed classifier, deferred capability |

### 4.3 Apparent contradictions resolved or narrowed

**`[FACT]`** `assigned` and `agent` look like the pre-ADR duplicate pointers, but sampled code gives them different current jobs: operational claimant/actor ownership versus agency identity selection. This is not necessarily an implementation defect; it is a naming/documentation defect because `assigned` has no local field comment while `agent` does (`src/graph.rs:714`, `853`).

**`[FACT]`** `TrustLevel` defaulting to Provisional is not by itself evidence that unknown federated authors are provisionally trusted. Author/provider resolver functions explicitly fail absent opinions closed to Unknown (`src/graph.rs:2530-2540`; `src/trust.rs:79-125`). The remaining contradiction is the stale `PeerConfig` comment, not observed resolver behavior.

**`[UNCERTAINTY]`** Cycle support is not simply absent: cycle metadata, analysis, add flags, and service iteration code remain. What is inconsistent is the **supported completion path**: a new task can carry `max_iterations`, but `wg done --converged` is rejected and the bundled contract calls cycles historical. A service-level end-to-end cycle with reviewed completion was not executed, so this audit does not claim all cycle execution is unreachable.

## 5. Risks and gaps

| ID | Label | Severity | Risk / gap |
|---|---|---:|---|
| `CONCEPT-RISK-001` | `[INFERENCE]` | S2 | A user following the manual can allow for failed dependencies to flow downstream, but current readiness blocks them. This changes orchestration semantics, not just wording. |
| `CONCEPT-RISK-002` | `[INFERENCE]` | S2 | Generated help advertises completion bypass/cycle flags that fail at dispatch. Automation can be built around an option accepted by Clap but rejected by the command body. |
| `CONCEPT-RISK-003` | `[INFERENCE]` | S2 | “One trust dial” can be misread as a single transitive reputation. That would collapse author trust and compute-provider trust—the exact escalation `src/trust.rs` says it corrected. |
| `CONCEPT-RISK-004` | `[INFERENCE]` | S2 | “Unified agent identity” can lead adapters to interchange an agency hash, `agent-N`, and `wgid:`. They have different persistence, authority, and verification rules. |
| `CONCEPT-RISK-005` | `[INFERENCE]` | S2 | “Provider” ambiguity can route model credentials/configuration discussions into the WG-Exec trust plane, or make a compute provider appear to be an API provider. |
| `CONCEPT-RISK-006` | `[INFERENCE]` | S2 | “Review” ambiguity can cause teams to assume inbound content safety substitutes for exact completion review, or vice versa. They guard different edges. |
| `CONCEPT-RISK-007` | `[INFERENCE]` | S3 | Multiple meanings of run weaken observability and data interchange: a `run_id` may not identify a replay, a function application, or a lifecycle attempt. |
| `CONCEPT-RISK-008` | `[FACT]` | S3 | No published core graph/agency/lifecycle JSON Schema was found. Serde types are authoritative for Rust, but external integrations must infer contracts from code/examples. |
| `CONCEPT-RISK-009` | `[UNCERTAINTY]` | S3 | Product maturity labels (“spark,” “wave,” “production,” “shipped”) are not systematically tied to reachability or seam coverage. A CLI command can be real while one internal classifier remains deterministic/stubbed. |
| `CONCEPT-GAP-001` | `[FACT]` | S3 | This audit did not execute chat/dispatcher/service human flows, so it did not verify which deprecated coordinator labels remain visible in TUI/status output. |
| `CONCEPT-GAP-002` | `[FACT]` | S3 | This audit did not run a full reviewed structural cycle; compatibility of cycle iteration with the immutable completion valve remains uncertain. |
| `CONCEPT-GAP-003` | `[FACT]` | S3 | Federation, provider, review, and Pilot security behavior was inspected but not executed here; sibling audits own those empirical claims. |

## 6. Recommendations

### 6.1 Factual synchronization work

1. **`CONCEPT-REC-001` — `[RECOMMENDATION]` (P0, docs/orchestration):** Rewrite `docs/manual/02-task-graph.md` status, dependency, cycle, and completion sections from current enums and the immutable completion path. Link: `CONCEPT-DRIFT-001..004`. Acceptance: an extracted table exactly matches `Status`; ordinary dependency text says only `Done`; no unsupported `--converged` flow remains.
2. **`CONCEPT-REC-002` — `[RECOMMENDATION]` (P0, CLI/completion):** Make `wg done --help` describe derivation from reviewed publication and remove or visibly mark rejected legacy flags. Link: `CONCEPT-DRIFT-002..003`. Acceptance: every advertised option has a successful supported path or is absent; a CLI snapshot test covers the text.
3. **`CONCEPT-REC-003` — `[RECOMMENDATION]` (P0, trust/federation):** Correct `PeerConfig.trust` documentation and replace “one trust dial” with “shared trust scale; separate local assertions.” Link: `CONCEPT-DRIFT-007`, `CONCEPT-AMB-009`. Acceptance: peer-without-vouch is documented and tested as Unknown; provider trust cannot raise author trust.
4. **`CONCEPT-REC-004` — `[RECOMMENDATION]` (P1, agency docs):** Regenerate role/tradeoff/agent field tables from `src/agency/types.rs`; move Motivation to an explicit migration note. Link: `CONCEPT-DRIFT-005`. Acceptance: docs name `component_ids`, `outcome_id`, and `tradeoff_id` and distinguish empty/optional human bindings from Rust field optionality.
5. **`CONCEPT-REC-005` — `[RECOMMENDATION]` (P1, model docs):** Mark handler-first design implemented/superseded as appropriate and qualify README's “sole model plane” claim as recommended attended/product path or remove exclusivity. Link: `CONCEPT-DRIFT-009..010`. Acceptance: README, design status, `config --models`, and handler tests tell the same scoped story.
6. **`CONCEPT-REC-006` — `[RECOMMENDATION]` (P1, review):** Update `src/review/mod.rs` overview to describe deterministic fallback versus config-aware live reviewer. Link: `CONCEPT-DRIFT-008`. Acceptance: module docs identify which path is credential-free, which is model-driven, and which ingest seams call each.

### 6.2 Implementation and schema work

7. **`CONCEPT-REC-007` — `[RECOMMENDATION]` (P1, schema/architecture):** Publish versioned schemas or generated reference tables for Task, lifecycle projection, agency Agent, identity envelope, provider envelopes, and verdict records. Link: `CONCEPT-010`. Acceptance: schema generation is tested against Serde round trips and docs consume generated enums rather than hand-counting statuses.
8. **`CONCEPT-REC-008` — `[RECOMMENDATION]` (P1, telemetry/runtime):** Replace unqualified `run_id` at boundaries with typed names (`replay_snapshot_id`, `function_application_id`, `spawn_launch_id`, `attempt_id`) while retaining serde aliases for migration. Link: `CONCEPT-009`, `CONCEPT-AMB-006`. Acceptance: status/JSON output identifies the namespace.
9. **`CONCEPT-REC-009` — `[RECOMMENDATION]` (P1, graph/lifecycle):** Document or rename `Task.assigned` to make operational ownership explicit; keep `Task.agent` as agency identity. Link: `CONCEPT-004`, `CONCEPT-AMB-010`. Acceptance: both fields have Rust docs, serialized reference docs, and distinct CLI labels.
10. **`CONCEPT-REC-010` — `[RECOMMENDATION]` (P2, tool schema):** Generate tool-manifest executor suggestions from `ExecutorKind` capabilities rather than a hand-maintained partial property set. Link: `CONCEPT-DRIFT-012`. Acceptance: supported execution kinds are either discoverable or explicitly excluded by schema scope.

### 6.3 Human product/design decisions

11. **`CONCEPT-REC-011` — `[RECOMMENDATION]` (P0, product owner):** Ratify a one-sentence product definition and the product-plane diagram. Decide whether “work OS” is positioning only or a formal category. Acceptance: README, concierge, manual overview, and website use the same sentence with scoped elaborations.
12. **`CONCEPT-REC-012` — `[RECOMMENDATION]` (P0, product/security):** Ratify the fixed trust/authority sentence from `CONCEPT-007`. Acceptance: Fed, Review, Exec, and Pilot introductions all distinguish attribution, trust, capability, lease, review, and completion.
13. **`CONCEPT-REC-013` — `[RECOMMENDATION]` (P1, product/UX):** Decide whether `executor` remains a supported public umbrella. If yes, define `handler ⊂ execution kind`; if no, confine executor to migration/internal JSON and use handler locally plus compute provider remotely. Acceptance: no page calls OpenRouter an executor or a remote box a model provider.
14. **`CONCEPT-REC-014` — `[RECOMMENDATION]` (P1, product/agency):** Stop claiming globally singular “Agent identity.” Present three explicit namespaces: agency identity, runtime process identity, federated principal. Acceptance: every user-visible ID is prefixed/labeled by namespace.
15. **`CONCEPT-REC-015` — `[RECOMMENDATION]` (P1, orchestration):** Adjudicate structural cycles under immutable completion: supported, compatibility-only, or retired. Acceptance: one live smoke scenario exercises the selected human flow; CLI help, role contract, manual, and cycle source agree.
16. **`CONCEPT-REC-016` — `[RECOMMENDATION]` (P1, product maturity):** Replace wave/spark-as-status with a capability matrix: **type exists**, **CLI reachable**, **live seam wired**, **credential-free fallback**, **live model**, **production-validated**, **deferred**. Acceptance: Review/Exec/Pilot pages publish the matrix and never use “shipped” to imply every seam.

## 7. Evidence appendix

### 7.1 Snapshot and method

**`[VERIFIED]`** Static source was exported from the charter-pinned commit rather than inferred from the later worktree head:

```bash
rm -rf /tmp/wg-audit-concepts-b089
mkdir -p /tmp/wg-audit-concepts-b089
git archive b0892ea7496fd2cc8f641417a3d8e33ca9add369 \
  | tar -x -C /tmp/wg-audit-concepts-b089
CARGO_TARGET_DIR=/home/bot/wg/target \
  cargo build --manifest-path /tmp/wg-audit-concepts-b089/Cargo.toml \
  --locked --bin wg
sha256sum /home/bot/wg/target/debug/wg
```

- cwd for archive/build: `/home/bot/wg/.wg-worktrees/agent-11`
- UTC date: 2026-08-08
- Rust: `rustc 1.96.0 (ac68faa20 2026-05-25)`
- Cargo: `cargo 1.96.0 (30a34c682 2026-05-25)`
- build exit: `0` (warnings present; no errors)
- pinned binary SHA-256: `33d29c847870840d555a5dcfeb38a9083e972e7217efd624c77af6cf42726fd4`

### 7.2 CLI help sample

**`[VERIFIED]`** The pinned binary help commands below all exited `0`; output was captured under `/tmp/wg-audit-concepts-help/`:

```bash
BIN=/home/bot/wg/target/debug/wg
$BIN --help
$BIN add --help
$BIN claim --help
$BIN done --help
$BIN runs --help
$BIN agent --help
$BIN agency --help
$BIN func --help
$BIN service --help
$BIN config --help
$BIN identity --help
$BIN provider --help
$BIN review --help
$BIN pilot --help
```

Bounded excerpts:

```text
wg - WG task management
Your most-used:
  ...
  agents          List or manage running agent processes (service workers)

wg agent --help:
  Manage agent definitions (identity: role + tradeoff pairings)
  ...
  See also: 'wg agents' to list running agent processes (service workers).

wg runs --help:
  Manage run snapshots (list, show, restore, diff)

wg pilot --help:
  WG-Pilot — turnkey family-team federation deploy
  (the deploy/UX wrapper ...; ships no new substrate)
```

**`[FACT]`** Attempting `wg agent-guide` from this worker context was refused by worker-control authority before content emission. The canonical text was therefore inspected directly at `src/text/agent_guide.md`, which `src/commands/agent_guide.rs:3-13` embeds with `include_str!`. This is E2 source evidence, not successful help execution.

### 7.3 Executed conceptual behavior fixtures

#### Failed predecessor does not satisfy an ordinary dependency

**`[VERIFIED]`** In an isolated temporary HOME/project, using the pinned binary, this sequence exited successfully through `fail`; `ready` exited `0` and returned no tasks:

```bash
wg init --no-agency
wg add A --id a
wg add B --id b --after a
wg publish a --wcc
wg claim a --actor tester
wg fail a --reason 'audit fixture'
wg ready --json
wg list --all
```

Bounded result:

```text
Marked 'a' as failed (audit fixture) (retry #1)
[]
[ ] b - B
[F] a - A
```

Environment: `/tmp/wg-concept-deps/project`, isolated `HOME`, all inherited `WG_*` worker-control variables removed; date 2026-08-08 UTC. This verifies only this input against the pinned build.

#### Advertised `--converged` is rejected

**`[VERIFIED]`** In another isolated graph:

```bash
wg init --no-agency
wg add 'Cycle item' --id cycle-item --max-iterations 2
wg done cycle-item --converged
```

Exit: `1`.

```text
Error: legacy wg done bypass/merge/cycle flags are not supported by publication-derived completion
```

The graph still contained `cycle_config.max_iterations: 2`, so the fixture proves the completion-option contradiction, not wholesale absence of cycle data.

#### Bare `wg done` requires a candidate

**`[VERIFIED]`** In an isolated initialized Git repository:

```bash
wg init --no-agency
wg add Report --id report
wg publish report --only
wg claim report --actor tester
wg done report
```

Exit: `1`.

```text
Error: missing completion candidate
```

### 7.4 Source and document evidence index

| Evidence | Observation | Class / freshness |
|---|---|---|
| `README.md:1-62`, `93-119`, `188-199` | product positioning, binaries, Pi claim, storage map | E4, snapshot-current/undated prose |
| `src/graph.rs:322-366`, `379-529`, `689-1035`, `2530-2591` | completion contracts/dispositions, statuses, task, trust, node types | E2, snapshot-current |
| `src/lifecycle.rs:29-86`, `181-270` | actor kinds, attempts, lifecycle projection, transitions | E2, snapshot-current |
| `src/attempt_runtime.rs:1-74` | exact attempt runtime key/namespace | E2, snapshot-current |
| `src/service/registry.rs:1-90` | runtime agent registry and process status | E2, snapshot-current |
| `src/runs.rs:1-128` | run means replay snapshot in this subsystem | E2, snapshot-current |
| `src/function.rs:31-60`, `300-335` | trace-function and function-run summary types | E2, snapshot-current |
| `src/agency/types.rs:328-535` | agency primitives, role, tradeoff, agent | E2, snapshot-current |
| `src/config.rs:1623-1765` | dispatch roles | E2, snapshot-current |
| `src/dispatch/handler_for_model.rs:1-105` | handler-first model grammar and term overlap | E2, snapshot-current |
| `src/dispatch/plan.rs:57-109` | internal ExecutorKind | E2, snapshot-current |
| `src/identity/keys.rs:1-25`, `110-161` | custody claim and wgid derivation | E2, snapshot-current |
| `src/identity/envelope.rs:41-78` | federated AgentFields and IdentityRecord | E2, snapshot-current |
| `src/identity/custody.rs:294-332`, `407-485` | capability and delegation | E2, snapshot-current |
| `src/trust.rs:1-53`, `79-125` | split author/provider trust resolution | E2, snapshot-current |
| `src/federation.rs:50-75` | PeerConfig trust comment | E2 comment/schema, snapshot-current |
| `src/review/mod.rs:1-40`, `63-172`, `320-432` | review boundary and stale deterministic-only overview | E2, snapshot-current |
| `src/providers/mod.rs:292-381`, `436-591` | compute provider registry and wire objects | E2, snapshot-current |
| `src/providers/placement.rs:1-246` | fail-closed leash | E2, snapshot-current |
| `src/commands/completion_done.rs:32-132` | derived Done enforcement | E2, snapshot-current |
| `src/text/agent_guide.md:44-68`, `215-292` | normative actor roles and completion contract | E4 embedded contract + E2 bundled text |
| `schemas/tool-manifest.schema.json:1-57`, `154-181` | only sampled/published schema and executor/context vocabulary | E2, snapshot-current |
| `docs/manual/01-overview.md:1-121` | graph/agency/core-loop conceptual narrative | E4, undated |
| `docs/manual/02-task-graph.md:1-213`, `228-386`, `441-447` | task/status/dependency/cycle/storage claims | E4, undated |
| `docs/AGENT-SERVICE.md:1-48`, `121-347` | daemon/dispatcher/coordinator/handler narrative | E4, undated |
| `docs/AGENCY.md:1-85` | public agency definitions | E4, undated |
| `docs/ADR-actor-vs-agent-identity.md:1-112` | accepted identity-unification decision | E4, accepted 2026-02-13 |
| `docs/design-handler-first-model-spec.md:1-45` | proposed/design-only status and target grammar | E4, proposed 2026-06-23 |
| `docs/design/trace-functions.md:1-34` | trace/replay/function distinction | E4, status not explicit |
| `docs/design/trace-function-protocol.md:1-50` | current-state claim and CLI rename note | E4, undated |

### 7.5 Schema inventory command

**`[VERIFIED]`** Executed against the exported pinned snapshot; exit `0`:

```bash
find . -path './target' -prune -o -path './.git' -prune -o \
  -type f \( -iname '*schema*.json' -o -iname '*.schema.json' \
  -o -iname '*schema*.yaml' \) -print | sort
```

Output:

```text
./schemas/tool-manifest.schema.json
```

**`[UNCERTAINTY]`** This filename search does not prove no schema-like contract exists under another name; it establishes that no other conventionally named JSON/YAML schema file was found. Rust Serde types remain schema-bearing implementation evidence.

### 7.6 Validation and limitations

**`[VERIFIED]`** Artifact validation performed after writing:

- `test -s docs/audit/2026-08-08-worksgood-system/19-conceptual-model-and-vocabulary.md`
- checked for concise product model, glossary candidate, three diagrams, ambiguity register, evidence-linked recommendations, and all required seven sections
- `git diff --check`

**`[FACT]`** No production source, test, schema, or pre-existing documentation file was edited. No full `cargo test`, smoke manifest, service, TUI, browser, live model, federation transport, provider execution, or Pilot scenario was run for this leaf audit. Presence of an implementation branch or test source is not presented as verified runtime behavior.
