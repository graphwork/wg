# Testing, CI, release evidence, and quality audit

**Audit date:** 2026-08-08

**Evidence checked through:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Inspection checkout:** `98b319c36aa8a21fd4506fc7469fe6d58978cdda` (the only path changed from the audit snapshot is the audit charter, per `git diff --name-only b0892ea..98b319c`)

**Artifact:** leaf audit required by `README.md` in this directory

**Change boundary:** this artifact only; production source, tests, workflows, and pre-existing documentation were not changed

## 1. Executive abstract

**`[FACT]`** This audit inspected the Rust unit/binary/integration targets, formal fixtures, Pi package tests, smoke manifest and runner, install tests, both GitHub workflows, release assembly, toolchain configuration, representative fixtures and command tests, and the authoritative `wg done` dispatch paths. It did **not** execute the full Rust suite, the 324-scenario smoke set, Lean, release jobs, signing/notarization, Windows/macOS flows, external model/provider flows, or destructive identity/provider/TUI scenarios.

**`[VERIFIED]`** Bounded checks executed on Linux against source-equivalent checkout `98b319c` produced mixed evidence: 11 smoke-runner unit tests passed; 34 response-contract tests passed; the shell release contract, synthetic shell installer, and static signing contract passed; PowerShell was skipped because `pwsh` was absent; Pi tests were not run because `worksgood-pi/node_modules` was absent. Most importantly, `cargo test --locked --test integration_smoke_gate -- --test-threads=1` failed all 6 tests. With ambient worker-control variables, fixture `wg init` was refused. After removing those variables, all 6 still failed because the tests expect the retired completion path (`missing completion candidate` or rejection of legacy `--skip-smoke` flags). Commands and bounded output are in section 7.

**`[FACT]` `TEST-001` — S1 High, shipped/current, high confidence:** the advertised owner-selected smoke gate is not on the authoritative publication-derived completion path. The CLI routes ordinary `wg done` directly to `completion_done::run` and rejects `--full-smoke`/`--skip-smoke` as legacy (`src/main.rs:1261-1275`). Worker handoff likewise routes to `completion_done::run` and rejects `full_smoke` (`src/worker_cli.rs:345-360`; `src/commands/service/ipc.rs:910-918`). `completion_done::run` verifies immutable review/publication evidence but does not load or run the smoke manifest (`src/commands/completion_done.rs:32-104`). The smoke call remains in a separate historical `commands/done.rs` path (`src/commands/done.rs:1583-1678`, `2024-2030`). This contradicts the repository's smoke contract and the task-agent guide. The permanent end-to-end smoke-gate test is stale and is not selected by CI.

**`[FACT]` `TEST-002` — S1 High, shipped/current, high confidence:** normal CI does not execute the general integration suite or any manifest smoke scenario. Cargo metadata exposes 177 integration-test targets (176 `tests/*.rs` files plus `tests/upgrade/main.rs`). CI explicitly runs seven lifecycle/completion targets and `integration_service`; all other top-level integration targets, including `integration_smoke_gate`, are absent from the workflow (`.github/workflows/ci.yml:71-80`, `113-125`, `127-162`). The repository therefore has broad executable specifications without a broad continuous gate. The nightly job is library-only and `continue-on-error` (`.github/workflows/ci.yml:203-230`).

**`[INFERENCE]`** These two findings create false confidence: “there is a smoke script/test” and “CI is green” do not currently imply either that the smoke ownership gate was reached or that most command/integration contracts passed. Confidence is high because both dispatch source and an executed stale integration target agree. A falsifying check would be a current publication-derived end-to-end test in CI that installs a failing owned scenario and proves the completion mutation is refused; none was found.

**`[FACT]`** Positive controls are substantial. CI does run the 3,149-test library harness serially, a Rust/Lean conformance subset, a completion canary, an isolated `cargo install`, Linux and Windows synthetic installer checks, and Pi build/selftests/Vitest/embed-staleness checks (`.github/workflows/ci.yml:68-201`). The release workflow builds five target triples with locked dependencies, hashes and attests archives, records signing status, and verifies tag/version consistency (`.github/workflows/release.yml:61-117`, `119-181`, `450-577`, `595-688`). These controls are real but narrower than the feature breadth and release claims.

**Next decision:** treat restoration of an authoritative, publication-derived smoke gate and CI execution of current integration contracts as release-blocking P0 work. Until then, describe manifest scenarios as an inventory runnable out of band—not as a hard `wg done` gate.

## 2. Scope and map

### 2.1 Verification taxonomy and counts

Counts below are static inventory/proxies, not proof of execution or semantic coverage.

