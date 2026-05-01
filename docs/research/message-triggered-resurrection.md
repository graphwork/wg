# Research: Message-Triggered Resurrection of Completed Agents

**Researcher:** A1 (Committee v2, Round 2)
**Date:** 2026-03-04
**Context:** Extends `docs/design/liveness-detection.md` Phase 4 (Parking & Resume)

## 1. Claude `--resume` Mechanics

### How it works
- `claude --resume <session-id>` (`-r`) resumes a **specific** session by ID
- The full conversation context is preserved **server-side** — no local replay needed
- Session ID is captured from the first `system` event in `stream.jsonl` (the `Init` event with `session_id` field)
- **Critical constraint**: `--resume` only works **after** the previous process has exited. You cannot resume a session currently in use by a running process.

### Session TTL
- Claude session persistence is server-side. Exact TTL is not publicly documented but empirical evidence suggests sessions survive for **hours to days** (not weeks).
- **Current blocker**: Workgraph spawns agents with `--no-session-persistence` (see `src/commands/spawn/execution.rs:401,437,466`). This flag **must be removed** (or made configurable) to enable `--resume` for resurrection.
- Sessions created with `--no-session-persistence` are ephemeral and **cannot** be resumed.

### Context preservation
- Server-side context includes the full conversation history: all user messages, assistant messages, tool calls, and tool results.
- The resumed session continues from where it left off — the agent sees all prior context as if it never stopped.
- **Cost**: Resuming is essentially free (zero replay tokens). The agent only pays for new generation.

## 2. Detection Flow: How the Coordinator Detects Unread Messages on Completed Tasks

### Current message system
Messages are stored in `.wg/messages/{task-id}.jsonl`. Each message has a monotonic ID, timestamp, sender, body, and delivery status (Sent → Delivered → Read → Acknowledged). Read cursors per agent are in `.wg/messages/.cursors/{agent-id}.{task-id}`.

### Proposed detection mechanism
The coordinator poll loop (`src/commands/service/coordinator.rs`) already runs on each tick. Add a check:

1. **For each task with status `Done`**: Check if there are messages with `status == Sent` (unread/undelivered) that arrived **after** the task's `completed_at` timestamp.
2. **If unread post-completion messages exist**: The task is a candidate for resurrection.
3. **Rate limit**: Only check tasks completed in the last N hours (configurable, e.g., 24h) to avoid scanning the entire history.

### What triggers resurrection
- A human or another agent sends a message via `wg msg send <task-id> "..."` to a task that's already `Done`.
- The coordinator detects the unread message on its next poll tick.
- The coordinator transitions the task and spawns a resumed agent.

## 3. State Transitions

### Current task states
From `src/graph.rs`: `Open`, `InProgress`, `Done`, `Blocked`, `Failed`, `Abandoned` (all terminal: Done, Failed, Abandoned).

### Proposed resurrection flow

```
Done → [message arrives] → InProgress → Done (again)
```

**Detailed sequence:**

1. Task is `Done`. Agent exited. `session_id` stored in stream.jsonl Init event (or agent registry).
2. New message arrives at `.wg/messages/{task-id}.jsonl`.
3. Coordinator detects unread message on Done task (next poll tick).
4. Coordinator sets task status to `InProgress`, sets `assigned` to new agent ID.
5. Coordinator spawns agent with `claude --resume <session-id> -p "New message: <body>"`.
6. Agent processes the message, does work, runs `wg done`.
7. Task returns to `Done`.

### Do evaluations need to re-run?
**Yes, conditionally.** If the resurrection results in code changes (new commits), the evaluation should re-run. The existing `eval-scheduled` tag mechanism can handle this:
- After resurrection completes (task → Done again), if there are new artifacts, schedule a new eval.
- If the resurrection was a simple acknowledgment (no code changes), skip re-eval.
- Heuristic: check if `git diff` shows changes in any of the task's artifact files.

### Alternative: New `Resuming` state
Instead of directly going to `InProgress`, a new transient state `Resuming` could signal that this is a resurrection (not a fresh start):

```
Done → Resuming → InProgress → Done
```

**Recommendation:** Don't add `Resuming`. It adds complexity for minimal benefit. The coordinator already knows it's resuming (it chose `--resume` over fresh spawn). A log entry is sufficient to record the resurrection event.

## 4. Edge Cases

### Session expired
- Try `--resume <session-id>` first.
- If it fails (non-zero exit, error in stream), fall back to **reincarnation**: spawn a fresh agent with the task description + a `## Previous Completion Context` section containing:
  - Original task description
  - Summary of what the previous agent did (from logs/artifacts)
  - The new message that triggered resurrection
- This is the "belt-and-suspenders" approach from the liveness-detection.md consensus.

### Task was abandoned
- `Abandoned` is terminal. Messages to abandoned tasks should **not** trigger resurrection.
- Coordinator should skip abandoned tasks in the resurrection check.
- If someone truly wants to resume an abandoned task, they should use `wg retry` first.

### Conflicting changes since completion
- Another agent may have modified files the original agent worked on.
- **Mitigation**: The resumed agent should be told about the time gap and instructed to check `git log` for recent changes to its artifact files before making modifications.
- Include in the resurrection prompt: "This task was previously completed at {completed_at}. You are being resumed due to a new message. Check for conflicting changes since your last session."

