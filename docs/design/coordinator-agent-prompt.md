# Coordinator Agent Prompt Design

## Status: Design (March 2026)

## Overview

This document specifies the system prompt, dynamic context injection, context pruning strategy, and tool definitions for the persistent coordinator agent. The coordinator agent is a long-lived LLM session inside the service daemon that interprets user intent, creates and manages tasks, monitors agents, and reports status.

**Audience**: `sh-impl-coordinator-agent` (the implementation task that will wire this into the daemon).

## 1. System Prompt

The system prompt is injected once at the start of the coordinator agent's session. It is static — it does not change between messages. Dynamic state goes through the context injection (Section 2).

### 1.1 Draft System Prompt

```
You are the workgraph coordinator — the persistent intelligence that manages a task graph.

## Your Role

You interpret user requests, manage the task graph, monitor agent progress, and report status. You are the bridge between the user's intent and the graph's execution.

You do NOT implement code. You do NOT write files. You do NOT run tests. You orchestrate: you create tasks, set dependencies, assign priorities, monitor agents, and communicate with the user.

## How the System Works

workgraph is a task orchestration system built on stigmergic coordination:
- **Tasks** form a directed graph with dependency edges (`--after`). Tasks can form cycles.
- **Agents** are spawned by the coordinator daemon to work on ready tasks (all dependencies met).
- **The graph is the coordination medium** — agents discover work by reading it, create work by writing to it. You dispatch agents; they self-organize through the graph.
- **You are persistent** — you maintain conversational context across messages, unlike task agents that are spawned for a single task and exit.

When a user asks you to do something, your job is to translate that into graph operations:
- Create tasks with clear descriptions and correct dependencies
- Group related work with shared prefixes (e.g., `auth-research`, `auth-impl`, `auth-test`)
- Set appropriate dependency chains — sequential for shared-file work, parallel for independent work
- Monitor progress and report back when asked

## Available Tools

You have access to these workgraph CLI commands via the `bash` tool:

### Task Management
- `wg add "title" [-d "description"] [--after dep1,dep2] [--tag tag1,tag2] [--skill skill1]` — Create a task
- `wg edit <task-id> [--title "new"] [--description "new"] [--after dep1,dep2] [--tag tag1]` — Modify a task
- `wg done <task-id>` — Mark a task complete
- `wg fail <task-id> --reason "why"` — Mark a task as failed
- `wg retry <task-id>` — Retry a failed task
- `wg pause <task-id>` / `wg resume <task-id>` — Pause/resume a task
- `wg abandon <task-id>` — Permanently abandon a task

### Inspection
- `wg show <task-id>` — Full task details (description, status, logs, artifacts, deps)
- `wg list [--status open|in-progress|done|failed|blocked]` — List tasks with optional filter
- `wg status` — One-screen project overview
- `wg ready` — List tasks ready to be worked on
- `wg blocked <task-id>` — Show what blocks a task
- `wg why-blocked <task-id>` — Transitive blocking chain
- `wg context <task-id>` — Show dependency context for a task
- `wg impact <task-id>` — What depends on this task

### Agent Management
- `wg agents` — List running agents and their tasks
- `wg kill <agent-id>` — Kill a running agent
- `wg unclaim <task-id>` — Release a claimed task

### Communication
- `wg msg send <task-id> "message"` — Send a message to a task's agent
- `wg msg list <task-id>` — View messages on a task
- `wg log <task-id> "note"` — Add a log entry to a task

### Analysis
- `wg critical-path` — Longest dependency chain
- `wg bottlenecks` — Tasks blocking the most downstream work
- `wg velocity` — Task completion rate
- `wg forecast` — Projected completion date
- `wg coordinate` — Ready tasks, in-progress tasks, parallelism opportunities

### Service Control
- `wg service status` — Service daemon status
- `wg service pause` / `wg service resume` — Pause/resume agent spawning

## Behavioral Rules

1. **Never implement** — You NEVER write code, modify source files, run builds, or execute tests. If the user asks you to "do" something that involves writing code, create a task for it.

2. **Decompose intelligently** — Break user requests into the right granularity:
   - Small request ("fix the typo in README") → single task
   - Medium request ("add JWT auth") → 3-5 tasks (research, implement, test, integrate)
   - Large request ("build a new microservice") → plan phase + implementation phase with fan-out

3. **Respect the golden rule** — Tasks that modify the same files MUST be sequential (pipeline), not parallel. When unsure, default to sequential.

4. **Include integration points** — When fanning out parallel work, always add an integrator task that depends on all parallel branches: `wg add "Integrate X" --after branch-a,branch-b,branch-c`

5. **Give clear descriptions** — Each task description should tell the agent exactly what to do, what files to touch, and what "done" looks like. Agents cannot ask you clarifying questions mid-task.

6. **Use the graph for status** — When the user asks "what's happening?", inspect the graph (`wg status`, `wg agents`, `wg list`) rather than guessing from memory.

7. **Report concisely** — Summarize graph state in human-readable form. Don't dump raw command output unless the user asks for details.

8. **Be conversational** — You're a collaborator, not a command parser. Understand intent, ask clarifying questions when ambiguous, and suggest approaches.

## Common Patterns

### User: "I need to implement X"
1. Clarify scope if ambiguous
2. Create a research/design task (if X is non-trivial)
3. Create implementation task(s) with `--after` the research task
4. Create test task(s) with `--after` the implementation task(s)
5. Report what you created and the expected flow

### User: "What's the status?"
1. Run `wg status` for overview
2. Run `wg agents` if agents are active
3. Run `wg list --status failed` if there are failures
4. Summarize in natural language

### User: "Why is task X stuck?"
1. Run `wg show <task-id>` for current state
2. Run `wg why-blocked <task-id>` for blocking chain
3. Check if blocking tasks have agents assigned
4. Suggest resolution (retry, unblock, reprioritize)

### User: "Retry the failed tasks"
1. Run `wg list --status failed` to identify failures
2. For each, check logs (`wg show <id>`) to understand why
3. If retriable, `wg retry <id>`. If systemic, explain the pattern.

### User: "Pause everything / I need to make manual changes"
1. `wg service pause` to stop new agent spawns
2. Explain what's currently in-progress (agents will finish)
3. Wait for user to say resume, then `wg service resume`

## Context You Receive

On each message, you receive a system context update (injected automatically) with:
- Graph summary: task count by status
- Recent events: completions, failures, new tasks since your last message
- Active agents: who's working on what
- Pending items: failed/blocked tasks that may need attention

Use this context to stay oriented. You do NOT need to run `wg status` on every message — only when you need more detail than the summary provides.
```

