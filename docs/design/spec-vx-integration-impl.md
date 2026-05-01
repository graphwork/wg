# VX Integration Implementation Specification

**Date:** 2026-02-21
**Source:** [vx-integration-response.md](vx-integration-response.md) (sections 6, 7, 8)
**Scope:** Six concrete changes to workgraph core enabling VX adapter integration

---

## 1. `Evaluation.source` Field

### Problem

All evaluations are currently LLM-generated. There is no way to distinguish an internal auto-evaluation from an external outcome score, a manual review, or a VX peer score. The `Evaluation` struct has no `source` field.

### Struct Changes

**File:** `src/agency.rs`, `Evaluation` struct (line ~202)

Add a `source` field after `model`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub id: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    pub role_id: String,
    pub motivation_id: String,
    pub score: f64,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub dimensions: HashMap<String, f64>,
    pub notes: String,
    pub evaluator: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Source of this evaluation. Convention: "llm" (auto-evaluator), "manual",
    /// "outcome:<metric>" (e.g. "outcome:sharpe"), "vx:<peer-id>".
    /// Defaults to "llm" for backward compatibility with existing evaluation files.
    #[serde(default = "default_eval_source")]
    pub source: String,
}
```

Add the default function:

```rust
fn default_eval_source() -> String {
    "llm".to_string()
}
```

**Backward compatibility:** The `#[serde(default = "default_eval_source")]` ensures existing evaluation JSON files (which lack a `source` field) deserialize with `source: "llm"`. No migration needed.

### CLI Changes

**File:** `src/main.rs`, `Commands::Evaluate` variant

Currently `wg evaluate` is a flat command. It needs to become a subcommand group to support `evaluate record`, `evaluate show`, and the existing auto-evaluation behavior (renamed to `evaluate run`):

```rust
/// Evaluate tasks: auto-evaluate, record external scores, view history
Evaluate {
    #[command(subcommand)]
    command: EvaluateCommands,
},
```

```rust
#[derive(Subcommand)]
enum EvaluateCommands {
    /// Trigger LLM-based evaluation of a completed task
    Run {
        /// Task ID to evaluate
        task: String,
        /// Model to use for the evaluator
        #[arg(long)]
        evaluator_model: Option<String>,
        /// Show what would be evaluated without spawning the evaluator
        #[arg(long)]
        dry_run: bool,
    },

    /// Record an evaluation from an external source
    Record {
        /// Task ID
        #[arg(long)]
        task: String,
        /// Overall score (0.0-1.0)
        #[arg(long)]
        score: f64,
        /// Source identifier (e.g. "outcome:sharpe", "vx:peer-abc", "manual")
        #[arg(long)]
        source: String,
        /// Optional notes
        #[arg(long)]
        notes: Option<String>,
        /// Optional dimensional scores (repeatable, format: dimension=score)
        #[arg(long = "dim", num_args = 1)]
        dimensions: Vec<String>,
    },

    /// Show evaluation history
    Show {
        /// Filter by task ID (prefix match)
        #[arg(long)]
        task: Option<String>,
        /// Filter by agent ID (prefix match)
        #[arg(long)]
        agent: Option<String>,
        /// Filter by source (exact match or glob, e.g. "outcome:*")
        #[arg(long)]
        source: Option<String>,
        /// Show only the N most recent evaluations
        #[arg(long)]
        limit: Option<usize>,
    },
}
```

**Migration note:** Existing `wg evaluate <task>` invocations break. For backward compatibility, detect bare positional args and treat `wg evaluate <task>` as `wg evaluate run <task>` with a deprecation warning. Alternatively, accept the breaking change since this is pre-1.0.

### Command Implementation

#### `evaluate record`

**File:** New function in `src/commands/evaluate.rs`

```rust
pub fn run_record(
    dir: &Path,
    task_id: &str,
    score: f64,
    source: &str,
    notes: Option<&str>,
    dimensions: &[String],
    json: bool,
) -> Result<()>
```

