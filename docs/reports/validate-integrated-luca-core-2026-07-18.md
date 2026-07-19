# Integrated Luca core validation

**Validation task:** `validate-integrated-luca-2`  
**Validation date:** 2026-07-19 (requested report filename retained)  
**Initial current-main baseline:** `ad10f278c7faa94d2c035c225528709ca1e4c460`  
**Validated and installed code main:** `0b8880665da4ed65c7ca38832bb6118d9ae20b40`  
**Original candidates:** Luca Pinello `d25cdd59`, `9a595528`, and `02204b19`

## Verdict

The three extracted core batches compose functionally on current main. Their owned regressions pass together, the clean stable suite is green, and the routing, provider-health, background-process, federation, review, and execution invariants exercised here did not regress.

Validation did find and repair one real lifecycle gap requested during this pass: a pre-schema `PendingEval` parent with a **completed, claimed** evaluator and exactly one post-start Evaluation could remain stuck across daemon restart because neither row had a persisted plan/lifecycle. Main `0b888066` now losslessly creates durable evidence, backfills metadata only after verified evidence exists, and consumes the verdict exactly once without reopening or rerunning the evaluator. The same commit fixes durable-verdict replay so observational `created_at`/wrapper-run changes do not conflict with identical semantic evidence, while the already-persisted record digest is still verified.

There are four separately tracked, non-Luca findings. They prevent a blanket claim that every current-main smoke is green, but none is in a Luca batch path:

1. U1 and U3 squash landing lost Git commit attribution on main, although their reports and source branches retain it (`repair-luca-core-attribution`).
2. `explicit_execution_selection` hangs in its interactive setup PTY fixture (`fix-explicit-execution`).
3. Two TUI performance smokes fail current main (`fix-tui-first`): first frame under 1 s storage delay, and 10k/50k initial graph publication at 2684 ms versus a 2 s budget. The 500 ms API-responsiveness and current four-sided layout smokes pass.
4. SIGKILLing a generated worker wrapper leaves its 120-second heartbeat sleep/subshell temporarily reparented to PID 1 (`fix-sigkilled-wrapper`). The U1 `/bg` job itself leaves no orphan; all validation-owned heartbeat groups were explicitly cleaned.

## Attribution and scope audit

### U1 — detached background process

- Luca's reviewed baseline is preserved on the integration branch as `8cf7a682` with Luca Pinello as author, the Claude co-author trailer, and `-x` provenance to `d25cdd59`.
- The hardening commit is `f68de662`: Rust-opened descriptors, checked in-process session creation, persisted PGID/start identity, TERM→KILL group cleanup, and explicit unsupported-platform behavior.
- Main contains the resulting tree through squash `60407659`, but `8cf7a682` is **not** a main ancestor and `60407659` has neither Luca author metadata nor a co-author trailer. This is an attribution defect, not a functional source loss.
- Retained implementation is visible in `Job.process_group`/`process_start_identity` (`src/executor/native/background.rs:39-59`), group-aware `kill` (`:313-382`), validated identity (`:682-730`), and in-process detached launch (`:768-857`).

### U2 — FailedPendingEval and route-stable evaluation lifecycle

- Main directly contains `9176849c`, authored by Luca Pinello, with Claude co-author and `-x` provenance to `9a595528`. Attribution is intact.
- Luca's positive safety predicate is narrowed by the integrated lifecycle to the owning `.flip-X`/`.evaluate-X` relation (`src/query.rs:358-387`) rather than every dot task.
- The reimplementation persists handler-first plans, bounded attempts, pipeline/verdict identity, exact planned invocation, and atomic reconciliation in `src/eval_lifecycle.rs` and `src/service/llm.rs`.
- This validation added the completed-legacy-evaluator recovery at `src/eval_lifecycle.rs:544-638,761-817,964-1100` and the semantic replay fix at `:376-466`.

### U3 — provider failure provenance

