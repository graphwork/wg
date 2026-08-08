# Product, documentation, verification, operations, UX, and conceptual-model synthesis

**Audit date:** 2026-08-08

**Evidence checked through:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369` (commit time 2026-08-07T12:38:38+02:00)

**Inspection checkout:** `e6fa1e8008dc967258a257ab5f983f342a0622ca`; relative to the audit snapshot, `git diff --name-status b0892ea..e6fa1e8` lists only the audit charter and accepted leaf artifacts 10 and 14–19. No product source, test, workflow, schema, package, or pre-existing documentation byte used here differs from the pinned snapshot.

**Artifact status:** thematic synthesis required by the audit charter; findings are audit conclusions, not product changes

**Inputs:**

- [`16-documentation-information-architecture.md`](16-documentation-information-architecture.md) — documentation estate, authority, freshness, duplication, synchronization
- [`17-testing-ci-quality.md`](17-testing-ci-quality.md) — tests, CI, smoke ownership, release evidence
- [`18-operations-configuration-ux.md`](18-operations-configuration-ux.md) — install, configuration, secrets, observability, UX, operational readiness
- [`19-conceptual-model-and-vocabulary.md`](19-conceptual-model-and-vocabulary.md) — product model, object/role vocabulary, trust and plane boundaries
- [`README.md`](README.md) — normative audit charter and evidence contract

**Change boundary:** this artifact only; no production source, tests, workflows, schemas, packaging, or pre-existing documentation were modified

## 1. Executive abstract

**`[INFERENCE]`** WorksGood is best presented as a **local-first durable work-and-evidence system**: a task graph is the stable work center; attended TUI/chat surfaces let humans operate it; an optional service dispatches bounded workers; agency supplies work identities; immutable review and publication derive successful completion; and federation, inbound review, and remote execution extend the local instance. This adopts `CONCEPT-001`, `CONCEPT-002`, and `CONCEPT-007` while narrowing the root README's broader “work OS” positioning to an implementable model (`README.md:1-62`; `src/graph.rs:379-529,689-1035`; `src/lifecycle.rs:66-86,181-213`; `src/commands/completion_done.rs:32-132`). Confidence: high.

**`[FACT]`** WorksGood has more product substance than its fragmented documentation suggests. Current source encodes eleven task statuses, separate task/generation/attempt/process/completion objects, a three-role contract, handler-first model routing, separate author and compute-provider trust assertions, capability/lease/review gates, conservative cleanup, and authenticated service state. Release, formal, Pi, installer, and library-test controls are also substantial. These positive controls are not merely plans: their current enforcement or selection sites include `src/graph.rs:379-529`, `src/lifecycle.rs:66-213`, `src/text/agent_guide.md:44-68,221-292`, `src/dispatch/handler_for_model.rs:20-60`, `src/trust.rs:29-53,79-125`, `src/commands/service/mod.rs:4383-4795`, `.github/workflows/ci.yml:68-201`, and `.github/workflows/release.yml:61-181,450-688`.

**`[INFERENCE]`** The product-quality problem is not missing prose, tests, or commands. It is **broken authority composition**. More than 550 Markdown files under `docs/`, hand-maintained help/reference, old and new lifecycle paths, 177 Cargo integration targets, 324 smoke entries, source/release package descriptions, and several operator diagnostics coexist without one machine-checkable map saying which claim is current, public, executable, selected in CI, or superseded. This synthesis adopts `DOC-001..010`, `TEST-001..011`, `OPS-001..014`, and `CONCEPT-001..010` only with the qualifications below; it does not treat file/test presence as proof.

**`[VERIFIED; DEPENDENCY EVIDENCE]`** The four inputs report direct, reproducible failures against snapshot-equivalent or pinned builds:

1. `OPS-001`: worker IPC array responses fail serialization after a message read may already advance state; the daemon emitted `can only flatten structs and maps (got a sequence)` after `wg msg read` (`18-operations-configuration-ux.md:13-32,196-205,480-514`; enforcement sites `src/commands/service/ipc.rs:251-274,716-758`).
2. `OPS-002/003`: `wg config set` erased comments, accepted an ineffective unknown key, and `config lint` called the file clean (`18-operations-configuration-ux.md:23-32,206-221,480-514`; implementation `src/commands/config_cmd.rs:3027-3102,3476-3676`).
3. `TEST-001/002`: all six `integration_smoke_gate` cases failed; after sanitizing ambient worker-control variables, they still expected a retired completion path. Current `wg done` dispatch rejects smoke flags and calls `completion_done::run`, which has no manifest invocation (`17-testing-ci-quality.md:15-29,124-141,279-297`; `src/main.rs:1261-1275`; `src/commands/completion_done.rs:32-104`). Normal CI does not select that target and names only eight of 177 integration targets (`.github/workflows/ci.yml:68-80,113-162`).
4. `CONCEPT-DRIFT-001..003`: a pinned build kept a successor unready after its predecessor failed, rejected advertised `wg done --converged`, and rejected bare `wg done` without a completion candidate (`19-conceptual-model-and-vocabulary.md:452-518`; source `src/graph.rs:379-529`, `src/cli.rs:527-548`, `src/main.rs:1261-1275`).

These are bounded verified behaviors from the named leaf audits, not reruns by this synthesis. They agree with direct source spot-checks at the pinned product bytes.

**`[FACT]`** The most important positive documentation synchronization control is the agent-guide chain: `src/text/agent_guide.md` is compiled with `include_str!`; tests assert the three-role and completion contract plus byte parity between `AGENTS.md` and `CLAUDE.md`; the files are byte-identical in this checkout (`src/commands/agent_guide.rs:3-15,132-185`; synthesis command in section 7). The Pi source-to-embedded gate is the strongest package analogue: CI builds/tests the TypeScript package, re-embeds it, and rejects a diff (`.github/workflows/ci.yml:174-201`). These controls demonstrate the maintainable pattern: **one source, explicit derivatives, regenerate/compare, and executable parity tests**.

**`[CONTRADICTION]`** Product and operator entry points do not currently tell one literal story. The root README says bare `worksgood` verifies Pi and ensures the plugin and calls Pi the sole model plane (`README.md:93-165`). The executable contract says an existing graph opens the setup-neutral TUI without inspecting Pi, plugins, profiles, config, or services (`src/bin/worksgood.rs:6-16,124-144`; `src/concierge.rs:1620-1648`). Setup accepts Pi as its supported route (`src/commands/setup.rs:72-150`), yet current handler code retains multiple model execution kinds (`src/dispatch/plan.rs:57-109`). “Sole” may mean the recommended attended plane, not total implementation capability; that scope remains a product decision, not something this audit resolves by choosing newer prose.

**`[INFERENCE]`** Operational readiness is mixed. Installer collision/receipt controls, profile fail-closed behavior, secret redaction, authenticated service status, and conservative cleanup are strong. But operators can currently lose worker replies, lose config comments, receive route-inappropriate doctor failures, see false “daily” spend and process-local “global” metrics, or believe completion was smoke-gated when it was not. These defects affect the control plane that is supposed to make AI work legible. Confidence: high; severity is driven by `OPS-R001..007` and `TEST-RISK-001..003`, not by documentation volume alone.

**`[RECOMMENDATION]`** The first release decision is not “rewrite the docs.” It is a three-part P0 integrity program:

1. repair stateful worker IPC response delivery (`OPS-REC-001`);
2. reconnect completion, owned smoke evidence, and CI-selected current integration tests (`TEST-REC-001/002`);
3. make generic config editing lossless and schema-aware (`OPS-REC-002`).

In parallel, establish a single product-contract manifest that connects vocabulary, supported journeys, CLI visibility, source authority, generated docs, evidence class, CI lane, release artifact, and supersession. Then regenerate/narrow public narratives from that contract. Rewriting first would produce another synchronized-looking but unenforced layer.

## 2. Scope and map

### 2.1 Synthesis method and disposition of inputs

**Local abstract.** This synthesis treats leaf findings as reviewed leads with provenance, not as a voting mechanism. Material claims were traced back to current source/configuration; leaf-executed behavior is explicitly attributed; contradictions remain open where implementation does not answer product intent.

| Input | Disposition in this synthesis | Material IDs retained | Important qualification |
|---|---|---|---|
| `16-documentation-information-architecture.md` | **Adopted** for estate shape, missing authority metadata, stale/copy-generation edges, root clutter, command-reference and link gaps | `DOC-001..010`, `DOC-DRIFT-001..010` | Counts changed as audit artifacts landed; public/internal command policy remains undecided; HTML equivalence was not semantically diffed |
| `17-testing-ci-quality.md` | **Adopted** for smoke-path disconnect, CI target selection, release-qualification gap and positive formal/Pi controls | `TEST-001..011`, `TEST-DRIFT-001..007` | Full suites and release workflows were not run; “169 omitted targets” is target-name selection, not 169 known failing products |
| `18-operations-configuration-ux.md` | **Adopted** for reproduced IPC/config defects, journey/precedence maps, diagnostics/accounting gaps, and safe defaults | `OPS-001..014`, `OPS-DRIFT-001..010` | Live IPC evidence is point-in-time but corroborates deterministic source; Windows/macOS behavior remains largely unqualified |
| `19-conceptual-model-and-vocabulary.md` | **Adopted** as a glossary candidate and object/plane map | `CONCEPT-001..010`, `CONCEPT-DRIFT-001..012`, `CONCEPT-AMB-001..018` | Candidate terms are not ratified product decisions; cycle support and “Pi sole plane” scope remain unresolved |
| Audit charter | **Normative method only** | `CHARTER-REC-001..007` | The charter performed static inventory and is not product behavior evidence |

**`[FACT]`** Direct spot-checks covered `README.md`, `docs/KEY_DOCS.md`, `docs/manual/README.md`, `scripts/sync-docs.sh`, `tests/smoke/{README.md,manifest.toml}`, `Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/ci.yml`, `src/{graph,lifecycle,trust,metrics}.rs`, `src/service/registry.rs`, `src/text/agent_guide.md`, `src/commands/{agent_guide,completion_done,config_cmd,doctor,spend}.rs`, `src/commands/service/ipc.rs`, `src/dispatch/handler_for_model.rs`, `src/bin/worksgood.rs`, and `src/concierge.rs`. This synthesis did not rely on `AGENTS.md` as sole evidence.

**`[UNCERTAINTY]`** Cross-domain implementation audits 10–15 may refine product facts outside this synthesis's four-input scope. This artifact deliberately does not generalize federation, review, execution, or agency security behavior beyond the vocabulary and operator boundaries directly needed here.

**Trace:** evidence section 7.2; risks `PRODUCT-RISK-001..003`; recommendations `PRODUCT-REC-001..004`.

### 2.2 Product and authority map

**Local abstract.** WorksGood's durable center is coherent, but its public explanation currently mixes work state, control processes, model execution, identity, and evidence maturity. The map below aligns product nouns with their current authority.

```text
HUMAN ENTRY
  worksgood (attended thin launcher) / wg tui / chat agent
                         │
                         ▼