### 1.2 Design Rationale

| Decision | Rationale |
|----------|-----------|
| "Never implement" hard constraint | Prevents the coordinator from becoming a bottleneck. Task agents are cheaper, parallelizable, and disposable. |
| Explicit tool list with exact CLI syntax | Reduces hallucinated flags. The coordinator uses `bash` to call `wg` commands, not custom tool_use functions (Section 4 discusses both options). |
| Common patterns section | Few-shot examples calibrate the coordinator's response style better than abstract rules. |
| "Be conversational" rule | The coordinator is a user-facing agent, unlike task agents that are autonomous workers. |
| Context update reference | Tells the coordinator that dynamic context is injected so it doesn't re-fetch redundantly. |

## 2. Context Injection on Wake-Up

Every time the coordinator processes a new user message, a **System Context Update** is prepended to the user message (or injected as a separate system message, depending on the API interface).

### 2.1 Template

```
## System Context Update ({{timestamp}})

### Graph Summary
{{total}} tasks: {{done}} done, {{in_progress}} in-progress, {{open}} open, {{blocked}} blocked, {{failed}} failed, {{abandoned}} abandoned

### Recent Events (since last interaction at {{last_interaction_time}})
{{#each events}}
- [{{timestamp}}] {{description}}
{{/each}}
{{#if no_events}}
- No events since last interaction.
{{/if}}

### Active Agents
{{#each agents}}
- {{agent_id}} working on {{task_id}} "{{task_title}}" (uptime: {{uptime}})
{{/each}}
{{#if no_agents}}
- No active agents.
{{/if}}

### Attention Needed
{{#each failed_tasks}}
- FAILED: {{task_id}} "{{task_title}}" — {{failure_reason}}
{{/each}}
{{#each stale_agents}}
- STALE: {{agent_id}} on {{task_id}} (no heartbeat for {{stale_duration}})
{{/each}}
{{#if no_attention}}
- Nothing requires attention.
{{/if}}
```

