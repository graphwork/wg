# Luca core fixes: current-main implementation batches

**Date:** 2026-07-18<br>
**Task:** `prepare-current-main`<br>
**Candidates:** `d25cdd59`, `9a595528`, `02204b19`<br>
**Source audited:** `e7fad9bb`; current `main`/`origin/main` was `9e33da3e` when this report was finalized. The only intervening path is the documentation-only `docs/reports/luca-casa-wgfed-overlap-2026-07-18.md`, so the audited source is identical to current main for every path below.

This report turns the refreshed [Luca/Casa inventory](./inventory-luca-casa-stream-2026-07-18.md) into three implementation-ready **current-main** batches. It does not authorize a merge of `luca/integration/casa-pinello`, and it does not treat a clean textual apply as semantic approval.

The FailedPendingEval plan below incorporates the independent [PendingEval / FLIP inline-route resolution](./pending-eval-flip-inline-route-resolution.md). `9a595528` is a necessary safety slice, not the incident fix by itself.

## Decision summary

All three candidate commits remain patch-distinct from main:

```text
$ git cherry main d25cdd59
+ d25cdd594dba4789eba310891cc2caed2ce1975d
$ git cherry main 9a595528
+ 9a5955288dc8419f2efff10986cfe25d8f3bc1a9
$ git cherry main 02204b19
+ 02204b19ce5d9f15a7706febcda43cd67a84d9a0
```

Stable patch IDs have no equivalent commit in main. All three patches pass both exact and three-way `git apply --check`, but each current-main defect was also independently reproduced; applicability is not the evidence for the recommendation.

| Batch | Defect on current main | Candidate disposition | Required landing shape |
|---|---|---|---|
| **U1 — detached process** (`d25cdd59`) | Reproduced. `spawn_detached` requires an external `setsid`; when unavailable, the tracked child exits and the command never starts. | Preserve Luca's commit as a `cherry-pick -x` baseline **only inside the U1 branch**, then complete containment before merge. | In-process session creation, file-descriptor redirection, process-group/tree cancellation, safe persisted identity, explicit Windows behavior, permanent `/bg` flow coverage. |
| **U2 — eval rescue lifecycle** (`9a595528`) | Reproduced. Dispatcher says `.flip-X` is ready over `FailedPendingEval`, while `wg done .flip-X` refuses it. Separately, current Codex scaffolding persists a bare route and inline preflight rejects it. | `cherry-pick -x` as U2-A, preserving Luca/Claude authorship and tests. Do **not** close U2 there. | U2-A safety predicate, then persisted stage-aware route plan, pipeline/verdict identity, CAS reconciler, bounded retry, and shared readiness diagnostics from the resolution report. |
| **U3 — provider failure provenance** (`02204b19`) | Reproduced. A genuine `wg done` refusal whose blocker title contains `HTTP 401`/`quota` is `FatalProvider`; three refusals pause the provider/service. | **Reimplement**, do not cherry-pick the phrase allowlist. Port Luca's regression fixtures and retain co-authorship if their test code is materially reused. | Structured completion-refusal provenance before prose classification, SpawnPlan-derived health-route identity, legacy-safe fallback, live triage regression. |

### Dependency/order decision

- **U1 is source-independent** and may be implemented and landed in parallel with U2/U3. It shares only the grow-only smoke manifest at integration time.
- **U2-A should land before the composed U3 rescue-satellite regression** so the original FailedPendingEval loop is removed at its state-machine source. This is a validation/attribution order, not a source dependency: U3 must also test a non-eval `wg done` refusal because such refusals remain valid after U2-A.
- **U3 does not wait for U2-B through U2-F.** It can use the already-landed `ExecutionSystemKey` and current `SpawnPlan`; it must add endpoint identity rather than depending on the new agency plan.
- U2's lifecycle work and U3's health work must not invent duplicate route types. U2 owns `AgencyDispatchPlan`; U3 owns a small `HealthRouteKey` derived from `SpawnPlan` plus `ExecutionSystemKey` and endpoint fingerprint.
- Recommended merge order is **U2-A → U3 → U2-B…F**, with **U1 anywhere**. The three implementation tasks may work concurrently if the smoke-manifest additions are merged grow-only.

