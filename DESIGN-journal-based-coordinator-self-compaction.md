# DESIGN: Journal-based Coordinator Self-Compaction

## Goal

Replace the current `.compact-*` task loop with a journal-based self-compaction system that mirrors the native executor's `compact_messages` pattern.

## Context

- Coordinator is a long-lived Claude CLI subprocess with unbounded LLM context growth
- Native executor uses journal-based compaction: summarizes old entries into a Compaction marker, on resume finds latest marker and injects summary + recent N messages
- Coordinator currently spawns `.compact-*` tasks which compact the graph state summary (`.workgraph/compactor/context.md`), NOT the coordinator's own LLM conversation history
- `accumulated_tokens` IS tracked but used only to trigger `.compact-*` tasks, not self-compaction
- API note: with prompt caching, `input_tokens` is near-zero (cached), `cache_creation` is where new content accumulates

## 1. Data Structures

### Journal File Location
`.workgraph/coordinator/journal.md`

### Compaction Marker Format
```
=== COMPACTION {timestamp} ===

```json
{
  "message_count": 150,
  "cache_creation_tokens": 180000,
  "summary": "Summarized conversation covering...",
  "model_used": "minimax/minimax-m2.7",
  "messages_before": 150,
  "messages_after": 50
}
```

===

### Summary Format (JSON fields)

| Field | Type | Description |
|-------|------|-------------|
| `message_count` | u32 | Number of messages collapsed |
| `cache_creation_tokens` | u32 | Tokens at compaction time |
| `summary` | string | Text summary of collapsed conversation |
| `model_used` | string | Model used for summarization |
| `messages_before` | u32 | Messages before compaction |
| `messages_after` | u32 | Messages retained after compaction |

## 2. Trigger Logic

### Token Threshold

- Default: 80% of model context limit (180K for 200K models)
- Configurable: `coordinator.compaction.threshold`

### CRITICAL: Count cache_creation, NOT input_tokens

```rust
// WRONG - input_tokens is ~0 with prompt caching
let tokens = response.usage.input_tokens;

// CORRECT - cache_creation accumulates new content
let tokens = response.usage.cache_creation;
```

### Trigger Condition

```rust
if cache_creation_tokens >= (context_limit * threshold) {
    self.perform_self_compaction()?; // NO .compact-* spawning
}
```

### Threshold Configuration

```toml
[coordinator.compaction]
threshold = 0.8  # 80% of context limit
messages_before = 50  # Messages to retain after compaction
```

## 3. Resume Logic

### On Restart Sequence

1. Read `.workgraph/coordinator/journal.md`
2. Find LATEST Compaction marker
3. Parse JSON summary from marker
4. Reconstruct context: `[summary] + [recent N messages]`

### Context Injection Format

```
[System prompt]
=== RESUMED FROM COMPACTION {timestamp} ===
[Summary text extracted from JSON]
[Recent 50 messages]
===
```

### Resume Implementation

```rust
fn resume_from_compaction(&mut self) -> Result<Vec<Message>> {
    let journal_path = self.base_path.join("coordinator/journal.md");
    let content = fs::read_to_string(&journal_path)?;
    
    // Find latest compaction marker
    let marker = find_latest_compaction_marker(&content)?;
    let summary: CompactionSummary = serde_json::from_str(&marker.json)?;
    
    // Load recent messages after compaction
    let recent = self.load_recent_messages(summary.messages_after)?;
    
    Ok(vec![
        system_message(),
        compaction_header_message(&marker.timestamp),
        summary_message(&summary.summary),
    ].into_iter().chain(recent).collect())
}

