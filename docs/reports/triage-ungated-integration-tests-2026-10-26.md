# TRIAGE — the un-gated `tests/*.rs` integration targets (`triage-the-182-un`)

Read-only triage. **No production code, no test code, and no manifest was changed;
nothing was deleted.** Every number below was measured on this machine
(`agent-248` worktree = `main` + zero commits, i.e. the base revision) on
2026-10-26, not estimated.

Predecessor survey: `.wg/survey/survey-tests-docs.md` §2.4 / C2.

---

## 0. TL;DR / recommendation

1. **The suite is not merely un-gated, it is bit-rotted.** Under a CI-like
   environment **36 of the 191 targets are red** — 161 failing test functions.
   Nothing noticed because nothing runs them. The rot is *patterned*: 27 targets
   assert CLI semantics that were **deliberately retired** (`wg done`
   publication-derived completion, the old `wg config init` UX, the removed
   `--executor` flag, the old `GraphError::Io` variant, stale prompt snapshots).
2. **Cost is now known.** Compiling all 191 target binaries costs **525 s** of
   CPU and produces **~50 MB per binary (~19 GB of `target/debug/deps`
   artifacts)**; running them sequentially costs **856 s**; the whole set runs in
   **485 s wall** with default parallelism on 16 cores.
3. **Gating a curated 56-target set is cheap: 42 s wall (warm), 99 s of test
   time, +~130 s of link time.** That is the recommended PR gate.
4. **Do not gate everything on PRs.** 48 further targets are green but either
   slow (>10 s each) or pin planes the daily `pi` workflow never enters
   (nex/native/claude/codex/openrouter handlers, federation/exec sparks,
   adaptive agency). Put those in a **weekly scheduled sweep** so they cannot rot
   silently again.
5. **Retire (operator-approved, one commit) 32 targets:** 27 rot + 5 duplicates.
   **Fold 44 more** (library-only targets restating `cargo test --lib` coverage)
   into a gated target, then retire. Together that is **54,781 LOC** of the
   120,432.
6. **3 repairs first** — they may be *real* regressions the un-gated suite lost
   (`integration_user_board`, `integration_pending_eval_state`,
   `spawn_site_isolation`). Fix/triage those before any deletion so a genuine bug
   is not archived along with the rot.

Nothing here is approved work yet; the plan in §5 needs operator sign-off
(`survey-synthesis` aggregates).

---

## 1. Scope

