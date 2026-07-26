# Pi task-worker session watchdog and continuation protocol

**Status:** Implementation-ready amended design; lifecycle-kernel extension requires ratification with the authoritative lifecycle implementation

**Date:** 2026-07-26

**Owner:** `design-pi-stalled`; completion/timeout policy amended by `amend-pi-watchdog`

**Normative dependency:** [Simplified authoritative task lifecycle](design-simplified-task-lifecycle.md)

**Scope:** unattended **Pi task workers** only

## 1. Decision

A silent, exited, or nonterminal Pi child is recoverable only by starting a new,
fenced **process epoch inside the same execution attempt** and reopening the
exact durable Pi session, active branch, frozen route, and worktree. The
watchdog is a process-epoch supervisor beneath `LifecycleKernel`; it is not a
retry controller.

The implementation MUST preserve these identities:

```text
(task_id, generation, attempt_id, attempt_fence, worktree_lease_epoch)
    └── immutable for the whole source attempt
        ├── pi_session_proof + route_snapshot: immutable
        └── process_epoch: 0, 1, 2, ... (fenced replacement processes)
```

It MUST NOT:

* reopen/reset the task;
* create an attempt or generation;
* transfer the worktree lease;
* fall back to a fresh Pi session, another model/endpoint, Claude, or Codex;
* treat an ordinary message, log, heartbeat, PID, or wall-clock runtime as
  useful-work evidence;
* mark a task done or failed; or
* race the generic dead-owner reaper.

The fixed initial **soft observation threshold** is exactly **300 seconds since
the last meaningful progress evidence**. Crossing it records suspicion and
requests a read-only probe only. It does not authorize a prompt, signal,
process fence, lifecycle disposition, or completion inference. A separate
`hard_resume_after_secs`, resolved from the frozen route and observed phase,
is a conservative not-before threshold for safe automatic continuation. The
initial free/low-QoS provider/TTFT and generation/inter-token values are no
lower than 900 seconds. Some states deliberately have no automatic hard
threshold. All initial thresholds are static, configuration-validated, and
never learned or decreased from telemetry.

Automatic continuation defaults to at most **3 replacement epochs** and **1,800
seconds (30 minutes) of reserved continuation runtime**. Both budgets are
charged durably before a replacement launch and never refunded or replenished
by a tick or restart. They bound recovery exposure; they are not deadlines for
the original task or a progressing worker. The rationale is in §11.

## 2. Authority and lifecycle reconciliation

### 2.1 One writer remains one writer

The lifecycle kernel in the normative design remains the only writer of task,
generation, attempt, fence, and worktree-lease state. Watchdog ticks, Pi events,
process exits, prompts, status queries, ordinary WG messages, and diagnostics
submit evidence and typed requests only.

The watchdog classifications are projections over process observations and a
reconciliation/readiness hold:

```text
Active | WaitingUser | LongTool | Suspect | HardResumeEligible |
NeedsFinalization | Fencing | Resuming | StalledOperatorRequired
```

Neither `Stalled` nor `NeedsFinalization` is a canonical task-generation or
attempt status. `NeedsFinalization` means only **Pi stopped autonomously without
completing the WG protocol**. It is not success evidence, failure evidence, or
a `Done` state. In `NeedsFinalization` or `StalledOperatorRequired`, the
generation remains `Running`, the attempt remains current and exclusively
owned, its worktree remains `Active` under the same attempt fence, and readiness
is held by one deduplicated reconciliation issue. `StalledOperatorRequired` is
non-dispatchable until an operator either grants a same-session continuation or
asks the lifecycle kernel to fail/cancel/abandon it.

### 2.2 Required kernel extension: `PiContinuationAuthorization`

The generic mapping in lifecycle design §8 remains correct:
`RuntimeExit`/`NoCompletionProtocol` normally becomes one terminal failed
attempt. Pi continuation is a narrow, typed **pre-terminal classification**, not
observer-side special casing.

When a Pi attempt enters `Running`, the kernel may append:

```rust
PiContinuationAuthorization {
    authorization_id: EventId,
    task_id: TaskId,
    generation: u64,
    attempt_id: AttemptId,
    attempt_fence: u64,
    worktree_lease_epoch: u64,
    session_proof_digest: Digest,
    route_snapshot_digest: Digest,
    state: Active | HeldOperatorRequired | Consumed | Revoked,
    max_replacement_epochs: u32,       // default 3
    max_reserved_elapsed_secs: u64,    // default 1800
    issued_by_policy: PolicyId,
}
```

A current child exit is submitted as `PiProcessEpochExited`; it is not submitted
as `AttemptLost`. Under the lifecycle lock, the kernel checks the exact attempt,
fence, current process epoch, authorization, terminal reservation, session/tool
safety, and budgets:

* **authorization active and prompt-safe after exact exit/reap proof:** append
  `PiProcessExitDeferred`, classify the running-held attempt as
  `NeedsFinalization`, and emit the same-session completion action promptly;
* **authorization active and safely continuable after a phase-policy hard
  threshold and hard-resume grace:** leave the attempt/generation running-held
  and emit the continuation fencing outbox action only after every §5.3 guard
  passes;
* **proof, ownership, effect, reap, phase, or budget ambiguous:** change the
  authorization to `HeldOperatorRequired`, create/update one reconciliation
  issue, and leave the attempt running-held;
* **no policy-valid authorization:** retain the generic
  `AttemptFailed(RuntimeExit|NoCompletionProtocol)` mapping unchanged.

A settled/exited process is prompt-eligible because Pi or the OS supplied
explicit quiescence evidence, not because a timer expired. Exit status does not
select success or failure.

`HeldOperatorRequired` remains the typed reason the generic reconciler must not
terminalize the already-observed process exit. The authorization ends only when:

1. the kernel accepts a success, failure, park, cancel, or operator-abort
   disposition for the attempt;
2. an operator explicitly revokes it and requests a lifecycle disposition; or
3. the attempt/generation fence changes through a legal lifecycle transition.

Proof mismatch and budget exhaustion **hold** the authorization; they do not end
it and thereby accidentally enable generic failure on the next tick.

This extension belongs in the lifecycle kernel/event schema. An observer may
never suppress `AttemptFailed` by checking `executor == pi` on its own.

### 2.3 Relationship to waits and the dead-owner reaper

An accepted `wg wait`/park request terminalizes the current attempt as `Parked`
and moves the generation to `Waiting`, exactly as lifecycle design §§6 and 9
specify. A matching correlated input later opens the same generation and the
dispatcher creates a **new attempt** from the checkpoint. That is not a process
epoch continuation. Ordinary messages neither park nor wake anything.

The existing dead-owner/worktree mechanism remains the generic authority. It
must skip an exact current attempt carrying an `Active` or
`HeldOperatorRequired` Pi continuation authorization. It may still reap
terminal zombie processes after the lifecycle disposition. The later
`impl-supervisor-hard-agent` daemon work is serialized after this implementation
and must skip every live/nonterminal Pi attempt; it never resumes or reaps one.

## 3. Pi capability research

The installed Pi examined for this design is
`@earendil-works/pi-coding-agent 0.82.0`. The public documents were read in full:
`README.md`, `docs/sessions.md`, `session-format.md`, `json.md`, `rpc.md`,
`sdk.md`, `extensions.md`, `environment-variables.md`, and `compaction.md`, plus
the session/runtime/tool/provider examples. WG anchors were also inspected.

### 3.1 Durable session and branch identity

Pi supports all of the following:

* `--session-id <exact-id>`: find an exact ID in the supplied project/session
  directory, opening it if present and **creating a fresh session if absent**;
* `--session <path|id>`: open a path/ID, also creating at a nonexistent explicit
  path;
* `--session-dir <dir>`;
* `--continue`: most recent, which is forbidden for this protocol because it is
  not an exact proof;
* `--resume`: interactive selection, also forbidden;
* RPC `get_state`: `sessionId`, `sessionFile`, selected full model,
  `thinkingLevel`, stream/compaction state, and counts;
* RPC `get_entries`: append-order entries plus the current `leafId`, with a
  stable entry-ID cursor; and
* SDK/extension `SessionManager`: header, exact session ID/file/dir/cwd,
  entries, active branch, and leaf ID.

The v3 session is an append-only JSONL tree. Its first line is a header with
`id`, `cwd`, and version. Every later entry has an `id` and `parentId`. The
active leaf after reopening is the last appended entry; in-place branching
appends a child of an older parent, so the last entry still re-attests the
active branch. Compaction is append-only and retains full history; it does not
change the session ID.

Important implementation facts:

* `--session-id` silently creates if no exact file exists
  (`dist/main.js:112-117,264-271`). Therefore the watchdog MUST prove one and
  only one existing file before continuation and MUST reject Pi's creation
  warning/new header.
