# Pi/Shell immutable-assignment boundary verification

**Date:** 2026-09-14

**Task:** `verify-pi-execution-boundary`

**Result:** **NO-GO for Pi rollout; keep the experiment opt-in.** The Shell half and the fail-closed admission/evidence boundaries are useful, but the Pi half cannot launch the project's ordinary supported Pi route with the tested Pi CLI. No live configuration, installed binary, origin/main ref, or production daemon was changed.

## Executive result

The experiment does remove the old *post-assignment* WG provider/handler/registry reconstruction from its controlled Pi branch. It also correctly keeps invalid admission from claiming an attempt. Those properties do not make it usable yet:

1. WG's supported project syntax is `pi:<provider>:<model>`. The experiment strips only the outer `pi:` and passes `<provider>:<model>` unchanged as Pi's single `--model` value. Pi 0.84.4 accepts either split `--provider <provider> --model <model>` or one `--model <provider>/<model>` selector, not `<provider>:<model>`. Thus the ordinary configured route `pi:openai-codex:gpt-5.6-sol` becomes the nonexistent Pi selector `openai-codex:gpt-5.6-sol`.
2. `RuntimeExecution::Pi` does not bind the complete Pi invocation/configuration. It stores only program, route, reasoning, and session. Worker and one-shot code later construct different argv/tool/session policies. This does not satisfy “the same actual Pi runtime/configuration” even if the selector dialect is repaired.
3. Managed-process wake is explicitly unsupported in this experiment. That is honestly fail-closed, but means the proven long-operation path cannot migrate yet.

The first issue blocked real Pi before a worker assignment was bound, so an opaque-Pi useful-work success was not manufactured with a fake route or a different provider. Direct Pi and current WG controls both completed the same neutral task on the same provider/model. Automated rollout is **not authorized** by this report.

## Tested identity and containment

| Item | Exact value |
| --- | --- |
| Source | `17fd9bf018ff97b82eef7f6627f955a1dcf539eb` (`fix: refuse conflicting opaque model selectors`) |
| Candidate WG | `wg 0.1.0`; SHA-256 `db87ff61b5638c1c3b3b679a735e02a618507b9cf84b73708209051846628a86` |
| Candidate path | explicit disposable build under `$CARGO_TARGET_DIR/.../agent-107/target/debug/wg` |
| Pi | `0.84.4`; resolved CLI bundle `/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js`; SHA-256 `5406c369954516fb56879d685e082ff9095cd6e06e41af406f394942377fd4bf` |
| Node / npm | `v25.4.0` / `11.13.0` |
| Model / reasoning | `openai-codex` / `gpt-5.6-sol` / `high` for the useful-work comparisons |
| Wake extension (reused evidence) | `@mjakl/pi-processes@2.0.0`, lock integrity `sha512-LAt8fKvGuVManprlSsiJ0V6cDb5hlL9DlZrGpIW5dpgHVJKozgtBOiCKNovBZR4JYqlFIzbrBvJd9VEfvM0Gmg==` |

Every WG run used a fresh `/tmp/wg-boundary-*` Git project, isolated `WG_GLOBAL_DIR`, no supervisor (`--no-supervise`), one daemon and the explicit candidate placed first on that disposable project's `PATH`. Real Pi alone read its existing Pi-owned authentication from the unchanged user home. Daemons were stopped after each case. The repository smoke harness owns cleanup for the controlled process case. No global install or route/profile write was performed.

## Comparable useful work

The neutral, non-Rust input was a four-row `data.csv`. The common useful objective was to calculate category totals, write a sorted Markdown `report.md`, preserve the CSV, and verify the arithmetic with Python. Direct Pi had its built-in read/write/bash tools with extension, skill, prompt-template, context-file, and session discovery disabled. WG necessarily added its task/graph prompt, WG tool surface, worktree, validation, review, and publication semantics; direct Pi is only the execution control, not a substitute for those semantics.

