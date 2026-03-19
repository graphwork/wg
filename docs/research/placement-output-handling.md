# Research: Placement Agent Spawning and Output Handling

## 1. File Paths Involved

| File | Role |
|------|------|
| `src/commands/eval_scaffold.rs:74-122` | `build_placement_context()` — builds the prompt/description for `.place-*` tasks |
| `src/commands/eval_scaffold.rs:140-196` | `scaffold_full_pipeline()` — creates `.place-*` tasks at publish time |
| `src/commands/service/coordinator.rs:744-801` | `build_placement_tasks()` — handles failed `.place-*` tasks (fallback-publish) |
| `src/commands/service/coordinator.rs:2492-2497` | Model resolution for `.place-*` tasks at spawn time |
| `src/commands/spawn/execution.rs:629-672` | `build_inner_command()` bare-mode branch — how bare exec mode launches Claude |
| `src/commands/spawn/execution.rs:139-140` | `resolve_task_exec_mode()` call — determines exec mode for a task |
| `src/commands/spawn/context.rs:379-401` | `resolve_task_exec_mode()` — priority chain: task.exec_mode > role.default_exec_mode > "full" |
| `src/service/executor.rs:332-444` | `build_prompt()` — scope-based prompt assembly |
| `src/config.rs:1452-1461` | `placer_agent` and `auto_place` config fields |
| `src/config.rs:570,644` | `DispatchRole::Placer` and its default tier (`Fast`) |
| `src/graph.rs:313,351-362` | `exec_mode`, `unplaced`, `place_near`, `place_before` task fields |

## 2. Placement Task Creation (Publish-Time Scaffolding)

`.place-*` tasks are created **atomically at `wg publish` time** by `scaffold_full_pipeline()` in `src/commands/eval_scaffold.rs`. The coordinator **no longer creates** them — it only handles failures.

### Creation logic (`eval_scaffold.rs:171-196`):

```rust
if config.agency.auto_place && graph.get_task(&place_task_id).is_none() {
    let placement_context = build_placement_context(graph, task_id);
    let placer_model = config.resolve_model_for_role(DispatchRole::Placer);
    let place_task = Task {
        id: format!(".place-{}", task_id),
        title: format!("Place: {}", task_id),
        description: Some(placement_context),
        status: Status::Open,
        after: vec![],  // No deps — runs first
        tags: vec!["placement", "agency"],
        exec_mode: Some("bare".to_string()),
        visibility: "internal",
        model: Some(placer_model.model),
        provider: placer_model.provider,
        agent: config.agency.placer_agent.clone(),
        ..Task::default()
    };
    graph.add_node(Node::Task(place_task));
}
```

### Pipeline wiring:
```
.place-foo → .assign-foo → foo → .flip-foo → .evaluate-foo
```
- `.place-*` has **no dependencies** (runs first in the pipeline)
- `.assign-*` depends on `.place-*` (waits for placement to finish)
- Source task is **paused** until `.assign-*` completes

## 3. Current Prompt Template for Placement Agents

The placement agent receives a **minimal context** via `build_placement_context()` (`eval_scaffold.rs:74-122`):

```
## Task to place
ID: {task_id}
Title: {task_title}
Existing deps: {comma-separated deps, if any}

## Active tasks (non-terminal)
- {task_id} ({task_title})
- ...

## Your job
Add `--after` or `--before` edges to the MAIN task '{task_id}' only.
Do NOT modify .assign-*, .flip-*, .evaluate-*, or any other dot-task.
Use: wg edit {task_id} --after <dep-id>  (or --before <dep-id>)
If no placement changes are needed, do nothing (no-op is valid).
```

**Key design decisions:**
- Description is **intentionally slim** — only task ID, title, and active task list
- **No full task description** is included (prevents scope creep where the agent tries to solve the problem instead of placing it)
- The `place_near` and `place_before` fields on the task are available but **not referenced in the prompt**

## 4. How Bare Exec Mode Works

Bare mode is defined in `src/commands/spawn/execution.rs:629-672`:

1. **System prompt**: The full prompt (from `build_prompt()`) is passed via `--system-prompt` flag
2. **User message**: Task title + description piped as stdin via a file:
   ```
   Complete this task:
   
   Title: {task_id}
   
   {task_description}
   ```
3. **Tool access**: Only `Bash(wg:*)` — the agent can only run `wg` commands
4. **Permissions**: `--dangerously-skip-permissions` (no confirmation prompts)
5. **Output format**: `stream-json` — JSONL output captured to `output.log`
6. **No file tools**: No Read, Glob, Grep, Write, Edit access

### Output capture flow:
- stdout → `raw_stream.jsonl` + `output.log` (tee'd)
- stderr → `output.log`
- The wrapper script (`src/commands/spawn/execution.rs:850-870`) captures both streams
- Agent is launched detached (`setsid()`) and the coordinator polls for process exit
- **After exit**: The coordinator's next cycle detects the agent has exited and checks the task status. If the agent called `wg done`, the task is marked Done. If the process crashed, the task may be left in-progress (handled by respawn throttle logic)

### Critical finding: **No structured output parsing happens**

Currently, when a bare-mode agent finishes:
1. The agent runs `wg` commands during execution (e.g., `wg edit`, `wg done`)
2. These commands modify the graph **inline** as the agent runs
3. After the agent exits, the coordinator just checks if the task status changed
4. **There is no post-processing of agent stdout/output**

The placement agent is expected to:
- Run `wg edit {task_id} --after <dep-id>` to add edges
- Run `wg done .place-{task_id}` to mark itself complete
- Or do nothing (no-op) if no placement needed

## 5. Failure Handling

`build_placement_tasks()` in `coordinator.rs:744-801`:
- Scans for `.place-*` tasks with `Status::Failed`
- Fallback-publishes the source task (unpauses it, adds "placed" tag)
- Tags the `.place-*` task with "fallback-published" to prevent re-processing
- **Never lets placement failure permanently block dispatch**

## 6. Recommended Approach for Structured Output Change

### Goal
Change placement agents to output a `wg edit` command (or "no-op") as their last line of stdout, with the system parsing and executing it.

### Recommended implementation locations

**Option A: Post-process in coordinator (recommended)**

Add output parsing in the coordinator's agent reaping/polling logic, specifically for `.place-*` tasks:

1. **In `src/commands/service/coordinator.rs`**: After detecting a `.place-*` agent has exited, read its `output.log`, extract the last line, and parse it:
   - If it matches `wg edit <task-id> --after <dep> [--before <dep>]`: execute the edit
   - If it matches `no-op` or is empty: do nothing (placement not needed)
   - If unparseable or agent failed: mark `.place-*` as failed (triggers fallback-publish)

2. **Modify the placement prompt** (`build_placement_context()` in `eval_scaffold.rs`): Change instructions from "run `wg edit`" to "output a single `wg edit` command as your last line, or `no-op` if no changes needed. Do NOT run the command yourself."

3. **Remove `Bash(wg:*)` from bare mode tools for placement tasks**: Since the agent no longer runs commands, it just outputs text. Could use a new exec mode or a flag.

**Option B: Wrapper script approach**

Modify the wrapper script (`build_wrapper_script()`) to capture the last line of agent output and execute it. Less clean but simpler.

### Key changes needed

| File | Change |
|------|--------|
| `src/commands/eval_scaffold.rs:110-119` | Update prompt to request structured output (command on last line) |
| `src/commands/service/coordinator.rs` | Add post-exit output parsing for `.place-*` tasks |
| `src/commands/spawn/execution.rs:629-672` | Optionally restrict tools further for placement (remove `Bash(wg:*)`) |

### Considerations

1. **Backward compatibility**: The change should gracefully handle agents that still use the old approach (running `wg edit` directly)
2. **Error handling**: If agent output is garbage, fall back to failed → fallback-publish
3. **Validation**: Parse the `wg edit` command to ensure it only modifies the target task (prevent agents from editing other tasks)
4. **The `place_near` and `place_before` fields** could be surfaced in the placement prompt to give the agent more hints
5. **Token efficiency**: Placement agents use `Tier::Fast` (cheapest model) — the prompt should stay slim