* Pi delays initial file creation until an assistant message exists
  (`dist/core/session-manager.js:632-688,733-759`). Continuation cannot be
  authorized until the initial process has produced a persisted assistant or
  the WG integration has explicitly flushed its attestation/session header.
* RPC `get_entries` reports both append history and `leafId`; it is stronger
  than parsing only `get_messages`, which omits abandoned branches and
  pre-compaction history.

### 3.2 Event and persistence ordering

Native JSON/RPC events include:

```text
agent_start
  turn_start
    message_start/update/end
    tool_execution_start/update/end (stable toolCallId)
    turn_end(message, toolResults)
  ...
agent_end(willRetry?)
agent_settled
```

`agent_end` is not necessarily settled: automatic provider retry, overflow
compaction/retry, summarization retry, or queued follow-up work may still run.
Only `agent_settled` says Pi will not continue automatically. The current WG
chat bridge stops at `agent_end` (`src/commands/pi_handler.rs:190-260`); the task
worker watchdog must use `agent_settled`.

RPC remains alive after settling and accepts another prompt; `abort` cancels an
active operation but is neither a task disposition nor a session replacement.
For a task worker, `agent_settled` without a kernel-accepted terminal/park
intent enters the nonterminal `NeedsFinalization` classification. Pi has
explicitly declared that it has no autonomous continuation, so the adapter does
not wait for the 300-second soft threshold: it promptly quiesces and closes the
idle process epoch, reconciles effects, and, when proof is safe, issues the one
bounded same-session completion action. A spontaneous EOF, exit code 0,
nonzero code, or signal is only an OS process observation. It also enters
`NeedsFinalization` after exact exit/reap classification; when safe, WG promptly
reopens the exact session/attempt/route/worktree for that action. None of these
observations proves success or failure while `PiContinuationAuthorization` is
active/held.

Pi exposes provider boundaries through extension hooks:

* `before_provider_headers` / `before_provider_request`: request about to be
  sent;
* `after_provider_response`: response headers received, **before** the body
  stream is consumed; and
* `message_update` token deltas: actual streamed text/thinking/tool-call data.

Provider hooks do not supply an upstream exactly-once request ID. WG may assign
a local observation ID, but it cannot claim a provider inference was billed
only once. Completed assistant usage is available on `turn_end.message.usage`
with Pi's native fields `{input, output, cacheRead, cacheWrite, totalTokens,
cost.total}`. The same cumulative snapshot appears on multiple message events,
so only the `turn_end` occurrence is an accounting receipt. Text/thinking delta
records prove active token progress before that receipt, but they do not prove
final usage or session durability.

Pi's ordering is especially important for tools
(`pi-agent-core/dist/agent-loop.js:90-132,260-347,515-548`):

1. a complete assistant message containing the stable tool call ID is emitted
   at `message_end` and persisted by `AgentSession`;
2. `tool_execution_start` is emitted;
3. the tool executes and may emit accumulated progress;
4. `tool_execution_end` is emitted; and
5. the `toolResult` message is emitted and persisted.

The start/end events themselves are **not native Pi session entries**
(`dist/core/agent-session.js:355-366,487-516`). A killed process can therefore
leave a persisted assistant tool call with no persisted result. The Pi
integration must synchronously append and fsync a WG call-intent receipt before
allowing execution and a completion receipt before returning from the end hook.
Pi's own `appendFileSync` is not an fsync guarantee, so the authoritative receipt
lives in WG's attempt evidence journal and is mirrored as a Pi custom entry.

### 3.3 Current WG gaps this design closes

* Task workers currently launch one-shot `pi --mode json -p` with no session
  identity (`src/service/executor.rs:1729-1753`).
* The generic command builder passes provider/model/reasoning but no exact Pi
  session (`src/commands/spawn/execution.rs:1783-1852,2529-2536`).
* `raw_stream.jsonl` is captured live, but `pi-stream-bridge` runs only after Pi
  exits (`src/commands/spawn/execution.rs:2716-2734`).
* The bridge maps session ID, tool names, turn usage, and final text, but drops
  tool call IDs, progress, provider phases, branch head, and settled state
  (`src/stream_event.rs:402-637`).
* The wrapper currently maps zero/nonzero exit directly to done/fail
  (`src/commands/spawn/execution.rs:2870-2948`), which the kernel extension must
  replace for an authorized Pi process epoch.
* Current Linux PID verification returns “same” when `/proc` is inconclusive and
  allows timestamp slack (`src/service/mod.rs:453-466`). Continuation fencing
  must use exact start ticks plus boot ID/nonce and fail closed.
* WG's Pi plugin discards `toolCallId` in every registered tool and does not pass
  lifecycle idempotency keys (`worksgood-pi/src/tools.ts`). That must change for
  receipt-backed continuation.

These are implementation seams, not reasons to weaken the proof.

## 4. Evidence model

### 4.1 Meaningful progress

`last_meaningful_progress_at` advances only when a newly sequenced event proves
work advanced. Replayed or cumulative duplicate data does not advance it.

| Signal | Meaningful? | Rule |
|---|---:|---|
| first `before_provider_request` for a new local provider-call ID | yes, once | phase advanced to provider call; starts TTFT clock |
| `after_provider_response` for that call | yes, once | headers arrived; body may still stall |
| nonempty new `text_delta`, `thinking_delta`, or completed `toolcall_delta` bytes | yes | hash/offset must advance; repeated cumulative partial does not count |
| persisted assistant `message_end`, user continuation input, tool result, `turn_end`, compaction end, or `agent_settled` | yes, once | durable/session sequence must advance |
| `tool_execution_start` with a newly fsynced call-intent | yes, once | also selects tool safety/lease state |
| `tool_execution_update` | yes only for a contract-valid tool | stable call ID and a monotonic progress counter/new partial digest; renews within hard lease |
| `tool_execution_end` plus fsynced completion receipt | yes | stable call ID/result digest |
| worktree mutation through a receipt-aware write/edit tool | yes | pre/postcondition digest recorded; generic filesystem polling does not count |
| accepted terminal or park intent | disarms | lifecycle event wins; no continuation detector remains armed |
| ordinary `wg log`, `wg msg_*`, `wg show/status`, task polling | **no** | data/diagnostic activity is not implementation progress |
| main-tree file/merge visibility | **no** | the active attempt owns an isolated worktree; main may remain stale |
| arbitrary stdout/stderr/log bytes or JSON parse success | **no** | pipe liveness only |
| heartbeat, `kill(pid,0)`, `/proc` CPU change, RPC `get_state` response | **no** | process/pipe liveness only |
| probe request/response | **no**, unless it reveals a previously unseen durable receipt/head | probing must not self-heal the silence timer |
| wall-clock runtime, TTFT estimate, token rate, historical p99 | **no** | telemetry only |
| ordinary incoming WG message | **no** | lifecycle design §9; cannot keep alive or wake |

A tool named `bash` does not automatically qualify. The plugin classifies the
specific call. Repeated `git status`, `wg show`, log tailing, or heartbeat loops
remain liveness-only even if they emit tool progress.

### 4.2 Facts that cannot be inferred

The implementation must render these as unknown, not guess:

* whether a provider is slow, queued, or permanently hung before a token;
* whether a response body will complete after headers;
* whether an upstream provider billed an interrupted request;
* whether a non-idempotent external side effect committed when no durable
  completion receipt exists;
* whether an arbitrary daemonized descendant escaped a process group on a host
  without containment;
* whether a clean Pi exit means the task is complete;
* whether an assistant partial stream was saved (it normally is not until
  `message_end`);
* whether main reflects a still-owned worktree; and
* whether free-form message text asks for retry, completion, or cancellation.

Unknown side effects and unknown ownership fail closed.

### 4.3 Layered lifecycle and completion proof

Time and process heuristics never satisfy a lifecycle disposition. A Pi
lifecycle tool is only an **intent** after `LifecycleKernel` durably accepts a
receipt for the exact current tuple:

```text
TerminalReceipt(
  task_id, generation, attempt_id, attempt_fence,
  process_epoch, toolCallId, disposition, lifecycle_event_id
)
```

The kernel validates currency and first-terminal-wins under `graph.lock`.
`wg_done` yields `SuccessIntent`; it does not set `Done`. `wg_fail` and a
correlated `wg_wait` require equivalent current-epoch accepted receipts, then
follow their failure or park dispositions directly; they do not enter the
success candidate/finalization path. Receipts from an older process epoch are
late evidence only. `agent_end`, `agent_settled`, stdout/EOF, any exit code,
silence, files, evaluator opinion, a completion prompt, and elapsed runtime
cannot substitute for an accepted receipt.

The proof that a task is really `Done` is the conjunction:

```text
DoneProofV1 {
  success_intent: TerminalReceipt(current tuple, wg_done, SuccessIntent),
  quiescence: PiQuiescenceReceipt(current tuple, exact process reap,
                                  final session head,
                                  final worktree manifest digest),
  candidate: CandidateCheckpointReceipt(candidate_id,
                                         worktree manifest digest,
                                         success/quiescence receipt digests),
  gates: RequiredValidationEvaluationReceipts(candidate_id, policy_digest),
  finalization: AuthoritativeFinalizationReceipt(
      candidate_id, target_head_before, target_head_after,
      merge_event_id, lifecycle_done_event_id),
}
```

Ownership is layered and single-writer:

1. `LifecycleKernel` accepts/reserves the current terminal intent and disarms
   conflicting watchdog actions.
2. The Pi watchdog quiesces the exact process and emits the current manifest
   receipt; it cannot create a candidate or declare completion.
3. The crash-safe finalizer creates and binds the candidate checkpoint to those
   exact receipts, runs/binds required validation and evaluation evidence, and
   requests the authoritative merge/finalization transaction.
4. The finalizer/lifecycle-kernel boundary records the merge/finalization event
   and only then projects the canonical task `Done` event.

A missing or mismatched member leaves the task non-Done. No timeout, process
state, prompt, or filesystem observation fills a missing member.

## 5. Observation policy, two clocks, waits, and long tools

### 5.1 Static two-clock policy

The policy snapshot contains two independent clocks:

```toml
[pi_watchdog]
enabled = true
meaningful_silence_secs = 300       # soft suspicion/read-only probe only
hard_resume_grace_secs = 60         # starts only after hard threshold
max_hard_resume_grace_secs = 180    # configuration validation cap
max_continuation_epochs = 3
max_continuation_elapsed_secs = 1800
continuation_epoch_lease_secs = 600
term_grace_secs = 10
kill_grace_secs = 5
long_tool_initial_lease_secs = 600
long_tool_renewal_secs = 300
long_tool_hard_cap_secs = 3600