| Layer | Snapshot inventory | What it can establish | Executed in this audit? | Default CI selection |
|---|---:|---|---|---|
| Rust library unit tests | `cargo test --lib -- --list` reported **3,149** tests | Pure/module behavior compiled into `src/lib.rs` harness | Only 11 `smoke::tests` | Yes, serial (`ci.yml:71-77`) |
| Rust `wg` binary unit harness | `cargo test --bin wg -- --list` reported **3,821** tests; source modules overlap the library harness | Binary-only modules and `#[cfg(test)]` callers | Listed, not run as a whole | Compiled and a completion-canary filter run (`ci.yml:124-125`), not whole harness |
| `worksgood` binary harness | **0** listed tests | Nothing beyond build/help at this target | Listed | Invoked, but zero tests (`ci.yml:76-77`) |
| Cargo integration targets | **177** Cargo targets: 176 top-level `tests/*.rs` plus `tests/upgrade/main.rs`; static top-level scan found **2,618** `#[test]` attributes | Real-binary, temp-dir, fixture and cross-module contracts, depending on target | `contract_tests` (34 pass); `integration_smoke_gate` (6 fail) | Only 8 named targets; 169 target names not selected |
| Ignored Rust tests | **180** `#[ignore]` attributes under `src/` + `tests/` (66 source, 114 tests) | Opt-in/retired/live/flaky contracts only when explicitly included | None | Ignored by default; no `--ignored`/`--include-ignored` job |
| Rust doc tests | inventory not enumerated separately | Compilable documentation examples | Not run | Yes (`ci.yml:79-80`) |
| Smoke manifest | **324** entries: 323 scenario-directory scripts plus `../install/release_contract.sh`; 393 unique owners / 535 owner references | Scripted CLI/TUI/network/static assertions when actually invoked | Only static signing scenario directly; full manifest not run | No manifest scenario job |
| Install/upgrade | shell and PowerShell installer smoke; release static contract; upgrade fixture target | Synthetic archive install/upgrade/uninstall/collision/checksum behavior | Shell installer + static contract pass; PowerShell skipped | Shell + Windows PowerShell yes |
| Pi TypeScript | 5 `worksgood-pi/test/*.ts` files; static scan found 95 `describe`/`it`/`test` call sites | Plugin API/bridge/backend behavior and source/build embed parity | Not run (dependencies absent) | Build, two selftests, Vitest, embed diff |
| Formal | 9 Lean files and 36 formal fixture files | Modeled lifecycle/reducer safety within declared abstraction | Not run | Lean build/proof-escape scan + seven Rust conformance targets |
| Fixtures/snapshots | 26 `tests/fixtures` files, 2 smoke fixture files, 14 Insta snapshots, 36 formal fixtures | Deterministic inputs/goldens when a selected test consumes them | Contract fixtures sampled | Selected transitively by tests; no fixture-usage census |
| Ancillary Python/bench | `terminal-bench/tests/` and root scripts/test-like files | Separate benchmark/adapter behavior | Not run | No workflow selection found |

**`[UNCERTAINTY]`** The 3,149 and 3,821 harness counts overlap because command modules are compiled into more than one crate surface; they must not be summed as unique tests. Static `#[test]` counts may include cfg-gated tests. Test count is a size proxy, not line/branch/invariant coverage.

### 2.2 CI pipeline map

```text
push main / pull request
  ├─ Check & Lint (Ubuntu)
  │    fmt; clippy default + worksgood; static release contract;
  │    synthetic shell installer
  ├─ Build & Test (Ubuntu)
  │    locked all-bin build; lib tests serial; worksgood bin (0 tests); docs
  ├─ Lifecycle Formal (Ubuntu)
  │    Lean escape scan/build/oracle; 7 Rust conformance targets;
  │    filtered wg completion canary
  ├─ Integration (Ubuntu, 15m)
  │    isolated cargo install + help for worksgood/wg/nex;
  │    integration_service only
  ├─ Windows installer
  │    synthetic PowerShell installer lifecycle only
  ├─ Pi package (Ubuntu, Node 22)
  │    npm ci/build/selftests/Vitest; re-embed and diff
  └─ Nightly (Ubuntu, non-blocking)
       build + library tests only

tag/workflow dispatch (separate Release workflow)
  ├─ resolve tag/version/dry-run
  ├─ locked release build on 5 target/runner pairs
  ├─ optional OS signing (absence warns but succeeds)
  ├─ package 3 binaries; checksum + Sigstore attestation
  └─ assemble manifest; optionally publish GitHub Release
```

**`[FACT]`** No job in `ci.yml` runs `cargo test --tests`, all integration targets, all features, no-default-features, an MSRV build, the manifest smoke set, coverage, mutation testing, sanitizer/Miri, or release archive installation. The Rust PR pipeline is Ubuntu-only except the synthetic PowerShell installer. The release workflow builds other OS targets, but it is a separate tag/dispatch workflow and contains no test-suite dependency or test job.

### 2.3 Smoke ownership and gate flow

| Step | Historical smoke implementation | Authoritative completion-v3 path |
|---|---|---|
| Resolve manifest | `Manifest::resolve_path` probes env/local/parent/git (`src/smoke.rs:104-136`) | No call found |
| Missing manifest | Returns an empty manifest, i.e. fail-open (`src/smoke.rs:70-76`) | No call found |
| Owner selection | Exact task-id equality; no owned scenario is a quiet no-op (`src/commands/done.rs:1620-1628`) | No call found |
| Execution | Sequential `bash`, optional GNU `timeout`, buffered output (`src/smoke.rs:166-240`, `334-356`) | No call found |
| Result policy | pass 0; skip 77 non-blocking; fail/error blocks historical `done` | No call found |
| Worker flags | Historical docs advertise `--full-smoke`; current worker IPC rejects it (`src/worker_cli.rs:345-360`; `ipc.rs:910-918`) | Rejected |
| Done mutation | Historical `commands/done.rs` calls gate before legacy completion | `completion_done::run` verifies immutable candidate/reviews/publication, then commits; no smoke call (`completion_done.rs:32-104`) |

**`[FACT]`** Manifest integrity is currently good in a narrow static sense: 324 unique names, 324 unique scripts, no empty owner lists, no empty descriptions, all timeouts present, and every manifest script path exists. The apparent “324 scripts = 324 entries” count is not a name-level proof: the scenario directory has 324 shell files only when `_helpers.sh` is counted, while the manifest has 323 scenario scripts plus one install script.