LOCAL WG INSTANCE (.wg plus project/worktree)
  durable work graph: Task ─after→ Task
    status + dependencies + artifacts + usage
             │
             ├── lifecycle authority: generation → attempt → fence
             │       └── runtime worker process / service registry entry
             │
             └── completion valve:
                   immutable candidate → FLIP + eval reviews
                   → contract publication → derived Task Done

OPTIONAL SIDECARS / OVERLAYS
  agency identity       model route/config      service daemon
  role + tradeoff       handler + native model  dispatcher + IPC + registries

FEDERATED POLICY BOUNDARY
  wgid attribution → local trust assertion → capability → lease
  → inbound review → consumption; remote result → completion evidence
```

**`[FACT]`** `Task` is the durable work object (`src/graph.rs:689-1035`); `LifecycleProjection` and `AttemptRef` separately model generation, attempt, actor and fence (`src/lifecycle.rs:66-86,181-213`); runtime `AgentEntry` is a PID/task/executor/heartbeat record (`src/service/registry.rs:37-90`); `completion_done::run` resolves exact immutable evidence and publication before committing `Done` (`src/commands/completion_done.rs:32-132`). The graph-node enum is narrower than the full `.wg` instance (`src/graph.rs:2577-2591`).

**`[INFERENCE]`** The product should introduce those objects in that order: **task → dependency → claim/attempt → evidence/completion**, then service/agency/model/federation overlays. Current narratives often begin with agents, theory, Pi, or subsystem waves and force readers to discover the durable hierarchy later.

**`[DOC-CLAIM]`** The root README already provides the right slogan—“Agents can come and go. The graph remains”—and says WG centers answerable work (`README.md:1-62`). Its storage section, however, can be read too narrowly: current durable authority includes lifecycle, completion, registry, functions, agency and federation sidecars in addition to `graph.jsonl` (`README.md:188-199`; `19-conceptual-model-and-vocabulary.md`, `CONCEPT-DRIFT-011`).

**Trace:** `CONCEPT-001/002/007`; risks `CONCEPT-RISK-001..007`; recommendations `PRODUCT-REC-006/007`.

### 2.3 Current operator journey: claimed, reachable, verified, and missing

**Local abstract.** There is no single authoritative operator path. Strong individual commands exist, but install, onboarding, automation, diagnosis, accounting, publication, and recovery must be assembled from different guides and sometimes conflicting help/tests.

| Stage | Current reachable authority | What is well controlled | Contradiction/gap | Evidence status |
|---|---|---|---|---|
| 1. Install | native archive scripts or `cargo install --path . --locked` | checksums, optional attestations, collision/symlink refusal, receipt-bound uninstall | source install exposes four Cargo binaries; native release/docs expose three; bundle replacement is not set-atomic | `[FACT]` `Cargo.toml:20-41`; installer/release source; synthetic installer passed in audit 17 |
| 2. Open existing/new graph | bare `worksgood` → `run_bare`; `wg init` + `wg tui` for expert graph-only | existing graph is setup-neutral; new graph uses minimal route-free bootstrap | README/install/concierge prose says bare entry verifies Pi/plugin; runtime says it does not on existing graphs | `[FACT]` `src/bin/worksgood.rs:6-16,124-144`; `src/concierge.rs:1620-1648`; `DOC/OPS-DRIFT-001` |
| 3. Enable unattended automation | `worksgood setup --model pi:...` or expert `wg setup --route pi ...` | explicit plan, dry-run, no fallback, project profile fingerprint, separate attended model ownership | older docs/smokes retain Claude/provider routes; code still has other handlers, so “Pi sole plane” scope is unresolved | `[FACT]` `src/commands/setup.rs:72-150`; `src/dispatch/plan.rs:57-109`; direct retired route failed in audit 18 |
| 4. Configure/profile/secrets | `wg config`, project profile selection, `wg secret` | project profile fails closed on drift; redaction/plaintext opt-in; endpoint source visibility | generic setter erases comments, accepts ignored keys; keyring may resolve to unencrypted file; Telegram uses separate token plane | `[VERIFIED; DEPENDENCY]` `OPS-002/003`; `[FACT]` `src/secret.rs`, `src/notify/config.rs` |
| 5. Start and diagnose service | `wg service start/status`; `wg status`, `ready`, `agents`, `check`, `show`, `trace` | authenticated identity; rich status; preserved logs; good cleanup defaults | `doctor` hard-fails missing Claude on Pi-only path; no one route-aware readiness command; worker array IPC can lose the response | `[VERIFIED; DEPENDENCY]` `OPS-001`; `[FACT]` `OPS-005/012` |
| 6. Complete work | immutable candidate + exact FLIP/eval + publication-derived `wg done` | current completion valve is materially stronger than process-exit success | CLI help advertises legacy flags; documented smoke gate is not invoked on authoritative path | `[VERIFIED; DEPENDENCY]` `TEST-001`, `CONCEPT-DRIFT-002/003` |
| 7. Observe cost/time/quality | `wg show`, `trace`, `stats`, `spend`, `metrics`; CI and smoke inventory | task usage persists; status/trace rich; formal/Pi CI lanes are strong | spend dates all usage “today”; metrics are process-local; CI omits most integration targets; smoke presence is not gate reachability | `[FACT]` `src/commands/spend.rs:27-67`; `src/metrics.rs:8-26`; `TEST-002/006/007` |
| 8. Publish/share/recover | completion land, HTML/publish, disk/worktree cleanup, upgrade/install rerun | land preserves reviewed truth; cleanup defaults dry-run/dirty-safe | dirty attached root blocks unrelated publication; remote HTML includes all tasks by default; general ops runbook is fragmented; upgrade/package policy differs | `[VERIFIED; DEPENDENCY]` `OPS-006`; `[FACT]` `OPS-008..010,012` |

**`[INFERENCE]`** The minimum supported operator story should be four explicitly tested journeys, not one universal quickstart:

1. **attended existing graph** — setup-neutral TUI, executor chosen only on user action;
2. **new graph without automation** — route-free bootstrap, no credentials/service;
3. **explicit unattended automation** — exact Pi route, project profile, plugin/readiness, authenticated service;
4. **day-2 operator** — route-aware doctor, config/secret/profile diagnosis, service/log/task/disk recovery, completion evidence, upgrade/rollback.

Each step should declare mutations, credentials, network/service effects, binary authority, supported platforms, and the last human-flow test. `DOC-REC-002`, `OPS-REC-003/004/010`, and `CHARTER-REC-005` converge on this model.

**`[UNCERTAINTY]`** The thin-launcher human-flow smoke and many TUI/PTY scenarios were inspected by dependencies but not run in normal CI or by this synthesis. The journey table distinguishes reachable source from continuously verified experience.

**Trace:** risks `PRODUCT-RISK-002/004/006`; recommendations `PRODUCT-REC-003/005/009`.

### 2.4 Vocabulary alignment across product, docs, CLI, and operations

**Local abstract.** The types contain most needed distinctions. The maintainable solution is to expose them consistently, not invent a wholly new ontology.

| Area | Canonical candidate | Avoid / qualify | Current evidence and disposition |
|---|---|---|---|
| Product | **WorksGood is a local-first durable work-and-evidence system**; “work OS” is positioning | do not equate product with daemon, model client, or agency | Adopt `CONCEPT-001` with product-owner ratification required |
| Instance/graph | **WG instance** = project-scoped `.wg` state plus associated project/worktree; **work graph** = node/dependency relation | graph is not every sidecar/process | Adopt `CONCEPT-DRIFT-011` correction |
| Execution hierarchy | task → generation → attempt → worker process; completion candidate/reviews/publication → task `Done` | “run,” “complete,” or “agent done” without layer | Adopt `CONCEPT-002`, lifecycle sequence in 19 §3.3 |
| Roles | **dispatcher** (scheduling role), **service daemon** (host process), **chat agent**, **worker agent** | coordinator/orchestrator as current role nouns | Adopt bundled contract `src/text/agent_guide.md:44-68`; legacy aliases remain |
| Agency/runtime/federation identity | **agency agent**, **runtime worker/process**, **federated principal (`wgid:`)** | “one unified Agent identity” without namespace | Narrow accepted ADR; retain three legitimate namespaces (`CONCEPT-004`) |
| Model path | **model route/spec**, **handler**, **model provider**, **endpoint** | executor as preferred public synonym; provider without type | Adopt handler-first grammar, but document internal execution-kind umbrella |
| Remote compute | **compute provider**, capability, lease | OpenRouter/Anthropic “provider” and remote box as same object | Adopt `CONCEPT-AMB-003/004` distinction |
| Trust/security | **shared trust scale; separate local author/provider assertions** | “one trust dial” as one transitive reputation; signature as trust | Adopt/narrow `CONCEPT-006/007`; `src/trust.rs:29-53,79-125` |
| Review | **inbound review** vs **completion review** | unqualified review | Adopt `CONCEPT-AMB-007`; gates protect different edges |
| Functions/history | **trace function**, **function application**, **replay snapshot**, **spawn launch ID** | standalone “run” | Adopt `CONCEPT-009` |
| Publish | **publish task**, **publish identity**, **publish completion**, **push Git** | unqualified publish at cross-plane boundaries | Adopt `CONCEPT-AMB-013` |
| Maturity | type exists / CLI reachable / seam wired / deterministic fallback / live-model path / continuously gated / production-validated / deferred | spark/wave/shipped as a single maturity value | Adopt `CONCEPT-REC-016` |

**`[FACT]`** `Status` has eleven variants and `is_dep_satisfied()` accepts only `Done` (`src/graph.rs:379-529`). `AttemptDisposition` is separate (`src/lifecycle.rs:66-73`), as is runtime `AgentStatus` (`src/service/registry.rs:37-58`). Documentation that collapses these types creates behavioral, not cosmetic, errors.

**`[FACT]`** Handler vocabulary remains internally transitional. `handler_for_model` documents handler-first routes and deprecated provider-first forms (`src/dispatch/handler_for_model.rs:20-60`), but the internal enum remains `ExecutorKind`, and even the same module's introductory examples include a bare `openrouter:` form (`src/dispatch/handler_for_model.rs:1-18`). Generated reference must therefore derive from a reviewed public contract, not copy source comments wholesale.

**`[CONTRADICTION]`** “One trust dial” is useful only if read as one ordinal enum. The enforcement source itself distinguishes the author assertion from the compute-provider assertion and folds provider trust only in the stricter direction (`src/trust.rs:29-53,79-125`). Public wording should say **one scale, multiple subject/purpose assertions**.

**Trace:** `CONCEPT-001..010`; risks `CONCEPT-RISK-001..009`; recommendations `PRODUCT-REC-006/007/010`.

### 2.5 Target information architecture

**Local abstract.** The target architecture must separate audience routing from complete inventory, normative contract from generated reference, operator procedure from historical evidence, and current architecture from research. Moving files is a later migration; first establish metadata, redirects, and generation edges.

```text
README.md                              # product promise + four supported journeys
AGENTS.md == CLAUDE.md                 # project-only layer 2; parity tested

