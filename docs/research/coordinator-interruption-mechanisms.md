# Coordinator Interruption Mechanisms in TUI

**Date:** 2026-03-13
**Task:** research-coordinator-interruption

## Problem Statement

The coordinator is unstoppable once running — a freight train. The user can watch it stream tokens but cannot interrupt it mid-thought. When the coordinator goes off in the wrong direction, the only option is switching to another coordinator chat, which is awkward.

## Current Architecture

### Coordinator Subprocess Lifecycle

The coordinator runs as a **persistent Claude CLI subprocess** managed by `CoordinatorAgent` (`src/commands/service/coordinator_agent.rs`).

1. **Spawn**: `CoordinatorAgent::spawn()` launches `claude --input-format stream-json --output-format stream-json` as a child process.
2. **Management thread**: A dedicated `agent_thread_main()` thread owns the `Child` process. It reads chat requests from an `mpsc::Sender<ChatRequest>` channel.
3. **Message flow**: TUI → `wg chat` (background command) → IPC `UserChat` → daemon main loop → `CoordinatorAgent::send_message()` → `mpsc` channel → agent thread → write to `stdin` as stream-json.
4. **Response flow**: Agent thread spawns a `stdout_reader` thread that parses stream-json events. `collect_response()` buffers text/tool_use/tool_result fragments and writes progressive text to `.workgraph/chat/<N>/.streaming` for TUI display.
5. **Shutdown**: `CoordinatorAgent::shutdown()` drops the `mpsc::Sender`, the agent thread detects the disconnect, calls `child.kill()` + `child.wait()`.
6. **Crash recovery**: On process exit, the thread auto-restarts with context injection (rate-limited: max 3 restarts per 10 minutes).

Key code locations:
- `coordinator_agent.rs:305` — `spawn()` creates the agent
- `coordinator_agent.rs:366` — `send_message()` queues via channel
- `coordinator_agent.rs:391` — `shutdown()` drops sender
- `coordinator_agent.rs:530-544` — channel disconnect → kill + wait
- `coordinator_agent.rs:624-629` — `collect_response()` with 300s timeout
- `coordinator_agent.rs:1439` — `spawn_claude_process()` creates the `Child`

### Current Ctrl+C Behavior in TUI

The TUI (`src/tui/viz_viewer/event.rs`) handles `Ctrl+C` differently depending on context:

| Context | Mode | Ctrl+C Behavior | Line |
|---------|------|-----------------|------|
| Search input | Insert | Clears search, returns to Normal | event.rs:276 |
| Text prompt | Insert | Clears editor, returns to Normal | event.rs:557 |
| Chat input | Insert | Clears editor, returns to Normal (or cancels edit mode) | event.rs:709-717 |
| Message input | Insert | Clears editor + draft, returns to Normal | event.rs:805-812 |
| Graph panel | Normal | **Kills the agent on the focused task** | event.rs:872-874 |
| Right panel | Normal | **Kills the agent on the focused task** | event.rs:1245-1246, 1271-1273 |
| Files panel (searching) | — | **Kills the agent on the focused task** | event.rs:1236-1237 |

The `kill_focused_agent()` method (`state.rs:7000`) loads the graph, finds the agent assigned to the selected task, and runs `wg kill <agent-id>` in the background.

**Critical insight:** In Normal mode, Ctrl+C already kills agents — but it kills the agent on the *focused task*, not the coordinator. There is no mechanism to interrupt the coordinator's current generation specifically.

### Existing IPC Commands for Coordinator Control

| Command | What it does | Effect on Agent |
|---------|-------------|-----------------|
| `StopCoordinator` | Kills agent via registry, resets task to Open | `agent.shutdown()` called in daemon loop; agent re-created on next message |
| `ArchiveCoordinator` | Marks coordinator task as Done, removes agent | Agent permanently stopped |
| `Pause` / `Resume` | Sets `paused` flag on daemon config + coordinator state | Prevents new ticks, but does NOT interrupt current generation |
| `DeleteCoordinator` | Removes coordinator task + agent | Agent permanently stopped |

`StopCoordinator` is the closest to interruption, but it's a **full restart** — the coordinator loses its entire conversation context and starts fresh on the next message. This is a sledgehammer, not a scalpel.

---

## Interruption Mechanisms: Analysis

### 1. Signal-Based Interruption (Send SIGINT to Claude CLI Subprocess)

**Mechanism:** The daemon's `CoordinatorAgent` holds the `Child` process. Expose a way to send `SIGINT` (not `SIGKILL`) to the child PID, causing Claude CLI to gracefully stop the current generation and return a partial response.