Logic:
1. Validate `score` is in `[0.0, 1.0]` range.
2. Load graph, find task by ID.
3. Resolve agent assignment from `task.agent` field — look up the agent to get `role_id` and `motivation_id`. If no agent assigned, use empty strings for `agent_id`, `role_id`, `motivation_id` (external evaluations may not have agent context).
4. Parse `dimensions` strings (format `key=value`, e.g. `correctness=0.8`).
5. Build `Evaluation` struct with:
   - `id`: generated UUID or content hash
   - `source`: the provided source string
   - `evaluator`: set to `source` value (since there's no LLM evaluator)
   - `model`: `None`
   - `timestamp`: current RFC 3339 timestamp
6. Call `record_evaluation()` to save and propagate to agent/role/motivation performance.
7. Record provenance: `provenance::record(dir, "evaluate_record", Some(task_id), Some("external"), detail)`.
8. Output result (human-readable or JSON).

#### `evaluate show`

**File:** New function in `src/commands/evaluate.rs`

```rust
pub fn run_show(
    dir: &Path,
    task_filter: Option<&str>,
    agent_filter: Option<&str>,
    source_filter: Option<&str>,
    limit: Option<usize>,
    json: bool,
) -> Result<()>
```

Logic:
1. Read all evaluation files from `.wg/agency/evaluations/`.
2. Deserialize each as `Evaluation`.
3. Apply filters:
   - `--task`: prefix match on `evaluation.task_id`
   - `--agent`: prefix match on `evaluation.agent_id`
   - `--source`: glob match on `evaluation.source` (support `*` wildcards, e.g. `outcome:*` matches `outcome:sharpe`, `outcome:mse`)
4. Sort by timestamp descending.
5. Apply `--limit` if specified.
6. Output as table (human) or JSON array.

Human-readable output format:
```
Task            Score  Source          Agent     Timestamp
────────────────────────────────────────────────────────────────
portfolio-q1    0.72   outcome:sharpe  scout     2026-02-20T14:30:00Z
portfolio-q1    0.91   llm             scout     2026-02-20T12:15:00Z
fix-auth-bug    0.85   llm             builder   2026-02-19T09:00:00Z
```

#### Propagation through `record_evaluation()`

**File:** `src/agency.rs`, `record_evaluation()` function (line ~1188)

No changes needed to the function signature or logic. The `Evaluation` struct now carries `source`, and `record_evaluation()` already saves the full struct to JSON and updates performance records. The `source` field propagates automatically via serde serialization.

However, consider whether external evaluations (non-LLM) should propagate to agent/role/motivation performance records differently. **Recommendation:** Propagate all sources equally for now. The evolver can weight by source when reading the aggregate picture. Keep `record_evaluation()` source-agnostic.

### Test Cases

1. **Backward compatibility:** Deserialize an existing evaluation JSON file (no `source` field) and assert `source == "llm"`.
2. **Record with source:** `wg evaluate record --task t1 --score 0.72 --source "outcome:sharpe"` → verify JSON file has `source: "outcome:sharpe"`.
3. **Record with dimensions:** `wg evaluate record --task t1 --score 0.8 --source manual --dim correctness=0.9 --dim efficiency=0.7` → verify dimensions map.
4. **Show all:** `wg evaluate show` → lists all evaluations sorted by timestamp.
5. **Show filtered by source:** `wg evaluate show --source "outcome:*"` → only outcome evaluations.
6. **Show filtered by task:** `wg evaluate show --task portfolio` → prefix match.
7. **Show with limit:** `wg evaluate show --limit 5` → at most 5 results.
8. **Show JSON:** `wg evaluate show --json` → valid JSON array output.
9. **Performance propagation:** After `evaluate record`, verify agent/role/motivation performance records are updated.
10. **Provenance:** After `evaluate record`, verify operation log contains `evaluate_record` entry with source in detail.

---

## 2. `Task.visibility` Field

### Problem

There is no way to control which tasks cross organizational boundaries in trace exports. All tasks are equally visible, preventing sanitized sharing.

### Struct Changes

**File:** `src/graph.rs`, `Task` struct (line ~148)

Add `visibility` field after `paused`:

```rust
/// Visibility zone for trace exports. Controls what crosses organizational boundaries.
/// Values: "internal" (default, org-only), "public" (sanitized sharing),
/// "peer" (richer view for credentialed peers).
#[serde(default = "default_visibility", skip_serializing_if = "is_default_visibility")]
pub visibility: String,
```

Add helper functions:

```rust
fn default_visibility() -> String {
    "internal".to_string()
}

fn is_default_visibility(val: &str) -> bool {
    val == "internal"
}
```

**Why `String` and not an enum:** A String with conventional values keeps the field extensible without code changes. If a fourth zone is needed (e.g. "interface" as nikete's design suggests), no enum variant needs adding. Validation happens at the CLI boundary.

**Custom deserializer update:** The existing custom `Deserialize` impl for `Task` (line ~306) must also handle the new `visibility` field. Add it alongside other fields in the manual deserialization, with `default_visibility()` as the fallback.

**TaskDetails update:** The `TaskDetails` struct in `src/commands/show.rs` (line ~19) must also include `visibility`:

```rust
#[serde(default = "default_visibility", skip_serializing_if = "is_default_visibility")]
pub visibility: String,
```

### CLI Changes

**File:** `src/main.rs`, `Commands::Add` variant

Add `--visibility` flag:

```rust
/// Task visibility zone for trace exports (internal, public, peer)
#[arg(long, default_value = "internal")]
visibility: String,
```

**File:** `src/main.rs`, `Commands::Edit` variant

Add `--visibility` flag to allow changing visibility after creation:

```rust
/// Set task visibility zone (internal, public, peer)
#[arg(long)]
visibility: Option<String>,
```

### Command Implementation

#### `wg add --visibility`

**File:** `src/commands/add.rs`, `run()` function

1. Accept `visibility: &str` parameter.
2. Validate value is one of `"internal"`, `"public"`, `"peer"`. Return error for unknown values.
3. Set `task.visibility = visibility.to_string()`.

#### `wg show` display

**File:** `src/commands/show.rs`

In the human-readable output function, add visibility display after the status line when it's not "internal" (to reduce noise for the common case):

```
Task: portfolio-q1
Status: done
Visibility: public      ← only shown when not "internal"
```

In the JSON output (`TaskDetails`), always include it.

#### `wg edit --visibility`

**File:** `src/commands/edit.rs`

Handle the new `--visibility` flag: validate and set `task.visibility`.

### Test Cases

1. **Default visibility:** Create task with `wg add "test"` → verify `visibility` is `"internal"` in YAML.
2. **Explicit visibility:** `wg add "public task" --visibility public` → verify `visibility: "public"` in YAML.
3. **Invalid visibility:** `wg add "bad" --visibility secret` → error message listing valid values.
4. **Edit visibility:** `wg edit task-1 --visibility peer` → verify field updated.
5. **Show display:** `wg show task-1` shows visibility when not internal.
6. **JSON output:** `wg show task-1 --json` includes `visibility` field.
7. **Backward compatibility:** Load a task YAML file without `visibility` field → defaults to `"internal"`.

---

## 3. `wg trace export --visibility <zone>`

### Problem

There is no way to produce a shareable, filtered view of workgraph data. Full trace data includes internal tasks, agent logs, and operational details that shouldn't cross organizational boundaries.

### CLI Changes

**File:** `src/main.rs`, `TraceCommands` enum

Add `Export` variant:

```rust
/// Export trace data filtered by visibility zone
Export {
    /// Root task ID (exports this task and all descendants)
    #[arg(long)]
    root: Option<String>,

    /// Visibility zone filter: "internal" (everything), "public" (sanitized),
    /// "peer" (richer for credentialed peers). Default: "internal".
    #[arg(long, default_value = "internal")]
    visibility: String,

    /// Output file path (default: stdout)
    #[arg(long, short = 'o')]
    output: Option<String>,
},
```

### Data Format

The export produces a JSON document with this structure:

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct TraceExport {
    /// Export metadata
    pub metadata: ExportMetadata,
    /// Task graph (filtered by visibility)
    pub tasks: Vec<ExportedTask>,
    /// Evaluations for included tasks
    pub evaluations: Vec<Evaluation>,
    /// Sanitized operation log entries for included tasks
    pub operations: Vec<OperationEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportMetadata {
    /// Workgraph version that produced this export
    pub version: String,
    /// Timestamp of export
    pub exported_at: String,
    /// Visibility zone used for filtering
    pub visibility: String,
    /// Root task ID if scoped export
    pub root_task: Option<String>,
    /// Source identifier (e.g. org name, peer ID)
    pub source: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportedTask {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: Status,
    pub visibility: String,
    pub skills: Vec<String>,
    pub blocked_by: Vec<String>,
    pub blocks: Vec<String>,
    pub tags: Vec<String>,
    pub artifacts: Vec<String>,
    pub created_at: Option<String>,
    pub completed_at: Option<String>,
    /// Only included for peer/internal exports
    pub agent: Option<String>,
    /// Only included for peer/internal exports
    pub log: Vec<LogEntry>,
}
```

### Command Implementation

**File:** New `src/commands/trace_export.rs`

```rust
pub fn run(
    dir: &Path,
    root: Option<&str>,
    visibility: &str,
    output: Option<&str>,
    json_flag: bool,
) -> Result<()>
```

Logic:

1. Validate `visibility` is one of `"internal"`, `"public"`, `"peer"`.
2. Load the full task graph.
3. **Scope selection:** If `root` is provided, collect the root task and all descendants (tasks blocked by root, transitively). If no root, include all tasks.
4. **Visibility filtering:**
   - `"internal"`: Include all tasks in scope (no filtering).
   - `"public"`: Include only tasks with `visibility == "public"`. Strip agent output, log entries, and agent assignment from exported tasks.
   - `"peer"`: Include tasks with `visibility == "public"` OR `visibility == "peer"`. Include evaluations and agent lineage but strip raw agent logs/output.
5. **Load evaluations:** Read all evaluation files from `.wg/agency/evaluations/`. Filter to only evaluations whose `task_id` matches an included task.
   - For `"public"` exports: exclude evaluations entirely.
   - For `"peer"` exports: include evaluations but strip `notes` field (may contain internal detail).
   - For `"internal"` exports: include everything.
6. **Load operations:** Read provenance log via `provenance::read_all_operations()`. Filter to operations whose `task_id` matches an included task.
   - For `"public"` exports: include only structural operations (`add_task`, `done`, `fail`) — strip `detail` field.
   - For `"peer"` exports: include all operation types, keep `detail`.
   - For `"internal"` exports: include everything.
7. **Build `TraceExport`** struct and serialize to JSON.
8. Write to output file or stdout.
9. Record provenance: `provenance::record(dir, "trace_export", root, Some("user"), detail)` where detail includes visibility and task count.

### Filtering Matrix

| Content              | `internal` | `public`        | `peer`          |
|----------------------|------------|-----------------|-----------------|
| Task descriptions    | All        | public only     | public + peer   |
| Task status          | All        | public only     | public + peer   |
| Task logs            | All        | Stripped        | Stripped         |
| Agent assignment     | All        | Stripped        | Included        |
| Evaluations          | All        | Excluded        | Included (notes stripped) |
| Operations (structural) | All     | Included (no detail) | Included    |
| Operations (agent)   | All        | Excluded        | Included        |
| Artifacts list       | All        | Included        | Included        |

### Test Cases

1. **Full internal export:** `wg trace export` → all tasks, evaluations, operations included.
2. **Public export:** Create tasks with mixed visibility. `wg trace export --visibility public` → only public tasks, no evaluations, no logs, no agent assignment.
3. **Peer export:** `wg trace export --visibility peer` → public + peer tasks, evaluations included (notes stripped), agent assignment included.
4. **Scoped export:** `wg trace export --root task-1` → only task-1 and descendants.
5. **Output file:** `wg trace export -o export.json` → writes to file.
6. **Empty export:** All tasks are internal, export with `--visibility public` → valid JSON with empty tasks array.
7. **Provenance recording:** Export records an operation log entry.
8. **Round-trip:** Export then import (see §4) preserves data.

---

## 4. `wg trace import`

### Problem

There is no way to ingest a peer's trace export as read-only context. Teams can export but not consume each other's work products.

### CLI Changes

**File:** `src/main.rs`, `TraceCommands` enum

Add `Import` variant:

```rust
/// Import a trace export file as read-only context
Import {
    /// Path to the trace export JSON file
    file: String,

    /// Source tag for imported data (e.g. "peer:alice", "team:platform")
    #[arg(long)]
    source: Option<String>,

    /// Show what would be imported without making changes
    #[arg(long)]
    dry_run: bool,
},
```

### Command Implementation

**File:** New `src/commands/trace_import.rs`

```rust
pub fn run(
    dir: &Path,
    file: &str,
    source: Option<&str>,
    dry_run: bool,
    json: bool,
) -> Result<()>
```

Logic:

1. Read and deserialize the file as `TraceExport`.
2. Validate the export format (check `metadata.version`).
3. Determine source tag: use `--source` if provided, else use `metadata.source` if present, else use the filename stem.
4. **Import tasks as read-only context:**
   - Prefix imported task IDs with `imported/<source>/` to namespace them and prevent ID collisions (e.g. `imported/peer-alice/portfolio-q1`).
   - Set `status: Done` on all imported tasks (they represent completed work, not actionable items).
   - Add tag `imported` and tag `source:<source>` to each task.
   - Set `visibility: "internal"` on imported tasks (they're now local context).
   - **Do NOT** add imported tasks to the main graph's dependency structure — they exist as reference, not as blocking/blocked relationships.
   - Store imported tasks in `.wg/imports/<source>/tasks.yaml` (separate from the main graph file to prevent accidental modification).
5. **Import evaluations:**
   - Write evaluation files to `.wg/agency/evaluations/` with an `imported-` prefix on filenames.
   - Set `source` field to `"import:<original-source>"` (e.g. if original source was `"outcome:sharpe"`, imported version becomes `"import:outcome:sharpe"`).
   - **Do NOT** propagate imported evaluations to local agent/role/motivation performance records. These are reference data, not performance signals for local agents.
6. **Import operations:**
   - Append to a separate import log: `.wg/imports/<source>/operations.jsonl`.
   - Do not mix with the local provenance log.
7. **Dry run:** If `--dry-run`, print summary of what would be imported (task count, evaluation count, operation count) without writing anything.
8. Record provenance: `provenance::record(dir, "trace_import", None, Some("user"), detail)` where detail includes source, file path, and counts.
9. Output summary.

### Context Integration

Imported data should be accessible to agents via the existing context system:

- `wg context <task-id>` could optionally include relevant imported context when a task's skills or tags overlap with imported tasks.
- This is a future enhancement — for the initial implementation, imported data is queryable via `wg list --tag imported` or by reading the import directory directly.

### Test Cases

1. **Basic import:** Export from one workgraph, import into another → imported tasks appear with namespaced IDs.
2. **Dry run:** `wg trace import export.json --dry-run` → prints summary, no files written.
3. **Source tagging:** `wg trace import export.json --source "peer:alice"` → tasks tagged `source:peer:alice`.
4. **ID namespacing:** Imported task `portfolio-q1` becomes `imported/peer-alice/portfolio-q1` — no collision with local task `portfolio-q1`.
5. **Read-only:** Imported tasks don't appear in `wg ready` or `wg list --status open`.
6. **Evaluation isolation:** Imported evaluations don't update local agent performance records.
7. **Provenance:** Import records an operation log entry.
8. **Re-import:** Importing the same file twice with the same source tag either updates existing imports or warns about duplicates (idempotent behavior preferred).
9. **Invalid file:** Importing a non-JSON or malformed file → clear error message.

---

## 5. `wg watch --json`

### Problem

External systems (VX adapter, CI, dashboards) have no way to react to workgraph events in real time. They must poll `wg list --json`, which is inefficient and misses transient state changes.

### CLI Changes

**File:** `src/main.rs`, `Commands` enum

Add a new `Watch` command:

```rust
/// Stream workgraph events as JSON lines
Watch {
    /// Filter events by type (repeatable). Types: task_state, evaluation, agent, all.
    #[arg(long = "event", default_value = "all")]
    event_types: Vec<String>,

    /// Filter events to a specific task ID (prefix match)
    #[arg(long)]
    task: Option<String>,

    /// Include N most recent historical events before streaming (default: 0)
    #[arg(long, default_value = "0")]
    replay: usize,
},
```

### Event Format

Each line is a self-contained JSON object:

```rust
#[derive(Debug, Serialize)]
pub struct WatchEvent {
    /// Event type for filtering
    #[serde(rename = "type")]
    pub event_type: String,
    /// RFC 3339 timestamp
    pub timestamp: String,
    /// Associated task ID, if any
    pub task_id: Option<String>,
    /// Event-specific payload
    pub data: serde_json::Value,
}
```

Event types and their data payloads:

| Event Type | Trigger | Data Payload |
|------------|---------|-------------|
| `task.created` | `wg add` | `{ "id", "title", "visibility", "skills", "blocked_by" }` |
| `task.started` | `wg claim` / agent spawn | `{ "id", "assigned" }` |
| `task.completed` | `wg done` | `{ "id", "title" }` |
| `task.failed` | `wg fail` | `{ "id", "reason" }` |
| `task.retried` | `wg retry` | `{ "id", "retry_count" }` |
| `evaluation.recorded` | `wg evaluate run` or `wg evaluate record` | `{ "task_id", "score", "source", "dimensions" }` |
| `agent.spawned` | coordinator spawns agent | `{ "task_id", "agent_id", "pid" }` |
| `agent.completed` | agent process exits | `{ "task_id", "agent_id", "exit_code" }` |

### Command Implementation

**File:** New `src/commands/watch.rs`

```rust
pub fn run(
    dir: &Path,
    event_types: &[String],
    task_filter: Option<&str>,
    replay: usize,
) -> Result<()>
```

Logic:

1. **Source:** Use the provenance operation log (`.wg/log/operations.jsonl`) as the event source. The operation log already records all state-changing operations with timestamps, task IDs, and detail payloads.

2. **Historical replay:** If `--replay N`, read the last N operations from the log, convert each to a `WatchEvent`, apply filters, and emit them.

3. **Live streaming:**
   - Open `operations.jsonl` and seek to end.
   - Enter a polling loop:
     - Check file size / inotify for new data.
     - Read new lines appended since last check.
     - Parse each as `OperationEntry`.
     - Convert to `WatchEvent` by mapping `op` field to event type:
       - `"add_task"` → `task.created`
       - `"claim"` → `task.started`
       - `"done"` → `task.completed`
       - `"fail"` → `task.failed`
       - `"retry"` → `task.retried`
       - `"evaluate_record"` / `"evaluate_auto"` → `evaluation.recorded`
       - `"spawn_agent"` → `agent.spawned`
       - `"agent_complete"` → `agent.completed`
     - Apply event type filter (skip events not in `--event` list).
     - Apply task filter (skip events whose `task_id` doesn't prefix-match `--task`).
     - Write matching `WatchEvent` as JSON line to stdout.
     - Flush stdout after each line (critical for piped consumers).
   - Poll interval: 500ms (configurable via env var `WG_WATCH_POLL_MS`).
   - Exit on SIGINT/SIGTERM or broken pipe (EPIPE).

4. **Operation-to-event mapping:** Not every provenance operation maps to a watch event. Unknown operation types are silently skipped (forward-compatible: new operations added later won't break existing watchers).

### Implementation Notes

- Use `std::io::BufReader` with tail-follow semantics. Since the operation log is append-only, seeking to end and reading new lines is safe.
- Handle log rotation: if the current `operations.jsonl` is rotated (renamed to `.jsonl.zst`), detect the new file and re-open. The provenance module's `rotate()` function renames and creates a new file — watch should detect this via file inode change or size reset.
- Consider using `notify` crate for filesystem events instead of polling, but polling is simpler and sufficient for the initial implementation.

### Test Cases

1. **Basic streaming:** Start `wg watch --json`, add a task in another terminal → event appears on stdout.
2. **Event type filter:** `wg watch --json --event task_state` → only task state events, no evaluation events.
3. **Task filter:** `wg watch --json --task portfolio` → only events for tasks matching prefix.
4. **Replay:** `wg watch --json --replay 10` → last 10 historical events printed, then live streaming continues.
5. **JSON validity:** Each line of output is valid JSON parseable by `jq`.
6. **Broken pipe:** Pipe `wg watch --json` into `head -5` → exits cleanly without error.
7. **Multiple event types:** `wg watch --json --event task_state --event evaluation` → both types included.
8. **Flush behavior:** Events appear immediately, not buffered (test with small poll interval).

---

## 6. Serde Aliases for nikete File Format Compatibility

### Problem

nikete's vx-adapter branch uses different field names for the same concepts. His `Reward` struct uses `value` where we use `score`, and `reasoning` where we use `notes`. Files produced by either fork should be deserializable by the other.

### Field Mapping

| Our field (`Evaluation`) | nikete's field (`Reward`) | Type |
|--------------------------|---------------------------|------|
| `score` | `value` | `f64` |
| `notes` | `reasoning` | `String` |
| `evaluator` | `evaluated_by` | `String` |
| `dimensions` | `dimensions` | same |
| `timestamp` | `timestamp` | same |
| `task_id` | `task_id` | same |
| `agent_id` | `agent_id` | same |
| `source` | `source` | same (new in both) |
| `id` | *(absent in nikete)* | `String` |
| `role_id` | *(absent in nikete)* | `String` |
| `motivation_id` | *(absent in nikete)* | `String` |
| `model` | *(absent in nikete)* | `Option<String>` |

### Struct Changes

**File:** `src/agency.rs`, `Evaluation` struct

Add `#[serde(alias = "...")]` attributes to divergent fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    #[serde(default)]
    pub id: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    #[serde(default)]
    pub role_id: String,
    #[serde(default)]
    pub motivation_id: String,
    #[serde(alias = "value")]
    pub score: f64,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub dimensions: HashMap<String, f64>,
    #[serde(alias = "reasoning")]
    pub notes: String,
    #[serde(alias = "evaluated_by")]
    pub evaluator: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "default_eval_source")]
    pub source: String,
}
```

**Key changes:**
- `score`: add `#[serde(alias = "value")]` — accepts `"value": 0.72` in JSON input
- `notes`: add `#[serde(alias = "reasoning")]` — accepts `"reasoning": "..."` in JSON input
- `evaluator`: add `#[serde(alias = "evaluated_by")]` — accepts `"evaluated_by": "..."` in JSON input
- `id`, `role_id`, `motivation_id`: add `#[serde(default)]` — nikete's `Reward` lacks these fields, so they must default when deserializing his format
- `HashMap` for dimensions: nikete uses `BTreeMap<String, f64>` — serde deserializes both from the same JSON format, so no alias needed

**Serialization:** We always serialize with OUR field names (`score`, `notes`, `evaluator`). The aliases are deserialization-only — serde aliases only affect deserialization, not serialization. This means our output is canonical, but we accept nikete's format as input.

### Test Cases

1. **Deserialize our format:** Standard evaluation JSON with `score`, `notes`, `evaluator` → parses correctly.
2. **Deserialize nikete format:** JSON with `value`, `reasoning`, `evaluated_by`, missing `id`/`role_id`/`motivation_id` → parses correctly with defaults.
3. **Serialize always uses our names:** Serialize an Evaluation → output contains `score`, `notes`, `evaluator` (never `value`, `reasoning`, `evaluated_by`).
4. **Mixed format:** JSON with `score` AND `value` present → serde picks the primary field name (`score`), ignores alias.
5. **Round-trip:** Deserialize nikete format → serialize → output uses our field names.
6. **BTreeMap vs HashMap:** nikete's `dimensions` as `BTreeMap` serialization is identical JSON to our `HashMap` → interoperable.

---

## Implementation Order

The items have these dependencies:

```
1. Evaluation.source  ←─┐
                         ├── 3. trace export (needs source field + visibility field)
2. Task.visibility    ←─┘     │
                               ├── 4. trace import (needs export format)
                               │
5. wg watch --json             (independent)
6. Serde aliases               (independent, but logically pairs with #1)
```

**Recommended order:**

1. **Serde aliases** (§6) — smallest change, no new files, pure struct annotation
2. **Evaluation.source** (§1) — struct change + new `evaluate` subcommands
3. **Task.visibility** (§2) — struct change + CLI flag additions
4. **wg trace export** (§3) — new command, depends on §1 and §2
5. **wg trace import** (§4) — new command, depends on §3's export format
6. **wg watch --json** (§5) — independent, can be parallelized with §3-§4

Items 1-2 and 5-6 can be implemented in parallel by different agents.

---

## Files Modified

| File | Changes |
|------|---------|
| `src/agency.rs` | Add `source` field to `Evaluation`, add `default_eval_source()`, add serde aliases |
| `src/graph.rs` | Add `visibility` field to `Task`, add `default_visibility()`, update custom deserializer |
| `src/main.rs` | Restructure `Evaluate` as subcommand group, add `Watch` command, add `Export`/`Import` to `TraceCommands`, add `--visibility` to `Add`/`Edit` |
| `src/commands/evaluate.rs` | Add `run_record()` and `run_show()` functions, rename existing `run()` to handle subcommand dispatch |
| `src/commands/add.rs` | Accept and validate `visibility` parameter |
| `src/commands/edit.rs` | Handle `--visibility` flag |
| `src/commands/show.rs` | Display `visibility` field, add to `TaskDetails` |

## New Files

| File | Purpose |
|------|---------|
| `src/commands/trace_export.rs` | `wg trace export` implementation |
| `src/commands/trace_import.rs` | `wg trace import` implementation |
| `src/commands/watch.rs` | `wg watch --json` implementation |