**`[FACT]`** Ownership is sparse and historical: 324 scenarios contain 393 unique owner IDs and 535 owner references; median owner count is 1, maximum 8. Only 53/324 entries include the README-required `smoke-gate-is` owner (`tests/smoke/README.md:31-44`). `audit-testing` owns no entry. Even on the historical path, an unlisted new task is a quiet no-op.

**`[FACT]`** A full sequential run has a declared timeout ceiling of 31,555 seconds (8.77 hours), before setup overhead. `run_scenarios` is sequential. On systems without a successful `timeout --version`, the runner falls back to plain `bash` and applies no timeout (`src/smoke.rs:184-197`). This is especially relevant to default macOS environments where GNU `timeout` is not normally present.

### 2.4 Feature-to-evidence sample matrix

This is a representative sample, not an exhaustive feature census.

| Advertised/current feature or invariant | Enforcement/source sampled | Executable evidence inspected | CI / audit execution | Residual confidence/gap |
|---|---|---|---|---|
| Publication-derived completion requires exact candidate, FLIP/eval and current publication | `src/commands/completion_done.rs:32-104` | lifecycle/completion conformance targets; completion canary | CI-selected; not executed here | Strong reducer/evidence coverage; effectful publication still bounded by adapters |
| “Owned failing smoke blocks `wg done`” | Historical `src/commands/done.rs:1583-1678`, not authoritative main/IPC route | `tests/integration_smoke_gate.rs:1-11`, `130-256`, `335-443` | Not CI-selected; **6/6 failed here** | **Contradicted/currently unproven; source says bypassed** |
| Smoke cleanup survives daemon orphan/panic | helper lifecycle (`_helpers.sh:21-33`, `290-455`) and Rust `/proc` sweep (`src/smoke.rs:334-439`) | `smoke_cleanup_survives_panic` manifest scenario; 11 runner unit tests | Runner units pass here; real scenario not run/CI-selected | Positive unit evidence; real process cleanup not reverified |
| Pi plugin build/compat/embed parity | package scripts (`worksgood-pi/package.json:30-47`), committed embed script | 5 Vitest files, host selftests, embed regeneration diff | Strong CI selection; not run here due absent node_modules | Best-aligned package subpipeline; live provider/TUI smokes still outside CI |
| Lifecycle formal safety and Rust conformance | `formal/` model and production reducers | 9 Lean files, 36 fixtures, 7 named Rust targets | Strong CI selection; not run here | Good modeled-state evidence; formal README explicitly excludes OS/Git/fs/network/provider/UI effects (`formal/README.md:3-5`, `26-29`, `133-138`) |
| WG-Fed identity/recovery/ACL/UCAN | `src/identity/` (98 static test attrs) | four federation smoke scenarios around manifest lines 1875-1919 | library tests CI; scenarios not CI | Component logic covered; cross-process/network composition is not continuously run |
| Review/Exec/Pilot composition | `src/review/` (54 attrs), `src/providers/` (54 attrs), pilot command | `content_safety_spark`, `exec_spark_borrowed_box`, `e2e_family_team`, `pilot_dry_run` | library tests CI; scenarios not CI | Advertised end-to-end evidence exists but is not a PR/release gate |
| TUI human flows | `src/tui/` (large unit surface) | static inventory found 88 smoke scripts mentioning tmux | unit harness CI; no smoke in CI | Render/state logic is broad; terminal/PTY behavior remains environment- and ownership-triggered |
| Shell/PowerShell install, upgrade, collision and uninstall | `scripts/install-wg.*` | `tests/install/installer_smoke.{sh,ps1}` | Both CI; shell passed here, PowerShell skipped locally | Good synthetic installer behavior; archives contain fake shell payloads, not release binaries (`installer_smoke.sh:44-105`) |
| Release signing and provenance | `release.yml` signing/archive/attestation | static release contract and static signing smoke | Static contracts passed here/CI (release contract); real signing only on release runners | Presence/order verified; no signature operation, published asset, or attestation verification executed here |
| CLI command reachability | 157 top-level command-name mappings in `src/cli.rs` | broad integration/smoke tree | only sampled targets in CI | Lexical proxy found 11 exact command names absent from `tests/`: see finding `TEST-010` |

## 3. Findings

### `TEST-001` — authoritative smoke gate is disconnected

- **Label/state:** **`[FACT]`**, shipped/current.
- **Severity/likelihood/confidence:** **S1 High; observed; high confidence.**
- **Affected boundary:** every task completion advertised as owner-smoke-gated; release/regression assurance.
- **Evidence:** current main and worker paths call `completion_done::run` and reject smoke flags (`src/main.rs:1261-1275`; `src/worker_cli.rs:345-360`; `src/commands/service/ipc.rs:910-918`). The completion-v3 function has no smoke invocation (`src/commands/completion_done.rs:32-104`). The only `run_smoke_gate` call is in historical `commands/done.rs:2024-2030` (confirmed by exact `rg -n 'run_smoke_gate\('`). All six end-to-end integration tests failed when executed.
- **Counterevidence:** the historical runner itself is implemented and its 11 isolated unit tests passed; the manifest is structurally complete. This proves runner mechanics, not reachability.
- **Recommendation:** `TEST-REC-001`.

### `TEST-002` — CI omits 169 of 177 Cargo integration targets

