# Code architecture, boundaries, and persistence model

**Audit date:** 2026-08-08
**Evidence checked through:** 2026-08-08
**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`
**Inspection checkout:** `98b319c36aa8a21fd4506fc7469fe6d58978cdda` (the charter-only successor; `git diff --quiet b0892e… -- Cargo.toml Cargo.lock src adapters/casa/src` returned 0)
**Freshness:** snapshot-current for cited source; executed behavior used a local debug build from the source-equivalent inspection checkout
**Scope:** Rust package/binaries, CLI parsing and dispatch, major module boundaries, graph/config/runtime persistence, locking/atomicity, and representative CLI-to-state flows
**Change boundary:** this audit artifact only
**Normative charter:** `docs/audit/2026-08-08-worksgood-system/README.md:1-10`, especially the fractal/evidence contract at `196-374`

## 1. Executive abstract

**`[FACT]`** WorksGood is one Cargo package and one library crate, not a multi-crate Rust workspace. The package declares four binaries: the comprehensive `wg` CLI, the thin attended `worksgood` launcher, the standalone/compatibility `nex` REPL, and the companion `casa-adapter` (`Cargo.toml:1-41`; `cargo metadata --no-deps --format-version 1`, evidence A1). The library exports 95 top-level modules, while `wg` additionally compiles a binary-private CLI, 198 command files, TUI, terminal host, and worker-capability adapter (`src/lib.rs:20-117`; `src/main.rs:17-25`; `src/commands/mod.rs:1-154`).

**`[FACT]`** The core graph is a `HashMap<String, Node>` whose serialized node variants are `Task`, `Resource`, and `ArchivedBoundary`; dependency edges are fields on `Task`, not a separately persisted graph relation (`src/graph.rs:689-1046`, `2577-2593`, `2705-2778`). `graph.jsonl` is a compatibility projection rewritten as one JSON object per node. `parser::modify_graph` holds `graph.lock` across load–mutate–lifecycle-ledger append–replace, and the lifecycle ledger is fsynced before the graph projection is atomically renamed (`src/parser.rs:285-395`; `src/lifecycle.rs:1526-1694`).

**`[VERIFIED]`** On Linux, the focused no-lost-update test and both lifecycle crash-replay tests passed. A clean-environment terminal fixture also completed `wg add` → `wg show --json` and `wg config set` → `wg config get`, observing a paused `open` task in one graph row, `graph.lock`, and an atomically written local TOML value (evidence A3–A4).

**`[INFERENCE]`** The most important architectural risk is migration overlap rather than absence of safety mechanisms. There are two workgraph-directory resolvers, two materially different completion implementations, a 158-variant CLI dispatched by a 4,739-line `main`, global/local/project-profile configuration authorities, and several bespoke persistence protocols. These seams make it possible for individually careful components to disagree about which path is current. Confidence is high for the structural claim; the system-wide runtime impact was only sampled.

**`[CONTRADICTION]`** Completion is the clearest realized drift. Clap still exposes legacy `wg done` flags, the ordinary CLI rejects every one, the worker adapter accepts two and the daemon then rejects them, while active smoke-manifest scenarios still invoke rejected flags (`src/cli.rs:527-557`; `src/main.rs:1261-1274`; `src/worker_cli.rs:345-361`; `src/commands/service/ipc.rs:908-919`; `tests/smoke/scenarios/eval_gate_low_score_fail_closed.sh:113-130`; `tests/smoke/manifest.toml:2630-2637`). After removing the ambient worker capability from the test process, the focused `integration_cli_workflows::test_done_via_cli` also failed because its legacy fixture has no immutable completion candidate (evidence A5).

**`[RECOMMENDATION]`** The next decision should be to finish or explicitly bound the completion-v3 migration: elect one completion authority per task class, make parser/help/worker IPC agree on flags, and quarantine or update incompatible active tests before further architectural expansion (`ARCH-REC-001`, P0).

## 2. Scope and map

### 2.1 Charter requirements applied here

**`[DOC-CLAIM]`** The dependency artifact `docs/audit/2026-08-08-worksgood-system/README.md` is the normative audit charter, not product-behavior evidence. It requires seven ordered fractal sections (`docs/audit/2026-08-08-worksgood-system/README.md:196-227`), visible statement labels (`docs/audit/2026-08-08-worksgood-system/README.md:229-247`), evidence-class/citation discipline (`docs/audit/2026-08-08-worksgood-system/README.md:249-294`), stable finding IDs with severity/likelihood/confidence (`docs/audit/2026-08-08-worksgood-system/README.md:296-326`), snapshot freshness (`docs/audit/2026-08-08-worksgood-system/README.md:328-350`), and an audit-only change boundary (`docs/audit/2026-08-08-worksgood-system/README.md:352-374`).

**`[FACT]`** This leaf maps those requirements as follows: sections 1–7 implement the ordered fractal contract; the component/state maps and four flows are in section 2; `ARCH-001`–`ARCH-009` use stable records in section 3; contradictions, risks, and typed recommendations are sections 4–6; and section 7 separates inspected E2/E3 evidence from executed E1 commands and limitations. The header inherits the charter's snapshot and identifies the source-equivalent inspection checkout. The only repository file changed by this task is this new audit artifact.

### 2.2 Package and executable map

**`[FACT]`** `cargo metadata` reported one workspace member and these product targets:

| Target | Entry point | Boundary and primary dependencies |
|---|---|---|
| library `worksgood` | `src/lib.rs:20-144` | Broad reusable surface: graph, parser/lifecycle, config/profile, service/dispatch, completion, identity/review/providers, chat/HTML, worker control. Re-exports graph persistence and service registry types. |
| binary `wg` | `src/main.rs:17-25`, `702-4739` | Owns `cli`, `commands`, `terminal_host`, `tui`, and `worker_cli` as binary-private modules. Parses 158 top-level commands and directly dispatches them from one match. |
| binary `worksgood` | `src/bin/worksgood.rs:1-151` | Thin existing-graph TUI launcher plus explicit concierge lifecycle/setup. Calls library `concierge`; does not share `wg`'s binary CLI. |
| binary `nex` | `src/bin/nex.rs:1-93` | Separate Clap surface; resolves standalone or legacy WG-compatible runtime through library `nex_runtime` and `workgraph_dir`. |
| binary `casa-adapter` | `adapters/casa/src/main.rs:1-160` | Companion adapter inside the same package. Owns channel/election/store policy while calling WG identity transport and review verdict APIs. |

**`[INFERENCE]`** A single package lowers dependency/version coordination cost, but it does not create a strong compile-time boundary between core and products: all four binaries can reach the broad public library, and `wg` has a second large binary-private application layer. A falsifying check would be a dependency-policy test or visibility rule proving that binaries cannot import disallowed modules; none was found in this sample.

### 2.3 Navigable component map

**`[FACT]`** The following map is based on module declarations and inspected call sites, not directory names alone.

```text
Cargo package worksgood
│
├─ library src/lib.rs (95 public top-level modules)
│  ├─ domain aggregate
│  │  ├─ graph.rs ─ Task/Status/Node/WorkGraph, derived cycle analysis
│  │  ├─ query.rs/check.rs/cycle.rs ─ readiness, reverse edges, validation
│  │  └─ lifecycle.rs ─ typed transition kernel + append-only event ledger
│  ├─ persistence adapters
│  │  ├─ parser.rs ─ graph.lock + JSONL load/replace + ledger integration
│  │  ├─ atomic_file.rs / lock.rs ─ generic atomic writes and retry policy
│  │  ├─ attempt_runtime.rs ─ source-tuple namespaced evidence
│  │  └─ provenance.rs ─ append/rotate operation history
│  ├─ orchestration
│  │  ├─ service/ ─ registry, coordinator, IPC/control plane
│  │  ├─ dispatch/ + execution_selection.rs
│  │  └─ worker_control.rs / attempt_runtime.rs / worktree_observer.rs
│  ├─ completion
│  │  ├─ completion_{manifest,evidence,review,task}.rs
│  │  ├─ save_transaction.rs / work_save.rs
│  │  └─ finalization.rs / merge_resolution.rs / simple_land.rs
│  ├─ configuration/model boundary
│  │  ├─ config.rs + config_defaults.rs + config_migrate.rs
│  │  ├─ profile/{named,project,...}.rs
│  │  └─ models.rs / executor/ / pi_* / secret.rs
│  └─ cross-plane features
│     ├─ identity/ + federation.rs + trust.rs
│     ├─ providers/ + review/
│     └─ agency/ + evaluation/ + chat* + html.rs + notify/
│
├─ wg binary
│  ├─ cli.rs (158 top-level variants)
│  ├─ main.rs (resolution, capability interception, usage log, direct dispatch)
│  ├─ commands/ (198 Rust files)
│  └─ tui/ + terminal_host/ + worker_cli.rs
│
├─ worksgood binary → concierge library
├─ nex binary → nex_runtime + library-included commands/nex.rs
└─ casa-adapter binary → adapter-local model/store + identity/review APIs
```

### 2.4 State and persistence map

**`[FACT]`** There is no single database or repository abstraction. Important durable authorities and projections include:

| State | Path/shape | Writer/locking boundary | Readback |
|---|---|---|---|
| Task/resource graph | `<graph>/graph.jsonl`, one `Node` per line | `parser::{save_graph,modify_graph}`; `graph.lock`; same-directory temp + fsync + rename (`src/parser.rs:225-395`) | `parser::load_graph`, with nonblocking shared-lock attempt and in-memory ledger replay |
| Lifecycle authority | `<graph>/lifecycle/events.jsonl`, checksummed newline frames | Appended and `sync_all` under `graph.lock` before graph replacement (`src/lifecycle.rs:1526-1664`) | `replay_ledger` applies revisions newer than task projection (`src/lifecycle.rs:1668-1694`) |
| Attempt runtime | `<graph>/attempts/by-source-tuple/<blake3>/...` | Create-new tuple manifest and exact tuple checks (`src/attempt_runtime.rs:1-135`, `175-276`) | Exact tuple first, legacy flat slot only after embedded tuple match |
| Agent registry | `<graph>/service/registry.json` | Separate `.registry.lock` on Unix; bespoke temp/sync/rename (`src/service/registry.rs:1-21`, `177-324`) | `AgentRegistry::load` or warning/default wrappers |
| Local/global config | `<graph>/config.toml`, `$WG_GLOBAL_DIR/config.toml` or `~/.wg/config.toml` | Generic `atomic_file::write_atomic` for typed saves/setter (`src/config.rs:6338-6350`, `6681-6702`; `src/commands/config_cmd.rs:3028-3109`) | Deep global→local merge, then project-profile routing overlay (`src/config.rs:5949-6142`) |
| Profiles | global `profiles/*.toml`, `active-profile`; project `profile-selection.json` | Generic atomic helpers; project selection content-fingerprint checks (`src/profile/named.rs:1-16`, `97-145`; `src/profile/project.rs:1-27`, `183-220`) | Legacy active profile is materialized globally; project association overlays in memory |
| Operation provenance | `<graph>/log/operations.jsonl` + compressed rotations | Append and size-triggered rotation, no lock in this module (`src/provenance.rs:1-117`) | Full rotated/current scan; add guardrail counts `add_task` rows |
| Completion objects | content-addressed completion store plus compact refs on `Task` | Submit/review/land commands, then `completion_done::commit_done` through `modify_graph` | `wg show` exposes candidate/receipt refs; completion modules resolve immutable bytes |

**`[FACT]`** Generic atomic files sync both the file and parent directory (`src/atomic_file.rs:31-66`, `129-142`), but graph and registry replacements use separate implementations that sync the temporary file and rename without an explicit parent-directory fsync (`src/parser.rs:303-353`; `src/service/registry.rs:218-247`). This distinction matters to durability claims, although no power-loss test was run.

### 2.5 Representative end-to-end control flows

#### Flow A — `wg add` creates a visible draft (executed)

1. **`[FACT]`** Clap parses `Commands::Add` (`src/cli.rs:156-369`); `main` first offers the command to the worker-capability adapter, resolves/canonicalizes the graph directory, records command usage, and enters its direct match (`src/main.rs:702-825`, `1014-1260`).
2. **`[FACT]`** Public add is forced to `paused = true`; `main` forwards more than forty scalar/vector arguments to `commands::add::run_with_remote_provider` (`src/main.rs:1014-1260`; `src/commands/add.rs:302-417`).
3. **`[FACT]`** Add validates route/scope/visibility/timing, reads config and the provenance-count guardrail, then enters one `modify_graph` closure (`src/commands/add.rs:418-575`). Inside the lock it derives dependencies and ID, constructs the 101-field `Task`, inserts `Node::Task`, and repairs forward/back links (`src/commands/add.rs:576-833`).
4. **`[FACT]`** `modify_graph` replays lifecycle frames, clones the before-state, applies the closure, bumps interaction timestamps, appends new lifecycle events, and replaces the graph (`src/parser.rs:377-414`). Add then sends best-effort notifications and records provenance after graph commit (`src/commands/add.rs:843-865`).
5. **`[VERIFIED]`** The fixture observed `id=audit-flow`, `status=open`, `paused=true`, `completion_contract=land`, one JSONL row, and `graph.lock` (evidence A4).

#### Flow B — `wg show` assembles a composite read model (executed)

1. **`[FACT]`** `main` restores Unix SIGPIPE behavior and calls `commands::show::run` (`src/main.rs:662-700`, `1736-1739`).
2. **`[FACT]`** `load_workgraph` maps the directory to `graph.jsonl`; `load_graph` attempts a nonblocking shared flock and may read without it during exclusive contention because the graph replacement is atomic (`src/commands/mod.rs:159-182`; `src/parser.rs:275-294`).
3. **`[FACT]`** Show gets the task, derives reverse edges from `after`, resolves remote dependencies, and then reads ancillary state from output/stream files, the agent registry, config, evaluation files, attempt-runtime observer/watchdog projections, worktree/Git state, and cron diagnostics (`src/commands/show.rs:603-850`; `route_pin_info` at `246-292`; `gather_task_runtime_info` at `365-462`).
4. **`[FACT]`** The output is a separate 86-field `TaskDetails` projection, serialized as JSON or formatted for humans (`src/commands/show.rs:40-222`, `784-847`).
5. **`[VERIFIED]`** The fixture's JSON readback matched the row written by add (evidence A4).

**`[INFERENCE]`** Show is not a point-in-time snapshot across those stores: only graph loading has graph-lock semantics, and ancillary reads occur afterward. A concurrent coordinator can therefore make runtime/evaluation/config portions newer than the graph portion. Confidence is high from control flow; no race was forced. This is acceptable for diagnostics only if the contract states that consistency level.

#### Flow C — `wg done` derives completion from immutable review/publication (static trace plus failing stale test)

1. **`[FACT]`** Outside worker capability mode, `main` rejects every legacy done flag and calls `completion_done::run` (`src/main.rs:1261-1274`). In worker mode, `worker_cli` converts own-task Done to a typed `DoneHandoff`; daemon IPC rejects `converged/full_smoke` and calls the same completion-v3 function (`src/worker_cli.rs:345-361`; `src/commands/service/ipc.rs:908-919`).
2. **`[FACT]`** `completion_done::run` loads the graph and task, checks actor ownership, loads submission/manifest/requirements/summary, re-resolves dependency outputs and exact FLIP/eval reviews, and verifies Git publication for `Land` contracts (`src/commands/completion_done.rs:33-119`, `143-198`).
3. **`[FACT]`** It writes a content-addressed completion receipt, then `commit_done` rechecks generation and manifest digest inside `modify_graph`; only then does the lifecycle kernel project `AttemptSucceeded`, disposition, receipt, timestamps, ownership release, and log (`src/commands/completion_done.rs:96-136`, `220-294`).
4. **`[VERIFIED]`** The focused legacy CLI integration test did not reach Done: with worker-capability variables scrubbed it exited 101 because the fixture task lacked a completion candidate. The test still expects direct `InProgress → Done` (`tests/integration_cli_workflows.rs:358-383`; evidence A5).

#### Flow D — `wg config set/get` mutates and reads layered configuration (executed)

1. **`[FACT]`** `main` handles the nested config subcommand inside the same global dispatch match and maps default/local/global scope (`src/main.rs:2910-3067`).
2. **`[FACT]`** `set_dotted` validates typed values, loads only the selected scope as raw TOML, applies a dotted key, deserializes the whole document for validation, atomically writes it, optionally reloads/restarts the daemon, and re-reads the effective value/source (`src/commands/config_cmd.rs:3028-3118`).
3. **`[FACT]`** Effective readback uses `Config::load_with_sources`; the related merged path normalizes legacy tables, deep-merges global then local, and overlays a fingerprinted project profile for routing (`src/config.rs:5949-6142`, `6681-6760`).
4. **`[VERIFIED]`** The fixture set `help.ordering=alphabetical`, read it back with source `local`, and observed `[help] ordering = "alphabetical"` in local TOML (evidence A4).

## 3. Findings

### `ARCH-001` — graph mutation has a strong Unix serialization/recovery spine

- **Label/state:** **`[FACT]`**, **`[VERIFIED]`**; shipped/current.
- **Severity/likelihood/confidence:** S4 positive control; observed; high.
- **Claim:** The central `modify_graph` path prevents read–modify–write loss under Unix flock, writes lifecycle events before replacing the compatibility projection, and repairs missing/torn lifecycle projection state on replay.
- **Evidence:** `src/parser.rs:76-232`, `285-414`; `src/lifecycle.rs:1526-1694`; focused tests at `src/parser.rs:1119-1205` and `src/lifecycle.rs:2194-2296`; evidence A3.
- **Counterevidence/limit:** This does not cover non-Unix locking, power-loss directory durability, or all non-graph state stores.
- **Owner/recommendation:** core persistence; preserve with `ARCH-REC-003`.

### `ARCH-002` — the core aggregate and CLI/read models are oversized and duplicative

- **Label/state:** **`[FACT]`**, **`[INFERENCE]`**; shipped/current.
- **Severity/likelihood/confidence:** S2; likely maintainability impact; high structure confidence, medium defect-prediction confidence.
- **Claim:** `Task` spans 101 public fields over 358 lines; `TaskDetails` manually mirrors 86 fields; `Config` is 12,322 lines; `cli.rs` is 7,329; and `main.rs` directly dispatches 158 variants across 4,739 lines. The three largest files are TUI state/render/event at 39,901/22,268/14,862 lines (evidence A2).
- **Evidence:** `src/graph.rs:689-1046`; `src/commands/show.rs:40-222`, `603-847`; `src/config.rs:215-377`, `3303-5420`, `5808-6770`; `src/cli.rs:11-38`; `src/main.rs:702-4739`.
- **Counterevidence:** Commands are split into 198 files and many domain kernels are already separate/pure (`lifecycle.rs`, `save_transaction.rs`). Line count alone is not a bug.
- **Inference:** Adding a field or command crosses parser, dispatch, graph/default/deserialization, read model, renderers, and tests, increasing drift probability.
- **Owner/recommendation:** CLI/core architecture; `ARCH-REC-002`.

### `ARCH-003` — dependency edges have duplicate persisted representations

- **Label/state:** **`[FACT]`**, **`[INFERENCE]`**; shipped/current compatibility design.
- **Severity/likelihood/confidence:** S2; possible; high.
- **Claim:** Each task persists both `after` and `before`, while core queries rebuild the reverse index exclusively from `after`. Add and remove paths explicitly maintain both directions (`src/graph.rs:718-727`, `2840-2863`; `src/query.rs:267-279`; `src/commands/add.rs:786-833`).
- **Inference:** `after` behaves as de facto authority and `before` as a denormalized cache, but that authority is not encoded in the type/schema. Any writer that updates only one side can leave contradictory rows; readers disagree depending on which field they use.
- **Counterevidence:** Add and `WorkGraph::remove_node` contain repair logic, and `show` safely derives dependents rather than trusting `before`.
- **Owner/recommendation:** graph model; `ARCH-REC-004`.

### `ARCH-004` — graph-directory resolution is duplicated across binary and library

- **Label/state:** **`[FACT]`**; shipped/current.
- **Severity/likelihood/confidence:** S3; likely eventual drift; high.
- **Claim:** `wg` uses private copies of `resolve_workgraph_dir` and `descend_into_wg_subdir_if_project_root`, while `nex` uses public `worksgood::workgraph_dir` versions (`src/main.rs:28-151`, `761-766`; `src/workgraph_dir.rs:1-80`; `src/bin/nex.rs:25-36`). The private copy has extensive inline tests; the public 80-line module has no local tests.
- **Counterevidence:** The sampled implementations currently encode the same precedence and descent behavior.
- **Owner/recommendation:** executable boundary; `ARCH-REC-005`.

### `ARCH-005` — completion has duplicate authorities and active code/test drift

- **Label/state:** **`[CONTRADICTION]`**, **`[VERIFIED]`**; partial migration/current drift.
- **Severity/likelihood/confidence:** S1; possible broad gate failure; high.
- **Claim:** The current ordinary/IPC path uses the 294-line publication-derived `completion_done.rs`, but the 5,622-line legacy `commands/done.rs` remains compiled and is called by user-board archive and finalization settlement (`src/main.rs:1261-1274`; `src/commands/user.rs:110-140`; `src/commands/finalize.rs:2541-2600`). Clap exposes legacy flags that the ordinary path rejects; the worker adapter and IPC disagree on two of those flags; active smoke scenarios still pass rejected flags.
- **Evidence:** `src/cli.rs:527-557`; `src/worker_cli.rs:345-361`; `src/commands/service/ipc.rs:908-919`; `tests/smoke/manifest.toml:2630-2637`, `2785-2793`; `tests/smoke/scenarios/eval_gate_low_score_fail_closed.sh:113-130`; `tests/smoke/scenarios/candidate_finalization_transaction.sh:55-62`; evidence A5.
- **Counterevidence:** Completion-v3 canary tests directly exercise `completion_done::run` (`src/commands/completion_canary_tests.rs:1-286` [inspected, not run]). Some legacy call sites may intentionally serve special task classes.
- **Uncertainty:** Full smoke was not run, so the number of manifest scenarios that fail on this checkout is unknown.
- **Owner/recommendation:** completion/runtime + testing; `ARCH-REC-001`.

### `ARCH-006` — persistence safety is fragmented and non-Unix graph/registry locking is a no-op

- **Label/state:** **`[FACT]`**, **`[INFERENCE]`**; shipped/current.
- **Severity/likelihood/confidence:** S2; possible; high for code, medium for operational impact.
- **Claim:** Graph and service-registry locks become no-op guards on non-Unix (`src/parser.rs:90-94`, `154-157`; `src/service/registry.rs:299-324`). Generic atomic writes sync the parent directory, but graph/registry bespoke replacements do not (`src/atomic_file.rs:31-66`, `129-142`; `src/parser.rs:303-353`; `src/service/registry.rs:218-247`). Static inventory found 25 Rust files using an atomic helper, 168 containing direct `fs::write`, and 31 containing rename calls; counts include tests (evidence A2).
- **Inference:** Atomic rename prevents torn files but does not serialize concurrent non-Unix load–modify–save or, by itself, guarantee post-power-loss directory entry durability. Similar state classes therefore have different guarantees.
- **Counterevidence:** The package does compile platform-specific Windows support, and CI may catch functional Windows regressions; this audit did not run Windows or power-loss tests.
- **Owner/recommendation:** persistence/platform; `ARCH-REC-003`.

### `ARCH-007` — configuration has explicit layers but fallback and profile authorities remain complex

- **Label/state:** **`[FACT]`**, **`[INFERENCE]`**; shipped/current.
- **Severity/likelihood/confidence:** S2; possible misconfiguration; high.
- **Claim:** Effective config can depend on global TOML, local TOML, legacy global path, materialized global active profile, fingerprinted project profile, defaults, environment credentials, and task-level route/profile fields (`src/config.rs:5808-6142`, `6200-6328`; `src/profile/named.rs:1-16`, `97-145`; `src/profile/project.rs:1-27`; `src/graph.rs:803-846`). Static inventory found 141 `Config::load_or_default` references in 69 source files versus 60 `load_merged` references in 31 files, including tests (evidence A2).
- **Positive control:** Explicit project-profile failure returns a blocked sentinel route rather than silently selecting another provider (`src/config.rs:6028-6057`, `6299-6334`); config writes use the generic atomic helper.
- **Inference:** Outside explicit project-profile failure, `load_or_default` converts load errors to defaults plus diagnostics. Reusable callers that do not emit/inspect diagnostics can continue with fallback behavior, making call-site discipline part of configuration correctness.
- **Owner/recommendation:** configuration/model plane; `ARCH-REC-006`.

### `ARCH-008` — autopoietic child-task guardrails depend on best-effort, nontransactional provenance

- **Label/state:** **`[FACT]`**, **`[INFERENCE]`**; shipped/current.
- **Severity/likelihood/confidence:** S2; possible; high.
- **Claim:** Add enforces `max_child_tasks_per_agent` by counting `add_task` rows in the operation log before acquiring the graph mutation lock. Read errors count as zero. After committing the graph, add ignores provenance-recording failure (`src/commands/add.rs:438-463`, `843-865`, `1376-1391`). The provenance module performs an unlocked size check/rotation/append (`src/provenance.rs:43-117`).
- **Inference:** Two concurrent adds can both observe the same count, and a successful graph mutation with failed provenance can permanently undercount. The configured maximum is therefore advisory under races/I/O failure, despite being presented as an enforced guardrail.
- **Counterevidence:** Scope guardrails and graph mutation itself remain enforced; no concurrent bypass was executed.
- **Owner/recommendation:** agency/core persistence; `ARCH-REC-007`.

### `ARCH-009` — worker capability interception is a strong boundary with a test-isolation coupling

- **Label/state:** **`[FACT]`**, **`[VERIFIED]`**; shipped/current.
- **Severity/likelihood/confidence:** S3 architecture/test seam; observed in this harness; high.
- **Claim:** Presence of `WG_WORKER_CAPABILITY` switches the entire CLI into a typed, own-task-only IPC mode before graph discovery or usage logging (`src/main.rs:735-748`; `src/worker_cli.rs:1-8`, `121-135`). This is a positive authority boundary. However, a focused integration helper removed several `WG_*` variables but not capability/protocol/IPC; under this WG worker it failed with `worker_control.cross_task_refused` until the outer test process was scrubbed (`tests/integration_cli_workflows.rs:24-42`; evidence A5).
- **Inference:** Tests that shell out must use one canonical environment-scrubbing helper; ad hoc lists are coupled to every new authority variable.
- **Owner/recommendation:** worker control/testing; `ARCH-REC-008`.

## 4. Contradictions and drift

| ID | Evidence A | Evidence B | State / impact | Severity | Likelihood | Confidence |
|---|---|---|---|---:|---|---|
| `ARCH-DRIFT-001` | Clap describes `--converged`, `--full-smoke`, and bypass flags as supported Done options (`src/cli.rs:527-557`). | Ordinary main rejects all flags; worker adapter accepts `converged/full_smoke`, then IPC rejects them (`src/main.rs:1261-1274`; `src/worker_cli.rs:345-361`; `src/commands/service/ipc.rs:908-919`). | **`[CONTRADICTION]`** Open; compiled help and execution layers disagree. Current mutation authority is main/IPC refusal. | S1 | possible | high |
| `ARCH-DRIFT-002` | Active manifest scenarios invoke `--ignore-unmerged-worktree --skip-smoke` or `--skip-smoke` (`tests/smoke/manifest.toml:2630-2637`, `2785-2793`; cited scripts). | Both ordinary and worker paths refuse those flags before completion. | **`[CONTRADICTION]`** Open; full extent unexecuted. Testing audit should run/name-level triage. | S1 | possible | high |
| `ARCH-DRIFT-003` | Integration test expects raw `InProgress → Done` from `wg done` (`tests/integration_cli_workflows.rs:358-383`). | Completion-v3 requires an immutable candidate and exact reviews (`src/commands/completion_done.rs:33-119`). | **`[VERIFIED]`** Open; focused test failed `missing completion candidate` after environment isolation. | S2 | observed | high |
| `ARCH-DRIFT-004` | `lifecycle.rs` calls its kernel the only production status-edge decider and its ledger authoritative (`src/lifecycle.rs:1-9`). | `graph.jsonl` retains mutable `Task.status` as compatibility projection and two completion implementations remain reachable for distinct paths. | **`[UNCERTAINTY]`** Open; architectural duality is explicit, not automatically a defect. A production call-graph audit is needed. | S2 | possible | medium |
| `ARCH-DRIFT-005` | Main and library each state the same workgraph-resolution contract (`src/main.rs:28-151`; `src/workgraph_dir.rs:1-80`). | Different binaries call different copies (`src/main.rs:761-766`; `src/bin/nex.rs:25-36`). | **`[FACT]`** Apparent/non-issue for current behavior; duplication remains accepted debt until consolidated. | S3 | likely drift over time | high |
| `ARCH-DRIFT-006` | Graph/parser comments promise atomic crash-safe replacement (`src/graph.rs:2699-2704`; `src/parser.rs:297-302`, `354-357`). | The graph replacement does not call the parent-directory sync used by the generic atomic helper (`src/atomic_file.rs:31-66`, `129-142`). | **`[UNCERTAINTY]`** Open; exact intended crash model (process crash vs host/power loss) is unspecified. | S2 | unknown | medium |

## 5. Risks and gaps

| ID | Severity | Likelihood | Confidence | Risk/gap | Boundary and residual uncertainty |
|---|---:|---|---|---|---|
| `ARCH-RISK-001` | S1 | possible | high | Completion migration drift can make help, worker behavior, integration tests, and smoke gates validate different protocols. | Core terminal lifecycle. Focused stale test failed; active smoke scenarios were inspected but not run. |
| `ARCH-RISK-002` | S2 | possible | high | Whole-graph rewrite is O(nodes) per mutation and iterates `HashMap::values()` without sorting, so row order is nondeterministic (`src/parser.rs:303-353`; `src/graph.rs:2705-2723`, `2780-2783`). | Large graphs may incur write amplification/noisy byte diffs. No benchmark or scale test was run in this audit. |
| `ARCH-RISK-003` | S2 | possible | high | Different stores have different locking, fsync, corruption, and fallback policies. | Cross-store operations (graph, registry, provenance, config, attempt state) have no single transaction. Intentional content-addressed protocols reduce but do not eliminate this gap. |
| `ARCH-RISK-004` | S2 | possible | medium | Denormalized edges and composite show output can present inconsistent state under partial writers/concurrency. | Graph/read-model boundary. No race was forced. |
| `ARCH-RISK-005` | S2 | possible | medium | Fail-open/default configuration loaders can hide a corrupt non-profile config unless the caller surfaces diagnostics. | Model/daemon/UI boundary. Call-site emission coverage was not exhaustively traced. |
| `ARCH-RISK-006` | S2 | possible | high | Provenance-backed limits can be exceeded under concurrency or logging failure. | Agency growth guardrail. Static inference only. |
| `ARCH-GAP-001` | S3 | observed gap | high | Non-Unix interprocess mutation safety was not verified. | Parser and registry explicitly use no-op locks there; no Windows host was available. |
| `ARCH-GAP-002` | S3 | observed gap | high | This audit did not run full Rust tests, full smoke, daemon dispatch, TUI, external providers, or destructive crash/power-loss tests. | Passing focused tests must not be generalized. |
| `ARCH-GAP-003` | S3 | observed gap | high | The rough coupling inventory counts lexical references and test code; it does not prove runtime reachability. | A compiler-derived module graph/call graph would refine it. |

## 6. Recommendations

### Factual synchronization work

1. **`ARCH-REC-001` — `[RECOMMENDATION]` (P0, completion + testing; links `ARCH-005`, drifts 001–003):** publish a completion-path matrix for operator CLI, capability worker, user board, watchdog/finalizer, and daemon IPC. Remove unsupported flags from Clap or implement them consistently; update/quarantine every active manifest scenario and integration fixture against the elected protocol. **Acceptance:** generated help, main, worker adapter, IPC, and all active completion scenarios agree; the focused CLI workflow passes with a candidate-based fixture.
2. **`ARCH-REC-008` — `[RECOMMENDATION]` (P1, testing/worker control; links `ARCH-009`):** provide one library/test helper that removes or constructs the complete worker authority environment. **Acceptance:** shell-out integration tests pass both inside and outside a WG worker without bespoke `env_remove` lists.

### Implementation architecture work

3. **`ARCH-REC-002` — `[RECOMMENDATION]` (P1, CLI/core; links `ARCH-002`):** split `main` into a small bootstrap plus typed command-family routers; move shared command entry points into library modules where cross-binary/test reuse is intended. Extract `TaskDetails` construction into a versioned read-model builder rather than manually mirroring `Task`. **Acceptance:** bootstrap owns only parse/context/dispatch, command families have bounded interfaces, and a compile/test check detects unmapped task fields.
4. **`ARCH-REC-003` — `[RECOMMENDATION]` (P1, persistence/platform; links `ARCH-001`, `ARCH-006`):** define one durability matrix (atomic visibility, interprocess serialization, file fsync, parent fsync, corruption recovery) and route graph/registry/config/runtime stores through reviewed primitives or explicit exceptions. Implement real Windows serialization. **Acceptance:** Unix and Windows concurrent writer tests, plus fault-injection tests at ledger append/temp sync/rename/parent-sync boundaries.
5. **`ARCH-REC-004` — `[RECOMMENDATION]` (P1, graph; links `ARCH-003`):** elect `after` as canonical or replace both vectors with typed edges; treat `before` as derived and validate/repair it at one boundary. **Acceptance:** schema invariant test injects contradictory backlinks and either rejects or deterministically repairs them; all readers use the same authority.
6. **`ARCH-REC-005` — `[RECOMMENDATION]` (P2, executable boundary; links `ARCH-004`):** delete the private resolver and call `worksgood::workgraph_dir` from `wg`; move resolver tests with it. **Acceptance:** all binaries share one resolver and a table-driven test covers CLI/env/walk-up/global/default/legacy descent.
7. **`ARCH-REC-006` — `[RECOMMENDATION]` (P1, config/model; links `ARCH-007`):** distinguish `load_required` (errors fail closed) from explicitly diagnostic/defaulted loads, and return a typed resolved-config snapshot with source provenance. **Acceptance:** execution/daemon entry points cannot compile while ignoring load errors; UI-only fallback is visibly degraded.
8. **`ARCH-REC-007` — `[RECOMMENDATION]` (P1, agency/persistence; links `ARCH-008`):** move child-creation accounting into the graph-locked authoritative mutation or a locked create-new ledger, and make provenance observational only. **Acceptance:** concurrent adds at `limit-1` admit at most one; injected provenance I/O failure cannot bypass the limit.
9. **`ARCH-REC-009` — `[RECOMMENDATION]` (P2, graph performance; links `ARCH-RISK-002`):** serialize nodes in stable ID order and benchmark rewrite cost at representative graph sizes before selecting incremental persistence. **Acceptance:** byte-identical save for unchanged logical graphs and published p50/p95 mutation bounds at 1k/10k/100k nodes.

### Human product/design decisions

10. **`ARCH-REC-010` — `[RECOMMENDATION]` (P1, product/core):** decide whether `graph.jsonl` is the public authoritative format or explicitly a compatibility projection of lifecycle/content-addressed authorities. **Acceptance:** one ADR names authority for status, edges, completion, and recovery, and operator repair tooling follows it.
11. **`ARCH-REC-011` — `[RECOMMENDATION]` (P2, product architecture):** decide whether Casa and future adapters should remain binary targets with full library reach or become crates behind a narrow SDK. **Acceptance:** dependency rules and versioning policy match the chosen plugin/companion boundary.

## 7. Evidence appendix

### 7.1 Primary source and tests inspected

| Evidence | Observation | Class/status |
|---|---|---|
| `docs/audit/2026-08-08-worksgood-system/README.md:1-10`, `194-374` | pinned snapshot/change boundary and normative fractal/evidence/freshness contract applied in section 2.1 | E4, snapshot-current charter |
| `Cargo.toml:1-56`; `src/lib.rs:20-144` | one package, four binary declarations, broad public library | E2, snapshot-current |
| `src/main.rs:28-151`, `702-4739`; `src/cli.rs:11-38`, `527-557`; `src/commands/mod.rs:1-198` | resolution, capability interception, 158-command parse/dispatch surface | E2 |
| `src/bin/{worksgood,nex}.rs`; `adapters/casa/src/main.rs` | distinct executable boundaries | E2 |
| `src/graph.rs:382-514`, `689-1046`, `2528-2930`; `src/query.rs:267-279` | status/task/node/workgraph and reverse-edge authority | E2 |
| `src/parser.rs:76-414`; `src/lock.rs:1-170`; `src/atomic_file.rs:1-142` | graph locks, retries, atomic replacement variants | E2 |
| `src/lifecycle.rs:1-9`, `1526-1694`; `src/save_transaction.rs:1-300` | authoritative lifecycle ledger and pure save reducer | E2 |
| `src/config.rs:215-377`, `5592-5630`, `5808-6770`; `src/profile/{named,project}.rs` | layered config/profile read/write authorities | E2 |
| `src/commands/{add,show,completion_done,done,config_cmd}.rs`; `src/worker_cli.rs`; `src/commands/service/ipc.rs` | four traced flows and completion split | E2 |
| `src/parser.rs:1047-1205`; `src/lifecycle.rs:2194-2296` | focused locking/replay assertions | E3, executed as E1 in A3 |
| `tests/integration_cli_workflows.rs:24-48`, `358-383` | shell-out environment helper and stale Done expectation | E3, executed/failing as E1 in A5 |
| `tests/smoke/manifest.toml:2630-2637`, `2785-2793`; cited scenario scripts | active legacy completion invocations | E3, inspected not run |

### 7.2 A1 — package metadata command

**`[VERIFIED]`** Executed 2026-08-08 on Linux from `/home/bot/wg/.wg-worktrees/agent-3`; exit 0.

```bash
cargo metadata --no-deps --format-version 1 |
python3 -c 'import json,sys; d=json.load(sys.stdin); print("workspace_members",len(d["workspace_members"])); p=d["packages"][0]; print("package",p["name"]); print("targets",[(t["name"],t["kind"]) for t in p["targets"] if "bin" in t["kind"] or "lib" in t["kind"]])'
```

```text
workspace_members 1
package worksgood
targets [('worksgood', ['lib']), ('casa-adapter', ['bin']), ('nex', ['bin']), ('wg', ['bin']), ('worksgood', ['bin'])]
```

### 7.3 A2 — quantitative inventory

**`[VERIFIED]`** Executed 2026-08-08; exit 0. Counts include inline tests and generated-looking checked-in Rust; lexical coupling counts are orientation, not runtime reachability.

```bash
python3 - <<'PY'
from pathlib import Path
import re

files = list(Path("src").rglob("*.rs"))
line_counts = {p: len(p.read_text(errors="ignore").splitlines()) for p in files}
print("rust_files", len(files), "loc", sum(line_counts.values()))
print("command_files", len(list(Path("src/commands").rglob("*.rs"))))
for path, lines in sorted(line_counts.items(), key=lambda item: item[1], reverse=True)[:20]:
    print(f"largest {lines} {path}")
lib = Path("src/lib.rs").read_text()
print("lib_pub_mod", len(re.findall(r"^pub mod ", lib, re.M)))

cli_lines = Path("src/cli.rs").read_text().splitlines()
in_commands = False
brace_depth = 0
variants = []
for line in cli_lines:
    if line.startswith("pub enum Commands {"):
        in_commands = True
        brace_depth = 1
        continue
    if in_commands:
        brace_depth += line.count("{") - line.count("}")
        match = re.match(r"^    ([A-Z][A-Za-z0-9_]*)\s*(?:\{|\(|,)", line)
        if brace_depth >= 1 and match:
            variants.append(match.group(1))
        if brace_depth == 0:
            break
print("cli_top_variants", len(variants))

def field_count(path, name, public_struct=True):
    lines = Path(path).read_text().splitlines()
    prefix = "pub struct" if public_struct else "struct"
    start = next(i for i, line in enumerate(lines)
                 if re.match(rf"{prefix} {name}\s*\{{", line))
    depth = 0
    fields = []
    for i, line in enumerate(lines[start:], start + 1):
        depth += line.count("{") - line.count("}")
        if depth == 1:
            match = re.match(r"\s+(?:pub\s+)?([A-Za-z0-9_]+)\s*:", line)
            if match:
                fields.append(match.group(1))
        if depth == 0:
            break
    return len(fields), start + 1, i

for path, name, public in [
    ("src/graph.rs", "Task", True),
    ("src/commands/show.rs", "TaskDetails", False),
    ("src/config.rs", "Config", True),
]:
    print("fields", name, *field_count(path, name, public))

for name in ["graph", "config", "parser", "lifecycle", "atomic_file", "service", "dispatch"]:
    pattern = re.compile(r"(?:crate|worksgood)::" + re.escape(name) + r"\b")
    counts = [len(pattern.findall(path.read_text(errors="ignore"))) for path in files]
    print("refs", name, sum(counts), sum(count > 0 for count in counts))

for label, pattern in [
    ("atomic_helper", r"atomic_file::write_atomic|\bwrite_atomic\("),
    ("direct_fs_write", r"(?:std::fs|fs)::write\("),
    ("rename", r"(?:std::fs|fs)::rename\("),
    ("modify_graph", r"\bmodify_graph\("),
    ("load_or_default", r"Config::load_or_default"),
    ("load_merged", r"Config::load_merged"),
]:
    regex = re.compile(pattern)
    counts = [len(regex.findall(path.read_text(errors="ignore"))) for path in files]
    print("pattern", label, "files", sum(count > 0 for count in counts), "hits", sum(counts))
PY
```

```text
src Rust files: 434; physical lines: 511,298
src/commands Rust files: 198
src/lib.rs public top-level modules: 95
CLI top-level Commands variants: 158
Task fields: 101; TaskDetails fields: 86; Config fields: 30
largest: state.rs 39,901; render.rs 22,268; event.rs 14,862;
         config.rs 12,322; service/mod.rs 7,878; spawn/execution.rs 7,332;
         cli.rs 7,329; commands/done.rs 5,622; graph.rs 5,236; main.rs 4,739
lexical references: graph 1,104/187 files; config 549/138; parser 420/128;
                    lifecycle 242/58; service 274/63; dispatch 107/37
files with atomic-helper patterns: 25; direct fs::write patterns: 168;
files with rename patterns: 31; files with modify_graph patterns: 68
Config::load_or_default: 141 occurrences; Config::load_merged: 60
```

### 7.4 A3 — build and focused persistence behavior

**`[VERIFIED]`** Linux `6.8.0-90-generic x86_64`, Rust/Cargo `1.96.0`; source-equivalent checkout; build exit 0 with warnings. Each focused test command exited 0.

```bash
cargo build --bin wg
cargo test --lib parser::tests::test_modify_graph_concurrent_no_lost_updates -- --exact
cargo test --lib lifecycle::tests::lifecycle_ledger_replays_after_projection_crash -- --exact
cargo test --lib lifecycle::tests::lifecycle_torn_final_ledger_frame_is_truncated_before_next_commit -- --exact
```

```text
wg debug build: PASS (warnings present)
modify_graph concurrent no-lost-updates: 1 passed
lifecycle ledger replay after projection crash: 1 passed
lifecycle torn final frame repair: 1 passed
```

### 7.5 A4 — isolated CLI add/show/config flow

**`[VERIFIED]`** Executed 2026-08-08 against `./target/debug/wg`; exit 0. `env -i` was required because a real WG worker capability intentionally forbids arbitrary graph fallback. Temporary files were removed afterward.

```bash
T=$(mktemp -d)
mkdir -p "$T/project/.wg" "$T/global" "$T/home"
: > "$T/project/.wg/graph.jsonl"
run(){ env -i PATH="$PATH" HOME="$T/home" USER=audit WG_GLOBAL_DIR="$T/global" \
  ./target/debug/wg --dir "$T/project/.wg" "$@"; }
run add "Audit flow task" --id audit-flow -d $'Evidence fixture\n\n## Validation\n- [ ] inspect'
run --json show audit-flow > "$T/show.json"
run config set help.ordering alphabetical --local --no-reload
run --json config get help.ordering > "$T/get.json"
# Python printed selected JSON fields, graph line/lock, and local TOML.
```

```text
show: id=audit-flow title="Audit flow task" status=open paused=true completion_contract=land
config_get: key=help.ordering value=alphabetical source=local
graph_lines=1; graph_lock_exists=true
config_body: [help] / ordering = "alphabetical"
```

### 7.6 A5 — completion-test drift and worker environment coupling

**`[VERIFIED]`** First execution inside the WG worker exited 101 with `worker_control.cross_task_refused`, proving the test helper had not removed the capability mode switch. The rerun below removed worker authority variables and still exited 101, now at the intended product path with `missing completion candidate`.

```bash
env -u WG_WORKER_CAPABILITY -u WG_WORKER_CONTROL_PROTOCOL -u WG_WORKER_IPC \
    -u WG_GRAPH_ID -u WG_TASK_ID -u WG_AGENT_ID -u WG_EXECUTOR_TYPE \
    -u WG_WORKTREE_PATH -u WG_SPAWN_RUN_ID \
  cargo test --test integration_cli_workflows test_done_via_cli -- --exact
```

```text
running 1 test
test test_done_via_cli ... FAILED
stderr from fixture wg: Error: missing completion candidate
test result: 0 passed; 1 failed; exit 101
```

### 7.7 Limitations and commands not run

**`[UNCERTAINTY]`** No full `cargo test`, full smoke, Windows test, daemon orchestration, network/provider, installer, TUI, formal, filesystem fault-injection, or power-loss test was run. The first attempted combined Cargo test command used multiple positional filters and exited 1 with Cargo usage error; no product behavior was exercised by that failed invocation. The focused build took long enough that expanding to full verification would have exceeded this leaf audit's bounded scope.

**`[UNCERTAINTY]`** Static counts are physical/lexical and include tests. Source line citations are valid at the pinned snapshot because the inspection checkout had no changes in `Cargo.toml`, `Cargo.lock`, `src/`, or `adapters/casa/src` relative to `b0892ea…`.
