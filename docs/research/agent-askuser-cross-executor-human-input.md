# Agent AskUser & Cross-Executor Human Input

Research report on how agents request human input during execution, and a design for cross-executor human input in wg.

---

## 1. What is AskUserQuestion?

`AskUserQuestion` is a **built-in Claude Code tool** — not a wg concept. It appears in the `system` init event that Claude Code emits at the start of every session:

```json
{
  "type": "system",
  "subtype": "init",
  "tools": [
    "Task", "Bash", "Glob", "Grep", "Read", "Edit", "Write",
    "AskUserQuestion", "Skill", "EnterPlanMode", ...
  ]
}
```

### What it looks like when called

When an interactive Claude Code session calls `AskUserQuestion`, it emits a tool_use block:

```json
{
  "type": "tool_use",
  "id": "toolu_abc123",
  "name": "AskUserQuestion",
  "input": {
    "questions": [
      {
        "question": "Which approach should I take for the refactor?",
        "header": "Approach",
        "options": [
          { "label": "Strategy A", "description": "Rewrite from scratch" },
          { "label": "Strategy B", "description": "Incremental migration" }
        ],
        "multiSelect": false
      }
    ]
  }
}
```

The Claude Code UI renders this as a chooser widget with clickable options.

### In --print mode (agent context)

Agents run via `claude --print`, which is non-interactive. Key finding:

**No agent in the wg logs has ever actually called `AskUserQuestion`.**

I scanned all `.wg/agents/*/output.log` files (across dozens of agents) and found zero instances of `AskUserQuestion` appearing as a `tool_use` call. The tool is _listed_ in the init event (because Claude Code always advertises its full tool set), but agents operating under wg prompts don't attempt to use it — they follow the `wg` commands in the Required Workflow section instead.

This means the problem is **preemptive** rather than reactive: we're designing for a scenario that will become important as agents become more autonomous, rather than fixing a current bug.

### What happens if an agent _did_ call it?

In `--print` mode, Claude Code would attempt to present the question but there's no terminal to render it in. The behavior is:
- The model emits the tool_use block in the JSONL stream
- Claude Code has no stdin to receive an answer
- The session either hangs, times out, or the tool returns an error
- The agent may retry or proceed without the answer

---

## 2. How the JSONL Stream Works Today

The wg stream capture system (`src/stream_event.rs`) translates Claude CLI JSONL events into a unified `StreamEvent` enum:

| Claude CLI event type | StreamEvent variant | Captured? |
|---|---|---|
| `system` | `Init` | Yes |
| `assistant` | `Turn` (extracts tool names + usage) | Yes |
| `result` | `Result` | Yes |
| Other types | Ignored | No — falls through `_ => None` |

**The `assistant` event translation already extracts tool names from content blocks.** If an agent called `AskUserQuestion`, it would appear in the `tools_used: Vec<String>` field of the `Turn` event. However, the tool _input_ (the actual question text and options) is not currently captured — only the tool name.

### Interception opportunity

To intercept `AskUserQuestion` from the Claude JSONL stream:

1. In `translate_claude_event()`, match on `assistant` events
2. Check if any content block has `"name": "AskUserQuestion"`
3. Extract the `input.questions` array
4. Convert to a `wg ask` record (see Section 4)

This would be a Claude-Code-specific adapter. The cross-executor solution needs to be generic.

---

## 3. How Other Executors Signal "I Need Human Input"

### Native executor (OpenRouter/Anthropic API direct)

The native executor (`src/executor/native/agent.rs`) runs a tool-use loop. It could signal human input need by:

1. **Calling a `wg_ask` tool** — we add a tool to the tool registry that the LLM can call
2. The tool handler writes a question record to the task, then blocks until answered
3. The answer becomes the tool result, and the loop continues

### Amplifier executor

Amplifier has its own approval system (`ApprovalSystem` in amplifier-core). It could:

1. Map `wg ask` to its internal approval flow
2. Or delegate to wg's mechanism via the `WG_TASK_ID` env var

### Generic pattern

All executors need exactly one thing: **a way for the agent process to block on human input and resume when answered.**

---

## 4. Recommended Architecture: `wg ask`

### Design

```
wg ask <task-id> "What color should the button be?" --options "red,blue,green" [--timeout 1h]
```

**Behavior:**
1. Creates a `question` record in `.wg/tasks/<task-id>/questions/<uuid>.json`
2. Sends notification via `NotificationRouter` with `EventType::Approval` (or a new `EventType::Question`)
3. **Blocks** (polls `.wg/tasks/<task-id>/answers/<uuid>.json`) until answered or timeout
4. Prints the answer to stdout and exits 0
5. If timeout: exits 1 with "no answer received"

**Non-blocking variant:**
```
wg ask <task-id> "question?" --no-wait    # creates question, prints question-id, exits immediately
wg ask check <question-id>                # check if answered (exit 0 = yes, exit 1 = no)
wg ask answer <question-id> "blue"        # answer a question (human/CLI)
```

### Question record format

```json
{
  "id": "q-a1b2c3",
  "task_id": "impl-button-colors",
  "agent_id": "agent-1234",
  "question": "What color should the button be?",
  "options": ["red", "blue", "green"],
  "allow_freeform": true,
  "created_at": "2026-03-04T12:00:00Z",
  "timeout": "PT1H",
  "status": "pending",
  "answer": null,
  "answered_by": null,
  "answered_at": null,
  "channel": null
}
```

### Answer flow

```
Human answers via:                           Agent receives via:
─────────────────                           ──────────────────
Telegram inline keyboard  ──┐
Matrix reaction/reply     ──┤
Slack button              ──┼──> wg ask answer q-a1b2c3 "blue" ──> answer file written
CLI directly              ──┤                                      agent poll succeeds
Discord button            ──┘                                      agent unblocks
```