# Static initial defaults; both free and low QoS MUST remain >= 900.
[pi_watchdog.qos.free.hard_resume_after_secs]
provider_request_in_flight = 900    # TTFT/body-stream silence
provider_response_stream = 900      # inter-token/generation silence

[pi_watchdog.qos.low.hard_resume_after_secs]
provider_request_in_flight = 900
provider_response_stream = 900

# Exact route/phase overrides are resolved and frozen on the attempt and must
# respect the applicable QoS safety floor.
[pi_watchdog.route."pi:provider:model"]
hard_resume_grace_secs = 120

[pi_watchdog.route."pi:provider:model".hard_resume_after_secs]
provider_request_in_flight = 1200
provider_response_stream = 1200
```

`meaningful_silence_secs` is a static **soft-suspect threshold**. At exactly
`last_meaningful + 300s`, any armed silent phase (including `Unknown`) becomes
`Suspect` by CAS and receives one read-only probe; accepted wait and valid
long-tool states are disarmed/protected as §5.2 specifies. The probe does not
advance progress and cannot authorize a prompt, TERM/KILL, process fence,
lifecycle disposition, or
completion/failure inference. In particular, reaching 300 seconds plus the old
maximum 180-second grace still does not authorize interruption.

`hard_resume_after_secs` is separately resolved from the frozen route and the
current proven phase. It is a not-before eligibility threshold, never a promise
to interrupt at that instant. Initial free/low-QoS provider/TTFT and
generation/inter-token policies MUST be at least 900 seconds and may be longer
by explicit route policy. `Unknown`, advancing progress, an accepted wait, and
a valid long-tool lease have **no automatic hard threshold**. Missing or invalid
phase/route policy fails closed to no automatic resume.

All production threshold values are static and configuration-validated; the
credential-free fixture may inject shorter ordered values through its explicit
test-only policy object, never through production config. Record the frozen
route/model/QoS, phase, input bytes/tokens, TTFT, provider duration,
inter-token gaps, tool/worktree progress, suspect/probe outcomes, false
suspects, resume latency/success, and possible duplicate inference cost.
Telemetry is observational: it cannot rewrite or decrease policy. Any future
p99 adaptation requires a separate ratified design and a non-bypassable safety
floor.

```text
new meaningful progress ───────────────────────────────► Active (clocks restart)
          │
          ├─ soft 300s ─► Suspect + read-only probe ───► keep process intact
          │                                  │
          │                    proven phase hard threshold
          │                                  ▼
          │                     HardResumeEligible + hard grace
          │                                  │
          │                    all safety proofs/CAS pass
          │                                  ▼
          │                     Fencing ─► Resuming (same session)
          │
          ├─ agent_settled / exact exit ─► NeedsFinalization (prompt promptly)
          ├─ accepted terminal receipt ──► disarm / lifecycle-finalizer path
          └─ unknown or unsafe proof ─────► observe/hold; never generic kill
