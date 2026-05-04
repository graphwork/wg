# Checkpointing for Long-Running LLM Agents: Systems Analysis

**Researcher:** B2 (committee-v2-researcher-4)
**Focus:** Systems-side patterns — event sourcing, real-world analogies, incremental vs full, triggers, implicit disk state, handoff documents

---

## 1. Event Sourcing Analogy

workgraph's `stream.jsonl` is already an event log. Every tool call produces `ToolStart`/`ToolEnd` events, every LLM turn produces `Turn` events with token usage. The agent's observable state is the reduction of all stream events.

**Can we replay events to restore state?**

Not directly for LLM agents. The key distinction:

| System | Events | State reconstruction |
|--------|--------|---------------------|
| Traditional event sourcing (e.g., bank ledger) | Deterministic | Replay events → identical state |
| LLM agent | Non-deterministic | Replaying tool calls doesn't recreate the LLM's internal context window |

The LLM's "state" lives in two places:
1. **Server-side context** (for Claude: accessible via `--resume <session-id>`) — this IS the true state
2. **Side effects on disk** (files written, git commits, artifacts registered) — these persist naturally

**Verdict:** Event sourcing gives us an *audit trail* and *progress summary*, but NOT a replay mechanism. The only true "replay" for an LLM agent is server-side session resumption.

## 2. Comparison with Real Systems

### Temporal Workflows
- Temporal records every workflow step as an event in its history
- On replay, activities are NOT re-executed — their recorded results are used
- **Key insight for us:** Temporal's model works because activities have deterministic recorded outputs. LLM tool calls are similar — the tool output is deterministic (same `cargo build` produces same result at that point in time), but the LLM's *decision* of what tool to call next is not
- **What to steal:** Temporal's concept of "continue-as-new" — when history gets too long, start a fresh execution with a summary of prior state. This is exactly the "handoff document" pattern.

### Redis AOF (Append-Only File)
- Every write command appended to a log file
- On restart: replay the AOF to reconstruct in-memory state
- Periodic rewrite (compaction) to prevent unbounded growth
- **Analogy:** `stream.jsonl` IS our AOF. But we can't "replay" LLM decisions.
- **What to steal:** AOF rewrite/compaction. A checkpoint summary is a "compacted" version of the full event stream. Instead of replaying 500 events, hand the new agent a 200-token summary.

### Database WAL (Write-Ahead Log)
- WAL records changes BEFORE they're applied to data pages
- On crash recovery: replay WAL from last checkpoint
- Checkpoints flush dirty pages to disk, allowing WAL truncation
- **Key insight:** WAL checkpoints exist to bound recovery time. Same principle applies: a checkpoint summary bounds the ramp-up time for a replacement agent.
- **What to steal:** The checkpoint = "snapshot of committed state" concept. For us, a checkpoint is: (1) what files were modified, (2) what was accomplished, (3) what's left to do.

### Summary of stolen ideas

| Pattern | Source | Application to workgraph |
|---------|--------|------------------------|
| Continue-as-new | Temporal | Handoff document → new agent with summary |
| AOF compaction | Redis | Summarize stream.jsonl into checkpoint |
| WAL checkpoint | Databases | Periodic snapshots bound recovery cost |
| Recorded activity results | Temporal | Don't re-run tools, trust file state |

## 3. Incremental vs Full Checkpoints

### Full checkpoint
- Dump: complete description of what was done, what files changed, current status
- Cost: LLM must generate the summary (~500-2000 tokens output)
- Frequency: expensive if done too often

### Incremental checkpoint
- Only record diffs since last checkpoint
- For LLM agents, "diff" = new tool calls since last checkpoint
- Could be as simple as: append to a `checkpoint.log` file after each tool call
- Recovery: replay all incremental checkpoints in order

### Practical assessment

**Full checkpoints are more practical for LLM agents.** Here's why:

1. **Incremental checkpoints accumulate** — after 20 increments, you need to read all 20 to reconstruct state, which is no better than reading `stream.jsonl`
2. **LLM summarization is cheap** — a haiku-class model can summarize a stream into a checkpoint for ~$0.01
3. **The replacement agent needs a narrative**, not raw events — "I implemented the parser, tests pass, now working on the formatter" is more useful than a list of 47 tool calls
4. **Full checkpoints are self-contained** — no dependency chain to reconstruct

**Recommendation:** Full checkpoints only. Incremental adds complexity with minimal benefit for LLM agent use cases.

## 4. Checkpoint Triggers

### Option A: Time-based (every N minutes)
- Simple to implement: timer in coordinator or agent
- Problem: wastes work if agent is idle or between meaningful steps
- Problem: might checkpoint mid-tool-call (meaningless intermediate state)

### Option B: Event-based (every N tool calls or turns)
- Better granularity: checkpoint after every 5 tool calls
- Stream events already track tool calls, so the coordinator can trigger this
- Still might checkpoint at arbitrary points

### Option C: Milestone-based (agent explicitly marks progress)
- Agent writes checkpoint when it reaches a meaningful state
- Requires agent cooperation (prompt engineering)
- Highest quality checkpoints but lowest reliability

### Option D: Hybrid (recommended)

```
Trigger checkpoint when ANY of:
  - 10+ minutes since last checkpoint AND agent has produced new events
  - 10+ tool calls since last checkpoint
  - Agent explicitly writes a progress marker
  - Agent is about to be parked/killed (pre-death checkpoint)
```

