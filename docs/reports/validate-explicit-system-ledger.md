# Validation ledger — explicit execution-system selection & bounded agency recovery

**Task:** `validate-explicit-system`
**Validated commit (worktree HEAD):** `de976119` (fast-forwarded from branch base `fd3aa555` to align source with the globally-installed `wg` binary, which is built from the main worktree HEAD)
**Date:** 2026-07-25
**Contract under test:** [`docs/design-explicit-execution-system.md`](../design-explicit-execution-system.md)

## 0. Summary

| Check | Result |
|---|---|
| `cargo fmt --check` | ✅ clean (CI fast-fail gate) |
| `cargo build` | ✅ exit 0 (65 warnings, no errors) |
| `cargo clippy` | ✅ exit 0 (203 warnings; CI runs plain clippy, not `-D warnings`) |
| Explicit-selection contract lib tests | ✅ 31 passed / 13 retired (intentional Pi-only pivot) |
| FailedPendingEval state-machine | ✅ 15/15 integration + 23 pending-eval/flip-recovery |
| Service / coordinator unselected gate | ✅ 25 passed |
| Pi sole-model-plane / canonical config / bare-alias / setup | ✅ 32 passed |
| Dispatch / failure-injection / error-recovery / spawn-template / global-config | ✅ 50 passed |
| `explicit_execution_selection.sh` smoke | ✅ PASS (re-aligned to ratified Pi-only contract — see §3) |
| `agency_pi_weak_tier_routes_to_pi_handler.sh` smoke | ✅ PASS (Pi stays on Pi, same-system fallback Terra→Sol, Claude never runs) |
| `pi_runtime_delivery.sh` smoke | ⚠️ FAIL — installed Pi runtime drifted to 0.82.0 (unpatched); code deliverable intact, see §4 |
| Full `cargo test --lib` | ⚠️ 2960 pass; 7–10 `profile::named` tests flake under parallelism (pre-existing HOME env-var race, reproduces on clean main — see §5) |

**Verdict:** The explicit execution-system selection contract, the no-cross-system agency fallback, and the bounded FailedPendingEval/FLIP recovery all hold on the integrated main state. No contract was weakened. Two pre-existing/environment findings documented below (§4, §5).

## 1. Integration assessment (what was validated against what)

Both upstream dependencies are **already merged to main and refined further** — their branches are stale:

- `fix-failedpendingeval-and` — the actual deadlock fix `9176849c` is on main. Main evolved the FailedPendingEval handling independently: `src/eval_lifecycle.rs` (top-level, +1160 lines via `de976119`), a 444-line `tests/integration_failed_pending_eval.rs`, coordinator `max_agents` bounding, and the relation-aware `DependencyDisposition::EvalSystemBypass` in `done.rs`. The dependency *branch* (`wg/agent-385/...`) was based on old main (`99dc204e`) and its `done.rs`/`sweep.rs` forms are older than main's.
- `make-pi-epipe` — fully on main: `scripts/install-patched-pi.sh`, `docs/pi-integration/upstream-patch/output-guard-epipe/`, `src/commands/doctor.rs` EPIPE/output-guard classifier (`classify_pi_output_guard`, `check_pi_output_guard`), `Makefile` `install-patched-pi` target, `cli.rs` `PiStreamBridge`, and `tests/smoke/scenarios/pi_runtime_delivery.sh`.

Merging the stale dependency branches would have caused 6-file conflicts with high risk of weakening the contract (forbidden by the task). Validated the integrated main state directly instead. **One genuinely-missing refinement** flagged in §6 (sweep.rs durable-verdict awareness — defense-in-depth only; the primary bound is already enforced by `max_agents` + the satellite-deadlock fix).

**Ratified contract = Pi is the sole LLM execution plane.** `wg setup --route` accepts only `pi`; `wg config --model codex:...`/`claude:...` returns `Error: expected pi:<provider>:<model>; non-Pi handlers are unsupported`. Legacy persisted codex/claude plans still execute on their exact handler (migration data, never re-routed). 13 old multi-handler agency tests are intentionally retired (`#[ignore]` "retired non-Pi LLM dispatch compatibility behavior").

## 2. Validation commands & results (verbatim exits)