No source merge or PR comment was made while preparing this report.

## Reproduction ledger

All graphs were disposable under `/tmp`; temporary regression tests were removed after execution. No active graph, profile, provider counter, or production process was changed.

### U1: current background launch fails without an external `setsid`

Current `src/executor/native/background.rs:626-646` executes:

```text
bash -c "setsid <command> > <log path> 2>&1 < /dev/null"
```

and stores the PID returned for that shell. On this Linux host `/usr/bin/setsid` exists, so the old `test_job_store_kill` passes. To exercise the macOS/missing-utility condition deterministically, a temporary unit regression set `WG_BASH_PATH=/bin/bash`, supplied an empty `PATH`, launched the absolute command `/bin/sleep 60`, waited 250 ms, and asserted the stored PID was alive. Current main failed:

```text
running 1 test
thread '...repro_missing_external_setsid_drops_background_job' panicked:
current spawn_detached returned a PID that exited because external setsid was unavailable
test ...repro_missing_external_setsid_drops_background_job ... FAILED
test result: FAILED. 0 passed; 1 failed
```

That is the candidate's portable failure, exercised through the current `JobStore::run` and current `spawn_detached`, not merely by checking for the utility. The temporary test was reverted. The ordinary Linux control passed, which also proves why the existing test does not protect macOS:

```text
cargo test test_job_store_kill -- --nocapture
... test_job_store_kill ... ok
```

### U2-A: ready rescue satellite cannot complete

A fresh graph was configured with `auto_evaluate=true`; `X` was sent through the real failure command:

```text
wg fail X --class agent-exit-nonzero --reason 'synthetic wrapper crash'
```

Current main recorded:

```text
Status: failed-pending-eval
Agent exited without wg done — entering failed-pending-eval for rescue evaluation
```

For the real chain `X -> .flip-X -> .evaluate-X`, the two public paths then disagreed:

```text
$ wg ready
Ready tasks:
  .flip-X - FLIP rescue

$ wg done .flip-X
Error: Cannot mark '.flip-X' as done: blocked by 1 unresolved task(s):
  - X (Crashed parent): FailedPendingEval
```

The command exited 1. This is the exact state asymmetry fixed by `9a595528`: readiness already bypasses both soft states in `src/query.rs:349-370`, while completion bypasses only `PendingEval` at `src/commands/done.rs:1444-1460`.

### U2-B: handler-qualified route is still lost before inline agency claim

The route half of the same lifecycle was re-run against current source in a clean `HOME` with explicit `codex-cli`, `auto_evaluate=true`, and FLIP enabled. Resume scaffolded the normal acyclic chain, the parent entered `PendingEval`, and `.flip-route-parent` was ready. The hidden row exposed bare `model=gpt-5.4-mini`; one real current-main tick reported:

```text
[dispatcher] Spawning eval inline for: .flip-route-parent ... (model: gpt-5.4-mini)
[dispatcher] Failed to spawn eval for .flip-route-parent:
  invalid invocation-scoped evaluator route "gpt-5.4-mini"
Tick complete: 0 alive, 1 ready, 0 spawned
```

The task stayed Open and no agent was claimed. Current `eval_scaffold` still stores `ResolvedModel.model` (`src/commands/eval_scaffold.rs:203-215,253-262`) instead of the full route that `ResolvedModel::spawn_model_spec` already knows how to produce (`src/config.rs:2121-2133`). `spawn_eval_inline` still validates only that stored model (`src/commands/service/coordinator.rs:3251-3279,4304`). This confirms the July 17 resolution remains current and is not Codex-specific.

### U3: a workflow refusal reaches the pause threshold

A temporary integration regression fed `classify_error` the exact current blocked-completion prefix and a legal blocker title containing provider-looking words:

```text
Cannot mark '.flip-X' as done: blocked by 1 unresolved task(s):
  - X (Authentication failed HTTP 401 quota): FailedPendingEval
```