| Path | Actual selector | Result | Wall time | Pi-reported usage/cost | Interventions / retries |
| --- | --- | --- | ---: | --- | --- |
| Direct Pi | `--provider openai-codex --model gpt-5.6-sol` | Correct report; CSV SHA unchanged; exit 0 | 22.859 s | input 5,096; output 524; cache-read 3,968; 6 turns; 5 tool calls; **$0.043184** | 0 / n/a |
| Current WG | configured `pi:openai-codex:gpt-5.6-sol`; actual worker argv split to the same provider/model | Correct report, Python optional evidence, FLIP pass, Eval pass, one publication, task `Done` | 93.204 s | task total input 14,821; output 1,182; cache-read 143,872; **$0.181501** | 0 / 0 in the clean run |
| Opaque Pi experiment | configured `pi:openai-codex:gpt-5.6-sol`; attempted single Pi model selector `openai-codex:gpt-5.6-sol` | **Admission blocked**: Pi query returned zero rows; task stayed `Open`; no attempt, assignment, worker, usage, or cost | 8.403 s observation | 0 model calls / $0 | 0 / 0 |
| Opaque Shell + advisory review | exact bound Shell argv created and committed `note.txt`; reviewer intended the same configured Pi route | Shell work, host checks, evidence and one publication succeeded. Pi review was unavailable at preflight; advisory policy recorded the finding and task became `Done` | 15.387 s observation | no model usage | 0 / 0 |
| Opaque Shell + strict review | exact bound Shell argv created and committed `strict.txt`; same unavailable Pi review | Strict review blocked acceptance, preserved candidate/evidence, no publication; wrapper reported source exit 1 and task `Failed` | 15.390 s observation | no model usage | 0 human interventions; graph displayed retry count 1 after the source exit |

The clean current-WG run is the relevant control: task `neutral-work`, attempt `attempt-0-1`, fence 1, manifest `b3:a29650b397d893ad6402284e8e98f04aaa51b0e6ec2cc193855fa9018e6ff308`, FLIP receipt `b3:81f78c36cd08d8b7104fbda7b216d62f145c5ee807177605cfc1d3fafbc32538`, Eval receipt `b3:ac87bc2433b77c6072ca8246cb74fb8a9fe051f060c97c26f763809c7fa1f070`, and publication receipt `b3:d8318eeaa5a9d2959d4e31c964736656f56e43ebb4d52184fce91246eca0f861`. The publication object says `already_published=false`, records one before/after main transition, and synchronizes the root checkout. No second publication object or command launch was observed.

Two setup mistakes were retained rather than counted as unattended success:

- A first control run invoked the explicit candidate daemon but left the installed `wg` ahead of it for worker shell calls. Useful work ran, but the worker could not use the candidate's new `wg done --check` option; two strict semantic reviews rejected missing machine evidence and the task intentionally failed after 223.349 s / $0.586446. Fix: put the explicit candidate first on the disposable child `PATH`.
- A second control left generated `AGENTS.md`/`CLAUDE.md` untracked in the integration checkout. Work, review, and one publication succeeded, but completion parked `Waiting` rather than overwrite those bytes. Attempting to commit them afterward changed the evidence environment and correctly produced `DigestMismatch` instead of relabeling old evidence. Fix in the clean run: preserve/commit generated files before dispatch. This is evidence that publication safety works, not an opaque-path success.

## First changed boundary: selector dialect

These were actual Pi 0.84.4 CLI observations:

```text
$ pi --offline --list-models openai-codex:gpt-5.6-sol
No models matching "openai-codex:gpt-5.6-sol"

$ pi --offline --list-models openai-codex/gpt-5.6-sol
provider      model        context  max-out  thinking  images
openai-codex  gpt-5.6-sol  272K     128K     yes       yes

$ pi --offline --mode json --no-session --no-tools \
    --model openai-codex:gpt-5.6-sol --thinking low -p 'Reply ...'
Error: Model "openai-codex:gpt-5.6-sol" not found. Use --list-models ...
```

WG structurally requires the colon form in `parse_exact_pi_route` (`src/config.rs:2440-2481`). Opaque authoring then removes only `pi:` and preserves the remaining colon bytes (`src/execution_assignment.rs:351-389`). Preflight and launch both use that value as one `--list-models`/`--model` argument (`src/execution_assignment.rs:464-510`, `src/commands/spawn/execution.rs:3379-3406`, `src/service/llm.rs:1260-1284`). The mismatch is therefore inherent in the current accepted experiment, not an unavailable credential or unsupported model.

The daemon behavior after the failure was safe but noisy: status remained `Open`, `retry_count=0`, `current_attempt=null`, assignment count 0, then an assignment-keyed 60-second admission backoff was reported. The same backoff line was emitted on every one-second coordinator tick despite saying identical deferrals were coalesced. This is not wasted model work, but it is avoidable operational noise and does not expose a single durable attention action.

## Assignment-to-runtime trace

### Worker

Intended:

1. Project/task/role/tier policy resolves before the runtime boundary (`src/commands/service/coordinator.rs:2599-2673`).
2. `ExecutionAssignment` carries task, identity, role, config revision/fingerprint and `Pi { program, opaque_route, reasoning, session_id }` (`src/execution_assignment.rs:33-80`).
3. Successful admission binds generation, attempt, fence and runtime agent, then create-once persistence refuses changed bytes (`src/execution_assignment.rs:600-640`).
4. Runtime skips task tier/profile and stable model/provider registry resolution and builds a single `--model` argument (`src/commands/spawn/execution.rs:1530-1569,1703-1795,2189-2210,3379-3459`). The source guard also rejects a second model selector.

