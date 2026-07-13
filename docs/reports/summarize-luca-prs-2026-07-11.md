# Maintainer summary: Luca Pinello PRs #49-#57

Source report: `docs/reports/review-luca-piniello-open-prs-2026-07-11.md`.
Review facts were gathered at 2026-07-11 14:58 UTC. No GitHub state was changed.

## Findings by severity

1. **Block #57: red stable CI plus evaluator commands can hang indefinitely.**
   PR: [#57](https://github.com/graphwork/wg/pull/57) `feat: evaluator executes validation commands (trust inversion fix)`.
   Status: `MERGEABLE` but `UNSTABLE`; `Build & Test (stable)=FAILURE`, other four checks green; no reviews/comments.
   Code finding: `src/agency/validation_exec.rs:356-361` runs task-authored validation commands through `sh -c` with `.output()` and no deadline. Even with the read-only allowlist, an allowed command can block forever. `src/commands/evaluate.rs:522-533` runs this inside evaluation for every task description with extracted commands.
   Recommended action: request changes. Fix stable CI and add hard per-command timeouts before reconsidering.

2. **Block #51 until human reply routing is tied to confirmed sender identity.**
   PR: [#51](https://github.com/graphwork/wg/pull/51) `feat: wire the human-task dispatch tail (R10/R11/R13)`.
   Status: all five checks green, `MERGEABLE`, `CLEAN`; no reviews/comments.
   Code finding: `src/commands/service/human_dispatch.rs:321-365` routes any inbound non-command Telegram body to a waiting human task by receiving bot/channel and newest parked task. `src/commands/service/human_dispatch.rs:340-342` explicitly allow a shared bot reply to answer any human's parked task, and `src/commands/service/human_dispatch.rs:360-364` writes the reply without checking the sender against #49's Telegram binding map.
   Recommended action: request changes. Rebase after #49 and require `sender -> confirmed binding -> agent_id` before routing or completing a human task.

3. **Block #54 unless the deliverable-gate override is removed or human-gated.**
   PR: [#54](https://github.com/graphwork/wg/pull/54) `fix: deliverable-preflight false positives on discard/external-worktree files`.
   Status: all five checks green, `MERGEABLE`, `CLEAN`; no reviews/comments.
   Code finding: parser fixes are useful, but `src/commands/done.rs:1520-1559` adds `WG_DELIVERABLE_PREFLIGHT_OVERRIDE=1`, which lets the same agent shell bypass missing deliverables and mark a task `Done`.
   Recommended action: request changes. Keep the parser fixes, but remove the override, gate it behind a human-only override comparable to the smoke override policy, or make it leave a non-success state requiring maintainer confirmation.

4. **Block #56 because disposable scope does not actually prevent ordinary durable task creation.**
   PR: [#56](https://github.com/graphwork/wg/pull/56) `feat: --scope disposable guard on wg add (R8)`.
   Status: all five checks green, `MERGEABLE`, `CLEAN`; no reviews/comments.
   Code finding: `src/dispatch/plan.rs:560-565` propagates `WG_SCOPE`, but enforcement in `src/scope_guard.rs:90-96` rejects `wg add` only when the new task is explicitly tagged `persistent`. `src/commands/add.rs:243-245` and `src/commands/add.rs:870-872` call only that tag-specific guard, so `WG_SCOPE=disposable wg add "new durable task"` is still allowed.
   Recommended action: request changes. Define whether all ordinary `wg add` tasks are persistent by default; if yes, block untagged `wg add` from disposable scope except a narrow ephemeral/subtask mode and add regression tests.

5. **Hold #49 and #55 until nightly is green; merge #55 before the larger Telegram stack if urgent.**
   PRs: [#49](https://github.com/graphwork/wg/pull/49) and [#55](https://github.com/graphwork/wg/pull/55).
   Status: both `MERGEABLE` but `UNSTABLE` because `Build & Test (nightly)=FAILURE`; stable build/test, stable integration, lint, and pi-plugin checks are green; no reviews/comments.
   Overlap: both touch `src/commands/telegram.rs`; #49 also changes `src/cli.rs`, `src/main.rs`, and agency human-binding modules.
   Recommended action: fix or re-run nightly before merge. If the panic fix is urgent, merge the smaller #55 first once green, then rebase #49.

## One-line PR inventory

- [#49](https://github.com/graphwork/wg/pull/49) `feat: wg agency human add - Telegram onboarding handshake (R21/R22)`: adds `wg agency human add/confirm`, Telegram binding map, human executor classification, and listener confirmation hook; `MERGEABLE` but `UNSTABLE` due nightly failure; wait/re-run CI.
- [#50](https://github.com/graphwork/wg/pull/50) `feat: bind named-agent identity to a persistent session (R2)`: binds agency agents to persistent chat sessions and injects session summaries into prompts; all checks green, `MERGEABLE`, `CLEAN`; merge after normal review.
- [#51](https://github.com/graphwork/wg/pull/51) `feat: wire the human-task dispatch tail (R10/R11/R13)`: parks human tasks, notifies via Telegram, routes replies, and completes tasks from replies; all checks green, `MERGEABLE`, `CLEAN`; request changes for sender-binding authorization.
- [#52](https://github.com/graphwork/wg/pull/52) `fix(platform_timeout): probe gtimeout/timeout, watchdog fallback on macOS without coreutils`: isolated timeout compatibility fix with watchdog fallback; all checks green, `MERGEABLE`, `CLEAN`; merge.
- [#53](https://github.com/graphwork/wg/pull/53) `test: fix flaky session_lock PID-reuse tests`: isolated Linux PID-reuse test stabilization; all checks green, `MERGEABLE`, `CLEAN`; merge.
- [#54](https://github.com/graphwork/wg/pull/54) `fix: deliverable-preflight false positives on discard/external-worktree files`: improves deliverable parsing but adds an agent-usable preflight bypass; all checks green, `MERGEABLE`, `CLEAN`; request changes.
- [#55](https://github.com/graphwork/wg/pull/55) `fix: don't panic in telegram listen banner on multi-bot-only config`: small Telegram banner panic fix; `MERGEABLE` but `UNSTABLE` due nightly failure; fix/re-run CI, then merge before #49/#51.
- [#56](https://github.com/graphwork/wg/pull/56) `feat: --scope disposable guard on wg add (R8)`: adds `--scope`, propagates `WG_SCOPE`, and blocks agent creation plus explicitly persistent-tagged tasks; all checks green, `MERGEABLE`, `CLEAN`; request changes because untagged durable `wg add` still works.
- [#57](https://github.com/graphwork/wg/pull/57) `feat: evaluator executes validation commands (trust inversion fix)`: executes allowlisted validation commands during evaluation and caps score to 0 on failures; `MERGEABLE` but `UNSTABLE` due stable build/test failure; request changes for CI and command timeouts.

## Overlap and stacking

- `src/commands/telegram.rs`: #49, #51, and #55 overlap. #55 is the smallest panic fix. #49 adds onboarding confirmation. #51 should consume #49's binding map but currently does not, so #51 is conceptually stacked on #49 even though GitHub reports independent branches.
- `src/cli.rs` and `src/main.rs`: #49, #50, and #56 all add command surface. GitHub reports each branch mergeable against current `main`, but merging any one first may create rebase work for the others.
- `src/agency/mod.rs`: #49 and #57 both export new agency modules.
- #52 and #53 are isolated single-file fixes.
- #54 is isolated to deliverables/done, but it changes a core completion gate.

## Recommended action and merge order

1. Merge [#52](https://github.com/graphwork/wg/pull/52) after normal maintainer review.
2. Merge [#53](https://github.com/graphwork/wg/pull/53) after normal maintainer review.
3. Merge [#50](https://github.com/graphwork/wg/pull/50) after normal maintainer review; watch prompt-size and stale-memory UX as follow-up risk.
4. Fix/re-run [#55](https://github.com/graphwork/wg/pull/55), then merge once nightly is green.
5. Fix/re-run [#49](https://github.com/graphwork/wg/pull/49), then merge before #51 if the human-onboarding flow is desired.
6. Request changes on [#51](https://github.com/graphwork/wg/pull/51); rebase after #49 and route replies by confirmed Telegram binding.
7. Request changes on [#54](https://github.com/graphwork/wg/pull/54); keep parser fixes but remove or human-gate `WG_DELIVERABLE_PREFLIGHT_OVERRIDE=1`.
8. Request changes on [#56](https://github.com/graphwork/wg/pull/56); close only if ordinary `wg add` from disposable scope is intentionally allowed.
9. Request changes on [#57](https://github.com/graphwork/wg/pull/57); stable CI must pass and validation command execution needs per-command timeouts.

## Validation

- All nine PRs #49 through #57 are included with direct URLs.
- Findings and actions are explicit and include file/line references for substantive code findings.
- No GitHub state was modified. No source files were modified; this is a report artifact only.
