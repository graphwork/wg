# Corrected opaque Pi boundary re-verification

**Date:** 2026-09-15

**Task:** `verify-repaired-pi-execution`

**Prior baseline:** [Pi/Shell immutable-assignment boundary verification](pi-opaque-execution-boundary-verification.md) (preserved; its result was NO-GO before the repair)

> **Post-fix closure (2026-09-15):** This report remains the historical record for `fc846d01`/`ed7cd474`, including its valid Eval rejection and two blockers. The later candidate `5a4b9f98` resolves those blockers and passes the integrated closure described in [Integrated Pi execution release-candidate closure](pi-execution-release-candidate-closure.md). Nothing below is retroactively relabeled.

## Decision

**BOUNDED GO for the opt-in ordinary opaque worker plus hermetic FLIP/Eval path on the exact tested Pi route. NO-GO for enabling the experiment generally or including its optional managed-process mode.**

The original selector and incomplete-launch-plan defects are repaired. A fresh independent run executed useful work through direct Pi, default WG, and corrected opaque WG on `openai-codex` / `gpt-5.6-sol` / `high`. The corrected opaque worker preserved `openai-codex/gpt-5.6-sol` as one `--model` value, ran its pinned worker plan, completed real FLIP and Eval calls with their pinned hermetic plans, and published once.

Rollout remains blocked by an observed optional-capability admission defect: explicitly selecting an invalid `@mjakl/pi-processes` package passed both preflights, claimed an attempt, then failed as generic source execution. Missing selection is safely deferred but is wrapped as `WG-OPAQUE-LEGACY-ACTIVE` and also triggers an unrelated OpenRouter registry credential error. Those outcomes do not meet the experiment's stated accurate-admission boundary.

No global binary, Pi/WG setting, provider, daemon, production flag, source history, or stable path was changed. All daemons and graphs were disposable, the candidate binary was explicit, and Pi-owned authentication was only read in place.

## Exact tested identity

| Item | Value |
| --- | --- |
| Source | `fc846d014a82a244222c436e7928cf19fee6a43f` (`fix: complete opaque Pi launch contract`) |
| Candidate | `wg 0.1.0`, `/tmp/wg-verify-agent114-target/debug/wg`, SHA-256 `a67291ac6f13c9a0e6e126ff95b0dae42ec90119efd61a372a6ea68ae98db3f9` |
| Pi | `0.84.4`, resolved CLI bundle `/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js`, SHA-256 `5406c369954516fb56879d685e082ff9095cd6e06e41af406f394942377fd4bf` |
| WG Pi plugin | compat `0.3.0`, cached `pi-worksgood/index.js`, SHA-256 `6f813e795877f54ffce87bbd4a732f2b7506b8bc0e0de47bf6eedfd37e933135` |
| Wake extension | `@mjakl/pi-processes@2.0.0`; installed from checked-in lock SHA-256 `d276316a8cb96cbe5d4186fe219a167a9b05eab1d2a3ef3fdc0588bb821623ef` |
| Node / npm | `v25.4.0` / `11.13.0` |
| Evidence root | `/tmp/wg-reverify-agent114.LwiqnL` (retained host diagnostic data) |

Default WG was tested with `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT` unset and the current split route `pi:openai-codex:gpt-5.6-sol`. Corrected opaque WG was tested only with `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT=1` and `pi:openai-codex/gpt-5.6-sol`. Managed wake was a second explicit opt-in via `WG_PI_PROCESS_WAKE_EXTENSION`; it is not a default prerequisite.

## Same useful task, real execution

Each path received the same four-row `data.csv` and objective: calculate category totals, create a sorted Markdown `report.md`, preserve the CSV, and verify with Python 3. All three produced the exact `alpha=7` and `beta=5` result and preserved the input hash.