- **Label/state:** **`[FACT]`**, shipped/current.
- **Severity/likelihood/confidence:** **S1 High; likely to miss regressions; high confidence.**
- **Affected boundary:** CLI dispatch, service, routing, persistence, federation, TUI and integration contracts.
- **Evidence:** Cargo metadata returned 177 test targets. CI explicitly names seven formal/completion targets plus `integration_service` (`.github/workflows/ci.yml:113-125`, `127-162`). It runs library/doc tests but no `cargo test --tests` (`ci.yml:68-80`). `integration_smoke_gate`—now failing—demonstrates a real stale target not selected by CI.
- **Counterevidence:** 3,149 library tests run, the `wg` binary test target is compiled by the filtered completion canary, and selected lifecycle targets are meaningful. The gap is execution breadth, not absence of tests.
- **Recommendation:** `TEST-REC-002`.

### `TEST-003` — smoke manifest policy is not enforced on the manifest

- **Label/state:** **`[FACT]`** plus bounded **`[INFERENCE]`**, partial.
- **Severity/likelihood/confidence:** **S2 Medium; observed static drift; high confidence for counts.**
- **Evidence:** README requires every new scenario to include `smoke-gate-is`, source `_helpers.sh`, use `make_scratch`/`start_wg_daemon`, and avoid own traps (`tests/smoke/README.md:31-69`). Static inventory found only 53/324 with `smoke-gate-is`; 21 scenario-directory scripts do not source `_helpers.sh`; 27 contain active `trap` lines; 25 contain active `mktemp -d`; and 15 contain active direct `wg ... service start` lines. Some may be deliberate self-tests or safe local cleanup, so counts are policy-screening results, not 27 proven leaks.
- **Positive control:** all names/scripts are unique and present; owners/descriptions/timeouts are nonempty.
- **Inference:** without a CI manifest linter, the grow-only corpus accumulates exceptions faster than its written fixture contract can be trusted.
- **Recommendation:** `TEST-REC-003`.

### `TEST-004` — skip and environment policy weakens smoke evidence

- **Label/state:** **`[FACT]`** and **`[INFERENCE]`**, shipped/current.
- **Severity/likelihood/confidence:** **S2 Medium; possible/observed environmental failures; medium-high confidence.**
- **Evidence:** exit 77 never blocks (`src/smoke.rs:215-231`; README:16-20); 158 scenario scripts contain `loud_skip`, and 26 contain explicit `exit 77` (static occurrence counts, overlapping). The helper only unsets `WG_DIR`, project/worktree/task variables (`_helpers.sh:37-50`), not `WG_WORKER_CAPABILITY`, `WG_WORKER_CONTROL_PROTOCOL`, or `WG_WORKER_IPC`. The first audit run inherited those variables and all six integration cases failed at `wg init` with `worker_control.operation_refused`. Sanitizing them exposed the separate stale-completion failures. Full-smoke is sequential with 8.77 hours declared timeout and may be unbounded without GNU `timeout`.
- **Inference:** a non-blocking SKIP is appropriate for unavailable live services only if release evidence tracks skip counts and required classes; currently it can make “done” compatible with zero live assertions.
- **Recommendation:** `TEST-REC-001`, `TEST-REC-003`, `TEST-REC-004`.

### `TEST-005` — release construction is stronger than release qualification

- **Label/state:** **`[FACT]`** and **`[INFERENCE]`**, partial.
- **Severity/likelihood/confidence:** **S1 High for release-assurance gap; possible; high confidence.**
- **Evidence:** release builds five targets, packages/hash-attests artifacts, and verifies stable tag/version (`release.yml:61-117`, `119-181`, `450-577`, `595-688`). But it is triggered separately by tags/dispatch and has no CI/test dependency or tests. It packages only `worksgood`, `wg`, and `nex` (`release.yml:470-487`, `646-667`) even though Cargo declares a fourth `casa-adapter` binary (`Cargo.toml:23-41`) and builds `--bins`. Installer tests create synthetic shell/text executables (`tests/install/installer_smoke.sh:44-105`) rather than install the just-built archive. No workflow step extracts each release archive and executes its binaries or installer. macOS/Windows signing credentials may be absent; the build exits successfully and publishes with a warning (`release.yml:208-224`, `320-331`, `792-815`).
- **Counterevidence:** checksums, Sigstore attestations, signing verification when credentials exist, explicit unsigned status in notes, and static contracts are valuable.
- **Inference:** a green release workflow proves artifact construction/provenance, not that the shipped archive passes the tested install/runtime journeys or that CI was green.
- **Recommendation:** `TEST-REC-005`.

### `TEST-006` — Pi package has the clearest source-to-embedded anti-drift gate

- **Label/state:** **`[FACT]`**, shipped/current positive control.
- **Severity/confidence:** **S4 Informational; high confidence.**
- **Evidence:** the package declares build, Vitest and host selftest scripts (`worksgood-pi/package.json:30-47`). CI performs clean npm install, build, two host compatibility selftests, Vitest, re-embed, and `git diff --exit-code` (`ci.yml:174-201`).
- **Limitation:** this audit did not run it because local `node_modules` was absent; live provider/TUI scenarios remain outside CI. Peer dependencies use `*` while dev dependencies pin `^0.79.4` (`package.json:37-47`), so consumer compatibility breadth exceeds the CI version sample.

### `TEST-007` — formal evidence is explicitly bounded and well connected to selected reducers

- **Label/state:** **`[FACT]`**, shipped/current positive control.
- **Severity/confidence:** **S4 Informational; high confidence.**
- **Evidence:** CI rejects common proof escapes, builds Lean/oracle, and runs seven Rust conformance targets (`ci.yml:82-125`). The formal README names production reducer seams and fixture replay (`formal/README.md:18-29`, `86-113`, `115-138`) and explicitly excludes OS, filesystem, Git, provider and UI behavior (`formal/README.md:3-5`, `133-138`).
- **Limitation:** this audit did not run Lean. The formal claims should not be generalized to the omitted effectful boundaries.

