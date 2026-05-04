# Native Executor: Unified Run Loop, Interruptibility, and Compaction

Design + phased implementation plan for restructuring the native executor
around a **turn-boundary-based run loop** that is shared between two
surfaces — `wg nex` (interactive TUI) and workgraph-spawned agents
(headless). Also rewires compaction to be always-on and content-preserving
rather than emergency-only.

## Motivation

The current native executor fails in three coupled ways:

1. **L1 compaction can no-op forever.** L1 stubs only *old large tool
   results*. When an agent does many small-output tool calls (`bg list`,
   `ls`, short curls), there are no old large tool results to stub, so
   L1 frees zero tokens, and the pressure keeps building. Trace evidence:
   eight consecutive `~26.5k → ~26.5k` L1 compactions culminating in
   `Context limit reached. Please start a new session.` L1 ran as
   designed — but designed for the wrong failure mode.
2. **Compaction pays a tax.** Each compaction injects a user-role
   `[System note: context was compacted …]` message, which itself
   consumes tokens. When compaction frees less than it injects (common
   in the above pathology), the system is strictly losing ground per
   turn.
3. **Nothing is interruptible.** Ctrl-C does not return to a prompt.
   Users cannot correct a thrashing agent without killing the process.
   There is no inbox, so a workgraph coordinator cannot tell a running
   agent "stop doing that." This is a *category* of missing capability,
   not a single bug.

All three resolve cleanly when the agent loop has a well-defined
**turn boundary** with explicit hooks. Compaction is a boundary hook.
Cancellation is a boundary hook. Message injection is a boundary hook.
Journaling is a boundary hook. Get the boundary right once, and every
downstream capability is a ~dozen-line addition.

## One run interface, two surfaces

```
                         ┌────────────────────┐
                         │   run_loop(…)      │  ← shared
                         │   turn boundary    │
                         └─────────┬──────────┘
                                   │
                 ┌─────────────────┼──────────────────┐
                 │                 │                  │
         ┌───────▼──────┐   ┌──────▼─────┐   ┌────────▼────────┐
         │  TUI inbox   │   │ IPC inbox  │   │    journal       │
         │  (stdin +    │   │ (wg send / │   │  (conversation.  │
         │   composing  │   │  file /    │   │   jsonl)         │
         │   buffer)    │   │  socket)   │   │                  │
         └───────┬──────┘   └──────┬─────┘   └──────────────────┘
                 │                 │
          ┌──────▼──────┐   ┌──────▼──────┐
          │ TUI render  │   │ headless    │
          │ (ratatui:   │   │ render      │
          │  transcript │   │ (stderr     │
          │  + buffer)  │   │  streaming) │
          └─────────────┘   └─────────────┘
```

The difference between `wg nex` and a workgraph agent is **only** (a)
which `Inbox` implementation is plugged in and (b) which `Renderer` is
plugged in. The loop, the compaction logic, the journal, the
cancellation model — all identical.

## The turn boundary

```rust
// Pseudocode. The only place the agent's in-memory state mutates
// between LLM calls. Every capability hangs off one of these hooks.
loop {
    // --- turn boundary ---------------------------------------------

    // 1. Cancellation check (cooperative)
    if cancel.requested_cooperative() {
        break ControlFlow::ReturnToPrompt;
    }

    // 2. Drain inbox (user input accumulated during prior turn)
    for input in inbox.drain().await {
        messages.push(input.into_user_message());
    }

    // 3. Microcompact (always — not emergency-only)
    if budget.above_soft_threshold(&messages) {
        messages = compact.microcompact(&messages).await;
    }

    // 4. Journal the pre-LLM state
    journal.append_turn_boundary(&messages, compaction_state)?;

    // --- end turn boundary -----------------------------------------

    // 5. LLM call — cancellable via tokio::select! on cancel signal
    let action = tokio::select! {
        action = next_action(&messages) => action?,
        _ = cancel.cooperative() => break ControlFlow::ReturnToPrompt,
        _ = cancel.hard() => break ControlFlow::ForceReturn,
    };

    // 6. Tool execution — cancellable; hard-cancel triggers tree-kill
    let result = tokio::select! {
        r = execute(action) => r,
        _ = cancel.cooperative() => { /* let tool finish cleanly */ continue }
        _ = cancel.hard() => { tree_kill_subprocesses(); break }
    };

    messages.push(result.into_message());
}
```

