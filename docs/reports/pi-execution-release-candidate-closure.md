# Integrated Pi execution release-candidate closure

**Date:** 2026-09-15 (UTC)

**Task:** `close-pi-execution-release-candidate`

**Production source under test:** `5a4b9f98789b747a1e2361635c4251377b4f13f8`

## Decision

**GO for operator push and required CI of this integrated candidate.** The stable colon-route behavior remains intact. The Pi-native slash route and managed-process adapter remain explicit opt-ins; this closure does not enable either globally or retire the stable path.

The two blockers recorded by the 2026-09-15 re-verification are resolved at `5a4b9f98`: an invalid selected managed-process package is rejected before claim, assignment, retry, or Pi invocation, and a missing route produces one `WG-EXEC-ROUTE-MISSING` episode without `WG-OPAQUE-LEGACY-ACTIVE` wrapping or unrelated OpenRouter-registry credential noise. No further production defect was found. This closure adds only a candidate-bound Rust smoke entry point and report reconciliation.

This conclusion does **not** rewrite history. The earlier `verify-repaired-pi-execution` Eval rejection was correct for `fc846d01`: its two findings describe behavior observed before `5a4b9f98`. The first real stable rehearsal in this closure also had a truthful FLIP rejection because it lacked host-captured `validate.py` evidence; the disposable fixture was corrected and rerun rather than calling that attempt a pass.

## Exact identity and isolation

| Item | Exact value |
|---|---|
| Production source | `5a4b9f98789b747a1e2361635c4251377b4f13f8` (`fix: reject invalid opaque Pi capability before claim`) |
| Candidate binary | Cargo debug `wg 0.1.0`, SHA-256 `d08183bc2da3a233b75bcc65e670e18cce2b5cc16d682b4bb76eebb06734a2d8` |
| Pi | `0.84.4`; CLI bundle SHA-256 `5406c369954516fb56879d685e082ff9095cd6e06e41af406f394942377fd4bf` |
| WG Pi plugin | compat `0.3.0`; cached `pi-worksgood/index.js` SHA-256 `6f813e795877f54ffce87bbd4a732f2b7506b8bc0e0de47bf6eedfd37e933135` |
| Managed extension | checked-in lock installs `@mjakl/pi-processes@2.0.0`; real proof launches it explicitly |
| Repository baseline | local `main` and initial HEAD were `5a4b9f98`; `origin/main` was `1f33c4e407feb29a0c05d0ca5ab0a1b4fa75844a` |
| Disposable evidence root | `/tmp/wg-rc-agent116` |

Real-provider runs used existing Pi-owned authentication in place. No credential file/value was read, copied, exported, or logged. Every graph, project configuration, daemon, cache, and XDG/WG state directory was disposable. No global Cargo install, Pi Console setting change, live route/profile/config mutation, production-daemon action, push, deploy, or automatic experiment rollout occurred.

## Real provider execution

Both fresh isolated projects received the same checked-in `validate.py`, four-row `data.csv`, and neutral task: produce a Markdown report containing `alpha=7` and `beta=5`, preserve the input bytes, capture `python3 validate.py` via `wg done --check`, commit, review, and land locally without pushing.

| Path | Selection and observed model | Outcome | Authoritative completion |
|---|---|---|
| Stable | configured `pi:openai-codex:gpt-5.6-sol`; experimental variables absent; actual model `openai-codex:gpt-5.6-sol` | one attempt, worker + two-phase FLIP + Eval passed, clean local landing; source data SHA-256 `ec8a9551ee69595d8b3ec69ac47dc8dfc51e4d6d889d76c4b18b0395b8954eb7` | `b3:e7244c2540b637d62ec09715c5d8c8b115b16d44857d2d02d59b4d2ebeea399d` |
| Opt-in opaque | `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT=1`; configured `pi:openai-codex/gpt-5.6-sol`; actual model retained as `pi:openai-codex/gpt-5.6-sol` | one attempt, worker + hermetic two-phase FLIP + hermetic Eval passed, clean local landing; identical source-data hash | `b3:2d7d9ba8b0cd963914569812b7c157f3c2c334962b1f08b0e2ed7efae26de4a9` |

The opaque assignment, session plan, and generated production `run.sh` preserve `openai-codex/gpt-5.6-sol` as one model value. `run.sh` contains exactly one `--model 'openai-codex/gpt-5.6-sol'`, no split provider selector, and the pinned Pi executable/plugin identities. Review activity records the exact slash route for both FLIP roles and Eval with non-zero provider usage. This is real provider evidence, not fixture inference.

The initial stable trial produced the right files but its FLIP correctly rejected prose-only validation. Adding the validator before dispatch and rerunning from a fresh project produced the stable receipt above. The rejection remains diagnostic history and is not counted as success.