```

### 5.2 Observation/action decision table

Time can authorize observation or make a safe continuation eligible. It never
proves success, failure, abandonment, quiescence, or absence of a side effect.
The supervisor applies this table before any timer logic:

| Observation | Required action |
|---|---|
| accepted current-epoch `wg_done` | Disarm the watchdog; quiesce the exact process; hand terminal and current-manifest receipts to candidate validation/evaluation/finalization. Do not project `Done` before the complete §4.3 proof. |
| accepted current-epoch `wg_fail` or correlated `wg_wait` | Disarm and follow the matching lifecycle failure or park disposition. Do not infer completion or use the success candidate path. |
| `agent_settled` with no terminal receipt | Enter nonterminal `NeedsFinalization`; promptly quiesce/reconcile and issue one bounded same-session completion action. Do not wait five minutes because Pi declared no autonomous continuation. |
| exact process exit/EOF with no terminal receipt | Enter `NeedsFinalization`; prove exact exit/reap, classify tool/effect ambiguity, and, when safe, promptly reopen the exact same session/attempt/route/worktree for the bounded completion action. Exit code chooses neither success nor failure. |
| provider/model request in flight with no meaningful output | At 300 seconds append `Suspect` and run a read-only probe only. Keep the current process/provider operation intact until the separately configured phase/route hard threshold and hard-resume grace pass. |
| advancing token/thinking/tool-call/session/worktree progress | Remain `Active`, advance the monotonic progress sequence, and cancel stale suspicion/eligibility. Total runtime is irrelevant; a progressing 20-minute task remains untouched. |
| valid declared long-tool lease | Remain `LongTool` and protect it through lease/renewal/hard-cap rules. At expiry use effect-specific reconcile/receipt/operator-hold behavior, not a provider timer. |
| accepted explicit user wait | Disarm the watchdog and park through the lifecycle kernel. Only its correlated wake follows normal lifecycle behavior; prose/UI waiting is not authority. |
| alive but phase/ownership/effect evidence is unknown | Suspect/probe conservatively, then hold for operator or continued observation. Never guess provider stall and never auto-kill under generic silence. |
| silent/no-terminal without safe same-session, route, process, or side-effect proof | Enter `StalledOperatorRequired`; no fresh session, fallback route, duplicate effect, inferred failure, or automatic new attempt. |

`NeedsFinalization` says only “Pi stopped autonomously without completing the WG
protocol.” It neither claims that the work is complete nor adds a canonical
lifecycle status.

### 5.3 Soft probe, hard eligibility, and grace

`PiSuspectObserved` captures the current progress sequence/digest, proven phase,
frozen route, session head, and exact process identity. Its idempotent
`PiProbeRequested` may query RPC state/entries, inspect process identity/pipe,
and request a plugin phase snapshot. It sends no prompt, invokes no tool, does
not reset meaningful progress, and grants no authority.

For a proven provider/generation phase, WG keeps the existing process and
provider operation intact until `last_meaningful + hard_resume_after_secs`.
Crossing that separate threshold records `PiHardResumeEligibilityObserved` and
only then starts `hard_resume_grace_secs`. New meaningful evidence at any point
CASes back to `Active`, advances the progress sequence, and invalidates both
suspect and eligibility actions.

After the hard grace, automatic fencing is permitted only if one CAS proves all
of the following unchanged/current:

* progress sequence/digest, session head, process epoch, attempt fence, and
  terminal reservation (`None`);
* a fresh read-only probe found no new durable or native progress evidence;
* exact PID/start/boot/nonce identity and descendant containment;
* a proven phase with a frozen hard-resume policy (never `Unknown`);
* safe tool/provider effect classification, including receipt/postcondition
  reconciliation and possible duplicate-inference accounting;
* available, already-reservable finite continuation budget; and
* exact same-session, route, attempt, and worktree proof.

A failed, stale, or unknown check holds/observes instead of signaling. Settled
and exited states take the prompt/finalization path promptly because of explicit
quiescence evidence, not either clock; an exited/replaced process still requires
its exact fence/reap receipt before replacement.

### 5.4 Explicit user wait

`WaitingUser` is entered only after the lifecycle kernel accepts a correlated
park/wait intent. An extension `extension_ui_request`, a question tool waiting
on RPC UI, or prose such as “waiting for the user” has no such authority. If a
worker needs input, it must call the WG wait tool with a wait ID, correlation,
message barrier, and allowed sender policy.

Once accepted, the terminal/park reservation disarms the watchdog, quiesces the
process, and finalizes `AttemptParked`. The later correlated wake follows the
normal new-attempt path. Unrelated/post-terminal messages remain inert.

### 5.5 Long-tool contract

A long tool is protected only if its pre-execution receipt declares:

```rust
LongToolContract {
    tool_call_id: String,
    effect: ReadOnly | Idempotent { precondition, postcondition }
          | ReceiptBacked { receipt_namespace }
          | NonIdempotent,
    progress_schema: ProgressSchema,
    lease_expires_at: Timestamp,
    hard_expires_at: Timestamp,
    renewable: bool,
}
```

The call ID must be observable. Initial lease defaults to 600 seconds, progress
may renew it by 300 seconds, and no automatic renewal passes the 3,600-second
hard cap. While a valid lease exists, the process remains `LongTool` and has no
automatic hard-resume threshold; the 300-second soft observer may record
diagnostics but cannot interrupt it. A renewal requires monotonic/new work evidence, not a
heartbeat.

At expiry the supervisor probes and applies the effect-specific contract rather
than a provider timer. A read-only/idempotent call becomes continuation-eligible
only after its own reconciliation policy and hard cap. A receipt-backed call can
proceed only if the stable call ID finds a durable completion receipt. An
ambiguous non-idempotent call enters `StalledOperatorRequired`; it is never
blindly replayed or killed merely because a budget edge arrived.

## 6. Same-session and exact-route proof

### 6.1 Proof tuple

Before every continuation launch, the following tuple is complete and persisted:

```rust
PiSessionProofV1 {
    source: {
        task_id, generation, attempt_id, attempt_fence,
        worktree_lease_epoch,
        worktree_canonical_path, worktree_device_identity,
    },
    continuation_epoch,
    process_epoch,
    session: {
        session_id,
        session_dir_canonical,
        session_file_canonical,
        session_file_device_identity,
        header_version,
        header_cwd,
        header_parent_session,
        header_digest,
        active_leaf_id,
        active_leaf_entry_digest,
        append_prefix_len,
        append_prefix_digest,
    },
    route: {
        handler: "pi",
        provider,
        model,
        reasoning,
        api_transport,
        endpoint_redacted,
        endpoint_hmac,
        credential_source_id,       // no secret bytes
        pi_version,
        pi_binary_digest,
        plugin_compat_version,
        plugin_bundle_digest,
    },
    policy_digest,
}
```

The endpoint HMAC covers the unredacted canonical endpoint while the displayed
value removes credentials/query secrets. `credential_source_id` names the Pi
credential/OAuth record or environment variable, not its value. OAuth refresh
within the same source is allowed; switching credential source, endpoint,
provider, or model is not.

`append_prefix_digest` protects against truncation/replacement. Legitimate
continuation appends produce a new head and a new proof version chained to the
prior digest; they do not rewrite the old tuple.

### 6.2 Establishing and re-attesting

Pi cannot expose its resolved endpoint before a process has loaded its model
registry. The strongest available fail-closed protocol is therefore two-phase:

1. **Initial attempt bootstrap:** the launch plan preassigns exact session ID,
   directory, worktree, handler/provider/model/reasoning, Pi/plugin digests, and
   expected endpoint source. The Pi process starts execution-gated. Its
   `session_start` hook and RPC `get_state/get_entries` attest the session file,
   leaf, model `baseUrl`, provider auth endpoint, and reasoning. No prompt,
   provider call, or tool is allowed until the kernel binds the complete tuple
   and returns `InitialExecutionPermit`.
2. **Continuation:** the now-complete proof exists before the launch intent.
   After old-process quiescence and before a replacement prompt, the new process
   re-attests through the plugin plus RPC. `before_agent_start` checks the
   current epoch permit as a second gate. Tools and provider requests remain
   blocked until `ContinuationExecutionPermit` matches the proof digest and
   epoch nonce.

The worker path should use Pi RPC mode for this control handshake. RPC emits the
same native `AgentSessionEvent`s as JSON mode while adding `get_state`,
`get_entries`, request correlation, and `agent_settled`. This is a Pi-worker
transport change, not a chat redesign.

Before continuation, the supervisor requires:

* exactly one existing, regular session file in the frozen directory with the
  frozen ID;
* unchanged header ID/cwd/version and prefix digest;
* leaf equal to the post-quiescence leaf recorded in the launch intent;
* exact worktree path/lease;
* exact route/endpoint/reasoning/plugin/Pi identity; and
* no unacknowledged branch movement after the fence snapshot.

Missing file, duplicate ID, Pi's “creating a new session” behavior, changed cwd,
branch ambiguity, route mismatch, unavailable attestation, or configuration
change produces an operator hold. There is no fresh-session fallback.

### 6.3 Configuration changes

The attempt carries the serialized route/provider descriptor needed to relaunch
Pi; it does not re-resolve the active WG profile. Pi's ambient config is
compared with, not substituted for, the snapshot. A later profile/model/endpoint
change affects future attempts only.

Rate limits, missing/rotated credentials, endpoint failures, and provider
failures remain attributed to that exact route. They may back off within Pi's
own frozen retry policy, but never select another route. Continuation failures
are excluded from spawn and provider circuit-breaker counters.

## 7. Side-effect safety and dangling session repair

### 7.1 Tool classifier

Every tool registration must expose an explicit effect class. The initial table
is conservative:

| Tool/call | Automatic continuation after missing result? |
|---|---|
| `read`, `grep`, `find`, `ls`, safe source inspection | yes; read-only |
| `wg_show`, `wg_ready`, `wg_msg_read`, status/log queries | replay-safe but liveness-only |
| `write` | only with call ID plus intended full-content digest and observed postcondition |
| `edit` | only with call ID plus before/after file digests; inspect postcondition before deciding |
| tests/builds | only when sandbox/worktree-local and no declared external side effect |
| arbitrary `bash` | no, unless a parser/declared contract proves read-only/idempotent |
| `wg_done`, `wg_fail`, `wg_wait` | receipt-backed lifecycle request with toolCallId as idempotency key |
| `wg_log`, message send, add/publish | only after those commands accept stable idempotency keys and expose receipts |
| deploy, payment, push, remote mutation, unknown extension tool | no; operator required |

Prompt wording is not an exactly-once mechanism.

### 7.2 Crash windows

| Crash point | Durable fact | Automatic action |
|---|---|---|
| before assistant `message_end` | no durable tool call; partial token may have been billed | same-session completion action is effect-safe once its state/phase eligibility path authorizes it; record possible duplicate inference cost |
| after assistant/tool call persisted, before WG call-intent fsync | tool cannot have been permitted to run | append interrupted tool result and continue |
| after call-intent, before tool side effect | intent only | read-only/idempotent may reconcile; non-idempotent needs proof it did not run |
| during read-only/idempotent tool | stable call ID, no unsafe external effect | fence; inspect; append recovered/interrupted result; continue |
| after side effect, before completion receipt | effect may have committed | receipt/postcondition lookup; otherwise operator hold |
| after completion receipt, before Pi toolResult entry | exact result digest/receipt exists | append a recovered toolResult for the same toolCallId, then continue |
| after Pi toolResult, before next provider call | complete session entry | continue normally |
| terminal tool starts while watchdog races | kernel CAS decides | terminal reservation or process-epoch fence wins (§10) |

### 7.3 Repairing a dangling tool call

A persisted assistant tool call without a matching tool result can make provider
history invalid. The plugin/SDK worker therefore has a gated recovery operation
that appends exactly one synthetic `ToolResultMessage` with the original
`toolCallId` before the continuation prompt:

* completion receipt found: restore the receipt-backed result;
* proven read-only/idempotent interruption: append an error result saying the
  call was interrupted and its postcondition was inspected;
* operator reconciliation: append the operator's signed disposition/receipt;
* ambiguous side effect: append nothing and remain held.

The recovery entry includes the WG evidence digest and process epoch. It never
fabricates success from a prompt or filesystem guess.

## 8. Persisted control records and projection

### 8.1 Low-volume authoritative events

The lifecycle ledger gains typed records/requests (names may be adjusted but
semantics may not):

```text
PiContinuationAuthorized
PiNeedsFinalizationObserved
PiSuspectObserved
PiProbeRequested / PiProbeObserved
PiHardResumeEligibilityObserved
PiCompletionActionRequested / PiCompletionActionObserved
PiContinuationEpochReserved
PiProcessSubleaseRevoked
PiSignalRequested / PiSignalReceipt
PiProcessReaped
PiContinuationLaunchIntent
PiProcessEpochStarted
PiSessionAttested / PiExecutionPermitted
PiProcessEpochExited / PiProcessExitDeferred
PiContinuationBudgetExhausted
PiOperatorHoldRaised / PiOperatorHoldResolved
PiContinuationAuthorizationConsumed|Revoked
```

Every record contains event/idempotency ID, source tuple, expected lifecycle
revision, process/continuation epoch, evidence refs, actor, reason code, and
timestamps. Decisions and related worktree/process-sublease projection changes
commit under `graph.lock` through the kernel.

### 8.2 Attempt evidence journal

High-volume native Pi evidence lives at:

```text
.wg/attempts/<attempt-id>/pi/
  route-snapshot.json
  session-proof.json
  progress.jsonl                 # checksummed/hash-linked, fsync append
  state.json                     # atomic derived projection
  receipts/<tool-call-id>.json
  epochs/<process-epoch>/
    launch.json
    raw_stream.jsonl
    stderr.log
    canonical-stream.jsonl
    exit.json