### Integration with NotificationRouter

Add `EventType::Question` to the notification system:

```rust
pub enum EventType {
    TaskReady,
    TaskBlocked,
    TaskFailed,
    Approval,
    Urgent,
    Question,  // NEW
}
```

When `wg ask` creates a question:
1. Format a notification with the question text and options
2. Route via `NotificationRouter` using `EventType::Question` rules
3. For channels that support actions (Telegram inline keyboards, Slack Block Kit, Discord buttons): render options as clickable buttons
4. For channels that don't (email, SMS): include option numbers ("Reply 1 for red, 2 for blue, 3 for green")
5. When a response comes in via `IncomingMessage`, the dispatch handler calls `wg ask answer`

### Bidirectional channel integration

The `NotificationChannel` trait already supports:
- `send_with_actions()` — for rendering option buttons
- `listen()` → `Receiver<IncomingMessage>` — for receiving responses
- `IncomingMessage.action_id` — maps directly to selected option

The Telegram channel (already implemented with `teloxide`) supports inline keyboards, which are perfect for multiple-choice questions.

---

## 5. Per-Executor Adapters

### Claude Code executor (current default)

Two strategies, in order of preference:

**A. Prompt-based (no code change):** Add to the agent system prompt:
```
If you need human input, use: wg ask <task-id> "question" --options "opt1,opt2"
Do NOT use AskUserQuestion — it doesn't work in non-interactive mode.
```
This is the simplest approach and works today once `wg ask` exists.

**B. Stream interception:** In the Claude JSONL stream translator (`translate_claude_event`), detect `AskUserQuestion` tool calls and automatically convert them to `wg ask`. This is a safety net for cases where the model ignores the prompt instruction.

### Native executor (OpenRouter/API)

Add a `wg_ask` tool to the `ToolRegistry`:

```rust
// In src/executor/native/tools.rs
fn handle_wg_ask(input: &Value) -> ToolResult {
    let task_id = input["task_id"].as_str()?;
    let question = input["question"].as_str()?;
    let options = input["options"].as_array()?;

    // Shell out to: wg ask <task-id> "question" --options "..."
    // This blocks until answered
    let output = Command::new("wg").args(["ask", task_id, question, ...]).output()?;

    ToolResult { content: String::from_utf8(output.stdout)?, is_error: !output.status.success() }
}
```

The LLM sees `wg_ask` in its tool list and can call it naturally. The tool handler runs `wg ask` which blocks until the human responds.

### Amplifier executor

Amplifier's `ApprovalSystem` already handles blocking approval flows. Map `wg ask` to amplifier's approval:
- Question → approval request
- Options → approval choices
- Answer → approval result

---

## 6. Agent Parking / Blocking

When an agent calls `wg ask` and blocks, the coordinator needs to know not to kill it for inactivity.

### Stream-based liveness

Add a new `StreamEvent` variant:

```rust
pub enum StreamEvent {
    // ... existing variants ...
    WaitingForInput {
        question_id: String,
        timestamp_ms: i64,
    },
}
```

While blocked, `wg ask` emits periodic `Heartbeat` events (every 30s) to the stream file, preventing the coordinator's stale-detection from killing the agent.

### Coordinator awareness

The coordinator's `AgentStreamState` already tracks `current_tool`. When it sees `WaitingForInput`, it can:
1. Skip stale-detection for this agent
2. Display "waiting for human input" in `wg agents` output
3. Optionally re-notify if no answer comes within escalation timeout

### Alternative: park and resume

Instead of blocking the agent process:
1. Agent calls `wg ask --no-wait` to post the question
2. Agent calls `wg done <task-id> --parked` (new status: parked)
3. Coordinator skips parked tasks
4. When human answers, a trigger re-opens the task (status → ready)
5. Coordinator spawns a new agent with the question answer in context

This avoids keeping an idle process alive but requires session resumption or fresh-start with context. For cost efficiency with expensive models, this is the better approach.

---

## 7. Summary of Recommendations

| Priority | Item | Effort |
|---|---|---|
| **P0** | Add `wg ask` / `wg ask answer` commands | Medium |
| **P0** | Add `EventType::Question` to notification system | Small |
| **P0** | Prompt agents to use `wg ask` instead of `AskUserQuestion` | Trivial |
| **P1** | Add `wg_ask` tool to native executor tool registry | Small |
| **P1** | Wire Telegram inline keyboards to `wg ask answer` | Medium |
| **P2** | Stream interception for Claude AskUserQuestion → `wg ask` | Medium |
| **P2** | Park-and-resume flow for cost efficiency | Large |
| **P3** | `WaitingForInput` stream event + coordinator awareness | Medium |

### Key design principles

1. **Executor-agnostic:** `wg ask` is a CLI command any executor can call
2. **Channel-agnostic:** Questions route through `NotificationRouter` like any other event
3. **Blocking or async:** Support both `wg ask` (blocking) and `wg ask --no-wait` (async)
4. **Human-friendly:** Options render as buttons on platforms that support them
5. **Graceful timeout:** Questions expire with configurable timeouts; agents get an error they can handle

---

## 8. Open Questions

1. **Should unanswered questions block task completion?** If an agent asks a question and then proceeds without waiting, should `wg done` succeed?
2. **Multiple questions per task?** Design supports it (each question has its own ID), but should we limit to one active question per task?
3. **Question persistence across agent restarts?** If an agent dies and is restarted, should it see pending questions and their answers?
4. **Priority/urgency of questions?** Should agents be able to mark questions as urgent (affecting notification routing)?
