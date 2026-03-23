# Compaction Metrics and Visibility Surface

_Research task: compact-viz-research | 2026-03-23_

---

## 1. What Triggers Compaction?

Compaction is **cycle-driven** — the old timer/ops-threshold path (`should_compact()`) is deprecated and no longer called by the daemon (see `src/service/compactor.rs:67-72`).

The actual trigger path (`src/commands/service/mod.rs:1368-1405`):

1. **Cycle readiness**: `.compact-0` must be `Open` and all its dependencies (`after: [".coordinator-0"]`) must be in a terminal state via cycle-aware readiness check.

2. **Token threshold gate**: Even when cycle-ready, compaction is **deferred** until `CoordinatorState.accumulated_tokens >= effective_compaction_threshold()`.

### Token threshold calculation (`src/config.rs:2276`)
- **Dynamic (default)**: `context_window * compaction_threshold_ratio`  
  Example: opus (200k window) × 0.8 = 160,000 tokens
- **Fallback**: hardcoded `compaction_token_threshold` config value (used when ratio is 0.0 or model not in registry)

### Token accumulation (`src/commands/service/coordinator_agent.rs:688-708`)
Per coordinator agent LLM turn: `accumulated += cache_creation_input_tokens + input_tokens + output_tokens`  
Reset to 0 after successful compaction.

> **Note**: The old tick-interval (`compactor_interval`) and ops-threshold (`compactor_ops_threshold`) config values are still parsed but are no-ops — the daemon never calls `should_compact()` anymore.

---

## 2. Metrics Available During Compaction

### Data we have today

| Metric | Source | Location |
|--------|--------|----------|
| `accumulated_tokens` | `CoordinatorState.accumulated_tokens` | `.workgraph/service/coordinator-state.json` |
| `threshold` | `config.effective_compaction_threshold()` | Computed from config + model registry |
| `percent` | `accumulated / threshold * 100` | Derived |
| `last_compaction` | `CompactorState.last_compaction` | `.workgraph/compactor/state.json` |
| `compaction_count` | `CompactorState.compaction_count` | `.workgraph/compactor/state.json` |
| `loop_iteration` | `.compact-0.loop_iteration` | `graph.jsonl` |
| Current state | `.compact-0.status` | `graph.jsonl` (Open/InProgress/Done) |
| Compaction duration | `.compact-0.started_at` + `.compact-0.completed_at` | `graph.jsonl` (already set, just need to subtract) |
| Context.md size | `wc -c .workgraph/compactor/context.md` | Filesystem |

**File refs**: `src/service/compactor.rs:36-65` (CompactorState), `src/commands/service/mod.rs:308-340` (CoordinatorState), `src/commands/status.rs:409-433` (gather_compaction_info already pulls all of this)

### Data that needs new instrumentation

| Metric | Gap | Work required |
|--------|-----|---------------|
| Compaction LLM token usage | `LlmCallResult.token_usage` is returned by `run_compaction()` but **discarded** | Store to `CompactorState.last_compaction_tokens` |
| Context.md token count | Size in bytes is known; token count requires estimation | Add byte count to `CompactorState` or estimate as `bytes / 4` |
| Whether threshold check is alive | Logged as INFO but not surfaced to TUI | Read `CoordinatorState` in viz (already done for the header gauge) |

---

## 3. Where Does the `.compact-0` Cycle Live?

### Cycle topology (`src/commands/service/mod.rs:1236-1237`)
```
.coordinator-0 → .compact-0 → .coordinator-0
```
Both tasks also have `.archive-0` as a parallel sibling (same pattern).

### Task properties
- `.coordinator-0`: tag `coordinator-loop`, `after: [".compact-0", ".archive-0"]`, `CycleConfig { max_iterations: 0 (unlimited), no_converge: true }`
- `.compact-0`: tag `compact-loop`, `after: [".coordinator-0"]`, no CycleConfig (coordinator drives the cycle, not the task itself)

### State tracking
| State | Where | When |
|-------|-------|------|
| `.compact-0.status` | `graph.jsonl` | Open (idle), InProgress (running), Done (just finished) |
| `accumulated_tokens` | `coordinator-state.json` | Incremented each coordinator LLM turn; reset to 0 on compaction success |
| `CompactorState` | `compactor/state.json` | Updated after each successful compaction |
| `loop_iteration` | `graph.jsonl` `.compact-0.loop_iteration` | Incremented on each compaction completion |

### Daemon interaction (`src/commands/service/mod.rs:1373-1500`)
`run_graph_compaction()` is called by the service daemon's main loop. It:
1. Checks cycle-readiness of `.compact-0`
2. Gates on `accumulated_tokens >= threshold` (defers if below)
3. Marks `.compact-0` as `InProgress` with `started_at`
4. Calls `run_compaction()` (synchronous, blocking, 120s timeout)
5. On success: resets `accumulated_tokens = 0`, marks `.compact-0` as `Done` with `completed_at`, increments `loop_iteration`
6. On failure: increments error count, marks `.compact-0` as `Failed`

---

## 4. Current Graph-Viz Rendering

### Node rendering pipeline

Both ASCII viz (`src/commands/viz/ascii.rs:277-437`) and graph-format viz (`src/commands/viz/graph.rs:196-254`) follow the same pattern:

```
format: "{color}{id}{reset}  ({status}{tokens}){delay}{age}{msgs}{phase}{loop_info}"
```

**Coordinator tasks** (`coordinator-loop` tag):
- Detected by `is_coordinator_task()` at `src/commands/viz/mod.rs:174`
- Rendered in **cyan** (`\x1b[36m`)
- `loop_info` = `[turn N]` instead of cycle info