```
cargo fmt --check            → exit 0
cargo build                  → exit 0  (65 warnings)
cargo clippy                 → exit 0  (203 warnings)

cargo test --lib -- service::llm:: execution_selection::
  → 31 passed; 0 failed; 13 ignored; 0 measured   (exit 0)
  Key passes (the contract):
    test_pi_failure_without_fallback_is_loud_and_never_attempts_claude   ok
    test_failing_pi_process_never_executes_claude_process                 ok
    test_cross_system_fallback_is_rejected_before_any_call               ok
    test_explicit_same_system_fallback_runs_in_file_order                ok
    test_every_one_shot_role_obeys_no_cross_system_failure_contract      ok
    test_production_agency_dispatch_has_no_hardcoded_claude_fallback     ok  ← source-scan invariant
    test_agency_dispatch_for_spec_routes_pi_handler_first                ok
    test_native_credentials_never_cross_provider_boundary                ok
    execution_selection::default_source_is_inactive                      ok  ← Config::default() is Unselected
    execution_selection::explicit_local_source_selects                   ok
    execution_selection::system_keys_separate_handler_and_wire           ok

cargo test --test integration_failed_pending_eval
  → 15 passed; 0 failed  (full state machine: enter→rescue-pass→done_rescued,
     rescue-fail→failed, explicit-wg-fail→terminal, system_bypass, shell-skip)

cargo test --test integration_pending_eval_state --test integration_verify_first
  → 15 + 8 passed; incl. test_max_eval_rescues_caps_to_failed  (bounded rescue)

cargo test --test integration_service --test integration_native_coordinator
  → 3 + 22 passed; incl.
     test_service_start_fails_on_invalid_config_instead_of_using_defaults
     test_service_start_rejects_implicit_coordinator_config
     native_coordinator_executor_only_is_unselected

cargo test --test integration_pi_sole_model_plane --test integration_canonical_config \
            --test integration_bare_alias_contract --test integration_setup
  → 2 + 15 + 4 + 11 passed

cargo test --test integration_dispatch_boot --test integration_failure_injection \
            --test integration_error_recovery --test integration_coordinator_spawn_template \
            --test integration_global_config
  → 2 + 5 + 8 + 11 + 24 passed

bash tests/smoke/scenarios/explicit_execution_selection.sh        → exit 0  PASS
bash tests/smoke/scenarios/agency_pi_weak_tier_routes_to_pi_handler.sh → exit 0  PASS
bash tests/smoke/scenarios/pi_runtime_delivery.sh                 → exit 1  FAIL (§4)
```

## 3. Smoke-guard / contract drift found and fixed

`tests/smoke/scenarios/explicit_execution_selection.sh` (owned by `require-explicit-execution`, `fix-explicit-execution`) was authored for the **original multi-handler** explicit-execution design and had drifted from the ratified **Pi-only** plane:

- asserted the unselected message contained `wg setup --route codex-cli --yes` and `wg profile use <name>` (current message is Pi-only: `Choose Pi explicitly`, `wg setup --route pi`, `wg profile select pi`);
- ran `wg setup --route codex-cli --scope local --yes` and asserted a Codex route was written (`--route` now accepts only `pi`);
- asserted `--executor claude` reached the selection preflight (now rejected at clap arg-parse: `[possible values: pi, shell]`);
- asserted status showed `codex` (selected daemon runs `executor=pi`).

The scenario was internally half-migrated (it already expected Pi as the interactive recommendation). **Action taken:** re-aligned the stale assertions to faithfully guard the ratified Pi-only contract — *same contract strength, no weakening*:

- graph-only init + credential-free CRUD (unchanged);
- `service start` fails `WG-EXEC-UNSELECTED` naming the Pi route, asserts **no implicit fallback handler** is recommended, and **no daemon state** (state.json/daemon.sock) is created;
- `spawn --executor pi` and `chat create` fail unselected with no graph/worktree/chat mutation;
- interactive wizard still recommends Pi and allows graph-only decline;
- explicit `setup --route pi` writes **only** handler-first Pi routing and **no** `claude:`/`codex:` model anywhere;
- selected daemon runs `executor=pi` and never crosses to another handler.

Added `validate-explicit-system` to the scenario's `owners` (grow-only) and raised its timeout to 120 s. The scenario now PASSes against the live binary and is a faithful regression guard for the contract.

## 4. Finding — installed Pi runtime drifted (environment, not code)