**How Claude Code handles Ctrl+C:** Claude Code (the CLI) interprets `SIGINT` during streaming as "stop generating" — it terminates the current API call, outputs what it has so far, and returns to the input prompt. The stream-json protocol would emit a `TurnComplete` signal after the interruption.

**Implementation:**
- Add `pub fn interrupt(&self)` to `CoordinatorAgent` that sends `SIGINT` to the stored PID
- Add `InterruptCoordinator { coordinator_id: u32 }` IPC request
- In the daemon loop, route it to `coordinator_agents[cid].interrupt()`
- In the TUI, wire Ctrl+C in chat context (while `awaiting_response`) to send this IPC

**Pros:**
- **Preserves conversation context** — the Claude CLI process stays alive
- **Graceful** — Claude Code handles SIGINT natively, outputs partial response
- **Fast** — signal delivery is immediate
- **No restart penalty** — next message continues the same session
- **Matches user mental model** — Ctrl+C means "stop what you're doing" everywhere

**Cons:**
- **Relies on Claude CLI's SIGINT handling** — if Claude CLI doesn't handle it gracefully (e.g., crashes), the agent thread's crash recovery kicks in
- **Partial response handling** — the `collect_response()` loop needs to handle a truncated turn gracefully (it likely already does since it handles timeouts)
- **Race condition window** — if SIGINT arrives between turns (no generation in progress), it might kill the process unnecessarily; need to guard with an "is generating" flag

**Complexity:** Low-medium. ~100 lines of new code.

### 2. Process-Level Kill + Restart (Enhanced StopCoordinator)

**Mechanism:** Use the existing `StopCoordinator` IPC, but enhance it to preserve conversation context on restart.

**Implementation:**
- Keep `StopCoordinator` as-is (kill + reset to Open)
- Enhance crash recovery to inject the full conversation history on restart
- Add a TUI keybinding that sends `StopCoordinator` and immediately re-sends the last message (or a "continue" message)

**Pros:**
- **Already mostly implemented** — StopCoordinator exists
- **Clean slate** — guaranteed consistent state after restart

**Cons:**
- **Loses conversation context** — the Claude CLI subprocess restarts; even with crash recovery, the LLM loses its internal reasoning state
- **Slow** — kill + wait + restart + context injection takes several seconds
- **Token waste** — crash recovery re-injects conversation summary, consuming input tokens
- **Poor UX** — visible delay while coordinator restarts
- **Doesn't feel like "interrupt"** — feels like "restart"

**Complexity:** Low. Mostly wiring existing code.

### 3. IPC-Based Interruption (New InterruptCoordinator Command)

**Mechanism:** Add a new `InterruptCoordinator` IPC command that is distinct from `StopCoordinator`. Instead of killing the process, it sends SIGINT to the child process specifically.

This is essentially **Mechanism 1 wrapped in an IPC command**, which is the right architecture since the TUI communicates with the daemon via IPC.

**Implementation:**
- Add `InterruptCoordinator { coordinator_id: u32 }` to `IpcRequest` enum
- Add `pub fn interrupt(&self) -> bool` to `CoordinatorAgent` — sends `SIGINT` to child PID, returns whether signal was sent
- Handle in `ipc::handle_connection` — call `coordinator_agents[cid].interrupt()` directly (no graph changes, no kill, no restart)
- In TUI: detect Ctrl+C while `chat.awaiting_response` is true → send `InterruptCoordinator` IPC instead of the normal Ctrl+C action
- Write an interrupted-response message to the outbox so the user sees what the coordinator was saying

**Pros:**
- All pros of Mechanism 1
- **Clean separation** — InterruptCoordinator ≠ StopCoordinator semantically
- **No graph mutation** — the coordinator task stays in-progress, no status changes
- **Extensible** — could later add an interrupt reason or follow-up message

**Cons:**
- **Slightly more IPC boilerplate** — new variant, handler, test
- **Same SIGINT dependency** as Mechanism 1

**Complexity:** Medium. ~150 lines of new code across 4 files.

### 4. Chat-Level Interruption (Interrupt Message via wg msg)

**Mechanism:** Send a special message to the coordinator's inbox that, when read by the coordinator on its next context refresh, causes it to stop.

**Implementation:**
- Define a convention (e.g., `__INTERRUPT__` message type)
- The coordinator agent checks for interrupt messages between tool calls
- On seeing an interrupt, it outputs a "generation interrupted" message and stops

**Pros:**
- **No process signals** — purely message-based
- **Works across any executor** — not tied to Claude CLI's SIGINT handling