| | |
|---|---|
| Targets in scope | **191** `tests/*.rs` + `tests/upgrade/main.rs` (a 192nd target the brief's glob misses) |
| LOC | **120,432** |
| `#[test]` functions | **2,640** (2,401 green / 167 red / 91 `#[ignore]`d at runtime) |
| CI-gated today | **8** (`lifecycle-formal`: 7 targets; `integration` job: `integration_service`) |
| Un-gated | **183** (brief said 182/190 — one target has been added since the survey) |
| Last-touched | 2026-04: 1 · 05: 8 · 06: 72 · 07: 54 · 08: 13 · 09: 41 · 10: 2 |

The suite keeps growing while nothing runs it — the opposite of the "cut scope"
direction the operator asked for.

## 2. Method (reproducible)

```bash
# 1. compile cost per target (cargo emits a self-contained timings unit graph)
cargo test --locked --no-run --tests --timings      # 3m03s wall, 440 crates
#    → parsed `const UNIT_DATA` out of target/cargo-timings/cargo-timing-*.html
#      (192 test units; lib test unit alone = 76.5 s; test-attributable total = 525 s)

# 2. run cost + pass/fail per target, one target at a time
for n in $(ls tests/*.rs | xargs -n1 basename | sed 's/\.rs$//'); do
  env -u WG_* HOME=<temp home with a .gitconfig> \
    timeout 300 cargo test --locked --test "$n" -- --test-threads=1
done                                                 # 8m38s total

# 3. every red target re-run twice more:
#    (a) with ALL WG_* worker-authority vars scrubbed  → 3 targets flipped green
#    (b) with a git identity present in the temp HOME → 2 more flipped green
#    → final red set, verified stable across runs

# 4. whole-suite cost in the shape a scheduled job would use
cargo test --locked --tests --no-fail-fast           # WALL 484.67s, 38 red targets
```

**Environment caveats (stated so the numbers are not over-read).** The triage ran
inside a WG worker session, which injects `WG_*` authority into children; a
handful of targets were sensitive to it and to a missing `git` identity. Both
were isolated and removed from the verdict (§3.1). Three targets are red for
*environment* reasons that a CI runner also hits or does not:

| target | why red | verdict |
|---|---|---|
| `simple_land_lean_oracle` | `formal/.lake/build/bin/simple-land-oracle` absent locally; CI's `lifecycle-formal` job runs `lake build simple-land-oracle` first | **not rot** — green in CI by construction |
| `integration_setup_pi_concierge_gates` | `pi` not on `PATH`; the `integration` CI job does not install node/pi either | env, and it is a scenario re-runner (§3.3) — same recommendation either way |
| `integration_pi_opaque_execution_boundary` | argv-contract assertion vs the actual launch plan | **rot** |

Raw data: `/tmp/triage_run/logs/*.log` (per-target), `/tmp/fullsuite.log`
(whole-suite), `triage-the-182-un-classification.tsv` (this directory,
machine-readable), and `Appendix A` below.

## 3. Findings

### 3.1 F1 — 36/191 targets are red at the base revision

Measured, stable set (all re-run twice): `integration_canonical_config(5 red)`,
`integration_config(8)`, `integration_cycle_detection(49)`,
`integration_deep_readonly_flip(6)`, `integration_deprecate_pending_validation(4)`,
`integration_done_uncommitted(3)`, `integration_error_paths(2)`,
`integration_evolver_pipeline(1)`, `integration_executor_arena_surfaces(1)`,
`integration_failed_pending_eval(1)`, `integration_global_config(1)`,
`integration_logging(8)`, `integration_native_coordinator(1)`,
`integration_native_executor_async_web(1)`, `integration_pending_eval_state(2)`,
`integration_pi_opaque_execution_boundary(1)`, `integration_pi_watchdog(1)`,
`integration_project_local_pi_cli(6)`, `integration_provider_model_format(1)`,
`integration_runs_exhaustive(7)`, `integration_scope_guard(1)`,
`integration_service_coordinator(1)`, `integration_setup(11)`,
`integration_setup_pi_concierge_gates(1)`, `integration_strong_merge_resolution(2)`,
`integration_task_lifecycle(2)`, `integration_tier_defaults(5)`,
`integration_trace_function_layers(3)`, `integration_user_board(8)`,
`prompt_snapshots(5)`, `simple_land_lean_oracle(1, env)`,
`spawn_site_isolation(2)`, `test_crash_scenarios(4)`, `test_prompt_logging_debug(1)`,
`test_shell_retry_loop(4)`, `test_verify_timeout_functionality(1)`.
Plus `tests/upgrade` (outside the glob): 2 red, version-string drift.

Two targets were red only because the test HOME had no `git` identity
(`integration_agency_pipeline`, `integration_subtask`); both are **green** in a
CI-like env and are recommended for the PR gate.

### 3.2 F2 — the rot is patterned around *deliberate* retirements

This is the important finding: it is not random flakiness. Each cluster has a
citable cause:

| cluster | targets | verbatim evidence |
|---|---|---|
| `wg done` publication-derived completion | `integration_cycle_detection` (49), `integration_logging` (8), `integration_task_lifecycle` (2), `integration_done_uncommitted` (3), `test_verify_timeout_functionality` (1) | `Error: legacy wg done bypass/merge/cycle flags are not supported by publication-derived completion` · `disposition: Repairing / command: git diff --check refs/heads/main..HEAD / exit category: contract-precondition` |
| project-local config UX | `integration_setup` (11), `integration_config` (8), `integration_canonical_config` (5), `integration_tier_defaults` (5), `integration_global_config` (1), `integration_provider_model_format` (1) | `worksgood.toml already exists; pass --force` · `Expected 'local' source annotation … got [project-file]` · `wg profile use codex --no-reload failed` |
| `--executor` flag + executor arena retired | `integration_scope_guard`, `test_prompt_logging_debug`, `integration_executor_arena_surfaces` | `error: invalid value 'claude' for '--executor <EXECUTOR>'` · `assertion failed: config.contains("owner = \"Pi\"")` |
| error-enum drift | `integration_error_paths` | `Error should be IO variant, got: IoPath { op: "open graph", … }` |
| prompt drift | `prompt_snapshots` | insta snapshot mismatch on `build_prompt_full` |
| launch-plan contract | `integration_pi_watchdog`, `integration_pi_opaque_execution_boundary` | `WG-OPAQUE-LAUNCH-PLAN-INCOMPLETE: managed RPC fixed argv does not match the assignment contract` |
| agency vocabulary | `integration_evolver_pipeline` | `could not propagate eval to component 'rust': No component matching 'rust'` |

Consequence: these files are not "failing tests to fix" — they are **assertions
of contract versions that were intentionally deleted from the binary**. Rewriting
them is new test authoring, not repair. That is the argument for retiring rather
than gating them.

### 3.3 F3 — real duplication at three levels

1. **One target includes two other test files**:
   `integration_project_local_pi_cli.rs` `#[path]`-includes
   `integration_deep_readonly_flip.rs` and
   `integration_dedicated_pi_evaluation.rs` — so those 6 reds are counted twice
   in `cargo test`, and a "fix" must be made in two places.
2. **10 exact-name twins** with the shell smoke suite: `agency_csv_roundtrip`,
   `concierge_operator_framing`, `executor_arena_surfaces`,
   `explicit_execution_selection`, `nex_streaming_resilience`,
   `pi_sole_model_plane`, `pi_two_tier_profile`, `project_local_pi_cli`,
   `project_local_pi_migrate`, `setup_pi_concierge_gates` (+21 fuzzy matches).
   Neither layer is CI-gated, so today there are *two* un-run copies of the same
   contract.
3. **Sibling clusters covering one surface**, which is where most of the LOC is:

| cluster | targets | LOC |
|---|---|---|
| trace / runs / replay | `integration_trace_functions`, `integration_trace_exhaustive`, `integration_trace_function_layers`, `integration_trace_replay`, `integration_replay_exhaustive`, `integration_runs_exhaustive` | 9,383 |
| agency | 12 `integration_agency_*` + `adaptive_*` 8 | ~10,500 |
| config / profile / setup | `integration_config`, `_global_config`, `_canonical_config`, `_tier_defaults`, `_setup`, `_setup_routes`, `_provider_model_format`, `_project_local_pi_*` (3) | 5,705 |
| openrouter / handler-specific | `integration_openrouter{,_flow,_smoke}`, `smoke_openrouter_*` (3), `codex_handler_oai_compat`, `integration_anthropic_streaming`, `integration_init_codex_runtime` | 6,567 |
| nex / native executor | `test_nex`, `integration_nex_*` (2), `integration_native_*` (3), `smoke_native_executor`, `integration_dual_executor` | 7,367 |
| "exhaustive"-named | `*_exhaustive` (3) | 6,147 |

The five `*_exhaustive`/`_layers` targets implement
`docs/test-specs/trace-replay-test-spec.md` (§3.1–3.23) — that spec is the real
deliverable; the five near-identical executors of it are not.

### 3.4 F4 — library-only targets duplicate the gated unit layer

**44 targets never spawn a process** (`mode=lib`, 0 spawn sites): 24,351 LOC,
588 test fns, green, and they assert library behaviour that `cargo test --lib`
(7,325 unit tests, CI-gated) already owns. Examples:
`test_edit_file_edge_cases` (887 L, 35 tests), `integration_agency_federation`
(2,566 L, 78 tests), `integration_model_management` (1,705 L, 51 tests),
`integration_context_scope` (615 L), `integration_session_summary`,
`test_context_pressure_agent`. These are the cheapest retirement (no CLI surface
is lost) **if** a per-case dedupe pass against `src/**` unit tests confirms
nothing unique is dropped. Note name-level duplication is *low* (26 test-fn names
collide with `src/` names), so the dedupe must be done case-by-case — I did not
do it, and it is the one place where a careless cut could lose real coverage.

### 3.5 F5 — parallel-unsafe targets

Only **4 targets** use `tests/common/isolated_cli.rs`, the harness written
precisely to stop a test-runner's WG authority/HOME leaking into child `wg`
processes. The other process-spawning targets inherit whatever `HOME`/`WG_*` the
runner has: 3 flipped red↔green purely on environment (`integration_subtask`,
`integration_agency_pipeline`, `integration_native_executor_async_web`). If the
suite is gated, those should adopt `isolated_cli` rather than be debugged in CI.

### 3.6 F6 — cost table (measured)

| measurement | value |
|---|---|
| compile all 192 test units (CPU seconds) | **525 s** (lib test unit alone 76.5 s) |
| per-target compile | 0.26 s – 7.42 s (median ~2.7 s, dominated by linking) |
| artifact per test binary | **~50 MB**; `target/debug/deps` = **22 GB** after one full build |
| run all 191 targets, `--test-threads=1` (sum) | **856 s** |
| run whole suite, default parallelism, 16 cores | **485 s wall** |
| slowest targets | `integration_smoke_gate` 81 s, `test_crash_scenarios` 66 s, `integration_messaging` 57 s, `integration_service` 50 s, `smoke_openrouter_errors` 35 s, `codex_handler_oai_compat` 34 s, `integration_resume` 33 s |
| recommended 56-target PR gate (measured as one command) | **42.1 s wall** warm (99 s sequential) |
| 8 currently-gated targets | 61 s sequential |

A GitHub `ubuntu-latest` runner has ~4 vCPU, so expect roughly 3–4× the wall
numbers above for full-suite shapes; the 56-target gate should still land under
~2 minutes.

### 3.7 F7 — adjacent gap found while measuring (out of scope, worth a task)

CI's build job runs `cargo test --bin worksgood`, **not** `cargo test --bin wg`.
`cargo test --bin wg` — the *actual* `wg` binary's unit tests — is
**3,882 passed / 42 failed** on the base revision in this environment (all in
`commands::completion_land::tests` / `commands::completion_submit::tests`). I did
not chase whether those need a git fixture like the two integration targets did,
so treat it as "unverified gap", not "42 bugs". Same class of problem as this
task: a test target no job runs.

---

## 4. Classification (all 191 targets, one row each — full table in Appendix A)

| class | n | LOC | test fns | red | run (seq) | meaning |
|---|---:|---:|---:|---:|---:|---|
| `keep-gated` | 8 | 4,216 | 59 | 1 (env) | 61 s | already in CI; leave alone |
| `wire-pr` | 56 | 31,223 | 731 | 0 | 99 s | green, drives the real binary, ≤10 s each → add to a PR job |
| `wire-nightly` | 48 | 29,092 | 512 | 0 | 444 s | green but slow and/or pins a plane the daily `pi` workflow does not enter → scheduled sweep |
| `fold` | 44 | 24,351 | 588 | 0 | 59 s | library-only restatement of gated `--lib` coverage → fold non-duplicate cases into a gated target, then retire the file |
| `repair` | 3 | 1,120 | 23 | 3 | 4 s | red, but pins behaviour that may still be intended → triage as possible real bugs **first** |
| `retire-rot` | 27 | 27,972 | 692 | 27 | 116 s | red against deliberately retired semantics (§3.2); rewriting = new test authoring |
| `retire-dup` | 5 | 2,458 | 35 | 5 | 74 s | red wrapper/duplicate of a maintained layer (shell scenario, hardcap variant, live-coordinator test) |
| **total** | **191** | **120,432** | **2,640** | **36** | **856 s** | |

## 5. Recommendation (needs operator approval)

**Phase 0 — freeze (no deletions).** Land this triage as the baseline. Add the
three `repair` targets to the next triage round; do not touch them yet.

**Phase 1 — PR gate `integration-targets` (new job, ~2 min).** Run exactly the
56 `wire-pr` targets. Two runner details that matter:

* `cargo build --locked --bins` first — the targets locate the binary via
  `current_exe()/..`, not via `PATH` (only the `integration` job's isolated
  install root puts `wg` on `PATH` today).
* Do **not** set a custom `CARGO_TARGET_DIR`: 3 targets hardcode
  `$CARGO_MANIFEST_DIR/target/debug/wg`
  (`integration_executor_arena_surfaces`, `integration_nex_entrypoint`,
  `integration_simplify_executor_taxonomy`). Fix those 3 to `CARGO_BIN_EXE_wg`
  if a shared target dir is ever wanted.

```yaml
  integration-targets:
    name: Integration targets (curated)
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/cache@v4          # same cargo cache block as the other jobs
      - run: cargo build --locked --bins
      - name: curated integration targets
        run: >
          cargo test --locked
          --test integration_cli_workflows --test integration_cli_endpoints
          # … the 56 `wire-pr` rows from Appendix A …
          -- --test-threads=1
```

**Phase 2 — weekly sweep (new scheduled job).** Add the 48 `wire-nightly`
targets behind `on: schedule` (e.g. `cron: '0 4 * * 1'`) with
`cargo test --locked --tests --no-fail-fast` (485 s wall on 16 cores, ~30 min on
a 4-vCPU runner) and, later, `continue-on-error: false` once the retire list is
gone. This is the guard that stops rot: *every* target then runs at least weekly,
which is the property the repo has never had.

**Phase 3 — repairs (3 targets, cheap and possibly real bugs).**
`integration_user_board` (8 red — `ensure_user_board` now leaves the board task
`Open` where the test expects `InProgress`), `integration_pending_eval_state`
(2 red — TUI chartreuse RGB drift; `.evaluate-*` not unblocked from a
`PendingEval` source even though `Status::PendingEval` still exists in
`src/commands/graph.rs`), `spawn_site_isolation` (2 red — a **source-layout
lint** whose `EXPECTED_SITES` needs the new `plan_spawn(` call site; fix is
~1 line and it protects the dispatch plane). Keep these three in the PR gate
whichever way the triage lands.

**Phase 4 — retire 32 (operator-approved, one commit, `git rm` only).**
27 `retire-rot` + 5 `retire-dup`: **30,430 LOC, 727 test fns, 190 s of run time**.
Recommended commit shape: delete the files and the `--test` selectors only — do
not rewrite them into passing shape. If any assertion in them still holds, it
must be re-expressed against the current contract, which is a separate task.

**Phase 5 — fold 44 (24,351 LOC).** Move the still-unique cases from each
`fold` target into a gated target (most naturally `integration_service`, or the
matching `wire-pr` sibling), *then* delete the file. This is the only phase where
careless cutting loses coverage: do it file by file, diffing test-fn names against
`src/**` unit tests.

Net effect if all phases land: **191 → ~111 target binaries**, **120,432 →
~66,000 LOC** of test source, **~525 s → ~250 s** of link time, and — the actual
prize — **every remaining target runs on every PR or every week**, so the next
retirement of a CLI contract cannot silently orphan 700 assertions.

**Explicitly rejected:** gate all 191 on PRs. It is 3–4× the wall time of the
curated set on top of the existing `integration_service` job, and 44 of those
targets duplicate `--lib` coverage that is already gated.

## 6. What I did NOT do (scope discipline)

* No file was deleted, renamed, edited, or added in the repository.
* No `--validation-command`, smoke manifest, or CI workflow was changed.
* No cleanup beyond measurement: `target/` and the temp HOMEs are throwaway.
* I did not convert any `retire`/`fold` decision into an approved action, and I
  did not publish the follow-up tasks (§7 are drafts only) — the brief requires
  operator approval first.

## 7. Follow-ups proposed (drafts, unpublished)

1. **`wire-ungated-integration-pr-gate`** — add the `wire-pr` CI job (§5 Phase 1)
   plus the 3 `CARGO_BIN_EXE_wg` fixes.
2. **`retire-rotted-integration-targets`** — Phase 4 deletion, blocked on
   operator approval of this classification.
3. **`audit-bin-wg-unit-tests`** — F7: determine whether the 42 red
   `--bin wg` unit tests are a real gap or a missing git fixture, and gate the
   target either way.

## Appendix A — full per-target classification (all 191 `tests/*.rs` targets)

`mode=proc` means the target spawns a process (real `wg`, `git`, or a shell); `mode=lib`
means it only calls library APIs. `run` is the measured wall time of
`cargo test --test <name> -- --test-threads=1` on a warm build. `status` is green/red
under the CI-like environment described in §2.


### `keep-gated` — 8 targets, 4,216 LOC

| target | LOC | test fns | mode | status | run | evidence |
|---|---:|---:|---|---|---:|---|
| `completion_manifest_resolver` | 413 | 12 | proc | green | 1.3s | CI `lifecycle-formal` job; green (1.3s). |
| `completion_review_valve` | 967 | 13 | lib | green | 3.4s | CI `lifecycle-formal` job; green (3.4s). |
| `completion_task_projection` | 145 | 3 | lib | green | 1.1s | CI `lifecycle-formal` job; green (1.1s). |
| `integration_service` | 1710 | 19 | proc | green | 50.1s | CI `integration` job; green (50.1s) — the daemon/IPC surface. |
| `legacy_completion_authority_retired` | 162 | 4 | proc | green | 1.2s | CI `lifecycle-formal` job; green (1.2s). |
| `lifecycle_protocol_conformance` | 510 | 5 | lib | green | 1.2s | CI `lifecycle-formal` job already runs it; green here (1.2s). |
| `simple_land_conformance` | 101 | 2 | lib | green | 1.1s | CI `lifecycle-formal` job; green (1.1s). |
| `simple_land_lean_oracle` | 208 | 1 | proc | red | 1.1s | CI `lifecycle-formal` job builds the Lean oracle first; red locally only because `formal/.lake/build/bin/simple-land-oracle` is absent (env, not rot). |

### `wire-pr` — 56 targets, 31,223 LOC

| target | LOC | test fns | mode | status | run | evidence |
|---|---:|---:|---|---|---:|---|
| `contract_tests` | 438 | 34 | lib | green | 1.2s | cheap structural guard (438L, 1.2s, 0 spawn sites). |
| `flip_role_model_routing` | 262 | 3 | proc | green | 1.0s | green, drives the real `wg` binary (7 spawn sites, subcommands: evaluate), 1.0s. |
| `integration_accidental_cycle` | 439 | 6 | proc | green | 1.5s | green, drives the real `wg` binary (21 spawn sites, subcommands: add,edit,publish,ready), 1.5s. |
| `integration_agency_csv_roundtrip` | 91 | 2 | proc | green | 0.5s | green, drives the real `wg` binary (3 spawn sites, subcommands: n/a), 0.5s. |
| `integration_agency_hash` | 238 | 3 | proc | green | 1.1s | green, drives the real `wg` binary (5 spawn sites, subcommands: agency), 1.1s. |
| `integration_agency_import` | 528 | 7 | proc | green | 0.6s | green, drives the real `wg` binary (14 spawn sites, subcommands: agency), 0.6s. |
| `integration_agency_pipeline` | 1175 | 39 | proc | green | 2.6s | GREEN once the test HOME has a git identity (34 pass / 5 ignored, 2.5s). It was red in the first pass only because `git commit` in the fixture had no user.name. Kept as a PR-gate candidate: it is the only target that drives `wg agency` end to end. |
| `integration_agency_stats` | 365 | 8 | proc | green | 1.3s | green, drives the real `wg` binary (14 spawn sites, subcommands: agency), 1.3s. |
| `integration_analytics` | 481 | 20 | proc | green | 1.4s | green, drives the real `wg` binary (29 spawn sites, subcommands: aging,coordinate,forecast,velocity,workload), 1.4s. |
| `integration_atomic_writes` | 166 | 4 | lib | green | 1.1s | cheap structural guard (166L, 1.1s, 0 spawn sites). |
| `integration_bare_alias_contract` | 59 | 2 | lib | green | 1.3s | cheap structural guard (59L, 1.3s, 0 spawn sites). |
| `integration_broken_pipe` | 110 | 1 | proc | green | 1.1s | green, drives the real `wg` binary (6 spawn sites, subcommands: show), 1.1s. |
| `integration_candidate_finalization` | 584 | 10 | proc | green | 4.4s | green, drives the real `wg` binary (3 spawn sites, subcommands: cat-file), 4.4s. |
| `integration_cli_endpoints` | 1130 | 25 | proc | green | 2.5s | green, drives the real `wg` binary (67 spawn sites, subcommands: endpoints), 2.5s. |
| `integration_cli_workflows` | 1454 | 72 | proc | green | 4.4s | green, drives the real `wg` binary (86 spawn sites, subcommands: abandon,add,agents,analyze,archive,blocked,bottlenecks,check,claim,context), 4.4s. |
| `integration_coordinator_agent` | 1047 | 13 | proc | green | 1.2s | green, drives the real `wg` binary (23 spawn sites, subcommands: add,chat,init,list,service,show), 1.2s. |
| `integration_coordinator_spawn_template` | 638 | 7 | proc | green | 1.1s | green, drives the real `wg` binary (6 spawn sites, subcommands: chat,init), 1.1s. |
| `integration_cron_dispatch` | 516 | 13 | proc | green | 1.5s | green, drives the real `wg` binary (20 spawn sites, subcommands: add,edit,list), 1.5s. |
| `integration_dev_check` | 41 | 1 | proc | green | 0.5s | green, drives the real `wg` binary (2 spawn sites, subcommands: n/a), 0.5s. |
| `integration_dispatch_boot` | 444 | 8 | proc | green | 1.0s | green, drives the real `wg` binary (7 spawn sites, subcommands: init,service), 1.0s. |
| `integration_explicit_execution_selection` | 78 | 1 | proc | green | 3.2s | green, drives the real `wg` binary (0 spawn sites, subcommands: n/a), 3.2s. |
| `integration_failure_classification` | 351 | 9 | proc | green | 1.4s | green, drives the real `wg` binary (5 spawn sites, subcommands: classify-failure,fail,show), 1.4s. |
| `integration_handler_stdout_pristine` | 185 | 2 | proc | green | 3.4s | green, drives the real `wg` binary (9 spawn sites, subcommands: init,status), 3.4s. |
| `integration_init` | 202 | 5 | proc | green | 1.7s | green, drives the real `wg` binary (3 spawn sites, subcommands: n/a), 1.7s. |
| `integration_login_openrouter` | 293 | 1 | proc | green | 0.6s | green, drives the real `wg` binary (7 spawn sites, subcommands: login,models), 0.6s. |
| `integration_multi_user_watcher` | 326 | 7 | proc | green | 4.5s | green, drives the real `wg` binary (0 spawn sites, subcommands: n/a), 4.5s. |
| `integration_phantom_edge` | 493 | 17 | proc | green | 1.5s | green, drives the real `wg` binary (29 spawn sites, subcommands: add,edit,publish,why-blocked), 1.5s. |
| `integration_pi_watchdog_hardcap` | 326 | 2 | proc | green | 3.3s | green, drives the real `wg` binary (15 spawn sites, subcommands: claim,pi-stream-observe,pi-watchdog,retry), 3.3s. |
| `integration_placement` | 488 | 12 | proc | green | 1.4s | green, drives the real `wg` binary (3 spawn sites, subcommands: apply-placement), 1.4s. |
| `integration_project_local_pi_migrate` | 660 | 9 | proc | green | 1.7s | green, drives the real `wg` binary (5 spawn sites, subcommands: n/a), 1.7s. |
| `integration_reap_kill_tree` | 650 | 14 | proc | green | 1.5s | green, drives the real `wg` binary (21 spawn sites, subcommands: kill,reap), 1.5s. |
| `integration_remove_validation_cli` | 273 | 6 | proc | green | 3.7s | green, drives the real `wg` binary (18 spawn sites, subcommands: add,edit,init), 3.7s. |
| `integration_replay_exhaustive` | 2797 | 79 | proc | green | 3.8s | green, drives the real `wg` binary (80 spawn sites, subcommands: replay,runs,trace), 3.8s. |
| `integration_retire_compact_archive` | 266 | 5 | proc | green | 1.2s | green, drives the real `wg` binary (3 spawn sites, subcommands: n/a), 1.2s. |
| `integration_scheduled_dispatch` | 936 | 27 | proc | green | 1.9s | green, drives the real `wg` binary (42 spawn sites, subcommands: add,edit,list,ready,show), 1.9s. |
| `integration_service_control_permissions` | 116 | 3 | proc | green | 0.5s | green, drives the real `wg` binary (7 spawn sites, subcommands: service), 0.5s. |
| `integration_setup_routes` | 729 | 24 | proc | green | 1.5s | green, drives the real `wg` binary (7 spawn sites, subcommands: init,setup), 1.5s. |
| `integration_subtask` | 368 | 7 | proc | green | 1.4s | GREEN once the test HOME has a git identity (7 pass, 0.3s). Was red only under the worker-authority/git-identity-less environment. Covers `wg add --subtask` child creation + parent parking. |
| `integration_trace_exhaustive` | 1941 | 45 | proc | green | 2.5s | green, drives the real `wg` binary (33 spawn sites, subcommands: trace), 2.5s. |
| `integration_trace_functions` | 2056 | 64 | proc | green | 2.0s | green, drives the real `wg` binary (44 spawn sites, subcommands: func,trace), 2.0s. |
| `integration_trace_replay` | 675 | 15 | proc | green | 1.7s | green, drives the real `wg` binary (24 spawn sites, subcommands: replay,runs,trace), 1.7s. |
| `integration_triage` | 364 | 7 | proc | green | 1.2s | green, drives the real `wg` binary (1 spawn sites, subcommands: n/a), 1.2s. |
| `integration_worktree` | 456 | 8 | proc | green | 2.2s | green, drives the real `wg` binary (12 spawn sites, subcommands: add,branch,commit,config,init,merge,worktree), 2.2s. |
| `integration_worktree_observer` | 956 | 16 | proc | green | 3.1s | green, drives the real `wg` binary (4 spawn sites, subcommands: hash-object,rev-parse), 3.1s. |
| `integration_yaml_graceful_degradation` | 125 | 6 | lib | green | 1.0s | cheap structural guard (125L, 1.0s, 0 spawn sites). |
| `legacy_local_lifecycle_compat` | 47 | 1 | lib | green | 1.0s | cheap structural guard (47L, 1.0s, 0 spawn sites). |
| `recovery_classification_evidence` | 89 | 1 | lib | green | 1.0s | cheap structural guard (89L, 1.0s, 0 spawn sites). |
| `save_transaction_conformance` | 228 | 2 | lib | green | 1.2s | cheap structural guard (228L, 1.2s, 0 spawn sites). |
| `smoke_context` | 678 | 12 | proc | green | 1.4s | green, drives the real `wg` binary (13 spawn sites, subcommands: add,agency,endpoint,init,show,spawn), 1.4s. |
| `terminology_lint` | 119 | 4 | lib | green | 0.5s | cheap structural guard (119L, 0.5s, 0 spawn sites). |
| `test_concurrent_head_reference` | 344 | 3 | proc | green | 1.1s | green, drives the real `wg` binary (17 spawn sites, subcommands: add,branch,commit,config,init,log,rev-parse,status,worktree), 1.1s. |
| `test_edge_cases` | 890 | 14 | proc | green | 0.9s | green, drives the real `wg` binary (11 spawn sites, subcommands: add,branch,commit,config,init,worktree), 0.9s. |
| `test_orphaned_cleanup` | 738 | 5 | proc | green | 1.7s | green, drives the real `wg` binary (9 spawn sites, subcommands: add,branch,commit,config,init,worktree), 1.7s. |
| `test_race_conditions` | 756 | 5 | proc | green | 0.9s | green, drives the real `wg` binary (11 spawn sites, subcommands: add,branch,commit,config,init,worktree), 0.9s. |
| `test_recovery_verification` | 771 | 8 | proc | green | 0.9s | green, drives the real `wg` binary (21 spawn sites, subcommands: add,branch,commit,config,init,log,show,worktree), 0.9s. |
| `test_verify_timeout_basic` | 247 | 7 | proc | green | 5.3s | green, drives the real `wg` binary (2 spawn sites, subcommands: add,init,list), 5.3s. |

### `wire-nightly` — 48 targets, 29,092 LOC

| target | LOC | test fns | mode | status | run | evidence |
|---|---:|---:|---|---|---:|---|
| `adaptive_accounting` | 63 | 1 | lib | green | 1.2s | green but 1.2s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `adaptive_agency_capabilities` | 71 | 2 | proc | green | 1.2s | green but 1.2s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `adaptive_agency_replay` | 110 | 1 | lib | green | 1.0s | green but 1.0s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `adaptive_assignment_reward` | 235 | 5 | lib | green | 1.6s | green but 1.6s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `adaptive_candidate_ledger` | 220 | 3 | lib | green | 1.3s | green but 1.3s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `adaptive_learning_episode` | 118 | 2 | lib | green | 1.1s | green but 1.1s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `adaptive_outcome` | 67 | 1 | lib | green | 1.1s | green but 1.1s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `adaptive_performance_projection` | 37 | 1 | lib | green | 1.0s | green but 1.0s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `codex_handler_oai_compat` | 270 | 3 | proc | green | 34.1s | green but 34.1s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_agency_federation` | 2566 | 78 | lib | green | 2.2s | green but 2.2s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_anthropic_streaming` | 640 | 9 | lib | green | 1.2s | green but 1.2s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_chat` | 522 | 9 | proc | green | 25.0s | green but 25.0s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_concierge_operator_framing` | 109 | 1 | proc | green | 0.6s | green but 0.6s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_cross_repo_dispatch` | 1383 | 30 | proc | green | 1.5s | green but 1.5s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_dedicated_pi_evaluation` | 740 | 9 | proc | green | 3.1s | green but 3.1s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_deprecated_provider_flag` | 231 | 8 | proc | green | 1.0s | green but 1.0s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_dispatcher_config_roundtrip` | 459 | 3 | proc | green | 19.3s | green but 19.3s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_dual_executor` | 1184 | 8 | lib | green | 4.1s | green but 4.1s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_e2e_smoke` | 399 | 3 | proc | green | 8.8s | green but 8.8s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_exec` | 534 | 17 | proc | green | 1.4s | green but 1.4s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_fed_wire` | 219 | 7 | lib | green | 1.1s | green but 1.1s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_heartbeat` | 302 | 3 | lib | green | 4.4s | green but 4.4s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_init_codex_runtime` | 279 | 1 | proc | green | 0.4s | green but 0.4s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_lazy_evaluation` | 371 | 6 | proc | green | 8.5s | green but 8.5s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_llm_autopoiesis` | 748 | 6 | proc | green | 0.4s | green but 0.4s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_messaging` | 1276 | 36 | proc | green | 57.3s | green but 57.3s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_multi_coordinator` | 516 | 7 | proc | green | 18.3s | green but 18.3s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_native_executor` | 348 | 12 | lib | green | 1.2s | green but 1.2s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_nex_entrypoint` | 1142 | 9 | proc | green | 7.0s | green but 7.0s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_nex_streaming_resilience` | 399 | 0 | lib | green | 7.3s | green but 7.3s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_openrouter` | 813 | 4 | proc | green | 0.6s | green but 0.6s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_openrouter_flow` | 1316 | 41 | lib | green | 1.4s | green but 1.4s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_openrouter_smoke` | 1230 | 26 | proc | green | 1.5s | green but 1.5s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_provider` | 971 | 24 | lib | green | 1.4s | green but 1.4s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_resume` | 859 | 7 | lib | green | 32.7s | green but 32.7s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_simplify_executor_taxonomy` | 230 | 6 | proc | green | 1.0s | green but 1.0s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_smoke_gate` | 383 | 9 | proc | green | 81.3s | 81s: re-runs smoke-gate scenarios end to end; keep, but only in a scheduled sweep (too slow for a PR gate). |
| `integration_triage_smoke` | 683 | 4 | proc | green | 11.5s | green but 11.5s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_tui_perf_benchmarks` | 183 | 2 | lib | green | 4.7s | green but 4.7s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `integration_untested_commands` | 1231 | 30 | proc | green | 27.6s | green but 27.6s — too slow for the PR job. |
| `llm_integration` | 510 | 7 | proc | green | 1.1s | green but 1.1s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `mock_executor_integration` | 561 | 17 | proc | green | 1.4s | green but 1.4s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `smoke_model_pipeline` | 1060 | 18 | proc | green | 1.2s | green but 1.2s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `smoke_native_executor` | 370 | 3 | proc | green | 1.3s | green but 1.3s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `smoke_openrouter_errors` | 590 | 11 | proc | green | 34.5s | green but 34.5s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `smoke_openrouter_routing` | 538 | 10 | proc | green | 1.2s | green but 1.2s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `smoke_openrouter_tool_loop` | 221 | 2 | proc | green | 0.4s | green but 0.4s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |
| `test_nex` | 1785 | 10 | lib | green | 21.9s | green but 21.9s and/or pins a plane the daily pi workflow does not enter; scheduled sweep only. |

### `fold` — 44 targets, 24,351 LOC

| target | LOC | test fns | mode | status | run | evidence |
|---|---:|---:|---|---|---:|---|
| `agency_schema_fields` | 197 | 5 | lib | green | 1.0s | library-only (197L, 5 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `daemon_planner_conformance` | 467 | 8 | lib | green | 1.2s | library-only (467L, 8 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `evaluation_recording` | 1067 | 30 | lib | green | 1.4s | library-only (1067L, 30 tests, 1.4s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_agency` | 1142 | 5 | lib | green | 1.4s | library-only (1142L, 5 tests, 1.4s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_agency_edge_cases` | 956 | 45 | lib | green | 1.2s | library-only (956L, 45 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_agency_lineage` | 640 | 25 | lib | green | 1.2s | library-only (640L, 25 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_agency_loop` | 808 | 12 | lib | green | 1.3s | library-only (808L, 12 tests, 1.3s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_agency_scope_rules` | 296 | 10 | lib | green | 1.0s | library-only (296L, 10 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_auto_evolver` | 599 | 9 | lib | green | 1.6s | library-only (599L, 9 tests, 1.6s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_chat_rename` | 264 | 9 | lib | green | 1.2s | library-only (264L, 9 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_check_context` | 318 | 12 | lib | green | 1.2s | library-only (318L, 12 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_cli_commands` | 399 | 15 | lib | green | 1.2s | library-only (399L, 15 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_context_pressure` | 263 | 3 | lib | green | 1.1s | library-only (263L, 3 tests, 1.1s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_context_scope` | 615 | 21 | lib | green | 1.1s | library-only (615L, 21 tests, 1.1s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_error_recovery` | 600 | 8 | lib | green | 1.6s | library-only (600L, 8 tests, 1.6s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_failure_injection` | 530 | 11 | lib | green | 1.6s | library-only (530L, 11 tests, 1.6s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_html` | 1912 | 45 | lib | green | 1.4s | library-only (1912L, 45 tests, 1.4s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_journal` | 569 | 6 | lib | green | 1.5s | library-only (569L, 6 tests, 1.5s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_last_interaction_at` | 321 | 14 | lib | green | 1.4s | library-only (321L, 14 tests, 1.4s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_model_management` | 1705 | 51 | lib | green | 1.8s | library-only (1705L, 51 tests, 1.8s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_pi_sole_model_plane` | 201 | 8 | lib | green | 1.0s | library-only (201L, 8 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_pi_two_tier_profile` | 230 | 6 | lib | green | 1.0s | library-only (230L, 6 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_profile_tier_pinning` | 177 | 6 | lib | green | 1.0s | library-only (177L, 6 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_project_local_pi_config` | 388 | 7 | lib | green | 1.2s | library-only (388L, 7 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_safety_interval_alias` | 138 | 8 | lib | green | 1.0s | library-only (138L, 8 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_self_healing` | 687 | 15 | lib | green | 1.2s | library-only (687L, 15 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_session_summary` | 439 | 12 | lib | green | 1.3s | library-only (439L, 12 tests, 1.3s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_special_agents` | 572 | 7 | lib | green | 3.6s | library-only (572L, 7 tests, 3.6s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_state_injection` | 784 | 8 | lib | green | 1.7s | library-only (784L, 8 tests, 1.7s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_streaming` | 571 | 15 | lib | green | 1.1s | library-only (571L, 15 tests, 1.1s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `integration_tool_parallelism` | 688 | 15 | lib | green | 1.9s | library-only (688L, 15 tests, 1.9s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `skill_resolution` | 257 | 12 | lib | green | 1.1s | library-only (257L, 12 tests, 1.1s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `smoke_no_org_eval` | 542 | 8 | lib | green | 1.6s | library-only (542L, 8 tests, 1.6s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_context_pressure_agent` | 1075 | 32 | lib | green | 1.5s | library-only (1075L, 32 tests, 1.5s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_coordinator_lifecycle` | 545 | 8 | lib | green | 1.1s | library-only (545L, 8 tests, 1.1s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_cron_integration` | 361 | 6 | lib | green | 1.2s | library-only (361L, 6 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_cron_serialization` | 151 | 1 | lib | green | 1.0s | library-only (151L, 1 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_edit_file_edge_cases` | 887 | 35 | lib | green | 1.2s | library-only (887L, 35 tests, 1.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_edit_file_fix_build_repro` | 163 | 2 | lib | green | 1.0s | library-only (163L, 2 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_prompt_from_components` | 761 | 13 | lib | green | 2.2s | library-only (761L, 13 tests, 2.2s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_provider_health` | 527 | 11 | lib | green | 1.1s | library-only (527L, 11 tests, 1.1s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_streaming_agent_loop` | 265 | 3 | lib | green | 1.4s | library-only (265L, 3 tests, 1.4s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_unconfigured_cycle_breakin` | 186 | 3 | lib | green | 1.0s | library-only (186L, 3 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |
| `test_verify_lint_integration` | 88 | 3 | lib | green | 1.0s | library-only (88L, 3 tests, 1.0s); asserts library behaviour that `cargo test --lib` already owns — fold any non-duplicate case into a gated target, then retire the file. |

### `repair` — 3 targets, 1,120 LOC

| target | LOC | test fns | mode | status | run | evidence |
|---|---:|---:|---|---|---:|---|
| `integration_pending_eval_state` | 350 | 9 | proc | red | 1.2s | 2/9 red: (a) the TUI no longer paints `PendingEval` in chartreuse RGB(140,230,80); (b) `.evaluate-*` dependents are not unblocked from a PendingEval source. `Status::PendingEval` still exists in src/commands/graph.rs, so one of these two is plausibly a live regression the ungated suite silently lost — 2 minutes to check. |
| `integration_user_board` | 487 | 12 | proc | red | 1.4s | 8/12 red: `ensure_user_board` now leaves the board task `Open` where the test expects `InProgress`. Cheap to check whether that status change was intentional; if not it is a real regression the ungated suite silently lost. |
| `spawn_site_isolation` | 283 | 2 | lib | red | 1.1s | 2/2 red: a source-layout guard whose `EXPECTED_SITES` list needs the new `plan_spawn(` call site registered. Cheap fix, real value (it is a lint on the dispatch plane). |

### `retire-rot` — 27 targets, 27,972 LOC

| target | LOC | test fns | mode | status | run | evidence |
|---|---:|---:|---|---|---:|---|
| `integration_canonical_config` | 548 | 15 | proc | red | 1.5s | 5/15 red: `wg config init` now requires `--route`; tests drive the pre-route form. |
| `integration_config` | 318 | 16 | proc | red | 1.5s | 8/16 red: asserts the retired config UX (`wg config --init` writing `config.toml`, `[local]` source label). `worksgood.toml`/project-local-pi-config replaced it. |
| `integration_cycle_detection` | 6808 | 174 | proc | red | 12.6s | 49/174 red: `wg done --converged` and the other cycle/bypass flags are now rejected ("legacy wg done bypass/merge/cycle flags are not supported by publication-derived completion"). 6808 LOC, most expensive target to keep rewriting. |
| `integration_deep_readonly_flip` | 1092 | 10 | proc | red | 2.1s | 6/10 red: deep-flip budget assertions + `Option::unwrap()` on None — the flip budget/verdict plumbing changed. |
| `integration_deprecate_pending_validation` | 389 | 11 | proc | red | 1.1s | 4/11 red: pins the pre-deprecation pending-validation readiness ("B should be blocked while .evaluate-A is InProgress"); the file exists to record that deprecation. |
| `integration_done_uncommitted` | 304 | 3 | proc | red | 1.1s | 3/3 red: pins the pre-contract `wg done` NoCommits path that the completion contract now gates. |
| `integration_error_paths` | 847 | 36 | lib | red | 1.3s | 2/36 red: matches the old `GraphError::Io` variant; the enum is now `IoPath{op,code,path}`. |
| `integration_evolver_pipeline` | 1154 | 14 | proc | red | 4.2s | 1/14 red: asserts agency components named `rust` / `code-review` / outcomes `Working code`; the seeded agency vocabulary changed. |
| `integration_executor_arena_surfaces` | 59 | 1 | proc | red | 0.5s | 1/1 red: asserts the retired executor-arena config (`owner = "Pi"`), which no longer exists. |
| `integration_failed_pending_eval` | 438 | 15 | proc | red | 1.9s | 1/15 red: expects the retired `.evaluate-*` system-bypass readiness ("No tasks ready"). |
| `integration_global_config` | 793 | 25 | lib | red | 1.2s | 1/25 red: expects source label `local`, binary now reports `legacy-global`/`project-file`. |
| `integration_logging` | 1062 | 29 | proc | red | 4.6s | 8/29 red: `wg done` in the fixture now runs the deterministic completion contract and fails in a non-git tempdir. |
| `integration_native_coordinator` | 1971 | 29 | proc | red | 14.9s | 1/29 red, and the whole target pins the native coordinator plane that 0/112 real tasks used. |
| `integration_native_executor_async_web` | 1199 | 38 | proc | red | 2.9s | 1/38 red (subtask child/parent resume) on the native-executor plane; unused daily. |
| `integration_project_local_pi_cli` | 1158 | 13 | proc | red | 22.2s | 6/13 red, and it `#[path]`-includes integration_deep_readonly_flip.rs and integration_dedicated_pi_evaluation.rs, so its 6 reds are the same rot counted twice. |
| `integration_provider_model_format` | 1225 | 49 | proc | red | 1.4s | 1/49 red: `wg profile use codex --no-reload` fails — `profile use` is a deprecated alias that now needs a profile definition/project root. |
| `integration_runs_exhaustive` | 1409 | 35 | proc | red | 2.0s | 7/35 red: `wg runs restore`/`runs diff` semantics drifted (restore leaves status Open, diff reports a change). |
| `integration_scope_guard` | 381 | 10 | proc | red | 1.2s | 1/10 red: drives `--executor claude`, a flag `parse_supported_execution_route` now rejects; the scope-guard assertion can never be reached. |
| `integration_service_coordinator` | 1969 | 49 | proc | red | 1.5s | 1/49 red (`test_eval_task_routes_to_inline_spawn` assertion drift). |
| `integration_setup` | 561 | 11 | proc | red | 1.3s | 11/11 red: driven `wg setup` is now route-gated (`--route <route> --yes`); tests use the retired interactive flow. |
| `integration_task_lifecycle` | 680 | 11 | proc | red | 10.0s | 2/11 red: `wg done` publication-derived completion + retry-of-abandoned now refused. |
| `integration_tier_defaults` | 377 | 13 | proc | red | 1.1s | 5/13 red: old `wg config -m` one-liner tier semantics (handler-first model spec replaced it). |
| `integration_trace_function_layers` | 1380 | 53 | lib | red | 1.2s | 3/53 red: per-run JSON summary layer assertions drifted (`layer3_*`). |
| `prompt_snapshots` | 370 | 14 | lib | red | 1.3s | 5/14 red: insta snapshots of the agent prompt are stale vs the current prompt assembly. |
| `test_prompt_logging_debug` | 263 | 2 | proc | red | 5.5s | 1/2 red: spawns with `--executor native`, removed flag. |
| `test_shell_retry_loop` | 755 | 5 | proc | red | 6.2s | 4/5 red: expects `run-job` back at `Open` after a failure restart; the failure-classification/restart path changed. |
| `test_verify_timeout_functionality` | 462 | 11 | proc | red | 9.2s | 1/11 red: expects the retired `wg done` timeout path ("legacy wg done ... flags are not supported"). |

### `retire-dup` — 5 targets, 2,458 LOC

| target | LOC | test fns | mode | status | run | evidence |
|---|---:|---:|---|---|---:|---|
| `integration_pi_opaque_execution_boundary` | 31 | 1 | proc | red | 2.4s | 1/1 red: Rust wrapper around the `pi_opaque_execution_assignment` shell scenario; the embedded argv assertion drifted. The shell scenario is the maintained copy. |
| `integration_pi_watchdog` | 1195 | 24 | proc | red | 2.1s | 1/24 red; the surviving live coverage is `integration_pi_watchdog_hardcap` (green, 3.3s). |
| `integration_setup_pi_concierge_gates` | 32 | 1 | proc | red | 1.5s | 1/1 red: a Rust wrapper that re-runs the `setup_pi_concierge_gates` shell smoke scenario, which needs `pi` on PATH; the scenario itself is the maintained copy. |
| `integration_strong_merge_resolution` | 237 | 3 | proc | red | 2.0s | 2/3 red: drives real git merge machinery that cannot work in the fixture ("control-plane.tree_unavailable"). |
| `test_crash_scenarios` | 963 | 6 | proc | red | 65.5s | 4/6 red after 65s; requires a live coordinator to actually pick up tasks ("Not all tasks were picked up by agents"). Duplicates the smoke suite/daemon coverage at 20x the cost. |