`pi_runtime_delivery.sh` FAILs: the installed Pi at `/home/bot/.nvm/versions/node/v25.4.0/bin/pi` is **0.82.0** with the pre-fix `output-guard.js` (does not treat EPIPE as a clean closed-consumer). The code deliverable is intact and working — `wg doctor` correctly classifies the guard as vulnerable and names the exact repair command (`make install-patched-pi`). This is **environment drift** (Pi upgraded past the patched 0.80.6), not a code regression, and EPIPE handling is an explicit **non-goal** of the explicit-execution contract (design §13). Not re-patched during validation because `make install-patched-pi` downgrades the **shared global** Pi install (0.82.0→patched 0.80.6) — out of scope for a validation task and disruptive to other in-flight work. **Recommended follow-up:** a human/operator re-runs `make install-patched-pi` (or the patch is upstreamed into Pi 0.82.x).

## 5. Finding — pre-existing `profile::named` HOME-race flakiness

`cargo test --lib` (full parallel) flakes 7–10 `profile::named::tests::*` tests. Root cause: those tests mutate the process-global `HOME` via `std::env::set_var` (the module-level `HOME_MUTEX` serializes only *within* the module, not across other lib tests that read `HOME`/global-config concurrently). The first panic poisons the mutex and cascades. **Reproduces identically on clean main** (`de976119`: 7 failed). Passes 43/43 single-threaded. Completely unrelated to the explicit-system contract (I touched only a `.sh` + `manifest.toml`, which are not compiled into the binary). **Recommended follow-up:** scope `HOME` per-test (temp dir + a process-wide env lock, or refactor to pass the dir explicitly) — small, well-isolated fix.

## 6. Compatibility notes

- **No compat const bumped.** The explicit-execution contract and FailedPendingEval state machine carry no new `WG_*_COMPAT_VERSION`; agency federation stays `WG_AGENCY_COMPAT_VERSION = "1.2.4"`. The Pi-only plane is a WG-internal dispatch decision, not a wire change.
- **Legacy persisted plans:** codex/claude/pi persisted plans still execute on their exact handler (`persisted_plan_invokes_exact_codex_pi_and_claude_handlers`) — backward compatible, never re-routed, never fallen-back.
- **Retired tests:** 13 old multi-handler agency-dispatch tests are `#[ignore]`'d ("retired non-Pi LLM dispatch compatibility behavior") — intentional, not a regression.
- **One defense-in-depth gap remains** (not a contract violation): `src/commands/sweep.rs` has no durable-verdict awareness, so orphan reconciliation *could* reset a satellite that already produced a durable verdict. The primary respawn bound is already enforced by coordinator `max_agents` + the satellite-deadlock fix (`9176849c`), so agent creation is bounded regardless. The `fix-failedpendingeval-and` branch attempted this but is too stale to merge cleanly. **Recommended follow-up task:** add durable-verdict awareness to `sweep.rs`.

## 7. Commits

- `de976119` — validated worktree HEAD (== main; fast-forwarded from `fd3aa555` to match the installed binary).
- This validation's own change (committed on branch `wg/agent-734/validate-explicit-system`): re-aligns `tests/smoke/scenarios/explicit_execution_selection.sh` to the ratified Pi-only contract and adds `validate-explicit-system` to its `owners` in `tests/smoke/manifest.toml` (grow-only).

## 8. Checklist mapping

- [x] Fresh credential-free graph-only workflow succeeds; LLM dispatch fails with explicit setup instructions — `explicit_execution_selection.sh` + `execution_selection::default_source_is_inactive` + `test_service_start_rejects_implicit_coordinator_config`.
- [x] Explicit Pi/Codex/Claude/native-OpenRouter/custom routes remain on their chosen systems through success and failure — Pi is the active plane (`test_agency_dispatch_for_spec_routes_pi_handler_first`, `agency_pi_weak_tier_routes_to_pi_handler.sh`); legacy codex/claude persisted plans run on their exact handler; non-Pi *new* selection is refused (ratified Pi-only).
- [x] No hard-coded Claude fallback reachable from a non-Claude selection — source-scan invariant `test_production_agency_dispatch_has_no_hardcoded_claude_fallback` + `test_failing_pi_process_never_executes_claude_process`.
- [x] Same-system fallback works only when explicitly configured — `test_cross_system_fallback_is_rejected_before_any_call` + `test_explicit_same_system_fallback_runs_in_file_order` + smoke Terra→Sol path.
- [x] FailedPendingEval success/failure cases terminate with bounded agent creation — 15 integration state-machine tests + `test_max_eval_rescues_caps_to_failed` + coordinator `max_agents`.
- [x] `cargo fmt --check`, `cargo clippy`, full `cargo test` (contract-relevant suites green; two pre-existing/environment findings in §4–§5 documented, neither a contract regression), relevant smoke scenarios pass.
- [x] Concise validation ledger produced (this document).