### 2.2 Data Sources

| Field | Source | How to compute |
|-------|--------|---------------|
| Graph summary counts | `load_graph()` + iterate tasks by status | O(n) scan, always cheap |
| Recent events | Task log entries with `timestamp > last_interaction` | Filter `LogEntry` timestamps across all tasks |
| Active agents | `AgentRegistry::load()` + `is_process_alive()` | Already computed in `cleanup_and_count_alive()` |
| Failed tasks | Graph tasks with `Status::Failed` | O(n) scan |
| Stale agents | Registry agents where `last_heartbeat` is old | Compare against configurable threshold (default 5min) |
| Last interaction time | Coordinator cursor → inbox message timestamp | Read from `.coordinator-cursor` + look up inbox message |

### 2.3 Implementation Sketch (Rust)

```rust
/// Build the context injection string for the coordinator agent.
pub fn build_coordinator_context(
    dir: &Path,
    last_interaction: &str,  // ISO 8601 timestamp of last processed message
) -> Result<String> {
    let graph_path = graph_path(dir);
    let graph = load_graph(&graph_path)?;
    let registry = AgentRegistry::load(dir)?;

    // --- Graph Summary ---
    let mut done = 0; let mut in_progress = 0; let mut open = 0;
    let mut blocked = 0; let mut failed = 0; let mut abandoned = 0;
    for task in graph.tasks() {
        match task.status {
            Status::Done => done += 1,
            Status::InProgress => in_progress += 1,
            Status::Open => {
                if task_is_blocked(&graph, &task.id) { blocked += 1; }
                else { open += 1; }
            }
            Status::Failed => failed += 1,
            Status::Abandoned => abandoned += 1,
            _ => {}
        }
    }
    let total = done + in_progress + open + blocked + failed + abandoned;

    // --- Recent Events ---
    let events = collect_recent_events(&graph, last_interaction)?;

    // --- Active Agents ---
    let alive_agents: Vec<_> = registry.agents.values()
        .filter(|a| a.is_alive() && is_process_alive(a.pid))
        .collect();

    // --- Attention Needed ---
    let failed_tasks: Vec<_> = graph.tasks()
        .filter(|t| t.status == Status::Failed)
        .collect();

    // Format into the template
    format_coordinator_context(total, done, in_progress, open, blocked, failed, abandoned,
                               &events, &alive_agents, &failed_tasks, last_interaction)
}
```

### 2.4 Estimated Token Cost

| Component | Typical size | Tokens (~) |
|-----------|-------------|-----------|
| Graph summary | 1 line | ~30 |
| Recent events (5 events) | 5 lines | ~100 |
| Active agents (3 agents) | 3 lines | ~60 |
| Attention needed (2 items) | 2 lines | ~50 |
| **Total context injection** | | **~250 tokens** |

At 100 tasks, 10 agents, and 20 recent events, this grows to ~600 tokens. The injection is compact by design — it's a summary, not a dump.

## 3. Context Pruning Strategy

The coordinator agent runs as a persistent session. Over time, conversation history grows and may approach LLM context window limits. This section specifies what to keep, what to prune, and how.

### 3.1 Context Budget

Assume a 200k token window (Claude Sonnet/Opus). Budget allocation:

| Category | Budget | Notes |
|----------|--------|-------|
| System prompt | ~2,000 tokens | Static, never pruned |
| Context injection | ~300-600 tokens/message | Grows linearly with graph size |
| Conversation history | ~180,000 tokens | Sliding window of user messages + responses |
| Tool call results | Included in conversation history | Often the largest portion |
| **Safety margin** | ~17,000 tokens | Buffer for response generation |

### 3.2 Pruning Rules

**Rule 1: System prompt is immutable.** Never prune it.

**Rule 2: Context injection is ephemeral.** Each injection replaces the previous one. Old injections in conversation history can be summarized or removed.

**Rule 3: Conversation history uses a sliding window.**
- Keep the last N user/assistant message pairs (default N=50).
- When a message pair is evicted, generate a one-line summary and add it to a "Session Summary" that persists at the top of the conversation.
- This summary captures: tasks created, key decisions made, user preferences expressed.

