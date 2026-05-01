# Research: Existing Design Documents for Journal-Based Compaction

**Task:** research-existing-design-documents-for-journal-com  
**Date:** 2026-04-09  
**Status:** Complete

---

## Executive Summary

Three compaction-related systems exist in the codebase, but none implements true journal-based coordinator self-compaction. The `design-journal-based-2` task (which this research feeds into) has no output artifact — the agent that ran it produced only subtasks but did not complete a design document. This research synthesizes findings from all relevant sources.

---

## 1. Relevant Source Documents

| Document | Purpose | Key Finding |
|----------|---------|-------------|
| `src/executor/native/journal.rs` | Native executor conversation journal | Defines `JournalEntryKind::Compaction` entry type |
| `src/executor/native/resume.rs` | Native executor resume/compaction | `compact_messages()` pattern — keep first msg + last N, summarize middle |
| `src/service/compactor.rs` | Graph-level compactor (context.md) | Deprecated `should_compact()` — cycle-driven now |
| `src/service/chat_compactor.rs` | Per-coordinator chat compaction | Produces `context-summary.md` from inbox/outbox |
| `src/commands/service/coordinator_agent.rs` | Coordinator subprocess management | Tracks `accumulated_tokens` from Claude CLI stream-json |
| `src/commands/service/mod.rs` | Daemon compaction orchestration | Cycle-driven + token-threshold gating (broken: resets on restart) |
| `.wg/docs/coordinator-compaction.md` | Research: coordinator context management | Documents gaps and 4 design strategies |
| `.wg/docs/compaction-frequency-analysis.md` | Research: compaction frequency | Threshold never fires due to restart bug |
| `docs/research/compaction-regimes.md` | Cross-provider compaction research | Anthropic auto-compact beta, API token counting |

---

## 2. Native Executor Journal Pattern (Reference Implementation)

The native executor uses a **journal-based compaction** pattern at `src/executor/native/`:

### 2.1 Journal Entry Types (`journal.rs:34-94`)
```rust
pub enum JournalEntryKind {
    Init { model, provider, system_prompt, tools, task_id },
    Message { role, content, usage, response_id, stop_reason },
    ToolExecution { tool_use_id, name, input, output, is_error, duration_ms },
    Compaction {
        compacted_through_seq: u64,
        summary: String,
        original_message_count: u32,
        original_token_count: u32,
    },
    End { reason, total_usage, turns },
}
```

### 2.2 Compaction Logic (`resume.rs:188-223`)
```rust
fn compact_messages(messages: Vec<Message>, budget_tokens: usize) -> Vec<Message> {
    // Keep first message (context) + last N verbatim
    const KEEP_RECENT_MESSAGES: usize = 6;
    
    let split_point = messages.len() - KEEP_RECENT_MESSAGES;
    let older = &messages[..split_point];
    let summary = summarize_messages(older);
    
    // Inject summary as a user message
    compacted.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!(
                "[Resume: This conversation is being resumed from a journal. \
                 The first {} messages were compacted into this summary:]\n\n{}",
                split_point, summary
            ),
        }],
    });
    
    compacted.extend_from_slice(&messages[split_point..]);
}
```

### 2.3 Resume Reconstruction (`resume.rs:127-161`)
On resume, the journal is read and messages reconstructed:
- `Message` entries → direct reconstruction
- `Compaction` entries → inject summary as a user message
- `ToolExecution` → skipped (tool results appear in following User message)

---

## 3. Current Coordinator Compaction System

### 3.1 What Exists vs. What's Needed

| Aspect | Current System | Journal-Based Need |
|--------|---------------|-------------------|
| **Target** | Graph state (`context.md`) + chat history | Coordinator's own LLM conversation |
| **Trigger** | Cycle-driven + token-threshold (broken) | Token threshold when coordinator approaches limit |
| **Execution** | `.compact-*` task with LLM call | Coordinator self-compacts OR daemon compacts coordinator journal |
| **Output** | `context.md` (graph summary) | Compacted coordinator journal with `Compaction` markers |
| **Token tracking** | `accumulated_tokens` (per-turn deltas, resets on restart) | Full context window utilization tracking |

### 3.2 Critical Bug: `accumulated_tokens` Resets on Restart
From `compaction-frequency-analysis.md`:
> **Critical finding: accumulated_tokens resets to 0 on every daemon restart**, losing all progress toward the threshold. Since the daemon was restarted 6+ times today, the counter never gets close to 160,000.

Location: `src/commands/service/mod.rs:2395-2396`:
```rust
accumulated_tokens: CoordinatorState::load(&dir)
    .map(|cs| cs.accumulated_tokens)
    .unwrap_or(0),
```
Wait — this DOES load from disk. But `start_service()` at line 1520 initializes to 0. The actual bug is that the CoordinatorAgent thread initializes fresh each time rather than loading persisted state.