The title is graph content; it is not provider stderr. Current main produced:

```text
kind=FatalProvider
kind=FatalProvider
kind=FatalProvider
count=3 paused=["codex"] service_paused=true
thread 'current_main_done_refusal_poisoning_reaches_pause_threshold' panicked:
workflow refusal poisoned provider health
```

The temporary test was removed. The live path can see this text because the wrapper appends `wg done` stderr to `output.log` (`src/commands/spawn/execution.rs:1947`) and triage classifies `failure_reason` or only the last ten output lines (`src/commands/service/triage.rs:550-592,657-681`). The reproducer isolates the deterministic classifier/counter defect; U3's permanent test must additionally drive the wrapper/dead-agent/triage path so truncation and provenance are covered.

## U1 — detached native background jobs

### Exact candidate semantics versus main

`d25cdd59` changes only `src/executor/native/background.rs` (36 additions, 6 deletions):

1. imports Unix `CommandExt`;
2. removes the external `setsid` program;
3. calls `libc::setsid()` from `pre_exec`;
4. prefixes the shell body with `exec`;
5. continues to return `child.id()`.

The important existing equivalents are **primitives, not a landed fix**:

- agent spawn already calls in-process `setsid` at `src/commands/spawn/execution.rs:746-756`;
- `terminal_host` has another checked `setsid` helper at `src/terminal_host/mod.rs:352-365`;
- `service::kill_process_graceful` already snapshots descendants, sends TERM then KILL, and has a Windows `/T` implementation (`src/service/mod.rs:147-225`).

None is used by `JobStore`. Current background code still shells out to `setsid`, `kill_process` signals only positive PID (`src/executor/native/background.rs:574-596`), and the Windows launch path waits for `cmd /C start /B` then returns PID 0. There is no landed patch-equivalent.

### What `d25cdd59` fixes and does not fix

It correctly removes the macOS dependency and makes a simple external command replace Bash, so the stored PID is useful for `sleep 60`. It does **not** establish the stronger invariant the API advertises: “kill this job and its descendants, even after an agent restart.”

Residual defects:

- A pipeline, compound shell form, subshell, or explicit backgrounding can have more than one process. `kill(pid, …)` leaves descendants alive.
- Redirection remains interpolated into shell text. `command` is intentionally shell syntax, but `log_path.display()` is internal data and must not become syntax; a graph path with spaces, quotes, `$()`, or shell metacharacters can redirect incorrectly or execute text.
- Dropping `tokio::process::Child` loses the natural wait handle. Later `waitpid(pid, WNOHANG)` can only reap the direct child while this WG process remains its parent; after process/WG restart it cannot recover an exit code.
- Bare PID/PGID persistence permits reuse. A stale “Running” row must never signal an unrelated process. `kill(pid, 0)` also treats `EPERM` as “dead,” although the process exists.
- On non-Unix, current `spawn_detached` returns 0. The candidate leaves this untouched; `taskkill /PID 0` is not a valid job identity.
- Reusing Linux-only `/proc` descendant walking alone is insufficient on macOS and loses already-reparented children. A session/process-group identity is the portable Unix containment primitive.

### Minimal current-main patch slices

**U1-A — preserve Luca's portable launch baseline.** Cherry-pick `d25cdd59` with `-x` onto the dedicated U1 branch. Add the failing no-external-`setsid` regression in the same review series. This preserves Luca Pinello as author and the existing Claude co-author rather than copying the hunk anonymously. Do not merge U1-A alone.

**U1-B — make process containment the stored contract.** Refactor launch to return a typed handle, for example `{ leader_pid, process_group, start_identity }`, and add backward-compatible `#[serde(default)]` job fields. On Unix:

- open the log with Rust and pass cloned file descriptors as `stdout`/`stderr`; use `stdin(Stdio::null())`;
- invoke `bash -c <command>` without composing redirection text;
- create a session in `pre_exec`; store the resulting PGID (normally the leader PID);
- TERM the negative PGID, wait for the group/leader, then KILL the group; use the existing tree helper as a Linux defense in depth, not as the only macOS mechanism;
- before signaling a persisted job, validate the recorded leader start identity. On mismatch, fail safe as `Orphaned`; do not kill a reused PID/group.