**Rule 4: Tool call results are aggressive prune targets.**
- `wg list` output (potentially hundreds of tasks) → keep only if referenced in the response
- `wg show` output → keep only the task ID and key fields
- `wg status` output → keep the summary line only
- After the coordinator has processed a tool result and responded, the detailed output can be replaced with a summary: `[wg show task-x: status=in-progress, assigned to agent-3]`

**Rule 5: Full task detail on demand only.**
- The context injection provides aggregate summaries.
- Full task details (`wg show`) are fetched only when the user asks about a specific task.
- After the coordinator responds, the tool result is pruned to a one-line summary.

### 3.3 Pruning Implementation

There are two strategies depending on the coordinator's execution model:

**Strategy A: Claude CLI with `--resume` (v1)**

Claude CLI manages its own context internally. The coordinator daemon cannot directly prune context. Instead:
- Limit context injection size (always compact summaries)
- Restart the session with a summary injection when approaching limits
- Detection: track approximate token count from response metadata

When restarting:
```
## Session Recovery

You are the workgraph coordinator resuming after a context rotation.

### Previous Session Summary
{{summary_of_previous_session}}

### Current State
{{current_context_injection}}
```

**Strategy B: Native executor with API control (v2)**

Direct API access means we control the messages array:
- Track message sizes (approximate tokens)
- When total exceeds threshold (e.g., 150k tokens), prune:
  1. Replace oldest tool results with summaries
  2. Evict oldest message pairs, adding their summaries to the session summary
  3. Never touch system prompt or most recent 10 message pairs

### 3.4 Session Summary Format

The session summary accumulates key facts from pruned conversation:

```
## Session Summary (auto-generated, do not modify)

Tasks created this session: auth-research, auth-impl, auth-test, auth-integrate
Key decisions:
- User prefers JWT over session tokens for auth
- Rate limiting should use sliding window algorithm
- Test coverage target: 80%
User preferences:
- Prefers small, focused tasks over large batches
- Wants status updates after each completion wave
Recent failures: none
```

This is generated by the coordinator itself (via a tool call or injected system prompt instruction) before the context rotation.

## 4. Tool Definitions

The coordinator agent needs tools to interact with the workgraph. There are two design options for how these tools are exposed.

### 4.1 Option A: Bash Tool Only (Recommended for v1)

The coordinator uses a single `bash` tool to execute `wg` CLI commands. This is the simplest approach and reuses the existing CLI surface.

```json
{
  "name": "bash",
  "description": "Execute a shell command. Use this to run wg commands for task management, inspection, and agent control. Common commands: wg add, wg show, wg list, wg status, wg agents, wg edit, wg done, wg fail, wg retry, wg msg send.",
  "input_schema": {
    "type": "object",
    "properties": {
      "command": {
        "type": "string",
        "description": "The shell command to execute. Prefer wg commands for graph operations."
      }
    },
    "required": ["command"]
  }
}
```

**Advantages**: Zero new code for tool definitions. Full CLI surface available. Easy to add new wg commands without updating tool schemas. Matches how task agents already work.

**Disadvantages**: Shell overhead per command. Risk of hallucinated flags. No structured output validation.

**Mitigation for hallucinated flags**: The system prompt lists exact command syntax. The coordinator can also run `wg <command> --help` if unsure.

### 4.2 Option B: Typed Tool Definitions (v2, native executor)

When the native executor is available, the coordinator gets typed tools that call workgraph library functions directly (no CLI subprocess). This is faster and safer.

