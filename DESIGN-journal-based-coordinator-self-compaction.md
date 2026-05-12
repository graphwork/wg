# DESIGN: Journal-based Coordinator Self-Compaction

## Status

**Design Document** — Implementation specification for replacing `.compact-*` task spawning with self-managed journal-based compaction.

---

## Summary

Replace the current cycle-driven `.compact-*` task spawning mechanism with a **journal-based self-compaction system** where the coordinator agent:

1. Writes its conversation to `.wg/coordinator/journal.md`
2. Detects token thresholds internally (80% of context window)
3. Writes compaction markers directly to the journal
4. Continues without spawning graph tasks

This mirrors the native executor's `compact_messages` pattern from `src/executor/native/resume.rs`.

---

## 1. Background

### 1.1 Current Architecture

The coordinator agent (`src/commands/service/coordinator_agent.rs`) runs as a persistent Claude CLI subprocess. Token tracking exists but is unused for self-compaction:

```rust
// coordinator_agent.rs:830-844 (existing code)
if let Some((input_toks, output_toks, cache_creation_toks)) = turn_token_usage {
    let total = cache_creation_toks.saturating_add(input_toks).saturating_add(output_toks);
    cs.accumulated_tokens = cs.accumulated_tokens.saturating_add(total);
    cs.save_for(dir, coordinator_id);
}
```

The `CoordinatorState.accumulated_tokens` field is updated but triggers nothing in the coordinator itself.

### 1.2 Current Compaction Mechanism

Compaction is **cycle-driven + token-threshold-gated**:

```
.coordinator-0 (done) ──▶ .compact-0 ──▶ .coordinator-0 (new iteration)
```

The daemon:
1. Detects `.compact-0` becomes cycle-ready when `.coordinator-0` completes
2. Checks `accumulated_tokens >= threshold` (80% of 200K = 160K tokens)
3. If threshold met, runs `compactor::run_compaction()` to produce `.wg/compactor/context.md`
4. Coordinator's next iteration reads `context.md` via `build_coordinator_context()`

**Problem**: The `.compact-*` task is a separate graph node. Compaction is an external operation, not self-managed by the coordinator.

### 1.3 Native Executor Journal Pattern (Reference)

The native executor already implements the desired pattern in `src/executor/native/journal.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "entry_type")]
pub enum JournalEntryKind {
    Init { model, provider, system_prompt, tools, task_id },
    Message { role, content, usage, response_id, stop_reason },
    ToolExecution { tool_use_id, name, input, output, is_error, duration_ms },
    Compaction { compacted_through_seq, summary, original_message_count, original_token_count },
    End { reason, total_usage, turns },
}
```

The resume logic in `src/executor/native/resume.rs` (lines 188-223) shows the compaction strategy:

```rust
fn compact_messages(messages: Vec<Message>, _budget_tokens: usize) -> Vec<Message> {
    if messages.len() <= KEEP_RECENT_MESSAGES + 1 {
        return messages;
    }
    let split_point = messages.len().saturating_sub(KEEP_RECENT_MESSAGES);
    let older = &messages[..split_point];
    let summary = summarize_messages(older);
    // Build new message list: summary + recent
    let mut result = vec![];
    result.push(Message { role: Role::User, content: vec![ContentBlock::Text {
        text: format!("[Prior {} messages summarized]: {}", split_point, summary),
    }]});
    result.extend(messages[split_point..].iter().cloned());
    result
}
```

---

## 2. Data Structures

### 2.1 Journal File Location

```
.wg/coordinator/journal.md
```

**Format rationale**: Markdown with embedded JSON markers (vs. JSONL in native executor) — human-readable for debugging, easy to inspect after compaction events.

### 2.2 Journal Entry Format

Messages in the journal are stored as markdown blocks:

```markdown
--- 2026-04-09T14:00:00Z [user] ---

User message content here...

--- 2026-04-09T14:00:05Z [assistant] ---

Assistant response content here...

--- 2026-04-09T14:00:10Z [tool_use] ---

Tool: wg_tasks_create
Input: {"tasks": [...]}

--- 2026-04-09T14:00:15Z [tool_result] ---

Created 5 tasks successfully.
```