Actual real-Pi case: steps 1-2 occurred transiently, preflight rejected before step 3, and no launch/continuation existed. Thus the “no post-assignment inference” property is supported by controlled source/fixture evidence, but there is no actual provider launch proving useful execution through that branch.

The controlled smoke fixture supplies the missing mechanics without pretending to be provider proof: it records an unusual suffix once in fake-Pi argv; persists the bound task/agent/generation/attempt/fence/config revision; keeps the file byte-identical through config mutation and daemon restart; refuses same-attempt replacement; kills the owned worker; and admits no attempt for missing, successful-empty, transient, or active legacy selection. It also verifies the exact Shell argv/environment/working directory. This task was added as an owner of the existing `pi_opaque_execution_assignment` scenario so `wg done` reruns it through the supported Linux subreaper harness.

### Review / one-shot

Intended one-shot flow resolves the selected role, constructs the same type, runs hermetic preflight, and calls it once with no fallback (`src/service/llm.rs:319-376,1225-1305`). Actual opaque Shell completions recorded the intended reviewer route `pi:openai-codex:gpt-5.6-sol`; both failed before a provider call with the same zero-row diagnostic. No fallback or retry was observed. The immutable completion receipt records the exact reviewer route and outcome, but the ephemeral one-shot assignment itself is not persisted as a bound `execution-assignment/assignment.json`, so program/argv identity cannot be reconstructed from assignment evidence as it can for a worker.

Advisory policy permitted deterministic completion while retaining `flip.inference_route_unavailable`. Strict policy refused acceptance with: “candidate is preserved, no semantic acceptance was recorded, and no source quality failure was inferred.” The surrounding Shell wrapper then reduced that typed blocker to `Agent exited with code 1` and a failed attempt. The review authority itself behaved correctly; the wrapper projection is a remaining fidelity issue because it obscures “review unavailable” as the root blocker.

### Complete runtime configuration is not bound

`RuntimeExecution::Pi` has no argv, environment, extension/tool policy, working directory, prompt-delivery mode, timeout, or offline/network policy (`src/execution_assignment.rs:33-46`). Worker runtime synthesizes `--mode json`, a task env entry and later session/prompt args (`src/commands/spawn/execution.rs:1735-1751,2070-2129,3379-3459`). One-shot runtime independently synthesizes `--print -ne --no-tools --no-context-files --no-session` (`src/service/llm.rs:1260-1284`). Preflight synthesizes `--offline`, and only hermetic review adds `-ne` (`src/execution_assignment.rs:481-490`).

Therefore preflight and execution share a pinned executable and route, but not the same complete runtime/configuration. A user Pi setting, discovered extension, executor env/arg, or invocation tool difference can still change execution after assignment. The HEAD fix correctly refuses a competing `--model`, but does not close this broader gap.

## Wait/wake, cancellation, restart and optional capability

No new long provider run was purchased solely for this report. The accepted evidence in `docs/reports/pi-managed-process-wakeup.md` matches Pi 0.84.4, Node 25.4.0, model `openai-codex:gpt-5.6-sol`, and extension 2.0.0. It shows one managed start, automatic wake into the same live Pi RPC process/session/handle, one bounded output read, no duplicate command, and targeted cancellation that reaped the managed 60-second command while an unrelated 90-second process survived. The logical-time watchdog fixture reaches 603 seconds without busy polling or a false timeout. This remains evidence for the **stable split-route adapter**, not proof for the opaque candidate: `src/commands/spawn/execution.rs` changed after that candidate and opaque admission explicitly excludes the adapter.

Controlled opaque-fixture coverage is the honest boundary for restart/config/cancel:

- running bound assignment hash unchanged after project model/reasoning change plus daemon stop/start;
- persisted transient backoff survived restart;
- explicit cancellation targeted the bound agent and left assignment evidence unchanged;
- scenario cleanup is owned by the Rust subreaper harness, so no owned daemon/fake Pi is allowed to survive;
- no evidence of cancellation revival or a duplicate worker invocation.

With `WG_PI_PROCESS_WAKE_EXTENSION=/definitely/missing/unrelated-extension.ts`, an actual candidate daemon kept the task `Open`, `retry_count=0`, `current_attempt=null`, and wrote no assignment. It reported `WG-OPAQUE-OPTIONAL-CAPABILITY-UNISOLATED` before consulting the path or launching Pi. This demonstrates the documented **unsupported limitation**, not isolation: requesting the optional wake capability blocks all Pi work under the experiment. An unrelated absent extension that is not explicitly selected was not made a startup prerequisite.

## Durable WG properties

The experiment did not replace graph semantics:

- Shell assignment artifacts bind task, role, config revision, runtime agent, generation, attempt and fence. The observed JSON exactly bound argv, explicit environment and working directory.
- Optional and mandatory checks produced host/environment/repository/candidate-bound immutable evidence; mutating the integration checkout after capture yielded `DigestMismatch` rather than reuse.
- The clean stable control moved `Open → InProgress → Done` under one owner/fence and a publication-derived finalizer event. Worktree work was saved, merged once, and the root tree was clean.
- Advisory and strict review policy remained distinct. Unavailable review was never represented as a pass; advisory could proceed with a finding and strict could not publish.
- The controlled cancellation and accepted wake evidence use exact owned process identities and preserve unrelated processes.

Limitations: because real opaque Pi never crossed admission, task ownership, saved model work, continuation, cost accounting, and cancellation **inside a real opaque-Pi attempt remain unproven**. The Shell success cannot fill that gap.

## Recommendation and exact boundaries

### Recommendation: NO-GO

Do not enable `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT` globally, do not route normal Pi workers/reviewers through it, and do not delete the stable path. Keep the current experiment opt-in for controlled development only. This recommendation is evidence, not rollout authority.

### Operator decisions required before another acceptance attempt

1. **Choose one selector dialect at the authoring boundary.** Either migrate project Pi identities to Pi-native opaque selectors such as `pi:openai-codex/gpt-5.6-sol` (including config parser, profiles, CLI help, status, historical migration and exact loss reporting), or define a typed Pi invocation that binds the split provider/model argv before runtime. The current requirement that WG's colon suffix be passed byte-for-byte as Pi `--model` is not executable on Pi 0.84.4. Do not “fix” this by splitting or guessing after assignment.
2. **Define a complete immutable Pi launch contract.** Bind or hermetically derive the exact executable, argv, env/config root, extension/tool allowlist, cwd, prompt/session policy and relevant timeout/network policy used by both preflight and launch. Decide which wrapper-owned attempt fields may be added. A source comment that route/program are the “same runtime” is insufficient.
3. **Decide managed-process capability support.** Either make the stable adapter consume the immutable assignment without re-inferring provider/model, or retain explicit stable routing for tasks that request it. A global migration while long managed work is unsupported fails the release-level proof.
4. **Preserve typed review blockers through the worker wrapper.** Strict `reviewer_unavailable` should remain a decision/repair blocker, not collapse to generic source failure/retry accounting.
5. After those decisions, rerun one real opaque worker and one real opaque review on the same provider/model, plus actual restart/config-pin/cancellation. Do not reuse the controlled fixture as provider acceptance.

### Deletion/migration boundary

Nothing is ready for deletion now. In particular retain:

- stable `plan_spawn`, `handler_for_model`, executor registry/backend and split Pi argv paths;
- stable managed-process wake adapter;
- historical route readers and explicit legacy refusals;
- current completion, evidence, review, worktree, ownership and publication machinery.

Only after a real-provider batch proves worker, review/eval, retry/recovery, restart, cancellation, managed wait/wake and configuration change may the matching **post-authoring runtime routing branches** be deleted. Authoring-time project/profile/tier/role resolution remains WG-owned. Remote-provider placement remains separately on WG-Exec; it must not be converted to Shell. No histories, identities, credentials, evidence, or optional external features are migration candidates.

## Validation inventory

Actual commands/cases used:

- `cargo build --locked --bin wg` (explicit candidate; passed, existing warnings only)
- Pi version/hash and `--offline --list-models` dialect probes
- direct Pi neutral task (actual authenticated provider; passed)
- three current-WG neutral task rehearsals: one retained candidate-PATH mismatch, one retained publication-safety park, one clean actual success
- opaque actual daemon admission with canonical route (fail-closed, no attempt)
- opaque Shell advisory and strict completion runs
- opaque actual daemon with explicitly requested missing optional wake extension (fail-closed, no attempt)
- focused Rust tests and the owned controlled smoke scenario are run as repository validation; their results are recorded in the task log/completion evidence

Raw disposable roots retained during this evaluation:

- direct: `/tmp/wg-boundary-direct-uz1YAg`
- clean stable WG: `/tmp/wg-boundary-stable-57qcFR`
- retained stable candidate-PATH mismatch: `/tmp/wg-boundary-stable-V2MRfv`
- retained stable publication-safety case: `/tmp/wg-boundary-stable-Ib3EHF`
- actual opaque admission: `/tmp/wg-boundary-opaque-admission-mPTsQJ`
- opaque Shell review policies: `/tmp/wg-boundary-opaque-review-XOSVgc`
- optional capability refusal: `/tmp/wg-boundary-opaque-optional-VmLlUA`

The raw roots are diagnostic host artifacts, not repository-portable receipts. Durable conclusions are tied above to candidate/Pi hashes, WG content-addressed receipts where available, committed source lines, and the repository-owned controlled scenario. No future self-receipt or mandatory gate was introduced.