```

The authoritative lifecycle event references journal record digests. The
journal cannot change task status. Existing `.wg/agents/<agent-id>` paths become
compatibility views/symlinks to the current process epoch; they are not the
session owner.

`PiWatchdogState` projects at least:

```rust
{
  task, generation, attempt, fence, worktree_lease_epoch,
  session_proof_digest, route_snapshot_digest,
  process_epoch, continuation_epoch,
  pid, pgid, pid_start_ticks, boot_id, process_nonce,
  phase,
  progress_seq, last_meaningful_at, last_meaningful_kind,
  soft_suspect_deadline,
  hard_resume_phase, hard_resume_after_secs, hard_resume_not_before,
  hard_resume_grace_deadline,
  provider_call, tool_state, wait_state,
  completion_action_id, prompt_version, prompt_digest, prompt_session_marker,
  epochs_used, elapsed_reserved_secs, elapsed_observed_secs,
  reason_code, pending_action_id, next_action,
}
```

### 8.3 Event identity and accounting

Native stream records are deduplicated by
`(attempt_id, process_epoch, raw_byte_offset, record_digest)`. Plugin receipts
use `(attempt_id, toolCallId, receipt_phase)`. Lifecycle requests use stable
action IDs.

Token/cost totals sum `turn_end.message.usage` once per process-epoch raw record,
retaining Pi's field mapping. The current bridge's correct “turn_end only”
dedup rule remains. A final attempt aggregate spans all epochs; it does not
rewrite a finished epoch stream. Interrupted calls with no Pi usage remain
`possible_unattributed_provider_cost`, not zero-cost claims.

Continuation count, duration, provider errors, probes, fences, and costs are
separate metrics. They do not increment task dispatch/retry, source generation,
admission, spawn breaker, provider breaker, evaluation job, or rescue counters.

### 8.4 Crash-safe finalization handoff

The watchdog is the sole owner of Pi stall classification, process-epoch
continuation, and PID/process-group fencing. It exports three typed receipts for
the later crash-safe finalizer:

```rust
PiContinuationReceipt {
    source_tuple,
    from_process_epoch,
    to_process_epoch,
    reason_code,
    session_proof_before,
    session_proof_after,
    worktree_lease_epoch,
    continuation_input_digest,
}

PiQuiescenceReceipt {
    source_tuple,
    process_epoch,
    pid_start_boot_nonce,
    wait_status,
    nonce_pipe_eof,
    process_group_empty,
    reaped_at,
    final_session_head,
    final_worktree_manifest_digest,
}

PiTerminalIntentReceipt {
    source_tuple,
    process_epoch,
    lifecycle_event_id,
    disposition: SuccessIntent | Failure | Park | Cancel | Abort,
    tool_call_id,
    idempotency_key,
}
```

Each receipt is content-addressed and referenced from the lifecycle ledger. A
quiescence receipt is valid only for the exact current source/process tuple and
manifest; a later write or stale epoch invalidates consumption.

The crash-safe finalizer may consume these receipts after kernel validation. It
may not infer a stall, signal/reap/resume Pi, or manufacture quiescence. In the
other direction, this watchdog does not create rescue/candidate commits, bind
evaluator evidence, merge, or expose main-tree bytes. After an accepted exact
current-epoch `SuccessIntent` and matching `PiQuiescenceReceipt`, the finalizer
alone checkpoints the exact worktree manifest, binds the candidate to both
receipts, gathers required validation/evaluation evidence, and performs the
authoritative merge/finalization transaction. Only that transaction supplies
the final member of `DoneProofV1` and authorizes the kernel's canonical `Done`
projection. Accepted `Failure`, `Park`, `Cancel`, and `Abort` receipts follow
their own lifecycle dispositions without candidate creation. The completion
prompt's request to use `wg_done` means request this existing path; it does not
make the Pi process, prompt, or watchdog checkpoint/merge authority.

## 9. Fence, reap, and continuation algorithm

### 9.1 Process identity

Each epoch starts in its own process group and, where available, a dedicated
cgroup v2/systemd scope (Windows: Job Object). Persist:

```text
pid + pgid + kernel start identity + host boot ID + random process nonce
```

Linux uses exact `/proc/<pid>/stat` start ticks, not a seconds timestamp with
slack. The child proves the nonce over a supervisor-owned pipe/socket before it
can receive an execution permit. PID alone is never sufficient.

If identity cannot be read or mismatches, do not signal the PID. Raise
`pid_identity_ambiguous`. If descendant containment cannot prove emptiness after
termination, do not launch a replacement that shares the worktree.

### 9.2 Ordered protocol

1. **Apply terminal receipts first.** A kernel-accepted current-epoch
   `wg_done`/`wg_fail`/correlated `wg_wait` disarms the watchdog and follows
   §4.3/§5.2. No observation below overrides first-terminal-wins.
2. **Classify settled/exit promptly.** `agent_settled` or exact exit/EOF without
   a terminal receipt enters `NeedsFinalization`. Quiesce/reap and reconcile
   effects immediately; if proof is safe, reserve a replacement epoch and send
   the one same-session completion action without waiting for either clock.
3. **Observe soft silence.** At 300 seconds CAS an armed silent phase (including
   `Unknown`) to `Suspect`; fsync and perform only the read-only probe action.
   Unknown then holds/observes with no hard deadline; accepted wait and valid
   long-tool states follow §5.2 instead.
4. **Keep observing until phase hard eligibility.** Do not signal or prompt at
   the soft threshold or old grace. New meaningful evidence advances the CAS
   sequence, restores `Active`, and cancels stale actions. At the frozen
   phase/route `hard_resume_after_secs`, append eligibility and start the
   separate hard-resume grace.
5. **Re-probe and guard.** After hard grace, require every §5.3 CAS, process,
   containment, terminal, effect, budget, session/route/worktree proof. A
   failed/unknown check holds or keeps observing; it never kills.
6. **Reserve next epoch.** Under the lifecycle lock, CAS the still-current
   source tuple/progress/session head; charge one epoch and elapsed allocation;
   append launch intent; increment the process fence/epoch; revoke the old
   process sublease. The attempt/worktree holder does not change.
7. **Terminate exact owner when still alive.** Verify PID/start/boot/nonce, send
   TERM to the exact contained process group once, wait 10 seconds, then KILL
   once if still exact, wait 5 seconds. For an already exited process, preserve
   the same epoch-fence rules and consume its exact reap proof; never signal a
   replacement/reused PID.
8. **Prove quiescence.** Obtain `waitpid`/platform reap receipt, nonce-pipe EOF,
   exact PID identity absent, and group/cgroup/job empty. Recompute Pi session
   head/prefix and worktree manifest. Persist `PiProcessReaped`.
9. **Repair safe dangling call.** Use §7 receipts/postconditions. Ambiguity holds.
10. **Launch once.** Outbox consumes the already-persisted launch intent. The
    replacement starts spawn-gated with the frozen path/route/session and a new
    process nonce. No replacement PID is created before step 8.
11. **Re-attest.** Plugin/RPC attests §6. The kernel compares it and sends the
    epoch execution permit. Mismatch kills the gated child after exact identity
    verification and holds.
12. **Send the action once.** Inspect the durable session action marker, then
    append the versioned prompt only if absent. The process becomes `Active`
    only after the exact prompt/session marker is observed.
13. **Finish by explicit protocol.** Only a current-epoch `wg_done`, `wg_fail`,
    or correlated `wg_wait` accepted by the kernel supplies a disposition.
    Success then still requires the remaining §4.3 finalization proof.

### 9.3 Same-session completion action

There is one short versioned stock prompt for both `NeedsFinalization` and a
safe hard-resume continuation. It is Pi session input, never a WG message:

```text
[WG_PI_CONTINUATION_V2]
WG observed `<OBSERVATION_CODE>` for this process epoch; no accepted terminal
receipt exists. Inspect the durable SAME Pi session, leased worktree, task
contract, candidate state, relevant tests, and supplied receipt summaries.
Do not repeat a side effect; reconcile it from receipts/postconditions first.
Then produce exactly one explicit outcome: `wg_done`, `wg_fail`, or the
correlated `wg_wait` required by the task. This prompt is guidance, not proof.
```

`<OBSERVATION_CODE>` is a bounded, kernel-derived fact such as
`agent_settled_no_autonomous_continuation`, `exact_process_exit_eof`, or
`no_meaningful_progress_since_sequence`; it never embeds provider/model prose.
The sentence is populated only when its fact is true and does not blame the
model or assert that the task is incomplete or complete.
The prompt includes only bounded identifiers, safe receipt summaries, and the
reason code. It cannot prove a side effect, quiescence, or lifecycle outcome.

Append/send is crash-idempotent by a durable action ID derived from
`(attempt_id, process_epoch, continuation_epoch, prompt_version,
prompt_digest)`. The session stores the same action marker. On replay, the
outbox first inspects the exact durable session marker and never appends a
second copy for that action; an uncertain marker holds instead of resending.
Prompt acceptance is recorded separately from any later provider/tool effect.

The action preserves the exact session and active branch, attempt/fence,
worktree lease, frozen Pi route, and process fencing. It never creates a fresh
session, attempt, route fallback, worktree owner, or concurrent process.

In particular, **main-tree visibility is never progress or completion proof**.
A process can still be thinking or writing a superior file in its isolated
worktree while main is unchanged. The supervisor proves quiescence before
replacement, preserves late writes in the same leased worktree, recomputes its
manifest, and makes the same-session action inspect those bytes.

## 10. First-terminal-wins

The kernel orders terminal/park reservations and process-epoch fencing with one
CAS under `graph.lock`.

### 10.1 CAS key

```text
(task, generation, attempt, attempt_fence,
 current_process_epoch, terminal_reservation = None)