### `TEST-008` — toolchain, feature, MSRV and platform matrices are incomplete

- **Label/state:** **`[FACT]`** and **`[UNCERTAINTY]`**, partial.
- **Severity/likelihood/confidence:** **S2 Medium; possible; high confidence for missing jobs.**
- **Evidence:** Cargo declares Rust 1.85 and optional `matrix`, `telegram`, `email`, `slack`, and `llm-tests` features (`Cargo.toml:10`, `43-54`). CI tests current default features only; there is no MSRV, `--all-features`, or no-default-features job. Rust tests run on Ubuntu; Windows gets installer script testing, while macOS Rust behavior is release-build-only. `rust-toolchain.toml` pins 1.96.0 and claims byte-identical CI formatting (`rust-toolchain.toml:1-19`), while every workflow Rust setup says `dtolnay/rust-toolchain@stable` (`ci.yml:18-21`, `54-56`, `110-112`, `134-136`; release uses stable too).
- **Uncertainty:** local commands did resolve to 1.96.0. Without a workflow log or a pinned action/toolchain input, this audit did not establish whether the action's floating `stable` selection or the repository override wins in each job. The configuration text is at least ambiguous and future-drifting.
- **Recommendation:** `TEST-REC-006`.

### `TEST-009` — ignored, timed, networked, and process-global tests lack an explicit quarantine policy

- **Label/state:** **`[FACT]`**, current inventory.
- **Severity/likelihood/confidence:** **S2 Medium; possible; medium confidence.**
- **Evidence:** static scan found 180 ignored attributes under `src` and `tests`: reasons include retired execution planes, credential requirements, real E2E, daemon subprocesses, and two explicitly “Flaky timing-sensitive” `integration_service` tests. The suite contains 142 `#[serial]` attributes, 297 sleep calls, 29 Rust test files with HTTP literals, 252 `Command::new` calls, and 1,467 `TempDir::new` calls. `llm-tests` appears as a non-default feature in four integration families. CI has no ignored-test census/job or policy preventing newly ignored tests.
- **Uncertainty:** keyword counts do not prove flakiness or network use at runtime; many sleeps/subprocesses are legitimate. No historical CI failure-rate data was available.
- **Recommendation:** `TEST-REC-007`.

### `TEST-010` — direct command evidence has identifiable holes

- **Label/state:** **`[FACT]`** as a lexical coverage proxy; **`[UNCERTAINTY]`** about indirect coverage.
- **Severity/likelihood/confidence:** **S2 Medium; possible; medium confidence.**
- **Method/evidence:** parse the 157 `Commands::<Variant> => "command-name"` mappings in `src/cli.rs`, then search token-bounded exact command names across text files under `tests/`. Eleven names had no exact test-tree mention: `chat-runtime-wrapper`, `classify-no-op`, `dead-agents`, `graph-export`, `record-telemetry`, `reprioritize`, `reschedule`, `screencast`, `trajectory`, `tui-nex`, and `tui-pty`.
- **Counterevidence:** `dead_agents.rs`, `reprioritize.rs`, `reschedule.rs`, `trajectory.rs`, and `tui_pty.rs` contain inline unit tests; generated wrappers mention telemetry/no-op commands in production source. Thus the result identifies missing **direct CLI-name evidence**, not necessarily untested underlying functions.
- **Recommendation:** prioritize direct real-binary negative/happy-path contracts for internal commands that mutate state or control workers, then user-visible PTY flows.

### `TEST-011` — test organization is broad but authority is fragmented

- **Label/state:** **`[FACT]`** plus **`[INFERENCE]`**, current.
- **Severity/likelihood/confidence:** **S3 Low; likely maintenance drag; high confidence.**
- **Evidence:** 126 top-level targets begin `integration_`, 23 use legacy `test_*`, 7 use `smoke*`, 5 are contract/conformance named, and 15 have other names. Separate test-like roots include `terminal-bench/tests`, root `integration_robustness_test.sh`, `verify_integration_tests.sh`, and root `test_cron_integration.rs`; they are not selected in either workflow. The smoke corpus mixes real endpoint/TUI flows, credential-free fake binaries, cargo compilation, and static workflow greps under one “live, not stubs” label.
- **Inference:** without a machine-readable taxonomy/CI ownership map, file presence overstates active verification and stale targets are hard to detect.
- **Recommendation:** `TEST-REC-002`, `TEST-REC-003`, `TEST-REC-007`.

## 4. Contradictions and drift

### `TEST-DRIFT-001` — smoke contract versus authoritative completion (**open, S1**)

**`[CONTRADICTION]`** `tests/smoke/README.md:1-29`, `src/smoke.rs:1-17`, and the emitted agent workflow claim owned smoke runs before `wg done`. Current CLI and worker dispatch instead call `completion_done::run`, reject `--full-smoke`, and never invoke the manifest (`src/main.rs:1261-1275`; `src/worker_cli.rs:345-360`; `src/commands/service/ipc.rs:910-918`; `src/commands/completion_done.rs:32-104`). Authority is current dispatch source. Resolution: restore a publication-derived smoke receipt/gate or narrow every claim immediately.

### `TEST-DRIFT-002` — permanent smoke-gate integration test versus completion-v3 (**open, S1**)