- `02204b19` was correctly **not** cherry-picked: its free-form refusal phrase matcher was spoofable.
- The diagnosis was reimplemented in source commit `2a440c71` with Luca and Claude co-author trailers. Typed, run-bound completion outcomes are defined at `src/service/provider_health.rs:151-218`; spawn persists a secret-free route key (`src/commands/spawn/execution.rs:140,967`); triage validates agent/task/run provenance before breaker accounting (`src/commands/service/triage.rs:554-722`).
- Main contains the resulting tree through squash `3027b825`, but `2a440c71` is **not** a main ancestor and `3027b825` dropped its co-author trailers. As with U1, this is a main-history attribution gap.

### No Casa import and PR #57 independence

The U1+U3 landing delta from their common pre-batch base changes only ten paths: background/bg tooling, done/spawn/triage/provider-health service code, two smoke scripts, and the grow-only manifest. U2 changes lifecycle/config/query/evaluator plumbing plus fixtures and its audit. The only `notify.rs` U2 change is a `Task` fixture initializer for additive fields; no Casa roster, household election, Telegram policy, Casa ledger/feed, alternate identity, review, federation, or execution authority was imported.

PR #57 remained a separate stream. It landed independently as merge `9604ff20` after the U1/U3 squashes; this task did not review, comment on, alter, reopen, or merge it. The Luca core changes do not touch PR #57's validation executor paths.

## Invariant audit

| Invariant | Evidence | Result |
|---|---|---|
| No implicit or cross-system provider fallback | Handler-first smoke; exact-plan Codex/Pi/Claude matrix; `test_cross_system_fallback_is_rejected_before_any_call`; all one-shot roles obey the same contract | PASS |
| No unbounded FLIP/evaluator agents | Plan generation has bounded schedule/transport attempts; overlapping slow-storage ticks reserve once; five extra ticks and three post-legacy ticks create no evaluation/agent growth | PASS |
| No provider false pause | Three real blocked `wg done` refusals containing `HTTP 401 quota` leave health empty/unpaused; three genuine fake 401 wrapper failures pause exactly one canonical Codex route | PASS |
| No U1 background job orphan | 14 background tests include 50 start/kill cycles, compound/pipeline, restart, TERM-ignoring descendants, PID reuse, and natural-exit race; real PTY `/bg kill` confirms leader and child gone | PASS |
| Wrapper heartbeat orphan | Intentional wrapper SIGKILL in the provider smoke leaves heartbeat sleep/subshell for up to 120 s; cleaned and tracked separately | OPEN FOLLOW-UP |
| Durable verdict replay | Existing record digest is verified; identical pipeline/stage/evaluation evidence replays as a no-op despite new wrapper/time observation | PASS after `0b888066` |
| Completed legacy evaluator restart | Exactly one post-start evaluation is migrated, plan metadata is backfilled only after verified durable evidence, source consumes once, repeated ticks do not rerun | PASS after `0b888066` |
| WG-Fed / Review / Exec authority planes | Credential-free spark smokes | PASS |

## Commands and results

All candidate Cargo work used one isolated target, `CARGO_TARGET_DIR=/tmp/wg-target-agent-562`, with `CARGO_INCREMENTAL=0`. Worker routing variables and WG path overrides were removed for the full suite.

### Formatting, build, lint, and clean stable suite

```bash
cargo fmt
cargo fmt --check
cargo build --locked
cargo clippy --locked

env -u WG_TASK_ID -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL \
    -u WG_TIER -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH \
    -u WG_WORKTREE_ACTIVE -u WG_BRANCH \
    CARGO_TARGET_DIR=/tmp/wg-target-agent-562 CARGO_INCREMENTAL=0 \
    cargo test --locked -- --test-threads=1
```

Results:

- `cargo fmt --check`: PASS.
- `cargo build --locked`: PASS.
- `cargo clippy --locked`: PASS with the repository warning baseline; no denied lint or new error.
- Full suite: PASS, **9238 passed, 0 failed, 34 ignored across 164 result blocks**, including 8 passing doctests and 2 ignored doctests.

### Focused code regressions