docs/
  README.md                            # curated audience router, generated links/status badges
  manifest.toml                        # complete classified estate inventory
  product-contract.toml                # terms, public surfaces, journeys, claim/evidence IDs

  getting-started/
    attended-existing.md
    new-graph-only.md
    unattended-automation.md
    install-and-upgrade.md

  concepts/
    README.md
    system-model.typ                   # one declared conceptual source graph
    glossary.toml                      # generated from/checked against code schemas
    generated/                         # Markdown/PDF diagrams and tables

  reference/
    cli.md                             # generated public signatures + keyed authored examples
    config.md                          # generated known keys/precedence + extension policy
    storage-and-schemas.md
    compatibility.md                   # generated constants and package membership

  operations/
    README.md
    runbooks/
      day-2.md
      recovery.md
      release.md
      federation.md
    troubleshooting/
    platform-support.md
    security-and-secrets.md

  architecture/
    README.md                          # status/decision index
    adr/
    designs/

  contributor/
    development.md
    testing.md
    documentation.md
    worktrees-and-publication.md

  evidence/
    README.md                          # immutable evidence semantics
    audits/<date-or-release>/
    reports/<topic>/
    incidents/
    test-results/

  research/
    README.md
    studies/
    plans/

  archive/
    README.md                          # policy, provenance, redirects, supersession
    <year>/<topic>/