**`[CONTRADICTION]`** `tests/integration_smoke_gate.rs:1-11` says its six tests pass in any environment and lock the `wg done` rule. Executed on the snapshot-equivalent source, all six failed. After sanitizing ambient worker-control variables, errors were `missing completion candidate` and rejection of legacy bypass flags. The test creates/claims a simple task then calls legacy `wg done` without constructing completion-v3 evidence (`integration_smoke_gate.rs:96-127`, `161-191`, `231-256`, `353-443`). The target is absent from CI.

### `TEST-DRIFT-003` — claimed binary-test blind spot is partly superseded (**open, S3**)

**`[CONTRADICTION]`** `tests/smoke/scenarios/bin_test_target_compiles.sh:4-18`, `45-54` says CI “NEVER compiles” the `wg` binary test target. Current CI runs `cargo test --locked --bin wg commands::completion_canary_tests`, which compiles that test harness (`ci.yml:124-125`). It does not run the whole 3,821-test harness, so an execution blind spot remains; the compilation claim is stale.

### `TEST-DRIFT-004` — “live, not stubs” versus manifest contents (**open, S2**)

**`[CONTRADICTION]`** Smoke README says scenarios MUST hit real endpoints/real binaries and directs stubs to unit tests (`tests/smoke/README.md:82-87`). Yet `release_workflow_signing_contract.sh:2-10` explicitly calls itself a static contract test, `bin_test_target_compiles.sh` only compiles, and static inventory found 84 scenario scripts containing fake/mock/stub vocabulary. Some fake binaries may be deliberate boundary tests and some words may be comments, but the categorical README claim is false for at least the explicit static scenario.

### `TEST-DRIFT-005` — fixture cleanup contract versus corpus (**open, S2**)

**`[CONTRADICTION]`** README and helper prohibit direct traps, `mktemp -d`, and direct daemon starts (`tests/smoke/README.md:46-75`; `_helpers.sh:21-33`). Static inventory found active occurrences in 27, 25, and 15 scenario scripts respectively; 21 scenarios do not source the helper. Some are cleanup-self-tests or scripts that do not create daemon fixtures, so per-file adjudication is still required.

### `TEST-DRIFT-006` — exact Rust pin versus floating workflow declaration (**open uncertainty, S2**)

**`[CONTRADICTION]`** `rust-toolchain.toml:1-19` says local and CI use exactly 1.96.0, while workflows configure `dtolnay/rust-toolchain@stable`. Local resolution was 1.96.0. The action/override precedence was not verified from a CI log, so current runtime authority remains uncertain; the declarations should not rely on implicit precedence.

### `TEST-DRIFT-007` — four Cargo binaries versus three release/install binaries (**open product-boundary decision, S2**)

**`[CONTRADICTION]`** Cargo declares `wg`, `nex`, `worksgood`, and `casa-adapter` (`Cargo.toml:23-41`); `cargo build --bins` builds all four. Release archives/manifests/install tests enumerate only the first three (`release.yml:470-487`, `646-667`; `release_contract.sh:29-48`). Comments call Casa a companion adapter, but no explicit packaging contract sampled here states whether normal Cargo installs should include it while native releases should not. Product/operations owners should resolve rather than infer.

## 5. Risks and gaps

| Rank | ID | Severity | Likelihood | Risk / false-confidence mode | Missing evidence |
|---:|---|---:|---|---|---|
| 1 | `TEST-RISK-001` | S1 | observed | Authoritative completion can reach Done without the advertised owned smoke gate; full-smoke flag is rejected | Current v3 failing-scenario end-to-end test and durable smoke receipt |
| 2 | `TEST-RISK-002` | S1 | likely | Green CI omits 169/177 integration target names; stale tests and dispatch breaks can persist | Sharded current integration execution and target allowlist/delta gate |
| 3 | `TEST-RISK-003` | S1 | possible | Release workflow can publish independently of CI and never installs/runs its actual archives | Required CI provenance, extract/run/install per target, publish policy |
| 4 | `TEST-RISK-004` | S2 | possible | Exit-77 and absent-owner/missing-manifest fail-open semantics turn unavailable evidence into completion | Required-vs-advisory scenario classes and skip budget/report |
| 5 | `TEST-RISK-005` | S2 | possible | Manifest fixture exceptions can leak daemons or make full smoke hang; macOS may lack GNU timeout | Manifest linter, portable timeout, bounded sharding |
| 6 | `TEST-RISK-006` | S2 | possible | Default-feature Ubuntu tests miss optional integrations, MSRV and OS-specific Rust behavior | Feature/MSRV/platform matrix |
| 7 | `TEST-RISK-007` | S2 | possible | Ignored/retired/live tests accumulate without owner, expiry, or execution telemetry | Ignored-test manifest and scheduled credentialed/quarantine lane |
| 8 | `TEST-RISK-008` | S2 | possible | Static release/signing tests prove text presence/order, not real signatures or installability | Release dry-run logs/artifacts and verification rehearsal |
| 9 | `TEST-RISK-009` | S3 | likely | Mixed naming and test roots obscure what Cargo/CI discovers | Machine-readable taxonomy and orphan-test census |
| 10 | `TEST-RISK-010` | S3 | unknown | No coverage/mutation proxy makes breadth look like depth | Coverage trend, changed-line proxy, critical-module mutation sample |

**`[UNCERTAINTY]`** No GitHub Actions run logs, coverage reports, flaky-test history, release assets, signing credentials, external provider credentials, macOS host, or Windows host were inspected. Test absence is a gap, not proof that the corresponding behavior is broken. Conversely, source presence is not proof that a test is selected or passing.

## 6. Recommendations