### 2.3 Compaction Marker Format

```markdown
=== COMPACTION 2026-04-09T20:30:00Z ===

```json
{
  "message_count": 150,
  "cache_creation_tokens": 180000,
  "summary": "Summarized conversation covering task assignments, graph state changes, and coordinator decisions from initial setup through iteration 50.",
  "model_used": "minimax/minimax-m2.7",
  "messages_before": 150,
  "messages_after": 50,
  "journal_before_bytes": 450000,
  "journal_after_bytes": 125000
}
```

===

[Previous messages summarized into the above summary]
```

### 2.4 CompactionData Schema

| Field | Type | Description |
|-------|------|-------------|
| `message_count` | `u32` | Number of messages collapsed into summary |
| `cache_creation_tokens` | `u64` | Tokens accumulated at compaction time |
| `summary` | `String` | Text summary of collapsed conversation |
| `model_used` | `String` | Model used for summarization |
| `messages_before` | `u32` | Count of messages before compaction |
| `messages_after` | `u32` | Count of messages retained after compaction |
| `journal_before_bytes` | `u64` | Journal file size before compaction |
| `journal_after_bytes` | `u64` | Journal file size after compaction |

### 2.5 CoordinatorJournalState (persisted)

Location: `.wg/coordinator/state.json`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorJournalState {
    /// Path to the journal file
    pub journal_path: PathBuf,
    /// Last compaction timestamp (ISO-8601)
    pub last_compaction: Option<String>,
    /// Token count at last compaction
    pub tokens_at_last_compaction: u64,
    /// Number of compactions performed
    pub compaction_count: u64,
    /// Total messages written since last compaction
    pub messages_since_compaction: u64,
}
```

---

## 3. Trigger Logic

### 3.1 Token Threshold

- **Default**: 80% of model context limit (e.g., 180,000 tokens for 200K context models)
- **Configurable** via `coordinator.compaction_threshold_ratio` (0.0-1.0)

### 3.2 Token Counting

**Critical**: Must count `cache_creation` tokens, NOT `input_tokens`.

With prompt caching, the API's `input_tokens` only counts tokens outside any cache block (typically 1-3 per turn). The actual new content accumulates in `cache_creation_input_tokens`.

```rust
// Already implemented in coordinator_agent.rs:830-844
if let Some((input_toks, output_toks, cache_creation_toks)) = turn_token_usage {
    let total = cache_creation_toks
        .saturating_add(input_toks)
        .saturating_add(output_toks);
    cs.accumulated_tokens = cs.accumulated_tokens.saturating_add(total);
}
```

### 3.3 Trigger Condition

```rust
fn should_trigger_compaction(
    accumulated_tokens: u64,
    context_window: u64,
    threshold_ratio: f64,
) -> bool {
    let threshold = (context_window as f64 * threshold_ratio) as u64;
    accumulated_tokens >= threshold
}
```

Where `context_window` comes from the model registry (e.g., 200,000 for opus models).

### 3.4 Check Timing

- Check after each coordinator turn completes
- If threshold exceeded, trigger self-compaction before processing the next message
- Compaction runs synchronously (blocks the next turn) to ensure consistency

### 3.5 Self-Compaction Process (No Task Spawning)

```rust
async fn perform_self_compaction(
    dir: &Path,
    coordinator_id: u32,
    messages: Vec<JournalMessage>,
    accumulated_tokens: u64,
    config: &Config,
) -> Result<CompactionData> {
    // 1. Read all messages from journal
    let journal_path = coordinator_journal_path(dir);
    let all_messages = parse_journal_messages(&fs::read_to_string(&journal_path)?)?;

    // 2. Determine split point: keep recent N messages
    let recent_count = config.coordinator.compaction_recent_count;
    let split_point = all_messages.len().saturating_sub(recent_count);

    if split_point == 0 {
        // Nothing to compact
        return Err(anyhow!("No messages to compact"));
    }

    // 3. Split: older (compact) vs recent (keep)
    let (older, recent) = all_messages.split_at(split_point);

    // 4. Summarize older messages using LLM
    let summary = summarize_messages_via_llm(older, &config).await?;

    // 5. Build compaction data
    let compaction_data = CompactionData {
        message_count: older.len() as u32,
        cache_creation_tokens: accumulated_tokens,
        summary: summary.clone(),
        model_used: config.effective_coordinator_model(),
        messages_before: older.len() as u32,
        messages_after: recent_count as u32,
        journal_before_bytes: fs::metadata(&journal_path)?.len(),
        journal_after_bytes: 0, // Will update after rewrite
    };

    // 6. Rewrite journal: compaction marker + recent messages
    let journal_before_bytes = compaction_data.journal_before_bytes;
    rewrite_journal_with_compaction(&journal_path, &compaction_data, recent)?;
    let journal_after_bytes = fs::metadata(&journal_path)?.len();

    // 7. Update compaction data with actual sizes
    let mut final_data = compaction_data;
    final_data.journal_after_bytes = journal_after_bytes;

    // 8. Update CoordinatorJournalState
    let mut state = CoordinatorJournalState::load(dir);
    state.last_compaction = Some(Utc::now().to_rfc3339());
    state.tokens_at_last_compaction = accumulated_tokens;
    state.compaction_count += 1;
    state.messages_since_compaction = 0;
    state.save(dir)?;

    // 9. Reset accumulated_tokens in CoordinatorState
    let mut cs = CoordinatorState::load_or_default_for(dir, coordinator_id);
    cs.accumulated_tokens = 0;
    cs.save_for(dir, coordinator_id)?;

    Ok(final_data)
}
```

---

## 4. Resume Logic

### 4.1 On Restart/Resume

1. Read `.wg/coordinator/journal.md`
2. Find **latest** `Compaction` marker (last occurrence)
3. Parse JSON summary from marker
4. Reconstruct context as: `[summary_text] + [recent N messages from journal]`

### 4.2 Context Injection Format

```
[System prompt]