```bash
cargo test --lib executor::native::background::tests -- --test-threads=1
cargo test --lib executor::native::tools::bg::tests -- --test-threads=1
cargo test --lib service::provider_health::tests -- --test-threads=1
cargo test --bin wg provider_breaker_uses_completion_provenance_not_refusal_prose
cargo test --lib eval_lifecycle::tests -- --test-threads=1
cargo test --lib service::llm::tests::test_cross_system_fallback_is_rejected_before_any_call -- --exact
cargo test --lib service::llm::tests::test_every_one_shot_role_obeys_no_cross_system_failure_contract -- --exact
```

Results: PASS — 14 background, 6 bg-tool, 9 provider-health, 1 live-triage unit, 14 lifecycle, and 2 focused no-cross-system tests. The lifecycle set includes the new observational replay and completed-claimed legacy evaluator cases.

### Owned and combined daemon flows

Run first with the isolated candidate binary and again with the installed-main binary:

```bash
WG_SMOKE_SCENARIO=provider_completion_refusal_breaker \
  tests/smoke/scenarios/provider_completion_refusal_breaker.sh
WG_SMOKE_SCENARIO=nex_bg_process_group_pty \
  tests/smoke/scenarios/nex_bg_process_group_pty.sh
WG_SMOKE_SCENARIO=pending_eval_lifecycle_route_recovery \
  tests/smoke/scenarios/pending_eval_lifecycle_route_recovery.sh
```

Results:

- `provider_completion_refusal_breaker`: PASS both times; refusals stay at zero and real 401s pause exactly one route.
- `nex_bg_process_group_pty`: PASS both times; real `/bg run/status/output/kill/cancel`, no job leader/descendant remains.
- `pending_eval_lifecycle_route_recovery`: PASS both times; Codex/Pi/Claude exact plans, relation-aware rescue, no scoreless promotion, one pre-claim repair, overlapping held-lock ticks, five restart ticks with no agent growth, and the new completed-legacy evaluator consumed once across restart with no new Evaluation.

### Routing and cross-plane smokes

```text
handler_first_bare_provider_model       PASS
federation_spark_two_graphs             PASS (all 7 steps)
content_safety_spark                    PASS (all 7 steps)
exec_spark_borrowed_box                 PASS (all 6 steps)
tui_responsive_under_500ms_latency      PASS
tui_four_sided_layout_mobile            PASS
```

Separately reproduced and routed as non-Luca follow-ups:

```text
explicit_execution_selection            TIMEOUT at interactive setup PTY
tui_first_frame_slow_storage             FAIL at 1000ms first-paint leg (twice)
tui_large_graph_continuous_mutation      FAIL: 2684ms > 2000ms publication budget
```

### Main-only installation and installed checks

The reviewed code commit was fast-forwarded without rewriting history, then pushed:

```text
main == origin/main == 0b8880665da4ed65c7ca38832bb6118d9ae20b40
```

Only then was the global binary installed:

```bash
cd /home/bot/wg
cargo install --path . --locked
/home/bot/.cargo/bin/wg --version
```

Result: PASS; `wg 0.1.0`, installed from main `0b888066`. The three owned/combined smokes above then passed through `/home/bot/.cargo/bin/wg`.

## Follow-up ledger

- `repair-luca-core-attribution` — non-destructive attribution remedy and future squash preservation; do not rewrite published main.
- `fix-explicit-execution` — interactive setup PTY hang while retaining graph-only default.
- `fix-tui-first` — first-frame and large-graph publication budgets.
- `fix-sigkilled-wrapper` — eliminate the 120-second heartbeat orphan after untrappable wrapper death.

## Conclusion

The retained/reimplemented Luca logic is functionally integrated and passes its full independent composition gate after the bounded legacy-evaluator and verdict-replay repairs in `0b888066`. No implicit cross-system fallback, unbounded evaluation growth, provider false pause, U1 job orphan, Casa authority import, or cross-plane regression was observed. The report does not conceal current-main debt: U1/U3 attribution was lost by squash landing, two unrelated TUI performance gates and one setup PTY fixture are red, and wrapper SIGKILL leaves a temporary heartbeat orphan. Those are isolated in focused follow-ups rather than being misattributed to Luca or silently patched into unrelated policy.