```json
[
  {
    "name": "wg_add",
    "description": "Create a new task in the workgraph.",
    "input_schema": {
      "type": "object",
      "properties": {
        "title": {
          "type": "string",
          "description": "Task title (short, descriptive)"
        },
        "description": {
          "type": "string",
          "description": "Detailed task description (what to do, what files to touch, what done looks like)"
        },
        "after": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Task IDs that this task depends on (blocks until they complete)"
        },
        "tags": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Tags for categorization (e.g., 'auth', 'frontend', 'bugfix')"
        },
        "skills": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Required skills for agent matching (e.g., 'rust', 'system-design')"
        }
      },
      "required": ["title"]
    }
  },
  {
    "name": "wg_show",
    "description": "Show detailed information about a task: description, status, logs, artifacts, dependencies.",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task ID to inspect"
        }
      },
      "required": ["task_id"]
    }
  },
  {
    "name": "wg_list",
    "description": "List tasks with optional status filter.",
    "input_schema": {
      "type": "object",
      "properties": {
        "status": {
          "type": "string",
          "enum": ["open", "in-progress", "done", "failed", "blocked", "abandoned", "paused"],
          "description": "Filter by task status. Omit for all tasks."
        }
      }
    }
  },
  {
    "name": "wg_status",
    "description": "Show a one-screen project overview with task counts by status, active agents, and recent completions.",
    "input_schema": {
      "type": "object",
      "properties": {}
    }
  },
  {
    "name": "wg_edit",
    "description": "Modify an existing task's properties.",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task ID to edit"
        },
        "title": {
          "type": "string",
          "description": "New title (optional)"
        },
        "description": {
          "type": "string",
          "description": "New description (optional)"
        },
        "after": {
          "type": "array",
          "items": { "type": "string" },
          "description": "New dependency list (replaces existing)"
        },
        "tags": {
          "type": "array",
          "items": { "type": "string" },
          "description": "New tags (replaces existing)"
        }
      },
      "required": ["task_id"]
    }
  },
  {
    "name": "wg_done",
    "description": "Mark a task as completed.",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task ID to mark done"
        },
        "converged": {
          "type": "boolean",
          "description": "If true, marks the cycle as converged (stops iteration). Use for cycle header tasks."
        }
      },
      "required": ["task_id"]
    }
  },
  {
    "name": "wg_fail",
    "description": "Mark a task as failed.",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task ID to mark failed"
        },
        "reason": {
          "type": "string",
          "description": "Why the task failed"
        }
      },
      "required": ["task_id", "reason"]
    }
  },
  {
    "name": "wg_retry",
    "description": "Retry a failed task (resets to open status).",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task ID to retry"
        }
      },
      "required": ["task_id"]
    }
  },
  {
    "name": "wg_agents",
    "description": "List running agents and their current tasks.",
    "input_schema": {
      "type": "object",
      "properties": {}
    }
  },
  {
    "name": "wg_msg_send",
    "description": "Send a message to a task's agent.",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task whose agent should receive the message"
        },
        "message": {
          "type": "string",
          "description": "The message content"
        }
      },
      "required": ["task_id", "message"]
    }
  },
  {
    "name": "wg_kill",
    "description": "Kill a running agent.",
    "input_schema": {
      "type": "object",
      "properties": {
        "agent_id": {
          "type": "string",
          "description": "The agent ID to kill"
        }
      },
      "required": ["agent_id"]
    }
  },
  {
    "name": "wg_critical_path",
    "description": "Show the critical path — the longest dependency chain in the graph.",
    "input_schema": {
      "type": "object",
      "properties": {}
    }
  },
  {
    "name": "wg_bottlenecks",
    "description": "Show tasks blocking the most downstream work.",
    "input_schema": {
      "type": "object",
      "properties": {}
    }
  },
  {
    "name": "wg_impact",
    "description": "Show what tasks transitively depend on a given task.",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task ID to analyze"
        }
      },
      "required": ["task_id"]
    }
  },
  {
    "name": "wg_blocked",
    "description": "Show what's blocking a task.",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task ID to check"
        }
      },
      "required": ["task_id"]
    }
  },
  {
    "name": "wg_why_blocked",
    "description": "Show the full transitive chain explaining why a task is blocked.",
    "input_schema": {
      "type": "object",
      "properties": {
        "task_id": {
          "type": "string",
          "description": "The task ID to trace"
        }
      },
      "required": ["task_id"]
    }
  },
  {
    "name": "wg_ready",
    "description": "List tasks that are ready to work on (all dependencies met).",
    "input_schema": {
      "type": "object",
      "properties": {}
    }
  },
  {
    "name": "wg_service_pause",
    "description": "Pause agent spawning. Active agents continue but no new ones are started.",
    "input_schema": {
      "type": "object",
      "properties": {}
    }
  },
  {
    "name": "wg_service_resume",
    "description": "Resume agent spawning after a pause.",
    "input_schema": {
      "type": "object",
      "properties": {}
    }
  },
  {
    "name": "chat_respond",
    "description": "Send a response back to the user. This is how you communicate with the user. Your text response will be displayed in their CLI or TUI chat panel.",
    "input_schema": {
      "type": "object",
      "properties": {
        "message": {
          "type": "string",
          "description": "Your response to the user"
        },
        "request_id": {
          "type": "string",
          "description": "The request_id from the user's message (for correlation)"
        }
      },
      "required": ["message", "request_id"]
    }
  }
]
```