Do not force `exec` as the containment mechanism for arbitrary shell grammar. It is a useful simple-command optimization, but the process group must remain authoritative for compound commands.

**U1-C — explicit platform outcome.** Either implement a real Windows process handle/job object and tree cancellation, or return a loud “background jobs unsupported on this Windows build” error. Persisting PID 0 is not an acceptable compatibility fallback. A later Job Object implementation can be a separate commit and rollback unit.

**U1-D — status/reaping.** Centralize liveness so refresh, status, kill, and delete use the same handle validation. Save terminal state atomically. Where a direct wait is available, reap and retain the exit code; after restart, report “exit unknown” rather than inventing Failed/Completed. Never change a vanished pre-kill job to Cancelled.

### Permanent regression contract

1. Unit: external `setsid` absent, absolute child command — launch remains alive.
2. Unit: log/base path contains spaces, single quotes, `$()`, and semicolons — output goes only to that file and no marker command executes.
3. Integration: simple command, `sh -c` compound, pipeline, and a child/grandchild that ignores TERM. Kill must remove the whole recorded group after TERM→KILL.
4. Integration: parent agent/WG process exits while job continues; a new `JobStore` can status then cancel it.
5. Negative: natural exit immediately before kill is terminal natural completion/unknown, not Cancelled; stale/reused identity is refused without signaling the foreign process.
6. Repetition: run start/status/kill at least 50 times to catch PID/shell races; assert no remaining PGID/process marker.
7. Platform: Linux and macOS are required. Windows must prove Job Object/tree behavior or assert the loud unsupported result; never accept PID 0.
8. User flow: add a credential-free PTY/`expect` smoke that drives the real Nex `/bg run`, `/bg status`, `/bg output`, and `/bg kill` commands, including a descendant. Register the scenario under the U1 implementation task in the grow-only manifest. Library-only `JobStore` tests are not enough for the terminal-facing slash command.

### Rollback boundary

U1 is isolated to native background jobs. U1-A may be reverted independently before merge; U1-B's schema is additive, so old rows remain readable. Once U1-B has created new Running rows, rollback must first stop/mark those jobs with the new binary or preserve a compatibility reader—an old binary would ignore the PGID/start identity and regress to unsafe PID-only killing. U1 must not alter Pi worker process groups or WG-Exec leases.

## U2 — FailedPendingEval, route identity, and verdict-safe rescue

### Exact candidate semantics versus main

`9a595528` changes only `src/commands/done.rs`:

- extends the existing dot-prefixed system-dependent completion bypass from `PendingEval` to `PendingEval | FailedPendingEval`;
- adds a positive three-node rescue test;
- adds a regular-dependent negative test.

This aligns completion with already-landed behavior:

- all main readiness variants allow a dot-prefixed system child over either soft state (`src/query.rs:349-370,423-445,501-523,709-730`);
- evaluator entry accepts both statuses (`src/commands/evaluate.rs:190-193,814-817`);
- shell tasks are now intentionally excluded from FailedPendingEval rescue (`src/commands/fail.rs:49-72`).

No equivalent exists in `done.rs`; the real CLI reproduction above proves the gap. The candidate is therefore a clean safety slice.

The candidate does not repair the handler-loss reproducer, stale/historical rows, durable verdict identity, scoreless promotion, duplicate scheduling attempts, or `why-blocked`. Those are part of U2 because they form the same rescue lifecycle and can amplify the completion asymmetry into repeated agents.

### State/security audit

The U2 implementation must preserve these invariants from the resolution report:

1. `X -> .flip-X -> .evaluate-X` is an ordinary acyclic chain. Do not add SCC exceptions.
2. `PendingEval`/`FailedPendingEval` satisfy only their **own** evaluation/rescue satellites, never ordinary dependents, remote dependents, `.assign-*`, `.place-*`, or unrelated dot-prefixed work.
3. Scaffold and invocation use one persisted, handler-first, stage-aware route plan. No invocation re-resolves ambient configuration or guesses a handler from `provider=openrouter`.
4. Transport/preflight/auth failure is not a semantic verdict and cannot promote or reject the parent.
5. One source attempt has one `pipeline_id`, one FLIP, one evaluator, keyed verdicts, and one atomic parent consumption.
6. A durable verdict is written before a stage/parent terminal transition. Restart after verdict write links it; it does not call the model again.
7. No `PendingEval` parent becomes Done because its evaluator is missing or merely terminal. Current `resolve_pending_eval_tasks` does both at `src/commands/service/coordinator.rs:904-950`; that must be removed behind verdict-required consumption.
8. Route, transport, format, semantic, and source-rescue counters are distinct and bounded. A route failure must use an expected-generation CAS so two ticks cannot both increment stale snapshots.
9. Historical repair is monotonic and fail-closed. An ambiguous legacy OpenRouter row or multiple verdicts requires an operator choice; “pick newest” and “use current profile” are unsafe.
10. Plans persist named endpoint/reference and reasoning but never secret material. Explicit fallback may change only a model whose `ExecutionSystemKey` is identical.

The current “system means dot prefix” predicate (`src/graph.rs:850-852`) is too broad for a trust-bearing bypass. The relation should be explicit: `.flip-{X}` and `.evaluate-{X}` (or matching lifecycle metadata) may bypass soft state on `X`; `.verify-*` is pipeline-eligible in other contexts and must not inherit rescue authority accidentally.

### Minimal current-main patch slices

These are sequential because several touch graph/coordinator state. They are one U2 batch, not independent PR claims that each “fix the incident.”

**U2-A — Luca safety slice.** Cherry-pick `9a595528 -x`. Keep Luca/Claude authorship. The clean emergency commit may retain main's broad predicate, but it does not complete the trust boundary. Record two required negatives for U2-F:

- an unrelated dot task depending on `X` remains blocked;
- `.assign-X`/`.verify-X` remain blocked unless lifecycle metadata proves they are the owning rescue stage.

U2-F replaces the broad predicate everywhere with the shared relation-aware disposition and makes those negatives pass. Do not advertise U2 complete after U2-A.

**U2-B — canonical agency plan.** Add the stage-aware `AgencyDispatchPlan`/`AgencyCallPlan` specified in the resolution report. FLIP must persist distinct `FlipInference` and `FlipComparison` calls; evaluator persists `Evaluate`. Every call stores canonical raw handler-first route, endpoint reference, reasoning, `ExecutionSystemKey`, selection provenance, validated same-system fallbacks, and a plan hash. `Task.model/provider` remain backward-compatible display mirrors, never invocation authority.

**U2-C — pipeline and verdict schema.** Add serde-defaulted lifecycle fields (`pipeline_id`, source attempt, route generation, plan hash, run ID, linked FLIP/eval verdict IDs, consumed verdict ID, separate counters). Verdict files add pipeline/attempt/stage/run identity and content digest. Creation is create-if-absent: same ID/digest is idempotent; a conflicting digest is quarantined.

**U2-D — exact invocation and bounded execution.** Scaffold once through the planner; `spawn_eval_inline` reloads and validates the persisted plan, CAS-claims its exact generation, and passes plan identity to `wg evaluate`. Evaluation uses those exact stage calls instead of role re-resolution. Invalid routes park as typed `ExecutionWaiting/Blocked` with bounded backoff; they do not call `wg fail` and do not consume semantic attempts.

**U2-E — reconciler/migration.** Implement the shared dry-run/apply primitive (`wg repair eval-lifecycle`) and daemon reconciliation described in the resolution report. Link valid durable verdicts before reopening anything; consume the parent verdict ID in the same graph CAS as the terminal outcome. Auto-repair only lossless route evidence (`codex + model`, or exact corroborated profile decomposition). Ambiguous `openrouter + model` remains `NeedsRouteRepair`. Never manually mark hidden tasks Done without a linked verdict.