### Multiple messages queued
- If 3 messages arrive before the next coordinator tick, batch them into a single resurrection.
- The resurrection prompt includes all unread messages, not just the latest.
- This prevents spawning 3 agents for the same task.

### Rapid message-resurrection-message cycles
- If a task keeps getting messages and resurrecting, apply a cooldown (e.g., 60s between resurrections, matching the retry cooldown from liveness-detection.md).
- Track `resurrection_count` per task. After N resurrections (e.g., 5), require manual approval.

## 5. Non-Claude Executors

### The challenge
Non-Claude executors (OpenAI-compat, native Rust, shell) don't have server-side session persistence. Resurrection means **replaying** the conversation or injecting a summary.

### Approaches by executor type

| Executor | Resurrection Strategy | Cost | Quality |
|----------|----------------------|------|---------|
| Claude CLI | `--resume <session-id>` | ~$0 incremental | Full context preserved |
| OpenAI-compat | Replay full message history + new message | $0.50-$2.00 per resurrection | Good but expensive for long conversations |
| Native Rust | Inject checkpoint summary + new message | $0.05-$0.12 | Fresh but lossy |
| Shell | Re-run script with new env vars | ~$0 | No context preservation |

### OpenAI-compat detail
- OpenAI's API is stateless — every request must include the full conversation history.
- For a 50-turn conversation, replaying all messages could cost $1-2 in input tokens alone.
- **Optimization**: Save a checkpoint summary at task completion time. On resurrection, inject summary + recent turns + new message instead of full history.
- This trades context fidelity for cost. Acceptable for most use cases.

### Native executor detail
- The native executor (`src/executor/native/agent.rs`) maintains conversation state in memory.
- On resurrection, it would need to reconstruct state from logs/artifacts.
- Cheapest option: generate a "you previously completed this task, here's what you did" preamble and start fresh.

### Universal fallback
Always save a **checkpoint summary** when a task completes, regardless of executor. This provides a consistent resurrection path for all executors:

```
## Checkpoint (saved at completion)
- Task: {title}
- Completed: {timestamp}
- Key actions: {summary from logs}
- Artifacts: {list of files created/modified}
- Session ID: {if available}
```

## 6. Security

### The threat
Any message to a completed task triggers resurrection, which spawns an agent with write access to the codebase. This is a potential vector for:
- **Resource exhaustion**: Flood messages to trigger many resurrections.
- **Prompt injection**: Craft a message that manipulates the resurrected agent.
- **Unauthorized code changes**: A message from an untrusted source triggers code modifications.

### Proposed controls

**Tier 1 — Sender whitelist (recommended):**
- Only messages from `user`, `coordinator`, or agents working on **dependent tasks** (downstream consumers) should trigger resurrection.
- Messages from arbitrary agents or external sources (Matrix, etc.) should be logged but NOT trigger automatic resurrection.
- Implementation: Check `msg.sender` against an allowed list before triggering resurrection.

**Tier 2 — Rate limiting (recommended):**
- Max resurrections per task: configurable (default: 5).
- Cooldown between resurrections: configurable (default: 60s).
- Max concurrent resurrections globally: tied to `max_agents` limit.

**Tier 3 — Approval gate (optional, for high-security environments):**
- Resurrection of tasks tagged `security` or `critical` requires user approval via HITL notification.
- The coordinator sends a notification: "Task X received a post-completion message. Approve resurrection?"
- Only proceed if approved within a timeout.

**Tier 4 — Message content scanning (future):**
- Scan resurrection-triggering messages for prompt injection patterns.
- This is orthogonal to workgraph and belongs in the executor layer.

### Recommendation
Implement Tier 1 + Tier 2 for v1. Tier 3 is a config option for sensitive workflows. Tier 4 is out of scope.

## Summary of Key Design Decisions

| Decision | Recommendation | Rationale |
|----------|---------------|-----------|
| New task state? | No — reuse `InProgress` | Simplicity; coordinator knows it's resuming |
| Session persistence | Enable by default (remove `--no-session-persistence`) | Required for `--resume` |
| Detection trigger | Coordinator poll: check Done tasks for unread messages | Fits existing architecture |
| Session expired fallback | Checkpoint summary + fresh spawn | Belt-and-suspenders |
| Non-Claude executors | Checkpoint summary injection | Universal, cost-effective |
| Security | Sender whitelist + rate limit | Prevents abuse without blocking legitimate use |
| Multiple queued messages | Batch into single resurrection | Prevents duplicate agents |
| Re-evaluation | Only if artifacts changed | Avoids unnecessary eval cost |

## Open Questions for Committee Discussion

1. **Should `--no-session-persistence` removal be global or per-task?** Per-task gives finer control but adds config complexity.
2. **Should resurrection be opt-in (task tag like `resurrectable`) or opt-out?** Opt-in is safer for v1.
3. **What's the right max resurrection count?** 5 seems reasonable but depends on workflow patterns.
4. **Should the checkpoint summary be LLM-generated or template-based?** LLM-generated is higher quality but costs tokens at completion time.