| Path | Actual selection | Result | Wall | Reported usage/cost | Retries / intervention after launch |
| --- | --- | --- | ---: | --- | --- |
| Direct Pi | split `--provider openai-codex --model gpt-5.6-sol` | exit 0; correct report | 15.775 s | input 1,985; output 313; cache-read 3,200; 4 turns; 3 tools; **$0.020915** | n/a / 0 |
| Default WG | configured colon route; stable split launch | Done; strict FLIP+Eval pass; one publication | 97.876 s | worker input 25,995; output 1,736; cache-read 156,800; **$0.260455**; reviews **$0.100260** | 0 / 0 |
| Corrected opaque WG | exact slash suffix as one model argument | Done; strict FLIP+Eval pass; one publication | 111.670 s | worker input 26,443; output 2,107; cache-read 119,040; **$0.254945**; reviews **$0.099270** | 0 / 0 |

The WG figures include graph prompting, worktree, deterministic evidence, semantic review, and landing semantics that direct Pi does not provide. Compared with direct Pi, default WG added 82.101 s and opaque WG added 95.895 s. Opaque added 13.794 s over default WG in this single observation. There was no human correction, restart, retry, or manual acceptance in the useful-work runs. Both agents attempted a Git push and accurately reported that the disposable repository had no remote; this did not cause a retry. These are single samples, not a benchmark.

Default WG recorded completion receipt `b3:585ebab3dc25624e5c159f611781d18a9e850922cb4b3b558840512b0d57a4c6`. Opaque WG recorded `b3:a225d94dfbf707a90a63a709c9f50860fa31f8ddd4de9f193cf18bb2aee873b5`. Each graph had one task-agent metadata record, one attempt (`attempt-0-1`, fence 1), and one publication receipt with `already_published=false`; neither had a second publication transition.

## Assignment and launch correspondence

The opaque worker assignment is retained under the evidence root. It binds:

- authored route `pi:openai-codex/gpt-5.6-sol` and opaque suffix `openai-codex/gpt-5.6-sol`;
- the exact Pi bundle and digest;
- reasoning `high`, fresh generated session, attempt worktree, 1,800-second timeout, and 5-second cancellation grace;
- fixed argv `--mode json -ne --no-skills --no-prompt-templates --no-context-files`;
- discovery disabled with only the exact WG extension; and
- Pi built-ins plus WG graph tools.

The generated production `run.sh` invoked that same absolute program and fixed argv, loaded only the pinned WG extension, supplied one `--model 'openai-codex/gpt-5.6-sol'`, supplied `--thinking high`, and used the assignment's worktree/session. There was no `--provider` and no second model selector. `pi-session-plan.json` records `opaque_assignment=true`, the same suffix, and the same fresh session.

Three content-addressed one-shot attribution files were emitted for `flip_inference`, `flip_comparison`, and `evaluator`. Each binds the same program/config identity, slash suffix, reasoning, and worktree, with `invocation_kind=hermetic_review`, no extensions, no tools, no context/skills/templates/session, and fixed JSON/print argv. The corresponding real review receipts report non-zero Pi usage and pass verdicts: FLIP used both pinned roles and Eval used the evaluator role. This is actual provider execution, not inference from task status.

## Managed adapter behavior

The adapter was exercised through the production `wg pi-process-worker` entry point, not by killing a guessed descendant group.

1. **Real Pi wake:** Pi 0.84.4 plus the exact installed extension received opaque `--model openai-codex/gpt-5.6-sol`, started one 2.5-second Node command, yielded, received one automatic wake, called process output once, and completed. Evidence has one process, exit 0, `success=true`, one start, one wake, one output capture, four turn ends, and no duplicate event. Wall time was 10.902 s. Evidence SHA-256 is `f23fabd5cbb9663ffdaf0ba47169985c7e59b275ce9e5e8c8a89618ba4304055`; raw stream SHA-256 is `c6dc0b120b001c53192ed4fe05bda63db390d872f4c4303c834bd25497b620ce`.
2. **Completion-before-yield:** the controlled Pi wire delivered completion twice before the yielded turn. The production adapter recorded one process and one continuation, counted one duplicate wake, waited for the continuation turn, then completed. This fixture proves adapter ordering, not provider support; the separate preceding case is the real-Pi proof.
3. **Adapter-owned timeout:** `--timeout-secs 1` expired inside the adapter. It returned `WG-PI-PROCESS-TIMEOUT`, terminated/reaped the receipt-bound command and its separately-owned Pi group, and left an unrelated `setsid sleep 60` alive. No negative-PID/group signal was sent by the test harness.
4. **Adapter-owned cancellation:** the harness sent SIGTERM only to the adapter PID. Its handler returned `WG-PI-PROCESS-CANCELLED`, reaped the receipt-bound command/Pi session, and left an unrelated process alive.
5. **Daemon restart:** with the production adapter active on a controlled long-running Pi wire, `wg service stop`/`start` left the agent and command alive. The immutable assignment SHA-256 stayed `86fc5a0d63536e08261906ce7b1a0473c0bd8d9d417a719674488449596f277c`; the trace contains exactly two offline probes (authoring plus worktree prelaunch), one RPC runtime launch, one agent record, and no publication. Cancellation then removed the owned command.

