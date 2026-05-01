# Native Executor: Design Requirements for Terminal Bench Readiness

## Purpose

This document specifies patterns and improvements the native executor needs to reliably complete complex multi-turn tasks with open models. These are design requirements, not implementation details -- agents working on these tasks should implement them idiomatically within the existing codebase.

---

## 1. Mid-Turn State Injection

### Requirement

Before each API request, the agent loop should check for dynamic state changes and inject them as ephemeral context. This context is NOT persisted to the conversation history -- it's rebuilt fresh each turn, like a HUD overlay.

### What to inject

1. **Pending messages**: Check `wg msg read` for messages from other agents, the coordinator, or humans
2. **Graph state changes**: Dependent tasks completed, new tasks created, blockers resolved
3. **Context pressure warnings**: "You're at 78% context capacity, consider wrapping up"

### Pattern

```
Before each API call:
  1. Collect injections (messages, graph changes, context pressure)
  2. If any exist, prepend as a <context-update> block in the messages array
  3. Do NOT persist this block to the journal -- it's ephemeral
  4. The model sees it, reacts to it, but it doesn't accumulate
```

### Why this matters

This is how the coordinator communicates with running agents in real-time. It's how graph state changes become visible mid-task. It's how context pressure warnings reach the model before it hits the wall. Without this, agents are blind to their environment between the initial prompt and task completion.

### Advanced: Interrupt support

When a high-priority message arrives (e.g., `wg msg send --priority high`):
1. Complete the current tool execution
2. Inject the urgent message before the next API call
3. Let the model react to the interruption

---

## 2. Tiered Context Pressure Management

### Requirement

The agent loop must estimate context usage per turn and manage it through escalating interventions. Currently there is no context management -- the agent runs until the API returns 400.

### Four threshold bands

```
80% capacity → Warning injection
   "You're at 80% context. Consider logging progress via wg log
    and completing the current subtask."

90% capacity → Emergency compaction
   Drop tool results from turns older than the last 5.
   Replace with: "[Tool result removed. Tool: {name}, Input: {summary}]"
   Keep all text blocks (reasoning) -- smaller and more valuable.

95% capacity → Clean exit
   wg log <task-id> "Context limit reached. Progress: <summary>"
   Create continuation subtask if work remains.
   Exit gracefully (not crash).

API 400/413 → Error recovery
   Attempt emergency compaction, retry once.
   If still too long, clean exit with progress logged.
```

### Token estimation

Rough estimate: `total_chars / 4`. This is imprecise but sufficient for threshold detection. The exact number doesn't matter -- the thresholds are set conservatively enough that ±20% estimation error is fine.

### Critical detail

The context window size must come from the provider/model configuration, NOT be hardcoded. Different models have different context windows (e.g., 32K, 128K, 200K). The compaction budget in `resume.rs` currently assumes 200K -- this must be configurable.

```rust
struct ContextBudget {
    window_size: usize,       // From provider config
    chars_per_token: f64,     // 4.0 default
    warning_threshold: f64,   // 0.80
    compact_threshold: f64,   // 0.90
    hard_limit: f64,          // 0.95
}
```

---

## 3. Smart Tool Result Truncation

### Requirement

When a tool returns large output, don't send the full output to the model. Send a smart preview that preserves the most useful information.

### Pattern

```
If output.len() <= max_chars:
    return output as-is

Otherwise:
    head = first (max_chars / 2) chars
    tail = last (max_chars / 2) chars
    omitted = output.len() - max_chars
    
    return:
    "{head}

    [... {omitted} characters omitted ({line_count} lines).
     Showing first and last {max_chars/2} chars.
     Use read_file or grep for specific content. ...]

    {tail}"
```

### Per-tool limits

| Tool | Max output chars | Rationale |
|------|-----------------|-----------|
| bash | 8,000 | Build logs are huge but only errors matter (usually at the end) |
| read_file | 16,000 | Large files need pagination, not cramming |
| grep | 4,000 | Results should be filtered, not dumped |
| glob | 4,000 | File lists are scannable |
| wg_show | 2,000 | Task details are structured |
| wg_list | 4,000 | Task lists can be long |

### Why head+tail

Error messages, test failures, and compilation results are almost always at the **end** of output. The beginning provides setup context. The middle is usually repetitive. Head+tail captures the most diagnostic value.

---

## 4. Error Recovery with Withholding

### Requirement

When a recoverable error occurs, attempt recovery BEFORE surfacing the error to the model. The model should never see "Error 413: prompt too long" -- it should just see a slightly compacted conversation on the next turn.

### Recoverable errors

| Error | Recovery action |
|-------|----------------|
| Context too long (400/413) | Emergency compact (drop old tool results), retry |
| Rate limited (429) | Exponential backoff (1s, 2s, 4s, max 60s), retry up to 5x |
| Server error (500/502/503) | Retry up to 3x with backoff |
| Max output tokens | Inject "Please continue" message, retry |