**U2-F — one dependency disposition and diagnostics.** Export a relation-aware `DependencyDisposition` and use it in every readiness variant, `wg done`, and `why-blocked`. Expected diagnostic is “dispatcher-ready via evaluation-system bypass,” followed by route/lifecycle health—not a false root blocker. Keep `wg cycles` unchanged.

### Permanent regression contract

The detailed matrix in `pending-eval-flip-inline-route-resolution.md` is normative. At minimum:

- Unit: Luca's positive rescue and regular negative, plus unrelated-dot, `.assign`, `.verify`, remote/federated, and loop negatives.
- Unit: handler-first round trips for Codex, Pi/OpenAI-Codex, Pi/OpenRouter, Nex/OpenRouter, and Claude; FLIP's two routes remain distinct; ambiguous legacy OpenRouter fails closed.
- Unit: stale concurrent invalid-route CAS increments once; verdict create/link/consume CAS is idempotent and stores the exact ID.
- Integration with fake handlers: current-main scaffold → parent soft state → FLIP verdict → evaluator verdict → one parent outcome for Codex, Pi, and Nex. Assert persisted route equals actual invocation.
- Historical fixtures: zero/one/multiple/stale/already-consumed verdicts for both PendingEval and FailedPendingEval. Missing timestamps do not select “newest.”
- Restart matrix: kill after each scaffold, claim, result, verdict-write, stage-link, parent-consume, and registry-cleanup boundary. Pre-verdict may retry within budget; post-verdict never re-runs semantics.
- Storage/concurrency: blocked graph lock, delayed fsync, and two overlapping ticks preserve old/new snapshots and one scheduling reservation.
- Permanent credential-free service smoke: real wrapper crash → `FailedPendingEval` → qualified fake-handler FLIP/evaluate → parent terminal; extra ticks produce no new agents/verdicts; `wg ready` and `why-blocked` agree; no cycle is reported. Register under U2.

### Rollback boundary

- U2-A is independently revertible, though doing so reopens the proven deadlock.
- U2-B/C must dual-write compatibility display fields during rollout. Before U2-D is enabled, their additive schema can be rolled back safely; old readers ignore it.
- After U2-D creates a planned pipeline, rollback may disable agency dispatch and run the new repair tool, but must **not** reactivate the old scoreless resolver. Drain or park all PendingEval/FailedPendingEval rows first.
- After a parent stores `consumed_verdict`, rollback requires a reader that honors that fence. Otherwise the old “latest timestamp” logic can consume/rerun semantic work.
- Back up graph, evaluations, and registry before historical apply. Dry-run and apply are separate commits/feature gates; ambiguity never mutates state.

## U3 — provider-health classification from provenance, not prose

### Exact candidate semantics versus main

`02204b19` changes only `src/service/provider_health.rs`:

1. adds `is_wg_done_refusal(stderr_lower)` before auth/quota/CLI keyword checks;
2. maps matching prose to `FatalTask`;
3. adds phrase coverage and a repeated-counter unit test.

The diagnosis and test scenario are valid. Current main has no completion-refusal guard and reproduced the pause. Some structured pieces have landed elsewhere:

- `Task.failure_class` exists (`src/graph.rs:129-181,498`) and deliverable preflight sets `DeliverableMissing`;
- wrapper raw-stream classification produces stable API failure classes;
- `ExecutionSystemKey` preserves handler + wire (`src/config.rs:1777-1852`);
- `SpawnPlan` already knows executor/model/endpoint/provenance.

They are not connected to provider health. Triage still rebuilds identity through legacy `extract_provider_id(executor, model)` and reconstructs “stderr” from human prose/ten lines. There is no landed equivalent.

### Why the exact phrase patch must not ship

