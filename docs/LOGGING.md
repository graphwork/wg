# Logging & Provenance System

The logging system records every graph-mutating operation as a structured JSONL entry, and archives agent conversation artifacts (prompts and outputs) for completed tasks. Together, these provide a complete audit trail of how a project evolved.

## What Is Logged

Two categories of data are captured:

1. **Operation log** — a JSONL append-only log of every graph mutation (task add, done, fail, edit, claim, etc.)
2. **Agent conversation archives** — copies of each agent's prompt and output, preserved permanently when a task completes or fails

## Directory Structure

```
.wg/
├── log/
│   ├── operations.jsonl          # Current (unrotated) operation log
│   ├── 20260218T153045.123456Z.jsonl.zst  # Rotated, zstd-compressed
│   ├── 20260219T080012.654321Z.jsonl.zst  # Another rotated file
│   └── agents/
│       ├── my-task/
│       │   └── 2026-02-18T15:30:45Z/
│       │       ├── prompt.txt     # Agent's input prompt
│       │       └── output.txt     # Agent's output
│       └── another-task/
│           ├── 2026-02-18T16:00:00Z/   # First attempt
│           │   ├── prompt.txt
│           │   └── output.txt
│           └── 2026-02-18T17:00:00Z/   # Retry attempt
│               ├── prompt.txt
│               └── output.txt
```

## Operation Log Format

Each line in `operations.jsonl` is a JSON object:

```json
{"timestamp":"2026-02-18T15:30:45.123456789+00:00","op":"add_task","task_id":"my-task","actor":"agent-1","detail":{"title":"Implement feature X"}}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | string | RFC 3339 UTC timestamp of the operation |
| `op` | string | Operation name (see table below) |
| `task_id` | string? | ID of the affected task (null for global operations) |
| `actor` | string? | Who performed the operation (agent ID, user name) |
| `detail` | object | Operation-specific payload (null if no extra data) |

### Operations

| Operation | Trigger | Detail fields |
|-----------|---------|---------------|
| `add_task` | `wg add` | `{"title": "..."}` |
| `edit` | `wg edit` (when changes made) | null |
| `done` | `wg done` | null |
| `fail` | `wg fail` | `{"reason": "..."}` or null |
| `abandon` | `wg abandon` | `{"reason": "..."}` or null |
| `retry` | `wg retry` | `{"attempt": N}` |
| `claim` | `wg claim` | null (actor field has claimant) |
| `unclaim` | `wg unclaim` | null |
| `pause` | `wg pause` | null |
| `resume` | `wg resume` | null |
| `archive` | `wg archive` | null (one entry per archived task) |
| `gc` | `wg gc` | null (one entry per gc'd task) |

## Agent Conversation Archive

When a task completes (`wg done`) or fails (`wg fail`), the system automatically archives the agent's working files:

- **Source**: `.wg/agents/<agent-id>/prompt.txt` and `output.log`
- **Destination**: `.wg/log/agents/<task-id>/<ISO-timestamp>/`
- The output file is renamed from `output.log` to `output.txt` in the archive

Each retry gets its own timestamped subdirectory, so the full history of attempts is preserved even if a task fails and is retried multiple times.

## Log Rotation

The operation log uses size-based rotation with zstd compression:

1. Before each append, the system checks if `operations.jsonl` exceeds the rotation threshold
2. If so, the file is compressed with zstd (level 3) and renamed to `<UTC-timestamp>.jsonl.zst`
3. A fresh empty `operations.jsonl` is created
4. The threshold defaults to **10 MB** and is configurable in `.wg/config.toml`:

```toml
[log]
rotation_threshold = 10485760  # bytes (default: 10 MB)
```

### Reading Rotated Files

`wg log --operations` automatically reads across all rotated files in chronological order. To manually inspect a rotated file:

```bash
# Decompress and view
zstd -d .wg/log/20260218T153045.123456Z.jsonl.zst --stdout | less

# Or use zstdcat
zstdcat .wg/log/20260218T153045.123456Z.jsonl.zst | head
```

### Concurrent Write Safety

Each operation is written as a single `write_all()` call to a file opened with `O_APPEND`. On Linux, this guarantees atomicity for writes under `PIPE_BUF` (4096 bytes). Since each JSONL line is typically under 500 bytes, concurrent agents writing to the same operation log will not produce corrupted entries.

## CLI Commands

### View operation log

```bash
# Human-readable format
wg log --operations

# JSON output (for scripting)
wg log --operations --json
```

### View agent archives

```bash
# Show archived prompts and outputs for a task
wg log --agent <task-id>

# JSON output
wg log --agent <task-id> --json
```

### View task-level log entries

```bash
# Show progress log entries for a task
wg log <task-id> --list

# JSON output
wg log <task-id> --list --json
```

## Replaying Project Workflows

The operation log enables replaying the full history of a project's task graph evolution. Since every mutation is recorded with timestamps and actor information, you can:

1. **Reconstruct the graph at any point in time** by replaying operations up to a given timestamp
2. **Compare model performance** by examining which agents (identified by `actor` field) completed tasks successfully vs. failed, and how many retries were needed
3. **Analyze workflow patterns** by examining the sequence of operations, identifying bottlenecks, and understanding how work was distributed across agents
4. **Audit changes** with a complete provenance trail from task creation through completion

This is particularly useful for research workflows where you want to run the same project with different models and compare outcomes. The operation log provides the ground truth for what happened, independent of the graph's current state.

## Storage Estimates

| Component | Typical size per entry | Notes |
|-----------|----------------------|-------|
| Operation log entry | 100–300 bytes | Depends on detail payload |
| Rotated compressed file | ~60% of raw size | zstd level 3 compression |
| Agent prompt archive | 1–50 KB | Varies by task complexity |
| Agent output archive | 1–200 KB | Depends on agent verbosity |

**Rough estimates for a 1000-task project:**

- Operation log: ~500 KB raw (3–5 operations per task × 200 bytes average)
- After rotation/compression: ~300 KB
- Agent archives: 10–100 MB (depending on task complexity and retries)

## Retention Policy

The operation log is append-only and never automatically deleted. Rotated files are compressed but retained indefinitely. To manage disk usage:

- Use `wg archive` to move completed tasks out of the active graph (their operation log entries remain)
- Use `wg gc` to remove failed/abandoned tasks from the graph (their operation log entries remain)
- Manually delete old `.jsonl.zst` files from `.wg/log/` if space is a concern
- Agent archives under `.wg/log/agents/` can be pruned manually for old tasks