```

Every Pi lifecycle tool carries that tuple plus `toolCallId` as its idempotency
key.

### 10.2 Ordering rules

* If success intent, failure, park, cancel, or operator abort is accepted before
  `PiContinuationEpochReserved`, it reserves the attempt disposition, cancels
  probe/fence/launch outbox actions, and prevents every replacement launch.
  Success intent follows the full §4.3 proof: the watchdog may help quiesce,
  but only the finalizer can bind the candidate, validation/evaluation, merge,
  and authoritative `Done` event.
* If `PiContinuationEpochReserved` commits first, the old process epoch loses
  transition authority immediately. Old `wg_done`, `wg_fail`, wait, exit, and
  tool receipts are late evidence only. Only the new current epoch may request
  disposition.
* A cancel/abort arriving after epoch reservation targets the new current
  process sublease, cancels an unconsumed launch intent or fences the launched
  child, then lets the kernel finalize the requested lifecycle edge.
* Identical terminal requests return the original event. Contradictory/late
  requests return stable `stale_process_epoch` or
  `attempt_already_terminal` and append deduplicated evidence only.
* Duplicate ticks, exits, signals, receipts, and daemon replay use the same
  idempotency/action IDs. Exactly one lifecycle disposition and at most one
  current process owner exist.

## 11. Finite budgets, manual control, and cost

### 11.1 Defaults and charging

Automatic policy grants:

* **3** replacement process epochs after epoch 0; and
* **1,800 seconds** of cumulative reserved continuation runtime.

Each `PiContinuationEpochReserved` atomically consumes one epoch and reserves
up to `continuation_epoch_lease_secs = 600` from the remaining elapsed budget.
The reservation is charged before launch, once even if launch later fails, and
is never refunded. It applies only to replacement recovery exposure: epoch 0
and ordinary task runtime are not charged. At the end of an allocation the
adapter stops admitting a new provider/tool phase and reconciles the current
phase. It may fence a proven read-only/idempotent phase only through the same
§5.3 safety gates; an unsafe or unknown in-flight effect holds rather than being
blindly killed. Long-tool leases and renewals are clamped and effect-aware at a
recovery budget edge. Actual observed runtime is recorded separately and cannot
replenish the reservation.

Three tries cover a transient process crash plus one repeated infrastructure
failure without creating an infinite recovery loop. The 1,800-second reserve
bounds possible duplicate inference/tool exposure. These are recovery-exposure
budgets, **not normal task-runtime deadlines**: a progressing 20-minute epoch-0
task is untouched, and crossing a budget does not infer failure.

At an epoch-lease or total-budget boundary, no new automatic recovery work
starts. A currently executing ambiguous tool follows its safety contract rather
than being blindly killed. Once safely quiescent/fenced, the attempt enters one
deduplicated operator hold. Restart/ticks cannot reset either counter. Further
action requires an audited finite manual grant or an explicit lifecycle
abort/fail/cancel disposition; exhaustion alone supplies none.

### 11.2 Manual commands

Proposed stable surface:

```text
wg pi-watchdog status <TASK> [--json]
wg pi-watchdog resume <TASK> --reason <text>
    [--grant-epochs 1] [--grant-elapsed 10m]
    [--ack-call <toolCallId> --disposition not-run|completed|reconciled
     --receipt <path-or-id>]
wg pi-watchdog abort <TASK> --reason <text>
```

`resume` is valid only for the same attempt/fence/worktree/session/route. It
appends an audited manual authorization extension, with explicit finite epoch
and elapsed grants (defaults: one epoch and 600 seconds). The grant is charged
before launch and cannot reset/refund consumed automatic budget. An ambiguous
side effect requires an exact call acknowledgment and receipt/disposition; an
operator may instead choose explicit lifecycle abort/fail. Neither budget
exhaustion nor a denied grant infers failure.

`abort` is an adapter to the lifecycle kernel's cancel/abandon/fail policy. It
first-terminal-wins and fences the current process; the watchdog does not assign
status. A separate existing `wg fail` operator request may be used when the
intended disposition is failure rather than cancellation/abandonment.

## 12. Crash convergence

| Durable boundary | Replay behavior |
|---|---|
| before suspect append | no action; recompute from durable progress/time |
| suspect appended, probe absent | consume same probe action ID |
| probe sent, receipt absent | repeat read-only probe; never reset silence |
| soft suspect/probe persisted before hard threshold | continue observing current process; no fence/prompt action exists |
| hard threshold reached, grace pending | consume same hard-eligibility/grace action; no fence before grace |
| progress arrives during suspicion/hard grace | fence CAS fails on progress sequence; restore `Active`; cancel actions |
| epoch reserved, old sublease not revoked physically | continue exact revoke/signal action; no launch |
| TERM sent, receipt absent | verify exact identity; repeat/wait; never signal reused PID |
| KILL sent, reap receipt absent | wait/verify containment; ambiguity holds |
| process reaped, receipt not appended | reconstruct from wait/nonce/group evidence or hold; no launch until receipt |
| launch intent appended, process not spawned | spawn once by action ID |
| process spawned, PID receipt absent | gated nonce handshake permits adoption; otherwise kill exact child |
| child started, attestation absent | no provider/tools; re-request attestation or kill/hold |
| attestation appended, permit absent | send same permit once |
| completion action requested, append uncertain | inspect exact version/action session marker; never send a second prompt for the action; ambiguity holds |
| tool intent/result receipt appended, Pi entry absent | repair same call ID per §7 |
| terminal accepted, outbox pending | terminal wins; cancel continuation and finish quiescence |
| ledger append before projection | lifecycle replay projects it |

A launch action is not complete merely because `spawn()` returned. The child is
execution-gated until PID identity, nonce, and attestation receipts are durable.

## 13. Operator diagnostics and metrics

### 13.1 Status

Human and JSON status must show:

* task, generation, attempt, attempt fence, worktree lease epoch/path;
* Pi session ID/dir/file/header digest/leaf/prefix digest and proof status;
* frozen handler/provider/model/reasoning/API/endpoint and Pi/plugin versions;
* continuation/process epoch, PID/PGID/start ticks/boot ID/nonce;
* phase and pending outbox action;
* last meaningful progress time, sequence, and kind;
* current silence and 300-second soft-suspect threshold/probe outcome;
* proven phase, frozen `hard_resume_after_secs` (or `none`), hard eligibility,
  and hard-resume grace/deadline;
* provider call/TTFT phase and token delta telemetry;
* tool call/effect/receipt/progress/lease state;
* correlated wait state;
* automatic/manual epoch and elapsed budget used/remaining;
* continuation reason, exact-route failure, possible unattributed cost;
* operator hold/issue ID and a copy-pasteable next safe command.

Example:

```text
Pi watchdog: Suspect (soft observation; process intact)
  source: task=build-x gen=2 attempt=a7 fence=19 worktree-lease=4
  session: id=... leaf=8fa21c9e proof=verified route=pi:openai-codex:gpt-5.6-sol@xhigh qos=low
  process: epoch=1 pid=4312 start=922001 nonce=... exact=yes
  progress: token_delta at 12:00:00Z; silence=314s / soft-suspect=300s
  probe: response=liveness-only; progress-reset=no
  hard-resume: phase=provider_request threshold=900s eligible=no grace=not-started
  tool: none; wait: none
  budget: epochs=1/3 elapsed-reserved=600/1800s (recovery only)
  next: continued observation; no signal before hard policy + grace + proof;
        `wg pi-watchdog status build-x --json`