Every existing capability fits into one of those six steps. Every new
capability we've discussed fits too.

## Interruption semantics

| Signal                              | Behavior                                         |
|-------------------------------------|--------------------------------------------------|
| **Single Ctrl-C**                   | Cooperative cancel. In-flight tool finishes; in-flight LLM request is aborted at the network level. Next turn-boundary returns to prompt. |
| **Double Ctrl-C** (<500ms apart)    | Hard cancel. Tree-kill current subprocesses (bash children, headless Chrome, etc.). In-flight LLM request aborted. Immediate return to prompt. |
| **`nohup`/`disown`'d children**     | Survive both forms of cancel. Standard Unix semantic — the user explicitly requested that the process outlive its parent. |
| **workgraph "stop" message**        | Equivalent to single Ctrl-C: cooperative cancel via the inbox. |
| **workgraph "kill" command**        | Equivalent to double Ctrl-C: hard cancel. |

Implementation note: a single `CancelToken` with two levels
(`Cooperative`, `Hard`) fed from multiple sources (signal handler,
inbox, external IPC) keeps the policy in one place.

## Inbox abstraction

```rust
pub trait AgentInbox: Send + Sync {
    /// Non-blocking drain of any accumulated user inputs.
    async fn drain(&mut self) -> Vec<UserInput>;
}

pub enum UserInput {
    /// Injected between turns, visible to the agent as its next user
    /// message. Does NOT interrupt in-flight work.
    Note(String),
    /// Same as Note but also triggers cooperative cancel of in-flight
    /// work — the agent will see this as its next user message after
    /// the current turn wraps up.
    Interrupt(String),
}
```

### TUI implementation

An in-memory `VecDeque` fed by the render loop. The composing buffer
is a separate piece of render state: text typed into the buffer is
*not* in the inbox until Enter is pressed; at that point the buffer's
contents are pushed into the inbox queue as `Note` (or `Interrupt`
when a hotkey modifier is held) and the buffer clears.

### workgraph implementation

A file-based inbox at `<workgraph>/inbox/<agent-id>.jsonl`. Writers
(coordinator, `wg send`, other agents) append one line per message.
The agent tails the file and `drain()` returns everything appended
since the last drain. File-based keeps the everything-is-a-file ethos
and works across processes without shared memory.

Later, a socket or channel-based implementation may be layered on top
for lower latency in latency-critical flows. The trait stays the same.

## TUI design

```
┌─────────────────────────────────────── wg nex ─── mistral-small ── 12847 tok ─┐
│                                                                               │
│  > read_file README.md                                                        │
│  ← Read 214 lines, 7312 bytes.                                                │
│                                                                               │
│  > summarize README.md what does this project do?                             │
│  ← workgraph is a lightweight task coordination graph for humans and AI…      │
│                                                                               │
│  [context compacted: 29100 → 14200 tokens via microcompact]                   │
│                                                                               │
│  > web_search "qwen3-coder benchmarks 2026"                                   │
│  ← (streaming…)                                                               │
│  │  • Benchmark release notes qwen3-coder-30b — jan 2026                      │
│  │  • Reddit thread comparing coder variants…                                 │
│                                                                               │
│                                                                               │
├───────────────────────────────────────────────────────────────────────────────┤
│ » also check the arxiv one, that looked interesting                       _   │  ← composing
└───────────────────────────────────────────────────────────────────────────────┘
   ^C cancel   ^C^C kill   Enter send   PgUp/PgDn scroll
```

Properties:

- **Composing buffer** at the bottom: always visible, always editable.
  The user can type during agent work; typing does *not* send. Enter
  sends — at that point the text is pushed to the inbox and scrolls up
  into the transcript as "user said: …".
- **Scrolling transcript** above: streams agent output upward as it
  arrives. Tool calls, results, compaction events, inbox-delivered
  user messages — all appear in the transcript.
- **Ctrl-C hotkeys** as in the table above.
- **Shift-Enter** adds a newline to the buffer (multi-line compose).
- **Ctrl-Enter** sends as `Interrupt` rather than `Note`.
- **PgUp/PgDn** scroll the transcript; the buffer is unaffected.

The user sees a conversation that *feels* like chat, but mechanically
it is an inbox drained at turn boundaries. No cross-thread locking,
no "are we in the middle of a tool call, can we accept input" — the
buffer is always accepting input, and the inbox is always draining at
a boundary.