1. **`TEST-REC-001` — `[RECOMMENDATION]` P0, completion/smoke owners:** wire owner-selected smoke into the publication-derived completion transaction, before the terminal mutation, and persist an exact policy/manifest/scenario result digest. Make `--full-smoke` meaningful or remove it from help/agent contracts. Acceptance: a current real-binary test constructs valid completion-v3 evidence, installs one owned failing scenario, proves `Done` is refused, then passes after the scenario succeeds; run this target in CI.
2. **`TEST-REC-002` — `[RECOMMENDATION]` P0, CI owners:** execute all non-live Cargo integration targets in deterministic shards, with an explicit quarantine manifest for exceptions. At minimum add `integration_smoke_gate` after modernization. Acceptance: `cargo metadata` target set is compared with the CI shard allowlist; an unclassified new target fails CI; shard logs record pass/fail/ignored counts.
3. **`TEST-REC-003` — `[RECOMMENDATION]` P0, smoke owners:** add a static manifest/script linter in ordinary CI. Validate unique name/path, nonempty owners/descriptions/timeouts, `smoke-gate-is`, helper use or a declared exception class, no clobbering traps/direct daemon starts, loud skip format, and portable timeout. Acceptance: the existing exception list is reviewed per file and cannot grow silently.
4. **`TEST-REC-004` — `[RECOMMENDATION]` P1, release/test owners:** classify scenarios as `required-hermetic`, `required-platform`, `live-advisory`, or `static-contract`; stop calling all of them live. Required scenarios must fail closed; advisory skips must be counted and published. Acceptance: completion/release evidence states pass/skip/not-run by class and forbids zero required assertions.
5. **`TEST-REC-005` — `[RECOMMENDATION]` P0, release owners:** make publish depend on a tested commit and qualify actual archives. Extract each archive, verify the exact binary set/metadata/checksum/attestation, run `--help` and a clean temp-prefix install/uninstall, and gate unsigned stable publishing by an explicit release policy rather than warning alone. Resolve Casa packaging. Acceptance: a release-test tag yields downloadable evidence and stable publish cannot proceed with failed CI or disallowed signing state.
6. **`TEST-REC-006` — `[RECOMMENDATION]` P1, build owners:** explicitly pin the CI Rust version/action, add MSRV 1.85, `--all-features` compile/test where feasible, no-default-features, and Windows/macOS Rust compile smoke. Acceptance: workflow logs print exact toolchains and feature sets; `rust-toolchain.toml` wording matches actual precedence.
7. **`TEST-REC-007` — `[RECOMMENDATION]` P1, test owners:** create an ignored-test register with owner, reason class, environment, command, last-pass date, expiry/removal decision, and replacement evidence. Run credential-free ignored tests in scheduled quarantine and credentialed tests in a protected lane. Acceptance: newly ignored tests without metadata fail lint.
8. **`TEST-REC-008` — `[RECOMMENDATION]` P2, quality owners:** add coverage proxies that do not turn percentages into guarantees: critical-invariant matrix, changed-command direct test check, line/branch trend, and mutation sampling for completion/smoke/identity/lease reducers. Acceptance: reports link uncovered branches to owned gaps rather than enforcing a blind global percentage.
9. **`TEST-REC-009` — `[RECOMMENDATION]` P1, command owners:** add direct real-binary tests for the 11 command-name gaps, prioritizing worker-control internal commands and state-mutating operator commands; require PTY human-flow tests for `tui-nex`/`tui-pty`. Acceptance: exact command-name proxy has no unexplained P0/P1 gaps.

## 7. Evidence appendix

### 7.1 Revision and environment

**`[VERIFIED]`** Commands ran in `/home/bot/wg/.wg-worktrees/agent-9` on 2026-08-08 UTC. Inspection checkout `98b319c36aa8a21fd4506fc7469fe6d58978cdda`; `git diff --name-only b0892ea..HEAD` returned only `docs/audit/2026-08-08-worksgood-system/README.md`, so audited production/test/workflow bytes match the pinned snapshot. Environment: Linux `6.8.0-90-generic x86_64`, Rust/Cargo `1.96.0`, Node `25.4.0`, npm `11.13.0`, Python `3.12.3`.

### 7.2 Commands actually executed

| Exact command | Exit | Duration/result | Interpretation |
|---|---:|---|---|
| `tests/install/release_contract.sh` | 0 | <1s; PASS | Static text/shape contract only |
| `tests/install/installer_smoke.sh` | 0 | 2s; shell install/dry-run/upgrade/collision/checksum/uninstall PASS; PowerShell SKIP | Synthetic archives and fake executables, isolated temp HOME |
| `cargo test --locked --lib smoke::tests -- --test-threads=1` | 0 | 523s compile, 0.09s tests; **11 passed**, 3,138 filtered | Runner unit mechanics pass; not done-path reachability |
| `cargo test --locked --test integration_smoke_gate -- --test-threads=1` | 101 | 120s; **0 passed, 6 failed** at `wg init` due inherited worker-control authority | Exposed test environment isolation gap |
| `env -u WG_WORKER_CAPABILITY -u WG_WORKER_CONTROL_PROTOCOL -u WG_WORKER_IPC -u WG_GRAPH_ID -u WG_SPAWN_EPOCH -u WG_SPAWN_RUN_ID cargo test --locked --test integration_smoke_gate -- --test-threads=1` | 101 | 84s; **0 passed, 6 failed** with completion-v3/legacy-flag errors | Confirms test and smoke contract are stale even after env sanitation |
| `cargo test --locked --test contract_tests` | 0 | 19s; **34 passed** | Fixture/parser contract sample |
| `bash tests/smoke/scenarios/release_workflow_signing_contract.sh` | 0 | 1s; static plist/macOS/Windows/Sigstore checks PASS | Workflow text/order only; no signing operation |
| conditional `npm --prefix worksgood-pi test` | 77 (audit-local skip) | not run; `node_modules` absent | Not a product test result; CI installs dependencies |
| `cargo test --locked --lib -- --list` | 0 | **3,149 tests** | Inventory only |
| `cargo test --locked --bin wg -- --list` | 0 | **3,821 tests** | Inventory/compile only |
| `cargo test --locked --bin worksgood -- --list` | 0 | **0 tests** | Inventory only |
| `cargo metadata --locked --no-deps --format-version=1` | 0 | 182 targets: 1 lib, 4 bins, 177 tests | Discovery inventory |