=== RESUMED FROM COMPACTION 2026-04-09T15:30:00Z ===

[Summary text from compaction marker]

[Recent 50 messages from journal]

===

[Continue from here...]
```

### 4.3 Resume Implementation

```rust
fn load_journal_for_resume(
    journal_path: &Path,
    recent_count: usize,
) -> Result<Option<(CompactionData, Vec<JournalMessage>)>> {
    if !journal_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(journal_path)?;

    // Find latest compaction marker
    let Some((_marker_line, compaction_data)) = find_latest_compaction(&content)? else {
        return Ok(None); // No compaction marker, start fresh
    };

    // Parse recent messages after the marker
    let recent = parse_recent_messages_after_marker(&content, recent_count)?;

    Ok(Some((compaction_data, recent)))
}

fn find_latest_compaction(content: &str) -> Option<(String, CompactionData)> {
    // Find all "=== COMPACTION ... ===" blocks
    // Return the last one (most recent)
    let mut latest: Option<(String, CompactionData)> = None;

    for line in content.lines() {
        if let Some(stripped) = line.strip_prefix("=== COMPACTION ") {
            if let Some(end_idx) = stripped.find(" ===") {
                let timestamp = stripped[..end_idx].to_string();
                // Find the JSON block after this line
                if let Some(json) = extract_json_block(content, line) {
                    if let Ok(data) = serde_json::from_str(&json) {
                        latest = Some((timestamp, data));
                    }
                }
            }
        }
    }

    latest
}

fn extract_json_block(content: &str, marker_line: &str) -> Option<String> {
    // Find the ```json block after marker_line
    // Parse JSON until ``` marker
    // Return the JSON content
}
```

### 4.4 Recent Messages Retention

- **Default**: 50 most recent messages after compaction
- **Configurable** via `coordinator.compaction_recent_count`
- Ensures working context while minimizing token usage

---

## 5. Module Structure

### 5.1 New Module: `src/coordinator/`

```
src/
├── coordinator/
│   ├── mod.rs           # Public API re-exports
│   ├── journal.rs        # Journal file I/O, compaction marker parsing
│   ├── compaction.rs    # Self-compaction trigger logic
│   └── state.rs         # CoordinatorJournalState persistence
```

### 5.2 File: `src/coordinator/journal.rs`

```rust
//! Coordinator journal: persists conversation to markdown journal file.
//!
//! Mirrors the native executor's journal pattern but uses markdown format
//! for human-readability.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::Path;

