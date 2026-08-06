# Pi compaction recovery patterns

Status: source audit, 2026-08-06. This report recommends a design direction for
WG task workers; it does **not** implement one.

## Scope and evidence rules

The primary baseline is the globally installed
`@earendil-works/pi-coding-agent` **0.83.0** (`pi --version`), whose npm record
pins git commit `845d6ff1f6643aba440341cce877ce1c43ebbc39` [S1]. Findings marked
**verified** below come from that installed distribution or an exact package
source revision. **Reported** means an issue/README claim that was not
independently reproduced here. Third-party packages were not installed or run;
their code was inspected, and release source is distinguished from a newer
repository head where they differ.

The installed package root used throughout is:

```text
/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/
  @earendil-works/pi-coding-agent/
```

## Executive result

Pi 0.83.0 has good primitives but intentionally does not infer unfinished work:

* threshold compaction happens after a low-level agent run and has
  `willRetry: false`;
* overflow compaction retries an interrupted run at most once;
* queued steering/follow-up messages survive compaction and are drained;
* messages queued by an `agent_end` extension are also drained (PR #5115);
* `agent_settled` is the true no-more-automatic-work boundary, unlike
  `agent_end`.

None of those primitives creates a continuation when threshold compaction ends
with an **empty queue**. That is the still-relevant kernel of issue #6424.
Third-party packages either add an unconditional/heuristic nudge, specialize in
watchdog detection, or precompute compaction. Their timers and background jobs
are not a dependable fit for Pi's one-shot `--mode json`, which returns after
the awaited prompt settles and then disposes the runtime.

For WG workers, the bounded recommendation is therefore: keep Pi's compactor,
observe its canonical events, and let WG own a durable, task-aware,
idempotent continuation decision. Do not adopt a third-party compactor or a
free-running idle watchdog as the authority.

## Pi 0.83.0: exact lifecycle and API surface

### Trigger and continuation sequence

The installed code follows this sequence (**verified**):

1. `_runAgentPrompt()` marks the session active, awaits `agent.prompt()`, and
   then loops over `_handlePostAgentRun()`; each `true` result invokes
   `agent.continue()` (`dist/core/agent-session.js:744-755`).
2. The low-level agent emits `agent_end`; Pi awaits extension `agent_end`
   handlers (`agent-session.js:432-434`). This is not settlement.
3. `_handlePostAgentRun()` considers transient retry, then compaction, then
   returns `agent.hasQueuedMessages()` for messages queued during `agent_end`
   (`agent-session.js:759-781`). Lines 779-781 are the installed form of PR
   #5115.
4. `_checkCompaction()` handles overflow before threshold
   (`agent-session.js:1497-1586`).
5. A successful auto-compaction emits `session_compact`, then the public
   `compaction_end`; overflow returns `true`, while threshold returns only
   `agent.hasQueuedMessages()` (`agent-session.js:1674-1709`).
6. Only after retries, compaction, and queued continuations are exhausted does
   `_emitAgentSettled()` clear the active flag and emit extension/public
   `agent_settled` (`agent-session.js:314-322`).

There is also a pre-prompt check for a prior aborted/large assistant response.
It compacts before appending the newly supplied user prompt and deliberately
does **not** call `continue()` because that user prompt is about to start the
run (`agent-session.js:858-863`; upstream fix #6074 [S8]).

### Exact APIs, events, and installed locations

| Required item | Installed source and exact behavior |
| --- | --- |
| Threshold | `dist/core/compaction/compaction.js:160-164`: `contextTokens > contextWindow - settings.reserveTokens`. Defaults are `reserveTokens: 16384`, `keepRecentTokens: 20000` at lines 74-78; settings resolve the same defaults in `dist/core/settings-manager.js:520-530`. Threshold dispatch is `agent-session.js:1558-1585` and calls `_runAutoCompaction("threshold", false)`. |
| Overflow | `dist/core/agent-session.js:1529-1556`: `isContextOverflow(...)` is checked first. An interrupted response gets `willRetry: true`; `_overflowRecoveryAttempted` caps compact-and-retry at one and emits a loud failure on the second attempt (lines 1538-1549). A successful over-window response (`stopReason === "stop"`) compacts without replaying an already completed answer. |
| `agent_end` | Type: `dist/core/extensions/types.d.ts:539-543`. Emission to extensions: `agent-session.js:432-434`. It is a low-level run boundary; retry/compaction/queued work may follow. |
| `agent_settled` | Type and contract: `types.d.ts:544-547` (“no automatic retry, compaction, or queued continuation”). Runtime: `agent-session.js:314-322`. It is also documented and emitted in JSON/RPC [S3, S4]. |
| `session_compact` | Type: `types.d.ts:452-460`, including `compactionEntry`, `fromExtension`, `reason: manual | threshold | overflow`, and `willRetry`. Manual emission is `agent-session.js:1440-1447`; auto emission is lines 1680-1688. It fires only after a compaction entry is saved. |
| `ctx.isIdle()` | Public type: `types.d.ts:231-232`. Runtime meaning is `!_isAgentRunActive` at `agent-session.js:587-594`, and the extension binding is line 1896. The active flag spans retry, compaction, and queued continuation, not only provider streaming. |
| `ctx.hasPendingMessages()` | Public type: `types.d.ts:239-240`. It is bound to `pendingMessageCount > 0` at `agent-session.js:1906`; that count is the mirrored steering plus follow-up queues at lines 1147-1157. |
| `ctx.compact()` | Public type: `types.d.ts:245-246`. It is fire-and-forget and reports through `onComplete`/`onError` (`agent-session.js:1911-1922`). The underlying manual compact aborts current work first and emits reason `manual` (`agent-session.js:1363-1478`). |

The installed docs agree: `session_before_compact`/`session_compact` carry
`reason` and `willRetry`; `agent_end` can precede retry/compaction/follow-up;
`agent_settled` is the status-integration boundary; `ctx.isIdle()` remains
false across automatic continuation; and `pi.sendUserMessage()` requires
`deliverAs: "steer" | "followUp"` while streaming [S2, S3].

### Manual, failure, and retry behavior

* Manual `/compact` uses the same before/after extension events but
  `willRetry: false`; it aborts an active run before summarization
  (`agent-session.js:1363-1478`).
* A cancelled auto-compaction emits `compaction_end` with `aborted: true` and
  returns no continuation (`agent-session.js:1627-1635`, 1664-1672).
* A failed auto-compaction emits `compaction_end` with `errorMessage` and
  returns false (`agent-session.js:1711-1725`). It does **not** emit
  `session_compact`, because no entry was persisted.
* Summarization calls use Pi's normal retry settings. Installed defaults are
  three retries with 2-second exponential base delay
  (`settings-manager.js:553-558`; callbacks at
  `agent-session.js:2087-2113`). This is separate from the one compact-and-
  retry cap for context overflow.
* The JSON stream exposes `queue_update`, `compaction_start`,
  `compaction_end`, summarization retry events, `agent_end`, and
  `agent_settled` [S4]. Thus an external host need not scrape prose.

## Upstream queue fixes and the remaining empty-queue gap

### Earlier auto-compaction queue work

Commit `b050c582` (“resume queued messages after auto-compaction”) added
`Agent.hasQueuedMessages()`, taught `Agent.continue()` to drain steering or
follow-up messages even from an assistant-tailed transcript, and kicked the
agent after compaction only when such a queue existed [S5]. Later settlement
refactoring removed the timer kick and made this an awaited boolean handoff
[S6]. PR #6730 subsequently preserved each queued message's steering/follow-up
mode when the interactive compaction queue flushes into a still-active run
[S7].

These changes solve **delivery of already queued intent**. They deliberately do
not decide that a completed assistant response was semantically unfinished.

### PR #5115

PR #5115 merged one additional check after `_checkCompaction()`:

```ts
// messages queued by agent_end extension handlers
return this.agent.hasQueuedMessages();
```

Its integration test installs an `agent_end` handler that calls
`sendUserMessage("conflict report", { deliverAs: "followUp" })`, and verifies a
second response [S9]. Installed 0.83.0 contains the fix at
`dist/core/agent-session.js:779-781`.

**Why it does not solve threshold compaction with an empty queue:** after
threshold compaction, `_runAutoCompaction("threshold", false)` itself returns
only `agent.hasQueuedMessages()` (`agent-session.js:1707-1709`). If no user or
extension has enqueued a message, that is false. `_checkCompaction()` is then
false, PR #5115 performs the same empty-queue check and also returns false, and
Pi emits `agent_settled`. PR #5115 is a drain, not a nudge generator.

### Issue #6424

Issue #6424 **reported** this exact symptom on 0.80.3: a long task ended on a
placeholder, threshold compaction persisted next steps, and the session went
idle. It proposed an opt-in hidden threshold follow-up [S10]. The issue was
closed automatically under the new-contributor policy; its only comment says
maintainers may review auto-closed issues, so the `no-action` label is not
technical evidence that the mechanism is wrong [S11]. The particular incident
was not reproduced here, but the empty-queue control flow remains present and
is directly verified in installed 0.83.0.

## Ecosystem approaches (source, not marketing)

### `@capyup/pi-auto-compact` 0.2.4

Npm 0.2.4 pins git `2d8bde79`, equal to the inspected repository head [S12].

**Verified from source [S13]:**

* Trigger timing: checks `turn_start`, `context`, `turn_end`, and resumed/forked
  `session_start`. The default threshold is 90% of model context; emergency
  `context` handling replaces older messages with a truncation notice and then
  schedules `ctx.compact()`.
* Delivery: `ctx.compact({ onComplete })`, then `setImmediate`; if
  `ctx.isIdle()`, it sends a phase-specific **user** follow-up. The defer is an
  explicit attempt to let Pi's own queued user input win the race.
* Guards: one in-memory `pendingCompaction` boolean; the nudge checks idle but
  does **not** check `ctx.hasPendingMessages()` directly.
* Exactly-once/retry: no compaction-id ledger and no nudge retry cap. `onError`
  merely clears the boolean. A restart forgets all state.
* Manual/overflow: calls to `ctx.compact()` enter Pi's `reason: "manual"` path,
  even though the extension initiated them automatically. User `/compact` does
  not invoke this closure. Built-in threshold/overflow remain separate.

Two marketing claims need qualification. First, the “mid-turn” check looks for
content type `tool_use`, while Pi 0.83.0 documents/emits `toolCall`; therefore
that branch is not source-compatible with the installed message shape. Second,
the emergency notice says messages “were summarized,” but the `context` hook
only substitutes a notice non-destructively; actual summarization is the later,
fire-and-forget `ctx.compact()`.

**Headless JSON:** not dependable. The extension uses `ctx.compact()` without
awaiting it and uses `setImmediate` for the nudge. One-shot JSON awaits
`session.prompt()` and then disposes the runtime (`dist/modes/print-mode.js:
93-99,124-129`); it does not promise to await extension background
work. TUI or long-lived RPC is a different environment.

### `@badliveware/pi-compaction-continue` 0.1.5

Npm 0.1.5 pins `d0901767`; the published source imports the former
`@mariozechner/*` names, which Pi 0.83.0 intentionally aliases to the installed
Earendil modules (`dist/core/extensions/loader.js:47-54,94-101`) [S14, S15].
The newer repository head inspected here is `eb6ae342`; conclusions below use
the published revision where behavior differs [S16].

**Verified from published source [S17]:**

* Trigger timing: records analysis at `session_before_compact`, reacts to
  successful `session_compact`, watches `turn_end` for an assistant that
  promised to continue or stopped blank, and re-scans on `session_start`.
* Delivery: after 1 second for compaction or 2 seconds for an assistant stall,
  sends a visible custom message via
  `pi.sendMessage(..., { triggerTurn: true })`. Its prompt asks the model to
  call `watchdog_answer(done: ...)` before deciding whether to continue.
* Guards: requires `ctx.isIdle()` and `!ctx.hasPendingMessages()`.
* Exactly-once/retry: `lastRecoveredCompactionId` suppresses a second nudge for
  one compaction **in the current process**. Assistant-stall recovery is capped
  at three consecutive nudges. There is no retry after a busy/pending skip.
  A second compaction cancels the first pending compaction timer.
* Manual/overflow: overflow is inferred heuristically from the parent assistant
  (`length` or selected error text), rather than using `event.reason`. A normal
  threshold/manual compaction is nudged only for a detected active `.ralph`
  loop. Therefore this package is **not** a generic fix for #6424's ordinary
  empty-queue threshold case.
* Restart: `session_start` can rediscover a leaf compaction/stalled assistant,
  but the in-memory compaction-id guard is reset, so it is at-least-once rather
  than durable exactly-once.

This is the strongest examined terminal-work safeguard—the prompt explicitly
allows `done: true`—but it delegates correctness to another model turn and
heuristics. False-positive continuation phrases, false-negative terminal
answers, and extra cost remain possible.

**Headless JSON:** the event APIs exist, but one-shot behavior is not reliable.
Its 1s/2s timers are cleared by `session_shutdown`, while print/JSON disposes
as soon as the awaited prompt is finished. It is suitable in principle for a
long-lived TUI/RPC process, not an authoritative one-shot worker recovery.

### `pi-async-compaction`

There are two materially different artifacts:

* npm **0.1.6**, git `c2e4fad4`, declares peer compatibility only for Pi
  `>=0.80.3 <0.81.0` [S18]; it is not declared compatible with installed 0.83.0;
* repository head `43234dc0` adds “auto resume after forced async compaction”
  but is newer than the published npm revision [S19]. Treat that behavior as
  unreleased source, not package behavior.

**Published 0.1.6 source [S20]:** starts background work at `turn_end` once
usage is between an early ratio (default 0.8) and Pi's force threshold. A job is
`pending`, `ready`, `stale`, or `failed`; it snapshots session/model/thinking/
settings/cut-point identity, times out after five minutes, and validates all of
those plus post-apply size before handing its result to
`session_before_compact`. It applies only when idle with no pending messages.
`agent_end` polls a ready job up to 40 times at 25 ms (about one second). It
precomputes and safely applies a summary; it does **not** deliver a continuation
nudge in 0.1.6.

**Unreleased head source [S21]:** may force-apply a ready result during an
abortable active run when above the async ratio and no messages are pending. It
records the in-memory job id, aborts, triggers compaction, and on a matching
`session_compact` schedules the user message `"continue"`. It clears the marker
before scheduling, so it is once per in-memory forced job, but send failure has
no retry and restart has no recovery. Idle application still has no nudge,
which is appropriate because it did not interrupt active work.

**Manual/overflow:** a valid ready result can satisfy Pi's next manual,
threshold, or overflow `session_before_compact`; custom manual instructions
invalidate it. Otherwise Pi falls back to synchronous compaction. **Headless
JSON:** background work is fire-and-forget and `session_shutdown` marks it
stale, so a one-shot worker will normally dispose before this approach can
finish/apply.

## Comparison

| Pattern | Trigger | Continuation delivery | Idle / queue guards | Exactly-once and retry cap | Manual / overflow | One-shot JSON |
| --- | --- | --- | --- | --- | --- | --- |
| Pi 0.83 core | Post-run threshold/overflow; pre-next-prompt; `/compact` | Only existing queue; overflow replay | Active state spans all automatic work; core queue checks | Queue dequeue; overflow compact-and-retry once; summarization retries default 3 | Native reasons and `willRetry` | **Yes**, canonical synchronous events |
| PR #5115 | After `agent_end` handlers | Drains handler-enqueued follow-up | `hasQueuedMessages()` | Queue semantics; creates nothing | Independent of reason | **Yes**, in core |
| capyup 0.2.4 | `turn_start`, `context`, claimed `turn_end`, resume/fork | User nudge after `ctx.compact` completion | `setImmediate` + idle; pending boolean | No durable id; no nudge retry cap | Own compactions look manual; built-in paths separate | **No guarantee** |
| badliveware 0.1.5 | Successful compaction, stall `turn_end`, restart | Custom watchdog turn + answer tool | idle **and** no pending | Per-compaction in-memory; stall cap 3; no busy retry | Overflow heuristic; threshold only active Ralph | **No guarantee** |
| async 0.1.6 | Early `turn_end` background job | None | idle + no pending; ~1s apply polling | Job/snapshot ids in memory; 5m timeout | Ready result can serve native paths | **No** |
| async unreleased head | Same, plus force-active apply | User `"continue"` after matching job marker | no pending; active must be abortable | Once in memory; no send retry/restart recovery | Same | **No guarantee** |

## Race, loop, and loss hazards

At least the following must be designed out for WG:

1. **Duplicate nudge:** both an extension and the WG wrapper react to one
   compaction, or a restart replays an in-memory-only guard. Two follow-ups can
   execute the same side effect twice.
2. **Terminal-after-compaction loop:** a threshold compaction can follow a
   genuinely complete final response. An unconditional nudge spends another
   turn and may turn “done” into invented work; repeated nudges can loop.
3. **Queued user work / TOCTOU:** an idle/no-pending check can pass just before
   real user input arrives. The synthetic nudge may race, reorder, or suppress
   that input. PR #6730 shows that even preserving steer versus follow-up mode
   matters.
4. **Failed or cancelled compaction:** a flag set at
   `session_before_compact` must not authorize continuation unless a matching
   successful `session_compact`/`compaction_end.result` exists. Fire-and-forget
   failures otherwise leave stale state or a nudge against uncompacted context.
5. **Crash/restart window:** crashing after the compaction entry is durable but
   before the nudge is durable loses continuation; crashing after nudge
   submission but before recording it can duplicate continuation on restart.
6. **Repeated compaction:** a summary that remains near the threshold can cause
   the synthetic continuation to compact again. Without a bounded epoch/turn
   budget, “compact → nudge → compact” is an autonomous loop.
7. **Stale session/process:** timers capture an extension context that becomes
   invalid after reload/session replacement. Pi now rejects stale contexts,
   but an external host must likewise fence by session, task attempt, and
   process generation.
8. **False semantic inference:** prose such as “I'll continue” can be quoted,
   historical, or already superseded; conversely a placeholder can omit the
   phrase. Semantic watchdogs are advisory evidence, not an exactly-once
   authority.

## Bounded recommendation for WG task workers

Use Pi core for compaction and WG for authority:

1. **Observe, do not replace.** Consume Pi 0.83 JSON events. Authorize a kick
   only after a successful `compaction_end`/persisted `session_compact` with
   `reason === "threshold"` and `willRetry === false`. Never kick overflow
   while Pi has already promised a retry; never kick failed/aborted/manual
   compaction by default.
2. **Prefer in-lifecycle delivery when possible.** A WG-owned Pi extension can
   enqueue one `followUp` synchronously from the successful `session_compact`
   handler, so the existing auto-compaction queue path drains it before
   `agent_settled` and remains compatible with one-shot JSON. Do not rely on a
   post-settlement timer. If WG instead starts a replacement process, treat it
   as a new fenced process epoch, not a loose extension timer.
3. **Make WG's task ledger decisive.** Suppress continuation if the WG attempt
   is terminal/cancelled, a real message is queued, or no continuation lease
   remains. The compaction summary's “next steps” and assistant prose may inform
   the nudge but must not override terminal task state.
4. **Use a durable idempotency key**, minimally
   `(task_id, attempt/fence, pi_session_id, compaction_entry_id,
   continuation_epoch)`. Record authorization before delivery and reconcile an
   indeterminate send on restart. One successful kick per compaction entry,
   with a small per-attempt epoch cap and elapsed-time cap, is safer than
   unlimited “keep going.”
5. **Require observable completion.** After delivery, accept either a later
   agent run followed by `agent_settled`, or a new fenced process with matching
   session proof. If the model is actually done, it should finalize the WG task;
   do not auto-kick again merely because it used terminal prose.

This recommendation is intentionally narrow: it covers WG's autonomous task
workers, not general interactive chat, does not change Pi's default semantics,
and does not select third-party summarization policy.

## Source inventory

All URLs and local sources used for claims above are recorded here.

* **[S1] Installed/npm Pi 0.83.0 metadata:**
  <https://registry.npmjs.org/@earendil-works%2Fpi-coding-agent/0.83.0>;
  local `package.json`, `dist/core/agent-session.js`,
  `dist/core/compaction/compaction.js`, `dist/core/settings-manager.js`,
  `dist/core/extensions/types.d.ts`, `dist/core/extensions/loader.js`, and
  `dist/modes/print-mode.js` under the package root stated above. Upstream
  pinned tree:
  <https://github.com/earendil-works/pi/tree/845d6ff1f6643aba440341cce877ce1c43ebbc39/packages/coding-agent>.
* **[S2] Installed compaction docs:** local `docs/compaction.md`; upstream
  pinned:
  <https://github.com/earendil-works/pi/blob/845d6ff1f6643aba440341cce877ce1c43ebbc39/packages/coding-agent/docs/compaction.md>.
* **[S3] Installed extension docs:** local `docs/extensions.md`; upstream
  pinned:
  <https://github.com/earendil-works/pi/blob/845d6ff1f6643aba440341cce877ce1c43ebbc39/packages/coding-agent/docs/extensions.md>.
* **[S4] Installed JSON/RPC docs:** local `docs/json.md`, `docs/rpc.md`;
  <https://github.com/earendil-works/pi/blob/845d6ff1f6643aba440341cce877ce1c43ebbc39/packages/coding-agent/docs/json.md> and
  <https://github.com/earendil-works/pi/blob/845d6ff1f6643aba440341cce877ce1c43ebbc39/packages/coding-agent/docs/rpc.md>.
* **[S5] Earlier queued-message recovery commit `b050c582`:**
  <https://github.com/earendil-works/pi/commit/b050c582a1e578fee39712cc8d05168d4ce911be> and
  <https://api.github.com/repos/earendil-works/pi/commits/b050c582a1e578fee39712cc8d05168d4ce911be>.
* **[S6] Awaited settlement refactor `32bcdc97`:**
  <https://github.com/earendil-works/pi/commit/32bcdc9739d4b806c209a584c6c81a7a22366482>.
* **[S7] PR #6730, compaction queue mode preservation:**
  <https://github.com/earendil-works/pi/pull/6730> and
  <https://api.github.com/repos/earendil-works/pi/pulls/6730/files>.
* **[S8] PR #6074/pre-prompt no-continue merge:**
  <https://github.com/earendil-works/pi/pull/6074> and
  <https://github.com/earendil-works/pi/commit/a8c692c712c52ab49607344ca659314322c533bf>.
* **[S9] PR #5115 and exact changed files:**
  <https://github.com/earendil-works/pi/pull/5115> and
  <https://api.github.com/repos/earendil-works/pi/pulls/5115/files>.
* **[S10] Issue #6424:**
  <https://github.com/earendil-works/pi/issues/6424>.
* **[S11] Issue #6424 auto-close comment:**
  <https://github.com/earendil-works/pi/issues/6424#issuecomment-4915771144>.
* **[S12] capyup npm metadata/repository:**
  <https://registry.npmjs.org/@capyup%2Fpi-auto-compact/latest> and
  <https://github.com/capyup/pi-auto-compact/tree/2d8bde795191f1913618b315b104d8563821fd39>.
* **[S13] capyup exact extension source:**
  <https://github.com/capyup/pi-auto-compact/blob/2d8bde795191f1913618b315b104d8563821fd39/extensions/auto-compact.ts>;
  inspected clone `/tmp/pi-github-repos/capyup/pi-auto-compact`.
* **[S14] badliveware npm metadata:**
  <https://registry.npmjs.org/@badliveware%2Fpi-compaction-continue/latest>.
* **[S15] Pi 0.83 legacy import aliases:** pinned upstream
  <https://github.com/earendil-works/pi/blob/845d6ff1f6643aba440341cce877ce1c43ebbc39/packages/coding-agent/src/core/extensions/loader.ts>;
  installed `dist/core/extensions/loader.js:47-54,94-101`.
* **[S16] badliveware repository/current source:**
  <https://github.com/BadLiveware/pi/tree/eb6ae3429771e7002476c332bd3dc07746474a81/agent/extensions/public/compaction-continue>;
  inspected clone
  `/tmp/pi-github-repos/BadLiveware/pi@main/agent/extensions/public/compaction-continue`.
* **[S17] badliveware published 0.1.5 source:**
  <https://github.com/BadLiveware/pi/tree/d0901767319343b690155851ac26379133b81d2d/agent/extensions/public/compaction-continue>, especially
  <https://github.com/BadLiveware/pi/blob/d0901767319343b690155851ac26379133b81d2d/agent/extensions/public/compaction-continue/src/runtime.ts>,
  `src/analysis.ts`, `src/loop-state.ts`, and `src/model.ts` at the same revision.
* **[S18] async npm 0.1.6 metadata:**
  <https://registry.npmjs.org/pi-async-compaction/latest>.
* **[S19] async repository head inspected:**
  <https://github.com/almogdepaz/pi-async-compaction/tree/43234dc0a4e379c41abba2eff9bcdc16280f8bd4>;
  clone `/tmp/pi-github-repos/almogdepaz/pi-async-compaction`.
* **[S20] async published 0.1.6 source:**
  <https://github.com/almogdepaz/pi-async-compaction/tree/c2e4fad4d19de9993d2cfb038dac797e347ef6bf>, especially
  <https://github.com/almogdepaz/pi-async-compaction/blob/c2e4fad4d19de9993d2cfb038dac797e347ef6bf/src/core.ts>,
  `src/job.ts`, `src/runtime-state.ts`, `src/constants.ts`, and
  `src/validation.ts` at that revision.
* **[S21] async unreleased continuation source:**
  <https://github.com/almogdepaz/pi-async-compaction/blob/43234dc0a4e379c41abba2eff9bcdc16280f8bd4/src/core.ts>,
  <https://github.com/almogdepaz/pi-async-compaction/blob/43234dc0a4e379c41abba2eff9bcdc16280f8bd4/src/job.ts>, and
  <https://github.com/almogdepaz/pi-async-compaction/blob/43234dc0a4e379c41abba2eff9bcdc16280f8bd4/src/constants.ts>.
* **[S22] Supplemental upstream failure/race reports consulted:** assistant-tail
  analysis <https://github.com/earendil-works/pi/issues/5056>, settlement meta
  issue <https://github.com/earendil-works/pi/issues/5886>, the later manual
  compaction-queue commit
  <https://github.com/earendil-works/pi/commit/3852cb2b813243f3c81a873ed97827798aa7dbeb>,
  and the auto-compaction queue fixture commit
  <https://github.com/earendil-works/pi/commit/fd1ba2c7feaedb78e5d87107f6e6a274044121b8>.