### Non-recoverable errors

| Error | Action |
|-------|--------|
| Authentication failure (401) | Fail immediately with clear message |
| Invalid request (400, not context-related) | Fail immediately |
| All retries exhausted | Log progress via `wg log`, fail task cleanly |

### Pattern

```rust
match send_request(request) {
    Ok(response) => { /* normal flow */ }
    Err(e) if is_context_too_long(e) => {
        emergency_compact(&mut messages);
        // Retry with compacted messages -- model doesn't know
        let response = send_request(request)?;
    }
    Err(e) if is_rate_limited(e) => {
        sleep(backoff);
        continue; // Retry loop iteration
    }
    Err(e) => return Err(e), // Surface non-recoverable
}
```

---

## 5. File State Cache

### Requirement

Cache file contents in memory to avoid redundant reads. When the agent reads `src/main.rs` twice in a conversation, the second read should return cached content if the file hasn't changed (checked by mtime).

### Design

```rust
struct FileCache {
    entries: HashMap<PathBuf, CachedFile>,
    total_size: usize,
    max_size: usize,  // 25MB
    max_entries: usize, // 100
}

struct CachedFile {
    content: String,
    mtime: SystemTime,
    last_accessed: Instant,
}
```

### Behavior

- On `read_file`: check cache. If hit AND mtime unchanged → return cached content
- If miss or stale → read from disk, update cache
- Evict LRU entries when over limits
- Cache is per-agent-session (not shared across agents)

### Why this matters for Terminal Bench

Agents repeatedly read the same files during edit-test-debug cycles. Without caching, each read burns context tokens on identical content. With caching, the file content is returned from memory and the model can be told `[cached read, file unchanged since last read]` saving context space.

---

## 6. Tool Execution Parallelism

### Requirement

When the model requests multiple tool calls in a single turn, execute read-only tools in parallel.

### Concurrent-safe tools (can run in parallel)

- `read_file`
- `glob`
- `grep`
- `wg_show`
- `wg_list`

### Serial tools (must run sequentially)

- `write_file`
- `edit_file`
- `bash`
- `wg_add`
- `wg_done`
- `wg_fail`
- `wg_log`
- `wg_artifact`

### Pattern

```
Partition tool calls into concurrent-safe and serial.
Execute concurrent tools via join_all (or equivalent).
Execute serial tools one by one.
Max concurrency: 8 (configurable).
```

### Impact

Models frequently request multiple file reads or grep searches in a single turn. Sequential execution adds latency proportional to the number of calls. Parallel execution reduces this to the latency of the slowest call.

---

## 7. Session Summary Extraction

### Requirement

Periodically extract a semantic summary of the agent's progress. This is distinct from `wg log` (which is for the graph) -- this is for the agent's own working memory during journal resume.

### When to extract

After every N turns (configurable, default 10), or when context pressure hits 80%.

### What to extract

A cheap model call with the last N turns:
> "Summarize the key findings, decisions, files modified, and current state in under 500 words."

### Where to store

`.wg/agents/<agent-id>/session-summary.md`

### How to use on resume

When resuming from journal, load this summary instead of replaying the full raw conversation. This produces a much more compact and useful context for the resumed agent.

---

## 8. Prompt Cache Optimization (Anthropic Provider)

### Requirement

For the Anthropic API, mark the system prompt with `cache_control: ephemeral` so subsequent turns in the same session hit the server-side prompt cache.

### Impact

The system prompt (REQUIRED_WORKFLOW_SECTION + graph context + task description) is largely static within a task. Caching it reduces input token costs by ~90% on turns 2+. This is free performance -- no behavioral change, just a request field.

### Note

For OpenAI-compatible APIs (OpenRouter, Ollama), server-side caching is automatic. No client action needed.

---

## Priority Order

| Pattern | Impact on Reliability | Effort | Day |
|---------|----------------------|--------|-----|
| Context pressure detection + warning injection | **Critical** | S | 1 |
| Smart tool result truncation | **High** | XS | 1 |
| Error recovery with withholding (413 handling) | **Critical** | M | 1-2 |
| Mid-turn state injection (wg msg, graph changes) | **High** | S | 2 |
| File state cache | **Medium** | S | 3 |
| Tool execution parallelism | **Medium** | M | 3 |
| Session summary extraction | **Medium** | M | 4+ |
| Prompt cache optimization (Anthropic) | **Low** (cost only) | XS | 4+ |

---

## Relationship to Other Documents

- **ROADMAP-terminal-bench.md**: The 6-day phase plan. This document provides the implementation patterns for Phases 1-2.
- **REFERENCE-terminal-bench-campaign.md**: The campaign knowledge base. This document provides the engineering details that support the experiment design in that reference.