**Cons:**
- **Not immediate** — the coordinator only checks messages at context-refresh boundaries (between turns), NOT mid-generation
- **Doesn't stop mid-token-stream** — if the coordinator is in the middle of a 300s generation, the interrupt won't be seen until the turn completes
- **Requires LLM cooperation** — the coordinator has to understand and obey the interrupt message, which is unreliable
- **Defeats the purpose** — if we have to wait for the current turn to complete, there's nothing to interrupt

**Complexity:** Low, but **fundamentally inadequate** for the stated problem.

### 5. Hybrid: SIGINT + Streaming File Sentinel

**Mechanism:** Combine process-level SIGINT with a sentinel file that the `collect_response()` loop checks.

**Implementation:**
- When user requests interrupt, write a sentinel file (`.workgraph/chat/<N>/.interrupt`)
- Send `SIGINT` to the Claude CLI subprocess
- `collect_response()` checks for the sentinel file on each token and exits early if found
- Clear sentinel after interruption

**Pros:**
- **Belt and suspenders** — even if SIGINT doesn't work immediately, the collect loop exits
- **Observable** — the sentinel file can be checked by other components

**Cons:**
- **Over-engineered** — SIGINT alone should suffice
- **Two mechanisms to maintain** for one feature

**Complexity:** Medium. Not recommended due to unnecessary complexity.

---

## Pros/Cons Matrix

| Criterion | 1. SIGINT | 2. Kill+Restart | 3. IPC+SIGINT | 4. Chat Message | 5. Hybrid |
|-----------|-----------|-----------------|---------------|-----------------|-----------|
| **Immediate** | ✅ | ❌ (seconds) | ✅ | ❌ (next turn) | ✅ |
| **Preserves context** | ✅ | ❌ | ✅ | N/A | ✅ |
| **Reliable** | ✅ | ✅ | ✅ | ❌ | ✅ |
| **Simple** | ✅ | ✅ | ⚠️ | ✅ | ❌ |
| **Matches UX** | ✅ | ❌ | ✅ | ❌ | ✅ |
| **No restart cost** | ✅ | ❌ | ✅ | ✅ | ✅ |
| **Works with any executor** | ❌ (CLI only) | ✅ | ❌ (CLI only) | ✅ | ❌ |

---

## Recommendation: Mechanism 3 (IPC-Based InterruptCoordinator with SIGINT)

**Rationale:**