The **pre-death checkpoint is the most important trigger**. If the coordinator detects a stuck agent and decides to kill it (per the liveness detection design), it should extract a checkpoint BEFORE killing. This is already partly implemented — the triage system generates a `## Previous Attempt Recovery` section using the output log.

### Integration with existing triage

The current triage flow (`triage.rs:480-497`) already does a form of checkpointing:
1. Agent dies
2. Triage LLM reads the output log
3. Generates a summary → injected as `## Previous Attempt Recovery`
4. New agent picks up from there

This IS a checkpoint, just triggered by death rather than proactively. The question is whether proactive checkpointing adds enough value over death-triggered checkpointing.

**Analysis:** For tasks under 30 minutes, death-triggered checkpointing is sufficient. The agent either finishes or dies, and triage handles it. For tasks over 30 minutes, proactive checkpointing provides insurance — if the agent dies at minute 45, we have a checkpoint from minute 30 instead of trying to summarize 45 minutes of work.

## 5. Disk State as Implicit Checkpoint

**Key insight: file changes persist regardless of agent death.**

When an LLM agent runs `cargo build`, edits files, writes tests — all of that survives agent death. The agent's working directory in `.wg/agents/<agent-id>/` contains its output log, and the project repo contains all file changes.

**What's actually lost when an agent dies:**
1. The LLM's context window (conversation history, reasoning chain)
2. The agent's "plan" — what it intended to do next
3. Uncommitted mental model of the code

**What's NOT lost:**
1. All file modifications (in the working tree)
2. Git commits (if the agent committed)
3. `stream.jsonl` events (tool calls, timestamps, token usage)
4. Output log (stdout/stderr)
5. Artifacts registered via `wg artifact`
6. Log entries via `wg log`

**Implication:** The real problem isn't preserving state — it's giving a NEW agent enough context to understand the existing file state and continue. This reframes checkpointing from "save agent state" to "generate orientation document for successor."

### What a successor agent needs

1. **What was the goal?** (already in task description)
2. **What was accomplished?** (summarize from stream/output log)
3. **What files were changed?** (`git diff` or artifact list)
4. **What's the current status?** (tests passing? build broken? mid-refactor?)
5. **What was the plan?** (what the dead agent intended to do next)

Items 1-4 can be reconstructed from disk state. Item 5 is the only thing truly lost — and that's what an explicit checkpoint (or pre-death handoff document) captures.

## 6. The Handoff Document Pattern

### What is it?
Agent writes a structured progress summary before dying (or periodically). Successor agent reads it as part of its prompt.

### Is it sufficient?

**For most workgraph tasks: yes.** Here's why:

1. Tasks are typically scoped (20-60 min of work)
2. The task description provides goal and validation criteria
3. File state provides ground truth of what was done
4. `git diff` shows exactly what changed
5. The handoff document fills the gap: what was the agent's plan, what's left

### What's lost even with a handoff document?

1. **Nuanced understanding** — the dead agent may have spent 10 minutes understanding a subtle bug. The summary can't capture all that reasoning. The successor will need to re-investigate.
2. **Failed approaches** — "I tried X and it didn't work because Y" is valuable but often not captured in summaries
3. **Context window warmth** — the dead agent had relevant code in its context. The successor starts cold.

### Cost of the handoff document

| Component | Cost |
|-----------|------|
| Generate summary (haiku) | ~$0.01-0.03 |
| Successor reads summary | ~200-500 extra input tokens |
| Successor ramp-up time | 2-5 minutes to re-orient |
| Quality loss | ~10-20% efficiency loss vs. uninterrupted agent |

### Recommendation: Tiered approach

1. **Tier 1 (free, always available):** Disk state + stream.jsonl + output log. Triage LLM generates summary post-mortem. This is the current `## Previous Attempt Recovery` pattern.

2. **Tier 2 (cheap, for long tasks):** Agent periodically writes a structured checkpoint file (`checkpoint.md` in its output dir) with: accomplishments, current status, next steps, blockers. Triggered every 10 minutes or 10 tool calls.

3. **Tier 3 (best, for Claude executor):** `claude --resume <session-id>` — zero-cost, full context preservation. Falls back to Tier 1/2 if session expired.

## Summary of Recommendations

1. **Don't build a replay system.** Event sourcing for LLM agents gives audit trails, not state reconstruction.
2. **Full checkpoints > incremental.** Summarize, don't accumulate diffs.
3. **Hybrid trigger:** Time + event count + pre-death. Pre-death is most critical.
4. **Disk state is your friend.** Most agent state survives death. Focus on generating orientation context, not preserving state.
5. **The handoff document IS sufficient** for most tasks, especially when combined with file state inspection.
6. **Tiered approach:** Free (triage summary) → Cheap (periodic checkpoint.md) → Best (session resume).
7. **Integrate with existing triage:** Enhance `## Previous Attempt Recovery` with `git diff` summary and last-known plan.

## Concrete Enhancement to Current System

The existing triage recovery context (`triage.rs:480-497`) should be enhanced:

```
## Previous Attempt Recovery
A previous agent worked on this task but died before completing.

**What was accomplished:** {triage_summary}

**Files changed:** {git_diff_stat}

**Last known activity:** {last_5_stream_events_summary}

**Checkpoint notes:** {contents_of_checkpoint.md_if_exists}

Continue from where the previous agent left off. Do NOT redo completed work.
Check existing artifacts and git diff before starting.
```

This is cheap (adds ~100-200 tokens to the prompt) and dramatically improves successor orientation.
