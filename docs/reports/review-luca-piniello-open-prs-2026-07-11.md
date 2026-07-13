# Review: Luca Pinello open pull requests

Review time: 2026-07-11 14:58 UTC.

Repository: `origin` is `git@github.com:graphwork/wg.git`, GitHub repo `graphwork/wg`, default branch `main`.

Author attribution: verified with `gh pr list --repo graphwork/wg --state open --json author`. The open PRs attributable to the requested Luca Piniello/name variant are by GitHub user `lucapinello`, display name `Luca Pinello`, user id `MDQ6VXNlcjEwODEzMjI=`. Text searches for open PRs containing `Luca Piniello`, `luca`, and `piniello` returned no additional PRs. I treat the task spelling `Piniello` as an uncertain spelling variant, not a separate verified identity.

No GitHub state was changed. No labels, branches, reviews, comments, or PR metadata were modified.

## Findings

1. **PR #57 should not merge: required stable CI is red, and the evaluator command execution path has no timeout.**
   Verified facts: [#57](https://github.com/graphwork/wg/pull/57) is mergeable but `mergeStateStatus=UNSTABLE`; CI has `Build & Test (stable)=FAILURE` and the other four checks green. There are no reviews or PR comments. Code risk: `src/agency/validation_exec.rs:356-361` runs task-authored validation commands through `sh -c` with `.output()` and no deadline. Even with the read-only allowlist, a task description can make evaluation hang on an allowed command such as a network-backed `gh pr view/list` or a blocking `grep` target. `src/commands/evaluate.rs:522-533` runs this inside evaluation for every task description with extracted commands. Request changes: fix the red stable job and put a hard timeout around each validation command before reconsidering.

2. **PR #51 should not merge before redesigning human reply routing around confirmed sender identity.**
   Verified facts: [#51](https://github.com/graphwork/wg/pull/51) has all five checks green, no reviews/comments, `MERGEABLE`, `CLEAN`. Code risk: `src/commands/service/human_dispatch.rs:321-365` routes any inbound non-command Telegram body to a waiting human task by receiving bot/channel and then newest parked task. Lines 340-342 explicitly allow a shared bot reply to answer any human's parked task, and lines 360-364 write the message without checking the sender against the Telegram binding map added in #49. That means in a shared bot setup, the wrong human can complete another human's task, and unconfirmed users can satisfy `WaitCondition::HumanInput`. This is a correctness and authorization boundary for human-as-agent dispatch. Request changes: make #51 depend on #49's binding map and require `sender -> confirmed binding -> agent_id` before routing or completing a human task.

3. **PR #54 weakens the deliverable gate with an agent-usable environment override.**
   Verified facts: [#54](https://github.com/graphwork/wg/pull/54) has all five checks green, no reviews/comments, `MERGEABLE`, `CLEAN`. The parsing fixes for discard/external-worktree false positives are directionally good, but `src/commands/done.rs:1520-1559` adds `WG_DELIVERABLE_PREFLIGHT_OVERRIDE=1`, which bypasses missing deliverables and lets the task become `Done`. The WG agent guide treats completion gates as hard because downstream evaluation relies on real artifacts. This env var is available to the same shell/process context as any agent running `wg done`, so it becomes an easy way to report success without producing required files. Request changes: keep the parser improvements, but either remove the override, gate it behind a human-only override comparable to smoke override policy, or make it leave a non-success task state requiring maintainer confirmation.

4. **PR #56 does not enforce its stated disposable-scope policy for ordinary `wg add`.**
   Verified facts: [#56](https://github.com/graphwork/wg/pull/56) has all five checks green, no reviews/comments, `MERGEABLE`, `CLEAN`. Code risk: the PR says a disposable-scoped agent cannot mint persistent tasks, and `src/dispatch/plan.rs:560-565` correctly propagates `WG_SCOPE` from `scope:<value>`. But enforcement in `src/scope_guard.rs:90-96` only rejects `wg add` when the new task is explicitly tagged `persistent`; `src/commands/add.rs:243-245` and `src/commands/add.rs:870-872` call only that tag-specific guard. A disposable worker can still run plain `wg add "new durable task"`, which creates a normal graph task. Request changes: define whether all `wg add` tasks are persistent by default; if yes, block all `wg add` from `WG_SCOPE=disposable` except a narrowly defined ephemeral/subtask mode and add a regression test for untagged `wg add`.

5. **PR #49 and #55 currently have failing nightly checks and overlap on Telegram listener code.**
   Verified facts: [#49](https://github.com/graphwork/wg/pull/49) and [#55](https://github.com/graphwork/wg/pull/55) are both `MERGEABLE` but `UNSTABLE` because `Build & Test (nightly)=FAILURE`; stable build/test, stable integration, lint, and pi-plugin checks are green. Both touch `src/commands/telegram.rs`, and #49 additionally changes `src/cli.rs`, `src/main.rs`, and agency human-binding modules. I could not retrieve failed job log text through `gh run view --log-failed`; the current check conclusion is still a blocker if the repository requires green CI. Recommendation: fix/re-run nightly before merge; merge the smaller #55 first if it is urgent and then rebase #49.

## PR Inventory

All open PRs by `lucapinello` at review time:

| PR | Age at review | Base <- head | Size | Purpose | Checks | Review/comment state | Mergeability |
| --- | --- | --- | --- | --- | --- | --- | --- |
| [#49](https://github.com/graphwork/wg/pull/49) `feat: wg agency human add - Telegram onboarding handshake (R21/R22)` | ~27.5h | `main` <- `feat/r21-human-add-handshake` | 9 files, +836/-1 | Adds `wg agency human add/confirm`, Telegram binding map, human executor classification, listener confirmation hook. | 4 success, nightly failure | No reviews, no comments | `MERGEABLE`, `UNSTABLE` |
| [#50](https://github.com/graphwork/wg/pull/50) `feat: bind named-agent identity to a persistent session (R2)` | ~27.5h | `main` <- `feat/r2-agent-session-binding` | 6 files, +446/-0 | Binds agency agents to persistent chat sessions and injects bound session summaries into prompts. | 5 success | No reviews, no comments | `MERGEABLE`, `CLEAN` |
| [#51](https://github.com/graphwork/wg/pull/51) `feat: wire the human-task dispatch tail (R10/R11/R13)` | ~27.4h | `main` <- `feat/r10-human-dispatch-tail` | 5 files, +701/-12 | Parks human-assigned tasks, notifies via Telegram, routes inbound replies, completes human tasks from replies. | 5 success | No reviews, no comments | `MERGEABLE`, `CLEAN` |
| [#52](https://github.com/graphwork/wg/pull/52) `fix(platform_timeout): probe gtimeout/timeout, watchdog fallback on macOS without coreutils` | ~27.1h | `main` <- `fix/platform-timeout-probe` | 1 file, +275/-46 | Resolves `gtimeout`/`timeout`, adds watchdog fallback when GNU timeout is unavailable. | 5 success | No reviews, no comments | `MERGEABLE`, `CLEAN` |
| [#53](https://github.com/graphwork/wg/pull/53) `test: fix flaky session_lock PID-reuse tests` | ~22.7h | `main` <- `fix/flaky-session-lock-tests-v2` | 1 file, +53/-12 | Makes Linux PID-reuse tests wait for child `/proc/<pid>/comm` to reflect post-exec image. | 5 success | No reviews, no comments | `MERGEABLE`, `CLEAN` |
| [#54](https://github.com/graphwork/wg/pull/54) `fix: deliverable-preflight false positives on discard/external-worktree files` | ~15.5h | `main` <- `fix/bug-deliverable-preflight` | 2 files, +217/-3 | Avoids false deliverables from discard wording and external worktree validation fallbacks; adds preflight override. | 5 success | No reviews, no comments | `MERGEABLE`, `CLEAN` |
| [#55](https://github.com/graphwork/wg/pull/55) `fix: don't panic in telegram listen banner on multi-bot-only config` | ~14.1h | `main` <- `fix/listen-banner-multibot-upstream` | 1 file, +90/-5 | Replaces direct token slicing with a `bot_banner` helper that handles multi-bot-only config. | 4 success, nightly failure | No reviews, no comments | `MERGEABLE`, `UNSTABLE` |
| [#56](https://github.com/graphwork/wg/pull/56) `feat: --scope disposable guard on wg add (R8)` | ~12.4h | `main` <- `r8-scope-guard` | 8 files, +237/-0 | Adds `--scope`, propagates `WG_SCOPE`, blocks disposable workers from agent creation and `--tag persistent` tasks. | 5 success | No reviews, no comments | `MERGEABLE`, `CLEAN` |
| [#57](https://github.com/graphwork/wg/pull/57) `feat: evaluator executes validation commands (trust inversion fix)` | ~12.1h | `main` <- `wg-eval-exec` | 3 files, +609/-1 | Runs allowlisted validation commands during evaluation and caps score to 0 on failures. | 4 success, stable build/test failure | No reviews, no comments | `MERGEABLE`, `UNSTABLE` |

## Dependencies And Overlap

Verified file overlap:

- `src/commands/telegram.rs`: #49, #51, #55 overlap. #55 is the smallest panic fix; #49 adds onboarding confirmation; #51 should consume #49's sender binding but currently does not.
- `src/cli.rs` and `src/main.rs`: #49, #50, #56 all add command surface. GitHub says each branch is mergeable against current `main`, but merging any one first can force the others to rebase if hunks drift.
- `src/agency/mod.rs`: #49 and #57 both export new agency modules.
- #52 and #53 are isolated single-file fixes.
- #54 is isolated to deliverables/done but changes a core completion gate.

Inference: #49 and #51 are conceptually stacked even though GitHub reports independent branches. #51's safe implementation should use #49's Telegram binding map, so #51 should wait for #49 or be rebased to include the binding dependency. #55 is a small independent fix in the same listener file and should merge before #49/#51 to reduce rebase surface.

## Suggested Merge/Rebase/Close Order

1. Merge #52 after normal maintainer review. It is isolated, green, and fixes a platform compatibility bug.
2. Merge #53 after normal maintainer review. It is isolated, green, and only stabilizes tests.
3. Merge #50 after normal maintainer review. It is green and architecturally coherent; residual risk is prompt bloat/stale memory from injecting `session-summary.md`, but no blocking issue was found.
4. Fix/re-run #55, then merge if nightly is green. It is the right small Telegram panic fix and should land before larger Telegram listener PRs.
5. Request changes on #49 until nightly is green; then merge before #51 if the human-onboarding flow is desired.
6. Request changes on #51. Rebase after #49 and route replies by confirmed Telegram binding rather than only bot/channel/newest waiting task.
7. Request changes on #54. Keep parser fixes, remove or human-gate the deliverable preflight override.
8. Request changes on #56. Close only if the project decides ordinary `wg add` from disposable scope is intentionally allowed; otherwise expand enforcement and tests.
9. Request changes on #57. Stable CI must pass, and validation command execution needs per-command timeouts before this changes evaluator behavior.

## Final Action Table

| PR | Recommendation | Rationale |
| --- | --- | --- |
| [#49](https://github.com/graphwork/wg/pull/49) | Wait / re-run CI | Useful onboarding base for #51, but nightly is red and it overlaps Telegram listener code. |
| [#50](https://github.com/graphwork/wg/pull/50) | Merge | Green, clean, no blocking code issue found; review prompt-size/stale-memory UX as follow-up risk. |
| [#51](https://github.com/graphwork/wg/pull/51) | Request changes | Human replies are routed/completed without confirmed sender-to-agent binding; should stack on #49. |
| [#52](https://github.com/graphwork/wg/pull/52) | Merge | Isolated macOS/Unix timeout compatibility fix with focused tests and green CI. |
| [#53](https://github.com/graphwork/wg/pull/53) | Merge | Isolated test flake fix with green CI. |
| [#54](https://github.com/graphwork/wg/pull/54) | Request changes | Parser fixes are good, but `WG_DELIVERABLE_PREFLIGHT_OVERRIDE=1` lets agents bypass a completion gate. |
| [#55](https://github.com/graphwork/wg/pull/55) | Wait / re-run CI | Small correct-looking panic fix, but nightly is red; merge before #49/#51 once green. |
| [#56](https://github.com/graphwork/wg/pull/56) | Request changes | Disposable scope still permits ordinary durable `wg add`; enforcement does not match stated R8 policy. |
| [#57](https://github.com/graphwork/wg/pull/57) | Request changes | Stable CI is red and validation command execution lacks a timeout. |

## Validation Notes

- Accounted for every open PR by verified GitHub author `lucapinello`: #49 through #57, each with direct URL above.
- CI/review/mergeability facts came from `gh pr view` at 2026-07-11 14:58 UTC.
- Review state: all nine PRs had no submitted reviews and no PR comments in `gh pr view`.
- Comments and review logs were accessible; failed job log text was not returned by `gh run view --log-failed`, so CI failure conclusions are verified but the exact failure output is not included.
- No repository or GitHub state was changed except for adding this local report artifact on the task branch.
