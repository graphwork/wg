# Research: Agent Lifecycle and Kill Mechanics

**Task:** research-agent-kill  
**Date:** 2026-04-13  

---

## 1. How Agents Are Registered and Tracked

### Registry File
- **Path:** `.wg/service/registry.json`
- **Source:** `src/service/registry.rs`

### Data Model (`AgentEntry` — `src/service/registry.rs:60`)
```rust
pub struct AgentEntry {
    pub id: String,           // "agent-7" (auto-incremented)
    pub pid: u32,             // OS process ID
    pub task_id: String,      // Task being worked on
    pub executor: String,     // "claude", "shell", "native"
    pub started_at: String,   // ISO 8601
    pub last_heartbeat: String,
    pub status: AgentStatus,  // Starting|Working|Idle|Stopping|Frozen|Done|Failed|Dead|Parked
    pub output_file: String,  // Path to output.log
    pub model: Option<String>,
    pub completed_at: Option<String>,
}
```

### Registry Structure (`AgentRegistry` — `src/service/registry.rs:120`)
```rust
pub struct AgentRegistry {
    pub agents: HashMap<String, AgentEntry>,  // All agents ever registered (in this session)
    pub next_agent_id: u32,                   // Monotonically increasing counter
}
```

### Registration Flow
1. **Spawn** (`src/commands/spawn/execution.rs:282-330`):
   - `AgentRegistry::load_locked(dir)` — acquires file lock
   - Pre-allocates agent ID: `format!("agent-{}", locked_registry.next_agent_id)`
   - Creates output directory at `.wg/agents/agent-N/`
   - Optionally creates a git worktree for isolation
   - Launches the executor process (gets PID)
   - `register_agent_with_model(pid, task_id, executor, output_file, model)` — inserts entry
   - Claims task in graph: sets `status = InProgress`, `assigned = Some(agent_id)`
   - Saves registry atomically (write-to-temp-then-rename)

2. **Agent ID monotonicity**: `next_agent_id` is incremented via `saturating_add(1)` on every registration (`src/service/registry.rs:312`). IDs are **never reused** — this is why you see agent-16000+ in long-running projects.

3. **Concurrency**: Registry uses `flock(LOCK_EX)` for all write paths (`src/service/registry.rs:217-258`). The lock is held during the entire spawn sequence to prevent two concurrent spawns from getting the same agent ID.

### Key Problem: Dead Agents Accumulate Forever
The registry is append-only in practice:
- `register_agent_with_model()` adds entries
- `unregister_agent()` removes entries — but is **only called from `wg kill`** (`src/commands/kill.rs:48`)
- Triage marks dead agents as `AgentStatus::Dead` but **never removes them** from the HashMap (`src/commands/service/triage.rs:276-283`)
- The coordinator's cleanup marks agents Dead but also never removes them

**Result:** Every agent that ever ran remains in `registry.json`. With thousands of tasks, this file grows unboundedly.

---

## 2. What `wg kill` Does Today

**Source:** `src/commands/kill.rs`

### Single Agent Kill (`kill::run` — line 23)
1. `AgentRegistry::load_locked(dir)` — acquires lock
2. Looks up agent by ID, gets PID and task_id
3. **Kills process:**
   - Default (graceful): `kill_process_graceful(pid, 5)` — sends SIGTERM, waits 5s, then SIGKILL (`src/service/mod.rs:41-80`)
   - `--force`: `kill_process_force(pid)` — sends SIGKILL immediately (`src/service/mod.rs:88-106`)
4. Updates registry status to `AgentStatus::Stopping`
5. **Unclaims the task** (`kill.rs:170-199`):
   - Sets task status back to `Open`
   - Sets `assigned = None`
   - Adds log entry: "Task unclaimed: agent 'X' was killed"
6. **Removes agent from registry** (`unregister_agent` — `registry.rs:346`)
7. Saves registry

### Kill All (`kill::run_all` — line 72)
- Iterates `list_alive_agents()` (only Starting/Working/Idle)
- Same kill + unclaim + unregister for each
- Continues on individual errors

### Key Behaviors
- **Yes, it sends actual signals** (SIGTERM then SIGKILL, or SIGKILL directly)
- **Yes, it cleans up the registry entry** (removes it entirely via `unregister_agent`)
- **Yes, it unclaims the task** (sets back to Open) — **which means the coordinator will respawn it**
- No tree-kill: kills only the specified agent, not any downstream/subtask agents
- No cascade: downstream tasks remain in their current state

---

## 3. How the Coordinator Decides to Respawn Work

### The Coordinator Tick Loop
**Source:** `src/commands/service/coordinator.rs`