/// Write a message entry to the coordinator journal.
pub fn append_message(
    journal_path: &Path,
    role: &str,
    content: &str,
    timestamp: &DateTime<Utc>,
) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal_path)
        .context("Failed to open coordinator journal")?;

    use std::io::Write;
    writeln!(file, "--- {} [{}] ---", timestamp.to_rfc3339(), role)?;
    writeln!(file)?;
    writeln!(file, "{}", content)?;
    writeln!(file)?;
    Ok(())
}

/// Parse all messages from a journal file.
pub fn parse_journal_messages(content: &str) -> Result<Vec<JournalMessage>> {
    // Parse --- timestamp [role] --- blocks
    // Extract content between blocks
}

/// Find the latest compaction marker in the journal.
pub fn find_latest_compaction(content: &str) -> Result<Option<(String, CompactionData)>> {
    // Find last "=== COMPACTION ... ===" block
    // Parse JSON inside
}

/// Write compaction marker and recent messages to journal.
pub fn rewrite_journal_with_compaction(
    journal_path: &Path,
    compaction: &CompactionData,
    recent_messages: &[JournalMessage],
) -> Result<()> {
    // Write new journal: marker + recent messages
}
```

### 5.3 File: `src/coordinator/compaction.rs`

```rust
//! Self-compaction logic for the coordinator agent.

use anyhow::Result;
use std::path::Path;

use crate::config::Config;

/// Check if compaction should trigger based on token count.
pub fn should_compact(
    accumulated_tokens: u64,
    context_window: u64,
    threshold_ratio: f64,
) -> bool {
    let threshold = (context_window as f64 * threshold_ratio) as u64;
    accumulated_tokens >= threshold
}

/// Perform self-compaction: summarize older messages, write marker.
pub async fn perform_self_compaction(
    dir: &Path,
    coordinator_id: u32,
    config: &Config,
) -> Result<CompactionData> {
    // 1. Read journal
    // 2. Split messages
    // 3. Summarize older via LLM
    // 4. Write compaction marker
    // 5. Update state
    // 6. Reset accumulated_tokens
}

/// Summarize messages via LLM (reuses Compactor role).
async fn summarize_messages_via_llm(
    messages: &[JournalMessage],
    config: &Config,
) -> Result<String> {
    // Build prompt from messages
    // Call LLM with Compactor role
    // Return summary
}
```

---

## 6. Configuration

### 6.1 New Config Fields (CoordinatorConfig)

```rust
// In src/config.rs ~line 2134
pub struct CoordinatorConfig {
    // ... existing fields ...

    /// Enable journal-based self-compaction (vs. spawning .compact-* tasks).
    /// When true, coordinator writes its own journal and compacts on threshold.
    /// When false (default), uses existing .compact-* task mechanism.
    #[serde(default)]
    pub use_self_compaction: bool,

    /// Path to the coordinator journal file.
    #[serde(default = "default_coordinator_journal_path")]
    pub coordinator_journal_path: PathBuf,

    /// Trigger self-compaction when cache_creation reaches this fraction
    /// of context window. Default: 0.8 (80%).
    #[serde(default = "default_compaction_threshold_ratio")]
    pub compaction_threshold_ratio: f64,

    /// Number of recent messages to retain after compaction. Default: 50.
    #[serde(default = "default_compaction_recent_count")]
    pub compaction_recent_count: usize,
}
```

### 6.2 Default Values

```rust
fn default_coordinator_journal_path() -> PathBuf {
    PathBuf::from(".wg/coordinator/journal.md")
}

fn default_compaction_threshold_ratio() -> f64 {
    0.8
}