- The classifier accepts arbitrary text. A real provider/model response can emit `Cannot mark ... as done` and evade the circuit breaker. The candidate's assertion that the phrase “can only appear” after `wg done` is not enforceable at this API boundary.
- Blocker titles, verify command stdout/stderr, task descriptions, and paths are untrusted graph content. They can contain auth/quota words (false provider failure) or refusal words (false task failure).
- Phrase inventory drifts. Current actual verify failure is `Verify command failed ...`, not the candidate's `The verify command must pass`; the candidate's `completion contract is unmet` is no longer emitted. New review/deliverable/merge gates will drift again.
- Triage reads only the last ten lines when output is longer. Cleanup warnings can push the refusal out, while a matching model sentence can be pulled in.
- `extract_provider_id` understands only `claude`, `native:*`, and `shell`. Handler-first routes now include Codex, Nex, Pi, and external CLIs. Collapsing all Nex/Pi executions ignores wire and endpoint; parsing model substrings is not identity.
- “Provider” here means an LLM execution route. It must not read or mutate WG-Exec `ProviderRegistry`, trust, signed liveness, leases, or result verification.

### Minimal current-main patch slices

**U3-A — structured completion outcome.** Introduce a serde-defaulted per-agent execution outcome (or an atomically written agent-outcome sidecar) containing agent ID, task ID, run/start identity, executor exit, and `CompletionAccepted | CompletionRefused(code) | ExecutorFailed(class)`. The trusted structural source is the `wg done` call/wrapper branch, not matching its stderr. Refactor done gates toward stable refusal codes (`blocked`, `deliverable`, `smoke`, `verify`, `validation`, `worktree`, `merge`, `agent-bypass`). Human prose remains for UX.

Instrument both paths:

- `wg done` records a typed refusal when called under a matching `WG_AGENT_ID`;
- the wrapper records a fallback refusal if its own final `wg done` returns nonzero.

Triage validates task/agent/run identity and checks structured outcome first. Legacy rows without outcome fail conservatively; any prose fallback must require wrapper provenance and an anchored exact format, never the candidate's free substring list.

**U3-B — canonical health-route key.** Derive and persist a `HealthRouteKey` at spawn from `SpawnPlan`: handler, provider/wire (`ExecutionSystemKey`), and a non-secret endpoint identity/fingerprint for Nex. CLI handlers can use their self-authenticated system key; endpoint routes must not collapse unrelated base URLs/credential references. Do not hash/log secret values. Success resets only the same key that ran successfully.

Agent registry and metadata need serde-defaulted key fields. Historical entries may use the old extractor only as an explicitly labelled legacy bucket; do not guess Pi versus Nex from `openrouter` model text.

**U3-C — outcome-aware classification.** Replace `classify_error(exit, stderr)` at triage with classification over structured outcome plus typed failure class. Only genuine provider/auth/quota/CLI-start failures increment. Completion refusal, verify/deliverable/smoke/graph rejection, context error, and task timeout do not. Keep a small pure prose classifier for legacy/provider diagnostics, but it must not override structured provenance.

**U3-D — atomic health state.** Serialize ProviderHealth updates under a lock/atomic rename. Current direct `fs::write` can lose counters under competing coordinator/manual ticks. Key pause/resume and last error by `HealthRouteKey`; retain configured global `on_provider_failure=pause` behavior explicitly rather than accidentally treating one route as all routes.

### Permanent regression contract

1. Port Luca's exact repeated blocked-refusal test; if test code is copied/adapted materially, retain Luca/Claude `Co-authored-by` trailers in the U3 test commit.
2. Current done refusals: blocked, deliverable, smoke, verify (including stderr `HTTP 401`), validation, uncommitted worktree, merge conflict, and forbidden skip flags all leave the matching health counter at zero.
3. Spoof negative: arbitrary provider stderr containing `Cannot mark ... as done` remains a provider failure when structured provenance says executor/provider failure. This is the security test missing from `02204b19`.
4. Truncation negative: more than ten cleanup/output lines after a typed refusal still classify correctly; a matching phrase in the last ten lines without provenance cannot suppress a real 401.
5. Real failures: 401/bad key, missing handler executable, and exhausted quota increment only the correct handler+wire+endpoint route and reach the configured threshold; 429 remains transient.
6. Handler matrix: Claude, Codex, two Nex endpoints on the same wire, Nex/OpenRouter versus Pi/OpenRouter, and an external CLI. One endpoint's failure must not poison another. No WG-Exec provider file changes.
7. Full wrapper/dead-agent/triage smoke: a credential-free fake handler succeeds, final `wg done` is refused by a real graph gate three times, one service tick/reap path runs, provider count stays zero, and service remains running. Then a fake structured 401 reaches threshold for exactly one route. Register under U3.
8. Concurrency: two triage writers cannot lose/inflate the atomic counter, and replaying one agent outcome is idempotent by run ID.

