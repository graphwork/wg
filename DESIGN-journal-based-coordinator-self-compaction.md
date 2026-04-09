# Design: Journal-Based Coordinator Self-Compaction

**Date:** 2026-04-09  
**Status:** Ready for Implementation  
**Type:** Architecture Enhancement

---

## Context

The Coordinator is a long-lived Claude CLI subprocess with unbounded LLM context growth. The current system spawns `.compact-*` tasks which compact the graph state summary (`.workgraph/compactor/context.md`), NOT the coordinator's own LLM conversation history.

The `accumulated_tokens` field IS tracked but used only to trigger `.compact-*` tasks, not self-compaction.

**API Critical Note:** With prompt caching, `input_tokens` is near-zero (cached), `cache_creation` is where new content accumulates.

---

## 1. Data Structures

### Journal File Location
`.workgraph/coordinator/journal.md`

### Compaction Marker Format
```
=== COMPACTION 2026-04-09T20:30:00Z ===

```json
{
  "message_count": 150,
  "cache_creation_tokens": 180000,
  "summary": "Summarized conversation covering task assignments, graph state changes, and coordinator decisions from initial setup through iteration 50.",
  "model_used": "minimax/minimax-m2.7",
  "messages_before": 150,
  "messages_after": 50
}
```

===

### Summary Format (JSON fields)
- `message_count`: u32 - Number of messages collapsed into summary
- `cache_creation_tokens`: u32 - Tokens accumulated at compaction time
- `summary`: string - Text summary of collapsed conversation
- `model_used`: string - Model used for summarization
- `messages_before`: u32 - Count of messages before compaction
- `messages_after`: u32 - Count of messages retained after compaction

---

## 2. Trigger Logic

### Token Threshold
- Default: 80% of model context limit (180,000 tokens for 200K context models)
- Configurable via `coordinator.compaction.threshold` (0.0-1.0)

### CRITICAL: Count cache_creation, NOT input_tokens

```rust
// WRONG - input_tokens is near-zero with prompt caching
let tokens = response.usage.input_tokens;

// CORRECT - cache_creation accumulates new content in cache
let tokens = response.usage.cache_creation;
```

### Trigger Condition

```rust
fn should_compact(cache_creation_tokens: u32, context_limit: u32, threshold: f32) -> bool {
    let limit = (context_limit as f32 * threshold) as u32;
    cache_creation_tokens >= limit
}
```

### Self-Compaction Process (No Task Spawning)

1. Check `cache_creation_tokens` after each coordinator turn
2. If threshold exceeded, coordinator calls LLM to summarize recent history
3. Coordinator writes Compaction marker to `journal.md`
4. Coordinator truncates messages, retaining only recent N (default: 50)
5. Coordinator continues WITHOUT spawning `.compact-*` task

### Configuration

```yaml
coordinator:
  compaction:
    enabled: true
    threshold: 0.8
    recent_count: 50
    journal_path: ".workgraph/coordinator/journal.md"
    use_self_compaction: true
```

---

## 3. Resume Logic

### On Restart/Resume

1. Read `.workgraph/coordinator/journal.md`
2. Find LATEST Compaction marker (last occurrence in file)
3. Parse JSON summary from marker body
4. Reconstruct context: `[system prompt]` + `[summary]` + `[recent N messages]`
5. Continue execution

### Context Injection Format

```
[System Prompt]

=== RESUMED FROM COMPACTION 2026-04-09T21:00:00Z ===
[Summarized conversation covering 200 messages across 50 tasks...]

Recent events:
- Task design-journal-based-2 assigned and in-progress
- 3 research subtasks created for parallel investigation

[Recent 50 messages from journal]
===

[Continue from here...]
```

---

## 4. Migration Path

### Phase 1: Implement Self-Compaction (Parallel)

- Implement journal-based self-compaction in coordinator
- Add `coordinator.compaction.use_self_compaction: false` config (default false)
- Run both systems in parallel, compare behavior

### Phase 2: Enable by Default

- Flip `use_self_compaction: true` as default
- Remove code that spawns `.compact-*` tasks from coordinator

### Phase 3: Cleanup

- Remove compactor/context.md dependency from coordinator
- Archive or remove compactor daemon logic

### Code Changes Required

```rust
// REMOVE:
if self.accumulated_tokens > threshold {
    self.spawn_compact_task()?;  // NO LONGER USED
}

// REPLACE with:
if should_compact(cache_creation, context_limit, threshold) {
    self.perform_self_compaction()?;  // NEW: writes to journal
}
```

---

## 5. Verification Criteria

### Functional Requirements

- [ ] Coordinator writes journal.md on each self-compaction
- [ ] Compaction marker contains valid JSON with all required fields
- [ ] Latest Compaction marker is found on resume
- [ ] Context reconstructed as: summary + recent N messages
- [ ] Token counting uses cache_creation, not input_tokens
- [ ] No `.compact-*` tasks spawned when self-compaction enabled

### Verification Commands

```bash
cargo test compaction
cargo test journal
```

### API Implementation Notes

```rust
struct Usage {
    input_tokens: u32,      // Near-zero with prompt caching
    cache_creation: u32,    // Where new content accumulates
}
let accumulated = response.usage.cache_creation;
```

---

## Appendix: Comparison with Native Executor

| Aspect | Native Executor | Coordinator (New) |
|--------|-----------------|------------------|
| Journal Location | `.workgraph/executor/journal.md` | `.workgraph/coordinator/journal.md` |
| Compaction Target | Task execution messages | Coordinator conversation |
| Trigger | Message count threshold | cache_creation token threshold |
| Task Spawning | None (self-contained) | None (NEW: no .compact-* spawning) |

---

## Appendix: Configuration Schema

```yaml
coordinator:
  compaction:
    use_self_compaction: true
    threshold: 0.8
    recent_count: 50
    journal_path: ".workgraph/coordinator/journal.md"

compactor:
  enabled: false  # DEPRECATED
```

---

## Relationship to Native Executor Pattern

The native executor uses a journal-based compaction pattern in `src/executor/native/resume.rs:698–845`. The coordinator's self-compaction mirrors this pattern:

| Component | Native Executor | Coordinator Self-Compaction |
|-----------|-----------------|---------------------------|
| Journal file | Per-task JSONL | Single coordinator journal |
| Compaction trigger | Per-turn pressure check | Post-turn token check |
| Summary method | LLM call | LLM call |
| Marker format | JSONL entry | Markdown with JSON block |
| Resume reconstruction | Parse last marker, inject summary + recent | Parse last marker, inject summary + recent |

The coordinator's implementation should reuse the same `ContextBudget` compaction algorithm from the native executor.
