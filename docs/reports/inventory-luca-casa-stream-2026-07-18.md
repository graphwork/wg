# Luca/Casa stream refresh against current main

**Snapshot:** 2026-07-18 09:07–09:16 UTC<br>
**Upstream:** [`graphwork/wg`](https://github.com/graphwork/wg), `origin/main` = [`528b8ee0`](https://github.com/graphwork/wg/commit/528b8ee0258bc48c6fd11ad9b87562610a276526)<br>
**Fork:** [`lucapinello/workgraph`](https://github.com/lucapinello/workgraph), `luca/integration/casa-pinello` = [`64f2cba8`](https://github.com/lucapinello/workgraph/commit/64f2cba83bf91e4e0203f09957e655ff635226ed)<br>
**Prior snapshot:** [`inventory-luca-casa-stream-2026-07-16.md`](./inventory-luca-casa-stream-2026-07-16.md)

This is a read-only refresh. It did not merge, rebase, cherry-pick, review, comment on, or otherwise mutate a source branch, PR, or production graph. GitHub CLI calls were made with `GH_BROWSER` and `BROWSER` unset. PR #57 was read once at its known exact head; the unchanged head was not polled repeatedly.

## Executive delta

- `origin/main` advanced from `fb19f595` to `528b8ee0`. The integration branch advanced from `42166fe0` to `64f2cba8`, but fell much farther behind: **128 ahead / 36 behind**, versus 125 / 5 on July 16.
- Integration added three commits: one merge wrapper and two source commits, Telegram voice notes (`057bb21e`) and Casa expired-link parity (`64f2cba8`). It now has **128 fork-only commits: 40 merges and 88 non-merges**.
- Patch identity changed from `86 + / 0 -` to **`83 + / 5 -`**. The five now patch-equivalent upstream commits are `f17867a8`, `43755390`, `d4b45596`, `11c4f695`, and `bbd699d4`. Semantic supersession is broader than those five exact patches.
- The fork now exposes **105 branch refs** (61 with July tips, 44 older), up from 100 (56 July, 44 older), and **26 fork PRs**, up from 21. The five new branches are the heads of fork PRs #22–#26.
- Upstream PRs #51, #54, and #56 are now merged. The active upstream Luca lane is only **PR #57**, still open at exact head `6509e4cf`.
- PR #57 is CLEAN/MERGEABLE and all five checks are green, but its decision remains **CHANGES_REQUESTED**. The latest review found that allowlisted `gh pr view|list` still admitted `--web`/`-w`, allowing task-authored validation to launch the configured browser outside the intended containment boundary. Luca's last response and revision preceded that review; there is no later reply or head.
- A full integration merge is even less viable: the tip-to-tip diff is **160 files, +42,597/-10,918**. The classic `git merge-tree` both-modified surface grew from nine to **21 paths**; the modern merge algorithm leaves **15 actual unresolved paths**. Shared CLI/service/manifest files also carry current TUI, handler routing, Pi, federation, review, and execution contracts.
- The three previous core candidates still describe real current-main bugs and their individual patches pass both exact and three-way `git apply --check`. That does **not** make them blind cherry-picks: `d25cdd59` needs process-tree containment work; `9a595528` must be tested against the current FailedPendingEval/scaffold flow; `02204b19` is coupled to obsolete unstructured error and provider-identity assumptions.

## Reproducible read-only snapshot

```bash
unset GH_BROWSER BROWSER

git fetch --prune origin '+refs/heads/main:refs/remotes/origin/main'
git fetch --prune luca '+refs/heads/*:refs/remotes/luca/*'
git fetch --prune luca '+refs/pull/*/head:refs/remotes/luca-pr/*'

env -u GH_BROWSER -u BROWSER gh pr list \
  --repo lucapinello/workgraph --state all --limit 200 \
  --json number,state,title,headRefName,headRefOid,baseRefName,isDraft,\
mergeCommit,createdAt,updatedAt,closedAt,mergedAt,url,reviewDecision,statusCheckRollup

env -u GH_BROWSER -u BROWSER gh pr list \
  --repo graphwork/wg --state all --author lucapinello --limit 200 \
  --json number,state,title,headRefName,headRefOid,baseRefName,isDraft,\
mergeCommit,createdAt,updatedAt,closedAt,mergedAt,url

# One exact-head read of #57; do not repeat until its head/timeline changes.
env -u GH_BROWSER -u BROWSER gh pr view 57 --repo graphwork/wg \
  --json number,state,title,url,author,headRefName,headRefOid,baseRefName,isDraft,\
mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,reviews,comments,commits,updatedAt

git rev-parse origin/main luca/main luca/integration/casa-pinello
git rev-list --left-right --count origin/main...luca/integration/casa-pinello
git merge-base origin/main luca/integration/casa-pinello
git rev-list --count origin/main..luca/integration/casa-pinello
git rev-list --count --merges origin/main..luca/integration/casa-pinello
git rev-list --count --no-merges origin/main..luca/integration/casa-pinello
git cherry -v origin/main luca/integration/casa-pinello
git diff --shortstat origin/main luca/integration/casa-pinello

git for-each-ref --sort=refname \
  --format='%(refname:short)%09%(committerdate:iso-strict)%09%(objectname)%09%(subject)' \
  refs/remotes/luca/
```

Observed core values:

```text
snapshot                    2026-07-18T09:07:09Z
origin/main                 528b8ee0258bc48c6fd11ad9b87562610a276526
luca/main                   1cee472d64555638a3bddf83a878a11e4539254f
integration/casa-pinello    64f2cba83bf91e4e0203f09957e655ff635226ed
origin vs luca/main         14 0     # fork main is 14 behind, 0 ahead
origin vs integration       36 128   # left-only origin, right-only integration
merge-base                  583808834860f31b58c40b1896f2d9d4f9bbfea1
integration fork-only       128 total = 40 merges + 88 non-merges
git cherry                  83 +, 5 -
tip-to-tip diff             160 files, 42597 insertions, 10918 deletions
fork branch refs            105 = 61 July-tip + 44 older
fork PR refs / metadata     26 / 26
```

The merge base is unchanged. `git cherry` is patch identity only; it does not recognize revised PR heads or semantic rewrites.

## Ref changes since the July 16 snapshot

The prior report recorded 100 refs. The current count is 105, and the five-ref increase is exactly the five new PR branches below. Existing upstream-lane refs and the integration/main refs also moved.

| Ref | July 16 state | Current tip | Current relation to `origin/main` | Meaning |
|---|---:|---:|---:|---|
| `luca/integration/casa-pinello` | `42166fe0` | `64f2cba8` | 128 ahead / 36 behind | Added fork #22 plus `64f2cba8`. |
| `luca/main` | `fb19f595` | `1cee472d` | 0 ahead / 14 behind | Contains merged upstream #51/#54/#56 era, but not current TUI/Pi follow-ons. |
| `luca/fix/bug-deliverable-preflight` | `11c4f695` PR head | `cda3dc66` | 0 ahead / 24 behind | Revised then merged as upstream #54; superseded. |
| `luca/r8-scope-guard` | `b97ba03d` PR head | `b505d4d1` | 0 ahead / 16 behind | Revised then merged as upstream #56; superseded. |
| `luca/wg-eval-exec` | `0925c010` PR head | `6509e4cf` | 7 ahead / 14 behind | Upstream #57 round-three head; still under security review. |
| `luca/casa/telegram-voice-notes` | absent | `057bb21e` | 126 ahead / 36 behind | **New**, fork #22; source is now in integration. |
| `luca/casa/bug-chat-rust` | absent | `b359f6ff` | divergent; unique patch not in integration | **New**, fork #23. |
| `luca/casa/bug-ops-daemon` | absent | `567c6a21` | 126 ahead / 36 behind | **New**, fork #24; unique patch not in integration. |
| `luca/casa/sign-in-confirmation` | absent | `e610213a` | 128 ahead / 36 behind | **New**, fork #25; unique patch not in integration. |
| `luca/casa/urgent-chat-quality` | absent | `982a86a3` | 129 ahead / 36 behind | **New**, fork #26; unique patch not in integration. |

No other post-snapshot branch-tip change is visible from the fetched refs. The 44 pre-July historical refs remain outside the Casa-active cut, as in the prior report.

Integration's precise delta from `42166fe0` is:

```text
057bb21e  feat: Telegram voice notes → transcript → normal message
ef10b3c6  Merge pull request #22 from lucapinello/casa/telegram-voice-notes
64f2cba8  feat: engine/listener parity — unknown-nonce → actionable reopen reply
```

## Fork PRs added since the prior inventory

Fork PRs #1–#21 retain the states described in the July 16 report. The fork now has 21 merged, one closed-unmerged, and four open PRs. There are still no submitted review decisions on the fork PRs.

| Fork PR | State / head | Integration | Checks at fetched head | Classification |
|---|---|---|---|---|
| [#22](https://github.com/lucapinello/workgraph/pull/22) voice notes | merged, `057bb21e` | source + merge wrapper present | stable/integration/nightly/pi green; lint red | Generic conversation substrate **plus security review**: untrusted media download, token-bearing Bot API URL, external transcription gateway, size/MIME/temp-file/process bounds, and review-gate ingestion. Rebuild after the generic boundary is agreed. |
| [#23](https://github.com/lucapinello/workgraph/pull/23) stable Casa feed `srcId` | open, `b359f6ff` | one patch unique, absent | stable/integration/nightly/pi green; lint red | Casa adapter/product. The use of `DefaultHasher` as a durable cross-process protocol identifier needs a stability/collision/privacy decision; retain with the Casa reader contract, not core. |
| [#24](https://github.com/lucapinello/workgraph/pull/24) benign IPC disconnects | open, `567c6a21` | one patch unique, absent | stable/integration/nightly/pi green; lint red | Reusable core intent. Reimplement on current service IPC; it conflicts with July 17 TUI/chat restart changes. Narrow EINVAL handling so real stream-state bugs are not hidden. |
| [#25](https://github.com/lucapinello/workgraph/pull/25) device-aware sign-in copy | open, `e610213a` | one patch unique, absent | stable/integration/nightly/pi green; lint red | Casa adapter/product **plus auth review**. Device labels and the `tablet` marker come from the external gateway and must be treated as untrusted display input. |
| [#26](https://github.com/lucapinello/workgraph/pull/26) single-owner/date/idempotency | open, `982a86a3` | one patch unique, absent | stable/integration/nightly/pi green; lint red | Casa product. The request-id idempotency idea is reusable, but ownership vocabulary, meal taxonomy, local calendar, and prompt shape are household policy. |

As before, fork check success is evidence, not upstream validation. Every #3–#26 head still has a red Check & Lint result; #22–#26 have the other four recorded jobs green.

## Upstream Luca PR lane now

| PR | Current state | Refresh conclusion |
|---|---|---|
| #36 | closed unmerged | Superseded by merged #52. |
| #37, #49, #50, #52, #53, #55 | merged | Already upstream; do not re-import fork copies. |
| **#51** | **merged** as `d78582a4` | The old integration dispatch commits are semantic duplicates. |
| **#54** | **merged** as `5f54c7f8` | Integration `43755390`/`11c4f695` are patch-equivalent or superseded; current main also has round-three `cda3dc66`. |
| **#56** | **merged** as `1cee472d` | Integration's scope-guard sequence is superseded by final `b505d4d1`. |
| **#57** | **open**, `6509e4cf` | Security-review required; exact status below. Do not import integration's older `87233bb9`/`738c88f5`. |

### PR #57 exact-head state — one read, no polling

At the single metadata read on 2026-07-18:

```text
head                    6509e4cf553b0deeb6b11f17b7f3958a4b5bf045
state                   OPEN
mergeability            CLEAN / MERGEABLE
review decision         CHANGES_REQUESTED
head / PR updated       2026-07-16T16:08:59Z
checks                   lint GREEN; stable GREEN; integration GREEN;
                         pi-plugin GREEN; nightly GREEN
latest review           ekg, CHANGES_REQUESTED, 2026-07-16T16:08:59Z,
                         submitted against exact head 6509e4cf
latest Luca comment     2026-07-16T16:05:16Z (addresses the preceding review)
response after review   none
revision after review   none; head remains 6509e4cf
```

The latest review accepts the round-three direct-argv parser, rejection of shell operators and `ls-remote` execution transports, process-group timeout cleanup, descendant reaping, and their tests. One blocker remains: `validate_gh_pr` uses a deny-small-list policy and still permits `gh pr view|list --web`/`-w`. Those flags launch `GH_BROWSER`/`BROWSER`; a browser can detach and escape the validation process group's lifetime. The requested fix is an explicit read-only option allowlist per `gh` subcommand plus a public-path no-side-effect regression. Because neither the head nor timeline changed after that review, another status poll would add no information.

## Current-main conflict surface

Two merge-tree views were generated with the unchanged merge base (neither updates a ref, index, or worktree):

```bash
base=$(git merge-base origin/main luca/integration/casa-pinello)
git merge-tree "$base" origin/main luca/integration/casa-pinello > /tmp/luca.merge-tree
# Modern merge result; exits 1 and prints unresolved paths.
git merge-tree --write-tree --name-only \
  origin/main luca/integration/casa-pinello > /tmp/luca.merge-tree.names
```

The classic view reports these **21 both-modified/add-add sections** (the measure called “direct conflict paths” in the prior report):

```text
added in both:   src/agency/human_binding.rs
                 src/commands/agency_human.rs
                 src/commands/service/human_dispatch.rs
                 src/scope_guard.rs
                 tests/integration_scope_guard.rs
changed in both: src/agency/mod.rs
                 src/cli.rs
                 src/commands/add.rs
                 src/commands/deliverables.rs
                 src/commands/done.rs
                 src/commands/mod.rs
                 src/commands/service/coordinator.rs
                 src/commands/service/ipc.rs
                 src/commands/service/mod.rs
                 src/commands/telegram.rs
                 src/dispatch/plan.rs
                 src/lib.rs
                 src/main.rs
                 src/notify/mod.rs
                 src/notify/telegram.rs
                 tests/smoke/manifest.toml
```

The modern merge algorithm auto-merges six of those 21 (`src/agency/mod.rs`, `src/commands/mod.rs`, `src/commands/service/ipc.rs`, `src/commands/service/mod.rs`, `src/dispatch/plan.rs`, and `src/lib.rs`) and leaves these **15 actual unresolved paths**:

```text
src/agency/human_binding.rs
src/cli.rs
src/commands/add.rs
src/commands/agency_human.rs
src/commands/deliverables.rs
src/commands/done.rs
src/commands/service/coordinator.rs
src/commands/service/human_dispatch.rs
src/commands/telegram.rs
src/main.rs
src/notify/mod.rs
src/notify/telegram.rs
src/scope_guard.rs
tests/integration_scope_guard.rs
tests/smoke/manifest.toml
```

Twenty-four paths were changed on both sides since the merge base. Three more happen to merge textually without appearing in the 21 classic sections but still require semantic review: `src/agency/starters.rs`, `src/agency/types.rs`, and `src/commands/agent_crud.rs`.

### Why current subsystems make file-level merging unsafe

| Current-main subsystem | Collision / hidden dependency |
|---|---|
| **TUI and persistent chat** | July 17 work rewrote chat close/navigation, mosh Enter handling, PTY rendering, stateful restart, service-health restart, and graph-output confinement. Fork #24 and integration both change `service/ipc.rs`; all fork smoke additions collide with the grow-only manifest. A fork IPC patch must pass the actual TUI/chat restart flow, not only fake-writer tests. |
| **Handler-first routing and human identity** | Final #49/#51/#56 changed `human_binding`, `human_dispatch`, `add`, scope guard, CLI, Telegram, and plan dispatch. Old fork copies must not restore pre-final numeric-sender or disposable-scope behavior. Provider health must distinguish handler (`claude`, `codex`, `nex`, `pi`) from inner wire/model rather than the legacy `native:*` heuristic. |
| **Pi** | Current main's Pi plugin/chat-model bridge touches `cli.rs`, `main.rs`, spawn execution, service coordination, and the smoke manifest. Native `/bg` jobs and Pi worker process groups are different execution paths; `d25cdd59` fixes only the native background tool. Any shared containment helper must preserve Pi's hermetic plugin spawn and stream bridge. |
| **WG-Fed** | Federation already exists at the common base, but the fork's Casa gateway/identity/ledger is a parallel identity and transport plane. New generic conversation work must reuse or explicitly adapt `wgid`, signed envelopes, canonical peer trust, and sealed ACLs rather than silently treating Casa gateway claims as WG identity. Shared `cli/main/lib/manifest` wiring is conflict-prone. |
| **WG-Review** | IC4 message ingestion now derives author trust canonically and fails closed before consumption. Telegram/Casa messages that create tasks or load context must not bypass that seam. The provider-classifier candidate also keys off human-readable `wg done` gate text that has evolved with deliverable/smoke/review protections. |
| **WG-Exec** | `src/providers/` is remote execution, leases, scoped UCANs, and signed results; `src/service/provider_health.rs` is LLM-handler availability. They are separate provider concepts. `02204b19` and provider auto-recovery must not pause or trust remote WG-Exec providers, while Casa lifecycle/result fast lanes must not bypass lease, signature, review, or digest-pinned acceptance. |

The correct extraction unit remains a fresh current-main implementation, never a merge of `integration/casa-pinello` or wholesale copies of `cli.rs`, `main.rs`, `telegram.rs`, service files, or the manifest.

## Complete candidate reclassification

The active candidate universe is the 88 integration source commits plus the four unique open-PR patches not in integration (`b359f6ff`, `567c6a21`, `e610213a`, `982a86a3`): **92 source patches**. The groups below account for every one exactly once by primary product boundary. Security review is also an overlay, listed afterward, because a Casa adapter can still be security-sensitive.

### 1. Merged or superseded — 17

Do not import these. Use current upstream merge/final PR heads.

```text
72128ea4 738c88f5 87233bb9 bbd699d4 c7950a28 d4b45596
11c4f695 342b69c9 107f1c40 24f69680 4a09e7f6 79b0900e
4144ef56 f17867a8 49dbec49 31768fba 43755390
```

Five are exact `git cherry -` patches (`f17867a8`, `43755390`, `d4b45596`, `11c4f695`, `bbd699d4`); the rest are older/partial implementations or fixture repairs superseded by merged #51/#54/#55/#56 or active #57.

### 2. Reusable WG core — 11

```text
d25cdd59 83d7cf82 02204b19 9a595528 f9f92435 afd79373
9b823397 b5987388 57e94067 ccc9597a 567c6a21
```

These are, respectively: native background PID handling; cron smoke/protection/re-registration; provider refusal classification and recovery/breaker; FailedPendingEval rescue; Telegram token redaction/client reuse/bots-map fallback; and benign service IPC disconnect handling. “Reusable” means reimplement/test on main, not cherry-pick without review.

### 3. Generic conversation substrate — 25

```text
2be465ed 589f63b0 d3002e46 0eb373bb 57d31c1c cc813c9b
d779d571 d5b666e8 b7a48fac 9f9377ce e372ec94 abc5343d
9038948b 6e1da3e3 c7e99f1f ad1c98e8 160f9c21 db431497
de57f1cf ddcde70e df6bdf75 51f857bb c8248625 9c1fef9f
057bb21e
```

This remains a coherent substrate—polling, dedupe, sender resolution, compose, addressing/election, discussions/buttons, and now voice transcription—but it is not ready as one core batch. It must be split from Casa policy and reconciled with #51 identity, canonical trust/review, Pi model routing, and the current grow-only smoke surface.

### 4. Casa adapter/product — 35

```text
36147260 0ace56a2 c7f8d6c6 ddfcc044 14863493 0928a13b
32bd52c4 097360a4 d10b1f52 78739f3d 872b2b5e a144f9ed
7fb94e0a 14b873e9 b3386157 db61d9ed 60a4ed97 0dfe117d
3dd6304e 2789cde2 a2249891 a03e82ea 15fd9fba fd98bee6
fa645f11 8fb6d1e3 48b9d69b 8e15c9c1 f1ef4e35 43c1bc7f
924db2c5 64f2cba8 b359f6ff e610213a 982a86a3
```

These carry the household gateway/feed contract, family plan/ownership/voices, onboarding/sign-in copy, photos/reminders/feedback, lifecycle/digests, grounding, and answer shape. Preserve them on the Casa line unless maintainers explicitly accept a product-neutral adapter interface.

### 5. Security/trust-boundary as the primary concern — 4

```text
a3d3a513 9295fd09 e2ebba9c c3810107
```

The first pair changes disposable result ingestion and completion contracts; the second pair changes authentication token discovery/setup. They require an explicit threat/policy review against final scope guard, current handler-first config/secrets, Pi, review, and execution custody before reuse.

### Security-review overlay across the other groups

The following remain in their product-boundary group above but cannot advance without security review:

- **Process control:** `d25cdd59`, provider probe/breaker `afd79373`/`9b823397`, and PR #57. Review process groups, descendants, reaping, shell/path quoting, command provenance, Windows behavior, and cancellation attribution.
- **Inbound identity/replay:** `ddcde70e`, `de57f1cf`, `abc5343d`, `ad1c98e8`, `9c1fef9f`, `c8248625`, `057bb21e`, and Casa feed `b359f6ff`. Preserve numeric sender identity and run review before consumption; test cross-bot replay/collision/stale/media cases.
- **Gateway/auth/secrets:** `a03e82ea`, `8e15c9c1`, `48b9d69b`, `64f2cba8`, `e610213a`, and voice/media `057bb21e`. Review nonce replay/TTL, gateway response trust, device-label escaping, token/log redaction, file permissions, and fail-closed behavior.
- **Completion/result trust:** lifecycle/report-back work plus disposable work must compose with WG-Review verdicts and WG-Exec signed/digest-pinned result acceptance, not create a parallel “delivered therefore trusted” path.

## The three prior core candidates on current main

All checks below were read-only:

```bash
for c in d25cdd59 9a595528 02204b19; do
  git show --format= --binary "$c" | git apply --check -
  git show --format= --binary "$c" | git apply --3way --check -
done
```

All six checks succeeded. This means the hunks still fit; the semantic dependencies below still govern landing.

### `d25cdd59` — native background PID

**Current status:** bug still present. `src/executor/native/background.rs::spawn_detached` still runs external `setsid` under `bash -c` and records the transient shell PID. The candidate is reusable core, with required process-containment review.

**Hidden dependencies / gaps:**

1. `exec <command>` only makes the shell PID become the target for a simple external command. Compound shell syntax, pipelines, and explicit backgrounding can leave the shell or descendants as the real lifetime owner.
2. The candidate creates a new session/process group but `kill_process` still signals only positive `pid`; grandchildren may survive. Record/control the group and TERM→KILL the whole tree, then reap deterministically.
3. The command and `log_path.display()` remain interpolated into shell text. A model-facing `/bg run` command and a path containing shell metacharacters/spaces need an explicit trust and quoting policy.
4. Current main has separate containment paths in `commands/spawn/execution.rs` (including Pi workers) and `platform_timeout.rs`; PR #57 contains a newer, still-unmerged direct-argv/process-group design. Avoid three divergent implementations.
5. The existing regression is only `sleep 60` plus a unit-level kill. Required proof is repeated Linux/macOS start/status/cancel, a pipeline/descendant case, no surviving PGID, path-with-spaces, parent exit survival, and Windows behavior. The patch does not cover Pi or WG-Exec remote leases.

### `9a595528` — FailedPendingEval rescue deadlock

**Current status:** bug still present and the candidate is the smallest clean core fix. Current `src/query.rs` already treats both `PendingEval` and `FailedPendingEval` as satisfied for system dependents in every readiness path, while `done.rs` bypasses only `PendingEval`. That mismatch is exactly the circular completion gate described by the commit.

**Hidden dependencies / gaps:**

1. Current failure routing now exempts shell tasks from agency scaffolding and sends agent-exit failures through the current `FailureClass` path. Exercise the real wrapper crash → `FailedPendingEval` → FLIP → evaluate verdict flow, not only in-memory `done::run` calls.
2. Both query and done use dot-prefix as “system.” Current `.verify-*` tasks can be pipeline-eligible, so confirm the broad bypass does not let an unrelated dot-prefixed task complete over a failed source. Prefer the canonical system predicate and document the `.verify-*` exception.
3. Test FLIP-enabled and FLIP-disabled graph shapes: evaluate depends on `.flip-X` in one and directly on `X` in the other.
4. Confirm regular local, remote/federated, and loop dependents remain blocked; the bypass must affect only the rescue edge, not `is_dep_satisfied` generally.
5. Keep the negative regular-dependent regression and add a permanent credential-free smoke for the actual dispatcher transition. No source conflict blocks this extraction even though a full integration merge conflicts in `done.rs`.

### `02204b19` — done refusals must not poison provider health

**Current status:** intent remains valid; exact patch fits. It should be treated as reusable core requiring redesign around current routing/telemetry, not just a copied phrase list.

**Hidden dependencies / gaps:**

1. `track_provider_health` reconstructs “stderr” from `failure_reason` or only the last ten output lines. A multi-line `wg done` refusal can be absent/truncated, so classifier unit tests do not prove the live triage path sees the marker.
2. Current `done.rs` has evolved deliverable, smoke, verify, validation-log, worktree, merge, and agent escape refusals. One candidate phrase (`completion contract is unmet`) is no longer emitted by current `done.rs`; future wording will drift again. Prefer a structured failure class/provenance from the wrapper/done command.
3. `extract_provider_id` still special-cases legacy `native:*`. Handler-first routes now include `claude`, `codex`, `nex`, and `pi`, with provider wires inside the model dialect. Collapsing all `nex` or all `pi` failures can pause unrelated endpoints/models; this must resolve the actual handler+endpoint/wire identity.
4. Do not confuse this LLM-handler circuit breaker with WG-Exec's remote `ProviderRegistry`, trust level, signed liveness, leases, or result verification.
5. Land after or with the `9a595528` rescue regression, then prove the actual wrapper/done-refusal/dead-agent/triage path keeps the counter at zero while a real provider 401 still increments only the correct route.

## Dependency-ordered action matrix

“Prepare” means a fresh current-main branch after human authorization; this report authorizes no source or PR mutation.

| Order | Batch | Classification / action | Dependencies and current-main proof |
|---:|---|---|---|
| 0 | Upstream #57 | **Security-review required; remain in existing PR** | Wait for a head or timeline change. Luca must replace `gh` flag denylisting with a read-only allowlist and add the no-browser-side-effect public-path test; then exact-head review/CI. Do not poll unchanged `6509e4cf` and do not import old integration commits. |
| 1 | `9a595528` rescue deadlock | **Reusable core; prepare first** | Smallest independent fix. Add credential-free real dispatcher crash→FLIP/eval smoke, FLIP-off variant, regular/dot/remote negatives; preserve final #54/#56 done gates. |
| 2 | `02204b19` provider classification | **Reusable core; redesign after order 1** | Introduce structured completion-refusal provenance and handler+endpoint identity; test current wrapper output and all current done gates. Keep WG-Exec provider state separate. |
| 3 | `d25cdd59` background process control | **Reusable core + process security review** | Align with the containment decision from #57 without waiting to copy its code. Track/kill/reap a process group, address shell/path policy, and run repeated macOS/Linux descendant tests. Explicitly scope out Pi/WG-Exec or share a proven helper. |
| 4 | `567c6a21` IPC noise | **Reusable core; reimplement, do not cherry-pick** | Current TUI/service restart work owns `service/ipc.rs`. Add actual TUI/chat connect-close/restart/health flow plus errno unit matrix; ensure EINVAL is not masking local misuse. |
| 5 | Provider recovery `afd79373`/`9b823397` | **Reusable later** | Only after order 2's route identity/classification. Bound probe command/timeout, retain operator attribution, and test per-route pause/recovery for claude/codex/nex/pi. |
| 6 | Cron `b5987388` → `f9f92435` + `83d7cf82` | **Reusable later** | Instance semantics first, protection/rearm second, smoke last. Rebase around current cleanup/graph/manifest behavior; keep Casa daily-digest wiring out. |
| 7 | Telegram runtime `57e94067` then `ccc9597a` | **Reusable after identity/review seam** | Final #51 is landed, but rebuild in current Telegram files. Prove token redaction, connection reuse, bots-map-only send/listen, numeric sender identity, and review-before-task-consumption. |
| 8 | Generic conversation substrate, ending with voice `057bb21e` | **Design boundary, then split batches** | Sequence polling → dedupe/sender identity → 1:1 compose → addressing/election → discussions/buttons → media. Reuse canonical trust/review, handler-first model routing, and product-neutral configuration. Voice is last because it adds untrusted bytes and an external gateway. |
| 9 | Auth/disposable `e2ebba9c`/`c3810107` and `a3d3a513`/`9295fd09` | **Security/policy review before code** | Reconcile secrets backend, final scope guard, Pi execution, review, and WG-Exec custody/result acceptance. No implementation extraction before the threat/policy decision. |
| 10 | Casa PRs #23/#25/#26 and integration Casa stack | **Casa adapter/product** | Keep on Casa line. First define a narrow adapter interface to WG identity/review/exec, then rebase within Casa. Upstream only product-neutral primitives accepted in orders 4–8. |

## Bottom line for downstream tasks

- `prepare-current-main` can start with `9a595528`; `02204b19` follows with structured telemetry/routing work; `d25cdd59` is independent in source but must take the process-containment review seriously.
- `study-luca-casa` should treat Casa gateway identity, feed provenance, lifecycle delivery, and conversational task creation as adapters to WG-Fed/Review/Exec—not competing trust systems. PRs #23/#25/#26 make those seams more explicit.
- The prior “finish four active upstream PRs” gate has collapsed to one: #57. It is green but not review-clean, and Luca has not yet responded to or revised after the current browser-environment escape finding.
- No communication action was taken. The July 16 draft message remains a draft and should be refreshed to mention merged #51/#54/#56, open #57, and new fork PRs #22–#26 before any human-authorized posting.