### Authorship and rollback boundary

Do not cherry-pick `02204b19`: its production strategy introduces a spoofable classifier and stale vocabulary. Reimplement on current main. Cite `02204b19` in the commit message as the original diagnosis; use `Co-authored-by: Luca Pinello` (and retain the existing Claude trailer) only where Luca's test/code is materially carried forward, not as a courtesy trailer on unrelated redesign.

U3-A/B are additive and can dual-write; U3-C should be feature-gated until the full live triage smoke passes. Once structured outcomes/route keys are written, rollback may continue through the legacy conservative classifier, but must not delete counters or reinterpret endpoint keys. U3-D's on-disk migration must preserve old provider IDs as legacy entries and be downgrade-readable or backed up before rewrite.

## Cross-batch integration and ownership

| Surface | U1 | U2 | U3 | Integration rule |
|---|---|---|---|---|
| `src/executor/native/background.rs` | owns | — | — | U1 only. |
| process helpers | may reuse/extend `service/mod.rs` | — | — | Do not change Pi/WG-Exec semantics. |
| `done.rs` | — | owns rescue predicate | may add typed refusal plumbing | Sequence U2-A first; U3 should minimize gate-logic edits and consume typed errors/outcomes. |
| `graph.rs` | additive Job data is outside graph | owns eval lifecycle schema | avoid task failure overloading | Separate `failure_class` (task) from agent execution outcome. |
| coordinator/triage | — | coordinator eval pipeline | triage provider health | Separate modules; no shared “provider” state. |
| route identity | — | `AgencyDispatchPlan` | `HealthRouteKey` from `SpawnPlan` | Reuse `ExecutionSystemKey`; U3 adds endpoint identity, not another planner. |
| smoke manifest | one grow-only entry | one grow-only entry | one grow-only entry | Never replace/reorder existing scenarios; resolve textually by adding all three. |

### Definition of done for downstream implementation tasks

A downstream task is not complete merely because the original commit applies or its unit tests pass:

- **`integrate-luca-detached`** closes only after the real `/bg` PTY flow kills descendants and platform behavior is explicit.
- **`integrate-luca-failedpendingeval`** closes only after U2-A plus the route/pipeline/verdict/reconciler contract is implemented or deliberately split into dependency-ordered follow-ups that keep the parent task open. Applying `9a595528` alone is explicitly insufficient.
- **`integrate-luca-provider`** closes only after structured provenance reaches live triage and route identity distinguishes handler/wire/endpoint. A phrase list alone is explicitly insufficient.

For every code slice: write the named failing test first, run focused tests, `cargo fmt`, `cargo fmt --check`, `cargo clippy`, `cargo build`, and `cargo test`; add the permanent owned smoke before `wg done`. After source changes, run `cargo install --path . --locked` before exercising the installed binary.

## Final recommendation

Extract three small, reviewable current-main branches—not the Casa integration branch:

1. preserve Luca's U2 safety commit first and immediately build the full verdict-safe lifecycle around it;
2. reimplement U3 from structured execution provenance and handler/wire/endpoint identity;
3. preserve Luca's U1 portable launch baseline, but merge it only with process-group/tree containment, safe redirection/identity, and explicit Windows behavior.

This keeps Luca's generally reusable diagnoses and authorship, rejects the unsafe/stale implementation portions, and leaves Casa identity, gateway, conversation, and product policy outside all three core batches.