### 4.3 Recommendation

**v1 (Claude CLI)**: Use Option A (bash tool only). The coordinator gets a `bash` tool and calls `wg` commands. This is identical to how task agents work and requires no new tool infrastructure.

**v2 (native executor)**: Use Option B (typed tools). Each tool maps to an in-process function call. The `chat_respond` tool is the coordinator's way to send responses back through the outbox.

**Hybrid consideration**: Even in v1, the coordinator needs a way to send its response back to the user. Two options:
1. **Implicit**: The coordinator's final text output (not in a tool call) is captured as the response. The daemon wraps the stream-json output and writes it to the outbox.
2. **Explicit**: Add a minimal `chat_respond` bash wrapper: `wg chat respond --request-id <id> "message"`. The coordinator calls this to send its response.

**Recommendation**: Use implicit capture (option 1) for v1. The daemon already captures agent output. The coordinator's text output becomes the chat response. This avoids adding a new CLI command and feels more natural — the coordinator just "says" things.

### 4.4 Tool Restriction

The coordinator agent should NOT have access to:
- File write/edit tools (it doesn't implement code)
- `wg done`/`wg fail` on tasks it didn't create (it orchestrates, it doesn't do the work)
- `cargo`, `npm`, or other build tools
- Network access beyond what `wg` commands provide

In v1 (bash tool), this is enforced by the system prompt ("you never write code"). In v2 (typed tools), this is enforced by the tool set — only the tools listed above are available.

## 5. Response Delivery

### 5.1 v1: Implicit Capture (Claude CLI)

The coordinator agent is a Claude CLI session running with `--output-format stream-json`. The daemon reads stdout, captures the assistant's text blocks, and writes them to the outbox.

```
Coordinator stdout (stream-json):
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll create..."}]}}

Daemon captures:
- text = "I'll create..."
- request_id = (from the inbox message that triggered this response)
- chat::append_outbox(dir, &text, &request_id)
```

The request_id is tracked by the daemon: when it reads an inbox message and injects it into the coordinator's stdin, it records the request_id. When the coordinator responds, the daemon uses that request_id for the outbox entry.

### 5.2 v2: Explicit Tool Call (Native Executor)

The coordinator calls `chat_respond` with the message and request_id. The tool implementation calls `chat::append_outbox()` directly.

### 5.3 Multi-Turn Responses

Sometimes the coordinator needs to call multiple tools before responding (e.g., `wg list` then `wg show` then respond). In both v1 and v2, the coordinator does tool calls, gets results, and then produces a final text response. Only the final text response is captured as the chat response.

If the coordinator wants to send an intermediate status ("Let me check..."), it can do so:
- v1: intermediate text blocks are captured. The daemon could either send each text block as a separate outbox message, or buffer and send only the final one. **Recommendation**: Buffer and send only the final text block. This prevents partial/confusing responses.
- v2: the coordinator calls `chat_respond` once with its final response.

## 6. Message Injection Format

### 6.1 v1: Stream-JSON Stdin Injection

When a user message arrives, the daemon injects it into the coordinator's stdin:

```json
{"type":"user","message":{"role":"user","content":"help me plan the auth system"}}
```

The context injection is prepended to the user content:

```json
{
  "type": "user",
  "message": {
    "role": "user",
    "content": "## System Context Update (2026-03-02T10:00:00Z)\n\n### Graph Summary\n45 tasks: 30 done, 5 in-progress, 8 open, 2 blocked, 0 failed\n\n### Recent Events\n- [09:58:32] task-alpha completed\n- [09:59:01] agent-3 spawned on task-beta\n\n### Active Agents\n- agent-1 on task-gamma (5m)\n- agent-2 on task-delta (12m)\n- agent-3 on task-beta (1m)\n\n### Attention Needed\n- Nothing requires attention.\n\n---\n\nUser message:\nhelp me plan the auth system"
  }
}
```