## Compaction overhaul

Replace the current three-tier emergency ladder with a **continuous
microcompact + LLM summary fallback** model.

### 1. Always-on microcompact (new L1)

Runs at **every turn boundary**, not on emergency. Threshold: ~60% of
the effective context window. When above threshold:

1. Identify the oldest "big" content in the message vec. "Big" = any
   block whose content exceeds a configurable byte threshold
   (default: 2KB). Unlike current L1, this includes **any** content
   type: tool_result, large text blocks, lengthy thinking blocks.
2. Summarize that block via a cheap LLM call (~200-tok target) using
   a focused instruction: *"Summarize this tool output / assistant
   turn for future reference. Preserve: decisions made, key facts,
   filenames/URLs cited. Drop: verbatim content, tangential detail."*
3. Replace the block with `[summarized, {orig_size}B → {summary_size}B]
   <summary>`.

Key differences from current L1:
- Runs *before* pressure, not at emergency. Pressure never builds.
- Works on narrative content too, not just tool_results — fixes the
  bug in the attached trace directly.
- Preserves signal (summary) instead of destroying it (stub).
- No self-injected `[System note: compacted]` tax — the replacement
  block *is* the note.

Cost: one cheap LLM call per "big block" per compaction event. In
practice, most turn boundaries will have no big block above threshold,
so cost is ~zero for most turns.

### 2. Full-history summary (new L2)

Fires when microcompact cannot free enough to get under the hard
threshold. Runs `recursive_summarize` over older messages using a
**9-section prompt** ported from Claude Code:

1. Primary Request and Intent
2. Key Technical Concepts
3. Files and Code Sections
4. Errors and Fixes
5. Problem Solving
6. All user messages (verbatim)
7. Pending Tasks
8. Current Work
9. Optional Next Step

Concrete evidence this works: the very top of the current Claude Code
session (that this design is being written in) is an L2 summary using
this exact prompt. The session is fully coherent despite having been
compacted. We have ground truth that the prompt is good.

### 3. Post-compact working-state re-injection (new L3)

After L2 runs, re-read the contents of files the agent has recently
touched (tracked by a `TouchedFiles` set updated by `read_file`,
`write_file`, `edit_file`) and attach them as fresh context, capped at
5K tokens per file and 25K total. The model wakes up from compaction
with: *(summary)* + *(recent messages verbatim)* + *(refreshed view of
active files)*. The summary carries narrative; the re-injection
carries state. Both are needed — either alone loses the agent.

### 4. Escalation ladder fix

Current bug: a successful L2 resets `noop_streak`, dropping us back to
L1 next turn. If L1 noops again, streak starts over. We ping-pong
indefinitely.

Fix: streak tracks *L1 noops specifically*, not any compaction. L2 and
L3 successes don't reset the L1-noop counter. Once 3 consecutive L1
noops are seen at the same context size, escalate to L2 and stay
there for the rest of the session.

### Acceptance test

The attached trace (user asks agent to measure token throughput, agent
loops for 8 turns burning compactions) must complete successfully.
Replay the conversation with the new loop and confirm (a) the agent
actually downloads the book, (b) context stays below 50% utilization,
(c) no `Context limit reached` error.

## Phased implementation

Six stages, each self-contained (leaves the tree buildable and the
executor usable), ordered so that each stage's prerequisites are
complete before it starts.

### Stage A: turn-boundary loop scaffold + single Ctrl-C

**Goal**: Restructure `agent.rs::run_loop` around the six-step
boundary, introduce `CancelToken` with `Cooperative` level only, hook
up `tokio::signal::ctrl_c()` to set it. Behavior unchanged otherwise —
compaction still uses the current ladder, inbox is a no-op.

**Verification**: Ctrl-C during an agent run cleanly returns to prompt
within the duration of the current tool / LLM request.

**Risk**: Low. The refactor preserves existing behavior; only the
Ctrl-C signal is new.

### Stage B: double Ctrl-C tree-kill + inbox trait

**Goal**: Add `Hard` cancel level. Detect double-Ctrl-C (<500ms).
Implement subprocess tree-kill (we already have `/proc`-walking code
from prior work). Introduce `AgentInbox` trait with an in-memory
implementation (empty queue for now — no UI yet). Wire inbox drain
into the boundary.