```

**`[RECOMMENDATION]`** `docs/manifest.toml` is inventory; `docs/README.md` and `KEY_DOCS`-style pages are curated views generated from it. Every artifact records at least `kind`, `audience`, `authority`, `status`, `owner`, `valid_as_of`, `source`, `generated_outputs`, `supersedes`, `superseded_by`, `claim_ids`, and `evidence_ids`. This adopts `DOC-REC-001` and prevents a curated list from silently impersonating an estate index.

**`[RECOMMENDATION]`** Keep immutable audits/reports unchanged. Add bundle indexes and supersession edges rather than editing historical findings to look current. This preserves `DOC-009`'s positive provenance while solving its navigation failure.

**`[RECOMMENDATION]`** Root cleanup follows an allowlist only after a path-by-path mapping and link/redirect review. Age alone is not an archival criterion. This adopts `DOC-REC-008..010` without moving anything in the audit.

**Trace:** `DOC-001/003/006/009`; risks `DOC-RISK-002/005/007`; recommendations `PRODUCT-REC-004/008`.

### 2.6 Evidence, authority, indexing, and testing model

**Local abstract.** The repository needs one contract graph connecting claims to implementation, executable evidence, generated docs, and release selection. A documentation manifest alone cannot detect a disconnected smoke gate; a test inventory alone cannot say which narrative it supports.

#### Authority by question

| Question | Primary authority | Required corroboration | Generated/public output |
|---|---|---|---|
| What term/object exists? | versioned Rust/Serde type or approved schema | round-trip/schema test; glossary mapping | generated glossary/reference table |
| What command/flag is supported? | public command contract **plus reachable dispatch** | real-binary positive and negative test | generated CLI reference/help snapshot |
| What does a journey do? | executed release-binary human flow | source mutation/effect map; platform lane | getting-started/runbook step with last-tested receipt |
| What invariant should hold? | accepted ADR/contract | enforcement site + negative/adversarial test | architecture page and claim record |
| What passed continuously? | CI/release receipt for exact commit/artifact | target/scenario selection manifest and skips | evidence dashboard/status badge |
| What happened historically? | immutable report at pinned revision | closure/supersession record | evidence bundle index, never rewritten verdict |

**`[INFERENCE]`** Generated help is necessary but not sufficient. `src/cli.rs:527-548` exposes `--converged`, `--full-smoke`, and `--skip-smoke`, while `src/main.rs:1261-1275` rejects them. Therefore command authority must join **parser + dispatch + behavior test**, not generate docs from Clap alone.

#### Proposed product-contract record

```toml
[[claim]]
id = "completion.owned-smoke-before-done"
statement = "Required owned scenarios pass before task Done is committed"
status = "broken"                 # current | partial | broken | deferred | historical
public = true
terms = ["task Done", "completion review", "required scenario"]
source = ["src/main.rs#Commands::Done", "src/commands/completion_done.rs#run"]
behavior_tests = ["tests/integration_smoke_gate.rs"]
ci_lane = "integration-required"
docs = ["tests/smoke/README.md", "src/text/agent_guide.md"]
release_gate = true
last_verified_revision = "..."
last_result = "fail"
owner = "completion/smoke"
supersedes = []
```

**`[RECOMMENDATION]`** The contract generator should produce:

- the public/internal/migration/hidden CLI inventory;
- glossary and enum tables;
- supported-journey matrices;
- docs estate/index views;
- CI target/scenario allowlists and orphan checks;
- release binary/package manifests;
- compatibility/version reference;
- an evidence dashboard that distinguishes pass, fail, skip, inspected-not-run, and not-selected.

**`[RECOMMENDATION]`** Required validation classes should be explicit: `unit-pure`, `integration-hermetic`, `human-flow`, `platform`, `live-advisory`, `static-contract`, `formal-bounded`, and `release-artifact`. A skip is acceptable only in a class declared advisory or unavailable for that lane; zero required assertions is not success. This adopts `TEST-REC-003/004/007` and preserves the formal model's explicit OS/fs/network exclusions (`formal/README.md:3-5,133-138`).

**`[FACT]`** Two existing controls are templates for this model:

- agent contract: one bundled source plus parity/unit/integration/smoke checks (`src/commands/agent_guide.rs:3-15,132-185`; `tests/integration_init.rs:51-64`);
- Pi embed: source build/selftests/Vitest followed by regenerate-and-diff (`.github/workflows/ci.yml:174-201`).

**`[UNCERTAINTY]`** A broad product-contract registry adds governance cost and can itself drift. Acceptance must include an orphan/delta gate: every new public command, enum variant, integration target, smoke scenario, release binary, generated output, or current doc must either join the contract or carry an explicit, reviewed exclusion.

**Trace:** `DOC-004/005/008/010`, `TEST-001..006/011`, `CONCEPT-010`; risks `PRODUCT-RISK-001/003/005`; recommendations `PRODUCT-REC-001/002/004/007`.

## 3. Findings

### `PRODUCT-001` — WorksGood has a coherent durable center but no coherent public contract

- **Label/state:** **`[INFERENCE]`**, shipped core / partial presentation.
- **Severity/likelihood/confidence:** S2 Medium; observed reader ambiguity; high confidence.
- **Adopts:** `CONCEPT-001/002/007`; `DOC-002/003`; `OPS-004`.
- **Evidence:** durable task, lifecycle, runtime and completion objects are separate (`src/graph.rs:379-529,689-1035`; `src/lifecycle.rs:66-213`; `src/service/registry.rs:37-90`; `src/commands/completion_done.rs:32-132`). Primary narratives disagree on first-run/model behavior (`README.md:93-165`; `src/bin/worksgood.rs:6-16,124-144`; `src/commands/setup.rs:72-150`).
- **Counterevidence:** the root work-centered slogan and bundled three-role contract are strong starting points.
- **Conclusion:** ratify the product sentence and plane/object diagrams, then generate narrower views for audiences.

### `PRODUCT-002` — authority multiplication is the systemic documentation defect

- **Label/state:** **`[FACT]` plus `[INFERENCE]`**, current.
- **Severity/likelihood/confidence:** S1 High when safety/operator claims drift; likely; high confidence.
- **Adopts:** `DOC-001/003/004/006/009/010`; `CHARTER-RISK-001/003`.
- **Evidence:** the leaf inventory found 555 docs Markdown files, 56 root Markdown files, only 151 docs files with a near-head status marker, and a dated-aging `KEY_DOCS` mentioning 290 paths (`16-documentation-information-architecture.md:17-75,79-175`). Direct evidence shows `KEY_DOCS` calls `COMMANDS.md` complete (`docs/KEY_DOCS.md:1-16`); manual source declarations conflict (`docs/manual/README.md:30-42`; `scripts/sync-docs.sh:1-8,102-118`).
- **Inference:** path, recency, and repetition currently substitute for authority. This is dangerous because stale setup/safety text can cause real actions.
- **Recommendation:** contract manifest, generated indexes/reference, supersession graph, root allowlist.

### `PRODUCT-003` — executable-evidence inventory materially overstates activated assurance

- **Label/state:** **`[FACT]` and `[VERIFIED; DEPENDENCY EVIDENCE]`**, current.
- **Severity/likelihood/confidence:** S1 High; observed; high confidence.
- **Adopts:** `TEST-001..005/011`.
- **Evidence:** current completion dispatch contains no smoke call (`src/main.rs:1261-1275`; `src/commands/completion_done.rs:32-104`); CI runs the library harness and selected formal/service targets but no broad `cargo test --tests` or manifest-smoke job (`.github/workflows/ci.yml:68-162`). Audit 17 executed six smoke-gate integration cases and all failed (`17-testing-ci-quality.md:279-297`).
- **Counterevidence:** 3,149 library tests are selected, formal conformance is connected, installer static/synthetic checks run, and Pi has a strong staleness gate. The defect is activation/coverage authority, not absence of verification work.
- **Conclusion:** a test file or smoke entry must never be presented as release evidence without selection and result receipts.

### `PRODUCT-004` — the operator control plane currently violates its own legibility promise

- **Label/state:** **`[VERIFIED; DEPENDENCY EVIDENCE]` plus `[INFERENCE]`**, current.
- **Severity/likelihood/confidence:** S1 High; observed/deterministic on reproduced paths; high confidence.
- **Adopts:** `OPS-001..007`.
- **Evidence:** stateful array-valued worker replies cannot serialize through flattened `IpcResponse.data` (`src/commands/service/ipc.rs:251-274,716-758`); generic config editing uses semantic TOML reserialization despite a preservation claim (`src/commands/config_cmd.rs:3027-3102`); doctor unconditionally errors on Claude before optional Pi (`src/commands/doctor.rs:166-226,267-412`); spend groups all records under invocation day (`src/commands/spend.rs:27-67`); metrics are process-local statics (`src/metrics.rs:8-26`).
- **Counterevidence:** service status, profile drift refusal, secret redaction, disk cleanup and worktree GC are conservative positive controls (`OPS-012`).
- **Inference:** a system selling durable legibility must prioritize correctness of messages, configuration, readiness, and accounting over additional operator surface area.

### `PRODUCT-005` — vocabulary drift maps directly to wrong operational decisions

- **Label/state:** **`[FACT]` plus `[INFERENCE]`**, current.
- **Severity/likelihood/confidence:** S2 Medium; likely; high confidence.
- **Adopts:** `CONCEPT-003..009`, `CONCEPT-DRIFT-001..012`.
- **Evidence:** manual status/dependency semantics disagree with `Status::is_dep_satisfied()`; CLI help exposes rejected completion flags; agent/provider/review/run/trust have multiple namespaces (`src/graph.rs:379-529`; `src/cli.rs:527-548`; `src/main.rs:1261-1275`; `src/trust.rs:29-53`; `src/dispatch/handler_for_model.rs:1-60`). Pinned fixtures verified the dependency and completion discrepancies (`19-conceptual-model-and-vocabulary.md:452-518`).
- **Inference:** these are not editorial niceties. They affect whether work dispatches, whether a caller believes authority exists, and which configuration/credential plane it edits.
- **Conclusion:** qualify nouns at boundaries and generate tables from schemas/dispatch contracts.

### `PRODUCT-006` — source, archive, installer, and platform support do not share one product manifest

- **Label/state:** **`[FACT]`**, current / product decision open.
- **Severity/likelihood/confidence:** S2 Medium; observed; high confidence.
- **Adopts:** `TEST-DRIFT-007`; `OPS-009/010/014`.
- **Evidence:** Cargo declares four binaries, including `casa-adapter` (`Cargo.toml:20-41`), while native release/install surfaces enumerate three (`.github/workflows/release.yml:470-523,646-667`; installer docs/scripts cited in audit 18). Rust runtime tests are Ubuntu-centric; Windows gets synthetic installer coverage and macOS gets release construction rather than regular runtime qualification (`.github/workflows/ci.yml:1-201`).
- **Uncertainty:** Casa may intentionally be source-only. Successful cross-build may work in practice. Neither point supplies the missing public support policy.
- **Conclusion:** generate Cargo/release/installer/docs membership and platform qualification from one approved manifest.

### `PRODUCT-007` — synchronization controls work where source and derivative are explicit

- **Label/state:** **`[FACT]`**, shipped positive control.
- **Severity/confidence:** S4 Informational; high confidence.
- **Adopts:** `DOC-008`, `TEST-006/007`, `OPS-012`.
- **Evidence:** agent-guide include/parity tests (`src/commands/agent_guide.rs:3-15,132-185`); Pi rebuild/embed diff (`.github/workflows/ci.yml:174-201`); formal model explicitly bounded and connected to Rust conformance (`.github/workflows/ci.yml:82-125`; `formal/README.md:3-5,86-138`); service/cleanup source defaults preserve state.
- **Inference:** the repository does not need a novel synchronization philosophy. It needs to generalize its successful **single source → declared derivative → executable parity/diff** pattern.

### `PRODUCT-008` — report immutability is good; missing supersession navigation is not

- **Label/state:** **`[FACT]` plus `[INFERENCE]`**, partial.
- **Severity/likelihood/confidence:** S2 Medium; likely reader reversal; high confidence.
- **Adopts:** `DOC-009`, `DOC-DRIFT-007/008`.
- **Evidence:** a historical federation report says the handshake was unwired, while current source implements it and a follow-up changes the verdict (`docs/prod-audit/audit-fed.md:34-40`; `src/identity/transport.rs:583-607`; `docs/prod-audit/01-production-readiness-followup.md:1-34`). The bundle has no root index.
- **Inference:** rewriting old reports would destroy evidence. Add status/supersession edges and bundle entry points instead.

## 4. Contradictions and drift

### 4.1 Cross-input contradiction register

| ID | Claims that cannot all be read literally | Current authority / status | Severity / confidence |
|---|---|---|---|
| `PRODUCT-DRIFT-001` | README: bare `worksgood` verifies Pi/plugin (`README.md:93-113`); runtime: existing graph opens TUI without inspecting them (`src/bin/worksgood.rs:6-16,124-144`; `src/concierge.rs:1620-1648`) | runtime controls current behavior; prose scope open | S1 operator journey / high |
| `PRODUCT-DRIFT-002` | setup supports Pi route (`src/commands/setup.rs:72-150`); older setup docs/smokes require multiple retired routes; handler implementation still supports multiple handlers | current setup source controls setup; total product-plane scope unresolved | S1 / high for setup, medium for “sole plane” |
| `PRODUCT-DRIFT-003` | smoke README/manifest/agent contract say owned smoke runs before `Done`; authoritative completion path does not invoke it | current dispatch controls; contract broken | S1 / high |
| `PRODUCT-DRIFT-004` | permanent smoke-gate integration target claims environment-independent protection; all six cases failed and target is not selected in CI | executed leaf evidence controls | S1 / high |
| `PRODUCT-DRIFT-005` | config source/docs promise comments/unknown sections preserved; implementation reserializes `toml::Value`, and reproduction erased comments | executed behavior controls | S1 / high |
| `PRODUCT-DRIFT-006` | setup says Pi is supported route; doctor makes Claude absence an error and Pi absence informational | neither command is a universal readiness authority; route-aware doctor absent | S1 / high |
| `PRODUCT-DRIFT-007` | `wg spend --today`/daily wording; implementation assigns every record to current day | implementation/reproduction controls | S1 / high |
| `PRODUCT-DRIFT-008` | manual: eight statuses and terminal failure unblocks; source: eleven statuses and only `Done` satisfies ordinary dependency; pinned fixture agrees with source | source + E1 controls | S2 / high |
| `PRODUCT-DRIFT-009` | `wg done --help` advertises legacy cycle/smoke/bypass flags; dispatch rejects all; bare completion requires immutable candidate | dispatch + E1 controls | S2 / high |
| `PRODUCT-DRIFT-010` | manual README says unified Typst is authoritative; sync script generates from chapter Typst and can copy raw Typst into `.md` | source graph unresolved | S2 / high |
| `PRODUCT-DRIFT-011` | `KEY_DOCS`/README call indexes/reference complete; inventory and help comparison find large omissions | useful curated pages, not complete inventory/reference | S2 / high for inventory, medium for public commands |
| `PRODUCT-DRIFT-012` | Cargo/source install has four binaries; native package/docs have three | product packaging policy unknown | S2 / high facts, unresolved intent |
| `PRODUCT-DRIFT-013` | exact Rust toolchain claim; workflows explicitly request floating stable | local override resolved to pin; CI precedence/log unverified | S2 / medium |
| `PRODUCT-DRIFT-014` | “one trust dial”; enforcement has separate author/provider assertions on one scale | code controls; phrase must be narrowed | S2 / high |
| `PRODUCT-DRIFT-015` | accepted unified-Agent ADR; current agency, runtime, and federated identities remain distinct | apparent overstatement; qualify ADR outcome | S3 / high |
| `PRODUCT-DRIFT-016` | studies/designs remain draft/proposed/deterministic-only while implementations/CLI contain later slices | historical intent remains; applicability metadata stale | S3 / high, maturity specifics vary |

### 4.2 Resolved or narrowed apparent contradictions

**`[FACT]`** `AGENTS.md` and `CLAUDE.md` duplicate a large project guide intentionally. They are byte-identical, cross-reference the bundled universal guide, and source tests assert parity (`src/commands/agent_guide.rs:132-185`). This is a positive controlled derivative, not root clutter to deduplicate blindly.

**`[FACT]`** `TrustLevel::default()` being Provisional does not mean unknown federated authors are accepted as Provisional. `peer_trust_opinion` maps a present but unvouched peer to Unknown, and `resolve_author_trust` fails absence closed to Unknown (`src/graph.rs:2530-2540`; `src/trust.rs:79-125`). The stale comment/phrase remains a documentation issue.

**`[FACT]`** `Task.assigned` and `Task.agent` are not necessarily duplicate identity pointers. Current evidence distinguishes execution ownership from agency identity selection (`src/graph.rs:714,853`; `src/lifecycle.rs:75-86`). Naming and schema documentation remain inadequate.

**`[UNCERTAINTY]`** Cycle support is neither safely “supported” nor safely “absent.” Cycle metadata/service code remains, while `--converged` is rejected under publication-derived completion. No reviewed structural-cycle human flow was run by the conceptual audit. Product owner must classify cycles as supported, compatibility-only, or retired.

**`[UNCERTAINTY]`** “Pi is the sole model plane” may be intended as product recommendation for attended use while other handlers remain advanced/migration implementation. No sampled authority states that scope precisely. This synthesis does not replace it with “Pi is not the model plane”; it requests adjudication.

### 4.3 Systemic causes of drift

1. **Unbounded prose authority.** Current guides, manuals, reports, designs, comments, captured help, agent contracts, and website copies lack a common status/source/supersession model (`DOC-001/003/009`).
2. **Hand-maintained mirrors.** `COMMANDS.md`, status tables, config narratives, quickstart HTML, manual Markdown, compatibility values, and package lists are copied rather than generated from declared sources (`DOC-004/005/010`, `CONCEPT-010`).
3. **Transition without retirement.** Publication-derived completion landed while legacy `done` parser flags, historical smoke calls, integration targets, manual text, and agent guidance remained visible (`TEST-001`, `CONCEPT-DRIFT-002/003`).
4. **Inventory mistaken for activation.** Test/script presence is treated as evidence even when normal CI and authoritative dispatch never select it (`TEST-001/002/011`).
5. **Parser mistaken for supported behavior.** Generated help can advertise options that command dispatch rejects (`src/cli.rs:527-548`; `src/main.rs:1261-1275`).
6. **Fragmented ownership by surface.** Cargo, native releases, installers, docs, and CI encode binary/platform membership independently (`PRODUCT-006`).
7. **Point-in-time evidence without closure edges.** Immutable reports preserve valuable history but lack bundle-level routing to follow-ups (`DOC-009`).
8. **High-velocity subsystem vocabulary.** coordinator→dispatcher, motivation→tradeoff, provider-first→handler-first, actor→agent, and spark→shipped migrations leave schema, help, comments, and prose at different epochs.
9. **Environment-dependent verification without result classes.** Worker-control env leakage, missing credentials, exit-77 skips, GNU-timeout assumptions, platform gaps, and separate release workflows make “pass” context-sensitive (`TEST-004/008/009`).
10. **Documentation fixes are not tested as user journeys.** Link/assets, command discovery, pipe handling, setup side effects, route-aware diagnosis, and release archives lack one joined release-binary experience gate.

## 5. Risks and gaps

| Rank | ID | Severity | Likelihood | Risk / affected boundary | Existing control and residual uncertainty |
|---:|---|---:|---|---|---|
| 1 | `PRODUCT-RISK-001` | S1 | observed | Stateful worker coordination response is lost after possible inbox mutation | messages/logs persist, but worker has no filesystem fallback; repair required |
| 2 | `PRODUCT-RISK-002` | S1 | observed | Task reaches `Done` without advertised owned smoke gate | immutable review/publication is strong but protects a different invariant |
| 3 | `PRODUCT-RISK-003` | S1 | likely | Green CI/release misses stale or failing integration contracts | library/formal/Pi/install lanes are positive but incomplete |
| 4 | `PRODUCT-RISK-004` | S1 | likely | Generic config edit erases operator context or silently stores ineffective key | atomic write prevents torn file, not semantic/format damage |
| 5 | `PRODUCT-RISK-005` | S1 | possible | Stale setup/safety/authority text causes wrong human action | current source may fail closed, but users act before reading source |
| 6 | `PRODUCT-RISK-006` | S1 | likely if commands used | Doctor/accounting/metrics lead to false readiness or budget/capacity decisions | raw service/task state exists; rollups are wrong or fragmented |
| 7 | `PRODUCT-RISK-007` | S1 | user-dependent | Remote HTML default publishes non-public task information | transcripts off and warnings exist; task metadata still broad by default |
| 8 | `PRODUCT-RISK-008` | S2 | likely | Identity/provider/review/run ambiguity leads integrations to cross authority namespaces | strong types exist, but schemas/public labels are incomplete |
| 9 | `PRODUCT-RISK-009` | S2 | possible | Source/archive binary or partial-upgrade mismatch produces support/runtime skew | checksums/receipt/per-file rename help; no set transaction or public Casa policy |
| 10 | `PRODUCT-RISK-010` | S2 | likely | Search/path age substitutes for current authority, reversing historical conclusions | immutable evidence is preserved; no global supersession graph |
| 11 | `PRODUCT-RISK-011` | S2 | possible | Platform claims exceed Windows/macOS runtime qualification | release construction and Windows installer test are not end-to-end runtime evidence |
| 12 | `PRODUCT-RISK-012` | S2 | possible | A new contract manifest becomes another stale index | must have orphan/delta CI gate and generated views |

**`[UNCERTAINTY]`** Neither the inputs nor this synthesis executed full Cargo/smoke suites, real release publication, real signing/notarization, live model/provider flows, macOS/Windows runtime journeys, all TUI/browser paths, or destructive federation/provider operations. Test absence is a gap, not proof of failure. Conversely, file presence and successful compilation are not runtime or release evidence.

**`[UNCERTAINTY]`** Documentation inventory counts are snapshot-sensitive because audit files themselves add docs. The underlying structural findings—missing metadata, incomplete indexes, weak generation edges—do not depend on the exact post-audit total.

## 6. Ranked recommendations

### P0 — restore product-control integrity

1. **`PRODUCT-REC-001` — repair stateful worker IPC before further operator expansion.** Adopt `OPS-REC-001`: replace flattened arbitrary JSON with a named field or map-only flattening; make read delivery replayable/receipted; use stable request IDs; test empty/nonempty arrays and objects across real sockets. **Acceptance:** worker `MessageRead`, `MessagePoll`, `ArtifactList`, `Show`, and `Context` round-trip; injected response loss cannot silently consume the only delivery.
2. **`PRODUCT-REC-002` — reconnect completion to classified smoke evidence and current CI.** Adopt `TEST-REC-001/002`: run required owner-selected smoke before terminal mutation, persist exact policy/manifest/result digests, modernize `integration_smoke_gate`, and select it in CI. **Acceptance:** valid completion-v3 evidence plus an owned failing scenario cannot become `Done`; succeeding scenario can; every integration target is selected or explicitly quarantined.
3. **`PRODUCT-REC-003` — make generic config editing lossless and schema-aware.** Adopt `OPS-REC-002`: use `toml_edit` or tested line patching; reject typo paths or require an explicit extension namespace/`--raw`; split schema, migration, selection, secret and runtime lint. **Acceptance:** golden formatting/comment preservation, typo suggestion, correct remedy/exit codes, human/JSON parity.
4. **`PRODUCT-REC-004` — create the product-contract and docs-estate manifests before mass rewriting.** Join terms, public commands, journeys, source/dispatch, docs, tests, CI lane, release artifacts, status and supersession. **Acceptance:** every current doc, public command, enum variant, integration target, smoke scenario, generated output and shipped binary is classified or explicitly excluded; new unclassified surfaces fail CI.

### P1 — align what the product says, what operators do, and what evidence proves

5. **`PRODUCT-REC-005` — elect four supported operator journeys and test actual human flows.** Existing attended graph, new route-free graph, explicit unattended automation, and day-2 operations each get one mutation/effect contract and release-binary flow. Synchronize README, install/concierge, help and smoke expectations. **Acceptance:** each step lists filesystem/config/plugin/auth/service effects, platform, rollback and last passing receipt; no retired setup route appears as a current success scenario.
6. **`PRODUCT-REC-006` — ratify the product and lifecycle vocabulary.** Adopt the candidate in section 2.4 after owner review: task/generation/attempt/process/completion; dispatcher/service/chat/worker; agency/runtime/federated identity; handler/model-provider/compute-provider; inbound/completion review. **Acceptance:** generated help/manual tables match Rust enums and reachable dispatch; user-visible IDs carry namespaces; unqualified “run,” “provider,” “review,” “identity,” and “done” disappear at cross-plane boundaries.
7. **`PRODUCT-REC-007` — publish generated schemas/reference for the conceptual core.** Generate Task/status/lifecycle, agency, identity envelope, capability/provider, review verdict and completion tables from versioned schemas/Serde contracts; generate CLI only from public contract + reachable dispatch, not parser alone. **Acceptance:** round-trip tests and docs regeneration produce no diff; adding an enum variant or public flag requires a documentation/evidence disposition.
8. **`PRODUCT-REC-008` — implement the target IA and supersession graph without destroying evidence.** Generate inventory and audience routers from `docs/manifest.toml`; add bundle indexes for audits/reports/studies/incidents; approve root allowlist and path mapping; preserve immutable historical bytes. **Acceptance:** every report has applicability/supersession navigation; every move has redirects/link review; current user docs pass a policy-aware link/asset check.
9. **`PRODUCT-REC-009` — build one route-aware operator readiness and accounting path.** Adopt `OPS-REC-004/005/010`: `doctor --all` resolves effective route/profile/plugin/auth/secret/service/graph/disk/platform; spend uses completion date; metrics state their scope or persist/query daemon state; publish a general day-2 runbook. **Acceptance:** healthy Pi-only project without Claude exits 0; dated spend fixtures reconcile to task usage; cleanup metrics survive/query according to documented retention.
10. **`PRODUCT-REC-010` — ratify the trust/authority and maturity sentences.** Use: “a signature proves attribution; trust is a local purpose-specific assertion on a shared scale; a capability grants authority; a lease bounds execution; inbound review permits consumption; completion evidence authorizes task `Done`.” Replace spark/wave-as-status with a capability matrix. **Acceptance:** Fed/Review/Exec/Pilot introductions use the same sentence and identify wired, fallback, live, gated, validated and deferred layers separately.

### P2 — complete release, platform, discovery, and maintenance controls

11. **`PRODUCT-REC-011` — unify package/release/platform authority.** Declare Casa policy; generate Cargo/release/archive/installer/receipt/docs membership; qualify actual archives on each target; make publish depend on tested commit and signing policy; add set-transaction rollback. **Acceptance:** every shipped binary extracts and runs; source-only differences are allowlisted; injected failure restores prior bundle/receipt; platform support page links runtime evidence.
12. **`PRODUCT-REC-012` — classify smoke/test evidence and publish results.** Add manifest lint, required/advisory/static/formal/platform classes, skip budgets, ignored-test registry, target-delta check, sharded integration execution and scheduled protected lanes. **Acceptance:** required lanes cannot pass with zero assertions; skips are counted; stale/ignored/new targets cannot disappear silently.
13. **`PRODUCT-REC-013` — generalize proven derivative controls.** Preserve agent-guide parity and Pi embed regeneration; apply regenerate-and-diff to manual, website quickstart, command reference, compatibility constants and package manifest. **Acceptance:** clean checkout regeneration is diff-free and records source revision/generator version.
14. **`PRODUCT-REC-014` — improve safe discovery and publication defaults.** Add category/search help, EPIPE-clean behavior, public-only remote HTML default, included-visibility manifest, and explicit non-public opt-in. **Acceptance:** `wg --help-all | head -1` exits cleanly; novice flow finds setup/doctor/logs/secrets/cleanup; private task data is absent from default remote output.
15. **`PRODUCT-REC-015` — adjudicate remaining product decisions explicitly.** Decide scoped Pi exclusivity, cycles under immutable completion, public meaning of executor, Casa distribution, manual source graph, and public/internal command set. **Acceptance:** each decision has ADR/status, implementation owner, migration plan, generated docs, executable acceptance, and named superseded claims.

**`[RECOMMENDATION]`** Sequence matters: P0 repairs and the contract manifest precede broad wording changes; factual synchronization can proceed once authority is known; IA moves follow inventory/redirect review; contested product choices remain human decisions rather than audit edits.

## 7. Evidence appendix

### 7.1 Synthesis-local commands

**`[VERIFIED]`** The following commands ran on 2026-08-08 in `/home/bot/wg/.wg-worktrees/agent-13`, Linux, against inspection checkout `e6fa1e8008dc967258a257ab5f983f342a0622ca`; exit 0 unless stated. They verify repository shape and product-byte equivalence only, not behavior discussed above.

```bash
git rev-parse HEAD
git diff --name-status b0892ea7496fd2cc8f641417a3d8e33ca9add369..HEAD
find docs -type f | wc -l
find docs -type f -name '*.md' | wc -l
find . -maxdepth 1 -type f -name '*.md' | wc -l
rg -c '^\[\[scenario\]\]' tests/smoke/manifest.toml
find tests -maxdepth 1 -type f -name '*.rs' | wc -l
cmp -s AGENTS.md CLAUDE.md
```

Bounded result:

```text
inspection revision: e6fa1e8008dc967258a257ab5f983f342a0622ca
diff from audit snapshot: eight added audit artifacts only
current docs files: 611 (includes new audit artifacts)
current docs Markdown: 562 (includes new audit artifacts)
root Markdown: 56
smoke manifest entries: 324
top-level Rust test files: 176
AGENTS.md / CLAUDE.md byte parity: yes
```

**`[FACT]`** Required final validation commands appear in section 7.5 after the artifact is complete.

### 7.2 Primary repository evidence spot-checked

| Topic | Primary evidence | What it establishes |
|---|---|---|
| Product entry/journeys | `README.md:1-62,93-165,188-225`; `src/bin/worksgood.rs:6-16,124-151`; `src/concierge.rs:1620-1735` | work-centered claim; bare existing/new behavior; automation separation |
| Durable objects | `src/graph.rs:322-529,689-1035,2530-2591`; `src/lifecycle.rs:66-86,181-213`; `src/service/registry.rs:37-90` | task/status/dependency; attempt hierarchy; process registry |
| Completion | `src/cli.rs:527-548`; `src/main.rs:1261-1275`; `src/commands/completion_done.rs:32-132` | advertised legacy flags, dispatch rejection, exact review/publication path |
| Roles/agent docs | `src/text/agent_guide.md:44-68,215-292`; `src/commands/agent_guide.rs:3-15,132-185` | canonical role/completion text and parity controls |
| Model vocabulary | `src/dispatch/handler_for_model.rs:1-105`; `src/dispatch/plan.rs:57-109`; `src/commands/setup.rs:72-150` | handler grammar, internal execution kinds, supported setup route |
| Trust vocabulary | `src/trust.rs:1-53,79-125`; `src/graph.rs:2530-2540` | one scale, separate assertions, fail-closed resolution |
| IPC defect | `src/commands/service/ipc.rs:251-274,716-758`; `src/messages.rs:631-696` | flattened response and array-returning stateful operations |
| Config defect | `src/commands/config_cmd.rs:3027-3102,3476-3676`; `docs/config-precedence.md:13-21` | semantic rewrite, unknown-key/lint path, preservation claim |
| Diagnosis/accounting | `src/commands/doctor.rs:166-226,267-412`; `src/commands/spend.rs:27-67`; `src/metrics.rs:8-26,83-193` | Claude/Pi severity; current-day grouping; process-local counters |
| Verification selection | `.github/workflows/ci.yml:1-230`; `tests/smoke/README.md:1-103`; `tests/smoke/manifest.toml:1-17` | actual CI lanes versus stated smoke contract |
| Documentation authority | `docs/KEY_DOCS.md:1-44`; `docs/manual/README.md:1-42`; `scripts/sync-docs.sh:1-8,83-118` | curated index claim and conflicting generation graph |
| Package/toolchain | `Cargo.toml:1-54`; `rust-toolchain.toml:1-19`; `.github/workflows/ci.yml:18-21,54-56,110-112,134-136` | four binaries, MSRV/features, exact-local versus workflow stable declarations |

### 7.3 Dependency evidence and exact executed commands adopted

**`[VERIFIED; DEPENDENCY EVIDENCE]`** The synthesis relies on the following executed records without claiming they were rerun here:

- Documentation/help: `cargo run --locked --quiet --bin wg -- --help`, `cargo run --locked --quiet --bin worksgood -- --help`, and checkout-built `target/debug/wg --help-all` all exited 0; 130 conservatively parsed root commands; see `16-documentation-information-architecture.md:551-624`.
- Testing: smoke runner unit subset **11 passed**; contract tests **34 passed**; `integration_smoke_gate` **0 passed, 6 failed** both before and after worker-env sanitation; shell installer and static signing contract passed; see `17-testing-ci-quality.md:275-329`.
- Operations: built `wg`/`worksgood`; live `wg msg read` reproduced the array serialization failure; isolated config/setup/metrics reproductions observed the documented results; secret tests **17 passed**, project-profile overlay **3 passed**, worktree GC **14 passed**; see `18-operations-configuration-ux.md:480-549`.
- Concepts: a pinned binary SHA-256 `33d29c847870840d555a5dcfeb38a9083e972e7217efd624c77af6cf42726fd4` kept a successor blocked after predecessor failure, rejected `--converged`, and required a completion candidate; see `19-conceptual-model-and-vocabulary.md:385-518`.

### 7.4 Evidence limits and decision boundaries

**`[FACT]`** This synthesis did not run Cargo builds/tests, the smoke manifest, installers, manual/website generators, release workflows, TUI/browser automation, live providers, federation/provider/Pilot flows, signing/notarization, or destructive cleanup. It inspected source and accepted explicitly attributed leaf execution records.

**`[UNCERTAINTY]`** GitHub Actions run logs, published archives/assets, external websites, credential stores, flaky-test history, coverage reports, and production telemetry were not inspected. CI YAML establishes selection intent at the snapshot, not a successful hosted run.

**`[UNCERTAINTY]`** The target IA and product-contract examples are recommendations, not discovered hidden systems. No repository-wide manifest with those fields was found by the documentation audit's searches.

**`[FACT]`** Stable input IDs remain the leaf authority for detailed evidence. This synthesis preserves rather than replaces `DOC-*`, `TEST-*`, `OPS-*`, and `CONCEPT-*`; downstream drift and final syntheses should link back when narrowing or adjudicating them.

### 7.5 Artifact validation

The task-specific acceptance checks are:

```bash
test -s docs/audit/2026-08-08-worksgood-system/22-product-docs-quality-synthesis.md
git diff --check
```

The artifact explicitly includes all four dependency citations, a target information architecture (§2.5), evidence/authority/index/testing model (§2.6), vocabulary alignment (§2.4), operator journey (§2.3), systemic drift causes (§4.3), ranked recommendations (§6), and visible fact/verified/inference/recommendation/contradiction/uncertainty labels.