The `---` separator clearly delineates system context from user message, and the "User message:" label helps the coordinator distinguish them.

### 6.2 v2: Separate System and User Messages

With direct API control, inject context as a system message and the user's text as a user message:

```json
[
  {"role": "system", "content": "## System Context Update..."},
  {"role": "user", "content": "help me plan the auth system"}
]
```

This is cleaner but requires API-level message manipulation.

## 7. Error Handling

### 7.1 Coordinator Agent Crashes

If the Claude CLI process exits unexpectedly:
1. Daemon detects exit (process no longer alive)
2. Saves conversation summary from the last known state
3. Restarts the coordinator with the system prompt + session recovery summary + fresh context injection
4. Any pending inbox messages are processed on restart

### 7.2 Tool Call Failures

If a `wg` command fails (e.g., `wg show nonexistent-task`):
- v1: The bash tool returns stderr. The coordinator sees the error and adjusts.
- v2: The tool returns an error result. The coordinator handles it.

The system prompt should instruct: "If a tool call fails, tell the user what happened and suggest an alternative."

### 7.3 Inbox Message During Processing

If a new inbox message arrives while the coordinator is processing a previous one:
- v1 (Claude CLI): The daemon queues the message. After the coordinator finishes responding to the current message, the daemon injects the next one.
- v2 (native executor): Same behavior — messages are processed sequentially from the inbox.

The coordinator processes one message at a time. Concurrent user messages are queued in the inbox and processed in order.

## 8. File Summary

### New files
| File | Purpose |
|------|---------|
| `docs/design/coordinator-agent-prompt.md` | This document |

### Files to create during implementation (`sh-impl-coordinator-agent`)
| File | Purpose |
|------|---------|
| `src/service/coordinator_agent.rs` | Coordinator agent lifecycle: spawn, inject messages, capture responses |
| `src/service/coordinator_context.rs` | `build_coordinator_context()` function |

### Files to modify during implementation
| File | Change |
|------|--------|
| `src/commands/service/mod.rs` | Spawn coordinator agent on service start; message injection loop |
| `src/commands/service/coordinator.rs` | Delegate chat processing to coordinator agent instead of stub response |

## 9. Design Decisions Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Static system prompt vs dynamic | Static with dynamic context injection | System prompt is the agent's identity; context injection provides real-time state. Separation keeps each simple. |
| Bash tool vs typed tools for v1 | Bash tool | Zero new infrastructure. Matches task agent pattern. Full CLI surface available. |
| Implicit response capture vs explicit | Implicit for v1, explicit for v2 | v1: simpler, no new CLI command. v2: cleaner tool-call semantics. |
| Context injection in user message vs system message | User message for v1, system message for v2 | v1: Claude CLI stream-json only supports user messages. v2: API supports system messages. |
| Buffer multi-turn responses | Yes, send only final text block | Prevents confusing partial responses. User sees one coherent answer. |
| Conversation pruning: sliding window | 50 message pairs with summary accumulation | Balances context retention with window limits. Summary preserves key decisions. |
| Sequential message processing | Yes, one at a time | Avoids race conditions in tool calls. Inbox ordering provides fairness. |
| Tool restrictions: prompt vs enforcement | Prompt for v1, tool set for v2 | v1 can't restrict bash usage programmatically. v2 typed tools are inherently restricted. |

## 10. Open Questions

1. **Should the coordinator have a persistent task in the graph?** Making it a task gives it logs, artifacts, and visibility. But it never completes, which is semantically unusual. Resolution: defer to implementation — try it as a hidden task first.

2. **How to handle multi-paragraph responses?** If the coordinator produces a long response, should it be chunked for TUI display or sent as one block? Resolution: send as one block; TUI handles line wrapping and scrolling.

3. **Should the context injection include the user's previous messages?** The LLM already has them in its conversation history. Including them in the injection would be redundant. Resolution: no, only include graph state. The LLM's own memory handles conversation continuity.

4. **Rate limiting**: Should the coordinator ignore rapid-fire messages? Resolution: no, process them all in order. The inbox queue handles backpressure naturally. If the coordinator falls behind, the user sees increasing latency — which is the appropriate signal.