Each tick:
1. **Cleanup dead agents** (`cleanup_and_count_alive` — line 43):
   - `triage::cleanup_dead_agents()` — detects dead agents, unclaims tasks
   - `sweep::reconcile_orphaned_tasks()` — catches orphaned InProgress tasks
   - Task-aware reaping — kills agents whose tasks are already Done/Failed
   
2. **Check ready tasks** — finds all tasks in `Open` status with satisfied dependencies
3. **Spawn agents** on ready tasks up to `max_agents` slots

### Dead Agent Detection (`src/commands/service/triage.rs:143-207`)
Three detection methods:
- **ProcessExited**: `!is_process_alive(agent.pid)` — PID no longer exists
- **PidReused**: PID exists but `/proc/<pid>/stat` shows different start time
- **HeartbeatTimeout**: No heartbeat within configured timeout (default: minutes), AND no recent stream activity

### What Happens When an Agent Dies (`triage.rs:288-361`)
When a dead agent is detected:
1. Agent status set to `Dead` in registry (but entry **stays**)
2. If task is still `InProgress`:
   - If `auto_triage` is enabled: runs LLM-based triage to assess progress
     - Verdict "done" → marks task Done
     - Verdict "continue" → resets task to Open (for respawn), increments `retry_count`, escalates model
     - Verdict "fail" → marks task Failed
   - If `auto_triage` disabled: simple unclaim (status → Open, `assigned = None`, increment `retry_count`)
3. Extracts token usage and session_id from stream files
4. Cleans up worktree

### Why Kill Isn't Permanent
**Because `kill` unclaims the task (sets it to `Open`), and the coordinator's next tick will see it as a ready task and spawn a new agent for it.**

The respawn is implicit — it's a side effect of the coordinator's main loop:
- `wg kill agent-5` → task becomes Open
- Coordinator tick → task is ready → spawn new agent

### Respawn Throttling (`coordinator.rs:3076-3174`)
Guards against rapid respawn loops:
- **Constants**: `RESPAWN_MAX_RAPID = 5` deaths in `RESPAWN_WINDOW_SECS = 300` (5 min)
- Examines task log for "process exited" / "PID reused" / "Triage:" entries
- Single death: normal, proceed immediately
- 2-4 deaths: exponential backoff (60s, 120s, 240s)
- 5+ deaths: **task auto-failed** ("Rapid respawn loop detected")