**Verification**: Start an agent running a slow `bash sleep 60`.
Single Ctrl-C waits for sleep. Double Ctrl-C kills the bash tree
immediately and returns to prompt.

**Risk**: Tree-kill can leave orphans if a child double-forks with
`setsid`. Acceptable — that's the `nohup` escape hatch by design.

### Stage C: microcompact-every-turn + 9-section summary + ladder fix

**Goal**: Implement the continuous microcompact at the boundary. Port
the 9-section summary prompt for the L2 fallback. Fix the noop_streak
reset bug. Remove the per-compaction `[System note]` tax.

**Verification**: Replay the attached trace. Agent completes the
Gutenberg-download task without hitting `Context limit reached`.
Add a unit test that generates a pathological "many small tool calls"
conversation and asserts the loop terminates successfully.

**Risk**: The cheap-LLM-call-per-block cost adds latency. Measure.
Likely <100ms per call against lambda01. Acceptable.

### Stage D: post-compact file re-injection

**Goal**: Track touched files in a `TouchedFiles` set. On L2 firing,
re-read up to 5 most-recent files (5K cap each, 25K total) and append
as `ToolResult` blocks tied to synthetic tool-use IDs.

**Verification**: Agent reads 10 files, hits L2, continues working on
the same files after compaction. File contents should still be
referenceable without a second `read_file` call.

**Risk**: If files have been modified externally between read and
re-inject, the model gets fresher content than the summary describes.
Acceptable — fresher is correct.

### Stage E: TUI composing buffer + streaming transcript

**Goal**: Rewrite the `wg nex` frontend around ratatui (if not
already). Composing buffer at the bottom, scrolling transcript above,
Ctrl-C hotkeys, Enter-sends, Shift-Enter multi-line, Ctrl-Enter
interrupt-send, PgUp/PgDn scroll. Stream agent output upward as it
arrives.

**Verification**: Type a message while the agent is running. Confirm
it sits in the buffer until Enter. Confirm Enter delivers it on the
next turn boundary without cancelling in-flight work.

**Risk**: Medium. ratatui input handling has sharp edges around
multi-line editing, IME, paste. Ship with a simple editor first,
iterate.

### Stage F: workgraph IPC inbox + `wg send`

**Goal**: Implement the file-based `AgentInbox` at
`<workgraph>/inbox/<agent-id>.jsonl`. Add a `wg send <agent-id>
"message" [--interrupt]` CLI that appends to the target agent's
inbox. workgraph coordinator uses this to cooperatively steer
in-flight agents.

**Verification**: Coordinator dispatches an agent. Before it
completes, `wg send <id> "stop that, do X instead"`. Agent picks up
the message at its next turn boundary (latency ≤ current tool
duration) and adjusts course.

**Risk**: Low. File-based IPC is well-understood. The latency story
is good because the inbox is checked at every boundary — there's no
polling interval.

## Verification gates

A stage is "done" only when:

1. All new code has unit or integration tests.
2. `cargo build --release` and `cargo test --release --lib --bins`
   are clean.
3. The stage's verification scenario (above) runs successfully
   against the installed binary.
4. The stage is committed as a single logical commit with a message
   that references this plan.

## What this plan does not include

Deliberately out of scope:

- **Multi-agent conversations** beyond the coordinator↔agent pattern.
  Agents don't directly talk to other agents yet; the coordinator
  remains the router.
- **Journal read-back during compaction.** Discussed, deferred. The
  journal stays fire-and-forget audit for now. If future sessions
  show that summaries are consistently losing critical content, we
  can revisit "stub with journal-pointer" as a refinement.
- **Replacing the TUI from scratch.** Stage E rewrites the frontend
  but keeps the current control/key bindings where they already
  work. It's a refactor, not a redesign.

## Related documents

- `docs/design/smooth-integration.pdf` — the broader workgraph
  integration story this plan fits into.
- `docs/design/agent-lifecycle.md` — how agents are spawned, killed,
  reaped. The cancellation model in Stage B must agree with this.
- `docs/design/coordinator-chat-protocol.md` — the coordinator-agent
  message format. The workgraph inbox in Stage F must be compatible.
- `docs/design/unified-path-forward.md` — the prior "Gate 1–4"
  readiness plan. This plan supersedes Gate 1 (context management).