```

### 13.2 Metrics

At minimum:

```text
wg_pi_watchdog_suspects_total{route,phase}
wg_pi_watchdog_false_suspects_total{route,progress_kind}
wg_pi_watchdog_fences_total{reason}
wg_pi_watchdog_continuations_total{route,outcome}
wg_pi_watchdog_operator_holds{reason}
wg_pi_watchdog_resume_seconds
wg_pi_watchdog_ttft_seconds / provider_call_seconds / inter_token_seconds
wg_pi_watchdog_tool_seconds{tool,effect}
wg_pi_watchdog_continuation_cost_usd{route}
wg_pi_watchdog_possible_unattributed_cost_total{route}
wg_pi_watchdog_stale_epoch_reports_total{kind}
wg_pi_watchdog_pid_identity_ambiguity_total
```

The metrics are observational. They tune neither the 300-second soft threshold
nor any phase/route hard threshold or grace. Future p99 adaptation is deferred
to a separate ratified design with an explicit safety floor.

### 13.3 Stable reason codes

```text
meaningful_silence_soft_suspect
probe_no_progress
hard_resume_phase_eligible
needs_finalization_settled
needs_finalization_exit
process_exit_zero_no_terminal
process_exit_nonzero_no_terminal
pipe_eof_no_terminal
tool_lease_expired
ambiguous_tool_side_effect
session_missing
session_duplicate_id
session_header_mismatch
session_head_mismatch
session_prefix_mismatch
route_mismatch
endpoint_mismatch
attestation_missing
pid_identity_ambiguous
process_group_not_quiescent
reap_unproven
continuation_epoch_budget_exhausted
continuation_elapsed_budget_exhausted
terminal_won
wait_parked
operator_resume
operator_abort
```

Reason text is bounded/redacted; attacker/provider output is an evidence digest,
not interpolated into category fields.

## 14. File-level implementation seams

The downstream implementation should touch these seams after the authoritative
lifecycle and admission-deferral work lands:

| Seam | Required change |
|---|---|
| lifecycle kernel/event/projector modules introduced by `implement-authoritative-lifecycle` | add §2/§8 typed requests, CAS, pre-terminal classification, hold, and first-terminal ordering |
| `src/commands/spawn/execution.rs` | replace Pi's generic one-shot wrapper terminal mapping with the dedicated process-epoch supervisor/launch outbox; retain non-Pi behavior |
| `src/service/executor.rs` | stop using anonymous Pi worker sessions; route Pi task workers through exact-session RPC worker adapter |
| new `src/pi_watchdog/` | policy, evidence projection, session proof, tool classifier, process fencing, reconciliation, clock abstraction, metrics |
| new/internal `src/commands/pi_worker.rs` | RPC worker transport, attestation/read-only probe/control, `agent_settled`, versioned idempotent same-session completion action |
| `src/stream_event.rs`, `src/commands/pi_stream_bridge.rs` | incremental epoch-aware native event ingestion; preserve call IDs/progress/provider phases/settled; aggregate costs across epochs idempotently |
| `worksgood-pi/src/index.ts` plus new watchdog bridge | session/route attestation, provider phase receipts, pre-provider epoch gate, tool intent/progress/end receipts |
| `worksgood-pi/src/tools.ts`, `wg-backend.ts` | retain toolCallId, declare effect contracts, pass lifecycle idempotency/process tuple to WG terminal/wait tools |
| `worksgood-pi/embedded/**` | re-embed the compatible plugin after source changes |
| `src/service/mod.rs` / coordinator tick | call Pi reconciliation phase after lifecycle replay/process evidence and before readiness; do not duplicate the reaper |
| generic dead-agent/sweep paths | skip current authorized/held Pi attempts; retain terminal zombie cleanup |
| `src/cli.rs`, `src/main.rs`, command modules | `wg pi-watchdog status/resume/abort` and hidden receipt/attestation adapters |
| show/attempt/reconcile/TUI views | §13 diagnostics; no status mutation |
| config/profile validation | §5/§11 static soft/hard settings, >=900s free/low-QoS hard-floor validation, absent hard policy for unknown/wait/long-tool, exact frozen route overrides, finite recovery budgets |
| tests/smoke | §15 Fake-Pi, model/race tests, installed-binary PTY scenario |

The shared daemon-loop edit lands before `impl-supervisor-hard-agent`; that later
task must preserve the exclusion contract. No generic multi-executor watchdog,
chat recovery, evaluation, admission, breaker redesign, or adaptive parallelism
belongs here.

## 15. Deterministic validation design

### 15.1 Fake-Pi protocol

Add a credential-free `tests/fixtures/fake-pi-watchdog` executable driven through
the real Pi worker argv/RPC path. It must:

* parse exact `--mode rpc`, provider/model/thinking, session path/dir, and epoch
  environment;
* maintain a real v3 session JSONL header/tree/leaf with stable IDs;
* implement LF-framed RPC `get_state`, `get_entries`, `prompt`, and a read-only
  probe;
* emit native Pi `agent_start`, `turn_start`, `message_*`,
  `tool_execution_*`, `turn_end`, `agent_end`, and `agent_settled` records with
  native usage shape;
* expose provider-boundary and WG receipt records through the same plugin bridge
  schema as production;
* provide stable tool call IDs and a receipt-keyed side-effect file that rejects
  duplicate execution;
* fork contained descendants for TERM/KILL/reap tests;
* support zero/nonzero/signal exits and a deliberate reused-PID/start-identity
  mismatch fixture;
* pause at named barriers (`tool_intent`, `side_effect`, `tool_receipt`,
  `terminal_request`, `exit`, `attestation`) for daemon kill/restart; and
* use the service's injected `Clock`/test control socket. Production code never
  reads arbitrary `WG_NOW`; virtual time is test-only and explicit.

A scenario file is declarative, for example:

```json
{"steps":[
  {"emit":"before_provider_request"},
  {"advance_virtual_secs":299},
  {"emit":"text_delta","text":"x"},
  {"tool_start":{"id":"call-1","name":"receipt_write","effect":"receipt-backed"}},
  {"side_effect":{"key":"call-1"}},
  {"barrier":"after_side_effect"},
  {"exit":137}
]}
```

### 15.2 Permanent model/fault matrix

1. **Soft versus hard clocks:** with production values frozen at soft 300 and
   low/free-QoS hard 900, assert 299 seconds is `Active`; exactly 300 appends one
   `Suspect` plus read-only probe; 300 plus the superseded 180-second grace does
   **not** fence; 899 seconds still preserves the process. At 900 seconds append
   hard eligibility/start hard grace, and permit the fence only at/after that
   grace with every §5.3 proof. Missing/`Unknown` phase policy never fences.
2. **Progress and long runtime:** token/thinking/tool-call/session/worktree
   progress advances the monotonic sequence, resets both clocks, and invalidates
   stale suspicion. A progressing 20-minute generation remains `Active` and is
   never signaled. Heartbeats, logs, status polls, main-tree changes, ordinary
   messages, and the probe itself do not reset meaningful time.
3. **Provider lifecycle:** cover request-in-flight/slow TTFT, response-body and
   inter-token silence, `agent_end(willRetry=true)`, provider retry, compaction
   retry, and queued follow-up. Before the phase hard policy plus grace the
   current provider operation stays intact; retry/follow-up remains active until
   `agent_settled`.
4. **Needs finalization:** `agent_settled` and clean/abnormal/signal exit/EOF
   without a terminal receipt promptly enter nonterminal `NeedsFinalization`
   and receive the one safe same-session action without waiting 300 seconds.
   Neither exit status nor settlement selects Done/Failed. Unsafe exit proof
   enters `StalledOperatorRequired`.
5. **Explicit terminal and Done proof:** accepted current-epoch done disarms and
   reaches Done only after exact quiescence/manifest, candidate checkpoint,
   required validation/evaluation, and authoritative finalization receipts.
   Accepted fail and correlated wait disarm into their own dispositions. Missing
   receipt or any missing success-proof member never terminalizes.
6. **Real eligible silence after partial work:** after hard policy plus grace,
   one guarded epoch CAS, exact group fence/reap, same proof/route/worktree
   re-attestation, one prompt, receipt-backed side effect exactly once, and an
   explicit accepted terminal receipt. A failed CAS/probe/effect/proof check
   holds without signal.
7. **Correlated wait and unknown:** accepted wait disarms across arbitrary
   virtual time and daemon restart; unrelated messages do nothing; only matching
   correlation follows normal new-attempt behavior. Alive silence with unknown
   phase/ownership/effect probes then holds forever, with no generic kill.
8. **Long tools:** renewable declared long tool survives 300, 900, and its
   valid lease while progressing; expired read-only tool uses effect-specific
   reconciliation; an ambiguous non-idempotent effect enters operator hold with
   zero replay and is not blindly killed at a provider/budget timer.
9. **Restart/idempotency matrix:** restart before/after soft suspect, probe,
   hard eligibility, hard grace, epoch/budget reservation, sublease revoke,
   TERM/KILL/reap, launch, attestation/permit, prompt action/marker, tool receipt,
   and terminal reservation. Assert one prompt append, action, process, charge,
   and disposition.
10. **Duplicate/race matrix:** duplicate ticks/exits/receipts; terminal versus
    epoch CAS in both orders for done/fail/park/cancel/operator abort; stale
    wrapper reports and PID reuse. Assert first-terminal-wins and old-epoch
    reports are late evidence only.
11. **Proof/route/budget/operator:** missing/duplicate/replaced session, branch
    or route mismatch, unavailable exact route, missing attestation, three-epoch
    and 1,800-second exhaustion all hold without fallback/fresh session or
    inferred failure. Manual safe resume/ambiguity receipt/abort/fail grants are
    finite, audited, charged once, and never replenish on restart.
12. **Dangling calls and domain isolation:** crash every §7.2 window; repair only
    with read-only/postcondition/receipt proof. Continuation creates no admission
    request, source attempt/generation/retry, evaluation job, worktree transfer,
    breaker charge, or second owner; cost aggregates once across epochs.

### 15.3 Installed-binary terminal/PTY smoke

Register one grow-only scenario, proposed name
`pi_session_watchdog_human_flow`, owned by `implement-pi-stalled`. It must:

1. `cargo install --path . --locked` before the installed-runtime validation;
2. run the real `wg service` and wrapper inside tmux/PTY against an isolated
   graph/HOME and Fake-Pi on PATH;
3. display/assert production `meaningful_silence=300s` and low/free-QoS
   phase hard-resume policy `>=900s`, then use explicit short soft/hard test
   values that preserve their ordering;
4. visibly show soft `Suspect` plus read-only probe, no fence at the old
   soft-plus-grace point, hold immediately before hard eligibility, and only
   then `HardResumeEligible -> Fencing -> Resuming -> Active` after hard grace
   and proof;
5. show a progressing long run, provider retry/in-flight, accepted wait, valid
   long tool, and unknown alive silence untouched by generic timing;
6. show `agent_settled` and safe zero/nonzero exit enter nonterminal
   `NeedsFinalization` and receive the same-session prompt promptly, without
   becoming Done/Failed;
7. prove explicit current-epoch done/fail/wait receipts and the full success
   finalization tuple; no early Done from prompt/exit/settled;
8. kill/restart the real daemon around prompt marker, continuation, and budget
   boundaries; drive terminal-vs-watchdog races in each ordering;
9. run `wg pi-watchdog resume` and `abort` as human terminal commands; and
10. assert one prompt/action/side-effect, no duplicate owner/PID/session/route or
    replenished budget, and observational-only telemetry.

This is not replaceable by a direct Rust helper or a fake lifecycle call. A
TUI status pane may be exercised, but the minimum gate is the actual installed
CLI/service/wrapper/operator terminal flow.

### 15.4 Optional attended canary

An opt-in, never-CI-required canary may run a low/free-QoS Pi route and record
TTFT, provider duration, token gaps, false suspects, tool durations, resume
outcomes, exact session re-attestation, and Pi-reported cost. It must use a
separate test task/worktree and may not modify defaults. Missing credentials or
provider availability is a loud skip, not a CI failure.

## 16. Implementation ordering and acceptance

1. Land the authoritative lifecycle kernel and ledger.
2. Land admission-deferral semantics so continuation does not consume admission
   or breakers.
3. Implement this Pi worker protocol and its RED matrix.
4. Re-embed the Pi plugin, install the binary, and pass the PTY smoke.
5. Only then allow shared daemon-loop work in `impl-supervisor-hard-agent`.

The implementation is accepted only when:

* the §5.2 table distinguishes terminal receipts, settled, exit, provider
  in-flight, progress, long tool, accepted wait, unknown alive silence, and
  unsafe proof without another task-status writer;
* 300 seconds remains a fixed soft suspect/probe only, separate from static
  phase/route hard eligibility (free/low-QoS >=900), hard grace, budgets, and
  future telemetry; unknown/wait/valid-long-tool has no automatic deadline;
* `NeedsFinalization` prompts promptly but never terminalizes, while `Done`
  requires every exact receipt in §4.3;
* same-session/route proof plus exact process fencing make a concurrent or fresh
  session impossible, and the versioned prompt/action is append-once on restart;
* ambiguous side effects, messages, PID reuse, restarts, cost, and finite
  recovery budgets are fail-closed and permanently tested;
* caps/config/events/files/commands/reasons are implemented as specified; and
* the credential-free real service/PTY flow is permanent in the smoke manifest.

## 17. Rationale and rejected alternatives

### Why same attempt rather than retry?

The semantic work, worktree, route, and Pi conversation did not change. Only the
OS process epoch failed. Creating an attempt/generation would consume unrelated
budgets, transfer ownership, permit stale reports, and contradict the source
history.

### Why RPC for the worker?

JSON mode provides rich native events but no command channel. RPC provides the
same event stream plus exact state/entry queries, request correlation, probe,
and a gate before the continuation prompt. It is the smallest Pi-specific
control surface that can prove the session twice.

### Why not trust `--session-id`?

Its documented/current behavior creates a new session when the exact ID is
missing. That is useful interactively and unsafe for recovery. File/header/head
proof plus post-launch attestation turns it into a fail-closed primitive.

### Why not use heartbeats or main-tree changes?

They prove neither useful work nor ownership of the source bytes. The live trace
that motivated this task had an alive, thinking Pi later write a superior 28KB
file after being treated as failed. Only native progress evidence and the exact
leased worktree are relevant; main is deliberately stale until acceptance.

### Why hold instead of fail on ambiguity/exhaustion?

Failure would claim a semantic attempt outcome from missing process evidence and
would strand recoverable source. Repeating would risk duplicate external side
effects and unbounded cost. A single owned, non-dispatchable operator hold
states exactly what is known without inventing success, failure, or retry. Time
can make a proven safe recovery eligible; it cannot eliminate ambiguity.

### Why is exit or `wg_done` not enough to call the task Done?

Exit/settlement proves only that one process stopped, and `wg_done` proves only
that the kernel accepted the current worker's success intent. Neither proves
that the exact worktree stopped changing, that its candidate was checkpointed,
that required validation/evaluation passed, or that the candidate was
atomically finalized into the authoritative target. The layered §4.3 tuple
assigns each proof to the component that can actually establish it and keeps
`LifecycleKernel`/the finalizer as the only completion boundary.

### Why two static clocks and no adaptation now?

Five minutes is useful for noticing silence but unsafe as a universal provider
interruption deadline. A distinct conservative phase/route threshold preserves
a slow low/free-QoS request while still allowing proven stalls to become
recovery-eligible. A learned threshold introduces feedback and sparse-route
failure modes before trustworthy telemetry exists. The static 300-second soft
observer, >=900-second initial low/free-QoS hard floor, bounded hard grace, and
fail-closed proofs are modelable and testable. Telemetry cannot modify them.
Any later p99 policy needs separate ratification and an explicit safety floor.