1. **Architecturally clean**: The TUI already communicates with the daemon via IPC. Adding a new IPC command is the natural extension.
2. **Semantically distinct from Stop**: Interrupting ("stop this generation") is fundamentally different from stopping ("kill this coordinator"). They should be separate IPC commands.
3. **Preserves conversation**: The Claude CLI process stays alive after SIGINT. The coordinator can continue with the next message without losing its reasoning context.
4. **User expectation**: Ctrl+C during streaming universally means "stop generating" in AI chat interfaces. Users will expect this behavior.
5. **Low risk**: The worst case (Claude CLI doesn't handle SIGINT gracefully) triggers the existing crash-recovery path, which already works.

---

## Implementation Sketch

### Files to Change

1. **`src/commands/service/coordinator_agent.rs`** — Add `interrupt()` method
2. **`src/commands/service/ipc.rs`** — Add `InterruptCoordinator` IPC variant + handler
3. **`src/commands/service/mod.rs`** — Route IPC to coordinator agent; add `run_interrupt_coordinator()` public API
4. **`src/tui/viz_viewer/event.rs`** — Wire Ctrl+C in chat context to interrupt
5. **`src/tui/viz_viewer/state.rs`** — Add `interrupt_coordinator()` method
6. **`src/chat.rs`** — (Optional) Add `write_interrupted()` for the interrupted response

### Step-by-Step

#### 1. `CoordinatorAgent::interrupt()` (coordinator_agent.rs)

```rust
/// Interrupt the current generation by sending SIGINT to the Claude CLI process.
///
/// Returns true if SIGINT was sent, false if the process is not alive.
/// The Claude CLI handles SIGINT by stopping the current generation and
/// emitting a TurnComplete signal, preserving the conversation context.
pub fn interrupt(&self) -> bool {
    let pid = *self.pid.lock().unwrap_or_else(|e| e.into_inner());
    if pid == 0 {
        return false;
    }
    // Send SIGINT (not SIGKILL) — Claude CLI treats this as "stop generating"
    unsafe {
        libc::kill(pid as i32, libc::SIGINT);
    }
    true
}
```

#### 2. IPC Variant (ipc.rs)

```rust
// In IpcRequest enum:
/// Interrupt the current coordinator generation (sends SIGINT, does NOT kill).
InterruptCoordinator { coordinator_id: u32 },
```

Handler in `handle_connection`:
```rust
IpcRequest::InterruptCoordinator { coordinator_id } => {
    logger.info(&format!("IPC InterruptCoordinator: coordinator_id={}", coordinator_id));
    // This does NOT go through handle_stop_coordinator — no graph changes,
    // no kill, no restart. Just signal the process.
    // The actual interrupt is done by the caller (daemon main loop)
    // since coordinator_agents is not accessible here.
    // Signal via a new output parameter.
    *interrupt_coordinator_id = Some(coordinator_id);
    IpcResponse::success(serde_json::json!({
        "coordinator_id": coordinator_id,
        "interrupted": true,
    }))
}
```

#### 3. Daemon Loop Wiring (mod.rs)

In the main daemon loop, after `handle_connection`:
```rust
// After the existing delete_coordinator_ids handling:
if let Some(cid) = conn_interrupt_coordinator_id {
    if let Some(agent) = coordinator_agents.get(&cid) {
        let sent = agent.interrupt();
        logger.info(&format!(
            "Interrupted coordinator {} (SIGINT sent: {})", cid, sent
        ));
    }
}
```

#### 4. TUI Ctrl+C Routing (event.rs)

In `handle_chat_input` (the Insert-mode chat handler), change the Ctrl+C case:
```rust
KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
    if app.chat.awaiting_response {
        // Interrupt the running coordinator instead of clearing input
        app.interrupt_coordinator();
    } else if in_edit_mode {
        app.cancel_chat_edit_mode();
    } else {
        editor_clear(&mut app.chat.editor);
        app.input_mode = InputMode::Normal;
        app.inspector_sub_focus = InspectorSubFocus::ChatHistory;
    }
    return;
}
```

Also in Normal mode (graph panel, right panel), when in chat tab and awaiting response:
```rust
// In the Normal-mode Ctrl+C handler:
KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
    if app.chat.awaiting_response && app.right_panel_tab == RightPanelTab::Chat {
        app.interrupt_coordinator();
    } else {
        app.kill_focused_agent();
    }
}
```

#### 5. `VizApp::interrupt_coordinator()` (state.rs)

```rust
/// Interrupt the active coordinator's current generation.
/// Sends InterruptCoordinator IPC to the daemon, which sends SIGINT
/// to the Claude CLI subprocess.
pub fn interrupt_coordinator(&mut self) {
    let cid = self.active_coordinator_id;
    self.exec_command(
        vec![
            "service".to_string(),
            "interrupt-coordinator".to_string(),
            cid.to_string(),
        ],
        CommandEffect::Notify(format!("Interrupted coordinator {}", cid)),
    );
    // Optimistically clear awaiting state — the response collector
    // will write whatever partial text it has to the outbox.
    self.chat.awaiting_response = false;
    self.chat.streaming_text.clear();
}
```

#### 6. Handle Partial Response in Agent Thread

In `collect_response()`, when the Claude CLI emits `TurnComplete` after SIGINT, the response may be truncated. The existing logic already handles this — it writes whatever text was collected to the outbox. The only addition is to detect the interruption and annotate the response:

```rust
// In agent_thread_main, after collect_response returns:
// If the response is shorter than expected and we just received SIGINT,
// append an [interrupted] marker to help the user understand.
```

### UX Flow

1. User is watching coordinator stream in TUI chat panel
2. Coordinator starts going off-track
3. User presses **Ctrl+C**
4. TUI detects `awaiting_response == true` in chat context
5. TUI sends `InterruptCoordinator` IPC to daemon
6. Daemon calls `coordinator_agents[cid].interrupt()` → SIGINT to PID
7. Claude CLI stops generation, emits partial `TurnComplete`
8. Agent thread's `collect_response()` receives the truncated turn
9. Partial response written to outbox with `[interrupted]` suffix
10. TUI picks up the response, clears streaming indicator
11. User can immediately type a new message to redirect the coordinator

### Interaction with Existing Mechanisms

- **Pause service**: Pause prevents new coordinator ticks but doesn't interrupt a generation in progress. `InterruptCoordinator` is complementary — it handles the mid-generation case.
- **StopCoordinator**: Remains the "nuclear option" — kills the process and resets. `InterruptCoordinator` is the gentle version.
- **Kill focused agent**: Ctrl+C in Normal mode when NOT awaiting a coordinator response still kills the task agent. The two behaviors are context-dependent.

### Edge Cases

- **No generation in progress**: SIGINT to an idle Claude CLI process would likely be ignored or cause it to exit. Guard with an `is_generating` flag or check `awaiting_response` state.
- **Rapid double-interrupt**: The second SIGINT might kill the process. Rate-limit interrupts or ignore if already interrupted.
- **Non-Claude executors**: The amplifier executor spawns differently. The interrupt mechanism should be executor-aware. For now, only implement for Claude CLI; add a "not supported" response for other executors.