The controlled race/restart cases are deliberately not reused as proof of real provider execution.

## Negative and migration flows

| Case | Observed production behavior | Assessment |
| --- | --- | --- |
| Unsupported `codex:gpt-historic` | Stayed Open; no attempt/retry; one coalesced `WG-OPAQUE-LEGACY-ACTIVE` diagnostic naming the expected outer `pi:` form | Correct bounded refusal |
| Missing selection | Stayed Open; no attempt/retry; nested `WG-EXEC-ROUTE-MISSING` and correct `wg setup` action | Safe state, inaccurate outer label; see blocker 2 |
| Invalid optional package | `wrong-package@9.9.9` passed admission, claimed `attempt-0-1`, failed after `WG-PI-PROCESS-EXTENSION-MISMATCH`, then projected `Pi worker exited with code 1 before reviewed completion`, `source_execution_failed`, and `retry_count=1` | **Blocking defect** |
| Strict review unavailable | Explicit Shell candidate was preserved in Waiting/`NeedsReview`; attempt disposition `parked`; FLIP receipt `unavailable`/`reviewer_unavailable`; retry count 0; no completion/publication | Correct typed attention |

The explicit migration command was also exercised on a standalone TOML file. Dry-run found two exact values without changing bytes. Apply changed only those two exact string values, preserved a lookalike substring and another route, and wrote a timestamped backup. Before/after hashes proved it did not alter either live test project's `worksgood.toml`, graph, completed historical attempt, or persisted opaque assignment. Migration is operator-declared and is not automatic runtime normalization.

## Remaining blockers and boundary

1. **Invalid selected wake capability is validated too late.** Package name/version must be checked before claim (or produce a typed, non-source-failure attention state). The current late failure consumes a source attempt and hides the actionable mismatch behind generic failure.
2. **Missing route is mislabeled and produces unrelated noise.** The actionable nested `WG-EXEC-ROUTE-MISSING` is wrapped as `WG-OPAQUE-LEGACY-ACTIVE`; the same unselected daemon also logged an OpenRouter API-key registry refresh error. Missing selection should have its own outer admission code and should not imply a legacy active route or unrelated provider credential requirement.
3. Evidence covers one model/provider, one useful task, one real managed command, Linux process groups, and single-task daemons. It does not establish performance distribution, other Pi providers/models, concurrent adapters, Windows behavior, crash recovery across host reboot, or authority to delete the stable route.

Therefore the exact ordinary worker/review path may continue as an explicit controlled experiment. Do not enable it by default, include the optional wake selection in that bounded GO, delete the stable path, migrate live settings automatically, or authorize a global rollout from this report.

## Checked evidence

Passed candidate checks:

- `cargo build --locked --bin wg` using explicit `CARGO_TARGET_DIR`;
- `cargo test --locked execution_assignment --lib -- --test-threads=1` (10 passed);
- `cargo test --locked --test integration_pi_watchdog -- --test-threads=1` (24 passed);
- `cargo test --locked --test integration_pi_opaque_execution_boundary -- --test-threads=1` (1 passed);
- focused migration and reviewer-unavailability unit tests (1 passed each);
- the actual direct/default/opaque useful-work flows, real-Pi managed wake flow, production-adapter race/timeout/cancel/restart flows, negative flows, and explicit migration flow described above.

The checked-in watchdog test named `production_process_adapter_timeout_cancels_only_owned_process_group` currently triggers cancellation by externally signaling the adapter's process group. It passed, but was not used as proof of adapter-owned timeout; the separate actual `--timeout-secs 1` production-entry-point flow supplies that evidence.