## Managed-process adapter and admission

The checked-in `tests/fixtures/pi-process-wakeup/real-provider-proof.sh` rebuilt exact source `5a4b9f98`, installed the lock-pinned extension into scratch, and invoked the production `wg pi-process-worker` entry point with Pi-owned auth. It passed with one process start, one automatic wake, one output capture, four turn ends, exit 0, and the exact wake marker. Evidence SHA-256 is `407f15f4650a064889a6fc9f2831b6189277698c393ee481cf91ac92889028d1`; raw-stream SHA-256 is `d47c63854ba33e56f28e5613ce688a34ac246a95cd282b1798379281a66ff624`.

The candidate-bound controlled scenario and watchdog integration establish the other properties through production entry points:

- the valid package is identity/version pinned;
- `wrong-package@9.9.9` produces exactly one `WG-PI-PROCESS-EXTENSION-MISMATCH` naming expected and found identities, while the task remains Open with no assignee/current attempt/retry and Pi is never invoked;
- an unselected graph produces exactly one actionable `WG-EXEC-ROUTE-MISSING`, remains Open without attempt/retry, and produces neither legacy wrapping nor OpenRouter/API-key registry noise;
- adapter timeout and SIGTERM cancellation reap the receipt-owned Pi/command descendants while an unrelated `setsid` process survives;
- restart preserves immutable assignment identity and explicit cancellation does not rewrite it.

The controlled fake-Pi cases prove ordering, admission, restart, timeout, cancellation, and process ownership. They are **not** represented as model/provider evidence; the separate real proof above supplies that evidence.

## Explicit-selection reconciliation

The first-user rehearsal's completion log contains an old optional validation failure for `explicit_execution_selection`: evidence `b3:9635c92af450ec594538fefaa7283a94163723a3dba693bb9e08a50c5ccb586b`, exit 101. Its later report prose claimed a passing rerun, but no successful host-bound receipt accompanied that claim; the subsequent FLIP rejection correctly identified the mismatch.

For this closure, `tests/integration_explicit_execution_selection.rs` is a focused Linux-subreaper entry point. It selects the exact existing grow-only manifest scenario, prepends Cargo's exact `CARGO_BIN_EXE_wg` directory to `PATH`, runs it via `worksgood::smoke`, restores `PATH`, and fails the Rust test if the scenario blocks completion. The unchanged checked-in `explicit_execution_selection` scenario now passes against the integrated candidate. The successful command is captured again as optional deterministic evidence on this closure task; the old exit-101 receipt remains historical and is not relabeled.

## Stable CI-equivalent checks

The following passed on the integrated tree (warnings from existing dead/unused code were visible and not promoted to failures):

- `cargo fmt --check`;
- `cargo clippy --locked` and the narrower `cargo clippy --locked --bin worksgood`;
- `cargo test --locked execution_assignment --lib -- --test-threads=1` — 56 passed;
- `cargo test --locked --test integration_pi_watchdog -- --test-threads=1` — 33 passed;
- `cargo test --locked --test integration_pi_opaque_execution_boundary -- --test-threads=1` — passed, including the isolated production-daemon negative-admission cases;
- `cargo test --locked --test integration_pi_sole_model_plane -- --test-threads=1` — 7 passed;
- `cargo test --locked --test integration_explicit_execution_selection -- --test-threads=1` — exact manifest scenario passed through the candidate-bound subreaper harness;
- `cargo test --locked --test integration_service -- --test-threads=1` — 9 passed, 3 documented timing/legacy tests ignored;
- `cargo test --locked --lib -- --test-threads=1`, `cargo test --locked --bin worksgood -- --test-threads=1`, and `cargo test --locked --doc` — passed;
- `cargo build --locked --bins` — passed;
- the exact-source real managed-provider proof and both real worker/reviewer routes described above — passed.

The minimum focused commands are rerun through `wg done close-pi-execution-release-candidate --check ...`, so completion stores their actual exit statuses as host-bound optional evidence before semantic review. The repository-required stable checks pass. No claim is made that this local closure executes non-required nightly CI; previously recorded nightly cleanup flakiness remains advisory rather than hidden or reclassified.

## Residual boundary

Evidence remains bounded to Pi 0.84.4, `openai-codex/gpt-5.6-sol`, Linux process ownership/subreaping, one real managed command, and isolated single-task daemons. It is not a performance benchmark and does not establish other providers/models, Windows semantics, high-concurrency adapters, or host-reboot recovery. The slash route and managed wake remain opt-in. Operator authority is still required for push, hosted CI, global installation, deployment, live configuration changes, or any default rollout.