fn default_compaction_recent_count() -> usize {
    50
}
```

### 6.3 Config File Example

```toml
[coordinator]
# Enable journal-based self-compaction (vs spawning .compact-* tasks)
use_self_compaction = true

# Trigger when cache_creation reaches 80% of context window
compaction_threshold_ratio = 0.8

# Keep 50 recent messages after compaction
compaction_recent_count = 50

# Path to coordinator journal file
coordinator_journal_path = ".wg/coordinator/journal.md"
```

---

## 7. Migration Path

### Phase 1: Parallel Implementation (Backward Compatible)

- Implement journal-based self-compaction alongside existing `.compact-*` system
- Add `coordinator.use_self_compaction: false` config flag (default: false)
- Self-compaction runs **before** the cycle-driven `.compact-0` would fire
- Test self-compaction in isolation with `use_self_compaction = true`

### Phase 2: Default Enabled

- Change default `use_self_compaction = true`
- Coordinator compacts itself, `.compact-*` tasks still created but unused
- Monitor for any issues in production

### Phase 3: Remove .compact-* Spawning

- Conditionally skip `.compact-*` task creation when `use_self_compaction = true`
- Remove dependency on `compactor::context_md_path` from coordinator agent
- Keep `.compact-*` tasks functional for manual invocation only

### Phase 4: Cleanup

- Remove `.compact-*` task spawning code from coordinator
- Archive compactor-related code (or remove if fully deprecated)
- Manual `wg compact` still works via compactor module

### 7.1 Code Changes for Migration

**In `src/commands/service/ipc.rs`:**

```rust
// Around line 1186, conditionally skip .compact-* creation
if !config.coordinator.use_self_compaction {
    // Create companion .compact-N task forming a visible cycle
    let compact_id = format!(".compact-{}", next_id);
    // ... existing .compact-* creation code
}
```

**In `src/commands/service/mod.rs`:**

```rust
// Around line 1467, conditionally create .compact-0
if !config.coordinator.use_self_compaction {
    // Ensure .compact-0 exists — forms a cycle with .coordinator-0
    if graph.get_task(".compact-0").is_none() {
        // ... existing .compact-0 creation code
    }
}
```

---

## 8. Edge Cases

### 8.1 Compaction Fails

- Log error but continue execution
- Don't block coordinator on compaction failure
- On next turn, will re-check threshold and retry
- Consider incrementing an error counter

### 8.2 Journal File Corrupted

- If `find_latest_compaction()` fails to parse, treat as no compaction
- Log warning and start fresh journal
- Consider backing up corrupted journal

### 8.3 Context Window Unknown

- If model not in registry, fall back to `compaction_token_threshold` (absolute value)
- Log warning that ratio-based threshold unavailable

### 8.4 Very Small Recent Count

- If `compaction_recent_count < 5`, warn and use minimum of 5
- Too few recent messages breaks conversation continuity

### 8.5 No Messages to Compact

- If `messages.len() <= compaction_recent_count`, skip compaction
- Nothing older than retention threshold to summarize

### 8.6 Very Long Single Messages

- Truncate extremely long messages (> 10KB) in the summary
- Keep full message in recent retention if under limit

---

## 9. Verification Criteria

### 9.1 Functional Requirements

| Requirement | Verification Method |
|------------|---------------------|
| Coordinator writes journal.md on each message | Check file exists after coordinator turn |
| Compaction marker contains valid JSON | Parse with `serde_json` |
| Latest Compaction marker found on resume | Parse marker, verify timestamp is newest |
| Context reconstructed as: summary + recent N | Check injected context format |
| Token counting uses `cache_creation` | Compare to API response metadata |
| No `.compact-*` tasks when `use_self_compaction=true` | Check graph for `.compact-*` tasks |

### 9.2 Test Scenarios

| Scenario | Test Method |
|----------|-------------|
| Compaction triggers at threshold | Set low threshold, run many turns, verify marker written |
| Journal marker format valid | Parse marker JSON, verify all fields present |
| Resume reconstructs correctly | Kill/restart coordinator, verify context |
| Token counting accurate | Compare accumulated_tokens to API response |
| Migration flip | Set `use_self_compaction=false→true`, verify no regression |

### 9.3 Unit Tests

```bash
# Test compaction marker parsing
cargo test coordinator::journal::tests