### 3.3 Three Separate Compaction Systems

1. **Graph compactor** (`src/service/compactor.rs`): Produces `context.md` — a rolling 3-layer summary (Narrative + Facts + Evaluation Digest). Triggered by `.compact-0` cycle task.

2. **Chat compactor** (`src/service/chat_compactor.rs`): Produces `context-summary.md` per coordinator from inbox/outbox JSONL. Triggered when new messages exceed threshold.

3. **Native executor journal** (`src/executor/native/`): Uses `conversation.jsonl` with `Compaction` markers. On resume, reconstructs messages from journal and auto-compacts if over budget.

**None of these** compacts the coordinator's own LLM conversation context.

---

## 4. The `design-journal-based-2` Task

### 4.1 Task Requirements (from `design-journal-based-2` task description)
```
Design a journal-based self-compaction system for the coordinator, replacing 
the current .compact-* task loop with an approach that mirrors the native 
executor’s compact_messages pattern.

Context:
- Coordinator is a long-lived Claude CLI subprocess whose LLM context grows unboundedly
- Native executor uses journal-based compaction: summarizes old entries into a 
  Compaction marker, on resume finds latest marker and injects summary + recent N messages
- Coordinator currently spawns .compact-* tasks which compact the graph state summary 
  (.wg/compactor/context.md), NOT the coordinator's own LLM conversation history
- accumulated_tokens IS tracked but used only to trigger .compact-* tasks, not self-compaction
- API limit note: with prompt caching, input_tokens is near-zero (cached), 
  cache_creation is where new content accumulates — must account for this
```

### 4.2 Output Status
The task at `.wg/output/design-journal-based-2/` contains only `conversation.jsonl` (97KB of agent work logs) — **no design document was produced**. The agent created 10+ subtasks but did not synthesize them.

---

## 5. API Token Counting Consideration

From `compaction-regimes.md`:
- **Anthropic**: `POST /v1/messages/count_tokens` for pre-flight counting
- **Claude CLI stream-json**: `usage.input_tokens` reports incremental tokens per turn, NOT full context size
- **cache_creation_input_tokens**: Where new content accumulates with prompt caching

The coordinator's `accumulated_tokens` is summed from per-turn deltas, not actual context window fill percentage. This is why the 160K threshold never fires — the values are in the thousands per session.

---

## 6. Recommendations for `design-bare-coordinator-architecture-document`

The downstream task `design-bare-coordinator-architecture-document` should consider:

1. **Option A**: Mirror native executor pattern — coordinator maintains its own `conversation.jsonl` journal, daemon compacts it when token threshold met, coordinator resumes from compacted journal on restart

2. **Option B**: Keep Claude CLI subprocess but track actual context via `count_tokens` API call before each coordinator message, trigger graceful restart when approaching limit

3. **Option C**: Migrate coordinator to native executor pattern (direct API calls) for full control over message array and precise compaction

4. **Critical bug to fix regardless**: `accumulated_tokens` persistence across daemon restarts (the compaction gating is currently non-functional)

---

## 7. Files to Reference for Design

| File | Lines | Relevance |
|------|-------|-----------|
| `src/executor/native/journal.rs` | 1-110 | JournalEntry, JournalEntryKind::Compaction definition |
| `src/executor/native/resume.rs` | 66-125 | `load_resume_data()` — journal loading and compaction |
| `src/executor/native/resume.rs` | 188-223 | `compact_messages()` — the pattern to mirror |
| `src/executor/native/resume.rs` | 127-161 | `reconstruct_messages()` — how Compaction markers are handled on resume |
| `src/commands/service/coordinator_agent.rs` | 680-710 | Token accumulation from stream-json |
| `src/commands/service/mod.rs` | 1639-1800 | `run_graph_compaction()` — current cycle-driven trigger |
| `src/service/compactor.rs` | 1-77 | Graph compactor state and structure |
| `.wg/docs/coordinator-compaction.md` | Full | Gap analysis and 4 design strategies |
| `.wg/docs/compaction-frequency-analysis.md` | Full | Threshold bug analysis |
| `docs/research/compaction-regimes.md` | 1-56 | API-level auto-compaction (Anthropic beta) |

---

## 8. Validation

This research document itself serves as the output. The downstream `design-bare-coordinator-architecture-document` task should:

1. Read `src/executor/native/resume.rs:188-223` for the compact_messages pattern
2. Read `src/executor/native/journal.rs:76-86` for the Compaction entry type
3. Read `.wg/docs/coordinator-compaction.md` for gap analysis
4. Read `src/commands/service/mod.rs:1639-1800` for current trigger mechanism
5. Fix the `accumulated_tokens` persistence bug as a prerequisite