**Compactor task** (`.compact-0`, `compact-loop` tag):
- **No special rendering today** — falls through to default status-based coloring
- Gets the generic cycle info: `↺ (iter N)` from `loop_iteration`
- Status color: green (Done), yellow (InProgress), white (Open)

### Annotation system

`VizOutput.annotation_map: HashMap<String, AnnotationInfo>` — this is the primary extension point. It currently powers agency phase badges like `[⊞ assigning]`. Any task ID can receive an annotation that appears after other suffixes as `phase_info`.

**Pink color override** exists for agency annotations (placing/assigning/evaluating/validating/verifying) at `src/commands/viz/ascii.rs:322-337`. No equivalent for compactor.

### TUI-specific rendering

1. **Header compaction gauge** (`src/tui/viz_viewer/render.rs:5174-5183`): Already renders `C:23k/160k(14%)` in blue (red when ≥80%). Requires `show_compact` in `COUNTERS` config.
2. **Coordinator line highlight** (`render.rs:939-943`): Applies dark cyan background to coordinator task rows when Chat tab is active. No equivalent for compactor.

---

## 5. What Would 'Compaction Progress' Look Like?

Compaction is a **single blocking LLM call** (120-second timeout, `src/service/compactor.rs:132-138`). There is no multi-step structure — it's one synchronous API call or subprocess.

| Phase | `.compact-0` status | `accumulated_tokens` | Meaning |
|-------|---------------------|----------------------|---------|
| Deferred | Open | < threshold | Cycle-ready but waiting for more tokens |
| Queued | Open | ≥ threshold | Next daemon tick will run compaction |
| Running | InProgress | ≥ threshold | LLM call in progress (no sub-steps) |
| Complete | Done | 0 (reset) | context.md written, cycle resets |
| Error | Failed | unchanged | LLM call failed; daemon will retry |

**Progress during InProgress**: none beyond "waiting for LLM response." No streaming, no percent-done. The only signal is elapsed time since `started_at`.

---

## 6. Recommendation: Compactor Node Annotation

### Proposed annotation format

```
.compact-0  (done) ↺ 2852  [C: 23k/160k 14%]
```

When InProgress:
```
.compact-0  (in-progress) ↺ 2852  [C: compacting…]
```

**Breakdown**:
- `[C:]` — compactor badge (mirrors agency `[⊞ assigning]` pattern)
- `23k/160k` — accumulated/threshold (abbreviated)
- `14%` — percentage fill
- `compacting…` when InProgress (no sub-step progress available)

**Color**: amber/yellow when ≥80% fill, blue otherwise. Red at 100% (missed threshold — shouldn't happen but possible in deferred state).

### What would need to be built

1. **`is_compactor_task()`** function at `src/commands/viz/mod.rs` (mirror of `is_coordinator_task()`, check for `compact-loop` tag)

2. **Load compaction metrics in viz generation** — pass `CompactorState` and `CoordinatorState` into `generate_ascii_graph()` or equivalent. Currently these are loaded separately for the TUI header gauge; they need to reach the node renderer too.

3. **Inject annotation** for `.compact-0` into `annotation_map` before calling the viz pipeline. The `AnnotationInfo` struct at `src/commands/viz/mod.rs:16-21` already supports the right shape.

4. **(Optional) Store LLM token usage** from compaction to `CompactorState` — gives an accurate "tokens consumed to compact" metric. Currently discarded in `run_compaction()`.

### Implementation sketch (pseudo-code)

```rust
// In the viz pipeline, after loading graph:
if let Some(compact_task) = graph.get_task(".compact-0") {
    let cs = CoordinatorState::load_or_default(dir);
    let compactor = CompactorState::load(dir);
    let config = Config::load_or_default(dir);
    let threshold = config.effective_compaction_threshold();
    
    let annotation_text = if compact_task.status == Status::InProgress {
        "[C: compacting…]".to_string()
    } else if threshold > 0 {
        let pct = (cs.accumulated_tokens as f64 / threshold as f64 * 100.0) as u64;
        format!("[C: {}/{}  {}%]",
            format_tokens(cs.accumulated_tokens),
            format_tokens(threshold),
            pct)
    } else {
        format!("[C: ↻{}]", compactor.compaction_count)
    };
    
    annotation_map.insert(".compact-0".to_string(), AnnotationInfo {
        text: annotation_text,
        dot_task_ids: vec![".compact-0".to_string()],
    });
}
```

---

## Validation Checklist

- [x] **Q1 — Trigger logic**: Cycle-driven via `.compact-0` readiness + token threshold gate (`src/commands/service/mod.rs:1368-1404`)
- [x] **Q2 — Available metrics**: `accumulated_tokens`, `threshold`, `%`, `last_compaction`, `compaction_count`, `loop_iteration`, status, duration (from timestamps), context.md size
- [x] **Q3 — `.compact-0` cycle location**: `.coordinator-0 → .compact-0 → .coordinator-0`, daemon-managed, state in `coordinator-state.json` + `compactor/state.json` + `graph.jsonl`
- [x] **Q4 — Viz rendering**: Coordinator gets cyan + `[turn N]`; compactor gets default status color + generic `↺ (iter N)`; annotation_map is the extension hook
- [x] **Q5 — Progress model**: Binary (waiting/running/done); single LLM call with no sub-steps; percentage to threshold is the only progress signal before the call starts

### Data we have (no code changes needed)
- accumulated_tokens, threshold, percent, last_compaction, compaction_count, loop_iteration, current status, duration (from task timestamps)

### Data we need to add (small instrumentation)
- Compaction LLM token usage (store `LlmCallResult.token_usage` back to `CompactorState`)
- Context.md byte count (store in `CompactorState` after write)