### Spawn Circuit Breaker (`coordinator.rs:3209-3300`)
Separate from respawn throttle — tracks **spawn failures** (process wouldn't start):
- `task.spawn_failures` counter incremented on each failure
- At `max_spawn_failures` threshold: task auto-failed

### Zero-Output Detection (`src/commands/service/zero_output.rs`)
Catches agents that start but never produce output (API call never returns):
- **Threshold**: 5 minutes with no stream data AND no active child processes
- Kills the zero-output agent, resets task to Open
- **Per-task circuit breaker**: 2 consecutive zero-output spawns → task failed
- **Global outage detection**: if ≥50% of alive agents have zero output → pause all spawning with exponential backoff

---

## 4. How Tasks Link to Subtasks

### No Parent-Child Relationship — Only `--after` Dependencies

**There is no explicit parent-child tracking in the Task struct.** The relevant fields:

```rust
pub struct Task {
    pub after: Vec<String>,    // "blocked_by" — tasks that must complete before this one
    pub before: Vec<String>,   // "blocks" — tasks that this blocks
    // No: parent, children, spawned_by, subtasks
}
```

When an agent creates subtasks via `wg add "Subtask" --after $WG_TASK_ID`:
- The subtask gets `after: ["parent-task-id"]`
- The parent task gets `before: ["subtask-id"]` (inverse edge, added automatically)
- But there's no semantic distinction between "subtask I created" and "another task I depend on"

### Implications for Tree-Kill
Since there's no parent-child tracking, a "tree kill" must:
1. Build a reverse dependency index from `before`/`after` edges
2. Walk all transitive dependents (already implemented: `collect_transitive_dependents` in `src/commands/mod.rs:152-165`)
3. Kill/abandon each discovered dependent

The `collect_transitive_dependents` function already exists and does exactly this — walks the reverse index recursively collecting all tasks that transitively depend on a given task.

---

## 5. Agent Reaping — Existing Cleanup Mechanisms

### What Exists Today

1. **Triage cleanup** (`triage::cleanup_dead_agents` — called every coordinator tick):
   - Detects dead agents (process exited, PID reused, heartbeat timeout)
   - Marks them `Dead` in registry **but does not remove them**
   - Unclaims their tasks

2. **Task-aware reaping** (`coordinator.rs:76-111`):
   - Finds agents whose tasks are already Done/Failed but whose process is still alive
   - Sends SIGTERM to free the agent slot
   - Marks them Dead in registry

3. **Orphaned task reconciliation** (`sweep::reconcile_orphaned_tasks` — `src/commands/sweep.rs:232`):
   - Catches InProgress tasks whose agents are Dead but weren't unclaimed (race condition)
   - Resets them to Open

4. **Zero-output sweep** (`zero_output.rs`):
   - Kills agents with no stream output for 5+ minutes
   - Circuit-breaks tasks with repeated zero-output failures

5. **Unix zombie reaping** (`src/commands/service/mod.rs:1189-1196`):
   - `reap_zombies()` calls `waitpid(-1, WNOHANG)` in a loop
   - Only reaps Unix process zombies (child processes of the daemon)
   - Does NOT clean up registry entries

### What's Missing: Registry Garbage Collection

**Dead agents accumulate forever in `registry.json`.** There is no mechanism to:
- Remove Dead/Done/Failed agent entries from the registry
- Prune the registry based on age or count
- Archive old entries

The only code that removes registry entries is `unregister_agent()`, called exclusively from `wg kill`. The triage/coordinator cleanup **never** calls it — agents are marked Dead but stay in the HashMap.

---

## Summary: Current Agent Lifecycle

```
spawn_agent_inner()          → register_agent_with_model()  → Agent in registry (Working)
                             → task.status = InProgress
                             → task.assigned = agent_id

[Agent works on task...]

Agent exits normally          → wg done / wg fail in agent's session
                             → task.status = Done/Failed

Coordinator tick              → cleanup_dead_agents()
                             → detect process exited
                             → agent.status = Dead (kept in registry)
                             → if task still InProgress: triage/unclaim
                             → if task Done/Failed: kill zombie process

wg kill <agent>               → SIGTERM/SIGKILL
                             → task.status = Open (unclaimed)
                             → unregister_agent() (REMOVED from registry)

[Task is Open again...]

Coordinator tick              → sees ready task
                             → spawn_agents_for_ready_tasks()
                             → (unless respawn-throttled or circuit-broken)
                             → new agent spawned
```

---

## Files That Need Modification

### For Dead Agent Reaping (`impl-reap`)
| File | Change |
|------|--------|
| `src/service/registry.rs` | Add `reap_dead_agents(&mut self, max_age: Duration) -> Vec<AgentEntry>` — remove Dead/Done/Failed entries older than threshold |
| `src/commands/service/triage.rs` | After marking agents Dead, optionally call reap on entries older than threshold |
| `src/commands/service/coordinator.rs` | Add periodic reap call (every N ticks, not every tick) to `cleanup_and_count_alive()` |
| `src/commands/mod.rs` | Add `pub mod reap;` for a new `wg reap` command |
| `src/commands/reap.rs` (new) | CLI command: `wg reap [--dry-run] [--max-age 1h]` — manually reap dead agents |
| `src/cli.rs` | Add `Reap` variant to `Commands` enum |
| `src/main.rs` | Route `Commands::Reap` to `commands::reap::run()` |

### For Tree-Kill (`impl-kill-tree`)
| File | Change |
|------|--------|
| `src/commands/kill.rs` | Add `run_tree(dir, task_id, force, json)` — kills agent for task + all transitive dependents |
| `src/commands/mod.rs` | `collect_transitive_dependents()` already exists — reuse it |
| `src/cli.rs` | Add `--tree` flag to `Kill` command |
| `src/main.rs` | Route `--tree` to `kill::run_tree()` |
| `src/commands/abandon.rs` | May need cascade logic — when tree-killing, downstream tasks should be abandoned, not just unclaimed |

### Gotchas and Race Conditions

1. **Kill-then-respawn race**: After `wg kill` unclaims a task, the coordinator may respawn it before the user expects. If tree-kill is meant to be permanent, it should set tasks to `Abandoned`, not `Open`.

2. **Registry lock contention**: Reaping should be batched (not per-agent) to minimize lock hold time. The flock is exclusive — long holds block all other registry operations.

3. **Concurrent triage + kill**: If `wg kill` fires while `cleanup_dead_agents()` is running, they may both try to modify the same task. The graph's `modify_graph()` function is atomic (write-to-temp-then-rename), but the registry lock prevents concurrent writes there.

4. **Worktree cleanup**: When tree-killing, the killed agents' worktrees must also be cleaned up. The existing worktree cleanup in triage (`triage.rs:477-534`) reads metadata.json from the agent's output directory — this should work for tree-kill too.

5. **Agent-task linkage for reaping**: When reaping dead agents, need to verify their task is truly Done/Failed before removing. If the task is still Open (rare edge case), the agent entry might still be referenced in logs.

6. **ID counter**: Reaping agents does NOT reset `next_agent_id` — IDs remain monotonically increasing. This is correct behavior (prevents ID reuse confusion).

7. **Zero-output state**: `ZeroOutputState` in `.wg/service/zero_output_state.json` tracks per-task respawn counts. Reaping doesn't need to touch this — it's keyed by task_id, not agent_id.