fn find_latest_compaction_marker(content: &str) -> Result<CompactionMarker> {
    // Find all markers, return the one with highest timestamp
    let markers: Vec<CompactionMarker> = extract_compaction_markers(content);
    markers.into_iter().max_by_key(|m| m.timestamp)
}
```

## 4. Migration Path

### Phase 1: Implement Parallel (Feature Flag)

- Add `use_self_compaction: false` config (default `false`)
- Implement journal writing and resume logic
- Test alongside existing `.compact-*` system
- No behavior change by default

```toml
[coordinator.compaction]
enabled = false  # Feature flag for parallel testing
```

### Phase 2: Enable by Default

- Flip `enabled = true` as default
- Coordinator performs self-compaction instead of spawning `.compact-*`
- Monitor in production, collect metrics

```toml
[coordinator.compaction]
enabled = true  # Now the default
```

### Phase 3: Cleanup

- Remove `.compact-*` task spawning from coordinator
- Archive or remove `compactor/context.md` dependency
- Remove compactor daemon code if unused
- Clean up configuration options related to old system

## 5. Verification Criteria

### Functional Requirements

- [ ] Journal file created at `.workgraph/coordinator/journal.md`
- [ ] Compaction marker written after each self-compaction
- [ ] Compaction marker contains valid JSON with all required fields
- [ ] Latest compaction marker correctly identified on resume
- [ ] Resumed context = summary + recent N messages
- [ ] Uses `cache_creation` tokens, NOT `input_tokens`
- [ ] No `.compact-*` tasks spawned when self-compaction is enabled
- [ ] Configuration `threshold` parameter respected
- [ ] Configuration `messages_after` parameter respected

### Integration Requirements

- [ ] Self-compaction works alongside graph state compaction during Phase 1
- [ ] Transition to self-compaction only is seamless in Phase 2
- [ ] Old journal format backward compatible if schema evolves
- [ ] Graceful handling if journal file is missing/corrupted

### Performance Requirements

- [ ] Compaction completes within existing task timeout
- [ ] Journal file size remains bounded (periodic cleanup of old markers)
- [ ] Resume operation does not cause notable delay

## 6. Error Handling

### Corrupted Journal

```rust
match find_latest_compaction_marker(&content) {
    Ok(marker) => marker,
    Err(e) => {
        // Log warning, start fresh (no compaction resume)
        log::warn!("Failed to parse journal, starting fresh: {}", e);
        return Ok(vec![]);
    }
}
```

### Missing Journal File

```rust
if !journal_path.exists() {
    return Ok(vec![]);  // No compaction to resume
}
```

### Compaction Failure

- Log error with details
- Do not update `accumulated_tokens` (will retry on next message)
- Do not spawn `.compact-*` as fallback (self-compaction is the new path)
- Alert if failure rate exceeds threshold

## 7. Testing Strategy

### Unit Tests

```bash
cargo test compaction
cargo test journal
```

### Test Scenarios

1. **Compaction marker parsing**: Valid JSON, all fields present
2. **Latest marker selection**: Correct timestamp ordering
3. **Resume context reconstruction**: Summary + recent messages correct
4. **Token threshold calculation**: Edge cases (exactly 80%, over 100%)
5. **Empty/corrupted journal handling**: Graceful degradation

### Integration Tests

1. **Full compaction cycle**: Messages → Compaction → Resume → Continue
2. **Multiple compactions**: Chain of markers, resume from latest
3. **Mode switch**: Toggle between `.compact-*` and self-compaction

## 8. Comparison with Native Executor Pattern

| Aspect | Native Executor | Coordinator (New) |
|--------|----------------|-------------------|
| Journal Location | `.workgraph/executor/journal.md` | `.workgraph/coordinator/journal.md` |
| Compaction Target | Executor messages | Coordinator messages |
| Trigger | Message count threshold | `cache_creation` token threshold |
| Summary Injection | `[COMPACTION] ...` marker | `=== RESUMED FROM COMPACTION ===` marker |
| Config | `compact_messages` block | `coordinator.compaction` block |
| Task Spawning | N/A (self-contained) | No `.compact-*` spawned |

## 9. Related Files

- Coordinator source: `src/coordinator/mod.rs`
- Compactor (to be deprecated): `src/compactor/`
- Config schema: `schema/workgraph.toml`
- Native compact_messages: `src/executor/compact.rs`

## 10. Open Questions

1. Should we keep the journal file indefinitely or periodically prune old markers?
2. Do we need to compact the journal itself if it grows too large?
3. Should we expose compaction statistics via the status command?
4. What's the expected behavior if self-compaction fails repeatedly?