# Test should_compact threshold logic
cargo test coordinator::compaction::tests

# Test CoordinatorJournalState persistence
cargo test coordinator::state::tests
```

### 9.4 Integration Tests

```bash
# Test full compaction cycle with mocked LLM
cargo test integration_coordinator_self_compaction

# Test resume from compacted journal
cargo test integration_coordinator_resume
```

### 9.5 Manual Verification

```bash
# Start coordinator with self-compaction enabled
WG_COORDINATOR_USE_SELF_COMPACTION=true wg service start

# Check journal after several interactions
cat .wg/coordinator/journal.md

# Verify compaction marker format
grep -A 20 "=== COMPACTION" .wg/coordinator/journal.md

# Kill and restart coordinator, verify resume
pkill -f "wg service"
wg service start
# Coordinator should resume from latest compaction marker
```

---

## 10. File Inventory

### New Files

| File | Purpose |
|------|---------|
| `src/coordinator/mod.rs` | Module root, public API |
| `src/coordinator/journal.rs` | Journal file I/O, message parsing, marker formatting |
| `src/coordinator/compaction.rs` | Compaction trigger logic, LLM summarization |
| `src/coordinator/state.rs` | CoordinatorJournalState persistence |

### Modified Files

| File | Change |
|------|--------|
| `src/commands/service/coordinator_agent.rs` | Wire in self-compaction checks, journal appends |
| `src/commands/service/mod.rs` | Import coordinator module |
| `src/config.rs` | Add `use_self_compaction`, `coordinator_journal_path`, etc. |
| `src/commands/service/ipc.rs` | Conditionally skip `.compact-*` creation |
| `Cargo.toml` | Add `mod coordinator` |

### Deprecated (Not Deleted)

| File | Reason |
|------|--------|
| `src/service/compactor.rs` | Still used by `wg compact` command; module kept for manual invocation |
| `.wg/compactor/context.md` | Still produced by manual `wg compact` invocations |

---

## 11. Appendix: Reference Implementation

### Native Executor Compaction (resume.rs:188-214)

```rust
fn compact_messages(messages: Vec<Message>, _budget_tokens: usize) -> Vec<Message> {
    if messages.len() <= KEEP_RECENT_MESSAGES + 1 {
        return messages;
    }

    let split_point = messages.len().saturating_sub(KEEP_RECENT_MESSAGES);

    // Summarize the older messages
    let older = &messages[..split_point];
    let summary = summarize_messages(older);

    // Build new message list: first (context) + summary + recent
    let mut result = vec![];
    result.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!("[Prior {} messages summarized]: {}", split_point, summary),
        }],
    });
    result.extend(messages[split_point..].iter().cloned());

    result
}
```

### Coordinator Compaction (equivalent design)

```rust
async fn perform_self_compaction(...) -> Result<CompactionData> {
    // 1. Read all messages from journal
    let messages = parse_journal_messages(&fs::read_to_string(&journal_path)?)?;

    // 2. Split: older (compact) vs recent (keep)
    let split_point = messages.len().saturating_sub(recent_count);
    let (older, recent) = messages.split_at(split_point);

    // 3. Summarize older messages using LLM
    let summary = summarize_messages_via_llm(older).await?;

    // 4. Write compaction marker to journal
    write_compaction_marker(&journal_path, &CompactionData {
        message_count: older.len() as u32,
        cache_creation_tokens: accumulated_tokens,
        summary: summary.clone(),
        messages_before: older.len() as u32,
        messages_after: recent_count as u32,
        // ...
    })?;

    // 5. Rewrite journal with: marker + recent messages (older removed)
    rewrite_journal(&journal_path, &recent)?;

    // 6. Reset accumulated_tokens in CoordinatorState
    Ok(compaction_data)
}
```