**`[FACT]`** Compiler warnings were emitted during targeted builds (27 library warnings in the first build and 133 `wg` binary warnings in later target builds). This audit did not classify them individually; successful compilation is not a lint result.

### 7.3 Static inventory commands and results

The following were inventory only; no test behavior was executed:

```bash
find tests -maxdepth 1 -type f -name '*.rs' | wc -l             # 176
find tests/smoke/scenarios -maxdepth 1 -type f -name '*.sh' | wc -l  # 324 incl. helper
rg -n '^\[\[scenario\]\]' tests/smoke/manifest.toml | wc -l  # 324
find tests/fixtures -type f | wc -l                             # 26
find tests/snapshots -type f | wc -l                            # 14
find formal/fixtures -type f | wc -l                            # 36
find worksgood-pi/test -type f | wc -l                          # 5
```

A Python `tomllib` name/path/owner join found: 324 unique scenario names and scripts; no missing referenced script; no unmanifested scenario script after excluding `_helpers.sh`; 393 unique owners; 535 owner references; 53 scenarios owned by `smoke-gate-is`; all descriptions/owners/timeouts nonempty; timeout sum 31,555s. Regex screening of scenario source found 302/323 source `_helpers.sh`, 158 contain `loud_skip`, 27 active `trap`, 25 active `mktemp -d`, 15 active direct `wg ... service start`, 88 mention tmux, 127 contain network/provider markers, and 84 contain fake/mock/stub vocabulary. These are triage counts, not semantic verdicts.

A Rust-source scan found 180 ignored attributes under `src` and `tests`, 142 serial attributes, 297 sleep calls, 29 test files with HTTP literals, 252 `Command::new` occurrences, and 1,467 `TempDir::new` occurrences. A command-name mapping/search found the 11 direct-name gaps listed in `TEST-010`.

### 7.4 Tests inspected but not executed

- `tests/integration_smoke_gate.rs` — inspected **and executed; failed**.
- `tests/contract_tests.rs` — inspected **and executed; passed**.
- `src/smoke.rs` unit tests — inspected; selected module executed and passed.
- `tests/integration_untested_commands.rs` — inspected, not executed; it covers agency migration/CRUD/functions/merge/pull/push/resources, not the 11 current lexical gaps.
- `tests/smoke/scenarios/bin_test_target_compiles.sh` — inspected, not executed; its “CI never compiles bin tests” comment is superseded by the completion-canary compile.
- `tests/smoke/scenarios/release_workflow_signing_contract.sh` — inspected and executed; static only.
- `tests/smoke/scenarios/_helpers.sh` — inspected, not executed directly.
- `tests/install/installer_smoke.ps1` — inspected, not executed locally; CI declares a Windows run.
- Federation/review/exec/pilot/TUI/Pi live smoke scenarios — manifest and representative names inspected, not executed.
- Formal Lean files/fixtures and seven Rust conformance targets — structure/CI selection inspected, not executed.
- Remaining Rust targets and ignored tests — counted/name-sampled, not executed exhaustively.

### 7.5 Files providing primary evidence

- `.github/workflows/ci.yml:1-230`
- `.github/workflows/release.yml:1-181`, `191-331`, `391-577`, `579-688`, `697-820`
- `Cargo.toml:1-54`, `155-179`
- `rust-toolchain.toml:1-19`
- `src/main.rs:1261-1275`
- `src/worker_cli.rs:345-360`
- `src/commands/service/ipc.rs:910-918`
- `src/commands/completion_done.rs:32-104`
- `src/commands/done.rs:1583-1678`, `2024-2030`
- `src/smoke.rs:1-17`, `70-140`, `166-240`, `334-439`, `522-768`
- `tests/smoke/README.md:1-103`
- `tests/smoke/manifest.toml:1-17` and scenario blocks named in section 2.4
- `tests/smoke/scenarios/_helpers.sh:1-145`, `290-455`
- `tests/integration_smoke_gate.rs:1-127`, `130-256`, `335-443`
- `tests/install/release_contract.sh:1-64`
- `tests/install/installer_smoke.sh:1-244`
- `tests/smoke/scenarios/release_workflow_signing_contract.sh:1-149`
- `tests/smoke/scenarios/bin_test_target_compiles.sh:1-54`
- `formal/README.md:1-138`
- `worksgood-pi/package.json:1-49`

### 7.6 Limitations

**`[UNCERTAINTY]`** No claim here certifies production correctness, security, signing, formal correctness outside the model, or release readiness. The audit intentionally avoided full smoke (potential 8.77-hour sequential timeout), full Cargo tests, live endpoints, daemons beyond test-local failed setup, browser/TUI automation, package installation into the global environment, identity mutation, and release publication. Repository facts are snapshot-current; executed behavior is limited to the listed Linux environment and inputs.
