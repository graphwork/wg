# Pi threshold-compaction idle-stall reproducer

Date reproduced: 2026-08-06

Upstream issue: [`earendil-works/pi#6424`](https://github.com/earendil-works/pi/issues/6424)

## Result

**Reproduced, credential-free, against the installed Pi CLI.** The user prompt explicitly requires both an intermediate progress record and a subsequent concrete finish-work provider turn. A two-response agentic run calls a real Pi tool, reports context usage above the configured threshold before that requested finish-work turn executes, successfully writes a real `compaction` session entry whose structured fields say that the required action is unexecuted, and has no steering or follow-up queued. Pi then emits:

```text
agent_end(willRetry=false)
compaction_start(reason=threshold)
compaction_end(reason=threshold, aborted=false, willRetry=false, result=<success>)
agent_settled
<process exits 0>
```

There is no later `agent_start`, `turn_start`, provider call, or assistant message. This is the narrow behavior reported in #6424, not an inference from vague prose such as “continue.”

The fixture does **not** implement the workaround or fix.

## Installed version and authoritative path

Both installed version surfaces agree:

```console
$ pi --version
0.83.0

$ node -p "require('/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/package.json').version"
0.83.0
```

The installed package is:

```text
/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent
```

The relevant installed control flow is in `dist/core/agent-session.js`:

- lines 1499-1504 distinguish overflow (“auto-retry”) from threshold (“NO auto-retry”);
- lines 1583-1585 invoke `_runAutoCompaction("threshold", false)`;
- line 1698 emits successful `compaction_end`;
- lines 1699-1705 continue unconditionally only for `willRetry` (overflow);
- lines 1707-1709 return `agent.hasQueuedMessages()` for a successful non-retry compaction;
- lines 776-781 continue after `agent_end` only if compaction requested it or messages were explicitly queued;
- lines 314-318 emit `agent_settled` once that post-run loop returns false.

Thus the observed empty-queue path is falsifiable directly from events: threshold passes `willRetry=false`; successful compaction leaves both queues empty; `_runAutoCompaction` returns false; Pi settles.

The upstream report originally named Pi 0.80.3 and proposed a setting-gated hidden `followUp` with custom type `auto_compaction_continue`. The installed 0.83.0 still reproduces the underlying no-queue path. This fixture intentionally does not apply that workaround.

## Fixture

All files are isolated under the required unique path:

```text
tests/fixtures/fake-pi-compaction-stall/
├── fixture-extension.ts
├── run-reproducer.mjs
└── run.sh
```

### Design

`fixture-extension.ts` uses Pi's supported extension APIs rather than synthesizing an event log:

1. It registers a credential-free custom provider and a model with a 2,000-token context window.
2. Isolated settings set `reserveTokens=500` and `keepRecentTokens=1`; reported usage of 1,700 tokens therefore crosses the real `contextTokens > contextWindow - reserveTokens` threshold.
3. The threshold prompt explicitly asks Pi to record intermediate progress **and then execute one concrete finish-work provider turn and report completion**. The provider returns a real `fixture_progress` tool call, lets Pi execute and persist the tool result, then returns a high-usage assistant response stating that the requested finish-work action has not executed. This is a multi-response agentic run with demonstrably pending requested work, not a one-message session stub or work invented only by the compaction hook.
4. `session_before_compact` supplies a deterministic successful compaction result. Pi itself emits the compaction events, appends the session entry, rebuilds context, checks its actual queues, emits `agent_settled`, and exits JSON mode.
5. If Pi requests another provider turn after compaction, the provider emits the unambiguous marker `FIXTURE_RECOVERY_TURN_EXECUTED`. Installed Pi never requests it.

The extension-supplied summary is deterministic and makes incompletion machine-checkable:

```text
UNFINISHED_WORK_STATE: true
NEXT_REQUIRED_ACTION: invoke the fake provider for one concrete finish-work turn
NEXT_ACTION_EXECUTED: false
```

The persisted entry also has structured details:

```json
{
  "unfinished": true,
  "nextRequiredAction": "finish-work provider turn",
  "nextActionExecuted": false,
  "reason": "threshold"
}
```

`run-reproducer.mjs` invokes the installed `pi` executable with `--mode json`, an isolated `HOME`, explicit extension loading, disabled discovery, `--offline`, and a literal fixture API key. Its child environment is an allowlist (`PATH`, isolated `HOME`, offline/color/locale settings, optional `TMPDIR`, and the fixture scenario); it does not forward parent API-key, provider-auth, Pi-session, or WG variables. No network listener is started and no credential is read. The manual control uses Pi's documented RPC `compact` command—the headless equivalent of explicit `/compact`—because built-in TUI slash commands are not commands in JSON mode.

## Run it

### Capture and validate the currently observed bug

```console
$ rm -rf /tmp/pi-stall-capture
$ tests/fixtures/fake-pi-compaction-stall/run.sh --capture-only /tmp/pi-stall-capture
...
CONTROL ASSERTIONS: PASS (overflow, manual, failed compaction, agent_end queued follow-up)
OBSERVED BUG: successful threshold compaction was followed by agent_settled with no concrete recovery turn
```

This exits **0** only when all controls pass and the installed stall is observed. Raw Pi output and sessions are retained under the given output directory:

```text
/tmp/pi-stall-capture/<case>/events.jsonl
/tmp/pi-stall-capture/<case>/stderr.txt
/tmp/pi-stall-capture/<case>/sessions/*.jsonl
/tmp/pi-stall-capture/trace.txt
/tmp/pi-stall-capture/session-paths.txt
```

### Run the implementation-facing red assertion

Omit `--capture-only`:

```console
$ rm -rf /tmp/pi-stall-red
$ tests/fixtures/fake-pi-compaction-stall/run.sh /tmp/pi-stall-red
...
CONTROL ASSERTIONS: PASS (overflow, manual, failed compaction, agent_end queued follow-up)
AssertionError [ERR_ASSERTION]: RED: threshold compaction with explicit unfinished work must schedule one concrete post-compaction recovery turn (expected assistant marker FIXTURE_RECOVERY_TURN_EXECUTED after successful compaction_end(willRetry=false))
$ echo $?
1
```

Set `PI_BIN=/path/to/pi` to test another Pi build. The runner otherwise uses the installed `pi` on `PATH`.

## Exact observed event ordering

Indices below are the indices in each raw JSONL record array. Streaming `message_update` records are omitted from this compact view but remain in `events.jsonl`.

### Target: successful threshold compaction, empty queue, idle stall

```text
001 agent_start
002 turn_start
004 message_end role=user
009 message_end role=assistant stopReason=toolUse
010 tool_execution_start
011 tool_execution_end
013 message_end role=toolResult
014 turn_end
015 turn_start
020 message_end role=assistant text="Preparation is complete. The required finish-work action has NOT been executed." stopReason=stop
021 turn_end
022 agent_end willRetry=false
023 compaction_start reason=threshold
024 compaction_end reason=threshold aborted=false willRetry=false result=true
025 agent_settled
```

There is no target `queue_update` with a nonempty queue (in this run there is no target `queue_update` at all), and record 025 is the final event. The JSON-mode child exits successfully immediately afterward.

The corresponding v3 session tail is:

```json
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"Perform the long fixture task: record intermediate progress, then execute one concrete finish-work provider turn and report completion."}]}}
{"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"fixture-progress-call","name":"fixture_progress","arguments":{"step":"prepare-recovery-input"}}],"usage":{"totalTokens":200},"stopReason":"toolUse"}}
{"type":"message","message":{"role":"toolResult","toolCallId":"fixture-progress-call","toolName":"fixture_progress","content":[{"type":"text","text":"RECORDED_INTERMEDIATE_STEP:prepare-recovery-input"}],"details":{"recorded":true,"step":"prepare-recovery-input"},"isError":false}}
{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Preparation is complete. The required finish-work action has NOT been executed."}],"usage":{"totalTokens":1700},"stopReason":"stop"}}
{"type":"compaction","summary":"## Explicit fixture state\nUNFINISHED_WORK_STATE: true\nNEXT_REQUIRED_ACTION: invoke the fake provider for one concrete finish-work turn\nNEXT_ACTION_EXECUTED: false\nCOMPACTION_REASON: threshold","tokensBefore":1700,"details":{"unfinished":true,"nextRequiredAction":"finish-work provider turn","nextActionExecuted":false,"reason":"threshold"},"fromHook":true}
```

IDs, parent IDs, and timestamps are deliberately omitted above because they vary per run. Raw capture preserves them and proves the compaction entry is parented after the final assistant message.

### Expected fixed ordering

The implementation task should make the target include one complete concrete provider turn after successful threshold compaction and before the **first** `agent_settled`. The fixture requires the following ordered lifecycle inside that interval; a marker after Pi has already settled does not pass:

```text
compaction_end reason=threshold aborted=false willRetry=false result=true
agent_start
turn_start
message_end role=assistant text="FIXTURE_RECOVERY_TURN_EXECUTED" stopReason=stop
turn_end
agent_end
agent_settled
```

The test does not prescribe whether the implementation uses an internal follow-up, a steering message, or another safe continuation primitive; it only requires the externally observable concrete recovery turn.

## Controls and exclusions

### Overflow recovery is already automatic

```text
019 agent_end willRetry=false
020 compaction_start reason=overflow
021 compaction_end reason=overflow aborted=false willRetry=true result=true
022 agent_start
023 turn_start
028 message_end role=assistant text="OVERFLOW_RETRY_CONTINUED FIXTURE_RECOVERY_TURN_EXECUTED" stopReason=stop
029 turn_end
030 agent_end willRetry=false
031 agent_settled
```

The `agent_end.willRetry` field concerns Pi's transient-error retry decision and is false here; the authoritative overflow signal is `compaction_end.willRetry=true`. Pi then starts another agent run without a queued extension message. This proves the missing threshold turn is not overflow recovery (`willRetry=true`).

### Explicit manual compaction is a separate operation

```text
011 agent_end willRetry=false
012 agent_settled
013 compaction_start reason=manual
014 compaction_end reason=manual aborted=false willRetry=false result=true
```

The RPC `compact` response succeeds after the seed agent run has already settled. This proves an explicit user/client compaction is not the post-`agent_end` threshold path.

### Failed compaction is not counted as the bug

```text
011 agent_end willRetry=false
012 compaction_start reason=threshold
013 compaction_end reason=threshold aborted=false willRetry=false result=false error="Auto-compaction failed: Turn prefix summarization failed: fixture summarization failure"
014 agent_settled
```

The runner also asserts that this control has no persisted `compaction` entry. The target requires `result=true`, an appended entry, and no `errorMessage`.

### A follow-up queued during `agent_end` is independent

```text
011 queue_update steering=[] followUp=["AGENT_END_QUEUED_FOLLOW_UP"]
012 agent_end willRetry=false
013 agent_start
014 turn_start
015 queue_update steering=[] followUp=[]
022 message_end role=assistant text="AGENT_END_FOLLOW_UP_CONTINUED FIXTURE_RECOVERY_TURN_EXECUTED" stopReason=stop
025 agent_settled
```

This control exercises the separate `agent_end` extension-queue behavior. It continues correctly because the queue is nonempty. The target has no such extension and no nonempty queue, so it cannot pass accidentally via that independent path.

## Exact red assertion to inherit

The implementation task should inherit this assertion verbatim:

```text
RED: threshold compaction with explicit unfinished work must schedule one concrete post-compaction recovery turn (expected assistant marker FIXTURE_RECOVERY_TURN_EXECUTED after successful compaction_end(willRetry=false))
```

All overflow/manual/failure/`agent_end`-follow-up controls are asserted before this final check. The final predicate requires, in order and strictly between target `compaction_end` and the first `agent_settled`, `agent_start` → `turn_start` → assistant recovery marker → `turn_end` → `agent_end`. Consequently the default invocation fails only because the successful threshold case lacks its complete post-compaction recovery turn.
