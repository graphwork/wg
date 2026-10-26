# Luca Pinello Casa/WG fork inventory and coordination draft

**Snapshot:** 2026-07-16 11:49–12:00 UTC<br>
**Upstream:** [`graphwork/wg`](https://github.com/graphwork/wg), `origin/main` = [`fb19f595`](https://github.com/graphwork/wg/commit/fb19f5955780130a0ae587e735d9255bffb66026)<br>
**Fork:** [`lucapinello/workgraph`](https://github.com/lucapinello/workgraph), [`integration/casa-pinello`](https://github.com/lucapinello/workgraph/tree/integration/casa-pinello) = [`42166fe0`](https://github.com/lucapinello/workgraph/commit/42166fe0c435d917dbbdd2a27325631b9a4d2eee)

This is a read-only inventory and a proposed coordination plan. It is **not** a merge review or an authorization to post the message at the end. During this task no PR, review, comment, issue, branch, or source file was changed; only this report was added.

## Executive summary

- The headline estimate is still exact after refresh: `integration/casa-pinello` is **125 commits ahead and 5 behind** current `origin/main` (`git rev-list --left-right --count` prints `5 125`). The 125 are **86 non-merge/source commits plus 39 merge commits**. All 86 source commits are patch-unique according to `git cherry`, although several are semantically duplicate or superseded by revised upstream PR heads.
- The fork has **21 internal PRs**: **20 merged, 1 closed unmerged**. The source heads of 20/21 are contained in the integration branch. Closed PR [fork #2](https://github.com/lucapinello/workgraph/pull/2), the per-agent 1:1 ledger writer, is the only source head not contained. Nineteen fork PR merge commits are ancestors; fork #4 merged through an intermediate Casa branch, so its source commit—not its merge wrapper—is in integration.
- This is not merely the upstream #49–#57 stack. The integration line contains a large Casa conversation/product layer, but also small generally reusable WG fixes: detached-process PID tracking, `FailedPendingEval` rescue deadlock prevention, provider-failure classification, provider auto-recovery, cron protection/re-registration, auth diagnostics, listener token redaction, and listener connection reuse.
- Work already upstream or in the active upstream lane must not be re-imported from integration. Upstream [#49](https://github.com/graphwork/wg/pull/49), [#50](https://github.com/graphwork/wg/pull/50), [#52](https://github.com/graphwork/wg/pull/52), [#53](https://github.com/graphwork/wg/pull/53), and [#55](https://github.com/graphwork/wg/pull/55) are merged. [#51](https://github.com/graphwork/wg/pull/51), [#54](https://github.com/graphwork/wg/pull/54), [#56](https://github.com/graphwork/wg/pull/56), and [#57](https://github.com/graphwork/wg/pull/57) remain open. The integration copies are older or partial heads.
- The branch cannot be treated as one merge candidate. A read-only `git merge-tree` predicts direct conflicts with final #49 in nine high-traffic files, and the full tip diff is **100 files, +41,622/-1,067**. The right unit is a dependency-ordered set of focused batches, each rebuilt on current `main` and independently tested.
- Communication was detailed on upstream PRs, not systematic across the broader fork stream. Maintainer `ekg` submitted nine reviews across #49–#57 (changes requested and later approvals where appropriate) plus detailed issue comments. In contrast, the 21 fork PRs have no submitted reviews; only fork #2 has a PR comment, by Luca himself. GitHub searches found no `graphwork/wg` comment mentioning `integration/casa-pinello`/`casa-pinello`, and no `ekg` comment in the fork. That supports the narrow statement that we reviewed the upstream PR lane but did not visibly acknowledge or coordinate the broader stream; it does not claim that no private conversation occurred.

## Reproducible snapshot and counts

Commands used after read-only fetches:

```bash
git fetch origin main
git fetch luca '+refs/heads/*:refs/remotes/luca/*'
date -u +'%Y-%m-%dT%H:%M:%SZ'
git rev-parse origin/main luca/integration/casa-pinello luca/main
git rev-list --left-right --count origin/main...luca/integration/casa-pinello
git merge-base origin/main luca/integration/casa-pinello
git rev-list --count origin/main..luca/integration/casa-pinello
git rev-list --count --merges origin/main..luca/integration/casa-pinello
git rev-list --count --no-merges origin/main..luca/integration/casa-pinello
git cherry origin/main luca/integration/casa-pinello | cut -c1 | sort | uniq -c
git diff --shortstat origin/main luca/integration/casa-pinello
```

Observed:

```text
snapshot                    2026-07-16T11:49:44Z
origin/main                 fb19f5955780130a0ae587e735d9255bffb66026
integration/casa-pinello    42166fe0c435d917dbbdd2a27325631b9a4d2eee
luca/main                   fb19f5955780130a0ae587e735d9255bffb66026
ahead/behind output         5 125   # left-only origin, right-only integration
merge-base                  583808834860f31b58c40b1896f2d9d4f9bbfea1
unique total                125
unique merges               39
unique non-merges           86
git cherry                  86 +, 0 -
tip-to-tip diff             100 files, 41622 insertions, 1067 deletions
```

`git cherry` answers patch identity, not semantic identity. For example, old integration implementations of #51/#55 are patch-unique against current main even though a revised or merged upstream PR supersedes them. Appendix A therefore groups all 86 commits by provenance and intended action rather than presenting them as a cherry-pick queue.

The five upstream-only commits are final #49 history: merge [`fb19f595`](https://github.com/graphwork/wg/commit/fb19f5955780130a0ae587e735d9255bffb66026), branch merge [`39f150f8`](https://github.com/graphwork/wg/commit/39f150f834f6bbfffc530da4ce29358a7250dfd6), maintainer round-2 fix [`aa359ff2`](https://github.com/graphwork/wg/commit/aa359ff296e4a2bd71df889988870973ac0b5faf), earlier main merge [`71b64bb4`](https://github.com/graphwork/wg/commit/71b64bb4ea34a90646a7d807ca11a594567c93b2), and session-lock fix [`1f925ceb`](https://github.com/graphwork/wg/commit/1f925cebdec12e6b76174d883c5778284c1174c4).

## Branch coverage and conflict surface

The fetch exposed **100** fork remote refs. Fifty-six have a tip dated July 2026; these, all 21 fork PR heads, the integration branch, and every upstream Luca PR head were considered. Forty-four refs last updated before July were explicitly scoped out of this Casa-stream analysis as historical branch inventory. Two older exceptions are still accounted for through upstream PRs: `feat/telegram-multi-bot` via merged #37 and `fix/timeout-binary-probe` via closed #36/superseding #52.

Reproduce the active-ref cut:

```bash
git for-each-ref \
  --format='%(refname:short)%09%(committerdate:short)%09%(objectname)%09%(subject)' \
  refs/remotes/luca | sort
# 100 total; 56 with date >= 2026-07-01, 44 older.
```

Recent branch tips not literally ancestors of integration are all explainable:

| Branch | Status relative to integration | Accounting |
|---|---|---|
| [`casa/fix-converse-hang`](https://github.com/lucapinello/workgraph/tree/casa/fix-converse-hang) | tip SHA differs; `git cherry integration branch` marks it `-` | Patch-equivalent integration commit [`9f9377ce`](https://github.com/lucapinello/workgraph/commit/9f9377ce1d3a0b8c05ad4f3d861bf76da93e8295); duplicate. |
| [`casa/listener-log-rejected-noise-v2`](https://github.com/lucapinello/workgraph/tree/casa/listener-log-rejected-noise-v2) | one merge wrapper ahead | Fork #4 merged into this intermediate branch; source [`8e15c9c1`](https://github.com/lucapinello/workgraph/commit/8e15c9c195595456614b0a30e3660f1026a0fc7f) is in integration. |
| [`feat/r10-human-dispatch-tail`](https://github.com/lucapinello/workgraph/tree/feat/r10-human-dispatch-tail) | two commits ahead of current main, not integration | Current upstream #51; use this revised head, not the older integration copy. |
| [`feat/r21-human-add-handshake`](https://github.com/lucapinello/workgraph/tree/feat/r21-human-add-handshake) | no patch unique against current main | Merged upstream #49; duplicate/superseded in integration. |
| [`wg/agent-893/listener-write-inbound-1-1`](https://github.com/lucapinello/workgraph/tree/wg/agent-893/listener-write-inbound-1-1) | one source commit not in integration | Closed unmerged fork #2; needs Luca's intent before any action. |

A read-only three-way `git merge-tree $(git merge-base ...) origin/main integration` reports nine direct conflict paths against final #49:

- added in both: `src/agency/human_binding.rs`, `src/commands/agency_human.rs`;
- changed in both: `src/agency/mod.rs`, `src/cli.rs`, `src/commands/mod.rs`, `src/commands/telegram.rs`, `src/main.rs`, `src/notify/mod.rs`, `src/notify/telegram.rs`.

This is the immediate conflict surface only. The 100-file branch also changes `done`, `cron`, provider health, dispatcher/service code, the grow-only smoke manifest, and many Telegram modules; later upstream changes will increase rebase risk. No merge or rebase was attempted.

## All 21 fork-internal PRs

CI evidence below is GitHub check state, not independent maintainer validation. Fork #1 and #2 report no checks. At the recorded heads, #3–#21 have stable build/test, stable integration, and pi-plugin checks passing; **Check & Lint fails on every #3–#21 head**, and nightly fails on #3 and #5–#21 (nightly passes on #4). The latest integration tip has no check runs. PR bodies often report focused local tests, but those claims must be re-run after extraction/rebase.

| Fork PR | State / integration | What it contributes | Classification / next action |
|---|---|---|---|
| [#1](https://github.com/lucapinello/workgraph/pull/1) | merged; source in integration | Casa group-feed JSONL writer, privacy allowlist, smoke | **Casa-only/later** unless WG adopts the Casa ledger contract. |
| [#2](https://github.com/lucapinello/workgraph/pull/2) | **closed unmerged**; source absent | Per-agent 1:1 ledger, replay, privacy smoke | **Needs discussion**: ask whether closure means abandoned, replaced, or intended later. |
| [#3](https://github.com/lucapinello/workgraph/pull/3) | merged; in integration | Distinguishes benign non-parked reply from security rejection | **Later**, after #51 identity/router settles. |
| [#4](https://github.com/lucapinello/workgraph/pull/4) | merged via intermediate branch; source in integration | Web-login nonce confirmation to loopback Casa gateway | **Casa/security discussion**; gateway-coupled and must receive threat review. |
| [#5](https://github.com/lucapinello/workgraph/pull/5) | merged; in integration | Bots-map fallback for Casa web inbound | **Casa adapter**; reusable fallback can be extracted after #51. |
| [#6](https://github.com/lucapinello/workgraph/pull/6) | merged; in integration | Off-domain task ownership choke point | **Casa-only** household-domain policy. |
| [#7](https://github.com/lucapinello/workgraph/pull/7) | merged; in integration | Domain-aware household voice election | **Casa-only** unless generalized behind policy/config. |
| [#8](https://github.com/lucapinello/workgraph/pull/8) | merged; in integration | Greeting precedence/defer/family-safe lifecycle names | **Casa product**, with potentially reusable no-ID-leak tests. |
| [#9](https://github.com/lucapinello/workgraph/pull/9) | merged; in integration | Direct plan-edit fast lane | **Casa-only** until a generic transactional fast-lane interface exists. |
| [#10](https://github.com/lucapinello/workgraph/pull/10) | merged; in integration | Lifecycle report-backs through ledger + delivery verification | **Later/needs discussion**; useful semantics, Casa feed dependency. |
| [#11](https://github.com/lucapinello/workgraph/pull/11) | merged; in integration | Provider probe, auto-resume, status and alerts | **Later reusable WG** after provider classification #14. |
| [#12](https://github.com/lucapinello/workgraph/pull/12) | merged; in integration | Protected cron tasks, `cron --rearm`, terminal warning | **Later reusable WG**, rebase and re-test cron state semantics. |
| [#13](https://github.com/lucapinello/workgraph/pull/13) | merged; in integration | `FailedPendingEval` system-satellite completion bypass | **Upstream-now candidate**, small and independent. |
| [#14](https://github.com/lucapinello/workgraph/pull/14) | merged; in integration | Do not count `wg done` refusals as provider outages | **Upstream-now candidate**, but audit string classifier and tests. |
| [#15](https://github.com/lucapinello/workgraph/pull/15) | merged; in integration | Typo-tolerant food classification, clarification continuation, web parity | **Casa-only** classifier/product behavior. |
| [#16](https://github.com/lucapinello/workgraph/pull/16) | merged; in integration | Production daily-digest flush | **Casa/later**, depends lifecycle/ledger batch. |
| [#17](https://github.com/lucapinello/workgraph/pull/17) | merged; in integration | Protected-cron smoke | **Later**, ship with #12 rather than separately. |
| [#18](https://github.com/lucapinello/workgraph/pull/18) | merged; in integration | Ask-to-dish transform and semantic gate | **Casa-only**, depends #9. |
| [#19](https://github.com/lucapinello/workgraph/pull/19) | merged; in integration | Grounded/corrigible/repetition-guarded conversation | **Casa product/needs discussion**; reusable ideas, household data model. |
| [#20](https://github.com/lucapinello/workgraph/pull/20) | merged; in integration | Track actual detached command PID using `exec` + in-process `setsid` | **Upstream-now candidate**, isolated one-file OS fix. |
| [#21](https://github.com/lucapinello/workgraph/pull/21) | merged; in integration | Direct, scoped, clock-aware answer shape | **Casa-only**, depends #19 grounding. |

## Upstream Luca PR inventory

`gh pr list --repo graphwork/wg --state all --author lucapinello --limit 100` returned exactly these 11 PRs; there are no other active upstream PRs by that account at the snapshot.

| PR | Current state | Accounting |
|---|---|---|
| [#36](https://github.com/graphwork/wg/pull/36) timeout binary probe | closed, unmerged | Superseded by the more complete merged #52; no action. |
| [#37](https://github.com/graphwork/wg/pull/37) multi-bot Telegram | merged 2026-06-25 | Already upstream foundation; do not import fork copy. |
| [#49](https://github.com/graphwork/wg/pull/49) human onboarding | merged 2026-07-16 | Final `fb19f595`; integration is behind this final round. |
| [#50](https://github.com/graphwork/wg/pull/50) persistent named-agent session | merged 2026-07-13 | Already upstream; integration fixture repair is duplicate. |
| [#51](https://github.com/graphwork/wg/pull/51) human dispatch tail | **open**, head `f16d86bf`, CLEAN/MERGEABLE, all checks green | Previous review requested current-main refresh, green nightly, and handle-binding parity; current head postdates that review and appears to address them. **Await exact-head human re-review; do not substitute integration's old commits.** |
| [#52](https://github.com/graphwork/wg/pull/52) platform timeout | merged 2026-07-11 | Already upstream; supersedes #36. |
| [#53](https://github.com/graphwork/wg/pull/53) session-lock test flake | merged 2026-07-11 | Already upstream and also absorbed by later PR histories. |
| [#54](https://github.com/graphwork/wg/pull/54) deliverable preflight | **open**, head `11c4f695`, all recorded checks green | Author removed the unsafe override requested in review; review decision remains CHANGES_REQUESTED. Re-review current head; no fork re-import. |
| [#55](https://github.com/graphwork/wg/pull/55) listen banner panic | merged 2026-07-13 | Already upstream; fork `79b0900e` is an older implementation. |
| [#56](https://github.com/graphwork/wg/pull/56) disposable scope guard | **open**, head `b97ba03d`; stable/lint/integration/pi green, nightly red | Author added later default-deny revisions beyond integration's `bbd699d4`; review decision remains CHANGES_REQUESTED. Resolve nightly and re-review exact head. |
| [#57](https://github.com/graphwork/wg/pull/57) evaluator validation execution | **open**, head `0925c010`; stable/lint/integration/pi green, nightly red | Current head includes later deterministic session-lock/fmt fixes not in integration; review decision remains CHANGES_REQUESTED. Resolve nightly and re-review exact head. |

## Categorized action matrix

“Upstream now” means prepare a focused current-main branch and review **after human authorization**; it does not mean this task merged anything.

| Batch | Included work / direct anchors | Action | Dependencies and conflict surface | Test evidence and required independent proof |
|---|---|---|---|---|
| U0: finish active upstream lane | #51, #54, #56, #57 | **Upstream-now / already in review** | #51 after merged #49; others mostly independent, but #56/#57 touch CLI/agency shared files. | #51/#54 green; #56/#57 nightly red. Exact-head review and required CI first. |
| U1: detached background PID | fork #20 / [`d25cdd59`](https://github.com/lucapinello/workgraph/commit/d25cdd594dba4789eba310891cc2caed2ce1975d) | **Upstream-now** | One file, `src/executor/native/background.rs`; independent of Casa. `unsafe pre_exec`/Unix portability deserves focused review. | Author reports 10/10 kill repetitions and full lib run with documented baseline failures. Rebase, Linux+macOS test, repeated kill/cancel smoke. |
| U2: failed-eval rescue deadlock | fork #13 / [`9a595528`](https://github.com/lucapinello/workgraph/commit/9a5955288dc8419f2efff10986cfe25d8f3bc1a9) | **Upstream-now** | Small `src/commands/done.rs` change; current main still bypasses only `PendingEval`, confirming bug remains. | Author reports focused `rescue`, done, query suites green. Independently reproduce full crash→FLIP→evaluate path and regular-dependent negative. |
| U3: provider failure classification | fork #14 / [`02204b19`](https://github.com/lucapinello/workgraph/commit/02204b19ce5d9f15a7706febcda43cd67a84d9a0) | **Upstream-now** | `provider_health.rs`; land before auto-probe U4. String signatures can under/over-classify, so keep narrow. | Author reports refusal matrix and real-401 negative. Add current wrapper stderr fixtures and provider-counter smoke. |
| U4: provider recovery and spawn breaker | fork #11 [`afd79373`](https://github.com/lucapinello/workgraph/commit/afd79373befd6a41d12f11be1fe94f48b042bba7), [`9b823397`](https://github.com/lucapinello/workgraph/commit/9b82339722b2f315736d3c9cefc65970419f2d14) | **Later** | After U3. Touches config, daemon, service status, alerts; policy and provider-specific probe command need design review. | Fork focused tests claimed, but no clean fork lint/nightly at tip. Independent pause→probe→resume human-flow plus auth-outage negative. |
| U5: cron correctness/protection | [`b5987388`](https://github.com/lucapinello/workgraph/commit/b5987388a4d208db80d1bcdd5dcc795faed7cab6), fork #12/#17 | **Later** | Re-registration/instance semantics first; protection/rearm/UI second. Touches graph, cron, abandon, gc, publish, manifest. | Author unit/focused smoke claims; rebase against current cron missed-fire behavior and run permanent smoke independently. |
| U6: Telegram runtime hardening | token redact [`57e94067`](https://github.com/lucapinello/workgraph/commit/57e9406753b794daf30435bd14f9ee4a0207898b), connection reuse/bots fallback [`ccc9597a`](https://github.com/lucapinello/workgraph/commit/ccc9597aeef0841f264258d4fcd2c7ceda894511) | **Upstream-now after #51**, split into small PRs | `telegram.rs` is a direct #49/#51 conflict hotspot. Extract behavior, do not merge integration file. | Add secret-redaction fixture, reconnect loop/resource test, and bots-map-only send/listen human flow on current main. |
| U7: generic Telegram conversation substrate | 24 commits in Appendix A: multi-bot polling/dedupe, 1:1 compose, mention/reply/election, discussion, buttons | **Later / needs discussion** | Stack after #51 and U6: polling→dedupe/sender identity→1:1 compose→addressing/election→discussion/buttons. Separate Casa names/ownership from generic policy. | Many focused and smoke claims, but fork lint/nightly red and no exact integration CI. Rebuild batches with current-main live/PTY or mock Bot API flows. |
| U8: disposable follow-ons | [`a3d3a513`](https://github.com/lucapinello/workgraph/commit/a3d3a5131121440ba19a053ea59a9135c7a8f8f7), [`9295fd09`](https://github.com/lucapinello/workgraph/commit/9295fd09093698b52f2d4e353f4c74f6ab3089b1) | **Later** | Only after policy in #56 and evaluator behavior in #57 settle. Completion gates are security-sensitive. | Rebase; test hard refusal, artifact/log provenance, session ingestion, and non-disposable negative. |
| D1: auth/setup token handling | [`e2ebba9c`](https://github.com/lucapinello/workgraph/commit/e2ebba9c9cb24f573ac89c5c071f70b7960b93bf), [`c3810107`](https://github.com/lucapinello/workgraph/commit/c38101076162fb65d9921bc8151be42f67cd3cb2) | **Needs discussion / security review** | Core auth/config/doctor behavior; may overlap newer handler-first/provider-login work on main. | Threat-model token file permissions, precedence, log redaction, headless flow; do not import based on live success alone. |
| C1: Casa gateway/identity/ledger | fork #1/#2/#4/#5, shared-secret [`a03e82ea`](https://github.com/lucapinello/workgraph/commit/a03e82ea7afa0789b9cdb24819a8bd846d9a3278), web inbound | **Needs discussion; Casa-only by default** | External Casa gateway contracts, `.casa/*` storage, loopback nonce flow. #2 is explicitly closed. | Security/privacy review of nonce, sender id, file permissions, replay, shared secret, and failure behavior. Confirm ownership/intended upstream API with Luca first. |
| C2: Casa family product | plan fast lane, household ownership/voices, photos, meal feedback, reminders, grounding/answer shape, family commands | **Casa-only by default** | Depends C1 and generic U7. Keep out of core unless maintainers choose a plugin/policy interface. | Preserve in Casa integration; later extract only generic interfaces with product-neutral fixtures. |
| C3: lifecycle/report-back/digest | lifecycle, parity, errand, daily digest, ledger delivery verification | **Needs discussion / later** | Useful generic semantics but currently coupled to Casa personas/feed/plan. Split core origin+delivery state machine from Casa renderers/sinks. | Independent exactly-once, retry, restart, wrong-bot, and no-1:1-feed-leak tests. |
| X1: upstream duplicates/repairs | 17 commits in first Appendix A group, plus integration merge wrappers for #49/#50/#52/#53/#55 | **Duplicate/superseded** | Use current upstream PR/merge heads only. | No testing needed for import; compare only to ensure no fork-only regression got lost. |

## Security-sensitive items requiring explicit review

1. **Human sender authorization:** #49 and #51 are the authorization base. Generic conversation work must preserve numeric sender identity, handle-binding parity, confirmed binding, receiving-bot identity, and wrong-human/shared-bot negatives.
2. **Completion/scope trust boundaries:** #54, #56, #57, disposable enforcement/ingestion, and `FailedPendingEval` bypass all change who can complete, create, or validate work. Review as policy, not convenience features.
3. **Secrets and identity:** auth token-file discovery, `auth-confirm` shared-secret headers, web login nonces, Telegram token redaction, feed/ledger allowlists, and bot API error logging need threat review and file/log permission checks.
4. **Process control:** the background PID fix uses Unix `pre_exec`/`setsid`; provider probes and spawn breakers execute commands and auto-change service state. Bound commands, timeouts, attribution, and operator visibility matter.
5. **Inbound content and dedupe:** multi-bot polling, content-fingerprint dedupe, stale backlog, bot-loop guards, web inbound, and clarification continuation determine whether untrusted/replayed content becomes a task. Test replay, collision, stale update, and cross-human cases.

## Proposed dependency-aware upstream plan

1. **Close the lane already open:** exact-head review #51, #54, #56, #57; land only when their required checks/reviews are satisfied. This removes stale duplicate commits from consideration and stabilizes human identity/scope/evaluation APIs.
2. **Land three independent core fixes:** U1 background PID, U2 failed-eval rescue, U3 provider classification. Each should be a new current-main branch with its original focused tests and a maintainer-owned regression. They can be reviewed in parallel after U0 because their file sets are disjoint.
3. **Build operational follow-ons:** U4 provider recovery after U3; U5 cron in two commits/batches. Do not mix alerts/digests or Casa policy into these engine PRs.
4. **Harden Telegram runtime after #51:** extract token redaction first, then client reuse/bots fallback. Preserve #49/#51 identity contracts. Avoid copying the monolithic `telegram.rs` diff.
5. **Agree on the product boundary with Luca:** decide whether the generic conversation substrate belongs in WG core, a WG plugin/module, or Casa. If core, sequence polling→dedupe/identity→compose→routing→discussion/buttons with product-neutral configuration and tests.
6. **Keep Casa adapters and household behavior together:** ledger/gateway, family plan, domain ownership, photos, reminders, grounding, and answer-shape remain on the Casa line unless a specific abstraction is accepted. The closed 1:1 ledger PR #2 needs an explicit intent decision.
7. **Split lifecycle semantics from renderers:** only after the boundary decision, propose a small origin/delivery/exactly-once core followed by Casa feed/digest/errand adapters.

For every extracted batch: rebase/reimplement on current `origin/main`; run pinned `cargo fmt --check`, `cargo clippy`, stable and nightly unit/integration suites; run the relevant permanent smoke/human flow; record which failures reproduce on pristine main. The existing fork evidence is valuable but not a substitute for this independent gate.

## Communication audit

### What did happen

- The repository already contains the factual review report [`docs/reports/review-luca-piniello-open-prs-2026-07-11.md`](./review-luca-piniello-open-prs-2026-07-11.md) and summary [`docs/reports/summarize-luca-prs-2026-07-11.md`](./summarize-luca-prs-2026-07-11.md).
- Maintainer `ekg` gave detailed, code-specific reviews on the upstream lane:
  - #49 changes requested for numeric sender identity, multi-bot listener behavior, and confirmation ordering, then [approved the fixed exact head](https://github.com/graphwork/wg/pull/49#pullrequestreview-4711697228).
  - #50 received [exact-head approval with local/CI evidence](https://github.com/graphwork/wg/pull/50#pullrequestreview-4683595409).
  - #51 received an initial [sender-authorization review](https://github.com/graphwork/wg/pull/51#pullrequestreview-4678151476) and a later [post-#49 exact-head re-review](https://github.com/graphwork/wg/pull/51#pullrequestreview-4711804260).
  - #54 received a [deliverable-bypass review](https://github.com/graphwork/wg/pull/54#pullrequestreview-4678151473).
  - #55 received [approval after exact-head/live-flow validation](https://github.com/graphwork/wg/pull/55#pullrequestreview-4683827417).
  - #56 received a [default-deny scope review](https://github.com/graphwork/wg/pull/56#pullrequestreview-4678151480).
  - #57 received a [validation timeout/CI review](https://github.com/graphwork/wg/pull/57#pullrequestreview-4678151479).
- Luca responded in detail on the PR threads, including [#49 round two](https://github.com/graphwork/wg/pull/49#issuecomment-4987622241), [#51 round two](https://github.com/graphwork/wg/pull/51#issuecomment-4987622331), [#54](https://github.com/graphwork/wg/pull/54#issuecomment-4987623489), [#56](https://github.com/graphwork/wg/pull/56#issuecomment-4987623336), and [#57](https://github.com/graphwork/wg/pull/57#issuecomment-4987623174).

### What the visible record does not show

- Across fork PRs #1–#21, the API returns **zero submitted reviews**. Only fork #2 has an issue comment, and it is by `lucapinello`; no `ekg` participation appears.
- These reproducible GitHub searches returned zero results:

```bash
gh api -X GET search/issues -f q='repo:graphwork/wg integration/casa-pinello in:comments'
gh api -X GET search/issues -f q='repo:graphwork/wg casa-pinello in:comments'
gh api -X GET search/issues -f q='repo:lucapinello/workgraph ekg in:comments'
```

Therefore the defensible conclusion is: **we coordinated the explicit upstream PRs in depth, but the visible GitHub record does not show systematic acknowledgment, prioritization, or an upstream boundary discussion for the broader Casa/integration stream.** Search cannot prove the absence of private/off-platform communication, so this report makes no such claim.

## Draft message to @lucapinello — human authorization required before posting

> @lucapinello — thank you for the substantial body of work in `integration/casa-pinello`, not just the upstream #49–#57 series. I inventoried the current fork line at 125 commits ahead / 5 behind upstream: it includes the 21 internal PRs, a broad Telegram/Casa conversation stack, and several generally reusable WG engine fixes. We have given detailed review to the upstream PR lane, and #49, #50, #52, #53 and #55 are now landed; #51, #54, #56 and #57 remain in exact-head review/CI. We have not yet coordinated the broader fork stream systematically, and I want to correct that rather than treat it as one giant cherry-pick list.
>
> Which of the Casa changes do you intend to propose for upstream WG, versus keep Casa-specific? My suggested order is: (1) finish the four active upstream PRs; (2) extract the small independent WG fixes—background PID tracking, FailedPendingEval rescue, and provider-failure classification; (3) handle provider/cron follow-ons; (4) agree on a product boundary before batching the Telegram conversation layer; and (5) keep gateway/ledger and household plan/voice behavior together as Casa adapters unless we agree on a generic interface. The closed fork PR #2 is also worth clarifying: abandoned, superseded, or intended for later?
>
> If that split matches your intent, we can share a batch checklist with dependencies and test gates and avoid making you repeatedly rebase the whole integration branch. Please correct any category or ordering I have misunderstood.

**Do not post this draft without explicit human approval.**

## Appendix A — all 86 patch-unique source commits, grouped

The grouping is the inventory and action model; it is deliberately not a flat cherry-pick prescription. Merge commits are accounted separately by the 39-count and fork PR table.

### Upstream PR lane / duplicate or superseded (17)
- [`72128ea4`](https://github.com/lucapinello/workgraph/commit/72128ea4f4f1c38f1ac7ecc953dc77674a9835c4) fix(human-dispatch): authorize inbound sender against confirmed binding (PR#51)
- [`738c88f5`](https://github.com/lucapinello/workgraph/commit/738c88f5024574ebd9e092e737d4abf301328193) fix: per-command validation timeout + green stable CI (pr57-timeout)
- [`87233bb9`](https://github.com/lucapinello/workgraph/commit/87233bb9d4be45457f33272fa4796da23c27b640) feat: evaluator executes validation commands (trust inversion fix)
- [`bbd699d4`](https://github.com/lucapinello/workgraph/commit/bbd699d4039a08fa9dd44ce22e334f08fb4e5746) fix(scope-guard): default-deny durable wg add from disposable scope (R8, PR #56)
- [`c7950a28`](https://github.com/lucapinello/workgraph/commit/c7950a28c4a7404042f327bd38453f7de3a6e7ea) feat: --scope disposable guard on wg add (R8)
- [`d4b45596`](https://github.com/lucapinello/workgraph/commit/d4b45596e99bbb69d38e8f996257e61d89667f61) test: failing test_scoped_disposable_cannot_spawn_persistent (R8)
- [`11c4f695`](https://github.com/lucapinello/workgraph/commit/11c4f6951e8463ca4cde81708bd385e8236318fe) fix: remove WG_DELIVERABLE_PREFLIGHT_OVERRIDE env bypass (pr54-override)
- [`342b69c9`](https://github.com/lucapinello/workgraph/commit/342b69c9c7ebbff34e18a2e93150b18a1f4ee23c) fix(tests): add missing choices field to graph::Task literals (fix-graph-task)
- [`107f1c40`](https://github.com/lucapinello/workgraph/commit/107f1c40042c195b355bbcd414eb901ec57d44ee) fix: 'wg agency human confirm' accepts agent id, clearer not-found error (fix-human-loop-wiring)
- [`24f69680`](https://github.com/lucapinello/workgraph/commit/24f696803dd7d76d8c7b96bc4a6cdd693ea28c76) fix: auto-assigner must not override explicit human assignment (fix-human-loop-wiring)
- [`4a09e7f6`](https://github.com/lucapinello/workgraph/commit/4a09e7f65e7466c8aaa18fae206d090cc40d9b5b) fix: confirm-first inbound routing so YES handshake never swallowed (fix-human-loop-wiring)
- [`79b0900e`](https://github.com/lucapinello/workgraph/commit/79b0900e65ee8cc3fce93248d45be96b4c1709ec) fix: don't panic in telegram listen banner on multi-bot-only config
- [`4144ef56`](https://github.com/lucapinello/workgraph/commit/4144ef5649803b395ad790634a281e7094fbaba3) fix: add bound_session_summary to TemplateVars test construction sites
- [`f17867a8`](https://github.com/lucapinello/workgraph/commit/f17867a8b3e89d87d34e478077ac0391df01b587) ci: re-run flaky session_lock tests (pre-existing flake, see #51/#52 discussion)
- [`49dbec49`](https://github.com/lucapinello/workgraph/commit/49dbec49ad7e8b26142b116156a6b5d04b96a0a4) fix: cargo fmt src/commands/service/human_dispatch.rs (fix-lint-pr51)
- [`31768fba`](https://github.com/lucapinello/workgraph/commit/31768fba22ca4b81364292c7265c8a467a711ed1) feat: wire the human-task dispatch tail (R10/R11/R13)
- [`43755390`](https://github.com/lucapinello/workgraph/commit/43755390842fcb8cefff82445b1aadf7371dd76f) fix: deliverable-preflight false-positives on discard/external-worktree files (bug-deliverable-preflight)

### Generally reusable WG core (12)
- [`d25cdd59`](https://github.com/lucapinello/workgraph/commit/d25cdd594dba4789eba310891cc2caed2ce1975d) fix: bg job tracks real command pid, not transient shell (fix-bg-job)
- [`83d7cf82`](https://github.com/lucapinello/workgraph/commit/83d7cf82f5cc65e1499bc4baf1b6531d39ecc0d5) test(smoke): pin the protected-cron cleanup-sweep guard (re-arm-the)
- [`02204b19`](https://github.com/lucapinello/workgraph/commit/02204b19ce5d9f15a7706febcda43cd67a84d9a0) fix(provider-health): wg-done graph refusals never count as provider failures (the-provider-pause)
- [`9a595528`](https://github.com/lucapinello/workgraph/commit/9a5955288dc8419f2efff10986cfe25d8f3bc1a9) fix(done): FailedPendingEval must never deadlock the rescue path (satellite-deadlock-failedpendingeval)
- [`f9f92435`](https://github.com/lucapinello/workgraph/commit/f9f92435b8ddb59f45490ea67b65d6488ef8c8d5) fix(cron): re-arm & protect production crons so a cleanup sweep can't kill the daily digest
- [`afd79373`](https://github.com/lucapinello/workgraph/commit/afd79373befd6a41d12f11be1fe94f48b042bba7) feat(service): the provider pause heals itself — probe, auto-resume, always alert (the-provider-pause)
- [`9b823397`](https://github.com/lucapinello/workgraph/commit/9b82339722b2f315736d3c9cefc65970419f2d14) feat: self-healing spawn circuit breaker + loud operator alert (retry-circuit-breaker)
- [`c3810107`](https://github.com/lucapinello/workgraph/commit/c38101076162fb65d9921bc8151be42f67cd3cb2) feat(setup): claude-cli route nudges the headless setup-token + [auth] file path (implement-the-llm)
- [`e2ebba9c`](https://github.com/lucapinello/workgraph/commit/e2ebba9c9cb24f573ac89c5c071f70b7960b93bf) fix(auth): close the chat/coordinator handler credential gap + doctor token-file check (implement-the-llm)
- [`b5987388`](https://github.com/lucapinello/workgraph/commit/b5987388a4d208db80d1bcdd5dcc795faed7cab6) fix(cron): mint a distinct instance per firing so re-registration never re-blocks children (cron-re-registration)
- [`57e94067`](https://github.com/lucapinello/workgraph/commit/57e9406753b794daf30435bd14f9ee4a0207898b) fix(telegram): redact bot tokens from poll-error log lines (listener-redact-bot)
- [`ccc9597a`](https://github.com/lucapinello/workgraph/commit/ccc9597aeef0841f264258d4fcd2c7ceda894511) feat: listener FD-leak fix (reuse+rebuild reqwest client) + telegram send bots-map-only 404 fix (listener-reconnect)

### Disposable follow-ons (2)
- [`a3d3a513`](https://github.com/lucapinello/workgraph/commit/a3d3a5131121440ba19a053ea59a9135c7a8f8f7) feat: ingest disposable results into spawner session memory (disposable-ingest)
- [`9295fd09`](https://github.com/lucapinello/workgraph/commit/9295fd09093698b52f2d4e353f4c74f6ab3089b1) feat: enforce disposable artifact + wg log before done (disposable-artifact-enforcement)

### Generic Telegram/conversation substrate (24)
- [`2be465ed`](https://github.com/lucapinello/workgraph/commit/2be465edb66dab7c30abaca976ab528b364a0265) fix(telegram): benign 'not a parked-task reply' log, not misleading 'Rejected'
- [`589f63b0`](https://github.com/lucapinello/workgraph/commit/589f63b0829128499c3e6c7bf166e80d253cff72) feat: group-chat discussion rounds — 'find consensus' asks get all four voices + a wrap-up (group-chat-discussion)
- [`d3002e46`](https://github.com/lucapinello/workgraph/commit/d3002e4605956f2519bafeaed65c59e6544881aa) fix: group-chat plural addressing elects roster, no swallowed follow-ups, reply via elected bot (group-chat-plural)
- [`0eb373bb`](https://github.com/lucapinello/workgraph/commit/0eb373bb4108cc3e4c28053ef050bd8a0d8cd148) fix: thread human_count through merged decide path + reconcile tests (membership-aware-silence)
- [`57d31c1c`](https://github.com/lucapinello/workgraph/commit/57d31c1c7188bd262ee3677380474459fe977f12) fix(telegram): punctuation is not a command; mention owns its message; operator reference never in family chat (fix-command-leaks)
- [`cc813c9b`](https://github.com/lucapinello/workgraph/commit/cc813c9bcd771b0798883a101c1bda7fb8ff0780) test(smoke): election @mention resolves without a configured username (fix-mention-precedence)
- [`d779d571`](https://github.com/lucapinello/workgraph/commit/d779d5718a525358282e4dff8dc8a3a4f7abb7b8) fix(telegram): @mention always resolves & wins over silence (fix-mention-precedence)
- [`d5b666e8`](https://github.com/lucapinello/workgraph/commit/d5b666e8010ab3989b7d4853388743d23fb7669e) feat: membership-aware silence — single-human group answers greetings (membership-aware-silence)
- [`b7a48fac`](https://github.com/lucapinello/workgraph/commit/b7a48fac6e4b77687265e1ad357e4b4c4566f0a8) fix(telegram): typo-tolerant summon detection in the election classifier (fuzzy-summon)
- [`9f9377ce`](https://github.com/lucapinello/workgraph/commit/9f9377ce1d3a0b8c05ad4f3d861bf76da93e8295) fix(telegram): converse turn composes a real reply instead of hanging (fix-converse-hang)
- [`e372ec94`](https://github.com/lucapinello/workgraph/commit/e372ec94425cb9a1c1359ef95955caf769270855) fix(telegram): resolve roster persona name to canonical agent id for session lookup (dedupe-key-fix)
- [`abc5343d`](https://github.com/lucapinello/workgraph/commit/abc5343dbbfa67a2c9a28e7958d2f4114c94a939) fix(telegram): dedupe on content fingerprint, not per-bot message_id (dedupe-key-fix)
- [`9038948b`](https://github.com/lucapinello/workgraph/commit/9038948bdbdeb9d9e1a990f92fb5ff5a7b4a666f) test(smoke): telegram_group_reply_tuning — live-binary proof of sender resolution + classifier (group-reply-tuning)
- [`6e1da3e3`](https://github.com/lucapinello/workgraph/commit/6e1da3e35e13a72b0fa5c9d1cead40312f1af45b) fix(telegram): pin pr51-auth conversational fallthrough + `wg telegram classify`
- [`c7e99f1f`](https://github.com/lucapinello/workgraph/commit/c7e99f1f925f0875612cbd33a13ef910e0d8a046) fix(telegram): stale-backlog policy + burst coalescing + sent-id regression (group-reply-tuning)
- [`ad1c98e8`](https://github.com/lucapinello/workgraph/commit/ad1c98e8b35a3051b4160d30cd160e626fd048a4) fix(telegram): sender resolution + bot-loop guard + election/grounding (group-reply-tuning)
- [`160f9c21`](https://github.com/lucapinello/workgraph/commit/160f9c21eaaaa8f1b5f1dd3624b7efe09eaf8bd1) feat: log every listener election decision (election-logging)
- [`db431497`](https://github.com/lucapinello/workgraph/commit/db43149775d1d5daf9cbec50bcf037134ced0ed4) feat: conversational replies for 1:1 and name-addressed group messages (telegram-1to1-chat)
- [`de57f1cf`](https://github.com/lucapinello/workgraph/commit/de57f1cf4e7778a1a00a493bb5ca3437c26dc051) feat: all-bots-off dedupe + responder election (group-dedupe-responder)
- [`ddcde70e`](https://github.com/lucapinello/workgraph/commit/ddcde70e997c44ea1afe16b0ac1dd1b44a6ae386) feat: listener long-polls ALL configured bots concurrently (fix-poll-all-bots)
- [`df6bdf75`](https://github.com/lucapinello/workgraph/commit/df6bdf75576245e0ebaaea993adfc48f2122933c) feat: natural group routing — name-addressed + reply-chain + concierge (group-natural-routing)
- [`51f857bb`](https://github.com/lucapinello/workgraph/commit/51f857bb45b8fb94d264105a679e3278cb97b9f7) feat: /standup — listener-orchestrated family roster check-in (group-standup)
- [`c8248625`](https://github.com/lucapinello/workgraph/commit/c8248625fe124de3b9cabbe091dc3ac9278bd4c7) feat: R18 generic inline-button -> task routing (r18-inline-buttons)
- [`9c1fef9f`](https://github.com/lucapinello/workgraph/commit/9c1fef9f802c989578dd45eee39d2c1b9066c4ab) feat: R17 group @mention routing for Telegram (r17-group-mention)

### Casa product/adapters (31)
- [`36147260`](https://github.com/lucapinello/workgraph/commit/36147260682d5e6159db8f9bb862380f3efae27e) feat: answer-shape read replies — direct, scoped, clock-aware, no counter-questions (answer-shape-direct)
- [`0ace56a2`](https://github.com/lucapinello/workgraph/commit/0ace56a2628e99c99cc44b5343a138155515287c) feat(convo): grounded, non-repetitive, corrigible conversation (otto-answers-like)
- [`c7f8d6c6`](https://github.com/lucapinello/workgraph/commit/c7f8d6c6841199e962124615cc295d5ae0789853) feat(fast-lane): ask→dish transform + semantic sanity gate before writing
- [`ddfcc044`](https://github.com/lucapinello/workgraph/commit/ddfcc044cc6179fa1b1714968747b79e4cf284b7) feat(telegram): wire the daily digest flush — the 12:00 UTC cron finally emits the morning message
- [`14863493`](https://github.com/lucapinello/workgraph/commit/148634932f8dc4710f0954babe423dfb7b0ad506) feat: web-inbound path parity + clarify-continuation wiring (one inbound brain) (redo-one-inbound)
- [`0928a13b`](https://github.com/lucapinello/workgraph/commit/0928a13b00ba03db25d85bd3b4abdd7130b397fe) feat: typo-tolerant swap-shaped classify_domain + clarify continuation (one inbound brain) (redo-one-inbound)
- [`32bd52c4`](https://github.com/lucapinello/workgraph/commit/32bd52c4287fa529331762c12202ef266ee8ee11) fix(notify/lifecycle): report-backs obey the ledger contract — every send mirrors to the pane feed and verifies delivery (lifecycle-messages-obey)
- [`097360a4`](https://github.com/lucapinello/workgraph/commit/097360a48e784f5d7bbe08e2c3f8a6a32a9e84c0) feat(notify): fast lane for simple asks — a meal swap takes a minute, not twenty (fast-lane-for)
- [`d10b1f52`](https://github.com/lucapinello/workgraph/commit/d10b1f523aa6fa198376c8e29e6fa6030f964fec) fix(election/lifecycle): greeting precedence, defer discipline, family voice (morning-taco-bugs)
- [`78739f3d`](https://github.com/lucapinello/workgraph/commit/78739f3d1118062ece7fe9c7e5aff5197547f349) feat(notify): domain-aware election voice — the chef/dietician/coach answers food/workout asks, not always Otto (one-ask-one-2)
- [`872b2b5e`](https://github.com/lucapinello/workgraph/commit/872b2b5ea57a411a008d5c740ee3aef66d9f38d6) fix(notify): off-domain guard at the creation choke point — single-voice meal asks land on the meal owner, not Otto (one-ask-one-2)
- [`a144f9ed`](https://github.com/lucapinello/workgraph/commit/a144f9edb877539a7ebadd671025f4853b6a3274) fix(telegram): web-inbound resolves the group chat id from the bots map (urgent-web-inbound)
- [`7fb94e0a`](https://github.com/lucapinello/workgraph/commit/7fb94e0a2516a837224065c067ada9006309d467) fix(notify): report-backs are replies — never capped, delivered from the listener (report-back-round-2)
- [`14b873e9`](https://github.com/lucapinello/workgraph/commit/14b873e93b6b51d78bd65351788243264f298778) feat(notify): lifecycle report-backs fire themselves — coordinator transition trigger + what-changed payoff (report-backs-must)
- [`b3386157`](https://github.com/lucapinello/workgraph/commit/b3386157be1e6e2ce7ece75fb9fbe8d38439db56) feat(notify): one ask, one owner — collective turns route work to the domain owner, not everyone (one-ask-one)
- [`db61d9ed`](https://github.com/lucapinello/workgraph/commit/db61d9ed37bd6192a40b9e0bdaa066b156dbc90e) feat(notify): promise-action parity — prove a persona's promise became an action (promise-action-parity)
- [`60a4ed97`](https://github.com/lucapinello/workgraph/commit/60a4ed9771af388fbb074aeb32a0510096a95c93) feat(telegram): close the conversational loop — wire report-back into the composer + CLI (close-the-conversational)
- [`0dfe117d`](https://github.com/lucapinello/workgraph/commit/0dfe117d737448fb504a4daf1fc84a440725024e) feat(notify): lifecycle loop core — origin stamping + report-back engine (close-the-conversational)
- [`3dd6304e`](https://github.com/lucapinello/workgraph/commit/3dd6304e3cf316e72afd99f1c794eeb2df6df08b) feat(notify): errand nudge — the remaining shopping list chases the runner to the market (retry-close-the)
- [`2789cde2`](https://github.com/lucapinello/workgraph/commit/2789cde2a384210ac4cbba05e9be43efb53ab78b) feat(notify): one calm daily digest — the single pacing layer for proactive DMs (one-calm-daily)
- [`a2249891`](https://github.com/lucapinello/workgraph/commit/a2249891e3ebcae80f91acbe77835e321845984b) fix(telegram): persona-aware send so the review digest leaves via the composing voice's bot, not the wrong bot (review-digest-sent)
- [`a03e82ea`](https://github.com/lucapinello/workgraph/commit/a03e82ea7afa0789b9cdb24819a8bd846d9a3278) fix(telegram): attach auth-confirm shared secret header on listener writes
- [`15fd9fba`](https://github.com/lucapinello/workgraph/commit/15fd9fba9009415d0aae9809b6b1ceecc9e4d277) feat(telegram): photo → shopping list vision turn — snap the fridge, Bruno adjusts the pickup list (photo-to-shopping)
- [`fd98bee6`](https://github.com/lucapinello/workgraph/commit/fd98bee6125e91078a861145be9327b8c3aa28cd) feat(telegram): reminder engine — scheduled Telegram nudges from plan rows + ad-hoc asks (reminders-that-reach)
- [`fa645f11`](https://github.com/lucapinello/workgraph/commit/fa645f11b286ec2e69f209e4a15d7a816402b1a7) feat(casa): meal feedback loop — ratings persist and steer next week's plan (the-feedback-loop)
- [`8fb6d1e3`](https://github.com/lucapinello/workgraph/commit/8fb6d1e36ae4afa810cc0cf4a6c2e825c099fd11) docs(casa): household.toml composition contract — wg-side persona identity consumers
- [`48b9d69b`](https://github.com/lucapinello/workgraph/commit/48b9d69b1b65ccc3f039ed44b6728da10b4a31a9) feat(telegram): onboarding-bootstrap listener — join_ invite gate + empty-roster founding handshake (onboarding-bootstrap-first)
- [`8e15c9c1`](https://github.com/lucapinello/workgraph/commit/8e15c9c195595456614b0a30e3660f1026a0fc7f) feat(telegram): web-identity /start login_<nonce> listener gate (web-identity-listener)
- [`f1ef4e35`](https://github.com/lucapinello/workgraph/commit/f1ef4e35778aed35a442b8724a685ae7440b291a) feat(telegram): web-inbound — kiosk pane messages are first-class group turns
- [`43c1bc7f`](https://github.com/lucapinello/workgraph/commit/43c1bc7fbbb39958cb25f6ea2f60497576446bd8) feat: casa group-feed writer — mirror inbound group + agent replies (listener-write-inbound)
- [`924db2c5`](https://github.com/lucapinello/workgraph/commit/924db2c55e7df1c7ae4d368ffb617e161244fdd8) feat: family command set /dinner /shopping /week /reminders /help (family-command-set)
